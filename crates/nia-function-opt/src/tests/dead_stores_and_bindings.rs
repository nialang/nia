use super::*;

#[test]
fn removes_unused_local_bindings_to_fixed_point() {
    let span = Span::default();
    let ty = test_ty();
    let mut body = test_body(vec![FunctionBlock {
        id: FunctionBlockId(0),
        scope: FunctionScopeId(0),
        span,
        ops: vec![
            FunctionOp::Binding(nia_function_ir::FunctionBinding {
                local_id: LocalId(0),
                name: local_name("a"),
                ty,
                value: Some(FunctionExpr {
                    span,
                    ty,
                    kind: FunctionExprKind::Integer("1".to_string()),
                }),
                is_let: false,
            }),
            FunctionOp::Binding(nia_function_ir::FunctionBinding {
                local_id: LocalId(1),
                name: local_name("b"),
                ty,
                value: Some(FunctionExpr {
                    span,
                    ty,
                    kind: FunctionExprKind::Local(LocalId(0)),
                }),
                is_let: false,
            }),
        ],
        terminator: FunctionTerminator::Return { value: None, span },
    }]);
    body.locals = vec![
        nia_function_ir::FunctionLocal {
            id: LocalId(0),
            name: local_name("a"),
            kind: FunctionLocalKind::MutableBinding,
            ty,
            span,
        },
        nia_function_ir::FunctionLocal {
            id: LocalId(1),
            name: local_name("b"),
            kind: FunctionLocalKind::MutableBinding,
            ty,
            span,
        },
    ];

    assert!(remove_unused_local_bindings(&mut body));

    assert!(body.locals.is_empty());
    assert!(body.blocks[0].ops.is_empty());
    validate_function_body(&body).expect("DCE body should remain valid");
}

#[test]
fn preserves_try_success_locals_as_referenced_bindings() {
    let span = Span::default();
    let ty = test_ty();
    let mut body = test_body(vec![
        FunctionBlock {
            id: FunctionBlockId(0),
            scope: FunctionScopeId(0),
            span,
            ops: vec![FunctionOp::Binding(nia_function_ir::FunctionBinding {
                local_id: LocalId(0),
                name: local_name("ok"),
                ty,
                value: None,
                is_let: false,
            })],
            terminator: FunctionTerminator::Try {
                value: FunctionExpr {
                    span,
                    ty,
                    kind: FunctionExprKind::Local(LocalId(1)),
                },
                kind: FunctionTryKind::ErrorUnion,
                error_conversion: None,
                success_local: LocalId(0),
                success_target: FunctionBlockId(1),
                span,
            },
        },
        FunctionBlock {
            id: FunctionBlockId(1),
            scope: FunctionScopeId(0),
            span,
            ops: Vec::new(),
            terminator: FunctionTerminator::Return { value: None, span },
        },
    ]);
    body.locals = vec![
        nia_function_ir::FunctionLocal {
            id: LocalId(0),
            name: local_name("ok"),
            kind: FunctionLocalKind::MutableBinding,
            ty,
            span,
        },
        nia_function_ir::FunctionLocal {
            id: LocalId(1),
            name: local_name("result"),
            kind: FunctionLocalKind::Param,
            ty,
            span,
        },
    ];

    assert!(!remove_unused_local_bindings(&mut body));

    assert!(body.locals.iter().any(|local| local.id == LocalId(0)));
    assert!(matches!(
        body.blocks[0].terminator,
        FunctionTerminator::Try {
            success_local: LocalId(0),
            ..
        }
    ));
    validate_function_body(&body).expect("try success local should remain valid");
}

#[test]
fn preserves_effects_from_unused_local_binding_initializer() {
    let span = Span::default();
    let ty = test_ty();
    let mut body = test_body(vec![FunctionBlock {
        id: FunctionBlockId(0),
        scope: FunctionScopeId(0),
        span,
        ops: vec![FunctionOp::Binding(nia_function_ir::FunctionBinding {
            local_id: LocalId(0),
            name: local_name("unused"),
            ty,
            value: Some(FunctionExpr {
                span,
                ty,
                kind: FunctionExprKind::Call {
                    callee: FunctionCallee::Function(nia_ids::GlobalDefId {
                        module_id: test_module_id(),
                        def_id: nia_ids::DefId(0),
                    }),
                    args: Vec::new(),
                },
            }),
            is_let: false,
        })],
        terminator: FunctionTerminator::Return { value: None, span },
    }]);
    body.locals = vec![nia_function_ir::FunctionLocal {
        id: LocalId(0),
        name: local_name("unused"),
        kind: FunctionLocalKind::MutableBinding,
        ty,
        span,
    }];

    assert!(remove_unused_local_bindings(&mut body));

    assert!(body.locals.is_empty());
    assert!(matches!(
        body.blocks[0].ops.as_slice(),
        [FunctionOp::Expr(FunctionExpr {
            kind: FunctionExprKind::Call { .. },
            ..
        })]
    ));
    validate_function_body(&body).expect("effect-preserving DCE body should remain valid");
}

#[test]
fn removes_never_read_local_store_with_pure_value() {
    let span = Span::default();
    let ty = test_ty();
    let mut body = test_body(vec![FunctionBlock {
        id: FunctionBlockId(0),
        scope: FunctionScopeId(0),
        span,
        ops: vec![FunctionOp::StoreLocal {
            local_id: LocalId(0),
            value: FunctionExpr {
                span,
                ty,
                kind: FunctionExprKind::Integer("1".to_string()),
            },
            span,
        }],
        terminator: FunctionTerminator::Return { value: None, span },
    }]);
    body.locals = vec![nia_function_ir::FunctionLocal {
        id: LocalId(0),
        name: local_name("unused"),
        kind: FunctionLocalKind::MutableBinding,
        ty,
        span,
    }];

    assert!(remove_never_read_local_stores(&mut body));

    assert!(body.blocks[0].ops.is_empty());
    validate_function_body(&body).expect("dead-store body should remain valid");
}

#[test]
fn preserves_effects_from_never_read_local_store_value() {
    let span = Span::default();
    let ty = test_ty();
    let mut body = test_body(vec![FunctionBlock {
        id: FunctionBlockId(0),
        scope: FunctionScopeId(0),
        span,
        ops: vec![FunctionOp::StoreLocal {
            local_id: LocalId(0),
            value: FunctionExpr {
                span,
                ty,
                kind: FunctionExprKind::Call {
                    callee: FunctionCallee::Function(nia_ids::GlobalDefId {
                        module_id: test_module_id(),
                        def_id: nia_ids::DefId(0),
                    }),
                    args: Vec::new(),
                },
            },
            span,
        }],
        terminator: FunctionTerminator::Return { value: None, span },
    }]);
    body.locals = vec![nia_function_ir::FunctionLocal {
        id: LocalId(0),
        name: local_name("unused"),
        kind: FunctionLocalKind::MutableBinding,
        ty,
        span,
    }];

    assert!(remove_never_read_local_stores(&mut body));

    assert!(matches!(
        body.blocks[0].ops.as_slice(),
        [FunctionOp::Expr(FunctionExpr {
            kind: FunctionExprKind::Call { .. },
            ..
        })]
    ));
    validate_function_body(&body).expect("effect-preserving dead-store body should remain valid");
}

#[test]
fn preserves_stores_to_read_locals() {
    let span = Span::default();
    let ty = test_ty();
    let mut body = test_body(vec![FunctionBlock {
        id: FunctionBlockId(0),
        scope: FunctionScopeId(0),
        span,
        ops: vec![FunctionOp::StoreLocal {
            local_id: LocalId(0),
            value: FunctionExpr {
                span,
                ty,
                kind: FunctionExprKind::Integer("1".to_string()),
            },
            span,
        }],
        terminator: FunctionTerminator::Tail {
            value: Some(FunctionExpr {
                span,
                ty,
                kind: FunctionExprKind::Local(LocalId(0)),
            }),
            span,
        },
    }]);
    body.locals = vec![nia_function_ir::FunctionLocal {
        id: LocalId(0),
        name: local_name("used"),
        kind: FunctionLocalKind::MutableBinding,
        ty,
        span,
    }];

    assert!(!remove_never_read_local_stores(&mut body));

    assert!(matches!(
        body.blocks[0].ops.as_slice(),
        [FunctionOp::StoreLocal {
            local_id: LocalId(0),
            ..
        }]
    ));
    validate_function_body(&body).expect("preserved-store body should remain valid");
}

#[test]
fn removes_local_store_overwritten_before_read() {
    let span = Span::default();
    let ty = test_ty();
    let mut body = test_body(vec![FunctionBlock {
        id: FunctionBlockId(0),
        scope: FunctionScopeId(0),
        span,
        ops: vec![
            FunctionOp::StoreLocal {
                local_id: LocalId(0),
                value: FunctionExpr {
                    span,
                    ty,
                    kind: FunctionExprKind::Integer("1".to_string()),
                },
                span,
            },
            FunctionOp::StoreLocal {
                local_id: LocalId(0),
                value: FunctionExpr {
                    span,
                    ty,
                    kind: FunctionExprKind::Integer("2".to_string()),
                },
                span,
            },
        ],
        terminator: FunctionTerminator::Tail {
            value: Some(FunctionExpr {
                span,
                ty,
                kind: FunctionExprKind::Local(LocalId(0)),
            }),
            span,
        },
    }]);
    body.locals = vec![nia_function_ir::FunctionLocal {
        id: LocalId(0),
        name: local_name("target"),
        kind: FunctionLocalKind::MutableBinding,
        ty,
        span,
    }];

    assert!(remove_overwritten_local_stores(&mut body.blocks));

    assert!(matches!(
        body.blocks[0].ops.as_slice(),
        [FunctionOp::StoreLocal {
            local_id: LocalId(0),
            value: FunctionExpr {
                kind: FunctionExprKind::Integer(value),
                ..
            },
            ..
        }] if value == "2"
    ));
    validate_function_body(&body).expect("overwritten-store body should remain valid");
}

#[test]
fn preserves_effects_from_overwritten_local_store_value() {
    let span = Span::default();
    let ty = test_ty();
    let mut body = test_body(vec![FunctionBlock {
        id: FunctionBlockId(0),
        scope: FunctionScopeId(0),
        span,
        ops: vec![
            FunctionOp::StoreLocal {
                local_id: LocalId(0),
                value: FunctionExpr {
                    span,
                    ty,
                    kind: FunctionExprKind::Call {
                        callee: FunctionCallee::Function(nia_ids::GlobalDefId {
                            module_id: test_module_id(),
                            def_id: nia_ids::DefId(0),
                        }),
                        args: Vec::new(),
                    },
                },
                span,
            },
            FunctionOp::StoreLocal {
                local_id: LocalId(0),
                value: FunctionExpr {
                    span,
                    ty,
                    kind: FunctionExprKind::Integer("2".to_string()),
                },
                span,
            },
        ],
        terminator: FunctionTerminator::Tail {
            value: Some(FunctionExpr {
                span,
                ty,
                kind: FunctionExprKind::Local(LocalId(0)),
            }),
            span,
        },
    }]);
    body.locals = vec![nia_function_ir::FunctionLocal {
        id: LocalId(0),
        name: local_name("target"),
        kind: FunctionLocalKind::MutableBinding,
        ty,
        span,
    }];

    assert!(remove_overwritten_local_stores(&mut body.blocks));

    assert!(matches!(
        body.blocks[0].ops.as_slice(),
        [
            FunctionOp::Expr(FunctionExpr {
                kind: FunctionExprKind::Call { .. },
                ..
            }),
            FunctionOp::StoreLocal {
                local_id: LocalId(0),
                ..
            },
        ]
    ));
    validate_function_body(&body)
        .expect("effect-preserving overwritten-store body should remain valid");
}

#[test]
fn preserves_local_store_read_before_overwrite() {
    let span = Span::default();
    let ty = test_ty();
    let mut body = test_body(vec![FunctionBlock {
        id: FunctionBlockId(0),
        scope: FunctionScopeId(0),
        span,
        ops: vec![
            FunctionOp::StoreLocal {
                local_id: LocalId(0),
                value: FunctionExpr {
                    span,
                    ty,
                    kind: FunctionExprKind::Integer("1".to_string()),
                },
                span,
            },
            FunctionOp::Expr(FunctionExpr {
                span,
                ty,
                kind: FunctionExprKind::Local(LocalId(0)),
            }),
            FunctionOp::StoreLocal {
                local_id: LocalId(0),
                value: FunctionExpr {
                    span,
                    ty,
                    kind: FunctionExprKind::Integer("2".to_string()),
                },
                span,
            },
        ],
        terminator: FunctionTerminator::Tail {
            value: Some(FunctionExpr {
                span,
                ty,
                kind: FunctionExprKind::Local(LocalId(0)),
            }),
            span,
        },
    }]);
    body.locals = vec![nia_function_ir::FunctionLocal {
        id: LocalId(0),
        name: local_name("target"),
        kind: FunctionLocalKind::MutableBinding,
        ty,
        span,
    }];

    assert!(!remove_overwritten_local_stores(&mut body.blocks));

    assert_eq!(
        body.blocks[0]
            .ops
            .iter()
            .filter(|op| matches!(op, FunctionOp::StoreLocal { .. }))
            .count(),
        2
    );
    validate_function_body(&body).expect("read-before-overwrite body should remain valid");
}
