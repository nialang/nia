// SPDX-License-Identifier: GPL-3.0-or-later
use nia_ast::{Expr, ExprKind, StringLiteral, UnaryOp};
use nia_ty::PrimitiveTy;

pub(super) fn integer_literal_value(expr: &Expr) -> Option<i128> {
    match &expr.kind {
        ExprKind::Integer(text) => {
            nia_comptime_engine::eval_int_literal(numeric_literal_body(text)).ok()
        }
        ExprKind::Unary {
            op: UnaryOp::Neg,
            expr,
        } => {
            nia_comptime_engine::eval_int_literal(numeric_literal_body(integer_literal_text(expr)?))
                .ok()?
                .checked_neg()
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

pub(super) fn numeric_literal_suffix(text: &str) -> Option<&str> {
    let start = numeric_suffix_start(text)?;
    Some(&text[start..])
}

fn integer_suffix_ty(text: &str) -> Option<PrimitiveTy> {
    Some(match numeric_literal_suffix(text)? {
        "i8" => PrimitiveTy::I8,
        "i16" => PrimitiveTy::I16,
        "i32" => PrimitiveTy::I32,
        "i64" => PrimitiveTy::I64,
        "i128" => PrimitiveTy::I128,
        "isize" => PrimitiveTy::Isize,
        "u8" => PrimitiveTy::U8,
        "u16" => PrimitiveTy::U16,
        "u32" => PrimitiveTy::U32,
        "u64" => PrimitiveTy::U64,
        "u128" => PrimitiveTy::U128,
        "usize" => PrimitiveTy::Usize,
        _ => return None,
    })
}

fn float_suffix_ty(text: &str) -> Option<PrimitiveTy> {
    Some(match numeric_literal_suffix(text)? {
        "f32" => PrimitiveTy::F32,
        "f64" => PrimitiveTy::F64,
        _ => return None,
    })
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
    if literal.parts.len() > 1 && literal.parts.iter().any(|part| is_multiline_literal(part)) {
        return None;
    }
    literal.parts.iter().try_fold(0usize, |len, text| {
        len.checked_add(string_literal_part_char_len(text)?)
    })
}

fn string_literal_part_char_len(text: &str) -> Option<usize> {
    if text
        .strip_prefix("b")
        .or_else(|| text.strip_prefix("c"))
        .unwrap_or(text)
        .starts_with("\\\\")
    {
        return multiline_string_literal_char_len(text);
    }
    let inner = text.strip_prefix('"')?.strip_suffix('"')?;
    decoded_scalar_len(inner)
}

pub(super) fn byte_string_literal_len(literal: &StringLiteral) -> Option<usize> {
    if literal.parts.len() > 1 && literal.parts.iter().any(|part| is_multiline_literal(part)) {
        return None;
    }
    literal.parts.iter().try_fold(0usize, |len, text| {
        len.checked_add(byte_string_literal_part_len(text)?)
    })
}

fn byte_string_literal_part_len(text: &str) -> Option<usize> {
    if text.strip_prefix("b").unwrap_or(text).starts_with("\\\\") {
        return multiline_string_literal_byte_len(text);
    }
    let inner = text
        .strip_prefix("b\"")
        .or_else(|| text.strip_prefix('"'))?
        .strip_suffix('"')?;
    decoded_byte_len(inner)
}

pub(super) fn c_string_literal_len(literal: &StringLiteral) -> Option<usize> {
    if literal.parts.len() > 1 && literal.parts.iter().any(|part| is_multiline_literal(part)) {
        return None;
    }
    let bytes = literal.parts.iter().try_fold(0usize, |len, text| {
        len.checked_add(c_string_literal_part_len(text)?)
    })?;
    bytes.checked_add(1)
}

fn is_multiline_literal(text: &str) -> bool {
    text.strip_prefix('b')
        .or_else(|| text.strip_prefix('c'))
        .unwrap_or(text)
        .starts_with("\\\\")
}

fn c_string_literal_part_len(text: &str) -> Option<usize> {
    if text.strip_prefix("c").unwrap_or(text).starts_with("\\\\") {
        return multiline_string_literal_byte_len(text);
    }
    let inner = text.strip_prefix("c\"")?.strip_suffix('"')?;
    decoded_byte_len(inner)
}

fn decoded_byte_len(inner: &str) -> Option<usize> {
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

fn decoded_scalar_len(inner: &str) -> Option<usize> {
    let mut scalars = 0usize;
    let mut chars = inner.chars();
    while let Some(ch) = chars.next() {
        if ch != '\\' {
            scalars += 1;
            continue;
        }
        match chars.next()? {
            'n' | 'r' | 't' | '\\' | '\'' | '"' | '0' => scalars += 1,
            'x' => {
                chars.next()?.to_digit(16)?;
                chars.next()?.to_digit(16)?;
                scalars += 1;
            }
            'u' => {
                unicode_escape_byte_len(&mut chars)?;
                scalars += 1;
            }
            _ => return None,
        }
    }
    Some(scalars)
}

fn multiline_string_literal_char_len(text: &str) -> Option<usize> {
    let mut scalars = 0usize;
    let source = strip_multiline_prefix(text)?;
    let mut pos = 0usize;
    loop {
        if !source[pos..].starts_with("\\\\") {
            return None;
        }
        pos += 2;

        let content_start = pos;
        while pos < source.len() && !matches!(source.as_bytes()[pos], b'\n' | b'\r') {
            pos += 1;
        }
        scalars += source[content_start..pos].chars().count();

        if pos == source.len() {
            break;
        }
        scalars += 1;
        pos = consume_newline(source, pos)?;
        while matches!(source.as_bytes().get(pos), Some(b' ' | b'\t')) {
            pos += 1;
        }
    }
    Some(scalars)
}

fn multiline_string_literal_byte_len(text: &str) -> Option<usize> {
    let source = strip_multiline_prefix(text)?;
    let mut bytes = 0usize;
    let mut pos = 0usize;
    loop {
        if !source[pos..].starts_with("\\\\") {
            return None;
        }
        pos += 2;

        let content_start = pos;
        while pos < source.len() && !matches!(source.as_bytes()[pos], b'\n' | b'\r') {
            pos += 1;
        }
        bytes += source[content_start..pos].len();

        if pos == source.len() {
            break;
        }
        bytes += 1;
        pos = consume_newline(source, pos)?;
        while matches!(source.as_bytes().get(pos), Some(b' ' | b'\t')) {
            pos += 1;
        }
    }
    Some(bytes)
}

fn strip_multiline_prefix(text: &str) -> Option<&str> {
    if text.starts_with("\\\\") {
        Some(text)
    } else if let Some(rest) = text.strip_prefix('b') {
        rest.starts_with("\\\\").then_some(rest)
    } else if let Some(rest) = text.strip_prefix('c') {
        rest.starts_with("\\\\").then_some(rest)
    } else {
        None
    }
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

    #[test]
    fn counts_adjacent_quoted_c_string_with_single_nul() {
        assert_eq!(
            c_string_literal_len(&StringLiteral {
                parts: vec![r#"c"foo""#.to_string(), r#"c"bar""#.to_string()],
            }),
            Some(7)
        );
    }
}
