// SPDX-License-Identifier: GPL-3.0-or-later
pub(super) fn parse_int_literal(text: &str) -> Option<i128> {
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

pub(super) fn decode_byte_char(text: &str) -> Option<u8> {
    let inner = text.strip_prefix("b'")?.strip_suffix('\'')?;
    decode_char_inner(inner).and_then(|bytes| (bytes.len() == 1).then_some(bytes[0]))
}

pub(super) fn decode_string_literal(text: &str) -> Option<Vec<u8>> {
    if text.starts_with("\\\\") {
        return decode_multiline_string_literal(text);
    }
    decode_quoted(text, '"', '"')
}

fn decode_multiline_string_literal(text: &str) -> Option<Vec<u8>> {
    let mut bytes = Vec::new();
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

fn decode_quoted(text: &str, prefix: char, suffix: char) -> Option<Vec<u8>> {
    let inner = text.strip_prefix(prefix)?.strip_suffix(suffix)?;
    decode_char_inner(inner)
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
        assert_eq!(decode_string_literal(r#""\u{41""#), None);
    }

    #[test]
    fn decodes_multiline_string_literal_without_escapes_or_trailing_newline() {
        assert_eq!(
            decode_string_literal("\\\\mov rax, 60\n    \\\\syscall"),
            Some(b"mov rax, 60\nsyscall".to_vec())
        );
        assert_eq!(
            decode_string_literal("\\\\hello\\n\n\\\\world"),
            Some(b"hello\\n\nworld".to_vec())
        );
    }
}
