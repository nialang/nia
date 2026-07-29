use super::*;

#[test]
fn loader_diagnostic_queries_reuse_session_payload_handles() {
    let sources = SourceDatabase::new();
    let main = SourcePath::new("missing.nia");
    let db = registered_query_db(test_loader_context(
        main.clone(),
        ModuleMap::default(),
        sources,
    ));

    let parsed = db.expect_get(parsed_module_query(&db, &main));
    let declarations = db.expect_get(module_declarations_query(&db, &main));

    assert!(!parsed.read_diagnostics.is_empty());
    assert_eq!(declarations.diagnostics.len(), 1);
    assert_eq!(
        parsed.read_diagnostics.id(),
        declarations.diagnostics[0].id()
    );
    assert!(
        db.context()
            .diagnostic_store
            .diagnostics(&parsed.read_diagnostics)
            .is_some_and(|diagnostics| diagnostics.len() == 1)
    );
    let load_diagnostics = db.expect_get(crate::queries::LoadDiagnosticsQuery);
    assert!(
        load_diagnostics.to_diagnostics()[0]
            .diagnostic
            .summary
            .contains("failed to read")
    );
}

#[test]
fn retired_loader_diagnostic_handles_remain_readable() {
    let sources = SourceDatabase::new();
    let main = SourcePath::new("main.nia");
    let first_file = sources.set_source(main.clone(), "module child; module child;");
    let db = registered_query_db(test_loader_context(
        main.clone(),
        ModuleMap::default(),
        sources.clone(),
    ));
    let first = db.expect_get(module_declarations_query(&db, &main));
    let retained = first.diagnostics[0].clone();
    assert!(!retained.is_empty());

    let second_file = sources.set_source(main.clone(), "");
    db.retirement_transaction(|retirement| {
        retirement.invalidate(SourceTextQuery(first_file.id));
        crate::queries::retire_source_revision_queries(retirement, first_file.version());
        db.context()
            .node_store
            .retire_revision(first_file.version());
    });
    let second = db.expect_get(module_declarations_query(&db, &main));

    assert_eq!(second_file.id, first_file.id);
    assert!(second.diagnostics.is_empty());
    assert!(
        db.context()
            .diagnostic_store
            .diagnostics(&retained)
            .is_some_and(|diagnostics| diagnostics.len() == 1)
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

    let first = db.expect_get(LoadedProgramQuery).to_program();
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

    let second = db.expect_get(LoadedProgramQuery).to_program();
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

    let first = db.expect_get(LoadedProgramQuery).to_program();
    assert_no_error_diagnostics(&first);
    assert_module_loaded(&first, "main.nia");
    assert_module_not_loaded(&first, "defs.nia");
    let first_entry = first.graph.entry();

    let source_id = sources.id_for_path(&main);
    sources.set_source(main, "module defs;");
    db.invalidate(SourceTextQuery(source_id));

    let second = db.expect_get(LoadedProgramQuery).to_program();
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
