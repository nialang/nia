use super::*;

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
    let remapped_provider = rebuilt
        .module_id_for_source_identity(&provider_path.identity())
        .expect("remapped provider module");
    rebuilt.mark_semantic_selected(remapped_provider);
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
