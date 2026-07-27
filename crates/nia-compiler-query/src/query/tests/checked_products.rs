// SPDX-License-Identifier: GPL-3.0-or-later
use super::*;

#[test]
fn checked_module_exposes_semantic_use_table_product() {
    let source = "fn main() i32 { let mut local: i32 = 1; local }";
    let fixture = LoadedProgramFixture::new("main.nia", source);
    let checked = fixture.database().analyze_program();

    assert!(checked.diagnostics.is_empty(), "{:?}", checked.diagnostics);
    let module = checked.modules.first().expect("checked module");
    let function_facts = module
        .semantic_facts
        .function_facts
        .values()
        .next()
        .expect("function semantic facts");
    assert_eq!(
        function_facts.store_id(),
        module.semantic_uses.store_id(),
        "frozen body facts should share the compiler session node owner"
    );
    assert_eq!(
        module.semantic_facts.store_id(),
        module.semantic_uses.store_id(),
        "module semantic facts should share the compiler session node owner"
    );
    assert!(matches!(
        module
            .semantic_uses
            .node_value_uses
            .values()
            .find(|value_use| matches!(value_use, SemanticValueUse::Local(_))),
        Some(SemanticValueUse::Local(_))
    ));
}

#[test]
fn checked_module_reuses_cached_semantic_product_handles() {
    let fixture =
        LoadedProgramFixture::new("main.nia", "fn main() i32 { let local: i32 = 1; local }");
    let module_id = fixture.entry_id();
    let db = query_db(fixture.program());

    let checked = db.expect_get(CheckedModuleQuery(module_id));
    let checked_program = db.expect_get(CheckedProgramQuery);
    let values = db.expect_get(ValueResolutionQuery(module_id));
    let locals = db.expect_get(LocalResolutionQuery(module_id));
    let semantic_uses = db.expect_get(SemanticUseTableQuery(module_id));
    let type_resolution = db.expect_get(TypeResolutionQuery(module_id));
    let type_lowering = db.expect_get(TypeLoweringQuery(module_id));
    let type_normalization = db.expect_get(TypeNormalizationQuery(module_id));
    let layouts = db.expect_get(LayoutsQuery(module_id));
    let body_check = db.expect_get(BodyCheckQuery(module_id));
    let const_eval = db.expect_get(ConstQuery(module_id));
    let const_array_lengths = db.expect_get(ConstArrayLengthsQuery(module_id));
    let const_enum_values = db.expect_get(ConstEnumValuesQuery(module_id));
    let const_values = db.expect_get(ConstValuesQuery(module_id));
    let const_typed_facts = db.expect_get(ConstTypedFactsQuery(module_id));
    let static_check = db.expect_get(StaticCheckQuery(module_id));
    let abi_check = db.expect_get(AbiCheckQuery(module_id));
    let flow_check = db.expect_get(FlowCheckQuery(module_id));

    assert!(Arc::ptr_eq(&checked, &checked_program.modules[0]));
    assert!(Arc::ptr_eq(&checked.value_resolution, &values.semantic));
    assert!(Arc::ptr_eq(&checked.local_resolution, &locals.semantic));
    assert!(Arc::ptr_eq(&checked.semantic_uses, &semantic_uses));
    assert!(Arc::ptr_eq(
        &checked.type_resolution,
        &type_resolution.semantic
    ));
    assert!(Arc::ptr_eq(&checked.type_lowering, &type_lowering.semantic));
    assert!(Arc::ptr_eq(
        &checked.type_normalization,
        &type_normalization.semantic
    ));
    assert!(Arc::ptr_eq(&checked.layouts, &layouts.semantic));
    assert!(Arc::ptr_eq(&checked.body_ir, &body_check.semantic.ir));
    assert!(Arc::ptr_eq(
        &checked.semantic_facts,
        &body_check.semantic.facts
    ));
    assert!(Arc::ptr_eq(
        &checked.provider_demands,
        &body_check.semantic.provider_demands
    ));
    assert_eq!(checked.body_diagnostics, body_check.diagnostics);
    assert_eq!(checked.const_diagnostics, const_eval.diagnostics);
    assert!(Arc::ptr_eq(&checked.const_eval, &const_eval.semantic));
    assert!(Arc::ptr_eq(
        &const_eval.semantic.values,
        &const_values.values
    ));
    assert!(Arc::ptr_eq(
        &const_eval.semantic.typed_values,
        &const_typed_facts.typed_values
    ));
    assert!(Arc::ptr_eq(
        &const_eval.semantic.enum_values,
        &const_enum_values.values
    ));
    assert!(Arc::ptr_eq(
        &const_eval.semantic.typed_enum_values,
        &const_enum_values.typed_values
    ));
    assert!(Arc::ptr_eq(
        &const_eval.semantic.array_lengths,
        &const_array_lengths.values
    ));
    assert!(Arc::ptr_eq(&checked.static_check, &static_check.semantic));
    assert!(Arc::ptr_eq(&checked.abi_check, &abi_check.semantic));
    assert!(Arc::ptr_eq(&checked.flow_check, &flow_check.semantic));
    assert_eq!(checked.static_diagnostics, static_check.diagnostics);
    assert_eq!(checked.layout_diagnostics, layouts.diagnostics);
    assert_eq!(checked.abi_diagnostics, abi_check.diagnostics);
    assert_eq!(checked.flow_diagnostics, flow_check.diagnostics);
}

#[test]
fn program_products_share_the_input_module_graph_snapshot() {
    let fixture = LoadedProgramFixture::new("main.nia", "fn main() i32 { 0 }");
    let loaded = fixture.program();
    let input_graph = loaded.graph.clone();
    let db = query_db(loaded);

    let cached_graph = db.expect_get(ModuleGraphQuery);
    let checked = db.expect_get(CheckedProgramQuery);
    let codegen = db.expect_get(CodegenProgramQuery);

    assert!(input_graph.ptr_eq(&cached_graph));
    assert!(input_graph.ptr_eq(&checked.graph));
    assert!(input_graph.ptr_eq(&codegen.graph));
}

#[test]
fn checked_modules_reuse_cached_definition_handles() {
    let fixture = LoadedProgramFixture::new("main.nia", "fn main() i32 { 1 }");
    let module_id = fixture.entry_id();
    let db = query_db(fixture.program());

    let defs = db.expect_get(FullModuleDefsQuery(module_id));
    let checked = db.expect_get(CheckedModuleQuery(module_id));
    let executable = db.expect_get(ExecutableCheckedModulesQuery);
    let executable = executable
        .iter()
        .find(|module| module.id == module_id)
        .expect("entry executable module");

    assert!(Arc::ptr_eq(&checked.defs, &defs.semantic));
    assert!(Arc::ptr_eq(&executable.defs, &defs.semantic));
    assert_eq!(checked.definition_diagnostics, defs.diagnostics);
    assert_eq!(executable.definition_diagnostics, defs.diagnostics);
}

#[test]
fn full_module_definitions_separate_semantic_value_from_diagnostics() {
    let fixture = LoadedProgramFixture::new("main.nia", "struct Duplicate {} struct Duplicate {}");
    let module_id = fixture.entry_id();
    let db = query_db(fixture.program());

    let definitions = db.expect_get(FullModuleDefsQuery(module_id));
    let checked = db.expect_get(CheckedModuleQuery(module_id));

    assert!(definitions.semantic.diagnostics.is_empty());
    assert!(
        resolve_diagnostic_bundle(db.context(), &definitions.diagnostics)
            .iter()
            .any(|diagnostic| diagnostic
                .primary_message()
                .is_some_and(|message| message.contains("duplicate type definition")))
    );
    assert!(Arc::ptr_eq(&checked.defs, &definitions.semantic));
    assert_eq!(checked.definition_diagnostics, definitions.diagnostics);
}
