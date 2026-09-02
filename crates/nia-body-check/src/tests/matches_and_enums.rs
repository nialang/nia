// SPDX-License-Identifier: GPL-3.0-or-later
use super::common::*;
use nia_ast::{ExprKind, ItemKind};
use nia_body_ir::{TypedExprKind, TypedStmtKind};
use nia_ids::GlobalDefId;
use nia_node_id::VersionedNodeKey;
use nia_span::Span;

#[test]
fn checks_payload_enum_construction_and_destructuring() {
    let checked = pipeline(
        r#"
enum Event {
    Closed,
    Data(i32),
    Move(i32, i32),
    Resize { width: i32, height: i32 },
}

fn data(value: i32) Event { Event::Data(value) }
fn resize(width: i32, height: i32) Event {
    Event::Resize { height: height, width: width }
}

fn sum(event: Event) i32 {
    match event {
        Event::Closed => 0,
        Event::Data(value) => value,
        Event::Move(x, y) => x + y,
        Event::Resize { width, height: h } => width + h,
    }
}
"#,
    );
    assert!(checked.diagnostics.is_empty(), "{:?}", checked.diagnostics);
    let payload_variants = checked
        .ir
        .function_bodies
        .values()
        .filter_map(|body| body.tail.as_ref())
        .filter(|expr| {
            matches!(
                &expr.kind,
                TypedExprKind::EnumVariant { fields, .. } if !fields.is_empty()
            )
        })
        .count();
    assert_eq!(payload_variants, 2, "{:#?}", checked.ir.function_bodies);
}

#[test]
fn checks_omitted_struct_and_enum_constructors_from_expected_types() {
    let checked = pipeline(
        r#"
struct Point {
    x: i32,
    y: i32,
}

enum Color {
    Red,
    Data(i32),
    Resize { value: i32 },
}

fn make() Point {
    let point: Point = .{ x: 1, y: 2 };
    let red: Color = .Red;
    let data: Color = .Data(3);
    let resize: Color = .Resize { value: 4 };
    _ = red;
    _ = data;
    _ = resize;
    point
}
"#,
    );
    assert!(checked.diagnostics.is_empty(), "{:?}", checked.diagnostics);
    let body = checked
        .ir
        .function_bodies
        .values()
        .find(|body| body.tail.is_some())
        .expect("make body");
    assert!(matches!(
        body.stmts.first().map(|stmt| &stmt.kind),
        Some(TypedStmtKind::Binding(_))
    ));
    assert!(body.stmts.iter().any(|stmt| matches!(
        &stmt.kind,
        TypedStmtKind::Binding(binding)
            if matches!(binding.value.as_ref().map(|value| &value.kind),
                Some(TypedExprKind::EnumVariant { fields, .. }) if fields.len() == 1)
    )));
}

#[test]
fn checks_omitted_enum_patterns_and_exhaustiveness() {
    let checked = pipeline(
        r#"
enum Color {
    Red,
    Data(i32),
}

fn score(color: Color) i32 {
    match color {
        .Red => 0,
        .Data(value) => value,
    }
}
"#,
    );
    assert!(checked.diagnostics.is_empty(), "{:?}", checked.diagnostics);
}

#[test]
fn materializes_const_enum_payloads_at_runtime() {
    let checked = pipeline(
        r#"
enum Event {
    Closed,
    Data(i32),
    Resize { width: i32, height: i32 },
}

const CLOSED: Event = Event::Closed;
const DATA: Event = Event::Data(7);
const RESIZE: Event = Event::Resize { height: 3, width: 5 };

fn score(event: Event) i32 {
    match event {
        Event::Closed => 0,
        Event::Data(value) => value,
        Event::Resize { width, height } => width + height,
    }
}

fn main() i32 {
    score(CLOSED) + score(DATA) + score(RESIZE)
}
"#,
    );

    assert!(checked.diagnostics.is_empty(), "{:?}", checked.diagnostics);
}

#[test]
fn rejects_invalid_payload_enum_constructors_and_patterns() {
    let checked = pipeline(
        r#"
enum Event {
    Closed,
    Data(i32),
    Resize { width: i32, height: i32 },
}

fn invalid_constructors() {
    _ = Event::Closed(1);
    _ = Event::Data;
    _ = Event::Data(1, 2);
    _ = Event::Resize { width: 1 };
    _ = Event::Resize { width: 1, height: 2, depth: 3 };
}

fn invalid_patterns(event: Event) i32 {
    match event {
        Event::Closed(value) => value,
        Event::Data() => 0,
        Event::Data { value } => value,
        Event::Resize { width, width: other } => width + other,
        _ => 0,
    }
}
"#,
    );
    let summaries = checked
        .diagnostics
        .iter()
        .map(|diagnostic| diagnostic.summary.as_str())
        .collect::<Vec<_>>();
    for expected in [
        "expects no payload",
        "requires a payload",
        "expects 1 payload values, found 2",
        "missing payload field `height`",
        "unknown payload field `depth`",
        "unit enum variant `Closed` has no payload",
        "expects 1 pattern fields, found 0",
        "payload pattern has the wrong shape",
        "duplicate payload pattern field `width`",
    ] {
        assert!(
            summaries.iter().any(|summary| summary.contains(expected)),
            "missing {expected:?} in {:#?}",
            checked.diagnostics
        );
    }
}

#[test]
fn payload_enum_coverage_merges_recursive_single_field_patterns() {
    let checked = pipeline(
        r#"
enum Event {
    Closed,
    Data(?i32),
    Result(i32!i32),
}

enum PairEvent {
    Bits(bool, bool),
}

enum Envelope {
    Event(Event),
}

fn complete(event: Event) i32 {
    match event {
        Event::Closed => 0,
        Event::Data(?value) => value,
        Event::Data(null) => 1,
        Event::Result(!value) => value,
        Event::Result(error!) => error,
    }
}

fn nested_complete(envelope: Envelope) i32 {
    match envelope {
        Envelope::Event(Event::Closed) => 0,
        Envelope::Event(Event::Data(?value)) => value,
        Envelope::Event(Event::Data(null)) => 1,
        Envelope::Event(Event::Result(!value)) => value,
        Envelope::Event(Event::Result(error!)) => error,
    }
}

fn missing_null(event: Event) i32 {
    match event {
        Event::Closed => 0,
        Event::Data(?value) => value,
        Event::Result(!value) => value,
        Event::Result(error!) => error,
    }
}

fn diagonal_is_not_complete(event: PairEvent) i32 {
    match event {
        PairEvent::Bits(true, true) => 1,
        PairEvent::Bits(false, false) => 0,
    }
}

fn binding_covers_product(event: PairEvent) i32 {
    match event {
        PairEvent::Bits(left, right) => if left == right { 1 } else { 0 },
    }
}
"#,
    );
    assert_eq!(checked.diagnostics.len(), 2, "{:?}", checked.diagnostics);
    assert_eq!(
        checked
            .diagnostics
            .iter()
            .filter(|diagnostic| diagnostic.summary.contains("non-exhaustive match"))
            .count(),
        2,
        "{:?}",
        checked.diagnostics
    );
}

#[test]
fn payload_enums_flow_through_generic_functions() {
    let checked = pipeline(
        r#"
enum Event {
    Closed,
    Data(i32),
}

fn id[T](value: T) T { value }

fn main() i32 {
    let event = id[Event](Event::Data(7));
    match event {
        Event::Closed => 0,
        Event::Data(value) => value,
    }
}
"#,
    );
    assert!(checked.diagnostics.is_empty(), "{:?}", checked.diagnostics);
}

#[test]
fn checks_enum_variants_and_match_exhaustiveness() {
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
    match c {
        Color::Red => return 1,
        Color::Green => return 2,
        Color::Blue => return 3,
    }
    0
}

fn missing(c: Color) i32 {
    match c {
        Color::Red => return 1,
    }
    0
}

fn with_default(c: Color) i32 {
    match c {
        Color::Red => return 1,
        _ => return 0,
    }
    0
}

fn bad(c: Color) i32 {
    match c {
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
            .filter(|diagnostic| diagnostic.summary.contains("non-exhaustive match"))
            .count(),
        1
    );
    assert!(
        checked
            .diagnostics
            .iter()
            .any(|diagnostic| diagnostic.summary.contains("match pattern"))
    );
    assert!(
        checked
            .diagnostics
            .iter()
            .any(|diagnostic| diagnostic.summary.contains("unknown enum variant"))
    );
}

#[test]
fn if_pattern_payload_field_lhs_shadows_imported_value_fact() {
    let source = r#"
struct S {
    start: i32,
}

fn imported_range() i32 {
    0
}

fn value(input: ?S) ?i32 {
    if input is ?range {
        ?range.start
    } else {
        null
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
                ItemKind::Function(function) if function.name == sym("imported_range") => {
                    defs.def_nodes.get(&function.node_key)
                }
                _ => None,
            })
            .expect("imported_range def");
        values.insert_node_qualified_value(
            if_pattern_payload_field_lhs_key(module),
            GlobalDefId {
                module_id: defs.module_id,
                def_id: imported_range,
            },
        );
    });

    assert!(checked.diagnostics.is_empty(), "{:?}", checked.diagnostics);
    assert!(
        if_pattern_payload_field_lhs_is_local(&checked.ir, field_span),
        "{:#?}",
        checked.ir.function_bodies
    );
}

fn if_pattern_payload_field_lhs_key(module: &nia_ast::Module) -> VersionedNodeKey {
    module
        .items
        .iter()
        .find_map(|item| match &item.kind {
            ItemKind::Function(function) if function.name == sym("value") => {
                let body = function.body.as_ref()?;
                let tail = body.tail.as_ref()?;
                let ExprKind::IfPattern(if_pattern) = &tail.kind else {
                    return None;
                };
                let expr = if_pattern.then_branch.tail.as_ref()?;
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
        .expect("if-pattern payload field lhs")
}

fn if_pattern_payload_field_lhs_is_local(ir: &nia_body_ir::BodyIr, field_span: Span) -> bool {
    ir.function_bodies.values().any(|body| {
        let Some(tail) = &body.tail else {
            return false;
        };
        let TypedExprKind::IfPattern(if_pattern) = &tail.kind else {
            return false;
        };
        let Some(expr) = &if_pattern.then_branch.tail else {
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
}

#[test]
fn checks_match_expressions() {
    let checked = pipeline(
        r#"
enum Color {
    Red,
    Green,
}

fn pick(c: Color) i32 {
    match c {
        Color::Red => 1,
        Color::Green => 2,
    }
}

fn with_default(x: u32) i32 {
    match x {
        0 => 10,
        _ => 20,
    }
}

fn with_return_arm(x: u32) i32 {
    match x {
        0 => return 1,
        _ => 2,
    }
}

fn bad(x: u32) i32 {
    match x {
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
            .any(|diagnostic| diagnostic.summary.contains("type mismatch in match arms")),
        "{:?}",
        checked.diagnostics
    );
    assert_eq!(
        checked
            .diagnostics
            .iter()
            .filter(|diagnostic| diagnostic.summary.contains("non-exhaustive enum match"))
            .count(),
        0,
        "{:?}",
        checked.diagnostics
    );
}

#[test]
fn checks_match_arm_body_edge_cases() {
    let checked = pipeline(
        r#"
fn value() i32 { 1 }
fn cleanup() {}

fn expr_stmt_arm(x: i32) i32 {
    match x {
        0 => cleanup(),
        _ => value(),
    }
}

fn block_arm_void_tail(x: i32) i32 {
    match x {
        0 => {
            cleanup();
        },
        _ => 2,
    }
}

fn block_arm_never_tail(x: i32) i32 {
    match x {
        0 => {
            return 10;
        },
        _ => 2,
    }
}

fn statement_arm_never(x: i32) i32 {
    match x {
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
            .filter(|diagnostic| diagnostic.summary.contains("type mismatch in match arms"))
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
fn infers_match_pattern_numeric_literals_from_target_type() {
    let checked = pipeline(
        r#"
fn classify(value: usize) i32 {
    match value {
        0 => return 0,
        1 + 1 => return 2,
        _ => return 3,
    }
    4
}

fn bad(value: u8) i32 {
    match value {
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
            .contains("type mismatch in match pattern")),
        "{:?}",
        checked.diagnostics
    );
}

#[test]
fn checks_match_pattern_lists_and_ranges() {
    let checked = pipeline(
        r#"
fn classify(value: i32) i32 {
    match value {
        0, 1 => 10,
        2..5 => 20,
        5..=7 => 30,
        _ => 40,
    }
}

fn overlap(value: i32) i32 {
    match value {
        0..3 => 10,
        2 => 20,
        _ => 30,
    }
}

fn empty(value: i32) i32 {
    match value {
        3..3 => 10,
        _ => 20,
    }
}

fn non_integer(value: bool) i32 {
    match value {
        false..=true => 10,
        _ => 20,
    }
}

fn non_constant(value: i32, start: i32) i32 {
    match value {
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
            .any(|diagnostic| diagnostic.summary.contains("match pattern is unreachable")),
        "{:?}",
        checked.diagnostics
    );
    assert!(
        checked
            .diagnostics
            .iter()
            .any(|diagnostic| diagnostic.summary.contains("match pattern range is empty")),
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
                .iter_node_pattern_values()
                .any(|(_, value)| *value == expected),
            "missing match pattern value {expected}: {:?}",
            checked.facts.iter_node_pattern_values().collect::<Vec<_>>()
        );
    }
}

#[test]
fn lowers_integer_match_patterns_from_checked_values() {
    let checked = pipeline(
        r#"
fn main() i32 {
    let mut x: i32 = 2;
    match x {
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
            nia_body_ir::TypedExprKind::Match(matched) => Some(matched.as_ref()),
            _ => None,
        })
        .flat_map(|matched| matched.arms.iter())
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
    match result {
        !value => {
            value
        },
        err! => {
            err
        },
    }
}

fn unwrap_nested(value: ?(i32!i32)) i32 {
    match value {
        ?!ok => {
            ok
        },
        ?err! => {
            err
        },
        null => {
            0
        },
    }
}

fn match_error_literal(value: ?(i32!i32)) i32 {
    match value {
        ?5! => {
            5
        },
        ?!ok => {
            ok
        },
        null => {
            0
        },
        _ => {
            9
        },
    }
}

fn bind_plain(value: i32) i32 {
    if value is mut current {
        current += 1;
        current
    }
}
"#,
    );
    assert!(checked.diagnostics.is_empty(), "{:?}", checked.diagnostics);
}

#[test]
fn checks_match_destructuring_and_recursive_exhaustiveness() {
    let checked = pipeline(
        r#"
fn optional(value: ?i32) i32 {
    match value {
        ?payload => payload,
        null => 0,
    }
}

fn error_union(value: i32!i32) i32 {
    match value {
        !payload => payload,
        error! => error,
    }
}

fn nested(value: ?(i32!i32)) i32 {
    match value {
        ?!payload => payload,
        ?error! => error,
        null => 0,
    }
}
"#,
    );
    assert!(checked.diagnostics.is_empty(), "{:?}", checked.diagnostics);
}

#[test]
fn checks_struct_patterns_across_runtime_and_const_contexts() {
    let checked = pipeline(
        r#"
struct Point { x: i32, y: i32 }
struct Box[T] { value: T }
enum Operation { Read, Write }
enum Failure { System { operation: Operation, code: i32 } }

fn sum(point: Point) i32 {
    let Point { y: second, x } = point;
    x + second
}

fn unbox[T](value: Box[T]) T {
    let Box { value } = value;
    value
}

fn throughPointer(point: &Point) i32 {
    let &Point { x, y } = point;
    x + y
}

fn nested(value: ?Point) i32 {
    if value is ?Point { x, y: _ } {
        x
    } else {
        0
    }
}

fn classify(point: Point) i32 {
    match point {
        Point { x: 0, y } => y,
        Point { x: _, y: _ } => 9,
    }
}

fn nestedEnum(value: Failure!i32) i32 {
    match value {
        !ok => { _ = ok; 0 },
        Failure::System { operation: Operation::Read, code: _ }! => 1,
        error! => { _ = error; 0 },
    }
}

const fn constSum(point: Point) i32 {
    let mut Point { y, x } = point;
    x += 1;
    x + y
}

const TOTAL: i32 = constSum(Point { x: 19, y: 22 });
"#,
    );
    assert!(checked.diagnostics.is_empty(), "{:?}", checked.diagnostics);

    let nominal_patterns = checked
        .ir
        .function_bodies
        .values()
        .flat_map(|body| &body.stmts)
        .filter_map(|stmt| match &stmt.kind {
            nia_body_ir::TypedStmtKind::PatternBinding(binding) => Some(&binding.pattern.kind),
            _ => None,
        })
        .filter(|kind| {
            matches!(
                kind,
                nia_body_ir::TypedPatternKind::Nominal {
                    constructor: nia_body_ir::TypedNominalPatternConstructor::Struct { .. },
                    ..
                }
            )
        })
        .count();
    assert!(nominal_patterns >= 2);
    assert!(checked.ir.function_bodies.values().any(|body| {
        let Some(tail) = &body.tail else {
            return false;
        };
        let nia_body_ir::TypedExprKind::Match(matched) = &tail.kind else {
            return false;
        };
        matches!(
            &matched.arms[1].patterns[0].kind,
            nia_body_ir::TypedPatternKind::ErrorErr(outer)
                if matches!(
                    &outer.kind,
                    nia_body_ir::TypedPatternKind::Nominal { fields, .. }
                        if fields.iter().any(|field| matches!(
                            &field.kind,
                            nia_body_ir::TypedPatternKind::Nominal {
                                constructor:
                                    nia_body_ir::TypedNominalPatternConstructor::EnumVariant { .. },
                                fields,
                            } if fields.is_empty()
                        ))
                )
        )
    }));
}

#[test]
fn checks_nominal_rest_and_matrix_pattern_soundness() {
    let accepted = pipeline(
        r#"
struct Point { x: bool, y: i32 }
struct Box[T] { value: T, tag: i32 }
enum Event { Stop, Resize { wide: bool, height: i32 } }

fn bind(point: Point) bool {
    let Point { x, .. } = point;
    x
}

fn generic[T](boxed: Box[T]) T {
    let Box { value, .. } = boxed;
    value
}

fn classify(event: Event) i32 {
    match event {
        Event::Stop => 0,
        Event::Resize { wide: true, .. } => 1,
        Event::Resize { wide: false, .. } => 2,
    }
}

fn byte(value: u8) i32 {
    match value {
        0..=127 => 0,
        128..=255 => 1,
    }
}

const fn constBind(point: Point) bool {
    let Point { x, .. } = point;
    x
}

const FLAG: bool = constBind(Point { x: true, y: 9 });
"#,
    );
    assert!(
        accepted.diagnostics.is_empty(),
        "{:?}",
        accepted.diagnostics
    );

    let rejected = pipeline(
        r#"
struct Flags { left: bool, right: bool }

fn boolean(value: bool) i32 {
    match value {
        true => 1,
    }
}

fn tupleProduct(value: (bool, bool)) i32 {
    match value {
        (true, _) => 1,
        (_, false) => 2,
    }
}

fn structProduct(value: Flags) i32 {
    match value {
        Flags { left: true, .. } => 1,
        Flags { right: false, .. } => 2,
    }
}

fn unreachable(value: Flags) i32 {
    match value {
        Flags { .. } => 1,
        Flags { left: true, .. } => 2,
    }
}

fn integerGap(value: u8) i32 {
    match value {
        0..10 => 0,
        11..=255 => 1,
    }
}
"#,
    );
    for witness in [
        "false",
        "(false, true)",
        "Flags { left: false, right: true }",
        "10",
    ] {
        assert!(
            rejected
                .diagnostics
                .iter()
                .any(|diagnostic| diagnostic.summary.contains(witness)),
            "missing witness `{witness}` in {:?}",
            rejected.diagnostics
        );
    }
    assert!(
        rejected
            .diagnostics
            .iter()
            .any(|diagnostic| diagnostic.summary.contains("match pattern is unreachable"))
    );
}

#[test]
fn formats_tuple_struct_exhaustiveness_witness_positionally() {
    let checked = pipeline(
        r#"
struct Pair(bool, bool)

fn classify(value: Pair) i32 {
    match value {
        Pair(true, _) => 1,
    }
}
"#,
    );

    assert!(
        checked.diagnostics.iter().any(|diagnostic| {
            diagnostic
                .summary
                .contains("non-exhaustive matched, missing pattern: `Pair(false, _)`")
        }),
        "{:?}",
        checked.diagnostics
    );
}

#[test]
fn rejects_invalid_struct_pattern_field_sets_and_constructors() {
    let checked = pipeline(
        r#"
struct Point { x: i32, y: i32 }
struct Other { x: i32, y: i32 }

fn duplicate(point: Point) i32 {
    let Point { x, x: again, y } = point;
    x + again + y
}

fn missing(point: Point) i32 {
    let Point { x } = point;
    x
}

fn unknown(point: Point) i32 {
    let Point { x, y, z } = point;
    x + y + z
}

fn wrong(point: Point) i32 {
    let Other { x, y } = point;
    x + y
}
"#,
    );
    for expected in [
        "duplicate struct pattern field `x`",
        "missing struct pattern field `y`",
        "unknown struct pattern field `z`",
        "constructor does not match target type `Point`",
    ] {
        assert!(
            checked
                .diagnostics
                .iter()
                .any(|diagnostic| diagnostic.summary.contains(expected)),
            "missing `{expected}` in {:?}",
            checked.diagnostics
        );
    }
}

#[test]
fn rejects_non_exhaustive_and_ambiguous_match_bindings() {
    let checked = pipeline(
        r#"
fn missing(value: ?i32) i32 {
    match value {
        ?payload => payload,
    }
}

fn alternatives(value: ?i32) i32 {
    match value {
        ?payload, null => payload,
        _ => 0,
    }
}
"#,
    );
    assert!(
        checked.diagnostics.iter().any(|diagnostic| {
            diagnostic
                .summary
                .contains("non-exhaustive matched, missing pattern")
        }),
        "{:?}",
        checked.diagnostics
    );
    assert!(
        checked.diagnostics.iter().any(|diagnostic| {
            diagnostic
                .summary
                .contains("multiple alternative patterns cannot bind values")
        }),
        "{:?}",
        checked.diagnostics
    );
}

#[test]
fn checks_if_pattern_binding_mutability() {
    let checked = pipeline(
        r#"
fn mutable(value: ?i32) i32 {
    match value {
        mut ?current => {
            current += 1;
            current
        },
        null => {
            0
        },
    }
}

struct Link { next: ?&mut Link }

fn mutable_pointer(value: ?&mut Link) i32 {
    match value {
        mut ?current => {
            _ = current;
            1
        },
        null => {
            0
        },
    }
}

fn immutable(value: ?i32) i32 {
    match value {
        ?current => {
            current += 1;
            current
        },
        null => {
            0
        },
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
    assert!(
        !checked.diagnostics.iter().any(|diagnostic| {
            diagnostic.summary.contains("non-exhaustive matched")
                || diagnostic.summary.contains("match pattern is unreachable")
        }),
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
    let mut same = Color::Red == Color::Green;
    let mut n: i32 = Color::Red;
    let mut explicit: i32 = Color::Red as i32;
    let mut bad_add = Color::Red + Color::Green;
    let mut bad_order = Color::Red < Color::Green;
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
