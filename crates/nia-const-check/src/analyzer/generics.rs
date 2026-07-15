use super::*;

impl Analyzer<'_> {
    pub(super) fn infer_generics_from_tys(
        &mut self,
        span: Span,
        target_module_id: ModuleId,
        pattern_ty: InternedTyId,
        actual_ty: InternedTyId,
        substitutions: &mut SymbolMap<InternedTyId>,
    ) -> Result<(), ConstError> {
        let pattern_kind = self.active_ty_kind(pattern_ty);
        match pattern_kind {
            TyKind::GenericParam(name) => {
                let imported = self.import_ty_into_module(actual_ty, target_module_id)?;
                if let Some(existing) = substitutions.get(&name) {
                    if *existing != imported {
                        let name = self.symbol_name(name);
                        return Err(ConstError {
                            span,
                            message: format!(
                                "conflicting inferred const generic type argument `{name}`"
                            ),
                        });
                    }
                } else {
                    substitutions.insert(name, imported);
                }
            }
            TyKind::SelfParam => {}
            TyKind::Pointer { is_readonly, elem } => {
                if let Some(TyKind::Pointer {
                    is_readonly: actual_readonly,
                    elem: actual_elem,
                }) = self.ty_kind(actual_ty)
                    && is_readonly == actual_readonly
                {
                    self.infer_generics_from_tys(
                        span,
                        target_module_id,
                        elem,
                        actual_elem,
                        substitutions,
                    )?;
                }
            }
            TyKind::VolatilePointer { is_readonly, elem } => {
                if let Some(TyKind::VolatilePointer {
                    is_readonly: actual_readonly,
                    elem: actual_elem,
                }) = self.ty_kind(actual_ty)
                    && is_readonly == actual_readonly
                {
                    self.infer_generics_from_tys(
                        span,
                        target_module_id,
                        elem,
                        actual_elem,
                        substitutions,
                    )?;
                }
            }
            TyKind::Slice { is_readonly, elem } => {
                if let Some(TyKind::Slice {
                    is_readonly: actual_readonly,
                    elem: actual_elem,
                }) = self.ty_kind(actual_ty)
                    && is_readonly == actual_readonly
                {
                    self.infer_generics_from_tys(
                        span,
                        target_module_id,
                        elem,
                        actual_elem,
                        substitutions,
                    )?;
                }
            }
            TyKind::SlicePointee { elem } => {
                if let Some(TyKind::SlicePointee { elem: actual_elem }) = self.ty_kind(actual_ty) {
                    self.infer_generics_from_tys(
                        span,
                        target_module_id,
                        elem,
                        actual_elem,
                        substitutions,
                    )?;
                }
            }
            TyKind::Array { len, elem } => {
                if let Some(TyKind::Array {
                    len: actual_len,
                    elem: actual_elem,
                }) = self.ty_kind(actual_ty)
                    && len == actual_len
                {
                    self.infer_generics_from_tys(
                        span,
                        target_module_id,
                        elem,
                        actual_elem,
                        substitutions,
                    )?;
                }
            }
            TyKind::Range { kind, bound } => {
                if let Some(TyKind::Range {
                    kind: actual_kind,
                    bound: actual_bound,
                }) = self.ty_kind(actual_ty)
                    && kind == actual_kind
                    && let (Some(bound), Some(actual_bound)) = (bound, actual_bound)
                {
                    self.infer_generics_from_tys(
                        span,
                        target_module_id,
                        bound,
                        actual_bound,
                        substitutions,
                    )?;
                }
            }
            TyKind::FunctionPointer {
                params,
                return_type,
                is_variadic,
            } => {
                if let Some(TyKind::FunctionPointer {
                    params: actual_params,
                    return_type: actual_return_type,
                    is_variadic: actual_is_variadic,
                }) = self.ty_kind(actual_ty)
                    && is_variadic == actual_is_variadic
                    && params.len() == actual_params.len()
                {
                    for (param, actual_param) in params.into_iter().zip(actual_params) {
                        self.infer_generics_from_tys(
                            span,
                            target_module_id,
                            param,
                            actual_param,
                            substitutions,
                        )?;
                    }
                    self.infer_generics_from_tys(
                        span,
                        target_module_id,
                        return_type,
                        actual_return_type,
                        substitutions,
                    )?;
                }
            }
            TyKind::Optional { elem } => {
                if let Some(TyKind::Optional { elem: actual_elem }) = self.ty_kind(actual_ty) {
                    self.infer_generics_from_tys(
                        span,
                        target_module_id,
                        elem,
                        actual_elem,
                        substitutions,
                    )?;
                }
            }
            TyKind::ErrorUnion { error, value } => {
                if let Some(TyKind::ErrorUnion {
                    error: actual_error,
                    value: actual_value,
                }) = self.ty_kind(actual_ty)
                {
                    self.infer_generics_from_tys(
                        span,
                        target_module_id,
                        error,
                        actual_error,
                        substitutions,
                    )?;
                    self.infer_generics_from_tys(
                        span,
                        target_module_id,
                        value,
                        actual_value,
                        substitutions,
                    )?;
                }
            }
            TyKind::Nominal {
                def_id,
                args,
                const_args,
            } => {
                if let Some(TyKind::Nominal {
                    def_id: actual_def_id,
                    args: actual_args,
                    const_args: actual_const_args,
                }) = self.ty_kind(actual_ty)
                    && def_id == actual_def_id
                    && args.len() == actual_args.len()
                    && const_args == actual_const_args
                {
                    for (arg, actual_arg) in args.into_iter().zip(actual_args) {
                        self.infer_generics_from_tys(
                            span,
                            target_module_id,
                            arg,
                            actual_arg,
                            substitutions,
                        )?;
                    }
                }
            }
            TyKind::BuiltinTrait { args, .. } => {
                if let Some(TyKind::BuiltinTrait {
                    args: actual_args, ..
                }) = self.ty_kind(actual_ty)
                    && args.len() == actual_args.len()
                {
                    for (arg, actual_arg) in args.into_iter().zip(actual_args) {
                        self.infer_generics_from_tys(
                            span,
                            target_module_id,
                            arg,
                            actual_arg,
                            substitutions,
                        )?;
                    }
                }
            }
            TyKind::TraitObject {
                trait_args,
                associated_type_bindings,
                ..
            }
            | TyKind::TraitObjectPointee {
                trait_args,
                associated_type_bindings,
                ..
            } => {
                if let Some(
                    TyKind::TraitObject {
                        trait_args: actual_trait_args,
                        associated_type_bindings: actual_bindings,
                        ..
                    }
                    | TyKind::TraitObjectPointee {
                        trait_args: actual_trait_args,
                        associated_type_bindings: actual_bindings,
                        ..
                    },
                ) = self.ty_kind(actual_ty)
                    && trait_args.len() == actual_trait_args.len()
                    && associated_type_bindings.len() == actual_bindings.len()
                {
                    for (arg, actual_arg) in trait_args.into_iter().zip(actual_trait_args) {
                        self.infer_generics_from_tys(
                            span,
                            target_module_id,
                            arg,
                            actual_arg,
                            substitutions,
                        )?;
                    }
                    for (binding, actual_binding) in
                        associated_type_bindings.into_iter().zip(actual_bindings)
                    {
                        if binding.name == actual_binding.name {
                            self.infer_generics_from_tys(
                                span,
                                target_module_id,
                                binding.ty,
                                actual_binding.ty,
                                substitutions,
                            )?;
                        }
                    }
                }
            }
            TyKind::Projection {
                self_ty,
                trait_args,
                ..
            } => {
                if let Some(TyKind::Projection {
                    self_ty: actual_self_ty,
                    trait_args: actual_trait_args,
                    ..
                }) = self.ty_kind(actual_ty)
                    && trait_args.len() == actual_trait_args.len()
                {
                    self.infer_generics_from_tys(
                        span,
                        target_module_id,
                        self_ty,
                        actual_self_ty,
                        substitutions,
                    )?;
                    for (arg, actual_arg) in trait_args.into_iter().zip(actual_trait_args) {
                        self.infer_generics_from_tys(
                            span,
                            target_module_id,
                            arg,
                            actual_arg,
                            substitutions,
                        )?;
                    }
                }
            }
            TyKind::Error
            | TyKind::ConstOnly
            | TyKind::Primitive(_)
            | TyKind::BuiltinType(_)
            | TyKind::Vector { .. } => {}
        }
        Ok(())
    }

    pub(super) fn ty_kind(&self, ty: InternedTyId) -> Option<TyKind> {
        Some(self.active_ty_kind(ty))
    }

    pub(super) fn import_ty_into_module(
        &mut self,
        ty: InternedTyId,
        target_module_id: ModuleId,
    ) -> Result<InternedTyId, ConstError> {
        if self
            .working_interners
            .get(&target_module_id)
            .is_some_and(|target| target.get(ty).is_some())
        {
            return Ok(ty);
        }
        let source_interner = self.active_interner_for_type(ty);
        let target = self
            .working_interners
            .get_mut(&target_module_id)
            .expect("target working interner must exist");
        Ok(import_type_into(target, &source_interner, ty))
    }

    pub(super) fn import_ty_into_module_or_none(
        &mut self,
        ty: InternedTyId,
        target_module_id: ModuleId,
    ) -> Option<InternedTyId> {
        if self
            .working_interners
            .get(&target_module_id)
            .is_some_and(|target| target.get(ty).is_some())
        {
            return Some(ty);
        }
        let source_interner = self.active_interner_for_type(ty);
        let target = self.working_interners.get_mut(&target_module_id)?;
        Some(import_type_into(target, &source_interner, ty))
    }
}
