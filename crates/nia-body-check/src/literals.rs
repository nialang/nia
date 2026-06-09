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

pub(super) fn decode_char_literal(text: &str) -> Option<u32> {
    let inner = text.strip_prefix('\'')?.strip_suffix('\'')?;
    let scalars = decode_char_scalars(inner)?;
    (scalars.len() == 1).then_some(scalars[0])
}

pub(super) fn decode_byte_char_literal(text: &str) -> Option<u8> {
    let inner = text.strip_prefix("b'")?.strip_suffix('\'')?;
    decode_char_inner(inner).and_then(|bytes| (bytes.len() == 1).then_some(bytes[0]))
}

pub(super) fn parse_int_literal(text: &str) -> Option<i128> {
    let text = numeric_literal_body(text);
    let (radix, digits) =
        if let Some(rest) = text.strip_prefix("0x").or_else(|| text.strip_prefix("0X")) {
            (16, rest)
        } else if let Some(rest) = text.strip_prefix("0b").or_else(|| text.strip_prefix("0B")) {
            (2, rest)
        } else if let Some(rest) = text.strip_prefix("0o").or_else(|| text.strip_prefix("0O")) {
            (8, rest)
        } else {
            (10, text)
        };
    i128::from_str_radix(&digits.replace('_', ""), radix).ok()
}

pub(super) fn decode_string_literal(literal: &StringLiteral) -> Option<Vec<u32>> {
    if literal.parts.len() > 1 && literal.parts.iter().any(|part| is_multiline_literal(part)) {
        return None;
    }
    let mut scalars = Vec::new();
    for part in &literal.parts {
        scalars.extend(decode_string_literal_part(part)?);
    }
    Some(scalars)
}

fn decode_string_literal_part(text: &str) -> Option<Vec<u32>> {
    if strip_multiline_prefix(text).is_some() {
        return decode_multiline_string_literal(text).map(bytes_to_scalars);
    }
    let inner = text.strip_prefix('"')?.strip_suffix('"')?;
    decode_char_scalars(inner)
}

pub(super) fn decode_byte_string_literal(literal: &StringLiteral) -> Option<Vec<u8>> {
    if literal.parts.len() > 1 && literal.parts.iter().any(|part| is_multiline_literal(part)) {
        return None;
    }
    let mut bytes = Vec::new();
    for part in &literal.parts {
        bytes.extend(decode_byte_string_literal_part(part)?);
    }
    Some(bytes)
}

fn decode_byte_string_literal_part(text: &str) -> Option<Vec<u8>> {
    if strip_multiline_prefix(text).is_some() {
        return decode_multiline_string_literal(text);
    }
    let inner = text.strip_prefix("b\"")?.strip_suffix('"')?;
    decode_char_inner(inner)
}

pub(super) fn decode_c_string_literal(literal: &StringLiteral) -> Option<Vec<u8>> {
    if literal.parts.len() > 1 && literal.parts.iter().any(|part| is_multiline_literal(part)) {
        return None;
    }
    let mut bytes = Vec::new();
    for part in &literal.parts {
        bytes.extend(decode_c_string_literal_part(part)?);
    }
    bytes.push(0);
    Some(bytes)
}

fn decode_c_string_literal_part(text: &str) -> Option<Vec<u8>> {
    if strip_multiline_prefix(text).is_some() {
        return decode_multiline_string_literal(text);
    }
    let inner = text.strip_prefix("c\"")?.strip_suffix('"')?;
    decode_char_inner(inner)
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

fn decode_char_inner(inner: &str) -> Option<Vec<u8>> {
    let mut bytes = Vec::new();
    let mut chars = inner.chars();
    while let Some(ch) = chars.next() {
        if ch != '\\' {
            let mut buf = [0; 4];
            bytes.extend_from_slice(ch.encode_utf8(&mut buf).as_bytes());
            continue;
        }
        match chars.next()? {
            'n' => bytes.push(b'\n'),
            'r' => bytes.push(b'\r'),
            't' => bytes.push(b'\t'),
            '\\' => bytes.push(b'\\'),
            '\'' => bytes.push(b'\''),
            '"' => bytes.push(b'"'),
            '0' => bytes.push(0),
            'x' => {
                let hi = chars.next()?.to_digit(16)?;
                let lo = chars.next()?.to_digit(16)?;
                bytes.push(((hi << 4) | lo) as u8);
            }
            'u' => {
                bytes.extend_from_slice(&decode_unicode_escape(&mut chars)?);
            }
            _ => return None,
        }
    }
    Some(bytes)
}

fn decode_char_scalars(inner: &str) -> Option<Vec<u32>> {
    let mut scalars = Vec::new();
    let mut chars = inner.chars();
    while let Some(ch) = chars.next() {
        if ch != '\\' {
            scalars.push(ch as u32);
            continue;
        }
        let scalar = match chars.next()? {
            'n' => '\n' as u32,
            'r' => '\r' as u32,
            't' => '\t' as u32,
            '\\' => '\\' as u32,
            '\'' => '\'' as u32,
            '"' => '"' as u32,
            '0' => 0,
            'x' => {
                let hi = chars.next()?.to_digit(16)?;
                let lo = chars.next()?.to_digit(16)?;
                (hi << 4) | lo
            }
            'u' => decode_unicode_escape_scalar(&mut chars)?,
            _ => return None,
        };
        char::from_u32(scalar)?;
        scalars.push(scalar);
    }
    Some(scalars)
}

fn bytes_to_scalars(bytes: Vec<u8>) -> Vec<u32> {
    bytes.into_iter().map(u32::from).collect()
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

fn decode_multiline_string_literal(text: &str) -> Option<Vec<u8>> {
    let source = strip_multiline_prefix(text)?;
    let mut bytes = Vec::new();
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
        bytes.extend_from_slice(&source.as_bytes()[content_start..pos]);

        if pos == source.len() {
            break;
        }
        bytes.push(b'\n');
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

fn decode_unicode_escape(chars: &mut std::str::Chars<'_>) -> Option<Vec<u8>> {
    if chars.next()? != '{' {
        return None;
    }
    let mut value = String::new();
    for ch in chars.by_ref() {
        if ch == '}' {
            let scalar = u32::from_str_radix(&value, 16).ok()?;
            let ch = char::from_u32(scalar)?;
            let mut buf = [0; 4];
            return Some(ch.encode_utf8(&mut buf).as_bytes().to_vec());
        }
        value.push(ch);
    }
    None
}

fn decode_unicode_escape_scalar(chars: &mut std::str::Chars<'_>) -> Option<u32> {
    if chars.next()? != '{' {
        return None;
    }
    let mut value = String::new();
    for ch in chars.by_ref() {
        if ch == '}' {
            let scalar = u32::from_str_radix(&value, 16).ok()?;
            char::from_u32(scalar)?;
            return Some(scalar);
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
