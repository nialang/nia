// SPDX-License-Identifier: GPL-3.0-or-later
//! Expression and conditional-compilation grammar.

use nia_lexer::TokenKind;
use nia_span::Span;

use super::parser::{Done, Parsed, Parser, Shape};
use super::{
    collect_until, patterns, recover_to_list_boundary, recover_to_member_boundary, skip_until,
    stmts, types,
};
use crate::{AlternateKind, ParseErrorKind, SyntaxKind};

pub(crate) fn expr_can_start(kind: &TokenKind) -> bool {
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
            | TokenKind::Backslash
            | TokenKind::LBracket
            | TokenKind::LParen
            | TokenKind::LBrace
            | TokenKind::If
            | TokenKind::Match
            | TokenKind::Not
            | TokenKind::Minus
            | TokenKind::Tilde
            | TokenKind::Bang
            | TokenKind::Question
            | TokenKind::Amp
    )
}

/// Expressions that end a statement without `;` at a line break.
pub(crate) fn can_terminate_statement(expr: &Parsed) -> bool {
    matches!(
        expr.class,
        SyntaxKind::Block | SyntaxKind::IfExpr | SyntaxKind::MatchExpr
    )
}

pub(crate) fn binary_precedence(kind: &TokenKind) -> Option<u8> {
    Some(match kind {
        TokenKind::Or => 1,
        TokenKind::And => 2,
        TokenKind::Pipe => 3,
        TokenKind::Caret => 4,
        TokenKind::Amp => 5,
        TokenKind::EqEq | TokenKind::BangEq => 6,
        TokenKind::Lt | TokenKind::LtEq | TokenKind::Gt | TokenKind::GtEq => 7,
        TokenKind::LtLt | TokenKind::GtGt => 8,
        TokenKind::Plus | TokenKind::Minus => 9,
        TokenKind::Star | TokenKind::Slash | TokenKind::Percent => 10,
        _ => return None,
    })
}

pub(crate) fn is_assignment_op(kind: &TokenKind) -> bool {
    matches!(
        kind,
        TokenKind::Eq
            | TokenKind::PlusEq
            | TokenKind::MinusEq
            | TokenKind::LtLtEq
            | TokenKind::GtGtEq
            | TokenKind::StarEq
            | TokenKind::SlashEq
            | TokenKind::PercentEq
            | TokenKind::AmpEq
            | TokenKind::CaretEq
            | TokenKind::PipeEq
    )
}

pub(crate) fn expr(p: &mut Parser<'_>) -> Option<Parsed> {
    assignment_until(p, &[])
}

/// Parses an expression, falling back to a raw expression up to `stops`.
pub(crate) fn expr_until(p: &mut Parser<'_>, stops: &[TokenKind]) -> Option<Parsed> {
    if let Some(expr) = expr_until_tokens(p, stops) {
        return Some(expr);
    }
    p.node(|p| {
        let span = collect_until(p, stops)?;
        Done::new(SyntaxKind::RawExpr, span)
    })
}

/// Parses an expression that stops at one of `stops`.
///
/// An assignment that does not end at a stop is reparsed as a binary
/// expression so a trailing `=` stays with the enclosing construct.
pub(crate) fn expr_until_tokens(p: &mut Parser<'_>, stops: &[TokenKind]) -> Option<Parsed> {
    let checkpoint = p.checkpoint();
    let errors = p.errors_len();
    let expr = assignment_until(p, stops)?;
    if p.at_any(stops) {
        return Some(expr);
    }
    p.rewind(checkpoint);
    p.truncate_errors(errors);
    binary_until(p, 0, stops)
}

fn assignment_until(p: &mut Parser<'_>, stops: &[TokenKind]) -> Option<Parsed> {
    let lhs = range_until(p, stops)?;
    if !is_assignment_op(p.kind()) {
        return Some(lhs);
    }
    p.node_preceding(&lhs, |p| {
        p.bump();
        let rhs = assignment_until(p, stops)?;
        Done::new(
            SyntaxKind::AssignExpr,
            Span::new(lhs.span.start, rhs.span.end),
        )
    })
}

fn range_end_absent(p: &Parser<'_>, stops: &[TokenKind]) -> bool {
    p.at_any(stops)
        || p.at_any(&[
            TokenKind::Comma,
            TokenKind::RParen,
            TokenKind::RBracket,
            TokenKind::RBrace,
            TokenKind::Semicolon,
            TokenKind::Eof,
        ])
}

fn range_until(p: &mut Parser<'_>, stops: &[TokenKind]) -> Option<Parsed> {
    if p.at(TokenKind::DotDot) || p.at(TokenKind::DotDotEq) {
        return p.node(|p| {
            let start = p.span().start;
            let inclusive = p.at(TokenKind::DotDotEq);
            p.bump();
            let mut shape = Shape::default();
            let end = if !inclusive && range_end_absent(p, stops) {
                p.previous_end()
            } else {
                shape = shape.with(Shape::RANGE_END);
                binary_until(p, 0, stops)?.span.end
            };
            Done::shaped(SyntaxKind::RangeExpr, Span::new(start, end), shape)
        });
    }
    let mut range_stops = stops.to_vec();
    for stop in [
        TokenKind::DotDot,
        TokenKind::DotDotEq,
        TokenKind::Comma,
        TokenKind::RParen,
        TokenKind::RBracket,
        TokenKind::RBrace,
        TokenKind::Semicolon,
    ] {
        if !range_stops.contains(&stop) {
            range_stops.push(stop);
        }
    }
    let start = binary_until(p, 0, &range_stops)?;
    if p.at_any(stops) {
        return Some(start);
    }
    if p.at(TokenKind::DotDot) {
        return p.node_preceding(&start, |p| {
            p.bump();
            let mut shape = Shape::RANGE_START;
            let end = if range_end_absent(p, stops) {
                p.previous_end()
            } else {
                shape = shape.with(Shape::RANGE_END);
                binary_until(p, 0, stops)?.span.end
            };
            Done::shaped(
                SyntaxKind::RangeExpr,
                Span::new(start.span.start, end),
                shape,
            )
        });
    }
    if p.at(TokenKind::DotDotEq) {
        return p.node_preceding(&start, |p| {
            p.bump();
            let end = binary_until(p, 0, stops)?;
            Done::shaped(
                SyntaxKind::RangeExpr,
                Span::new(start.span.start, end.span.end),
                Shape::RANGE_START.with(Shape::RANGE_END),
            )
        });
    }
    Some(start)
}

pub(crate) fn binary_until(
    p: &mut Parser<'_>,
    min_prec: u8,
    stops: &[TokenKind],
) -> Option<Parsed> {
    let mut lhs = not_until(p, stops)?;
    while let Some(prec) = binary_precedence(p.kind()) {
        if p.at_any(stops) {
            break;
        }
        if can_terminate_statement(&lhs) && p.has_line_break_between(lhs.span.end, p.span().start) {
            break;
        }
        if prec < min_prec {
            break;
        }
        let left = lhs.clone();
        lhs = p.node_preceding(&left, |p| {
            p.bump();
            let rhs = binary_until(p, prec + 1, stops)?;
            Done::new(
                SyntaxKind::BinaryExpr,
                Span::new(left.span.start, rhs.span.end),
            )
        })?;
    }
    Some(lhs)
}

fn not_until(p: &mut Parser<'_>, stops: &[TokenKind]) -> Option<Parsed> {
    if !p.at(TokenKind::Not) {
        return cast_until(p, stops);
    }
    p.node(|p| {
        let start = p.bump().start;
        let expr = binary_until(p, 6, stops)?;
        Done::new(SyntaxKind::NotExpr, Span::new(start, expr.span.end))
    })
}

fn cast_until(p: &mut Parser<'_>, stops: &[TokenKind]) -> Option<Parsed> {
    let mut expr = unary_until(p, stops)?;
    while p.at(TokenKind::As) {
        let inner = expr.clone();
        expr = p.node_preceding(&inner, |p| {
            p.bump();
            let ty = types::type_(p)?;
            Done::new(
                SyntaxKind::CastExpr,
                Span::new(inner.span.start, ty.span.end),
            )
        })?;
    }
    Some(expr)
}

fn unary_until(p: &mut Parser<'_>, stops: &[TokenKind]) -> Option<Parsed> {
    if !matches!(
        p.kind(),
        TokenKind::Minus
            | TokenKind::Tilde
            | TokenKind::Amp
            | TokenKind::Question
            | TokenKind::Bang
    ) {
        return postfix_until(p, stops);
    }
    p.node(|p| {
        let is_amp = p.at(TokenKind::Amp);
        let start = p.bump().start;
        if is_amp {
            p.eat(TokenKind::Mut);
        }
        let expr = unary_until(p, stops)?;
        Done::new(SyntaxKind::PrefixExpr, Span::new(start, expr.span.end))
    })
}

fn postfix_until(p: &mut Parser<'_>, stops: &[TokenKind]) -> Option<Parsed> {
    let mut expr = primary_until(p, stops)?;
    loop {
        if p.at_any(stops) {
            break;
        }
        if can_terminate_statement(&expr) && p.has_line_break_between(expr.span.end, p.span().start)
        {
            break;
        }
        let lhs = expr.clone();
        let start = lhs.span.start;
        let next = match p.kind() {
            TokenKind::LParen => p.node_preceding(&lhs, |p| {
                arg_list(p)?;
                Done::new(SyntaxKind::CallExpr, Span::new(start, p.previous_end()))
            }),
            TokenKind::Dot => p.node_preceding(&lhs, |p| {
                p.bump();
                let kind = if p.eat(TokenKind::Question).is_some() {
                    SyntaxKind::TryExpr
                } else if p.eat(TokenKind::Star).is_some() {
                    SyntaxKind::DerefExpr
                } else if p.at(TokenKind::Integer) {
                    tuple_field_index(p)?;
                    SyntaxKind::TupleFieldExpr
                } else {
                    p.expect_name("expected field name")?;
                    SyntaxKind::FieldExpr
                };
                Done::new(kind, Span::new(start, p.previous_end()))
            }),
            TokenKind::Bang => p.node_preceding(&lhs, |p| {
                p.bump();
                Done::new(SyntaxKind::ErrorErrExpr, Span::new(start, p.previous_end()))
            }),
            TokenKind::ColonColon => p.node_preceding(&lhs, |p| {
                p.bump();
                p.expect_name("expected name after `::`")?;
                let shape = if lhs.shape.contains(Shape::VALUE_PATH) {
                    Shape::VALUE_PATH
                } else {
                    Shape::default()
                };
                Done::shaped(
                    SyntaxKind::QualifiedExpr,
                    Span::new(start, p.previous_end()),
                    shape,
                )
            }),
            TokenKind::LBracket => p.node_preceding(&lhs, |p| {
                p.bump();
                bracket_suffix_after_open(p)?;
                p.expect(TokenKind::RBracket, "expected `]` after bracket suffix")?;
                Done::new(SyntaxKind::BracketExpr, Span::new(start, p.previous_end()))
            }),
            _ => break,
        };
        expr = next?;
    }
    Some(expr)
}

fn tuple_field_index(p: &mut Parser<'_>) -> Option<()> {
    let text = p.text();
    let span = p.bump();
    if !text.chars().all(|ch| ch.is_ascii_digit()) {
        p.error_at(span, "tuple field must be a decimal integer");
        return None;
    }
    if text.len() > 1 && text.starts_with('0') {
        p.error_at(span, "tuple field must not contain leading zeroes");
        return None;
    }
    if text.parse::<usize>().is_err() {
        p.error_at(span, "tuple field index is too large");
        return None;
    }
    Some(())
}

fn arg_list(p: &mut Parser<'_>) -> Option<Parsed> {
    p.node(|p| {
        let start = p.bump().start;
        while !p.at(TokenKind::RParen) && !p.at(TokenKind::Eof) {
            let checkpoint = p.checkpoint();
            if expr_until_tokens(p, &[TokenKind::Comma, TokenKind::RParen]).is_some() {
                if p.eat(TokenKind::Comma).is_none() {
                    if expr_can_start(p.kind()) {
                        p.expected_here(
                            ParseErrorKind::Grammar,
                            "expected `,` or `)` after call argument",
                        );
                        continue;
                    }
                    break;
                }
            } else if p.eat(TokenKind::Comma).is_none() {
                if p.at(TokenKind::RParen) || p.at(TokenKind::Eof) {
                    break;
                }
                recover_to_list_boundary(p, checkpoint, TokenKind::RParen);
            }
        }
        p.expect(TokenKind::RParen, "expected `)` after call")?;
        Done::new(SyntaxKind::ArgList, Span::new(start, p.previous_end()))
    })
}

fn bracket_suffix_after_open(p: &mut Parser<'_>) -> Option<()> {
    if p.at(TokenKind::DotDot) || p.at(TokenKind::DotDotEq) {
        p.node(|p| {
            let start = p.span().start;
            let inclusive = p.at(TokenKind::DotDotEq);
            p.bump();
            if inclusive || !p.at(TokenKind::RBracket) {
                binary_until(p, 0, &[TokenKind::RBracket])?;
            }
            Done::new(SyntaxKind::SliceRange, Span::new(start, p.previous_end()))
        })?;
        return Some(());
    }
    let checkpoint = p.checkpoint();
    let errors = p.errors_len();
    let first = binary_until(
        p,
        0,
        &[
            TokenKind::Comma,
            TokenKind::RBracket,
            TokenKind::DotDot,
            TokenKind::DotDotEq,
        ],
    );
    if p.at(TokenKind::DotDot) || p.at(TokenKind::DotDotEq) {
        let Some(first) = first else {
            p.recover_in_error(|p| {
                p.bump();
            });
            return None;
        };
        let start = first.span.start;
        p.node_preceding(&first, |p| {
            let inclusive = p.at(TokenKind::DotDotEq);
            p.bump();
            if inclusive || !p.at(TokenKind::RBracket) {
                binary_until(p, 0, &[TokenKind::RBracket])?;
            }
            Done::new(SyntaxKind::SliceRange, Span::new(start, p.previous_end()))
        })?;
        return Some(());
    }
    p.rewind(checkpoint);
    p.truncate_errors(errors);
    bracket_args(p);
    Some(())
}

fn bracket_arg_expr_skipped(kind: &TokenKind) -> bool {
    !matches!(
        kind,
        TokenKind::Integer
            | TokenKind::Float
            | TokenKind::String
            | TokenKind::Char
            | TokenKind::ByteChar
            | TokenKind::True
            | TokenKind::False
            | TokenKind::Null
            | TokenKind::Ident
            | TokenKind::Underscore
            | TokenKind::At
            | TokenKind::SelfValue
            | TokenKind::Pkg
            | TokenKind::Super
            | TokenKind::LBracket
            | TokenKind::LParen
            | TokenKind::LBrace
            | TokenKind::If
            | TokenKind::Not
            | TokenKind::Minus
            | TokenKind::Bang
            | TokenKind::Question
            | TokenKind::Amp
            | TokenKind::Star
    )
}

fn bracket_args(p: &mut Parser<'_>) {
    p.node(|p| {
        let start = p.span().start;
        while !p.at(TokenKind::RBracket) && !p.at(TokenKind::Eof) {
            // A failed argument already recovered through its separator.
            if bracket_arg(p).is_none() {
                continue;
            }
            if p.eat(TokenKind::Comma).is_none() {
                if expr_can_start(p.kind()) {
                    p.expected_here(
                        ParseErrorKind::Grammar,
                        "expected `,` or `]` after bracket argument",
                    );
                    continue;
                }
                break;
            }
        }
        Done::new(
            SyntaxKind::BracketArgList,
            Span::new(start, p.previous_end()),
        )
    });
}

/// One bracket argument whose type/expression reading is decided later.
///
/// The longest successful reading owns the tokens. When the expression owns
/// them and the type covers the same tokens, the type is kept as an
/// alternate.
fn bracket_arg(p: &mut Parser<'_>) -> Option<Parsed> {
    p.node(|p| {
        let start = p.span().start;
        let type_checkpoint = p.checkpoint();
        let type_errors = p.errors_len();
        let ty = types::type_(p);
        let type_events = ty.as_ref().map(|ty| p.snapshot(ty, type_checkpoint));
        let type_end = ty.as_ref().map(|ty| ty.span.end);
        p.rewind(type_checkpoint);
        p.truncate_errors(type_errors);

        let expr_errors = p.errors_len();
        let mut expr = if type_end.is_some() && bracket_arg_expr_skipped(p.kind()) {
            None
        } else {
            expr_until_tokens(p, &[TokenKind::Comma, TokenKind::RBracket])
        };
        let expr_end = expr.as_ref().map(|expr| expr.span.end);
        if expr.is_none() || expr_end.is_some_and(|expr_end| Some(expr_end) < type_end) {
            p.rewind(type_checkpoint);
            p.truncate_errors(expr_errors);
            expr = None;
        }
        let end = match (type_end, expr.as_ref().map(|expr| expr.span.end)) {
            (Some(type_end), Some(expr_end)) => type_end.max(expr_end),
            (Some(type_end), None) => type_end,
            (None, Some(expr_end)) => expr_end,
            (None, None) => {
                p.error_here("expected bracket argument");
                // Keep commas inside a discarded nested expression local to
                // that expression so later top-level arguments survive.
                recover_to_list_boundary(p, type_checkpoint, TokenKind::RBracket);
                return None;
            }
        };
        let span = Span::new(start, end);
        if expr.is_some() {
            if type_end == Some(end)
                && let Some(events) = type_events
            {
                p.push_alternate(AlternateKind::BracketType, span, events);
            }
        } else {
            let errors = p.errors_len();
            types::type_(p);
            p.truncate_errors(errors);
        }
        Done::new(SyntaxKind::BracketArg, span)
    })
}

fn primary_until(p: &mut Parser<'_>, stops: &[TokenKind]) -> Option<Parsed> {
    if let Some(expr) = omitted_constructor(p) {
        if expr.class == SyntaxKind::OmittedMemberExpr
            && p.at(TokenKind::LBrace)
            && !stops.contains(&TokenKind::LBrace)
        {
            return p.node_preceding(&expr, |p| {
                field_init_list(
                    p,
                    "expected `{` before omitted named payload",
                    "expected `}` after omitted named payload",
                )?;
                Done::new(
                    SyntaxKind::QualifiedStructLiteral,
                    Span::new(expr.span.start, p.previous_end()),
                )
            });
        }
        return Some(expr);
    }
    if !stops.contains(&TokenKind::LBrace)
        && let Some(expr) = qualified_struct_literal(p)
    {
        return Some(expr);
    }
    if let Some(expr) = typed_aggregate_literal(p, stops) {
        return Some(expr);
    }
    let start = p.span().start;
    match p.kind() {
        TokenKind::Integer
        | TokenKind::Float
        | TokenKind::Char
        | TokenKind::ByteChar
        | TokenKind::True
        | TokenKind::False
        | TokenKind::Null => p.node(|p| {
            let span = p.bump();
            Done::new(SyntaxKind::LiteralExpr, span)
        }),
        TokenKind::String | TokenKind::ByteString => p.node(|p| {
            let kind = p.kind().clone();
            while p.at(kind.clone()) {
                p.bump();
            }
            if matches!(p.kind(), TokenKind::String | TokenKind::ByteString) {
                p.error_here("adjacent string literals must use the same literal prefix");
            }
            Done::new(SyntaxKind::StringExpr, Span::new(start, p.previous_end()))
        }),
        TokenKind::Ident => p.node(|p| {
            let span = p.bump();
            Done::shaped(SyntaxKind::NameExpr, span, Shape::VALUE_PATH)
        }),
        TokenKind::SelfValue | TokenKind::Pkg | TokenKind::Super | TokenKind::Underscore => {
            p.node(|p| {
                let span = p.bump();
                Done::new(SyntaxKind::NameExpr, span)
            })
        }
        TokenKind::Backslash => closure(p),
        TokenKind::LBracket => bracket_primary(p),
        TokenKind::LParen => tuple_or_paren(p),
        TokenKind::LBrace => stmts::block(p),
        TokenKind::If => if_expr(p),
        TokenKind::Match => stmts::match_expr(p),
        _ => {
            p.error_here_as(ParseErrorKind::ExpectedExpression, "expected expression");
            p.missing();
            None
        }
    }
}

fn tuple_or_paren(p: &mut Parser<'_>) -> Option<Parsed> {
    p.node(|p| {
        let start = p.bump().start;
        if p.eat(TokenKind::RParen).is_some() {
            return Done::new(SyntaxKind::TupleExpr, Span::new(start, p.previous_end()));
        }
        let first_checkpoint = p.checkpoint();
        let first = expr_until_tokens(p, &[TokenKind::Comma, TokenKind::RParen]);
        if first.is_none() {
            recover_to_list_boundary(p, first_checkpoint, TokenKind::RParen);
        }
        if let Some(first) = first
            && p.eat(TokenKind::Comma).is_none()
        {
            if expr_can_start(p.kind()) {
                p.expected_here(
                    ParseErrorKind::Grammar,
                    "expected `,` or `)` after tuple element",
                );
            } else {
                p.expect(TokenKind::RParen, "expected `)`")?;
                return Done::wrapping(SyntaxKind::ParenExpr, &first, first.span);
            }
        }
        while !p.at(TokenKind::RParen) && !p.at(TokenKind::Eof) {
            let checkpoint = p.checkpoint();
            if expr_until_tokens(p, &[TokenKind::Comma, TokenKind::RParen]).is_some() {
                if p.eat(TokenKind::Comma).is_none() {
                    break;
                }
            } else {
                recover_to_list_boundary(p, checkpoint, TokenKind::RParen);
            }
        }
        let end = p.expect(TokenKind::RParen, "expected `)` after tuple")?.end;
        Done::new(SyntaxKind::TupleExpr, Span::new(start, end))
    })
}

/// `.{ fields }` or `.name`; rewinds when `.` is not followed by either.
pub(crate) fn omitted_constructor(p: &mut Parser<'_>) -> Option<Parsed> {
    if !p.at(TokenKind::Dot) {
        return None;
    }
    let checkpoint = p.checkpoint();
    let errors = p.errors_len();
    let mut not_constructor = false;
    let parsed = p.node(|p| {
        let start = p.bump().start;
        if p.at(TokenKind::LBrace) {
            field_init_list(
                p,
                "expected `{` before omitted aggregate literal",
                "expected `}` after omitted aggregate literal",
            )?;
            return Done::new(
                SyntaxKind::OmittedAggregateLiteral,
                Span::new(start, p.previous_end()),
            );
        }
        if p.expect_name("expected name after omitted `.`").is_none() {
            not_constructor = true;
            return None;
        }
        Done::new(
            SyntaxKind::OmittedMemberExpr,
            Span::new(start, p.previous_end()),
        )
    });
    if not_constructor {
        p.rewind(checkpoint);
        p.truncate_errors(errors);
        return None;
    }
    parsed
}

fn qualified_struct_literal(p: &mut Parser<'_>) -> Option<Parsed> {
    let checkpoint = p.checkpoint();
    let errors = p.errors_len();
    let target = qualified_value_path(p);
    let Some(target) = target.filter(|_| p.at(TokenKind::LBrace)) else {
        p.rewind(checkpoint);
        p.truncate_errors(errors);
        return None;
    };
    p.node_preceding(&target, |p| {
        field_init_list(
            p,
            "expected `{` before qualified literal",
            "expected `}` after qualified literal",
        )?;
        Done::new(
            SyntaxKind::QualifiedStructLiteral,
            Span::new(target.span.start, p.previous_end()),
        )
    })
}

/// `name::name...` value path with at least one `::`.
///
/// Returns `None` after consuming input when the path is unqualified or
/// malformed; callers rewind.
pub(crate) fn qualified_value_path(p: &mut Parser<'_>) -> Option<Parsed> {
    if !matches!(
        p.kind(),
        TokenKind::Ident | TokenKind::Pkg | TokenKind::Super
    ) {
        return None;
    }
    let is_name = p.at(TokenKind::Ident);
    let mut expr = p.node(|p| {
        let span = p.bump();
        let shape = if is_name {
            Shape::VALUE_PATH
        } else {
            Shape::default()
        };
        Done::shaped(SyntaxKind::NameExpr, span, shape)
    })?;
    let mut qualified = false;
    while p.at(TokenKind::ColonColon) {
        let lhs = expr.clone();
        expr = p.node_preceding(&lhs, |p| {
            p.bump();
            p.expect_name("expected name after `::`")?;
            Done::shaped(
                SyntaxKind::QualifiedExpr,
                Span::new(lhs.span.start, p.previous_end()),
                lhs.shape,
            )
        })?;
        qualified = true;
    }
    qualified.then_some(expr)
}

fn typed_aggregate_literal(p: &mut Parser<'_>, stops: &[TokenKind]) -> Option<Parsed> {
    if !types::type_can_start(p.kind()) {
        return None;
    }
    let checkpoint = p.checkpoint();
    let errors = p.errors_len();
    let start = p.span().start;
    let ty = types::type_before_aggregate_literal(p);
    let Some(ty) = ty.filter(|_| p.at(TokenKind::LBrace) && !p.at_any(stops)) else {
        p.rewind(checkpoint);
        p.truncate_errors(errors);
        return None;
    };
    p.node_preceding(&ty, |p| {
        field_init_list(
            p,
            "expected `{` before struct literal",
            "expected `}` after struct literal",
        )?;
        Done::new(
            SyntaxKind::TypedStructLiteral,
            Span::new(start, p.previous_end()),
        )
    })
}

fn field_init_list(p: &mut Parser<'_>, open_message: &str, close_message: &str) -> Option<Parsed> {
    p.node(|p| {
        let start = p.expect(TokenKind::LBrace, open_message)?.start;
        while !p.at(TokenKind::RBrace) && !p.at(TokenKind::Eof) {
            let mut failed = false;
            p.node(|p| {
                let start = p.span().start;
                if p.expect_name("expected field name").is_none() {
                    failed = true;
                    return None;
                }
                if p.eat(TokenKind::Colon).is_some() && expr(p).is_none() {
                    failed = true;
                    return None;
                }
                Done::new(SyntaxKind::FieldInit, Span::new(start, p.previous_end()))
            });
            if failed {
                if p.eat(TokenKind::Comma).is_some() {
                    continue;
                }
                recover_to_member_boundary(p, None);
                continue;
            }
            if p.eat(TokenKind::Comma).is_none() {
                if p.at(TokenKind::Ident) {
                    p.expected_here(
                        ParseErrorKind::Grammar,
                        "expected `,` or `}` after struct field",
                    );
                    continue;
                }
                break;
            }
        }
        p.expect(TokenKind::RBrace, close_message)?;
        Done::new(
            SyntaxKind::FieldInitList,
            Span::new(start, p.previous_end()),
        )
    })
}

fn is_clause(p: &mut Parser<'_>, target: &Parsed) -> Option<Parsed> {
    p.node_preceding(target, |p| {
        p.bump();
        if target.class == SyntaxKind::NotExpr {
            p.error_at(
                target.span,
                "`not value is pattern` is not a valid pattern condition",
            );
            return None;
        }
        let pattern =
            patterns::pattern_until(p, &[TokenKind::And, TokenKind::Or, TokenKind::LBrace])?;
        Done::new(
            SyntaxKind::IsClause,
            Span::new(target.span.start, pattern.span.end),
        )
    })
}

const OR_MESSAGE: &str = "pattern conditions do not support `or`; use `match` for alternatives";

fn if_expr(p: &mut Parser<'_>) -> Option<Parsed> {
    p.node(|p| {
        let start = p.bump().start;
        let condition = p.checkpoint();
        let errors = p.errors_len();
        let target = expr_until_tokens(p, &[TokenKind::Is, TokenKind::And, TokenKind::LBrace])?;
        if p.at(TokenKind::Is) {
            is_clause(p, &target)?;
            return if_chain_tail(p, start);
        }
        if p.at(TokenKind::And) {
            match condition_first_chain(p, start)? {
                Some(done) => return Some(done),
                None => {
                    p.rewind(condition);
                    p.truncate_errors(errors);
                    expr_until_tokens(p, &[TokenKind::Is, TokenKind::LBrace])?;
                    if p.at(TokenKind::Is) {
                        p.error_here(OR_MESSAGE);
                        return None;
                    }
                }
            }
        }
        if_branches(p, start)
    })
}

/// Parses `and` clauses after a leading plain condition.
///
/// Returns `Some(None)` when no pattern clause follows, so the caller
/// reparses the condition as one boolean expression.
fn condition_first_chain(p: &mut Parser<'_>, start: usize) -> Option<Option<Done>> {
    while p.eat(TokenKind::And).is_some() {
        if p.at(TokenKind::Or) {
            return Some(None);
        }
        let target = expr_until_tokens(
            p,
            &[
                TokenKind::Is,
                TokenKind::And,
                TokenKind::Or,
                TokenKind::LBrace,
            ],
        )?;
        if p.at(TokenKind::Is) {
            is_clause(p, &target)?;
            return if_chain_tail(p, start).map(Some);
        }
    }
    Some(None)
}

fn if_chain_tail(p: &mut Parser<'_>, start: usize) -> Option<Done> {
    while p.eat(TokenKind::And).is_some() {
        if p.at(TokenKind::Or) {
            p.error_here(OR_MESSAGE);
            return None;
        }
        let target = expr_until_tokens(
            p,
            &[
                TokenKind::Is,
                TokenKind::And,
                TokenKind::Or,
                TokenKind::LBrace,
            ],
        )?;
        if p.at(TokenKind::Is) {
            is_clause(p, &target)?;
        }
    }
    if p.at(TokenKind::Or) {
        p.error_here(OR_MESSAGE);
        return None;
    }
    if_branches(p, start)
}

fn if_branches(p: &mut Parser<'_>, start: usize) -> Option<Done> {
    let mut end = stmts::block(p)?.span.end;
    if p.eat(TokenKind::Else).is_some() {
        end = if p.at(TokenKind::If) {
            if_expr(p)?.span.end
        } else {
            stmts::block(p)?.span.end
        };
    }
    Done::new(SyntaxKind::IfExpr, Span::new(start, end))
}

fn bracket_primary(p: &mut Parser<'_>) -> Option<Parsed> {
    let checkpoint = p.checkpoint();
    let errors = p.errors_len();
    let trait_target = p.node(|p| {
        let start = p.bump().start;
        if !types::type_can_start(p.kind()) {
            return None;
        }
        types::type_(p)?;
        p.expect(TokenKind::As, "expected `as` in trait target")?;
        types::type_(p)?;
        p.expect(TokenKind::RBracket, "expected `]` after trait target")?;
        if !p.at(TokenKind::ColonColon) {
            return None;
        }
        Done::new(
            SyntaxKind::TraitTargetExpr,
            Span::new(start, p.previous_end()),
        )
    });
    if trait_target.is_some() {
        return trait_target;
    }
    p.rewind(checkpoint);
    p.truncate_errors(errors);
    let type_target = p.node(|p| {
        let start = p.bump().start;
        if !types::type_can_start(p.kind()) {
            let span = p.span();
            p.error_at(span, "expected type target");
            return None;
        }
        types::type_(p)?;
        p.expect(TokenKind::RBracket, "expected `]` after type target")?;
        if !p.at(TokenKind::ColonColon) {
            return None;
        }
        Done::new(
            SyntaxKind::TypeTargetExpr,
            Span::new(start, p.previous_end()),
        )
    });
    if type_target.is_some() {
        return type_target;
    }
    p.rewind(checkpoint);
    p.truncate_errors(errors);
    array_literal(p)
}

fn array_literal(p: &mut Parser<'_>) -> Option<Parsed> {
    p.node(|p| {
        let start = p.bump().start;
        array_elements(p)?;
        let end = p
            .expect(TokenKind::RBracket, "expected `]` after array literal")?
            .end;
        Done::new(SyntaxKind::ArrayExpr, Span::new(start, end))
    })
}

fn array_elements(p: &mut Parser<'_>) -> Option<()> {
    if p.at(TokenKind::RBracket) {
        return Some(());
    }
    let first_checkpoint = p.checkpoint();
    let first = expr(p);
    if p.at(TokenKind::Semicolon) {
        p.bump();
        first?;
        expr(p)?;
        return Some(());
    }
    let had_first = first.is_some();
    if !had_first {
        recover_to_list_boundary(p, first_checkpoint, TokenKind::RBracket);
    }
    if had_first && p.eat(TokenKind::Comma).is_none() {
        if !expr_can_start(p.kind()) {
            return Some(());
        }
        p.expected_here(
            ParseErrorKind::Grammar,
            "expected `,` or `]` after array element",
        );
    }
    while !p.at(TokenKind::RBracket) && !p.at(TokenKind::Eof) {
        let checkpoint = p.checkpoint();
        if expr(p).is_some() {
            if p.eat(TokenKind::Comma).is_none() {
                if expr_can_start(p.kind()) {
                    p.expected_here(
                        ParseErrorKind::Grammar,
                        "expected `,` or `]` after array element",
                    );
                    continue;
                }
                break;
            }
        } else {
            recover_to_list_boundary(p, checkpoint, TokenKind::RBracket);
        }
    }
    Some(())
}

fn closure(p: &mut Parser<'_>) -> Option<Parsed> {
    p.node(|p| {
        let start = p.bump().start;
        if p.at(TokenKind::LBracket) {
            capture_list(p)?;
        }
        closure_params(p);
        p.expect(
            TokenKind::ThinArrow,
            "expected `->` after closure parameters",
        )?;
        let body = assignment_until(p, &[])?;
        Done::new(SyntaxKind::ClosureExpr, Span::new(start, body.span.end))
    })
}

fn capture_list(p: &mut Parser<'_>) -> Option<Parsed> {
    p.node(|p| {
        let open = p.bump();
        let mut captures = 0usize;
        while !p.at(TokenKind::RBracket) && !p.at(TokenKind::Eof) {
            let checkpoint = p.checkpoint();
            let capture = p.node(|p| {
                let start = p.span().start;
                if p.eat(TokenKind::Amp).is_some() {
                    p.eat(TokenKind::Mut);
                }
                p.expect_name("expected capture name")?;
                Done::new(SyntaxKind::Capture, Span::new(start, p.previous_end()))
            });
            if capture.is_none() {
                recover_to_list_boundary(p, checkpoint, TokenKind::RBracket);
                continue;
            }
            captures += 1;
            if p.eat(TokenKind::Comma).is_none() {
                if p.at(TokenKind::RBracket) || p.at(TokenKind::Eof) {
                    break;
                }
                if p.at(TokenKind::Ident) || p.at(TokenKind::Amp) {
                    p.expected_here(
                        ParseErrorKind::Grammar,
                        "expected `,` or `]` after closure capture",
                    );
                    continue;
                }
                break;
            }
        }
        let close = p.expect(TokenKind::RBracket, "expected `]` after closure captures")?;
        if captures == 0 {
            p.error_at(
                Span::new(open.start, close.end),
                "empty closure capture list must be omitted",
            );
        }
        Done::new(SyntaxKind::CaptureList, Span::new(open.start, close.end))
    })
}

fn closure_params(p: &mut Parser<'_>) {
    p.node(|p| {
        let start = p.span().start;
        while !p.at(TokenKind::ThinArrow) && !p.at(TokenKind::Eof) {
            if !p.at(TokenKind::Ident) {
                p.expected_here(
                    ParseErrorKind::ExpectedName,
                    "expected closure parameter name",
                );
                // Keep a missing parameter local to this comma-delimited
                // entry so later parameters and the closure body survive.
                skip_until(
                    p,
                    &[TokenKind::Comma, TokenKind::ThinArrow, TokenKind::RBrace],
                );
                if p.eat(TokenKind::Comma).is_some() {
                    continue;
                }
                break;
            }
            p.node(|p| {
                let start = p.bump().start;
                let mut end = p.previous_end();
                if p.eat(TokenKind::Colon).is_some()
                    && let Some(ty) = types::closure_param_type(p)
                {
                    end = ty.span.end;
                }
                Done::new(SyntaxKind::Param, Span::new(start, end))
            });
            if p.eat(TokenKind::Comma).is_none() {
                if p.at(TokenKind::ThinArrow) || p.at(TokenKind::Eof) {
                    break;
                }
                let next = p.at(TokenKind::Ident);
                p.expected_here(
                    ParseErrorKind::Grammar,
                    "expected `,` or `->` after closure parameter",
                );
                if next {
                    continue;
                }
                skip_until(p, &[TokenKind::ThinArrow]);
                break;
            }
        }
        Done::new(
            SyntaxKind::ClosureParamList,
            Span::new(start, p.previous_end()),
        )
    });
}

// ---- conditional-compilation expressions -----------------------------------

pub(crate) fn condition_until(p: &mut Parser<'_>, stops: &[TokenKind]) -> Option<Parsed> {
    condition_binary(p, stops, 0)
}

fn condition_level(kind: &TokenKind) -> Option<u8> {
    match kind {
        TokenKind::Or => Some(0),
        TokenKind::And => Some(1),
        TokenKind::EqEq | TokenKind::BangEq => Some(2),
        _ => None,
    }
}

fn condition_binary(p: &mut Parser<'_>, stops: &[TokenKind], level: u8) -> Option<Parsed> {
    if level > 2 {
        return condition_unary(p, stops);
    }
    let mut lhs = condition_binary(p, stops, level + 1)?;
    while !p.at_any(stops) && condition_level(p.kind()) == Some(level) {
        let left = lhs.clone();
        lhs = p.node_preceding(&left, |p| {
            p.bump();
            let rhs = condition_binary(p, stops, level + 1)?;
            Done::new(
                SyntaxKind::ConditionBinary,
                Span::new(left.span.start, rhs.span.end),
            )
        })?;
    }
    Some(lhs)
}

fn condition_unary(p: &mut Parser<'_>, stops: &[TokenKind]) -> Option<Parsed> {
    if p.at(TokenKind::Not) {
        return p.node(|p| {
            let start = p.bump().start;
            let expr = condition_unary(p, stops)?;
            Done::new(SyntaxKind::ConditionNot, Span::new(start, expr.span.end))
        });
    }
    if p.at_any(stops) {
        p.error_here_as(
            ParseErrorKind::ExpectedExpression,
            "expected condition expression",
        );
        p.missing();
        return None;
    }
    match p.kind() {
        TokenKind::True
        | TokenKind::False
        | TokenKind::Integer
        | TokenKind::String
        | TokenKind::Ident => p.node(|p| {
            let span = p.bump();
            Done::new(SyntaxKind::ConditionAtom, span)
        }),
        TokenKind::LParen => p.node(|p| {
            let start = p.bump().start;
            let inner = condition_until(p, &[TokenKind::RParen])?;
            let end = p
                .expect(TokenKind::RParen, "expected `)` after condition expression")?
                .end;
            Done::wrapping(SyntaxKind::ConditionParen, &inner, Span::new(start, end))
        }),
        _ => {
            p.error_here_as(
                ParseErrorKind::ExpectedExpression,
                "expected condition expression",
            );
            p.missing();
            None
        }
    }
}
