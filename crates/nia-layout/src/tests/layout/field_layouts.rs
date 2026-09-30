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
    let defs = collect_module_defs(module_id, &module).expect("collect definitions");
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
    let defs = collect_module_defs(module_id, &module).expect("collect definitions");
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
    let defs = collect_module_defs(module_id, &module).expect("collect definitions");
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
    let defs = collect_module_defs(module_id, &module).expect("collect definitions");
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

#[test]
fn wide_scalars_follow_the_target_data_model() {
    let module_ids = ModuleIdAllocator::new().expect("create module ID allocator");
    let module_id = module_ids.allocate().expect("allocate module ID");
    let (module, symbols) = parse_test_module(
        r#"
extern struct CWide {
tag: u8,
count: i64,
ratio: f64,
pair: [u64; 2],
}
extern union CWord {
tag: u8,
ratio: f64,
}
struct Wide {
tag: u8,
count: i64,
}
"#,
    );
    let defs = collect_module_defs(module_id, &module).expect("collect definitions");
    let resolved = resolve_module_types_with_symbols(&module, &defs, &symbols);
    let (type_store, lowered) = lower_test_module(&module, &resolved, &defs);
    let signatures = collect_test_signatures(&module, &defs, &lowered, &type_store);
    let def = |name| defs.module_scope.types.get(&sym(name)).expect("type def");
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
    let layouts_for = |target| {
        let layouts = compute_layouts_with_program_context(LayoutComputationInput {
            type_store: &type_store,
            defs: &defs,
            signatures: &signatures,
            root_types: &root_types,
            normalized: &HashMap::new(),
            array_lengths: &|id| const_eval.array_lengths.get(&id).copied(),
            target: TargetDataLayout::for_target(
                nia_target::TargetConfig::parse(target).expect("maintained target"),
            ),
            program: ProgramLayoutContext::default(),
        })
        .expect("compute layouts");
        assert!(layouts.diagnostics.is_empty(), "{:?}", layouts.diagnostics);
        layouts
    };
    let offsets = |layout: &StructLayout| {
        layout
            .fields
            .iter()
            .map(|field| field.offset)
            .collect::<Vec<_>>()
    };

    // The i386 System V data model aligns 8-byte scalars to 4, in Nia and C
    // aggregates alike.
    let i686 = layouts_for("x86-unknown-linux");
    let c_wide = &i686.structs[&def("CWide")];
    assert_eq!(c_wide.layout, TypeLayout { size: 36, align: 4 });
    assert_eq!(offsets(c_wide), [0, 4, 12, 20]);
    assert_eq!(
        i686.unions[&def("CWord")].layout,
        TypeLayout { size: 8, align: 4 }
    );
    assert_eq!(
        i686.structs[&def("Wide")].layout,
        TypeLayout { size: 12, align: 4 }
    );

    for target in [
        "x86_64-unknown-linux",
        "x86_64-pc-windows-msvc",
        "aarch64-unknown-linux",
    ] {
        let layouts = layouts_for(target);
        let c_wide = &layouts.structs[&def("CWide")];
        assert_eq!(c_wide.layout, TypeLayout { size: 40, align: 8 }, "{target}");
        assert_eq!(offsets(c_wide), [0, 8, 16, 24], "{target}");
        assert_eq!(
            layouts.unions[&def("CWord")].layout,
            TypeLayout { size: 8, align: 8 },
            "{target}"
        );
        assert_eq!(
            layouts.structs[&def("Wide")].layout,
            TypeLayout { size: 16, align: 8 },
            "{target}"
        );
    }
}
