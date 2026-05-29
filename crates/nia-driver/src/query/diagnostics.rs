// SPDX-License-Identifier: GPL-3.0-or-later
use super::*;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub(super) struct ProgramDiagnosticsQuery;

impl QueryKey<DriverContext> for ProgramDiagnosticsQuery {
    type Value = Vec<ProgramDiagnostic>;

    fn name() -> &'static str {
        "program_diagnostics"
    }

    fn execute(&self, db: &QueryDb<DriverContext>) -> Self::Value {
        let mut diagnostics = db.context().loaded.diagnostics.clone();
        for loaded_module in &db.context().loaded.modules {
            for error in &loaded_module.parse_errors {
                diagnostics.push(ProgramDiagnostic {
                    path: loaded_module.path.clone(),
                    diagnostic: Diagnostic::error(error.span, error.message.clone()),
                });
            }
        }
        let public = db.query(PublicSurfaceQuery);
        for (module_id, diagnostic) in public.diagnostics {
            diagnostics.push(ProgramDiagnostic {
                path: db.context().path_for_module(module_id),
                diagnostic,
            });
        }
        let first_path = db
            .query(ParseOkModuleIdsQuery)
            .first()
            .map(|module_id| db.context().path_for_module(*module_id))
            .unwrap_or_else(|| SourcePath::new("<unknown>"));
        diagnostics.extend(db.query(ExtensionMethodsQuery).diagnostics.into_iter().map(
            |diagnostic| ProgramDiagnostic {
                path: first_path.clone(),
                diagnostic,
            },
        ));

        let checked_modules = db.query(CheckedModulesQuery);
        for checked in &checked_modules {
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
            diagnostics.extend(module_diagnostics(
                &checked.path,
                &checked.item_signatures.diagnostics,
            ));
            diagnostics.extend(module_diagnostics(
                &checked.path,
                &checked.type_normalization.diagnostics,
            ));
            diagnostics.extend(module_diagnostics(
                &checked.path,
                &checked.comptime.diagnostics,
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
                &checked.body_check.diagnostics,
            ));
        }

        let monomorphization = db.query(MonomorphizationQuery);
        diagnostics.extend(
            monomorphization
                .diagnostics
                .iter()
                .cloned()
                .map(|diagnostic| ProgramDiagnostic {
                    path: path_for_diagnostic_span(&checked_modules, diagnostic.span),
                    diagnostic,
                }),
        );
        let backend_lowering = db.query(BackendLoweringQuery);
        diagnostics.extend(
            backend_lowering
                .diagnostics
                .iter()
                .cloned()
                .map(|diagnostic| ProgramDiagnostic {
                    path: path_for_diagnostic_span(&checked_modules, diagnostic.span),
                    diagnostic,
                }),
        );
        diagnostics
    }
}

pub(super) fn modules_in_order(db: &QueryDb<DriverContext>) -> Vec<LoadedModule> {
    db.query(ParseOkModuleIdsQuery)
        .into_iter()
        .map(|module_id| db.query(LoadedModuleQuery(module_id)))
        .collect()
}

pub(super) fn path_for_diagnostic_span(modules: &[CheckedModule], span: Span) -> SourcePath {
    modules
        .iter()
        .find(|module| {
            module
                .body_check
                .ir
                .generic_instantiations
                .iter()
                .any(|instantiation| instantiation.span == span)
        })
        .map(|module| module.path.clone())
        .unwrap_or_else(|| {
            modules
                .first()
                .map(|module| module.path.clone())
                .unwrap_or_else(|| SourcePath::new("<unknown>"))
        })
}
