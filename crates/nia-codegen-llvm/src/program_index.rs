// SPDX-License-Identifier: GPL-3.0-or-later
use std::collections::HashMap;

use nia_backend_ir::{BackendFunctionInstance, BackendProgram};
use nia_ids::{GlobalDefId, ModuleId, TyId};

pub(super) struct ProgramIndex<'a> {
    pub(super) modules: HashMap<ModuleId, &'a nia_backend_ir::BackendModule>,
    pub(super) structs: HashMap<GlobalDefId, &'a nia_backend_ir::BackendStruct>,
    pub(super) struct_instances:
        HashMap<(GlobalDefId, Vec<TyId>), &'a nia_backend_ir::BackendStructInstance>,
    pub(super) enums: HashMap<GlobalDefId, &'a nia_backend_ir::BackendEnum>,
    pub(super) globals: HashMap<GlobalDefId, &'a nia_backend_ir::BackendGlobal>,
    pub(super) functions: HashMap<GlobalDefId, &'a nia_backend_ir::BackendFunction>,
    pub(super) function_instances: HashMap<(GlobalDefId, Vec<TyId>), &'a BackendFunctionInstance>,
}

impl<'a> ProgramIndex<'a> {
    pub(super) fn new(program: &'a BackendProgram) -> Self {
        let mut index = Self {
            modules: HashMap::new(),
            structs: HashMap::new(),
            struct_instances: HashMap::new(),
            enums: HashMap::new(),
            globals: HashMap::new(),
            functions: HashMap::new(),
            function_instances: HashMap::new(),
        };
        for module in &program.modules {
            index.modules.insert(module.id, module);
            for item in &module.structs {
                index.structs.insert(item.def_id, item);
            }
            for item in &module.struct_instances {
                index
                    .struct_instances
                    .insert((item.def_id, item.args.clone()), item);
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
                    .insert((item.def_id, item.args.clone()), item);
            }
        }
        index
    }

    pub(super) fn module(&self, module_id: ModuleId) -> Option<&'a nia_backend_ir::BackendModule> {
        self.modules.get(&module_id).copied()
    }
}
