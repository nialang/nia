//! Structural generic inference for const execution.
//!
//! Const calls are instantiated after the ordinary body checker has produced
//! resolved const IR, but the evaluator still needs concrete substitutions for
//! the callee's signature. Inference therefore walks a *pattern* type from the
//! signature beside the actual argument type. A constructor mismatch contributes
//! no evidence; seeing the same parameter with two incompatible actual types is
//! an error.
//!
//! Type and value parameters are collected by separate recursive walks. Keep the
//! IR shape guards in those walks aligned: neither walk may descend through a
//! different nominal/trait/projection identity or through incompatible concrete
//! const arguments. Otherwise an unrelated type can supply a plausible-looking
//! substitution before the evaluator has a chance to reject the call.

use super::*;

struct TraitTypeParts<'a> {
    args: &'a [InternedTyId],
    const_args: &'a [ConstGenericArg],
    bindings: &'a [nia_ty::AssociatedTypeBindingTy],
}

/// Whether an actual pointer can satisfy a pattern pointer during inference.
///
/// A mutable pointer can be viewed through a readonly parameter, but a readonly
/// pointer cannot satisfy a mutable parameter. This must mirror the coercion
/// accepted by ordinary call checking or const calls infer different generic
/// arguments from otherwise identical source calls.
fn readonly_pointer_accepts(pattern_readonly: bool, actual_readonly: bool) -> bool {
    pattern_readonly == actual_readonly || (pattern_readonly && !actual_readonly)
}

impl Analyzer<'_> {
    fn associated_binding_types_match_after_inference(
        &mut self,
        pattern: &nia_ty::AssociatedTypeBindingTy,
        actual: &nia_ty::AssociatedTypeBindingTy,
        substitutions: &SymbolMap<InternedTyId>,
    ) -> bool {
        pattern
            .trait_args
            .iter()
            .zip(&actual.trait_args)
            .chain(std::iter::once((&pattern.ty, &actual.ty)))
            .all(|(pattern, actual)| {
                let pattern = self.substitute_ty_generics_from_map(*pattern, substitutions);
                self.inference_pattern_accepts_type_shape(pattern, *actual)
            })
    }

    fn const_generic_args_allow_inference(
        &mut self,
        pattern: &[ConstGenericArg],
        actual: &[ConstGenericArg],
    ) -> bool {
        pattern.len() == actual.len()
            && pattern.iter().zip(actual).all(|(pattern, actual)| {
                let types_compatible =
                    self.inference_pattern_accepts_type_shape(pattern.ty, actual.ty);
                let values_compatible = matches!(pattern.value, ConstGenericValue::GenericParam(_))
                    || self.const_generic_values_match_for_execution(pattern, actual);
                types_compatible && values_compatible
            })
    }

    /// Checks the complete structural path through which generic inference
    /// would collect evidence. Concrete leaves use normal const-call matching;
    /// compatible constructors are still walked so readonly coercions and
    /// nested shapes are checked consistently after substitutions.
    pub(super) fn inference_pattern_accepts_type_shape(
        &mut self,
        pattern: InternedTyId,
        actual: InternedTyId,
    ) -> bool {
        let pattern_kind = self.active_ty_kind(pattern);
        if matches!(pattern_kind, TyKind::GenericParam(_)) {
            return true;
        }
        match (pattern_kind, self.active_ty_kind(actual)) {
            (TyKind::Tuple(patterns), TyKind::Tuple(actuals)) => {
                patterns.len() == actuals.len()
                    && patterns.iter().zip(actuals).all(|(pattern, actual)| {
                        self.inference_pattern_accepts_type_shape(*pattern, actual)
                    })
            }
            (
                TyKind::ClosureState {
                    closure_id: pattern_id,
                    captures: pattern_captures,
                    params: pattern_params,
                    return_type: pattern_return,
                },
                TyKind::ClosureState {
                    closure_id: actual_id,
                    captures: actual_captures,
                    params: actual_params,
                    return_type: actual_return,
                },
            ) => {
                pattern_id == actual_id
                    && pattern_captures.len() == actual_captures.len()
                    && pattern_params.len() == actual_params.len()
                    && pattern_captures
                        .iter()
                        .zip(actual_captures)
                        .all(|(pattern, actual)| {
                            self.inference_pattern_accepts_type_shape(*pattern, actual)
                        })
                    && pattern_params
                        .iter()
                        .zip(actual_params)
                        .all(|(pattern, actual)| {
                            self.inference_pattern_accepts_type_shape(*pattern, actual)
                        })
                    && self.inference_pattern_accepts_type_shape(pattern_return, actual_return)
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
            ) => {
                readonly_pointer_accepts(pattern_readonly, actual_readonly)
                    && self.inference_pattern_accepts_type_shape(pattern_elem, actual_elem)
            }
            (
                TyKind::Pointer {
                    is_readonly: true,
                    elem: pattern_elem,
                },
                TyKind::Array {
                    elem: actual_elem, ..
                },
            ) => {
                let TyKind::SlicePointee { elem: pattern_elem } = self.active_ty_kind(pattern_elem)
                else {
                    return false;
                };
                self.inference_pattern_accepts_type_shape(pattern_elem, actual_elem)
            }
            (
                TyKind::Slice {
                    is_readonly: true,
                    elem: pattern_elem,
                },
                TyKind::Array {
                    elem: actual_elem, ..
                },
            ) => self.inference_pattern_accepts_type_shape(pattern_elem, actual_elem),
            (
                TyKind::Slice {
                    is_readonly: pattern_readonly,
                    elem: pattern_elem,
                },
                TyKind::Pointer {
                    is_readonly: actual_readonly,
                    elem: actual_elem,
                },
            ) => {
                if !readonly_pointer_accepts(pattern_readonly, actual_readonly) {
                    return false;
                }
                match self.active_ty_kind(actual_elem) {
                    TyKind::SlicePointee { elem: actual_elem }
                    | TyKind::Array {
                        elem: actual_elem, ..
                    } => self.inference_pattern_accepts_type_shape(pattern_elem, actual_elem),
                    _ => false,
                }
            }
            (
                TyKind::SlicePointee { elem: pattern_elem },
                TyKind::SlicePointee { elem: actual_elem },
            )
            | (TyKind::Optional { elem: pattern_elem }, TyKind::Optional { elem: actual_elem }) => {
                self.inference_pattern_accepts_type_shape(pattern_elem, actual_elem)
            }
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
                self.inference_array_len_pattern_accepts(&pattern_len, &actual_len)
                    && self.inference_pattern_accepts_type_shape(pattern_elem, actual_elem)
            }
            (
                TyKind::Range {
                    kind: pattern_kind,
                    bound: pattern_bound,
                },
                TyKind::Range {
                    kind: actual_kind,
                    bound: actual_bound,
                },
            ) => {
                pattern_kind == actual_kind
                    && match (pattern_bound, actual_bound) {
                        (Some(pattern), Some(actual)) => {
                            self.inference_pattern_accepts_type_shape(pattern, actual)
                        }
                        (None, None) => true,
                        _ => false,
                    }
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
            ) => {
                pattern_variadic == actual_variadic
                    && pattern_params.len() == actual_params.len()
                    && pattern_params
                        .iter()
                        .zip(actual_params)
                        .all(|(pattern, actual)| {
                            self.inference_pattern_accepts_type_shape(*pattern, actual)
                        })
                    && self.inference_pattern_accepts_type_shape(pattern_return, actual_return)
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
            ) => {
                pattern_readonly == actual_readonly
                    && pattern_params.len() == actual_params.len()
                    && pattern_params
                        .iter()
                        .zip(actual_params)
                        .all(|(pattern, actual)| {
                            self.inference_pattern_accepts_type_shape(*pattern, actual)
                        })
                    && self.inference_pattern_accepts_type_shape(pattern_return, actual_return)
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
            ) => {
                pattern_params.len() == actual_params.len()
                    && pattern_params
                        .iter()
                        .zip(actual_params)
                        .all(|(pattern, actual)| {
                            self.inference_pattern_accepts_type_shape(*pattern, actual)
                        })
                    && self.inference_pattern_accepts_type_shape(pattern_return, actual_return)
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
                self.inference_pattern_accepts_type_shape(pattern_error, actual_error)
                    && self.inference_pattern_accepts_type_shape(pattern_value, actual_value)
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
            ) => {
                pattern_def == actual_def
                    && pattern_args.len() == actual_args.len()
                    && self
                        .const_generic_args_allow_inference(&pattern_const_args, &actual_const_args)
                    && pattern_args
                        .iter()
                        .zip(actual_args)
                        .all(|(pattern, actual)| {
                            self.inference_pattern_accepts_type_shape(*pattern, actual)
                        })
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
            ) => {
                pattern_trait == actual_trait
                    && pattern_args.len() == actual_args.len()
                    && pattern_args
                        .iter()
                        .zip(actual_args)
                        .all(|(pattern, actual)| {
                            self.inference_pattern_accepts_type_shape(*pattern, actual)
                        })
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
            ) => {
                pattern_readonly == actual_readonly
                    && pattern_trait == actual_trait
                    && self.inference_trait_type_parts_accept_shapes(
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
                    )
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
            ) => {
                pattern_trait == actual_trait
                    && self.inference_trait_type_parts_accept_shapes(
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
                    )
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
            ) => {
                pattern_trait == actual_trait
                    && pattern_name == actual_name
                    && pattern_args.len() == actual_args.len()
                    && self
                        .const_generic_args_allow_inference(&pattern_const_args, &actual_const_args)
                    && self.inference_pattern_accepts_type_shape(pattern_self, actual_self)
                    && pattern_args
                        .iter()
                        .zip(actual_args)
                        .all(|(pattern, actual)| {
                            self.inference_pattern_accepts_type_shape(*pattern, actual)
                        })
            }
            _ => self.const_function_types_match(pattern, actual),
        }
    }

    fn inference_array_len_pattern_accepts(
        &mut self,
        pattern: &ArrayLenTy,
        actual: &ArrayLenTy,
    ) -> bool {
        if matches!(pattern, ArrayLenTy::GenericParam(_)) || pattern == actual {
            return true;
        }
        if let (
            ArrayLenTy::Builtin {
                builtin: pattern_builtin,
                ty: pattern_ty,
            },
            ArrayLenTy::Builtin {
                builtin: actual_builtin,
                ty: actual_ty,
            },
        ) = (pattern, actual)
            && pattern_builtin == actual_builtin
        {
            return self.inference_pattern_accepts_type_shape(*pattern_ty, *actual_ty);
        }
        matches!(
            (
                self.array_len_const_value(pattern.clone()),
                self.array_len_const_value(actual.clone()),
            ),
            (Some(pattern), Some(actual)) if pattern == actual
        )
    }

    fn inference_trait_type_parts_accept_shapes(
        &mut self,
        pattern: TraitTypeParts<'_>,
        actual: TraitTypeParts<'_>,
    ) -> bool {
        pattern.args.len() == actual.args.len()
            && pattern.bindings.len() == actual.bindings.len()
            && pattern
                .args
                .iter()
                .zip(actual.args)
                .all(|(pattern, actual)| {
                    self.inference_pattern_accepts_type_shape(*pattern, *actual)
                })
            && self.const_generic_args_allow_inference(pattern.const_args, actual.const_args)
            && self.inference_binding_patterns_accept_shapes(pattern.bindings, actual.bindings)
    }

    fn inference_binding_patterns_accept_shapes(
        &mut self,
        patterns: &[nia_ty::AssociatedTypeBindingTy],
        actuals: &[nia_ty::AssociatedTypeBindingTy],
    ) -> bool {
        if patterns.len() != actuals.len() {
            return false;
        }
        self.inference_binding_patterns_accept_shapes_inner(
            patterns,
            actuals,
            0,
            &mut vec![false; actuals.len()],
        )
    }

    fn inference_binding_patterns_accept_shapes_inner(
        &mut self,
        patterns: &[nia_ty::AssociatedTypeBindingTy],
        actuals: &[nia_ty::AssociatedTypeBindingTy],
        pattern_index: usize,
        used: &mut [bool],
    ) -> bool {
        let Some(pattern) = patterns.get(pattern_index) else {
            return true;
        };
        for (actual_index, actual) in actuals.iter().enumerate() {
            if used[actual_index]
                || pattern.name != actual.name
                || pattern.trait_id != actual.trait_id
                || pattern.trait_args.len() != actual.trait_args.len()
                || !pattern
                    .trait_args
                    .iter()
                    .zip(&actual.trait_args)
                    .all(|(pattern, actual)| {
                        self.inference_pattern_accepts_type_shape(*pattern, *actual)
                    })
                || !self.const_generic_args_allow_inference(
                    &pattern.trait_const_args,
                    &actual.trait_const_args,
                )
                || !self.inference_pattern_accepts_type_shape(pattern.ty, actual.ty)
            {
                continue;
            }
            used[actual_index] = true;
            let matched = self.inference_binding_patterns_accept_shapes_inner(
                patterns,
                actuals,
                pattern_index + 1,
                used,
            );
            used[actual_index] = false;
            if matched {
                return true;
            }
        }
        false
    }

    fn infer_type_generics_from_associated_bindings(
        &mut self,
        span: Span,
        target_module_id: ModuleId,
        pattern: &[nia_ty::AssociatedTypeBindingTy],
        actual: &[nia_ty::AssociatedTypeBindingTy],
        substitutions: &mut SymbolMap<InternedTyId>,
    ) -> Result<(), ConstError> {
        // Associated bindings are an unordered set keyed by defining trait,
        // member name, and trait arguments. Match them as a bijection: a
        // successful inference on one binding must not consume the same actual
        // binding twice, and an early compatible choice may need backtracking.
        if pattern.len() != actual.len() {
            return Ok(());
        }
        let mut used = vec![false; actual.len()];
        let mut first_error = None;
        let matched = self.infer_type_generics_from_associated_bindings_inner(
            span,
            target_module_id,
            pattern,
            actual,
            0,
            &mut used,
            substitutions,
            &mut first_error,
        );
        if !matched && let Some(error) = first_error {
            return Err(error);
        }
        Ok(())
    }

    #[allow(clippy::too_many_arguments)]
    fn infer_type_generics_from_associated_bindings_inner(
        &mut self,
        span: Span,
        target_module_id: ModuleId,
        pattern: &[nia_ty::AssociatedTypeBindingTy],
        actual: &[nia_ty::AssociatedTypeBindingTy],
        pattern_index: usize,
        used: &mut [bool],
        substitutions: &mut SymbolMap<InternedTyId>,
        first_error: &mut Option<ConstError>,
    ) -> bool {
        let Some(pattern_binding) = pattern.get(pattern_index) else {
            return true;
        };
        for (actual_index, actual_binding) in actual.iter().enumerate() {
            if used[actual_index]
                || actual_binding.trait_id != pattern_binding.trait_id
                || actual_binding.name != pattern_binding.name
                || actual_binding.trait_args.len() != pattern_binding.trait_args.len()
                || !pattern_binding
                    .trait_args
                    .iter()
                    .zip(&actual_binding.trait_args)
                    .all(|(pattern, actual)| {
                        self.inference_pattern_accepts_type_shape(*pattern, *actual)
                    })
                || !self.const_generic_args_allow_inference(
                    &pattern_binding.trait_const_args,
                    &actual_binding.trait_const_args,
                )
            {
                continue;
            }
            let mut candidate = substitutions.clone();
            let result = (|| {
                for (pattern_arg, actual_arg) in pattern_binding
                    .trait_args
                    .iter()
                    .zip(&actual_binding.trait_args)
                {
                    self.infer_generics_from_tys(
                        span,
                        target_module_id,
                        *pattern_arg,
                        *actual_arg,
                        &mut candidate,
                    )?;
                }
                self.infer_generics_from_tys(
                    span,
                    target_module_id,
                    pattern_binding.ty,
                    actual_binding.ty,
                    &mut candidate,
                )?;
                Ok::<_, ConstError>(())
            })();
            match result {
                Ok(())
                    if self.associated_binding_types_match_after_inference(
                        pattern_binding,
                        actual_binding,
                        &candidate,
                    ) =>
                {
                    used[actual_index] = true;
                    if self.infer_type_generics_from_associated_bindings_inner(
                        span,
                        target_module_id,
                        pattern,
                        actual,
                        pattern_index + 1,
                        used,
                        &mut candidate,
                        first_error,
                    ) {
                        *substitutions = candidate;
                        used[actual_index] = false;
                        return true;
                    }
                    used[actual_index] = false;
                }
                Ok(()) => {}
                Err(error) => {
                    first_error.get_or_insert(error);
                }
            }
        }
        false
    }

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
        let pattern_ty = self.substitute_inference_generics(
            target_module_id,
            pattern_ty,
            type_substitutions,
            const_substitutions,
        );
        self.infer_const_generics_from_tys(
            span,
            target_module_id,
            pattern_ty,
            actual_ty,
            const_substitutions,
        )
    }

    fn substitute_inference_generics(
        &mut self,
        target_module_id: ModuleId,
        ty: InternedTyId,
        type_substitutions: &SymbolMap<InternedTyId>,
        const_substitutions: &SymbolMap<ConstGenericArg>,
    ) -> InternedTyId {
        if self.ensure_type_context(target_module_id).is_none() {
            return ty;
        }
        let interner = self
            .type_contexts
            .get(&target_module_id)
            .expect("type context must exist for generic inference");
        nia_ty::substitute_ty(
            interner.store,
            &interner.append,
            ty,
            &|name| type_substitutions.get(name).copied(),
            &|name| const_substitutions.get(name).cloned(),
            None,
        )
    }

    fn infer_type_generics_from_compatible_tys(
        &mut self,
        span: Span,
        target_module_id: ModuleId,
        pattern_ty: InternedTyId,
        actual_ty: InternedTyId,
        substitutions: &mut SymbolMap<InternedTyId>,
    ) -> Result<bool, ConstError> {
        let mut candidate = substitutions.clone();
        self.infer_generics_from_tys(
            span,
            target_module_id,
            pattern_ty,
            actual_ty,
            &mut candidate,
        )?;
        let instantiated = self.substitute_inference_generics(
            target_module_id,
            pattern_ty,
            &candidate,
            &SymbolMap::default(),
        );
        let compatible = self.inference_pattern_accepts_type_shape(instantiated, actual_ty);
        if compatible {
            *substitutions = candidate;
        }
        Ok(compatible)
    }

    fn infer_const_generics_from_compatible_tys(
        &mut self,
        span: Span,
        target_module_id: ModuleId,
        pattern_ty: InternedTyId,
        actual_ty: InternedTyId,
        substitutions: &mut SymbolMap<ConstGenericArg>,
    ) -> Result<bool, ConstError> {
        let mut candidate = substitutions.clone();
        self.infer_const_generics_from_tys(
            span,
            target_module_id,
            pattern_ty,
            actual_ty,
            &mut candidate,
        )?;
        let instantiated = self.substitute_inference_generics(
            target_module_id,
            pattern_ty,
            &SymbolMap::default(),
            &candidate,
        );
        let compatible = self.inference_pattern_accepts_type_shape(instantiated, actual_ty);
        if compatible {
            *substitutions = candidate;
        }
        Ok(compatible)
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
                let canonical = self.type_for_module(span, actual_ty, target_module_id)?;
                if let Some(existing) = substitutions.get(&name) {
                    if !self.const_function_types_match(*existing, canonical) {
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
                    && readonly_pointer_accepts(is_readonly, actual_readonly)
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
                    && readonly_pointer_accepts(is_readonly, actual_readonly)
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
                    && readonly_pointer_accepts(is_readonly, actual_readonly)
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
                {
                    let lengths_match = matches!(len, ArrayLenTy::GenericParam(_))
                        || len == actual_len
                        || match (&len, &actual_len) {
                            (
                                ArrayLenTy::Builtin {
                                    builtin: pattern_builtin,
                                    ty: pattern_ty,
                                },
                                ArrayLenTy::Builtin {
                                    builtin: actual_builtin,
                                    ty: actual_ty,
                                },
                            ) if pattern_builtin == actual_builtin => self
                                .infer_type_generics_from_compatible_tys(
                                    span,
                                    target_module_id,
                                    *pattern_ty,
                                    *actual_ty,
                                    substitutions,
                                )?,
                            _ => matches!(
                                (
                                    self.array_len_const_value(len.clone()),
                                    self.array_len_const_value(actual_len.clone()),
                                ),
                                (Some(pattern), Some(actual)) if pattern == actual
                            ),
                        };
                    if lengths_match {
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
                    && self.const_generic_args_allow_inference(&const_args, &actual_const_args)
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
            TyKind::BuiltinTrait { trait_id, args } => {
                if let Some(TyKind::BuiltinTrait {
                    trait_id: actual_trait_id,
                    args: actual_args,
                }) = self.ty_kind(actual_ty)
                    && trait_id == actual_trait_id
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
                is_readonly,
                trait_id,
                trait_args,
                trait_const_args,
                associated_type_bindings,
            } => {
                if let Some(TyKind::TraitObject {
                    is_readonly: actual_readonly,
                    trait_id: actual_trait_id,
                    trait_args: actual_trait_args,
                    trait_const_args: actual_const_args,
                    associated_type_bindings: actual_bindings,
                }) = self.ty_kind(actual_ty)
                    && is_readonly == actual_readonly
                    && trait_id == actual_trait_id
                    && trait_args.len() == actual_trait_args.len()
                    && self
                        .const_generic_args_allow_inference(&trait_const_args, &actual_const_args)
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
                    self.infer_type_generics_from_associated_bindings(
                        span,
                        target_module_id,
                        &associated_type_bindings,
                        &actual_bindings,
                        substitutions,
                    )?;
                }
            }
            TyKind::TraitObjectPointee {
                trait_id,
                trait_args,
                trait_const_args,
                associated_type_bindings,
            } => {
                if let Some(TyKind::TraitObjectPointee {
                    trait_id: actual_trait_id,
                    trait_args: actual_trait_args,
                    trait_const_args: actual_const_args,
                    associated_type_bindings: actual_bindings,
                }) = self.ty_kind(actual_ty)
                    && trait_id == actual_trait_id
                    && trait_args.len() == actual_trait_args.len()
                    && self
                        .const_generic_args_allow_inference(&trait_const_args, &actual_const_args)
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
                    self.infer_type_generics_from_associated_bindings(
                        span,
                        target_module_id,
                        &associated_type_bindings,
                        &actual_bindings,
                        substitutions,
                    )?;
                }
            }
            TyKind::Projection {
                self_ty,
                trait_id,
                trait_args,
                trait_const_args,
                name,
            } => {
                if let Some(TyKind::Projection {
                    self_ty: actual_self_ty,
                    trait_id: actual_trait_id,
                    trait_args: actual_trait_args,
                    trait_const_args: actual_const_args,
                    name: actual_name,
                }) = self.ty_kind(actual_ty)
                    && trait_id == actual_trait_id
                    && name == actual_name
                    && trait_args.len() == actual_trait_args.len()
                    && self
                        .const_generic_args_allow_inference(&trait_const_args, &actual_const_args)
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
            (TyKind::Tuple(pattern_elems), TyKind::Tuple(actual_elems))
                if pattern_elems.len() == actual_elems.len() =>
            {
                for (pattern, actual) in pattern_elems.into_iter().zip(actual_elems) {
                    self.infer_const_generics_from_tys(
                        span,
                        target_module_id,
                        pattern,
                        actual,
                        substitutions,
                    )?;
                }
            }
            (
                TyKind::ClosureState {
                    closure_id: pattern_id,
                    captures: pattern_captures,
                    params: pattern_params,
                    return_type: pattern_return,
                },
                TyKind::ClosureState {
                    closure_id: actual_id,
                    captures: actual_captures,
                    params: actual_params,
                    return_type: actual_return,
                },
            ) if pattern_id == actual_id
                && pattern_captures.len() == actual_captures.len()
                && pattern_params.len() == actual_params.len() =>
            {
                for (pattern, actual) in pattern_captures.into_iter().zip(actual_captures) {
                    self.infer_const_generics_from_tys(
                        span,
                        target_module_id,
                        pattern,
                        actual,
                        substitutions,
                    )?;
                }
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
                TyKind::Array {
                    len: pattern_len,
                    elem: pattern_elem,
                },
                TyKind::Array {
                    len: actual_len,
                    elem: actual_elem,
                },
            ) => {
                let lengths_match = matches!(pattern_len, ArrayLenTy::GenericParam(_))
                    || pattern_len == actual_len
                    || match (&pattern_len, &actual_len) {
                        (
                            ArrayLenTy::Builtin {
                                builtin: pattern_builtin,
                                ty: pattern_ty,
                            },
                            ArrayLenTy::Builtin {
                                builtin: actual_builtin,
                                ty: actual_ty,
                            },
                        ) if pattern_builtin == actual_builtin => self
                            .infer_const_generics_from_compatible_tys(
                                span,
                                target_module_id,
                                *pattern_ty,
                                *actual_ty,
                                substitutions,
                            )?,
                        _ => matches!(
                            (
                                self.array_len_const_value(pattern_len.clone()),
                                self.array_len_const_value(actual_len.clone()),
                            ),
                            (Some(pattern), Some(actual)) if pattern == actual
                        ),
                    };
                if lengths_match {
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
            ) if readonly_pointer_accepts(pattern_readonly, actual_readonly) => {
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
                && self.const_generic_args_allow_inference(
                    &pattern_const_args,
                    &actual_const_args,
                ) =>
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
            ) if pattern_readonly == actual_readonly
                && pattern_trait == actual_trait
                && self.const_generic_args_allow_inference(
                    &pattern_const_args,
                    &actual_const_args,
                ) =>
            {
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
            ) if pattern_trait == actual_trait
                && self.const_generic_args_allow_inference(
                    &pattern_const_args,
                    &actual_const_args,
                ) =>
            {
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
            ) if pattern_trait == actual_trait
                && pattern_name == actual_name
                && pattern_args.len() == actual_args.len()
                && self.const_generic_args_allow_inference(
                    &pattern_const_args,
                    &actual_const_args,
                ) =>
            {
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
        let mut used = vec![false; actual.bindings.len()];
        let mut first_error = None;
        if !self.infer_const_generics_from_associated_bindings(
            span,
            target_module_id,
            pattern.bindings,
            actual.bindings,
            0,
            &mut used,
            substitutions,
            &mut first_error,
        ) && let Some(error) = first_error
        {
            return Err(error);
        }
        Ok(())
    }

    #[allow(clippy::too_many_arguments)]
    fn infer_const_generics_from_associated_bindings(
        &mut self,
        span: Span,
        target_module_id: ModuleId,
        pattern: &[nia_ty::AssociatedTypeBindingTy],
        actual: &[nia_ty::AssociatedTypeBindingTy],
        pattern_index: usize,
        used: &mut [bool],
        substitutions: &mut SymbolMap<ConstGenericArg>,
        first_error: &mut Option<ConstError>,
    ) -> bool {
        let Some(pattern_binding) = pattern.get(pattern_index) else {
            return true;
        };
        for (actual_index, actual_binding) in actual.iter().enumerate() {
            if used[actual_index]
                || actual_binding.trait_id != pattern_binding.trait_id
                || actual_binding.name != pattern_binding.name
                || actual_binding.trait_args.len() != pattern_binding.trait_args.len()
                || !self.const_generic_args_allow_inference(
                    &pattern_binding.trait_const_args,
                    &actual_binding.trait_const_args,
                )
            {
                continue;
            }

            let mut candidate = substitutions.clone();
            let result = (|| {
                self.infer_const_generics_from_tys(
                    span,
                    target_module_id,
                    pattern_binding.ty,
                    actual_binding.ty,
                    &mut candidate,
                )?;
                self.infer_const_generics_from_type_args(
                    span,
                    target_module_id,
                    &pattern_binding.trait_args,
                    &actual_binding.trait_args,
                    &mut candidate,
                )?;
                self.infer_const_generics_from_args(
                    span,
                    target_module_id,
                    &pattern_binding.trait_const_args,
                    &actual_binding.trait_const_args,
                    &mut candidate,
                )
            })();
            match result {
                Ok(())
                    if self.associated_binding_const_types_match_after_inference(
                        target_module_id,
                        pattern_binding,
                        actual_binding,
                        &candidate,
                    ) =>
                {
                    used[actual_index] = true;
                    if self.infer_const_generics_from_associated_bindings(
                        span,
                        target_module_id,
                        pattern,
                        actual,
                        pattern_index + 1,
                        used,
                        &mut candidate,
                        first_error,
                    ) {
                        *substitutions = candidate;
                        used[actual_index] = false;
                        return true;
                    }
                    used[actual_index] = false;
                }
                Ok(()) => {}
                Err(error) => {
                    first_error.get_or_insert(error);
                }
            }
        }
        false
    }

    fn associated_binding_const_types_match_after_inference(
        &mut self,
        target_module_id: ModuleId,
        pattern: &nia_ty::AssociatedTypeBindingTy,
        actual: &nia_ty::AssociatedTypeBindingTy,
        substitutions: &SymbolMap<ConstGenericArg>,
    ) -> bool {
        let type_substitutions = SymbolMap::default();
        pattern
            .trait_args
            .iter()
            .zip(&actual.trait_args)
            .chain(std::iter::once((&pattern.ty, &actual.ty)))
            .all(|(pattern, actual)| {
                let pattern = self.substitute_inference_generics(
                    target_module_id,
                    *pattern,
                    &type_substitutions,
                    substitutions,
                );
                self.inference_pattern_accepts_type_shape(pattern, *actual)
            })
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
            actual.ty = self.type_for_module(span, actual.ty, target_module_id)?;
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
        span: Span,
        ty: InternedTyId,
        target_module_id: ModuleId,
    ) -> Result<InternedTyId, ConstError> {
        validate_type_for_module(
            span,
            self.type_contexts.contains_key(&target_module_id),
            self.input.type_store.get(ty).is_some(),
        )?;
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

fn validate_type_for_module(
    span: Span,
    has_type_context: bool,
    belongs_to_store: bool,
) -> Result<(), ConstError> {
    if !has_type_context {
        return Err(ConstError {
            span,
            message: "const type context is unavailable for target module".to_string(),
        });
    }
    if !belongs_to_store {
        return Err(ConstError {
            span,
            message: "const type belongs to a foreign type store".to_string(),
        });
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn type_for_module_validation_recovers_missing_context_or_store() {
        let span = Span::new(4, 9);
        let missing_context = validate_type_for_module(span, false, true).unwrap_err();
        assert_eq!(missing_context.span, span);
        assert_eq!(
            missing_context.message,
            "const type context is unavailable for target module"
        );

        let foreign_type = validate_type_for_module(span, true, false).unwrap_err();
        assert_eq!(foreign_type.span, span);
        assert_eq!(
            foreign_type.message,
            "const type belongs to a foreign type store"
        );
    }
}
