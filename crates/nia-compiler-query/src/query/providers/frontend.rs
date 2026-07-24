// SPDX-License-Identifier: GPL-3.0-or-later
use super::*;

pub(super) fn provide_parse_ok_module_ids(db: &QueryDb<CompilerContext>) -> StableModuleSequence {
    let loaded_modules = resolve_stable_module_sequence(db, &db.get(LoadedModulesQuery));
    let module_ids = loaded_modules
        .into_iter()
        .filter(|module_id| {
            let parse_errors = db.get(ModuleParseErrorsQuery(*module_id));
            parse_errors.is_empty()
        })
        .collect::<Vec<_>>();
    stable_module_sequence(db, module_ids)
}

pub(super) fn provide_semantic_module_ids(db: &QueryDb<CompilerContext>) -> StableModuleSequence {
    let graph = db.get(ModuleGraphQuery);
    let entry = graph.entry();
    let module_ids =
        resolve_stable_module_sequence_from_current_inputs(db, &db.get(ParseOkModuleIdsQuery))
            .into_iter()
            .filter(|module_id| {
                graph
                    .get(*module_id)
                    .is_some_and(|node| *module_id == entry || node.process_used_paths)
            })
            .collect::<Vec<_>>();
    stable_module_sequence(db, module_ids)
}

pub(super) fn provide_module_item_tree(
    db: &QueryDb<CompilerContext>,
    module_id: ModuleId,
) -> ModuleItemTree {
    db.get(ModuleItemTreeInputQuery(module_id)).as_ref().clone()
}

pub(super) fn provide_active_module_item_tree(
    db: &QueryDb<CompilerContext>,
    module_id: ModuleId,
) -> ActiveModuleItemTree {
    let _raw_item_tree = db.get(ModuleItemTreeQuery(module_id));
    db.get(ActiveModuleItemTreeInputQuery(module_id))
        .as_ref()
        .clone()
}

pub(super) fn provide_full_module_item_tree(
    db: &QueryDb<CompilerContext>,
    module_id: ModuleId,
) -> ModuleItemTree {
    db.get(FullModuleItemTreeInputQuery(module_id))
        .as_ref()
        .clone()
}

pub(super) fn provide_full_active_module_item_tree(
    db: &QueryDb<CompilerContext>,
    module_id: ModuleId,
) -> ActiveModuleItemTree {
    let _raw_item_tree = db.get(FullModuleItemTreeQuery(module_id));
    db.get(FullActiveModuleItemTreeInputQuery(module_id))
        .as_ref()
        .clone()
}

pub(super) fn provide_module_defs(
    db: &QueryDb<CompilerContext>,
    module_id: ModuleId,
) -> DefCollection {
    let item_tree = db.get(ActiveModuleItemTreeQuery(module_id));
    let symbols = db.context().symbols();
    nia_defs::collect_module_defs_from_active_item_tree_with_node_store_and_symbols(
        module_id,
        &item_tree,
        db.context().node_store(),
        &symbols,
    )
}

pub(super) fn provide_full_module_defs(
    db: &QueryDb<CompilerContext>,
    module_id: ModuleId,
) -> DefCollection {
    let item_tree = db.get(FullActiveModuleItemTreeQuery(module_id));
    let symbols = db.context().symbols();
    nia_defs::collect_module_defs_from_active_item_tree_with_node_store_and_symbols(
        module_id,
        &item_tree,
        db.context().node_store(),
        &symbols,
    )
}

fn shared_defs_by_module(db: &QueryDb<CompilerContext>) -> Vec<Arc<DefCollection>> {
    let module_ids = resolve_stable_module_sequence(db, &db.get(ParseOkModuleIdsQuery));
    module_ids
        .into_iter()
        .map(|module_id| db.get(ModuleDefsQuery(module_id)))
        .collect()
}

fn shared_public_surface_defs_by_module(db: &QueryDb<CompilerContext>) -> Vec<DefCollection> {
    let module_ids = resolve_stable_module_sequence(db, &db.get(ParseOkModuleIdsQuery));
    module_ids
        .into_iter()
        .map(|module_id| {
            db.get(PublicSurfaceModuleFactsQuery(module_id))
                .materialize_for_public_surface(module_id)
        })
        .collect()
}

pub(super) fn provide_public_surfaces(db: &QueryDb<CompilerContext>) -> PublicSurfacesValue {
    time_provider(db.context().timings(), "public_surfaces", || {
        let defs = shared_public_surface_defs_by_module(db);
        let graph = db.get(ModuleGraphQuery);
        let symbols = db.context().symbols();
        let exports = compute_exported_public_surfaces_with_symbols(&defs, &graph, &symbols);
        PublicSurfacesQueryValue {
            surfaces: exports.surfaces,
            diagnostics: exports.diagnostics,
        }
    })
}

pub(super) fn provide_module_public_surface(
    db: &QueryDb<CompilerContext>,
    module_id: ModuleId,
) -> Option<Arc<ModulePublicSurface>> {
    db.get(PublicSurfacesQuery)
        .surfaces
        .public_surface(module_id)
}

pub(super) fn provide_public_using_scopes(db: &QueryDb<CompilerContext>) -> PublicUsingScopesValue {
    time_provider(db.context().timings(), "public_using_scopes", || {
        let defs = shared_public_surface_defs_by_module(db);
        let graph = db.get(ModuleGraphQuery);
        let public_surfaces = db.get(PublicSurfacesQuery);
        let symbols = db.context().symbols();
        let using_scopes = compute_using_scopes_from_surfaces_with_symbols(
            &defs,
            &graph,
            &public_surfaces.surfaces,
            &symbols,
        );
        PublicUsingScopesQueryValue {
            using_scopes: using_scopes.using_scopes,
            diagnostics: using_scopes.diagnostics,
        }
    })
}

pub(super) fn provide_module_using_scope(
    db: &QueryDb<CompilerContext>,
    module_id: ModuleId,
) -> ModuleUsingScope {
    db.get(PublicUsingScopesQuery)
        .using_scopes
        .get(&module_id)
        .cloned()
        .unwrap_or_default()
}

pub(super) fn provide_type_exposure_index(db: &QueryDb<CompilerContext>) -> TypeExposureIndexValue {
    time_provider(db.context().timings(), "type_exposure_index", || {
        let defs = shared_defs_by_module(db);
        let public_surfaces = db.get(PublicSurfacesQuery);
        let public_using_scopes = db.get(PublicUsingScopesQuery);
        TypeExposureIndex::from_defs_surfaces_and_using_scopes(
            &defs,
            &public_surfaces.surfaces,
            &public_using_scopes.using_scopes,
        )
    })
}

pub(super) fn provide_type_resolution(
    db: &QueryDb<CompilerContext>,
    module_id: ModuleId,
) -> TypeResolution {
    time_module_provider(db, "type_resolution", module_id, || {
        let active_item_tree = db.get(FullActiveModuleItemTreeQuery(module_id));
        let defs = db.get(FullModuleDefsQuery(module_id));
        let program_defs = |module_id| Some(db.get(FullModuleDefsQuery(module_id)));
        let graph = QueryModuleGraphLookup::new(db);
        let public_surfaces = QueryPublicSurfaceLookup::new(db);
        let using_scope = QueryUsingScopeLookup::new(db, module_id);
        let symbols = db.context().symbols();
        nia_type_resolve::resolve_module_types_from_active_item_tree_with_symbols_in_store(
            &active_item_tree,
            &defs,
            nia_type_resolve::ProgramDefsContext {
                defs: Some(&program_defs),
                graph: Some(&graph),
            },
            &public_surfaces,
            &using_scope,
            &symbols,
            db.context().node_store(),
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
        nia_type_resolve::resolve_module_types_from_active_item_tree_with_symbols_in_store(
            &active_item_tree,
            &defs,
            nia_type_resolve::ProgramDefsContext {
                defs: Some(&program_defs),
                graph: Some(&graph),
            },
            &public_surfaces,
            &using_scope,
            &symbols,
            db.context().node_store(),
        )
    })
}

pub(super) fn provide_signature_type_resolution(
    db: &QueryDb<CompilerContext>,
    module_id: ModuleId,
    set: nia_item_tree::SignatureItemSet,
) -> TypeResolution {
    time_module_provider(db, "signature_type_resolution", module_id, || {
        let program_sources = db.get(FrontendProgramSourcesQuery);
        let cache_input = program_sources
            .as_ref()
            .as_ref()
            .and_then(|program_sources| {
                let source = program_sources.by_module.get(&module_id)?;
                let namespace = crate::FrontendCacheNamespace::new(
                    &db.context().loader_facts.target(),
                    db.context().loader_facts.runtime(),
                );
                let key = crate::FrontendSignatureTypeResolutionCacheKey::new(
                    namespace,
                    &source.module,
                    set,
                    program_sources.fingerprint,
                );
                Some((program_sources, source, namespace, key))
            });
        let symbols = db.context().symbols();
        let cached = if let Some(cache) = db.context().signature_cache.as_ref()
            && let Some((program_sources, source, namespace, key)) = cache_input
        {
            match cache.load_type_resolution(
                crate::signature_cache::SignatureTypeResolutionIdentity {
                    key,
                    namespace,
                    module: &source.module,
                    set,
                    program_sources: program_sources.fingerprint,
                    source_version: source.version,
                    source_len: source.len,
                },
                &program_sources.module_by_path,
                &symbols,
                db.context().node_store(),
            ) {
                Ok(lookup) => {
                    match lookup {
                        crate::signature_cache::SignatureTypeResolutionLookup::Hit(_) => {
                            nia_timing::emit_counter(
                                "frontend.signature_type_resolution_reuse_hits",
                                1,
                            );
                        }
                        crate::signature_cache::SignatureTypeResolutionLookup::NotFound => {
                            nia_timing::emit_counter(
                                "frontend.signature_type_resolution_reuse_miss_not_found",
                                1,
                            );
                        }
                        crate::signature_cache::SignatureTypeResolutionLookup::Corrupt => {
                            nia_timing::emit_counter(
                                "frontend.signature_type_resolution_reuse_miss_corrupt",
                                1,
                            );
                        }
                    }
                    Some(lookup)
                }
                Err(_) => {
                    nia_timing::emit_counter(
                        "frontend.signature_type_resolution_reuse_miss_read_error",
                        1,
                    );
                    None
                }
            }
        } else {
            None
        };
        if let Some(crate::signature_cache::SignatureTypeResolutionLookup::Hit(cached)) = &cached
            && !db.context().verify_frontend_cache
        {
            return cached.as_ref().clone();
        }
        let active_item_tree = db.get(SignatureItemTreeQuery(module_id, set));
        let defs = db.get(ModuleDefsQuery(module_id));
        let program_defs = |module_id| Some(db.get(ModuleDefsQuery(module_id)));
        let graph = QueryModuleGraphLookup::new(db);
        let public_surfaces = QueryPublicSurfaceLookup::new(db);
        let using_scope = QueryUsingScopeLookup::new(db, module_id);
        let fresh = nia_type_resolve::resolve_module_declaration_types_from_active_item_tree_with_symbols_in_store(
            &active_item_tree,
            &defs,
            nia_type_resolve::ProgramDefsContext {
                defs: Some(&program_defs),
                graph: Some(&graph),
            },
            &public_surfaces,
            &using_scope,
            &symbols,
            db.context().node_store(),
        );
        if fresh.diagnostics.is_empty()
            && let Some(cache) = &db.context().signature_cache
            && let Some((program_sources, source, namespace, key)) = cache_input
        {
            let replace = matches!(
                &cached,
                Some(crate::signature_cache::SignatureTypeResolutionLookup::Hit(cached))
                    if cached.as_ref() != &fresh
            );
            if replace {
                cache.remove_type_resolution(key);
            }
            let _ = cache.publish_type_resolution(
                crate::signature_cache::SignatureTypeResolutionIdentity {
                    key,
                    namespace,
                    module: &source.module,
                    set,
                    program_sources: program_sources.fingerprint,
                    source_version: source.version,
                    source_len: source.len,
                },
                &fresh,
                &program_sources.path_by_module,
                &symbols,
                replace,
            );
        }
        fresh
    })
}

pub(super) fn provide_signature_const_type_resolution(
    db: &QueryDb<CompilerContext>,
    module_id: ModuleId,
) -> TypeResolution {
    time_module_provider(db, "signature_const_type_resolution", module_id, || {
        let active_item_tree = db.get(SignatureConstItemTreeQuery(module_id));
        let defs = db.get(ModuleDefsQuery(module_id));
        let program_defs = |module_id| Some(db.get(ModuleDefsQuery(module_id)));
        let graph = QueryModuleGraphLookup::new(db);
        let public_surfaces = QueryPublicSurfaceLookup::new(db);
        let using_scope = QueryUsingScopeLookup::new(db, module_id);
        let symbols = db.context().symbols();
        nia_type_resolve::resolve_module_types_from_active_item_tree_with_symbols_in_store(
            &active_item_tree,
            &defs,
            nia_type_resolve::ProgramDefsContext {
                defs: Some(&program_defs),
                graph: Some(&graph),
            },
            &public_surfaces,
            &using_scope,
            &symbols,
            db.context().node_store(),
        )
    })
}

pub(super) fn provide_type_lowering(
    db: &QueryDb<CompilerContext>,
    module_id: ModuleId,
) -> TypeLowering {
    let active_item_tree = db.get(FullActiveModuleItemTreeQuery(module_id));
    let type_resolution = db.get(TypeResolutionQuery(module_id));
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
    let type_resolution = db.get(DeclarationTypeResolutionQuery(module_id));
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
    let program_sources = db.get(FrontendProgramSourcesQuery);
    let cache_input = program_sources
        .as_ref()
        .as_ref()
        .and_then(|program_sources| {
            let source = program_sources.by_module.get(&module_id)?;
            let namespace = crate::FrontendCacheNamespace::new(
                &db.context().loader_facts.target(),
                db.context().loader_facts.runtime(),
            );
            let key = crate::FrontendSignatureTypeLoweringCacheKey::new(
                namespace,
                &source.module,
                set,
                program_sources.fingerprint,
            );
            Some((program_sources, source, namespace, key))
        });
    let symbols = db.context().symbols();
    let cached = if let Some(cache) = db.context().signature_cache.as_ref()
        && let Some((program_sources, source, namespace, key)) = cache_input
    {
        match cache.load_type_lowering(
            crate::signature_cache::SignatureTypeLoweringIdentity {
                key,
                namespace,
                module: &source.module,
                set,
                program_sources: program_sources.fingerprint,
                source_version: source.version,
                source_len: source.len,
            },
            &program_sources.module_by_path,
            &symbols,
            db.context().type_store(),
        ) {
            Ok(lookup) => {
                match lookup {
                    crate::signature_cache::SignatureTypeLoweringLookup::Hit(_) => {
                        nia_timing::emit_counter("frontend.signature_type_lowering_reuse_hits", 1);
                    }
                    crate::signature_cache::SignatureTypeLoweringLookup::NotFound => {
                        nia_timing::emit_counter(
                            "frontend.signature_type_lowering_reuse_miss_not_found",
                            1,
                        );
                    }
                    crate::signature_cache::SignatureTypeLoweringLookup::Corrupt => {
                        nia_timing::emit_counter(
                            "frontend.signature_type_lowering_reuse_miss_corrupt",
                            1,
                        );
                    }
                }
                Some(lookup)
            }
            Err(_) => {
                nia_timing::emit_counter(
                    "frontend.signature_type_lowering_reuse_miss_read_error",
                    1,
                );
                None
            }
        }
    } else {
        None
    };
    if let Some(crate::signature_cache::SignatureTypeLoweringLookup::Hit(cached)) = &cached
        && !db.context().verify_frontend_cache
    {
        return cached.as_ref().clone();
    }
    let active_item_tree = db.get(SignatureItemTreeQuery(module_id, set));
    let type_resolution = db.get(SignatureTypeResolutionQuery(module_id, set));
    let program_defs = |module_id| Some(db.get(ModuleDefsQuery(module_id)));
    let lowering =
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
        );
    nia_timing::emit_counter(
        if lowering.const_exprs.is_empty() && lowering.const_expr_summaries.is_empty() {
            "frontend.signature_type_lowering_cacheable"
        } else {
            "frontend.signature_type_lowering_has_const_exprs"
        },
        1,
    );
    if let Some(cache) = &db.context().signature_cache
        && let Some((program_sources, source, namespace, key)) = cache_input
    {
        let replace = matches!(
            &cached,
            Some(crate::signature_cache::SignatureTypeLoweringLookup::Hit(cached))
                if cached.as_ref() != &lowering
        );
        if replace {
            cache.remove_type_lowering(key);
        }
        if lowering.diagnostics.is_empty()
            && lowering.const_exprs.is_empty()
            && lowering.const_expr_summaries.is_empty()
        {
            let _ = cache.publish_type_lowering(
                crate::signature_cache::SignatureTypeLoweringIdentity {
                    key,
                    namespace,
                    module: &source.module,
                    set,
                    program_sources: program_sources.fingerprint,
                    source_version: source.version,
                    source_len: source.len,
                },
                &lowering,
                &program_sources.path_by_module,
                &symbols,
                db.context().type_store(),
                replace,
            );
        }
    }
    lowering
}

pub(super) fn provide_signature_const_type_lowering(
    db: &QueryDb<CompilerContext>,
    module_id: ModuleId,
) -> TypeLowering {
    let active_item_tree = db.get(SignatureConstItemTreeQuery(module_id));
    let type_resolution = db.get(SignatureConstTypeResolutionQuery(module_id));
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
    let type_lowering = db.get(DeclarationTypeLoweringQuery(module_id));
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
    let program_sources = db.get(FrontendProgramSourcesQuery);
    let cache_input = program_sources
        .as_ref()
        .as_ref()
        .and_then(|program_sources| {
            let source = program_sources.by_module.get(&module_id)?;
            let namespace = crate::FrontendCacheNamespace::new(
                &db.context().loader_facts.target(),
                db.context().loader_facts.runtime(),
            );
            let key = crate::FrontendSignatureItemSignaturesCacheKey::new(
                namespace,
                &source.module,
                set,
                program_sources.fingerprint,
            );
            Some((program_sources, source, namespace, key))
        });
    let symbols = db.context().symbols();
    let cached = if let Some(cache) = db.context().signature_cache.as_ref()
        && let Some((program_sources, source, namespace, key)) = cache_input
    {
        match cache.load_item_signatures(
            crate::signature_cache::SignatureItemSignaturesIdentity {
                key,
                namespace,
                module: &source.module,
                set,
                program_sources: program_sources.fingerprint,
                source_len: source.len,
            },
            &program_sources.module_by_path,
            &symbols,
            db.context().type_store(),
        ) {
            Ok(lookup) => {
                match lookup {
                    crate::signature_cache::SignatureItemSignaturesLookup::Hit(_) => {
                        nia_timing::emit_counter(
                            "frontend.signature_item_signatures_reuse_hits",
                            1,
                        );
                    }
                    crate::signature_cache::SignatureItemSignaturesLookup::NotFound => {
                        nia_timing::emit_counter(
                            "frontend.signature_item_signatures_reuse_miss_not_found",
                            1,
                        );
                    }
                    crate::signature_cache::SignatureItemSignaturesLookup::Corrupt => {
                        nia_timing::emit_counter(
                            "frontend.signature_item_signatures_reuse_miss_corrupt",
                            1,
                        );
                    }
                }
                Some(lookup)
            }
            Err(_) => {
                nia_timing::emit_counter(
                    "frontend.signature_item_signatures_reuse_miss_read_error",
                    1,
                );
                None
            }
        }
    } else {
        None
    };
    if let Some(crate::signature_cache::SignatureItemSignaturesLookup::Hit(cached)) = &cached
        && !db.context().verify_frontend_cache
    {
        return cached.as_ref().clone();
    }
    let active_item_tree = db.get(SignatureItemTreeQuery(module_id, set));
    let defs = db.get(ModuleDefsQuery(module_id));
    let type_lowering = db.get(SignatureTypeLoweringQuery(module_id, set));
    let fresh =
        nia_item_signatures::collect_item_signatures(nia_item_signatures::ItemSignatureInput {
            source: nia_item_signatures::ItemSignatureSource::ActiveItemTree(&active_item_tree),
            defs: &defs,
            lowered: &type_lowering,
            type_store: db.context().type_store(),
            symbols: Some(&symbols),
        });
    let cacheable = fresh.diagnostics.is_empty()
        && type_lowering.diagnostics.is_empty()
        && type_lowering.const_exprs.is_empty()
        && type_lowering.const_expr_summaries.is_empty();
    nia_timing::emit_counter(
        if cacheable {
            "frontend.signature_item_signatures_cacheable"
        } else {
            "frontend.signature_item_signatures_uncacheable"
        },
        1,
    );
    if let Some(cache) = &db.context().signature_cache
        && let Some((program_sources, source, namespace, key)) = cache_input
    {
        let replace = matches!(
            &cached,
            Some(crate::signature_cache::SignatureItemSignaturesLookup::Hit(cached))
                if cached.as_ref() != &fresh
        );
        if replace {
            cache.remove_item_signatures(key);
        }
        if cacheable {
            let _ = cache.publish_item_signatures(
                crate::signature_cache::SignatureItemSignaturesIdentity {
                    key,
                    namespace,
                    module: &source.module,
                    set,
                    program_sources: program_sources.fingerprint,
                    source_len: source.len,
                },
                &fresh,
                &program_sources.path_by_module,
                &symbols,
                db.context().type_store(),
                replace,
            );
        }
    }
    fresh
}

pub(super) fn provide_signature_const_item_signatures(
    db: &QueryDb<CompilerContext>,
    module_id: ModuleId,
) -> ItemSignatures {
    let active_item_tree = db.get(SignatureConstItemTreeQuery(module_id));
    let defs = db.get(ModuleDefsQuery(module_id));
    let type_lowering = db.get(SignatureConstTypeLoweringQuery(module_id));
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
    let type_lowering = db.get(TypeLoweringQuery(module_id));
    let item_signatures = db.get(ItemSignaturesQuery(module_id));
    normalize_types_in_session_store(db, module_id, &type_lowering, &item_signatures)
}

pub(super) fn provide_layout_type_normalization(
    db: &QueryDb<CompilerContext>,
    module_id: ModuleId,
) -> TypeNormalization {
    let type_lowering = db.get(TypeLoweringQuery(module_id));
    let item_signatures = db.get(ItemSignaturesQuery(module_id));
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
    let type_lowering = db.get(SignatureConstTypeLoweringQuery(module_id));
    let item_signatures = db.get(SignatureConstItemSignaturesQuery(module_id));
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
