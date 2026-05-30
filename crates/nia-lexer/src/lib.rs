// SPDX-License-Identifier: GPL-3.0-or-later
mod token;
mod tokenizer;

pub use token::{LexError, LosslessToken, LosslessTokenKind, Token, TokenKind};
pub use tokenizer::{Tokenizer, tokenize, tokenize_lossless};
