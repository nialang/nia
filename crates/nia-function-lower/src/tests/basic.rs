// SPDX-License-Identifier: GPL-3.0-or-later
use super::common::*;

#[test]
fn lowers_body_to_entry_block_with_tail() {
    let span = Span::default();
    let ty = test_ty();
    let body = TypedBody {
        span,
        locals: vec![TypedLocal {
            id: LocalId(0),
            name: local_name("x"),
            kind: TypedLocalKind::MutableBinding,
            ty,
            span,
        }],
        stmts: Vec::new(),
        tail: Some(Box::new(TypedExpr {
            span,
            ty,
            kind: TypedExprKind::Integer("1".to_string()),
        })),
        ty,
    };

    let function_body = lower_test_function_body(&body).expect("valid typed body");

    assert_eq!(function_body.entry, FunctionBlockId(0));
    assert_eq!(function_body.blocks.len(), 1);
    assert!(function_body.blocks[0].ops.is_empty());
    assert!(matches!(
        function_body.blocks[0].terminator,
        FunctionTerminator::Tail { value: Some(_), .. }
    ));
}

#[test]
fn lowers_struct_pattern_bindings_through_nominal_field_projection() {
    let span = Span::default();
    let ty = test_ty();
    let source = LocalId(0);
    let field_local = LocalId(1);
    let mut module_ids = ModuleIdAllocator::new();
    let field_def = GlobalDefId {
        module_id: module_ids.allocate(),
        def_id: DefId(9),
    };
    let body = TypedBody {
        span,
        locals: vec![
            TypedLocal {
                id: source,
                name: local_name("source"),
                kind: TypedLocalKind::Param,
                ty,
                span,
            },
            TypedLocal {
                id: field_local,
                name: local_name("field"),
                kind: TypedLocalKind::ImmutableBinding,
                ty,
                span,
            },
        ],
        stmts: vec![TypedStmt {
            span,
            kind: TypedStmtKind::PatternBinding(Box::new(TypedPatternBinding {
                pattern: TypedPattern {
                    ty,
                    span,
                    kind: TypedPatternKind::Nominal {
                        constructor: TypedNominalPatternConstructor::Struct {
                            field_defs: vec![field_def],
                        },
                        fields: vec![TypedPattern {
                            ty,
                            span,
                            kind: TypedPatternKind::Bind {
                                local_id: field_local,
                                name: local_name("field"),
                            },
                        }],
                    },
                },
                value: TypedExpr {
                    span,
                    ty,
                    kind: TypedExprKind::Local(source),
                },
            })),
        }],
        tail: Some(Box::new(TypedExpr {
            span,
            ty,
            kind: TypedExprKind::Local(field_local),
        })),
        ty,
    };

    let function_body = lower_test_function_body(&body).expect("valid struct pattern body");
    assert!(function_body.blocks.iter().any(|block| {
        block.ops.iter().any(|op| {
            matches!(
                op,
                FunctionOp::StoreLocal {
                    local_id,
                    value: FunctionExpr {
                        kind: FunctionExprKind::Field { field, .. },
                        ..
                    },
                    ..
                } if *local_id == field_local && *field == field_def
            )
        })
    }));
}

#[test]
fn lowers_closure_state_and_direct_call_to_generated_entry() {
    let span = Span::default();
    let mut module_ids = ModuleIdAllocator::new();
    let module_id = module_ids.allocate();
    let type_store = TypeStore::new();
    let append = type_store.append_for_module(module_id);
    let i32_ty = append.intern(TyKind::Primitive(PrimitiveTy::I32));
    let closure_id = ClosureId {
        owner: GlobalDefId {
            module_id,
            def_id: DefId(7),
        },
        ordinal: 0,
    };
    let closure_ty = append.intern(TyKind::ClosureState {
        closure_id,
        captures: vec![i32_ty],
        params: vec![i32_ty],
        return_type: i32_ty,
    });
    let base = LocalId(0);
    let callback = LocalId(1);
    let captured_base = LocalId(2);
    let value = LocalId(3);
    let closure_body = TypedBody {
        span,
        // Capture aliases belong to `TypedClosureCapture`, not the closure
        // body's storage-bearing local table. Only the ABI parameter is local
        // to this body.
        locals: vec![TypedLocal {
            id: value,
            name: local_name("value"),
            kind: TypedLocalKind::Param,
            ty: i32_ty,
            span,
        }],
        stmts: Vec::new(),
        tail: Some(Box::new(TypedExpr {
            span,
            ty: i32_ty,
            kind: TypedExprKind::Binary {
                lhs: Box::new(TypedExpr {
                    span,
                    ty: i32_ty,
                    kind: TypedExprKind::Local(captured_base),
                }),
                op: nia_ast::BinaryOp::Add,
                rhs: Box::new(TypedExpr {
                    span,
                    ty: i32_ty,
                    kind: TypedExprKind::Local(value),
                }),
            },
        })),
        ty: i32_ty,
    };
    let body = TypedBody {
        span,
        locals: vec![
            TypedLocal {
                id: base,
                name: local_name("base"),
                kind: TypedLocalKind::Param,
                ty: i32_ty,
                span,
            },
            TypedLocal {
                id: callback,
                name: local_name("callback"),
                kind: TypedLocalKind::ImmutableBinding,
                ty: closure_ty,
                span,
            },
        ],
        stmts: vec![TypedStmt {
            span,
            kind: TypedStmtKind::Binding(TypedBinding {
                local_id: callback,
                name: local_name("callback"),
                ty: closure_ty,
                value: Some(TypedExpr {
                    span,
                    ty: closure_ty,
                    kind: TypedExprKind::Closure {
                        closure_id,
                        captures: vec![nia_body_ir::TypedClosureCapture {
                            local_id: captured_base,
                            value: TypedExpr {
                                span,
                                ty: i32_ty,
                                kind: TypedExprKind::Local(base),
                            },
                        }],
                        params: vec![value],
                        body: closure_body,
                    },
                }),
                is_mutable: false,
            }),
        }],
        tail: Some(Box::new(TypedExpr {
            span,
            ty: i32_ty,
            kind: TypedExprKind::Call {
                callee: TypedCallee::Closure(Box::new(TypedExpr {
                    span,
                    ty: closure_ty,
                    kind: TypedExprKind::Local(callback),
                })),
                args: vec![TypedExpr {
                    span,
                    ty: i32_ty,
                    kind: TypedExprKind::Integer("2".to_string()),
                }],
            },
        })),
        ty: i32_ty,
    };

    let lowered = lower_function_body(
        module_id,
        &body,
        FunctionTypeContext::for_module(&type_store, module_id),
    )
    .expect("closure function IR");

    assert_eq!(lowered.closure_entries.len(), 1);
    let entry = &lowered.closure_entries[0];
    assert_eq!(entry.closure_id, closure_id);
    assert_eq!(entry.params, vec![value]);
    assert!(
        !entry
            .body
            .locals
            .iter()
            .any(|local| local.id == captured_base)
    );
    assert!(matches!(
        type_store.get(entry.body.locals[0].ty),
        Some(TyKind::Pointer {
            is_readonly: true,
            elem,
        }) if *elem == closure_ty
    ));
    assert!(matches!(
        &entry.body.blocks[0].terminator,
        FunctionTerminator::Tail {
            value: Some(FunctionExpr {
                kind: FunctionExprKind::Binary { lhs, .. },
                ..
            }),
            ..
        } if matches!(
            lhs.kind,
            FunctionExprKind::TupleField {
                index: 0,
                ..
            }
        )
    ));
    assert!(lowered.body.blocks.iter().any(|block| matches!(
        &block.terminator,
        FunctionTerminator::Tail {
            value: Some(FunctionExpr {
                kind: FunctionExprKind::Call {
                    callee: FunctionCallee::ClosureEntry { closure_id: id, state },
                    ..
                },
                ..
            }),
            ..
        } if *id == closure_id && matches!(state.kind, FunctionExprKind::AddrOf(_))
    )));

    fn closure_value_mut(body: &mut TypedBody) -> &mut TypedExpr {
        let TypedStmtKind::Binding(binding) = &mut body.stmts[0].kind else {
            panic!("expected closure binding");
        };
        binding.value.as_mut().expect("closure initializer")
    }

    let assert_rejected = |candidate: &TypedBody, expected: &str| {
        let error = lower_function_body(
            module_id,
            candidate,
            FunctionTypeContext::for_module(&type_store, module_id),
        )
        .expect_err("malformed closure contract must not lower");
        assert!(error.message.contains(expected), "{error:?}");
    };

    let mut malformed = body.clone();
    closure_value_mut(&mut malformed).ty = i32_ty;
    assert_rejected(&malformed, "does not have a closure-state type");

    let other_closure_ty = append.intern(TyKind::ClosureState {
        closure_id: ClosureId {
            owner: closure_id.owner,
            ordinal: 1,
        },
        captures: vec![i32_ty],
        params: vec![i32_ty],
        return_type: i32_ty,
    });
    let mut malformed = body.clone();
    closure_value_mut(&mut malformed).ty = other_closure_ty;
    assert_rejected(&malformed, "identity does not match");

    let mut malformed = body.clone();
    let TypedExprKind::Closure { captures, .. } = &mut closure_value_mut(&mut malformed).kind
    else {
        panic!("expected closure initializer");
    };
    captures.clear();
    assert_rejected(&malformed, "capture count does not match");

    let mut malformed = body.clone();
    let TypedExprKind::Closure { params, .. } = &mut closure_value_mut(&mut malformed).kind else {
        panic!("expected closure initializer");
    };
    params.clear();
    assert_rejected(&malformed, "parameter count does not match");

    let mut malformed = body.clone();
    let TypedExprKind::Closure {
        body: closure_body, ..
    } = &mut closure_value_mut(&mut malformed).kind
    else {
        panic!("expected closure initializer");
    };
    closure_body.ty = closure_ty;
    assert_rejected(&malformed, "body type does not match");

    let mut malformed = body.clone();
    let TypedExprKind::Closure { captures, .. } = &mut closure_value_mut(&mut malformed).kind
    else {
        panic!("expected closure initializer");
    };
    captures[0].value.ty = closure_ty;
    assert_rejected(&malformed, "capture type does not match");

    let mut malformed = body.clone();
    let TypedExprKind::Closure {
        body: closure_body, ..
    } = &mut closure_value_mut(&mut malformed).kind
    else {
        panic!("expected closure initializer");
    };
    closure_body
        .locals
        .iter_mut()
        .find(|local| local.id == value)
        .expect("parameter local")
        .ty = closure_ty;
    assert_rejected(&malformed, "parameter local does not match");

    let mut malformed = body.clone();
    let TypedExprKind::Closure { captures, .. } = &mut closure_value_mut(&mut malformed).kind
    else {
        panic!("expected closure initializer");
    };
    captures.push(captures[0].clone());
    let duplicate_capture_ty = append.intern(TyKind::ClosureState {
        closure_id,
        captures: vec![i32_ty, i32_ty],
        params: vec![i32_ty],
        return_type: i32_ty,
    });
    closure_value_mut(&mut malformed).ty = duplicate_capture_ty;
    assert_rejected(&malformed, "capture locals must be unique");

    let mut malformed = body.clone();
    let TypedExprKind::Call {
        callee: TypedCallee::Closure(callee),
        ..
    } = &mut malformed.tail.as_mut().expect("call tail").kind
    else {
        panic!("expected closure call");
    };
    callee.ty = i32_ty;
    assert_rejected(&malformed, "callee does not have a closure-state type");
}

#[test]
fn non_terminal_ops_branch_to_tail_block() {
    let span = Span::default();
    let ty = test_ty();
    let expr = TypedExpr {
        span,
        ty,
        kind: TypedExprKind::Integer("1".to_string()),
    };
    let body = TypedBody {
        span,
        locals: Vec::new(),
        stmts: vec![TypedStmt {
            span,
            kind: TypedStmtKind::Expr(expr.clone()),
        }],
        tail: Some(Box::new(expr)),
        ty,
    };

    let function_body = lower_test_function_body(&body).expect("valid typed body");

    assert_eq!(function_body.blocks.len(), 2);
    assert_eq!(function_body.blocks[0].ops.len(), 1);
    assert!(matches!(
        function_body.blocks[0].terminator,
        FunctionTerminator::Next {
            target: FunctionBlockId(1),
            ..
        }
    ));
    assert_eq!(
        function_body.blocks[0].terminator.successors(),
        vec![FunctionBlockId(1)]
    );
    assert!(matches!(
        function_body.blocks[1].terminator,
        FunctionTerminator::Tail { value: Some(_), .. }
    ));
}

#[test]
fn lowers_try_expression_to_try_terminator_and_success_local() {
    let span = Span::default();
    let mut module_ids = ModuleIdAllocator::new();
    let module_id = module_ids.allocate();
    let type_store = TypeStore::new();
    let append = type_store.append_for_module(module_id);
    let i32_ty = append.intern(TyKind::Primitive(PrimitiveTy::I32));
    let optional_i32 = append.intern(TyKind::Optional { elem: i32_ty });
    let body = TypedBody {
        span,
        locals: vec![TypedLocal {
            id: LocalId(0),
            name: local_name("value"),
            kind: TypedLocalKind::MutableBinding,
            ty: optional_i32,
            span,
        }],
        stmts: Vec::new(),
        tail: Some(Box::new(TypedExpr {
            span,
            ty: i32_ty,
            kind: TypedExprKind::Try {
                expr: Box::new(TypedExpr {
                    span,
                    ty: optional_i32,
                    kind: TypedExprKind::Local(LocalId(0)),
                }),
                error_conversion: None,
            },
        })),
        ty: i32_ty,
    };

    let function_body = lower_function_body(
        module_id,
        &body,
        FunctionTypeContext::for_module(&type_store, module_id),
    )
    .expect("valid typed body")
    .body;

    assert!(function_body.blocks.iter().any(|block| {
        matches!(
            block.terminator,
            FunctionTerminator::Try {
                kind: FunctionTryKind::Optional,
                ..
            }
        )
    }));
    assert!(
        !function_body.blocks.iter().any(|block| matches!(
            &block.terminator,
            FunctionTerminator::Tail {
                value: Some(FunctionExpr {
                    kind: FunctionExprKind::Try { .. },
                    ..
                }),
                ..
            }
        )),
        "{function_body:?}"
    );
}

#[test]
fn lowers_address_of_places_to_function_place() {
    let span = Span::default();
    let ty = test_ty();
    let body = TypedBody {
        span,
        locals: vec![TypedLocal {
            id: LocalId(0),
            name: local_name("x"),
            kind: TypedLocalKind::MutableBinding,
            ty,
            span,
        }],
        stmts: Vec::new(),
        tail: Some(Box::new(TypedExpr {
            span,
            ty,
            kind: TypedExprKind::Unary {
                op: nia_ast::UnaryOp::Ref,
                expr: Box::new(TypedExpr {
                    span,
                    ty,
                    kind: TypedExprKind::Local(LocalId(0)),
                }),
            },
        })),
        ty,
    };

    let function_body = lower_test_function_body(&body).expect("valid typed body");

    let tail_block = function_body
        .blocks
        .iter()
        .find(|block| matches!(block.terminator, FunctionTerminator::Tail { .. }))
        .expect("expected tail block");
    let FunctionTerminator::Tail {
        value: Some(value), ..
    } = &tail_block.terminator
    else {
        panic!("expected address-of tail value");
    };
    let FunctionExprKind::AddrOf(place) = &value.kind else {
        panic!("expected address-of place");
    };
    assert!(matches!(place.base, FunctionPlaceBase::Local(LocalId(0))));
}

#[test]
fn address_of_rvalue_materializes_temp_place() {
    let span = Span::default();
    let ty = test_ty();
    let body = TypedBody {
        span,
        locals: Vec::new(),
        stmts: Vec::new(),
        tail: Some(Box::new(TypedExpr {
            span,
            ty,
            kind: TypedExprKind::Unary {
                op: nia_ast::UnaryOp::Ref,
                expr: Box::new(TypedExpr {
                    span,
                    ty,
                    kind: TypedExprKind::Integer("1".to_string()),
                }),
            },
        })),
        ty,
    };

    let function_body = lower_test_function_body(&body).expect("valid typed body");

    let tail_block = function_body
        .blocks
        .iter()
        .find(|block| matches!(block.terminator, FunctionTerminator::Tail { .. }))
        .expect("expected tail block");
    let FunctionTerminator::Tail {
        value: Some(value), ..
    } = &tail_block.terminator
    else {
        panic!("expected address-of tail value");
    };
    let FunctionExprKind::AddrOf(place) = &value.kind else {
        panic!("expected address-of place");
    };
    let FunctionPlaceBase::Local(temp) = place.base else {
        panic!("expected materialized temp local");
    };
    assert_eq!(function_body.locals.len(), 1);
    assert_eq!(function_body.locals[0].id, temp);
    let materialize_block = function_body
        .blocks
        .iter()
        .find(|block| !block.ops.is_empty())
        .expect("expected materialization block");
    assert_eq!(materialize_block.ops.len(), 1);
    let FunctionOp::Binding(binding) = &materialize_block.ops[0] else {
        panic!("expected temp binding");
    };
    assert_eq!(binding.local_id, temp);
    assert!(matches!(
        binding.value.as_ref().map(|value| &value.kind),
        Some(FunctionExprKind::Integer(_))
    ));
}

#[test]
fn address_of_slice_lowers_to_slice_value_not_place() {
    let span = Span::default();
    let ty = test_ty();
    let body = TypedBody {
        span,
        locals: vec![TypedLocal {
            id: LocalId(0),
            name: local_name("ptr"),
            kind: TypedLocalKind::MutableBinding,
            ty,
            span,
        }],
        stmts: Vec::new(),
        tail: Some(Box::new(TypedExpr {
            span,
            ty,
            kind: TypedExprKind::Unary {
                op: nia_ast::UnaryOp::Ref,
                expr: Box::new(TypedExpr {
                    span,
                    ty,
                    kind: TypedExprKind::Slice {
                        lhs: Box::new(TypedExpr {
                            span,
                            ty,
                            kind: TypedExprKind::Local(LocalId(0)),
                        }),
                        range: nia_body_ir::TypedSliceRange {
                            start: Some(Box::new(int_expr(0))),
                            end: None,
                            inclusive: false,
                        },
                        is_readonly: false,
                    },
                }),
            },
        })),
        ty,
    };

    let function_body = lower_test_function_body(&body).expect("valid typed body");

    let tail_block = function_body
        .blocks
        .iter()
        .find(|block| matches!(block.terminator, FunctionTerminator::Tail { .. }))
        .expect("expected tail block");
    let FunctionTerminator::Tail {
        value: Some(value), ..
    } = &tail_block.terminator
    else {
        panic!("expected slice tail value");
    };
    assert!(
        matches!(value.kind, FunctionExprKind::Slice { .. }),
        "{value:?}"
    );
}

#[test]
fn return_terminates_block_before_later_statements() {
    let span = Span::default();
    let ty = test_ty();
    let expr = TypedExpr {
        span,
        ty,
        kind: TypedExprKind::Integer("1".to_string()),
    };
    let body = TypedBody {
        span,
        locals: Vec::new(),
        stmts: vec![
            TypedStmt {
                span,
                kind: TypedStmtKind::Return(Some(expr.clone())),
            },
            TypedStmt {
                span,
                kind: TypedStmtKind::Expr(expr),
            },
        ],
        tail: None,
        ty,
    };

    let function_body = lower_test_function_body(&body).expect("valid typed body");

    assert_eq!(function_body.blocks.len(), 1);
    assert!(function_body.blocks[0].ops.is_empty());
    assert!(matches!(
        function_body.blocks[0].terminator,
        FunctionTerminator::Return { value: Some(_), .. }
    ));
}
