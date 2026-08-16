// SPDX-License-Identifier: GPL-3.0-or-later
//! Structural ordering for generic type patterns.
//!
//! Candidate selection treats one pattern as more specific when the general
//! pattern can bind to it, but the reverse binding is impossible. Repeated
//! type and const parameters share substitution maps so repeated parameters
//! cannot bind to different components. Recursive probes are transactional:
//! failed associated-binding alternatives never leak partial substitutions.

use super::*;

#[derive(Clone, Default)]
struct PatternSubstitutions {
    types: SymbolMap<InternedTyId>,
    consts: SymbolMap<nia_ty::ConstGenericArg>,
}

impl<'a> BodyChecker<'a> {
    pub(crate) fn strictly_more_specific(
        &mut self,
        specific: InternedTyId,
        general: InternedTyId,
    ) -> bool {
        self.pattern_subsumes(general, specific) && !self.pattern_subsumes(specific, general)
    }

    pub(crate) fn pattern_subsumes(
        &mut self,
        general: InternedTyId,
        specific: InternedTyId,
    ) -> bool {
        self.pattern_subsumes_inner(general, specific, &mut PatternSubstitutions::default())
    }

    pub(crate) fn const_patterns_subsume(
        &mut self,
        general: &[nia_ty::ConstGenericArg],
        specific: &[nia_ty::ConstGenericArg],
    ) -> bool {
        self.const_pattern_args_subsume(general, specific, &mut PatternSubstitutions::default())
    }

    fn pattern_subsumes_inner(
        &mut self,
        general: InternedTyId,
        specific: InternedTyId,
        substitutions: &mut PatternSubstitutions,
    ) -> bool {
        let mut candidate = substitutions.clone();
        if !self.pattern_subsumes_candidate(general, specific, &mut candidate) {
            return false;
        }
        *substitutions = candidate;
        true
    }

    fn pattern_subsumes_candidate(
        &mut self,
        general: InternedTyId,
        specific: InternedTyId,
        substitutions: &mut PatternSubstitutions,
    ) -> bool {
        let general = self.normalization.normalize(general);
        let specific = self.normalization.normalize(specific);
        match self.interner.get(general).cloned() {
            Some(TyKind::GenericParam(name)) => {
                if let Some(existing) = substitutions.types.get(&name).copied() {
                    self.patterns_equivalent(existing, specific)
                } else {
                    substitutions.types.insert(name, specific);
                    true
                }
            }
            Some(TyKind::SelfParam) => true,
            Some(TyKind::Primitive(general_primitive)) => matches!(
                self.interner.get(specific).cloned(),
                Some(TyKind::Primitive(specific_primitive)) if general_primitive == specific_primitive
            ),
            Some(TyKind::Opaque) => {
                matches!(self.interner.get(specific), Some(TyKind::Opaque))
            }
            Some(TyKind::Tuple(general_elems)) => match self.interner.get(specific).cloned() {
                Some(TyKind::Tuple(specific_elems))
                    if general_elems.len() == specific_elems.len() =>
                {
                    general_elems
                        .iter()
                        .zip(&specific_elems)
                        .all(|(general, specific)| {
                            self.pattern_subsumes_inner(*general, *specific, substitutions)
                        })
                }
                _ => false,
            },
            Some(TyKind::BuiltinType(general_builtin)) => matches!(
                self.interner.get(specific).cloned(),
                Some(TyKind::BuiltinType(specific_builtin)) if general_builtin == specific_builtin
            ),
            Some(TyKind::Vector {
                elem: general_elem,
                lanes: general_lanes,
            }) => matches!(
                self.interner.get(specific).cloned(),
                Some(TyKind::Vector {
                    elem: specific_elem,
                    lanes: specific_lanes,
                }) if general_elem == specific_elem && general_lanes == specific_lanes
            ),
            Some(TyKind::Pointer {
                is_readonly: general_const,
                elem: general_elem,
            }) => match self.interner.get(specific).cloned() {
                Some(TyKind::Pointer {
                    is_readonly: specific_const,
                    elem: specific_elem,
                }) => {
                    general_const == specific_const
                        && self.pattern_subsumes_inner(general_elem, specific_elem, substitutions)
                }
                _ => false,
            },
            Some(TyKind::VolatilePointer {
                is_readonly: general_const,
                elem: general_elem,
            }) => match self.interner.get(specific).cloned() {
                Some(TyKind::VolatilePointer {
                    is_readonly: specific_const,
                    elem: specific_elem,
                }) => {
                    general_const == specific_const
                        && self.pattern_subsumes_inner(general_elem, specific_elem, substitutions)
                }
                _ => false,
            },
            Some(TyKind::Slice {
                is_readonly: general_const,
                elem: general_elem,
            }) => match self.interner.get(specific).cloned() {
                Some(TyKind::Slice {
                    is_readonly: specific_const,
                    elem: specific_elem,
                }) => {
                    general_const == specific_const
                        && self.pattern_subsumes_inner(general_elem, specific_elem, substitutions)
                }
                _ => false,
            },
            Some(TyKind::SlicePointee { elem: general_elem }) => {
                match self.interner.get(specific).cloned() {
                    Some(TyKind::SlicePointee {
                        elem: specific_elem,
                    }) => self.pattern_subsumes_inner(general_elem, specific_elem, substitutions),
                    _ => false,
                }
            }
            Some(TyKind::Array {
                len: general_len,
                elem: general_elem,
            }) => match self.interner.get(specific).cloned() {
                Some(TyKind::Array {
                    len: specific_len,
                    elem: specific_elem,
                }) if self.array_len_pattern_subsumes(
                    &general_len,
                    &specific_len,
                    substitutions,
                ) =>
                {
                    self.pattern_subsumes_inner(general_elem, specific_elem, substitutions)
                }
                _ => false,
            },
            Some(TyKind::Range {
                kind: general_kind,
                bound: general_bound,
            }) => match self.interner.get(specific).cloned() {
                Some(TyKind::Range {
                    kind: specific_kind,
                    bound: specific_bound,
                }) if general_kind == specific_kind => match (general_bound, specific_bound) {
                    (Some(general_bound), Some(specific_bound)) => {
                        self.pattern_subsumes_inner(general_bound, specific_bound, substitutions)
                    }
                    (None, None) => true,
                    _ => false,
                },
                _ => false,
            },
            Some(TyKind::FunctionPointer {
                params: general_params,
                return_type: general_return,
                is_variadic: general_variadic,
            }) => match self.interner.get(specific).cloned() {
                Some(TyKind::FunctionPointer {
                    params: specific_params,
                    return_type: specific_return,
                    is_variadic: specific_variadic,
                }) if general_variadic == specific_variadic
                    && general_params.len() == specific_params.len() =>
                {
                    general_params
                        .iter()
                        .zip(&specific_params)
                        .all(|(general, specific)| {
                            self.pattern_subsumes_inner(*general, *specific, substitutions)
                        })
                        && self.pattern_subsumes_inner(
                            general_return,
                            specific_return,
                            substitutions,
                        )
                }
                _ => false,
            },
            Some(TyKind::Callable {
                is_readonly: general_readonly,
                params: general_params,
                return_type: general_return,
            }) => match self.interner.get(specific).cloned() {
                Some(TyKind::Callable {
                    is_readonly: specific_readonly,
                    params: specific_params,
                    return_type: specific_return,
                }) if general_readonly == specific_readonly
                    && general_params.len() == specific_params.len() =>
                {
                    general_params
                        .iter()
                        .zip(&specific_params)
                        .all(|(general, specific)| {
                            self.pattern_subsumes_inner(*general, *specific, substitutions)
                        })
                        && self.pattern_subsumes_inner(
                            general_return,
                            specific_return,
                            substitutions,
                        )
                }
                _ => false,
            },
            Some(TyKind::CallablePointee {
                params: general_params,
                return_type: general_return,
            }) => match self.interner.get(specific).cloned() {
                Some(TyKind::CallablePointee {
                    params: specific_params,
                    return_type: specific_return,
                }) if general_params.len() == specific_params.len() => {
                    general_params
                        .iter()
                        .zip(&specific_params)
                        .all(|(general, specific)| {
                            self.pattern_subsumes_inner(*general, *specific, substitutions)
                        })
                        && self.pattern_subsumes_inner(
                            general_return,
                            specific_return,
                            substitutions,
                        )
                }
                _ => false,
            },
            Some(TyKind::Optional { elem: general_elem }) => {
                match self.interner.get(specific).cloned() {
                    Some(TyKind::Optional {
                        elem: specific_elem,
                    }) => self.pattern_subsumes_inner(general_elem, specific_elem, substitutions),
                    _ => false,
                }
            }
            Some(TyKind::ErrorUnion {
                error: general_error,
                value: general_value,
            }) => match self.interner.get(specific).cloned() {
                Some(TyKind::ErrorUnion {
                    error: specific_error,
                    value: specific_value,
                }) => {
                    self.pattern_subsumes_inner(general_error, specific_error, substitutions)
                        && self.pattern_subsumes_inner(general_value, specific_value, substitutions)
                }
                _ => false,
            },
            Some(TyKind::Nominal {
                def_id: general_def,
                args: general_args,
                const_args: general_const_args,
            }) => match self.interner.get(specific).cloned() {
                Some(TyKind::Nominal {
                    def_id: specific_def,
                    args: specific_args,
                    const_args: specific_const_args,
                }) => {
                    general_def == specific_def
                        && general_args.len() == specific_args.len()
                        && self.const_pattern_args_subsume(
                            &general_const_args,
                            &specific_const_args,
                            substitutions,
                        )
                        && general_args
                            .iter()
                            .zip(&specific_args)
                            .all(|(general, specific)| {
                                self.pattern_subsumes_inner(*general, *specific, substitutions)
                            })
                }
                _ => false,
            },
            Some(TyKind::BuiltinTrait {
                trait_id: general_trait,
                args: general_args,
            }) => match self.interner.get(specific).cloned() {
                Some(TyKind::BuiltinTrait {
                    trait_id: specific_trait,
                    args: specific_args,
                }) if general_trait == specific_trait
                    && general_args.len() == specific_args.len() =>
                {
                    general_args
                        .iter()
                        .zip(&specific_args)
                        .all(|(general, specific)| {
                            self.pattern_subsumes_inner(*general, *specific, substitutions)
                        })
                }
                _ => false,
            },
            Some(TyKind::TraitObject {
                is_readonly: general_const,
                trait_id: general_trait,
                trait_args: general_args,
                trait_const_args: general_const_args,
                associated_type_bindings: general_bindings,
            }) => match self.interner.get(specific).cloned() {
                Some(TyKind::TraitObject {
                    is_readonly: specific_const,
                    trait_id: specific_trait,
                    trait_args: specific_args,
                    trait_const_args: specific_const_args,
                    associated_type_bindings: specific_bindings,
                }) => {
                    general_const == specific_const
                        && general_trait == specific_trait
                        && general_args.len() == specific_args.len()
                        && self.const_pattern_args_subsume(
                            &general_const_args,
                            &specific_const_args,
                            substitutions,
                        )
                        && general_bindings.len() == specific_bindings.len()
                        && general_args
                            .iter()
                            .zip(&specific_args)
                            .all(|(general, specific)| {
                                self.pattern_subsumes_inner(*general, *specific, substitutions)
                            })
                        && general_bindings.iter().all(|general_binding| {
                            specific_bindings
                                .iter()
                                .find(|specific_binding| {
                                    general_binding.name == specific_binding.name
                                        && general_binding.trait_id == specific_binding.trait_id
                                        && general_binding.trait_args.len()
                                            == specific_binding.trait_args.len()
                                        && self.const_pattern_args_subsume(
                                            &general_binding.trait_const_args,
                                            &specific_binding.trait_const_args,
                                            substitutions,
                                        )
                                        && general_binding
                                            .trait_args
                                            .iter()
                                            .zip(&specific_binding.trait_args)
                                            .all(|(general, specific)| {
                                                self.pattern_subsumes_inner(
                                                    *general,
                                                    *specific,
                                                    substitutions,
                                                )
                                            })
                                })
                                .is_some_and(|specific_binding| {
                                    self.pattern_subsumes_inner(
                                        general_binding.ty,
                                        specific_binding.ty,
                                        substitutions,
                                    )
                                })
                        })
                }
                _ => false,
            },
            Some(TyKind::TraitObjectPointee {
                trait_id: general_trait,
                trait_args: general_args,
                trait_const_args: general_const_args,
                associated_type_bindings: general_bindings,
            }) => match self.interner.get(specific).cloned() {
                Some(TyKind::TraitObjectPointee {
                    trait_id: specific_trait,
                    trait_args: specific_args,
                    trait_const_args: specific_const_args,
                    associated_type_bindings: specific_bindings,
                }) => {
                    general_trait == specific_trait
                        && general_args.len() == specific_args.len()
                        && self.const_pattern_args_subsume(
                            &general_const_args,
                            &specific_const_args,
                            substitutions,
                        )
                        && general_bindings.len() == specific_bindings.len()
                        && general_args
                            .iter()
                            .zip(&specific_args)
                            .all(|(general, specific)| {
                                self.pattern_subsumes_inner(*general, *specific, substitutions)
                            })
                        && general_bindings.iter().all(|general_binding| {
                            specific_bindings
                                .iter()
                                .find(|specific_binding| {
                                    general_binding.name == specific_binding.name
                                        && general_binding.trait_id == specific_binding.trait_id
                                        && general_binding.trait_args.len()
                                            == specific_binding.trait_args.len()
                                        && self.const_pattern_args_subsume(
                                            &general_binding.trait_const_args,
                                            &specific_binding.trait_const_args,
                                            substitutions,
                                        )
                                        && general_binding
                                            .trait_args
                                            .iter()
                                            .zip(&specific_binding.trait_args)
                                            .all(|(general, specific)| {
                                                self.pattern_subsumes_inner(
                                                    *general,
                                                    *specific,
                                                    substitutions,
                                                )
                                            })
                                })
                                .is_some_and(|specific_binding| {
                                    self.pattern_subsumes_inner(
                                        general_binding.ty,
                                        specific_binding.ty,
                                        substitutions,
                                    )
                                })
                        })
                }
                _ => false,
            },
            Some(TyKind::Projection {
                self_ty: general_self,
                trait_id: general_trait,
                trait_args: general_args,
                trait_const_args: general_const_args,
                name: general_name,
            }) => match self.interner.get(specific).cloned() {
                Some(TyKind::Projection {
                    self_ty: specific_self,
                    trait_id: specific_trait,
                    trait_args: specific_args,
                    trait_const_args: specific_const_args,
                    name: specific_name,
                }) if general_trait == specific_trait
                    && general_name == specific_name
                    && general_args.len() == specific_args.len()
                    && self.const_pattern_args_subsume(
                        &general_const_args,
                        &specific_const_args,
                        substitutions,
                    ) =>
                {
                    self.pattern_subsumes_inner(general_self, specific_self, substitutions)
                        && general_args
                            .iter()
                            .zip(&specific_args)
                            .all(|(general, specific)| {
                                self.pattern_subsumes_inner(*general, *specific, substitutions)
                            })
                }
                _ => false,
            },
            Some(TyKind::ConstOnly | TyKind::Error | TyKind::ClosureState { .. }) => false,
            None => panic!(
                "Nia ICE: method pattern type {:?} is missing from type store {:?}",
                general,
                self.interner.store_id()
            ),
        }
    }

    fn array_len_pattern_subsumes(
        &mut self,
        general: &ArrayLenTy,
        specific: &ArrayLenTy,
        substitutions: &mut PatternSubstitutions,
    ) -> bool {
        if self.array_lens_match(general, specific) {
            return true;
        }
        let ArrayLenTy::GenericParam(name) = general else {
            return false;
        };
        let Some(value) = self.method_const_generic_value_from_array_len(specific) else {
            return false;
        };
        let specific = nia_ty::ConstGenericArg {
            ty: self.interner.primitive(PrimitiveTy::Usize),
            value,
        };
        self.record_const_pattern_subsumption(*name, specific, substitutions)
    }

    fn const_pattern_args_subsume(
        &mut self,
        general: &[nia_ty::ConstGenericArg],
        specific: &[nia_ty::ConstGenericArg],
        substitutions: &mut PatternSubstitutions,
    ) -> bool {
        general.len() == specific.len()
            && general.iter().zip(specific).all(|(general, specific)| {
                self.const_pattern_arg_subsumes(general, specific, substitutions)
            })
    }

    fn const_pattern_arg_subsumes(
        &mut self,
        general: &nia_ty::ConstGenericArg,
        specific: &nia_ty::ConstGenericArg,
        substitutions: &mut PatternSubstitutions,
    ) -> bool {
        if !self.pattern_subsumes_inner(general.ty, specific.ty, substitutions) {
            return false;
        }
        match general.value {
            nia_ty::ConstGenericValue::GenericParam(name) => {
                self.record_const_pattern_subsumption(name, specific.clone(), substitutions)
            }
            _ => self.const_generic_args_match(general, specific),
        }
    }

    fn record_const_pattern_subsumption(
        &mut self,
        name: SymbolId,
        specific: nia_ty::ConstGenericArg,
        substitutions: &mut PatternSubstitutions,
    ) -> bool {
        if let Some(existing) = substitutions.consts.get(&name).cloned() {
            self.const_generic_args_match(&existing, &specific)
        } else {
            substitutions.consts.insert(name, specific);
            true
        }
    }

    fn patterns_equivalent(&mut self, left: InternedTyId, right: InternedTyId) -> bool {
        self.types_equivalent_without_projection_resolution(left, right)
    }
}
