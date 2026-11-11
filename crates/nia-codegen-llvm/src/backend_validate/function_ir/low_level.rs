// SPDX-License-Identifier: GPL-3.0-or-later
//! Low-level builtin and SIMD validation contracts for function IR.

use super::*;

impl BackendValidator<'_> {
    /// Checks one intrinsic against its contract. The operand count is
    /// checked first, so each contract can bind its operands by position.
    pub(super) fn validate_intrinsic(
        &mut self,
        result_ty: nia_ids::InternedTyId,
        op: nia_ids::IntrinsicOp,
        args: &[FunctionExpr],
        span: Span,
    ) {
        use nia_ids::IntrinsicOp;
        match (op, args) {
            (IntrinsicOp::LoadUnaligned, [ptr]) => {
                self.validate_load_unaligned(result_ty, ptr, span)
            }
            (IntrinsicOp::Splat, [value]) => self.validate_splat(result_ty, value, span),
            (IntrinsicOp::ExtractElement, [vector, index]) => {
                self.validate_vector_element(result_ty, vector, index, None, span)
            }
            (IntrinsicOp::InsertElement, [vector, index, value]) => {
                self.validate_vector_element(result_ty, vector, index, Some(value), span)
            }
            (IntrinsicOp::Bitmask, [vector]) => self.validate_bitmask(result_ty, vector, span),
            (IntrinsicOp::Ctz | IntrinsicOp::Clz | IntrinsicOp::Popcount, [value]) => {
                self.validate_bit_intrinsic(result_ty, value, span)
            }
            (IntrinsicOp::SaturatingAdd | IntrinsicOp::SaturatingSub, [lhs, rhs]) => {
                self.validate_saturating(result_ty, lhs, rhs, span)
            }
            (IntrinsicOp::Shuffle, [first, second, indices @ ..]) if !indices.is_empty() => {
                self.validate_shuffle(result_ty, first, second, indices, span)
            }
            // Every operation is named so that a new one is a compile error
            // here until it has a contract.
            (
                IntrinsicOp::LoadUnaligned
                | IntrinsicOp::Splat
                | IntrinsicOp::ExtractElement
                | IntrinsicOp::InsertElement
                | IntrinsicOp::Bitmask
                | IntrinsicOp::Ctz
                | IntrinsicOp::Clz
                | IntrinsicOp::Popcount
                | IntrinsicOp::SaturatingAdd
                | IntrinsicOp::SaturatingSub
                | IntrinsicOp::Shuffle,
                _,
            ) => {
                for arg in args {
                    self.validate_expr(arg);
                }
                self.invalid_low_level_builtin(
                    op.name(),
                    span,
                    &format!("takes {}, found {}", op.arity(), args.len()),
                );
            }
        }
    }

    fn validate_saturating(
        &mut self,
        result_ty: nia_ids::InternedTyId,
        lhs: &FunctionExpr,
        rhs: &FunctionExpr,
        span: Span,
    ) {
        self.validate_expr(lhs);
        self.validate_expr(rhs);
        let is_integer = match self.index.ty_kind(result_ty) {
            Some(TyKind::Primitive(primitive)) => primitive.is_integer(),
            Some(TyKind::Vector { elem, .. }) => elem.is_integer(),
            _ => false,
        };
        if !is_integer {
            self.invalid_low_level_builtin(
                "saturating arithmetic",
                span,
                "result is not an integer or integer vector",
            );
        }
        if !self.same_type(result_ty, lhs.ty) || !self.same_type(result_ty, rhs.ty) {
            self.invalid_low_level_builtin(
                "saturating arithmetic",
                span,
                "operand type does not match the result type",
            );
        }
    }

    fn validate_shuffle(
        &mut self,
        result_ty: nia_ids::InternedTyId,
        first: &FunctionExpr,
        second: &FunctionExpr,
        indices: &[FunctionExpr],
        span: Span,
    ) {
        self.validate_expr(first);
        self.validate_expr(second);
        let operand = match self.index.ty_kind(first.ty) {
            Some(TyKind::Vector { elem, lanes }) => Some((*elem, *lanes)),
            _ => None,
        };
        let Some((elem, lanes)) = operand else {
            self.invalid_low_level_builtin("SIMD shuffle", span, "operand is not a vector");
            return;
        };
        if !self.same_type(first.ty, second.ty) {
            self.invalid_low_level_builtin(
                "SIMD shuffle",
                span,
                "operands do not have the same type",
            );
        }
        let result_matches = matches!(
            self.index.ty_kind(result_ty),
            Some(TyKind::Vector { elem: result_elem, lanes: result_lanes })
                if *result_elem == elem
                    && usize::try_from(*result_lanes).is_ok_and(|count| count == indices.len())
        );
        if !result_matches {
            self.invalid_low_level_builtin(
                "SIMD shuffle",
                span,
                "result is not a vector of the operand lane type with one lane per index",
            );
        }
        let limit = u64::from(lanes) * 2;
        if !indices.iter().all(|index| {
            nia_function_ir::shuffle_lane_index(index).is_some_and(|lane| lane < limit)
        }) {
            self.invalid_low_level_builtin(
                "SIMD shuffle",
                span,
                "lane index is not a constant below twice the operand lane count",
            );
        }
    }

    fn validate_load_unaligned(
        &mut self,
        result_ty: nia_ids::InternedTyId,
        ptr: &FunctionExpr,
        span: Span,
    ) {
        self.current_subject = Some("unaligned load value");
        self.validate_runtime_type(result_ty, span);
        self.current_subject = None;
        self.validate_expr(ptr);
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
        message: &str,
    ) {
        self.diagnostics.push(Diagnostic::internal_error_at(
            nia_diagnostic::codes::INVALID_BACKEND_IR,
            span,
            format!("backend IR {kind} has an invalid contract: {message}"),
        ));
    }
}
