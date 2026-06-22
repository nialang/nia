use crate::{
    ComptimeAssignOp, ComptimeNameResolution, EarlyComptimeAssign, EarlyComptimeAssignTarget,
    EarlyComptimeBlock, EarlyComptimeExpr, EarlyComptimeExprKind, EarlyComptimeFunction,
    EarlyComptimeLowerInputs, EarlyComptimeName, EarlyComptimeParam, EarlyComptimeTypeArg,
    ResolvedComptimeAssignTargetKind, ResolvedComptimeExpr, ResolvedComptimeExprKind,
    ResolvedComptimeFunction, ResolvedComptimeLowerInputs, lower_expr_early,
    lower_expr_early_with_context, lower_expr_resolved_with_context,
};
use nia_ids::{LayoutBuiltin, LocalId};
use nia_node_id::{NodeChildPath, SyntaxKind, VersionedNodeKey};
use nia_sema_ir::{SemanticUseTable, SemanticValueUse};
use nia_source::{SourceId, SourceRevision, SourceVersion};
use nia_span::Span;

fn span() -> Span {
    Span::new(0, 1)
}

fn other_span() -> Span {
    Span::new(2, 3)
}

fn int_expr(value: &str) -> EarlyComptimeExpr {
    EarlyComptimeExpr {
        span: span(),
        kind: EarlyComptimeExprKind::Integer(value.to_string()),
    }
}

fn node_key(kind: SyntaxKind, ordinal: u32) -> VersionedNodeKey {
    VersionedNodeKey::child_path(
        SourceVersion {
            id: SourceId(0),
            revision: SourceRevision::INITIAL,
        },
        kind,
        NodeChildPath::from_steps([ordinal]),
    )
}

fn expr_key(ordinal: u32) -> VersionedNodeKey {
    node_key(SyntaxKind::Expr, ordinal)
}

fn stmt_key(ordinal: u32) -> VersionedNodeKey {
    node_key(SyntaxKind::Stmt, ordinal)
}

fn type_key(ordinal: u32) -> VersionedNodeKey {
    node_key(SyntaxKind::Type, ordinal)
}

fn ast_ident(name: &str) -> nia_ast::Expr {
    nia_ast::Expr {
        span: span(),
        node_key: expr_key(0),
        kind: nia_ast::ExprKind::Ident(name.to_string()),
    }
}

#[test]
fn resolved_expr_rejects_unresolved_names() {
    let expr = EarlyComptimeExpr {
        span: span(),
        kind: EarlyComptimeExprKind::Ident(EarlyComptimeName::unresolved("x".to_string())),
    };

    let err = ResolvedComptimeExpr::new(expr).expect_err("unresolved name must be rejected");
    assert_eq!(err.message, "failed to resolve comptime name");
}

#[test]
fn resolved_expr_rejects_unresolved_assignment_targets() {
    let expr = EarlyComptimeExpr {
        span: span(),
        kind: EarlyComptimeExprKind::Assign(Box::new(EarlyComptimeAssign {
            lhs: EarlyComptimeAssignTarget::Local {
                span: span(),
                name: "x".to_string(),
                local_id: None,
                path: Vec::new(),
            },
            op: ComptimeAssignOp::Assign,
            rhs: int_expr("1"),
        })),
    };

    let err =
        ResolvedComptimeExpr::new(expr).expect_err("unresolved assignment target must be rejected");
    assert_eq!(err.message, "failed to resolve comptime assignment target");
}

#[test]
fn resolved_function_rejects_unresolved_locals() {
    let function = EarlyComptimeFunction {
        span: span(),
        params: vec![EarlyComptimeParam {
            span: span(),
            name: "x".to_string(),
            local_id: None,
            ty: None,
        }],
        body: EarlyComptimeBlock {
            span: span(),
            stmts: Vec::new(),
            tail: None,
        },
    };

    let err = ResolvedComptimeFunction::new(function)
        .expect_err("unresolved function parameter must be rejected");
    assert_eq!(
        err.message,
        "failed to resolve comptime function parameter local"
    );
}

#[test]
fn resolved_expr_rejects_unresolved_type_args() {
    let expr = EarlyComptimeExpr {
        span: span(),
        kind: EarlyComptimeExprKind::LayoutBuiltin {
            builtin: LayoutBuiltin::Size,
            type_arg: EarlyComptimeTypeArg {
                span: span(),
                ty_span: span(),
                ty: None,
            },
        },
    };

    let err = ResolvedComptimeExpr::new(expr).expect_err("unresolved type arg must be rejected");
    assert_eq!(err.message, "failed to resolve comptime type argument");
}

#[test]
fn resolved_lowering_requires_name_resolution() {
    let semantic_uses = SemanticUseTable::default();
    let context = ResolvedComptimeLowerInputs::new(&semantic_uses);
    let err = lower_expr_resolved_with_context(&ast_ident("x"), &context)
        .expect_err("resolved lowering must reject unresolved names");
    assert_eq!(err.message, "failed to resolve comptime name");
}

#[test]
fn early_name_lowering_separates_unresolved_and_resolved_states() {
    let early = lower_expr_early(&ast_ident("x")).expect("early lowering should keep display name");
    let EarlyComptimeExprKind::Ident(name) = early.kind else {
        panic!("identifier should lower to early comptime name");
    };
    assert_eq!(name.display(), "x");
    assert_eq!(name.resolution(), None);

    let ident = ast_ident("x");
    let mut semantic_uses = SemanticUseTable::default();
    semantic_uses
        .node_value_uses
        .insert(ident.node_key.clone(), SemanticValueUse::Local(LocalId(0)));
    let context = EarlyComptimeLowerInputs::default().with_semantic_uses(&semantic_uses);
    let early = lower_expr_early_with_context(&ident, &context)
        .expect("early lowering with semantic inputs should resolve names");
    let EarlyComptimeExprKind::Ident(name) = early.kind else {
        panic!("identifier should lower to early comptime name");
    };
    assert_eq!(name.display(), "x");
    assert_eq!(
        name.resolution(),
        Some(ComptimeNameResolution::Local(LocalId(0)))
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
                name: "x".to_string(),
                pattern_kind: nia_ast::BindingPatternKind::Value,
                pattern_span: span(),
                pattern_node_key: stmt_key(2),
                ty: None,
                value: Some(ast_ident("x")),
                is_let: true,
                is_comptime: true,
            })),
        }],
        tail: None,
    };
    let expr = nia_ast::Expr {
        span: span(),
        node_key: expr_key(1),
        kind: nia_ast::ExprKind::Block(block),
    };
    let mut semantic_uses = SemanticUseTable::default();
    semantic_uses
        .node_value_uses
        .insert(expr_key(0), SemanticValueUse::Local(LocalId(0)));
    let context = ResolvedComptimeLowerInputs::new(&semantic_uses);
    let err = lower_expr_resolved_with_context(&expr, &context)
        .expect_err("resolved lowering must reject unresolved local bindings");
    assert_eq!(err.message, "failed to resolve comptime local binding");
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
                kind: nia_ast::ExprKind::Ident("x".to_string()),
            }),
            op: nia_ast::AssignOp::Assign,
            rhs: Box::new(nia_ast::Expr {
                span: span(),
                node_key: expr_key(4),
                kind: nia_ast::ExprKind::Integer("1".to_string()),
            }),
        },
    };
    let mut semantic_uses = SemanticUseTable::default();
    semantic_uses
        .node_value_uses
        .insert(lhs_key, SemanticValueUse::Local(LocalId(7)));
    let context = ResolvedComptimeLowerInputs::new(&semantic_uses);
    let lowered = lower_expr_resolved_with_context(&expr, &context)
        .expect("assignment target should use local-use facts");

    let ResolvedComptimeExprKind::Assign(assign) = lowered.kind() else {
        panic!("expression should lower to assignment");
    };
    let ResolvedComptimeAssignTargetKind::Local { local_id, .. } = assign.lhs().kind();
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
                        name: "i32".to_string(),
                        args: Vec::new(),
                    }],
                },
            },
        },
    };
    let mut semantic_uses = SemanticUseTable::default();
    semantic_uses
        .node_value_uses
        .insert(expr_key(6), SemanticValueUse::Local(LocalId(0)));
    semantic_uses
        .node_local_defs
        .insert(stmt_key(0), LocalId(0));
    let context = ResolvedComptimeLowerInputs::new(&semantic_uses);
    let err = lower_expr_resolved_with_context(&expr, &context)
        .expect_err("resolved lowering must reject unresolved types");
    assert_eq!(err.message, "failed to resolve comptime type");
}
