use super::test_environments::PatternEnv;
use super::*;

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
