use super::*;

#[test]
fn computes_separate_generic_struct_instance_layouts() {
    let module_ids = ModuleIdAllocator::new().expect("create module ID allocator");
    let module_id = module_ids.allocate().expect("allocate module ID");
    let (module, symbols) = parse_test_module(
        r#"
struct ArrayBox[T] {
values: [T; 3],
}

fun main(a: ArrayBox[u8], b: ArrayBox[i32]) {}
"#,
    );
    let defs = collect_module_defs(module_id, &module).expect("collect definitions");
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
    })
    .expect("compute layouts");
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
                    .intern(TyKind::Primitive(PrimitiveTy::U8))
                    .expect("intern u8 type"),
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
                    .intern(TyKind::Primitive(PrimitiveTy::I32))
                    .expect("intern i32 type"),
            ],
            const_args: Vec::new(),
        })
        .expect("ArrayBox[i32] layout");
    assert_eq!(u8_layout.layout, TypeLayout { size: 3, align: 1 });
    assert_eq!(i32_layout.layout, TypeLayout { size: 12, align: 4 });
}

#[test]
fn computes_union_layouts() {
    let module_ids = ModuleIdAllocator::new().expect("create module ID allocator");
    let module_id = module_ids.allocate().expect("allocate module ID");
    let (module, symbols) = parse_test_module(
        r#"
union Bits[T] {
byte: u8,
value: T,
}

fun main(a: Bits[i32]) {}
"#,
    );
    let defs = collect_module_defs(module_id, &module).expect("collect definitions");
    let resolved = resolve_module_types_with_symbols(&module, &defs, &symbols);
    let (type_store, lowered) = lower_test_module(&module, &resolved, &defs);
    let signatures = collect_test_signatures(&module, &defs, &lowered, &type_store);
    let layouts = compute_layouts(&type_store, &defs, &signatures, TargetDataLayout::LP64)
        .expect("compute layouts");
    assert!(layouts.diagnostics.is_empty(), "{:?}", layouts.diagnostics);
    let bits_id = defs.module_scope.types.get(&sym("Bits")).expect("Bits def");
    let bits_i32 = layouts
        .union_instances
        .get(&StructLayoutKey {
            def_id: bits_id,
            args: vec![
                type_store
                    .append_for_module(module_id)
                    .intern(TyKind::Primitive(PrimitiveTy::I32))
                    .expect("intern i32 type"),
            ],
            const_args: Vec::new(),
        })
        .expect("Bits[i32] layout");
    assert_eq!(bits_i32.layout, TypeLayout { size: 4, align: 4 });
    assert!(bits_i32.fields.iter().all(|field| field.offset == 0));
}

#[test]
fn sized_where_bounds_materialize_pointer_hidden_nominal_arguments() {
    let module_ids = ModuleIdAllocator::new().expect("create module ID allocator");
    let module_id = module_ids.allocate().expect("allocate module ID");
    let (module, symbols) = parse_test_module(
        r#"
struct StructEntry[T]
where T: Sized
{
    value: T,
}

struct StructHolder[T]
where StructEntry[T]: Sized
{
    value: &StructEntry[T],
}

struct UnionEntry[T]
where T: Sized
{
    value: T,
}

union UnionHolder[T]
where UnionEntry[T]: Sized
{
    value: &UnionEntry[T],
}

fun main(a: StructHolder[i32], b: UnionHolder[i32]) {}
"#,
    );
    let defs = collect_module_defs(module_id, &module).expect("collect definitions");
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
    let roots = signatures.type_roots();
    let layouts = compute_layouts_with_program_context(LayoutComputationInput {
        type_store: &type_store,
        defs: &defs,
        signatures: &signatures,
        root_types: &roots,
        normalized: &HashMap::new(),
        array_lengths: &|id| const_eval.array_lengths.get(&id).copied(),
        target: TargetDataLayout::LP64,
        program: ProgramLayoutContext::default(),
    })
    .expect("compute layouts");
    assert!(layouts.diagnostics.is_empty(), "{:?}", layouts.diagnostics);

    let struct_entry_id = defs
        .module_scope
        .types
        .get(&sym("StructEntry"))
        .expect("StructEntry def");
    let union_entry_id = defs
        .module_scope
        .types
        .get(&sym("UnionEntry"))
        .expect("UnionEntry def");
    let i32_ty = type_store
        .append_for_module(module_id)
        .intern(TyKind::Primitive(PrimitiveTy::I32))
        .expect("intern i32 type");
    let struct_entry_i32 = type_store
        .append_for_module(module_id)
        .intern(TyKind::Nominal {
            def_id: nia_ids::GlobalDefId {
                module_id,
                def_id: struct_entry_id,
            },
            args: vec![i32_ty],
            const_args: Vec::new(),
        })
        .expect("intern StructEntry[i32] type");
    let union_entry_i32 = type_store
        .append_for_module(module_id)
        .intern(TyKind::Nominal {
            def_id: nia_ids::GlobalDefId {
                module_id,
                def_id: union_entry_id,
            },
            args: vec![i32_ty],
            const_args: Vec::new(),
        })
        .expect("intern UnionEntry[i32] type");
    assert_eq!(
        layouts.types.get(&struct_entry_i32),
        Some(&TypeLayout { size: 4, align: 4 }),
        "the Sized predicate must demand the concrete hidden nominal layout"
    );
    assert_eq!(
        layouts.types.get(&union_entry_i32),
        Some(&TypeLayout { size: 4, align: 4 }),
        "the union Sized predicate must demand the concrete hidden nominal layout"
    );
}
