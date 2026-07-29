use super::*;

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
    assert_eq!(first_dependencies.semantic.declarations.len(), 1);
    assert_eq!(
        first_dependencies.semantic.declarations[0].name,
        sym("child")
    );
    assert_eq!(query_executions(&first.query_trace(), "parsed_module"), 1);
    let first_span = first_dependencies.semantic.declarations[0].span;

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
    assert_eq!(
        edited_dependencies.semantic.declarations[0].name,
        sym("child")
    );
    assert_ne!(
        first_span,
        edited_dependencies.semantic.declarations[0].span
    );
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
        .semantic
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
        .semantic
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
