// SPDX-License-Identifier: GPL-3.0-or-later
use super::*;

#[test]
fn executable_value_refs_resolve_only_the_requested_body_item() {
    let fixture = LoadedProgramFixture::new(
        "main.nia",
        "fn helper() i32 { 1 } fn main() i32 { helper() }",
    );
    let module_id = fixture.entry_id();
    let db = query_db(fixture.program());
    let defs = db.expect_get(ModuleDefsQuery(module_id));
    let main = GlobalDefId {
        module_id,
        def_id: defs.semantic.module_scope.values.get(&sym("main")).unwrap(),
    };
    let helper = GlobalDefId {
        module_id,
        def_id: defs
            .semantic
            .module_scope
            .values
            .get(&sym("helper"))
            .unwrap(),
    };

    let edges = db.expect_get(ExecutableValueRefEdgesQuery(main));
    let trace = db.query_trace();

    assert!(edges.functions.contains(&helper), "{:?}", edges.functions);
    assert!(trace_has_dependency(
        &trace,
        "executable_value_ref_edges",
        "executable_value_ref_item"
    ));
    assert!(!trace_has_dependency(
        &trace,
        "executable_value_ref_edges",
        "value_resolution"
    ));
    assert!(trace_has_dependency(
        &trace,
        "executable_value_ref_edges",
        "full_active_module_item_tree"
    ));
    assert!(trace_has_dependency(
        &trace,
        "executable_value_ref_item",
        "executable_value_ref_item_index"
    ));
    assert!(trace_has_dependency(
        &trace,
        "executable_value_ref_item_index",
        "full_active_module_item_tree"
    ));
    assert!(trace_has_dependency(
        &trace,
        "executable_value_ref_item_index",
        "module_defs"
    ));
}

#[test]
fn executable_value_ref_item_refreshes_from_current_module_facts() {
    let source = "fn helper() i32 { 1 } fn main() i32 { helper() }";
    let mut fixture = LoadedProgramFixture::new("main.nia", source);
    let module_id = fixture.entry_id();
    let database = fixture.database();
    let defs = database.db.expect_get(ModuleDefsQuery(module_id));
    let owner = GlobalDefId {
        module_id,
        def_id: defs.semantic.module_scope.values.get(&sym("main")).unwrap(),
    };
    let first = database.db.expect_get(ExecutableValueRefItemQuery(owner));
    assert_eq!(
        first.as_ref().as_ref().unwrap().owner_node_key.revision,
        SourceRevision::INITIAL
    );

    fixture.update_module_source(module_id, source, SourceRevision(1));
    database.update(CompileRequest::new(fixture.program()));

    let latest = database.db.expect_get(ExecutableValueRefItemQuery(owner));
    assert!(!Arc::ptr_eq(&first, &latest));
    assert_eq!(
        latest.as_ref().as_ref().unwrap().owner_node_key.revision,
        SourceRevision(1)
    );
}

#[test]
fn executable_value_refs_include_unqualified_static_uses() {
    let fixture = LoadedProgramFixture::new(
        "main.nia",
        "static mut calls: i32 = 0; fn main() i32 { calls += 1; calls }",
    );
    let module_id = fixture.entry_id();
    let db = query_db(fixture.program());
    let defs = db.expect_get(ModuleDefsQuery(module_id));
    let main = GlobalDefId {
        module_id,
        def_id: defs.semantic.module_scope.values.get(&sym("main")).unwrap(),
    };
    let calls = GlobalDefId {
        module_id,
        def_id: defs
            .semantic
            .module_scope
            .values
            .get(&sym("calls"))
            .unwrap(),
    };

    let edges = db.expect_get(ExecutableValueRefEdgesQuery(main));

    assert!(edges.globals.contains(&calls), "{:?}", edges.globals);
}
