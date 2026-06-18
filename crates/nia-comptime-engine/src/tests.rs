use crate::{
    ComptimeCommonEnv, ComptimeError, ComptimeValue, EarlyComptimeEnv, EmptyEnv,
    ResolvedComptimeEnv, eval_early_comptime_bool_expr, eval_early_comptime_expr,
    eval_early_comptime_int_expr, eval_float_literal, eval_int_literal,
    eval_resolved_comptime_int_expr,
};
use nia_comptime_ir::{
    ComptimeAssignOp, ComptimeNameResolution, EarlyComptimeAssign, EarlyComptimeAssignTarget,
    EarlyComptimeExpr, EarlyComptimeExprKind, EarlyComptimeName, EarlyComptimeTypeArg,
    ResolvedComptimeExpr, ResolvedComptimeTypeArg,
};
use nia_ids::{LayoutBuiltin, ModuleId, ValueBuiltin};
use nia_span::Span;
use nia_ty::IntConst;
use std::collections::BTreeMap;

#[test]
fn eval_int_literal_ignores_type_suffix() {
    assert_eq!(eval_int_literal("42i32"), Ok(42));
    assert_eq!(eval_int_literal("0xffu8"), Ok(255));
    assert_eq!(eval_int_literal("1_024usize"), Ok(1024));
}

#[test]
fn eval_float_literal_ignores_type_suffix_and_separators() {
    assert_eq!(eval_float_literal("0.0f64"), Ok(0.0));
    assert_eq!(eval_float_literal("1_024.5f32"), Ok(1024.5));
    assert_eq!(eval_float_literal("1.25e-1f64"), Ok(0.125));
}

#[test]
fn evaluates_builtin_struct_field_conditions() {
    let (module, errors) = nia_parser::parse_module(
        r#"
fn main() bool {
    @builtin().target.os == "linux" and @builtin().target.pointer_width == 64
}
"#,
    );
    assert!(errors.is_empty(), "{errors:?}");
    let nia_ast::ItemKind::Function(function) = &module.items[0].kind else {
        panic!("expected function");
    };
    let expr = function.body.as_ref().unwrap().tail.as_deref().unwrap();
    let expr = nia_comptime_ir::lower_expr_early(expr).unwrap();
    let value = eval_early_comptime_bool_expr(&expr, &mut BuiltinEnv).unwrap();
    assert!(value);
}

#[test]
fn evaluates_lowered_comptime_expr_directly() {
    let (module, errors) = nia_parser::parse_module(
        r#"
fn main() bool {
    @builtin().target.os == "linux"
}
"#,
    );
    assert!(errors.is_empty(), "{errors:?}");
    let nia_ast::ItemKind::Function(function) = &module.items[0].kind else {
        panic!("expected function");
    };
    let expr = function.body.as_ref().unwrap().tail.as_deref().unwrap();
    let lowered = nia_comptime_ir::lower_expr_early(expr).unwrap();
    let EarlyComptimeExprKind::Binary { lhs, .. } = &lowered.kind else {
        panic!("expected lowered binary expression");
    };
    let EarlyComptimeExprKind::Field { name, .. } = &lhs.kind else {
        panic!("expected lowered field expression");
    };
    assert_eq!(name, "os");

    let value = eval_early_comptime_bool_expr(&lowered, &mut BuiltinEnv).unwrap();
    assert!(value);
}

#[test]
fn unknown_builtin_value_is_rejected_during_lowering() {
    let (module, errors) = nia_parser::parse_module(
        r#"
fn main() bool {
    @unknown
}
"#,
    );
    assert!(errors.is_empty(), "{errors:?}");
    let nia_ast::ItemKind::Function(function) = &module.items[0].kind else {
        panic!("expected function");
    };
    let expr = function.body.as_ref().unwrap().tail.as_deref().unwrap();
    let err =
        nia_comptime_ir::lower_expr_early(expr).expect_err("unknown builtin should not lower");
    assert_eq!(
        err.message,
        "unsupported builtin value in comptime expression: @unknown"
    );
}

#[test]
fn resolved_names_do_not_fall_back_to_ident_lookup() {
    let expr = EarlyComptimeExpr {
        span: Span::new(0, 1),
        kind: EarlyComptimeExprKind::Ident(nia_comptime_ir::EarlyComptimeName::resolved(
            "x".to_string(),
            ComptimeNameResolution::Local(nia_ids::LocalId(0)),
        )),
    };
    let err = eval_early_comptime_expr(&expr, &mut EmptyEnv)
        .expect_err("resolved names must be handled explicitly");
    assert_eq!(
        err.message,
        "resolved comptime value `x` is not available in this context"
    );
}

#[test]
fn resolved_function_calls_use_resolved_callee_identity() {
    struct ResolvedCallEnv;

    impl ComptimeCommonEnv for ResolvedCallEnv {}

    impl ResolvedComptimeEnv for ResolvedCallEnv {
        fn resolve_resolved_name(
            &mut self,
            span: Span,
            _resolution: ComptimeNameResolution,
        ) -> Result<ComptimeValue, ComptimeError> {
            Err(ComptimeError {
                span,
                message: "unexpected resolved value lookup".to_string(),
            })
        }

        fn resolve_resolved_layout_builtin(
            &mut self,
            span: Span,
            _builtin: LayoutBuiltin,
            _type_arg: &ResolvedComptimeTypeArg,
        ) -> Result<ComptimeValue, ComptimeError> {
            Err(ComptimeError {
                span,
                message: "layout builtins are not available in this test".to_string(),
            })
        }

        fn call_resolved_function(
            &mut self,
            span: Span,
            callee: &ResolvedComptimeExpr,
            type_args: &[ResolvedComptimeTypeArg],
            arg_exprs: &[ResolvedComptimeExpr],
            args: Vec<ComptimeValue>,
        ) -> Result<ComptimeValue, ComptimeError> {
            assert!(type_args.is_empty());
            assert!(arg_exprs.is_empty());
            assert!(args.is_empty());
            assert_eq!(
                callee.name_resolution(),
                Some(ComptimeNameResolution::Global(nia_ids::GlobalDefId {
                    module_id: ModuleId(0),
                    def_id: nia_ids::DefId(1),
                }))
            );
            Ok(ComptimeValue::Int((span.start as i128).into()))
        }
    }

    let expr = ResolvedComptimeExpr::call(
        Span::new(7, 8),
        ResolvedComptimeExpr::name(
            Span::new(0, 1),
            ComptimeNameResolution::Global(nia_ids::GlobalDefId {
                module_id: ModuleId(0),
                def_id: nia_ids::DefId(1),
            }),
        ),
        Vec::new(),
        Vec::new(),
    );
    let value = eval_resolved_comptime_int_expr(&expr, &mut ResolvedCallEnv).unwrap();
    assert_eq!(value, IntConst::signed(7));
}

#[test]
fn assignment_targets_require_resolved_locals() {
    let expr = EarlyComptimeExpr {
        span: Span::new(0, 1),
        kind: EarlyComptimeExprKind::Assign(Box::new(EarlyComptimeAssign {
            lhs: EarlyComptimeAssignTarget::Local {
                span: Span::new(0, 1),
                name: "x".to_string(),
                local_id: None,
                path: Vec::new(),
            },
            op: ComptimeAssignOp::Add,
            rhs: EarlyComptimeExpr {
                span: Span::new(4, 5),
                kind: EarlyComptimeExprKind::Integer("1".to_string()),
            },
        })),
    };
    let err = eval_early_comptime_expr(&expr, &mut EmptyEnv)
        .expect_err("assignment targets must carry resolved local ids");
    assert_eq!(
        err.message,
        "failed to resolve comptime assignment target `x`"
    );
}

#[test]
fn pattern_bindings_require_resolved_locals() {
    let mut env = EmptyEnv;
    let err = EarlyComptimeEnv::bind_pattern_local(
        &mut env,
        Span::new(0, 1),
        "payload",
        None,
        ComptimeValue::Int(IntConst::signed(1)),
    )
    .expect_err("pattern bindings must carry resolved local ids");
    assert_eq!(
        err.message,
        "failed to resolve comptime pattern local `payload`"
    );
}

#[test]
fn evaluates_lowered_switch_with_string_patterns() {
    let (module, errors) = nia_parser::parse_module(
        r#"
fn main() usize {
    switch "linux" {
        "linux" => 8,
        "windows" => 4,
        _ => 2,
    }
}
"#,
    );
    assert!(errors.is_empty(), "{errors:?}");
    let nia_ast::ItemKind::Function(function) = &module.items[0].kind else {
        panic!("expected function");
    };
    let expr = function.body.as_ref().unwrap().tail.as_deref().unwrap();
    let lowered = nia_comptime_ir::lower_expr_early(expr).unwrap();
    let value = eval_early_comptime_int_expr(&lowered, &mut EmptyEnv).unwrap();
    assert_eq!(value, IntConst::signed(8));
}

#[test]
fn evaluates_lowered_if_pattern_with_optional_payload_patterns() {
    let (module, errors) = nia_parser::parse_module(
        r#"
fn main() usize {
    if let ?value = ?8 {
        value
    } else null {
        0
    }
}
"#,
    );
    assert!(errors.is_empty(), "{errors:?}");
    let nia_ast::ItemKind::Function(function) = &module.items[0].kind else {
        panic!("expected function");
    };
    let expr = function.body.as_ref().unwrap().tail.as_deref().unwrap();
    let lowered = nia_comptime_ir::lower_expr_early(expr).unwrap();
    let value = eval_early_comptime_int_expr(&lowered, &mut PatternEnv::default()).unwrap();
    assert_eq!(value, IntConst::signed(8));
}

#[test]
fn evaluates_lowered_if_pattern_with_error_union_payload_patterns() {
    let (module, errors) = nia_parser::parse_module(
        r#"
fn main() usize {
    if let !value = 5! {
        value
    } else error! {
        error
    }
}
"#,
    );
    assert!(errors.is_empty(), "{errors:?}");
    let nia_ast::ItemKind::Function(function) = &module.items[0].kind else {
        panic!("expected function");
    };
    let expr = function.body.as_ref().unwrap().tail.as_deref().unwrap();
    let lowered = nia_comptime_ir::lower_expr_early(expr).unwrap();
    let value = eval_early_comptime_int_expr(&lowered, &mut PatternEnv::default()).unwrap();
    assert_eq!(value, IntConst::signed(5));
}

#[test]
fn evaluates_lowered_array_literals_and_indexes() {
    let (module, errors) = nia_parser::parse_module(
        r#"
fn main() usize {
    [2, 4, 8][1]
}
"#,
    );
    assert!(errors.is_empty(), "{errors:?}");
    let nia_ast::ItemKind::Function(function) = &module.items[0].kind else {
        panic!("expected function");
    };
    let expr = function.body.as_ref().unwrap().tail.as_deref().unwrap();
    let lowered = nia_comptime_ir::lower_expr_early(expr).unwrap();
    let value = eval_early_comptime_int_expr(&lowered, &mut EmptyEnv).unwrap();
    assert_eq!(value, IntConst::signed(4));
}

#[test]
fn evaluates_lowered_array_repeat_literals() {
    let (module, errors) = nia_parser::parse_module(
        r#"
fn main() bool {
    [7; 3] == [7, 7, 7]
}
"#,
    );
    assert!(errors.is_empty(), "{errors:?}");
    let nia_ast::ItemKind::Function(function) = &module.items[0].kind else {
        panic!("expected function");
    };
    let expr = function.body.as_ref().unwrap().tail.as_deref().unwrap();
    let lowered = nia_comptime_ir::lower_expr_early(expr).unwrap();
    let value = eval_early_comptime_bool_expr(&lowered, &mut EmptyEnv).unwrap();
    assert!(value);
}

struct BuiltinEnv;

impl ComptimeCommonEnv for BuiltinEnv {
    fn resolve_builtin_value(
        &mut self,
        span: Span,
        builtin_value: ValueBuiltin,
    ) -> Result<ComptimeValue, ComptimeError> {
        let _ = span;
        match builtin_value {
            ValueBuiltin::Builtin => {}
            ValueBuiltin::Error => {
                return Err(ComptimeError {
                    span,
                    message: "`@error` is not available in this test environment".to_string(),
                });
            }
        }
        let mut target = BTreeMap::new();
        target.insert("os".to_string(), ComptimeValue::String("linux".to_string()));
        target.insert(
            "pointer_width".to_string(),
            ComptimeValue::Int(IntConst::signed(64)),
        );
        let mut builtin = BTreeMap::new();
        builtin.insert("target".to_string(), ComptimeValue::Struct(target));
        Ok(ComptimeValue::Struct(builtin))
    }
}

impl EarlyComptimeEnv for BuiltinEnv {
    fn resolve_name(
        &mut self,
        span: Span,
        name: &EarlyComptimeName,
    ) -> Result<ComptimeValue, ComptimeError> {
        match name {
            EarlyComptimeName::Unresolved(display) => Err(ComptimeError {
                span,
                message: format!("unknown comptime value `{display}`"),
            }),
            EarlyComptimeName::Resolved { display, .. } => Err(ComptimeError {
                span,
                message: format!(
                    "resolved comptime value `{display}` is not available in this test"
                ),
            }),
        }
    }

    fn resolve_layout_builtin(
        &mut self,
        span: Span,
        _builtin: LayoutBuiltin,
        _type_arg: &EarlyComptimeTypeArg,
    ) -> Result<ComptimeValue, ComptimeError> {
        Err(ComptimeError {
            span,
            message: "layout builtins are not available in this test".to_string(),
        })
    }
}

#[derive(Default)]
struct PatternEnv {
    scopes: Vec<BTreeMap<String, ComptimeValue>>,
}

impl ComptimeCommonEnv for PatternEnv {
    fn push_comptime_scope(&mut self, _span: Span) -> Result<(), ComptimeError> {
        self.scopes.push(BTreeMap::new());
        Ok(())
    }

    fn pop_comptime_scope(&mut self) {
        self.scopes.pop();
    }
}

impl EarlyComptimeEnv for PatternEnv {
    fn resolve_name(
        &mut self,
        span: Span,
        name: &EarlyComptimeName,
    ) -> Result<ComptimeValue, ComptimeError> {
        let EarlyComptimeName::Unresolved(name) = name else {
            return Err(ComptimeError {
                span,
                message: format!(
                    "resolved comptime value `{}` is not available in this test",
                    name.display()
                ),
            });
        };
        self.scopes
            .iter()
            .rev()
            .find_map(|scope| scope.get(name).cloned())
            .ok_or_else(|| ComptimeError {
                span,
                message: format!("unknown comptime value `{name}`"),
            })
    }

    fn resolve_layout_builtin(
        &mut self,
        span: Span,
        _builtin: LayoutBuiltin,
        _type_arg: &EarlyComptimeTypeArg,
    ) -> Result<ComptimeValue, ComptimeError> {
        Err(ComptimeError {
            span,
            message: "layout builtins are not available in this test".to_string(),
        })
    }

    fn bind_pattern_local(
        &mut self,
        span: Span,
        name: &str,
        _local_id: Option<nia_ids::LocalId>,
        value: ComptimeValue,
    ) -> Result<(), ComptimeError> {
        let Some(scope) = self.scopes.last_mut() else {
            return Err(ComptimeError {
                span,
                message: "internal comptime switch pattern scope is missing".to_string(),
            });
        };
        scope.insert(name.to_string(), value);
        Ok(())
    }
}
