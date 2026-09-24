// SPDX-License-Identifier: GPL-3.0-or-later
//! Grammar recognition and recovery events for Nia source.
//!
//! Grammar owns parse decisions; `nia-syntax` owns green/red storage, while
//! `nia-parser` owns AST lowering. The first production covers module items.

use nia_lexer::{LosslessToken, LosslessTokenKind, TokenKind, tokenize_lossless};
use nia_source::SourceVersion;
use nia_span::Span;
use nia_syntax::{GreenBuildError, GreenEvent, SyntaxKind, SyntaxTree};

/// One error emitted while building the grammar tree.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct GrammarError {
    /// Location of the missing or malformed construct.
    pub span: Span,
    /// Grammar rule that rejected the input.
    pub message: String,
}

/// A lossless grammar tree and its grammar errors.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct GrammarTree {
    /// Green tree with borrowed red views.
    pub tree: SyntaxTree,
    /// Errors from productions already migrated to this crate.
    pub errors: Vec<GrammarError>,
}

/// Parses the supported grammar productions into one lossless source tree.
pub fn parse(source: &str, version: Option<SourceVersion>) -> Result<GrammarTree, GreenBuildError> {
    let tokens = tokenize_lossless(source);
    let significant = tokens
        .iter()
        .enumerate()
        .filter_map(|(index, token)| match token.kind {
            LosslessTokenKind::Token(_) => Some(index),
            LosslessTokenKind::Whitespace | LosslessTokenKind::LineComment => None,
        })
        .collect();
    let mut parser = GrammarParser {
        tokens,
        significant,
        position: 0,
        emitted: 0,
        events: vec![GreenEvent::Start(SyntaxKind::SourceFile)],
        errors: Vec::new(),
    };
    parser.parse_source();
    let tree = SyntaxTree::from_green_events(source, version, parser.events)?;
    Ok(GrammarTree {
        tree,
        errors: parser.errors,
    })
}

struct GrammarParser {
    tokens: Vec<LosslessToken>,
    significant: Vec<usize>,
    position: usize,
    emitted: usize,
    events: Vec<GreenEvent>,
    errors: Vec<GrammarError>,
}

impl GrammarParser {
    fn parse_source(&mut self) {
        while !self.at(TokenKind::Eof) {
            if self.at(TokenKind::Module) {
                self.emit_trivia_before_current();
                self.parse_module_item();
            } else {
                self.parse_unmigrated_item();
            }
        }
        self.emit_until(self.tokens.len());
        self.events.push(GreenEvent::Finish);
    }

    fn parse_module_item(&mut self) {
        self.events.push(GreenEvent::Start(SyntaxKind::Module));
        self.bump();
        if self.at(TokenKind::Ident) {
            self.bump();
        } else {
            self.missing("expected module name");
        }
        if self.at(TokenKind::Semicolon) {
            self.bump();
        } else if self.at(TokenKind::Eof) || is_item_start(&self.current_kind()) {
            self.missing("expected `;` after module declaration");
        } else {
            self.events.push(GreenEvent::Start(SyntaxKind::Error));
            while !self.at(TokenKind::Semicolon)
                && !self.at(TokenKind::Eof)
                && !is_item_start(&self.current_kind())
            {
                self.bump();
            }
            self.events.push(GreenEvent::Finish);
            self.missing("expected `;` after module declaration");
            if self.at(TokenKind::Semicolon) {
                self.bump();
            }
        }
        self.events.push(GreenEvent::Finish);
    }

    fn parse_unmigrated_item(&mut self) {
        self.emit_trivia_before_current();
        self.events.push(GreenEvent::Start(SyntaxKind::Unparsed));
        let mut delimiters = Vec::new();
        while !self.at(TokenKind::Eof) {
            if delimiters.is_empty() && self.at(TokenKind::Module) {
                break;
            }
            let kind = self.current_kind();
            match kind {
                TokenKind::LParen | TokenKind::LBracket | TokenKind::LBrace => {
                    delimiters.push(kind);
                }
                TokenKind::RParen | TokenKind::RBracket | TokenKind::RBrace
                    if delimiters.last().is_some_and(|open| closes(open, &kind)) =>
                {
                    delimiters.pop();
                }
                _ => {}
            }
            self.bump();
        }
        self.events.push(GreenEvent::Finish);
    }

    fn at(&self, kind: TokenKind) -> bool {
        self.current_kind() == kind
    }

    fn current_kind(&self) -> TokenKind {
        self.significant
            .get(self.position)
            .and_then(|&index| match &self.tokens[index].kind {
                LosslessTokenKind::Token(kind) => Some(kind.clone()),
                LosslessTokenKind::Whitespace | LosslessTokenKind::LineComment => None,
            })
            .unwrap_or(TokenKind::Eof)
    }

    fn current_span(&self) -> Span {
        self.significant
            .get(self.position)
            .map_or_else(|| Span::new(0, 0), |&index| self.tokens[index].span)
    }

    fn bump(&mut self) {
        let Some(&index) = self.significant.get(self.position) else {
            return;
        };
        self.emit_until(index + 1);
        self.position += 1;
    }

    fn emit_trivia_before_current(&mut self) {
        if let Some(&index) = self.significant.get(self.position) {
            self.emit_until(index);
        }
    }

    fn emit_until(&mut self, end: usize) {
        self.events.extend(
            self.tokens[self.emitted..end]
                .iter()
                .cloned()
                .map(GreenEvent::Token),
        );
        self.emitted = end;
    }

    fn missing(&mut self, message: &str) {
        self.errors.push(GrammarError {
            span: self.current_span(),
            message: message.into(),
        });
        self.events.push(GreenEvent::Start(SyntaxKind::Missing));
        self.events.push(GreenEvent::Finish);
    }
}

fn closes(open: &TokenKind, close: &TokenKind) -> bool {
    matches!(
        (open, close),
        (TokenKind::LParen, TokenKind::RParen)
            | (TokenKind::LBracket, TokenKind::RBracket)
            | (TokenKind::LBrace, TokenKind::RBrace)
    )
}

fn is_item_start(kind: &TokenKind) -> bool {
    matches!(
        kind,
        TokenKind::Module
            | TokenKind::Using
            | TokenKind::Extern
            | TokenKind::Struct
            | TokenKind::Union
            | TokenKind::Trait
            | TokenKind::Extend
            | TokenKind::Enum
            | TokenKind::Type
            | TokenKind::Fn
            | TokenKind::Const
            | TokenKind::Static
            | TokenKind::Pub
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use nia_syntax::GreenElement;

    #[test]
    fn module_items_have_grammar_nodes_and_trivia_remains_lossless() {
        let source = "// header\nmodule math;\nmodule util;\n";
        let parsed = parse(source, None).expect("valid event stream");
        assert!(parsed.errors.is_empty(), "{:?}", parsed.errors);
        assert_eq!(parsed.tree.full_text(), source);
        let items = parsed.tree.root().child_nodes();
        assert_eq!(items.len(), 2);
        assert!(items.iter().all(|item| item.kind() == &SyntaxKind::Module));
    }

    #[test]
    fn missing_module_name_and_semicolon_create_missing_nodes() {
        let source = "module module next;";
        let parsed = parse(source, None).expect("valid event stream");
        assert_eq!(parsed.tree.full_text(), source);
        assert_eq!(parsed.errors.len(), 2);
        let items = parsed.tree.root().child_nodes();
        assert_eq!(items.len(), 2);
        assert_eq!(
            items[0]
                .child_nodes()
                .iter()
                .filter(|child| child.kind() == &SyntaxKind::Missing)
                .count(),
            2
        );
    }

    #[test]
    fn unmigrated_function_is_preserved_without_false_grammar_errors() {
        let source = "pub fn main() () { module_name(); }\nmodule next;";
        let parsed = parse(source, None).expect("valid event stream");
        assert!(parsed.errors.is_empty(), "{:?}", parsed.errors);
        assert_eq!(parsed.tree.full_text(), source);
        let children = parsed.tree.green_root().children();
        assert!(children.iter().any(|child| matches!(
            child,
            GreenElement::Node(node) if node.kind() == &SyntaxKind::Unparsed
        )));
        assert!(
            parsed
                .tree
                .root()
                .child_nodes()
                .iter()
                .any(|item| item.kind() == &SyntaxKind::Module)
        );
    }
}
