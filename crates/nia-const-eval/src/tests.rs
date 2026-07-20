use crate::{
    ConstCommonEnv, ConstError, ConstValue, EarlyConstEnv, EmptyEnv, ResolvedConstEnv,
    eval_early_const_bool_expr, eval_early_const_expr, eval_early_const_int_expr,
    eval_float_literal, eval_int_literal, eval_resolved_const_int_expr,
};
use nia_const_ir::{
    ConstAssignOp, ConstNameResolution, EarlyConstAssign, EarlyConstAssignTarget, EarlyConstExpr,
    EarlyConstExprKind, EarlyConstName, EarlyConstTypeArg, ResolvedConstExpr, ResolvedConstTypeArg,
};
use nia_ids::{LayoutBuiltin, ModuleId, ModuleIdAllocator, ValueBuiltin};
use nia_span::Span;
use nia_symbol::{SymbolId, stable_hash};
use nia_ty::IntConst;
use std::collections::BTreeMap;

fn sym(text: &str) -> SymbolId {
    SymbolId::from_stable_hash(stable_hash(text))
}

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
fn evaluates_struct_field_conditions() {
    let (module, errors) = nia_parser::parse_module(
        r#"
fn main() bool {
    config.target.os == "linux" and config.target.pointer_width == 64
}
"#,
    );
    assert!(errors.is_empty(), "{errors:?}");
    let nia_ast::ItemKind::Function(function) = &module.items[0].kind else {
        panic!("expected function");
    };
    let expr = function.body.as_ref().unwrap().tail.as_deref().unwrap();
    let expr = nia_const_ir::lower_expr_early(expr).unwrap();
    let value = eval_early_const_bool_expr(&expr, &mut ConfigEnv).unwrap();
    assert!(value);
}

#[test]
fn evaluates_lowered_const_expr_directly() {
    let (module, errors) = nia_parser::parse_module(
        r#"
fn main() bool {
    config.target.os == "linux"
}
"#,
    );
    assert!(errors.is_empty(), "{errors:?}");
    let nia_ast::ItemKind::Function(function) = &module.items[0].kind else {
        panic!("expected function");
    };
    let expr = function.body.as_ref().unwrap().tail.as_deref().unwrap();
    let lowered = nia_const_ir::lower_expr_early(expr).unwrap();
    let EarlyConstExprKind::Binary { lhs, .. } = &lowered.kind else {
        panic!("expected lowered binary expression");
    };
    let EarlyConstExprKind::Field { name, .. } = &lhs.kind else {
        panic!("expected lowered field expression");
    };
    assert_eq!(*name, sym("os"));

    let value = eval_early_const_bool_expr(&lowered, &mut ConfigEnv).unwrap();
    assert!(value);
}

#[test]
fn old_at_builtin_expr_syntax_is_rejected_by_parser() {
    let (_module, errors) = nia_parser::parse_module(
        r#"
fn main() bool {
    @unknown
}
"#,
    );
    assert!(
        errors
            .iter()
            .any(|error| error.message == "expected expression")
    );
}

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
    let lowered = nia_const_ir::lower_expr_early(expr).unwrap();
    let value = eval_early_const_int_expr(&lowered, &mut EmptyEnv).unwrap();
    assert_eq!(value, IntConst::signed(8));
}

#[test]
fn evaluates_lowered_if_pattern_with_optional_payload_patterns() {
    let (module, errors) = nia_parser::parse_module(
        r#"
fn main() usize {
    if ?value = ?8 {
        value
    } or null {
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
    let lowered = nia_const_ir::lower_expr_early(expr).unwrap();
    let value = eval_early_const_int_expr(&lowered, &mut PatternEnv::default()).unwrap();
    assert_eq!(value, IntConst::signed(8));
}

#[test]
fn evaluates_lowered_if_pattern_with_error_union_payload_patterns() {
    let (module, errors) = nia_parser::parse_module(
        r#"
fn main() usize {
    if !value = 5! {
        value
    } or error! {
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
    let lowered = nia_const_ir::lower_expr_early(expr).unwrap();
    let value = eval_early_const_int_expr(&lowered, &mut PatternEnv::default()).unwrap();
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
    let lowered = nia_const_ir::lower_expr_early(expr).unwrap();
    let value = eval_early_const_int_expr(&lowered, &mut EmptyEnv).unwrap();
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
    let lowered = nia_const_ir::lower_expr_early(expr).unwrap();
    let value = eval_early_const_bool_expr(&lowered, &mut EmptyEnv).unwrap();
    assert!(value);
}

struct ConfigEnv;

impl ConstCommonEnv for ConfigEnv {
    fn resolve_builtin_value(
        &mut self,
        span: Span,
        _builtin_value: ValueBuiltin,
    ) -> Result<ConstValue, ConstError> {
        Err(ConstError {
            span,
            message: "`error` is not available in this test environment".to_string(),
        })
    }
}

impl EarlyConstEnv for ConfigEnv {
    fn resolve_name(
        &mut self,
        span: Span,
        name: &EarlyConstName,
    ) -> Result<ConstValue, ConstError> {
        let EarlyConstName::Unresolved(name) = name else {
            return Err(ConstError {
                span,
                message: format!(
                    "resolved const value `{}` is not available in this test",
                    self.symbol_name(name.display())
                ),
            });
        };
        if *name != sym("config") {
            return Err(ConstError {
                span,
                message: format!("unknown const value `{}`", self.symbol_name(*name)),
            });
        }
        let mut target = BTreeMap::new();
        target.insert(sym("os"), ConstValue::String("linux".to_string()));
        target.insert(sym("pointer_width"), ConstValue::Int(IntConst::signed(64)));
        let mut builtin = BTreeMap::new();
        builtin.insert(sym("target"), ConstValue::Struct(target));
        Ok(ConstValue::Struct(builtin))
    }

    fn resolve_layout_builtin(
        &mut self,
        span: Span,
        _builtin: LayoutBuiltin,
        _type_arg: &EarlyConstTypeArg,
    ) -> Result<ConstValue, ConstError> {
        Err(ConstError {
            span,
            message: "layout builtins are not available in this test".to_string(),
        })
    }
}

#[derive(Default)]
struct PatternEnv {
    scopes: Vec<BTreeMap<SymbolId, ConstValue>>,
}

impl ConstCommonEnv for PatternEnv {
    fn push_const_scope(&mut self, _span: Span) -> Result<(), ConstError> {
        self.scopes.push(BTreeMap::new());
        Ok(())
    }

    fn pop_const_scope(&mut self) {
        self.scopes.pop();
    }
}

impl EarlyConstEnv for PatternEnv {
    fn resolve_name(
        &mut self,
        span: Span,
        name: &EarlyConstName,
    ) -> Result<ConstValue, ConstError> {
        let EarlyConstName::Unresolved(name) = name else {
            return Err(ConstError {
                span,
                message: format!(
                    "resolved const value `{}` is not available in this test",
                    self.symbol_name(name.display())
                ),
            });
        };
        self.scopes
            .iter()
            .rev()
            .find_map(|scope| scope.get(name).cloned())
            .ok_or_else(|| ConstError {
                span,
                message: format!("unknown const value `{}`", self.symbol_name(*name)),
            })
    }

    fn resolve_layout_builtin(
        &mut self,
        span: Span,
        _builtin: LayoutBuiltin,
        _type_arg: &EarlyConstTypeArg,
    ) -> Result<ConstValue, ConstError> {
        Err(ConstError {
            span,
            message: "layout builtins are not available in this test".to_string(),
        })
    }

    fn bind_pattern_local(
        &mut self,
        span: Span,
        name: &SymbolId,
        _local_id: Option<nia_ids::LocalId>,
        value: ConstValue,
    ) -> Result<(), ConstError> {
        let Some(scope) = self.scopes.last_mut() else {
            return Err(ConstError {
                span,
                message: "internal const switch pattern scope is missing".to_string(),
            });
        };
        scope.insert(*name, value);
        Ok(())
    }
}
