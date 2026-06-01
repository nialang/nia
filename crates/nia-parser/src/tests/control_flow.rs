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
fn parses_c_style_for_header_with_parenthesized_block_expressions() {
    let (module, errors) = parse_module(
        r#"
fn main() {
    var i = 0;
    for ({
        var a = 1;
        var b = 3;
        {
            var c = 0;
            c + a + b
        }
    }); ({
        var d = 0;
        d < 4;
        true
    }); i += 1 {
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
    let StmtKind::For(for_stmt) = &body.stmts[1].kind else {
        panic!("expected for statement");
    };
    let ForHeader::CStyle { init, cond, step } = &for_stmt.header else {
        panic!("expected C-style for header");
    };
    assert!(matches!(
        init.as_deref(),
        Some(ForInit::Expr(expr)) if matches!(expr.kind, ExprKind::Block(_))
    ));
    assert!(matches!(
        cond.as_deref().map(|expr| &expr.kind),
        Some(ExprKind::Block(_))
    ));
    assert!(matches!(
        step.as_deref().map(|expr| &expr.kind),
        Some(ExprKind::Assign { .. })
    ));
}

#[test]
fn reports_ambiguous_block_as_first_for_header_expression() {
    let (_module, errors) = parse_module(
        r#"
fn main() {
    var i = 0;
    for {
        var a = 1;
        a
    }; true; i += 1 {
        _ = i;
    }
}
"#,
    );
    assert!(
        errors
            .iter()
            .any(|error| error.message.contains("expected expression")),
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
    for {
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
    let StmtKind::For(for_stmt) = &body.stmts[0].kind else {
        panic!("expected for statement");
    };
    let expr = for_stmt.body.tail.as_ref().expect("expected switch tail");
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
