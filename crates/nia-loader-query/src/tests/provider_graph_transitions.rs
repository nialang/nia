use super::*;

#[test]
fn compiler_withdraws_provider_body_when_the_last_call_is_removed() {
    let root = temp_dir("compiler_withdraws_provider_body");
    let main = root.join("main.nia");
    let package = root.join("dep/root.nia");
    let provider = root.join("dep/provider.nia");
    let without_call = "using dep;\nusing dep::provider;\nfun main(counter: &dep::Counter) i32 { _ = counter; 0 }\n";
    let with_call = without_call.replace("_ = counter; 0", "counter.measure()");
    write(&main, without_call);
    write(&package, "pub module provider;\npub struct Counter {}\n");
    write(
        &provider,
        "using root::Counter;\nextend Counter { pub fun measure(&self) i32 { 42 } }\n",
    );
    let provider_identity = SourcePath::new(provider.to_string_lossy()).identity();
    let mut module_map = ModuleMap::new();
    module_map
        .insert("dep", SourcePath::new(package.to_string_lossy()))
        .unwrap();
    let loader = LoaderDatabase::new_for_test(
        test_request(main.to_string_lossy()).with_module_map(module_map),
    );
    let compiler = CompilerDatabase::new(CompileRequest::new(loader.clone())).unwrap();
    let assert_provider_body = |expected| {
        compiler
            .update(CompileRequest::new(loader.clone()))
            .unwrap();
        let analyzed = compiler.analyze_entry_program().unwrap();
        assert!(
            !has_error_diagnostics(&analyzed.diagnostics),
            "{:?}",
            analyzed.diagnostics
        );
        let provider = analyzed
            .graph
            .module_id_for_source_identity(&provider_identity)
            .and_then(|id| analyzed.graph.get(id))
            .expect("explicitly imported provider");
        assert_eq!(provider.process_used_paths, expected);
        let facts = loader.provider_facts().unwrap();
        assert_eq!(
            facts.demands().iter().any(|demand| matches!(
                &demand.request, nia_compiler_query::ProviderRequest::ModuleBody { module_path }
                    if module_path.identity() == provider_identity
            )),
            expected
        );
        provider.id
    };

    let provider_id = assert_provider_body(false);
    loader
        .set_source(main.to_string_lossy(), with_call.clone())
        .unwrap();
    assert_eq!(assert_provider_body(true), provider_id);
    let stable = loader.provider_facts().unwrap().revision();
    let epoch = loader.source_grammar_revision().unwrap();
    loader
        .set_source(
            provider.to_string_lossy(),
            "using root::Counter;\nextend Counter { pub fun measure(&self) i32 { 13 } }\n",
        )
        .unwrap();
    assert_eq!(loader.provider_facts().unwrap().revision(), stable);
    assert_eq!(loader.source_grammar_revision().unwrap(), epoch);
    assert_eq!(assert_provider_body(true), provider_id);
    let grammar = loader.source_grammar_revision().unwrap();
    let provider_revision = loader.provider_facts().unwrap().revision();
    let graph = loader.module_graph().unwrap();
    let unrelated = root.join("unrelated.nia");
    for text in ["fun value() i32 { 1 }", "fun value() bool { true }"] {
        loader
            .set_source(unrelated.to_string_lossy(), text)
            .unwrap();
        assert_provider_body(true);
        assert_eq!(loader.source_grammar_revision().unwrap(), grammar);
        assert_eq!(
            loader.provider_facts().unwrap().revision(),
            provider_revision
        );
        assert!(loader.module_graph().unwrap().ptr_eq(&graph));
    }

    loader
        .set_source(main.to_string_lossy(), without_call)
        .unwrap();
    assert_eq!(assert_provider_body(false), provider_id);
    loader
        .set_source(main.to_string_lossy(), with_call)
        .unwrap();
    assert_eq!(assert_provider_body(true), provider_id);
}

#[test]
fn rebuilt_graphs_track_module_removal_and_reintroduction() {
    let root = temp_dir("rebuilt_graphs_track_module_removal");
    let main = root.join("main.nia");
    let main_path = main.to_string_lossy();
    let child = root.join("child.nia");
    let child_path = child.to_string_lossy();
    let source = "module child;\nfun main() i32 { child::value() }";
    write(&main, source);
    write(&child, "pub fun value() i32 { 1 }");
    let loader = LoaderDatabase::new_for_test(test_request(main_path.as_ref()));
    let before = loader.module_graph().unwrap();
    let child_id = before.module_id_for_path(&child_path).unwrap();

    loader
        .set_source(main_path.as_ref(), "fun main() i32 { 2 }")
        .unwrap();
    let without_child = loader.module_graph().unwrap();
    assert_eq!(without_child.entry(), before.entry());
    assert!(without_child.module_id_for_path(&child_path).is_none());
    assert!(without_child.get(child_id).is_none());
    assert!(before.get(child_id).is_some());

    loader.set_source(main_path.as_ref(), source).unwrap();
    let with_child = loader.module_graph().unwrap();
    assert_eq!(with_child.module_id_for_path(&child_path), Some(child_id));
    assert!(with_child.get(child_id).is_some());

    let independent = LoaderDatabase::new_for_test(test_request(main_path.as_ref()));
    let independent_graph = independent.module_graph().unwrap();
    assert!(independent_graph.module_id_for_path(&child_path).is_some());
    assert!(independent_graph.get(child_id).is_none());
}

#[test]
fn source_edit_preserves_demands_for_comments_and_retires_them_for_grammar_changes() {
    let sources = SourceDatabase::new();
    let main = SourcePath::new("main.nia");
    let alternate = SourcePath::with_identity(main.as_str(), "overlay:/main.nia");
    sources
        .set_source(alternate.clone(), "fun main() i32 { 1 }\n")
        .unwrap();
    let initial = sources
        .set_source(main.clone(), "fun main() i32 { 1 }\n")
        .unwrap();
    let database = LoaderDatabase::new_for_test(test_request(main.as_str()).with_sources(sources));
    database
        .db
        .get(crate::queries::SyntaxModuleQuery(initial.version()))
        .unwrap();
    let demand = ProviderDemand {
        source_path: main.clone(),
        request: nia_compiler_query::ProviderRequest::Method {
            target_type_name: None,
            target_package: nia_symbol::known::STD,
            target_module_segments: Vec::new(),
            method_name: sym("missing"),
        },
    };
    database.update_provider_demands([demand.clone()]).unwrap();
    let facts = &database.db.context().provider_facts;
    let revision = facts.revision_if_nonempty().unwrap();

    database
        .set_source_path(alternate, "// comment\nfun main() i32 { 1 }\n")
        .unwrap();
    assert_eq!(facts.revision_if_nonempty(), Some(revision));

    let edited = database
        .set_source(main.as_str(), "// comment\nfun main() i32 { 1 }\n")
        .unwrap();
    assert_ne!(initial.version(), edited.version());
    assert_eq!(facts.revision_if_nonempty(), Some(revision));
    assert!(facts.contains_all(&[demand]));

    database
        .set_source(main.as_str(), "// comment\nfun main() i32 { 2 }\n")
        .unwrap();
    assert_eq!(facts.revision_if_nonempty(), Some(revision));
    database
        .set_source(main.as_str(), "// comment\nfun main() i32 { f() }\n")
        .unwrap();
    assert!(facts.revision_if_nonempty().is_none());
}

#[test]
fn provider_demand_update_keeps_unmatched_and_known_demands_graph_stable() {
    let root = temp_dir("provider_demand_update_keeps_unmatched_and_known_demands_graph_stable");
    let main_path = root.join("main.nia");
    write(&main_path, "fun main() () {}");
    let database =
        LoaderDatabase::new_for_test(test_request(main_path.to_string_lossy().into_owned()));
    let demand = ProviderDemand {
        source_path: SourcePath::new(main_path.to_string_lossy()),
        request: nia_compiler_query::ProviderRequest::Method {
            target_type_name: None,
            target_package: nia_symbol::known::STD,
            target_module_segments: Vec::new(),
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
            .expect("query trace")
            .queries
            .iter()
            .all(|query| query.frame.name != "loaded_program"),
        "a stable graph should not rebuild the aggregate loaded program"
    );
    let trace = database.query_trace().expect("query trace");
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
    let mut initial = ModuleGraph::new(SourcePath::new("main.nia")).expect("create module graph");
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
    let mut rebuilt = ModuleGraph::new(SourcePath::new("main.nia")).expect("create module graph");
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
fn body_provider_demand_uses_canonical_source_identity() {
    let root = temp_dir("body_provider_demand_uses_canonical_source_identity");
    let main_path = root.join("main.nia");
    write(
        &main_path,
        "using std::process; fun main(value: process::Init) () { _ = value; }",
    );
    let database = LoaderDatabase::new_for_test(
        LoadRequest::new(main_path.to_string_lossy())
            .with_runtime(test_freestanding_runtime())
            .with_toolchain_layout(test_toolchain_layout()),
    );
    let initial = database.db.expect_get(crate::graph::ModuleGraphQuery);
    let child = initial
        .semantic
        .modules()
        .find(|module| module.path.as_str().ends_with("lib/std/process.nia"))
        .expect("shallow std process facade")
        .clone();
    assert_ne!(
        child.path.as_str(),
        child.path.identity().normalized_path(),
        "toolchain modules must exercise distinct physical and logical identities"
    );
    assert!(!child.process_used_paths);

    assert_eq!(
        database
            .update_provider_demands([ProviderDemand {
                source_path: child.path.clone(),
                request: nia_compiler_query::ProviderRequest::ModuleBody {
                    module_path: child.path.clone(),
                },
            }])
            .expect("activate child body"),
        ProviderGraphUpdate::Changed {
            invalidates_resolved_body_facts: false,
        }
    );
    let updated = database.db.expect_get(crate::graph::ModuleGraphQuery);
    assert!(
        updated
            .semantic
            .get(child.id)
            .expect("activated child module")
            .process_used_paths
    );
}

#[test]
fn query_loader_does_not_load_a_declared_provider_without_an_explicit_using_edge() {
    let root =
        temp_dir("query_loader_does_not_load_a_declared_provider_without_an_explicit_using_edge");
    let main_path = root.join("main.nia");
    let pkg_root = root.join("root.nia");
    write(
        &main_path,
        r#"
using dep::facade;

fun main(value: facade::Widget) i32 {
_ = value;
0
}
"#,
    );
    write(&pkg_root, "pub module facade;");
    fs::create_dir_all(root.join("facade")).expect("create package dir");
    write(
        &root.join("facade.nia"),
        r#"
pub(root) module providers;
pub(root) module types;

pub using types::Widget;
"#,
    );
    write(
        &root.join("facade/types.nia"),
        r#"pub struct Widget { value: i32 }"#,
    );
    write(
        &root.join("facade/providers.nia"),
        r#"
using self::types;

extend types::Widget {
pub fun score(&self) i32 {
    self.value
}
}
"#,
    );
    let provider_path = root.join("facade/providers.nia");
    let mut module_map = ModuleMap::new();
    module_map
        .insert("dep", SourcePath::new(pkg_root.to_string_lossy()))
        .expect("insert module map entry");
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
