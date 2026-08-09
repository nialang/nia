// SPDX-License-Identifier: GPL-3.0-or-later
use super::*;

#[test]
fn function_body_update_refreshes_handles_but_keeps_public_snapshots_green() {
    let mut fixture = LoadedProgramFixture::new(
        "main.nia",
        "pub struct S { value: i32 } fn main() i32 { 0 }",
    );
    let module_id = fixture.entry_id();
    let database = fixture.database();

    let first = database.check_program();
    assert!(first.diagnostics.is_empty(), "{:?}", first.diagnostics);
    let first_using_scope = database.db.expect_get(ModuleUsingScopeQuery(module_id));

    fixture.update_module_source(
        module_id,
        "pub struct S { value: i32 } fn main() i32 { 1 }",
        SourceRevision(1),
    );
    database.update(CompileRequest::new(fixture.program()));

    let second = database.analyze_program();
    assert!(second.diagnostics.is_empty(), "{:?}", second.diagnostics);
    let latest_using_scope = database.db.expect_get(ModuleUsingScopeQuery(module_id));
    assert!(Arc::ptr_eq(&first_using_scope, &latest_using_scope));
}

#[test]
fn function_body_type_update_refreshes_revision_bearing_signature_queries() {
    let mut fixture = LoadedProgramFixture::new(
        "main.nia",
        "pub struct S { value: i32 } fn main() i32 { let value: i32 = 0; value }",
    );
    let module_id = fixture.entry_id();
    let database = fixture.database();

    let first = database.check_program();
    assert!(first.diagnostics.is_empty(), "{:?}", first.diagnostics);

    fixture.update_module_source(
        module_id,
        "pub struct S { value: i32 } fn main() i32 { let value: u8 = 0; value as i32 }",
        SourceRevision(1),
    );
    database.update(CompileRequest::new(fixture.program()));

    let second = database.check_program();
    assert!(second.diagnostics.is_empty(), "{:?}", second.diagnostics);
}

#[test]
fn body_local_type_update_reuses_program_body_signature_indexes() {
    let mut fixture = LoadedProgramFixture::new(
        "main.nia",
        "pub struct S { value: i32 } fn main() i32 { let value: i32 = 0; value }",
    );
    let module_id = fixture.entry_id();
    let database = fixture.database();

    let first = database.check_program();
    assert!(first.diagnostics.is_empty(), "{:?}", first.diagnostics);

    fixture.update_module_source(
        module_id,
        "pub struct S { value: i32 } fn main() i32 { let value: u8 = 0; value as i32 }",
        SourceRevision(1),
    );
    database.update(CompileRequest::new(fixture.program()));
    let before_second_check = database.query_trace();

    let second = database.check_program();
    assert!(second.diagnostics.is_empty(), "{:?}", second.diagnostics);
    let after_second_check = database.query_trace();

    assert_query_executions_unchanged(
        &before_second_check,
        &after_second_check,
        "extension_provider_discovery_index",
    );
}

#[test]
fn function_signature_update_refreshes_revision_bearing_definition_queries() {
    let mut fixture = LoadedProgramFixture::new(
        "main.nia",
        "pub struct S { value: i32 } fn helper() i32 { 1 } fn main() i32 { helper() }",
    );
    let module_id = fixture.entry_id();
    let database = fixture.database();

    let first = database.codegen_program();
    assert!(first.diagnostics.is_empty(), "{:?}", first.diagnostics);

    fixture.update_module_source(
        module_id,
        "pub struct S { value: i32 } fn helper() u8 { 1 } fn main() i32 { helper() as i32 }",
        SourceRevision(1),
    );
    database.update(CompileRequest::new(fixture.program()));
}

#[test]
fn tuple_element_order_and_type_updates_invalidate_signature_lowering() {
    let mut fixture = LoadedProgramFixture::new(
        "main.nia",
        "pub fn take(value: (i32, bool)) i32 { value.0 } fn main() i32 { 0 }",
    );
    let module_id = fixture.entry_id();
    let database = fixture.database();

    let first = database.check_program();
    assert!(first.diagnostics.is_empty(), "{:?}", first.diagnostics);
    let after_first = database.query_trace();

    fixture.update_module_source(
        module_id,
        "pub fn take(value: (bool, i32)) i32 { value.1 } fn main() i32 { 0 }",
        SourceRevision(1),
    );
    database.update(CompileRequest::new(fixture.program()));
    let reordered = database.check_program();
    assert!(
        reordered.diagnostics.is_empty(),
        "{:?}",
        reordered.diagnostics
    );
    let after_reorder = database.query_trace();
    assert!(
        query_executions(&after_first, "signature_type_lowering")
            < query_executions(&after_reorder, "signature_type_lowering")
    );

    fixture.update_module_source(
        module_id,
        "pub fn take(value: (bool, u8)) u8 { value.1 } fn main() i32 { 0 }",
        SourceRevision(2),
    );
    database.update(CompileRequest::new(fixture.program()));
    let changed = database.check_program();
    assert!(changed.diagnostics.is_empty(), "{:?}", changed.diagnostics);
    let after_type_change = database.query_trace();
    assert!(
        query_executions(&after_reorder, "signature_type_lowering")
            < query_executions(&after_type_change, "signature_type_lowering")
    );
}

#[test]
fn function_body_type_update_refreshes_signature_program_type_context() {
    let mut fixture =
        LoadedProgramFixture::new("main.nia", "fn main() i32 { let value: i32 = 0; value }");
    let entry_id = fixture.entry_id();
    fixture.add_child(entry_id, "helper", "helper.nia", "fn helper() i32 { 1 }");
    let database = fixture.database();

    let first = database.check_program();
    assert!(first.diagnostics.is_empty(), "{:?}", first.diagnostics);

    fixture.update_module_source(
        entry_id,
        "fn main() i32 { let value: u8 = 0; value as i32 }",
        SourceRevision(1),
    );
    database.update(CompileRequest::new(fixture.program()));
    let before_second_check = database.query_trace();

    let second = database.check_program();
    assert!(second.diagnostics.is_empty(), "{:?}", second.diagnostics);
    let after_second_check = database.query_trace();

    assert!(
        query_executions(&before_second_check, "signature_type_normalization")
            < query_executions(&after_second_check, "signature_type_normalization")
    );
}

#[test]
fn source_identity_update_invalidates_module_dependent_queries() {
    let mut fixture = LoadedProgramFixture::new(
        "main.nia",
        "pub struct S { value: i32 } fn main() i32 { 0 }",
    );
    let module_id = fixture.entry_id();
    let database = fixture.database();

    let first = database.check_program();
    assert!(first.diagnostics.is_empty(), "{:?}", first.diagnostics);

    fixture.update_module_path(module_id, "renamed.nia");
    database.update(CompileRequest::new(fixture.program()));

    let second = database.analyze_program();
    assert!(second.diagnostics.is_empty(), "{:?}", second.diagnostics);
    assert_eq!(second.modules[0].path.as_str(), "renamed.nia");
}
