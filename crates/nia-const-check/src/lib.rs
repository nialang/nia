// SPDX-License-Identifier: GPL-3.0-or-later
//! Compile-time semantic analysis for Nia modules.
//!
//! Const checking is deliberately staged: array lengths are evaluated first,
//! then enum discriminants, general const values, and finally typed facts. Each
//! result carries diagnostics forward so query clients can cache and reuse an
//! earlier phase without rerunning it. The analyzer also implements
//! [`nia_const_eval`] environments, which keeps the interpreter independent of
//! module lookup, type normalization, trait solving, and layout computation.
//!
//! In Nia, `const` denotes compile-time execution rather than Rust-style static
//! storage. Runtime-addressable `static` items are handled by later lowering and
//! backend phases; this crate is concerned with values that must be known while
//! compiling.
mod analyzer;
mod module_lowering;
mod support;
mod types;

pub use analyzer::{
    ConstGenericInstantiation, TypedConstFrame, TypedConstQueryInput, check_module_const,
    check_module_const_with_all_phases, check_module_const_with_array_lengths,
    check_module_const_with_phases, compute_module_const_array_lengths,
    compute_module_const_enum_values, compute_module_const_typed_facts,
    compute_module_const_values, infer_resolved_const_expr_type,
    instantiate_resolved_const_function_generics,
};
pub use module_lowering::*;
pub use nia_type_normalize::TypeNormalization;
pub use types::*;

#[cfg(test)]
mod tests;
