// SPDX-License-Identifier: GPL-3.0-or-later
mod analyzer;
mod module_lowering;
mod support;
mod types;

pub use analyzer::{
    ComptimeGenericInstantiation, TypedComptimeFrame, TypedComptimeQueryInput,
    check_module_comptime, check_module_comptime_with_all_phases,
    check_module_comptime_with_array_lengths, check_module_comptime_with_phases,
    compute_module_comptime_array_lengths, compute_module_comptime_enum_values,
    compute_module_comptime_typed_facts, compute_module_comptime_values,
    infer_resolved_comptime_expr_type, instantiate_resolved_comptime_function_generics,
};
pub use module_lowering::*;
pub use types::*;

#[cfg(test)]
mod tests;
