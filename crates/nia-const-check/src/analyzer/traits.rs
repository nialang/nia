use super::ty_substitution::{substitute_ty_generics, substitute_ty_generics_and_consts};
use super::*;

type ActiveProjectionSet = HashSet<(
    InternedTyId,
    TraitId,
    Vec<InternedTyId>,
    Vec<nia_ty::ConstGenericArg>,
    SymbolId,
)>;

fn builtin_trait_has_associated_const_symbol(
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
        active: &mut ActiveProjectionSet,
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
                    name,
                );
                let projection = self
                    .intern_current_ty(TyKind::Projection {
                        self_ty,
                        trait_id,
                        trait_args: trait_args.clone(),
                        trait_const_args: trait_const_args.clone(),
                        name,
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
        if let Some(normalized) = self.instantiated_type_normalizations.borrow().get(&ty) {
            return *normalized;
        }
        let module_id = self.current_execution_module_id();
        if let Some(normalized) = self
            .type_normalization_for_module(module_id)
            .and_then(|normalization| normalization.as_ref().normalized.get(&ty).copied())
        {
            return normalized;
        }
        // Substitution creates types after the module normalization product was
        // built. Normalize those roots with the same alias expander, including
        // aliases defined in another module; never inspect their raw shape.
        let Some(signatures) = self.signatures_for_module(module_id) else {
            return ty;
        };
        let external_alias = |def_id| {
            self.program_type_alias_signature(def_id)
                .map(|alias| alias.signature)
        };
        let Some(normalization) =
            self.recover_internal(nia_type_normalize::normalize_module_types(
                nia_type_normalize::TypeNormalizationInput {
                    module_id,
                    type_store: self.input.type_store,
                    input_ids: &[ty],
                    signatures: signatures.as_ref(),
                    external_alias: Some(&external_alias),
                },
            ))
        else {
            return self.input.type_store.error();
        };
        let result = normalization.normalized.get(&ty).copied().unwrap_or(ty);
        self.instantiated_type_normalizations
            .borrow_mut()
            .extend(normalization.normalized);
        result
    }

    pub(super) fn proves_trait_obligation(
        &mut self,
        self_ty: InternedTyId,
        trait_id: TraitId,
        trait_args: Vec<InternedTyId>,
    ) -> bool {
        self.proves_trait_obligation_with_const_args(self_ty, trait_id, trait_args, Vec::new())
    }

    pub(super) fn proves_trait_obligation_with_const_args(
        &mut self,
        self_ty: InternedTyId,
        trait_id: TraitId,
        trait_args: Vec<InternedTyId>,
        trait_const_args: Vec<ConstGenericArg>,
    ) -> bool {
        self.explain_trait_obligation_with_const_args(
            self_ty,
            trait_id,
            trait_args,
            trait_const_args,
        )
        .satisfied_resolution()
        .is_some()
    }

    pub(super) fn resolve_trait_obligation(
        &mut self,
        self_ty: InternedTyId,
        trait_id: TraitId,
        trait_args: Vec<InternedTyId>,
    ) -> TraitResolution {
        self.resolve_trait_obligation_with_const_args(self_ty, trait_id, trait_args, Vec::new())
    }

    pub(super) fn resolve_trait_obligation_with_const_args(
        &mut self,
        self_ty: InternedTyId,
        trait_id: TraitId,
        trait_args: Vec<InternedTyId>,
        trait_const_args: Vec<ConstGenericArg>,
    ) -> TraitResolution {
        self.explain_trait_obligation_with_const_args(
            self_ty,
            trait_id,
            trait_args,
            trait_const_args,
        )
        .into_resolution()
    }

    fn explain_trait_obligation_with_const_args(
        &mut self,
        self_ty: InternedTyId,
        trait_id: TraitId,
        trait_args: Vec<InternedTyId>,
        trait_const_args: Vec<ConstGenericArg>,
    ) -> nia_trait_solve::TraitObligationResolution {
        self.explain_trait_obligation_with_requirements(
            self_ty,
            trait_id,
            trait_args,
            trait_const_args,
            &[],
        )
    }

    fn explain_trait_obligation_with_requirements(
        &mut self,
        self_ty: InternedTyId,
        trait_id: TraitId,
        trait_args: Vec<InternedTyId>,
        trait_const_args: Vec<ConstGenericArg>,
        requirements: &[nia_trait_solve::AssociatedTypeRequirement],
    ) -> nia_trait_solve::TraitObligationResolution {
        let Some(module_id) = self.ensure_trait_solver_module(self_ty, &trait_args) else {
            return nia_trait_solve::TraitObligationResolution::Unavailable;
        };
        let assumptions = self.current_trait_goals();
        let local_enums = self
            .signatures_for_module(module_id)
            .map(|signatures| signatures.as_ref().enums.clone())
            .unwrap_or_else(|| self.input.signatures.enums.clone());
        let program_fieldless_enum_query = self.input.program.program_fieldless_enum;
        let program_fieldless_enum = |def_id: GlobalDefId| {
            if def_id.module_id == module_id {
                return local_enums
                    .get(&def_id.def_id)
                    .is_some_and(nia_item_signatures::EnumSignature::is_fieldless);
            }
            program_fieldless_enum_query
                .is_some_and(|program_fieldless_enum| program_fieldless_enum(def_id))
        };
        let trait_impls = self.trait_impls_for_solver_module(module_id);
        let mut array_lengths = self.trait_solver_array_lengths(
            self_ty,
            trait_id,
            &trait_args,
            &trait_const_args,
            &assumptions,
            &trait_impls,
        );
        let mut required_lengths = HashSet::new();
        for requirement in requirements {
            self.collect_array_len_const_exprs_in_ty(requirement.ty, &mut required_lengths);
        }
        self.extend_trait_solver_array_lengths(&mut array_lengths, required_lengths);
        let const_expr_value = |id, _ty| {
            array_lengths
                .get(&id)
                .map(|value| ConstGenericValue::Int(IntConst::unsigned((*value).into())))
        };
        let normalization = self.type_normalization_for_module(module_id);
        let Some(normalization) = normalization else {
            return nia_trait_solve::TraitObligationResolution::Unavailable;
        };
        let trait_impl_index = nia_item_signatures::ProgramTraitImplIndex::new(&trait_impls);
        let layout_module_id = match self.ty_kind(self_ty) {
            Some(TyKind::Nominal { def_id, .. }) => def_id.module_id,
            _ => module_id,
        };
        let layouts = matches!(trait_id, TraitId::Builtin(nia_ty::BuiltinTrait::Sized))
            .then(|| self.compute_program_layout(layout_module_id, &array_lengths))
            .flatten();
        let program_layouts = |module_id| self.compute_program_layout(module_id, &array_lengths);
        let type_layout = |ty| self.concrete_type_has_layout(module_id, ty, &array_lengths);
        let context = TraitSolverContext {
            type_store: self.input.type_store,
            normalization: normalization.as_ref(),
            trait_impls: &trait_impls,
            trait_impl_index: Some(&trait_impl_index),
            layouts: layouts.as_deref(),
            program_layouts: Some(&program_layouts),
            type_layout: Some(&type_layout),
            local_module_id: module_id,
            local_enums: &local_enums,
            program_fieldless_enum: Some(&program_fieldless_enum),
            const_expr_value: Some(&const_expr_value),
            // The provider supplies the witness closure for this execution
            // module. A method index cannot re-filter that proof scope:
            // marker and associated-type-only impls have no method entries.
            impl_is_visible: None,
        };
        let goal = TraitGoal {
            self_ty,
            trait_id,
            trait_args,
            trait_const_args,
        };
        let mut solver = context.solver(&assumptions);
        let Some(resolution) =
            self.recover_internal(solver.explain_with_associated_types(goal.clone(), requirements))
        else {
            return nia_trait_solve::TraitObligationResolution::Unavailable;
        };
        resolution
    }

    pub(super) fn validate_const_call_where_predicates(
        &mut self,
        span: Span,
        signature_module_id: ModuleId,
        predicates: &[WherePredicateSignature],
        substitutions: &SymbolMap<InternedTyId>,
        const_substitutions: &SymbolMap<ConstGenericArg>,
    ) -> Result<(), ConstError> {
        for predicate in predicates {
            let self_ty = self
                .const_expected_param_type(
                    signature_module_id,
                    predicate.ty,
                    substitutions,
                    const_substitutions,
                )
                .ok_or_else(|| ConstError {
                    span,
                    message: "cannot resolve const call trait bound target type".to_string(),
                })?;
            for bound in &predicate.bounds {
                let trait_ty = self
                    .const_expected_param_type(
                        signature_module_id,
                        bound.trait_ty,
                        substitutions,
                        const_substitutions,
                    )
                    .ok_or_else(|| ConstError {
                        span,
                        message: "cannot resolve const call trait bound".to_string(),
                    })?;
                let Some((trait_id, trait_args, trait_const_args)) =
                    self.trait_id_and_args(trait_ty)
                else {
                    continue;
                };
                let mut requirements = Vec::with_capacity(bound.associated_type_bindings.len());
                for binding in &bound.associated_type_bindings {
                    let ty = self
                        .const_expected_param_type(
                            signature_module_id,
                            binding.ty,
                            substitutions,
                            const_substitutions,
                        )
                        .ok_or_else(|| ConstError {
                            span,
                            message: "cannot resolve const call associated type bound".to_string(),
                        })?;
                    requirements.push(nia_trait_solve::AssociatedTypeRequirement {
                        name: binding.name,
                        ty,
                    });
                }
                let obligation = self.explain_trait_obligation_with_requirements(
                    self_ty,
                    trait_id,
                    trait_args.clone(),
                    trait_const_args,
                    &requirements,
                );
                if let nia_trait_solve::TraitObligationResolution::Failed(failure) = obligation {
                    let target_name = self.const_type_name(self_ty);
                    let trait_name = self.const_trait_name(trait_id, &trait_args);
                    let explanation = match failure {
                        nia_trait_solve::TraitObligationFailure::AssociatedTypeMissing { name } => {
                            let name = symbol_text_or_unresolved(self.input.symbols, name);
                            nia_diagnostic::explain_associated_type_binding(
                                &trait_name,
                                &name,
                                None,
                            )
                        }
                        nia_trait_solve::TraitObligationFailure::AssociatedTypeMismatch {
                            name,
                            expected,
                            actual,
                        } => {
                            let name = symbol_text_or_unresolved(self.input.symbols, name);
                            let expected = self.const_type_name(expected);
                            let actual = self.const_type_name(actual);
                            nia_diagnostic::explain_associated_type_binding(
                                &trait_name,
                                &name,
                                Some((&expected, &actual)),
                            )
                        }
                        nia_trait_solve::TraitObligationFailure::NoImplementation => {
                            explain_trait_obligation(
                                &target_name,
                                &trait_name,
                                TraitObligationExplanationKind::NoImplementation,
                            )
                        }
                        nia_trait_solve::TraitObligationFailure::NotVisible => {
                            explain_trait_obligation(
                                &target_name,
                                &trait_name,
                                TraitObligationExplanationKind::NotVisible,
                            )
                        }
                        nia_trait_solve::TraitObligationFailure::Ambiguous => {
                            explain_trait_obligation(
                                &target_name,
                                &trait_name,
                                TraitObligationExplanationKind::Ambiguous,
                            )
                        }
                        nia_trait_solve::TraitObligationFailure::AdditionalConstraint => {
                            explain_trait_obligation(
                                &target_name,
                                &trait_name,
                                TraitObligationExplanationKind::AdditionalConstraint,
                            )
                        }
                    };
                    return Err(ConstError {
                        span,
                        message: format!(
                            "trait bound not satisfied for const function call: {}",
                            explanation.note
                        ),
                    });
                }
            }
        }
        Ok(())
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
        let local_enums = self
            .signatures_for_module(module_id)
            .map(|signatures| signatures.as_ref().enums.clone())
            .unwrap_or_else(|| self.input.signatures.enums.clone());
        let program_fieldless_enum_query = self.input.program.program_fieldless_enum;
        let program_fieldless_enum = |def_id: GlobalDefId| {
            if def_id.module_id == module_id {
                return local_enums
                    .get(&def_id.def_id)
                    .is_some_and(nia_item_signatures::EnumSignature::is_fieldless);
            }
            program_fieldless_enum_query
                .is_some_and(|program_fieldless_enum| program_fieldless_enum(def_id))
        };
        let trait_impls = self.trait_impls_for_solver_module(module_id);
        let array_lengths = self.trait_solver_array_lengths(
            self_ty,
            trait_id,
            trait_args,
            trait_const_args,
            &assumptions,
            &trait_impls,
        );
        let const_expr_value = |id, _ty| {
            array_lengths
                .get(&id)
                .map(|value| ConstGenericValue::Int(IntConst::unsigned((*value).into())))
        };
        let normalization = self.type_normalization_for_module(module_id)?;
        let trait_impl_index = nia_item_signatures::ProgramTraitImplIndex::new(&trait_impls);
        let program_layouts = |module_id| self.compute_program_layout(module_id, &array_lengths);
        let type_layout = |ty| self.concrete_type_has_layout(module_id, ty, &array_lengths);
        let context = TraitSolverContext {
            type_store: self.input.type_store,
            normalization: normalization.as_ref(),
            trait_impls: &trait_impls,
            trait_impl_index: Some(&trait_impl_index),
            layouts: None,
            program_layouts: Some(&program_layouts),
            type_layout: Some(&type_layout),
            local_module_id: module_id,
            local_enums: &local_enums,
            program_fieldless_enum: Some(&program_fieldless_enum),
            const_expr_value: Some(&const_expr_value),
            // The provider supplies the witness closure for this execution
            // module. A method index cannot re-filter that proof scope:
            // marker and associated-type-only impls have no method entries.
            impl_is_visible: None,
        };
        let mut solver = context.solver(&assumptions);
        let result =
            solver.resolve_associated_type(self_ty, trait_id, trait_args, trait_const_args, name);
        self.recover_internal(result).flatten()
    }

    pub(super) fn resolve_associated_const_projection(
        &mut self,
        projection: &AssociatedConstProjection,
    ) -> Option<nia_trait_solve::AssociatedConstResolution> {
        let (type_substitutions, const_substitutions) = self.current_execution_substitutions();
        let self_ty = self.substitute_ty_generics_and_consts_from_maps(
            projection.self_ty,
            &type_substitutions,
            &const_substitutions,
        );
        let trait_args = projection
            .trait_args
            .iter()
            .map(|arg| {
                self.substitute_ty_generics_and_consts_from_maps(
                    *arg,
                    &type_substitutions,
                    &const_substitutions,
                )
            })
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
        let local_enums = self
            .signatures_for_module(module_id)
            .map(|signatures| signatures.as_ref().enums.clone())
            .unwrap_or_else(|| self.input.signatures.enums.clone());
        let program_fieldless_enum_query = self.input.program.program_fieldless_enum;
        let program_fieldless_enum = |def_id: GlobalDefId| {
            if def_id.module_id == module_id {
                return local_enums
                    .get(&def_id.def_id)
                    .is_some_and(nia_item_signatures::EnumSignature::is_fieldless);
            }
            program_fieldless_enum_query
                .is_some_and(|program_fieldless_enum| program_fieldless_enum(def_id))
        };
        let trait_impls = self.trait_impls_for_solver_module(module_id);
        let array_lengths = self.trait_solver_array_lengths(
            self_ty,
            projection.trait_id,
            &trait_args,
            &trait_const_args,
            &assumptions,
            &trait_impls,
        );
        let const_expr_value = |id, _ty| {
            array_lengths
                .get(&id)
                .map(|value| ConstGenericValue::Int(IntConst::unsigned((*value).into())))
        };
        let normalization = self.type_normalization_for_module(module_id)?;
        let trait_impl_index = nia_item_signatures::ProgramTraitImplIndex::new(&trait_impls);
        let program_layouts = |module_id| self.compute_program_layout(module_id, &array_lengths);
        let type_layout = |ty| self.concrete_type_has_layout(module_id, ty, &array_lengths);
        let context = TraitSolverContext {
            type_store: self.input.type_store,
            normalization: normalization.as_ref(),
            trait_impls: &trait_impls,
            trait_impl_index: Some(&trait_impl_index),
            layouts: None,
            program_layouts: Some(&program_layouts),
            type_layout: Some(&type_layout),
            local_module_id: module_id,
            local_enums: &local_enums,
            program_fieldless_enum: Some(&program_fieldless_enum),
            const_expr_value: Some(&const_expr_value),
            // The provider supplies the witness closure for this execution
            // module. A method index cannot re-filter that proof scope:
            // marker and associated-type-only impls have no method entries.
            impl_is_visible: None,
        };
        let mut solver = context.solver(&assumptions);
        let result = solver.resolve_associated_const(
            self_ty,
            projection.trait_id,
            &trait_args,
            &trait_const_args,
            &projection.name,
        );
        self.recover_internal(result).flatten()
    }

    pub(super) fn associated_const_projection_type(
        &mut self,
        projection: &AssociatedConstProjection,
    ) -> Option<InternedTyId> {
        match projection.trait_id {
            TraitId::Builtin(trait_id) => {
                builtin_trait_has_associated_const_symbol(trait_id, projection.name)
                    .then(|| self.primitive_ty_for_current_module(PrimitiveTy::Usize))
            }
            TraitId::Source(trait_def_id) => {
                let signature = self.signatures_for_module(trait_def_id.module_id)?;
                let associated_value = signature
                    .as_ref()
                    .traits
                    .get(&trait_def_id.def_id)?
                    .associated_values
                    .iter()
                    .find(|value| value.name == projection.name)?;
                self.type_for_module_or_none(
                    associated_value.ty,
                    self.current_execution_module_id(),
                )
            }
        }
    }

    fn primitive_ty_for_current_module(&mut self, primitive: PrimitiveTy) -> InternedTyId {
        let module_id = self.current_execution_module_id();
        self.primitive_ty_for_module(module_id, primitive)
    }

    pub(super) fn ensure_trait_solver_module(
        &mut self,
        _self_ty: InternedTyId,
        _trait_args: &[InternedTyId],
    ) -> Option<ModuleId> {
        let module_id = self.current_execution_module_id();
        self.type_context(module_id);
        Some(module_id)
    }

    pub(super) fn current_trait_goals(&mut self) -> Vec<TraitGoal> {
        let mut goals = self
            .active_execution_frames()
            .flat_map(|frame| frame.trait_goals.iter().cloned())
            .collect::<Vec<_>>();
        let Some(function_id) = self.current_execution_function_id() else {
            return goals;
        };
        let Some(signature) = self.function_signature(function_id) else {
            return goals;
        };
        let substitutions = self
            .active_execution_frames()
            .find(|frame| frame.function_id == Some(function_id))
            .map(|frame| frame.type_substitutions.clone())
            .unwrap_or_default();
        let const_substitutions = self
            .active_execution_frames()
            .find(|frame| frame.function_id == Some(function_id))
            .map(|frame| frame.const_substitutions.clone())
            .unwrap_or_default();
        goals.extend(self.trait_goals_from_where_predicates(
            &signature.where_predicates,
            &substitutions,
            &const_substitutions,
        ));
        goals
    }

    pub(super) fn trait_goals_from_where_predicates(
        &mut self,
        predicates: &[WherePredicateSignature],
        substitutions: &SymbolMap<InternedTyId>,
        const_substitutions: &SymbolMap<ConstGenericArg>,
    ) -> Vec<TraitGoal> {
        let mut goals = Vec::new();
        for predicate in predicates {
            let self_ty = self.substitute_ty_generics_and_consts_from_maps(
                predicate.ty,
                substitutions,
                const_substitutions,
            );
            for bound in &predicate.bounds {
                let trait_ty = self.substitute_ty_generics_and_consts_from_maps(
                    bound.trait_ty,
                    substitutions,
                    const_substitutions,
                );
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

    fn trait_solver_array_lengths(
        &mut self,
        self_ty: InternedTyId,
        trait_id: TraitId,
        trait_args: &[InternedTyId],
        trait_const_args: &[ConstGenericArg],
        assumptions: &[TraitGoal],
        trait_impls: &[ProgramTraitImplSignature],
    ) -> HashMap<GlobalConstExprId, u64> {
        let mut needed = HashSet::new();
        self.collect_array_len_const_exprs_in_ty(self_ty, &mut needed);
        for ty in trait_args {
            self.collect_array_len_const_exprs_in_ty(*ty, &mut needed);
        }
        self.collect_trait_solver_const_arg_exprs(trait_const_args, &mut needed);

        for assumption in assumptions {
            self.collect_array_len_const_exprs_in_ty(assumption.self_ty, &mut needed);
            for ty in &assumption.trait_args {
                self.collect_array_len_const_exprs_in_ty(*ty, &mut needed);
            }
            self.collect_trait_solver_const_arg_exprs(&assumption.trait_const_args, &mut needed);
        }

        for impl_signature in trait_impls {
            if impl_signature.trait_id != trait_id {
                continue;
            }
            self.collect_array_len_const_exprs_in_ty(impl_signature.target_ty, &mut needed);
            for ty in &impl_signature.trait_args {
                self.collect_array_len_const_exprs_in_ty(*ty, &mut needed);
            }
            self.collect_trait_solver_const_arg_exprs(
                &impl_signature.trait_const_args,
                &mut needed,
            );
            for associated in &impl_signature.associated_types {
                self.collect_array_len_const_exprs_in_ty(associated.ty, &mut needed);
            }
            for predicate in &impl_signature.where_predicates {
                self.collect_array_len_const_exprs_in_ty(predicate.ty, &mut needed);
                for bound in &predicate.bounds {
                    self.collect_array_len_const_exprs_in_ty(bound.trait_ty, &mut needed);
                    for binding in &bound.associated_type_bindings {
                        self.collect_array_len_const_exprs_in_ty(binding.ty, &mut needed);
                    }
                }
            }
        }

        let mut array_lengths = self.array_lengths.clone();
        self.extend_trait_solver_array_lengths(&mut array_lengths, needed);
        array_lengths
    }

    fn extend_trait_solver_array_lengths(
        &mut self,
        array_lengths: &mut HashMap<GlobalConstExprId, u64>,
        needed: HashSet<GlobalConstExprId>,
    ) {
        for id in needed {
            if !array_lengths.contains_key(&id)
                && let Some(value) = self.eval_array_len_const_expr_id(&id)
            {
                array_lengths.insert(id, value);
            }
        }
    }

    fn collect_trait_solver_const_arg_exprs(
        &self,
        args: &[ConstGenericArg],
        out: &mut HashSet<GlobalConstExprId>,
    ) {
        for arg in args {
            self.collect_array_len_const_exprs_in_ty(arg.ty, out);
            if matches!(
                self.ty_kind(arg.ty),
                Some(TyKind::Primitive(PrimitiveTy::Usize))
            ) && let ConstGenericValue::ConstExpr(id) = &arg.value
            {
                out.insert(id.clone());
            }
        }
    }

    pub(super) fn substitute_ty_generics_from_map(
        &mut self,
        ty: InternedTyId,
        substitutions: &SymbolMap<InternedTyId>,
    ) -> InternedTyId {
        let module_id = self.current_execution_module_id();
        let interner = self.type_context(module_id);
        substitute_ty_generics(&interner, ty, &|name| substitutions.get(name).copied())
    }

    pub(super) fn substitute_ty_generics_and_consts_from_maps(
        &mut self,
        ty: InternedTyId,
        type_substitutions: &SymbolMap<InternedTyId>,
        const_substitutions: &SymbolMap<ConstGenericArg>,
    ) -> InternedTyId {
        let module_id = self.current_execution_module_id();
        let interner = self.type_context(module_id);
        substitute_ty_generics_and_consts(
            &interner,
            ty,
            &|name| type_substitutions.get(name).copied(),
            &|name| const_substitutions.get(name).cloned(),
        )
    }

    pub(super) fn substitute_const_generic_arg_from_maps(
        &mut self,
        arg: ConstGenericArg,
        substitutions: &SymbolMap<InternedTyId>,
        const_substitutions: &SymbolMap<ConstGenericArg>,
    ) -> ConstGenericArg {
        let mut arg = match &arg.value {
            nia_ty::ConstGenericValue::GenericParam(name) => {
                const_substitutions.get(name).cloned().unwrap_or(arg)
            }
            _ => arg,
        };
        arg.ty = self.substitute_ty_generics_and_consts_from_maps(
            arg.ty,
            substitutions,
            const_substitutions,
        );
        arg
    }

    fn const_trait_name(&mut self, trait_id: TraitId, args: &[InternedTyId]) -> String {
        let base = match trait_id {
            TraitId::Builtin(trait_id) => trait_id.name().to_string(),
            TraitId::Source(def_id) => self
                .global_defs(def_id.module_id)
                .and_then(|defs| {
                    defs.as_ref()
                        .defs
                        .get(def_id.def_id)
                        .map(|def| symbol_text_or_unresolved(self.input.symbols, def.name))
                })
                .unwrap_or_else(|| "<const trait>".to_string()),
        };
        if args.is_empty() {
            base
        } else {
            let args = args
                .iter()
                .map(|arg| self.const_type_name(*arg))
                .collect::<Vec<_>>()
                .join(", ");
            format!("{base}[{args}]")
        }
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
