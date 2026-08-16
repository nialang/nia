// SPDX-License-Identifier: GPL-3.0-or-later
//! Environment-parametric interpreter for Nia's compile-time IR.
//!
//! The crate evaluates both early IR, where names and some types are not yet
//! resolved, and resolved IR used by semantic queries. The interpreter owns
//! control-flow semantics and cleanup boundaries; an [`EarlyConstEnv`] or
//! [`ResolvedConstEnv`] supplies compiler-specific name lookup, typing, memory,
//! trait witnesses, and function dispatch.
//!
//! Root expression and function-call entry points open nested
//! [`ConstEvalBudget`] sessions. Function frames and lexical scopes are restored
//! before an error or non-local control-flow value is returned, allowing one
//! environment to serve multiple queries without leaking state between them.
mod env;
mod eval;
mod literals;
mod numeric;
mod value;

pub use env::*;
pub use eval::*;
pub use literals::{
    eval_byte_string_literal, eval_float_literal, eval_int_literal, eval_string_literal,
};
pub use value::*;

#[cfg(test)]
mod tests;
