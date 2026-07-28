use super::*;

#[test]
fn resolved_names_do_not_fall_back_to_ident_lookup() {
    let expr = EarlyConstExpr {
        span: Span::new(0, 1),
        kind: EarlyConstExprKind::Ident(nia_const_ir::EarlyConstName::resolved(
            sym("x"),
            ConstNameResolution::Local(nia_ids::LocalId(0)),
        )),
    };
    let err = eval_early_const_expr(&expr, &mut EmptyEnv)
        .expect_err("resolved names must be handled explicitly");
    let expected_name = EmptyEnv.symbol_name(sym("x"));
    assert_eq!(
        err.message,
        format!(
            "resolved const value `{}` is not available in this context",
            expected_name
        )
    );
}

#[test]
fn resolved_function_calls_use_resolved_callee_identity() {
    struct ResolvedCallEnv {
        module_id: ModuleId,
    }

    impl ConstCommonEnv for ResolvedCallEnv {}

    impl ResolvedConstEnv for ResolvedCallEnv {
        fn resolve_resolved_name(
            &mut self,
            span: Span,
            _resolution: ConstNameResolution,
        ) -> Result<ConstValue, ConstError> {
            Err(ConstError {
                span,
                message: "unexpected resolved value lookup".to_string(),
            })
        }

        fn resolve_resolved_layout_builtin(
            &mut self,
            span: Span,
            _builtin: LayoutBuiltin,
            _type_arg: &ResolvedConstTypeArg,
        ) -> Result<ConstValue, ConstError> {
            Err(ConstError {
                span,
                message: "layout builtins are not available in this test".to_string(),
            })
        }

        fn call_resolved_function(
            &mut self,
            span: Span,
            callee: &ResolvedConstExpr,
            type_args: &[ResolvedConstTypeArg],
            arg_exprs: &[ResolvedConstExpr],
            args: Vec<ConstValue>,
        ) -> Result<ConstValue, ConstError> {
            assert!(type_args.is_empty());
            assert!(arg_exprs.is_empty());
            assert!(args.is_empty());
            assert_eq!(
                callee.name_resolution(),
                Some(ConstNameResolution::Global(nia_ids::GlobalDefId {
                    module_id: self.module_id,
                    def_id: nia_ids::DefId(1),
                }))
            );
            Ok(ConstValue::Int((span.start as i128).into()))
        }
    }

    let module_id = ModuleIdAllocator::new().allocate();
    let expr = ResolvedConstExpr::call(
        Span::new(7, 8),
        ResolvedConstExpr::name(
            Span::new(0, 1),
            ConstNameResolution::Global(nia_ids::GlobalDefId {
                module_id,
                def_id: nia_ids::DefId(1),
            }),
        ),
        Vec::new(),
        Vec::new(),
    );
    let value = eval_resolved_const_int_expr(&expr, &mut ResolvedCallEnv { module_id }).unwrap();
    assert_eq!(value, IntConst::signed(7));
}

#[test]
fn assignment_targets_require_resolved_locals() {
    let expr = EarlyConstExpr {
        span: Span::new(0, 1),
        kind: EarlyConstExprKind::Assign(Box::new(EarlyConstAssign {
            lhs: EarlyConstAssignTarget::Local {
                span: Span::new(0, 1),
                name: sym("x"),
                local_id: None,
                path: Vec::new(),
            },
            op: ConstAssignOp::Add,
            rhs: EarlyConstExpr {
                span: Span::new(4, 5),
                kind: EarlyConstExprKind::Integer("1".to_string()),
            },
        })),
    };
    let err = eval_early_const_expr(&expr, &mut EmptyEnv)
        .expect_err("assignment targets must carry resolved local ids");
    let expected_name = EmptyEnv.symbol_name(sym("x"));
    assert_eq!(
        err.message,
        format!(
            "failed to resolve const assignment target `{}`",
            expected_name
        )
    );
}

#[test]
fn pattern_bindings_require_resolved_locals() {
    let mut env = EmptyEnv;
    let err = EarlyConstEnv::bind_pattern_local(
        &mut env,
        Span::new(0, 1),
        &sym("payload"),
        None,
        ConstValue::Int(IntConst::signed(1)),
    )
    .expect_err("pattern bindings must carry resolved local ids");
    let expected_name = EmptyEnv.symbol_name(sym("payload"));
    assert_eq!(
        err.message,
        format!("failed to resolve const pattern local `{}`", expected_name)
    );
}
