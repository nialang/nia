use crate::ComptimeValue;

use nia_comptime_ir::ComptimeStringLiteral;
pub(crate) use nia_literals::{decode_byte_char_literal, decode_char_literal};
pub use nia_literals::{eval_float_literal, eval_int_literal};
use nia_ty::IntConst;

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
    nia_literals::eval_string_literal_parts(literal.parts.iter().map(String::as_str))
}

pub fn eval_byte_string_literal(literal: &ComptimeStringLiteral) -> Option<Vec<u8>> {
    nia_literals::eval_byte_string_literal_parts(literal.parts.iter().map(String::as_str))
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
