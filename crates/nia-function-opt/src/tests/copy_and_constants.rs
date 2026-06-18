use super::*;

#[test]
fn propagates_local_copies_within_one_block() {
    let span = Span::default();
    let ty = test_ty();
    let mut body = test_body(vec![FunctionBlock {
        id: FunctionBlockId(0),
        scope: FunctionScopeId(0),
        span,
        ops: vec![
            FunctionOp::Binding(nia_function_ir::FunctionBinding {
                local_id: LocalId(0),
                name: "source".to_string(),
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
                name: "copy".to_string(),
                ty,
                value: Some(FunctionExpr {
                    span,
                    ty,
                    kind: FunctionExprKind::Local(LocalId(0)),
                }),
                is_let: false,
            }),
        ],
        terminator: FunctionTerminator::Tail {
            value: Some(FunctionExpr {
                span,
                ty,
                kind: FunctionExprKind::Local(LocalId(1)),
            }),
            span,
        },
    }]);
    body.locals = vec![
        nia_function_ir::FunctionLocal {
            id: LocalId(0),
            name: "source".to_string(),
            kind: FunctionLocalKind::Binding,
            ty,
            span,
        },
        nia_function_ir::FunctionLocal {
            id: LocalId(1),
            name: "copy".to_string(),
            kind: FunctionLocalKind::Binding,
            ty,
            span,
        },
    ];

    assert!(propagate_local_copies(&mut body));

    let FunctionTerminator::Tail {
        value: Some(value), ..
    } = &body.blocks[0].terminator
    else {
        panic!("expected tail value");
    };
    assert!(matches!(value.kind, FunctionExprKind::Local(LocalId(0))));
    validate_function_body(&body).expect("copy-propagated body should remain valid");
}

#[test]
fn propagates_local_copies_inside_defer_bodies() {
    let span = Span::default();
    let ty = test_ty();
    let mut body = test_body(vec![FunctionBlock {
        id: FunctionBlockId(0),
        scope: FunctionScopeId(0),
        span,
        ops: vec![FunctionOp::Defer(nia_function_ir::FunctionDeferBody {
            span,
            scopes: vec![FunctionScope {
                id: FunctionScopeId(0),
                parent: None,
                span,
            }],
            blocks: vec![FunctionBlock {
                id: FunctionBlockId(10),
                scope: FunctionScopeId(0),
                span,
                ops: vec![
                    FunctionOp::Binding(nia_function_ir::FunctionBinding {
                        local_id: LocalId(0),
                        name: "source".to_string(),
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
                        name: "copy".to_string(),
                        ty,
                        value: Some(FunctionExpr {
                            span,
                            ty,
                            kind: FunctionExprKind::Local(LocalId(0)),
                        }),
                        is_let: false,
                    }),
                ],
                terminator: FunctionTerminator::Tail {
                    value: Some(FunctionExpr {
                        span,
                        ty,
                        kind: FunctionExprKind::Local(LocalId(1)),
                    }),
                    span,
                },
            }],
            entry: FunctionBlockId(10),
        })],
        terminator: FunctionTerminator::Return { value: None, span },
    }]);
    body.locals = vec![
        nia_function_ir::FunctionLocal {
            id: LocalId(0),
            name: "source".to_string(),
            kind: FunctionLocalKind::Binding,
            ty,
            span,
        },
        nia_function_ir::FunctionLocal {
            id: LocalId(1),
            name: "copy".to_string(),
            kind: FunctionLocalKind::Binding,
            ty,
            span,
        },
    ];

    assert!(propagate_local_copies(&mut body));

    let FunctionOp::Defer(defer_body) = &body.blocks[0].ops[0] else {
        panic!("expected defer body");
    };
    let FunctionTerminator::Tail {
        value: Some(value), ..
    } = &defer_body.blocks[0].terminator
    else {
        panic!("expected defer tail value");
    };
    assert!(matches!(value.kind, FunctionExprKind::Local(LocalId(0))));
    validate_function_body(&body).expect("copy-propagated defer body should remain valid");
}

#[test]
fn propagates_local_constants_within_one_block() {
    let span = Span::default();
    let ty = test_ty();
    let mut body = test_body(vec![FunctionBlock {
        id: FunctionBlockId(0),
        scope: FunctionScopeId(0),
        span,
        ops: vec![FunctionOp::Binding(nia_function_ir::FunctionBinding {
            local_id: LocalId(0),
            name: "value".to_string(),
            ty,
            value: Some(FunctionExpr {
                span,
                ty,
                kind: FunctionExprKind::Integer("42".to_string()),
            }),
            is_let: false,
        })],
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
        name: "value".to_string(),
        kind: FunctionLocalKind::Binding,
        ty,
        span,
    }];

    assert!(propagate_local_constants(&mut body));

    let FunctionTerminator::Tail {
        value: Some(value), ..
    } = &body.blocks[0].terminator
    else {
        panic!("expected tail value");
    };
    assert!(matches!(
        &value.kind,
        FunctionExprKind::Integer(value) if value == "42"
    ));
    validate_function_body(&body).expect("constant-propagated body should remain valid");
}

#[test]
fn propagates_local_constants_inside_defer_bodies() {
    let span = Span::default();
    let ty = test_ty();
    let mut body = test_body(vec![FunctionBlock {
        id: FunctionBlockId(0),
        scope: FunctionScopeId(0),
        span,
        ops: vec![FunctionOp::Defer(nia_function_ir::FunctionDeferBody {
            span,
            scopes: vec![FunctionScope {
                id: FunctionScopeId(0),
                parent: None,
                span,
            }],
            blocks: vec![FunctionBlock {
                id: FunctionBlockId(10),
                scope: FunctionScopeId(0),
                span,
                ops: vec![FunctionOp::Binding(nia_function_ir::FunctionBinding {
                    local_id: LocalId(0),
                    name: "value".to_string(),
                    ty,
                    value: Some(FunctionExpr {
                        span,
                        ty,
                        kind: FunctionExprKind::Integer("42".to_string()),
                    }),
                    is_let: false,
                })],
                terminator: FunctionTerminator::Tail {
                    value: Some(FunctionExpr {
                        span,
                        ty,
                        kind: FunctionExprKind::Local(LocalId(0)),
                    }),
                    span,
                },
            }],
            entry: FunctionBlockId(10),
        })],
        terminator: FunctionTerminator::Return { value: None, span },
    }]);
    body.locals = vec![nia_function_ir::FunctionLocal {
        id: LocalId(0),
        name: "value".to_string(),
        kind: FunctionLocalKind::Binding,
        ty,
        span,
    }];

    assert!(propagate_local_constants(&mut body));

    let FunctionOp::Defer(defer_body) = &body.blocks[0].ops[0] else {
        panic!("expected defer body");
    };
    let FunctionTerminator::Tail {
        value: Some(value), ..
    } = &defer_body.blocks[0].terminator
    else {
        panic!("expected defer tail value");
    };
    assert!(matches!(
        &value.kind,
        FunctionExprKind::Integer(value) if value == "42"
    ));
    validate_function_body(&body).expect("constant-propagated defer body should remain valid");
}

#[test]
fn does_not_propagate_constants_for_locals_used_as_places() {
    let span = Span::default();
    let ty = test_ty();
    let mut body = test_body(vec![FunctionBlock {
        id: FunctionBlockId(0),
        scope: FunctionScopeId(0),
        span,
        ops: vec![
            FunctionOp::Binding(nia_function_ir::FunctionBinding {
                local_id: LocalId(0),
                name: "value".to_string(),
                ty,
                value: Some(FunctionExpr {
                    span,
                    ty,
                    kind: FunctionExprKind::Integer("1".to_string()),
                }),
                is_let: false,
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
        name: "value".to_string(),
        kind: FunctionLocalKind::Binding,
        ty,
        span,
    }];

    assert!(!propagate_local_constants(&mut body));

    let FunctionTerminator::Tail {
        value: Some(value), ..
    } = &body.blocks[0].terminator
    else {
        panic!("expected tail value");
    };
    assert!(matches!(value.kind, FunctionExprKind::Local(LocalId(0))));
    validate_function_body(&body).expect("unpropagated body should remain valid");
}

#[test]
fn does_not_propagate_locals_used_as_places() {
    let span = Span::default();
    let ty = test_ty();
    let mut body = test_body(vec![FunctionBlock {
        id: FunctionBlockId(0),
        scope: FunctionScopeId(0),
        span,
        ops: vec![
            FunctionOp::Binding(nia_function_ir::FunctionBinding {
                local_id: LocalId(0),
                name: "source".to_string(),
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
                name: "copy".to_string(),
                ty,
                value: Some(FunctionExpr {
                    span,
                    ty,
                    kind: FunctionExprKind::Local(LocalId(0)),
                }),
                is_let: false,
            }),
            FunctionOp::StoreLocal {
                local_id: LocalId(1),
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
                kind: FunctionExprKind::Local(LocalId(1)),
            }),
            span,
        },
    }]);
    body.locals = vec![
        nia_function_ir::FunctionLocal {
            id: LocalId(0),
            name: "source".to_string(),
            kind: FunctionLocalKind::Binding,
            ty,
            span,
        },
        nia_function_ir::FunctionLocal {
            id: LocalId(1),
            name: "copy".to_string(),
            kind: FunctionLocalKind::Binding,
            ty,
            span,
        },
    ];

    assert!(!propagate_local_copies(&mut body));

    let FunctionTerminator::Tail {
        value: Some(value), ..
    } = &body.blocks[0].terminator
    else {
        panic!("expected tail value");
    };
    assert!(matches!(value.kind, FunctionExprKind::Local(LocalId(1))));
    validate_function_body(&body).expect("unpropagated body should remain valid");
}
