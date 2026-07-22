// SPDX-License-Identifier: GPL-3.0-or-later
use super::*;

pub(super) fn provide_monomorphization(
    db: &QueryDb<CompilerContext>,
) -> nia_monomorphize::Monomorphization {
    time_provider(db.context().timings(), "monomorphization", || {
        let checked_modules = checked_modules_for_codegen(db);
        monomorphization_for_checked_modules(db, &checked_modules)
    })
}

pub(super) fn monomorphization_for_checked_modules(
    db: &QueryDb<CompilerContext>,
    checked_modules: &[Arc<CheckedModule>],
) -> nia_monomorphize::Monomorphization {
    let executable_signatures = executable_program_non_function_signatures(db);
    let program_enums = &executable_signatures.enums;
    let trait_impls = executable_signatures.trait_impls.as_slice();
    let trait_impl_index = &executable_signatures.trait_impl_index;
    let local_signatures = checked_modules
        .iter()
        .map(|module| (module.id, db.get(ItemSignaturesQuery(module.id))))
        .collect::<HashMap<_, _>>();
    let _function_bodies = function_bodies_from_checked_modules(db, checked_modules);
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
    nia_monomorphize::collect_monomorphizations(
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
    )
}

pub(super) fn checked_modules_for_codegen(
    db: &QueryDb<CompilerContext>,
) -> Vec<Arc<CheckedModule>> {
    db.get(ExecutableCheckedModulesQuery).as_ref().clone()
}

pub(super) fn checked_modules_for_diagnostics(
    db: &QueryDb<CompilerContext>,
) -> Vec<Arc<CheckedModule>> {
    db.get(ExecutableCheckedModulesQuery).as_ref().clone()
}

pub(super) fn materialize_checked_modules(
    db: &QueryDb<CompilerContext>,
    module_ids: Vec<ModuleId>,
) -> Vec<Arc<CheckedModule>> {
    db.get_many(module_ids.into_iter().map(CheckedModuleQuery))
}

fn function_bodies_from_checked_modules(
    db: &QueryDb<CompilerContext>,
    checked_modules: &[Arc<CheckedModule>],
) -> Vec<LoweredFunctionBodyHandle> {
    time_provider(
        db.context().timings(),
        "function_bodies_from_checked_modules",
        || {
            let mut def_ids = checked_modules
                .iter()
                .flat_map(|module| module.body_ir.function_bodies.keys().copied())
                .collect::<Vec<_>>();
            def_ids.sort_unstable();
            let lowered = db.get_many(def_ids.iter().copied().map(LoweredFunctionBodyQuery));
            def_ids
                .into_iter()
                .zip(lowered)
                .map(|(def_id, value)| LoweredFunctionBodyHandle { def_id, value })
                .collect()
        },
    )
}

fn static_inits_from_checked_modules(
    db: &QueryDb<CompilerContext>,
    checked_modules: &[Arc<CheckedModule>],
) -> Vec<StaticInitHandle> {
    time_provider(
        db.context().timings(),
        "static_inits_from_checked_modules",
        || {
            let mut def_ids = checked_modules
                .iter()
                .flat_map(|module| module.body_ir.global_inits.keys().copied())
                .collect::<Vec<_>>();
            def_ids.sort_unstable();
            let inits = db.get_many(def_ids.iter().copied().map(ExecutableStaticInitQuery));
            def_ids
                .into_iter()
                .zip(inits)
                .map(|(def_id, value)| StaticInitHandle { def_id, value })
                .collect()
        },
    )
}

pub(in crate::query) fn provide_lowered_function_body(
    db: &QueryDb<CompilerContext>,
    def_id: GlobalDefId,
) -> LoweredFunctionBodyValue {
    let checked_body = db.get(ExecutableFunctionBodyQuery(def_id));
    let Some(body) = checked_body.as_ref() else {
        return LoweredFunctionBodyValue::Diagnostic(
            nia_function_lower::FunctionLoweringDiagnostic {
                span: Span::default(),
                message: format!("missing executable checked function body for {def_id:?}"),
            },
        );
    };
    match nia_function_lower::lower_function_body(
        def_id.module_id,
        body,
        nia_function_lower::FunctionTypeContext::for_module(
            &db.context().type_store,
            def_id.module_id,
        ),
    ) {
        Ok(lowered) => LoweredFunctionBodyValue::Body(lowered.body),
        Err(diagnostic) => LoweredFunctionBodyValue::Diagnostic(diagnostic),
    }
}

pub(super) fn provide_backend_lowering(
    db: &QueryDb<CompilerContext>,
) -> nia_backend_lower::BackendLowering {
    time_provider(db.context().timings(), "backend_lowering", || {
        provide_backend_lowering_inner(db)
    })
}

fn provide_backend_lowering_inner(
    db: &QueryDb<CompilerContext>,
) -> nia_backend_lower::BackendLowering {
    let checked_modules = checked_modules_for_codegen(db);
    let monomorphization = db.get(MonomorphizationQuery);
    provide_backend_lowering_inner_for_modules(db, &monomorphization, &checked_modules)
}

pub(super) fn provide_backend_lowering_inner_for_modules(
    db: &QueryDb<CompilerContext>,
    monomorphization: &nia_monomorphize::Monomorphization,
    checked_modules: &[Arc<CheckedModule>],
) -> nia_backend_lower::BackendLowering {
    let (
        all_visible_extensions,
        active_item_trees,
        item_signatures,
        const_array_lengths,
        const_enum_values,
        visible_extensions,
        extension_methods,
        function_bodies,
        static_inits,
    ) = time_provider(db.context().timings(), "backend_lowering.inputs", || {
        let timings = db.context().timings();
        let all_visible_extensions = time_provider(
            timings,
            "backend_lowering.inputs.all_visible_extensions",
            || {
                checked_modules
                    .iter()
                    .map(|module| (module.id, db.get(VisibleExtensionsQuery(module.id))))
                    .collect::<Vec<_>>()
            },
        );
        let active_item_trees =
            time_provider(timings, "backend_lowering.inputs.active_item_trees", || {
                checked_modules
                    .iter()
                    .map(|checked_module| db.get(FullActiveModuleItemTreeQuery(checked_module.id)))
                    .collect::<Vec<_>>()
            });
        let item_signatures =
            time_provider(timings, "backend_lowering.inputs.item_signatures", || {
                checked_modules
                    .iter()
                    .map(|checked_module| {
                        body_local_item_signatures(
                            db,
                            checked_module.id,
                            &checked_module.type_lowering,
                        )
                    })
                    .collect::<Vec<_>>()
            });
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
            || {
                checked_modules
                    .iter()
                    .map(|checked_module| db.get(VisibleExtensionsQuery(checked_module.id)))
                    .collect::<Vec<_>>()
            },
        );
        let extension_methods =
            time_provider(timings, "backend_lowering.inputs.extension_methods", || {
                db.get(ExtensionMethodIndexQuery)
            });
        let function_bodies = function_bodies_from_checked_modules(db, checked_modules);
        let static_inits = static_inits_from_checked_modules(db, checked_modules);
        (
            all_visible_extensions,
            active_item_trees,
            item_signatures,
            const_array_lengths,
            const_enum_values,
            visible_extensions,
            extension_methods,
            function_bodies,
            static_inits,
        )
    });
    let function_lowering_diagnostics =
        function_lowering_diagnostics(checked_modules, &function_bodies);
    if !function_lowering_diagnostics.is_empty() {
        return nia_backend_lower::BackendLowering {
            diagnostics: function_lowering_diagnostics
                .into_iter()
                .map(|program_diagnostic| program_diagnostic.diagnostic)
                .collect(),
            ..empty_backend_lowering(*db.get(CompilerOptimizationQuery))
        };
    }
    let indexes = time_provider(db.context().timings(), "backend_lowering.indexes", || {
        build_backend_lowering_indexes(
            &all_visible_extensions,
            checked_modules,
            &const_array_lengths,
            &function_bodies,
            &static_inits,
        )
    });
    let program_defs = |module_id| Some(db.get(FullModuleDefsQuery(module_id)));
    let executable_program_signatures = executable_program_non_function_signatures(db);
    let executable_program_functions = executable_program_functions_for_modules(
        db,
        checked_modules.iter().map(|module| module.id),
    );
    let program_signatures =
        executable_program_signatures.codegen_maps_with_functions(&executable_program_functions);
    let symbols = db.context().symbols();
    let inputs = time_provider(
        db.context().timings(),
        "backend_lowering.module_inputs",
        || {
            build_backend_lowering_module_inputs(BackendLoweringModuleInputsInput {
                symbols: &symbols,
                checked_modules,
                runtime: *db.get(CompilerRuntimeQuery),
                active_item_trees: &active_item_trees,
                item_signatures: &item_signatures,
                const_array_lengths: &const_array_lengths,
                const_enum_values: &const_enum_values,
                visible_extensions: &visible_extensions,
                extension_methods: &extension_methods.methods,
                program_defs: &program_defs,
                program_signatures,
                indexes: &indexes,
            })
        },
    );
    time_provider(
        db.context().timings(),
        "backend_lowering.lower_backend_program",
        || {
            nia_backend_lower::lower_backend_program_with_timings(
                &inputs,
                &db.context().type_store,
                monomorphization,
                *db.get(CompilerOptimizationQuery),
                db.context().timings(),
            )
        },
    )
}

pub(super) fn early_program_diagnostics(db: &QueryDb<CompilerContext>) -> Vec<ProgramDiagnostic> {
    let mut diagnostics = db.get(ProgramLoadDiagnosticsQuery).as_ref().clone();
    let loaded_modules = resolve_stable_module_sequence(db, &db.get(LoadedModulesQuery));
    for module_id in loaded_modules {
        let parse_errors = db.get(ModuleParseErrorsQuery(module_id));
        let path = db.get(ModulePathQuery(module_id));
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
    let public_surfaces = db.get(PublicSurfacesQuery);
    let public_using_scopes = db.get(PublicUsingScopesQuery);
    for (module_id, diagnostic) in public_surfaces
        .diagnostics
        .iter()
        .chain(public_using_scopes.diagnostics.iter())
    {
        diagnostics.push(ProgramDiagnostic {
            path: db.get(ModulePathQuery(*module_id)).as_ref().clone(),
            diagnostic: diagnostic.clone(),
        });
    }
    diagnostics
}

pub(super) fn checked_module_diagnostics(
    db: &QueryDb<CompilerContext>,
    checked_modules: &[Arc<CheckedModule>],
) -> Vec<ProgramDiagnostic> {
    let mut diagnostics = Vec::new();
    for checked in checked_modules {
        diagnostics.extend(module_diagnostics(&checked.path, &checked.defs.diagnostics));
        diagnostics.extend(module_diagnostics(
            &checked.path,
            &checked.type_resolution.diagnostics,
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
        let item_signatures = db.get(ItemSignaturesQuery(checked.id));
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
        diagnostics.extend(module_diagnostics(&checked.path, &checked.body_diagnostics));
        let extension_validation = db.get(ExtensionProviderValidationFactsQuery(checked.id));
        diagnostics.extend(module_diagnostics(
            &checked.path,
            &extension_validation.diagnostics,
        ));
        let extension_provider = db.get(ExtensionProviderModuleFactsQuery(checked.id));
        diagnostics.extend(module_diagnostics(
            &checked.path,
            &extension_provider.associated_value_diagnostics,
        ));
    }
    diagnostics
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
