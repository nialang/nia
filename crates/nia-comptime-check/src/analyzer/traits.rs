use super::ty_substitution::substitute_ty_generics_in_interner;
use super::*;

impl Analyzer<'_> {
    pub(super) fn normalize_projection(&mut self, ty: InternedTyId) -> InternedTyId {
        self.normalize_projection_inner(ty, &mut HashSet::new())
    }

    pub(super) fn normalize_projection_inner(
        &mut self,
        ty: InternedTyId,
        active: &mut HashSet<(InternedTyId, TraitId, Vec<InternedTyId>, String)>,
    ) -> InternedTyId {
        let ty = self.normalized_ty(ty);
        match self.ty_kind(ty) {
            Some(TyKind::Projection {
                self_ty,
                trait_id,
                trait_args,
                name,
            }) => {
                let self_ty = self.normalize_projection_inner(self_ty, active);
                let trait_args = trait_args
                    .into_iter()
                    .map(|arg| self.normalize_projection_inner(arg, active))
                    .collect::<Vec<_>>();
                let key = (self_ty, trait_id, trait_args.clone(), name.clone());
                let projection = self
                    .intern_current_ty(TyKind::Projection {
                        self_ty,
                        trait_id,
                        trait_args: trait_args.clone(),
                        name: name.clone(),
                    })
                    .unwrap_or(ty);
                if !active.insert(key.clone()) {
                    return projection;
                }
                let normalized = self
                    .resolve_associated_type_projection(self_ty, trait_id, &trait_args, &name)
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
        let program_enums = self.program_enum_signatures();
        let normalized = self
            .normalized_for_module(module_id)
            .cloned()
            .unwrap_or_default();
        let local_enums = self
            .signatures_for_module(module_id)
            .map(|signatures| signatures.enums.clone())
            .unwrap_or_else(|| self.input.signatures.enums.clone());
        let Some(interner) = self.working_interners.get_mut(&module_id) else {
            return false;
        };
        let normalization = nia_type_normalize::TypeNormalization {
            interner: interner.clone(),
            normalized,
            diagnostics: Vec::new(),
        };
        let context = TraitSolverContext {
            normalization: &normalization,
            trait_impls: self.input.program.trait_impls,
            layouts: None,
            local_module_id: module_id,
            local_enums: &local_enums,
            program_enums: Some(&program_enums),
        };
        let mut solver = context.solver(interner, &assumptions);
        solver.proves(TraitGoal {
            self_ty,
            trait_id,
            trait_args,
        })
    }

    pub(super) fn resolve_associated_type_projection(
        &mut self,
        self_ty: InternedTyId,
        trait_id: TraitId,
        trait_args: &[InternedTyId],
        name: &str,
    ) -> Option<InternedTyId> {
        let module_id = self.ensure_trait_solver_module(self_ty, trait_args)?;
        let assumptions = self.current_trait_goals();
        let program_enums = self.program_enum_signatures();
        let normalized = self
            .normalized_for_module(module_id)
            .cloned()
            .unwrap_or_default();
        let local_enums = self
            .signatures_for_module(module_id)
            .map(|signatures| signatures.enums.clone())
            .unwrap_or_else(|| self.input.signatures.enums.clone());
        let interner = self.working_interners.get_mut(&module_id)?;
        let normalization = nia_type_normalize::TypeNormalization {
            interner: interner.clone(),
            normalized,
            diagnostics: Vec::new(),
        };
        let context = TraitSolverContext {
            normalization: &normalization,
            trait_impls: self.input.program.trait_impls,
            layouts: None,
            local_module_id: module_id,
            local_enums: &local_enums,
            program_enums: Some(&program_enums),
        };
        let mut solver = context.solver(interner, &assumptions);
        solver.resolve_associated_type(self_ty, trait_id, trait_args, name)
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
        self.trait_goals_from_where_predicates(&signature.where_predicates, &substitutions)
    }

    pub(super) fn trait_goals_from_where_predicates(
        &mut self,
        predicates: &[WherePredicateSignature],
        substitutions: &HashMap<String, InternedTyId>,
    ) -> Vec<TraitGoal> {
        let mut goals = Vec::new();
        for predicate in predicates {
            let self_ty = self.substitute_ty_generics_from_map(predicate.ty, substitutions);
            for bound in &predicate.bounds {
                let trait_ty = self.substitute_ty_generics_from_map(bound.trait_ty, substitutions);
                let Some((trait_id, trait_args)) = self.trait_id_and_args(trait_ty) else {
                    continue;
                };
                goals.push(TraitGoal {
                    self_ty,
                    trait_id,
                    trait_args,
                });
            }
        }
        goals
    }

    pub(super) fn substitute_ty_generics_from_map(
        &mut self,
        ty: InternedTyId,
        substitutions: &HashMap<String, InternedTyId>,
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

    pub(super) fn trait_id_and_args(
        &self,
        ty: InternedTyId,
    ) -> Option<(TraitId, Vec<InternedTyId>)> {
        match self.ty_kind(ty)? {
            TyKind::Nominal { def_id, args } => Some((TraitId::Source(def_id), args)),
            TyKind::BuiltinTrait { trait_id, args } => Some((TraitId::Builtin(trait_id), args)),
            _ => None,
        }
    }
}
