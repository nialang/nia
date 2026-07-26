// SPDX-License-Identifier: GPL-3.0-or-later
use super::*;

#[derive(Debug, Clone, PartialEq)]
pub(super) struct ModuleDiagnosticBundle {
    pub(super) module_id: ModuleId,
    pub(super) diagnostics: nia_diagnostic::DiagnosticBundle,
}

pub(super) fn store_module_diagnostics(
    store: &nia_diagnostic::DiagnosticStore,
    diagnostics: Vec<(ModuleId, Diagnostic)>,
) -> Vec<ModuleDiagnosticBundle> {
    let mut diagnostics = diagnostics.into_iter().peekable();
    let mut bundles = Vec::new();
    while let Some((module_id, diagnostic)) = diagnostics.next() {
        let mut module_diagnostics = vec![diagnostic];
        while diagnostics
            .peek()
            .is_some_and(|(next_module_id, _)| *next_module_id == module_id)
        {
            let (_, diagnostic) = diagnostics
                .next()
                .expect("peeked module diagnostic must remain available");
            module_diagnostics.push(diagnostic);
        }
        bundles.push(ModuleDiagnosticBundle {
            module_id,
            diagnostics: store.bundle(module_diagnostics),
        });
    }
    bundles
}

pub(super) fn resolve_diagnostic_bundle<'bundle>(
    context: &CompilerContext,
    bundle: &'bundle nia_diagnostic::DiagnosticBundle,
) -> &'bundle [Diagnostic] {
    context
        .diagnostic_store
        .diagnostics(bundle)
        .unwrap_or_else(|| panic!("Nia ICE: diagnostic bundle has a foreign store owner"))
}

pub(super) fn synthetic_diagnostic_path() -> SourcePath {
    SourcePath::new("<nia:diagnostic>")
}

pub(super) fn path_for_diagnostic_span(modules: &[Arc<CheckedModule>], span: Span) -> SourcePath {
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

#[cfg(test)]
mod tests {
    use nia_diagnostic::{Diagnostic, DiagnosticStore, codes};
    use nia_ids::ModuleIdAllocator;
    use nia_span::Span;

    use super::store_module_diagnostics;

    #[test]
    fn module_diagnostic_bundles_preserve_order_and_only_group_adjacent_owners() {
        let mut modules = ModuleIdAllocator::new();
        let first = modules.allocate();
        let second = modules.allocate();
        let store = DiagnosticStore::new();
        let diagnostic =
            |summary| Diagnostic::user_error_at(codes::NAME_RESOLUTION, Span::new(0, 1), summary);
        let bundles = store_module_diagnostics(
            &store,
            vec![
                (first, diagnostic("first")),
                (first, diagnostic("second")),
                (second, diagnostic("third")),
                (first, diagnostic("fourth")),
            ],
        );

        assert_eq!(
            bundles
                .iter()
                .map(|bundle| {
                    (
                        bundle.module_id,
                        store
                            .diagnostics(&bundle.diagnostics)
                            .unwrap()
                            .iter()
                            .map(|diagnostic| diagnostic.summary.as_str())
                            .collect::<Vec<_>>(),
                    )
                })
                .collect::<Vec<_>>(),
            vec![
                (first, vec!["first", "second"]),
                (second, vec!["third"]),
                (first, vec!["fourth"]),
            ]
        );
    }
}
