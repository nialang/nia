use super::*;

#[test]
fn resolved_lowering_requires_name_resolution() {
    let semantic_uses = SemanticUseTable::default();
    let context = ResolvedConstLowerInputs::new(&semantic_uses);
    let err = lower_expr_resolved_with_context(&ast_ident("x"), &context)
        .expect_err("resolved lowering must reject unresolved names");
    assert_eq!(err.message, "failed to resolve const name");
}

#[test]
fn early_name_lowering_separates_unresolved_and_resolved_states() {
    let early = lower_expr_early(&ast_ident("x")).expect("early lowering should keep display name");
    let EarlyConstExprKind::Ident(name) = early.kind else {
        panic!("identifier should lower to early const name");
    };
    assert_eq!(name.display(), sym("x"));
    assert_eq!(name.resolution(), None);

    let ident = ast_ident("x");
    let mut semantic_uses = SemanticUseTable::builder();
    semantic_uses.insert_node_local_value_use(ident.node_key.clone(), LocalId(0));
    let semantic_uses = semantic_uses.finish();
    let context = EarlyConstLowerInputs::default().with_semantic_uses(&semantic_uses);
    let early = lower_expr_early_with_context(&ident, &context)
        .expect("early lowering with semantic inputs should resolve names");
    let EarlyConstExprKind::Ident(name) = early.kind else {
        panic!("identifier should lower to early const name");
    };
    assert_eq!(name.display(), sym("x"));
    assert_eq!(
        name.resolution(),
        Some(ConstNameResolution::Local(LocalId(0)))
    );
}

#[test]
fn resolved_lowering_requires_local_ids() {
    let block = nia_ast::Block {
        span: span(),
        stmts: vec![nia_ast::Stmt {
            span: span(),
            attributes: Vec::new(),
            node_key: stmt_key(0),
            kind: nia_ast::StmtKind::Binding(Box::new(nia_ast::BindingStmt {
                pattern: nia_ast::Pattern {
                    span: span(),
                    kind: nia_ast::PatternKind::Bind {
                        name: sym("x"),
                        node_key: stmt_key(2),
                        is_mutable: false,
                    },
                },
                ty: None,
                value: Some(ast_ident("x")),
                kind: nia_ast::LocalBindingKind::Const,
            })),
        }],
        tail: None,
    };
    let expr = nia_ast::Expr {
        span: span(),
        node_key: expr_key(1),
        kind: nia_ast::ExprKind::Block(block),
    };
    let mut semantic_uses = SemanticUseTable::builder();
    semantic_uses.insert_node_local_value_use(expr_key(0), LocalId(0));
    let semantic_uses = semantic_uses.finish();
    let context = ResolvedConstLowerInputs::new(&semantic_uses);
    let err = lower_expr_resolved_with_context(&expr, &context)
        .expect_err("resolved lowering must reject unresolved local bindings");
    assert_eq!(err.message, "failed to resolve const local binding");
}

#[test]
fn resolved_lowering_uses_local_uses_for_assignment_targets() {
    let assign_span = other_span();
    let lhs_key = expr_key(2);
    let expr = nia_ast::Expr {
        span: Span::new(0, 3),
        node_key: expr_key(3),
        kind: nia_ast::ExprKind::Assign {
            lhs: Box::new(nia_ast::Expr {
                span: assign_span,
                node_key: lhs_key.clone(),
                kind: nia_ast::ExprKind::Ident(sym("x")),
            }),
            op: nia_ast::AssignOp::Assign,
            rhs: Box::new(nia_ast::Expr {
                span: span(),
                node_key: expr_key(4),
                kind: nia_ast::ExprKind::Integer("1".to_string()),
            }),
        },
    };
    let mut semantic_uses = SemanticUseTable::builder();
    semantic_uses.insert_node_local_value_use(lhs_key, LocalId(7));
    let semantic_uses = semantic_uses.finish();
    let context = ResolvedConstLowerInputs::new(&semantic_uses);
    let lowered = lower_expr_resolved_with_context(&expr, &context)
        .expect("assignment target should use local-use facts");

    let ResolvedConstExprKind::Assign(assign) = lowered.kind() else {
        panic!("expression should lower to assignment");
    };
    let ResolvedConstAssignTargetKind::Local { local_id, .. } = assign.lhs().kind();
    assert_eq!(*local_id, LocalId(7));
}

#[test]
fn resolved_lowering_requires_type_ids() {
    let expr = nia_ast::Expr {
        span: span(),
        node_key: expr_key(5),
        kind: nia_ast::ExprKind::Cast {
            expr: Box::new(nia_ast::Expr {
                span: span(),
                node_key: expr_key(6),
                kind: nia_ast::ExprKind::Integer("1".to_string()),
            }),
            ty: nia_ast::TypeRef {
                span: span(),
                node_key: type_key(0),
                text: "i32".to_string(),
                kind: nia_ast::TypeKind::Path {
                    segments: vec![nia_ast::TypePathSegment {
                        kind: nia_ast::PathSegmentKind::Name(sym("i32")),
                        args: Vec::new(),
                    }],
                },
            },
        },
    };
    let mut semantic_uses = SemanticUseTable::builder();
    semantic_uses.insert_node_local_value_use(expr_key(6), LocalId(0));
    semantic_uses.insert_node_local_def(stmt_key(0), LocalId(0));
    let semantic_uses = semantic_uses.finish();
    let context = ResolvedConstLowerInputs::new(&semantic_uses);
    let err = lower_expr_resolved_with_context(&expr, &context)
        .expect_err("resolved lowering must reject unresolved types");
    assert_eq!(err.message, "failed to resolve const type");
}

#[test]
fn generic_call_lowering_uses_semantic_facts_to_distinguish_type_and_const_args() {
    let generic_name = sym("N");
    let function_key = expr_key(10);
    let arg_expr_key = expr_key(11);
    let arg_type_key = type_key(10);
    let call = nia_ast::Expr {
        span: Span::new(0, 8),
        node_key: expr_key(12),
        kind: nia_ast::ExprKind::Call {
            callee: Box::new(nia_ast::Expr {
                span: Span::new(0, 6),
                node_key: expr_key(13),
                kind: nia_ast::ExprKind::BracketSuffix {
                    callee: Box::new(nia_ast::Expr {
                        span: span(),
                        node_key: function_key.clone(),
                        kind: nia_ast::ExprKind::Ident(sym("select")),
                    }),
                    args: vec![nia_ast::BracketArg {
                        span: other_span(),
                        expr: Some(nia_ast::Expr {
                            span: other_span(),
                            node_key: arg_expr_key.clone(),
                            kind: nia_ast::ExprKind::Ident(generic_name),
                        }),
                        ty: Some(nia_ast::TypeRef {
                            span: other_span(),
                            node_key: arg_type_key,
                            text: "N".to_string(),
                            kind: nia_ast::TypeKind::Path {
                                segments: vec![nia_ast::TypePathSegment {
                                    kind: nia_ast::PathSegmentKind::Name(generic_name),
                                    args: Vec::new(),
                                }],
                            },
                        }),
                    }],
                },
            }),
            args: Vec::new(),
        },
    };

    let early = lower_expr_early(&call)
        .expect("early lowering without semantic facts should retain type candidates");
    let EarlyConstExprKind::Call { generic_args, .. } = early.kind else {
        panic!("expression should lower to a call");
    };
    assert!(matches!(
        generic_args.as_slice(),
        [EarlyConstGenericArg::Type(_)]
    ));

    let module_id = ModuleIdAllocator::new().allocate();
    let mut semantic_uses = SemanticUseTable::builder();
    semantic_uses.insert_node_global_value_use(
        function_key,
        GlobalDefId {
            module_id,
            def_id: DefId(0),
        },
    );
    semantic_uses.insert_node_const_generic_use(arg_expr_key, generic_name);
    let semantic_uses = semantic_uses.finish();
    let context = ResolvedConstLowerInputs::new(&semantic_uses);
    let resolved = lower_expr_resolved_with_context(&call, &context)
        .expect("resolved lowering should use const-generic value facts");
    let ResolvedConstExprKind::Call { generic_args, .. } = resolved.kind() else {
        panic!("expression should lower to a resolved call");
    };
    assert!(matches!(
        generic_args.as_slice(),
        [ResolvedConstGenericArg::Const(expr)]
            if expr.name_resolution() == Some(ConstNameResolution::GenericParam(generic_name))
    ));
}
