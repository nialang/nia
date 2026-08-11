// SPDX-License-Identifier: GPL-3.0-or-later
//! Module-lowerer construction and initial source-item materialization.

use super::*;

impl<'a> ModuleLowerer<'a> {
    pub(crate) fn new(
        input: &'a BackendLowerModuleInput<'a>,
        type_store: &'a nia_ty::TypeStore,
        optimization: OptimizationPolicy,
        shared: &'a BackendLowerShared,
        timing: bool,
    ) -> Self {
        let type_context =
            time_backend_stage(timing, "backend_lower.new_lowerer.type_context", || {
                type_context::BackendTypeContext::new(input, type_store)
            });
        let extension_generics_by_method = time_backend_stage(
            timing,
            "backend_lower.new_lowerer.local_extension_generics",
            || index_local_extension_generics_by_method(input.extensions),
        );
        let extension_method_sources_by_def = time_backend_stage(
            timing,
            "backend_lower.new_lowerer.local_extension_sources",
            || index_local_extension_method_sources_by_def(input),
        );
        let trait_context =
            time_backend_stage(timing, "backend_lower.new_lowerer.trait_context", || {
                trait_context::BackendTraitContext::new(input)
            });
        let struct_layout_instances_by_def = time_backend_stage(
            timing,
            "backend_lower.new_lowerer.struct_layout_instances",
            || index_layout_instances_by_def(input.layouts.struct_instances.keys()),
        );
        let union_layout_instances_by_def = time_backend_stage(
            timing,
            "backend_lower.new_lowerer.union_layout_instances",
            || index_layout_instances_by_def(input.layouts.union_instances.keys()),
        );
        Self {
            input,
            type_store,
            shared,
            optimization,
            type_context,
            diagnostics: Vec::new(),
            optimization_report: BackendOptimizationReport::default(),
            missing_array_len_diagnostics: HashSet::new(),
            extension_generics_by_method,
            extension_method_sources_by_def,
            trait_context,
            instantiation: instantiation_context::BackendInstantiationContext::default(),
            foreign_function_refs: Vec::new(),
            foreign_function_instance_refs: Vec::new(),
            foreign_global_instance_refs: Vec::new(),
            struct_layout_instances_by_def,
            union_layout_instances_by_def,
            effective_generics: HashMap::new(),
            def_names: HashMap::new(),
            function_sources: HashMap::new(),
            aggregate_sources: HashMap::new(),
        }
    }

    pub(crate) fn trait_impl_index_for_method(&self, def_id: GlobalDefId) -> Option<usize> {
        self.trait_context
            .trait_impls_by_method
            .get(&def_id)
            .copied()
            .or_else(|| {
                self.shared
                    .program_trait_impls_by_method
                    .get(&def_id)
                    .copied()
            })
    }

    pub(crate) fn extension_method_source(
        &self,
        def_id: GlobalDefId,
    ) -> Option<&ExtensionMethodSource> {
        self.extension_method_sources_by_def
            .get(&def_id)
            .or_else(|| {
                self.shared
                    .program_extension_method_sources_by_def
                    .get(&def_id)
            })
    }

    pub(crate) fn method_symbol_for_def(&self, def_id: GlobalDefId) -> Option<SymbolId> {
        self.trait_context
            .method_symbols_by_def
            .get(&def_id)
            .or_else(|| self.shared.program_method_symbols_by_def.get(&def_id))
            .copied()
    }

    pub(crate) fn lower_initial_module(&mut self) -> BackendModule {
        // Materialize only items owned or rooted here. Foreign references are returned to the
        // declaring module's lowerer so each emitted symbol has one authority before finalization.
        let mut structs = Vec::new();
        let mut unions = Vec::new();
        let mut struct_instances = Vec::new();
        let mut union_instances = Vec::new();
        let mut enums = Vec::new();
        let mut globals = Vec::new();
        let mut global_instances = Vec::new();
        let mut functions = Vec::new();
        let mut function_templates = Vec::new();
        let mut closure_entries = Vec::new();
        let mut worklist = ReachabilityWorklist::default();
        let mut trait_object_vtables = Vec::new();

        for item in &self.input.active_item_tree.items {
            match &item.kind {
                ItemTreeNodeKind::Struct(item_struct) => {
                    let def_id =
                        self.index_aggregate_source(&item.node_key, item.span, item_struct);
                    if !matches!(
                        self.input.roots,
                        BackendFunctionRoots::EntryPoints | BackendFunctionRoots::NoFunctions
                    ) && def_id.is_some_and(|def_id| self.is_backend_struct_reachable(def_id))
                    {
                        if item_struct.generics.is_empty()
                            && let Some(item) =
                                self.lower_struct(&item.node_key, item.span, item_struct)
                        {
                            structs.push(item);
                        }
                        struct_instances.extend(self.lower_struct_instances(
                            &item.node_key,
                            item.span,
                            item_struct,
                        ));
                    }
                }
                ItemTreeNodeKind::Union(item_union) => {
                    let def_id = self.index_union_source(&item.node_key, item.span, item_union);
                    if !matches!(
                        self.input.roots,
                        BackendFunctionRoots::EntryPoints | BackendFunctionRoots::NoFunctions
                    ) && def_id.is_some_and(|def_id| self.is_backend_union_reachable(def_id))
                    {
                        if item_union.generics.is_empty()
                            && let Some(item) =
                                self.lower_union(&item.node_key, item.span, item_union)
                        {
                            unions.push(item);
                        }
                        union_instances.extend(self.lower_union_instances(
                            &item.node_key,
                            item.span,
                            item_union,
                        ));
                    }
                }
                ItemTreeNodeKind::Trait(item_trait) => {
                    for method in &item_trait.methods {
                        if method.function.body.is_none() {
                            continue;
                        }
                        self.index_function_source(
                            method.function.span,
                            &method.function,
                            &mut worklist,
                        );
                        self.lower_function_local_static_globals(
                            &method.function,
                            &mut globals,
                            &mut worklist,
                        );
                    }
                }
                ItemTreeNodeKind::Extend(extend) => {
                    for method in &extend.methods {
                        if method.function.body.is_none() {
                            continue;
                        }
                        self.index_function_source(
                            method.function.span,
                            &method.function,
                            &mut worklist,
                        );
                        self.lower_function_local_static_globals(
                            &method.function,
                            &mut globals,
                            &mut worklist,
                        );
                    }
                }
                ItemTreeNodeKind::Enum(item_enum) => {
                    if let Some(item) = self.lower_enum(&item.node_key, item.span, item_enum) {
                        enums.push(item);
                    }
                }
                ItemTreeNodeKind::Function(function) => {
                    self.index_function_source(item.span, function, &mut worklist);
                    self.lower_function_local_static_globals(function, &mut globals, &mut worklist);
                }
                ItemTreeNodeKind::Binding(binding) => {
                    if binding.is_const() {
                        continue;
                    }
                    self.lower_static_global_binding(
                        item.span,
                        binding,
                        &mut globals,
                        &mut worklist,
                    );
                }
                ItemTreeNodeKind::Module(_)
                | ItemTreeNodeKind::Using(_)
                | ItemTreeNodeKind::TypeAlias(_) => {}
            }
        }

        worklist.enqueue_instances(self.initial_planned_function_instance_refs());
        self.lower_reachable_function_closure(
            &mut functions,
            &mut worklist,
            &mut trait_object_vtables,
        );
        self.lower_missing_static_globals(&mut globals, &mut worklist);
        let mut function_instances = Vec::new();
        self.lower_reachable_instances_and_vtables(
            &mut functions,
            &mut function_templates,
            (&mut function_instances, &mut closure_entries),
            &mut global_instances,
            &mut worklist,
            &mut trait_object_vtables,
        );
        closure_entries.extend(self.lower_source_closure_entries(&functions));
        closure_entries.sort_unstable_by(|left, right| left.symbol.cmp(&right.symbol));
        BackendModule {
            id: self.input.module_id,
            source_identity: self.input.source_identity.clone(),
            name: self.input.module_name.clone(),
            const_eval: nia_backend_ir::BackendConstFacts {
                array_lengths: self.input.const_array_lengths.clone(),
            },
            layouts: BackendLayouts::from_module_layouts(self.input.module_id, self.input.layouts),
            structs,
            unions,
            struct_instances,
            union_instances,
            enums,
            globals,
            global_instances,
            functions,
            function_instances,
            closure_entries,
            trait_object_vtables,
            generic_instantiations: self
                .input
                .semantic_facts
                .iter_generic_instantiations()
                .map(|inst| nia_backend_ir::BackendGenericInstantiation {
                    def_id: inst.def_id,
                    arg_module_id: self.input.module_id,
                    self_arg: inst.self_arg,
                    args: inst.args.clone(),
                    const_args: inst.const_args.clone(),
                    span: inst.span,
                    source_def_id: inst.source_def_id,
                })
                .collect(),
        }
    }

    pub(crate) fn lower_source_closure_entries(
        &mut self,
        functions: &[BackendFunction],
    ) -> Vec<BackendClosureEntry> {
        let mut lowered = Vec::new();
        for function in functions {
            if !function.generics.is_empty() || function.function_body.is_none() {
                continue;
            }
            let owner_symbol =
                self.mangle_instance_symbol(function.def_id, function.name, None, &[], &[]);
            for entry in self.input.program.closure_entries(function.def_id) {
                let state_local = entry
                    .body
                    .locals
                    .iter()
                    .find(|local| local.id == entry.state_param)
                    .unwrap_or_else(|| {
                        panic!(
                            "Nia ICE: closure entry {:?} is missing state parameter {:?}",
                            entry.closure_id, entry.state_param
                        )
                    });
                let params = entry
                    .params
                    .iter()
                    .map(|param| {
                        entry
                            .body
                            .locals
                            .iter()
                            .find(|local| local.id == *param)
                            .unwrap_or_else(|| {
                                panic!(
                                    "Nia ICE: closure entry {:?} is missing parameter {:?}",
                                    entry.closure_id, param
                                )
                            })
                            .ty
                    })
                    .collect();
                lowered.push(BackendClosureEntry {
                    key: BackendClosureEntryKey {
                        closure_id: entry.closure_id,
                        owner: BackendClosureEntryOwner::Source(function.def_id),
                    },
                    symbol: mangle_closure_entry_symbol(&owner_symbol, entry.closure_id),
                    abi: BackendClosureEntryAbi {
                        state_type: entry.state_ty,
                        state_pointer_type: state_local.ty,
                        params,
                        return_type: entry.return_type,
                    },
                    state_param: entry.state_param,
                    params: entry.params.clone(),
                    local_names: self.function_local_names(&entry.body),
                    function_body: entry.body.clone(),
                    span: entry.body.span,
                });
            }
        }
        lowered
    }
}
