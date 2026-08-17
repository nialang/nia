// SPDX-License-Identifier: GPL-3.0-or-later
use std::collections::HashSet;

use crate::{
    ExtensionTraitMethodCandidate, ModuleLowerer, TypeInstantiationKey, TypeSubstitutionId,
};
use nia_backend_ir::{BackendFunction, BackendParam};
use nia_function_ir::{
    FunctionArrayElements, FunctionAsmInput, FunctionAsmOutput, FunctionBinding, FunctionBody,
    FunctionCallee, FunctionDeferBody, FunctionExpr, FunctionExprKind, FunctionFieldInit,
    FunctionForHeader, FunctionInlineAsm, FunctionLocal, FunctionOp, FunctionPlace,
    FunctionPlaceBase, FunctionPlaceElem, FunctionRange, FunctionSliceRange, FunctionTerminator,
};
use nia_ids::{BuiltinTrait, BuiltinTraitMethod, GlobalDefId, InternedTyId, ModuleId, TraitId};
use nia_symbol::{SymbolId, SymbolMap};
use nia_trait_solve::{AssociatedTypeProjectionEq, TraitGoal, TraitResolution, TraitSolverContext};
use nia_ty::{LayoutBuiltin, PrimitiveTy, TyKind};

mod function_body_instantiation;
mod trait_resolution;

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
struct ProjectionInstantiationKey {
    self_ty: InternedTyId,
    trait_id: nia_ty::TraitId,
    trait_args: Vec<InternedTyId>,
    trait_const_args: Vec<nia_ty::ConstGenericArg>,
    name: SymbolId,
}

pub(crate) struct FunctionBodyInstantiation<'a> {
    pub(crate) function: nia_ids::GlobalDefId,
    pub(crate) module_id: ModuleId,
    pub(crate) is_instance: bool,
    pub(crate) type_arg_count: usize,
    pub(crate) body: FunctionBody,
    pub(crate) self_arg: Option<InternedTyId>,
    pub(crate) substitutions: &'a SymbolMap<InternedTyId>,
    pub(crate) const_substitutions: &'a SymbolMap<nia_ty::ConstGenericArg>,
}

impl<'a> ModuleLowerer<'a> {
    fn normalized_program_type_for_ty(&self, ty: InternedTyId) -> Option<InternedTyId> {
        if matches!(self.ty_kind(ty), Some(TyKind::Error)) {
            return None;
        }
        if let Some(normalized) = self.input.type_normalization.normalized.get(&ty) {
            return Some(*normalized);
        }
        self.input.program.normalized_type(ty)
    }

    pub(crate) fn normalized_type_from_module(
        &self,
        module_id: ModuleId,
        ty: InternedTyId,
    ) -> InternedTyId {
        let kind = self.ty_kind(ty).unwrap_or_else(|| {
            panic!("Nia ICE: backend type {ty:?} is missing from the session store")
        });
        if matches!(kind, TyKind::Error) {
            return ty;
        }
        if module_id == self.input.module_id {
            return self.input.type_normalization.normalize(ty);
        }
        self.input
            .program
            .normalized_type_from_module(module_id, ty)
            .unwrap_or(ty)
    }

    fn instantiate_external_type_alias(
        &mut self,
        def_id: GlobalDefId,
        args: &[InternedTyId],
        const_args: &[nia_ty::ConstGenericArg],
        substitutions: TypeSubstitutionId,
        active_projections: &mut HashSet<ProjectionInstantiationKey>,
    ) -> Option<InternedTyId> {
        let alias = self.input.program.type_aliases().get(&def_id)?.clone();
        if alias.signature.generics.len() != args.len() + const_args.len() {
            return Some(self.type_context.append.intern(TyKind::Error));
        }
        let (alias_substitutions, alias_const_substitutions) =
            self.generic_substitutions_and_consts_for_def(def_id, args, const_args);
        let alias_substitutions = self
            .intern_type_and_const_substitutions(&alias_substitutions, &alias_const_substitutions);
        let target = self.normalized_type_from_module(def_id.module_id, alias.signature.target);
        let target =
            self.instantiate_ty_with_id_inner(target, alias_substitutions, active_projections);
        Some(self.instantiate_ty_with_id_inner(target, substitutions, active_projections))
    }

    pub(crate) fn generic_substitutions(
        generics: &[SymbolId],
        args: &[InternedTyId],
    ) -> SymbolMap<InternedTyId> {
        generics.iter().cloned().zip(args.iter().copied()).collect()
    }

    /// Rebuilds type and const substitutions from declaration-kind metadata.
    ///
    /// Instance identities store type and const arguments in separate vectors,
    /// while source declarations may interleave their parameters. Walking the
    /// effective declaration order with independent cursors prevents a const
    /// parameter from consuming the next type argument.
    pub(crate) fn generic_substitutions_and_consts_for_def(
        &mut self,
        def_id: GlobalDefId,
        args: &[InternedTyId],
        const_args: &[nia_ty::ConstGenericArg],
    ) -> (SymbolMap<InternedTyId>, SymbolMap<nia_ty::ConstGenericArg>) {
        let Some(generic_params) = self.effective_generic_params_for_def(def_id) else {
            return (SymbolMap::default(), SymbolMap::default());
        };
        let mut type_index = 0;
        let mut const_index = 0;
        let mut substitutions = SymbolMap::default();
        let mut const_substitutions = SymbolMap::default();
        for generic in &generic_params {
            match generic.kind {
                nia_ast::GenericParamKind::Type => {
                    if let Some(arg) = args.get(type_index).copied() {
                        substitutions.insert(generic.name, arg);
                    }
                    type_index += 1;
                }
                nia_ast::GenericParamKind::Const { .. } => {
                    if let Some(arg) = const_args.get(const_index).cloned() {
                        const_substitutions.insert(generic.name, arg);
                    }
                    const_index += 1;
                }
            }
        }
        (substitutions, const_substitutions)
    }

    fn effective_generic_params_for_def(
        &self,
        def_id: GlobalDefId,
    ) -> Option<Vec<nia_ast::GenericParam>> {
        let def = crate::program_def(self.input, def_id)?;
        let mut generic_params = def
            .parent
            .and_then(|parent| {
                crate::program_def(
                    self.input,
                    GlobalDefId {
                        module_id: def_id.module_id,
                        def_id: parent,
                    },
                )
            })
            .map(|parent| parent.generic_params)
            .unwrap_or_default();
        generic_params.extend(def.generic_params);
        Some(generic_params)
    }

    pub(crate) fn effective_generics(
        &mut self,
        def_id: GlobalDefId,
        own_generics: &[SymbolId],
    ) -> &[SymbolId] {
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
        own_generics: &[SymbolId],
    ) -> Vec<SymbolId> {
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
            let mut generics = self
                .input
                .defs
                .defs
                .get(def_id.def_id)
                .and_then(|def| def.parent)
                .and_then(|parent| self.input.defs.defs.get(parent))
                .map(|parent| parent.generics.clone())
                .unwrap_or_default();
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

    fn extension_method_impl_generics(&self, def_id: GlobalDefId) -> Option<Vec<SymbolId>> {
        self.extension_generics_by_method
            .get(&def_id)
            .or_else(|| {
                self.shared
                    .program_extension_generics_by_method
                    .get(&def_id)
            })
            .cloned()
    }

    fn program_trait_method_generics(
        &self,
        def_id: GlobalDefId,
        own_generics: &[SymbolId],
    ) -> Option<Vec<SymbolId>> {
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
                        } == def_id
                    })
                    .then(|| {
                        let mut generics = signature.signature.generics.clone();
                        generics.extend(own_generics.iter().cloned());
                        generics
                    })
            })
    }

    pub(crate) fn normalize_instance_arg_type(&self, ty: InternedTyId) -> InternedTyId {
        let kind = self.ty_kind(ty).unwrap_or_else(|| {
            panic!("Nia ICE: backend type {ty:?} is missing from the session store")
        });
        if matches!(kind, TyKind::Error) {
            return ty;
        }
        self.normalized_program_type_for_ty(ty).unwrap_or(ty)
    }

    pub(crate) fn normalize_const_generic_arg(
        &self,
        arg: &nia_ty::ConstGenericArg,
    ) -> nia_ty::ConstGenericArg {
        nia_ty::ConstGenericArg {
            ty: self.normalize_instance_arg_type(arg.ty),
            value: arg.value.clone(),
        }
    }

    pub(crate) fn canonicalize_instance_const_arg(
        &self,
        arg: &nia_ty::ConstGenericArg,
    ) -> nia_ty::ConstGenericArg {
        let mut arg = self.normalize_const_generic_arg(arg);
        if let nia_ty::ConstGenericValue::ConstExpr(id) = arg.value
            && let Some(value) = self
                .input
                .program
                .const_array_lengths(id.module_id)
                .and_then(|lengths| lengths.get(&id))
        {
            arg.value = nia_ty::ConstGenericValue::Int(i128::from(*value).into());
        }
        arg
    }

    fn instantiate_const_generic_arg_with_id(
        &mut self,
        arg: &nia_ty::ConstGenericArg,
        substitutions: TypeSubstitutionId,
        active_projections: &mut HashSet<ProjectionInstantiationKey>,
    ) -> nia_ty::ConstGenericArg {
        if let nia_ty::ConstGenericValue::GenericParam(name) = &arg.value
            && let Some(substituted) = self.const_substitution(substitutions, name)
        {
            return self.instantiate_const_generic_arg_with_id(
                &substituted,
                substitutions,
                active_projections,
            );
        }
        nia_ty::ConstGenericArg {
            ty: self.instantiate_ty_with_id_inner(arg.ty, substitutions, active_projections),
            value: arg.value.clone(),
        }
    }

    fn instantiate_const_generic_arg(
        &mut self,
        arg: &nia_ty::ConstGenericArg,
        substitutions: TypeSubstitutionId,
    ) -> nia_ty::ConstGenericArg {
        self.instantiate_const_generic_arg_with_id(arg, substitutions, &mut HashSet::new())
    }

    fn const_generic_expr_from_arg(&mut self, arg: nia_ty::ConstGenericArg) -> FunctionExprKind {
        match arg.value {
            nia_ty::ConstGenericValue::Int(value) => {
                let ty = self.ty_kind(arg.ty).cloned();
                match ty {
                    Some(TyKind::Primitive(PrimitiveTy::Usize)) => FunctionExprKind::BuiltinValue(
                        nia_function_ir::FunctionBuiltinValue::Usize(value.bits() as u64),
                    ),
                    _ => FunctionExprKind::BuiltinValue(
                        nia_function_ir::FunctionBuiltinValue::Int(value),
                    ),
                }
            }
            nia_ty::ConstGenericValue::Bool(value) => FunctionExprKind::Bool(value),
            nia_ty::ConstGenericValue::Char(value) => FunctionExprKind::Char(value as u32),
            nia_ty::ConstGenericValue::GenericParam(_)
            | nia_ty::ConstGenericValue::ConstExpr(_) => FunctionExprKind::ConstGeneric(arg),
        }
    }

    pub(crate) fn instantiate_params_with_id(
        &mut self,
        function: &BackendFunction,
        substitutions: TypeSubstitutionId,
    ) -> Vec<BackendParam> {
        function
            .params
            .iter()
            .map(|param| BackendParam {
                local_id: param.local_id,
                name: param.name,
                receiver: param.receiver,
                passing_ty: self.instantiate_ty_with_id(param.passing_ty, substitutions),
                local_ty: self.instantiate_ty_with_id(param.local_ty, substitutions),
                span: param.span,
            })
            .collect()
    }

    pub(crate) fn instantiate_function_body(
        &mut self,
        input: FunctionBodyInstantiation<'_>,
    ) -> FunctionBody {
        let FunctionBodyInstantiation {
            function,
            module_id: instantiation_module_id,
            is_instance,
            type_arg_count,
            body,
            self_arg,
            substitutions,
            const_substitutions,
        } = input;
        let instantiation_snapshot = self.instantiation.take_snapshot();
        let substitutions = self.intern_type_and_const_substitutions_with_self(
            self_arg,
            substitutions,
            const_substitutions,
        );
        self.instantiation.set_instance_scope(
            function,
            instantiation_module_id,
            substitutions,
            !is_instance || type_arg_count == 0,
        );
        if instantiation_module_id != self.input.module_id
            && let Some(extensions) = self.input.program.extensions(instantiation_module_id)
        {
            self.instantiation.extension_trait_method_candidates = Some((
                instantiation_module_id,
                crate::index_extension_trait_method_candidates(extensions, instantiation_module_id),
            ));
        }
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
        let body = self.optimize_function_body(function, is_instance, type_arg_count, body);
        self.instantiation.restore(instantiation_snapshot);
        body
    }

    pub(super) fn current_arg_module_id(&self) -> ModuleId {
        self.instantiation
            .instantiation_module_id
            .unwrap_or(self.input.module_id)
    }

    pub(crate) fn match_extension_trait_impl_candidate(
        &mut self,
        candidate: &ExtensionTraitMethodCandidate,
        trait_args: &[InternedTyId],
        trait_const_args: &[nia_ty::ConstGenericArg],
        self_ty: InternedTyId,
    ) -> Option<SymbolMap<InternedTyId>> {
        let mut substitutions = SymbolMap::default();
        let target_ty = self.normalized_type_from_module(candidate.module_id, candidate.target_ty);
        if !self.match_extension_type_pattern(target_ty, self_ty, &mut substitutions) {
            return None;
        }
        if candidate.trait_args.len() != trait_args.len() {
            return None;
        }
        if !self.const_trait_args_match(&candidate.trait_const_args, trait_const_args) {
            return None;
        }
        let candidate_trait_args = candidate
            .trait_args
            .iter()
            .map(|arg| self.normalized_type_from_module(candidate.module_id, *arg))
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

    /// Matches concrete const arguments against an impl pattern. Generic
    /// pattern values remain open; the trait solver resolves their bindings.
    pub(crate) fn const_trait_args_match(
        &self,
        pattern: &[nia_ty::ConstGenericArg],
        actual: &[nia_ty::ConstGenericArg],
    ) -> bool {
        pattern.len() == actual.len()
            && pattern.iter().zip(actual).all(|(pattern, actual)| {
                matches!(pattern.value, nia_ty::ConstGenericValue::GenericParam(_))
                    || pattern.value == actual.value
            })
    }

    pub(crate) fn normalize_where_predicates(
        &self,
        predicates: &[nia_item_signatures::WherePredicateSignature],
        module_id: ModuleId,
    ) -> Vec<nia_item_signatures::WherePredicateSignature> {
        predicates
            .iter()
            .map(|predicate| nia_item_signatures::WherePredicateSignature {
                ty: self.normalized_type_from_module(module_id, predicate.ty),
                bounds: predicate
                    .bounds
                    .iter()
                    .map(|bound| nia_item_signatures::WhereBoundSignature {
                        trait_ty: self.normalized_type_from_module(module_id, bound.trait_ty),
                        associated_type_bindings: bound
                            .associated_type_bindings
                            .iter()
                            .map(
                                |binding| nia_item_signatures::AssociatedTypeBindingSignature {
                                    name: binding.name,
                                    ty: self.normalized_type_from_module(module_id, binding.ty),
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
        substitutions: &SymbolMap<InternedTyId>,
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
                                name: binding.name,
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
    ) -> &'b [SymbolId] {
        &candidate.effective_generics
    }

    pub(crate) fn trait_method_call_is_concrete(
        &mut self,
        self_ty: InternedTyId,
        trait_args: &[InternedTyId],
        trait_const_args: &[nia_ty::ConstGenericArg],
        method_args: &[InternedTyId],
    ) -> bool {
        !self.ty_contains_generic_param(self_ty)
            && !trait_args
                .iter()
                .chain(method_args)
                .any(|arg| self.ty_contains_generic_param(*arg))
            && !trait_const_args.iter().any(|arg| {
                self.ty_contains_generic_param(arg.ty)
                    || matches!(arg.value, nia_ty::ConstGenericValue::GenericParam(_))
            })
    }

    pub(crate) fn trait_method_call_requires_concrete_impl(
        &mut self,
        self_ty: InternedTyId,
        trait_id: GlobalDefId,
        trait_args: &[InternedTyId],
        method_args: &[InternedTyId],
    ) -> bool {
        if self.instantiation.defer_concrete_trait_diagnostics {
            return false;
        }
        self.trait_method_call_is_concrete(self_ty, trait_args, &[], method_args)
            && !self.source_trait_goal_is_satisfied(trait_id, trait_args, self_ty)
    }

    pub(crate) fn builtin_trait_method_call_requires_concrete_impl(
        &mut self,
        self_ty: InternedTyId,
        trait_id: BuiltinTrait,
        trait_args: &[InternedTyId],
        method_args: &[InternedTyId],
    ) -> bool {
        if self.instantiation.defer_concrete_trait_diagnostics {
            return false;
        }
        self.trait_method_call_is_concrete(self_ty, trait_args, &[], method_args)
            && !self.builtin_trait_goal_is_satisfied(trait_id, trait_args, self_ty)
    }

    pub(crate) fn default_trait_method_self_arg(
        &mut self,
        trait_id: GlobalDefId,
        trait_args: &[InternedTyId],
        self_ty: InternedTyId,
    ) -> InternedTyId {
        if self.source_trait_goal_is_satisfied(trait_id, trait_args, self_ty) {
            return self_ty;
        }
        if let Some(pointee) = self.pointer_elem_ty(self_ty)
            && self.source_trait_goal_is_satisfied(trait_id, trait_args, pointee)
        {
            return pointee;
        }
        self_ty
    }

    fn source_trait_goal_is_satisfied(
        &mut self,
        trait_id: GlobalDefId,
        trait_args: &[InternedTyId],
        self_ty: InternedTyId,
    ) -> bool {
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
        let goal = TraitGoal {
            self_ty,
            trait_id: TraitId::Source(trait_id),
            trait_args: trait_args.to_vec(),
            trait_const_args: Vec::new(),
        };
        solver.proves(goal)
    }

    fn builtin_trait_goal_is_satisfied(
        &mut self,
        trait_id: BuiltinTrait,
        trait_args: &[InternedTyId],
        self_ty: InternedTyId,
    ) -> bool {
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
        solver.proves(TraitGoal {
            self_ty,
            trait_id: TraitId::Builtin(trait_id),
            trait_args: trait_args.to_vec(),
            trait_const_args: Vec::new(),
        })
    }

    fn ty_contains_generic_param(&mut self, ty: InternedTyId) -> bool {
        let ty = self.normalize_instance_arg_type(ty);
        let mut ty_kind = |ty: InternedTyId| self.ty_kind(ty).cloned();
        crate::function_instances::contains_generic_param(ty, &mut ty_kind, None)
    }

    pub(crate) fn instantiate_ty(
        &mut self,
        ty: InternedTyId,
        substitutions: &SymbolMap<InternedTyId>,
    ) -> InternedTyId {
        let substitutions = self.intern_type_substitutions(substitutions);
        self.instantiate_ty_with_id(ty, substitutions)
    }

    pub(crate) fn instantiate_ty_with_id(
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
        let ty = self.normalize_instance_arg_type(ty);
        let key = TypeInstantiationKey {
            ty,
            substitutions,
            current_function: self.instantiation.function,
        };
        let can_use_cache = active_projections.is_empty();
        if can_use_cache && let Some(instantiated) = self.type_context.type_instantiation(&key) {
            return instantiated;
        }
        match self.ty_kind(ty).cloned() {
            Some(TyKind::GenericParam(name)) => {
                let instantiated = self.type_substitution(substitutions, &name).unwrap_or(ty);
                self.finish_type_instantiation(key, instantiated, can_use_cache)
            }
            Some(TyKind::SelfParam) => {
                let instantiated = self.self_substitution(substitutions).unwrap_or(ty);
                self.finish_type_instantiation(key, instantiated, can_use_cache)
            }
            Some(TyKind::Opaque) => self.finish_type_instantiation(key, ty, can_use_cache),
            Some(TyKind::ClosureState {
                closure_id,
                captures,
                params,
                return_type,
            }) => {
                let captures = captures
                    .into_iter()
                    .map(|capture| {
                        self.instantiate_ty_with_id_inner(
                            capture,
                            substitutions,
                            active_projections,
                        )
                    })
                    .collect();
                let params = params
                    .into_iter()
                    .map(|param| {
                        self.instantiate_ty_with_id_inner(param, substitutions, active_projections)
                    })
                    .collect();
                let return_type = self.instantiate_ty_with_id_inner(
                    return_type,
                    substitutions,
                    active_projections,
                );
                let instantiated = self.type_context.append.intern(TyKind::ClosureState {
                    closure_id,
                    captures,
                    params,
                    return_type,
                });
                self.finish_type_instantiation(key, instantiated, can_use_cache)
            }
            Some(TyKind::Tuple(elems)) => {
                let elems = elems
                    .into_iter()
                    .map(|elem| {
                        self.instantiate_ty_with_id_inner(elem, substitutions, active_projections)
                    })
                    .collect();
                let instantiated = self.type_context.append.intern(TyKind::Tuple(elems));
                self.finish_type_instantiation(key, instantiated, can_use_cache)
            }
            Some(TyKind::Pointer { is_readonly, elem }) => {
                let elem =
                    self.instantiate_ty_with_id_inner(elem, substitutions, active_projections);
                let instantiated = match self.ty_kind(elem).cloned() {
                    Some(TyKind::SlicePointee { elem }) => self
                        .type_context
                        .append
                        .intern(TyKind::Slice { is_readonly, elem }),
                    Some(TyKind::TraitObjectPointee {
                        trait_id,
                        trait_args,
                        trait_const_args,
                        associated_type_bindings,
                    }) => self.type_context.append.intern(TyKind::TraitObject {
                        is_readonly,
                        trait_id,
                        trait_args,
                        trait_const_args,
                        associated_type_bindings,
                    }),
                    Some(TyKind::CallablePointee {
                        params,
                        return_type,
                    }) => self.type_context.append.intern(TyKind::Callable {
                        is_readonly,
                        params,
                        return_type,
                    }),
                    _ => self
                        .type_context
                        .append
                        .intern(TyKind::Pointer { is_readonly, elem }),
                };
                self.finish_type_instantiation(key, instantiated, can_use_cache)
            }
            Some(TyKind::VolatilePointer { is_readonly, elem }) => {
                let elem =
                    self.instantiate_ty_with_id_inner(elem, substitutions, active_projections);
                let instantiated = self
                    .type_context
                    .append
                    .intern(TyKind::VolatilePointer { is_readonly, elem });
                self.finish_type_instantiation(key, instantiated, can_use_cache)
            }
            Some(TyKind::Slice { is_readonly, elem }) => {
                let elem =
                    self.instantiate_ty_with_id_inner(elem, substitutions, active_projections);
                let instantiated = self
                    .type_context
                    .append
                    .intern(TyKind::Slice { is_readonly, elem });
                self.finish_type_instantiation(key, instantiated, can_use_cache)
            }
            Some(TyKind::SlicePointee { elem }) => {
                let elem =
                    self.instantiate_ty_with_id_inner(elem, substitutions, active_projections);
                let instantiated = self
                    .type_context
                    .append
                    .intern(TyKind::SlicePointee { elem });
                self.finish_type_instantiation(key, instantiated, can_use_cache)
            }
            Some(TyKind::Array { len, elem }) => {
                let len = self.instantiate_array_len(len, substitutions);
                let elem =
                    self.instantiate_ty_with_id_inner(elem, substitutions, active_projections);
                let instantiated = self.type_context.append.intern(TyKind::Array { len, elem });
                self.finish_type_instantiation(key, instantiated, can_use_cache)
            }
            Some(TyKind::Range { kind, bound }) => {
                let bound = bound.map(|bound| {
                    self.instantiate_ty_with_id_inner(bound, substitutions, active_projections)
                });
                let instantiated = self
                    .type_context
                    .append
                    .intern(TyKind::Range { kind, bound });
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
                let instantiated = self.type_context.append.intern(TyKind::FunctionPointer {
                    params,
                    return_type,
                    is_variadic,
                });
                self.finish_type_instantiation(key, instantiated, can_use_cache)
            }
            Some(TyKind::Callable {
                is_readonly,
                params,
                return_type,
            }) => {
                let params = params
                    .into_iter()
                    .map(|param| {
                        self.instantiate_ty_with_id_inner(param, substitutions, active_projections)
                    })
                    .collect();
                let return_type = self.instantiate_ty_with_id_inner(
                    return_type,
                    substitutions,
                    active_projections,
                );
                let instantiated = self.type_context.append.intern(TyKind::Callable {
                    is_readonly,
                    params,
                    return_type,
                });
                self.finish_type_instantiation(key, instantiated, can_use_cache)
            }
            Some(TyKind::CallablePointee {
                params,
                return_type,
            }) => {
                let params = params
                    .into_iter()
                    .map(|param| {
                        self.instantiate_ty_with_id_inner(param, substitutions, active_projections)
                    })
                    .collect();
                let return_type = self.instantiate_ty_with_id_inner(
                    return_type,
                    substitutions,
                    active_projections,
                );
                let instantiated = self.type_context.append.intern(TyKind::CallablePointee {
                    params,
                    return_type,
                });
                self.finish_type_instantiation(key, instantiated, can_use_cache)
            }
            Some(TyKind::Optional { elem }) => {
                let elem =
                    self.instantiate_ty_with_id_inner(elem, substitutions, active_projections);
                let instantiated = self.type_context.append.intern(TyKind::Optional { elem });
                self.finish_type_instantiation(key, instantiated, can_use_cache)
            }
            Some(TyKind::ErrorUnion { error, value }) => {
                let error =
                    self.instantiate_ty_with_id_inner(error, substitutions, active_projections);
                let value =
                    self.instantiate_ty_with_id_inner(value, substitutions, active_projections);
                let instantiated = self
                    .type_context
                    .append
                    .intern(TyKind::ErrorUnion { error, value });
                self.finish_type_instantiation(key, instantiated, can_use_cache)
            }
            Some(TyKind::Nominal {
                def_id,
                args,
                const_args,
            }) => {
                let args = args
                    .iter()
                    .copied()
                    .map(|arg| {
                        self.instantiate_ty_with_id_inner(arg, substitutions, active_projections)
                    })
                    .collect::<Vec<_>>();
                let const_args = const_args
                    .iter()
                    .map(|arg| {
                        self.instantiate_const_generic_arg_with_id(
                            arg,
                            substitutions,
                            active_projections,
                        )
                    })
                    .collect::<Vec<_>>();
                if let Some(instantiated) = self.instantiate_external_type_alias(
                    def_id,
                    &args,
                    &const_args,
                    substitutions,
                    active_projections,
                ) {
                    return self.finish_type_instantiation(key, instantiated, can_use_cache);
                }
                let instantiated = self.type_context.append.intern(TyKind::Nominal {
                    def_id,
                    args,
                    const_args,
                });
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
                    .type_context
                    .append
                    .intern(TyKind::BuiltinTrait { trait_id, args });
                self.finish_type_instantiation(key, instantiated, can_use_cache)
            }
            Some(TyKind::BuiltinType(_)) => self.finish_type_instantiation(key, ty, can_use_cache),
            Some(TyKind::TraitObject {
                is_readonly,
                trait_id,
                trait_args,
                trait_const_args,
                associated_type_bindings,
            }) => {
                let trait_args = trait_args
                    .iter()
                    .copied()
                    .map(|arg| {
                        self.instantiate_ty_with_id_inner(arg, substitutions, active_projections)
                    })
                    .collect::<Vec<_>>();
                let trait_const_args = trait_const_args
                    .iter()
                    .map(|arg| {
                        self.instantiate_const_generic_arg_with_id(
                            arg,
                            substitutions,
                            active_projections,
                        )
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
                        trait_const_args: binding
                            .trait_const_args
                            .iter()
                            .map(|arg| {
                                self.instantiate_const_generic_arg_with_id(
                                    arg,
                                    substitutions,
                                    active_projections,
                                )
                            })
                            .collect(),
                        name: binding.name,
                        ty: self.instantiate_ty_with_id_inner(
                            binding.ty,
                            substitutions,
                            active_projections,
                        ),
                    })
                    .collect();
                let instantiated = self.type_context.append.intern(TyKind::TraitObject {
                    is_readonly,
                    trait_id,
                    trait_args,
                    trait_const_args,
                    associated_type_bindings,
                });
                self.finish_type_instantiation(key, instantiated, can_use_cache)
            }
            Some(TyKind::TraitObjectPointee {
                trait_id,
                trait_args,
                trait_const_args,
                associated_type_bindings,
            }) => {
                let trait_args = trait_args
                    .iter()
                    .copied()
                    .map(|arg| {
                        self.instantiate_ty_with_id_inner(arg, substitutions, active_projections)
                    })
                    .collect::<Vec<_>>();
                let trait_const_args = trait_const_args
                    .iter()
                    .map(|arg| {
                        self.instantiate_const_generic_arg_with_id(
                            arg,
                            substitutions,
                            active_projections,
                        )
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
                        trait_const_args: binding
                            .trait_const_args
                            .iter()
                            .map(|arg| {
                                self.instantiate_const_generic_arg_with_id(
                                    arg,
                                    substitutions,
                                    active_projections,
                                )
                            })
                            .collect(),
                        name: binding.name,
                        ty: self.instantiate_ty_with_id_inner(
                            binding.ty,
                            substitutions,
                            active_projections,
                        ),
                    })
                    .collect();
                let instantiated = self.type_context.append.intern(TyKind::TraitObjectPointee {
                    trait_id,
                    trait_args,
                    trait_const_args,
                    associated_type_bindings,
                });
                self.finish_type_instantiation(key, instantiated, can_use_cache)
            }
            Some(TyKind::Projection {
                self_ty,
                trait_id,
                trait_args,
                trait_const_args,
                name,
            }) => {
                let original_self_ty = self_ty;
                let original_trait_args = trait_args.clone();
                let original_trait_const_args = trait_const_args.clone();
                let self_ty =
                    self.instantiate_ty_with_id_inner(self_ty, substitutions, active_projections);
                let trait_args = trait_args
                    .iter()
                    .copied()
                    .map(|arg| {
                        self.instantiate_ty_with_id_inner(arg, substitutions, active_projections)
                    })
                    .collect::<Vec<_>>();
                let trait_const_args = trait_const_args
                    .iter()
                    .map(|arg| {
                        self.instantiate_const_generic_arg_with_id(
                            arg,
                            substitutions,
                            active_projections,
                        )
                    })
                    .collect::<Vec<_>>();
                let projection_key = ProjectionInstantiationKey {
                    self_ty,
                    trait_id,
                    trait_args: trait_args.clone(),
                    trait_const_args: trait_const_args.clone(),
                    name,
                };
                let projection = self.type_context.append.intern(TyKind::Projection {
                    self_ty,
                    trait_id,
                    trait_args: trait_args.clone(),
                    trait_const_args: trait_const_args.clone(),
                    name,
                });
                if !active_projections.insert(projection_key.clone()) {
                    return projection;
                }
                let resolved = self
                    .resolve_associated_type_projection(
                        &projection_key,
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
                    if self_ty == original_self_ty
                        && trait_args == original_trait_args
                        && trait_const_args == original_trait_const_args
                    {
                        ty
                    } else {
                        projection
                    }
                });
                self.finish_type_instantiation(key, instantiated, can_use_cache)
            }
            Some(TyKind::Error | TyKind::ConstOnly)
            | Some(TyKind::Primitive(_) | TyKind::Vector { .. })
            | None => self.finish_type_instantiation(key, ty, can_use_cache),
        }
    }

    pub(crate) fn intern_type_substitutions(
        &mut self,
        substitutions: &SymbolMap<InternedTyId>,
    ) -> TypeSubstitutionId {
        self.intern_type_and_const_substitutions(substitutions, &SymbolMap::default())
    }

    pub(crate) fn intern_type_and_const_substitutions_with_self(
        &mut self,
        self_arg: Option<InternedTyId>,
        substitutions: &SymbolMap<InternedTyId>,
        const_substitutions: &SymbolMap<nia_ty::ConstGenericArg>,
    ) -> TypeSubstitutionId {
        let self_arg = self_arg.map(|ty| self.normalize_instance_arg_type(ty));
        let mut substitutions = substitutions
            .iter()
            .map(|(name, ty)| (*name, self.normalize_instance_arg_type(*ty)))
            .collect::<Vec<_>>();
        substitutions.sort_by_key(|left| left.0);
        let mut const_substitutions = const_substitutions
            .iter()
            .map(|(name, arg)| (*name, self.normalize_const_generic_arg(arg)))
            .collect::<Vec<_>>();
        const_substitutions.sort_by_key(|left| left.0);
        self.type_context
            .intern_type_substitutions(self_arg, substitutions, const_substitutions)
    }

    pub(crate) fn intern_type_and_const_substitutions(
        &mut self,
        substitutions: &SymbolMap<InternedTyId>,
        const_substitutions: &SymbolMap<nia_ty::ConstGenericArg>,
    ) -> TypeSubstitutionId {
        self.intern_type_and_const_substitutions_with_self(None, substitutions, const_substitutions)
    }

    pub(super) fn empty_type_substitution_id(&mut self) -> TypeSubstitutionId {
        self.intern_type_substitutions(&SymbolMap::default())
    }

    fn normalize_type_for_match(&mut self, ty: InternedTyId) -> InternedTyId {
        let substitutions = self.empty_type_substitution_id();
        self.instantiate_ty_with_id(ty, substitutions)
    }

    fn type_substitution(
        &self,
        substitutions: TypeSubstitutionId,
        name: &SymbolId,
    ) -> Option<InternedTyId> {
        self.type_context.type_substitution(substitutions, name)
    }

    fn self_substitution(&self, substitutions: TypeSubstitutionId) -> Option<InternedTyId> {
        self.type_context.self_substitution(substitutions)
    }

    fn const_substitution(
        &self,
        substitutions: TypeSubstitutionId,
        name: &SymbolId,
    ) -> Option<nia_ty::ConstGenericArg> {
        self.type_context.const_substitution(substitutions, name)
    }

    fn instantiate_array_len(
        &self,
        len: nia_ty::ArrayLenTy,
        substitutions: TypeSubstitutionId,
    ) -> nia_ty::ArrayLenTy {
        match len {
            nia_ty::ArrayLenTy::GenericParam(name) => self
                .const_substitution(substitutions, &name)
                .and_then(|arg| nia_ty::array_len_from_const_arg(&arg))
                .unwrap_or(nia_ty::ArrayLenTy::GenericParam(name)),
            len => len,
        }
    }

    /// Reads concrete type arguments for `def_id` in declaration type-parameter order.
    ///
    /// Generic parameter names include const parameters, but function-instance
    /// identities store type and const arguments separately. Filtering by kind
    /// here keeps implicit instance references aligned with that identity.
    pub(super) fn effective_instance_type_args_for_def(
        &mut self,
        def_id: GlobalDefId,
        substitutions: TypeSubstitutionId,
    ) -> Option<Vec<InternedTyId>> {
        let generic_params = self.effective_generic_params_for_def(def_id)?;
        if generic_params.is_empty() {
            return Some(Vec::new());
        }
        generic_params
            .iter()
            .filter(|generic| matches!(generic.kind, nia_ast::GenericParamKind::Type))
            .map(|generic| self.type_substitution(substitutions, &generic.name))
            .collect::<Option<Vec<_>>>()
            .map(|args| self.canonicalize_instance_args(&args))
    }

    /// Reads concrete const arguments in declaration const-parameter order.
    pub(super) fn effective_instance_const_args_for_def(
        &mut self,
        def_id: GlobalDefId,
        substitutions: TypeSubstitutionId,
    ) -> Option<Vec<nia_ty::ConstGenericArg>> {
        self.effective_generic_params_for_def(def_id)?
            .iter()
            .filter(|generic| matches!(generic.kind, nia_ast::GenericParamKind::Const { .. }))
            .map(|generic| self.const_substitution(substitutions, &generic.name))
            .collect::<Option<Vec<_>>>()
            .map(|args| {
                args.iter()
                    .map(|arg| self.canonicalize_instance_const_arg(arg))
                    .collect()
            })
    }

    pub(super) fn global_instance_args_for_def(
        &mut self,
        def_id: GlobalDefId,
        substitutions: TypeSubstitutionId,
    ) -> Option<(ModuleId, Vec<InternedTyId>, Vec<nia_ty::ConstGenericArg>)> {
        let def = self.input.defs.defs.get(def_id.def_id)?;
        if def.kind != nia_defs::DefKind::Global {
            return None;
        }
        let owner = def.parent?;
        let owner_def_id = GlobalDefId {
            module_id: def_id.module_id,
            def_id: owner,
        };
        let owner_def = self.input.defs.defs.get(owner)?;
        if !matches!(
            owner_def.kind,
            nia_defs::DefKind::Function | nia_defs::DefKind::Method
        ) {
            return None;
        }
        if self.instantiation.function != Some(owner_def_id) {
            return None;
        }
        if self.instantiation.defer_concrete_trait_diagnostics {
            return None;
        }
        let args = self.effective_instance_type_args_for_def(owner_def_id, substitutions)?;
        let const_args = self.effective_instance_const_args_for_def(owner_def_id, substitutions)?;
        if args.is_empty() && const_args.is_empty() {
            None
        } else {
            Some((self.current_arg_module_id(), args, const_args))
        }
    }

    fn cache_type_instantiation(
        &mut self,
        key: TypeInstantiationKey,
        instantiated: InternedTyId,
    ) -> InternedTyId {
        self.type_context
            .cache_type_instantiation(key, instantiated)
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
        projection: &ProjectionInstantiationKey,
        substitutions: TypeSubstitutionId,
        active_projections: &mut HashSet<ProjectionInstantiationKey>,
    ) -> Option<InternedTyId> {
        let associated_type_assumptions =
            self.current_associated_type_assumptions(substitutions, active_projections);
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
        let mut solver =
            context.solver_with_associated_type_assumptions(&[], &associated_type_assumptions);
        solver.resolve_associated_type(
            projection.self_ty,
            projection.trait_id,
            &projection.trait_args,
            &projection.trait_const_args,
            &projection.name,
        )
    }

    fn current_associated_type_assumptions(
        &mut self,
        substitutions: TypeSubstitutionId,
        active_projections: &mut HashSet<ProjectionInstantiationKey>,
    ) -> Vec<AssociatedTypeProjectionEq> {
        let Some(current) = self.instantiation.function else {
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
        let Some(impl_index) = self.trait_impl_index_for_method(current) else {
            return Vec::new();
        };
        let Some(impl_signature) = self.input.program.trait_impls().get(impl_index) else {
            return Vec::new();
        };
        let target_ty =
            self.normalized_type_from_module(impl_signature.module_id, impl_signature.target_ty);
        let target_ty =
            self.instantiate_ty_with_id_inner(target_ty, substitutions, active_projections);
        let trait_id = impl_signature.trait_id;
        let trait_args = impl_signature.trait_args.clone();
        let associated_types = impl_signature.associated_types.clone();
        let trait_args = trait_args
            .into_iter()
            .map(|arg| {
                let actual = self.normalized_type_from_module(impl_signature.module_id, arg);
                self.instantiate_ty_with_id_inner(actual, substitutions, active_projections)
            })
            .collect::<Vec<_>>();
        let trait_const_args = impl_signature
            .trait_const_args
            .iter()
            .map(|arg| {
                self.instantiate_const_generic_arg_with_id(arg, substitutions, active_projections)
            })
            .collect::<Vec<_>>();
        let goal = TraitGoal {
            self_ty: target_ty,
            trait_id,
            trait_args,
            trait_const_args,
        };
        associated_types
            .into_iter()
            .map(|associated_type| {
                let ty =
                    self.normalized_type_from_module(impl_signature.module_id, associated_type.ty);
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
        let Some(substitutions) = self.instantiation.type_substitutions else {
            return Vec::new();
        };
        self.current_associated_type_assumptions(substitutions, &mut HashSet::new())
    }

    pub(crate) fn match_extension_type_pattern(
        &mut self,
        pattern: InternedTyId,
        actual: InternedTyId,
        substitutions: &mut SymbolMap<InternedTyId>,
    ) -> bool {
        let pattern = self.normalize_instance_arg_type(pattern);
        let actual = self.canonicalize_instance_arg(actual);
        if self.extension_pattern_contains_generic(pattern)
            && self.extension_pattern_generics_are_bound(pattern, substitutions)
        {
            let substitution_id = self.intern_type_substitutions(substitutions);
            let mut active_projections = HashSet::new();
            let pattern = self.instantiate_ty_with_id_inner(
                pattern,
                substitution_id,
                &mut active_projections,
            );
            return self.types_match(pattern, actual);
        }
        match self.ty_kind(pattern).cloned() {
            Some(TyKind::GenericParam(name)) => {
                if let Some(existing) = substitutions.get(&name).copied() {
                    self.types_match(existing, actual)
                } else {
                    substitutions.insert(name, actual);
                    true
                }
            }
            Some(TyKind::SelfParam) => self.types_match(pattern, actual),
            Some(TyKind::BuiltinType(pattern_builtin)) => {
                matches!(self.ty_kind(actual), Some(TyKind::BuiltinType(actual_builtin)) if pattern_builtin == *actual_builtin)
            }
            Some(TyKind::Opaque) => matches!(self.ty_kind(actual), Some(TyKind::Opaque)),
            Some(TyKind::Tuple(pattern_elems)) => match self.ty_kind(actual).cloned() {
                Some(TyKind::Tuple(actual_elems)) if pattern_elems.len() == actual_elems.len() => {
                    pattern_elems
                        .iter()
                        .zip(actual_elems)
                        .all(|(pattern_elem, actual_elem)| {
                            self.match_extension_type_pattern(
                                *pattern_elem,
                                actual_elem,
                                substitutions,
                            )
                        })
                }
                _ => false,
            },
            Some(TyKind::Pointer {
                is_readonly: pattern_const,
                elem: pattern_elem,
            }) => match self.ty_kind(actual).cloned() {
                Some(TyKind::Pointer { is_readonly, elem }) if is_readonly == pattern_const => {
                    self.match_extension_type_pattern(pattern_elem, elem, substitutions)
                }
                _ => false,
            },
            Some(TyKind::VolatilePointer {
                is_readonly: pattern_const,
                elem: pattern_elem,
            }) => match self.ty_kind(actual).cloned() {
                Some(TyKind::VolatilePointer { is_readonly, elem })
                    if is_readonly == pattern_const =>
                {
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
            Some(TyKind::SlicePointee { elem: pattern_elem }) => {
                match self.ty_kind(actual).cloned() {
                    Some(TyKind::SlicePointee { elem }) => {
                        self.match_extension_type_pattern(pattern_elem, elem, substitutions)
                    }
                    _ => false,
                }
            }
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
            Some(TyKind::Callable {
                is_readonly: pattern_readonly,
                params: pattern_params,
                return_type: pattern_return,
            }) => match self.ty_kind(actual).cloned() {
                Some(TyKind::Callable {
                    is_readonly,
                    params,
                    return_type,
                }) if pattern_readonly == is_readonly && pattern_params.len() == params.len() => {
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
            Some(TyKind::CallablePointee {
                params: pattern_params,
                return_type: pattern_return,
            }) => match self.ty_kind(actual).cloned() {
                Some(TyKind::CallablePointee {
                    params,
                    return_type,
                }) if pattern_params.len() == params.len() => {
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
                const_args: pattern_const_args,
            }) => match self.ty_kind(actual).cloned() {
                Some(TyKind::Nominal {
                    def_id,
                    args,
                    const_args,
                }) if pattern_def == def_id
                    && pattern_const_args == const_args
                    && pattern_args.len() == args.len() =>
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
                trait_const_args: pattern_const_args,
                associated_type_bindings: pattern_bindings,
            }) => match self.ty_kind(actual).cloned() {
                Some(TyKind::TraitObject {
                    is_readonly,
                    trait_id,
                    trait_args,
                    trait_const_args,
                    associated_type_bindings,
                }) if is_readonly == pattern_const
                    && trait_id == pattern_trait
                    && pattern_args.len() == trait_args.len()
                    && pattern_const_args == trait_const_args
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
            Some(TyKind::TraitObjectPointee {
                trait_id: pattern_trait,
                trait_args: pattern_args,
                trait_const_args: pattern_const_args,
                associated_type_bindings: pattern_bindings,
            }) => match self.ty_kind(actual).cloned() {
                Some(TyKind::TraitObjectPointee {
                    trait_id,
                    trait_args,
                    trait_const_args,
                    associated_type_bindings,
                }) if trait_id == pattern_trait
                    && pattern_args.len() == trait_args.len()
                    && pattern_const_args == trait_const_args
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
                trait_const_args: pattern_const_args,
                name: pattern_name,
            }) => match self.ty_kind(actual).cloned() {
                Some(TyKind::Projection {
                    self_ty,
                    trait_id,
                    trait_args,
                    trait_const_args,
                    name,
                }) if pattern_trait == trait_id
                    && pattern_name == name
                    && pattern_args.len() == trait_args.len()
                    && pattern_const_args == trait_const_args =>
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
            Some(TyKind::Primitive(_) | TyKind::Vector { .. })
            | Some(TyKind::ConstOnly | TyKind::Error | TyKind::ClosureState { .. })
            | None => self.types_match(pattern, actual),
        }
    }

    fn extension_pattern_generics_are_bound(
        &self,
        pattern: InternedTyId,
        substitutions: &SymbolMap<InternedTyId>,
    ) -> bool {
        match self.ty_kind(pattern) {
            Some(TyKind::GenericParam(name)) => substitutions.contains_key(name),
            Some(TyKind::SelfParam) => true,
            Some(TyKind::Opaque) => true,
            Some(TyKind::Tuple(elems)) => elems
                .iter()
                .all(|elem| self.extension_pattern_generics_are_bound(*elem, substitutions)),
            Some(
                TyKind::Pointer { elem, .. }
                | TyKind::VolatilePointer { elem, .. }
                | TyKind::Slice { elem, .. }
                | TyKind::SlicePointee { elem },
            ) => self.extension_pattern_generics_are_bound(*elem, substitutions),
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
            })
            | Some(TyKind::Callable {
                params,
                return_type,
                ..
            })
            | Some(TyKind::CallablePointee {
                params,
                return_type,
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
                trait_const_args,
                associated_type_bindings,
                ..
            })
            | Some(TyKind::TraitObjectPointee {
                trait_args,
                trait_const_args,
                associated_type_bindings,
                ..
            }) => {
                trait_args
                    .iter()
                    .all(|arg| self.extension_pattern_generics_are_bound(*arg, substitutions))
                    && trait_const_args
                        .iter()
                        .all(|arg| self.extension_pattern_generics_are_bound(arg.ty, substitutions))
                    && associated_type_bindings.iter().all(|binding| {
                        binding.trait_args.iter().all(|arg| {
                            self.extension_pattern_generics_are_bound(*arg, substitutions)
                        }) && binding.trait_const_args.iter().all(|arg| {
                            self.extension_pattern_generics_are_bound(arg.ty, substitutions)
                        }) && self.extension_pattern_generics_are_bound(binding.ty, substitutions)
                    })
            }
            Some(TyKind::Projection {
                self_ty,
                trait_args,
                trait_const_args,
                ..
            }) => {
                self.extension_pattern_generics_are_bound(*self_ty, substitutions)
                    && trait_args
                        .iter()
                        .all(|arg| self.extension_pattern_generics_are_bound(*arg, substitutions))
                    && trait_const_args
                        .iter()
                        .all(|arg| self.extension_pattern_generics_are_bound(arg.ty, substitutions))
            }
            Some(
                TyKind::Error
                | TyKind::ConstOnly
                | TyKind::Primitive(_)
                | TyKind::BuiltinType(_)
                | TyKind::Vector { .. }
                | TyKind::ClosureState { .. },
            )
            | None => true,
        }
    }

    fn extension_pattern_contains_generic(&self, pattern: InternedTyId) -> bool {
        match self.ty_kind(pattern) {
            Some(TyKind::GenericParam(_) | TyKind::SelfParam) => true,
            Some(TyKind::Tuple(elems)) => elems
                .iter()
                .any(|elem| self.extension_pattern_contains_generic(*elem)),
            Some(
                TyKind::Pointer { elem, .. }
                | TyKind::VolatilePointer { elem, .. }
                | TyKind::Slice { elem, .. }
                | TyKind::SlicePointee { elem },
            ) => self.extension_pattern_contains_generic(*elem),
            Some(TyKind::Array { elem, .. }) => self.extension_pattern_contains_generic(*elem),
            Some(TyKind::Range { bound, .. }) => {
                bound.is_some_and(|bound| self.extension_pattern_contains_generic(bound))
            }
            Some(TyKind::FunctionPointer {
                params,
                return_type,
                ..
            })
            | Some(TyKind::Callable {
                params,
                return_type,
                ..
            })
            | Some(TyKind::CallablePointee {
                params,
                return_type,
            }) => {
                params
                    .iter()
                    .any(|param| self.extension_pattern_contains_generic(*param))
                    || self.extension_pattern_contains_generic(*return_type)
            }
            Some(TyKind::Optional { elem }) => self.extension_pattern_contains_generic(*elem),
            Some(TyKind::ErrorUnion { error, value }) => {
                self.extension_pattern_contains_generic(*error)
                    || self.extension_pattern_contains_generic(*value)
            }
            Some(TyKind::Nominal { args, .. } | TyKind::BuiltinTrait { args, .. }) => args
                .iter()
                .any(|arg| self.extension_pattern_contains_generic(*arg)),
            Some(TyKind::TraitObject {
                trait_args,
                trait_const_args,
                associated_type_bindings,
                ..
            })
            | Some(TyKind::TraitObjectPointee {
                trait_args,
                trait_const_args,
                associated_type_bindings,
                ..
            }) => {
                trait_args
                    .iter()
                    .any(|arg| self.extension_pattern_contains_generic(*arg))
                    || trait_const_args
                        .iter()
                        .any(|arg| self.extension_pattern_contains_generic(arg.ty))
                    || associated_type_bindings.iter().any(|binding| {
                        binding
                            .trait_args
                            .iter()
                            .any(|arg| self.extension_pattern_contains_generic(*arg))
                            || binding
                                .trait_const_args
                                .iter()
                                .any(|arg| self.extension_pattern_contains_generic(arg.ty))
                            || self.extension_pattern_contains_generic(binding.ty)
                    })
            }
            Some(TyKind::Projection {
                self_ty,
                trait_args,
                trait_const_args,
                ..
            }) => {
                self.extension_pattern_contains_generic(*self_ty)
                    || trait_args
                        .iter()
                        .any(|arg| self.extension_pattern_contains_generic(*arg))
                    || trait_const_args
                        .iter()
                        .any(|arg| self.extension_pattern_contains_generic(arg.ty))
            }
            Some(
                TyKind::Error
                | TyKind::ConstOnly
                | TyKind::Opaque
                | TyKind::Primitive(_)
                | TyKind::BuiltinType(_)
                | TyKind::Vector { .. }
                | TyKind::ClosureState { .. },
            )
            | None => false,
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
                Some(TyKind::Callable {
                    is_readonly: left_readonly,
                    params: left_params,
                    return_type: left_return,
                }),
                Some(TyKind::Callable {
                    is_readonly: right_readonly,
                    params: right_params,
                    return_type: right_return,
                }),
            ) => {
                left_readonly == right_readonly
                    && left_params.len() == right_params.len()
                    && left_params
                        .iter()
                        .zip(&right_params)
                        .all(|(left, right)| self.types_match(*left, *right))
                    && self.types_match(left_return, right_return)
            }
            (
                Some(TyKind::CallablePointee {
                    params: left_params,
                    return_type: left_return,
                }),
                Some(TyKind::CallablePointee {
                    params: right_params,
                    return_type: right_return,
                }),
            ) => {
                left_params.len() == right_params.len()
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
                    trait_const_args: left_const_args,
                    associated_type_bindings: left_bindings,
                }),
                Some(TyKind::TraitObject {
                    is_readonly: right_const,
                    trait_id: right_trait,
                    trait_args: right_args,
                    trait_const_args: right_const_args,
                    associated_type_bindings: right_bindings,
                }),
            ) => {
                left_const == right_const
                    && left_trait == right_trait
                    && left_args.len() == right_args.len()
                    && left_const_args == right_const_args
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
                    trait_const_args: left_const_args,
                    name: left_name,
                }),
                Some(TyKind::Projection {
                    self_ty: right_self,
                    trait_id: right_trait,
                    trait_args: right_args,
                    trait_const_args: right_const_args,
                    name: right_name,
                }),
            ) => {
                left_trait == right_trait
                    && left_name == right_name
                    && left_args.len() == right_args.len()
                    && left_const_args == right_const_args
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
            && left.trait_const_args == right.trait_const_args
            && left
                .trait_args
                .iter()
                .zip(&right.trait_args)
                .all(|(left, right)| self.types_match(*left, *right))
    }
}
