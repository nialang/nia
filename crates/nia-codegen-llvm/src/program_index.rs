// SPDX-License-Identifier: GPL-3.0-or-later
use std::collections::HashMap;

use nia_backend_ir::{
    BackendFunctionInstance, BackendProgram, BackendStructInstanceKey, BackendTraitObjectVtable,
};
use nia_ids::{GlobalDefId, InternedTyId, ModuleId};
use nia_layout::{StructLayout, TypeLayout};

pub(super) struct ProgramIndex<'a> {
    pub(super) modules: HashMap<ModuleId, &'a nia_backend_ir::BackendModule>,
    pub(super) structs: HashMap<GlobalDefId, &'a nia_backend_ir::BackendStruct>,
    pub(super) unions: HashMap<GlobalDefId, &'a nia_backend_ir::BackendUnion>,
    pub(super) struct_instances:
        HashMap<(GlobalDefId, Vec<InternedTyId>), &'a nia_backend_ir::BackendStructInstance>,
    pub(super) union_instances:
        HashMap<(GlobalDefId, Vec<InternedTyId>), &'a nia_backend_ir::BackendUnionInstance>,
    pub(super) enums: HashMap<GlobalDefId, &'a nia_backend_ir::BackendEnum>,
    pub(super) globals: HashMap<GlobalDefId, &'a nia_backend_ir::BackendGlobal>,
    pub(super) functions: HashMap<GlobalDefId, &'a nia_backend_ir::BackendFunction>,
    pub(super) function_instances:
        HashMap<(GlobalDefId, ModuleId, Vec<InternedTyId>), &'a BackendFunctionInstance>,
    pub(super) function_instances_by_def: HashMap<GlobalDefId, Vec<&'a BackendFunctionInstance>>,
    trait_object_vtables_by_object_ty: HashMap<InternedTyId, Vec<&'a BackendTraitObjectVtable>>,
    type_layouts: HashMap<InternedTyId, &'a TypeLayout>,
    struct_layouts: HashMap<GlobalDefId, &'a StructLayout>,
    union_layouts: HashMap<GlobalDefId, &'a StructLayout>,
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

impl<'a> ProgramIndex<'a> {
    pub(super) fn new(program: &'a BackendProgram) -> Self {
        let mut index = Self {
            modules: HashMap::new(),
            structs: HashMap::new(),
            unions: HashMap::new(),
            struct_instances: HashMap::new(),
            union_instances: HashMap::new(),
            enums: HashMap::new(),
            globals: HashMap::new(),
            functions: HashMap::new(),
            function_instances: HashMap::new(),
            function_instances_by_def: HashMap::new(),
            trait_object_vtables_by_object_ty: HashMap::new(),
            type_layouts: HashMap::new(),
            struct_layouts: HashMap::new(),
            union_layouts: HashMap::new(),
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
                    .struct_instance_layouts_by_def
                    .entry(key.def_id)
                    .or_default()
                    .push(BackendLayoutInstance { key, layout });
            }
            for (key, layout) in &module.layouts.union_instances {
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
}
