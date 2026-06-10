// SPDX-License-Identifier: GPL-3.0-or-later
use std::collections::{HashMap, HashSet, VecDeque};

use crate::ModuleLowerer;
use crate::function_refs::{
    FunctionInstanceKey, FunctionInstanceRef, FunctionRefs,
    collect_function_refs_from_optional_body,
};
use nia_backend_ir::{
    BackendFunction, BackendFunctionAttribute, BackendFunctionInstance, BackendParam,
};
use nia_function_ir::{FunctionBody, FunctionLocal, FunctionLocalKind};
use nia_ids::{GlobalDefId, InternedTyId, ModuleId};
use nia_item_signatures::FunctionAttribute;
use nia_ty::TyKind;

type InstanceKey = (GlobalDefId, ModuleId, Vec<InternedTyId>);

struct PlannedFunctionInstance {
    def_id: GlobalDefId,
    arg_module_id: ModuleId,
    args: Vec<InternedTyId>,
    symbol: String,
}

const MAX_BACKEND_FUNCTION_INSTANCES: usize = 4096;
const MAX_BACKEND_INSTANCE_TYPE_DEPTH: usize = 256;

impl<'a> ModuleLowerer<'a> {
    pub(crate) fn lower_function_instances(
        &mut self,
        functions: &[BackendFunction],
    ) -> Vec<BackendFunctionInstance> {
        let mut initial_instances = Vec::new();
        for instance in self
            .monomorphization
            .instances
            .iter()
            .filter(|instance| instance.def_id.module_id == self.input.module_id)
        {
            let args = self.import_monomorphized_instance_args(&instance.args);
            initial_instances.push(FunctionInstanceRef {
                def_id: instance.def_id,
                arg_module_id: instance.arg_module_id,
                args,
                span: instance.span,
            });
        }

        let mut root_refs = FunctionRefs::default();
        for function in functions {
            if !function.generics.is_empty() {
                continue;
            }
            collect_function_refs_from_optional_body(
                self.input.module_id,
                &function.function_body,
                &mut root_refs,
            );
        }
        initial_instances.extend(root_refs.instances);
        self.lower_function_instances_from_refs(functions, initial_instances, &[])
    }

    pub(crate) fn lower_additional_function_instances(
        &mut self,
        refs: Vec<FunctionInstanceRef>,
        existing: &[BackendFunctionInstance],
    ) -> Vec<BackendFunctionInstance> {
        self.lower_function_instances_from_refs(&[], refs, existing)
    }

    fn lower_function_instances_from_refs(
        &mut self,
        functions: &[BackendFunction],
        initial_instances: Vec<FunctionInstanceRef>,
        existing: &[BackendFunctionInstance],
    ) -> Vec<BackendFunctionInstance> {
        let mut instances = Vec::new();
        let mut seen = HashSet::<InstanceKey>::new();
        let mut queued = HashSet::<FunctionInstanceKey>::new();
        let mut functions_by_def = functions
            .iter()
            .map(|function| (function.def_id, function.clone()))
            .collect::<HashMap<_, _>>();
        let mut pending = VecDeque::new();
        for instance in existing {
            let args = self.canonicalize_instance_args(&instance.args);
            seen.insert((instance.def_id, instance.arg_module_id, args));
        }
        for instance in initial_instances {
            enqueue_function_instance_ref(&mut pending, &mut queued, instance);
        }

        let mut planned_symbols = self
            .monomorphization
            .instances
            .iter()
            .map(|instance| {
                (
                    (
                        instance.def_id,
                        instance.arg_module_id,
                        self.canonicalize_instance_args(&instance.args),
                    ),
                    instance.symbol.clone(),
                )
            })
            .collect::<HashMap<_, _>>();

        while let Some(instance) = pending.pop_front() {
            if instance.def_id.module_id != self.input.module_id {
                self.foreign_function_instance_refs.push(instance);
                continue;
            }
            let args = self.canonicalize_instance_args(&instance.args);
            let key = (instance.def_id, instance.arg_module_id, args.clone());
            if seen.contains(&key) {
                continue;
            }
            if args.iter().any(|arg| {
                self.cached_ty_contains_generic_param(*arg)
                    || self.cached_ty_contains_unresolved_projection(*arg)
                    || self.cached_ty_is_error(*arg)
            }) {
                continue;
            }
            if args.iter().any(|arg| {
                self.ty_exceeds_backend_instance_depth(*arg, MAX_BACKEND_INSTANCE_TYPE_DEPTH)
            }) {
                self.report_backend_instance_type_depth_limit(instance.span, instance.def_id);
                continue;
            }
            if instances.len() >= MAX_BACKEND_FUNCTION_INSTANCES {
                self.report_backend_instance_limit(
                    instance.span,
                    instance.def_id,
                    &args,
                    instances.len(),
                );
                continue;
            }
            let symbol = if let Some(symbol) =
                planned_symbols.get(&(instance.def_id, instance.arg_module_id, args.clone()))
            {
                symbol.clone()
            } else {
                let Some(name) =
                    self.function_instance_name(&mut functions_by_def, instance.def_id)
                else {
                    continue;
                };
                let symbol = self.mangle_instance_symbol(instance.def_id, &name, &args);
                planned_symbols.insert(
                    (instance.def_id, instance.arg_module_id, args.clone()),
                    symbol.clone(),
                );
                symbol
            };
            let Some(body) = self.lower_planned_function_instance(
                &mut functions_by_def,
                &mut seen,
                &mut instances,
                PlannedFunctionInstance {
                    def_id: instance.def_id,
                    arg_module_id: instance.arg_module_id,
                    args,
                    symbol,
                },
            ) else {
                continue;
            };
            let mut refs = FunctionRefs::default();
            collect_function_refs_from_optional_body(
                instance.arg_module_id,
                &Some(body),
                &mut refs,
            );
            for discovered in refs.instances {
                let discovered_args = self.canonicalize_instance_args(&discovered.args);
                if !seen.contains(&(
                    discovered.def_id,
                    discovered.arg_module_id,
                    discovered_args.clone(),
                )) {
                    enqueue_function_instance_ref(
                        &mut pending,
                        &mut queued,
                        FunctionInstanceRef {
                            def_id: discovered.def_id,
                            arg_module_id: discovered.arg_module_id,
                            args: discovered_args,
                            span: discovered.span,
                        },
                    );
                }
            }
        }
        instances
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
            .unwrap_or_else(|| format!("def{}", def_id.def_id.0));
        let type_args = args
            .iter()
            .map(|arg| self.instance_arg_debug_name(*arg))
            .collect::<Vec<_>>()
            .join(", ");
        self.diagnostics.push(
            nia_diagnostic::Diagnostic::user_error(
                "E0601",
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
            .unwrap_or_else(|| format!("def{}", def_id.def_id.0));
        self.diagnostics.push(
            nia_diagnostic::Diagnostic::user_error(
                "E0601",
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
        if ty.interner_id == self.interner.interner_id() {
            return self
                .interner
                .get(ty)
                .map(|kind| format!("{kind:?}"))
                .unwrap_or_else(|| format!("{ty:?}"));
        }
        self.known_interner_containing_ty(ty)
            .and_then(|interner| interner.get(ty))
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
            TyKind::Pointer { elem, .. }
            | TyKind::Slice { elem, .. }
            | TyKind::SlicePointee { elem } => self.ty_exceeds_backend_instance_depth(*elem, next),
            TyKind::Array { elem, .. } => self.ty_exceeds_backend_instance_depth(*elem, next),
            TyKind::Range { bound, .. } => {
                bound.is_some_and(|bound| self.ty_exceeds_backend_instance_depth(bound, next))
            }
            TyKind::FunctionPointer {
                params,
                return_type,
                ..
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
            TyKind::Nominal { args, .. } | TyKind::BuiltinTrait { args, .. } => args
                .iter()
                .any(|arg| self.ty_exceeds_backend_instance_depth(*arg, next)),
            TyKind::TraitObject {
                trait_args,
                associated_type_bindings,
                ..
            }
            | TyKind::TraitObjectPointee {
                trait_args,
                associated_type_bindings,
                ..
            } => {
                trait_args
                    .iter()
                    .any(|arg| self.ty_exceeds_backend_instance_depth(*arg, next))
                    || associated_type_bindings.iter().any(|binding| {
                        binding
                            .trait_args
                            .iter()
                            .any(|arg| self.ty_exceeds_backend_instance_depth(*arg, next))
                            || self.ty_exceeds_backend_instance_depth(binding.ty, next)
                    })
            }
            TyKind::Projection {
                self_ty,
                trait_args,
                ..
            } => {
                self.ty_exceeds_backend_instance_depth(*self_ty, next)
                    || trait_args
                        .iter()
                        .any(|arg| self.ty_exceeds_backend_instance_depth(*arg, next))
            }
            TyKind::GenericParam(_)
            | TyKind::Primitive(_)
            | TyKind::Vector { .. }
            | TyKind::ComptimeOnly
            | TyKind::Error => false,
        }
    }

    fn function_instance_name(
        &mut self,
        functions_by_def: &mut HashMap<GlobalDefId, BackendFunction>,
        def_id: GlobalDefId,
    ) -> Option<String> {
        if !functions_by_def.contains_key(&def_id)
            && let Some(function) = self.backend_function_template_for_program_def(def_id)
        {
            functions_by_def.insert(def_id, function);
        }
        functions_by_def
            .get(&def_id)
            .map(|function| function.name.clone())
    }

    fn lower_planned_function_instance(
        &mut self,
        functions_by_def: &mut HashMap<GlobalDefId, BackendFunction>,
        seen: &mut HashSet<InstanceKey>,
        instances: &mut Vec<BackendFunctionInstance>,
        plan: PlannedFunctionInstance,
    ) -> Option<nia_function_ir::FunctionBody> {
        let PlannedFunctionInstance {
            def_id,
            arg_module_id,
            args,
            symbol,
        } = plan;
        if !seen.insert((def_id, arg_module_id, args.clone())) {
            return None;
        }
        if !functions_by_def.contains_key(&def_id)
            && let Some(function) = self.backend_function_template_for_program_def(def_id)
        {
            functions_by_def.insert(def_id, function);
        }
        let base = functions_by_def.get(&def_id).cloned()?;
        let imported_args = args
            .iter()
            .map(|arg| self.import_instance_arg_type(*arg))
            .collect::<Vec<_>>();
        let substitutions = ModuleLowerer::generic_substitutions(&base.generics, &imported_args);
        let function_body = base.function_body.clone().map(|body| {
            self.instantiate_function_body(
                def_id,
                arg_module_id,
                true,
                args.len(),
                body,
                &substitutions,
            )
        });
        let discovered_body = function_body.clone();
        instances.push(BackendFunctionInstance {
            def_id,
            name: base.name.clone(),
            arg_module_id,
            args,
            symbol,
            params: self.instantiate_params(&base, &substitutions),
            return_type: self.instantiate_ty(base.return_type, &substitutions),
            is_extern: base.is_extern,
            is_variadic: base.is_variadic,
            attributes: base.attributes.clone(),
            function_body,
            span: base.span,
        });
        discovered_body
    }

    fn backend_function_template_for_program_def(
        &mut self,
        def_id: GlobalDefId,
    ) -> Option<BackendFunction> {
        let signature = self.input.program_functions.get(&def_id)?;
        if signature.signature.is_comptime {
            return None;
        }
        let source_interner = &signature.interner;
        let body_interner = self
            .input
            .program_type_interners
            .get(&def_id.module_id)
            .copied()
            .unwrap_or(source_interner);
        let own_generics = &signature.signature.generics;
        let effective_generics = self.effective_generics(def_id, own_generics).to_vec();
        let identity_substitutions = effective_generics
            .iter()
            .map(|generic| {
                (
                    generic.clone(),
                    self.interner.intern(TyKind::GenericParam(generic.clone())),
                )
            })
            .collect::<HashMap<_, _>>();
        let raw_function_body = self.input.program_function_bodies.get(&def_id).cloned();
        let param_locals = raw_function_body
            .as_ref()
            .map(|body| self.template_param_locals(def_id, &signature.signature.params, body))
            .unwrap_or_default();
        let function_body = raw_function_body.map(|body| {
            self.instantiate_function_body(
                def_id,
                self.input.module_id,
                true,
                0,
                body,
                &identity_substitutions,
            )
        });
        Some(BackendFunction {
            def_id,
            name: signature.name.clone(),
            generics: effective_generics,
            params: signature
                .signature
                .params
                .iter()
                .enumerate()
                .map(|(index, param)| {
                    let signature_ty =
                        nia_ty::import_type_into(&mut self.interner, source_interner, param.ty);
                    let param_local = param_locals.get(index).copied();
                    let local_ty = if param.receiver.is_some() {
                        param_local
                            .map(|(_, ty)| {
                                nia_ty::import_type_into(&mut self.interner, body_interner, ty)
                            })
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
                        name: param.name.clone(),
                        receiver: param.receiver,
                        passing_ty,
                        local_ty,
                        span: param.span,
                    }
                })
                .collect(),
            return_type: nia_ty::import_type_into(
                &mut self.interner,
                source_interner,
                signature.signature.return_type,
            ),
            is_extern: signature.signature.is_extern,
            is_variadic: signature.signature.is_variadic,
            attributes: signature
                .signature
                .attributes
                .iter()
                .map(|attribute| match attribute {
                    FunctionAttribute::Naked => BackendFunctionAttribute::Naked,
                })
                .collect(),
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
                && local.name != *name
            {
                self.report_backend_template_param_name_mismatch(def_id, index, name, local);
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
                "I0300",
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
        signature_name: &str,
        local: &FunctionLocal,
    ) {
        self.diagnostics.push(
            nia_diagnostic::Diagnostic::internal_error(
                "I0300",
                "backend function template parameter local order does not match its signature",
            )
            .primary(
                local.span,
                "backend function template parameter local order does not match its signature",
            )
            .debug("def_id", def_id)
            .debug("param_index", index)
            .debug("signature_name", signature_name)
            .debug("local_name", local.name.as_str())
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
        let local = if arg.interner_id == self.interner.interner_id()
            && let Some(kind) = self.interner.get(arg)
            && !matches!(kind, TyKind::Error)
        {
            arg
        } else if let Some(source) = self.known_interner_containing_ty(arg).cloned()
            && let Some(kind) = source.get(arg)
            && !matches!(kind, TyKind::Error)
        {
            nia_ty::import_type_into(&mut self.interner, &source, arg)
        } else if arg.interner_id == self.interner.interner_id() && self.interner.get(arg).is_some()
        {
            arg
        } else if let Some(source) = self.known_interner_containing_ty(arg).cloned() {
            nia_ty::import_type_into(&mut self.interner, &source, arg)
        } else {
            arg
        };
        self.instantiate_ty(local, &HashMap::new())
    }

    fn cached_ty_contains_generic_param(&mut self, ty: InternedTyId) -> bool {
        let current_interner = self.interner.clone();
        contains_generic_param(
            ty,
            &mut |ty| {
                (ty.interner_id == current_interner.interner_id())
                    .then(|| current_interner.get(ty).cloned())
                    .flatten()
                    .or_else(|| self.ty_kind(ty).cloned())
                    .or_else(|| Some(TyKind::GenericParam("<unknown>".to_string())))
            },
            None,
        )
    }

    fn cached_ty_contains_unresolved_projection(&mut self, ty: InternedTyId) -> bool {
        let current_interner = self.interner.clone();
        contains_unresolved_projection(ty, &mut |ty| {
            (ty.interner_id == current_interner.interner_id())
                .then(|| current_interner.get(ty).cloned())
                .flatten()
                .or_else(|| self.ty_kind(ty).cloned())
        })
    }

    fn cached_ty_is_error(&mut self, ty: InternedTyId) -> bool {
        matches!(self.ty_kind(ty), Some(TyKind::Error))
    }

    fn import_monomorphized_instance_args(&mut self, args: &[InternedTyId]) -> Vec<InternedTyId> {
        args.iter()
            .copied()
            .map(|arg| {
                self.monomorphization
                    .type_interners
                    .get(&arg.interner_id)
                    .map(|source| nia_ty::import_type_into(&mut self.interner, source, arg))
                    .unwrap_or(arg)
            })
            .collect()
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
        Some(
            TyKind::Pointer { elem, .. }
            | TyKind::Slice { elem, .. }
            | TyKind::SlicePointee { elem },
        ) => contains_generic_param(elem, ty_kind, cache.as_deref_mut()),
        Some(TyKind::Array { elem, .. }) => {
            contains_generic_param(elem, ty_kind, cache.as_deref_mut())
        }
        Some(TyKind::Range { bound, .. }) => {
            bound.is_some_and(|bound| contains_generic_param(bound, ty_kind, cache.as_deref_mut()))
        }
        Some(TyKind::FunctionPointer {
            params,
            return_type,
            ..
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
        Some(TyKind::Nominal { args, .. } | TyKind::BuiltinTrait { args, .. }) => args
            .iter()
            .any(|arg| contains_generic_param(*arg, ty_kind, cache.as_deref_mut())),
        Some(TyKind::TraitObject {
            trait_args,
            associated_type_bindings,
            ..
        })
        | Some(TyKind::TraitObjectPointee {
            trait_args,
            associated_type_bindings,
            ..
        }) => {
            trait_args
                .iter()
                .any(|arg| contains_generic_param(*arg, ty_kind, cache.as_deref_mut()))
                || associated_type_bindings.iter().any(|binding| {
                    binding
                        .trait_args
                        .iter()
                        .any(|arg| contains_generic_param(*arg, ty_kind, cache.as_deref_mut()))
                        || contains_generic_param(binding.ty, ty_kind, cache.as_deref_mut())
                })
        }
        Some(TyKind::Projection {
            self_ty,
            trait_args,
            ..
        }) => {
            contains_generic_param(self_ty, ty_kind, cache.as_deref_mut())
                || trait_args
                    .iter()
                    .any(|arg| contains_generic_param(*arg, ty_kind, cache.as_deref_mut()))
        }
        Some(
            TyKind::Primitive(_) | TyKind::Vector { .. } | TyKind::ComptimeOnly | TyKind::Error,
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
            | TyKind::Slice { elem, .. }
            | TyKind::SlicePointee { elem },
        ) => contains_unresolved_projection(elem, ty_kind),
        Some(TyKind::Array { elem, .. }) => contains_unresolved_projection(elem, ty_kind),
        Some(TyKind::Range { bound, .. }) => {
            bound.is_some_and(|bound| contains_unresolved_projection(bound, ty_kind))
        }
        Some(TyKind::FunctionPointer {
            params,
            return_type,
            ..
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
        Some(TyKind::Nominal { args, .. } | TyKind::BuiltinTrait { args, .. }) => args
            .into_iter()
            .any(|arg| contains_unresolved_projection(arg, ty_kind)),
        Some(TyKind::TraitObject {
            trait_args,
            associated_type_bindings,
            ..
        })
        | Some(TyKind::TraitObjectPointee {
            trait_args,
            associated_type_bindings,
            ..
        }) => {
            trait_args
                .into_iter()
                .any(|arg| contains_unresolved_projection(arg, ty_kind))
                || associated_type_bindings.into_iter().any(|binding| {
                    binding
                        .trait_args
                        .into_iter()
                        .any(|arg| contains_unresolved_projection(arg, ty_kind))
                        || contains_unresolved_projection(binding.ty, ty_kind)
                })
        }
        Some(TyKind::GenericParam(_))
        | Some(
            TyKind::Error | TyKind::ComptimeOnly | TyKind::Primitive(_) | TyKind::Vector { .. },
        )
        | None => false,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use nia_ids::{ModuleId, TyInternerIndex};
    use nia_ty::PrimitiveTy;

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
                    0 => Some(TyKind::GenericParam("T".to_string())),
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

    fn test_ty(index: u32) -> InternedTyId {
        InternedTyId::new(ModuleId(0), TyInternerIndex::from_interner_index(index))
    }
}
