// SPDX-License-Identifier: GPL-3.0-or-later

// Nia's ThinLTO backend.
//
// LLVM's in-process ThinLTO backend optimizes every module after the thin
// link with a pass builder of its own, which only pass plugins loaded from
// files can extend. Nia links LLVM statically and has no such files, so with
// that backend Nia's passes would run before the thin link and never after
// it, where imported functions have been inlined and their facts are first in
// view. This backend runs what `lto::thinBackend` runs before optimization,
// optimizes with LLVM's ThinLTO post-link pipeline built by a pass builder
// that has Nia's passes, and leaves code generation to `lto::thinBackend`
// with `CodeGenOnly` set. Each step below mirrors one of
// llvm/lib/LTO/LTOBackend.cpp or LTO.cpp, in the same order, and calls the
// same configuration hooks.
//
// A backend object's cache key is LLVM's with the pipeline identity added:
// a change in Nia's passes leaves the bitcode inputs, and so LLVM's key, as
// they were, but must not reuse objects that the earlier passes produced.

#include "llvm_nia.h"

#include "llvm/ADT/DenseSet.h"
#include "llvm/ADT/MapVector.h"
#include "llvm/ADT/STLExtras.h"
#include "llvm/Analysis/TargetLibraryInfo.h"
#include "llvm/Bitcode/BitcodeReader.h"
#include "llvm/IR/Module.h"
#include "llvm/IR/ModuleSummaryIndex.h"
#include "llvm/IR/Verifier.h"
#include "llvm/LTO/Config.h"
#include "llvm/LTO/LTOBackend.h"
#include "llvm/MC/TargetRegistry.h"
#include "llvm/Passes/StandardInstrumentations.h"
#include "llvm/Support/Caching.h"
#include "llvm/Support/Error.h"
#include "llvm/Target/TargetMachine.h"
#include "llvm/TargetParser/SubtargetFeature.h"
#include "llvm/Transforms/IPO/FunctionImport.h"
#include "llvm/Transforms/IPO/WholeProgramDevirt.h"
#include "llvm/Transforms/Utils/FunctionImportUtils.h"

#include <map>
#include <memory>
#include <mutex>
#include <optional>
#include <string>
#include <utility>
#include <vector>

using namespace llvm;

namespace {

Error failure(const Twine &Message) {
  return make_error<StringError>(Message, inconvertibleErrorCode());
}

// The backend mirrors how `lto::opt` and `lto::thinBackend` run for the
// configuration Nia's bridge builds; profiles, remarks, a custom pipeline or
// code generation alone take paths there that it does not mirror.
Error checkConfiguration(const lto::Config &Conf) {
  if (!Conf.SampleProfile.empty() || !Conf.CSIRProfile.empty() ||
      Conf.RunCSIRInstr || Conf.AddFSDiscriminator ||
      !Conf.OptPipeline.empty() || !Conf.AAPipeline.empty() ||
      !Conf.RemarksFilename.empty() || Conf.CodeGenOnly ||
      Conf.TimeTraceEnabled)
    return failure("Nia's ThinLTO backend received an LTO configuration "
                   "it does not support");
  return Error::success();
}

// Whether `M` has nothing to optimize, as `lto::opt` decides it.
bool isEmptyModule(const Module &M) {
  return M.empty() && M.global_empty() && M.named_metadata_empty() &&
         M.getModuleInlineAsm().empty();
}

// The target machine that `lto::thinBackend` creates for `M`.
Expected<std::unique_ptr<TargetMachine>>
createTargetMachine(const lto::Config &Conf, Module &M) {
  if (!Conf.OverrideTriple.empty())
    M.setTargetTriple(Triple(Conf.OverrideTriple));
  else if (M.getTargetTriple().empty())
    M.setTargetTriple(Triple(Conf.DefaultTriple));
  std::string Message;
  const Target *TheTarget =
      TargetRegistry::lookupTarget(M.getTargetTriple(), Message);
  if (!TheTarget)
    return failure(Message);

  const Triple &TheTriple = M.getTargetTriple();
  SubtargetFeatures Features;
  Features.getDefaultSubtargetFeatures(TheTriple);
  for (const std::string &Attribute : Conf.MAttrs)
    Features.AddFeature(Attribute);

  std::optional<Reloc::Model> RelocModel;
  if (Conf.RelocModel)
    RelocModel = *Conf.RelocModel;
  else if (M.getModuleFlag("PIC Level"))
    RelocModel =
        M.getPICLevel() == PICLevel::NotPIC ? Reloc::Static : Reloc::PIC_;
  std::optional<CodeModel::Model> CodeModel;
  if (Conf.CodeModel)
    CodeModel = *Conf.CodeModel;
  else
    CodeModel = M.getCodeModel();
  TargetOptions Options = Conf.Options;
  if (Options.MCOptions.ABIName.empty())
    Options.MCOptions.ABIName = M.getTargetABIFromMD();

  std::unique_ptr<TargetMachine> TM(TheTarget->createTargetMachine(
      TheTriple, Conf.CPU, Features.getString(), Options, RelocModel,
      CodeModel, Conf.CGOptLevel));
  if (!TM)
    return failure("LLVM could not create a ThinLTO target machine");
  if (std::optional<uint64_t> Threshold = M.getLargeDataThreshold())
    TM->setLargeDataThreshold(*Threshold);
  return std::move(TM);
}

// Turns the definitions the thin link found dead into declarations, and
// removes those nothing uses.
void dropDeadSymbols(Module &M, const GVSummaryMapTy &DefinedGlobals,
                     const ModuleSummaryIndex &Index) {
  std::vector<GlobalValue *> Dead;
  for (GlobalValue &Value : M.global_values()) {
    std::optional<GlobalValue::GUID> GUID = Value.getGUIDIfAssigned();
    if (!GUID)
      continue;
    if (GlobalValueSummary *Summary = DefinedGlobals.lookup(*GUID))
      if (!Index.isGlobalValueLive(Summary)) {
        Dead.push_back(&Value);
        convertToDeclaration(Value);
      }
  }
  for (GlobalValue *Value : Dead) {
    Value->removeDeadConstantUsers();
    if (Value->use_empty())
      Value->eraseFromParent();
  }
}

OptimizationLevel optimizationLevel(unsigned Level) {
  switch (Level) {
  case 0:
    return OptimizationLevel::O0;
  case 1:
    return OptimizationLevel::O1;
  case 3:
    return OptimizationLevel::O3;
  default:
    return OptimizationLevel::O2;
  }
}

// Optimizes `M` after import with LLVM's ThinLTO post-link pipeline, set up
// as `lto::opt` sets it up, from a pass builder that has Nia's passes.
void optimize(const lto::Config &Conf, TargetMachine &TM, Module &M,
              const ModuleSummaryIndex &Index,
              ArrayRef<StringRef> BitcodeLibFuncs) {
  LoopAnalysisManager LAM;
  FunctionAnalysisManager FAM;
  CGSCCAnalysisManager CGAM;
  ModuleAnalysisManager MAM;
  PassInstrumentationCallbacks PIC;
  StandardInstrumentations SI(M.getContext(), Conf.DebugPassManager,
                              Conf.VerifyEach);
  SI.registerCallbacks(PIC, &MAM);
  PassBuilder PB(&TM, Conf.PTO, std::nullopt, &PIC);
  niaRegisterPasses(PB);

  TargetLibraryInfoImpl LibraryInfo(TM.getTargetTriple(), TM.Options.VecLib);
  if (Conf.Freestanding)
    LibraryInfo.disableAllFunctions();
  // A library function that the link's bitcode could have defined, but that
  // no module defines, has lost its only definition: calls to it must not be
  // introduced.
  DenseSet<StringRef> InBitcode(BitcodeLibFuncs.begin(),
                                BitcodeLibFuncs.end());
  TargetLibraryInfo Library(LibraryInfo);
  for (unsigned Index = 0,
                End = static_cast<unsigned>(LibFunc::NumLibFuncs);
       Index != End; ++Index) {
    LibFunc Function = static_cast<LibFunc>(Index);
    if (InBitcode.contains(Library.getName(Function)))
      LibraryInfo.setUnavailable(Function);
  }
  FAM.registerPass([&] { return TargetLibraryAnalysis(LibraryInfo); });

  PB.registerModuleAnalyses(MAM);
  PB.registerCGSCCAnalyses(CGAM);
  PB.registerFunctionAnalyses(FAM);
  PB.registerLoopAnalyses(LAM);
  PB.crossRegisterProxies(LAM, FAM, CGAM, MAM);

  ModulePassManager MPM;
  if (!Conf.DisableVerify)
    MPM.addPass(VerifierPass());
  MPM.addPass(
      PB.buildThinLTODefaultPipeline(optimizationLevel(Conf.OptLevel), &Index));
  if (!Conf.DisableVerify)
    MPM.addPass(VerifierPass());
  MPM.run(M, MAM);
}

// Runs on `M` what `lto::thinBackend` runs before code generation, with
// Nia's optimization in place of LLVM's: promotion, dead symbol removal,
// internalization, import, and optimization. False when a hook stopped the
// task.
Expected<bool> prepare(const lto::Config &Conf, unsigned Task, Module &M,
                       const ModuleSummaryIndex &Index,
                       const FunctionImporter::ImportMapTy &ImportList,
                       const GVSummaryMapTy &DefinedGlobals,
                       MapVector<StringRef, BitcodeModule> &ModuleMap,
                       ArrayRef<StringRef> BitcodeLibFuncs) {
  Expected<std::unique_ptr<TargetMachine>> TM = createTargetMachine(Conf, M);
  if (!TM)
    return TM.takeError();
  M.setPartialSampleProfileRatio(Index);
  if (Conf.PreOptModuleHook && !Conf.PreOptModuleHook(Task, M))
    return false;

  // When linking an ELF shared object, dso_local is dropped from
  // declarations; LLVM does so for any position-independent code.
  bool ClearDSOLocalOnDeclarations =
      (*TM)->getTargetTriple().isOSBinFormatELF() &&
      (*TM)->getRelocationModel() != Reloc::Static &&
      M.getPIELevel() == PIELevel::Default;
  renameModuleForThinLTO(M, Index, ClearDSOLocalOnDeclarations);
  dropDeadSymbols(M, DefinedGlobals, Index);
  thinLTOFinalizeInModule(M, DefinedGlobals, /*PropagateAttrs=*/true);
  if (Conf.PostPromoteModuleHook && !Conf.PostPromoteModuleHook(Task, M))
    return false;

  if (!DefinedGlobals.empty())
    thinLTOInternalizeModule(M, DefinedGlobals);
  if (Conf.PostInternalizeModuleHook &&
      !Conf.PostInternalizeModuleHook(Task, M))
    return false;

  auto ModuleLoader =
      [&](StringRef Identifier) -> Expected<std::unique_ptr<Module>> {
    auto Source = ModuleMap.find(Identifier);
    if (Source == ModuleMap.end())
      return failure("ThinLTO imports from `" + Identifier +
                     "`, which is not an input of the link");
    return Source->second.getLazyModule(M.getContext(),
                                        /*ShouldLazyLoadMetadata=*/true,
                                        /*IsImporting=*/true);
  };
  FunctionImporter Importer(Index, ModuleLoader, ClearDSOLocalOnDeclarations);
  if (Expected<bool> Imported = Importer.importFunctions(M, ImportList);
      !Imported)
    return Imported.takeError();
  // After import, so that imported code is updated as well.
  updatePublicTypeTestCalls(M, Index.withWholeProgramVisibility());
  if (Conf.PostImportModuleHook && !Conf.PostImportModuleHook(Task, M))
    return false;

  if (!isEmptyModule(M))
    optimize(Conf, **TM, M, Index, BitcodeLibFuncs);
  return !Conf.PostOptModuleHook || Conf.PostOptModuleHook(Task, M);
}

// Runs each module's backend on a thread of its own, as LLVM's in-process
// backend does, and consults the cache first.
class NiaThinBackend final : public lto::ThinBackendProc {
public:
  NiaThinBackend(
      const lto::Config &Conf, ModuleSummaryIndex &CombinedIndex,
      ThreadPoolStrategy Parallelism,
      const DenseMap<StringRef, GVSummaryMapTy> &ModuleToDefinedGVSummaries,
      AddStreamFn AddStream, FileCache Cache,
      ArrayRef<StringRef> BitcodeLibFuncs, std::string PipelineIdentity)
      : ThinBackendProc(Conf, CombinedIndex, ModuleToDefinedGVSummaries,
                        /*OnWrite=*/nullptr, /*ShouldEmitImportsFiles=*/false,
                        Parallelism),
        AddStream(std::move(AddStream)), Cache(std::move(Cache)),
        BitcodeLibFuncs(BitcodeLibFuncs),
        PipelineIdentity(std::move(PipelineIdentity)) {
    CfiFunctionDefs.insert_range(
        CombinedIndex.cfiFunctionDefs().getExportedThinLTOGUIDs());
    CfiFunctionDecls.insert_range(
        CombinedIndex.cfiFunctionDecls().getExportedThinLTOGUIDs());
  }

  Error start(
      unsigned Task, BitcodeModule BM,
      const FunctionImporter::ImportMapTy &ImportList,
      const FunctionImporter::ExportSetTy &ExportList,
      const std::map<GlobalValue::GUID, GlobalValue::LinkageTypes> &ResolvedODR,
      MapVector<StringRef, BitcodeModule> &ModuleMap) override {
    if (Error Unsupported = checkConfiguration(Conf))
      return Unsupported;
    auto Defined = ModuleToDefinedGVSummaries.find(BM.getModuleIdentifier());
    if (Defined == ModuleToDefinedGVSummaries.end())
      return failure("ThinLTO module `" + BM.getModuleIdentifier() +
                     "` has no summary");
    const GVSummaryMapTy &DefinedGlobals = Defined->second;
    // The import and export lists, the resolutions and the module map belong
    // to the link and outlive `wait`, which joins every task.
    BackendThreadPool.async([this, Task, BM, &ImportList, &ExportList,
                             &ResolvedODR, &DefinedGlobals, &ModuleMap] {
      Error Failed = run(Task, BM, ImportList, ExportList, ResolvedODR,
                         DefinedGlobals, ModuleMap);
      if (!Failed)
        return;
      std::unique_lock<std::mutex> Lock(ErrMu);
      if (Err)
        Err = joinErrors(std::move(*Err), std::move(Failed));
      else
        Err = std::move(Failed);
    });
    return Error::success();
  }

private:
  AddStreamFn AddStream;
  FileCache Cache;
  ArrayRef<StringRef> BitcodeLibFuncs;
  std::string PipelineIdentity;
  DenseSet<GlobalValue::GUID> CfiFunctionDefs;
  DenseSet<GlobalValue::GUID> CfiFunctionDecls;

  Error run(
      unsigned Task, BitcodeModule BM,
      const FunctionImporter::ImportMapTy &ImportList,
      const FunctionImporter::ExportSetTy &ExportList,
      const std::map<GlobalValue::GUID, GlobalValue::LinkageTypes> &ResolvedODR,
      const GVSummaryMapTy &DefinedGlobals,
      MapVector<StringRef, BitcodeModule> &ModuleMap) {
    auto Backend = [&](AddStreamFn Stream) -> Error {
      lto::LTOLLVMContext Context(Conf);
      Expected<std::unique_ptr<Module>> M = BM.parseModule(Context);
      if (!M)
        return M.takeError();
      Expected<bool> Prepared =
          prepare(Conf, Task, **M, CombinedIndex, ImportList, DefinedGlobals,
                  ModuleMap, BitcodeLibFuncs);
      if (!Prepared)
        return Prepared.takeError();
      if (!*Prepared)
        return Error::success();
      return lto::thinBackend(Conf, Task, Stream, **M, CombinedIndex,
                              ImportList, DefinedGlobals, &ModuleMap,
                              /*CodeGenOnly=*/true, BitcodeLibFuncs);
    };

    StringRef ModuleID = BM.getModuleIdentifier();
    // Without a cache, or without a hash of the module to key it by, the
    // module is always compiled.
    if (!Cache.isValid() || !CombinedIndex.modulePaths().count(ModuleID) ||
        all_of(CombinedIndex.getModuleHash(ModuleID),
               [](uint32_t Word) { return Word == 0; }))
      return Backend(AddStream);

    std::string Key = computeLTOCacheKey(
        Conf, CombinedIndex, ModuleID, ImportList, ExportList, ResolvedODR,
        DefinedGlobals, CfiFunctionDefs, CfiFunctionDecls);
    Key = recomputeLTOCacheKey(Key, PipelineIdentity);
    Expected<AddStreamFn> CacheStream = Cache(Task, Key, ModuleID);
    if (!CacheStream)
      return CacheStream.takeError();
    // No stream means the cache had the object and has added it already.
    if (*CacheStream)
      return Backend(*CacheStream);
    return Error::success();
  }
};

} // namespace

lto::ThinBackend niaThinBackend(ThreadPoolStrategy Parallelism,
                                std::string PipelineIdentity) {
  auto Create =
      [=](const lto::Config &Conf, ModuleSummaryIndex &CombinedIndex,
          const DenseMap<StringRef, GVSummaryMapTy> &ModuleToDefinedGVSummaries,
          AddStreamFn AddStream, FileCache Cache,
          ArrayRef<StringRef> BitcodeLibFuncs)
      -> std::unique_ptr<lto::ThinBackendProc> {
    return std::make_unique<NiaThinBackend>(
        Conf, CombinedIndex, Parallelism, ModuleToDefinedGVSummaries,
        std::move(AddStream), std::move(Cache), BitcodeLibFuncs,
        PipelineIdentity);
  };
  return lto::ThinBackend(std::move(Create), Parallelism);
}
