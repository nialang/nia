// SPDX-License-Identifier: GPL-3.0-or-later
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
