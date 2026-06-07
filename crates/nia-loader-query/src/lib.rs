// SPDX-License-Identifier: GPL-3.0-or-later
use nia_ast::{ImportPath, ImportPathKind};
use nia_compiler_query::{LoadedModule, LoadedProgram, ProgramDiagnostic};
use nia_diagnostic::Diagnostic;
use nia_imports::resolve_import_path;
use nia_imports::{
    ImportAliasMap, ModuleGraph, ModuleMap, ResolvedImport, add_resolved_imports,
    collect_import_aliases, resolve_module_imports_from_active_item_tree,
};
use nia_item_tree::{ActiveModuleItemTree, ModuleItemTree};
use nia_query::{QueryDb, QueryKey};
use nia_source::{SourceDatabase, SourceFile, SourcePath, SourceVersion};
use nia_span::Span;
use nia_target_config::{TargetConfig, prune_module_for_target};
use std::path::Path;

pub fn load_program(root_path: impl Into<String>) -> LoadedProgram {
    load_program_with_map(root_path, ModuleMap::default())
}

pub fn load_program_with_map(root_path: impl Into<String>, module_map: ModuleMap) -> LoadedProgram {
    load_program_with_map_and_entry_runtime(root_path, module_map, EntryRuntime::None)
}

pub fn load_program_with_map_and_entry_runtime(
    root_path: impl Into<String>,
    module_map: ModuleMap,
    entry_runtime: EntryRuntime,
) -> LoadedProgram {
    let root_path = SourcePath::new(root_path.into());
    let module_map = effective_module_map(&root_path, module_map);
    let db = QueryDb::new(LoaderContext {
        root_path,
        module_map,
        sources: SourceDatabase::new(),
        target: TargetConfig::host(),
        entry_runtime,
    });
    db.query(LoadedProgramQuery)
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default)]
pub enum EntryRuntime {
    #[default]
    None,
    Freestanding,
}

#[cfg(test)]
fn load_program_from_sources(
    root_path: impl Into<String>,
    module_map: ModuleMap,
    sources: SourceDatabase,
) -> LoadedProgram {
    let root_path = SourcePath::new(root_path.into());
    let module_map = effective_module_map(&root_path, module_map);
    let db = QueryDb::new(LoaderContext {
        root_path,
        module_map,
        sources,
        target: TargetConfig::host(),
        entry_runtime: EntryRuntime::None,
    });
    db.query(LoadedProgramQuery)
}

#[cfg(test)]
fn load_program_trace(
    root_path: impl Into<String>,
    module_map: ModuleMap,
) -> nia_query::QueryTrace {
    let root_path = SourcePath::new(root_path.into());
    let module_map = effective_module_map(&root_path, module_map);
    let db = QueryDb::new(LoaderContext {
        root_path,
        module_map,
        sources: SourceDatabase::new(),
        target: TargetConfig::host(),
        entry_runtime: EntryRuntime::None,
    });
    let _ = db.query(LoadedProgramQuery);
    db.query_trace()
}

fn effective_module_map(root_path: &SourcePath, module_map: ModuleMap) -> ModuleMap {
    module_map
        .with_compiler_root(root_path.clone())
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
    root_path: SourcePath,
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
        let imports = db.query(ImportAliasMapQuery);
        let diagnostics = db.query(LoadDiagnosticsQuery);
        LoadedProgram {
            graph,
            imports,
            target: db.context().target.clone(),
            modules,
            diagnostics,
        }
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
        let mut graph = ModuleGraph::new(db.context().root_path.clone());
        if let Some(runtime_import) = entry_runtime_import(db.context()) {
            let root = graph.root();
            add_resolved_imports(&mut graph, root, [runtime_import]).unwrap_or_else(|diagnostic| {
                db.invalid_input(
                    self,
                    format!(
                        "failed to add entry runtime import: [{}] {}",
                        diagnostic.code, diagnostic.summary
                    ),
                )
            });
        }
        let mut index = 0;
        while index < graph.modules().count() {
            let Some(node) = graph.get(nia_imports::ModuleId(index as u32)).cloned() else {
                break;
            };
            let imports = db.query(module_imports_query(db, node.path));
            if let Err(diagnostic) = add_resolved_imports(&mut graph, node.id, imports.imports) {
                db.invalid_input(
                    self,
                    format!(
                        "failed to add resolved imports: [{}] {}",
                        diagnostic.code, diagnostic.summary
                    ),
                );
            }
            index += 1;
        }
        graph
    }
}

fn entry_runtime_import(context: &LoaderContext) -> Option<ResolvedImport> {
    match context.entry_runtime {
        EntryRuntime::None => None,
        EntryRuntime::Freestanding => {
            let import_path = ImportPath {
                kind: ImportPathKind::Root,
                segments: vec!["std".to_string(), "start".to_string()],
            };
            let path = resolve_import_path(&context.root_path, &import_path, &context.module_map)?;
            Some(ResolvedImport {
                alias: "<std.start>".to_string(),
                path,
                span: Span::default(),
            })
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
struct ImportAliasMapQuery;

impl QueryKey<LoaderContext> for ImportAliasMapQuery {
    type Value = ImportAliasMap;

    fn name() -> &'static str {
        "import_alias_map"
    }

    fn execute(&self, db: &QueryDb<LoaderContext>) -> Self::Value {
        collect_import_aliases(&db.query(ModuleGraphQuery))
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
        for node in graph.modules() {
            let parsed = db.query(parsed_module_query(db, node.path.clone()));
            diagnostics.extend(module_diagnostics(
                &node.path,
                &parsed
                    .parse_errors
                    .iter()
                    .map(|error| {
                        Diagnostic::user_error_at("E0102", error.span, error.message.clone())
                    })
                    .collect::<Vec<_>>(),
            ));
            diagnostics.extend(module_diagnostics(&node.path, &parsed.prune_diagnostics));
            diagnostics.extend(module_diagnostics(
                &node.path,
                &db.query(module_imports_query(db, node.path.clone()))
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
            source_version: parsed.source.version(),
            source: parsed.source.text,
            raw_module: parsed.raw_module,
            module: parsed.module,
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
            module: prune_result.module,
            raw_module,
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
    raw_module: nia_ast::Module,
    item_tree: ModuleItemTree,
    active_item_tree: ActiveModuleItemTree,
    module: nia_ast::Module,
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
                diagnostic: Some(Diagnostic::user_error_at(
                    "E0102",
                    Span::default(),
                    format!("failed to read `{}`: {err}", self.0.as_str()),
                )),
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
struct ModuleImportsQuery {
    path: SourcePath,
    version: SourceVersion,
}

impl QueryKey<LoaderContext> for ModuleImportsQuery {
    type Value = ModuleImports;

    fn name() -> &'static str {
        "module_imports"
    }

    fn description(&self) -> String {
        format!("module_imports({})@{:?}", self.path.as_str(), self.version)
    }

    fn execute(&self, db: &QueryDb<LoaderContext>) -> Self::Value {
        let parsed = db.query(ParsedModuleQuery {
            path: self.path.clone(),
            version: self.version,
        });
        let mut diagnostics = parsed.read_diagnostic.into_iter().collect::<Vec<_>>();
        let imports = if diagnostics.is_empty()
            && parsed.parse_errors.is_empty()
            && parsed.prune_diagnostics.is_empty()
        {
            resolve_module_imports_from_active_item_tree(
                &mut diagnostics,
                &self.path,
                &parsed.active_item_tree,
                &db.context().module_map,
            )
        } else {
            Vec::new()
        };
        ModuleImports {
            imports,
            diagnostics,
        }
    }
}

#[derive(Debug, Clone, PartialEq)]
struct ModuleImports {
    imports: Vec<ResolvedImport>,
    diagnostics: Vec<Diagnostic>,
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

fn module_imports_query(db: &QueryDb<LoaderContext>, path: SourcePath) -> ModuleImportsQuery {
    let source = db.query(SourceTextQuery(path.clone()));
    let version = source
        .file
        .as_ref()
        .map(SourceFile::version)
        .unwrap_or_else(|| db.context().sources.empty_source(&path).version());
    ModuleImportsQuery { path, version }
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
    fn query_loader_loads_import_graph_once() {
        let root = temp_dir("query_loader_loads_import_graph_once");
        write(&root.join("main.nia"), "import .a; import .b;");
        write(&root.join("a.nia"), "import .b;");
        write(&root.join("b.nia"), "pub fn value() i32 { 1 }");

        let program = load_program(root.join("main.nia").to_string_lossy().into_owned());

        assert!(program.diagnostics.is_empty(), "{:?}", program.diagnostics);
        assert_eq!(program.modules.len(), 3);
        assert_eq!(program.graph.modules().count(), 3);
    }

    #[test]
    fn query_loader_reports_missing_source() {
        let root = temp_dir("query_loader_reports_missing_source");
        write(&root.join("main.nia"), "import .missing;");

        let program = load_program(root.join("main.nia").to_string_lossy().into_owned());

        assert!(
            program
                .diagnostics
                .iter()
                .any(|diagnostic| { diagnostic.diagnostic.summary.contains("failed to read") })
        );
    }

    #[test]
    fn comptime_if_prunes_unselected_imports_before_graph_loading() {
        let root = temp_dir("comptime_if_prunes_unselected_imports_before_graph_loading");
        write(
            &root.join("main.nia"),
            r#"
comptime if false {
    import .missing;
} else {
    import .present;
}
"#,
        );
        write(&root.join("present.nia"), "pub fn value() i32 { 1 }");

        let program = load_program(root.join("main.nia").to_string_lossy().into_owned());

        assert!(program.diagnostics.is_empty(), "{:?}", program.diagnostics);
        assert_eq!(program.modules.len(), 2);
        assert!(
            program
                .imports
                .get(program.graph.root(), "present")
                .is_some()
        );
        assert!(
            program
                .imports
                .get(program.graph.root(), "missing")
                .is_none()
        );
    }

    #[test]
    fn comptime_if_uses_builtin_target_fields_for_import_pruning() {
        let root = temp_dir("comptime_if_uses_builtin_target_fields_for_import_pruning");
        write(
            &root.join("main.nia"),
            r#"
comptime if @builtin().target.os == "definitely-not-the-host-os" {
    import .missing;
} else {
    import .present;
}
"#,
        );
        write(&root.join("present.nia"), "pub fn value() i32 { 1 }");

        let program = load_program(root.join("main.nia").to_string_lossy().into_owned());

        assert!(program.diagnostics.is_empty(), "{:?}", program.diagnostics);
        assert_eq!(program.modules.len(), 2);
        assert!(
            program
                .imports
                .get(program.graph.root(), "present")
                .is_some()
        );
        assert!(
            program
                .imports
                .get(program.graph.root(), "missing")
                .is_none()
        );
    }

    #[test]
    fn query_loader_uses_root_module_map() {
        let root = temp_dir("query_loader_uses_root_module_map");
        write(&root.join("main.nia"), "import std.io as io;");
        write(&root.join("std.nia"), "");
        fs::create_dir_all(root.join("std")).expect("create std dir");
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
        assert!(program.imports.get(program.graph.root(), "io").is_some());
        assert!(program.modules.iter().any(
            |module| module.path.as_str() == root.join("std/io.nia").to_string_lossy().as_ref()
        ));
    }

    #[test]
    fn query_loader_injects_default_std_module_map_to_toolchain_lib() {
        let root = temp_dir("query_loader_injects_default_std_module_map_to_toolchain_lib");
        let main_path = root.join("main.nia");
        write(&main_path, "import std;");

        let program = load_program(main_path.to_string_lossy().into_owned());

        assert!(program.diagnostics.is_empty(), "{:?}", program.diagnostics);
        let std_alias = program
            .imports
            .get(program.graph.root(), "std")
            .expect("std import alias");
        let std_module = program.graph.get(std_alias.target).expect("std module");
        assert_eq!(std_module.path.as_str(), default_std_module_path().as_str());
        for relative in [
            "lib/std/fmt.nia",
            "lib/std/fs.nia",
            "lib/std/io.nia",
            "lib/std/mem.nia",
            "lib/std/os.nia",
            "lib/std/process.nia",
            "lib/std/unicode.nia",
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
    fn query_loader_injects_freestanding_entry_runtime_through_std_start_facade() {
        let root =
            temp_dir("query_loader_injects_freestanding_entry_runtime_through_std_start_facade");
        let main_path = root.join("main.nia");
        write(
            &main_path,
            "import std.process; pub fn main(init: process::Init) process::ExitCode!void { _ = init; !{} }",
        );

        let program = load_program_with_map_and_entry_runtime(
            main_path.to_string_lossy().into_owned(),
            ModuleMap::default(),
            EntryRuntime::Freestanding,
        );

        assert!(program.diagnostics.is_empty(), "{:?}", program.diagnostics);
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
        let root = program
            .graph
            .get(program.graph.root())
            .expect("root module");
        assert!(root.imports.iter().any(|import| {
            import.alias == "<std.start>" && import.path.as_str().ends_with("lib/std/start.nia")
        }));
    }

    #[test]
    fn query_loader_injects_root_module_map_to_entry_file() {
        let root = temp_dir("query_loader_injects_root_module_map_to_entry_file");
        let main_path = root.join("main.nia");
        write(&main_path, "import root;");

        let program = load_program(main_path.to_string_lossy().into_owned());

        assert!(program.diagnostics.is_empty(), "{:?}", program.diagnostics);
        assert_eq!(program.modules.len(), 1);
        let root_alias = program
            .imports
            .get(program.graph.root(), "root")
            .expect("root import alias");
        assert_eq!(root_alias.target, program.graph.root());
    }

    #[test]
    fn query_loader_accepts_in_memory_sources() {
        let sources = SourceDatabase::new();
        sources.set_source(SourcePath::new("main.nia"), "import .defs;");
        sources.set_source(SourcePath::new("defs.nia"), "pub fn value() i32 { 1 }");

        let program = load_program_from_sources("main.nia", ModuleMap::default(), sources);

        assert!(program.diagnostics.is_empty(), "{:?}", program.diagnostics);
        assert_eq!(program.modules.len(), 2);
        assert_eq!(program.modules[0].source, "import .defs;");
        assert_eq!(program.modules[1].source, "pub fn value() i32 { 1 }");
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
                .starts_with(&format!("module_imports({main_path})@"))
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
            root_path: main.clone(),
            module_map: ModuleMap::default(),
            sources: sources.clone(),
            target: TargetConfig::host(),
            entry_runtime: EntryRuntime::None,
        });

        let first = db.query(LoadedProgramQuery);
        assert!(first.diagnostics.is_empty(), "{:?}", first.diagnostics);
        assert_eq!(first.modules[0].source, "fn main() i32 { 0 }");

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
        assert_eq!(second.modules[0].source, "fn main() i32 { 1 }");
    }

    #[test]
    fn invalidates_import_graph_after_import_text_change() {
        let sources = SourceDatabase::new();
        let main = SourcePath::new("main.nia");
        sources.set_source(main.clone(), "");
        sources.set_source(SourcePath::new("defs.nia"), "pub fn value() i32 { 1 }");
        let db = QueryDb::new(LoaderContext {
            root_path: main.clone(),
            module_map: ModuleMap::default(),
            sources: sources.clone(),
            target: TargetConfig::host(),
            entry_runtime: EntryRuntime::None,
        });

        let first = db.query(LoadedProgramQuery);
        assert!(first.diagnostics.is_empty(), "{:?}", first.diagnostics);
        assert_eq!(first.modules.len(), 1);

        sources.set_source(main.clone(), "import .defs;");
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
            root_path: SourcePath::new("main.nia"),
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
}
