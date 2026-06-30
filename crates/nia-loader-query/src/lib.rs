// SPDX-License-Identifier: GPL-3.0-or-later
use nia_ast::{
    Expr, ExprKind, Stmt, StmtKind, TypeKind, TypeRef, UsingGroupItem, UsingItem, UsingSelector,
};
use nia_ast_walk::{Visitor, walk_expr, walk_module, walk_stmt, walk_type};
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
        .with_builtin_root(builtin_module_path())
        .with_default_std(default_std_module_path())
}

fn builtin_module_path() -> SourcePath {
    SourcePath::new("<nia:builtin>")
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
    node.module_path.package == nia_imports::ENTRY_MODULE_MAP_NAME
        || !node.module_path.is_package_root()
        || (node.module_path.package == nia_imports::STD_MODULE_MAP_NAME
            && node
                .module_path
                .segments
                .first()
                .is_some_and(|segment| segment == "start"))
}

fn should_process_used_module_paths(graph: &ModuleGraph, node: &ModuleNode) -> bool {
    node.module_path.package != nia_imports::STD_MODULE_MAP_NAME
        || !node.module_path.is_package_root()
        || graph.package_facade_active(nia_imports::STD_MODULE_MAP_NAME)
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
        package,
        segments,
        include_declared_children,
        ..
    } = path
        && let Some((first, rest)) = segments.split_first()
    {
        let Some(first_module) =
            add_visible_declared_module_child_if_present(db, graph, current_module, start, first)?
        else {
            activate_package_facade(db, graph, package)?;
            return Ok(());
        };
        let Some(module_id) =
            add_visible_declared_module_path(db, graph, current_module, first_module, rest)?
        else {
            return Ok(());
        };
        if *include_declared_children {
            add_declared_module_children(db, graph, module_id)?;
        }
        return Ok(());
    }
    let Some(module_id) =
        add_visible_declared_module_path(db, graph, current_module, start, path.segments())?
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

fn add_visible_declared_module_path(
    db: &QueryDb<LoaderContext>,
    graph: &mut ModuleGraph,
    accessing_module: nia_imports::ModuleId,
    start: nia_imports::ModuleId,
    segments: &[String],
) -> Result<Option<nia_imports::ModuleId>, Diagnostic> {
    let mut current = start;
    for segment in segments {
        let Some(next) = add_visible_declared_module_child_if_present(
            db,
            graph,
            accessing_module,
            current,
            segment,
        )?
        else {
            return Ok(None);
        };
        current = next;
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

fn add_visible_declared_module_child_if_present(
    db: &QueryDb<LoaderContext>,
    graph: &mut ModuleGraph,
    accessing_module: nia_imports::ModuleId,
    module_id: nia_imports::ModuleId,
    name: &str,
) -> Result<Option<nia_imports::ModuleId>, Diagnostic> {
    if let Some(existing) = graph
        .get(module_id)
        .and_then(|node| node.children.get(name).copied())
    {
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
    add_declared_module_child(db, graph, module_id, declaration).map(Some)
}

fn add_declared_module_child(
    _db: &QueryDb<LoaderContext>,
    graph: &mut ModuleGraph,
    module_id: nia_imports::ModuleId,
    declaration: ResolvedModuleDeclaration,
) -> Result<nia_imports::ModuleId, Diagnostic> {
    if let Some(existing) = graph
        .get(module_id)
        .and_then(|node| node.children.get(&declaration.name).copied())
    {
        return Ok(existing);
    }
    graph.intern_declared_child(
        module_id,
        &declaration.name,
        declaration.visibility,
        declaration.span,
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
        let prune_result = prune_module_for_target(raw_module.clone(), &db.context().target);
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
        if self.0 == builtin_module_path() {
            return SourceText {
                file: Some(
                    db.context()
                        .sources
                        .set_source(self.0.clone(), builtin_module_source(&db.context().target)),
                ),
                diagnostic: None,
            };
        }
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

fn builtin_module_source(target: &TargetConfig) -> String {
    let fields = [
        ("arch", target.arch.as_str()),
        ("vendor", target.vendor.as_str()),
        ("os", target.os.as_str()),
        ("env", target.env.as_str()),
        ("abi", target.abi.as_str()),
        ("endian", target.endian.as_str()),
    ];
    let mut source = String::new();
    for (name, value) in fields {
        source.push_str(&format!(
            "pub comptime {name}: [{}]char = {};\n",
            value.chars().count(),
            nia_string_literal(value)
        ));
    }
    source.push_str(&format!(
        "pub comptime pointer_width: usize = {}usize;\n",
        target.pointer_width
    ));
    source
}

fn nia_string_literal(value: &str) -> String {
    let mut literal = String::from("\"");
    for ch in value.chars() {
        match ch {
            '"' => literal.push_str("\\\""),
            '\\' => literal.push_str("\\\\"),
            '\n' => literal.push_str("\\n"),
            '\r' => literal.push_str("\\r"),
            '\t' => literal.push_str("\\t"),
            '\0' => literal.push_str("\\0"),
            ch if ch.is_control() => literal.push_str(&format!("\\u{{{:x}}}", ch as u32)),
            ch => literal.push(ch),
        }
    }
    literal.push('"');
    literal
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
    for item in &item_tree.items {
        let ItemTreeNodeKind::Using(using) = &item.kind else {
            continue;
        };
        collect_using_modules(
            &using.host,
            &using.selector,
            module_map,
            &local_module_names,
            &mut packages,
            &mut paths,
        );
    }
    let module = item_tree.to_module();
    let mut collector = QualifiedPathModuleCollector {
        module_map,
        local_module_names: &local_module_names,
        packages: &mut packages,
        paths: &mut paths,
    };
    walk_module(&mut collector, &module);
    packages.sort();
    packages.dedup();
    paths.sort();
    paths.dedup();
    (packages, paths)
}

struct QualifiedPathModuleCollector<'a> {
    module_map: &'a ModuleMap,
    local_module_names: &'a [String],
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
            self.packages,
            self.paths,
        );
    }

    fn collect_path_segments(&mut self, segments: Vec<String>) {
        let Some((first, rest)) = segments.split_first() else {
            return;
        };
        if first == nia_imports::PACKAGE_MODULE_MAP_NAME {
            self.paths.push(UsedModulePath::PackageRelative {
                segments: rest.to_vec(),
                include_declared_children: false,
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
            });
        }
    }
}

impl<'ast> Visitor<'ast> for QualifiedPathModuleCollector<'_> {
    fn visit_stmt(&mut self, stmt: &'ast Stmt) {
        if let StmtKind::Using(using) = &stmt.kind {
            self.collect_using(using);
        }
        walk_stmt(self, stmt);
    }

    fn visit_expr(&mut self, expr: &'ast Expr) {
        if let Some(segments) = expr_qualified_segments(expr) {
            self.collect_path_segments(segments);
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
        }
        walk_type(self, ty);
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
    packages: &mut Vec<String>,
    paths: &mut Vec<UsedModulePath>,
) {
    if host.is_empty() {
        collect_root_group_modules(selector, module_map, local_module_names, packages, paths);
        return;
    }
    let Some(root) = UsedModuleRoot::from_host(host, module_map, local_module_names, packages)
    else {
        return;
    };
    collect_selector_modules(root, selector, paths);
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
                        include_declared_children: true,
                    });
                }
            }
            UsingGroupItem::Nested { host, selector } => {
                collect_using_modules(
                    host,
                    selector,
                    module_map,
                    local_module_names,
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
    },
    PackageRelative {
        segments: Vec<String>,
        include_declared_children: bool,
    },
    Local {
        segments: Vec<String>,
        include_declared_children: bool,
    },
}

impl UsedModulePath {
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

    fn activates_package_facade(&self) -> Option<&str> {
        match self {
            UsedModulePath::Package {
                package,
                segments,
                include_declared_children,
            } if segments.is_empty() && *include_declared_children => Some(package),
            _ => None,
        }
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
            return None;
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

    fn path(&self, extra: &[String], include_declared_children: bool) -> UsedModulePath {
        match self {
            UsedModuleRoot::Package { package, base } => UsedModulePath::Package {
                package: package.clone(),
                segments: joined_segments(base, extra),
                include_declared_children,
            },
            UsedModuleRoot::PackageRelative { base } => UsedModulePath::PackageRelative {
                segments: joined_segments(base, extra),
                include_declared_children,
            },
            UsedModuleRoot::Local { base } => UsedModulePath::Local {
                segments: joined_segments(base, extra),
                include_declared_children,
            },
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
            let include_children = matches!(
                &used_root,
                UsedModuleRoot::Package { base, .. } if base.is_empty()
            );
            paths.push(used_root.path(&[], include_children));
        }
        UsingSelector::Wildcard { .. } => {
            paths.push(used_root.path(&[], true));
        }
        UsingSelector::Single(name) => {
            paths.push(used_root.path(&[], false));
            paths.push(used_root.path(std::slice::from_ref(&name.name), false));
        }
        UsingSelector::Group(items) => {
            let include_children = matches!(
                &used_root,
                UsedModuleRoot::Package { base, .. } if base.is_empty()
            );
            paths.push(used_root.path(&[], include_children));
            for item in items {
                collect_group_item_modules(&used_root, item, paths);
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
            paths.push(root.path(std::slice::from_ref(&name.name), false));
        }
        UsingGroupItem::Nested { host, selector } => {
            let nested_root = root_with_extra(root, &host_segments(host));
            collect_selector_modules(nested_root, selector, paths);
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
        assert_eq!(program.modules.len(), 4);
        assert_eq!(program.graph.modules().count(), 4);
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
        assert_eq!(program.modules.len(), 2);
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
        assert_eq!(program.modules.len(), 2);
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
        for relative in [
            "lib/std/atomic.nia",
            "lib/std/build.nia",
            "lib/std/collections.nia",
            "lib/std/hash.nia",
            "lib/std/iter.nia",
            "lib/std/iter/range.nia",
            "lib/std/slice.nia",
        ] {
            assert!(
                program
                    .modules
                    .iter()
                    .any(|module| module.path.as_str().ends_with(relative)),
                "missing std facade dependency {relative}: {:?}",
                program.modules
            );
        }
    }

    #[test]
    fn query_loader_injects_builtin_module_map() {
        let root = temp_dir("query_loader_injects_builtin_module_map");
        let main_path = root.join("main.nia");
        write(&main_path, "using builtin;");

        let program = load_program(main_path.to_string_lossy().into_owned());

        assert!(program.diagnostics.is_empty(), "{:?}", program.diagnostics);
        let builtin_module = program
            .graph
            .get(
                program
                    .graph
                    .package_root(nia_imports::BUILTIN_MODULE_MAP_NAME)
                    .expect("builtin package root"),
            )
            .expect("builtin module");
        assert_eq!(builtin_module.path.as_str(), builtin_module_path().as_str());
        let builtin_loaded = program
            .modules
            .iter()
            .find(|module| module.path.as_str() == builtin_module_path().as_str())
            .expect("loaded builtin module");
        assert!(builtin_loaded.item_tree.items.iter().any(|item| {
            matches!(
                &item.kind,
                ItemTreeNodeKind::Binding(binding)
                    if binding.is_comptime && binding.name == "pointer_width"
            )
        }));
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
        assert_module_loaded(&program, "lib/std/process.nia");
        assert_module_loaded(&program, "lib/std/start/freestanding/linux/x86_64.nia");
        assert_module_not_loaded(&program, "lib/std/build/core.nia");
        assert_module_not_loaded(&program, "lib/std/atomic.nia");
        assert_module_not_loaded(&program, "lib/std/debug.nia");
    }

    #[test]
    fn query_loader_activates_std_facade_for_root_reexport_import() {
        let root = temp_dir("query_loader_activates_std_facade_for_root_reexport_import");
        let main_path = root.join("main.nia");
        write(&main_path, "using std::CStringView; fn main() void {}");

        let program = load_program(main_path.to_string_lossy().into_owned());

        assert!(program.diagnostics.is_empty(), "{:?}", program.diagnostics);
        assert!(program.graph.package_facade_active("std"));
        assert_module_loaded(&program, "lib/std/cstring.nia");
        assert_module_not_loaded(&program, "lib/std/process.nia");
    }

    #[test]
    fn query_loader_activates_std_facade_for_single_value_reexport_import() {
        let root = temp_dir("query_loader_activates_std_facade_for_single_value_reexport_import");
        let main_path = root.join("main.nia");
        write(&main_path, "using std::CStringView; fn main() void {}");

        let program = load_program(main_path.to_string_lossy().into_owned());

        assert!(program.diagnostics.is_empty(), "{:?}", program.diagnostics);
        assert!(program.graph.package_facade_active("std"));
        assert_module_loaded(&program, "lib/std/cstring.nia");
        assert_module_not_loaded(&program, "lib/std/process.nia");
    }

    #[test]
    fn query_loader_activates_std_facade_for_qualified_root_reexport() {
        let root = temp_dir("query_loader_activates_std_facade_for_qualified_root_reexport");
        let main_path = root.join("main.nia");
        write(
            &main_path,
            r#"fn main() void { if ?text = std::CStringView::from_bytes(b"nia\0") { _ = text; } or null {} }"#,
        );

        let program = load_program(main_path.to_string_lossy().into_owned());

        assert!(program.diagnostics.is_empty(), "{:?}", program.diagnostics);
        assert!(program.graph.package_facade_active("std"));
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
        assert_eq!(program.modules.len(), 3);
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
        assert_eq!(program.modules.len(), 2);
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
        assert_eq!(program.modules.len(), 2);
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
        assert_eq!(first.modules.len(), 1);

        sources.set_source(main.clone(), "module defs;");
        db.invalidate(SourceTextQuery(main));

        let second = db.query(LoadedProgramQuery);
        assert!(second.diagnostics.is_empty(), "{:?}", second.diagnostics);
        assert_eq!(second.modules.len(), 2);
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
