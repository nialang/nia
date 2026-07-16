// SPDX-License-Identifier: GPL-3.0-or-later
use std::collections::{HashMap, HashSet, VecDeque};

use crate::function_refs::{
    FunctionInstanceKey, FunctionInstanceRef, FunctionRefs,
    collect_function_refs_from_optional_body, collect_function_refs_from_static_init,
};
use crate::{BackendOptimizationChange, ModuleLowerer};
use nia_backend_ir::{
    BackendFunction, BackendFunctionInstance, BackendGlobal, BackendTraitObjectVtable,
    BackendTraitObjectVtableFunction,
};
use nia_defs::DefKind;
use nia_ids::{GlobalDefId, Visibility};
use nia_opt::OptimizationDepth;
use nia_symbol::known;

pub(crate) const REMOVE_UNUSED_FUNCTIONS_PASS: &str = "remove-unused-functions";
pub(crate) const REMOVE_UNUSED_FUNCTION_INSTANCES_PASS: &str = "remove-unused-function-instances";

impl<'a> ModuleLowerer<'a> {
    pub(crate) fn remove_unused_private_functions(
        &mut self,
        functions: &mut Vec<BackendFunction>,
        function_instances: &mut Vec<BackendFunctionInstance>,
        globals: &[BackendGlobal],
        trait_object_vtables: &[BackendTraitObjectVtable],
    ) {
        if !self
            .optimization
            .dead_code_elim
            .at_least(OptimizationDepth::Full)
        {
            return;
        }

        let removable_functions = functions
            .iter()
            .filter(|function| self.is_removable_private_function(function))
            .map(|function| function.def_id)
            .collect::<HashSet<_>>();
        let removable_instances = function_instances
            .iter()
            .filter(|instance| self.is_removable_private_function_instance(instance))
            .map(FunctionInstanceKey::from)
            .collect::<HashSet<_>>();
        if removable_functions.is_empty() && removable_instances.is_empty() {
            return;
        }

        let mut refs = FunctionRefs::default();
        for function in functions.iter() {
            if !removable_functions.contains(&function.def_id) {
                collect_function_refs_from_optional_body(
                    self.input.module_id,
                    &function.function_body,
                    &mut refs,
                );
            }
        }
        for instance in function_instances.iter() {
            if !removable_instances.contains(&FunctionInstanceKey::from(instance)) {
                collect_function_refs_from_optional_body(
                    instance.arg_module_id,
                    &instance.function_body,
                    &mut refs,
                );
            }
        }
        for global in globals {
            if let Some(init) = &global.init {
                collect_function_refs_from_static_init(self.input.module_id, init, &mut refs);
            }
        }
        for vtable in trait_object_vtables {
            for entry in &vtable.entries {
                match &entry.function {
                    BackendTraitObjectVtableFunction::Function(function) => {
                        refs.functions.insert(*function);
                    }
                    BackendTraitObjectVtableFunction::FunctionInstance {
                        def_id,
                        arg_module_id,
                        self_arg,
                        args,
                        const_args,
                    } => {
                        refs.instances.push(FunctionInstanceRef {
                            def_id: *def_id,
                            arg_module_id: *arg_module_id,
                            self_arg: *self_arg,
                            args: args.clone(),
                            const_args: const_args.clone(),
                            span: vtable.span,
                        });
                    }
                }
            }
        }
        collect_transitive_refs(functions, function_instances, &mut refs);

        let mut removed_functions = Vec::new();
        functions.retain(|function| {
            let remove = removable_functions.contains(&function.def_id)
                && !refs.functions.contains(&function.def_id);
            if remove {
                removed_functions.push(function.def_id);
            }
            !remove
        });
        for function in removed_functions {
            self.optimization_report
                .changed_passes
                .push(BackendOptimizationChange::Function {
                    module_id: self.input.module_id,
                    function,
                    pass: REMOVE_UNUSED_FUNCTIONS_PASS,
                    is_instance: false,
                    type_arg_count: 0,
                });
        }

        let mut removed_instances = Vec::new();
        let live_instance_keys = refs
            .instances
            .iter()
            .map(FunctionInstanceRef::key)
            .collect::<HashSet<_>>();
        function_instances.retain(|instance| {
            let key = FunctionInstanceKey::from(instance);
            let remove = removable_instances.contains(&key) && !live_instance_keys.contains(&key);
            if remove {
                removed_instances.push((instance.def_id, instance.args.len()));
            }
            !remove
        });
        for (function, type_arg_count) in removed_instances {
            self.optimization_report
                .changed_passes
                .push(BackendOptimizationChange::Function {
                    module_id: self.input.module_id,
                    function,
                    pass: REMOVE_UNUSED_FUNCTION_INSTANCES_PASS,
                    is_instance: true,
                    type_arg_count,
                });
        }
    }

    fn is_removable_private_function(&self, function: &BackendFunction) -> bool {
        if function.is_extern || function.def_id.module_id != self.input.module_id {
            return false;
        }
        let Some(def) = self.input.defs.defs.get(function.def_id.def_id) else {
            return false;
        };
        if def.name == known::MAIN {
            return false;
        }
        matches!(def.kind, DefKind::Function) && def.visibility != Visibility::Public
    }

    fn is_removable_private_function_instance(&self, instance: &BackendFunctionInstance) -> bool {
        if instance.is_extern || instance.def_id.module_id != self.input.module_id {
            return false;
        }
        let Some(def) = self.input.defs.defs.get(instance.def_id.def_id) else {
            return false;
        };
        matches!(def.kind, DefKind::Function) && def.visibility != Visibility::Public
    }
}

impl From<&BackendFunctionInstance> for FunctionInstanceRef {
    fn from(instance: &BackendFunctionInstance) -> Self {
        Self {
            def_id: instance.def_id,
            arg_module_id: instance.arg_module_id,
            self_arg: instance.self_arg,
            args: instance.args.clone(),
            const_args: instance.const_args.clone(),
            span: instance.span,
        }
    }
}

impl From<&BackendFunctionInstance> for FunctionInstanceKey {
    fn from(instance: &BackendFunctionInstance) -> Self {
        Self {
            def_id: instance.def_id,
            arg_module_id: instance.arg_module_id,
            self_arg: instance.self_arg,
            args: instance.args.clone(),
            const_args: instance.const_args.clone(),
        }
    }
}

fn collect_transitive_refs(
    functions: &[BackendFunction],
    instances: &[BackendFunctionInstance],
    refs: &mut FunctionRefs,
) {
    let functions_by_id = functions
        .iter()
        .map(|function| (function.def_id, function))
        .collect::<HashMap<_, _>>();
    let instances_by_ref = instances
        .iter()
        .map(|instance| (FunctionInstanceKey::from(instance), instance))
        .collect::<HashMap<_, _>>();
    let mut visited_functions = HashSet::new();
    let mut visited_instances = HashSet::new();
    let mut pending_functions = refs.functions.iter().copied().collect::<VecDeque<_>>();
    let mut pending_instances = refs.instances.iter().cloned().collect::<VecDeque<_>>();
    let mut known_instances = refs
        .instances
        .iter()
        .map(FunctionInstanceRef::key)
        .collect::<HashSet<_>>();

    while !pending_functions.is_empty() || !pending_instances.is_empty() {
        while let Some(function_id) = pending_functions.pop_front() {
            if !visited_functions.insert(function_id) {
                continue;
            }
            let Some(function) = functions_by_id.get(&function_id) else {
                continue;
            };
            let mut discovered = FunctionRefs::default();
            collect_function_refs_from_optional_body(
                function.def_id.module_id,
                &function.function_body,
                &mut discovered,
            );
            enqueue_new_refs(
                refs,
                discovered,
                &mut known_instances,
                &mut pending_functions,
                &mut pending_instances,
            );
        }

        while let Some(instance_ref) = pending_instances.pop_front() {
            let instance_key = instance_ref.key();
            if !visited_instances.insert(instance_key.clone()) {
                continue;
            }
            let Some(instance) = instances_by_ref.get(&instance_key) else {
                continue;
            };
            let mut discovered = FunctionRefs::default();
            collect_function_refs_from_optional_body(
                instance.arg_module_id,
                &instance.function_body,
                &mut discovered,
            );
            enqueue_new_refs(
                refs,
                discovered,
                &mut known_instances,
                &mut pending_functions,
                &mut pending_instances,
            );
        }
    }
}

fn enqueue_new_refs(
    refs: &mut FunctionRefs,
    discovered: FunctionRefs,
    known_instances: &mut HashSet<FunctionInstanceKey>,
    pending_functions: &mut VecDeque<GlobalDefId>,
    pending_instances: &mut VecDeque<FunctionInstanceRef>,
) {
    for function in discovered.functions {
        if refs.functions.insert(function) {
            pending_functions.push_back(function);
        }
    }
    for instance in discovered.instances {
        if known_instances.insert(instance.key()) {
            refs.instances.push(instance.clone());
            pending_instances.push_back(instance);
        }
    }
}
