// SPDX-License-Identifier: GPL-3.0-or-later
use super::*;

#[cfg(test)]
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
    checked_modules: &[CheckedModule],
) -> nia_monomorphize::Monomorphization {
    let executable_signatures = executable_program_non_function_signatures(db);
    let program_enums = &executable_signatures.enums;
    let trait_impls = executable_signatures.trait_impls.as_slice();
    let trait_impl_index = &executable_signatures.trait_impl_index;
    let local_signatures = checked_modules
        .iter()
        .map(|module| (module.id, db.query(ItemSignaturesQuery(module.id))))
        .collect::<HashMap<_, _>>();
    let _function_bodies = function_bodies_from_checked_modules(db, checked_modules);
    let function_interners = checked_modules
        .iter()
        .map(|module| {
            (
                module.id,
                db.context().type_store.module_snapshot(module.id),
            )
        })
        .collect::<HashMap<_, _>>();
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
                    interner: function_interners
                        .get(&module.id)
                        .expect("function lowering interner snapshot"),
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
    )
}

pub(super) fn checked_modules_for_codegen(db: &QueryDb<CompilerContext>) -> Vec<CheckedModule> {
    materialize_executable_checked_modules(db, db.query(ExecutableCheckedModuleSetQuery))
}

pub(super) fn checked_modules_for_diagnostics(db: &QueryDb<CompilerContext>) -> Vec<CheckedModule> {
    materialize_executable_checked_modules(db, db.query(ExecutableCheckedModuleSetQuery))
}

pub(super) fn materialize_checked_modules(
    db: &QueryDb<CompilerContext>,
    module_ids: Vec<ModuleId>,
) -> Vec<CheckedModule> {
    db.query_many(module_ids.into_iter().map(CheckedModuleQuery))
}

fn materialize_executable_checked_modules(
    db: &QueryDb<CompilerContext>,
    set: ExecutableCheckedModuleSet,
) -> Vec<CheckedModule> {
    db.context().executable_checked_modules(&set)
}

fn function_bodies_from_checked_modules(
    db: &QueryDb<CompilerContext>,
    checked_modules: &[CheckedModule],
) -> Vec<LoweredFunctionBodies> {
    time_provider(
        db.context().timings(),
        "function_bodies_from_checked_modules",
        || {
            checked_modules
                .iter()
                .map(|module| {
                    let lowered = db
                        .context()
                        .type_store
                        .with_module_interner_for_semantic_migration(module.id, |interner| {
                            assert!(
                                module.body_ir.interner.is_prefix_of(interner),
                                "Nia ICE: function lowering input is not a prefix of its session type store"
                            );
                            nia_function_lower::lower_function_bodies_with_interner(
                                module.id,
                                module.body_ir.function_bodies.iter(),
                                interner,
                            )
                            .unwrap_or_else(|diagnostics| {
                                nia_function_lower::LoweredFunctionBodies {
                                    bodies: HashMap::new(),
                                    diagnostics,
                                }
                            })
                        });
                    LoweredFunctionBodies {
                        bodies: lowered.bodies,
                        diagnostics: lowered.diagnostics,
                    }
                })
                .collect()
        },
    )
}

#[cfg(test)]
pub(super) fn provide_backend_lowering(
    db: &QueryDb<CompilerContext>,
) -> nia_backend_lower::BackendLowering {
    time_provider(db.context().timings(), "backend_lowering", || {
        provide_backend_lowering_inner(db)
    })
}

#[cfg(test)]
fn provide_backend_lowering_inner(
    db: &QueryDb<CompilerContext>,
) -> nia_backend_lower::BackendLowering {
    let checked_modules = checked_modules_for_codegen(db);
    let monomorphization = db.query(MonomorphizationQuery);
    provide_backend_lowering_inner_for_modules(db, &monomorphization, &checked_modules)
}

pub(super) fn provide_backend_lowering_inner_for_modules(
    db: &QueryDb<CompilerContext>,
    monomorphization: &nia_monomorphize::Monomorphization,
    checked_modules: &[CheckedModule],
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
        function_interners,
    ) = time_provider(db.context().timings(), "backend_lowering.inputs", || {
        let timings = db.context().timings();
        let all_visible_extensions = time_provider(
            timings,
            "backend_lowering.inputs.all_visible_extensions",
            || {
                checked_modules
                    .iter()
                    .map(|module| (module.id, db.query(VisibleExtensionsQuery(module.id))))
                    .collect::<Vec<_>>()
            },
        );
        let active_item_trees =
            time_provider(timings, "backend_lowering.inputs.active_item_trees", || {
                checked_modules
                    .iter()
                    .map(|checked_module| {
                        db.query(FullActiveModuleItemTreeQuery(checked_module.id))
                    })
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
                    .map(|checked_module| db.query(VisibleExtensionsQuery(checked_module.id)))
                    .collect::<Vec<_>>()
            },
        );
        let extension_methods =
            time_provider(timings, "backend_lowering.inputs.extension_methods", || {
                db.query(ExtensionMethodIndexQuery)
            });
        let function_bodies = function_bodies_from_checked_modules(db, checked_modules);
        let function_interners = checked_modules
            .iter()
            .map(|module| {
                (
                    module.id,
                    db.context().type_store.module_snapshot(module.id),
                )
            })
            .collect::<HashMap<_, _>>();
        (
            all_visible_extensions,
            active_item_trees,
            item_signatures,
            const_array_lengths,
            const_enum_values,
            visible_extensions,
            extension_methods,
            function_bodies,
            function_interners,
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
            ..empty_backend_lowering(db.query(CompilerOptimizationQuery))
        };
    }
    let indexes = time_provider(db.context().timings(), "backend_lowering.indexes", || {
        build_backend_lowering_indexes(
            &all_visible_extensions,
            checked_modules,
            &const_array_lengths,
            &function_bodies,
            &function_interners,
        )
    });
    let program_defs = |module_id| Some(db.query_shared(FullModuleDefsQuery(module_id)));
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
                runtime: db.query(CompilerRuntimeQuery),
                active_item_trees: &active_item_trees,
                item_signatures: &item_signatures,
                const_array_lengths: &const_array_lengths,
                const_enum_values: &const_enum_values,
                visible_extensions: &visible_extensions,
                function_bodies: &function_bodies,
                function_interners: &function_interners,
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
                monomorphization,
                db.query(CompilerOptimizationQuery),
                db.context().timings(),
            )
        },
    )
}

pub(super) fn early_program_diagnostics(db: &QueryDb<CompilerContext>) -> Vec<ProgramDiagnostic> {
    let mut diagnostics = db.query(ProgramLoadDiagnosticsQuery);
    for module_id in db.query(LoadedModulesQuery) {
        let parse_errors = db.query(ModuleParseErrorsQuery(module_id));
        let path = db.query(ModulePathQuery(module_id));
        for error in &parse_errors {
            diagnostics.push(ProgramDiagnostic {
                path: path.clone(),
                diagnostic: Diagnostic::user_error_at(
                    codes::PARSE,
                    error.span,
                    error.message.clone(),
                ),
            });
        }
    }
    let public_surfaces = db.query(PublicSurfacesQuery);
    let public_using_scopes = db.query(PublicUsingScopesQuery);
    for (module_id, diagnostic) in public_surfaces
        .diagnostics
        .iter()
        .chain(public_using_scopes.diagnostics.iter())
    {
        diagnostics.push(ProgramDiagnostic {
            path: db.query(ModulePathQuery(*module_id)),
            diagnostic: diagnostic.clone(),
        });
    }
    diagnostics
}

pub(super) fn checked_module_diagnostics(
    db: &QueryDb<CompilerContext>,
    checked_modules: &[CheckedModule],
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
        let item_signatures = db.query(ItemSignaturesQuery(checked.id));
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
        let extension_validation = db.query(ExtensionProviderValidationFactsQuery(checked.id));
        diagnostics.extend(module_diagnostics(
            &checked.path,
            &extension_validation.diagnostics,
        ));
        let extension_provider = db.query(ExtensionProviderModuleFactsQuery(checked.id));
        diagnostics.extend(module_diagnostics(
            &checked.path,
            &extension_provider.associated_value_diagnostics,
        ));
    }
    diagnostics
}

pub(super) fn monomorphization_diagnostics(
    checked_modules: &[CheckedModule],
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
    checked_modules: &[CheckedModule],
    function_bodies: &[LoweredFunctionBodies],
) -> Vec<ProgramDiagnostic> {
    checked_modules
        .iter()
        .zip(function_bodies.iter())
        .flat_map(|(module, lowered)| {
            lowered
                .diagnostics
                .iter()
                .map(|diagnostic| ProgramDiagnostic {
                    path: module.path.clone(),
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
    checked_modules: &[CheckedModule],
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
