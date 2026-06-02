// SPDX-License-Identifier: GPL-3.0-or-later
use std::collections::HashMap;

use nia_backend_ir::{
    BackendEnum, BackendEnumVariant, BackendFunctionInstance, BackendProgram,
    BackendStructInstanceKey, BackendTraitObjectVtable,
};
use nia_ids::{GlobalDefId, InternedTyId, ModuleId};
use nia_layout::{StructLayout, TypeLayout};
use nia_ty::{TraitId, TyKind};

pub(super) struct ProgramIndex<'a> {
    pub(super) modules: HashMap<ModuleId, &'a nia_backend_ir::BackendModule>,
    pub(super) structs: HashMap<GlobalDefId, &'a nia_backend_ir::BackendStruct>,
    pub(super) unions: HashMap<GlobalDefId, &'a nia_backend_ir::BackendUnion>,
    pub(super) struct_instances:
        HashMap<(GlobalDefId, Vec<InternedTyId>), &'a nia_backend_ir::BackendStructInstance>,
    pub(super) union_instances:
        HashMap<(GlobalDefId, Vec<InternedTyId>), &'a nia_backend_ir::BackendUnionInstance>,
    pub(super) enums: HashMap<GlobalDefId, &'a nia_backend_ir::BackendEnum>,
    pub(super) enum_variants: HashMap<GlobalDefId, &'a nia_backend_ir::BackendEnumVariant>,
    pub(super) enum_variant_infos: HashMap<GlobalDefId, EnumVariantInfo<'a>>,
    pub(super) globals: HashMap<GlobalDefId, &'a nia_backend_ir::BackendGlobal>,
    pub(super) functions: HashMap<GlobalDefId, &'a nia_backend_ir::BackendFunction>,
    pub(super) function_instances:
        HashMap<(GlobalDefId, ModuleId, Vec<InternedTyId>), &'a BackendFunctionInstance>,
    pub(super) function_instances_by_def: HashMap<GlobalDefId, Vec<&'a BackendFunctionInstance>>,
    trait_object_vtables_by_object_ty: HashMap<InternedTyId, Vec<&'a BackendTraitObjectVtable>>,
    trait_object_vtables_by_trait: HashMap<TraitId, Vec<&'a BackendTraitObjectVtable>>,
    type_layouts: HashMap<InternedTyId, &'a TypeLayout>,
    struct_layouts: HashMap<GlobalDefId, &'a StructLayout>,
    union_layouts: HashMap<GlobalDefId, &'a StructLayout>,
    struct_instance_layouts: HashMap<GlobalDefId, HashMap<Vec<InternedTyId>, &'a StructLayout>>,
    union_instance_layouts: HashMap<GlobalDefId, HashMap<Vec<InternedTyId>, &'a StructLayout>>,
    struct_instance_layouts_by_def: HashMap<GlobalDefId, Vec<BackendLayoutInstance<'a>>>,
    union_instance_layouts_by_def: HashMap<GlobalDefId, Vec<BackendLayoutInstance<'a>>>,
    pub(super) struct_instances_by_def:
        HashMap<GlobalDefId, Vec<&'a nia_backend_ir::BackendStructInstance>>,
    pub(super) union_instances_by_def:
        HashMap<GlobalDefId, Vec<&'a nia_backend_ir::BackendUnionInstance>>,
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

impl<'a> ProgramIndex<'a> {
    pub(super) fn new(program: &'a BackendProgram) -> Self {
        let mut index = Self {
            modules: HashMap::new(),
            structs: HashMap::new(),
            unions: HashMap::new(),
            struct_instances: HashMap::new(),
            union_instances: HashMap::new(),
            enums: HashMap::new(),
            enum_variants: HashMap::new(),
            enum_variant_infos: HashMap::new(),
            globals: HashMap::new(),
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
        for module in &program.modules {
            index.modules.insert(module.id, module);
            for (ty, layout) in &module.layouts.types {
                index.type_layouts.insert(*ty, layout);
            }
            for (def_id, layout) in &module.layouts.structs {
                index.struct_layouts.insert(*def_id, layout);
            }
            for (def_id, layout) in &module.layouts.unions {
                index.union_layouts.insert(*def_id, layout);
            }
            for (key, layout) in &module.layouts.struct_instances {
                index
                    .struct_instance_layouts
                    .entry(key.def_id)
                    .or_default()
                    .insert(key.args.clone(), layout);
                index
                    .struct_instance_layouts_by_def
                    .entry(key.def_id)
                    .or_default()
                    .push(BackendLayoutInstance { key, layout });
            }
            for (key, layout) in &module.layouts.union_instances {
                index
                    .union_instance_layouts
                    .entry(key.def_id)
                    .or_default()
                    .insert(key.args.clone(), layout);
                index
                    .union_instance_layouts_by_def
                    .entry(key.def_id)
                    .or_default()
                    .push(BackendLayoutInstance { key, layout });
            }
            for item in &module.structs {
                index.structs.insert(item.def_id, item);
            }
            for item in &module.unions {
                index.unions.insert(item.def_id, item);
            }
            for item in &module.struct_instances {
                index
                    .struct_instances
                    .insert((item.def_id, item.args.clone()), item);
                index
                    .struct_instances_by_def
                    .entry(item.def_id)
                    .or_default()
                    .push(item);
            }
            for item in &module.union_instances {
                index
                    .union_instances
                    .insert((item.def_id, item.args.clone()), item);
                index
                    .union_instances_by_def
                    .entry(item.def_id)
                    .or_default()
                    .push(item);
            }
            for item in &module.enums {
                index.enums.insert(item.def_id, item);
                for (variant_index, variant) in item.variants.iter().enumerate() {
                    index.enum_variants.insert(variant.def_id, variant);
                    index.enum_variant_infos.insert(
                        variant.def_id,
                        EnumVariantInfo {
                            owner: item,
                            variant,
                            index: variant_index,
                        },
                    );
                }
            }
            for item in &module.globals {
                index.globals.insert(item.def_id, item);
            }
            for item in &module.functions {
                index.functions.insert(item.def_id, item);
            }
            for item in &module.function_instances {
                index
                    .function_instances
                    .insert((item.def_id, item.arg_module_id, item.args.clone()), item);
                index
                    .function_instances_by_def
                    .entry(item.def_id)
                    .or_default()
                    .push(item);
            }
            for vtable in &module.trait_object_vtables {
                index
                    .trait_object_vtables_by_object_ty
                    .entry(vtable.key.object_ty)
                    .or_default()
                    .push(vtable);
                if let Some(TyKind::TraitObject { trait_id, .. }) =
                    module.interner.get(vtable.key.object_ty)
                {
                    index
                        .trait_object_vtables_by_trait
                        .entry(*trait_id)
                        .or_default()
                        .push(vtable);
                }
            }
        }
        index
    }

    pub(super) fn module(&self, module_id: ModuleId) -> Option<&'a nia_backend_ir::BackendModule> {
        self.modules.get(&module_id).copied()
    }

    pub(super) fn type_layout(&self, ty: InternedTyId) -> Option<&'a TypeLayout> {
        self.type_layouts.get(&ty).copied()
    }

    pub(super) fn struct_layout(&self, def_id: GlobalDefId) -> Option<&'a StructLayout> {
        self.struct_layouts.get(&def_id).copied()
    }

    pub(super) fn union_layout(&self, def_id: GlobalDefId) -> Option<&'a StructLayout> {
        self.union_layouts.get(&def_id).copied()
    }

    pub(super) fn struct_instance_layout(
        &self,
        def_id: GlobalDefId,
        args: &[InternedTyId],
    ) -> Option<&'a StructLayout> {
        self.struct_instance_layouts
            .get(&def_id)
            .and_then(|layouts| layouts.get(args))
            .copied()
    }

    pub(super) fn union_instance_layout(
        &self,
        def_id: GlobalDefId,
        args: &[InternedTyId],
    ) -> Option<&'a StructLayout> {
        self.union_instance_layouts
            .get(&def_id)
            .and_then(|layouts| layouts.get(args))
            .copied()
    }

    pub(super) fn struct_instance(
        &self,
        def_id: GlobalDefId,
        args: &[InternedTyId],
    ) -> Option<&'a nia_backend_ir::BackendStructInstance> {
        self.struct_instances.get(&(def_id, args.to_vec())).copied()
    }

    pub(super) fn union_instance(
        &self,
        def_id: GlobalDefId,
        args: &[InternedTyId],
    ) -> Option<&'a nia_backend_ir::BackendUnionInstance> {
        self.union_instances.get(&(def_id, args.to_vec())).copied()
    }

    pub(super) fn function_instance(
        &self,
        def_id: GlobalDefId,
        arg_module_id: ModuleId,
        args: &[InternedTyId],
    ) -> Option<&'a BackendFunctionInstance> {
        self.function_instances
            .get(&(def_id, arg_module_id, args.to_vec()))
            .copied()
    }

    pub(super) fn struct_instance_layouts(
        &self,
        def_id: GlobalDefId,
    ) -> impl Iterator<Item = &BackendLayoutInstance<'a>> {
        self.struct_instance_layouts_by_def
            .get(&def_id)
            .into_iter()
            .flatten()
    }

    pub(super) fn union_instance_layouts(
        &self,
        def_id: GlobalDefId,
    ) -> impl Iterator<Item = &BackendLayoutInstance<'a>> {
        self.union_instance_layouts_by_def
            .get(&def_id)
            .into_iter()
            .flatten()
    }

    pub(super) fn trait_object_vtables_for_object_ty(
        &self,
        object_ty: InternedTyId,
    ) -> impl Iterator<Item = &'a BackendTraitObjectVtable> {
        self.trait_object_vtables_by_object_ty
            .get(&object_ty)
            .into_iter()
            .flatten()
            .copied()
    }

    pub(super) fn trait_object_vtables_for_trait(
        &self,
        trait_id: TraitId,
    ) -> impl Iterator<Item = &'a BackendTraitObjectVtable> {
        self.trait_object_vtables_by_trait
            .get(&trait_id)
            .into_iter()
            .flatten()
            .copied()
    }
}
