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
use nia_ty::ConstGenericArg;
use nia_ty::{TraitId, TyKind};

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub(crate) struct TraitObligation {
    self_ty: InternedTyId,
    trait_id: TraitId,
    trait_args: Vec<InternedTyId>,
    trait_const_args: Vec<ConstGenericArg>,
    associated_type_bindings: Vec<TraitObligationAssociatedTypeBinding>,
}

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
struct TraitObligationAssociatedTypeBinding {
    name: String,
    ty: InternedTyId,
}

#[derive(Debug, Clone)]
struct MethodTraitImplContext {
    target_ty: InternedTyId,
    trait_ref: Option<(TraitId, Vec<InternedTyId>, Vec<ConstGenericArg>)>,
    where_predicates: Vec<WherePredicateSignature>,
    associated_types: Vec<nia_item_signatures::TraitImplAssociatedTypeSignature>,
}

impl From<TraitObligation> for TraitGoal {
    fn from(obligation: TraitObligation) -> Self {
        Self {
            self_ty: obligation.self_ty,
            trait_id: obligation.trait_id,
            trait_args: obligation.trait_args,
            trait_const_args: obligation.trait_const_args,
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
        self.current_context_proves_trait_obligation_with_const_args(
            self_ty,
            trait_id,
            trait_args,
            Vec::new(),
        )
    }

    pub(crate) fn current_context_proves_trait_obligation_with_const_args(
        &mut self,
        self_ty: InternedTyId,
        trait_id: TraitId,
        trait_args: Vec<InternedTyId>,
        trait_const_args: Vec<ConstGenericArg>,
    ) -> bool {
        self.profile_stage("body_check.profile.trait_obligation.proves", |this| {
            matches!(
                this.current_context_resolve_trait_obligation_with_const_args(
                    self_ty,
                    trait_id,
                    trait_args,
                    trait_const_args,
                ),
                TraitResolution::Intrinsic(_)
                    | TraitResolution::User(_)
                    | TraitResolution::Assumed(_)
            )
        })
    }

    pub(crate) fn current_context_resolve_trait_obligation(
        &mut self,
        self_ty: InternedTyId,
        trait_id: TraitId,
        trait_args: Vec<InternedTyId>,
    ) -> TraitResolution {
        self.current_context_resolve_trait_obligation_with_const_args(
            self_ty,
            trait_id,
            trait_args,
            Vec::new(),
        )
    }

    pub(crate) fn current_context_resolve_trait_obligation_with_const_args(
        &mut self,
        self_ty: InternedTyId,
        trait_id: TraitId,
        trait_args: Vec<InternedTyId>,
        trait_const_args: Vec<ConstGenericArg>,
    ) -> TraitResolution {
        let self_ty = self.normalize_aliases_in_type(self_ty);
        let key = crate::TraitObligationResolutionKey {
            current_def_id: self.current_def_id,
            self_ty,
            trait_id,
            trait_args: trait_args.clone(),
            trait_const_args: trait_const_args.clone(),
        };
        if let Some(resolution) = self.trait_obligation_resolution_cache.get(&key) {
            return resolution.clone();
        }
        let obligations = self.current_trait_obligations();
        let required = TraitObligation {
            self_ty,
            trait_id,
            trait_args,
            trait_const_args,
            associated_type_bindings: Vec::new(),
        };
        let resolution = self.resolve_trait_obligation(&obligations, &required);
        self.trait_obligation_resolution_cache
            .insert(key, resolution.clone());
        resolution
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
        if let Some(obligations) = self.def_trait_obligations_cache.get(&def_id) {
            return obligations.clone();
        }
        let mut obligations = Vec::new();
        self.push_method_owner_trait_obligations(def_id, &mut obligations);
        if let Some(signature) = self.signatures.functions.get(&def_id).cloned() {
            if self
                .function_signature_scope
                .includes_function(&self.global_def_id(def_id))
            {
                self.push_where_predicate_obligations(
                    &mut obligations,
                    &signature.where_predicates,
                );
            }
        }
        self.def_trait_obligations_cache
            .insert(def_id, obligations.clone());
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
                if let Some(impl_context) =
                    self.trait_impl_context_for_method(self.global_def_id(def_id))
                {
                    let predicates =
                        self.instantiate_impl_where_predicates_for_method(def_id, &impl_context);
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
        impl_context: &MethodTraitImplContext,
    ) -> Vec<WherePredicateSignature> {
        let Some(owner_ty) = self.method_owner_type(def_id) else {
            return impl_context.where_predicates.clone();
        };
        let target_ty = self.normalization.normalize(impl_context.target_ty);
        let owner_ty = self.normalization.normalize(owner_ty);
        let mut substitutions = HashMap::new();
        let mut const_substitutions = HashMap::new();
        self.match_type_pattern_with_consts(
            target_ty,
            owner_ty,
            &mut substitutions,
            &mut const_substitutions,
        );
        impl_context
            .where_predicates
            .iter()
            .map(|predicate| {
                self.substitute_where_predicate_with_consts(
                    predicate,
                    &substitutions,
                    &const_substitutions,
                )
            })
            .collect()
    }

    fn push_nominal_owner_where_obligations(
        &mut self,
        obligations: &mut Vec<TraitObligation>,
        owner_ty: InternedTyId,
    ) {
        let owner_ty = self.normalization.normalize(owner_ty);
        let Some(TyKind::Nominal {
            def_id,
            args,
            const_args,
        }) = self.interner.get(owner_ty).cloned()
        else {
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
        let (substitutions, const_substitutions) =
            self.generic_substitutions_and_consts_for_def(def_id, &args, &const_args);
        let predicates = predicates
            .1
            .iter()
            .map(|predicate| {
                self.substitute_where_predicate_with_consts(
                    predicate,
                    &substitutions,
                    &const_substitutions,
                )
            })
            .collect::<Vec<_>>();
        self.push_where_predicate_obligations(obligations, &predicates);
    }

    pub(crate) fn substitute_where_predicate_with_consts(
        &mut self,
        predicate: &WherePredicateSignature,
        substitutions: &std::collections::HashMap<String, InternedTyId>,
        const_substitutions: &std::collections::HashMap<String, ConstGenericArg>,
    ) -> WherePredicateSignature {
        WherePredicateSignature {
            ty: self.substitute_generics_and_consts(
                predicate.ty,
                substitutions,
                const_substitutions,
            ),
            bounds: predicate
                .bounds
                .iter()
                .map(|bound| nia_item_signatures::WhereBoundSignature {
                    trait_ty: self.substitute_generics_and_consts(
                        bound.trait_ty,
                        substitutions,
                        const_substitutions,
                    ),
                    associated_type_bindings: bound
                        .associated_type_bindings
                        .iter()
                        .map(
                            |binding| nia_item_signatures::AssociatedTypeBindingSignature {
                                name: binding.name.clone(),
                                ty: self.substitute_generics_and_consts(
                                    binding.ty,
                                    substitutions,
                                    const_substitutions,
                                ),
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
        const_substitutions: &std::collections::HashMap<String, ConstGenericArg>,
        span: Span,
    ) {
        let predicates = predicates
            .iter()
            .map(|predicate| {
                self.substitute_where_predicate_with_consts(
                    predicate,
                    substitutions,
                    const_substitutions,
                )
            })
            .collect::<Vec<_>>();
        for predicate in predicates {
            for bound in predicate.bounds {
                let Some((trait_id, trait_args, trait_const_args)) =
                    self.trait_id_and_args(bound.trait_ty)
                else {
                    continue;
                };
                if !self.current_context_proves_trait_obligation_with_const_args(
                    predicate.ty,
                    trait_id,
                    trait_args.clone(),
                    trait_const_args.clone(),
                ) {
                    self.diagnostics
                        .push(nia_diagnostic::Diagnostic::user_error_at(
                            nia_diagnostic::codes::TYPE_CHECK,
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

    pub(crate) fn infer_where_predicate_candidates(
        &mut self,
        predicate: &WherePredicateSignature,
        substitutions: &HashMap<String, InternedTyId>,
    ) -> Vec<HashMap<String, InternedTyId>> {
        let self_ty = self.substitute_ty(predicate.ty, substitutions);
        let bounds = predicate.bounds.clone();
        let mut candidates = Vec::new();
        for bound in bounds {
            let bound_ty = self.substitute_ty(bound.trait_ty, substitutions);
            let Some((trait_id, trait_args, trait_const_args)) = self.trait_id_and_args(bound_ty)
            else {
                continue;
            };
            self.push_where_predicate_trait_impl_candidates(
                self_ty,
                trait_id,
                &trait_args,
                &trait_const_args,
                substitutions,
                &mut candidates,
            );
        }
        candidates
    }

    fn push_where_predicate_trait_impl_candidates(
        &mut self,
        self_ty: InternedTyId,
        trait_id: TraitId,
        trait_args: &[InternedTyId],
        trait_const_args: &[ConstGenericArg],
        substitutions: &HashMap<String, InternedTyId>,
        candidates: &mut Vec<HashMap<String, InternedTyId>>,
    ) {
        for impl_index in self.trait_impl_indexes_for_trait(trait_id) {
            let impl_signature = self.program_trait_impls[impl_index].clone();
            if !self.trait_impl_signature_is_visible(&impl_signature) {
                continue;
            }
            let impl_target_ty =
                self.import_type_from(&impl_signature.interner, impl_signature.target_ty);
            let mut impl_substitutions = HashMap::new();
            if !self.match_type_pattern(impl_target_ty, self_ty, &mut impl_substitutions) {
                continue;
            }

            let mut candidate = substitutions.clone();
            let mut ok = true;
            for (required_arg, impl_arg) in trait_args.iter().zip(&impl_signature.trait_args) {
                let impl_arg = self.import_type_from(&impl_signature.interner, *impl_arg);
                let impl_arg = self.substitute_generics(impl_arg, &impl_substitutions);
                if !self.match_where_candidate_type(*required_arg, impl_arg, &mut candidate) {
                    ok = false;
                    break;
                }
            }
            if ok && trait_const_args.len() != impl_signature.trait_const_args.len() {
                ok = false;
            }
            if ok {
                for (required_arg, impl_arg) in trait_const_args
                    .iter()
                    .zip(&impl_signature.trait_const_args)
                {
                    let mut impl_arg = impl_arg.clone();
                    impl_arg.ty = self.import_type_from(&impl_signature.interner, impl_arg.ty);
                    impl_arg.ty = self.substitute_generics(impl_arg.ty, &impl_substitutions);
                    if required_arg != &impl_arg {
                        ok = false;
                        break;
                    }
                }
            }
            if ok {
                self.push_unique_where_candidate(candidates, candidate);
            }
        }
    }

    fn match_where_candidate_type(
        &mut self,
        required: InternedTyId,
        actual: InternedTyId,
        substitutions: &mut HashMap<String, InternedTyId>,
    ) -> bool {
        let required = self.normalization.normalize(required);
        let actual = self.normalization.normalize(actual);
        match self.interner.get(required).cloned() {
            Some(TyKind::GenericParam(name)) => {
                if let Some(existing) = substitutions.get(&name).copied() {
                    self.types_match(existing, actual)
                } else {
                    substitutions.insert(name, actual);
                    true
                }
            }
            Some(TyKind::Pointer {
                is_readonly: required_readonly,
                elem: required_elem,
            }) => {
                let Some(TyKind::Pointer {
                    is_readonly: actual_readonly,
                    elem: actual_elem,
                }) = self.interner.get(actual).cloned()
                else {
                    return false;
                };
                required_readonly == actual_readonly
                    && self.match_where_candidate_type(required_elem, actual_elem, substitutions)
            }
            Some(TyKind::VolatilePointer {
                is_readonly: required_readonly,
                elem: required_elem,
            }) => {
                let Some(TyKind::VolatilePointer {
                    is_readonly: actual_readonly,
                    elem: actual_elem,
                }) = self.interner.get(actual).cloned()
                else {
                    return false;
                };
                required_readonly == actual_readonly
                    && self.match_where_candidate_type(required_elem, actual_elem, substitutions)
            }
            Some(TyKind::Slice {
                is_readonly: required_readonly,
                elem: required_elem,
            }) => {
                let Some(TyKind::Slice {
                    is_readonly: actual_readonly,
                    elem: actual_elem,
                }) = self.interner.get(actual).cloned()
                else {
                    return false;
                };
                required_readonly == actual_readonly
                    && self.match_where_candidate_type(required_elem, actual_elem, substitutions)
            }
            Some(TyKind::Nominal {
                def_id: required_def,
                args: required_args,
                const_args: required_const_args,
            }) => {
                let Some(TyKind::Nominal {
                    def_id: actual_def,
                    args: actual_args,
                    const_args: actual_const_args,
                }) = self.interner.get(actual).cloned()
                else {
                    return false;
                };
                required_def == actual_def
                    && required_const_args == actual_const_args
                    && required_args.len() == actual_args.len()
                    && required_args
                        .iter()
                        .zip(actual_args)
                        .all(|(required, actual)| {
                            self.match_where_candidate_type(*required, actual, substitutions)
                        })
            }
            _ => self.types_match(required, actual),
        }
    }

    fn push_unique_where_candidate(
        &mut self,
        candidates: &mut Vec<HashMap<String, InternedTyId>>,
        candidate: HashMap<String, InternedTyId>,
    ) {
        if candidates.iter().any(|existing| {
            existing.len() == candidate.len()
                && existing.iter().all(|(name, left)| {
                    candidate
                        .get(name)
                        .is_some_and(|right| self.types_match(*left, *right))
                })
        }) {
            return;
        }
        candidates.push(candidate);
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
            Some(TyKind::VolatilePointer { is_readonly, elem }) => {
                let elem = self.substitute_ty(elem, substitutions);
                self.interner
                    .intern(TyKind::VolatilePointer { is_readonly, elem })
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
            Some(TyKind::Nominal { def_id, args, .. }) => {
                let args = args
                    .into_iter()
                    .map(|arg| self.substitute_ty(arg, substitutions))
                    .collect();
                self.interner.intern(TyKind::Nominal {
                    def_id,
                    args,
                    const_args: Vec::new(),
                })
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
                trait_const_args,
                associated_type_bindings,
                ..
            }) => {
                let trait_args = trait_args
                    .into_iter()
                    .map(|arg| self.substitute_ty(arg, substitutions))
                    .collect();
                let trait_const_args = trait_const_args
                    .into_iter()
                    .map(|mut arg| {
                        arg.ty = self.substitute_ty(arg.ty, substitutions);
                        arg
                    })
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
                        trait_const_args: binding
                            .trait_const_args
                            .into_iter()
                            .map(|mut arg| {
                                arg.ty = self.substitute_ty(arg.ty, substitutions);
                                arg
                            })
                            .collect(),
                        name: binding.name,
                        ty: self.substitute_ty(binding.ty, substitutions),
                    })
                    .collect();
                self.interner.intern(TyKind::TraitObject {
                    is_readonly,
                    trait_id,
                    trait_args,
                    trait_const_args,
                    associated_type_bindings,
                })
            }
            Some(TyKind::TraitObjectPointee {
                trait_id,
                trait_args,
                trait_const_args,
                associated_type_bindings,
                ..
            }) => {
                let trait_args = trait_args
                    .into_iter()
                    .map(|arg| self.substitute_ty(arg, substitutions))
                    .collect();
                let trait_const_args = trait_const_args
                    .into_iter()
                    .map(|mut arg| {
                        arg.ty = self.substitute_ty(arg.ty, substitutions);
                        arg
                    })
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
                        trait_const_args: binding
                            .trait_const_args
                            .into_iter()
                            .map(|mut arg| {
                                arg.ty = self.substitute_ty(arg.ty, substitutions);
                                arg
                            })
                            .collect(),
                        name: binding.name,
                        ty: self.substitute_ty(binding.ty, substitutions),
                    })
                    .collect();
                self.interner.intern(TyKind::TraitObjectPointee {
                    trait_id,
                    trait_args,
                    trait_const_args,
                    associated_type_bindings,
                })
            }
            Some(TyKind::Projection {
                self_ty,
                trait_id,
                trait_args,
                trait_const_args,
                name,
                ..
            }) => {
                let self_ty = self.substitute_ty(self_ty, substitutions);
                let trait_args = trait_args
                    .into_iter()
                    .map(|arg| self.substitute_ty(arg, substitutions))
                    .collect();
                let trait_const_args = trait_const_args
                    .into_iter()
                    .map(|mut arg| {
                        arg.ty = self.substitute_ty(arg.ty, substitutions);
                        arg
                    })
                    .collect();
                self.interner.intern(TyKind::Projection {
                    self_ty,
                    trait_id,
                    trait_args,
                    trait_const_args,
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
                let (trait_args, trait_const_args) =
                    self.generic_param_args_for_trait_obligation(trait_id);
                let trait_id = trait_signature
                    .builtin
                    .map(TraitId::Builtin)
                    .unwrap_or(TraitId::Source(trait_id));
                Some(TraitObligation {
                    self_ty: self
                        .interner
                        .intern(TyKind::GenericParam("Self".to_string())),
                    trait_id,
                    trait_args,
                    trait_const_args,
                    associated_type_bindings: Vec::new(),
                })
            }
            DefKind::Method => {
                if let Some(TyKind::TraitObjectPointee {
                    trait_id,
                    trait_args,
                    trait_const_args,
                    associated_type_bindings,
                    ..
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
                        trait_const_args,
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
                let impl_context = self.trait_impl_context_for_method(method_id)?;
                let (trait_id, trait_args, trait_const_args) = impl_context.trait_ref?;
                Some(TraitObligation {
                    self_ty: target_ty,
                    trait_id,
                    trait_args,
                    trait_const_args,
                    associated_type_bindings: impl_context
                        .associated_types
                        .iter()
                        .map(|associated_type| TraitObligationAssociatedTypeBinding {
                            name: associated_type.name.clone(),
                            ty: associated_type.ty,
                        })
                        .collect(),
                })
            }
            _ => None,
        }
    }

    fn generic_param_args_for_trait_obligation(
        &mut self,
        trait_id: GlobalDefId,
    ) -> (Vec<InternedTyId>, Vec<ConstGenericArg>) {
        let Some(defs) = self.defs_for_module(trait_id.module_id) else {
            return (Vec::new(), Vec::new());
        };
        let Some(def) = defs.as_ref().defs.get(trait_id.def_id) else {
            return (Vec::new(), Vec::new());
        };
        let params = def.generic_params.clone();
        let mut type_args = Vec::new();
        let mut const_args = Vec::new();
        for param in params {
            match param.kind {
                nia_ast::GenericParamKind::Type => {
                    type_args.push(self.interner.intern(TyKind::GenericParam(param.name)));
                }
                nia_ast::GenericParamKind::Comptime { ty } => {
                    let ty = self.ty_for_type(&ty);
                    const_args.push(ConstGenericArg {
                        ty,
                        value: nia_ty::ConstGenericValue::GenericParam(param.name),
                    });
                }
            }
        }
        (type_args, const_args)
    }

    fn trait_impl_context_for_method(
        &mut self,
        method_id: GlobalDefId,
    ) -> Option<MethodTraitImplContext> {
        if method_id.module_id == self.defs.module_id
            && let Some(signature) = self
                .local_trait_impl_signature_for_method(method_id)
                .cloned()
        {
            return self.local_trait_impl_context(&signature);
        }
        let lookup = self.extension_method_lookup_for_id(method_id)?;
        let impl_signature = self
            .program_trait_impls
            .iter()
            .find(|impl_signature| {
                impl_signature.module_id == method_id.module_id
                    && impl_signature.impl_id == lookup.impl_id
            })?
            .clone();
        Some(self.program_trait_impl_context(&impl_signature))
    }

    fn local_trait_impl_signature_for_method(
        &self,
        method_id: GlobalDefId,
    ) -> Option<&TraitImplSignature> {
        self.signatures.trait_impls.iter().find(|signature| {
            signature
                .methods
                .iter()
                .any(|method| method.def_id == method_id.def_id)
        })
    }

    fn local_trait_impl_context(
        &mut self,
        signature: &TraitImplSignature,
    ) -> Option<MethodTraitImplContext> {
        let source = self.normalization.interner.clone();
        let target_ty = self.import_type_from(&source, signature.target_ty);
        let trait_ref = match signature.trait_ty {
            Some(trait_ty) => {
                let trait_ty = self.import_type_from(&source, trait_ty);
                let trait_ty = self.normalization.normalize(trait_ty);
                Some(self.trait_id_and_args(trait_ty)?)
            }
            None => None,
        };
        Some(MethodTraitImplContext {
            target_ty,
            trait_ref,
            where_predicates: self
                .import_where_predicates_from(&source, &signature.where_predicates),
            associated_types: self
                .import_trait_impl_associated_types_from(&source, &signature.associated_types),
        })
    }

    fn program_trait_impl_context(
        &mut self,
        signature: &ProgramTraitImplSignature,
    ) -> MethodTraitImplContext {
        let source = signature.interner.clone();
        MethodTraitImplContext {
            target_ty: self.import_type_from(&source, signature.target_ty),
            trait_ref: Some((
                signature.trait_id,
                signature
                    .trait_args
                    .iter()
                    .map(|arg| self.import_type_from(&source, *arg))
                    .collect(),
                signature
                    .trait_const_args
                    .iter()
                    .map(|arg| nia_ty::ConstGenericArg {
                        ty: self.import_type_from(&source, arg.ty),
                        value: arg.value.clone(),
                    })
                    .collect(),
            )),
            where_predicates: self
                .import_where_predicates_from(&source, &signature.where_predicates),
            associated_types: self
                .import_trait_impl_associated_types_from(&source, &signature.associated_types),
        }
    }

    fn import_trait_impl_associated_types_from(
        &mut self,
        source: &nia_ty::TyInterner,
        associated_types: &[nia_item_signatures::TraitImplAssociatedTypeSignature],
    ) -> Vec<nia_item_signatures::TraitImplAssociatedTypeSignature> {
        associated_types
            .iter()
            .map(
                |associated_type| nia_item_signatures::TraitImplAssociatedTypeSignature {
                    name: associated_type.name.clone(),
                    ty: self.import_type_from(source, associated_type.ty),
                    span: associated_type.span,
                },
            )
            .collect()
    }

    fn push_trait_obligation_from_bound(
        &mut self,
        obligations: &mut Vec<TraitObligation>,
        self_ty: InternedTyId,
        bound: &nia_item_signatures::WhereBoundSignature,
    ) {
        let bound_ty = self.normalization.normalize(bound.trait_ty);
        let Some((trait_id, args, const_args)) = self.trait_id_and_args(bound_ty) else {
            return;
        };
        self.push_trait_obligation_with_supertraits(
            obligations,
            TraitObligation {
                self_ty,
                trait_id,
                trait_args: args,
                trait_const_args: const_args,
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
            .and_then(|defs| defs.as_ref().defs.get(def_id.def_id).map(|def| def.kind))
            == Some(DefKind::Trait)
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
        visited: &mut HashSet<(TraitId, Vec<InternedTyId>, Vec<ConstGenericArg>)>,
    ) {
        let key = (
            obligation.trait_id,
            obligation.trait_args.clone(),
            obligation.trait_const_args.clone(),
        );
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
                            trait_const_args: Vec::new(),
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
                let (substitutions, const_substitutions) = self
                    .generic_substitutions_and_consts_for_def(
                        source_trait_id,
                        &obligation.trait_args,
                        &obligation.trait_const_args,
                    );
                for supertrait in &trait_signature.supertraits {
                    let supertrait = self.substitute_generics_and_consts(
                        supertrait.ty,
                        &substitutions,
                        &const_substitutions,
                    );
                    let supertrait = self.normalization.normalize(supertrait);
                    let Some((trait_id, trait_args, trait_const_args)) =
                        self.trait_id_and_args(supertrait)
                    else {
                        continue;
                    };
                    self.push_trait_obligation_with_supertraits_inner(
                        obligations,
                        TraitObligation {
                            self_ty: obligation.self_ty,
                            trait_id,
                            trait_args,
                            trait_const_args,
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
    ) -> Vec<(Vec<InternedTyId>, Vec<ConstGenericArg>)> {
        let mut candidates = Vec::new();
        let obligations = self
            .current_def_id
            .and_then(|def_id| (def_id.module_id == self.defs.module_id).then_some(def_id.def_id))
            .and_then(|def_id| {
                if !self
                    .function_signature_scope
                    .includes_function(&self.global_def_id(def_id))
                {
                    return None;
                }
                let signature = self.signatures.functions.get(&def_id)?.clone();
                Some(self.function_signature_trait_obligations(def_id, &signature))
            })
            .unwrap_or_default();
        for obligation in obligations {
            if obligation.trait_id == trait_id
                && self.types_equivalent_without_projection_resolution(obligation.self_ty, self_ty)
            {
                self.push_unique_trait_arg_candidate(
                    &mut candidates,
                    obligation.trait_args,
                    obligation.trait_const_args,
                );
            }
        }

        for impl_index in self.trait_impl_indexes_for_trait(trait_id) {
            let impl_signature = self.program_trait_impls[impl_index].clone();
            if !self.trait_impl_signature_is_visible(&impl_signature) {
                continue;
            }
            let target_ty =
                self.import_type_from(&impl_signature.interner, impl_signature.target_ty);
            let target_ty = self.normalization.normalize(target_ty);
            let mut substitutions = HashMap::new();
            let mut const_substitutions = HashMap::new();
            if !self.match_type_pattern_with_consts(
                target_ty,
                self_ty,
                &mut substitutions,
                &mut const_substitutions,
            ) {
                continue;
            }
            let where_predicates = self.import_where_predicates_from(
                &impl_signature.interner,
                &impl_signature.where_predicates,
            );
            if !self.where_predicates_can_hold_with_consts(
                &where_predicates,
                &substitutions,
                &const_substitutions,
            ) {
                continue;
            }
            let trait_args = impl_signature
                .trait_args
                .iter()
                .map(|arg| {
                    let arg = self.import_type_from(&impl_signature.interner, *arg);
                    let arg = self.substitute_generics_and_consts(
                        arg,
                        &substitutions,
                        &const_substitutions,
                    );
                    self.normalization.normalize(arg)
                })
                .collect::<Vec<_>>();
            let trait_const_args = impl_signature
                .trait_const_args
                .iter()
                .map(|arg| {
                    let mut arg = arg.clone();
                    arg.ty = self.import_type_from(&impl_signature.interner, arg.ty);
                    arg.ty = self.substitute_generics_and_consts(
                        arg.ty,
                        &substitutions,
                        &const_substitutions,
                    );
                    match &arg.value {
                        nia_ty::ConstGenericValue::GenericParam(name) => {
                            const_substitutions.get(name).cloned().unwrap_or(arg)
                        }
                        _ => arg,
                    }
                })
                .collect::<Vec<_>>();
            self.push_unique_trait_arg_candidate(&mut candidates, trait_args, trait_const_args);
        }
        candidates
    }

    fn trait_impl_indexes_for_trait(&mut self, trait_id: TraitId) -> Vec<usize> {
        if let Some(indexes) = self.trait_impls_by_trait.get(&trait_id) {
            return indexes.clone();
        }
        let indexes = self
            .program_trait_impls
            .iter()
            .enumerate()
            .filter_map(|(index, impl_signature)| {
                (impl_signature.trait_id == trait_id).then_some(index)
            })
            .collect::<Vec<_>>();
        self.trait_impls_by_trait.insert(trait_id, indexes.clone());
        indexes
    }

    fn push_unique_trait_arg_candidate(
        &mut self,
        candidates: &mut Vec<(Vec<InternedTyId>, Vec<ConstGenericArg>)>,
        trait_args: Vec<InternedTyId>,
        trait_const_args: Vec<ConstGenericArg>,
    ) {
        if candidates
            .iter()
            .any(|(candidate_args, candidate_const_args)| {
                candidate_const_args == &trait_const_args
                    && candidate_args.len() == trait_args.len()
                    && candidate_args.iter().zip(&trait_args).all(|(left, right)| {
                        self.types_equivalent_without_projection_resolution(*left, *right)
                    })
            })
        {
            return;
        }
        candidates.push((trait_args, trait_const_args));
    }

    fn trait_impl_signature_is_visible(
        &mut self,
        impl_signature: &ProgramTraitImplSignature,
    ) -> bool {
        if impl_signature.module_id == self.defs.module_id {
            return true;
        }
        self.extensions
            .has_trait_witness_impl(impl_signature.module_id, impl_signature.impl_id)
    }

    pub(crate) fn where_predicates_can_hold(
        &mut self,
        predicates: &[WherePredicateSignature],
        substitutions: &HashMap<String, InternedTyId>,
    ) -> bool {
        self.where_predicates_can_hold_with_consts(predicates, substitutions, &HashMap::new())
    }

    pub(crate) fn where_predicates_can_hold_with_consts(
        &mut self,
        predicates: &[WherePredicateSignature],
        substitutions: &HashMap<String, InternedTyId>,
        const_substitutions: &HashMap<String, ConstGenericArg>,
    ) -> bool {
        self.profile_stage("body_check.profile.where_predicates.can_hold", |this| {
            predicates.iter().all(|predicate| {
                let predicate = this.substitute_where_predicate_with_consts(
                    predicate,
                    substitutions,
                    const_substitutions,
                );
                if this.type_contains_generic_param(predicate.ty) {
                    return true;
                }
                predicate.bounds.iter().all(|bound| {
                    let bound_ty = this.substitute_generics_and_consts(
                        bound.trait_ty,
                        substitutions,
                        const_substitutions,
                    );
                    if this.type_contains_generic_param(bound_ty) {
                        return true;
                    }
                    let Some((trait_id, trait_args, trait_const_args)) =
                        this.trait_id_and_args(bound_ty)
                    else {
                        return false;
                    };
                    this.current_context_proves_trait_obligation_with_const_args(
                        predicate.ty,
                        trait_id,
                        trait_args,
                        trait_const_args,
                    )
                })
            })
        })
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
                | TyKind::VolatilePointer { elem, .. }
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
            Some(TyKind::Nominal {
                def_id,
                args,
                const_args,
            }) => {
                for arg in &args {
                    self.check_type_projection_obligations(span, *arg, obligations);
                }
                for arg in &const_args {
                    self.check_type_projection_obligations(span, arg.ty, obligations);
                }
                self.check_nominal_where_obligations(span, def_id, &args, &const_args, obligations);
            }
            Some(TyKind::BuiltinTrait { args, .. }) => {
                for arg in args {
                    self.check_type_projection_obligations(span, arg, obligations);
                }
            }
            Some(TyKind::TraitObject {
                trait_id,
                trait_args,
                trait_const_args,
                associated_type_bindings,
                ..
            })
            | Some(TyKind::TraitObjectPointee {
                trait_id,
                trait_args,
                trait_const_args,
                associated_type_bindings,
                ..
            }) => {
                for arg in &trait_args {
                    self.check_type_projection_obligations(span, *arg, obligations);
                }
                for arg in &trait_const_args {
                    self.check_type_projection_obligations(span, arg.ty, obligations);
                }
                if let TraitId::Source(def_id) = trait_id {
                    self.check_nominal_where_obligations(
                        span,
                        def_id,
                        &trait_args,
                        &trait_const_args,
                        obligations,
                    );
                }
                for binding in associated_type_bindings {
                    for arg in binding.trait_args {
                        self.check_type_projection_obligations(span, arg, obligations);
                    }
                    for arg in binding.trait_const_args {
                        self.check_type_projection_obligations(span, arg.ty, obligations);
                    }
                    self.check_type_projection_obligations(span, binding.ty, obligations);
                }
            }
            Some(TyKind::Projection {
                self_ty,
                trait_id,
                trait_args,
                trait_const_args,
                ..
            }) => {
                self.check_type_projection_obligations(span, self_ty, obligations);
                for arg in &trait_args {
                    self.check_type_projection_obligations(span, *arg, obligations);
                }
                for arg in &trait_const_args {
                    self.check_type_projection_obligations(span, arg.ty, obligations);
                }
                let required = TraitObligation {
                    self_ty,
                    trait_id,
                    trait_args,
                    trait_const_args,
                    associated_type_bindings: Vec::new(),
                };
                if !self.proves_trait_obligation(obligations, &required) {
                    self.diagnostics
                        .push(nia_diagnostic::Diagnostic::user_error_at(
                            nia_diagnostic::codes::TYPE_CHECK,
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
        const_args: &[ConstGenericArg],
        obligations: &[TraitObligation],
    ) {
        let Some(predicates) = self
            .resolved_struct_signature(def_id)
            .map(|resolved| resolved.signature.where_predicates)
            .or_else(|| {
                self.resolved_union_signature(def_id)
                    .map(|resolved| resolved.signature.where_predicates)
            })
            .or_else(|| {
                self.resolved_trait_signature(def_id)
                    .map(|signature| signature.where_predicates)
            })
        else {
            return;
        };
        let (substitutions, const_substitutions) =
            self.generic_substitutions_and_consts_for_def(def_id, args, const_args);
        let predicates = predicates
            .iter()
            .map(|predicate| {
                self.substitute_where_predicate_with_consts(
                    predicate,
                    &substitutions,
                    &const_substitutions,
                )
            })
            .collect::<Vec<_>>();
        for predicate in predicates {
            for bound in predicate.bounds {
                let Some((trait_id, trait_args, trait_const_args)) =
                    self.trait_id_and_args(bound.trait_ty)
                else {
                    continue;
                };
                let required = TraitObligation {
                    self_ty: predicate.ty,
                    trait_id,
                    trait_args,
                    trait_const_args,
                    associated_type_bindings: Vec::new(),
                };
                if !self.proves_trait_obligation(obligations, &required) {
                    self.diagnostics
                        .push(nia_diagnostic::Diagnostic::user_error_at(
                            nia_diagnostic::codes::TYPE_CHECK,
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
        let module_id = self.defs.module_id;
        let local_array_lengths = self.comptime.array_lengths;
        let program_array_lengths = self.program_comptime_array_lengths;
        let const_expr_value = move |id: nia_ids::GlobalConstExprId| {
            if id.module_id == module_id {
                return local_array_lengths.get(&id).copied();
            }
            program_array_lengths(id.module_id)
                .and_then(|array_lengths| array_lengths.values.get(&id).copied())
        };
        let context = TraitSolverContext {
            normalization: self.normalization,
            trait_impls: self.program_trait_impls,
            layouts: Some(self.layouts),
            local_module_id: self.defs.module_id,
            local_enums: &self.signatures.enums,
            program_enums: Some(self.program_enums),
            const_expr_value: Some(&const_expr_value),
            impl_is_visible: Some(&|module_id, impl_id| {
                module_id == self.defs.module_id
                    || self.extensions.has_trait_witness_impl(module_id, impl_id)
            }),
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
            trait_const_args: required.trait_const_args.clone(),
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
                            trait_const_args: obligation.trait_const_args.clone(),
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
                Some(TyKind::VolatilePointer {
                    is_readonly: left_const,
                    elem: left_elem,
                }),
                Some(TyKind::VolatilePointer {
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
                    const_args: left_const_args,
                }),
                Some(TyKind::Nominal {
                    def_id: right_def,
                    args: right_args,
                    const_args: right_const_args,
                }),
            ) => {
                left_def == right_def
                    && left_const_args == right_const_args
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
                    ..
                }),
                Some(TyKind::Projection {
                    self_ty: right_self,
                    trait_id: right_trait,
                    trait_args: right_args,
                    name: right_name,
                    ..
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
