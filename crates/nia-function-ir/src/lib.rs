// SPDX-License-Identifier: GPL-3.0-or-later
//! Stable function-level IR, reference collection, and invariant validation.
//!
//! Producers may construct and transform the public tables directly; call
//! [`validate_function_body`] before handing a body to another compiler phase.
mod ir;
mod refs;
mod validate;

pub use ir::*;
pub use refs::*;
pub use validate::{
    FunctionIrError, validate_function_body, validate_function_closure_entry,
    validate_function_defer_body,
};
