use super::*;

#[test]
fn recursive_generic_body_reuses_same_concrete_instance() {
    let (module, errors) = parse_module("fn recurse[T](value: T) T { recurse[T](value) }");
    assert!(errors.is_empty(), "{errors:?}");
    let fixture = test_fixture();
    let defs = collect_module_defs(fixture.module_id, &module);
    let recurse_id = GlobalDefId {
        module_id: fixture.module_id,
        def_id: value_def(&defs, "recurse"),
    };
    let i32_ty = fixture.types.primitive(PrimitiveTy::I32);
    let generic_t = generic_param(&fixture.types, "T");
    let instantiations = vec![
        inst(
            recurse_id,
            vec![generic_t],
            Span::new(1, 2),
            Some(recurse_id),
        ),
        inst(recurse_id, vec![i32_ty], Span::new(3, 4), None),
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
    assert_eq!(mono.instances[0].def_id, recurse_id);
    assert_eq!(mono.instances[0].args, vec![i32_ty]);
}

#[test]
fn growing_recursive_generic_body_reports_type_depth_limit() {
    let (module, errors) = parse_module("fn grow[T](value: &T) i32 { grow[&T](&value) }");
    assert!(errors.is_empty(), "{errors:?}");
    let fixture = test_fixture();
    let defs = collect_module_defs(fixture.module_id, &module);
    let grow_id = GlobalDefId {
        module_id: fixture.module_id,
        def_id: value_def(&defs, "grow"),
    };
    let i32_ty = fixture.types.primitive(PrimitiveTy::I32);
    let i32_ptr = fixture.types.intern(TyKind::Pointer {
        is_readonly: true,
        elem: i32_ty,
    });
    let generic_t = generic_param(&fixture.types, "T");
    let generic_ptr = fixture.types.intern(TyKind::Pointer {
        is_readonly: true,
        elem: generic_t,
    });
    let instantiations = vec![
        inst(grow_id, vec![generic_ptr], Span::new(10, 20), Some(grow_id)),
        inst(grow_id, vec![i32_ty], Span::new(1, 2), None),
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

    assert!(
        mono.instances
            .iter()
            .any(|instance| { instance.def_id == grow_id && instance.args == vec![i32_ptr] })
    );
    let diagnostic = mono
        .diagnostics
        .iter()
        .find(|diagnostic| diagnostic.summary.contains("type depth limit"))
        .expect("type depth diagnostic");
    assert_eq!(diagnostic.code.as_str(), "E0601");
    assert_eq!(diagnostic.primary_span(), Some(Span::new(10, 20)));
    assert!(
        diagnostic
            .notes
            .iter()
            .any(|note| note.contains("already-seen concrete generic instance"))
    );
    assert!(
        diagnostic
            .help
            .iter()
            .any(|help| help.contains("finite set of concrete type arguments"))
    );
}
