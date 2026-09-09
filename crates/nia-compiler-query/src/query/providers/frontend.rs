// SPDX-License-Identifier: GPL-3.0-or-later
use super::*;

/// Materializes public declaration facts directly from a selected package
/// artifact. No source node or synthetic module identity is introduced.
pub(in crate::query) fn provide_artifact_public_surface_facts(
    db: &QueryDb<CompilerContext>,
    module_id: ModuleId,
) -> QueryResult<Option<PublicSurfaceModuleFacts>> {
    let Some(identity) = db
        .context()
        .loader_facts()
        .compiled_package_module_identity(module_id)?
    else {
        return Ok(None);
    };
    let index = db.get(CompiledPackageInterfaceIndexQuery)?;
    let Some(interface) = index.package(&identity.package) else {
        return Err(db.invalid_input(
            &CompiledPackageInterfaceIndexQuery,
            format!("artifact module owner is not selected: {identity:?}"),
        ));
    };
    let symbols = db.context().symbols();
    let mut defs = Vec::new();
    let mut modules = Vec::new();
    let mut types = Vec::new();
    let mut values = Vec::new();
    for record in interface.module_records(&identity.path) {
        let declaration =
            nia_package_metadata::decode_declaration(&record.declaration).map_err(|error| {
                db.invalid_input(&CompiledPackageInterfaceIndexQuery, error.to_string())
            })?;
        if declaration.visibility != 3 || record.definition.module != identity {
            return Err(db.invalid_input(
                &CompiledPackageInterfaceIndexQuery,
                format!(
                    "artifact declaration is not public or belongs to another module: {:?}",
                    record.definition
                ),
            ));
        }
        let kind = def_kind_from_tag(record.definition.kind).ok_or_else(|| {
            db.invalid_input(
                &CompiledPackageInterfaceIndexQuery,
                format!(
                    "artifact declaration has unknown definition kind: {}",
                    record.definition.kind
                ),
            )
        })?;
        let name = symbols.intern(&record.definition.name).map_err(|error| {
            db.invalid_input(&CompiledPackageInterfaceIndexQuery, error.to_string())
        })?;
        let id = nia_defs::stable_top_level_def_id(kind, name);
        defs.push(nia_defs::PublicSurfaceDefFact {
            id,
            name,
            kind,
            parent: None,
            visibility: nia_defs::Visibility::Public,
            span: Span::default(),
        });
        match kind {
            nia_defs::DefKind::Module => modules.push((name, id)),
            nia_defs::DefKind::Function | nia_defs::DefKind::Global | nia_defs::DefKind::Const => {
                values.push((name, id))
            }
            nia_defs::DefKind::Struct
            | nia_defs::DefKind::Union
            | nia_defs::DefKind::Trait
            | nia_defs::DefKind::Enum
            | nia_defs::DefKind::TypeAlias => types.push((name, id)),
            _ => {
                return Err(db.invalid_input(
                    &CompiledPackageInterfaceIndexQuery,
                    format!(
                        "artifact declaration is not top-level: {:?}",
                        record.definition
                    ),
                ));
            }
        }
    }
    defs.sort_by_key(|fact| fact.id);
    modules.sort_unstable();
    types.sort_unstable();
    values.sort_unstable();
    Ok(Some(PublicSurfaceModuleFacts {
        defs,
        module_scope: nia_defs::PublicSurfaceModuleScopeFacts {
            modules,
            types,
            values,
        },
        enum_scopes: Vec::new(),
        module_usings: Vec::new(),
    }))
}

/// Materializes a complete artifact public surface for direct lookup queries.
/// Stable targets are remapped through the loaded module graph and canonical
/// definition-id constructor; no source path is used as an identity key.
pub(in crate::query) fn provide_artifact_public_surface(
    db: &QueryDb<CompilerContext>,
    module_id: ModuleId,
) -> QueryResult<Option<ModulePublicSurface>> {
    let Some(identity) = db
        .context()
        .loader_facts()
        .compiled_package_module_identity(module_id)?
    else {
        return Ok(None);
    };
    let index = db.get(CompiledPackageInterfaceIndexQuery)?;
    let Some(interface) = index.package(&identity.package) else {
        return Ok(None);
    };
    let Some(section) = interface.public_surface() else {
        return Ok(None);
    };
    let Some(module) = section
        .modules
        .iter()
        .find(|module| module.path == identity.path)
    else {
        return Ok(None);
    };
    let symbols = db.context().symbols();
    let graph = db.get(ModuleGraphQuery)?;
    let resolve_module = |target: &nia_package_metadata::ModuleId| -> Option<ModuleId> {
        graph.modules().find_map(|node| {
            if let Ok(Some(identity)) = db
                .context()
                .loader_facts()
                .compiled_package_module_identity(node.id)
            {
                if identity == *target {
                    return Some(node.id);
                }
            }
            let key = graph.stable_key(node.id)?;
            (target.package == section.package
                && key.source_identity().normalized_path() == target.path)
                .then_some(node.id)
        })
    };
    let resolve_def = |target: &nia_package_metadata::DefinitionId,
                       parent: Option<&nia_package_metadata::DefinitionId>|
     -> QueryResult<nia_ids::GlobalDefId> {
        let target_module = resolve_module(&target.module).ok_or_else(|| {
            db.invalid_input(
                &CompiledPackageInterfaceIndexQuery,
                format!(
                    "artifact export target module is not loaded: {:?}",
                    target.module
                ),
            )
        })?;
        let name = symbols
            .intern(&target.name)
            .map_err(|e| db.invalid_input(&CompiledPackageInterfaceIndexQuery, e.to_string()))?;
        let kind = def_kind_from_tag(target.kind).ok_or_else(|| {
            db.invalid_input(
                &CompiledPackageInterfaceIndexQuery,
                "artifact export has unknown definition kind".to_string(),
            )
        })?;
        if let Some(parent) = parent {
            let parent_module = resolve_module(&parent.module).ok_or_else(|| {
                db.invalid_input(
                    &CompiledPackageInterfaceIndexQuery,
                    "artifact enum parent module is not loaded".to_string(),
                )
            })?;
            let parent_name = symbols.intern(&parent.name).map_err(|e| {
                db.invalid_input(&CompiledPackageInterfaceIndexQuery, e.to_string())
            })?;
            let parent_kind = def_kind_from_tag(parent.kind).ok_or_else(|| {
                db.invalid_input(
                    &CompiledPackageInterfaceIndexQuery,
                    "artifact enum parent has unknown kind".to_string(),
                )
            })?;
            let parent_global = GlobalDefId {
                module_id: parent_module,
                def_id: nia_defs::stable_top_level_def_id(parent_kind, parent_name),
            };
            let defs = db.get(FullModuleDefsQuery(target_module))?;
            if let Some((def_id, _)) = defs.semantic.defs.iter().find(|(_, def)| {
                def.name == name && def.kind == kind && def.parent == Some(parent_global.def_id)
            }) {
                return Ok(GlobalDefId {
                    module_id: target_module,
                    def_id,
                });
            }
            return Err(db.invalid_input(
                &CompiledPackageInterfaceIndexQuery,
                "artifact nested export definition is not present".to_string(),
            ));
        }
        Ok(GlobalDefId {
            module_id: target_module,
            def_id: nia_defs::stable_top_level_def_id(kind, name),
        })
    };
    let mut surface = ModulePublicSurface::new(module_id);
    for (name, child) in &module.modules {
        if let Some(child_id) = resolve_module(child) {
            surface.modules.insert(
                symbols.intern(name).map_err(|e| {
                    db.invalid_input(&CompiledPackageInterfaceIndexQuery, e.to_string())
                })?,
                child_id,
            );
        }
    }
    for export in &module.exports {
        let name = symbols
            .intern(&export.name)
            .map_err(|e| db.invalid_input(&CompiledPackageInterfaceIndexQuery, e.to_string()))?;
        let target = resolve_def(&export.target, export.parent_enum.as_ref())?;
        let parent_enum = export
            .parent_enum
            .as_ref()
            .map(|parent| resolve_def(parent, None))
            .transpose()?;
        let item = nia_defs::PublicItem {
            target_module: target.module_id,
            target_def_id: target.def_id,
            namespace: if export.namespace == 0 {
                nia_defs::PublicNamespace::Value
            } else {
                nia_defs::PublicNamespace::Type
            },
            name_span: Span::default(),
            source: if export.source == 0 {
                nia_defs::PublicSource::Direct
            } else {
                nia_defs::PublicSource::PubUsing {
                    directive_span: Span::default(),
                }
            },
            parent_enum,
        };
        if export.namespace == 0 {
            surface.values.insert(name, item);
        } else {
            surface.types.insert(name, item);
        }
    }
    Ok(Some(surface))
}

fn def_kind_from_tag(tag: u8) -> Option<nia_defs::DefKind> {
    use nia_defs::DefKind;
    Some(match tag {
        1 => DefKind::Module,
        2 => DefKind::Function,
        3 => DefKind::Global,
        4 => DefKind::Const,
        5 => DefKind::Struct,
        6 => DefKind::StructField,
        7 => DefKind::Union,
        8 => DefKind::UnionField,
        9 => DefKind::Trait,
        10 => DefKind::TraitAssociatedType,
        11 => DefKind::TraitMethod,
        12 => DefKind::Method,
        13 => DefKind::Enum,
        14 => DefKind::EnumVariant,
        15 => DefKind::EnumVariantField,
        16 => DefKind::TypeAlias,
        _ => return None,
    })
}

pub(super) fn provide_parse_ok_module_ids(
    db: &QueryDb<CompilerContext>,
) -> QueryResult<StableModuleSequence> {
    let loaded_modules = db.get(LoadedModulesQuery)?;
    let loaded_modules = resolve_stable_module_sequence(db, &loaded_modules)?;
    let mut module_ids = Vec::with_capacity(loaded_modules.len());
    for module_id in loaded_modules {
        if db.get(ModuleParseErrorsQuery(module_id))?.is_empty() {
            module_ids.push(module_id);
        }
    }
    stable_module_sequence(db, module_ids)
}

pub(super) fn provide_semantic_module_ids(
    db: &QueryDb<CompilerContext>,
) -> QueryResult<StableModuleSequence> {
    let graph = db.get(ModuleGraphQuery)?;
    let entry = graph.entry();
    let parse_ok_modules = db.get(ParseOkModuleIdsQuery)?;
    let module_ids = resolve_stable_module_sequence_from_current_inputs(db, &parse_ok_modules)?
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
) -> QueryResult<ModuleItemTree> {
    Ok(db
        .get(ModuleItemTreeInputQuery(module_id))?
        .as_ref()
        .clone())
}

pub(super) fn provide_active_module_item_tree(
    db: &QueryDb<CompilerContext>,
    module_id: ModuleId,
) -> QueryResult<ActiveModuleItemTree> {
    let _raw_item_tree = db.get(ModuleItemTreeQuery(module_id))?;
    Ok(db
        .get(ActiveModuleItemTreeInputQuery(module_id))?
        .as_ref()
        .clone())
}

pub(super) fn provide_full_module_item_tree(
    db: &QueryDb<CompilerContext>,
    module_id: ModuleId,
) -> QueryResult<ModuleItemTree> {
    Ok(db
        .get(FullModuleItemTreeInputQuery(module_id))?
        .as_ref()
        .clone())
}

pub(super) fn provide_full_active_module_item_tree(
    db: &QueryDb<CompilerContext>,
    module_id: ModuleId,
) -> QueryResult<ActiveModuleItemTree> {
    let _raw_item_tree = db.get(FullModuleItemTreeQuery(module_id))?;
    Ok(db
        .get(FullActiveModuleItemTreeInputQuery(module_id))?
        .as_ref()
        .clone())
}

pub(super) fn provide_module_defs(
    db: &QueryDb<CompilerContext>,
    module_id: ModuleId,
) -> QueryResult<ModuleDefinitions> {
    if let Some(facts) = provide_artifact_public_surface_facts(db, module_id)? {
        return Ok(ModuleDefinitions {
            semantic: Arc::new(facts.materialize_for_public_surface(module_id)),
            diagnostics: db.context().diagnostic_store.bundle(Vec::new()),
        });
    }
    let item_tree = db.get(ActiveModuleItemTreeQuery(module_id))?;
    let symbols = db.context().symbols();
    let mut defs = nia_defs::collect_module_defs_from_active_item_tree_with_node_store_and_symbols(
        module_id,
        &item_tree,
        db.context().node_store(),
        &symbols,
    );
    let diagnostics = std::mem::take(&mut defs.diagnostics);
    Ok(ModuleDefinitions {
        semantic: Arc::new(defs),
        diagnostics: db.context().diagnostic_store.bundle(diagnostics),
    })
}

pub(super) fn provide_full_module_defs(
    db: &QueryDb<CompilerContext>,
    module_id: ModuleId,
) -> QueryResult<FullModuleDefinitions> {
    if let Some(facts) = provide_artifact_public_surface_facts(db, module_id)? {
        return Ok(FullModuleDefinitions {
            semantic: Arc::new(facts.materialize_for_public_surface(module_id)),
            diagnostics: db.context().diagnostic_store.bundle(Vec::new()),
        });
    }
    let item_tree = db.get(FullActiveModuleItemTreeQuery(module_id))?;
    let symbols = db.context().symbols();
    let mut defs = nia_defs::collect_module_defs_from_active_item_tree_with_node_store_and_symbols(
        module_id,
        &item_tree,
        db.context().node_store(),
        &symbols,
    );
    let diagnostics = std::mem::take(&mut defs.diagnostics);
    Ok(FullModuleDefinitions {
        semantic: Arc::new(defs),
        diagnostics: db.context().diagnostic_store.bundle(diagnostics),
    })
}

fn shared_defs_by_module(db: &QueryDb<CompilerContext>) -> QueryResult<Vec<Arc<DefCollection>>> {
    let parse_ok_modules = db.get(ParseOkModuleIdsQuery)?;
    let _graph = db.get(ModuleGraphQuery)?;
    let module_ids = db
        .context()
        .resolve_stable_module_sequence(&parse_ok_modules)?;
    module_ids
        .into_iter()
        .map(|module_id| module_defs_semantic(db, module_id))
        .collect()
}

fn shared_public_surface_defs_by_module(
    db: &QueryDb<CompilerContext>,
) -> QueryResult<Vec<DefCollection>> {
    let parse_ok_modules = db.get(ParseOkModuleIdsQuery)?;
    let _graph = db.get(ModuleGraphQuery)?;
    let module_ids = db
        .context()
        .resolve_stable_module_sequence(&parse_ok_modules)?;
    let mut source_module_ids = Vec::new();
    for module_id in module_ids {
        if db
            .context()
            .loader_facts()
            .compiled_package_module_identity(module_id)?
            .is_none()
        {
            source_module_ids.push(module_id);
        }
    }
    source_module_ids
        .into_iter()
        .map(|module_id| {
            Ok(db
                .get(PublicSurfaceModuleFactsQuery(module_id))?
                .materialize_for_public_surface(module_id))
        })
        .collect()
}

pub(super) fn capture_query_failure<T>(
    failure: &RefCell<Option<QueryError>>,
    result: QueryResult<T>,
) -> Option<T> {
    match result {
        Ok(value) => Some(value),
        Err(error) => {
            if failure.borrow().is_none() {
                *failure.borrow_mut() = Some(error);
            }
            None
        }
    }
}

pub(super) fn provide_public_surfaces(
    db: &QueryDb<CompilerContext>,
) -> QueryResult<PublicSurfacesValue> {
    time_provider(db.context().timings(), "public_surfaces", || {
        let defs = shared_public_surface_defs_by_module(db)?;
        let graph = db.get(ModuleGraphQuery)?;
        let symbols = db.context().symbols();
        let exports = compute_exported_public_surfaces_with_symbols(&defs, &graph, &symbols);
        let mut surfaces = exports.surfaces;
        for module in graph.modules() {
            if let Some(surface) = provide_artifact_public_surface(db, module.id)? {
                surfaces.insert(surface);
            }
        }
        Ok(PublicSurfacesQueryValue {
            surfaces,
            diagnostics: store_module_diagnostics(
                &db.context().diagnostic_store,
                exports.diagnostics,
            ),
        })
    })
}

pub(super) fn provide_module_public_surface(
    db: &QueryDb<CompilerContext>,
    module_id: ModuleId,
) -> QueryResult<Option<Arc<ModulePublicSurface>>> {
    Ok(db
        .get(PublicSurfacesQuery)?
        .surfaces
        .public_surface(module_id))
}

pub(super) fn provide_public_using_scopes(
    db: &QueryDb<CompilerContext>,
) -> QueryResult<PublicUsingScopesValue> {
    time_provider(db.context().timings(), "public_using_scopes", || {
        let defs = shared_public_surface_defs_by_module(db)?;
        let graph = db.get(ModuleGraphQuery)?;
        let public_surfaces = db.get(PublicSurfacesQuery)?;
        let symbols = db.context().symbols();
        let using_scopes = compute_using_scopes_from_surfaces_with_symbols(
            &defs,
            &graph,
            &public_surfaces.surfaces,
            &symbols,
        );
        Ok(PublicUsingScopesQueryValue {
            using_scopes: using_scopes.using_scopes,
            diagnostics: store_module_diagnostics(
                &db.context().diagnostic_store,
                using_scopes.diagnostics,
            ),
        })
    })
}

pub(super) fn provide_module_using_scope(
    db: &QueryDb<CompilerContext>,
    module_id: ModuleId,
) -> QueryResult<ModuleUsingScope> {
    Ok(db
        .get(PublicUsingScopesQuery)?
        .using_scopes
        .get(&module_id)
        .cloned()
        .unwrap_or_default())
}

pub(super) fn provide_type_exposure_index(
    db: &QueryDb<CompilerContext>,
) -> QueryResult<TypeExposureIndexValue> {
    time_provider(db.context().timings(), "type_exposure_index", || {
        let defs = shared_defs_by_module(db)?;
        let public_surfaces = db.get(PublicSurfacesQuery)?;
        let public_using_scopes = db.get(PublicUsingScopesQuery)?;
        Ok(TypeExposureIndex::from_defs_surfaces_and_using_scopes(
            &defs,
            &public_surfaces.surfaces,
            &public_using_scopes.using_scopes,
        ))
    })
}

pub(super) fn provide_type_resolution(
    db: &QueryDb<CompilerContext>,
    module_id: ModuleId,
) -> QueryResult<ModuleTypeResolution> {
    time_module_provider(db, "type_resolution", module_id, || {
        let active_item_tree = db.get(FullActiveModuleItemTreeQuery(module_id))?;
        let defs = full_module_defs_semantic(db, module_id)?;
        let graph = db.get(ModuleGraphQuery)?;
        let public_surfaces = db.get(PublicSurfacesQuery)?;
        let using_scope = db.get(ModuleUsingScopeQuery(module_id))?;
        let query_failure = RefCell::new(None);
        let program_defs = |module_id| {
            capture_query_failure(&query_failure, full_module_defs_semantic(db, module_id))
        };
        let symbols = db.context().symbols();
        let mut resolution =
            nia_type_resolve::resolve_module_types_from_active_item_tree_with_symbols_in_store(
                &active_item_tree,
                &defs,
                nia_type_resolve::ProgramDefsContext {
                    defs: Some(&program_defs),
                    graph: Some(graph.as_ref()),
                },
                &public_surfaces.surfaces,
                using_scope.as_ref(),
                &symbols,
                db.context().node_store(),
            );
        let diagnostics = std::mem::take(&mut resolution.diagnostics);
        query_failure.into_inner().map_or_else(
            || {
                Ok(ModuleTypeResolution {
                    semantic: Arc::new(resolution),
                    diagnostics: db.context().diagnostic_store.bundle(diagnostics),
                })
            },
            Err,
        )
    })
}

pub(super) fn provide_declaration_type_resolution(
    db: &QueryDb<CompilerContext>,
    module_id: ModuleId,
) -> QueryResult<TypeResolution> {
    time_module_provider(db, "declaration_type_resolution", module_id, || {
        let active_item_tree = db.get(DeclarationActiveModuleItemTreeQuery(module_id))?;
        let defs = module_defs_semantic(db, module_id)?;
        let graph = db.get(ModuleGraphQuery)?;
        let public_surfaces = db.get(PublicSurfacesQuery)?;
        let using_scope = db.get(ModuleUsingScopeQuery(module_id))?;
        let query_failure = RefCell::new(None);
        let program_defs =
            |module_id| capture_query_failure(&query_failure, module_defs_semantic(db, module_id));
        let symbols = db.context().symbols();
        let resolution =
            nia_type_resolve::resolve_module_types_from_active_item_tree_with_symbols_in_store(
                &active_item_tree,
                &defs,
                nia_type_resolve::ProgramDefsContext {
                    defs: Some(&program_defs),
                    graph: Some(graph.as_ref()),
                },
                &public_surfaces.surfaces,
                using_scope.as_ref(),
                &symbols,
                db.context().node_store(),
            );
        query_failure.into_inner().map_or(Ok(resolution), Err)
    })
}

pub(super) fn provide_signature_type_resolution(
    db: &QueryDb<CompilerContext>,
    module_id: ModuleId,
    set: nia_item_tree::SignatureItemSet,
) -> QueryResult<SignatureTypeResolution> {
    time_module_provider(db, "signature_type_resolution", module_id, || {
        let program_sources = db.get(FrontendProgramSourcesQuery)?;
        let cache_input = program_sources
            .as_ref()
            .as_ref()
            .and_then(|program_sources| {
                let source = program_sources.by_module.get(&module_id)?;
                let namespace = db.context().frontend_cache_namespace();
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
            return Ok(SignatureTypeResolution {
                semantic: Arc::new(cached.as_ref().clone()),
                diagnostics: db.context().diagnostic_store.bundle(Vec::new()),
            });
        }
        let active_item_tree = db.get(SignatureItemTreeQuery(module_id, set))?;
        let defs = module_defs_semantic(db, module_id)?;
        let graph = db.get(ModuleGraphQuery)?;
        let public_surfaces = db.get(PublicSurfacesQuery)?;
        let using_scope = db.get(ModuleUsingScopeQuery(module_id))?;
        let query_failure = RefCell::new(None);
        let program_defs =
            |module_id| capture_query_failure(&query_failure, module_defs_semantic(db, module_id));
        let mut fresh = nia_type_resolve::resolve_module_declaration_types_from_active_item_tree_with_symbols_in_store(
            &active_item_tree,
            &defs,
            nia_type_resolve::ProgramDefsContext {
                defs: Some(&program_defs),
                graph: Some(graph.as_ref()),
            },
            &public_surfaces.surfaces,
            using_scope.as_ref(),
            &symbols,
            db.context().node_store(),
        );
        if let Some(error) = query_failure.into_inner() {
            return Err(error);
        }
        let diagnostics = std::mem::take(&mut fresh.diagnostics);
        if diagnostics.is_empty()
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
        Ok(SignatureTypeResolution {
            semantic: Arc::new(fresh),
            diagnostics: db.context().diagnostic_store.bundle(diagnostics),
        })
    })
}

pub(super) fn provide_signature_const_type_resolution(
    db: &QueryDb<CompilerContext>,
    module_id: ModuleId,
) -> QueryResult<TypeResolution> {
    time_module_provider(db, "signature_const_type_resolution", module_id, || {
        let active_item_tree = db.get(SignatureConstItemTreeQuery(module_id))?;
        let defs = module_defs_semantic(db, module_id)?;
        let graph = db.get(ModuleGraphQuery)?;
        let public_surfaces = db.get(PublicSurfacesQuery)?;
        let using_scope = db.get(ModuleUsingScopeQuery(module_id))?;
        let query_failure = RefCell::new(None);
        let program_defs =
            |module_id| capture_query_failure(&query_failure, module_defs_semantic(db, module_id));
        let symbols = db.context().symbols();
        let resolution =
            nia_type_resolve::resolve_module_types_from_active_item_tree_with_symbols_in_store(
                &active_item_tree,
                &defs,
                nia_type_resolve::ProgramDefsContext {
                    defs: Some(&program_defs),
                    graph: Some(graph.as_ref()),
                },
                &public_surfaces.surfaces,
                using_scope.as_ref(),
                &symbols,
                db.context().node_store(),
            );
        query_failure.into_inner().map_or(Ok(resolution), Err)
    })
}

pub(super) fn provide_type_lowering(
    db: &QueryDb<CompilerContext>,
    module_id: ModuleId,
) -> QueryResult<ModuleTypeLowering> {
    let active_item_tree = db.get(FullActiveModuleItemTreeQuery(module_id))?;
    let type_resolution = type_resolution_semantic(db, module_id)?;
    let query_failure = RefCell::new(None);
    let program_defs =
        |module_id| capture_query_failure(&query_failure, full_module_defs_semantic(db, module_id));
    let symbols = db.context().symbols();
    let mut lowering = nia_type_lower::lower_module_types_from_active_item_tree_with_context(
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
    let diagnostics = std::mem::take(&mut lowering.diagnostics);
    query_failure.into_inner().map_or_else(
        || {
            Ok(ModuleTypeLowering {
                semantic: Arc::new(lowering),
                diagnostics: db.context().diagnostic_store.bundle(diagnostics),
            })
        },
        Err,
    )
}

pub(super) fn provide_declaration_type_lowering(
    db: &QueryDb<CompilerContext>,
    module_id: ModuleId,
) -> QueryResult<TypeLowering> {
    let active_item_tree = db.get(DeclarationActiveModuleItemTreeQuery(module_id))?;
    let type_resolution = db.get(DeclarationTypeResolutionQuery(module_id))?;
    let query_failure = RefCell::new(None);
    let program_defs =
        |module_id| capture_query_failure(&query_failure, module_defs_semantic(db, module_id));
    let symbols = db.context().symbols();
    let lowering = nia_type_lower::lower_module_types_from_active_item_tree_with_context(
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
    query_failure.into_inner().map_or(Ok(lowering), Err)
}

pub(super) fn provide_signature_type_lowering(
    db: &QueryDb<CompilerContext>,
    module_id: ModuleId,
    set: nia_item_tree::SignatureItemSet,
) -> QueryResult<SignatureTypeLowering> {
    let program_sources = db.get(FrontendProgramSourcesQuery)?;
    let cache_input = program_sources
        .as_ref()
        .as_ref()
        .and_then(|program_sources| {
            let source = program_sources.by_module.get(&module_id)?;
            let namespace = db.context().frontend_cache_namespace();
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
        return Ok(SignatureTypeLowering {
            semantic: Arc::new(cached.as_ref().clone()),
            diagnostics: db.context().diagnostic_store.bundle(Vec::new()),
        });
    }
    let active_item_tree = db.get(SignatureItemTreeQuery(module_id, set))?;
    let type_resolution = db.get(SignatureTypeResolutionQuery(module_id, set))?;
    let query_failure = RefCell::new(None);
    let program_defs =
        |module_id| capture_query_failure(&query_failure, module_defs_semantic(db, module_id));
    let mut lowering =
        nia_type_lower::lower_module_declaration_types_from_active_item_tree_with_context(
            module_id,
            &active_item_tree,
            &type_resolution.semantic,
            nia_type_lower::TypeLoweringContext::from_program_defs(
                db.context().type_store(),
                nia_type_lower::ProgramDefsContext {
                    defs: Some(&program_defs),
                },
            )
            .with_symbols(&symbols),
        );
    let diagnostics = std::mem::take(&mut lowering.diagnostics);
    if db.context().timings().enabled() {
        nia_timing::emit_counter(
            if lowering.const_exprs.is_empty() && lowering.const_expr_summaries.is_empty() {
                "frontend.signature_type_lowering_cacheable"
            } else {
                "frontend.signature_type_lowering_has_const_exprs"
            },
            1,
        );
    }
    if let Some(error) = query_failure.into_inner() {
        return Err(error);
    }
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
        if diagnostics.is_empty()
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
    Ok(SignatureTypeLowering {
        semantic: Arc::new(lowering),
        diagnostics: db.context().diagnostic_store.bundle(diagnostics),
    })
}

pub(super) fn provide_signature_const_type_lowering(
    db: &QueryDb<CompilerContext>,
    module_id: ModuleId,
) -> QueryResult<TypeLowering> {
    let active_item_tree = db.get(SignatureConstItemTreeQuery(module_id))?;
    let type_resolution = db.get(SignatureConstTypeResolutionQuery(module_id))?;
    let query_failure = RefCell::new(None);
    let program_defs =
        |module_id| capture_query_failure(&query_failure, module_defs_semantic(db, module_id));
    let symbols = db.context().symbols();
    let lowering = nia_type_lower::lower_module_types_from_active_item_tree_with_context(
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
    query_failure.into_inner().map_or(Ok(lowering), Err)
}

pub(super) fn provide_item_signatures(
    db: &QueryDb<CompilerContext>,
    module_id: ModuleId,
) -> QueryResult<ModuleItemSignatures> {
    let active_item_tree = db.get(DeclarationActiveModuleItemTreeQuery(module_id))?;
    let defs = module_defs_semantic(db, module_id)?;
    let type_lowering = db.get(DeclarationTypeLoweringQuery(module_id))?;
    let symbols = db.context().symbols();
    let mut signatures =
        nia_item_signatures::collect_item_signatures(nia_item_signatures::ItemSignatureInput {
            source: nia_item_signatures::ItemSignatureSource::ActiveItemTree(&active_item_tree),
            defs: &defs,
            lowered: &type_lowering,
            type_store: db.context().type_store(),
            symbols: Some(&symbols),
        });
    let diagnostics = std::mem::take(&mut signatures.diagnostics);
    Ok(ModuleItemSignatures {
        semantic: Arc::new(signatures),
        diagnostics: db.context().diagnostic_store.bundle(diagnostics),
    })
}

pub(super) fn provide_signature_item_signatures(
    db: &QueryDb<CompilerContext>,
    module_id: ModuleId,
    set: nia_item_tree::SignatureItemSet,
) -> QueryResult<SignatureItemSignatures> {
    let program_sources = db.get(FrontendProgramSourcesQuery)?;
    let cache_input = program_sources
        .as_ref()
        .as_ref()
        .and_then(|program_sources| {
            let source = program_sources.by_module.get(&module_id)?;
            let namespace = db.context().frontend_cache_namespace();
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
        return Ok(SignatureItemSignatures {
            semantic: Arc::new(cached.as_ref().clone()),
            diagnostics: db.context().diagnostic_store.bundle(Vec::new()),
        });
    }
    let active_item_tree = db.get(SignatureItemTreeQuery(module_id, set))?;
    let defs = module_defs_semantic(db, module_id)?;
    let type_lowering = db.get(SignatureTypeLoweringQuery(module_id, set))?;
    let mut fresh =
        nia_item_signatures::collect_item_signatures(nia_item_signatures::ItemSignatureInput {
            source: nia_item_signatures::ItemSignatureSource::ActiveItemTree(&active_item_tree),
            defs: &defs,
            lowered: &type_lowering.semantic,
            type_store: db.context().type_store(),
            symbols: Some(&symbols),
        });
    let diagnostics = std::mem::take(&mut fresh.diagnostics);
    let cacheable = diagnostics.is_empty()
        && resolve_diagnostic_bundle(db.context(), &type_lowering.diagnostics).is_empty()
        && type_lowering.semantic.const_exprs.is_empty()
        && type_lowering.semantic.const_expr_summaries.is_empty();
    if db.context().timings().enabled() {
        nia_timing::emit_counter(
            if cacheable {
                "frontend.signature_item_signatures_cacheable"
            } else {
                "frontend.signature_item_signatures_uncacheable"
            },
            1,
        );
    }
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
    Ok(SignatureItemSignatures {
        semantic: Arc::new(fresh),
        diagnostics: db.context().diagnostic_store.bundle(diagnostics),
    })
}

pub(super) fn provide_signature_const_item_signatures(
    db: &QueryDb<CompilerContext>,
    module_id: ModuleId,
) -> QueryResult<ItemSignatures> {
    let active_item_tree = db.get(SignatureConstItemTreeQuery(module_id))?;
    let defs = module_defs_semantic(db, module_id)?;
    let type_lowering = db.get(SignatureConstTypeLoweringQuery(module_id))?;
    let symbols = db.context().symbols();
    Ok(nia_item_signatures::collect_item_signatures(
        nia_item_signatures::ItemSignatureInput {
            source: nia_item_signatures::ItemSignatureSource::ActiveItemTree(&active_item_tree),
            defs: &defs,
            lowered: &type_lowering,
            type_store: db.context().type_store(),
            symbols: Some(&symbols),
        },
    ))
}

pub(super) fn provide_type_normalization(
    db: &QueryDb<CompilerContext>,
    module_id: ModuleId,
) -> QueryResult<ModuleTypeNormalization> {
    let type_lowering = type_lowering_semantic(db, module_id)?;
    let item_signatures = item_signatures_semantic(db, module_id)?;
    let mut normalization =
        normalize_types_in_session_store(db, module_id, &type_lowering, &item_signatures);
    let diagnostics = std::mem::take(&mut normalization.diagnostics);
    Ok(ModuleTypeNormalization {
        semantic: Arc::new(normalization),
        diagnostics: db.context().diagnostic_store.bundle(diagnostics),
    })
}

pub(super) fn provide_layout_type_normalization(
    db: &QueryDb<CompilerContext>,
    module_id: ModuleId,
) -> QueryResult<TypeNormalization> {
    let type_lowering = type_lowering_semantic(db, module_id)?;
    let item_signatures = item_signatures_semantic(db, module_id)?;
    Ok(normalize_types_in_session_store(
        db,
        module_id,
        &type_lowering,
        &item_signatures,
    ))
}

pub(super) fn provide_signature_type_normalization(
    db: &QueryDb<CompilerContext>,
    module_id: ModuleId,
    set: nia_item_tree::SignatureItemSet,
) -> QueryResult<SignatureTypeNormalization> {
    let type_lowering = db.get(SignatureTypeLoweringQuery(module_id, set))?;
    let item_signatures = db.get(SignatureItemSignaturesQuery(module_id, set))?;
    let mut normalization = normalize_types_in_session_store(
        db,
        module_id,
        &type_lowering.semantic,
        &item_signatures.semantic,
    );
    let diagnostics = std::mem::take(&mut normalization.diagnostics);
    Ok(SignatureTypeNormalization {
        semantic: Arc::new(normalization),
        diagnostics: db.context().diagnostic_store.bundle(diagnostics),
    })
}

pub(super) fn provide_signature_const_type_normalization(
    db: &QueryDb<CompilerContext>,
    module_id: ModuleId,
) -> QueryResult<TypeNormalization> {
    let type_lowering = db.get(SignatureConstTypeLoweringQuery(module_id))?;
    let item_signatures = db.get(SignatureConstItemSignaturesQuery(module_id))?;
    Ok(normalize_types_in_session_store(
        db,
        module_id,
        &type_lowering,
        &item_signatures,
    ))
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
