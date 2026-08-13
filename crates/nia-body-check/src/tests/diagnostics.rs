// SPDX-License-Identifier: GPL-3.0-or-later
use super::common::*;

#[test]
fn renders_source_type_names_in_diagnostics() {
    let checked = pipeline(
        r#"
struct Point {
    x: i32,
}

struct Pair[A, B] {
    first: A,
    second: B,
}

fn take_pair(value: Pair[i32, usize]) () {}
fn take_read_point_ptr(value: &Point) () {}
fn take_array(value: [3]i32) () {}
fn take_slice(value: &[i32]) () {}
fn take_fn_ptr(value: &fn(i32, usize) bool) () {}
fn pred(value: i32, width: usize) () {}

fn main(value: (), ptr: &u8) () {
    let mut short = [1, 2];
    _ = value as usize;
    take_pair(true);
    take_read_point_ptr(ptr);
    take_array(short);
    take_slice(true);
    take_fn_ptr(& pred);
}
"#,
    );
    let messages = checked
        .diagnostics
        .iter()
        .map(|diagnostic| diagnostic.summary.as_str())
        .collect::<Vec<_>>();
    assert!(
        messages
            .iter()
            .any(|message| message.contains("invalid cast: cannot cast () to usize")),
        "{:?}",
        checked.diagnostics
    );
    assert!(
        messages
            .iter()
            .any(|message| message.contains("expected Pair[i32, usize], got bool")),
        "{:?}",
        checked.diagnostics
    );
    assert!(
        messages
            .iter()
            .any(|message| message.contains("expected &Point, got &u8")),
        "{:?}",
        checked.diagnostics
    );
    assert!(
        messages
            .iter()
            .any(|message| message.contains("expected [3]i32, got [2]i32")),
        "{:?}",
        checked.diagnostics
    );
    assert!(
        messages
            .iter()
            .any(|message| message.contains("expected &[i32], got bool")),
        "{:?}",
        checked.diagnostics
    );
    assert!(
        messages
            .iter()
            .any(|message| message.contains("expected &fn(i32, usize) bool")),
        "{:?}",
        checked.diagnostics
    );
}

#[test]
fn type_checks_closure_state_and_body_without_panicking() {
    let checked = pipeline(
        r#"
fn main(base: i32) () {
    let callback = \[base] value: i32 -> { base + value };
    ()
}
"#,
    );
    assert!(checked.diagnostics.is_empty(), "{:?}", checked.diagnostics);
    let body = checked
        .ir
        .function_bodies
        .values()
        .next()
        .expect("main body");
    let nia_body_ir::TypedStmtKind::Binding(binding) = &body.stmts[0].kind else {
        panic!("expected closure binding");
    };
    let Some(value) = &binding.value else {
        panic!("expected closure value");
    };
    let nia_body_ir::TypedExprKind::Closure {
        captures,
        params,
        body,
        ..
    } = &value.kind
    else {
        panic!("expected typed closure");
    };
    assert_eq!(captures.len(), 1);
    assert_eq!(params.len(), 1);
    assert!(body.tail.is_some());
}

#[test]
fn generic_inference_reuses_closure_identity_for_one_source_node() {
    let checked = pipeline(
        r#"
fn identity[T](value: T) T
where T: Sized
{
    value
}

fn main(base: i32) i32 {
    let callback = identity(\[base] value: i32 -> { base + value });
    callback(1)
}
"#,
    );

    assert!(checked.diagnostics.is_empty(), "{:?}", checked.diagnostics);
}
