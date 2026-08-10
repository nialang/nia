use super::*;

struct TraitTypeParts<'a> {
    args: &'a [InternedTyId],
    const_args: &'a [ConstGenericArg],
    bindings: &'a [nia_ty::AssociatedTypeBindingTy],
}

impl Analyzer<'_> {
    pub(super) fn infer_generic_substitutions_from_tys(
        &mut self,
        span: Span,
        target_module_id: ModuleId,
        pattern_ty: InternedTyId,
        actual_ty: InternedTyId,
        type_substitutions: &mut SymbolMap<InternedTyId>,
        const_substitutions: &mut SymbolMap<ConstGenericArg>,
    ) -> Result<(), ConstError> {
        self.infer_generics_from_tys(
            span,
            target_module_id,
            pattern_ty,
            actual_ty,
            type_substitutions,
        )?;
        self.infer_const_generics_from_tys(
            span,
            target_module_id,
            pattern_ty,
            actual_ty,
            const_substitutions,
        )
    }

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
                let canonical = self.type_for_module(actual_ty, target_module_id)?;
                if let Some(existing) = substitutions.get(&name) {
                    if *existing != canonical {
                        let name = self.symbol_name(name);
                        return Err(ConstError {
                            span,
                            message: format!(
                                "conflicting inferred const generic type argument `{name}`"
                            ),
                        });
                    }
                } else {
                    substitutions.insert(name, canonical);
                }
            }
            TyKind::SelfParam => {}
            TyKind::Opaque => {}
            TyKind::Tuple(elems) => {
                if let Some(TyKind::Tuple(actual_elems)) = self.ty_kind(actual_ty)
                    && elems.len() == actual_elems.len()
                {
                    for (elem, actual_elem) in elems.into_iter().zip(actual_elems) {
                        self.infer_generics_from_tys(
                            span,
                            target_module_id,
                            elem,
                            actual_elem,
                            substitutions,
                        )?;
                    }
                }
            }
            TyKind::ClosureState {
                closure_id,
                captures,
                params,
                return_type,
            } => {
                if let Some(TyKind::ClosureState {
                    closure_id: actual_id,
                    captures: actual_captures,
                    params: actual_params,
                    return_type: actual_return,
                }) = self.ty_kind(actual_ty)
                    && closure_id == actual_id
                    && captures.len() == actual_captures.len()
                    && params.len() == actual_params.len()
                {
                    for (pattern, actual) in captures.into_iter().zip(actual_captures) {
                        self.infer_generics_from_tys(
                            span,
                            target_module_id,
                            pattern,
                            actual,
                            substitutions,
                        )?;
                    }
                    for (pattern, actual) in params.into_iter().zip(actual_params) {
                        self.infer_generics_from_tys(
                            span,
                            target_module_id,
                            pattern,
                            actual,
                            substitutions,
                        )?;
                    }
                    self.infer_generics_from_tys(
                        span,
                        target_module_id,
                        return_type,
                        actual_return,
                        substitutions,
                    )?;
                }
            }
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
                    && (matches!(len, ArrayLenTy::GenericParam(_))
                        || len == actual_len
                        || matches!(
                            (
                                self.array_len_const_value(len),
                                self.array_len_const_value(actual_len),
                            ),
                            (Some(pattern), Some(actual)) if pattern == actual
                        ))
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
            TyKind::Callable {
                is_readonly,
                params,
                return_type,
            } => {
                if let Some(TyKind::Callable {
                    is_readonly: actual_readonly,
                    params: actual_params,
                    return_type: actual_return_type,
                }) = self.ty_kind(actual_ty)
                    && is_readonly == actual_readonly
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
            TyKind::CallablePointee {
                params,
                return_type,
            } => {
                if let Some(TyKind::CallablePointee {
                    params: actual_params,
                    return_type: actual_return_type,
                }) = self.ty_kind(actual_ty)
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
                    && const_args.len() == actual_const_args.len()
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

    fn infer_const_generics_from_tys(
        &mut self,
        span: Span,
        target_module_id: ModuleId,
        pattern_ty: InternedTyId,
        actual_ty: InternedTyId,
        substitutions: &mut SymbolMap<ConstGenericArg>,
    ) -> Result<(), ConstError> {
        let pattern_kind = self.active_ty_kind(pattern_ty);
        let actual_kind = self.active_ty_kind(actual_ty);
        match (pattern_kind, actual_kind) {
            (
                TyKind::Array {
                    len: pattern_len,
                    elem: pattern_elem,
                },
                TyKind::Array {
                    len: actual_len,
                    elem: actual_elem,
                },
            ) => {
                self.infer_const_generic_from_array_len(
                    span,
                    target_module_id,
                    pattern_len,
                    actual_len,
                    substitutions,
                )?;
                self.infer_const_generics_from_tys(
                    span,
                    target_module_id,
                    pattern_elem,
                    actual_elem,
                    substitutions,
                )?;
            }
            (
                TyKind::Pointer {
                    is_readonly: pattern_readonly,
                    elem: pattern_elem,
                },
                TyKind::Pointer {
                    is_readonly: actual_readonly,
                    elem: actual_elem,
                },
            )
            | (
                TyKind::VolatilePointer {
                    is_readonly: pattern_readonly,
                    elem: pattern_elem,
                },
                TyKind::VolatilePointer {
                    is_readonly: actual_readonly,
                    elem: actual_elem,
                },
            )
            | (
                TyKind::Slice {
                    is_readonly: pattern_readonly,
                    elem: pattern_elem,
                },
                TyKind::Slice {
                    is_readonly: actual_readonly,
                    elem: actual_elem,
                },
            ) if pattern_readonly == actual_readonly => {
                self.infer_const_generics_from_tys(
                    span,
                    target_module_id,
                    pattern_elem,
                    actual_elem,
                    substitutions,
                )?;
            }
            (
                TyKind::SlicePointee { elem: pattern_elem },
                TyKind::SlicePointee { elem: actual_elem },
            )
            | (TyKind::Optional { elem: pattern_elem }, TyKind::Optional { elem: actual_elem }) => {
                self.infer_const_generics_from_tys(
                    span,
                    target_module_id,
                    pattern_elem,
                    actual_elem,
                    substitutions,
                )?;
            }
            (
                TyKind::Range {
                    kind: pattern_kind,
                    bound: Some(pattern_bound),
                },
                TyKind::Range {
                    kind: actual_kind,
                    bound: Some(actual_bound),
                },
            ) if pattern_kind == actual_kind => {
                self.infer_const_generics_from_tys(
                    span,
                    target_module_id,
                    pattern_bound,
                    actual_bound,
                    substitutions,
                )?;
            }
            (
                TyKind::FunctionPointer {
                    params: pattern_params,
                    return_type: pattern_return,
                    is_variadic: pattern_variadic,
                },
                TyKind::FunctionPointer {
                    params: actual_params,
                    return_type: actual_return,
                    is_variadic: actual_variadic,
                },
            ) if pattern_variadic == actual_variadic
                && pattern_params.len() == actual_params.len() =>
            {
                for (pattern, actual) in pattern_params.into_iter().zip(actual_params) {
                    self.infer_const_generics_from_tys(
                        span,
                        target_module_id,
                        pattern,
                        actual,
                        substitutions,
                    )?;
                }
                self.infer_const_generics_from_tys(
                    span,
                    target_module_id,
                    pattern_return,
                    actual_return,
                    substitutions,
                )?;
            }
            (
                TyKind::Callable {
                    is_readonly: pattern_readonly,
                    params: pattern_params,
                    return_type: pattern_return,
                },
                TyKind::Callable {
                    is_readonly: actual_readonly,
                    params: actual_params,
                    return_type: actual_return,
                },
            ) if pattern_readonly == actual_readonly
                && pattern_params.len() == actual_params.len() =>
            {
                for (pattern, actual) in pattern_params.into_iter().zip(actual_params) {
                    self.infer_const_generics_from_tys(
                        span,
                        target_module_id,
                        pattern,
                        actual,
                        substitutions,
                    )?;
                }
                self.infer_const_generics_from_tys(
                    span,
                    target_module_id,
                    pattern_return,
                    actual_return,
                    substitutions,
                )?;
            }
            (
                TyKind::CallablePointee {
                    params: pattern_params,
                    return_type: pattern_return,
                },
                TyKind::CallablePointee {
                    params: actual_params,
                    return_type: actual_return,
                },
            ) if pattern_params.len() == actual_params.len() => {
                for (pattern, actual) in pattern_params.into_iter().zip(actual_params) {
                    self.infer_const_generics_from_tys(
                        span,
                        target_module_id,
                        pattern,
                        actual,
                        substitutions,
                    )?;
                }
                self.infer_const_generics_from_tys(
                    span,
                    target_module_id,
                    pattern_return,
                    actual_return,
                    substitutions,
                )?;
            }
            (
                TyKind::ErrorUnion {
                    error: pattern_error,
                    value: pattern_value,
                },
                TyKind::ErrorUnion {
                    error: actual_error,
                    value: actual_value,
                },
            ) => {
                self.infer_const_generics_from_tys(
                    span,
                    target_module_id,
                    pattern_error,
                    actual_error,
                    substitutions,
                )?;
                self.infer_const_generics_from_tys(
                    span,
                    target_module_id,
                    pattern_value,
                    actual_value,
                    substitutions,
                )?;
            }
            (
                TyKind::Nominal {
                    def_id: pattern_def,
                    args: pattern_args,
                    const_args: pattern_const_args,
                },
                TyKind::Nominal {
                    def_id: actual_def,
                    args: actual_args,
                    const_args: actual_const_args,
                },
            ) if pattern_def == actual_def
                && pattern_args.len() == actual_args.len()
                && pattern_const_args.len() == actual_const_args.len() =>
            {
                self.infer_const_generics_from_type_args(
                    span,
                    target_module_id,
                    &pattern_args,
                    &actual_args,
                    substitutions,
                )?;
                self.infer_const_generics_from_args(
                    span,
                    target_module_id,
                    &pattern_const_args,
                    &actual_const_args,
                    substitutions,
                )?;
            }
            (
                TyKind::BuiltinTrait {
                    trait_id: pattern_trait,
                    args: pattern_args,
                },
                TyKind::BuiltinTrait {
                    trait_id: actual_trait,
                    args: actual_args,
                },
            ) if pattern_trait == actual_trait && pattern_args.len() == actual_args.len() => {
                self.infer_const_generics_from_type_args(
                    span,
                    target_module_id,
                    &pattern_args,
                    &actual_args,
                    substitutions,
                )?;
            }
            (
                TyKind::TraitObject {
                    is_readonly: pattern_readonly,
                    trait_id: pattern_trait,
                    trait_args: pattern_args,
                    trait_const_args: pattern_const_args,
                    associated_type_bindings: pattern_bindings,
                },
                TyKind::TraitObject {
                    is_readonly: actual_readonly,
                    trait_id: actual_trait,
                    trait_args: actual_args,
                    trait_const_args: actual_const_args,
                    associated_type_bindings: actual_bindings,
                },
            ) if pattern_readonly == actual_readonly && pattern_trait == actual_trait => {
                self.infer_const_generics_from_trait_type(
                    span,
                    target_module_id,
                    TraitTypeParts {
                        args: &pattern_args,
                        const_args: &pattern_const_args,
                        bindings: &pattern_bindings,
                    },
                    TraitTypeParts {
                        args: &actual_args,
                        const_args: &actual_const_args,
                        bindings: &actual_bindings,
                    },
                    substitutions,
                )?;
            }
            (
                TyKind::TraitObjectPointee {
                    trait_id: pattern_trait,
                    trait_args: pattern_args,
                    trait_const_args: pattern_const_args,
                    associated_type_bindings: pattern_bindings,
                },
                TyKind::TraitObjectPointee {
                    trait_id: actual_trait,
                    trait_args: actual_args,
                    trait_const_args: actual_const_args,
                    associated_type_bindings: actual_bindings,
                },
            ) if pattern_trait == actual_trait => {
                self.infer_const_generics_from_trait_type(
                    span,
                    target_module_id,
                    TraitTypeParts {
                        args: &pattern_args,
                        const_args: &pattern_const_args,
                        bindings: &pattern_bindings,
                    },
                    TraitTypeParts {
                        args: &actual_args,
                        const_args: &actual_const_args,
                        bindings: &actual_bindings,
                    },
                    substitutions,
                )?;
            }
            (
                TyKind::Projection {
                    self_ty: pattern_self,
                    trait_id: pattern_trait,
                    trait_args: pattern_args,
                    trait_const_args: pattern_const_args,
                    name: pattern_name,
                },
                TyKind::Projection {
                    self_ty: actual_self,
                    trait_id: actual_trait,
                    trait_args: actual_args,
                    trait_const_args: actual_const_args,
                    name: actual_name,
                },
            ) if pattern_trait == actual_trait && pattern_name == actual_name => {
                self.infer_const_generics_from_tys(
                    span,
                    target_module_id,
                    pattern_self,
                    actual_self,
                    substitutions,
                )?;
                self.infer_const_generics_from_type_args(
                    span,
                    target_module_id,
                    &pattern_args,
                    &actual_args,
                    substitutions,
                )?;
                self.infer_const_generics_from_args(
                    span,
                    target_module_id,
                    &pattern_const_args,
                    &actual_const_args,
                    substitutions,
                )?;
            }
            _ => {}
        }
        Ok(())
    }

    fn infer_const_generics_from_trait_type(
        &mut self,
        span: Span,
        target_module_id: ModuleId,
        pattern: TraitTypeParts<'_>,
        actual: TraitTypeParts<'_>,
        substitutions: &mut SymbolMap<ConstGenericArg>,
    ) -> Result<(), ConstError> {
        if pattern.args.len() != actual.args.len()
            || pattern.const_args.len() != actual.const_args.len()
            || pattern.bindings.len() != actual.bindings.len()
        {
            return Ok(());
        }
        self.infer_const_generics_from_type_args(
            span,
            target_module_id,
            pattern.args,
            actual.args,
            substitutions,
        )?;
        self.infer_const_generics_from_args(
            span,
            target_module_id,
            pattern.const_args,
            actual.const_args,
            substitutions,
        )?;
        for pattern_binding in pattern.bindings {
            let Some(actual_binding) = actual.bindings.iter().find(|actual| {
                actual.trait_id == pattern_binding.trait_id && actual.name == pattern_binding.name
            }) else {
                continue;
            };
            self.infer_const_generics_from_tys(
                span,
                target_module_id,
                pattern_binding.ty,
                actual_binding.ty,
                substitutions,
            )?;
            self.infer_const_generics_from_type_args(
                span,
                target_module_id,
                &pattern_binding.trait_args,
                &actual_binding.trait_args,
                substitutions,
            )?;
            self.infer_const_generics_from_args(
                span,
                target_module_id,
                &pattern_binding.trait_const_args,
                &actual_binding.trait_const_args,
                substitutions,
            )?;
        }
        Ok(())
    }

    fn infer_const_generics_from_type_args(
        &mut self,
        span: Span,
        target_module_id: ModuleId,
        pattern_args: &[InternedTyId],
        actual_args: &[InternedTyId],
        substitutions: &mut SymbolMap<ConstGenericArg>,
    ) -> Result<(), ConstError> {
        if pattern_args.len() != actual_args.len() {
            return Ok(());
        }
        for (pattern, actual) in pattern_args.iter().zip(actual_args) {
            self.infer_const_generics_from_tys(
                span,
                target_module_id,
                *pattern,
                *actual,
                substitutions,
            )?;
        }
        Ok(())
    }

    fn infer_const_generics_from_args(
        &mut self,
        span: Span,
        target_module_id: ModuleId,
        pattern_args: &[ConstGenericArg],
        actual_args: &[ConstGenericArg],
        substitutions: &mut SymbolMap<ConstGenericArg>,
    ) -> Result<(), ConstError> {
        if pattern_args.len() != actual_args.len() {
            return Ok(());
        }
        for (pattern, actual) in pattern_args.iter().zip(actual_args) {
            self.infer_const_generics_from_tys(
                span,
                target_module_id,
                pattern.ty,
                actual.ty,
                substitutions,
            )?;
            let ConstGenericValue::GenericParam(name) = pattern.value else {
                continue;
            };
            let mut actual = actual.clone();
            if let Some(value) = self.resolve_const_generic_arg_for_execution(&actual) {
                actual.value = value;
            }
            actual.ty = self.type_for_module(actual.ty, target_module_id)?;
            self.record_const_generic_substitution(span, name, actual, substitutions)?;
        }
        Ok(())
    }

    fn infer_const_generic_from_array_len(
        &mut self,
        span: Span,
        target_module_id: ModuleId,
        pattern: ArrayLenTy,
        actual: ArrayLenTy,
        substitutions: &mut SymbolMap<ConstGenericArg>,
    ) -> Result<(), ConstError> {
        let ArrayLenTy::GenericParam(name) = pattern else {
            return Ok(());
        };
        let value = match actual {
            ArrayLenTy::ConstValue(value) => {
                Some(ConstGenericValue::Int(IntConst::unsigned(value.into())))
            }
            ArrayLenTy::ConstExpr(id) => self
                .array_len_const_value(ArrayLenTy::ConstExpr(id))
                .map(|value| ConstGenericValue::Int(IntConst::unsigned(value.into()))),
            ArrayLenTy::Builtin { builtin, ty } => {
                let ConstValue::Int(value) =
                    self.resolve_layout_builtin_for_ty(span, builtin, ty)?
                else {
                    return Ok(());
                };
                Some(ConstGenericValue::Int(value))
            }
            ArrayLenTy::GenericParam(name) => Some(ConstGenericValue::GenericParam(name)),
            ArrayLenTy::Infer => None,
        };
        let Some(value) = value else {
            return Ok(());
        };
        let ty = self.primitive_ty_for_module(target_module_id, PrimitiveTy::Usize);
        self.record_const_generic_substitution(
            span,
            name,
            ConstGenericArg { ty, value },
            substitutions,
        )
    }

    fn record_const_generic_substitution(
        &mut self,
        span: Span,
        name: SymbolId,
        arg: ConstGenericArg,
        substitutions: &mut SymbolMap<ConstGenericArg>,
    ) -> Result<(), ConstError> {
        if let Some(existing) = substitutions.get(&name).cloned() {
            if !self.const_generic_values_match_for_execution(&existing, &arg) {
                let name = self.symbol_name(name);
                return Err(ConstError {
                    span,
                    message: format!(
                        "conflicting inferred value for const generic parameter `{name}`"
                    ),
                });
            }
        } else {
            substitutions.insert(name, arg);
        }
        Ok(())
    }

    pub(super) fn type_for_module(
        &mut self,
        ty: InternedTyId,
        target_module_id: ModuleId,
    ) -> Result<InternedTyId, ConstError> {
        self.type_contexts
            .get(&target_module_id)
            .expect("target type context must exist");
        assert!(
            self.input.type_store.get(ty).is_some(),
            "Nia ICE: const type belongs to a foreign type store"
        );
        Ok(ty)
    }

    pub(super) fn type_for_module_or_none(
        &mut self,
        ty: InternedTyId,
        target_module_id: ModuleId,
    ) -> Option<InternedTyId> {
        self.type_contexts.get(&target_module_id)?;
        self.input.type_store.get(ty).map(|_| ty)
    }
}
