// SPDX-License-Identifier: GPL-3.0-or-later
use super::common::*;

#[test]
fn checks_unary_operator_builtin_traits() {
    let checked = pipeline(
        r#"
fn main(flag: bool, bits: u32, x: i32) bool {
    var neg = -x;
    var flipped = ~bits;
    var logical = !flag;
    neg < 0 and flipped != bits and logical
}
"#,
    );
    assert!(checked.diagnostics.is_empty(), "{:?}", checked.diagnostics);
}

#[test]
fn rejects_unary_operators_without_builtin_trait_impls() {
    let checked = pipeline(
        r#"
fn main(flag: bool, x: i32) bool {
    var bad_bits = ~flag;
    var bad_not = !x;
    bad_bits or bad_not
}
"#,
    );
    assert!(
        checked.diagnostics.iter().any(|diagnostic| diagnostic
            .message
            .contains("trait bound not satisfied: bool: BitNot")),
        "{:?}",
        checked.diagnostics
    );
    assert!(
        checked.diagnostics.iter().any(|diagnostic| diagnostic
            .message
            .contains("trait bound not satisfied: i32: Not")),
        "{:?}",
        checked.diagnostics
    );
}

#[test]
fn generic_layout_builtins_require_sized_bound() {
    let checked = pipeline(
        r#"
fn bytes[T]() usize
where T: Sized {
    @size[T]() + @align[T]()
}

fn main() usize {
    bytes[i32]()
}
"#,
    );
    assert!(checked.diagnostics.is_empty(), "{:?}", checked.diagnostics);

    let rejected = pipeline(
        r#"
fn bytes[T]() usize {
    @size[T]()
}

fn main() usize {
    bytes[i32]()
}
"#,
    );
    assert!(
        rejected
            .diagnostics
            .iter()
            .any(|diagnostic| diagnostic.message.contains("requires T: Sized")),
        "{:?}",
        rejected.diagnostics
    );
}

#[test]
fn checks_open_enum_integer_casts_and_switch_exhaustiveness() {
    let checked = pipeline(
        r#"
enum Flag: u8 {
    A,
    B,
    _,
}

enum Closed {
    A,
    B,
}

fn open_cast() Flag {
    3 as Flag
}

fn bad_open_cast() Flag {
    256 as Flag
}

fn closed_cast() Closed {
    3 as Closed
}

fn missing_open_default(flag: Flag) i32 {
    switch flag {
        Flag::A => return 1,
        Flag::B => return 2,
    }
    0
}

fn with_open_default(flag: Flag) i32 {
    switch flag {
        Flag::A => return 1,
        Flag::B => return 2,
        3 => return 3,
        256 => return 4,
        _ => return 0,
    }
    0
}

fn closed_integer_pattern(closed: Closed) i32 {
    switch closed {
        0 => return 0,
        _ => return 1,
    }
    0
}
"#,
    );
    assert_eq!(
        checked
            .diagnostics
            .iter()
            .filter(|diagnostic| diagnostic.message.contains("invalid cast"))
            .count(),
        1,
        "{:?}",
        checked.diagnostics
    );
    assert_eq!(
        checked
            .diagnostics
            .iter()
            .filter(|diagnostic| diagnostic
                .message
                .contains("non-exhaustive open enum switch"))
            .count(),
        1,
        "{:?}",
        checked.diagnostics
    );
    assert!(
        !checked
            .diagnostics
            .iter()
            .any(|diagnostic| diagnostic.message.contains("i32 to Flag")),
        "{:?}",
        checked.diagnostics
    );
    assert_eq!(
        checked
            .diagnostics
            .iter()
            .filter(|diagnostic| diagnostic
                .message
                .contains("out of range for Flag backing type"))
            .count(),
        2,
        "{:?}",
        checked.diagnostics
    );
    assert!(
        checked.diagnostics.iter().any(|diagnostic| diagnostic
            .message
            .contains("type mismatch in switch pattern")),
        "{:?}",
        checked.diagnostics
    );
}

#[test]
fn checks_shift_operators() {
    let checked = pipeline(
        r#"
fn main(flag: bool) i32 {
    var x = 1 << 3;
    var y = x >> 1;
    var z = x << flag;
    var bad = flag << 1;
    y + z + bad
}
"#,
    );
    assert!(
        checked.diagnostics.iter().any(|diagnostic| diagnostic
            .message
            .contains("trait bound not satisfied: i32: Shl[bool]")),
        "{:?}",
        checked.diagnostics
    );
    assert!(
        checked.diagnostics.iter().any(|diagnostic| diagnostic
            .message
            .contains("trait bound not satisfied: bool: Shl[i32]")),
        "{:?}",
        checked.diagnostics
    );
}

#[test]
fn checks_global_initializers_and_inferred_global_types() {
    let checked = pipeline(
        r#"
var counter = 1;
var flag = true;
const limit = 10;
var bad: bool = 1;

fn main() i32 {
    counter = counter + 1;
    limit = 11;
    if flag { counter } else { limit }
}
"#,
    );
    assert!(
        checked
            .diagnostics
            .iter()
            .any(|diagnostic| diagnostic.message.contains("global initializer"))
    );
    assert!(
        checked
            .diagnostics
            .iter()
            .any(|diagnostic| diagnostic.message.contains("global is const"))
    );
    assert!(
        !checked
            .diagnostics
            .iter()
            .any(|diagnostic| diagnostic.message.contains("global type is not available"))
    );
}
