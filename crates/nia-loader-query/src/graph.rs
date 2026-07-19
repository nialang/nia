#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub(crate) struct ModuleGraphQuery;

use crate::provider_loading::{
    add_public_reexport_source_module, module_defines_extensions, process_provider_request,
    process_reexport_provider_request,
};
use crate::queries::{SourceTextQuery, module_declarations_query};
use crate::used_paths::{UsedModulePath, UsedModulePathProcessing};
use crate::{EntryRuntime, LoaderContext, default_std_module_path};
use nia_diagnostic::Diagnostic;
use nia_imports::{
    ModuleGraph, ModuleNode, ResolvedModuleDeclaration, module_declaration_visibility_allows,
};
use nia_query::{QueryDb, QueryKey};
use nia_source::{SourceIdentity, SourceVersion};
use nia_span::Span;
use nia_symbol::{SymbolId, known};

impl QueryKey<LoaderContext> for ModuleGraphQuery {
    type Value = ModuleGraph;

    fn name() -> &'static str {
        "module_graph"
    }

    fn execute(&self, db: &QueryDb<LoaderContext>) -> Self::Value {
        let provider_demands = db
            .context()
            .provider_demands
            .read()
            .expect("loader provider demand lock poisoned")
            .clone();
        let (mut seed, mut applied_provider_demands, source_versions) = {
            let state = db
                .context()
                .graph_state
                .read()
                .expect("loader graph state lock poisoned");
            (
                state.graph.clone(),
                state.applied_provider_demands.clone(),
                state.source_versions.clone(),
            )
        };
        if seed
            .as_ref()
            .is_some_and(|graph| graph_source_versions(db, graph) != source_versions)
        {
            seed = None;
            applied_provider_demands.clear();
        }
        let new_provider_demands = provider_demands
            .difference(&applied_provider_demands)
            .cloned()
            .collect::<Vec<_>>();
        let (mut graph, mut index) = match seed {
            Some(mut graph) => {
                let existing_modules = graph.modules().count();
                for demand in &new_provider_demands {
                    match &demand.request {
                        nia_compiler_query::ProviderRequest::ModuleSemantic { module_id } => {
                            graph.mark_semantic_selected(*module_id);
                        }
                        nia_compiler_query::ProviderRequest::ModuleBody { module_path } => {
                            if let Some(module_id) = graph.module_id_for_path(module_path.as_str())
                                && let Err(diagnostic) =
                                    mark_process_used_paths_and_process(db, &mut graph, module_id)
                            {
                                graph.push_diagnostic(module_path.clone(), diagnostic);
                            }
                        }
                        nia_compiler_query::ProviderRequest::Method { .. }
                        | nia_compiler_query::ProviderRequest::TraitImpl { .. } => {}
                    }
                }
                for module_index in 0..existing_modules {
                    let Some(node) = graph
                        .get(nia_imports::ModuleId(module_index as u32))
                        .cloned()
                    else {
                        continue;
                    };
                    let declarations = db.get(module_declarations_query(db, &node.path));
                    apply_provider_demands(
                        db,
                        &mut graph,
                        &node,
                        &declarations.explicit_imports,
                        &new_provider_demands,
                    );
                }
                (graph, existing_modules)
            }
            None => {
                let mut graph = ModuleGraph::with_symbol_text(
                    db.context().entry_path.clone(),
                    std::sync::Arc::new(db.context().symbols.clone()),
                );
                inject_entry_runtime(db, &mut graph);
                (graph, 0)
            }
        };
        loop {
            while index < graph.modules().count() {
                let Some(node) = graph.get(nia_imports::ModuleId(index as u32)).cloned() else {
                    break;
                };
                let declarations = db.get(module_declarations_query(db, &node.path));
                for package in &declarations.package_roots {
                    if graph.package_root(package).is_none()
                        && let Some(path) = db.context().module_map.get_name(package)
                    {
                        graph.intern_package_root(package, path.clone());
                    }
                }
                if should_eager_add_declarations(db.context(), &node) {
                    let result = add_declared_module_children(db, &mut graph, node.id);
                    if let Err(diagnostic) = result {
                        graph.push_diagnostic(node.path.clone(), diagnostic);
                    }
                }
                if should_process_used_module_paths(db.context(), &graph, &node) {
                    for path in &declarations.used_module_paths {
                        if let Err(diagnostic) = add_used_module_path(db, &mut graph, node.id, path)
                        {
                            graph.push_diagnostic(node.path.clone(), diagnostic);
                        }
                    }
                }
                apply_provider_demands(
                    db,
                    &mut graph,
                    &node,
                    &declarations.explicit_imports,
                    &new_provider_demands,
                );
                index += 1;
            }
            if index == graph.modules().count() {
                break;
            }
        }
        let mut state = db
            .context()
            .graph_state
            .write()
            .expect("loader graph state lock poisoned");
        state.graph = Some(graph.clone());
        state.applied_provider_demands = provider_demands;
        state.source_versions = graph_source_versions(db, &graph);
        graph
    }
}

fn graph_source_versions(
    db: &QueryDb<LoaderContext>,
    graph: &ModuleGraph,
) -> std::collections::HashMap<SourceIdentity, Option<SourceVersion>> {
    graph
        .modules()
        .map(|node| {
            let source_id = db.context().sources.id_for_path(&node.path);
            let source = db.get(SourceTextQuery(source_id));
            (
                node.path.identity(),
                source.file.as_ref().map(nia_source::SourceFile::version),
            )
        })
        .collect()
}

fn apply_provider_demands(
    db: &QueryDb<LoaderContext>,
    graph: &mut ModuleGraph,
    node: &ModuleNode,
    imports: &[crate::used_paths::ExplicitUsingImport],
    provider_demands: &[nia_compiler_query::ProviderDemand],
) {
    let demands = provider_demands
        .iter()
        .filter(|demand| demand.source_path.identity() == node.path.identity())
        .cloned()
        .collect::<Vec<_>>();
    for demand in demands {
        if let nia_compiler_query::ProviderRequest::ModuleSemantic { module_id } = demand.request {
            graph.mark_semantic_selected(module_id);
            continue;
        }
        for import in imports {
            let processing = match demand.request {
                nia_compiler_query::ProviderRequest::Method {
                    target_type_name,
                    method_name,
                } => UsedModulePathProcessing::IfProvidesTraitMethod {
                    target_type_name,
                    associated_name: method_name,
                },
                nia_compiler_query::ProviderRequest::TraitImpl { trait_name } => {
                    UsedModulePathProcessing::IfProvidesTraitImpl { trait_name }
                }
                nia_compiler_query::ProviderRequest::ModuleSemantic { .. } => continue,
                nia_compiler_query::ProviderRequest::ModuleBody { .. } => continue,
            };
            let path =
                import
                    .path
                    .with_appended_segments_with_processing_mode(&[], false, processing);
            if let Err(diagnostic) = add_used_module_path(db, graph, node.id, &path) {
                graph.push_diagnostic(node.path.clone(), diagnostic);
            }
        }
    }
}

fn should_eager_add_declarations(context: &LoaderContext, node: &ModuleNode) -> bool {
    node.process_declared_children
        || (node.module_path.is_package_root()
            && context
                .package_roots_with_used_paths
                .contains(&node.module_path.package))
        || node.module_path.is_std_start_module()
}

fn should_process_used_module_paths(
    context: &LoaderContext,
    graph: &ModuleGraph,
    node: &ModuleNode,
) -> bool {
    node.process_used_paths
        && (!node.module_path.is_package_root()
            || context
                .package_roots_with_used_paths
                .contains(&node.module_path.package)
            || !node.module_path.is_std_package()
            || graph.std_package_facade_active())
}

fn add_used_module_path(
    db: &QueryDb<LoaderContext>,
    graph: &mut ModuleGraph,
    current_module: nia_imports::ModuleId,
    path: &UsedModulePath,
) -> Result<(), Diagnostic> {
    let Some(start) = used_path_start(graph, current_module, path) else {
        return Ok(());
    };
    if let Some(package) = path.activates_package_facade() {
        activate_package_facade(db, graph, package)?;
    }
    let Some(module_id) = add_visible_declared_module_path(
        db,
        graph,
        current_module,
        start,
        path.segments(),
        path.processing(),
    )?
    else {
        return Ok(());
    };
    if path.include_declared_children() {
        add_declared_module_children(db, graph, module_id)?;
    }
    Ok(())
}

fn activate_package_facade(
    db: &QueryDb<LoaderContext>,
    graph: &mut ModuleGraph,
    package: SymbolId,
) -> Result<(), Diagnostic> {
    if graph.package_facade_active(&package) {
        return Ok(());
    }
    let Some(root) = graph.mark_package_facade_active(&package) else {
        return Ok(());
    };
    let Some(node) = graph.get(root).cloned() else {
        return Ok(());
    };
    let declarations = db.get(module_declarations_query(db, &node.path));
    for package in &declarations.package_roots {
        if graph.package_root(package).is_none()
            && let Some(path) = db.context().module_map.get_name(package)
        {
            graph.intern_package_root(package, path.clone());
        }
    }
    for path in &declarations.used_module_paths {
        add_used_module_path(db, graph, root, path)?;
    }
    Ok(())
}

pub(crate) fn used_path_start(
    graph: &ModuleGraph,
    current_module: nia_imports::ModuleId,
    path: &UsedModulePath,
) -> Option<nia_imports::ModuleId> {
    match path {
        UsedModulePath::Package { package, .. } => graph.package_root(package),
        UsedModulePath::PackageRelative { .. } => graph.current_package_root(current_module),
        UsedModulePath::ParentRelative { .. } => {
            graph.get(current_module).and_then(|node| node.parent)
        }
        UsedModulePath::Local { .. } => Some(current_module),
    }
}

pub(crate) fn mark_process_used_paths_and_process(
    db: &QueryDb<LoaderContext>,
    graph: &mut ModuleGraph,
    module_id: nia_imports::ModuleId,
) -> Result<(), Diagnostic> {
    if !graph.mark_process_used_paths(module_id) {
        return Ok(());
    }
    let Some(node) = graph.get(module_id).cloned() else {
        return Ok(());
    };
    let declarations = db.get(module_declarations_query(db, &node.path));
    for package in &declarations.package_roots {
        if graph.package_root(package).is_none()
            && let Some(path) = db.context().module_map.get_name(package)
        {
            graph.intern_package_root(package, path.clone());
        }
    }
    for path in &declarations.used_module_paths {
        add_used_module_path(db, graph, module_id, path)?;
    }
    Ok(())
}

pub(crate) fn add_visible_declared_module_path(
    db: &QueryDb<LoaderContext>,
    graph: &mut ModuleGraph,
    accessing_module: nia_imports::ModuleId,
    start: nia_imports::ModuleId,
    segments: &[SymbolId],
    processing: UsedModulePathProcessing,
) -> Result<Option<nia_imports::ModuleId>, Diagnostic> {
    let mut current = start;
    if processing == UsedModulePathProcessing::Always && segments.is_empty() {
        mark_process_used_paths_and_process(db, graph, current)?;
    }
    if segments.is_empty() {
        match processing {
            UsedModulePathProcessing::IfSelectedItem => {
                mark_process_used_paths_and_process(db, graph, current)?;
            }
            UsedModulePathProcessing::IfProvidesExtensions
                if module_defines_extensions(db, graph, current) =>
            {
                mark_process_used_paths_and_process(db, graph, current)?;
            }
            _ => {}
        }
    }
    if segments.is_empty() {
        process_provider_request(db, graph, current, &processing)?;
    }
    for (index, segment) in segments.iter().enumerate() {
        let is_terminal = index + 1 == segments.len();
        let process_segment_used_paths =
            processing == UsedModulePathProcessing::Always && is_terminal;
        let Some(next) = add_visible_declared_module_child_if_present(
            db,
            graph,
            accessing_module,
            current,
            segment,
            process_segment_used_paths,
        )?
        else {
            let reexport_facade = current;
            let Some(reexport_source) =
                add_public_reexport_source_module(db, graph, current, segment)?
            else {
                if processing.should_process_module() {
                    mark_process_used_paths_and_process(db, graph, current)?;
                }
                return Ok(Some(current));
            };
            if processing == UsedModulePathProcessing::Always && !is_terminal {
                mark_process_used_paths_and_process(db, graph, current)?;
            }
            process_reexport_provider_request(db, graph, reexport_facade, segment, &processing)?;
            current = reexport_source;
            if processing == UsedModulePathProcessing::IfSelectedItem && is_terminal {
                mark_process_used_paths_and_process(db, graph, current)?;
            }
            if is_terminal {
                process_provider_request(db, graph, current, &processing)?;
            }
            continue;
        };
        current = next;
        if processing.is_provider_demand() {
            process_provider_request(db, graph, current, &processing)?;
        }
        if processing == UsedModulePathProcessing::IfSelectedItem
            && is_terminal
            && module_defines_extensions(db, graph, current)
        {
            mark_process_used_paths_and_process(db, graph, current)?;
        }
        if processing == UsedModulePathProcessing::IfProvidesExtensions
            && is_terminal
            && module_defines_extensions(db, graph, current)
        {
            mark_process_used_paths_and_process(db, graph, current)?;
        }
        if is_terminal && !processing.is_provider_demand() {
            process_provider_request(db, graph, current, &processing)?;
        }
    }
    Ok(Some(current))
}

fn add_declared_module_children(
    db: &QueryDb<LoaderContext>,
    graph: &mut ModuleGraph,
    module_id: nia_imports::ModuleId,
) -> Result<(), Diagnostic> {
    let Some(node) = graph.get(module_id).cloned() else {
        return Ok(());
    };
    let declarations = db.get(module_declarations_query(db, &node.path));
    for declaration in declarations.declarations.iter().cloned() {
        add_declared_module_child(db, graph, module_id, declaration)?;
    }
    Ok(())
}

pub(crate) fn add_visible_declared_module_child_if_present(
    db: &QueryDb<LoaderContext>,
    graph: &mut ModuleGraph,
    accessing_module: nia_imports::ModuleId,
    module_id: nia_imports::ModuleId,
    name: &SymbolId,
    process_used_paths: bool,
) -> Result<Option<nia_imports::ModuleId>, Diagnostic> {
    if let Some(existing) = graph
        .get(module_id)
        .and_then(|node| node.children.get(name).copied())
    {
        if process_used_paths {
            mark_process_used_paths_and_process(db, graph, existing)?;
        }
        return Ok(Some(existing));
    }
    let Some(node) = graph.get(module_id).cloned() else {
        return Ok(None);
    };
    let declarations = db.get(module_declarations_query(db, &node.path));
    let Some(declaration) = declarations
        .declarations
        .iter()
        .find(|declaration| {
            declaration.name == *name
                && module_declaration_visibility_allows(
                    declaration.visibility,
                    graph,
                    module_id,
                    accessing_module,
                )
        })
        .cloned()
    else {
        return Ok(None);
    };
    add_declared_module_child_with_processing(
        db,
        graph,
        module_id,
        declaration,
        process_used_paths,
        false,
    )
    .map(Some)
}

fn add_declared_module_child(
    db: &QueryDb<LoaderContext>,
    graph: &mut ModuleGraph,
    module_id: nia_imports::ModuleId,
    declaration: ResolvedModuleDeclaration,
) -> Result<nia_imports::ModuleId, Diagnostic> {
    add_declared_module_child_with_processing(db, graph, module_id, declaration, true, true)
}

fn add_declared_module_child_with_processing(
    db: &QueryDb<LoaderContext>,
    graph: &mut ModuleGraph,
    module_id: nia_imports::ModuleId,
    declaration: ResolvedModuleDeclaration,
    process_used_paths: bool,
    process_declared_children: bool,
) -> Result<nia_imports::ModuleId, Diagnostic> {
    if let Some(existing) = graph
        .get(module_id)
        .and_then(|node| node.children.get(&declaration.name).copied())
    {
        if process_used_paths {
            mark_process_used_paths_and_process(db, graph, existing)?;
        }
        if process_declared_children {
            graph.mark_process_declared_children(existing);
        }
        return Ok(existing);
    }
    graph.intern_declared_child_with_processing(
        module_id,
        &declaration.name,
        declaration.visibility,
        declaration.span,
        process_used_paths,
        process_declared_children,
    )
}

fn inject_entry_runtime(db: &QueryDb<LoaderContext>, graph: &mut ModuleGraph) {
    match db.context().entry_runtime {
        EntryRuntime::None => {}
        EntryRuntime::Freestanding => {
            let std_root = graph.std_package_root().or_else(|| {
                db.context()
                    .module_map
                    .std_path()
                    .map(|path| graph.intern_std_package_root(path.clone()))
            });
            let Some(std_root) = std_root else { return };
            match graph.intern_declared_child(
                std_root,
                &known::START,
                nia_imports::Visibility::PublicPkg,
                Span::default(),
            ) {
                Ok(start_root) => graph.mark_executable_root_subtree(start_root),
                Err(diagnostic) => {
                    let path = graph
                        .get(std_root)
                        .map(|node| node.path.clone())
                        .unwrap_or_else(default_std_module_path);
                    graph.push_diagnostic(path, diagnostic);
                }
            }
        }
    }
}
