// SPDX-License-Identifier: GPL-3.0-or-later
//! Compiler-provided builtin value validation contracts.

use super::*;

impl BackendValidator<'_> {
    pub(super) fn validate_builtin_value(
        &mut self,
        result_ty: nia_ids::InternedTyId,
        value: &nia_function_ir::FunctionBuiltinValue,
        span: Span,
    ) {
        use nia_function_ir::FunctionBuiltinValue;

        match value {
            FunctionBuiltinValue::Usize(value) => {
                self.validate_builtin_usize_result(result_ty, span);
                if !self.builtin_usize_fits(*value) {
                    self.invalid_builtin_value(
                        span,
                        "usize constant value is outside its target pointer width",
                    );
                }
            }
            FunctionBuiltinValue::Layout { ty, .. } => {
                self.current_subject = Some("layout builtin operand");
                self.validate_runtime_type(*ty, span);
                self.current_subject = None;
                self.validate_builtin_usize_result(result_ty, span);
            }
            FunctionBuiltinValue::FieldOffset { ty, field } => {
                self.current_subject = Some("field-offset builtin operand");
                self.validate_runtime_type(*ty, span);
                self.current_subject = None;
                self.validate_aggregate_field(
                    *ty,
                    *field,
                    span,
                    "backend IR field-offset builtin references missing field",
                );
                self.validate_builtin_usize_result(result_ty, span);
            }
            FunctionBuiltinValue::Int(value) => {
                if !self.is_integer_type(result_ty) {
                    self.invalid_builtin_value(span, "integer constant result is not integer-like");
                } else if !self.builtin_integer_fits(result_ty, *value) {
                    self.invalid_builtin_value(
                        span,
                        "integer constant value is outside its result type",
                    );
                }
            }
        }
    }

    fn validate_builtin_usize_result(&mut self, result_ty: nia_ids::InternedTyId, span: Span) {
        if !matches!(
            self.index.ty_kind(result_ty),
            Some(TyKind::Primitive(PrimitiveTy::Usize))
        ) {
            self.invalid_builtin_value(span, "result type is not usize");
        }
    }

    fn builtin_usize_fits(&self, value: u64) -> bool {
        let Some(bits) = self
            .target
            .pointer_size
            .checked_mul(8)
            .and_then(|bits| u32::try_from(bits).ok())
        else {
            return false;
        };
        bits >= u64::BITS || value < (1_u64 << bits)
    }

    fn builtin_integer_fits(&self, ty: nia_ids::InternedTyId, value: nia_ty::IntConst) -> bool {
        let Some(TyKind::Primitive(primitive)) = self.ty_kind(ty) else {
            return false;
        };
        let pointer_width = self
            .target
            .pointer_size
            .checked_mul(8)
            .and_then(|bits| u32::try_from(bits).ok())
            .unwrap_or(0);
        match *primitive {
            PrimitiveTy::Bool => !value.is_signed() && matches!(value.bits(), 0 | 1),
            PrimitiveTy::Char => {
                !value.is_signed()
                    && u32::try_from(value.bits())
                        .ok()
                        .and_then(char::from_u32)
                        .is_some()
            }
            primitive if primitive.is_integer() => {
                value.fits_primitive_int(primitive, pointer_width)
                    || (value.is_signed()
                        && primitive.is_signed_integer()
                        && primitive.integer_bits(pointer_width).is_some_and(|bits| {
                            bits < u128::BITS
                                && value.bits() < (1_u128 << bits)
                                && value.bits() >= (1_u128 << (bits - 1))
                        }))
            }
            _ => false,
        }
    }

    fn invalid_builtin_value(&mut self, span: Span, message: &'static str) {
        self.diagnostics.push(Diagnostic::internal_error_at(
            nia_diagnostic::codes::INVALID_BACKEND_IR,
            span,
            format!("backend IR builtin value has an invalid contract: {message}"),
        ));
    }
}
