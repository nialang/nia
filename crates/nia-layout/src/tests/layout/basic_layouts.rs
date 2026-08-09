use super::*;

#[test]
fn computes_primitive_pointer_array_and_struct_layouts() {
    let mut module_ids = ModuleIdAllocator::new();
    let module_id = module_ids.allocate();
    let (module, symbols) = parse_test_module(
        r#"
struct Pair {
a: u8,
b: i32,
}

fn main(p: &Pair, xs: [3]u16) {}
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
    assert_eq!(
        layouts
            .types
            .get(
                &type_store
                    .append_for_module(module_id)
                    .intern(TyKind::Primitive(PrimitiveTy::U8)),
            )
            .expect("u8 layout"),
        &TypeLayout { size: 1, align: 1 }
    );
    assert!(root_types.iter().copied().any(|ty_id| {
        matches!(type_store.get(ty_id), Some(TyKind::Pointer { .. }))
            && layouts.types.get(&ty_id) == Some(&TypeLayout { size: 8, align: 8 })
    }));
    assert!(root_types.iter().copied().any(|ty_id| {
        matches!(type_store.get(ty_id), Some(TyKind::Array { .. }))
            && layouts.types.get(&ty_id) == Some(&TypeLayout { size: 6, align: 2 })
    }));
    let pair_id = defs.module_scope.types.get(&sym("Pair")).expect("Pair def");
    let pair = layouts.structs.get(&pair_id).expect("Pair layout");
    assert_eq!(pair.layout, TypeLayout { size: 8, align: 4 });
    assert_eq!(pair.fields[0].offset, 0);
    assert_eq!(pair.fields[1].offset, 4);
}

#[test]
fn computes_empty_struct_layout() {
    let mut module_ids = ModuleIdAllocator::new();
    let module_id = module_ids.allocate();
    let (module, symbols) = parse_test_module(
        r#"
struct Empty {}

fn main(value: Empty) {}
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
    let empty_id = defs
        .module_scope
        .types
        .get(&sym("Empty"))
        .expect("Empty def");
    let empty = layouts.structs.get(&empty_id).expect("Empty layout");
    assert_eq!(empty.layout, TypeLayout { size: 0, align: 1 });
    assert!(empty.fields.is_empty());
}

#[test]
fn computes_unit_and_ordered_tuple_layouts() {
    let mut module_ids = ModuleIdAllocator::new();
    let module_id = module_ids.allocate();
    let (module, symbols) = parse_test_module(
        r#"
fn main(unit: (), pair: (u8, i32), nested: (u8, (), i64)) {}
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

    let tuple_layouts = root_types
        .iter()
        .filter_map(|ty| match type_store.get(*ty) {
            Some(TyKind::Tuple(elems)) => Some((elems.len(), layouts.types.get(ty).cloned())),
            _ => None,
        })
        .collect::<Vec<_>>();
    assert!(tuple_layouts.contains(&(0, Some(TypeLayout { size: 0, align: 1 }))));
    assert!(tuple_layouts.contains(&(2, Some(TypeLayout { size: 8, align: 4 }))));
    assert!(tuple_layouts.contains(&(3, Some(TypeLayout { size: 16, align: 8 }))));
}
