use super::*;

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
