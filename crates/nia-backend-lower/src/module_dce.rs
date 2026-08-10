// SPDX-License-Identifier: GPL-3.0-or-later
use std::collections::{HashMap, HashSet, VecDeque};

use crate::{BackendOptimizationChange, ModuleLowerer, backend_function_instance_key};
use nia_backend_ir::{
    BackendClosureEntry, BackendClosureEntryOwner, BackendFunction, BackendFunctionInstance,
    BackendGlobal, BackendTraitObjectVtable, BackendTraitObjectVtableFunction,
};
use nia_defs::DefKind;
use nia_function_ir::{FunctionBodyRefs, FunctionInstanceKey, FunctionInstanceRef};
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
        closure_entries: &mut Vec<BackendClosureEntry>,
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
            .map(backend_function_instance_key)
            .collect::<HashSet<_>>();
        if removable_functions.is_empty() && removable_instances.is_empty() {
            return;
        }

        let mut refs = FunctionBodyRefs::default();
        for function in functions.iter() {
            if !removable_functions.contains(&function.def_id)
                && let Some(body) = &function.function_body
            {
                refs.extend(body.value_refs(self.type_store));
                extend_closure_entry_refs(
                    closure_entries,
                    BackendClosureEntryOwner::Source(function.def_id),
                    self.type_store,
                    &mut refs,
                );
            }
        }
        for instance in function_instances.iter() {
            if !removable_instances.contains(&backend_function_instance_key(instance))
                && let Some(body) = &instance.function_body
            {
                refs.extend(body.value_refs(self.type_store));
                extend_closure_entry_refs(
                    closure_entries,
                    BackendClosureEntryOwner::FunctionInstance(backend_function_instance_key(
                        instance,
                    )),
                    self.type_store,
                    &mut refs,
                );
            }
        }
        for global in globals {
            if let Some(init) = &global.init {
                refs.extend(init.value_refs(self.input.module_id));
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
                        refs.function_instances.push(FunctionInstanceRef {
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
        collect_transitive_refs(
            functions,
            function_instances,
            closure_entries,
            self.type_store,
            &mut refs,
        );

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
            .function_instances
            .iter()
            .map(FunctionInstanceRef::key)
            .collect::<HashSet<_>>();
        function_instances.retain(|instance| {
            let key = backend_function_instance_key(instance);
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
        let live_functions = functions
            .iter()
            .map(|function| function.def_id)
            .collect::<HashSet<_>>();
        let live_instances = function_instances
            .iter()
            .map(backend_function_instance_key)
            .collect::<HashSet<_>>();
        closure_entries.retain(|entry| match &entry.key.owner {
            BackendClosureEntryOwner::Source(def_id) => live_functions.contains(def_id),
            BackendClosureEntryOwner::FunctionInstance(key) => live_instances.contains(key),
        });
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
        matches!(def.kind, DefKind::Function) && def.visibility == Visibility::Private
    }

    fn is_removable_private_function_instance(&self, instance: &BackendFunctionInstance) -> bool {
        if instance.is_extern || instance.def_id.module_id != self.input.module_id {
            return false;
        }
        let Some(def) = self.input.defs.defs.get(instance.def_id.def_id) else {
            return false;
        };
        matches!(def.kind, DefKind::Function) && def.visibility == Visibility::Private
    }
}

fn collect_transitive_refs(
    functions: &[BackendFunction],
    instances: &[BackendFunctionInstance],
    closure_entries: &[BackendClosureEntry],
    types: &nia_ty::TypeStore,
    refs: &mut FunctionBodyRefs,
) {
    let functions_by_id = functions
        .iter()
        .map(|function| (function.def_id, function))
        .collect::<HashMap<_, _>>();
    let instances_by_ref = instances
        .iter()
        .map(|instance| (backend_function_instance_key(instance), instance))
        .collect::<HashMap<_, _>>();
    let mut visited_functions = HashSet::new();
    let mut visited_instances = HashSet::new();
    let mut pending_functions = refs.functions.iter().copied().collect::<VecDeque<_>>();
    let mut pending_instances = refs
        .function_instances
        .iter()
        .cloned()
        .collect::<VecDeque<_>>();
    let mut known_instances = refs
        .function_instances
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
            let discovered = function
                .function_body
                .as_ref()
                .map(|body| body.value_refs(types))
                .unwrap_or_default();
            let mut discovered = discovered;
            extend_closure_entry_refs(
                closure_entries,
                BackendClosureEntryOwner::Source(function_id),
                types,
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
            let discovered = instance
                .function_body
                .as_ref()
                .map(|body| body.value_refs(types))
                .unwrap_or_default();
            let mut discovered = discovered;
            extend_closure_entry_refs(
                closure_entries,
                BackendClosureEntryOwner::FunctionInstance(instance_key.clone()),
                types,
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

fn extend_closure_entry_refs(
    closure_entries: &[BackendClosureEntry],
    owner: BackendClosureEntryOwner,
    types: &nia_ty::TypeStore,
    refs: &mut FunctionBodyRefs,
) {
    for entry in closure_entries
        .iter()
        .filter(|entry| entry.key.owner == owner)
    {
        refs.extend(entry.function_body.value_refs(types));
    }
}

fn enqueue_new_refs(
    refs: &mut FunctionBodyRefs,
    discovered: FunctionBodyRefs,
    known_instances: &mut HashSet<FunctionInstanceKey>,
    pending_functions: &mut VecDeque<GlobalDefId>,
    pending_instances: &mut VecDeque<FunctionInstanceRef>,
) {
    for function in discovered.functions {
        if refs.functions.insert(function) {
            pending_functions.push_back(function);
        }
    }
    for instance in discovered.function_instances {
        if known_instances.insert(instance.key()) {
            refs.function_instances.push(instance.clone());
            pending_instances.push_back(instance);
        }
    }
}
