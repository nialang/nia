use crate::graph::ModuleGraphQuery;
use crate::used_paths::{ModuleDeclarations, collect_used_modules};
use crate::{EntryRuntime, LoaderContext};
use nia_compiler_query::{LoadedModule, LoadedProgram, ProgramDiagnostic, RuntimeModel};
use nia_diagnostic::{Diagnostic, codes};
use nia_imports::resolve_module_declarations_from_active_item_tree;
use nia_item_tree::{ActiveModuleItemTree, ModuleItemTree};
use nia_query::{QueryDb, QueryKey};
use nia_source::{SourceFile, SourcePath, SourceVersion};
use nia_target_config::prune_module_for_target;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub(crate) struct LoadedProgramQuery;

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
pub(crate) struct LoadedModuleQuery(pub(crate) SourcePath);

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
pub(crate) struct ParsedModuleQuery {
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
pub(crate) struct ParsedModule {
    pub(crate) source: SourceFile,
    pub(crate) item_tree: ModuleItemTree,
    pub(crate) active_item_tree: ActiveModuleItemTree,
    pub(crate) origins: nia_node_id::NodeOriginTable,
    pub(crate) parse_errors: Vec<nia_parser::ParseError>,
    pub(crate) prune_diagnostics: Vec<Diagnostic>,
    pub(crate) read_diagnostic: Option<Diagnostic>,
}

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub(crate) struct SourceTextQuery(pub(crate) SourcePath);

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
pub(crate) struct SourceText {
    file: Option<SourceFile>,
    diagnostic: Option<Diagnostic>,
}

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub(crate) struct ModuleDeclarationsQuery {
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

pub(crate) fn parsed_module_query(
    db: &QueryDb<LoaderContext>,
    path: SourcePath,
) -> ParsedModuleQuery {
    let source = db.query(SourceTextQuery(path.clone()));
    let version = source
        .file
        .as_ref()
        .map(SourceFile::version)
        .unwrap_or_else(|| db.context().sources.empty_source(&path).version());
    ParsedModuleQuery { path, version }
}

pub(crate) fn module_declarations_query(
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
