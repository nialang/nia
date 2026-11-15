// SPDX-License-Identifier: GPL-3.0-or-later

// What the bridges share of Nia's LLVM pipeline: Nia's own IR passes.

#pragma once

#include "llvm/Passes/PassBuilder.h"

// Adds Nia's IR passes to the pipelines `PB` builds; see llvm_nia_passes.cpp.
void niaRegisterPasses(llvm::PassBuilder &PB);

