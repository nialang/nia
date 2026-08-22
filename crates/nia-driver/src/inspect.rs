// SPDX-License-Identifier: GPL-3.0-or-later
use nia_parser::ParseError;

/// Lossless token inspection output for CLI/debug tooling.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TokensInspection {
    /// Human-readable token stream.
    pub text: String,
}

/// Pretty-printed AST inspection output and parse diagnostics.
#[derive(Debug, Clone, PartialEq)]
pub struct AstInspection {
    /// Debug-formatted AST text.
    pub text: String,
    /// Structured parser errors encountered while building the AST.
    pub parse_errors: Vec<ParseError>,
}

/// Tokenizes source and formats each token with its byte span.
pub fn tokens_inspection(source: &str) -> TokensInspection {
    let mut text = String::new();
    for token in nia_lexer::tokenize(source) {
        text.push_str(&format!(
            "{:?} {}..{}\n",
            token.kind, token.span.start, token.span.end
        ));
    }
    TokensInspection { text }
}

/// Parses source and formats the resulting AST with its parse errors.
pub fn ast_inspection(source: &str) -> AstInspection {
    let (module, parse_errors) = nia_parser::parse_module(source);
    AstInspection {
        text: format!("{module:#?}\n"),
        parse_errors,
    }
}
