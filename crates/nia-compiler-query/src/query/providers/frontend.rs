// SPDX-License-Identifier: GPL-3.0-or-later
use super::*;

pub(super) fn provide_module_graph(db: &QueryDb<CompilerContext>) -> ModuleGraph {
    db.context().module_graph()
}

pub(super) fn provide_parse_ok_module_ids(db: &QueryDb<CompilerContext>) -> Vec<ModuleId> {
    db.query(LoadedModulesQuery)
        .into_iter()
        .filter(|module_id| {
            let parse_errors = db.query(ModuleParseErrorsQuery(*module_id));
            parse_errors.is_empty()
        })
        .collect()
}

pub(super) fn provide_semantic_module_ids(db: &QueryDb<CompilerContext>) -> Vec<ModuleId> {
    let graph = db.get(ModuleGraphQuery);
    let entry = graph.entry();
    db.query(ParseOkModuleIdsQuery)
        .into_iter()
        .filter(|module_id| {
            graph
                .get(*module_id)
                .is_some_and(|node| *module_id == entry || node.process_used_paths)
        })
        .collect()
}

pub(super) fn provide_module_item_tree(
    db: &QueryDb<CompilerContext>,
    module_id: ModuleId,
) -> ModuleItemTree {
    db.query(ModuleItemTreeInputQuery(module_id))
        .as_ref()
        .clone()
}

pub(super) fn provide_active_module_item_tree(
    db: &QueryDb<CompilerContext>,
    module_id: ModuleId,
) -> ActiveModuleItemTree {
    let _raw_item_tree = db.get(ModuleItemTreeQuery(module_id));
    db.query(ActiveModuleItemTreeInputQuery(module_id))
        .as_ref()
        .clone()
}

pub(super) fn provide_full_module_item_tree(
    db: &QueryDb<CompilerContext>,
    module_id: ModuleId,
) -> Arc<ModuleItemTree> {
    db.query(FullModuleItemTreeInputQuery(module_id))
}

pub(super) fn provide_full_active_module_item_tree(
    db: &QueryDb<CompilerContext>,
    module_id: ModuleId,
) -> Arc<ActiveModuleItemTree> {
    let _raw_item_tree = db.query(FullModuleItemTreeQuery(module_id));
    db.query(FullActiveModuleItemTreeInputQuery(module_id))
}

pub(super) fn provide_module_defs(
    db: &QueryDb<CompilerContext>,
    module_id: ModuleId,
) -> DefCollection {
    let item_tree = db.get(ActiveModuleItemTreeQuery(module_id));
    let symbols = db.context().symbols();
    nia_defs::collect_module_defs_from_active_item_tree_with_symbols(
        module_id, &item_tree, &symbols,
    )
}

pub(super) fn provide_full_module_defs(
    db: &QueryDb<CompilerContext>,
    module_id: ModuleId,
) -> DefCollection {
    let item_tree = db.query(FullActiveModuleItemTreeQuery(module_id));
    let symbols = db.context().symbols();
    nia_defs::collect_module_defs_from_active_item_tree_with_symbols(
        module_id, &item_tree, &symbols,
    )
}

fn shared_defs_by_module(db: &QueryDb<CompilerContext>) -> Vec<Arc<DefCollection>> {
    db.query(ParseOkModuleIdsQuery)
        .into_iter()
        .map(|module_id| db.get(ModuleDefsQuery(module_id)))
        .collect()
}

pub(super) fn provide_public_surfaces(db: &QueryDb<CompilerContext>) -> PublicSurfacesValue {
    time_provider(db.context().timings(), "public_surfaces", || {
        db.context().public_surfaces()
    })
}

pub(super) fn provide_module_public_surface(
    db: &QueryDb<CompilerContext>,
    module_id: ModuleId,
) -> Option<Arc<ModulePublicSurface>> {
    db.context()
        .public_surfaces()
        .surfaces
        .public_surface(module_id)
}

pub(super) fn provide_public_using_scopes(db: &QueryDb<CompilerContext>) -> PublicUsingScopesValue {
    time_provider(db.context().timings(), "public_using_scopes", || {
        db.context().public_using_scopes()
    })
}

pub(super) fn provide_module_using_scope(
    db: &QueryDb<CompilerContext>,
    module_id: ModuleId,
) -> ModuleUsingScope {
    db.context()
        .public_using_scopes()
        .using_scopes
        .get(&module_id)
        .cloned()
        .unwrap_or_default()
}

pub(super) fn provide_type_exposure_index(db: &QueryDb<CompilerContext>) -> TypeExposureIndexValue {
    time_provider(db.context().timings(), "type_exposure_index", || {
        let defs = shared_defs_by_module(db);
        let public_surfaces = db.query(PublicSurfacesQuery);
        let public_using_scopes = db.query(PublicUsingScopesQuery);
        Arc::new(TypeExposureIndex::from_defs_surfaces_and_using_scopes(
            &defs,
            &public_surfaces.surfaces,
            &public_using_scopes.using_scopes,
        ))
    })
}

pub(super) fn provide_type_resolution(
    db: &QueryDb<CompilerContext>,
    module_id: ModuleId,
) -> TypeResolution {
    time_module_provider(db, "type_resolution", module_id, || {
        let active_item_tree = db.query(FullActiveModuleItemTreeQuery(module_id));
        let defs = db.get(FullModuleDefsQuery(module_id));
        let program_defs = |module_id| Some(db.get(FullModuleDefsQuery(module_id)));
        let graph = QueryModuleGraphLookup::new(db);
        let public_surfaces = QueryPublicSurfaceLookup::new(db);
        let using_scope = QueryUsingScopeLookup::new(db, module_id);
        let symbols = db.context().symbols();
        nia_type_resolve::resolve_module_types_from_active_item_tree_with_symbols(
            &active_item_tree,
            &defs,
            nia_type_resolve::ProgramDefsContext {
                defs: Some(&program_defs),
                graph: Some(&graph),
            },
            &public_surfaces,
            &using_scope,
            &symbols,
        )
    })
}

pub(super) fn provide_declaration_type_resolution(
    db: &QueryDb<CompilerContext>,
    module_id: ModuleId,
) -> TypeResolution {
    time_module_provider(db, "declaration_type_resolution", module_id, || {
        let active_item_tree = db.get(DeclarationActiveModuleItemTreeQuery(module_id));
        let defs = db.get(ModuleDefsQuery(module_id));
        let program_defs = |module_id| Some(db.get(ModuleDefsQuery(module_id)));
        let graph = QueryModuleGraphLookup::new(db);
        let public_surfaces = QueryPublicSurfaceLookup::new(db);
        let using_scope = QueryUsingScopeLookup::new(db, module_id);
        let symbols = db.context().symbols();
        nia_type_resolve::resolve_module_types_from_active_item_tree_with_symbols(
            &active_item_tree,
            &defs,
            nia_type_resolve::ProgramDefsContext {
                defs: Some(&program_defs),
                graph: Some(&graph),
            },
            &public_surfaces,
            &using_scope,
            &symbols,
        )
    })
}

pub(super) fn provide_signature_type_resolution(
    db: &QueryDb<CompilerContext>,
    module_id: ModuleId,
    set: nia_item_tree::SignatureItemSet,
) -> TypeResolution {
    time_module_provider(db, "signature_type_resolution", module_id, || {
        let active_item_tree = db.get(SignatureItemTreeQuery(module_id, set));
        let defs = db.get(ModuleDefsQuery(module_id));
        let program_defs = |module_id| Some(db.get(ModuleDefsQuery(module_id)));
        let graph = QueryModuleGraphLookup::new(db);
        let public_surfaces = QueryPublicSurfaceLookup::new(db);
        let using_scope = QueryUsingScopeLookup::new(db, module_id);
        let symbols = db.context().symbols();
        nia_type_resolve::resolve_module_declaration_types_from_active_item_tree_with_symbols(
            &active_item_tree,
            &defs,
            nia_type_resolve::ProgramDefsContext {
                defs: Some(&program_defs),
                graph: Some(&graph),
            },
            &public_surfaces,
            &using_scope,
            &symbols,
        )
    })
}

pub(super) fn provide_signature_const_type_resolution(
    db: &QueryDb<CompilerContext>,
    module_id: ModuleId,
) -> TypeResolution {
    time_module_provider(db, "signature_const_type_resolution", module_id, || {
        let active_item_tree = db.query(SignatureConstItemTreeQuery(module_id));
        let defs = db.get(ModuleDefsQuery(module_id));
        let program_defs = |module_id| Some(db.get(ModuleDefsQuery(module_id)));
        let graph = QueryModuleGraphLookup::new(db);
        let public_surfaces = QueryPublicSurfaceLookup::new(db);
        let using_scope = QueryUsingScopeLookup::new(db, module_id);
        let symbols = db.context().symbols();
        nia_type_resolve::resolve_module_types_from_active_item_tree_with_symbols(
            &active_item_tree,
            &defs,
            nia_type_resolve::ProgramDefsContext {
                defs: Some(&program_defs),
                graph: Some(&graph),
            },
            &public_surfaces,
            &using_scope,
            &symbols,
        )
    })
}

pub(super) fn provide_type_lowering(
    db: &QueryDb<CompilerContext>,
    module_id: ModuleId,
) -> TypeLowering {
    let active_item_tree = db.query(FullActiveModuleItemTreeQuery(module_id));
    let type_resolution = db.query(TypeResolutionQuery(module_id));
    let program_defs = |module_id| Some(db.get(FullModuleDefsQuery(module_id)));
    let symbols = db.context().symbols();
    nia_type_lower::lower_module_types_from_active_item_tree_with_context(
        module_id,
        &active_item_tree,
        &type_resolution,
        nia_type_lower::TypeLoweringContext::from_program_defs(
            db.context().type_store(),
            nia_type_lower::ProgramDefsContext {
                defs: Some(&program_defs),
            },
        )
        .with_symbols(&symbols),
    )
}

pub(super) fn provide_declaration_type_lowering(
    db: &QueryDb<CompilerContext>,
    module_id: ModuleId,
) -> TypeLowering {
    let active_item_tree = db.get(DeclarationActiveModuleItemTreeQuery(module_id));
    let type_resolution = db.query(DeclarationTypeResolutionQuery(module_id));
    let program_defs = |module_id| Some(db.get(ModuleDefsQuery(module_id)));
    let symbols = db.context().symbols();
    nia_type_lower::lower_module_types_from_active_item_tree_with_context(
        module_id,
        &active_item_tree,
        &type_resolution,
        nia_type_lower::TypeLoweringContext::from_program_defs(
            db.context().type_store(),
            nia_type_lower::ProgramDefsContext {
                defs: Some(&program_defs),
            },
        )
        .with_symbols(&symbols),
    )
}

pub(super) fn provide_signature_type_lowering(
    db: &QueryDb<CompilerContext>,
    module_id: ModuleId,
    set: nia_item_tree::SignatureItemSet,
) -> TypeLowering {
    let active_item_tree = db.get(SignatureItemTreeQuery(module_id, set));
    let type_resolution = db.query(SignatureTypeResolutionQuery(module_id, set));
    let program_defs = |module_id| Some(db.get(ModuleDefsQuery(module_id)));
    let symbols = db.context().symbols();
    nia_type_lower::lower_module_declaration_types_from_active_item_tree_with_context(
        module_id,
        &active_item_tree,
        &type_resolution,
        nia_type_lower::TypeLoweringContext::from_program_defs(
            db.context().type_store(),
            nia_type_lower::ProgramDefsContext {
                defs: Some(&program_defs),
            },
        )
        .with_symbols(&symbols),
    )
}

pub(super) fn provide_signature_const_type_lowering(
    db: &QueryDb<CompilerContext>,
    module_id: ModuleId,
) -> TypeLowering {
    let active_item_tree = db.query(SignatureConstItemTreeQuery(module_id));
    let type_resolution = db.query(SignatureConstTypeResolutionQuery(module_id));
    let program_defs = |module_id| Some(db.get(ModuleDefsQuery(module_id)));
    let symbols = db.context().symbols();
    nia_type_lower::lower_module_types_from_active_item_tree_with_context(
        module_id,
        &active_item_tree,
        &type_resolution,
        nia_type_lower::TypeLoweringContext::from_program_defs(
            db.context().type_store(),
            nia_type_lower::ProgramDefsContext {
                defs: Some(&program_defs),
            },
        )
        .with_symbols(&symbols),
    )
}

pub(super) fn provide_item_signatures(
    db: &QueryDb<CompilerContext>,
    module_id: ModuleId,
) -> ItemSignatures {
    let active_item_tree = db.get(DeclarationActiveModuleItemTreeQuery(module_id));
    let defs = db.get(ModuleDefsQuery(module_id));
    let type_lowering = db.query(DeclarationTypeLoweringQuery(module_id));
    let symbols = db.context().symbols();
    nia_item_signatures::collect_item_signatures(nia_item_signatures::ItemSignatureInput {
        source: nia_item_signatures::ItemSignatureSource::ActiveItemTree(&active_item_tree),
        defs: &defs,
        lowered: &type_lowering,
        type_store: db.context().type_store(),
        symbols: Some(&symbols),
    })
}

pub(super) fn provide_signature_item_signatures(
    db: &QueryDb<CompilerContext>,
    module_id: ModuleId,
    set: nia_item_tree::SignatureItemSet,
) -> ItemSignatures {
    let active_item_tree = db.get(SignatureItemTreeQuery(module_id, set));
    let defs = db.get(ModuleDefsQuery(module_id));
    let type_lowering = db.get(SignatureTypeLoweringQuery(module_id, set));
    let symbols = db.context().symbols();
    nia_item_signatures::collect_item_signatures(nia_item_signatures::ItemSignatureInput {
        source: nia_item_signatures::ItemSignatureSource::ActiveItemTree(&active_item_tree),
        defs: &defs,
        lowered: &type_lowering,
        type_store: db.context().type_store(),
        symbols: Some(&symbols),
    })
}

pub(super) fn provide_signature_const_item_signatures(
    db: &QueryDb<CompilerContext>,
    module_id: ModuleId,
) -> ItemSignatures {
    let active_item_tree = db.query(SignatureConstItemTreeQuery(module_id));
    let defs = db.get(ModuleDefsQuery(module_id));
    let type_lowering = db.query(SignatureConstTypeLoweringQuery(module_id));
    let symbols = db.context().symbols();
    nia_item_signatures::collect_item_signatures(nia_item_signatures::ItemSignatureInput {
        source: nia_item_signatures::ItemSignatureSource::ActiveItemTree(&active_item_tree),
        defs: &defs,
        lowered: &type_lowering,
        type_store: db.context().type_store(),
        symbols: Some(&symbols),
    })
}

pub(super) fn provide_type_normalization(
    db: &QueryDb<CompilerContext>,
    module_id: ModuleId,
) -> TypeNormalization {
    let type_lowering = db.query(TypeLoweringQuery(module_id));
    let item_signatures = db.query(ItemSignaturesQuery(module_id));
    normalize_types_in_session_store(db, module_id, &type_lowering, &item_signatures)
}

pub(super) fn provide_layout_type_normalization(
    db: &QueryDb<CompilerContext>,
    module_id: ModuleId,
) -> TypeNormalization {
    let type_lowering = db.query(TypeLoweringQuery(module_id));
    let item_signatures = db.query(ItemSignaturesQuery(module_id));
    normalize_types_in_session_store(db, module_id, &type_lowering, &item_signatures)
}

pub(super) fn provide_signature_type_normalization(
    db: &QueryDb<CompilerContext>,
    module_id: ModuleId,
    set: nia_item_tree::SignatureItemSet,
) -> TypeNormalization {
    let type_lowering = db.get(SignatureTypeLoweringQuery(module_id, set));
    let item_signatures = db.get(SignatureItemSignaturesQuery(module_id, set));
    normalize_types_in_session_store(db, module_id, &type_lowering, &item_signatures)
}

pub(super) fn provide_signature_const_type_normalization(
    db: &QueryDb<CompilerContext>,
    module_id: ModuleId,
) -> TypeNormalization {
    let type_lowering = db.query(SignatureConstTypeLoweringQuery(module_id));
    let item_signatures = db.query(SignatureConstItemSignaturesQuery(module_id));
    normalize_types_in_session_store(db, module_id, &type_lowering, &item_signatures)
}

fn normalize_types_in_session_store(
    db: &QueryDb<CompilerContext>,
    module_id: ModuleId,
    type_lowering: &nia_type_lower::TypeLowering,
    item_signatures: &ItemSignatures,
) -> TypeNormalization {
    let input_ids = type_lowering.explicit_type_roots();
    nia_type_normalize::normalize_module_types(nia_type_normalize::TypeNormalizationInput {
        module_id,
        type_store: &db.context().type_store,
        input_ids: &input_ids,
        signatures: item_signatures,
    })
}
