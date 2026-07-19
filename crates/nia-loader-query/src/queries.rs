use crate::facade_facts::ModuleFacadeFacts;
use crate::graph::ModuleGraphQuery;
use crate::used_paths::{ModuleDeclarations, UsedModulePath, collect_used_modules};
use crate::{EntryRuntime, LoaderContext};
use nia_compiler_query::{LoadedModule, LoadedProgram, ProgramDiagnostic, RuntimeModel};
use nia_diagnostic::{Diagnostic, codes};
use nia_imports::resolve_module_declarations_from_active_item_tree_with_symbols;
use nia_item_tree::{ActiveModuleItemTree, ModuleItemTree};
use nia_provider_summary::ProviderSummary;
use nia_query::{QueryDb, QueryKey};
use nia_source::{SourceFile, SourcePath, SourceVersion};
use nia_target_config::prune_module_for_target_with_symbols;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub(crate) struct LoadedProgramQuery;

impl QueryKey<LoaderContext> for LoadedProgramQuery {
    type Value = LoadedProgram;

    fn name() -> &'static str {
        "loaded_program"
    }

    fn execute(&self, db: &QueryDb<LoaderContext>) -> Self::Value {
        let graph = db.get(ModuleGraphQuery);
        let modules = graph
            .modules()
            .map(|node| {
                db.get(LoadedModuleQuery(node.path.clone()))
                    .as_ref()
                    .clone()
            })
            .collect::<Vec<_>>();
        let diagnostics = db.get(LoadDiagnosticsQuery);
        LoadedProgram {
            graph: graph.as_ref().clone(),
            symbols: db.context().symbols.clone(),
            target: db.context().target.clone(),
            runtime: runtime_model(db.context().entry_runtime),
            modules,
            diagnostics: diagnostics.as_ref().clone(),
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
pub(crate) struct LoadDiagnosticsQuery;

impl QueryKey<LoaderContext> for LoadDiagnosticsQuery {
    type Value = Vec<ProgramDiagnostic>;

    fn name() -> &'static str {
        "load_diagnostics"
    }

    fn execute(&self, db: &QueryDb<LoaderContext>) -> Self::Value {
        let graph = db.get(ModuleGraphQuery);
        let mut diagnostics = Vec::new();
        for (path, diagnostic) in graph.diagnostics() {
            diagnostics.push(ProgramDiagnostic {
                path: path.clone(),
                diagnostic: diagnostic.clone(),
            });
        }
        for node in graph.modules() {
            let parsed = db.get(parsed_module_query(db, node.path.clone()));
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
                &db.get(module_declarations_query(db, node.path.clone()))
                    .diagnostics,
            ));
            if node.module_path.is_entry_package() {
                diagnostics.extend(unused_import_diagnostics(
                    &graph,
                    node.id,
                    &node.path,
                    &db.get(module_declarations_query(db, node.path.clone())),
                    &db.context().symbols,
                ));
            }
        }
        diagnostics
    }
}

fn unused_import_diagnostics(
    graph: &nia_imports::ModuleGraph,
    module_id: nia_imports::ModuleId,
    path: &SourcePath,
    declarations: &ModuleDeclarations,
    symbols: &nia_symbol_table::SymbolTable,
) -> Vec<ProgramDiagnostic> {
    declarations
        .explicit_imports
        .iter()
        .filter(|import| {
            !declarations.used_import_aliases.contains(&import.alias)
                && !import_target_is_semantic(graph, module_id, &import.path)
        })
        .map(|import| ProgramDiagnostic {
            path: path.clone(),
            diagnostic: import.warning(symbols),
        })
        .collect()
}

fn import_target_is_semantic(
    graph: &nia_imports::ModuleGraph,
    current_module: nia_imports::ModuleId,
    path: &UsedModulePath,
) -> bool {
    let Some(module_id) = import_target_module(graph, current_module, path) else {
        return false;
    };
    graph
        .get(module_id)
        .is_some_and(|module| module.semantic_selected)
        || matches!(
            path,
            UsedModulePath::Package { segments, .. } if segments.is_empty()
        )
}

fn import_target_module(
    graph: &nia_imports::ModuleGraph,
    current_module: nia_imports::ModuleId,
    path: &UsedModulePath,
) -> Option<nia_imports::ModuleId> {
    let mut current = match path {
        UsedModulePath::Package { package, .. } => graph.package_root(package),
        UsedModulePath::PackageRelative { .. } => graph.current_package_root(current_module),
        UsedModulePath::ParentRelative { .. } => {
            graph.get(current_module).and_then(|node| node.parent)
        }
        UsedModulePath::Local { .. } => Some(current_module),
    }?;
    for segment in path.segments() {
        current = graph.get(current)?.children.get(segment).copied()?;
    }
    Some(current)
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
        let graph = db.get(ModuleGraphQuery);
        let id = graph
            .module_id_for_source_identity(&self.0.identity())
            .unwrap_or_else(|| {
                db.invalid_input(self, format!("missing module id for `{}`", self.0.as_str()))
            });
        let parsed = db.get(parsed_module_query(db, self.0.clone()));
        let provider_summary = db.get(provider_summary_query(db, self.0.clone()));
        LoadedModule {
            id,
            path: self.0.clone(),
            source_identity: self.0.identity(),
            source_version: parsed.source_version,
            item_tree: parsed.item_tree.clone(),
            active_item_tree: parsed.active_item_tree.clone(),
            provider_summary: provider_summary.as_ref().clone(),
            origins: parsed.origins.clone(),
            parse_errors: parsed.parse_errors.clone(),
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
        let source = db.get(SourceTextQuery(self.path.clone()));
        let syntax = db.get(SyntaxModuleQuery {
            path: self.path.clone(),
            version: self.version,
        });
        let (raw_module, parse_errors, origins) =
            nia_parser::parse_module_syntax_with_origins_and_symbols(
                &syntax,
                db.context().symbols.clone(),
            );
        let item_tree = ModuleItemTree::from_module(&raw_module);
        let prune_result = prune_module_for_target_with_symbols(
            raw_module,
            &db.context().target,
            Some(&db.context().symbols),
        );
        let source_version = source
            .file
            .as_ref()
            .map(SourceFile::version)
            .unwrap_or_else(|| db.context().sources.empty_source(&self.path).version());
        ParsedModule {
            source_version,
            item_tree,
            active_item_tree: prune_result.active_item_tree,
            origins,
            parse_errors,
            prune_diagnostics: prune_result.diagnostics,
            read_diagnostic: source.diagnostic.clone(),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub(crate) struct SyntaxModuleQuery {
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
        let source = db.get(SourceTextQuery(self.path.clone()));
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
    pub(crate) source_version: SourceVersion,
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
    pub(crate) file: Option<SourceFile>,
    pub(crate) diagnostic: Option<Diagnostic>,
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
        let parsed = db.get(ParsedModuleQuery {
            path: self.path.clone(),
            version: self.version,
        });
        let mut diagnostics = parsed
            .read_diagnostic
            .clone()
            .into_iter()
            .collect::<Vec<_>>();
        let (declarations, used_modules) = if diagnostics.is_empty()
            && parsed.parse_errors.is_empty()
            && parsed.prune_diagnostics.is_empty()
        {
            let declarations = resolve_module_declarations_from_active_item_tree_with_symbols(
                &mut diagnostics,
                &parsed.active_item_tree,
                &db.context().symbols,
            );
            let used_modules =
                collect_used_modules(&parsed.active_item_tree, &db.context().module_map);
            (declarations, used_modules)
        } else {
            (
                Vec::new(),
                crate::used_paths::UsedModuleCollection {
                    package_roots: Vec::new(),
                    used_module_paths: Vec::new(),
                    explicit_imports: Vec::new(),
                    used_aliases: Vec::new(),
                },
            )
        };
        ModuleDeclarations {
            declarations,
            package_roots: used_modules.package_roots,
            used_module_paths: used_modules.used_module_paths,
            explicit_imports: used_modules.explicit_imports,
            used_import_aliases: used_modules.used_aliases,
            diagnostics,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub(crate) struct ProviderSummaryQuery {
    path: SourcePath,
    version: SourceVersion,
}

impl QueryKey<LoaderContext> for ProviderSummaryQuery {
    type Value = ProviderSummary;

    fn name() -> &'static str {
        "provider_summary"
    }

    fn description(&self) -> String {
        format!(
            "provider_summary({})@{:?}",
            self.path.as_str(),
            self.version
        )
    }

    fn execute(&self, db: &QueryDb<LoaderContext>) -> Self::Value {
        let parsed = db.get(ParsedModuleQuery {
            path: self.path.clone(),
            version: self.version,
        });
        ProviderSummary::from_active_item_tree(&parsed.active_item_tree)
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub(crate) struct ModuleFacadeFactsQuery {
    path: SourcePath,
    version: SourceVersion,
}

impl QueryKey<LoaderContext> for ModuleFacadeFactsQuery {
    type Value = ModuleFacadeFacts;

    fn name() -> &'static str {
        "module_facade_facts"
    }

    fn description(&self) -> String {
        format!(
            "module_facade_facts({})@{:?}",
            self.path.as_str(),
            self.version
        )
    }

    fn execute(&self, db: &QueryDb<LoaderContext>) -> Self::Value {
        let parsed = db.get(ParsedModuleQuery {
            path: self.path.clone(),
            version: self.version,
        });
        ModuleFacadeFacts::from_active_item_tree(&parsed.active_item_tree, &db.context().module_map)
    }
}

pub(crate) fn parsed_module_query(
    db: &QueryDb<LoaderContext>,
    path: SourcePath,
) -> ParsedModuleQuery {
    let source = db.get(SourceTextQuery(path.clone()));
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
    let source = db.get(SourceTextQuery(path.clone()));
    let version = source
        .file
        .as_ref()
        .map(SourceFile::version)
        .unwrap_or_else(|| db.context().sources.empty_source(&path).version());
    ModuleDeclarationsQuery { path, version }
}

pub(crate) fn provider_summary_query(
    db: &QueryDb<LoaderContext>,
    path: SourcePath,
) -> ProviderSummaryQuery {
    let source = db.get(SourceTextQuery(path.clone()));
    let version = source
        .file
        .as_ref()
        .map(SourceFile::version)
        .unwrap_or_else(|| db.context().sources.empty_source(&path).version());
    ProviderSummaryQuery { path, version }
}

pub(crate) fn module_facade_facts_query(
    db: &QueryDb<LoaderContext>,
    path: SourcePath,
) -> ModuleFacadeFactsQuery {
    let source = db.get(SourceTextQuery(path.clone()));
    let version = source
        .file
        .as_ref()
        .map(SourceFile::version)
        .unwrap_or_else(|| db.context().sources.empty_source(&path).version());
    ModuleFacadeFactsQuery { path, version }
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
