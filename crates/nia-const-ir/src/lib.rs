// SPDX-License-Identifier: GPL-3.0-or-later
//! Const-capable intermediate representation and its lowering boundaries.
//!
//! Syntax is first lowered into permissive early IR, then resolved into an
//! identity-complete form consumed by const checking, evaluation, and static
//! initialization.

mod defs;
mod lower;
mod resolve;

pub use defs::*;
pub use lower::*;
pub use resolve::*;

#[cfg(test)]
mod tests;
