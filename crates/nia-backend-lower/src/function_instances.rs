// SPDX-License-Identifier: GPL-3.0-or-later
use std::collections::{HashMap, HashSet, VecDeque};

use crate::ModuleLowerer;
use crate::function_refs::{
    FunctionInstanceRef, FunctionRefs, collect_function_refs_from_optional_body,
};
use nia_backend_ir::{
    BackendFunction, BackendFunctionAttribute, BackendFunctionInstance, BackendParam,
};
use nia_ids::{GlobalDefId, InternedTyId, ModuleId};
use nia_item_signatures::FunctionAttribute;
use nia_ty::TyKind;

type InstanceKey = (GlobalDefId, ModuleId, Vec<InternedTyId>);

impl<'a> ModuleLowerer<'a> {
    pub(crate) fn lower_function_instances(
        &mut self,
        functions: &[BackendFunction],
    ) -> Vec<BackendFunctionInstance> {
        let mut instances = Vec::new();
        let mut seen = HashSet::<InstanceKey>::new();
        let mut functions_by_def = functions
            .iter()
            .map(|function| (function.def_id, function.clone()))
            .collect::<HashMap<_, _>>();
        let mut pending = self
            .monomorphization
            .instances
            .iter()
            .filter(|instance| instance.def_id.module_id == self.input.module_id)
            .map(|instance| FunctionInstanceRef {
                def_id: instance.def_id,
                arg_module_id: instance.arg_module_id,
                args: instance.args.clone(),
            })
            .collect::<VecDeque<_>>();
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
                continue;
            }
            let args = self.canonicalize_instance_args(&instance.args);
            if args.iter().any(|arg| {
                self.cached_ty_contains_generic_param(*arg)
                    || self.cached_ty_contains_unresolved_projection(*arg)
            }) {
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
                instance.def_id,
                instance.arg_module_id,
                args,
                symbol,
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
                if !seen.contains(&(
                    discovered.def_id,
                    discovered.arg_module_id,
                    self.canonicalize_instance_args(&discovered.args),
                )) {
                    pending.push_back(discovered);
                }
            }
        }
        instances
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
        def_id: GlobalDefId,
        arg_module_id: ModuleId,
        args: Vec<InternedTyId>,
        symbol: String,
    ) -> Option<nia_function_ir::FunctionBody> {
        if !seen.insert((def_id, arg_module_id, args.clone())) {
            return None;
        }
        if !functions_by_def.contains_key(&def_id)
            && let Some(function) = self.backend_function_template_for_program_def(def_id)
        {
            functions_by_def.insert(def_id, function);
        }
        let Some(base) = functions_by_def.get(&def_id).cloned() else {
            return None;
        };
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
        let param_local_tys = raw_function_body
            .as_ref()
            .map(|body| {
                body.locals
                    .iter()
                    .filter(|local| local.kind == nia_function_ir::FunctionLocalKind::Param)
                    .map(|local| local.ty)
                    .collect::<Vec<_>>()
            })
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
                    let local_ty = if param.receiver.is_some() {
                        param_local_tys
                            .get(index)
                            .copied()
                            .map(|ty| {
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
                        local_id: None,
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
        let local = if arg.interner_id == self.interner.interner_id() {
            arg
        } else if let Some(source) = self.known_type_interners.get(&arg.interner_id).copied() {
            nia_ty::import_type_into(&mut self.interner, source, arg)
        } else {
            arg
        };
        self.instantiate_ty(local, &HashMap::new())
    }

    fn cached_ty_contains_generic_param(&mut self, ty: InternedTyId) -> bool {
        let current_interner = &self.interner;
        let known_type_interners = &self.known_type_interners;
        contains_generic_param(
            ty,
            &mut |ty| {
                (ty.interner_id == current_interner.interner_id())
                    .then(|| current_interner.get(ty).cloned())
                    .flatten()
                    .or_else(|| {
                        known_type_interners
                            .get(&ty.interner_id)
                            .and_then(|interner| interner.get(ty))
                            .cloned()
                    })
                    .or_else(|| Some(TyKind::GenericParam("<unknown>".to_string())))
            },
            None,
        )
    }

    fn cached_ty_contains_unresolved_projection(&mut self, ty: InternedTyId) -> bool {
        let current_interner = &self.interner;
        let known_type_interners = &self.known_type_interners;
        contains_unresolved_projection(ty, &mut |ty| {
            (ty.interner_id == current_interner.interner_id())
                .then(|| current_interner.get(ty).cloned())
                .flatten()
                .or_else(|| {
                    known_type_interners
                        .get(&ty.interner_id)
                        .and_then(|interner| interner.get(ty))
                        .cloned()
                })
        })
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
        Some(TyKind::Pointer { elem, .. } | TyKind::Slice { elem, .. }) => {
            contains_generic_param(elem, ty_kind, cache.as_deref_mut())
        }
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
        Some(TyKind::Primitive(_) | TyKind::ComptimeOnly | TyKind::Error) | None => false,
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
        Some(TyKind::Pointer { elem, .. } | TyKind::Slice { elem, .. }) => {
            contains_unresolved_projection(elem, ty_kind)
        }
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
        | Some(TyKind::Error | TyKind::ComptimeOnly | TyKind::Primitive(_))
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
