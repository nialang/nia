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
    nia_defs::collect_module_defs(module_id, &module)
}

fn graph_with_public_children(children: &[&str]) -> ModuleGraph {
    let mut graph = ModuleGraph::new(SourcePath::new("main.nia"));
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
