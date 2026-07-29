// SPDX-License-Identifier: GPL-3.0-or-later
use super::*;

#[derive(Debug, Clone, PartialEq)]
pub(super) struct ProgramMonomorphization {
    pub(super) semantic: Arc<nia_monomorphize::Monomorphization>,
    pub(super) diagnostics: nia_diagnostic::DiagnosticBundle,
}

#[derive(Debug, Clone, PartialEq)]
pub(super) struct ProgramBackendLowering {
    pub(super) semantic: Arc<nia_backend_lower::BackendLowering>,
    pub(super) diagnostics: nia_diagnostic::DiagnosticBundle,
}

pub(super) struct ProgramBackendLoweringInputs {
    pub(super) semantic: Option<BackendLoweringInputs>,
    pub(super) diagnostics: nia_diagnostic::DiagnosticBundle,
}

#[derive(Debug, Clone, PartialEq)]
pub(super) struct ModuleBodyCheck {
    pub(super) semantic: Arc<nia_body_check::BodyCheck>,
    pub(super) diagnostics: nia_diagnostic::DiagnosticBundle,
}

#[derive(Debug, Clone, PartialEq)]
pub(super) struct FullModuleDefinitions {
    pub(super) semantic: Arc<DefCollection>,
    pub(super) diagnostics: nia_diagnostic::DiagnosticBundle,
}

#[derive(Debug, Clone, PartialEq)]
pub(super) struct ModuleDefinitions {
    pub(super) semantic: Arc<DefCollection>,
    pub(super) diagnostics: nia_diagnostic::DiagnosticBundle,
}

#[derive(Debug, Clone, PartialEq)]
pub(super) struct SignatureTypeResolution {
    pub(super) semantic: Arc<TypeResolution>,
    pub(super) diagnostics: nia_diagnostic::DiagnosticBundle,
}

#[derive(Debug, Clone, PartialEq)]
pub(super) struct SignatureTypeLowering {
    pub(super) semantic: Arc<TypeLowering>,
    pub(super) diagnostics: nia_diagnostic::DiagnosticBundle,
}

#[derive(Debug, Clone, PartialEq)]
pub(super) struct SignatureItemSignatures {
    pub(super) semantic: Arc<ItemSignatures>,
    pub(super) diagnostics: nia_diagnostic::DiagnosticBundle,
}

#[derive(Debug, Clone, PartialEq)]
pub(super) struct SignatureTypeNormalization {
    pub(super) semantic: Arc<TypeNormalization>,
    pub(super) diagnostics: nia_diagnostic::DiagnosticBundle,
}

#[derive(Debug, Clone, PartialEq)]
pub(super) struct ModuleTypeNormalization {
    pub(super) semantic: Arc<TypeNormalization>,
    pub(super) diagnostics: nia_diagnostic::DiagnosticBundle,
}

#[derive(Debug, Clone, PartialEq)]
pub(super) struct ModuleTypeResolution {
    pub(super) semantic: Arc<TypeResolution>,
    pub(super) diagnostics: nia_diagnostic::DiagnosticBundle,
}

#[derive(Debug, Clone, PartialEq)]
pub(super) struct ModuleTypeLowering {
    pub(super) semantic: Arc<TypeLowering>,
    pub(super) diagnostics: nia_diagnostic::DiagnosticBundle,
}

#[derive(Debug, Clone, PartialEq)]
pub(super) struct ModuleItemSignatures {
    pub(super) semantic: Arc<ItemSignatures>,
    pub(super) diagnostics: nia_diagnostic::DiagnosticBundle,
}

#[derive(Debug, Clone, PartialEq)]
pub(super) struct ModuleValueResolution {
    pub(super) semantic: Arc<ValueResolution>,
    pub(super) diagnostics: nia_diagnostic::DiagnosticBundle,
}

#[derive(Debug, Clone, PartialEq)]
pub(super) struct ModuleLocalResolution {
    pub(super) semantic: Arc<LocalResolution>,
    pub(super) diagnostics: nia_diagnostic::DiagnosticBundle,
}

#[derive(Debug, Clone, PartialEq)]
pub(super) struct ModuleLayouts {
    pub(super) semantic: Arc<nia_layout::Layouts>,
    pub(super) diagnostics: nia_diagnostic::DiagnosticBundle,
}

pub(super) fn store_module_layouts(
    context: &CompilerContext,
    mut layouts: nia_layout::Layouts,
) -> ModuleLayouts {
    let diagnostics = std::mem::take(&mut layouts.diagnostics);
    ModuleLayouts {
        semantic: Arc::new(layouts),
        diagnostics: context.diagnostic_store.bundle(diagnostics),
    }
}

#[derive(Debug, Clone, PartialEq)]
pub(super) struct ModuleConstCheck {
    pub(super) semantic: Arc<ConstCheck>,
    pub(super) diagnostics: nia_diagnostic::DiagnosticBundle,
}

#[derive(Debug, Clone, PartialEq)]
pub(super) struct ModuleStaticCheck {
    pub(super) semantic: Arc<nia_static_check::StaticCheck>,
    pub(super) diagnostics: nia_diagnostic::DiagnosticBundle,
}

#[derive(Debug, Clone, PartialEq)]
pub(super) struct ModuleAbiCheck {
    pub(super) semantic: Arc<nia_abi_check::AbiCheck>,
    pub(super) diagnostics: nia_diagnostic::DiagnosticBundle,
}

#[derive(Debug, Clone, PartialEq)]
pub(super) struct ModuleFlowCheck {
    pub(super) semantic: Arc<nia_flow_check::FlowCheck>,
    pub(super) diagnostics: nia_diagnostic::DiagnosticBundle,
}

#[derive(Debug, Clone, PartialEq)]
pub(super) struct ProgramLoadDiagnostics {
    pub(super) bundles: Vec<SourceDiagnosticBundle>,
}

#[derive(Debug, Clone, PartialEq)]
pub(super) struct SourceDiagnosticBundle {
    pub(super) path: SourcePath,
    pub(super) diagnostics: nia_diagnostic::DiagnosticBundle,
}

pub(super) fn store_program_diagnostics(
    store: &nia_diagnostic::DiagnosticStore,
    diagnostics: Vec<ProgramDiagnostic>,
) -> ProgramLoadDiagnostics {
    let mut diagnostics = diagnostics.into_iter().peekable();
    let mut bundles = Vec::new();
    while let Some(ProgramDiagnostic { path, diagnostic }) = diagnostics.next() {
        let mut source_diagnostics = vec![diagnostic];
        while let Some(next) = diagnostics.next_if(|next| next.path == path) {
            source_diagnostics.push(next.diagnostic);
        }
        bundles.push(SourceDiagnosticBundle {
            path,
            diagnostics: store.bundle(source_diagnostics),
        });
    }
    ProgramLoadDiagnostics { bundles }
}

pub(super) fn resolve_program_diagnostics(
    context: &CompilerContext,
    diagnostics: &ProgramLoadDiagnostics,
) -> Vec<ProgramDiagnostic> {
    diagnostics
        .bundles
        .iter()
        .flat_map(|bundle| {
            resolve_diagnostic_bundle(context, &bundle.diagnostics)
                .iter()
                .cloned()
                .map(|diagnostic| ProgramDiagnostic {
                    path: bundle.path.clone(),
                    diagnostic,
                })
        })
        .collect()
}

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
        while let Some((_, diagnostic)) =
            diagnostics.next_if(|(next_module_id, _)| *next_module_id == module_id)
        {
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

pub(super) fn full_module_defs_semantic(
    db: &QueryDb<CompilerContext>,
    module_id: ModuleId,
) -> QueryResult<Arc<DefCollection>> {
    Ok(Arc::clone(
        &db.get(FullModuleDefsQuery(module_id))?.semantic,
    ))
}

pub(super) fn module_defs_semantic(
    db: &QueryDb<CompilerContext>,
    module_id: ModuleId,
) -> QueryResult<Arc<DefCollection>> {
    Ok(Arc::clone(&db.get(ModuleDefsQuery(module_id))?.semantic))
}

pub(super) fn signature_item_signatures_semantic(
    db: &QueryDb<CompilerContext>,
    module_id: ModuleId,
    set: nia_item_tree::SignatureItemSet,
) -> QueryResult<Arc<ItemSignatures>> {
    Ok(Arc::clone(
        &db.get(SignatureItemSignaturesQuery(module_id, set))?
            .semantic,
    ))
}

pub(super) fn signature_type_normalization_semantic(
    db: &QueryDb<CompilerContext>,
    module_id: ModuleId,
    set: nia_item_tree::SignatureItemSet,
) -> QueryResult<Arc<TypeNormalization>> {
    Ok(Arc::clone(
        &db.get(SignatureTypeNormalizationQuery(module_id, set))?
            .semantic,
    ))
}

pub(super) fn type_normalization_semantic(
    db: &QueryDb<CompilerContext>,
    module_id: ModuleId,
) -> QueryResult<Arc<TypeNormalization>> {
    Ok(Arc::clone(
        &db.get(TypeNormalizationQuery(module_id))?.semantic,
    ))
}

pub(super) fn type_resolution_semantic(
    db: &QueryDb<CompilerContext>,
    module_id: ModuleId,
) -> QueryResult<Arc<TypeResolution>> {
    Ok(Arc::clone(
        &db.get(TypeResolutionQuery(module_id))?.semantic,
    ))
}

pub(super) fn type_lowering_semantic(
    db: &QueryDb<CompilerContext>,
    module_id: ModuleId,
) -> QueryResult<Arc<TypeLowering>> {
    Ok(Arc::clone(&db.get(TypeLoweringQuery(module_id))?.semantic))
}

pub(super) fn item_signatures_semantic(
    db: &QueryDb<CompilerContext>,
    module_id: ModuleId,
) -> QueryResult<Arc<ItemSignatures>> {
    Ok(Arc::clone(
        &db.get(ItemSignaturesQuery(module_id))?.semantic,
    ))
}

pub(super) fn value_resolution_semantic(
    db: &QueryDb<CompilerContext>,
    module_id: ModuleId,
) -> QueryResult<Arc<ValueResolution>> {
    Ok(Arc::clone(
        &db.get(ValueResolutionQuery(module_id))?.semantic,
    ))
}

pub(super) fn local_resolution_semantic(
    db: &QueryDb<CompilerContext>,
    module_id: ModuleId,
) -> QueryResult<Arc<LocalResolution>> {
    Ok(Arc::clone(
        &db.get(LocalResolutionQuery(module_id))?.semantic,
    ))
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
    use nia_source::SourcePath;
    use nia_span::Span;

    use crate::ProgramDiagnostic;

    use super::{store_module_diagnostics, store_program_diagnostics};

    #[test]
    fn program_diagnostic_bundles_preserve_order_and_only_group_adjacent_sources() {
        let store = DiagnosticStore::new();
        let first = SourcePath::new("src/first.nia");
        let second = SourcePath::new("src/second.nia");
        let diagnostic =
            |summary| Diagnostic::user_error_at(codes::NAME_RESOLUTION, Span::new(0, 1), summary);
        let bundles = store_program_diagnostics(
            &store,
            vec![
                ProgramDiagnostic {
                    path: first.clone(),
                    diagnostic: diagnostic("first"),
                },
                ProgramDiagnostic {
                    path: first.clone(),
                    diagnostic: diagnostic("second"),
                },
                ProgramDiagnostic {
                    path: second.clone(),
                    diagnostic: diagnostic("third"),
                },
                ProgramDiagnostic {
                    path: first.clone(),
                    diagnostic: diagnostic("fourth"),
                },
            ],
        );

        assert_eq!(
            bundles
                .bundles
                .iter()
                .map(|bundle| {
                    (
                        bundle.path.clone(),
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
                (first.clone(), vec!["first", "second"]),
                (second, vec!["third"]),
                (first, vec!["fourth"]),
            ]
        );
    }

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
