// SPDX-License-Identifier: GPL-3.0-or-later
//! Grammar parser that lowers lossless syntax trees into the semantic AST.
//!
//! The parser preserves syntax-origin identities for accepted AST nodes and
//! reports lexical or grammar errors without discarding the original source.
mod parser;

pub use parser::{
    ParseError, Parser, parse_module, parse_module_syntax,
    parse_module_syntax_with_node_store_and_symbols, parse_module_syntax_with_origins,
    parse_module_syntax_with_origins_and_symbols, parse_module_with_symbols,
};
