// SPDX-License-Identifier: GPL-3.0-or-later
use nia_ast::{AssignOp, BinaryOp};

pub(super) fn parse_int_literal(text: &str) -> Option<nia_ty::IntConst> {
    nia_literals::eval_int_literal(text)
        .ok()
        .map(nia_ty::IntConst::unsigned)
        .or_else(|| {
            // Source integer tokens never include a leading sign, but function
            // lowering synthesizes signed decimal text for checked match patterns.
            // Parse that internal form without teaching the source literal parser
            // to accept syntax that remains a unary expression in the language.
            text.strip_prefix('-')?;
            text.parse().ok().map(nia_ty::IntConst::signed)
        })
}

pub(super) fn parse_float_literal(text: &str) -> Option<f64> {
    nia_literals::eval_float_literal(text).ok().or_else(|| {
        // As with checked integer patterns above, source tokens keep the sign
        // as a unary expression. Static lowering folds that unary operation
        // into an internal signed spelling so backend validation and emission
        // must accept exactly that additional form.
        let magnitude = text.strip_prefix('-')?.parse::<f64>().ok()?;
        Some(-magnitude)
    })
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

    #[test]
    fn parses_lowered_negative_integer_pattern() {
        assert_eq!(parse_int_literal("-1"), Some(nia_ty::IntConst::signed(-1)));
        assert_eq!(
            parse_int_literal(&i128::MIN.to_string()),
            Some(nia_ty::IntConst::signed(i128::MIN))
        );
    }
}
