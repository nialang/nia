// SPDX-License-Identifier: GPL-3.0-or-later
//! Trait-object coercion and upcast validation contracts.

use super::*;

impl BackendValidator<'_> {
    pub(super) fn validate_trait_object_upcast(
        &mut self,
        result_ty: nia_ids::InternedTyId,
        inner: &FunctionExpr,
        source_ty: nia_ids::InternedTyId,
        target_ty: nia_ids::InternedTyId,
        span: Span,
    ) {
        self.validate_expr(inner);
        self.validate_runtime_type(source_ty, span);
        self.validate_runtime_type(target_ty, span);
        let source_readonly = match self.ty_kind(source_ty) {
            Some(TyKind::TraitObject { is_readonly, .. }) => Some(*is_readonly),
            _ => None,
        };
        let target_readonly = match self.ty_kind(target_ty) {
            Some(TyKind::TraitObject { is_readonly, .. }) => Some(*is_readonly),
            _ => None,
        };
        if source_readonly.is_none() || target_readonly.is_none() {
            self.invalid_trait_object(span, "upcast source and target must be trait objects");
            return;
        }
        if !self.same_type(inner.ty, source_ty) {
            self.invalid_trait_object(span, "upcast source metadata does not match the operand");
        }
        if !self.same_type(result_ty, target_ty) {
            self.invalid_trait_object(span, "upcast result type does not match target metadata");
        }
        if source_readonly == Some(true) && target_readonly == Some(false) {
            self.invalid_trait_object(span, "upcast cannot strengthen readonly access");
        }
    }

    pub(super) fn validate_trait_object_coercion(
        &mut self,
        result_ty: nia_ids::InternedTyId,
        inner: &FunctionExpr,
        self_ty: nia_ids::InternedTyId,
        target_ty: nia_ids::InternedTyId,
        span: Span,
    ) {
        self.validate_expr(inner);
        // `self_ty` may be an unsized pointee marker (for example
        // `SlicePointee`), so validate its recursive type identity without
        // requiring a standalone ABI layout.
        self.validate_type(self_ty, span);
        self.validate_runtime_type(target_ty, span);
        let Some(target_readonly) = (match self.ty_kind(target_ty) {
            Some(TyKind::TraitObject { is_readonly, .. }) => Some(*is_readonly),
            _ => None,
        }) else {
            self.invalid_trait_object(span, "coercion target is not a trait object");
            return;
        };
        if !self.same_type(result_ty, target_ty) {
            self.invalid_trait_object(span, "coercion result type does not match target metadata");
        }
        let source = match self.ty_kind(inner.ty) {
            Some(TyKind::Pointer { is_readonly, elem }) => Some((*is_readonly, *elem)),
            Some(TyKind::Slice { is_readonly, elem }) => Some((*is_readonly, *elem)),
            _ => None,
        };
        let Some((source_readonly, source_elem)) = source else {
            self.invalid_trait_object(span, "coercion source is not a pointer or slice");
            return;
        };
        let source_matches_self = self.same_type(source_elem, self_ty)
            || match self.ty_kind(self_ty) {
                Some(TyKind::SlicePointee { elem }) => self.same_type(source_elem, *elem),
                _ => false,
            };
        if !source_matches_self {
            self.invalid_trait_object(span, "coercion self type does not match source element");
        }
        if !target_readonly && source_readonly {
            self.invalid_trait_object(span, "coercion cannot strengthen readonly access");
        }
        let key = nia_backend_ir::BackendTraitObjectVtableKey {
            self_ty,
            object_ty: target_ty,
        };
        if self.index.trait_object_vtable(&key).is_none() {
            self.invalid_trait_object(span, "coercion target vtable is missing");
        }
    }

    pub(super) fn invalid_trait_object(&mut self, span: Span, message: &'static str) {
        self.diagnostics.push(Diagnostic::internal_error_at(
            nia_diagnostic::codes::INVALID_BACKEND_IR,
            span,
            format!("backend IR trait-object expression has an invalid contract: {message}"),
        ));
    }
}
