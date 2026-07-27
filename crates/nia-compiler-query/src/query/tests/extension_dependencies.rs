// SPDX-License-Identifier: GPL-3.0-or-later
use super::*;

#[test]
fn extension_queries_use_module_semantic_queries() {
    let fixture = LoadedProgramFixture::new(
        "main.nia",
        "struct S { value: i32 } extend S { pub fn make(value: i32) S { { value: value } } }",
    );
    let module_id = fixture.entry_id();
    let db = query_db(fixture.program());

    let _ = db.expect_get(ExtensionProviderValidationFactsQuery(module_id));
    let _ = db.expect_get(ExtensionProviderModuleFactsQuery(module_id));
    let _ = db.expect_get(ExtensionMethodIndexQuery);
    let _ = db.expect_get(ExtensionProviderDiscoveryIndexQuery);
    let trace = db.query_trace();

    assert!(
        !trace
            .queries
            .iter()
            .any(|query| query.frame.name == "extension_provider_program_facts")
    );
    assert!(trace_has_dependency(
        &trace,
        "extension_provider_validation_facts",
        "extension_signature_module_input"
    ));
    assert!(trace_has_dependency(
        &trace,
        "extension_provider_validation_facts",
        "extension_trait_signature_index"
    ));
    assert!(trace_has_dependency(
        &trace,
        "extension_trait_signature_index",
        "module_program_signature_facts"
    ));
    assert!(trace_has_dependency(
        &trace,
        "module_program_signature_facts",
        "signature_item_signatures"
    ));
    assert!(!trace_has_dependency(
        &trace,
        "module_program_signature_facts",
        "module_defs"
    ));
    assert!(!trace_has_dependency(
        &trace,
        "module_program_signature_facts",
        "signature_type_lowering"
    ));
    assert!(!trace_has_dependency(
        &trace,
        "extension_provider_validation_facts",
        "program_trait_solving_signatures"
    ));
    assert!(trace_has_dependency(
        &trace,
        "extension_signature_module_input",
        "signature_item_signatures"
    ));
    assert!(trace_has_dependency(
        &trace,
        "extension_signature_module_input",
        "signature_type_normalization"
    ));
    assert!(trace_has_dependency(
        &trace,
        "extension_provider_module_ids",
        "parse_ok_module_ids"
    ));
    assert!(trace_has_dependency(
        &trace,
        "extension_provider_discovery_index",
        "parse_ok_module_ids"
    ));
    assert!(trace_has_dependency(
        &trace,
        "extension_provider_discovery_index",
        "extension_provider_summary"
    ));
    assert!(trace_has_dependency(
        &trace,
        "extension_provider_module_ids",
        "extension_provider_module_eligibility"
    ));
    assert!(!trace_has_dependency(
        &trace,
        "extension_provider_summary",
        "signature_item_tree"
    ));
    assert!(trace_has_dependency(
        &trace,
        "extension_signature_module_input",
        "module_defs"
    ));
    assert!(trace_has_dependency(
        &trace,
        "extension_signature_module_input",
        "signature_type_lowering"
    ));
    assert!(!trace_has_dependency(
        &trace,
        "program_trait_solving_signatures",
        "program_signature_module_ids"
    ));
    assert!(!trace_has_dependency(
        &trace,
        "program_trait_solving_signatures",
        "module_program_signature_facts"
    ));
    for query in [
        "extension_provider_validation_facts",
        "extension_trait_signature_index",
        "extension_provider_module_eligibility",
        "extension_provider_summary",
        "extension_signature_module_input",
        "extension_trait_solving_module_facts",
    ] {
        assert!(!trace.dependencies.iter().any(|dependency| {
            dependency.from.name == query
                && matches!(
                    dependency.to.name,
                    "item_signatures" | "declaration_type_lowering"
                )
        }));
        assert!(!trace.dependencies.iter().any(|dependency| {
            dependency.from.name == query && dependency.to.name == "active_module_item_tree"
        }));
        assert!(!trace.dependencies.iter().any(|dependency| {
            dependency.from.name == query && dependency.to.name == "full_module_defs"
        }));
        assert!(!trace.dependencies.iter().any(|dependency| {
            dependency.from.name == query && dependency.to.name == "program_type_normalizations"
        }));
    }
    {
        let query = "extension_method_index";
        assert!(trace.dependencies.iter().any(|dependency| {
            dependency.from.name == query && dependency.to.name == "extension_provider_module_facts"
        }));
        assert!(!trace.dependencies.iter().any(|dependency| {
            dependency.from.name == query
                && matches!(
                    dependency.to.name,
                    "signature_item_signatures"
                        | "signature_type_lowering"
                        | "signature_type_normalization"
                )
        }));
    }
    assert!(trace.dependencies.iter().any(|dependency| {
        dependency.from.name == "extension_provider_module_facts"
            && dependency.to.name == "module_defs"
    }));
    assert!(trace.dependencies.iter().any(|dependency| {
        dependency.from.name == "extension_provider_module_facts"
            && dependency.to.name == "signature_item_signatures"
    }));
    assert!(trace.dependencies.iter().any(|dependency| {
        dependency.from.name == "extension_provider_module_facts"
            && dependency.to.name == "signature_type_lowering"
    }));
    assert!(trace.dependencies.iter().any(|dependency| {
        dependency.from.name == "extension_provider_module_facts"
            && dependency.to.name == "signature_type_normalization"
    }));
    assert!(trace.dependencies.iter().any(|dependency| {
        dependency.from.name == "extension_method_index"
            && dependency.to.name == "extension_provider_module_facts"
    }));
    assert!(!trace.dependencies.iter().any(|dependency| {
        dependency.from.name == "extension_method_index"
            && matches!(
                dependency.to.name,
                "item_signatures" | "declaration_type_lowering"
            )
    }));
    assert!(!trace.dependencies.iter().any(|dependency| {
        dependency.from.name == "extension_method_index"
            && matches!(
                dependency.to.name,
                "extension_provider_validation_facts" | "program_trait_solving_signatures"
            )
    }));
    assert!(!trace.dependencies.iter().any(|dependency| {
        dependency.from.name == "declaration_type_lowering"
            && dependency.to.name == "program_defs_by_id"
    }));
    assert!(!trace.dependencies.iter().any(|dependency| {
        dependency.from.name == "declaration_type_lowering"
            && dependency.to.name == "program_full_defs_by_id"
    }));
}
