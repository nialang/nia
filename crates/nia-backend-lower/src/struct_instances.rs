// SPDX-License-Identifier: GPL-3.0-or-later
use std::collections::HashSet;

use crate::{ModuleLowerer, function_instances::contains_generic_param};
use nia_backend_ir::{
    BackendField, BackendFunction, BackendFunctionInstance, BackendStructInstance,
    BackendUnionInstance,
};
use nia_defs::DefKind;
use nia_function_ir::FunctionBody;
use nia_ids::{GlobalDefId, InternedTyId};
use nia_node_id::VersionedNodeKey;
use nia_span::Span;
use nia_ty::{ConstGenericArg, TyKind};

type AggregateInstanceKey = (GlobalDefId, Vec<InternedTyId>, Vec<ConstGenericArg>);

impl<'a> ModuleLowerer<'a> {
    pub(crate) fn lower_struct_instances(
        &mut self,
        node_key: &VersionedNodeKey,
        span: Span,
        item: &nia_ast::StructItem,
    ) -> Vec<BackendStructInstance> {
        let Some(def_id) = self.def_id_for_node(node_key, DefKind::Struct) else {
            return Vec::new();
        };
        let Some(signature) = self.input.signatures.structs.get(&def_id) else {
            return Vec::new();
        };
        if signature.generics.is_empty() {
            return Vec::new();
        }
        let keys = self
            .struct_layout_instances_by_def
            .get(&def_id)
            .into_iter()
            .flatten()
            .cloned()
            .collect::<Vec<_>>();
        let mut instances = Vec::new();
        for mut key in keys {
            key.const_args = key
                .const_args
                .iter()
                .map(|arg| self.canonicalize_instance_const_arg(arg))
                .collect();
            if self.instance_args_contain_generic_param(&key.args) {
                continue;
            }
            let global_def_id = self.global_def_id(def_id);
            let (substitutions, const_substitutions) = self
                .generic_substitutions_and_consts_for_def(
                    global_def_id,
                    &key.args,
                    &key.const_args,
                );
            let substitution_id =
                self.intern_type_and_const_substitutions(&substitutions, &const_substitutions);
            let Some(symbol) = self.mangle_type_instance_symbol(
                self.global_def_id(def_id),
                item.name,
                &key.args,
                &key.const_args,
            ) else {
                continue;
            };
            instances.push(BackendStructInstance {
                def_id: global_def_id,
                name: item.name,
                args: key.args.clone(),
                const_args: key.const_args.clone(),
                symbol,
                fields: signature
                    .fields
                    .iter()
                    .map(|field| BackendField {
                        def_id: self.global_def_id(field.def_id),
                        name: field.name,
                        ty: self.instantiate_ty_with_id(field.ty, substitution_id),
                        span: field.span,
                    })
                    .collect(),
                is_extern: signature.is_extern,
                span,
            });
        }
        instances
    }

    pub(crate) fn lower_union_instances(
        &mut self,
        node_key: &VersionedNodeKey,
        span: Span,
        item: &nia_ast::UnionItem,
    ) -> Vec<BackendUnionInstance> {
        let Some(def_id) = self.def_id_for_node(node_key, DefKind::Union) else {
            return Vec::new();
        };
        let Some(signature) = self.input.signatures.unions.get(&def_id) else {
            return Vec::new();
        };
        if signature.generics.is_empty() {
            return Vec::new();
        }
        let keys = self
            .union_layout_instances_by_def
            .get(&def_id)
            .into_iter()
            .flatten()
            .cloned()
            .collect::<Vec<_>>();
        let mut instances = Vec::new();
        for mut key in keys {
            key.const_args = key
                .const_args
                .iter()
                .map(|arg| self.canonicalize_instance_const_arg(arg))
                .collect();
            if self.instance_args_contain_generic_param(&key.args) {
                continue;
            }
            let global_def_id = self.global_def_id(def_id);
            let (substitutions, const_substitutions) = self
                .generic_substitutions_and_consts_for_def(
                    global_def_id,
                    &key.args,
                    &key.const_args,
                );
            let substitution_id =
                self.intern_type_and_const_substitutions(&substitutions, &const_substitutions);
            let Some(symbol) = self.mangle_type_instance_symbol(
                self.global_def_id(def_id),
                item.name,
                &key.args,
                &key.const_args,
            ) else {
                continue;
            };
            instances.push(BackendUnionInstance {
                def_id: global_def_id,
                name: item.name,
                args: key.args.clone(),
                const_args: key.const_args.clone(),
                symbol,
                fields: signature
                    .fields
                    .iter()
                    .map(|field| BackendField {
                        def_id: self.global_def_id(field.def_id),
                        name: field.name,
                        ty: self.instantiate_ty_with_id(field.ty, substitution_id),
                        span: field.span,
                    })
                    .collect(),
                is_extern: signature.is_extern,
                span,
            });
        }
        instances
    }

    pub(crate) fn extend_struct_instances_from_functions(
        &mut self,
        struct_instances: &mut Vec<BackendStructInstance>,
        union_instances: &mut Vec<BackendUnionInstance>,
        functions: &[BackendFunction],
        function_instances: &[BackendFunctionInstance],
    ) {
        let mut types = Vec::new();
        for function in functions {
            types.push(function.return_type);
            types.extend(
                function
                    .params
                    .iter()
                    .flat_map(|param| [param.passing_ty, param.local_ty]),
            );
            if let Some(body) = &function.function_body {
                types.extend(self.body_types(body));
            }
        }
        for function in function_instances {
            types.push(function.return_type);
            types.extend(
                function
                    .params
                    .iter()
                    .flat_map(|param| [param.passing_ty, param.local_ty]),
            );
            if let Some(body) = &function.function_body {
                types.extend(self.body_types(body));
            }
        }
        self.extend_struct_instances_from_types(types, struct_instances, union_instances);
    }

    /// The types a function body uses, from the one body traversal that
    /// every backend consumer shares: expression, local, place and callee
    /// types and the operands of layout queries. Struct and union instances
    /// are the nominal instances among them.
    fn body_types(&self, body: &FunctionBody) -> Vec<InternedTyId> {
        body.value_refs(self.type_context.type_store())
            .types
            .into_iter()
            .collect()
    }

    pub(crate) fn extend_struct_instances_from_types(
        &mut self,
        types: impl IntoIterator<Item = InternedTyId>,
        struct_instances: &mut Vec<BackendStructInstance>,
        union_instances: &mut Vec<BackendUnionInstance>,
    ) {
        let mut seen = struct_instances
            .iter()
            .map(|item| (item.def_id, item.args.clone(), item.const_args.clone()))
            .collect::<HashSet<_>>();
        let mut seen_unions = union_instances
            .iter()
            .map(|item| (item.def_id, item.args.clone(), item.const_args.clone()))
            .collect::<HashSet<_>>();
        for ty in types {
            self.collect_struct_instance_ty(ty, &mut seen, struct_instances);
            self.collect_union_instance_ty(ty, &mut seen_unions, union_instances);
        }
        let mut struct_index = 0usize;
        let mut union_index = 0usize;
        while struct_index < struct_instances.len() || union_index < union_instances.len() {
            while struct_index < struct_instances.len() {
                let fields = struct_instances[struct_index].fields.clone();
                for field in fields {
                    self.collect_struct_instance_ty(field.ty, &mut seen, struct_instances);
                    self.collect_union_instance_ty(field.ty, &mut seen_unions, union_instances);
                }
                struct_index += 1;
            }
            while union_index < union_instances.len() {
                let fields = union_instances[union_index].fields.clone();
                for field in fields {
                    self.collect_struct_instance_ty(field.ty, &mut seen, struct_instances);
                    self.collect_union_instance_ty(field.ty, &mut seen_unions, union_instances);
                }
                union_index += 1;
            }
        }
    }

    fn collect_struct_instance_ty(
        &mut self,
        ty: InternedTyId,
        seen: &mut HashSet<AggregateInstanceKey>,
        out: &mut Vec<BackendStructInstance>,
    ) {
        match self.ty_kind(ty).cloned() {
            Some(TyKind::Tuple(elems)) => {
                for elem in elems {
                    self.collect_struct_instance_ty(elem, seen, out);
                }
            }
            Some(TyKind::Pointer { elem, .. })
            | Some(TyKind::VolatilePointer { elem, .. })
            | Some(TyKind::Slice { elem, .. })
            | Some(TyKind::SlicePointee { elem }) => {
                self.collect_struct_instance_ty(elem, seen, out);
            }
            Some(TyKind::Array { len, elem }) => {
                if let nia_ty::ArrayLenTy::Builtin { ty, .. } = len {
                    self.collect_struct_instance_ty(ty, seen, out);
                }
                self.collect_struct_instance_ty(elem, seen, out);
            }
            Some(TyKind::Range { bound, .. }) => {
                if let Some(bound) = bound {
                    self.collect_struct_instance_ty(bound, seen, out);
                }
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
                for param in params {
                    self.collect_struct_instance_ty(param, seen, out);
                }
                self.collect_struct_instance_ty(return_type, seen, out);
            }
            Some(TyKind::FunctionItem {
                args,
                const_args,
                params,
                return_type,
                self_arg,
                ..
            }) => {
                if let Some(self_arg) = self_arg {
                    self.collect_struct_instance_ty(self_arg, seen, out);
                }
                for arg in args {
                    self.collect_struct_instance_ty(arg, seen, out);
                }
                for arg in const_args {
                    arg.visit_referenced_types(|ty| self.collect_struct_instance_ty(ty, seen, out));
                }
                for param in params {
                    self.collect_struct_instance_ty(param, seen, out);
                }
                self.collect_struct_instance_ty(return_type, seen, out);
            }
            Some(TyKind::Optional { elem }) => self.collect_struct_instance_ty(elem, seen, out),
            Some(TyKind::ErrorUnion { error, value }) => {
                self.collect_struct_instance_ty(error, seen, out);
                self.collect_struct_instance_ty(value, seen, out);
            }
            Some(TyKind::Nominal {
                def_id,
                args,
                const_args,
            }) => {
                for arg in &args {
                    self.collect_struct_instance_ty(*arg, seen, out);
                }
                for arg in &const_args {
                    arg.visit_referenced_types(|ty| self.collect_struct_instance_ty(ty, seen, out));
                }
                if seen.insert((def_id, args.clone(), const_args.clone())) {
                    if !args.is_empty() || !const_args.is_empty() {
                        if let Some(item) = self.lower_struct_instance(def_id, args, const_args) {
                            out.push(item);
                        }
                    } else {
                        for field_ty in self.struct_field_tys(def_id) {
                            self.collect_struct_instance_ty(field_ty, seen, out);
                        }
                    }
                }
            }
            Some(TyKind::BuiltinTrait { args, .. }) => {
                for arg in args {
                    self.collect_struct_instance_ty(arg, seen, out);
                }
            }
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
                for arg in trait_args {
                    self.collect_struct_instance_ty(arg, seen, out);
                }
                for arg in trait_const_args {
                    arg.visit_referenced_types(|ty| self.collect_struct_instance_ty(ty, seen, out));
                }
                for binding in associated_type_bindings {
                    for arg in binding.trait_args {
                        self.collect_struct_instance_ty(arg, seen, out);
                    }
                    for arg in binding.trait_const_args {
                        arg.visit_referenced_types(|ty| {
                            self.collect_struct_instance_ty(ty, seen, out)
                        });
                    }
                    self.collect_struct_instance_ty(binding.ty, seen, out);
                }
            }
            Some(TyKind::Projection {
                self_ty,
                trait_args,
                trait_const_args,
                ..
            }) => {
                self.collect_struct_instance_ty(self_ty, seen, out);
                for arg in trait_args {
                    self.collect_struct_instance_ty(arg, seen, out);
                }
                for arg in trait_const_args {
                    arg.visit_referenced_types(|ty| self.collect_struct_instance_ty(ty, seen, out));
                }
            }
            Some(
                TyKind::Error
                | TyKind::ConstOnly
                | TyKind::Opaque
                | TyKind::GenericParam(_)
                | TyKind::SelfParam
                | TyKind::BuiltinType(_)
                | TyKind::Primitive(_)
                | TyKind::Vector { .. }
                | TyKind::ClosureState { .. },
            )
            | None => {}
        }
    }

    fn lower_struct_instance(
        &mut self,
        def_id: GlobalDefId,
        args: Vec<InternedTyId>,
        const_args: Vec<ConstGenericArg>,
    ) -> Option<BackendStructInstance> {
        if self.instance_args_contain_generic_param(&args) {
            return None;
        }
        if def_id.module_id != self.input.module_id {
            return self.lower_foreign_struct_instance(def_id, args, const_args);
        }
        let signature = self.input.signatures.structs.get(&def_id.def_id)?.clone();
        if signature.generics.is_empty()
            || signature.generics.len() != args.len() + const_args.len()
        {
            return None;
        }
        let def = self.input.defs.defs.get(def_id.def_id)?;
        let (substitutions, const_substitutions) =
            self.generic_substitutions_and_consts_for_def(def_id, &args, &const_args);
        let substitution_id =
            self.intern_type_and_const_substitutions(&substitutions, &const_substitutions);
        Some(BackendStructInstance {
            def_id,
            name: def.name,
            args: args.clone(),
            const_args: const_args.clone(),
            symbol: self.mangle_type_instance_symbol(def_id, def.name, &args, &const_args)?,
            fields: signature
                .fields
                .iter()
                .map(|field| BackendField {
                    def_id: GlobalDefId {
                        module_id: self.input.module_id,
                        def_id: field.def_id,
                    },
                    name: field.name,
                    ty: self.instantiate_ty_with_id(field.ty, substitution_id),
                    span: field.span,
                })
                .collect(),
            is_extern: signature.is_extern,
            span: signature.span,
        })
    }

    pub(crate) fn struct_field_tys(&mut self, def_id: GlobalDefId) -> Vec<InternedTyId> {
        if def_id.module_id != self.input.module_id {
            let Some(program_signature) = self.input.program.structs().get(&def_id) else {
                return Vec::new();
            };
            return program_signature
                .signature
                .fields
                .iter()
                .map(|field| self.normalized_type_from_module(def_id.module_id, field.ty))
                .collect();
        }
        let Some(signature) = self.input.signatures.structs.get(&def_id.def_id) else {
            return Vec::new();
        };
        signature.fields.iter().map(|field| field.ty).collect()
    }

    fn lower_foreign_struct_instance(
        &mut self,
        def_id: GlobalDefId,
        args: Vec<InternedTyId>,
        const_args: Vec<ConstGenericArg>,
    ) -> Option<BackendStructInstance> {
        if self.instance_args_contain_generic_param(&args) {
            return None;
        }
        let program_signature = self.input.program.structs().get(&def_id)?.clone();
        let signature = program_signature.signature;
        if signature.generics.is_empty()
            || signature.generics.len() != args.len() + const_args.len()
        {
            return None;
        }
        let symbol_name = self.def_symbol_name(def_id)?;
        let (substitutions, const_substitutions) =
            self.generic_substitutions_and_consts_for_def(def_id, &args, &const_args);
        let substitution_id =
            self.intern_type_and_const_substitutions(&substitutions, &const_substitutions);
        Some(BackendStructInstance {
            def_id,
            name: symbol_name,
            args: args.clone(),
            const_args: const_args.clone(),
            symbol: self.mangle_type_instance_symbol(def_id, symbol_name, &args, &const_args)?,
            fields: signature
                .fields
                .iter()
                .map(|field| {
                    let ty = self.normalized_type_from_module(def_id.module_id, field.ty);
                    BackendField {
                        def_id: GlobalDefId {
                            module_id: def_id.module_id,
                            def_id: field.def_id,
                        },
                        name: field.name,
                        ty: self.instantiate_ty_with_id(ty, substitution_id),
                        span: field.span,
                    }
                })
                .collect(),
            is_extern: signature.is_extern,
            span: signature.span,
        })
    }

    fn collect_union_instance_ty(
        &mut self,
        ty: InternedTyId,
        seen: &mut HashSet<AggregateInstanceKey>,
        out: &mut Vec<BackendUnionInstance>,
    ) {
        match self.ty_kind(ty).cloned() {
            Some(TyKind::Tuple(elems)) => {
                for elem in elems {
                    self.collect_union_instance_ty(elem, seen, out);
                }
            }
            Some(TyKind::Pointer { elem, .. })
            | Some(TyKind::VolatilePointer { elem, .. })
            | Some(TyKind::Slice { elem, .. })
            | Some(TyKind::SlicePointee { elem }) => {
                self.collect_union_instance_ty(elem, seen, out);
            }
            Some(TyKind::Array { len, elem }) => {
                if let nia_ty::ArrayLenTy::Builtin { ty, .. } = len {
                    self.collect_union_instance_ty(ty, seen, out);
                }
                self.collect_union_instance_ty(elem, seen, out);
            }
            Some(TyKind::Range { bound, .. }) => {
                if let Some(bound) = bound {
                    self.collect_union_instance_ty(bound, seen, out);
                }
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
                for param in params {
                    self.collect_union_instance_ty(param, seen, out);
                }
                self.collect_union_instance_ty(return_type, seen, out);
            }
            Some(TyKind::FunctionItem {
                args,
                const_args,
                params,
                return_type,
                self_arg,
                ..
            }) => {
                if let Some(self_arg) = self_arg {
                    self.collect_union_instance_ty(self_arg, seen, out);
                }
                for arg in args {
                    self.collect_union_instance_ty(arg, seen, out);
                }
                for arg in const_args {
                    arg.visit_referenced_types(|ty| self.collect_union_instance_ty(ty, seen, out));
                }
                for param in params {
                    self.collect_union_instance_ty(param, seen, out);
                }
                self.collect_union_instance_ty(return_type, seen, out);
            }
            Some(TyKind::Optional { elem }) => self.collect_union_instance_ty(elem, seen, out),
            Some(TyKind::ErrorUnion { error, value }) => {
                self.collect_union_instance_ty(error, seen, out);
                self.collect_union_instance_ty(value, seen, out);
            }
            Some(TyKind::Nominal {
                def_id,
                args,
                const_args,
            }) => {
                for arg in &args {
                    self.collect_union_instance_ty(*arg, seen, out);
                }
                for arg in &const_args {
                    arg.visit_referenced_types(|ty| self.collect_union_instance_ty(ty, seen, out));
                }
                if seen.insert((def_id, args.clone(), const_args.clone())) {
                    if !args.is_empty() || !const_args.is_empty() {
                        if let Some(item) = self.lower_union_instance(def_id, args, const_args) {
                            out.push(item);
                        }
                    } else {
                        for field_ty in self.union_field_tys(def_id) {
                            self.collect_union_instance_ty(field_ty, seen, out);
                        }
                    }
                }
            }
            Some(TyKind::BuiltinTrait { args, .. }) => {
                for arg in args {
                    self.collect_union_instance_ty(arg, seen, out);
                }
            }
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
                for arg in trait_args {
                    self.collect_union_instance_ty(arg, seen, out);
                }
                for arg in trait_const_args {
                    arg.visit_referenced_types(|ty| self.collect_union_instance_ty(ty, seen, out));
                }
                for binding in associated_type_bindings {
                    for arg in binding.trait_args {
                        self.collect_union_instance_ty(arg, seen, out);
                    }
                    for arg in binding.trait_const_args {
                        arg.visit_referenced_types(|ty| {
                            self.collect_union_instance_ty(ty, seen, out)
                        });
                    }
                    self.collect_union_instance_ty(binding.ty, seen, out);
                }
            }
            Some(TyKind::Projection {
                self_ty,
                trait_args,
                trait_const_args,
                ..
            }) => {
                self.collect_union_instance_ty(self_ty, seen, out);
                for arg in trait_args {
                    self.collect_union_instance_ty(arg, seen, out);
                }
                for arg in trait_const_args {
                    arg.visit_referenced_types(|ty| self.collect_union_instance_ty(ty, seen, out));
                }
            }
            Some(
                TyKind::Error
                | TyKind::ConstOnly
                | TyKind::Opaque
                | TyKind::GenericParam(_)
                | TyKind::SelfParam
                | TyKind::BuiltinType(_)
                | TyKind::Primitive(_)
                | TyKind::Vector { .. }
                | TyKind::ClosureState { .. },
            )
            | None => {}
        }
    }

    fn lower_union_instance(
        &mut self,
        def_id: GlobalDefId,
        args: Vec<InternedTyId>,
        const_args: Vec<ConstGenericArg>,
    ) -> Option<BackendUnionInstance> {
        if self.instance_args_contain_generic_param(&args) {
            return None;
        }
        if def_id.module_id != self.input.module_id {
            return self.lower_foreign_union_instance(def_id, args, const_args);
        }
        let signature = self.input.signatures.unions.get(&def_id.def_id)?.clone();
        if signature.generics.is_empty()
            || signature.generics.len() != args.len() + const_args.len()
        {
            return None;
        }
        let def = self.input.defs.defs.get(def_id.def_id)?;
        let (substitutions, const_substitutions) =
            self.generic_substitutions_and_consts_for_def(def_id, &args, &const_args);
        let substitution_id =
            self.intern_type_and_const_substitutions(&substitutions, &const_substitutions);
        Some(BackendUnionInstance {
            def_id,
            name: def.name,
            args: args.clone(),
            const_args: const_args.clone(),
            symbol: self.mangle_type_instance_symbol(def_id, def.name, &args, &const_args)?,
            fields: signature
                .fields
                .iter()
                .map(|field| BackendField {
                    def_id: GlobalDefId {
                        module_id: self.input.module_id,
                        def_id: field.def_id,
                    },
                    name: field.name,
                    ty: self.instantiate_ty_with_id(field.ty, substitution_id),
                    span: field.span,
                })
                .collect(),
            is_extern: signature.is_extern,
            span: signature.span,
        })
    }

    pub(crate) fn union_field_tys(&mut self, def_id: GlobalDefId) -> Vec<InternedTyId> {
        if def_id.module_id != self.input.module_id {
            let Some(program_signature) = self.input.program.unions().get(&def_id) else {
                return Vec::new();
            };
            return program_signature
                .signature
                .fields
                .iter()
                .map(|field| self.normalized_type_from_module(def_id.module_id, field.ty))
                .collect();
        }
        let Some(signature) = self.input.signatures.unions.get(&def_id.def_id) else {
            return Vec::new();
        };
        signature.fields.iter().map(|field| field.ty).collect()
    }

    fn lower_foreign_union_instance(
        &mut self,
        def_id: GlobalDefId,
        args: Vec<InternedTyId>,
        const_args: Vec<ConstGenericArg>,
    ) -> Option<BackendUnionInstance> {
        if self.instance_args_contain_generic_param(&args) {
            return None;
        }
        let program_signature = self.input.program.unions().get(&def_id)?.clone();
        let signature = program_signature.signature;
        if signature.generics.is_empty()
            || signature.generics.len() != args.len() + const_args.len()
        {
            return None;
        }
        let symbol_name = self.def_symbol_name(def_id)?;
        let (substitutions, const_substitutions) =
            self.generic_substitutions_and_consts_for_def(def_id, &args, &const_args);
        let substitution_id =
            self.intern_type_and_const_substitutions(&substitutions, &const_substitutions);
        Some(BackendUnionInstance {
            def_id,
            name: symbol_name,
            args: args.clone(),
            const_args: const_args.clone(),
            symbol: self.mangle_type_instance_symbol(def_id, symbol_name, &args, &const_args)?,
            fields: signature
                .fields
                .iter()
                .map(|field| {
                    let ty = self.normalized_type_from_module(def_id.module_id, field.ty);
                    BackendField {
                        def_id: GlobalDefId {
                            module_id: def_id.module_id,
                            def_id: field.def_id,
                        },
                        name: field.name,
                        ty: self.instantiate_ty_with_id(ty, substitution_id),
                        span: field.span,
                    }
                })
                .collect(),
            is_extern: signature.is_extern,
            span: signature.span,
        })
    }

    fn instance_args_contain_generic_param(&mut self, args: &[InternedTyId]) -> bool {
        let mut ty_kind = |ty: InternedTyId| self.ty_kind(ty).cloned();
        args.iter()
            .any(|arg| contains_generic_param(*arg, &mut ty_kind, None))
    }
}
