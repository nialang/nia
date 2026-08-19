// SPDX-License-Identifier: GPL-3.0-or-later
use super::*;
use nia_symbol::ToSymbolId;

struct DynamicTraitMethodSearch<'a> {
    candidates: &'a mut Vec<DynamicTraitMethodCandidate>,
    object_ty: InternedTyId,
    associated_type_bindings: Vec<nia_ty::AssociatedTypeBindingTy>,
    name: &'a SymbolId,
    // Vtable slots are assigned by walking the object trait and its
    // supertraits in declaration order. This counter must be shared across the
    // recursive walk so codegen indexes the same slot order that type checking
    // recorded.
    next_slot: &'a mut usize,
    visiting: &'a mut Vec<DynamicTraitInstanceKey>,
}

#[derive(Clone, PartialEq, Eq)]
struct DynamicTraitInstanceKey {
    trait_id: TraitId,
    trait_args: Vec<InternedTyId>,
    trait_const_args: Vec<ConstGenericArg>,
}

struct TraitMethodCandidateSource<'a> {
    trait_id: GlobalDefId,
    trait_args: Vec<InternedTyId>,
    trait_const_args: Vec<ConstGenericArg>,
    self_ty: InternedTyId,
    name: &'a SymbolId,
    signature: &'a nia_item_signatures::TraitSignature,
    is_assumed: bool,
}

impl<'a> BodyChecker<'a> {
    fn record_provider_demand(&self, demand: crate::ProviderDemand) {
        self.provider_demands.borrow_mut().insert(demand.clone());
        if let Some(function) = self.current_def_id {
            self.provider_demands_by_function
                .borrow_mut()
                .entry(function)
                .or_default()
                .insert(demand);
        }
    }

    pub(in crate::calls) fn record_method_provider_demand(
        &mut self,
        receiver_ty: InternedTyId,
        method_name: SymbolId,
    ) {
        if self.provider_demand_target_is_error(receiver_ty) {
            return;
        }
        let target_type_name = self.provider_target_type_name(receiver_ty);
        self.record_provider_demand(crate::ProviderDemand {
            source_path: self.source_path.clone(),
            request: crate::ProviderRequest::Method {
                target_type_name,
                method_name,
            },
        });
    }

    pub(crate) fn record_trait_provider_demand(&self, self_ty: InternedTyId, trait_id: TraitId) {
        if self.provider_demand_target_is_error(self_ty) {
            return;
        }
        let target_type_name = self.provider_target_type_name(self_ty);
        let trait_name = match trait_id {
            TraitId::Source(def_id) => {
                let Some(name) = self.definition_name(def_id) else {
                    return;
                };
                name
            }
            TraitId::Builtin(builtin) => builtin.symbol_id(),
        };
        self.record_provider_demand(crate::ProviderDemand {
            source_path: self.source_path.clone(),
            request: crate::ProviderRequest::TraitImpl {
                target_type_name,
                trait_name,
            },
        });
    }

    fn provider_target_type_name(&self, ty: InternedTyId) -> Option<SymbolId> {
        match self.interner.get(ty) {
            Some(TyKind::Primitive(primitive)) => Some(primitive.symbol_id()),
            Some(TyKind::Nominal { def_id, .. }) => self.definition_name(*def_id),
            Some(TyKind::Pointer { elem, .. }) | Some(TyKind::VolatilePointer { elem, .. }) => {
                self.provider_target_type_name(*elem)
            }
            _ => None,
        }
    }

    fn provider_demand_target_is_error(&self, ty: InternedTyId) -> bool {
        matches!(
            self.interner.get(ty),
            Some(TyKind::Error | TyKind::ConstOnly)
        )
    }

    pub(in crate::calls) fn record_semantic_provider_module(&self, module_id: nia_ids::ModuleId) {
        if module_id == self.defs.module_id {
            return;
        }
        let Some(module_path) = self
            .program
            .module_source_path
            .and_then(|module_source_path| module_source_path(module_id))
        else {
            return;
        };
        self.record_provider_demand(crate::ProviderDemand {
            source_path: self.source_path.clone(),
            request: crate::ProviderRequest::ModuleSemantic { module_path },
        });
    }

    pub(crate) fn definition_name(&self, def_id: GlobalDefId) -> Option<SymbolId> {
        if def_id.module_id == self.defs.module_id {
            return self.defs.defs.get(def_id.def_id).map(|def| def.name);
        }
        self.program
            .defs
            .and_then(|defs| defs(def_id.module_id))
            .and_then(|defs| defs.defs.get(def_id.def_id).map(|def| def.name))
    }

    fn ensure_callable_extension_methods_named(&mut self, name: &SymbolId) {
        if self.callable_extension_methods_by_name.contains_key(name) {
            return;
        }
        let mut methods = self
            .with_visible_extensions(|extensions| extensions.all_methods_named(name))
            .into_iter()
            .filter(|(_, method)| !method.is_trait_witness)
            .map(|(target_ty, method)| crate::CallableExtensionMethod { target_ty, method })
            .collect::<Vec<_>>();
        let program_methods = self
            .program
            .extension_methods_named
            .map(|methods_named| methods_named(name))
            .unwrap_or_else(|| {
                self.program_extension_methods
                    .methods_named(name)
                    .cloned()
                    .collect()
            })
            .into_iter()
            .filter(|method| method.trait_id.is_none() && &method.name == name)
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
                    let Some(lookup) = self.program_extension_method_lookup(&method) else {
                        continue;
                    };
                    self.extension_method_lookup_cache
                        .insert(method.def_id, lookup.clone());
                    lookup
                };
            methods.push(crate::CallableExtensionMethod {
                target_ty: lookup.target_ty,
                method: VisibleExtensionMethod {
                    name: method.name,
                    def_id: method.def_id,
                    impl_id: method.impl_id,
                    effective_generics: lookup.effective_generics.clone(),
                    effective_const_generics: lookup.effective_const_generics.clone(),
                    trait_id: method.trait_id,
                    trait_args: Vec::new(),
                    trait_const_args: Vec::new(),
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
            .insert(*name, indexed);
    }

    pub(in crate::calls) fn method_candidates_for_target(
        &mut self,
        target_ty: InternedTyId,
        name: &SymbolId,
    ) -> Vec<MethodCandidate> {
        self.profile_stage("body_check.profile.method.callable_named", |this| {
            this.ensure_callable_extension_methods_named(name)
        });
        let methods_len = self
            .callable_extension_methods_by_name
            .get(name)
            .map(|methods| methods.methods.len())
            .unwrap_or_default();
        let mut candidates = Vec::new();
        for index in 0..methods_len {
            let Some((candidate_ty, method)) = self
                .callable_extension_methods_by_name
                .get(name)
                .and_then(|methods| methods.methods.get(index))
                .map(|method| (method.target_ty, method.method.clone()))
            else {
                continue;
            };
            let mut target_substitutions = SymbolMap::default();
            let mut target_const_substitutions = SymbolMap::default();
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
                    self_ty: target_ty,
                    method,
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
        name: &SymbolId,
    ) -> Vec<MethodCandidate> {
        self.profile_stage("body_check.profile.method.callable_named", |this| {
            this.ensure_callable_extension_methods_named(name)
        });
        let mut receiver_ty = self.normalize_aliases_in_type(receiver_ty);
        loop {
            let receiver_base = self.receiver_base_type(receiver_ty);
            let candidate_indexes = self
                .callable_extension_methods_by_name
                .get(name)
                .map(|methods| {
                    self.callable_method_indexes_for_receiver_base(
                        methods,
                        receiver_base.as_ref().map(|base| base.def_id),
                    )
                })
                .unwrap_or_default();
            let mut candidates = Vec::new();
            for index in candidate_indexes {
                let Some((target_ty, method)) = self
                    .callable_extension_methods_by_name
                    .get(name)
                    .and_then(|methods| methods.methods.get(index))
                    .map(|method| (method.target_ty, method.method.clone()))
                else {
                    continue;
                };
                let target_ty = self.normalize_aliases_in_type(target_ty);
                if self.extension_receiver_base_mismatch(target_ty, receiver_base.as_ref()) {
                    continue;
                }
                let mut target_substitutions = SymbolMap::default();
                let mut target_const_substitutions = SymbolMap::default();
                let matches_receiver =
                    self.profile_stage("body_check.profile.method.match_receiver", |this| {
                        this.match_extension_receiver_target(
                            target_ty,
                            method.def_id,
                            receiver_ty,
                            &mut target_substitutions,
                            &mut target_const_substitutions,
                        )
                    });
                if matches_receiver {
                    if !self.extension_method_where_predicates_can_hold(
                        &method,
                        &target_substitutions,
                        &target_const_substitutions,
                    ) {
                        continue;
                    }
                    candidates.push(MethodCandidate {
                        target_ty,
                        self_ty: receiver_ty,
                        method,
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
        substitutions: &mut SymbolMap<InternedTyId>,
        const_substitutions: &mut SymbolMap<nia_ty::ConstGenericArg>,
    ) -> bool {
        let candidate_target_ty = self.receiver_candidate_target_ty(target_ty, method_id);
        if self.try_match_type_pattern_with_consts(
            candidate_target_ty,
            receiver_ty,
            substitutions,
            const_substitutions,
        ) {
            return true;
        }
        if self.try_match_type_pattern_with_consts(
            target_ty,
            receiver_ty,
            substitutions,
            const_substitutions,
        ) {
            return true;
        }
        if self.method_receiver_kind(method_id) == Some(ReceiverKind::RefReadOnly)
            && let Some(readonly_receiver_ty) = self.mutable_slice_as_readonly(receiver_ty)
            && self.try_match_type_pattern_with_consts(
                candidate_target_ty,
                readonly_receiver_ty,
                substitutions,
                const_substitutions,
            )
        {
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

    fn mutable_slice_as_readonly(&mut self, receiver_ty: InternedTyId) -> Option<InternedTyId> {
        let receiver_ty = self.normalization.normalize(receiver_ty);
        let Some(TyKind::Slice {
            is_readonly: false,
            elem,
        }) = self.interner.get(receiver_ty).cloned()
        else {
            return None;
        };
        Some(self.interner.intern(TyKind::Slice {
            is_readonly: true,
            elem,
        }))
    }

    fn trait_object_extension_target_matches_receiver(
        &mut self,
        target_ty: InternedTyId,
        receiver_ty: InternedTyId,
        substitutions: &mut SymbolMap<InternedTyId>,
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
        true
    }

    pub(in crate::calls::methods) fn trait_method_candidates_for_receiver(
        &mut self,
        receiver_ty: InternedTyId,
        name: &SymbolId,
    ) -> Vec<TraitMethodCandidate> {
        let mut candidates = self.assumed_trait_method_candidates_for_receiver(receiver_ty, name);
        if !candidates.is_empty() {
            return candidates;
        }
        let Some(self_ty) = self.trait_receiver_self_ty(receiver_ty) else {
            return Vec::new();
        };
        self.push_visible_impl_trait_method_candidates(&mut candidates, self_ty, name);
        candidates
    }

    pub(in crate::calls::methods) fn assumed_trait_method_candidates_for_receiver(
        &mut self,
        receiver_ty: InternedTyId,
        name: &SymbolId,
    ) -> Vec<TraitMethodCandidate> {
        let Some(self_ty) = self.trait_receiver_self_ty(receiver_ty) else {
            return Vec::new();
        };
        let mut candidates = Vec::new();
        for goal in self.current_trait_goals() {
            if !self.types_match(goal.self_ty, self_ty) {
                continue;
            }
            let TraitId::Source(trait_id) = goal.trait_id else {
                continue;
            };
            let Some(trait_signature) = self.resolved_trait_signature(trait_id) else {
                continue;
            };
            let trait_args = goal.trait_args;
            let trait_const_args = goal.trait_const_args;
            self.push_trait_method_candidates(
                &mut candidates,
                TraitMethodCandidateSource {
                    trait_id,
                    trait_args,
                    trait_const_args,
                    self_ty,
                    name,
                    signature: &trait_signature,
                    is_assumed: true,
                },
            );
        }
        candidates
    }

    pub(in crate::calls::methods) fn trait_method_candidates_for_target(
        &mut self,
        target_ty: InternedTyId,
        name: &SymbolId,
    ) -> Vec<TraitMethodCandidate> {
        let self_ty = target_ty;
        let mut candidates = Vec::new();
        for goal in self.current_trait_goals() {
            if !self.types_match(goal.self_ty, self_ty) {
                continue;
            }
            let TraitId::Source(trait_id) = goal.trait_id else {
                continue;
            };
            let Some(trait_signature) = self.resolved_trait_signature(trait_id) else {
                continue;
            };
            let trait_args = goal.trait_args;
            let trait_const_args = goal.trait_const_args;
            self.push_trait_method_candidates(
                &mut candidates,
                TraitMethodCandidateSource {
                    trait_id,
                    trait_args,
                    trait_const_args,
                    self_ty,
                    name,
                    signature: &trait_signature,
                    is_assumed: true,
                },
            );
        }
        if !candidates.is_empty() {
            return candidates;
        }
        self.push_visible_impl_trait_method_candidates(&mut candidates, self_ty, name);
        candidates
    }

    fn push_visible_impl_trait_method_candidates(
        &mut self,
        candidates: &mut Vec<TraitMethodCandidate>,
        self_ty: InternedTyId,
        name: &SymbolId,
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
                    TraitMethodCandidateSource {
                        trait_id,
                        trait_args,
                        trait_const_args,
                        self_ty,
                        name,
                        signature: &trait_signature,
                        is_assumed: false,
                    },
                );
            }
        }
    }

    fn trait_ids_with_method_named(&mut self, name: &SymbolId) -> Vec<GlobalDefId> {
        if let Some(trait_ids) = self.traits_by_method_name.get(name) {
            return trait_ids.clone();
        }
        let trait_ids = self
            .program_signature_scope
            .trait_ids_with_method_named(name);
        self.traits_by_method_name.insert(*name, trait_ids.clone());
        trait_ids
    }

    pub(in crate::calls::methods) fn dynamic_trait_method_candidates_for_receiver(
        &mut self,
        receiver_ty: InternedTyId,
        name: &SymbolId,
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
        let visit_key = DynamicTraitInstanceKey {
            trait_id,
            trait_args: trait_args.clone(),
            trait_const_args: trait_const_args.clone(),
        };
        if search.visiting.contains(&visit_key) {
            return;
        }
        search.visiting.push(visit_key);
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
            if &method.name != search.name {
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
        source: TraitMethodCandidateSource<'_>,
    ) {
        let TraitMethodCandidateSource {
            trait_id,
            trait_args,
            trait_const_args,
            self_ty,
            name,
            signature: trait_signature,
            is_assumed,
        } = source;
        for method in &trait_signature.methods {
            if &method.name == name {
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
                        && self.const_generic_arg_slices_match(
                            &candidate.trait_const_args,
                            &trait_const_args,
                        )
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
                TraitMethodCandidateSource {
                    trait_id: supertrait_id,
                    trait_args: supertrait_args,
                    trait_const_args: supertrait_const_args,
                    self_ty,
                    name,
                    signature: &supertrait_signature,
                    is_assumed,
                },
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
        name: &SymbolId,
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
            let name = self.symbol_name(*name);
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
            .map(|param| {
                self.substitute_generics_and_consts_with_self(
                    param.ty,
                    &candidate.target_substitutions,
                    &candidate.target_const_substitutions,
                    candidate.self_ty,
                )
            })
            .collect()
    }

    pub(in crate::calls::methods) fn viable_method_candidates(
        &mut self,
        call: &MethodCall<'_>,
        candidates: &[MethodCandidate],
    ) -> Vec<MethodCandidate> {
        let viable = candidates
            .iter()
            .filter(|candidate| self.method_candidate_matches_call(call, candidate))
            .cloned()
            .collect::<Vec<_>>();
        // A lone candidate still owns the detailed type/arity diagnostic.
        // Multiple rejected overloads have no honest single expected type.
        if viable.is_empty() && candidates.len() == 1 {
            candidates.to_vec()
        } else {
            viable
        }
    }

    fn method_candidate_matches_call(
        &self,
        call: &MethodCall<'_>,
        candidate: &MethodCandidate,
    ) -> bool {
        let mut probe = self.clone_for_type_compare();
        probe.local_types = self.local_types.clone();
        probe.global_types = self.global_types.clone();
        probe.const_types = self.const_types.clone();
        // Rejected overloads are speculative and must not schedule providers
        // or publish semantic facts through the shared checker state.
        probe.provider_demands =
            std::rc::Rc::new(std::cell::RefCell::new(std::collections::HashSet::new()));
        probe.provider_demands_by_function =
            std::rc::Rc::new(std::cell::RefCell::new(std::collections::HashMap::new()));
        probe.method_candidate_matches_call_inner(call, candidate) && probe.diagnostics.is_empty()
    }

    fn method_candidate_matches_call_inner(
        &mut self,
        call: &MethodCall<'_>,
        candidate: &MethodCandidate,
    ) -> bool {
        let Some(signature) = self
            .resolved_function_signature(candidate.method.def_id)
            .map(|resolved| resolved.signature)
        else {
            return false;
        };
        let Some(method_args) = self.lowered_method_type_args(call.type_args) else {
            return false;
        };
        let Some(mut substitutions) = self.method_generic_substitutions(
            MethodGenericContext {
                span: call.span,
                self_ty: candidate.self_ty,
                target_substitutions: &candidate.target_substitutions,
                target_const_substitutions: &candidate.target_const_substitutions,
                method_args: call.type_args,
                lowered_method_args: &method_args,
                expected: call.expected,
            },
            &signature,
        ) else {
            return false;
        };
        let mut params = self.method_candidate_param_types(candidate, &signature);
        if call.type_args.is_none() {
            self.infer_method_generics_from_args(call.args, &params, &mut substitutions);
            if !self.method_generics_are_complete(call.span, &signature, &substitutions) {
                return false;
            }
            params = signature
                .params
                .iter()
                .skip(1)
                .map(|param| {
                    self.substitute_generics_and_consts_with_self(
                        param.ty,
                        &substitutions,
                        &candidate.target_const_substitutions,
                        candidate.self_ty,
                    )
                })
                .collect();
        }
        self.infer_method_generics_from_where_predicates(
            &signature,
            &candidate.method.where_predicates,
            &mut substitutions,
        );
        self.check_where_predicates_hold(
            &signature.where_predicates,
            &substitutions,
            &candidate.target_const_substitutions,
            call.span,
        );
        self.check_where_predicates_hold(
            &candidate.method.where_predicates,
            &substitutions,
            &candidate.target_const_substitutions,
            call.span,
        );
        self.check_direct_call_args(call.span, call.args, &params, false);
        true
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
    ) -> Option<SymbolMap<InternedTyId>> {
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
            let return_type = self.substitute_generics_and_consts_with_self(
                signature.return_type,
                &substitutions,
                context.target_const_substitutions,
                context.self_ty,
            );
            self.infer_generics_from_type(return_type, expected, &mut substitutions, context.span);
        }
        Some(substitutions)
    }

    fn extension_method_where_predicates_can_hold(
        &mut self,
        method: &nia_defs::VisibleExtensionMethod,
        substitutions: &SymbolMap<InternedTyId>,
        const_substitutions: &SymbolMap<ConstGenericArg>,
    ) -> bool {
        self.where_predicates_can_hold_with_consts(
            &method.where_predicates,
            substitutions,
            const_substitutions,
        )
    }

    pub(in crate::calls) fn extension_target_instance_args(
        &mut self,
        method_id: GlobalDefId,
        substitutions: &SymbolMap<InternedTyId>,
    ) -> Vec<InternedTyId> {
        let Some(impl_generics) = self.extension_impl_generics_for_method(method_id) else {
            return Vec::new();
        };
        impl_generics
            .iter()
            .filter_map(|generic| substitutions.get(generic).copied())
            .collect()
    }

    fn extension_impl_generics_for_method(
        &mut self,
        method_id: GlobalDefId,
    ) -> Option<&[SymbolId]> {
        self.ensure_extension_method_lookup_for_id(method_id)
            .map(|method| method.effective_generics.as_slice())
    }

    pub(in crate::calls::methods) fn infer_method_generics_from_args(
        &mut self,
        args: &[Expr],
        params: &[InternedTyId],
        substitutions: &mut SymbolMap<InternedTyId>,
    ) {
        let actuals = args
            .iter()
            .enumerate()
            .map(|(index, arg)| {
                let param = params.get(index).copied();
                let inferred_from_closure = param.is_some_and(|param| {
                    self.infer_generics_from_closure_signature(param, arg, substitutions, arg.span)
                });
                if let Some(expected) =
                    param.map(|param| self.substitute_generics(param, substitutions))
                {
                    let expected = if self.type_contains_generic_param(expected) {
                        None
                    } else {
                        Some(expected)
                    };
                    match expected {
                        Some(expected) => Some(self.check_expr_with_expected(arg, Some(expected))),
                        None if inferred_from_closure => None,
                        None => Some(self.check_expr(arg)),
                    }
                } else {
                    Some(self.check_expr(arg))
                }
            })
            .collect::<Vec<_>>();
        for (param, (arg, actual)) in params.iter().zip(args.iter().zip(actuals.iter())) {
            if let Some(actual) = actual
                && (self.inferred_closure_signature(arg).is_none()
                    || self.generic_pattern_accepts_type_shape(*param, *actual))
            {
                self.infer_generics_from_type(*param, *actual, substitutions, arg.span);
            }
        }
    }

    pub(in crate::calls::methods) fn infer_method_generics_from_where_predicates(
        &mut self,
        signature: &FunctionSignature,
        extension_where_predicates: &[nia_defs::WherePredicateSignature],
        substitutions: &mut SymbolMap<InternedTyId>,
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
                        substitutions.insert(*generic, *ty);
                        changed = true;
                    }
                }
            }
        }
    }

    pub(in crate::calls) fn single_where_candidate<'b>(
        &mut self,
        candidates: &'b [SymbolMap<InternedTyId>],
    ) -> Option<&'b SymbolMap<InternedTyId>> {
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
        substitutions: &SymbolMap<InternedTyId>,
    ) -> bool {
        let mut complete = true;
        for generic in &signature.generics {
            if !substitutions.contains_key(generic) {
                complete = false;
                let name = self.symbol_name(*generic);
                self.diagnostics.push(Diagnostic::user_error_at(
                    codes::TYPE_CHECK,
                    span,
                    format!("cannot infer generic parameter `{name}`"),
                ));
            }
        }
        complete
    }

    pub(in crate::calls::methods) fn check_receiver_match(
        &mut self,
        receiver: &Expr,
        actual_receiver_ty: InternedTyId,
        receiver_kind: ReceiverKind,
    ) {
        if receiver_kind == ReceiverKind::Ref {
            let base = self.receiver_base_type(actual_receiver_ty);
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
