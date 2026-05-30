// SPDX-License-Identifier: GPL-3.0-or-later
mod parser;

pub use parser::{
    ParseError, Parser, parse_module, parse_module_syntax, parse_module_syntax_with_origins,
};
