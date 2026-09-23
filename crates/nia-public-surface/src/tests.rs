use super::*;
use nia_defs::Visibility;
use nia_imports::{ModuleGraph, SourcePath};
use nia_symbol::stable_hash;

fn name(text: &str) -> SymbolId {
    SymbolId::from_stable_hash(stable_hash(text))
}

fn defs(module_id: ModuleId, source: &str) -> DefCollection {
    let (module, errors) = nia_parser::parse_module(source);
    assert!(errors.is_empty(), "{errors:?}");
    nia_defs::collect_module_defs(module_id, &module).expect("collect definitions")
}

fn graph_with_public_children(children: &[&str]) -> ModuleGraph {
    let mut graph = ModuleGraph::new(SourcePath::new("main.nia")).expect("create module graph");
    for child in children {
        let child = name(child);
        graph
            .intern_declared_child(graph.entry(), &child, Visibility::Public, Span::default())
            .expect("child declaration");
    }
    graph
}

#[test]
fn wildcard_reexports_preserve_item_name_spans_for_duplicate_diagnostics() {
    let graph = graph_with_public_children(&["left", "right"]);
    let entry_id = graph.entry();
    let left_id = graph
        .root_module_for_name(entry_id, name("left"))
        .expect("left child");
    let right_id = graph
        .root_module_for_name(entry_id, name("right"))
        .expect("right child");
    let main = defs(
        entry_id,
        r#"
pub module left;
pub module right;
using { left::*, right::* };
"#,
    );
    let left = defs(left_id, "pub fn value() i32 { 1 }");
    let right = defs(right_id, "pub fn value() i32 { 2 }");

    let (_, _, diagnostics) = compute_public_surfaces(&[main, left, right], &graph);

    assert_eq!(diagnostics.len(), 1);
    assert!(diagnostics[0].1.summary.contains("duplicate using name"));
    assert_ne!(diagnostics[0].1.primary_span(), Some(Span::default()));
    assert_eq!(diagnostics[0].1.related.len(), 2);
    assert!(
        diagnostics[0].1.related[0]
            .message
            .contains("imported here first")
    );
    assert!(
        diagnostics[0].1.related[1]
            .message
            .contains("earlier `using` directive")
    );
}

#[test]
fn self_selector_requires_named_host_segment() {
    let graph = graph_with_public_children(&[]);
    let main = defs(graph.entry(), "using self;");

    let (_, _, diagnostics) = compute_public_surfaces(&[main], &graph);

    assert_eq!(diagnostics.len(), 1);
    assert!(diagnostics[0].1.summary.contains("using `self` requires"));
}

#[test]
fn type_exposure_index_collects_direct_public_and_using_names() {
    let mut graph = graph_with_public_children(&["facade"]);
    let entry_id = graph.entry();
    let facade_id = graph
        .root_module_for_name(entry_id, name("facade"))
        .expect("facade child");
    let types_id = graph
        .intern_declared_child(
            facade_id,
            &name("types"),
            Visibility::Public,
            Span::default(),
        )
        .expect("types child declaration");
    let main = defs(
        entry_id,
        r#"
pub module facade;
using entry::facade::FacadeUsed as LocalUsed;
"#,
    );
    let facade = defs(
        facade_id,
        r#"
pub module types;
pub using self::types::Used as FacadeUsed;
"#,
    );
    let types = defs(types_id, "pub struct Used {}");
    let used_def_id = types
        .module_scope
        .types
        .get(&name("Used"))
        .expect("Used def");
    let defs_by_module = vec![main, facade, types];

    let exported = compute_exported_public_surfaces(&defs_by_module, &graph);
    let using_scopes =
        compute_using_scopes_from_surfaces(&defs_by_module, &graph, &exported.surfaces);
    let index = TypeExposureIndex::from_defs_surfaces_and_using_scopes(
        &defs_by_module,
        &exported.surfaces,
        &using_scopes.using_scopes,
    );

    let names = index.names_for(GlobalDefId {
        module_id: types_id,
        def_id: used_def_id,
    });

    let mut expected = vec![name("FacadeUsed"), name("LocalUsed"), name("Used")];
    expected.sort();
    expected.dedup();
    assert_eq!(names, expected.as_slice());
}

struct UsingGroupFixture {
    scope: ModuleUsingScope,
    diagnostics: Vec<Diagnostic>,
    source: &'static str,
}

impl UsingGroupFixture {
    fn new(main_source: &'static str) -> Self {
        let graph = graph_with_public_children(&["api"]);
        let entry_id = graph.entry();
        let api_id = graph
            .root_module_for_name(entry_id, name("api"))
            .expect("api child");
        let main = defs(entry_id, main_source);
        let api = defs(api_id, "pub fn present() i32 { 1 }");
        let (_, mut scopes, diagnostics) = compute_public_surfaces(&[main, api], &graph);
        Self {
            scope: scopes.remove(&entry_id).expect("entry scope"),
            diagnostics: diagnostics
                .into_iter()
                .map(|(_, diagnostic)| diagnostic)
                .collect(),
            source: main_source,
        }
    }

    fn span(&self, text: &str) -> Span {
        let start = self.source.find(text).expect("fixture text");
        Span::new(start, start + text.len())
    }

    fn primary_spans(&self) -> Vec<Span> {
        self.diagnostics
            .iter()
            .map(|diagnostic| diagnostic.primary_span().expect("primary span"))
            .collect()
    }
}

#[test]
fn failed_group_selector_keeps_resolved_siblings() {
    let fixture = UsingGroupFixture::new("pub module api;\nusing api::{present, absent};\n");

    assert!(fixture.scope.values.contains_key(&name("present")));
    assert!(
        !fixture
            .scope
            .unresolved_usings
            .contains_key(&name("present"))
    );
    assert_eq!(
        fixture.scope.unresolved_usings[&name("absent")].name_span,
        fixture.span("absent")
    );
    assert_eq!(fixture.primary_spans(), [fixture.span("absent")]);
}

#[test]
fn every_failed_group_selector_is_an_independent_root() {
    let fixture = UsingGroupFixture::new("pub module api;\nusing api::{absent, present, gone};\n");

    assert!(fixture.scope.values.contains_key(&name("present")));
    assert_eq!(
        fixture.primary_spans(),
        [fixture.span("absent"), fixture.span("gone")]
    );
}

#[test]
fn invalid_public_reexport_path_is_reported() {
    let fixture = UsingGroupFixture::new("pub module api;\npub using api::nested::absent;\n");

    assert_eq!(fixture.primary_spans(), [fixture.span("nested")]);
    assert!(fixture.diagnostics[0].summary.contains("unknown namespace"));
}

#[test]
fn hidden_child_module_reports_visibility_with_its_declaration() {
    let mut graph = graph_with_public_children(&["api"]);
    let entry_id = graph.entry();
    let api_id = graph
        .root_module_for_name(entry_id, name("api"))
        .expect("api child");
    let hidden_declaration = Span::new(0, "module hidden;".len());
    let hidden_id = graph
        .intern_declared_child(
            api_id,
            &name("hidden"),
            Visibility::Private,
            hidden_declaration,
        )
        .expect("hidden child");
    let source = "pub module api;\nusing api::hidden;\nusing api::hidden::value;\n";
    let main = defs(entry_id, source);
    let api = defs(api_id, "module hidden;");
    let hidden = defs(hidden_id, "pub fn value() i32 { 1 }");

    let (_, mut scopes, diagnostics) = compute_public_surfaces(&[main, api, hidden], &graph);
    let scope = scopes.remove(&entry_id).expect("entry scope");

    for local in ["hidden", "value"] {
        let unresolved = &scope.unresolved_usings[&name(local)];
        assert_eq!(
            unresolved.reason,
            UnresolvedUsingReason::NamespaceNotVisible
        );
    }
    let hidden_segments = source
        .match_indices("hidden")
        .map(|(start, text)| Span::new(start, start + text.len()))
        .collect::<Vec<_>>();
    let primaries = diagnostics
        .iter()
        .map(|(_, diagnostic)| diagnostic.primary_span().expect("primary span"))
        .collect::<Vec<_>>();
    assert_eq!(primaries, hidden_segments, "{diagnostics:?}");
    for (_, diagnostic) in &diagnostics {
        assert!(
            diagnostic
                .related
                .iter()
                .any(|related| related.span == hidden_declaration
                    && related.message.contains("without public visibility")),
            "{diagnostic:?}"
        );
    }
}

#[test]
fn using_through_unloaded_module_defers_to_the_load_error() {
    let graph = graph_with_public_children(&["api"]);
    let entry_id = graph.entry();
    let main = defs(
        entry_id,
        "pub module api;\nusing api::value;\nusing api::nested::item;\n",
    );

    // `api` is declared in the graph but has no loaded definitions.
    let (_, mut scopes, diagnostics) = compute_public_surfaces(&[main], &graph);
    let scope = scopes.remove(&entry_id).expect("entry scope");

    assert!(diagnostics.is_empty(), "{diagnostics:?}");
    for local in ["value", "item"] {
        assert_eq!(
            scope.unresolved_usings[&name(local)].reason,
            UnresolvedUsingReason::ModuleUnavailable
        );
    }
}
