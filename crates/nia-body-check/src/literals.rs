// SPDX-License-Identifier: GPL-3.0-or-later
use nia_ast::{Expr, ExprKind, UnaryOp};
use nia_ty::PrimitiveTy;

pub(super) fn integer_literal_value(expr: &Expr) -> Option<i128> {
    match &expr.kind {
        ExprKind::Integer(text) => nia_const_eval::eval_int_literal(text).ok(),
        ExprKind::Unary {
            op: UnaryOp::Neg,
            expr,
        } => nia_const_eval::eval_int_literal(integer_literal_text(expr)?)
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
        ExprKind::Float(text) => Some(text),
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

pub(super) fn string_literal_byte_len(text: &str) -> Option<usize> {
    if text.starts_with("\\\\") {
        return multiline_string_literal_byte_len(text);
    }
    let inner = text.strip_prefix('"')?.strip_suffix('"')?;
    let mut bytes = 0usize;
    let mut chars = inner.chars();
    while let Some(ch) = chars.next() {
        if ch != '\\' {
            bytes += ch.len_utf8();
            continue;
        }
        match chars.next()? {
            'n' | 'r' | 't' | '\\' | '\'' | '"' | '0' => bytes += 1,
            'x' => {
                chars.next()?;
                chars.next()?;
                bytes += 1;
            }
            'u' => bytes += unicode_escape_byte_len(&mut chars)?,
            _ => return None,
        }
    }
    Some(bytes)
}

fn multiline_string_literal_byte_len(text: &str) -> Option<usize> {
    let mut bytes = 0usize;
    let mut pos = 0usize;
    loop {
        if !text[pos..].starts_with("\\\\") {
            return None;
        }
        pos += 2;

        let content_start = pos;
        while pos < text.len() && !matches!(text.as_bytes()[pos], b'\n' | b'\r') {
            pos += 1;
        }
        bytes += text[content_start..pos].len();

        if pos == text.len() {
            break;
        }
        bytes += 1;
        pos = consume_newline(text, pos)?;
        while matches!(text.as_bytes().get(pos), Some(b' ' | b'\t')) {
            pos += 1;
        }
    }
    Some(bytes)
}

fn unicode_escape_byte_len(chars: &mut std::str::Chars<'_>) -> Option<usize> {
    if chars.next()? != '{' {
        return None;
    }
    let mut value = String::new();
    for ch in chars.by_ref() {
        if ch == '}' {
            let scalar = u32::from_str_radix(&value, 16).ok()?;
            return Some(char::from_u32(scalar)?.len_utf8());
        }
        value.push(ch);
    }
    None
}

fn consume_newline(text: &str, pos: usize) -> Option<usize> {
    match text.as_bytes().get(pos)? {
        b'\n' => Some(pos + 1),
        b'\r' if text.as_bytes().get(pos + 1) == Some(&b'\n') => Some(pos + 2),
        b'\r' => Some(pos + 1),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rejects_unclosed_unicode_string_escape() {
        assert_eq!(string_literal_byte_len(r#""\u{41""#), None);
    }

    #[test]
    fn counts_multiline_string_literal_bytes() {
        assert_eq!(
            string_literal_byte_len("\\\\hello\n    \\\\world"),
            Some("hello\nworld".len())
        );
        assert_eq!(
            string_literal_byte_len("\\\\hello\\n"),
            Some(br"hello\n".len())
        );
    }
}
