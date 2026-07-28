use super::*;
use crate::provider_facts::{ProviderDemandsQuery, ProviderFactStore};
use crate::queries::{
    ActiveModuleItemTreeFactQuery, LoadedModuleQuery, ModuleDeclarationsQuery,
    ModuleFacadeFactsQuery, ModuleItemTreeFactQuery, ModuleOriginsFactQuery,
    ModuleParseErrorsFactQuery, ParsedModuleQuery, ProviderSummaryQuery,
    PublicSurfaceModuleFactsQuery, SourceStatus, SourceStatusQuery, SourceTextQuery,
    SyntaxModuleQuery, module_declarations_query as fallible_module_declarations_query,
    module_facade_facts_query as fallible_module_facade_facts_query,
    parsed_module_query as fallible_parsed_module_query,
    provider_summary_query as fallible_provider_summary_query,
    public_surface_module_facts_query as fallible_public_surface_module_facts_query,
};
use nia_compiler_query::{
    CompileRequest, CompilerDatabase, FrontendCacheNamespace, FrontendFacadeFactsCacheKey,
    FrontendModuleDependenciesCacheKey, FrontendModuleMapFingerprint,
    FrontendProviderSummaryCacheKey, FrontendPublicSurfaceFactsCacheKey, FrontendSourceCacheKey,
    ItemSignatureFingerprint, ProviderDemand, ProviderGraphUpdate, RuntimeModel,
    SourceContentFingerprint, frontend_module_map_fingerprint, has_error_diagnostics,
    item_signature_fingerprint, source_content_fingerprint,
};
use nia_imports::{ModuleGraph, ModuleNode, StableModuleKey, Visibility};
use nia_item_tree::{ItemTreeNodeKind, ModuleItemTree};
use nia_source::SourceId;
use nia_symbol::{SymbolId, stable_hash};
use nia_symbol_table::SymbolTable;
use std::{
    collections::{HashMap, HashSet},
    fs,
    path::{Path, PathBuf},
    sync::{
        Arc,
        atomic::{AtomicUsize, Ordering},
    },
};

trait QueryDbTestExt<C> {
    fn expect_get<K>(&self, key: K) -> Arc<K::Value>
    where
        K: nia_query::QueryKey<C>;
}

impl<C> QueryDbTestExt<C> for QueryDb<C> {
    fn expect_get<K>(&self, key: K) -> Arc<K::Value>
    where
        K: nia_query::QueryKey<C>,
    {
        self.get(key).expect("test query must succeed")
    }
}

fn load_program(entry_path: impl Into<String>) -> LoadedProgram {
    super::load_program(entry_path).expect("test program load must succeed")
}

fn load_program_with_map(entry_path: impl Into<String>, module_map: ModuleMap) -> LoadedProgram {
    super::load_program_with_map(entry_path, module_map).expect("test program load must succeed")
}

fn load_program_with_map_and_entry_runtime(
    entry_path: impl Into<String>,
    module_map: ModuleMap,
    entry_runtime: EntryRuntime,
) -> LoadedProgram {
    super::load_program_with_map_and_entry_runtime(entry_path, module_map, entry_runtime)
        .expect("test program load must succeed")
}

fn parsed_module_query(db: &QueryDb<LoaderContext>, path: &SourcePath) -> ParsedModuleQuery {
    fallible_parsed_module_query(db, path).expect("test source path must be registered")
}

fn module_declarations_query(
    db: &QueryDb<LoaderContext>,
    path: &SourcePath,
) -> ModuleDeclarationsQuery {
    fallible_module_declarations_query(db, path).expect("test source path must be registered")
}

fn public_surface_module_facts_query(
    db: &QueryDb<LoaderContext>,
    path: &SourcePath,
) -> PublicSurfaceModuleFactsQuery {
    fallible_public_surface_module_facts_query(db, path)
        .expect("test source path must be registered")
}

fn provider_summary_query(db: &QueryDb<LoaderContext>, path: &SourcePath) -> ProviderSummaryQuery {
    fallible_provider_summary_query(db, path).expect("test source path must be registered")
}

fn module_facade_facts_query(
    db: &QueryDb<LoaderContext>,
    path: &SourcePath,
) -> ModuleFacadeFactsQuery {
    fallible_module_facade_facts_query(db, path).expect("test source path must be registered")
}

static TEMP_DIR_COUNTER: AtomicUsize = AtomicUsize::new(0);

fn sym(text: &str) -> SymbolId {
    SymbolId::from_stable_hash(stable_hash(text))
}

fn symbols_for(texts: &[&str]) -> SymbolTable {
    let symbols = SymbolTable::new();
    for text in texts {
        symbols.intern(text).expect("test symbols must not collide");
    }
    symbols
}

#[path = "tests/facade_orchestration.rs"]
mod facade_orchestration;
#[path = "tests/facade_persistence.rs"]
mod facade_persistence;
#[path = "tests/freestanding_runtime.rs"]
mod freestanding_runtime;
#[path = "tests/loader_query_boundaries.rs"]
mod loader_query_boundaries;
#[path = "tests/loader_query_contracts.rs"]
mod loader_query_contracts;
#[path = "tests/module_dependency_roundtrip.rs"]
mod module_dependency_roundtrip;
#[path = "tests/module_dependency_verification.rs"]
mod module_dependency_verification;
#[path = "tests/module_discovery.rs"]
mod module_discovery;
#[path = "tests/module_map_loading.rs"]
mod module_map_loading;
#[path = "tests/package_provider_loading.rs"]
mod package_provider_loading;
#[path = "tests/persistent_module_dependencies.rs"]
mod persistent_module_dependencies;
#[path = "tests/persistent_semantic_products.rs"]
mod persistent_semantic_products;
#[path = "tests/provider_demand_plan.rs"]
mod provider_demand_plan;
#[path = "tests/provider_graph_transitions.rs"]
mod provider_graph_transitions;
#[path = "tests/provider_summary_persistence.rs"]
mod provider_summary_persistence;
#[path = "tests/provider_summary_revisions.rs"]
mod provider_summary_revisions;
#[path = "tests/public_surface_persistence.rs"]
mod public_surface_persistence;
#[path = "tests/public_surface_roundtrip.rs"]
mod public_surface_roundtrip;
#[path = "tests/query_key_contracts.rs"]
mod query_key_contracts;
#[path = "tests/query_observability.rs"]
mod query_observability;
#[path = "tests/revision_invalidation.rs"]
mod revision_invalidation;
#[path = "tests/source_resolution.rs"]
mod source_resolution;
#[path = "tests/std_facade_reexports.rs"]
mod std_facade_reexports;
#[path = "tests/std_import_usage.rs"]
mod std_import_usage;
#[path = "tests/std_provider_loading.rs"]
mod std_provider_loading;
#[path = "tests/std_reexport_resolution.rs"]
mod std_reexport_resolution;
fn test_loader_context(
    entry_path: SourcePath,
    module_map: ModuleMap,
    sources: SourceDatabase,
) -> LoaderContext {
    LoaderContext {
        entry_path: entry_path.clone(),
        module_map: effective_module_map(&entry_path, module_map),
        sources,
        node_store: nia_node_id::NodeStore::new(),
        symbols: SymbolTable::new(),
        target: TargetConfig::host(),
        entry_runtime: EntryRuntime::None,
        package_roots_with_used_paths: HashSet::new(),
        package_root_used_paths: false,
        provider_facts: ProviderFactStore::default(),
        frontend_cache: None,
        verify_frontend_cache: false,
        provider_demand_plan_key: None,
        provider_demand_plan_candidate: std::sync::Mutex::new(None),
    }
}

fn registered_query_db(context: LoaderContext) -> QueryDb<LoaderContext> {
    QueryDb::new_registered(context, crate::loader_query_registry())
}

fn query_executions(trace: &nia_query::QueryTrace, name: &str) -> usize {
    trace
        .queries
        .iter()
        .filter(|query| query.frame.name == name)
        .map(|query| query.stats.executions)
        .sum()
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
enum SemanticFieldParentKind {
    Declaration,
    FunctionSignature,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
struct SemanticFieldParent(SourceId, SemanticFieldParentKind);

impl nia_query::QueryKey<LoaderContext> for SemanticFieldParent {
    type Value = usize;

    const FINGERPRINT: nia_query::QueryFingerprintPolicy =
        nia_query::QueryFingerprintPolicy::StableValue;

    fn name() -> &'static str {
        "semantic_field_parent"
    }

    fn execute_result(&self, db: &QueryDb<LoaderContext>) -> nia_query::QueryResult<Self::Value> {
        Ok(match self.1 {
            SemanticFieldParentKind::Declaration => {
                db.expect_get(ModuleItemTreeFactQuery(self.0)).items.len()
            }
            SemanticFieldParentKind::FunctionSignature => db
                .get(ActiveModuleItemTreeFactQuery(
                    self.0,
                    nia_compiler_query::ActiveModuleItemTreeFactKind::Signature(
                        nia_item_tree::SignatureItemSet::Functions,
                    ),
                ))?
                .items
                .len(),
        })
    }

    fn fingerprint(&self, value: &Self::Value) -> Option<nia_query::QueryFingerprint> {
        let mut builder =
            nia_query::QueryFingerprintBuilder::new("nia.loader.test.semantic-field-parent.v1");
        builder.write_u64(*value as u64);
        Some(builder.finish())
    }
}

fn assert_no_error_diagnostics(program: &nia_compiler_query::LoadedProgram) {
    assert!(
        !has_error_diagnostics(&program.diagnostics),
        "{:?}",
        program.diagnostics
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
    if dir.exists() {
        fs::remove_dir_all(&dir).expect("remove stale temp dir");
    }
    fs::create_dir_all(&dir).expect("create temp dir");
    dir
}

fn provider_summary_database(
    main: &SourcePath,
    sources: &SourceDatabase,
    cache: Arc<crate::frontend_cache::PersistentFrontendCache>,
    verify: bool,
) -> QueryDb<LoaderContext> {
    frontend_cache_database(main, sources, ModuleMap::default(), cache, verify)
}

fn frontend_cache_database(
    main: &SourcePath,
    sources: &SourceDatabase,
    module_map: ModuleMap,
    cache: Arc<crate::frontend_cache::PersistentFrontendCache>,
    verify: bool,
) -> QueryDb<LoaderContext> {
    let mut context = test_loader_context(main.clone(), module_map, sources.clone());
    context.frontend_cache = Some(cache);
    context.verify_frontend_cache = verify;
    registered_query_db(context)
}

struct ProviderCacheIdentity {
    namespace: FrontendCacheNamespace,
    module: StableModuleKey,
    source: SourceContentFingerprint,
    source_key: FrontendSourceCacheKey,
    item_signature: ItemSignatureFingerprint,
    provider_key: FrontendProviderSummaryCacheKey,
}

struct FacadeCacheIdentity {
    namespace: FrontendCacheNamespace,
    module: StableModuleKey,
    source: SourceContentFingerprint,
    source_key: FrontendSourceCacheKey,
    item_signature: ItemSignatureFingerprint,
    module_map: FrontendModuleMapFingerprint,
    facade_key: FrontendFacadeFactsCacheKey,
}

struct ModuleDependenciesCacheIdentity {
    namespace: FrontendCacheNamespace,
    module: StableModuleKey,
    source: SourceContentFingerprint,
    source_len: usize,
    module_map: FrontendModuleMapFingerprint,
    key: FrontendModuleDependenciesCacheKey,
}

struct PublicSurfaceFactsCacheIdentity {
    namespace: FrontendCacheNamespace,
    module: StableModuleKey,
    source: SourceContentFingerprint,
    source_len: usize,
    key: FrontendPublicSurfaceFactsCacheKey,
}

fn provider_cache_identity(file: &SourceFile) -> ProviderCacheIdentity {
    let namespace = FrontendCacheNamespace::new(&TargetConfig::host(), RuntimeModel::Bare);
    let module = StableModuleKey::from_source_identity(file.path.identity());
    let source = source_content_fingerprint(&file.text);
    let source_key = FrontendSourceCacheKey::new(namespace, &module, source);
    let syntax = nia_syntax::parse_source(&file.text, Some(file.version()));
    let (raw_module, _, _) = nia_parser::parse_module_syntax_with_node_store_and_symbols(
        &syntax,
        &nia_node_id::NodeStore::new(),
        SymbolTable::new(),
    );
    let item_tree = ModuleItemTree::from_module(&raw_module);
    let item_signature = item_signature_fingerprint(&syntax, &item_tree);
    let provider_key = FrontendProviderSummaryCacheKey::new(namespace, &module, item_signature);
    ProviderCacheIdentity {
        namespace,
        module,
        source,
        source_key,
        item_signature,
        provider_key,
    }
}

fn facade_cache_identity(
    file: &SourceFile,
    entry_path: &SourcePath,
    module_map: &ModuleMap,
) -> FacadeCacheIdentity {
    let provider = provider_cache_identity(file);
    let effective_module_map = effective_module_map(entry_path, module_map.clone());
    let module_map = frontend_module_map_fingerprint(&effective_module_map);
    let facade_key = FrontendFacadeFactsCacheKey::new(
        provider.namespace,
        &provider.module,
        provider.item_signature,
        module_map,
    );
    FacadeCacheIdentity {
        namespace: provider.namespace,
        module: provider.module,
        source: provider.source,
        source_key: provider.source_key,
        item_signature: provider.item_signature,
        module_map,
        facade_key,
    }
}

fn module_dependencies_cache_identity(
    file: &SourceFile,
    entry_path: &SourcePath,
    module_map: &ModuleMap,
) -> ModuleDependenciesCacheIdentity {
    let namespace = FrontendCacheNamespace::new(&TargetConfig::host(), RuntimeModel::Bare);
    let module = StableModuleKey::from_source_identity(file.path.identity());
    let source = source_content_fingerprint(&file.text);
    let source_len = file.text.len();
    let effective_module_map = effective_module_map(entry_path, module_map.clone());
    let module_map = frontend_module_map_fingerprint(&effective_module_map);
    let key = FrontendModuleDependenciesCacheKey::new(namespace, &module, source, module_map);
    ModuleDependenciesCacheIdentity {
        namespace,
        module,
        source,
        source_len,
        module_map,
        key,
    }
}

fn public_surface_facts_cache_identity(file: &SourceFile) -> PublicSurfaceFactsCacheIdentity {
    let namespace = FrontendCacheNamespace::new(&TargetConfig::host(), RuntimeModel::Bare);
    let module = StableModuleKey::from_source_identity(file.path.identity());
    let source = source_content_fingerprint(&file.text);
    let source_len = file.text.len();
    let key = FrontendPublicSurfaceFactsCacheKey::new(namespace, &module, source);
    PublicSurfaceFactsCacheIdentity {
        namespace,
        module,
        source,
        source_len,
        key,
    }
}

fn write(path: &Path, source: &str) {
    fs::write(path, source).expect("write source");
}

fn load_program_with_provider_demand(
    entry_path: &Path,
    module_map: ModuleMap,
    target_type_name: Option<&str>,
    method_name: &str,
) -> LoadedProgram {
    let source_path = SourcePath::new(entry_path.to_string_lossy());
    let database = LoaderDatabase::new(
        LoadRequest::new(entry_path.to_string_lossy().into_owned()).with_module_map(module_map),
    );
    let update = database
        .update_provider_demands([ProviderDemand {
            source_path,
            request: nia_compiler_query::ProviderRequest::Method {
                target_type_name: target_type_name.map(sym),
                method_name: sym(method_name),
            },
        }])
        .expect("provider graph update");
    let _ = update;
    database.load_program().expect("provider program load")
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
