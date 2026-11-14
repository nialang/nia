// SPDX-License-Identifier: GPL-3.0-or-later
use super::*;

pub(super) fn provide_checked_program(
    db: &QueryDb<CompilerContext>,
) -> QueryResult<CheckedProgramAnalysis> {
    time_provider(db.context().timings(), "checked_program", || {
        let graph = db.get(ModuleGraphQuery)?.as_ref().clone();
        let optimization = *db.get(CompilerOptimizationQuery)?;
        let mut diagnostics = early_program_diagnostics(db)?;
        let module_ids = db.get(CheckedModuleIdsQuery)?.as_ref().clone();
        let diagnostic_modules = materialize_checked_modules(db, module_ids)?;
        let (module_diagnostics, suppressed_downstream) =
            checked_module_diagnostics(db, &diagnostic_modules)?;
        diagnostics.extend(module_diagnostics);
        diagnostics.extend(deferred_const_instance_diagnostics(
            db,
            &diagnostic_modules,
            &diagnostics,
        )?);
        diagnostics.extend(time_provider(
            db.context().timings(),
            "checked_program.closure_safety",
            || closure_safety_diagnostics(db, &diagnostic_modules),
        )?);
        Ok(CheckedProgramAnalysis {
            diagnostic_sources: diagnostic_sources(db, &graph, &diagnostics)?,
            graph,
            optimization,
            modules: diagnostic_modules,
            diagnostics,
            suppressed_downstream,
        })
    })
}

pub(super) fn provide_entry_checked_program(
    db: &QueryDb<CompilerContext>,
) -> QueryResult<CheckedProgramAnalysis> {
    time_provider(db.context().timings(), "entry_checked_program", || {
        let graph = db.get(ModuleGraphQuery)?.as_ref().clone();
        let optimization = *db.get(CompilerOptimizationQuery)?;
        let mut diagnostics = early_program_diagnostics(db)?;
        let diagnostic_modules = checked_modules_for_diagnostics(db)?;
        let (module_diagnostics, suppressed_downstream) =
            checked_module_diagnostics(db, &diagnostic_modules)?;
        diagnostics.extend(module_diagnostics);
        diagnostics.extend(deferred_const_instance_diagnostics(
            db,
            &diagnostic_modules,
            &diagnostics,
        )?);
        diagnostics.extend(time_provider(
            db.context().timings(),
            "entry_checked_program.closure_safety",
            || closure_safety_diagnostics(db, &diagnostic_modules),
        )?);
        Ok(CheckedProgramAnalysis {
            diagnostic_sources: diagnostic_sources(db, &graph, &diagnostics)?,
            graph,
            optimization,
            modules: diagnostic_modules,
            diagnostics,
            suppressed_downstream,
        })
    })
}

fn deferred_const_instance_diagnostics(
    db: &QueryDb<CompilerContext>,
    modules: &[Arc<CheckedModule>],
    diagnostics: &[ProgramDiagnostic],
) -> QueryResult<Vec<ProgramDiagnostic>> {
    // A layout consumer can observe a failed closed computation before instance
    // validation. Its canonical type cache cannot own all source obligations:
    // equal types can arise from independent declarations. Continue validating
    // instances when these are the only failures, while all earlier semantic
    // failures still block instantiation. Use the owning diagnostic bundle,
    // never message text, to establish that distinction.
    if diagnostics.iter().any(|reported| {
        reported.diagnostic.severity() == nia_diagnostic::Severity::Error
            && !modules.iter().any(|module| {
                resolve_diagnostic_bundle(&module.layout_diagnostics)
                    .iter()
                    .filter(|diagnostic| diagnostic.code == nia_diagnostic::codes::CONST.into())
                    .any(|diagnostic| {
                        let path = diagnostic
                            .source_path
                            .as_deref()
                            .unwrap_or(module.path.as_str());
                        same_reported_diagnostic(path, diagnostic, reported)
                    })
            })
    }) {
        return Ok(Vec::new());
    }
    let has_computations = modules.iter().any(|module| {
        module
            .const_eval
            .symbolic_locals
            .values()
            .any(|arg| matches!(arg.value, nia_ty::ConstGenericValue::Computation(_)))
            || module
                .const_eval
                .symbolic_const_exprs
                .values()
                .any(|arg| matches!(arg.value, nia_ty::ConstGenericValue::Computation(_)))
            || module
                .semantic_facts
                .iter_node_array_repeat_counts()
                .any(|(_, len)| matches!(len, nia_ty::ArrayLenTy::Computation(_)))
            || module
                .semantic_facts
                .iter_generic_instantiations()
                .any(|inst| {
                    inst.const_args
                        .iter()
                        .any(|arg| matches!(arg.value, nia_ty::ConstGenericValue::Computation(_)))
                })
    });
    if !has_computations {
        return Ok(Vec::new());
    }
    // Declaration-wide checking also includes unrelated std bodies. Instance
    // obligations belong to the executable demand closure, shared with emission.
    let validation = db.get(MonomorphizationQuery)?;
    let mut validation =
        monomorphization_diagnostics(modules, resolve_diagnostic_bundle(&validation.diagnostics));
    validation.retain(|candidate| {
        !diagnostics.iter().any(|existing| {
            same_reported_diagnostic(candidate.path.as_str(), &candidate.diagnostic, existing)
        })
    });
    Ok(validation)
}

/// Program diagnostics move the source path out of the underlying diagnostic.
/// Compare the same complete report across that representation boundary.
fn same_reported_diagnostic(
    path: &str,
    diagnostic: &Diagnostic,
    reported: &ProgramDiagnostic,
) -> bool {
    if path != reported.path.as_str() {
        return false;
    }
    let mut diagnostic = diagnostic.clone();
    diagnostic
        .source_path
        .clone_from(&reported.diagnostic.source_path);
    diagnostic == reported.diagnostic
}

pub(in crate::query) fn provide_codegen_preparation(
    db: &QueryDb<CompilerContext>,
) -> QueryResult<CodegenPreparation> {
    time_provider(db.context().timings(), "codegen_preparation", || {
        let graph = time_provider(
            db.context().timings(),
            "codegen_preparation.module_graph",
            || db.get(ModuleGraphQuery),
        )?
        .as_ref()
        .clone();
        let optimization = time_provider(
            db.context().timings(),
            "codegen_preparation.optimization",
            || db.get(CompilerOptimizationQuery),
        )?;
        let optimization = *optimization;
        let mut diagnostics = time_provider(
            db.context().timings(),
            "codegen_preparation.early_diagnostics",
            || early_program_diagnostics(db),
        )?;
        let modules = time_provider(
            db.context().timings(),
            "codegen_preparation.checked_modules",
            || checked_modules_for_codegen(db),
        )?;
        let (module_diagnostics, suppressed_downstream) = time_provider(
            db.context().timings(),
            "codegen_preparation.checked_diagnostics",
            || checked_module_diagnostics(db, &modules),
        )?;
        diagnostics.extend(module_diagnostics);
        diagnostics.extend(time_provider(
            db.context().timings(),
            "codegen_preparation.closure_safety",
            || closure_safety_diagnostics(db, &modules),
        )?);
        if crate::has_error_diagnostics(&diagnostics) {
            diagnostics.extend(deferred_const_instance_diagnostics(
                db,
                &modules,
                &diagnostics,
            )?);
            return Ok(CodegenPreparation {
                diagnostic_sources: diagnostic_sources(db, &graph, &diagnostics)?,
                type_store: Arc::clone(&db.context().type_store),
                graph,
                optimization,
                modules,
                monomorphization: Arc::new(empty_monomorphization()),
                diagnostics,
                suppressed_downstream,
            });
        }
        let monomorphization = db.get(MonomorphizationQuery)?;
        diagnostics.extend(time_provider(
            db.context().timings(),
            "codegen_program.monomorphization_diagnostics",
            || {
                monomorphization_diagnostics(
                    &modules,
                    resolve_diagnostic_bundle(&monomorphization.diagnostics),
                )
            },
        ));
        Ok(CodegenPreparation {
            diagnostic_sources: diagnostic_sources(db, &graph, &diagnostics)?,
            type_store: Arc::clone(&db.context().type_store),
            graph,
            optimization,
            modules,
            monomorphization: Arc::clone(&monomorphization.semantic),
            diagnostics,
            suppressed_downstream,
        })
    })
}

pub(super) fn provide_codegen_program(
    db: &QueryDb<CompilerContext>,
) -> QueryResult<CodegenProgram> {
    time_provider(db.context().timings(), "codegen_program", || {
        let preparation = db.get(CodegenPreparationQuery)?;
        let (backend_lowering, backend_diagnostics) =
            if crate::has_error_diagnostics(&preparation.diagnostics) {
                (
                    Arc::new(empty_backend_lowering(preparation.optimization)?),
                    Vec::new(),
                )
            } else {
                let backend_lowering = db.get(BackendLoweringQuery)?;
                let diagnostics = time_provider(
                    db.context().timings(),
                    "codegen_program.backend_diagnostics",
                    || {
                        backend_lowering_diagnostics(
                            &preparation.modules,
                            resolve_diagnostic_bundle(&backend_lowering.diagnostics),
                        )
                    },
                );
                (Arc::clone(&backend_lowering.semantic), diagnostics)
            };
        let mut diagnostics = preparation.diagnostics.clone();
        diagnostics.extend(backend_diagnostics);
        Ok(CodegenProgram {
            diagnostic_sources: diagnostic_sources(db, &preparation.graph, &diagnostics)?,
            type_store: Arc::clone(&preparation.type_store),
            graph: preparation.graph.clone(),
            optimization: preparation.optimization,
            modules: preparation.modules.clone(),
            monomorphization: Arc::clone(&preparation.monomorphization),
            backend_lowering,
            diagnostics,
            suppressed_downstream: preparation.suppressed_downstream,
        })
    })
}
