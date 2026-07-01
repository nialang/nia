// SPDX-License-Identifier: GPL-3.0-or-later
use super::*;

struct DynamicTraitMethodSearch<'a> {
    candidates: &'a mut Vec<DynamicTraitMethodCandidate>,
    object_ty: InternedTyId,
    associated_type_bindings: Vec<nia_ty::AssociatedTypeBindingTy>,
    name: &'a str,
    // Vtable slots are assigned by walking the object trait and its
    // supertraits in declaration order. This counter must be shared across the
    // recursive walk so codegen indexes the same slot order that type checking
    // recorded.
    next_slot: &'a mut usize,
    visiting: &'a mut Vec<TraitId>,
}

impl<'a> BodyChecker<'a> {
    fn all_callable_extension_methods_named(
        &mut self,
        name: &str,
    ) -> crate::CallableExtensionMethods {
        if let Some(methods) = self.callable_extension_methods_by_name.get(name) {
            return methods.clone();
        }
        let mut methods = self
            .extensions
            .all_methods_named(name)
            .into_iter()
            .filter(|(_, method)| !method.is_trait_witness)
            .map(|(target_ty, method)| crate::CallableExtensionMethod { target_ty, method })
            .collect::<Vec<_>>();
        let program_methods = self
            .program_extension_methods
            .all_methods()
            .filter(|method| method.trait_id.is_none() && method.name == name)
            .cloned()
            .collect::<Vec<_>>();
        for method in program_methods {
            if methods
                .iter()
                .any(|existing| existing.method.def_id == method.def_id)
            {
                continue;
            }
            let lookup =
                if let Some(lookup) = self.extension_method_lookup_for_id(method.def_id).cloned() {
                    lookup
                } else {
                    let Some(lookup) = self.import_program_extension_method_lookup(&method) else {
                        continue;
                    };
                    self.extension_method_lookup_cache
                        .insert(method.def_id, lookup.clone());
                    lookup
                };
            methods.push(crate::CallableExtensionMethod {
                target_ty: lookup.target_ty,
                method: VisibleExtensionMethod {
                    name: method.name.clone(),
                    def_id: method.def_id,
                    impl_id: method.impl_id,
                    effective_generics: lookup.effective_generics.clone(),
                    trait_id: method.trait_id,
                    trait_args: Vec::new(),
                    where_predicates: lookup.where_predicates.clone(),
                    is_callable: true,
                    is_trait_witness: false,
                },
            });
        }
        let mut seen = std::collections::HashSet::new();
        methods.retain(|method| seen.insert(method.method.def_id));
        let mut indexed = crate::CallableExtensionMethods {
            methods,
            ..crate::CallableExtensionMethods::default()
        };
        for (index, method) in indexed.methods.iter().enumerate() {
            if let Some(base) = self.receiver_base_type(method.target_ty) {
                indexed
                    .methods_by_base
                    .entry(base.def_id)
                    .or_default()
                    .push(index);
            } else {
                indexed.unbased_methods.push(index);
            }
        }
        self.callable_extension_methods_by_name
            .insert(name.to_string(), indexed.clone());
        indexed
    }

    pub(in crate::calls) fn method_candidates_for_target(
        &mut self,
        target_ty: InternedTyId,
        name: &str,
    ) -> Vec<MethodCandidate> {
        let methods = self.profile_stage("body_check.profile.method.callable_named", |this| {
            this.all_callable_extension_methods_named(name)
        });
        let mut candidates = Vec::new();
        for method in methods.methods {
            let candidate_ty = method.target_ty;
            let mut target_substitutions = HashMap::new();
            let mut target_const_substitutions = HashMap::new();
            if self.profile_stage("body_check.profile.method.match_target", |this| {
                this.match_type_pattern_with_consts(
                    candidate_ty,
                    target_ty,
                    &mut target_substitutions,
                    &mut target_const_substitutions,
                )
            }) {
                candidates.push(MethodCandidate {
                    target_ty: candidate_ty,
                    method: method.method,
                    target_substitutions,
                    target_const_substitutions,
                });
            }
        }
        candidates
    }

    pub(in crate::calls::methods) fn method_candidates_for_receiver(
        &mut self,
        receiver_ty: InternedTyId,
        name: &str,
    ) -> Vec<MethodCandidate> {
        let methods = self.profile_stage("body_check.profile.method.callable_named", |this| {
            this.all_callable_extension_methods_named(name)
        });
        let mut receiver_ty = self.normalize_aliases_in_type(receiver_ty);
        loop {
            let receiver_base = self.receiver_base_type(receiver_ty);
            let candidate_indexes = self.callable_method_indexes_for_receiver_base(
                &methods,
                receiver_base.as_ref().map(|base| base.def_id),
            );
            let mut candidates = Vec::new();
            for index in candidate_indexes {
                let method = &methods.methods[index];
                let target_ty = self.normalize_aliases_in_type(method.target_ty);
                if self.extension_receiver_base_mismatch(target_ty, receiver_base.as_ref()) {
                    continue;
                }
                let mut target_substitutions = HashMap::new();
                let mut target_const_substitutions = HashMap::new();
                let matches_receiver =
                    self.profile_stage("body_check.profile.method.match_receiver", |this| {
                        this.match_extension_receiver_target(
                            target_ty,
                            method.method.def_id,
                            receiver_ty,
                            &mut target_substitutions,
                            &mut target_const_substitutions,
                        )
                    });
                if matches_receiver
                    && self.extension_method_where_predicates_can_hold(
                        &method.method,
                        &target_substitutions,
                    )
                {
                    candidates.push(MethodCandidate {
                        target_ty,
                        method: method.method.clone(),
                        target_substitutions,
                        target_const_substitutions,
                    });
                }
            }
            if !candidates.is_empty() {
                return candidates;
            }
            if self.receiver_is_trait_object(receiver_ty) {
                return Vec::new();
            }
            match self.interner.get(receiver_ty) {
                Some(TyKind::Pointer { elem, .. }) => {
                    receiver_ty = self.normalize_aliases_in_type(*elem);
                }
                _ => return Vec::new(),
            }
        }
    }

    fn callable_method_indexes_for_receiver_base(
        &self,
        methods: &crate::CallableExtensionMethods,
        receiver_base: Option<GlobalDefId>,
    ) -> Vec<usize> {
        let base_methods = receiver_base
            .and_then(|base| methods.methods_by_base.get(&base))
            .into_iter()
            .flat_map(|methods| methods.iter().copied());
        base_methods
            .chain(methods.unbased_methods.iter().copied())
            .collect()
    }

    fn extension_receiver_base_mismatch(
        &self,
        target_ty: InternedTyId,
        receiver_base: Option<&crate::ReceiverBase>,
    ) -> bool {
        let Some(receiver_base) = receiver_base else {
            return false;
        };
        let Some(target_base) = self.receiver_base_type(target_ty) else {
            return false;
        };
        target_base.def_id != receiver_base.def_id
    }

    fn receiver_is_trait_object(&mut self, receiver_ty: InternedTyId) -> bool {
        let receiver_ty = self.normalize_aliases_in_type(receiver_ty);
        matches!(
            self.interner.get(receiver_ty),
            Some(TyKind::TraitObject { .. })
        )
    }

    fn receiver_candidate_target_ty(
        &mut self,
        target_ty: InternedTyId,
        method_id: GlobalDefId,
    ) -> InternedTyId {
        self.method_receiver_kind(method_id)
            .map(|receiver| self.receiver_ty_for_target(target_ty, receiver))
            .unwrap_or(target_ty)
    }

    fn method_receiver_kind(&mut self, method_id: GlobalDefId) -> Option<ReceiverKind> {
        if let Some(receiver_kind) = self.method_receiver_kinds.get(&method_id).copied() {
            return receiver_kind;
        }
        let receiver_kind = self
            .resolved_function_signature(method_id)
            .and_then(|resolved| {
                resolved
                    .signature
                    .params
                    .first()
                    .and_then(|param| param.receiver)
            });
        self.method_receiver_kinds.insert(method_id, receiver_kind);
        receiver_kind
    }

    fn match_extension_receiver_target(
        &mut self,
        target_ty: InternedTyId,
        method_id: GlobalDefId,
        receiver_ty: InternedTyId,
        substitutions: &mut HashMap<String, InternedTyId>,
        const_substitutions: &mut HashMap<String, nia_ty::ConstGenericArg>,
    ) -> bool {
        let candidate_target_ty = self.receiver_candidate_target_ty(target_ty, method_id);
        if self.try_match_type_pattern_with_consts(
            candidate_target_ty,
            receiver_ty,
            substitutions,
            const_substitutions,
        ) {
            self.bind_extension_self_from_target(target_ty, substitutions);
            return true;
        }
        if self.try_match_type_pattern_with_consts(
            target_ty,
            receiver_ty,
            substitutions,
            const_substitutions,
        ) {
            self.bind_extension_self_from_target(target_ty, substitutions);
            return true;
        }
        if self.trait_object_extension_target_matches_receiver(
            target_ty,
            receiver_ty,
            substitutions,
        ) {
            return true;
        }
        let receiver_ty = self.normalization.normalize(receiver_ty);
        if let Some(TyKind::Pointer { elem, .. }) = self.interner.get(receiver_ty) {
            return self.match_extension_receiver_target(
                target_ty,
                method_id,
                *elem,
                substitutions,
                const_substitutions,
            );
        }
        false
    }

    fn bind_extension_self_from_target(
        &mut self,
        target_ty: InternedTyId,
        substitutions: &mut HashMap<String, InternedTyId>,
    ) {
        if substitutions.contains_key("Self") {
            return;
        }
        substitutions.insert("Self".to_string(), target_ty);
    }

    fn try_match_type_pattern(
        &self,
        pattern: InternedTyId,
        actual: InternedTyId,
        substitutions: &mut HashMap<String, InternedTyId>,
    ) -> bool {
        let mut const_substitutions = HashMap::new();
        self.try_match_type_pattern_with_consts(
            pattern,
            actual,
            substitutions,
            &mut const_substitutions,
        )
    }

    fn try_match_type_pattern_with_consts(
        &self,
        pattern: InternedTyId,
        actual: InternedTyId,
        substitutions: &mut HashMap<String, InternedTyId>,
        const_substitutions: &mut HashMap<String, nia_ty::ConstGenericArg>,
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

    fn trait_object_extension_target_matches_receiver(
        &mut self,
        target_ty: InternedTyId,
        receiver_ty: InternedTyId,
        substitutions: &mut HashMap<String, InternedTyId>,
    ) -> bool {
        let Some(TyKind::TraitObjectPointee {
            trait_id,
            trait_args,
            trait_const_args,
            associated_type_bindings,
        }) = self.interner.get(target_ty).cloned()
        else {
            return false;
        };
        if !associated_type_bindings.is_empty() {
            return false;
        }
        let receiver_ty = self.normalization.normalize(receiver_ty);
        let receiver_self_ty = match self.interner.get(receiver_ty).cloned() {
            Some(TyKind::Pointer { elem, .. }) => self.normalization.normalize(elem),
            _ => receiver_ty,
        };
        let mut concrete_trait_args = Vec::new();
        for trait_arg in trait_args {
            let trait_arg = self.substitute_generics(trait_arg, substitutions);
            if matches!(
                self.interner.get(self.normalization.normalize(trait_arg)),
                Some(TyKind::GenericParam(_))
            ) {
                return false;
            }
            concrete_trait_args.push(trait_arg);
        }
        let mut concrete_trait_const_args = Vec::new();
        for mut trait_const_arg in trait_const_args {
            trait_const_arg.ty = self.substitute_generics(trait_const_arg.ty, substitutions);
            concrete_trait_const_args.push(trait_const_arg);
        }
        if !self.current_context_proves_trait_obligation_with_const_args(
            receiver_self_ty,
            trait_id,
            concrete_trait_args,
            concrete_trait_const_args,
        ) {
            return false;
        }
        substitutions.entry("Self".to_string()).or_insert(target_ty);
        true
    }

    pub(in crate::calls::methods) fn trait_method_candidates_for_receiver(
        &mut self,
        receiver_ty: InternedTyId,
        name: &str,
    ) -> Vec<TraitMethodCandidate> {
        let debug = std::env::var_os("NIA_DEBUG_FORMAT_METHOD").is_some() && name == "format";
        let Some(self_ty) = self.trait_receiver_self_ty(receiver_ty) else {
            return Vec::new();
        };
        let mut candidates = Vec::new();
        let self_ty = self.import_type_for_method_resolution(self_ty);
        if debug {
            eprintln!(
                "trait candidates for format receiver={} self={}",
                self.ty_name(receiver_ty),
                self.ty_name(self_ty)
            );
        }
        for goal in self.current_trait_goals() {
            let goal_self_ty = self.import_type_for_method_resolution(goal.self_ty);
            if debug {
                eprintln!(
                    "  goal self={} trait={:?} args={}",
                    self.ty_name(goal_self_ty),
                    goal.trait_id,
                    goal.trait_args.len()
                );
            }
            if !self.types_match(goal_self_ty, self_ty) {
                if debug {
                    eprintln!("    self mismatch");
                }
                continue;
            }
            let TraitId::Source(trait_id) = goal.trait_id else {
                continue;
            };
            let Some(trait_signature) = self.resolved_trait_signature(trait_id) else {
                continue;
            };
            let trait_args = goal
                .trait_args
                .into_iter()
                .map(|arg| self.import_type_for_method_resolution(arg))
                .collect();
            let trait_const_args = goal.trait_const_args;
            self.push_trait_method_candidates(
                &mut candidates,
                trait_id,
                trait_args,
                trait_const_args,
                self_ty,
                name,
                &trait_signature,
                true,
            );
        }
        if debug {
            eprintln!("  candidates from goals={}", candidates.len());
        }
        if !candidates.is_empty() {
            return candidates;
        }
        self.push_visible_impl_trait_method_candidates(&mut candidates, self_ty, name);
        if debug {
            eprintln!("  candidates from visible impls={}", candidates.len());
        }
        candidates
    }

    pub(in crate::calls::methods) fn trait_method_candidates_for_target(
        &mut self,
        target_ty: InternedTyId,
        name: &str,
    ) -> Vec<TraitMethodCandidate> {
        let self_ty = self.import_type_for_method_resolution(target_ty);
        let mut candidates = Vec::new();
        for goal in self.current_trait_goals() {
            let goal_self_ty = self.import_type_for_method_resolution(goal.self_ty);
            if !self.types_match(goal_self_ty, self_ty) {
                continue;
            }
            let TraitId::Source(trait_id) = goal.trait_id else {
                continue;
            };
            let Some(trait_signature) = self.resolved_trait_signature(trait_id) else {
                continue;
            };
            let trait_args = goal
                .trait_args
                .into_iter()
                .map(|arg| self.import_type_for_method_resolution(arg))
                .collect();
            let trait_const_args = goal.trait_const_args;
            self.push_trait_method_candidates(
                &mut candidates,
                trait_id,
                trait_args,
                trait_const_args,
                self_ty,
                name,
                &trait_signature,
                true,
            );
        }
        if !candidates.is_empty() {
            return candidates;
        }
        self.push_visible_impl_trait_method_candidates(&mut candidates, self_ty, name);
        candidates
    }

    fn import_type_for_method_resolution(&mut self, ty: InternedTyId) -> InternedTyId {
        if self.interner.get(ty).is_some() {
            return ty;
        }
        if self.normalization.interner.get(ty).is_some() {
            let source = self.normalization.interner.clone();
            return self.import_type_from(&source, ty);
        }
        ty
    }

    fn push_visible_impl_trait_method_candidates(
        &mut self,
        candidates: &mut Vec<TraitMethodCandidate>,
        self_ty: InternedTyId,
        name: &str,
    ) {
        for trait_id in self.trait_ids_with_method_named(name) {
            let Some(trait_signature) = self.resolved_trait_signature(trait_id) else {
                continue;
            };
            for (trait_args, trait_const_args) in
                self.visible_trait_arg_candidates(self_ty, TraitId::Source(trait_id))
            {
                self.push_trait_method_candidates(
                    candidates,
                    trait_id,
                    trait_args,
                    trait_const_args,
                    self_ty,
                    name,
                    &trait_signature,
                    false,
                );
            }
        }
    }

    fn trait_ids_with_method_named(&mut self, name: &str) -> Vec<GlobalDefId> {
        if let Some(trait_ids) = self.traits_by_method_name.get(name) {
            return trait_ids.clone();
        }
        let trait_ids = self
            .program_traits
            .iter()
            .filter_map(|(trait_id, signature)| {
                signature
                    .signature
                    .methods
                    .iter()
                    .any(|method| method.name == name)
                    .then_some(*trait_id)
            })
            .collect::<Vec<_>>();
        self.traits_by_method_name
            .insert(name.to_string(), trait_ids.clone());
        trait_ids
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
            trait_const_args,
            associated_type_bindings,
            ..
        }) = self.interner.get(receiver_ty).cloned()
        else {
            return Vec::new();
        };
        if !self.is_object_safe_trait_object(
            Span::new(0, 0),
            trait_id,
            &trait_args,
            &trait_const_args,
            &associated_type_bindings,
        ) {
            return Vec::new();
        }
        let mut candidates = Vec::new();
        let mut next_slot = 0;
        let mut visiting = Vec::new();
        self.push_dynamic_trait_method_candidates(
            &mut DynamicTraitMethodSearch {
                candidates: &mut candidates,
                object_ty: receiver_ty,
                associated_type_bindings,
                name,
                next_slot: &mut next_slot,
                visiting: &mut visiting,
            },
            trait_id,
            trait_args,
            trait_const_args,
        );
        candidates
    }

    fn push_dynamic_trait_method_candidates(
        &mut self,
        search: &mut DynamicTraitMethodSearch<'_>,
        trait_id: TraitId,
        trait_args: Vec<InternedTyId>,
        trait_const_args: Vec<ConstGenericArg>,
    ) {
        if search.visiting.contains(&trait_id) {
            return;
        }
        search.visiting.push(trait_id);
        let TraitId::Source(source_trait_id) = trait_id else {
            search.visiting.pop();
            return;
        };
        let Some(trait_signature) = self.resolved_trait_signature(source_trait_id) else {
            search.visiting.pop();
            return;
        };
        for method in &trait_signature.methods {
            let slot = *search.next_slot;
            *search.next_slot += 1;
            if method.name != search.name {
                continue;
            }
            search.candidates.push(DynamicTraitMethodCandidate {
                object_ty: search.object_ty,
                trait_id,
                method_id: GlobalDefId {
                    module_id: source_trait_id.module_id,
                    def_id: method.def_id,
                },
                trait_generics: trait_signature.generics.clone(),
                trait_args: trait_args.clone(),
                trait_const_args: trait_const_args.clone(),
                associated_type_bindings: search.associated_type_bindings.clone(),
                signature: method.signature.clone(),
                slot,
            });
        }
        let (substitutions, const_substitutions) = self.generic_substitutions_and_consts_for_def(
            source_trait_id,
            &trait_args,
            &trait_const_args,
        );
        for supertrait in &trait_signature.supertraits {
            let supertrait = self.substitute_generics_and_consts(
                supertrait.ty,
                &substitutions,
                &const_substitutions,
            );
            let Some(TyKind::Nominal {
                def_id: supertrait_id,
                args: supertrait_args,
                const_args: supertrait_const_args,
            }) = self
                .interner
                .get(self.normalization.normalize(supertrait))
                .cloned()
            else {
                continue;
            };
            self.push_dynamic_trait_method_candidates(
                search,
                TraitId::Source(supertrait_id),
                supertrait_args,
                supertrait_const_args,
            );
        }
        search.visiting.pop();
    }

    pub(in crate::calls::methods) fn trait_object_self_ty(
        &mut self,
        object_ty: InternedTyId,
    ) -> InternedTyId {
        let object_ty = self.normalization.normalize(object_ty);
        let Some(TyKind::TraitObject { is_readonly, .. }) = self.interner.get(object_ty).cloned()
        else {
            return object_ty;
        };
        self.interner.intern(TyKind::Pointer {
            is_readonly,
            elem: object_ty,
        })
    }

    fn push_trait_method_candidates(
        &mut self,
        candidates: &mut Vec<TraitMethodCandidate>,
        trait_id: GlobalDefId,
        trait_args: Vec<InternedTyId>,
        trait_const_args: Vec<ConstGenericArg>,
        self_ty: InternedTyId,
        name: &str,
        trait_signature: &nia_item_signatures::TraitSignature,
        is_assumed: bool,
    ) {
        for method in &trait_signature.methods {
            if method.name == name {
                let method_id = GlobalDefId {
                    module_id: trait_id.module_id,
                    def_id: method.def_id,
                };
                if candidates.iter().any(|candidate| {
                    candidate.trait_id == trait_id
                        && candidate.method_id == method_id
                        && self.types_equivalent_without_projection_resolution(
                            candidate.self_ty,
                            self_ty,
                        )
                        && candidate.trait_args.len() == trait_args.len()
                        && candidate.trait_const_args == trait_const_args
                        && candidate
                            .trait_args
                            .iter()
                            .zip(&trait_args)
                            .all(|(left, right)| {
                                self.types_equivalent_without_projection_resolution(*left, *right)
                            })
                }) {
                    continue;
                }
                candidates.push(TraitMethodCandidate {
                    trait_id,
                    trait_method_id: method_id,
                    method_id,
                    self_ty,
                    trait_generics: trait_signature.generics.clone(),
                    trait_args: trait_args.clone(),
                    trait_const_args: trait_const_args.clone(),
                    signature: method.signature.clone(),
                    has_default: method.has_default,
                    is_assumed,
                });
            }
        }
        let (substitutions, const_substitutions) =
            self.generic_substitutions_and_consts_for_def(trait_id, &trait_args, &trait_const_args);
        for supertrait in &trait_signature.supertraits {
            let supertrait = self.substitute_generics_and_consts(
                supertrait.ty,
                &substitutions,
                &const_substitutions,
            );
            let Some(TyKind::Nominal {
                def_id: supertrait_id,
                args: supertrait_args,
                const_args: supertrait_const_args,
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
                supertrait_const_args,
                self_ty,
                name,
                &supertrait_signature,
                is_assumed,
            );
        }
    }

    pub(crate) fn trait_receiver_self_ty(
        &mut self,
        receiver_ty: InternedTyId,
    ) -> Option<InternedTyId> {
        let receiver_ty = self.normalization.normalize(receiver_ty);
        match self.interner.get(receiver_ty).cloned() {
            Some(TyKind::TraitObject { .. }) => None,
            Some(TyKind::Pointer { elem, .. }) => {
                let elem = self.normalization.normalize(elem);
                if matches!(self.interner.get(elem), Some(TyKind::TraitObject { .. })) {
                    None
                } else {
                    Some(elem)
                }
            }
            Some(TyKind::Slice { elem, .. }) => {
                Some(self.interner.intern(TyKind::SlicePointee { elem }))
            }
            _ => Some(receiver_ty),
        }
    }

    pub(in crate::calls) fn single_method_candidate(
        &mut self,
        span: Span,
        name: &str,
        candidates: &[MethodCandidate],
    ) -> Option<MethodCandidate> {
        let mut selected = None;
        let mut count = 0;
        for candidate in candidates {
            if candidates.iter().any(|other| {
                other.method.def_id != candidate.method.def_id
                    && self.method_candidate_more_specific(other, candidate)
            }) {
                continue;
            }
            selected = Some(candidate.clone());
            count += 1;
            if count <= 1 {
                continue;
            }
            self.diagnostics.push(Diagnostic::user_error_at(
                codes::TYPE_CHECK,
                span,
                format!("ambiguous method `{name}`"),
            ));
            return None;
        }
        selected
    }

    fn method_candidate_more_specific(
        &mut self,
        specific: &MethodCandidate,
        general: &MethodCandidate,
    ) -> bool {
        if !self.pattern_subsumes(general.target_ty, specific.target_ty) {
            return false;
        }
        let mut any_strict = self.strictly_more_specific(specific.target_ty, general.target_ty);
        let Some(specific_signature) = self
            .resolved_function_signature(specific.method.def_id)
            .map(|resolved| resolved.signature)
        else {
            return any_strict;
        };
        let Some(general_signature) = self
            .resolved_function_signature(general.method.def_id)
            .map(|resolved| resolved.signature)
        else {
            return any_strict;
        };
        let specific_params = self.method_candidate_param_types(specific, &specific_signature);
        let general_params = self.method_candidate_param_types(general, &general_signature);
        if specific_params.len() != general_params.len() {
            return any_strict;
        }
        for (specific_param, general_param) in specific_params.iter().zip(&general_params) {
            if !self.pattern_subsumes(*general_param, *specific_param) {
                return false;
            }
            if self.strictly_more_specific(*specific_param, *general_param) {
                any_strict = true;
            }
        }
        any_strict
    }

    fn method_candidate_param_types(
        &mut self,
        candidate: &MethodCandidate,
        signature: &FunctionSignature,
    ) -> Vec<InternedTyId> {
        signature
            .params
            .iter()
            .skip(1)
            .map(|param| self.substitute_generics(param.ty, &candidate.target_substitutions))
            .collect()
    }

    pub(in crate::calls::methods) fn viable_method_candidates(
        &mut self,
        call: &MethodCall<'_>,
        candidates: &[MethodCandidate],
    ) -> Vec<MethodCandidate> {
        if call.type_args.is_some() || call.expected.is_none() {
            return candidates.to_vec();
        }
        candidates
            .iter()
            .filter_map(|candidate| {
                let signature = self
                    .resolved_function_signature(candidate.method.def_id)
                    .map(|resolved| resolved.signature)?;
                let mut substitutions = candidate.target_substitutions.clone();
                if let Some(expected) = call.expected {
                    // Context can refine unconstrained method generics, but a
                    // nested expression may provide an outer expected type.
                    // Use it only as an inference hint during candidate search.
                    self.try_match_type_pattern(
                        signature.return_type,
                        expected,
                        &mut substitutions,
                    );
                }
                self.extension_method_where_predicates_can_hold(&candidate.method, &substitutions)
                    .then(|| candidate.clone())
            })
            .collect()
    }

    pub(crate) fn strictly_more_specific(
        &self,
        specific: InternedTyId,
        general: InternedTyId,
    ) -> bool {
        self.pattern_subsumes(general, specific) && !self.pattern_subsumes(specific, general)
    }

    pub(crate) fn pattern_subsumes(&self, general: InternedTyId, specific: InternedTyId) -> bool {
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
            Some(TyKind::Vector {
                elem: general_elem,
                lanes: general_lanes,
            }) => matches!(
                self.interner.get(specific),
                Some(TyKind::Vector {
                    elem: specific_elem,
                    lanes: specific_lanes,
                }) if general_elem == specific_elem && general_lanes == specific_lanes
            ),
            Some(TyKind::Pointer {
                is_readonly: general_const,
                elem: general_elem,
            }) => matches!(
                self.interner.get(specific),
                Some(TyKind::Pointer {
                    is_readonly: specific_const,
                    elem: specific_elem,
                }) if general_const == specific_const
                    && self.pattern_subsumes_inner(*general_elem, *specific_elem, substitutions)
            ),
            Some(TyKind::VolatilePointer {
                is_readonly: general_const,
                elem: general_elem,
            }) => matches!(
                self.interner.get(specific),
                Some(TyKind::VolatilePointer {
                    is_readonly: specific_const,
                    elem: specific_elem,
                }) if general_const == specific_const
                    && self.pattern_subsumes_inner(*general_elem, *specific_elem, substitutions)
            ),
            Some(TyKind::Slice {
                is_readonly: general_const,
                elem: general_elem,
            }) => matches!(
                self.interner.get(specific),
                Some(TyKind::Slice {
                    is_readonly: specific_const,
                    elem: specific_elem,
                }) if general_const == specific_const
                    && self.pattern_subsumes_inner(*general_elem, *specific_elem, substitutions)
            ),
            Some(TyKind::SlicePointee { elem: general_elem }) => matches!(
                self.interner.get(specific),
                Some(TyKind::SlicePointee {
                    elem: specific_elem,
                }) if self.pattern_subsumes_inner(*general_elem, *specific_elem, substitutions)
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
            Some(TyKind::Optional { elem: general_elem }) => match self.interner.get(specific) {
                Some(TyKind::Optional {
                    elem: specific_elem,
                }) => self.pattern_subsumes_inner(*general_elem, *specific_elem, substitutions),
                _ => false,
            },
            Some(TyKind::ErrorUnion {
                error: general_error,
                value: general_value,
            }) => match self.interner.get(specific) {
                Some(TyKind::ErrorUnion {
                    error: specific_error,
                    value: specific_value,
                }) => {
                    self.pattern_subsumes_inner(*general_error, *specific_error, substitutions)
                        && self.pattern_subsumes_inner(
                            *general_value,
                            *specific_value,
                            substitutions,
                        )
                }
                _ => false,
            },
            Some(TyKind::Nominal {
                def_id: general_def,
                args: general_args,
                const_args: general_const_args,
            }) => match self.interner.get(specific) {
                Some(TyKind::Nominal {
                    def_id: specific_def,
                    args: specific_args,
                    const_args: specific_const_args,
                }) if general_def == specific_def
                    && general_const_args == specific_const_args
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
                is_readonly: general_const,
                trait_id: general_trait,
                trait_args: general_args,
                trait_const_args: general_const_args,
                associated_type_bindings: general_bindings,
            }) => match self.interner.get(specific) {
                Some(TyKind::TraitObject {
                    is_readonly: specific_const,
                    trait_id: specific_trait,
                    trait_args: specific_args,
                    trait_const_args: specific_const_args,
                    associated_type_bindings: specific_bindings,
                }) if general_const == specific_const
                    && general_trait == specific_trait
                    && general_args.len() == specific_args.len()
                    && general_const_args == specific_const_args
                    && general_bindings.len() == specific_bindings.len() =>
                {
                    general_args
                        .iter()
                        .zip(specific_args)
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
            }) => match self.interner.get(specific) {
                Some(TyKind::TraitObjectPointee {
                    trait_id: specific_trait,
                    trait_args: specific_args,
                    trait_const_args: specific_const_args,
                    associated_type_bindings: specific_bindings,
                }) if general_trait == specific_trait
                    && general_args.len() == specific_args.len()
                    && general_const_args == specific_const_args
                    && general_bindings.len() == specific_bindings.len() =>
                {
                    general_args
                        .iter()
                        .zip(specific_args)
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
                name: general_name,
                ..
            }) => match self.interner.get(specific) {
                Some(TyKind::Projection {
                    self_ty: specific_self,
                    trait_id: specific_trait,
                    trait_args: specific_args,
                    name: specific_name,
                    ..
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
            Some(TyKind::ComptimeOnly | TyKind::Error) => false,
            None => panic!(
                "Nia ICE: method pattern type {:?} is missing from interner {:?}",
                general,
                self.interner.interner_id()
            ),
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
        let mut substitutions = context.target_substitutions.clone();
        let method_arg_count = context.lowered_method_args.len();
        if context.method_args.is_some() && signature.generics.len() != method_arg_count {
            self.diagnostics.push(Diagnostic::user_error_at(codes::TYPE_CHECK,
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
            let return_type = self.substitute_generics_and_consts(
                signature.return_type,
                &substitutions,
                context.target_const_substitutions,
            );
            self.infer_generics_from_type(return_type, expected, &mut substitutions, context.span);
        }
        Some(substitutions)
    }

    fn extension_method_where_predicates_can_hold(
        &mut self,
        method: &nia_defs::VisibleExtensionMethod,
        substitutions: &HashMap<String, InternedTyId>,
    ) -> bool {
        self.where_predicates_can_hold(&method.where_predicates, substitutions)
    }

    pub(in crate::calls) fn extension_target_instance_args(
        &mut self,
        method_id: GlobalDefId,
        substitutions: &HashMap<String, InternedTyId>,
    ) -> Vec<InternedTyId> {
        let Some(impl_generics) = self.extension_impl_generics_for_method(method_id) else {
            return Vec::new();
        };
        impl_generics
            .iter()
            .filter_map(|generic| substitutions.get(generic).copied())
            .collect()
    }

    fn extension_impl_generics_for_method(&self, method_id: GlobalDefId) -> Option<&[String]> {
        self.extension_method_lookup_for_id(method_id)
            .map(|method| method.effective_generics.as_slice())
    }

    pub(crate) fn match_type_pattern(
        &self,
        pattern: InternedTyId,
        actual: InternedTyId,
        substitutions: &mut HashMap<String, InternedTyId>,
    ) -> bool {
        let mut const_substitutions = HashMap::new();
        self.match_type_pattern_with_consts(
            pattern,
            actual,
            substitutions,
            &mut const_substitutions,
        )
    }

    pub(crate) fn match_type_pattern_with_consts(
        &self,
        pattern: InternedTyId,
        actual: InternedTyId,
        substitutions: &mut HashMap<String, InternedTyId>,
        const_substitutions: &mut HashMap<String, nia_ty::ConstGenericArg>,
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
                is_readonly: pattern_const,
                elem: pattern_elem,
            }) => matches!(
                self.interner.get(actual),
                Some(TyKind::Pointer {
                    is_readonly,
                    elem
                }) if is_readonly == pattern_const
                    && self.match_type_pattern_with_consts(
                        *pattern_elem,
                        *elem,
                        substitutions,
                        const_substitutions
                    )
            ),
            Some(TyKind::VolatilePointer {
                is_readonly: pattern_const,
                elem: pattern_elem,
            }) => matches!(
                self.interner.get(actual),
                Some(TyKind::VolatilePointer {
                    is_readonly,
                    elem
                }) if is_readonly == pattern_const
                    && self.match_type_pattern_with_consts(
                        *pattern_elem,
                        *elem,
                        substitutions,
                        const_substitutions
                    )
            ),
            Some(TyKind::Slice {
                is_readonly: pattern_const,
                elem: pattern_elem,
            }) => matches!(
                self.interner.get(actual),
                Some(TyKind::Slice {
                    is_readonly,
                    elem
                }) if is_readonly == pattern_const
                    && self.match_type_pattern_with_consts(
                        *pattern_elem,
                        *elem,
                        substitutions,
                        const_substitutions
                    )
            ),
            Some(TyKind::SlicePointee { elem: pattern_elem }) => matches!(
                self.interner.get(actual),
                Some(TyKind::SlicePointee { elem })
                    if self.match_type_pattern_with_consts(
                        *pattern_elem,
                        *elem,
                        substitutions,
                        const_substitutions
                    )
            ),
            Some(TyKind::Array {
                len: pattern_len,
                elem: pattern_elem,
            }) => match self.interner.get(actual) {
                Some(TyKind::Array { len, elem })
                    if self.match_array_len_pattern(pattern_len, len, const_substitutions) =>
                {
                    self.match_type_pattern_with_consts(
                        *pattern_elem,
                        *elem,
                        substitutions,
                        const_substitutions,
                    )
                }
                _ => false,
            },
            Some(TyKind::Range {
                kind: pattern_kind,
                bound: pattern_bound,
            }) => match self.interner.get(actual) {
                Some(TyKind::Range { kind, bound }) if pattern_kind == kind => {
                    match (pattern_bound, bound) {
                        (Some(pattern_bound), Some(bound)) => self.match_type_pattern_with_consts(
                            *pattern_bound,
                            *bound,
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
            }) => match self.interner.get(actual) {
                Some(TyKind::FunctionPointer {
                    params,
                    return_type,
                    is_variadic,
                }) if pattern_variadic == is_variadic && pattern_params.len() == params.len() => {
                    pattern_params.iter().zip(params).all(|(pattern, actual)| {
                        self.match_type_pattern_with_consts(
                            *pattern,
                            *actual,
                            substitutions,
                            const_substitutions,
                        )
                    }) && self.match_type_pattern_with_consts(
                        *pattern_return,
                        *return_type,
                        substitutions,
                        const_substitutions,
                    )
                }
                _ => false,
            },
            Some(TyKind::Optional { elem: pattern_elem }) => match self.interner.get(actual) {
                Some(TyKind::Optional { elem }) => self.match_type_pattern_with_consts(
                    *pattern_elem,
                    *elem,
                    substitutions,
                    const_substitutions,
                ),
                _ => false,
            },
            Some(TyKind::ErrorUnion {
                error: pattern_error,
                value: pattern_value,
            }) => match self.interner.get(actual) {
                Some(TyKind::ErrorUnion { error, value }) => {
                    self.match_type_pattern_with_consts(
                        *pattern_error,
                        *error,
                        substitutions,
                        const_substitutions,
                    ) && self.match_type_pattern_with_consts(
                        *pattern_value,
                        *value,
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
            }) => match self.interner.get(actual) {
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
                            *actual,
                            substitutions,
                            const_substitutions,
                        )
                    }) && pattern_const_args
                        .iter()
                        .zip(const_args)
                        .all(|(pattern, actual)| {
                            self.match_const_generic_arg_pattern(
                                pattern,
                                actual,
                                const_substitutions,
                            )
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
            Some(TyKind::TraitObject {
                is_readonly: pattern_const,
                trait_id: pattern_trait,
                trait_args: pattern_args,
                trait_const_args: pattern_const_args,
                associated_type_bindings: pattern_bindings,
            }) => match self.interner.get(actual) {
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
                                *actual,
                                substitutions,
                                const_substitutions,
                            )
                        })
                        && pattern_const_args.iter().zip(trait_const_args).all(
                            |(pattern, actual)| {
                                self.match_const_generic_arg_pattern(
                                    pattern,
                                    actual,
                                    const_substitutions,
                                )
                            },
                        )
                        && pattern_bindings.iter().all(|pattern_binding| {
                            associated_type_bindings
                                .iter()
                                .find(|actual_binding| {
                                    pattern_binding.name == actual_binding.name
                                        && pattern_binding.trait_id == actual_binding.trait_id
                                        && pattern_binding.trait_args.len()
                                            == actual_binding.trait_args.len()
                                        && pattern_binding.trait_const_args.len()
                                            == actual_binding.trait_const_args.len()
                                        && pattern_binding
                                            .trait_args
                                            .iter()
                                            .zip(&actual_binding.trait_args)
                                            .all(|(pattern, actual)| {
                                                self.match_type_pattern_with_consts(
                                                    *pattern,
                                                    *actual,
                                                    substitutions,
                                                    const_substitutions,
                                                )
                                            })
                                        && pattern_binding
                                            .trait_const_args
                                            .iter()
                                            .zip(&actual_binding.trait_const_args)
                                            .all(|(pattern, actual)| {
                                                self.match_const_generic_arg_pattern(
                                                    pattern,
                                                    actual,
                                                    const_substitutions,
                                                )
                                            })
                                })
                                .is_some_and(|actual_binding| {
                                    self.match_type_pattern_with_consts(
                                        pattern_binding.ty,
                                        actual_binding.ty,
                                        substitutions,
                                        const_substitutions,
                                    )
                                })
                        })
                }
                _ => false,
            },
            Some(TyKind::TraitObjectPointee {
                trait_id: pattern_trait,
                trait_args: pattern_args,
                trait_const_args: pattern_const_args,
                associated_type_bindings: pattern_bindings,
            }) => match self.interner.get(actual) {
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
                                *actual,
                                substitutions,
                                const_substitutions,
                            )
                        })
                        && pattern_const_args.iter().zip(trait_const_args).all(
                            |(pattern, actual)| {
                                self.match_const_generic_arg_pattern(
                                    pattern,
                                    actual,
                                    const_substitutions,
                                )
                            },
                        )
                        && pattern_bindings.iter().all(|pattern_binding| {
                            associated_type_bindings
                                .iter()
                                .find(|actual_binding| {
                                    pattern_binding.name == actual_binding.name
                                        && pattern_binding.trait_id == actual_binding.trait_id
                                        && pattern_binding.trait_args.len()
                                            == actual_binding.trait_args.len()
                                        && pattern_binding.trait_const_args.len()
                                            == actual_binding.trait_const_args.len()
                                        && pattern_binding
                                            .trait_args
                                            .iter()
                                            .zip(&actual_binding.trait_args)
                                            .all(|(pattern, actual)| {
                                                self.match_type_pattern_with_consts(
                                                    *pattern,
                                                    *actual,
                                                    substitutions,
                                                    const_substitutions,
                                                )
                                            })
                                        && pattern_binding
                                            .trait_const_args
                                            .iter()
                                            .zip(&actual_binding.trait_const_args)
                                            .all(|(pattern, actual)| {
                                                self.match_const_generic_arg_pattern(
                                                    pattern,
                                                    actual,
                                                    const_substitutions,
                                                )
                                            })
                                })
                                .is_some_and(|actual_binding| {
                                    self.match_type_pattern_with_consts(
                                        pattern_binding.ty,
                                        actual_binding.ty,
                                        substitutions,
                                        const_substitutions,
                                    )
                                })
                        })
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
            }) => match self.interner.get(actual) {
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
                        *pattern_self,
                        *self_ty,
                        substitutions,
                        const_substitutions,
                    ) && pattern_args
                        .iter()
                        .zip(trait_args)
                        .all(|(pattern, actual)| {
                            self.match_type_pattern_with_consts(
                                *pattern,
                                *actual,
                                substitutions,
                                const_substitutions,
                            )
                        })
                        && pattern_const_args.iter().zip(trait_const_args).all(
                            |(pattern, actual)| {
                                self.match_const_generic_arg_pattern(
                                    pattern,
                                    actual,
                                    const_substitutions,
                                )
                            },
                        )
                }
                _ => false,
            },
            Some(TyKind::Primitive(_) | TyKind::Vector { .. })
            | Some(TyKind::ComptimeOnly | TyKind::Error)
            | None => self.types_match(pattern, actual),
        }
    }

    fn array_lens_match(&self, expected: &ArrayLenTy, actual: &ArrayLenTy) -> bool {
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
        &self,
        pattern: &ArrayLenTy,
        actual: &ArrayLenTy,
        const_substitutions: &mut HashMap<String, nia_ty::ConstGenericArg>,
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
        Self::record_const_pattern_substitution(name, arg, const_substitutions)
    }

    fn match_const_generic_arg_pattern(
        &self,
        pattern: &nia_ty::ConstGenericArg,
        actual: &nia_ty::ConstGenericArg,
        const_substitutions: &mut HashMap<String, nia_ty::ConstGenericArg>,
    ) -> bool {
        if pattern == actual {
            return true;
        }
        let nia_ty::ConstGenericValue::GenericParam(name) = &pattern.value else {
            return false;
        };
        Self::record_const_pattern_substitution(name, actual.clone(), const_substitutions)
    }

    fn record_const_pattern_substitution(
        name: &str,
        arg: nia_ty::ConstGenericArg,
        const_substitutions: &mut HashMap<String, nia_ty::ConstGenericArg>,
    ) -> bool {
        if let Some(existing) = const_substitutions.get(name) {
            existing == &arg
        } else {
            const_substitutions.insert(name.to_string(), arg);
            true
        }
    }

    fn method_const_generic_value_from_array_len(
        &self,
        len: &ArrayLenTy,
    ) -> Option<nia_ty::ConstGenericValue> {
        match len {
            ArrayLenTy::ConstValue(value) => Some(nia_ty::ConstGenericValue::Int(
                nia_ty::IntConst::unsigned((*value).into()),
            )),
            ArrayLenTy::ConstExpr(id) => {
                self.comptime.array_lengths.get(id).copied().map(|value| {
                    nia_ty::ConstGenericValue::Int(nia_ty::IntConst::unsigned(value.into()))
                })
            }
            ArrayLenTy::GenericParam(name) => {
                Some(nia_ty::ConstGenericValue::GenericParam(name.clone()))
            }
            ArrayLenTy::Infer | ArrayLenTy::Builtin { .. } => None,
        }
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
                    let expected = if self.type_contains_generic_param(expected) {
                        None
                    } else {
                        Some(expected)
                    };
                    self.check_expr_with_expected(arg, expected)
                } else {
                    self.check_expr(arg)
                }
            })
            .collect::<Vec<_>>();
        for (param, (arg, actual)) in params.iter().zip(args.iter().zip(actuals.iter())) {
            self.infer_generics_from_type(*param, *actual, substitutions, arg.span);
        }
    }

    pub(in crate::calls::methods) fn infer_method_generics_from_where_predicates(
        &mut self,
        signature: &FunctionSignature,
        extension_where_predicates: &[nia_defs::WherePredicateSignature],
        substitutions: &mut HashMap<String, InternedTyId>,
    ) {
        let mut changed = true;
        while changed {
            changed = false;
            for predicate in signature
                .where_predicates
                .iter()
                .chain(extension_where_predicates)
            {
                let candidates = self.infer_where_predicate_candidates(predicate, substitutions);
                let Some(candidate) = self.single_where_candidate(&candidates) else {
                    continue;
                };
                for (generic, ty) in candidate {
                    if !substitutions.contains_key(generic) {
                        substitutions.insert(generic.clone(), *ty);
                        changed = true;
                    }
                }
            }
        }
    }

    pub(in crate::calls) fn single_where_candidate<'b>(
        &mut self,
        candidates: &'b [HashMap<String, InternedTyId>],
    ) -> Option<&'b HashMap<String, InternedTyId>> {
        let first = candidates.first()?;
        if candidates.iter().skip(1).any(|candidate| {
            candidate.len() != first.len()
                || candidate.iter().any(|(name, ty)| {
                    first
                        .get(name)
                        .is_none_or(|first_ty| !self.types_match(*first_ty, *ty))
                })
        }) {
            return None;
        }
        Some(first)
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
                self.diagnostics.push(Diagnostic::user_error_at(
                    codes::TYPE_CHECK,
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
                self.diagnostics.push(Diagnostic::user_error_at(
                    codes::TYPE_CHECK,
                    receiver.span,
                    "receiver cannot be matched through read-only `&T`",
                ));
            } else if !base.as_ref().is_some_and(|base| base.from_pointer) {
                self.check_reference_target(receiver, "receiver", false);
            }
        }
    }
}
