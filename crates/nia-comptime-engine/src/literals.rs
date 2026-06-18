use crate::ComptimeValue;

use nia_comptime_ir::ComptimeStringLiteral;
use nia_ty::IntConst;

pub fn eval_int_literal(text: &str) -> Result<i128, String> {
    parse_int_literal(text)
}

pub fn eval_float_literal(text: &str) -> Result<f64, String> {
    let body = numeric_literal_body(text);
    body.replace('_', "")
        .parse::<f64>()
        .map_err(|_| "invalid float constant".to_string())
}

pub(crate) fn string_to_char_array(value: &str) -> Vec<ComptimeValue> {
    value
        .chars()
        .map(|ch| ComptimeValue::Int(IntConst::unsigned(ch as u128)))
        .collect()
}

pub(crate) fn bytes_to_array(value: &[u8]) -> Vec<ComptimeValue> {
    value
        .iter()
        .map(|byte| ComptimeValue::Int(IntConst::unsigned(u128::from(*byte))))
        .collect()
}

pub(crate) fn char_array_to_string(values: &[ComptimeValue]) -> Option<String> {
    let mut out = String::new();
    for value in values {
        let ComptimeValue::Int(value) = value else {
            return None;
        };
        let value = u32::try_from(value.bits()).ok()?;
        out.push(char::from_u32(value)?);
    }
    Some(out)
}

pub(crate) fn comptime_error_message(value: &ComptimeValue) -> Option<String> {
    match value {
        ComptimeValue::String(value) => Some(value.clone()),
        ComptimeValue::Array(values) => char_array_to_string(values),
        ComptimeValue::Pointer(value) => comptime_error_message(value),
        _ => None,
    }
}

pub fn eval_string_literal(literal: &ComptimeStringLiteral) -> Option<String> {
    if literal.parts.len() > 1 && literal.parts.iter().any(|part| is_multiline_literal(part)) {
        return None;
    }
    let mut out = String::new();
    for part in &literal.parts {
        out.push_str(&decode_string_literal_part(part)?);
    }
    Some(out)
}

fn decode_string_literal_part(text: &str) -> Option<String> {
    if is_multiline_literal(text) {
        return decode_multiline_string_literal(text)?
            .into_iter()
            .map(|byte| char::from_u32(u32::from(byte)))
            .collect();
    }
    let inner = text.strip_prefix('"')?.strip_suffix('"')?;
    decode_scalar_literal_inner(inner)
        .and_then(|scalars| scalars.into_iter().map(char::from_u32).collect())
}

pub fn eval_byte_string_literal(literal: &ComptimeStringLiteral) -> Option<Vec<u8>> {
    eval_byte_literal(literal, "b\"")
}

fn eval_byte_literal(literal: &ComptimeStringLiteral, prefix: &str) -> Option<Vec<u8>> {
    if literal.parts.len() > 1 && literal.parts.iter().any(|part| is_multiline_literal(part)) {
        return None;
    }
    let mut bytes = Vec::new();
    for part in &literal.parts {
        bytes.extend(decode_byte_literal_part(part, prefix)?);
    }
    Some(bytes)
}

fn decode_byte_literal_part(text: &str, prefix: &str) -> Option<Vec<u8>> {
    if is_multiline_literal(text) {
        return decode_multiline_string_literal(text);
    }
    let inner = text.strip_prefix(prefix)?.strip_suffix('"')?;
    decode_byte_literal_inner(inner)
}

fn is_multiline_literal(text: &str) -> bool {
    text.strip_prefix('b')
        .or_else(|| text.strip_prefix('c'))
        .unwrap_or(text)
        .starts_with("\\\\")
}

pub(crate) fn decode_char_literal(text: &str) -> Option<u32> {
    let inner = text.strip_prefix('\'')?.strip_suffix('\'')?;
    let scalars = decode_scalar_literal_inner(inner)?;
    (scalars.len() == 1).then_some(scalars[0])
}

pub(crate) fn decode_byte_char_literal(text: &str) -> Option<u8> {
    let inner = text.strip_prefix("b'")?.strip_suffix('\'')?;
    let bytes = decode_byte_literal_inner(inner)?;
    (bytes.len() == 1).then_some(bytes[0])
}

fn decode_byte_literal_inner(text: &str) -> Option<Vec<u8>> {
    let mut bytes = Vec::new();
    let mut chars = text.chars();
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
                let mut buf = [0; 4];
                let ch = char::from_u32(decode_unicode_escape_scalar(&mut chars)?)?;
                bytes.extend_from_slice(ch.encode_utf8(&mut buf).as_bytes());
            }
            _ => return None,
        }
    }
    Some(bytes)
}

fn decode_scalar_literal_inner(text: &str) -> Option<Vec<u32>> {
    let mut scalars = Vec::new();
    let mut chars = text.chars();
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

fn consume_newline(source: &str, pos: usize) -> Option<usize> {
    match source.as_bytes().get(pos)? {
        b'\n' => Some(pos + 1),
        b'\r' if source.as_bytes().get(pos + 1) == Some(&b'\n') => Some(pos + 2),
        b'\r' => Some(pos + 1),
        _ => None,
    }
}

pub(crate) fn checked_shift(lhs: i128, rhs: i128, is_left: bool) -> Result<i128, String> {
    let Ok(rhs) = u32::try_from(rhs) else {
        return Err("shift count is out of range in comptime expression".to_string());
    };
    if rhs >= i128::BITS {
        return Err("shift count is out of range in comptime expression".to_string());
    }
    if is_left {
        lhs.checked_shl(rhs)
            .ok_or_else(|| "integer overflow in comptime left shift".to_string())
    } else {
        lhs.checked_shr(rhs)
            .ok_or_else(|| "integer overflow in comptime right shift".to_string())
    }
}

pub(crate) fn checked_shift_u128(lhs: u128, rhs: u128, is_left: bool) -> Result<u128, String> {
    let Ok(rhs) = u32::try_from(rhs) else {
        return Err("shift count is out of range in comptime expression".to_string());
    };
    if rhs >= u128::BITS {
        return Err("shift count is out of range in comptime expression".to_string());
    }
    if is_left {
        lhs.checked_shl(rhs)
            .ok_or_else(|| "integer overflow in comptime left shift".to_string())
    } else {
        lhs.checked_shr(rhs)
            .ok_or_else(|| "integer overflow in comptime right shift".to_string())
    }
}

fn parse_int_literal(text: &str) -> Result<i128, String> {
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
    let digits = digits.replace('_', "");
    if digits.is_empty() {
        return Err("invalid integer constant".to_string());
    }
    i128::from_str_radix(&digits, radix)
        .map_err(|_| "integer literal is out of range for comptime evaluation".to_string())
}

fn numeric_literal_body(text: &str) -> &str {
    let suffix_start = numeric_suffix_start(text).unwrap_or(text.len());
    &text[..suffix_start]
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
    if !non_decimal_radix && index < bytes.len() && bytes[index] == b'.' {
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
    if !non_decimal_radix && index < bytes.len() && matches!(bytes[index], b'e' | b'E') {
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
        b'0'..=b'9' => Some(u32::from(byte - b'0')),
        b'a'..=b'f' => Some(u32::from(byte - b'a') + 10),
        b'A'..=b'F' => Some(u32::from(byte - b'A') + 10),
        _ => None,
    }
}
