use super::*;

#[test]
fn computes_separate_generic_struct_instance_layouts() {
    let mut module_ids = ModuleIdAllocator::new();
    let module_id = module_ids.allocate();
    let (module, symbols) = parse_test_module(
        r#"
struct ArrayBox[T] {
values: [T; 3],
}

fn main(a: ArrayBox[u8], b: ArrayBox[i32]) {}
"#,
    );
    let defs = collect_module_defs(module_id, &module);
    let resolved = resolve_module_types_with_symbols(&module, &defs, &symbols);
    let (type_store, lowered) = lower_test_module(&module, &resolved, &defs);
    let signatures = collect_test_signatures(&module, &defs, &lowered, &type_store);
    let const_eval = compute_test_const(
        module_id,
        &type_store,
        &module,
        &symbols,
        &defs,
        &signatures,
        &lowered,
    );
    let root_types = signatures.type_roots();
    let layouts = compute_layouts_with_program_context(LayoutComputationInput {
        type_store: &type_store,
        defs: &defs,
        signatures: &signatures,
        root_types: &root_types,
        normalized: &HashMap::new(),
        array_lengths: &|id| const_eval.array_lengths.get(&id).copied(),
        target: TargetDataLayout::LP64,
        program: ProgramLayoutContext::default(),
    });
    assert!(layouts.diagnostics.is_empty(), "{:?}", layouts.diagnostics);
    let array_box_id = defs
        .module_scope
        .types
        .get(&sym("ArrayBox"))
        .expect("ArrayBox def");
    let u8_layout = layouts
        .struct_instances
        .get(&StructLayoutKey {
            def_id: array_box_id,
            args: vec![
                type_store
                    .append_for_module(module_id)
                    .intern(TyKind::Primitive(PrimitiveTy::U8)),
            ],
            const_args: Vec::new(),
        })
        .expect("ArrayBox[u8] layout");
    let i32_layout = layouts
        .struct_instances
        .get(&StructLayoutKey {
            def_id: array_box_id,
            args: vec![
                type_store
                    .append_for_module(module_id)
                    .intern(TyKind::Primitive(PrimitiveTy::I32)),
            ],
            const_args: Vec::new(),
        })
        .expect("ArrayBox[i32] layout");
    assert_eq!(u8_layout.layout, TypeLayout { size: 3, align: 1 });
    assert_eq!(i32_layout.layout, TypeLayout { size: 12, align: 4 });
}

#[test]
fn computes_union_layouts() {
    let mut module_ids = ModuleIdAllocator::new();
    let module_id = module_ids.allocate();
    let (module, symbols) = parse_test_module(
        r#"
union Bits[T] {
byte: u8,
value: T,
}

fn main(a: Bits[i32]) {}
"#,
    );
    let defs = collect_module_defs(module_id, &module);
    let resolved = resolve_module_types_with_symbols(&module, &defs, &symbols);
    let (type_store, lowered) = lower_test_module(&module, &resolved, &defs);
    let signatures = collect_test_signatures(&module, &defs, &lowered, &type_store);
    let layouts = compute_layouts(&type_store, &defs, &signatures, TargetDataLayout::LP64);
    assert!(layouts.diagnostics.is_empty(), "{:?}", layouts.diagnostics);
    let bits_id = defs.module_scope.types.get(&sym("Bits")).expect("Bits def");
    let bits_i32 = layouts
        .union_instances
        .get(&StructLayoutKey {
            def_id: bits_id,
            args: vec![
                type_store
                    .append_for_module(module_id)
                    .intern(TyKind::Primitive(PrimitiveTy::I32)),
            ],
            const_args: Vec::new(),
        })
        .expect("Bits[i32] layout");
    assert_eq!(bits_i32.layout, TypeLayout { size: 4, align: 4 });
    assert!(bits_i32.fields.iter().all(|field| field.offset == 0));
}
