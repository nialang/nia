// SPDX-License-Identifier: GPL-3.0-or-later
use nia_parser::ParseError;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TokensInspection {
    pub text: String,
}

#[derive(Debug, Clone, PartialEq)]
pub struct AstInspection {
    pub text: String,
    pub parse_errors: Vec<ParseError>,
}

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

pub fn ast_inspection(source: &str) -> AstInspection {
    let (module, parse_errors) = nia_parser::parse_module(source);
    AstInspection {
        text: format!("{module:#?}\n"),
        parse_errors,
    }
}
