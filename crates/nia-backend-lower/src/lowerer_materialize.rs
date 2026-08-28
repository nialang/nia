// SPDX-License-Identifier: GPL-3.0-or-later
//! Reachability fixed point and concrete function/global instance materialization.

use super::*;

impl ModuleLowerer<'_> {
    pub(crate) fn lower_reachable_function_closure(
        &mut self,
        functions: &mut Vec<BackendFunction>,
        worklist: &mut ReachabilityWorklist,
        trait_object_vtables: &mut Vec<BackendTraitObjectVtable>,
    ) -> bool {
        let mut changed = false;
        let mut lowered = functions
            .iter()
            .map(|function| function.def_id)
            .collect::<HashSet<_>>();
        while let Some(def_id) = worklist.pending_functions.pop_front() {
            if def_id.module_id != self.input.module_id {
                if !self.foreign_source_function_is_preplanned(def_id) {
                    self.foreign_function_refs.push(def_id);
                }
                continue;
            }
            if lowered.contains(&def_id) {
                continue;
            }
            if self
                .input
                .defs
                .defs
                .get(def_id.def_id)
                .is_some_and(|def| def.kind == DefKind::TraitMethod)
            {
                continue;
            }
            let Some(source) = self.function_sources.get(&def_id).copied() else {
                continue;
            };
            let Some(function) = self.lower_function(source.span, source.function) else {
                continue;
            };
            lowered.insert(def_id);
            if function.generics.is_empty() {
                let mut discovery =
                    self.discover_backend_items_from_optional_body(&function.function_body);
                for entry in self.input.program.closure_entries(function.def_id) {
                    discovery.extend(self.discover_backend_items_from_body(&entry.body));
                }
                worklist.enqueue_refs(discovery.refs);
                self.append_trait_object_vtable_delta(
                    trait_object_vtables,
                    discovery.trait_object_vtables,
                    worklist,
                );
                functions.push(function);
                changed = true;
            }
        }
        changed
    }

    pub(crate) fn lower_additional_functions(
        &mut self,
        refs: Vec<GlobalDefId>,
        module: &mut BackendModule,
    ) {
        let mut worklist = self.reachability_worklist_for_module(module);
        for def_id in refs {
            worklist.enqueue_function(def_id);
        }
        let mut function_templates = Vec::new();
        self.lower_reachable_instances_and_vtables(
            &mut module.functions,
            &mut function_templates,
            (&mut module.function_instances, &mut module.closure_entries),
            &mut module.global_instances,
            &mut worklist,
            &mut module.trait_object_vtables,
        );
    }

    pub(crate) fn lower_additional_function_instances_into_module(
        &mut self,
        refs: Vec<FunctionInstanceRef>,
        module: &mut BackendModule,
    ) {
        let materialized = self.lower_additional_function_instances(
            refs,
            &module.functions,
            &module.function_instances,
        );
        let FunctionInstanceMaterialization {
            instances,
            closure_entries,
            discovery,
        } = materialized;
        if instances.is_empty() {
            return;
        }
        module.function_instances.extend(instances);
        module.closure_entries.extend(closure_entries);
        self.lower_additional_reachable_items(discovery, module);
    }

    pub(crate) fn lower_additional_global_instances(
        &mut self,
        refs: Vec<GlobalInstanceRef>,
        module: &mut BackendModule,
    ) {
        let materialized = self.lower_global_instances_from_refs(refs, &module.global_instances);
        let GlobalInstanceMaterialization {
            instances,
            discovery,
        } = materialized;
        if instances.is_empty() {
            return;
        }
        module.global_instances.extend(instances);
        self.lower_additional_reachable_items(discovery, module);
    }

    pub(crate) fn lower_additional_reachable_items(
        &mut self,
        discovery: BackendItemDiscovery,
        module: &mut BackendModule,
    ) {
        let mut worklist = self.reachability_worklist_for_module(module);
        worklist.enqueue_refs(discovery.refs);
        self.append_trait_object_vtable_delta(
            &mut module.trait_object_vtables,
            discovery.trait_object_vtables,
            &mut worklist,
        );
        let mut function_templates = Vec::new();
        self.lower_reachable_instances_and_vtables(
            &mut module.functions,
            &mut function_templates,
            (&mut module.function_instances, &mut module.closure_entries),
            &mut module.global_instances,
            &mut worklist,
            &mut module.trait_object_vtables,
        );
    }

    pub(crate) fn reachability_worklist_for_module(
        &self,
        module: &BackendModule,
    ) -> ReachabilityWorklist {
        ReachabilityWorklist {
            pending_functions: VecDeque::new(),
            queued_functions: module
                .functions
                .iter()
                .map(|function| function.def_id)
                .collect::<HashSet<_>>(),
            pending_instances: Vec::new(),
            queued_instances: module
                .function_instances
                .iter()
                .map(backend_function_instance_key)
                .collect::<HashSet<_>>(),
            pending_global_instances: Vec::new(),
            queued_global_instances: module
                .global_instances
                .iter()
                .map(|instance| GlobalInstanceKey {
                    def_id: instance.def_id,
                    arg_module_id: instance.arg_module_id,
                    args: instance.args.clone(),
                    const_args: instance.const_args.clone(),
                })
                .collect::<HashSet<_>>(),
        }
    }

    pub(crate) fn lower_reachable_instances_and_vtables(
        &mut self,
        functions: &mut Vec<BackendFunction>,
        function_templates: &mut Vec<BackendFunction>,
        callable_instances: (
            &mut Vec<BackendFunctionInstance>,
            &mut Vec<BackendClosureEntry>,
        ),
        global_instances: &mut Vec<BackendGlobalInstance>,
        worklist: &mut ReachabilityWorklist,
        trait_object_vtables: &mut Vec<BackendTraitObjectVtable>,
    ) {
        let (function_instances, closure_entries) = callable_instances;
        // Function instances, global instances, and vtables can discover references to each
        // other. Drain all three queues as one fixed point so every referenced backend item has
        // an owning finalized module.
        loop {
            let mut changed =
                self.lower_reachable_function_closure(functions, worklist, trait_object_vtables);
            if !worklist.pending_instances.is_empty() {
                self.lower_pending_instance_templates(
                    function_templates,
                    &worklist.pending_instances,
                );
                let refs = std::mem::take(&mut worklist.pending_instances);
                let additional = self.lower_additional_function_instances(
                    refs,
                    function_templates,
                    function_instances,
                );
                let FunctionInstanceMaterialization {
                    instances,
                    closure_entries: additional_closure_entries,
                    discovery,
                } = additional;
                worklist.enqueue_refs(discovery.refs);
                changed |= self.append_trait_object_vtable_delta(
                    trait_object_vtables,
                    discovery.trait_object_vtables,
                    worklist,
                );
                changed |= !instances.is_empty();
                function_instances.extend(instances);
                closure_entries.extend(additional_closure_entries);
            }
            if !worklist.pending_global_instances.is_empty() {
                let refs = std::mem::take(&mut worklist.pending_global_instances);
                let additional =
                    self.lower_global_instances_from_refs(refs, global_instances.as_slice());
                let GlobalInstanceMaterialization {
                    instances,
                    discovery,
                } = additional;
                worklist.enqueue_refs(discovery.refs);
                changed |= self.append_trait_object_vtable_delta(
                    trait_object_vtables,
                    discovery.trait_object_vtables,
                    worklist,
                );
                changed |= !instances.is_empty();
                global_instances.extend(instances);
            }
            if !changed
                && worklist.pending_functions.is_empty()
                && worklist.pending_instances.is_empty()
                && worklist.pending_global_instances.is_empty()
            {
                break;
            }
        }
    }

    pub(crate) fn lower_pending_instance_templates(
        &mut self,
        function_templates: &mut Vec<BackendFunction>,
        pending_instances: &[FunctionInstanceRef],
    ) {
        let mut known = function_templates
            .iter()
            .map(|function| function.def_id)
            .collect::<HashSet<_>>();
        for instance in pending_instances {
            if !known.insert(instance.def_id) {
                continue;
            }
            let function = if instance.def_id.module_id == self.input.module_id {
                self.function_sources
                    .get(&instance.def_id)
                    .copied()
                    .and_then(|source| self.lower_function(source.span, source.function))
            } else {
                self.backend_function_template_for_program_def(instance.def_id)
            };
            if let Some(function) = function {
                function_templates.push(function);
            }
        }
    }

    pub(crate) fn lower_global_instances_from_refs(
        &mut self,
        refs: Vec<GlobalInstanceRef>,
        existing: &[BackendGlobalInstance],
    ) -> GlobalInstanceMaterialization {
        let mut instances = Vec::new();
        let mut discovery = BackendItemDiscovery::default();
        let mut seen = existing
            .iter()
            .map(|instance| BackendGlobalInstanceKey {
                def_id: instance.def_id,
                arg_module_id: instance.arg_module_id,
                args: self.canonicalize_instance_args(&instance.args),
                const_args: instance.const_args.clone(),
            })
            .collect::<HashSet<_>>();
        // Canonical argument identities make duplicate discovery independent of which module or
        // worklist edge reached an instance first.
        for instance in refs {
            if instance.def_id.module_id != self.input.module_id {
                self.foreign_global_instance_refs.push(instance);
                continue;
            }
            let args = self.canonicalize_global_instance_ref_args(&instance);
            let const_args = instance.const_args.clone();
            let key = BackendGlobalInstanceKey {
                def_id: instance.def_id,
                arg_module_id: instance.arg_module_id,
                args: args.clone(),
                const_args: const_args.clone(),
            };
            if !seen.insert(key) {
                continue;
            }
            if args.iter().any(|arg| {
                self.cached_ty_contains_generic_param(*arg)
                    || self.cached_ty_contains_unresolved_projection(*arg)
                    || self.cached_ty_contains_error(*arg)
            }) {
                continue;
            }
            let Some(global) = self.lower_planned_global_instance(
                instance.def_id,
                instance.arg_module_id,
                args,
                const_args,
            ) else {
                continue;
            };
            if let Some(init) = &global.init {
                discovery.refs.extend(init.value_refs(global.arg_module_id));
            }
            instances.push(global);
        }
        GlobalInstanceMaterialization {
            instances,
            discovery,
        }
    }

    pub(crate) fn lower_planned_global_instance(
        &mut self,
        def_id: GlobalDefId,
        arg_module_id: ModuleId,
        args: Vec<InternedTyId>,
        const_args: Vec<nia_ty::ConstGenericArg>,
    ) -> Option<BackendGlobalInstance> {
        let signature = self.input.signatures.globals.get(&def_id.def_id)?;
        if signature.is_extern {
            return None;
        }
        let def = self.input.defs.defs.get(def_id.def_id)?;
        let owner = def.parent?;
        let owner_def_id = GlobalDefId {
            module_id: def_id.module_id,
            def_id: owner,
        };
        if owner_def_id.module_id == self.input.module_id {
            self.input.signatures.functions.get(&owner).map(|_| ())?
        } else {
            self.input
                .program
                .functions()
                .get(&owner_def_id)
                .map(|_| ())?
        }
        let imported_args = args
            .iter()
            .map(|arg| self.normalize_instance_arg_type(*arg))
            .collect::<Vec<_>>();
        let (substitutions, const_substitutions) = self.generic_substitutions_and_consts_for_def(
            owner_def_id,
            &imported_args,
            &const_args,
        );
        let substitutions =
            self.intern_type_and_const_substitutions(&substitutions, &const_substitutions);
        let ty = self
            .input
            .semantic_facts
            .global_types
            .get(&def_id)
            .copied()
            .or(signature.explicit_type)
            .map(|ty| self.instantiate_ty_with_id(ty, substitutions))?;
        let init = self
            .input
            .program
            .static_init(def_id)
            .cloned()
            .map(|init| self.instantiate_static_init(init, substitutions))
            .map(|init| self.optimize_static_init(def_id, init));
        Some(BackendGlobalInstance {
            def_id,
            name: def.name,
            arg_module_id,
            args: args.clone(),
            const_args: const_args.clone(),
            symbol: self.mangle_contextual_instance_symbol(
                def_id,
                def.name,
                arg_module_id,
                None,
                &args,
                &const_args,
            ),
            ty,
            is_let: !signature.is_mutable,
            init,
            span: def.span,
        })
    }

    pub(crate) fn instantiate_static_init(
        &mut self,
        init: nia_static_ir::StaticInit,
        substitutions: TypeSubstitutionId,
    ) -> nia_static_ir::StaticInit {
        match init {
            nia_static_ir::StaticInit::Array(elems) => nia_static_ir::StaticInit::Array(
                elems
                    .into_iter()
                    .map(|elem| self.instantiate_static_init(elem, substitutions))
                    .collect(),
            ),
            nia_static_ir::StaticInit::Tuple(elems) => nia_static_ir::StaticInit::Tuple(
                elems
                    .into_iter()
                    .map(|elem| self.instantiate_static_init(elem, substitutions))
                    .collect(),
            ),
            nia_static_ir::StaticInit::Vector(elems) => nia_static_ir::StaticInit::Vector(
                elems
                    .into_iter()
                    .map(|elem| self.instantiate_static_init(elem, substitutions))
                    .collect(),
            ),
            nia_static_ir::StaticInit::Repeat { value, count } => {
                nia_static_ir::StaticInit::Repeat {
                    value: Box::new(self.instantiate_static_init(*value, substitutions)),
                    count,
                }
            }
            nia_static_ir::StaticInit::Struct(fields) => nia_static_ir::StaticInit::Struct(
                fields
                    .into_iter()
                    .map(|field| nia_static_ir::StaticFieldInit {
                        field: field.field,
                        value: self.instantiate_static_init(field.value, substitutions),
                    })
                    .collect(),
            ),
            nia_static_ir::StaticInit::AddrOfFunction {
                function,
                args,
                const_args,
            } => nia_static_ir::StaticInit::AddrOfFunction {
                function,
                args: args
                    .into_iter()
                    .map(|arg| self.instantiate_ty_with_id(arg, substitutions))
                    .collect(),
                const_args: const_args
                    .iter()
                    .map(|arg| self.instantiate_const_generic_arg(arg, substitutions))
                    .collect(),
            },
            nia_static_ir::StaticInit::Zero
            | nia_static_ir::StaticInit::Int(_)
            | nia_static_ir::StaticInit::Float(_)
            | nia_static_ir::StaticInit::Bool(_)
            | nia_static_ir::StaticInit::Char(_)
            | nia_static_ir::StaticInit::Byte(_)
            | nia_static_ir::StaticInit::Chars(_)
            | nia_static_ir::StaticInit::Bytes(_)
            | nia_static_ir::StaticInit::NullPtr
            | nia_static_ir::StaticInit::AddrOfGlobal { .. } => init,
        }
    }

    pub(crate) fn discover_backend_items_from_optional_body(
        &mut self,
        body: &Option<FunctionBody>,
    ) -> BackendItemDiscovery {
        body.as_ref()
            .map(|body| self.discover_backend_items_from_body(body))
            .unwrap_or_default()
    }

    pub(crate) fn discover_backend_items_from_body(
        &mut self,
        body: &FunctionBody,
    ) -> BackendItemDiscovery {
        BackendItemDiscovery {
            refs: body.value_refs(self.type_store),
            trait_object_vtables: self.collect_trait_object_vtables_from_concrete_body(body),
        }
    }

    pub(crate) fn append_trait_object_vtable_delta(
        &self,
        trait_object_vtables: &mut Vec<BackendTraitObjectVtable>,
        discovered: Vec<BackendTraitObjectVtable>,
        worklist: &mut ReachabilityWorklist,
    ) -> bool {
        let mut seen = trait_object_vtables
            .iter()
            .map(|vtable| vtable.key.clone())
            .collect::<HashSet<BackendTraitObjectVtableKey>>();
        let mut changed = false;
        for vtable in discovered {
            if !seen.insert(vtable.key.clone()) {
                continue;
            }
            worklist.enqueue_vtable_refs(&vtable);
            trait_object_vtables.push(vtable);
            changed = true;
        }
        changed
    }

    pub(crate) fn extend_backend_layouts_for_finalized_module(
        &mut self,
        layouts: &mut BackendLayouts,
        module: &BackendModule,
    ) {
        layout_extender::BackendLayoutExtender::new(self.input, self.type_store)
            .extend_for_finalized_module(layouts, module);
    }
}
