// SPDX-License-Identifier: GPL-3.0-or-later
//! Refutable and irrefutable pattern grammar.
//!
//! Binding contexts (`let`, `for`) accept only the irrefutable subset;
//! `match` arms, `is` conditions, and nominal payloads accept every pattern.

use nia_lexer::TokenKind;
use nia_span::Span;

use super::parser::{Done, Parsed, Parser, Shape};
use super::{exprs, recover_to_list_boundary};
use crate::{ParseErrorKind, SyntaxKind};

pub(crate) fn pattern_can_start(kind: &TokenKind) -> bool {
    matches!(
        kind,
        TokenKind::Integer
            | TokenKind::Float
            | TokenKind::String
            | TokenKind::ByteString
            | TokenKind::Char
            | TokenKind::ByteChar
            | TokenKind::True
            | TokenKind::False
            | TokenKind::Null
            | TokenKind::Ident
            | TokenKind::SelfValue
            | TokenKind::Pkg
            | TokenKind::Super
            | TokenKind::Underscore
            | TokenKind::LParen
            | TokenKind::Question
            | TokenKind::Bang
            | TokenKind::Amp
            | TokenKind::Mut
            | TokenKind::Minus
    )
}

pub(crate) fn starts_binding_pattern(p: &Parser<'_>) -> bool {
    matches!(
        p.kind(),
        TokenKind::Ident
            | TokenKind::Underscore
            | TokenKind::Amp
            | TokenKind::Mut
            | TokenKind::LParen
    )
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum Context {
    Binding,
    Match,
}

pub(crate) fn irrefutable_pattern_until(p: &mut Parser<'_>, stops: &[TokenKind]) -> Option<Parsed> {
    let pattern = atom(p, stops, Context::Binding)?;
    if p.at(TokenKind::Bang) {
        p.recover_in_error(|p| {
            p.bump();
        });
        p.error_here("binding patterns do not support error payload suffix `!`");
    }
    Some(pattern)
}

pub(crate) fn pattern_until(p: &mut Parser<'_>, stops: &[TokenKind]) -> Option<Parsed> {
    let mut pattern = atom(p, stops, Context::Match)?;
    while p.at(TokenKind::Bang) {
        let inner = pattern.clone();
        pattern = p.node_preceding(&inner, |p| {
            p.bump();
            Done::new(
                SyntaxKind::ErrorErrPattern,
                Span::new(inner.span.start, p.previous_end()),
            )
        })?;
    }
    Some(pattern)
}

fn nested(p: &mut Parser<'_>, stops: &[TokenKind], context: Context) -> Option<Parsed> {
    match context {
        Context::Binding => irrefutable_pattern_until(p, stops),
        Context::Match => pattern_until(p, stops),
    }
}

fn atom(p: &mut Parser<'_>, stops: &[TokenKind], context: Context) -> Option<Parsed> {
    let start = p.span().start;
    match p.kind() {
        TokenKind::Amp => {
            return p.node(|p| {
                p.bump();
                p.eat(TokenKind::Mut);
                let inner = atom(p, stops, context)?;
                Done::new(SyntaxKind::PointerPattern, Span::new(start, inner.span.end))
            });
        }
        TokenKind::Mut => {
            return p.node(|p| {
                p.bump();
                let inner = atom(p, stops, context)?;
                Done::wrapping(
                    SyntaxKind::MutPattern,
                    &inner,
                    Span::new(start, inner.span.end),
                )
            });
        }
        TokenKind::Underscore => {
            return p.node(|p| {
                let span = p.bump();
                Done::new(SyntaxKind::WildcardPattern, span)
            });
        }
        TokenKind::LParen => return tuple_or_paren(p, context),
        _ => {}
    }
    if context == Context::Match {
        match p.kind() {
            TokenKind::Question | TokenKind::Bang => {
                let kind = if p.at(TokenKind::Question) {
                    SyntaxKind::OptionalPattern
                } else {
                    SyntaxKind::ErrorOkPattern
                };
                return p.node(|p| {
                    p.bump();
                    let inner = pattern_until(p, stops)?;
                    Done::new(kind, Span::new(start, inner.span.end))
                });
            }
            TokenKind::Null => {
                return p.node(|p| {
                    let span = p.bump();
                    Done::new(SyntaxKind::NullPattern, span)
                });
            }
            _ => {}
        }
    }
    if let Some(pattern) = nominal(p, stops) {
        return Some(pattern);
    }
    let bare = p.at(TokenKind::Ident)
        && p.nth_kind(1).is_some_and(|next| {
            stops.contains(next) || (context == Context::Match && *next == TokenKind::Bang)
        });
    if bare {
        return p.node(|p| {
            let span = p.bump();
            Done::new(SyntaxKind::BindPattern, span)
        });
    }
    if context == Context::Binding {
        p.error_here_as(
            ParseErrorKind::ExpectedBindingPattern,
            "expected binding pattern",
        );
        p.missing();
        return None;
    }
    let mut expr_stops = stops.to_vec();
    if !expr_stops.contains(&TokenKind::Bang) {
        expr_stops.push(TokenKind::Bang);
    }
    let expr = exprs::expr_until_tokens(p, &expr_stops)?;
    let closed_range = expr
        .shape
        .contains(Shape::RANGE_START.with(Shape::RANGE_END));
    if expr.class == SyntaxKind::RangeExpr && !closed_range {
        p.error_at(
            expr.span,
            "open-ended match range patterns are not supported; use `_` for the default arm",
        );
    }
    let kind = if expr.class == SyntaxKind::RangeExpr && closed_range {
        SyntaxKind::RangePattern
    } else {
        SyntaxKind::ExprPattern
    };
    p.node_preceding(&expr, |_| Done::new(kind, expr.span))
}

fn tuple_or_paren(p: &mut Parser<'_>, context: Context) -> Option<Parsed> {
    const STOPS: [TokenKind; 2] = [TokenKind::Comma, TokenKind::RParen];
    p.node(|p| {
        let start = p.bump().start;
        if p.eat(TokenKind::RParen).is_some() {
            return Done::new(SyntaxKind::TuplePattern, Span::new(start, p.previous_end()));
        }
        let first_checkpoint = p.checkpoint();
        let first = nested(p, &STOPS, context);
        if first.is_none() && !p.at(TokenKind::RParen) {
            recover_to_list_boundary(p, first_checkpoint, TokenKind::RParen);
        }
        match &first {
            None if p.at(TokenKind::RParen) => {
                p.expect(TokenKind::RParen, "expected `)` after tuple pattern")?;
                return Done::new(SyntaxKind::TuplePattern, Span::new(start, p.previous_end()));
            }
            Some(first) if p.eat(TokenKind::Comma).is_none() => {
                if pattern_can_start(p.kind()) {
                    p.expected_here(
                        ParseErrorKind::Grammar,
                        "expected `,` or `)` after tuple pattern",
                    );
                } else {
                    p.expect(
                        TokenKind::RParen,
                        "expected `)` after parenthesized pattern",
                    )?;
                    return Done::wrapping(
                        SyntaxKind::ParenPattern,
                        first,
                        Span::new(start, p.previous_end()),
                    );
                }
            }
            _ => {}
        }
        while !p.at(TokenKind::RParen) && !p.at(TokenKind::Eof) {
            let checkpoint = p.checkpoint();
            let field = nested(p, &STOPS, context);
            if context == Context::Binding && field.is_none() {
                if p.eat(TokenKind::Comma).is_none() {
                    if p.at(TokenKind::RParen) || p.at(TokenKind::Eof) {
                        break;
                    }
                    recover_to_list_boundary(p, checkpoint, TokenKind::RParen);
                }
                continue;
            }
            if p.eat(TokenKind::Comma).is_none() {
                if p.at(TokenKind::RParen) || p.at(TokenKind::Eof) {
                    break;
                } else if pattern_can_start(p.kind()) {
                    p.expected_here(
                        ParseErrorKind::Grammar,
                        "expected `,` or `)` after tuple pattern",
                    );
                    continue;
                }
                recover_to_list_boundary(p, checkpoint, TokenKind::RParen);
            }
        }
        let end = p
            .expect(TokenKind::RParen, "expected `)` after tuple pattern")?
            .end;
        Done::new(SyntaxKind::TuplePattern, Span::new(start, end))
    })
}

fn nominal(p: &mut Parser<'_>, stops: &[TokenKind]) -> Option<Parsed> {
    let checkpoint = p.checkpoint();
    let errors = p.errors_len();
    let (constructor, qualified) = if let Some(constructor) = exprs::omitted_constructor(p) {
        (constructor, true)
    } else if let Some(constructor) = exprs::qualified_value_path(p) {
        (constructor, true)
    } else {
        p.rewind(checkpoint);
        p.truncate_errors(errors);
        if !p.at(TokenKind::Ident) {
            return None;
        }
        let constructor = p.node(|p| {
            let span = p.bump();
            Done::new(SyntaxKind::NameExpr, span)
        })?;
        (constructor, false)
    };
    let accepted = if p.at(TokenKind::LParen) {
        true
    } else if p.at(TokenKind::LBrace) {
        brace_is_followed_by_pattern_boundary(p, stops)
    } else {
        false
    };
    if !accepted {
        let _ = qualified;
        p.rewind(checkpoint);
        p.truncate_errors(errors);
        return None;
    }
    p.node_preceding(&constructor, |p| {
        if p.at(TokenKind::LParen) {
            tuple_fields(p)?;
        } else {
            named_fields(p)?;
        }
        Done::new(
            SyntaxKind::NominalPattern,
            Span::new(constructor.span.start, p.previous_end()),
        )
    })
}

fn tuple_fields(p: &mut Parser<'_>) -> Option<Parsed> {
    p.node(|p| {
        let start = p.bump().start;
        while !p.at(TokenKind::RParen) && !p.at(TokenKind::Eof) {
            let checkpoint = p.checkpoint();
            if pattern_until(p, &[TokenKind::Comma, TokenKind::RParen]).is_none() {
                if p.eat(TokenKind::Comma).is_some() {
                    continue;
                }
                if p.at(TokenKind::RParen) {
                    break;
                }
                recover_to_list_boundary(p, checkpoint, TokenKind::RParen);
                continue;
            }
            if p.eat(TokenKind::Comma).is_none() {
                if pattern_can_start(p.kind()) {
                    p.expected_here(
                        ParseErrorKind::Grammar,
                        "expected `,` or `)` after tuple pattern",
                    );
                    continue;
                }
                break;
            }
        }
        p.expect(TokenKind::RParen, "expected `)` after enum variant pattern")?;
        Done::new(
            SyntaxKind::NominalTupleFields,
            Span::new(start, p.previous_end()),
        )
    })
}

fn named_fields(p: &mut Parser<'_>) -> Option<Parsed> {
    p.node(|p| {
        let start = p.bump().start;
        let mut has_rest = false;
        while !p.at(TokenKind::RBrace) && !p.at(TokenKind::Eof) {
            let checkpoint = p.checkpoint();
            if p.at(TokenKind::DotDot) {
                let rest = p.bump();
                if has_rest {
                    p.error_at(rest, "nominal pattern may contain `..` only once");
                }
                has_rest = true;
                if p.eat(TokenKind::Comma).is_some() {
                    if !p.at(TokenKind::RBrace) {
                        let span = p.span();
                        p.error_at(span, "`..` must be the final nominal pattern field");
                    }
                } else if !p.at(TokenKind::RBrace) {
                    let span = p.span();
                    p.error_at(span, "expected `,` or `}` after nominal pattern `..`");
                }
                continue;
            }
            let mut failed = false;
            p.node(|p| {
                let start = p.span().start;
                if p.expect_name("expected payload field name").is_none() {
                    failed = true;
                    return None;
                }
                if p.eat(TokenKind::Colon).is_some()
                    && pattern_until(p, &[TokenKind::Comma, TokenKind::RBrace]).is_none()
                {
                    failed = true;
                    return None;
                }
                Done::new(
                    SyntaxKind::NamedPatternField,
                    Span::new(start, p.previous_end()),
                )
            });
            if failed {
                if p.eat(TokenKind::Comma).is_some() {
                    continue;
                }
                if p.at(TokenKind::RBrace) {
                    break;
                }
                recover_to_list_boundary(p, checkpoint, TokenKind::RBrace);
                continue;
            }
            if p.eat(TokenKind::Comma).is_none() {
                if p.at(TokenKind::Ident) {
                    p.expected_here(
                        ParseErrorKind::Grammar,
                        "expected `,` or `}` after nominal pattern field",
                    );
                    continue;
                }
                break;
            }
        }
        p.expect(TokenKind::RBrace, "expected `}` after enum variant pattern")?;
        Done::new(
            SyntaxKind::NominalNamedFields,
            Span::new(start, p.previous_end()),
        )
    })
}

fn brace_is_followed_by_pattern_boundary(p: &Parser<'_>, stops: &[TokenKind]) -> bool {
    let mut depth = 0usize;
    let mut offset = 0usize;
    loop {
        match p.nth_kind(offset) {
            Some(TokenKind::LBrace) => depth += 1,
            Some(TokenKind::RBrace) => {
                depth = depth.saturating_sub(1);
                if depth == 0 {
                    // `!` belongs to the surrounding pattern grammar, so it is
                    // a boundary only when the token after that suffix is also
                    // a caller boundary. This keeps an `if` body's `{ ... } !()`
                    // tail from reading as a nominal pattern `error { ... }!`.
                    return match p.nth_kind(offset + 1) {
                        Some(TokenKind::Bang) => p
                            .nth_kind(offset + 2)
                            .is_some_and(|kind| stops.contains(kind)),
                        Some(kind) => stops.contains(kind),
                        None => false,
                    };
                }
            }
            Some(TokenKind::Eof) | None => return false,
            _ => {}
        }
        offset += 1;
    }
}
