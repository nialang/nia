// SPDX-License-Identifier: GPL-3.0-or-later
use nia_ast::{Expr, ExprKind, StringLiteral, UnaryOp};
use nia_ty::PrimitiveTy;

pub(super) fn integer_literal_value(expr: &Expr) -> Option<nia_ty::IntConst> {
    match &expr.kind {
        ExprKind::Integer(text) => nia_literals::eval_int_literal(text)
            .ok()
            .map(nia_ty::IntConst::unsigned),
        ExprKind::Unary {
            op: UnaryOp::Neg,
            expr,
        } => {
            let magnitude = nia_literals::eval_int_literal(integer_literal_text(expr)?).ok()?;
            if magnitude == 1_u128 << 127 {
                Some(nia_ty::IntConst::signed(i128::MIN))
            } else {
                i128::try_from(magnitude)
                    .ok()?
                    .checked_neg()
                    .map(nia_ty::IntConst::signed)
            }
        }
        _ => None,
    }
}

fn integer_literal_text(expr: &Expr) -> Option<&str> {
    match &expr.kind {
        ExprKind::Integer(text) => Some(text),
        _ => None,
    }
}

pub(super) fn float_literal_text(expr: &Expr) -> Option<&str> {
    match &expr.kind {
        ExprKind::Float(text) => Some(nia_literals::numeric_literal_body(text)),
        _ => None,
    }
}

pub(super) fn integer_literal_suffix_ty(expr: &Expr) -> Option<PrimitiveTy> {
    match &expr.kind {
        ExprKind::Integer(text) => integer_suffix_ty(text),
        ExprKind::Unary {
            op: UnaryOp::Neg,
            expr,
        } => match &expr.kind {
            ExprKind::Integer(text) => integer_suffix_ty(text),
            _ => None,
        },
        _ => None,
    }
}

pub(super) fn float_literal_suffix_ty(expr: &Expr) -> Option<PrimitiveTy> {
    match &expr.kind {
        ExprKind::Float(text) => float_suffix_ty(text),
        ExprKind::Unary {
            op: UnaryOp::Neg,
            expr,
        } => match &expr.kind {
            ExprKind::Float(text) => float_suffix_ty(text),
            _ => None,
        },
        _ => None,
    }
}

pub(super) fn has_numeric_literal_suffix(expr: &Expr) -> bool {
    match &expr.kind {
        ExprKind::Integer(text) | ExprKind::Float(text) => numeric_literal_suffix(text).is_some(),
        ExprKind::Unary {
            op: UnaryOp::Neg,
            expr,
        } => has_numeric_literal_suffix(expr),
        _ => false,
    }
}

pub(super) fn numeric_literal_body(text: &str) -> &str {
    nia_literals::numeric_literal_body(text)
}

pub(super) fn decode_char_literal(text: &str) -> Option<u32> {
    nia_literals::decode_char_literal(text)
}

pub(super) fn decode_byte_char_literal(text: &str) -> Option<u8> {
    nia_literals::decode_byte_char_literal(text)
}

pub(super) fn parse_int_literal(text: &str) -> Option<u128> {
    nia_literals::eval_int_literal(text).ok()
}

pub(super) fn decode_string_literal(literal: &StringLiteral) -> Option<Vec<u32>> {
    nia_literals::decode_string_literal_scalars(literal.parts.iter().map(String::as_str))
}

pub(super) fn decode_byte_string_literal(literal: &StringLiteral) -> Option<Vec<u8>> {
    nia_literals::eval_byte_string_literal_parts(literal.parts.iter().map(String::as_str))
}

pub(super) fn numeric_literal_suffix(text: &str) -> Option<&str> {
    nia_literals::numeric_literal_suffix(text)
}

fn integer_suffix_ty(text: &str) -> Option<PrimitiveTy> {
    let primitive = PrimitiveTy::from_name(numeric_literal_suffix(text)?)?;
    primitive.is_integer().then_some(primitive)
}

fn float_suffix_ty(text: &str) -> Option<PrimitiveTy> {
    let primitive = PrimitiveTy::from_name(numeric_literal_suffix(text)?)?;
    primitive.is_float().then_some(primitive)
}

pub(super) trait FiniteFloat: Sized {
    fn parse(text: &str) -> Option<Self>;
}

impl FiniteFloat for f32 {
    fn parse(text: &str) -> Option<Self> {
        let value = text.parse::<f32>().ok()?;
        value.is_finite().then_some(value)
    }
}

impl FiniteFloat for f64 {
    fn parse(text: &str) -> Option<Self> {
        let value = text.parse::<f64>().ok()?;
        value.is_finite().then_some(value)
    }
}

pub(super) fn parse_float_literal<T: FiniteFloat>(text: &str) -> bool {
    T::parse(text).is_some()
}

pub(super) fn string_literal_char_len(literal: &StringLiteral) -> Option<usize> {
    nia_literals::string_literal_char_len(literal.parts.iter().map(String::as_str))
}

pub(super) fn byte_string_literal_len(literal: &StringLiteral) -> Option<usize> {
    nia_literals::byte_string_literal_len(literal.parts.iter().map(String::as_str))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rejects_unclosed_unicode_string_escape() {
        assert_eq!(
            string_literal_char_len(&StringLiteral {
                parts: vec![r#""\u{41""#.to_string()],
            }),
            None
        );
    }

    #[test]
    fn counts_multiline_string_literal_scalars() {
        assert_eq!(
            string_literal_char_len(&StringLiteral {
                parts: vec!["\\\\hello\n    \\\\world".to_string()],
            }),
            Some("hello\nworld".chars().count())
        );
        assert_eq!(
            string_literal_char_len(&StringLiteral {
                parts: vec!["\\\\hello\\n".to_string()],
            }),
            Some("hello\\n".chars().count())
        );
    }
}
