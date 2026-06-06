// SPDX-License-Identifier: GPL-3.0-or-later
use super::common::*;

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
            .any(|diagnostic| diagnostic.summary.contains("switch range pattern is empty")),
        "{:?}",
        checked.diagnostics
    );
    assert!(
        checked
            .diagnostics
            .iter()
            .any(|diagnostic| diagnostic.summary.contains("integer switch target")),
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
            pattern,
            nia_body_ir::TypedSwitchPattern::CheckedInt { value: 1, .. }
        )),
        "{patterns:?}"
    );
    assert!(
        patterns.iter().any(|pattern| matches!(
            pattern,
            nia_body_ir::TypedSwitchPattern::CheckedIntRange {
                start: 2,
                end: 5,
                inclusive: false,
                ..
            }
        )),
        "{patterns:?}"
    );
    assert!(
        !patterns.iter().any(|pattern| matches!(
            pattern,
            nia_body_ir::TypedSwitchPattern::Expr(_)
                | nia_body_ir::TypedSwitchPattern::Range { .. }
        )),
        "{patterns:?}"
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
