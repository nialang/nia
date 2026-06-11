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
        (db.context().providers.program_diagnostics)(db)
    }
}

pub(super) fn defs_by_module_id(db: &QueryDb<DriverContext>) -> ProgramDefsById {
    db.query(ProgramDefsByIdQuery)
}

pub(super) fn path_for_diagnostic_span(modules: &[CheckedModule], span: Span) -> SourcePath {
    modules
        .iter()
        .find(|module| {
            module
                .semantic_facts
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
