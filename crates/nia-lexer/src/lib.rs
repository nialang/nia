// SPDX-License-Identifier: GPL-3.0-or-later
//! Byte-offset lexer for Nia source text.
//!
//! Language identifiers are intentionally ASCII, while source input is UTF-8.
//! Unsupported Unicode scalars therefore become one error token each rather
//! than one token per encoded byte. This keeps every span on a UTF-8 boundary
//! and lets the lossless token stream reconstruct the original source.
//!
//! The lexer recognizes comments, identifiers, keywords, numbers, string and
//! byte-string literals, character literals, punctuation, and lexical errors.
//! It does not resolve types, evaluate constants, or classify identifiers
//! beyond keyword recognition. Numeric separator placement is part of the
//! lexical grammar: every `_` must sit between two digits valid for that
//! literal's radix.

mod token;
mod tokenizer;

pub use token::{LexError, LosslessToken, LosslessTokenKind, Token, TokenKind};
pub use tokenizer::{Tokenizer, tokenize, tokenize_lossless};
