use super::*;

#[test]
fn persistent_facade_facts_reuse_body_stable_entries_and_recover_from_corruption() {
    let root = temp_dir("persistent_facade_facts_reuse_body_stable_entries_and_recover");
    let sources = SourceDatabase::new();
    let main = SourcePath::new("main.nia");
    let facade = SourcePath::new("facade.nia");
    sources.set_source(main.clone(), "fn main() () {}");
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
    sources.set_source(main.clone(), "fn main() () {}");
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
    sources.set_source(main.clone(), "fn main() () {}");
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
