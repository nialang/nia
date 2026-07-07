use super::ty_substitution::substitute_ty_generics_in_interner;
use super::*;

fn builtin_trait_has_associated_comptime_symbol(
    trait_id: nia_ty::BuiltinTrait,
    name: SymbolId,
) -> bool {
    matches!(
        (trait_id, name),
        (nia_ty::BuiltinTrait::Simd, nia_symbol::known::LANES)
    )
}

impl Analyzer<'_> {
    pub(super) fn normalize_projection(&mut self, ty: InternedTyId) -> InternedTyId {
        self.normalize_projection_inner(ty, &mut HashSet::new())
    }

    pub(super) fn normalize_projection_inner(
        &mut self,
        ty: InternedTyId,
        active: &mut HashSet<(
            InternedTyId,
            TraitId,
            Vec<InternedTyId>,
            Vec<nia_ty::ConstGenericArg>,
            SymbolId,
        )>,
    ) -> InternedTyId {
        let ty = self.normalized_ty(ty);
        match self.ty_kind(ty) {
            Some(TyKind::Projection {
                self_ty,
                trait_id,
                trait_args,
                trait_const_args,
                name,
            }) => {
                let self_ty = self.normalize_projection_inner(self_ty, active);
                let trait_args = trait_args
                    .into_iter()
                    .map(|arg| self.normalize_projection_inner(arg, active))
                    .collect::<Vec<_>>();
                let key = (
                    self_ty,
                    trait_id,
                    trait_args.clone(),
                    trait_const_args.clone(),
                    name.clone(),
                );
                let projection = self
                    .intern_current_ty(TyKind::Projection {
                        self_ty,
                        trait_id,
                        trait_args: trait_args.clone(),
                        trait_const_args: trait_const_args.clone(),
                        name: name.clone(),
                    })
                    .unwrap_or(ty);
                if !active.insert(key.clone()) {
                    return projection;
                }
                let normalized = self
                    .resolve_associated_type_projection(
                        self_ty,
                        trait_id,
                        &trait_args,
                        &trait_const_args,
                        &name,
                    )
                    .map(|resolved| self.normalize_projection_inner(resolved, active))
                    .unwrap_or(projection);
                active.remove(&key);
                normalized
            }
            Some(TyKind::Pointer { is_readonly, elem }) => {
                let elem = self.normalize_projection_inner(elem, active);
                self.intern_current_ty(TyKind::Pointer { is_readonly, elem })
                    .unwrap_or(ty)
            }
            Some(TyKind::Optional { elem }) => {
                let elem = self.normalize_projection_inner(elem, active);
                self.intern_current_ty(TyKind::Optional { elem })
                    .unwrap_or(ty)
            }
            Some(TyKind::ErrorUnion { error, value }) => {
                let error = self.normalize_projection_inner(error, active);
                let value = self.normalize_projection_inner(value, active);
                self.intern_current_ty(TyKind::ErrorUnion { error, value })
                    .unwrap_or(ty)
            }
            _ => ty,
        }
    }

    pub(super) fn normalized_ty(&self, ty: InternedTyId) -> InternedTyId {
        self.normalized_for_module(self.type_owner(ty).module_id())
            .and_then(|normalized| normalized.get(&ty).copied())
            .unwrap_or(ty)
    }

    pub(super) fn proves_trait_obligation(
        &mut self,
        self_ty: InternedTyId,
        trait_id: TraitId,
        trait_args: Vec<InternedTyId>,
    ) -> bool {
        let Some(module_id) = self.ensure_trait_solver_module(self_ty, &trait_args) else {
            return false;
        };
        let assumptions = self.current_trait_goals();
        let normalized = self.normalized_for_module(module_id).unwrap_or_default();
        let local_enums = self
            .signatures_for_module(module_id)
            .map(|signatures| signatures.enums.clone())
            .unwrap_or_else(|| self.input.signatures.enums.clone());
        let program_is_enum = |def_id: GlobalDefId| {
            if def_id.module_id == module_id {
                return local_enums.contains_key(&def_id.def_id);
            }
            self.input
                .program
                .program_is_enum
                .is_some_and(|program_is_enum| program_is_enum(def_id))
        };
        let Some(interner_snapshot) = self.working_interners.get(&module_id).cloned() else {
            return false;
        };
        let normalization = nia_type_normalize::TypeNormalization {
            interner: interner_snapshot,
            normalized,
            diagnostics: Vec::new(),
        };
        let visible_extensions = self
            .input
            .program
            .visible_extensions
            .and_then(|visible_extensions| visible_extensions(module_id));
        let trait_impls = self.trait_impls_for_solver_module(module_id);
        let impl_is_visible = |impl_module_id, impl_id| {
            impl_module_id == module_id
                || visible_extensions
                    .as_ref()
                    .is_none_or(|visible_extensions| {
                        visible_extensions.has_trait_witness_impl(impl_module_id, impl_id)
                    })
        };
        let Some(interner) = self.working_interners.get_mut(&module_id) else {
            return false;
        };
        let context = TraitSolverContext {
            normalization: &normalization,
            trait_impls: &trait_impls,
            layouts: None,
            local_module_id: module_id,
            local_enums: &local_enums,
            program_is_enum: Some(&program_is_enum),
            const_expr_value: None,
            impl_is_visible: Some(&impl_is_visible),
        };
        let mut solver = context.solver(interner, &assumptions);
        solver.proves(TraitGoal {
            self_ty,
            trait_id,
            trait_args,
            trait_const_args: Vec::new(),
        })
    }

    pub(super) fn resolve_associated_type_projection(
        &mut self,
        self_ty: InternedTyId,
        trait_id: TraitId,
        trait_args: &[InternedTyId],
        trait_const_args: &[nia_ty::ConstGenericArg],
        name: &SymbolId,
    ) -> Option<InternedTyId> {
        let module_id = self.ensure_trait_solver_module(self_ty, trait_args)?;
        let assumptions = self.current_trait_goals();
        let normalized = self.normalized_for_module(module_id).unwrap_or_default();
        let local_enums = self
            .signatures_for_module(module_id)
            .map(|signatures| signatures.enums.clone())
            .unwrap_or_else(|| self.input.signatures.enums.clone());
        let program_is_enum = |def_id: GlobalDefId| {
            if def_id.module_id == module_id {
                return local_enums.contains_key(&def_id.def_id);
            }
            self.input
                .program
                .program_is_enum
                .is_some_and(|program_is_enum| program_is_enum(def_id))
        };
        let interner_snapshot = self.working_interners.get(&module_id)?.clone();
        let normalization = nia_type_normalize::TypeNormalization {
            interner: interner_snapshot,
            normalized,
            diagnostics: Vec::new(),
        };
        let visible_extensions = self
            .input
            .program
            .visible_extensions
            .and_then(|visible_extensions| visible_extensions(module_id));
        let trait_impls = self.trait_impls_for_solver_module(module_id);
        let impl_is_visible = |impl_module_id, impl_id| {
            impl_module_id == module_id
                || visible_extensions
                    .as_ref()
                    .is_none_or(|visible_extensions| {
                        visible_extensions.has_trait_witness_impl(impl_module_id, impl_id)
                    })
        };
        let interner = self.working_interners.get_mut(&module_id)?;
        let context = TraitSolverContext {
            normalization: &normalization,
            trait_impls: &trait_impls,
            layouts: None,
            local_module_id: module_id,
            local_enums: &local_enums,
            program_is_enum: Some(&program_is_enum),
            const_expr_value: None,
            impl_is_visible: Some(&impl_is_visible),
        };
        let mut solver = context.solver(interner, &assumptions);
        solver.resolve_associated_type(self_ty, trait_id, trait_args, trait_const_args, name)
    }

    pub(super) fn resolve_associated_comptime_projection(
        &mut self,
        projection: &AssociatedComptimeProjection,
    ) -> Option<nia_trait_solve::AssociatedComptimeResolution> {
        let (type_substitutions, const_substitutions) = self.current_substitution_maps();
        let self_ty = self.substitute_ty_generics_from_map(projection.self_ty, &type_substitutions);
        let trait_args = projection
            .trait_args
            .iter()
            .map(|arg| self.substitute_ty_generics_from_map(*arg, &type_substitutions))
            .collect::<Vec<_>>();
        let trait_const_args = projection
            .trait_const_args
            .iter()
            .cloned()
            .map(|arg| {
                self.substitute_const_generic_arg_from_maps(
                    arg,
                    &type_substitutions,
                    &const_substitutions,
                )
            })
            .collect::<Vec<_>>();
        let module_id = self.ensure_trait_solver_module(self_ty, &trait_args)?;
        let assumptions = self.current_trait_goals();
        let normalized = self.normalized_for_module(module_id).unwrap_or_default();
        let local_enums = self
            .signatures_for_module(module_id)
            .map(|signatures| signatures.enums.clone())
            .unwrap_or_else(|| self.input.signatures.enums.clone());
        let program_is_enum = |def_id: GlobalDefId| {
            if def_id.module_id == module_id {
                return local_enums.contains_key(&def_id.def_id);
            }
            self.input
                .program
                .program_is_enum
                .is_some_and(|program_is_enum| program_is_enum(def_id))
        };
        let interner_snapshot = self.working_interners.get(&module_id)?.clone();
        let normalization = nia_type_normalize::TypeNormalization {
            interner: interner_snapshot,
            normalized,
            diagnostics: Vec::new(),
        };
        let visible_extensions = self
            .input
            .program
            .visible_extensions
            .and_then(|visible_extensions| visible_extensions(module_id));
        let trait_impls = self.trait_impls_for_solver_module(module_id);
        let impl_is_visible = |impl_module_id, impl_id| {
            impl_module_id == module_id
                || visible_extensions
                    .as_ref()
                    .is_none_or(|visible_extensions| {
                        visible_extensions.has_trait_witness_impl(impl_module_id, impl_id)
                    })
        };
        let interner = self.working_interners.get_mut(&module_id)?;
        let context = TraitSolverContext {
            normalization: &normalization,
            trait_impls: &trait_impls,
            layouts: None,
            local_module_id: module_id,
            local_enums: &local_enums,
            program_is_enum: Some(&program_is_enum),
            const_expr_value: None,
            impl_is_visible: Some(&impl_is_visible),
        };
        let mut solver = context.solver(interner, &assumptions);
        solver.resolve_associated_comptime(
            self_ty,
            projection.trait_id,
            &trait_args,
            &trait_const_args,
            &projection.name,
        )
    }

    pub(super) fn associated_comptime_projection_type(
        &mut self,
        projection: &AssociatedComptimeProjection,
    ) -> Option<InternedTyId> {
        match projection.trait_id {
            TraitId::Builtin(trait_id) => {
                builtin_trait_has_associated_comptime_symbol(trait_id, projection.name)
                    .then(|| self.primitive_ty_for_current_module(PrimitiveTy::Usize))
            }
            TraitId::Source(trait_def_id) => {
                let signature = self.signatures_for_module(trait_def_id.module_id)?;
                let associated_value = signature
                    .traits
                    .get(&trait_def_id.def_id)?
                    .associated_values
                    .iter()
                    .find(|value| value.name == projection.name)?;
                self.import_ty_into_module_or_none(
                    associated_value.ty,
                    self.current_execution_module_id(),
                )
            }
        }
    }

    fn current_substitution_maps(&self) -> (SymbolMap<InternedTyId>, SymbolMap<ConstGenericArg>) {
        let mut type_substitutions = HashMap::new();
        let mut const_substitutions = HashMap::new();
        for frame in &self.call_locals {
            type_substitutions.extend(frame.type_substitutions.clone());
            const_substitutions.extend(frame.const_substitutions.clone());
        }
        (type_substitutions, const_substitutions)
    }

    fn primitive_ty_for_current_module(&mut self, primitive: PrimitiveTy) -> InternedTyId {
        let module_id = self.current_execution_module_id();
        self.source_interner_for_module(module_id)
            .unwrap_or_else(|| self.input.interner.clone())
            .primitive(primitive)
    }

    pub(super) fn ensure_trait_solver_module(
        &mut self,
        self_ty: InternedTyId,
        trait_args: &[InternedTyId],
    ) -> Option<ModuleId> {
        let module_id = self.type_owner(self_ty).module_id();
        self.ensure_working_interner(module_id)?;
        for arg in trait_args {
            self.ensure_working_interner(self.type_owner(*arg).module_id())?;
        }
        Some(module_id)
    }

    pub(super) fn current_trait_goals(&mut self) -> Vec<TraitGoal> {
        let Some(function_id) = self.current_execution_function_id() else {
            return Vec::new();
        };
        let Some(signatures) = self.signatures_for_module(function_id.module_id) else {
            return Vec::new();
        };
        let Some(signature) = signatures.functions.get(&function_id.def_id).cloned() else {
            return Vec::new();
        };
        let substitutions = self
            .call_locals
            .iter()
            .rev()
            .find(|frame| frame.function_id == Some(function_id))
            .map(|frame| frame.type_substitutions.clone())
            .unwrap_or_default();
        let const_substitutions = self
            .call_locals
            .iter()
            .rev()
            .find(|frame| frame.function_id == Some(function_id))
            .map(|frame| frame.const_substitutions.clone())
            .unwrap_or_default();
        self.trait_goals_from_where_predicates(
            &signature.where_predicates,
            &substitutions,
            &const_substitutions,
        )
    }

    pub(super) fn trait_goals_from_where_predicates(
        &mut self,
        predicates: &[WherePredicateSignature],
        substitutions: &SymbolMap<InternedTyId>,
        const_substitutions: &SymbolMap<ConstGenericArg>,
    ) -> Vec<TraitGoal> {
        let mut goals = Vec::new();
        for predicate in predicates {
            let self_ty = self.substitute_ty_generics_from_map(predicate.ty, substitutions);
            for bound in &predicate.bounds {
                let trait_ty = self.substitute_ty_generics_from_map(bound.trait_ty, substitutions);
                let Some((trait_id, trait_args, trait_const_args)) =
                    self.trait_id_and_args(trait_ty)
                else {
                    continue;
                };
                goals.push(TraitGoal {
                    self_ty,
                    trait_id,
                    trait_args,
                    trait_const_args: trait_const_args
                        .into_iter()
                        .map(|arg| {
                            self.substitute_const_generic_arg_from_maps(
                                arg,
                                substitutions,
                                const_substitutions,
                            )
                        })
                        .collect(),
                });
            }
        }
        goals
    }

    pub(super) fn substitute_ty_generics_from_map(
        &mut self,
        ty: InternedTyId,
        substitutions: &SymbolMap<InternedTyId>,
    ) -> InternedTyId {
        let module_id = self.type_owner(ty).module_id();
        if self.ensure_working_interner(module_id).is_none() {
            return ty;
        }
        let interner = self
            .working_interners
            .get_mut(&module_id)
            .expect("working interner must exist");
        substitute_ty_generics_in_interner(interner, ty, &|name| substitutions.get(name).copied())
    }

    pub(super) fn substitute_const_generic_arg_from_maps(
        &mut self,
        mut arg: ConstGenericArg,
        substitutions: &SymbolMap<InternedTyId>,
        const_substitutions: &SymbolMap<ConstGenericArg>,
    ) -> ConstGenericArg {
        arg.ty = self.substitute_ty_generics_from_map(arg.ty, substitutions);
        if let nia_ty::ConstGenericValue::GenericParam(name) = &arg.value
            && let Some(resolved) = const_substitutions.get(name)
        {
            arg = resolved.clone();
        }
        arg
    }

    pub(super) fn trait_id_and_args(
        &self,
        ty: InternedTyId,
    ) -> Option<(TraitId, Vec<InternedTyId>, Vec<nia_ty::ConstGenericArg>)> {
        match self.ty_kind(ty)? {
            TyKind::Nominal {
                def_id,
                args,
                const_args,
            } => Some((TraitId::Source(def_id), args, const_args)),
            TyKind::TraitObject {
                trait_id,
                trait_args,
                trait_const_args,
                ..
            }
            | TyKind::TraitObjectPointee {
                trait_id,
                trait_args,
                trait_const_args,
                ..
            } => Some((trait_id, trait_args, trait_const_args)),
            TyKind::BuiltinTrait { trait_id, args } => {
                Some((TraitId::Builtin(trait_id), args, Vec::new()))
            }
            _ => None,
        }
    }
}
