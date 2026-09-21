// SPDX-License-Identifier: GPL-3.0-or-later

#include "llvm-c/Core.h"
#include "llvm-c/TargetMachine.h"
#include "llvm/ADT/DenseMap.h"
#include "llvm/ADT/SmallVector.h"
#include "llvm/ADT/StringRef.h"
#include "llvm/IR/DiagnosticInfo.h"
#include "llvm/IR/DiagnosticPrinter.h"
#include "llvm/IR/LegacyPassManager.h"
#include "llvm/IR/Module.h"
#include "llvm/IR/PassManager.h"
#include "llvm/LTO/LTO.h"
#include "llvm/LTO/LTOBackend.h"
#include "llvm/Passes/PassBuilder.h"
#include "llvm/Support/CBindingWrapping.h"
#include "llvm/Support/Caching.h"
#include "llvm/Support/Endian.h"
#include "llvm/Support/FileSystem.h"
#include "llvm/Support/MemoryBuffer.h"
#include "llvm/Support/Path.h"
#include "llvm/Support/SHA256.h"
#include "llvm/Support/TargetSelect.h"
#include "llvm/Support/Threading.h"
#include "llvm/Support/raw_ostream.h"
#include "llvm/Target/TargetMachine.h"
#include "llvm/Transforms/IPO/ThinLTOBitcodeWriter.h"

#include <algorithm>
#include <array>
#include <chrono>
#include <cstddef>
#include <cstdint>
#include <map>
#include <memory>
#include <mutex>
#include <optional>
#include <string>
#include <unordered_map>
#include <unordered_set>
#include <utility>
#include <vector>

using namespace llvm;

namespace {

using Clock = std::chrono::steady_clock;

struct ByteSlice {
  const uint8_t *Data;
  size_t Len;
};

struct ThinInput {
  ByteSlice Name;
  ByteSlice Bitcode;
};

struct ThinConfig {
  ByteSlice CPU;
  ByteSlice Features;
  const ByteSlice *PreservedSymbols;
  size_t PreservedSymbolCount;
  uint32_t Optimization;
  uint32_t Parallelism;
  uint8_t Freestanding;
  ByteSlice CacheDirectory;
  ByteSlice CacheMagic;
  uint32_t CacheReleaseCompatibility;
};

struct OwnedBuffer {
  std::string Bytes;
  std::string Error;
};

struct ThinObject {
  uint32_t Task = 0;
  std::string ModuleName;
  std::string CacheKey;
  std::string Bytes;
};

struct ThinDiagnostic {
  uint32_t Severity = 0;
  std::string Message;
};

struct ThinTimings {
  uint64_t ThinLinkNs = 0;
  uint64_t BackendNs = 0;
  uint64_t PromotionNs = 0;
  uint64_t InternalizationNs = 0;
  uint64_t ImportNs = 0;
  uint64_t OptimizationNs = 0;
  uint64_t CodegenNs = 0;
};

struct ThinCacheStats {
  uint64_t Hits = 0;
  uint64_t Misses = 0;
  uint64_t Corrupt = 0;
  uint64_t ReadErrors = 0;
  uint64_t WriteErrors = 0;
};

struct ThinResult {
  std::vector<ThinObject> Objects;
  std::vector<ThinDiagnostic> Diagnostics;
  std::unordered_map<unsigned, std::string> CacheKeys;
  std::string Error;
  ThinTimings Timings;
  ThinCacheStats Cache;
};

uint64_t elapsedNs(Clock::time_point Start, Clock::time_point End) {
  return static_cast<uint64_t>(
      std::chrono::duration_cast<std::chrono::nanoseconds>(End - Start).count());
}

StringRef asStringRef(ByteSlice Slice) {
  if (Slice.Len == 0)
    return {};
  return StringRef(reinterpret_cast<const char *>(Slice.Data), Slice.Len);
}

std::string errorText(Error Err) {
  if (!Err)
    return {};
  return toString(std::move(Err));
}

OptimizationLevel optimizationLevel(uint32_t Value) {
  switch (Value) {
  case 0:
    return OptimizationLevel::O0;
  case 1:
    return OptimizationLevel::O1;
  case 2:
    return OptimizationLevel::O2;
  case 3:
    return OptimizationLevel::O3;
  default:
    return OptimizationLevel::O0;
  }
}

CodeGenOptLevel codegenOptimizationLevel(uint32_t Value) {
  switch (Value) {
  case 0:
    return CodeGenOptLevel::None;
  case 1:
    return CodeGenOptLevel::Less;
  case 2:
    return CodeGenOptLevel::Default;
  case 3:
    return CodeGenOptLevel::Aggressive;
  default:
    return CodeGenOptLevel::None;
  }
}

uint32_t diagnosticSeverity(DiagnosticSeverity Severity) {
  switch (Severity) {
  case DS_Error:
    return 0;
  case DS_Warning:
    return 1;
  case DS_Remark:
    return 2;
  case DS_Note:
    return 3;
  }
  return 0;
}

class StageRecorder {
public:
  enum class Stage {
    Promotion,
    Internalization,
    Import,
    Optimization,
    Codegen,
  };

  void begin(unsigned Task) {
    std::lock_guard<std::mutex> Lock(Mutex);
    Starts[Task] = Clock::now();
  }

  void finish(unsigned Task, Stage Current) {
    auto Now = Clock::now();
    std::lock_guard<std::mutex> Lock(Mutex);
    auto It = Starts.find(Task);
    if (It == Starts.end()) {
      Starts[Task] = Now;
      return;
    }
    uint64_t Elapsed = elapsedNs(It->second, Now);
    It->second = Now;
    switch (Current) {
    case Stage::Promotion:
      Timings.PromotionNs += Elapsed;
      break;
    case Stage::Internalization:
      Timings.InternalizationNs += Elapsed;
      break;
    case Stage::Import:
      Timings.ImportNs += Elapsed;
      break;
    case Stage::Optimization:
      Timings.OptimizationNs += Elapsed;
      break;
    case Stage::Codegen:
      Timings.CodegenNs += Elapsed;
      Starts.erase(It);
      break;
    }
  }

  ThinTimings snapshot() const {
    std::lock_guard<std::mutex> Lock(Mutex);
    return Timings;
  }

private:
  mutable std::mutex Mutex;
  std::unordered_map<unsigned, Clock::time_point> Starts;
  ThinTimings Timings;
};

constexpr size_t CacheMagicSize = 8;
constexpr size_t CacheChecksumSize = 32;
constexpr size_t CacheHeaderSize = CacheMagicSize + sizeof(uint32_t) +
                                   sizeof(uint64_t) + CacheChecksumSize;

enum class CacheRead { NotFound, Hit, Corrupt, Error };

struct CacheReadResult {
  CacheRead Status = CacheRead::NotFound;
  std::string Bytes;
};

SmallString<256> cacheEntryPath(StringRef Directory, StringRef Key) {
  SmallString<256> Path(Directory);
  sys::path::append(Path, "llvmcache-" + Key);
  return Path;
}

CacheReadResult readCacheEntry(StringRef Path, ArrayRef<uint8_t> Magic,
                               uint32_t ReleaseCompatibility) {
  ErrorOr<std::unique_ptr<MemoryBuffer>> Buffer =
      MemoryBuffer::getFile(Path, /*IsText=*/false,
                            /*RequiresNullTerminator=*/false);
  if (!Buffer) {
    if (Buffer.getError() == std::errc::no_such_file_or_directory)
      return {CacheRead::NotFound, {}};
    return {CacheRead::Error, {}};
  }
  StringRef Record = (*Buffer)->getBuffer();
  if (Magic.size() != CacheMagicSize || Record.size() < CacheHeaderSize ||
      !std::equal(Magic.begin(), Magic.end(),
                  reinterpret_cast<const uint8_t *>(Record.data())) ||
      support::endian::read32le(Record.data() + CacheMagicSize) !=
          ReleaseCompatibility) {
    return {CacheRead::Corrupt, {}};
  }
  uint64_t PayloadLength = support::endian::read64le(
      Record.data() + CacheMagicSize + sizeof(uint32_t));
  if (PayloadLength != Record.size() - CacheHeaderSize)
    return {CacheRead::Corrupt, {}};
  ArrayRef<uint8_t> Payload(
      reinterpret_cast<const uint8_t *>(Record.data() + CacheHeaderSize),
      static_cast<size_t>(PayloadLength));
  std::array<uint8_t, CacheChecksumSize> Actual = SHA256::hash(Payload);
  const uint8_t *Expected = reinterpret_cast<const uint8_t *>(
      Record.data() + CacheMagicSize + sizeof(uint32_t) + sizeof(uint64_t));
  if (!std::equal(Actual.begin(), Actual.end(), Expected))
    return {CacheRead::Corrupt, {}};
  return {CacheRead::Hit,
          std::string(reinterpret_cast<const char *>(Payload.data()),
                      Payload.size())};
}

bool writeCacheEntry(StringRef Directory, StringRef Path,
                     ArrayRef<uint8_t> Magic,
                     uint32_t ReleaseCompatibility, StringRef Payload) {
  if (Magic.size() != CacheMagicSize)
    return false;
  if (std::error_code EC =
          sys::fs::create_directories(Directory, /*IgnoreExisting=*/true))
    return false;
  SmallString<256> TempModel(Directory);
  sys::path::append(TempModel, "nia-thin-%%%%%%.tmp");
  Expected<sys::fs::TempFile> Temp = sys::fs::TempFile::create(
      TempModel, sys::fs::owner_read | sys::fs::owner_write);
  if (!Temp)
    return false;

  std::array<char, sizeof(uint32_t) + sizeof(uint64_t)> Lengths{};
  support::endian::write32le(Lengths.data(), ReleaseCompatibility);
  support::endian::write64le(Lengths.data() + sizeof(uint32_t), Payload.size());
  ArrayRef<uint8_t> PayloadBytes(
      reinterpret_cast<const uint8_t *>(Payload.data()), Payload.size());
  std::array<uint8_t, CacheChecksumSize> Checksum = SHA256::hash(PayloadBytes);
  raw_fd_ostream OS(Temp->FD, /*ShouldClose=*/false);
  OS.write(reinterpret_cast<const char *>(Magic.data()), Magic.size());
  OS.write(Lengths.data(), Lengths.size());
  OS.write(reinterpret_cast<const char *>(Checksum.data()), Checksum.size());
  OS.write(Payload.data(), Payload.size());
  OS.flush();
  if (OS.has_error()) {
    OS.clear_error();
    consumeError(Temp->discard());
    return false;
  }
  if (Error Err = Temp->keep(Path)) {
    consumeError(std::move(Err));
    return false;
  }
  return true;
}

class ResultStream final : public CachedFileStream {
public:
  static std::unique_ptr<ResultStream>
  create(unsigned Task, std::string ModuleName, ThinResult &Result,
         std::mutex &ResultMutex, StageRecorder &Stages,
         std::string CacheDirectory, std::string CachePath,
         std::array<uint8_t, CacheMagicSize> CacheMagic,
         uint32_t CacheReleaseCompatibility) {
    return std::unique_ptr<ResultStream>(new ResultStream(
        Task, std::move(ModuleName), Result, ResultMutex, Stages,
        std::move(CacheDirectory), std::move(CachePath), CacheMagic,
        CacheReleaseCompatibility,
        std::make_shared<SmallVector<char, 0>>()));
  }

  ~ResultStream() override { OS.reset(); }

  Error commit() override {
    StringRef Bytes(Storage->data(), Storage->size());
    bool CacheWriteFailed =
        !CachePath.empty() &&
        !writeCacheEntry(CacheDirectory, CachePath, CacheMagic,
                         CacheReleaseCompatibility, Bytes);
    {
      std::lock_guard<std::mutex> Lock(ResultMutex);
      Result.Cache.WriteErrors += CacheWriteFailed;
      Result.Objects.push_back(ThinObject{
          Task, std::move(ModuleName), Result.CacheKeys[Task],
          Bytes.str()});
    }
    Stages.finish(Task, StageRecorder::Stage::Codegen);
    return CachedFileStream::commit();
  }

private:
  ResultStream(unsigned Task, std::string ModuleName, ThinResult &Result,
               std::mutex &ResultMutex, StageRecorder &Stages,
               std::string CacheDirectory, std::string CachePath,
               std::array<uint8_t, CacheMagicSize> CacheMagic,
               uint32_t CacheReleaseCompatibility,
               std::shared_ptr<SmallVector<char, 0>> Storage)
      : CachedFileStream(std::make_unique<raw_svector_ostream>(*Storage)),
        Storage(std::move(Storage)), Task(Task),
        ModuleName(std::move(ModuleName)), Result(Result),
        ResultMutex(ResultMutex), Stages(Stages),
        CacheDirectory(std::move(CacheDirectory)),
        CachePath(std::move(CachePath)), CacheMagic(CacheMagic),
        CacheReleaseCompatibility(CacheReleaseCompatibility) {}

  std::shared_ptr<SmallVector<char, 0>> Storage;
  unsigned Task;
  std::string ModuleName;
  ThinResult &Result;
  std::mutex &ResultMutex;
  StageRecorder &Stages;
  std::string CacheDirectory;
  std::string CachePath;
  std::array<uint8_t, CacheMagicSize> CacheMagic;
  uint32_t CacheReleaseCompatibility;
};

std::optional<std::string> initializeTargets() {
  static std::once_flag Once;
  static std::optional<std::string> Error;
  std::call_once(Once, [] {
    if (InitializeNativeTarget())
      Error = "LLVM failed to initialize the native target";
    else if (InitializeNativeTargetAsmPrinter())
      Error = "LLVM failed to initialize the native assembly printer";
    else if (InitializeNativeTargetAsmParser())
      Error = "LLVM failed to initialize the native assembly parser";
  });
  return Error;
}

} // namespace

extern "C" {

OwnedBuffer *nia_llvm_emit_thin_lto_bitcode(LLVMModuleRef RawModule,
                                             LLVMTargetMachineRef RawTarget,
                                             uint32_t Optimization) {
  auto Result = std::make_unique<OwnedBuffer>();
  if (!RawModule || !RawTarget) {
    Result->Error = "ThinLTO pre-link emission received a null LLVM handle";
    return Result.release();
  }
  if (Optimization > 3) {
    Result->Error = "ThinLTO pre-link emission received an invalid optimization level";
    return Result.release();
  }

  Module &M = *unwrap(RawModule);
  TargetMachine *TM = reinterpret_cast<TargetMachine *>(RawTarget);
  LoopAnalysisManager LAM;
  FunctionAnalysisManager FAM;
  CGSCCAnalysisManager CGAM;
  ModuleAnalysisManager MAM;
  PassBuilder PB(TM);
  PB.registerModuleAnalyses(MAM);
  PB.registerCGSCCAnalyses(CGAM);
  PB.registerFunctionAnalyses(FAM);
  PB.registerLoopAnalyses(LAM);
  PB.crossRegisterProxies(LAM, FAM, CGAM, MAM);

  raw_string_ostream OS(Result->Bytes);
  ModulePassManager MPM =
      PB.buildThinLTOPreLinkDefaultPipeline(optimizationLevel(Optimization));
  MPM.addPass(ThinLTOBitcodeWriterPass(OS, nullptr));
  MPM.run(M, MAM);
  OS.flush();
  return Result.release();
}

const uint8_t *nia_llvm_owned_buffer_data(const OwnedBuffer *Buffer) {
  if (!Buffer || Buffer->Bytes.empty())
    return nullptr;
  return reinterpret_cast<const uint8_t *>(Buffer->Bytes.data());
}

size_t nia_llvm_owned_buffer_len(const OwnedBuffer *Buffer) {
  return Buffer ? Buffer->Bytes.size() : 0;
}

const uint8_t *nia_llvm_owned_buffer_error(const OwnedBuffer *Buffer) {
  if (!Buffer || Buffer->Error.empty())
    return nullptr;
  return reinterpret_cast<const uint8_t *>(Buffer->Error.data());
}

size_t nia_llvm_owned_buffer_error_len(const OwnedBuffer *Buffer) {
  return Buffer ? Buffer->Error.size() : 0;
}

void nia_llvm_owned_buffer_free(OwnedBuffer *Buffer) { delete Buffer; }

ThinResult *nia_llvm_run_thin_lto(const ThinInput *Inputs, size_t InputCount,
                                  const ThinConfig *InputConfig) {
  auto Result = std::make_unique<ThinResult>();
  if (!InputConfig || (!Inputs && InputCount != 0)) {
    Result->Error = "ThinLTO received invalid input pointers";
    return Result.release();
  }
  if (InputCount == 0) {
    Result->Error = "ThinLTO requires at least one bitcode input";
    return Result.release();
  }
  if (InputConfig->Optimization > 3 || InputConfig->Parallelism == 0) {
    Result->Error = "ThinLTO received an invalid configuration";
    return Result.release();
  }
  StringRef CacheDirectory = asStringRef(InputConfig->CacheDirectory);
  StringRef CacheMagicBytes = asStringRef(InputConfig->CacheMagic);
  std::array<uint8_t, CacheMagicSize> CacheMagic{};
  if (!CacheDirectory.empty()) {
    if (CacheMagicBytes.size() != CacheMagicSize) {
      Result->Error = "ThinLTO backend cache received an invalid format identity";
      return Result.release();
    }
    std::copy(CacheMagicBytes.bytes_begin(), CacheMagicBytes.bytes_end(),
              CacheMagic.begin());
  }
  if (auto Error = initializeTargets()) {
    Result->Error = std::move(*Error);
    return Result.release();
  }

  std::unordered_set<std::string> Preserved;
  for (size_t I = 0; I < InputConfig->PreservedSymbolCount; ++I)
    Preserved.insert(asStringRef(InputConfig->PreservedSymbols[I]).str());

  std::vector<std::unique_ptr<MemoryBuffer>> Buffers;
  std::vector<std::unique_ptr<lto::InputFile>> Files;
  Buffers.reserve(InputCount);
  Files.reserve(InputCount);
  std::unordered_set<std::string> Names;
  for (size_t I = 0; I < InputCount; ++I) {
    StringRef Name = asStringRef(Inputs[I].Name);
    StringRef Bitcode = asStringRef(Inputs[I].Bitcode);
    if (Name.empty() || !Names.insert(Name.str()).second) {
      Result->Error = "ThinLTO input module names must be non-empty and unique";
      return Result.release();
    }
    Buffers.push_back(MemoryBuffer::getMemBufferCopy(Bitcode, Name));
    auto ThinModule = lto::findThinLTOModule(Buffers.back()->getMemBufferRef());
    if (!ThinModule) {
      Result->Error = "ThinLTO input `" + Name.str() +
                      "` does not contain a ThinLTO summary: " +
                      errorText(ThinModule.takeError());
      return Result.release();
    }
    auto File = lto::InputFile::create(Buffers.back()->getMemBufferRef());
    if (!File) {
      Result->Error = "failed to parse ThinLTO input `" + Name.str() + "`: " +
                      errorText(File.takeError());
      return Result.release();
    }
    Files.push_back(std::move(*File));
  }

  struct Definition {
    size_t File;
    bool Weak;
  };
  std::unordered_map<std::string, Definition> Definitions;
  for (size_t FileIndex = 0; FileIndex < Files.size(); ++FileIndex) {
    for (const lto::InputFile::Symbol &Symbol : Files[FileIndex]->symbols()) {
      if (Symbol.isUndefined())
        continue;
      std::string Name = Symbol.getName().str();
      auto [It, Inserted] = Definitions.emplace(
          Name, Definition{FileIndex, Symbol.isWeak()});
      if (Inserted)
        continue;
      if (!It->second.Weak && !Symbol.isWeak()) {
        Result->Error = "multiple strong ThinLTO definitions for symbol `" + Name + "`";
        return Result.release();
      }
      if (It->second.Weak && !Symbol.isWeak())
        It->second = Definition{FileIndex, false};
    }
  }

  lto::Config Config;
  Config.CPU = asStringRef(InputConfig->CPU).str();
  StringRef Features = asStringRef(InputConfig->Features);
  while (!Features.empty()) {
    auto Split = Features.split(',');
    if (!Split.first.empty())
      Config.MAttrs.push_back(Split.first.str());
    Features = Split.second;
  }
  Config.OptLevel = InputConfig->Optimization;
  Config.CGOptLevel = codegenOptimizationLevel(InputConfig->Optimization);
  Config.Freestanding = InputConfig->Freestanding != 0;
  Config.PTO.LoopVectorization = Config.OptLevel > 1;
  Config.PTO.SLPVectorization = Config.OptLevel > 1;

  std::mutex ResultMutex;
  StageRecorder Stages;
  bool HasDiagnosticError = false;
  Config.DiagHandler = [&](const DiagnosticInfo &Info) {
    std::string Message;
    raw_string_ostream OS(Message);
    DiagnosticPrinterRawOStream Printer(OS);
    Info.print(Printer);
    OS.flush();
    std::lock_guard<std::mutex> Lock(ResultMutex);
    uint32_t Severity = diagnosticSeverity(Info.getSeverity());
    HasDiagnosticError |= Severity == 0;
    Result->Diagnostics.push_back(
        ThinDiagnostic{Severity, std::move(Message)});
  };
  Config.PreOptModuleHook = [&](unsigned Task, const Module &) {
    Stages.begin(Task);
    return true;
  };
  Config.PostPromoteModuleHook = [&](unsigned Task, const Module &) {
    Stages.finish(Task, StageRecorder::Stage::Promotion);
    return true;
  };
  Config.PostInternalizeModuleHook = [&](unsigned Task, const Module &) {
    Stages.finish(Task, StageRecorder::Stage::Internalization);
    return true;
  };
  Config.PostImportModuleHook = [&](unsigned Task, const Module &) {
    Stages.finish(Task, StageRecorder::Stage::Import);
    return true;
  };
  Config.PostOptModuleHook = [&](unsigned Task, const Module &) {
    Stages.finish(Task, StageRecorder::Stage::Optimization);
    return true;
  };
  Config.PreCodeGenModuleHook = [&](unsigned Task, const Module &) {
    Stages.begin(Task);
    return true;
  };

  ThreadPoolStrategy Parallelism =
      heavyweight_hardware_concurrency(InputConfig->Parallelism);
  lto::ThinBackend Inner = lto::createInProcessThinBackend(Parallelism);
  Clock::time_point RunStart = Clock::now();
  Clock::time_point BackendStart = RunStart;
  lto::ThinBackendFunction Factory =
      [&, Inner](const lto::Config &FactoryConfig,
                 ModuleSummaryIndex &CombinedIndex,
                 const DenseMap<StringRef, GVSummaryMapTy> &ModuleSummaries,
                 AddStreamFn AddStream, FileCache Cache) mutable {
        BackendStart = Clock::now();
        Result->Timings.ThinLinkNs = elapsedNs(RunStart, BackendStart);
        return Inner(FactoryConfig, CombinedIndex, ModuleSummaries,
                     std::move(AddStream), std::move(Cache));
      };
  lto::ThinBackend Backend(std::move(Factory), Parallelism);
  lto::LTO Lto(std::move(Config), std::move(Backend), 1);

  for (size_t FileIndex = 0; FileIndex < Files.size(); ++FileIndex) {
    std::vector<lto::SymbolResolution> Resolutions;
    Resolutions.reserve(Files[FileIndex]->symbols().size());
    for (const lto::InputFile::Symbol &Symbol : Files[FileIndex]->symbols()) {
      lto::SymbolResolution Resolution;
      std::string Name = Symbol.getName().str();
      auto Definition = Definitions.find(Name);
      bool DefinedInUnit = Definition != Definitions.end();
      Resolution.Prevailing = !Symbol.isUndefined() && DefinedInUnit &&
                              Definition->second.File == FileIndex;
      Resolution.FinalDefinitionInLinkageUnit = DefinedInUnit;
      Resolution.VisibleToRegularObj =
          Resolution.Prevailing && Preserved.count(Name) != 0;
      Resolutions.push_back(Resolution);
    }
    if (Error Err = Lto.add(std::move(Files[FileIndex]), Resolutions)) {
      Result->Error = errorText(std::move(Err));
      return Result.release();
    }
  }

  AddStreamFn AddStream = [&](unsigned Task, const Twine &ModuleName)
      -> Expected<std::unique_ptr<CachedFileStream>> {
    return ResultStream::create(Task, ModuleName.str(), *Result, ResultMutex,
                                Stages, {}, {}, {}, 0);
  };
  FileCache Cache(
      [&, AddStream](unsigned Task, StringRef Key,
                     const Twine &ModuleName) -> Expected<AddStreamFn> {
        {
          std::lock_guard<std::mutex> Lock(ResultMutex);
          Result->CacheKeys[Task] = Key.str();
        }
        if (!CacheDirectory.empty()) {
          SmallString<256> Path = cacheEntryPath(CacheDirectory, Key);
          CacheReadResult Cached = readCacheEntry(
              Path, CacheMagic, InputConfig->CacheReleaseCompatibility);
          if (Cached.Status == CacheRead::Hit) {
            std::lock_guard<std::mutex> Lock(ResultMutex);
            ++Result->Cache.Hits;
            Result->Objects.push_back(ThinObject{
                Task, ModuleName.str(), Key.str(), std::move(Cached.Bytes)});
            return AddStreamFn();
          }
          {
            std::lock_guard<std::mutex> Lock(ResultMutex);
            ++Result->Cache.Misses;
            Result->Cache.Corrupt += Cached.Status == CacheRead::Corrupt;
            Result->Cache.ReadErrors += Cached.Status == CacheRead::Error;
          }
          if (Cached.Status == CacheRead::Corrupt)
            sys::fs::remove(Path);
          std::string OwnedDirectory = CacheDirectory.str();
          std::string OwnedPath = Path.str().str();
          return [&, OwnedDirectory = std::move(OwnedDirectory),
                  OwnedPath = std::move(OwnedPath), CacheMagic,
                  ReleaseCompatibility =
                      InputConfig->CacheReleaseCompatibility](
                     unsigned StreamTask,
                     const Twine &StreamModuleName)
                     -> Expected<std::unique_ptr<CachedFileStream>> {
            return ResultStream::create(
                StreamTask, StreamModuleName.str(), *Result, ResultMutex,
                Stages, OwnedDirectory, OwnedPath, CacheMagic,
                ReleaseCompatibility);
          };
        }
        return AddStream;
      },
      CacheDirectory.str());
  if (Error Err = Lto.run(AddStream, std::move(Cache)))
    Result->Error = errorText(std::move(Err));
  auto End = Clock::now();
  Result->Timings.BackendNs = elapsedNs(BackendStart, End);
  ThinTimings StageTimings = Stages.snapshot();
  Result->Timings.PromotionNs = StageTimings.PromotionNs;
  Result->Timings.InternalizationNs = StageTimings.InternalizationNs;
  Result->Timings.ImportNs = StageTimings.ImportNs;
  Result->Timings.OptimizationNs = StageTimings.OptimizationNs;
  Result->Timings.CodegenNs = StageTimings.CodegenNs;
  std::sort(Result->Objects.begin(), Result->Objects.end(),
            [](const ThinObject &Left, const ThinObject &Right) {
              return Left.Task < Right.Task;
            });
  if (Result->Error.empty() && HasDiagnosticError)
    Result->Error = "LLVM reported an error during ThinLTO";
  return Result.release();
}

size_t nia_llvm_thin_result_object_count(const ThinResult *Result) {
  return Result ? Result->Objects.size() : 0;
}

uint32_t nia_llvm_thin_result_object_task(const ThinResult *Result,
                                          size_t Index) {
  return Result && Index < Result->Objects.size() ? Result->Objects[Index].Task : 0;
}

const uint8_t *nia_llvm_thin_result_object_name(const ThinResult *Result,
                                                size_t Index) {
  if (!Result || Index >= Result->Objects.size() ||
      Result->Objects[Index].ModuleName.empty())
    return nullptr;
  return reinterpret_cast<const uint8_t *>(
      Result->Objects[Index].ModuleName.data());
}

size_t nia_llvm_thin_result_object_name_len(const ThinResult *Result,
                                            size_t Index) {
  return Result && Index < Result->Objects.size()
             ? Result->Objects[Index].ModuleName.size()
             : 0;
}

const uint8_t *nia_llvm_thin_result_object_cache_key(
    const ThinResult *Result, size_t Index) {
  if (!Result || Index >= Result->Objects.size() ||
      Result->Objects[Index].CacheKey.empty())
    return nullptr;
  return reinterpret_cast<const uint8_t *>(
      Result->Objects[Index].CacheKey.data());
}

size_t nia_llvm_thin_result_object_cache_key_len(const ThinResult *Result,
                                                 size_t Index) {
  return Result && Index < Result->Objects.size()
             ? Result->Objects[Index].CacheKey.size()
             : 0;
}

const uint8_t *nia_llvm_thin_result_object_data(const ThinResult *Result,
                                                size_t Index) {
  if (!Result || Index >= Result->Objects.size() ||
      Result->Objects[Index].Bytes.empty())
    return nullptr;
  return reinterpret_cast<const uint8_t *>(Result->Objects[Index].Bytes.data());
}

size_t nia_llvm_thin_result_object_len(const ThinResult *Result, size_t Index) {
  return Result && Index < Result->Objects.size()
             ? Result->Objects[Index].Bytes.size()
             : 0;
}

size_t nia_llvm_thin_result_diagnostic_count(const ThinResult *Result) {
  return Result ? Result->Diagnostics.size() : 0;
}

uint32_t nia_llvm_thin_result_diagnostic_severity(const ThinResult *Result,
                                                  size_t Index) {
  return Result && Index < Result->Diagnostics.size()
             ? Result->Diagnostics[Index].Severity
             : 0;
}

const uint8_t *nia_llvm_thin_result_diagnostic_message(
    const ThinResult *Result, size_t Index) {
  if (!Result || Index >= Result->Diagnostics.size() ||
      Result->Diagnostics[Index].Message.empty())
    return nullptr;
  return reinterpret_cast<const uint8_t *>(
      Result->Diagnostics[Index].Message.data());
}

size_t nia_llvm_thin_result_diagnostic_message_len(const ThinResult *Result,
                                                   size_t Index) {
  return Result && Index < Result->Diagnostics.size()
             ? Result->Diagnostics[Index].Message.size()
             : 0;
}

const uint8_t *nia_llvm_thin_result_error(const ThinResult *Result) {
  if (!Result || Result->Error.empty())
    return nullptr;
  return reinterpret_cast<const uint8_t *>(Result->Error.data());
}

size_t nia_llvm_thin_result_error_len(const ThinResult *Result) {
  return Result ? Result->Error.size() : 0;
}

ThinTimings nia_llvm_thin_result_timings(const ThinResult *Result) {
  return Result ? Result->Timings : ThinTimings{};
}

uint64_t nia_llvm_thin_result_cache_hits(const ThinResult *Result) {
  return Result ? Result->Cache.Hits : 0;
}

uint64_t nia_llvm_thin_result_cache_misses(const ThinResult *Result) {
  return Result ? Result->Cache.Misses : 0;
}

uint64_t nia_llvm_thin_result_cache_corrupt(const ThinResult *Result) {
  return Result ? Result->Cache.Corrupt : 0;
}

uint64_t nia_llvm_thin_result_cache_read_errors(const ThinResult *Result) {
  return Result ? Result->Cache.ReadErrors : 0;
}

uint64_t nia_llvm_thin_result_cache_write_errors(const ThinResult *Result) {
  return Result ? Result->Cache.WriteErrors : 0;
}

void nia_llvm_thin_result_free(ThinResult *Result) { delete Result; }

} // extern "C"
