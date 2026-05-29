// SPDX-License-Identifier: GPL-3.0-or-later
mod ir;
mod lower;
mod validate;

#[cfg(test)]
mod tests;

pub use ir::*;
pub use lower::lower_function_body;
pub use validate::{FunctionIrError, validate_function_body, validate_function_defer_body};
