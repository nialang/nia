use crate::LoaderContext;
use crate::graph::{
    TraversalResult, add_visible_declared_module_path, mark_process_used_paths_and_process,
    used_path_start,
};
use crate::queries::{module_facade_facts_query, provider_summary_query};
use crate::used_paths::{UsedModulePath, UsedModulePathProcessing};
use nia_imports::ModuleGraph;
use nia_query::QueryDb;
use nia_source::SourcePath;
use nia_symbol::{SymbolId, known};
use std::collections::HashSet;

pub(crate) fn process_reexport_provider_request(
    db: &QueryDb<LoaderContext>,
    graph: &mut ModuleGraph,
    facade_module: nia_imports::ModuleId,
    exported_name: &SymbolId,
    processing: &UsedModulePathProcessing,
) -> TraversalResult<()> {
    match processing {
        UsedModulePathProcessing::IfProvidesTraitImpl {
            target_type_name,
            trait_name,
        } if trait_name == exported_name => add_public_reexport_trait_impl_provider_modules(
            db,
            graph,
            facade_module,
            target_type_name.as_ref(),
            trait_name,
        ),
        UsedModulePathProcessing::IfProvidesImplicitTraitImpl { trait_name } => {
            let prefer_selected = graph.get(facade_module).is_some_and(|node| {
                !node.module_path.is_package_root() && *trait_name != known::LEN_TYPE
            });
            add_implicit_trait_impl_provider_modules(
                db,
                graph,
                facade_module,
                trait_name,
                prefer_selected,
            )
        }
        UsedModulePathProcessing::IfProvidesTraitMethod {
            target_type_name,
            associated_name,
        } => add_public_reexport_trait_method_provider_modules(
            db,
            graph,
            facade_module,
            target_type_name.as_ref(),
            associated_name,
        ),
        _ => Ok(()),
    }
}

pub(crate) fn process_provider_request(
    db: &QueryDb<LoaderContext>,
    graph: &mut ModuleGraph,
    module_id: nia_imports::ModuleId,
    processing: &UsedModulePathProcessing,
) -> TraversalResult<()> {
    if direct_provider_module_matches_request(db, graph, module_id, processing)? {
        mark_process_used_paths_and_process(db, graph, module_id)?;
    }
    match processing {
        UsedModulePathProcessing::IfProvidesTraitImpl {
            target_type_name,
            trait_name,
        } => add_public_reexport_trait_impl_provider_modules(
            db,
            graph,
            module_id,
            target_type_name.as_ref(),
            trait_name,
        ),
        UsedModulePathProcessing::IfProvidesImplicitTraitImpl { trait_name } => {
            let prefer_selected = graph.get(module_id).is_some_and(|node| {
                !node.module_path.is_package_root() && *trait_name != known::LEN_TYPE
            });
            add_implicit_trait_impl_provider_modules(
                db,
                graph,
                module_id,
                trait_name,
                prefer_selected,
            )
        }
        UsedModulePathProcessing::IfProvidesTraitMethod {
            target_type_name,
            associated_name,
        } => add_public_reexport_trait_method_provider_modules(
            db,
            graph,
            module_id,
            target_type_name.as_ref(),
            associated_name,
        ),
        UsedModulePathProcessing::Never
        | UsedModulePathProcessing::Always
        | UsedModulePathProcessing::IfSelectedItem
        | UsedModulePathProcessing::IfProvidesExtensions => Ok(()),
    }
}

fn direct_provider_module_matches_request(
    db: &QueryDb<LoaderContext>,
    graph: &ModuleGraph,
    module_id: nia_imports::ModuleId,
    processing: &UsedModulePathProcessing,
) -> TraversalResult<bool> {
    let Some(node) = graph.get(module_id) else {
        return Ok(false);
    };
    match processing {
        UsedModulePathProcessing::IfProvidesTraitImpl {
            target_type_name,
            trait_name,
        } => provider_candidate_has_trait_impl(
            db,
            node.path.clone(),
            target_type_name.as_ref(),
            trait_name,
            None,
        ),
        UsedModulePathProcessing::IfProvidesImplicitTraitImpl { trait_name } => {
            provider_candidate_has_trait_impl(db, node.path.clone(), None, trait_name, None)
        }
        UsedModulePathProcessing::IfProvidesTraitMethod {
            target_type_name,
            associated_name,
        } => {
            let summary = db.get(provider_summary_query(db, &node.path)?)?;
            Ok(summary.defines_public_extension_method_for_facade(
                |_| true,
                target_type_name.as_ref(),
                associated_name,
            ))
        }
        UsedModulePathProcessing::Never
        | UsedModulePathProcessing::Always
        | UsedModulePathProcessing::IfSelectedItem
        | UsedModulePathProcessing::IfProvidesExtensions => Ok(false),
    }
}

pub(crate) fn add_public_reexport_source_module(
    db: &QueryDb<LoaderContext>,
    graph: &mut ModuleGraph,
    module_id: nia_imports::ModuleId,
    name: &SymbolId,
    processing: Option<UsedModulePathProcessing>,
) -> TraversalResult<Option<nia_imports::ModuleId>> {
    let Some(node) = graph.get(module_id).cloned() else {
        return Ok(None);
    };
    let facts = db.get(module_facade_facts_query(db, &node.path)?)?;
    for source_path in facts.reexport_source_paths(name) {
        let source_path = processing.as_ref().map_or_else(
            || source_path.clone(),
            |processing| {
                source_path.with_appended_segments_with_processing_mode(
                    &[],
                    source_path.include_declared_children(),
                    processing.clone(),
                )
            },
        );
        let Some(start) = used_path_start(graph, module_id, &source_path) else {
            continue;
        };
        return add_visible_declared_module_path(
            db,
            graph,
            module_id,
            start,
            source_path.segments(),
            source_path.processing(),
        );
    }
    Ok(None)
}

fn add_public_reexport_trait_impl_provider_modules(
    db: &QueryDb<LoaderContext>,
    graph: &mut ModuleGraph,
    facade_module: nia_imports::ModuleId,
    target_type_name: Option<&SymbolId>,
    trait_name: &SymbolId,
) -> TraversalResult<()> {
    let Some(node) = graph.get(facade_module).cloned() else {
        return Ok(());
    };
    let facts = db.get(module_facade_facts_query(db, &node.path)?)?;
    add_trait_provider_modules_matching(
        db,
        graph,
        facade_module,
        &facts,
        |db, path| provider_candidate_has_trait_impl(db, path, target_type_name, trait_name, None),
        false,
    )
}

fn add_implicit_trait_impl_provider_modules(
    db: &QueryDb<LoaderContext>,
    graph: &mut ModuleGraph,
    facade_module: nia_imports::ModuleId,
    trait_name: &SymbolId,
    prefer_selected_branches: bool,
) -> TraversalResult<()> {
    let Some(node) = graph.get(facade_module).cloned() else {
        return Ok(());
    };
    if prefer_selected_branches && node.semantic_selected && !node.process_used_paths {
        // A shallow facade may be semantic-selected only to resolve a public
        // name. It must not turn that visibility scaffold into an implicit
        // sibling-provider search.
        return Ok(());
    }
    let facts = db.get(module_facade_facts_query(db, &node.path)?)?;
    add_trait_provider_modules_matching(
        db,
        graph,
        facade_module,
        &facts,
        |db, path| provider_candidate_has_trait_impl(db, path, None, trait_name, None),
        prefer_selected_branches,
    )
}

fn add_public_reexport_trait_method_provider_modules(
    db: &QueryDb<LoaderContext>,
    graph: &mut ModuleGraph,
    facade_module: nia_imports::ModuleId,
    target_type_name: Option<&SymbolId>,
    associated_name: &SymbolId,
) -> TraversalResult<()> {
    let Some(node) = graph.get(facade_module).cloned() else {
        return Ok(());
    };
    let facts = db.get(module_facade_facts_query(db, &node.path)?)?;
    add_trait_provider_modules_matching(
        db,
        graph,
        facade_module,
        &facts,
        |db, path| {
            provider_candidate_has_public_extension_method_for_facade(
                db,
                path,
                &facts,
                target_type_name,
                associated_name,
            )
        },
        false,
    )
}

fn add_trait_provider_modules_matching(
    db: &QueryDb<LoaderContext>,
    graph: &mut ModuleGraph,
    facade_module: nia_imports::ModuleId,
    facts: &crate::facade_facts::ModuleFacadeFacts,
    mut matches_provider: impl FnMut(&QueryDb<LoaderContext>, SourcePath) -> TraversalResult<bool>,
    prefer_selected_branches: bool,
) -> TraversalResult<()> {
    add_reexport_provider_modules_matching(
        db,
        graph,
        facade_module,
        facts.provider_source_paths(),
        &mut matches_provider,
        prefer_selected_branches,
    )
}

fn add_reexport_provider_modules_matching(
    db: &QueryDb<LoaderContext>,
    graph: &mut ModuleGraph,
    facade_module: nia_imports::ModuleId,
    source_paths: &[UsedModulePath],
    mut matches_provider: impl FnMut(&QueryDb<LoaderContext>, SourcePath) -> TraversalResult<bool>,
    prefer_selected_branches: bool,
) -> TraversalResult<()> {
    let selected_paths = prefer_selected_branches.then(|| {
        let selected = source_paths
            .iter()
            .filter(|path| provider_branch_is_semantic_selected(db, graph, facade_module, path))
            .cloned()
            .collect::<Vec<_>>();
        selected
    });
    let source_paths = if prefer_selected_branches {
        if let Some(paths) = selected_paths.as_deref().filter(|paths| !paths.is_empty()) {
            paths
        } else if graph
            .get(facade_module)
            .is_some_and(|node| node.semantic_selected)
        {
            // A selected facade with no processed provider branch is only a
            // visibility scaffold. Wait for a target-aware demand instead of
            // speculatively walking every sibling provider.
            &[]
        } else {
            source_paths
        }
    } else {
        source_paths
    };
    let mut visited = HashSet::new();
    add_reexport_provider_modules_matching_inner(
        db,
        graph,
        facade_module,
        source_paths,
        &mut matches_provider,
        &mut visited,
    )?;
    Ok(())
}

fn provider_branch_is_semantic_selected(
    db: &QueryDb<LoaderContext>,
    graph: &ModuleGraph,
    facade_module: nia_imports::ModuleId,
    path: &UsedModulePath,
) -> bool {
    let Some(candidate_path) = resolve_used_module_path_source(db, graph, facade_module, path)
    else {
        return false;
    };
    let Some(candidate) = graph.module_id_for_source_identity(&candidate_path.identity()) else {
        return false;
    };
    let mut branch_root = candidate;
    while graph
        .get(branch_root)
        .and_then(|module| module.parent)
        .is_some_and(|parent| parent != facade_module)
    {
        let Some(parent) = graph.get(branch_root).and_then(|module| module.parent) else {
            break;
        };
        branch_root = parent;
    }
    graph
        .get(branch_root)
        .is_some_and(|module| module.semantic_selected && module.process_used_paths)
}

fn add_reexport_provider_modules_matching_inner(
    db: &QueryDb<LoaderContext>,
    graph: &mut ModuleGraph,
    facade_module: nia_imports::ModuleId,
    source_paths: &[UsedModulePath],
    matches_provider: &mut impl FnMut(&QueryDb<LoaderContext>, SourcePath) -> TraversalResult<bool>,
    visited: &mut HashSet<SourcePath>,
) -> TraversalResult<bool> {
    let mut any_match = false;
    for source_path in source_paths {
        let Some(candidate_path) =
            resolve_used_module_path_source(db, graph, facade_module, source_path)
        else {
            continue;
        };
        if visited.contains(&candidate_path) {
            continue;
        }
        let direct_match = matches_provider(db, candidate_path.clone())?;
        let nested_paths = if direct_match {
            Vec::new()
        } else {
            db.get(module_facade_facts_query(db, &candidate_path)?)?
                .provider_source_paths()
                .to_vec()
        };
        if !direct_match && nested_paths.is_empty() {
            continue;
        }
        let mut branch_graph = graph.clone();
        let mut branch_visited = visited.clone();
        branch_visited.insert(candidate_path);
        let Some(start) = used_path_start(&branch_graph, facade_module, source_path) else {
            continue;
        };
        let Some(provider_module) = add_visible_declared_module_path(
            db,
            &mut branch_graph,
            facade_module,
            start,
            source_path.segments(),
            UsedModulePathProcessing::Never,
        )?
        else {
            continue;
        };
        let branch_matches = if direct_match {
            mark_process_used_paths_and_process(db, &mut branch_graph, provider_module)?;
            true
        } else {
            add_reexport_provider_modules_matching_inner(
                db,
                &mut branch_graph,
                provider_module,
                &nested_paths,
                matches_provider,
                &mut branch_visited,
            )?
        };
        if branch_matches {
            branch_graph.mark_semantic_selected(facade_module);
            *graph = branch_graph;
            *visited = branch_visited;
            any_match = true;
        }
    }
    Ok(any_match)
}

fn resolve_used_module_path_source(
    db: &QueryDb<LoaderContext>,
    graph: &ModuleGraph,
    current_module: nia_imports::ModuleId,
    path: &UsedModulePath,
) -> Option<SourcePath> {
    let start = used_path_start(graph, current_module, path)?;
    let start_node = graph.get(start)?;
    resolve_child_source_path(
        graph,
        start_node.path.clone(),
        start_node.module_path.clone(),
        path.segments(),
    )
    .or_else(|| {
        let existing = add_existing_module_path_source(graph, start, path.segments())?;
        Some(existing)
    })
    .or_else(|| {
        let _ = db;
        None
    })
}

fn resolve_child_source_path(
    graph: &ModuleGraph,
    start_path: SourcePath,
    start_module_path: nia_imports::ModulePath,
    segments: &[SymbolId],
) -> Option<SourcePath> {
    let mut path = start_path;
    let mut module_path = start_module_path;
    for segment in segments {
        path = if let Some(existing) = graph
            .module_id_for_module_path(&module_path.child(*segment))
            .and_then(|module_id| graph.get(module_id))
            .map(|node| node.path.clone())
        {
            existing
        } else {
            graph.declared_child_source_path_for(&path, &module_path, *segment)
        };
        module_path = module_path.child(*segment);
    }
    Some(path)
}

fn add_existing_module_path_source(
    graph: &ModuleGraph,
    start: nia_imports::ModuleId,
    segments: &[SymbolId],
) -> Option<SourcePath> {
    let mut current = start;
    for segment in segments {
        current = graph.get(current)?.children.get(segment).copied()?;
    }
    graph.get(current).map(|node| node.path.clone())
}

fn provider_candidate_has_trait_impl(
    db: &QueryDb<LoaderContext>,
    path: SourcePath,
    target_type_name: Option<&SymbolId>,
    trait_name: &SymbolId,
    associated_name: Option<&SymbolId>,
) -> TraversalResult<bool> {
    let summary = db.get(provider_summary_query(db, &path)?)?;
    Ok(summary.defines_trait_impl(target_type_name, trait_name, associated_name))
}

fn provider_candidate_has_public_extension_method_for_facade(
    db: &QueryDb<LoaderContext>,
    path: SourcePath,
    facade_facts: &crate::facade_facts::ModuleFacadeFacts,
    target_type_name: Option<&SymbolId>,
    associated_name: &SymbolId,
) -> TraversalResult<bool> {
    let summary = db.get(provider_summary_query(db, &path)?)?;
    Ok(summary.defines_public_extension_method_for_facade(
        |trait_name| facade_facts.public_type_exposes_name(trait_name),
        target_type_name,
        associated_name,
    ))
}

pub(crate) fn module_defines_extensions(
    db: &QueryDb<LoaderContext>,
    graph: &ModuleGraph,
    module_id: nia_imports::ModuleId,
) -> TraversalResult<bool> {
    let Some(node) = graph.get(module_id).cloned() else {
        return Ok(false);
    };
    Ok(db
        .get(provider_summary_query(db, &node.path)?)?
        .has_providers())
}
