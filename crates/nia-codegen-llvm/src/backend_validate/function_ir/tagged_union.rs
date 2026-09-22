// SPDX-License-Identifier: GPL-3.0-or-later
//! Optional and error-union expression validation contracts.

use super::*;

impl BackendValidator<'_> {
    pub(super) fn validate_tagged_union_constructor(
        &mut self,
        result_ty: nia_ids::InternedTyId,
        inner: &FunctionExpr,
        constructor: TaggedUnionConstructor,
        span: Span,
    ) {
        self.validate_expr(inner);
        let expected_payload = match (constructor, self.index.ty_kind(result_ty)) {
            (TaggedUnionConstructor::OptionalSome, Some(TyKind::Optional { elem })) => Some(*elem),
            (TaggedUnionConstructor::ErrorOk, Some(TyKind::ErrorUnion { value, .. })) => {
                Some(*value)
            }
            (TaggedUnionConstructor::ErrorErr, Some(TyKind::ErrorUnion { error, .. })) => {
                Some(*error)
            }
            _ => None,
        };
        let Some(expected_payload) = expected_payload else {
            self.invalid_tagged_union(
                span,
                "constructor result is not the matching Optional or ErrorUnion type",
            );
            return;
        };
        if !self.same_type(inner.ty, expected_payload) {
            self.invalid_tagged_union(span, "constructor payload type does not match its result");
        }
    }

    pub(super) fn validate_tagged_union_projection(
        &mut self,
        result_ty: nia_ids::InternedTyId,
        inner: &FunctionExpr,
        projection: TaggedUnionProjection,
        span: Span,
    ) {
        self.validate_expr(inner);
        let Some(kind) = self.index.ty_kind(inner.ty) else {
            self.invalid_tagged_union(span, "projection input has no runtime type");
            return;
        };
        match (projection, kind) {
            (TaggedUnionProjection::Tag, TyKind::Optional { .. } | TyKind::ErrorUnion { .. }) => {
                if !matches!(
                    self.index.ty_kind(result_ty),
                    Some(TyKind::Primitive(PrimitiveTy::U8))
                ) {
                    self.invalid_tagged_union(span, "tag projection result is not u8");
                }
            }
            (TaggedUnionProjection::Payload, TyKind::Optional { elem }) => {
                if !self.same_type(result_ty, *elem) {
                    self.invalid_tagged_union(
                        span,
                        "optional payload result does not match its element",
                    );
                }
            }
            (TaggedUnionProjection::Payload, TyKind::ErrorUnion { error, value }) => {
                if !self.same_type(result_ty, *error) && !self.same_type(result_ty, *value) {
                    self.invalid_tagged_union(
                        span,
                        "error-union payload result matches neither error nor value type",
                    );
                }
            }
            (TaggedUnionProjection::Tag, _) => {
                self.invalid_tagged_union(span, "tag projection input is not a tagged union");
            }
            (TaggedUnionProjection::Payload, _) => {
                self.invalid_tagged_union(span, "payload projection input is not a tagged union");
            }
        }
    }

    pub(super) fn invalid_tagged_union(&mut self, span: Span, message: &'static str) {
        self.diagnostics.push(Diagnostic::internal_error_at(
            nia_diagnostic::codes::INVALID_BACKEND_IR,
            span,
            format!("backend IR tagged-union expression has an invalid contract: {message}"),
        ));
    }
}
