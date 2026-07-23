// SPDX-License-Identifier: GPL-3.0-or-later
use nia_backend_ir::BackendField;
use nia_diagnostic::Diagnostic;
use nia_ids::{GlobalDefId, InternedTyId};
use nia_span::Span;
use nia_ty::{ConstGenericArg, TyKind};

use super::BackendValidator;

impl BackendValidator<'_> {
    pub(super) fn validate_field_init(
        &mut self,
        base_ty: InternedTyId,
        field: Option<GlobalDefId>,
        span: Span,
    ) {
        let Some(field) = field else {
            self.diagnostics.push(Diagnostic::internal_error_at(
                nia_diagnostic::codes::INVALID_BACKEND_IR,
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
        let Some((def_id, args, const_args)) = self.field_base_type(base_ty) else {
            self.diagnostics.push(Diagnostic::internal_error_at(
                nia_diagnostic::codes::INVALID_BACKEND_IR,
                span,
                "backend IR field base type is not nominal",
            ));
            return None;
        };
        let Some(fields) = self.aggregate_fields(def_id, &args, &const_args) else {
            self.diagnostics.push(Diagnostic::internal_error_at(
                nia_diagnostic::codes::INVALID_BACKEND_IR,
                span,
                format!("backend IR aggregate fields are missing for {def_id:?}"),
            ));
            return None;
        };
        if let Some(field) = fields.iter().find(|candidate| candidate.def_id == field) {
            return Some(field.ty);
        }
        self.diagnostics.push(Diagnostic::internal_error_at(
            nia_diagnostic::codes::INVALID_BACKEND_IR,
            span,
            format!("{message} {field:?}"),
        ));
        None
    }

    pub(super) fn validate_aggregate_def(
        &mut self,
        def_id: GlobalDefId,
        span: Span,
        message: &str,
    ) {
        if !self.index.has_struct(def_id)
            && !self.index.has_union(def_id)
            && !self.index.has_struct_instances(def_id)
            && !self.index.has_union_instances(def_id)
        {
            self.diagnostics.push(Diagnostic::internal_error_at(
                nia_diagnostic::codes::INVALID_BACKEND_IR,
                span,
                format!("{message} {def_id:?}"),
            ));
        }
    }

    pub(super) fn validate_enum_variant_ref(
        &mut self,
        def_id: GlobalDefId,
        span: Span,
        message: &str,
    ) {
        if !self.index.has_enum_variant(def_id) {
            self.diagnostics.push(Diagnostic::internal_error_at(
                nia_diagnostic::codes::INVALID_BACKEND_IR,
                span,
                format!("{message} {def_id:?}"),
            ));
        }
    }

    pub(super) fn array_elem_ty(&self, ty: InternedTyId) -> Option<InternedTyId> {
        match self.ty_kind(ty) {
            Some(TyKind::Array { elem, .. })
            | Some(TyKind::Pointer { elem, .. })
            | Some(TyKind::VolatilePointer { elem, .. })
            | Some(TyKind::Slice { elem, .. }) => Some(*elem),
            _ => None,
        }
    }

    pub(super) fn field_base_type(
        &self,
        ty: InternedTyId,
    ) -> Option<(GlobalDefId, Vec<InternedTyId>, Vec<ConstGenericArg>)> {
        match self.ty_kind(ty) {
            Some(TyKind::Nominal {
                def_id,
                args,
                const_args,
            }) => Some((*def_id, args.clone(), const_args.clone())),
            Some(TyKind::Pointer { elem, .. }) | Some(TyKind::VolatilePointer { elem, .. }) => {
                self.field_base_type(*elem)
            }
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
                self.index.global(*def_id).map(|item| item.ty)
            }
            nia_function_ir::FunctionPlaceBase::GlobalInstance {
                def_id,
                arg_module_id,
                args,
                const_args,
            } => self
                .index
                .global_instance(*def_id, *arg_module_id, args, const_args)
                .map(|item| item.ty),
            nia_function_ir::FunctionPlaceBase::Deref(expr) => match self.ty_kind(expr.ty) {
                Some(TyKind::Pointer { elem, .. }) | Some(TyKind::VolatilePointer { elem, .. }) => {
                    Some(*elem)
                }
                _ => Some(place.ty),
            },
            nia_function_ir::FunctionPlaceBase::Error => None,
        }
    }

    pub(super) fn aggregate_fields(
        &self,
        def_id: GlobalDefId,
        args: &[InternedTyId],
        const_args: &[ConstGenericArg],
    ) -> Option<&[BackendField]> {
        self.struct_fields(def_id, args, const_args)
            .or_else(|| self.union_fields(def_id, args, const_args))
    }

    fn struct_fields(
        &self,
        def_id: GlobalDefId,
        args: &[InternedTyId],
        const_args: &[ConstGenericArg],
    ) -> Option<&[BackendField]> {
        if let Some(item) = self.index.struct_instance(def_id, args, const_args) {
            return Some(&item.fields);
        }
        let key = (def_id, args.to_vec(), const_args.to_vec());
        if let Some(cached_args) = self.struct_fields_lookup_cache.borrow().get(&key).cloned() {
            if let Some(fields) = cached_args
                .as_deref()
                .and_then(|args| self.index.struct_instance(def_id, args, const_args))
                .map(|item| item.fields.as_slice())
            {
                return Some(fields);
            }
        } else {
            let matched_args = self
                .index
                .struct_instances_for(def_id)
                .find(|item| {
                    self.same_type_args(&item.args, args)
                        && item.const_args.as_slice() == const_args
                })
                .map(|item| item.args.clone());
            self.struct_fields_lookup_cache
                .borrow_mut()
                .insert(key, matched_args.clone());
            if let Some(matched_args) = matched_args {
                return self
                    .index
                    .struct_instance(def_id, &matched_args, const_args)
                    .map(|item| item.fields.as_slice());
            }
        }
        self.index
            .struct_item(def_id)
            .map(|item| item.fields.as_slice())
    }

    fn union_fields(
        &self,
        def_id: GlobalDefId,
        args: &[InternedTyId],
        const_args: &[ConstGenericArg],
    ) -> Option<&[BackendField]> {
        if let Some(item) = self.index.union_instance(def_id, args, const_args) {
            return Some(&item.fields);
        }
        let key = (def_id, args.to_vec(), const_args.to_vec());
        if let Some(cached_args) = self.union_fields_lookup_cache.borrow().get(&key).cloned() {
            if let Some(fields) = cached_args
                .as_deref()
                .and_then(|args| self.index.union_instance(def_id, args, const_args))
                .map(|item| item.fields.as_slice())
            {
                return Some(fields);
            }
        } else {
            let matched_args = self
                .index
                .union_instances_for(def_id)
                .find(|item| {
                    self.same_type_args(&item.args, args)
                        && item.const_args.as_slice() == const_args
                })
                .map(|item| item.args.clone());
            self.union_fields_lookup_cache
                .borrow_mut()
                .insert(key, matched_args.clone());
            if let Some(matched_args) = matched_args {
                return self
                    .index
                    .union_instance(def_id, &matched_args, const_args)
                    .map(|item| item.fields.as_slice());
            }
        }
        self.index
            .union_item(def_id)
            .map(|item| item.fields.as_slice())
    }
}
