// SPDX-License-Identifier: GPL-3.0-or-later
mod ir;
mod validate;

pub use ir::*;
pub use validate::{FunctionIrError, validate_function_body, validate_function_defer_body};
