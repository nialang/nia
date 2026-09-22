// SPDX-License-Identifier: GPL-3.0-or-later
//! Control-flow and propagation validation contracts for function IR.

use super::*;

impl BackendValidator<'_> {
    pub(super) fn validate_terminator(&mut self, terminator: &FunctionTerminator) {
        match terminator {
            FunctionTerminator::If { cond, span, .. } => {
                self.validate_expr(cond);
                self.validate_bool_condition(cond.ty, *span);
            }
            FunctionTerminator::Switch { target, arms, .. } => {
                self.validate_expr(target);
                let mut case_values = HashSet::new();
                for arm in arms {
                    self.validate_expr(&arm.pattern);
                    if !self.same_type(target.ty, arm.pattern.ty) {
                        self.invalid_terminator(
                            arm.pattern.span,
                            "switch arm pattern type does not match its target",
                        );
                    }
                    match self.switch_case_value(&arm.pattern) {
                        Some(value) if !case_values.insert(value) => self.invalid_terminator(
                            arm.pattern.span,
                            "switch contains duplicate case values",
                        ),
                        Some(_) => {}
                        None => self.invalid_terminator(
                            arm.pattern.span,
                            "switch arm pattern is not a compile-time integer constant",
                        ),
                    }
                }
                if !self.is_integer_type(target.ty) {
                    self.invalid_terminator(target.span, "switch target must have an integer type");
                }
            }
            FunctionTerminator::Try {
                value,
                kind,
                error_conversion,
                success_local,
                span,
                ..
            } => {
                self.validate_expr(value);
                if let Some(conversion) = error_conversion {
                    self.validate_expr(conversion);
                }
                self.validate_try_contract(
                    value,
                    *kind,
                    error_conversion.as_deref(),
                    *success_local,
                    *span,
                );
            }
            FunctionTerminator::Loop { header, span, .. } => match header {
                nia_function_ir::FunctionForHeader::Infinite => {}
                nia_function_ir::FunctionForHeader::Condition(expr) => {
                    self.validate_expr(expr);
                    self.validate_bool_condition(expr.ty, *span);
                }
            },
            FunctionTerminator::Return { value, .. } | FunctionTerminator::Tail { value, .. } => {
                if let Some(value) = value {
                    self.validate_expr(value);
                }
            }
            FunctionTerminator::Error { .. }
            | FunctionTerminator::Branch { .. }
            | FunctionTerminator::Next { .. } => {}
        }
    }

    pub(super) fn validate_body_return_contract(
        &mut self,
        terminator: &FunctionTerminator,
        body_ty: nia_ids::InternedTyId,
        expected_return_ty: nia_ids::InternedTyId,
        in_defer: bool,
    ) {
        let (value, span, is_tail) = match terminator {
            FunctionTerminator::Return { value, span } => (value.as_ref(), *span, false),
            FunctionTerminator::Tail { value, span } => (value.as_ref(), *span, true),
            _ => return,
        };
        // A defer Tail exits only the mini-CFG; it does not return the enclosing
        // function. Its optional value is therefore intentionally unchecked here.
        if in_defer && is_tail {
            return;
        }
        match value {
            Some(value)
                if !self.same_type(value.ty, expected_return_ty)
                    && !matches!(
                        self.ty_kind(value.ty),
                        Some(TyKind::Primitive(PrimitiveTy::Never))
                    ) =>
            {
                self.invalid_terminator(
                    value.span,
                    "return value type does not match the function body return type",
                )
            }
            Some(_) => {}
            None if !self.is_unit_or_never(expected_return_ty)
                && !matches!(
                    self.ty_kind(body_ty),
                    Some(TyKind::Primitive(PrimitiveTy::Never))
                ) =>
            {
                self.invalid_terminator(
                    span,
                    "empty return terminator requires a unit or never return type",
                )
            }
            None => {}
        }
    }

    fn validate_bool_condition(&mut self, ty: nia_ids::InternedTyId, span: Span) {
        if !matches!(self.ty_kind(ty), Some(TyKind::Primitive(PrimitiveTy::Bool))) {
            self.invalid_terminator(span, "control-flow condition must have type bool");
        }
    }

    pub(super) fn is_integer_type(&self, ty: nia_ids::InternedTyId) -> bool {
        matches!(
            self.ty_kind(ty),
            Some(TyKind::Primitive(
                PrimitiveTy::I8
                    | PrimitiveTy::I16
                    | PrimitiveTy::I32
                    | PrimitiveTy::I64
                    | PrimitiveTy::I128
                    | PrimitiveTy::Isize
                    | PrimitiveTy::U8
                    | PrimitiveTy::U16
                    | PrimitiveTy::U32
                    | PrimitiveTy::U64
                    | PrimitiveTy::U128
                    | PrimitiveTy::Usize
                    | PrimitiveTy::Bool
                    | PrimitiveTy::Char
            ))
        )
    }

    /// Returns the case's LLVM integer bit pattern at its target width.
    ///
    /// Function lowering normally reduces integer and boolean patterns to
    /// `Integer` and enum patterns to `EnumVariantTag`. The other literal forms
    /// are retained because they are also directly representable LLVM integer
    /// constants. Keeping this allowlist here prevents a runtime expression
    /// from reaching `LLVMBuildSwitch`, whose case operands must be constants.
    fn switch_case_value(&self, pattern: &FunctionExpr) -> Option<u128> {
        use nia_function_ir::FunctionBuiltinValue;

        let value = match &pattern.kind {
            FunctionExprKind::Integer(text) => parse_int_literal(text)?.bits(),
            FunctionExprKind::Char(value) => u128::from(*value),
            FunctionExprKind::ByteChar(text) => u128::from(decode_byte_char_literal(text)?),
            FunctionExprKind::Bool(value) => u128::from(*value),
            FunctionExprKind::BuiltinValue(FunctionBuiltinValue::Int(value)) => value.bits(),
            FunctionExprKind::EnumVariantTag(variant) => {
                let info = self.index.enum_variant_info(*variant)?;
                info.variant.value.unwrap_or(info.index as i128) as u128
            }
            _ => return None,
        };
        let bits = self.switch_integer_bits(pattern.ty)?;
        let mask = if bits == u128::BITS {
            u128::MAX
        } else {
            (1_u128 << bits) - 1
        };
        Some(value & mask)
    }

    fn switch_integer_bits(&self, ty: nia_ids::InternedTyId) -> Option<u32> {
        let Some(TyKind::Primitive(primitive)) = self.ty_kind(ty) else {
            return None;
        };
        match primitive {
            PrimitiveTy::Bool => Some(1),
            PrimitiveTy::Char => Some(32),
            PrimitiveTy::Isize | PrimitiveTy::Usize => self
                .target
                .pointer_size
                .checked_mul(8)
                .and_then(|bits| u32::try_from(bits).ok()),
            _ => primitive.integer_bits(0),
        }
    }

    fn is_unit_or_never(&self, ty: nia_ids::InternedTyId) -> bool {
        match self.ty_kind(ty) {
            Some(TyKind::Primitive(PrimitiveTy::Never)) => true,
            Some(TyKind::Tuple(fields)) => fields.is_empty(),
            _ => false,
        }
    }

    fn invalid_terminator(&mut self, span: Span, message: &'static str) {
        self.diagnostics.push(Diagnostic::internal_error_at(
            nia_diagnostic::codes::INVALID_BACKEND_IR,
            span,
            format!("backend IR contains an invalid terminator contract: {message}"),
        ));
    }

    fn validate_try_contract(
        &mut self,
        value: &FunctionExpr,
        kind: FunctionTryKind,
        error_conversion: Option<&FunctionExpr>,
        success_local: nia_ids::LocalId,
        span: Span,
    ) {
        let value_kind = self.ty_kind(value.ty).cloned();
        let (source_error_ty, success_ty) = match (kind, value_kind) {
            (FunctionTryKind::Optional, Some(TyKind::Optional { elem })) => (None, elem),
            (FunctionTryKind::ErrorUnion, Some(TyKind::ErrorUnion { error, value })) => {
                (Some(error), value)
            }
            _ => {
                self.invalid_try(span, "propagation kind does not match its input union type");
                return;
            }
        };

        if let Some(local_ty) = self
            .local_tys
            .last()
            .and_then(|locals| locals.get(&success_local))
            .copied()
            && !self.same_type(local_ty, success_ty)
        {
            self.invalid_try(
                span,
                "propagation success local type does not match the input success payload",
            );
        }

        let Some(body_ty) = self.body_tys.last().copied() else {
            self.invalid_try(span, "propagation is not owned by a function body");
            return;
        };
        match (kind, self.ty_kind(body_ty).cloned()) {
            (FunctionTryKind::Optional, Some(TyKind::Optional { .. })) => {
                if error_conversion.is_some() {
                    self.invalid_try(span, "optional propagation carries an error conversion");
                }
            }
            (
                FunctionTryKind::ErrorUnion,
                Some(TyKind::ErrorUnion {
                    error: target_error_ty,
                    ..
                }),
            ) => {
                let propagated_error_ty = error_conversion
                    .map(|conversion| conversion.ty)
                    .or(source_error_ty);
                if !propagated_error_ty
                    .is_some_and(|error_ty| self.same_type(error_ty, target_error_ty))
                {
                    let message = if error_conversion.is_some() {
                        "propagation conversion type does not match the return error payload"
                    } else {
                        "direct propagation error type does not match the return error payload"
                    };
                    self.invalid_try(span, message);
                }
            }
            _ => self.invalid_try(
                span,
                "propagation kind does not match the function return union",
            ),
        }
    }

    pub(super) fn invalid_try(&mut self, span: Span, message: &'static str) {
        self.diagnostics.push(Diagnostic::internal_error_at(
            nia_diagnostic::codes::INVALID_BACKEND_IR,
            span,
            format!("backend IR contains invalid propagation: {message}"),
        ));
    }
}
