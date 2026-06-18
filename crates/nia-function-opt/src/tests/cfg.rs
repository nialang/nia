use super::*;

#[test]
fn cfg_indexes_blocks_and_structural_predecessors() {
    let span = Span::default();
    let body = test_body(vec![
        FunctionBlock {
            id: FunctionBlockId(0),
            scope: FunctionScopeId(0),
            span,
            ops: Vec::new(),
            terminator: FunctionTerminator::Loop {
                header: nia_function_ir::FunctionForHeader::Infinite,
                body: FunctionBlockId(1),
                continue_target: FunctionBlockId(2),
                break_target: FunctionBlockId(3),
                span,
            },
        },
        FunctionBlock {
            id: FunctionBlockId(1),
            scope: FunctionScopeId(0),
            span,
            ops: Vec::new(),
            terminator: FunctionTerminator::Branch {
                target: FunctionBlockId(2),
                span,
            },
        },
        FunctionBlock {
            id: FunctionBlockId(2),
            scope: FunctionScopeId(0),
            span,
            ops: Vec::new(),
            terminator: FunctionTerminator::Branch {
                target: FunctionBlockId(0),
                span,
            },
        },
        FunctionBlock {
            id: FunctionBlockId(3),
            scope: FunctionScopeId(0),
            span,
            ops: Vec::new(),
            terminator: FunctionTerminator::Return { value: None, span },
        },
    ]);

    let cfg = FunctionCfg::new(&body.blocks);

    assert_eq!(cfg.block(FunctionBlockId(2)), Some(2));
    assert_eq!(
        cfg.predecessors(FunctionBlockId(2)),
        &[FunctionBlockId(0), FunctionBlockId(1)]
    );
    assert_eq!(
        cfg.reachable_from(&body.blocks, body.entry),
        HashSet::from([
            FunctionBlockId(0),
            FunctionBlockId(1),
            FunctionBlockId(2),
            FunctionBlockId(3),
        ])
    );
}

#[test]
fn removes_blocks_unreachable_from_entry() {
    let span = Span::default();
    let mut body = test_body(vec![
        FunctionBlock {
            id: FunctionBlockId(0),
            scope: FunctionScopeId(0),
            span,
            ops: Vec::new(),
            terminator: FunctionTerminator::Next {
                target: FunctionBlockId(1),
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
        FunctionBlock {
            id: FunctionBlockId(2),
            scope: FunctionScopeId(0),
            span,
            ops: Vec::new(),
            terminator: FunctionTerminator::Return { value: None, span },
        },
    ]);

    remove_unreachable_blocks(&mut body);

    assert_eq!(
        body.blocks.iter().map(|block| block.id).collect::<Vec<_>>(),
        vec![FunctionBlockId(0), FunctionBlockId(1)]
    );
    validate_function_body(&body).expect("optimized function body should remain valid");
}

#[test]
fn preserves_blocks_referenced_by_reachable_loop_terminators() {
    let span = Span::default();
    let mut body = test_body(vec![
        FunctionBlock {
            id: FunctionBlockId(0),
            scope: FunctionScopeId(0),
            span,
            ops: Vec::new(),
            terminator: FunctionTerminator::Loop {
                header: nia_function_ir::FunctionForHeader::Infinite,
                body: FunctionBlockId(1),
                continue_target: FunctionBlockId(2),
                break_target: FunctionBlockId(3),
                span,
            },
        },
        FunctionBlock {
            id: FunctionBlockId(1),
            scope: FunctionScopeId(0),
            span,
            ops: Vec::new(),
            terminator: FunctionTerminator::Branch {
                target: FunctionBlockId(2),
                span,
            },
        },
        FunctionBlock {
            id: FunctionBlockId(2),
            scope: FunctionScopeId(0),
            span,
            ops: Vec::new(),
            terminator: FunctionTerminator::Branch {
                target: FunctionBlockId(0),
                span,
            },
        },
        FunctionBlock {
            id: FunctionBlockId(3),
            scope: FunctionScopeId(0),
            span,
            ops: Vec::new(),
            terminator: FunctionTerminator::Return { value: None, span },
        },
        FunctionBlock {
            id: FunctionBlockId(4),
            scope: FunctionScopeId(0),
            span,
            ops: Vec::new(),
            terminator: FunctionTerminator::Return { value: None, span },
        },
    ]);

    remove_unreachable_blocks(&mut body);

    assert_eq!(
        body.blocks.iter().map(|block| block.id).collect::<Vec<_>>(),
        vec![
            FunctionBlockId(0),
            FunctionBlockId(1),
            FunctionBlockId(2),
            FunctionBlockId(3),
        ]
    );
    validate_function_body(&body).expect("optimized loop body should remain valid");
}

#[test]
fn merges_empty_jump_blocks_within_the_same_scope() {
    let span = Span::default();
    let mut body = test_body(vec![
        FunctionBlock {
            id: FunctionBlockId(0),
            scope: FunctionScopeId(0),
            span,
            ops: Vec::new(),
            terminator: FunctionTerminator::Next {
                target: FunctionBlockId(1),
                span,
            },
        },
        FunctionBlock {
            id: FunctionBlockId(1),
            scope: FunctionScopeId(0),
            span,
            ops: Vec::new(),
            terminator: FunctionTerminator::Next {
                target: FunctionBlockId(2),
                span,
            },
        },
        FunctionBlock {
            id: FunctionBlockId(2),
            scope: FunctionScopeId(0),
            span,
            ops: Vec::new(),
            terminator: FunctionTerminator::Return { value: None, span },
        },
    ]);

    merge_empty_jump_blocks(&mut body);
    remove_unreachable_blocks(&mut body);

    assert_eq!(
        body.blocks.iter().map(|block| block.id).collect::<Vec<_>>(),
        vec![FunctionBlockId(0), FunctionBlockId(2)]
    );
    assert_eq!(
        body.blocks[0].terminator.successors(),
        vec![FunctionBlockId(2)]
    );
    validate_function_body(&body).expect("merged function body should remain valid");
}

#[test]
fn folds_constant_bool_if_to_selected_branch() {
    let span = Span::default();
    let ty = test_ty();
    let mut body = test_body(vec![
        FunctionBlock {
            id: FunctionBlockId(0),
            scope: FunctionScopeId(0),
            span,
            ops: Vec::new(),
            terminator: FunctionTerminator::If {
                cond: nia_function_ir::FunctionExpr {
                    span,
                    ty,
                    kind: FunctionExprKind::Bool(false),
                },
                then_target: FunctionBlockId(1),
                else_target: FunctionBlockId(2),
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
        FunctionBlock {
            id: FunctionBlockId(2),
            scope: FunctionScopeId(0),
            span,
            ops: Vec::new(),
            terminator: FunctionTerminator::Return { value: None, span },
        },
    ]);

    fold_constant_bool_branches(&mut body.blocks);
    remove_unreachable_blocks(&mut body);

    assert_eq!(
        body.blocks.iter().map(|block| block.id).collect::<Vec<_>>(),
        vec![FunctionBlockId(0), FunctionBlockId(2)]
    );
    assert_eq!(
        body.blocks[0].terminator.successors(),
        vec![FunctionBlockId(2)]
    );
    validate_function_body(&body).expect("folded function body should remain valid");
}

#[test]
fn folds_constant_bool_branches_inside_defer_bodies() {
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
            blocks: vec![
                FunctionBlock {
                    id: FunctionBlockId(10),
                    scope: FunctionScopeId(0),
                    span,
                    ops: Vec::new(),
                    terminator: FunctionTerminator::If {
                        cond: FunctionExpr {
                            span,
                            ty,
                            kind: FunctionExprKind::Bool(true),
                        },
                        then_target: FunctionBlockId(11),
                        else_target: FunctionBlockId(12),
                        span,
                    },
                },
                FunctionBlock {
                    id: FunctionBlockId(11),
                    scope: FunctionScopeId(0),
                    span,
                    ops: Vec::new(),
                    terminator: FunctionTerminator::Error { span },
                },
                FunctionBlock {
                    id: FunctionBlockId(12),
                    scope: FunctionScopeId(0),
                    span,
                    ops: Vec::new(),
                    terminator: FunctionTerminator::Error { span },
                },
            ],
            entry: FunctionBlockId(10),
        })],
        terminator: FunctionTerminator::Return { value: None, span },
    }]);

    assert!(fold_constant_bool_branches(&mut body.blocks));

    let FunctionOp::Defer(defer_body) = &body.blocks[0].ops[0] else {
        panic!("expected defer op");
    };
    assert!(matches!(
        defer_body.blocks[0].terminator,
        FunctionTerminator::Branch {
            target: FunctionBlockId(11),
            ..
        }
    ));
    validate_function_body(&body).expect("folded defer body should remain valid");
}

#[test]
fn simplifies_same_target_if_with_pure_condition() {
    let span = Span::default();
    let ty = test_ty();
    let mut body = test_body(vec![
        FunctionBlock {
            id: FunctionBlockId(0),
            scope: FunctionScopeId(0),
            span,
            ops: Vec::new(),
            terminator: FunctionTerminator::If {
                cond: nia_function_ir::FunctionExpr {
                    span,
                    ty,
                    kind: FunctionExprKind::Local(LocalId(0)),
                },
                then_target: FunctionBlockId(1),
                else_target: FunctionBlockId(1),
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
    body.locals = vec![nia_function_ir::FunctionLocal {
        id: LocalId(0),
        name: "cond".to_string(),
        kind: FunctionLocalKind::Binding,
        ty,
        span,
    }];

    assert!(simplify_trivial_branches(&mut body.blocks));

    assert_eq!(
        body.blocks[0].terminator.successors(),
        vec![FunctionBlockId(1)]
    );
    assert!(matches!(
        body.blocks[0].terminator,
        FunctionTerminator::Branch {
            target: FunctionBlockId(1),
            ..
        }
    ));
    validate_function_body(&body).expect("trivial-branch body should remain valid");
}

#[test]
fn simplifies_same_target_if_inside_defer_bodies() {
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
            blocks: vec![
                FunctionBlock {
                    id: FunctionBlockId(10),
                    scope: FunctionScopeId(0),
                    span,
                    ops: Vec::new(),
                    terminator: FunctionTerminator::If {
                        cond: FunctionExpr {
                            span,
                            ty,
                            kind: FunctionExprKind::Local(LocalId(0)),
                        },
                        then_target: FunctionBlockId(11),
                        else_target: FunctionBlockId(11),
                        span,
                    },
                },
                FunctionBlock {
                    id: FunctionBlockId(11),
                    scope: FunctionScopeId(0),
                    span,
                    ops: Vec::new(),
                    terminator: FunctionTerminator::Error { span },
                },
            ],
            entry: FunctionBlockId(10),
        })],
        terminator: FunctionTerminator::Return { value: None, span },
    }]);
    body.locals = vec![nia_function_ir::FunctionLocal {
        id: LocalId(0),
        name: "cond".to_string(),
        kind: FunctionLocalKind::Binding,
        ty,
        span,
    }];

    assert!(simplify_trivial_branches(&mut body.blocks));

    let FunctionOp::Defer(defer_body) = &body.blocks[0].ops[0] else {
        panic!("expected defer op");
    };
    assert!(matches!(
        defer_body.blocks[0].terminator,
        FunctionTerminator::Branch {
            target: FunctionBlockId(11),
            ..
        }
    ));
    validate_function_body(&body).expect("defer trivial-branch body should remain valid");
}

#[test]
fn preserves_same_target_if_with_effectful_condition() {
    let span = Span::default();
    let ty = test_ty();
    let mut body = test_body(vec![
        FunctionBlock {
            id: FunctionBlockId(0),
            scope: FunctionScopeId(0),
            span,
            ops: Vec::new(),
            terminator: FunctionTerminator::If {
                cond: nia_function_ir::FunctionExpr {
                    span,
                    ty,
                    kind: FunctionExprKind::Call {
                        callee: FunctionCallee::Function(nia_ids::GlobalDefId {
                            module_id: nia_ids::ModuleId(0),
                            def_id: nia_ids::DefId(0),
                        }),
                        args: Vec::new(),
                    },
                },
                then_target: FunctionBlockId(1),
                else_target: FunctionBlockId(1),
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

    assert!(!simplify_trivial_branches(&mut body.blocks));

    assert!(matches!(
        body.blocks[0].terminator,
        FunctionTerminator::If {
            then_target: FunctionBlockId(1),
            else_target: FunctionBlockId(1),
            ..
        }
    ));
    validate_function_body(&body).expect("effectful-condition body should remain valid");
}

#[test]
fn simplifies_same_target_switch_with_pure_target() {
    let span = Span::default();
    let ty = test_ty();
    let mut body = test_body(vec![
        FunctionBlock {
            id: FunctionBlockId(0),
            scope: FunctionScopeId(0),
            span,
            ops: Vec::new(),
            terminator: FunctionTerminator::Switch {
                target: FunctionExpr {
                    span,
                    ty,
                    kind: FunctionExprKind::Local(LocalId(0)),
                },
                arms: vec![
                    nia_function_ir::FunctionSwitchArm {
                        pattern: FunctionExpr {
                            span,
                            ty,
                            kind: FunctionExprKind::Integer("1".to_string()),
                        },
                        target: FunctionBlockId(1),
                    },
                    nia_function_ir::FunctionSwitchArm {
                        pattern: FunctionExpr {
                            span,
                            ty,
                            kind: FunctionExprKind::Integer("2".to_string()),
                        },
                        target: FunctionBlockId(1),
                    },
                ],
                default: Some(FunctionBlockId(1)),
                fallback: FunctionBlockId(1),
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
    body.locals = vec![nia_function_ir::FunctionLocal {
        id: LocalId(0),
        name: "target".to_string(),
        kind: FunctionLocalKind::Binding,
        ty,
        span,
    }];

    assert!(simplify_same_target_switches(&mut body.blocks));

    assert!(matches!(
        body.blocks[0].terminator,
        FunctionTerminator::Branch {
            target: FunctionBlockId(1),
            ..
        }
    ));
    validate_function_body(&body).expect("same-target switch body should remain valid");
}

#[test]
fn folds_constant_switch_to_matching_arm() {
    let span = Span::default();
    let ty = test_ty();
    let mut body = test_body(vec![
        FunctionBlock {
            id: FunctionBlockId(0),
            scope: FunctionScopeId(0),
            span,
            ops: Vec::new(),
            terminator: FunctionTerminator::Switch {
                target: FunctionExpr {
                    span,
                    ty,
                    kind: FunctionExprKind::Integer("0x2".to_string()),
                },
                arms: vec![
                    nia_function_ir::FunctionSwitchArm {
                        pattern: FunctionExpr {
                            span,
                            ty,
                            kind: FunctionExprKind::Integer("1".to_string()),
                        },
                        target: FunctionBlockId(1),
                    },
                    nia_function_ir::FunctionSwitchArm {
                        pattern: FunctionExpr {
                            span,
                            ty,
                            kind: FunctionExprKind::Integer("2".to_string()),
                        },
                        target: FunctionBlockId(2),
                    },
                ],
                default: Some(FunctionBlockId(3)),
                fallback: FunctionBlockId(4),
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
        FunctionBlock {
            id: FunctionBlockId(2),
            scope: FunctionScopeId(0),
            span,
            ops: Vec::new(),
            terminator: FunctionTerminator::Return { value: None, span },
        },
        FunctionBlock {
            id: FunctionBlockId(3),
            scope: FunctionScopeId(0),
            span,
            ops: Vec::new(),
            terminator: FunctionTerminator::Return { value: None, span },
        },
        FunctionBlock {
            id: FunctionBlockId(4),
            scope: FunctionScopeId(0),
            span,
            ops: Vec::new(),
            terminator: FunctionTerminator::Return { value: None, span },
        },
    ]);

    assert!(fold_constant_switches(&mut body.blocks));

    assert!(matches!(
        body.blocks[0].terminator,
        FunctionTerminator::Branch {
            target: FunctionBlockId(2),
            ..
        }
    ));
    validate_function_body(&body).expect("constant switch body should remain valid");
}

#[test]
fn folds_constant_switch_to_default_or_fallback() {
    let span = Span::default();
    let ty = test_ty();
    let mut with_default = test_body(vec![
        FunctionBlock {
            id: FunctionBlockId(0),
            scope: FunctionScopeId(0),
            span,
            ops: Vec::new(),
            terminator: FunctionTerminator::Switch {
                target: FunctionExpr {
                    span,
                    ty,
                    kind: FunctionExprKind::Bool(false),
                },
                arms: vec![nia_function_ir::FunctionSwitchArm {
                    pattern: FunctionExpr {
                        span,
                        ty,
                        kind: FunctionExprKind::Bool(true),
                    },
                    target: FunctionBlockId(1),
                }],
                default: Some(FunctionBlockId(2)),
                fallback: FunctionBlockId(3),
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
        FunctionBlock {
            id: FunctionBlockId(2),
            scope: FunctionScopeId(0),
            span,
            ops: Vec::new(),
            terminator: FunctionTerminator::Return { value: None, span },
        },
        FunctionBlock {
            id: FunctionBlockId(3),
            scope: FunctionScopeId(0),
            span,
            ops: Vec::new(),
            terminator: FunctionTerminator::Return { value: None, span },
        },
    ]);

    assert!(fold_constant_switches(&mut with_default.blocks));
    assert!(matches!(
        with_default.blocks[0].terminator,
        FunctionTerminator::Branch {
            target: FunctionBlockId(2),
            ..
        }
    ));

    let mut without_default = test_body(vec![
        FunctionBlock {
            id: FunctionBlockId(0),
            scope: FunctionScopeId(0),
            span,
            ops: Vec::new(),
            terminator: FunctionTerminator::Switch {
                target: FunctionExpr {
                    span,
                    ty,
                    kind: FunctionExprKind::Char('b' as u32),
                },
                arms: vec![nia_function_ir::FunctionSwitchArm {
                    pattern: FunctionExpr {
                        span,
                        ty,
                        kind: FunctionExprKind::Char('a' as u32),
                    },
                    target: FunctionBlockId(1),
                }],
                default: None,
                fallback: FunctionBlockId(2),
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
        FunctionBlock {
            id: FunctionBlockId(2),
            scope: FunctionScopeId(0),
            span,
            ops: Vec::new(),
            terminator: FunctionTerminator::Return { value: None, span },
        },
    ]);

    assert!(fold_constant_switches(&mut without_default.blocks));
    assert!(matches!(
        without_default.blocks[0].terminator,
        FunctionTerminator::Branch {
            target: FunctionBlockId(2),
            ..
        }
    ));
}

#[test]
fn preserves_constant_switch_when_any_pattern_is_not_constant() {
    let span = Span::default();
    let ty = test_ty();
    let mut body = test_body(vec![
        FunctionBlock {
            id: FunctionBlockId(0),
            scope: FunctionScopeId(0),
            span,
            ops: Vec::new(),
            terminator: FunctionTerminator::Switch {
                target: FunctionExpr {
                    span,
                    ty,
                    kind: FunctionExprKind::Integer("2".to_string()),
                },
                arms: vec![
                    nia_function_ir::FunctionSwitchArm {
                        pattern: FunctionExpr {
                            span,
                            ty,
                            kind: FunctionExprKind::Integer("1".to_string()),
                        },
                        target: FunctionBlockId(1),
                    },
                    nia_function_ir::FunctionSwitchArm {
                        pattern: FunctionExpr {
                            span,
                            ty,
                            kind: FunctionExprKind::Call {
                                callee: FunctionCallee::Function(nia_ids::GlobalDefId {
                                    module_id: nia_ids::ModuleId(0),
                                    def_id: nia_ids::DefId(0),
                                }),
                                args: Vec::new(),
                            },
                        },
                        target: FunctionBlockId(2),
                    },
                ],
                default: Some(FunctionBlockId(3)),
                fallback: FunctionBlockId(3),
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
        FunctionBlock {
            id: FunctionBlockId(2),
            scope: FunctionScopeId(0),
            span,
            ops: Vec::new(),
            terminator: FunctionTerminator::Return { value: None, span },
        },
        FunctionBlock {
            id: FunctionBlockId(3),
            scope: FunctionScopeId(0),
            span,
            ops: Vec::new(),
            terminator: FunctionTerminator::Return { value: None, span },
        },
    ]);

    assert!(!fold_constant_switches(&mut body.blocks));

    assert!(matches!(
        body.blocks[0].terminator,
        FunctionTerminator::Switch { .. }
    ));
    validate_function_body(&body).expect("unfolded switch body should remain valid");
}

#[test]
fn preserves_same_target_switch_with_effectful_target() {
    let span = Span::default();
    let ty = test_ty();
    let mut body = test_body(vec![
        FunctionBlock {
            id: FunctionBlockId(0),
            scope: FunctionScopeId(0),
            span,
            ops: Vec::new(),
            terminator: FunctionTerminator::Switch {
                target: FunctionExpr {
                    span,
                    ty,
                    kind: FunctionExprKind::Call {
                        callee: FunctionCallee::Function(nia_ids::GlobalDefId {
                            module_id: nia_ids::ModuleId(0),
                            def_id: nia_ids::DefId(0),
                        }),
                        args: Vec::new(),
                    },
                },
                arms: vec![nia_function_ir::FunctionSwitchArm {
                    pattern: FunctionExpr {
                        span,
                        ty,
                        kind: FunctionExprKind::Integer("1".to_string()),
                    },
                    target: FunctionBlockId(1),
                }],
                default: Some(FunctionBlockId(1)),
                fallback: FunctionBlockId(1),
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

    assert!(!simplify_same_target_switches(&mut body.blocks));

    assert!(matches!(
        body.blocks[0].terminator,
        FunctionTerminator::Switch {
            fallback: FunctionBlockId(1),
            ..
        }
    ));
    validate_function_body(&body).expect("effectful-target switch body should remain valid");
}

#[test]
fn preserves_same_target_switch_with_effectful_pattern() {
    let span = Span::default();
    let ty = test_ty();
    let mut body = test_body(vec![
        FunctionBlock {
            id: FunctionBlockId(0),
            scope: FunctionScopeId(0),
            span,
            ops: Vec::new(),
            terminator: FunctionTerminator::Switch {
                target: FunctionExpr {
                    span,
                    ty,
                    kind: FunctionExprKind::Local(LocalId(0)),
                },
                arms: vec![nia_function_ir::FunctionSwitchArm {
                    pattern: FunctionExpr {
                        span,
                        ty,
                        kind: FunctionExprKind::Call {
                            callee: FunctionCallee::Function(nia_ids::GlobalDefId {
                                module_id: nia_ids::ModuleId(0),
                                def_id: nia_ids::DefId(0),
                            }),
                            args: Vec::new(),
                        },
                    },
                    target: FunctionBlockId(1),
                }],
                default: Some(FunctionBlockId(1)),
                fallback: FunctionBlockId(1),
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
    body.locals = vec![nia_function_ir::FunctionLocal {
        id: LocalId(0),
        name: "target".to_string(),
        kind: FunctionLocalKind::Binding,
        ty,
        span,
    }];

    assert!(!simplify_same_target_switches(&mut body.blocks));

    assert!(matches!(
        body.blocks[0].terminator,
        FunctionTerminator::Switch {
            fallback: FunctionBlockId(1),
            ..
        }
    ));
    validate_function_body(&body).expect("effectful-pattern switch body should remain valid");
}

#[test]
fn simplifies_same_target_switches_inside_defer_bodies() {
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
            blocks: vec![
                FunctionBlock {
                    id: FunctionBlockId(10),
                    scope: FunctionScopeId(0),
                    span,
                    ops: Vec::new(),
                    terminator: FunctionTerminator::Switch {
                        target: FunctionExpr {
                            span,
                            ty,
                            kind: FunctionExprKind::Local(LocalId(0)),
                        },
                        arms: vec![nia_function_ir::FunctionSwitchArm {
                            pattern: FunctionExpr {
                                span,
                                ty,
                                kind: FunctionExprKind::Integer("1".to_string()),
                            },
                            target: FunctionBlockId(11),
                        }],
                        default: Some(FunctionBlockId(11)),
                        fallback: FunctionBlockId(11),
                        span,
                    },
                },
                FunctionBlock {
                    id: FunctionBlockId(11),
                    scope: FunctionScopeId(0),
                    span,
                    ops: Vec::new(),
                    terminator: FunctionTerminator::Error { span },
                },
            ],
            entry: FunctionBlockId(10),
        })],
        terminator: FunctionTerminator::Return { value: None, span },
    }]);
    body.locals = vec![nia_function_ir::FunctionLocal {
        id: LocalId(0),
        name: "target".to_string(),
        kind: FunctionLocalKind::Binding,
        ty,
        span,
    }];

    assert!(simplify_same_target_switches(&mut body.blocks));

    let FunctionOp::Defer(defer_body) = &body.blocks[0].ops[0] else {
        panic!("expected defer op");
    };
    assert!(matches!(
        defer_body.blocks[0].terminator,
        FunctionTerminator::Branch {
            target: FunctionBlockId(11),
            ..
        }
    ));
    validate_function_body(&body).expect("defer switch body should remain valid");
}
