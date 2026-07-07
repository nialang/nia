// SPDX-License-Identifier: GPL-3.0-or-later
use super::*;

#[test]
fn o1_removes_unreachable_backend_function_blocks() {
    let source = r#"
fn main() i32 {
    defer {
    };
    0
}
"#;
    let lowering = lower_source_with_body_mutation_and_optimization(
        source,
        |body| {
            let mut unreachable = body.blocks[0].clone();
            unreachable.id = FunctionBlockId(999);
            unreachable.ops.clear();
            unreachable.terminator = FunctionTerminator::Return {
                value: None,
                span: unreachable.span,
            };
            body.blocks.push(unreachable);
        },
        nia_opt::NiaOptimizationLevel::O1.policy(),
    );
    let main = lowering.program.modules[0]
        .functions
        .iter()
        .find(|function| function.name == sym("main"))
        .expect("main function");
    let body = main.function_body.as_ref().expect("main function body");

    assert!(body.block(FunctionBlockId(999)).is_none());
}

#[test]
fn o0_preserves_unreachable_backend_function_blocks() {
    let source = r#"
fn main() i32 {
    defer {
    };
    0
}
"#;
    let lowering = lower_source_with_body_mutation_and_optimization(
        source,
        |body| {
            let mut unreachable = body.blocks[0].clone();
            unreachable.id = FunctionBlockId(999);
            unreachable.ops.clear();
            unreachable.terminator = FunctionTerminator::Return {
                value: None,
                span: unreachable.span,
            };
            body.blocks.push(unreachable);
        },
        nia_opt::NiaOptimizationLevel::O0.policy(),
    );
    let main = lowering.program.modules[0]
        .functions
        .iter()
        .find(|function| function.name == sym("main"))
        .expect("main function");
    let body = main.function_body.as_ref().expect("main function body");

    assert!(body.block(FunctionBlockId(999)).is_some());
}

#[test]
fn o1_merges_empty_backend_function_jump_blocks() {
    let source = r#"
fn main() i32 {
    0
}
"#;
    let lowering = lower_source_with_body_mutation_and_optimization(
        source,
        |body| {
            let mut original = body.blocks[0].clone();
            let empty_id = FunctionBlockId(998);
            let original_id = FunctionBlockId(999);
            original.id = original_id;
            body.blocks[0].terminator = FunctionTerminator::Next {
                target: empty_id,
                span: body.blocks[0].span,
            };
            body.blocks.push(nia_function_ir::FunctionBlock {
                id: empty_id,
                scope: original.scope,
                span: original.span,
                ops: Vec::new(),
                terminator: FunctionTerminator::Next {
                    target: original_id,
                    span: original.span,
                },
            });
            body.blocks.push(original);
        },
        nia_opt::NiaOptimizationLevel::O1.policy(),
    );
    let main = lowering.program.modules[0]
        .functions
        .iter()
        .find(|function| function.name == sym("main"))
        .expect("main function");
    let body = main.function_body.as_ref().expect("main function body");

    assert!(body.block(FunctionBlockId(998)).is_none());
}

#[test]
fn o0_preserves_empty_backend_function_jump_blocks() {
    let source = r#"
fn main() i32 {
    0
}
"#;
    let lowering = lower_source_with_body_mutation_and_optimization(
        source,
        |body| {
            let mut original = body.blocks[0].clone();
            let empty_id = FunctionBlockId(998);
            let original_id = FunctionBlockId(999);
            original.id = original_id;
            body.blocks[0].terminator = FunctionTerminator::Next {
                target: empty_id,
                span: body.blocks[0].span,
            };
            body.blocks.push(nia_function_ir::FunctionBlock {
                id: empty_id,
                scope: original.scope,
                span: original.span,
                ops: Vec::new(),
                terminator: FunctionTerminator::Next {
                    target: original_id,
                    span: original.span,
                },
            });
            body.blocks.push(original);
        },
        nia_opt::NiaOptimizationLevel::O0.policy(),
    );
    let main = lowering.program.modules[0]
        .functions
        .iter()
        .find(|function| function.name == sym("main"))
        .expect("main function");
    let body = main.function_body.as_ref().expect("main function body");

    assert!(body.block(FunctionBlockId(998)).is_some());
}

#[test]
fn o1_folds_constant_bool_backend_if_branches() {
    let source = r#"
fn main() i32 {
    0
}
"#;
    let lowering = lower_source_with_body_mutation_and_optimization(
        source,
        |body| {
            let mut original = body.blocks[0].clone();
            let then_id = FunctionBlockId(998);
            let else_id = FunctionBlockId(999);
            original.id = then_id;
            body.blocks[0].terminator = FunctionTerminator::If {
                cond: nia_function_ir::FunctionExpr {
                    span: body.blocks[0].span,
                    ty: body.ty,
                    kind: FunctionExprKind::Bool(false),
                },
                then_target: then_id,
                else_target: else_id,
                span: body.blocks[0].span,
            };
            body.blocks.push(original);
            let mut selected = body.blocks[0].clone();
            selected.id = else_id;
            selected.terminator = FunctionTerminator::Return {
                value: None,
                span: selected.span,
            };
            body.blocks.push(selected);
        },
        nia_opt::NiaOptimizationLevel::O1.policy(),
    );
    let main = lowering.program.modules[0]
        .functions
        .iter()
        .find(|function| function.name == sym("main"))
        .expect("main function");
    let body = main.function_body.as_ref().expect("main function body");

    assert!(body.block(FunctionBlockId(998)).is_none());
    assert!(body.block(FunctionBlockId(999)).is_some());
}

#[test]
fn o1_simplifies_constant_logical_backend_if_conditions() {
    let source = r#"
fn main() i32 {
    0
}
"#;
    let lowering = lower_source_with_body_mutation_and_optimization(
        source,
        |body| {
            let mut original = body.blocks[0].clone();
            let then_id = FunctionBlockId(998);
            let else_id = FunctionBlockId(999);
            original.id = then_id;
            body.blocks[0].terminator = FunctionTerminator::If {
                cond: nia_function_ir::FunctionExpr {
                    span: body.blocks[0].span,
                    ty: body.ty,
                    kind: FunctionExprKind::Binary {
                        lhs: Box::new(nia_function_ir::FunctionExpr {
                            span: body.blocks[0].span,
                            ty: body.ty,
                            kind: FunctionExprKind::Bool(true),
                        }),
                        op: nia_ast::BinaryOp::And,
                        rhs: Box::new(nia_function_ir::FunctionExpr {
                            span: body.blocks[0].span,
                            ty: body.ty,
                            kind: FunctionExprKind::Bool(false),
                        }),
                    },
                },
                then_target: then_id,
                else_target: else_id,
                span: body.blocks[0].span,
            };
            body.blocks.push(original);
            let mut selected = body.blocks[0].clone();
            selected.id = else_id;
            selected.terminator = FunctionTerminator::Return {
                value: None,
                span: selected.span,
            };
            body.blocks.push(selected);
        },
        nia_opt::NiaOptimizationLevel::O1.policy(),
    );
    let main = lowering.program.modules[0]
        .functions
        .iter()
        .find(|function| function.name == sym("main"))
        .expect("main function");
    let body = main.function_body.as_ref().expect("main function body");

    assert!(body.block(FunctionBlockId(998)).is_none());
    assert!(body.block(FunctionBlockId(999)).is_some());
    assert!(
        lowering
            .optimization_report
            .changed_passes
            .iter()
            .any(|change| matches!(
                change,
                BackendOptimizationChange::Function {
                    pass: "simplify-constant-logical-exprs",
                    ..
                }
            ))
    );
}

#[test]
fn o0_preserves_constant_bool_backend_if_branches() {
    let source = r#"
fn main() i32 {
    0
}
"#;
    let lowering = lower_source_with_body_mutation_and_optimization(
        source,
        |body| {
            let mut original = body.blocks[0].clone();
            let then_id = FunctionBlockId(998);
            let else_id = FunctionBlockId(999);
            original.id = then_id;
            body.blocks[0].terminator = FunctionTerminator::If {
                cond: nia_function_ir::FunctionExpr {
                    span: body.blocks[0].span,
                    ty: body.ty,
                    kind: FunctionExprKind::Bool(false),
                },
                then_target: then_id,
                else_target: else_id,
                span: body.blocks[0].span,
            };
            body.blocks.push(original);
            let mut selected = body.blocks[0].clone();
            selected.id = else_id;
            selected.terminator = FunctionTerminator::Return {
                value: None,
                span: selected.span,
            };
            body.blocks.push(selected);
        },
        nia_opt::NiaOptimizationLevel::O0.policy(),
    );
    let main = lowering.program.modules[0]
        .functions
        .iter()
        .find(|function| function.name == sym("main"))
        .expect("main function");
    let body = main.function_body.as_ref().expect("main function body");

    assert!(body.block(FunctionBlockId(998)).is_some());
    assert!(body.block(FunctionBlockId(999)).is_some());
}

#[test]
fn o1_removes_same_type_backend_casts() {
    let source = r#"
fn main() i32 {
    0
}
"#;
    let lowering = lower_source_with_body_mutation_and_optimization(
        source,
        |body| {
            let value = first_terminal_value_mut(body);
            let original = value.clone();
            value.kind = FunctionExprKind::Cast {
                expr: Box::new(original),
                ty: value.ty,
            };
        },
        nia_opt::NiaOptimizationLevel::O1.policy(),
    );
    let main = lowering.program.modules[0]
        .functions
        .iter()
        .find(|function| function.name == sym("main"))
        .expect("main function");
    let body = main.function_body.as_ref().expect("main function body");
    let value = first_terminal_value(body);

    assert!(!matches!(value.kind, FunctionExprKind::Cast { .. }));
}

#[test]
fn o0_preserves_same_type_backend_casts() {
    let source = r#"
fn main() i32 {
    0
}
"#;
    let lowering = lower_source_with_body_mutation_and_optimization(
        source,
        |body| {
            let value = first_terminal_value_mut(body);
            let original = value.clone();
            value.kind = FunctionExprKind::Cast {
                expr: Box::new(original),
                ty: value.ty,
            };
        },
        nia_opt::NiaOptimizationLevel::O0.policy(),
    );
    let main = lowering.program.modules[0]
        .functions
        .iter()
        .find(|function| function.name == sym("main"))
        .expect("main function");
    let body = main.function_body.as_ref().expect("main function body");
    let value = first_terminal_value(body);

    assert!(matches!(value.kind, FunctionExprKind::Cast { .. }));
}
