// SPDX-License-Identifier: GPL-3.0-or-later
//! Enum expression and payload validation contracts.

use super::*;

impl BackendValidator<'_> {
    pub(super) fn validate_enum_variant_expr(
        &mut self,
        result_ty: nia_ids::InternedTyId,
        variant: nia_ids::GlobalDefId,
        fields: &[FunctionExpr],
        span: Span,
    ) {
        for field in fields {
            self.validate_expr(field);
        }
        let Some(info) = self.index.enum_variant_info(variant) else {
            self.validate_enum_variant_ref(
                variant,
                span,
                "backend IR expression references missing enum variant",
            );
            return;
        };
        self.validate_enum_variant_value(
            info.owner.backing_type,
            info.variant.value,
            info.index,
            span,
        );
        let owner = info.owner.def_id;
        let backing_type = info.owner.backing_type;
        let payload = info.variant.payload.clone();
        let Some(layout) = self.index.enum_layout(owner) else {
            self.invalid_enum(span, "variant owner has no enum layout");
            return;
        };
        // Fieldless enums use their backing integer directly; payload-bearing
        // enums use the nominal tagged aggregate. Derive that distinction from
        // the declared fields as well as the offset metadata so a scalar enum
        // remains valid when its layout has no payload fields.
        let has_payload = layout
            .variants
            .iter()
            .any(|variant| !variant.fields.is_empty());
        let result_matches = if has_payload {
            matches!(
                self.index.ty_kind(result_ty),
                Some(TyKind::Nominal { def_id, .. }) if *def_id == owner
            )
        } else {
            self.same_type(result_ty, backing_type)
                || matches!(
                    self.index.ty_kind(result_ty),
                    Some(TyKind::Nominal { def_id, .. }) if *def_id == owner
                )
        };
        if !result_matches {
            self.invalid_enum(
                span,
                "variant result type does not match its enum representation",
            );
        }

        let expected_fields = Self::enum_payload_types(&payload);
        if fields.len() != expected_fields.len() {
            self.invalid_enum(
                span,
                "variant payload field count does not match its declaration",
            );
        }
        for (field, expected_ty) in fields.iter().zip(expected_fields) {
            if !self.same_type(field.ty, expected_ty) {
                self.invalid_enum(
                    field.span,
                    "variant payload field type does not match its declaration",
                );
            }
        }
    }

    pub(super) fn validate_enum_constructor(
        &mut self,
        function_pointer_ty: nia_ids::InternedTyId,
        variant: nia_ids::GlobalDefId,
        span: Span,
    ) {
        let Some(info) = self.index.enum_variant_info(variant) else {
            self.validate_enum_variant_ref(
                variant,
                span,
                "backend IR expression references missing enum constructor variant",
            );
            return;
        };
        let expected_params = match &info.variant.payload {
            nia_backend_ir::BackendEnumVariantPayload::Unit => {
                self.invalid_enum(
                    span,
                    "unit variant cannot be used as a constructor function",
                );
                return;
            }
            nia_backend_ir::BackendEnumVariantPayload::Tuple(fields) => fields.clone(),
            nia_backend_ir::BackendEnumVariantPayload::Named(fields) => {
                fields.iter().map(|field| field.ty).collect()
            }
        };
        let owner_id = info.owner.def_id;
        let Some(TyKind::FunctionPointer {
            params,
            return_type,
            is_variadic,
        }) = self.ty_kind(function_pointer_ty).cloned()
        else {
            self.invalid_enum(span, "constructor value is not a function pointer");
            return;
        };
        if is_variadic {
            self.invalid_enum(span, "constructor function pointer is variadic");
        }
        if params.len() != expected_params.len()
            || !params
                .iter()
                .zip(expected_params)
                .all(|(actual, expected)| self.same_type(*actual, expected))
        {
            self.invalid_enum(
                span,
                "constructor function parameters do not match the variant payload",
            );
        }
        if !matches!(
            self.ty_kind(return_type),
            Some(TyKind::Nominal { def_id, .. }) if *def_id == owner_id
        ) {
            self.invalid_enum(
                span,
                "constructor function return type does not match the variant owner",
            );
        }
    }

    pub(super) fn validate_enum_variant_tag(
        &mut self,
        result_ty: nia_ids::InternedTyId,
        variant: nia_ids::GlobalDefId,
        span: Span,
    ) {
        let Some(info) = self.index.enum_variant_info(variant) else {
            self.validate_enum_variant_ref(
                variant,
                span,
                "backend IR expression references missing enum variant tag",
            );
            return;
        };
        self.validate_enum_variant_value(
            info.owner.backing_type,
            info.variant.value,
            info.index,
            span,
        );
        if !self.same_type(result_ty, info.owner.backing_type) {
            self.invalid_enum(
                span,
                "variant tag result does not match the enum backing type",
            );
        }
    }

    fn validate_enum_variant_value(
        &mut self,
        backing_type: nia_ids::InternedTyId,
        explicit_value: Option<i128>,
        index: usize,
        span: Span,
    ) {
        let value = explicit_value.unwrap_or(index as i128);
        let Some(TyKind::Primitive(primitive)) = self.ty_kind(backing_type) else {
            return;
        };
        let pointer_width = self
            .target
            .pointer_size
            .checked_mul(8)
            .and_then(|bits| u32::try_from(bits).ok())
            .unwrap_or(0);
        if !nia_ty::IntConst::from_i128(value).fits_primitive_int(*primitive, pointer_width) {
            self.invalid_enum(span, "variant value is outside its enum backing type");
        }
    }

    pub(super) fn validate_enum_tag(
        &mut self,
        result_ty: nia_ids::InternedTyId,
        value: &FunctionExpr,
        span: Span,
    ) {
        self.validate_expr(value);
        let expected_ty = match self.index.ty_kind(value.ty) {
            Some(TyKind::Nominal { def_id, .. }) => {
                let Some(item) = self.index.enum_item(*def_id) else {
                    self.invalid_enum(span, "tag input nominal type is not an enum");
                    return;
                };
                item.backing_type
            }
            Some(TyKind::Primitive(primitive)) if primitive.is_integer() => value.ty,
            _ => {
                self.invalid_enum(
                    span,
                    "tag input is not an enum value or integer representation",
                );
                return;
            }
        };
        if !self.same_type(result_ty, expected_ty) {
            self.invalid_enum(span, "tag result does not match the enum backing type");
        }
    }

    pub(super) fn validate_enum_payload_field(
        &mut self,
        result_ty: nia_ids::InternedTyId,
        value: &FunctionExpr,
        variant: nia_ids::GlobalDefId,
        field: usize,
        span: Span,
    ) {
        self.validate_expr(value);
        let Some(info) = self.index.enum_variant_info(variant) else {
            self.validate_enum_variant_ref(
                variant,
                span,
                "backend IR expression references missing enum payload variant",
            );
            return;
        };
        let owner = info.owner.def_id;
        let payload = info.variant.payload.clone();
        if !matches!(
            self.index.ty_kind(value.ty),
            Some(TyKind::Nominal { def_id, .. }) if *def_id == owner
        ) {
            self.invalid_enum(
                span,
                "payload projection input does not match the variant owner",
            );
        }
        if !self.index.enum_layout(owner).is_some_and(|layout| {
            layout
                .variants
                .iter()
                .any(|variant| !variant.fields.is_empty())
        }) {
            self.invalid_enum(span, "payload projection enum has no payload storage");
        }
        let fields = Self::enum_payload_types(&payload);
        let Some(expected_ty) = fields.get(field).copied() else {
            self.invalid_enum(span, "payload projection field index is out of bounds");
            return;
        };
        if !self.same_type(result_ty, expected_ty) {
            self.invalid_enum(
                span,
                "payload projection result does not match its field type",
            );
        }
    }

    fn enum_payload_types(
        payload: &nia_backend_ir::BackendEnumVariantPayload,
    ) -> Vec<nia_ids::InternedTyId> {
        match payload {
            nia_backend_ir::BackendEnumVariantPayload::Unit => Vec::new(),
            nia_backend_ir::BackendEnumVariantPayload::Tuple(fields) => fields.clone(),
            nia_backend_ir::BackendEnumVariantPayload::Named(fields) => {
                fields.iter().map(|field| field.ty).collect()
            }
        }
    }

    pub(super) fn invalid_enum(&mut self, span: Span, message: &'static str) {
        self.diagnostics.push(Diagnostic::internal_error_at(
            nia_diagnostic::codes::INVALID_BACKEND_IR,
            span,
            format!("backend IR enum expression has an invalid contract: {message}"),
        ));
    }
}
