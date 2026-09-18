use super::*;

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
    sources.set_source(main.clone(), "fn main() () {}");
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

    let source_id = sources.id_for_path(&provider);
    sources.set_source(
        provider.clone(),
        r#"
pub struct Widget {}

extend Widget {
    pub fn score(&self) i32 { 1 }
}

pub using dep::Other;
"#,
    );
    db.invalidate(SourceTextQuery(source_id))
        .expect("invalidate source text");

    let present_summary = db.expect_get(provider_summary_query(&db, &provider));
    let present_facade = db.expect_get(module_facade_facts_query(&db, &provider));
    assert!(present_summary.has_providers());
    assert!(!present_facade.provider_source_paths().is_empty());
}
