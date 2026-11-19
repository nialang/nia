use super::*;

#[test]
fn query_trace_records_source_frontend_dependencies() {
    let root = temp_dir("query_trace_records_source_frontend_dependencies");
    let main_path = root.join("main.nia");
    write(&main_path, "fun main() i32 { 0 }");
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
        dependency.from.name == "module_graph_revision"
            && dependency.to.name == "module_declarations"
    }));
}

#[test]
fn trivia_edit_reuses_the_module_graph() {
    let root = temp_dir("trivia_edit_reuses_the_module_graph");
    let main = root.join("main.nia");
    write(&main, "fun main() i32 { 0 }\n");
    write(&root.join("child.nia"), "pub fun value() i32 { 0 }\n");
    let path = main.to_string_lossy();
    let database = LoaderDatabase::new_for_test(test_request(path.as_ref()));
    let before = database.module_graph().expect("initial module graph");
    database
        .set_source(path.as_ref(), "fun main() i32 { 0 }\n// comment\n")
        .expect("edit trivia");
    let after = database.module_graph().expect("updated module graph");
    assert!(before.ptr_eq(&after));
    database
        .set_source(path.as_ref(), "module child;\nfun main() i32 { 0 }\n")
        .expect("add module declaration");
    let changed = database.module_graph().expect("graph with new module");
    assert_eq!(changed.modules().count(), before.modules().count() + 1);
}

#[test]
fn provider_updates_preserve_the_graphs_existing_source_dependencies() {
    let root = temp_dir("provider_updates_preserve_source_dependencies");
    let main = root.join("main.nia");
    let child = root.join("child.nia");
    write(&main, "module child;\nfun main() () {}\n");
    write(&child, "module nested;\n");
    write(
        &root.join("child/nested.nia"),
        "pub fun value() i32 { 0 }\n",
    );
    let database = LoaderDatabase::new_for_test(test_request(main.to_string_lossy()));
    database
        .update_provider_demands([ProviderDemand {
            source_path: SourcePath::new(main.to_string_lossy()),
            request: nia_compiler_query::ProviderRequest::Method {
                target_type_name: None,
                target_package: nia_symbol::known::STD,
                target_module_segments: Vec::new(),
                method_name: sym("unmatched"),
            },
        }])
        .expect("settle an unrelated provider demand");
    let declaration_start = |graph: &nia_imports::ModuleGraphSnapshot| {
        graph
            .modules()
            .find(|node| node.path.as_str().ends_with("/child.nia"))
            .expect("child module")
            .declarations[0]
            .span
            .start
    };
    let before = database.module_graph().unwrap();
    let prefix = "// offset the declaration\n";
    database
        .set_source(child.to_string_lossy(), format!("{prefix}module nested;\n"))
        .unwrap();
    let after = database.module_graph().unwrap();
    assert_eq!(
        declaration_start(&after),
        declaration_start(&before) + prefix.len()
    );
}

#[test]
fn provider_summary_is_cached_per_source() {
    let sources = SourceDatabase::new();
    let main = SourcePath::new("main.nia");
    let provider = SourcePath::new("provider.nia");
    sources
        .set_source(main.clone(), "fun main() () {}")
        .expect("store main source");
    sources
        .set_source(
            provider.clone(),
            r#"
struct Widget { value: i32 }

extend Widget {
    pub fun score(&self) i32 {
        self.value
    }
}
"#,
        )
        .expect("store provider source");
    let db = registered_query_db(test_loader_context(
        main.clone(),
        ModuleMap::default(),
        sources,
    ));
    let first = db.expect_get(provider_summary_query(&db, &provider));
    let second = db.expect_get(provider_summary_query(&db, &provider));
    assert!(first.defines_inherent_associated_item(&sym("Widget"), &sym("score")));
    assert_eq!(first, second);

    let trace = db.query_trace().expect("query trace");
    let query = trace
        .queries
        .iter()
        .find(|query| query.frame.name == "provider_summary")
        .expect("provider summary query should be recorded");
    assert_eq!(query.stats.executions, 1, "{query:?}");
    assert_eq!(query.stats.cache_hits, 1, "{query:?}");
}

#[test]
fn missing_provider_products_skip_syntax_and_parse_queries() {
    let sources = SourceDatabase::new();
    let main = SourcePath::new("main.nia");
    let missing_provider = SourcePath::new("missing_provider.nia");
    let db = registered_query_db(test_loader_context(main, ModuleMap::default(), sources));

    let summary = db.expect_get(provider_summary_query(&db, &missing_provider));
    let facade = db.expect_get(module_facade_facts_query(&db, &missing_provider));

    assert_eq!(
        summary.as_ref(),
        &nia_provider_summary::ProviderSummary::default()
    );
    assert!(facade.provider_source_paths().is_empty());
    let trace = db.query_trace().expect("query trace");
    assert!(
        trace
            .queries
            .iter()
            .any(|query| { query.frame.name == "provider_summary" && query.stats.executions == 1 })
    );
    assert!(
        trace
            .queries
            .iter()
            .all(|query| query.frame.name != "syntax_module")
    );
    assert!(
        trace
            .queries
            .iter()
            .all(|query| query.frame.name != "parsed_module")
    );
}

#[test]
fn missing_provider_products_refresh_when_source_appears() {
    let sources = SourceDatabase::new();
    let main = SourcePath::new("main.nia");
    let provider = SourcePath::new("provider.nia");
    let db = registered_query_db(test_loader_context(
        main,
        ModuleMap::default(),
        sources.clone(),
    ));

    let missing_summary = db.expect_get(provider_summary_query(&db, &provider));
    let missing_facade = db.expect_get(module_facade_facts_query(&db, &provider));
    assert!(!missing_summary.has_providers());
    assert!(missing_facade.provider_source_paths().is_empty());

    let source_id = sources.id_for_path(&provider).expect("allocate source id");
    sources
        .set_source(
            provider.clone(),
            r#"
pub struct Widget {}

extend Widget {
    pub fun score(&self) i32 { 1 }
}

pub using dep::Other;
"#,
        )
        .expect("store provider source");
    db.invalidate(SourceTextQuery(source_id))
        .expect("invalidate source text");

    let present_summary = db.expect_get(provider_summary_query(&db, &provider));
    let present_facade = db.expect_get(module_facade_facts_query(&db, &provider));
    assert!(present_summary.has_providers());
    assert!(!present_facade.provider_source_paths().is_empty());
}
