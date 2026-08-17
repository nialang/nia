//! Pure numeric and equality semantics for const evaluation.

use std::cmp::Ordering;

use crate::{ConstEnumPayload, ConstError, ConstIntegerSemantics, ConstValue};
use nia_const_ir::ConstBinaryOp;
use nia_span::Span;
use nia_ty::IntConst;

use crate::literals::{char_array_to_string, checked_shift, checked_shift_u128};

pub(super) fn const_bit_not(value: IntConst) -> IntConst {
    if value.is_signed() {
        IntConst::from_i128(!value.as_i128().unwrap_or(value.bits() as i128))
    } else {
        IntConst::unsigned(!value.bits())
    }
}

/// Applies bitwise not in the expression's concrete integer width.
///
/// `IntConst` can carry all 128 bits independently of its source type, so the
/// result must be masked before signed two's-complement reconstruction.
pub(super) fn const_typed_bit_not(
    value: IntConst,
    semantics: Option<ConstIntegerSemantics>,
) -> Result<IntConst, String> {
    let Some(semantics) = semantics else {
        return Ok(const_bit_not(value));
    };
    let mask = match semantics.bits {
        1..=127 => (1u128 << semantics.bits) - 1,
        128 => u128::MAX,
        _ => return Err("invalid integer width in const bitwise not semantics".to_string()),
    };
    let bits = !value.bits() & mask;
    if semantics.signed {
        // Shift-based sign extension works for every width through 128. It
        // avoids constructing positive `2^127`, which is not representable by
        // `i128` and made the previous subtraction overflow at 127 bits.
        let shift = 128 - semantics.bits;
        Ok(IntConst::from_i128(((bits << shift) as i128) >> shift))
    } else {
        Ok(IntConst::unsigned(bits))
    }
}

pub(super) fn int_to_array_len(span: Span, value: IntConst) -> Result<u64, ConstError> {
    let Some(value) = value.as_i128() else {
        return Err(ConstError {
            span,
            message: "array length is too large".to_string(),
        });
    };
    if value < 0 {
        return Err(ConstError {
            span,
            message: "array length must be non-negative".to_string(),
        });
    }
    u64::try_from(value).map_err(|_| ConstError {
        span,
        message: "array length is too large".to_string(),
    })
}

fn int_to_i128(value: IntConst, context: &str) -> Result<i128, String> {
    value
        .as_i128()
        .ok_or_else(|| format!("integer value is too large for {context}"))
}

pub(super) fn eval_binary_int(
    lhs: IntConst,
    op: ConstBinaryOp,
    rhs: IntConst,
) -> Result<IntConst, String> {
    if !lhs.is_signed() && !rhs.is_signed() {
        return eval_binary_uint(lhs.bits(), op, rhs.bits()).map(IntConst::unsigned);
    }
    let lhs = int_to_i128(lhs, "const operation")?;
    let rhs = int_to_i128(rhs, "const operation")?;
    Ok(match op {
        ConstBinaryOp::Mul => lhs
            .checked_mul(rhs)
            .ok_or_else(|| "integer overflow in const multiplication".to_string())?,
        ConstBinaryOp::Div => {
            if rhs == 0 {
                return Err("division by zero in const expression".to_string());
            }
            lhs.checked_div(rhs)
                .ok_or_else(|| "integer overflow in const division".to_string())?
        }
        ConstBinaryOp::Rem => {
            if rhs == 0 {
                return Err("remainder by zero in const expression".to_string());
            }
            lhs.checked_rem(rhs)
                .ok_or_else(|| "integer overflow in const remainder".to_string())?
        }
        ConstBinaryOp::Add => lhs
            .checked_add(rhs)
            .ok_or_else(|| "integer overflow in const addition".to_string())?,
        ConstBinaryOp::Sub => lhs
            .checked_sub(rhs)
            .ok_or_else(|| "integer overflow in const subtraction".to_string())?,
        ConstBinaryOp::Shl => checked_shift(lhs, rhs, true)?,
        ConstBinaryOp::Shr => checked_shift(lhs, rhs, false)?,
        ConstBinaryOp::BitAnd => lhs & rhs,
        ConstBinaryOp::BitXor => lhs ^ rhs,
        ConstBinaryOp::BitOr => lhs | rhs,
        _ => {
            return Err(format!(
                "unsupported binary operator in const expression: {op:?}"
            ));
        }
    }
    .into())
}

/// Evaluates an integer operation under the resolved source type's width.
///
/// Host `i128`/`u128` checked operations only detect overflow at 128 bits. The
/// explicit fit check is what preserves Nia's narrower integer semantics at
/// compile time and keeps const evaluation aligned with runtime traps.
pub(super) fn eval_typed_binary_int(
    lhs: IntConst,
    op: ConstBinaryOp,
    rhs: IntConst,
    semantics: ConstIntegerSemantics,
) -> Result<IntConst, String> {
    if !(1..=128).contains(&semantics.bits) {
        return Err("invalid integer width in const operation semantics".to_string());
    }
    if semantics.signed {
        let lhs = int_to_i128(lhs, "const operation")?;
        let rhs = int_to_i128(rhs, "const operation")?;
        let value = match op {
            ConstBinaryOp::Mul => lhs.checked_mul(rhs),
            ConstBinaryOp::Div if rhs == 0 => {
                return Err("division by zero in const expression".to_string());
            }
            ConstBinaryOp::Div => lhs.checked_div(rhs),
            ConstBinaryOp::Rem if rhs == 0 => {
                return Err("remainder by zero in const expression".to_string());
            }
            ConstBinaryOp::Rem => lhs.checked_rem(rhs),
            ConstBinaryOp::Add => lhs.checked_add(rhs),
            ConstBinaryOp::Sub => lhs.checked_sub(rhs),
            ConstBinaryOp::Shl => {
                let count = typed_shift_count(rhs, semantics.bits)?;
                lhs.checked_shl(count)
            }
            ConstBinaryOp::Shr => {
                let count = typed_shift_count(rhs, semantics.bits)?;
                return Ok(IntConst::from_i128(lhs >> count));
            }
            _ => return eval_binary_int(IntConst::from_i128(lhs), op, IntConst::from_i128(rhs)),
        }
        .filter(|value| signed_value_fits(*value, semantics.bits))
        .ok_or_else(|| integer_overflow_message(op))?;
        Ok(IntConst::from_i128(value))
    } else {
        let lhs = int_to_u128(lhs, "const operation")?;
        let rhs = int_to_u128(rhs, "const operation")?;
        let value = match op {
            ConstBinaryOp::Mul => lhs.checked_mul(rhs),
            ConstBinaryOp::Div if rhs == 0 => {
                return Err("division by zero in const expression".to_string());
            }
            ConstBinaryOp::Div => Some(lhs / rhs),
            ConstBinaryOp::Rem if rhs == 0 => {
                return Err("remainder by zero in const expression".to_string());
            }
            ConstBinaryOp::Rem => Some(lhs % rhs),
            ConstBinaryOp::Add => lhs.checked_add(rhs),
            ConstBinaryOp::Sub => lhs.checked_sub(rhs),
            ConstBinaryOp::Shl => {
                let count = typed_shift_count_u128(rhs, semantics.bits)?;
                lhs.checked_shl(count)
            }
            ConstBinaryOp::Shr => {
                let count = typed_shift_count_u128(rhs, semantics.bits)?;
                return Ok(IntConst::unsigned(lhs >> count));
            }
            _ => return eval_binary_uint(lhs, op, rhs).map(IntConst::unsigned),
        }
        .filter(|value| unsigned_value_fits(*value, semantics.bits))
        .ok_or_else(|| integer_overflow_message(op))?;
        Ok(IntConst::unsigned(value))
    }
}

fn int_to_u128(value: IntConst, context: &str) -> Result<u128, String> {
    if value.is_signed() {
        return value
            .as_i128()
            .and_then(|value| u128::try_from(value).ok())
            .ok_or_else(|| format!("integer value is negative in unsigned {context}"));
    }
    Ok(value.bits())
}

fn typed_shift_count(value: i128, bits: u32) -> Result<u32, String> {
    u32::try_from(value)
        .ok()
        .filter(|count| *count < bits)
        .ok_or_else(|| "shift count is out of range in const expression".to_string())
}

fn typed_shift_count_u128(value: u128, bits: u32) -> Result<u32, String> {
    u32::try_from(value)
        .ok()
        .filter(|count| *count < bits)
        .ok_or_else(|| "shift count is out of range in const expression".to_string())
}

pub(super) fn eval_typed_int_neg(
    value: IntConst,
    semantics: Option<ConstIntegerSemantics>,
) -> Result<IntConst, String> {
    let value = value
        .as_i128()
        .and_then(i128::checked_neg)
        .ok_or_else(|| "integer overflow in const negation".to_string())?;
    if semantics
        .is_some_and(|semantics| !semantics.signed || !signed_value_fits(value, semantics.bits))
    {
        return Err("integer overflow in const negation".to_string());
    }
    Ok(IntConst::from_i128(value))
}

fn signed_value_fits(value: i128, bits: u32) -> bool {
    match bits {
        1..=127 => {
            let magnitude = 1i128 << (bits - 1);
            value >= -magnitude && value < magnitude
        }
        128 => true,
        _ => false,
    }
}

fn unsigned_value_fits(value: u128, bits: u32) -> bool {
    match bits {
        1..=127 => value < (1u128 << bits),
        128 => true,
        _ => false,
    }
}

fn integer_overflow_message(op: ConstBinaryOp) -> String {
    let operation = match op {
        ConstBinaryOp::Mul => "multiplication",
        ConstBinaryOp::Div => "division",
        ConstBinaryOp::Rem => "remainder",
        ConstBinaryOp::Add => "addition",
        ConstBinaryOp::Sub => "subtraction",
        ConstBinaryOp::Shl => "left shift",
        _ => "operation",
    };
    format!("integer overflow in const {operation}")
}

fn eval_binary_uint(lhs: u128, op: ConstBinaryOp, rhs: u128) -> Result<u128, String> {
    Ok(match op {
        ConstBinaryOp::Mul => lhs
            .checked_mul(rhs)
            .ok_or_else(|| "integer overflow in const multiplication".to_string())?,
        ConstBinaryOp::Div => {
            if rhs == 0 {
                return Err("division by zero in const expression".to_string());
            }
            lhs / rhs
        }
        ConstBinaryOp::Rem => {
            if rhs == 0 {
                return Err("remainder by zero in const expression".to_string());
            }
            lhs % rhs
        }
        ConstBinaryOp::Add => lhs
            .checked_add(rhs)
            .ok_or_else(|| "integer overflow in const addition".to_string())?,
        ConstBinaryOp::Sub => lhs
            .checked_sub(rhs)
            .ok_or_else(|| "integer overflow in const subtraction".to_string())?,
        ConstBinaryOp::Shl => checked_shift_u128(lhs, rhs, true)?,
        ConstBinaryOp::Shr => checked_shift_u128(lhs, rhs, false)?,
        ConstBinaryOp::BitAnd => lhs & rhs,
        ConstBinaryOp::BitXor => lhs ^ rhs,
        ConstBinaryOp::BitOr => lhs | rhs,
        _ => {
            return Err(format!(
                "unsupported binary operator in const expression: {op:?}"
            ));
        }
    })
}

pub(super) fn eval_numeric_binary_value(
    lhs: ConstValue,
    op: ConstBinaryOp,
    rhs: ConstValue,
) -> Result<ConstValue, String> {
    match (lhs, rhs) {
        (ConstValue::Int(lhs), ConstValue::Int(rhs)) => {
            eval_binary_int(lhs, op, rhs).map(ConstValue::Int)
        }
        (ConstValue::Float(lhs), ConstValue::Float(rhs)) => eval_binary_float(lhs, op, rhs),
        _ => Err("const numeric operation requires matching operand types".to_string()),
    }
}

pub(super) fn eval_binary_int_compare(lhs: IntConst, op: ConstBinaryOp, rhs: IntConst) -> bool {
    let ordering = compare_int_values(lhs, rhs);
    match op {
        ConstBinaryOp::Lt => ordering == Ordering::Less,
        ConstBinaryOp::Le => ordering != Ordering::Greater,
        ConstBinaryOp::Gt => ordering == Ordering::Greater,
        ConstBinaryOp::Ge => ordering != Ordering::Less,
        _ => unreachable!("non-comparison binary operator routed to integer comparison"),
    }
}

/// Orders the complete mathematical values without narrowing unsigned inputs.
fn compare_int_values(lhs: IntConst, rhs: IntConst) -> Ordering {
    match (lhs.is_signed(), rhs.is_signed()) {
        (false, false) => lhs.bits().cmp(&rhs.bits()),
        (true, true) => (lhs.bits() as i128).cmp(&(rhs.bits() as i128)),
        (true, false) => {
            let lhs = lhs.bits() as i128;
            if lhs < 0 {
                Ordering::Less
            } else {
                (lhs as u128).cmp(&rhs.bits())
            }
        }
        (false, true) => {
            let rhs = rhs.bits() as i128;
            if rhs < 0 {
                Ordering::Greater
            } else {
                lhs.bits().cmp(&(rhs as u128))
            }
        }
    }
}

pub(super) fn eval_binary_float(
    lhs: f64,
    op: ConstBinaryOp,
    rhs: f64,
) -> Result<ConstValue, String> {
    Ok(match op {
        ConstBinaryOp::Add => ConstValue::Float(lhs + rhs),
        ConstBinaryOp::Sub => ConstValue::Float(lhs - rhs),
        ConstBinaryOp::Mul => ConstValue::Float(lhs * rhs),
        ConstBinaryOp::Div => ConstValue::Float(lhs / rhs),
        ConstBinaryOp::Rem => ConstValue::Float(lhs % rhs),
        ConstBinaryOp::Lt => ConstValue::Bool(lhs < rhs),
        ConstBinaryOp::Le => ConstValue::Bool(lhs <= rhs),
        ConstBinaryOp::Gt => ConstValue::Bool(lhs > rhs),
        ConstBinaryOp::Ge => ConstValue::Bool(lhs >= rhs),
        _ => {
            return Err(format!(
                "unsupported binary operator for float const expression: {op:?}"
            ));
        }
    })
}

/// Returns `None` when values do not share a const-comparable shape.
pub(super) fn values_equal(lhs: &ConstValue, rhs: &ConstValue) -> Option<bool> {
    match (lhs, rhs) {
        (ConstValue::Int(lhs), ConstValue::Int(rhs)) => Some(int_values_equal(*lhs, *rhs)),
        (ConstValue::Float(lhs), ConstValue::Float(rhs)) => Some(lhs == rhs),
        (ConstValue::Bool(lhs), ConstValue::Bool(rhs)) => Some(lhs == rhs),
        (ConstValue::String(lhs), ConstValue::String(rhs)) => Some(lhs == rhs),
        (ConstValue::Pointer(lhs), ConstValue::Pointer(rhs)) => Some(lhs == rhs),
        (ConstValue::String(lhs), ConstValue::Array(rhs)) => {
            Some(char_array_to_string(rhs)? == *lhs)
        }
        (ConstValue::Array(lhs), ConstValue::String(rhs)) => {
            Some(char_array_to_string(lhs)? == *rhs)
        }
        (ConstValue::Range(lhs), ConstValue::Range(rhs)) => Some(lhs == rhs),
        (ConstValue::Array(lhs), ConstValue::Array(rhs))
        | (ConstValue::Tuple(lhs), ConstValue::Tuple(rhs)) => sequence_values_equal(lhs, rhs),
        (
            ConstValue::Enum {
                variant: lhs_variant,
                payload: lhs_payload,
            },
            ConstValue::Enum {
                variant: rhs_variant,
                payload: rhs_payload,
            },
        ) => {
            if lhs_variant != rhs_variant {
                return Some(false);
            }
            enum_payloads_equal(lhs_payload, rhs_payload)
        }
        (ConstValue::Optional(lhs), ConstValue::Optional(rhs)) => match (lhs, rhs) {
            (None, None) => Some(true),
            (Some(lhs), Some(rhs)) => values_equal(lhs, rhs),
            _ => Some(false),
        },
        (ConstValue::ErrorUnion(lhs), ConstValue::ErrorUnion(rhs)) => match (lhs, rhs) {
            (Ok(lhs), Ok(rhs)) | (Err(lhs), Err(rhs)) => values_equal(lhs, rhs),
            _ => Some(false),
        },
        _ => None,
    }
}

fn sequence_values_equal(lhs: &[ConstValue], rhs: &[ConstValue]) -> Option<bool> {
    if lhs.len() != rhs.len() {
        return Some(false);
    }
    lhs.iter().zip(rhs).try_fold(true, |equal, (lhs, rhs)| {
        Some(equal && values_equal(lhs, rhs)?)
    })
}

fn int_values_equal(lhs: IntConst, rhs: IntConst) -> bool {
    match (lhs.as_i128(), rhs.as_i128()) {
        (Some(lhs), Some(rhs)) => lhs == rhs,
        (None, None) => lhs.bits() == rhs.bits(),
        (Some(_), None) | (None, Some(_)) => false,
    }
}

fn enum_payloads_equal(lhs: &ConstEnumPayload, rhs: &ConstEnumPayload) -> Option<bool> {
    match (lhs, rhs) {
        (ConstEnumPayload::Unit, ConstEnumPayload::Unit) => Some(true),
        (ConstEnumPayload::Tuple(lhs), ConstEnumPayload::Tuple(rhs)) => {
            sequence_values_equal(lhs, rhs)
        }
        (ConstEnumPayload::Named(lhs), ConstEnumPayload::Named(rhs)) => {
            if lhs.len() != rhs.len() {
                return Some(false);
            }
            lhs.iter().try_fold(true, |equal, (name, lhs)| {
                let rhs = rhs.get(name)?;
                Some(equal && values_equal(lhs, rhs)?)
            })
        }
        _ => Some(false),
    }
}

#[cfg(test)]
mod tests {
    use std::collections::BTreeMap;

    use nia_ids::{DefId, GlobalDefId, ModuleIdAllocator};
    use nia_symbol::SymbolId;

    use super::*;

    fn int(value: i128) -> ConstValue {
        ConstValue::Int(IntConst::from_i128(value))
    }

    #[test]
    fn typed_bit_not_sign_extends_127_bit_results_without_overflow() {
        let value = const_typed_bit_not(
            IntConst::unsigned(0),
            Some(ConstIntegerSemantics {
                bits: 127,
                signed: true,
            }),
        )
        .expect("127-bit bitwise not");

        assert_eq!(value.as_i128(), Some(-1));
    }

    #[test]
    fn typed_bit_not_rejects_invalid_integer_widths() {
        assert_eq!(
            const_typed_bit_not(
                IntConst::unsigned(0),
                Some(ConstIntegerSemantics {
                    bits: 0,
                    signed: false,
                }),
            ),
            Err("invalid integer width in const bitwise not semantics".to_string())
        );
    }

    #[test]
    fn integer_comparison_does_not_narrow_large_unsigned_values() {
        let huge = IntConst::unsigned(u128::MAX);
        let signed_max = IntConst::from_i128(i128::MAX);
        let negative = IntConst::from_i128(-1);

        assert!(eval_binary_int_compare(huge, ConstBinaryOp::Gt, signed_max));
        assert!(eval_binary_int_compare(huge, ConstBinaryOp::Gt, negative));
        assert!(eval_binary_int_compare(negative, ConstBinaryOp::Lt, huge));
    }

    #[test]
    fn sequence_equality_preserves_earlier_mismatches() {
        assert_eq!(
            values_equal(
                &ConstValue::Array(vec![int(1), int(3)]),
                &ConstValue::Array(vec![int(2), int(3)]),
            ),
            Some(false)
        );
        assert_eq!(
            values_equal(
                &ConstValue::Tuple(vec![int(1), int(3)]),
                &ConstValue::Tuple(vec![int(2), int(3)]),
            ),
            Some(false)
        );
    }

    #[test]
    fn enum_payload_equality_preserves_earlier_mismatches() {
        let variant = GlobalDefId {
            module_id: ModuleIdAllocator::new().allocate(),
            def_id: DefId(2),
        };
        let tuple = |first| ConstValue::Enum {
            variant,
            payload: ConstEnumPayload::Tuple(vec![int(first), int(3)]),
        };
        assert_eq!(values_equal(&tuple(1), &tuple(2)), Some(false));

        let field_a = SymbolId::from_stable_hash(1);
        let field_b = SymbolId::from_stable_hash(2);
        let named = |first| {
            let mut fields = BTreeMap::new();
            fields.insert(field_a, int(first));
            fields.insert(field_b, int(3));
            ConstValue::Enum {
                variant,
                payload: ConstEnumPayload::Named(fields),
            }
        };
        assert_eq!(values_equal(&named(1), &named(2)), Some(false));
    }
}
