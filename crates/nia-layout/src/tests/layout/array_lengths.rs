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

fn main(xs: [u8; std::builtin::size[Pair]()], ys: [u8; std::builtin::align[Pair]()]) {}
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
data: [T; N],
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

#[test]
fn public_nominal_queries_keep_const_arguments_in_the_cache_key() {
    let mut module_ids = ModuleIdAllocator::new();
    let module_id = module_ids.allocate();
    let (module, symbols) = parse_test_module(
        r#"
struct Packet[N: usize] {
    marker: u8,
    values: [u32; N],
}

fn main(packet: Packet[3]) {}
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
    });
    let packet_id = defs
        .module_scope
        .types
        .get(&sym("Packet"))
        .expect("Packet def");
    let key = layouts
        .struct_instances
        .keys()
        .find(|key| key.def_id == packet_id)
        .expect("Packet instance key");
    let marker = signatures
        .structs
        .get(&packet_id)
        .expect("Packet signature")
        .fields[0]
        .def_id;
    let global_id = GlobalDefId {
        module_id,
        def_id: packet_id,
    };
    assert_eq!(
        layouts.nominal_type_layout_with_const_args(global_id, &key.args, &key.const_args),
        Some(TypeLayout { size: 16, align: 4 })
    );
    assert_eq!(
        layouts.field_offset_with_const_args(
            global_id,
            &key.args,
            &key.const_args,
            GlobalDefId {
                module_id,
                def_id: marker,
            },
        ),
        Some(12)
    );
}

#[test]
fn binds_mixed_type_and_const_aggregate_parameters_in_declaration_order() {
    let mut module_ids = ModuleIdAllocator::new();
    let module_id = module_ids.allocate();
    let (module, symbols) = parse_test_module(
        r#"
struct Mixed[T, N: usize, U] {
values: [T; N],
tail: U,
}

union MixedBits[T, N: usize, U] {
values: [T; N],
tail: U,
}

fn main(value: Mixed[u8, 3, u32], bits: MixedBits[u16, 5, u8]) {}
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

    let mixed_id = defs
        .module_scope
        .types
        .get(&sym("Mixed"))
        .expect("Mixed def");
    let (mixed_key, mixed) = layouts
        .struct_instances
        .iter()
        .find(|(key, _)| key.def_id == mixed_id)
        .expect("Mixed instance layout");
    assert_eq!(mixed.layout, TypeLayout { size: 8, align: 4 });

    let mixed_bits_id = defs
        .module_scope
        .types
        .get(&sym("MixedBits"))
        .expect("MixedBits def");
    let (mixed_bits_key, mixed_bits) = layouts
        .union_instances
        .iter()
        .find(|(key, _)| key.def_id == mixed_bits_id)
        .expect("MixedBits instance layout");
    assert_eq!(mixed_bits.layout, TypeLayout { size: 10, align: 2 });

    let consumer_module_id = module_ids.allocate();
    let (consumer_module, _) = parse_test_module("");
    let consumer_defs = collect_module_defs(consumer_module_id, &consumer_module);
    let cached_layouts = |requested_module_id| {
        (requested_module_id == module_id).then(|| std::sync::Arc::new(layouts.clone()))
    };
    let program_struct = |def_id: GlobalDefId| {
        (def_id.module_id == module_id)
            .then(|| signatures.structs.get(&def_id.def_id).cloned())
            .flatten()
            .map(|signature| nia_item_signatures::ProgramStructSignature { signature })
    };
    let program_union = |def_id: GlobalDefId| {
        (def_id.module_id == module_id)
            .then(|| signatures.unions.get(&def_id.def_id).cloned())
            .flatten()
            .map(|signature| nia_item_signatures::ProgramUnionSignature { signature })
    };
    let program = ProgramLayoutContext {
        layouts: Some(&cached_layouts),
        struct_: Some(&program_struct),
        union: Some(&program_union),
        ..Default::default()
    };
    let input = LayoutComputationInput {
        type_store: &type_store,
        defs: &consumer_defs,
        signatures: &signatures,
        root_types: &[],
        normalized: &HashMap::new(),
        array_lengths: &NoArrayLengthValues,
        target: TargetDataLayout::LP64,
        program,
    };
    let external_mixed = compute_struct_instance_layout_with_program_context(
        &input,
        InstanceLayoutRequest {
            def_id: GlobalDefId {
                module_id,
                def_id: mixed_id,
            },
            args: &mixed_key.args,
            const_args: &mixed_key.const_args,
        },
    )
    .expect("external Mixed instance detail");
    assert_eq!(external_mixed, mixed.clone());

    let external_mixed_bits = compute_union_instance_layout_with_program_context(
        &input,
        InstanceLayoutRequest {
            def_id: GlobalDefId {
                module_id,
                def_id: mixed_bits_id,
            },
            args: &mixed_bits_key.args,
            const_args: &mixed_bits_key.const_args,
        },
    )
    .expect("external MixedBits instance detail");
    assert_eq!(external_mixed_bits, mixed_bits.clone());
}

#[test]
fn substitutes_interleaved_type_and_const_alias_parameters_in_layouts() {
    let mut module_ids = ModuleIdAllocator::new();
    let module_id = module_ids.allocate();
    let (module, symbols) = parse_test_module(
        r#"
type Mixed[T, N: usize, U] = ([T; N], U);
fn main(value: Mixed[u8, 3, u32]) {}
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
    let alias_id = defs
        .module_scope
        .types
        .get(&sym("Mixed"))
        .expect("Mixed alias def");
    assert!(
        root_types.iter().copied().any(|ty| {
            matches!(
                type_store.get(ty),
                Some(TyKind::Nominal {
                    def_id,
                    const_args,
                    ..
                }) if def_id.def_id == alias_id
                    && matches!(
                        &const_args[0].value,
                        ConstGenericValue::Int(value) if value.bits() == 3
                    )
            ) && layouts.types.get(&ty) == Some(&TypeLayout { size: 8, align: 4 })
        }),
        "root types: {:?}; layouts: {:?}",
        root_types
            .iter()
            .map(|ty| (*ty, type_store.get(*ty)))
            .collect::<Vec<_>>(),
        layouts.types
    );
}
