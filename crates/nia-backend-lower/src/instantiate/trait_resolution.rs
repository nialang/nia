// SPDX-License-Identifier: GPL-3.0-or-later
use super::*;
use crate::{BuiltinTraitGoalKey, ExtensionTraitMethodCandidate, ExtensionTraitMethodKey};

impl<'a> ModuleLowerer<'a> {
    pub(super) fn current_impl_trait_method(
        &mut self,
        key: &ExtensionTraitMethodKey,
        trait_args: &[InternedTyId],
        self_ty: InternedTyId,
    ) -> Option<(GlobalDefId, Vec<InternedTyId>)> {
        let current = self.current_instantiated_function?;
        if current.module_id != self.input.module_id {
            return None;
        }
        let current_impl_index = *self.trait_impls_by_method.get(&current)?;
        let impl_signature = self.input.trait_impls.get(current_impl_index)?;
        if impl_signature.trait_id != key.trait_id {
            return None;
        }
        let impl_interner = impl_signature.interner.clone();
        let target_ty =
            nia_ty::import_type_into(&mut self.interner, &impl_interner, impl_signature.target_ty);
        let mut substitutions = std::collections::HashMap::new();
        if !self.match_extension_type_pattern(target_ty, self_ty, &mut substitutions) {
            return None;
        }
        if impl_signature.trait_args.len() != trait_args.len() {
            return None;
        }
        let impl_trait_args = impl_signature
            .trait_args
            .iter()
            .map(|arg| nia_ty::import_type_into(&mut self.interner, &impl_interner, *arg))
            .collect::<Vec<_>>();
        if !impl_trait_args.iter().zip(trait_args).all(|(left, right)| {
            self.match_extension_type_pattern(*left, *right, &mut substitutions)
        }) {
            return None;
        }
        self.input
            .extensions
            .targets()
            .iter()
            .flat_map(|target| target.methods.iter())
            .find(|method| {
                method.is_trait_witness
                    && self
                        .trait_impls_by_method
                        .get(&method.def_id)
                        .is_some_and(|impl_index| *impl_index == current_impl_index)
                    && method.trait_id == Some(key.trait_id)
                    && method.name == key.method_name
                    && method.trait_args.len() == key.trait_arg_count
            })
            .map(|method| {
                let args = method
                    .impl_generics
                    .iter()
                    .filter_map(|generic| substitutions.get(generic).copied())
                    .collect();
                (method.def_id, args)
            })
    }

    pub(super) fn trait_impl_method_for_candidate(
        &mut self,
        candidate: &ExtensionTraitMethodCandidate,
        trait_args: &[InternedTyId],
        self_ty: InternedTyId,
    ) -> Option<(GlobalDefId, Vec<InternedTyId>)> {
        let substitutions =
            self.match_extension_trait_impl_candidate(candidate, trait_args, self_ty)?;
        let args = self
            .candidate_impl_generics(candidate)
            .iter()
            .filter_map(|generic| substitutions.get(generic).copied())
            .collect::<Vec<_>>();
        Some((candidate.method_def_id, args))
    }

    pub(super) fn candidate_where_predicates_hold(
        &mut self,
        candidate: &ExtensionTraitMethodCandidate,
        substitutions: &std::collections::HashMap<String, InternedTyId>,
    ) -> bool {
        let predicates =
            self.import_where_predicates(&candidate.where_predicates, &candidate.source_interner);
        let predicates = predicates
            .iter()
            .map(|predicate| self.substitute_where_predicate(predicate, substitutions))
            .collect::<Vec<_>>();
        self.where_predicates_hold(&predicates)
    }

    fn where_predicates_hold(
        &mut self,
        predicates: &[nia_item_signatures::WherePredicateSignature],
    ) -> bool {
        let mut checks = Vec::new();
        for predicate in predicates {
            for bound in &predicate.bounds {
                let Some((trait_id, trait_args)) = self.trait_id_and_args(bound.trait_ty) else {
                    return false;
                };
                checks.push((
                    predicate.ty,
                    trait_id,
                    trait_args,
                    bound.associated_type_bindings.clone(),
                ));
            }
        }
        let assumptions = self.current_trait_assumptions();
        let associated_type_assumptions =
            self.current_associated_type_assumptions_without_active_projections();
        let context = TraitSolverContext {
            normalization: self.input.type_normalization,
            trait_impls: self.input.trait_impls,
            layouts: Some(self.input.layouts),
            local_module_id: self.input.module_id,
            local_enums: &self.input.signatures.enums,
            program_enums: Some(self.input.program_enums),
        };
        let mut solver = context.solver_with_associated_type_assumptions(
            &mut self.interner,
            &assumptions,
            &associated_type_assumptions,
        );
        for (self_ty, trait_id, trait_args, associated_type_bindings) in checks {
            if !solver.proves(TraitGoal {
                self_ty,
                trait_id,
                trait_args: trait_args.clone(),
            }) {
                return false;
            }
            for binding in associated_type_bindings {
                let Some(actual_ty) =
                    solver.resolve_associated_type(self_ty, trait_id, &trait_args, &binding.name)
                else {
                    return false;
                };
                if !solver.types_equivalent(actual_ty, binding.ty) {
                    return false;
                }
            }
        }
        true
    }

    pub(super) fn current_trait_assumptions(&mut self) -> Vec<TraitGoal> {
        let Some(current) = self.current_instantiated_function else {
            return Vec::new();
        };
        let Some(substitutions) = self.current_type_substitutions else {
            return Vec::new();
        };
        let mut assumptions = self.current_method_owner_trait_assumptions(current);
        let Some((predicates, source_interner)) = self.current_function_where_predicates(current)
        else {
            return assumptions;
        };
        let predicates = self.import_where_predicates(&predicates, &source_interner);
        let substitutions = self
            .type_substitutions
            .get(substitutions.0)
            .cloned()
            .unwrap_or_default();
        let predicates = predicates
            .iter()
            .map(|predicate| self.substitute_where_predicate(predicate, &substitutions))
            .collect::<Vec<_>>();
        for predicate in predicates {
            for bound in predicate.bounds {
                if let Some((trait_id, trait_args)) = Self::trait_id_and_args_from(
                    &self.interner,
                    self.input.type_normalization,
                    self.input.program_traits,
                    bound.trait_ty,
                ) {
                    assumptions.push(TraitGoal {
                        self_ty: predicate.ty,
                        trait_id,
                        trait_args,
                    });
                }
            }
        }
        assumptions
    }

    fn current_method_owner_trait_assumptions(&mut self, current: GlobalDefId) -> Vec<TraitGoal> {
        if let Some(goal) = self.current_trait_method_owner_assumption(current) {
            return vec![goal];
        }
        Vec::new()
    }

    fn current_trait_method_owner_assumption(&mut self, current: GlobalDefId) -> Option<TraitGoal> {
        let (trait_def_id, generics) = if current.module_id == self.input.module_id
            && let Some(def) = self.input.defs.defs.get(current.def_id)
            && def.kind == nia_defs::DefKind::TraitMethod
        {
            let trait_def_id = GlobalDefId {
                module_id: current.module_id,
                def_id: def.parent?,
            };
            let trait_signature = self.input.signatures.traits.get(&trait_def_id.def_id)?;
            (trait_def_id, trait_signature.generics.clone())
        } else {
            self.input
                .program_traits
                .iter()
                .find_map(|(trait_id, signature)| {
                    signature
                        .signature
                        .methods
                        .iter()
                        .any(|method| {
                            GlobalDefId {
                                module_id: trait_id.module_id,
                                def_id: method.def_id,
                            } == current
                        })
                        .then(|| (*trait_id, signature.signature.generics.clone()))
                })?
        };
        let trait_args = generics
            .iter()
            .map(|generic| self.interner.intern(TyKind::GenericParam(generic.clone())))
            .collect();
        Some(TraitGoal {
            self_ty: self
                .interner
                .intern(TyKind::GenericParam("Self".to_string())),
            trait_id: TraitId::Source(trait_def_id),
            trait_args,
        })
    }

    fn current_function_where_predicates(
        &self,
        current: GlobalDefId,
    ) -> Option<(
        Vec<nia_item_signatures::WherePredicateSignature>,
        nia_ty::TyInterner,
    )> {
        if current.module_id == self.input.module_id
            && let Some(signature) = self.input.signatures.functions.get(&current.def_id)
        {
            return Some((
                signature.where_predicates.clone(),
                self.input.body_ir.interner.clone(),
            ));
        }
        self.input.program_functions.get(&current).map(|signature| {
            (
                signature.signature.where_predicates.clone(),
                signature.interner.clone(),
            )
        })
    }

    fn trait_id_and_args(&self, ty: InternedTyId) -> Option<(TraitId, Vec<InternedTyId>)> {
        Self::trait_id_and_args_from(
            &self.interner,
            self.input.type_normalization,
            self.input.program_traits,
            ty,
        )
    }

    fn trait_id_and_args_from(
        interner: &nia_ty::TyInterner,
        normalization: &nia_type_normalize::TypeNormalization,
        program_traits: &std::collections::HashMap<
            GlobalDefId,
            nia_item_signatures::ProgramTraitSignature,
        >,
        ty: InternedTyId,
    ) -> Option<(TraitId, Vec<InternedTyId>)> {
        match interner.get(normalization.normalize(ty)) {
            Some(TyKind::Nominal { def_id, args }) if program_traits.contains_key(def_id) => {
                Some((TraitId::Source(*def_id), args.clone()))
            }
            Some(TyKind::BuiltinTrait { trait_id, args }) => {
                Some((TraitId::Builtin(*trait_id), args.clone()))
            }
            _ => None,
        }
    }

    pub(super) fn resolve_builtin_place_method_impl(
        &mut self,
        trait_id: BuiltinTrait,
        trait_args: &[InternedTyId],
        method: BuiltinTraitMethod,
        self_ty: InternedTyId,
    ) -> Option<(GlobalDefId, Vec<InternedTyId>)> {
        let key = ExtensionTraitMethodKey {
            trait_id: TraitId::Builtin(trait_id),
            method_name: method.name().to_string(),
            trait_arg_count: trait_args.len(),
        };
        let candidates = self.extension_trait_method_candidates(&key);
        let candidates = candidates
            .iter()
            .filter_map(|candidate| {
                self.builtin_place_impl_method_for_candidate(candidate, trait_args, self_ty)
            })
            .collect::<Vec<_>>();
        match candidates.as_slice() {
            [candidate] => Some(candidate.clone()),
            _ => None,
        }
    }

    pub(super) fn builtin_place_impl_method_for_candidate(
        &mut self,
        candidate: &ExtensionTraitMethodCandidate,
        trait_args: &[InternedTyId],
        self_ty: InternedTyId,
    ) -> Option<(GlobalDefId, Vec<InternedTyId>)> {
        let substitutions =
            self.match_extension_trait_impl_candidate(candidate, trait_args, self_ty)?;
        let args = self
            .candidate_impl_generics(candidate)
            .iter()
            .filter_map(|generic| substitutions.get(generic).copied())
            .collect::<Vec<_>>();
        Some((candidate.method_def_id, args))
    }

    pub(super) fn lower_intrinsic_builtin_place_method_call(
        &mut self,
        trait_id: BuiltinTrait,
        method: BuiltinTraitMethod,
        self_ty: InternedTyId,
        trait_args: &[InternedTyId],
        receiver: FunctionExpr,
        args: &[FunctionExpr],
    ) -> Option<FunctionExpr> {
        let receiver_span = receiver.span;
        match (trait_id, method, trait_args, args) {
            (
                BuiltinTrait::DerefRead | BuiltinTrait::Deref,
                BuiltinTraitMethod::DerefRead | BuiltinTraitMethod::Deref,
                [],
                [],
            ) => {
                if !matches!(
                    self.resolve_builtin_trait_goal(self_ty, trait_id, Vec::new()),
                    TraitResolution::Intrinsic(_)
                ) {
                    return None;
                }
                let elem = self.pointer_elem_ty(self_ty)?;
                let receiver_ptr = FunctionExpr {
                    span: receiver_span,
                    ty: self_ty,
                    kind: FunctionExprKind::Unary {
                        op: nia_ast::UnaryOp::Deref,
                        expr: Box::new(receiver),
                    },
                };
                Some(FunctionExpr {
                    span: receiver_ptr.span,
                    ty: self.interner.intern(TyKind::Pointer {
                        is_readonly: matches!(trait_id, BuiltinTrait::DerefRead),
                        elem,
                    }),
                    kind: FunctionExprKind::AddrOf(FunctionPlace {
                        span: receiver_ptr.span,
                        ty: elem,
                        base: FunctionPlaceBase::Deref(Box::new(receiver_ptr)),
                        elems: Vec::new(),
                    }),
                })
            }
            (
                BuiltinTrait::IndexRead | BuiltinTrait::Index,
                BuiltinTraitMethod::IndexRead | BuiltinTraitMethod::Index,
                [index_ty],
                [index],
            ) => {
                if !matches!(
                    self.resolve_builtin_trait_goal(self_ty, trait_id, vec![*index_ty]),
                    TraitResolution::Intrinsic(_)
                ) {
                    return None;
                }
                let elem = self.index_elem_ty(self_ty)?;
                let base = FunctionExpr {
                    span: receiver_span,
                    ty: self_ty,
                    kind: FunctionExprKind::Unary {
                        op: nia_ast::UnaryOp::Deref,
                        expr: Box::new(receiver),
                    },
                };
                Some(FunctionExpr {
                    span: index.span,
                    ty: self.interner.intern(TyKind::Pointer {
                        is_readonly: matches!(trait_id, BuiltinTrait::IndexRead),
                        elem,
                    }),
                    kind: FunctionExprKind::AddrOf(FunctionPlace {
                        span: index.span,
                        ty: elem,
                        base: FunctionPlaceBase::Deref(Box::new(base)),
                        elems: vec![FunctionPlaceElem::Index(Box::new(index.clone()))],
                    }),
                })
            }
            (
                BuiltinTrait::SliceRead | BuiltinTrait::Slice,
                BuiltinTraitMethod::SliceRead | BuiltinTraitMethod::Slice,
                [_range_ty],
                [range],
            ) => {
                if !matches!(
                    self.resolve_builtin_trait_goal(self_ty, trait_id, trait_args.to_vec()),
                    TraitResolution::Intrinsic(_)
                ) {
                    return None;
                }
                let base = FunctionExpr {
                    span: receiver_span,
                    ty: self_ty,
                    kind: FunctionExprKind::Unary {
                        op: nia_ast::UnaryOp::Deref,
                        expr: Box::new(receiver),
                    },
                };
                let substitutions = self.empty_type_substitution_id();
                let mut active_projections = std::collections::HashSet::new();
                Some(FunctionExpr {
                    span: range.span,
                    ty: self.resolve_associated_type_projection(
                        self_ty,
                        TraitId::Builtin(trait_id),
                        trait_args,
                        BuiltinTrait::OUTPUT_ASSOC_TYPE,
                        substitutions,
                        &mut active_projections,
                    )?,
                    kind: FunctionExprKind::Slice {
                        lhs: Box::new(base),
                        range: self.range_expr_to_slice_range(range)?,
                        is_readonly: matches!(trait_id, BuiltinTrait::SliceRead),
                    },
                })
            }
            _ => None,
        }
    }

    pub(super) fn resolve_builtin_trait_goal(
        &mut self,
        self_ty: InternedTyId,
        trait_id: BuiltinTrait,
        trait_args: Vec<InternedTyId>,
    ) -> TraitResolution {
        let key = BuiltinTraitGoalKey {
            self_ty,
            trait_id,
            trait_args,
        };
        if let Some(resolution) = self.builtin_trait_resolutions.get(&key) {
            return resolution.clone();
        }
        let context = TraitSolverContext {
            normalization: self.input.type_normalization,
            trait_impls: self.input.trait_impls,
            layouts: Some(self.input.layouts),
            local_module_id: self.input.module_id,
            local_enums: &self.input.signatures.enums,
            program_enums: Some(self.input.program_enums),
        };
        let assumptions = self.current_trait_assumptions();
        let mut solver = context.solver(&mut self.interner, &assumptions);
        let resolution = solver.resolve(TraitGoal {
            self_ty: key.self_ty,
            trait_id: TraitId::Builtin(key.trait_id),
            trait_args: key.trait_args.clone(),
        });
        self.builtin_trait_resolutions
            .insert(key, resolution.clone());
        resolution
    }

    pub(super) fn extension_trait_method_candidates(
        &self,
        key: &ExtensionTraitMethodKey,
    ) -> Vec<ExtensionTraitMethodCandidate> {
        let mut out = self
            .extension_trait_method_candidates
            .get(key)
            .cloned()
            .unwrap_or_default();
        if let Some((_, instance_candidates)) = &self.instance_extension_trait_method_candidates {
            for candidate in instance_candidates.get(key).cloned().unwrap_or_default() {
                if !out
                    .iter()
                    .any(|existing| existing.method_def_id == candidate.method_def_id)
                {
                    out.push(candidate);
                }
            }
        }
        out
    }

    pub(super) fn program_extension_trait_method_candidates(
        &self,
        key: &ExtensionTraitMethodKey,
    ) -> Vec<ExtensionTraitMethodCandidate> {
        let candidates = self
            .program_extension_trait_method_candidates
            .get(key)
            .cloned()
            .unwrap_or_default();
        if candidates.is_empty() {
            return self.extension_trait_method_candidates(key);
        }
        candidates
    }

    pub(super) fn pointer_elem_ty(&self, ty: InternedTyId) -> Option<InternedTyId> {
        match self.ty_kind(ty) {
            Some(TyKind::Pointer { elem, .. }) => Some(*elem),
            _ => None,
        }
    }

    pub(super) fn index_elem_ty(&self, ty: InternedTyId) -> Option<InternedTyId> {
        match self.ty_kind(ty) {
            Some(TyKind::Array { elem, .. })
            | Some(TyKind::Pointer { elem, .. })
            | Some(TyKind::Slice { elem, .. }) => Some(*elem),
            _ => None,
        }
    }

    pub(super) fn range_expr_to_slice_range(
        &self,
        range: &FunctionExpr,
    ) -> Option<FunctionSliceRange> {
        match &range.kind {
            FunctionExprKind::Range(range) => Some(FunctionSliceRange {
                start: range.start.clone(),
                end: range.end.clone(),
                inclusive: range.inclusive,
            }),
            _ => None,
        }
    }
}
