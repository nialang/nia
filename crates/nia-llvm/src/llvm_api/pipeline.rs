// SPDX-License-Identifier: GPL-3.0-or-later
//! The IR pass pipeline that runs before an object or LTO bitcode is written.
//!
//! Every Nia module goes through one pipeline builder in the C++ bridge. Its
//! output differs only by phase: a native object, ThinLTO pre-link bitcode, or
//! full-LTO pre-link bitcode. Modules are always optimized as freestanding code:
//! Nia never relies on a hosted C library, so no pass may assume that library
//! functions exist or introduce calls to them.

use std::ptr::NonNull;

use llvm_sys::prelude::LLVMModuleRef;
use llvm_sys::target_machine::LLVMTargetMachineRef;

use super::lto::{copy_bytes, copy_text};
use super::{LlvmError, LlvmResult, Module, OptimizationLevel, TargetMachine};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
/// LLVM's per-module optimization pipeline, from unoptimized to size-focused.
pub enum PipelineLevel {
    /// Only stack-slot promotion.
    O0,
    /// Cheap, local optimization.
    O1,
    /// LLVM's default optimization.
    O2,
    /// Optimization that spends more compile time and code size on speed.
    O3,
    /// The default pipeline with every function marked `optsize`.
    Os,
    /// The default pipeline with every function marked `optsize` and `minsize`.
    Oz,
}

impl PipelineLevel {
    pub(super) fn tag(self) -> u32 {
        match self {
            Self::O0 => 0,
            Self::O1 => 1,
            Self::O2 => 2,
            Self::O3 => 3,
            Self::Os => 4,
            Self::Oz => 5,
        }
    }

    /// The code-generation tier that turns this pipeline's output into machine
    /// code. `Os` keeps the default tier and `Oz` the lighter one; their size
    /// goals reach code generation through function attributes.
    pub fn codegen_level(self) -> OptimizationLevel {
        match self {
            Self::O0 => OptimizationLevel::None,
            Self::O1 | Self::Oz => OptimizationLevel::Less,
            Self::O2 | Self::Os => OptimizationLevel::Default,
            Self::O3 => OptimizationLevel::Aggressive,
        }
    }

    /// The level's conventional name, for reports.
    pub fn name(self) -> &'static str {
        match self {
            Self::O0 => "O0",
            Self::O1 => "O1",
            Self::O2 => "O2",
            Self::O3 => "O3",
            Self::Os => "Os",
            Self::Oz => "Oz",
        }
    }

    /// Stable identity of the pipeline at this level, for products that cache
    /// optimized output. It names how the pipeline is assembled; the LLVM
    /// version that implements it is identified separately.
    pub fn identity(self) -> &'static str {
        match self {
            Self::O0 => "freestanding:mem2reg+o0-default",
            Self::O1 => "freestanding:pass-builder-default:O1",
            Self::O2 => "freestanding:pass-builder-default:O2",
            Self::O3 => "freestanding:pass-builder-default:O3",
            Self::Os => "freestanding:pass-builder-default:O2+optsize",
            Self::Oz => "freestanding:pass-builder-default:O2+optsize+minsize",
        }
    }
}

enum FfiOwnedBuffer {}

unsafe extern "C" {
    fn nia_llvm_optimize_module(
        module: LLVMModuleRef,
        target: LLVMTargetMachineRef,
        level: u32,
    ) -> *mut FfiOwnedBuffer;
    fn nia_llvm_emit_thin_lto_bitcode(
        module: LLVMModuleRef,
        target: LLVMTargetMachineRef,
        level: u32,
    ) -> *mut FfiOwnedBuffer;
    fn nia_llvm_emit_full_lto_bitcode(
        module: LLVMModuleRef,
        target: LLVMTargetMachineRef,
        level: u32,
    ) -> *mut FfiOwnedBuffer;
    fn nia_llvm_owned_buffer_data(buffer: *const FfiOwnedBuffer) -> *const u8;
    fn nia_llvm_owned_buffer_len(buffer: *const FfiOwnedBuffer) -> usize;
    fn nia_llvm_owned_buffer_error(buffer: *const FfiOwnedBuffer) -> *const u8;
    fn nia_llvm_owned_buffer_error_len(buffer: *const FfiOwnedBuffer) -> usize;
    fn nia_llvm_owned_buffer_free(buffer: *mut FfiOwnedBuffer);
}

struct OwnedBufferHandle(NonNull<FfiOwnedBuffer>);

impl Drop for OwnedBufferHandle {
    fn drop(&mut self) {
        unsafe { nia_llvm_owned_buffer_free(self.0.as_ptr()) };
    }
}

// Takes ownership of a bridge result and returns its bytes or its error.
fn finish(result: *mut FfiOwnedBuffer, label: &str) -> LlvmResult<Vec<u8>> {
    let result = OwnedBufferHandle(
        NonNull::new(result)
            .ok_or_else(|| LlvmError::error(format!("LLVM returned no {label} result")))?,
    );
    let error = unsafe {
        copy_text(
            nia_llvm_owned_buffer_error(result.0.as_ptr()),
            nia_llvm_owned_buffer_error_len(result.0.as_ptr()),
            label,
        )?
    };
    if !error.is_empty() {
        return Err(LlvmError::error(error));
    }
    unsafe {
        copy_bytes(
            nia_llvm_owned_buffer_data(result.0.as_ptr()),
            nia_llvm_owned_buffer_len(result.0.as_ptr()),
            label,
        )
    }
}

/// Optimizes a module in place before native object emission.
pub fn optimize_module(
    module: &Module<'_>,
    target: &TargetMachine,
    level: PipelineLevel,
) -> LlvmResult<()> {
    let result = unsafe { nia_llvm_optimize_module(module.raw, target.raw, level.tag()) };
    finish(result, "module optimization").map(|_| ())
}

/// Runs the ThinLTO pre-link pipeline and emits summary-bearing bitcode.
pub fn emit_thin_lto_bitcode(
    module: &Module<'_>,
    target: &TargetMachine,
    level: PipelineLevel,
) -> LlvmResult<Vec<u8>> {
    let result = unsafe { nia_llvm_emit_thin_lto_bitcode(module.raw, target.raw, level.tag()) };
    finish(result, "ThinLTO pre-link")
}

/// Runs the full-LTO pre-link pipeline and emits regular bitcode.
pub fn emit_full_lto_bitcode(
    module: &Module<'_>,
    target: &TargetMachine,
    level: PipelineLevel,
) -> LlvmResult<Vec<u8>> {
    let result = unsafe { nia_llvm_emit_full_lto_bitcode(module.raw, target.raw, level.tag()) };
    finish(result, "full-LTO pre-link")
}
