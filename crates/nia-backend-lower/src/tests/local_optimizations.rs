// SPDX-License-Identifier: GPL-3.0-or-later
use super::*;

#[test]
fn o1_removes_noop_backend_local_stores() {
    let source = r#"
fn main() i32 {
    let mut value = 0;
    value
}
"#;
    let lowering = lower_source_with_body_mutation_and_optimization(
        source,
        |body| {
            let span = body.blocks[0].span;
            let local = body
                .locals
                .iter()
                .find(|local| local.kind == nia_function_ir::FunctionLocalKind::MutableBinding)
                .expect("binding local");
            body.blocks[0].ops.push(FunctionOp::StoreLocal {
                local_id: local.id,
                value: nia_function_ir::FunctionExpr {
                    span,
                    ty: local.ty,
                    kind: FunctionExprKind::Local(local.id),
                },
                span,
            });
        },
        nia_opt::NiaOptimizationLevel::O1.policy(),
    );
    let main = lowering.program.modules[0]
        .functions
        .iter()
        .find(|function| function.name == sym("main"))
        .expect("main function");
    let body = main.function_body.as_ref().expect("main function body");

    assert!(body.blocks.iter().all(|block| {
        block
            .ops
            .iter()
            .all(|op| !matches!(op, FunctionOp::StoreLocal { .. }))
    }));
    assert!(
        lowering
            .optimization_report
            .changed_passes
            .iter()
            .any(|change| matches!(
                change,
                BackendOptimizationChange::Function {
                    pass: "remove-noop-local-stores",
                    is_instance: false,
                    type_arg_count: 0,
                    ..
                }
            ))
    );
}

#[test]
fn o0_preserves_noop_backend_local_stores() {
    let source = r#"
fn main() i32 {
    let mut value = 0;
    value
}
"#;
    let lowering = lower_source_with_body_mutation_and_optimization(
        source,
        |body| {
            let span = body.blocks[0].span;
            let local = body
                .locals
                .iter()
                .find(|local| local.kind == nia_function_ir::FunctionLocalKind::MutableBinding)
                .expect("binding local");
            body.blocks[0].ops.push(FunctionOp::StoreLocal {
                local_id: local.id,
                value: nia_function_ir::FunctionExpr {
                    span,
                    ty: local.ty,
                    kind: FunctionExprKind::Local(local.id),
                },
                span,
            });
        },
        nia_opt::NiaOptimizationLevel::O0.policy(),
    );
    let main = lowering.program.modules[0]
        .functions
        .iter()
        .find(|function| function.name == sym("main"))
        .expect("main function");
    let body = main.function_body.as_ref().expect("main function body");

    assert!(body.blocks.iter().any(|block| {
        block
            .ops
            .iter()
            .any(|op| matches!(op, FunctionOp::StoreLocal { .. }))
    }));
    assert!(lowering.optimization_report.changed_passes.is_empty());
}

#[test]
fn optimization_report_lists_enabled_pass_inventory_by_scope() {
    let source = r#"
static zeroes: [i32; 4] = [0; 4];

fn answer() i32 {
    42
}

fn main() i32 {
    answer() + zeroes[0]
}
"#;

    let o0 = lower_source_with_body_mutation_and_optimization(
        source,
        |_| {},
        nia_opt::NiaOptimizationLevel::O0.policy(),
    );
    assert!(o0.optimization_report.enabled_module_passes.is_empty());
    assert!(o0.optimization_report.enabled_function_passes.is_empty());
    assert!(o0.optimization_report.enabled_global_passes.is_empty());

    let o1 = lower_source_with_body_mutation_and_optimization(
        source,
        |_| {},
        nia_opt::NiaOptimizationLevel::O1.policy(),
    );
    assert_eq!(
        o1.optimization_report.enabled_module_passes,
        vec!["inline-leaf-functions"]
    );
    assert!(
        o1.optimization_report
            .enabled_function_passes
            .contains(&"remove-unused-temp-bindings")
    );
    assert!(o1.optimization_report.enabled_global_passes.is_empty());

    let o2 = lower_source_with_body_mutation_and_optimization(
        source,
        |_| {},
        nia_opt::NiaOptimizationLevel::O2.policy(),
    );
    assert!(
        o2.optimization_report
            .enabled_module_passes
            .contains(&"remove-unused-functions")
    );
    assert!(
        o2.optimization_report
            .enabled_function_passes
            .contains(&"remove-unused-local-bindings")
    );
    assert_eq!(
        o2.optimization_report.enabled_global_passes,
        vec!["simplify-static-init"]
    );

    let o3 = lower_source_with_body_mutation_and_optimization(
        source,
        |_| {},
        nia_opt::NiaOptimizationLevel::O3.policy(),
    );
    assert!(
        o3.optimization_report
            .enabled_module_passes
            .contains(&"devirtualize-direct-trait-calls")
    );
    assert!(
        o3.optimization_report
            .enabled_module_passes
            .contains(&"propagate-cross-function-constants")
    );
    assert!(
        o3.optimization_report
            .enabled_function_passes
            .contains(&"propagate-local-constants")
    );

    for level in [
        nia_opt::NiaOptimizationLevel::Os,
        nia_opt::NiaOptimizationLevel::Oz,
    ] {
        let lowering =
            lower_source_with_body_mutation_and_optimization(source, |_| {}, level.policy());
        assert!(
            lowering
                .optimization_report
                .enabled_module_passes
                .contains(&"inline-leaf-functions"),
            "{level:?}"
        );
        assert_eq!(
            lowering.optimization_report.enabled_global_passes,
            vec!["simplify-static-init"],
            "{level:?}"
        );
    }
}

#[test]
fn o1_removes_pure_backend_expr_ops() {
    let source = r#"
fn main() i32 {
    0
}
"#;
    let lowering = lower_source_with_body_mutation_and_optimization(
        source,
        |body| {
            let span = body.blocks[0].span;
            let ty = body.ty;
            body.blocks[0]
                .ops
                .push(FunctionOp::Expr(nia_function_ir::FunctionExpr {
                    span,
                    ty,
                    kind: FunctionExprKind::Binary {
                        lhs: Box::new(nia_function_ir::FunctionExpr {
                            span,
                            ty,
                            kind: FunctionExprKind::Integer("0".to_string()),
                        }),
                        op: nia_ast::BinaryOp::Add,
                        rhs: Box::new(nia_function_ir::FunctionExpr {
                            span,
                            ty,
                            kind: FunctionExprKind::Integer("1".to_string()),
                        }),
                    },
                }));
        },
        nia_opt::NiaOptimizationLevel::O1.policy(),
    );
    let main = lowering.program.modules[0]
        .functions
        .iter()
        .find(|function| function.name == sym("main"))
        .expect("main function");
    let body = main.function_body.as_ref().expect("main function body");

    assert!(body.blocks.iter().all(|block| block.ops.is_empty()));
}

#[test]
fn o0_preserves_pure_backend_expr_ops() {
    let source = r#"
fn main() i32 {
    0
}
"#;
    let lowering = lower_source_with_body_mutation_and_optimization(
        source,
        |body| {
            let span = body.blocks[0].span;
            let ty = body.ty;
            body.blocks[0]
                .ops
                .push(FunctionOp::Expr(nia_function_ir::FunctionExpr {
                    span,
                    ty,
                    kind: FunctionExprKind::Binary {
                        lhs: Box::new(nia_function_ir::FunctionExpr {
                            span,
                            ty,
                            kind: FunctionExprKind::Integer("0".to_string()),
                        }),
                        op: nia_ast::BinaryOp::Add,
                        rhs: Box::new(nia_function_ir::FunctionExpr {
                            span,
                            ty,
                            kind: FunctionExprKind::Integer("1".to_string()),
                        }),
                    },
                }));
        },
        nia_opt::NiaOptimizationLevel::O0.policy(),
    );
    let main = lowering.program.modules[0]
        .functions
        .iter()
        .find(|function| function.name == sym("main"))
        .expect("main function");
    let body = main.function_body.as_ref().expect("main function body");

    assert!(body.blocks.iter().any(|block| !block.ops.is_empty()));
}

#[test]
fn o2_removes_unused_backend_local_bindings() {
    let source = r#"
fn main() i32 {
    let mut unused = 1;
    0
}
"#;
    let lowering = lower_source_with_body_mutation_and_optimization(
        source,
        |_| {},
        nia_opt::NiaOptimizationLevel::O2.policy(),
    );
    let main = lowering.program.modules[0]
        .functions
        .iter()
        .find(|function| function.name == sym("main"))
        .expect("main function");
    let body = main.function_body.as_ref().expect("main function body");

    assert!(
        body.locals
            .iter()
            .all(|local| local.name != local_name("unused"))
    );
    assert!(body.blocks.iter().all(|block| {
        block.ops.iter().all(|op| {
            !matches!(
                op,
                FunctionOp::Binding(binding) if binding.name == local_name("unused")
            )
        })
    }));
}

#[test]
fn o1_preserves_unused_backend_local_bindings() {
    let source = r#"
fn main() i32 {
    let mut unused = 1;
    0
}
"#;
    let lowering = lower_source_with_body_mutation_and_optimization(
        source,
        |_| {},
        nia_opt::NiaOptimizationLevel::O1.policy(),
    );
    let main = lowering.program.modules[0]
        .functions
        .iter()
        .find(|function| function.name == sym("main"))
        .expect("main function");
    let body = main.function_body.as_ref().expect("main function body");

    assert!(
        body.locals
            .iter()
            .any(|local| local.name == local_name("unused"))
    );
    assert!(body.blocks.iter().any(|block| {
        block.ops.iter().any(|op| {
            matches!(
                op,
                FunctionOp::Binding(binding) if binding.name == local_name("unused")
            )
        })
    }));
}

#[test]
fn o1_removes_unused_backend_temp_bindings() {
    let source = r#"
fn main() i32 {
    0
}
"#;
    let lowering = lower_source_with_body_mutation_and_optimization(
        source,
        |body| {
            let span = body.span;
            let ty = body.ty;
            let temp = LocalId(999);
            body.locals.push(nia_function_ir::FunctionLocal {
                id: temp,
                name: nia_function_ir::LocalName::temporary(999),
                kind: nia_function_ir::FunctionLocalKind::MutableBinding,
                ty,
                span,
            });
            body.blocks[0]
                .ops
                .push(FunctionOp::Binding(nia_function_ir::FunctionBinding {
                    local_id: temp,
                    name: nia_function_ir::LocalName::temporary(999),
                    ty,
                    value: Some(FunctionExpr {
                        span,
                        ty,
                        kind: FunctionExprKind::Integer("1".to_string()),
                    }),
                    is_let: false,
                }));
        },
        nia_opt::NiaOptimizationLevel::O1.policy(),
    );
    let main = lowering.program.modules[0]
        .functions
        .iter()
        .find(|function| function.name == sym("main"))
        .expect("main function");
    let body = main.function_body.as_ref().expect("main function body");

    assert!(
        body.locals
            .iter()
            .all(|local| local.name != nia_function_ir::LocalName::temporary(999))
    );
    assert!(body.blocks.iter().all(|block| {
        block.ops.iter().all(|op| {
            !matches!(
                op,
                FunctionOp::Binding(binding) if binding.name == nia_function_ir::LocalName::temporary(999)
            )
        })
    }));
    assert!(
        lowering
            .optimization_report
            .changed_passes
            .iter()
            .any(|change| {
                matches!(
                    change,
                    BackendOptimizationChange::Function {
                        pass: "remove-unused-temp-bindings",
                        ..
                    }
                )
            })
    );
}

#[test]
fn o1_removes_pure_zst_local_runtime_ops() {
    let source = r#"
struct Empty {}

fn main() i32 {
    let mut local = Empty {};
    local = Empty {};
    0
}
"#;
    let lowering = lower_source_with_body_mutation_and_optimization(
        source,
        |_| {},
        nia_opt::NiaOptimizationLevel::O1.policy(),
    );
    let main = lowering.program.modules[0]
        .functions
        .iter()
        .find(|function| function.name == sym("main"))
        .expect("main function");
    let body = main.function_body.as_ref().expect("main function body");

    assert!(body.blocks.iter().all(|block| {
        block.ops.iter().all(|op| {
            !matches!(
                op,
                FunctionOp::Binding(binding) if binding.name == local_name("local")
            )
        })
    }));
    assert!(body.blocks.iter().all(|block| {
        block
            .ops
            .iter()
            .all(|op| !matches!(op, FunctionOp::StoreLocal { .. }))
    }));
    assert!(
        lowering
            .optimization_report
            .changed_passes
            .iter()
            .any(|change| {
                matches!(
                    change,
                    BackendOptimizationChange::Function {
                        pass: "remove-zst-local-runtime-ops",
                        ..
                    }
                )
            })
    );
}

#[test]
fn o0_preserves_pure_zst_local_runtime_ops() {
    let source = r#"
struct Empty {}

fn main() i32 {
    let mut local = Empty {};
    local = {};
    0
}
"#;
    let lowering = lower_source_with_body_mutation_and_optimization(
        source,
        |_| {},
        nia_opt::NiaOptimizationLevel::O0.policy(),
    );
    let main = lowering.program.modules[0]
        .functions
        .iter()
        .find(|function| function.name == sym("main"))
        .expect("main function");
    let body = main.function_body.as_ref().expect("main function body");

    assert!(body.blocks.iter().any(|block| {
        block.ops.iter().any(|op| {
            matches!(
                op,
                FunctionOp::Binding(binding) if binding.name == local_name("local")
            )
        })
    }));
    assert!(body.blocks.iter().any(|block| {
        block.ops.iter().any(|op| {
            matches!(
                op,
                FunctionOp::Expr(FunctionExpr {
                    kind: FunctionExprKind::Assign { .. },
                    ..
                })
            )
        })
    }));
    assert!(lowering.optimization_report.changed_passes.is_empty());
}

#[test]
fn o1_preserves_effects_from_zst_local_runtime_ops() {
    let source = r#"
struct Wrap {
    value: (),
}

extern fn log(value: i32);

fn effect(value: i32) () {
    log(value);
}

fn main() i32 {
    let mut local = Wrap { value: effect(1) };
    local = Wrap { value: effect(2) };
    0
}
"#;
    let lowering = lower_source_with_body_mutation_and_optimization(
        source,
        |_| {},
        nia_opt::NiaOptimizationLevel::O1.policy(),
    );
    let main = lowering.program.modules[0]
        .functions
        .iter()
        .find(|function| function.name == sym("main"))
        .expect("main function");
    let body = main.function_body.as_ref().expect("main function body");
    let effectful_exprs = body
        .blocks
        .iter()
        .flat_map(|block| block.ops.iter())
        .filter(|op| zst_struct_effect_expr(op))
        .count();

    assert_eq!(effectful_exprs, 2);
    assert!(body.blocks.iter().all(|block| {
        block.ops.iter().all(|op| {
            !matches!(
                op,
                FunctionOp::Binding(binding) if binding.name == local_name("local")
            )
        })
    }));
    assert!(body.blocks.iter().all(|block| {
        block
            .ops
            .iter()
            .all(|op| !matches!(op, FunctionOp::StoreLocal { .. }))
    }));
}

fn zst_struct_effect_expr(op: &FunctionOp) -> bool {
    match op {
        FunctionOp::Expr(FunctionExpr {
            kind: FunctionExprKind::StructLiteral { .. },
            ..
        }) => true,
        FunctionOp::Expr(FunctionExpr {
            kind: FunctionExprKind::Discard(inner),
            ..
        }) => matches!(inner.kind, FunctionExprKind::StructLiteral { .. }),
        _ => false,
    }
}

#[test]
fn o0_preserves_zst_local_runtime_ops() {
    let source = r#"
struct Empty {}

fn main() i32 {
    let mut local = Empty {};
    local = {};
    0
}
"#;
    let lowering = lower_source_with_body_mutation_and_optimization(
        source,
        |_| {},
        nia_opt::NiaOptimizationLevel::O0.policy(),
    );
    let main = lowering.program.modules[0]
        .functions
        .iter()
        .find(|function| function.name == sym("main"))
        .expect("main function");
    let body = main.function_body.as_ref().expect("main function body");

    assert!(body.blocks.iter().any(|block| {
        block.ops.iter().any(|op| {
            matches!(
                op,
                FunctionOp::Binding(binding) if binding.name == local_name("local")
            )
        })
    }));
    assert!(body.blocks.iter().any(|block| {
        block.ops.iter().any(|op| {
            matches!(
                op,
                FunctionOp::Expr(FunctionExpr {
                    kind: FunctionExprKind::Assign { .. },
                    ..
                })
            )
        })
    }));
}

#[test]
fn o2_propagates_backend_local_copies() {
    let source = r#"
fn main() i32 {
    let mut source = 1;
    let mut copy = source;
    copy
}
"#;
    let lowering = lower_source_with_body_mutation_and_optimization(
        source,
        |_| {},
        nia_opt::NiaOptimizationLevel::O2.policy(),
    );
    let main = lowering.program.modules[0]
        .functions
        .iter()
        .find(|function| function.name == sym("main"))
        .expect("main function");
    let body = main.function_body.as_ref().expect("main function body");
    let source_id = body
        .locals
        .iter()
        .find(|local| local.name == local_name("source"))
        .expect("source local")
        .id;
    let value = first_terminal_value(body);
    assert!(matches!(value.kind, FunctionExprKind::Local(id) if id == source_id));
    assert!(
        body.locals
            .iter()
            .all(|local| local.name != local_name("copy"))
    );
}

#[test]
fn o1_preserves_backend_local_copies() {
    let source = r#"
fn main() i32 {
    let mut source = 1;
    let mut copy = source;
    copy
}
"#;
    let lowering = lower_source_with_body_mutation_and_optimization(
        source,
        |_| {},
        nia_opt::NiaOptimizationLevel::O1.policy(),
    );
    let main = lowering.program.modules[0]
        .functions
        .iter()
        .find(|function| function.name == sym("main"))
        .expect("main function");
    let body = main.function_body.as_ref().expect("main function body");
    let copy_id = body
        .locals
        .iter()
        .find(|local| local.name == local_name("copy"))
        .expect("copy local")
        .id;
    let value = first_terminal_value(body);

    assert!(matches!(value.kind, FunctionExprKind::Local(id) if id == copy_id));
    assert!(
        body.locals
            .iter()
            .any(|local| local.name == local_name("copy"))
    );
}

#[test]
fn o3_propagates_backend_local_constants() {
    let source = r#"
fn main() i32 {
    let mut value = 42;
    value
}
"#;
    let lowering = lower_source_with_body_mutation_and_optimization(
        source,
        |_| {},
        nia_opt::NiaOptimizationLevel::O3.policy(),
    );
    let main = lowering.program.modules[0]
        .functions
        .iter()
        .find(|function| function.name == sym("main"))
        .expect("main function");
    let body = main.function_body.as_ref().expect("main function body");
    let value = first_terminal_value(body);

    assert!(matches!(
        &value.kind,
        FunctionExprKind::Integer(value) if value == "42"
    ));
    assert!(
        body.locals
            .iter()
            .all(|local| local.name != local_name("value"))
    );
    assert!(
        lowering
            .optimization_report
            .changed_passes
            .iter()
            .any(|change| matches!(
                change,
                BackendOptimizationChange::Function {
                    pass: "propagate-local-constants",
                    ..
                }
            ))
    );
}

#[test]
fn o2_preserves_backend_local_constants() {
    let source = r#"
fn main() i32 {
    let mut value = 42;
    value
}
"#;
    let lowering = lower_source_with_body_mutation_and_optimization(
        source,
        |_| {},
        nia_opt::NiaOptimizationLevel::O2.policy(),
    );
    let main = lowering.program.modules[0]
        .functions
        .iter()
        .find(|function| function.name == sym("main"))
        .expect("main function");
    let body = main.function_body.as_ref().expect("main function body");
    let value_id = body
        .locals
        .iter()
        .find(|local| local.name == local_name("value"))
        .expect("value local")
        .id;
    let value = first_terminal_value(body);

    assert!(matches!(value.kind, FunctionExprKind::Local(id) if id == value_id));
    assert!(
        lowering
            .optimization_report
            .changed_passes
            .iter()
            .all(|change| !matches!(
                change,
                BackendOptimizationChange::Function {
                    pass: "propagate-local-constants",
                    ..
                }
            ))
    );
}

#[test]
fn o2_folds_constant_backend_switches() {
    let source = r#"
fn main() i32 {
    match 2 {
        1 => 10,
        2 => 20,
        _ => 30,
    }
}
"#;
    let lowering = lower_source_with_body_mutation_and_optimization(
        source,
        |_| {},
        nia_opt::NiaOptimizationLevel::O2.policy(),
    );
    let main = lowering.program.modules[0]
        .functions
        .iter()
        .find(|function| function.name == sym("main"))
        .expect("main function");
    let body = main.function_body.as_ref().expect("main function body");

    assert!(
        body.blocks
            .iter()
            .all(|block| { !matches!(block.terminator, FunctionTerminator::Switch { .. }) })
    );
    assert!(
        lowering
            .optimization_report
            .changed_passes
            .iter()
            .any(|change| matches!(
                change,
                BackendOptimizationChange::Function {
                    pass: "fold-constant-switches",
                    ..
                }
            ))
    );
}

#[test]
fn o1_preserves_constant_backend_switches() {
    let source = r#"
fn main() i32 {
    match 2 {
        1 => 10,
        2 => 20,
        _ => 30,
    }
}
"#;
    let lowering = lower_source_with_body_mutation_and_optimization(
        source,
        |_| {},
        nia_opt::NiaOptimizationLevel::O1.policy(),
    );
    let main = lowering.program.modules[0]
        .functions
        .iter()
        .find(|function| function.name == sym("main"))
        .expect("main function");
    let body = main.function_body.as_ref().expect("main function body");

    assert!(
        body.blocks
            .iter()
            .any(|block| matches!(block.terminator, FunctionTerminator::Switch { .. }))
    );
    assert!(
        lowering
            .optimization_report
            .changed_passes
            .iter()
            .all(|change| !matches!(
                change,
                BackendOptimizationChange::Function {
                    pass: "fold-constant-switches",
                    ..
                }
            ))
    );
}

#[test]
fn o2_removes_overwritten_backend_local_stores() {
    let source = r#"
fn main() i32 {
    let mut target = 0;
    target
}
"#;
    let lowering = lower_source_with_body_mutation_and_optimization(
        source,
        |body| {
            let target_id = body
                .locals
                .iter()
                .find(|local| local.name == local_name("target"))
                .expect("target local")
                .id;
            let span = body.blocks[0].span;
            let ty = body.ty;
            body.blocks[0].ops.push(FunctionOp::StoreLocal {
                local_id: target_id,
                value: nia_function_ir::FunctionExpr {
                    span,
                    ty,
                    kind: FunctionExprKind::Integer("1".to_string()),
                },
                span,
            });
            body.blocks[0].ops.push(FunctionOp::StoreLocal {
                local_id: target_id,
                value: nia_function_ir::FunctionExpr {
                    span,
                    ty,
                    kind: FunctionExprKind::Integer("2".to_string()),
                },
                span,
            });
        },
        nia_opt::NiaOptimizationLevel::O2.policy(),
    );
    let main = lowering.program.modules[0]
        .functions
        .iter()
        .find(|function| function.name == sym("main"))
        .expect("main function");
    let body = main.function_body.as_ref().expect("main function body");

    assert_eq!(
        body.blocks
            .iter()
            .flat_map(|block| &block.ops)
            .filter(|op| matches!(op, FunctionOp::StoreLocal { .. }))
            .count(),
        1
    );
}

#[test]
fn o1_preserves_overwritten_backend_local_stores() {
    let source = r#"
fn main() i32 {
    let mut target = 0;
    target
}
"#;
    let lowering = lower_source_with_body_mutation_and_optimization(
        source,
        |body| {
            let target_id = body
                .locals
                .iter()
                .find(|local| local.name == local_name("target"))
                .expect("target local")
                .id;
            let span = body.blocks[0].span;
            let ty = body.ty;
            body.blocks[0].ops.push(FunctionOp::StoreLocal {
                local_id: target_id,
                value: nia_function_ir::FunctionExpr {
                    span,
                    ty,
                    kind: FunctionExprKind::Integer("1".to_string()),
                },
                span,
            });
            body.blocks[0].ops.push(FunctionOp::StoreLocal {
                local_id: target_id,
                value: nia_function_ir::FunctionExpr {
                    span,
                    ty,
                    kind: FunctionExprKind::Integer("2".to_string()),
                },
                span,
            });
        },
        nia_opt::NiaOptimizationLevel::O1.policy(),
    );
    let main = lowering.program.modules[0]
        .functions
        .iter()
        .find(|function| function.name == sym("main"))
        .expect("main function");
    let body = main.function_body.as_ref().expect("main function body");

    assert_eq!(
        body.blocks
            .iter()
            .flat_map(|block| &block.ops)
            .filter(|op| matches!(op, FunctionOp::StoreLocal { .. }))
            .count(),
        2
    );
}

#[test]
fn o2_removes_never_read_backend_local_stores() {
    let source = r#"
fn main() i32 {
    let mut target = 0;
    1
}
"#;
    let lowering = lower_source_with_body_mutation_and_optimization(
        source,
        |body| {
            let target_id = body
                .locals
                .iter()
                .find(|local| local.name == local_name("target"))
                .expect("target local")
                .id;
            let span = body.blocks[0].span;
            let ty = body.ty;
            body.blocks[0].ops.push(FunctionOp::StoreLocal {
                local_id: target_id,
                value: nia_function_ir::FunctionExpr {
                    span,
                    ty,
                    kind: FunctionExprKind::Integer("2".to_string()),
                },
                span,
            });
        },
        nia_opt::NiaOptimizationLevel::O2.policy(),
    );
    let main = lowering.program.modules[0]
        .functions
        .iter()
        .find(|function| function.name == sym("main"))
        .expect("main function");
    let body = main.function_body.as_ref().expect("main function body");

    assert!(body.blocks.iter().all(|block| {
        block
            .ops
            .iter()
            .all(|op| !matches!(op, FunctionOp::StoreLocal { .. }))
    }));
}

#[test]
fn o1_preserves_never_read_backend_local_stores() {
    let source = r#"
fn main() i32 {
    let mut target = 0;
    1
}
"#;
    let lowering = lower_source_with_body_mutation_and_optimization(
        source,
        |body| {
            let target_id = body
                .locals
                .iter()
                .find(|local| local.name == local_name("target"))
                .expect("target local")
                .id;
            let span = body.blocks[0].span;
            let ty = body.ty;
            body.blocks[0].ops.push(FunctionOp::StoreLocal {
                local_id: target_id,
                value: nia_function_ir::FunctionExpr {
                    span,
                    ty,
                    kind: FunctionExprKind::Integer("2".to_string()),
                },
                span,
            });
        },
        nia_opt::NiaOptimizationLevel::O1.policy(),
    );
    let main = lowering.program.modules[0]
        .functions
        .iter()
        .find(|function| function.name == sym("main"))
        .expect("main function");
    let body = main.function_body.as_ref().expect("main function body");

    assert!(body.blocks.iter().any(|block| {
        block
            .ops
            .iter()
            .any(|op| matches!(op, FunctionOp::StoreLocal { .. }))
    }));
}
