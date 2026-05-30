// SPDX-License-Identifier: GPL-3.0-or-later
use super::*;
use std::collections::HashMap;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub(super) struct ProgramDiagnosticsQuery;

impl QueryKey<DriverContext> for ProgramDiagnosticsQuery {
    type Value = Vec<ProgramDiagnostic>;

    fn name() -> &'static str {
        "program_diagnostics"
    }

    fn execute(&self, db: &QueryDb<DriverContext>) -> Self::Value {
        (db.context().providers.program_diagnostics)(db)
    }
}

pub(super) fn modules_in_order(db: &QueryDb<DriverContext>) -> Vec<LoadedModule> {
    db.query(ParseOkModuleIdsQuery)
        .into_iter()
        .map(|module_id| db.query(LoadedModuleQuery(module_id)))
        .collect()
}

pub(super) fn defs_by_module_id(db: &QueryDb<DriverContext>) -> HashMap<ModuleId, DefCollection> {
    db.query(ParseOkModuleIdsQuery)
        .into_iter()
        .map(|module_id| (module_id, db.query(ModuleDefsQuery(module_id))))
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
