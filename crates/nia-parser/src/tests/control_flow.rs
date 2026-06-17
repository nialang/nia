// SPDX-License-Identifier: GPL-3.0-or-later
use super::common::*;

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
    assert_eq!(item.name, "Bits");
    assert_eq!(item.generics, ["T"]);
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
    assert_eq!(for_stmt.pattern.name(), Some("i"));
    assert_eq!(for_stmt.pattern.kind, BindingPatternKind::Value);
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
    assert_eq!(first.pattern.name(), Some("x"));
    assert_eq!(first.pattern.kind, BindingPatternKind::Pointer);
    let StmtKind::ForIn(second) = &body.stmts[1].kind else {
        panic!("expected for-in statement");
    };
    assert_eq!(second.pattern.name(), Some("y"));
    assert_eq!(second.pattern.kind, BindingPatternKind::MutPointer);
    let StmtKind::ForIn(third) = &body.stmts[2].kind else {
        panic!("expected for-in statement");
    };
    assert_eq!(third.pattern.name(), None);
    assert_eq!(third.pattern.kind, BindingPatternKind::Value);
}

#[test]
fn parses_local_binding_pointer_patterns() {
    let (module, errors) = parse_module(
        r#"
fn main(ptr: &i32, mut_ptr: &mut i32) {
    let &x = ptr;
    var &mut y: i32 = mut_ptr;
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
    assert_eq!(first.name, "x");
    assert_eq!(first.pattern_kind, BindingPatternKind::Pointer);
    let StmtKind::Binding(second) = &body.stmts[1].kind else {
        panic!("expected binding");
    };
    assert_eq!(second.name, "y");
    assert_eq!(second.pattern_kind, BindingPatternKind::MutPointer);
}

#[test]
fn parses_comptime_if_items_and_expressions() {
    let (module, errors) = parse_module(
        r#"
comptime if true {
    fn selected() i32 { 1 }
} else {
    fn skipped() i32 { 2 }
}

fn main() i32 {
    comptime if true {
        1
    } else {
        missing_name
    }
}
"#,
    );
    assert!(errors.is_empty(), "{errors:?}");
    assert!(matches!(module.items[0].kind, ItemKind::ComptimeIf(_)));
    let ItemKind::Function(function) = &module.items[1].kind else {
        panic!("expected function");
    };
    let body = function.body.as_ref().expect("expected body");
    let tail = body.tail.as_ref().expect("expected tail");
    assert!(matches!(tail.kind, ExprKind::ComptimeIf(_)));
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
    if let ?start = maybe {
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
fn rejects_var_in_for_in_binding() {
    let (_module, errors) = parse_module(
        r#"
fn main() {
    for var i in 0..10 {}
}
"#,
    );
    assert!(
        errors
            .iter()
            .any(|error| error.message.contains("do not use `let` or `var`")),
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
        var state = 1;
        switch state {
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
fn parses_switch_arm_expression_statement_and_block_bodies() {
    let (module, errors) = parse_module(
        r#"
fn cleanup() {}

fn main(state: i32) i32 {
    loop {
        switch state {
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
    let expr = loop_stmt.body.tail.as_ref().expect("expected switch tail");
    let ExprKind::Switch(switch) = &expr.kind else {
        panic!("expected switch expression");
    };
    assert!(matches!(switch.arms[0].body, SwitchArmBody::Expr(_)));
    assert!(matches!(
        &switch.arms[1].body,
        SwitchArmBody::Stmt(stmt) if matches!(stmt.kind, StmtKind::Defer(_))
    ));
    assert!(matches!(switch.arms[2].body, SwitchArmBody::Block(_)));
    assert!(matches!(
        &switch.arms[3].body,
        SwitchArmBody::Stmt(stmt) if matches!(stmt.kind, StmtKind::Break)
    ));
}

#[test]
fn parses_if_pattern_arms_and_recursive_payload_patterns() {
    let (module, errors) = parse_module(
        r#"
fn main(result: i32!i32, nested: ?(i32!i32), value: i32) i32 {
    let a = if let !ok = result {
        ok
    } else err! {
        err
    };
    let b = if var x = value {
        x
    } else {
        0
    };
    if let ?5! = nested {
        5
    } else ?err! {
        err
    } else ?!ok {
        ok
    } else null {
        a + b
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
    let ExprKind::IfPattern(if_pattern) = &first.value.as_ref().expect("expected value").kind
    else {
        panic!("expected if-pattern expression");
    };
    assert_eq!(if_pattern.binding_mode, PatternBindingMode::Let);
    assert_eq!(if_pattern.arms.len(), 2);
    assert!(matches!(
        &if_pattern.arms[0].pattern.kind,
        PatternKind::ErrorOk(inner)
            if matches!(&inner.kind, PatternKind::Bind { name, .. } if name == "ok")
    ));
    assert!(matches!(
        &if_pattern.arms[1].pattern.kind,
        PatternKind::ErrorErr(inner)
            if matches!(&inner.kind, PatternKind::Bind { name, .. } if name == "err")
    ));

    let StmtKind::Binding(second) = &body.stmts[1].kind else {
        panic!("expected second binding");
    };
    let ExprKind::IfPattern(if_pattern) = &second.value.as_ref().expect("expected value").kind
    else {
        panic!("expected if-pattern expression");
    };
    assert_eq!(if_pattern.binding_mode, PatternBindingMode::Var);
    assert!(matches!(
        &if_pattern.arms[0].pattern.kind,
        PatternKind::Bind { name, .. } if name == "x"
    ));
    assert!(if_pattern.else_branch.is_some());

    let ExprKind::IfPattern(if_pattern) = &body.tail.as_ref().expect("expected tail").kind else {
        panic!("expected if-pattern tail");
    };
    assert!(matches!(
        &if_pattern.arms[0].pattern.kind,
        PatternKind::OptionalSome(inner)
            if matches!(
                &inner.kind,
                PatternKind::ErrorErr(payload)
                    if matches!(&payload.kind, PatternKind::Expr(_))
            )
    ));
    assert!(matches!(
        &if_pattern.arms[1].pattern.kind,
        PatternKind::OptionalSome(inner)
            if matches!(
                &inner.kind,
                PatternKind::ErrorErr(payload)
                    if matches!(&payload.kind, PatternKind::Bind { name, .. } if name == "err")
            )
    ));
    assert!(matches!(
        &if_pattern.arms[2].pattern.kind,
        PatternKind::OptionalSome(inner)
            if matches!(
                &inner.kind,
                PatternKind::ErrorOk(payload)
                    if matches!(&payload.kind, PatternKind::Bind { name, .. } if name == "ok")
            )
    ));
}

#[test]
fn parses_switch_arm_pattern_lists_and_ranges() {
    let (module, errors) = parse_module(
        r#"
fn main(state: i32) i32 {
    switch state {
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
    let expr = body.tail.as_ref().expect("expected switch tail");
    let ExprKind::Switch(switch) = &expr.kind else {
        panic!("expected switch expression");
    };
    assert_eq!(switch.arms[0].patterns.len(), 2);
    assert!(matches!(
        &switch.arms[0].patterns[0].kind,
        SwitchPatternKind::Expr(_)
    ));
    assert!(matches!(
        &switch.arms[0].patterns[1].kind,
        SwitchPatternKind::Expr(_)
    ));
    assert!(matches!(
        &switch.arms[1].patterns[0].kind,
        SwitchPatternKind::Range {
            inclusive: false,
            ..
        }
    ));
    assert!(matches!(
        &switch.arms[2].patterns[0].kind,
        SwitchPatternKind::Range {
            inclusive: true,
            ..
        }
    ));
    assert!(matches!(
        &switch.arms[3].patterns[0].kind,
        SwitchPatternKind::Wildcard
    ));
}

#[test]
fn rejects_open_ended_switch_range_patterns() {
    let (_module, errors) = parse_module(
        r#"
fn main(state: i32) i32 {
    switch state {
        1.. => 10,
        _ => 20,
    }
}
"#,
    );
    assert!(
        errors.iter().any(|error| error
            .message
            .contains("open-ended switch range patterns are not supported")),
        "{errors:?}"
    );
}
