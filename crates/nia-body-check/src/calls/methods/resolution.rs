// SPDX-License-Identifier: GPL-3.0-or-later
use super::*;

impl<'a> BodyChecker<'a> {
    pub(in crate::calls) fn method_candidates_for_struct(
        &mut self,
        struct_id: GlobalDefId,
        name: &str,
    ) -> Vec<MethodCandidate> {
        self.extensions
            .all_methods_named(name)
            .into_iter()
            .filter_map(|(target_ty, method_id)| {
                let target_ty = self.normalization.normalize(target_ty);
                matches!(
                    self.interner.get(target_ty),
                    Some(TyKind::Nominal { def_id, .. }) if *def_id == struct_id
                )
                .then_some(MethodCandidate {
                    target_ty,
                    method_id,
                })
            })
            .collect()
    }

    pub(in crate::calls) fn method_candidates_for_target(
        &mut self,
        target_ty: InternedTyId,
        name: &str,
    ) -> Vec<MethodCandidate> {
        self.extensions
            .all_methods_named(name)
            .into_iter()
            .filter_map(|(candidate_ty, method_id)| {
                self.match_type_pattern(candidate_ty, target_ty, &mut HashMap::new())
                    .then_some(MethodCandidate {
                        target_ty: candidate_ty,
                        method_id,
                    })
            })
            .collect()
    }

    pub(in crate::calls::methods) fn method_candidates_for_receiver(
        &mut self,
        receiver_ty: InternedTyId,
        name: &str,
    ) -> Vec<MethodCandidate> {
        let mut receiver_ty = self.normalization.normalize(receiver_ty);
        loop {
            let candidates = self
                .extensions
                .all_methods_named(name)
                .into_iter()
                .filter_map(|(target_ty, method_id)| {
                    let target_ty = self.normalization.normalize(target_ty);
                    self.match_type_pattern(target_ty, receiver_ty, &mut HashMap::new())
                        .then_some(MethodCandidate {
                            target_ty,
                            method_id,
                        })
                })
                .collect::<Vec<_>>();
            if !candidates.is_empty() {
                return candidates;
            }
            match self.interner.get(receiver_ty) {
                Some(TyKind::Pointer { elem, .. }) => {
                    receiver_ty = self.normalization.normalize(*elem);
                }
                _ => return Vec::new(),
            }
        }
    }

    pub(in crate::calls::methods) fn trait_method_candidates_for_receiver(
        &mut self,
        receiver_ty: InternedTyId,
        name: &str,
    ) -> Vec<TraitMethodCandidate> {
        let Some(self_ty) = self.generic_receiver_self_ty(receiver_ty) else {
            return Vec::new();
        };
        let mut candidates = Vec::new();
        let Some(current_def_id) = self.current_def_id else {
            return candidates;
        };
        let Some(signature) = self
            .resolved_function_signature(current_def_id)
            .map(|resolved| resolved.signature)
        else {
            return candidates;
        };
        if let Some(trait_id) = self.current_trait_method_parent(current_def_id) {
            if let Some(trait_signature) = self.resolved_trait_signature(trait_id) {
                let trait_args = trait_signature
                    .generics
                    .iter()
                    .map(|generic| self.interner.intern(TyKind::GenericParam(generic.clone())))
                    .collect::<Vec<_>>();
                self.push_trait_method_candidates(
                    &mut candidates,
                    trait_id,
                    trait_args,
                    self_ty,
                    name,
                    &trait_signature,
                );
            }
        }
        for predicate in &signature.where_predicates {
            if !self.types_match(predicate.ty, self_ty) {
                continue;
            }
            for bound in &predicate.bounds {
                let Some(TyKind::Nominal {
                    def_id: trait_id,
                    args: trait_args,
                }) = self
                    .interner
                    .get(self.normalization.normalize(bound.trait_ty))
                    .cloned()
                else {
                    continue;
                };
                let Some(trait_signature) = self.resolved_trait_signature(trait_id) else {
                    continue;
                };
                self.push_trait_method_candidates(
                    &mut candidates,
                    trait_id,
                    trait_args,
                    self_ty,
                    name,
                    &trait_signature,
                );
            }
        }
        candidates
    }

    pub(in crate::calls::methods) fn dynamic_trait_method_candidates_for_receiver(
        &mut self,
        receiver_ty: InternedTyId,
        name: &str,
    ) -> Vec<DynamicTraitMethodCandidate> {
        let receiver_ty = self.normalization.normalize(receiver_ty);
        let Some(TyKind::TraitObject {
            trait_id,
            trait_args,
            ..
        }) = self.interner.get(receiver_ty).cloned()
        else {
            return Vec::new();
        };
        if !self.is_object_safe_trait_object(Span::new(0, 0), trait_id, &trait_args) {
            return Vec::new();
        }
        let mut candidates = Vec::new();
        self.push_dynamic_trait_method_candidates(
            &mut candidates,
            receiver_ty,
            trait_id,
            trait_args,
            name,
            &mut 0,
            &mut Vec::new(),
        );
        candidates
    }

    fn push_dynamic_trait_method_candidates(
        &mut self,
        candidates: &mut Vec<DynamicTraitMethodCandidate>,
        object_ty: InternedTyId,
        trait_id: TraitId,
        trait_args: Vec<InternedTyId>,
        name: &str,
        next_slot: &mut usize,
        visiting: &mut Vec<TraitId>,
    ) {
        if visiting.contains(&trait_id) {
            return;
        }
        visiting.push(trait_id);
        let TraitId::Source(source_trait_id) = trait_id else {
            visiting.pop();
            return;
        };
        let Some(trait_signature) = self.resolved_trait_signature(source_trait_id) else {
            visiting.pop();
            return;
        };
        for method in &trait_signature.methods {
            let slot = *next_slot;
            *next_slot += 1;
            if method.name != name {
                continue;
            }
            candidates.push(DynamicTraitMethodCandidate {
                object_ty,
                trait_id,
                method_id: GlobalDefId {
                    module_id: source_trait_id.module_id,
                    def_id: method.def_id,
                },
                trait_generics: trait_signature.generics.clone(),
                trait_args: trait_args.clone(),
                signature: method.signature.clone(),
                slot,
            });
        }
        let substitutions = self.generic_substitutions(&trait_signature.generics, &trait_args);
        for supertrait in &trait_signature.supertraits {
            let supertrait = self.substitute_generics(*supertrait, &substitutions);
            let Some(TyKind::Nominal {
                def_id: supertrait_id,
                args: supertrait_args,
            }) = self
                .interner
                .get(self.normalization.normalize(supertrait))
                .cloned()
            else {
                continue;
            };
            self.push_dynamic_trait_method_candidates(
                candidates,
                object_ty,
                TraitId::Source(supertrait_id),
                supertrait_args,
                name,
                next_slot,
                visiting,
            );
        }
        visiting.pop();
    }

    pub(in crate::calls::methods) fn trait_object_self_ty(
        &mut self,
        object_ty: InternedTyId,
    ) -> InternedTyId {
        let object_ty = self.normalization.normalize(object_ty);
        let Some(TyKind::TraitObject { is_const, .. }) = self.interner.get(object_ty).cloned()
        else {
            return object_ty;
        };
        self.interner.intern(TyKind::Pointer {
            is_const,
            elem: object_ty,
        })
    }

    fn push_trait_method_candidates(
        &mut self,
        candidates: &mut Vec<TraitMethodCandidate>,
        trait_id: GlobalDefId,
        trait_args: Vec<InternedTyId>,
        self_ty: InternedTyId,
        name: &str,
        trait_signature: &nia_item_signatures::TraitSignature,
    ) {
        for method in &trait_signature.methods {
            if method.name == name {
                candidates.push(TraitMethodCandidate {
                    trait_id,
                    method_id: GlobalDefId {
                        module_id: trait_id.module_id,
                        def_id: method.def_id,
                    },
                    self_ty,
                    trait_generics: trait_signature.generics.clone(),
                    trait_args: trait_args.clone(),
                    signature: method.signature.clone(),
                    has_default: method.has_default,
                });
            }
        }
        let substitutions = self.generic_substitutions(&trait_signature.generics, &trait_args);
        for supertrait in &trait_signature.supertraits {
            let supertrait = self.substitute_generics(*supertrait, &substitutions);
            let Some(TyKind::Nominal {
                def_id: supertrait_id,
                args: supertrait_args,
            }) = self
                .interner
                .get(self.normalization.normalize(supertrait))
                .cloned()
            else {
                continue;
            };
            let Some(supertrait_signature) = self.resolved_trait_signature(supertrait_id) else {
                continue;
            };
            self.push_trait_method_candidates(
                candidates,
                supertrait_id,
                supertrait_args,
                self_ty,
                name,
                &supertrait_signature,
            );
        }
    }

    fn current_trait_method_parent(&self, current_def_id: GlobalDefId) -> Option<GlobalDefId> {
        if current_def_id.module_id != self.defs.module_id {
            return None;
        }
        let def = self.defs.defs.get(current_def_id.def_id)?;
        if def.kind != nia_defs::DefKind::TraitMethod {
            return None;
        }
        Some(GlobalDefId {
            module_id: current_def_id.module_id,
            def_id: def.parent?,
        })
    }

    fn generic_receiver_self_ty(&mut self, receiver_ty: InternedTyId) -> Option<InternedTyId> {
        let receiver_ty = self.normalization.normalize(receiver_ty);
        match self.interner.get(receiver_ty).cloned() {
            Some(TyKind::GenericParam(_)) => Some(receiver_ty),
            Some(TyKind::Pointer { elem, .. }) => {
                let elem = self.normalization.normalize(elem);
                matches!(self.interner.get(elem), Some(TyKind::GenericParam(_))).then_some(elem)
            }
            _ => None,
        }
    }

    pub(in crate::calls) fn single_method_candidate(
        &mut self,
        span: Span,
        name: &str,
        candidates: Vec<MethodCandidate>,
    ) -> Option<GlobalDefId> {
        let candidates = self.most_specific_candidates(&candidates);
        match candidates.as_slice() {
            [method] => Some(method.method_id),
            [] => None,
            _ => {
                self.diagnostics.push(Diagnostic::error(
                    span,
                    format!("ambiguous method `{name}`"),
                ));
                None
            }
        }
    }

    fn most_specific_candidates(&self, candidates: &[MethodCandidate]) -> Vec<MethodCandidate> {
        candidates
            .iter()
            .copied()
            .filter(|candidate| {
                !candidates.iter().any(|other| {
                    other.method_id != candidate.method_id
                        && self.strictly_more_specific(other.target_ty, candidate.target_ty)
                })
            })
            .collect()
    }

    fn strictly_more_specific(&self, specific: InternedTyId, general: InternedTyId) -> bool {
        self.pattern_subsumes(general, specific) && !self.pattern_subsumes(specific, general)
    }

    fn pattern_subsumes(&self, general: InternedTyId, specific: InternedTyId) -> bool {
        self.pattern_subsumes_inner(general, specific, &mut HashMap::new())
    }

    fn pattern_subsumes_inner(
        &self,
        general: InternedTyId,
        specific: InternedTyId,
        substitutions: &mut HashMap<String, InternedTyId>,
    ) -> bool {
        let general = self.normalization.normalize(general);
        let specific = self.normalization.normalize(specific);
        match self.interner.get(general) {
            Some(TyKind::GenericParam(name)) => {
                if let Some(existing) = substitutions.get(name).copied() {
                    self.patterns_equivalent(existing, specific)
                } else {
                    substitutions.insert(name.clone(), specific);
                    true
                }
            }
            Some(TyKind::Primitive(general_primitive)) => matches!(
                self.interner.get(specific),
                Some(TyKind::Primitive(specific_primitive)) if general_primitive == specific_primitive
            ),
            Some(TyKind::Pointer {
                is_const: general_const,
                elem: general_elem,
            }) => matches!(
                self.interner.get(specific),
                Some(TyKind::Pointer {
                    is_const: specific_const,
                    elem: specific_elem,
                }) if general_const == specific_const
                    && self.pattern_subsumes_inner(*general_elem, *specific_elem, substitutions)
            ),
            Some(TyKind::Slice {
                is_const: general_const,
                elem: general_elem,
            }) => matches!(
                self.interner.get(specific),
                Some(TyKind::Slice {
                    is_const: specific_const,
                    elem: specific_elem,
                }) if general_const == specific_const
                    && self.pattern_subsumes_inner(*general_elem, *specific_elem, substitutions)
            ),
            Some(TyKind::Array {
                len: general_len,
                elem: general_elem,
            }) => match self.interner.get(specific) {
                Some(TyKind::Array {
                    len: specific_len,
                    elem: specific_elem,
                }) if self.array_lens_match(general_len, specific_len) => {
                    self.pattern_subsumes_inner(*general_elem, *specific_elem, substitutions)
                }
                _ => false,
            },
            Some(TyKind::Range {
                kind: general_kind,
                bound: general_bound,
            }) => match self.interner.get(specific) {
                Some(TyKind::Range {
                    kind: specific_kind,
                    bound: specific_bound,
                }) if general_kind == specific_kind => match (general_bound, specific_bound) {
                    (Some(general_bound), Some(specific_bound)) => {
                        self.pattern_subsumes_inner(*general_bound, *specific_bound, substitutions)
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
            }) => match self.interner.get(specific) {
                Some(TyKind::FunctionPointer {
                    params: specific_params,
                    return_type: specific_return,
                    is_variadic: specific_variadic,
                }) if general_variadic == specific_variadic
                    && general_params.len() == specific_params.len() =>
                {
                    general_params
                        .iter()
                        .zip(specific_params)
                        .all(|(general, specific)| {
                            self.pattern_subsumes_inner(*general, *specific, substitutions)
                        })
                        && self.pattern_subsumes_inner(
                            *general_return,
                            *specific_return,
                            substitutions,
                        )
                }
                _ => false,
            },
            Some(TyKind::Nominal {
                def_id: general_def,
                args: general_args,
            }) => match self.interner.get(specific) {
                Some(TyKind::Nominal {
                    def_id: specific_def,
                    args: specific_args,
                }) if general_def == specific_def && general_args.len() == specific_args.len() => {
                    general_args
                        .iter()
                        .zip(specific_args)
                        .all(|(general, specific)| {
                            self.pattern_subsumes_inner(*general, *specific, substitutions)
                        })
                }
                _ => false,
            },
            Some(TyKind::BuiltinTrait {
                trait_id: general_trait,
                args: general_args,
            }) => match self.interner.get(specific) {
                Some(TyKind::BuiltinTrait {
                    trait_id: specific_trait,
                    args: specific_args,
                }) if general_trait == specific_trait
                    && general_args.len() == specific_args.len() =>
                {
                    general_args
                        .iter()
                        .zip(specific_args)
                        .all(|(general, specific)| {
                            self.pattern_subsumes_inner(*general, *specific, substitutions)
                        })
                }
                _ => false,
            },
            Some(TyKind::TraitObject {
                is_const: general_const,
                trait_id: general_trait,
                trait_args: general_args,
                associated_type_bindings: general_bindings,
            }) => match self.interner.get(specific) {
                Some(TyKind::TraitObject {
                    is_const: specific_const,
                    trait_id: specific_trait,
                    trait_args: specific_args,
                    associated_type_bindings: specific_bindings,
                }) if general_const == specific_const
                    && general_trait == specific_trait
                    && general_args.len() == specific_args.len()
                    && general_bindings.len() == specific_bindings.len() =>
                {
                    general_args
                        .iter()
                        .zip(specific_args)
                        .all(|(general, specific)| {
                            self.pattern_subsumes_inner(*general, *specific, substitutions)
                        })
                        && general_bindings.iter().all(|(general_name, general_ty)| {
                            specific_bindings
                                .iter()
                                .find(|(specific_name, _)| specific_name == general_name)
                                .is_some_and(|(_, specific_ty)| {
                                    self.pattern_subsumes_inner(
                                        *general_ty,
                                        *specific_ty,
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
                name: general_name,
            }) => match self.interner.get(specific) {
                Some(TyKind::Projection {
                    self_ty: specific_self,
                    trait_id: specific_trait,
                    trait_args: specific_args,
                    name: specific_name,
                }) if general_trait == specific_trait
                    && general_name == specific_name
                    && general_args.len() == specific_args.len() =>
                {
                    self.pattern_subsumes_inner(*general_self, *specific_self, substitutions)
                        && general_args
                            .iter()
                            .zip(specific_args)
                            .all(|(general, specific)| {
                                self.pattern_subsumes_inner(*general, *specific, substitutions)
                            })
                }
                _ => false,
            },
            Some(TyKind::Error) | None => false,
        }
    }

    fn patterns_equivalent(&self, left: InternedTyId, right: InternedTyId) -> bool {
        self.pattern_subsumes(left, right) && self.pattern_subsumes(right, left)
    }

    pub(in crate::calls::methods) fn lowered_method_type_args(
        &mut self,
        type_args: Option<&[BracketArg]>,
    ) -> Option<Vec<InternedTyId>> {
        type_args
            .map(|args| self.lower_bracket_type_args(args))
            .or(Some(Vec::new()))
    }

    pub(in crate::calls::methods) fn method_generic_substitutions(
        &mut self,
        context: MethodGenericContext<'_>,
        signature: &FunctionSignature,
    ) -> Option<HashMap<String, InternedTyId>> {
        let mut substitutions =
            self.extension_target_substitutions(context.method_id, context.receiver_ty);
        let method_arg_count = context.lowered_method_args.len();
        if context.method_args.is_some() && signature.generics.len() != method_arg_count {
            self.diagnostics.push(Diagnostic::error(
                context.span,
                format!(
                    "generic argument count mismatch for method: expected {}, got {method_arg_count}",
                    signature.generics.len()
                ),
            ));
            return None;
        }
        if context.method_args.is_some() {
            substitutions.extend(
                self.generic_substitutions(&signature.generics, context.lowered_method_args),
            );
        } else if let Some(expected) = context.expected {
            self.infer_generics_from_type(
                signature.return_type,
                expected,
                &mut substitutions,
                context.span,
            );
        }
        Some(substitutions)
    }

    pub(in crate::calls) fn extension_target_substitutions(
        &mut self,
        method_id: GlobalDefId,
        receiver_ty: InternedTyId,
    ) -> HashMap<String, InternedTyId> {
        let Some(target_ty) = self.extension_target_ty_for_method(method_id) else {
            return HashMap::new();
        };
        let mut substitutions = HashMap::new();
        self.match_receiver_target(target_ty, receiver_ty, &mut substitutions);
        substitutions
    }

    pub(in crate::calls) fn extension_target_instance_args(
        &mut self,
        method_id: GlobalDefId,
        substitutions: &HashMap<String, InternedTyId>,
    ) -> Vec<InternedTyId> {
        let Some(target_ty) = self.extension_target_ty_for_method(method_id) else {
            return Vec::new();
        };
        self.generic_params_in_ty(target_ty)
            .iter()
            .filter_map(|generic| substitutions.get(generic).copied())
            .collect()
    }

    fn extension_target_ty_for_method(&self, method_id: GlobalDefId) -> Option<InternedTyId> {
        self.extensions
            .targets()
            .iter()
            .find(|target| {
                target
                    .methods
                    .iter()
                    .any(|method| method.def_id == method_id)
            })
            .map(|target| target.target_ty)
    }

    fn match_receiver_target(
        &self,
        target_ty: InternedTyId,
        receiver_ty: InternedTyId,
        substitutions: &mut HashMap<String, InternedTyId>,
    ) -> bool {
        let receiver_ty = self.normalization.normalize(receiver_ty);
        if self.match_type_pattern(target_ty, receiver_ty, substitutions) {
            return true;
        }
        match self.interner.get(receiver_ty) {
            Some(TyKind::Pointer { elem, .. }) => {
                self.match_receiver_target(target_ty, *elem, substitutions)
            }
            _ => false,
        }
    }

    pub(in crate::calls::methods) fn match_type_pattern(
        &self,
        pattern: InternedTyId,
        actual: InternedTyId,
        substitutions: &mut HashMap<String, InternedTyId>,
    ) -> bool {
        let pattern = self.normalization.normalize(pattern);
        let actual = self.normalization.normalize(actual);
        match self.interner.get(pattern) {
            Some(TyKind::GenericParam(name)) => {
                if let Some(existing) = substitutions.get(name).copied() {
                    self.types_match(existing, actual)
                } else {
                    substitutions.insert(name.clone(), actual);
                    true
                }
            }
            Some(TyKind::Pointer {
                is_const: pattern_const,
                elem: pattern_elem,
            }) => matches!(
                self.interner.get(actual),
                Some(TyKind::Pointer {
                    is_const,
                    elem
                }) if is_const == pattern_const
                    && self.match_type_pattern(*pattern_elem, *elem, substitutions)
            ),
            Some(TyKind::Slice {
                is_const: pattern_const,
                elem: pattern_elem,
            }) => matches!(
                self.interner.get(actual),
                Some(TyKind::Slice {
                    is_const,
                    elem
                }) if is_const == pattern_const
                    && self.match_type_pattern(*pattern_elem, *elem, substitutions)
            ),
            Some(TyKind::Array {
                len: pattern_len,
                elem: pattern_elem,
            }) => match self.interner.get(actual) {
                Some(TyKind::Array { len, elem }) if self.array_lens_match(pattern_len, len) => {
                    self.match_type_pattern(*pattern_elem, *elem, substitutions)
                }
                _ => false,
            },
            Some(TyKind::Range {
                kind: pattern_kind,
                bound: pattern_bound,
            }) => match self.interner.get(actual) {
                Some(TyKind::Range { kind, bound }) if pattern_kind == kind => {
                    match (pattern_bound, bound) {
                        (Some(pattern_bound), Some(bound)) => {
                            self.match_type_pattern(*pattern_bound, *bound, substitutions)
                        }
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
            }) => match self.interner.get(actual) {
                Some(TyKind::FunctionPointer {
                    params,
                    return_type,
                    is_variadic,
                }) if pattern_variadic == is_variadic && pattern_params.len() == params.len() => {
                    pattern_params.iter().zip(params).all(|(pattern, actual)| {
                        self.match_type_pattern(*pattern, *actual, substitutions)
                    }) && self.match_type_pattern(*pattern_return, *return_type, substitutions)
                }
                _ => false,
            },
            Some(TyKind::Nominal {
                def_id: pattern_def,
                args: pattern_args,
            }) => match self.interner.get(actual) {
                Some(TyKind::Nominal { def_id, args })
                    if pattern_def == def_id && pattern_args.len() == args.len() =>
                {
                    pattern_args.iter().zip(args).all(|(pattern, actual)| {
                        self.match_type_pattern(*pattern, *actual, substitutions)
                    })
                }
                _ => false,
            },
            Some(TyKind::BuiltinTrait {
                trait_id: pattern_trait,
                args: pattern_args,
            }) => match self.interner.get(actual) {
                Some(TyKind::BuiltinTrait { trait_id, args })
                    if pattern_trait == trait_id && pattern_args.len() == args.len() =>
                {
                    pattern_args.iter().zip(args).all(|(pattern, actual)| {
                        self.match_type_pattern(*pattern, *actual, substitutions)
                    })
                }
                _ => false,
            },
            Some(TyKind::TraitObject {
                is_const: pattern_const,
                trait_id: pattern_trait,
                trait_args: pattern_args,
                associated_type_bindings: pattern_bindings,
            }) => match self.interner.get(actual) {
                Some(TyKind::TraitObject {
                    is_const,
                    trait_id,
                    trait_args,
                    associated_type_bindings,
                }) if is_const == pattern_const
                    && trait_id == pattern_trait
                    && pattern_args.len() == trait_args.len()
                    && pattern_bindings.len() == associated_type_bindings.len() =>
                {
                    pattern_args
                        .iter()
                        .zip(trait_args)
                        .all(|(pattern, actual)| {
                            self.match_type_pattern(*pattern, *actual, substitutions)
                        })
                        && pattern_bindings.iter().all(|(pattern_name, pattern_ty)| {
                            associated_type_bindings
                                .iter()
                                .find(|(name, _)| name == pattern_name)
                                .is_some_and(|(_, actual_ty)| {
                                    self.match_type_pattern(*pattern_ty, *actual_ty, substitutions)
                                })
                        })
                }
                _ => false,
            },
            Some(TyKind::Projection {
                self_ty: pattern_self,
                trait_id: pattern_trait,
                trait_args: pattern_args,
                name: pattern_name,
            }) => match self.interner.get(actual) {
                Some(TyKind::Projection {
                    self_ty,
                    trait_id,
                    trait_args,
                    name,
                }) if pattern_trait == trait_id
                    && pattern_name == name
                    && pattern_args.len() == trait_args.len() =>
                {
                    self.match_type_pattern(*pattern_self, *self_ty, substitutions)
                        && pattern_args
                            .iter()
                            .zip(trait_args)
                            .all(|(pattern, actual)| {
                                self.match_type_pattern(*pattern, *actual, substitutions)
                            })
                }
                _ => false,
            },
            Some(TyKind::Primitive(_)) | Some(TyKind::Error) | None => {
                self.types_match(pattern, actual)
            }
        }
    }

    fn array_lens_match(&self, expected: &ArrayLenTy, actual: &ArrayLenTy) -> bool {
        if expected == actual {
            return true;
        }
        let expected = self.array_len_value(Span::default(), expected).ok();
        let actual = self.array_len_value(Span::default(), actual).ok();
        expected.is_some() && expected == actual
    }

    pub(in crate::calls::methods) fn infer_method_generics_from_args(
        &mut self,
        args: &[Expr],
        params: &[InternedTyId],
        substitutions: &mut HashMap<String, InternedTyId>,
    ) {
        let actuals = args
            .iter()
            .enumerate()
            .map(|(index, arg)| {
                if let Some(expected) = params
                    .get(index)
                    .copied()
                    .map(|param| self.substitute_generics(param, substitutions))
                {
                    self.check_expr_with_expected(arg, Some(expected))
                } else {
                    self.check_expr(arg)
                }
            })
            .collect::<Vec<_>>();
        for (param, (arg, actual)) in params.iter().zip(args.iter().zip(actuals.iter())) {
            self.infer_generics_from_type(*param, *actual, substitutions, arg.span);
        }
    }

    pub(in crate::calls::methods) fn method_generics_are_complete(
        &mut self,
        span: Span,
        signature: &FunctionSignature,
        substitutions: &HashMap<String, InternedTyId>,
    ) -> bool {
        let mut complete = true;
        for generic in &signature.generics {
            if !substitutions.contains_key(generic) {
                complete = false;
                self.diagnostics.push(Diagnostic::error(
                    span,
                    format!("cannot infer generic parameter `{generic}`"),
                ));
            }
        }
        complete
    }

    pub(in crate::calls::methods) fn check_receiver_match(
        &mut self,
        receiver: &Expr,
        receiver_ty: InternedTyId,
        receiver_kind: ReceiverKind,
    ) {
        if receiver_kind == ReceiverKind::Ref {
            let base = self.receiver_base_type(receiver_ty);
            if base.as_ref().is_some_and(|base| base.has_readonly_pointer) {
                self.diagnostics.push(Diagnostic::error(
                    receiver.span,
                    "receiver cannot be matched through `&const T`",
                ));
            } else if !base.as_ref().is_some_and(|base| base.from_pointer) {
                self.check_assignable(receiver, "receiver");
            }
        }
    }
}
