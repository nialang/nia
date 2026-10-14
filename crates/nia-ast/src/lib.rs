// SPDX-License-Identifier: GPL-3.0-or-later
//! Semantic-free abstract syntax tree nodes produced by the parser.
//!
//! AST values carry source spans and syntax identities only. Type, definition,
//! layout, and backend facts live in later phase products rather than here.
//! Expressions, statements, patterns, items, and type references retain only
//! syntax payloads plus span and node-key identity.
mod expr;
mod items;
mod types;

pub use expr::*;
pub use items::*;
pub use types::*;
