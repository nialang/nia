// SPDX-License-Identifier: GPL-3.0-or-later

use super::*;

#[test]
fn body_check_resolves_program_signatures_through_precise_signature_queries() {
    let mut fixture = LoadedProgramFixture::new(
        "main.nia",
        "using helper::{Alias, value}; fn main() Alias { value() }",
    );
    let entry_id = fixture.entry_id();
    fixture.add_child(
        entry_id,
        "helper",
        "helper.nia",
        "pub type Alias = i32; pub fn value() Alias { 1 }",
    );
    let db = query_db(fixture.program());

    let checked = db.expect_get(BodyCheckQuery(entry_id));
    assert!(checked.diagnostics.is_empty(), "{:?}", checked.diagnostics);
    let trace = db.query_trace();

    assert!(trace_has_dependency(
        &trace,
        "body_check",
        "signature_item_signatures"
    ));
    assert!(!trace_has_dependency(
        &trace,
        "body_check",
        "signature_type_lowering"
    ));
    assert!(!trace_has_dependency(
        &trace,
        "body_check",
        "visible_extensions"
    ));
    assert!(!trace_has_dependency(
        &trace,
        "body_check",
        "program_trait_solving_signatures"
    ));
    assert!(!trace.dependencies.iter().any(|dependency| {
        dependency.from.name == "body_check"
            && matches!(
                dependency.to.name,
                "program_body_value_signatures"
                    | "program_body_type_signatures"
                    | "program_body_trait_signatures"
            )
    }));
}

#[test]
fn body_check_resolves_trait_method_candidates_through_program_trait_method_index() {
    let mut fixture = LoadedProgramFixture::new(
        "main.nia",
        r#"
module traits;
using entry::traits::{Ops, Value};

fn main() i32 {
let value = Value {};
value.used()
}
"#,
    );
    let entry_id = fixture.entry_id();
    fixture.add_child(
        entry_id,
        "traits",
        "traits.nia",
        r#"
pub trait Ops {
fn used(self) i32;
}

pub struct Value {}

extend Value : Ops {
fn used(self) i32 {
    1
}
}
"#,
    );
    let db = query_db(fixture.program());

    let checked = db.expect_get(BodyCheckQuery(entry_id));
    assert!(checked.diagnostics.is_empty(), "{:?}", checked.diagnostics);
    let trace = db.query_trace();

    assert!(trace_has_dependency(
        &trace,
        "body_check",
        "program_trait_method_index"
    ));
    assert!(trace_has_dependency(
        &trace,
        "program_trait_method_index",
        "module_program_signature_facts"
    ));
    assert!(trace_has_dependency(
        &trace,
        "program_trait_method_index",
        "program_signature_module_ids"
    ));
    assert!(!trace.dependencies.iter().any(|dependency| {
        dependency.from.name == "body_check"
            && dependency.to.name == "module_program_signature_facts"
            && dependency.to.description.contains("Traits")
    }));
}

#[test]
fn program_signature_module_ids_use_set_specific_module_facts() {
    let mut fixture = LoadedProgramFixture::new(
        "main.nia",
        "module module1; module module2; module module3; module module4; module module5; module module6;",
    );
    let entry_id = fixture.entry_id();
    let module1 = fixture.add_child(
        entry_id,
        "module1",
        "module1.nia",
        "struct S { value: i32 }",
    );
    let module2 = fixture.add_child(entry_id, "module2", "module2.nia", "fn helper() i32 { 1 }");
    let module3 = fixture.add_child(
        entry_id,
        "module3",
        "module3.nia",
        "const WIDTH: usize = 4usize;",
    );
    let module4 = fixture.add_child(
        entry_id,
        "module4",
        "module4.nia",
        "trait Read { fn read(self) i32; }",
    );
    let module5 = fixture.add_child(
        entry_id,
        "module5",
        "module5.nia",
        "struct T {} extend T { pub fn make() T { {} } }",
    );
    let module6 = fixture.add_child(
        entry_id,
        "module6",
        "module6.nia",
        "struct U {} extend U { const WIDTH: usize = 4usize; }",
    );
    let db = query_db(fixture.program());

    assert_eq!(
        resolve_stable_module_sequence(
            &db,
            &db.expect_get(ProgramSignatureModuleIdsQuery(
                nia_item_tree::SignatureItemSet::Functions
            ))
        )
        .expect("function signature module sequence")
        .as_slice(),
        &[module2, module4, module5]
    );
    assert_eq!(
        resolve_stable_module_sequence(
            &db,
            &db.expect_get(ProgramSignatureModuleIdsQuery(
                nia_item_tree::SignatureItemSet::Values
            ))
        )
        .expect("value signature module sequence")
        .as_slice(),
        &[module3, module6]
    );
    assert_eq!(
        resolve_stable_module_sequence(
            &db,
            &db.expect_get(ProgramSignatureModuleIdsQuery(
                nia_item_tree::SignatureItemSet::Types
            ))
        )
        .expect("type signature module sequence")
        .as_slice(),
        &[module1, module5, module6]
    );
    assert_eq!(
        resolve_stable_module_sequence(
            &db,
            &db.expect_get(ProgramSignatureModuleIdsQuery(
                nia_item_tree::SignatureItemSet::Traits
            ))
        )
        .expect("trait signature module sequence")
        .as_slice(),
        &[module4, module5, module6]
    );
    assert_eq!(
        resolve_stable_module_sequence(
            &db,
            &db.expect_get(ProgramSignatureModuleIdsQuery(
                nia_item_tree::SignatureItemSet::ExtensionFunctions
            ))
        )
        .expect("extension signature module sequence")
        .as_slice(),
        &[module4, module5]
    );

    let trace = db.query_trace();
    assert!(trace.dependencies.iter().any(|dependency| {
        dependency.from.name == "program_signature_module_ids"
            && dependency.to.name == "program_signature_module_eligibility"
    }));
    assert!(trace.dependencies.iter().any(|dependency| {
        dependency.from.name == "program_signature_module_eligibility"
            && dependency.to.name == "signature_item_tree"
    }));
    assert!(!trace.dependencies.iter().any(|dependency| {
        dependency.from.name == "program_signature_module_eligibility"
            && matches!(
                dependency.to.name,
                "signature_type_lowering" | "signature_item_signatures" | "module_defs"
            )
    }));
}

#[test]
fn extension_provider_module_ids_use_parse_ok_provider_summaries() {
    let mut fixture = LoadedProgramFixture::new(
        "main.nia",
        "module module1; module module2; module module3; module module4; module module5;",
    );
    let entry_id = fixture.entry_id();
    fixture.add_child(
        entry_id,
        "module1",
        "module1.nia",
        "struct S { value: i32 }",
    );
    fixture.add_child(entry_id, "module2", "module2.nia", "fn helper() i32 { 1 }");
    fixture.add_child(
        entry_id,
        "module3",
        "module3.nia",
        "const WIDTH: usize = 4usize;",
    );
    fixture.add_child(
        entry_id,
        "module4",
        "module4.nia",
        "trait Read { fn read(self) i32; }",
    );
    let module5 = fixture.add_child(
        entry_id,
        "module5",
        "module5.nia",
        "struct T {} extend T { pub fn make() T { {} } }",
    );
    let db = query_db(fixture.program());

    assert_eq!(
        resolve_stable_module_sequence(&db, &db.expect_get(ExtensionProviderModuleIdsQuery))
            .expect("extension provider module sequence")
            .as_slice(),
        &[module5]
    );
    let trace = db.query_trace();
    assert!(trace_has_dependency(
        &trace,
        "extension_provider_module_ids",
        "parse_ok_module_ids"
    ));
    assert!(trace_has_dependency(
        &trace,
        "extension_provider_module_ids",
        "extension_provider_module_eligibility"
    ));
}

#[test]
fn program_type_alias_signature_uses_precise_module_facts() {
    let fixture = LoadedProgramFixture::new(
        "main.nia",
        "struct S { value: i32 } type Alias = S; fn helper() i32 { 1 }",
    );
    let module_id = fixture.entry_id();
    let db = query_db(fixture.program());

    let defs = db.expect_get(ModuleDefsQuery(module_id));
    let alias_id = defs.semantic.module_scope.types.get(&sym("Alias")).unwrap();
    let _ = db.expect_get(ProgramTypeAliasSignatureQuery(GlobalDefId {
        module_id,
        def_id: alias_id,
    }));
    let trace = db.query_trace();

    assert!(trace_has_dependency(
        &trace,
        "program_type_alias_signature",
        "module_program_signature_facts"
    ));
    assert!(!trace_has_dependency(
        &trace,
        "program_type_alias_signature",
        "program_signature_module_ids"
    ));
}

#[test]
fn layout_uses_full_type_module_signatures_and_array_lengths_without_body_products() {
    let fixture =
        LoadedProgramFixture::new("main.nia", "struct S { value: i32 } fn helper() i32 { 1 }");
    let module_id = fixture.entry_id();
    let db = query_db(fixture.program());

    let layouts = db.expect_get(LayoutsQuery(module_id));
    let trace = db.query_trace();

    assert!(
        layouts.semantic.diagnostics.is_empty(),
        "{:?}",
        layouts.semantic.diagnostics
    );
    assert!(trace.dependencies.iter().any(|dependency| {
        dependency.from.name == "layouts" && dependency.to.name == "layout_type_normalization"
    }));
    assert!(trace.dependencies.iter().any(|dependency| {
        dependency.from.name == "layouts" && dependency.to.name == "const_array_lengths"
    }));
    assert!(trace.dependencies.iter().any(|dependency| {
        dependency.from.name == "layouts" && dependency.to.name == "item_signatures"
    }));
    assert!(!trace.dependencies.iter().any(|dependency| {
        dependency.from.name == "layouts"
            && matches!(
                dependency.to.name,
                "type_normalization" | "const" | "body_check"
            )
    }));
}

#[test]
fn ordinary_and_signature_layouts_use_artifact_pointer_width() {
    let fixture = LoadedProgramFixture::new("main.nia", "pub struct Word { value: usize }");
    let module_id = fixture.entry_id();
    let mut program = fixture.program();
    program.target.pointer_width = 32;
    let db = query_db(program);

    let defs = db.expect_get(ModuleDefsQuery(module_id));
    let word = defs
        .semantic
        .module_scope
        .types
        .get(&sym("Word"))
        .expect("Word definition");
    for layouts in [
        db.expect_get(LayoutsQuery(module_id)),
        db.expect_get(SignatureLayoutsQuery(module_id)),
    ] {
        assert_eq!(
            layouts.semantic.target,
            nia_layout::TargetDataLayout {
                pointer_size: 4,
                pointer_align: 4,
            }
        );
        assert_eq!(
            layouts
                .semantic
                .structs
                .get(&word)
                .expect("Word layout")
                .layout,
            nia_layout::TypeLayout { size: 4, align: 4 }
        );
    }
}

#[test]
fn layout_uses_signature_layouts_for_cross_module_types() {
    let mut fixture = LoadedProgramFixture::new(
        "main.nia",
        "module module1; using self::module1::S; struct Holder { value: S }",
    );
    let entry_id = fixture.entry_id();
    let module1 = fixture.add_child(
        entry_id,
        "module1",
        "module1.nia",
        "pub struct S { value: i32 } fn helper() i32 { 1 }",
    );
    let db = query_db(fixture.program());

    let layouts = db.expect_get(LayoutsQuery(entry_id));
    let trace = db.query_trace();
    let entry_description = format!("{entry_id:?}");
    let module1_description = format!("{module1:?}");

    assert!(
        layouts.semantic.diagnostics.is_empty(),
        "{:?}",
        layouts.semantic.diagnostics
    );
    assert!(trace.dependencies.iter().any(|dependency| {
        dependency.from.name == "layouts"
            && dependency.from.description.contains(&entry_description)
            && dependency.to.name == "signature_layouts"
            && dependency.to.description.contains(&module1_description)
    }));
    assert!(!trace.dependencies.iter().any(|dependency| {
        dependency.from.name == "layouts"
            && dependency.from.description.contains(&entry_description)
            && dependency.to.name == "layouts"
            && dependency.to.description.contains(&module1_description)
    }));
}

#[test]
fn signature_layout_reads_canonical_types_from_store() {
    let mut fixture = LoadedProgramFixture::new(
        "main.nia",
        "module module1; using self::module1::Box; struct Holder { value: Box[u16] }",
    );
    let entry_id = fixture.entry_id();
    fixture.add_child(
        entry_id,
        "module1",
        "module1.nia",
        "pub struct Box[T] { value: [3]T }",
    );
    let db = query_db(fixture.program());
    let signature_types = nia_item_tree::SignatureItemSet::Types;

    let _ = db.expect_get(SignatureTypeNormalizationQuery(entry_id, signature_types));
    let _ = db.expect_get(SignatureItemSignaturesQuery(entry_id, signature_types));
    let layouts = db.expect_get(SignatureLayoutsQuery(entry_id));

    assert!(
        layouts.semantic.diagnostics.is_empty(),
        "{:?}",
        layouts.semantic.diagnostics
    );
    assert!(
        layouts
            .semantic
            .types
            .keys()
            .all(|ty| db.context().type_store.get(*ty).is_some())
    );
}

#[test]
fn abi_check_uses_abi_signature_index_not_body_signatures() {
    let fixture = LoadedProgramFixture::new(
        "main.nia",
        "extern struct S { value: i32 } extern fn take(value: S) ();",
    );
    let module_id = fixture.entry_id();
    let db = query_db(fixture.program());

    let _ = db.expect_get(AbiCheckQuery(module_id));
    let trace = db.query_trace();

    assert!(trace.dependencies.iter().any(|dependency| {
        dependency.from.name == "abi_check" && dependency.to.name == "program_abi_signatures"
    }));
    assert!(trace.dependencies.iter().any(|dependency| {
        dependency.from.name == "program_abi_signatures"
            && dependency.to.name == "module_abi_signature_facts"
    }));
    assert!(trace.dependencies.iter().any(|dependency| {
        dependency.from.name == "module_abi_signature_facts"
            && dependency.to.name == "signature_item_signatures"
    }));
    assert!(trace.dependencies.iter().any(|dependency| {
        dependency.from.name == "abi_check" && dependency.to.name == "signature_item_signatures"
    }));
    assert!(!trace.dependencies.iter().any(|dependency| {
        matches!(
            dependency.from.name,
            "program_abi_signatures" | "module_abi_signature_facts"
        ) && matches!(
            dependency.to.name,
            "item_signatures" | "type_normalization" | "signature_type_lowering"
        )
    }));
    assert!(!depends_on_body_signature_query(&trace, "abi_check"));
}
