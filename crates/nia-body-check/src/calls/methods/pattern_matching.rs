// SPDX-License-Identifier: GPL-3.0-or-later
//! Structural type-pattern matching and generic substitution.
//!
//! The recursive matcher may record partial type and const substitutions
//! before a later component fails. Candidate probing must therefore enter
//! through `try_match_*`: those wrappers clone both maps and commit them
//! together only after the complete pattern succeeds.

use super::*;

impl<'a> BodyChecker<'a> {
    pub(super) fn try_match_type_pattern_with_consts(
        &mut self,
        pattern: InternedTyId,
        actual: InternedTyId,
        substitutions: &mut SymbolMap<InternedTyId>,
        const_substitutions: &mut SymbolMap<nia_ty::ConstGenericArg>,
    ) -> bool {
        let mut candidate_substitutions = substitutions.clone();
        let mut candidate_const_substitutions = const_substitutions.clone();
        if !self.match_type_pattern_with_consts(
            pattern,
            actual,
            &mut candidate_substitutions,
            &mut candidate_const_substitutions,
        ) {
            return false;
        }
        *substitutions = candidate_substitutions;
        *const_substitutions = candidate_const_substitutions;
        true
    }

    pub(crate) fn match_type_pattern(
        &mut self,
        pattern: InternedTyId,
        actual: InternedTyId,
        substitutions: &mut SymbolMap<InternedTyId>,
    ) -> bool {
        let mut const_substitutions = SymbolMap::default();
        self.match_type_pattern_with_consts(
            pattern,
            actual,
            substitutions,
            &mut const_substitutions,
        )
    }

    pub(crate) fn match_type_pattern_with_consts(
        &mut self,
        pattern: InternedTyId,
        actual: InternedTyId,
        substitutions: &mut SymbolMap<InternedTyId>,
        const_substitutions: &mut SymbolMap<nia_ty::ConstGenericArg>,
    ) -> bool {
        let pattern = self.normalization.normalize(pattern);
        let actual = self.normalization.normalize(actual);
        let pattern_kind = self.interner.get(pattern).cloned();
        let actual_kind = self.interner.get(actual).cloned();
        match pattern_kind {
            Some(TyKind::GenericParam(name)) => {
                if let Some(existing) = substitutions.get(&name).copied() {
                    self.types_match(existing, actual)
                } else {
                    substitutions.insert(name, actual);
                    true
                }
            }
            Some(TyKind::SelfParam) => true,
            Some(TyKind::BuiltinType(pattern_builtin)) => {
                matches!(actual_kind, Some(TyKind::BuiltinType(actual_builtin)) if pattern_builtin == actual_builtin)
            }
            Some(TyKind::Opaque) => matches!(actual_kind, Some(TyKind::Opaque)),
            Some(TyKind::Tuple(pattern_elems)) => match actual_kind {
                Some(TyKind::Tuple(actual_elems)) if pattern_elems.len() == actual_elems.len() => {
                    pattern_elems
                        .iter()
                        .zip(&actual_elems)
                        .all(|(pattern, actual)| {
                            self.match_type_pattern_with_consts(
                                *pattern,
                                *actual,
                                substitutions,
                                const_substitutions,
                            )
                        })
                }
                _ => false,
            },
            Some(TyKind::Pointer {
                is_readonly: pattern_const,
                elem: pattern_elem,
            }) => match actual_kind {
                Some(TyKind::Pointer { is_readonly, elem }) if is_readonly == pattern_const => self
                    .match_type_pattern_with_consts(
                        pattern_elem,
                        elem,
                        substitutions,
                        const_substitutions,
                    ),
                _ => false,
            },
            Some(TyKind::VolatilePointer {
                is_readonly: pattern_const,
                elem: pattern_elem,
            }) => match actual_kind {
                Some(TyKind::VolatilePointer { is_readonly, elem })
                    if is_readonly == pattern_const =>
                {
                    self.match_type_pattern_with_consts(
                        pattern_elem,
                        elem,
                        substitutions,
                        const_substitutions,
                    )
                }
                _ => false,
            },
            Some(TyKind::Slice {
                is_readonly: pattern_const,
                elem: pattern_elem,
            }) => match actual_kind {
                Some(TyKind::Slice { is_readonly, elem }) if is_readonly == pattern_const => self
                    .match_type_pattern_with_consts(
                        pattern_elem,
                        elem,
                        substitutions,
                        const_substitutions,
                    ),
                _ => false,
            },
            Some(TyKind::SlicePointee { elem: pattern_elem }) => match actual_kind {
                Some(TyKind::SlicePointee { elem }) => self.match_type_pattern_with_consts(
                    pattern_elem,
                    elem,
                    substitutions,
                    const_substitutions,
                ),
                _ => false,
            },
            Some(TyKind::Array {
                len: pattern_len,
                elem: pattern_elem,
            }) => match actual_kind {
                Some(TyKind::Array {
                    len: actual_len,
                    elem,
                }) => {
                    let mut candidate_substitutions = substitutions.clone();
                    let mut candidate_const_substitutions = const_substitutions.clone();
                    let length_matches = match (&pattern_len, &actual_len) {
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
                            .match_type_pattern_with_consts(
                                *pattern_ty,
                                *actual_ty,
                                &mut candidate_substitutions,
                                &mut candidate_const_substitutions,
                            ),
                        _ => self.match_array_len_pattern(
                            &pattern_len,
                            &actual_len,
                            &mut candidate_const_substitutions,
                        ),
                    };
                    if length_matches
                        && self.match_type_pattern_with_consts(
                            pattern_elem,
                            elem,
                            &mut candidate_substitutions,
                            &mut candidate_const_substitutions,
                        )
                    {
                        *substitutions = candidate_substitutions;
                        *const_substitutions = candidate_const_substitutions;
                        true
                    } else {
                        false
                    }
                }
                _ => false,
            },
            Some(TyKind::Range {
                kind: pattern_kind,
                bound: pattern_bound,
            }) => match actual_kind {
                Some(TyKind::Range { kind, bound }) if pattern_kind == kind => {
                    match (pattern_bound, bound) {
                        (Some(pattern_bound), Some(bound)) => self.match_type_pattern_with_consts(
                            pattern_bound,
                            bound,
                            substitutions,
                            const_substitutions,
                        ),
                        (None, None) => true,
                        _ => false,
                    }
                }
                _ => false,
            },
            Some(TyKind::FunctionPointer {
                params: pattern_params,
                return_type: pattern_return,
                is_variadic: pattern_variadic,
            }) => match actual_kind {
                Some(TyKind::FunctionPointer {
                    params,
                    return_type,
                    is_variadic,
                }) if pattern_variadic == is_variadic && pattern_params.len() == params.len() => {
                    pattern_params.iter().zip(params).all(|(pattern, actual)| {
                        self.match_type_pattern_with_consts(
                            *pattern,
                            actual,
                            substitutions,
                            const_substitutions,
                        )
                    }) && self.match_type_pattern_with_consts(
                        pattern_return,
                        return_type,
                        substitutions,
                        const_substitutions,
                    )
                }
                _ => false,
            },
            Some(TyKind::Callable {
                is_readonly: pattern_readonly,
                params: pattern_params,
                return_type: pattern_return,
            }) => match actual_kind {
                Some(TyKind::Callable {
                    is_readonly,
                    params,
                    return_type,
                }) if pattern_readonly == is_readonly && pattern_params.len() == params.len() => {
                    pattern_params.iter().zip(params).all(|(pattern, actual)| {
                        self.match_type_pattern_with_consts(
                            *pattern,
                            actual,
                            substitutions,
                            const_substitutions,
                        )
                    }) && self.match_type_pattern_with_consts(
                        pattern_return,
                        return_type,
                        substitutions,
                        const_substitutions,
                    )
                }
                _ => false,
            },
            Some(TyKind::CallablePointee {
                params: pattern_params,
                return_type: pattern_return,
            }) => match actual_kind {
                Some(TyKind::CallablePointee {
                    params,
                    return_type,
                }) if pattern_params.len() == params.len() => {
                    pattern_params.iter().zip(params).all(|(pattern, actual)| {
                        self.match_type_pattern_with_consts(
                            *pattern,
                            actual,
                            substitutions,
                            const_substitutions,
                        )
                    }) && self.match_type_pattern_with_consts(
                        pattern_return,
                        return_type,
                        substitutions,
                        const_substitutions,
                    )
                }
                _ => false,
            },
            Some(TyKind::Optional { elem: pattern_elem }) => match actual_kind {
                Some(TyKind::Optional { elem }) => self.match_type_pattern_with_consts(
                    pattern_elem,
                    elem,
                    substitutions,
                    const_substitutions,
                ),
                _ => false,
            },
            Some(TyKind::ErrorUnion {
                error: pattern_error,
                value: pattern_value,
            }) => match actual_kind {
                Some(TyKind::ErrorUnion { error, value }) => {
                    self.match_type_pattern_with_consts(
                        pattern_error,
                        error,
                        substitutions,
                        const_substitutions,
                    ) && self.match_type_pattern_with_consts(
                        pattern_value,
                        value,
                        substitutions,
                        const_substitutions,
                    )
                }
                _ => false,
            },
            Some(TyKind::Nominal {
                def_id: pattern_def,
                args: pattern_args,
                const_args: pattern_const_args,
            }) => match actual_kind {
                Some(TyKind::Nominal {
                    def_id,
                    args,
                    const_args,
                }) if pattern_def == def_id
                    && pattern_args.len() == args.len()
                    && pattern_const_args.len() == const_args.len() =>
                {
                    pattern_args.iter().zip(args).all(|(pattern, actual)| {
                        self.match_type_pattern_with_consts(
                            *pattern,
                            actual,
                            substitutions,
                            const_substitutions,
                        )
                    }) && pattern_const_args
                        .iter()
                        .zip(const_args)
                        .all(|(pattern, actual)| {
                            self.match_const_generic_arg_pattern(
                                pattern,
                                &actual,
                                substitutions,
                                const_substitutions,
                            )
                        })
                }
                _ => false,
            },
            Some(TyKind::BuiltinTrait {
                trait_id: pattern_trait,
                args: pattern_args,
            }) => match actual_kind {
                Some(TyKind::BuiltinTrait { trait_id, args })
                    if pattern_trait == trait_id && pattern_args.len() == args.len() =>
                {
                    pattern_args.iter().zip(args).all(|(pattern, actual)| {
                        self.match_type_pattern_with_consts(
                            *pattern,
                            actual,
                            substitutions,
                            const_substitutions,
                        )
                    })
                }
                _ => false,
            },
            Some(TyKind::TraitObject {
                is_readonly: pattern_const,
                trait_id: pattern_trait,
                trait_args: pattern_args,
                trait_const_args: pattern_const_args,
                associated_type_bindings: pattern_bindings,
            }) => match actual_kind {
                Some(TyKind::TraitObject {
                    is_readonly,
                    trait_id,
                    trait_args,
                    trait_const_args,
                    associated_type_bindings,
                    ..
                }) if is_readonly == pattern_const
                    && trait_id == pattern_trait
                    && pattern_args.len() == trait_args.len()
                    && pattern_const_args.len() == trait_const_args.len()
                    && pattern_bindings.len() == associated_type_bindings.len() =>
                {
                    pattern_args
                        .iter()
                        .zip(trait_args)
                        .all(|(pattern, actual)| {
                            self.match_type_pattern_with_consts(
                                *pattern,
                                actual,
                                substitutions,
                                const_substitutions,
                            )
                        })
                        && pattern_const_args.iter().zip(trait_const_args).all(
                            |(pattern, actual)| {
                                self.match_const_generic_arg_pattern(
                                    pattern,
                                    &actual,
                                    substitutions,
                                    const_substitutions,
                                )
                            },
                        )
                        && self.match_associated_type_binding_patterns(
                            &pattern_bindings,
                            &associated_type_bindings,
                            substitutions,
                            const_substitutions,
                        )
                }
                _ => false,
            },
            Some(TyKind::TraitObjectPointee {
                trait_id: pattern_trait,
                trait_args: pattern_args,
                trait_const_args: pattern_const_args,
                associated_type_bindings: pattern_bindings,
            }) => match actual_kind {
                Some(TyKind::TraitObjectPointee {
                    trait_id,
                    trait_args,
                    trait_const_args,
                    associated_type_bindings,
                    ..
                }) if trait_id == pattern_trait
                    && pattern_args.len() == trait_args.len()
                    && pattern_const_args.len() == trait_const_args.len()
                    && pattern_bindings.len() == associated_type_bindings.len() =>
                {
                    pattern_args
                        .iter()
                        .zip(trait_args)
                        .all(|(pattern, actual)| {
                            self.match_type_pattern_with_consts(
                                *pattern,
                                actual,
                                substitutions,
                                const_substitutions,
                            )
                        })
                        && pattern_const_args.iter().zip(trait_const_args).all(
                            |(pattern, actual)| {
                                self.match_const_generic_arg_pattern(
                                    pattern,
                                    &actual,
                                    substitutions,
                                    const_substitutions,
                                )
                            },
                        )
                        && self.match_associated_type_binding_patterns(
                            &pattern_bindings,
                            &associated_type_bindings,
                            substitutions,
                            const_substitutions,
                        )
                }
                _ => false,
            },
            Some(TyKind::Projection {
                self_ty: pattern_self,
                trait_id: pattern_trait,
                trait_args: pattern_args,
                trait_const_args: pattern_const_args,
                name: pattern_name,
                ..
            }) => match actual_kind {
                Some(TyKind::Projection {
                    self_ty,
                    trait_id,
                    trait_args,
                    trait_const_args,
                    name,
                    ..
                }) if pattern_trait == trait_id
                    && pattern_name == name
                    && pattern_args.len() == trait_args.len()
                    && pattern_const_args.len() == trait_const_args.len() =>
                {
                    self.match_type_pattern_with_consts(
                        pattern_self,
                        self_ty,
                        substitutions,
                        const_substitutions,
                    ) && pattern_args
                        .iter()
                        .zip(trait_args)
                        .all(|(pattern, actual)| {
                            self.match_type_pattern_with_consts(
                                *pattern,
                                actual,
                                substitutions,
                                const_substitutions,
                            )
                        })
                        && pattern_const_args.iter().zip(trait_const_args).all(
                            |(pattern, actual)| {
                                self.match_const_generic_arg_pattern(
                                    pattern,
                                    &actual,
                                    substitutions,
                                    const_substitutions,
                                )
                            },
                        )
                }
                _ => false,
            },
            Some(TyKind::Primitive(_) | TyKind::Vector { .. })
            | Some(TyKind::ConstOnly | TyKind::Error | TyKind::ClosureState { .. })
            | None => self.types_match(pattern, actual),
        }
    }

    pub(super) fn array_lens_match(&self, expected: &ArrayLenTy, actual: &ArrayLenTy) -> bool {
        if expected == actual {
            return true;
        }
        // Method matching only needs a yes/no answer here. Invalid symbolic
        // lengths are diagnosed where the array type is constructed or checked.
        let expected = self.array_len_value(Span::default(), expected).ok();
        let actual = self.array_len_value(Span::default(), actual).ok();
        expected.is_some() && expected == actual
    }

    fn match_array_len_pattern(
        &mut self,
        pattern: &ArrayLenTy,
        actual: &ArrayLenTy,
        const_substitutions: &mut SymbolMap<nia_ty::ConstGenericArg>,
    ) -> bool {
        if pattern == actual || self.array_lens_match(pattern, actual) {
            return true;
        }
        let ArrayLenTy::GenericParam(name) = pattern else {
            return false;
        };
        let Some(value) = self.method_const_generic_value_from_array_len(actual) else {
            return false;
        };
        let arg = nia_ty::ConstGenericArg {
            ty: self.interner.primitive(PrimitiveTy::Usize),
            value,
        };
        self.record_const_pattern_substitution(name, arg, const_substitutions)
    }

    pub(crate) fn match_const_generic_arg_pattern(
        &mut self,
        pattern: &nia_ty::ConstGenericArg,
        actual: &nia_ty::ConstGenericArg,
        substitutions: &SymbolMap<InternedTyId>,
        const_substitutions: &mut SymbolMap<nia_ty::ConstGenericArg>,
    ) -> bool {
        let pattern_ty =
            self.substitute_generics_and_consts(pattern.ty, substitutions, const_substitutions);
        let actual_ty =
            self.substitute_generics_and_consts(actual.ty, substitutions, const_substitutions);
        if !self.types_equivalent_without_projection_resolution(pattern_ty, actual_ty) {
            return false;
        }
        if let nia_ty::ConstGenericValue::GenericParam(name) = &pattern.value {
            if let Some(existing) = const_substitutions.get(name) {
                return self.const_pattern_args_match_with_substitutions(
                    existing,
                    actual,
                    substitutions,
                    const_substitutions,
                );
            }
            const_substitutions.insert(
                *name,
                nia_ty::ConstGenericArg {
                    ty: actual_ty,
                    value: actual.value.clone(),
                },
            );
            return true;
        }
        self.const_pattern_values_match(&pattern.value, &actual.value)
    }

    fn record_const_pattern_substitution(
        &mut self,
        name: &SymbolId,
        arg: nia_ty::ConstGenericArg,
        const_substitutions: &mut SymbolMap<nia_ty::ConstGenericArg>,
    ) -> bool {
        if let Some(existing) = const_substitutions.get(name) {
            self.const_pattern_args_match(existing, &arg)
        } else {
            const_substitutions.insert(*name, arg);
            true
        }
    }

    fn const_pattern_args_match(
        &mut self,
        left: &nia_ty::ConstGenericArg,
        right: &nia_ty::ConstGenericArg,
    ) -> bool {
        self.types_equivalent_without_projection_resolution(left.ty, right.ty)
            && self.const_pattern_values_match(&left.value, &right.value)
    }

    fn const_pattern_args_match_with_substitutions(
        &mut self,
        left: &nia_ty::ConstGenericArg,
        right: &nia_ty::ConstGenericArg,
        substitutions: &SymbolMap<InternedTyId>,
        const_substitutions: &SymbolMap<nia_ty::ConstGenericArg>,
    ) -> bool {
        let left_ty =
            self.substitute_generics_and_consts(left.ty, substitutions, const_substitutions);
        let right_ty =
            self.substitute_generics_and_consts(right.ty, substitutions, const_substitutions);
        self.types_equivalent_without_projection_resolution(left_ty, right_ty)
            && self.const_pattern_values_match(&left.value, &right.value)
    }

    fn const_pattern_values_match(
        &self,
        left: &nia_ty::ConstGenericValue,
        right: &nia_ty::ConstGenericValue,
    ) -> bool {
        match (left, right) {
            (nia_ty::ConstGenericValue::Int(left), nia_ty::ConstGenericValue::Int(right)) => {
                left.bits() == right.bits()
            }
            (left, right) => left == right,
        }
    }

    pub(super) fn method_const_generic_value_from_array_len(
        &self,
        len: &ArrayLenTy,
    ) -> Option<nia_ty::ConstGenericValue> {
        match len {
            ArrayLenTy::ConstValue(value) => Some(nia_ty::ConstGenericValue::Int(
                nia_ty::IntConst::unsigned((*value).into()),
            )),
            ArrayLenTy::ConstExpr(id) => self.array_len_const_expr_value(*id).map(|value| {
                nia_ty::ConstGenericValue::Int(nia_ty::IntConst::unsigned(value.into()))
            }),
            ArrayLenTy::Builtin { .. } => {
                self.array_len_value(Span::default(), len)
                    .ok()
                    .map(|value| {
                        nia_ty::ConstGenericValue::Int(nia_ty::IntConst::unsigned(value.into()))
                    })
            }
            ArrayLenTy::GenericParam(name) => Some(nia_ty::ConstGenericValue::GenericParam(*name)),
            ArrayLenTy::Infer => None,
        }
    }

    pub(crate) fn try_match_associated_type_binding(
        &mut self,
        pattern: &nia_ty::AssociatedTypeBindingTy,
        actual: &nia_ty::AssociatedTypeBindingTy,
        substitutions: &mut SymbolMap<InternedTyId>,
        const_substitutions: &mut SymbolMap<nia_ty::ConstGenericArg>,
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
                self.match_type_pattern_with_consts(
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
                    self.match_const_generic_arg_pattern(
                        pattern,
                        actual,
                        &candidate_substitutions,
                        &mut candidate_const_substitutions,
                    )
                })
            || !self.match_type_pattern_with_consts(
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
        const_substitutions: &mut SymbolMap<nia_ty::ConstGenericArg>,
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
        const_substitutions: &mut SymbolMap<nia_ty::ConstGenericArg>,
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
