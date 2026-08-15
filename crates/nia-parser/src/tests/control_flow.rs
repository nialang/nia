// SPDX-License-Identifier: GPL-3.0-or-later
use super::common::*;

#[test]
fn parses_nested_tuple_binding_patterns_and_mutability() {
    let (module, errors) = parse_module(
        r#"
fn main(pair: (i32, (bool, i32))) {
    let mut (x, (flag, y)) = pair;
    let (mut selected, fixed) = (1, 2);
    let () = ();
}
"#,
    );
    assert!(errors.is_empty(), "{errors:?}");
    let ItemKind::Function(function) = &module.items[0].kind else {
        panic!("expected function");
    };
    let body = function.body.as_ref().expect("expected body");

    let StmtKind::Binding(all_mutable) = &body.stmts[0].kind else {
        panic!("expected binding");
    };
    let PatternKind::Tuple(fields) = &all_mutable.pattern.kind else {
        panic!("expected tuple pattern");
    };
    assert!(matches!(
        fields[0].kind,
        PatternKind::Bind {
            is_mutable: true,
            ..
        }
    ));
    let PatternKind::Tuple(nested) = &fields[1].kind else {
        panic!("expected nested tuple pattern");
    };
    assert!(nested.iter().all(|pattern| matches!(
        pattern.kind,
        PatternKind::Bind {
            is_mutable: true,
            ..
        }
    )));

    let StmtKind::Binding(selective) = &body.stmts[1].kind else {
        panic!("expected binding");
    };
    let PatternKind::Tuple(fields) = &selective.pattern.kind else {
        panic!("expected tuple pattern");
    };
    assert!(matches!(
        fields[0].kind,
        PatternKind::Bind {
            is_mutable: true,
            ..
        }
    ));
    assert!(matches!(
        fields[1].kind,
        PatternKind::Bind {
            is_mutable: false,
            ..
        }
    ));
    let StmtKind::Binding(unit) = &body.stmts[2].kind else {
        panic!("expected binding");
    };
    assert!(matches!(
        &unit.pattern.kind,
        PatternKind::Tuple(fields) if fields.is_empty()
    ));
}

#[test]
fn parses_union_items() {
    let (module, errors) = parse_module(
        r#"
pub extern union Bits[T] {
    i: i64,
    value: T,
}
"#,
    );
    assert!(errors.is_empty(), "{errors:?}");
    let ItemKind::Union(item) = &module.items[0].kind else {
        panic!("expected union item");
    };
    assert_eq!(item.name, sym("Bits"));
    assert_eq!(nia_ast::generic_param_names(&item.generics), [sym("T")]);
    assert_eq!(item.fields.len(), 2);
    assert!(item.is_extern);
}

#[test]
fn parses_for_in_binding_with_range_iterator() {
    let (module, errors) = parse_module(
        r#"
fn main() {
    for i in 0i32..10i32 {
        _ = i;
    }
}
"#,
    );
    assert!(errors.is_empty(), "{errors:?}");
    let ItemKind::Function(function) = &module.items[0].kind else {
        panic!("expected function");
    };
    let body = function.body.as_ref().expect("expected body");
    let StmtKind::ForIn(for_stmt) = &body.stmts[0].kind else {
        panic!("expected for-in statement");
    };
    assert_eq!(bind_pattern_name(&for_stmt.pattern), Some(sym("i")));
    assert!(matches!(
        for_stmt.pattern.kind,
        PatternKind::Bind {
            is_mutable: false,
            ..
        }
    ));
    assert!(matches!(for_stmt.iter.kind, ExprKind::Range(_)));
}

#[test]
fn parses_for_in_pointer_patterns() {
    let (module, errors) = parse_module(
        r#"
fn main(xs: &[&i32], ys: &[&mut i32]) {
    for &x in xs {}
    for &mut y in ys {}
    for _ in 0..3 {}
}
"#,
    );
    assert!(errors.is_empty(), "{errors:?}");
    let ItemKind::Function(function) = &module.items[0].kind else {
        panic!("expected function");
    };
    let body = function.body.as_ref().expect("expected body");
    let StmtKind::ForIn(first) = &body.stmts[0].kind else {
        panic!("expected for-in statement");
    };
    assert_eq!(bind_pattern_name(&first.pattern), Some(sym("x")));
    assert!(matches!(first.pattern.kind, PatternKind::Pointer(_)));
    let StmtKind::ForIn(second) = &body.stmts[1].kind else {
        panic!("expected for-in statement");
    };
    assert_eq!(bind_pattern_name(&second.pattern), Some(sym("y")));
    assert!(matches!(second.pattern.kind, PatternKind::MutPointer(_)));
    let StmtKind::ForIn(third) = &body.stmts[2].kind else {
        panic!("expected for-in statement");
    };
    assert_eq!(bind_pattern_name(&third.pattern), None);
    assert!(matches!(third.pattern.kind, PatternKind::Wildcard));
}

#[test]
fn parses_local_binding_pointer_patterns() {
    let (module, errors) = parse_module(
        r#"
fn main(ptr: &i32, mut_ptr: &mut i32) {
    let &x = ptr;
    let &mut y: &mut i32 = mut_ptr;
}
"#,
    );
    assert!(errors.is_empty(), "{errors:?}");
    let ItemKind::Function(function) = &module.items[0].kind else {
        panic!("expected function");
    };
    let body = function.body.as_ref().expect("expected body");
    let StmtKind::Binding(first) = &body.stmts[0].kind else {
        panic!("expected binding");
    };
    assert_eq!(bind_pattern_name(&first.pattern), Some(sym("x")));
    assert!(matches!(first.pattern.kind, PatternKind::Pointer(_)));
    let StmtKind::Binding(second) = &body.stmts[1].kind else {
        panic!("expected binding");
    };
    assert_eq!(bind_pattern_name(&second.pattern), Some(sym("y")));
    assert!(matches!(second.pattern.kind, PatternKind::MutPointer(_)));
}

#[test]
fn rejects_local_const_mut_binding() {
    let (_module, errors) = parse_module(
        r#"
const fn width() usize {
    const mut value: usize = 1usize;
    value
}
"#,
    );
    assert!(
        errors
            .iter()
            .any(|error| error.message.contains("const bindings cannot be mutable")),
        "{errors:?}"
    );
}

#[test]
fn rejects_local_const_let_binding() {
    let (_module, errors) = parse_module(
        r#"
const fn width() usize {
    const let value: usize = 1usize;
    value
}
"#,
    );
    assert!(
        errors
            .iter()
            .any(|error| error.message.contains("expected const binding")),
        "{errors:?}"
    );
}

#[test]
fn parses_conditional_item_and_statement_attributes() {
    let (module, errors) = parse_module(
        r#"
@[if os == "linux" and arch == "x86_64"]
fn selected() i32 { 1 }

fn main() i32 {
    @[if pointer_width == 64]
    let value = 1;
    @[if os == "linux"]
    _ = value;
    value
}
"#,
    );
    assert!(errors.is_empty(), "{errors:?}");
    assert!(matches!(
        module.items[0].attributes[0].kind,
        AttributeKind::If(_)
    ));
    let ItemKind::Function(function) = &module.items[1].kind else {
        panic!("expected function");
    };
    let body = function.body.as_ref().expect("expected body");
    assert!(matches!(
        body.stmts[0].attributes[0].kind,
        AttributeKind::If(_)
    ));
    assert!(matches!(
        body.stmts[1].attributes[0].kind,
        AttributeKind::If(_)
    ));
}

#[test]
fn control_flow_statement_boundary_stops_binary_expr_across_newline() {
    let (module, errors) = parse_module(
        r#"
fn tail_after_if(bytes: &[u8], start: usize) &[u8] {
    if start == 0usize {
    }
    &bytes[start..bytes.len()]
}

fn tail_after_if_let(bytes: &[u8], maybe: ?usize) &[u8] {
    if maybe is ?start {
        _ = start;
    }
    &bytes[..]
}

fn parenthesized_if_can_still_be_binary(flag: bool, mask: bool) bool {
    (if flag {
        true
    } else {
        false
    }) & mask
}
"#,
    );
    assert!(errors.is_empty(), "{errors:?}");

    let ItemKind::Function(first) = &module.items[0].kind else {
        panic!("expected function");
    };
    let first_body = first.body.as_ref().expect("expected body");
    assert_eq!(first_body.stmts.len(), 1);
    assert!(matches!(
        first_body.stmts[0].kind,
        StmtKind::Expr(ref expr) if matches!(expr.kind, ExprKind::If { .. })
    ));
    let first_tail = first_body.tail.as_ref().expect("expected tail");
    assert!(matches!(first_tail.kind, ExprKind::Unary { .. }));

    let ItemKind::Function(second) = &module.items[1].kind else {
        panic!("expected function");
    };
    let second_body = second.body.as_ref().expect("expected body");
    assert_eq!(second_body.stmts.len(), 1);
    assert!(matches!(
        second_body.stmts[0].kind,
        StmtKind::Expr(ref expr) if matches!(expr.kind, ExprKind::IfPattern(_))
    ));
    let second_tail = second_body.tail.as_ref().expect("expected tail");
    assert!(matches!(second_tail.kind, ExprKind::Unary { .. }));

    let ItemKind::Function(third) = &module.items[2].kind else {
        panic!("expected function");
    };
    let third_body = third.body.as_ref().expect("expected body");
    let third_tail = third_body.tail.as_ref().expect("expected tail");
    assert!(matches!(third_tail.kind, ExprKind::Binary { .. }));
}

#[test]
fn rejects_let_keyword_in_for_in_binding() {
    let (_module, errors) = parse_module(
        r#"
fn main() {
    for let mut i in 0..10 {}
}
"#,
    );
    assert!(
        errors
            .iter()
            .any(|error| error.message.contains("expected binding pattern")),
        "{errors:?}"
    );
}

#[test]
fn rejects_for_in_binding_type_annotation() {
    let (_module, errors) = parse_module(
        r#"
fn main() {
    for i: usize in 0..10 {}
}
"#,
    );
    assert!(
        errors
            .iter()
            .any(|error| error.message.contains("do not support type annotations")),
        "{errors:?}"
    );
}

#[test]
fn parses_defer_with_complex_expression_forms() {
    let (module, errors) = parse_module(
        r#"
fn cleanup() {}

fn main(flag: bool) {
    defer cleanup();
    defer if flag {
        cleanup();
    } else {
        cleanup();
    };
    defer {
        let mut state = 1;
        match state {
            0 => cleanup(),
            _ => cleanup(),
        }
    };
}
"#,
    );
    assert!(errors.is_empty(), "{errors:?}");
    let ItemKind::Function(function) = &module.items[1].kind else {
        panic!("expected function");
    };
    let body = function.body.as_ref().expect("expected body");
    assert_eq!(
        body.stmts
            .iter()
            .filter(|stmt| matches!(stmt.kind, StmtKind::Defer(_)))
            .count(),
        3
    );
    assert!(matches!(
        &body.stmts[1].kind,
        StmtKind::Defer(expr) if matches!(expr.kind, ExprKind::If { .. })
    ));
    assert!(matches!(
        &body.stmts[2].kind,
        StmtKind::Defer(expr) if matches!(expr.kind, ExprKind::Block(_))
    ));
}

#[test]
fn parses_match_arm_expression_statement_and_block_bodies() {
    let (module, errors) = parse_module(
        r#"
fn cleanup() {}

fn main(state: i32) i32 {
    loop {
        match state {
            0 => cleanup(),
            1 => defer cleanup(),
            2 => {
                defer cleanup();
                20
            },
            _ => break,
        }
    }
    0
}
"#,
    );
    assert!(errors.is_empty(), "{errors:?}");
    let ItemKind::Function(function) = &module.items[1].kind else {
        panic!("expected function");
    };
    let body = function.body.as_ref().expect("expected body");
    let StmtKind::Loop(loop_stmt) = &body.stmts[0].kind else {
        panic!("expected loop statement");
    };
    let expr = loop_stmt.body.tail.as_ref().expect("expected match tail");
    let ExprKind::Match(matched) = &expr.kind else {
        panic!("expected match expression");
    };
    assert!(matches!(matched.arms[0].body, MatchArmBody::Expr(_)));
    assert!(matches!(
        &matched.arms[1].body,
        MatchArmBody::Stmt(stmt) if matches!(stmt.kind, StmtKind::Defer(_))
    ));
    assert!(matches!(matched.arms[2].body, MatchArmBody::Block(_)));
    assert!(matches!(
        &matched.arms[3].body,
        MatchArmBody::Stmt(stmt) if matches!(stmt.kind, StmtKind::Break)
    ));
}

#[test]
fn parses_if_is_and_recursive_match_patterns() {
    let (module, errors) = parse_module(
        r#"
fn main(result: i32!i32, nested: ?(i32!i32), value: i32) i32 {
    let a = match result {
        !ok => {
            ok
        },
        err! => {
            err
        },
    };
    let b = if value is mut x {
        x
    } else {
        0
    };
    match nested {
        ?5! => {
            5
        },
        ?err! => {
            err
        },
        ?!ok => {
            ok
        },
        null => {
            a + b
        },
    }
}
"#,
    );
    assert!(errors.is_empty(), "{errors:?}");
    let ItemKind::Function(function) = &module.items[0].kind else {
        panic!("expected function");
    };
    let body = function.body.as_ref().expect("expected body");

    let StmtKind::Binding(first) = &body.stmts[0].kind else {
        panic!("expected first binding");
    };
    let ExprKind::Match(matched) = &first.value.as_ref().expect("expected value").kind else {
        panic!("expected match expression");
    };
    assert_eq!(matched.arms.len(), 2);
    assert!(matches!(
        &matched.arms[0].patterns[0].kind,
        PatternKind::ErrorOk(inner)
            if matches!(&inner.kind, PatternKind::Bind { name, .. } if *name == sym("ok"))
    ));
    assert!(matches!(
        &matched.arms[1].patterns[0].kind,
        PatternKind::ErrorErr(inner)
            if matches!(&inner.kind, PatternKind::Bind { name, .. } if *name == sym("err"))
    ));

    let StmtKind::Binding(second) = &body.stmts[1].kind else {
        panic!("expected second binding");
    };
    let ExprKind::IfPattern(if_pattern) = &second.value.as_ref().expect("expected value").kind
    else {
        panic!("expected if-pattern expression");
    };
    assert!(matches!(
        &if_pattern.pattern.kind,
        PatternKind::Bind {
            name,
            is_mutable: true,
            ..
        } if *name == sym("x")
    ));
    assert!(if_pattern.else_branch.is_some());

    let ExprKind::Match(matched) = &body.tail.as_ref().expect("expected tail").kind else {
        panic!("expected match tail");
    };
    assert!(matches!(
        &matched.arms[0].patterns[0].kind,
        PatternKind::OptionalSome(inner)
            if matches!(
                &inner.kind,
                PatternKind::ErrorErr(payload)
                    if matches!(&payload.kind, PatternKind::Expr(_))
            )
    ));
    assert!(matches!(
        &matched.arms[1].patterns[0].kind,
        PatternKind::OptionalSome(inner)
            if matches!(
                &inner.kind,
                PatternKind::ErrorErr(payload)
                    if matches!(&payload.kind, PatternKind::Bind { name, .. } if *name == sym("err"))
            )
    ));
    assert!(matches!(
        &matched.arms[2].patterns[0].kind,
        PatternKind::OptionalSome(inner)
            if matches!(
                &inner.kind,
                PatternKind::ErrorOk(payload)
                    if matches!(&payload.kind, PatternKind::Bind { name, .. } if *name == sym("ok"))
            )
    ));
}

#[test]
fn parses_nominal_patterns_with_shorthand_and_renaming() {
    let (module, errors) = parse_module(
        r#"
struct Point { x: i32, y: i32 }
enum Event { Resize { width: i32, height: i32 } }

fn inspect(point: Point, event: Event) i32 {
    let Point { y: second, x } = point;
    match event {
        Event::Resize { width, height: h } => x + second + width + h,
    }
}
"#,
    );
    assert!(errors.is_empty(), "{errors:?}");
    let ItemKind::Function(function) = &module.items[2].kind else {
        panic!("expected function");
    };
    let body = function.body.as_ref().expect("expected body");
    let StmtKind::Binding(binding) = &body.stmts[0].kind else {
        panic!("expected binding");
    };
    let PatternKind::Nominal {
        constructor,
        fields: nia_ast::NominalPatternFields::Named { fields, rest: None },
    } = &binding.pattern.kind
    else {
        panic!("expected struct pattern");
    };
    assert!(matches!(constructor.kind, ExprKind::Ident(name) if name == sym("Point")));
    assert_eq!(fields.len(), 2);
    assert_eq!(fields[0].name, sym("y"));
    assert!(matches!(
        fields[0].pattern.kind,
        PatternKind::Bind { name, .. } if name == sym("second")
    ));
    assert_eq!(fields[1].name, sym("x"));
    assert!(matches!(
        fields[1].pattern.kind,
        PatternKind::Bind { name, .. } if name == sym("x")
    ));
}

#[test]
fn parses_terminal_nominal_pattern_rest() {
    let (module, errors) = parse_module(
        r#"
struct Point { x: i32, y: i32 }
enum Event { Resize { width: i32, height: i32 } }

fn inspect(point: Point, event: Event) i32 {
    let Point { x, .. } = point;
    match event {
        Event::Resize { .. } => x,
    }
}
"#,
    );
    assert!(errors.is_empty(), "{errors:?}");
    let ItemKind::Function(function) = &module.items[2].kind else {
        panic!("expected function");
    };
    let body = function.body.as_ref().expect("expected body");
    let StmtKind::Binding(binding) = &body.stmts[0].kind else {
        panic!("expected binding");
    };
    assert!(matches!(
        &binding.pattern.kind,
        PatternKind::Nominal {
            fields: nia_ast::NominalPatternFields::Named {
                fields,
                rest: Some(_),
            },
            ..
        } if fields.len() == 1 && fields[0].name == sym("x")
    ));
}

#[test]
fn diagnoses_duplicate_and_non_terminal_nominal_pattern_rest() {
    let (_, errors) = parse_module(
        r#"
struct Point { x: i32, y: i32 }

fn duplicate(point: Point) {
    let Point { .., .. } = point;
}

fn non_terminal(point: Point) {
    let Point { .., x } = point;
}
"#,
    );
    assert!(
        errors
            .iter()
            .any(|error| error.message.contains("may contain `..` only once")),
        "{errors:?}"
    );
    assert!(
        errors.iter().any(|error| error
            .message
            .contains("must be the final nominal pattern field")),
        "{errors:?}"
    );
}

#[test]
fn parses_named_nominal_pattern_inside_error_payload() {
    let (module, errors) = parse_module(
        r#"
enum Operation { Read, Write }
enum Failure { System { operation: Operation, code: i32 } }

fn inspect(value: Failure!i32) i32 {
    match value {
        Failure::System { operation: Operation::Read, code: _ }! => 1,
        _ => 0,
    }
}
"#,
    );
    assert!(errors.is_empty(), "{errors:?}");
    let ItemKind::Function(function) = &module.items[2].kind else {
        panic!("expected function");
    };
    let body = function.body.as_ref().expect("expected body");
    let ExprKind::Match(matched) = &body.tail.as_ref().expect("expected tail").kind else {
        panic!("expected match expression");
    };
    assert!(matches!(
        &matched.arms[0].patterns[0].kind,
        PatternKind::ErrorErr(inner)
            if matches!(
                &inner.kind,
                PatternKind::Nominal {
                    fields: nia_ast::NominalPatternFields::Named { fields, rest: None },
                    ..
                } if matches!(
                    &fields[0].pattern.kind,
                    PatternKind::Expr(expr)
                        if matches!(expr.kind, ExprKind::Qualified { .. })
                )
            )
    ));
}

#[test]
fn does_not_consume_if_body_as_unqualified_nominal_pattern_before_fallible_tail() {
    let (_, errors) = parse_module(
        r#"
fn inspect(value: ?i32) i32 {
    if value is ?error {
        return error!;
    }
    !()
}
"#,
    );
    assert!(errors.is_empty(), "{errors:?}");
}

#[test]
fn parses_match_arm_pattern_lists_and_ranges() {
    let (module, errors) = parse_module(
        r#"
fn main(state: i32) i32 {
    match state {
        0, 1 => 10,
        2..5 => 20,
        5..=7 => 30,
        _ => 40,
    }
}
"#,
    );
    assert!(errors.is_empty(), "{errors:?}");
    let ItemKind::Function(function) = &module.items[0].kind else {
        panic!("expected function");
    };
    let body = function.body.as_ref().expect("expected body");
    let expr = body.tail.as_ref().expect("expected match tail");
    let ExprKind::Match(matched) = &expr.kind else {
        panic!("expected match expression");
    };
    assert_eq!(matched.arms[0].patterns.len(), 2);
    assert!(matches!(
        &matched.arms[0].patterns[0].kind,
        PatternKind::Expr(_)
    ));
    assert!(matches!(
        &matched.arms[0].patterns[1].kind,
        PatternKind::Expr(_)
    ));
    assert!(matches!(
        &matched.arms[1].patterns[0].kind,
        PatternKind::Range {
            inclusive: false,
            ..
        }
    ));
    assert!(matches!(
        &matched.arms[2].patterns[0].kind,
        PatternKind::Range {
            inclusive: true,
            ..
        }
    ));
    assert!(matches!(
        &matched.arms[3].patterns[0].kind,
        PatternKind::Wildcard
    ));
}

#[test]
fn parses_match_destructuring_bindings_and_explicit_value_patterns() {
    let (module, errors) = parse_module(
        r#"
fn main(value: ?i32, tag: i32) i32 {
    match value {
        ?payload => payload,
        null => match tag {
            (expected) => 1,
            _ => 0,
        },
    }
}
"#,
    );
    assert!(errors.is_empty(), "{errors:?}");
    let ItemKind::Function(function) = &module.items[0].kind else {
        panic!("expected function");
    };
    let body = function.body.as_ref().expect("expected body");
    let ExprKind::Match(matched) = &body.tail.as_ref().expect("expected match tail").kind else {
        panic!("expected match expression");
    };
    assert!(matches!(
        &matched.arms[0].patterns[0].kind,
        PatternKind::OptionalSome(inner)
            if matches!(&inner.kind, PatternKind::Bind { name, .. } if *name == sym("payload"))
    ));
    assert!(matches!(
        matched.arms[1].patterns[0].kind,
        PatternKind::OptionalNull
    ));
    let MatchArmBody::Expr(nested) = &matched.arms[1].body else {
        panic!("expected nested match expression");
    };
    let ExprKind::Match(nested) = &nested.kind else {
        panic!("expected nested match");
    };
    assert!(matches!(
        nested.arms[0].patterns[0].kind,
        PatternKind::Expr(_)
    ));
}

#[test]
fn rejects_open_ended_match_range_patterns() {
    let (_module, errors) = parse_module(
        r#"
fn main(state: i32) i32 {
    match state {
        1.. => 10,
        _ => 20,
    }
}
"#,
    );
    assert!(
        errors.iter().any(|error| error
            .message
            .contains("open-ended match range patterns are not supported")),
        "{errors:?}"
    );
}
