// SPDX-License-Identifier: GPL-3.0-or-later
use super::*;

pub(super) fn defs_by_module_id(db: &QueryDb<CompilerContext>) -> ProgramDefsById {
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
