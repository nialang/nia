// SPDX-License-Identifier: GPL-3.0-or-later
//! Low-level builtin and SIMD validation contracts for function IR.

use super::*;

impl BackendValidator<'_> {
    pub(super) fn validate_load_unaligned(
        &mut self,
        result_ty: nia_ids::InternedTyId,
        load_ty: nia_ids::InternedTyId,
        ptr: &FunctionExpr,
        span: Span,
    ) {
        self.current_subject = Some("unaligned load value");
        self.validate_runtime_type(load_ty, span);
        self.current_subject = None;
        self.validate_expr(ptr);
        if !self.same_type(result_ty, load_ty) {
            self.invalid_low_level_builtin(
                "unaligned load",
                span,
                "result type does not match its load metadata",
            );
        }
        let is_byte_pointer = match self.index.ty_kind(ptr.ty) {
            Some(TyKind::Pointer { elem, .. }) => matches!(
                self.index.ty_kind(*elem),
                Some(TyKind::Primitive(PrimitiveTy::U8))
            ),
            _ => false,
        };
        if !is_byte_pointer {
            self.invalid_low_level_builtin("unaligned load", span, "operand is not a byte pointer");
        }
    }

    pub(super) fn validate_splat(
        &mut self,
        result_ty: nia_ids::InternedTyId,
        value: &FunctionExpr,
        span: Span,
    ) {
        self.validate_expr(value);
        match self.index.ty_kind(result_ty) {
            Some(TyKind::Vector { elem, .. }) => {
                if !matches!(self.index.ty_kind(value.ty), Some(TyKind::Primitive(actual)) if actual == elem)
                {
                    self.invalid_low_level_builtin(
                        "SIMD splat",
                        span,
                        "scalar value type does not match the result lane type",
                    );
                }
            }
            _ => self.invalid_low_level_builtin("SIMD splat", span, "result is not a vector"),
        }
    }

    pub(super) fn validate_vector_element(
        &mut self,
        result_ty: nia_ids::InternedTyId,
        vector: &FunctionExpr,
        index: &FunctionExpr,
        inserted: Option<&FunctionExpr>,
        span: Span,
    ) {
        self.validate_expr(vector);
        self.validate_expr(index);
        if let Some(value) = inserted {
            self.validate_expr(value);
        }
        if !matches!(self.index.ty_kind(index.ty), Some(TyKind::Primitive(primitive)) if primitive.is_integer())
        {
            self.invalid_low_level_builtin(
                "SIMD lane",
                span,
                "index does not have an integer type",
            );
        }

        let Some(TyKind::Vector { elem, .. }) = self.index.ty_kind(vector.ty) else {
            self.invalid_low_level_builtin("SIMD lane", span, "operand is not a vector");
            return;
        };
        match inserted {
            Some(value) => {
                if !self.same_type(result_ty, vector.ty) {
                    self.invalid_low_level_builtin(
                        "SIMD insert",
                        span,
                        "result type does not match its vector operand",
                    );
                }
                if !matches!(self.index.ty_kind(value.ty), Some(TyKind::Primitive(actual)) if actual == elem)
                {
                    self.invalid_low_level_builtin(
                        "SIMD insert",
                        span,
                        "inserted value type does not match the vector lane type",
                    );
                }
            }
            None => {
                if !matches!(self.index.ty_kind(result_ty), Some(TyKind::Primitive(actual)) if actual == elem)
                {
                    self.invalid_low_level_builtin(
                        "SIMD extract",
                        span,
                        "result type does not match the vector lane type",
                    );
                }
            }
        }
    }

    pub(super) fn validate_bitmask(
        &mut self,
        result_ty: nia_ids::InternedTyId,
        vector: &FunctionExpr,
        span: Span,
    ) {
        self.validate_expr(vector);
        if !matches!(
            self.index.ty_kind(result_ty),
            Some(TyKind::Primitive(PrimitiveTy::Usize))
        ) {
            self.invalid_low_level_builtin("SIMD bitmask", span, "result type is not usize");
        }
        match self.index.ty_kind(vector.ty) {
            Some(TyKind::Vector {
                elem: PrimitiveTy::Bool,
                lanes,
            }) if u64::from(*lanes) <= self.target.pointer_size.saturating_mul(8) => {}
            Some(TyKind::Vector {
                elem: PrimitiveTy::Bool,
                ..
            }) => self.invalid_low_level_builtin(
                "SIMD bitmask",
                span,
                "mask exceeds the target usize width",
            ),
            _ => {
                self.invalid_low_level_builtin("SIMD bitmask", span, "operand is not a bool vector")
            }
        }
    }

    pub(super) fn validate_bit_intrinsic(
        &mut self,
        result_ty: nia_ids::InternedTyId,
        value: &FunctionExpr,
        span: Span,
    ) {
        self.validate_expr(value);
        if !matches!(self.index.ty_kind(value.ty), Some(TyKind::Primitive(primitive)) if primitive.is_integer())
        {
            self.invalid_low_level_builtin(
                "bit intrinsic",
                span,
                "operand does not have an integer type",
            );
        }
        if !self.same_type(result_ty, value.ty) {
            self.invalid_low_level_builtin(
                "bit intrinsic",
                span,
                "result type does not match its operand",
            );
        }
    }

    pub(super) fn validate_char_from_u32(
        &mut self,
        result_ty: nia_ids::InternedTyId,
        value: &FunctionExpr,
        span: Span,
    ) {
        self.validate_expr(value);
        if !matches!(
            self.index.ty_kind(value.ty),
            Some(TyKind::Primitive(PrimitiveTy::U32))
        ) {
            self.invalid_low_level_builtin("char conversion", span, "operand type is not u32");
        }
        let optional_char = match self.index.ty_kind(result_ty) {
            Some(TyKind::Optional { elem }) => matches!(
                self.index.ty_kind(*elem),
                Some(TyKind::Primitive(PrimitiveTy::Char))
            ),
            _ => false,
        };
        if !optional_char {
            self.invalid_low_level_builtin(
                "char conversion",
                span,
                "result type is not Optional[char]",
            );
        }
    }

    pub(super) fn invalid_low_level_builtin(
        &mut self,
        kind: &'static str,
        span: Span,
        message: &'static str,
    ) {
        self.diagnostics.push(Diagnostic::internal_error_at(
            nia_diagnostic::codes::INVALID_BACKEND_IR,
            span,
            format!("backend IR {kind} has an invalid contract: {message}"),
        ));
    }
}
