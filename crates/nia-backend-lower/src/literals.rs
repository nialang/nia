// SPDX-License-Identifier: GPL-3.0-or-later
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

pub(super) fn numeric_literal_body(text: &str) -> &str {
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
        b'0'..=b'9' => Some((byte - b'0') as u32),
        b'a'..=b'f' => Some((byte - b'a' + 10) as u32),
        b'A'..=b'F' => Some((byte - b'A' + 10) as u32),
        _ => None,
    }
}

pub(super) fn decode_byte_char(text: &str) -> Option<u8> {
    let inner = text.strip_prefix("b'")?.strip_suffix('\'')?;
    decode_char_inner(inner).and_then(|bytes| (bytes.len() == 1).then_some(bytes[0]))
}

pub(super) fn decode_char_literal(text: &str) -> Option<u32> {
    let inner = text.strip_prefix('\'')?.strip_suffix('\'')?;
    let scalars = decode_char_scalars(inner)?;
    (scalars.len() == 1).then_some(scalars[0])
}

pub(super) fn decode_string_literal(literal: &nia_ast::StringLiteral) -> Option<Vec<u32>> {
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

pub(super) fn decode_byte_string_literal(literal: &nia_ast::StringLiteral) -> Option<Vec<u8>> {
    if literal.parts.len() > 1 && literal.parts.iter().any(|part| is_multiline_literal(part)) {
        return None;
    }
    let mut bytes = Vec::new();
    for part in &literal.parts {
        bytes.extend(decode_byte_string_literal_part(part)?);
    }
    Some(bytes)
}

pub(super) fn decode_byte_string_literal_part(text: &str) -> Option<Vec<u8>> {
    if strip_multiline_prefix(text).is_some() {
        return decode_multiline_string_literal(text);
    }
    let inner = text.strip_prefix("b\"")?.strip_suffix('"')?;
    decode_char_inner(inner)
}

pub(super) fn decode_c_string_literal(literal: &nia_ast::StringLiteral) -> Option<Vec<u8>> {
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

fn is_multiline_literal(text: &str) -> bool {
    strip_multiline_prefix(text).is_some()
}

fn decode_c_string_literal_part(text: &str) -> Option<Vec<u8>> {
    if strip_multiline_prefix(text).is_some() {
        return decode_multiline_string_literal(text);
    }
    let inner = text.strip_prefix("c\"")?.strip_suffix('"')?;
    decode_char_inner(inner)
}

fn decode_multiline_string_literal(text: &str) -> Option<Vec<u8>> {
    let mut bytes = Vec::new();
    let text = strip_multiline_prefix(text)?;
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
        bytes.extend_from_slice(&text.as_bytes()[content_start..pos]);

        if pos == text.len() {
            break;
        }
        bytes.push(b'\n');
        pos = consume_newline(text, pos)?;
        while matches!(text.as_bytes().get(pos), Some(b' ' | b'\t')) {
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
            decode_string_literal(&nia_ast::StringLiteral {
                parts: vec![r#""\u{41""#.to_string()],
            }),
            None
        );
    }

    #[test]
    fn decodes_multiline_string_literal_without_escapes_or_trailing_newline() {
        assert_eq!(
            decode_string_literal(&nia_ast::StringLiteral {
                parts: vec!["\\\\mov rax, 60\n    \\\\syscall".to_string()],
            }),
            Some("mov rax, 60\nsyscall".chars().map(|ch| ch as u32).collect())
        );
        assert_eq!(
            decode_string_literal(&nia_ast::StringLiteral {
                parts: vec!["\\\\hello\\n\n\\\\world".to_string()],
            }),
            Some("hello\\n\nworld".chars().map(|ch| ch as u32).collect())
        );
        assert_eq!(
            decode_byte_string_literal(&nia_ast::StringLiteral {
                parts: vec!["b\\\\hello\n\\\\world".to_string()],
            }),
            Some(b"hello\nworld".to_vec())
        );
        assert_eq!(
            decode_c_string_literal(&nia_ast::StringLiteral {
                parts: vec!["c\\\\hello\n\\\\world".to_string()],
            }),
            Some(b"hello\nworld\0".to_vec())
        );
    }

    #[test]
    fn decodes_adjacent_c_string_with_single_nul() {
        assert_eq!(
            decode_c_string_literal(&nia_ast::StringLiteral {
                parts: vec![
                    r#"c"""#.to_string(),
                    r#"c"foo""#.to_string(),
                    r#"c"""#.to_string(),
                    r#"c"bar""#.to_string(),
                    r#"c"""#.to_string(),
                    r#"c"baz""#.to_string(),
                    r#"c"""#.to_string(),
                    r#"c"qux""#.to_string(),
                ],
            }),
            Some(b"foobarbazqux\0".to_vec())
        );
    }
}
