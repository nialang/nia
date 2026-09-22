// SPDX-License-Identifier: GPL-3.0-or-later
//! Operator and cast validation contracts for function IR.

use super::*;
use nia_ast::{BinaryOp, UnaryOp};

impl BackendValidator<'_> {
    pub(super) fn validate_unary(
        &mut self,
        result_ty: nia_ids::InternedTyId,
        op: UnaryOp,
        inner: &FunctionExpr,
        span: Span,
    ) {
        self.validate_expr(inner);
        match op {
            UnaryOp::Ref | UnaryOp::RefReadOnly => {
                if !matches!(
                    inner.kind,
                    FunctionExprKind::Function(_) | FunctionExprKind::FunctionInstance { .. }
                ) {
                    self.invalid_operator(
                        span,
                        "reference unary operation requires a function item",
                    );
                }
                if !matches!(
                    self.index.ty_kind(result_ty),
                    Some(TyKind::FunctionPointer { .. })
                ) {
                    self.invalid_operator(
                        span,
                        "function reference result is not a function pointer",
                    );
                }
                if !self.same_type(result_ty, inner.ty) {
                    self.invalid_operator(
                        span,
                        "function reference result type does not match its function item",
                    );
                }
            }
            UnaryOp::Deref => {
                let expected = match self.index.ty_kind(inner.ty) {
                    Some(TyKind::Pointer { elem, .. } | TyKind::VolatilePointer { elem, .. }) => {
                        Some(*elem)
                    }
                    _ => None,
                };
                if let Some(expected) = expected {
                    if !self.same_type(result_ty, expected) {
                        self.invalid_operator(span, "deref result type does not match its pointee");
                    }
                } else {
                    self.invalid_operator(span, "deref operand is not a pointer");
                }
            }
            UnaryOp::Neg => {
                if !self.is_numeric_operator_type(inner.ty) {
                    self.invalid_operator(span, "negation operand is not numeric");
                }
                if !self.same_type(result_ty, inner.ty) {
                    self.invalid_operator(span, "negation result type does not match its operand");
                }
            }
            UnaryOp::Not => {
                if !self.is_bool_type(inner.ty) {
                    self.invalid_operator(span, "logical not operand is not bool");
                }
                if !self.same_type(result_ty, inner.ty) {
                    self.invalid_operator(
                        span,
                        "logical not result type does not match its operand",
                    );
                }
            }
            UnaryOp::BitNot => {
                if !self.is_integer_operator_type(inner.ty) {
                    self.invalid_operator(span, "bitwise unary operand is not integer-like");
                }
                if !self.same_type(result_ty, inner.ty) {
                    self.invalid_operator(
                        span,
                        "bitwise unary result type does not match its operand",
                    );
                }
            }
        }
    }

    pub(super) fn validate_cast(
        &mut self,
        result_ty: nia_ids::InternedTyId,
        target_ty: nia_ids::InternedTyId,
        inner: &FunctionExpr,
        span: Span,
    ) {
        self.validate_expr(inner);
        self.current_subject = Some("cast target");
        self.validate_runtime_type(target_ty, span);
        self.current_subject = None;
        if !self.same_type(result_ty, target_ty) {
            self.invalid_operator(span, "cast result type does not match its target metadata");
        }
        if self.same_type(inner.ty, target_ty) {
            return;
        }
        let source_pointer = self.is_pointer_like_type(inner.ty);
        let target_pointer = self.is_pointer_like_type(target_ty);
        let source_pointer_int = self.is_pointer_integer_type(inner.ty);
        let target_pointer_int = self.is_pointer_integer_type(target_ty);
        let source_integer = self.is_cast_integer_type(inner.ty);
        let target_integer = self.is_cast_integer_type(target_ty);
        let source_float = self.is_cast_float_type(inner.ty);
        let target_float = self.is_cast_float_type(target_ty);
        let numeric = (source_integer || source_float) && (target_integer || target_float);
        let char_to_u32 = matches!(
            self.index.ty_kind(inner.ty),
            Some(TyKind::Primitive(PrimitiveTy::Char))
        ) && matches!(
            self.index.ty_kind(target_ty),
            Some(TyKind::Primitive(PrimitiveTy::U32))
        );
        let enum_cast = (self.is_enum_type(inner.ty) && target_integer)
            || (source_integer && self.is_enum_type(target_ty));
        let pointer_cast = (source_pointer && target_pointer)
            || (source_pointer && target_pointer_int)
            || (source_pointer_int && target_pointer);
        if !(numeric || char_to_u32 || enum_cast || pointer_cast) {
            self.invalid_operator(span, "cast source and target categories are incompatible");
            return;
        }
        if numeric && !self.cast_shapes_match(inner.ty, target_ty) {
            self.invalid_operator(span, "numeric cast changes scalar/vector shape");
        }
    }

    fn is_cast_integer_type(&self, ty: nia_ids::InternedTyId) -> bool {
        match self.index.ty_kind(ty) {
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
                | PrimitiveTy::Usize,
            )) => true,
            Some(TyKind::Vector { elem, .. }) => matches!(
                elem,
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
            ),
            _ => false,
        }
    }

    fn is_cast_float_type(&self, ty: nia_ids::InternedTyId) -> bool {
        matches!(
            self.index.ty_kind(ty),
            Some(TyKind::Primitive(PrimitiveTy::F32 | PrimitiveTy::F64))
                | Some(TyKind::Vector {
                    elem: PrimitiveTy::F32 | PrimitiveTy::F64,
                    ..
                })
        )
    }

    fn is_pointer_like_type(&self, ty: nia_ids::InternedTyId) -> bool {
        matches!(
            self.index.ty_kind(ty),
            Some(
                TyKind::Pointer { .. }
                    | TyKind::VolatilePointer { .. }
                    | TyKind::FunctionPointer { .. }
            )
        )
    }

    fn is_pointer_integer_type(&self, ty: nia_ids::InternedTyId) -> bool {
        matches!(
            self.index.ty_kind(ty),
            Some(TyKind::Primitive(PrimitiveTy::Isize | PrimitiveTy::Usize))
        )
    }

    fn is_enum_type(&self, ty: nia_ids::InternedTyId) -> bool {
        matches!(self.index.ty_kind(ty), Some(TyKind::Nominal { def_id, .. }) if self.index.has_enum(*def_id))
    }

    fn cast_shapes_match(
        &self,
        source: nia_ids::InternedTyId,
        target: nia_ids::InternedTyId,
    ) -> bool {
        match (self.index.ty_kind(source), self.index.ty_kind(target)) {
            (
                Some(TyKind::Vector { lanes: source, .. }),
                Some(TyKind::Vector { lanes: target, .. }),
            ) => source == target,
            (Some(TyKind::Vector { .. }), _) | (_, Some(TyKind::Vector { .. })) => false,
            _ => true,
        }
    }

    pub(super) fn validate_binary(
        &mut self,
        result_ty: nia_ids::InternedTyId,
        lhs: &FunctionExpr,
        op: BinaryOp,
        rhs: &FunctionExpr,
        span: Span,
    ) {
        self.validate_expr(lhs);
        self.validate_expr(rhs);
        self.validate_binary_contract(result_ty, lhs.ty, op, rhs.ty, span);
    }

    pub(super) fn validate_binary_contract(
        &mut self,
        result_ty: nia_ids::InternedTyId,
        lhs_ty: nia_ids::InternedTyId,
        op: BinaryOp,
        rhs_ty: nia_ids::InternedTyId,
        span: Span,
    ) {
        if matches!(op, BinaryOp::And | BinaryOp::Or) {
            if !self.is_bool_type(lhs_ty)
                || !self.is_bool_type(rhs_ty)
                || !self.is_bool_type(result_ty)
            {
                self.invalid_operator(span, "logical operator requires bool operands and result");
            }
            return;
        }
        let matching_operands = if matches!(op, BinaryOp::Shl | BinaryOp::Shr) {
            match self.index.ty_kind(lhs_ty) {
                Some(TyKind::Vector { .. }) => self.same_type(lhs_ty, rhs_ty),
                _ => self.is_integer_operator_type(rhs_ty),
            }
        } else {
            self.same_type(lhs_ty, rhs_ty)
        };
        if !matching_operands {
            self.invalid_operator(span, "binary operands do not have a compatible type");
        }
        let valid_operand = match op {
            BinaryOp::Add
            | BinaryOp::Sub
            | BinaryOp::Mul
            | BinaryOp::Div
            | BinaryOp::Rem
            | BinaryOp::Lt
            | BinaryOp::Le
            | BinaryOp::Gt
            | BinaryOp::Ge => self.is_numeric_operator_type(lhs_ty) || self.is_char_type(lhs_ty),
            BinaryOp::Eq | BinaryOp::Ne => self.is_comparable_operator_type(lhs_ty),
            BinaryOp::BitAnd | BinaryOp::BitXor | BinaryOp::BitOr => {
                self.is_bitwise_operator_type(lhs_ty)
            }
            BinaryOp::Shl | BinaryOp::Shr => self.is_integer_operator_type(lhs_ty),
            BinaryOp::And | BinaryOp::Or => true,
        };
        if !valid_operand {
            self.invalid_operator(
                span,
                "binary operand type is not supported by the operation",
            );
        }
        let comparison = matches!(
            op,
            BinaryOp::Lt | BinaryOp::Le | BinaryOp::Gt | BinaryOp::Ge | BinaryOp::Eq | BinaryOp::Ne
        );
        let expected_result = if comparison {
            match self.index.ty_kind(lhs_ty) { Some(TyKind::Vector { lanes, .. }) => self.index.ty_kind(result_ty).is_some_and(|kind| matches!(kind, TyKind::Vector { elem: PrimitiveTy::Bool, lanes: result_lanes } if result_lanes == lanes)), _ => self.is_bool_type(result_ty) }
        } else {
            self.same_type(result_ty, lhs_ty)
        };
        if !expected_result {
            self.invalid_operator(span, "binary result type does not match the operation");
        }
    }

    pub(super) fn is_bool_type(&self, ty: nia_ids::InternedTyId) -> bool {
        matches!(
            self.index.ty_kind(ty),
            Some(TyKind::Primitive(PrimitiveTy::Bool))
        )
    }
    fn is_char_type(&self, ty: nia_ids::InternedTyId) -> bool {
        matches!(
            self.index.ty_kind(ty),
            Some(TyKind::Primitive(PrimitiveTy::Char))
        )
    }
    fn is_comparable_operator_type(&self, ty: nia_ids::InternedTyId) -> bool {
        self.is_numeric_operator_type(ty)
            || self.is_bool_type(ty)
            || self.is_char_type(ty)
            || match self.index.ty_kind(ty) {
                Some(TyKind::Nominal { def_id, .. }) => self.index.has_enum(*def_id),
                Some(TyKind::Pointer { .. } | TyKind::FunctionPointer { .. }) => true,
                _ => false,
            }
    }
    fn is_integer_operator_type(&self, ty: nia_ids::InternedTyId) -> bool {
        match self.index.ty_kind(ty) {
            Some(TyKind::Primitive(primitive)) => primitive.is_integer(),
            Some(TyKind::Nominal { def_id, .. }) => self.index.has_enum(*def_id),
            Some(TyKind::Vector { elem, .. }) => elem.is_integer(),
            _ => false,
        }
    }
    fn is_bitwise_operator_type(&self, ty: nia_ids::InternedTyId) -> bool {
        self.is_integer_operator_type(ty)
            || matches!(
                self.index.ty_kind(ty),
                Some(TyKind::Vector {
                    elem: PrimitiveTy::Bool,
                    ..
                })
            )
    }
    fn is_numeric_operator_type(&self, ty: nia_ids::InternedTyId) -> bool {
        self.is_integer_operator_type(ty)
            || matches!(
                self.index.ty_kind(ty),
                Some(TyKind::Primitive(PrimitiveTy::F32 | PrimitiveTy::F64))
                    | Some(TyKind::Vector {
                        elem: PrimitiveTy::F32 | PrimitiveTy::F64,
                        ..
                    })
            )
    }
    fn invalid_operator(&mut self, span: Span, message: &'static str) {
        self.diagnostics.push(Diagnostic::internal_error_at(
            nia_diagnostic::codes::INVALID_BACKEND_IR,
            span,
            format!("backend IR operator has an invalid contract: {message}"),
        ));
    }
}
