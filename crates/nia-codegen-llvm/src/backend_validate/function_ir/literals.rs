// SPDX-License-Identifier: GPL-3.0-or-later
//! Aggregate and scalar literal validation contracts.

use super::*;

impl BackendValidator<'_> {
    pub(super) fn has_direct_aggregate_field_contract(&self, ty: nia_ids::InternedTyId) -> bool {
        let Some((def_id, args, const_args)) = self.field_base_type(ty) else {
            return false;
        };
        if args.is_empty() && const_args.is_empty() {
            return true;
        }
        self.index
            .struct_instance(def_id, &args, &const_args)
            .is_some()
            || self
                .index
                .union_instance(def_id, &args, &const_args)
                .is_some()
            || self.index.struct_instances_for(def_id).any(|item| {
                self.same_type_args(&item.args, &args)
                    && self.same_const_args(&item.const_args, &const_args)
            })
            || self.index.union_instances_for(def_id).any(|item| {
                self.same_type_args(&item.args, &args)
                    && self.same_const_args(&item.const_args, &const_args)
            })
    }

    pub(super) fn validate_aggregate_literal_identity(
        &mut self,
        kind: &'static str,
        result_ty: nia_ids::InternedTyId,
        def_id: nia_ids::GlobalDefId,
        span: Span,
    ) {
        if !matches!(self.ty_kind(result_ty), Some(TyKind::Nominal { def_id: result_def, .. }) if *result_def == def_id)
        {
            self.invalid_literal_contract(span, kind, "definition does not match expression type");
        }
        let valid_kind = match kind {
            "struct" => self.index.has_struct(def_id) || self.index.has_struct_instances(def_id),
            "union" => self.index.has_union(def_id) || self.index.has_union_instances(def_id),
            _ => false,
        };
        if !valid_kind {
            self.invalid_literal_contract(span, kind, "definition has the wrong aggregate kind");
        }
    }

    pub(super) fn validate_struct_literal_field_coverage(
        &mut self,
        result_ty: nia_ids::InternedTyId,
        fields: &[nia_function_ir::FunctionFieldInit],
        span: Span,
    ) {
        let Some((def_id, args, const_args)) = self.field_base_type(result_ty) else {
            return;
        };
        let Some(expected) = self.aggregate_fields(def_id, &args, &const_args) else {
            return;
        };
        let supplied = fields
            .iter()
            .filter_map(|field| field.field)
            .collect::<HashSet<_>>();
        let covers_exactly = supplied.len() == fields.len()
            && supplied.len() == expected.len()
            && supplied
                .iter()
                .all(|field| expected.iter().any(|candidate| candidate.def_id == *field));
        if !covers_exactly {
            self.invalid_literal_contract(
                span,
                "struct",
                "fields do not initialize each declared field exactly once",
            );
        }
    }

    pub(super) fn invalid_literal_contract(
        &mut self,
        span: Span,
        kind: &'static str,
        message: &'static str,
    ) {
        self.diagnostics.push(Diagnostic::internal_error_at(
            nia_diagnostic::codes::INVALID_BACKEND_IR,
            span,
            format!("backend IR {kind} literal has an invalid type contract: {message}"),
        ));
    }

    pub(super) fn validate_integer_literal(
        &mut self,
        ty: nia_ids::InternedTyId,
        text: &str,
        span: Span,
    ) {
        let Some(TyKind::Primitive(primitive)) = self.index.ty_kind(ty) else {
            self.invalid_literal_contract(span, "integer", "target type is not an integer");
            return;
        };
        let primitive = *primitive;
        if !primitive.is_integer() && !matches!(primitive, PrimitiveTy::Bool | PrimitiveTy::Char) {
            self.invalid_literal_contract(span, "integer", "target type is not an integer");
            return;
        }
        let Some(value) = parse_int_literal(text) else {
            self.invalid_literal_contract(span, "integer", "spelling is invalid");
            return;
        };
        if !self.integer_literal_fits(primitive, value) {
            self.invalid_literal_contract(span, "integer", "value is outside its target type");
        }
    }

    fn integer_literal_fits(&self, primitive: PrimitiveTy, value: nia_ty::IntConst) -> bool {
        match primitive {
            PrimitiveTy::Bool => !value.is_signed() && matches!(value.bits(), 0 | 1),
            PrimitiveTy::Char => {
                !value.is_signed()
                    && u32::try_from(value.bits())
                        .ok()
                        .and_then(char::from_u32)
                        .is_some()
            }
            primitive if primitive.is_integer() => value.fits_primitive_int(
                primitive,
                self.target
                    .pointer_size
                    .checked_mul(8)
                    .and_then(|bits| u32::try_from(bits).ok())
                    .unwrap_or(0),
            ),
            _ => false,
        }
    }

    pub(super) fn validate_float_literal(
        &mut self,
        ty: nia_ids::InternedTyId,
        text: &str,
        span: Span,
    ) {
        let primitive = match self.index.ty_kind(ty) {
            Some(TyKind::Primitive(primitive @ (PrimitiveTy::F32 | PrimitiveTy::F64))) => {
                *primitive
            }
            _ => {
                self.invalid_literal_contract(span, "float", "target type is not f32 or f64");
                return;
            }
        };
        let Some(value) = parse_float_literal(text) else {
            self.invalid_literal_contract(span, "float", "spelling is invalid");
            return;
        };
        if !value.is_finite()
            || matches!(primitive, PrimitiveTy::F32) && !(value as f32).is_finite()
        {
            self.invalid_literal_contract(span, "float", "value is outside its target type");
        }
    }

    pub(super) fn validate_string_scalars(&mut self, scalars: &[u32], span: Span) {
        if scalars
            .iter()
            .any(|scalar| char::from_u32(*scalar).is_none())
        {
            self.invalid_literal_contract(
                span,
                "string",
                "value contains an invalid Unicode scalar",
            );
        }
    }

    pub(super) fn validate_string_literal(
        &mut self,
        ty: nia_ids::InternedTyId,
        length: usize,
        span: Span,
        bytes: bool,
    ) {
        let kind = if bytes { "byte string" } else { "string" };
        let expected_elem = if bytes {
            PrimitiveTy::U8
        } else {
            PrimitiveTy::Char
        };
        let Some(TyKind::Array { len, elem }) = self.index.ty_kind(ty).cloned() else {
            self.invalid_literal_contract(span, kind, "target type is not an array");
            return;
        };
        if !matches!(self.index.ty_kind(elem), Some(TyKind::Primitive(actual)) if *actual == expected_elem)
        {
            self.invalid_literal_contract(span, kind, "array element type does not match literal");
        }
        if self
            .array_len_value(&len)
            .is_some_and(|expected| u64::try_from(length) != Ok(expected))
        {
            self.invalid_literal_contract(span, kind, "array length does not match literal");
        }
    }
}
