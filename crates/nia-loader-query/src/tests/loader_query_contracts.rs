// SPDX-License-Identifier: GPL-3.0-or-later

use super::*;

#[test]
fn loader_query_registry_covers_all_declared_query_contracts() {
    let descriptors = crate::loader_query_registry().descriptors();

    assert_eq!(descriptors.len(), 18);
    assert!(
        descriptors
            .windows(2)
            .all(|pair| pair[0].name < pair[1].name)
    );
    assert!(descriptors.iter().all(|descriptor| {
        let expected_fingerprint = match descriptor.name {
            "source_status" => nia_query::QueryFingerprintPolicy::StableValue,
            "provider_demands"
            | "loader_active_module_item_tree_fact"
            | "loader_module_item_tree_fact"
            | "loader_module_origins_fact"
            | "loader_module_parse_errors_fact" => nia_query::QueryFingerprintPolicy::SemanticValue,
            _ => nia_query::QueryFingerprintPolicy::None,
        };
        descriptor.context_type == std::any::type_name::<LoaderContext>()
            && descriptor.provider == nia_query::QueryProviderPolicy::KeyExecute
            && descriptor.fingerprint == expected_fingerprint
            && descriptor.storage == nia_query::QueryStoragePolicy::CacheOwnedArc
    }));
}

#[test]
fn source_status_tracks_missing_and_present_revisions() {
    let sources = SourceDatabase::new();
    let main = SourcePath::new("main.nia");
    let source_id = sources.id_for_path(&main);
    let db = registered_query_db(test_loader_context(
        main.clone(),
        ModuleMap::default(),
        sources.clone(),
    ));

    let missing = db.expect_get(SourceStatusQuery(source_id));
    assert_eq!(*missing, SourceStatus::Missing);
    let file = sources.set_source(main, "fn main() i32 { 0 }");
    db.invalidate(SourceTextQuery(source_id));
    let present = db.expect_get(SourceStatusQuery(source_id));

    assert!(!Arc::ptr_eq(&missing, &present));
    assert_eq!(*present, SourceStatus::Present(file.version()));
}

#[test]
fn source_products_propagate_unknown_source_query_failures() {
    let sources = SourceDatabase::new();
    let main = SourcePath::new("main.nia");
    let db = registered_query_db(test_loader_context(main, ModuleMap::default(), sources));
    let unknown = SourceId(u32::MAX);

    for error in [
        db.get(SourceStatusQuery(unknown))
            .expect_err("unknown source status must fail"),
        db.get(LoadedModuleQuery(unknown))
            .expect_err("unknown loaded module must propagate its source failure"),
    ] {
        assert!(matches!(error, nia_query::QueryError::InvalidInput { .. }));
    }
}

#[test]
fn source_updates_remove_old_revision_owners_and_detach_external_snapshot() {
    let sources = SourceDatabase::new();
    let main = SourcePath::new("main.nia");
    let first_file = sources.set_source(main.clone(), "fn main() i32 { 0 }");
    let database = LoaderDatabase::new(LoadRequest::new(main.as_str()).with_sources(sources));
    assert_no_error_diagnostics(&database.load_program().expect("initial program load"));
    let first_version = first_file.version();
    let old_parsed = database
        .db
        .expect_get(parsed_module_query(&database.db, &main));
    let old_item_span = old_parsed.item_tree.items[0].span;
    let old_item_id = old_parsed
        .origins
        .node_id(nia_node_id::SyntaxKind::Item, old_item_span)
        .expect("first revision item node id");
    let old_item_locator = old_parsed
        .origins
        .locator(nia_node_id::SyntaxKind::Item, old_item_span)
        .expect("first revision item locator");
    database
        .db
        .expect_get(module_declarations_query(&database.db, &main));
    database
        .db
        .expect_get(provider_summary_query(&database.db, &main));
    database
        .db
        .expect_get(module_facade_facts_query(&database.db, &main));
    let initial_query_count = database.query_trace().queries.len();
    let initial_node_count = database.db.context().node_store.len();
    assert_eq!(database.db.context().node_store.active_revision_count(), 1);

    let mut latest_version = first_version;
    for revision in 1..=100 {
        let file = database.set_source(main.as_str(), format!("fn main() i32 {{ {revision} }}"));
        latest_version = file.version();
        assert_no_error_diagnostics(&database.load_program().expect("updated program load"));
        database
            .db
            .expect_get(parsed_module_query(&database.db, &main));
        database
            .db
            .expect_get(module_declarations_query(&database.db, &main));
        database
            .db
            .expect_get(provider_summary_query(&database.db, &main));
        database
            .db
            .expect_get(module_facade_facts_query(&database.db, &main));
        assert_eq!(database.db.context().node_store.active_revision_count(), 1);
        assert_eq!(database.db.context().node_store.len(), initial_node_count);
    }

    let trace = database.query_trace();
    assert_eq!(trace.queries.len(), initial_query_count);
    for name in [
        "parsed_module",
        "syntax_module",
        "module_declarations",
        "provider_summary",
        "module_facade_facts",
    ] {
        let queries = trace
            .queries
            .iter()
            .filter(|query| query.frame.name == name)
            .collect::<Vec<_>>();
        assert_eq!(queries.len(), 1, "{name}: {queries:?}");
        assert!(
            queries[0]
                .frame
                .key
                .contains(&format!("revision: {:?}", latest_version.revision)),
            "{}",
            queries[0].frame.key
        );
    }
    assert_eq!(old_parsed.source_version, first_version);
    assert_eq!(old_parsed.item_tree.items.len(), 1);
    assert_eq!(database.db.context().node_store.locator(old_item_id), None);
    assert_eq!(
        old_parsed
            .origins
            .locator(nia_node_id::SyntaxKind::Item, old_item_span),
        Some(old_item_locator)
    );
}

#[test]
fn provider_add_and_reset_keep_graph_revision_storage_bounded() {
    let sources = SourceDatabase::new();
    let main = SourcePath::new("main.nia");
    sources.set_source(main.clone(), "fn main() i32 { 0 }");
    let database = LoaderDatabase::new(LoadRequest::new(main.as_str()).with_sources(sources));
    let initial_graph = database.db.expect_get(crate::graph::ModuleGraphQuery);
    let initial_entry = initial_graph.entry();

    for revision in 1..=100 {
        assert_eq!(
            database
                .update_provider_demands([ProviderDemand {
                    source_path: main.clone(),
                    request: nia_compiler_query::ProviderRequest::Method {
                        target_type_name: None,
                        method_name: sym(&format!("missing_{revision}")),
                    },
                }])
                .expect("provider graph update"),
            ProviderGraphUpdate::Stable
        );
        assert_eq!(
            database
                .db
                .context()
                .provider_facts
                .retained_transition_count(),
            0
        );
        assert_eq!(
            database
                .query_trace()
                .queries
                .iter()
                .filter(|query| query.frame.name == "module_graph_revision")
                .count(),
            1
        );

        database.set_source(main.as_str(), format!("fn main() i32 {{ {revision} }}"));
        let graph = database.db.expect_get(crate::graph::ModuleGraphQuery);
        assert_eq!(graph.get(graph.entry()).map(|node| &node.path), Some(&main));
        assert_eq!(
            database
                .query_trace()
                .queries
                .iter()
                .filter(|query| query.frame.name == "module_graph_revision")
                .count(),
            1
        );
    }

    assert_eq!(initial_graph.entry(), initial_entry);
    assert_eq!(
        initial_graph
            .get(initial_graph.entry())
            .map(|node| &node.path),
        Some(&main)
    );
    assert_eq!(initial_graph.modules().count(), 1);
}

#[test]
fn compiler_loader_roots_record_cross_database_dependencies() {
    let sources = SourceDatabase::new();
    sources.set_source(SourcePath::new("main.nia"), "fn main() i32 { 0 }");
    let loader = LoaderDatabase::new(LoadRequest::new("main.nia").with_sources(sources));
    let compiler = CompilerDatabase::new(CompileRequest::new(loader.clone()));
    assert!(compiler.query_session().ptr_eq(&loader.query_session()));

    let checked = compiler.check_program().expect("compiler check");
    let _ = compiler.provider_fact_revision();

    assert!(!has_error_diagnostics(&checked.diagnostics));
    let trace = compiler.query_trace();
    assert!(trace.dependencies.iter().any(|dependency| {
        dependency.from.name == "loaded_modules" && dependency.to.name == "module_graph"
    }));
    assert!(trace.dependencies.iter().any(|dependency| {
        dependency.from.name == "program_load_diagnostics"
            && dependency.to.name == "load_diagnostics"
    }));
    assert!(trace.dependencies.iter().any(|dependency| {
        dependency.from.name == "module_path" && dependency.to.name == "module_graph"
    }));
    assert!(trace.dependencies.iter().any(|dependency| {
        dependency.from.name == "module_source_version" && dependency.to.name == "source_status"
    }));
    assert!(trace.dependencies.iter().any(|dependency| {
        dependency.from.name == "extension_provider_summary"
            && dependency.to.name == "provider_summary"
    }));
    assert!(trace.dependencies.iter().any(|dependency| {
        dependency.from.name == "module_item_tree_input"
            && dependency.to.name == "loader_module_item_tree_fact"
    }));
    assert!(trace.dependencies.iter().any(|dependency| {
        dependency.from.name == "signature_item_tree"
            && dependency.to.name == "loader_active_module_item_tree_fact"
    }));
    assert!(trace.dependencies.iter().any(|dependency| {
        dependency.from.name == "module_origins"
            && dependency.to.name == "loader_module_origins_fact"
    }));
    assert!(trace.dependencies.iter().any(|dependency| {
        dependency.from.name == "module_parse_errors"
            && dependency.to.name == "loader_module_parse_errors_fact"
    }));
    assert!(trace.dependencies.iter().any(|dependency| {
        dependency.from.name == "public_surface_module_facts"
            && dependency.to.name == "loader_public_surface_module_facts"
    }));
    assert!(trace.dependencies.iter().any(|dependency| {
        dependency.from.name == "provider_fact_worklist" && dependency.to.name == "provider_demands"
    }));
}
