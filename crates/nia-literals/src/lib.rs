// SPDX-License-Identifier: GPL-3.0-or-later

pub fn eval_int_literal(text: &str) -> Result<i128, String> {
    parse_int_literal(text)
}

pub fn eval_float_literal(text: &str) -> Result<f64, String> {
    let body = numeric_literal_body(text);
    body.replace('_', "")
        .parse::<f64>()
        .map_err(|_| "invalid float constant".to_string())
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
        .map_err(|_| "integer literal is out of range for const evaluation".to_string())
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

pub fn decode_char_literal(text: &str) -> Option<u32> {
    let inner = text.strip_prefix('\'')?.strip_suffix('\'')?;
    let scalars = decode_scalar_literal_inner(inner)?;
    (scalars.len() == 1).then_some(scalars[0])
}

pub fn decode_byte_char_literal(text: &str) -> Option<u8> {
    let inner = text.strip_prefix("b'")?.strip_suffix('\'')?;
    let bytes = decode_byte_literal_inner(inner)?;
    (bytes.len() == 1).then_some(bytes[0])
}

pub fn eval_string_literal_parts<'a>(parts: impl IntoIterator<Item = &'a str>) -> Option<String> {
    let parts = collect_literal_parts(parts);
    if parts.len() > 1 && parts.iter().any(|part| is_multiline_literal(part)) {
        return None;
    }
    let mut out = String::new();
    for part in parts {
        out.push_str(&decode_string_literal_part(part)?);
    }
    Some(out)
}

pub fn eval_byte_string_literal_parts<'a>(
    parts: impl IntoIterator<Item = &'a str>,
) -> Option<Vec<u8>> {
    let parts = collect_literal_parts(parts);
    if parts.len() > 1 && parts.iter().any(|part| is_multiline_literal(part)) {
        return None;
    }
    let mut bytes = Vec::new();
    for part in parts {
        bytes.extend(decode_byte_string_literal_part(part)?);
    }
    Some(bytes)
}

pub fn decode_string_literal_scalars<'a>(
    parts: impl IntoIterator<Item = &'a str>,
) -> Option<Vec<u32>> {
    eval_string_literal_parts(parts).map(|value| value.chars().map(|ch| ch as u32).collect())
}

pub fn string_literal_char_len<'a>(parts: impl IntoIterator<Item = &'a str>) -> Option<usize> {
    let parts = collect_literal_parts(parts);
    if parts.len() > 1 && parts.iter().any(|part| is_multiline_literal(part)) {
        return None;
    }
    parts.iter().try_fold(0usize, |len, text| {
        len.checked_add(string_literal_part_char_len(text)?)
    })
}

pub fn byte_string_literal_len<'a>(parts: impl IntoIterator<Item = &'a str>) -> Option<usize> {
    let parts = collect_literal_parts(parts);
    if parts.len() > 1 && parts.iter().any(|part| is_multiline_literal(part)) {
        return None;
    }
    parts.iter().try_fold(0usize, |len, text| {
        len.checked_add(byte_string_literal_part_len(text)?)
    })
}

fn collect_literal_parts<'a>(parts: impl IntoIterator<Item = &'a str>) -> Vec<&'a str> {
    parts.into_iter().collect()
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

fn decode_byte_string_literal_part(text: &str) -> Option<Vec<u8>> {
    if is_multiline_literal(text) {
        return decode_multiline_string_literal(text);
    }
    let inner = text
        .strip_prefix("b\"")
        .or_else(|| text.strip_prefix('"'))?
        .strip_suffix('"')?;
    decode_byte_literal_inner(inner)
}

fn string_literal_part_char_len(text: &str) -> Option<usize> {
    if is_multiline_literal(text) {
        return multiline_string_literal_char_len(text);
    }
    let inner = text.strip_prefix('"')?.strip_suffix('"')?;
    decoded_scalar_len(inner)
}

fn byte_string_literal_part_len(text: &str) -> Option<usize> {
    if is_multiline_literal(text) {
        return multiline_string_literal_byte_len(text);
    }
    let inner = text
        .strip_prefix("b\"")
        .or_else(|| text.strip_prefix('"'))?
        .strip_suffix('"')?;
    decoded_byte_len(inner)
}

fn is_multiline_literal(text: &str) -> bool {
    text.strip_prefix('b')
        .or_else(|| text.strip_prefix('c'))
        .unwrap_or(text)
        .starts_with("\\\\")
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

fn decoded_byte_len(inner: &str) -> Option<usize> {
    let mut bytes = 0usize;
    let mut chars = inner.chars();
    while let Some(ch) = chars.next() {
        if ch != '\\' {
            bytes = bytes.checked_add(ch.len_utf8())?;
            continue;
        }
        match chars.next()? {
            'n' | 'r' | 't' | '\\' | '\'' | '"' | '0' => bytes = bytes.checked_add(1)?,
            'x' => {
                chars.next()?.to_digit(16)?;
                chars.next()?.to_digit(16)?;
                bytes = bytes.checked_add(1)?;
            }
            'u' => bytes = bytes.checked_add(unicode_escape_byte_len(&mut chars)?)?,
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
            scalars = scalars.checked_add(1)?;
            continue;
        }
        match chars.next()? {
            'n' | 'r' | 't' | '\\' | '\'' | '"' | '0' => scalars = scalars.checked_add(1)?,
            'x' => {
                let hi = chars.next()?.to_digit(16)?;
                let lo = chars.next()?.to_digit(16)?;
                char::from_u32((hi << 4) | lo)?;
                scalars = scalars.checked_add(1)?;
            }
            'u' => {
                decode_unicode_escape_scalar(&mut chars)?;
                scalars = scalars.checked_add(1)?;
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
        scalars = scalars.checked_add(source[content_start..pos].chars().count())?;

        if pos == source.len() {
            break;
        }
        scalars = scalars.checked_add(1)?;
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
        bytes = bytes.checked_add(source[content_start..pos].len())?;

        if pos == source.len() {
            break;
        }
        bytes = bytes.checked_add(1)?;
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
    fn eval_int_literal_ignores_type_suffix() {
        assert_eq!(eval_int_literal("42i32"), Ok(42));
        assert_eq!(eval_int_literal("0xffu8"), Ok(255));
        assert_eq!(eval_int_literal("1_024usize"), Ok(1024));
    }

    #[test]
    fn eval_float_literal_ignores_type_suffix_and_separators() {
        assert_eq!(eval_float_literal("0.0f64"), Ok(0.0));
        assert_eq!(eval_float_literal("1_024.5f32"), Ok(1024.5));
        assert_eq!(eval_float_literal("1.25e-1f64"), Ok(0.125));
    }

    #[test]
    fn decodes_string_literal_parts() {
        assert_eq!(
            eval_string_literal_parts([r#""he\n""#, r#""llo""#]),
            Some("he\nllo".to_string())
        );
        assert_eq!(
            decode_string_literal_scalars([r#""\u{41}""#]),
            Some(vec!['A' as u32])
        );
    }

    #[test]
    fn decodes_byte_string_literal_parts() {
        assert_eq!(
            eval_byte_string_literal_parts([r#"b"a\x00""#]),
            Some(vec![b'a', 0])
        );
        assert_eq!(byte_string_literal_len([r#"b"a\u{20ac}""#]), Some(4));
    }

    #[test]
    fn counts_multiline_string_literal_scalars() {
        assert_eq!(
            string_literal_char_len(["\\\\hello\n    \\\\world"]),
            Some("hello\nworld".chars().count())
        );
        assert_eq!(
            string_literal_char_len(["\\\\hello\\n"]),
            Some("hello\\n".chars().count())
        );
    }
}
