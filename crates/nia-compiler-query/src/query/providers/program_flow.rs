// SPDX-License-Identifier: GPL-3.0-or-later
use super::*;

pub(super) fn provide_checked_program(db: &QueryDb<CompilerContext>) -> CheckedProgram {
    time_provider(db.context().timings(), "checked_program", || {
        let graph = db.query(ModuleGraphQuery);
        let optimization = db.query(CompilerOptimizationQuery);
        let mut diagnostics = early_program_diagnostics(db);
        let diagnostic_modules = materialize_checked_modules(db, db.query(CheckedModuleIdsQuery));
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
        let graph = db.query(ModuleGraphQuery);
        let optimization = db.query(CompilerOptimizationQuery);
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
            || db.query(ModuleGraphQuery),
        );
        let optimization = time_provider(
            db.context().timings(),
            "codegen_program.optimization",
            || db.query(CompilerOptimizationQuery),
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
                graph,
                optimization,
                modules,
                monomorphization: empty_monomorphization(),
                backend_lowering: empty_backend_lowering(optimization),
                diagnostics,
            };
        }
        let monomorphization = time_provider(db.context().timings(), "monomorphization", || {
            monomorphization_for_checked_modules(db, &modules)
        });
        diagnostics.extend(time_provider(
            db.context().timings(),
            "codegen_program.monomorphization_diagnostics",
            || monomorphization_diagnostics(&modules, &monomorphization),
        ));
        if crate::has_error_diagnostics(&diagnostics) {
            return CodegenProgram {
                graph,
                optimization,
                modules,
                monomorphization,
                backend_lowering: empty_backend_lowering(optimization),
                diagnostics,
            };
        }
        let backend_lowering = time_provider(db.context().timings(), "backend_lowering", || {
            provide_backend_lowering_inner_for_modules(db, &monomorphization, &modules)
        });
        diagnostics.extend(time_provider(
            db.context().timings(),
            "codegen_program.backend_diagnostics",
            || backend_lowering_diagnostics(&modules, &backend_lowering),
        ));
        CodegenProgram {
            graph,
            optimization,
            modules,
            monomorphization,
            backend_lowering,
            diagnostics,
        }
    })
}
