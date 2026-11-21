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
//!
//! The crate does not depend on AST, symbols, item trees, or semantic
//! products. A [`Parse`] retains the tree, its diagnostics, and the alternate
//! type/expression readings. Speculation rolls back token position, events,
//! forward-parent links, and alternate readings together; callers also
//! discard speculative diagnostics when abandoning a reading. Invalid green
//! events, including those of alternate trees, are internal failures rather
//! than silently discarded syntax. Red tokens carry source-versioned child
//! paths used by diagnostics and AST lowering.
//!
//! A detached grammar snapshot retains the nested alternate readings its
//! children require as well as its own events. Restoring an outer type
//! interpretation preserves inner const interpretations, such as
//! `Buffer[COUNT]` inside a call's bracket argument, and clean and incremental
//! lowering agree on those readings.

mod format;
mod grammar;
mod kind;
mod reparse;
mod tree;

#[cfg(test)]
mod tests;

pub use format::{FormatError, FormattedSource, Indentation, format_parsed, format_source};
pub use kind::SyntaxKind;
pub use tree::{
    GreenBuildError, GreenElement, GreenEvent, GreenNode, GreenToken, SyntaxNode, SyntaxToken,
    SyntaxTree, TextEdit, shift_span,
};

use nia_diagnostic::{Diagnostic, codes};
use nia_lexer::{LosslessToken, LosslessTokenKind, TokenKind, tokenize_lossless};
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
    /// True when the entire edit stays inside one representable integer
    /// literal returned directly from an ordinary integer function.
    pub fn changes_runtime_integer_return(&self, other: &Self, edit: &TextEdit) -> bool {
        let Some(end) = edit.span.start.checked_add(edit.replacement.len()) else {
            return false;
        };
        let after = Span::new(edit.span.start, end);
        self.runtime_integer_return(edit.span)
            .zip(other.runtime_integer_return(after))
            .is_some_and(|(old, new)| old == new)
            && self.tree.source().get(..edit.span.start) == other.tree.source().get(..after.start)
            && self.tree.source().get(edit.span.end..) == other.tree.source().get(after.end..)
            && other.tree.source().get(after.start..after.end) == Some(edit.replacement.as_str())
    }

    fn runtime_integer_return(&self, edit: Span) -> Option<(usize, &str, Option<&str>)> {
        if !self.errors.is_empty() {
            return None;
        }
        let covers = |node: &GreenNode| {
            let span = node.span();
            span.start <= edit.start && edit.end <= span.end
        };
        let mut function = self.tree.green_root();
        while *function.kind() != SyntaxKind::FunctionDecl {
            function = function.child_nodes().find(|child| covers(child))?;
        }
        if function.token(&TokenKind::Const).is_some() {
            return None;
        }
        let ty = function.child(&SyntaxKind::PathType)?.token_span()?;
        let ty = &self.tree.source()[ty.start..ty.end];
        let max = match ty {
            "u8" => u8::MAX.into(),
            "u16" => u16::MAX.into(),
            "u32" | "usize" => u32::MAX.into(),
            "u64" => u64::MAX.into(),
            "u128" => u128::MAX,
            "i8" => i8::MAX as u128,
            "i16" => i16::MAX as u128,
            "i32" | "isize" => i32::MAX as u128,
            "i64" => i64::MAX as u128,
            "i128" => i128::MAX as u128,
            _ => return None,
        };
        let block = function.child(&SyntaxKind::Block)?;
        let mut expr = block.child_nodes().find(|child| covers(child))?;
        if *expr.kind() == SyntaxKind::ReturnStmt {
            expr = expr.child_nodes().find(|child| covers(child))?;
        }
        while *expr.kind() == SyntaxKind::ParenExpr {
            expr = expr.child_nodes().next()?;
        }
        if *expr.kind() != SyntaxKind::LiteralExpr {
            return None;
        }
        let literal = expr.token(&TokenKind::Integer)?;
        let span = literal.span();
        if edit.start < span.start || edit.end > span.end {
            return None;
        }
        let suffix = nia_literals::numeric_literal_suffix(literal.text());
        (suffix.is_none_or(|suffix| suffix == ty)
            && nia_literals::eval_int_literal(literal.text()).ok()? <= max)
            .then_some((span.start, ty, suffix))
    }

    /// Compares error-free grammar trees, ignoring trivia and source positions.
    pub fn same_grammar(&self, other: &Self) -> bool {
        self.errors.is_empty()
            && other.errors.is_empty()
            && self.tree.green_root().same_grammar(other.tree.green_root())
            && self.alternates.len() == other.alternates.len()
            && self
                .alternates
                .iter()
                .zip(&other.alternates)
                .all(|(left, right)| left.kind == right.kind && left.root.same_grammar(&right.root))
    }

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
    let tokens = tokenize_lossless(source);
    if let Some(parse) = check_nesting(source, version, &tokens)? {
        return Ok(parse);
    }
    let mut parser = Parser::new(source, tokens);
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

// Delimited expressions enter several recursive grammar productions per level.
// Apply the same bound before clean, block and declaration parsing.
const MAX_DELIMITER_DEPTH: usize = 64;

fn check_nesting(
    source: &str,
    version: Option<SourceVersion>,
    tokens: &[LosslessToken],
) -> Result<Option<Parse>, GreenBuildError> {
    let mut depth = grammar::Depth::default();
    let Some(token) = tokens.iter().find(|token| {
        matches!(&token.kind, LosslessTokenKind::Token(kind)
            if depth.track(kind) && depth.level() > MAX_DELIMITER_DEPTH)
    }) else {
        return Ok(None);
    };
    let events = [
        GreenEvent::Start(SyntaxKind::SourceFile),
        GreenEvent::Start(SyntaxKind::Error),
    ]
    .into_iter()
    .chain(tokens.iter().cloned().map(GreenEvent::Token))
    .chain([GreenEvent::Finish, GreenEvent::Finish]);
    let tree = SyntaxTree::from_green_events(source, version, events)?;
    let error = RawError {
        span: token.span,
        token: Some(token.span),
        kind: ParseErrorKind::Grammar,
        message: format!("source exceeds the nesting limit of {MAX_DELIMITER_DEPTH} delimiters"),
    };
    Ok(Some(finish_parse(
        tree,
        vec![error],
        vec![],
        vec![],
        vec![],
    )))
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
            | TokenKind::Fun
            | TokenKind::Const
            | TokenKind::Static
            | TokenKind::Pub
    )
}
