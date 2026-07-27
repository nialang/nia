// SPDX-License-Identifier: GPL-3.0-or-later

use super::*;

#[test]
fn compiler_query_registry_covers_all_declared_query_contracts() {
    let descriptors = compiler_query_registry().descriptors();

    assert_eq!(descriptors.len(), 132);
    assert!(
        !descriptors
            .iter()
            .any(|descriptor| descriptor.name == "module_graph_node")
    );
    for name in [
        "body_activation_worklist",
        "executable_fact_epoch",
        "module_graph_entry",
        "module_graph_path",
        "module_graph_parent",
        "module_graph_child",
        "module_package_root",
        "provider_fact_revision",
        "provider_fact_worklist",
    ] {
        assert!(
            descriptors.iter().any(|descriptor| descriptor.name == name),
            "missing precise graph fact query `{name}`"
        );
    }
    assert!(
        descriptors
            .windows(2)
            .all(|pair| pair[0].name < pair[1].name)
    );
    assert!(descriptors.iter().all(|descriptor| {
        let expected_storage = if matches!(
            descriptor.name,
            "backend_item_plan" | "backend_module_item_plan" | "backend_module_finalization"
        ) {
            nia_query::QueryStoragePolicy::SingleConsumerOwned
        } else {
            nia_query::QueryStoragePolicy::CacheOwnedArc
        };
        let expected_provider = if descriptor.name == "backend_module_item_plan" {
            nia_query::QueryProviderPolicy::ExternallyPublished
        } else {
            nia_query::QueryProviderPolicy::KeyExecute
        };
        descriptor.context_type == std::any::type_name::<CompilerContext>()
            && descriptor.provider == expected_provider
            && descriptor.storage == expected_storage
    }));
    for descriptor in descriptors {
        let expected = match descriptor.name {
            "extension_provider_module_ids"
            | "extension_provider_module_eligibility"
            | "extension_provider_summary"
            | "loaded_modules"
            | "module_graph_child"
            | "module_graph_entry"
            | "module_graph_parent"
            | "module_graph_path"
            | "module_package_root"
            | "module_path"
            | "module_source_version"
            | "parse_ok_module_ids"
            | "program_signature_module_eligibility"
            | "program_signature_module_ids"
            | "provider_fact_revision"
            | "provider_fact_worklist"
            | "public_surface_module"
            | "semantic_module_ids"
            | "using_scope_module" => nia_query::QueryFingerprintPolicy::StableValue,
            "active_module_item_tree_input"
            | "backend_module_function_instance_plan"
            | "backend_module_source_item_plan"
            | "body_activation_worklist"
            | "declaration_active_module_item_tree_input"
            | "declaration_module_item_tree_input"
            | "executable_function_body"
            | "executable_static_init"
            | "full_active_module_item_tree_input"
            | "executable_fact_epoch"
            | "full_module_item_tree_input"
            | "lowered_function_body"
            | "module_public_surface"
            | "module_item_tree_input"
            | "module_origins"
            | "module_parse_errors"
            | "module_using_scope"
            | "public_surface_module_facts"
            | "public_surface_type"
            | "public_surface_value"
            | "public_surfaces"
            | "public_using_scopes"
            | "signature_const_item_tree"
            | "signature_item_tree"
            | "using_scope_type"
            | "using_scope_unresolved"
            | "using_scope_value" => nia_query::QueryFingerprintPolicy::SemanticValue,
            _ => nia_query::QueryFingerprintPolicy::None,
        };
        assert_eq!(descriptor.fingerprint, expected, "{}", descriptor.name);
    }
}

#[test]
fn public_options_flow_through_compiler_query_context() {
    for level in [
        NiaOptimizationLevel::O0,
        NiaOptimizationLevel::O1,
        NiaOptimizationLevel::O2,
        NiaOptimizationLevel::O3,
        NiaOptimizationLevel::Os,
        NiaOptimizationLevel::Oz,
    ] {
        let fixture = LoadedProgramFixture::new(
            "main.nia",
            r#"
static zeroes: [4]i32 = [0; 4];

fn main() i32 {
zeroes[0]
}
"#,
        );
        let checked =
            CompilerDatabase::new(CompileRequest::new(fixture.program()).with_optimization(level))
                .codegen_program();
        let policy = level.policy();

        assert!(
            checked.diagnostics.is_empty(),
            "{level:?}: {:?}",
            checked.diagnostics
        );
        assert_eq!(checked.optimization, policy, "{level:?}");
        assert_eq!(checked.backend_lowering.optimization, policy, "{level:?}");
        assert_eq!(
            checked
                .backend_lowering
                .optimization_report
                .enabled_global_passes,
            if policy.prefer_size || policy.const_fold.at_least(nia_opt::OptimizationDepth::Full) {
                vec!["simplify-static-init"]
            } else {
                Vec::new()
            },
            "{level:?}"
        );
    }
}

#[test]
fn compiler_database_exposes_query_trace() {
    let fixture = LoadedProgramFixture::new("main.nia", "fn main() i32 { 0 }");
    let database = CompilerDatabase::new(CompileRequest::new(fixture.program()));

    let checked = database.check_program();
    let trace = database.query_trace();

    assert!(checked.diagnostics.is_empty(), "{:?}", checked.diagnostics);
    assert!(trace.dependencies.iter().any(|dependency| {
        dependency.from.name == "checked_program" && dependency.to.name == "checked_module_ids"
    }));
}

#[test]
fn compiler_update_rejects_untracked_snapshot_provider() {
    let fixture = LoadedProgramFixture::new("main.nia", "fn main() i32 { 0 }");
    let database = crate::query::CompilerDatabase::new(CompileRequest::new(fixture.program()));

    let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        let _ = database.update(CompileRequest::new(fixture.program()));
    }));

    assert!(result.is_err());
}

#[test]
fn compiler_query_providers_can_override_query_execution() {
    fn no_parse_ok_modules(_: &QueryDb<CompilerContext>) -> QueryResult<StableModuleSequence> {
        Ok(StableModuleSequence::default())
    }

    let providers = CompilerQueryProviders {
        parse_ok_module_ids: no_parse_ok_modules,
        ..CompilerQueryProviders::default()
    };
    let fixture = LoadedProgramFixture::new("main.nia", "fn main() i32 { 0 }");
    let checked =
        compiler_database_with_providers(CompileRequest::new(fixture.program()), providers)
            .codegen_program()
            .expect("overridden codegen program");

    assert!(checked.modules.is_empty());
}

#[test]
fn missing_loaded_module_id_propagates_query_failure() {
    fn unknown_module_id() -> ModuleId {
        let mut module_ids = nia_ids::ModuleIdAllocator::new();
        module_ids.allocate();
        module_ids.allocate()
    }

    fn unknown_checked_module(_: &QueryDb<CompilerContext>) -> QueryResult<Vec<ModuleId>> {
        Ok(vec![unknown_module_id()])
    }

    let providers = CompilerQueryProviders {
        checked_module_ids: unknown_checked_module,
        ..CompilerQueryProviders::default()
    };
    let fixture = LoadedProgramFixture::new("main.nia", "fn main() i32 { 0 }");
    let database = compiler_database_with_providers(
        CompileRequest::new(fixture.program()).with_optimization(NiaOptimizationLevel::Oz),
        providers,
    );
    let missing_module = unknown_module_id();
    for error in [
        database
            .db
            .get(ModulePathQuery(missing_module))
            .expect_err("missing module path should be a query error"),
        database
            .db
            .get(ModuleItemTreeQuery(missing_module))
            .expect_err("missing module item tree should propagate its input query error"),
        database
            .db
            .get(ModuleDefsQuery(missing_module))
            .expect_err("module definitions should propagate a missing item tree error"),
        database
            .db
            .get(TypeResolutionQuery(missing_module))
            .expect_err("type resolution should propagate a missing module input"),
        database
            .db
            .get(TypeNormalizationQuery(missing_module))
            .expect_err("type normalization should propagate a missing module input"),
        database
            .db
            .get(SemanticUseTableQuery(missing_module))
            .expect_err("semantic uses should propagate a missing module input"),
        database
            .db
            .get(ConstModuleQuery(missing_module))
            .expect_err("const lowering should propagate a missing module input"),
        database
            .db
            .get(SignatureConstModuleQuery(missing_module))
            .expect_err("signature const lowering should propagate a missing module input"),
        database
            .db
            .get(ConstArrayLengthsQuery(missing_module))
            .expect_err("const array lengths should propagate a missing module input"),
        database
            .db
            .get(ConstEnumValuesQuery(missing_module))
            .expect_err("const enum values should propagate a missing module input"),
        database
            .db
            .get(ConstValuesQuery(missing_module))
            .expect_err("const values should propagate a missing module input"),
        database
            .db
            .get(ConstTypedFactsQuery(missing_module))
            .expect_err("const typed facts should propagate a missing module input"),
        database
            .db
            .get(ConstQuery(missing_module))
            .expect_err("const checking should propagate a missing module input"),
        database
            .db
            .get(SignatureLayoutsQuery(missing_module))
            .expect_err("signature layouts should propagate a missing module input"),
        database
            .db
            .get(LayoutsQuery(missing_module))
            .expect_err("layouts should propagate a missing module input"),
        database
            .db
            .get(StaticCheckQuery(missing_module))
            .expect_err("static checking should propagate a missing module input"),
        database
            .db
            .get(BodyCheckQuery(missing_module))
            .expect_err("body checking should propagate a missing module input"),
        full_body_check_resolution_inputs(&database.db, missing_module)
            .err()
            .expect("body resolution inputs should propagate a missing module input"),
        database
            .db
            .get(CheckedModuleQuery(missing_module))
            .expect_err("checked modules should propagate a missing module input"),
        database
            .db
            .get(CheckedProgramQuery)
            .expect_err("checked program aggregation should propagate a missing module input"),
        database
            .db
            .get(FlowCheckQuery(missing_module))
            .expect_err("flow checking should propagate a missing module input"),
        database
            .db
            .get(ModuleAbiSignatureFactsQuery(missing_module))
            .expect_err("ABI signature facts should propagate a missing module input"),
        database
            .db
            .get(AbiCheckQuery(missing_module))
            .expect_err("ABI checking should propagate a missing module input"),
        database
            .db
            .get(ExtensionSignatureModuleInputQuery(missing_module))
            .expect_err("extension signature input should propagate a missing module input"),
        database
            .db
            .get(ExtensionTraitSolvingModuleFactsQuery(missing_module))
            .expect_err("extension trait facts should propagate a missing module input"),
        database
            .db
            .get(ExtensionProviderValidationFactsQuery(missing_module))
            .expect_err("extension validation should propagate a missing module input"),
        database
            .db
            .get(VisibleExtensionsQuery(missing_module))
            .expect_err("visible extensions should propagate a missing module input"),
        database
            .db
            .get(ExecutableValueRefEdgesQuery(GlobalDefId {
                module_id: missing_module,
                def_id: nia_ids::DefId(0),
            }))
            .expect_err("value-ref edges should propagate a missing module input"),
    ] {
        assert!(matches!(error, QueryError::InvalidInput { .. }));
        assert!(
            error
                .to_string()
                .contains(&format!("missing loaded module {missing_module:?}"))
        );
    }

    let error = database
        .analyze_program()
        .expect_err("public analysis must propagate a missing module query failure");
    assert!(matches!(error, QueryError::InvalidInput { .. }));
    assert!(
        error
            .to_string()
            .contains(&format!("missing loaded module {:?}", unknown_module_id()))
    );
}
