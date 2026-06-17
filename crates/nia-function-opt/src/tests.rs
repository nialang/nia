use super::passes::*;
use super::*;
use nia_function_ir::{
    FunctionArrayElements, FunctionBlock, FunctionBlockId, FunctionCallee, FunctionExpr,
    FunctionExprKind, FunctionLocalKind, FunctionOp, FunctionScope, FunctionScopeId,
    FunctionTerminator, FunctionTryKind, validate_function_body,
};
use nia_ids::LocalId;
use nia_opt::NiaOptimizationLevel;
use nia_span::Span;
use std::collections::HashSet;

#[test]
fn o0_pipeline_has_no_optional_backend_passes() {
    let pipeline = FunctionOptPipeline::for_policy(&NiaOptimizationLevel::O0.policy());

    assert!(pipeline.passes.is_empty());
}

#[test]
fn o1_pipeline_keeps_canonical_pass_order() {
    let pipeline = FunctionOptPipeline::for_policy(&NiaOptimizationLevel::O1.policy());

    assert_eq!(pipeline.passes.as_slice(), O1_PASSES);
}

#[test]
fn o2_family_pipeline_starts_from_o1_cleanup_passes() {
    for level in [
        NiaOptimizationLevel::O2,
        NiaOptimizationLevel::Os,
        NiaOptimizationLevel::Oz,
    ] {
        let pipeline = FunctionOptPipeline::for_policy(&level.policy());

        assert_eq!(pipeline.passes.as_slice(), O2_PASSES);
        for pass in O1_PASSES {
            assert!(pipeline.passes.contains(pass), "{level:?} missing {pass:?}");
        }
        assert!(
            pipeline
                .passes
                .contains(&FunctionOptPass::PropagateLocalCopies)
        );
        assert!(
            pipeline
                .passes
                .contains(&FunctionOptPass::RemoveUnusedLocalBindings)
        );
    }
}

#[test]
fn o3_pipeline_adds_aggressive_constant_propagation() {
    let pipeline = FunctionOptPipeline::for_policy(&NiaOptimizationLevel::O3.policy());

    assert_eq!(pipeline.passes.as_slice(), O3_PASSES);
    assert!(
        pipeline
            .passes
            .contains(&FunctionOptPass::PropagateLocalConstants)
    );
}

#[test]
fn backend_pipeline_is_selected_from_policy_capabilities() {
    let policy = nia_opt::OptimizationPolicy {
        level: NiaOptimizationLevel::O2,
        simplify_cfg: OptimizationDepth::Required,
        const_fold: OptimizationDepth::Cheap,
        dead_code_elim: OptimizationDepth::Full,
        local_copy_prop: OptimizationDepth::Disabled,
        inline_threshold: nia_opt::InlineThreshold::Never,
        specialize_generics: nia_opt::SpecializationPolicy::RequiredOnly,
        dedup_monomorphized_instances: false,
        prefer_size: false,
    };
    let pipeline = FunctionOptPipeline::for_policy(&policy);

    assert_eq!(
        pipeline.passes,
        vec![
            FunctionOptPass::SimplifySameTypeCasts,
            FunctionOptPass::RemoveNoopLocalStores,
            FunctionOptPass::RemovePureExprOps,
            FunctionOptPass::RemoveZstLocalRuntimeOps,
            FunctionOptPass::SimplifyConstantLogicalExprs,
            FunctionOptPass::RemoveOverwrittenLocalStores,
            FunctionOptPass::RemoveNeverReadLocalStores,
            FunctionOptPass::RemoveUnusedTempBindings,
            FunctionOptPass::RemoveUnusedLocalBindings,
            FunctionOptPass::FoldConstantBoolBranches,
        ]
    );
}

#[test]
fn constant_logical_expr_simplification_is_selected_from_const_fold_policy() {
    let policy = nia_opt::OptimizationPolicy {
        level: NiaOptimizationLevel::O2,
        simplify_cfg: OptimizationDepth::Cheap,
        const_fold: OptimizationDepth::Disabled,
        dead_code_elim: OptimizationDepth::Disabled,
        local_copy_prop: OptimizationDepth::Disabled,
        inline_threshold: nia_opt::InlineThreshold::Never,
        specialize_generics: nia_opt::SpecializationPolicy::RequiredOnly,
        dedup_monomorphized_instances: false,
        prefer_size: false,
    };
    let pipeline = FunctionOptPipeline::for_policy(&policy);

    assert!(
        !pipeline
            .passes
            .contains(&FunctionOptPass::SimplifyConstantLogicalExprs)
    );

    let policy = nia_opt::OptimizationPolicy {
        const_fold: OptimizationDepth::Cheap,
        ..policy
    };
    let pipeline = FunctionOptPipeline::for_policy(&policy);

    assert!(
        pipeline
            .passes
            .contains(&FunctionOptPass::SimplifyConstantLogicalExprs)
    );
}

#[test]
fn constant_bool_branch_folding_is_selected_from_const_fold_policy() {
    let policy = nia_opt::OptimizationPolicy {
        level: NiaOptimizationLevel::O2,
        simplify_cfg: OptimizationDepth::Cheap,
        const_fold: OptimizationDepth::Disabled,
        dead_code_elim: OptimizationDepth::Disabled,
        local_copy_prop: OptimizationDepth::Disabled,
        inline_threshold: nia_opt::InlineThreshold::Never,
        specialize_generics: nia_opt::SpecializationPolicy::RequiredOnly,
        dedup_monomorphized_instances: false,
        prefer_size: false,
    };
    let pipeline = FunctionOptPipeline::for_policy(&policy);

    assert!(
        !pipeline
            .passes
            .contains(&FunctionOptPass::FoldConstantBoolBranches)
    );
    assert!(
        pipeline
            .passes
            .contains(&FunctionOptPass::SimplifyTrivialBranches)
    );

    let policy = nia_opt::OptimizationPolicy {
        simplify_cfg: OptimizationDepth::Required,
        const_fold: OptimizationDepth::Cheap,
        ..policy
    };
    let pipeline = FunctionOptPipeline::for_policy(&policy);

    assert!(
        pipeline
            .passes
            .contains(&FunctionOptPass::FoldConstantBoolBranches)
    );
    assert!(
        !pipeline
            .passes
            .contains(&FunctionOptPass::SimplifyTrivialBranches)
    );
}

#[test]
fn same_target_switch_simplification_requires_full_cfg_policy() {
    let policy = nia_opt::OptimizationPolicy {
        level: NiaOptimizationLevel::O2,
        simplify_cfg: OptimizationDepth::Cheap,
        const_fold: OptimizationDepth::Disabled,
        dead_code_elim: OptimizationDepth::Disabled,
        local_copy_prop: OptimizationDepth::Disabled,
        inline_threshold: nia_opt::InlineThreshold::Never,
        specialize_generics: nia_opt::SpecializationPolicy::RequiredOnly,
        dedup_monomorphized_instances: false,
        prefer_size: false,
    };
    let pipeline = FunctionOptPipeline::for_policy(&policy);

    assert!(
        !pipeline
            .passes
            .contains(&FunctionOptPass::SimplifySameTargetSwitches)
    );

    let policy = nia_opt::OptimizationPolicy {
        simplify_cfg: OptimizationDepth::Full,
        ..policy
    };
    let pipeline = FunctionOptPipeline::for_policy(&policy);

    assert!(
        pipeline
            .passes
            .contains(&FunctionOptPass::SimplifySameTargetSwitches)
    );
}

#[test]
fn constant_switch_folding_requires_full_const_and_cfg_policy() {
    let policy = nia_opt::OptimizationPolicy {
        level: NiaOptimizationLevel::O2,
        simplify_cfg: OptimizationDepth::Full,
        const_fold: OptimizationDepth::Cheap,
        dead_code_elim: OptimizationDepth::Disabled,
        local_copy_prop: OptimizationDepth::Disabled,
        inline_threshold: nia_opt::InlineThreshold::Never,
        specialize_generics: nia_opt::SpecializationPolicy::RequiredOnly,
        dedup_monomorphized_instances: false,
        prefer_size: false,
    };
    let pipeline = FunctionOptPipeline::for_policy(&policy);

    assert!(
        !pipeline
            .passes
            .contains(&FunctionOptPass::FoldConstantSwitches)
    );

    let policy = nia_opt::OptimizationPolicy {
        const_fold: OptimizationDepth::Full,
        simplify_cfg: OptimizationDepth::Cheap,
        ..policy
    };
    let pipeline = FunctionOptPipeline::for_policy(&policy);

    assert!(
        !pipeline
            .passes
            .contains(&FunctionOptPass::FoldConstantSwitches)
    );

    let policy = nia_opt::OptimizationPolicy {
        const_fold: OptimizationDepth::Full,
        simplify_cfg: OptimizationDepth::Full,
        ..policy
    };
    let pipeline = FunctionOptPipeline::for_policy(&policy);

    assert!(
        pipeline
            .passes
            .contains(&FunctionOptPass::FoldConstantSwitches)
    );
}

#[test]
fn local_constant_propagation_requires_aggressive_non_size_policy() {
    for level in [
        NiaOptimizationLevel::O0,
        NiaOptimizationLevel::O1,
        NiaOptimizationLevel::O2,
        NiaOptimizationLevel::Os,
        NiaOptimizationLevel::Oz,
    ] {
        let pipeline = FunctionOptPipeline::for_policy(&level.policy());

        assert!(
            !pipeline
                .passes
                .contains(&FunctionOptPass::PropagateLocalConstants),
            "{level:?} unexpectedly enables local constant propagation"
        );
    }

    let pipeline = FunctionOptPipeline::for_policy(&NiaOptimizationLevel::O3.policy());

    assert!(
        pipeline
            .passes
            .contains(&FunctionOptPass::PropagateLocalConstants)
    );
}

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
                name: "a".to_string(),
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
                name: "b".to_string(),
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
            name: "a".to_string(),
            kind: FunctionLocalKind::Binding,
            ty,
            span,
        },
        nia_function_ir::FunctionLocal {
            id: LocalId(1),
            name: "b".to_string(),
            kind: FunctionLocalKind::Binding,
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
                name: "ok".to_string(),
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
            name: "ok".to_string(),
            kind: FunctionLocalKind::Binding,
            ty,
            span,
        },
        nia_function_ir::FunctionLocal {
            id: LocalId(1),
            name: "result".to_string(),
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
            name: "unused".to_string(),
            ty,
            value: Some(FunctionExpr {
                span,
                ty,
                kind: FunctionExprKind::Call {
                    callee: FunctionCallee::Function(nia_ids::GlobalDefId {
                        module_id: nia_ids::ModuleId(0),
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
        name: "unused".to_string(),
        kind: FunctionLocalKind::Binding,
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
        name: "unused".to_string(),
        kind: FunctionLocalKind::Binding,
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
                        module_id: nia_ids::ModuleId(0),
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
        name: "unused".to_string(),
        kind: FunctionLocalKind::Binding,
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
        name: "used".to_string(),
        kind: FunctionLocalKind::Binding,
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
        name: "target".to_string(),
        kind: FunctionLocalKind::Binding,
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
                            module_id: nia_ids::ModuleId(0),
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
        name: "target".to_string(),
        kind: FunctionLocalKind::Binding,
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
        name: "target".to_string(),
        kind: FunctionLocalKind::Binding,
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

#[test]
fn removes_same_type_cast_wrappers_recursively() {
    let span = Span::default();
    let ty = test_ty();
    let mut expr = FunctionExpr {
        span,
        ty,
        kind: FunctionExprKind::Discard(Box::new(FunctionExpr {
            span,
            ty,
            kind: FunctionExprKind::Cast {
                expr: Box::new(FunctionExpr {
                    span,
                    ty,
                    kind: FunctionExprKind::Local(LocalId(0)),
                }),
                ty,
            },
        })),
    };

    simplify_same_type_casts_in_expr(&mut expr);

    let FunctionExprKind::Discard(inner) = expr.kind else {
        panic!("expected discard wrapper");
    };
    assert!(matches!(inner.kind, FunctionExprKind::Local(LocalId(0))));
}

#[test]
fn preserves_casts_that_change_type() {
    let span = Span::default();
    let source_ty = test_ty();
    let target_ty = nia_ids::InternedTyId::new(
        nia_ids::ModuleId(0),
        nia_ids::TyInternerIndex::from_interner_index(1),
    );
    let mut expr = FunctionExpr {
        span,
        ty: target_ty,
        kind: FunctionExprKind::Cast {
            expr: Box::new(FunctionExpr {
                span,
                ty: source_ty,
                kind: FunctionExprKind::Local(LocalId(0)),
            }),
            ty: target_ty,
        },
    };

    simplify_same_type_casts_in_expr(&mut expr);

    assert!(matches!(expr.kind, FunctionExprKind::Cast { .. }));
}

#[test]
fn simplifies_constant_logical_exprs_without_dropping_effects() {
    let span = Span::default();
    let ty = test_ty();
    let mut body = test_body(vec![FunctionBlock {
        id: FunctionBlockId(0),
        scope: FunctionScopeId(0),
        span,
        ops: Vec::new(),
        terminator: FunctionTerminator::Tail {
            value: Some(FunctionExpr {
                span,
                ty,
                kind: FunctionExprKind::Binary {
                    lhs: Box::new(FunctionExpr {
                        span,
                        ty,
                        kind: FunctionExprKind::Bool(true),
                    }),
                    op: nia_ast::BinaryOp::And,
                    rhs: Box::new(FunctionExpr {
                        span,
                        ty,
                        kind: FunctionExprKind::Binary {
                            lhs: Box::new(FunctionExpr {
                                span,
                                ty,
                                kind: FunctionExprKind::Local(LocalId(0)),
                            }),
                            op: nia_ast::BinaryOp::Or,
                            rhs: Box::new(FunctionExpr {
                                span,
                                ty,
                                kind: FunctionExprKind::Bool(false),
                            }),
                        },
                    }),
                },
            }),
            span,
        },
    }]);
    body.locals = vec![nia_function_ir::FunctionLocal {
        id: LocalId(0),
        name: "flag".to_string(),
        kind: FunctionLocalKind::Param,
        ty,
        span,
    }];

    assert!(simplify_constant_logical_exprs(&mut body));

    let FunctionTerminator::Tail {
        value: Some(value), ..
    } = &body.blocks[0].terminator
    else {
        panic!("expected tail value");
    };
    assert!(matches!(value.kind, FunctionExprKind::Local(LocalId(0))));
    validate_function_body(&body).expect("logical-simplified body should remain valid");
}

#[test]
fn preserves_constant_logical_rhs_when_lhs_must_be_evaluated() {
    let span = Span::default();
    let ty = test_ty();
    let mut expr = FunctionExpr {
        span,
        ty,
        kind: FunctionExprKind::Binary {
            lhs: Box::new(FunctionExpr {
                span,
                ty,
                kind: FunctionExprKind::Local(LocalId(0)),
            }),
            op: nia_ast::BinaryOp::And,
            rhs: Box::new(FunctionExpr {
                span,
                ty,
                kind: FunctionExprKind::Bool(false),
            }),
        },
    };

    assert!(!simplify_constant_logical_expr(&mut expr));
    assert!(matches!(
        expr.kind,
        FunctionExprKind::Binary {
            op: nia_ast::BinaryOp::And,
            ..
        }
    ));
}

#[test]
fn removes_noop_local_store_ops() {
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
                    kind: FunctionExprKind::Local(LocalId(0)),
                },
                span,
            },
            FunctionOp::StoreLocal {
                local_id: LocalId(0),
                value: FunctionExpr {
                    span,
                    ty,
                    kind: FunctionExprKind::Local(LocalId(1)),
                },
                span,
            },
        ],
        terminator: FunctionTerminator::Return { value: None, span },
    }]);

    remove_noop_local_stores(&mut body.blocks);

    assert_eq!(body.blocks[0].ops.len(), 1);
    assert!(matches!(
        body.blocks[0].ops[0],
        FunctionOp::StoreLocal {
            local_id: LocalId(0),
            value: FunctionExpr {
                kind: FunctionExprKind::Local(LocalId(1)),
                ..
            },
            ..
        }
    ));
}

#[test]
fn removes_noop_local_store_after_same_type_cast_simplification() {
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
                kind: FunctionExprKind::Cast {
                    expr: Box::new(FunctionExpr {
                        span,
                        ty,
                        kind: FunctionExprKind::Local(LocalId(0)),
                    }),
                    ty,
                },
            },
            span,
        }],
        terminator: FunctionTerminator::Return { value: None, span },
    }]);

    simplify_same_type_casts_in_blocks(&mut body.blocks);
    remove_noop_local_stores(&mut body.blocks);

    assert!(body.blocks[0].ops.is_empty());
}

#[test]
fn removes_pure_expr_ops_but_preserves_effectful_expr_ops() {
    let span = Span::default();
    let ty = test_ty();
    let mut body = test_body(vec![FunctionBlock {
        id: FunctionBlockId(0),
        scope: FunctionScopeId(0),
        span,
        ops: vec![
            FunctionOp::Expr(FunctionExpr {
                span,
                ty,
                kind: FunctionExprKind::Discard(Box::new(FunctionExpr {
                    span,
                    ty,
                    kind: FunctionExprKind::Local(LocalId(0)),
                })),
            }),
            FunctionOp::Expr(FunctionExpr {
                span,
                ty,
                kind: FunctionExprKind::Call {
                    callee: FunctionCallee::Function(nia_ids::GlobalDefId {
                        module_id: nia_ids::ModuleId(0),
                        def_id: nia_ids::DefId(0),
                    }),
                    args: Vec::new(),
                },
            }),
        ],
        terminator: FunctionTerminator::Return { value: None, span },
    }]);

    remove_pure_expr_ops(&mut body.blocks);

    assert_eq!(body.blocks[0].ops.len(), 1);
    assert!(matches!(
        body.blocks[0].ops[0],
        FunctionOp::Expr(FunctionExpr {
            kind: FunctionExprKind::Call { .. },
            ..
        })
    ));
}

#[test]
fn removes_pure_wrapper_expr_ops() {
    let span = Span::default();
    let ty = test_ty();
    let mut body = test_body(vec![FunctionBlock {
        id: FunctionBlockId(0),
        scope: FunctionScopeId(0),
        span,
        ops: vec![
            FunctionOp::Expr(FunctionExpr {
                span,
                ty,
                kind: FunctionExprKind::Binary {
                    lhs: Box::new(FunctionExpr {
                        span,
                        ty,
                        kind: FunctionExprKind::Local(LocalId(0)),
                    }),
                    op: nia_ast::BinaryOp::Add,
                    rhs: Box::new(FunctionExpr {
                        span,
                        ty,
                        kind: FunctionExprKind::Integer("1".to_string()),
                    }),
                },
            }),
            FunctionOp::Expr(FunctionExpr {
                span,
                ty,
                kind: FunctionExprKind::ArrayLiteral {
                    elems: FunctionArrayElements::List(vec![FunctionExpr {
                        span,
                        ty,
                        kind: FunctionExprKind::Cast {
                            expr: Box::new(FunctionExpr {
                                span,
                                ty,
                                kind: FunctionExprKind::Bool(false),
                            }),
                            ty,
                        },
                    }]),
                },
            }),
        ],
        terminator: FunctionTerminator::Return { value: None, span },
    }]);

    remove_pure_expr_ops(&mut body.blocks);

    assert!(body.blocks[0].ops.is_empty());
}

#[test]
fn preserves_aggregate_expr_ops_with_effectful_elements() {
    let span = Span::default();
    let ty = test_ty();
    let mut body = test_body(vec![FunctionBlock {
        id: FunctionBlockId(0),
        scope: FunctionScopeId(0),
        span,
        ops: vec![FunctionOp::Expr(FunctionExpr {
            span,
            ty,
            kind: FunctionExprKind::ArrayLiteral {
                elems: FunctionArrayElements::List(vec![FunctionExpr {
                    span,
                    ty,
                    kind: FunctionExprKind::Call {
                        callee: FunctionCallee::Function(nia_ids::GlobalDefId {
                            module_id: nia_ids::ModuleId(0),
                            def_id: nia_ids::DefId(0),
                        }),
                        args: Vec::new(),
                    },
                }]),
            },
        })],
        terminator: FunctionTerminator::Return { value: None, span },
    }]);

    remove_pure_expr_ops(&mut body.blocks);

    assert_eq!(body.blocks[0].ops.len(), 1);
    assert!(matches!(
        body.blocks[0].ops[0],
        FunctionOp::Expr(FunctionExpr {
            kind: FunctionExprKind::ArrayLiteral { .. },
            ..
        })
    ));
}

#[test]
fn does_not_merge_entry_block_even_when_it_is_an_empty_jump() {
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
    ]);

    merge_empty_jump_blocks(&mut body);
    remove_unreachable_blocks(&mut body);

    assert_eq!(
        body.blocks.iter().map(|block| block.id).collect::<Vec<_>>(),
        vec![FunctionBlockId(0), FunctionBlockId(1)]
    );
    validate_function_body(&body).expect("entry-preserving merge should remain valid");
}

#[test]
fn does_not_merge_empty_jump_blocks_across_scope_boundaries() {
    let span = Span::default();
    let mut body = test_body_with_scopes(
        vec![
            FunctionScope {
                id: FunctionScopeId(0),
                parent: None,
                span,
            },
            FunctionScope {
                id: FunctionScopeId(1),
                parent: Some(FunctionScopeId(0)),
                span,
            },
        ],
        vec![
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
                scope: FunctionScopeId(1),
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
        ],
    );

    merge_empty_jump_blocks(&mut body);
    remove_unreachable_blocks(&mut body);

    assert_eq!(
        body.blocks.iter().map(|block| block.id).collect::<Vec<_>>(),
        vec![FunctionBlockId(0), FunctionBlockId(1), FunctionBlockId(2)]
    );
    assert_eq!(
        body.edge_exited_scopes(FunctionBlockId(1), FunctionBlockId(2)),
        Some(vec![FunctionScopeId(1)])
    );
    validate_function_body(&body).expect("scope-preserving merge should remain valid");
}

fn test_body(blocks: Vec<FunctionBlock>) -> FunctionBody {
    test_body_with_scopes(
        vec![FunctionScope {
            id: FunctionScopeId(0),
            parent: None,
            span: Span::default(),
        }],
        blocks,
    )
}

fn test_body_with_scopes(scopes: Vec<FunctionScope>, blocks: Vec<FunctionBlock>) -> FunctionBody {
    FunctionBody {
        span: Span::default(),
        locals: Vec::new(),
        scopes,
        blocks,
        entry: FunctionBlockId(0),
        ty: test_ty(),
    }
}

fn test_ty() -> nia_ids::InternedTyId {
    nia_ids::InternedTyId::new(
        nia_ids::ModuleId(0),
        nia_ids::TyInternerIndex::from_interner_index(0),
    )
}
