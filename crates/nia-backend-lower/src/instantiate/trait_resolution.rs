// SPDX-License-Identifier: GPL-3.0-or-later
use super::*;
use crate::{BuiltinTraitGoalKey, ExtensionTraitMethodCandidate, ExtensionTraitMethodKey};
use nia_ids::BuiltinAssociatedType;
use nia_symbol::ToSymbolId;
use nia_trait_solve::TraitSelection;

impl<'a> ModuleLowerer<'a> {
    pub(super) fn current_impl_trait_method(
        &mut self,
        key: &ExtensionTraitMethodKey,
        trait_args: &[InternedTyId],
        self_ty: InternedTyId,
    ) -> Option<(GlobalDefId, Vec<InternedTyId>)> {
        let current = self.instantiation.function?;
        if current.module_id != self.input.module_id {
            return None;
        }
        let current_impl_index = self.trait_impl_index_for_method(current)?;
        let impl_signature = self.input.trait_impls.get(current_impl_index)?;
        if impl_signature.trait_id != key.trait_id {
            return None;
        }
        let impl_interner = impl_signature.interner.clone();
        let target_ty =
            self.import_type_from_known_interner(&impl_interner, impl_signature.target_ty);
        let mut substitutions = SymbolMap::default();
        if !self.match_extension_type_pattern(target_ty, self_ty, &mut substitutions) {
            return None;
        }
        if impl_signature.trait_args.len() != trait_args.len() {
            return None;
        }
        let impl_trait_args = impl_signature
            .trait_args
            .iter()
            .map(|arg| self.import_type_from_known_interner(&impl_interner, *arg))
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
                        .trait_impl_index_for_method(method.def_id)
                        .is_some_and(|impl_index| impl_index == current_impl_index)
                    && method.trait_id == Some(key.trait_id)
                    && method.name == key.method_name
                    && method.trait_args.len() == key.trait_arg_count
            })
            .and_then(|method| {
                let args = method
                    .effective_generics
                    .iter()
                    .map(|generic| substitutions.get(generic).copied())
                    .collect::<Option<Vec<_>>>()?;
                Some((method.def_id, args))
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
            .map(|generic| substitutions.get(generic).copied())
            .collect::<Option<Vec<_>>>()?;
        Some((candidate.method_def_id, args))
    }

    pub(super) fn global_trait_method_impl_candidate(
        &mut self,
        key: &ExtensionTraitMethodKey,
        trait_args: &[InternedTyId],
        self_ty: InternedTyId,
    ) -> Option<(GlobalDefId, Vec<InternedTyId>)> {
        let mut matches = Vec::new();
        for method in self.input.program_extension_methods.all_methods() {
            if method.trait_id != Some(key.trait_id)
                || method.name != key.method_name
                || method.trait_args.len() != key.trait_arg_count
            {
                continue;
            }
            let Some(impl_signature) = self.input.trait_impls.iter().find(|impl_signature| {
                impl_signature.module_id == method.def_id.module_id
                    && impl_signature.impl_id == method.impl_id
            }) else {
                continue;
            };
            let candidate = ExtensionTraitMethodCandidate {
                target_ty: impl_signature.target_ty,
                method_def_id: method.def_id,
                trait_args: impl_signature.trait_args.clone(),
                where_predicates: impl_signature.where_predicates.clone(),
                effective_generics: impl_signature.generics.clone(),
                interner: std::sync::Arc::new(impl_signature.interner.clone()),
            };
            if let Some(resolved) =
                self.trait_impl_method_for_candidate(&candidate, trait_args, self_ty)
            {
                matches.push((candidate, resolved));
            }
        }
        matches.sort_by_key(|(_, resolved)| resolved.clone());
        matches.dedup_by(|(_, left), (_, right)| left == right);
        match matches.as_slice() {
            [(_, resolved)] => Some(resolved.clone()),
            _ => None,
        }
    }

    pub(super) fn selected_user_trait_method_impl(
        &mut self,
        key: &ExtensionTraitMethodKey,
        trait_args: &[InternedTyId],
        self_ty: InternedTyId,
    ) -> Option<(GlobalDefId, Vec<InternedTyId>)> {
        let program_is_enum = |def_id| self.input.program_enums.contains_key(&def_id);
        let context = TraitSolverContext {
            normalization: self.input.type_normalization,
            trait_impls: self.input.trait_impls,
            trait_impl_index: Some(self.input.trait_impl_index),
            layouts: Some(self.input.layouts),
            local_module_id: self.input.module_id,
            local_enums: &self.input.signatures.enums,
            program_is_enum: Some(&program_is_enum),
            const_expr_value: None,
            impl_is_visible: None,
        };
        let mut solver = context.solver(&mut self.type_context.interner, &[]);
        let TraitSelection::User(user_impl) = solver.select_user_impl(TraitGoal {
            self_ty,
            trait_id: key.trait_id,
            trait_args: trait_args.to_vec(),
            trait_const_args: Vec::new(),
        }) else {
            return None;
        };
        let impl_signature = self.input.trait_impls.get(user_impl.impl_index)?;
        for method in self.input.program_extension_methods.all_methods() {
            if method.def_id.module_id != impl_signature.module_id
                || method.impl_id != impl_signature.impl_id
                || method.trait_id != Some(key.trait_id)
                || method.name != key.method_name
                || method.trait_args.len() != key.trait_arg_count
            {
                continue;
            }
            let args = method
                .effective_generics
                .iter()
                .map(|generic| user_impl.substitutions.get(generic).copied())
                .collect::<Option<Vec<_>>>()?;
            return Some((method.def_id, args));
        }
        for target in self.input.extensions.targets() {
            for method in &target.methods {
                if !method.is_trait_witness
                    || method.def_id.module_id != impl_signature.module_id
                    || method.impl_id != impl_signature.impl_id
                    || method.trait_id != Some(key.trait_id)
                    || method.name != key.method_name
                    || method.trait_args.len() != key.trait_arg_count
                {
                    continue;
                }
                let args = method
                    .effective_generics
                    .iter()
                    .map(|generic| user_impl.substitutions.get(generic).copied())
                    .collect::<Option<Vec<_>>>()?;
                return Some((method.def_id, args));
            }
        }
        None
    }

    pub(super) fn candidate_where_predicates_hold(
        &mut self,
        candidate: &ExtensionTraitMethodCandidate,
        substitutions: &SymbolMap<InternedTyId>,
    ) -> bool {
        let candidate_interner = self.candidate_type_interner(candidate).clone();
        let predicates =
            self.import_where_predicates(&candidate.where_predicates, &candidate_interner);
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
                let Some((trait_id, trait_args, trait_const_args)) =
                    self.trait_id_and_args(bound.trait_ty)
                else {
                    return false;
                };
                checks.push((
                    predicate.ty,
                    trait_id,
                    trait_args,
                    trait_const_args,
                    bound.associated_type_bindings.clone(),
                ));
            }
        }
        let assumptions = self.current_trait_assumptions();
        let associated_type_assumptions =
            self.current_associated_type_assumptions_without_active_projections();
        let program_is_enum = |def_id| self.input.program_enums.contains_key(&def_id);
        let context = TraitSolverContext {
            normalization: self.input.type_normalization,
            trait_impls: self.input.trait_impls,
            trait_impl_index: Some(self.input.trait_impl_index),
            layouts: Some(self.input.layouts),
            local_module_id: self.input.module_id,
            local_enums: &self.input.signatures.enums,
            program_is_enum: Some(&program_is_enum),
            const_expr_value: None,
            impl_is_visible: None,
        };
        let mut solver = context.solver_with_associated_type_assumptions(
            &mut self.type_context.interner,
            &assumptions,
            &associated_type_assumptions,
        );
        for (self_ty, trait_id, trait_args, trait_const_args, associated_type_bindings) in checks {
            if !solver.proves(TraitGoal {
                self_ty,
                trait_id,
                trait_args: trait_args.clone(),
                trait_const_args: trait_const_args.clone(),
            }) {
                return false;
            }
            for binding in associated_type_bindings {
                let Some(actual_ty) = solver.resolve_associated_type(
                    self_ty,
                    trait_id,
                    &trait_args,
                    &trait_const_args,
                    &binding.name,
                ) else {
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
        let Some(current) = self.instantiation.function else {
            return Vec::new();
        };
        let Some(substitutions) = self.instantiation.type_substitutions else {
            return Vec::new();
        };
        let mut assumptions = self.current_method_owner_trait_assumptions(current);
        let substitutions = self
            .type_context
            .type_substitutions(substitutions)
            .cloned()
            .unwrap_or_default();
        for (predicates, source_interner) in self.current_where_predicate_sources(current) {
            let predicates = self.import_where_predicates(&predicates, &source_interner);
            let predicates = predicates
                .iter()
                .map(|predicate| self.substitute_where_predicate(predicate, &substitutions))
                .collect::<Vec<_>>();
            for predicate in predicates {
                for bound in predicate.bounds {
                    if let Some((trait_id, trait_args, trait_const_args)) =
                        Self::trait_id_and_args_from(
                            &self.type_context.interner,
                            self.input.type_normalization,
                            self.input.program_traits,
                            bound.trait_ty,
                        )
                    {
                        assumptions.push(TraitGoal {
                            self_ty: predicate.ty,
                            trait_id,
                            trait_args,
                            trait_const_args,
                        });
                    }
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
            .map(|generic| {
                self.type_context
                    .interner
                    .intern(TyKind::GenericParam(*generic))
            })
            .collect();
        Some(TraitGoal {
            self_ty: self.type_context.interner.intern(TyKind::SelfParam),
            trait_id: TraitId::Source(trait_def_id),
            trait_args,
            trait_const_args: Vec::new(),
        })
    }

    fn current_where_predicate_sources(
        &mut self,
        current: GlobalDefId,
    ) -> Vec<(
        Vec<nia_item_signatures::WherePredicateSignature>,
        nia_ty::TyInterner,
    )> {
        let mut sources = Vec::new();
        if let Some(source) = self.current_extension_where_predicates(current) {
            sources.push(source);
        }
        if let Some(source) = self.current_extension_owner_where_predicates(current) {
            sources.push(source);
        }
        if current.module_id == self.input.module_id
            && let Some(signature) = self.input.signatures.functions.get(&current.def_id)
        {
            sources.push((
                signature.where_predicates.clone(),
                self.input.type_lowering.interner.clone(),
            ));
            return sources;
        }
        if let Some(signature) = self.input.program_functions.get(&current) {
            sources.push((
                signature.signature.where_predicates.clone(),
                signature.interner.clone(),
            ));
        }
        sources
    }

    fn current_extension_owner_where_predicates(
        &mut self,
        current: GlobalDefId,
    ) -> Option<(
        Vec<nia_item_signatures::WherePredicateSignature>,
        nia_ty::TyInterner,
    )> {
        let source = self.extension_method_source(current)?;
        let interner = source.interner.clone();
        let (target_interner, target_ty) = self
            .normalized_program_type_source_for_module(
                interner.interner_id().module_id(),
                &interner,
                source.target_ty,
            )
            .unwrap_or_else(|| (interner.clone(), source.target_ty));
        let Some(TyKind::Nominal { def_id, args, .. }) = target_interner.get(target_ty).cloned()
        else {
            return None;
        };
        let args = args
            .iter()
            .map(|arg| self.import_type_from_known_interner(&target_interner, *arg))
            .collect::<Vec<_>>();
        let predicates = if def_id.module_id == self.input.module_id {
            self.input
                .signatures
                .structs
                .get(&def_id.def_id)
                .map(|signature| {
                    (
                        signature.generics.clone(),
                        signature.where_predicates.clone(),
                        self.input.type_lowering.interner.clone(),
                    )
                })
                .or_else(|| {
                    self.input
                        .signatures
                        .unions
                        .get(&def_id.def_id)
                        .map(|signature| {
                            (
                                signature.generics.clone(),
                                signature.where_predicates.clone(),
                                self.input.type_lowering.interner.clone(),
                            )
                        })
                })?
        } else {
            self.input
                .program_structs
                .get(&def_id)
                .map(|signature| {
                    (
                        signature.signature.generics.clone(),
                        signature.signature.where_predicates.clone(),
                        signature.interner.clone(),
                    )
                })
                .or_else(|| {
                    self.input.program_unions.get(&def_id).map(|signature| {
                        (
                            signature.signature.generics.clone(),
                            signature.signature.where_predicates.clone(),
                            signature.interner.clone(),
                        )
                    })
                })?
        };
        let substitutions = predicates
            .0
            .iter()
            .cloned()
            .zip(args)
            .collect::<SymbolMap<_>>();
        let imported_predicates = self.import_where_predicates(&predicates.1, &predicates.2);
        let predicates = imported_predicates
            .iter()
            .map(|predicate| self.substitute_where_predicate(predicate, &substitutions))
            .collect();
        Some((predicates, self.type_context.interner.clone()))
    }

    fn current_extension_where_predicates(
        &self,
        current: GlobalDefId,
    ) -> Option<(
        Vec<nia_item_signatures::WherePredicateSignature>,
        nia_ty::TyInterner,
    )> {
        if let Some(source) = self.extension_method_source(current) {
            return Some((source.where_predicates.clone(), source.interner.clone()));
        }
        let program_index = self.trait_impl_index_for_method(current)?;
        self.input.trait_impls.get(program_index).map(|signature| {
            (
                signature.where_predicates.clone(),
                signature.interner.clone(),
            )
        })
    }

    fn trait_id_and_args(
        &self,
        ty: InternedTyId,
    ) -> Option<(TraitId, Vec<InternedTyId>, Vec<nia_ty::ConstGenericArg>)> {
        Self::trait_id_and_args_from(
            &self.type_context.interner,
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
    ) -> Option<(TraitId, Vec<InternedTyId>, Vec<nia_ty::ConstGenericArg>)> {
        match interner.get(normalization.normalize(ty)) {
            Some(TyKind::Nominal {
                def_id,
                args,
                const_args,
            }) if program_traits.contains_key(def_id) => {
                Some((TraitId::Source(*def_id), args.clone(), const_args.clone()))
            }
            Some(TyKind::BuiltinTrait { trait_id, args }) => {
                Some((TraitId::Builtin(*trait_id), args.clone(), Vec::new()))
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
            method_name: method.symbol_id(),
            trait_arg_count: trait_args.len(),
        };
        let candidates = self.program_extension_trait_method_candidates(&key);
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
                BuiltinTrait::Deref | BuiltinTrait::DerefMut,
                BuiltinTraitMethod::Deref | BuiltinTraitMethod::DerefMut,
                [],
                [],
            ) => {
                if !self.intrinsic_builtin_trait_goal_exists(self_ty, trait_id, &[]) {
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
                    ty: self.type_context.interner.intern(TyKind::Pointer {
                        is_readonly: matches!(trait_id, BuiltinTrait::Deref),
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
                BuiltinTrait::Index | BuiltinTrait::IndexMut,
                BuiltinTraitMethod::Index | BuiltinTraitMethod::IndexMut,
                [index_ty],
                [index],
            ) => {
                if !self.intrinsic_builtin_trait_goal_exists(self_ty, trait_id, &[*index_ty]) {
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
                    ty: self.type_context.interner.intern(TyKind::Pointer {
                        is_readonly: matches!(trait_id, BuiltinTrait::Index),
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
                BuiltinTrait::Slice | BuiltinTrait::SliceMut,
                BuiltinTraitMethod::Slice | BuiltinTraitMethod::SliceMut,
                [_range_ty],
                [range],
            ) => {
                if !self.intrinsic_builtin_trait_goal_exists(self_ty, trait_id, trait_args) {
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
                let projection = super::ProjectionInstantiationKey {
                    self_ty,
                    trait_id: TraitId::Builtin(trait_id),
                    trait_args: trait_args.to_vec(),
                    trait_const_args: Vec::new(),
                    name: BuiltinAssociatedType::Output.symbol_id(),
                };
                Some(FunctionExpr {
                    span: range.span,
                    ty: self.resolve_associated_type_projection(
                        &projection,
                        substitutions,
                        &mut active_projections,
                    )?,
                    kind: FunctionExprKind::Slice {
                        lhs: Box::new(base),
                        range: self.range_expr_to_slice_range(range)?,
                        is_readonly: matches!(trait_id, BuiltinTrait::Slice),
                    },
                })
            }
            _ => None,
        }
    }

    fn intrinsic_builtin_trait_goal_exists(
        &mut self,
        self_ty: InternedTyId,
        trait_id: BuiltinTrait,
        trait_args: &[InternedTyId],
    ) -> bool {
        let program_is_enum = |def_id| self.input.program_enums.contains_key(&def_id);
        let context = TraitSolverContext {
            normalization: self.input.type_normalization,
            trait_impls: self.input.trait_impls,
            trait_impl_index: Some(self.input.trait_impl_index),
            layouts: Some(self.input.layouts),
            local_module_id: self.input.module_id,
            local_enums: &self.input.signatures.enums,
            program_is_enum: Some(&program_is_enum),
            const_expr_value: None,
            impl_is_visible: None,
        };
        let mut solver = context.solver(&mut self.type_context.interner, &[]);
        matches!(
            solver.resolve(TraitGoal {
                self_ty,
                trait_id: TraitId::Builtin(trait_id),
                trait_args: trait_args.to_vec(),
                trait_const_args: Vec::new(),
            }),
            TraitResolution::Intrinsic(_)
        )
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
        let assumptions = self.current_trait_assumptions();
        if assumptions.is_empty()
            && let Some(resolution) = self.trait_context.builtin_trait_resolutions.get(&key)
        {
            return resolution.clone();
        }
        let program_is_enum = |def_id| self.input.program_enums.contains_key(&def_id);
        let context = TraitSolverContext {
            normalization: self.input.type_normalization,
            trait_impls: self.input.trait_impls,
            trait_impl_index: Some(self.input.trait_impl_index),
            layouts: Some(self.input.layouts),
            local_module_id: self.input.module_id,
            local_enums: &self.input.signatures.enums,
            program_is_enum: Some(&program_is_enum),
            const_expr_value: None,
            impl_is_visible: None,
        };
        let mut solver = context.solver(&mut self.type_context.interner, &assumptions);
        let resolution = solver.resolve(TraitGoal {
            self_ty: key.self_ty,
            trait_id: TraitId::Builtin(key.trait_id),
            trait_args: key.trait_args.clone(),
            trait_const_args: Vec::new(),
        });
        if assumptions.is_empty() {
            self.trait_context
                .builtin_trait_resolutions
                .insert(key, resolution.clone());
        }
        resolution
    }

    pub(crate) fn extension_trait_method_candidates(
        &self,
        key: &ExtensionTraitMethodKey,
    ) -> Vec<ExtensionTraitMethodCandidate> {
        let mut out = self
            .trait_context
            .extension_trait_method_candidates
            .get(key)
            .cloned()
            .unwrap_or_default();
        if let Some((_, instance_candidates)) =
            &self.instantiation.extension_trait_method_candidates
        {
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

    pub(crate) fn program_extension_trait_method_candidates(
        &self,
        key: &ExtensionTraitMethodKey,
    ) -> Vec<ExtensionTraitMethodCandidate> {
        let mut candidates = self
            .shared
            .program_extension_trait_method_candidates
            .get(key)
            .cloned()
            .unwrap_or_default();
        for candidate in self.extension_trait_method_candidates(key) {
            if candidates
                .iter()
                .any(|existing| existing.method_def_id == candidate.method_def_id)
            {
                continue;
            }
            candidates.push(candidate);
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
