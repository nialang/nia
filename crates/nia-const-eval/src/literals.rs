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

pub fn eval_string_literal(literal: &ConstStringLiteral) -> Option<String> {
    nia_literals::eval_string_literal_parts(literal.parts.iter().map(String::as_str))
}

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
        lhs.checked_shl(rhs)
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
        lhs.checked_shl(rhs)
            .ok_or_else(|| "integer overflow in const left shift".to_string())
    } else {
        lhs.checked_shr(rhs)
            .ok_or_else(|| "integer overflow in const right shift".to_string())
    }
}
