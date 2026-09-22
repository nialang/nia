// SPDX-License-Identifier: GPL-3.0-or-later
//! Range and promoted static-array pointer validation contracts.

use super::*;

impl BackendValidator<'_> {
    pub(super) fn validate_range_expr(
        &mut self,
        result_ty: nia_ids::InternedTyId,
        range: &nia_function_ir::FunctionRange,
        span: Span,
    ) {
        for value in range.start.iter().chain(range.end.iter()) {
            self.validate_expr(value);
        }
        let Some(TyKind::Range { kind, bound }) = self.index.ty_kind(result_ty).cloned() else {
            self.invalid_range(span, "expression type is not a range");
            return;
        };
        let has_start = range.start.is_some();
        let has_end = range.end.is_some();
        if has_start != kind.has_start_bound() || has_end != kind.has_end_bound() {
            self.invalid_range(span, "range bound presence does not match its range kind");
        }
        let expected_inclusive = matches!(
            kind,
            nia_ty::RangeTyKind::Inclusive | nia_ty::RangeTyKind::ToInclusive
        );
        if range.inclusive != expected_inclusive {
            self.invalid_range(span, "inclusive metadata does not match its range kind");
        }
        let Some(bound_ty) = bound else {
            if has_start || has_end {
                self.invalid_range(span, "full range carries a bound expression");
            }
            return;
        };
        for value in range.start.iter().chain(range.end.iter()) {
            if !self.same_type(value.ty, bound_ty) {
                self.invalid_range(span, "range bound type does not match its range bound type");
            }
        }
    }

    pub(super) fn validate_static_array_pointer(
        &mut self,
        result_ty: nia_ids::InternedTyId,
        array: &FunctionExpr,
        declared_readonly: bool,
        span: Span,
    ) {
        self.validate_expr(array);
        if !matches!(self.index.ty_kind(array.ty), Some(TyKind::Array { .. })) {
            self.invalid_static_array_pointer(span, "promoted value is not an array");
        }
        let Some(TyKind::Pointer { is_readonly, elem }) = self.index.ty_kind(result_ty) else {
            self.invalid_static_array_pointer(span, "result type is not a pointer");
            return;
        };
        if *is_readonly != declared_readonly {
            self.invalid_static_array_pointer(span, "readonly metadata does not match its result");
        }
        if !self.same_type(*elem, array.ty) {
            self.invalid_static_array_pointer(
                span,
                "result pointer element does not match the promoted array",
            );
        }
    }

    fn invalid_static_array_pointer(&mut self, span: Span, message: &'static str) {
        self.diagnostics.push(Diagnostic::internal_error_at(
            nia_diagnostic::codes::INVALID_BACKEND_IR,
            span,
            format!("backend IR static array pointer has an invalid contract: {message}"),
        ));
    }

    pub(super) fn validate_range_bound(
        &mut self,
        result_ty: nia_ids::InternedTyId,
        range: &FunctionExpr,
        bound: nia_function_ir::FunctionRangeBound,
        span: Span,
    ) {
        self.validate_expr(range);
        let Some(TyKind::Range {
            kind,
            bound: Some(bound_ty),
        }) = self.index.ty_kind(range.ty).cloned()
        else {
            self.invalid_range(span, "bound projection input is not a bounded range");
            return;
        };
        let available = match bound {
            nia_function_ir::FunctionRangeBound::Start => kind.has_start_bound(),
            nia_function_ir::FunctionRangeBound::End => kind.has_end_bound(),
        };
        if !available {
            self.invalid_range(span, "requested bound is not present for the range kind");
        }
        if !self.same_type(result_ty, bound_ty) {
            self.invalid_range(
                span,
                "bound projection result does not match its range bound type",
            );
        }
    }

    pub(super) fn invalid_range(&mut self, span: Span, message: &'static str) {
        self.diagnostics.push(Diagnostic::internal_error_at(
            nia_diagnostic::codes::INVALID_BACKEND_IR,
            span,
            format!("backend IR range expression has an invalid contract: {message}"),
        ));
    }
}
