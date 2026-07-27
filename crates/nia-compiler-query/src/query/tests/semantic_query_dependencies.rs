// SPDX-License-Identifier: GPL-3.0-or-later

use super::*;

#[test]
fn module_defs_query_uses_active_item_tree_query() {
    let fixture = LoadedProgramFixture::new("main.nia", "pub struct S { value: i32 }");
    let module_id = fixture.entry_id();
    let db = query_db(fixture.program());

    let defs = db.expect_get(ModuleDefsQuery(module_id));
    let item_tree = db.expect_get(ActiveModuleItemTreeQuery(module_id));
    let item_node_key = &item_tree.items[0].node_key;
    let item_node_id = defs
        .semantic
        .def_nodes
        .node_id(item_node_key)
        .expect("definition node id");
    let trace = db.query_trace();

    assert_eq!(
        defs.semantic.def_nodes.store_id(),
        db.context().node_store.id()
    );
    assert_eq!(
        db.context().node_store.locator(item_node_id),
        Some(item_node_key.clone())
    );
    assert!(trace.dependencies.iter().any(|dependency| {
        dependency.from.name == "module_defs" && dependency.to.name == "active_module_item_tree"
    }));
    assert!(!trace.dependencies.iter().any(|dependency| {
        dependency.from.name == "module_defs" && dependency.to.name == "module_origins"
    }));
    assert!(trace.dependencies.iter().any(|dependency| {
        dependency.from.name == "active_module_item_tree"
            && dependency.to.name == "module_item_tree"
    }));
    assert!(trace.dependencies.iter().any(|dependency| {
        dependency.from.name == "module_item_tree" && dependency.to.name == "module_item_tree_input"
    }));
}

#[test]
fn body_sensitive_resolution_uses_full_active_item_tree_query() {
    let fixture = LoadedProgramFixture::new("main.nia", "fn main() i32 { let value = 1; value }");
    let module_id = fixture.entry_id();
    let db = query_db(fixture.program());

    let _ = db.expect_get(ValueResolutionQuery(module_id));
    let trace = db.query_trace();

    assert!(trace.dependencies.iter().any(|dependency| {
        dependency.from.name == "value_resolution"
            && dependency.to.name == "full_active_module_item_tree"
    }));
    assert!(trace.dependencies.iter().any(|dependency| {
        dependency.from.name == "full_active_module_item_tree"
            && dependency.to.name == "full_module_item_tree"
    }));
    assert!(trace.dependencies.iter().any(|dependency| {
        dependency.from.name == "full_module_item_tree"
            && dependency.to.name == "full_module_item_tree_input"
    }));
}

#[test]
fn value_resolution_does_not_build_visible_extensions_for_plain_paths() {
    let mut fixture = LoadedProgramFixture::new(
        "main.nia",
        r#"
module helper;

fn main() i32 {
helper::value()
}
"#,
    );
    let entry_id = fixture.entry_id();
    fixture.add_child(entry_id, "helper", "helper.nia", "pub fn value() i32 { 1 }");
    let db = query_db(fixture.program());

    let values = db.expect_get(ValueResolutionQuery(entry_id));
    let trace = db.query_trace();

    assert!(values.diagnostics.is_empty(), "{:?}", values.diagnostics);
    assert!(!trace_has_dependency(
        &trace,
        "value_resolution",
        "visible_extensions"
    ));
    assert!(!trace_has_dependency(
        &trace,
        "value_resolution",
        "extension_provider_nominal_modules"
    ));
}

#[test]
fn value_resolution_loads_visible_extensions_for_associated_values() {
    let fixture = LoadedProgramFixture::new(
        "main.nia",
        r#"
struct S {}

extend S {
const WIDTH: usize = 4usize;
}

fn main() usize {
S::WIDTH
}
"#,
    );
    let module_id = fixture.entry_id();
    let db = query_db(fixture.program());

    let values = db.expect_get(ValueResolutionQuery(module_id));
    let trace = db.query_trace();

    assert!(values.diagnostics.is_empty(), "{:?}", values.diagnostics);
    assert!(trace_has_dependency(
        &trace,
        "value_resolution",
        "visible_extensions"
    ));
}

#[test]
fn flow_check_uses_full_active_item_tree_query() {
    let fixture = LoadedProgramFixture::new("main.nia", "fn main() i32 { return 1; }");
    let module_id = fixture.entry_id();
    let db = query_db(fixture.program());

    let _ = db.expect_get(FlowCheckQuery(module_id));
    let trace = db.query_trace();

    assert!(trace.dependencies.iter().any(|dependency| {
        dependency.from.name == "flow_check" && dependency.to.name == "full_active_module_item_tree"
    }));
    assert!(trace.dependencies.iter().any(|dependency| {
        dependency.from.name == "flow_check" && dependency.to.name == "signature_item_signatures"
    }));
    assert!(!trace.dependencies.iter().any(|dependency| {
        dependency.from.name == "flow_check"
            && matches!(dependency.to.name, "item_signatures" | "type_lowering")
    }));
}

#[test]
fn static_check_uses_full_active_item_tree_query() {
    let fixture = LoadedProgramFixture::new(
        "main.nia",
        "static mut global: i32 = 1; fn main() i32 { global }",
    );
    let module_id = fixture.entry_id();
    let db = query_db(fixture.program());

    let _ = db.expect_get(StaticCheckQuery(module_id));
    let trace = db.query_trace();

    assert!(trace.dependencies.iter().any(|dependency| {
        dependency.from.name == "static_check"
            && dependency.to.name == "full_active_module_item_tree"
    }));
    assert!(trace.dependencies.iter().any(|dependency| {
        dependency.from.name == "static_check" && dependency.to.name == "signature_item_signatures"
    }));
    assert!(trace.dependencies.iter().any(|dependency| {
        dependency.from.name == "static_check" && dependency.to.name == "const_values"
    }));
    assert!(!trace.dependencies.iter().any(|dependency| {
        dependency.from.name == "static_check"
            && matches!(dependency.to.name, "item_signatures" | "const")
    }));
    assert!(
        !trace
            .dependencies
            .iter()
            .any(|dependency| dependency.from.name == "static_check"
                && dependency.to.name == "program_const")
    );
    assert!(
        !trace
            .dependencies
            .iter()
            .any(|dependency| dependency.from.name == "static_check"
                && dependency.to.name == "program_full_defs_by_id")
    );
}

#[test]
fn body_check_collects_local_signature_subsets_with_full_type_lowering() {
    let fixture = LoadedProgramFixture::new(
        "main.nia",
        "struct S { value: i32 } static mut global: i32 = 1; fn main() i32 { global }",
    );
    let module_id = fixture.entry_id();
    let db = query_db(fixture.program());

    let _ = db.expect_get(BodyCheckQuery(module_id));
    let trace = db.query_trace();

    assert!(trace.dependencies.iter().any(|dependency| {
        dependency.from.name == "body_check" && dependency.to.name == "type_lowering"
    }));
    assert!(trace.dependencies.iter().any(|dependency| {
        dependency.from.name == "body_check" && dependency.to.name == "signature_item_tree"
    }));
    assert!(trace.dependencies.iter().any(|dependency| {
        dependency.from.name == "body_check"
            && dependency.to.name == "signature_item_tree"
            && dependency.to.description.contains("ExtensionFunctions")
    }));
    assert!(trace.dependencies.iter().any(|dependency| {
        dependency.from.name == "body_check" && dependency.to.name == "const_values"
    }));
    assert!(trace.dependencies.iter().any(|dependency| {
        dependency.from.name == "body_check" && dependency.to.name == "const_array_lengths"
    }));
    assert!(!trace.dependencies.iter().any(|dependency| {
        dependency.from.name == "body_check"
            && dependency.to.name == "program_body_function_signatures"
    }));
    for query in [
        "program_body_function_signatures",
        "program_body_value_signatures",
        "program_body_type_signatures",
        "program_body_trait_signatures",
    ] {
        assert!(
            !trace_has_dependency(&trace, "body_check", query),
            "body_check should not use {query}"
        );
    }
    assert!(!trace.dependencies.iter().any(|dependency| {
        dependency.from.name == "body_check"
            && matches!(dependency.to.name, "item_signatures" | "const")
    }));
    assert!(!trace.dependencies.iter().any(|dependency| {
        dependency.from.name == "body_check"
            && dependency.to.name == "signature_item_signatures"
            && dependency.to.description.contains("ExtensionFunctions")
    }));
}

#[test]
fn body_check_reads_full_lowering_types_from_canonical_store() {
    let fixture = LoadedProgramFixture::new(
        "main.nia",
        r#"
struct Item {
state: i32,
}

fn set(items: &mut [Item], index: usize, state: i32) void {
items[index].state = state;
}

fn main() i32 {
let mut items: [2]Item = [
    { state: 1 },
    { state: 2 },
];
set(&mut items[..], 1usize, 9);
items[1].state
}
"#,
    );
    let module_id = fixture.entry_id();
    let db = query_db(fixture.program());

    let checked = db.expect_get(BodyCheckQuery(module_id));

    assert!(checked.diagnostics.is_empty(), "{:?}", checked.diagnostics);
}
