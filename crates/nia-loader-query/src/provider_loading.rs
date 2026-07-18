use crate::LoaderContext;
use crate::graph::{
    add_visible_declared_module_path, mark_process_used_paths_and_process, used_path_start,
};
use crate::queries::{module_facade_facts_query, provider_summary_query};
use crate::used_paths::{UsedModulePath, UsedModulePathProcessing};
use nia_diagnostic::Diagnostic;
use nia_imports::ModuleGraph;
use nia_query::QueryDb;
use nia_source::SourcePath;
use nia_symbol::SymbolId;
use std::collections::HashSet;

pub(crate) fn process_reexport_provider_request(
    db: &QueryDb<LoaderContext>,
    graph: &mut ModuleGraph,
    facade_module: nia_imports::ModuleId,
    exported_name: &SymbolId,
    processing: &UsedModulePathProcessing,
) -> Result<(), Diagnostic> {
    match processing {
        UsedModulePathProcessing::IfProvidesTraitImpl { trait_name }
            if trait_name == exported_name =>
        {
            add_public_reexport_trait_impl_provider_modules(db, graph, facade_module, trait_name)
        }
        UsedModulePathProcessing::IfProvidesImplicitTraitImpl { trait_name } => {
            add_implicit_trait_impl_provider_modules(db, graph, facade_module, trait_name)
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
) -> Result<(), Diagnostic> {
    if direct_provider_module_matches_request(db, graph, module_id, processing) {
        mark_process_used_paths_and_process(db, graph, module_id)?;
    }
    match processing {
        UsedModulePathProcessing::IfProvidesTraitImpl { trait_name } => {
            add_public_reexport_trait_impl_provider_modules(db, graph, module_id, trait_name)
        }
        UsedModulePathProcessing::IfProvidesImplicitTraitImpl { trait_name } => {
            add_implicit_trait_impl_provider_modules(db, graph, module_id, trait_name)
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
) -> bool {
    let Some(node) = graph.get(module_id) else {
        return false;
    };
    match processing {
        UsedModulePathProcessing::IfProvidesTraitImpl { trait_name } => {
            provider_candidate_has_trait_impl(db, node.path.clone(), trait_name, None)
        }
        UsedModulePathProcessing::IfProvidesImplicitTraitImpl { trait_name } => {
            provider_candidate_has_trait_impl(db, node.path.clone(), trait_name, None)
        }
        UsedModulePathProcessing::IfProvidesTraitMethod {
            target_type_name,
            associated_name,
        } => {
            let summary = db.get(provider_summary_query(db, node.path.clone()));
            summary.defines_public_extension_method_for_facade(
                |_| true,
                target_type_name.as_ref(),
                associated_name,
            )
        }
        UsedModulePathProcessing::Never
        | UsedModulePathProcessing::Always
        | UsedModulePathProcessing::IfSelectedItem
        | UsedModulePathProcessing::IfProvidesExtensions => false,
    }
}

pub(crate) fn add_public_reexport_source_module(
    db: &QueryDb<LoaderContext>,
    graph: &mut ModuleGraph,
    module_id: nia_imports::ModuleId,
    name: &SymbolId,
) -> Result<Option<nia_imports::ModuleId>, Diagnostic> {
    let Some(node) = graph.get(module_id).cloned() else {
        return Ok(None);
    };
    let facts = db.get(module_facade_facts_query(db, node.path));
    for source_path in facts.reexport_source_paths(name) {
        let Some(start) = used_path_start(graph, module_id, source_path) else {
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
    trait_name: &SymbolId,
) -> Result<(), Diagnostic> {
    let Some(node) = graph.get(facade_module).cloned() else {
        return Ok(());
    };
    let facts = db.get(module_facade_facts_query(db, node.path));
    add_trait_provider_modules_matching(db, graph, facade_module, &facts, |db, path| {
        provider_candidate_has_trait_impl(db, path, trait_name, None)
    })
}

fn add_implicit_trait_impl_provider_modules(
    db: &QueryDb<LoaderContext>,
    graph: &mut ModuleGraph,
    facade_module: nia_imports::ModuleId,
    trait_name: &SymbolId,
) -> Result<(), Diagnostic> {
    let Some(node) = graph.get(facade_module).cloned() else {
        return Ok(());
    };
    let facts = db.get(module_facade_facts_query(db, node.path));
    add_trait_provider_modules_matching(db, graph, facade_module, &facts, |db, path| {
        provider_candidate_has_trait_impl(db, path, trait_name, None)
    })
}

fn add_public_reexport_trait_method_provider_modules(
    db: &QueryDb<LoaderContext>,
    graph: &mut ModuleGraph,
    facade_module: nia_imports::ModuleId,
    target_type_name: Option<&SymbolId>,
    associated_name: &SymbolId,
) -> Result<(), Diagnostic> {
    let Some(node) = graph.get(facade_module).cloned() else {
        return Ok(());
    };
    let facts = db.get(module_facade_facts_query(db, node.path));
    add_trait_provider_modules_matching(db, graph, facade_module, &facts, |db, path| {
        provider_candidate_has_public_extension_method_for_facade(
            db,
            path,
            &facts,
            target_type_name,
            associated_name,
        )
    })
}

fn add_trait_provider_modules_matching(
    db: &QueryDb<LoaderContext>,
    graph: &mut ModuleGraph,
    facade_module: nia_imports::ModuleId,
    facts: &crate::facade_facts::ModuleFacadeFacts,
    mut matches_provider: impl FnMut(&QueryDb<LoaderContext>, SourcePath) -> bool,
) -> Result<(), Diagnostic> {
    add_reexport_provider_modules_matching(
        db,
        graph,
        facade_module,
        facts.provider_source_paths(),
        &mut matches_provider,
    )
}

fn add_reexport_provider_modules_matching(
    db: &QueryDb<LoaderContext>,
    graph: &mut ModuleGraph,
    facade_module: nia_imports::ModuleId,
    source_paths: &[UsedModulePath],
    mut matches_provider: impl FnMut(&QueryDb<LoaderContext>, SourcePath) -> bool,
) -> Result<(), Diagnostic> {
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

fn add_reexport_provider_modules_matching_inner(
    db: &QueryDb<LoaderContext>,
    graph: &mut ModuleGraph,
    facade_module: nia_imports::ModuleId,
    source_paths: &[UsedModulePath],
    matches_provider: &mut impl FnMut(&QueryDb<LoaderContext>, SourcePath) -> bool,
    visited: &mut HashSet<SourcePath>,
) -> Result<bool, Diagnostic> {
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
        let direct_match = matches_provider(db, candidate_path.clone());
        let nested_paths = if direct_match {
            Vec::new()
        } else {
            db.get(module_facade_facts_query(db, candidate_path.clone()))
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
    trait_name: &SymbolId,
    associated_name: Option<&SymbolId>,
) -> bool {
    let summary = db.get(provider_summary_query(db, path));
    summary.defines_trait_impl(trait_name, associated_name)
}

fn provider_candidate_has_public_extension_method_for_facade(
    db: &QueryDb<LoaderContext>,
    path: SourcePath,
    facade_facts: &crate::facade_facts::ModuleFacadeFacts,
    target_type_name: Option<&SymbolId>,
    associated_name: &SymbolId,
) -> bool {
    let summary = db.get(provider_summary_query(db, path));
    summary.defines_public_extension_method_for_facade(
        |trait_name| facade_facts.public_type_exposes_name(trait_name),
        target_type_name,
        associated_name,
    )
}

pub(crate) fn module_defines_extensions(
    db: &QueryDb<LoaderContext>,
    graph: &ModuleGraph,
    module_id: nia_imports::ModuleId,
) -> bool {
    let Some(node) = graph.get(module_id).cloned() else {
        return false;
    };
    db.get(provider_summary_query(db, node.path))
        .has_providers()
}
