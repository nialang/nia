// SPDX-License-Identifier: GPL-3.0-or-later
use super::common::*;
use nia_ast::{ExprKind, ItemKind, SwitchArmBody};
use nia_body_ir::{TypedExprKind, TypedSwitchArmBody};
use nia_ids::GlobalDefId;
use nia_node_id::NodeKey;
use nia_span::Span;

#[test]
fn checks_enum_variants_and_switch_exhaustiveness() {
    let checked = pipeline(
        r#"
enum Color {
    Red,
    Green,
    Blue,
}

enum Other {
    One,
}

fn full(c: Color) i32 {
    switch c {
        Color::Red => return 1,
        Color::Green => return 2,
        Color::Blue => return 3,
    }
    0
}

fn missing(c: Color) i32 {
    switch c {
        Color::Red => return 1,
    }
    0
}

fn with_default(c: Color) i32 {
    switch c {
        Color::Red => return 1,
        _ => return 0,
    }
    0
}

fn bad(c: Color) i32 {
    switch c {
        Other::One => return 1,
        Color::Missing => return 2,
        _ => return 0,
    }
    0
}
"#,
    );
    assert_eq!(
        checked
            .diagnostics
            .iter()
            .filter(|diagnostic| diagnostic.summary.contains("non-exhaustive enum switch"))
            .count(),
        1
    );
    assert!(
        checked
            .diagnostics
            .iter()
            .any(|diagnostic| diagnostic.summary.contains("switch pattern"))
    );
    assert!(
        checked
            .diagnostics
            .iter()
            .any(|diagnostic| diagnostic.summary.contains("unknown enum variant"))
    );
}

#[test]
fn switch_payload_field_lhs_shadows_imported_value_fact() {
    let source = r#"
struct S {
    start: i32,
}

fn imported_range() i32 {
    0
}

fn value(input: ?S) ?i32 {
    switch input {
        ?range => ?range.start,
        null => null,
    }
}
"#;
    let field_span = {
        let start = source.find("?range.start").expect("range field use") + 1;
        Span::new(start, start + "range.start".len())
    };
    let checked = pipeline_with_values(source, |module, defs, values| {
        let imported_range = module
            .items
            .iter()
            .find_map(|item| match &item.kind {
                ItemKind::Function(function) if function.name == "imported_range" => {
                    defs.def_nodes.get(&function.node_key)
                }
                _ => None,
            })
            .expect("imported_range def");
        values.node_qualified_values.insert(
            switch_payload_field_lhs_key(module),
            GlobalDefId {
                module_id: ModuleId(0),
                def_id: imported_range,
            },
        );
    });

    assert!(checked.diagnostics.is_empty(), "{:?}", checked.diagnostics);
    assert!(
        switch_payload_field_lhs_is_local(&checked.ir, field_span),
        "{:#?}",
        checked.ir.function_bodies
    );
}

fn switch_payload_field_lhs_key(module: &nia_ast::Module) -> NodeKey {
    module
        .items
        .iter()
        .find_map(|item| match &item.kind {
            ItemKind::Function(function) if function.name == "value" => {
                let body = function.body.as_ref()?;
                let tail = body.tail.as_ref()?;
                let ExprKind::Switch(switch) = &tail.kind else {
                    return None;
                };
                let SwitchArmBody::Expr(expr) = &switch.arms.first()?.body else {
                    return None;
                };
                let ExprKind::OptionalSome { expr } = &expr.kind else {
                    return None;
                };
                let ExprKind::Field { lhs, .. } = &expr.kind else {
                    return None;
                };
                Some(lhs.node_key.clone())
            }
            _ => None,
        })
        .expect("switch payload field lhs")
}

fn switch_payload_field_lhs_is_local(ir: &nia_body_ir::BodyIr, field_span: Span) -> bool {
    ir.function_bodies.values().any(|body| {
        let Some(tail) = &body.tail else {
            return false;
        };
        let TypedExprKind::Switch(switch) = &tail.kind else {
            return false;
        };
        switch.arms.iter().any(|arm| {
            let TypedSwitchArmBody::Expr(expr) = &arm.body else {
                return false;
            };
            let TypedExprKind::OptionalSome { expr } = &expr.kind else {
                return false;
            };
            let TypedExprKind::Field { lhs, .. } = &expr.kind else {
                return false;
            };
            expr.span == field_span && matches!(lhs.kind, TypedExprKind::Local(_))
        })
    })
}

#[test]
fn checks_switch_expressions() {
    let checked = pipeline(
        r#"
enum Color {
    Red,
    Green,
}

fn pick(c: Color) i32 {
    switch c {
        Color::Red => 1,
        Color::Green => 2,
    }
}

fn with_default(x: u32) i32 {
    switch x {
        0 => 10,
        _ => 20,
    }
}

fn with_return_arm(x: u32) i32 {
    switch x {
        0 => return 1,
        _ => 2,
    }
}

fn bad(x: u32) i32 {
    switch x {
        0 => 1,
        _ => true,
    }
}
"#,
    );
    assert!(
        checked
            .diagnostics
            .iter()
            .any(|diagnostic| diagnostic.summary.contains("type mismatch in switch arms")),
        "{:?}",
        checked.diagnostics
    );
    assert_eq!(
        checked
            .diagnostics
            .iter()
            .filter(|diagnostic| diagnostic.summary.contains("non-exhaustive enum switch"))
            .count(),
        0,
        "{:?}",
        checked.diagnostics
    );
}

#[test]
fn checks_switch_arm_body_edge_cases() {
    let checked = pipeline(
        r#"
fn value() i32 { 1 }
fn cleanup() {}

fn expr_stmt_arm(x: i32) i32 {
    switch x {
        0 => cleanup(),
        _ => value(),
    }
}

fn block_arm_void_tail(x: i32) i32 {
    switch x {
        0 => {
            cleanup();
        },
        _ => 2,
    }
}

fn block_arm_never_tail(x: i32) i32 {
    switch x {
        0 => {
            return 10;
        },
        _ => 2,
    }
}

fn statement_arm_never(x: i32) i32 {
    switch x {
        0 => return 1,
        _ => 2,
    }
}

fn main() i32 {
    expr_stmt_arm(0) + block_arm_void_tail(0) + block_arm_never_tail(0) + statement_arm_never(0)
}
"#,
    );
    assert_eq!(
        checked
            .diagnostics
            .iter()
            .filter(|diagnostic| diagnostic.summary.contains("type mismatch in switch arms"))
            .count(),
        2,
        "{:?}",
        checked.diagnostics
    );
    assert!(
        !checked
            .diagnostics
            .iter()
            .any(|diagnostic| diagnostic.summary.contains("statement_arm_never")),
        "{:?}",
        checked.diagnostics
    );
}

#[test]
fn infers_switch_pattern_numeric_literals_from_target_type() {
    let checked = pipeline(
        r#"
fn classify(value: usize) i32 {
    switch value {
        0 => return 0,
        1 + 1 => return 2,
        _ => return 3,
    }
    4
}

fn bad(value: u8) i32 {
    switch value {
        256 => return 1,
        _ => return 0,
    }
    0
}
"#,
    );
    assert_eq!(
        checked
            .diagnostics
            .iter()
            .filter(|diagnostic| diagnostic.summary.contains("out of range for u8"))
            .count(),
        1,
        "{:?}",
        checked.diagnostics
    );
    assert!(
        !checked.diagnostics.iter().any(|diagnostic| diagnostic
            .summary
            .contains("type mismatch in switch pattern")),
        "{:?}",
        checked.diagnostics
    );
}

#[test]
fn checks_switch_pattern_lists_and_ranges() {
    let checked = pipeline(
        r#"
fn classify(value: i32) i32 {
    switch value {
        0, 1 => 10,
        2..5 => 20,
        5..=7 => 30,
        _ => 40,
    }
}

fn overlap(value: i32) i32 {
    switch value {
        0..3 => 10,
        2 => 20,
        _ => 30,
    }
}

fn empty(value: i32) i32 {
    switch value {
        3..3 => 10,
        _ => 20,
    }
}

fn non_integer(value: bool) i32 {
    switch value {
        false..=true => 10,
        _ => 20,
    }
}

fn non_constant(value: i32, start: i32) i32 {
    switch value {
        start..3 => 10,
        _ => 20,
    }
}
"#,
    );
    assert!(
        checked
            .diagnostics
            .iter()
            .any(|diagnostic| diagnostic.summary.contains("switch pattern overlaps")),
        "{:?}",
        checked.diagnostics
    );
    assert!(
        checked
            .diagnostics
            .iter()
            .any(|diagnostic| diagnostic.summary.contains("switch pattern range is empty")),
        "{:?}",
        checked.diagnostics
    );
    assert!(
        checked
            .diagnostics
            .iter()
            .any(|diagnostic| diagnostic.summary.contains("integer target")),
        "{:?}",
        checked.diagnostics
    );
    assert!(
        checked
            .diagnostics
            .iter()
            .any(|diagnostic| diagnostic.summary.contains("compile-time integer constant")),
        "{:?}",
        checked.diagnostics
    );
    for expected in [0, 1, 2, 5, 7] {
        assert!(
            checked
                .facts
                .node_switch_pattern_values
                .values()
                .any(|value| *value == expected),
            "missing switch pattern value {expected}: {:?}",
            checked.facts.node_switch_pattern_values
        );
    }
}

#[test]
fn lowers_integer_switch_patterns_from_checked_values() {
    let checked = pipeline(
        r#"
fn main() i32 {
    var x: i32 = 2;
    switch x {
        1 => return 10,
        2..5 => return 20,
        _ => return 30,
    }
    0
}
"#,
    );
    assert!(checked.diagnostics.is_empty(), "{:?}", checked.diagnostics);

    let patterns = checked
        .ir
        .function_bodies
        .values()
        .flat_map(|body| body.stmts.iter())
        .filter_map(|stmt| match &stmt.kind {
            nia_body_ir::TypedStmtKind::Expr(expr) => Some(expr),
            _ => None,
        })
        .filter_map(|expr| match &expr.kind {
            nia_body_ir::TypedExprKind::Switch(switch) => Some(switch.as_ref()),
            _ => None,
        })
        .flat_map(|switch| switch.arms.iter())
        .flat_map(|arm| arm.patterns.iter())
        .collect::<Vec<_>>();

    assert!(
        patterns.iter().any(|pattern| matches!(
            &pattern.kind,
            nia_body_ir::TypedPatternKind::CheckedInt { value: 1 }
        )),
        "{patterns:?}"
    );
    assert!(
        patterns.iter().any(|pattern| matches!(
            &pattern.kind,
            nia_body_ir::TypedPatternKind::CheckedIntRange {
                start: 2,
                end: 5,
                inclusive: false,
            }
        )),
        "{patterns:?}"
    );
    assert!(
        !patterns.iter().any(|pattern| matches!(
            &pattern.kind,
            nia_body_ir::TypedPatternKind::Expr(_) | nia_body_ir::TypedPatternKind::Range { .. }
        )),
        "{patterns:?}"
    );
}

#[test]
fn checks_recursive_optional_error_union_patterns_and_if_patterns() {
    let checked = pipeline(
        r#"
fn unwrap_result(result: i32!i32) i32 {
    if let !value = result {
        value
    } else err! {
        err
    }
}

fn unwrap_nested(value: ?(i32!i32)) i32 {
    switch value {
        ?!ok => ok,
        ?err! => err,
        null => 0,
    }
}

fn match_error_literal(value: ?(i32!i32)) i32 {
    switch value {
        ?5! => 5,
        ?!ok => ok,
        null => 0,
        _ => 9,
    }
}

fn bind_plain(value: i32) i32 {
    if var current = value {
        current += 1;
        current
    }
}
"#,
    );
    assert!(checked.diagnostics.is_empty(), "{:?}", checked.diagnostics);
}

#[test]
fn checks_if_pattern_let_and_var_binding_mutability() {
    let checked = pipeline(
        r#"
fn mutable(value: ?i32) i32 {
    if var ?current = value {
        current += 1;
        current
    } else null {
        0
    }
}

fn immutable(value: ?i32) i32 {
    if let ?current = value {
        current += 1;
        current
    } else null {
        0
    }
}
"#,
    );
    assert!(
        checked
            .diagnostics
            .iter()
            .any(|diagnostic| diagnostic.summary.contains("local is let")),
        "{:?}",
        checked.diagnostics
    );
    assert_eq!(
        checked
            .diagnostics
            .iter()
            .filter(|diagnostic| diagnostic.summary.contains("local is let"))
            .count(),
        1,
        "{:?}",
        checked.diagnostics
    );
}

#[test]
fn rejects_implicit_enum_integer_mixing() {
    let checked = pipeline(
        r#"
enum Color: u8 {
    Red,
    Green,
}

fn main() i32 {
    var same = Color::Red == Color::Green;
    var n: i32 = Color::Red;
    var explicit: i32 = Color::Red as i32;
    var bad_add = Color::Red + Color::Green;
    var bad_order = Color::Red < Color::Green;
    if same { explicit } else { n }
}
"#,
    );
    assert!(
        checked
            .diagnostics
            .iter()
            .any(|diagnostic| diagnostic.summary.contains("binding initializer"))
    );
    assert!(
        checked
            .diagnostics
            .iter()
            .any(|diagnostic| diagnostic.summary.contains("trait bound not satisfied"))
    );
    assert!(
        !checked
            .diagnostics
            .iter()
            .any(|diagnostic| diagnostic.summary.contains("explicit"))
    );
}
