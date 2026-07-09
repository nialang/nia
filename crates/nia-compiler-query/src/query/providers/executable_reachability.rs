// SPDX-License-Identifier: GPL-3.0-or-later
use super::*;

pub(in crate::query) fn provide_executable_checked_module_set(
    db: &QueryDb<CompilerContext>,
) -> ExecutableCheckedModuleSet {
    time_provider(
        db.context().timings(),
        "executable_checked_module_set",
        || executable_checked_module_set_inner(db),
    )
}

#[cfg(test)]
pub(in crate::query) fn provide_executable_checked_modules(
    db: &QueryDb<CompilerContext>,
) -> Vec<CheckedModule> {
    time_provider(db.context().timings(), "executable_checked_modules", || {
        let set = db.query(ExecutableCheckedModuleSetQuery);
        db.context().executable_checked_modules(&set)
    })
}

struct QueryExecutableExtensionLookup<'a> {
    db: &'a QueryDb<CompilerContext>,
    trait_impls_by_trait:
        RefCell<HashMap<nia_ty::TraitId, Vec<nia_item_signatures::ProgramTraitImplSignature>>>,
    module_ids_by_trait: RefCell<HashMap<nia_ty::TraitId, Vec<ModuleId>>>,
    methods_by_trait: RefCell<HashMap<nia_ty::TraitId, Vec<nia_defs::ExtensionMethod>>>,
    methods_by_trait_name:
        RefCell<HashMap<(nia_ty::TraitId, SymbolId), Vec<nia_defs::ExtensionMethod>>>,
}

impl QueryExecutableExtensionLookup<'_> {
    fn new(db: &QueryDb<CompilerContext>) -> QueryExecutableExtensionLookup<'_> {
        QueryExecutableExtensionLookup {
            db,
            trait_impls_by_trait: RefCell::new(HashMap::new()),
            module_ids_by_trait: RefCell::new(HashMap::new()),
            methods_by_trait: RefCell::new(HashMap::new()),
            methods_by_trait_name: RefCell::new(HashMap::new()),
        }
    }

    fn ensure_trait_impls_for_trait(&self, trait_id: nia_ty::TraitId) {
        if self.trait_impls_by_trait.borrow().contains_key(&trait_id) {
            return;
        }
        let trait_impls = self
            .db
            .query(ExtensionTraitImplsForTraitQuery(trait_id))
            .trait_impls
            .clone();
        self.trait_impls_by_trait
            .borrow_mut()
            .insert(trait_id, trait_impls);
    }

    fn with_trait_impls_for_trait(
        &self,
        trait_id: nia_ty::TraitId,
        f: &mut dyn FnMut(&[nia_item_signatures::ProgramTraitImplSignature]),
    ) {
        self.ensure_trait_impls_for_trait(trait_id);
        let trait_impls = self.trait_impls_by_trait.borrow();
        let trait_impls = trait_impls.get(&trait_id).map(Vec::as_slice).unwrap_or(&[]);
        f(trait_impls);
    }

    fn module_ids_for_trait(&self, trait_id: nia_ty::TraitId) -> Vec<ModuleId> {
        if let Some(modules) = self.module_ids_by_trait.borrow().get(&trait_id) {
            return modules.clone();
        }
        let mut modules = Vec::new();
        self.with_trait_impls_for_trait(trait_id, &mut |trait_impls| {
            modules.extend(
                trait_impls
                    .iter()
                    .map(|impl_signature| impl_signature.module_id),
            );
        });
        modules.sort();
        modules.dedup();
        self.module_ids_by_trait
            .borrow_mut()
            .insert(trait_id, modules.clone());
        modules
    }
}

impl ExecutableExtensionLookup for QueryExecutableExtensionLookup<'_> {
    fn for_each_method_for_trait(
        &self,
        trait_id: nia_ty::TraitId,
        f: &mut dyn FnMut(&nia_defs::ExtensionMethod),
    ) {
        if self.methods_by_trait.borrow().contains_key(&trait_id) {
            let methods = self.methods_by_trait.borrow();
            if let Some(methods) = methods.get(&trait_id) {
                for method in methods {
                    f(method);
                }
            }
            return;
        }
        let mut seen = HashSet::new();
        let mut methods = Vec::new();
        for facts in self.db.query_many(
            self.module_ids_for_trait(trait_id)
                .into_iter()
                .map(ExtensionProviderModuleFactsQuery),
        ) {
            methods.extend(
                facts
                    .methods
                    .all_methods()
                    .filter(|method| method.trait_id == Some(trait_id))
                    .filter(|method| seen.insert(method.def_id))
                    .cloned(),
            );
        }
        self.methods_by_trait.borrow_mut().insert(trait_id, methods);
        let methods = self.methods_by_trait.borrow();
        if let Some(methods) = methods.get(&trait_id) {
            for method in methods {
                f(method);
            }
        }
    }

    fn for_each_method_for_trait_method(
        &self,
        trait_id: nia_ty::TraitId,
        method_name: &SymbolId,
        f: &mut dyn FnMut(&nia_defs::ExtensionMethod),
    ) {
        let key = (trait_id, *method_name);
        if self.methods_by_trait_name.borrow().contains_key(&key) {
            let methods = self.methods_by_trait_name.borrow();
            if let Some(methods) = methods.get(&key) {
                for method in methods {
                    f(method);
                }
            }
            return;
        }
        let mut seen = HashSet::new();
        let mut methods = Vec::new();
        for facts in self.db.query_many(
            self.module_ids_for_trait(trait_id)
                .into_iter()
                .map(ExtensionProviderModuleFactsQuery),
        ) {
            methods.extend(
                facts
                    .methods
                    .methods_named(method_name)
                    .filter(|method| method.trait_id == Some(trait_id))
                    .filter(|method| seen.insert(method.def_id))
                    .cloned(),
            );
        }
        self.methods_by_trait_name.borrow_mut().insert(key, methods);
        let methods = self.methods_by_trait_name.borrow();
        if let Some(methods) = methods.get(&key) {
            for method in methods {
                f(method);
            }
        }
    }

    fn with_where_predicates_for_def(
        &self,
        def_id: GlobalDefId,
        f: &mut dyn FnMut(&[nia_defs::WherePredicateSignature]),
    ) {
        let method = self.db.query(ExtensionMethodByIdQuery(def_id));
        let predicates = method
            .method
            .as_ref()
            .map(|method| method.where_predicates.as_slice())
            .unwrap_or(&[]);
        f(predicates);
    }

    fn with_trait_impl_for_method(
        &self,
        method: &nia_defs::ExtensionMethod,
        trait_id: nia_ty::TraitId,
        f: &mut dyn FnMut(&nia_item_signatures::ProgramTraitImplSignature),
    ) -> bool {
        let mut found = false;
        self.with_trait_impls_for_trait(trait_id, &mut |trait_impls| {
            let Some(signature) = trait_impls.iter().find(|impl_signature| {
                impl_signature.module_id == method.def_id.module_id
                    && impl_signature.impl_id == method.impl_id
            }) else {
                return;
            };
            found = true;
            f(signature);
        });
        found
    }
}

struct ExecutableBodyCheckBatchItem {
    module_id: ModuleId,
    checked_functions: HashSet<GlobalDefId>,
}

fn executable_checked_module_set_inner(
    db: &QueryDb<CompilerContext>,
) -> ExecutableCheckedModuleSet {
    let parse_ok = db.query(SemanticModuleIdsQuery);
    let graph = db.query_shared(ModuleGraphQuery);
    let mut non_function_signatures = None::<ProgramExecutableNonFunctionSignatures>;
    let caches = ExecutableCheckCaches::default();
    let function_signature = |def_id: GlobalDefId| {
        if let Some(signature) = caches
            .reachability_function_signatures
            .borrow()
            .get(&def_id)
            .cloned()
        {
            return Some(signature);
        }
        let signatures = db.query_shared(SignatureItemSignaturesQuery(
            def_id.module_id,
            nia_item_tree::SignatureItemSet::Functions,
        ));
        let lowering = db.query_shared(SignatureTypeLoweringQuery(
            def_id.module_id,
            nia_item_tree::SignatureItemSet::Functions,
        ));
        let signature = signatures
            .functions
            .get(&def_id.def_id)
            .cloned()
            .map(|signature| ProgramFunctionSignature {
                name: db
                    .query(ModuleDefsQuery(def_id.module_id))
                    .defs
                    .get(def_id.def_id)
                    .map(|def| def.name.clone())
                    .unwrap_or_default(),
                signature,
                interner: lowering.interner.clone(),
            })?;
        let signature = Arc::new(signature);
        caches
            .reachability_function_signatures
            .borrow_mut()
            .insert(def_id, signature.clone());
        Some(signature)
    };
    let struct_signature = |def_id: GlobalDefId| {
        db.query_shared(SignatureItemSignaturesQuery(
            def_id.module_id,
            nia_item_tree::SignatureItemSet::Types,
        ))
        .structs
        .get(&def_id.def_id)
        .cloned()
        .map(|signature| ProgramStructSignature {
            signature,
            interner: signature_type_interner(
                db,
                def_id.module_id,
                nia_item_tree::SignatureItemSet::Types,
            ),
        })
    };
    let union_signature = |def_id: GlobalDefId| {
        db.query_shared(SignatureItemSignaturesQuery(
            def_id.module_id,
            nia_item_tree::SignatureItemSet::Types,
        ))
        .unions
        .get(&def_id.def_id)
        .cloned()
        .map(|signature| ProgramUnionSignature {
            signature,
            interner: signature_type_interner(
                db,
                def_id.module_id,
                nia_item_tree::SignatureItemSet::Types,
            ),
        })
    };
    let trait_signature = |def_id: GlobalDefId| {
        db.query_shared(SignatureItemSignaturesQuery(
            def_id.module_id,
            nia_item_tree::SignatureItemSet::Traits,
        ))
        .traits
        .get(&def_id.def_id)
        .cloned()
        .map(|signature| ProgramTraitSignature {
            signature,
            interner: signature_type_interner(
                db,
                def_id.module_id,
                nia_item_tree::SignatureItemSet::Traits,
            ),
        })
    };
    let trait_default_method = |def_id: GlobalDefId| {
        let signatures = db.query_shared(SignatureItemSignaturesQuery(
            def_id.module_id,
            nia_item_tree::SignatureItemSet::Traits,
        ));
        signatures
            .traits
            .iter()
            .find_map(|(trait_def_id, signature)| {
                signature
                    .methods
                    .iter()
                    .any(|method| method.def_id == def_id.def_id && method.has_default)
                    .then(|| {
                        (
                            GlobalDefId {
                                module_id: def_id.module_id,
                                def_id: *trait_def_id,
                            },
                            ProgramTraitSignature {
                                signature: signature.clone(),
                                interner: signature_type_interner(
                                    db,
                                    def_id.module_id,
                                    nia_item_tree::SignatureItemSet::Traits,
                                ),
                            },
                        )
                    })
            })
    };
    let named_function = |module_id, name: SymbolId| {
        let defs = db.query_shared(FullModuleDefsQuery(module_id));
        defs.defs.iter().find_map(|(def_id, def)| {
            (def.kind == DefKind::Function && def.name == name)
                .then_some(GlobalDefId { module_id, def_id })
        })
    };
    let module_functions = |module_id| {
        let defs = db.query_shared(FullModuleDefsQuery(module_id));
        defs.defs
            .iter()
            .filter_map(|(def_id, def)| {
                (def.kind == DefKind::Function).then_some(GlobalDefId { module_id, def_id })
            })
            .collect::<Vec<_>>()
    };
    let mut fact_by_id = HashMap::<ModuleId, ExecutableFactModuleState>::new();
    let comptime_module_cache = RefCell::new(HashMap::<ModuleId, ComptimeModuleLowering>::new());
    let mut reachability_state = IncrementalExecutableReachability::default();
    let extension_lookup = QueryExecutableExtensionLookup::new(db);
    loop {
        let reachable_inputs = time_provider(
            db.context().timings(),
            "executable_checked_modules.inputs",
            || reachable_fact_module_inputs(&fact_by_id),
        );
        time_provider(
            db.context().timings(),
            "executable_checked_modules.reachability_compute",
            || {
                compute_executable_reachability_incremental_with_timings(
                    &mut reachability_state,
                    &parse_ok,
                    &graph,
                    ExecutableRootDefs {
                        named_function: &named_function,
                        module_functions: &module_functions,
                    },
                    nia_executable_reachability::ExecutableSignatureIndex {
                        function: &function_signature,
                        struct_: &struct_signature,
                        union: &union_signature,
                        trait_: &trait_signature,
                        trait_default_method: &trait_default_method,
                    },
                    &extension_lookup,
                    &reachable_inputs,
                    db.context().timings(),
                )
            },
        );
        let reachability_by_module = reachability_state.reachability().by_module();
        let value_edges_changed = time_provider(
            db.context().timings(),
            "executable_checked_modules.value_ref_edges",
            || {
                let reachability = reachability_state.reachability_mut();
                extend_reachability_from_value_ref_edges(
                    db,
                    &parse_ok,
                    reachability,
                    &reachability_by_module,
                    &function_signature,
                    &fact_by_id,
                )
            },
        );
        if value_edges_changed {
            continue;
        }
        let stale = time_provider(
            db.context().timings(),
            "executable_checked_modules.stale_select",
            || {
                stale_executable_fact_modules(
                    db,
                    &parse_ok,
                    reachability_state.reachability(),
                    &reachability_by_module,
                    &fact_by_id,
                )
            },
        );
        if stale.is_empty() {
            break;
        }
        let round_reachable_body_modules =
            executable_reachable_body_modules(db, &reachability_by_module);
        let mut batch_items = Vec::new();
        for module_id in stale {
            let already_checked_functions = fact_by_id
                .get(&module_id)
                .map(|state| &state.checked_functions);
            let already_checked_globals = fact_by_id
                .get(&module_id)
                .map(|state| &state.checked_globals);
            let (module_functions, module_globals) =
                unchecked_executable_items(&reachability_by_module, module_id, &fact_by_id);
            let module_functions = time_module_provider(
                db,
                "executable_checked_modules.extend_local_static_owners",
                module_id,
                || {
                    extend_module_functions_from_local_static_globals(
                        db,
                        module_id,
                        module_functions,
                        &module_globals,
                        already_checked_functions,
                    )
                },
            );
            let module_functions = time_module_provider(
                db,
                "executable_checked_modules.extend_value_refs",
                module_id,
                || {
                    extend_module_functions_from_filtered_value_refs(
                        db,
                        module_id,
                        module_functions,
                        &module_globals,
                        already_checked_functions,
                    )
                },
            );
            reachability_state
                .reachability_mut()
                .insert_functions(module_functions.iter().copied());
            let filter = nia_body_check::BodyCheckFilter::ReachableItems {
                functions: &module_functions,
                globals: &module_globals,
                already_checked_functions: already_checked_functions,
                already_checked_globals: already_checked_globals,
            };
            let mut reachable_body_modules = round_reachable_body_modules.clone();
            if !module_functions.is_empty()
                || module_globals.iter().any(|def_id| {
                    db.query_shared(ModuleDefsQuery(def_id.module_id))
                        .defs
                        .get(def_id.def_id)
                        .is_some_and(|def| def.kind == DefKind::Global)
                })
            {
                reachable_body_modules.insert(module_id);
            }
            let layouts = {
                let reachability = reachability_state.reachability();
                executable_layouts_for_reachable_items(
                    db,
                    module_id,
                    reachability.functions(),
                    reachability.globals(),
                    Some(&caches.array_lengths),
                    None,
                    Some(&reachable_body_modules),
                )
            };
            let seed_interner = fact_by_id
                .get(&module_id)
                .map(|state| state.body_ir.interner.clone());
            let body_check = {
                let resolution_inputs = {
                    let cached = caches
                        .body_resolution_inputs
                        .borrow()
                        .get(&module_id)
                        .cloned();
                    cached.unwrap_or_else(|| {
                        let inputs = time_module_provider(
                            db,
                            "executable_checked_modules.full_body_inputs",
                            module_id,
                            || full_body_check_resolution_inputs(db, module_id),
                        );
                        caches
                            .body_resolution_inputs
                            .borrow_mut()
                            .insert(module_id, inputs.clone());
                        inputs
                    })
                };
                let program_layout_cache = RefCell::new(HashMap::new());
                program_layout_cache
                    .borrow_mut()
                    .insert(module_id, layouts.clone());
                let executable_program_layouts = {
                    let reachability = reachability_state.reachability();
                    executable_program_layouts(
                        db,
                        &program_layout_cache,
                        reachability.functions(),
                        reachability.globals(),
                        Some(&caches.array_lengths),
                        None,
                        Some(&reachable_body_modules),
                    )
                };
                time_module_provider(db, "executable_fact_check", module_id, || {
                    body_check_with_filter_and_layouts_with_inputs_and_product(
                        db,
                        module_id,
                        filter,
                        Some(layouts.clone()),
                        Some(&executable_program_layouts),
                        ExecutableFactMode::executable(&reachable_body_modules),
                        Some(resolution_inputs),
                        seed_interner,
                        Some(&caches.global_initializers),
                        Some(&comptime_module_cache),
                        Some(&caches.body_function_signatures),
                        nia_body_check::BodyCheckProduct::FactsOnly,
                    )
                })
            };
            let checked_this_round = body_check.body_check.checked_functions.clone();
            let new_globals_len = module_globals.len();
            let module_path = db.query(ModulePathQuery(module_id));
            time_module_provider(
                db,
                "executable_checked_modules.fact_merge",
                module_id,
                || match fact_by_id.get_mut(&module_id) {
                    Some(state) => state.extend(body_check, module_globals),
                    None => {
                        fact_by_id.insert(
                            module_id,
                            ExecutableFactModuleState::new(
                                db,
                                module_id,
                                body_check,
                                module_globals,
                            ),
                        );
                    }
                },
            );
            if let Some(state) = fact_by_id.get(&module_id) {
                let reachability = reachability_state.reachability();
                print_executable_round_debug(ExecutableRoundDebug {
                    module_id,
                    module_path: &module_path,
                    requested_function_names: executable_debug_function_names(
                        db,
                        module_id,
                        &module_functions,
                    ),
                    checked_function_names: executable_debug_function_names(
                        db,
                        module_id,
                        &checked_this_round,
                    ),
                    requested_functions: module_functions.len(),
                    new_functions: checked_this_round.len(),
                    new_globals: new_globals_len,
                    checked_functions_total: state.checked_functions.len(),
                    checked_globals_total: state.checked_globals.len(),
                    reachable_functions_total: reachability.functions().len(),
                    reachable_globals_total: reachability.globals().len(),
                    reachable_modules_total: reachability.modules().len(),
                    type_modules_total: reachability.type_modules().len(),
                });
            }
            batch_items.push(ExecutableBodyCheckBatchItem {
                module_id,
                checked_functions: checked_this_round,
            });
        }
        let checked_inputs = time_provider(
            db.context().timings(),
            "executable_checked_modules.batch_inputs",
            || reachable_fact_module_inputs(&fact_by_id),
        );
        let checked_inputs_by_id = time_provider(
            db.context().timings(),
            "executable_checked_modules.batch_inputs_by_id",
            || reachable_module_inputs_by_id(&checked_inputs),
        );
        for batch_item in batch_items {
            time_module_provider(
                db,
                "executable_checked_modules.incremental_extend",
                batch_item.module_id,
                || {
                    extend_incremental_executable_reachability_from_checked_module_with_timings(
                        &mut reachability_state,
                        &parse_ok,
                        nia_executable_reachability::ExecutableSignatureIndex {
                            function: &function_signature,
                            struct_: &struct_signature,
                            union: &union_signature,
                            trait_: &trait_signature,
                            trait_default_method: &trait_default_method,
                        },
                        &extension_lookup,
                        checked_inputs
                            .iter()
                            .copied()
                            .find(|input| input.module_id == batch_item.module_id)
                            .expect("just-checked module must have a reachable input"),
                        &batch_item.checked_functions,
                        &checked_inputs_by_id,
                        db.context().timings(),
                    )
                },
            );
        }
    }
    let reachability = reachability_state.into_reachability();
    let reachability_by_module = reachability.by_module();

    let parse_ok_modules = parse_ok;
    let mut checked_modules_by_id = time_provider(
        db.context().timings(),
        "executable_checked_modules.final.full_body_check",
        || {
            final_executable_checked_modules(
                db,
                &parse_ok_modules,
                &reachability,
                &reachability_by_module,
                &mut fact_by_id,
                &caches,
                &comptime_module_cache,
            )
        },
    );
    let mut codegen_modules = time_provider(
        db.context().timings(),
        "executable_checked_modules.final.codegen_modules",
        || {
            parse_ok_modules
                .iter()
                .copied()
                .filter(|module_id| reachability.modules().contains(module_id))
                .filter_map(|module_id| checked_modules_by_id.remove(&module_id))
                .collect::<Vec<_>>()
        },
    );
    let codegen_layout_cache = time_provider(
        db.context().timings(),
        "executable_checked_modules.final.layout_cache",
        || {
            RefCell::new(
                codegen_modules
                    .iter()
                    .map(|module| (module.id, module.layouts.clone()))
                    .collect::<HashMap<_, _>>(),
            )
        },
    );
    let non_function_signatures = time_provider(
        db.context().timings(),
        "executable_checked_modules.final.non_function_signatures",
        || {
            non_function_signatures
                .get_or_insert_with(|| executable_program_non_function_signatures(db))
        },
    );
    let executable_program_layouts = executable_program_layouts(
        db,
        &codegen_layout_cache,
        reachability.functions(),
        reachability.globals(),
        Some(&caches.array_lengths),
        Some(&*non_function_signatures),
        None,
    );
    let type_only_modules = time_provider(
        db.context().timings(),
        "executable_checked_modules.final.type_only_modules",
        || {
            parse_ok_modules
                .iter()
                .copied()
                .filter(|module_id| reachability.type_modules().contains(module_id))
                .filter(|module_id| !reachability.modules().contains(module_id))
                .map(|module_id| {
                    let layouts = executable_program_layouts(module_id).unwrap_or_else(|| {
                        signature_layouts_for_types(db, module_id, Some(&*non_function_signatures))
                    });
                    executable_signature_checked_module(
                        db,
                        module_id,
                        layouts,
                        non_function_signatures,
                    )
                })
                .collect::<Vec<_>>()
        },
    );
    codegen_modules.extend(type_only_modules);
    let codegen_array_lengths = time_provider(
        db.context().timings(),
        "executable_checked_modules.final.array_lengths",
        || {
            codegen_modules
                .iter()
                .map(|module| (module.id, module.comptime.array_lengths.clone()))
                .collect::<HashMap<_, _>>()
        },
    );
    let executable_program_array_lengths = |id: nia_ids::GlobalConstExprId| {
        codegen_array_lengths
            .get(&id.module_id)
            .and_then(|array_lengths| array_lengths.get(&id).copied())
            .or_else(|| {
                caches
                    .array_lengths
                    .borrow()
                    .get(&id.module_id)
                    .and_then(|array_lengths| array_lengths.values.get(&id).copied())
            })
    };
    codegen_modules = time_provider(
        db.context().timings(),
        "executable_checked_modules.final.filter_codegen",
        || {
            codegen_modules
                .into_iter()
                .map(|module| {
                    let empty_functions = HashSet::new();
                    let empty_globals = HashSet::new();
                    let module_items = reachability_by_module.get(module.id);
                    let reachable_functions = module_items
                        .map(|items| &items.functions)
                        .unwrap_or(&empty_functions);
                    let reachable_globals = module_items
                        .map(|items| &items.globals)
                        .unwrap_or(&empty_globals);
                    filter_checked_module_for_codegen(
                        module,
                        db,
                        reachable_functions,
                        reachable_globals,
                        Some(&executable_program_layouts),
                        Some(&executable_program_array_lengths),
                    )
                })
                .collect::<Vec<_>>()
        },
    );
    let aggregate_roots = time_provider(
        db.context().timings(),
        "executable_checked_modules.final.aggregate_roots",
        || {
            executable_reachable_aggregate_roots(
                &struct_signature,
                &union_signature,
                &codegen_modules,
            )
        },
    );
    time_provider(
        db.context().timings(),
        "executable_checked_modules.final.store_aggregate_roots",
        || {
            for module in &mut codegen_modules {
                module.executable_reachable_structs = Some(aggregate_roots.structs.clone());
                module.executable_reachable_unions = Some(aggregate_roots.unions.clone());
            }
        },
    );
    db.context()
        .store_executable_checked_modules(codegen_modules)
}

fn executable_debug_function_names(
    db: &QueryDb<CompilerContext>,
    module_id: ModuleId,
    functions: &HashSet<GlobalDefId>,
) -> Vec<String> {
    if !debug_executable_reachability_enabled() {
        return Vec::new();
    }
    let defs = db.query_shared(FullModuleDefsQuery(module_id));
    let symbols = db.context().symbols();
    let mut names = functions
        .iter()
        .filter(|def_id| def_id.module_id == module_id)
        .filter_map(|def_id| defs.defs.get(def_id.def_id))
        .map(|def| nia_symbol::symbol_text_or_unresolved(&symbols, def.name))
        .collect::<Vec<_>>();
    names.sort();
    names
}

fn final_executable_checked_modules(
    db: &QueryDb<CompilerContext>,
    parse_ok: &[ModuleId],
    reachability: &nia_executable_reachability::ExecutableReachability,
    reachability_by_module: &nia_executable_reachability::ExecutableReachabilityByModule,
    fact_by_id: &mut HashMap<ModuleId, ExecutableFactModuleState>,
    caches: &ExecutableCheckCaches,
    comptime_module_cache: &RefCell<HashMap<ModuleId, ComptimeModuleLowering>>,
) -> HashMap<ModuleId, CheckedModule> {
    let reachable_body_modules = executable_reachable_body_modules(db, reachability_by_module);
    let modules_with_executable_items = parse_ok
        .iter()
        .copied()
        .filter(|module_id| reachability.modules().contains(module_id))
        .filter_map(|module_id| {
            let module_items = reachability_by_module.get(module_id)?;
            (!module_items.functions.is_empty() || !module_items.globals.is_empty())
                .then_some((module_id, module_items))
        })
        .collect::<Vec<_>>();
    let program_layout_cache = RefCell::new(HashMap::<ModuleId, nia_layout::Layouts>::new());
    for (module_id, _) in modules_with_executable_items.iter().copied() {
        let layouts = executable_layouts_for_reachable_items(
            db,
            module_id,
            reachability.functions(),
            reachability.globals(),
            Some(&caches.array_lengths),
            None,
            Some(&reachable_body_modules),
        );
        program_layout_cache.borrow_mut().insert(module_id, layouts);
    }
    let executable_program_layouts = executable_program_layouts(
        db,
        &program_layout_cache,
        reachability.functions(),
        reachability.globals(),
        Some(&caches.array_lengths),
        None,
        Some(&reachable_body_modules),
    );
    modules_with_executable_items
        .into_iter()
        .map(|(module_id, module_items)| {
            let module_functions = &module_items.functions;
            let module_globals = &module_items.globals;
            let layouts = program_layout_cache
                .borrow()
                .get(&module_id)
                .cloned()
                .unwrap_or_else(|| {
                    executable_layouts_for_reachable_items(
                        db,
                        module_id,
                        reachability.functions(),
                        reachability.globals(),
                        Some(&caches.array_lengths),
                        None,
                        Some(&reachable_body_modules),
                    )
                });
            let filter = nia_body_check::BodyCheckFilter::ReachableItems {
                functions: module_functions,
                globals: module_globals,
                already_checked_functions: None,
                already_checked_globals: None,
            };
            let resolution_inputs = {
                let cached = caches
                    .body_resolution_inputs
                    .borrow()
                    .get(&module_id)
                    .cloned();
                cached.unwrap_or_else(|| {
                    let inputs = time_module_provider(
                        db,
                        "executable_checked_modules.full_body_inputs",
                        module_id,
                        || full_body_check_resolution_inputs(db, module_id),
                    );
                    caches
                        .body_resolution_inputs
                        .borrow_mut()
                        .insert(module_id, inputs.clone());
                    inputs
                })
            };
            let seed_interner = fact_by_id
                .remove(&module_id)
                .map(|state| state.body_ir.interner);
            let body_check = time_module_provider(db, "executable_body_check", module_id, || {
                body_check_with_filter_and_layouts_with_inputs(
                    db,
                    module_id,
                    filter,
                    Some(layouts.clone()),
                    Some(&executable_program_layouts),
                    ExecutableFactMode::executable(&reachable_body_modules),
                    Some(resolution_inputs),
                    seed_interner,
                    Some(&caches.global_initializers),
                    Some(comptime_module_cache),
                    Some(&caches.body_function_signatures),
                )
            });
            let checked_functions = body_check.body_check.checked_functions.clone();
            let flow_check = executable_flow_check(db, module_id, &checked_functions);
            let module = executable_checked_module_with_body_and_flow_check(
                db, module_id, body_check, flow_check, layouts,
            );
            (module_id, module)
        })
        .collect()
}

fn extend_reachability_from_value_ref_edges(
    db: &QueryDb<CompilerContext>,
    parse_ok: &[ModuleId],
    reachability: &mut nia_executable_reachability::ExecutableReachability,
    reachability_by_module: &nia_executable_reachability::ExecutableReachabilityByModule,
    function_signature: &dyn Fn(GlobalDefId) -> Option<Arc<ProgramFunctionSignature>>,
    fact_by_id: &HashMap<ModuleId, ExecutableFactModuleState>,
) -> bool {
    let mut changed = false;
    for module_id in parse_ok.iter().copied() {
        if !reachability.modules().contains(&module_id) {
            continue;
        }
        let (module_functions, module_globals) =
            unchecked_executable_items(reachability_by_module, module_id, fact_by_id);
        if module_functions.is_empty() && module_globals.is_empty() {
            continue;
        }
        let edges = executable_value_ref_edges_from_reachable_items(
            db,
            module_id,
            &module_functions,
            &module_globals,
        );
        for def_id in edges.functions {
            if (function_signature)(def_id).is_none() {
                continue;
            }
            changed |= reachability.insert_function(def_id);
        }
        for def_id in edges.globals {
            changed |= reachability.insert_global(def_id);
        }
    }
    changed
}
