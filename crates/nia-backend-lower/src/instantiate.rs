// SPDX-License-Identifier: GPL-3.0-or-later
use std::collections::HashMap;

use crate::{ModuleLowerer, TypeInstantiationKey, TypeSubstitutionId, TypeSubstitutionKey};
use nia_backend_ir::{BackendFunction, BackendParam};
use nia_function_ir::{
    FunctionArrayElements, FunctionAsmInput, FunctionAsmOutput, FunctionBinding, FunctionBody,
    FunctionCallee, FunctionDeferBody, FunctionExpr, FunctionExprKind, FunctionFieldInit,
    FunctionForHeader, FunctionInlineAsm, FunctionLocal, FunctionOp, FunctionPlace,
    FunctionPlaceBase, FunctionPlaceElem, FunctionRange, FunctionSliceRange, FunctionTerminator,
};
use nia_ids::{BuiltinTrait, BuiltinTraitMethod, GlobalDefId, InternedTyId, ModuleId, TraitId};
use nia_trait_solve::{TraitGoal, TraitResolution, TraitSolverContext};
use nia_ty::{LayoutBuiltin, TyKind};

mod function_body_instantiation;
mod trait_resolution;

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

    fn effective_generics_for_def(&mut self, def_id: GlobalDefId) -> &[String] {
        if !self.effective_generics.contains_key(&def_id) {
            let own_generics = self
                .input
                .defs
                .defs
                .get(def_id.def_id)
                .map(|def| def.generics.as_slice())
                .unwrap_or(&[]);
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
        let mut generics = self.extension_target_generics(def_id).unwrap_or_else(|| {
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

    fn extension_target_generics(&self, def_id: GlobalDefId) -> Option<Vec<String>> {
        self.extension_targets_by_method
            .get(&def_id)
            .map(|target_ty| self.generic_params_in_ty(*target_ty))
    }

    pub(crate) fn generic_params_in_ty(&self, ty: InternedTyId) -> Vec<String> {
        let mut generics = Vec::new();
        self.collect_generic_params_in_ty(ty, &mut generics);
        generics
    }

    pub(crate) fn collect_generic_params_in_ty(
        &self,
        ty: InternedTyId,
        generics: &mut Vec<String>,
    ) {
        match self.ty_kind(ty) {
            Some(TyKind::GenericParam(name)) => {
                if !generics.contains(name) {
                    generics.push(name.clone());
                }
            }
            Some(TyKind::Pointer { elem, .. } | TyKind::Slice { elem, .. }) => {
                self.collect_generic_params_in_ty(*elem, generics);
            }
            Some(TyKind::Array { elem, .. }) => {
                self.collect_generic_params_in_ty(*elem, generics);
            }
            Some(TyKind::Range { bound, .. }) => {
                if let Some(bound) = bound {
                    self.collect_generic_params_in_ty(*bound, generics);
                }
            }
            Some(TyKind::FunctionPointer {
                params,
                return_type,
                ..
            }) => {
                for param in params {
                    self.collect_generic_params_in_ty(*param, generics);
                }
                self.collect_generic_params_in_ty(*return_type, generics);
            }
            Some(TyKind::Optional { elem }) => {
                self.collect_generic_params_in_ty(*elem, generics);
            }
            Some(TyKind::ErrorUnion { error, value }) => {
                self.collect_generic_params_in_ty(*error, generics);
                self.collect_generic_params_in_ty(*value, generics);
            }
            Some(TyKind::Nominal { args, .. }) => {
                for arg in args {
                    self.collect_generic_params_in_ty(*arg, generics);
                }
            }
            Some(TyKind::BuiltinTrait { args, .. }) => {
                for arg in args {
                    self.collect_generic_params_in_ty(*arg, generics);
                }
            }
            Some(TyKind::TraitObject {
                trait_args,
                associated_type_bindings,
                ..
            }) => {
                for arg in trait_args {
                    self.collect_generic_params_in_ty(*arg, generics);
                }
                for binding in associated_type_bindings {
                    for arg in &binding.trait_args {
                        self.collect_generic_params_in_ty(*arg, generics);
                    }
                    self.collect_generic_params_in_ty(binding.ty, generics);
                }
            }
            Some(TyKind::Projection {
                self_ty,
                trait_args,
                ..
            }) => {
                self.collect_generic_params_in_ty(*self_ty, generics);
                for arg in trait_args {
                    self.collect_generic_params_in_ty(*arg, generics);
                }
            }
            Some(TyKind::Error | TyKind::ComptimeOnly | TyKind::Primitive(_)) | None => {}
        }
    }

    pub(crate) fn effective_generic_substitutions(
        &mut self,
        def_id: GlobalDefId,
        args: &[InternedTyId],
    ) -> HashMap<String, InternedTyId> {
        let generics = self.effective_generics_for_def(def_id);
        Self::generic_substitutions(generics, args)
    }

    pub(crate) fn effective_generic_substitutions_for_instance(
        &mut self,
        def_id: GlobalDefId,
        arg_module_id: ModuleId,
        args: &[InternedTyId],
    ) -> HashMap<String, InternedTyId> {
        let args = args
            .iter()
            .map(|arg| self.import_instance_arg_type(arg_module_id, *arg))
            .collect::<Vec<_>>();
        self.effective_generic_substitutions(def_id, &args)
    }

    fn import_instance_arg_type(
        &mut self,
        arg_module_id: ModuleId,
        ty: InternedTyId,
    ) -> InternedTyId {
        if ty.interner_id == self.interner.interner_id() {
            return ty;
        }
        if let Some(interner) = self.input.program_type_interners.get(&arg_module_id)
            && ty.interner_id == interner.interner_id()
        {
            return nia_ty::import_type_into(&mut self.interner, interner, ty);
        }
        if let Some((_, interner)) = self.input.program_extensions.get(&arg_module_id)
            && ty.interner_id == interner.interner_id()
        {
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
        body
    }

    pub(crate) fn generic_params_in_extension_ty(&mut self, ty: InternedTyId) -> &[String] {
        if self.extension_ty_generics.contains_key(&ty) {
            return self
                .extension_ty_generics
                .get(&ty)
                .map(Vec::as_slice)
                .unwrap();
        }
        let mut generics = Vec::new();
        self.collect_generic_params_in_extension_ty(ty, &mut generics);
        self.extension_ty_generics.insert(ty, generics);
        self.extension_ty_generics
            .get(&ty)
            .map(Vec::as_slice)
            .unwrap()
    }

    fn collect_generic_params_in_extension_ty(&self, ty: InternedTyId, generics: &mut Vec<String>) {
        match self.extension_ty_kind(ty) {
            Some(TyKind::GenericParam(name)) => {
                if !generics.contains(name) {
                    generics.push(name.clone());
                }
            }
            Some(TyKind::Pointer { elem, .. } | TyKind::Slice { elem, .. }) => {
                self.collect_generic_params_in_extension_ty(*elem, generics);
            }
            Some(TyKind::Array { elem, .. }) => {
                self.collect_generic_params_in_extension_ty(*elem, generics);
            }
            Some(TyKind::Range { bound, .. }) => {
                if let Some(bound) = bound {
                    self.collect_generic_params_in_extension_ty(*bound, generics);
                }
            }
            Some(TyKind::FunctionPointer {
                params,
                return_type,
                ..
            }) => {
                for param in params {
                    self.collect_generic_params_in_extension_ty(*param, generics);
                }
                self.collect_generic_params_in_extension_ty(*return_type, generics);
            }
            Some(TyKind::Optional { elem }) => {
                self.collect_generic_params_in_extension_ty(*elem, generics);
            }
            Some(TyKind::ErrorUnion { error, value }) => {
                self.collect_generic_params_in_extension_ty(*error, generics);
                self.collect_generic_params_in_extension_ty(*value, generics);
            }
            Some(TyKind::Nominal { args, .. }) => {
                for arg in args {
                    self.collect_generic_params_in_extension_ty(*arg, generics);
                }
            }
            Some(TyKind::BuiltinTrait { args, .. }) => {
                for arg in args {
                    self.collect_generic_params_in_extension_ty(*arg, generics);
                }
            }
            Some(TyKind::TraitObject {
                trait_args,
                associated_type_bindings,
                ..
            }) => {
                for arg in trait_args {
                    self.collect_generic_params_in_extension_ty(*arg, generics);
                }
                for binding in associated_type_bindings {
                    for arg in &binding.trait_args {
                        self.collect_generic_params_in_extension_ty(*arg, generics);
                    }
                    self.collect_generic_params_in_extension_ty(binding.ty, generics);
                }
            }
            Some(TyKind::Projection {
                self_ty,
                trait_args,
                ..
            }) => {
                self.collect_generic_params_in_extension_ty(*self_ty, generics);
                for arg in trait_args {
                    self.collect_generic_params_in_extension_ty(*arg, generics);
                }
            }
            Some(TyKind::Error | TyKind::ComptimeOnly | TyKind::Primitive(_)) | None => {}
        }
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
        let key = TypeInstantiationKey {
            ty,
            substitutions,
            current_function: self.current_instantiated_function,
        };
        if let Some(instantiated) = self.type_instantiations.get(&key) {
            return *instantiated;
        }
        match self.interner.get(ty).cloned() {
            Some(TyKind::GenericParam(name)) => {
                let instantiated = self.type_substitution(substitutions, &name).unwrap_or(ty);
                self.cache_type_instantiation(key, instantiated)
            }
            Some(TyKind::Pointer { is_readonly, elem }) => {
                let elem = self.instantiate_ty_with_id(elem, substitutions);
                let instantiated = self.interner.intern(TyKind::Pointer { is_readonly, elem });
                self.cache_type_instantiation(key, instantiated)
            }
            Some(TyKind::Slice { is_readonly, elem }) => {
                let elem = self.instantiate_ty_with_id(elem, substitutions);
                let instantiated = self.interner.intern(TyKind::Slice { is_readonly, elem });
                self.cache_type_instantiation(key, instantiated)
            }
            Some(TyKind::Array { len, elem }) => {
                let elem = self.instantiate_ty_with_id(elem, substitutions);
                let instantiated = self.interner.intern(TyKind::Array { len, elem });
                self.cache_type_instantiation(key, instantiated)
            }
            Some(TyKind::Range { kind, bound }) => {
                let bound = bound.map(|bound| self.instantiate_ty_with_id(bound, substitutions));
                let instantiated = self.interner.intern(TyKind::Range { kind, bound });
                self.cache_type_instantiation(key, instantiated)
            }
            Some(TyKind::FunctionPointer {
                params,
                return_type,
                is_variadic,
            }) => {
                let params = params
                    .iter()
                    .copied()
                    .map(|param| self.instantiate_ty_with_id(param, substitutions))
                    .collect();
                let return_type = self.instantiate_ty_with_id(return_type, substitutions);
                let instantiated = self.interner.intern(TyKind::FunctionPointer {
                    params,
                    return_type,
                    is_variadic,
                });
                self.cache_type_instantiation(key, instantiated)
            }
            Some(TyKind::Optional { elem }) => {
                let elem = self.instantiate_ty_with_id(elem, substitutions);
                let instantiated = self.interner.intern(TyKind::Optional { elem });
                self.cache_type_instantiation(key, instantiated)
            }
            Some(TyKind::ErrorUnion { error, value }) => {
                let error = self.instantiate_ty_with_id(error, substitutions);
                let value = self.instantiate_ty_with_id(value, substitutions);
                let instantiated = self.interner.intern(TyKind::ErrorUnion { error, value });
                self.cache_type_instantiation(key, instantiated)
            }
            Some(TyKind::Nominal { def_id, args }) => {
                let args = args
                    .iter()
                    .copied()
                    .map(|arg| self.instantiate_ty_with_id(arg, substitutions))
                    .collect::<Vec<_>>();
                let instantiated = self.interner.intern(TyKind::Nominal { def_id, args });
                self.cache_type_instantiation(key, instantiated)
            }
            Some(TyKind::BuiltinTrait { trait_id, args }) => {
                let args = args
                    .iter()
                    .copied()
                    .map(|arg| self.instantiate_ty_with_id(arg, substitutions))
                    .collect::<Vec<_>>();
                let instantiated = self
                    .interner
                    .intern(TyKind::BuiltinTrait { trait_id, args });
                self.cache_type_instantiation(key, instantiated)
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
                    .map(|arg| self.instantiate_ty_with_id(arg, substitutions))
                    .collect::<Vec<_>>();
                let associated_type_bindings = associated_type_bindings
                    .iter()
                    .map(|binding| nia_ty::AssociatedTypeBindingTy {
                        trait_id: binding.trait_id,
                        trait_args: binding
                            .trait_args
                            .iter()
                            .copied()
                            .map(|arg| self.instantiate_ty_with_id(arg, substitutions))
                            .collect(),
                        name: binding.name.clone(),
                        ty: self.instantiate_ty_with_id(binding.ty, substitutions),
                    })
                    .collect();
                let instantiated = self.interner.intern(TyKind::TraitObject {
                    is_readonly,
                    trait_id,
                    trait_args,
                    associated_type_bindings,
                });
                self.cache_type_instantiation(key, instantiated)
            }
            Some(TyKind::Projection {
                self_ty,
                trait_id,
                trait_args,
                name,
            }) => {
                let self_ty = self.instantiate_ty_with_id(self_ty, substitutions);
                let trait_args = trait_args
                    .iter()
                    .copied()
                    .map(|arg| self.instantiate_ty_with_id(arg, substitutions))
                    .collect::<Vec<_>>();
                let resolved = self
                    .resolve_associated_type_projection_from_current_impl(
                        self_ty,
                        trait_id,
                        &trait_args,
                        &name,
                        substitutions,
                    )
                    .or_else(|| {
                        self.resolve_associated_type_projection(
                            self_ty,
                            trait_id,
                            &trait_args,
                            &name,
                        )
                    })
                    .map(|resolved| self.instantiate_ty_with_id(resolved, substitutions));
                let instantiated = resolved.unwrap_or_else(|| {
                    self.interner.intern(TyKind::Projection {
                        self_ty,
                        trait_id,
                        trait_args,
                        name,
                    })
                });
                self.cache_type_instantiation(key, instantiated)
            }
            Some(TyKind::Error | TyKind::ComptimeOnly) | Some(TyKind::Primitive(_)) | None => {
                self.cache_type_instantiation(key, ty)
            }
        }
    }

    fn intern_type_substitutions(
        &mut self,
        substitutions: &HashMap<String, InternedTyId>,
    ) -> TypeSubstitutionId {
        let mut substitutions = substitutions
            .iter()
            .map(|(name, ty)| (name.clone(), *ty))
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

    fn resolve_associated_type_projection(
        &mut self,
        self_ty: InternedTyId,
        trait_id: nia_ty::TraitId,
        trait_args: &[InternedTyId],
        name: &str,
    ) -> Option<InternedTyId> {
        let context = TraitSolverContext {
            normalization: self.input.type_normalization,
            trait_impls: self.input.trait_impls,
            layouts: Some(self.input.layouts),
            local_module_id: self.input.module_id,
            local_enums: &self.input.signatures.enums,
            program_enums: Some(self.input.program_enums),
        };
        let mut solver = context.solver(&mut self.interner, &[]);
        solver.resolve_associated_type(self_ty, trait_id, trait_args, name)
    }

    fn resolve_associated_type_projection_from_current_impl(
        &mut self,
        self_ty: InternedTyId,
        trait_id: nia_ty::TraitId,
        trait_args: &[InternedTyId],
        name: &str,
        substitutions: TypeSubstitutionId,
    ) -> Option<InternedTyId> {
        let current = self.current_instantiated_function?;
        if current.module_id != self.input.module_id {
            return None;
        }
        let def = self.input.defs.defs.get(current.def_id)?;
        if def.kind != nia_defs::DefKind::Method {
            return None;
        }
        let target_ty = self.extension_targets_by_method.get(&current).copied()?;
        let target_ty = self.instantiate_ty_with_id(target_ty, substitutions);
        if !self.types_match(target_ty, self_ty) {
            return None;
        }

        let mut matched = None;
        for impl_signature in self.input.trait_impls {
            if impl_signature.trait_id != trait_id
                || impl_signature.trait_args.len() != trait_args.len()
            {
                continue;
            }

            let impl_target = nia_ty::import_type_into(
                &mut self.interner,
                &impl_signature.interner,
                impl_signature.target_ty,
            );
            let impl_target = self.instantiate_ty_with_id(impl_target, substitutions);
            if !self.types_match(impl_target, self_ty) {
                continue;
            }

            let mut trait_args_match = true;
            for (actual, required) in impl_signature
                .trait_args
                .iter()
                .copied()
                .zip(trait_args.iter().copied())
            {
                let actual =
                    nia_ty::import_type_into(&mut self.interner, &impl_signature.interner, actual);
                let actual = self.instantiate_ty_with_id(actual, substitutions);
                if !self.types_match(actual, required) {
                    trait_args_match = false;
                    break;
                }
            }
            if !trait_args_match {
                continue;
            }

            if let Some(associated_type) = impl_signature
                .associated_types
                .iter()
                .find(|associated_type| associated_type.name == name)
            {
                matched = Some((impl_signature.interner.clone(), associated_type.ty));
                break;
            }
        }

        let (source_interner, ty) = matched?;
        let ty = nia_ty::import_type_into(&mut self.interner, &source_interner, ty);
        Some(self.instantiate_ty_with_id(ty, substitutions))
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
        &self,
        pattern: InternedTyId,
        actual: InternedTyId,
        substitutions: &mut HashMap<String, InternedTyId>,
    ) -> bool {
        match self.extension_ty_kind(pattern) {
            Some(TyKind::GenericParam(name)) => {
                if let Some(existing) = substitutions.get(name).copied() {
                    self.types_match(existing, actual)
                } else {
                    substitutions.insert(name.clone(), actual);
                    true
                }
            }
            Some(TyKind::Pointer {
                is_readonly: pattern_const,
                elem: pattern_elem,
            }) => matches!(
                self.ty_kind(actual),
                Some(TyKind::Pointer { is_readonly, elem })
                    if is_readonly == pattern_const
                        && self.match_extension_type_pattern(*pattern_elem, *elem, substitutions)
            ),
            Some(TyKind::Slice {
                is_readonly: pattern_const,
                elem: pattern_elem,
            }) => matches!(
                self.ty_kind(actual),
                Some(TyKind::Slice { is_readonly, elem })
                    if is_readonly == pattern_const
                        && self.match_extension_type_pattern(*pattern_elem, *elem, substitutions)
            ),
            Some(TyKind::Array {
                len: pattern_len,
                elem: pattern_elem,
            }) => match self.ty_kind(actual) {
                Some(TyKind::Array { len, elem }) if pattern_len == len => {
                    self.match_extension_type_pattern(*pattern_elem, *elem, substitutions)
                }
                _ => false,
            },
            Some(TyKind::Range {
                kind: pattern_kind,
                bound: pattern_bound,
            }) => match self.ty_kind(actual) {
                Some(TyKind::Range { kind, bound }) if pattern_kind == kind => {
                    match (pattern_bound, bound) {
                        (Some(pattern_bound), Some(bound)) => {
                            self.match_extension_type_pattern(*pattern_bound, *bound, substitutions)
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
            }) => match self.ty_kind(actual) {
                Some(TyKind::FunctionPointer {
                    params,
                    return_type,
                    is_variadic,
                }) if pattern_variadic == is_variadic && pattern_params.len() == params.len() => {
                    pattern_params.iter().zip(params).all(|(pattern, actual)| {
                        self.match_extension_type_pattern(*pattern, *actual, substitutions)
                    }) && self.match_extension_type_pattern(
                        *pattern_return,
                        *return_type,
                        substitutions,
                    )
                }
                _ => false,
            },
            Some(TyKind::Optional { elem: pattern_elem }) => match self.ty_kind(actual) {
                Some(TyKind::Optional { elem }) => {
                    self.match_extension_type_pattern(*pattern_elem, *elem, substitutions)
                }
                _ => false,
            },
            Some(TyKind::ErrorUnion {
                error: pattern_error,
                value: pattern_value,
            }) => match self.ty_kind(actual) {
                Some(TyKind::ErrorUnion { error, value }) => {
                    self.match_extension_type_pattern(*pattern_error, *error, substitutions)
                        && self.match_extension_type_pattern(*pattern_value, *value, substitutions)
                }
                _ => false,
            },
            Some(TyKind::Nominal {
                def_id: pattern_def,
                args: pattern_args,
            }) => match self.ty_kind(actual) {
                Some(TyKind::Nominal { def_id, args })
                    if pattern_def == def_id && pattern_args.len() == args.len() =>
                {
                    pattern_args.iter().zip(args).all(|(pattern, actual)| {
                        self.match_extension_type_pattern(*pattern, *actual, substitutions)
                    })
                }
                _ => false,
            },
            Some(TyKind::BuiltinTrait {
                trait_id: pattern_trait,
                args: pattern_args,
            }) => match self.ty_kind(actual) {
                Some(TyKind::BuiltinTrait { trait_id, args })
                    if pattern_trait == trait_id && pattern_args.len() == args.len() =>
                {
                    pattern_args.iter().zip(args).all(|(pattern, actual)| {
                        self.match_extension_type_pattern(*pattern, *actual, substitutions)
                    })
                }
                _ => false,
            },
            Some(TyKind::TraitObject {
                is_readonly: pattern_const,
                trait_id: pattern_trait,
                trait_args: pattern_args,
                associated_type_bindings: pattern_bindings,
            }) => match self.ty_kind(actual) {
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
                            self.match_extension_type_pattern(*pattern, *actual, substitutions)
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
            }) => match self.ty_kind(actual) {
                Some(TyKind::Projection {
                    self_ty,
                    trait_id,
                    trait_args,
                    name,
                }) if pattern_trait == trait_id
                    && pattern_name == name
                    && pattern_args.len() == trait_args.len() =>
                {
                    self.match_extension_type_pattern(*pattern_self, *self_ty, substitutions)
                        && pattern_args
                            .iter()
                            .zip(trait_args)
                            .all(|(pattern, actual)| {
                                self.match_extension_type_pattern(*pattern, *actual, substitutions)
                            })
                }
                _ => false,
            },
            Some(TyKind::Primitive(_)) | Some(TyKind::ComptimeOnly | TyKind::Error) | None => {
                self.types_match(pattern, actual)
            }
        }
    }

    pub(crate) fn types_match(&self, left: InternedTyId, right: InternedTyId) -> bool {
        if left == right {
            return true;
        }
        match (self.ty_kind(left), self.ty_kind(right)) {
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
            ) => left_const == right_const && self.types_match(*left_elem, *right_elem),
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
                        .zip(right_args)
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
                        .zip(right_args)
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
                        .zip(right_args)
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
                            self.types_match(*left_bound, *right_bound)
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
                    && self.types_match(*left_self, *right_self)
                    && left_args
                        .iter()
                        .zip(right_args)
                        .all(|(left, right)| self.types_match(*left, *right))
            }
            _ => false,
        }
    }

    fn associated_type_binding_keys_match(
        &self,
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
