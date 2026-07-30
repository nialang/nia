use crate::facade_facts::ModuleFacadeFacts;
use crate::graph::ModuleGraphQuery;
use crate::provider_facts::ProviderDemandsQuery;
use crate::used_paths::{ModuleDeclarations, UsedModulePath, collect_used_modules};
use crate::{EntryRuntime, LoaderContext};
use nia_compiler_query::{
    ActiveModuleItemTreeFactKind, FrontendCacheNamespace, FrontendFacadeFactsCacheKey,
    FrontendModuleDependenciesCacheKey, FrontendProviderSummaryCacheKey,
    FrontendPublicSurfaceFactsCacheKey, FrontendSourceCacheKey, ItemSignatureFingerprint,
    LoadedModule, LoadedProgram, ProgramDiagnostic, ProgramDiagnosticBundles, RuntimeModel,
    SourceContentFingerprint, frontend_module_map_fingerprint, item_signature_fingerprint,
    source_content_fingerprint,
};
use nia_diagnostic::{Diagnostic, codes};
use nia_imports::{
    StableModuleKey, resolve_module_declarations_from_active_item_tree_with_symbols,
};
use nia_item_tree::{ActiveModuleItemTree, ModuleItemTree};
use nia_provider_summary::ProviderSummary;
use nia_query::{
    QueryDb, QueryFingerprint, QueryFingerprintBuilder, QueryFingerprintPolicy, QueryKey,
    QueryResult, QueryRetirement,
};
use nia_source::{SourceFile, SourceId, SourcePath, SourceRevision, SourceVersion};
use nia_target_config::prune_module_for_target_with_symbols;
use std::sync::Arc;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub(crate) struct LoadedProgramQuery;

impl QueryKey<LoaderContext> for LoadedProgramQuery {
    type Value = LoadedProgramValue;

    fn name() -> &'static str {
        "loaded_program"
    }

    fn execute_result(&self, db: &QueryDb<LoaderContext>) -> QueryResult<Self::Value> {
        let graph = db.get(ModuleGraphQuery)?;
        let modules = graph
            .semantic
            .modules()
            .map(|node| {
                let source_id = db.context().sources.id_for_path(&node.path);
                db.get(LoadedModuleQuery(source_id))
                    .map(|module| module.as_ref().clone())
            })
            .collect::<QueryResult<Vec<_>>>()?;
        let diagnostics = db.get(LoadDiagnosticsQuery)?;
        let provider_fact_revision = db.get(ProviderDemandsQuery)?.revision();
        Ok(LoadedProgramValue {
            graph: graph.semantic.clone(),
            provider_fact_revision,
            symbols: db.context().symbols.clone(),
            target: db.context().target.clone(),
            runtime: runtime_model(db.context().entry_runtime),
            toolchain_identity: db.context().toolchain_identity,
            modules,
            diagnostics: diagnostics.as_ref().clone(),
        })
    }
}

#[derive(Debug, Clone, PartialEq)]
pub(crate) struct LoadedProgramValue {
    pub(crate) graph: nia_imports::ModuleGraphSnapshot,
    pub(crate) provider_fact_revision: nia_compiler_query::ProviderFactRevision,
    pub(crate) symbols: nia_symbol_table::SymbolTable,
    pub(crate) target: nia_target_config::TargetConfig,
    pub(crate) runtime: RuntimeModel,
    pub(crate) toolchain_identity: nia_toolchain::ToolchainIdentityFingerprint,
    pub(crate) modules: Vec<LoadedModule>,
    pub(crate) diagnostics: ProgramDiagnosticBundles,
}

impl LoadedProgramValue {
    pub(crate) fn to_program(&self) -> LoadedProgram {
        LoadedProgram {
            graph: self.graph.clone(),
            provider_fact_revision: self.provider_fact_revision,
            symbols: self.symbols.clone(),
            target: self.target.clone(),
            runtime: self.runtime,
            toolchain_identity: self.toolchain_identity,
            modules: self.modules.clone(),
            diagnostics: self.diagnostics.to_diagnostics(),
        }
    }
}

pub(crate) fn runtime_model(entry_runtime: EntryRuntime) -> RuntimeModel {
    match entry_runtime {
        EntryRuntime::None => RuntimeModel::Bare,
        EntryRuntime::Freestanding => RuntimeModel::FreestandingExecutable,
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub(crate) struct LoadDiagnosticsQuery;

impl QueryKey<LoaderContext> for LoadDiagnosticsQuery {
    type Value = ProgramDiagnosticBundles;

    fn name() -> &'static str {
        "load_diagnostics"
    }

    fn execute_result(&self, db: &QueryDb<LoaderContext>) -> QueryResult<Self::Value> {
        let graph = db.get(ModuleGraphQuery)?;
        let mut diagnostics = graph.diagnostics.clone();
        for node in graph.semantic.modules() {
            let parsed = db.get(parsed_module_query(db, &node.path)?)?;
            let declarations = db.get(module_declarations_query(db, &node.path)?)?;
            diagnostics = diagnostics.append(&ProgramDiagnosticBundles::from_diagnostics_in(
                db.context().diagnostic_store.clone(),
                parsed
                    .semantic
                    .parse_errors
                    .iter()
                    .map(|error| ProgramDiagnostic {
                        path: node.path.clone(),
                        diagnostic: Diagnostic::user_error_at(
                            codes::PARSE,
                            error.span,
                            error.message.clone(),
                        ),
                    })
                    .collect(),
            ));
            diagnostics = diagnostics.append(&ProgramDiagnosticBundles::from_source_bundle(
                db.context().diagnostic_store.clone(),
                node.path.clone(),
                parsed.prune_diagnostics.clone(),
            ));
            for bundle in declarations.diagnostics.iter() {
                diagnostics = diagnostics.append(&ProgramDiagnosticBundles::from_source_bundle(
                    db.context().diagnostic_store.clone(),
                    node.path.clone(),
                    bundle.clone(),
                ));
            }
            if node.module_path.is_entry_package() {
                diagnostics = diagnostics.append(&ProgramDiagnosticBundles::from_diagnostics_in(
                    db.context().diagnostic_store.clone(),
                    unused_import_diagnostics(
                        &graph.semantic,
                        node.id,
                        &node.path,
                        &declarations.semantic,
                        &db.context().symbols,
                    ),
                ));
            }
        }
        Ok(diagnostics)
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

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub(crate) struct LoadedModuleQuery(pub(crate) SourceId);

impl QueryKey<LoaderContext> for LoadedModuleQuery {
    type Value = LoadedModule;

    fn name() -> &'static str {
        "loaded_module"
    }

    fn description(&self) -> String {
        format!("loaded_module({:?})", self.0)
    }

    fn execute_result(&self, db: &QueryDb<LoaderContext>) -> QueryResult<Self::Value> {
        let path = db
            .context()
            .sources
            .path_for_id(self.0)
            .ok_or_else(|| db.invalid_input(self, format!("unknown source id {:?}", self.0)))?;
        let graph = db.get(ModuleGraphQuery)?;
        let id = graph
            .semantic
            .module_id_for_source_identity(&path.identity())
            .ok_or_else(|| {
                db.invalid_input(self, format!("missing module id for `{}`", path.as_str()))
            })?;
        let parsed = db.get(parsed_module_query_for_id(db, self.0)?)?;
        let provider_summary = db.get(provider_summary_query_for_id(db, self.0)?)?;
        Ok(LoadedModule {
            id,
            path: path.as_ref().clone(),
            source_identity: path.identity(),
            source_version: parsed.semantic.source_version,
            item_tree: parsed.semantic.item_tree.clone(),
            active_item_tree: parsed.semantic.active_item_tree.clone(),
            provider_summary: provider_summary.as_ref().clone(),
            origins: parsed.semantic.origins.clone(),
            parse_errors: parsed.semantic.parse_errors.clone(),
        })
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub(crate) struct ParsedModuleQuery(SourceVersion);

impl QueryKey<LoaderContext> for ParsedModuleQuery {
    type Value = ParsedModuleValue;

    fn name() -> &'static str {
        "parsed_module"
    }

    fn description(&self) -> String {
        format!("parsed_module({:?})", self.0)
    }

    fn execute_result(&self, db: &QueryDb<LoaderContext>) -> QueryResult<Self::Value> {
        let source = db.get(SourceTextQuery(self.0.id))?;
        let syntax = db.get(SyntaxModuleQuery(self.0))?;
        let (raw_module, parse_errors, origins) =
            nia_parser::parse_module_syntax_with_node_store_and_symbols(
                &syntax,
                &db.context().node_store,
                db.context().symbols.clone(),
            );
        let item_tree = ModuleItemTree::from_module(&raw_module);
        let prune_result = prune_module_for_target_with_symbols(
            raw_module,
            &db.context().target,
            Some(&db.context().symbols),
        );
        Ok(ParsedModuleValue {
            semantic: ParsedModule {
                source_version: self.0,
                item_tree,
                active_item_tree: prune_result.active_item_tree,
                origins,
                parse_errors,
            },
            prune_diagnostics: db
                .context()
                .diagnostic_store
                .bundle(prune_result.diagnostics),
            read_diagnostics: source.diagnostics.clone(),
        })
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub(crate) struct SyntaxModuleQuery(SourceVersion);

impl QueryKey<LoaderContext> for SyntaxModuleQuery {
    type Value = nia_syntax::SyntaxTree;

    fn name() -> &'static str {
        "syntax_module"
    }

    fn description(&self) -> String {
        format!("syntax_module({:?})", self.0)
    }

    fn execute_result(&self, db: &QueryDb<LoaderContext>) -> QueryResult<Self::Value> {
        let source = db.get(SourceTextQuery(self.0.id))?;
        Ok(source
            .file
            .as_ref()
            .filter(|file| file.version() == self.0)
            .map(|file| nia_syntax::parse_source(&file.text, Some(file.version())))
            .unwrap_or_else(|| nia_syntax::parse_source("", Some(self.0))))
    }
}

#[derive(Debug, Clone, PartialEq)]
pub(crate) struct ParsedModule {
    pub(crate) source_version: SourceVersion,
    pub(crate) item_tree: ModuleItemTree,
    pub(crate) active_item_tree: ActiveModuleItemTree,
    pub(crate) origins: nia_node_id::NodeOriginTable,
    pub(crate) parse_errors: Vec<nia_parser::ParseError>,
}

#[derive(Debug, Clone, PartialEq)]
pub(crate) struct ParsedModuleValue {
    pub(crate) semantic: ParsedModule,
    pub(crate) prune_diagnostics: nia_diagnostic::DiagnosticBundle,
    pub(crate) read_diagnostics: nia_diagnostic::DiagnosticBundle,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub(crate) struct ModuleItemTreeFactQuery(pub(crate) SourceId);

impl QueryKey<LoaderContext> for ModuleItemTreeFactQuery {
    type Value = ModuleItemTree;

    const FINGERPRINT: QueryFingerprintPolicy = QueryFingerprintPolicy::SemanticValue;

    fn name() -> &'static str {
        "loader_module_item_tree_fact"
    }

    fn description(&self) -> String {
        format!("loader_module_item_tree_fact({:?})", self.0)
    }

    fn execute_result(&self, db: &QueryDb<LoaderContext>) -> QueryResult<Self::Value> {
        Ok(db
            .get(parsed_module_query_for_id(db, self.0)?)?
            .semantic
            .item_tree
            .clone())
    }

    fn values_equal(&self, old: &Self::Value, new: &Self::Value) -> bool {
        old == new
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub(crate) struct ActiveModuleItemTreeFactQuery(
    pub(crate) SourceId,
    pub(crate) ActiveModuleItemTreeFactKind,
);

impl QueryKey<LoaderContext> for ActiveModuleItemTreeFactQuery {
    type Value = ActiveModuleItemTree;

    const FINGERPRINT: QueryFingerprintPolicy = QueryFingerprintPolicy::SemanticValue;

    fn name() -> &'static str {
        "loader_active_module_item_tree_fact"
    }

    fn description(&self) -> String {
        format!(
            "loader_active_module_item_tree_fact({:?}, {:?})",
            self.0, self.1
        )
    }

    fn execute_result(&self, db: &QueryDb<LoaderContext>) -> QueryResult<Self::Value> {
        let tree = &db
            .get(parsed_module_query_for_id(db, self.0)?)?
            .semantic
            .active_item_tree;
        Ok(match self.1 {
            ActiveModuleItemTreeFactKind::Signature(set) => tree.signature_items(set),
            ActiveModuleItemTreeFactKind::ConstSignature => tree.const_signature_items(),
            ActiveModuleItemTreeFactKind::Full => tree.clone(),
        })
    }

    fn values_equal(&self, old: &Self::Value, new: &Self::Value) -> bool {
        old == new
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub(crate) struct ModuleOriginsFactQuery(pub(crate) SourceId);

impl QueryKey<LoaderContext> for ModuleOriginsFactQuery {
    type Value = nia_node_id::NodeOriginTable;

    const FINGERPRINT: QueryFingerprintPolicy = QueryFingerprintPolicy::SemanticValue;

    fn name() -> &'static str {
        "loader_module_origins_fact"
    }

    fn description(&self) -> String {
        format!("loader_module_origins_fact({:?})", self.0)
    }

    fn execute_result(&self, db: &QueryDb<LoaderContext>) -> QueryResult<Self::Value> {
        Ok(db
            .get(parsed_module_query_for_id(db, self.0)?)?
            .semantic
            .origins
            .clone())
    }

    fn values_equal(&self, old: &Self::Value, new: &Self::Value) -> bool {
        old == new
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub(crate) struct ModuleParseErrorsFactQuery(pub(crate) SourceId);

impl QueryKey<LoaderContext> for ModuleParseErrorsFactQuery {
    type Value = Vec<nia_parser::ParseError>;

    const FINGERPRINT: QueryFingerprintPolicy = QueryFingerprintPolicy::SemanticValue;

    fn name() -> &'static str {
        "loader_module_parse_errors_fact"
    }

    fn description(&self) -> String {
        format!("loader_module_parse_errors_fact({:?})", self.0)
    }

    fn execute_result(&self, db: &QueryDb<LoaderContext>) -> QueryResult<Self::Value> {
        Ok(db
            .get(parsed_module_query_for_id(db, self.0)?)?
            .semantic
            .parse_errors
            .clone())
    }

    fn values_equal(&self, old: &Self::Value, new: &Self::Value) -> bool {
        old == new
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub(crate) struct SourceTextQuery(pub(crate) SourceId);

impl QueryKey<LoaderContext> for SourceTextQuery {
    type Value = SourceText;

    fn name() -> &'static str {
        "source_text"
    }

    fn description(&self) -> String {
        format!("source_text({:?})", self.0)
    }

    fn execute_result(&self, db: &QueryDb<LoaderContext>) -> QueryResult<Self::Value> {
        let path = db
            .context()
            .sources
            .path_for_id(self.0)
            .ok_or_else(|| db.invalid_input(self, format!("unknown source id {:?}", self.0)))?;
        Ok(match db.context().sources.read_source(&path) {
            Ok(file) => SourceText {
                file: Some(file),
                diagnostics: db.context().diagnostic_store.bundle(Vec::new()),
            },
            Err(err) => SourceText {
                file: None,
                diagnostics: db.context().diagnostic_store.bundle(vec![
                    Diagnostic::user_error(
                        codes::LOAD,
                        format!("failed to read `{}`: {err}", path.as_str()),
                    )
                    .debug("path", path.as_str())
                    .finish(),
                ]),
            },
        })
    }
}

#[derive(Debug, Clone, PartialEq)]
pub(crate) struct SourceText {
    pub(crate) file: Option<SourceFile>,
    pub(crate) diagnostics: nia_diagnostic::DiagnosticBundle,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum SourceStatus {
    Missing,
    Present(SourceVersion),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub(crate) struct SourceStatusQuery(pub(crate) SourceId);

impl QueryKey<LoaderContext> for SourceStatusQuery {
    type Value = SourceStatus;

    const FINGERPRINT: QueryFingerprintPolicy = QueryFingerprintPolicy::StableValue;

    fn name() -> &'static str {
        "source_status"
    }

    fn description(&self) -> String {
        format!("source_status({:?})", self.0)
    }

    fn execute_result(&self, db: &QueryDb<LoaderContext>) -> QueryResult<Self::Value> {
        Ok(db
            .get(SourceTextQuery(self.0))?
            .file
            .as_ref()
            .map_or(SourceStatus::Missing, |file| {
                SourceStatus::Present(file.version())
            }))
    }

    fn fingerprint(&self, value: &Self::Value) -> Option<QueryFingerprint> {
        let mut builder = QueryFingerprintBuilder::new("nia.loader.source-status.v1");
        builder.write_u64(u64::from(self.0.0));
        match value {
            SourceStatus::Missing => builder.write_u8(0),
            SourceStatus::Present(version) => {
                builder.write_u8(1);
                builder.write_u64(version.revision.0);
            }
        }
        Some(builder.finish())
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub(crate) struct ModuleDeclarationsQuery(SourceVersion);

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub(crate) struct PublicSurfaceModuleFactsQuery(SourceVersion);

impl QueryKey<LoaderContext> for ModuleDeclarationsQuery {
    type Value = ModuleDeclarationsValue;

    fn name() -> &'static str {
        "module_declarations"
    }

    fn description(&self) -> String {
        format!("module_declarations({:?})", self.0)
    }

    fn execute_result(&self, db: &QueryDb<LoaderContext>) -> QueryResult<Self::Value> {
        let cache_input = frontend_cache_input(db, self.0)?;
        let module_map = frontend_module_map_fingerprint(&db.context().module_map);
        let cached = cache_input.as_ref().and_then(|input| {
            let cache = db.context().frontend_cache.as_ref()?;
            let key = FrontendModuleDependenciesCacheKey::new(
                input.namespace,
                &input.module,
                input.source,
                module_map,
            );
            match cache
                .load_module_dependencies(
                    key,
                    input.namespace,
                    &input.module,
                    crate::frontend_cache::ModuleDependenciesSource::new(
                        input.source,
                        input.source_len,
                    ),
                    module_map,
                    &db.context().symbols,
                )
                .ok()?
            {
                crate::frontend_cache::ModuleDependenciesCacheLookup::Hit(declarations) => {
                    Some(declarations)
                }
                crate::frontend_cache::ModuleDependenciesCacheLookup::NotFound
                | crate::frontend_cache::ModuleDependenciesCacheLookup::Corrupt => None,
            }
        });
        if let Some(cached) = &cached
            && !db.context().verify_frontend_cache
        {
            return Ok(ModuleDeclarationsValue {
                semantic: cached.clone(),
                diagnostics: Arc::from([]),
            });
        }

        let parsed = db.get(ParsedModuleQuery(self.0))?;
        let mut declaration_diagnostics = Vec::new();
        let (declarations, used_modules) = if parsed.read_diagnostics.is_empty()
            && parsed.semantic.parse_errors.is_empty()
            && parsed.prune_diagnostics.is_empty()
        {
            let declarations = resolve_module_declarations_from_active_item_tree_with_symbols(
                &mut declaration_diagnostics,
                &parsed.semantic.active_item_tree,
                &db.context().symbols,
            );
            let used_modules =
                collect_used_modules(&parsed.semantic.active_item_tree, &db.context().module_map);
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
        let declaration_diagnostics = db
            .context()
            .diagnostic_store
            .bundle(declaration_diagnostics);
        let mut diagnostic_bundles = Vec::new();
        if !parsed.read_diagnostics.is_empty() {
            diagnostic_bundles.push(parsed.read_diagnostics.clone());
        }
        if !declaration_diagnostics.is_empty() {
            diagnostic_bundles.push(declaration_diagnostics);
        }
        let fresh = ModuleDeclarationsValue {
            semantic: ModuleDeclarations {
                declarations,
                package_roots: used_modules.package_roots,
                used_module_paths: used_modules.used_module_paths,
                explicit_imports: used_modules.explicit_imports,
                used_import_aliases: used_modules.used_aliases,
            },
            diagnostics: diagnostic_bundles.into(),
        };
        if let Some(input) = cache_input
            && parsed.read_diagnostics.is_empty()
            && parsed.semantic.parse_errors.is_empty()
            && parsed.prune_diagnostics.is_empty()
            && fresh.diagnostics.is_empty()
            && let Some(cache) = &db.context().frontend_cache
        {
            let key = FrontendModuleDependenciesCacheKey::new(
                input.namespace,
                &input.module,
                input.source,
                module_map,
            );
            if cached
                .as_ref()
                .is_some_and(|cached| cached != &fresh.semantic)
            {
                cache.remove_module_dependencies(key);
            }
            let _ = cache.publish_module_dependencies(
                input.namespace,
                &input.module,
                crate::frontend_cache::ModuleDependenciesSource::new(
                    input.source,
                    input.source_len,
                ),
                module_map,
                &fresh.semantic,
                &db.context().symbols,
            );
        }
        Ok(fresh)
    }
}

#[derive(Debug, Clone, PartialEq)]
pub(crate) struct ModuleDeclarationsValue {
    pub(crate) semantic: ModuleDeclarations,
    pub(crate) diagnostics: Arc<[nia_diagnostic::DiagnosticBundle]>,
}

impl QueryKey<LoaderContext> for PublicSurfaceModuleFactsQuery {
    type Value = nia_defs::PublicSurfaceModuleFacts;

    fn name() -> &'static str {
        "loader_public_surface_module_facts"
    }

    fn description(&self) -> String {
        format!("loader_public_surface_module_facts({:?})", self.0)
    }

    fn execute_result(&self, db: &QueryDb<LoaderContext>) -> QueryResult<Self::Value> {
        let cache_input = frontend_cache_input(db, self.0)?;
        let cached = cache_input.as_ref().and_then(|input| {
            let cache = db.context().frontend_cache.as_ref()?;
            let key = FrontendPublicSurfaceFactsCacheKey::new(
                input.namespace,
                &input.module,
                input.source,
            );
            match cache
                .load_public_surface_facts(
                    key,
                    input.namespace,
                    &input.module,
                    crate::frontend_cache::PublicSurfaceFactsSource::new(
                        input.source,
                        input.source_len,
                    ),
                    &db.context().symbols,
                )
                .ok()?
            {
                crate::frontend_cache::PublicSurfaceFactsCacheLookup::Hit(facts) => Some(facts),
                crate::frontend_cache::PublicSurfaceFactsCacheLookup::NotFound
                | crate::frontend_cache::PublicSurfaceFactsCacheLookup::Corrupt => None,
            }
        });
        if let Some(cached) = &cached
            && !db.context().verify_frontend_cache
        {
            return Ok(cached.clone());
        }

        let graph = db.get(ModuleGraphQuery)?;
        let source = db.get(SourceTextQuery(self.0.id))?;
        let source_identity = source
            .file
            .as_ref()
            .filter(|file| file.version() == self.0)
            .map(|file| file.path.identity())
            .ok_or_else(|| db.invalid_input(self, format!("missing source for {:?}", self.0)))?;
        let module_id = graph
            .semantic
            .module_id_for_source_identity(&source_identity)
            .ok_or_else(|| {
                db.invalid_input(
                    self,
                    format!("source {:?} is outside the module graph", self.0),
                )
            })?;
        let item_tree = db.get(ActiveModuleItemTreeFactQuery(
            self.0.id,
            ActiveModuleItemTreeFactKind::Full,
        ))?;
        let defs = nia_defs::collect_module_defs_from_active_item_tree_with_node_store_and_symbols(
            module_id,
            &item_tree,
            &db.context().node_store,
            &db.context().symbols,
        );
        let fresh = nia_defs::PublicSurfaceModuleFacts::from_defs(&defs);
        let parsed = db.get(ParsedModuleQuery(self.0))?;
        if let Some(input) = cache_input
            && parsed.read_diagnostics.is_empty()
            && parsed.semantic.parse_errors.is_empty()
            && parsed.prune_diagnostics.is_empty()
            && defs.diagnostics.is_empty()
            && let Some(cache) = &db.context().frontend_cache
        {
            let key = FrontendPublicSurfaceFactsCacheKey::new(
                input.namespace,
                &input.module,
                input.source,
            );
            if cached.as_ref().is_some_and(|cached| cached != &fresh) {
                cache.remove_public_surface_facts(key);
            }
            let _ = cache.publish_public_surface_facts(
                input.namespace,
                &input.module,
                crate::frontend_cache::PublicSurfaceFactsSource::new(
                    input.source,
                    input.source_len,
                ),
                &fresh,
                &db.context().symbols,
            );
        }
        Ok(fresh)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub(crate) struct ProviderSummaryQuery(SourceVersion);

struct FrontendCacheInput {
    namespace: FrontendCacheNamespace,
    module: StableModuleKey,
    source: SourceContentFingerprint,
    source_len: usize,
    source_key: FrontendSourceCacheKey,
}

fn frontend_cache_input(
    db: &QueryDb<LoaderContext>,
    version: SourceVersion,
) -> QueryResult<Option<FrontendCacheInput>> {
    let source = db.get(SourceTextQuery(version.id))?;
    Ok(source
        .file
        .as_ref()
        .filter(|file| file.version() == version)
        .map(|file| {
            let namespace = db.context().frontend_cache_namespace();
            let module = StableModuleKey::from_source_identity(file.path.identity());
            let source = source_content_fingerprint(&file.text);
            let source_len = file.text.len();
            let source_key = FrontendSourceCacheKey::new(namespace, &module, source);
            FrontendCacheInput {
                namespace,
                module,
                source,
                source_len,
                source_key,
            }
        }))
}

fn cached_item_signature(
    db: &QueryDb<LoaderContext>,
    input: Option<&FrontendCacheInput>,
) -> Option<ItemSignatureFingerprint> {
    let input = input?;
    let cache = db.context().frontend_cache.as_ref()?;
    match cache
        .load_dependency_manifest(
            input.source_key,
            input.namespace,
            &input.module,
            input.source,
        )
        .ok()?
    {
        crate::frontend_cache::DependencyManifestCacheLookup::Hit(item_signature) => {
            Some(item_signature)
        }
        crate::frontend_cache::DependencyManifestCacheLookup::NotFound
        | crate::frontend_cache::DependencyManifestCacheLookup::Corrupt => None,
    }
}

fn fresh_item_signature(
    db: &QueryDb<LoaderContext>,
    version: SourceVersion,
    input: Option<&FrontendCacheInput>,
    cached: Option<ItemSignatureFingerprint>,
) -> QueryResult<(Arc<ParsedModuleValue>, ItemSignatureFingerprint)> {
    let syntax = db.get(SyntaxModuleQuery(version))?;
    let parsed = db.get(ParsedModuleQuery(version))?;
    let item_signature = item_signature_fingerprint(&syntax, &parsed.semantic.item_tree);
    if let Some(input) = input
        && let Some(cache) = &db.context().frontend_cache
        && cached != Some(item_signature)
    {
        if cached.is_some() {
            cache.remove_dependency_manifest(input.source_key);
        }
        let _ = cache.publish_dependency_manifest(
            input.source_key,
            input.namespace,
            &input.module,
            input.source,
            item_signature,
        );
    }
    Ok((parsed, item_signature))
}

impl QueryKey<LoaderContext> for ProviderSummaryQuery {
    type Value = ProviderSummary;

    fn name() -> &'static str {
        "provider_summary"
    }

    fn description(&self) -> String {
        format!("provider_summary({:?})", self.0)
    }

    fn execute_result(&self, db: &QueryDb<LoaderContext>) -> QueryResult<Self::Value> {
        let cache_input = frontend_cache_input(db, self.0)?;
        let cached_item_signature = cached_item_signature(db, cache_input.as_ref());
        let cached =
            cache_input
                .as_ref()
                .zip(cached_item_signature)
                .and_then(|(input, item_signature)| {
                    let cache = db.context().frontend_cache.as_ref()?;
                    let key = FrontendProviderSummaryCacheKey::new(
                        input.namespace,
                        &input.module,
                        item_signature,
                    );
                    match cache
                        .load_provider_summary(
                            key,
                            input.namespace,
                            &input.module,
                            item_signature,
                            &db.context().symbols,
                        )
                        .ok()?
                    {
                        crate::frontend_cache::ProviderSummaryCacheLookup::Hit(summary) => {
                            Some(summary)
                        }
                        crate::frontend_cache::ProviderSummaryCacheLookup::NotFound
                        | crate::frontend_cache::ProviderSummaryCacheLookup::Corrupt => None,
                    }
                });
        if let Some(cached) = &cached
            && !db.context().verify_frontend_cache
        {
            return Ok(cached.clone());
        }

        let (parsed, item_signature) =
            fresh_item_signature(db, self.0, cache_input.as_ref(), cached_item_signature)?;
        let mut cached_for_item_signature = None;
        if let Some(input) = &cache_input
            && let Some(cache) = &db.context().frontend_cache
        {
            let key = FrontendProviderSummaryCacheKey::new(
                input.namespace,
                &input.module,
                item_signature,
            );
            cached_for_item_signature = match cache.load_provider_summary(
                key,
                input.namespace,
                &input.module,
                item_signature,
                &db.context().symbols,
            ) {
                Ok(crate::frontend_cache::ProviderSummaryCacheLookup::Hit(summary)) => {
                    Some(summary)
                }
                Ok(crate::frontend_cache::ProviderSummaryCacheLookup::NotFound)
                | Ok(crate::frontend_cache::ProviderSummaryCacheLookup::Corrupt)
                | Err(_) => None,
            };
            if let Some(cached) = &cached_for_item_signature
                && !db.context().verify_frontend_cache
            {
                return Ok(cached.clone());
            }
        }

        let fresh = ProviderSummary::from_active_item_tree(&parsed.semantic.active_item_tree);
        if let Some(input) = cache_input
            && let Some(cache) = &db.context().frontend_cache
        {
            let key = FrontendProviderSummaryCacheKey::new(
                input.namespace,
                &input.module,
                item_signature,
            );
            if cached_for_item_signature
                .as_ref()
                .is_some_and(|cached| cached != &fresh)
            {
                cache.remove_provider_summary(key);
            }
            let _ = cache.publish_provider_summary(
                key,
                input.namespace,
                &input.module,
                item_signature,
                &fresh,
                &db.context().symbols,
            );
        }
        Ok(fresh)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub(crate) struct ModuleFacadeFactsQuery(SourceVersion);

impl QueryKey<LoaderContext> for ModuleFacadeFactsQuery {
    type Value = ModuleFacadeFacts;

    fn name() -> &'static str {
        "module_facade_facts"
    }

    fn description(&self) -> String {
        format!("module_facade_facts({:?})", self.0)
    }

    fn execute_result(&self, db: &QueryDb<LoaderContext>) -> QueryResult<Self::Value> {
        let cache_input = frontend_cache_input(db, self.0)?;
        let cached_item_signature = cached_item_signature(db, cache_input.as_ref());
        let module_map = frontend_module_map_fingerprint(&db.context().module_map);
        let cached =
            cache_input
                .as_ref()
                .zip(cached_item_signature)
                .and_then(|(input, item_signature)| {
                    let cache = db.context().frontend_cache.as_ref()?;
                    let key = FrontendFacadeFactsCacheKey::new(
                        input.namespace,
                        &input.module,
                        item_signature,
                        module_map,
                    );
                    match cache
                        .load_facade_facts(
                            key,
                            input.namespace,
                            &input.module,
                            item_signature,
                            module_map,
                            &db.context().symbols,
                        )
                        .ok()?
                    {
                        crate::frontend_cache::FacadeFactsCacheLookup::Hit(facts) => Some(facts),
                        crate::frontend_cache::FacadeFactsCacheLookup::NotFound
                        | crate::frontend_cache::FacadeFactsCacheLookup::Corrupt => None,
                    }
                });
        if let Some(cached) = &cached
            && !db.context().verify_frontend_cache
        {
            return Ok(cached.clone());
        }

        let (parsed, item_signature) =
            fresh_item_signature(db, self.0, cache_input.as_ref(), cached_item_signature)?;
        let mut cached_for_item_signature = None;
        if let Some(input) = &cache_input
            && let Some(cache) = &db.context().frontend_cache
        {
            let key = FrontendFacadeFactsCacheKey::new(
                input.namespace,
                &input.module,
                item_signature,
                module_map,
            );
            cached_for_item_signature = match cache.load_facade_facts(
                key,
                input.namespace,
                &input.module,
                item_signature,
                module_map,
                &db.context().symbols,
            ) {
                Ok(crate::frontend_cache::FacadeFactsCacheLookup::Hit(facts)) => Some(facts),
                Ok(crate::frontend_cache::FacadeFactsCacheLookup::NotFound)
                | Ok(crate::frontend_cache::FacadeFactsCacheLookup::Corrupt)
                | Err(_) => None,
            };
            if let Some(cached) = &cached_for_item_signature
                && !db.context().verify_frontend_cache
            {
                return Ok(cached.clone());
            }
        }

        let fresh = ModuleFacadeFacts::from_active_item_tree(
            &parsed.semantic.active_item_tree,
            &db.context().module_map,
        );
        if let Some(input) = cache_input
            && let Some(cache) = &db.context().frontend_cache
        {
            let key = FrontendFacadeFactsCacheKey::new(
                input.namespace,
                &input.module,
                item_signature,
                module_map,
            );
            if cached_for_item_signature
                .as_ref()
                .is_some_and(|cached| cached != &fresh)
            {
                cache.remove_facade_facts(key);
            }
            let _ = cache.publish_facade_facts(
                input.namespace,
                &input.module,
                item_signature,
                module_map,
                &fresh,
                &db.context().symbols,
            );
        }
        Ok(fresh)
    }
}

pub(crate) fn parsed_module_query(
    db: &QueryDb<LoaderContext>,
    path: &SourcePath,
) -> QueryResult<ParsedModuleQuery> {
    parsed_module_query_for_id(db, db.context().sources.id_for_path(path))
}

pub(crate) fn module_declarations_query(
    db: &QueryDb<LoaderContext>,
    path: &SourcePath,
) -> QueryResult<ModuleDeclarationsQuery> {
    let source_id = db.context().sources.id_for_path(path);
    Ok(ModuleDeclarationsQuery(source_version(db, source_id)?))
}

pub(crate) fn public_surface_module_facts_query(
    db: &QueryDb<LoaderContext>,
    path: &SourcePath,
) -> QueryResult<PublicSurfaceModuleFactsQuery> {
    let source_id = db.context().sources.id_for_path(path);
    Ok(PublicSurfaceModuleFactsQuery(source_version(
        db, source_id,
    )?))
}

pub(crate) fn provider_summary_query(
    db: &QueryDb<LoaderContext>,
    path: &SourcePath,
) -> QueryResult<ProviderSummaryQuery> {
    provider_summary_query_for_id(db, db.context().sources.id_for_path(path))
}

pub(crate) fn module_facade_facts_query(
    db: &QueryDb<LoaderContext>,
    path: &SourcePath,
) -> QueryResult<ModuleFacadeFactsQuery> {
    let source_id = db.context().sources.id_for_path(path);
    Ok(ModuleFacadeFactsQuery(source_version(db, source_id)?))
}

pub(crate) fn retire_source_revision_queries(
    retirement: &QueryRetirement<'_, LoaderContext>,
    version: SourceVersion,
) {
    retirement.retire(&ParsedModuleQuery(version));
    retirement.retire(&SyntaxModuleQuery(version));
    retirement.retire(&ModuleDeclarationsQuery(version));
    retirement.retire(&PublicSurfaceModuleFactsQuery(version));
    retirement.retire(&ProviderSummaryQuery(version));
    retirement.retire(&ModuleFacadeFactsQuery(version));
}

fn parsed_module_query_for_id(
    db: &QueryDb<LoaderContext>,
    source_id: SourceId,
) -> QueryResult<ParsedModuleQuery> {
    Ok(ParsedModuleQuery(source_version(db, source_id)?))
}

fn provider_summary_query_for_id(
    db: &QueryDb<LoaderContext>,
    source_id: SourceId,
) -> QueryResult<ProviderSummaryQuery> {
    Ok(ProviderSummaryQuery(source_version(db, source_id)?))
}

fn source_version(db: &QueryDb<LoaderContext>, source_id: SourceId) -> QueryResult<SourceVersion> {
    Ok(match *db.get(SourceStatusQuery(source_id))? {
        SourceStatus::Present(version) => version,
        SourceStatus::Missing => SourceVersion {
            id: source_id,
            revision: SourceRevision::INITIAL,
        },
    })
}
