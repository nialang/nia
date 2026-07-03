// SPDX-License-Identifier: GPL-3.0-or-later
use nia_ast::{
    Expr, ExprKind, Item, ItemKind, Stmt, StmtKind, TypeKind, TypeRef, UsingGroupItem, UsingItem,
    UsingSelector,
};
use nia_ast_walk::{Visitor, walk_expr, walk_item, walk_module, walk_stmt, walk_type};
use nia_compiler_query::{LoadedModule, LoadedProgram, ProgramDiagnostic, RuntimeModel};
use nia_diagnostic::{Diagnostic, codes};
use nia_imports::{
    ModuleGraph, ModuleMap, ModuleNode, ResolvedModuleDeclaration,
    module_declaration_visibility_allows, resolve_module_declarations_from_active_item_tree,
};
use nia_item_tree::{ActiveModuleItemTree, ItemTreeNodeKind, ModuleItemTree};
use nia_query::{QueryDb, QueryKey};
use nia_source::{SourceDatabase, SourceFile, SourcePath, SourceVersion};
use nia_span::Span;
use nia_target_config::{TargetConfig, prune_module_for_target};
use std::collections::HashMap;
use std::path::Path;

pub fn load_program(entry_path: impl Into<String>) -> LoadedProgram {
    load_program_with_map(entry_path, ModuleMap::default())
}

pub fn load_program_with_map(
    entry_path: impl Into<String>,
    module_map: ModuleMap,
) -> LoadedProgram {
    load_program_with_map_and_entry_runtime(entry_path, module_map, EntryRuntime::None)
}

pub fn load_program_with_map_and_entry_runtime(
    entry_path: impl Into<String>,
    module_map: ModuleMap,
    entry_runtime: EntryRuntime,
) -> LoadedProgram {
    load_program_request(
        LoadRequest::new(entry_path)
            .with_module_map(module_map)
            .with_entry_runtime(entry_runtime),
    )
}

pub fn load_program_request(request: LoadRequest) -> LoadedProgram {
    LoaderDatabase::new(request).load_program()
}

#[derive(Clone)]
pub struct LoaderDatabase {
    db: QueryDb<LoaderContext>,
    sources: SourceDatabase,
}

impl LoaderDatabase {
    pub fn new(request: LoadRequest) -> Self {
        let entry_path = SourcePath::new(request.entry_path);
        let module_map = effective_module_map(&entry_path, request.module_map);
        let sources = request.sources;
        let db = QueryDb::new(LoaderContext {
            entry_path,
            module_map,
            sources: sources.clone(),
            target: request.target,
            entry_runtime: request.entry_runtime,
        });
        Self { db, sources }
    }

    pub fn load_program(&self) -> LoadedProgram {
        self.db.query(LoadedProgramQuery)
    }

    pub fn sources(&self) -> &SourceDatabase {
        &self.sources
    }

    pub fn set_source(&self, path: impl Into<String>, text: impl Into<String>) -> SourceFile {
        let path = SourcePath::new(path.into());
        let file = self.sources.set_source(path.clone(), text);
        self.db.invalidate(SourceTextQuery(path));
        file
    }

    pub fn invalidate_source(&self, path: impl Into<String>) -> nia_query::QueryInvalidation {
        self.db
            .invalidate(SourceTextQuery(SourcePath::new(path.into())))
    }

    pub fn query_trace(&self) -> nia_query::QueryTrace {
        self.db.query_trace()
    }
}

#[derive(Debug, Clone)]
pub struct LoadRequest {
    pub entry_path: String,
    pub module_map: ModuleMap,
    pub sources: SourceDatabase,
    pub target: TargetConfig,
    pub entry_runtime: EntryRuntime,
}

impl LoadRequest {
    pub fn new(entry_path: impl Into<String>) -> Self {
        Self {
            entry_path: entry_path.into(),
            module_map: ModuleMap::default(),
            sources: SourceDatabase::new(),
            target: TargetConfig::host(),
            entry_runtime: EntryRuntime::None,
        }
    }

    pub fn with_module_map(mut self, module_map: ModuleMap) -> Self {
        self.module_map = module_map;
        self
    }

    pub fn with_sources(mut self, sources: SourceDatabase) -> Self {
        self.sources = sources;
        self
    }

    pub fn with_target(mut self, target: TargetConfig) -> Self {
        self.target = target;
        self
    }

    pub fn with_entry_runtime(mut self, entry_runtime: EntryRuntime) -> Self {
        self.entry_runtime = entry_runtime;
        self
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default)]
pub enum EntryRuntime {
    #[default]
    None,
    Freestanding,
}

#[cfg(test)]
fn load_program_from_sources(
    entry_path: impl Into<String>,
    module_map: ModuleMap,
    sources: SourceDatabase,
) -> LoadedProgram {
    load_program_request(
        LoadRequest::new(entry_path)
            .with_module_map(module_map)
            .with_sources(sources),
    )
}

#[cfg(test)]
fn load_program_trace(
    entry_path: impl Into<String>,
    module_map: ModuleMap,
) -> nia_query::QueryTrace {
    let entry_path = SourcePath::new(entry_path.into());
    let module_map = effective_module_map(&entry_path, module_map);
    let db = QueryDb::new(LoaderContext {
        entry_path,
        module_map,
        sources: SourceDatabase::new(),
        target: TargetConfig::host(),
        entry_runtime: EntryRuntime::None,
    });
    let _ = db.query(LoadedProgramQuery);
    db.query_trace()
}

fn effective_module_map(entry_path: &SourcePath, module_map: ModuleMap) -> ModuleMap {
    module_map
        .with_entry(entry_path.clone())
        .with_default_std(default_std_module_path())
}

fn default_std_module_path() -> SourcePath {
    let manifest_dir = Path::new(env!("CARGO_MANIFEST_DIR"));
    let workspace_root = manifest_dir
        .parent()
        .and_then(Path::parent)
        .unwrap_or(manifest_dir);
    SourcePath::new(
        workspace_root
            .join("lib/std.nia")
            .to_string_lossy()
            .into_owned(),
    )
}

struct LoaderContext {
    entry_path: SourcePath,
    module_map: ModuleMap,
    sources: SourceDatabase,
    target: TargetConfig,
    entry_runtime: EntryRuntime,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
struct LoadedProgramQuery;

impl QueryKey<LoaderContext> for LoadedProgramQuery {
    type Value = LoadedProgram;

    fn name() -> &'static str {
        "loaded_program"
    }

    fn execute(&self, db: &QueryDb<LoaderContext>) -> Self::Value {
        let graph = db.query(ModuleGraphQuery);
        let modules = graph
            .modules()
            .map(|node| db.query(LoadedModuleQuery(node.path.clone())))
            .collect::<Vec<_>>();
        let diagnostics = db.query(LoadDiagnosticsQuery);
        LoadedProgram {
            graph,
            target: db.context().target.clone(),
            runtime: runtime_model(db.context().entry_runtime),
            modules,
            diagnostics,
        }
    }
}

fn runtime_model(entry_runtime: EntryRuntime) -> RuntimeModel {
    match entry_runtime {
        EntryRuntime::None => RuntimeModel::Bare,
        EntryRuntime::Freestanding => RuntimeModel::FreestandingExecutable,
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
struct ModuleGraphQuery;

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
            if should_eager_add_declarations(&node)
                && let Err(diagnostic) = add_declared_module_children(db, &mut graph, node.id)
            {
                graph.push_diagnostic(node.path.clone(), diagnostic);
            }
            if should_process_used_module_paths(&graph, &node) {
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

fn should_eager_add_declarations(node: &ModuleNode) -> bool {
    node.process_declared_children
        || (node.module_path.package == nia_imports::STD_MODULE_MAP_NAME
            && node
                .module_path
                .segments
                .first()
                .is_some_and(|segment| segment == "start"))
}

fn should_process_used_module_paths(graph: &ModuleGraph, node: &ModuleNode) -> bool {
    node.process_used_paths
        && (node.module_path.package != nia_imports::STD_MODULE_MAP_NAME
            || !node.module_path.is_package_root()
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
    if let UsedModulePath::Package {
        package: _,
        segments,
        include_declared_children,
        ..
    } = path
        && let Some((first, rest)) = segments.split_first()
    {
        let Some(first_module) = add_visible_declared_module_child_if_present(
            db,
            graph,
            current_module,
            start,
            first,
            if rest.is_empty() {
                path.process_used_paths()
            } else {
                false
            },
        )?
        else {
            let Some(reexport_source) = add_public_reexport_source_module(db, graph, start, first)?
            else {
                return Ok(());
            };
            let Some(module_id) = add_visible_declared_module_path(
                db,
                graph,
                current_module,
                reexport_source,
                rest,
                path.processing(),
            )?
            else {
                return Ok(());
            };
            if let Some(associated_name) = rest.first() {
                add_public_reexport_extension_provider_modules(
                    db,
                    graph,
                    start,
                    first,
                    first,
                    associated_name,
                )?;
            }
            if *include_declared_children {
                add_declared_module_children(db, graph, module_id)?;
            }
            return Ok(());
        };
        let Some(module_id) = add_visible_declared_module_path(
            db,
            graph,
            current_module,
            first_module,
            rest,
            path.processing(),
        )?
        else {
            return Ok(());
        };
        if *include_declared_children {
            add_declared_module_children(db, graph, module_id)?;
        }
        return Ok(());
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

fn used_path_start(
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

fn mark_process_used_paths_and_process(
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

fn add_visible_declared_module_path(
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
    if processing == UsedModulePathProcessing::IfProvidesExtensions
        && segments.is_empty()
        && module_defines_extensions(db, graph, current)
    {
        mark_process_used_paths_and_process(db, graph, current)?;
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

fn process_reexport_provider_request(
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
        UsedModulePathProcessing::IfProvidesTraitMethod { associated_name } => {
            add_public_reexport_trait_method_provider_modules(
                db,
                graph,
                facade_module,
                associated_name,
            )
        }
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

fn process_provider_request(
    db: &QueryDb<LoaderContext>,
    graph: &mut ModuleGraph,
    module_id: nia_imports::ModuleId,
    processing: &UsedModulePathProcessing,
) -> Result<(), Diagnostic> {
    match processing {
        UsedModulePathProcessing::IfProvidesTraitImpl { trait_name } => {
            add_public_reexport_trait_impl_provider_modules(db, graph, module_id, trait_name)
        }
        UsedModulePathProcessing::IfProvidesTraitMethod { associated_name } => {
            add_public_reexport_trait_method_provider_modules(db, graph, module_id, associated_name)
        }
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

fn add_public_reexport_source_module(
    db: &QueryDb<LoaderContext>,
    graph: &mut ModuleGraph,
    module_id: nia_imports::ModuleId,
    name: &str,
) -> Result<Option<nia_imports::ModuleId>, Diagnostic> {
    let Some(node) = graph.get(module_id).cloned() else {
        return Ok(None);
    };
    let parsed = db.query(parsed_module_query(db, node.path));
    let local_module_names = parsed
        .active_item_tree
        .items
        .iter()
        .filter_map(|item| match &item.kind {
            ItemTreeNodeKind::Module(module) => Some(module.name.clone()),
            _ => None,
        })
        .collect::<Vec<_>>();
    let aliases = module_using_aliases(
        &parsed.active_item_tree,
        &db.context().module_map,
        &local_module_names,
    );
    for item in &parsed.active_item_tree.items {
        if item.visibility != nia_imports::Visibility::Public {
            continue;
        }
        let ItemTreeNodeKind::Using(using) = &item.kind else {
            continue;
        };
        let Some(host_path) = using_host_path(
            &using.host,
            &db.context().module_map,
            &local_module_names,
            &aliases,
        ) else {
            continue;
        };
        let Some(source_path) =
            reexport_source_path_for_selector(&host_path, &using.selector, name)
        else {
            continue;
        };
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

fn add_public_reexport_extension_provider_modules(
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
    let parsed = db.query(parsed_module_query(db, node.path));
    if !public_reexport_exposes_name(&parsed.active_item_tree, exported_name) {
        return Ok(());
    }
    let local_module_names = parsed
        .active_item_tree
        .items
        .iter()
        .filter_map(|item| match &item.kind {
            ItemTreeNodeKind::Module(module) => Some(module.name.clone()),
            _ => None,
        })
        .collect::<Vec<_>>();
    let aliases = module_using_aliases(
        &parsed.active_item_tree,
        &db.context().module_map,
        &local_module_names,
    );
    for item in &parsed.active_item_tree.items {
        let ItemTreeNodeKind::Using(using) = &item.kind else {
            continue;
        };
        let Some(host_path) = using_host_path(
            &using.host,
            &db.context().module_map,
            &local_module_names,
            &aliases,
        ) else {
            continue;
        };
        for source_path in provider_source_paths_for_selector(&host_path, &using.selector) {
            let Some(candidate_path) =
                resolve_used_module_path_source(db, graph, facade_module, &source_path)
            else {
                continue;
            };
            if !source_defines_inherent_associated_item(
                db,
                candidate_path,
                target_type_name,
                associated_name,
            ) {
                continue;
            }
            let Some(start) = used_path_start(graph, facade_module, &source_path) else {
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
    }
    Ok(())
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
    let parsed = db.query(parsed_module_query(db, node.path));
    if !public_type_exposes_name(&parsed.active_item_tree, trait_name) {
        return Ok(());
    }
    add_trait_provider_modules_matching(
        db,
        graph,
        facade_module,
        &parsed.active_item_tree,
        |db, path| source_defines_trait_impl(db, path, trait_name, None),
    )
}

fn add_public_reexport_trait_method_provider_modules(
    db: &QueryDb<LoaderContext>,
    graph: &mut ModuleGraph,
    facade_module: nia_imports::ModuleId,
    associated_name: &str,
) -> Result<(), Diagnostic> {
    let Some(node) = graph.get(facade_module).cloned() else {
        return Ok(());
    };
    let parsed = db.query(parsed_module_query(db, node.path));
    add_trait_provider_modules_matching(
        db,
        graph,
        facade_module,
        &parsed.active_item_tree,
        |db, path| {
            source_defines_trait_method_for_public_type(
                db,
                path,
                &parsed.active_item_tree,
                associated_name,
            )
        },
    )
}

fn add_trait_provider_modules_matching(
    db: &QueryDb<LoaderContext>,
    graph: &mut ModuleGraph,
    facade_module: nia_imports::ModuleId,
    item_tree: &ActiveModuleItemTree,
    mut matches_provider: impl FnMut(&QueryDb<LoaderContext>, SourcePath) -> bool,
) -> Result<(), Diagnostic> {
    let local_module_names = item_tree
        .items
        .iter()
        .filter_map(|item| match &item.kind {
            ItemTreeNodeKind::Module(module) => Some(module.name.clone()),
            _ => None,
        })
        .collect::<Vec<_>>();
    let aliases = module_using_aliases(item_tree, &db.context().module_map, &local_module_names);
    for item in &item_tree.items {
        let ItemTreeNodeKind::Using(using) = &item.kind else {
            continue;
        };
        let Some(host_path) = using_host_path(
            &using.host,
            &db.context().module_map,
            &local_module_names,
            &aliases,
        ) else {
            continue;
        };
        for source_path in provider_source_paths_for_selector(&host_path, &using.selector) {
            let Some(candidate_path) =
                resolve_used_module_path_source(db, graph, facade_module, &source_path)
            else {
                continue;
            };
            if !matches_provider(db, candidate_path) {
                continue;
            }
            let Some(start) = used_path_start(graph, facade_module, &source_path) else {
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

fn public_reexport_exposes_name(item_tree: &ActiveModuleItemTree, name: &str) -> bool {
    item_tree.items.iter().any(|item| {
        item.visibility == nia_imports::Visibility::Public
            && matches!(
                &item.kind,
                ItemTreeNodeKind::Using(using)
                    if selector_exposes_name(&using.selector, name)
            )
    })
}

fn public_type_exposes_name(item_tree: &ActiveModuleItemTree, name: &str) -> bool {
    item_tree.items.iter().any(|item| {
        if item.visibility != nia_imports::Visibility::Public {
            return false;
        }
        match &item.kind {
            ItemTreeNodeKind::Struct(item) => item.name == name,
            ItemTreeNodeKind::Union(item) => item.name == name,
            ItemTreeNodeKind::Trait(item) => item.name == name,
            ItemTreeNodeKind::Enum(item) => item.name == name,
            ItemTreeNodeKind::TypeAlias(item) => item.name == name,
            ItemTreeNodeKind::Using(using) => selector_exposes_name(&using.selector, name),
            _ => false,
        }
    })
}

fn selector_exposes_name(selector: &UsingSelector, name: &str) -> bool {
    match selector {
        UsingSelector::SelfName => false,
        UsingSelector::Wildcard { .. } => true,
        UsingSelector::Single(using_name) => using_name_exposes_name(using_name, name),
        UsingSelector::Group(items) => items.iter().any(|item| match item {
            UsingGroupItem::Name(using_name) => using_name_exposes_name(using_name, name),
            UsingGroupItem::Nested { selector, .. } => selector_exposes_name(selector, name),
        }),
    }
}

fn provider_source_paths_for_selector(
    host_path: &UsedModulePath,
    selector: &UsingSelector,
) -> Vec<UsedModulePath> {
    let mut paths = Vec::new();
    match selector {
        UsingSelector::SelfName | UsingSelector::Wildcard { .. } => {
            paths.push(host_path.with_declared_children_and_processing(false, false));
        }
        UsingSelector::Single(name) => {
            paths.push(host_path.with_appended_segments_with_processing(
                std::slice::from_ref(&name.name),
                false,
                false,
            ));
        }
        UsingSelector::Group(items) => {
            for item in items {
                provider_source_paths_for_group_item(host_path, item, &mut paths);
            }
        }
    }
    paths
}

fn provider_source_paths_for_group_item(
    host_path: &UsedModulePath,
    item: &UsingGroupItem,
    paths: &mut Vec<UsedModulePath>,
) {
    match item {
        UsingGroupItem::Name(name) => {
            paths.push(host_path.with_appended_segments_with_processing(
                std::slice::from_ref(&name.name),
                false,
                false,
            ));
        }
        UsingGroupItem::Nested { host, selector } => {
            let nested = host_path.with_appended_segments_with_processing(
                &host_segments(host),
                false,
                false,
            );
            paths.extend(provider_source_paths_for_selector(&nested, selector));
        }
    }
}

fn source_defines_inherent_associated_item(
    db: &QueryDb<LoaderContext>,
    path: SourcePath,
    target_type_name: &str,
    associated_name: &str,
) -> bool {
    let parsed = db.query(parsed_module_query(db, path));
    parsed.active_item_tree.items.iter().any(|item| {
        let ItemTreeNodeKind::Extend(extend) = &item.kind else {
            return false;
        };
        if extend.trait_ref.is_some() || !type_ref_ends_with_name(&extend.target, target_type_name)
        {
            return false;
        }
        extend
            .methods
            .iter()
            .any(|method| method.function.name == associated_name)
            || extend
                .associated_values
                .iter()
                .any(|value| value.binding.name == associated_name)
    })
}

fn source_defines_trait_impl(
    db: &QueryDb<LoaderContext>,
    path: SourcePath,
    trait_name: &str,
    associated_name: Option<&str>,
) -> bool {
    let parsed = db.query(parsed_module_query(db, path));
    parsed.active_item_tree.items.iter().any(|item| {
        let ItemTreeNodeKind::Extend(extend) = &item.kind else {
            return false;
        };
        let Some(trait_ref) = &extend.trait_ref else {
            return false;
        };
        if !type_ref_ends_with_name(trait_ref, trait_name) {
            return false;
        }
        associated_name.is_none_or(|associated_name| {
            extend
                .methods
                .iter()
                .any(|method| method.function.name == associated_name)
                || extend
                    .associated_values
                    .iter()
                    .any(|value| value.binding.name == associated_name)
        })
    })
}

fn source_defines_trait_method_for_public_type(
    db: &QueryDb<LoaderContext>,
    path: SourcePath,
    facade_item_tree: &ActiveModuleItemTree,
    associated_name: &str,
) -> bool {
    let parsed = db.query(parsed_module_query(db, path));
    parsed.active_item_tree.items.iter().any(|item| {
        let ItemTreeNodeKind::Extend(extend) = &item.kind else {
            return false;
        };
        let Some(trait_ref) = &extend.trait_ref else {
            return false;
        };
        let Some(trait_name) = type_ref_last_name(trait_ref) else {
            return false;
        };
        public_type_exposes_name(facade_item_tree, trait_name)
            && (extend
                .methods
                .iter()
                .any(|method| method.function.name == associated_name)
                || extend
                    .associated_values
                    .iter()
                    .any(|value| value.binding.name == associated_name))
    })
}

fn module_defines_extensions(
    db: &QueryDb<LoaderContext>,
    graph: &ModuleGraph,
    module_id: nia_imports::ModuleId,
) -> bool {
    let Some(node) = graph.get(module_id).cloned() else {
        return false;
    };
    let parsed = db.query(parsed_module_query(db, node.path));
    parsed
        .active_item_tree
        .items
        .iter()
        .any(|item| matches!(item.kind, ItemTreeNodeKind::Extend(_)))
}

fn type_ref_ends_with_name(ty: &TypeRef, name: &str) -> bool {
    type_ref_last_name(ty).is_some_and(|last| last == name)
}

fn type_ref_last_name(ty: &TypeRef) -> Option<&str> {
    match &ty.kind {
        TypeKind::Path { segments } => segments.last().map(|segment| segment.name.as_str()),
        _ => None,
    }
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

fn add_visible_declared_module_child_if_present(
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

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
struct LoadDiagnosticsQuery;

impl QueryKey<LoaderContext> for LoadDiagnosticsQuery {
    type Value = Vec<ProgramDiagnostic>;

    fn name() -> &'static str {
        "load_diagnostics"
    }

    fn execute(&self, db: &QueryDb<LoaderContext>) -> Self::Value {
        let graph = db.query(ModuleGraphQuery);
        let mut diagnostics = Vec::new();
        for (path, diagnostic) in graph.diagnostics() {
            diagnostics.push(ProgramDiagnostic {
                path: path.clone(),
                diagnostic: diagnostic.clone(),
            });
        }
        for node in graph.modules() {
            let parsed = db.query(parsed_module_query(db, node.path.clone()));
            diagnostics.extend(module_diagnostics(
                &node.path,
                &parsed
                    .parse_errors
                    .iter()
                    .map(|error| {
                        Diagnostic::user_error_at(codes::PARSE, error.span, error.message.clone())
                    })
                    .collect::<Vec<_>>(),
            ));
            diagnostics.extend(module_diagnostics(&node.path, &parsed.prune_diagnostics));
            diagnostics.extend(module_diagnostics(
                &node.path,
                &db.query(module_declarations_query(db, node.path.clone()))
                    .diagnostics,
            ));
        }
        diagnostics
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
struct LoadedModuleQuery(SourcePath);

impl QueryKey<LoaderContext> for LoadedModuleQuery {
    type Value = LoadedModule;

    fn name() -> &'static str {
        "loaded_module"
    }

    fn description(&self) -> String {
        format!("loaded_module({})", self.0.as_str())
    }

    fn execute(&self, db: &QueryDb<LoaderContext>) -> Self::Value {
        let graph = db.query(ModuleGraphQuery);
        let id = graph
            .module_id_for_path(self.0.as_str())
            .unwrap_or_else(|| {
                db.invalid_input(self, format!("missing module id for `{}`", self.0.as_str()))
            });
        let parsed = db.query(parsed_module_query(db, self.0.clone()));
        LoadedModule {
            id,
            path: self.0.clone(),
            source_identity: self.0.identity(),
            source_version: parsed.source.version(),
            item_tree: parsed.item_tree,
            active_item_tree: parsed.active_item_tree,
            origins: parsed.origins,
            parse_errors: parsed.parse_errors,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
struct ParsedModuleQuery {
    path: SourcePath,
    version: SourceVersion,
}

impl QueryKey<LoaderContext> for ParsedModuleQuery {
    type Value = ParsedModule;

    fn name() -> &'static str {
        "parsed_module"
    }

    fn description(&self) -> String {
        format!("parsed_module({})@{:?}", self.path.as_str(), self.version)
    }

    fn execute(&self, db: &QueryDb<LoaderContext>) -> Self::Value {
        let source = db.query(SourceTextQuery(self.path.clone()));
        let syntax = db.query(SyntaxModuleQuery {
            path: self.path.clone(),
            version: self.version,
        });
        let (raw_module, parse_errors, origins) =
            nia_parser::parse_module_syntax_with_origins(&syntax);
        let item_tree = ModuleItemTree::from_module(&raw_module);
        let prune_result = prune_module_for_target(raw_module, &db.context().target);
        ParsedModule {
            source: source
                .file
                .unwrap_or_else(|| db.context().sources.empty_source(&self.path)),
            item_tree,
            active_item_tree: prune_result.active_item_tree,
            origins,
            parse_errors,
            prune_diagnostics: prune_result.diagnostics,
            read_diagnostic: source.diagnostic,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
struct SyntaxModuleQuery {
    path: SourcePath,
    version: SourceVersion,
}

impl QueryKey<LoaderContext> for SyntaxModuleQuery {
    type Value = nia_syntax::SyntaxTree;

    fn name() -> &'static str {
        "syntax_module"
    }

    fn description(&self) -> String {
        format!("syntax_module({})@{:?}", self.path.as_str(), self.version)
    }

    fn execute(&self, db: &QueryDb<LoaderContext>) -> Self::Value {
        let source = db.query(SourceTextQuery(self.path.clone()));
        source
            .file
            .as_ref()
            .filter(|file| file.version() == self.version)
            .map(|file| nia_syntax::parse_source(&file.text, Some(file.version())))
            .unwrap_or_else(|| nia_syntax::parse_source("", Some(self.version)))
    }
}

#[derive(Debug, Clone, PartialEq)]
struct ParsedModule {
    source: SourceFile,
    item_tree: ModuleItemTree,
    active_item_tree: ActiveModuleItemTree,
    origins: nia_node_id::NodeOriginTable,
    parse_errors: Vec<nia_parser::ParseError>,
    prune_diagnostics: Vec<Diagnostic>,
    read_diagnostic: Option<Diagnostic>,
}

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
struct SourceTextQuery(SourcePath);

impl QueryKey<LoaderContext> for SourceTextQuery {
    type Value = SourceText;

    fn name() -> &'static str {
        "source_text"
    }

    fn description(&self) -> String {
        format!("source_text({})", self.0.as_str())
    }

    fn execute(&self, db: &QueryDb<LoaderContext>) -> Self::Value {
        match db.context().sources.read_source(&self.0) {
            Ok(file) => SourceText {
                file: Some(file),
                diagnostic: None,
            },
            Err(err) => SourceText {
                file: None,
                diagnostic: Some(
                    Diagnostic::user_error(
                        codes::LOAD,
                        format!("failed to read `{}`: {err}", self.0.as_str()),
                    )
                    .debug("path", self.0.as_str())
                    .finish(),
                ),
            },
        }
    }
}

#[derive(Debug, Clone, PartialEq)]
struct SourceText {
    file: Option<SourceFile>,
    diagnostic: Option<Diagnostic>,
}

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
struct ModuleDeclarationsQuery {
    path: SourcePath,
    version: SourceVersion,
}

impl QueryKey<LoaderContext> for ModuleDeclarationsQuery {
    type Value = ModuleDeclarations;

    fn name() -> &'static str {
        "module_declarations"
    }

    fn description(&self) -> String {
        format!(
            "module_declarations({})@{:?}",
            self.path.as_str(),
            self.version
        )
    }

    fn execute(&self, db: &QueryDb<LoaderContext>) -> Self::Value {
        let parsed = db.query(ParsedModuleQuery {
            path: self.path.clone(),
            version: self.version,
        });
        let mut diagnostics = parsed.read_diagnostic.into_iter().collect::<Vec<_>>();
        let (declarations, package_roots, used_module_paths) = if diagnostics.is_empty()
            && parsed.parse_errors.is_empty()
            && parsed.prune_diagnostics.is_empty()
        {
            let declarations = resolve_module_declarations_from_active_item_tree(
                &mut diagnostics,
                &parsed.active_item_tree,
            );
            let (package_roots, used_module_paths) =
                collect_used_modules(&parsed.active_item_tree, &db.context().module_map);
            (declarations, package_roots, used_module_paths)
        } else {
            (Vec::new(), Vec::new(), Vec::new())
        };
        ModuleDeclarations {
            declarations,
            package_roots,
            used_module_paths,
            diagnostics,
        }
    }
}

fn collect_used_modules(
    item_tree: &ActiveModuleItemTree,
    module_map: &ModuleMap,
) -> (Vec<String>, Vec<UsedModulePath>) {
    let mut packages = Vec::new();
    let mut paths = Vec::new();
    let local_module_names = item_tree
        .items
        .iter()
        .filter_map(|item| match &item.kind {
            ItemTreeNodeKind::Module(module) => Some(module.name.clone()),
            _ => None,
        })
        .collect::<Vec<_>>();
    let using_aliases = module_using_aliases(item_tree, module_map, &local_module_names);
    for item in &item_tree.items {
        let ItemTreeNodeKind::Using(using) = &item.kind else {
            continue;
        };
        collect_using_modules(
            &using.host,
            &using.selector,
            module_map,
            &local_module_names,
            &using_aliases,
            &mut packages,
            &mut paths,
        );
    }
    let module = item_tree.to_module();
    let mut collector = QualifiedPathModuleCollector {
        module_map,
        local_module_names: &local_module_names,
        using_aliases: &using_aliases,
        packages: &mut packages,
        paths: &mut paths,
    };
    walk_module(&mut collector, &module);
    packages.sort();
    packages.dedup();
    paths.sort();
    paths.dedup();
    for path in &paths {
        if let UsedModulePath::Package { package, .. } = path {
            packages.push(package.clone());
        }
    }
    packages.sort();
    packages.dedup();
    (packages, paths)
}

struct QualifiedPathModuleCollector<'a> {
    module_map: &'a ModuleMap,
    local_module_names: &'a [String],
    using_aliases: &'a HashMap<String, UsedModulePath>,
    packages: &'a mut Vec<String>,
    paths: &'a mut Vec<UsedModulePath>,
}

impl QualifiedPathModuleCollector<'_> {
    fn collect_using(&mut self, using: &UsingItem) {
        collect_using_modules(
            &using.host,
            &using.selector,
            self.module_map,
            self.local_module_names,
            self.using_aliases,
            self.packages,
            self.paths,
        );
    }

    fn collect_path_segments(&mut self, segments: Vec<String>) {
        self.collect_path_segments_with_processing(
            segments,
            UsedModulePathProcessing::IfSelectedItem,
        );
    }

    fn collect_path_segments_with_processing(
        &mut self,
        segments: Vec<String>,
        processing: UsedModulePathProcessing,
    ) {
        let Some((first, rest)) = segments.split_first() else {
            return;
        };
        if let Some(alias) = self.using_aliases.get(first) {
            self.paths
                .push(alias.with_appended_segments_with_processing_mode(rest, false, processing));
            return;
        }
        if first == nia_imports::PACKAGE_MODULE_MAP_NAME {
            self.paths.push(UsedModulePath::PackageRelative {
                segments: rest.to_vec(),
                include_declared_children: false,
                processing: if processing == UsedModulePathProcessing::IfSelectedItem {
                    UsedModulePathProcessing::Always
                } else {
                    processing
                },
            });
            return;
        }
        if first == nia_imports::ENTRY_MODULE_MAP_NAME {
            return;
        }
        if !self.local_module_names.contains(first) && self.module_map.get(first).is_some() {
            self.packages.push(first.clone());
            self.paths.push(UsedModulePath::Package {
                package: first.clone(),
                segments: rest.to_vec(),
                include_declared_children: false,
                processing: if processing == UsedModulePathProcessing::IfSelectedItem {
                    UsedModulePathProcessing::Always
                } else {
                    processing
                },
            });
        }
    }

    fn collect_trait_provider_for_type(&mut self, ty: &TypeRef) {
        let TypeKind::Path { segments } = &ty.kind else {
            return;
        };
        let Some(last) = segments.last() else {
            return;
        };
        self.collect_path_segments_with_processing(
            segments
                .iter()
                .map(|segment| segment.name.clone())
                .collect(),
            UsedModulePathProcessing::IfProvidesTraitImpl {
                trait_name: last.name.clone(),
            },
        );
    }

    fn collect_trait_method_provider(&mut self, name: &str) {
        for alias in self.using_aliases.values() {
            self.paths
                .push(alias.with_appended_segments_with_processing_mode(
                    &[],
                    false,
                    UsedModulePathProcessing::IfProvidesTraitMethod {
                        associated_name: name.to_string(),
                    },
                ));
        }
    }

    fn collect_inherent_provider_for_type(&mut self, target: &TypeRef, associated_name: &str) {
        let TypeKind::Path { segments } = &target.kind else {
            return;
        };
        let Some(last) = segments.last() else {
            return;
        };
        self.collect_path_segments_with_processing(
            segments
                .iter()
                .map(|segment| segment.name.clone())
                .collect(),
            UsedModulePathProcessing::IfProvidesInherentAssociated {
                target_type_name: last.name.clone(),
                associated_name: associated_name.to_string(),
            },
        );
    }
}

fn module_using_aliases(
    item_tree: &ActiveModuleItemTree,
    module_map: &ModuleMap,
    local_module_names: &[String],
) -> HashMap<String, UsedModulePath> {
    let mut aliases: HashMap<String, UsedModulePath> = HashMap::new();
    let mut packages = Vec::new();
    for item in &item_tree.items {
        let ItemTreeNodeKind::Using(using) = &item.kind else {
            continue;
        };
        if !using.host.is_empty()
            && let Some((first, rest)) = using.host.split_first()
            && let Some(alias) = aliases.get(&first.name).cloned()
        {
            let root =
                alias.with_appended_segments_with_processing(&host_segments(rest), false, false);
            collect_selector_aliases_from_path(root, &using.selector, &mut aliases);
            continue;
        }
        collect_using_aliases(
            &using.host,
            &using.selector,
            module_map,
            local_module_names,
            &mut packages,
            &mut aliases,
        );
    }
    aliases
}

impl<'ast> Visitor<'ast> for QualifiedPathModuleCollector<'_> {
    fn visit_item(&mut self, item: &'ast Item) {
        let ItemKind::Extend(extend) = &item.kind else {
            walk_item(self, item);
            return;
        };
        self.visit_type(&extend.target);
        if let Some(trait_ref) = &extend.trait_ref {
            self.visit_type(trait_ref);
        }
        nia_ast_walk::walk_where_clause(self, &extend.where_clause);
        for associated_type in &extend.associated_types {
            self.visit_type(&associated_type.ty);
        }
        for associated_value in &extend.associated_values {
            if let Some(ty) = &associated_value.binding.ty {
                self.visit_type(ty);
            }
            if let Some(value) = &associated_value.binding.value {
                self.visit_expr(value);
            }
        }
        for method in &extend.methods {
            self.visit_function_signature_without_body(&method.function);
            if let Some(body) = &method.function.body {
                let mut collector = ExtendSelfMethodCollector {
                    target: &extend.target,
                    module_collector: self,
                };
                collector.visit_block(body);
            }
        }
    }

    fn visit_stmt(&mut self, stmt: &'ast Stmt) {
        if let StmtKind::Using(using) = &stmt.kind {
            self.collect_using(using);
        }
        walk_stmt(self, stmt);
    }

    fn visit_expr(&mut self, expr: &'ast Expr) {
        if let ExprKind::Field { name, .. } = &expr.kind {
            self.collect_trait_method_provider(name);
        }
        if let Some(segments) = expr_qualified_segments(expr) {
            self.collect_path_segments(segments);
            return;
        }
        walk_expr(self, expr);
    }

    fn visit_type(&mut self, ty: &'ast TypeRef) {
        if let TypeKind::Path { segments } = &ty.kind {
            self.collect_path_segments(
                segments
                    .iter()
                    .map(|segment| segment.name.clone())
                    .collect::<Vec<_>>(),
            );
            for segment in segments {
                for arg in &segment.args {
                    match arg {
                        nia_ast::TypeArg::Type(ty)
                        | nia_ast::TypeArg::AssocBinding { ty, .. }
                        | nia_ast::TypeArg::TypeOrConst { ty, .. } => {
                            self.collect_trait_provider_for_type(ty);
                        }
                        nia_ast::TypeArg::Const(_) => {}
                    }
                }
            }
        }
        walk_type(self, ty);
    }
}

impl QualifiedPathModuleCollector<'_> {
    fn visit_function_signature_without_body(&mut self, function: &nia_ast::FunctionItem) {
        nia_ast_walk::walk_where_clause(self, &function.where_clause);
        for param in &function.params {
            if let Some(ty) = &param.ty {
                self.visit_type(ty);
            }
        }
        if let Some(return_type) = &function.return_type {
            self.visit_type(return_type);
        }
    }
}

struct ExtendSelfMethodCollector<'a, 'b> {
    target: &'a TypeRef,
    module_collector: &'a mut QualifiedPathModuleCollector<'b>,
}

impl<'ast> Visitor<'ast> for ExtendSelfMethodCollector<'_, '_> {
    fn visit_expr(&mut self, expr: &'ast Expr) {
        if let ExprKind::Field { lhs, name } = &expr.kind
            && matches!(&lhs.kind, ExprKind::Ident(lhs_name) if lhs_name == "self")
        {
            self.module_collector
                .collect_inherent_provider_for_type(self.target, name);
        }
        if let ExprKind::Field { name, .. } = &expr.kind {
            self.module_collector.collect_trait_method_provider(name);
        }
        if let Some(segments) = expr_qualified_segments(expr) {
            self.module_collector.collect_path_segments(segments);
            return;
        }
        walk_expr(self, expr);
    }

    fn visit_type(&mut self, ty: &'ast TypeRef) {
        self.module_collector.visit_type(ty);
    }

    fn visit_stmt(&mut self, stmt: &'ast Stmt) {
        if let StmtKind::Using(using) = &stmt.kind {
            self.module_collector.collect_using(using);
        }
        walk_stmt(self, stmt);
    }
}

fn expr_qualified_segments(expr: &Expr) -> Option<Vec<String>> {
    fn collect(expr: &Expr, segments: &mut Vec<String>) -> Option<()> {
        match &expr.kind {
            ExprKind::Ident(name) => {
                segments.push(name.clone());
                Some(())
            }
            ExprKind::Qualified { lhs, name } => {
                collect(lhs, segments)?;
                segments.push(name.clone());
                Some(())
            }
            _ => None,
        }
    }

    let mut segments = Vec::new();
    collect(expr, &mut segments)?;
    Some(segments)
}

fn collect_using_modules(
    host: &[nia_ast::UsingHostSegment],
    selector: &UsingSelector,
    module_map: &ModuleMap,
    local_module_names: &[String],
    aliases: &HashMap<String, UsedModulePath>,
    packages: &mut Vec<String>,
    paths: &mut Vec<UsedModulePath>,
) {
    if host.is_empty() {
        collect_root_group_modules(selector, module_map, local_module_names, packages, paths);
        return;
    }
    if let Some((first, rest)) = host.split_first()
        && let Some(alias) = aliases.get(&first.name)
    {
        let host_path =
            alias.with_appended_segments_with_processing(&host_segments(rest), false, false);
        collect_selector_modules_from_path(host_path, selector, paths);
        return;
    }
    let Some(root) = UsedModuleRoot::from_host(host, module_map, local_module_names, packages)
    else {
        return;
    };
    collect_selector_modules(root, selector, paths);
}

fn collect_using_aliases(
    host: &[nia_ast::UsingHostSegment],
    selector: &UsingSelector,
    module_map: &ModuleMap,
    local_module_names: &[String],
    packages: &mut Vec<String>,
    aliases: &mut HashMap<String, UsedModulePath>,
) {
    if host.is_empty() {
        return;
    }
    let Some(root) = UsedModuleRoot::from_host(host, module_map, local_module_names, packages)
    else {
        return;
    };
    collect_selector_aliases(root, selector, aliases);
}

fn collect_selector_aliases(
    used_root: UsedModuleRoot,
    selector: &UsingSelector,
    aliases: &mut HashMap<String, UsedModulePath>,
) {
    match selector {
        UsingSelector::SelfName => {
            if let Some(name) = used_root.last_segment_name() {
                insert_using_alias(aliases, name, used_root.path(&[], false, false));
            }
        }
        UsingSelector::Wildcard { .. } => {}
        UsingSelector::Single(name) => {
            insert_using_alias(
                aliases,
                name.alias.clone().unwrap_or_else(|| name.name.clone()),
                used_root.path(std::slice::from_ref(&name.name), false, false),
            );
        }
        UsingSelector::Group(items) => {
            for item in items {
                collect_group_item_aliases(&used_root, item, aliases);
            }
        }
    }
}

fn collect_selector_aliases_from_path(
    host_path: UsedModulePath,
    selector: &UsingSelector,
    aliases: &mut HashMap<String, UsedModulePath>,
) {
    match selector {
        UsingSelector::SelfName => {
            if let Some(name) = host_path.last_segment_name() {
                insert_using_alias(aliases, name.to_string(), host_path);
            }
        }
        UsingSelector::Wildcard { .. } => {}
        UsingSelector::Single(name) => {
            insert_using_alias(
                aliases,
                name.alias.clone().unwrap_or_else(|| name.name.clone()),
                host_path.with_appended_segments_with_processing(
                    std::slice::from_ref(&name.name),
                    false,
                    false,
                ),
            );
        }
        UsingSelector::Group(items) => {
            for item in items {
                collect_group_item_aliases_from_path(&host_path, item, aliases);
            }
        }
    }
}

fn collect_group_item_aliases(
    root: &UsedModuleRoot,
    item: &UsingGroupItem,
    aliases: &mut HashMap<String, UsedModulePath>,
) {
    match item {
        UsingGroupItem::Name(name) => {
            insert_using_alias(
                aliases,
                name.alias.clone().unwrap_or_else(|| name.name.clone()),
                root.path(std::slice::from_ref(&name.name), false, false),
            );
        }
        UsingGroupItem::Nested { host, selector } => {
            let nested_root = root_with_extra(root, &host_segments(host));
            collect_selector_aliases(nested_root, selector, aliases);
        }
    }
}

fn collect_group_item_aliases_from_path(
    root: &UsedModulePath,
    item: &UsingGroupItem,
    aliases: &mut HashMap<String, UsedModulePath>,
) {
    match item {
        UsingGroupItem::Name(name) => {
            insert_using_alias(
                aliases,
                name.alias.clone().unwrap_or_else(|| name.name.clone()),
                root.with_appended_segments_with_processing(
                    std::slice::from_ref(&name.name),
                    false,
                    false,
                ),
            );
        }
        UsingGroupItem::Nested { host, selector } => {
            let nested =
                root.with_appended_segments_with_processing(&host_segments(host), false, false);
            collect_selector_aliases_from_path(nested, selector, aliases);
        }
    }
}

fn insert_using_alias(
    aliases: &mut HashMap<String, UsedModulePath>,
    name: String,
    path: UsedModulePath,
) {
    aliases.entry(name).or_insert(path);
}

fn using_host_path(
    host: &[nia_ast::UsingHostSegment],
    module_map: &ModuleMap,
    local_module_names: &[String],
    aliases: &HashMap<String, UsedModulePath>,
) -> Option<UsedModulePath> {
    let first = host.first()?;
    if let Some(alias) = aliases.get(&first.name) {
        return Some(alias.with_appended_segments_with_processing_mode(
            &host_segments(&host[1..]),
            false,
            UsedModulePathProcessing::IfSelectedItem,
        ));
    }
    let mut packages = Vec::new();
    let root = UsedModuleRoot::from_host(host, module_map, local_module_names, &mut packages)?;
    Some(root.path(&[], false, true))
}

fn reexport_source_path_for_selector(
    host_path: &UsedModulePath,
    selector: &UsingSelector,
    name: &str,
) -> Option<UsedModulePath> {
    match selector {
        UsingSelector::SelfName => host_path
            .last_segment_name()
            .filter(|last| *last == name)
            .map(|_| host_path.clone()),
        UsingSelector::Wildcard { .. } => Some(host_path.clone()),
        UsingSelector::Single(using_name) => {
            using_name_exposes_name(using_name, name).then(|| host_path.clone())
        }
        UsingSelector::Group(items) => {
            for item in items {
                if let Some(path) = reexport_source_path_for_group_item(host_path, item, name) {
                    return Some(path);
                }
            }
            None
        }
    }
}

fn reexport_source_path_for_group_item(
    host_path: &UsedModulePath,
    item: &UsingGroupItem,
    name: &str,
) -> Option<UsedModulePath> {
    match item {
        UsingGroupItem::Name(using_name) => {
            using_name_exposes_name(using_name, name).then(|| host_path.clone())
        }
        UsingGroupItem::Nested { host, selector } => {
            let nested = host_path.with_appended_segments(&host_segments(host), false);
            reexport_source_path_for_selector(&nested, selector, name)
        }
    }
}

fn using_name_exposes_name(using_name: &nia_ast::UsingName, name: &str) -> bool {
    using_name.alias.as_deref().unwrap_or(&using_name.name) == name
}

fn collect_root_group_modules(
    selector: &UsingSelector,
    module_map: &ModuleMap,
    local_module_names: &[String],
    packages: &mut Vec<String>,
    paths: &mut Vec<UsedModulePath>,
) {
    let UsingSelector::Group(items) = selector else {
        return;
    };
    for item in items {
        match item {
            UsingGroupItem::Name(name) => {
                if name.name != nia_imports::ENTRY_MODULE_MAP_NAME
                    && name.name != nia_imports::PACKAGE_MODULE_MAP_NAME
                    && !local_module_names.contains(&name.name)
                    && module_map.get(&name.name).is_some()
                {
                    packages.push(name.name.clone());
                    paths.push(UsedModulePath::Package {
                        package: name.name.clone(),
                        segments: Vec::new(),
                        include_declared_children: false,
                        processing: UsedModulePathProcessing::Never,
                    });
                }
            }
            UsingGroupItem::Nested { host, selector } => {
                collect_using_modules(
                    host,
                    selector,
                    module_map,
                    local_module_names,
                    &HashMap::new(),
                    packages,
                    paths,
                );
            }
        }
    }
}

#[derive(Debug, Clone, PartialEq)]
struct ModuleDeclarations {
    declarations: Vec<ResolvedModuleDeclaration>,
    package_roots: Vec<String>,
    used_module_paths: Vec<UsedModulePath>,
    diagnostics: Vec<Diagnostic>,
}

#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash)]
enum UsedModulePath {
    Package {
        package: String,
        segments: Vec<String>,
        include_declared_children: bool,
        processing: UsedModulePathProcessing,
    },
    PackageRelative {
        segments: Vec<String>,
        include_declared_children: bool,
        processing: UsedModulePathProcessing,
    },
    Local {
        segments: Vec<String>,
        include_declared_children: bool,
        processing: UsedModulePathProcessing,
    },
}

impl UsedModulePath {
    fn with_appended_segments(&self, extra: &[String], include_declared_children: bool) -> Self {
        self.with_appended_segments_with_processing(extra, include_declared_children, true)
    }

    fn with_appended_segments_with_processing(
        &self,
        extra: &[String],
        include_declared_children: bool,
        process_used_paths: bool,
    ) -> Self {
        self.with_appended_segments_with_processing_mode(
            extra,
            include_declared_children,
            UsedModulePathProcessing::from_bool(process_used_paths),
        )
    }

    fn with_appended_segments_with_processing_mode(
        &self,
        extra: &[String],
        include_declared_children: bool,
        processing: UsedModulePathProcessing,
    ) -> Self {
        match self {
            UsedModulePath::Package {
                package, segments, ..
            } => UsedModulePath::Package {
                package: package.clone(),
                segments: joined_segments(segments, extra),
                include_declared_children,
                processing,
            },
            UsedModulePath::PackageRelative { segments, .. } => UsedModulePath::PackageRelative {
                segments: joined_segments(segments, extra),
                include_declared_children,
                processing,
            },
            UsedModulePath::Local { segments, .. } => UsedModulePath::Local {
                segments: joined_segments(segments, extra),
                include_declared_children,
                processing,
            },
        }
    }

    fn with_declared_children_and_processing(
        &self,
        include_declared_children: bool,
        process_used_paths: bool,
    ) -> Self {
        self.with_appended_segments_with_processing(
            &[],
            include_declared_children,
            process_used_paths,
        )
    }

    fn segments(&self) -> &[String] {
        match self {
            UsedModulePath::Package { segments, .. }
            | UsedModulePath::PackageRelative { segments, .. }
            | UsedModulePath::Local { segments, .. } => segments,
        }
    }

    fn include_declared_children(&self) -> bool {
        match self {
            UsedModulePath::Package {
                include_declared_children,
                ..
            }
            | UsedModulePath::PackageRelative {
                include_declared_children,
                ..
            }
            | UsedModulePath::Local {
                include_declared_children,
                ..
            } => *include_declared_children,
        }
    }

    fn process_used_paths(&self) -> bool {
        self.processing() == UsedModulePathProcessing::Always
    }

    fn processing(&self) -> UsedModulePathProcessing {
        match self {
            UsedModulePath::Package { processing, .. }
            | UsedModulePath::PackageRelative { processing, .. }
            | UsedModulePath::Local { processing, .. } => processing.clone(),
        }
    }

    fn last_segment_name(&self) -> Option<&str> {
        match self {
            UsedModulePath::Package {
                package, segments, ..
            } => segments
                .last()
                .map_or(Some(package.as_str()), |segment| Some(segment.as_str())),
            UsedModulePath::PackageRelative { segments, .. }
            | UsedModulePath::Local { segments, .. } => segments.last().map(String::as_str),
        }
    }

    fn activates_package_facade(&self) -> Option<&str> {
        match self {
            UsedModulePath::Package {
                package,
                segments,
                include_declared_children,
                ..
            } if segments.is_empty() && *include_declared_children => Some(package),
            _ => None,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash)]
enum UsedModulePathProcessing {
    Never,
    Always,
    IfSelectedItem,
    IfProvidesExtensions,
    IfProvidesTraitImpl {
        trait_name: String,
    },
    IfProvidesTraitMethod {
        associated_name: String,
    },
    IfProvidesInherentAssociated {
        target_type_name: String,
        associated_name: String,
    },
}

impl UsedModulePathProcessing {
    fn from_bool(process_used_paths: bool) -> Self {
        if process_used_paths {
            Self::Always
        } else {
            Self::Never
        }
    }

    fn should_process_module(self) -> bool {
        matches!(self, Self::Always | Self::IfSelectedItem)
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
enum UsedModuleRoot {
    Package { package: String, base: Vec<String> },
    PackageRelative { base: Vec<String> },
    Local { base: Vec<String> },
}

impl UsedModuleRoot {
    fn from_host(
        host: &[nia_ast::UsingHostSegment],
        module_map: &ModuleMap,
        local_module_names: &[String],
        packages: &mut Vec<String>,
    ) -> Option<Self> {
        let first = host.first()?;
        if first.name == nia_imports::ENTRY_MODULE_MAP_NAME {
            return Some(Self::Package {
                package: nia_imports::ENTRY_MODULE_MAP_NAME.to_string(),
                base: host_segments(&host[1..]),
            });
        }
        if first.name == nia_imports::PACKAGE_MODULE_MAP_NAME {
            return Some(Self::PackageRelative {
                base: host_segments(&host[1..]),
            });
        }
        if local_module_names.contains(&first.name) {
            return Some(Self::Local {
                base: host_segments(host),
            });
        }
        if module_map.get(&first.name).is_some() {
            packages.push(first.name.clone());
            return Some(Self::Package {
                package: first.name.clone(),
                base: host_segments(&host[1..]),
            });
        }
        Some(Self::Local {
            base: host_segments(host),
        })
    }

    fn path(
        &self,
        extra: &[String],
        include_declared_children: bool,
        process_used_paths: bool,
    ) -> UsedModulePath {
        self.path_with_processing_mode(
            extra,
            include_declared_children,
            UsedModulePathProcessing::from_bool(process_used_paths),
        )
    }

    fn path_with_processing_mode(
        &self,
        extra: &[String],
        include_declared_children: bool,
        processing: UsedModulePathProcessing,
    ) -> UsedModulePath {
        match self {
            UsedModuleRoot::Package { package, base } => UsedModulePath::Package {
                package: package.clone(),
                segments: joined_segments(base, extra),
                include_declared_children,
                processing,
            },
            UsedModuleRoot::PackageRelative { base } => UsedModulePath::PackageRelative {
                segments: joined_segments(base, extra),
                include_declared_children,
                processing,
            },
            UsedModuleRoot::Local { base } => UsedModulePath::Local {
                segments: joined_segments(base, extra),
                include_declared_children,
                processing,
            },
        }
    }

    fn last_segment_name(&self) -> Option<String> {
        match self {
            UsedModuleRoot::Package { package, base } => {
                Some(base.last().cloned().unwrap_or_else(|| package.clone()))
            }
            UsedModuleRoot::PackageRelative { base } | UsedModuleRoot::Local { base } => {
                base.last().cloned()
            }
        }
    }
}

fn collect_selector_modules(
    used_root: UsedModuleRoot,
    selector: &UsingSelector,
    paths: &mut Vec<UsedModulePath>,
) {
    match selector {
        UsingSelector::SelfName => {
            paths.push(used_root.path_with_processing_mode(
                &[],
                false,
                UsedModulePathProcessing::IfProvidesExtensions,
            ));
        }
        UsingSelector::Wildcard { .. } => {
            paths.push(used_root.path(&[], true, true));
        }
        UsingSelector::Single(name) => {
            paths.push(used_root.path_with_processing_mode(
                std::slice::from_ref(&name.name),
                false,
                UsedModulePathProcessing::IfSelectedItem,
            ));
        }
        UsingSelector::Group(items) => {
            for item in items {
                collect_group_item_modules(&used_root, item, paths);
            }
        }
    }
}

fn collect_selector_modules_from_path(
    host_path: UsedModulePath,
    selector: &UsingSelector,
    paths: &mut Vec<UsedModulePath>,
) {
    match selector {
        UsingSelector::SelfName => {
            paths.push(host_path.with_appended_segments_with_processing_mode(
                &[],
                false,
                UsedModulePathProcessing::IfProvidesExtensions,
            ));
        }
        UsingSelector::Wildcard { .. } => {
            paths.push(host_path.with_declared_children_and_processing(true, true));
        }
        UsingSelector::Single(name) => {
            paths.push(host_path.with_appended_segments_with_processing_mode(
                std::slice::from_ref(&name.name),
                false,
                UsedModulePathProcessing::IfSelectedItem,
            ));
        }
        UsingSelector::Group(items) => {
            for item in items {
                collect_group_item_modules_from_path(&host_path, item, paths);
            }
        }
    }
}

fn collect_group_item_modules(
    root: &UsedModuleRoot,
    item: &UsingGroupItem,
    paths: &mut Vec<UsedModulePath>,
) {
    match item {
        UsingGroupItem::Name(name) => {
            paths.push(root.path_with_processing_mode(
                std::slice::from_ref(&name.name),
                false,
                UsedModulePathProcessing::IfSelectedItem,
            ));
        }
        UsingGroupItem::Nested { host, selector } => {
            let nested_root = root_with_extra(root, &host_segments(host));
            collect_selector_modules(nested_root, selector, paths);
        }
    }
}

fn collect_group_item_modules_from_path(
    root: &UsedModulePath,
    item: &UsingGroupItem,
    paths: &mut Vec<UsedModulePath>,
) {
    match item {
        UsingGroupItem::Name(name) => {
            paths.push(root.with_appended_segments_with_processing_mode(
                std::slice::from_ref(&name.name),
                false,
                UsedModulePathProcessing::IfSelectedItem,
            ));
        }
        UsingGroupItem::Nested { host, selector } => {
            let nested =
                root.with_appended_segments_with_processing(&host_segments(host), false, false);
            collect_selector_modules_from_path(nested, selector, paths);
        }
    }
}

fn root_with_extra(root: &UsedModuleRoot, extra: &[String]) -> UsedModuleRoot {
    match root {
        UsedModuleRoot::Package { package, base } => UsedModuleRoot::Package {
            package: package.clone(),
            base: joined_segments(base, extra),
        },
        UsedModuleRoot::PackageRelative { base } => UsedModuleRoot::PackageRelative {
            base: joined_segments(base, extra),
        },
        UsedModuleRoot::Local { base } => UsedModuleRoot::Local {
            base: joined_segments(base, extra),
        },
    }
}

fn host_segments(host: &[nia_ast::UsingHostSegment]) -> Vec<String> {
    host.iter().map(|segment| segment.name.clone()).collect()
}

fn joined_segments(base: &[String], extra: &[String]) -> Vec<String> {
    let mut segments = Vec::with_capacity(base.len() + extra.len());
    segments.extend_from_slice(base);
    segments.extend_from_slice(extra);
    segments
}

fn parsed_module_query(db: &QueryDb<LoaderContext>, path: SourcePath) -> ParsedModuleQuery {
    let source = db.query(SourceTextQuery(path.clone()));
    let version = source
        .file
        .as_ref()
        .map(SourceFile::version)
        .unwrap_or_else(|| db.context().sources.empty_source(&path).version());
    ParsedModuleQuery { path, version }
}

fn module_declarations_query(
    db: &QueryDb<LoaderContext>,
    path: SourcePath,
) -> ModuleDeclarationsQuery {
    let source = db.query(SourceTextQuery(path.clone()));
    let version = source
        .file
        .as_ref()
        .map(SourceFile::version)
        .unwrap_or_else(|| db.context().sources.empty_source(&path).version());
    ModuleDeclarationsQuery { path, version }
}

fn module_diagnostics(path: &SourcePath, diagnostics: &[Diagnostic]) -> Vec<ProgramDiagnostic> {
    diagnostics
        .iter()
        .cloned()
        .map(|diagnostic| ProgramDiagnostic {
            path: path.clone(),
            diagnostic,
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::{
        fs,
        path::{Path, PathBuf},
        sync::atomic::{AtomicUsize, Ordering},
    };

    static TEMP_DIR_COUNTER: AtomicUsize = AtomicUsize::new(0);

    #[test]
    fn query_loader_loads_declared_modules_once() {
        let root = temp_dir("query_loader_loads_declared_modules_once");
        write(&root.join("main.nia"), "module a; module b;");
        write(&root.join("a.nia"), "module b;");
        fs::create_dir_all(root.join("a")).expect("create child dir");
        write(&root.join("a/b.nia"), "");
        write(&root.join("b.nia"), "pub fn value() i32 { 1 }");

        let program = load_program(root.join("main.nia").to_string_lossy().into_owned());

        assert!(program.diagnostics.is_empty(), "{:?}", program.diagnostics);
        assert_eq!(program.modules.len(), program.graph.modules().count());
        assert_module_loaded(&program, root.join("main.nia").to_string_lossy().as_ref());
        assert_module_loaded(&program, root.join("a.nia").to_string_lossy().as_ref());
        assert_module_loaded(&program, root.join("a/b.nia").to_string_lossy().as_ref());
        assert_module_loaded(&program, root.join("b.nia").to_string_lossy().as_ref());
    }

    #[test]
    fn query_loader_reports_missing_source() {
        let root = temp_dir("query_loader_reports_missing_source");
        write(&root.join("main.nia"), "module missing;");

        let program = load_program(root.join("main.nia").to_string_lossy().into_owned());

        assert!(
            program
                .diagnostics
                .iter()
                .any(|diagnostic| { diagnostic.diagnostic.summary.contains("failed to read") })
        );
    }

    #[test]
    fn conditional_attribute_prunes_unselected_modules_before_graph_loading() {
        let root = temp_dir("conditional_attribute_prunes_unselected_modules_before_graph_loading");
        write(
            &root.join("main.nia"),
            r#"
@[if false]
module missing;
@[if true]
module present;
"#,
        );
        write(&root.join("present.nia"), "pub fn value() i32 { 1 }");

        let program = load_program(root.join("main.nia").to_string_lossy().into_owned());

        assert!(program.diagnostics.is_empty(), "{:?}", program.diagnostics);
        assert_module_loaded(&program, root.join("main.nia").to_string_lossy().as_ref());
        assert_module_loaded(
            &program,
            root.join("present.nia").to_string_lossy().as_ref(),
        );
        let root_module = program
            .graph
            .get(program.graph.entry())
            .expect("entry module");
        assert!(root_module.children.contains_key("present"));
        assert!(!root_module.children.contains_key("missing"));
    }

    #[test]
    fn conditional_attribute_uses_target_fields_for_module_pruning() {
        let root = temp_dir("conditional_attribute_uses_target_fields_for_module_pruning");
        write(
            &root.join("main.nia"),
            r#"
@[if os == "definitely-not-the-host-os"]
module missing;
@[if os != "definitely-not-the-host-os"]
module present;
"#,
        );
        write(&root.join("present.nia"), "pub fn value() i32 { 1 }");

        let program = load_program(root.join("main.nia").to_string_lossy().into_owned());

        assert!(program.diagnostics.is_empty(), "{:?}", program.diagnostics);
        assert_module_loaded(&program, root.join("main.nia").to_string_lossy().as_ref());
        assert_module_loaded(
            &program,
            root.join("present.nia").to_string_lossy().as_ref(),
        );
        let root_module = program
            .graph
            .get(program.graph.entry())
            .expect("entry module");
        assert!(root_module.children.contains_key("present"));
        assert!(!root_module.children.contains_key("missing"));
    }

    #[test]
    fn query_loader_uses_package_module_map() {
        let root = temp_dir("query_loader_uses_package_module_map");
        write(&root.join("main.nia"), "using std::io;");
        write(&root.join("std.nia"), "");
        fs::create_dir_all(root.join("std")).expect("create std dir");
        write(&root.join("std.nia"), "pub module io;");
        write(&root.join("std/io.nia"), "pub fn value() i32 { 1 }");
        let mut module_map = ModuleMap::new();
        module_map.insert(
            "std",
            SourcePath::new(root.join("std.nia").to_string_lossy()),
        );

        let program = load_program_with_map(
            root.join("main.nia").to_string_lossy().into_owned(),
            module_map,
        );

        assert!(program.diagnostics.is_empty(), "{:?}", program.diagnostics);
        assert_eq!(program.runtime, RuntimeModel::Bare);
        assert!(program.graph.package_root("std").is_some());
        assert!(program.modules.iter().any(
            |module| module.path.as_str() == root.join("std/io.nia").to_string_lossy().as_ref()
        ));
    }

    #[test]
    fn query_loader_injects_default_std_module_map_to_toolchain_lib() {
        let root = temp_dir("query_loader_injects_default_std_module_map_to_toolchain_lib");
        let main_path = root.join("main.nia");
        write(&main_path, "using std;");

        let program = load_program(main_path.to_string_lossy().into_owned());

        assert!(program.diagnostics.is_empty(), "{:?}", program.diagnostics);
        let std_module = program
            .graph
            .get(program.graph.package_root("std").expect("std package root"))
            .expect("std module");
        assert_eq!(std_module.path.as_str(), default_std_module_path().as_str());
        assert!(!program.graph.package_facade_active("std"));
        assert_module_not_loaded(&program, "lib/std/build.nia");
        assert_module_not_loaded(&program, "lib/std/process.nia");
    }

    #[test]
    fn query_loader_loads_std_builtin_target_module() {
        let root = temp_dir("query_loader_loads_std_builtin_target_module");
        let main_path = root.join("main.nia");
        write(&main_path, "using std::builtin::target;");

        let program = load_program(main_path.to_string_lossy().into_owned());

        assert!(program.diagnostics.is_empty(), "{:?}", program.diagnostics);
        assert!(program.graph.package_root("builtin").is_none());
        let target_loaded = program
            .modules
            .iter()
            .find(|module| module.path.as_str().ends_with("lib/std/builtin/target.nia"))
            .expect("loaded std::builtin::target module");
        assert!(target_loaded.item_tree.items.iter().any(|item| {
            matches!(
                &item.kind,
                ItemTreeNodeKind::Binding(binding)
                    if binding.is_comptime && binding.name == "pointer_width"
            )
        }));
    }

    #[test]
    fn query_loader_loads_facade_reexport_sources_by_used_name() {
        let root = temp_dir("query_loader_loads_facade_reexport_sources_by_used_name");
        let main_path = root.join("main.nia");
        write(
            &main_path,
            r#"
using std::collections;

fn main(values: collections::ArrayList[i32]) void {
    _ = values;
}
"#,
        );

        let program = load_program(main_path.to_string_lossy().into_owned());

        assert!(program.diagnostics.is_empty(), "{:?}", program.diagnostics);
        assert_module_loaded(&program, "lib/std/collections.nia");
        let collections_node = program
            .graph
            .modules()
            .find(|module| module.path.as_str().ends_with("lib/std/collections.nia"))
            .expect("std collections node");
        assert!(
            !collections_node.process_used_paths,
            "collections facade should stay shallow: {collections_node:?}"
        );
        assert_module_loaded(&program, "lib/std/collections/array_list.nia");
        assert_module_loaded(&program, "lib/std/collections/array_list/list.nia");
        assert_module_not_loaded(&program, "lib/std/collections/hash_map.nia");
        assert_module_not_loaded(&program, "lib/std/collections/hash_map/map.nia");
    }

    #[test]
    fn query_loader_keeps_facade_used_paths_shallow_for_reexported_value() {
        let root = temp_dir("query_loader_keeps_facade_used_paths_shallow_for_reexported_value");
        let main_path = root.join("main.nia");
        write(
            &main_path,
            r#"
using std::builtin::size;

comptime word_size: usize = size[usize]();
"#,
        );

        let program = load_program(main_path.to_string_lossy().into_owned());

        assert!(program.diagnostics.is_empty(), "{:?}", program.diagnostics);
        let builtin = module_by_suffix(&program, "lib/std/builtin.nia");
        let layout = module_by_suffix(&program, "lib/std/builtin/layout.nia");
        assert!(!builtin.process_used_paths);
        assert!(layout.process_used_paths);
        assert_module_not_loaded(&program, "lib/std/builtin/atomic.nia");
        assert_module_not_loaded(&program, "lib/std/builtin/ops.nia");
    }

    #[test]
    fn query_loader_loads_reexported_std_type_module_dependencies() {
        let root = temp_dir("query_loader_loads_reexported_std_type_module_dependencies");
        let main_path = root.join("main.nia");
        write(
            &main_path,
            r#"
using std::CStringView;

fn main() void {
    _ = CStringView::from_ptr(&0u8);
}
"#,
        );

        let program = load_program(main_path.to_string_lossy().into_owned());

        assert!(program.diagnostics.is_empty(), "{:?}", program.diagnostics);
        let cstring = module_by_suffix(&program, "lib/std/cstring.nia");
        let std_root = program.graph.package_root("std").expect("std package root");
        let std_root = program.graph.get(std_root).expect("std root module");
        let fmt = module_by_suffix(&program, "lib/std/fmt.nia");
        let fmt_core = module_by_suffix(&program, "lib/std/fmt/core.nia");
        assert!(cstring.process_used_paths);
        assert!(
            std_root
                .declarations
                .iter()
                .any(|declaration| declaration.name == "fmt"),
            "std root should record the fmt module declaration: {std_root:?}"
        );
        assert!(
            !fmt.process_used_paths,
            "fmt facade should stay shallow while selected exports load their source modules: {fmt:?}"
        );
        assert!(fmt_core.process_used_paths);
        assert_module_loaded(&program, "lib/std/cstring.nia");
        assert_module_loaded(&program, "lib/std/fmt.nia");
        assert_module_loaded(&program, "lib/std/fmt/core.nia");
        assert_module_not_loaded(&program, "lib/std/build.nia");
        assert_module_not_loaded(&program, "lib/std/process.nia");
    }

    #[test]
    fn query_loader_injects_freestanding_entry_runtime_through_std_start_facade() {
        let root =
            temp_dir("query_loader_injects_freestanding_entry_runtime_through_std_start_facade");
        let main_path = root.join("main.nia");
        write(
            &main_path,
            "using std::process; pub fn main(init: process::Init) process::ExitCode!void { _ = init; !{} }",
        );

        let program = load_program_with_map_and_entry_runtime(
            main_path.to_string_lossy().into_owned(),
            ModuleMap::default(),
            EntryRuntime::Freestanding,
        );

        assert!(program.diagnostics.is_empty(), "{:?}", program.diagnostics);
        assert_eq!(program.runtime, RuntimeModel::FreestandingExecutable);
        assert!(
            program
                .modules
                .iter()
                .any(|module| module.path.as_str().ends_with("lib/std/start.nia")),
            "{:?}",
            program
                .modules
                .iter()
                .map(|module| module.path.as_str())
                .collect::<Vec<_>>()
        );
        assert!(
            program.modules.iter().any(|module| module
                .path
                .as_str()
                .ends_with("lib/std/start/freestanding/linux/x86_64.nia")),
            "{:?}",
            program
                .modules
                .iter()
                .map(|module| module.path.as_str())
                .collect::<Vec<_>>()
        );
        assert!(
            program
                .graph
                .modules()
                .any(|module| module.path.as_str().ends_with("lib/std/start.nia"))
        );
        let std_root = program.graph.package_root("std").expect("std package root");
        let std = program.graph.get(std_root).expect("std entry module");
        let start_declaration = std
            .declarations
            .iter()
            .find(|declaration| declaration.name == "start")
            .expect("injected std start declaration");
        assert_eq!(
            start_declaration.visibility,
            nia_imports::Visibility::PublicPkg
        );
    }

    #[test]
    fn query_loader_loads_std_package_root_children_on_demand() {
        let root = temp_dir("query_loader_loads_std_package_root_children_on_demand");
        let main_path = root.join("main.nia");
        write(
            &main_path,
            "using std::process; pub fn main(init: process::Init) process::ExitCode!void { _ = init; !{} }",
        );

        let program = load_program_with_map_and_entry_runtime(
            main_path.to_string_lossy().into_owned(),
            ModuleMap::default(),
            EntryRuntime::Freestanding,
        );

        assert!(program.diagnostics.is_empty(), "{:?}", program.diagnostics);
        assert!(!program.graph.package_facade_active("std"));
        let process = module_by_suffix(&program, "lib/std/process.nia");
        assert!(
            !process.process_used_paths,
            "process facade should stay shallow while selected exports load their source modules: {process:?}"
        );
        assert!(
            process
                .declarations
                .iter()
                .any(|declaration| declaration.name == "types"),
            "process should record the selected types child: {process:?}"
        );
        let process_types = module_by_suffix(&program, "lib/std/process/types.nia");
        let process_init = module_by_suffix(&program, "lib/std/process/init.nia");
        let process_args = module_by_suffix(&program, "lib/std/process/args.nia");
        let process_env = module_by_suffix(&program, "lib/std/process/env.nia");
        let slice = module_by_suffix(&program, "lib/std/slice.nia");
        let iter = module_by_suffix(&program, "lib/std/iter.nia");
        assert!(process_types.process_used_paths);
        assert!(process_init.process_used_paths);
        assert!(process_args.process_used_paths);
        assert!(process_env.process_used_paths);
        assert!(slice.process_used_paths);
        assert!(iter.process_used_paths);
        assert_module_loaded(&program, "lib/std/process/init.nia");
        assert_module_loaded(&program, "lib/std/process/args.nia");
        assert_module_loaded(&program, "lib/std/process/env.nia");
        assert_module_loaded(&program, "lib/std/process/types.nia");
        assert_module_not_loaded(&program, "lib/std/process/command.nia");
        assert_module_loaded(&program, "lib/std/start/freestanding/linux/x86_64.nia");
        assert_module_not_loaded(&program, "lib/std/build/core.nia");
        assert_module_not_loaded(&program, "lib/std/atomic.nia");
        assert_module_not_loaded(&program, "lib/std/debug.nia");
    }

    #[test]
    fn query_loader_loads_facade_trait_impl_provider_for_used_trait_method() {
        let root = temp_dir("query_loader_loads_facade_trait_impl_provider_for_used_trait_method");
        let main_path = root.join("main.nia");
        write(
            &main_path,
            r#"
using std::hash;

fn main() u64 {
    let mut hasher = hash::Wyhash::init(1u64);
    42usize.hash(&mut hasher);
    hasher.finish()
}
"#,
        );

        let program = load_program(main_path.to_string_lossy().into_owned());

        assert!(program.diagnostics.is_empty(), "{:?}", program.diagnostics);
        assert_module_loaded(&program, "lib/std/hash.nia");
        assert_module_loaded(&program, "lib/std/hash/impls.nia");
        assert_module_loaded(&program, "lib/std/hash/wyhash.nia");
    }

    #[test]
    fn query_loader_loads_reexported_type_inherent_provider_chain() {
        let root = temp_dir("query_loader_loads_reexported_type_inherent_provider_chain");
        let main_path = root.join("main.nia");
        write(
            &main_path,
            r#"
using std::fs;
using std::io;
using std::os;

fn main(file: fs::File, state: &mut io::Io[Error = os::Error], buffer: &mut [u8]) fs::Error!io::FileWriter {
    file.writer(state, buffer)
}
"#,
        );

        let program = load_program(main_path.to_string_lossy().into_owned());

        assert!(program.diagnostics.is_empty(), "{:?}", program.diagnostics);
        assert_module_loaded(&program, "lib/std/io/file_adapter.nia");
        assert_module_loaded(&program, "lib/std/fs/file.nia");
        assert_module_loaded(&program, "lib/std/fs/types.nia");
    }

    #[test]
    fn query_loader_resolves_std_root_reexport_import_shallowly() {
        let root = temp_dir("query_loader_resolves_std_root_reexport_import_shallowly");
        let main_path = root.join("main.nia");
        write(&main_path, "using std::CStringView; fn main() void {}");

        let program = load_program(main_path.to_string_lossy().into_owned());

        assert!(program.diagnostics.is_empty(), "{:?}", program.diagnostics);
        assert!(!program.graph.package_facade_active("std"));
        assert_module_loaded(&program, "lib/std/cstring.nia");
        assert_module_not_loaded(&program, "lib/std/process.nia");
    }

    #[test]
    fn query_loader_resolves_std_single_value_reexport_import_shallowly() {
        let root = temp_dir("query_loader_resolves_std_single_value_reexport_import_shallowly");
        let main_path = root.join("main.nia");
        write(&main_path, "using std::CStringView; fn main() void {}");

        let program = load_program(main_path.to_string_lossy().into_owned());

        assert!(program.diagnostics.is_empty(), "{:?}", program.diagnostics);
        assert!(!program.graph.package_facade_active("std"));
        assert_module_loaded(&program, "lib/std/cstring.nia");
        assert_module_not_loaded(&program, "lib/std/process.nia");
    }

    #[test]
    fn query_loader_resolves_std_qualified_root_reexport_shallowly() {
        let root = temp_dir("query_loader_resolves_std_qualified_root_reexport_shallowly");
        let main_path = root.join("main.nia");
        write(
            &main_path,
            r#"fn main() void { if ?text = std::CStringView::from_bytes(b"nia\0") { _ = text; } or null {} }"#,
        );

        let program = load_program(main_path.to_string_lossy().into_owned());

        assert!(program.diagnostics.is_empty(), "{:?}", program.diagnostics);
        assert!(!program.graph.package_facade_active("std"));
        assert_module_loaded(&program, "lib/std/cstring.nia");
        assert_module_not_loaded(&program, "lib/std/process.nia");
    }

    #[test]
    fn query_loader_keeps_local_modules_from_activating_same_named_package() {
        let root = temp_dir("query_loader_keeps_local_modules_from_activating_same_named_package");
        let main_path = root.join("main.nia");
        write(
            &main_path,
            r#"
module std;

fn main(value: std::fmt::Value) void {
    _ = value;
}
"#,
        );
        write(&root.join("std.nia"), "pub module fmt;");
        fs::create_dir_all(root.join("std")).expect("create std dir");
        write(&root.join("std/fmt.nia"), "pub struct Value {}");

        let program = load_program(main_path.to_string_lossy().into_owned());

        assert!(program.diagnostics.is_empty(), "{:?}", program.diagnostics);
        assert!(!program.graph.package_facade_active("std"));
        assert!(program.graph.package_root("std").is_none());
        assert!(
            program
                .modules
                .iter()
                .any(|module| module.path.as_str() == root.join("std/fmt.nia").to_string_lossy())
        );
        assert_module_not_loaded(&program, "lib/std/builtin.nia");
        assert_module_not_loaded(&program, "lib/std/fmt.nia");
    }

    #[test]
    fn query_loader_resolves_root_children_relative_to_entry_file() {
        let root = temp_dir("query_loader_resolves_root_children_relative_to_entry_file");
        let main_path = root.join("main.nia");
        write(&main_path, "module defs;");
        write(&root.join("defs.nia"), "pub fn value() i32 { 1 }");

        let program = load_program(main_path.to_string_lossy().into_owned());

        assert!(program.diagnostics.is_empty(), "{:?}", program.diagnostics);
        assert_module_loaded(&program, root.join("main.nia").to_string_lossy().as_ref());
        assert_module_loaded(&program, root.join("defs.nia").to_string_lossy().as_ref());
        let root_module = program
            .graph
            .get(program.graph.entry())
            .expect("entry module");
        let defs_module = program
            .graph
            .get(root_module.children["defs"])
            .expect("defs module");
        assert_eq!(
            defs_module.path.as_str(),
            root.join("defs.nia").to_string_lossy().as_ref()
        );
    }

    #[test]
    fn query_loader_accepts_in_memory_sources() {
        let sources = SourceDatabase::new();
        sources.set_source(SourcePath::new("main.nia"), "module defs;");
        sources.set_source(SourcePath::new("defs.nia"), "pub fn value() i32 { 1 }");

        let program = load_program_from_sources("main.nia", ModuleMap::default(), sources);

        assert!(program.diagnostics.is_empty(), "{:?}", program.diagnostics);
        assert!(program.modules.iter().any(|module| {
            module.path.as_str() == "main.nia"
                && module.item_tree.items.iter().any(|item| {
                    matches!(
                        &item.kind,
                        ItemTreeNodeKind::Module(module_item) if module_item.name == "defs"
                    )
                })
        }));
        assert!(program.modules.iter().any(|module| {
            module.path.as_str() == "defs.nia"
                && module.item_tree.items.iter().any(|item| {
                    matches!(
                        &item.kind,
                        ItemTreeNodeKind::Function(function) if function.name == "value"
                    )
                })
        }));
    }

    #[test]
    fn query_trace_records_source_frontend_dependencies() {
        let root = temp_dir("query_trace_records_source_frontend_dependencies");
        let main_path = root.join("main.nia");
        write(&main_path, "fn main() i32 { 0 }");
        let main_path = main_path.to_string_lossy().into_owned();

        let trace = load_program_trace(main_path.clone(), ModuleMap::default());

        assert!(trace.dependencies.iter().any(|dependency| {
            dependency
                .from
                .description
                .starts_with(&format!("parsed_module({main_path})@"))
                && dependency
                    .to
                    .description
                    .starts_with(&format!("syntax_module({main_path})@"))
        }));
        assert!(trace.dependencies.iter().any(|dependency| {
            dependency
                .from
                .description
                .starts_with(&format!("syntax_module({main_path})@"))
                && dependency.to.description == format!("source_text({main_path})")
        }));
        assert!(trace.dependencies.iter().any(|dependency| {
            dependency
                .from
                .description
                .starts_with(&format!("module_declarations({main_path})@"))
                && dependency
                    .to
                    .description
                    .starts_with(&format!("parsed_module({main_path})@"))
        }));
    }

    #[test]
    fn invalidates_source_dependents_after_in_memory_text_change() {
        let sources = SourceDatabase::new();
        let main = SourcePath::new("main.nia");
        sources.set_source(main.clone(), "fn main() i32 { 0 }");
        let db = QueryDb::new(LoaderContext {
            entry_path: main.clone(),
            module_map: effective_module_map(&main, ModuleMap::default()),
            sources: sources.clone(),
            target: TargetConfig::host(),
            entry_runtime: EntryRuntime::None,
        });

        let first = db.query(LoadedProgramQuery);
        assert!(first.diagnostics.is_empty(), "{:?}", first.diagnostics);
        let first_module = first
            .modules
            .iter()
            .find(|module| module.path == main)
            .expect("loaded main module");
        let first_version = first_module.source_version;
        let first_item_tree = first_module.item_tree.clone();

        sources.set_source(main.clone(), "fn main() i32 { 1 }");
        let invalidation = db.invalidate(SourceTextQuery(main.clone()));
        let invalidated = invalidation
            .invalidated
            .iter()
            .map(|frame| frame.description.as_str())
            .collect::<Vec<_>>();
        assert!(
            invalidated.contains(&"source_text(main.nia)"),
            "{invalidated:?}"
        );
        assert!(
            invalidated
                .iter()
                .any(|description| description.starts_with("parsed_module(main.nia)@")),
            "{invalidated:?}"
        );
        assert!(
            invalidated.contains(&"loaded_program::LoadedProgramQuery"),
            "{invalidated:?}"
        );

        let second = db.query(LoadedProgramQuery);
        assert!(second.diagnostics.is_empty(), "{:?}", second.diagnostics);
        let second_module = second
            .modules
            .iter()
            .find(|module| module.path == main)
            .expect("reloaded main module");
        assert_ne!(second_module.source_version, first_version);
        assert_ne!(second_module.item_tree, first_item_tree);
    }

    #[test]
    fn invalidates_module_graph_after_module_declaration_text_change() {
        let sources = SourceDatabase::new();
        let main = SourcePath::new("main.nia");
        sources.set_source(main.clone(), "");
        sources.set_source(SourcePath::new("defs.nia"), "pub fn value() i32 { 1 }");
        let db = QueryDb::new(LoaderContext {
            entry_path: main.clone(),
            module_map: effective_module_map(&main, ModuleMap::default()),
            sources: sources.clone(),
            target: TargetConfig::host(),
            entry_runtime: EntryRuntime::None,
        });

        let first = db.query(LoadedProgramQuery);
        assert!(first.diagnostics.is_empty(), "{:?}", first.diagnostics);
        assert_module_loaded(&first, "main.nia");
        assert_module_not_loaded(&first, "defs.nia");

        sources.set_source(main.clone(), "module defs;");
        db.invalidate(SourceTextQuery(main));

        let second = db.query(LoadedProgramQuery);
        assert!(second.diagnostics.is_empty(), "{:?}", second.diagnostics);
        assert!(
            second
                .modules
                .iter()
                .any(|module| module.path.as_str() == "defs.nia")
        );
    }

    #[test]
    fn loaded_module_query_reports_paths_outside_module_graph() {
        let db = QueryDb::new(LoaderContext {
            entry_path: SourcePath::new("main.nia"),
            module_map: ModuleMap::default(),
            sources: SourceDatabase::new(),
            target: TargetConfig::host(),
            entry_runtime: EntryRuntime::None,
        });

        let err = db
            .try_query(LoadedModuleQuery(SourcePath::new("missing.nia")))
            .expect_err("missing module path should be an invalid query input");

        assert!(matches!(err, nia_query::QueryError::InvalidInput { .. }));
        assert!(
            err.to_string()
                .contains("missing module id for `missing.nia`"),
            "{err}"
        );
    }

    fn temp_dir(name: &str) -> PathBuf {
        let mut dir = std::env::temp_dir();
        let id = TEMP_DIR_COUNTER.fetch_add(1, Ordering::Relaxed);
        dir.push(format!(
            "nia_loader_query_{name}_{}_{:?}_{id}",
            std::process::id(),
            std::thread::current().id()
        ));
        fs::create_dir_all(&dir).expect("create temp dir");
        dir
    }

    fn write(path: &Path, source: &str) {
        fs::write(path, source).expect("write source");
    }

    fn assert_module_loaded(program: &LoadedProgram, suffix: &str) {
        assert!(
            program
                .modules
                .iter()
                .any(|module| module.path.as_str().ends_with(suffix)),
            "missing module {suffix}: {:?}",
            program
                .modules
                .iter()
                .map(|module| module.path.as_str())
                .collect::<Vec<_>>()
        );
    }

    fn module_by_suffix<'a>(program: &'a LoadedProgram, suffix: &str) -> &'a ModuleNode {
        program
            .graph
            .modules()
            .find(|module| module.path.as_str().ends_with(suffix))
            .unwrap_or_else(|| {
                panic!(
                    "missing module {suffix}: {:?}",
                    program
                        .modules
                        .iter()
                        .map(|module| module.path.as_str())
                        .collect::<Vec<_>>()
                )
            })
    }

    fn assert_module_not_loaded(program: &LoadedProgram, suffix: &str) {
        assert!(
            !program
                .modules
                .iter()
                .any(|module| module.path.as_str().ends_with(suffix)),
            "unexpected module {suffix}: {:?}",
            program
                .modules
                .iter()
                .map(|module| module.path.as_str())
                .collect::<Vec<_>>()
        );
    }
}
