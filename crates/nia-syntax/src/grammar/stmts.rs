// SPDX-License-Identifier: GPL-3.0-or-later
//! Block, statement, and `match` grammar.

use nia_lexer::TokenKind;
use nia_span::Span;

use super::parser::{Checkpoint, Done, Parsed, Parser};
use super::{collect_until, exprs, items, patterns, recover_to_stmt_boundary, types};
use crate::{ParseErrorKind, SyntaxKind};

pub(crate) fn starts_stmt(p: &Parser<'_>) -> bool {
    matches!(
        p.kind(),
        TokenKind::Let
            | TokenKind::Return
            | TokenKind::Break
            | TokenKind::Continue
            | TokenKind::Defer
            | TokenKind::For
            | TokenKind::While
            | TokenKind::Loop
            | TokenKind::Using
            | TokenKind::Static
            | TokenKind::Const
    )
}

pub(crate) fn block(p: &mut Parser<'_>) -> Option<Parsed> {
    let prefix_read_end = p.read_end();
    let parsed = p.node(|p| {
        let start = p.expect(TokenKind::LBrace, "expected `{`")?.start;
        while !p.at(TokenKind::RBrace) && !p.at(TokenKind::Eof) {
            if starts_stmt(p) || p.at_attribute_start() {
                let checkpoint = p.checkpoint();
                if stmt(p).is_none() {
                    recover_to_stmt_boundary(p, checkpoint);
                }
                continue;
            }
            let checkpoint = p.checkpoint();
            if !expr_stmt_or_tail(p, checkpoint) {
                break;
            }
        }
        let end = p.expect(TokenKind::RBrace, "expected `}` after block")?.end;
        Done::new(SyntaxKind::Block, Span::new(start, end))
    })?;
    p.record_block(parsed.span, prefix_read_end);
    Some(parsed)
}

/// Parses a block expression statement or the block's tail expression.
///
/// A tail expression stays a direct child of the block; an expression
/// statement wraps the expression and its optional terminator. Returns
/// `false` once the tail expression has been parsed.
fn expr_stmt_or_tail(p: &mut Parser<'_>, checkpoint: Checkpoint) -> bool {
    let Some(expr) = exprs::expr(p) else {
        recover_to_stmt_boundary(p, checkpoint);
        return true;
    };
    if !p.at(TokenKind::Semicolon) && p.at(TokenKind::RBrace) {
        return false;
    }
    p.node_preceding(&expr, |p| {
        if p.eat(TokenKind::Semicolon).is_none() && !exprs::can_terminate_statement(&expr) {
            p.error_at_end_as(
                ParseErrorKind::MissingSemicolon,
                expr.span,
                "expected `;` after expression",
            );
        }
        Done::new(SyntaxKind::ExprStmt, expr.span)
    });
    true
}

pub(crate) fn stmt(p: &mut Parser<'_>) -> Option<Parsed> {
    p.node(|p| {
        let attributes = items::attributes(p);
        let start = attributes.start.unwrap_or(p.span().start);
        if p.at(TokenKind::Pub) {
            let span = p.span();
            p.error_at(
                span,
                "`pub` is not allowed on statements; only top-level `pub using` is permitted",
            );
            p.recover_in_error(|p| {
                p.bump();
            });
        }
        let done = |p: &Parser<'_>, kind| Done::new(kind, Span::new(start, p.previous_end()));
        if p.eat(TokenKind::Using).is_some() {
            items::using_tree(p)?;
            p.expect(TokenKind::Semicolon, "expected `;` after using")?;
            return done(p, SyntaxKind::UsingStmt);
        }
        if p.at(TokenKind::Static) {
            items::binding_decl(p, false, true)?;
            return done(p, SyntaxKind::StaticStmt);
        }
        if p.at(TokenKind::Const) || p.at(TokenKind::Let) {
            let_stmt(p)?;
            return done(p, SyntaxKind::LetStmt);
        }
        if p.eat(TokenKind::Return).is_some() {
            if !p.at(TokenKind::Semicolon) {
                exprs::expr(p)?;
            }
            p.expect(TokenKind::Semicolon, "expected `;` after return")?;
            return done(p, SyntaxKind::ReturnStmt);
        }
        if p.eat(TokenKind::Break).is_some() {
            p.expect(TokenKind::Semicolon, "expected `;` after break")?;
            return done(p, SyntaxKind::BreakStmt);
        }
        if p.eat(TokenKind::Continue).is_some() {
            p.expect(TokenKind::Semicolon, "expected `;` after continue")?;
            return done(p, SyntaxKind::ContinueStmt);
        }
        if p.eat(TokenKind::Defer).is_some() {
            exprs::expr_until_tokens(p, &[TokenKind::Semicolon, TokenKind::RBrace])?;
            p.expect(TokenKind::Semicolon, "expected `;` after defer")?;
            return done(p, SyntaxKind::DeferStmt);
        }
        if p.at(TokenKind::For) {
            for_stmt(p)?;
            return done(p, SyntaxKind::ForStmt);
        }
        if p.at(TokenKind::While) {
            p.bump();
            exprs::expr_until(p, &[TokenKind::LBrace])?;
            block(p)?;
            return done(p, SyntaxKind::WhileStmt);
        }
        if p.at(TokenKind::Loop) {
            p.bump();
            block(p)?;
            return done(p, SyntaxKind::LoopStmt);
        }
        if attributes.count == 0 {
            return None;
        }
        let expr = exprs::expr(p)?;
        let has_semicolon = p.eat(TokenKind::Semicolon).is_some();
        if !has_semicolon && !exprs::can_terminate_statement(&expr) {
            p.error_at_end_as(
                ParseErrorKind::MissingSemicolon,
                expr.span,
                "expected `;` after expression",
            );
        }
        done(p, SyntaxKind::ExprStmt)
    })
}

fn let_stmt(p: &mut Parser<'_>) -> Option<()> {
    let is_const = p.eat(TokenKind::Const).is_some();
    if is_const {
        if !patterns::starts_binding_pattern(p) {
            p.error_here("expected const binding");
            return None;
        }
        if p.at(TokenKind::Mut) {
            p.error_here("const bindings cannot be mutable; use `let mut` inside const evaluation");
            return None;
        }
    } else {
        p.bump();
    }
    p.eat(TokenKind::Mut);
    patterns::irrefutable_pattern_until(
        p,
        &[TokenKind::Colon, TokenKind::Eq, TokenKind::Semicolon],
    )?;
    let mut anchor = None;
    if p.eat(TokenKind::Colon).is_some() {
        anchor = Some(types::type_until(p, &[TokenKind::Eq, TokenKind::Semicolon])?.span);
    }
    let mut has_value = false;
    if p.eat(TokenKind::Eq).is_some() {
        anchor = Some(exprs::expr_until_tokens(p, &[TokenKind::Semicolon])?.span);
        has_value = true;
    }
    if is_const && !has_value {
        p.error_here("const binding requires an initializer");
        return None;
    }
    let anchor = anchor.unwrap_or_else(|| Span::new(p.previous_end(), p.previous_end()));
    p.expect_semicolon_after(anchor, "expected `;` after binding")?;
    Some(())
}

fn for_stmt(p: &mut Parser<'_>) -> Option<()> {
    p.bump();
    patterns::irrefutable_pattern_until(p, &[TokenKind::In, TokenKind::Colon])?;
    if p.at(TokenKind::Colon) {
        p.error_here("for patterns do not support type annotations");
        let mut collected = false;
        p.recover_in_error(|p| {
            collected = collect_until(p, &[TokenKind::LBrace]).is_some();
        });
        if !collected {
            return None;
        }
        block(p)?;
        return Some(());
    }
    p.expect(TokenKind::In, "expected `in` after for pattern")?;
    exprs::expr_until(p, &[TokenKind::LBrace])?;
    block(p)?;
    Some(())
}

pub(crate) fn match_expr(p: &mut Parser<'_>) -> Option<Parsed> {
    p.node(|p| {
        let start = p.bump().start;
        exprs::expr_until_tokens(p, &[TokenKind::LBrace])?;
        p.expect(TokenKind::LBrace, "expected `{` after match target")?;
        while !p.at(TokenKind::RBrace) && !p.at(TokenKind::Eof) {
            if match_arm(p).is_none() {
                recover_match_arm_boundary(p);
            } else {
                p.eat(TokenKind::Comma);
            }
        }
        p.expect(TokenKind::RBrace, "expected `}` after match")?;
        Done::new(SyntaxKind::MatchExpr, Span::new(start, p.previous_end()))
    })
}

fn match_arm(p: &mut Parser<'_>) -> Option<Parsed> {
    p.node(|p| {
        let start = p.span().start;
        match_arm_patterns(p)?;
        p.expect(TokenKind::FatArrow, "expected `=>` in match arm")?;
        let body = match_arm_body(p)?;
        Done::new(SyntaxKind::MatchArm, Span::new(start, body.span.end))
    })
}

fn recover_match_arm_boundary(p: &mut Parser<'_>) {
    p.recover_in_error(|p| {
        let mut depth = super::Depth::default();
        while !p.at(TokenKind::Eof) {
            let kind = p.kind().clone();
            if depth.track(&kind) {
                p.bump();
                continue;
            }
            if depth.top() {
                match kind {
                    TokenKind::Comma => {
                        p.bump();
                        break;
                    }
                    TokenKind::RBrace => break,
                    _ => {}
                }
            }
            p.bump();
        }
    });
}

fn match_arm_patterns(p: &mut Parser<'_>) -> Option<()> {
    let mut patterns = 0usize;
    loop {
        let checkpoint = p.checkpoint();
        if patterns::pattern_until(p, &[TokenKind::Comma, TokenKind::FatArrow]).is_none() {
            if recover_match_pattern_boundary(p, checkpoint) {
                continue;
            }
            return (patterns == 0).then_some(());
        }
        patterns += 1;
        if p.at(TokenKind::FatArrow) {
            break;
        }
        if p.eat(TokenKind::Comma).is_none() {
            if patterns::pattern_can_start(p.kind()) {
                p.expected_here(
                    ParseErrorKind::Grammar,
                    "expected `,` or `=>` after match pattern",
                );
                continue;
            }
            p.expect(TokenKind::Comma, "expected `,` or `=>` after match pattern")?;
        }
        if p.at(TokenKind::FatArrow) {
            p.error_here("trailing comma is not allowed in match pattern list");
            break;
        }
    }
    Some(())
}

fn recover_match_pattern_boundary(p: &mut Parser<'_>, checkpoint: Checkpoint) -> bool {
    let mut continue_list = false;
    p.recover_in_error(|p| {
        let mut depth = super::Depth::default();
        while !p.at(TokenKind::Eof) {
            let kind = p.kind().clone();
            if depth.track(&kind) {
                p.bump();
                continue;
            }
            if depth.top() {
                match kind {
                    TokenKind::Comma => {
                        p.bump();
                        continue_list = true;
                        return;
                    }
                    TokenKind::FatArrow | TokenKind::RBrace => return,
                    _ => {}
                }
            }
            p.bump();
        }
        p.ensure_progress(checkpoint);
    });
    continue_list
}

fn match_arm_body(p: &mut Parser<'_>) -> Option<Parsed> {
    if p.at(TokenKind::LBrace) {
        return block(p);
    }
    if starts_stmt(p) {
        return match_arm_stmt(p);
    }
    exprs::expr(p)
}

/// Arm bodies accept `return`, `break`, `continue`, and `defer` without a
/// terminator, ending at the arm's `,` or `}`; other statements parse
/// normally.
fn match_arm_stmt(p: &mut Parser<'_>) -> Option<Parsed> {
    let terminators = [TokenKind::Comma, TokenKind::RBrace];
    match p.kind() {
        TokenKind::Return => p.node(|p| {
            let start = p.bump().start;
            let mut end = p.previous_end();
            if !p.at_any(&terminators) {
                end = exprs::expr_until(p, &terminators)?.span.end;
            }
            Done::new(SyntaxKind::ReturnStmt, Span::new(start, end))
        }),
        TokenKind::Break => p.node(|p| {
            let span = p.bump();
            Done::new(SyntaxKind::BreakStmt, span)
        }),
        TokenKind::Continue => p.node(|p| {
            let span = p.bump();
            Done::new(SyntaxKind::ContinueStmt, span)
        }),
        TokenKind::Defer => p.node(|p| {
            let start = p.bump().start;
            let expr = exprs::expr_until(p, &terminators)?;
            Done::new(SyntaxKind::DeferStmt, Span::new(start, expr.span.end))
        }),
        _ => stmt(p),
    }
}
