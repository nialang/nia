// SPDX-License-Identifier: GPL-3.0-or-later
use super::*;

#[test]
fn parse_error_changes_keep_stable_program_module_membership_green() {
    let mut fixture = LoadedProgramFixture::new("main.nia", "module broken; fn main() i32 { 0 }");
    let entry_id = fixture.entry_id();
    let broken_id = fixture.add_child(entry_id, "broken", "broken.nia", "fn helper() i32 { 1 }");
    let mut initial = fixture.program();
    initial
        .modules
        .iter_mut()
        .find(|module| module.id == broken_id)
        .expect("broken fixture module")
        .parse_errors = vec![ParseError {
        span: Span::default(),
        message: "first parse failure".to_string(),
        node_key: None,
    }];
    let database = CompilerDatabase::new(CompileRequest::new(initial));
    let first = database.db.expect_get(ProgramSignatureModuleIdsQuery(
        nia_item_tree::SignatureItemSet::Functions,
    ));
    assert_eq!(
        resolve_stable_module_sequence(&database.db, &first)
            .expect("program signature module sequence"),
        vec![entry_id]
    );
    let mut updated = fixture.program();
    updated
        .modules
        .iter_mut()
        .find(|module| module.id == broken_id)
        .expect("broken fixture module")
        .parse_errors = vec![ParseError {
        span: Span::default(),
        message: "second parse failure".to_string(),
        node_key: None,
    }];

    database.update(CompileRequest::new(updated));

    let latest = database.db.expect_get(ProgramSignatureModuleIdsQuery(
        nia_item_tree::SignatureItemSet::Functions,
    ));
    assert!(Arc::ptr_eq(&first, &latest));
    let trace = database.query_trace();
    let module_ids = trace
        .queries
        .iter()
        .find(|query| query.frame.name == "program_signature_module_ids")
        .expect("program signature module ids trace");
    assert_eq!(module_ids.stats.executions, 1);
    assert_eq!(module_ids.stats.validations, 1);
    assert_eq!(module_ids.stats.green_validations, 1);
}

#[test]
fn signature_changes_keep_stable_program_module_membership_green() {
    let mut fixture =
        LoadedProgramFixture::new("main.nia", "fn first() i32 { 1 } fn main() i32 { first() }");
    let module_id = fixture.entry_id();
    let database = fixture.database();
    let first = database.db.expect_get(ProgramSignatureModuleIdsQuery(
        nia_item_tree::SignatureItemSet::Functions,
    ));

    fixture.update_module_source(
        module_id,
        "fn first() i32 { 1 } fn second() i32 { 2 } fn main() i32 { first() }",
        SourceRevision(1),
    );
    database.update(CompileRequest::new(fixture.program()));

    let latest = database.db.expect_get(ProgramSignatureModuleIdsQuery(
        nia_item_tree::SignatureItemSet::Functions,
    ));
    assert!(Arc::ptr_eq(&first, &latest));
    let trace = database.query_trace();
    let module_ids = trace
        .queries
        .iter()
        .find(|query| query.frame.name == "program_signature_module_ids")
        .expect("program signature module ids trace");
    assert_eq!(module_ids.stats.executions, 1);
    assert_eq!(module_ids.stats.validations, 1);
    assert_eq!(module_ids.stats.green_validations, 1);
}
