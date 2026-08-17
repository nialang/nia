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
                    self.add(param);
                }
                self.add(return_type);
            }
            Some(TyKind::ErrorUnion { error, value }) => {
                self.add(error);
                self.add(value);
            }
            Some(TyKind::Tuple(elems)) => {
                for elem in elems {
                    self.add(elem);
                }
            }
            Some(TyKind::ClosureState {
                captures,
                params,
                return_type,
                ..
            }) => {
                for ty in captures.into_iter().chain(params).chain([return_type]) {
                    self.add(ty);
                }
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
                self.add_nominal_fields(def_id, &args, &const_args);
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
            | Some(TyKind::Opaque)
            | Some(TyKind::BuiltinType(_))
            | Some(TyKind::Vector { .. })
            | Some(TyKind::Error)
            | Some(TyKind::ConstOnly)
            | Some(TyKind::SelfParam)
            | Some(TyKind::GenericParam(_))
            | None => {}
        }
    }

    fn add_nominal_fields(
        &mut self,
        def_id: GlobalDefId,
        args: &[InternedTyId],
        const_args: &[nia_ty::ConstGenericArg],
    ) {
        if def_id.module_id == self.module_id && !self.expand_local_aggregate_fields {
            return;
        }
        if let Some(program_struct) = self.program_struct
            && let Some(signature) = program_struct(def_id)
        {
            let signature = signature.signature;
            self.add_aggregate_fields(
                &signature.generic_params,
                &signature.fields,
                args,
                const_args,
            );
            return;
        }
        if let Some(program_union) = self.program_union
            && let Some(signature) = program_union(def_id)
        {
            let signature = signature.signature;
            self.add_aggregate_fields(
                &signature.generic_params,
                &signature.fields,
                args,
                const_args,
            );
        }
    }

    fn add_aggregate_fields(
        &mut self,
        generic_params: &[nia_item_signatures::GenericParamSignature],
        fields: &[nia_item_signatures::FieldSignature],
        args: &[InternedTyId],
        const_args: &[nia_ty::ConstGenericArg],
    ) {
        let Some((substitutions, const_substitutions)) =
            nia_item_signatures::generic_argument_substitutions(generic_params, args, const_args)
        else {
            return;
        };
        // Root discovery must traverse the concrete field graph. Reuse the
        // canonical type substituter so const array lengths and nested nominal
        // arguments follow the same rules as layout computation itself.
        for field in fields {
            let field_ty = nia_ty::substitute_ty(
                self.type_store,
                &self.append,
                field.ty,
                &|name| substitutions.get(name).copied(),
                &|name| const_substitutions.get(name).cloned(),
                None,
            );
            self.add(field_ty);
        }
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn aggregate_field_roots_substitute_interleaved_const_arguments() {
        let mut module_ids = nia_ids::ModuleIdAllocator::new();
        let defining_module = module_ids.allocate();
        let consuming_module = module_ids.allocate();
        let packet_id = GlobalDefId {
            module_id: defining_module,
            def_id: nia_defs::DefId(1),
        };
        let type_store = nia_ty::TypeStore::new();
        let defining_types = type_store.append_for_module(defining_module);
        let consuming_types = type_store.append_for_module(consuming_module);
        let type_name = SymbolId::from_stable_hash(nia_symbol::stable_hash("T"));
        let const_name = SymbolId::from_stable_hash(nia_symbol::stable_hash("N"));
        let usize_ty = defining_types.intern(TyKind::Primitive(nia_ty::PrimitiveTy::Usize));
        let generic_ty = defining_types.intern(TyKind::GenericParam(type_name));
        let field_ty = defining_types.intern(TyKind::Array {
            elem: generic_ty,
            len: nia_ty::ArrayLenTy::GenericParam(const_name),
        });
        let signature = ProgramStructSignature {
            signature: nia_item_signatures::StructSignature {
                generics: vec![type_name, const_name],
                generic_params: vec![
                    nia_item_signatures::GenericParamSignature {
                        name: type_name,
                        kind: nia_item_signatures::GenericParamSignatureKind::Type,
                    },
                    nia_item_signatures::GenericParamSignature {
                        name: const_name,
                        kind: nia_item_signatures::GenericParamSignatureKind::Const {
                            ty: usize_ty,
                        },
                    },
                ],
                where_predicates: Vec::new(),
                fields: vec![nia_item_signatures::FieldSignature {
                    def_id: nia_defs::DefId(2),
                    name: SymbolId::from_stable_hash(nia_symbol::stable_hash("values")),
                    ty: field_ty,
                    span: nia_span::Span::default(),
                }],
                is_tuple: false,
                is_extern: false,
                span: nia_span::Span::default(),
            },
        };
        let program_struct = |requested| (requested == packet_id).then(|| signature.clone());
        let program_union = |_| None;
        let u8_ty = consuming_types.intern(TyKind::Primitive(nia_ty::PrimitiveTy::U8));
        let packet_ty = consuming_types.intern(TyKind::Nominal {
            def_id: packet_id,
            args: vec![u8_ty],
            const_args: vec![nia_ty::ConstGenericArg {
                ty: usize_ty,
                value: nia_ty::ConstGenericValue::Int(nia_ty::IntConst::unsigned(4)),
            }],
        });

        let mut roots = LayoutRootCollector::with_program(
            &type_store,
            consuming_module,
            &program_struct,
            &program_union,
        );
        roots.add(packet_ty);
        let roots = roots.finish();

        assert!(roots.types.iter().any(|ty| matches!(
            type_store.get(*ty),
            Some(TyKind::Array {
                elem,
                len: nia_ty::ArrayLenTy::ConstValue(4),
            }) if *elem == u8_ty
        )));
    }
}
