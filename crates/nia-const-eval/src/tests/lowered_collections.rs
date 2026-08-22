use super::test_environments::PatternEnv;
use super::*;
use crate::{ResolvedConstIterator, eval_resolved_const_expr};
use nia_const_ir::{
    ResolvedConstArrayElements, ResolvedConstBlock, ResolvedConstExprKind, ResolvedConstForIn,
    ResolvedConstPattern, ResolvedConstStmt, ResolvedConstStmtKind,
};
use nia_ids::LocalId;
use nia_ty::TyKind;

#[test]
fn evaluates_lowered_match_with_string_patterns() {
    let (module, errors) = nia_parser::parse_module(
        r#"
fn main() usize {
    match "linux" {
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
fn evaluates_lowered_match_with_destructured_payloads() {
    let (module, errors) = nia_parser::parse_module(
        r#"
fn main() usize {
    match ?5! {
        ?!value => value,
        ?error! => error,
        null => 0,
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

#[test]
fn early_const_for_in_reports_witness_dispatch_boundary() {
    let (module, errors) = nia_parser::parse_module(
        r#"
const fn iterate() usize {
    for item in [1, 2] {
        item;
    }
    0
}
"#,
    );
    assert!(errors.is_empty(), "{errors:?}");
    let nia_ast::ItemKind::Function(function) = &module.items[0].kind else {
        panic!("expected function");
    };
    let lowered = nia_const_ir::lower_function_early(module.items[0].span, function).unwrap();
    let module_id = ModuleIdAllocator::new().allocate();
    let error = crate::eval_early_const_function_call(
        module.items[0].span,
        module_id,
        &lowered,
        Vec::new(),
        Vec::new(),
        &mut PatternEnv::default(),
    )
    .expect_err("early const for-in must stop before witness dispatch");

    assert_eq!(
        error.message,
        "const for-in Iterator execution is not implemented yet"
    );
}

#[test]
fn resolved_const_for_in_threads_iterator_state_and_restores_item_scopes() {
    let (expr, iterator_ty) = resolved_for_in_expr(false);
    let mut env = IteratorEnv::new(iterator_ty);

    let value = eval_resolved_const_expr(&expr, &mut env).unwrap();

    assert_eq!(value, ConstValue::Int(IntConst::signed(0)));
    assert_eq!(
        env.bound_values,
        vec![IntConst::signed(1), IntConst::signed(2)]
    );
    assert_eq!(env.next_calls, 3, "iterator termination must be observed");
    assert_eq!(env.scope_depth, 0, "all loop scopes must be restored");
}

#[test]
fn resolved_const_for_in_restores_item_scope_after_binding_error() {
    let (expr, iterator_ty) = resolved_for_in_expr(true);
    let mut env = IteratorEnv::new(iterator_ty);

    let error = eval_resolved_const_expr(&expr, &mut env)
        .expect_err("pattern binding failure must stop resolved iteration");

    assert_eq!(error.message, "intentional iterator binding failure");
    assert_eq!(env.next_calls, 1);
    assert_eq!(env.scope_depth, 0, "failed item scope must be restored");
}

fn resolved_for_in_expr(fail_binding: bool) -> (ResolvedConstExpr, nia_ids::InternedTyId) {
    let span = Span::new(0, 1);
    let module_id = ModuleIdAllocator::new().allocate();
    let type_store = TypeStore::new();
    let iterator_ty = type_store
        .append_for_module(module_id)
        .intern(TyKind::Primitive(PrimitiveTy::Usize));
    let integer = |value: u128| {
        ResolvedConstExpr::from_parts(span, ResolvedConstExprKind::Integer(value.to_string()))
    };
    let iter = ResolvedConstExpr::from_parts(
        span,
        ResolvedConstExprKind::ArrayLiteral {
            elems: ResolvedConstArrayElements::list(vec![integer(1), integer(2)]),
        },
    );
    let pattern = ResolvedConstPattern::bind(
        if fail_binding {
            sym("fail")
        } else {
            sym("item")
        },
        LocalId(0),
        span,
    );
    let for_in = ResolvedConstStmt::new(
        span,
        ResolvedConstStmtKind::ForIn(ResolvedConstForIn::new(
            pattern,
            iter,
            ResolvedConstBlock::new(span, Vec::new(), None),
        )),
    );
    let block = ResolvedConstBlock::new(span, vec![for_in], Some(Box::new(integer(0))));
    (
        ResolvedConstExpr::from_parts(span, ResolvedConstExprKind::Block(block)),
        iterator_ty,
    )
}

struct IteratorEnv {
    iterator_ty: nia_ids::InternedTyId,
    scope_depth: usize,
    next_calls: usize,
    bound_values: Vec<IntConst>,
}

impl IteratorEnv {
    fn new(iterator_ty: nia_ids::InternedTyId) -> Self {
        Self {
            iterator_ty,
            scope_depth: 0,
            next_calls: 0,
            bound_values: Vec::new(),
        }
    }
}

impl ConstCommonEnv for IteratorEnv {
    fn push_const_scope(&mut self, _span: Span) -> Result<(), ConstError> {
        self.scope_depth += 1;
        Ok(())
    }

    fn pop_const_scope(&mut self) {
        self.scope_depth -= 1;
    }
}

impl ResolvedConstEnv for IteratorEnv {
    fn resolve_resolved_name(
        &mut self,
        span: Span,
        _resolution: ConstNameResolution,
    ) -> Result<ConstValue, ConstError> {
        Err(ConstError {
            span,
            message: "unexpected resolved name lookup".to_string(),
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
            message: "unexpected resolved layout lookup".to_string(),
        })
    }

    fn resolved_for_iterator(
        &mut self,
        span: Span,
        _iterable: &ResolvedConstExpr,
        value: ConstValue,
    ) -> Result<ResolvedConstIterator, ConstError> {
        if !matches!(value, ConstValue::Array(_)) {
            return Err(ConstError {
                span,
                message: "expected array iterator input".to_string(),
            });
        }
        Ok(ResolvedConstIterator {
            ty: self.iterator_ty,
            value,
        })
    }

    fn resolved_iterator_next(
        &mut self,
        span: Span,
        mut iterator: ResolvedConstIterator,
    ) -> Result<(ResolvedConstIterator, ConstValue), ConstError> {
        self.next_calls += 1;
        let ConstValue::Array(values) = &mut iterator.value else {
            return Err(ConstError {
                span,
                message: "expected array iterator state".to_string(),
            });
        };
        let next = (!values.is_empty()).then(|| Box::new(values.remove(0)));
        Ok((iterator, ConstValue::Optional(next)))
    }

    fn bind_resolved_pattern_local(
        &mut self,
        span: Span,
        name: &SymbolId,
        _local_id: LocalId,
        value: ConstValue,
    ) -> Result<(), ConstError> {
        assert_eq!(self.scope_depth, 2, "item binding requires a fresh scope");
        if *name == sym("fail") {
            return Err(ConstError {
                span,
                message: "intentional iterator binding failure".to_string(),
            });
        }
        let ConstValue::Int(value) = value else {
            return Err(ConstError {
                span,
                message: "expected integer iterator item".to_string(),
            });
        };
        self.bound_values.push(value);
        Ok(())
    }
}
