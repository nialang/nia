use super::*;

#[test]
fn unresolved_array_lengths_in_symbols_are_diagnostic_not_panic() {
    let (module, errors) = parse_module("fn take[T](value: T) T { value }");
    assert!(errors.is_empty(), "{errors:?}");
    let fixture = test_fixture();
    let defs = collect_module_defs(fixture.module_id, &module);
    let take_id = GlobalDefId {
        module_id: fixture.module_id,
        def_id: value_def(&defs, "take"),
    };
    let len_id = GlobalConstExprId {
        module_id: fixture.module_id,
        const_expr_id: ConstExprId(0),
    };
    let elem = fixture.types.primitive(PrimitiveTy::I32);
    let array_ty = fixture.types.intern(TyKind::Array {
        len: ArrayLenTy::ConstExpr(len_id),
        elem,
    });
    let instantiations = vec![inst(take_id, vec![array_ty], Span::new(1, 2), None)];
    let mut const_expr_summaries = HashMap::new();
    const_expr_summaries.insert(
        len_id,
        ConstExprSummary {
            span: Span::new(10, 12),
            literal_array_len: None,
        },
    );

    let normalization = normalization_for();
    let const_eval = ConstCheck::default();
    let mono = collect_test_monomorphizations(
        &[mono_input(
            &defs,
            &normalization,
            &const_eval,
            &const_expr_summaries,
            &instantiations,
        )],
        &fixture.type_store,
    );

    assert_eq!(mono.instances.len(), 1);
    assert!(
        mono.instances[0].symbol.contains("len_unresolved__s")
            && mono.instances[0].symbol.contains("__c0"),
        "{}",
        mono.instances[0].symbol
    );
    assert_eq!(mono.diagnostics.len(), 1);
    assert!(
        mono.diagnostics[0]
            .summary
            .contains("was not evaluated before monomorphization")
    );
    assert_eq!(mono.diagnostics[0].primary_span(), Some(Span::new(10, 12)));
}

#[test]
fn missing_source_identity_in_symbols_is_diagnostic_not_panic() {
    let (module, errors) = parse_module("fn take[T](value: T) T { value }");
    assert!(errors.is_empty(), "{errors:?}");
    let fixture = test_fixture();
    let defs = collect_module_defs(fixture.module_id, &module);
    let take_id = GlobalDefId {
        module_id: fixture.module_id,
        def_id: value_def(&defs, "take"),
    };
    let elem = fixture.types.primitive(PrimitiveTy::I32);
    let instantiations = vec![inst(take_id, vec![elem], Span::new(1, 2), None)];
    let normalization = normalization_for();
    let const_eval = ConstCheck::default();
    let const_exprs = HashMap::new();

    let mono = collect_monomorphizations(
        &[mono_input(
            &defs,
            &normalization,
            &const_eval,
            &const_exprs,
            &instantiations,
        )],
        std::iter::empty(),
        &fixture.type_store,
    );

    assert_eq!(mono.instances.len(), 1);
    assert!(mono.instances[0].symbol.contains("__inst__"));
    assert_eq!(mono.diagnostics.len(), 1);
    assert!(mono.diagnostics[0]
        .summary
        .contains("missing source identity"));
}

#[test]
fn repeated_unresolved_array_length_symbol_reuses_cached_diagnostic() {
    let (module, errors) = parse_module(
        r#"
fn take[T](value: T) T { value }
fn wrap[T](value: T) T { value }
"#,
    );
    assert!(errors.is_empty(), "{errors:?}");
    let fixture = test_fixture();
    let defs = collect_module_defs(fixture.module_id, &module);
    let take_id = GlobalDefId {
        module_id: fixture.module_id,
        def_id: value_def(&defs, "take"),
    };
    let wrap_id = GlobalDefId {
        module_id: fixture.module_id,
        def_id: value_def(&defs, "wrap"),
    };
    let len_id = GlobalConstExprId {
        module_id: fixture.module_id,
        const_expr_id: ConstExprId(0),
    };
    let elem = fixture.types.primitive(PrimitiveTy::I32);
    let array_ty = fixture.types.intern(TyKind::Array {
        len: ArrayLenTy::ConstExpr(len_id),
        elem,
    });
    let instantiations = vec![
        inst(take_id, vec![array_ty], Span::new(1, 2), None),
        inst(wrap_id, vec![array_ty], Span::new(3, 4), None),
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

    assert_eq!(mono.instances.len(), 2);
    assert_eq!(mono.diagnostics.len(), 1);
}
