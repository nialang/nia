// SPDX-License-Identifier: GPL-3.0-or-later
mod analyzer;
mod module_lowering;
mod support;
mod types;

pub use analyzer::{
    TypedComptimeFrame, TypedComptimeQueryInput, check_module_comptime,
    infer_resolved_comptime_expr_type, instantiate_resolved_comptime_function_generics,
};
pub use module_lowering::*;
pub use types::*;

#[cfg(test)]
mod tests;
