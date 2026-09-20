// SPDX-License-Identifier: GPL-3.0-or-later
use super::*;

pub(in crate::query) fn provide_executable_checked_module_facts(
    db: &QueryDb<CompilerContext>,
) -> QueryResult<ExecutableCheckedModuleFacts> {
    time_provider(
        db.context().timings(),
        "executable_checked_module_facts",
        || match executable_check(db, ExecutableCheckProduct::Modules)? {
            ExecutableCheckOutput::Modules(set) => Ok(set),
            ExecutableCheckOutput::ProviderDemands(_) => Err(QueryError::internal(
                "module executable check returned provider demands",
            )),
        },
    )
}

pub(in crate::query) fn provide_executable_provider_demands(
    db: &QueryDb<CompilerContext>,
) -> QueryResult<Vec<crate::ProviderDemand>> {
    time_provider(
        db.context().timings(),
        "executable_provider_demands",
        || match executable_check(db, ExecutableCheckProduct::ProviderDemands)? {
            ExecutableCheckOutput::ProviderDemands(demands) => Ok(demands),
            ExecutableCheckOutput::Modules(_) => Err(QueryError::internal(
                "provider-demand executable check returned modules",
            )),
        },
    )
}

#[derive(Clone, Copy)]
enum ExecutableCheckProduct {
    ProviderDemands,
    Modules,
}

enum ExecutableCheckOutput {
    ProviderDemands(Vec<crate::ProviderDemand>),
    Modules(ExecutableCheckedModuleFacts),
}

struct QueryExecutableExtensionLookup<'a> {
    db: &'a QueryDb<CompilerContext>,
    failure: RefCell<Option<QueryError>>,
    trait_impls_by_trait:
        RefCell<HashMap<nia_ty::TraitId, Vec<nia_item_signatures::ProgramTraitImplSignature>>>,
    trait_impl_index_by_trait:
        RefCell<HashMap<nia_ty::TraitId, nia_item_signatures::ProgramTraitImplIndex>>,
    trait_impl_position_by_key:
        RefCell<HashMap<(ModuleId, nia_ids::TraitImplId, nia_ty::TraitId), usize>>,
    module_ids_by_trait: RefCell<HashMap<nia_ty::TraitId, Vec<ModuleId>>>,
    methods_by_trait: RefCell<HashMap<nia_ty::TraitId, Vec<nia_defs::ExtensionMethod>>>,
    methods_by_trait_impl: RefCell<
        HashMap<(nia_ty::TraitId, ModuleId, nia_ids::TraitImplId), Vec<nia_defs::ExtensionMethod>>,
    >,
}

impl QueryExecutableExtensionLookup<'_> {
    fn new(db: &QueryDb<CompilerContext>) -> QueryExecutableExtensionLookup<'_> {
        QueryExecutableExtensionLookup {
            db,
            failure: RefCell::new(None),
            trait_impls_by_trait: RefCell::new(HashMap::new()),
            trait_impl_index_by_trait: RefCell::new(HashMap::new()),
            trait_impl_position_by_key: RefCell::new(HashMap::new()),
            module_ids_by_trait: RefCell::new(HashMap::new()),
            methods_by_trait: RefCell::new(HashMap::new()),
            methods_by_trait_impl: RefCell::new(HashMap::new()),
        }
    }

    fn take_failure(&self) -> Option<QueryError> {
        self.failure.borrow_mut().take()
    }

    fn ensure_trait_impls_for_trait(&self, trait_id: nia_ty::TraitId) {
        if self.trait_impls_by_trait.borrow().contains_key(&trait_id) {
            return;
        }
        let trait_impls = capture_query_failure(
            &self.failure,
            self.db.get(ExtensionTraitImplsForTraitQuery(trait_id)),
        )
        .map(|facts| facts.trait_impls.clone())
        .unwrap_or_default();
        let trait_impl_index = nia_item_signatures::ProgramTraitImplIndex::new_with_type_store(
            &trait_impls,
            &self.db.context().type_store,
        );
        self.trait_impl_position_by_key
            .borrow_mut()
            .extend(trait_impls.iter().enumerate().map(|(index, signature)| {
                (
                    (signature.module_id, signature.impl_id, signature.trait_id),
                    index,
                )
            }));
        self.trait_impls_by_trait
            .borrow_mut()
            .insert(trait_id, trait_impls);
        self.trait_impl_index_by_trait
            .borrow_mut()
            .insert(trait_id, trait_impl_index);
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

    fn ensure_methods_for_trait(&self, trait_id: nia_ty::TraitId) {
        if self.methods_by_trait.borrow().contains_key(&trait_id) {
            return;
        }
        let mut seen = HashSet::new();
        let mut methods = Vec::new();
        for module_id in self.module_ids_for_trait(trait_id) {
            let Some(facts) = capture_query_failure(
                &self.failure,
                self.db.get(ExtensionProviderModuleFactsQuery(module_id)),
            ) else {
                break;
            };
            methods.extend(
                facts
                    .methods
                    .all_methods()
                    .filter(|method| method.trait_id == Some(trait_id))
                    .filter(|method| seen.insert(method.def_id))
                    .cloned(),
            );
        }
        {
            let mut by_impl = self.methods_by_trait_impl.borrow_mut();
            for method in &methods {
                by_impl
                    .entry((trait_id, method.def_id.module_id, method.impl_id))
                    .or_default()
                    .push(method.clone());
            }
        }
        self.methods_by_trait.borrow_mut().insert(trait_id, methods);
    }

    fn candidate_impl_keys_for_target(
        &self,
        trait_id: nia_ty::TraitId,
        target_ty: InternedTyId,
    ) -> Vec<(ModuleId, nia_ids::TraitImplId)> {
        self.ensure_trait_impls_for_trait(trait_id);
        let indexes = self.trait_impl_index_by_trait.borrow();
        let Some(index) = indexes.get(&trait_id) else {
            return Vec::new();
        };
        let mut candidate_indexes = index.indexes_for_trait_and_target(trait_id, target_ty);
        if let Some(nia_ty::TyKind::Pointer { elem, .. }) =
            self.db.context().type_store.get(target_ty)
        {
            candidate_indexes.extend(index.indexes_for_trait_and_target(trait_id, *elem));
        }
        candidate_indexes.sort_unstable();
        candidate_indexes.dedup();
        let trait_impls = self.trait_impls_by_trait.borrow();
        let Some(trait_impls) = trait_impls.get(&trait_id) else {
            return Vec::new();
        };
        candidate_indexes
            .into_iter()
            .filter_map(|index| trait_impls.get(index))
            .map(|signature| (signature.module_id, signature.impl_id))
            .collect()
    }
}

impl ExecutableExtensionLookup for QueryExecutableExtensionLookup<'_> {
    fn for_each_method_for_trait(
        &self,
        trait_id: nia_ty::TraitId,
        f: &mut dyn FnMut(&nia_defs::ExtensionMethod),
    ) {
        self.ensure_methods_for_trait(trait_id);
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
        self.ensure_methods_for_trait(trait_id);
        let methods = self.methods_by_trait.borrow();
        if let Some(methods) = methods.get(&trait_id) {
            for method in methods.iter().filter(|method| method.name == *method_name) {
                f(method);
            }
        }
    }

    fn for_each_method_candidate_for_trait(
        &self,
        trait_id: nia_ty::TraitId,
        target_ty: InternedTyId,
        f: &mut dyn FnMut(&nia_defs::ExtensionMethod),
    ) {
        let candidates = self.candidate_impl_keys_for_target(trait_id, target_ty);
        self.ensure_methods_for_trait(trait_id);
        let methods = self.methods_by_trait_impl.borrow();
        for (module_id, impl_id) in candidates {
            if let Some(candidates) = methods.get(&(trait_id, module_id, impl_id)) {
                for method in candidates {
                    f(method);
                }
            }
        }
    }

    fn for_each_method_candidate_for_trait_method(
        &self,
        trait_id: nia_ty::TraitId,
        method_name: &SymbolId,
        target_ty: InternedTyId,
        f: &mut dyn FnMut(&nia_defs::ExtensionMethod),
    ) {
        let candidates = self.candidate_impl_keys_for_target(trait_id, target_ty);
        self.ensure_methods_for_trait(trait_id);
        let methods = self.methods_by_trait_impl.borrow();
        for (module_id, impl_id) in candidates {
            if let Some(candidates) = methods.get(&(trait_id, module_id, impl_id)) {
                for method in candidates
                    .iter()
                    .filter(|method| method.name == *method_name)
                {
                    f(method);
                }
            }
        }
    }

    fn with_where_predicates_for_def(
        &self,
        def_id: GlobalDefId,
        f: &mut dyn FnMut(&[nia_defs::WherePredicateSignature]),
    ) {
        let method =
            capture_query_failure(&self.failure, self.db.get(ExtensionMethodByIdQuery(def_id)));
        let predicates = method
            .as_ref()
            .and_then(|method| method.method.as_ref())
            .map(|method| method.where_predicates.as_slice())
            .unwrap_or(&[]);
        f(predicates);
    }

    fn with_const_generics_for_def(&self, def_id: GlobalDefId, f: &mut dyn FnMut(&[SymbolId])) {
        let method =
            capture_query_failure(&self.failure, self.db.get(ExtensionMethodByIdQuery(def_id)));
        let generics = method
            .as_ref()
            .and_then(|method| method.method.as_ref())
            .map(|method| method.effective_const_generics.as_slice())
            .unwrap_or(&[]);
        f(generics);
    }

    fn with_trait_impl_for_method(
        &self,
        method: &nia_defs::ExtensionMethod,
        trait_id: nia_ty::TraitId,
        f: &mut dyn FnMut(&nia_item_signatures::ProgramTraitImplSignature),
    ) -> bool {
        self.ensure_trait_impls_for_trait(trait_id);
        let Some(position) = self
            .trait_impl_position_by_key
            .borrow()
            .get(&(method.def_id.module_id, method.impl_id, trait_id))
            .copied()
        else {
            return false;
        };
        let trait_impls = self.trait_impls_by_trait.borrow();
        let Some(signature) = trait_impls
            .get(&trait_id)
            .and_then(|trait_impls| trait_impls.get(position))
        else {
            return false;
        };
        f(signature);
        true
    }
}

struct ExecutableBodyCheckBatchItem {
    module_id: ModuleId,
    checked_functions: HashSet<GlobalDefId>,
}

fn executable_check(
    db: &QueryDb<CompilerContext>,
    product: ExecutableCheckProduct,
) -> QueryResult<ExecutableCheckOutput> {
    let non_function_signatures = matches!(product, ExecutableCheckProduct::Modules)
        .then(|| executable_program_non_function_signatures(db))
        .transpose()?;
    let _scheduler = db.context().executable_fact_scheduler.lock();
    let session = std::mem::take(&mut *db.context().executable_fact_session.lock());
    let (output, session) =
        executable_check_in_session(db, product, session, non_function_signatures);
    *db.context().executable_fact_session.lock() = session;
    output
}

fn executable_check_in_session(
    db: &QueryDb<CompilerContext>,
    product: ExecutableCheckProduct,
    mut session: ExecutableFactSession,
    mut non_function_signatures: Option<ProgramExecutableNonFunctionSignatures>,
) -> (QueryResult<ExecutableCheckOutput>, ExecutableFactSession) {
    let initial_inputs = (|| {
        let provider_fact_worklist = db.get(ProviderFactWorklistQuery)?;
        let body_activation_worklist = db.get(BodyActivationWorklistQuery)?;
        let executable_fact_epoch = db.get(ExecutableFactEpochQuery)?;
        let parse_ok_module_ids = db.get(ParseOkModuleIdsQuery)?;
        let parse_ok = resolve_stable_module_sequence(db, &parse_ok_module_ids)?;
        let module_versions = parse_ok
            .iter()
            .copied()
            .map(|module_id| {
                db.get(ModuleSourceVersionQuery(module_id))
                    .map(|version| (module_id, *version))
            })
            .collect::<QueryResult<HashMap<_, _>>>()?;
        let (entry_module, runtime_root_modules) =
            db.get(ExecutableRootModulesQuery)?.as_ref().clone();
        let (root_functions, root_globals) =
            executable_root_defs(db, entry_module, &runtime_root_modules, &parse_ok)?;
        Ok((
            provider_fact_worklist,
            body_activation_worklist,
            executable_fact_epoch,
            parse_ok,
            module_versions,
            entry_module,
            root_functions,
            root_globals,
        ))
    })();
    let (
        provider_fact_worklist,
        body_activation_worklist,
        executable_fact_epoch,
        parse_ok,
        module_versions,
        entry_module,
        root_functions,
        root_globals,
    ) = match initial_inputs {
        Ok(inputs) => inputs,
        Err(error) => return (Err(error), session),
    };
    session.enter_epoch(&executable_fact_epoch);
    let precise_provider_growth = session.can_preserve_diagnostic_facts_for_provider_growth(
        &provider_fact_worklist,
        &module_versions,
    );
    let (module_sync, invalidation) = if precise_provider_growth {
        let invalidation =
            session.apply_provider_fact_worklist(&provider_fact_worklist, &db.context().type_store);
        let module_sync = session.synchronize_module_versions(&module_versions, true);
        (module_sync, invalidation)
    } else {
        let module_sync = session.synchronize_module_versions(&module_versions, false);
        session.apply_body_activation_worklist(&body_activation_worklist);
        let invalidation =
            session.apply_provider_fact_worklist(&provider_fact_worklist, &db.context().type_store);
        (module_sync, invalidation)
    };
    emit_module_version_sync_counters(db, product, module_sync);
    emit_provider_fact_invalidation_counters(db, product, invalidation);
    if precise_provider_growth {
        session.apply_body_activation_worklist(&body_activation_worklist);
    }
    let ExecutableFactSession {
        epoch,
        module_versions,
        mut modules,
        reachability,
        caches,
        applied_provider_fact_revision,
        applied_provider_changes,
        applied_body_activations,
    } = session;
    let query_failure = RefCell::new(None);
    let mut value_ref_scan_progress = ValueRefScanProgress::default();
    let function_signature = |def_id: GlobalDefId| {
        if let Some(signature) = caches
            .reachability_function_signatures
            .borrow()
            .get(&def_id)
            .cloned()
        {
            return Some(signature);
        }
        let signatures = capture_query_failure(
            &query_failure,
            db.get(SignatureItemSignaturesQuery(
                def_id.module_id,
                nia_item_tree::SignatureItemSet::Functions,
            )),
        )?;
        let signature = signatures.semantic.functions.get(&def_id.def_id).cloned()?;
        let defs =
            capture_query_failure(&query_failure, module_defs_semantic(db, def_id.module_id))?;
        let signature = ProgramFunctionSignature {
            name: defs
                .defs
                .get(def_id.def_id)
                .map(|def| def.name)
                .unwrap_or_default(),
            signature,
        };
        let signature = Arc::new(signature);
        caches
            .reachability_function_signatures
            .borrow_mut()
            .insert(def_id, signature.clone());
        Some(signature)
    };
    let struct_signature = |def_id: GlobalDefId| {
        capture_query_failure(
            &query_failure,
            db.get(SignatureItemSignaturesQuery(
                def_id.module_id,
                nia_item_tree::SignatureItemSet::Types,
            )),
        )?
        .semantic
        .structs
        .get(&def_id.def_id)
        .cloned()
        .map(|signature| ProgramStructSignature { signature })
    };
    let union_signature = |def_id: GlobalDefId| {
        capture_query_failure(
            &query_failure,
            db.get(SignatureItemSignaturesQuery(
                def_id.module_id,
                nia_item_tree::SignatureItemSet::Types,
            )),
        )?
        .semantic
        .unions
        .get(&def_id.def_id)
        .cloned()
        .map(|signature| ProgramUnionSignature { signature })
    };
    let enum_signature = |def_id: GlobalDefId| {
        capture_query_failure(
            &query_failure,
            db.get(SignatureItemSignaturesQuery(
                def_id.module_id,
                nia_item_tree::SignatureItemSet::Types,
            )),
        )?
        .semantic
        .enums
        .get(&def_id.def_id)
        .cloned()
        .map(|signature| ProgramEnumSignature { signature })
    };
    let type_alias_signature = |def_id: GlobalDefId| {
        capture_query_failure(
            &query_failure,
            db.get(SignatureItemSignaturesQuery(
                def_id.module_id,
                nia_item_tree::SignatureItemSet::Types,
            )),
        )?
        .semantic
        .type_aliases
        .get(&def_id.def_id)
        .cloned()
        .map(|signature| ProgramTypeAliasSignature { signature })
    };
    let trait_signature = |def_id: GlobalDefId| {
        capture_query_failure(
            &query_failure,
            db.get(SignatureItemSignaturesQuery(
                def_id.module_id,
                nia_item_tree::SignatureItemSet::Traits,
            )),
        )?
        .semantic
        .traits
        .get(&def_id.def_id)
        .cloned()
        .map(|signature| ProgramTraitSignature { signature })
    };
    let trait_default_method = |def_id: GlobalDefId| {
        let signatures = capture_query_failure(
            &query_failure,
            db.get(SignatureItemSignaturesQuery(
                def_id.module_id,
                nia_item_tree::SignatureItemSet::Traits,
            )),
        )?;
        signatures
            .semantic
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
                            },
                        )
                    })
            })
    };
    let parse_ok_set = parse_ok.iter().copied().collect::<HashSet<_>>();
    modules.retain(|module_id, _| parse_ok_set.contains(module_id));
    let mut fact_by_id = modules;
    let mut reachability_state = reachability;
    macro_rules! return_session_error {
        ($error:expr) => {
            return (
                Err($error.into()),
                ExecutableFactSession {
                    epoch,
                    module_versions: module_versions.clone(),
                    modules: fact_by_id,
                    reachability: reachability_state,
                    caches,
                    applied_provider_fact_revision,
                    applied_provider_changes,
                    applied_body_activations,
                },
            )
        };
    }
    let extension_lookup = QueryExecutableExtensionLookup::new(db);
    loop {
        emit_executable_check_counter(db, product, "iterations", 1);
        let reachable_inputs = time_provider(
            db.context().timings(),
            "executable_checked_modules.inputs",
            || reachable_fact_module_inputs(&fact_by_id, &db.context().type_store),
        );
        let reachability_result = time_provider(
            db.context().timings(),
            "executable_checked_modules.reachability_compute",
            || {
                compute_executable_reachability_incremental_with_timings(
                    &mut reachability_state,
                    nia_executable_reachability::ExecutableReachabilityInput {
                        parse_ok: &parse_ok,
                        entry_module,
                        root_defs: ExecutableRootDefs {
                            functions: &root_functions,
                            globals: &root_globals,
                        },
                        program_signatures: nia_executable_reachability::ExecutableSignatureIndex {
                            function: &function_signature,
                            struct_: &struct_signature,
                            union: &union_signature,
                            enum_: &enum_signature,
                            type_alias: &type_alias_signature,
                            trait_: &trait_signature,
                            trait_default_method: &trait_default_method,
                        },
                        modules: &reachable_inputs,
                    },
                    &extension_lookup,
                    db.context().timings(),
                )
            },
        );
        if let Err(error) = reachability_result {
            return_session_error!(error);
        }
        if let Some(error) = query_failure.borrow_mut().take() {
            return_session_error!(error);
        }
        let (value_edges_changed, value_ref_stats) = match time_provider(
            db.context().timings(),
            "executable_checked_modules.value_ref_edges",
            || {
                let reachability = reachability_state.reachability_mut();
                extend_reachability_from_value_ref_edges(
                    db,
                    &parse_ok,
                    reachability,
                    &function_signature,
                    &fact_by_id,
                    &caches,
                    &mut value_ref_scan_progress,
                )
            },
        ) {
            Ok(changed) => changed,
            Err(error) => return_session_error!(error),
        };
        emit_value_ref_scan_counters(db, product, &value_ref_stats);
        if let Some(error) = query_failure.borrow_mut().take() {
            return_session_error!(error);
        }
        if value_edges_changed {
            emit_executable_check_counter(db, product, "value_edge_restarts", 1);
            continue;
        }
        let reachability_by_module = reachability_state.reachability().by_module();
        let stale = match time_provider(
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
        ) {
            Ok(stale) => stale,
            Err(error) => return_session_error!(error),
        };
        if stale.is_empty() {
            break;
        }
        emit_executable_check_counter(db, product, "stale_modules", stale.len() as u64);
        let round_reachable_body_modules =
            match executable_reachable_body_modules(db, &reachability_by_module) {
                Ok(modules) => modules,
                Err(error) => return_session_error!(error),
            };
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
            let (module_functions, static_owner_added) = match time_module_provider(
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
            ) {
                Ok(functions) => functions,
                Err(error) => return_session_error!(error),
            };
            let module_functions = if static_owner_added {
                match time_module_provider(
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
                ) {
                    Ok(functions) => functions,
                    Err(error) => return_session_error!(error),
                }
            } else {
                module_functions
            };
            reachability_state
                .reachability_mut()
                .insert_functions(module_functions.iter().copied());
            let filter = nia_body_check::BodyCheckFilter::ReachableItems {
                functions: &module_functions,
                globals: &module_globals,
                already_checked_functions,
                already_checked_globals,
            };
            let mut has_reachable_body_items = !module_functions.is_empty();
            if !has_reachable_body_items && !module_globals.is_empty() {
                let defs = match module_defs_semantic(db, module_id) {
                    Ok(defs) => defs,
                    Err(error) => return_session_error!(error),
                };
                has_reachable_body_items = module_globals.iter().any(|def_id| {
                    defs.defs
                        .get(def_id.def_id)
                        .is_some_and(|def| def.kind == DefKind::Global)
                });
            }
            let reachable_body_modules = if has_reachable_body_items {
                ReachableBodyModules::new(&round_reachable_body_modules).with_extra(module_id)
            } else {
                ReachableBodyModules::new(&round_reachable_body_modules)
            };
            let layouts = match store_module_layouts(db.context(), {
                let reachability = reachability_state.reachability();
                match executable_layouts_for_reachable_items(
                    db,
                    module_id,
                    reachability.functions(),
                    reachability.globals(),
                    Some(&caches.array_lengths),
                    None,
                    Some(reachable_body_modules),
                ) {
                    Ok(layouts) => layouts,
                    Err(error) => return_session_error!(error),
                }
            }) {
                Ok(layouts) => layouts,
                Err(error) => return_session_error!(error),
            };
            let seed = fact_by_id
                .get(&module_id)
                .map(|state| nia_body_check::BodyCheckSeed {
                    facts: &state.semantic_facts,
                });
            let body_check = {
                let resolution_inputs = {
                    let cached = caches
                        .body_resolution_inputs
                        .borrow()
                        .get(&module_id)
                        .cloned();
                    match cached {
                        Some(inputs) => inputs,
                        None => {
                            let inputs = match time_module_provider(
                                db,
                                "executable_checked_modules.full_body_inputs",
                                module_id,
                                || full_body_check_resolution_inputs(db, module_id),
                            ) {
                                Ok(inputs) => inputs,
                                Err(error) => return_session_error!(error),
                            };
                            caches
                                .body_resolution_inputs
                                .borrow_mut()
                                .insert(module_id, inputs.clone());
                            inputs
                        }
                    }
                };
                let program_layout_cache = RefCell::new(HashMap::new());
                let program_layout_failure = RefCell::new(None);
                program_layout_cache
                    .borrow_mut()
                    .insert(module_id, layouts.clone());
                let executable_program_layouts = {
                    let reachability = reachability_state.reachability();
                    executable_program_layouts(
                        db,
                        (&program_layout_cache, &program_layout_failure),
                        reachability.functions(),
                        reachability.globals(),
                        Some(&caches.array_lengths),
                        None,
                        Some(reachable_body_modules),
                    )
                };
                let body_check =
                    match time_module_provider(db, "executable_fact_check", module_id, || {
                        body_check_with_filter_and_layouts_with_inputs(
                            db,
                            ExecutableBodyCheckInput {
                                module_id,
                                filter,
                                layouts: Some(Arc::clone(&layouts.semantic)),
                                program_layouts_override: Some(&executable_program_layouts),
                                fact_mode: ExecutableFactMode::executable(reachable_body_modules),
                                resolution_inputs: Some(resolution_inputs),
                                seed,
                                global_initializer_cache: Some(&caches.global_initializers),
                                const_module_cache: Some(&caches.const_modules),
                                const_inputs: None,
                                program_function_signature_cache: Some(
                                    &caches.body_function_signatures,
                                ),
                                product: nia_body_check::BodyCheckProduct::FactsOnly,
                                prechecked: None,
                            },
                        )
                    }) {
                        Ok(body_check) => body_check,
                        Err(error) => {
                            drop(executable_program_layouts);
                            return_session_error!(error)
                        }
                    };
                drop(executable_program_layouts);
                match program_layout_failure.into_inner() {
                    Some(error) => return_session_error!(error),
                    None => body_check,
                }
            };
            let checked_this_round = body_check.body_check.checked_functions.clone();
            reachability_state
                .reachability_mut()
                .insert_functions(checked_this_round.iter().copied());
            let merge_result = time_module_provider(
                db,
                "executable_checked_modules.fact_merge",
                module_id,
                || -> QueryResult<()> {
                    match fact_by_id.get_mut(&module_id) {
                        Some(state) => {
                            state.extend(body_check, module_globals, &db.context().type_store)?;
                        }
                        None => {
                            fact_by_id.insert(
                                module_id,
                                ExecutableFactModuleState::new(
                                    db,
                                    module_id,
                                    body_check,
                                    module_globals,
                                )?,
                            );
                        }
                    }
                    Ok(())
                },
            );
            if let Err(error) = merge_result {
                return_session_error!(error);
            }
            batch_items.push(ExecutableBodyCheckBatchItem {
                module_id,
                checked_functions: checked_this_round,
            });
        }
        let checked_inputs = time_provider(
            db.context().timings(),
            "executable_checked_modules.batch_inputs",
            || reachable_fact_module_inputs(&fact_by_id, &db.context().type_store),
        );
        let checked_inputs_by_id = time_provider(
            db.context().timings(),
            "executable_checked_modules.batch_inputs_by_id",
            || reachable_module_inputs_by_id(&checked_inputs),
        );
        for batch_item in batch_items {
            let Some(module_input) = checked_inputs
                .iter()
                .copied()
                .find(|input| input.module_id == batch_item.module_id)
            else {
                return_session_error!(QueryError::internal(format!(
                    "checked module {:?} has no reachable input",
                    batch_item.module_id
                )))
            };
            let incremental_result = time_module_provider(
                db,
                "executable_checked_modules.incremental_extend",
                batch_item.module_id,
                || {
                    extend_incremental_executable_reachability_from_checked_module_with_timings(
                        &mut reachability_state,
                        nia_executable_reachability::CheckedModuleReachabilityInput {
                            parse_ok: &parse_ok,
                            program_signatures:
                                nia_executable_reachability::ExecutableSignatureIndex {
                                    function: &function_signature,
                                    struct_: &struct_signature,
                                    union: &union_signature,
                                    enum_: &enum_signature,
                                    type_alias: &type_alias_signature,
                                    trait_: &trait_signature,
                                    trait_default_method: &trait_default_method,
                                },
                            module: module_input,
                            checked_functions: &batch_item.checked_functions,
                            modules_by_id: &checked_inputs_by_id,
                        },
                        &extension_lookup,
                        db.context().timings(),
                    )
                },
            );
            if let Err(error) = incremental_result {
                return_session_error!(error);
            }
            if let Some(error) = query_failure.borrow_mut().take() {
                return_session_error!(error);
            }
        }
    }
    if matches!(product, ExecutableCheckProduct::ProviderDemands) {
        let reachability_by_module = reachability_state.reachability().by_module();
        let mut demands = fact_by_id
            .values()
            .flat_map(|state| state.provider_demands.iter().cloned())
            .collect::<HashSet<_>>();
        match executable_module_body_demands(db, &reachability_by_module) {
            Ok(module_demands) => demands.extend(module_demands),
            Err(error) => return_session_error!(error),
        }
        let output = match extension_lookup.take_failure() {
            Some(error) => Err(error),
            None => Ok(ExecutableCheckOutput::ProviderDemands(
                demands.into_iter().collect(),
            )),
        };
        return (
            output,
            ExecutableFactSession {
                epoch,
                module_versions: module_versions.clone(),
                modules: fact_by_id,
                reachability: reachability_state,
                caches,
                applied_provider_fact_revision,
                applied_provider_changes,
                applied_body_activations,
            },
        );
    }
    let reachability = reachability_state.reachability().clone();
    let reachability_by_module = reachability.by_module();
    let parse_ok_modules = parse_ok;
    let mut checked_modules_by_id = match time_provider(
        db.context().timings(),
        "executable_checked_module_facts.finalize",
        || {
            final_executable_checked_modules(
                db,
                &parse_ok_modules,
                &reachability,
                &reachability_by_module,
                &mut fact_by_id,
                &caches,
                non_function_signatures.as_ref(),
            )
        },
    ) {
        Ok(modules) => modules,
        Err(error) => return_session_error!(error),
    };
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
                    .map(|module| {
                        (
                            module.id,
                            ModuleLayouts {
                                semantic: Arc::clone(&module.layouts),
                                diagnostics: module.layout_diagnostics.clone(),
                            },
                        )
                    })
                    .collect::<HashMap<_, _>>(),
            )
        },
    );
    let codegen_layout_failure = RefCell::new(None);
    let non_function_signatures = time_provider(
        db.context().timings(),
        "executable_checked_modules.final.non_function_signatures",
        || non_function_signatures.as_mut(),
    );
    let Some(non_function_signatures) = non_function_signatures else {
        return_session_error!(QueryError::internal(
            "module executable check did not preload non-function signatures",
        ));
    };
    let executable_program_layouts = executable_program_layouts(
        db,
        (&codegen_layout_cache, &codegen_layout_failure),
        reachability.functions(),
        reachability.globals(),
        Some(&caches.array_lengths),
        Some(&*non_function_signatures),
        None,
    );
    let mut type_only_modules = Vec::new();
    for module_id in parse_ok_modules
        .iter()
        .copied()
        .filter(|module_id| reachability.type_modules().contains(module_id))
        .filter(|module_id| !reachability.modules().contains(module_id))
    {
        let layouts = match executable_program_layouts(module_id) {
            Some(_) => {
                let layouts = codegen_layout_cache.borrow().get(&module_id).cloned();
                let Some(layouts) = layouts else {
                    drop(executable_program_layouts);
                    return_session_error!(QueryError::internal(format!(
                        "executable layout callback did not cache module {module_id:?}",
                    )))
                };
                layouts
            }
            None => {
                match signature_layouts_for_types(db, module_id, Some(&*non_function_signatures)) {
                    Ok(layouts) => match store_module_layouts(db.context(), layouts) {
                        Ok(layouts) => layouts,
                        Err(error) => {
                            drop(executable_program_layouts);
                            return_session_error!(error)
                        }
                    },
                    Err(error) => {
                        drop(executable_program_layouts);
                        return_session_error!(error)
                    }
                }
            }
        };
        let module = match executable_signature_checked_module(
            db,
            module_id,
            layouts,
            non_function_signatures,
        ) {
            Ok(module) => module,
            Err(error) => {
                drop(executable_program_layouts);
                return_session_error!(error)
            }
        };
        type_only_modules.push(module);
    }
    codegen_modules.extend(type_only_modules);
    let codegen_array_lengths = time_provider(
        db.context().timings(),
        "executable_checked_modules.final.array_lengths",
        || {
            codegen_modules
                .iter()
                .map(|module| (module.id, module.const_eval.array_lengths.clone()))
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
    let codegen_modules = time_provider(
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
                .collect::<QueryResult<Vec<_>>>()
        },
    );
    let mut codegen_modules = match codegen_modules {
        Ok(modules) => modules,
        Err(error) => {
            drop(executable_program_layouts);
            return_session_error!(error)
        }
    };
    if let Some(error) = codegen_layout_failure.borrow_mut().take() {
        drop(executable_program_layouts);
        return_session_error!(error);
    }
    let aggregate_roots = match time_provider(
        db.context().timings(),
        "executable_checked_modules.final.aggregate_roots",
        || {
            executable_reachable_aggregate_roots(
                &db.context().type_store,
                &function_signature,
                &struct_signature,
                &union_signature,
                &codegen_modules,
            )
        },
    ) {
        Ok(roots) => roots,
        Err(error) => {
            drop(executable_program_layouts);
            return_session_error!(QueryError::Internal(error))
        }
    };
    if let Some(error) = query_failure.borrow_mut().take() {
        drop(executable_program_layouts);
        return_session_error!(error);
    }
    time_provider(
        db.context().timings(),
        "executable_checked_modules.final.store_aggregate_roots",
        || {
            let reachable_structs = std::sync::Arc::new(aggregate_roots.structs);
            let reachable_unions = std::sync::Arc::new(aggregate_roots.unions);
            for module in &mut codegen_modules {
                module.executable_reachable_structs =
                    Some(std::sync::Arc::clone(&reachable_structs));
                module.executable_reachable_unions = Some(std::sync::Arc::clone(&reachable_unions));
            }
        },
    );
    let module_body_demands = match executable_module_body_demands(db, &reachability_by_module) {
        Ok(demands) => demands,
        Err(error) => {
            drop(executable_program_layouts);
            return_session_error!(error)
        }
    };
    if let Some(module) = codegen_modules.first_mut() {
        Arc::make_mut(&mut module.provider_demands).extend(module_body_demands);
    }
    drop(executable_program_layouts);
    let mut runtime_functions = reachability.functions().iter().copied().collect::<Vec<_>>();
    runtime_functions.sort_unstable();
    let mut runtime_globals = reachability.globals().iter().copied().collect::<Vec<_>>();
    runtime_globals.sort_unstable();
    let reachable_body_modules =
        match executable_reachable_body_modules(db, &reachability_by_module) {
            Ok(modules) => modules,
            Err(error) => return_session_error!(error),
        };
    let runtime_module_ids = runtime_functions
        .iter()
        .chain(&runtime_globals)
        .map(|def_id| def_id.module_id)
        .collect::<HashSet<_>>();
    let const_modules = caches
        .const_modules
        .borrow()
        .iter()
        .filter(|(module_id, _)| runtime_module_ids.contains(module_id))
        .map(|(module_id, module)| (*module_id, Arc::clone(&module.module)))
        .collect();
    let mut function_bodies = HashMap::new();
    for module in &mut codegen_modules {
        let body_ir = Arc::make_mut(&mut module.body_ir);
        function_bodies.extend(body_ir.function_bodies.drain());
    }
    let output = match extension_lookup.take_failure() {
        Some(error) => Err(error),
        None => Ok(ExecutableCheckOutput::Modules(
            ExecutableCheckedModuleFacts {
                modules: codegen_modules.into_iter().map(Arc::new).collect(),
                function_bodies,
                const_modules,
                runtime_functions,
                runtime_globals,
                reachable_body_modules,
            },
        )),
    };
    (
        output,
        ExecutableFactSession {
            epoch,
            module_versions,
            modules: fact_by_id,
            reachability: reachability_state,
            caches,
            applied_provider_fact_revision,
            applied_provider_changes,
            applied_body_activations,
        },
    )
}

fn emit_executable_check_counter(
    db: &QueryDb<CompilerContext>,
    product: ExecutableCheckProduct,
    name: &str,
    value: u64,
) {
    if !db.context().timings().enabled() {
        return;
    }
    let product = match product {
        ExecutableCheckProduct::ProviderDemands => "provider_demands",
        ExecutableCheckProduct::Modules => "modules",
    };
    nia_timing::emit_counter(
        format!("compiler.executable_checked_modules.{product}.{name}"),
        value,
    );
}

fn emit_provider_fact_invalidation_counters(
    db: &QueryDb<CompilerContext>,
    product: ExecutableCheckProduct,
    stats: ProviderFactInvalidationStats,
) {
    emit_executable_check_counter(db, product, "provider_changes", stats.changes as u64);
    emit_executable_check_counter(
        db,
        product,
        "invalidating_provider_changes",
        stats.invalidating_changes as u64,
    );
    emit_executable_check_counter(
        db,
        product,
        "provider_discarded_modules",
        stats.discarded_modules as u64,
    );
    emit_executable_check_counter(
        db,
        product,
        "provider_invalidated_functions",
        stats.invalidated_functions as u64,
    );
    emit_executable_check_counter(
        db,
        product,
        "provider_reachability_resets",
        u64::from(stats.reset_reachability),
    );
}

fn emit_module_version_sync_counters(
    db: &QueryDb<CompilerContext>,
    product: ExecutableCheckProduct,
    stats: ModuleVersionSyncStats,
) {
    emit_executable_check_counter(
        db,
        product,
        "module_sync_added_modules",
        stats.added_modules as u64,
    );
    emit_executable_check_counter(
        db,
        product,
        "module_sync_removed_or_changed_modules",
        stats.removed_or_changed_modules as u64,
    );
    emit_executable_check_counter(
        db,
        product,
        "module_sync_discarded_diagnostic_modules",
        stats.discarded_diagnostic_modules as u64,
    );
    emit_executable_check_counter(
        db,
        product,
        "module_sync_discarded_functions",
        stats.discarded_functions as u64,
    );
    emit_executable_check_counter(
        db,
        product,
        "module_sync_reachability_resets",
        u64::from(stats.reset_reachability),
    );
}

#[derive(Default)]
struct ValueRefScanProgress {
    functions: HashSet<GlobalDefId>,
    globals: HashSet<GlobalDefId>,
}

#[derive(Default)]
struct ValueRefScanStats {
    candidate_functions: usize,
    candidate_globals: usize,
    work_modules: usize,
    scanned_functions: usize,
    scanned_globals: usize,
    first_scanned_functions: usize,
    repeated_scanned_functions: usize,
    first_scanned_globals: usize,
    repeated_scanned_globals: usize,
    discovered_function_edges: usize,
    discovered_global_edges: usize,
    new_function_edges: usize,
    new_global_edges: usize,
}

fn emit_value_ref_scan_counters(
    db: &QueryDb<CompilerContext>,
    product: ExecutableCheckProduct,
    stats: &ValueRefScanStats,
) {
    emit_executable_check_counter(
        db,
        product,
        "value_ref_candidate_functions",
        stats.candidate_functions as u64,
    );
    emit_executable_check_counter(
        db,
        product,
        "value_ref_candidate_globals",
        stats.candidate_globals as u64,
    );
    emit_executable_check_counter(
        db,
        product,
        "value_ref_work_modules",
        stats.work_modules as u64,
    );
    emit_executable_check_counter(
        db,
        product,
        "value_ref_scanned_functions",
        stats.scanned_functions as u64,
    );
    emit_executable_check_counter(
        db,
        product,
        "value_ref_scanned_globals",
        stats.scanned_globals as u64,
    );
    emit_executable_check_counter(
        db,
        product,
        "value_ref_first_scanned_functions",
        stats.first_scanned_functions as u64,
    );
    emit_executable_check_counter(
        db,
        product,
        "value_ref_repeated_scanned_functions",
        stats.repeated_scanned_functions as u64,
    );
    emit_executable_check_counter(
        db,
        product,
        "value_ref_first_scanned_globals",
        stats.first_scanned_globals as u64,
    );
    emit_executable_check_counter(
        db,
        product,
        "value_ref_repeated_scanned_globals",
        stats.repeated_scanned_globals as u64,
    );
    emit_executable_check_counter(
        db,
        product,
        "value_ref_discovered_function_edges",
        stats.discovered_function_edges as u64,
    );
    emit_executable_check_counter(
        db,
        product,
        "value_ref_discovered_global_edges",
        stats.discovered_global_edges as u64,
    );
    emit_executable_check_counter(
        db,
        product,
        "value_ref_new_function_edges",
        stats.new_function_edges as u64,
    );
    emit_executable_check_counter(
        db,
        product,
        "value_ref_new_global_edges",
        stats.new_global_edges as u64,
    );
}

fn executable_module_body_demands(
    db: &QueryDb<CompilerContext>,
    reachability_by_module: &nia_executable_reachability::ExecutableReachabilityByModule,
) -> QueryResult<Vec<crate::ProviderDemand>> {
    executable_reachable_body_modules(db, reachability_by_module)?
        .into_iter()
        .map(|module_id| {
            let module_path = db.get(ModulePathQuery(module_id))?;
            let module_path = module_path.as_ref().clone();
            Ok(crate::ProviderDemand {
                source_path: module_path.clone(),
                request: crate::ProviderRequest::ModuleBody { module_path },
            })
        })
        .collect()
}

fn executable_root_defs(
    db: &QueryDb<CompilerContext>,
    entry: ModuleId,
    runtime_root_modules: &[ModuleId],
    parse_ok: &[ModuleId],
) -> QueryResult<(Vec<GlobalDefId>, Vec<GlobalDefId>)> {
    if *db.get(CompilerCodegenScopeQuery)? == crate::CodegenScope::Package {
        return package_root_defs(db, entry, parse_ok);
    }
    match db.get(CompilerRuntimeQuery)?.as_ref() {
        RuntimeSpec::Bare => {
            let defs = full_module_defs_semantic(db, entry)?;
            let signatures = db.get(SignatureItemSignaturesQuery(
                entry,
                nia_item_tree::SignatureItemSet::Functions,
            ))?;
            let mut functions = Vec::new();
            let mut globals = Vec::new();
            for (def_id, def) in defs.defs.iter().filter(|(_, def)| def.parent.is_none()) {
                match def.kind {
                    DefKind::Function
                        if signatures
                            .semantic
                            .functions
                            .get(&def_id)
                            .is_some_and(|signature| !signature.is_const) =>
                    {
                        functions.push(GlobalDefId {
                            module_id: entry,
                            def_id,
                        });
                    }
                    DefKind::Global => globals.push(GlobalDefId {
                        module_id: entry,
                        def_id,
                    }),
                    _ => {}
                }
            }
            Ok((functions, globals))
        }
        RuntimeSpec::Source(runtime) => {
            let graph = db.get(ModuleGraphQuery)?;
            let identity = nia_source::SourceIdentity::new(runtime.entry_point().module_identity());
            let key = nia_imports::StableModuleKey::from_source_identity(identity);
            let module_id = graph.module_id_for_stable_key(&key).ok_or_else(|| {
                db.invalid_input(
                    &CompilerRuntimeQuery,
                    format!(
                        "runtime entry module is not loaded: {}",
                        runtime.entry_point().module_identity()
                    ),
                )
            })?;
            if !runtime_root_modules.contains(&module_id) || !parse_ok.contains(&module_id) {
                return Ok((Vec::new(), Vec::new()));
            }
            let symbol = db
                .context()
                .loader_facts()
                .symbols()
                .intern(runtime.entry_point().definition_name())
                .map_err(|error| {
                    db.invalid_input(
                        &CompilerRuntimeQuery,
                        format!("runtime entry definition name is invalid: {error}"),
                    )
                })?;
            let start = named_top_level_function(db, module_id, symbol)?.ok_or_else(|| {
                db.invalid_input(
                    &CompilerRuntimeQuery,
                    format!(
                        "runtime entry `{}` is absent from {}",
                        runtime.entry_point().definition_name(),
                        runtime.entry_point().module_identity()
                    ),
                )
            })?;
            Ok((vec![start], Vec::new()))
        }
    }
}

fn package_root_defs(
    db: &QueryDb<CompilerContext>,
    entry: ModuleId,
    parse_ok: &[ModuleId],
) -> QueryResult<(Vec<GlobalDefId>, Vec<GlobalDefId>)> {
    let graph = db.get(ModuleGraphQuery)?;
    let package_root = graph.current_package_root(entry);
    let mut functions = Vec::new();
    let mut globals = Vec::new();
    for module_id in parse_ok.iter().copied() {
        if graph.current_package_root(module_id) != package_root {
            continue;
        }
        let defs = full_module_defs_semantic(db, module_id)?;
        let signatures = db.get(SignatureItemSignaturesQuery(
            module_id,
            nia_item_tree::SignatureItemSet::Functions,
        ))?;
        for (def_id, signature) in &signatures.semantic.functions {
            let Some(definition) = defs.defs.get(*def_id) else {
                continue;
            };
            if matches!(definition.kind, DefKind::Function | DefKind::Method)
                && signature.has_body
                && effective_function_generic_params(&signatures.semantic, &defs, *def_id)
                    .is_empty()
            {
                functions.push(GlobalDefId {
                    module_id,
                    def_id: *def_id,
                });
            }
        }
        globals.extend(defs.defs.iter().filter_map(|(def_id, definition)| {
            (definition.kind == DefKind::Global && definition.parent.is_none())
                .then_some(GlobalDefId { module_id, def_id })
        }));
    }
    functions.sort_unstable();
    functions.dedup();
    globals.sort_unstable();
    globals.dedup();
    Ok((functions, globals))
}

fn named_top_level_function(
    db: &QueryDb<CompilerContext>,
    module_id: ModuleId,
    name: SymbolId,
) -> QueryResult<Option<GlobalDefId>> {
    let defs = full_module_defs_semantic(db, module_id)?;
    Ok(defs.defs.iter().find_map(|(def_id, def)| {
        (def.kind == DefKind::Function && def.parent.is_none() && def.name == name)
            .then_some(GlobalDefId { module_id, def_id })
    }))
}

fn final_executable_checked_modules(
    db: &QueryDb<CompilerContext>,
    parse_ok: &[ModuleId],
    reachability: &nia_executable_reachability::ExecutableReachability,
    reachability_by_module: &nia_executable_reachability::ExecutableReachabilityByModule,
    fact_by_id: &mut HashMap<ModuleId, ExecutableFactModuleState>,
    caches: &ExecutableCheckCaches,
    _program_signatures: Option<&ProgramExecutableNonFunctionSignatures>,
) -> QueryResult<HashMap<ModuleId, CheckedModule>> {
    let reachable_body_modules = executable_reachable_body_modules(db, reachability_by_module)?;
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
    let program_layout_cache = RefCell::new(HashMap::<ModuleId, ModuleLayouts>::new());
    let program_layout_failure = RefCell::new(None);
    for (module_id, _) in modules_with_executable_items.iter().copied() {
        let layouts = executable_layouts_for_reachable_items(
            db,
            module_id,
            reachability.functions(),
            reachability.globals(),
            Some(&caches.array_lengths),
            None,
            Some(ReachableBodyModules::new(&reachable_body_modules)),
        )?;
        program_layout_cache
            .borrow_mut()
            .insert(module_id, store_module_layouts(db.context(), layouts)?);
    }
    let executable_program_layouts = executable_program_layouts(
        db,
        (&program_layout_cache, &program_layout_failure),
        reachability.functions(),
        reachability.globals(),
        Some(&caches.array_lengths),
        None,
        Some(ReachableBodyModules::new(&reachable_body_modules)),
    );
    let modules = modules_with_executable_items
        .into_iter()
        .map(
            |(module_id, module_items)| -> QueryResult<(ModuleId, CheckedModule)> {
                let module_functions = &module_items.functions;
                let module_globals = &module_items.globals;
                let layouts =
                    if let Some(layouts) = program_layout_cache.borrow().get(&module_id).cloned() {
                        layouts
                    } else {
                        store_module_layouts(
                            db.context(),
                            executable_layouts_for_reachable_items(
                                db,
                                module_id,
                                reachability.functions(),
                                reachability.globals(),
                                Some(&caches.array_lengths),
                                None,
                                Some(ReachableBodyModules::new(&reachable_body_modules)),
                            )?,
                        )?
                    };
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
                    match cached {
                        Some(inputs) => inputs,
                        None => {
                            let inputs = time_module_provider(
                                db,
                                "executable_checked_modules.full_body_inputs",
                                module_id,
                                || full_body_check_resolution_inputs(db, module_id),
                            )?;
                            caches
                                .body_resolution_inputs
                                .borrow_mut()
                                .insert(module_id, inputs.clone());
                            inputs
                        }
                    }
                };
                let (prechecked, provider_demands) = match fact_by_id.remove(&module_id) {
                    Some(state) => (
                        Some(nia_body_check::PrecheckedBodyCheck {
                            ir: state.body_ir,
                            facts: state.semantic_facts,
                            checked_functions: state.checked_functions,
                            diagnostic_owners: state.diagnostic_owners,
                            diagnostics: state.diagnostics,
                        }),
                        state.provider_demands,
                    ),
                    None => (None, HashSet::new()),
                };
                let body_check =
                    time_module_provider(db, "executable_body_check", module_id, || {
                        body_check_with_filter_and_layouts_with_inputs(
                            db,
                            ExecutableBodyCheckInput {
                                module_id,
                                filter,
                                layouts: Some(Arc::clone(&layouts.semantic)),
                                program_layouts_override: Some(&executable_program_layouts),
                                fact_mode: ExecutableFactMode::executable(
                                    ReachableBodyModules::new(&reachable_body_modules),
                                ),
                                resolution_inputs: Some(resolution_inputs),
                                seed: None,
                                global_initializer_cache: Some(&caches.global_initializers),
                                const_module_cache: Some(&caches.const_modules),
                                const_inputs: None,
                                program_function_signature_cache: Some(
                                    &caches.body_function_signatures,
                                ),
                                product: nia_body_check::BodyCheckProduct::Full,
                                prechecked,
                            },
                        )
                    })?;
                let checked_functions = body_check.body_check.checked_functions.clone();
                let flow_check = executable_flow_check(db, module_id, &checked_functions)?;
                let mut module = executable_checked_module_with_body_and_flow_check(
                    db, module_id, body_check, flow_check, layouts,
                )?;
                Arc::make_mut(&mut module.provider_demands).extend(provider_demands);
                Ok((module_id, module))
            },
        )
        .collect::<QueryResult<HashMap<_, _>>>();
    drop(executable_program_layouts);
    match program_layout_failure.into_inner() {
        Some(error) => Err(error),
        None => modules,
    }
}

fn extend_reachability_from_value_ref_edges(
    db: &QueryDb<CompilerContext>,
    parse_ok: &[ModuleId],
    reachability: &mut nia_executable_reachability::ExecutableReachability,
    function_signature: &dyn Fn(GlobalDefId) -> Option<Arc<ProgramFunctionSignature>>,
    fact_by_id: &HashMap<ModuleId, ExecutableFactModuleState>,
    caches: &ExecutableCheckCaches,
    progress: &mut ValueRefScanProgress,
) -> QueryResult<(bool, ValueRefScanStats)> {
    let mut work_by_module =
        HashMap::<ModuleId, (HashSet<GlobalDefId>, HashSet<GlobalDefId>)>::new();
    for def_id in reachability.functions().iter().copied() {
        if progress.functions.contains(&def_id) || !parse_ok.contains(&def_id.module_id) {
            continue;
        }
        if fact_by_id
            .get(&def_id.module_id)
            .is_some_and(|state| state.checked_functions.contains(&def_id))
        {
            continue;
        }
        work_by_module
            .entry(def_id.module_id)
            .or_default()
            .0
            .insert(def_id);
    }
    for def_id in reachability.globals().iter().copied() {
        if progress.globals.contains(&def_id) || !parse_ok.contains(&def_id.module_id) {
            continue;
        }
        if fact_by_id
            .get(&def_id.module_id)
            .is_some_and(|state| state.checked_globals.contains(&def_id))
        {
            continue;
        }
        work_by_module
            .entry(def_id.module_id)
            .or_default()
            .1
            .insert(def_id);
    }
    let mut work = work_by_module
        .into_iter()
        .map(|(module_id, (functions, globals))| (module_id, functions, globals))
        .collect::<Vec<_>>();
    let mut stats = ValueRefScanStats {
        candidate_functions: work.iter().map(|(_, functions, _)| functions.len()).sum(),
        candidate_globals: work.iter().map(|(_, _, globals)| globals.len()).sum(),
        work_modules: work.len(),
        ..ValueRefScanStats::default()
    };
    work.sort_unstable_by_key(|(module_id, _, _)| *module_id);
    let tasks = work
        .into_iter()
        .map(|(module_id, module_functions, module_globals)| {
            let db = db.clone();
            move || {
                Ok(executable_value_ref_edges_from_reachable_items(
                    &db,
                    module_id,
                    &module_functions,
                    &module_globals,
                )
                .map(|(edges, closure_functions)| {
                    (module_id, module_globals, closure_functions, edges)
                }))
            }
        });
    let results = db.session().run_tasks_bounded(tasks, 4)?;
    let mut changed = false;
    for result in results {
        let (_, module_globals, closure_functions, edges) = result?;
        stats.scanned_functions += closure_functions.len();
        stats.scanned_globals += module_globals.len();
        if db.context().timings().enabled() {
            let mut observed = caches.observed_value_ref_functions.borrow_mut();
            for function in &closure_functions {
                if observed.insert(*function) {
                    stats.first_scanned_functions += 1;
                } else {
                    stats.repeated_scanned_functions += 1;
                }
            }
            let mut observed = caches.observed_value_ref_globals.borrow_mut();
            for global in &module_globals {
                if observed.insert(*global) {
                    stats.first_scanned_globals += 1;
                } else {
                    stats.repeated_scanned_globals += 1;
                }
            }
        }
        stats.discovered_function_edges += edges.functions.len();
        stats.discovered_global_edges += edges.globals.len();
        progress.functions.extend(closure_functions);
        progress.globals.extend(module_globals);
        for def_id in edges.functions {
            if (function_signature)(def_id).is_none() {
                continue;
            }
            let inserted = reachability.insert_function(def_id);
            changed |= inserted;
            stats.new_function_edges += usize::from(inserted);
        }
        for def_id in edges.globals {
            let inserted = reachability.insert_global(def_id);
            changed |= inserted;
            stats.new_global_edges += usize::from(inserted);
        }
    }
    Ok((changed, stats))
}
