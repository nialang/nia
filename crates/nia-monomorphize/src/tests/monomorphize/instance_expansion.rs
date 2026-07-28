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
