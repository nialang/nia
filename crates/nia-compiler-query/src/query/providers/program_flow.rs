// SPDX-License-Identifier: GPL-3.0-or-later
use super::*;

pub(super) fn provide_checked_program(db: &QueryDb<CompilerContext>) -> CheckedProgram {
    time_provider(db.context().timings(), "checked_program", || {
        let graph = db.get(ModuleGraphQuery).as_ref().clone();
        let optimization = *db.get(CompilerOptimizationQuery);
        let mut diagnostics = early_program_diagnostics(db);
        let diagnostic_modules =
            materialize_checked_modules(db, db.get(CheckedModuleIdsQuery).as_ref().clone());
        diagnostics.extend(checked_module_diagnostics(db, &diagnostic_modules));
        CheckedProgram {
            graph,
            optimization,
            modules: diagnostic_modules,
            diagnostics,
        }
    })
}

pub(super) fn provide_entry_checked_program(db: &QueryDb<CompilerContext>) -> CheckedProgram {
    time_provider(db.context().timings(), "entry_checked_program", || {
        let graph = db.get(ModuleGraphQuery).as_ref().clone();
        let optimization = *db.get(CompilerOptimizationQuery);
        let mut diagnostics = early_program_diagnostics(db);
        let diagnostic_modules = checked_modules_for_diagnostics(db);
        diagnostics.extend(checked_module_diagnostics(db, &diagnostic_modules));
        CheckedProgram {
            graph,
            optimization,
            modules: diagnostic_modules,
            diagnostics,
        }
    })
}

pub(super) fn provide_codegen_program(db: &QueryDb<CompilerContext>) -> CodegenProgram {
    time_provider(db.context().timings(), "codegen_program", || {
        let graph = time_provider(
            db.context().timings(),
            "codegen_program.module_graph",
            || db.get(ModuleGraphQuery).as_ref().clone(),
        );
        let optimization = time_provider(
            db.context().timings(),
            "codegen_program.optimization",
            || *db.get(CompilerOptimizationQuery),
        );
        let mut diagnostics = time_provider(
            db.context().timings(),
            "codegen_program.early_diagnostics",
            || early_program_diagnostics(db),
        );
        let modules = time_provider(
            db.context().timings(),
            "codegen_program.checked_modules",
            || checked_modules_for_codegen(db),
        );
        diagnostics.extend(time_provider(
            db.context().timings(),
            "codegen_program.checked_diagnostics",
            || checked_module_diagnostics(db, &modules),
        ));
        if crate::has_error_diagnostics(&diagnostics) {
            return CodegenProgram {
                type_store: Arc::clone(&db.context().type_store),
                graph,
                optimization,
                modules,
                monomorphization: Arc::new(empty_monomorphization()),
                backend_lowering: Arc::new(empty_backend_lowering(optimization)),
                diagnostics,
            };
        }
        let monomorphization = db.get(MonomorphizationQuery);
        diagnostics.extend(time_provider(
            db.context().timings(),
            "codegen_program.monomorphization_diagnostics",
            || monomorphization_diagnostics(&modules, &monomorphization),
        ));
        if crate::has_error_diagnostics(&diagnostics) {
            return CodegenProgram {
                type_store: Arc::clone(&db.context().type_store),
                graph,
                optimization,
                modules,
                monomorphization,
                backend_lowering: Arc::new(empty_backend_lowering(optimization)),
                diagnostics,
            };
        }
        let backend_lowering = db.get(BackendLoweringQuery);
        diagnostics.extend(time_provider(
            db.context().timings(),
            "codegen_program.backend_diagnostics",
            || backend_lowering_diagnostics(&modules, &backend_lowering),
        ));
        CodegenProgram {
            type_store: Arc::clone(&db.context().type_store),
            graph,
            optimization,
            modules,
            monomorphization,
            backend_lowering,
            diagnostics,
        }
    })
}
