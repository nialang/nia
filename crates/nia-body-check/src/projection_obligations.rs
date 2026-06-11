// SPDX-License-Identifier: GPL-3.0-or-later
use std::collections::{HashMap, HashSet};

use crate::BodyChecker;
use nia_defs::{DefId, DefKind};
use nia_ids::{GlobalDefId, InternedTyId};
use nia_item_signatures::{
    FunctionSignature, ProgramTraitImplSignature, TraitImplSignature, WherePredicateSignature,
};
use nia_span::Span;
use nia_trait_solve::{AssociatedTypeProjectionEq, TraitGoal, TraitResolution, TraitSolverContext};
use nia_ty::{TraitId, TyKind};

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
struct TraitObligation {
    self_ty: InternedTyId,
    trait_id: TraitId,
    trait_args: Vec<InternedTyId>,
    associated_type_bindings: Vec<TraitObligationAssociatedTypeBinding>,
}

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
struct TraitObligationAssociatedTypeBinding {
    name: String,
    ty: InternedTyId,
}

impl From<TraitObligation> for TraitGoal {
    fn from(obligation: TraitObligation) -> Self {
        Self {
            self_ty: obligation.self_ty,
            trait_id: obligation.trait_id,
            trait_args: obligation.trait_args,
        }
    }
}

impl<'a> BodyChecker<'a> {
    pub(crate) fn current_context_proves_trait_obligation(
        &mut self,
        self_ty: InternedTyId,
        trait_id: TraitId,
        trait_args: Vec<InternedTyId>,
    ) -> bool {
        matches!(
            self.current_context_resolve_trait_obligation(self_ty, trait_id, trait_args),
            TraitResolution::Intrinsic(_) | TraitResolution::User(_) | TraitResolution::Assumed(_)
        )
    }

    pub(crate) fn current_context_resolve_trait_obligation(
        &mut self,
        self_ty: InternedTyId,
        trait_id: TraitId,
        trait_args: Vec<InternedTyId>,
    ) -> TraitResolution {
        let self_ty = self.normalize_aliases_in_type(self_ty);
        let obligations = self.current_trait_obligations();
        let required = TraitObligation {
            self_ty,
            trait_id,
            trait_args,
            associated_type_bindings: Vec::new(),
        };
        self.resolve_trait_obligation(&obligations, &required)
    }

    pub(crate) fn check_function_signature_projection_obligations(
        &mut self,
        def_id: DefId,
        signature: &FunctionSignature,
    ) {
        let obligations = self.function_signature_trait_obligations(def_id, signature);
        for param in &signature.params {
            self.check_type_projection_obligations(param.span, param.ty, &obligations);
        }
        self.check_type_projection_obligations(signature.span, signature.return_type, &obligations);
        for predicate in &signature.where_predicates {
            self.check_type_projection_obligations(predicate.span, predicate.ty, &obligations);
            for bound in &predicate.bounds {
                self.check_type_projection_obligations(
                    predicate.span,
                    bound.trait_ty,
                    &obligations,
                );
                for binding in &bound.associated_type_bindings {
                    self.check_type_projection_obligations(binding.span, binding.ty, &obligations);
                }
            }
        }
    }

    pub(crate) fn current_trait_goals(&mut self) -> Vec<TraitGoal> {
        self.current_trait_obligations()
            .into_iter()
            .map(TraitGoal::from)
            .collect()
    }

    pub(crate) fn current_associated_type_assumptions(
        &mut self,
    ) -> Vec<AssociatedTypeProjectionEq> {
        let obligations = self.current_trait_obligations();
        self.associated_type_assumptions_for_obligations(&obligations)
    }

    fn current_trait_obligations(&mut self) -> Vec<TraitObligation> {
        self.current_def_id
            .and_then(|def_id| (def_id.module_id == self.defs.module_id).then_some(def_id.def_id))
            .map(|def_id| self.def_trait_obligations(def_id))
            .unwrap_or_default()
    }

    fn def_trait_obligations(&mut self, def_id: DefId) -> Vec<TraitObligation> {
        let mut obligations = Vec::new();
        self.push_method_owner_trait_obligations(def_id, &mut obligations);
        if let Some(signature) = self.signatures.functions.get(&def_id).cloned() {
            self.push_where_predicate_obligations(&mut obligations, &signature.where_predicates);
        }
        obligations
    }

    fn function_signature_trait_obligations(
        &mut self,
        def_id: DefId,
        signature: &FunctionSignature,
    ) -> Vec<TraitObligation> {
        let mut obligations = Vec::new();
        self.push_method_owner_trait_obligations(def_id, &mut obligations);
        self.push_where_predicate_obligations(&mut obligations, &signature.where_predicates);
        obligations
    }

    fn push_method_owner_trait_obligations(
        &mut self,
        def_id: DefId,
        obligations: &mut Vec<TraitObligation>,
    ) {
        if let Some(trait_obligation) = self.method_trait_obligation(def_id) {
            self.push_trait_obligation_with_supertraits(obligations, trait_obligation);
        }
        let Some(method) = self.defs.defs.get(def_id) else {
            return;
        };
        match method.kind {
            DefKind::Method => {
                if let Some(impl_signature) = self
                    .trait_impl_signature_for_method(self.global_def_id(def_id))
                    .cloned()
                {
                    let predicates =
                        self.instantiate_impl_where_predicates_for_method(def_id, &impl_signature);
                    self.push_where_predicate_obligations(obligations, &predicates);
                }
                if let Some(owner_ty) = self.method_owner_type(def_id) {
                    self.push_nominal_owner_where_obligations(obligations, owner_ty);
                }
            }
            DefKind::TraitMethod => {
                if let Some(parent) = method.parent
                    && let Some(trait_signature) = self.signatures.traits.get(&parent).cloned()
                {
                    self.push_where_predicate_obligations(
                        obligations,
                        &trait_signature.where_predicates,
                    );
                }
            }
            _ => {}
        }
    }

    fn instantiate_impl_where_predicates_for_method(
        &mut self,
        def_id: DefId,
        impl_signature: &TraitImplSignature,
    ) -> Vec<WherePredicateSignature> {
        let Some(owner_ty) = self.method_owner_type(def_id) else {
            return impl_signature.where_predicates.clone();
        };
        let target_ty =
            self.import_type_from(&self.normalization.interner, impl_signature.target_ty);
        let target_ty = self.normalization.normalize(target_ty);
        let owner_ty = self.normalization.normalize(owner_ty);
        let mut substitutions = HashMap::new();
        self.match_type_pattern(target_ty, owner_ty, &mut substitutions);
        impl_signature
            .where_predicates
            .iter()
            .map(|predicate| self.substitute_where_predicate(predicate, &substitutions))
            .collect()
    }

    fn push_nominal_owner_where_obligations(
        &mut self,
        obligations: &mut Vec<TraitObligation>,
        owner_ty: InternedTyId,
    ) {
        let owner_ty = self.normalization.normalize(owner_ty);
        let Some(TyKind::Nominal { def_id, args }) = self.interner.get(owner_ty).cloned() else {
            return;
        };
        if def_id.module_id != self.defs.module_id {
            return;
        }
        let Some(predicates) = self
            .signatures
            .structs
            .get(&def_id.def_id)
            .map(|signature| {
                (
                    signature.generics.clone(),
                    signature.where_predicates.clone(),
                )
            })
            .or_else(|| {
                self.signatures.unions.get(&def_id.def_id).map(|signature| {
                    (
                        signature.generics.clone(),
                        signature.where_predicates.clone(),
                    )
                })
            })
        else {
            return;
        };
        let substitutions = predicates
            .0
            .iter()
            .cloned()
            .zip(args.iter().copied())
            .collect::<std::collections::HashMap<_, _>>();
        let predicates = predicates
            .1
            .iter()
            .map(|predicate| self.substitute_where_predicate(predicate, &substitutions))
            .collect::<Vec<_>>();
        self.push_where_predicate_obligations(obligations, &predicates);
    }

    pub(crate) fn substitute_where_predicate(
        &mut self,
        predicate: &WherePredicateSignature,
        substitutions: &std::collections::HashMap<String, InternedTyId>,
    ) -> WherePredicateSignature {
        WherePredicateSignature {
            ty: self.substitute_ty(predicate.ty, substitutions),
            bounds: predicate
                .bounds
                .iter()
                .map(|bound| nia_item_signatures::WhereBoundSignature {
                    trait_ty: self.substitute_ty(bound.trait_ty, substitutions),
                    associated_type_bindings: bound
                        .associated_type_bindings
                        .iter()
                        .map(
                            |binding| nia_item_signatures::AssociatedTypeBindingSignature {
                                name: binding.name.clone(),
                                ty: self.substitute_ty(binding.ty, substitutions),
                                span: binding.span,
                            },
                        )
                        .collect(),
                    span: bound.span,
                })
                .collect(),
            span: predicate.span,
        }
    }

    pub(crate) fn check_where_predicates_hold(
        &mut self,
        predicates: &[WherePredicateSignature],
        substitutions: &std::collections::HashMap<String, InternedTyId>,
        span: Span,
    ) {
        let predicates = predicates
            .iter()
            .map(|predicate| self.substitute_where_predicate(predicate, substitutions))
            .collect::<Vec<_>>();
        for predicate in predicates {
            for bound in predicate.bounds {
                let Some((trait_id, trait_args)) = self.trait_id_and_args(bound.trait_ty) else {
                    continue;
                };
                if !self.current_context_proves_trait_obligation(
                    predicate.ty,
                    trait_id,
                    trait_args.clone(),
                ) {
                    self.diagnostics
                        .push(nia_diagnostic::Diagnostic::user_error_at(
                            "E0301",
                            span,
                            format!(
                                "trait bound not satisfied: {}: {}",
                                self.ty_name(predicate.ty),
                                self.trait_ty_name(trait_id, &trait_args)
                            ),
                        ));
                }
            }
        }
    }

    fn substitute_ty(
        &mut self,
        ty: InternedTyId,
        substitutions: &std::collections::HashMap<String, InternedTyId>,
    ) -> InternedTyId {
        match self.interner.get(ty).cloned() {
            Some(TyKind::GenericParam(name)) => substitutions.get(&name).copied().unwrap_or(ty),
            Some(TyKind::Pointer { is_readonly, elem }) => {
                let elem = self.substitute_ty(elem, substitutions);
                self.interner.intern(TyKind::Pointer { is_readonly, elem })
            }
            Some(TyKind::Slice { is_readonly, elem }) => {
                let elem = self.substitute_ty(elem, substitutions);
                self.interner.intern(TyKind::Slice { is_readonly, elem })
            }
            Some(TyKind::SlicePointee { elem }) => {
                let elem = self.substitute_ty(elem, substitutions);
                self.interner.intern(TyKind::SlicePointee { elem })
            }
            Some(TyKind::Array { len, elem }) => {
                let elem = self.substitute_ty(elem, substitutions);
                self.interner.intern(TyKind::Array { len, elem })
            }
            Some(TyKind::Range { kind, bound }) => {
                let bound = bound.map(|bound| self.substitute_ty(bound, substitutions));
                self.interner.intern(TyKind::Range { kind, bound })
            }
            Some(TyKind::FunctionPointer {
                params,
                return_type,
                is_variadic,
            }) => {
                let params = params
                    .into_iter()
                    .map(|param| self.substitute_ty(param, substitutions))
                    .collect();
                let return_type = self.substitute_ty(return_type, substitutions);
                self.interner.intern(TyKind::FunctionPointer {
                    params,
                    return_type,
                    is_variadic,
                })
            }
            Some(TyKind::Optional { elem }) => {
                let elem = self.substitute_ty(elem, substitutions);
                self.interner.intern(TyKind::Optional { elem })
            }
            Some(TyKind::ErrorUnion { error, value }) => {
                let error = self.substitute_ty(error, substitutions);
                let value = self.substitute_ty(value, substitutions);
                self.interner.intern(TyKind::ErrorUnion { error, value })
            }
            Some(TyKind::Nominal { def_id, args }) => {
                let args = args
                    .into_iter()
                    .map(|arg| self.substitute_ty(arg, substitutions))
                    .collect();
                self.interner.intern(TyKind::Nominal { def_id, args })
            }
            Some(TyKind::BuiltinTrait { trait_id, args }) => {
                let args = args
                    .into_iter()
                    .map(|arg| self.substitute_ty(arg, substitutions))
                    .collect();
                self.interner
                    .intern(TyKind::BuiltinTrait { trait_id, args })
            }
            Some(TyKind::TraitObject {
                is_readonly,
                trait_id,
                trait_args,
                associated_type_bindings,
            }) => {
                let trait_args = trait_args
                    .into_iter()
                    .map(|arg| self.substitute_ty(arg, substitutions))
                    .collect();
                let associated_type_bindings = associated_type_bindings
                    .into_iter()
                    .map(|binding| nia_ty::AssociatedTypeBindingTy {
                        trait_id: binding.trait_id,
                        trait_args: binding
                            .trait_args
                            .into_iter()
                            .map(|arg| self.substitute_ty(arg, substitutions))
                            .collect(),
                        name: binding.name,
                        ty: self.substitute_ty(binding.ty, substitutions),
                    })
                    .collect();
                self.interner.intern(TyKind::TraitObject {
                    is_readonly,
                    trait_id,
                    trait_args,
                    associated_type_bindings,
                })
            }
            Some(TyKind::TraitObjectPointee {
                trait_id,
                trait_args,
                associated_type_bindings,
            }) => {
                let trait_args = trait_args
                    .into_iter()
                    .map(|arg| self.substitute_ty(arg, substitutions))
                    .collect();
                let associated_type_bindings = associated_type_bindings
                    .into_iter()
                    .map(|binding| nia_ty::AssociatedTypeBindingTy {
                        trait_id: binding.trait_id,
                        trait_args: binding
                            .trait_args
                            .into_iter()
                            .map(|arg| self.substitute_ty(arg, substitutions))
                            .collect(),
                        name: binding.name,
                        ty: self.substitute_ty(binding.ty, substitutions),
                    })
                    .collect();
                self.interner.intern(TyKind::TraitObjectPointee {
                    trait_id,
                    trait_args,
                    associated_type_bindings,
                })
            }
            Some(TyKind::Projection {
                self_ty,
                trait_id,
                trait_args,
                name,
            }) => {
                let self_ty = self.substitute_ty(self_ty, substitutions);
                let trait_args = trait_args
                    .into_iter()
                    .map(|arg| self.substitute_ty(arg, substitutions))
                    .collect();
                self.interner.intern(TyKind::Projection {
                    self_ty,
                    trait_id,
                    trait_args,
                    name,
                })
            }
            Some(
                TyKind::Error | TyKind::ComptimeOnly | TyKind::Primitive(_) | TyKind::Vector { .. },
            )
            | None => ty,
        }
    }

    fn push_where_predicate_obligations(
        &mut self,
        obligations: &mut Vec<TraitObligation>,
        predicates: &[WherePredicateSignature],
    ) {
        for predicate in predicates {
            for bound in &predicate.bounds {
                self.push_trait_obligation_from_bound(obligations, predicate.ty, bound);
            }
        }
    }

    fn method_trait_obligation(&mut self, def_id: DefId) -> Option<TraitObligation> {
        let method = self.defs.defs.get(def_id)?;
        match method.kind {
            DefKind::TraitMethod => {
                let trait_id = GlobalDefId {
                    module_id: self.defs.module_id,
                    def_id: method.parent?,
                };
                let trait_signature = self.resolved_trait_signature(trait_id)?;
                let trait_args = trait_signature
                    .generics
                    .iter()
                    .map(|generic| self.interner.intern(TyKind::GenericParam(generic.clone())))
                    .collect();
                Some(TraitObligation {
                    self_ty: self
                        .interner
                        .intern(TyKind::GenericParam("Self".to_string())),
                    trait_id: TraitId::Source(trait_id),
                    trait_args,
                    associated_type_bindings: Vec::new(),
                })
            }
            DefKind::Method => {
                if let Some(TyKind::TraitObjectPointee {
                    trait_id,
                    trait_args,
                    associated_type_bindings,
                }) = self
                    .method_owner_trait_object_type(def_id)
                    .and_then(|ty| self.interner.get(self.normalization.normalize(ty)).cloned())
                {
                    return Some(TraitObligation {
                        self_ty: self
                            .interner
                            .intern(TyKind::GenericParam("Self".to_string())),
                        trait_id,
                        trait_args,
                        associated_type_bindings: associated_type_bindings
                            .iter()
                            .map(|binding| TraitObligationAssociatedTypeBinding {
                                name: binding.name.clone(),
                                ty: binding.ty,
                            })
                            .collect(),
                    });
                }
                let method_id = self.global_def_id(def_id);
                let target_ty = self.method_owner_type(def_id)?;
                let impl_signature = self.trait_impl_signature_for_method(method_id)?.clone();
                let (trait_id, trait_args) = self.trait_impl_signature_trait(&impl_signature)?;
                Some(TraitObligation {
                    self_ty: target_ty,
                    trait_id,
                    trait_args,
                    associated_type_bindings: impl_signature
                        .associated_types
                        .iter()
                        .map(|associated_type| TraitObligationAssociatedTypeBinding {
                            name: associated_type.name.clone(),
                            ty: self
                                .import_type_from(&self.normalization.interner, associated_type.ty),
                        })
                        .collect(),
                })
            }
            _ => None,
        }
    }

    fn trait_impl_signature_trait(
        &mut self,
        impl_signature: &TraitImplSignature,
    ) -> Option<(TraitId, Vec<InternedTyId>)> {
        let trait_ty =
            self.import_type_from(&self.normalization.interner, impl_signature.trait_ty?);
        let trait_ty = self.normalization.normalize(trait_ty);
        self.trait_id_and_args(trait_ty)
    }

    pub(crate) fn trait_impl_signature_for_method(
        &self,
        method_id: GlobalDefId,
    ) -> Option<&TraitImplSignature> {
        if method_id.module_id != self.defs.module_id {
            return None;
        }
        self.module.items.iter().find_map(|item| {
            let nia_ast::ItemKind::Extend(extend) = &item.kind else {
                return None;
            };
            let has_method = extend.methods.iter().any(|method| {
                self.defs
                    .def_nodes
                    .get(&method.function.node_key)
                    .is_some_and(|def_id| def_id == method_id.def_id)
            });
            if !has_method {
                return None;
            }
            self.signatures
                .trait_impls
                .iter()
                .find(|signature| signature.span == extend.target.span)
        })
    }

    fn push_trait_obligation_from_bound(
        &mut self,
        obligations: &mut Vec<TraitObligation>,
        self_ty: InternedTyId,
        bound: &nia_item_signatures::WhereBoundSignature,
    ) {
        let bound_ty = self.normalization.normalize(bound.trait_ty);
        let Some((trait_id, args)) = self.trait_id_and_args(bound_ty) else {
            return;
        };
        self.push_trait_obligation_with_supertraits(
            obligations,
            TraitObligation {
                self_ty,
                trait_id,
                trait_args: args,
                associated_type_bindings: bound
                    .associated_type_bindings
                    .iter()
                    .map(|binding| TraitObligationAssociatedTypeBinding {
                        name: binding.name.clone(),
                        ty: binding.ty,
                    })
                    .collect(),
            },
        );
    }

    pub(crate) fn is_trait_def_id(&self, def_id: GlobalDefId) -> bool {
        self.defs_for_module(def_id.module_id)
            .and_then(|defs| defs.defs.get(def_id.def_id))
            .is_some_and(|def| def.kind == DefKind::Trait)
    }

    fn push_trait_obligation_with_supertraits(
        &mut self,
        obligations: &mut Vec<TraitObligation>,
        obligation: TraitObligation,
    ) {
        self.push_trait_obligation_with_supertraits_inner(
            obligations,
            obligation,
            &mut HashSet::new(),
        );
    }

    fn push_trait_obligation_with_supertraits_inner(
        &mut self,
        obligations: &mut Vec<TraitObligation>,
        obligation: TraitObligation,
        visited: &mut HashSet<(TraitId, Vec<InternedTyId>)>,
    ) {
        let key = (obligation.trait_id, obligation.trait_args.clone());
        if !visited.insert(key) {
            return;
        }
        if !obligations
            .iter()
            .any(|existing| self.trait_obligations_equivalent(existing, &obligation))
        {
            obligations.push(obligation.clone());
        }
        match obligation.trait_id {
            TraitId::Builtin(trait_id) => {
                for supertrait in trait_id.supertraits() {
                    let trait_args = if supertrait.preserves_trait_args {
                        obligation.trait_args.clone()
                    } else {
                        Vec::new()
                    };
                    self.push_trait_obligation_with_supertraits_inner(
                        obligations,
                        TraitObligation {
                            self_ty: obligation.self_ty,
                            trait_id: TraitId::Builtin(supertrait.trait_id),
                            trait_args,
                            associated_type_bindings: Vec::new(),
                        },
                        visited,
                    );
                }
            }
            TraitId::Source(source_trait_id) => {
                let Some(trait_signature) = self.resolved_trait_signature(source_trait_id) else {
                    return;
                };
                let substitutions =
                    self.generic_substitutions(&trait_signature.generics, &obligation.trait_args);
                for supertrait in &trait_signature.supertraits {
                    let supertrait = self.substitute_generics(*supertrait, &substitutions);
                    let supertrait = self.normalization.normalize(supertrait);
                    let Some((trait_id, trait_args)) = (match self.interner.get(supertrait).cloned()
                    {
                        Some(TyKind::Nominal {
                            def_id: supertrait_id,
                            args: supertrait_args,
                        }) => Some((TraitId::Source(supertrait_id), supertrait_args)),
                        Some(TyKind::BuiltinTrait { trait_id, args }) => {
                            Some((TraitId::Builtin(trait_id), args))
                        }
                        _ => None,
                    }) else {
                        continue;
                    };
                    self.push_trait_obligation_with_supertraits_inner(
                        obligations,
                        TraitObligation {
                            self_ty: obligation.self_ty,
                            trait_id,
                            trait_args,
                            associated_type_bindings: Vec::new(),
                        },
                        visited,
                    );
                }
            }
        }
    }

    pub(crate) fn visible_trait_arg_candidates(
        &mut self,
        self_ty: InternedTyId,
        trait_id: TraitId,
    ) -> Vec<Vec<InternedTyId>> {
        let mut candidates = Vec::new();
        let obligations = self
            .current_def_id
            .and_then(|def_id| (def_id.module_id == self.defs.module_id).then_some(def_id.def_id))
            .and_then(|def_id| {
                let signature = self.signatures.functions.get(&def_id)?.clone();
                Some(self.function_signature_trait_obligations(def_id, &signature))
            })
            .unwrap_or_default();
        for obligation in obligations {
            if obligation.trait_id == trait_id
                && self.types_equivalent_without_projection_resolution(obligation.self_ty, self_ty)
            {
                self.push_unique_trait_arg_candidate(&mut candidates, obligation.trait_args);
            }
        }

        let impls = self.program_trait_impls.to_vec();
        for impl_signature in impls {
            if impl_signature.trait_id != trait_id {
                continue;
            }
            if !self.trait_impl_signature_is_visible(&impl_signature) {
                continue;
            }
            let target_ty =
                self.import_type_from(&impl_signature.interner, impl_signature.target_ty);
            let target_ty = self.normalization.normalize(target_ty);
            let mut substitutions = HashMap::new();
            if !self.match_type_pattern(target_ty, self_ty, &mut substitutions) {
                continue;
            }
            let trait_args = impl_signature
                .trait_args
                .iter()
                .map(|arg| {
                    let arg = self.import_type_from(&impl_signature.interner, *arg);
                    let arg = self.substitute_generics(arg, &substitutions);
                    self.normalization.normalize(arg)
                })
                .collect::<Vec<_>>();
            self.push_unique_trait_arg_candidate(&mut candidates, trait_args);
        }
        candidates
    }

    fn push_unique_trait_arg_candidate(
        &mut self,
        candidates: &mut Vec<Vec<InternedTyId>>,
        trait_args: Vec<InternedTyId>,
    ) {
        if candidates.iter().any(|candidate| {
            candidate.len() == trait_args.len()
                && candidate.iter().zip(&trait_args).all(|(left, right)| {
                    self.types_equivalent_without_projection_resolution(*left, *right)
                })
        }) {
            return;
        }
        candidates.push(trait_args);
    }

    fn trait_impl_signature_is_visible(
        &mut self,
        impl_signature: &ProgramTraitImplSignature,
    ) -> bool {
        self.extensions
            .has_trait_witness_impl(impl_signature.module_id, impl_signature.local_index)
    }

    fn check_type_projection_obligations(
        &mut self,
        span: Span,
        ty: InternedTyId,
        obligations: &[TraitObligation],
    ) {
        let ty = self.normalization.normalize(ty);
        match self.interner.get(ty).cloned() {
            Some(
                TyKind::Pointer { elem, .. }
                | TyKind::Slice { elem, .. }
                | TyKind::SlicePointee { elem },
            ) => {
                self.check_type_projection_obligations(span, elem, obligations);
            }
            Some(TyKind::Array { elem, .. }) => {
                self.check_type_projection_obligations(span, elem, obligations);
            }
            Some(TyKind::Range { bound, .. }) => {
                if let Some(bound) = bound {
                    self.check_type_projection_obligations(span, bound, obligations);
                }
            }
            Some(TyKind::FunctionPointer {
                params,
                return_type,
                ..
            }) => {
                for param in params {
                    self.check_type_projection_obligations(span, param, obligations);
                }
                self.check_type_projection_obligations(span, return_type, obligations);
            }
            Some(TyKind::Optional { elem }) => {
                self.check_type_projection_obligations(span, elem, obligations);
            }
            Some(TyKind::ErrorUnion { error, value }) => {
                self.check_type_projection_obligations(span, error, obligations);
                self.check_type_projection_obligations(span, value, obligations);
            }
            Some(TyKind::Nominal { def_id, args }) => {
                for arg in &args {
                    self.check_type_projection_obligations(span, *arg, obligations);
                }
                self.check_nominal_where_obligations(span, def_id, &args, obligations);
            }
            Some(TyKind::BuiltinTrait { args, .. }) => {
                for arg in args {
                    self.check_type_projection_obligations(span, arg, obligations);
                }
            }
            Some(TyKind::TraitObject {
                trait_id,
                trait_args,
                associated_type_bindings,
                ..
            })
            | Some(TyKind::TraitObjectPointee {
                trait_id,
                trait_args,
                associated_type_bindings,
            }) => {
                for arg in &trait_args {
                    self.check_type_projection_obligations(span, *arg, obligations);
                }
                if let TraitId::Source(def_id) = trait_id {
                    self.check_nominal_where_obligations(span, def_id, &trait_args, obligations);
                }
                for binding in associated_type_bindings {
                    for arg in binding.trait_args {
                        self.check_type_projection_obligations(span, arg, obligations);
                    }
                    self.check_type_projection_obligations(span, binding.ty, obligations);
                }
            }
            Some(TyKind::Projection {
                self_ty,
                trait_id,
                trait_args,
                ..
            }) => {
                self.check_type_projection_obligations(span, self_ty, obligations);
                for arg in &trait_args {
                    self.check_type_projection_obligations(span, *arg, obligations);
                }
                let required = TraitObligation {
                    self_ty,
                    trait_id,
                    trait_args,
                    associated_type_bindings: Vec::new(),
                };
                if !self.proves_trait_obligation(obligations, &required) {
                    self.diagnostics
                        .push(nia_diagnostic::Diagnostic::user_error_at(
                            "E0301",
                            span,
                            format!(
                                "trait bound not satisfied: {}: {}",
                                self.ty_name(required.self_ty),
                                self.trait_ty_name(required.trait_id, &required.trait_args)
                            ),
                        ));
                }
            }
            Some(
                TyKind::Error
                | TyKind::ComptimeOnly
                | TyKind::Primitive(_)
                | TyKind::Vector { .. }
                | TyKind::GenericParam(_),
            )
            | None => {}
        }
    }

    fn check_nominal_where_obligations(
        &mut self,
        span: Span,
        def_id: GlobalDefId,
        args: &[InternedTyId],
        obligations: &[TraitObligation],
    ) {
        let Some((generics, predicates)) = self
            .resolved_struct_signature(def_id)
            .map(|resolved| {
                (
                    resolved.signature.generics,
                    resolved.signature.where_predicates,
                )
            })
            .or_else(|| {
                self.resolved_union_signature(def_id).map(|resolved| {
                    (
                        resolved.signature.generics,
                        resolved.signature.where_predicates,
                    )
                })
            })
            .or_else(|| {
                self.resolved_trait_signature(def_id)
                    .map(|signature| (signature.generics, signature.where_predicates))
            })
        else {
            return;
        };
        let substitutions = generics
            .iter()
            .cloned()
            .zip(args.iter().copied())
            .collect::<std::collections::HashMap<_, _>>();
        let predicates = predicates
            .iter()
            .map(|predicate| self.substitute_where_predicate(predicate, &substitutions))
            .collect::<Vec<_>>();
        for predicate in predicates {
            for bound in predicate.bounds {
                let Some((trait_id, trait_args)) = self.trait_id_and_args(bound.trait_ty) else {
                    continue;
                };
                let required = TraitObligation {
                    self_ty: predicate.ty,
                    trait_id,
                    trait_args,
                    associated_type_bindings: Vec::new(),
                };
                if !self.proves_trait_obligation(obligations, &required) {
                    self.diagnostics
                        .push(nia_diagnostic::Diagnostic::user_error_at(
                            "E0301",
                            span,
                            format!(
                                "trait bound not satisfied: {}: {}",
                                self.ty_name(required.self_ty),
                                self.trait_ty_name(required.trait_id, &required.trait_args)
                            ),
                        ));
                }
            }
        }
    }

    fn proves_trait_obligation(
        &mut self,
        obligations: &[TraitObligation],
        required: &TraitObligation,
    ) -> bool {
        matches!(
            self.resolve_trait_obligation(obligations, required),
            TraitResolution::Intrinsic(_) | TraitResolution::User(_) | TraitResolution::Assumed(_)
        )
    }

    fn resolve_trait_obligation(
        &mut self,
        obligations: &[TraitObligation],
        required: &TraitObligation,
    ) -> TraitResolution {
        let assumptions = self.trait_goals_for_obligations(obligations);
        let associated_type_assumptions =
            self.associated_type_assumptions_for_obligations(obligations);
        let context = TraitSolverContext {
            normalization: self.normalization,
            trait_impls: self.program_trait_impls,
            layouts: Some(self.layouts),
            local_module_id: self.defs.module_id,
            local_enums: &self.signatures.enums,
            program_enums: Some(self.program_enums),
        };
        let mut solver = context.solver_with_associated_type_assumptions(
            &mut self.interner,
            &assumptions,
            &associated_type_assumptions,
        );
        solver.resolve(TraitGoal {
            self_ty: required.self_ty,
            trait_id: required.trait_id,
            trait_args: required.trait_args.clone(),
        })
    }

    fn trait_goals_for_obligations(&self, obligations: &[TraitObligation]) -> Vec<TraitGoal> {
        obligations.iter().cloned().map(TraitGoal::from).collect()
    }

    fn associated_type_assumptions_for_obligations(
        &self,
        obligations: &[TraitObligation],
    ) -> Vec<AssociatedTypeProjectionEq> {
        obligations
            .iter()
            .flat_map(|obligation| {
                obligation.associated_type_bindings.iter().map(|binding| {
                    AssociatedTypeProjectionEq {
                        goal: TraitGoal {
                            self_ty: obligation.self_ty,
                            trait_id: obligation.trait_id,
                            trait_args: obligation.trait_args.clone(),
                        },
                        name: binding.name.clone(),
                        ty: binding.ty,
                    }
                })
            })
            .collect()
    }

    fn trait_obligations_equivalent(
        &self,
        left: &TraitObligation,
        right: &TraitObligation,
    ) -> bool {
        left.trait_id == right.trait_id
            && left.trait_args.len() == right.trait_args.len()
            && self.types_equivalent_without_projection_resolution(left.self_ty, right.self_ty)
            && left
                .trait_args
                .iter()
                .zip(&right.trait_args)
                .all(|(left, right)| {
                    self.types_equivalent_without_projection_resolution(*left, *right)
                })
    }

    pub(crate) fn types_equivalent_without_projection_resolution(
        &self,
        left: InternedTyId,
        right: InternedTyId,
    ) -> bool {
        let left = self.normalization.normalize(left);
        let right = self.normalization.normalize(right);
        if left == right {
            return true;
        }
        match (self.interner.get(left), self.interner.get(right)) {
            (Some(TyKind::Primitive(left)), Some(TyKind::Primitive(right))) => left == right,
            (
                Some(TyKind::Pointer {
                    is_readonly: left_const,
                    elem: left_elem,
                }),
                Some(TyKind::Pointer {
                    is_readonly: right_const,
                    elem: right_elem,
                }),
            )
            | (
                Some(TyKind::Slice {
                    is_readonly: left_const,
                    elem: left_elem,
                }),
                Some(TyKind::Slice {
                    is_readonly: right_const,
                    elem: right_elem,
                }),
            ) => {
                left_const == right_const
                    && self.types_equivalent_without_projection_resolution(*left_elem, *right_elem)
            }
            (
                Some(TyKind::Array {
                    len: left_len,
                    elem: left_elem,
                }),
                Some(TyKind::Array {
                    len: right_len,
                    elem: right_elem,
                }),
            ) => {
                left_len == right_len
                    && self.types_equivalent_without_projection_resolution(*left_elem, *right_elem)
            }
            (
                Some(TyKind::Range {
                    kind: left_kind,
                    bound: left_bound,
                }),
                Some(TyKind::Range {
                    kind: right_kind,
                    bound: right_bound,
                }),
            ) => {
                left_kind == right_kind
                    && match (left_bound, right_bound) {
                        (Some(left_bound), Some(right_bound)) => self
                            .types_equivalent_without_projection_resolution(
                                *left_bound,
                                *right_bound,
                            ),
                        (None, None) => true,
                        _ => false,
                    }
            }
            (
                Some(TyKind::FunctionPointer {
                    params: left_params,
                    return_type: left_return,
                    is_variadic: left_variadic,
                }),
                Some(TyKind::FunctionPointer {
                    params: right_params,
                    return_type: right_return,
                    is_variadic: right_variadic,
                }),
            ) => {
                left_variadic == right_variadic
                    && left_params.len() == right_params.len()
                    && left_params.iter().zip(right_params).all(|(left, right)| {
                        self.types_equivalent_without_projection_resolution(*left, *right)
                    })
                    && self
                        .types_equivalent_without_projection_resolution(*left_return, *right_return)
            }
            (
                Some(TyKind::Nominal {
                    def_id: left_def,
                    args: left_args,
                }),
                Some(TyKind::Nominal {
                    def_id: right_def,
                    args: right_args,
                }),
            ) => {
                left_def == right_def
                    && left_args.len() == right_args.len()
                    && left_args.iter().zip(right_args).all(|(left, right)| {
                        self.types_equivalent_without_projection_resolution(*left, *right)
                    })
            }
            (
                Some(TyKind::BuiltinTrait {
                    trait_id: left_trait,
                    args: left_args,
                }),
                Some(TyKind::BuiltinTrait {
                    trait_id: right_trait,
                    args: right_args,
                }),
            ) => {
                left_trait == right_trait
                    && left_args.len() == right_args.len()
                    && left_args.iter().zip(right_args).all(|(left, right)| {
                        self.types_equivalent_without_projection_resolution(*left, *right)
                    })
            }
            (
                Some(TyKind::Projection {
                    self_ty: left_self,
                    trait_id: left_trait,
                    trait_args: left_args,
                    name: left_name,
                }),
                Some(TyKind::Projection {
                    self_ty: right_self,
                    trait_id: right_trait,
                    trait_args: right_args,
                    name: right_name,
                }),
            ) => {
                left_trait == right_trait
                    && left_name == right_name
                    && left_args.len() == right_args.len()
                    && self.types_equivalent_without_projection_resolution(*left_self, *right_self)
                    && left_args.iter().zip(right_args).all(|(left, right)| {
                        self.types_equivalent_without_projection_resolution(*left, *right)
                    })
            }
            (Some(TyKind::GenericParam(left)), Some(TyKind::GenericParam(right))) => left == right,
            _ => false,
        }
    }
}
