// SPDX-License-Identifier: GPL-3.0-or-later
//! User-implementation candidate matching, where-clause proof, and specificity.

use super::*;

impl TraitSolver<'_> {
    pub(crate) fn matching_user_impls(&mut self, goal: &TraitGoal) -> Vec<UserImpl> {
        let mut matches = Vec::new();
        if let Some(index) = self.trait_impl_index {
            for impl_index in index.indexes_for_trait(goal.trait_id).iter().copied() {
                if let Some(user_impl) = self.match_user_impl_at(goal, impl_index) {
                    matches.push(user_impl);
                }
            }
        } else {
            for impl_index in 0..self.trait_impls.len() {
                if let Some(user_impl) = self.match_user_impl_at(goal, impl_index) {
                    matches.push(user_impl);
                }
            }
        }
        self.filter_more_specific_user_impls(matches)
    }

    pub(crate) fn match_user_impl_at(
        &mut self,
        goal: &TraitGoal,
        impl_index: usize,
    ) -> Option<UserImpl> {
        let impl_signature = self.trait_impls.get(impl_index)?;
        if impl_signature.builtin.is_some() {
            return None;
        }
        if !(self.impl_is_visible)(impl_signature.module_id, impl_signature.impl_id) {
            return None;
        }
        if impl_signature.trait_id != goal.trait_id {
            return None;
        }
        let target_ty = impl_signature.target_ty;
        let trait_args = &impl_signature.trait_args;
        let trait_const_args = &impl_signature.trait_const_args;
        if trait_args.len() != goal.trait_args.len()
            || trait_const_args.len() != goal.trait_const_args.len()
        {
            return None;
        }
        let mut substitutions = SymbolMap::default();
        let mut const_substitutions = SymbolMap::default();
        let target_matches = self.match_impl_pattern_with_consts(
            target_ty,
            goal.self_ty,
            &mut substitutions,
            &mut const_substitutions,
        );
        let trait_args_match = target_matches
            && trait_args
                .iter()
                .zip(&goal.trait_args)
                .all(|(actual, expected)| {
                    self.match_impl_pattern_with_consts(
                        *actual,
                        *expected,
                        &mut substitutions,
                        &mut const_substitutions,
                    )
                });
        let trait_const_args_match = trait_args_match
            && trait_const_args
                .iter()
                .zip(&goal.trait_const_args)
                .all(|(actual, expected)| {
                    self.match_const_impl_pattern(actual, expected, &mut const_substitutions)
                });
        let where_holds = trait_const_args_match
            && self.impl_where_predicates_hold(
                impl_signature,
                &substitutions,
                &const_substitutions,
            );
        (target_matches && trait_args_match && trait_const_args_match && where_holds).then(|| {
            UserImpl {
                goal: goal.clone(),
                impl_index,
                substitutions,
                const_substitutions,
            }
        })
    }

    pub(crate) fn filter_more_specific_user_impls(
        &mut self,
        matches: Vec<UserImpl>,
    ) -> Vec<UserImpl> {
        // Specificity is a partial order. Remove candidates dominated by another match; multiple
        // maximal candidates remain ambiguous at the selection layer.
        matches
            .iter()
            .filter(|candidate| {
                !matches.iter().any(|other| {
                    other.impl_index != candidate.impl_index
                        && self.user_impl_more_specific(other.impl_index, candidate.impl_index)
                })
            })
            .cloned()
            .collect()
    }

    pub(crate) fn user_impl_more_specific(
        &mut self,
        specific_index: usize,
        general_index: usize,
    ) -> bool {
        self.impl_header_subsumes(general_index, specific_index)
            && !self.impl_header_subsumes(specific_index, general_index)
    }

    fn impl_header_subsumes(&mut self, general_index: usize, specific_index: usize) -> bool {
        let general = &self.trait_impls[general_index];
        let specific = &self.trait_impls[specific_index];
        if general.trait_id != specific.trait_id
            || general.trait_args.len() != specific.trait_args.len()
            || general.trait_const_args.len() != specific.trait_const_args.len()
        {
            return false;
        }
        let general_target = general.target_ty;
        let general_args = general.trait_args.clone();
        let general_const_args = general.trait_const_args.clone();
        let specific_target = specific.target_ty;
        let specific_args = specific.trait_args.clone();
        let specific_const_args = specific.trait_const_args.clone();

        // The complete impl header is one product pattern. Sharing these maps
        // across the target and every trait argument preserves equality
        // constraints such as `T: Relation[T]` and `Box[N]: Rank[N]`.
        let mut substitutions = SymbolMap::default();
        let mut const_substitutions = SymbolMap::default();
        self.match_impl_pattern_with_consts(
            general_target,
            specific_target,
            &mut substitutions,
            &mut const_substitutions,
        ) && general_args
            .iter()
            .zip(&specific_args)
            .all(|(general, specific)| {
                self.match_impl_pattern_with_consts(
                    *general,
                    *specific,
                    &mut substitutions,
                    &mut const_substitutions,
                )
            })
            && general_const_args
                .iter()
                .zip(&specific_const_args)
                .all(|(general, specific)| {
                    self.match_const_impl_pattern(general, specific, &mut const_substitutions)
                })
    }

    pub(crate) fn impl_where_predicates_hold(
        &mut self,
        impl_signature: &ProgramTraitImplSignature,
        substitutions: &SymbolMap<InternedTyId>,
        const_substitutions: &SymbolMap<ConstGenericArg>,
    ) -> bool {
        for predicate in &impl_signature.where_predicates {
            let self_ty =
                self.substitute_ty_with_consts(predicate.ty, substitutions, const_substitutions);
            for bound in &predicate.bounds {
                let trait_ty = self.substitute_ty_with_consts(
                    bound.trait_ty,
                    substitutions,
                    const_substitutions,
                );
                let Some((trait_id, trait_args, trait_const_args)) =
                    self.trait_id_and_args(trait_ty)
                else {
                    return false;
                };
                if !self.proves(TraitGoal {
                    self_ty,
                    trait_id,
                    trait_args: trait_args.clone(),
                    trait_const_args: trait_const_args.clone(),
                }) {
                    return false;
                }
                for binding in &bound.associated_type_bindings {
                    let binding_ty = self.substitute_ty_with_consts(
                        binding.ty,
                        substitutions,
                        const_substitutions,
                    );
                    let Some(actual_ty) = self.resolve_associated_type(
                        self_ty,
                        trait_id,
                        &trait_args,
                        &trait_const_args,
                        &binding.name,
                    ) else {
                        return false;
                    };
                    if !self.types_equivalent(actual_ty, binding_ty) {
                        return false;
                    }
                }
            }
        }
        true
    }

    pub(crate) fn match_impl_pattern_with_consts(
        &mut self,
        pattern: InternedTyId,
        actual: InternedTyId,
        substitutions: &mut SymbolMap<InternedTyId>,
        const_substitutions: &mut SymbolMap<ConstGenericArg>,
    ) -> bool {
        let pattern = self.normalize(pattern);
        let actual = self.normalize(actual);
        if let Some(resolved_pattern) = self.resolve_projection_ty(pattern)
            && resolved_pattern != pattern
        {
            return self.match_impl_pattern_with_consts(
                resolved_pattern,
                actual,
                substitutions,
                const_substitutions,
            );
        }
        if let Some(resolved_actual) = self.resolve_projection_ty(actual)
            && resolved_actual != actual
        {
            return self.match_impl_pattern_with_consts(
                pattern,
                resolved_actual,
                substitutions,
                const_substitutions,
            );
        }
        match self.interner.get(pattern).cloned() {
            Some(TyKind::GenericParam(name)) => {
                // The first occurrence binds the implementation parameter. Later occurrences are
                // equality constraints and must agree with that original substitution.
                if let Some(existing) = substitutions.get(&name).copied() {
                    self.types_equivalent(existing, actual)
                } else {
                    substitutions.insert(name, actual);
                    true
                }
            }
            Some(TyKind::SelfParam) => matches!(self.interner.get(actual), Some(TyKind::SelfParam)),
            Some(TyKind::BuiltinType(pattern_builtin)) => {
                matches!(self.interner.get(actual), Some(TyKind::BuiltinType(actual_builtin)) if pattern_builtin == *actual_builtin)
            }
            Some(TyKind::Opaque) => matches!(self.interner.get(actual), Some(TyKind::Opaque)),
            Some(TyKind::Tuple(pattern_elems)) => match self.interner.get(actual).cloned() {
                Some(TyKind::Tuple(actual_elems)) if pattern_elems.len() == actual_elems.len() => {
                    pattern_elems
                        .iter()
                        .zip(actual_elems)
                        .all(|(pattern_elem, actual_elem)| {
                            self.match_impl_pattern_with_consts(
                                *pattern_elem,
                                actual_elem,
                                substitutions,
                                const_substitutions,
                            )
                        })
                }
                _ => false,
            },
            Some(TyKind::ClosureState {
                closure_id,
                captures,
                params,
                return_type,
            }) => match self.interner.get(actual).cloned() {
                Some(TyKind::ClosureState {
                    closure_id: actual_id,
                    captures: actual_captures,
                    params: actual_params,
                    return_type: actual_return,
                }) if closure_id == actual_id
                    && captures.len() == actual_captures.len()
                    && params.len() == actual_params.len() =>
                {
                    captures
                        .iter()
                        .zip(actual_captures)
                        .all(|(pattern, actual)| {
                            self.match_impl_pattern_with_consts(
                                *pattern,
                                actual,
                                substitutions,
                                const_substitutions,
                            )
                        })
                        && params.iter().zip(actual_params).all(|(pattern, actual)| {
                            self.match_impl_pattern_with_consts(
                                *pattern,
                                actual,
                                substitutions,
                                const_substitutions,
                            )
                        })
                        && self.match_impl_pattern_with_consts(
                            return_type,
                            actual_return,
                            substitutions,
                            const_substitutions,
                        )
                }
                _ => false,
            },
            Some(TyKind::Pointer { is_readonly, elem }) => matches!(
                self.interner.get(actual).cloned(),
                Some(TyKind::Pointer {
                    is_readonly: actual_readonly,
                    elem: actual_elem,
                }) if is_readonly == actual_readonly
                    && self.match_impl_pattern_with_consts(
                        elem,
                        actual_elem,
                        substitutions,
                        const_substitutions
                    )
            ),
            Some(TyKind::VolatilePointer { is_readonly, elem }) => matches!(
                self.interner.get(actual).cloned(),
                Some(TyKind::VolatilePointer {
                    is_readonly: actual_readonly,
                    elem: actual_elem,
                }) if is_readonly == actual_readonly
                    && self.match_impl_pattern_with_consts(elem, actual_elem, substitutions, const_substitutions)
            ),
            Some(TyKind::Slice { is_readonly, elem }) => matches!(
                self.interner.get(actual).cloned(),
                Some(TyKind::Slice {
                    is_readonly: actual_readonly,
                    elem: actual_elem,
                }) if is_readonly == actual_readonly
                    && self.match_impl_pattern_with_consts(elem, actual_elem, substitutions, const_substitutions)
            ),
            Some(TyKind::SlicePointee { elem }) => matches!(
                self.interner.get(actual).cloned(),
                Some(TyKind::SlicePointee { elem: actual_elem })
                    if self.match_impl_pattern_with_consts(elem, actual_elem, substitutions, const_substitutions)
            ),
            Some(TyKind::Array { len, elem }) => match self.interner.get(actual).cloned() {
                Some(TyKind::Array {
                    len: actual_len,
                    elem: actual_elem,
                }) => {
                    // Array length inference and element matching form one
                    // candidate. Do not publish a length binding when the
                    // later element pattern rejects the same candidate.
                    let mut candidate_substitutions = substitutions.clone();
                    let mut candidate_const_substitutions = const_substitutions.clone();
                    if !self.match_array_len_pattern(
                        &len,
                        &actual_len,
                        &mut candidate_const_substitutions,
                    ) {
                        return false;
                    }
                    if !self.match_impl_pattern_with_consts(
                        elem,
                        actual_elem,
                        &mut candidate_substitutions,
                        &mut candidate_const_substitutions,
                    ) {
                        return false;
                    }
                    *substitutions = candidate_substitutions;
                    *const_substitutions = candidate_const_substitutions;
                    true
                }
                _ => false,
            },
            Some(TyKind::Range { kind, bound }) => match self.interner.get(actual).cloned() {
                Some(TyKind::Range {
                    kind: actual_kind,
                    bound: actual_bound,
                }) if kind == actual_kind => match (bound, actual_bound) {
                    (Some(bound), Some(actual_bound)) => self.match_impl_pattern_with_consts(
                        bound,
                        actual_bound,
                        substitutions,
                        const_substitutions,
                    ),
                    (None, None) => true,
                    _ => false,
                },
                _ => false,
            },
            Some(TyKind::FunctionPointer {
                params,
                return_type,
                is_variadic,
            }) => match self.interner.get(actual).cloned() {
                Some(TyKind::FunctionPointer {
                    params: actual_params,
                    return_type: actual_return,
                    is_variadic: actual_variadic,
                }) if is_variadic == actual_variadic && params.len() == actual_params.len() => {
                    params
                        .iter()
                        .zip(actual_params)
                        .all(|(param, actual_param)| {
                            self.match_impl_pattern_with_consts(
                                *param,
                                actual_param,
                                substitutions,
                                const_substitutions,
                            )
                        })
                        && self.match_impl_pattern_with_consts(
                            return_type,
                            actual_return,
                            substitutions,
                            const_substitutions,
                        )
                }
                _ => false,
            },
            Some(TyKind::Callable {
                is_readonly,
                params,
                return_type,
            }) => match self.interner.get(actual).cloned() {
                Some(TyKind::Callable {
                    is_readonly: actual_readonly,
                    params: actual_params,
                    return_type: actual_return,
                }) if is_readonly == actual_readonly && params.len() == actual_params.len() => {
                    params
                        .iter()
                        .zip(actual_params)
                        .all(|(param, actual_param)| {
                            self.match_impl_pattern_with_consts(
                                *param,
                                actual_param,
                                substitutions,
                                const_substitutions,
                            )
                        })
                        && self.match_impl_pattern_with_consts(
                            return_type,
                            actual_return,
                            substitutions,
                            const_substitutions,
                        )
                }
                _ => false,
            },
            Some(TyKind::CallablePointee {
                params,
                return_type,
            }) => match self.interner.get(actual).cloned() {
                Some(TyKind::CallablePointee {
                    params: actual_params,
                    return_type: actual_return,
                }) if params.len() == actual_params.len() => {
                    params
                        .iter()
                        .zip(actual_params)
                        .all(|(param, actual_param)| {
                            self.match_impl_pattern_with_consts(
                                *param,
                                actual_param,
                                substitutions,
                                const_substitutions,
                            )
                        })
                        && self.match_impl_pattern_with_consts(
                            return_type,
                            actual_return,
                            substitutions,
                            const_substitutions,
                        )
                }
                _ => false,
            },
            Some(TyKind::Optional { elem }) => match self.interner.get(actual).cloned() {
                Some(TyKind::Optional { elem: actual_elem }) => self
                    .match_impl_pattern_with_consts(
                        elem,
                        actual_elem,
                        substitutions,
                        const_substitutions,
                    ),
                _ => false,
            },
            Some(TyKind::ErrorUnion { error, value }) => match self.interner.get(actual).cloned() {
                Some(TyKind::ErrorUnion {
                    error: actual_error,
                    value: actual_value,
                }) => {
                    self.match_impl_pattern_with_consts(
                        error,
                        actual_error,
                        substitutions,
                        const_substitutions,
                    ) && self.match_impl_pattern_with_consts(
                        value,
                        actual_value,
                        substitutions,
                        const_substitutions,
                    )
                }
                _ => false,
            },
            Some(TyKind::Nominal {
                def_id,
                args,
                const_args,
            }) => match self.interner.get(actual).cloned() {
                Some(TyKind::Nominal {
                    def_id: actual_def,
                    args: actual_args,
                    const_args: actual_const_args,
                }) if def_id == actual_def
                    && const_args.len() == actual_const_args.len()
                    && args.len() == actual_args.len() =>
                {
                    args.iter().zip(actual_args).all(|(arg, actual_arg)| {
                        self.match_impl_pattern_with_consts(
                            *arg,
                            actual_arg,
                            substitutions,
                            const_substitutions,
                        )
                    }) && const_args
                        .iter()
                        .zip(&actual_const_args)
                        .all(|(arg, actual_arg)| {
                            self.match_const_impl_pattern(arg, actual_arg, const_substitutions)
                        })
                }
                _ => false,
            },
            Some(TyKind::BuiltinTrait { trait_id, args }) => {
                match self.interner.get(actual).cloned() {
                    Some(TyKind::BuiltinTrait {
                        trait_id: actual_trait,
                        args: actual_args,
                    }) if trait_id == actual_trait && args.len() == actual_args.len() => {
                        args.iter().zip(actual_args).all(|(arg, actual_arg)| {
                            self.match_impl_pattern_with_consts(
                                *arg,
                                actual_arg,
                                substitutions,
                                const_substitutions,
                            )
                        })
                    }
                    _ => false,
                }
            }
            Some(TyKind::TraitObject {
                is_readonly,
                trait_id,
                trait_args,
                trait_const_args,
                associated_type_bindings,
            }) => match self.interner.get(actual).cloned() {
                Some(TyKind::TraitObject {
                    is_readonly: actual_readonly,
                    trait_id: actual_trait,
                    trait_args: actual_args,
                    trait_const_args: actual_const_args,
                    associated_type_bindings: actual_bindings,
                }) if is_readonly == actual_readonly
                    && trait_id == actual_trait
                    && trait_args.len() == actual_args.len()
                    && trait_const_args.len() == actual_const_args.len()
                    && associated_type_bindings.len() == actual_bindings.len() =>
                {
                    trait_args.iter().zip(actual_args).all(|(arg, actual_arg)| {
                        self.match_impl_pattern_with_consts(
                            *arg,
                            actual_arg,
                            substitutions,
                            const_substitutions,
                        )
                    }) && trait_const_args.iter().zip(&actual_const_args).all(
                        |(arg, actual_arg)| {
                            self.match_const_impl_pattern(arg, actual_arg, const_substitutions)
                        },
                    ) && self.match_associated_type_binding_patterns(
                        &associated_type_bindings,
                        &actual_bindings,
                        substitutions,
                        const_substitutions,
                    )
                }
                _ => false,
            },
            Some(TyKind::TraitObjectPointee {
                trait_id,
                trait_args,
                trait_const_args,
                associated_type_bindings,
            }) => match self.interner.get(actual).cloned() {
                Some(TyKind::TraitObjectPointee {
                    trait_id: actual_trait,
                    trait_args: actual_args,
                    trait_const_args: actual_const_args,
                    associated_type_bindings: actual_bindings,
                }) if trait_id == actual_trait
                    && trait_args.len() == actual_args.len()
                    && trait_const_args.len() == actual_const_args.len()
                    && associated_type_bindings.len() == actual_bindings.len() =>
                {
                    trait_args.iter().zip(actual_args).all(|(arg, actual_arg)| {
                        self.match_impl_pattern_with_consts(
                            *arg,
                            actual_arg,
                            substitutions,
                            const_substitutions,
                        )
                    }) && trait_const_args.iter().zip(&actual_const_args).all(
                        |(arg, actual_arg)| {
                            self.match_const_impl_pattern(arg, actual_arg, const_substitutions)
                        },
                    ) && self.match_associated_type_binding_patterns(
                        &associated_type_bindings,
                        &actual_bindings,
                        substitutions,
                        const_substitutions,
                    )
                }
                _ => false,
            },
            Some(TyKind::Projection {
                self_ty,
                trait_id,
                trait_args,
                trait_const_args,
                name,
            }) => match self.interner.get(actual).cloned() {
                Some(TyKind::Projection {
                    self_ty: actual_self,
                    trait_id: actual_trait,
                    trait_args: actual_args,
                    trait_const_args: actual_const_args,
                    name: actual_name,
                }) if trait_id == actual_trait
                    && name == actual_name
                    && trait_args.len() == actual_args.len()
                    && trait_const_args.len() == actual_const_args.len() =>
                {
                    self.match_impl_pattern_with_consts(
                        self_ty,
                        actual_self,
                        substitutions,
                        const_substitutions,
                    ) && trait_args.iter().zip(actual_args).all(|(arg, actual_arg)| {
                        self.match_impl_pattern_with_consts(
                            *arg,
                            actual_arg,
                            substitutions,
                            const_substitutions,
                        )
                    }) && trait_const_args.iter().zip(&actual_const_args).all(
                        |(arg, actual_arg)| {
                            self.match_const_impl_pattern(arg, actual_arg, const_substitutions)
                        },
                    )
                }
                _ => false,
            },
            Some(
                TyKind::Error | TyKind::ConstOnly | TyKind::Primitive(_) | TyKind::Vector { .. },
            )
            | None => self.types_equivalent(pattern, actual),
        }
    }

    pub(crate) fn match_const_impl_pattern(
        &mut self,
        pattern: &ConstGenericArg,
        actual: &ConstGenericArg,
        substitutions: &mut SymbolMap<ConstGenericArg>,
    ) -> bool {
        if !self.types_equivalent(pattern.ty, actual.ty) {
            return false;
        }
        match &pattern.value {
            ConstGenericValue::GenericParam(name) => {
                if let Some(existing) = substitutions.get(name).cloned() {
                    self.const_generic_args_equivalent(&existing, actual)
                } else {
                    substitutions.insert(*name, actual.clone());
                    true
                }
            }
            _ => self.const_generic_args_equivalent(pattern, actual),
        }
    }

    pub(crate) fn match_array_len_pattern(
        &mut self,
        pattern: &ArrayLenTy,
        actual: &ArrayLenTy,
        substitutions: &mut SymbolMap<ConstGenericArg>,
    ) -> bool {
        if pattern == actual {
            return true;
        }
        match (pattern, actual) {
            (ArrayLenTy::GenericParam(name), actual) => {
                let Some(actual_arg) = self.const_arg_from_array_len(actual) else {
                    return false;
                };
                if let Some(existing) = substitutions.get(name).cloned() {
                    self.const_generic_args_equivalent(&existing, &actual_arg)
                } else {
                    substitutions.insert(*name, actual_arg);
                    true
                }
            }
            _ => self.same_array_len_for_equiv(pattern, actual),
        }
    }

    pub(crate) fn const_arg_from_array_len(&self, len: &ArrayLenTy) -> Option<ConstGenericArg> {
        let ty = self.interner.primitive(PrimitiveTy::Usize);
        let value = match len {
            ArrayLenTy::ConstValue(value) => {
                ConstGenericValue::Int(nia_ty::IntConst::unsigned((*value).into()))
            }
            ArrayLenTy::GenericParam(name) => ConstGenericValue::GenericParam(*name),
            ArrayLenTy::ConstExpr(id) => ConstGenericValue::ConstExpr(*id),
            ArrayLenTy::Builtin {
                builtin,
                ty: layout_ty,
            } => {
                let layout_ty = self.normalize(*layout_ty);
                let layouts = self.layouts?;
                let layout = layouts.types.get(&layout_ty).cloned().or_else(|| {
                    layouts.types.iter().find_map(|(candidate, layout)| {
                        self.types_equivalent_in_layout_interner(
                            layout_ty,
                            *candidate,
                            layouts,
                            &mut HashSet::new(),
                        )
                        .then(|| layout.clone())
                    })
                })?;
                ConstGenericValue::Int(nia_ty::IntConst::unsigned(
                    layout.builtin_value(*builtin).into(),
                ))
            }
            ArrayLenTy::Infer => return None,
        };
        Some(ConstGenericArg { ty, value })
    }

    fn try_match_associated_type_binding(
        &mut self,
        pattern: &nia_ty::AssociatedTypeBindingTy,
        actual: &nia_ty::AssociatedTypeBindingTy,
        substitutions: &mut SymbolMap<InternedTyId>,
        const_substitutions: &mut SymbolMap<ConstGenericArg>,
    ) -> bool {
        if pattern.name != actual.name
            || pattern.trait_id != actual.trait_id
            || pattern.trait_args.len() != actual.trait_args.len()
            || pattern.trait_const_args.len() != actual.trait_const_args.len()
        {
            return false;
        }
        let mut candidate_substitutions = substitutions.clone();
        let mut candidate_const_substitutions = const_substitutions.clone();
        if !pattern
            .trait_args
            .iter()
            .zip(&actual.trait_args)
            .all(|(pattern, actual)| {
                self.match_impl_pattern_with_consts(
                    *pattern,
                    *actual,
                    &mut candidate_substitutions,
                    &mut candidate_const_substitutions,
                )
            })
            || !pattern
                .trait_const_args
                .iter()
                .zip(&actual.trait_const_args)
                .all(|(pattern, actual)| {
                    self.match_const_impl_pattern(
                        pattern,
                        actual,
                        &mut candidate_const_substitutions,
                    )
                })
            || !self.match_impl_pattern_with_consts(
                pattern.ty,
                actual.ty,
                &mut candidate_substitutions,
                &mut candidate_const_substitutions,
            )
        {
            return false;
        }
        *substitutions = candidate_substitutions;
        *const_substitutions = candidate_const_substitutions;
        true
    }

    fn match_associated_type_binding_patterns(
        &mut self,
        patterns: &[nia_ty::AssociatedTypeBindingTy],
        actuals: &[nia_ty::AssociatedTypeBindingTy],
        substitutions: &mut SymbolMap<InternedTyId>,
        const_substitutions: &mut SymbolMap<ConstGenericArg>,
    ) -> bool {
        if patterns.len() != actuals.len() {
            return false;
        }
        self.match_associated_type_binding_patterns_inner(
            patterns,
            actuals,
            0,
            &mut vec![false; actuals.len()],
            substitutions,
            const_substitutions,
        )
    }

    fn match_associated_type_binding_patterns_inner(
        &mut self,
        patterns: &[nia_ty::AssociatedTypeBindingTy],
        actuals: &[nia_ty::AssociatedTypeBindingTy],
        pattern_index: usize,
        used: &mut [bool],
        substitutions: &mut SymbolMap<InternedTyId>,
        const_substitutions: &mut SymbolMap<ConstGenericArg>,
    ) -> bool {
        let Some(pattern) = patterns.get(pattern_index) else {
            return true;
        };
        for (actual_index, actual) in actuals.iter().enumerate() {
            if used[actual_index] {
                continue;
            }
            let mut candidate_substitutions = substitutions.clone();
            let mut candidate_const_substitutions = const_substitutions.clone();
            if !self.try_match_associated_type_binding(
                pattern,
                actual,
                &mut candidate_substitutions,
                &mut candidate_const_substitutions,
            ) {
                continue;
            }
            used[actual_index] = true;
            let matched = self.match_associated_type_binding_patterns_inner(
                patterns,
                actuals,
                pattern_index + 1,
                used,
                &mut candidate_substitutions,
                &mut candidate_const_substitutions,
            );
            used[actual_index] = false;
            if matched {
                *substitutions = candidate_substitutions;
                *const_substitutions = candidate_const_substitutions;
                return true;
            }
        }
        false
    }
}
