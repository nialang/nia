use super::*;

#[test]
fn computes_nia_struct_layout_in_physical_field_order() {
    let module_ids = ModuleIdAllocator::new().expect("create module ID allocator");
    let module_id = module_ids.allocate().expect("allocate module ID");
    let (module, symbols) = parse_test_module(
        r#"
struct Mixed {
a: u8,
b: i64,
c: u8,
}
"#,
    );
    let defs = collect_module_defs(module_id, &module);
    let resolved = resolve_module_types_with_symbols(&module, &defs, &symbols);
    let (type_store, lowered) = lower_test_module(&module, &resolved, &defs);
    let signatures = collect_test_signatures(&module, &defs, &lowered, &type_store);
    let mixed_id = defs
        .module_scope
        .types
        .get(&sym("Mixed"))
        .expect("Mixed def");
    let signature = signatures.structs.get(&mixed_id).expect("Mixed signature");
    let a_id = signature.fields[0].def_id;
    let b_id = signature.fields[1].def_id;
    let c_id = signature.fields[2].def_id;
    let layouts = compute_layouts(&type_store, &defs, &signatures, TargetDataLayout::LP64)
        .expect("compute layouts");
    assert!(layouts.diagnostics.is_empty(), "{:?}", layouts.diagnostics);
    let mixed = layouts.structs.get(&mixed_id).expect("Mixed layout");
    assert_eq!(mixed.layout, TypeLayout { size: 16, align: 8 });
    assert_eq!(
        mixed
            .fields
            .iter()
            .map(|field| (field.def_id, field.offset))
            .collect::<Vec<_>>(),
        vec![(b_id, 0), (a_id, 8), (c_id, 9)]
    );
}

#[test]
fn preserves_tuple_struct_field_order() {
    let module_ids = ModuleIdAllocator::new().expect("create module ID allocator");
    let module_id = module_ids.allocate().expect("allocate module ID");
    let (module, symbols) = parse_test_module("struct Mixed(u8, i64, u16)");
    let defs = collect_module_defs(module_id, &module);
    let resolved = resolve_module_types_with_symbols(&module, &defs, &symbols);
    let (type_store, lowered) = lower_test_module(&module, &resolved, &defs);
    let signatures = collect_test_signatures(&module, &defs, &lowered, &type_store);
    let mixed_id = defs
        .module_scope
        .types
        .get(&sym("Mixed"))
        .expect("Mixed def");
    let signature = signatures.structs.get(&mixed_id).expect("Mixed signature");
    let field_ids = signature
        .fields
        .iter()
        .map(|field| field.def_id)
        .collect::<Vec<_>>();
    let layouts = compute_layouts(&type_store, &defs, &signatures, TargetDataLayout::LP64)
        .expect("compute layouts");
    assert!(layouts.diagnostics.is_empty(), "{:?}", layouts.diagnostics);
    let mixed = layouts.structs.get(&mixed_id).expect("Mixed layout");
    assert_eq!(mixed.layout, TypeLayout { size: 24, align: 8 });
    assert_eq!(
        mixed
            .fields
            .iter()
            .map(|field| (field.def_id, field.offset))
            .collect::<Vec<_>>(),
        vec![(field_ids[0], 0), (field_ids[1], 8), (field_ids[2], 16)]
    );
}

#[test]
fn ignores_inferred_array_placeholders_during_global_layout_scan() {
    let module_ids = ModuleIdAllocator::new().expect("create module ID allocator");
    let module_id = module_ids.allocate().expect("allocate module ID");
    let (module, symbols) = parse_test_module(
        r#"
fn main() {
let mut xs: [u8; _] = [1, 2];
}
"#,
    );
    let defs = collect_module_defs(module_id, &module);
    let resolved = resolve_module_types_with_symbols(&module, &defs, &symbols);
    let (type_store, lowered) = lower_test_module(&module, &resolved, &defs);
    let signatures = collect_test_signatures(&module, &defs, &lowered, &type_store);
    assert!(lowered.explicit_type_roots().iter().any(|ty| matches!(
        type_store.get(*ty),
        Some(TyKind::Array {
            len: ArrayLenTy::Infer,
            ..
        })
    )));
    let layouts = compute_layouts(&type_store, &defs, &signatures, TargetDataLayout::LP64)
        .expect("compute layouts");
    assert!(layouts.diagnostics.is_empty(), "{:?}", layouts.diagnostics);
}

#[test]
fn computes_extern_struct_c_field_layout() {
    let module_ids = ModuleIdAllocator::new().expect("create module ID allocator");
    let module_id = module_ids.allocate().expect("allocate module ID");
    let (module, symbols) = parse_test_module(
        r#"
extern struct CPair {
tag: u8,
value: i32,
}
"#,
    );
    let defs = collect_module_defs(module_id, &module);
    let resolved = resolve_module_types_with_symbols(&module, &defs, &symbols);
    let (type_store, lowered) = lower_test_module(&module, &resolved, &defs);
    let signatures = collect_test_signatures(&module, &defs, &lowered, &type_store);
    let cpair_id = defs
        .module_scope
        .types
        .get(&sym("CPair"))
        .expect("CPair def");
    assert!(
        signatures
            .structs
            .get(&cpair_id)
            .expect("CPair signature")
            .is_extern
    );
    let layouts = compute_layouts(&type_store, &defs, &signatures, TargetDataLayout::LP64)
        .expect("compute layouts");
    assert!(layouts.diagnostics.is_empty(), "{:?}", layouts.diagnostics);
    let cpair = layouts.structs.get(&cpair_id).expect("CPair layout");
    assert_eq!(cpair.layout, TypeLayout { size: 8, align: 4 });
    assert_eq!(cpair.fields[0].offset, 0);
    assert_eq!(cpair.fields[1].offset, 4);
}
