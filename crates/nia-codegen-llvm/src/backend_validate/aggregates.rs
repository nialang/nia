// SPDX-License-Identifier: GPL-3.0-or-later
use nia_backend_ir::BackendField;
use nia_diagnostic::Diagnostic;
use nia_ids::{GlobalDefId, InternedTyId};
use nia_span::Span;
use nia_ty::TyKind;

use super::BackendValidator;

impl BackendValidator<'_> {
    pub(super) fn validate_field_init(
        &mut self,
        base_ty: InternedTyId,
        field: Option<GlobalDefId>,
        span: Span,
    ) {
        let Some(field) = field else {
            self.diagnostics.push(Diagnostic::error(
                span,
                "backend IR aggregate literal has invalid field",
            ));
            return;
        };
        self.validate_aggregate_field(
            base_ty,
            field,
            span,
            "backend IR aggregate literal references missing field",
        );
    }

    pub(super) fn validate_aggregate_field(
        &mut self,
        base_ty: InternedTyId,
        field: GlobalDefId,
        span: Span,
        message: &str,
    ) -> Option<InternedTyId> {
        let Some((def_id, args)) = self.field_base_type(base_ty) else {
            self.diagnostics.push(Diagnostic::error(
                span,
                "backend IR field base type is not nominal",
            ));
            return None;
        };
        let Some(fields) = self.aggregate_fields(def_id, &args) else {
            self.diagnostics.push(Diagnostic::error(
                span,
                format!("backend IR aggregate fields are missing for {def_id:?}"),
            ));
            return None;
        };
        if let Some(field) = fields.iter().find(|candidate| candidate.def_id == field) {
            return Some(field.ty);
        }
        self.diagnostics
            .push(Diagnostic::error(span, format!("{message} {field:?}")));
        None
    }

    pub(super) fn validate_aggregate_def(
        &mut self,
        def_id: GlobalDefId,
        span: Span,
        message: &str,
    ) {
        if !self.index.structs.contains_key(&def_id)
            && !self.index.unions.contains_key(&def_id)
            && !self.index.struct_instances_by_def.contains_key(&def_id)
            && !self.index.union_instances_by_def.contains_key(&def_id)
        {
            self.diagnostics
                .push(Diagnostic::error(span, format!("{message} {def_id:?}")));
        }
    }

    pub(super) fn validate_enum_variant_ref(
        &mut self,
        def_id: GlobalDefId,
        span: Span,
        message: &str,
    ) {
        if !self.index.enum_variants.contains_key(&def_id) {
            self.diagnostics
                .push(Diagnostic::error(span, format!("{message} {def_id:?}")));
        }
    }

    pub(super) fn array_elem_ty(&self, ty: InternedTyId) -> Option<InternedTyId> {
        match self.ty_kind(ty) {
            Some(TyKind::Array { elem, .. }) => Some(*elem),
            _ => None,
        }
    }

    pub(super) fn field_base_type(
        &self,
        ty: InternedTyId,
    ) -> Option<(GlobalDefId, Vec<InternedTyId>)> {
        match self.ty_kind(ty) {
            Some(TyKind::Nominal { def_id, args }) => Some((*def_id, args.clone())),
            Some(TyKind::Pointer { elem, .. }) => self.field_base_type(*elem),
            _ => None,
        }
    }

    pub(super) fn place_base_ty(
        &self,
        place: &nia_function_ir::FunctionPlace,
    ) -> Option<InternedTyId> {
        match &place.base {
            nia_function_ir::FunctionPlaceBase::Local(local_id) => self
                .local_tys
                .last()
                .and_then(|local_tys| local_tys.get(local_id).copied()),
            nia_function_ir::FunctionPlaceBase::Global(def_id) => {
                self.index.globals.get(def_id).map(|item| item.ty)
            }
            nia_function_ir::FunctionPlaceBase::Deref(expr) => match self.ty_kind(expr.ty) {
                Some(TyKind::Pointer { elem, .. }) => Some(*elem),
                _ => Some(place.ty),
            },
            nia_function_ir::FunctionPlaceBase::Error => None,
        }
    }

    pub(super) fn aggregate_fields(
        &self,
        def_id: GlobalDefId,
        args: &[InternedTyId],
    ) -> Option<&[BackendField]> {
        self.struct_fields(def_id, args)
            .or_else(|| self.union_fields(def_id, args))
    }

    fn struct_fields(&self, def_id: GlobalDefId, args: &[InternedTyId]) -> Option<&[BackendField]> {
        if let Some(item) = self
            .index
            .struct_instances_by_def
            .get(&def_id)
            .into_iter()
            .flatten()
            .find(|item| self.same_type_args(&item.args, args))
        {
            return Some(&item.fields);
        }
        self.index
            .structs
            .get(&def_id)
            .map(|item| item.fields.as_slice())
    }

    fn union_fields(&self, def_id: GlobalDefId, args: &[InternedTyId]) -> Option<&[BackendField]> {
        if let Some(item) = self
            .index
            .union_instances_by_def
            .get(&def_id)
            .into_iter()
            .flatten()
            .find(|item| self.same_type_args(&item.args, args))
        {
            return Some(&item.fields);
        }
        self.index
            .unions
            .get(&def_id)
            .map(|item| item.fields.as_slice())
    }
}
