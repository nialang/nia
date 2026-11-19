// SPDX-License-Identifier: GPL-3.0-or-later
//! Source formatting that preserves significant tokens and comments.

use std::num::NonZeroU8;

use nia_lexer::TokenKind;
use nia_span::Span;

use crate::{GreenElement, GreenNode, GreenToken, ParseError, SyntaxKind, TextEdit, parse};

/// Indentation for one level of source nesting.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Indentation {
    /// Indent with the specified positive number of spaces.
    Spaces(NonZeroU8),
    /// Indent with a single tab per level.
    Tabs,
}

impl Default for Indentation {
    fn default() -> Self {
        Self::Spaces(NonZeroU8::new(4).unwrap())
    }
}

/// A formatting attempt can only publish edits to a verified syntax tree.
#[derive(Debug)]
pub enum FormatError {
    /// Source must parse completely before a formatter can rewrite its layout.
    Syntax(ParseError),
    /// A lossless tree could not be built or its grammar changed while formatting.
    InvalidTree,
}

/// Resulting source and its non-overlapping edits in original source order.
#[derive(Debug)]
pub struct FormattedSource {
    /// Source text after applying the edits.
    pub text: String,
    /// Replacements for leading indentation and horizontal whitespace.
    pub edits: Vec<TextEdit>,
}

/// Normalizes indentation and horizontal whitespace at the ends of lines.
///
/// Existing line breaks, comments and literals are preserved. The formatted
/// source is reparsed to verify both the grammar and alternate readings.
pub fn format_source(
    source: &str,
    indentation: Indentation,
) -> Result<FormattedSource, FormatError> {
    let parsed = parse(source, None).map_err(|_| FormatError::InvalidTree)?;
    if let Some(error) = parsed.errors.first() {
        return Err(FormatError::Syntax(error.clone()));
    }
    let mut constraints = Vec::new();
    let mut tokens = Vec::new();
    layout_inputs(parsed.tree.green_root(), &mut constraints, &mut tokens);
    let unit = match indentation {
        Indentation::Spaces(width) => " ".repeat(width.get().into()),
        Indentation::Tabs => "\t".to_owned(),
    };
    let edits = layout_edits(source, &unit, &constraints, &tokens);
    let mut text = String::with_capacity(source.len());
    let mut copied = 0;
    for edit in &edits {
        text.push_str(&source[copied..edit.span.start]);
        text.push_str(&edit.replacement);
        copied = edit.span.end;
    }
    text.push_str(&source[copied..]);
    if !edits.is_empty() {
        let formatted = parse(&text, None).map_err(|_| FormatError::InvalidTree)?;
        if !parsed.same_grammar(&formatted) {
            return Err(FormatError::InvalidTree);
        }
    }
    Ok(FormattedSource { text, edits })
}

fn layout_inputs<'a>(node: &'a GreenNode, spans: &mut Vec<Span>, tokens: &mut Vec<&'a GreenToken>) {
    if node.kind() == &SyntaxKind::WhereClause
        && let Some(keyword) = node.first_token()
        && let Some(last) = node.last_token()
    {
        spans.push(Span::new(keyword.span().end, last.span().end));
    }
    for child in node.children() {
        match child {
            GreenElement::Node(node) => layout_inputs(node, spans, tokens),
            GreenElement::Token(token)
                if !matches!(
                    token.kind(),
                    SyntaxKind::Whitespace | SyntaxKind::Token(TokenKind::Eof)
                ) =>
            {
                tokens.push(token);
            }
            GreenElement::Token(_) => {}
        }
    }
}

fn layout_edits(
    source: &str,
    unit: &str,
    constraints: &[Span],
    tokens: &[&GreenToken],
) -> Vec<TextEdit> {
    let mut edits = Vec::new();
    let mut index = 0;
    let mut line_start = 0;
    let mut token_end = 0;
    let mut depth: usize = 0;
    for line in source.split_inclusive('\n') {
        let line_end = line_start + line.len();
        let body_end = line_start + line.trim_end_matches(['\r', '\n']).len();
        let start = index;
        while index < tokens.len() && tokens[index].span().start < line_end {
            index += 1;
        }
        let on_line = &tokens[start..index];
        if let Some(first) = on_line.first()
            && token_end <= line_start
        {
            let first = first.span().start;
            let existing = &source[line_start..first];
            if existing.bytes().all(|byte| matches!(byte, b' ' | b'\t')) {
                let closing = on_line
                    .iter()
                    .take_while(|token| delimiter(token.kind()) == -1)
                    .count();
                let constraint = constraints
                    .iter()
                    .any(|span| span.start <= first && first < span.end);
                let indentation =
                    unit.repeat(depth.saturating_sub(closing) + usize::from(constraint));
                if existing != indentation {
                    edits.push(TextEdit::replace(Span::new(line_start, first), indentation));
                }
            }
        }
        for token in on_line {
            depth = depth.saturating_add_signed(delimiter(token.kind()));
            token_end = token.span().end;
        }
        let trailing_start = line_start
            + source[line_start..body_end]
                .trim_end_matches([' ', '\t'])
                .len();
        if token_end <= trailing_start && trailing_start < body_end {
            edits.push(TextEdit::delete(Span::new(trailing_start, body_end)));
        }
        line_start = line_end;
    }
    edits
}

fn delimiter(kind: &SyntaxKind) -> isize {
    match kind {
        SyntaxKind::Token(TokenKind::LBrace | TokenKind::LParen | TokenKind::LBracket) => 1,
        SyntaxKind::Token(TokenKind::RBrace | TokenKind::RParen | TokenKind::RBracket) => -1,
        _ => 0,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn formats_constraints_and_nested_blocks_preserving_comments_and_crlf() {
        let source = concat!(
            "pub fun choose[T](\r\n",
            "x: T,\r\n",
            ") T\r\n",
            "where\r\n",
            " T: Sized\r\n",
            "{\r\n",
            "// Documentation { }  \r\n",
            "  _ = \"{ text }  \";  \r\n",
            " \t\r\n",
            " x\r\n",
            "}\r\n",
        );
        let expected = concat!(
            "pub fun choose[T](\r\n",
            "\tx: T,\r\n",
            ") T\r\n",
            "where\r\n",
            "\tT: Sized\r\n",
            "{\r\n",
            "\t// Documentation { }  \r\n",
            "\t_ = \"{ text }  \";\r\n",
            "\r\n",
            "\tx\r\n",
            "}\r\n",
        );
        let formatted = format_source(source, Indentation::Tabs).unwrap();
        assert_eq!(formatted.text, expected);
        assert!(
            format_source(expected, Indentation::Tabs)
                .unwrap()
                .edits
                .is_empty()
        );
    }

    #[test]
    fn keeps_error_recovery_regions_out_of_formatting_edits() {
        assert!(matches!(
            format_source("fun unfinished() () {\n _ = (", Indentation::default()),
            Err(FormatError::Syntax(_))
        ));
    }
}
