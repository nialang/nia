// SPDX-License-Identifier: GPL-3.0-or-later
//! Stable function-level IR, reference collection, and invariant validation.
//!
//! Producers may construct and transform the public tables directly; call
//! [`validate_function_body`] before handing a body to another compiler phase.
//!
//! Function IR is the function-level backend boundary: blocks, scopes,
//! operations, terminators, places, callees, locals, builtin values, inline
//! assembly, and runtime expressions. Source-shaped control expressions do
//! not survive into it; block, `if`, `match`, `for`, `return`, `break`,
//! `continue`, and `defer` behavior is expressed through blocks, terminators,
//! scope edges, and defer bodies. A call carries its resolved callee identity,
//! with trait, method, and const arguments and receiver substitutions as
//! separate fields, so later phases never resolve names again.
mod ir;
mod refs;
mod validate;

pub use ir::*;
pub use refs::*;
pub use validate::{
    FunctionIrError, validate_function_body, validate_function_closure_entry,
    validate_function_defer_body,
};
