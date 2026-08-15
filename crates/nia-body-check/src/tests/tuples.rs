// SPDX-License-Identifier: GPL-3.0-or-later
use super::common::*;

#[test]
fn tuple_structs_construct_project_and_destructure_nominal_values() {
    let checked = pipeline(
        r#"
struct FooId(u64)
struct RGB(u8, u8, u8)

fn unwrap(id: FooId) u64 {
    match id {
        FooId(value) => value,
    }
}

fn sum(color: RGB) u8 {
    match color {
        RGB(r, g, b) => r + g + b,
    }
}

fn main() u64 {
    let id = FooId(40u64);
    let color = RGB(1u8, 2u8, 3u8);
    unwrap(id) + color.0 as u64 + sum(color) as u64
}
"#,
    );

    assert!(checked.diagnostics.is_empty(), "{:?}", checked.diagnostics);
}

#[test]
fn generic_tuple_structs_accept_explicit_constructor_arguments() {
    let checked = pipeline(
        r#"
struct Pair[T, U](T, U)

fn main() i32 {
    let pair = Pair[i32, bool](40, true);
    match pair {
        Pair(left, _) => left,
    }
}
"#,
    );
    assert!(checked.diagnostics.is_empty(), "{:?}", checked.diagnostics);
}

#[test]
fn const_generic_tuple_structs_accept_explicit_constructor_arguments() {
    let checked = pipeline(
        r#"
struct Box[T](T)

const VALUE: Box[i32] = Box[i32](42);

fn main() i32 {
    VALUE.0
}
"#,
    );

    assert!(checked.diagnostics.is_empty(), "{:?}", checked.diagnostics);
}

#[test]
fn tuple_structs_enforce_shape_arity_and_nominal_identity() {
    let checked = pipeline(
        r#"
struct FooId(u64)
struct Pair(i32, bool)
struct Point { x: i32 }

fn invalid(id: FooId, pair: Pair) {
    let raw: u64 = id;
    _ = FooId();
    _ = FooId(1u64, 2u64);
    _ = id.1;
    _ = FooId { value: 1u64 };
    match pair {
        Pair(value) => value,
    };
    match pair {
        Pair { value: value } => value,
    };
}

"#,
    );

    let summaries = checked
        .diagnostics
        .iter()
        .map(|diagnostic| diagnostic.summary.as_str())
        .collect::<Vec<_>>();
    for expected in [
        "type mismatch",
        "expects 1 constructor arguments, found 0",
        "expects 1 constructor arguments, found 2",
        "out of bounds for tuple struct of arity 1",
        "expects 2 pattern fields, found 1",
        "tuple struct patterns require positional fields",
    ] {
        assert!(
            summaries.iter().any(|summary| summary.contains(expected)),
            "missing {expected:?} in {:#?}",
            checked.diagnostics
        );
    }
}

#[test]
fn const_tuple_structs_support_construction_and_matching() {
    let checked = pipeline(
        r#"
struct FooId(u64)
const ID: FooId = FooId(42u64);

fn main() u64 {
    match ID {
        FooId(value) => value,
    }
}
"#,
    );
    assert!(checked.diagnostics.is_empty(), "{:?}", checked.diagnostics);
}

#[test]
fn tuple_projections_preserve_place_and_address_semantics() {
    let checked = pipeline(
        r#"
fn main() i32 {
    let mut nested = ((40, 2), ());
    nested.0.0 = nested.0.0 + nested.0.1;
    let value = &nested.0.0;
    let unit = &nested.1;
    _ = unit;
    value.*
}
"#,
    );

    assert!(checked.diagnostics.is_empty(), "{:?}", checked.diagnostics);
}

#[test]
fn const_tuple_projection_supports_nested_fields() {
    let checked = pipeline(
        r#"
const ANSWER: i32 = ((40, 2), ()).0.1;

fn main() i32 {
    ANSWER
}
"#,
    );

    assert!(checked.diagnostics.is_empty(), "{:?}", checked.diagnostics);
}

#[test]
fn materializes_const_tuples_optionals_and_error_unions_at_runtime() {
    let checked = pipeline(
        r#"
const PAIR: (i32, bool) = (40, true);
const SOME: ?i32 = ?2;
const NONE: ?i32 = null;
const OK: i32!i32 = !3;
const ERR: i32!i32 = 4!;

fn main() i32 {
    let some = match SOME { ?value => value, null => 0 };
    let none = match NONE { ?value => value, null => 0 };
    let ok = match OK { !value => value, error! => error };
    let err = match ERR { !value => value, error! => error };
    PAIR.0 + some + none + ok + err
}
"#,
    );

    assert!(checked.diagnostics.is_empty(), "{:?}", checked.diagnostics);
}

#[test]
fn tuple_projection_reports_target_and_bounds_errors() {
    let checked = pipeline(
        r#"
fn invalid(pair: (i32, bool), scalar: i32) {
    let past_pair = pair.2;
    let scalar_field = scalar.0;
    let unit_field = ().0;
}
"#,
    );

    for expected in [
        "tuple field index 2 is out of bounds for tuple of arity 2",
        "cannot project tuple field .0 from i32",
        "tuple field index 0 is out of bounds for tuple of arity 0",
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
fn tuple_patterns_work_in_binding_if_match_and_for_contexts() {
    let checked = pipeline(
        r#"
struct PairIter {}

extend PairIter : Iterator {
    type Item = (i32, i32);

    fn next(&mut self) ?(i32, i32) {
        null
    }
}

fn classify(pair: (i32, (bool, i32))) i32 {
    if pair is (40, (true, value)) {
        value
    } else {
        match pair {
            (left, (false, right)) => left + right,
            (_, (_, fallback)) => fallback,
        }
    }
}

fn main(pair: (i32, (bool, i32))) i32 {
    let mut (left, (enabled, right)) = pair;
    left += right;
    let (mut selected, fixed) = (1, 2);
    selected += fixed;
    let () = ();

    let mut total = left + selected;
    let mut iter = PairIter {};
    for (first, second) in iter {
        total += first + second;
    }

    if enabled { total + classify(pair) } else { total }
}
"#,
    );

    assert!(checked.diagnostics.is_empty(), "{:?}", checked.diagnostics);
}

#[test]
fn tuple_patterns_report_target_and_arity_mismatches() {
    let checked = pipeline(
        r#"
fn main(pair: (i32, bool), scalar: i32) {
    let (only,) = pair;
    let (left, right) = scalar;
    if pair is (value,) {
        _ = value;
    }
    match scalar {
        (value,) => value,
        _ => 0,
    };
}
"#,
    );

    for expected in [
        "binding pattern tuple arity mismatch: expected 2, found 1",
        "binding pattern requires a tuple value",
        "if pattern tuple arity mismatch: expected 2, found 1",
        "match pattern tuple pattern requires a tuple target",
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
