// SPDX-License-Identifier: GPL-3.0-or-later
//! Nia concrete syntax: the grammar and the lossless syntax tree it builds.
//!
//! [`parse`] is the only owner of parse decisions. It turns source text into
//! one lossless [`SyntaxTree`] whose nodes are complete grammar productions,
//! plus every syntax diagnostic. Recovery is local: a malformed region becomes
//! an `Error` node, an absent construct becomes a zero-width `Missing` node,
//! and parsing resumes at the nearest boundary that owns the region.
//!
//! The crate has three parts with one direction of dependency:
//! [`SyntaxKind`] is the node vocabulary, the tree module stores immutable
//! green nodes with red views, and the grammar module emits the events that
//! build trees. `nia-parser` consumes finished trees and lowers them to AST.
//!
//! A few constructs are decided semantically rather than syntactically. Their
//! grammar node records the syntactic choice, and the other reading of the
//! same tokens is kept as an [`Alternate`] so lowering never re-reads source.

mod grammar;
mod kind;
mod reparse;
mod tree;

#[cfg(test)]
mod tests;

pub use kind::SyntaxKind;
pub use tree::{
    GreenBuildError, GreenElement, GreenEvent, GreenNode, GreenToken, SyntaxNode, SyntaxToken,
    SyntaxTree, TextEdit, shift_span,
};

use nia_diagnostic::{Diagnostic, codes};
use nia_lexer::{TokenKind, tokenize_lossless};
use nia_node_id::VersionedNodeKey;
use nia_source::SourceVersion;
use nia_span::Span;

use grammar::parser::{BlockRegion, GrammarOutput, Iteration, Parser, RawError};

/// A lexical or grammatical error with its syntax origin.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ParseError {
    /// Source span associated with the lexical or grammar error.
    pub span: Span,
    /// Grammar rule classification assigned where the parser rejected input.
    pub kind: ParseErrorKind,
    /// Human-readable diagnostic message.
    pub message: String,
    /// Stable identity of the token where the error was detected.
    pub node_key: Option<VersionedNodeKey>,
}

/// Grammar rule that rejected the input, independent of its message wording.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum ParseErrorKind {
    /// The lexer produced an invalid token.
    Lexical,
    /// A declaration or statement terminator is missing.
    MissingSemicolon,
    /// A closing `)`, `]`, or `}` is missing.
    MissingClosingDelimiter,
    /// An identifier is required in this position.
    ExpectedName,
    /// An expression is required in this position.
    ExpectedExpression,
    /// A type is required in this position.
    ExpectedType,
    /// A binding pattern is required in this position.
    ExpectedBindingPattern,
    /// Any other grammar rule; its message is the complete explanation.
    Grammar,
}

impl ParseErrorKind {
    /// Returns a bounded, actionable hint for this grammar rule.
    pub fn help(self) -> Option<&'static str> {
        match self {
            Self::MissingSemicolon => {
                Some("add the missing `;` to terminate this declaration or statement")
            }
            Self::MissingClosingDelimiter => {
                Some("check that the surrounding delimiters are balanced")
            }
            Self::ExpectedName => Some("use an identifier in this position"),
            Self::ExpectedExpression => {
                Some("provide an expression here, such as a literal, name, or call")
            }
            Self::ExpectedType => Some("provide a type name or type expression here"),
            Self::ExpectedBindingPattern => {
                Some("use a name, `_`, or a supported destructuring pattern")
            }
            Self::Lexical | Self::Grammar => None,
        }
    }

    pub(crate) fn for_expected_token(kind: &TokenKind) -> Self {
        match kind {
            TokenKind::Semicolon => Self::MissingSemicolon,
            TokenKind::RParen | TokenKind::RBracket | TokenKind::RBrace => {
                Self::MissingClosingDelimiter
            }
            TokenKind::Ident => Self::ExpectedName,
            _ => Self::Grammar,
        }
    }
}

impl ParseError {
    /// Builds the canonical `E0101` diagnostic for this parse error.
    ///
    /// Every renderer and query boundary uses this constructor so the code,
    /// primary label, and remediation stay identical across text, JSON, and
    /// incremental diagnostic products.
    pub fn to_diagnostic(&self) -> Diagnostic {
        let diagnostic = Diagnostic::user_error(codes::PARSE, self.message.clone())
            .primary(self.span, self.message.clone());
        match self.kind.help() {
            Some(help) => diagnostic.help(help),
            None => diagnostic,
        }
        .finish()
    }
}

/// Construct whose second reading is decided after name resolution.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum AlternateKind {
    /// Const-expression reading of a path type argument.
    ConstArg,
    /// Type reading of a bracket argument that parsed as an expression.
    BracketType,
}

/// Second reading of the tokens at `span`, keyed by construct kind.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Alternate {
    /// Construct this alternate belongs to.
    pub kind: AlternateKind,
    /// Span of the primary grammar node that shares these tokens.
    pub span: Span,
    /// Standalone green node for the second reading.
    pub root: GreenNode,
}

/// One parsed source file: its lossless tree, diagnostics, and alternates.
#[derive(Debug, Clone)]
pub struct Parse {
    /// Green tree with borrowed red views.
    pub tree: SyntaxTree,
    /// Lexical and grammar errors in detection order.
    pub errors: Vec<ParseError>,
    /// Alternate readings of semantically ambiguous constructs.
    pub alternates: Vec<Alternate>,
    iterations: Vec<Iteration>,
    grammar_errors: Vec<RawError>,
    blocks: Vec<BlockRegion>,
}

impl PartialEq for Parse {
    fn eq(&self, other: &Self) -> bool {
        // Incremental dependency bounds can be more conservative than clean ones.
        self.tree == other.tree
            && self.errors == other.errors
            && self.alternates == other.alternates
    }
}

impl Eq for Parse {}

impl Parse {
    /// Returns the alternate reading of `kind` for the node spanning `span`.
    pub fn alternate(&self, kind: AlternateKind, span: Span) -> Option<&GreenNode> {
        self.alternates
            .iter()
            .find(|alternate| alternate.kind == kind && alternate.span == span)
            .map(|alternate| &alternate.root)
    }
}

/// Parses one source file into a complete grammar tree.
pub fn parse(source: &str, version: Option<SourceVersion>) -> Result<Parse, GreenBuildError> {
    let mut parser = Parser::new(source, tokenize_lossless(source));
    parser.start_root();
    let mut iterations = Vec::new();
    grammar::source_file(&mut parser, &mut iterations, |_| false);
    parser.finish_root();
    let GrammarOutput {
        events,
        errors: grammar_errors,
        alternates,
        blocks,
    } = parser.into_parts()?;
    let tree = SyntaxTree::from_green_events(source, version, events)?;
    Ok(finish_parse(
        tree,
        grammar_errors,
        alternates,
        iterations,
        blocks,
    ))
}

fn finish_parse(
    tree: SyntaxTree,
    grammar_errors: Vec<RawError>,
    alternates: Vec<Alternate>,
    iterations: Vec<Iteration>,
    blocks: Vec<BlockRegion>,
) -> Parse {
    let tokens = tree.tokens();
    let mut errors = tokens
        .iter()
        .filter_map(|token| match &token.kind {
            TokenKind::Error(error) => Some(ParseError {
                span: token.span,
                kind: ParseErrorKind::Lexical,
                message: format!("lexical error: {error}"),
                node_key: token.node_key(),
            }),
            _ => None,
        })
        .collect::<Vec<_>>();
    errors.extend(grammar_errors.iter().map(|error| ParseError {
        span: error.span,
        kind: error.kind,
        message: error.message.clone(),
        node_key: error.token.and_then(|span| {
            let index = tokens
                .binary_search_by_key(&span.start, |token| token.span.start)
                .ok()?;
            (tokens[index].span == span)
                .then(|| tokens[index].node_key())
                .flatten()
        }),
    }));
    Parse {
        tree,
        errors,
        alternates,
        iterations,
        grammar_errors,
        blocks,
    }
}

/// Token kinds that begin a top-level declaration.
pub(crate) fn is_item_keyword(kind: &TokenKind) -> bool {
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
