// SPDX-License-Identifier: GPL-3.0-or-later
//! Thin typed wrappers around the LLVM C API.
//!
//! This crate is intentionally independent from nia frontend and backend IR
//! crates. Higher-level codegen should depend on this wrapper instead of using
//! raw `llvm-sys` handles directly. The public facade sits over private LLVM
//! API modules. Wrapper construction is fallible: null LLVM results become
//! `LlvmResult` errors that codegen propagates instead of exposing invalid
//! handles.
//!
//! The crate also owns LTO coordination through LLVM's resolution-based
//! `llvm::lto::LTO` interface, not the legacy ThinLTO C API or a linker
//! plugin. It builds the combined index, applies symbol resolutions, runs
//! ThinLTO imports with parallel backends or the full-LTO backend, and returns
//! native objects with separate stage timings. ThinLTO backend objects are
//! cached under LLVM's complete cache key, which includes the combined-index
//! import/export and target decisions; LLVM owns that key, while the records
//! use the registered ThinLTO backend-object format identity. Before reading
//! or publishing, the bridge rechecks an existing cache directory and rejects
//! redirected directories and non-regular records.

#[cfg(all(feature = "llvm-static", feature = "llvm-dynamic"))]
compile_error!("llvm-static and llvm-dynamic features are mutually exclusive");

mod llvm_api;
mod llvm_facade;

pub use llvm_api::{Attribute, AttributeLoc, Context, InlineAsmDialect, InlineAsmOptions};
pub use llvm_facade::{
    AddressSpace, AtomicOrdering, AtomicRMWBinOp, FloatPredicate, IntPredicate, LlvmError,
    LlvmResult, OptimizationLevel, attributes, basic_block, builder, coff, context, intrinsics,
    llvm_sys, lto, module, target, types, values,
};
