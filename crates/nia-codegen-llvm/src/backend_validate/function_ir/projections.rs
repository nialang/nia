// SPDX-License-Identifier: GPL-3.0-or-later
//! Projection, global-value, and slice validation contracts.

use super::*;

impl BackendValidator<'_> {
    pub(super) fn validate_projection_result_type(
        &mut self,
        actual_ty: nia_ids::InternedTyId,
        expected_ty: nia_ids::InternedTyId,
        span: Span,
        kind: &'static str,
    ) {
        if !self.projection_result_compatible(actual_ty, expected_ty) {
            self.diagnostics.push(Diagnostic::internal_error_at(
                nia_diagnostic::codes::INVALID_BACKEND_IR,
                span,
                format!("backend IR {kind} result type does not match its selected value"),
            ));
        }
    }

    pub(super) fn projection_result_compatible(
        &self,
        actual_ty: nia_ids::InternedTyId,
        selected_ty: nia_ids::InternedTyId,
    ) -> bool {
        if self.same_type(actual_ty, selected_ty) {
            return true;
        }
        match (
            self.index.ty_kind(actual_ty),
            self.index.ty_kind(selected_ty),
        ) {
            (
                Some(TyKind::Pointer {
                    is_readonly: actual_readonly,
                    elem: actual_elem,
                }),
                Some(TyKind::Pointer {
                    is_readonly: selected_readonly,
                    elem: selected_elem,
                }),
            )
            | (
                Some(TyKind::VolatilePointer {
                    is_readonly: actual_readonly,
                    elem: actual_elem,
                }),
                Some(TyKind::VolatilePointer {
                    is_readonly: selected_readonly,
                    elem: selected_elem,
                }),
            )
            | (
                Some(TyKind::Slice {
                    is_readonly: actual_readonly,
                    elem: actual_elem,
                }),
                Some(TyKind::Slice {
                    is_readonly: selected_readonly,
                    elem: selected_elem,
                }),
            ) => {
                (*actual_readonly || !*selected_readonly)
                    && self.projection_result_compatible(*actual_elem, *selected_elem)
            }
            _ => false,
        }
    }

    pub(super) fn invalid_global_value_type(&mut self, span: Span, kind: &'static str) {
        self.diagnostics.push(Diagnostic::internal_error_at(
            nia_diagnostic::codes::INVALID_BACKEND_IR,
            span,
            format!("backend IR {kind} expression type does not match its storage type"),
        ));
    }

    pub(super) fn validate_slice_contract(
        &mut self,
        result_ty: nia_ids::InternedTyId,
        lhs: &FunctionExpr,
        requested_readonly: bool,
        span: Span,
    ) {
        self.validate_expr(lhs);
        let Some(source) = self.slice_source_info(lhs.ty) else {
            self.invalid_slice(span, "slice input is not an array, pointer, or slice");
            return;
        };
        let Some(TyKind::Slice {
            is_readonly,
            elem: result_elem,
        }) = self.index.ty_kind(result_ty)
        else {
            self.invalid_slice(span, "slice result is not a Slice type");
            return;
        };
        if !self.same_type(*result_elem, source.elem) {
            self.invalid_slice(span, "slice result element does not match its input");
        }
        if *is_readonly != requested_readonly {
            self.invalid_slice(
                span,
                "slice result readonly metadata does not match the expression",
            );
        }
        if source.readonly && !*is_readonly {
            self.invalid_slice(span, "slice drops readonly access from its input");
        }
    }

    pub(super) fn validate_slice_bound(&mut self, bound: &FunctionExpr) {
        if !self.is_integer_type(bound.ty) {
            self.invalid_slice(bound.span, "slice range bound is not an integer");
        }
    }

    fn slice_source_info(&self, ty: nia_ids::InternedTyId) -> Option<SliceSourceInfo> {
        match self.index.ty_kind(ty) {
            Some(TyKind::Array { elem, .. }) => Some(SliceSourceInfo {
                elem: *elem,
                readonly: false,
            }),
            Some(TyKind::Pointer { is_readonly, elem })
            | Some(TyKind::VolatilePointer { is_readonly, elem }) => {
                if let Some(TyKind::Array {
                    elem: array_elem, ..
                }) = self.index.ty_kind(*elem)
                {
                    Some(SliceSourceInfo {
                        elem: *array_elem,
                        readonly: *is_readonly,
                    })
                } else {
                    Some(SliceSourceInfo {
                        elem: *elem,
                        readonly: *is_readonly,
                    })
                }
            }
            Some(TyKind::Slice { is_readonly, elem }) => Some(SliceSourceInfo {
                elem: *elem,
                readonly: *is_readonly,
            }),
            _ => None,
        }
    }

    fn invalid_slice(&mut self, span: Span, message: &'static str) {
        self.diagnostics.push(Diagnostic::internal_error_at(
            nia_diagnostic::codes::INVALID_BACKEND_IR,
            span,
            format!("backend IR slice expression has an invalid contract: {message}"),
        ));
    }
}
