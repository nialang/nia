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

fn take_pair(value: Pair[i32, usize]) void {}
fn take_read_point_ptr(value: &Point) void {}
fn take_array(value: [3]i32) void {}
fn take_slice(value: &[i32]) void {}
fn take_fn_ptr(value: &fn(i32, usize) bool) void {}
fn pred(value: i32, width: usize) void {}

fn main(value: void, ptr: &u8) void {
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
            .any(|message| message.contains("invalid cast: cannot cast void to usize")),
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
