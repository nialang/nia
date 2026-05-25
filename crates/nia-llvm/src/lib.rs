// SPDX-License-Identifier: GPL-3.0-or-later
//! Thin typed wrappers around the LLVM C API.
//!
//! This crate is intentionally independent from nia frontend and backend IR
//! crates. Higher-level codegen should depend on this wrapper instead of using
//! raw `llvm-sys` handles directly.

mod llvm_api;
mod llvm_facade;

pub use llvm_api::{Context, InlineAsmDialect, InlineAsmOptions};
pub use llvm_facade::{
    AddressSpace, AtomicOrdering, AtomicRMWBinOp, FloatPredicate, IntPredicate, OptimizationLevel,
    attributes, basic_block, builder, context, intrinsics, llvm_sys, module, target, types, values,
};
