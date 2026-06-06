// SPDX-License-Identifier: GPL-3.0-or-later
use std::collections::{HashMap, HashSet};

use crate::{
    ExtensionTraitMethodCandidate, ModuleLowerer, TypeInstantiationKey, TypeSubstitutionId,
    TypeSubstitutionKey,
};
use nia_backend_ir::{BackendFunction, BackendParam};
use nia_function_ir::{
    FunctionArrayElements, FunctionAsmInput, FunctionAsmOutput, FunctionBinding, FunctionBody,
    FunctionCallee, FunctionDeferBody, FunctionExpr, FunctionExprKind, FunctionFieldInit,
    FunctionForHeader, FunctionInlineAsm, FunctionLocal, FunctionOp, FunctionPlace,
    FunctionPlaceBase, FunctionPlaceElem, FunctionRange, FunctionSliceRange, FunctionTerminator,
};
use nia_ids::{BuiltinTrait, BuiltinTraitMethod, GlobalDefId, InternedTyId, ModuleId, TraitId};
use nia_trait_solve::{AssociatedTypeProjectionEq, TraitGoal, TraitResolution, TraitSolverContext};
use nia_ty::{LayoutBuiltin, TyKind};

mod function_body_instantiation;
mod trait_resolution;

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
struct ProjectionInstantiationKey {
    self_ty: InternedTyId,
    trait_id: nia_ty::TraitId,
    trait_args: Vec<InternedTyId>,
    name: String,
}

impl<'a> ModuleLowerer<'a> {
    pub(crate) fn generic_substitutions(
        generics: &[String],
        args: &[InternedTyId],
    ) -> HashMap<String, InternedTyId> {
        generics.iter().cloned().zip(args.iter().copied()).collect()
    }

    pub(crate) fn effective_generics(
        &mut self,
        def_id: GlobalDefId,
        own_generics: &[String],
    ) -> &[String] {
        if !self.effective_generics.contains_key(&def_id) {
            let generics = self.compute_effective_generics(def_id, own_generics);
            self.effective_generics.insert(def_id, generics);
        }
        self.effective_generics
            .get(&def_id)
            .map(Vec::as_slice)
            .unwrap_or(&[])
    }

    fn compute_effective_generics(
        &self,
        def_id: GlobalDefId,
        own_generics: &[String],
    ) -> Vec<String> {
        if let Some(generics) = self.program_trait_method_generics(def_id, own_generics) {
            return generics;
        }
        if self
            .input
            .defs
            .defs
            .get(def_id.def_id)
            .is_some_and(|def| def.kind == nia_defs::DefKind::TraitMethod)
        {
            let mut generics = vec!["Self".to_string()];
            generics.extend(
                self.input
                    .defs
                    .defs
                    .get(def_id.def_id)
                    .and_then(|def| def.parent)
                    .and_then(|parent| self.input.defs.defs.get(parent))
                    .map(|parent| parent.generics.clone())
                    .unwrap_or_default(),
            );
            generics.extend(own_generics.iter().cloned());
            return generics;
        }
        let mut generics = self
            .extension_method_impl_generics(def_id)
            .unwrap_or_else(|| {
                self.input
                    .defs
                    .defs
                    .get(def_id.def_id)
                    .and_then(|def| def.parent)
                    .and_then(|parent| self.input.defs.defs.get(parent))
                    .map(|parent| parent.generics.clone())
                    .unwrap_or_default()
            });
        generics.extend(own_generics.iter().cloned());
        generics
    }

    fn extension_method_impl_generics(&self, def_id: GlobalDefId) -> Option<Vec<String>> {
        self.extension_generics_by_method.get(&def_id).cloned()
    }

    fn program_trait_method_generics(
        &self,
        def_id: GlobalDefId,
        own_generics: &[String],
    ) -> Option<Vec<String>> {
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
                        } == def_id
                    })
                    .then(|| {
                        let mut generics = vec!["Self".to_string()];
                        generics.extend(signature.signature.generics.iter().cloned());
                        generics.extend(own_generics.iter().cloned());
                        generics
                    })
            })
    }

    pub(crate) fn import_instance_arg_type(&mut self, ty: InternedTyId) -> InternedTyId {
        if ty.interner_id == self.interner.interner_id() {
            return ty;
        }
        if let Some(interner) = self.current_instantiated_body_interner
            && ty.interner_id == interner.interner_id()
        {
            return nia_ty::import_type_into(&mut self.interner, interner, ty);
        }
        if let Some(interner) = self.known_type_interners.get(&ty.interner_id).copied() {
            return nia_ty::import_type_into(&mut self.interner, interner, ty);
        }
        ty
    }

    pub(crate) fn instantiate_params(
        &mut self,
        function: &BackendFunction,
        substitutions: &HashMap<String, InternedTyId>,
    ) -> Vec<BackendParam> {
        let substitutions = self.intern_type_substitutions(substitutions);
        function
            .params
            .iter()
            .map(|param| BackendParam {
                local_id: param.local_id,
                name: param.name.clone(),
                receiver: param.receiver,
                passing_ty: self.instantiate_ty_with_id(param.passing_ty, substitutions),
                local_ty: self.instantiate_ty_with_id(param.local_ty, substitutions),
                span: param.span,
            })
            .collect()
    }

    pub(crate) fn instantiate_function_body(
        &mut self,
        function: nia_ids::GlobalDefId,
        instantiation_module_id: ModuleId,
        type_arg_count: usize,
        body: FunctionBody,
        substitutions: &HashMap<String, InternedTyId>,
    ) -> FunctionBody {
        let previous_candidates = self.instance_extension_trait_method_candidates.take();
        let previous_interner = self.instance_extension_interner.take();
        let previous_function = self.current_instantiated_function.replace(function);
        let previous_body_interner = self.current_instantiated_body_interner.take();
        let previous_substitutions = self.current_type_substitutions.take();
        self.current_instantiated_body_interner = self
            .input
            .program_type_interners
            .get(&function.module_id)
            .copied()
            .or(Some(&self.input.body_ir.interner));
        if instantiation_module_id != self.input.module_id
            && let Some((extensions, interner)) =
                self.input.program_extensions.get(&instantiation_module_id)
        {
            self.instance_extension_trait_method_candidates = Some((
                instantiation_module_id,
                crate::index_extension_trait_method_candidates(extensions, interner),
            ));
            self.instance_extension_interner = Some(*interner);
        }
        let substitutions = self.intern_type_substitutions(substitutions);
        self.current_type_substitutions = Some(substitutions);
        let body = FunctionBody {
            span: body.span,
            locals: body
                .locals
                .into_iter()
                .map(|local| FunctionLocal {
                    id: local.id,
                    name: local.name,
                    kind: local.kind,
                    ty: self.instantiate_ty_with_id(local.ty, substitutions),
                    span: local.span,
                })
                .collect(),
            scopes: body.scopes,
            blocks: body
                .blocks
                .into_iter()
                .map(|block| nia_function_ir::FunctionBlock {
                    id: block.id,
                    scope: block.scope,
                    span: block.span,
                    ops: block
                        .ops
                        .into_iter()
                        .map(|op| self.instantiate_op(op, substitutions))
                        .collect(),
                    terminator: self.instantiate_terminator(block.terminator, substitutions),
                })
                .collect(),
            entry: body.entry,
            ty: self.instantiate_ty_with_id(body.ty, substitutions),
        };
        let body = self.resolve_builtin_operator_calls_in_body(body);
        let body = self.optimize_function_body(function, true, type_arg_count, body);
        self.instance_extension_trait_method_candidates = previous_candidates;
        self.instance_extension_interner = previous_interner;
        self.current_instantiated_function = previous_function;
        self.current_instantiated_body_interner = previous_body_interner;
        self.current_type_substitutions = previous_substitutions;
        body
    }

    pub(crate) fn match_extension_trait_impl_candidate(
        &mut self,
        candidate: &ExtensionTraitMethodCandidate,
        trait_args: &[InternedTyId],
        self_ty: InternedTyId,
    ) -> Option<HashMap<String, InternedTyId>> {
        let mut substitutions = HashMap::new();
        let target_ty = nia_ty::import_type_into(
            &mut self.interner,
            &candidate.source_interner,
            candidate.target_ty,
        );
        if !self.match_extension_type_pattern(target_ty, self_ty, &mut substitutions) {
            return None;
        }
        if candidate.trait_args.len() != trait_args.len() {
            return None;
        }
        let candidate_trait_args = candidate
            .trait_args
            .iter()
            .map(|arg| {
                nia_ty::import_type_into(&mut self.interner, &candidate.source_interner, *arg)
            })
            .collect::<Vec<_>>();
        if !candidate_trait_args
            .iter()
            .zip(trait_args)
            .all(|(pattern, actual)| {
                self.match_extension_type_pattern(*pattern, *actual, &mut substitutions)
            })
        {
            return None;
        }
        if !self.candidate_where_predicates_hold(candidate, &substitutions) {
            return None;
        }
        Some(substitutions)
    }

    pub(crate) fn import_where_predicates(
        &mut self,
        predicates: &[nia_item_signatures::WherePredicateSignature],
        source_interner: &nia_ty::TyInterner,
    ) -> Vec<nia_item_signatures::WherePredicateSignature> {
        predicates
            .iter()
            .map(|predicate| nia_item_signatures::WherePredicateSignature {
                ty: nia_ty::import_type_into(&mut self.interner, source_interner, predicate.ty),
                bounds: predicate
                    .bounds
                    .iter()
                    .map(|bound| nia_item_signatures::WhereBoundSignature {
                        trait_ty: nia_ty::import_type_into(
                            &mut self.interner,
                            source_interner,
                            bound.trait_ty,
                        ),
                        associated_type_bindings: bound
                            .associated_type_bindings
                            .iter()
                            .map(
                                |binding| nia_item_signatures::AssociatedTypeBindingSignature {
                                    name: binding.name.clone(),
                                    ty: nia_ty::import_type_into(
                                        &mut self.interner,
                                        source_interner,
                                        binding.ty,
                                    ),
                                    span: binding.span,
                                },
                            )
                            .collect(),
                        span: bound.span,
                    })
                    .collect(),
                span: predicate.span,
            })
            .collect()
    }

    pub(crate) fn substitute_where_predicate(
        &mut self,
        predicate: &nia_item_signatures::WherePredicateSignature,
        substitutions: &HashMap<String, InternedTyId>,
    ) -> nia_item_signatures::WherePredicateSignature {
        nia_item_signatures::WherePredicateSignature {
            ty: self.instantiate_ty(predicate.ty, substitutions),
            bounds: predicate
                .bounds
                .iter()
                .map(|bound| nia_item_signatures::WhereBoundSignature {
                    trait_ty: self.instantiate_ty(bound.trait_ty, substitutions),
                    associated_type_bindings: bound
                        .associated_type_bindings
                        .iter()
                        .map(
                            |binding| nia_item_signatures::AssociatedTypeBindingSignature {
                                name: binding.name.clone(),
                                ty: self.instantiate_ty(binding.ty, substitutions),
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

    pub(crate) fn candidate_impl_generics<'b>(
        &self,
        candidate: &'b ExtensionTraitMethodCandidate,
    ) -> &'b [String] {
        &candidate.impl_generics
    }

    pub(crate) fn trait_method_call_is_concrete(
        &mut self,
        self_ty: InternedTyId,
        trait_args: &[InternedTyId],
        method_args: &[InternedTyId],
    ) -> bool {
        !self.ty_contains_generic_param(self_ty)
            && !trait_args
                .iter()
                .chain(method_args)
                .any(|arg| self.ty_contains_generic_param(*arg))
    }

    fn ty_contains_generic_param(&mut self, ty: InternedTyId) -> bool {
        let current_interner = self.interner.clone();
        let body_interner = &self.input.body_ir.interner;
        let extension_interner = self.input.extension_interner;
        let mut ty_kind = |ty: InternedTyId| {
            if ty.interner_id == current_interner.interner_id() {
                return current_interner.get(ty).cloned();
            }
            if ty.interner_id == body_interner.interner_id() {
                return body_interner.get(ty).cloned();
            }
            if let Some(extension_interner) = extension_interner
                && ty.interner_id == extension_interner.interner_id()
            {
                return extension_interner.get(ty).cloned();
            }
            if let Some(interner) = self.current_instantiated_body_interner
                && ty.interner_id == interner.interner_id()
            {
                return interner.get(ty).cloned();
            }
            if let Some(current) = self.current_instantiated_function
                && let Some(interner) = self.input.program_type_interners.get(&current.module_id)
                && ty.interner_id == interner.interner_id()
            {
                return interner.get(ty).cloned();
            }
            self.known_type_interners
                .get(&ty.interner_id)
                .and_then(|interner| interner.get(ty).cloned())
                .or_else(|| Some(TyKind::GenericParam("<unknown>".to_string())))
        };
        crate::function_instances::contains_generic_param(ty, &mut ty_kind, None)
    }

    pub(crate) fn instantiate_ty(
        &mut self,
        ty: InternedTyId,
        substitutions: &HashMap<String, InternedTyId>,
    ) -> InternedTyId {
        let substitutions = self.intern_type_substitutions(substitutions);
        self.instantiate_ty_with_id(ty, substitutions)
    }

    fn instantiate_ty_with_id(
        &mut self,
        ty: InternedTyId,
        substitutions: TypeSubstitutionId,
    ) -> InternedTyId {
        self.instantiate_ty_with_id_inner(ty, substitutions, &mut HashSet::new())
    }

    fn instantiate_ty_with_id_inner(
        &mut self,
        ty: InternedTyId,
        substitutions: TypeSubstitutionId,
        active_projections: &mut HashSet<ProjectionInstantiationKey>,
    ) -> InternedTyId {
        let ty = self.import_instance_arg_type(ty);
        let key = TypeInstantiationKey {
            ty,
            substitutions,
            current_function: self.current_instantiated_function,
        };
        let can_use_cache = active_projections.is_empty();
        if can_use_cache && let Some(instantiated) = self.type_instantiations.get(&key) {
            return *instantiated;
        }
        match self.interner.get(ty).cloned() {
            Some(TyKind::GenericParam(name)) => {
                let instantiated = self.type_substitution(substitutions, &name).unwrap_or(ty);
                self.finish_type_instantiation(key, instantiated, can_use_cache)
            }
            Some(TyKind::Pointer { is_readonly, elem }) => {
                let elem =
                    self.instantiate_ty_with_id_inner(elem, substitutions, active_projections);
                let instantiated = self.interner.intern(TyKind::Pointer { is_readonly, elem });
                self.finish_type_instantiation(key, instantiated, can_use_cache)
            }
            Some(TyKind::Slice { is_readonly, elem }) => {
                let elem =
                    self.instantiate_ty_with_id_inner(elem, substitutions, active_projections);
                let instantiated = self.interner.intern(TyKind::Slice { is_readonly, elem });
                self.finish_type_instantiation(key, instantiated, can_use_cache)
            }
            Some(TyKind::Array { len, elem }) => {
                let elem =
                    self.instantiate_ty_with_id_inner(elem, substitutions, active_projections);
                let instantiated = self.interner.intern(TyKind::Array { len, elem });
                self.finish_type_instantiation(key, instantiated, can_use_cache)
            }
            Some(TyKind::Range { kind, bound }) => {
                let bound = bound.map(|bound| {
                    self.instantiate_ty_with_id_inner(bound, substitutions, active_projections)
                });
                let instantiated = self.interner.intern(TyKind::Range { kind, bound });
                self.finish_type_instantiation(key, instantiated, can_use_cache)
            }
            Some(TyKind::FunctionPointer {
                params,
                return_type,
                is_variadic,
            }) => {
                let params = params
                    .iter()
                    .copied()
                    .map(|param| {
                        self.instantiate_ty_with_id_inner(param, substitutions, active_projections)
                    })
                    .collect();
                let return_type = self.instantiate_ty_with_id_inner(
                    return_type,
                    substitutions,
                    active_projections,
                );
                let instantiated = self.interner.intern(TyKind::FunctionPointer {
                    params,
                    return_type,
                    is_variadic,
                });
                self.finish_type_instantiation(key, instantiated, can_use_cache)
            }
            Some(TyKind::Optional { elem }) => {
                let elem =
                    self.instantiate_ty_with_id_inner(elem, substitutions, active_projections);
                let instantiated = self.interner.intern(TyKind::Optional { elem });
                self.finish_type_instantiation(key, instantiated, can_use_cache)
            }
            Some(TyKind::ErrorUnion { error, value }) => {
                let error =
                    self.instantiate_ty_with_id_inner(error, substitutions, active_projections);
                let value =
                    self.instantiate_ty_with_id_inner(value, substitutions, active_projections);
                let instantiated = self.interner.intern(TyKind::ErrorUnion { error, value });
                self.finish_type_instantiation(key, instantiated, can_use_cache)
            }
            Some(TyKind::Nominal { def_id, args }) => {
                let args = args
                    .iter()
                    .copied()
                    .map(|arg| {
                        self.instantiate_ty_with_id_inner(arg, substitutions, active_projections)
                    })
                    .collect::<Vec<_>>();
                let instantiated = self.interner.intern(TyKind::Nominal { def_id, args });
                self.finish_type_instantiation(key, instantiated, can_use_cache)
            }
            Some(TyKind::BuiltinTrait { trait_id, args }) => {
                let args = args
                    .iter()
                    .copied()
                    .map(|arg| {
                        self.instantiate_ty_with_id_inner(arg, substitutions, active_projections)
                    })
                    .collect::<Vec<_>>();
                let instantiated = self
                    .interner
                    .intern(TyKind::BuiltinTrait { trait_id, args });
                self.finish_type_instantiation(key, instantiated, can_use_cache)
            }
            Some(TyKind::TraitObject {
                is_readonly,
                trait_id,
                trait_args,
                associated_type_bindings,
            }) => {
                let trait_args = trait_args
                    .iter()
                    .copied()
                    .map(|arg| {
                        self.instantiate_ty_with_id_inner(arg, substitutions, active_projections)
                    })
                    .collect::<Vec<_>>();
                let associated_type_bindings = associated_type_bindings
                    .iter()
                    .map(|binding| nia_ty::AssociatedTypeBindingTy {
                        trait_id: binding.trait_id,
                        trait_args: binding
                            .trait_args
                            .iter()
                            .copied()
                            .map(|arg| {
                                self.instantiate_ty_with_id_inner(
                                    arg,
                                    substitutions,
                                    active_projections,
                                )
                            })
                            .collect(),
                        name: binding.name.clone(),
                        ty: self.instantiate_ty_with_id_inner(
                            binding.ty,
                            substitutions,
                            active_projections,
                        ),
                    })
                    .collect();
                let instantiated = self.interner.intern(TyKind::TraitObject {
                    is_readonly,
                    trait_id,
                    trait_args,
                    associated_type_bindings,
                });
                self.finish_type_instantiation(key, instantiated, can_use_cache)
            }
            Some(TyKind::Projection {
                self_ty,
                trait_id,
                trait_args,
                name,
            }) => {
                let original_self_ty = self_ty;
                let original_trait_args = trait_args.clone();
                let self_ty =
                    self.instantiate_ty_with_id_inner(self_ty, substitutions, active_projections);
                let trait_args = trait_args
                    .iter()
                    .copied()
                    .map(|arg| {
                        self.instantiate_ty_with_id_inner(arg, substitutions, active_projections)
                    })
                    .collect::<Vec<_>>();
                let projection_key = ProjectionInstantiationKey {
                    self_ty,
                    trait_id,
                    trait_args: trait_args.clone(),
                    name: name.clone(),
                };
                let projection = self.interner.intern(TyKind::Projection {
                    self_ty,
                    trait_id,
                    trait_args: trait_args.clone(),
                    name: name.clone(),
                });
                if !active_projections.insert(projection_key.clone()) {
                    return projection;
                }
                let resolved = self
                    .resolve_associated_type_projection(
                        self_ty,
                        trait_id,
                        &trait_args,
                        &name,
                        substitutions,
                        active_projections,
                    )
                    .map(|resolved| {
                        self.instantiate_ty_with_id_inner(
                            resolved,
                            substitutions,
                            active_projections,
                        )
                    });
                active_projections.remove(&projection_key);
                let instantiated = resolved.unwrap_or_else(|| {
                    if self_ty == original_self_ty && trait_args == original_trait_args {
                        ty
                    } else {
                        projection
                    }
                });
                self.finish_type_instantiation(key, instantiated, can_use_cache)
            }
            Some(TyKind::Error | TyKind::ComptimeOnly) | Some(TyKind::Primitive(_)) | None => {
                self.finish_type_instantiation(key, ty, can_use_cache)
            }
        }
    }

    pub(crate) fn intern_type_substitutions(
        &mut self,
        substitutions: &HashMap<String, InternedTyId>,
    ) -> TypeSubstitutionId {
        let mut substitutions = substitutions
            .iter()
            .map(|(name, ty)| (name.clone(), self.import_instance_arg_type(*ty)))
            .collect::<Vec<_>>();
        substitutions.sort_by(|left, right| left.0.cmp(&right.0));
        let key = TypeSubstitutionKey { substitutions };
        if let Some(id) = self.type_substitution_ids.get(&key) {
            return *id;
        }
        let id = TypeSubstitutionId(self.type_substitutions.len());
        self.type_substitutions
            .push(key.substitutions.iter().cloned().collect());
        self.type_substitution_ids.insert(key, id);
        id
    }

    pub(super) fn empty_type_substitution_id(&mut self) -> TypeSubstitutionId {
        self.intern_type_substitutions(&HashMap::new())
    }

    fn normalize_type_for_match(&mut self, ty: InternedTyId) -> InternedTyId {
        let substitutions = self.empty_type_substitution_id();
        self.instantiate_ty_with_id(ty, substitutions)
    }

    fn type_substitution(
        &self,
        substitutions: TypeSubstitutionId,
        name: &str,
    ) -> Option<InternedTyId> {
        self.type_substitutions
            .get(substitutions.0)?
            .get(name)
            .copied()
    }

    fn cache_type_instantiation(
        &mut self,
        key: TypeInstantiationKey,
        instantiated: InternedTyId,
    ) -> InternedTyId {
        self.type_instantiations.insert(key, instantiated);
        instantiated
    }

    fn finish_type_instantiation(
        &mut self,
        key: TypeInstantiationKey,
        instantiated: InternedTyId,
        cache: bool,
    ) -> InternedTyId {
        if cache {
            self.cache_type_instantiation(key, instantiated)
        } else {
            instantiated
        }
    }

    fn resolve_associated_type_projection(
        &mut self,
        self_ty: InternedTyId,
        trait_id: nia_ty::TraitId,
        trait_args: &[InternedTyId],
        name: &str,
        substitutions: TypeSubstitutionId,
        active_projections: &mut HashSet<ProjectionInstantiationKey>,
    ) -> Option<InternedTyId> {
        let associated_type_assumptions =
            self.current_associated_type_assumptions(substitutions, active_projections);
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
            &[],
            &associated_type_assumptions,
        );
        solver.resolve_associated_type(self_ty, trait_id, trait_args, name)
    }

    fn current_associated_type_assumptions(
        &mut self,
        substitutions: TypeSubstitutionId,
        active_projections: &mut HashSet<ProjectionInstantiationKey>,
    ) -> Vec<AssociatedTypeProjectionEq> {
        let Some(current) = self.current_instantiated_function else {
            return Vec::new();
        };
        if current.module_id != self.input.module_id {
            return Vec::new();
        }
        let Some(def) = self.input.defs.defs.get(current.def_id) else {
            return Vec::new();
        };
        if def.kind != nia_defs::DefKind::Method {
            return Vec::new();
        }
        let Some(impl_index) = self.trait_impls_by_method.get(&current).copied() else {
            return Vec::new();
        };
        let Some(impl_signature) = self.input.trait_impls.get(impl_index) else {
            return Vec::new();
        };
        let target_ty = nia_ty::import_type_into(
            &mut self.interner,
            &impl_signature.interner,
            impl_signature.target_ty,
        );
        let target_ty =
            self.instantiate_ty_with_id_inner(target_ty, substitutions, active_projections);
        let trait_id = impl_signature.trait_id;
        let impl_interner = impl_signature.interner.clone();
        let trait_args = impl_signature.trait_args.clone();
        let associated_types = impl_signature.associated_types.clone();
        let trait_args = trait_args
            .into_iter()
            .map(|arg| {
                let actual = nia_ty::import_type_into(&mut self.interner, &impl_interner, arg);
                self.instantiate_ty_with_id_inner(actual, substitutions, active_projections)
            })
            .collect::<Vec<_>>();
        let goal = TraitGoal {
            self_ty: target_ty,
            trait_id,
            trait_args,
        };
        associated_types
            .into_iter()
            .map(|associated_type| {
                let ty = nia_ty::import_type_into(
                    &mut self.interner,
                    &impl_interner,
                    associated_type.ty,
                );
                AssociatedTypeProjectionEq {
                    goal: goal.clone(),
                    name: associated_type.name,
                    ty: self.instantiate_ty_with_id_inner(ty, substitutions, active_projections),
                }
            })
            .collect()
    }

    pub(super) fn current_associated_type_assumptions_without_active_projections(
        &mut self,
    ) -> Vec<AssociatedTypeProjectionEq> {
        let Some(substitutions) = self.current_type_substitutions else {
            return Vec::new();
        };
        self.current_associated_type_assumptions(substitutions, &mut HashSet::new())
    }

    fn extension_ty_kind(&self, ty: InternedTyId) -> Option<&TyKind> {
        self.instance_extension_interner
            .filter(|interner| ty.interner_id == interner.interner_id())
            .and_then(|interner| interner.get(ty))
            .or_else(|| {
                self.input
                    .extension_interner
                    .filter(|interner| ty.interner_id == interner.interner_id())
                    .and_then(|interner| interner.get(ty))
            })
            .or_else(|| self.ty_kind(ty))
    }

    pub(crate) fn match_extension_type_pattern(
        &mut self,
        pattern: InternedTyId,
        actual: InternedTyId,
        substitutions: &mut HashMap<String, InternedTyId>,
    ) -> bool {
        let actual = self.canonicalize_instance_arg(actual);
        if self.extension_pattern_generics_are_bound(pattern, substitutions) {
            let substitution_id = self.intern_type_substitutions(substitutions);
            let mut active_projections = HashSet::new();
            let pattern = self.instantiate_ty_with_id_inner(
                pattern,
                substitution_id,
                &mut active_projections,
            );
            return self.types_match(pattern, actual);
        }
        match self.extension_ty_kind(pattern).cloned() {
            Some(TyKind::GenericParam(name)) => {
                if let Some(existing) = substitutions.get(&name).copied() {
                    self.types_match(existing, actual)
                } else {
                    substitutions.insert(name.clone(), actual);
                    true
                }
            }
            Some(TyKind::Pointer {
                is_readonly: pattern_const,
                elem: pattern_elem,
            }) => match self.ty_kind(actual).cloned() {
                Some(TyKind::Pointer { is_readonly, elem }) if is_readonly == pattern_const => {
                    self.match_extension_type_pattern(pattern_elem, elem, substitutions)
                }
                _ => false,
            },
            Some(TyKind::Slice {
                is_readonly: pattern_const,
                elem: pattern_elem,
            }) => match self.ty_kind(actual).cloned() {
                Some(TyKind::Slice { is_readonly, elem }) if is_readonly == pattern_const => {
                    self.match_extension_type_pattern(pattern_elem, elem, substitutions)
                }
                _ => false,
            },
            Some(TyKind::Array {
                len: pattern_len,
                elem: pattern_elem,
            }) => match self.ty_kind(actual).cloned() {
                Some(TyKind::Array { len, elem }) if pattern_len == len => {
                    self.match_extension_type_pattern(pattern_elem, elem, substitutions)
                }
                _ => false,
            },
            Some(TyKind::Range {
                kind: pattern_kind,
                bound: pattern_bound,
            }) => match self.ty_kind(actual).cloned() {
                Some(TyKind::Range { kind, bound }) if pattern_kind == kind => {
                    match (pattern_bound, bound) {
                        (Some(pattern_bound), Some(bound)) => {
                            self.match_extension_type_pattern(pattern_bound, bound, substitutions)
                        }
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
            }) => match self.ty_kind(actual).cloned() {
                Some(TyKind::FunctionPointer {
                    params,
                    return_type,
                    is_variadic,
                }) if pattern_variadic == is_variadic && pattern_params.len() == params.len() => {
                    pattern_params.iter().zip(params).all(|(pattern, actual)| {
                        self.match_extension_type_pattern(*pattern, actual, substitutions)
                    }) && self.match_extension_type_pattern(
                        pattern_return,
                        return_type,
                        substitutions,
                    )
                }
                _ => false,
            },
            Some(TyKind::Optional { elem: pattern_elem }) => match self.ty_kind(actual).cloned() {
                Some(TyKind::Optional { elem }) => {
                    self.match_extension_type_pattern(pattern_elem, elem, substitutions)
                }
                _ => false,
            },
            Some(TyKind::ErrorUnion {
                error: pattern_error,
                value: pattern_value,
            }) => match self.ty_kind(actual).cloned() {
                Some(TyKind::ErrorUnion { error, value }) => {
                    self.match_extension_type_pattern(pattern_error, error, substitutions)
                        && self.match_extension_type_pattern(pattern_value, value, substitutions)
                }
                _ => false,
            },
            Some(TyKind::Nominal {
                def_id: pattern_def,
                args: pattern_args,
            }) => match self.ty_kind(actual).cloned() {
                Some(TyKind::Nominal { def_id, args })
                    if pattern_def == def_id && pattern_args.len() == args.len() =>
                {
                    pattern_args.iter().zip(args).all(|(pattern, actual)| {
                        self.match_extension_type_pattern(*pattern, actual, substitutions)
                    })
                }
                _ => false,
            },
            Some(TyKind::BuiltinTrait {
                trait_id: pattern_trait,
                args: pattern_args,
            }) => match self.ty_kind(actual).cloned() {
                Some(TyKind::BuiltinTrait { trait_id, args })
                    if pattern_trait == trait_id && pattern_args.len() == args.len() =>
                {
                    pattern_args.iter().zip(args).all(|(pattern, actual)| {
                        self.match_extension_type_pattern(*pattern, actual, substitutions)
                    })
                }
                _ => false,
            },
            Some(TyKind::TraitObject {
                is_readonly: pattern_const,
                trait_id: pattern_trait,
                trait_args: pattern_args,
                associated_type_bindings: pattern_bindings,
            }) => match self.ty_kind(actual).cloned() {
                Some(TyKind::TraitObject {
                    is_readonly,
                    trait_id,
                    trait_args,
                    associated_type_bindings,
                }) if is_readonly == pattern_const
                    && trait_id == pattern_trait
                    && pattern_args.len() == trait_args.len()
                    && pattern_bindings.len() == associated_type_bindings.len() =>
                {
                    pattern_args
                        .iter()
                        .zip(trait_args)
                        .all(|(pattern, actual)| {
                            self.match_extension_type_pattern(*pattern, actual, substitutions)
                        })
                        && pattern_bindings.iter().all(|pattern_binding| {
                            associated_type_bindings
                                .iter()
                                .find(|actual_binding| {
                                    self.associated_type_binding_keys_match(
                                        pattern_binding,
                                        actual_binding,
                                    )
                                })
                                .is_some_and(|actual_binding| {
                                    self.match_extension_type_pattern(
                                        pattern_binding.ty,
                                        actual_binding.ty,
                                        substitutions,
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
                name: pattern_name,
            }) => match self.ty_kind(actual).cloned() {
                Some(TyKind::Projection {
                    self_ty,
                    trait_id,
                    trait_args,
                    name,
                }) if pattern_trait == trait_id
                    && pattern_name == name
                    && pattern_args.len() == trait_args.len() =>
                {
                    self.match_extension_type_pattern(pattern_self, self_ty, substitutions)
                        && pattern_args
                            .iter()
                            .zip(trait_args)
                            .all(|(pattern, actual)| {
                                self.match_extension_type_pattern(*pattern, actual, substitutions)
                            })
                }
                _ => false,
            },
            Some(TyKind::Primitive(_)) | Some(TyKind::ComptimeOnly | TyKind::Error) | None => {
                self.types_match(pattern, actual)
            }
        }
    }

    fn extension_pattern_generics_are_bound(
        &self,
        pattern: InternedTyId,
        substitutions: &HashMap<String, InternedTyId>,
    ) -> bool {
        match self.extension_ty_kind(pattern) {
            Some(TyKind::GenericParam(name)) => substitutions.contains_key(name),
            Some(TyKind::Pointer { elem, .. } | TyKind::Slice { elem, .. }) => {
                self.extension_pattern_generics_are_bound(*elem, substitutions)
            }
            Some(TyKind::Array { elem, .. }) => {
                self.extension_pattern_generics_are_bound(*elem, substitutions)
            }
            Some(TyKind::Range { bound, .. }) => bound.is_none_or(|bound| {
                self.extension_pattern_generics_are_bound(bound, substitutions)
            }),
            Some(TyKind::FunctionPointer {
                params,
                return_type,
                ..
            }) => {
                params
                    .iter()
                    .all(|param| self.extension_pattern_generics_are_bound(*param, substitutions))
                    && self.extension_pattern_generics_are_bound(*return_type, substitutions)
            }
            Some(TyKind::Optional { elem }) => {
                self.extension_pattern_generics_are_bound(*elem, substitutions)
            }
            Some(TyKind::ErrorUnion { error, value }) => {
                self.extension_pattern_generics_are_bound(*error, substitutions)
                    && self.extension_pattern_generics_are_bound(*value, substitutions)
            }
            Some(TyKind::Nominal { args, .. } | TyKind::BuiltinTrait { args, .. }) => args
                .iter()
                .all(|arg| self.extension_pattern_generics_are_bound(*arg, substitutions)),
            Some(TyKind::TraitObject {
                trait_args,
                associated_type_bindings,
                ..
            }) => {
                trait_args
                    .iter()
                    .all(|arg| self.extension_pattern_generics_are_bound(*arg, substitutions))
                    && associated_type_bindings.iter().all(|binding| {
                        binding.trait_args.iter().all(|arg| {
                            self.extension_pattern_generics_are_bound(*arg, substitutions)
                        }) && self.extension_pattern_generics_are_bound(binding.ty, substitutions)
                    })
            }
            Some(TyKind::Projection {
                self_ty,
                trait_args,
                ..
            }) => {
                self.extension_pattern_generics_are_bound(*self_ty, substitutions)
                    && trait_args
                        .iter()
                        .all(|arg| self.extension_pattern_generics_are_bound(*arg, substitutions))
            }
            Some(TyKind::Error | TyKind::ComptimeOnly | TyKind::Primitive(_)) | None => true,
        }
    }

    pub(crate) fn types_match(&mut self, left: InternedTyId, right: InternedTyId) -> bool {
        let left = self.normalize_type_for_match(left);
        let right = self.normalize_type_for_match(right);
        if left == right {
            return true;
        }
        match (self.ty_kind(left).cloned(), self.ty_kind(right).cloned()) {
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
            ) => left_const == right_const && self.types_match(left_elem, right_elem),
            (
                Some(TyKind::Array {
                    len: left_len,
                    elem: left_elem,
                }),
                Some(TyKind::Array {
                    len: right_len,
                    elem: right_elem,
                }),
            ) => left_len == right_len && self.types_match(left_elem, right_elem),
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
                    && left_params
                        .iter()
                        .zip(&right_params)
                        .all(|(left, right)| self.types_match(*left, *right))
                    && self.types_match(left_return, right_return)
            }
            (
                Some(TyKind::Optional { elem: left_elem }),
                Some(TyKind::Optional { elem: right_elem }),
            ) => self.types_match(left_elem, right_elem),
            (
                Some(TyKind::ErrorUnion {
                    error: left_error,
                    value: left_value,
                }),
                Some(TyKind::ErrorUnion {
                    error: right_error,
                    value: right_value,
                }),
            ) => {
                self.types_match(left_error, right_error)
                    && self.types_match(left_value, right_value)
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
                    && left_args
                        .iter()
                        .zip(&right_args)
                        .all(|(left, right)| self.types_match(*left, *right))
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
                    && left_args
                        .iter()
                        .zip(&right_args)
                        .all(|(left, right)| self.types_match(*left, *right))
            }
            (
                Some(TyKind::TraitObject {
                    is_readonly: left_const,
                    trait_id: left_trait,
                    trait_args: left_args,
                    associated_type_bindings: left_bindings,
                }),
                Some(TyKind::TraitObject {
                    is_readonly: right_const,
                    trait_id: right_trait,
                    trait_args: right_args,
                    associated_type_bindings: right_bindings,
                }),
            ) => {
                left_const == right_const
                    && left_trait == right_trait
                    && left_args.len() == right_args.len()
                    && left_bindings.len() == right_bindings.len()
                    && left_args
                        .iter()
                        .zip(&right_args)
                        .all(|(left, right)| self.types_match(*left, *right))
                    && left_bindings.iter().all(|left_binding| {
                        right_bindings
                            .iter()
                            .find(|right_binding| {
                                self.associated_type_binding_keys_match(left_binding, right_binding)
                            })
                            .is_some_and(|right_binding| {
                                self.types_match(left_binding.ty, right_binding.ty)
                            })
                    })
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
                        (Some(left_bound), Some(right_bound)) => {
                            self.types_match(left_bound, right_bound)
                        }
                        (None, None) => true,
                        _ => false,
                    }
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
                    && self.types_match(left_self, right_self)
                    && left_args
                        .iter()
                        .zip(&right_args)
                        .all(|(left, right)| self.types_match(*left, *right))
            }
            _ => false,
        }
    }

    fn associated_type_binding_keys_match(
        &mut self,
        left: &nia_ty::AssociatedTypeBindingTy,
        right: &nia_ty::AssociatedTypeBindingTy,
    ) -> bool {
        left.name == right.name
            && left.trait_id == right.trait_id
            && left.trait_args.len() == right.trait_args.len()
            && left
                .trait_args
                .iter()
                .zip(&right.trait_args)
                .all(|(left, right)| self.types_match(*left, *right))
    }
}
