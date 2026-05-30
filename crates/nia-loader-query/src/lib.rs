// SPDX-License-Identifier: GPL-3.0-or-later
use nia_compiler_query::{LoadedModule, LoadedProgram, ProgramDiagnostic};
use nia_diagnostic::Diagnostic;
use nia_imports::{
    ImportAliasMap, ModuleGraph, ModuleMap, ResolvedImport, add_resolved_imports,
    collect_import_aliases, resolve_module_imports,
};
use nia_lexer::Token;
use nia_query::{QueryDb, QueryKey};
use nia_source::{SourceDatabase, SourceFile, SourcePath};
use nia_span::Span;

pub fn load_program(root_path: impl Into<String>) -> LoadedProgram {
    load_program_with_map(root_path, ModuleMap::default())
}

pub fn load_program_with_map(root_path: impl Into<String>, module_map: ModuleMap) -> LoadedProgram {
    let db = QueryDb::new(LoaderContext {
        root_path: SourcePath::new(root_path.into()),
        module_map,
        sources: SourceDatabase::new(),
    });
    db.query(LoadedProgramQuery)
}

#[cfg(test)]
fn load_program_from_sources(
    root_path: impl Into<String>,
    module_map: ModuleMap,
    sources: SourceDatabase,
) -> LoadedProgram {
    let db = QueryDb::new(LoaderContext {
        root_path: SourcePath::new(root_path.into()),
        module_map,
        sources,
    });
    db.query(LoadedProgramQuery)
}

#[cfg(test)]
fn load_program_trace(
    root_path: impl Into<String>,
    module_map: ModuleMap,
) -> nia_query::QueryTrace {
    let db = QueryDb::new(LoaderContext {
        root_path: SourcePath::new(root_path.into()),
        module_map,
        sources: SourceDatabase::new(),
    });
    let _ = db.query(LoadedProgramQuery);
    db.query_trace()
}

struct LoaderContext {
    root_path: SourcePath,
    module_map: ModuleMap,
    sources: SourceDatabase,
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
        let mut index = 0;
        while index < graph.modules().count() {
            let Some(node) = graph.get(nia_imports::ModuleId(index as u32)).cloned() else {
                break;
            };
            let imports = db.query(ModuleImportsQuery(node.path));
            add_resolved_imports(&mut graph, node.id, imports.imports);
            index += 1;
        }
        graph
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
            let parsed = db.query(ParsedModuleQuery(node.path.clone()));
            diagnostics.extend(module_diagnostics(
                &node.path,
                &parsed
                    .parse_errors
                    .iter()
                    .map(|error| Diagnostic::error(error.span, error.message.clone()))
                    .collect::<Vec<_>>(),
            ));
            diagnostics.extend(module_diagnostics(
                &node.path,
                &db.query(ModuleImportsQuery(node.path.clone())).diagnostics,
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
            .unwrap_or_else(|| panic!("missing module id for `{}`", self.0.as_str()));
        let parsed = db.query(ParsedModuleQuery(self.0.clone()));
        LoadedModule {
            id,
            path: self.0.clone(),
            source: parsed.source.text,
            module: parsed.module,
            parse_errors: parsed.parse_errors,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
struct ParsedModuleQuery(SourcePath);

impl QueryKey<LoaderContext> for ParsedModuleQuery {
    type Value = ParsedModule;

    fn name() -> &'static str {
        "parsed_module"
    }

    fn description(&self) -> String {
        format!("parsed_module({})", self.0.as_str())
    }

    fn execute(&self, db: &QueryDb<LoaderContext>) -> Self::Value {
        let source = db.query(SourceTextQuery(self.0.clone()));
        let tokens = db.query(TokenizedModuleQuery(self.0.clone()));
        let text = source
            .file
            .as_ref()
            .map(|file| file.text.as_str())
            .unwrap_or("");
        let (module, parse_errors) = nia_parser::parse_module_tokens(text, tokens);
        ParsedModule {
            source: source
                .file
                .unwrap_or_else(|| db.context().sources.empty_source(&self.0)),
            module,
            parse_errors,
            read_diagnostic: source.diagnostic,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
struct TokenizedModuleQuery(SourcePath);

impl QueryKey<LoaderContext> for TokenizedModuleQuery {
    type Value = Vec<Token>;

    fn name() -> &'static str {
        "tokenized_module"
    }

    fn description(&self) -> String {
        format!("tokenized_module({})", self.0.as_str())
    }

    fn execute(&self, db: &QueryDb<LoaderContext>) -> Self::Value {
        let source = db.query(SourceTextQuery(self.0.clone()));
        source
            .file
            .as_ref()
            .map(|file| nia_lexer::tokenize(&file.text))
            .unwrap_or_else(|| nia_lexer::tokenize(""))
    }
}

#[derive(Debug, Clone, PartialEq)]
struct ParsedModule {
    source: SourceFile,
    module: nia_ast::Module,
    parse_errors: Vec<nia_parser::ParseError>,
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
                diagnostic: Some(Diagnostic::error(
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
struct ModuleImportsQuery(SourcePath);

impl QueryKey<LoaderContext> for ModuleImportsQuery {
    type Value = ModuleImports;

    fn name() -> &'static str {
        "module_imports"
    }

    fn description(&self) -> String {
        format!("module_imports({})", self.0.as_str())
    }

    fn execute(&self, db: &QueryDb<LoaderContext>) -> Self::Value {
        let parsed = db.query(ParsedModuleQuery(self.0.clone()));
        let mut diagnostics = parsed.read_diagnostic.into_iter().collect::<Vec<_>>();
        let imports = if diagnostics.is_empty() && parsed.parse_errors.is_empty() {
            resolve_module_imports(
                &mut diagnostics,
                &self.0,
                &parsed.module,
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
    };

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
                .any(|diagnostic| { diagnostic.diagnostic.message.contains("failed to read") })
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
            dependency.from.description == format!("parsed_module({main_path})")
                && dependency.to.description == format!("tokenized_module({main_path})")
        }));
        assert!(trace.dependencies.iter().any(|dependency| {
            dependency.from.description == format!("tokenized_module({main_path})")
                && dependency.to.description == format!("source_text({main_path})")
        }));
    }

    fn temp_dir(name: &str) -> PathBuf {
        let mut dir = std::env::temp_dir();
        dir.push(format!("nia_loader_query_{name}_{}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).expect("create temp dir");
        dir
    }

    fn write(path: &Path, source: &str) {
        fs::write(path, source).expect("write source");
    }
}
