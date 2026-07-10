// SPDX-License-Identifier: GPL-3.0-or-later
use super::*;

pub(super) fn synthetic_diagnostic_path() -> SourcePath {
    SourcePath::new("<nia:diagnostic>")
}

pub(super) fn path_for_diagnostic_span(modules: &[CheckedModule], span: Span) -> SourcePath {
    modules
        .iter()
        .find(|module| {
            module
                .semantic_facts
                .iter_generic_instantiations()
                .any(|instantiation| instantiation.span == span)
        })
        .map(|module| module.path.clone())
        .unwrap_or_else(|| {
            modules
                .first()
                .map(|module| module.path.clone())
                .unwrap_or_else(synthetic_diagnostic_path)
        })
}
