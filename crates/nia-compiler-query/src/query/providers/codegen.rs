// SPDX-License-Identifier: GPL-3.0-or-later
use super::*;

pub(in crate::query) fn provide_backend_module_source_item_plan(
    db: &QueryDb<CompilerContext>,
    module_id: ModuleId,
) -> QueryResult<BackendModuleSourceItemPlan> {
    let facts = db.get(ExecutableCheckedModuleFactsQuery)?;
    let module = facts
        .modules
        .iter()
        .find(|module| module.id == module_id)
        .unwrap_or_else(|| panic!("Nia ICE: missing executable facts for module {module_id:?}"));
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
    assert!(
        facts.modules.iter().any(|module| module.id == module_id),
        "Nia ICE: missing executable facts for module {module_id:?}"
    );
    let monomorphization = db.get(MonomorphizationQuery)?;
    let mut instances = monomorphization
        .instances
        .iter()
        .filter(|instance| instance.def_id.module_id == module_id)
        .collect::<Vec<_>>();
    instances.sort_by(|left, right| left.symbol.cmp(&right.symbol));
    let mut seen = HashSet::new();
    let instances = instances
        .into_iter()
        .map(|instance| {
            let key = (
                instance.def_id,
                instance.arg_module_id,
                instance.self_arg,
                instance.args.clone(),
                instance.const_args.clone(),
            );
            assert!(
                seen.insert(key),
                "Nia ICE: duplicate monomorphized function instance `{}`",
                instance.symbol
            );
            nia_backend_lower::BackendFunctionInstancePlan {
                def_id: instance.def_id,
                arg_module_id: instance.arg_module_id,
                self_arg: instance.self_arg,
                args: instance.args.clone(),
                const_args: instance.const_args.clone(),
                span: instance.span,
            }
        })
        .collect();
    Ok(BackendModuleFunctionInstancePlan { instances })
}

pub(super) fn provide_monomorphization(
    db: &QueryDb<CompilerContext>,
) -> QueryResult<nia_monomorphize::Monomorphization> {
    time_provider(db.context().timings(), "monomorphization", || {
        let checked_modules = checked_modules_for_codegen(db)?;
        monomorphization_for_checked_modules(db, &checked_modules)
    })
}

pub(super) fn monomorphization_for_checked_modules(
    db: &QueryDb<CompilerContext>,
    checked_modules: &[Arc<CheckedModule>],
) -> QueryResult<nia_monomorphize::Monomorphization> {
    let executable_signatures = executable_program_non_function_signatures(db)?;
    let program_enums = &executable_signatures.enums;
    let trait_impls = executable_signatures.trait_impls.as_slice();
    let trait_impl_index = &executable_signatures.trait_impl_index;
    let local_signatures = checked_modules
        .iter()
        .map(|module| Ok((module.id, db.get(ItemSignaturesQuery(module.id))?)))
        .collect::<QueryResult<HashMap<_, _>>>()?;
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
    Ok(nia_monomorphize::collect_monomorphizations(
        &checked_modules
            .iter()
            .zip(semantic_instantiations.iter())
            .map(
                |(module, semantic_instantiations)| MonomorphizeModuleInput {
                    module_id: module.id,
                    defs: &module.defs,
                    normalization: &module.type_normalization,
                    const_eval: &module.const_eval,
                    const_expr_summaries: &module.type_lowering.const_expr_summaries,
                    layouts: Some(&module.layouts),
                    local_enums: &local_signatures
                        .get(&module.id)
                        .expect("monomorphization signatures must exist for checked module")
                        .enums,
                    program_enums,
                    trait_impls,
                    trait_impl_index,
                    instantiations: semantic_instantiations,
                },
            )
            .collect::<Vec<_>>(),
        &db.context().type_store,
    ))
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
    module_ids
        .into_iter()
        .map(|module_id| db.get(CheckedModuleQuery(module_id)))
        .collect()
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
            let lowered = def_ids
                .iter()
                .copied()
                .map(|def_id| db.get(LoweredFunctionBodyQuery(def_id)))
                .collect::<QueryResult<Vec<_>>>()?;
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
    let Some(body) = checked_body.as_ref() else {
        return Ok(LoweredFunctionBodyValue::Diagnostic(
            nia_function_lower::FunctionLoweringDiagnostic {
                span: Span::default(),
                message: format!("missing executable checked function body for {def_id:?}"),
            },
        ));
    };
    match nia_function_lower::lower_function_body(
        def_id.module_id,
        body,
        nia_function_lower::FunctionTypeContext::for_module(
            &db.context().type_store,
            def_id.module_id,
        ),
    ) {
        Ok(lowered) => Ok(LoweredFunctionBodyValue::Body(lowered.body)),
        Err(diagnostic) => Ok(LoweredFunctionBodyValue::Diagnostic(diagnostic)),
    }
}

pub(super) fn provide_backend_lowering(
    db: &QueryDb<CompilerContext>,
) -> QueryResult<nia_backend_lower::BackendLowering> {
    time_provider(db.context().timings(), "backend_lowering", || {
        provide_backend_lowering_inner(db)
    })
}

pub(in crate::query) fn provide_backend_item_plan(
    db: &QueryDb<CompilerContext>,
) -> QueryResult<nia_backend_lower::BackendItemPlan> {
    time_provider(db.context().timings(), "backend_item_plan", || {
        let inputs = db.get(BackendLoweringInputsQuery)?;
        let optimization = *db.get(CompilerOptimizationQuery)?;
        match inputs.as_ref() {
            Ok(inputs) => {
                let module_inputs = inputs.module_inputs();
                Ok(nia_backend_lower::plan_backend_program_with_timings(
                    &module_inputs,
                    &db.context().type_store,
                    optimization,
                    db.context().timings(),
                ))
            }
            Err(diagnostics) => Ok(nia_backend_lower::BackendItemPlan::from_diagnostics(
                optimization,
                diagnostics.clone(),
            )),
        }
    })
}

pub(in crate::query) fn provide_backend_finalization_task_context(
    db: &QueryDb<CompilerContext>,
) -> QueryResult<BackendFinalizationTaskContext> {
    Ok(BackendFinalizationTaskContext::new(
        db.get(BackendLoweringInputsQuery)?,
        Arc::clone(&db.context().type_store),
        *db.get(CompilerOptimizationQuery)?,
        db.context().timings(),
    ))
}

pub(in crate::query) fn provide_backend_module_finalization(
    db: &QueryDb<CompilerContext>,
    key: BackendModuleFinalizationQuery,
) -> QueryResult<nia_backend_lower::BackendModuleFinalization> {
    let context = db.get(BackendFinalizationTaskContextQuery)?;
    let module_plan = db.get_owned(BackendModuleItemPlanQuery(key.module_id))?;
    Ok(context.finalize_module(key.position, key.module_id, module_plan))
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
    lowering?
}

pub(in crate::query) fn with_backend_finalization_schedule<R>(
    db: &QueryDb<CompilerContext>,
    consume: impl for<'borrow, 'stream, 'executor> FnOnce(
        Result<
            crate::BackendFinalizationSchedule<'borrow, 'stream, 'executor>,
            nia_backend_lower::BackendLowering,
        >,
    ) -> R,
) -> QueryResult<R> {
    let plan = db.get_owned(BackendItemPlanQuery)?;
    emit_backend_module_plan_allocation("before_publish");
    let has_diagnostics = !plan.diagnostics().is_empty();
    let (finalization, module_plans) = plan.into_module_plans();
    let module_ids = module_plans
        .iter()
        .map(|module_plan| module_plan.module().id)
        .collect::<Vec<_>>();
    for (module_id, module_plan) in module_ids.iter().copied().zip(module_plans) {
        db.publish_owned(
            BackendModuleItemPlanQuery(module_id),
            module_plan,
            &BackendItemPlanQuery,
        );
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
        );
        return Ok(consume(Err(lowering)));
    }
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
                nia_backend_lower::BackendModuleFinalizationCollector::new(
                    finalization,
                    &module_ids,
                ),
            )))
        },
    );
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
) -> QueryResult<Result<BackendLoweringInputs, Vec<Diagnostic>>> {
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
                .map(|checked_module| nia_const_check::ConstArrayLengths {
                    values: checked_module.const_eval.array_lengths.clone(),
                    diagnostics: checked_module.const_eval.diagnostics.clone(),
                })
                .collect::<Vec<_>>();
            let const_enum_values = checked_modules
                .iter()
                .map(|checked_module| nia_const_check::ConstEnumValues {
                    values: checked_module.const_eval.enum_values.clone(),
                    typed_values: checked_module.const_eval.typed_enum_values.clone(),
                    diagnostics: checked_module.const_eval.diagnostics.clone(),
                })
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
                .map(|module| db.get(FullModuleDefsQuery(module.id)))
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
    let function_lowering_diagnostics =
        function_lowering_diagnostics(&checked_modules, &function_bodies);
    if !function_lowering_diagnostics.is_empty() {
        return Ok(Err(function_lowering_diagnostics
            .into_iter()
            .map(|program_diagnostic| program_diagnostic.diagnostic)
            .collect()));
    }
    let non_function_signatures = executable_program_non_function_signatures(db)?;
    let functions = executable_program_functions_for_modules(
        db,
        checked_modules.iter().map(|module| module.id),
    )?;
    let runtime = *db.get(CompilerRuntimeQuery)?;
    let inputs = time_provider(
        db.context().timings(),
        "backend_lowering.module_inputs",
        || {
            BackendLoweringInputs::new(BackendLoweringInputsParts {
                symbols: db.context().symbols(),
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
    );
    Ok(Ok(inputs))
}

pub(super) fn early_program_diagnostics(
    db: &QueryDb<CompilerContext>,
) -> QueryResult<Vec<ProgramDiagnostic>> {
    let mut diagnostics = db.get(ProgramLoadDiagnosticsQuery)?.as_ref().clone();
    let loaded_modules = db.get(LoadedModulesQuery)?;
    let _graph = db.get(ModuleGraphQuery)?;
    let loaded_modules = resolve_stable_module_sequence_from_current_inputs(db, &loaded_modules)?;
    for module_id in loaded_modules {
        let parse_errors = db.get(ModuleParseErrorsQuery(module_id))?;
        let path = db.get(ModulePathQuery(module_id))?;
        for error in parse_errors.iter() {
            diagnostics.push(ProgramDiagnostic {
                path: path.as_ref().clone(),
                diagnostic: Diagnostic::user_error_at(
                    codes::PARSE,
                    error.span,
                    error.message.clone(),
                ),
            });
        }
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
            resolve_diagnostic_bundle(db.context(), &bundle.diagnostics),
        ));
    }
    Ok(diagnostics)
}

pub(super) fn checked_module_diagnostics(
    db: &QueryDb<CompilerContext>,
    checked_modules: &[Arc<CheckedModule>],
) -> QueryResult<Vec<ProgramDiagnostic>> {
    let mut diagnostics = Vec::new();
    for checked in checked_modules {
        diagnostics.extend(module_diagnostics(&checked.path, &checked.defs.diagnostics));
        diagnostics.extend(module_diagnostics(
            &checked.path,
            &checked.type_resolution.diagnostics,
        ));
        diagnostics.extend(module_diagnostics(
            &checked.path,
            resolve_diagnostic_bundle(db.context(), &checked.frontend_diagnostics),
        ));
        diagnostics.extend(module_diagnostics(
            &checked.path,
            &checked.type_lowering.diagnostics,
        ));
        diagnostics.extend(module_diagnostics(
            &checked.path,
            &checked.value_resolution.diagnostics,
        ));
        diagnostics.extend(module_diagnostics(
            &checked.path,
            &checked.local_resolution.diagnostics,
        ));
        let item_signatures = db.get(ItemSignaturesQuery(checked.id))?;
        diagnostics.extend(module_diagnostics(
            &checked.path,
            &item_signatures.diagnostics,
        ));
        diagnostics.extend(module_diagnostics(
            &checked.path,
            &checked.type_normalization.diagnostics,
        ));
        diagnostics.extend(module_diagnostics(
            &checked.path,
            &checked.const_eval.diagnostics,
        ));
        diagnostics.extend(module_diagnostics(
            &checked.path,
            &checked.static_check.diagnostics,
        ));
        diagnostics.extend(module_diagnostics(
            &checked.path,
            &checked.layouts.diagnostics,
        ));
        diagnostics.extend(module_diagnostics(
            &checked.path,
            &checked.abi_check.diagnostics,
        ));
        diagnostics.extend(module_diagnostics(
            &checked.path,
            &checked.flow_check.diagnostics,
        ));
        diagnostics.extend(module_diagnostics(
            &checked.path,
            resolve_diagnostic_bundle(db.context(), &checked.body_diagnostics),
        ));
        let extension_validation = db.get(ExtensionProviderValidationFactsQuery(checked.id))?;
        let extension_validation_diagnostics =
            resolve_diagnostic_bundle(db.context(), &extension_validation.diagnostics);
        diagnostics.extend(module_diagnostics(
            &checked.path,
            extension_validation_diagnostics,
        ));
        let extension_provider = db.get(ExtensionProviderModuleFactsQuery(checked.id))?;
        let associated_value_diagnostics = resolve_diagnostic_bundle(
            db.context(),
            &extension_provider.associated_value_diagnostics,
        );
        diagnostics.extend(module_diagnostics(
            &checked.path,
            associated_value_diagnostics,
        ));
    }
    Ok(diagnostics)
}

pub(super) fn monomorphization_diagnostics(
    checked_modules: &[Arc<CheckedModule>],
    monomorphization: &nia_monomorphize::Monomorphization,
) -> Vec<ProgramDiagnostic> {
    monomorphization
        .diagnostics
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

fn function_lowering_diagnostics(
    checked_modules: &[Arc<CheckedModule>],
    function_bodies: &[LoweredFunctionBodyHandle],
) -> Vec<ProgramDiagnostic> {
    let paths = checked_modules
        .iter()
        .map(|module| (module.id, module.path.clone()))
        .collect::<HashMap<_, _>>();
    function_bodies
        .iter()
        .filter_map(|lowered| {
            let diagnostic = lowered.value.diagnostic()?;
            Some(ProgramDiagnostic {
                path: paths
                    .get(&lowered.def_id.module_id)
                    .expect("lowered function module must have a checked module")
                    .clone(),
                diagnostic: Diagnostic::internal_error_at(
                    codes::INVALID_FUNCTION_IR,
                    diagnostic.span,
                    diagnostic.message.clone(),
                ),
            })
        })
        .collect()
}

pub(super) fn backend_lowering_diagnostics(
    checked_modules: &[Arc<CheckedModule>],
    backend_lowering: &nia_backend_lower::BackendLowering,
) -> Vec<ProgramDiagnostic> {
    backend_lowering
        .diagnostics
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
