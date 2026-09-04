// SPDX-License-Identifier: GPL-3.0-or-later
use std::collections::{HashMap, HashSet, VecDeque};

use crate::{
    BackendItemDiscovery, FunctionInstanceMaterialization, ModuleLowerer,
    backend_function_instance_key,
};
use nia_backend_ir::{
    BackendClosureEntry, BackendClosureEntryOwner, BackendFunction, BackendFunctionInstance,
    BackendParam,
};
use nia_function_ir::{
    FunctionBody, FunctionInstanceKey, FunctionInstanceRef, FunctionLocal, FunctionLocalKind,
    GlobalInstanceRef,
};
use nia_ids::{GlobalDefId, InternedTyId, ModuleId};
use nia_symbol::{SymbolId, SymbolMap};
use nia_ty::{ArrayLenTy, ConstGenericArg, TyKind};

struct PlannedFunctionInstance {
    def_id: GlobalDefId,
    arg_module_id: ModuleId,
    self_arg: Option<InternedTyId>,
    args: Vec<InternedTyId>,
    const_args: Vec<ConstGenericArg>,
    symbol: String,
}

const MAX_BACKEND_FUNCTION_INSTANCES: usize = 4096;
const MAX_BACKEND_INSTANCE_TYPE_DEPTH: usize = 256;

impl<'a> ModuleLowerer<'a> {
    pub(crate) fn initial_planned_function_instance_refs(&mut self) -> Vec<FunctionInstanceRef> {
        self.input
            .function_instance_plan
            .iter()
            .map(|instance| {
                let args = self.import_monomorphized_instance_args(&instance.args);
                FunctionInstanceRef {
                    def_id: instance.def_id,
                    arg_module_id: instance.arg_module_id,
                    self_arg: instance.self_arg,
                    args,
                    const_args: instance.const_args.clone(),
                    span: instance.span,
                }
            })
            .collect()
    }

    pub(crate) fn lower_additional_function_instances(
        &mut self,
        refs: Vec<FunctionInstanceRef>,
        functions: &[BackendFunction],
        existing: &[BackendFunctionInstance],
    ) -> FunctionInstanceMaterialization {
        self.lower_function_instances_from_refs(functions, refs, existing)
    }

    fn lower_function_instances_from_refs(
        &mut self,
        functions: &[BackendFunction],
        initial_instances: Vec<FunctionInstanceRef>,
        existing: &[BackendFunctionInstance],
    ) -> FunctionInstanceMaterialization {
        let mut instances = Vec::new();
        let mut closure_entries = Vec::new();
        let mut materialized_discovery = BackendItemDiscovery::default();
        let mut seen = HashSet::<FunctionInstanceKey>::new();
        let mut queued = HashSet::<FunctionInstanceKey>::new();
        let mut functions_by_def = functions
            .iter()
            .map(|function| (function.def_id, function.clone()))
            .collect::<HashMap<_, _>>();
        let mut pending = VecDeque::new();
        for instance in existing {
            let self_arg = self.canonicalize_instance_self_arg(instance.self_arg);
            let args = self.canonicalize_instance_args(&instance.args);
            seen.insert(FunctionInstanceKey {
                def_id: instance.def_id,
                arg_module_id: instance.arg_module_id,
                self_arg,
                args,
                const_args: instance.const_args.clone(),
            });
        }
        for instance in initial_instances {
            enqueue_function_instance_ref(&mut pending, &mut queued, instance);
        }

        while let Some(instance) = pending.pop_front() {
            if instance.def_id.module_id != self.input.module_id {
                self.foreign_function_instance_refs.push(instance);
                continue;
            }
            let self_arg = self.canonicalize_instance_ref_self_arg(&instance);
            let args = self.canonicalize_instance_ref_args(&instance);
            let const_args = instance.const_args.clone();
            let key = FunctionInstanceKey {
                def_id: instance.def_id,
                arg_module_id: instance.arg_module_id,
                self_arg,
                args: args.clone(),
                const_args: const_args.clone(),
            };
            if seen.contains(&key) {
                continue;
            }
            if args
                .iter()
                .chain(const_args.iter().map(|arg| &arg.ty))
                .any(|arg| {
                    self.cached_ty_contains_generic_param(*arg)
                        || self.cached_ty_contains_unresolved_projection(*arg)
                        || self.cached_ty_contains_error(*arg)
                })
            {
                continue;
            }
            if args
                .iter()
                .chain(const_args.iter().map(|arg| &arg.ty))
                .any(|arg| {
                    self.ty_exceeds_backend_instance_depth(*arg, MAX_BACKEND_INSTANCE_TYPE_DEPTH)
                })
            {
                self.report_backend_instance_type_depth_limit(instance.span, instance.def_id);
                continue;
            }
            // Materialization runs repeatedly as functions, globals, and
            // vtables discover each other. The limit is module-wide, not a
            // fresh allowance for each fixed-point iteration.
            let known_instances =
                known_backend_function_instance_count(existing.len(), instances.len());
            if known_instances >= MAX_BACKEND_FUNCTION_INSTANCES {
                self.report_backend_instance_limit(
                    instance.span,
                    instance.def_id,
                    &args,
                    known_instances,
                );
                continue;
            }
            let Some(name) = self.def_symbol_name(instance.def_id) else {
                continue;
            };
            let symbol = self.mangle_contextual_instance_symbol(
                instance.def_id,
                name,
                instance.arg_module_id,
                self_arg,
                &args,
                &const_args,
            );
            let Some(instance_index) = self.lower_planned_function_instance(
                &mut functions_by_def,
                &mut seen,
                &mut instances,
                &mut closure_entries,
                PlannedFunctionInstance {
                    def_id: instance.def_id,
                    arg_module_id: instance.arg_module_id,
                    self_arg,
                    args,
                    const_args,
                    symbol,
                },
            ) else {
                continue;
            };
            let instance = &instances[instance_index];
            let body = &instance.function_body;
            let mut discovery = self.discover_backend_items_from_optional_body(body);
            let owner = backend_function_instance_key(instance);
            for entry in closure_entries.iter().rev().take_while(|entry| {
                entry.key.owner == BackendClosureEntryOwner::FunctionInstance(owner.clone())
            }) {
                discovery.extend(self.discover_backend_items_from_body(&entry.function_body));
            }
            for discovered in &discovery.refs.function_instances {
                let discovered_args = self.canonicalize_instance_args(&discovered.args);
                let discovered_self_arg = self.canonicalize_instance_ref_self_arg(discovered);
                let discovered_const_args = discovered.const_args.clone();
                if !seen.contains(&FunctionInstanceKey {
                    def_id: discovered.def_id,
                    arg_module_id: discovered.arg_module_id,
                    self_arg: discovered_self_arg,
                    args: discovered_args.clone(),
                    const_args: discovered_const_args.clone(),
                }) {
                    enqueue_function_instance_ref(
                        &mut pending,
                        &mut queued,
                        FunctionInstanceRef {
                            def_id: discovered.def_id,
                            arg_module_id: discovered.arg_module_id,
                            self_arg: discovered_self_arg,
                            args: discovered_args,
                            const_args: discovered_const_args,
                            span: discovered.span,
                        },
                    );
                }
            }
            materialized_discovery.extend(discovery);
        }
        FunctionInstanceMaterialization {
            instances,
            closure_entries,
            discovery: materialized_discovery,
        }
    }

    fn report_backend_instance_limit(
        &mut self,
        span: nia_span::Span,
        def_id: GlobalDefId,
        args: &[InternedTyId],
        known_instances: usize,
    ) {
        let name = self
            .function_instance_name(&mut HashMap::new(), def_id)
            .map(|name| self.symbol_name(name))
            .unwrap_or_else(|| format!("def{}", def_id.def_id.0));
        let type_args = args
            .iter()
            .map(|arg| self.instance_arg_debug_name(*arg))
            .collect::<Vec<_>>()
            .join(", ");
        self.diagnostics.push(
            nia_diagnostic::Diagnostic::user_error(nia_diagnostic::codes::LLVM_CODEGEN,
                "generic instantiation did not converge before the backend instance limit",
            )
            .primary(
                span,
                format!(
                    "lowering `{name}[{type_args}]` would exceed the backend function instance limit"
                ),
            )
            .note(
                "backend lowering may discover additional concrete instances after trait and method calls are resolved",
            )
            .note(
                "recursive calls to an already-seen concrete generic instance are allowed and reuse the existing lowered function",
            )
            .help("make the generic recursion produce a finite set of concrete function instances")
            .debug("limit", MAX_BACKEND_FUNCTION_INSTANCES)
            .debug("known_instances", known_instances)
            .debug("def_id", def_id)
            .finish(),
        );
    }

    fn report_backend_instance_type_depth_limit(
        &mut self,
        span: nia_span::Span,
        def_id: GlobalDefId,
    ) {
        let name = self
            .function_instance_name(&mut HashMap::new(), def_id)
            .map(|name| self.symbol_name(name))
            .unwrap_or_else(|| format!("def{}", def_id.def_id.0));
        self.diagnostics.push(
            nia_diagnostic::Diagnostic::user_error(nia_diagnostic::codes::LLVM_CODEGEN,
                "generic instantiation did not converge before the backend type depth limit",
            )
            .primary(
                span,
                format!("lowering `{name}` would exceed the backend instance type depth limit"),
            )
            .note(
                "backend lowering may discover additional concrete instances after trait and method calls are resolved",
            )
            .note(
                "recursive calls to an already-seen concrete generic instance are allowed and reuse the existing lowered function",
            )
            .help("make the generic recursion produce a finite set of concrete function instances")
            .debug("type_depth_limit", MAX_BACKEND_INSTANCE_TYPE_DEPTH)
            .debug("def_id", def_id)
            .finish(),
        );
    }

    fn instance_arg_debug_name(&self, ty: InternedTyId) -> String {
        self.ty_kind(ty)
            .map(|kind| format!("{kind:?}"))
            .unwrap_or_else(|| format!("{ty:?}"))
    }

    fn ty_exceeds_backend_instance_depth(&self, ty: InternedTyId, remaining: usize) -> bool {
        if remaining == 0 {
            return true;
        }
        let Some(kind) = self.ty_kind(ty) else {
            return false;
        };
        let next = remaining - 1;
        match kind {
            TyKind::Tuple(elems) => elems
                .iter()
                .any(|elem| self.ty_exceeds_backend_instance_depth(*elem, next)),
            TyKind::Pointer { elem, .. }
            | TyKind::VolatilePointer { elem, .. }
            | TyKind::Slice { elem, .. }
            | TyKind::SlicePointee { elem } => self.ty_exceeds_backend_instance_depth(*elem, next),
            TyKind::Array { len, elem } => {
                self.ty_exceeds_backend_instance_depth(*elem, next)
                    || matches!(len, ArrayLenTy::Builtin { ty, .. }
                        if self.ty_exceeds_backend_instance_depth(*ty, next))
            }
            TyKind::Range { bound, .. } => {
                bound.is_some_and(|bound| self.ty_exceeds_backend_instance_depth(bound, next))
            }
            TyKind::FunctionPointer {
                params,
                return_type,
                ..
            }
            | TyKind::Callable {
                params,
                return_type,
                ..
            }
            | TyKind::CallablePointee {
                params,
                return_type,
            } => {
                params
                    .iter()
                    .any(|param| self.ty_exceeds_backend_instance_depth(*param, next))
                    || self.ty_exceeds_backend_instance_depth(*return_type, next)
            }
            TyKind::Optional { elem } => self.ty_exceeds_backend_instance_depth(*elem, next),
            TyKind::ErrorUnion { error, value } => {
                self.ty_exceeds_backend_instance_depth(*error, next)
                    || self.ty_exceeds_backend_instance_depth(*value, next)
            }
            TyKind::Nominal {
                args, const_args, ..
            } => {
                args.iter()
                    .any(|arg| self.ty_exceeds_backend_instance_depth(*arg, next))
                    || const_args
                        .iter()
                        .any(|arg| self.ty_exceeds_backend_instance_depth(arg.ty, next))
            }
            TyKind::BuiltinTrait { args, .. } => args
                .iter()
                .any(|arg| self.ty_exceeds_backend_instance_depth(*arg, next)),
            TyKind::TraitObject {
                trait_args,
                associated_type_bindings,
                trait_const_args,
                ..
            }
            | TyKind::TraitObjectPointee {
                trait_args,
                associated_type_bindings,
                trait_const_args,
                ..
            } => {
                trait_args
                    .iter()
                    .any(|arg| self.ty_exceeds_backend_instance_depth(*arg, next))
                    || trait_const_args
                        .iter()
                        .any(|arg| self.ty_exceeds_backend_instance_depth(arg.ty, next))
                    || associated_type_bindings.iter().any(|binding| {
                        binding
                            .trait_args
                            .iter()
                            .any(|arg| self.ty_exceeds_backend_instance_depth(*arg, next))
                            || binding
                                .trait_const_args
                                .iter()
                                .any(|arg| self.ty_exceeds_backend_instance_depth(arg.ty, next))
                            || self.ty_exceeds_backend_instance_depth(binding.ty, next)
                    })
            }
            TyKind::Projection {
                self_ty,
                trait_args,
                trait_const_args,
                ..
            } => {
                self.ty_exceeds_backend_instance_depth(*self_ty, next)
                    || trait_args
                        .iter()
                        .any(|arg| self.ty_exceeds_backend_instance_depth(*arg, next))
                    || trait_const_args
                        .iter()
                        .any(|arg| self.ty_exceeds_backend_instance_depth(arg.ty, next))
            }
            TyKind::GenericParam(_)
            | TyKind::SelfParam
            | TyKind::Opaque
            | TyKind::Primitive(_)
            | TyKind::BuiltinType(_)
            | TyKind::Vector { .. }
            | TyKind::ConstOnly
            | TyKind::Error
            | TyKind::ClosureState { .. } => false,
        }
    }

    fn function_instance_name(
        &mut self,
        functions_by_def: &mut HashMap<GlobalDefId, BackendFunction>,
        def_id: GlobalDefId,
    ) -> Option<SymbolId> {
        if !functions_by_def.contains_key(&def_id)
            && let Some(function) = self.backend_function_template_for_program_def(def_id)
        {
            functions_by_def.insert(def_id, function);
        }
        functions_by_def.get(&def_id).map(|function| function.name)
    }

    fn lower_planned_function_instance(
        &mut self,
        functions_by_def: &mut HashMap<GlobalDefId, BackendFunction>,
        seen: &mut HashSet<FunctionInstanceKey>,
        instances: &mut Vec<BackendFunctionInstance>,
        closure_entries: &mut Vec<BackendClosureEntry>,
        plan: PlannedFunctionInstance,
    ) -> Option<usize> {
        let PlannedFunctionInstance {
            def_id,
            arg_module_id,
            self_arg,
            args,
            const_args,
            symbol,
        } = plan;
        if !seen.insert(FunctionInstanceKey {
            def_id,
            arg_module_id,
            self_arg,
            args: args.clone(),
            const_args: const_args.clone(),
        }) {
            return None;
        }
        if !functions_by_def.contains_key(&def_id)
            && let Some(function) = self.backend_function_template_for_program_def(def_id)
        {
            functions_by_def.insert(def_id, function);
        }
        let mut base = functions_by_def.get(&def_id).cloned()?;
        let imported_args = args
            .iter()
            .map(|arg| self.normalize_instance_arg_type(*arg))
            .collect::<Vec<_>>();
        let (substitutions, const_substitutions) =
            self.generic_substitutions_and_consts_for_def(def_id, &imported_args, &const_args);
        let substitution_id = self.intern_type_and_const_substitutions_with_self(
            self_arg,
            &substitutions,
            &const_substitutions,
        );
        let function_body = base.function_body.take().map(|body| {
            self.instantiate_function_body(crate::instantiate::FunctionBodyInstantiation {
                function: def_id,
                module_id: arg_module_id,
                is_instance: true,
                type_arg_count: args.len(),
                body,
                self_arg,
                substitutions: &substitutions,
                const_substitutions: &const_substitutions,
            })
        });
        let has_body = function_body.is_some();
        let owner_key = FunctionInstanceKey {
            def_id,
            arg_module_id,
            self_arg,
            args: args.clone(),
            const_args: const_args.clone(),
        };
        let source_closure_entries = self.input.program.closure_entries(def_id).to_vec();
        for entry in source_closure_entries {
            let body =
                self.instantiate_function_body(crate::instantiate::FunctionBodyInstantiation {
                    function: def_id,
                    module_id: arg_module_id,
                    is_instance: true,
                    type_arg_count: args.len(),
                    body: entry.body.clone(),
                    self_arg,
                    substitutions: &substitutions,
                    const_substitutions: &const_substitutions,
                });
            let state_type = self.instantiate_ty_with_id(entry.state_ty, substitution_id);
            let return_type = self.instantiate_ty_with_id(entry.return_type, substitution_id);
            if let Some(lowered_entry) = self.materialize_closure_entry(
                &entry,
                BackendClosureEntryOwner::FunctionInstance(owner_key.clone()),
                &symbol,
                state_type,
                return_type,
                body,
            ) {
                closure_entries.push(lowered_entry);
            }
        }
        instances.push(BackendFunctionInstance {
            def_id,
            name: base.name,
            arg_module_id,
            self_arg,
            args,
            const_args,
            symbol,
            params: self.instantiate_params_with_id(&base, substitution_id),
            return_type: self.instantiate_ty_with_id(base.return_type, substitution_id),
            is_extern: base.is_extern,
            is_variadic: base.is_variadic,
            attributes: base.attributes.clone(),
            local_names: function_body
                .as_ref()
                .map(|body| self.function_local_names(body))
                .unwrap_or_default(),
            function_body,
            span: base.span,
        });
        let instance_index = instances.len() - 1;
        has_body.then_some(instance_index)
    }

    pub(crate) fn backend_function_template_for_program_def(
        &mut self,
        def_id: GlobalDefId,
    ) -> Option<BackendFunction> {
        self.backend_function_template_for_program_def_with_body(def_id, true)
    }

    fn backend_function_template_for_program_def_with_body(
        &mut self,
        def_id: GlobalDefId,
        include_body: bool,
    ) -> Option<BackendFunction> {
        let signature = self.input.program.functions().get(&def_id)?;
        if include_body && !signature.signature.is_extern && !signature.signature.has_body {
            return None;
        }
        if include_body
            && !signature.signature.is_extern
            && self.input.program.function_body(def_id).is_none()
        {
            return None;
        }
        let own_generics = &signature.signature.generics;
        let effective_generics = self.effective_generics(def_id, own_generics).to_vec();
        let effective_params = self
            .effective_generic_params_for_def(def_id)
            .unwrap_or_else(|| {
                effective_generics
                    .iter()
                    .map(|generic| (*generic, false))
                    .collect()
            });
        let identity_substitutions = effective_params
            .iter()
            .filter(|(_, is_const)| !is_const)
            .map(|(generic, _)| {
                (
                    *generic,
                    self.type_context
                        .append
                        .intern(TyKind::GenericParam(*generic)),
                )
            })
            .collect::<SymbolMap<_>>();
        let raw_function_body = include_body
            .then(|| self.input.program.function_body(def_id))
            .flatten();
        let param_locals = raw_function_body
            .as_ref()
            .map(|body| self.template_param_locals(def_id, &signature.signature.params, body))
            .unwrap_or_default();
        let function_body = raw_function_body.map(|body| {
            self.instantiate_function_body(crate::instantiate::FunctionBodyInstantiation {
                function: def_id,
                module_id: self.input.module_id,
                is_instance: true,
                type_arg_count: 0,
                body: body.clone(),
                self_arg: None,
                substitutions: &identity_substitutions,
                const_substitutions: &SymbolMap::default(),
            })
        });
        Some(BackendFunction {
            def_id,
            name: signature.name,
            link_name: signature
                .signature
                .is_extern
                .then(|| self.symbol_name(signature.name)),
            generics: effective_generics,
            params: signature
                .signature
                .params
                .iter()
                .enumerate()
                .map(|(index, param)| {
                    let signature_ty = self.normalized_type_from_module(def_id.module_id, param.ty);
                    let param_local = param_locals.get(index).copied();
                    let local_ty = if param.receiver.is_some() {
                        param_local
                            .map(|(_, ty)| self.normalized_type_from_module(def_id.module_id, ty))
                            .unwrap_or(signature_ty)
                    } else {
                        signature_ty
                    };
                    let passing_ty = param
                        .receiver
                        .map(|receiver| self.receiver_passing_ty(receiver, local_ty))
                        .unwrap_or(local_ty);
                    BackendParam {
                        local_id: param_local.map(|(local_id, _)| local_id),
                        name: param.name,
                        receiver: param.receiver,
                        passing_ty,
                        local_ty,
                        span: param.span,
                    }
                })
                .collect(),
            return_type: self
                .normalized_type_from_module(def_id.module_id, signature.signature.return_type),
            is_extern: signature.signature.is_extern,
            is_variadic: signature.signature.is_variadic,
            attributes: self.backend_function_attributes(def_id, &signature.signature.attributes),
            local_names: function_body
                .as_ref()
                .map(|body| self.function_local_names(body))
                .unwrap_or_default(),
            function_body,
            span: signature.signature.span,
        })
    }

    fn template_param_locals(
        &mut self,
        def_id: GlobalDefId,
        params: &[nia_item_signatures::ParamSignature],
        body: &FunctionBody,
    ) -> Vec<(nia_ids::LocalId, InternedTyId)> {
        let locals = body
            .locals
            .iter()
            .filter(|local| local.kind == FunctionLocalKind::Param)
            .collect::<Vec<_>>();
        if locals.len() != params.len() {
            self.report_backend_template_param_local_mismatch(
                def_id,
                body.span,
                params.len(),
                locals.len(),
            );
            return Vec::new();
        }
        for (index, (param, local)) in params.iter().zip(locals.iter()).enumerate() {
            if let Some(name) = &param.name
                && local.name.symbol() != Some(*name)
            {
                self.report_backend_template_param_name_mismatch(def_id, index, *name, local);
            }
        }
        locals
            .into_iter()
            .map(|local| (local.id, local.ty))
            .collect()
    }

    fn report_backend_template_param_local_mismatch(
        &mut self,
        def_id: GlobalDefId,
        span: nia_span::Span,
        signature_params: usize,
        body_param_locals: usize,
    ) {
        self.diagnostics.push(
            nia_diagnostic::Diagnostic::internal_error(
                nia_diagnostic::codes::INVALID_BACKEND_IR,
                "backend function template parameter locals do not match its signature",
            )
            .primary(
                span,
                "backend function template parameter locals do not match its signature",
            )
            .debug("def_id", def_id)
            .debug("signature_params", signature_params)
            .debug("body_param_locals", body_param_locals)
            .finish(),
        );
    }

    fn report_backend_template_param_name_mismatch(
        &mut self,
        def_id: GlobalDefId,
        index: usize,
        signature_name: nia_symbol::SymbolId,
        local: &FunctionLocal,
    ) {
        let local_name = self.local_name(local.name);
        self.diagnostics.push(
            nia_diagnostic::Diagnostic::internal_error(
                nia_diagnostic::codes::INVALID_BACKEND_IR,
                "backend function template parameter local order does not match its signature",
            )
            .primary(
                local.span,
                "backend function template parameter local order does not match its signature",
            )
            .debug("def_id", def_id)
            .debug("param_index", index)
            .debug("signature_name", signature_name)
            .debug("local_name", local_name.as_str())
            .finish(),
        );
    }

    pub(crate) fn canonicalize_instance_args(
        &mut self,
        args: &[InternedTyId],
    ) -> Vec<InternedTyId> {
        args.iter()
            .copied()
            .map(|arg| self.canonicalize_instance_arg(arg))
            .collect()
    }

    pub(crate) fn canonicalize_instance_arg(&mut self, arg: InternedTyId) -> InternedTyId {
        self.instantiate_ty(arg, &SymbolMap::default())
    }

    pub(crate) fn canonicalize_instance_ref_args(
        &mut self,
        instance: &FunctionInstanceRef,
    ) -> Vec<InternedTyId> {
        self.canonicalize_instance_args(&instance.args)
    }

    pub(crate) fn canonicalize_instance_self_arg(
        &mut self,
        self_arg: Option<InternedTyId>,
    ) -> Option<InternedTyId> {
        self_arg.map(|self_arg| self.canonicalize_instance_arg(self_arg))
    }

    pub(crate) fn canonicalize_instance_ref_self_arg(
        &mut self,
        instance: &FunctionInstanceRef,
    ) -> Option<InternedTyId> {
        self.canonicalize_instance_self_arg(instance.self_arg)
    }

    pub(crate) fn canonicalize_global_instance_ref_args(
        &mut self,
        instance: &GlobalInstanceRef,
    ) -> Vec<InternedTyId> {
        self.canonicalize_instance_args(&instance.args)
    }

    pub(crate) fn cached_ty_contains_generic_param(&mut self, ty: InternedTyId) -> bool {
        contains_generic_param(ty, &mut |ty| self.ty_kind(ty).cloned(), None)
    }

    pub(crate) fn cached_ty_contains_unresolved_projection(&mut self, ty: InternedTyId) -> bool {
        contains_unresolved_projection(ty, &mut |ty| self.ty_kind(ty).cloned())
    }

    pub(crate) fn cached_ty_contains_error(&mut self, ty: InternedTyId) -> bool {
        contains_error(ty, &mut |ty| self.ty_kind(ty).cloned(), None)
    }

    fn import_monomorphized_instance_args(&mut self, args: &[InternedTyId]) -> Vec<InternedTyId> {
        self.canonicalize_instance_args(args)
    }
}

fn enqueue_function_instance_ref(
    pending: &mut VecDeque<FunctionInstanceRef>,
    queued: &mut HashSet<FunctionInstanceKey>,
    instance: FunctionInstanceRef,
) {
    if queued.insert(instance.key()) {
        pending.push_back(instance);
    }
}

pub(crate) fn contains_generic_param(
    ty: InternedTyId,
    ty_kind: &mut impl FnMut(InternedTyId) -> Option<TyKind>,
    mut cache: Option<&mut HashMap<InternedTyId, bool>>,
) -> bool {
    if let Some(cache) = cache.as_deref()
        && let Some(cached) = cache.get(&ty)
    {
        return *cached;
    }
    let contains = match ty_kind(ty) {
        Some(TyKind::GenericParam(_)) => true,
        Some(TyKind::SelfParam) => true,
        Some(TyKind::Tuple(elems)) => elems
            .iter()
            .any(|elem| contains_generic_param(*elem, ty_kind, cache.as_deref_mut())),
        Some(
            TyKind::Pointer { elem, .. }
            | TyKind::VolatilePointer { elem, .. }
            | TyKind::Slice { elem, .. }
            | TyKind::SlicePointee { elem },
        ) => contains_generic_param(elem, ty_kind, cache.as_deref_mut()),
        Some(TyKind::Array { len, elem }) => {
            contains_generic_param(elem, ty_kind, cache.as_deref_mut())
                || matches!(len, ArrayLenTy::Builtin { ty, .. }
                    if contains_generic_param(ty, ty_kind, cache.as_deref_mut()))
        }
        Some(TyKind::Range { bound, .. }) => {
            bound.is_some_and(|bound| contains_generic_param(bound, ty_kind, cache.as_deref_mut()))
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
                .any(|param| contains_generic_param(*param, ty_kind, cache.as_deref_mut()))
                || contains_generic_param(return_type, ty_kind, cache.as_deref_mut())
        }
        Some(TyKind::Optional { elem }) => {
            contains_generic_param(elem, ty_kind, cache.as_deref_mut())
        }
        Some(TyKind::ErrorUnion { error, value }) => {
            contains_generic_param(error, ty_kind, cache.as_deref_mut())
                || contains_generic_param(value, ty_kind, cache.as_deref_mut())
        }
        Some(TyKind::Nominal {
            args, const_args, ..
        }) => {
            args.iter()
                .any(|arg| contains_generic_param(*arg, ty_kind, cache.as_deref_mut()))
                || const_args
                    .iter()
                    .any(|arg| contains_generic_param(arg.ty, ty_kind, cache.as_deref_mut()))
        }
        Some(TyKind::BuiltinTrait { args, .. }) => args
            .iter()
            .any(|arg| contains_generic_param(*arg, ty_kind, cache.as_deref_mut())),
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
                .any(|arg| contains_generic_param(*arg, ty_kind, cache.as_deref_mut()))
                || trait_const_args
                    .iter()
                    .any(|arg| contains_generic_param(arg.ty, ty_kind, cache.as_deref_mut()))
                || associated_type_bindings.iter().any(|binding| {
                    binding
                        .trait_args
                        .iter()
                        .any(|arg| contains_generic_param(*arg, ty_kind, cache.as_deref_mut()))
                        || binding.trait_const_args.iter().any(|arg| {
                            contains_generic_param(arg.ty, ty_kind, cache.as_deref_mut())
                        })
                        || contains_generic_param(binding.ty, ty_kind, cache.as_deref_mut())
                })
        }
        Some(TyKind::Projection {
            self_ty,
            trait_args,
            trait_const_args,
            ..
        }) => {
            contains_generic_param(self_ty, ty_kind, cache.as_deref_mut())
                || trait_args
                    .iter()
                    .any(|arg| contains_generic_param(*arg, ty_kind, cache.as_deref_mut()))
                || trait_const_args
                    .iter()
                    .any(|arg| contains_generic_param(arg.ty, ty_kind, cache.as_deref_mut()))
        }
        Some(
            TyKind::Primitive(_)
            | TyKind::Opaque
            | TyKind::BuiltinType(_)
            | TyKind::Vector { .. }
            | TyKind::ConstOnly
            | TyKind::Error
            | TyKind::ClosureState { .. },
        )
        | None => false,
    };
    if let Some(cache) = cache {
        cache.insert(ty, contains);
    }
    contains
}

pub(crate) fn contains_unresolved_projection(
    ty: InternedTyId,
    ty_kind: &mut impl FnMut(InternedTyId) -> Option<TyKind>,
) -> bool {
    match ty_kind(ty) {
        Some(TyKind::Projection { .. }) => true,
        Some(
            TyKind::Pointer { elem, .. }
            | TyKind::VolatilePointer { elem, .. }
            | TyKind::Slice { elem, .. }
            | TyKind::SlicePointee { elem },
        ) => contains_unresolved_projection(elem, ty_kind),
        Some(TyKind::Array { len, elem }) => {
            contains_unresolved_projection(elem, ty_kind)
                || matches!(len, ArrayLenTy::Builtin { ty, .. }
                    if contains_unresolved_projection(ty, ty_kind))
        }
        Some(TyKind::Tuple(elems)) => elems
            .into_iter()
            .any(|elem| contains_unresolved_projection(elem, ty_kind)),
        Some(TyKind::Range { bound, .. }) => {
            bound.is_some_and(|bound| contains_unresolved_projection(bound, ty_kind))
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
                .into_iter()
                .any(|param| contains_unresolved_projection(param, ty_kind))
                || contains_unresolved_projection(return_type, ty_kind)
        }
        Some(TyKind::Optional { elem }) => contains_unresolved_projection(elem, ty_kind),
        Some(TyKind::ErrorUnion { error, value }) => {
            contains_unresolved_projection(error, ty_kind)
                || contains_unresolved_projection(value, ty_kind)
        }
        Some(TyKind::Nominal {
            args, const_args, ..
        }) => {
            args.into_iter()
                .any(|arg| contains_unresolved_projection(arg, ty_kind))
                || const_args
                    .into_iter()
                    .any(|arg| contains_unresolved_projection(arg.ty, ty_kind))
        }
        Some(TyKind::BuiltinTrait { args, .. }) => args
            .into_iter()
            .any(|arg| contains_unresolved_projection(arg, ty_kind)),
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
                .into_iter()
                .any(|arg| contains_unresolved_projection(arg, ty_kind))
                || trait_const_args
                    .into_iter()
                    .any(|arg| contains_unresolved_projection(arg.ty, ty_kind))
                || associated_type_bindings.into_iter().any(|binding| {
                    binding
                        .trait_args
                        .into_iter()
                        .any(|arg| contains_unresolved_projection(arg, ty_kind))
                        || binding
                            .trait_const_args
                            .into_iter()
                            .any(|arg| contains_unresolved_projection(arg.ty, ty_kind))
                        || contains_unresolved_projection(binding.ty, ty_kind)
                })
        }
        Some(TyKind::GenericParam(_))
        | Some(TyKind::SelfParam)
        | Some(
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

pub(crate) fn contains_error(
    ty: InternedTyId,
    ty_kind: &mut impl FnMut(InternedTyId) -> Option<TyKind>,
    cache: Option<&mut HashMap<InternedTyId, bool>>,
) -> bool {
    if let Some(cache) = cache.as_ref()
        && let Some(contains) = cache.get(&ty).copied()
    {
        return contains;
    }
    let contains = match ty_kind(ty) {
        Some(TyKind::Error) => true,
        Some(
            TyKind::Pointer { elem, .. }
            | TyKind::VolatilePointer { elem, .. }
            | TyKind::Slice { elem, .. }
            | TyKind::SlicePointee { elem },
        ) => contains_error(elem, ty_kind, None),
        Some(TyKind::Array { len, elem }) => {
            contains_error(elem, ty_kind, None)
                || matches!(len, ArrayLenTy::Builtin { ty, .. }
                    if contains_error(ty, ty_kind, None))
        }
        Some(TyKind::Tuple(elems)) => elems
            .into_iter()
            .any(|elem| contains_error(elem, ty_kind, None)),
        Some(TyKind::Range { bound, .. }) => {
            bound.is_some_and(|bound| contains_error(bound, ty_kind, None))
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
                .into_iter()
                .any(|param| contains_error(param, ty_kind, None))
                || contains_error(return_type, ty_kind, None)
        }
        Some(TyKind::Optional { elem }) => contains_error(elem, ty_kind, None),
        Some(TyKind::ErrorUnion { error, value }) => {
            contains_error(error, ty_kind, None) || contains_error(value, ty_kind, None)
        }
        Some(TyKind::Nominal {
            args, const_args, ..
        }) => {
            args.into_iter()
                .any(|arg| contains_error(arg, ty_kind, None))
                || const_args
                    .into_iter()
                    .any(|arg| contains_error(arg.ty, ty_kind, None))
        }
        Some(TyKind::BuiltinTrait { args, .. }) => args
            .into_iter()
            .any(|arg| contains_error(arg, ty_kind, None)),
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
                .into_iter()
                .any(|arg| contains_error(arg, ty_kind, None))
                || trait_const_args
                    .into_iter()
                    .any(|arg| contains_error(arg.ty, ty_kind, None))
                || associated_type_bindings.into_iter().any(|binding| {
                    binding
                        .trait_args
                        .into_iter()
                        .any(|arg| contains_error(arg, ty_kind, None))
                        || binding
                            .trait_const_args
                            .into_iter()
                            .any(|arg| contains_error(arg.ty, ty_kind, None))
                        || contains_error(binding.ty, ty_kind, None)
                })
        }
        Some(TyKind::Projection {
            self_ty,
            trait_args,
            ..
        }) => {
            contains_error(self_ty, ty_kind, None)
                || trait_args
                    .into_iter()
                    .any(|arg| contains_error(arg, ty_kind, None))
        }
        Some(TyKind::GenericParam(_))
        | Some(TyKind::SelfParam)
        | Some(
            TyKind::ConstOnly
            | TyKind::Opaque
            | TyKind::Primitive(_)
            | TyKind::BuiltinType(_)
            | TyKind::Vector { .. }
            | TyKind::ClosureState { .. },
        )
        | None => false,
    };
    if let Some(cache) = cache {
        cache.insert(ty, contains);
    }
    contains
}

fn known_backend_function_instance_count(existing: usize, newly_materialized: usize) -> usize {
    existing.saturating_add(newly_materialized)
}

#[cfg(test)]
mod tests {
    use super::*;
    use nia_defs::DefId;
    use nia_ids::{GlobalDefId, ModuleIdAllocator, TypeStoreIndex};
    use nia_ty::{ArrayLenTy, ConstGenericValue, IntConst, PrimitiveTy};

    #[test]
    fn backend_instance_limit_counts_existing_and_new_instances() {
        assert_eq!(
            known_backend_function_instance_count(MAX_BACKEND_FUNCTION_INSTANCES - 1, 0),
            MAX_BACKEND_FUNCTION_INSTANCES - 1
        );
        assert_eq!(
            known_backend_function_instance_count(MAX_BACKEND_FUNCTION_INSTANCES - 1, 1),
            MAX_BACKEND_FUNCTION_INSTANCES
        );
        assert_eq!(
            known_backend_function_instance_count(usize::MAX, 1),
            usize::MAX
        );
    }

    #[test]
    fn generic_param_presence_cache_reuses_recursive_results() {
        let generic = test_ty(0);
        let pointer = test_ty(1);
        let mut calls = 0;
        let mut cache = HashMap::new();

        let first = contains_generic_param(
            pointer,
            &mut |ty| {
                calls += 1;
                match ty.index.index() {
                    0 => Some(TyKind::GenericParam(
                        nia_symbol::SymbolId::from_stable_hash(nia_symbol::stable_hash("T")),
                    )),
                    1 => Some(TyKind::Pointer {
                        is_readonly: true,
                        elem: generic,
                    }),
                    _ => None,
                }
            },
            Some(&mut cache),
        );
        let first_calls = calls;
        let second = contains_generic_param(
            pointer,
            &mut |_| {
                calls += 1;
                None
            },
            Some(&mut cache),
        );

        assert!(first);
        assert!(second);
        assert_eq!(first_calls, 2);
        assert_eq!(calls, first_calls);
    }

    #[test]
    fn generic_param_presence_cache_reuses_negative_results() {
        let int = test_ty(0);
        let slice = test_ty(1);
        let mut calls = 0;
        let mut cache = HashMap::new();

        let first = contains_generic_param(
            slice,
            &mut |ty| {
                calls += 1;
                match ty.index.index() {
                    0 => Some(TyKind::Primitive(PrimitiveTy::I32)),
                    1 => Some(TyKind::Slice {
                        is_readonly: false,
                        elem: int,
                    }),
                    _ => None,
                }
            },
            Some(&mut cache),
        );
        let first_calls = calls;
        let second = contains_generic_param(
            slice,
            &mut |_| {
                calls += 1;
                None
            },
            Some(&mut cache),
        );

        assert!(!first);
        assert!(!second);
        assert_eq!(first_calls, 2);
        assert_eq!(calls, first_calls);
    }

    #[test]
    fn recursive_type_filters_visit_const_argument_types() {
        let mut modules = ModuleIdAllocator::new();
        let module_id = modules.allocate();
        let nominal = test_ty(3);
        let generic = test_ty(0);
        let projection = test_ty(1);
        let error = test_ty(2);
        let const_arg = |ty| ConstGenericArg {
            ty,
            value: ConstGenericValue::Int(IntConst::from(0)),
        };
        let kind = |ty: InternedTyId| match ty.index.index() {
            0 => Some(TyKind::GenericParam(
                nia_symbol::SymbolId::from_stable_hash(nia_symbol::stable_hash("N")),
            )),
            1 => Some(TyKind::Projection {
                self_ty: generic,
                trait_id: nia_ids::TraitId::Source(GlobalDefId {
                    module_id,
                    def_id: DefId(0),
                }),
                trait_args: Vec::new(),
                trait_const_args: Vec::new(),
                name: nia_symbol::SymbolId::from_stable_hash(nia_symbol::stable_hash("Item")),
            }),
            2 => Some(TyKind::Error),
            3 => Some(TyKind::Nominal {
                def_id: GlobalDefId {
                    module_id,
                    def_id: DefId(1),
                },
                args: Vec::new(),
                const_args: vec![const_arg(generic)],
            }),
            4 => Some(TyKind::Nominal {
                def_id: GlobalDefId {
                    module_id,
                    def_id: DefId(1),
                },
                args: Vec::new(),
                const_args: vec![const_arg(projection)],
            }),
            5 => Some(TyKind::Nominal {
                def_id: GlobalDefId {
                    module_id,
                    def_id: DefId(1),
                },
                args: Vec::new(),
                const_args: vec![const_arg(error)],
            }),
            _ => None,
        };

        assert!(contains_generic_param(nominal, &mut |ty| kind(ty), None));
        assert!(contains_unresolved_projection(test_ty(4), &mut |ty| kind(
            ty
        )));
        assert!(contains_error(test_ty(5), &mut |ty| kind(ty), None));
    }

    #[test]
    fn recursive_type_filters_visit_layout_builtin_array_length_types() {
        let mut modules = ModuleIdAllocator::new();
        let module_id = modules.allocate();
        let generic = test_ty(0);
        let projection = test_ty(1);
        let error = test_ty(2);
        let array = |len_ty| TyKind::Array {
            len: ArrayLenTy::Builtin {
                builtin: nia_ty::LayoutBuiltin::Size,
                ty: len_ty,
            },
            elem: test_ty(3),
        };
        let kind = |ty: InternedTyId| match ty.index.index() {
            0 => Some(TyKind::GenericParam(
                nia_symbol::SymbolId::from_stable_hash(nia_symbol::stable_hash("N")),
            )),
            1 => Some(TyKind::Projection {
                self_ty: test_ty(3),
                trait_id: nia_ids::TraitId::Source(GlobalDefId {
                    module_id,
                    def_id: DefId(0),
                }),
                trait_args: Vec::new(),
                trait_const_args: Vec::new(),
                name: nia_symbol::SymbolId::from_stable_hash(nia_symbol::stable_hash("Item")),
            }),
            2 => Some(TyKind::Error),
            3 => Some(TyKind::Primitive(PrimitiveTy::U8)),
            4 => Some(array(generic)),
            5 => Some(array(projection)),
            6 => Some(array(error)),
            _ => None,
        };

        assert!(contains_generic_param(test_ty(4), &mut |ty| kind(ty), None));
        assert!(contains_unresolved_projection(test_ty(5), &mut |ty| kind(
            ty
        )));
        assert!(contains_error(test_ty(6), &mut |ty| kind(ty), None));
    }

    fn test_ty(index: u32) -> InternedTyId {
        static TYPE_STORE: std::sync::OnceLock<nia_ty::TypeStore> = std::sync::OnceLock::new();
        let type_store = TYPE_STORE.get_or_init(nia_ty::TypeStore::new);
        InternedTyId::new(type_store.id(), TypeStoreIndex::from_store_index(index))
    }
}
