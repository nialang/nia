// SPDX-License-Identifier: GPL-3.0-or-later
use super::common::*;

#[test]
fn checks_return_tail_and_local_binding_types() {
    let checked = pipeline(
        r#"
fn add(a: i32, b: i32) i32 {
    var sum: i32 = a + b;
    sum
}
"#,
    );
    assert!(checked.diagnostics.is_empty(), "{:?}", checked.diagnostics);
}

#[test]
fn checks_union_literals_and_field_access() {
    let checked = pipeline(
        r#"
union Bits[T] {
    i: i64,
    value: T,
}

fn main() i32 {
    var bits: Bits[i32] = { value: 10 };
    bits.value
}
"#,
    );
    assert!(checked.diagnostics.is_empty(), "{:?}", checked.diagnostics);

    let bad_empty = pipeline(
        r#"
union Bits {
    i: i32,
}

fn main() i32 {
    var bits: Bits = {};
    0
}
"#,
    );
    assert!(
        bad_empty
            .diagnostics
            .iter()
            .any(|diagnostic| diagnostic.summary.contains("exactly one field")),
        "{:?}",
        bad_empty.diagnostics
    );

    let bad_multi = pipeline(
        r#"
union Bits {
    i: i32,
    f: f32,
}

fn main() i32 {
    var bits: Bits = { i: 1, f: 2.0 };
    0
}
"#,
    );
    assert!(
        bad_multi
            .diagnostics
            .iter()
            .any(|diagnostic| diagnostic.summary.contains("exactly one field")),
        "{:?}",
        bad_multi.diagnostics
    );
}

#[test]
fn checks_void_values_empty_structs_and_void_pointers() {
    let checked = pipeline(
        r#"
struct Empty {}

fn take_void(p: &void) {}
fn take_read_void(p: &void) {}

fn main() {
    var unit: void = {};
    var empty: Empty = {};
    var value: i32 = 1;
    take_void(&value as &void);
    take_read_void(&value as &void);
    unit
}
"#,
    );
    assert!(checked.diagnostics.is_empty(), "{:?}", checked.diagnostics);

    let bad_implicit = pipeline(
        r#"
fn main() {
    var value: i32 = 1;
    var ptr: &void = &value;
}
"#,
    );
    assert!(
        bad_implicit
            .diagnostics
            .iter()
            .any(|diagnostic| diagnostic.summary.contains("binding initializer")),
        "{:?}",
        bad_implicit.diagnostics
    );

    let bad_deref = pipeline(
        r#"
fn main() i32 {
    var value: i32 = 1;
    var ptr: &void = &value as &void;
    ptr.*
}
"#,
    );
    assert!(
        bad_deref
            .diagnostics
            .iter()
            .any(|diagnostic| diagnostic.summary.contains("cannot dereference `&void`")),
        "{:?}",
        bad_deref.diagnostics
    );
}

#[test]
fn checks_volatile_pointer_deref_and_readonly_writes() {
    let checked = pipeline(
        r#"
fn read_reg(reg: ^u32) u32 {
    reg.*
}

fn write_reg(reg: ^mut u32, value: u32) void {
    reg.* = value;
}

fn cast_reg(addr: usize) ^mut u32 {
    addr as ^mut u32
}

fn readonly_from_mut(reg: ^mut u32) ^u32 {
    reg
}
"#,
    );
    assert!(checked.diagnostics.is_empty(), "{:?}", checked.diagnostics);

    let bad = pipeline(
        r#"
fn write_readonly(reg: ^u32, value: u32) void {
    reg.* = value;
}
"#,
    );
    assert!(
        bad.diagnostics
            .iter()
            .any(|diagnostic| diagnostic.summary.contains("pointer is read-only")),
        "{:?}",
        bad.diagnostics
    );
}

#[test]
fn accepts_explicit_return_without_tail_expression() {
    let checked = pipeline(
        r#"
extern fn printf(fmt: &u8, ...);

fn main() i32 {
    var hello = b"hello, world!\n\0";
    printf(&(hello.*[0]));
    return 0;
}
"#,
    );
    assert!(checked.diagnostics.is_empty(), "{:?}", checked.diagnostics);
}

#[test]
fn reports_mismatched_return_and_binding_types() {
    let checked = pipeline(
        r#"
fn bad(flag: bool) i32 {
    var x: bool = 1;
    flag
}
"#,
    );
    assert!(checked.diagnostics.len() >= 2, "{:?}", checked.diagnostics);
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
            .any(|diagnostic| diagnostic.summary.contains("function body"))
    );
}

#[test]
fn checks_optional_and_error_union_construction_and_propagation() {
    let checked = pipeline(
        r#"
fn maybe(flag: bool) ?i32 {
    if flag {
        return ?1;
    }
    null
}

fn ok(value: i32) i32!i32 {
    !value
}

fn fail(error: i32) i32!i32 {
    error!
}

fn use_error(value: i32!i32) i32!i32 {
    var unwrapped = value.?;
    !unwrapped
}
"#,
    );

    assert!(checked.diagnostics.is_empty(), "{:?}", checked.diagnostics);
}

#[test]
fn rejects_error_union_construction_without_expected_type() {
    let checked = pipeline(
        r#"
fn bad() i32 {
    var a = !10i32;
    var b = 20i32!;
    a + b
}
"#,
    );

    assert!(
        checked.diagnostics.iter().any(|diagnostic| diagnostic
            .summary
            .contains("requires an expected error union type")),
        "{:?}",
        checked.diagnostics
    );
}

#[test]
fn checks_optional_and_error_union_if_patterns() {
    let checked = pipeline(
        r#"
fn unwrap_optional(value: ?i32) i32 {
    if let ?x = value {
        x
    } else null {
        0
    }
}

fn unwrap_error(value: i32!i32) i32 {
    if let !x = value {
        x
    } else e! {
        e
    }
}
"#,
    );

    assert!(checked.diagnostics.is_empty(), "{:?}", checked.diagnostics);

    let missing = pipeline(
        r#"
fn bad(value: ?i32) i32 {
    let unwrapped: i32 = if let ?x = value {
        x
    };
    unwrapped
}
"#,
    );
    assert!(
        missing
            .diagnostics
            .iter()
            .any(|diagnostic| diagnostic.summary.contains("non-exhaustive if pattern")),
        "{:?}",
        missing.diagnostics
    );

    let wrong_target = pipeline(
        r#"
fn bad(value: i32) i32 {
    if let ?x = value {
        x
    } else _ {
        0
    }
}
"#,
    );
    assert!(
        wrong_target
            .diagnostics
            .iter()
            .any(|diagnostic| diagnostic.summary.contains("requires an optional target")),
        "{:?}",
        wrong_target.diagnostics
    );
}
