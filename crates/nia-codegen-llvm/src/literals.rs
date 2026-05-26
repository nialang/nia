// SPDX-License-Identifier: GPL-3.0-or-later
use nia_ast::{AssignOp, BinaryOp};

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

pub(super) fn parse_float_literal(text: &str) -> Option<f64> {
    let text = numeric_literal_body(text);
    text.parse::<f64>().ok()
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
        b'0'..=b'9' => Some((byte - b'0') as u32),
        b'a'..=b'f' => Some((byte - b'a' + 10) as u32),
        b'A'..=b'F' => Some((byte - b'A' + 10) as u32),
        _ => None,
    }
}

pub(super) fn decode_char_literal(text: &str) -> Option<u32> {
    let inner = text.strip_prefix('\'')?.strip_suffix('\'')?;
    let mut chars = inner.chars();
    let ch = match chars.next()? {
        '\\' => match chars.next()? {
            'n' => '\n',
            'r' => '\r',
            't' => '\t',
            '\\' => '\\',
            '\'' => '\'',
            '"' => '"',
            '0' => '\0',
            'x' => {
                let hi = chars.next()?.to_digit(16)?;
                let lo = chars.next()?.to_digit(16)?;
                char::from_u32((hi << 4) | lo)?
            }
            'u' => decode_unicode_escape(&mut chars)?,
            _ => return None,
        },
        ch => ch,
    };
    chars.next().is_none().then_some(ch as u32)
}

pub(super) fn decode_byte_char_literal(text: &str) -> Option<u8> {
    let inner = text.strip_prefix("b'")?.strip_suffix('\'')?;
    let char_text = format!("'{inner}'");
    let value = decode_char_literal(&char_text)?;
    u8::try_from(value).ok()
}

fn decode_unicode_escape(chars: &mut std::str::Chars<'_>) -> Option<char> {
    if chars.next()? != '{' {
        return None;
    }
    let mut value = String::new();
    for ch in chars.by_ref() {
        if ch == '}' {
            let scalar = u32::from_str_radix(&value, 16).ok()?;
            return char::from_u32(scalar);
        }
        value.push(ch);
    }
    None
}

pub(super) fn assign_to_binary_op(op: AssignOp) -> Option<BinaryOp> {
    Some(match op {
        AssignOp::Assign => return None,
        AssignOp::Add => BinaryOp::Add,
        AssignOp::Sub => BinaryOp::Sub,
        AssignOp::Shl => BinaryOp::Shl,
        AssignOp::Shr => BinaryOp::Shr,
        AssignOp::Mul => BinaryOp::Mul,
        AssignOp::Div => BinaryOp::Div,
        AssignOp::Rem => BinaryOp::Rem,
        AssignOp::BitAnd => BinaryOp::BitAnd,
        AssignOp::BitXor => BinaryOp::BitXor,
        AssignOp::BitOr => BinaryOp::BitOr,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rejects_unclosed_unicode_char_escape() {
        assert_eq!(decode_char_literal(r#"'\u{41'"#), None);
    }
}
