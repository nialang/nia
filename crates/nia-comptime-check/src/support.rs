use std::collections::HashSet;

use nia_comptime_engine::ComptimeValue;
use nia_diagnostic::Diagnostic;
use nia_span::Span;
use nia_ty::{IntConst, PrimitiveTy};

pub(crate) fn validate_assignment_shape(
    diagnostics: &mut Vec<Diagnostic>,
    span: Span,
    value: &ComptimeValue,
    previous: &ComptimeValue,
) {
    match (value, previous) {
        (ComptimeValue::Array(values), ComptimeValue::Array(previous_values)) => {
            if values.len() != previous_values.len() {
                diagnostics.push(Diagnostic::user_error_at(
                    "E0401",
                    span,
                    format!(
                        "comptime array length {} does not match expected length {}",
                        values.len(),
                        previous_values.len()
                    ),
                ));
            }
            for (value, previous) in values.iter().zip(previous_values) {
                validate_assignment_shape(diagnostics, span, value, previous);
            }
        }
        (ComptimeValue::Struct(values), ComptimeValue::Struct(previous_values)) => {
            let previous_names = previous_values
                .keys()
                .map(String::as_str)
                .collect::<HashSet<_>>();
            for (name, previous) in previous_values {
                if let Some(value) = values.get(name) {
                    validate_assignment_shape(diagnostics, span, value, previous);
                } else {
                    diagnostics.push(Diagnostic::user_error_at(
                        "E0401",
                        span,
                        format!("comptime struct value is missing field `{name}`"),
                    ));
                }
            }
            for name in values.keys() {
                if !previous_names.contains(name.as_str()) {
                    diagnostics.push(Diagnostic::user_error_at(
                        "E0401",
                        span,
                        format!("comptime struct value has extra field `{name}`"),
                    ));
                }
            }
        }
        (ComptimeValue::Optional(Some(value)), ComptimeValue::Optional(Some(previous))) => {
            validate_assignment_shape(diagnostics, span, value, previous);
        }
        (ComptimeValue::ErrorUnion(Ok(value)), ComptimeValue::ErrorUnion(Ok(previous)))
        | (ComptimeValue::ErrorUnion(Err(value)), ComptimeValue::ErrorUnion(Err(previous))) => {
            validate_assignment_shape(diagnostics, span, value, previous);
        }
        _ => {}
    }
}

pub(crate) fn comptime_string_to_char_array(value: &str) -> Vec<ComptimeValue> {
    value
        .chars()
        .map(|value| ComptimeValue::Int(IntConst::unsigned(value as u128)))
        .collect()
}

pub(crate) fn int_const_in_i128_range(value: IntConst, min: i128, max: i128) -> bool {
    value
        .as_i128()
        .is_some_and(|value| value >= min && value <= max)
}

pub(crate) fn enum_next_value(value: IntConst) -> IntConst {
    if value.is_signed() {
        value
            .as_i128()
            .and_then(|value| value.checked_add(1))
            .map(IntConst::from_i128)
            .unwrap_or(value)
    } else {
        value
            .bits()
            .checked_add(1)
            .map(IntConst::unsigned)
            .unwrap_or(value)
    }
}

pub(crate) fn cast_comptime_integer(
    value: IntConst,
    ty: PrimitiveTy,
    pointer_width: u32,
) -> Option<IntConst> {
    let (bits, signed) = primitive_integer_layout(ty, pointer_width)?;
    let mask = integer_mask(bits)?;
    let raw = value.bits() & mask;
    if signed {
        Some(IntConst::from_i128(sign_extend_integer(raw, bits)))
    } else {
        Some(IntConst::unsigned(raw))
    }
}

pub(crate) fn cast_int_to_float(value: IntConst, ty: PrimitiveTy) -> Option<f64> {
    let value = if let Some(value) = value.as_i128() {
        value as f64
    } else {
        value.bits() as f64
    };
    cast_float_to_float(value, ty)
}

pub(crate) fn cast_float_to_float(value: f64, ty: PrimitiveTy) -> Option<f64> {
    let value = match ty {
        PrimitiveTy::F32 => f64::from(value as f32),
        PrimitiveTy::F64 => value,
        _ => value,
    };
    value.is_finite().then_some(value)
}

pub(crate) fn cast_float_to_integer(
    value: f64,
    ty: PrimitiveTy,
    pointer_width: u32,
) -> Option<i128> {
    if !value.is_finite() {
        return None;
    }
    let (min, max) = primitive_integer_range_for_target(ty, pointer_width)?;
    if value < min as f64 || value > max as f64 {
        return None;
    }
    Some(value.trunc() as i128)
}

pub(crate) fn primitive_integer_range_for_target(
    ty: PrimitiveTy,
    pointer_width: u32,
) -> Option<(i128, i128)> {
    match ty {
        PrimitiveTy::Isize => signed_integer_range(pointer_width),
        PrimitiveTy::Usize => unsigned_integer_range(pointer_width),
        _ => integer_range(ty),
    }
}

fn signed_integer_range(bits: u32) -> Option<(i128, i128)> {
    match bits {
        0 => None,
        1..=127 => Some((-(1i128 << (bits - 1)), (1i128 << (bits - 1)) - 1)),
        128 => Some((i128::MIN, i128::MAX)),
        _ => None,
    }
}

fn unsigned_integer_range(bits: u32) -> Option<(i128, i128)> {
    match bits {
        0 => None,
        1..=126 => Some((0, (1i128 << bits) - 1)),
        127 | 128 => Some((0, i128::MAX)),
        _ => None,
    }
}

pub(crate) fn primitive_integer_layout(ty: PrimitiveTy, pointer_width: u32) -> Option<(u32, bool)> {
    match ty {
        PrimitiveTy::I8 => Some((8, true)),
        PrimitiveTy::I16 => Some((16, true)),
        PrimitiveTy::I32 => Some((32, true)),
        PrimitiveTy::I64 => Some((64, true)),
        PrimitiveTy::I128 => Some((128, true)),
        PrimitiveTy::Isize => Some((pointer_width, true)),
        PrimitiveTy::U8 => Some((8, false)),
        PrimitiveTy::U16 => Some((16, false)),
        PrimitiveTy::U32 => Some((32, false)),
        PrimitiveTy::U64 => Some((64, false)),
        PrimitiveTy::U128 => Some((128, false)),
        PrimitiveTy::Usize => Some((pointer_width, false)),
        PrimitiveTy::Char => Some((32, false)),
        PrimitiveTy::Bool
        | PrimitiveTy::F32
        | PrimitiveTy::F64
        | PrimitiveTy::Void
        | PrimitiveTy::Never => None,
    }
}

pub(crate) fn is_float_primitive(ty: PrimitiveTy) -> bool {
    matches!(ty, PrimitiveTy::F32 | PrimitiveTy::F64)
}

fn integer_mask(bits: u32) -> Option<u128> {
    match bits {
        0 => None,
        1..=127 => Some((1u128 << bits) - 1),
        128 => Some(u128::MAX),
        _ => None,
    }
}

fn sign_extend_integer(raw: u128, bits: u32) -> i128 {
    if bits == 128 {
        return raw as i128;
    }
    let sign_bit = 1u128 << (bits - 1);
    if raw & sign_bit == 0 {
        raw as i128
    } else {
        (raw as i128) - (1i128 << bits)
    }
}

pub(crate) fn integer_range(ty: PrimitiveTy) -> Option<(i128, i128)> {
    match ty {
        PrimitiveTy::I8 => Some((i8::MIN as i128, i8::MAX as i128)),
        PrimitiveTy::I16 => Some((i16::MIN as i128, i16::MAX as i128)),
        PrimitiveTy::I32 => Some((i32::MIN as i128, i32::MAX as i128)),
        PrimitiveTy::I64 => Some((i64::MIN as i128, i64::MAX as i128)),
        PrimitiveTy::I128 => Some((i128::MIN, i128::MAX)),
        PrimitiveTy::Isize => Some((isize::MIN as i128, isize::MAX as i128)),
        PrimitiveTy::U8 => Some((u8::MIN as i128, u8::MAX as i128)),
        PrimitiveTy::U16 => Some((u16::MIN as i128, u16::MAX as i128)),
        PrimitiveTy::U32 => Some((u32::MIN as i128, u32::MAX as i128)),
        PrimitiveTy::U64 => Some((u64::MIN as i128, u64::MAX as i128)),
        PrimitiveTy::U128 => Some((0, i128::MAX)),
        PrimitiveTy::Usize => Some((0, usize::MAX as i128)),
        PrimitiveTy::Char => Some((0, 0x10FFFF)),
        PrimitiveTy::Bool
        | PrimitiveTy::F32
        | PrimitiveTy::F64
        | PrimitiveTy::Void
        | PrimitiveTy::Never => None,
    }
}

pub(crate) fn integer_literal_suffix_ty(text: &str) -> Option<PrimitiveTy> {
    let primitive = PrimitiveTy::from_name(numeric_literal_suffix(text)?)?;
    primitive.is_integer().then_some(primitive)
}

pub(crate) fn float_literal_suffix_ty(text: &str) -> Option<PrimitiveTy> {
    let primitive = PrimitiveTy::from_name(numeric_literal_suffix(text)?)?;
    primitive.is_float().then_some(primitive)
}

fn numeric_literal_suffix(text: &str) -> Option<&str> {
    let non_decimal_radix = text.starts_with("0x")
        || text.starts_with("0X")
        || text.starts_with("0b")
        || text.starts_with("0B")
        || text.starts_with("0o")
        || text.starts_with("0O");
    let mut index = if non_decimal_radix { 2 } else { 0 };
    let bytes = text.as_bytes();
    while index < bytes.len() {
        let byte = bytes[index];
        if byte == b'_'
            || if non_decimal_radix {
                byte.is_ascii_hexdigit()
            } else {
                byte.is_ascii_digit()
            }
        {
            index += 1;
        } else {
            break;
        }
    }
    (index < bytes.len()).then_some(&text[index..])
}
