// SPDX-License-Identifier: GPL-3.0-or-later

// COFF import libraries written from module definitions, so linking a Windows
// executable needs no platform SDK import libraries.

#include "llvm/Object/COFFImportFile.h"
#include "llvm/Object/COFFModuleDefinition.h"
#include "llvm/Support/Error.h"
#include "llvm/Support/MemoryBuffer.h"

#include <cstddef>
#include <cstdint>
#include <string>
#include <utility>

using namespace llvm;

namespace {

struct ImportLibraryError {
  std::string Message;
};

ImportLibraryError *fail(std::string Message) {
  return new ImportLibraryError{std::move(Message)};
}

} // namespace

extern "C" {

// Returns null on success, otherwise an error owned by the caller.
ImportLibraryError *nia_llvm_write_coff_import_library(const uint8_t *Definition,
                                                       size_t DefinitionLen,
                                                       uint16_t Machine,
                                                       const uint8_t *Path,
                                                       size_t PathLen) {
  if (!Definition || !Path)
    return fail("COFF import library writer received a null buffer");
  auto Type = static_cast<COFF::MachineTypes>(Machine);
  MemoryBufferRef Buffer(
      StringRef(reinterpret_cast<const char *>(Definition), DefinitionLen),
      "module definition");
  Expected<object::COFFModuleDefinition> Parsed =
      object::parseCOFFModuleDefinition(Buffer, Type, /*MingwDef=*/false);
  if (!Parsed)
    return fail(toString(Parsed.takeError()));
  if (Parsed->OutputFile.empty())
    return fail("module definition has no LIBRARY statement");
  std::string Output(reinterpret_cast<const char *>(Path), PathLen);
  if (Error Written = object::writeImportLibrary(Parsed->OutputFile, Output,
                                                 Parsed->Exports, Type,
                                                 /*MinGW=*/false))
    return fail(toString(std::move(Written)));
  return nullptr;
}

const uint8_t *nia_llvm_import_library_error_data(const ImportLibraryError *Error) {
  return reinterpret_cast<const uint8_t *>(Error->Message.data());
}

size_t nia_llvm_import_library_error_len(const ImportLibraryError *Error) {
  return Error->Message.size();
}

void nia_llvm_import_library_error_free(ImportLibraryError *Error) { delete Error; }

} // extern "C"
