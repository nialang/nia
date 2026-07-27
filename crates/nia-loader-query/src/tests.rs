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

#[path = "tests/loader_query_contracts.rs"]
mod loader_query_contracts;
#[path = "tests/module_discovery.rs"]
mod module_discovery;
#[path = "tests/module_map_loading.rs"]
mod module_map_loading;
#[path = "tests/persistent_semantic_products.rs"]
mod persistent_semantic_products;
#[path = "tests/provider_demand_plan.rs"]
mod provider_demand_plan;
#[path = "tests/revision_invalidation.rs"]
mod revision_invalidation;
#[path = "tests/std_facade_reexports.rs"]
mod std_facade_reexports;
#[path = "tests/std_import_usage.rs"]
mod std_import_usage;
#[test]
fn source_frontend_query_keys_are_compact_handles() {
    assert_eq!(std::mem::size_of::<ProviderDemandsQuery>(), 0);
    assert_eq!(
        std::mem::size_of::<crate::graph::ModuleGraphRevisionQuery>(),
        16
    );
    assert_eq!(std::mem::size_of::<SourceTextQuery>(), 4);
    assert_eq!(std::mem::size_of::<SourceStatusQuery>(), 4);
    assert_eq!(std::mem::size_of::<LoadedModuleQuery>(), 4);
    assert_eq!(std::mem::size_of::<ModuleOriginsFactQuery>(), 4);
    assert_eq!(std::mem::size_of::<ModuleParseErrorsFactQuery>(), 4);
    assert_eq!(std::mem::size_of::<ModuleItemTreeFactQuery>(), 4);
    assert_eq!(std::mem::size_of::<ActiveModuleItemTreeFactQuery>(), 8);
    assert_eq!(std::mem::size_of::<ParsedModuleQuery>(), 16);
    assert_eq!(std::mem::size_of::<SyntaxModuleQuery>(), 16);
    assert_eq!(std::mem::size_of::<ModuleDeclarationsQuery>(), 16);
    assert_eq!(std::mem::size_of::<ProviderSummaryQuery>(), 16);
    assert_eq!(std::mem::size_of::<ModuleFacadeFactsQuery>(), 16);
    assert_eq!(std::mem::size_of::<PublicSurfaceModuleFactsQuery>(), 16);
}

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

#[test]
fn query_loader_injects_freestanding_entry_runtime_through_std_start_facade() {
    let root = temp_dir("query_loader_injects_freestanding_entry_runtime_through_std_start_facade");
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

    assert_no_error_diagnostics(&program);
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
    let std_root = program
        .graph
        .package_root(&sym("std"))
        .expect("std package root");
    let std = program.graph.get(std_root).expect("std entry module");
    let start_declaration = std
        .declarations
        .iter()
        .find(|declaration| declaration.name == sym("start"))
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

    assert_no_error_diagnostics(&program);
    assert!(!program.graph.package_facade_active(&sym("std")));
    let process = module_by_suffix(&program, "lib/std/process.nia");
    assert!(
        !process.process_used_paths,
        "process facade should stay shallow while selected exports load their source modules: {process:?}"
    );
    assert!(
        process
            .declarations
            .iter()
            .any(|declaration| declaration.name == sym("types")),
        "process should record the selected types child: {process:?}"
    );
    let process_types = module_by_suffix(&program, "lib/std/process/types.nia");
    assert!(process_types.process_used_paths);
    assert_module_loaded(&program, "lib/std/process/types.nia");
    assert_module_not_loaded(&program, "lib/std/process/command.nia");
    assert_module_loaded(&program, "lib/std/start/freestanding/linux/x86_64.nia");
    assert_module_not_loaded(&program, "lib/std/build/core.nia");
    assert_module_not_loaded(&program, "lib/std/atomic.nia");
    assert_module_not_loaded(&program, "lib/std/debug.nia");
}

#[test]
fn query_loader_loads_implicit_builtin_trait_provider_from_facade() {
    let root = temp_dir("query_loader_loads_implicit_builtin_trait_provider_from_facade");
    let main_path = root.join("main.nia");
    write(
        &main_path,
        r#"
using std;

fn main() void {
    for _ in 1usize..4usize {}
}
"#,
    );

    let program = load_program(main_path.to_string_lossy().into_owned());

    assert_no_error_diagnostics(&program);
    assert_module_loaded(&program, "lib/std/iter.nia");
    assert_module_loaded(&program, "lib/std/iter/range.nia");
    assert_module_not_loaded(&program, "lib/std/process/command.nia");
}

#[test]
fn query_loader_loads_iterator_provider_for_for_in_iterator_values() {
    let root = temp_dir("query_loader_loads_iterator_provider_for_for_in_iterator_values");
    let main_path = root.join("main.nia");
    write(
        &main_path,
        r#"
using std::process;

fn main(init: process::Init) void {
    for _ in init.env().iter() {}
}
"#,
    );

    let program = load_program(main_path.to_string_lossy().into_owned());

    assert_no_error_diagnostics(&program);
    assert_module_loaded(&program, "lib/std/process/env.nia");
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

    let program =
        load_program_with_provider_demand(&main_path, ModuleMap::default(), Some("usize"), "hash");

    assert_no_error_diagnostics(&program);
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

    let program =
        load_program_with_provider_demand(&main_path, ModuleMap::default(), Some("File"), "writer");

    assert_no_error_diagnostics(&program);
    assert_module_loaded(&program, "lib/std/io/file_adapter.nia");
    assert_module_loaded(&program, "lib/std/fs/types.nia");
}

#[test]
fn query_loader_forwards_provider_requests_to_the_selected_reexport_source() {
    let root = temp_dir("query_loader_forwards_provider_requests_to_the_selected_reexport_source");
    let main_path = root.join("main.nia");
    write(
        &main_path,
        r#"
using std::collections::ArrayList;

fn main() usize {
let list = ArrayList[i32]::init();
_ = list;
0
}
"#,
    );

    let program = load_program_with_provider_demand(
        &main_path,
        ModuleMap::default(),
        Some("ArrayList"),
        "init",
    );

    assert_no_error_diagnostics(&program);
    assert_module_loaded(&program, "lib/std/collections.nia");
    assert_module_loaded(&program, "lib/std/collections/array_list.nia");
    assert_module_loaded(&program, "lib/std/collections/array_list/list.nia");
    assert!(
        !program
            .modules
            .iter()
            .any(|module| module.path.as_str().contains("/collections/hash_map")),
        "following the selected ArrayList provider chain must not load the HashMap subtree"
    );
}

#[test]
fn query_loader_loads_package_private_provider_for_reexported_build_type() {
    let root = temp_dir("query_loader_loads_package_private_provider_for_reexported_build_type");
    let main_path = root.join("main.nia");
    write(
        &main_path,
        r#"
using std::build;
using std::fs;
using std::mem;
using std::process;

fn main(init: process::Init, allocator: &mut mem::Allocator) build::Build {
let path = fs::PathView::init(&"");
build::Build::init(init, allocator, path, path, path, path, 1usize)
}
"#,
    );

    let program =
        load_program_with_provider_demand(&main_path, ModuleMap::default(), Some("Build"), "init");

    assert_no_error_diagnostics(&program);
    assert_module_loaded(&program, "lib/std/build.nia");
    assert_module_loaded(&program, "lib/std/build/core.nia");
    assert_module_loaded(&program, "lib/std/build/types.nia");
}

#[test]
fn query_loader_loads_package_private_provider_for_custom_reexported_type() {
    let root = temp_dir("query_loader_loads_package_private_provider_for_custom_reexported_type");
    let main_path = root.join("main.nia");
    let pkg_root = root.join("pkg.nia");
    let main_source = r#"
using dep::facade;

fn main(value: facade::Widget) i32 {
value.score()
}
"#;
    write(&main_path, main_source);
    write(&pkg_root, "pub module facade;");
    fs::create_dir_all(root.join("pkg").join("facade")).expect("create package dir");
    write(
        &root.join("pkg/facade.nia"),
        r#"
pub(pkg) module providers;
pub(pkg) module types;

using self::providers;
pub using types::Widget;
"#,
    );
    write(
        &root.join("pkg/facade/types.nia"),
        r#"pub struct Widget { value: i32 }"#,
    );
    write(
        &root.join("pkg/facade/providers.nia"),
        r#"
using self::types;

extend types::Widget {
pub fn score(&self) i32 {
    self.value
}
}
"#,
    );
    let mut module_map = ModuleMap::new();
    module_map.insert("dep", SourcePath::new(pkg_root.to_string_lossy()));

    let source_path = SourcePath::new(main_path.to_string_lossy());
    let database = LoaderDatabase::new(
        LoadRequest::new(main_path.to_string_lossy().into_owned()).with_module_map(module_map),
    );
    let initial = database.load_program().expect("initial program load");
    let initial_module_ids = initial
        .graph
        .modules()
        .map(|node| (node.path.identity(), node.id))
        .collect::<HashMap<_, _>>();
    let update = database
        .update_provider_demands([ProviderDemand {
            source_path,
            request: nia_compiler_query::ProviderRequest::Method {
                target_type_name: Some(sym("Widget")),
                method_name: sym("score"),
            },
        }])
        .expect("provider graph update");
    let ProviderGraphUpdate::Changed { .. } = update else {
        panic!("provider demand should grow the module graph");
    };
    let program = database.load_program().expect("provider program load");
    assert_eq!(
        program.provider_fact_revision,
        nia_compiler_query::LoaderFactProvider::provider_facts(&database)
            .expect("provider facts")
            .revision()
    );

    assert_no_error_diagnostics(&program);
    for (identity, initial_id) in initial_module_ids {
        assert_eq!(
            program.graph.module_id_for_source_identity(&identity),
            Some(initial_id),
            "provider graph growth changed the module id for {}",
            identity.normalized_path()
        );
    }
    assert_module_loaded(
        &program,
        root.join("pkg/facade.nia").to_string_lossy().as_ref(),
    );
    assert_module_loaded(
        &program,
        root.join("pkg/facade/types.nia").to_string_lossy().as_ref(),
    );
    assert_module_loaded(
        &program,
        root.join("pkg/facade/providers.nia")
            .to_string_lossy()
            .as_ref(),
    );
    let provider_entry = program.graph.entry();

    database.set_source(main_path.to_string_lossy().into_owned(), main_source);
    let reset = database.load_program().expect("reset program load");

    assert_ne!(reset.graph.entry(), provider_entry);
    assert_module_not_loaded(&reset, "pkg/facade/providers.nia");
}

#[test]
fn provider_demand_update_keeps_unmatched_and_known_demands_graph_stable() {
    let root = temp_dir("provider_demand_update_keeps_unmatched_and_known_demands_graph_stable");
    let main_path = root.join("main.nia");
    write(&main_path, "fn main() void {}");
    let database = LoaderDatabase::new(LoadRequest::new(main_path.to_string_lossy().into_owned()));
    let demand = ProviderDemand {
        source_path: SourcePath::new(main_path.to_string_lossy()),
        request: nia_compiler_query::ProviderRequest::Method {
            target_type_name: None,
            method_name: sym("missing"),
        },
    };

    assert_eq!(
        database
            .update_provider_demands([demand.clone()])
            .expect("first provider update"),
        ProviderGraphUpdate::Stable
    );
    assert_eq!(
        database
            .update_provider_demands([demand])
            .expect("second provider update"),
        ProviderGraphUpdate::Stable
    );
    assert!(
        database
            .query_trace()
            .queries
            .iter()
            .all(|query| query.frame.name != "loaded_program"),
        "a stable graph should not rebuild the aggregate loaded program"
    );
    let trace = database.query_trace();
    assert!(trace.dependencies.iter().any(|dependency| {
        dependency.from.name == "module_graph" && dependency.to.name == "provider_demands"
    }));
    assert!(trace.dependencies.iter().all(|dependency| {
        dependency.from.name != "module_graph_revision"
            || dependency.to.name != "module_graph_revision"
    }));
    assert_eq!(
        trace
            .queries
            .iter()
            .filter(|query| query.frame.name == "module_graph_revision")
            .count(),
        1
    );
    assert_eq!(
        database
            .db
            .context()
            .provider_facts
            .retained_transition_count(),
        0
    );
    let graph_query = trace
        .queries
        .iter()
        .find(|query| query.frame.name == "module_graph")
        .expect("module graph query trace");
    assert_eq!(graph_query.stats.executions, 2, "{graph_query:?}");
}

#[test]
fn semantic_provider_demand_remaps_across_graph_owners() {
    let mut initial = ModuleGraph::new(SourcePath::new("main.nia"));
    let initial_provider = initial
        .intern_declared_child_with_processing(
            initial.entry(),
            &sym("provider"),
            nia_ast::Visibility::Private,
            nia_span::Span::default(),
            false,
            false,
        )
        .expect("initial provider module");
    let provider_path = initial.get(initial_provider).unwrap().path.clone();
    let mut rebuilt = ModuleGraph::new(SourcePath::new("main.nia"));
    let rebuilt_provider = rebuilt
        .intern_declared_child_with_processing(
            rebuilt.entry(),
            &sym("provider"),
            nia_ast::Visibility::Private,
            nia_span::Span::default(),
            false,
            false,
        )
        .expect("rebuilt provider module");

    assert_ne!(rebuilt_provider, initial_provider);
    assert!(!rebuilt.get(rebuilt_provider).unwrap().semantic_selected);
    crate::graph::mark_semantic_provider_module(&mut rebuilt, &provider_path);
    assert!(rebuilt.get(rebuilt_provider).unwrap().semantic_selected);
}

#[test]
fn query_loader_does_not_load_a_declared_provider_without_an_explicit_using_edge() {
    let root =
        temp_dir("query_loader_does_not_load_a_declared_provider_without_an_explicit_using_edge");
    let main_path = root.join("main.nia");
    let pkg_root = root.join("pkg.nia");
    write(
        &main_path,
        r#"
using dep::facade;

fn main(value: facade::Widget) i32 {
_ = value;
0
}
"#,
    );
    write(&pkg_root, "pub module facade;");
    fs::create_dir_all(root.join("pkg").join("facade")).expect("create package dir");
    write(
        &root.join("pkg/facade.nia"),
        r#"
pub(pkg) module providers;
pub(pkg) module types;

pub using types::Widget;
"#,
    );
    write(
        &root.join("pkg/facade/types.nia"),
        r#"pub struct Widget { value: i32 }"#,
    );
    write(
        &root.join("pkg/facade/providers.nia"),
        r#"
using self::types;

extend types::Widget {
pub fn score(&self) i32 {
    self.value
}
}
"#,
    );
    let provider_path = root.join("pkg/facade/providers.nia");
    let mut module_map = ModuleMap::new();
    module_map.insert("dep", SourcePath::new(pkg_root.to_string_lossy()));

    let program = load_program_with_map(main_path.to_string_lossy().into_owned(), module_map);

    assert_no_error_diagnostics(&program);
    assert!(
        !program
            .modules
            .iter()
            .any(|module| module.path.as_str() == provider_path.to_string_lossy()),
        "declaring a provider child must not make it visible without an explicit using edge"
    );
}

#[test]
fn query_loader_resolves_std_root_reexport_import_shallowly() {
    let root = temp_dir("query_loader_resolves_std_root_reexport_import_shallowly");
    let main_path = root.join("main.nia");
    write(&main_path, "using std::CStringView; fn main() void {}");

    let program = load_program(main_path.to_string_lossy().into_owned());

    assert_no_error_diagnostics(&program);
    assert!(!program.graph.package_facade_active(&sym("std")));
    assert_module_loaded(&program, "lib/std/cstring.nia");
    assert_module_not_loaded(&program, "lib/std/process.nia");
}

#[test]
fn query_loader_resolves_std_single_value_reexport_import_shallowly() {
    let root = temp_dir("query_loader_resolves_std_single_value_reexport_import_shallowly");
    let main_path = root.join("main.nia");
    write(&main_path, "using std::CStringView; fn main() void {}");

    let program = load_program(main_path.to_string_lossy().into_owned());

    assert_no_error_diagnostics(&program);
    assert!(!program.graph.package_facade_active(&sym("std")));
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

    assert_no_error_diagnostics(&program);
    assert!(!program.graph.package_facade_active(&sym("std")));
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

    assert_no_error_diagnostics(&program);
    assert!(!program.graph.package_facade_active(&sym("std")));
    assert!(program.graph.package_root(&sym("std")).is_none());
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

    assert_no_error_diagnostics(&program);
    assert_module_loaded(&program, root.join("main.nia").to_string_lossy().as_ref());
    assert_module_loaded(&program, root.join("defs.nia").to_string_lossy().as_ref());
    let root_module = program
        .graph
        .get(program.graph.entry())
        .expect("entry module");
    let defs_module = program
        .graph
        .get(root_module.children[&sym("defs")])
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

    assert_no_error_diagnostics(&program);
    assert!(program.modules.iter().any(|module| {
        module.path.as_str() == "main.nia"
            && module.item_tree.items.iter().any(|item| {
                matches!(
                    &item.kind,
                    ItemTreeNodeKind::Module(module_item) if module_item.name == sym("defs")
                )
            })
    }));
    assert!(program.modules.iter().any(|module| {
        module.path.as_str() == "defs.nia"
            && module.item_tree.items.iter().any(|item| {
                matches!(
                    &item.kind,
                    ItemTreeNodeKind::Function(function) if function.name == sym("value")
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

    let trace = load_program_trace(main_path, ModuleMap::default());

    assert!(trace.dependencies.iter().any(|dependency| {
        dependency.from.name == "parsed_module" && dependency.to.name == "syntax_module"
    }));
    assert!(trace.dependencies.iter().any(|dependency| {
        dependency.from.name == "syntax_module" && dependency.to.name == "source_text"
    }));
    assert!(trace.dependencies.iter().any(|dependency| {
        dependency.from.name == "module_declarations" && dependency.to.name == "parsed_module"
    }));
    assert!(trace.dependencies.iter().any(|dependency| {
        dependency.from.name == "module_graph_revision" && dependency.to.name == "source_status"
    }));
}

#[test]
fn provider_summary_is_cached_per_module_source_version() {
    let sources = SourceDatabase::new();
    let main = SourcePath::new("main.nia");
    let provider = SourcePath::new("provider.nia");
    sources.set_source(main.clone(), "fn main() void {}");
    sources.set_source(
        provider.clone(),
        r#"
struct Widget { value: i32 }

extend Widget {
    pub fn score(&self) i32 {
        self.value
    }
}
"#,
    );
    let db = registered_query_db(test_loader_context(
        main.clone(),
        ModuleMap::default(),
        sources,
    ));
    let first = db.expect_get(provider_summary_query(&db, &provider));
    let second = db.expect_get(provider_summary_query(&db, &provider));
    assert!(first.defines_inherent_associated_item(&sym("Widget"), &sym("score")));
    assert_eq!(first, second);

    let trace = db.query_trace();
    let query = trace
        .queries
        .iter()
        .find(|query| query.frame.name == "provider_summary")
        .expect("provider summary query should be recorded");
    assert_eq!(query.stats.executions, 1, "{query:?}");
    assert_eq!(query.stats.cache_hits, 1, "{query:?}");
}

#[test]
fn persistent_module_dependencies_hit_skips_parse_and_tracks_exact_source_spans() {
    let root = temp_dir("persistent_module_dependencies_hit_skips_parse_and_tracks_spans");
    let sources = SourceDatabase::new();
    let main = SourcePath::new("main.nia");
    sources.set_source(main.clone(), "fn helper() i32 { 1 } pub module child;");
    let first_file = sources
        .source_for_path(&main)
        .expect("main source should be present");
    let module_map = ModuleMap::new();
    let cache = Arc::new(crate::frontend_cache::PersistentFrontendCache::new(
        root.join("cache"),
    ));
    let first_identity = module_dependencies_cache_identity(&first_file, &main, &module_map);

    let first = frontend_cache_database(&main, &sources, module_map.clone(), cache.clone(), false);
    let first_dependencies = first.expect_get(module_declarations_query(&first, &main));
    assert!(first_dependencies.diagnostics.is_empty());
    assert_eq!(first_dependencies.declarations.len(), 1);
    assert_eq!(first_dependencies.declarations[0].name, sym("child"));
    assert_eq!(query_executions(&first.query_trace(), "parsed_module"), 1);
    let first_span = first_dependencies.declarations[0].span;

    let second = frontend_cache_database(&main, &sources, module_map.clone(), cache.clone(), false);
    let second_dependencies = second.expect_get(module_declarations_query(&second, &main));
    assert_eq!(first_dependencies, second_dependencies);
    assert_eq!(query_executions(&second.query_trace(), "parsed_module"), 0);

    let path = cache.module_dependencies_path(first_identity.key);
    fs::write(&path, b"corrupt module dependency summary")
        .expect("corrupt module dependencies entry");
    let repaired =
        frontend_cache_database(&main, &sources, module_map.clone(), cache.clone(), false);
    let repaired_dependencies = repaired.expect_get(module_declarations_query(&repaired, &main));
    assert_eq!(first_dependencies, repaired_dependencies);
    assert_eq!(
        query_executions(&repaired.query_trace(), "parsed_module"),
        1
    );

    let edited_file = sources.set_source(
        main.clone(),
        "fn helper() i32 { 1000 + 2000 + 3000 } pub module child;",
    );
    let edited_identity = module_dependencies_cache_identity(&edited_file, &main, &module_map);
    assert_ne!(first_identity.key, edited_identity.key);
    let edited = frontend_cache_database(&main, &sources, module_map.clone(), cache.clone(), false);
    let edited_dependencies = edited.expect_get(module_declarations_query(&edited, &main));
    assert_eq!(edited_dependencies.declarations[0].name, sym("child"));
    assert_ne!(first_span, edited_dependencies.declarations[0].span);
    assert_eq!(query_executions(&edited.query_trace(), "parsed_module"), 1);

    let reused = frontend_cache_database(&main, &sources, module_map, cache, false);
    let reused_dependencies = reused.expect_get(module_declarations_query(&reused, &main));
    assert_eq!(edited_dependencies, reused_dependencies);
    assert_eq!(query_executions(&reused.query_trace(), "parsed_module"), 0);
}

#[test]
fn persistent_module_dependencies_skip_all_graph_discovery_parses_across_sessions() {
    let root = temp_dir("persistent_module_dependencies_skip_graph_discovery_parses");
    let main = SourcePath::new("main.nia");
    let cache = Arc::new(crate::frontend_cache::PersistentFrontendCache::new(
        root.join("cache"),
    ));
    let first_sources = SourceDatabase::new();
    first_sources.set_source(main.clone(), "module middle;");
    first_sources.set_source(SourcePath::new("middle.nia"), "module leaf;");
    first_sources.set_source(SourcePath::new("middle/leaf.nia"), "pub struct Value {}");
    let first = frontend_cache_database(
        &main,
        &first_sources,
        ModuleMap::default(),
        cache.clone(),
        false,
    );

    let first_graph = first.expect_get(crate::graph::ModuleGraphQuery);
    let first_paths = first_graph
        .modules()
        .map(|module| module.path.clone())
        .collect::<Vec<_>>();

    assert_eq!(first_paths.len(), 3);
    assert_eq!(query_executions(&first.query_trace(), "parsed_module"), 3);
    assert_eq!(
        query_executions(&first.query_trace(), "module_declarations"),
        3
    );
    drop(first_graph);
    drop(first);

    let second_sources = SourceDatabase::new();
    second_sources.set_source(main.clone(), "module middle;");
    second_sources.set_source(SourcePath::new("middle.nia"), "module leaf;");
    second_sources.set_source(SourcePath::new("middle/leaf.nia"), "pub struct Value {}");
    let second =
        frontend_cache_database(&main, &second_sources, ModuleMap::default(), cache, false);

    let second_graph = second.expect_get(crate::graph::ModuleGraphQuery);
    let second_paths = second_graph
        .modules()
        .map(|module| module.path.clone())
        .collect::<Vec<_>>();

    assert_eq!(second_paths, first_paths);
    assert_eq!(query_executions(&second.query_trace(), "parsed_module"), 0);
    assert_eq!(
        query_executions(&second.query_trace(), "module_declarations"),
        3
    );
}

#[test]
fn module_dependencies_cache_keys_include_effective_module_map() {
    let root = temp_dir("module_dependencies_cache_keys_include_effective_module_map");
    let sources = SourceDatabase::new();
    let main = SourcePath::new("main.nia");
    let file = sources.set_source(main.clone(), "using dep::Thing; fn main() void {}");
    let mut mapped = ModuleMap::new();
    mapped.insert("dep", SourcePath::new("deps/root.nia"));
    let unmapped = ModuleMap::new();
    let mapped_identity = module_dependencies_cache_identity(&file, &main, &mapped);
    let unmapped_identity = module_dependencies_cache_identity(&file, &main, &unmapped);
    assert_ne!(mapped_identity.module_map, unmapped_identity.module_map);
    assert_ne!(mapped_identity.key, unmapped_identity.key);
    let cache = Arc::new(crate::frontend_cache::PersistentFrontendCache::new(
        root.join("cache"),
    ));

    let mapped_db = frontend_cache_database(&main, &sources, mapped, cache.clone(), false);
    let mapped_dependencies = mapped_db.expect_get(module_declarations_query(&mapped_db, &main));
    assert!(matches!(
        mapped_dependencies.explicit_imports[0].path,
        crate::used_paths::UsedModulePath::Package { .. }
    ));

    let unmapped_db =
        frontend_cache_database(&main, &sources, unmapped.clone(), cache.clone(), false);
    let unmapped_dependencies =
        unmapped_db.expect_get(module_declarations_query(&unmapped_db, &main));
    assert!(matches!(
        unmapped_dependencies.explicit_imports[0].path,
        crate::used_paths::UsedModulePath::Local { .. }
    ));
    assert_ne!(mapped_dependencies, unmapped_dependencies);
    assert_eq!(
        query_executions(&unmapped_db.query_trace(), "parsed_module"),
        1
    );

    let reused = frontend_cache_database(&main, &sources, unmapped, cache, false);
    let reused_dependencies = reused.expect_get(module_declarations_query(&reused, &main));
    assert_eq!(unmapped_dependencies, reused_dependencies);
    assert_eq!(query_executions(&reused.query_trace(), "parsed_module"), 0);
}

#[test]
fn module_dependencies_verification_replaces_semantically_wrong_valid_entry() {
    let root = temp_dir("module_dependencies_verification_replaces_wrong_valid_entry");
    let sources = SourceDatabase::new();
    let main = SourcePath::new("main.nia");
    let file = sources.set_source(main.clone(), "pub module child;");
    let module_map = ModuleMap::new();
    let identity = module_dependencies_cache_identity(&file, &main, &module_map);
    let cache = Arc::new(crate::frontend_cache::PersistentFrontendCache::new(
        root.join("cache"),
    ));
    let wrong = crate::used_paths::ModuleDeclarations {
        declarations: Vec::new(),
        package_roots: Vec::new(),
        used_module_paths: Vec::new(),
        explicit_imports: Vec::new(),
        used_import_aliases: Vec::new(),
        diagnostics: Vec::new(),
    };
    cache
        .publish_module_dependencies(
            identity.namespace,
            &identity.module,
            crate::frontend_cache::ModuleDependenciesSource::new(
                identity.source,
                identity.source_len,
            ),
            identity.module_map,
            &wrong,
            &SymbolTable::new(),
        )
        .expect("publish wrong but structurally valid module dependencies");

    let verifying =
        frontend_cache_database(&main, &sources, module_map.clone(), cache.clone(), true);
    let verified = verifying.expect_get(module_declarations_query(&verifying, &main));
    assert_eq!(verified.declarations.len(), 1);
    assert_eq!(verified.declarations[0].name, sym("child"));
    assert_eq!(
        query_executions(&verifying.query_trace(), "parsed_module"),
        1
    );

    let reused = frontend_cache_database(&main, &sources, module_map, cache, false);
    let reused_dependencies = reused.expect_get(module_declarations_query(&reused, &main));
    assert_eq!(verified, reused_dependencies);
    assert_eq!(query_executions(&reused.query_trace(), "parsed_module"), 0);
}

#[test]
fn module_dependencies_with_diagnostics_are_not_persisted() {
    let root = temp_dir("module_dependencies_with_diagnostics_are_not_persisted");
    let sources = SourceDatabase::new();
    let main = SourcePath::new("main.nia");
    let file = sources.set_source(main.clone(), "module child; module child;");
    let module_map = ModuleMap::new();
    let identity = module_dependencies_cache_identity(&file, &main, &module_map);
    let cache = Arc::new(crate::frontend_cache::PersistentFrontendCache::new(
        root.join("cache"),
    ));
    let database = frontend_cache_database(&main, &sources, module_map, cache.clone(), false);
    let dependencies = database.expect_get(module_declarations_query(&database, &main));

    assert!(!dependencies.diagnostics.is_empty());
    assert!(!cache.module_dependencies_path(identity.key).is_file());

    let malformed_file = sources.set_source(main.clone(), "fn broken(");
    let malformed_identity =
        module_dependencies_cache_identity(&malformed_file, &main, &ModuleMap::new());
    let malformed =
        frontend_cache_database(&main, &sources, ModuleMap::new(), cache.clone(), false);
    let malformed_dependencies = malformed.expect_get(module_declarations_query(&malformed, &main));
    assert!(malformed_dependencies.declarations.is_empty());
    assert!(
        !cache
            .module_dependencies_path(malformed_identity.key)
            .is_file()
    );
}

#[test]
fn persistent_public_surface_facts_skip_item_tree_and_recover_from_corruption() {
    let root = temp_dir("persistent_public_surface_facts_skip_item_tree");
    let main = SourcePath::new("main.nia");
    let source = r#"
pub fn before() i32 { 1 }
pub struct Widget { value: i32 }
pub enum Choice { First, Second }
pub using self::Choice::{First as Selected, Second};
"#;
    let cache = Arc::new(crate::frontend_cache::PersistentFrontendCache::new(
        root.join("cache"),
    ));
    let first_sources = SourceDatabase::new();
    let first_file = first_sources.set_source(main.clone(), source);
    let first_identity = public_surface_facts_cache_identity(&first_file);
    let first = frontend_cache_database(
        &main,
        &first_sources,
        ModuleMap::default(),
        cache.clone(),
        false,
    );
    let first_facts = first.expect_get(public_surface_module_facts_query(&first, &main));
    let first_widget_span = first_facts
        .defs
        .iter()
        .find(|def| def.name == sym("Widget"))
        .expect("Widget definition fact")
        .span;
    assert_eq!(
        query_executions(&first.query_trace(), "loader_active_module_item_tree_fact"),
        1
    );

    let second_sources = SourceDatabase::new();
    second_sources.set_source(main.clone(), source);
    let second = frontend_cache_database(
        &main,
        &second_sources,
        ModuleMap::default(),
        cache.clone(),
        false,
    );
    let second_facts = second.expect_get(public_surface_module_facts_query(&second, &main));
    assert_eq!(first_facts, second_facts);
    assert_eq!(
        query_executions(&second.query_trace(), "loader_public_surface_module_facts"),
        1
    );
    assert_eq!(
        query_executions(&second.query_trace(), "loader_active_module_item_tree_fact"),
        0
    );
    assert_eq!(query_executions(&second.query_trace(), "parsed_module"), 0);
    assert_eq!(
        second.context().symbols.resolve(sym("Widget")).as_deref(),
        Some("Widget")
    );

    let path = cache.public_surface_facts_path(first_identity.key);
    fs::write(&path, b"corrupt public surface facts").expect("corrupt facts entry");
    let repaired_sources = SourceDatabase::new();
    repaired_sources.set_source(main.clone(), source);
    let repaired = frontend_cache_database(
        &main,
        &repaired_sources,
        ModuleMap::default(),
        cache.clone(),
        false,
    );
    let repaired_facts = repaired.expect_get(public_surface_module_facts_query(&repaired, &main));
    assert_eq!(first_facts, repaired_facts);
    assert_eq!(
        query_executions(&repaired.query_trace(), "parsed_module"),
        1
    );

    let edited_source = source.replace("{ 1 }", "{ 1000 + 2000 + 3000 }");
    let edited_sources = SourceDatabase::new();
    let edited_file = edited_sources.set_source(main.clone(), edited_source.clone());
    let edited_identity = public_surface_facts_cache_identity(&edited_file);
    assert_ne!(first_identity.key, edited_identity.key);
    let edited = frontend_cache_database(
        &main,
        &edited_sources,
        ModuleMap::default(),
        cache.clone(),
        false,
    );
    let edited_facts = edited.expect_get(public_surface_module_facts_query(&edited, &main));
    let edited_widget_span = edited_facts
        .defs
        .iter()
        .find(|def| def.name == sym("Widget"))
        .expect("edited Widget definition fact")
        .span;
    assert_ne!(first_widget_span, edited_widget_span);
    assert_eq!(query_executions(&edited.query_trace(), "parsed_module"), 1);

    let reused_sources = SourceDatabase::new();
    reused_sources.set_source(main.clone(), edited_source);
    let reused =
        frontend_cache_database(&main, &reused_sources, ModuleMap::default(), cache, false);
    let reused_facts = reused.expect_get(public_surface_module_facts_query(&reused, &main));
    assert_eq!(edited_facts, reused_facts);
    assert_eq!(query_executions(&reused.query_trace(), "parsed_module"), 0);
}

#[test]
fn public_surface_facts_verification_replaces_semantically_wrong_valid_entry() {
    let root = temp_dir("public_surface_facts_verification_replaces_wrong_entry");
    let sources = SourceDatabase::new();
    let main = SourcePath::new("main.nia");
    let file = sources.set_source(main.clone(), "pub struct Widget {}");
    let identity = public_surface_facts_cache_identity(&file);
    let cache = Arc::new(crate::frontend_cache::PersistentFrontendCache::new(
        root.join("cache"),
    ));
    cache
        .publish_public_surface_facts(
            identity.namespace,
            &identity.module,
            crate::frontend_cache::PublicSurfaceFactsSource::new(
                identity.source,
                identity.source_len,
            ),
            &nia_defs::PublicSurfaceModuleFacts {
                defs: Vec::new(),
                module_scope: nia_defs::PublicSurfaceModuleScopeFacts::default(),
                enum_scopes: Vec::new(),
                module_usings: Vec::new(),
            },
            &SymbolTable::new(),
        )
        .expect("publish wrong but structurally valid public surface facts");

    let verifying =
        frontend_cache_database(&main, &sources, ModuleMap::default(), cache.clone(), true);
    let verified = verifying.expect_get(public_surface_module_facts_query(&verifying, &main));
    assert!(verified.defs.iter().any(|def| def.name == sym("Widget")));
    assert_eq!(
        query_executions(&verifying.query_trace(), "parsed_module"),
        1
    );

    let reused_sources = SourceDatabase::new();
    reused_sources.set_source(main.clone(), "pub struct Widget {}");
    let reused =
        frontend_cache_database(&main, &reused_sources, ModuleMap::default(), cache, false);
    let reused_facts = reused.expect_get(public_surface_module_facts_query(&reused, &main));
    assert_eq!(verified, reused_facts);
    assert_eq!(query_executions(&reused.query_trace(), "parsed_module"), 0);
}

#[test]
fn public_surface_facts_with_diagnostics_are_not_persisted() {
    let root = temp_dir("public_surface_facts_with_diagnostics_are_not_persisted");
    let sources = SourceDatabase::new();
    let main = SourcePath::new("main.nia");
    let file = sources.set_source(
        main.clone(),
        "pub fn value() void {} pub fn value() void {}",
    );
    let identity = public_surface_facts_cache_identity(&file);
    let cache = Arc::new(crate::frontend_cache::PersistentFrontendCache::new(
        root.join("cache"),
    ));
    let database =
        frontend_cache_database(&main, &sources, ModuleMap::default(), cache.clone(), false);
    let facts = database.expect_get(public_surface_module_facts_query(&database, &main));
    assert_eq!(
        facts
            .defs
            .iter()
            .filter(|def| def.name == sym("value"))
            .count(),
        2
    );
    assert!(!cache.public_surface_facts_path(identity.key).is_file());

    let malformed_file = sources.set_source(main.clone(), "pub fn broken(");
    let malformed_identity = public_surface_facts_cache_identity(&malformed_file);
    let malformed =
        frontend_cache_database(&main, &sources, ModuleMap::default(), cache.clone(), false);
    let _ = malformed.expect_get(public_surface_module_facts_query(&malformed, &main));
    assert!(
        !cache
            .public_surface_facts_path(malformed_identity.key)
            .is_file()
    );
}

#[test]
fn public_surface_facts_cache_round_trips_all_stable_fields() {
    use nia_ast::PathSegmentKind;
    use nia_defs::{
        DefId, DefKind, ModuleUsing, PublicSurfaceDefFact, PublicSurfaceEnumScopeFact,
        PublicSurfaceModuleFacts, PublicSurfaceModuleScopeFacts, UsingGroupItem, UsingName,
        UsingPathSegment, UsingSelector,
    };
    use nia_span::Span;

    let root = temp_dir("public_surface_facts_round_trip_all_stable_fields");
    let sources = SourceDatabase::new();
    let main = SourcePath::new("main.nia");
    let file = sources.set_source(main, " ".repeat(512));
    let identity = public_surface_facts_cache_identity(&file);
    let cache = crate::frontend_cache::PersistentFrontendCache::new(root.join("cache"));
    let names = [
        "module",
        "function",
        "global",
        "const",
        "struct",
        "struct_field",
        "union",
        "union_field",
        "trait",
        "associated_type",
        "trait_method",
        "method",
        "enum",
        "variant",
        "type_alias",
    ];
    let symbols = symbols_for(&[
        "module",
        "function",
        "global",
        "const",
        "struct",
        "struct_field",
        "union",
        "union_field",
        "trait",
        "associated_type",
        "trait_method",
        "method",
        "enum",
        "variant",
        "type_alias",
        "host",
        "selected",
        "renamed",
        "nested",
        "plain",
        "final",
    ]);
    let kinds = [
        DefKind::Module,
        DefKind::Function,
        DefKind::Global,
        DefKind::Const,
        DefKind::Struct,
        DefKind::StructField,
        DefKind::Union,
        DefKind::UnionField,
        DefKind::Trait,
        DefKind::TraitAssociatedType,
        DefKind::TraitMethod,
        DefKind::Method,
        DefKind::Enum,
        DefKind::EnumVariant,
        DefKind::TypeAlias,
    ];
    let parents = [
        None,
        None,
        None,
        None,
        None,
        Some(DefId(5)),
        None,
        Some(DefId(7)),
        None,
        Some(DefId(9)),
        Some(DefId(9)),
        Some(DefId(5)),
        None,
        Some(DefId(13)),
        None,
    ];
    let visibilities = [
        Visibility::Private,
        Visibility::PublicSuper,
        Visibility::PublicPkg,
        Visibility::Public,
    ];
    let defs = names
        .into_iter()
        .zip(kinds)
        .zip(parents)
        .enumerate()
        .map(|(index, ((name, kind), parent))| PublicSurfaceDefFact {
            id: DefId((index + 1) as u64),
            name: sym(name),
            kind,
            parent,
            visibility: visibilities[index % visibilities.len()],
            span: Span::new(index + 1, index + 2),
        })
        .collect::<Vec<_>>();
    let mut modules = vec![(sym("module"), DefId(1))];
    let mut types = vec![
        (sym("struct"), DefId(5)),
        (sym("union"), DefId(7)),
        (sym("trait"), DefId(9)),
        (sym("enum"), DefId(13)),
        (sym("type_alias"), DefId(15)),
    ];
    let mut values = vec![
        (sym("function"), DefId(2)),
        (sym("global"), DefId(3)),
        (sym("const"), DefId(4)),
    ];
    modules.sort_by_key(|entry| entry.0);
    types.sort_by_key(|entry| entry.0);
    values.sort_by_key(|entry| entry.0);
    let facts = PublicSurfaceModuleFacts {
        defs,
        module_scope: PublicSurfaceModuleScopeFacts {
            modules,
            types,
            values,
        },
        enum_scopes: vec![PublicSurfaceEnumScopeFact {
            owner: DefId(13),
            variants: vec![(sym("variant"), DefId(14))],
        }],
        module_usings: vec![ModuleUsing {
            visibility: Visibility::PublicPkg,
            span: Span::new(40, 90),
            host: vec![
                UsingPathSegment {
                    kind: PathSegmentKind::Name(sym("host")),
                    span: Span::new(41, 45),
                },
                UsingPathSegment {
                    kind: PathSegmentKind::Package,
                    span: Span::new(46, 49),
                },
                UsingPathSegment {
                    kind: PathSegmentKind::Super,
                    span: Span::new(50, 53),
                },
                UsingPathSegment {
                    kind: PathSegmentKind::SelfValue,
                    span: Span::new(54, 57),
                },
            ],
            selector: UsingSelector::Group(vec![
                UsingGroupItem::Name(UsingName {
                    name: sym("selected"),
                    name_span: Span::new(58, 62),
                    alias: Some(sym("renamed")),
                    alias_span: Some(Span::new(63, 67)),
                }),
                UsingGroupItem::Nested {
                    host: vec![UsingPathSegment {
                        kind: PathSegmentKind::Name(sym("nested")),
                        span: Span::new(68, 72),
                    }],
                    selector: Box::new(UsingSelector::Group(vec![
                        UsingGroupItem::Name(UsingName {
                            name: sym("plain"),
                            name_span: Span::new(73, 75),
                            alias: None,
                            alias_span: None,
                        }),
                        UsingGroupItem::Nested {
                            host: vec![UsingPathSegment {
                                kind: PathSegmentKind::Super,
                                span: Span::new(76, 77),
                            }],
                            selector: Box::new(UsingSelector::Wildcard {
                                span: Span::new(78, 79),
                            }),
                        },
                        UsingGroupItem::Nested {
                            host: vec![UsingPathSegment {
                                kind: PathSegmentKind::Package,
                                span: Span::new(80, 81),
                            }],
                            selector: Box::new(UsingSelector::SelfName),
                        },
                        UsingGroupItem::Nested {
                            host: vec![UsingPathSegment {
                                kind: PathSegmentKind::SelfValue,
                                span: Span::new(82, 83),
                            }],
                            selector: Box::new(UsingSelector::Single(UsingName {
                                name: sym("final"),
                                name_span: Span::new(84, 85),
                                alias: None,
                                alias_span: None,
                            })),
                        },
                    ])),
                },
            ]),
        }],
    };
    let source =
        crate::frontend_cache::PublicSurfaceFactsSource::new(identity.source, identity.source_len);
    cache
        .publish_public_surface_facts(
            identity.namespace,
            &identity.module,
            source,
            &facts,
            &symbols,
        )
        .expect("publish complete public surface facts");
    let loaded_symbols = SymbolTable::new();

    assert!(matches!(
        cache
            .load_public_surface_facts(
                identity.key,
                identity.namespace,
                &identity.module,
                source,
                &loaded_symbols,
            )
            .expect("load complete public surface facts"),
        crate::frontend_cache::PublicSurfaceFactsCacheLookup::Hit(cached) if cached == facts
    ));
    assert_eq!(
        loaded_symbols.resolve(sym("renamed")).as_deref(),
        Some("renamed")
    );
    assert_eq!(
        loaded_symbols.resolve(sym("final")).as_deref(),
        Some("final")
    );

    let short_sources = SourceDatabase::new();
    let short_file = short_sources.set_source(SourcePath::new("short.nia"), " ".repeat(32));
    let short_identity = public_surface_facts_cache_identity(&short_file);
    assert!(
        cache
            .publish_public_surface_facts(
                short_identity.namespace,
                &short_identity.module,
                crate::frontend_cache::PublicSurfaceFactsSource::new(
                    short_identity.source,
                    short_identity.source_len,
                ),
                &facts,
                &symbols,
            )
            .is_err()
    );
}

#[test]
fn module_dependencies_cache_round_trips_all_stable_fields() {
    use crate::used_paths::{ExplicitUsingImport, UsedModulePath, UsedModulePathProcessing};

    let root = temp_dir("module_dependencies_cache_round_trips_all_stable_fields");
    let sources = SourceDatabase::new();
    let main = SourcePath::new("main.nia");
    let file = sources.set_source(main.clone(), " ".repeat(256));
    let module_map = ModuleMap::new();
    let identity = module_dependencies_cache_identity(&file, &main, &module_map);
    let cache = crate::frontend_cache::PersistentFrontendCache::new(root.join("cache"));
    let symbols = symbols_for(&[
        "private", "super", "package", "public", "dep_b", "dep_a", "one", "two", "three", "four",
        "Trait", "Alias", "value", "AliasB", "AliasA",
    ]);
    let declarations = [
        ("private", Visibility::Private, nia_span::Span::new(1, 2)),
        ("super", Visibility::PublicSuper, nia_span::Span::new(3, 4)),
        ("package", Visibility::PublicPkg, nia_span::Span::new(5, 6)),
        ("public", Visibility::Public, nia_span::Span::new(7, 8)),
    ]
    .into_iter()
    .map(
        |(name, visibility, span)| nia_imports::ResolvedModuleDeclaration {
            name: sym(name),
            visibility,
            span,
        },
    )
    .collect::<Vec<_>>();
    let mut package_roots = vec![sym("dep_b"), sym("dep_a")];
    package_roots.sort();
    let mut used_module_paths = vec![
        UsedModulePath::Package {
            package: sym("dep_a"),
            segments: vec![sym("one")],
            include_declared_children: true,
            processing: UsedModulePathProcessing::Always,
        },
        UsedModulePath::PackageRelative {
            segments: vec![sym("two")],
            include_declared_children: false,
            processing: UsedModulePathProcessing::IfSelectedItem,
        },
        UsedModulePath::ParentRelative {
            segments: vec![sym("three")],
            include_declared_children: true,
            processing: UsedModulePathProcessing::IfProvidesExtensions,
        },
        UsedModulePath::Local {
            segments: vec![sym("four")],
            include_declared_children: false,
            processing: UsedModulePathProcessing::IfProvidesTraitImpl {
                trait_name: sym("Trait"),
            },
        },
    ];
    used_module_paths.sort();
    let explicit_imports = vec![ExplicitUsingImport {
        span: nia_span::Span::new(9, 20),
        alias: sym("Alias"),
        path: UsedModulePath::Local {
            segments: vec![sym("value")],
            include_declared_children: false,
            processing: UsedModulePathProcessing::Never,
        },
    }];
    let mut used_import_aliases = vec![sym("AliasB"), sym("AliasA")];
    used_import_aliases.sort();
    let dependencies = crate::used_paths::ModuleDeclarations {
        declarations,
        package_roots,
        used_module_paths,
        explicit_imports,
        used_import_aliases,
        diagnostics: Vec::new(),
    };
    cache
        .publish_module_dependencies(
            identity.namespace,
            &identity.module,
            crate::frontend_cache::ModuleDependenciesSource::new(
                identity.source,
                identity.source_len,
            ),
            identity.module_map,
            &dependencies,
            &symbols,
        )
        .expect("publish complete module dependency summary");
    let loaded_symbols = SymbolTable::new();

    assert!(matches!(
        cache
            .load_module_dependencies(
                identity.key,
                identity.namespace,
                &identity.module,
                crate::frontend_cache::ModuleDependenciesSource::new(
                    identity.source,
                    identity.source_len,
                ),
                identity.module_map,
                &loaded_symbols,
            )
            .expect("load complete module dependency summary"),
        crate::frontend_cache::ModuleDependenciesCacheLookup::Hit(cached)
            if cached == dependencies
    ));
    assert_eq!(
        loaded_symbols.resolve(sym("private")).as_deref(),
        Some("private")
    );
}

#[test]
fn persistent_provider_summary_hit_skips_parse_and_recovers_from_corruption() {
    let root = temp_dir("persistent_provider_summary_hit_skips_parse_and_recovers_from_corruption");
    let cache_root = root.join("cache");
    let sources = SourceDatabase::new();
    let main = SourcePath::new("main.nia");
    let provider = SourcePath::new("provider.nia");
    sources.set_source(main.clone(), "fn main() void {}");
    let provider_file = sources.set_source(
        provider.clone(),
        "struct Widget {} extend Widget { pub fn score(&self) i32 { 1 } }",
    );
    let cache = Arc::new(crate::frontend_cache::PersistentFrontendCache::new(
        cache_root,
    ));
    let identity = provider_cache_identity(&provider_file);

    let first = provider_summary_database(&main, &sources, cache.clone(), false);
    let first_summary = first.expect_get(provider_summary_query(&first, &provider));
    assert!(first_summary.defines_inherent_associated_item(&sym("Widget"), &sym("score")));
    assert_eq!(query_executions(&first.query_trace(), "parsed_module"), 1);

    let second = provider_summary_database(&main, &sources, cache.clone(), false);
    let second_summary = second.expect_get(provider_summary_query(&second, &provider));
    assert_eq!(first_summary, second_summary);
    assert_eq!(query_executions(&second.query_trace(), "parsed_module"), 0);

    let path = cache.provider_summary_path(identity.provider_key);
    fs::write(&path, b"corrupt frontend cache entry").expect("corrupt provider summary cache");
    let third = provider_summary_database(&main, &sources, cache.clone(), false);
    let third_summary = third.expect_get(provider_summary_query(&third, &provider));
    assert_eq!(first_summary, third_summary);
    assert_eq!(query_executions(&third.query_trace(), "parsed_module"), 1);
    assert!(matches!(
        {
            let loaded_symbols = SymbolTable::new();
            cache
                .load_provider_summary(
                    identity.provider_key,
                    identity.namespace,
                    &identity.module,
                    identity.item_signature,
                    &loaded_symbols,
                )
                .expect("reload repaired provider summary")
        },
        crate::frontend_cache::ProviderSummaryCacheLookup::Hit(_)
    ));

    let manifest_path = cache.dependency_manifest_path(identity.source_key);
    fs::write(&manifest_path, b"corrupt frontend manifest").expect("corrupt dependency manifest");
    let fourth = provider_summary_database(&main, &sources, cache.clone(), false);
    let fourth_summary = fourth.expect_get(provider_summary_query(&fourth, &provider));
    assert_eq!(first_summary, fourth_summary);
    assert_eq!(query_executions(&fourth.query_trace(), "parsed_module"), 1);
    assert!(matches!(
        cache
            .load_dependency_manifest(
                identity.source_key,
                identity.namespace,
                &identity.module,
                identity.source,
            )
            .expect("reload repaired dependency manifest"),
        crate::frontend_cache::DependencyManifestCacheLookup::Hit(item_signature)
            if item_signature == identity.item_signature
    ));

    let fifth = provider_summary_database(&main, &sources, cache, false);
    let fifth_summary = fifth.expect_get(provider_summary_query(&fifth, &provider));
    assert_eq!(first_summary, fifth_summary);
    assert_eq!(query_executions(&fifth.query_trace(), "parsed_module"), 0);
}

#[test]
fn provider_summary_verification_replaces_semantically_wrong_valid_entry() {
    let root = temp_dir("provider_summary_verification_replaces_semantically_wrong_valid_entry");
    let sources = SourceDatabase::new();
    let main = SourcePath::new("main.nia");
    let provider = SourcePath::new("provider.nia");
    sources.set_source(main.clone(), "fn main() void {}");
    let provider_file = sources.set_source(
        provider.clone(),
        "struct Widget {} extend Widget { pub fn score(&self) i32 { 1 } }",
    );
    let cache = Arc::new(crate::frontend_cache::PersistentFrontendCache::new(
        root.join("cache"),
    ));
    let identity = provider_cache_identity(&provider_file);
    cache
        .publish_dependency_manifest(
            identity.source_key,
            identity.namespace,
            &identity.module,
            identity.source,
            identity.item_signature,
        )
        .expect("publish dependency manifest");
    cache
        .publish_provider_summary(
            identity.provider_key,
            identity.namespace,
            &identity.module,
            identity.item_signature,
            &nia_provider_summary::ProviderSummary::default(),
            &SymbolTable::new(),
        )
        .expect("publish wrong but structurally valid provider summary");

    let verifying = provider_summary_database(&main, &sources, cache.clone(), true);
    let verified = verifying.expect_get(provider_summary_query(&verifying, &provider));
    assert!(verified.defines_inherent_associated_item(&sym("Widget"), &sym("score")));
    assert_eq!(
        query_executions(&verifying.query_trace(), "parsed_module"),
        1
    );

    let reused = provider_summary_database(&main, &sources, cache, false);
    let reused_summary = reused.expect_get(provider_summary_query(&reused, &provider));
    assert_eq!(verified, reused_summary);
    assert_eq!(query_executions(&reused.query_trace(), "parsed_module"), 0);
}

#[test]
fn body_only_edits_reuse_item_signature_provider_summary() {
    let root = temp_dir("body_only_edits_reuse_item_signature_provider_summary");
    let sources = SourceDatabase::new();
    let main = SourcePath::new("main.nia");
    let provider = SourcePath::new("provider.nia");
    sources.set_source(main.clone(), "fn main() void {}");
    let first_file = sources.set_source(
        provider.clone(),
        "struct Widget {} extend Widget { pub fn score(&self) i32 { 1 } }",
    );
    let cache = Arc::new(crate::frontend_cache::PersistentFrontendCache::new(
        root.join("cache"),
    ));

    let first = provider_summary_database(&main, &sources, cache.clone(), false);
    let first_summary = first.expect_get(provider_summary_query(&first, &provider));
    assert_eq!(query_executions(&first.query_trace(), "parsed_module"), 1);
    let first_identity = provider_cache_identity(&first_file);

    let edited_file = sources.set_source(
        provider.clone(),
        "struct Widget {} extend Widget { pub fn score(&self) i32 { 2 + 3 } }",
    );
    let edited_identity = provider_cache_identity(&edited_file);
    assert_ne!(first_identity.source_key, edited_identity.source_key);
    assert_eq!(
        first_identity.item_signature,
        edited_identity.item_signature
    );
    assert_eq!(first_identity.provider_key, edited_identity.provider_key);

    let edited = provider_summary_database(&main, &sources, cache.clone(), false);
    let edited_summary = edited.expect_get(provider_summary_query(&edited, &provider));
    assert_eq!(first_summary, edited_summary);
    assert_eq!(query_executions(&edited.query_trace(), "parsed_module"), 1);
    assert!(matches!(
        cache
            .load_dependency_manifest(
                edited_identity.source_key,
                edited_identity.namespace,
                &edited_identity.module,
                edited_identity.source,
            )
            .expect("load edited dependency manifest"),
        crate::frontend_cache::DependencyManifestCacheLookup::Hit(item_signature)
            if item_signature == first_identity.item_signature
    ));

    let reused = provider_summary_database(&main, &sources, cache, false);
    let reused_summary = reused.expect_get(provider_summary_query(&reused, &provider));
    assert_eq!(edited_summary, reused_summary);
    assert_eq!(query_executions(&reused.query_trace(), "parsed_module"), 0);
}

#[test]
fn signature_edits_publish_distinct_provider_summaries() {
    let root = temp_dir("signature_edits_publish_distinct_provider_summaries");
    let sources = SourceDatabase::new();
    let main = SourcePath::new("main.nia");
    let provider = SourcePath::new("provider.nia");
    sources.set_source(main.clone(), "fn main() void {}");
    let first_file = sources.set_source(
        provider.clone(),
        "struct Widget {} extend Widget { pub fn score(&self) i32 { 1 } }",
    );
    let cache = Arc::new(crate::frontend_cache::PersistentFrontendCache::new(
        root.join("cache"),
    ));
    let first = provider_summary_database(&main, &sources, cache.clone(), false);
    let first_summary = first.expect_get(provider_summary_query(&first, &provider));
    assert!(first_summary.defines_inherent_associated_item(&sym("Widget"), &sym("score")));
    let first_identity = provider_cache_identity(&first_file);

    let edited_file = sources.set_source(
        provider.clone(),
        "struct Widget {} extend Widget { pub fn rank(&self) i32 { 1 } }",
    );
    let edited_identity = provider_cache_identity(&edited_file);
    assert_ne!(
        first_identity.item_signature,
        edited_identity.item_signature
    );
    assert_ne!(first_identity.provider_key, edited_identity.provider_key);

    let edited = provider_summary_database(&main, &sources, cache.clone(), false);
    let edited_summary = edited.expect_get(provider_summary_query(&edited, &provider));
    assert!(!edited_summary.defines_inherent_associated_item(&sym("Widget"), &sym("score")));
    assert!(edited_summary.defines_inherent_associated_item(&sym("Widget"), &sym("rank")));
    assert_eq!(query_executions(&edited.query_trace(), "parsed_module"), 1);
    assert!(
        cache
            .provider_summary_path(first_identity.provider_key)
            .is_file()
    );
    assert!(
        cache
            .provider_summary_path(edited_identity.provider_key)
            .is_file()
    );

    let reused = provider_summary_database(&main, &sources, cache, false);
    let reused_summary = reused.expect_get(provider_summary_query(&reused, &provider));
    assert_eq!(edited_summary, reused_summary);
    assert_eq!(query_executions(&reused.query_trace(), "parsed_module"), 0);
}

#[test]
fn provider_summary_verification_repairs_wrong_dependency_manifest() {
    let root = temp_dir("provider_summary_verification_repairs_wrong_dependency_manifest");
    let sources = SourceDatabase::new();
    let main = SourcePath::new("main.nia");
    let provider = SourcePath::new("provider.nia");
    sources.set_source(main.clone(), "fn main() void {}");
    let provider_file = sources.set_source(
        provider.clone(),
        "struct Widget {} extend Widget { pub fn score(&self) i32 { 1 } }",
    );
    let cache = Arc::new(crate::frontend_cache::PersistentFrontendCache::new(
        root.join("cache"),
    ));
    let identity = provider_cache_identity(&provider_file);
    let wrong_item_signature = ItemSignatureFingerprint::from_parts([1, 2]);
    let wrong_provider_key = FrontendProviderSummaryCacheKey::new(
        identity.namespace,
        &identity.module,
        wrong_item_signature,
    );
    cache
        .publish_dependency_manifest(
            identity.source_key,
            identity.namespace,
            &identity.module,
            identity.source,
            wrong_item_signature,
        )
        .expect("publish wrong dependency manifest");
    cache
        .publish_provider_summary(
            wrong_provider_key,
            identity.namespace,
            &identity.module,
            wrong_item_signature,
            &nia_provider_summary::ProviderSummary::default(),
            &SymbolTable::new(),
        )
        .expect("publish provider summary for wrong dependency");

    let verifying = provider_summary_database(&main, &sources, cache.clone(), true);
    let verified = verifying.expect_get(provider_summary_query(&verifying, &provider));
    assert!(verified.defines_inherent_associated_item(&sym("Widget"), &sym("score")));
    assert_eq!(
        query_executions(&verifying.query_trace(), "parsed_module"),
        1
    );
    assert!(matches!(
        cache
            .load_dependency_manifest(
                identity.source_key,
                identity.namespace,
                &identity.module,
                identity.source,
            )
            .expect("load repaired dependency manifest"),
        crate::frontend_cache::DependencyManifestCacheLookup::Hit(item_signature)
            if item_signature == identity.item_signature
    ));

    let reused = provider_summary_database(&main, &sources, cache, false);
    let reused_summary = reused.expect_get(provider_summary_query(&reused, &provider));
    assert_eq!(verified, reused_summary);
    assert_eq!(query_executions(&reused.query_trace(), "parsed_module"), 0);
}

#[test]
fn persistent_facade_facts_reuse_body_stable_entries_and_recover_from_corruption() {
    let root = temp_dir("persistent_facade_facts_reuse_body_stable_entries_and_recover");
    let sources = SourceDatabase::new();
    let main = SourcePath::new("main.nia");
    let facade = SourcePath::new("facade.nia");
    sources.set_source(main.clone(), "fn main() void {}");
    let first_file =
        sources.set_source(facade.clone(), "pub struct Widget {} fn helper() i32 { 1 }");
    let module_map = ModuleMap::default();
    let cache = Arc::new(crate::frontend_cache::PersistentFrontendCache::new(
        root.join("cache"),
    ));
    let first_identity = facade_cache_identity(&first_file, &main, &module_map);

    let first = frontend_cache_database(&main, &sources, module_map.clone(), cache.clone(), false);
    let first_facts = first.expect_get(module_facade_facts_query(&first, &facade));
    assert!(first_facts.public_type_exposes_name(&sym("Widget")));
    assert_eq!(query_executions(&first.query_trace(), "parsed_module"), 1);

    let second = frontend_cache_database(&main, &sources, module_map.clone(), cache.clone(), false);
    let second_facts = second.expect_get(module_facade_facts_query(&second, &facade));
    assert_eq!(first_facts, second_facts);
    assert_eq!(query_executions(&second.query_trace(), "parsed_module"), 0);

    let path = cache.facade_facts_path(first_identity.facade_key);
    fs::write(&path, b"corrupt facade facts").expect("corrupt facade facts entry");
    let repaired =
        frontend_cache_database(&main, &sources, module_map.clone(), cache.clone(), false);
    let repaired_facts = repaired.expect_get(module_facade_facts_query(&repaired, &facade));
    assert_eq!(first_facts, repaired_facts);
    assert_eq!(
        query_executions(&repaired.query_trace(), "parsed_module"),
        1
    );

    let edited_file = sources.set_source(
        facade.clone(),
        "pub struct Widget {} fn helper() i32 { 20 + 22 }",
    );
    let edited_identity = facade_cache_identity(&edited_file, &main, &module_map);
    assert_ne!(first_identity.source_key, edited_identity.source_key);
    assert_eq!(
        first_identity.item_signature,
        edited_identity.item_signature
    );
    assert_eq!(first_identity.facade_key, edited_identity.facade_key);
    let edited = frontend_cache_database(&main, &sources, module_map.clone(), cache.clone(), false);
    let edited_facts = edited.expect_get(module_facade_facts_query(&edited, &facade));
    assert_eq!(first_facts, edited_facts);
    assert_eq!(query_executions(&edited.query_trace(), "parsed_module"), 1);

    let reused = frontend_cache_database(&main, &sources, module_map, cache, false);
    let reused_facts = reused.expect_get(module_facade_facts_query(&reused, &facade));
    assert_eq!(edited_facts, reused_facts);
    assert_eq!(query_executions(&reused.query_trace(), "parsed_module"), 0);
}

#[test]
fn facade_facts_cache_keys_include_effective_module_map() {
    let root = temp_dir("facade_facts_cache_keys_include_effective_module_map");
    let sources = SourceDatabase::new();
    let main = SourcePath::new("main.nia");
    let facade = SourcePath::new("facade.nia");
    sources.set_source(main.clone(), "fn main() void {}");
    let facade_file = sources.set_source(facade.clone(), "pub using dep::Widget;");
    let mut mapped = ModuleMap::new();
    mapped.insert("dep", SourcePath::new("deps/root.nia"));
    let unmapped = ModuleMap::new();
    let cache = Arc::new(crate::frontend_cache::PersistentFrontendCache::new(
        root.join("cache"),
    ));
    let mapped_identity = facade_cache_identity(&facade_file, &main, &mapped);
    let unmapped_identity = facade_cache_identity(&facade_file, &main, &unmapped);
    assert_ne!(mapped_identity.module_map, unmapped_identity.module_map);
    assert_ne!(mapped_identity.facade_key, unmapped_identity.facade_key);

    let mapped_db = frontend_cache_database(&main, &sources, mapped, cache.clone(), false);
    let mapped_facts = mapped_db.expect_get(module_facade_facts_query(&mapped_db, &facade));
    assert!(mapped_facts.public_type_exposes_name(&sym("Widget")));
    assert!(matches!(
        mapped_facts.reexport_source_paths(&sym("Widget")).next(),
        Some(crate::used_paths::UsedModulePath::Package { .. })
    ));

    let unmapped_db =
        frontend_cache_database(&main, &sources, unmapped.clone(), cache.clone(), false);
    let unmapped_facts = unmapped_db.expect_get(module_facade_facts_query(&unmapped_db, &facade));
    assert!(matches!(
        unmapped_facts.reexport_source_paths(&sym("Widget")).next(),
        Some(crate::used_paths::UsedModulePath::Local { .. })
    ));
    assert_ne!(mapped_facts, unmapped_facts);
    assert_eq!(
        query_executions(&unmapped_db.query_trace(), "parsed_module"),
        1
    );

    let reused = frontend_cache_database(&main, &sources, unmapped, cache, false);
    let reused_facts = reused.expect_get(module_facade_facts_query(&reused, &facade));
    assert_eq!(unmapped_facts, reused_facts);
    assert_eq!(query_executions(&reused.query_trace(), "parsed_module"), 0);
}

#[test]
fn facade_facts_verification_replaces_semantically_wrong_valid_entry() {
    let root = temp_dir("facade_facts_verification_replaces_semantically_wrong_valid_entry");
    let sources = SourceDatabase::new();
    let main = SourcePath::new("main.nia");
    let facade = SourcePath::new("facade.nia");
    sources.set_source(main.clone(), "fn main() void {}");
    let facade_file = sources.set_source(facade.clone(), "pub struct Widget {}");
    let module_map = ModuleMap::new();
    let cache = Arc::new(crate::frontend_cache::PersistentFrontendCache::new(
        root.join("cache"),
    ));
    let identity = facade_cache_identity(&facade_file, &main, &module_map);
    cache
        .publish_dependency_manifest(
            identity.source_key,
            identity.namespace,
            &identity.module,
            identity.source,
            identity.item_signature,
        )
        .expect("publish facade dependency manifest");
    cache
        .publish_facade_facts(
            identity.namespace,
            &identity.module,
            identity.item_signature,
            identity.module_map,
            &crate::facade_facts::ModuleFacadeFacts::from_cache_parts([], Vec::new(), Vec::new()),
            &SymbolTable::new(),
        )
        .expect("publish wrong but structurally valid facade facts");

    let verifying =
        frontend_cache_database(&main, &sources, module_map.clone(), cache.clone(), true);
    let verified = verifying.expect_get(module_facade_facts_query(&verifying, &facade));
    assert!(verified.public_type_exposes_name(&sym("Widget")));
    assert_eq!(
        query_executions(&verifying.query_trace(), "parsed_module"),
        1
    );

    let reused = frontend_cache_database(&main, &sources, module_map, cache, false);
    let reused_facts = reused.expect_get(module_facade_facts_query(&reused, &facade));
    assert_eq!(verified, reused_facts);
    assert_eq!(query_executions(&reused.query_trace(), "parsed_module"), 0);
}

#[test]
fn facade_facts_cache_round_trips_all_path_processing_modes() {
    use crate::used_paths::{UsedModulePath, UsedModulePathProcessing};

    let root = temp_dir("facade_facts_cache_round_trips_all_path_processing_modes");
    let sources = SourceDatabase::new();
    let main = SourcePath::new("main.nia");
    let facade = SourcePath::new("facade.nia");
    sources.set_source(main.clone(), "fn main() void {}");
    let facade_file = sources.set_source(facade, "pub struct Widget {}");
    let module_map = ModuleMap::new();
    let identity = facade_cache_identity(&facade_file, &main, &module_map);
    let cache = crate::frontend_cache::PersistentFrontendCache::new(root.join("cache"));
    let symbol_texts = [
        "Widget", "TraitA", "TraitB", "first", "second", "dep", "segment0", "segment1", "segment2",
        "segment3", "segment4", "segment5", "segment6", "segment7",
    ];
    let symbols = symbols_for(&symbol_texts);
    let processing = [
        UsedModulePathProcessing::Never,
        UsedModulePathProcessing::Always,
        UsedModulePathProcessing::IfSelectedItem,
        UsedModulePathProcessing::IfProvidesExtensions,
        UsedModulePathProcessing::IfProvidesTraitImpl {
            trait_name: sym("TraitA"),
        },
        UsedModulePathProcessing::IfProvidesImplicitTraitImpl {
            trait_name: sym("TraitB"),
        },
        UsedModulePathProcessing::IfProvidesTraitMethod {
            target_type_name: None,
            associated_name: sym("first"),
        },
        UsedModulePathProcessing::IfProvidesTraitMethod {
            target_type_name: Some(sym("Widget")),
            associated_name: sym("second"),
        },
    ];
    let mut paths = processing
        .into_iter()
        .enumerate()
        .map(|(index, processing)| {
            let segments = vec![sym(symbol_texts[index + 6])];
            match index % 4 {
                0 => UsedModulePath::Package {
                    package: sym("dep"),
                    segments,
                    include_declared_children: index % 2 == 0,
                    processing,
                },
                1 => UsedModulePath::PackageRelative {
                    segments,
                    include_declared_children: index % 2 == 0,
                    processing,
                },
                2 => UsedModulePath::ParentRelative {
                    segments,
                    include_declared_children: index % 2 == 0,
                    processing,
                },
                _ => UsedModulePath::Local {
                    segments,
                    include_declared_children: index % 2 == 0,
                    processing,
                },
            }
        })
        .collect::<Vec<_>>();
    paths.sort();
    let facts = crate::facade_facts::ModuleFacadeFacts::from_cache_parts(
        [sym("Widget")],
        Vec::new(),
        paths,
    );
    cache
        .publish_facade_facts(
            identity.namespace,
            &identity.module,
            identity.item_signature,
            identity.module_map,
            &facts,
            &symbols,
        )
        .expect("publish facade facts path variants");
    let loaded_symbols = SymbolTable::new();

    assert!(matches!(
        cache
            .load_facade_facts(
                identity.facade_key,
                identity.namespace,
                &identity.module,
                identity.item_signature,
                identity.module_map,
                &loaded_symbols,
            )
            .expect("load facade facts path variants"),
        crate::frontend_cache::FacadeFactsCacheLookup::Hit(cached) if cached == facts
    ));
    assert_eq!(
        loaded_symbols.resolve(sym("segment7")).as_deref(),
        Some("segment7")
    );
}

#[test]
fn facade_facts_are_cached_for_reexport_and_provider_loading() {
    let root = temp_dir("facade_facts_are_cached_for_reexport_and_provider_loading");
    let main = root.join("main.nia");
    let pkg_root = root.join("pkg.nia");
    write(
        &main,
        r#"
using dep::facade;

fn first(value: facade::Widget) i32 {
    value.score()
}

fn second(value: facade::Widget) i32 {
    value.score()
}
"#,
    );
    write(&pkg_root, "pub module facade;");
    fs::create_dir_all(root.join("pkg").join("facade")).expect("create facade dir");
    write(
        &root.join("pkg/facade.nia"),
        r#"
pub(pkg) module providers;
pub(pkg) module types;

using self::providers;
pub using types::Widget;
"#,
    );
    write(
        &root.join("pkg/facade/types.nia"),
        "pub struct Widget { value: i32 }",
    );
    write(
        &root.join("pkg/facade/providers.nia"),
        r#"
using self::types;

extend types::Widget {
    pub fn score(&self) i32 { self.value }
}
"#,
    );
    let mut module_map = ModuleMap::new();
    module_map.insert("dep", SourcePath::new(pkg_root.to_string_lossy()));

    let entry_path = SourcePath::new(main.to_string_lossy());
    let database = LoaderDatabase::new(
        LoadRequest::new(main.to_string_lossy().into_owned()).with_module_map(module_map),
    );
    database
        .update_provider_demands([ProviderDemand {
            source_path: entry_path,
            request: nia_compiler_query::ProviderRequest::Method {
                target_type_name: Some(sym("Widget")),
                method_name: sym("score"),
            },
        }])
        .expect("provider graph update");
    let program = database.load_program().expect("provider program load");

    assert_no_error_diagnostics(&program);
    assert_module_loaded(
        &program,
        root.join("pkg/facade/providers.nia")
            .to_string_lossy()
            .as_ref(),
    );

    let trace = database.query_trace();
    let query = trace
        .queries
        .iter()
        .find(|query| query.frame.name == "module_facade_facts")
        .expect("facade facts query should be recorded for custom package facade");
    assert_eq!(query.stats.executions, 1, "{query:?}");
    assert!(
        query.stats.cache_hits >= 1,
        "reexport and provider loading should reuse facade facts: {query:?}"
    );
}

#[test]
fn invalidates_source_dependents_after_in_memory_text_change() {
    let sources = SourceDatabase::new();
    let main = SourcePath::new("main.nia");
    sources.set_source(main.clone(), "fn main() i32 { 0 }");
    let db = registered_query_db(test_loader_context(
        main.clone(),
        ModuleMap::default(),
        sources.clone(),
    ));

    let first = db.expect_get(LoadedProgramQuery);
    assert_no_error_diagnostics(&first);
    let first_module = first
        .modules
        .iter()
        .find(|module| module.path == main)
        .expect("loaded main module");
    let first_version = first_module.source_version;
    let first_item_tree = first_module.item_tree.clone();
    let first_item_span = first_module.item_tree.items[0].span;
    let first_node_id = first_module
        .origins
        .node_id(nia_node_id::SyntaxKind::Item, first_item_span)
        .expect("first revision item node id");
    let first_locator = db
        .context()
        .node_store
        .locator(first_node_id)
        .expect("first revision item locator");
    assert_eq!(
        first_module.origins.store_id(),
        db.context().node_store.id()
    );
    assert_eq!(first_locator.source_version(), first_version);

    let source_id = sources.id_for_path(&main);
    sources.set_source(main.clone(), "fn main() i32 { 1 }");
    let invalidation = db.retirement_transaction(|retirement| {
        let invalidation = retirement.invalidate(SourceTextQuery(source_id));
        crate::queries::retire_source_revision_queries(retirement, first_version);
        db.context().node_store.retire_revision(first_version);
        invalidation
    });
    let invalidated = invalidation
        .invalidated
        .iter()
        .map(|frame| frame.description.as_str())
        .collect::<Vec<_>>();
    let source_description = format!("source_text({source_id:?})");
    assert!(
        invalidated.contains(&source_description.as_str()),
        "{invalidated:?}"
    );
    assert!(
        invalidated
            .iter()
            .any(|description| description.starts_with("parsed_module(SourceVersion")),
        "{invalidated:?}"
    );
    assert!(
        invalidated.contains(&"loaded_program::LoadedProgramQuery"),
        "{invalidated:?}"
    );

    let second = db.expect_get(LoadedProgramQuery);
    assert_no_error_diagnostics(&second);
    let second_module = second
        .modules
        .iter()
        .find(|module| module.path == main)
        .expect("reloaded main module");
    let second_node_id = second_module
        .origins
        .node_id(
            nia_node_id::SyntaxKind::Item,
            second_module.item_tree.items[0].span,
        )
        .expect("second revision item node id");
    assert_ne!(second_module.source_version, first_version);
    assert_ne!(second_module.item_tree, first_item_tree);
    assert_ne!(second_node_id, first_node_id);
    assert_eq!(
        second_module.origins.store_id(),
        db.context().node_store.id()
    );
    assert_eq!(db.context().node_store.locator(first_node_id), None);
    assert_eq!(
        first_module
            .origins
            .locator(nia_node_id::SyntaxKind::Item, first_item_span),
        Some(first_locator)
    );
}

#[test]
fn invalidates_module_graph_after_module_declaration_text_change() {
    let sources = SourceDatabase::new();
    let main = SourcePath::new("main.nia");
    sources.set_source(main.clone(), "");
    sources.set_source(SourcePath::new("defs.nia"), "pub fn value() i32 { 1 }");
    let db = registered_query_db(test_loader_context(
        main.clone(),
        ModuleMap::default(),
        sources.clone(),
    ));

    let first = db.expect_get(LoadedProgramQuery);
    assert_no_error_diagnostics(&first);
    assert_module_loaded(&first, "main.nia");
    assert_module_not_loaded(&first, "defs.nia");
    let first_entry = first.graph.entry();

    let source_id = sources.id_for_path(&main);
    sources.set_source(main, "module defs;");
    db.invalidate(SourceTextQuery(source_id));

    let second = db.expect_get(LoadedProgramQuery);
    assert_no_error_diagnostics(&second);
    assert_ne!(second.graph.entry(), first_entry);
    assert!(
        second
            .modules
            .iter()
            .any(|module| module.path.as_str() == "defs.nia")
    );
}

#[test]
fn loader_source_update_replaces_graph_only_at_query_boundary() {
    let sources = SourceDatabase::new();
    let main = SourcePath::new("main.nia");
    sources.set_source(main.clone(), "");
    sources.set_source(SourcePath::new("defs.nia"), "pub fn value() i32 { 1 }");
    let database = LoaderDatabase::new(LoadRequest::new(main.as_str()).with_sources(sources));
    let first = database.load_program().expect("initial program load");
    let executions_before_update = query_executions(&database.query_trace(), "module_graph");

    database.set_source(main.as_str(), "module defs;");

    assert_eq!(
        query_executions(&database.query_trace(), "module_graph"),
        executions_before_update
    );
    let second = database.load_program().expect("updated program load");
    assert_ne!(second.graph.entry(), first.graph.entry());
    assert_module_loaded(&second, "defs.nia");
    assert!(query_executions(&database.query_trace(), "module_graph") > executions_before_update);
}

#[test]
fn loaded_module_query_reports_paths_outside_module_graph() {
    let sources = SourceDatabase::new();
    let db = registered_query_db(test_loader_context(
        SourcePath::new("main.nia"),
        ModuleMap::default(),
        sources.clone(),
    ));
    let missing = SourcePath::new("missing.nia");
    let missing_id = sources.id_for_path(&missing);

    let err = db
        .get(LoadedModuleQuery(missing_id))
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
