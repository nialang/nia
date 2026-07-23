// SPDX-License-Identifier: GPL-3.0-or-later
use super::*;

pub(super) fn collect_semantic_layout_roots(
    semantic_facts: &nia_sema_ir::SemanticFacts,
    roots: &mut LayoutRootCollector<'_>,
) {
    for ty in semantic_facts.global_types.values().copied() {
        roots.add(ty);
    }
    for facts in semantic_facts.function_facts.values() {
        for ty in facts.local_types.values().copied() {
            roots.add(ty);
        }
    }
    for (_, ty) in semantic_facts.iter_node_expr_types() {
        roots.add(*ty);
    }
    for instantiation in semantic_facts.iter_generic_instantiations() {
        for ty in &instantiation.args {
            roots.add(*ty);
        }
    }
    for (_, coercion) in semantic_facts.iter_node_pointer_array_to_slice_coercions() {
        roots.add(coercion.pointer_ty);
        roots.add(coercion.array_ty);
        roots.add(coercion.slice_ty);
    }
    for (_, coercion) in semantic_facts.iter_node_trait_object_coercions() {
        roots.add(coercion.source_ty);
        roots.add(coercion.target_ty);
        roots.add(coercion.self_ty);
    }
    for (_, upcast) in semantic_facts.iter_node_trait_object_upcasts() {
        roots.add(upcast.source_ty);
        roots.add(upcast.target_ty);
    }
    for (_, value) in semantic_facts.iter_node_builtin_values() {
        collect_builtin_value_layout_roots(value, roots);
    }
}

fn collect_builtin_value_layout_roots(
    value: &nia_sema_ir::BuiltinValue,
    roots: &mut LayoutRootCollector<'_>,
) {
    match value {
        nia_sema_ir::BuiltinValue::Layout { ty, .. }
        | nia_sema_ir::BuiltinValue::FieldOffset { ty, .. } => roots.add(*ty),
        nia_sema_ir::BuiltinValue::Int(_) | nia_sema_ir::BuiltinValue::Usize(_) => {}
    }
}

pub(super) struct LayoutRootCollector<'a> {
    type_store: &'a nia_ty::TypeStore,
    append: nia_ty::TypeStoreAppend,
    module_id: ModuleId,
    program_struct: Option<&'a dyn Fn(GlobalDefId) -> Option<ProgramStructSignature>>,
    program_union: Option<&'a dyn Fn(GlobalDefId) -> Option<ProgramUnionSignature>>,
    expand_local_aggregate_fields: bool,
    seen: HashSet<InternedTyId>,
    types: Vec<InternedTyId>,
    seen_structs: HashSet<nia_defs::DefId>,
    structs: Vec<nia_defs::DefId>,
    seen_global_structs: HashSet<GlobalDefId>,
    global_structs: Vec<GlobalDefId>,
    seen_unions: HashSet<nia_defs::DefId>,
    unions: Vec<nia_defs::DefId>,
    seen_global_unions: HashSet<GlobalDefId>,
    global_unions: Vec<GlobalDefId>,
}

impl<'a> LayoutRootCollector<'a> {
    pub(super) fn new(type_store: &'a nia_ty::TypeStore, module_id: ModuleId) -> Self {
        Self {
            type_store,
            append: type_store.append_for_module(module_id),
            module_id,
            program_struct: None,
            program_union: None,
            expand_local_aggregate_fields: false,
            seen: HashSet::new(),
            types: Vec::new(),
            seen_structs: HashSet::new(),
            structs: Vec::new(),
            seen_global_structs: HashSet::new(),
            global_structs: Vec::new(),
            seen_unions: HashSet::new(),
            unions: Vec::new(),
            seen_global_unions: HashSet::new(),
            global_unions: Vec::new(),
        }
    }

    pub(super) fn with_program(
        type_store: &'a nia_ty::TypeStore,
        module_id: ModuleId,
        program_struct: &'a dyn Fn(GlobalDefId) -> Option<ProgramStructSignature>,
        program_union: &'a dyn Fn(GlobalDefId) -> Option<ProgramUnionSignature>,
    ) -> Self {
        let mut collector = Self::new(type_store, module_id);
        collector.program_struct = Some(program_struct);
        collector.program_union = Some(program_union);
        collector
    }

    pub(super) fn with_program_including_local_aggregates(
        type_store: &'a nia_ty::TypeStore,
        module_id: ModuleId,
        program_struct: &'a dyn Fn(GlobalDefId) -> Option<ProgramStructSignature>,
        program_union: &'a dyn Fn(GlobalDefId) -> Option<ProgramUnionSignature>,
    ) -> Self {
        let mut collector =
            Self::with_program(type_store, module_id, program_struct, program_union);
        collector.expand_local_aggregate_fields = true;
        collector
    }

    pub(super) fn add(&mut self, ty: InternedTyId) {
        if !self.seen.insert(ty) {
            return;
        }
        self.types.push(ty);
        match self.type_store.get(ty).cloned() {
            Some(TyKind::Pointer { elem, .. })
            | Some(TyKind::VolatilePointer { elem, .. })
            | Some(TyKind::Slice { elem, .. })
            | Some(TyKind::SlicePointee { elem })
            | Some(TyKind::Optional { elem }) => self.add(elem),
            Some(TyKind::Array { len, elem }) => {
                self.add_array_len(len);
                self.add(elem);
            }
            Some(TyKind::Range { bound, .. }) => {
                if let Some(bound) = bound {
                    self.add(bound);
                }
            }
            Some(TyKind::FunctionPointer {
                params,
                return_type,
                ..
            }) => {
                for param in params {
                    self.add(param);
                }
                self.add(return_type);
            }
            Some(TyKind::ErrorUnion { error, value }) => {
                self.add(error);
                self.add(value);
            }
            Some(TyKind::Nominal {
                def_id,
                args,
                const_args,
            }) => {
                self.add_global_struct(def_id);
                self.add_global_union(def_id);
                for arg in &args {
                    self.add(*arg);
                }
                for arg in &const_args {
                    self.add(arg.ty);
                }
                self.add_nominal_fields(def_id, &args);
            }
            Some(TyKind::BuiltinTrait { args, .. })
            | Some(TyKind::TraitObject {
                trait_args: args, ..
            })
            | Some(TyKind::TraitObjectPointee {
                trait_args: args, ..
            }) => {
                for arg in args {
                    self.add(arg);
                }
            }
            Some(TyKind::Projection {
                self_ty,
                trait_args,
                ..
            }) => {
                self.add(self_ty);
                for arg in trait_args {
                    self.add(arg);
                }
            }
            Some(TyKind::Primitive(_))
            | Some(TyKind::BuiltinType(_))
            | Some(TyKind::Vector { .. })
            | Some(TyKind::Error)
            | Some(TyKind::ConstOnly)
            | Some(TyKind::SelfParam)
            | Some(TyKind::GenericParam(_))
            | None => {}
        }
    }

    fn add_nominal_fields(&mut self, def_id: GlobalDefId, args: &[InternedTyId]) {
        if def_id.module_id == self.module_id && !self.expand_local_aggregate_fields {
            return;
        }
        if let Some(program_struct) = self.program_struct
            && let Some(signature) = program_struct(def_id)
        {
            let signature = signature.signature;
            self.add_aggregate_fields(&signature.generics, &signature.fields, args);
            return;
        }
        if let Some(program_union) = self.program_union
            && let Some(signature) = program_union(def_id)
        {
            let signature = signature.signature;
            self.add_aggregate_fields(&signature.generics, &signature.fields, args);
        }
    }

    fn add_aggregate_fields(
        &mut self,
        generics: &[SymbolId],
        fields: &[nia_item_signatures::FieldSignature],
        args: &[InternedTyId],
    ) {
        if generics.len() != args.len() {
            return;
        }
        let substitutions = generics
            .iter()
            .cloned()
            .zip(args.iter().copied())
            .collect::<SymbolMap<_>>();
        for field in fields {
            let field_ty = self.substitute_generics(field.ty, &substitutions);
            self.add(field_ty);
        }
    }

    fn substitute_generics(
        &mut self,
        ty: InternedTyId,
        substitutions: &SymbolMap<InternedTyId>,
    ) -> InternedTyId {
        match self.type_store.get(ty).cloned() {
            Some(TyKind::GenericParam(name)) => substitutions.get(&name).copied().unwrap_or(ty),
            Some(TyKind::Pointer { is_readonly, elem }) => {
                let elem = self.substitute_generics(elem, substitutions);
                self.intern(TyKind::Pointer { is_readonly, elem })
            }
            Some(TyKind::VolatilePointer { is_readonly, elem }) => {
                let elem = self.substitute_generics(elem, substitutions);
                self.intern(TyKind::VolatilePointer { is_readonly, elem })
            }
            Some(TyKind::Slice { is_readonly, elem }) => {
                let elem = self.substitute_generics(elem, substitutions);
                self.intern(TyKind::Slice { is_readonly, elem })
            }
            Some(TyKind::SlicePointee { elem }) => {
                let elem = self.substitute_generics(elem, substitutions);
                self.intern(TyKind::SlicePointee { elem })
            }
            Some(TyKind::Array { len, elem }) => {
                let len = self.substitute_array_len_generics(len, substitutions);
                let elem = self.substitute_generics(elem, substitutions);
                self.intern(TyKind::Array { len, elem })
            }
            Some(TyKind::Range { kind, bound }) => {
                let bound = bound.map(|bound| self.substitute_generics(bound, substitutions));
                self.intern(TyKind::Range { kind, bound })
            }
            Some(TyKind::FunctionPointer {
                params,
                return_type,
                is_variadic,
            }) => {
                let params = params
                    .into_iter()
                    .map(|param| self.substitute_generics(param, substitutions))
                    .collect();
                let return_type = self.substitute_generics(return_type, substitutions);
                self.intern(TyKind::FunctionPointer {
                    params,
                    return_type,
                    is_variadic,
                })
            }
            Some(TyKind::Optional { elem }) => {
                let elem = self.substitute_generics(elem, substitutions);
                self.intern(TyKind::Optional { elem })
            }
            Some(TyKind::ErrorUnion { error, value }) => {
                let error = self.substitute_generics(error, substitutions);
                let value = self.substitute_generics(value, substitutions);
                self.intern(TyKind::ErrorUnion { error, value })
            }
            Some(TyKind::Nominal {
                def_id,
                args,
                const_args,
            }) => {
                let args = args
                    .into_iter()
                    .map(|arg| self.substitute_generics(arg, substitutions))
                    .collect();
                let const_args = const_args
                    .into_iter()
                    .map(|mut arg| {
                        arg.ty = self.substitute_generics(arg.ty, substitutions);
                        arg
                    })
                    .collect();
                self.intern(TyKind::Nominal {
                    def_id,
                    args,
                    const_args,
                })
            }
            Some(TyKind::BuiltinTrait { trait_id, args }) => {
                let args = args
                    .into_iter()
                    .map(|arg| self.substitute_generics(arg, substitutions))
                    .collect();
                self.intern(TyKind::BuiltinTrait { trait_id, args })
            }
            Some(TyKind::BuiltinType(_)) => ty,
            Some(TyKind::Projection {
                self_ty,
                trait_id,
                trait_args,
                trait_const_args,
                name,
            }) => {
                let self_ty = self.substitute_generics(self_ty, substitutions);
                let trait_args = trait_args
                    .into_iter()
                    .map(|arg| self.substitute_generics(arg, substitutions))
                    .collect();
                let trait_const_args = trait_const_args
                    .into_iter()
                    .map(|mut arg| {
                        arg.ty = self.substitute_generics(arg.ty, substitutions);
                        arg
                    })
                    .collect();
                self.intern(TyKind::Projection {
                    self_ty,
                    trait_id,
                    trait_args,
                    trait_const_args,
                    name,
                })
            }
            Some(TyKind::TraitObject {
                is_readonly,
                trait_id,
                trait_args,
                trait_const_args,
                associated_type_bindings,
            }) => {
                let trait_args = trait_args
                    .into_iter()
                    .map(|arg| self.substitute_generics(arg, substitutions))
                    .collect();
                let trait_const_args = trait_const_args
                    .into_iter()
                    .map(|mut arg| {
                        arg.ty = self.substitute_generics(arg.ty, substitutions);
                        arg
                    })
                    .collect();
                let associated_type_bindings = associated_type_bindings
                    .into_iter()
                    .map(|binding| nia_ty::AssociatedTypeBindingTy {
                        trait_id: binding.trait_id,
                        trait_args: binding
                            .trait_args
                            .into_iter()
                            .map(|arg| self.substitute_generics(arg, substitutions))
                            .collect(),
                        trait_const_args: binding
                            .trait_const_args
                            .into_iter()
                            .map(|mut arg| {
                                arg.ty = self.substitute_generics(arg.ty, substitutions);
                                arg
                            })
                            .collect(),
                        name: binding.name,
                        ty: self.substitute_generics(binding.ty, substitutions),
                    })
                    .collect();
                self.intern(TyKind::TraitObject {
                    is_readonly,
                    trait_id,
                    trait_args,
                    trait_const_args,
                    associated_type_bindings,
                })
            }
            Some(TyKind::TraitObjectPointee {
                trait_id,
                trait_args,
                trait_const_args,
                associated_type_bindings,
            }) => {
                let trait_args = trait_args
                    .into_iter()
                    .map(|arg| self.substitute_generics(arg, substitutions))
                    .collect();
                let trait_const_args = trait_const_args
                    .into_iter()
                    .map(|mut arg| {
                        arg.ty = self.substitute_generics(arg.ty, substitutions);
                        arg
                    })
                    .collect();
                let associated_type_bindings = associated_type_bindings
                    .into_iter()
                    .map(|binding| nia_ty::AssociatedTypeBindingTy {
                        trait_id: binding.trait_id,
                        trait_args: binding
                            .trait_args
                            .into_iter()
                            .map(|arg| self.substitute_generics(arg, substitutions))
                            .collect(),
                        trait_const_args: binding
                            .trait_const_args
                            .into_iter()
                            .map(|mut arg| {
                                arg.ty = self.substitute_generics(arg.ty, substitutions);
                                arg
                            })
                            .collect(),
                        name: binding.name,
                        ty: self.substitute_generics(binding.ty, substitutions),
                    })
                    .collect();
                self.intern(TyKind::TraitObjectPointee {
                    trait_id,
                    trait_args,
                    trait_const_args,
                    associated_type_bindings,
                })
            }
            Some(TyKind::Primitive(_))
            | Some(TyKind::Vector { .. })
            | Some(TyKind::Error)
            | Some(TyKind::ConstOnly)
            | Some(TyKind::SelfParam)
            | None => ty,
        }
    }

    fn substitute_array_len_generics(
        &mut self,
        len: nia_ty::ArrayLenTy,
        substitutions: &SymbolMap<InternedTyId>,
    ) -> nia_ty::ArrayLenTy {
        match len {
            nia_ty::ArrayLenTy::Builtin { builtin, ty } => nia_ty::ArrayLenTy::Builtin {
                builtin,
                ty: self.substitute_generics(ty, substitutions),
            },
            nia_ty::ArrayLenTy::Infer
            | nia_ty::ArrayLenTy::GenericParam(_)
            | nia_ty::ArrayLenTy::ConstValue(_)
            | nia_ty::ArrayLenTy::ConstExpr(_) => len,
        }
    }

    fn intern(&mut self, kind: TyKind) -> InternedTyId {
        self.append.intern(kind)
    }

    pub(super) fn add_struct(&mut self, def_id: nia_defs::DefId) {
        if self.seen_structs.insert(def_id) {
            self.structs.push(def_id);
        }
    }

    fn add_global_struct(&mut self, def_id: GlobalDefId) {
        if def_id.module_id == self.module_id {
            self.add_struct(def_id.def_id);
        }
        if self.seen_global_structs.insert(def_id) {
            self.global_structs.push(def_id);
        }
    }

    pub(super) fn add_union(&mut self, def_id: nia_defs::DefId) {
        if self.seen_unions.insert(def_id) {
            self.unions.push(def_id);
        }
    }

    fn add_global_union(&mut self, def_id: GlobalDefId) {
        if def_id.module_id == self.module_id {
            self.add_union(def_id.def_id);
        }
        if self.seen_global_unions.insert(def_id) {
            self.global_unions.push(def_id);
        }
    }

    fn add_array_len(&mut self, len: nia_ty::ArrayLenTy) {
        if let nia_ty::ArrayLenTy::Builtin { ty, .. } = len {
            self.add(ty);
        }
    }

    pub(super) fn finish(self) -> CollectedLayoutRoots {
        CollectedLayoutRoots {
            types: self.types,
            structs: self.structs,
            unions: self.unions,
        }
    }

    pub(super) fn finish_global(self) -> CollectedGlobalLayoutRoots {
        CollectedGlobalLayoutRoots {
            structs: self.global_structs,
            unions: self.global_unions,
        }
    }
}

pub(super) struct CollectedLayoutRoots {
    pub(super) types: Vec<InternedTyId>,
    pub(super) structs: Vec<nia_defs::DefId>,
    pub(super) unions: Vec<nia_defs::DefId>,
}

pub(super) struct CollectedGlobalLayoutRoots {
    pub(super) structs: Vec<GlobalDefId>,
    pub(super) unions: Vec<GlobalDefId>,
}
