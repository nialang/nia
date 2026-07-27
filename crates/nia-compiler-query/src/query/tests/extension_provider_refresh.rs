// SPDX-License-Identifier: GPL-3.0-or-later

use super::*;

#[test]
fn extension_provider_module_facts_refresh_across_source_revisions() {
    let mut fixture = LoadedProgramFixture::new(
        "main.nia",
        "struct S { value: i32 } extend S { pub fn make(value: i32) S { { value: value } } }",
    );
    let module_id = fixture.entry_id();
    let database = fixture.database();

    let _ = database.db.expect_get(ExtensionMethodIndexQuery);
    let before_update = database.query_trace();
    assert!(
        query_executions(&before_update, "extension_provider_module_facts") > 0,
        "{before_update:?}"
    );

    fixture.update_module_source(
        module_id,
        "struct S { value: i32 } extend S { pub fn make(value: i32) S { let next = value; { value: next } } }",
        SourceRevision(1),
    );
    database.update(CompileRequest::new(fixture.program()));
    let before_second_query = database.query_trace();

    let _ = database.db.expect_get(ExtensionMethodIndexQuery);
    let after_second_query = database.query_trace();

    assert_query_executions_unchanged(
        &before_second_query,
        &after_second_query,
        "extension_provider_summary",
    );
    assert!(
        query_executions(&before_second_query, "extension_provider_module_facts")
            < query_executions(&after_second_query, "extension_provider_module_facts")
    );
}

#[test]
fn provider_summary_changes_validate_stable_module_eligibility() {
    let mut fixture = LoadedProgramFixture::new(
        "main.nia",
        "struct S {} extend S { pub fn first() i32 { 1 } }",
    );
    let module_id = fixture.entry_id();
    let database = fixture.database();
    let first = database
        .db
        .expect_get(ExtensionProviderModuleEligibilityQuery(module_id));
    assert!(*first);
    let first_modules = database.db.expect_get(ExtensionProviderModuleIdsQuery);

    fixture.update_module_source(
        module_id,
        "struct S {} extend S { pub fn first() i32 { 1 } pub fn second() i32 { 2 } }",
        SourceRevision(1),
    );
    database.update(CompileRequest::new(fixture.program()));

    let second = database
        .db
        .expect_get(ExtensionProviderModuleEligibilityQuery(module_id));
    assert!(*second);
    assert!(!Arc::ptr_eq(&first, &second));
    let latest_modules = database.db.expect_get(ExtensionProviderModuleIdsQuery);
    assert!(Arc::ptr_eq(&first_modules, &latest_modules));
    let trace = database.query_trace();
    let eligibility = trace
        .queries
        .iter()
        .find(|query| query.frame.name == "extension_provider_module_eligibility")
        .expect("extension provider eligibility trace");
    assert_eq!(eligibility.stats.executions, 2);
    assert_eq!(eligibility.stats.validations, 1);
    assert_eq!(eligibility.stats.green_validations, 0);
    let module_ids = trace
        .queries
        .iter()
        .find(|query| query.frame.name == "extension_provider_module_ids")
        .expect("extension provider module ids trace");
    assert_eq!(module_ids.stats.executions, 1);
    assert_eq!(module_ids.stats.validations, 1);
    assert_eq!(module_ids.stats.green_validations, 1);
}
