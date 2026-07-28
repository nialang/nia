use super::*;

#[test]
fn computes_layout_builtin_array_lengths() {
    let mut module_ids = ModuleIdAllocator::new();
    let module_id = module_ids.allocate();
    let (module, symbols) = parse_test_module(
        r#"
struct Pair {
a: u8,
b: i32,
}

fn main(xs: [std::builtin::size[Pair]()]u8, ys: [std::builtin::align[Pair]()]u8) {}
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
    assert!(root_types.iter().copied().any(|ty_id| {
        matches!(
            type_store.get(ty_id),
            Some(TyKind::Array {
                len: ArrayLenTy::Builtin { builtin, .. },
                ..
            }) if *builtin == LayoutBuiltin::Size
        ) && layouts.types.get(&ty_id) == Some(&TypeLayout { size: 8, align: 1 })
    }));
    assert!(root_types.iter().copied().any(|ty_id| {
        matches!(
            type_store.get(ty_id),
            Some(TyKind::Array {
                len: ArrayLenTy::Builtin { builtin, .. },
                ..
            }) if *builtin == LayoutBuiltin::Align
        ) && layouts.types.get(&ty_id) == Some(&TypeLayout { size: 4, align: 1 })
    }));
}

#[test]
fn substitutes_const_generic_array_lengths_in_struct_layouts() {
    let mut module_ids = ModuleIdAllocator::new();
    let module_id = module_ids.allocate();
    let (module, symbols) = parse_test_module(
        r#"
struct Buffer[T, N: usize] {
data: [N]T,
}

fn main(buf: Buffer[u8, 4]) {}
"#,
    );
    let defs = collect_module_defs(module_id, &module);
    let resolved = resolve_module_types_with_symbols(&module, &defs, &symbols);
    assert!(
        resolved.diagnostics.is_empty(),
        "{:?}",
        resolved.diagnostics
    );
    let (type_store, lowered) = lower_test_module(&module, &resolved, &defs);
    assert!(lowered.diagnostics.is_empty(), "{:?}", lowered.diagnostics);
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
    assert!(layouts.struct_instances.iter().any(|(key, layout)| {
        key.const_args
            .iter()
            .any(|arg| matches!(&arg.value, ConstGenericValue::Int(value) if value.bits() == 4))
            && layout.layout == TypeLayout { size: 4, align: 1 }
    }));
}
