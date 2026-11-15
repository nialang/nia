// SPDX-License-Identifier: GPL-3.0-or-later

// What the bridges share of Nia's LLVM pipeline: Nia's own IR passes, and the
// ThinLTO backend that runs them after the thin link as well.

#pragma once

#include "llvm/LTO/LTO.h"
#include "llvm/Passes/PassBuilder.h"
#include "llvm/Support/Threading.h"

#include <string>

// Adds Nia's IR passes to the pipelines `PB` builds; see llvm_nia_passes.cpp.
void niaRegisterPasses(llvm::PassBuilder &PB);

// The ThinLTO backend that optimizes each module after the thin link with
// Nia's passes, `Parallelism` modules at once. `PipelineIdentity` names the
// pipeline in backend cache keys. See llvm_thin_backend.cpp.
llvm::lto::ThinBackend niaThinBackend(llvm::ThreadPoolStrategy Parallelism,
                                      std::string PipelineIdentity);
