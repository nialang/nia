// SPDX-License-Identifier: GPL-3.0-or-later

use super::*;

#[test]
fn signature_type_resolution_separates_semantic_value_from_diagnostics() {
    let fixture = LoadedProgramFixture::new("main.nia", "fn main(value: Missing) {}");
    let module_id = fixture.entry_id();
    let db = query_db(fixture.program());

    let resolution = db.expect_get(SignatureTypeResolutionQuery(
        module_id,
        nia_item_tree::SignatureItemSet::Functions,
    ));
    assert!(resolution.semantic.diagnostics.is_empty());
    assert!(!resolve_diagnostic_bundle(db.context(), &resolution.diagnostics).is_empty());
}

#[test]
fn type_resolution_separates_semantic_value_from_diagnostics() {
    let fixture = LoadedProgramFixture::new("main.nia", "fn main(value: Missing) {}");
    let module_id = fixture.entry_id();
    let db = query_db(fixture.program());

    let resolution = db.expect_get(TypeResolutionQuery(module_id));
    assert!(resolution.semantic.diagnostics.is_empty());
    assert!(!resolve_diagnostic_bundle(db.context(), &resolution.diagnostics).is_empty());
}

#[test]
fn signature_type_lowering_separates_semantic_value_from_diagnostics() {
    let fixture = LoadedProgramFixture::new(
        "main.nia",
        "struct Box[T] { value: T } fn main(value: Box) {}",
    );
    let module_id = fixture.entry_id();
    let db = query_db(fixture.program());

    let lowering = db.expect_get(SignatureTypeLoweringQuery(
        module_id,
        nia_item_tree::SignatureItemSet::Functions,
    ));
    assert!(lowering.semantic.diagnostics.is_empty());
    assert!(
        resolve_diagnostic_bundle(db.context(), &lowering.diagnostics)
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
        "struct Box[T] { value: T } fn main(value: Box) {}",
    );
    let module_id = fixture.entry_id();
    let db = query_db(fixture.program());

    let lowering = db.expect_get(TypeLoweringQuery(module_id));
    assert!(lowering.semantic.diagnostics.is_empty());
    assert!(
        resolve_diagnostic_bundle(db.context(), &lowering.diagnostics)
            .iter()
            .any(|diagnostic| diagnostic
                .summary
                .contains("generic argument count mismatch"))
    );
}

#[test]
fn signature_item_signatures_separate_semantic_value_from_diagnostics() {
    let fixture = LoadedProgramFixture::new("main.nia", "fn missing_body() ();");
    let module_id = fixture.entry_id();
    let db = query_db(fixture.program());

    let signatures = db.expect_get(SignatureItemSignaturesQuery(
        module_id,
        nia_item_tree::SignatureItemSet::Functions,
    ));
    assert!(signatures.semantic.diagnostics.is_empty());
    assert!(
        resolve_diagnostic_bundle(db.context(), &signatures.diagnostics)
            .iter()
            .any(|diagnostic| diagnostic
                .summary
                .contains("bodyless non-extern functions require `@[builtin]`"))
    );
}

#[test]
fn item_signatures_separate_semantic_value_from_diagnostics() {
    let fixture = LoadedProgramFixture::new("main.nia", "fn missing_body() ();");
    let module_id = fixture.entry_id();
    let db = query_db(fixture.program());

    let signatures = db.expect_get(ItemSignaturesQuery(module_id));
    assert!(signatures.semantic.diagnostics.is_empty());
    assert!(
        resolve_diagnostic_bundle(db.context(), &signatures.diagnostics)
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
        "module helper; fn main() i32 { helper::missing() }",
    );
    let module_id = fixture.entry_id();
    fixture.add_child(
        module_id,
        "helper",
        "helper.nia",
        "pub fn value() i32 { 1 }",
    );
    let db = query_db(fixture.program());

    let resolution = db.expect_get(ValueResolutionQuery(module_id));
    assert!(resolution.semantic.diagnostics.is_empty());
    assert!(!resolve_diagnostic_bundle(db.context(), &resolution.diagnostics).is_empty());
}

#[test]
fn local_resolution_separates_semantic_value_from_diagnostics() {
    let fixture =
        LoadedProgramFixture::new("main.nia", "fn main(value: i32, value: i32) i32 { value }");
    let module_id = fixture.entry_id();
    let db = query_db(fixture.program());

    let resolution = db.expect_get(LocalResolutionQuery(module_id));
    assert!(resolution.semantic.diagnostics.is_empty());
    assert!(!resolve_diagnostic_bundle(db.context(), &resolution.diagnostics).is_empty());
}

#[test]
fn uncaptured_outer_closure_locals_stop_before_backend_lowering() {
    let fixture = LoadedProgramFixture::new(
        "main.nia",
        r#"
fn main() i32 {
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
            diagnostic.diagnostic.category != nia_diagnostic::DiagnosticCategory::Internal
        }),
        "frontend capture errors must not degrade into backend diagnostics: {:?}",
        codegen.diagnostics
    );
    assert!(codegen.backend_lowering.diagnostics.is_empty());
}

#[test]
fn flow_check_separates_semantic_value_from_diagnostics() {
    let fixture = LoadedProgramFixture::new(
        "main.nia",
        "fn main(flag: bool) i32 { if flag { return 1; } }",
    );
    let module_id = fixture.entry_id();
    let db = query_db(fixture.program());

    let flow_check = db.expect_get(FlowCheckQuery(module_id));
    assert!(flow_check.semantic.diagnostics.is_empty());
    assert!(
        resolve_diagnostic_bundle(db.context(), &flow_check.diagnostics)
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
        "static global: i32 = make(); fn make() i32 { 1 }",
    );
    let static_module_id = static_fixture.entry_id();
    let static_db = query_db(static_fixture.program());
    let static_check = static_db.expect_get(StaticCheckQuery(static_module_id));

    assert!(static_check.semantic.diagnostics.is_empty());
    assert!(
        resolve_diagnostic_bundle(static_db.context(), &static_check.diagnostics)
            .iter()
            .any(|diagnostic| diagnostic
                .summary
                .contains("global initializer is not static data"))
    );

    let abi_fixture = LoadedProgramFixture::new("main.nia", "extern fn bad(flag: bool) ();");
    let abi_module_id = abi_fixture.entry_id();
    let abi_db = query_db(abi_fixture.program());
    let abi_check = abi_db.expect_get(AbiCheckQuery(abi_module_id));

    assert!(abi_check.semantic.diagnostics.is_empty());
    assert!(
        resolve_diagnostic_bundle(abi_db.context(), &abi_check.diagnostics)
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
    assert!(
        resolve_diagnostic_bundle(db.context(), &layouts.diagnostics)
            .iter()
            .any(|diagnostic| diagnostic
                .summary
                .contains("recursive struct layout is not supported"))
    );
}

#[test]
fn const_check_separates_semantic_value_from_diagnostics() {
    let fixture = LoadedProgramFixture::new("main.nia", "const a: i32 = b; const b: i32 = a;");
    let module_id = fixture.entry_id();
    let db = query_db(fixture.program());

    let const_eval = db.expect_get(ConstQuery(module_id));
    assert!(const_eval.semantic.diagnostics.is_empty());
    assert!(!resolve_diagnostic_bundle(db.context(), &const_eval.diagnostics).is_empty());
}

#[test]
fn monomorphization_separates_semantic_value_from_diagnostics() {
    let fixture = LoadedProgramFixture::new(
        "main.nia",
        "fn grow[T](value: &T) i32 { grow[&T](&value) } fn main() i32 { let value: i32 = 1; grow[i32](&value) }",
    );
    let db = query_db(fixture.program());

    let monomorphization = db.expect_get(MonomorphizationQuery);
    assert!(monomorphization.semantic.diagnostics.is_empty());
    assert!(
        resolve_diagnostic_bundle(db.context(), &monomorphization.diagnostics)
            .iter()
            .any(|diagnostic| diagnostic.summary.contains("type depth limit"))
    );
}

#[test]
fn body_check_separates_semantic_value_from_diagnostics() {
    let fixture = LoadedProgramFixture::new("main.nia", "fn main() i32 { false }");
    let module_id = fixture.entry_id();
    let db = query_db(fixture.program());

    let body_check = db.expect_get(BodyCheckQuery(module_id));
    assert!(body_check.semantic.diagnostics.is_empty());
    assert!(!resolve_diagnostic_bundle(db.context(), &body_check.diagnostics).is_empty());
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
    assert!(!resolve_diagnostic_bundle(db.context(), &normalization.diagnostics).is_empty());
}

#[test]
fn type_normalization_separates_semantic_value_from_diagnostics() {
    let fixture = LoadedProgramFixture::new("main.nia", "type A = B; type B = A;");
    let module_id = fixture.entry_id();
    let db = query_db(fixture.program());

    let normalization = db.expect_get(TypeNormalizationQuery(module_id));
    assert!(normalization.semantic.diagnostics.is_empty());
    assert!(!resolve_diagnostic_bundle(db.context(), &normalization.diagnostics).is_empty());
}
