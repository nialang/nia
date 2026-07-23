// SPDX-License-Identifier: GPL-3.0-or-later
use std::{collections::HashMap, sync::Arc};

use nia_backend_ir::{
    BackendEnum, BackendEnumVariant, BackendFunctionInstance, BackendProgram,
    BackendStructInstanceKey, BackendTraitObjectVtable,
};
use nia_backend_lower::BackendLowering;
use nia_ids::{GlobalDefId, InternedTyId, ModuleId};
use nia_layout::{StructLayout, TypeLayout};
use nia_ty::{ConstGenericArg, TraitId, TyKind, TypeStore};

type InstanceArgs = (Vec<InternedTyId>, Vec<ConstGenericArg>);
type AggregateInstanceIndex = HashMap<GlobalDefId, HashMap<InstanceArgs, ItemPosition>>;
type GlobalInstanceIndex = HashMap<(GlobalDefId, ModuleId), HashMap<InstanceArgs, ItemPosition>>;
type FunctionInstanceArgs = (
    Option<InternedTyId>,
    Vec<InternedTyId>,
    Vec<ConstGenericArg>,
);
type FunctionInstanceIndex =
    HashMap<(GlobalDefId, ModuleId), HashMap<FunctionInstanceArgs, ItemPosition>>;
type AggregateLayoutIndex = HashMap<GlobalDefId, HashMap<InstanceArgs, LayoutPosition>>;

#[derive(Clone, Copy)]
struct ItemPosition {
    module: usize,
    item: usize,
}

#[derive(Clone, Copy)]
struct LayoutPosition {
    module: usize,
    layout: usize,
}

#[derive(Clone, Copy)]
struct EnumVariantPosition {
    module: usize,
    owner: usize,
    variant: usize,
}

pub(super) struct ProgramIndex {
    lowering: Arc<BackendLowering>,
    type_store: Arc<TypeStore>,
    modules: HashMap<ModuleId, usize>,
    structs: HashMap<GlobalDefId, ItemPosition>,
    unions: HashMap<GlobalDefId, ItemPosition>,
    struct_instances: AggregateInstanceIndex,
    union_instances: AggregateInstanceIndex,
    enums: HashMap<GlobalDefId, ItemPosition>,
    enum_variants: HashMap<GlobalDefId, EnumVariantPosition>,
    globals: HashMap<GlobalDefId, ItemPosition>,
    global_instances: GlobalInstanceIndex,
    global_instances_by_def: HashMap<GlobalDefId, Vec<ItemPosition>>,
    functions: HashMap<GlobalDefId, ItemPosition>,
    function_instances: FunctionInstanceIndex,
    function_instances_by_def: HashMap<GlobalDefId, Vec<ItemPosition>>,
    trait_object_vtables_by_object_ty: HashMap<InternedTyId, Vec<ItemPosition>>,
    trait_object_vtables_by_trait: HashMap<TraitId, Vec<ItemPosition>>,
    type_layouts: HashMap<InternedTyId, LayoutPosition>,
    struct_layouts: HashMap<GlobalDefId, LayoutPosition>,
    union_layouts: HashMap<GlobalDefId, LayoutPosition>,
    struct_instance_layouts: AggregateLayoutIndex,
    union_instance_layouts: AggregateLayoutIndex,
    struct_instance_layouts_by_def: HashMap<GlobalDefId, Vec<LayoutPosition>>,
    union_instance_layouts_by_def: HashMap<GlobalDefId, Vec<LayoutPosition>>,
    struct_instances_by_def: HashMap<GlobalDefId, Vec<ItemPosition>>,
    union_instances_by_def: HashMap<GlobalDefId, Vec<ItemPosition>>,
}

pub(super) struct BackendLayoutInstance<'a> {
    pub(super) key: &'a BackendStructInstanceKey,
    pub(super) layout: &'a StructLayout,
}

pub(super) struct EnumVariantInfo<'a> {
    pub(super) owner: &'a BackendEnum,
    pub(super) variant: &'a BackendEnumVariant,
    pub(super) index: usize,
}

impl ProgramIndex {
    pub(super) fn new(lowering: Arc<BackendLowering>, type_store: Arc<TypeStore>) -> Self {
        let mut index = Self {
            lowering,
            type_store,
            modules: HashMap::new(),
            structs: HashMap::new(),
            unions: HashMap::new(),
            struct_instances: HashMap::new(),
            union_instances: HashMap::new(),
            enums: HashMap::new(),
            enum_variants: HashMap::new(),
            globals: HashMap::new(),
            global_instances: HashMap::new(),
            global_instances_by_def: HashMap::new(),
            functions: HashMap::new(),
            function_instances: HashMap::new(),
            function_instances_by_def: HashMap::new(),
            trait_object_vtables_by_object_ty: HashMap::new(),
            trait_object_vtables_by_trait: HashMap::new(),
            type_layouts: HashMap::new(),
            struct_layouts: HashMap::new(),
            union_layouts: HashMap::new(),
            struct_instance_layouts: HashMap::new(),
            union_instance_layouts: HashMap::new(),
            struct_instance_layouts_by_def: HashMap::new(),
            union_instance_layouts_by_def: HashMap::new(),
            struct_instances_by_def: HashMap::new(),
            union_instances_by_def: HashMap::new(),
        };
        for (module_index, module) in index.lowering.program.modules.iter().enumerate() {
            index.modules.insert(module.id, module_index);
            for (layout_index, (ty, _)) in module.layouts.types.iter().enumerate() {
                index.type_layouts.insert(
                    *ty,
                    LayoutPosition {
                        module: module_index,
                        layout: layout_index,
                    },
                );
            }
            for (layout_index, (def_id, _)) in module.layouts.structs.iter().enumerate() {
                index.struct_layouts.insert(
                    *def_id,
                    LayoutPosition {
                        module: module_index,
                        layout: layout_index,
                    },
                );
            }
            for (layout_index, (def_id, _)) in module.layouts.unions.iter().enumerate() {
                index.union_layouts.insert(
                    *def_id,
                    LayoutPosition {
                        module: module_index,
                        layout: layout_index,
                    },
                );
            }
            for (layout_index, (key, _)) in module.layouts.struct_instances.iter().enumerate() {
                let position = LayoutPosition {
                    module: module_index,
                    layout: layout_index,
                };
                index
                    .struct_instance_layouts
                    .entry(key.def_id)
                    .or_default()
                    .insert((key.args.clone(), key.const_args.clone()), position);
                index
                    .struct_instance_layouts_by_def
                    .entry(key.def_id)
                    .or_default()
                    .push(position);
            }
            for (layout_index, (key, _)) in module.layouts.union_instances.iter().enumerate() {
                let position = LayoutPosition {
                    module: module_index,
                    layout: layout_index,
                };
                index
                    .union_instance_layouts
                    .entry(key.def_id)
                    .or_default()
                    .insert((key.args.clone(), key.const_args.clone()), position);
                index
                    .union_instance_layouts_by_def
                    .entry(key.def_id)
                    .or_default()
                    .push(position);
            }
            for (item_index, item) in module.structs.iter().enumerate() {
                index.structs.insert(
                    item.def_id,
                    ItemPosition {
                        module: module_index,
                        item: item_index,
                    },
                );
            }
            for (item_index, item) in module.unions.iter().enumerate() {
                index.unions.insert(
                    item.def_id,
                    ItemPosition {
                        module: module_index,
                        item: item_index,
                    },
                );
            }
            for (item_index, item) in module.struct_instances.iter().enumerate() {
                let position = ItemPosition {
                    module: module_index,
                    item: item_index,
                };
                index
                    .struct_instances
                    .entry(item.def_id)
                    .or_default()
                    .insert((item.args.clone(), item.const_args.clone()), position);
                index
                    .struct_instances_by_def
                    .entry(item.def_id)
                    .or_default()
                    .push(position);
            }
            for (item_index, item) in module.union_instances.iter().enumerate() {
                let position = ItemPosition {
                    module: module_index,
                    item: item_index,
                };
                index
                    .union_instances
                    .entry(item.def_id)
                    .or_default()
                    .insert((item.args.clone(), item.const_args.clone()), position);
                index
                    .union_instances_by_def
                    .entry(item.def_id)
                    .or_default()
                    .push(position);
            }
            for (item_index, item) in module.enums.iter().enumerate() {
                index.enums.insert(
                    item.def_id,
                    ItemPosition {
                        module: module_index,
                        item: item_index,
                    },
                );
                for (variant_index, variant) in item.variants.iter().enumerate() {
                    index.enum_variants.insert(
                        variant.def_id,
                        EnumVariantPosition {
                            module: module_index,
                            owner: item_index,
                            variant: variant_index,
                        },
                    );
                }
            }
            for (item_index, item) in module.globals.iter().enumerate() {
                index.globals.insert(
                    item.def_id,
                    ItemPosition {
                        module: module_index,
                        item: item_index,
                    },
                );
            }
            for (item_index, item) in module.global_instances.iter().enumerate() {
                let position = ItemPosition {
                    module: module_index,
                    item: item_index,
                };
                index
                    .global_instances
                    .entry((item.def_id, item.arg_module_id))
                    .or_default()
                    .insert((item.args.clone(), item.const_args.clone()), position);
                index
                    .global_instances_by_def
                    .entry(item.def_id)
                    .or_default()
                    .push(position);
            }
            for (item_index, item) in module.functions.iter().enumerate() {
                index.functions.insert(
                    item.def_id,
                    ItemPosition {
                        module: module_index,
                        item: item_index,
                    },
                );
            }
            for (item_index, item) in module.function_instances.iter().enumerate() {
                let position = ItemPosition {
                    module: module_index,
                    item: item_index,
                };
                index
                    .function_instances
                    .entry((item.def_id, item.arg_module_id))
                    .or_default()
                    .insert(
                        (item.self_arg, item.args.clone(), item.const_args.clone()),
                        position,
                    );
                index
                    .function_instances_by_def
                    .entry(item.def_id)
                    .or_default()
                    .push(position);
            }
            for (item_index, vtable) in module.trait_object_vtables.iter().enumerate() {
                let position = ItemPosition {
                    module: module_index,
                    item: item_index,
                };
                index
                    .trait_object_vtables_by_object_ty
                    .entry(vtable.key.object_ty)
                    .or_default()
                    .push(position);
                if let Some(TyKind::TraitObject { trait_id, .. }) =
                    index.ty_kind(vtable.key.object_ty)
                {
                    index
                        .trait_object_vtables_by_trait
                        .entry(*trait_id)
                        .or_default()
                        .push(position);
                }
            }
        }
        index
    }

    pub(super) fn program(&self) -> &BackendProgram {
        &self.lowering.program
    }

    pub(super) fn modules(&self) -> impl Iterator<Item = &nia_backend_ir::BackendModule> {
        self.modules
            .values()
            .map(|module| &self.lowering.program.modules[*module])
    }

    pub(super) fn module(&self, module_id: ModuleId) -> Option<&nia_backend_ir::BackendModule> {
        self.modules
            .get(&module_id)
            .map(|module| &self.lowering.program.modules[*module])
    }

    pub(super) fn type_store(&self) -> &TypeStore {
        &self.type_store
    }

    pub(super) fn ty_kind(&self, ty: InternedTyId) -> Option<&TyKind> {
        self.type_store.get(ty)
    }

    pub(super) fn type_layout(&self, ty: InternedTyId) -> Option<&TypeLayout> {
        self.type_layouts.get(&ty).map(|position| {
            &self.lowering.program.modules[position.module].layouts.types[position.layout].1
        })
    }

    pub(super) fn struct_layout(&self, def_id: GlobalDefId) -> Option<&StructLayout> {
        self.struct_layouts.get(&def_id).map(|position| {
            &self.lowering.program.modules[position.module]
                .layouts
                .structs[position.layout]
                .1
        })
    }

    pub(super) fn union_layout(&self, def_id: GlobalDefId) -> Option<&StructLayout> {
        self.union_layouts.get(&def_id).map(|position| {
            &self.lowering.program.modules[position.module]
                .layouts
                .unions[position.layout]
                .1
        })
    }

    pub(super) fn struct_instance_layout(
        &self,
        def_id: GlobalDefId,
        args: &[InternedTyId],
        const_args: &[ConstGenericArg],
    ) -> Option<&StructLayout> {
        self.struct_instance_layouts
            .get(&def_id)
            .and_then(|layouts| layouts.get(&(args.to_vec(), const_args.to_vec())))
            .map(|position| {
                &self.lowering.program.modules[position.module]
                    .layouts
                    .struct_instances[position.layout]
                    .1
            })
    }

    pub(super) fn union_instance_layout(
        &self,
        def_id: GlobalDefId,
        args: &[InternedTyId],
        const_args: &[ConstGenericArg],
    ) -> Option<&StructLayout> {
        self.union_instance_layouts
            .get(&def_id)
            .and_then(|layouts| layouts.get(&(args.to_vec(), const_args.to_vec())))
            .map(|position| {
                &self.lowering.program.modules[position.module]
                    .layouts
                    .union_instances[position.layout]
                    .1
            })
    }

    pub(super) fn struct_instance(
        &self,
        def_id: GlobalDefId,
        args: &[InternedTyId],
        const_args: &[ConstGenericArg],
    ) -> Option<&nia_backend_ir::BackendStructInstance> {
        self.struct_instances
            .get(&def_id)
            .and_then(|instances| instances.get(&(args.to_vec(), const_args.to_vec())))
            .map(|position| {
                &self.lowering.program.modules[position.module].struct_instances[position.item]
            })
    }

    pub(super) fn union_instance(
        &self,
        def_id: GlobalDefId,
        args: &[InternedTyId],
        const_args: &[ConstGenericArg],
    ) -> Option<&nia_backend_ir::BackendUnionInstance> {
        self.union_instances
            .get(&def_id)
            .and_then(|instances| instances.get(&(args.to_vec(), const_args.to_vec())))
            .map(|position| {
                &self.lowering.program.modules[position.module].union_instances[position.item]
            })
    }

    pub(super) fn function_instance(
        &self,
        def_id: GlobalDefId,
        arg_module_id: ModuleId,
        self_arg: Option<InternedTyId>,
        args: &[InternedTyId],
        const_args: &[ConstGenericArg],
    ) -> Option<&BackendFunctionInstance> {
        self.function_instances
            .get(&(def_id, arg_module_id))
            .and_then(|instances| instances.get(&(self_arg, args.to_vec(), const_args.to_vec())))
            .map(|position| {
                &self.lowering.program.modules[position.module].function_instances[position.item]
            })
    }

    pub(super) fn structs(&self) -> impl Iterator<Item = &nia_backend_ir::BackendStruct> {
        self.structs
            .values()
            .map(|position| &self.lowering.program.modules[position.module].structs[position.item])
    }

    pub(super) fn struct_item(
        &self,
        def_id: GlobalDefId,
    ) -> Option<&nia_backend_ir::BackendStruct> {
        self.structs
            .get(&def_id)
            .map(|position| &self.lowering.program.modules[position.module].structs[position.item])
    }

    pub(super) fn has_struct(&self, def_id: GlobalDefId) -> bool {
        self.structs.contains_key(&def_id)
    }

    pub(super) fn unions(&self) -> impl Iterator<Item = &nia_backend_ir::BackendUnion> {
        self.unions
            .values()
            .map(|position| &self.lowering.program.modules[position.module].unions[position.item])
    }

    pub(super) fn union_item(&self, def_id: GlobalDefId) -> Option<&nia_backend_ir::BackendUnion> {
        self.unions
            .get(&def_id)
            .map(|position| &self.lowering.program.modules[position.module].unions[position.item])
    }

    pub(super) fn has_union(&self, def_id: GlobalDefId) -> bool {
        self.unions.contains_key(&def_id)
    }

    pub(super) fn struct_instances(
        &self,
    ) -> impl Iterator<Item = &nia_backend_ir::BackendStructInstance> {
        self.struct_instances_by_def
            .values()
            .flatten()
            .map(|position| {
                &self.lowering.program.modules[position.module].struct_instances[position.item]
            })
    }

    pub(super) fn union_instances(
        &self,
    ) -> impl Iterator<Item = &nia_backend_ir::BackendUnionInstance> {
        self.union_instances_by_def
            .values()
            .flatten()
            .map(|position| {
                &self.lowering.program.modules[position.module].union_instances[position.item]
            })
    }

    pub(super) fn has_struct_instances(&self, def_id: GlobalDefId) -> bool {
        self.struct_instances_by_def.contains_key(&def_id)
    }

    pub(super) fn struct_instances_for(
        &self,
        def_id: GlobalDefId,
    ) -> impl Iterator<Item = &nia_backend_ir::BackendStructInstance> {
        self.struct_instances_by_def
            .get(&def_id)
            .into_iter()
            .flatten()
            .map(|position| {
                &self.lowering.program.modules[position.module].struct_instances[position.item]
            })
    }

    pub(super) fn has_union_instances(&self, def_id: GlobalDefId) -> bool {
        self.union_instances_by_def.contains_key(&def_id)
    }

    pub(super) fn union_instances_for(
        &self,
        def_id: GlobalDefId,
    ) -> impl Iterator<Item = &nia_backend_ir::BackendUnionInstance> {
        self.union_instances_by_def
            .get(&def_id)
            .into_iter()
            .flatten()
            .map(|position| {
                &self.lowering.program.modules[position.module].union_instances[position.item]
            })
    }

    pub(super) fn has_enum(&self, def_id: GlobalDefId) -> bool {
        self.enums.contains_key(&def_id)
    }

    pub(super) fn enum_item(&self, def_id: GlobalDefId) -> Option<&BackendEnum> {
        self.enums
            .get(&def_id)
            .map(|position| &self.lowering.program.modules[position.module].enums[position.item])
    }

    pub(super) fn enum_variant_info(&self, def_id: GlobalDefId) -> Option<EnumVariantInfo<'_>> {
        self.enum_variants.get(&def_id).map(|position| {
            let owner = &self.lowering.program.modules[position.module].enums[position.owner];
            EnumVariantInfo {
                owner,
                variant: &owner.variants[position.variant],
                index: position.variant,
            }
        })
    }

    pub(super) fn has_enum_variant(&self, def_id: GlobalDefId) -> bool {
        self.enum_variants.contains_key(&def_id)
    }

    pub(super) fn globals(&self) -> impl Iterator<Item = &nia_backend_ir::BackendGlobal> {
        self.globals
            .values()
            .map(|position| &self.lowering.program.modules[position.module].globals[position.item])
    }

    pub(super) fn global(&self, def_id: GlobalDefId) -> Option<&nia_backend_ir::BackendGlobal> {
        self.globals
            .get(&def_id)
            .map(|position| &self.lowering.program.modules[position.module].globals[position.item])
    }

    pub(super) fn has_global(&self, def_id: GlobalDefId) -> bool {
        self.globals.contains_key(&def_id)
    }

    pub(super) fn global_instances(
        &self,
    ) -> impl Iterator<Item = &nia_backend_ir::BackendGlobalInstance> {
        self.global_instances_by_def
            .values()
            .flatten()
            .map(|position| {
                &self.lowering.program.modules[position.module].global_instances[position.item]
            })
    }

    pub(super) fn global_instance(
        &self,
        def_id: GlobalDefId,
        arg_module_id: ModuleId,
        args: &[InternedTyId],
        const_args: &[ConstGenericArg],
    ) -> Option<&nia_backend_ir::BackendGlobalInstance> {
        self.global_instances
            .get(&(def_id, arg_module_id))
            .and_then(|instances| instances.get(&(args.to_vec(), const_args.to_vec())))
            .map(|position| {
                &self.lowering.program.modules[position.module].global_instances[position.item]
            })
    }

    pub(super) fn functions(&self) -> impl Iterator<Item = &nia_backend_ir::BackendFunction> {
        self.functions.values().map(|position| {
            &self.lowering.program.modules[position.module].functions[position.item]
        })
    }

    pub(super) fn function(&self, def_id: GlobalDefId) -> Option<&nia_backend_ir::BackendFunction> {
        self.functions.get(&def_id).map(|position| {
            &self.lowering.program.modules[position.module].functions[position.item]
        })
    }

    pub(super) fn has_function(&self, def_id: GlobalDefId) -> bool {
        self.functions.contains_key(&def_id)
    }

    pub(super) fn function_instances(&self) -> impl Iterator<Item = &BackendFunctionInstance> {
        self.function_instances_by_def
            .values()
            .flatten()
            .map(|position| {
                &self.lowering.program.modules[position.module].function_instances[position.item]
            })
    }

    pub(super) fn function_instances_for(
        &self,
        def_id: GlobalDefId,
    ) -> impl Iterator<Item = &BackendFunctionInstance> {
        self.function_instances_by_def
            .get(&def_id)
            .into_iter()
            .flatten()
            .map(|position| {
                &self.lowering.program.modules[position.module].function_instances[position.item]
            })
    }

    pub(super) fn function_instance_count(&self, def_id: GlobalDefId) -> usize {
        self.function_instances_by_def
            .get(&def_id)
            .map_or(0, Vec::len)
    }

    pub(super) fn struct_instance_layouts(
        &self,
        def_id: GlobalDefId,
    ) -> impl Iterator<Item = BackendLayoutInstance<'_>> {
        self.struct_instance_layouts_by_def
            .get(&def_id)
            .into_iter()
            .flatten()
            .map(|position| {
                let (key, layout) = &self.lowering.program.modules[position.module]
                    .layouts
                    .struct_instances[position.layout];
                BackendLayoutInstance { key, layout }
            })
    }

    pub(super) fn union_instance_layouts(
        &self,
        def_id: GlobalDefId,
    ) -> impl Iterator<Item = BackendLayoutInstance<'_>> {
        self.union_instance_layouts_by_def
            .get(&def_id)
            .into_iter()
            .flatten()
            .map(|position| {
                let (key, layout) = &self.lowering.program.modules[position.module]
                    .layouts
                    .union_instances[position.layout];
                BackendLayoutInstance { key, layout }
            })
    }

    pub(super) fn trait_object_vtables_for_object_ty(
        &self,
        object_ty: InternedTyId,
    ) -> impl Iterator<Item = &BackendTraitObjectVtable> {
        self.trait_object_vtables_by_object_ty
            .get(&object_ty)
            .into_iter()
            .flatten()
            .map(|position| {
                &self.lowering.program.modules[position.module].trait_object_vtables[position.item]
            })
    }

    pub(super) fn trait_object_vtables_for_trait(
        &self,
        trait_id: TraitId,
    ) -> impl Iterator<Item = &BackendTraitObjectVtable> {
        self.trait_object_vtables_by_trait
            .get(&trait_id)
            .into_iter()
            .flatten()
            .map(|position| {
                &self.lowering.program.modules[position.module].trait_object_vtables[position.item]
            })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use nia_backend_ir::{
        BackendConstFacts, BackendFunctionInstance, BackendGenericInstantiation, BackendLayouts,
        BackendModule, BackendParam, BackendProgram, BackendStructInstance,
        BackendTraitObjectVtable, BackendTraitObjectVtableKey,
    };
    use nia_ids::{DefId, ModuleId, ModuleIdAllocator};
    use nia_layout::{StructLayout, TypeLayout};
    use nia_span::Span;
    use nia_symbol::{SymbolId, stable_hash};
    use nia_ty::{PrimitiveTy, TyKind, TypeStore};

    fn sym(text: &str) -> SymbolId {
        SymbolId::from_stable_hash(stable_hash(text))
    }

    fn lowering(program: BackendProgram) -> Arc<BackendLowering> {
        let codegen_partitions = program.codegen_partition_plan();
        Arc::new(BackendLowering {
            program,
            codegen_partitions,
            optimization: nia_opt::OptimizationPolicy::default(),
            optimization_report: nia_backend_lower::BackendOptimizationReport::default(),
            diagnostics: Vec::new(),
        })
    }

    #[test]
    fn indexes_codegen_lookup_tables_by_exact_keys_and_fallback_groups() {
        let mut module_ids = ModuleIdAllocator::new();
        let module_id = module_ids.allocate();
        let type_store = TypeStore::new();
        let interner = type_store.append_for_module(module_id);
        let i32_ty = interner.primitive(PrimitiveTy::I32);
        let function_def = global(module_id, 1);
        let struct_def = global(module_id, 2);
        let enum_def = global(module_id, 3);
        let first_variant = global(module_id, 4);
        let second_variant = global(module_id, 5);
        let trait_def = global(module_id, 6);
        let object_ty = interner.intern(TyKind::TraitObject {
            is_readonly: true,
            trait_id: TraitId::Source(trait_def),
            trait_args: vec![i32_ty],
            trait_const_args: Vec::new(),
            associated_type_bindings: Vec::new(),
        });
        let struct_key = BackendStructInstanceKey {
            def_id: struct_def,
            args: vec![i32_ty],
            const_args: Vec::new(),
        };
        let struct_layout = StructLayout {
            layout: TypeLayout { size: 4, align: 4 },
            fields: Vec::new(),
        };
        let function_instance = BackendFunctionInstance {
            def_id: function_def,
            name: sym("id"),
            arg_module_id: module_id,
            self_arg: None,
            args: vec![i32_ty],
            const_args: Vec::new(),
            symbol: "id_i32".to_string(),
            params: vec![BackendParam {
                local_id: None,
                name: Some(sym("value")),
                receiver: None,
                passing_ty: i32_ty,
                local_ty: i32_ty,
                span: Span::default(),
            }],
            return_type: i32_ty,
            is_extern: false,
            is_variadic: false,
            attributes: Vec::new(),
            local_names: Default::default(),
            function_body: None,
            span: Span::default(),
        };
        let vtable = BackendTraitObjectVtable {
            key: BackendTraitObjectVtableKey {
                self_ty: i32_ty,
                object_ty,
            },
            trait_id: TraitId::Source(trait_def),
            trait_args: vec![i32_ty],
            entries: Vec::new(),
            span: Span::default(),
        };
        let program = BackendProgram {
            modules: vec![BackendModule {
                id: module_id,
                name: "main".to_string(),
                const_eval: BackendConstFacts::default(),
                layouts: BackendLayouts {
                    target: nia_layout::TargetDataLayout::LP64,
                    types: vec![
                        (i32_ty, TypeLayout { size: 4, align: 4 }),
                        (object_ty, TypeLayout { size: 16, align: 8 }),
                    ],
                    structs: Vec::new(),
                    unions: Vec::new(),
                    struct_instances: vec![(struct_key.clone(), struct_layout.clone())],
                    union_instances: Vec::new(),
                },
                structs: Vec::new(),
                unions: Vec::new(),
                struct_instances: vec![BackendStructInstance {
                    def_id: struct_def,
                    name: sym("Box"),
                    args: vec![i32_ty],
                    const_args: Vec::new(),
                    symbol: "Box_i32".to_string(),
                    fields: Vec::new(),
                    is_extern: false,
                    span: Span::default(),
                }],
                union_instances: Vec::new(),
                enums: vec![BackendEnum {
                    def_id: enum_def,
                    name: sym("Choice"),
                    backing_type: i32_ty,
                    variants: vec![
                        BackendEnumVariant {
                            def_id: first_variant,
                            name: sym("First"),
                            value: None,
                            span: Span::default(),
                        },
                        BackendEnumVariant {
                            def_id: second_variant,
                            name: sym("Second"),
                            value: Some(7),
                            span: Span::default(),
                        },
                    ],
                    span: Span::default(),
                }],
                globals: Vec::new(),
                global_instances: Vec::new(),
                functions: Vec::new(),
                function_instances: vec![function_instance],
                trait_object_vtables: vec![vtable],
                generic_instantiations: Vec::<BackendGenericInstantiation>::new(),
            }],
        };

        let lowering = lowering(program);
        let index = ProgramIndex::new(Arc::clone(&lowering), Arc::new(type_store));

        assert!(std::ptr::eq(
            index.module(module_id).expect("indexed module"),
            &lowering.program.modules[0]
        ));
        assert_eq!(
            index.type_layout(i32_ty),
            Some(&TypeLayout { size: 4, align: 4 })
        );
        assert_eq!(
            index.struct_instance_layout(struct_def, &[i32_ty], &[]),
            Some(&struct_layout)
        );
        assert!(index.struct_instance(struct_def, &[i32_ty], &[]).is_some());
        assert!(std::ptr::eq(
            index
                .function_instance(function_def, module_id, None, &[i32_ty], &[])
                .expect("indexed function instance"),
            &lowering.program.modules[0].function_instances[0]
        ));
        let variant_info = index
            .enum_variant_info(second_variant)
            .expect("second enum variant info");
        assert_eq!(variant_info.owner.def_id, enum_def);
        assert_eq!(variant_info.variant.def_id, second_variant);
        assert_eq!(variant_info.index, 1);
        assert_eq!(
            index
                .trait_object_vtables_for_object_ty(object_ty)
                .map(|vtable| vtable.key.self_ty)
                .collect::<Vec<_>>(),
            vec![i32_ty]
        );
        assert_eq!(
            index
                .trait_object_vtables_for_trait(TraitId::Source(trait_def))
                .map(|vtable| vtable.key.object_ty)
                .collect::<Vec<_>>(),
            vec![object_ty]
        );
    }

    #[test]
    fn position_index_is_send_and_sync() {
        fn assert_send_sync<T: Send + Sync>() {}

        assert_send_sync::<ProgramIndex>();
    }

    #[test]
    fn resolves_types_without_a_program_module_view() {
        let mut module_ids = ModuleIdAllocator::new();
        let module_id = module_ids.allocate();
        let type_store = TypeStore::new();
        let ty = {
            let interner = type_store.append_for_module(module_id);
            let elem = interner.primitive(PrimitiveTy::U32);
            interner.intern(TyKind::Pointer {
                is_readonly: true,
                elem,
            })
        };
        let program = BackendProgram {
            modules: Vec::new(),
        };
        let index = ProgramIndex::new(lowering(program), Arc::new(type_store));

        assert!(matches!(
            index.ty_kind(ty),
            Some(TyKind::Pointer {
                is_readonly: true,
                ..
            })
        ));
    }

    fn global(module_id: ModuleId, def_id: u64) -> GlobalDefId {
        GlobalDefId {
            module_id,
            def_id: DefId(def_id),
        }
    }
}
