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

#[path = "tests/freestanding_runtime.rs"]
mod freestanding_runtime;
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
#[path = "tests/public_surface_persistence.rs"]
mod public_surface_persistence;
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
