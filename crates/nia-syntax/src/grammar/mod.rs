// SPDX-License-Identifier: GPL-3.0-or-later
//! Nia grammar productions.
//!
//! Productions mirror the language grammar one function per construct. Each
//! production emits exactly one node on success; on failure after consuming
//! input its node becomes an `Error` node and the caller decides the recovery
//! boundary. Recovery helpers in this module always wrap the tokens they skip
//! in an `Error` node, so completed productions contain only well-formed
//! children plus zero-width `Missing` markers.

mod exprs;
mod items;
pub(crate) mod parser;
mod patterns;
mod stmts;
mod types;

use nia_lexer::TokenKind;
use nia_span::Span;

pub(crate) use items::source_file;
use parser::{Checkpoint, Parser};
pub(crate) use stmts::block;

use crate::ParseErrorKind;

impl Parser<'_> {
    pub(crate) fn at_attribute_start(&self) -> bool {
        self.at(TokenKind::At) && matches!(self.nth_kind(1), Some(TokenKind::LBracket))
    }

    pub(crate) fn at_const_fn(&self) -> bool {
        self.at(TokenKind::Const) && matches!(self.nth_kind(1), Some(TokenKind::Fn))
    }

    pub(crate) fn at_top_level_item_keyword(&self) -> bool {
        crate::is_item_keyword(self.kind())
    }

    pub(crate) fn at_top_level_item_start(&self) -> bool {
        self.at_attribute_start() || self.at_top_level_item_keyword()
    }

    pub(crate) fn at_top_level_item_boundary_except_fn(&self) -> bool {
        self.at_top_level_item_keyword() && !self.at(TokenKind::Fn)
    }

    pub(crate) fn at_namespace_segment(&self) -> bool {
        matches!(
            self.kind(),
            TokenKind::Ident | TokenKind::Pkg | TokenKind::Super | TokenKind::SelfValue
        )
    }
}

/// Consumes tokens up to a top-level stop and returns their span.
///
/// Delimiters are tracked with one shared depth, matching how recovery
/// regions nest inside already-delimited productions.
pub(crate) fn collect_until(p: &mut Parser<'_>, stops: &[TokenKind]) -> Option<Span> {
    let start = p.span().start;
    let mut depth = 0usize;
    while !p.at(TokenKind::Eof) {
        if depth == 0 && p.at_any(stops) {
            break;
        }
        match p.kind() {
            TokenKind::LParen | TokenKind::LBracket | TokenKind::LBrace => depth += 1,
            TokenKind::RParen | TokenKind::RBracket | TokenKind::RBrace => {
                depth = depth.saturating_sub(1);
            }
            _ => {}
        }
        p.bump();
    }
    let end = p.previous_end();
    if start >= end {
        p.error_here("expected syntax");
        None
    } else {
        Some(Span::new(start, end))
    }
}

/// Skips tokens in an `Error` node until one of `stops` or EOF.
pub(crate) fn skip_until(p: &mut Parser<'_>, stops: &[TokenKind]) {
    p.recover_in_error(|p| {
        while !p.at(TokenKind::Eof) && !p.at_any(stops) {
            p.bump();
        }
    });
}

#[derive(Default)]
pub(crate) struct Depth {
    paren: usize,
    bracket: usize,
    brace: usize,
}

impl Depth {
    pub(crate) fn top(&self) -> bool {
        self.paren == 0 && self.bracket == 0 && self.brace == 0
    }

    /// Tracks an opening or matched closing delimiter; returns whether it did.
    pub(crate) fn track(&mut self, kind: &TokenKind) -> bool {
        match kind {
            TokenKind::LParen => self.paren += 1,
            TokenKind::LBracket => self.bracket += 1,
            TokenKind::LBrace => self.brace += 1,
            TokenKind::RParen if self.paren > 0 => self.paren -= 1,
            TokenKind::RBracket if self.bracket > 0 => self.bracket -= 1,
            TokenKind::RBrace if self.brace > 0 => self.brace -= 1,
            _ => return false,
        }
        true
    }
}

pub(crate) fn recover_to_item_boundary(p: &mut Parser<'_>, checkpoint: Checkpoint) {
    p.recover_in_error(|p| {
        let mut depth = Depth::default();
        while !p.at(TokenKind::Eof) {
            if depth.top() {
                if p.eat(TokenKind::Semicolon).is_some() {
                    break;
                }
                if p.at_attribute_start() || p.at_top_level_item_keyword() {
                    break;
                }
            }
            if p.at(TokenKind::RBrace) && depth.brace > 0 {
                depth.brace -= 1;
                p.bump();
                if depth.top() {
                    break;
                }
                continue;
            }
            let kind = p.kind().clone();
            depth.track(&kind);
            p.bump();
        }
        p.ensure_progress(checkpoint);
    });
}

pub(crate) fn recover_to_attribute_boundary(p: &mut Parser<'_>, checkpoint: Checkpoint) {
    p.recover_in_error(|p| {
        let mut depth = Depth::default();
        while !p.at(TokenKind::Eof) {
            if depth.top()
                && (p.at(TokenKind::RBracket)
                    || p.at_attribute_start()
                    || matches!(
                        p.kind(),
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
                            | TokenKind::Let
                            | TokenKind::Return
                            | TokenKind::Break
                            | TokenKind::Continue
                            | TokenKind::Defer
                            | TokenKind::For
                            | TokenKind::While
                            | TokenKind::Loop
                            | TokenKind::If
                            | TokenKind::Match
                    ))
            {
                break;
            }
            let kind = p.kind().clone();
            depth.track(&kind);
            p.bump();
        }
        p.ensure_progress(checkpoint);
        p.eat(TokenKind::RBracket);
    });
}

pub(crate) fn recover_to_member_boundary(p: &mut Parser<'_>, checkpoint: Option<Checkpoint>) {
    p.recover_in_error(|p| {
        let mut depth = Depth::default();
        while !p.at(TokenKind::Eof) {
            let kind = p.kind().clone();
            if depth.track(&kind) {
                p.bump();
                continue;
            }
            if depth.top() {
                match kind {
                    TokenKind::Comma | TokenKind::Semicolon => {
                        p.bump();
                        break;
                    }
                    TokenKind::RBrace => break,
                    TokenKind::At if p.at_attribute_start() => break,
                    _ => {}
                }
            }
            p.bump();
        }
        if let Some(checkpoint) = checkpoint {
            p.ensure_progress(checkpoint);
        }
    });
}

pub(crate) fn recover_to_stmt_boundary(p: &mut Parser<'_>, checkpoint: Checkpoint) {
    p.recover_in_error(|p| {
        let mut depth = Depth::default();
        while !p.at(TokenKind::Eof) {
            let kind = p.kind().clone();
            if kind == TokenKind::RBrace && depth.brace > 0 {
                depth.brace -= 1;
                p.bump();
                if depth.top() {
                    p.eat(TokenKind::Semicolon);
                    break;
                }
                continue;
            }
            if depth.track(&kind) {
                p.bump();
                continue;
            }
            if depth.top() {
                match kind {
                    TokenKind::RBrace => break,
                    TokenKind::Semicolon => {
                        p.bump();
                        break;
                    }
                    _ => {}
                }
            }
            p.bump();
        }
        p.ensure_progress(checkpoint);
    });
}

/// Recovers to the end of a comma-delimited entry closed by `close`.
///
/// The comma is consumed; the closing delimiter is left for the list owner.
pub(crate) fn recover_to_list_boundary(
    p: &mut Parser<'_>,
    checkpoint: Checkpoint,
    close: TokenKind,
) {
    p.recover_in_error(|p| {
        let mut depth = Depth::default();
        while !p.at(TokenKind::Eof) {
            let kind = p.kind().clone();
            if kind == close && depth.top() {
                break;
            }
            if depth.track(&kind) {
                p.bump();
                continue;
            }
            if kind == TokenKind::Comma && depth.top() {
                p.bump();
                break;
            }
            p.bump();
        }
        p.ensure_progress(checkpoint);
    });
}

pub(crate) fn recover_to_using_boundary(p: &mut Parser<'_>, checkpoint: Checkpoint) {
    p.recover_in_error(|p| {
        let mut depth = Depth::default();
        while !p.at(TokenKind::Eof) {
            if depth.top() && (p.at(TokenKind::Comma) || p.at(TokenKind::RBrace)) {
                break;
            }
            if depth.top() && p.at_top_level_item_start() {
                break;
            }
            let kind = p.kind().clone();
            depth.track(&kind);
            p.bump();
        }
        if p.at(TokenKind::Comma) {
            p.bump();
        } else if !p.at(TokenKind::RBrace) && !p.at_top_level_item_start() {
            p.ensure_progress(checkpoint);
        }
    });
}

/// Consumes a comma after a list entry, or reports a missing separator.
///
/// Returns `true` when the list should continue with another entry.
pub(crate) fn list_separator(p: &mut Parser<'_>, next_entry_starts: bool, message: &str) -> bool {
    if p.eat(TokenKind::Comma).is_some() {
        return true;
    }
    if next_entry_starts {
        p.expected_here(ParseErrorKind::Grammar, message);
        return true;
    }
    false
}
