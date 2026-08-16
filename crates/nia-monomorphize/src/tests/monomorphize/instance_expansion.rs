use super::*;

#[test]
fn deduplicates_generic_instances() {
    let (module, errors) = parse_module("fn id[T](value: T) T { value }");
    assert!(errors.is_empty(), "{errors:?}");
    let fixture = test_fixture();
    let defs = collect_module_defs(fixture.module_id, &module);
    let def_id = value_def(&defs, "id");
    let i32_ty = fixture.types.primitive(PrimitiveTy::I32);
    let instantiations = vec![
        inst(
            GlobalDefId {
                module_id: fixture.module_id,
                def_id,
            },
            vec![i32_ty],
            Span::new(1, 2),
            None,
        ),
        inst(
            GlobalDefId {
                module_id: fixture.module_id,
                def_id,
            },
            vec![i32_ty],
            Span::new(3, 4),
            None,
        ),
    ];

    let normalization = normalization_for();
    let const_eval = ConstCheck::default();
    let const_exprs = HashMap::new();
    let mono = collect_test_monomorphizations(
        &[mono_input(
            &defs,
            &normalization,
            &const_eval,
            &const_exprs,
            &instantiations,
        )],
        &fixture.type_store,
    );

    assert!(mono.diagnostics.is_empty(), "{:?}", mono.diagnostics);
    assert_eq!(mono.instances.len(), 1);
}

#[test]
fn generic_body_instantiations_are_expanded_from_concrete_roots_only() {
    let (module, errors) = parse_module(
        r#"
fn inner[T](value: T) T { value }
fn outer[T](value: T) T { inner[T](value) }
fn main() i32 { outer(1) }
"#,
    );
    assert!(errors.is_empty(), "{errors:?}");
    let fixture = test_fixture();
    let defs = collect_module_defs(fixture.module_id, &module);
    let inner_id = GlobalDefId {
        module_id: fixture.module_id,
        def_id: value_def(&defs, "inner"),
    };
    let outer_id = GlobalDefId {
        module_id: fixture.module_id,
        def_id: value_def(&defs, "outer"),
    };
    let i32_ty = fixture.types.primitive(PrimitiveTy::I32);
    let generic_t = generic_param(&fixture.types, "T");
    let instantiations = vec![
        inst(inner_id, vec![generic_t], Span::new(1, 2), Some(outer_id)),
        inst(outer_id, vec![i32_ty], Span::new(3, 4), None),
    ];

    let normalization = normalization_for();
    let const_eval = ConstCheck::default();
    let const_exprs = HashMap::new();
    let mono = collect_test_monomorphizations(
        &[mono_input(
            &defs,
            &normalization,
            &const_eval,
            &const_exprs,
            &instantiations,
        )],
        &fixture.type_store,
    );

    assert!(mono.diagnostics.is_empty(), "{:?}", mono.diagnostics);
    assert_eq!(mono.instances.len(), 2);
    assert!(
        mono.instances
            .iter()
            .any(|instance| instance.def_id == outer_id && instance.args == vec![i32_ty])
    );
    assert!(
        mono.instances
            .iter()
            .any(|instance| instance.def_id == inner_id && instance.args == vec![i32_ty])
    );
    assert!(
        !mono
            .instances
            .iter()
            .any(|instance| instance.def_id == inner_id && instance.args == vec![generic_t])
    );
}

#[test]
fn nested_generic_body_instantiations_append_to_session_store() {
    let (module, errors) = parse_module(
        r#"
fn inner[T](value: T) T { value }
fn outer[T](value: &T) &T { inner[&T](value) }
fn main() i32 { 0 }
"#,
    );
    assert!(errors.is_empty(), "{errors:?}");
    let fixture = test_fixture();
    let defs = collect_module_defs(fixture.module_id, &module);
    let inner_id = GlobalDefId {
        module_id: fixture.module_id,
        def_id: value_def(&defs, "inner"),
    };
    let outer_id = GlobalDefId {
        module_id: fixture.module_id,
        def_id: value_def(&defs, "outer"),
    };
    let i32_ty = fixture.types.primitive(PrimitiveTy::I32);
    let generic_t = generic_param(&fixture.types, "T");
    let generic_ptr = fixture.types.intern(TyKind::Pointer {
        is_readonly: true,
        elem: generic_t,
    });
    let instantiations = vec![
        inst(inner_id, vec![generic_ptr], Span::new(1, 2), Some(outer_id)),
        inst(outer_id, vec![i32_ty], Span::new(3, 4), None),
    ];

    let normalization = normalization_for();
    let const_eval = ConstCheck::default();
    let const_exprs = HashMap::new();
    let mono = collect_test_monomorphizations(
        &[mono_input(
            &defs,
            &normalization,
            &const_eval,
            &const_exprs,
            &instantiations,
        )],
        &fixture.type_store,
    );
    let i32_ptr = mono
        .instances
        .iter()
        .find_map(|instance| {
            (instance.def_id == inner_id)
                .then(|| instance.args.first().copied())
                .flatten()
        })
        .expect("monomorphized inner instance");

    assert!(mono.diagnostics.is_empty(), "{:?}", mono.diagnostics);
    assert_eq!(
        fixture.type_store.get(i32_ptr),
        Some(&TyKind::Pointer {
            is_readonly: true,
            elem: i32_ty,
        })
    );
    assert!(
        mono.instances
            .iter()
            .any(|instance| instance.def_id == inner_id && instance.args == vec![i32_ptr])
    );
    assert!(
        !mono
            .instances
            .iter()
            .any(|instance| instance.def_id == inner_id && instance.args == vec![generic_ptr])
    );
}

#[test]
fn nested_instances_substitute_const_arguments_and_array_lengths() {
    let (module, errors) = parse_module(
        r#"
fn typed[T]() () {}
fn counted[N: usize]() () {}
fn mixed[T, N: usize]() () {}
fn outer[N: usize]() () {}
"#,
    );
    assert!(errors.is_empty(), "{errors:?}");
    let fixture = test_fixture();
    let defs = collect_module_defs(fixture.module_id, &module);
    let typed_id = GlobalDefId {
        module_id: fixture.module_id,
        def_id: value_def(&defs, "typed"),
    };
    let counted_id = GlobalDefId {
        module_id: fixture.module_id,
        def_id: value_def(&defs, "counted"),
    };
    let mixed_id = GlobalDefId {
        module_id: fixture.module_id,
        def_id: value_def(&defs, "mixed"),
    };
    let outer_id = GlobalDefId {
        module_id: fixture.module_id,
        def_id: value_def(&defs, "outer"),
    };
    let usize_ty = fixture.types.primitive(PrimitiveTy::Usize);
    let i32_ty = fixture.types.primitive(PrimitiveTy::I32);
    let n = sym("N");
    let generic_n = ConstGenericArg {
        ty: usize_ty,
        value: ConstGenericValue::GenericParam(n),
    };
    let concrete_four = ConstGenericArg {
        ty: usize_ty,
        value: ConstGenericValue::Int(nia_ty::IntConst::unsigned(4_u8.into())),
    };
    let generic_array = fixture.types.intern(TyKind::Array {
        len: ArrayLenTy::GenericParam(n),
        elem: i32_ty,
    });
    let instantiations = vec![
        GenericInstantiation {
            def_id: typed_id,
            self_arg: None,
            args: vec![generic_array],
            const_args: Vec::new(),
            generics: vec![sym("T")],
            span: Span::new(1, 2),
            source_def_id: Some(outer_id),
        },
        GenericInstantiation {
            def_id: counted_id,
            self_arg: None,
            args: Vec::new(),
            const_args: vec![generic_n.clone()],
            generics: vec![n],
            span: Span::new(3, 4),
            source_def_id: Some(outer_id),
        },
        GenericInstantiation {
            def_id: mixed_id,
            self_arg: None,
            args: vec![i32_ty],
            const_args: vec![generic_n.clone()],
            generics: vec![sym("T"), n],
            span: Span::new(4, 5),
            source_def_id: Some(outer_id),
        },
        GenericInstantiation {
            def_id: outer_id,
            self_arg: None,
            args: Vec::new(),
            const_args: vec![concrete_four.clone()],
            generics: vec![n],
            span: Span::new(6, 7),
            source_def_id: None,
        },
    ];

    let normalization = normalization_for();
    let const_eval = ConstCheck::default();
    let const_exprs = HashMap::new();
    let mono = collect_test_monomorphizations(
        &[mono_input(
            &defs,
            &normalization,
            &const_eval,
            &const_exprs,
            &instantiations,
        )],
        &fixture.type_store,
    );

    assert!(mono.diagnostics.is_empty(), "{:?}", mono.diagnostics);
    assert!(mono.instances.iter().any(|instance| {
        instance.def_id == counted_id && instance.const_args == vec![concrete_four.clone()]
    }));
    assert!(mono.instances.iter().any(|instance| {
        instance.def_id == mixed_id
            && instance.args == vec![i32_ty]
            && instance.const_args == vec![concrete_four.clone()]
    }));
    let typed_arg = mono
        .instances
        .iter()
        .find(|instance| instance.def_id == typed_id)
        .and_then(|instance| instance.args.first())
        .copied()
        .expect("nested typed instance");
    assert_eq!(
        fixture.type_store.get(typed_arg),
        Some(&TyKind::Array {
            len: ArrayLenTy::ConstValue(4),
            elem: i32_ty,
        })
    );
}
