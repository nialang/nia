// SPDX-License-Identifier: GPL-3.0-or-later
use super::*;

#[test]
fn revision_only_update_refreshes_all_revision_bearing_products() {
    let source = "pub struct S { value: i32 } fn main() i32 { let value: i32 = 0; value }";
    let mut fixture = LoadedProgramFixture::new("main.nia", source);
    let module_id = fixture.entry_id();
    let database = CompilerDatabase::new(CompileRequest::new(fixture.program()));

    let first = database.check_program();
    assert!(first.diagnostics.is_empty(), "{:?}", first.diagnostics);
    let first_tree = database
        .db
        .expect_get(DeclarationModuleItemTreeInputQuery(module_id));
    let first_defs = database.db.expect_get(ModuleDefsQuery(module_id));
    assert!(
        first_tree
            .items
            .iter()
            .all(|item| item.node_key.revision == SourceRevision::INITIAL)
    );
    assert!(
        first_defs
            .semantic
            .def_nodes
            .entries()
            .all(|(key, _)| key.revision == SourceRevision::INITIAL)
    );

    fixture.update_module_source(module_id, source, SourceRevision(1));
    database.update(CompileRequest::new(fixture.program()));
    let before_second_check = database.query_trace();

    let second = database.check_program();
    assert!(second.diagnostics.is_empty(), "{:?}", second.diagnostics);
    let latest_tree = database
        .db
        .expect_get(DeclarationModuleItemTreeInputQuery(module_id));
    let latest_defs = database.db.expect_get(ModuleDefsQuery(module_id));
    let after_second_check = database.query_trace();

    assert!(!Arc::ptr_eq(&first_tree, &latest_tree));
    assert!(!Arc::ptr_eq(&first_defs, &latest_defs));
    assert!(
        latest_tree
            .items
            .iter()
            .all(|item| item.node_key.revision == SourceRevision(1))
    );
    assert!(
        latest_defs
            .semantic
            .def_nodes
            .entries()
            .all(|(key, _)| key.revision == SourceRevision(1))
    );
    assert!(
        query_executions(&before_second_check, "declaration_type_lowering")
            < query_executions(&after_second_check, "declaration_type_lowering")
    );
    assert!(
        query_executions(&before_second_check, "item_signatures")
            < query_executions(&after_second_check, "item_signatures")
    );
}

#[test]
fn type_store_preserves_published_slots_across_database_updates() {
    let mut fixture = LoadedProgramFixture::new("main.nia", "pub struct S { value: i32 }");
    let module_id = fixture.entry_id();
    let database = CompilerDatabase::new(CompileRequest::new(fixture.program()));
    let first_lowering = database.db.expect_get(TypeLoweringQuery(module_id));
    let type_store = &database.db.context().type_store;
    let first_i32 = type_store
        .append_for_module(module_id)
        .intern(nia_ty::TyKind::Primitive(nia_ty::PrimitiveTy::I32));
    assert!(
        first_lowering
            .semantic
            .explicit_type_roots()
            .into_iter()
            .all(|ty| type_store.get(ty).is_some())
    );

    fixture.update_module_source(
        module_id,
        "pub struct S { value: i32, flag: &bool }",
        SourceRevision(1),
    );
    database.update(CompileRequest::new(fixture.program()));
    let second_lowering = database.db.expect_get(TypeLoweringQuery(module_id));

    assert_eq!(
        type_store.get(first_i32),
        Some(&nia_ty::TyKind::Primitive(nia_ty::PrimitiveTy::I32))
    );
    assert!(
        second_lowering
            .semantic
            .explicit_type_roots()
            .into_iter()
            .any(|ty| {
                matches!(
                    type_store.get(ty),
                    Some(nia_ty::TyKind::Pointer { elem, .. })
                        if matches!(
                            type_store.get(*elem),
                            Some(nia_ty::TyKind::Primitive(nia_ty::PrimitiveTy::Bool))
                        )
                )
            })
    );
}

#[test]
fn type_normalization_appends_to_the_session_type_store() {
    let fixture = LoadedProgramFixture::new(
        "main.nia",
        "type ByteRef = &u8; pub fn read(value: ByteRef) u8 { 0 }",
    );
    let module_id = fixture.entry_id();
    let database = fixture.database();
    let lowering = database.db.expect_get(TypeLoweringQuery(module_id));
    let normalization = database.db.expect_get(TypeNormalizationQuery(module_id));
    let type_store = &database.db.context().type_store;

    for ty_id in lowering.semantic.explicit_type_roots() {
        assert!(type_store.get(ty_id).is_some());
    }
    for normalized in normalization.semantic.normalized.values() {
        assert!(type_store.get(*normalized).is_some());
    }
    assert!(
        normalization
            .semantic
            .normalized
            .iter()
            .any(|(source, normalized)| source != normalized)
    );
}

#[test]
fn const_phases_publish_synthesized_types_to_canonical_store() {
    let fixture = LoadedProgramFixture::new(
        "main.nia",
        r#"
const values = 0usize..3usize;
const width: usize = values.end();

fn main() i32 { 0 }
"#,
    );
    let module_id = fixture.entry_id();
    let database = fixture.database();
    let lowering = database.db.expect_get(TypeLoweringQuery(module_id));
    let _ = database.db.expect_get(TypeNormalizationQuery(module_id));

    let _ = database.db.expect_get(ConstArrayLengthsQuery(module_id));
    let _ = database.db.expect_get(ConstEnumValuesQuery(module_id));
    let values = database.db.expect_get(ConstValuesQuery(module_id));
    let _ = database.db.expect_get(ConstTypedFactsQuery(module_id));
    let _ = database.db.expect_get(ConstQuery(module_id));

    for ty in lowering.semantic.explicit_type_roots() {
        assert!(database.db.context().type_store.get(ty).is_some());
    }
    let range_ty = values
        .typed_values
        .values()
        .filter_map(|value| value.ty.runtime())
        .find(|ty| {
            matches!(
                database.db.context().type_store.get(*ty),
                Some(nia_ty::TyKind::Range { .. })
            )
        })
        .expect("const range type published to canonical store");
    assert!(database.db.context().type_store.get(range_ty).is_some());
}

#[test]
fn body_check_publishes_synthesized_types_to_canonical_store() {
    let fixture = LoadedProgramFixture::new(
        "main.nia",
        r#"
fn main() i32 {
let values = [1i32, 2i32, 3i32];
values[0]
}
"#,
    );
    let module_id = fixture.entry_id();
    let database = fixture.database();
    let _ = database.db.expect_get(ConstQuery(module_id));

    let body = database.db.expect_get(BodyCheckQuery(module_id));

    assert!(body.semantic.facts.function_facts.values().any(|facts| {
        facts.local_types.values().any(|ty| {
            matches!(
                database.db.context().type_store.get(*ty),
                Some(nia_ty::TyKind::Array {
                    len: nia_ty::ArrayLenTy::ConstValue(3),
                    ..
                })
            )
        })
    }));
}

#[test]
fn signature_and_full_normalization_share_ids_in_either_query_order() {
    fn assert_order(signature_first: bool) {
        let fixture = LoadedProgramFixture::new(
            "main.nia",
            "type Ref[T] = &T; pub fn read(value: Ref[u16]) u16 { 0 }",
        );
        let module_id = fixture.entry_id();
        let database = fixture.database();
        let signature_key =
            SignatureTypeNormalizationQuery(module_id, nia_item_tree::SignatureItemSet::Functions);
        let (signature, full) = if signature_first {
            let signature = database.db.expect_get(signature_key);
            let full = database.db.expect_get(TypeNormalizationQuery(module_id));
            (signature, full)
        } else {
            let full = database.db.expect_get(TypeNormalizationQuery(module_id));
            let signature = database.db.expect_get(signature_key);
            (signature, full)
        };

        assert!(
            signature
                .semantic
                .normalized
                .values()
                .chain(full.semantic.normalized.values())
                .all(|ty| database.db.context().type_store.get(*ty).is_some())
        );
        let shared_alias_expansions = signature
            .semantic
            .normalized
            .iter()
            .filter(|(source, normalized)| {
                source != normalized && full.semantic.normalized.get(source) == Some(normalized)
            })
            .count();
        assert!(
            shared_alias_expansions > 0,
            "signature/full normalization did not share an alias expansion"
        );
    }

    assert_order(true);
    assert_order(false);
}

#[test]
fn type_store_isolates_compiler_database_handle_identity() {
    let first_fixture = LoadedProgramFixture::new("main.nia", "pub struct S { value: i32 }");
    let second_fixture = LoadedProgramFixture::new("main.nia", "pub struct S { value: i32 }");
    let first_module_id = first_fixture.entry_id();
    let second_module_id = second_fixture.entry_id();
    let first = first_fixture.database();
    let second = second_fixture.database();
    let _ = first.db.expect_get(TypeLoweringQuery(first_module_id));
    let _ = second.db.expect_get(TypeLoweringQuery(second_module_id));
    let first_store = &first.db.context().type_store;
    let second_store = &second.db.context().type_store;
    let first_i32 = first_store
        .append_for_module(first_module_id)
        .intern(nia_ty::TyKind::Primitive(nia_ty::PrimitiveTy::I32));
    let second_i32 = second_store
        .append_for_module(second_module_id)
        .intern(nia_ty::TyKind::Primitive(nia_ty::PrimitiveTy::I32));

    assert_ne!(first_store.id(), second_store.id());
    assert_ne!(first_i32, second_i32);
    assert_eq!(first_store.get(second_i32), None);
    assert_eq!(second_store.get(first_i32), None);
}
