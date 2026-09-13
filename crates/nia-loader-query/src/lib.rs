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
mod package_artifact;
mod provider_facts;
mod provider_loading;
mod queries;
mod used_paths;

#[cfg(test)]
mod tests;

use nia_compiler_query::{
    FrontendCacheNamespace, FrontendProgramSourceFingerprint, FrontendProviderDemandPlanCacheKey,
    LoadedProgram, LoaderFactProvider, ProviderDemand, ProviderRequest, SourceContentFingerprint,
    frontend_module_map_fingerprint_with_package_root, frontend_program_source_fingerprint,
    source_content_fingerprint,
};
use nia_imports::{ModuleMap, StableModuleKey};
use nia_package_metadata::PackageId;
use nia_query::{QueryDb, QueryResult, QueryRetirement, QuerySession};
use nia_source::{SourceDatabase, SourceFile, SourcePath, SourceRevision, SourceVersion};
use nia_symbol::ToSymbolId;
use nia_symbol_table::SymbolTable;
use nia_target_config::{BuildProfile, CompilationMode, TargetConfig};
use nia_toolchain::ToolchainLayout;
use nia_toolchain::{RuntimeSpec, SourceRuntimeSpec};
use provider_facts::{ProviderDemandsQuery, ProviderFactStore};
use queries::{LoadedProgramQuery, SourceTextQuery};
use std::{
    collections::HashSet,
    io,
    path::PathBuf,
    sync::{Arc, Mutex},
};

/// Returns the synthetic, stable package-root identity for toolchain runtime
/// resources. The root is graph-owned and source-backed with an empty facade;
/// only its injected `start` child is a real runtime source module.
pub(crate) fn runtime_start_module_path(runtime: &SourceRuntimeSpec) -> SourcePath {
    SourcePath::with_identity(
        runtime.start_module().to_string_lossy().into_owned(),
        runtime.start_module_identity(),
    )
}

pub(crate) fn runtime_package_root_path(runtime: &SourceRuntimeSpec) -> SourcePath {
    let physical = runtime
        .start_module()
        .to_string_lossy()
        .rsplit_once('/')
        .map(|(parent, _)| format!("{parent}/pkg.nia"))
        .unwrap_or_else(|| "runtime/pkg.nia".to_owned());
    SourcePath::with_identity(physical, runtime.package_root_identity())
}

pub use nia_package_metadata::CompiledPackageInterface;
pub use package_artifact::{
    PackageArtifactError, PackageArtifactFallback, PackageArtifactLoad, PackageArtifactMismatch,
    PackageArtifactRequest, package_artifact_path, select_package_artifact,
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
    load_program_with_map_and_runtime(entry_path, module_map, RuntimeSpec::Bare, toolchain)
}

/// Loads a program with mappings and an explicit validated runtime.
pub fn load_program_with_map_and_runtime(
    entry_path: impl Into<String>,
    module_map: ModuleMap,
    runtime: RuntimeSpec,
    toolchain: Arc<ToolchainLayout>,
) -> QueryResult<LoadedProgram> {
    load_program_request(
        LoadRequest::new(entry_path)
            .with_module_map(module_map)
            .with_runtime(runtime)
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
    package_artifact: Option<PackageArtifactRequest>,
    expected_package: Option<PackageId>,
    artifact_compatibility: package_artifact::ArtifactCompatibility,
    required_native_optimization: Option<u8>,
    artifact_selection_cache: Arc<Mutex<Option<CachedPackageArtifact>>>,
}

#[derive(Clone)]
struct CachedPackageArtifact {
    stamp: Option<ArtifactFileStamp>,
    selection: Result<Option<PackageArtifactLoad>, PackageArtifactError>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct ArtifactFileStamp {
    length: u64,
    modified_nanos: u128,
}

fn artifact_file_stamp(path: &std::path::Path) -> io::Result<Option<ArtifactFileStamp>> {
    match std::fs::metadata(path) {
        Ok(metadata) => {
            let modified_nanos = metadata
                .modified()?
                .duration_since(std::time::UNIX_EPOCH)
                .map_err(io::Error::other)?
                .as_nanos();
            Ok(Some(ArtifactFileStamp {
                length: metadata.len(),
                modified_nanos,
            }))
        }
        Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(None),
        Err(error) => Err(error),
    }
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
        let runtime_start_module = request.runtime.source().map(runtime_start_module_path);
        let toolchain_std_artifact = request
            .discover_toolchain_std_artifact
            .then_some(request.toolchain.as_ref())
            .flatten()
            .map(|toolchain| {
                toolchain.std_package_artifact(
                    &request.target,
                    request.profile,
                    request.compilation_mode,
                )
            })
            .filter(|path| path.is_file());
        let auto_std_artifact =
            request.package_artifact.is_none() && toolchain_std_artifact.is_some();
        let explicit_std_artifact = request
            .package_artifact
            .as_ref()
            .zip(toolchain_std_artifact.as_ref())
            .is_some_and(|(request, path)| request.path() == path);
        let std_artifact_requested = auto_std_artifact || explicit_std_artifact;
        let expected_package = request.expected_package.clone().or_else(|| {
            std_artifact_requested.then(|| {
                request
                    .toolchain
                    .as_ref()
                    .expect("std artifact has toolchain")
                    .std_package_id()
            })
        });
        let artifact_compatibility = package_artifact::ArtifactCompatibility::current(
            request.toolchain.as_deref(),
            &request.target,
            request.profile,
            request.compilation_mode,
        );
        let selected_std_interfaces = if std_artifact_requested {
            request.toolchain.as_ref().and_then(|toolchain| {
                let artifact_request = request.package_artifact.clone().unwrap_or_else(|| {
                    PackageArtifactRequest::Optional(toolchain.std_package_artifact(
                        &request.target,
                        request.profile,
                        request.compilation_mode,
                    ))
                });
                match package_artifact::load(
                    &artifact_request,
                    expected_package.as_ref(),
                    &artifact_compatibility,
                    request.required_native_optimization,
                ) {
                    Ok(PackageArtifactLoad::Loaded { interface, .. }) => Some(interface),
                    _ => None,
                }
            })
        } else {
            None
        };
        let selected_std_modules = selected_std_interfaces
            .as_ref()
            .map(|interface| {
                interface
                    .module_identities()
                    .filter(|identity| {
                        expected_package
                            .as_ref()
                            .is_some_and(|package| &identity.package == package)
                    })
                    .collect::<Vec<_>>()
            })
            .unwrap_or_default();
        let std_artifact_root = selected_std_modules
            .iter()
            .map(|module| module.path.as_str())
            .find(|path| path.rsplit('/').next() == Some("pkg.nia"))
            .map(str::to_owned);
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
            std_artifact_root.as_deref(),
        );
        let sources = request.sources;
        if runtime_start_module.is_some() {
            // The runtime package root is a synthetic facade used only for
            // graph ownership. Keep it source-backed so normal query loading
            // does not manufacture a missing-file diagnostic.
            let runtime = request
                .runtime
                .source()
                .expect("runtime start requires source runtime");
            sources.set_source(runtime_package_root_path(runtime), "");
        }
        for module in &selected_std_modules {
            sources.set_source(
                SourcePath::with_identity(
                    format!("/__nia_artifact__/std/{}", module.path),
                    &module.path,
                ),
                "",
            );
        }
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
            request.runtime.clone(),
            toolchain_identity,
        );
        let runtime_package_root = request.runtime.source().map(runtime_package_root_path);
        let source_roots = std::iter::once(entry_path.clone())
            .chain(request.package_root.clone())
            .chain(module_map.entries().map(|(_, path)| path.clone()))
            .chain(runtime_package_root.clone())
            .chain(runtime_start_module.clone())
            .collect::<Vec<_>>();
        let module_map_fingerprint = frontend_module_map_fingerprint_with_package_root(
            &module_map,
            request
                .package_root
                .as_ref()
                .map(SourcePath::identity)
                .as_ref(),
        );
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
                compiled_package_modules: Arc::new(selected_std_modules.clone()),
                compiled_package_interfaces: selected_std_interfaces
                    .map(|interface| Arc::new(vec![interface]))
                    .unwrap_or_default(),
                node_store: nia_node_id::NodeStore::new(),
                diagnostic_store: Arc::new(nia_diagnostic::DiagnosticStore::new()),
                symbols,
                target: request.target,
                profile: request.profile,
                compilation_mode: request.compilation_mode,
                runtime: request.runtime,
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
        Self {
            db,
            sources,
            package_artifact: request
                .package_artifact
                .or_else(|| toolchain_std_artifact.map(PackageArtifactRequest::Optional)),
            expected_package,
            artifact_compatibility,
            required_native_optimization: request.required_native_optimization,
            artifact_selection_cache: Arc::new(Mutex::new(None)),
        }
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

    /// Selects the explicitly requested compiled package, if any.
    ///
    /// Source loading remains the default. An optional artifact returns
    /// [`PackageArtifactLoad::SourceFallback`] when absent, corrupt, or
    /// incompatible; a required artifact returns a typed error instead.
    pub fn package_artifact(&self) -> Result<Option<PackageArtifactLoad>, PackageArtifactError> {
        let Some(request) = self.package_artifact.as_ref() else {
            return Ok(None);
        };
        let stamp = artifact_file_stamp(request.path()).ok();
        if let Some(stamp) = stamp {
            if let Some(cached) = self
                .artifact_selection_cache
                .lock()
                .expect("loader artifact selection cache lock poisoned")
                .as_ref()
                .filter(|cached| cached.stamp == stamp)
            {
                return cached.selection.clone();
            }
        }
        let selection = package_artifact::load(
            request,
            self.expected_package.as_ref(),
            &self.artifact_compatibility,
            self.required_native_optimization,
        )
        .map(Some)?;
        if let Ok(stamp) = artifact_file_stamp(request.path()) {
            *self
                .artifact_selection_cache
                .lock()
                .expect("loader artifact selection cache lock poisoned") =
                Some(CachedPackageArtifact {
                    stamp,
                    selection: Ok(selection.clone()),
                });
        }
        Ok(selection)
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
        let module_map = frontend_module_map_fingerprint_with_package_root(
            &context.module_map,
            context
                .package_root
                .as_ref()
                .map(SourcePath::identity)
                .as_ref(),
        );
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
        if self
            .db
            .context()
            .compiled_package_modules
            .iter()
            .any(|identity| identity.path == module.path.identity().normalized_path())
        {
            return Ok(Some(self.sources.empty_source(&module.path).version()));
        }
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
        if let Some(identity) = self
            .db
            .context()
            .compiled_package_modules
            .iter()
            .find(|identity| identity.path == module.path.identity().normalized_path())
        {
            if let Some(interface) = self
                .db
                .context()
                .compiled_package_interfaces
                .iter()
                .find(|interface| interface.manifest().package == identity.package)
            {
                return Ok(Some(compiled_provider_summary(
                    interface,
                    identity,
                    &self.db.context().symbols,
                )?));
            }
        }
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

    fn profile(&self) -> BuildProfile {
        self.db.context().profile
    }

    fn compilation_mode(&self) -> CompilationMode {
        self.db.context().compilation_mode
    }

    fn runtime(&self) -> nia_compiler_query::RuntimeSpec {
        self.db.context().runtime.clone()
    }

    fn toolchain_identity(&self) -> nia_toolchain::ToolchainIdentityFingerprint {
        self.db.context().toolchain_identity
    }

    fn compiled_package_interfaces(
        &self,
    ) -> QueryResult<Vec<nia_package_metadata::CompiledPackageInterface>> {
        let selection = self.package_artifact().map_err(|error| {
            self.db
                .invalid_input(&queries::LoadedProgramQuery, error.to_string())
        })?;
        Ok(selection
            .and_then(|selection| match selection {
                PackageArtifactLoad::Loaded { interface, .. } => Some(interface),
                PackageArtifactLoad::SourceFallback { .. } => None,
            })
            .into_iter()
            .collect())
    }

    fn compiled_package_module_identity(
        &self,
        module_id: nia_imports::ModuleId,
    ) -> QueryResult<Option<nia_package_metadata::ModuleId>> {
        let graph = self.db.get(graph::ModuleGraphQuery)?;
        let Some(module) = graph.semantic.get(module_id) else {
            return Ok(None);
        };
        let path = module.path.identity().normalized_path().to_owned();
        if let Some(identity) = self
            .db
            .context()
            .compiled_package_modules
            .iter()
            .find(|identity| identity.path == path)
        {
            return Ok(Some(identity.clone()));
        }
        // Source-backed package modules remain authoritative until the
        // artifact publishes the complete public-surface projection (including
        // re-export directives). Artifact facts are therefore used only for
        // source-free modules.
        let source_id = self.sources.id_for_path(&module.path);
        if self
            .db
            .get(queries::SourceTextQuery(source_id))?
            .file
            .is_some()
        {
            return Ok(None);
        }
        let entry_root = graph.semantic.current_package_root(graph.semantic.entry());
        if entry_root != graph.semantic.current_package_root(module_id) {
            return Ok(None);
        }
        let mut matches = self
            .compiled_package_interfaces()?
            .into_iter()
            .flat_map(|interface| interface.module_identities().collect::<Vec<_>>())
            .filter(|identity| identity.path == path)
            .collect::<Vec<_>>();
        matches.sort();
        matches.dedup();
        match matches.as_slice() {
            [] => Ok(None),
            [identity] => Ok(Some(identity.clone())),
            _ => Err(self.db.invalid_input(
                &queries::LoadedProgramQuery,
                format!("compiled artifacts claim multiple owners for module `{path}`"),
            )),
        }
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
    /// Build profile used for profile-conditional frontend selection.
    pub profile: BuildProfile,
    /// Whether test-only source participates in frontend selection.
    pub compilation_mode: CompilationMode,
    /// Validated runtime startup selection.
    pub runtime: RuntimeSpec,
    /// Whether package-root `using` paths participate in the source manifest.
    pub package_root_used_paths: bool,
    /// Optional persistent frontend cache root.
    pub frontend_cache_dir: Option<PathBuf>,
    /// Whether semantically valid cache products must be recomputed and verified.
    pub verify_frontend_cache: bool,
    /// Optional resolved toolchain supplying std modules and identity.
    pub toolchain: Option<Arc<ToolchainLayout>>,
    /// Optional compiled package artifact selection request.
    pub package_artifact: Option<PackageArtifactRequest>,
    /// Optional stable package identity expected from the selected artifact.
    pub expected_package: Option<PackageId>,
    /// Native optimization variant required before selecting an artifact.
    /// Semantic-only requests leave this unset.
    pub required_native_optimization: Option<u8>,
    /// Whether the resolved toolchain's standard-library artifact is probed.
    pub discover_toolchain_std_artifact: bool,
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
            profile: BuildProfile::default(),
            compilation_mode: CompilationMode::default(),
            runtime: RuntimeSpec::Bare,
            package_root_used_paths: false,
            frontend_cache_dir: None,
            verify_frontend_cache: false,
            toolchain: None,
            package_artifact: None,
            expected_package: None,
            required_native_optimization: None,
            discover_toolchain_std_artifact: true,
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

    /// Selects the build profile used by conditional source selection.
    pub fn with_profile(mut self, profile: BuildProfile) -> Self {
        self.profile = profile;
        self
    }

    /// Selects whether test-only source participates in compilation.
    pub fn with_compilation_mode(mut self, mode: CompilationMode) -> Self {
        self.compilation_mode = mode;
        self
    }

    /// Requires an exact native optimization variant from a selected artifact.
    pub fn with_required_native_optimization(mut self, optimization: u8) -> Self {
        self.required_native_optimization = Some(optimization);
        self
    }

    /// Enables or disables automatic toolchain standard-library artifacts.
    pub fn with_toolchain_std_artifact_discovery(mut self, enabled: bool) -> Self {
        self.discover_toolchain_std_artifact = enabled;
        self
    }

    /// Selects a validated runtime startup specification.
    pub fn with_runtime(mut self, runtime: RuntimeSpec) -> Self {
        self.runtime = runtime;
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

    /// Tries a compiled package artifact and falls back to source on rejection.
    pub fn with_package_artifact(mut self, path: impl Into<PathBuf>) -> Self {
        self.package_artifact = Some(PackageArtifactRequest::Optional(path.into()));
        self
    }

    /// Requires a compiled package artifact and surfaces rejection as an error.
    pub fn require_package_artifact(mut self, path: impl Into<PathBuf>) -> Self {
        self.package_artifact = Some(PackageArtifactRequest::Required(path.into()));
        self
    }

    /// Checks the selected artifact against a stable package identity.
    pub fn with_expected_package(mut self, package: PackageId) -> Self {
        self.expected_package = Some(package);
        self
    }
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
        None,
    );
    let db = QueryDb::new_registered(
        LoaderContext {
            entry_path,
            package_root: None,
            module_map,
            sources: SourceDatabase::new(),
            compiled_package_modules: Arc::new(Vec::new()),
            compiled_package_interfaces: Arc::new(Vec::new()),
            node_store: nia_node_id::NodeStore::new(),
            diagnostic_store: Arc::new(nia_diagnostic::DiagnosticStore::new()),
            symbols: SymbolTable::new(),
            target: TargetConfig::host(),
            profile: BuildProfile::default(),
            compilation_mode: CompilationMode::default(),
            runtime: RuntimeSpec::Bare,
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
    std_artifact_root: Option<&str>,
) -> ModuleMap {
    let module_map = module_map.with_entry(entry_path.clone());
    let Some(toolchain) = toolchain else {
        return module_map;
    };
    let std_path = std_artifact_root.map_or_else(
        || {
            SourcePath::with_identity(
                toolchain.std_module().to_string_lossy().into_owned(),
                "toolchain:/std/pkg.nia",
            )
        },
        |identity| SourcePath::with_identity(format!("/__nia_artifact__/std/{identity}"), identity),
    );
    module_map.with_default_std(std_path)
}

pub(crate) struct LoaderContext {
    pub(crate) entry_path: SourcePath,
    pub(crate) package_root: Option<SourcePath>,
    pub(crate) module_map: ModuleMap,
    pub(crate) sources: SourceDatabase,
    pub(crate) compiled_package_modules: Arc<Vec<nia_package_metadata::ModuleId>>,
    pub(crate) compiled_package_interfaces:
        Arc<Vec<nia_package_metadata::CompiledPackageInterface>>,
    pub(crate) node_store: nia_node_id::NodeStore,
    pub(crate) diagnostic_store: Arc<nia_diagnostic::DiagnosticStore>,
    pub(crate) symbols: SymbolTable,
    pub(crate) target: TargetConfig,
    pub(crate) profile: BuildProfile,
    pub(crate) compilation_mode: CompilationMode,
    pub(crate) runtime: RuntimeSpec,
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
    pub(crate) fn compiled_provider_modules_for_demand(
        &self,
        request: &ProviderRequest,
    ) -> Vec<nia_package_metadata::ModuleId> {
        self.compiled_package_modules
            .iter()
            .filter(|module| {
                let Some(interface) = self
                    .compiled_package_interfaces
                    .iter()
                    .find(|interface| interface.manifest().package == module.package)
                else {
                    return false;
                };
                let Some(summary) =
                    compiled_provider_summary(interface, module, &self.symbols).ok()
                else {
                    return false;
                };
                match request {
                    ProviderRequest::TraitImpl {
                        target_type_name,
                        trait_name,
                    } => summary.defines_trait_impl(target_type_name.as_ref(), trait_name, None),
                    ProviderRequest::Method {
                        target_type_name,
                        method_name,
                    } => target_type_name.as_ref().is_some_and(|target| {
                        summary.defines_public_extension_method_for_facade(
                            |_| true,
                            Some(target),
                            method_name,
                        )
                    }),
                    ProviderRequest::ModuleSemantic { .. } | ProviderRequest::ModuleBody { .. } => {
                        false
                    }
                }
            })
            .cloned()
            .collect()
    }

    fn compiled_provider_summary_for_path(
        &self,
        path: &SourcePath,
    ) -> Option<nia_provider_summary::ProviderSummary> {
        let path_identity = path.identity();
        let identity = path_identity.normalized_path();
        let module = self
            .compiled_package_modules
            .iter()
            .find(|module| identity == module.path)?;
        let interface = self
            .compiled_package_interfaces
            .iter()
            .find(|interface| interface.manifest().package == module.package)?;
        compiled_provider_summary(interface, module, &self.symbols).ok()
    }

    pub(crate) fn frontend_cache_namespace(&self) -> FrontendCacheNamespace {
        FrontendCacheNamespace::for_toolchain_with_profile_and_mode(
            &self.target,
            self.runtime.clone(),
            self.profile,
            self.compilation_mode,
            self.toolchain_identity,
        )
    }
}

fn compiled_provider_summary(
    interface: &nia_package_metadata::CompiledPackageInterface,
    module: &nia_package_metadata::ModuleId,
    symbols: &SymbolTable,
) -> QueryResult<nia_provider_summary::ProviderSummary> {
    let Some(signatures) = interface.signatures() else {
        return Ok(nia_provider_summary::ProviderSummary::default());
    };
    let Some(graph) = interface.type_graph() else {
        return Ok(nia_provider_summary::ProviderSummary::default());
    };
    let type_ref = |root: u32| -> nia_provider_summary::ProviderTypeRef {
        match graph.nodes.get(root as usize) {
            Some(nia_package_metadata::StableTypeNode::Named(definition))
            | Some(nia_package_metadata::StableTypeNode::NamedApplied { definition, .. }) => {
                let name = symbols.intern(&definition.name).ok();
                nia_provider_summary::ProviderTypeRef {
                    last_name: name,
                    is_generic_or_structural_target: false,
                    semantic_is_conservative: false,
                }
            }
            Some(nia_package_metadata::StableTypeNode::BuiltinTrait { trait_id, .. }) => {
                let name = nia_ids::BuiltinTrait::from_stable_tag(*trait_id as u32)
                    .map(|builtin| builtin.symbol_id());
                nia_provider_summary::ProviderTypeRef {
                    last_name: name,
                    is_generic_or_structural_target: false,
                    semantic_is_conservative: false,
                }
            }
            _ => nia_provider_summary::ProviderTypeRef {
                last_name: None,
                is_generic_or_structural_target: true,
                semantic_is_conservative: true,
            },
        }
    };
    let mut providers = Vec::new();
    for extension in signatures
        .extensions
        .iter()
        .filter(|extension| extension.module == *module)
    {
        let mut associated_methods = extension
            .members
            .iter()
            .filter(|member| member.kind == 11 || member.kind == 12)
            .filter_map(|member| symbols.intern(&member.name).ok())
            .collect::<std::collections::BTreeSet<_>>();
        if let Some(trait_definition) =
            extension
                .trait_root
                .and_then(|root| match graph.nodes.get(root as usize) {
                    Some(nia_package_metadata::StableTypeNode::Named(definition))
                    | Some(nia_package_metadata::StableTypeNode::NamedApplied {
                        definition, ..
                    }) => Some(definition),
                    _ => None,
                })
            && let Some(trait_record) = signatures
                .traits
                .iter()
                .find(|record| &record.definition == trait_definition)
        {
            associated_methods.extend(
                trait_record
                    .members
                    .iter()
                    .filter(|member| {
                        member.kind == 11
                            && member.flags & nia_package_metadata::SIGNATURE_FLAG_HAS_BODY != 0
                    })
                    .filter_map(|member| symbols.intern(&member.name).ok()),
            );
        }
        let associated_values = extension
            .members
            .iter()
            .filter(|member| member.kind == 4)
            .filter_map(|member| symbols.intern(&member.name).ok())
            .collect();
        providers.push(nia_provider_summary::Provider {
            target: nia_provider_summary::ProviderTarget {
                ty: type_ref(extension.target_root),
            },
            trait_ref: extension.trait_root.map(type_ref),
            associated_methods: associated_methods.into_iter().collect(),
            associated_values,
        });
    }
    Ok(nia_provider_summary::ProviderSummary::from_providers(
        providers,
    ))
}
