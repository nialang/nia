// SPDX-License-Identifier: GPL-3.0-or-later

use super::*;

const MODULE_VALUE_TYPES_MATRIX: &str =
    include_str!("../../../../../formal/NiaFormal/module_value_types_matrix.json");

fn assert_formal_value_type_case(case: &str) {
    let matrix: serde_json::Value =
        serde_json::from_str(MODULE_VALUE_TYPES_MATRIX).expect("valid module value-type matrix");
    let cases = matrix["dependency_cases"]
        .as_array()
        .expect("dependency case list");
    assert!(
        cases.iter().any(|entry| entry.as_str() == Some(case)),
        "formal module value-type matrix is missing case `{case}`"
    );
}

#[test]
fn module_value_type_cases_match_the_formal_inventory() {
    let matrix: serde_json::Value =
        serde_json::from_str(MODULE_VALUE_TYPES_MATRIX).expect("valid module value-type matrix");
    let declaration_kinds = matrix["declaration_kinds"]
        .as_array()
        .expect("declaration kind list")
        .iter()
        .map(|entry| entry.as_str().expect("string declaration kind"))
        .collect::<Vec<_>>();
    let dependency_cases = matrix["dependency_cases"]
        .as_array()
        .expect("dependency case list")
        .iter()
        .map(|entry| entry.as_str().expect("string dependency case"))
        .collect::<Vec<_>>();

    assert_eq!(declaration_kinds, ["global", "const"]);
    assert_eq!(
        dependency_cases,
        [
            "forward_const_used_by_static",
            "transitive_const_chain",
            "same_type_fact_for_const_and_global",
            "cross_module_forward_const",
            "cross_module_const_dependency_change",
            "cross_module_const_cycle",
            "incremental_const_dependency_change",
            "const_cycle_source_diagnostic",
            "forward_global_dependency",
        ]
    );
}

#[test]
fn signature_type_resolution_separates_semantic_value_from_diagnostics() {
    let fixture = LoadedProgramFixture::new("main.nia", "fun main(value: Missing) {}");
    let module_id = fixture.entry_id();
    let db = query_db(fixture.program());

    let resolution = db.expect_get(SignatureTypeResolutionQuery(
        module_id,
        nia_item_tree::SignatureItemSet::Functions,
    ));
    assert!(resolution.semantic.diagnostics.is_empty());
    assert!(!resolve_diagnostic_bundle(&resolution.diagnostics).is_empty());
}

#[test]
fn inferred_module_value_types_do_not_reenter_unrelated_module_bodies() {
    let mut fixture = LoadedProgramFixture::new(
        "main.nia",
        "module a; module b; using a; using b; fun main() i32 { a::fa() + b::fb() }",
    );
    let entry = fixture.entry_id();
    fixture.add_child(
        entry,
        "a",
        "a.nia",
        "using super::b; pub static x = 1; pub fun fa() i32 { b::y }",
    );
    fixture.add_child(
        entry,
        "b",
        "b.nia",
        "using super::a; pub static y = 2; pub fun fb() i32 { a::x }",
    );

    let checked = fixture.database().check_program();

    assert!(
        checked.diagnostics.is_empty(),
        "independent inferred globals must not make mutually importing module bodies recursive: {:?}",
        checked.diagnostics
    );
}

#[test]
fn inferred_module_value_types_follow_cross_module_initializer_dependencies() {
    let mut fixture = LoadedProgramFixture::new(
        "main.nia",
        "module a; module b; using a; fun main() i32 { a::x.* }",
    );
    let entry = fixture.entry_id();
    // `x`'s type is inferred from `y`'s; an initializer may store another
    // static's address but not read its value.
    fixture.add_child(entry, "a", "a.nia", "using super::b; pub static x = &b::y;");
    fixture.add_child(entry, "b", "b.nia", "pub static y = 2;");

    let checked = fixture.database().check_program();

    assert!(
        checked.diagnostics.is_empty(),
        "a one-way cross-module inferred global dependency is valid: {:?}",
        checked.diagnostics
    );
}

#[test]
fn inferred_module_value_types_follow_forward_dependencies_in_one_module() {
    assert_formal_value_type_case("forward_global_dependency");
    let fixture = LoadedProgramFixture::new(
        "main.nia",
        "static pointer = &value; static value = 2; fun main() i32 { pointer.* }",
    );

    let checked = fixture.database().check_program();

    assert!(
        checked.diagnostics.is_empty(),
        "an earlier pointer global may infer its pointee from a later global: {:?}",
        checked.diagnostics
    );
}

#[test]
fn inferred_static_types_follow_forward_unannotated_const_dependencies() {
    assert_formal_value_type_case("forward_const_used_by_static");
    assert_formal_value_type_case("transitive_const_chain");
    let fixture = LoadedProgramFixture::new(
        "main.nia",
        "static value = FIRST; const FIRST = SECOND; const SECOND = 3; fun main() i32 { value }",
    );

    let checked = fixture.database().check_program();

    assert!(
        checked.diagnostics.is_empty(),
        "an inferred static may use later unannotated const values: {:?}",
        checked.diagnostics
    );
}

#[test]
fn inferred_module_value_types_publish_forward_consts_across_modules() {
    assert_formal_value_type_case("cross_module_forward_const");
    let mut fixture = LoadedProgramFixture::new(
        "main.nia",
        "module values; using values; static result = values::FIRST; fun main() i32 { result }",
    );
    let entry = fixture.entry_id();
    let values = fixture.add_child(
        entry,
        "values",
        "values.nia",
        "pub const FIRST = SECOND; pub const SECOND = 3;",
    );
    let database = fixture.database();

    let checked = database.check_program();

    assert!(
        checked.diagnostics.is_empty(),
        "a static initializer may use forward inferred consts from another module: {:?}",
        checked.diagnostics
    );
    let inferred = database
        .db
        .expect_get(InferredModuleValueTypesQuery(values));
    assert_eq!(inferred.const_types.len(), 2);

    assert_formal_value_type_case("cross_module_const_dependency_change");
    fixture.update_module_source(
        values,
        "pub const FIRST = SECOND; pub const SECOND = true;",
        SourceRevision(1),
    );
    fixture.update_module_source(
        entry,
        "module values; using values; static result = values::FIRST; fun main() bool { result }",
        SourceRevision(1),
    );
    database.update(CompileRequest::new(fixture.program()));

    let updated = database.check_program();
    assert!(
        updated.diagnostics.is_empty(),
        "the importing module must observe an edited const type through query invalidation: {:?}",
        updated.diagnostics
    );
}

#[test]
fn cross_module_const_type_cycles_remain_source_diagnostics() {
    assert_formal_value_type_case("cross_module_const_cycle");
    let mut fixture = LoadedProgramFixture::new(
        "main.nia",
        "module a; using a; static value = a::FIRST; fun main() i32 { 0 }",
    );
    let entry = fixture.entry_id();
    fixture.add_child(
        entry,
        "a",
        "a.nia",
        "module b; using super::b; pub const FIRST = b::SECOND;",
    );
    fixture.add_child(
        entry,
        "b",
        "b.nia",
        "using super::a; pub const SECOND = a::FIRST;",
    );

    let checked = fixture.database().check_program();

    assert!(
        checked.diagnostics.iter().all(|diagnostic| {
            diagnostic.diagnostic.code.def().category
                != nia_diagnostic::DiagnosticCategory::Internal
        }),
        "cross-module const cycles must not escape as internal query failures: {:?}",
        checked.diagnostics
    );
    assert!(
        !checked.diagnostics.is_empty(),
        "a cross-module const type cycle must produce a source diagnostic"
    );
}

#[test]
fn const_and_global_initializers_publish_the_same_value_type_fact() {
    assert_formal_value_type_case("same_type_fact_for_const_and_global");
    let fixture = LoadedProgramFixture::new(
        "main.nia",
        "const ELEMENTS = [1, 2, 3]; static from_const = ELEMENTS; static direct = [1, 2, 3]; fun main() i32 { from_const[0] + direct[0] }",
    );
    let module_id = fixture.entry_id();
    let database = fixture.database();

    let checked = database.check_program();

    assert!(checked.diagnostics.is_empty(), "{:?}", checked.diagnostics);
    let defs = database.db.expect_get(FullModuleDefsQuery(module_id));
    let inferred = database
        .db
        .expect_get(InferredModuleValueTypesQuery(module_id));
    let def_id = |name| {
        defs.semantic
            .module_scope
            .values
            .get(&sym(name))
            .expect("value declaration")
    };
    let const_type = inferred.const_types[&GlobalDefId {
        module_id,
        def_id: def_id("ELEMENTS"),
    }];
    let copied_global_type = inferred.global_types[&GlobalDefId {
        module_id,
        def_id: def_id("from_const"),
    }];
    let direct_global_type = inferred.global_types[&GlobalDefId {
        module_id,
        def_id: def_id("direct"),
    }];

    assert_eq!(const_type, copied_global_type);
    assert_eq!(const_type, direct_global_type);
}

#[test]
fn inferred_const_value_type_facts_invalidate_when_a_forward_target_changes_type() {
    assert_formal_value_type_case("incremental_const_dependency_change");
    let mut fixture = LoadedProgramFixture::new(
        "main.nia",
        "static value = FIRST; const FIRST = SECOND; const SECOND = 2; fun main() i32 { value }",
    );
    let module_id = fixture.entry_id();
    let database = fixture.database();

    let first = database.check_program();
    assert!(first.diagnostics.is_empty(), "{:?}", first.diagnostics);

    fixture.update_module_source(
        module_id,
        "static value = FIRST; const FIRST = SECOND; const SECOND = true; fun main() bool { value }",
        SourceRevision(1),
    );
    database.update(CompileRequest::new(fixture.program()));

    let second = database.check_program();
    assert!(
        second.diagnostics.is_empty(),
        "inferred const and static types must both follow an edited forward dependency: {:?}",
        second.diagnostics
    );
}

#[test]
fn inferred_const_type_cycles_remain_source_diagnostics() {
    assert_formal_value_type_case("const_cycle_source_diagnostic");
    let fixture = LoadedProgramFixture::new(
        "main.nia",
        "const FIRST = SECOND; const SECOND = FIRST; static value = FIRST; fun main() i32 { 0 }",
    );

    let checked = fixture.database().check_program();

    assert!(
        checked.diagnostics.iter().all(|diagnostic| {
            diagnostic.diagnostic.code.def().category
                != nia_diagnostic::DiagnosticCategory::Internal
        }),
        "cyclic unannotated const types must not escape as an internal query failure: {:?}",
        checked.diagnostics
    );
    assert!(
        checked.diagnostics.iter().any(|diagnostic| {
            diagnostic
                .diagnostic
                .summary
                .to_ascii_lowercase()
                .contains("cyclic")
        }),
        "a const cycle should retain its source-level diagnostic: {:?}",
        checked.diagnostics
    );
}

#[test]
fn inferred_global_type_facts_invalidate_when_a_forward_target_changes_type() {
    let mut fixture = LoadedProgramFixture::new(
        "main.nia",
        "static pointer = &value; static value = 2; fun main() i32 { pointer.* }",
    );
    let module_id = fixture.entry_id();
    let database = fixture.database();

    let first = database.check_program();
    assert!(first.diagnostics.is_empty(), "{:?}", first.diagnostics);

    fixture.update_module_source(
        module_id,
        "static pointer = &value; static value = true; fun main() bool { pointer.* }",
        SourceRevision(1),
    );
    database.update(CompileRequest::new(fixture.program()));

    let second = database.check_program();
    assert!(
        second.diagnostics.is_empty(),
        "the inferred pointer type must follow the changed forward target: {:?}",
        second.diagnostics
    );
}

#[test]
fn recursive_inferred_global_pointer_types_are_source_diagnostics() {
    let fixture = LoadedProgramFixture::new(
        "main.nia",
        "static a = &b; static b = &a; fun main() i32 { 0 }",
    );

    let database = fixture.database();
    let inferred = database
        .db
        .expect_get(InferredModuleValueTypesQuery(fixture.entry_id()));
    assert!(
        !inferred.global_type_issues.is_empty(),
        "the recursive type cycle should retain a source issue: {:?}",
        inferred.global_type_issues
    );
    let checked = database.check_program();

    assert!(
        checked.diagnostics.iter().all(|diagnostic| {
            diagnostic.diagnostic.code.def().category
                != nia_diagnostic::DiagnosticCategory::Internal
        }),
        "recursive inferred pointer types must not escape as query failures: {:?}",
        checked.diagnostics
    );
    assert!(
        checked.diagnostics.iter().any(|diagnostic| {
            diagnostic
                .diagnostic
                .summary
                .contains("cyclic initializer dependency")
        }),
        "an ungrounded recursive inferred type should explain the source-level cycle: {:?}",
        checked.diagnostics
    );
}

#[test]
fn cyclic_inferred_module_value_types_are_not_query_engine_failures() {
    let mut fixture = LoadedProgramFixture::new(
        "main.nia",
        "module a; module b; using a; using b; fun main() i32 { a::x + b::y }",
    );
    let entry = fixture.entry_id();
    fixture.add_child(entry, "a", "a.nia", "using super::b; pub static x = b::y;");
    fixture.add_child(entry, "b", "b.nia", "using super::a; pub static y = a::x;");

    let checked = fixture.database().check_program();

    assert!(
        checked.diagnostics.iter().all(|diagnostic| {
            diagnostic.diagnostic.code.def().category
                != nia_diagnostic::DiagnosticCategory::Internal
        }),
        "an ungrounded inferred-global type dependency should be diagnosed as source-level input: {:?}",
        checked.diagnostics
    );
    assert!(
        checked.diagnostics.iter().any(|diagnostic| {
            diagnostic
                .diagnostic
                .summary
                .contains("cyclic initializer dependency")
        }),
        "the cycle should produce a source-level type diagnostic: {:?}",
        checked.diagnostics
    );
}

#[test]
fn type_resolution_separates_semantic_value_from_diagnostics() {
    let fixture = LoadedProgramFixture::new("main.nia", "fun main(value: Missing) {}");
    let module_id = fixture.entry_id();
    let db = query_db(fixture.program());

    let resolution = db.expect_get(TypeResolutionQuery(module_id));
    assert!(resolution.semantic.diagnostics.is_empty());
    assert!(!resolve_diagnostic_bundle(&resolution.diagnostics).is_empty());
}

#[test]
fn signature_type_lowering_separates_semantic_value_from_diagnostics() {
    let fixture = LoadedProgramFixture::new(
        "main.nia",
        "struct Box[T] { value: T } fun main(value: Box) {}",
    );
    let module_id = fixture.entry_id();
    let db = query_db(fixture.program());

    let lowering = db.expect_get(SignatureTypeLoweringQuery(
        module_id,
        nia_item_tree::SignatureItemSet::Functions,
    ));
    assert!(lowering.semantic.diagnostics.is_empty());
    assert!(
        resolve_diagnostic_bundle(&lowering.diagnostics)
            .iter()
            .any(|diagnostic| diagnostic
                .summary
                .contains("generic argument count mismatch"))
    );
}

#[test]
fn type_lowering_separates_semantic_value_from_diagnostics() {
    let fixture = LoadedProgramFixture::new(
        "main.nia",
        "struct Box[T] { value: T } fun main(value: Box) {}",
    );
    let module_id = fixture.entry_id();
    let db = query_db(fixture.program());

    let lowering = db.expect_get(TypeLoweringQuery(module_id));
    assert!(lowering.semantic.diagnostics.is_empty());
    assert!(
        resolve_diagnostic_bundle(&lowering.diagnostics)
            .iter()
            .any(|diagnostic| diagnostic
                .summary
                .contains("generic argument count mismatch"))
    );
}

#[test]
fn signature_item_signatures_separate_semantic_value_from_diagnostics() {
    let fixture = LoadedProgramFixture::new("main.nia", "fun missing_body() ();");
    let module_id = fixture.entry_id();
    let db = query_db(fixture.program());

    let signatures = db.expect_get(SignatureItemSignaturesQuery(
        module_id,
        nia_item_tree::SignatureItemSet::Functions,
    ));
    assert!(signatures.semantic.diagnostics.is_empty());
    assert!(
        resolve_diagnostic_bundle(&signatures.diagnostics)
            .iter()
            .any(|diagnostic| diagnostic
                .summary
                .contains("bodyless non-extern functions require `@[builtin]`"))
    );
}

#[test]
fn item_signatures_separate_semantic_value_from_diagnostics() {
    let fixture = LoadedProgramFixture::new("main.nia", "fun missing_body() ();");
    let module_id = fixture.entry_id();
    let db = query_db(fixture.program());

    let signatures = db.expect_get(ItemSignaturesQuery(module_id));
    assert!(signatures.semantic.diagnostics.is_empty());
    assert!(
        resolve_diagnostic_bundle(&signatures.diagnostics)
            .iter()
            .any(|diagnostic| diagnostic
                .summary
                .contains("bodyless non-extern functions require `@[builtin]`"))
    );
}

#[test]
fn value_resolution_separates_semantic_value_from_diagnostics() {
    let mut fixture = LoadedProgramFixture::new(
        "main.nia",
        "module helper; fun main() i32 { helper::missing() }",
    );
    let module_id = fixture.entry_id();
    fixture.add_child(
        module_id,
        "helper",
        "helper.nia",
        "pub fun value() i32 { 1 }",
    );
    let db = query_db(fixture.program());

    let resolution = db.expect_get(ValueResolutionQuery(module_id));
    assert!(resolution.semantic.diagnostics.is_empty());
    assert!(!resolve_diagnostic_bundle(&resolution.diagnostics).is_empty());
}

#[test]
fn local_resolution_separates_semantic_value_from_diagnostics() {
    let fixture =
        LoadedProgramFixture::new("main.nia", "fun main(value: i32, value: i32) i32 { value }");
    let module_id = fixture.entry_id();
    let db = query_db(fixture.program());

    let resolution = db.expect_get(LocalResolutionQuery(module_id));
    assert!(resolution.semantic.diagnostics.is_empty());
    assert!(!resolve_diagnostic_bundle(&resolution.diagnostics).is_empty());
}

#[test]
fn uncaptured_outer_closure_locals_stop_before_backend_lowering() {
    let fixture = LoadedProgramFixture::new(
        "main.nia",
        r#"
fun main() i32 {
    let make = \x: i32, y: i32 -> \z: i32 -> x * y + z;
    let add = make(2, 3);
    add(4)
}
"#,
    );
    let codegen = fixture.database().codegen_program();

    assert!(
        codegen.diagnostics.iter().any(|diagnostic| diagnostic
            .diagnostic
            .summary
            .contains("not captured by this closure")),
        "{:?}",
        codegen.diagnostics
    );
    assert!(
        codegen.diagnostics.iter().all(|diagnostic| {
            diagnostic.diagnostic.category() != nia_diagnostic::DiagnosticCategory::Internal
        }),
        "frontend capture errors must not degrade into backend diagnostics: {:?}",
        codegen.diagnostics
    );
    assert!(codegen.backend_lowering.diagnostics.is_empty());
}

#[test]
fn uncaptured_method_receiver_stops_before_backend_lowering() {
    let fixture = LoadedProgramFixture::new(
        "main.nia",
        r#"
struct Box { value: i32 }

extend Box {
    fun run(self) i32 {
        let callback = \z: i32 -> self.value + z;
        callback(1)
    }
}

fun main() i32 { Box { value: 2 }.run() }
"#,
    );
    let codegen = fixture.database().codegen_program();

    assert!(
        codegen.diagnostics.iter().any(|diagnostic| diagnostic
            .diagnostic
            .summary
            .contains("`self` cannot cross a closure boundary")),
        "{:?}",
        codegen.diagnostics
    );
    assert!(
        codegen.diagnostics.iter().all(|diagnostic| {
            diagnostic.diagnostic.category() != nia_diagnostic::DiagnosticCategory::Internal
        }),
        "frontend receiver errors must not degrade into backend diagnostics: {:?}",
        codegen.diagnostics
    );
    assert!(codegen.backend_lowering.diagnostics.is_empty());
}

#[test]
fn flow_check_separates_semantic_value_from_diagnostics() {
    let fixture = LoadedProgramFixture::new(
        "main.nia",
        "fun main(flag: bool) i32 { if flag { return 1; } }",
    );
    let module_id = fixture.entry_id();
    let db = query_db(fixture.program());

    let flow_check = db.expect_get(FlowCheckQuery(module_id));
    assert!(flow_check.semantic.diagnostics.is_empty());
    assert!(
        resolve_diagnostic_bundle(&flow_check.diagnostics)
            .iter()
            .any(|diagnostic| diagnostic
                .summary
                .contains("does not return on all reachable paths"))
    );
}

#[test]
fn terminal_checks_separate_semantic_values_from_diagnostics() {
    let static_fixture = LoadedProgramFixture::new(
        "main.nia",
        "static global: i32 = make(); fun make() i32 { 1 }",
    );
    let static_module_id = static_fixture.entry_id();
    let static_db = query_db(static_fixture.program());
    let static_check = static_db.expect_get(StaticCheckQuery(static_module_id));

    assert!(static_check.semantic.diagnostics.is_empty());
    assert!(
        resolve_diagnostic_bundle(&static_check.diagnostics)
            .iter()
            .any(|diagnostic| diagnostic
                .summary
                .contains("global initializer is not static data"))
    );

    let abi_fixture = LoadedProgramFixture::new("main.nia", "extern fun bad(flag: bool) ();");
    let abi_module_id = abi_fixture.entry_id();
    let abi_db = query_db(abi_fixture.program());
    let abi_check = abi_db.expect_get(AbiCheckQuery(abi_module_id));

    assert!(abi_check.semantic.diagnostics.is_empty());
    assert!(
        resolve_diagnostic_bundle(&abi_check.diagnostics)
            .iter()
            .any(|diagnostic| diagnostic.summary.contains("cannot use `bool` directly"))
    );
}

#[test]
fn layouts_separate_semantic_value_from_diagnostics() {
    let fixture = LoadedProgramFixture::new("main.nia", "struct Node { next: Node }");
    let module_id = fixture.entry_id();
    let db = query_db(fixture.program());

    let layouts = db.expect_get(LayoutsQuery(module_id));
    assert!(layouts.semantic.diagnostics.is_empty());
    let diagnostic = resolve_diagnostic_bundle(&layouts.diagnostics)
        .iter()
        .find(|diagnostic| {
            diagnostic
                .summary
                .contains("recursive struct layout is not supported")
        })
        .expect("recursive layout diagnostic");
    assert!(
        diagnostic
            .notes
            .iter()
            .any(|note| note.contains("size would be infinite"))
    );
    assert!(
        diagnostic
            .help
            .iter()
            .any(|help| help.contains("pointer") && help.contains("indirection"))
    );
    assert!(
        diagnostic
            .labels
            .iter()
            .any(|label| label.style == nia_diagnostic::LabelStyle::Secondary)
    );
}

#[test]
fn const_check_separates_semantic_value_from_diagnostics() {
    let fixture = LoadedProgramFixture::new(
        "main.nia",
        "const a: i32 = b; const b: i32 = a; const independent: i32 = 42;",
    );
    let module_id = fixture.entry_id();
    let db = query_db(fixture.program());

    let const_eval = db.expect_get(ConstQuery(module_id));
    let defs = db.expect_get(ModuleDefsQuery(module_id));
    let independent = nia_const_check::ConstKey::Global(GlobalDefId {
        module_id,
        def_id: defs
            .semantic
            .module_scope
            .values
            .get(&sym("independent"))
            .expect("independent const definition"),
    });
    assert!(const_eval.semantic.diagnostics.is_empty());
    assert!(
        const_eval
            .diagnostics
            .iter()
            .any(|diagnostic| diagnostic.summary == "cyclic const dependency")
    );
    assert_eq!(
        const_eval.semantic.values.get(&independent),
        // A non-negative source literal decodes as an unsigned magnitude; a
        // source sign remains a unary expression. The recorded const value
        // therefore carries the magnitude's signedness, not the annotation's.
        Some(&nia_const_check::ConstValue::Int(
            nia_ty::IntConst::unsigned(42)
        ))
    );
    assert!(const_eval.semantic.typed_values.contains_key(&independent));
}

#[test]
fn monomorphization_separates_semantic_value_from_diagnostics() {
    let fixture = LoadedProgramFixture::new(
        "main.nia",
        "fun grow[T](value: &T) i32 { grow[&T](&value) } fun main() i32 { let value: i32 = 1; grow[i32](&value) }",
    );
    let db = query_db(fixture.program());

    let monomorphization = db.expect_get(MonomorphizationQuery);
    assert!(monomorphization.semantic.diagnostics.is_empty());
    assert!(
        resolve_diagnostic_bundle(&monomorphization.diagnostics)
            .iter()
            .any(|diagnostic| diagnostic.summary.contains("type depth limit"))
    );
}

#[test]
fn body_check_separates_semantic_value_from_diagnostics() {
    let fixture = LoadedProgramFixture::new("main.nia", "fun main() i32 { false }");
    let module_id = fixture.entry_id();
    let db = query_db(fixture.program());

    let body_check = db.expect_get(BodyCheckQuery(module_id));
    assert!(body_check.semantic.diagnostics.is_empty());
    assert!(!resolve_diagnostic_bundle(&body_check.diagnostics).is_empty());
}

#[test]
fn signature_type_normalization_separates_semantic_value_from_diagnostics() {
    let fixture = LoadedProgramFixture::new("main.nia", "type A = B; type B = A;");
    let module_id = fixture.entry_id();
    let db = query_db(fixture.program());

    let normalization = db.expect_get(SignatureTypeNormalizationQuery(
        module_id,
        nia_item_tree::SignatureItemSet::Types,
    ));
    assert!(normalization.semantic.diagnostics.is_empty());
    assert!(!resolve_diagnostic_bundle(&normalization.diagnostics).is_empty());
}

#[test]
fn type_normalization_separates_semantic_value_from_diagnostics() {
    let fixture = LoadedProgramFixture::new("main.nia", "type A = B; type B = A;");
    let module_id = fixture.entry_id();
    let db = query_db(fixture.program());

    let normalization = db.expect_get(TypeNormalizationQuery(module_id));
    assert!(normalization.semantic.diagnostics.is_empty());
    assert!(!resolve_diagnostic_bundle(&normalization.diagnostics).is_empty());
}
