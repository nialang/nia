use super::*;

#[test]
fn computes_payload_enum_tag_union_and_field_offsets() {
    let mut module_ids = ModuleIdAllocator::new();
    let module_id = module_ids.allocate();
    let (module, symbols) = parse_test_module(
        r#"
enum Event: u16 {
    Closed,
    Byte(u8),
    Move(i32, i32),
    Resize { width: i64, height: u8 },
}
"#,
    );
    let defs = collect_module_defs(module_id, &module);
    let resolved = resolve_module_types_with_symbols(&module, &defs, &symbols);
    let (type_store, lowered) = lower_test_module(&module, &resolved, &defs);
    assert!(lowered.diagnostics.is_empty(), "{:?}", lowered.diagnostics);
    let signatures = collect_test_signatures(&module, &defs, &lowered, &type_store);
    assert!(
        signatures.diagnostics.is_empty(),
        "{:?}",
        signatures.diagnostics
    );
    let layouts = compute_layouts(&type_store, &defs, &signatures, TargetDataLayout::LP64);
    assert!(layouts.diagnostics.is_empty(), "{:?}", layouts.diagnostics);

    let event_id = defs
        .module_scope
        .types
        .get(&sym("Event"))
        .expect("Event def");
    let layout = layouts.enums.get(&event_id).expect("Event layout");
    assert_eq!(layout.tag, TypeLayout { size: 2, align: 2 });
    assert_eq!(layout.payload_offset, Some(8));
    assert_eq!(layout.layout, TypeLayout { size: 24, align: 8 });
    assert!(layout.variants[0].fields.is_empty());
    assert_eq!(layout.variants[1].payload, TypeLayout { size: 1, align: 1 });
    assert_eq!(layout.variants[2].payload, TypeLayout { size: 8, align: 4 });
    assert_eq!(layout.variants[2].fields[0].offset, 0);
    assert_eq!(layout.variants[2].fields[1].offset, 4);
    assert_eq!(
        layout.variants[3].payload,
        TypeLayout { size: 16, align: 8 }
    );
    assert_eq!(layout.variants[3].fields[0].offset, 0);
    assert_eq!(layout.variants[3].fields[1].offset, 8);
    assert!(
        layout.variants[3]
            .fields
            .iter()
            .all(|field| field.def_id.is_some())
    );
}

#[test]
fn defaults_enum_tag_to_u8_and_keeps_fieldless_layout_scalar() {
    let mut module_ids = ModuleIdAllocator::new();
    let module_id = module_ids.allocate();
    let (module, symbols) = parse_test_module("enum Flag { Off, On }");
    let defs = collect_module_defs(module_id, &module);
    let resolved = resolve_module_types_with_symbols(&module, &defs, &symbols);
    let (type_store, lowered) = lower_test_module(&module, &resolved, &defs);
    let signatures = collect_test_signatures(&module, &defs, &lowered, &type_store);
    let layouts = compute_layouts(&type_store, &defs, &signatures, TargetDataLayout::LP64);
    let flag_id = defs.module_scope.types.get(&sym("Flag")).expect("Flag def");
    let signature = signatures.enums.get(&flag_id).expect("Flag signature");
    assert!(matches!(
        type_store.get(signature.backing_type),
        Some(TyKind::Primitive(PrimitiveTy::U8))
    ));
    let layout = layouts.enums.get(&flag_id).expect("Flag layout");
    assert_eq!(layout.layout, TypeLayout { size: 1, align: 1 });
    assert_eq!(layout.payload_offset, None);
}

#[test]
fn rejects_open_payload_enums() {
    let mut module_ids = ModuleIdAllocator::new();
    let module_id = module_ids.allocate();
    let (module, symbols) = parse_test_module("enum Event { Data(u8), _, }");
    let defs = collect_module_defs(module_id, &module);
    let resolved = resolve_module_types_with_symbols(&module, &defs, &symbols);
    let (type_store, lowered) = lower_test_module(&module, &resolved, &defs);
    let signatures = collect_test_signatures(&module, &defs, &lowered, &type_store);
    assert!(signatures.diagnostics.iter().any(|diagnostic| {
        diagnostic
            .summary
            .contains("payload enum cannot use the open enum marker")
    }));
}
