use crate::LoaderContext;
use crate::graph::{
    add_visible_declared_module_child_if_present, add_visible_declared_module_path,
    mark_process_used_paths_and_process, used_path_start,
};
use crate::queries::{
    module_declarations_query, module_facade_facts_query, provider_summary_query,
};
use crate::used_paths::{UsedModulePath, UsedModulePathProcessing};
use nia_diagnostic::Diagnostic;
use nia_imports::ModuleGraph;
use nia_query::QueryDb;
use nia_source::SourcePath;

pub(crate) fn process_reexport_provider_request(
    db: &QueryDb<LoaderContext>,
    graph: &mut ModuleGraph,
    facade_module: nia_imports::ModuleId,
    exported_name: &str,
    processing: &UsedModulePathProcessing,
) -> Result<(), Diagnostic> {
    match processing {
        UsedModulePathProcessing::IfProvidesTraitImpl { trait_name }
            if trait_name == exported_name =>
        {
            add_public_reexport_trait_impl_provider_modules(db, graph, facade_module, trait_name)
        }
        UsedModulePathProcessing::IfProvidesTraitMethod {
            target_type_name,
            associated_name,
        } => add_public_reexport_trait_method_provider_modules(
            db,
            graph,
            facade_module,
            target_type_name.as_deref(),
            associated_name,
        ),
        UsedModulePathProcessing::IfProvidesInherentAssociated {
            target_type_name,
            associated_name,
        } => add_public_reexport_extension_provider_modules(
            db,
            graph,
            facade_module,
            exported_name,
            target_type_name,
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
    match processing {
        UsedModulePathProcessing::IfProvidesTraitImpl { trait_name } => {
            add_public_reexport_trait_impl_provider_modules(db, graph, module_id, trait_name)
        }
        UsedModulePathProcessing::IfProvidesTraitMethod {
            target_type_name,
            associated_name,
        } => add_public_reexport_trait_method_provider_modules(
            db,
            graph,
            module_id,
            target_type_name.as_deref(),
            associated_name,
        ),
        UsedModulePathProcessing::IfProvidesInherentAssociated {
            target_type_name,
            associated_name,
        } => add_public_reexport_extension_provider_modules(
            db,
            graph,
            module_id,
            target_type_name,
            target_type_name,
            associated_name,
        ),
        UsedModulePathProcessing::Never
        | UsedModulePathProcessing::Always
        | UsedModulePathProcessing::IfSelectedItem
        | UsedModulePathProcessing::IfProvidesExtensions => Ok(()),
    }
}

pub(crate) fn process_loaded_provider_request(
    db: &QueryDb<LoaderContext>,
    graph: &mut ModuleGraph,
    module_ids: &[nia_imports::ModuleId],
    processing: &UsedModulePathProcessing,
) -> Result<(), Diagnostic> {
    let UsedModulePathProcessing::IfProvidesTraitMethod {
        target_type_name: None,
        associated_name,
    } = processing
    else {
        return Ok(());
    };
    for module_id in module_ids.iter().copied() {
        add_loaded_unknown_method_provider_modules(db, graph, module_id, associated_name)?;
    }
    Ok(())
}

fn add_loaded_unknown_method_provider_modules(
    db: &QueryDb<LoaderContext>,
    graph: &mut ModuleGraph,
    facade_module: nia_imports::ModuleId,
    associated_name: &str,
) -> Result<(), Diagnostic> {
    let Some(node) = graph.get(facade_module).cloned() else {
        return Ok(());
    };
    let facts = db.query(module_facade_facts_query(db, node.path));
    add_reexport_provider_modules_matching(
        db,
        graph,
        facade_module,
        facts.provider_source_paths(),
        |db, path| {
            provider_candidate_has_public_extension_method_for_facade(
                db,
                path,
                &facts,
                None,
                associated_name,
            )
        },
    )
}

pub(crate) fn add_public_reexport_source_module(
    db: &QueryDb<LoaderContext>,
    graph: &mut ModuleGraph,
    module_id: nia_imports::ModuleId,
    name: &str,
) -> Result<Option<nia_imports::ModuleId>, Diagnostic> {
    let Some(node) = graph.get(module_id).cloned() else {
        return Ok(None);
    };
    let facts = db.query(module_facade_facts_query(db, node.path));
    for source_path in facts.reexport_source_paths(name) {
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

pub(crate) fn add_public_reexport_extension_provider_modules(
    db: &QueryDb<LoaderContext>,
    graph: &mut ModuleGraph,
    facade_module: nia_imports::ModuleId,
    exported_name: &str,
    target_type_name: &str,
    associated_name: &str,
) -> Result<(), Diagnostic> {
    let Some(node) = graph.get(facade_module).cloned() else {
        return Ok(());
    };
    let facts = db.query(module_facade_facts_query(db, node.path));
    if !facts.public_reexport_exposes_name(exported_name) {
        return Ok(());
    }
    add_reexport_provider_modules_matching(
        db,
        graph,
        facade_module,
        facts.provider_source_paths(),
        |db, path| {
            provider_candidate_has_inherent_associated_item(
                db,
                path,
                target_type_name,
                associated_name,
            )
        },
    )?;
    add_declared_child_extension_provider_modules(
        db,
        graph,
        facade_module,
        target_type_name,
        associated_name,
    )?;
    Ok(())
}

fn add_declared_child_extension_provider_modules(
    db: &QueryDb<LoaderContext>,
    graph: &mut ModuleGraph,
    facade_module: nia_imports::ModuleId,
    target_type_name: &str,
    associated_name: &str,
) -> Result<(), Diagnostic> {
    add_declared_child_provider_modules_matching(db, graph, facade_module, |db, path| {
        provider_candidate_has_inherent_associated_item(db, path, target_type_name, associated_name)
    })
}

fn add_public_reexport_trait_impl_provider_modules(
    db: &QueryDb<LoaderContext>,
    graph: &mut ModuleGraph,
    facade_module: nia_imports::ModuleId,
    trait_name: &str,
) -> Result<(), Diagnostic> {
    let Some(node) = graph.get(facade_module).cloned() else {
        return Ok(());
    };
    let facts = db.query(module_facade_facts_query(db, node.path));
    if !facts.public_type_exposes_name(trait_name) {
        return Ok(());
    }
    add_trait_provider_modules_matching(db, graph, facade_module, &facts, |db, path| {
        provider_candidate_has_trait_impl(db, path, trait_name, None)
    })
}

fn add_public_reexport_trait_method_provider_modules(
    db: &QueryDb<LoaderContext>,
    graph: &mut ModuleGraph,
    facade_module: nia_imports::ModuleId,
    target_type_name: Option<&str>,
    associated_name: &str,
) -> Result<(), Diagnostic> {
    let Some(node) = graph.get(facade_module).cloned() else {
        return Ok(());
    };
    let facts = db.query(module_facade_facts_query(db, node.path));
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
    )?;
    add_declared_child_provider_modules_matching(db, graph, facade_module, matches_provider)
}

fn add_reexport_provider_modules_matching(
    db: &QueryDb<LoaderContext>,
    graph: &mut ModuleGraph,
    facade_module: nia_imports::ModuleId,
    source_paths: &[UsedModulePath],
    mut matches_provider: impl FnMut(&QueryDb<LoaderContext>, SourcePath) -> bool,
) -> Result<(), Diagnostic> {
    for source_path in source_paths {
        let Some(candidate_path) =
            resolve_used_module_path_source(db, graph, facade_module, source_path)
        else {
            continue;
        };
        if !matches_provider(db, candidate_path) {
            continue;
        }
        let Some(start) = used_path_start(graph, facade_module, source_path) else {
            continue;
        };
        let Some(provider_module) = add_visible_declared_module_path(
            db,
            graph,
            facade_module,
            start,
            source_path.segments(),
            UsedModulePathProcessing::Never,
        )?
        else {
            continue;
        };
        mark_process_used_paths_and_process(db, graph, provider_module)?;
    }
    Ok(())
}

fn add_declared_child_provider_modules_matching(
    db: &QueryDb<LoaderContext>,
    graph: &mut ModuleGraph,
    facade_module: nia_imports::ModuleId,
    mut matches_provider: impl FnMut(&QueryDb<LoaderContext>, SourcePath) -> bool,
) -> Result<(), Diagnostic> {
    let Some(node) = graph.get(facade_module).cloned() else {
        return Ok(());
    };
    let declarations = db.query(module_declarations_query(db, node.path.clone()));
    for declaration in declarations.declarations {
        let child_path = nia_imports::declared_child_source_path_for(
            &node.path,
            &node.module_path,
            &declaration.name,
        );
        if !matches_provider(db, child_path.clone()) {
            continue;
        }
        let Some(provider_module) = add_visible_declared_module_child_if_present(
            db,
            graph,
            facade_module,
            facade_module,
            &declaration.name,
            false,
        )?
        else {
            continue;
        };
        mark_process_used_paths_and_process(db, graph, provider_module)?;
    }
    Ok(())
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
    segments: &[String],
) -> Option<SourcePath> {
    let mut path = start_path;
    let mut module_path = start_module_path;
    for segment in segments {
        path = if let Some(existing) = graph
            .module_id_for_module_path(&module_path.child(segment))
            .and_then(|module_id| graph.get(module_id))
            .map(|node| node.path.clone())
        {
            existing
        } else {
            nia_imports::declared_child_source_path_for(&path, &module_path, segment)
        };
        module_path = module_path.child(segment);
    }
    Some(path)
}

fn add_existing_module_path_source(
    graph: &ModuleGraph,
    start: nia_imports::ModuleId,
    segments: &[String],
) -> Option<SourcePath> {
    let mut current = start;
    for segment in segments {
        current = graph.get(current)?.children.get(segment).copied()?;
    }
    graph.get(current).map(|node| node.path.clone())
}

fn provider_candidate_has_inherent_associated_item(
    db: &QueryDb<LoaderContext>,
    path: SourcePath,
    target_type_name: &str,
    associated_name: &str,
) -> bool {
    let summary = db.query(provider_summary_query(db, path));
    summary.defines_inherent_associated_item(target_type_name, associated_name)
}

fn provider_candidate_has_trait_impl(
    db: &QueryDb<LoaderContext>,
    path: SourcePath,
    trait_name: &str,
    associated_name: Option<&str>,
) -> bool {
    let summary = db.query(provider_summary_query(db, path));
    summary.defines_trait_impl(trait_name, associated_name)
}

fn provider_candidate_has_public_extension_method_for_facade(
    db: &QueryDb<LoaderContext>,
    path: SourcePath,
    facade_facts: &crate::facade_facts::ModuleFacadeFacts,
    target_type_name: Option<&str>,
    associated_name: &str,
) -> bool {
    let summary = db.query(provider_summary_query(db, path));
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
    db.query(provider_summary_query(db, node.path))
        .has_providers()
}
