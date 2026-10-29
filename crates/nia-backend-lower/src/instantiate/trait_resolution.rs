// SPDX-License-Identifier: GPL-3.0-or-later
use super::*;
use crate::{BuiltinTraitGoalKey, ExtensionTraitMethodCandidate, ExtensionTraitMethodKey};
use nia_ids::BuiltinAssociatedType;
use nia_symbol::ToSymbolId;
use nia_trait_solve::TraitSelection;

impl<'a> ModuleLowerer<'a> {
    pub(super) fn selected_user_trait_method_impl(
        &mut self,
        key: &ExtensionTraitMethodKey,
        trait_args: &[InternedTyId],
        trait_const_args: &[nia_ty::ConstGenericArg],
        self_ty: InternedTyId,
    ) -> Option<(GlobalDefId, Vec<InternedTyId>, Vec<nia_ty::ConstGenericArg>)> {
        let assumptions = self.current_trait_assumptions();
        let program_is_enum = |def_id| self.input.program.enums().contains_key(&def_id);
        let context = TraitSolverContext {
            type_store: self.type_store,
            normalization: self.input.type_normalization,
            trait_impls: self.input.program.trait_impls(),
            trait_impl_index: Some(self.input.program.trait_impl_index()),
            layouts: Some(self.input.layouts),
            local_module_id: self.input.module_id,
            local_enums: &self.input.signatures.enums,
            program_is_enum: Some(&program_is_enum),
            const_expr_value: None,
            impl_is_visible: None,
        };
        let mut solver = context.solver(&assumptions);
        let selection = match solver.select_user_impl(TraitGoal {
            self_ty,
            trait_id: key.trait_id,
            trait_args: trait_args.to_vec(),
            trait_const_args: trait_const_args.to_vec(),
        }) {
            Ok(selection) => selection,
            Err(error) => {
                self.type_context.record_internal(error);
                return None;
            }
        };
        let TraitSelection::User(user_impl) = selection else {
            return None;
        };
        let impl_signature = self.input.program.trait_impls().get(user_impl.impl_index)?;
        // Selection owns applicability and specificity. Materialization only
        // finds the selected impl's witness and preserves its complete binder.
        // Method-owned parameters are appended separately by the call site.
        let def_id = self
            .input
            .program
            .extension_methods()
            .all_methods()
            .find(|method| {
                method.def_id.module_id == impl_signature.module_id
                    && method.impl_id == impl_signature.impl_id
                    && method.trait_id == Some(key.trait_id)
                    && method.name == key.method_name
            })?
            .def_id;
        let mut args = Vec::new();
        let mut const_args = Vec::new();
        for param in &impl_signature.generic_params {
            match param.kind {
                nia_item_signatures::GenericParamSignatureKind::Type => {
                    args.push(*user_impl.substitutions.get(&param.name)?);
                }
                nia_item_signatures::GenericParamSignatureKind::Const { .. } => {
                    const_args.push(self.canonicalize_instance_const_arg(
                        user_impl.const_substitutions.get(&param.name)?,
                    ));
                }
            }
        }
        Some((def_id, args, const_args))
    }

    pub(super) fn candidate_where_predicates_hold(
        &mut self,
        candidate: &ExtensionTraitMethodCandidate,
        substitutions: &SymbolMap<InternedTyId>,
    ) -> bool {
        let predicates =
            self.normalize_where_predicates(&candidate.where_predicates, candidate.module_id);
        let substitutions = self.intern_type_substitutions(substitutions);
        let predicates = predicates
            .iter()
            .map(|predicate| self.instantiate_where_predicate(predicate, substitutions))
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
        let program_is_enum = |def_id| self.input.program.enums().contains_key(&def_id);
        let context = TraitSolverContext {
            type_store: self.type_store,
            normalization: self.input.type_normalization,
            trait_impls: self.input.program.trait_impls(),
            trait_impl_index: Some(self.input.program.trait_impl_index()),
            layouts: Some(self.input.layouts),
            local_module_id: self.input.module_id,
            local_enums: &self.input.signatures.enums,
            program_is_enum: Some(&program_is_enum),
            const_expr_value: None,
            impl_is_visible: None,
        };
        let mut solver = context
            .solver_with_associated_type_assumptions(&assumptions, &associated_type_assumptions);
        for (self_ty, trait_id, trait_args, trait_const_args, associated_type_bindings) in checks {
            let proven = match solver.proves(TraitGoal {
                self_ty,
                trait_id,
                trait_args: trait_args.clone(),
                trait_const_args: trait_const_args.clone(),
            }) {
                Ok(proven) => proven,
                Err(error) => {
                    self.type_context.record_internal(error);
                    return false;
                }
            };
            if !proven {
                return false;
            }
            for binding in associated_type_bindings {
                let actual_ty = match solver.resolve_associated_type(
                    self_ty,
                    trait_id,
                    &trait_args,
                    &trait_const_args,
                    &binding.name,
                ) {
                    Ok(Some(actual_ty)) => actual_ty,
                    Ok(None) => return false,
                    Err(error) => {
                        self.type_context.record_internal(error);
                        return false;
                    }
                };
                match solver.types_equivalent(actual_ty, binding.ty) {
                    Ok(true) => {}
                    Ok(false) => return false,
                    Err(error) => {
                        self.type_context.record_internal(error);
                        return false;
                    }
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
        for (predicates, module_id) in self.current_where_predicate_sources(current) {
            let predicates = self.normalize_where_predicates(&predicates, module_id);
            let predicates = predicates
                .iter()
                .map(|predicate| self.instantiate_where_predicate(predicate, substitutions))
                .collect::<Vec<_>>();
            for predicate in predicates {
                for bound in predicate.bounds {
                    if let Some((trait_id, trait_args, trait_const_args)) =
                        self.trait_id_and_args(bound.trait_ty)
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
        let (trait_def_id, generic_params) = if current.module_id == self.input.module_id
            && let Some(def) = self.input.defs.defs.get(current.def_id)
            && def.kind == nia_defs::DefKind::TraitMethod
        {
            let trait_def_id = GlobalDefId {
                module_id: current.module_id,
                def_id: def.parent?,
            };
            let trait_signature = self.input.signatures.traits.get(&trait_def_id.def_id)?;
            (trait_def_id, trait_signature.generic_params.clone())
        } else {
            self.input
                .program
                .traits()
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
                        .then(|| (*trait_id, signature.signature.generic_params.clone()))
                })?
        };
        let (trait_args, trait_const_args) =
            match trait_owner_generic_arguments(&self.type_context.append, &generic_params) {
                Ok(arguments) => arguments,
                Err(error) => {
                    self.type_context.record_internal(error);
                    return None;
                }
            };
        let substitutions = self.instantiation.type_substitutions?;
        let self_ty = self.type_context.intern(TyKind::SelfParam);
        let self_ty = self.instantiate_ty_with_id(self_ty, substitutions);
        let trait_args = trait_args
            .into_iter()
            .map(|arg| self.instantiate_ty_with_id(arg, substitutions))
            .collect();
        let trait_const_args = trait_const_args
            .iter()
            .map(|arg| {
                self.instantiate_const_generic_arg_with_id(arg, substitutions, &mut Vec::new())
            })
            .collect();
        Some(TraitGoal {
            self_ty,
            trait_id: TraitId::Source(trait_def_id),
            trait_args,
            trait_const_args,
        })
    }

    fn current_where_predicate_sources(
        &mut self,
        current: GlobalDefId,
    ) -> Vec<(Vec<nia_item_signatures::WherePredicateSignature>, ModuleId)> {
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
            sources.push((signature.where_predicates.clone(), current.module_id));
            return sources;
        }
        if let Some(signature) = self.input.program.functions().get(&current) {
            sources.push((
                signature.signature.where_predicates.clone(),
                current.module_id,
            ));
        }
        sources
    }

    fn current_extension_owner_where_predicates(
        &mut self,
        current: GlobalDefId,
    ) -> Option<(Vec<nia_item_signatures::WherePredicateSignature>, ModuleId)> {
        let source = self.extension_method_source(current)?.clone();
        let target_ty = self.normalized_type_from_module(source.module_id, source.target_ty);
        let Some(TyKind::Nominal {
            def_id,
            args,
            const_args,
        }) = self.ty_kind(target_ty).cloned()
        else {
            return None;
        };
        let args = args
            .iter()
            .map(|arg| self.normalized_type_from_module(source.module_id, *arg))
            .collect::<Vec<_>>();
        let predicates = if def_id.module_id == self.input.module_id {
            self.input
                .signatures
                .structs
                .get(&def_id.def_id)
                .map(|signature| (signature.where_predicates.clone(), def_id.module_id))
                .or_else(|| {
                    self.input
                        .signatures
                        .unions
                        .get(&def_id.def_id)
                        .map(|signature| (signature.where_predicates.clone(), def_id.module_id))
                })?
        } else {
            self.input
                .program
                .structs()
                .get(&def_id)
                .map(|signature| {
                    (
                        signature.signature.where_predicates.clone(),
                        def_id.module_id,
                    )
                })
                .or_else(|| {
                    self.input.program.unions().get(&def_id).map(|signature| {
                        (
                            signature.signature.where_predicates.clone(),
                            def_id.module_id,
                        )
                    })
                })?
        };
        let (substitutions, const_substitutions) =
            self.generic_substitutions_and_consts_for_def(def_id, &args, &const_args);
        let substitutions =
            self.intern_type_and_const_substitutions(&substitutions, &const_substitutions);
        let normalized_predicates = self.normalize_where_predicates(&predicates.0, predicates.1);
        let predicates = normalized_predicates
            .iter()
            .map(|predicate| self.instantiate_where_predicate(predicate, substitutions))
            .collect();
        Some((predicates, self.input.module_id))
    }

    fn current_extension_where_predicates(
        &self,
        current: GlobalDefId,
    ) -> Option<(Vec<nia_item_signatures::WherePredicateSignature>, ModuleId)> {
        if let Some(source) = self.extension_method_source(current) {
            return Some((source.where_predicates.clone(), source.module_id));
        }
        let program_index = self.trait_impl_index_for_method(current)?;
        self.input
            .program
            .trait_impls()
            .get(program_index)
            .map(|signature| (signature.where_predicates.clone(), signature.module_id))
    }

    fn trait_id_and_args(
        &mut self,
        ty: InternedTyId,
    ) -> Option<(TraitId, Vec<InternedTyId>, Vec<nia_ty::ConstGenericArg>)> {
        let ty = self.normalize_instance_arg_type(ty);
        match self.ty_kind(ty) {
            Some(TyKind::Nominal {
                def_id,
                args,
                const_args,
            }) if self.input.program.traits().contains_key(def_id) => {
                Some((TraitId::Source(*def_id), args.clone(), const_args.clone()))
            }
            Some(TyKind::BuiltinTrait { trait_id, args }) => {
                Some((TraitId::Builtin(*trait_id), args.clone(), Vec::new()))
            }
            _ => None,
        }
    }

    pub(super) fn resolve_builtin_trait_method_call_impl(
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
            trait_const_arg_count: 0,
        };
        let candidates = self.program_extension_trait_method_candidates(&key);
        let candidates = candidates
            .iter()
            .filter_map(|candidate| {
                self.builtin_trait_impl_method_for_candidate(candidate, trait_args, self_ty)
            })
            .collect::<Vec<_>>();
        match candidates.as_slice() {
            [candidate] => Some(candidate.clone()),
            _ => None,
        }
    }

    pub(super) fn builtin_trait_impl_method_for_candidate(
        &mut self,
        candidate: &ExtensionTraitMethodCandidate,
        trait_args: &[InternedTyId],
        self_ty: InternedTyId,
    ) -> Option<(GlobalDefId, Vec<InternedTyId>)> {
        let substitutions =
            self.match_extension_trait_impl_candidate(candidate, trait_args, &[], self_ty)?;
        let args = self
            .candidate_impl_generics(candidate)
            .iter()
            .filter_map(|generic| substitutions.get(generic).copied())
            .collect::<Vec<_>>();
        Some((candidate.method_def_id, args))
    }

    pub(super) fn lower_intrinsic_builtin_trait_method_call(
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
                    ty: self.type_context.intern(TyKind::Pointer {
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
                    ty: self.type_context.intern(TyKind::Pointer {
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
                let mut active_projections = Vec::new();
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
        let program_is_enum = |def_id| self.input.program.enums().contains_key(&def_id);
        let context = TraitSolverContext {
            type_store: self.type_store,
            normalization: self.input.type_normalization,
            trait_impls: self.input.program.trait_impls(),
            trait_impl_index: Some(self.input.program.trait_impl_index()),
            layouts: Some(self.input.layouts),
            local_module_id: self.input.module_id,
            local_enums: &self.input.signatures.enums,
            program_is_enum: Some(&program_is_enum),
            const_expr_value: None,
            impl_is_visible: None,
        };
        let mut solver = context.solver(&[]);
        matches!(
            solver.resolve(TraitGoal {
                self_ty,
                trait_id: TraitId::Builtin(trait_id),
                trait_args: trait_args.to_vec(),
                trait_const_args: Vec::new(),
            }),
            Ok(TraitResolution::Intrinsic(_))
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
        let program_is_enum = |def_id| self.input.program.enums().contains_key(&def_id);
        let context = TraitSolverContext {
            type_store: self.type_store,
            normalization: self.input.type_normalization,
            trait_impls: self.input.program.trait_impls(),
            trait_impl_index: Some(self.input.program.trait_impl_index()),
            layouts: Some(self.input.layouts),
            local_module_id: self.input.module_id,
            local_enums: &self.input.signatures.enums,
            program_is_enum: Some(&program_is_enum),
            const_expr_value: None,
            impl_is_visible: None,
        };
        let mut solver = context.solver(&assumptions);
        let resolution = match solver.resolve(TraitGoal {
            self_ty: key.self_ty,
            trait_id: TraitId::Builtin(key.trait_id),
            trait_args: key.trait_args.clone(),
            trait_const_args: Vec::new(),
        }) {
            Ok(resolution) => resolution,
            Err(error) => {
                self.type_context.record_internal(error);
                return TraitResolution::Unsatisfied;
            }
        };
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

/// Rebuilds a trait's open owner instance from declaration-order generic metadata.
///
/// `TraitGoal` stores type and const arguments in separate vectors, so walking
/// the kind-aware parameter list is required for interleaved declarations.
fn trait_owner_generic_arguments(
    append: &nia_ty::TypeStoreAppend,
    generic_params: &[nia_item_signatures::GenericParamSignature],
) -> nia_ice::IceResult<(Vec<InternedTyId>, Vec<nia_ty::ConstGenericArg>)> {
    let mut trait_args = Vec::new();
    let mut trait_const_args = Vec::new();
    for param in generic_params {
        match param.kind {
            nia_item_signatures::GenericParamSignatureKind::Type => {
                trait_args.push(append.intern(TyKind::GenericParam(param.name))?);
            }
            nia_item_signatures::GenericParamSignatureKind::Const { ty } => {
                trait_const_args.push(nia_ty::ConstGenericArg {
                    ty,
                    value: nia_ty::ConstGenericValue::GenericParam(param.name),
                });
            }
        }
    }
    Ok((trait_args, trait_const_args))
}

#[cfg(test)]
mod tests {
    use super::*;
    use nia_item_signatures::{GenericParamSignature, GenericParamSignatureKind};
    use nia_ty::PrimitiveTy;

    trait TestTypeStoreAppend {
        fn test_intern(&self, kind: TyKind) -> nia_ids::InternedTyId;
    }

    impl TestTypeStoreAppend for nia_ty::TypeStoreAppend {
        fn test_intern(&self, kind: TyKind) -> nia_ids::InternedTyId {
            self.intern(kind)
                .expect("intern trait-resolution test type")
        }
    }

    #[test]
    fn trait_owner_arguments_preserve_interleaved_generic_kinds() {
        let module_ids = nia_ids::ModuleIdAllocator::new().expect("create module ID allocator");
        let module_id = module_ids.allocate().expect("allocate module ID");
        let type_store = nia_ty::TypeStore::new().expect("create type store");
        let append = type_store.append_for_module(module_id);
        let usize_ty = append.test_intern(TyKind::Primitive(PrimitiveTy::Usize));
        let first_type = nia_symbol::SymbolId::from_stable_hash(nia_symbol::stable_hash("T"));
        let const_name = nia_symbol::SymbolId::from_stable_hash(nia_symbol::stable_hash("N"));
        let second_type = nia_symbol::SymbolId::from_stable_hash(nia_symbol::stable_hash("U"));
        let params = [
            GenericParamSignature {
                name: first_type,
                kind: GenericParamSignatureKind::Type,
            },
            GenericParamSignature {
                name: const_name,
                kind: GenericParamSignatureKind::Const { ty: usize_ty },
            },
            GenericParamSignature {
                name: second_type,
                kind: GenericParamSignatureKind::Type,
            },
        ];

        let (type_args, const_args) =
            trait_owner_generic_arguments(&append, &params).expect("trait owner arguments");

        assert_eq!(type_args.len(), 2);
        assert!(matches!(
            type_store.get(type_args[0]),
            Some(TyKind::GenericParam(name)) if *name == first_type
        ));
        assert!(matches!(
            type_store.get(type_args[1]),
            Some(TyKind::GenericParam(name)) if *name == second_type
        ));
        assert_eq!(
            const_args,
            vec![nia_ty::ConstGenericArg {
                ty: usize_ty,
                value: nia_ty::ConstGenericValue::GenericParam(const_name),
            }]
        );
    }
}
