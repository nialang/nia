// SPDX-License-Identifier: GPL-3.0-or-later
//! Incremental source loading, module graph discovery, and frontend products.
//!
//! Loader query state is scoped to a [`nia_query::QuerySession`]. Persistent
//! frontend products use versioned logical source identities and verify their
//! source/module-map dependencies before reuse; a cache miss or invalidated
//! entry falls back to the in-memory query graph.

mod facade_facts;
mod frontend_cache;
mod graph;
mod provider_facts;
mod provider_loading;
mod queries;
mod used_paths;

#[cfg(test)]
mod tests;

use nia_compiler_query::{
    FrontendCacheNamespace, FrontendProgramSourceFingerprint, FrontendProviderDemandPlanCacheKey,
    LoadedProgram, LoaderFactProvider, ProviderDemand, SourceContentFingerprint,
    frontend_module_map_fingerprint, frontend_program_source_fingerprint,
    source_content_fingerprint,
};
use nia_imports::{ModuleMap, StableModuleKey};
use nia_query::{QueryDb, QueryResult, QueryRetirement, QuerySession};
use nia_source::{SourceDatabase, SourceFile, SourcePath, SourceRevision, SourceVersion};
use nia_symbol_table::SymbolTable;
use nia_target_config::TargetConfig;
use nia_toolchain::ToolchainLayout;
use provider_facts::{ProviderDemandsQuery, ProviderFactStore};
use queries::{LoadedProgramQuery, SourceTextQuery};
use std::{
    collections::HashSet,
    path::PathBuf,
    sync::{Arc, Mutex},
};

fn loader_query_registry() -> nia_query::QueryRegistry {
    let mut registry = nia_query::QueryRegistry::new();
    macro_rules! register {
        ($($key:ty),+ $(,)?) => {
            $(registry.register::<LoaderContext, $key>();)+
        };
    }
    register!(
        graph::ModuleGraphQuery,
        graph::ModuleGraphRevisionQuery,
        queries::LoadDiagnosticsQuery,
        queries::ActiveModuleItemTreeFactQuery,
        queries::LoadedModuleQuery,
        queries::LoadedProgramQuery,
        queries::ModuleDeclarationsQuery,
        queries::ModuleFacadeFactsQuery,
        queries::ModuleItemTreeFactQuery,
        queries::ModuleOriginsFactQuery,
        queries::ModuleParseErrorsFactQuery,
        queries::ParsedModuleQuery,
        queries::PublicSurfaceModuleFactsQuery,
        provider_facts::ProviderDemandsQuery,
        queries::ProviderSummaryQuery,
        queries::SourceStatusQuery,
        queries::SourceTextQuery,
        queries::SyntaxModuleQuery,
    );
    registry
}

/// Loads a program from an entry path using the host target and default runtime.
pub fn load_program(
    entry_path: impl Into<String>,
    toolchain: Arc<ToolchainLayout>,
) -> QueryResult<LoadedProgram> {
    load_program_with_map(entry_path, ModuleMap::default(), toolchain)
}

/// Loads a program with explicit package/module mappings.
pub fn load_program_with_map(
    entry_path: impl Into<String>,
    module_map: ModuleMap,
    toolchain: Arc<ToolchainLayout>,
) -> QueryResult<LoadedProgram> {
    load_program_with_map_and_entry_runtime(entry_path, module_map, EntryRuntime::None, toolchain)
}

/// Loads a program with mappings and an explicit entry runtime model.
pub fn load_program_with_map_and_entry_runtime(
    entry_path: impl Into<String>,
    module_map: ModuleMap,
    entry_runtime: EntryRuntime,
    toolchain: Arc<ToolchainLayout>,
) -> QueryResult<LoadedProgram> {
    load_program_request(
        LoadRequest::new(entry_path)
            .with_module_map(module_map)
            .with_entry_runtime(entry_runtime)
            .with_toolchain_layout(toolchain),
    )
}

/// Loads a program using the complete request configuration.
pub fn load_program_request(request: LoadRequest) -> QueryResult<LoadedProgram> {
    LoaderDatabase::new(request).load_program()
}

/// Query-backed loader database with mutable source revisions.
#[derive(Clone)]
pub struct LoaderDatabase {
    db: QueryDb<LoaderContext>,
    sources: SourceDatabase,
}

/// Presence and stable content identity of one loaded source input.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SourceInputContent {
    /// The module was discovered but has no readable source file.
    Missing,
    /// The module has source bytes and their stable identity.
    Present {
        /// Stable content fingerprint.
        fingerprint: SourceContentFingerprint,
        /// Source byte length used alongside the fingerprint.
        byte_len: usize,
    },
}

/// One logical module path and its source-input status.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SourceInput {
    /// Relocation-aware source path.
    pub path: SourcePath,
    /// Presence and content identity.
    pub content: SourceInputContent,
}

/// Deterministically sorted source manifest for a loaded module graph.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SourceInputManifest {
    sources: Vec<SourceInput>,
    fingerprint: Option<FrontendProgramSourceFingerprint>,
}

impl SourceInputManifest {
    fn new(mut sources: Vec<SourceInput>) -> Self {
        sources.sort_unstable_by(|left, right| {
            left.path
                .identity()
                .normalized_path()
                .cmp(right.path.identity().normalized_path())
        });
        assert!(
            sources.windows(2).all(|pair| {
                pair[0].path.identity().normalized_path()
                    != pair[1].path.identity().normalized_path()
            }),
            "Nia ICE: loader source manifest contains duplicate logical identities"
        );
        let fingerprint = sources
            .iter()
            .map(|source| match source.content {
                SourceInputContent::Missing => None,
                SourceInputContent::Present {
                    fingerprint,
                    byte_len,
                } => Some((
                    StableModuleKey::from_source_identity(source.path.identity()),
                    fingerprint,
                    byte_len,
                )),
            })
            .collect::<Option<Vec<_>>>()
            .map(|sources| {
                frontend_program_source_fingerprint(
                    sources
                        .iter()
                        .map(|(module, fingerprint, len)| (module, *fingerprint, *len)),
                )
            });
        Self {
            sources,
            fingerprint,
        }
    }

    /// Returns entries sorted by normalized logical source identity.
    pub fn sources(&self) -> &[SourceInput] {
        &self.sources
    }

    /// Returns a program fingerprint when every discovered source is present.
    pub fn fingerprint(&self) -> Option<FrontendProgramSourceFingerprint> {
        self.fingerprint
    }
}

impl LoaderDatabase {
    /// Creates a loader with an isolated default query session.
    pub fn new(request: LoadRequest) -> Self {
        Self::new_in_session(request, QuerySession::new())
    }

    /// Creates a loader sharing dependency and execution state with `session`.
    pub fn new_in_session(request: LoadRequest, session: QuerySession) -> Self {
        let entry_path = request.entry_path;
        let package_roots_with_used_paths = if request.package_root_used_paths {
            request.module_map.entries().map(|(name, _)| name).collect()
        } else {
            HashSet::new()
        };
        let module_map = effective_module_map(
            &entry_path,
            request.module_map,
            request.toolchain.as_deref(),
        );
        let sources = request.sources;
        let symbols = SymbolTable::new();
        let frontend_cache = request
            .frontend_cache_dir
            .map(|root| Arc::new(frontend_cache::PersistentFrontendCache::new(root)));
        let toolchain_identity = request
            .toolchain
            .as_deref()
            .map(|toolchain| toolchain.identity().fingerprint())
            .unwrap_or_else(nia_toolchain::ToolchainIdentityFingerprint::current);
        let namespace = FrontendCacheNamespace::for_toolchain(
            &request.target,
            queries::runtime_model(request.entry_runtime),
            toolchain_identity,
        );
        let source_roots = std::iter::once(entry_path.clone())
            .chain(request.package_root.clone())
            .chain(module_map.entries().map(|(_, path)| path.clone()))
            .collect::<Vec<_>>();
        let module_map_fingerprint = frontend_module_map_fingerprint(&module_map);
        let provider_demand_plan_key = frontend_cache.as_ref().map(|_| {
            FrontendProviderDemandPlanCacheKey::new_with_package_root(
                namespace,
                &entry_path.identity(),
                module_map_fingerprint,
                request
                    .package_root
                    .as_ref()
                    .map(SourcePath::identity)
                    .as_ref(),
                request.package_root_used_paths,
            )
        });
        let cached_provider_demands = frontend_cache
            .as_ref()
            .zip(provider_demand_plan_key)
            .and_then(|(cache, key)| {
                match cache.load_provider_demand_plan(
                    key,
                    namespace,
                    &entry_path.identity(),
                    module_map_fingerprint,
                    request.package_root_used_paths,
                    &source_roots,
                    &sources,
                    &symbols,
                ) {
                    Ok(frontend_cache::ProviderDemandPlanCacheLookup::Hit(demands)) => {
                        Some(demands)
                    }
                    Ok(
                        frontend_cache::ProviderDemandPlanCacheLookup::NotFound
                        | frontend_cache::ProviderDemandPlanCacheLookup::Invalidated
                        | frontend_cache::ProviderDemandPlanCacheLookup::Corrupt,
                    )
                    | Err(_) => None,
                }
            });
        let provider_facts = ProviderFactStore::default();
        let db = QueryDb::new_registered_in_session(
            LoaderContext {
                entry_path,
                package_root: request.package_root,
                module_map,
                sources: sources.clone(),
                node_store: nia_node_id::NodeStore::new(),
                diagnostic_store: Arc::new(nia_diagnostic::DiagnosticStore::new()),
                symbols,
                target: request.target,
                entry_runtime: request.entry_runtime,
                toolchain_identity,
                package_roots_with_used_paths,
                package_root_used_paths: request.package_root_used_paths,
                provider_facts,
                frontend_cache,
                verify_frontend_cache: request.verify_frontend_cache,
                provider_demand_plan_key,
                provider_demand_plan_candidate: Mutex::new(cached_provider_demands),
            },
            loader_query_registry(),
            session,
        );
        Self { db, sources }
    }

    /// Returns the query session governing this loader.
    pub fn query_session(&self) -> QuerySession {
        self.db.session()
    }

    /// Loads the complete program, replaying a verified provider-demand plan first.
    pub fn load_program(&self) -> QueryResult<LoadedProgram> {
        self.replay_provider_demand_plan()?;
        self.db
            .get(LoadedProgramQuery)
            .map(|program| program.to_program())
    }

    /// Builds a deterministic source manifest from the discovered module graph.
    pub fn source_input_manifest(&self) -> QueryResult<SourceInputManifest> {
        self.replay_provider_demand_plan()?;
        let graph = self.db.get(graph::ModuleGraphQuery)?;
        let mut sources = Vec::with_capacity(graph.semantic.modules().count());
        for module in graph.semantic.modules() {
            let source_id = self.sources.id_for_path(&module.path);
            let source = self.db.get(SourceTextQuery(source_id))?;
            let content = source
                .file
                .as_ref()
                .map_or(SourceInputContent::Missing, |file| {
                    SourceInputContent::Present {
                        fingerprint: source_content_fingerprint(&file.text),
                        byte_len: file.text.len(),
                    }
                });
            sources.push(SourceInput {
                path: module.path.clone(),
                content,
            });
        }
        Ok(SourceInputManifest::new(sources))
    }

    /// Returns the mutable-source database owned by this loader.
    pub fn sources(&self) -> &SourceDatabase {
        &self.sources
    }

    /// Replaces source text, retires the previous revision, and invalidates dependents atomically.
    pub fn set_source(&self, path: impl Into<String>, text: impl Into<Arc<str>>) -> SourceFile {
        let path = SourcePath::new(path.into());
        let source_id = self.sources.id_for_path(&path);
        let previous_version = self.sources.source_for_id(source_id).map_or(
            SourceVersion {
                id: source_id,
                revision: SourceRevision::INITIAL,
            },
            |file| file.version(),
        );
        let text = text.into();
        self.db.retirement_transaction(|retirement| {
            let file = self.sources.set_source(path, text);
            self.reset_provider_facts(retirement);
            retirement.invalidate(SourceTextQuery(file.id));
            queries::retire_source_revision_queries(retirement, previous_version);
            self.db
                .context()
                .node_store
                .retire_revision(previous_version);
            file
        })
    }

    /// Invalidates one source and retires its revision-owned query identities.
    pub fn invalidate_source(&self, path: impl Into<String>) -> nia_query::QueryInvalidation {
        let path = SourcePath::new(path.into());
        let source_id = self.sources.id_for_path(&path);
        let previous_version = self.sources.source_for_id(source_id).map_or(
            SourceVersion {
                id: source_id,
                revision: SourceRevision::INITIAL,
            },
            |file| file.version(),
        );
        self.db.retirement_transaction(|retirement| {
            self.reset_provider_facts(retirement);
            let invalidation = retirement.invalidate(SourceTextQuery(source_id));
            queries::retire_source_revision_queries(retirement, previous_version);
            self.db
                .context()
                .node_store
                .retire_revision(previous_version);
            invalidation
        })
    }

    /// Returns loader query dependencies and per-slot statistics.
    pub fn query_trace(&self) -> nia_query::QueryTrace {
        self.db.query_trace()
    }

    /// Adds provider demands and advances the provider graph until callers settle it.
    pub fn update_provider_demands(
        &self,
        demands: impl IntoIterator<Item = ProviderDemand>,
    ) -> QueryResult<nia_compiler_query::ProviderGraphUpdate> {
        self.replay_provider_demand_plan()?;
        self.update_provider_demands_inner(demands)
    }

    fn update_provider_demands_inner(
        &self,
        demands: impl IntoIterator<Item = ProviderDemand>,
    ) -> QueryResult<nia_compiler_query::ProviderGraphUpdate> {
        let demands = demands.into_iter().collect::<Vec<_>>();
        let all_known = self.db.context().provider_facts.contains_all(&demands);
        if all_known {
            return Ok(nia_compiler_query::ProviderGraphUpdate::Stable);
        }
        let previous_revision = self.db.get(ProviderDemandsQuery)?.revision();
        let previous_graph = self.db.get(graph::ModuleGraphQuery)?;
        let added = self.db.context().provider_facts.insert_new(demands);
        if added.is_empty() {
            return Ok(nia_compiler_query::ProviderGraphUpdate::Stable);
        }
        self.db.invalidate(ProviderDemandsQuery);
        let graph = self.db.get(graph::ModuleGraphQuery)?;
        let current_revision = self.db.get(ProviderDemandsQuery)?.revision();
        assert!(self.db.seal_and_retire_predecessor(
            &graph::ModuleGraphRevisionQuery(current_revision),
            &graph::ModuleGraphRevisionQuery(previous_revision),
        ));
        if graph == previous_graph {
            Ok(nia_compiler_query::ProviderGraphUpdate::Stable)
        } else {
            Ok(nia_compiler_query::ProviderGraphUpdate::Changed {
                invalidates_resolved_body_facts: added
                    .iter()
                    .any(|demand| demand.request.invalidates_resolved_body_facts()),
            })
        }
    }

    fn replay_provider_demand_plan(&self) -> QueryResult<()> {
        if self.db.context().verify_frontend_cache {
            return Ok(());
        }
        let candidate = self
            .db
            .context()
            .provider_demand_plan_candidate
            .lock()
            .expect("provider demand plan candidate lock poisoned")
            .clone();
        if let Some(demands) = candidate
            && !self
                .db
                .context()
                .provider_facts
                .contains_all(&demands.iter().cloned().collect::<Vec<_>>())
        {
            self.update_provider_demands_inner(demands)?;
        }
        Ok(())
    }

    fn reset_provider_facts(&self, retirement: &QueryRetirement<'_, LoaderContext>) {
        if let (Some(cache), Some(key)) = (
            self.db.context().frontend_cache.as_ref(),
            self.db.context().provider_demand_plan_key,
        ) {
            cache.remove_provider_demand_plan(key);
            *self
                .db
                .context()
                .provider_demand_plan_candidate
                .lock()
                .expect("provider demand plan candidate lock poisoned") = None;
        }
        if let Some(previous_revision) = self.db.context().provider_facts.clear() {
            retirement.invalidate(ProviderDemandsQuery);
            retirement.retire(&graph::ModuleGraphRevisionQuery(previous_revision));
        }
    }

    fn settle_provider_demand_plan(&self) -> QueryResult<()> {
        let context = self.db.context();
        let (Some(cache), Some(key)) = (
            context.frontend_cache.as_ref(),
            context.provider_demand_plan_key,
        ) else {
            return Ok(());
        };
        let provider_facts = self.db.get(ProviderDemandsQuery)?;
        let candidate = context
            .provider_demand_plan_candidate
            .lock()
            .expect("provider demand plan candidate lock poisoned")
            .take();
        if candidate
            .as_ref()
            .is_some_and(|demands| demands == provider_facts.as_snapshot().demands())
        {
            return Ok(());
        }
        if candidate.is_some() {
            cache.remove_provider_demand_plan(key);
        }
        let graph = self.db.get(graph::ModuleGraphQuery)?;
        let source_paths = graph
            .semantic
            .modules()
            .map(|module| module.path.clone())
            .collect::<Vec<_>>();
        let namespace = context.frontend_cache_namespace();
        let module_map = frontend_module_map_fingerprint(&context.module_map);
        let snapshot = provider_facts.as_snapshot();
        let _ = cache.publish_provider_demand_plan(
            key,
            namespace,
            &context.entry_path.identity(),
            module_map,
            context.package_root_used_paths,
            &source_paths,
            snapshot.demands(),
            &context.sources,
            &context.symbols,
        );
        Ok(())
    }

    fn source_id_for_module(
        &self,
        module_id: nia_imports::ModuleId,
    ) -> QueryResult<Option<nia_source::SourceId>> {
        let graph = self.db.get(graph::ModuleGraphQuery)?;
        Ok(graph
            .semantic
            .get(module_id)
            .map(|module| self.sources.id_for_path(&module.path)))
    }
}

impl LoaderFactProvider for LoaderDatabase {
    fn query_session(&self) -> Option<QuerySession> {
        Some(self.query_session())
    }

    fn provider_facts(&self) -> QueryResult<nia_compiler_query::ProviderFactSnapshot> {
        Ok(self.db.get(ProviderDemandsQuery)?.as_snapshot())
    }

    fn update_provider_demands(
        &self,
        demands: Vec<ProviderDemand>,
    ) -> QueryResult<nia_compiler_query::ProviderGraphUpdate> {
        LoaderDatabase::update_provider_demands(self, demands)
    }

    fn settle_provider_demands(&self) -> QueryResult<()> {
        self.settle_provider_demand_plan()
    }

    fn node_store(&self) -> nia_node_id::NodeStore {
        self.db.context().node_store.clone()
    }

    fn module_graph(&self) -> QueryResult<nia_imports::ModuleGraphSnapshot> {
        self.db
            .get(graph::ModuleGraphQuery)
            .map(|graph| graph.semantic.clone())
    }

    fn loaded_module_source_identities(&self) -> QueryResult<Vec<nia_source::SourceIdentity>> {
        Ok(self
            .db
            .get(graph::ModuleGraphQuery)?
            .semantic
            .modules()
            .map(|module| module.path.identity())
            .collect())
    }

    fn module_path(&self, module_id: nia_imports::ModuleId) -> QueryResult<Option<SourcePath>> {
        let graph = self.db.get(graph::ModuleGraphQuery)?;
        Ok(graph
            .semantic
            .get(module_id)
            .map(|module| module.path.clone()))
    }

    fn module_source_version(
        &self,
        module_id: nia_imports::ModuleId,
    ) -> QueryResult<Option<nia_source::SourceVersion>> {
        let graph = self.db.get(graph::ModuleGraphQuery)?;
        let Some(module) = graph.semantic.get(module_id) else {
            return Ok(None);
        };
        let source_id = self.sources.id_for_path(&module.path);
        Ok(match *self.db.get(queries::SourceStatusQuery(source_id))? {
            queries::SourceStatus::Present(version) => Some(version),
            queries::SourceStatus::Missing => None,
        })
    }

    fn module_source_fingerprint(
        &self,
        module_id: nia_imports::ModuleId,
    ) -> QueryResult<Option<(nia_compiler_query::SourceContentFingerprint, usize)>> {
        let Some(source_id) = self.source_id_for_module(module_id)? else {
            return Ok(None);
        };
        let source = self.db.get(queries::SourceTextQuery(source_id))?;
        let Some(file) = source.file.as_ref() else {
            return Ok(None);
        };
        Ok(Some((
            nia_compiler_query::source_content_fingerprint(&file.text),
            file.text.len(),
        )))
    }

    fn module_source_text(
        &self,
        module_id: nia_imports::ModuleId,
    ) -> QueryResult<Option<Arc<str>>> {
        let Some(source_id) = self.source_id_for_module(module_id)? else {
            return Ok(None);
        };
        Ok(self
            .db
            .get(queries::SourceTextQuery(source_id))?
            .file
            .as_ref()
            .map(|file| file.text.clone()))
    }

    fn module_provider_summary(
        &self,
        module_id: nia_imports::ModuleId,
    ) -> QueryResult<Option<nia_provider_summary::ProviderSummary>> {
        let graph = self.db.get(graph::ModuleGraphQuery)?;
        let Some(module) = graph.semantic.get(module_id) else {
            return Ok(None);
        };
        let key = queries::provider_summary_query(&self.db, &module.path)?;
        Ok(Some(self.db.get(key)?.as_ref().clone()))
    }

    fn module_public_surface_facts(
        &self,
        module_id: nia_imports::ModuleId,
    ) -> QueryResult<Option<nia_defs::PublicSurfaceModuleFacts>> {
        let graph = self.db.get(graph::ModuleGraphQuery)?;
        let Some(module) = graph.semantic.get(module_id) else {
            return Ok(None);
        };
        let key = queries::public_surface_module_facts_query(&self.db, &module.path)?;
        Ok(Some(self.db.get(key)?.as_ref().clone()))
    }

    fn module_origins(
        &self,
        module_id: nia_imports::ModuleId,
    ) -> QueryResult<Option<nia_node_id::NodeOriginTable>> {
        let Some(source_id) = self.source_id_for_module(module_id)? else {
            return Ok(None);
        };
        Ok(Some(
            self.db
                .get(queries::ModuleOriginsFactQuery(source_id))?
                .as_ref()
                .clone(),
        ))
    }

    fn module_parse_errors(
        &self,
        module_id: nia_imports::ModuleId,
    ) -> QueryResult<Option<Vec<nia_parser::ParseError>>> {
        let Some(source_id) = self.source_id_for_module(module_id)? else {
            return Ok(None);
        };
        Ok(Some(
            self.db
                .get(queries::ModuleParseErrorsFactQuery(source_id))?
                .as_ref()
                .clone(),
        ))
    }

    fn module_item_tree(
        &self,
        module_id: nia_imports::ModuleId,
    ) -> QueryResult<Option<nia_item_tree::ModuleItemTree>> {
        let Some(source_id) = self.source_id_for_module(module_id)? else {
            return Ok(None);
        };
        Ok(Some(
            self.db
                .get(queries::ModuleItemTreeFactQuery(source_id))?
                .as_ref()
                .clone(),
        ))
    }

    fn active_module_item_tree(
        &self,
        module_id: nia_imports::ModuleId,
        kind: nia_compiler_query::ActiveModuleItemTreeFactKind,
    ) -> QueryResult<Option<nia_item_tree::ActiveModuleItemTree>> {
        let Some(source_id) = self.source_id_for_module(module_id)? else {
            return Ok(None);
        };
        Ok(Some(
            self.db
                .get(queries::ActiveModuleItemTreeFactQuery(source_id, kind))?
                .as_ref()
                .clone(),
        ))
    }

    fn load_diagnostics(&self) -> QueryResult<nia_compiler_query::ProgramDiagnosticBundles> {
        self.db
            .get(queries::LoadDiagnosticsQuery)
            .map(|diagnostics| diagnostics.as_ref().clone())
    }

    fn symbols(&self) -> SymbolTable {
        self.db.context().symbols.clone()
    }

    fn target(&self) -> TargetConfig {
        self.db.context().target.clone()
    }

    fn runtime(&self) -> nia_compiler_query::RuntimeModel {
        queries::runtime_model(self.db.context().entry_runtime)
    }

    fn toolchain_identity(&self) -> nia_toolchain::ToolchainIdentityFingerprint {
        self.db.context().toolchain_identity
    }
}

/// Complete loader configuration, including source, target, runtime, and cache policy.
#[derive(Debug, Clone)]
pub struct LoadRequest {
    /// Entry source path.
    pub entry_path: SourcePath,
    /// Optional package root source path for a separate package-root module.
    pub package_root: Option<SourcePath>,
    /// Explicit package/module mappings.
    pub module_map: ModuleMap,
    /// Initial in-memory source database.
    pub sources: SourceDatabase,
    /// Artifact target used for conditional frontend selection.
    pub target: TargetConfig,
    /// Entry runtime model.
    pub entry_runtime: EntryRuntime,
    /// Whether package-root `using` paths participate in the source manifest.
    pub package_root_used_paths: bool,
    /// Optional persistent frontend cache root.
    pub frontend_cache_dir: Option<PathBuf>,
    /// Whether semantically valid cache products must be recomputed and verified.
    pub verify_frontend_cache: bool,
    /// Optional resolved toolchain supplying std modules and identity.
    pub toolchain: Option<Arc<ToolchainLayout>>,
}

impl LoadRequest {
    /// Creates a request from a string entry path with host/default settings.
    pub fn new(entry_path: impl Into<String>) -> Self {
        Self::from_source_path(SourcePath::new(entry_path.into()))
    }

    /// Creates a request preserving an already normalized [`SourcePath`].
    pub fn from_source_path(entry_path: SourcePath) -> Self {
        Self {
            entry_path,
            package_root: None,
            module_map: ModuleMap::default(),
            sources: SourceDatabase::new(),
            target: TargetConfig::host(),
            entry_runtime: EntryRuntime::None,
            package_root_used_paths: false,
            frontend_cache_dir: None,
            verify_frontend_cache: false,
            toolchain: None,
        }
    }

    /// Selects a separate `pkg.nia` package root for this entry.
    pub fn with_package_root(mut self, package_root: SourcePath) -> Self {
        self.package_root = Some(package_root);
        self
    }

    /// Replaces explicit module/package mappings.
    pub fn with_module_map(mut self, module_map: ModuleMap) -> Self {
        self.module_map = module_map;
        self
    }

    /// Supplies an initial source database, useful for in-memory compilation.
    pub fn with_sources(mut self, sources: SourceDatabase) -> Self {
        self.sources = sources;
        self
    }

    /// Selects the artifact target used by conditional item selection.
    pub fn with_target(mut self, target: TargetConfig) -> Self {
        self.target = target;
        self
    }

    /// Selects the entry runtime model.
    pub fn with_entry_runtime(mut self, entry_runtime: EntryRuntime) -> Self {
        self.entry_runtime = entry_runtime;
        self
    }

    /// Enables package-root paths in provider/source dependency identity.
    pub fn with_package_root_used_paths(mut self, package_root_used_paths: bool) -> Self {
        self.package_root_used_paths = package_root_used_paths;
        self
    }

    /// Selects or disables the persistent frontend cache root.
    pub fn with_frontend_cache_dir(mut self, frontend_cache_dir: Option<PathBuf>) -> Self {
        self.frontend_cache_dir = frontend_cache_dir;
        self
    }

    /// Enables semantic verification and replacement of valid-but-stale cache products.
    pub fn with_frontend_cache_verification(mut self, verify: bool) -> Self {
        self.verify_frontend_cache = verify;
        self
    }

    /// Supplies a resolved toolchain for std modules and compatibility identity.
    pub fn with_toolchain_layout(mut self, toolchain: Arc<ToolchainLayout>) -> Self {
        self.toolchain = Some(toolchain);
        self
    }
}

/// Runtime mode injected for the entry module.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default)]
pub enum EntryRuntime {
    #[default]
    /// No freestanding startup module is injected.
    None,
    /// Inject the freestanding startup module and executable entry behavior.
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
            .with_sources(sources)
            .with_toolchain_layout(tests::test_toolchain_layout()),
    )
    .expect("test program load must succeed")
}

#[cfg(test)]
fn load_program_trace(
    entry_path: impl Into<String>,
    module_map: ModuleMap,
) -> nia_query::QueryTrace {
    let entry_path = SourcePath::new(entry_path.into());
    let module_map = effective_module_map(
        &entry_path,
        module_map,
        Some(tests::test_toolchain_layout().as_ref()),
    );
    let db = QueryDb::new_registered(
        LoaderContext {
            entry_path,
            package_root: None,
            module_map,
            sources: SourceDatabase::new(),
            node_store: nia_node_id::NodeStore::new(),
            diagnostic_store: Arc::new(nia_diagnostic::DiagnosticStore::new()),
            symbols: SymbolTable::new(),
            target: TargetConfig::host(),
            entry_runtime: EntryRuntime::None,
            toolchain_identity: tests::test_toolchain_layout().identity().fingerprint(),
            package_roots_with_used_paths: HashSet::new(),
            package_root_used_paths: false,
            provider_facts: ProviderFactStore::default(),
            frontend_cache: None,
            verify_frontend_cache: false,
            provider_demand_plan_key: None,
            provider_demand_plan_candidate: Mutex::new(None),
        },
        loader_query_registry(),
    );
    let _program = db
        .get(LoadedProgramQuery)
        .expect("test program load must succeed");
    db.query_trace()
}

fn effective_module_map(
    entry_path: &SourcePath,
    module_map: ModuleMap,
    toolchain: Option<&ToolchainLayout>,
) -> ModuleMap {
    let module_map = module_map.with_entry(entry_path.clone());
    let Some(toolchain) = toolchain else {
        return module_map;
    };
    module_map.with_default_std(SourcePath::with_identity(
        toolchain.std_module().to_string_lossy().into_owned(),
        "toolchain:/std.nia",
    ))
}

pub(crate) struct LoaderContext {
    pub(crate) entry_path: SourcePath,
    pub(crate) package_root: Option<SourcePath>,
    pub(crate) module_map: ModuleMap,
    pub(crate) sources: SourceDatabase,
    pub(crate) node_store: nia_node_id::NodeStore,
    pub(crate) diagnostic_store: Arc<nia_diagnostic::DiagnosticStore>,
    pub(crate) symbols: SymbolTable,
    pub(crate) target: TargetConfig,
    pub(crate) entry_runtime: EntryRuntime,
    pub(crate) toolchain_identity: nia_toolchain::ToolchainIdentityFingerprint,
    pub(crate) package_roots_with_used_paths: HashSet<nia_symbol::SymbolId>,
    pub(crate) package_root_used_paths: bool,
    pub(crate) provider_facts: ProviderFactStore,
    pub(crate) frontend_cache: Option<Arc<frontend_cache::PersistentFrontendCache>>,
    pub(crate) verify_frontend_cache: bool,
    pub(crate) provider_demand_plan_key: Option<FrontendProviderDemandPlanCacheKey>,
    pub(crate) provider_demand_plan_candidate: Mutex<Option<HashSet<ProviderDemand>>>,
}

impl LoaderContext {
    pub(crate) fn frontend_cache_namespace(&self) -> FrontendCacheNamespace {
        FrontendCacheNamespace::for_toolchain(
            &self.target,
            queries::runtime_model(self.entry_runtime),
            self.toolchain_identity,
        )
    }
}
