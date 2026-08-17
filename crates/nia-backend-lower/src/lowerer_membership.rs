// SPDX-License-Identifier: GPL-3.0-or-later
//! Final definition membership, static globals, and reachable aggregate closure.

use super::*;

impl<'a> ModuleLowerer<'a> {
    pub(crate) fn finish_module(&mut self, module: &mut BackendModule) {
        self.devirtualize_direct_trait_calls(&mut module.functions, &mut module.function_instances);
        self.propagate_cross_function_constants(
            &mut module.functions,
            &mut module.function_instances,
        );
        self.inline_leaf_functions(&mut module.functions, &mut module.function_instances);
        self.remove_unused_private_functions(
            &mut module.functions,
            &mut module.function_instances,
            &mut module.closure_entries,
            &module.globals,
            &module.global_instances,
            &module.trait_object_vtables,
        );
        let mut layouts =
            BackendLayouts::from_module_layouts(self.input.module_id, self.input.layouts);
        self.extend_backend_layouts_for_finalized_module(&mut layouts, module);
        module.layouts = layouts;
        module
            .closure_entries
            .sort_unstable_by(|left, right| left.symbol.cmp(&right.symbol));
    }

    pub(crate) fn complete_definition_membership(&mut self, module: &mut BackendModule) {
        self.extend_struct_instances_from_functions(
            &mut module.struct_instances,
            &mut module.union_instances,
            &module.functions,
            &module.function_instances,
        );
        self.complete_reachable_aggregates(
            &mut module.structs,
            &mut module.unions,
            ReachableAggregateInputs {
                globals: &module.globals,
                functions: &module.functions,
                function_instances: &module.function_instances,
                closure_entries: &module.closure_entries,
                struct_instances: &module.struct_instances,
                union_instances: &module.union_instances,
                trait_object_vtables: &module.trait_object_vtables,
            },
        );
    }

    pub(crate) fn lower_function_local_static_globals(
        &mut self,
        function: &nia_ast::FunctionItem,
        globals: &mut Vec<BackendGlobal>,
        worklist: &mut ReachabilityWorklist,
    ) {
        let Some(body) = &function.body else {
            return;
        };
        let owner_has_effective_generics = self
            .def_id_for_node_any_function(&function.node_key)
            .map(|def_id| {
                let global_def_id = self.global_def_id(def_id);
                !self
                    .effective_generics(global_def_id, &generic_param_names(&function.generics))
                    .is_empty()
            })
            .unwrap_or(false);
        self.lower_block_static_globals(body, owner_has_effective_generics, globals, worklist);
    }

    pub(crate) fn lower_block_static_globals(
        &mut self,
        block: &Block,
        owner_has_effective_generics: bool,
        globals: &mut Vec<BackendGlobal>,
        worklist: &mut ReachabilityWorklist,
    ) {
        nia_ast_walk::walk_static_bindings(block, &mut |stmt| {
            let StmtKind::Static(binding) = &stmt.kind else {
                return;
            };
            self.lower_local_static_global_binding(
                stmt.span,
                binding,
                owner_has_effective_generics,
                globals,
                worklist,
            );
        });
    }

    pub(crate) fn lower_local_static_global_binding(
        &mut self,
        span: nia_span::Span,
        binding: &BindingItem,
        owner_has_effective_generics: bool,
        globals: &mut Vec<BackendGlobal>,
        worklist: &mut ReachabilityWorklist,
    ) {
        let Some(global_def_id) = self
            .def_id_for_node(&binding.node_key, DefKind::Global)
            .map(|def_id| self.global_def_id(def_id))
        else {
            return;
        };
        if owner_has_effective_generics && self.input.program.static_init(global_def_id).is_none() {
            return;
        }
        self.lower_static_global_binding(span, binding, globals, worklist);
    }

    pub(crate) fn lower_missing_static_globals(
        &mut self,
        globals: &mut Vec<BackendGlobal>,
        worklist: &mut ReachabilityWorklist,
    ) {
        let mut seen = globals
            .iter()
            .map(|global| global.def_id)
            .collect::<HashSet<_>>();
        let mut pending = self.input.program.static_init_ids().to_vec();
        pending.sort_by_key(|def_id| def_id.def_id);
        for global_def_id in pending {
            if global_def_id.module_id != self.input.module_id || !seen.insert(global_def_id) {
                continue;
            }
            let Some(global) = self.lower_global_from_static_init(global_def_id) else {
                continue;
            };
            if let Some(init) = &global.init {
                let mut refs = FunctionBodyRefs::default();
                refs.extend(init.value_refs(self.input.module_id));
                worklist.enqueue_refs(refs);
            }
            globals.push(global);
        }
    }

    pub(crate) fn lower_static_global_binding(
        &mut self,
        span: nia_span::Span,
        binding: &BindingItem,
        globals: &mut Vec<BackendGlobal>,
        worklist: &mut ReachabilityWorklist,
    ) {
        let Some(global_def_id) = self
            .def_id_for_node(&binding.node_key, DefKind::Global)
            .map(|def_id| self.global_def_id(def_id))
        else {
            return;
        };
        if !self.is_backend_global_reachable(global_def_id) {
            return;
        }
        if let Some(global) = self.lower_global(&binding.node_key, span, binding) {
            if let Some(init) = &global.init {
                let mut refs = FunctionBodyRefs::default();
                refs.extend(init.value_refs(self.input.module_id));
                worklist.enqueue_refs(refs);
            }
            globals.push(global);
        }
    }

    pub(crate) fn index_function_source(
        &mut self,
        span: nia_span::Span,
        function: &'a nia_ast::FunctionItem,
        worklist: &mut ReachabilityWorklist,
    ) -> Option<GlobalDefId> {
        let def_id = self.def_id_for_node_any_function(&function.node_key)?;
        let global_def_id = self.global_def_id(def_id);
        self.function_sources
            .insert(global_def_id, BackendFunctionSource { span, function });
        if self.is_backend_function_root(global_def_id, function) {
            worklist.enqueue_function(global_def_id);
        }
        Some(global_def_id)
    }

    pub(crate) fn index_aggregate_source(
        &mut self,
        node_key: &'a VersionedNodeKey,
        span: nia_span::Span,
        item: &'a nia_ast::StructItem,
    ) -> Option<GlobalDefId> {
        let def_id = self.def_id_for_node(node_key, DefKind::Struct)?;
        let global_def_id = self.global_def_id(def_id);
        self.aggregate_sources.insert(
            global_def_id,
            BackendAggregateSource::Struct {
                node_key,
                span,
                item,
            },
        );
        Some(global_def_id)
    }

    pub(crate) fn index_union_source(
        &mut self,
        node_key: &'a VersionedNodeKey,
        span: nia_span::Span,
        item: &'a nia_ast::UnionItem,
    ) -> Option<GlobalDefId> {
        let def_id = self.def_id_for_node(node_key, DefKind::Union)?;
        let global_def_id = self.global_def_id(def_id);
        self.aggregate_sources.insert(
            global_def_id,
            BackendAggregateSource::Union {
                node_key,
                span,
                item,
            },
        );
        Some(global_def_id)
    }

    pub(crate) fn is_backend_function_root(
        &mut self,
        def_id: GlobalDefId,
        function: &nia_ast::FunctionItem,
    ) -> bool {
        if self.input.roots == BackendFunctionRoots::NoFunctions {
            return false;
        }
        let Some(def) = self.input.defs.defs.get(def_id.def_id) else {
            return false;
        };
        if def.kind == DefKind::TraitMethod {
            return false;
        }
        if self.input.roots == BackendFunctionRoots::FunctionBodies {
            return function.is_extern
                || (self.input.program.function_body(def_id).is_some()
                    && !self
                        .has_effective_generics(def_id, &generic_param_names(&function.generics)));
        }
        if function.is_extern {
            return true;
        }
        if self.input.roots == BackendFunctionRoots::EntryPoints {
            return self
                .input
                .reachable_functions
                .is_some_and(|functions| functions.binary_search(&def_id).is_ok());
        }
        def.name == known::MAIN
            || def.name == known::START_ENTRY
            || def.visibility != Visibility::Private
    }

    /// Returns whether initial lowering already selected a foreign source body.
    ///
    /// Having a checked body is sufficient only when every body is a root. In
    /// executable mode the frontend plan is intentionally narrower, so a
    /// vtable discovered by backend substitution may still need to route its
    /// implementation back to the defining module.
    pub(crate) fn foreign_source_function_is_preplanned(&self, def_id: GlobalDefId) -> bool {
        source_function_is_preplanned(
            self.input.roots,
            self.input.reachable_functions,
            def_id,
            self.input.program.function_body(def_id).is_some(),
        )
    }

    pub(crate) fn is_backend_global_reachable(&self, def_id: GlobalDefId) -> bool {
        if self.input.roots == BackendFunctionRoots::NoFunctions {
            return false;
        }
        match self.input.reachable_globals {
            Some(globals) if self.input.roots == BackendFunctionRoots::EntryPoints => {
                globals.binary_search(&def_id).is_ok()
            }
            _ => true,
        }
    }

    pub(crate) fn is_backend_struct_reachable(&self, def_id: GlobalDefId) -> bool {
        match self.input.reachable_structs {
            Some(structs) => structs.binary_search(&def_id).is_ok(),
            _ => true,
        }
    }

    pub(crate) fn is_backend_union_reachable(&self, def_id: GlobalDefId) -> bool {
        match self.input.reachable_unions {
            Some(unions) => unions.binary_search(&def_id).is_ok(),
            _ => true,
        }
    }

    pub(crate) fn complete_reachable_aggregates(
        &mut self,
        structs: &mut Vec<BackendStruct>,
        unions: &mut Vec<BackendUnion>,
        input: ReachableAggregateInputs<'_>,
    ) {
        if !matches!(
            self.input.roots,
            BackendFunctionRoots::EntryPoints | BackendFunctionRoots::NoFunctions
        ) {
            return;
        }
        let mut roots = ReachableAggregateRoots::default();
        for global in input.globals {
            roots.add_ty(self, global.ty);
            if let Some(init) = &global.init {
                roots.add_static_init(self, init);
            }
        }
        for function in input.functions {
            roots.add_backend_function(self, function);
        }
        for instance in input.function_instances {
            roots.add_backend_function_instance(self, instance);
        }
        for entry in input.closure_entries {
            roots.add_backend_closure_entry(self, entry);
        }
        for instance in input.struct_instances {
            roots.add_struct(instance.def_id);
            for arg in &instance.args {
                roots.add_ty(self, *arg);
            }
            for field in &instance.fields {
                roots.add_ty(self, field.ty);
            }
        }
        for instance in input.union_instances {
            roots.add_union(instance.def_id);
            for arg in &instance.args {
                roots.add_ty(self, *arg);
            }
            for field in &instance.fields {
                roots.add_ty(self, field.ty);
            }
        }
        for vtable in input.trait_object_vtables {
            roots.add_ty(self, vtable.key.self_ty);
            roots.add_ty(self, vtable.key.object_ty);
            for arg in &vtable.trait_args {
                roots.add_ty(self, *arg);
            }
            for entry in &vtable.entries {
                if let BackendTraitObjectVtableFunction::FunctionInstance { args, .. } =
                    &entry.function
                {
                    for arg in args {
                        roots.add_ty(self, *arg);
                    }
                }
            }
        }
        if let Some(reachable_structs) = self.input.reachable_structs {
            for def_id in reachable_structs {
                roots.add_struct(*def_id);
            }
        }
        if let Some(reachable_unions) = self.input.reachable_unions {
            for def_id in reachable_unions {
                roots.add_union(*def_id);
            }
        }

        let mut seen_structs = structs
            .iter()
            .map(|item| item.def_id)
            .collect::<HashSet<_>>();
        for def_id in roots.structs {
            if def_id.module_id != self.input.module_id
                || !self.is_backend_struct_reachable(def_id)
                || !seen_structs.insert(def_id)
            {
                continue;
            }
            let Some(BackendAggregateSource::Struct {
                node_key,
                span,
                item,
            }) = self.aggregate_sources.get(&def_id).copied()
            else {
                continue;
            };
            if item.generics.is_empty()
                && let Some(item) = self.lower_struct(node_key, span, item)
            {
                structs.push(item);
            }
        }

        let mut seen_unions = unions
            .iter()
            .map(|item| item.def_id)
            .collect::<HashSet<_>>();
        for def_id in roots.unions {
            if def_id.module_id != self.input.module_id
                || !self.is_backend_union_reachable(def_id)
                || !seen_unions.insert(def_id)
            {
                continue;
            }
            let Some(BackendAggregateSource::Union {
                node_key,
                span,
                item,
            }) = self.aggregate_sources.get(&def_id).copied()
            else {
                continue;
            };
            if item.generics.is_empty()
                && let Some(item) = self.lower_union(node_key, span, item)
            {
                unions.push(item);
            }
        }
    }

    pub(crate) fn has_effective_generics(
        &mut self,
        def_id: GlobalDefId,
        own_generics: &[SymbolId],
    ) -> bool {
        !self.effective_generics(def_id, own_generics).is_empty()
    }
}

fn source_function_is_preplanned(
    roots: BackendFunctionRoots,
    reachable_functions: Option<&[GlobalDefId]>,
    def_id: GlobalDefId,
    has_body: bool,
) -> bool {
    if !has_body {
        return false;
    }
    match roots {
        BackendFunctionRoots::FunctionBodies => true,
        BackendFunctionRoots::EntryPoints => {
            reachable_functions.is_some_and(|functions| functions.binary_search(&def_id).is_ok())
        }
        BackendFunctionRoots::Public | BackendFunctionRoots::NoFunctions => false,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use nia_ids::{DefId, ModuleIdAllocator};

    #[test]
    fn executable_preplanning_requires_exact_reachability_membership() {
        let module_id = ModuleIdAllocator::new().allocate();
        let selected = GlobalDefId {
            module_id,
            def_id: DefId(1),
        };
        let late = GlobalDefId {
            module_id,
            def_id: DefId(2),
        };
        let reachable = [selected];

        assert!(source_function_is_preplanned(
            BackendFunctionRoots::EntryPoints,
            Some(&reachable),
            selected,
            true,
        ));
        assert!(!source_function_is_preplanned(
            BackendFunctionRoots::EntryPoints,
            Some(&reachable),
            late,
            true,
        ));
        assert!(source_function_is_preplanned(
            BackendFunctionRoots::FunctionBodies,
            None,
            late,
            true,
        ));
        assert!(!source_function_is_preplanned(
            BackendFunctionRoots::FunctionBodies,
            None,
            late,
            false,
        ));
    }
}
