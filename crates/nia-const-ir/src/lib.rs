// SPDX-License-Identifier: GPL-3.0-or-later
//! Const-capable intermediate representation and its lowering boundaries.
//!
//! Syntax is first lowered into permissive early IR, then resolved into an
//! identity-complete form consumed by const checking, evaluation, and static
//! initialization.
//!
//! The IR stores only the expression, statement, block, function, parameter,
//! binding, and field forms that are valid inputs to compile-time evaluation,
//! while preserving the semantic ids and source spans needed by name, local,
//! type-argument, and diagnostic queries. Compile-time execution never reads
//! AST. A [`ResolvedConstModule`] holds a module's const enums, global and
//! local const initializers, `const fn` bodies, and type-level constant
//! expressions.

mod defs;
mod lower;
mod resolve;

pub use defs::*;
pub use lower::*;
pub use resolve::*;

#[cfg(test)]
mod tests;
