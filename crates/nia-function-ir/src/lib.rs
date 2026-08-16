// SPDX-License-Identifier: GPL-3.0-or-later
mod ir;
mod refs;
mod validate;

pub use ir::*;
pub use refs::*;
pub use validate::{
    FunctionIrError, validate_function_body, validate_function_closure_entry,
    validate_function_defer_body,
};
