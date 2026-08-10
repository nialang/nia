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
        diagnostics.extend(checked_module_diagnostics(db, &diagnostic_modules)?);
        diagnostics.extend(time_provider(
            db.context().timings(),
            "checked_program.closure_safety",
            || closure_safety_diagnostics(db, &diagnostic_modules),
        ));
        Ok(CheckedProgramAnalysis {
            graph,
            optimization,
            modules: diagnostic_modules,
            diagnostics,
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
        diagnostics.extend(checked_module_diagnostics(db, &diagnostic_modules)?);
        diagnostics.extend(time_provider(
            db.context().timings(),
            "entry_checked_program.closure_safety",
            || closure_safety_diagnostics(db, &diagnostic_modules),
        ));
        Ok(CheckedProgramAnalysis {
            graph,
            optimization,
            modules: diagnostic_modules,
            diagnostics,
        })
    })
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
        diagnostics.extend(time_provider(
            db.context().timings(),
            "codegen_preparation.checked_diagnostics",
            || checked_module_diagnostics(db, &modules),
        )?);
        diagnostics.extend(time_provider(
            db.context().timings(),
            "codegen_preparation.closure_safety",
            || closure_safety_diagnostics(db, &modules),
        ));
        if crate::has_error_diagnostics(&diagnostics) {
            return Ok(CodegenPreparation {
                type_store: Arc::clone(&db.context().type_store),
                graph,
                optimization,
                modules,
                monomorphization: Arc::new(empty_monomorphization()),
                diagnostics,
            });
        }
        let monomorphization = db.get(MonomorphizationQuery)?;
        diagnostics.extend(time_provider(
            db.context().timings(),
            "codegen_program.monomorphization_diagnostics",
            || {
                monomorphization_diagnostics(
                    &modules,
                    resolve_diagnostic_bundle(db.context(), &monomorphization.diagnostics),
                )
            },
        ));
        Ok(CodegenPreparation {
            type_store: Arc::clone(&db.context().type_store),
            graph,
            optimization,
            modules,
            monomorphization: Arc::clone(&monomorphization.semantic),
            diagnostics,
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
                    Arc::new(empty_backend_lowering(preparation.optimization)),
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
                            resolve_diagnostic_bundle(db.context(), &backend_lowering.diagnostics),
                        )
                    },
                );
                (Arc::clone(&backend_lowering.semantic), diagnostics)
            };
        let mut diagnostics = preparation.diagnostics.clone();
        diagnostics.extend(backend_diagnostics);
        Ok(CodegenProgram {
            type_store: Arc::clone(&preparation.type_store),
            graph: preparation.graph.clone(),
            optimization: preparation.optimization,
            modules: preparation.modules.clone(),
            monomorphization: Arc::clone(&preparation.monomorphization),
            backend_lowering,
            diagnostics,
        })
    })
}
