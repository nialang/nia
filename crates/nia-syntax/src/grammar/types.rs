// SPDX-License-Identifier: GPL-3.0-or-later
//! Type, type-argument, generic-parameter, and `where` clause grammar.

use nia_lexer::TokenKind;
use nia_span::Span;

use super::parser::{Done, Parsed, Parser, Shape};
use super::{Depth, collect_until, exprs, recover_to_list_boundary, skip_until};
use crate::{AlternateKind, ParseErrorKind, SyntaxKind};

/// Whether a type is parsed where `Type {` begins a struct literal.
#[derive(Clone, Copy, PartialEq, Eq)]
pub(crate) enum TypeMode {
    Normal,
    BeforeAggregateLiteral,
}

pub(crate) fn type_can_start(kind: &TokenKind) -> bool {
    matches!(
        kind,
        TokenKind::Amp
            | TokenKind::LBracket
            | TokenKind::Ident
            | TokenKind::Pkg
            | TokenKind::Super
            | TokenKind::SelfValue
            | TokenKind::DotDot
            | TokenKind::DotDotEq
            | TokenKind::LParen
            | TokenKind::Bool
            | TokenKind::SelfType
            | TokenKind::Opaque
            | TokenKind::Never
            | TokenKind::Question
            | TokenKind::Caret
            | TokenKind::Underscore
    )
}

/// `[T, N: usize]`; returns the number of accepted parameters.
pub(crate) fn generic_params(p: &mut Parser<'_>) -> usize {
    if !p.at(TokenKind::LBracket) {
        return 0;
    }
    let mut count = 0usize;
    p.node(|p| {
        let start = p.bump().start;
        while !p.at(TokenKind::RBracket) && !p.at(TokenKind::Eof) {
            if p.at(TokenKind::Ident) {
                if generic_param(p).is_some() {
                    count += 1;
                }
            } else {
                p.error_here_as(ParseErrorKind::ExpectedName, "expected generic parameter");
                // A missing parameter name is local to this comma-delimited
                // entry. Keep later parameters and the enclosing item
                // recoverable instead of treating the whole list as lost.
                skip_until(p, &[TokenKind::Comma, TokenKind::RBracket]);
                if p.eat(TokenKind::Comma).is_some() {
                    continue;
                }
                break;
            }
            if p.eat(TokenKind::Comma).is_none() {
                if p.at(TokenKind::Ident) {
                    // An identifier is an unambiguous start of the next type
                    // or const parameter, so a missing separator stays local.
                    p.expected_here(
                        ParseErrorKind::Grammar,
                        "expected `,` or `]` after generic parameter",
                    );
                    continue;
                }
                break;
            }
        }
        p.expect(TokenKind::RBracket, "expected `]` after generic parameters");
        Done::new(
            SyntaxKind::GenericParamList,
            Span::new(start, p.previous_end()),
        )
    });
    count
}

fn generic_param(p: &mut Parser<'_>) -> Option<Parsed> {
    p.node(|p| {
        let name = p.bump();
        if p.eat(TokenKind::Colon).is_some() {
            let checkpoint = p.checkpoint();
            let errors = p.errors_len();
            if type_(p).is_none() {
                if p.errors_len() == errors {
                    p.error_here_as(ParseErrorKind::ExpectedType, "expected type");
                }
                if !p.at(TokenKind::Comma) && !p.at(TokenKind::RBracket) && !p.at(TokenKind::Eof) {
                    recover_to_list_boundary(p, checkpoint, TokenKind::RBracket);
                }
                return None;
            }
        }
        Done::new(SyntaxKind::GenericParam, name)
    })
}

/// Parses a type that must end at one of `stops`, or an error type.
pub(crate) fn type_until(p: &mut Parser<'_>, stops: &[TokenKind]) -> Option<Parsed> {
    let checkpoint = p.checkpoint();
    let errors = p.errors_len();
    let bare_fn_type = p.at(TokenKind::Fn);
    if let Some(ty) = type_(p)
        && p.at_any(stops)
    {
        return Some(ty);
    }
    p.rewind(checkpoint);
    p.truncate_errors(errors);
    if p.at_any(stops) {
        p.expected_here(ParseErrorKind::ExpectedType, "expected type");
        return None;
    }
    p.node(|p| {
        let span = collect_until(p, stops)?;
        if bare_fn_type {
            p.error_at(span, "function pointer types must be written as `&fn(...)`");
        } else {
            p.error_at(span, "invalid type syntax");
        }
        Done::new(SyntaxKind::ErrorType, span)
    })
}

/// Speculatively parses a type that ends at an accepted follow token.
fn type_followed_by(
    p: &mut Parser<'_>,
    follows: impl FnOnce(&Parser<'_>) -> bool,
    stops: &[TokenKind],
) -> Option<Parsed> {
    let checkpoint = p.checkpoint();
    let errors = p.errors_len();
    if let Some(ty) = type_(p)
        && follows(p)
    {
        return Some(ty);
    }
    p.rewind(checkpoint);
    p.truncate_errors(errors);
    type_until(p, stops)
}

fn at_ident_colon(p: &Parser<'_>) -> bool {
    p.at(TokenKind::Ident) && matches!(p.nth_kind(1), Some(TokenKind::Colon))
}

pub(crate) fn field_type(p: &mut Parser<'_>) -> Option<Parsed> {
    type_followed_by(
        p,
        |p| p.at(TokenKind::Comma) || p.at(TokenKind::RBrace) || at_ident_colon(p),
        &[TokenKind::Comma, TokenKind::RBrace],
    )
}

pub(crate) fn param_type(p: &mut Parser<'_>) -> Option<Parsed> {
    type_followed_by(
        p,
        |p| {
            p.at(TokenKind::Comma)
                || p.at(TokenKind::RParen)
                || p.at(TokenKind::LBrace)
                || at_ident_colon(p)
        },
        &[TokenKind::Comma, TokenKind::RParen, TokenKind::LBrace],
    )
}

pub(crate) fn closure_param_type(p: &mut Parser<'_>) -> Option<Parsed> {
    type_followed_by(
        p,
        |p| p.at(TokenKind::Comma) || p.at(TokenKind::ThinArrow) || at_ident_colon(p),
        &[TokenKind::Comma, TokenKind::ThinArrow],
    )
}

pub(crate) fn type_list_element(p: &mut Parser<'_>, closing: TokenKind) -> Option<Parsed> {
    let close = closing.clone();
    type_followed_by(
        p,
        move |p| p.at(TokenKind::Comma) || p.at(close) || type_can_start(p.kind()),
        &[TokenKind::Comma, closing],
    )
}

pub(crate) fn type_(p: &mut Parser<'_>) -> Option<Parsed> {
    type_with_mode(p, TypeMode::Normal)
}

pub(crate) fn type_before_aggregate_literal(p: &mut Parser<'_>) -> Option<Parsed> {
    type_with_mode(p, TypeMode::BeforeAggregateLiteral)
}

fn type_with_mode(p: &mut Parser<'_>, mode: TypeMode) -> Option<Parsed> {
    let error = optional_type(p, mode)?;
    if p.at(TokenKind::Bang) {
        return p.node_preceding(&error, |p| {
            p.bump();
            let value = type_with_mode(p, mode)?;
            Done::new(
                SyntaxKind::ErrorUnionType,
                Span::new(error.span.start, value.span.end),
            )
        });
    }
    Some(error)
}

fn optional_type(p: &mut Parser<'_>, mode: TypeMode) -> Option<Parsed> {
    if !p.at(TokenKind::Question) {
        return range_type(p, mode);
    }
    p.node(|p| {
        let start = p.bump().start;
        let elem = optional_type(p, mode)?;
        Done::new(SyntaxKind::OptionalType, Span::new(start, elem.span.end))
    })
}

fn range_type(p: &mut Parser<'_>, mode: TypeMode) -> Option<Parsed> {
    let start = p.span().start;
    if p.at(TokenKind::DotDot) || p.at(TokenKind::DotDotEq) {
        return p.node(|p| {
            range_type_tail(p, mode)?;
            Done::new(SyntaxKind::RangeType, Span::new(start, p.previous_end()))
        });
    }
    let bound = type_atom(p, mode)?;
    if p.at(TokenKind::DotDot) || p.at(TokenKind::DotDotEq) {
        return p.node_preceding(&bound, |p| {
            range_type_tail(p, mode)?;
            Done::new(SyntaxKind::RangeType, Span::new(start, p.previous_end()))
        });
    }
    Some(bound)
}

fn range_type_tail(p: &mut Parser<'_>, mode: TypeMode) -> Option<()> {
    if p.eat(TokenKind::DotDotEq).is_some() {
        type_with_mode(p, mode)?;
    } else {
        p.bump();
        if type_can_start(p.kind()) {
            type_with_mode(p, mode)?;
        }
    }
    Some(())
}

fn type_atom(p: &mut Parser<'_>, mode: TypeMode) -> Option<Parsed> {
    p.node(|p| {
        let start = p.span().start;
        let done = |p: &Parser<'_>, kind| Done::new(kind, Span::new(start, p.previous_end()));
        if p.eat(TokenKind::Amp).is_some() {
            let kind = type_after_amp(p, mode)?;
            return done(p, kind);
        }
        if p.eat(TokenKind::Caret).is_some() {
            p.eat(TokenKind::Mut);
            type_with_mode(p, mode)?;
            return done(p, SyntaxKind::VolatilePointerType);
        }
        if p.eat(TokenKind::LBracket).is_some() {
            if projection_after_open(p) {
                return done(p, SyntaxKind::ProjectionType);
            }
            type_with_mode(p, mode)?;
            if p.eat(TokenKind::Semicolon).is_some() {
                if p.eat(TokenKind::Underscore).is_none() {
                    exprs::expr(p)?;
                }
                p.expect(TokenKind::RBracket, "expected `]` after array type")?;
                return done(p, SyntaxKind::ArrayType);
            }
            p.expect(TokenKind::RBracket, "expected `;` or `]` in bracket type")?;
            return done(p, SyntaxKind::SlicePointeeType);
        }
        if p.eat(TokenKind::LParen).is_some() {
            return match paren_type(p, mode)? {
                Some(inner) => Done::wrapping(
                    SyntaxKind::ParenType,
                    &inner,
                    Span::new(start, p.previous_end()),
                ),
                None => done(p, SyntaxKind::TupleType),
            };
        }
        if at_callable_type(p) {
            callable_type(p, mode)?;
            return done(p, SyntaxKind::CallableType);
        }
        if p.eat(TokenKind::Underscore).is_some() {
            return done(p, SyntaxKind::InferType);
        }
        if p.at(TokenKind::Fn) {
            p.error_here("function pointer types must be written as `&fn(...)`");
            return None;
        }
        if p.eat(TokenKind::SelfType).is_some() {
            return done(p, SyntaxKind::SelfType);
        }
        if p.eat(TokenKind::Opaque).is_some() {
            return done(p, SyntaxKind::OpaqueType);
        }
        if p.eat(TokenKind::Never).is_some() {
            return done(p, SyntaxKind::NeverType);
        }
        if matches!(
            p.kind(),
            TokenKind::Ident
                | TokenKind::Bool
                | TokenKind::Pkg
                | TokenKind::Super
                | TokenKind::SelfValue
        ) && (!p.at(TokenKind::SelfValue) || p.nth_kind(1) == Some(&TokenKind::ColonColon))
        {
            let shape = path_segments(p, mode)?;
            return Done::shaped(
                SyntaxKind::PathType,
                Span::new(start, p.previous_end()),
                shape,
            );
        }
        p.error_here_as(ParseErrorKind::ExpectedType, "expected type");
        p.missing();
        None
    })
}

fn type_after_amp(p: &mut Parser<'_>, mode: TypeMode) -> Option<SyntaxKind> {
    let is_readonly = p.eat(TokenKind::Mut).is_none();
    if p.at(TokenKind::Fn) {
        if !is_readonly {
            p.error_here("function pointer types must be written as `&fn(...)`");
        }
        p.bump();
        p.expect(TokenKind::LParen, "expected `(` in function pointer type")?;
        while !p.at(TokenKind::RParen) && !p.at(TokenKind::Eof) {
            if p.eat(TokenKind::Ellipsis).is_some() {
                break;
            }
            let checkpoint = p.checkpoint();
            if type_with_mode(p, mode).is_some() {
                if !type_parameter_delimiter(p) {
                    break;
                }
            } else {
                recover_to_list_boundary(p, checkpoint, TokenKind::RParen);
            }
        }
        p.expect(TokenKind::RParen, "expected `)` in function pointer type")?;
        if type_can_start(p.kind()) {
            type_with_mode(p, mode)?;
        }
        return Some(SyntaxKind::FnPointerType);
    }
    let checkpoint = p.checkpoint();
    let errors = p.errors_len();
    if p.eat(TokenKind::LBracket).is_some()
        && type_with_mode(p, mode).is_some()
        && p.eat(TokenKind::RBracket).is_some()
        && !type_can_start(p.kind())
    {
        return Some(SyntaxKind::SliceType);
    }
    p.rewind(checkpoint);
    p.truncate_errors(errors);
    type_with_mode(p, mode)?;
    Some(SyntaxKind::PointerType)
}

/// Parses the contents after `(`; returns the inner type of a parenthesized
/// type, or `None` for a tuple type.
fn paren_type(p: &mut Parser<'_>, mode: TypeMode) -> Option<Option<Parsed>> {
    if p.eat(TokenKind::RParen).is_some() {
        return Some(None);
    }
    let first_checkpoint = p.checkpoint();
    let first = type_with_mode(p, mode);
    let first_failed = first.is_none();
    if first_failed && !p.at(TokenKind::RParen) {
        recover_to_list_boundary(p, first_checkpoint, TokenKind::RParen);
    }
    if first_failed && p.at(TokenKind::RParen) {
        p.expect(TokenKind::RParen, "expected `)` after tuple type")?;
        return Some(None);
    }
    if let Some(first) = first
        && p.eat(TokenKind::Comma).is_none()
    {
        if !type_can_start(p.kind()) {
            p.expect(TokenKind::RParen, "expected `)` after parenthesized type")?;
            return Some(Some(first));
        }
        p.expected_here(
            ParseErrorKind::Grammar,
            "expected `,` or `)` after type parameter",
        );
    }
    while !p.at(TokenKind::RParen) && !p.at(TokenKind::Eof) {
        let checkpoint = p.checkpoint();
        if type_with_mode(p, mode).is_some() {
            if p.eat(TokenKind::Comma).is_none() {
                if p.at(TokenKind::RParen) || p.at(TokenKind::Eof) {
                    break;
                } else if type_can_start(p.kind()) {
                    p.expected_here(
                        ParseErrorKind::Grammar,
                        "expected `,` or `)` after type parameter",
                    );
                    continue;
                }
                recover_to_list_boundary(p, checkpoint, TokenKind::RParen);
            }
        } else if p.eat(TokenKind::Comma).is_none() {
            if p.at(TokenKind::RParen) || p.at(TokenKind::Eof) {
                break;
            }
            recover_to_list_boundary(p, checkpoint, TokenKind::RParen);
        }
    }
    p.expect(TokenKind::RParen, "expected `)` after tuple type")?;
    Some(None)
}

fn projection_after_open(p: &mut Parser<'_>) -> bool {
    let checkpoint = p.checkpoint();
    let errors = p.errors_len();
    let parsed = type_(p).is_some()
        && p.eat(TokenKind::As).is_some()
        && type_(p).is_some()
        && p.expect(TokenKind::RBracket, "expected `]` after projection trait")
            .is_some()
        && p.expect(TokenKind::ColonColon, "expected `::` after projection type")
            .is_some()
        && p.expect_name("expected associated type name").is_some();
    if !parsed {
        p.rewind(checkpoint);
        p.truncate_errors(errors);
    }
    parsed
}

fn at_callable_type(p: &Parser<'_>) -> bool {
    p.at(TokenKind::Ident) && p.text() == "Fn" && matches!(p.nth_kind(1), Some(TokenKind::LParen))
}

fn callable_type(p: &mut Parser<'_>, mode: TypeMode) -> Option<()> {
    p.bump();
    p.expect(TokenKind::LParen, "expected `(` in callable type")?;
    while !p.at(TokenKind::RParen) && !p.at(TokenKind::Eof) {
        if p.at(TokenKind::Ellipsis) {
            p.recover_in_error(|p| {
                p.bump();
            });
            p.error_here("callable interface types cannot be variadic");
            break;
        }
        let checkpoint = p.checkpoint();
        if type_with_mode(p, mode).is_some() {
            if !type_parameter_delimiter(p) {
                break;
            }
        } else {
            recover_to_list_boundary(p, checkpoint, TokenKind::RParen);
        }
    }
    p.expect(TokenKind::RParen, "expected `)` in callable type")?;
    if type_can_start(p.kind()) {
        type_with_mode(p, mode)?;
    }
    Some(())
}

fn path_segments(p: &mut Parser<'_>, mode: TypeMode) -> Option<Shape> {
    let mut segments = 0usize;
    let mut first_is_name = false;
    let mut kept_args = false;
    loop {
        let is_name = matches!(p.kind(), TokenKind::Ident | TokenKind::Bool);
        p.node(|p| {
            let start = p.span().start;
            if !matches!(
                p.kind(),
                TokenKind::Ident
                    | TokenKind::Bool
                    | TokenKind::Pkg
                    | TokenKind::Super
                    | TokenKind::SelfValue
            ) {
                p.expected_here(ParseErrorKind::ExpectedName, "expected type path segment");
                return None;
            }
            p.bump();
            let checkpoint = p.checkpoint();
            let errors = p.errors_len();
            let args = type_args(p);
            if mode == TypeMode::BeforeAggregateLiteral
                && args > 0
                && !p.at(TokenKind::LBrace)
                && !p.at(TokenKind::LBracket)
            {
                p.rewind(checkpoint);
                p.truncate_errors(errors);
            } else {
                kept_args |= args > 0;
            }
            Done::new(SyntaxKind::PathSegment, Span::new(start, p.previous_end()))
        })?;
        if segments == 0 {
            first_is_name = is_name;
        }
        segments += 1;
        if p.eat(TokenKind::ColonColon).is_none() {
            break;
        }
    }
    let mut shape = Shape::default();
    if !kept_args {
        shape = shape.with(Shape::PATH_WITHOUT_ARGS);
    }
    if segments == 1 && first_is_name {
        shape = shape.with(Shape::SINGLE_NAME_PATH);
    }
    Some(shape)
}

/// `[...]` type arguments directly after a path segment; returns their count.
pub(crate) fn type_args(p: &mut Parser<'_>) -> usize {
    if p.span().start != p.previous_end() || !p.at(TokenKind::LBracket) {
        return 0;
    }
    let mut count = 0usize;
    p.node(|p| {
        let start = p.bump().start;
        count = type_args_after_open(p);
        Done::new(SyntaxKind::TypeArgList, Span::new(start, p.previous_end()))
    });
    count
}

fn at_type_arg_boundary(p: &Parser<'_>) -> bool {
    p.at(TokenKind::Comma) || p.at(TokenKind::RBracket) || p.at(TokenKind::Eof)
}

fn at_type_arg_recovery_boundary(p: &Parser<'_>) -> bool {
    matches!(
        p.kind(),
        TokenKind::RParen | TokenKind::RBrace | TokenKind::Semicolon
    ) || p.at_top_level_item_start()
}

const ARG_STOPS: [TokenKind; 2] = [TokenKind::Comma, TokenKind::RBracket];

fn const_arg(p: &mut Parser<'_>) -> Option<Parsed> {
    p.node(|p| {
        let expr = exprs::expr_until_tokens(p, &ARG_STOPS)?;
        Done::new(SyntaxKind::ConstArg, expr.span)
    })
}

fn type_arg(p: &mut Parser<'_>, kind: SyntaxKind) -> Option<Parsed> {
    p.node(|p| {
        let ty = type_(p)?;
        Done::new(kind, ty.span)
    })
}

fn type_args_after_open(p: &mut Parser<'_>) -> usize {
    let mut count = 0usize;
    while !p.at(TokenKind::RBracket) && !p.at(TokenKind::Eof) && !at_type_arg_recovery_boundary(p) {
        let checkpoint = p.checkpoint();
        let errors = p.errors_len();
        if p.at(TokenKind::DotDot) || p.at(TokenKind::DotDotEq) {
            if type_arg(p, SyntaxKind::TypeArg).is_some() {
                count += 1;
            } else {
                p.rewind(checkpoint);
                p.truncate_errors(errors);
                if const_arg(p).is_some() {
                    count += 1;
                }
            }
            if !type_arg_delimiter(p) {
                break;
            }
            continue;
        }
        if type_can_start(p.kind()) {
            match assoc_binding_arg(p) {
                AssocBinding::Parsed => {
                    count += 1;
                    if !type_arg_delimiter(p) {
                        break;
                    }
                    continue;
                }
                AssocBinding::FailedValue => {
                    if p.eat(TokenKind::Comma).is_some() {
                        continue;
                    }
                    break;
                }
                AssocBinding::NotBinding => {}
            }
        }
        let ty = speculate(p, |p| type_(p).filter(|_| at_type_arg_boundary(p)));
        let expr = speculate(p, |p| {
            exprs::expr_until_tokens(p, &ARG_STOPS).filter(|_| at_type_arg_boundary(p))
        });
        match (ty, expr) {
            (Some(ty), Some(expr))
                if ty.span == expr.span
                    && ty.class == SyntaxKind::PathType
                    && ty.shape.contains(Shape::PATH_WITHOUT_ARGS)
                    && expr.shape.contains(Shape::VALUE_PATH) =>
            {
                // Both readings are valid: keep the type in the tree and the
                // const expression as an alternate for semantic resolution.
                let alternate = p.checkpoint();
                let alternate_errors = p.errors_len();
                let snapshot = const_arg(p).map(|expr| (expr.span, p.snapshot(&expr, alternate)));
                p.rewind(alternate);
                p.truncate_errors(alternate_errors);
                if let Some((span, snapshot)) = snapshot {
                    p.push_alternate(AlternateKind::ConstArg, span, snapshot);
                }
                if type_arg(p, SyntaxKind::TypeOrConstArg).is_some() {
                    count += 1;
                } else {
                    p.error_here_as(ParseErrorKind::ExpectedType, "expected type argument");
                    skip_to_type_arg_boundary(p);
                    continue;
                }
            }
            (Some(_), _) => {
                if type_arg(p, SyntaxKind::TypeArg).is_some() {
                    count += 1;
                } else {
                    p.error_here_as(ParseErrorKind::ExpectedType, "expected type argument");
                    skip_to_type_arg_boundary(p);
                }
            }
            (None, Some(_)) => {
                if const_arg(p).is_some() {
                    count += 1;
                } else {
                    p.error_here("expected const generic argument");
                    skip_to_type_arg_boundary(p);
                }
            }
            (None, None) => {
                p.rewind(checkpoint);
                p.truncate_errors(errors);
                let before = p.errors_len();
                if const_arg(p).is_some() {
                    count += 1;
                } else {
                    if p.errors_len() == before {
                        p.error_here_as(ParseErrorKind::ExpectedExpression, "expected expression");
                    }
                    skip_to_type_arg_boundary(p);
                    if p.eat(TokenKind::Comma).is_some() {
                        continue;
                    }
                    break;
                }
            }
        }
        if !type_arg_delimiter(p) {
            break;
        }
    }
    p.expect(TokenKind::RBracket, "expected `]` after type arguments");
    count
}

/// Runs a production speculatively and returns its facts after rewinding.
fn speculate(
    p: &mut Parser<'_>,
    production: impl FnOnce(&mut Parser<'_>) -> Option<Parsed>,
) -> Option<Parsed> {
    let checkpoint = p.checkpoint();
    let errors = p.errors_len();
    let parsed = production(p);
    p.rewind(checkpoint);
    p.truncate_errors(errors);
    parsed
}

enum AssocBinding {
    Parsed,
    FailedValue,
    NotBinding,
}

fn assoc_binding_arg(p: &mut Parser<'_>) -> AssocBinding {
    let checkpoint = p.checkpoint();
    let errors = p.errors_len();
    let mut committed = false;
    let parsed = p.node(|p| {
        let key = type_(p)?;
        p.eat(TokenKind::Eq)?;
        committed = true;
        let value_errors = p.errors_len();
        let Some(value) = type_(p) else {
            p.truncate_errors(value_errors);
            p.error_here("expected associated type binding value");
            return None;
        };
        let key_is_valid = (key.class == SyntaxKind::PathType
            && key.shape.contains(Shape::SINGLE_NAME_PATH))
            || key.class == SyntaxKind::ProjectionType;
        if !key_is_valid {
            p.error_at(
                key.span,
                "associated type binding key must be a name or projection",
            );
        }
        Done::new(
            SyntaxKind::AssocBindingArg,
            Span::new(key.span.start, value.span.end),
        )
    });
    match (committed, parsed) {
        (true, Some(_)) => AssocBinding::Parsed,
        (true, None) => AssocBinding::FailedValue,
        (false, _) => {
            p.rewind(checkpoint);
            p.truncate_errors(errors);
            AssocBinding::NotBinding
        }
    }
}

fn type_arg_delimiter(p: &mut Parser<'_>) -> bool {
    if p.eat(TokenKind::Comma).is_some() {
        return true;
    }
    if p.at(TokenKind::RBracket) || p.at(TokenKind::Eof) || at_type_arg_recovery_boundary(p) {
        return false;
    }
    p.expected_here(
        ParseErrorKind::Grammar,
        "expected `,` or `]` after type argument",
    );
    if type_can_start(p.kind()) || exprs::expr_can_start(p.kind()) {
        return true;
    }
    skip_to_type_arg_boundary(p);
    p.eat(TokenKind::Comma).is_some()
}

fn type_parameter_delimiter(p: &mut Parser<'_>) -> bool {
    if p.eat(TokenKind::Comma).is_some() {
        return true;
    }
    if p.at(TokenKind::RParen) || p.at(TokenKind::Eof) {
        return false;
    }
    p.expected_here(
        ParseErrorKind::Grammar,
        "expected `,` or `)` after type parameter",
    );
    if type_can_start(p.kind()) {
        return true;
    }
    skip_until(p, &[TokenKind::RParen]);
    false
}

fn skip_to_type_arg_boundary(p: &mut Parser<'_>) {
    p.recover_in_error(|p| {
        let mut depth = Depth::default();
        while !p.at(TokenKind::Eof) {
            let kind = p.kind().clone();
            if depth.track(&kind) {
                p.bump();
                continue;
            }
            if depth.top()
                && (matches!(kind, TokenKind::Comma | TokenKind::RBracket)
                    || at_type_arg_recovery_boundary(p))
            {
                break;
            }
            p.bump();
        }
    });
}

const WHERE_STOPS: [TokenKind; 11] = [
    TokenKind::Struct,
    TokenKind::Union,
    TokenKind::Enum,
    TokenKind::Trait,
    TokenKind::Fn,
    TokenKind::Extern,
    TokenKind::Pub,
    TokenKind::Using,
    TokenKind::Module,
    TokenKind::Static,
    TokenKind::Type,
];

pub(crate) fn where_clause(p: &mut Parser<'_>) {
    if !p.at(TokenKind::Where) {
        return;
    }
    p.node(|p| {
        let start = p.bump().start;
        while !p.at(TokenKind::LBrace)
            && !p.at(TokenKind::Semicolon)
            && !p.at(TokenKind::Eof)
            && !p.at_any(&WHERE_STOPS)
        {
            let mut skip = false;
            where_predicate(p, &mut skip);
            if skip {
                skip_until(
                    p,
                    &[TokenKind::Comma, TokenKind::LBrace, TokenKind::Semicolon],
                );
                if p.eat(TokenKind::Comma).is_some() {
                    continue;
                }
                break;
            }
            if p.eat(TokenKind::Comma).is_none() {
                if at_ident_colon(p) {
                    p.expected_here(
                        ParseErrorKind::Grammar,
                        "expected `,` after where predicate",
                    );
                    continue;
                }
                break;
            }
        }
        Done::new(SyntaxKind::WhereClause, Span::new(start, p.previous_end()))
    });
}

fn where_predicate(p: &mut Parser<'_>, skip: &mut bool) -> Option<Parsed> {
    p.node(|p| {
        let start = p.span().start;
        if type_until(p, &[TokenKind::Colon]).is_none() {
            *skip = true;
            return None;
        }
        if p.expect(TokenKind::Colon, "expected `:` in where predicate")
            .is_none()
        {
            *skip = true;
            return None;
        }
        while let Some(bound) = where_bound(p) {
            if p.eat(TokenKind::Plus).is_some() {
                continue;
            }
            return Done::new(SyntaxKind::WherePredicate, Span::new(start, bound.span.end));
        }
        None
    })
}

fn where_bound(p: &mut Parser<'_>) -> Option<Parsed> {
    let mut stops = vec![
        TokenKind::Comma,
        TokenKind::Plus,
        TokenKind::LBrace,
        TokenKind::Semicolon,
    ];
    stops.extend(WHERE_STOPS);
    type_followed_by(
        p,
        |p| {
            p.at(TokenKind::Comma)
                || p.at(TokenKind::Plus)
                || p.at(TokenKind::LBrace)
                || p.at(TokenKind::Semicolon)
                || p.at_any(&WHERE_STOPS)
                || at_ident_colon(p)
        },
        &stops,
    )
}
