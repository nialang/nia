use crate::ConstValue;

use nia_const_ir::ConstStringLiteral;
pub(crate) use nia_literals::{decode_byte_char_literal, decode_char_literal};
pub use nia_literals::{eval_float_literal, eval_int_literal};
use nia_ty::IntConst;

pub(crate) fn string_to_char_array(value: &str) -> Vec<ConstValue> {
    value
        .chars()
        .map(|ch| ConstValue::Int(IntConst::unsigned(ch as u128)))
        .collect()
}

pub(crate) fn bytes_to_array(value: &[u8]) -> Vec<ConstValue> {
    value
        .iter()
        .map(|byte| ConstValue::Int(IntConst::unsigned(u128::from(*byte))))
        .collect()
}

pub(crate) fn char_array_to_string(values: &[ConstValue]) -> Option<String> {
    let mut out = String::new();
    for value in values {
        let ConstValue::Int(value) = value else {
            return None;
        };
        let value = u32::try_from(value.bits()).ok()?;
        out.push(char::from_u32(value)?);
    }
    Some(out)
}

pub(crate) fn const_error_message(value: &ConstValue) -> Option<String> {
    match value {
        ConstValue::String(value) => Some(value.clone()),
        ConstValue::Array(values) => char_array_to_string(values),
        ConstValue::Pointer(crate::ConstPointerValue::Frozen { pointee, .. }) => {
            const_error_message(pointee)
        }
        ConstValue::Pointer(crate::ConstPointerValue::Place { .. }) => None,
        _ => None,
    }
}

/// Decodes a segmented string literal, rejecting malformed escapes.
pub fn eval_string_literal(literal: &ConstStringLiteral) -> Option<String> {
    nia_literals::eval_string_literal_parts(literal.parts.iter().map(String::as_str))
}

/// Decodes a segmented byte-string literal, rejecting malformed escapes.
pub fn eval_byte_string_literal(literal: &ConstStringLiteral) -> Option<Vec<u8>> {
    nia_literals::eval_byte_string_literal_parts(literal.parts.iter().map(String::as_str))
}

pub(crate) fn checked_shift(lhs: i128, rhs: i128, is_left: bool) -> Result<i128, String> {
    let Ok(rhs) = u32::try_from(rhs) else {
        return Err("shift count is out of range in const expression".to_string());
    };
    if rhs >= i128::BITS {
        return Err("shift count is out of range in const expression".to_string());
    }
    if is_left {
        checked_signed_left_shift(lhs, rhs, i128::BITS)
            .ok_or_else(|| "integer overflow in const left shift".to_string())
    } else {
        lhs.checked_shr(rhs)
            .ok_or_else(|| "integer overflow in const right shift".to_string())
    }
}

pub(crate) fn checked_shift_u128(lhs: u128, rhs: u128, is_left: bool) -> Result<u128, String> {
    let Ok(rhs) = u32::try_from(rhs) else {
        return Err("shift count is out of range in const expression".to_string());
    };
    if rhs >= u128::BITS {
        return Err("shift count is out of range in const expression".to_string());
    }
    if is_left {
        checked_unsigned_left_shift(lhs, rhs, u128::BITS)
            .ok_or_else(|| "integer overflow in const left shift".to_string())
    } else {
        lhs.checked_shr(rhs)
            .ok_or_else(|| "integer overflow in const right shift".to_string())
    }
}

/// Computes a signed left shift only when the mathematical result fits `bits`.
/// Rust's `checked_shl` validates the count but still discards high bits, so it
/// cannot implement Nia's trapping overflow semantics by itself.
pub(crate) fn checked_signed_left_shift(lhs: i128, rhs: u32, bits: u32) -> Option<i128> {
    if !(1..=i128::BITS).contains(&bits) || rhs >= bits {
        return None;
    }
    let negative_limit = 1_u128 << (bits - 1);
    let magnitude_limit = if lhs < 0 {
        negative_limit
    } else {
        negative_limit - 1
    } >> rhs;
    if lhs.unsigned_abs() > magnitude_limit {
        return None;
    }
    Some(lhs.wrapping_shl(rhs))
}

/// Computes an unsigned left shift only when the mathematical result fits
/// `bits`; no host-width shift can wrap into an apparently valid narrow value.
pub(crate) fn checked_unsigned_left_shift(lhs: u128, rhs: u32, bits: u32) -> Option<u128> {
    if !(1..=u128::BITS).contains(&bits) || rhs >= bits {
        return None;
    }
    let mask = if bits == u128::BITS {
        u128::MAX
    } else {
        (1_u128 << bits) - 1
    };
    if lhs > mask || lhs > (mask >> rhs) {
        return None;
    }
    Some(lhs << rhs)
}
