// SPDX-License-Identifier: GPL-3.0-or-later
use nia_loader_contract::UnusedUsingImport;
use nia_symbol_table::SymbolTable;
use std::collections::HashSet;

use super::*;

fn symbol_package_identities(
    db: &QueryDb<CompilerContext>,
    graph: &nia_imports::ModuleGraphSnapshot,
) -> QueryResult<HashMap<ModuleId, String>> {
    let runtime = db.context().loader_facts().runtime();
    let runtime_root = graph.package_root(&nia_symbol::known::RUNTIME);
    let entry_root = graph.current_package_root(graph.entry());
    let current_package = db.context().current_package();
    let mut identities = HashMap::new();
    for module in graph.modules() {
        let package = if graph.current_package_root(module.id) == graph.std_package_root() {
            nia_package_metadata::PackageId::standard_library().canonical_text()
        } else if graph.current_package_root(module.id) == runtime_root
            && let nia_toolchain::RuntimeSpec::Source(runtime) = &runtime
        {
            runtime.package().canonical_text()
        } else if graph.current_package_root(module.id) == entry_root
            && let Some(current_package) = &current_package
        {
            current_package.canonical_text()
        } else {
            let root = graph
                .current_package_root(module.id)
                .and_then(|root| graph.stable_key(root))
                .map(|key| key.source_identity().normalized_path())
                .unwrap_or_else(|| module.stable_key.source_identity().normalized_path());
            format!("source-root:{root}")
        };
        identities.insert(module.id, package);
    }
    Ok(identities)
}

pub(in crate::query) fn provide_backend_module_source_item_plan(
    db: &QueryDb<CompilerContext>,
    module_id: ModuleId,
) -> QueryResult<BackendModuleSourceItemPlan> {
    let facts = db.get(ExecutableCheckedModuleFactsQuery)?;
    let Some(module) = facts.modules.iter().find(|module| module.id == module_id) else {
        return Err(QueryError::internal(format!(
            "missing executable facts for module {module_id:?}"
        )));
    };
    let mut functions = facts
        .runtime_functions
        .iter()
        .copied()
        .filter(|def_id| def_id.module_id == module_id)
        .collect::<Vec<_>>();
    functions.sort_unstable();
    functions.dedup();
    let mut globals = facts
        .runtime_globals
        .iter()
        .copied()
        .filter(|def_id| def_id.module_id == module_id)
        .collect::<Vec<_>>();
    globals.sort_unstable();
    globals.dedup();
    let mut structs = module
        .executable_reachable_structs
        .iter()
        .flat_map(|items| items.iter().copied())
        .filter(|def_id| def_id.module_id == module_id)
        .collect::<Vec<_>>();
    structs.sort_unstable();
    structs.dedup();
    let mut unions = module
        .executable_reachable_unions
        .iter()
        .flat_map(|items| items.iter().copied())
        .filter(|def_id| def_id.module_id == module_id)
        .collect::<Vec<_>>();
    unions.sort_unstable();
    unions.dedup();
    Ok(BackendModuleSourceItemPlan {
        functions,
        globals,
        structs,
        unions,
    })
}

pub(in crate::query) fn provide_backend_module_function_instance_plan(
    db: &QueryDb<CompilerContext>,
    module_id: ModuleId,
) -> QueryResult<BackendModuleFunctionInstancePlan> {
    let facts = db.get(ExecutableCheckedModuleFactsQuery)?;
    if !facts.modules.iter().any(|module| module.id == module_id) {
        return Err(QueryError::internal(format!(
            "missing executable facts for module {module_id:?}"
        )));
    }
    let monomorphization = db.get(MonomorphizationQuery)?;
    let mut instances = monomorphization
        .semantic
        .instances
        .iter()
        .filter(|instance| instance.def_id.module_id == module_id)
        .collect::<Vec<_>>();
    instances.sort_by(|left, right| left.symbol.cmp(&right.symbol));
    let mut seen = HashSet::new();
    let mut planned_instances = Vec::with_capacity(instances.len());
    for instance in instances {
        let key = (
            instance.def_id,
            instance.arg_module_id,
            instance.self_arg,
            instance.args.clone(),
            instance.const_args.clone(),
        );
        if !seen.insert(key) {
            return Err(QueryError::internal(format!(
                "duplicate monomorphized function instance `{}`",
                instance.symbol
            )));
        }
        planned_instances.push(nia_backend_lower::BackendFunctionInstancePlan {
            def_id: instance.def_id,
            arg_module_id: instance.arg_module_id,
            self_arg: instance.self_arg,
            args: instance.args.clone(),
            const_args: instance.const_args.clone(),
            span: instance.span,
        });
    }
    Ok(BackendModuleFunctionInstancePlan {
        instances: planned_instances,
    })
}

pub(super) fn provide_monomorphization(
    db: &QueryDb<CompilerContext>,
) -> QueryResult<ProgramMonomorphization> {
    time_provider(db.context().timings(), "monomorphization", || {
        let checked_modules = checked_modules_for_codegen(db)?;
        let mut monomorphization = monomorphization_for_checked_modules(db, &checked_modules)?;
        let diagnostics = std::mem::take(&mut monomorphization.diagnostics);
        Ok(ProgramMonomorphization {
            semantic: Arc::new(monomorphization),
            diagnostics: db.context().diagnostic_store.bundle(diagnostics)?,
        })
    })
}

pub(super) fn monomorphization_for_checked_modules(
    db: &QueryDb<CompilerContext>,
    checked_modules: &[Arc<CheckedModule>],
) -> QueryResult<nia_monomorphize::Monomorphization> {
    let graph = db.context().loader_facts.module_graph()?;
    let source_identities = graph
        .modules()
        .map(|module| (module.id, module.stable_key.source_identity().clone()))
        .collect::<Vec<_>>();
    let symbol_package_identities = symbol_package_identities(db, &graph)?;
    let executable_signatures = executable_program_non_function_signatures_for_modules(
        db,
        checked_modules.iter().map(|module| module.id),
    )?;
    let program_enums = &executable_signatures.enums;
    let trait_impls = executable_signatures.trait_impls.as_slice();
    let trait_impl_index = &executable_signatures.trait_impl_index;
    let local_signatures = checked_modules
        .iter()
        .map(|module| Ok((module.id, item_signatures_semantic(db, module.id)?)))
        .collect::<QueryResult<HashMap<_, _>>>()?;
    let generic_params = HashMap::new();
    let _function_bodies = function_bodies_from_checked_modules(db, checked_modules)?;
    let semantic_instantiations = checked_modules
        .iter()
        .map(|module| {
            module
                .semantic_facts
                .iter_generic_instantiations()
                .cloned()
                .collect::<Vec<_>>()
        })
        .collect::<Vec<_>>();
    let mut module_inputs = Vec::with_capacity(checked_modules.len());
    for (module, semantic_instantiations) in checked_modules.iter().zip(&semantic_instantiations) {
        let Some(symbol_package_identity) = symbol_package_identities.get(&module.id).cloned()
        else {
            return Err(QueryError::internal(format!(
                "monomorphization module {:?} is missing package identity",
                module.id
            )));
        };
        let Some(signatures) = local_signatures.get(&module.id) else {
            return Err(QueryError::internal(format!(
                "monomorphization signatures are missing for checked module {:?}",
                module.id
            )));
        };
        module_inputs.push(MonomorphizeModuleInput {
            module_id: module.id,
            source_identity: module.path.identity(),
            symbol_package_identity,
            defs: &module.defs,
            generic_params: &generic_params,
            normalization: &module.type_normalization,
            const_eval: &module.const_eval,
            const_expr_summaries: &module.type_lowering.const_expr_summaries,
            layouts: Some(&module.layouts),
            local_enums: &signatures.enums,
            program_enums,
            trait_impls,
            trait_impl_index,
            instantiations: semantic_instantiations,
        });
    }
    Ok(nia_monomorphize::collect_monomorphizations(
        &module_inputs,
        source_identities,
        &db.context().type_store,
    )?)
}

pub(super) fn checked_modules_for_codegen(
    db: &QueryDb<CompilerContext>,
) -> QueryResult<Vec<Arc<CheckedModule>>> {
    Ok(db.get(ExecutableCheckedModulesQuery)?.as_ref().clone())
}

pub(super) fn checked_modules_for_diagnostics(
    db: &QueryDb<CompilerContext>,
) -> QueryResult<Vec<Arc<CheckedModule>>> {
    checked_modules_for_codegen(db)
}

pub(super) fn materialize_checked_modules(
    db: &QueryDb<CompilerContext>,
    module_ids: Vec<ModuleId>,
) -> QueryResult<Vec<Arc<CheckedModule>>> {
    db.get_many(module_ids.into_iter().map(CheckedModuleQuery))
}

fn function_bodies_from_checked_modules(
    db: &QueryDb<CompilerContext>,
    checked_modules: &[Arc<CheckedModule>],
) -> QueryResult<Vec<LoweredFunctionBodyHandle>> {
    time_provider(
        db.context().timings(),
        "function_bodies_from_checked_modules",
        || {
            let mut def_ids = checked_modules
                .iter()
                .flat_map(|module| module.body_ir.function_bodies.keys().copied())
                .collect::<Vec<_>>();
            def_ids.sort_unstable();
            let lowered = db.get_many(def_ids.iter().copied().map(LoweredFunctionBodyQuery))?;
            Ok(def_ids
                .into_iter()
                .zip(lowered)
                .map(|(def_id, value)| LoweredFunctionBodyHandle { def_id, value })
                .collect())
        },
    )
}

fn static_inits_from_checked_modules(
    db: &QueryDb<CompilerContext>,
    checked_modules: &[Arc<CheckedModule>],
) -> QueryResult<Vec<StaticInitHandle>> {
    time_provider(
        db.context().timings(),
        "static_inits_from_checked_modules",
        || {
            let mut def_ids = checked_modules
                .iter()
                .flat_map(|module| module.body_ir.global_inits.keys().copied())
                .collect::<Vec<_>>();
            def_ids.sort_unstable();
            let inits = def_ids
                .iter()
                .copied()
                .map(|def_id| db.get(ExecutableStaticInitQuery(def_id)))
                .collect::<QueryResult<Vec<_>>>()?;
            Ok(def_ids
                .into_iter()
                .zip(inits)
                .map(|(def_id, value)| StaticInitHandle { def_id, value })
                .collect())
        },
    )
}

pub(in crate::query) fn provide_lowered_function_body(
    db: &QueryDb<CompilerContext>,
    def_id: GlobalDefId,
) -> QueryResult<LoweredFunctionBodyValue> {
    let checked_body = db.get(ExecutableFunctionBodyQuery(def_id))?;
    let body = match checked_body.as_ref() {
        Some(body) => body,
        None => {
            return Err(nia_ice::Ice::new(format!(
                "missing executable checked function body for {def_id:?}"
            ))
            .into());
        }
    };
    nia_function_lower::lower_function_body(
        def_id.module_id,
        body,
        nia_function_lower::FunctionTypeContext::for_module(
            &db.context().type_store,
            def_id.module_id,
        ),
    )
    .map_err(Into::into)
}

pub(super) fn provide_backend_lowering(
    db: &QueryDb<CompilerContext>,
) -> QueryResult<ProgramBackendLowering> {
    time_provider(db.context().timings(), "backend_lowering", || {
        let mut lowering = provide_backend_lowering_inner(db)?;
        let diagnostics = std::mem::take(&mut lowering.diagnostics);
        Ok(ProgramBackendLowering {
            semantic: Arc::new(lowering),
            diagnostics: db.context().diagnostic_store.bundle(diagnostics)?,
        })
    })
}

pub(in crate::query) fn provide_backend_item_plan(
    db: &QueryDb<CompilerContext>,
) -> QueryResult<nia_backend_lower::BackendItemPlan> {
    time_provider(db.context().timings(), "backend_item_plan", || {
        let inputs = db.get(BackendLoweringInputsQuery)?;
        let optimization = *db.get(CompilerOptimizationQuery)?;
        match inputs.semantic.as_ref() {
            Some(inputs) => {
                let module_inputs = inputs.module_inputs()?;
                Ok(nia_backend_lower::plan_backend_program_with_timings(
                    &module_inputs,
                    &db.context().type_store,
                    optimization,
                    db.context().timings(),
                )?)
            }
            None => Ok(nia_backend_lower::BackendItemPlan::from_diagnostics(
                optimization,
                resolve_diagnostic_bundle(&inputs.diagnostics).to_vec(),
            )),
        }
    })
}

pub(in crate::query) fn provide_backend_finalization_task_context(
    db: &QueryDb<CompilerContext>,
) -> QueryResult<BackendFinalizationTaskContext> {
    BackendFinalizationTaskContext::new(
        db.get(BackendLoweringInputsQuery)?,
        Arc::clone(&db.context().type_store),
        *db.get(CompilerOptimizationQuery)?,
        db.context().timings(),
    )
}

pub(in crate::query) fn provide_backend_module_finalization(
    db: &QueryDb<CompilerContext>,
    key: BackendModuleFinalizationQuery,
) -> QueryResult<nia_backend_lower::BackendModuleFinalization> {
    let context = db.get(BackendFinalizationTaskContextQuery)?;
    let module_plan = db.get_owned(BackendModuleItemPlanQuery(key.module_id))?;
    context.finalize_module(key.position, key.module_id, module_plan)
}

fn provide_backend_lowering_inner(
    db: &QueryDb<CompilerContext>,
) -> QueryResult<nia_backend_lower::BackendLowering> {
    let (lowering, finalization_allocation) = nia_timing::measure_allocation_live_window(|| {
        with_backend_finalization_schedule(db, |schedule| match schedule {
            Ok(schedule) => schedule.finish(),
            Err(lowering) => Ok(lowering),
        })
    });
    if let Some(measurement) = finalization_allocation {
        emit_backend_module_finalization_allocation(measurement);
    }
    lowering
}

pub(in crate::query) fn with_backend_finalization_schedule<R>(
    db: &QueryDb<CompilerContext>,
    consume: impl for<'borrow, 'stream, 'executor> FnOnce(
        Result<
            crate::BackendFinalizationSchedule<'borrow, 'stream, 'executor>,
            nia_backend_lower::BackendLowering,
        >,
    ) -> QueryResult<R>,
) -> QueryResult<R> {
    let plan = db.get_owned(BackendItemPlanQuery)?;
    emit_backend_module_plan_allocation("before_publish");
    let has_diagnostics = !plan.diagnostics().is_empty();
    let (finalization, module_plans) = plan.into_module_plans().map_err(QueryError::Internal)?;
    let module_ids = module_plans
        .iter()
        .map(|module_plan| module_plan.module().id)
        .collect::<Vec<_>>();
    for (module_id, module_plan) in module_ids.iter().copied().zip(module_plans) {
        db.publish_owned(
            BackendModuleItemPlanQuery(module_id),
            module_plan,
            &BackendItemPlanQuery,
        )?;
    }
    emit_backend_module_plan_allocation("after_publish");
    if has_diagnostics {
        let module_plans = module_ids
            .iter()
            .copied()
            .map(|module_id| db.get_owned(BackendModuleItemPlanQuery(module_id)))
            .collect::<QueryResult<Vec<_>>>()?;
        emit_backend_module_plan_allocation("after_consume");
        let lowering = nia_backend_lower::finalize_backend_module_item_plans_with_timings(
            &[],
            &db.context().type_store,
            finalization,
            module_plans,
            db.context().timings(),
        )?;
        return consume(Err(lowering));
    }
    let collector =
        nia_backend_lower::BackendModuleFinalizationCollector::new(finalization, &module_ids)?;
    let readiness = collector.take_readiness()?;
    let result = db.with_many_owned_completion(
        module_ids
            .iter()
            .copied()
            .enumerate()
            .map(|(position, module_id)| BackendModuleFinalizationQuery {
                module_id,
                position,
            }),
        |completions| {
            consume(Ok(crate::BackendFinalizationSchedule::new(
                completions,
                collector,
                readiness,
            )))
        },
    )?;
    emit_backend_module_plan_allocation("after_consume");
    Ok(result)
}

fn emit_backend_module_plan_allocation(stage: &str) {
    let Some(snapshot) = nia_timing::allocation_live_snapshot() else {
        return;
    };
    nia_timing::emit_counter(
        format!("backend.module_plan.{stage}.live_bytes"),
        snapshot.live_bytes,
    );
    nia_timing::emit_counter(
        format!("backend.module_plan.{stage}.peak_live_bytes"),
        snapshot.peak_live_bytes,
    );
}

fn emit_backend_module_finalization_allocation(
    measurement: nia_timing::AllocationLiveWindowMeasurement,
) {
    nia_timing::emit_counter(
        "backend.module_finalization.start_live_bytes",
        measurement.start_live_bytes,
    );
    nia_timing::emit_counter(
        "backend.module_finalization.end_live_bytes",
        measurement.end_live_bytes,
    );
    nia_timing::emit_counter(
        "backend.module_finalization.peak_live_bytes",
        measurement.peak_live_bytes,
    );
    nia_timing::emit_counter(
        "backend.module_finalization.peak_growth_bytes",
        measurement
            .peak_live_bytes
            .saturating_sub(measurement.start_live_bytes),
    );
}

pub(in crate::query) fn provide_backend_lowering_inputs(
    db: &QueryDb<CompilerContext>,
) -> QueryResult<ProgramBackendLoweringInputs> {
    let checked_modules = checked_modules_for_codegen(db)?;
    let (
        active_item_trees,
        item_signatures,
        const_array_lengths,
        const_enum_values,
        visible_extensions,
        extension_methods,
        function_bodies,
        static_inits,
        source_item_plans,
        function_instance_plans,
        program_defs,
    ) = time_provider(
        db.context().timings(),
        "backend_lowering.inputs",
        || -> QueryResult<_> {
            let timings = db.context().timings();
            let active_item_trees = time_provider(
                timings,
                "backend_lowering.inputs.active_item_trees",
                || -> QueryResult<Vec<_>> {
                    checked_modules
                        .iter()
                        .map(|checked_module| {
                            db.get(FullActiveModuleItemTreeQuery(checked_module.id))
                        })
                        .collect::<QueryResult<Vec<_>>>()
                },
            )?;
            let item_signatures = time_provider(
                timings,
                "backend_lowering.inputs.item_signatures",
                || -> QueryResult<Vec<_>> {
                    checked_modules
                        .iter()
                        .map(|checked_module| {
                            if checked_module.executable_type_only {
                                return Ok(db
                                    .get(SignatureItemSignaturesQuery(
                                        checked_module.id,
                                        nia_item_tree::SignatureItemSet::Types,
                                    ))?
                                    .semantic
                                    .as_ref()
                                    .clone());
                            }
                            body_local_item_signatures(
                                db,
                                checked_module.id,
                                &checked_module.type_lowering,
                            )
                        })
                        .collect::<QueryResult<Vec<_>>>()
                },
            )?;
            let const_array_lengths = checked_modules
                .iter()
                .map(|checked_module| Arc::clone(&checked_module.const_eval.array_lengths))
                .collect::<Vec<_>>();
            let const_enum_values = checked_modules
                .iter()
                .map(|checked_module| Arc::clone(&checked_module.const_eval.enum_values))
                .collect::<Vec<_>>();
            let visible_extensions = time_provider(
                timings,
                "backend_lowering.inputs.visible_extensions",
                || -> QueryResult<Vec<_>> {
                    checked_modules
                        .iter()
                        .map(|checked_module| db.get(VisibleExtensionsQuery(checked_module.id)))
                        .collect::<QueryResult<Vec<_>>>()
                },
            )?;
            let extension_methods =
                time_provider(timings, "backend_lowering.inputs.extension_methods", || {
                    db.get(ExtensionMethodIndexQuery)
                })?;
            let function_bodies = function_bodies_from_checked_modules(db, &checked_modules)?;
            let static_inits = static_inits_from_checked_modules(db, &checked_modules)?;
            let source_item_plans = checked_modules
                .iter()
                .map(|module| db.get(BackendModuleSourceItemPlanQuery(module.id)))
                .collect::<QueryResult<Vec<_>>>()?;
            let function_instance_plans = checked_modules
                .iter()
                .map(|module| db.get(BackendModuleFunctionInstancePlanQuery(module.id)))
                .collect::<QueryResult<Vec<_>>>()?;
            let program_defs = checked_modules
                .iter()
                .map(|module| full_module_defs_semantic(db, module.id))
                .collect::<QueryResult<Vec<_>>>()?;
            Ok((
                active_item_trees,
                item_signatures,
                const_array_lengths,
                const_enum_values,
                visible_extensions,
                extension_methods,
                function_bodies,
                static_inits,
                source_item_plans,
                function_instance_plans,
                program_defs,
            ))
        },
    )?;
    let non_function_signatures = executable_program_non_function_signatures_for_modules(
        db,
        checked_modules.iter().map(|module| module.id),
    )?;
    let functions = executable_program_functions_for_modules(
        db,
        checked_modules.iter().map(|module| module.id),
    )?;
    let runtime = db.get(CompilerRuntimeQuery)?.as_ref().clone();
    let graph = db.context().loader_facts.module_graph()?;
    let source_identities = graph
        .modules()
        .map(|module| (module.id, module.stable_key.source_identity().clone()))
        .collect();
    let symbol_package_identities = symbol_package_identities(db, &graph)?;
    let inputs = time_provider(
        db.context().timings(),
        "backend_lowering.module_inputs",
        || {
            BackendLoweringInputs::new(BackendLoweringInputsParts {
                symbols: db.context().symbols(),
                source_identities,
                symbol_package_identities,
                checked_modules,
                runtime,
                active_item_trees,
                item_signatures,
                const_array_lengths,
                const_enum_values,
                visible_extensions,
                extension_methods,
                function_bodies,
                static_inits,
                source_item_plans,
                function_instance_plans,
                program_defs,
                non_function_signatures,
                functions,
            })
        },
    )?;
    Ok(ProgramBackendLoweringInputs {
        semantic: Some(Arc::new(inputs)),
        diagnostics: db.context().diagnostic_store.bundle(Vec::new())?,
    })
}

pub(in crate::query) fn effective_function_generic_params(
    signatures: &nia_item_signatures::ItemSignatures,
    defs: &DefCollection,
    def_id: DefId,
) -> Vec<nia_item_signatures::GenericParamSignature> {
    let mut params = Vec::new();
    if let Some(parent) = defs.defs.get(def_id).and_then(|def| def.parent) {
        if let Some(signature) = signatures.structs.get(&parent) {
            params.extend(signature.generic_params.iter().cloned());
        } else if let Some(signature) = signatures.unions.get(&parent) {
            params.extend(signature.generic_params.iter().cloned());
        } else if let Some(signature) = signatures.traits.get(&parent) {
            params.extend(signature.generic_params.iter().cloned());
        }
    }
    if let Some(signature) = signatures.trait_impls.iter().find(|signature| {
        signature
            .methods
            .iter()
            .any(|method| method.def_id == def_id)
    }) {
        params.extend(signature.generic_params.iter().cloned());
    }
    if let Some(signature) = signatures.functions.get(&def_id) {
        params.extend(signature.generic_params.iter().cloned());
    }
    let mut seen = HashSet::new();
    params
        .into_iter()
        .filter(|param| seen.insert(param.name))
        .collect()
}

pub(super) fn early_program_diagnostics(
    db: &QueryDb<CompilerContext>,
) -> QueryResult<Vec<ProgramDiagnostic>> {
    let load_diagnostics = db.get(ProgramLoadDiagnosticsQuery)?;
    let mut diagnostics = load_diagnostics.to_diagnostics();
    // Parse errors are published once by the loader as load diagnostics.
    let loaded_modules = db.get(LoadedModulesQuery)?;
    let loaded_modules = resolve_stable_module_sequence_from_current_inputs(db, &loaded_modules)?;
    let symbols = db.context().symbols();
    for module_id in loaded_modules {
        let unused = db.get(ModuleUnusedImportsQuery(module_id))?;
        if unused.is_empty() {
            continue;
        }
        let scope = db.get(ModuleUsingScopeQuery(module_id))?;
        let path = db.get(ModulePathQuery(module_id))?;
        diagnostics.extend(
            unused
                .iter()
                // A failed `using` already has a root error; calling the same
                // name unused would describe the recovery, not the source.
                .filter(|import| !scope.unresolved_usings.contains_key(&import.name))
                .map(|import| ProgramDiagnostic {
                    path: path.as_ref().clone(),
                    diagnostic: unused_import_warning(import, &symbols),
                }),
        );
    }
    let public_surfaces = db.get(PublicSurfacesQuery)?;
    let public_using_scopes = db.get(PublicUsingScopesQuery)?;
    for bundle in public_surfaces
        .diagnostics
        .iter()
        .chain(public_using_scopes.diagnostics.iter())
    {
        let path = db.get(ModulePathQuery(bundle.module_id))?;
        diagnostics.extend(module_diagnostics(
            &path,
            resolve_diagnostic_bundle(&bundle.diagnostics),
        ));
    }
    Ok(diagnostics)
}

pub(super) fn checked_module_diagnostics(
    db: &QueryDb<CompilerContext>,
    checked_modules: &[Arc<CheckedModule>],
) -> QueryResult<(Vec<ProgramDiagnostic>, usize)> {
    let mut diagnostics = Vec::new();
    let mut suppressed_downstream = 0;
    for checked in checked_modules {
        // Diagnostics after the first failing phase are usually observations
        // of the same invalid semantic state (for example an unresolved value
        // later appearing as an invalid error conversion). Keep the complete
        // first failing phase, but do not let downstream recovery placeholders
        // turn one source mistake into an error cascade.
        let mut gate = DiagnosticGate::default();
        gate.append(
            &mut diagnostics,
            &checked.path,
            resolve_diagnostic_bundle(&checked.definition_diagnostics),
        );
        for bundle in &checked.frontend_diagnostics {
            gate.append(
                &mut diagnostics,
                &checked.path,
                resolve_diagnostic_bundle(bundle),
            );
        }
        for bundle in &checked.resolution_diagnostics {
            gate.append(
                &mut diagnostics,
                &checked.path,
                resolve_diagnostic_bundle(bundle),
            );
        }
        let remaining = [
            resolve_diagnostic_bundle(&checked.item_diagnostics),
            resolve_diagnostic_bundle(&checked.const_diagnostics),
            resolve_diagnostic_bundle(&checked.static_diagnostics),
            resolve_diagnostic_bundle(&checked.layout_diagnostics),
            resolve_diagnostic_bundle(&checked.abi_diagnostics),
            resolve_diagnostic_bundle(&checked.flow_diagnostics),
            resolve_diagnostic_bundle(&checked.body_diagnostics),
        ];
        for bundle in remaining {
            gate.append(&mut diagnostics, &checked.path, bundle);
        }
        let extension_validation = db.get(ExtensionProviderValidationFactsQuery(checked.id))?;
        gate.append(
            &mut diagnostics,
            &checked.path,
            resolve_diagnostic_bundle(&extension_validation.diagnostics),
        );
        let extension_provider = db.get(ExtensionProviderModuleFactsQuery(checked.id))?;
        gate.append(
            &mut diagnostics,
            &checked.path,
            resolve_diagnostic_bundle(&extension_provider.associated_value_diagnostics),
        );
        suppressed_downstream += gate.suppressed_downstream;
    }
    Ok((diagnostics, suppressed_downstream))
}

#[derive(Default)]
struct DiagnosticGate {
    root_codes: HashSet<String>,
    seen_error_sites: HashSet<(String, nia_span::Span)>,
    suppressed_downstream: usize,
}

impl DiagnosticGate {
    fn append(
        &mut self,
        diagnostics: &mut Vec<ProgramDiagnostic>,
        path: &nia_source::SourcePath,
        bundle: &[Diagnostic],
    ) {
        for diagnostic in bundle {
            if diagnostic.severity == nia_diagnostic::Severity::Error
                && self
                    .root_codes
                    .iter()
                    .any(|root| suppresses_downstream(root, diagnostic.code.as_str()))
            {
                self.suppressed_downstream += 1;
                continue;
            }
            if diagnostic.severity == nia_diagnostic::Severity::Error
                && let Some(span) = diagnostic.primary_span()
                && !self
                    .seen_error_sites
                    .insert((diagnostic.code.as_str().to_string(), span))
            {
                continue;
            }
            if diagnostic.severity == nia_diagnostic::Severity::Error {
                self.root_codes.insert(diagnostic.code.as_str().to_string());
            }
            diagnostics.push(ProgramDiagnostic {
                path: path.clone(),
                diagnostic: diagnostic.clone(),
            });
        }
    }
}

fn unused_import_warning(import: &UnusedUsingImport, symbols: &SymbolTable) -> Diagnostic {
    let name = nia_symbol::symbol_text_or_unresolved(symbols, import.name);
    let summary = format!("unused import `{name}`");
    Diagnostic::user_warning(codes::UNUSED_IMPORT, summary)
        .primary(import.name_span, "this imported name is never used")
        .help(format!("remove `{name}` from this `using` directive"))
        .finish()
}

fn suppresses_downstream(root: &str, candidate: &str) -> bool {
    const DERIVED: &[&str] = &["E0301", "E0302", "E0401", "E0501", "E0601"];
    match root {
        // Resolution and signature failures poison later semantic products.
        "E0101" | "E0102" | "E0201" | "E0202" | "E0203" => DERIVED.contains(&candidate),
        // Keep independent resolution/signature diagnostics, but suppress
        // products which consume an already-invalid checked body.
        "E0301" | "E0302" => matches!(candidate, "E0401" | "E0501" | "E0601"),
        "E0401" => matches!(candidate, "E0501" | "E0601"),
        "E0501" => candidate == "E0601",
        _ => false,
    }
}

pub(super) fn closure_safety_diagnostics(
    db: &QueryDb<CompilerContext>,
    checked_modules: &[Arc<CheckedModule>],
) -> QueryResult<Vec<ProgramDiagnostic>> {
    let check = closure_safety_check(db, checked_modules)?;
    Ok(check
        .diagnostics
        .into_iter()
        .map(|diagnostic| ProgramDiagnostic {
            path: checked_modules
                .iter()
                .find(|module| module.id == diagnostic.owner.module_id)
                .map(|module| module.path.clone())
                .unwrap_or_else(synthetic_diagnostic_path),
            diagnostic: diagnostic.diagnostic,
        })
        .collect())
}

fn closure_support_modules(
    db: &QueryDb<CompilerContext>,
    roots: &[nia_closure_check::ClosureCheckFunction<'_>],
    checked_modules: &[Arc<CheckedModule>],
) -> QueryResult<(Vec<Arc<CheckedModule>>, HashSet<GlobalDefId>)> {
    let mut modules = checked_modules
        .iter()
        .map(|module| (module.id, Arc::clone(module)))
        .collect::<HashMap<_, _>>();
    let mut pending = roots
        .iter()
        .map(|function| function.def_id)
        .collect::<Vec<_>>();
    let mut functions = HashSet::new();

    while let Some(def_id) = pending.pop() {
        if !functions.insert(def_id) {
            continue;
        }
        if let std::collections::hash_map::Entry::Vacant(entry) = modules.entry(def_id.module_id) {
            entry.insert(db.get(CheckedModuleQuery(def_id.module_id))?);
        }
        let Some(module) = modules.get(&def_id.module_id) else {
            return Err(QueryError::internal(format!(
                "closure support module {:?} was not materialized",
                def_id.module_id
            )));
        };
        if !module.body_ir.function_bodies.contains_key(&def_id) {
            continue;
        }
        let empty_refs = nia_executable_facts::ExecutableModuleRefs::default();
        let input = nia_executable_facts::ReachableModuleInput {
            module_id: module.id,
            defs: &module.defs,
            type_store: &db.context().type_store,
            body_ir: &module.body_ir,
            executable_refs: &empty_refs,
            semantic_facts: &module.semantic_facts,
        };
        let selected = HashSet::from([def_id]);
        let refs =
            nia_executable_facts::executable_refs_for_items(&input, &selected, &HashSet::new());
        pending.extend(refs.functions);
    }

    let mut modules = modules.into_values().collect::<Vec<_>>();
    modules.sort_by_key(|module| module.id);
    Ok((modules, functions))
}

pub(in crate::query) fn closure_safety_check(
    db: &QueryDb<CompilerContext>,
    checked_modules: &[Arc<CheckedModule>],
) -> QueryResult<nia_closure_check::ClosureCheck> {
    let functions = checked_modules
        .iter()
        .flat_map(|module| {
            module.body_ir.function_bodies.iter().map(|(def_id, body)| {
                nia_closure_check::ClosureCheckFunction {
                    def_id: *def_id,
                    body,
                }
            })
        })
        .collect::<Vec<_>>();
    let closure_modules = functions
        .iter()
        .filter(|function| {
            nia_closure_check::contains_closure_constructs(std::slice::from_ref(function))
        })
        .map(|function| function.def_id.module_id)
        .collect::<HashSet<_>>();
    if closure_modules.is_empty() {
        return Ok(nia_closure_check::ClosureCheck {
            summaries: HashMap::new(),
            diagnostics: Vec::new(),
        });
    }
    let functions = functions
        .into_iter()
        .filter(|function| closure_modules.contains(&function.def_id.module_id))
        .collect::<Vec<_>>();
    let (support_modules, support_function_ids) =
        closure_support_modules(db, &functions, checked_modules)?;
    let support_functions = support_modules
        .iter()
        .flat_map(|module| {
            module
                .body_ir
                .function_bodies
                .iter()
                .filter(|(def_id, _)| support_function_ids.contains(def_id))
                .map(|(def_id, body)| nia_closure_check::ClosureCheckFunction {
                    def_id: *def_id,
                    body,
                })
        })
        .collect::<Vec<_>>();
    Ok(nia_closure_check::check_closure_safety_with_support(
        &functions,
        &support_functions,
        &db.context().type_store,
    )?)
}

pub(super) fn monomorphization_diagnostics(
    checked_modules: &[Arc<CheckedModule>],
    diagnostics: &[Diagnostic],
) -> Vec<ProgramDiagnostic> {
    diagnostics
        .iter()
        .cloned()
        .map(|diagnostic| ProgramDiagnostic {
            path: path_for_diagnostic_span(
                checked_modules,
                diagnostic.primary_span().unwrap_or_default(),
            ),
            diagnostic,
        })
        .collect()
}

pub(super) fn backend_lowering_diagnostics(
    checked_modules: &[Arc<CheckedModule>],
    diagnostics: &[Diagnostic],
) -> Vec<ProgramDiagnostic> {
    diagnostics
        .iter()
        .cloned()
        .map(|diagnostic| ProgramDiagnostic {
            path: path_for_diagnostic_span(
                checked_modules,
                diagnostic.primary_span().unwrap_or_default(),
            ),
            diagnostic,
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::{DiagnosticGate, suppresses_downstream};
    use nia_diagnostic::{Diagnostic, Severity, codes};
    use nia_source::SourcePath;
    use nia_span::Span;

    #[test]
    fn phase_gate_suppresses_derived_errors_but_keeps_independent_roots() {
        let path = SourcePath::new("main.nia");
        let mut diagnostics = Vec::new();
        let mut gate = DiagnosticGate::default();
        gate.append(
            &mut diagnostics,
            &path,
            &[Diagnostic::user_error_at(
                codes::NAME_RESOLUTION,
                Span::new(0, 1),
                "unknown name",
            )],
        );
        gate.append(
            &mut diagnostics,
            &path,
            &[
                Diagnostic::user_error_at(codes::TYPE_CHECK, Span::new(2, 3), "derived type error"),
                Diagnostic::user_error_at(
                    codes::NAME_RESOLUTION,
                    Span::new(4, 5),
                    "independent constraint error",
                ),
            ],
        );
        assert_eq!(diagnostics.len(), 2);
        assert!(suppresses_downstream("E0201", "E0301"));
        assert!(!suppresses_downstream("E0201", "E0304"));
        assert!(!suppresses_downstream("E0301", "E0201"));
        assert_eq!(diagnostics[1].diagnostic.severity, Severity::Error);
    }

    #[test]
    fn phase_gate_deduplicates_same_error_site_but_keeps_distinct_sites() {
        let path = SourcePath::new("main.nia");
        let mut diagnostics = Vec::new();
        let mut gate = DiagnosticGate::default();
        gate.append(
            &mut diagnostics,
            &path,
            &[Diagnostic::user_error_at(
                codes::TYPE_CHECK,
                Span::new(8, 12),
                "type mismatch from body checking",
            )],
        );
        gate.append(
            &mut diagnostics,
            &path,
            &[
                Diagnostic::user_error_at(
                    codes::TYPE_CHECK,
                    Span::new(8, 12),
                    "type mismatch from const checking",
                ),
                Diagnostic::user_error_at(
                    codes::TYPE_CHECK,
                    Span::new(20, 24),
                    "independent type mismatch",
                ),
            ],
        );
        assert_eq!(diagnostics.len(), 2);
        assert_eq!(
            diagnostics[0].diagnostic.primary_span(),
            Some(Span::new(8, 12))
        );
        assert_eq!(
            diagnostics[1].diagnostic.primary_span(),
            Some(Span::new(20, 24))
        );
    }
}
