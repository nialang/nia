#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub(crate) struct ModuleGraphQuery;

use crate::provider_loading::{
    add_public_reexport_extension_provider_modules, add_public_reexport_source_module,
    module_defines_extensions, process_provider_request, process_reexport_provider_request,
};
use crate::queries::module_declarations_query;
use crate::used_paths::{UsedModulePath, UsedModulePathProcessing};
use crate::{EntryRuntime, LoaderContext, default_std_module_path};
use nia_diagnostic::Diagnostic;
use nia_imports::{
    ModuleGraph, ModuleNode, ResolvedModuleDeclaration, module_declaration_visibility_allows,
};
use nia_query::{QueryDb, QueryKey};
use nia_span::Span;

impl QueryKey<LoaderContext> for ModuleGraphQuery {
    type Value = ModuleGraph;

    fn name() -> &'static str {
        "module_graph"
    }

    fn execute(&self, db: &QueryDb<LoaderContext>) -> Self::Value {
        let mut graph = ModuleGraph::new(db.context().entry_path.clone());
        inject_entry_runtime(db, &mut graph);
        let mut index = 0;
        while index < graph.modules().count() {
            let Some(node) = graph.get(nia_imports::ModuleId(index as u32)).cloned() else {
                break;
            };
            let declarations = db.query(module_declarations_query(db, node.path.clone()));
            for package in declarations.package_roots {
                if graph.package_root(&package).is_none()
                    && let Some(path) = db.context().module_map.get(&package)
                {
                    graph.intern_package_root(&package, path.clone());
                }
            }
            if should_eager_add_declarations(db.context(), &node)
                && let Err(diagnostic) = add_declared_module_children(db, &mut graph, node.id)
            {
                graph.push_diagnostic(node.path.clone(), diagnostic);
            }
            if should_process_used_module_paths(db.context(), &graph, &node) {
                for path in declarations.used_module_paths {
                    if let Err(diagnostic) = add_used_module_path(db, &mut graph, node.id, &path) {
                        graph.push_diagnostic(node.path.clone(), diagnostic);
                    }
                }
            }
            index += 1;
        }
        graph
    }
}

fn should_eager_add_declarations(context: &LoaderContext, node: &ModuleNode) -> bool {
    node.process_declared_children
        || (context.package_root_used_paths && node.module_path.is_package_root())
        || (node.module_path.package == nia_imports::STD_MODULE_MAP_NAME
            && node
                .module_path
                .segments
                .first()
                .is_some_and(|segment| segment == "start"))
}

fn should_process_used_module_paths(
    context: &LoaderContext,
    graph: &ModuleGraph,
    node: &ModuleNode,
) -> bool {
    node.process_used_paths
        && (!node.module_path.is_package_root()
            || context.package_root_used_paths
            || node.module_path.package != nia_imports::STD_MODULE_MAP_NAME
            || graph.package_facade_active(nia_imports::STD_MODULE_MAP_NAME))
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
    package: &str,
) -> Result<(), Diagnostic> {
    if graph.package_facade_active(package) {
        return Ok(());
    }
    let Some(root) = graph.mark_package_facade_active(package) else {
        return Ok(());
    };
    let Some(node) = graph.get(root).cloned() else {
        return Ok(());
    };
    let declarations = db.query(module_declarations_query(db, node.path));
    for package in declarations.package_roots {
        if graph.package_root(&package).is_none()
            && let Some(path) = db.context().module_map.get(&package)
        {
            graph.intern_package_root(&package, path.clone());
        }
    }
    for path in declarations.used_module_paths {
        add_used_module_path(db, graph, root, &path)?;
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
    let declarations = db.query(module_declarations_query(db, node.path));
    for package in declarations.package_roots {
        if graph.package_root(&package).is_none()
            && let Some(path) = db.context().module_map.get(&package)
        {
            graph.intern_package_root(&package, path.clone());
        }
    }
    for path in declarations.used_module_paths {
        add_used_module_path(db, graph, module_id, &path)?;
    }
    Ok(())
}

pub(crate) fn add_visible_declared_module_path(
    db: &QueryDb<LoaderContext>,
    graph: &mut ModuleGraph,
    accessing_module: nia_imports::ModuleId,
    start: nia_imports::ModuleId,
    segments: &[String],
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
                if processing == UsedModulePathProcessing::IfSelectedItem
                    && !is_terminal
                    && let Some(associated_name) = segments.get(index + 1)
                    && let Some(parent_facade) = graph.get(current).and_then(|node| node.parent)
                {
                    add_public_reexport_extension_provider_modules(
                        db,
                        graph,
                        parent_facade,
                        segment,
                        segment,
                        associated_name,
                    )?;
                }
                if processing.should_process_module() {
                    mark_process_used_paths_and_process(db, graph, current)?;
                }
                return Ok(Some(current));
            };
            if processing == UsedModulePathProcessing::Always && !is_terminal {
                mark_process_used_paths_and_process(db, graph, current)?;
            }
            if processing == UsedModulePathProcessing::IfSelectedItem
                && !is_terminal
                && let Some(associated_name) = segments.get(index + 1)
            {
                add_public_reexport_extension_provider_modules(
                    db,
                    graph,
                    reexport_facade,
                    segment,
                    segment,
                    associated_name,
                )?;
            }
            process_reexport_provider_request(db, graph, reexport_facade, segment, &processing)?;
            current = reexport_source;
            if processing == UsedModulePathProcessing::IfSelectedItem && is_terminal {
                mark_process_used_paths_and_process(db, graph, current)?;
            }
            continue;
        };
        current = next;
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
        if is_terminal {
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
    let declarations = db.query(module_declarations_query(db, node.path));
    for declaration in declarations.declarations {
        add_declared_module_child(db, graph, module_id, declaration)?;
    }
    Ok(())
}

pub(crate) fn add_visible_declared_module_child_if_present(
    db: &QueryDb<LoaderContext>,
    graph: &mut ModuleGraph,
    accessing_module: nia_imports::ModuleId,
    module_id: nia_imports::ModuleId,
    name: &str,
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
    let declarations = db.query(module_declarations_query(db, node.path));
    let Some(declaration) = declarations.declarations.into_iter().find(|declaration| {
        declaration.name == name
            && module_declaration_visibility_allows(
                declaration.visibility,
                graph,
                module_id,
                accessing_module,
            )
    }) else {
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
            let std_root = graph
                .package_root(nia_imports::STD_MODULE_MAP_NAME)
                .or_else(|| {
                    db.context()
                        .module_map
                        .get(nia_imports::STD_MODULE_MAP_NAME)
                        .map(|path| {
                            graph
                                .intern_package_root(nia_imports::STD_MODULE_MAP_NAME, path.clone())
                        })
                });
            let Some(std_root) = std_root else { return };
            if let Err(diagnostic) = graph.intern_declared_child(
                std_root,
                "start",
                nia_imports::Visibility::PublicPkg,
                Span::default(),
            ) {
                let path = graph
                    .get(std_root)
                    .map(|node| node.path.clone())
                    .unwrap_or_else(default_std_module_path);
                graph.push_diagnostic(path, diagnostic);
            }
        }
    }
}
