// SPDX-License-Identifier: GPL-3.0-or-later
use nia_ast::{Expr, ExprKind, StringLiteral, UnaryOp};
use nia_ty::PrimitiveTy;

pub(super) fn integer_literal_value(expr: &Expr) -> Option<i128> {
    match &expr.kind {
        ExprKind::Integer(text) => nia_literals::eval_int_literal(text).ok(),
        ExprKind::Unary {
            op: UnaryOp::Neg,
            expr,
        } => nia_literals::eval_int_literal(integer_literal_text(expr)?)
            .ok()?
            .checked_neg(),
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
        ExprKind::Float(text) => Some(numeric_literal_body(text)),
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
    let suffix_start = numeric_suffix_start(text).unwrap_or(text.len());
    &text[..suffix_start]
}

pub(super) fn decode_char_literal(text: &str) -> Option<u32> {
    nia_literals::decode_char_literal(text)
}

pub(super) fn decode_byte_char_literal(text: &str) -> Option<u8> {
    nia_literals::decode_byte_char_literal(text)
}

pub(super) fn parse_int_literal(text: &str) -> Option<i128> {
    nia_literals::eval_int_literal(text).ok()
}

pub(super) fn decode_string_literal(literal: &StringLiteral) -> Option<Vec<u32>> {
    nia_literals::decode_string_literal_scalars(literal.parts.iter().map(String::as_str))
}

pub(super) fn decode_byte_string_literal(literal: &StringLiteral) -> Option<Vec<u8>> {
    nia_literals::eval_byte_string_literal_parts(literal.parts.iter().map(String::as_str))
}

pub(super) fn numeric_literal_suffix(text: &str) -> Option<&str> {
    let start = numeric_suffix_start(text)?;
    Some(&text[start..])
}

fn integer_suffix_ty(text: &str) -> Option<PrimitiveTy> {
    let primitive = PrimitiveTy::from_name(numeric_literal_suffix(text)?)?;
    primitive.is_integer().then_some(primitive)
}

fn float_suffix_ty(text: &str) -> Option<PrimitiveTy> {
    let primitive = PrimitiveTy::from_name(numeric_literal_suffix(text)?)?;
    primitive.is_float().then_some(primitive)
}

fn numeric_suffix_start(text: &str) -> Option<usize> {
    let bytes = text.as_bytes();
    let non_decimal_radix = text.starts_with("0x")
        || text.starts_with("0X")
        || text.starts_with("0b")
        || text.starts_with("0B")
        || text.starts_with("0o")
        || text.starts_with("0O");
    let mut index = if non_decimal_radix { 2 } else { 0 };
    while index < bytes.len() {
        let byte = bytes[index];
        if byte == b'_'
            || if non_decimal_radix {
                digit_value(byte).is_some()
            } else {
                byte.is_ascii_digit()
            }
        {
            index += 1;
        } else {
            break;
        }
    }
    if index < bytes.len() && bytes[index] == b'.' {
        index += 1;
        while index < bytes.len() {
            let byte = bytes[index];
            if byte == b'_' || byte.is_ascii_digit() {
                index += 1;
            } else {
                break;
            }
        }
    }
    if index < bytes.len() && matches!(bytes[index], b'e' | b'E') {
        index += 1;
        if index < bytes.len() && matches!(bytes[index], b'+' | b'-') {
            index += 1;
        }
        while index < bytes.len() {
            let byte = bytes[index];
            if byte == b'_' || byte.is_ascii_digit() {
                index += 1;
            } else {
                break;
            }
        }
    }
    (index < bytes.len()).then_some(index)
}

fn digit_value(byte: u8) -> Option<u32> {
    match byte {
        b'0'..=b'9' => Some((byte - b'0') as u32),
        b'a'..=b'f' => Some((byte - b'a' + 10) as u32),
        b'A'..=b'F' => Some((byte - b'A' + 10) as u32),
        _ => None,
    }
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

pub(super) fn integer_range(primitive: PrimitiveTy) -> Option<(i128, i128)> {
    Some(match primitive {
        PrimitiveTy::I8 => (i8::MIN as i128, i8::MAX as i128),
        PrimitiveTy::I16 => (i16::MIN as i128, i16::MAX as i128),
        PrimitiveTy::I32 => (i32::MIN as i128, i32::MAX as i128),
        PrimitiveTy::I64 => (i64::MIN as i128, i64::MAX as i128),
        PrimitiveTy::I128 => (i128::MIN, i128::MAX),
        PrimitiveTy::Isize => (isize::MIN as i128, isize::MAX as i128),
        PrimitiveTy::U8 => (u8::MIN as i128, u8::MAX as i128),
        PrimitiveTy::U16 => (u16::MIN as i128, u16::MAX as i128),
        PrimitiveTy::U32 => (u32::MIN as i128, u32::MAX as i128),
        PrimitiveTy::U64 => (u64::MIN as i128, u64::MAX as i128),
        PrimitiveTy::U128 => (0, i128::MAX),
        PrimitiveTy::Usize => (usize::MIN as i128, usize::MAX as i128),
        PrimitiveTy::F32
        | PrimitiveTy::F64
        | PrimitiveTy::Bool
        | PrimitiveTy::Char
        | PrimitiveTy::Void
        | PrimitiveTy::Never => return None,
    })
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
