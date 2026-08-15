use super::*;

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
fn const_block_restores_scope_after_statement_error() {
    let (module, errors) = nia_parser::parse_module(
        r#"
fn main() usize {
    {
        missing;
        1
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
    let mut env = ScopeErrorEnv::default();

    let error = eval_early_const_int_expr(&lowered, &mut env)
        .expect_err("unknown statement value must fail const evaluation");

    assert!(error.message.starts_with("unknown const value `"));
    assert_eq!(env.scope_depth, 0, "failed block must restore its scope");
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
fn evaluates_lowered_if_pattern_with_optional_payload_patterns() {
    let (module, errors) = nia_parser::parse_module(
        r#"
fn main() usize {
    match ?8 {
        ?value => {
            value
        },
        null => {
            0
        },
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
    match 5! {
        !value => {
            value
        },
        error! => {
            error
        },
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
struct ScopeErrorEnv {
    scope_depth: usize,
}

impl ConstCommonEnv for ScopeErrorEnv {
    fn push_const_scope(&mut self, _span: Span) -> Result<(), ConstError> {
        self.scope_depth += 1;
        Ok(())
    }

    fn pop_const_scope(&mut self) {
        self.scope_depth -= 1;
    }
}

impl EarlyConstEnv for ScopeErrorEnv {
    fn resolve_name(
        &mut self,
        span: Span,
        name: &EarlyConstName,
    ) -> Result<ConstValue, ConstError> {
        Err(ConstError {
            span,
            message: format!("unknown const value `{}`", self.symbol_name(name.display())),
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
}

#[derive(Default)]
pub(super) struct PatternEnv {
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
                message: "internal const match pattern scope is missing".to_string(),
            });
        };
        scope.insert(*name, value);
        Ok(())
    }
}
