// SPDX-License-Identifier: GPL-3.0-or-later
use super::common::*;

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
fn tuple_patterns_work_in_binding_if_switch_and_for_contexts() {
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
        switch pair {
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
    switch scalar {
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
        "switch pattern tuple pattern requires a tuple target",
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
