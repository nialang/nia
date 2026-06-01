// SPDX-License-Identifier: GPL-3.0-or-later
use super::common::*;

#[test]
fn infers_generic_function_type_arguments_from_call_arguments() {
    let checked = pipeline(
        r#"
struct Box[T] {
    value: T,
}

fn id[T](value: T) T { value }
fn unbox[T](box: Box[T]) T { box.value }
fn deref_id[T](value: &T) T { value.* }
fn choose[T](left: T, right: T) T { left }

fn main(box: Box[i32], ptr: &const i32, flag: bool) i32 {
    var a: i32 = id(1);
    var b: i32 = unbox(box);
    var c: i32 = deref_id(ptr);
    _ = choose(1, flag);
    a + b + c
}
"#,
    );
    assert!(
        !checked
            .diagnostics
            .iter()
            .any(|diagnostic| diagnostic.message.contains("binding initializer"))
    );
    assert!(checked.diagnostics.iter().any(|diagnostic| {
        diagnostic
            .message
            .contains("conflicting inferred type for generic parameter `T`")
    }));
}

#[test]
fn infers_generic_function_type_arguments_from_expected_return_type() {
    let checked = pipeline(
        r#"
fn id[T](value: T) T { value }
fn choose[T](left: T, right: T) T { left }

fn from_return() i32 {
    id(1)
}

fn main() i32 {
    var a: i32 = id(1);
    var b: usize = id(1);
    var c: i32 = choose(id(1), 2);
    _ = id(1);
    a + b as i32 + c + from_return()
}
"#,
    );
    assert!(
        !checked
            .diagnostics
            .iter()
            .any(|diagnostic| diagnostic.message.contains("binding initializer")),
        "{:?}",
        checked.diagnostics
    );
    assert!(
        !checked
            .diagnostics
            .iter()
            .any(|diagnostic| diagnostic.message.contains("function body")),
        "{:?}",
        checked.diagnostics
    );
    assert_eq!(
        checked
            .diagnostics
            .iter()
            .filter(|diagnostic| diagnostic
                .message
                .contains("cannot infer generic parameter `T`"))
            .count(),
        0,
        "{:?}",
        checked.diagnostics
    );
}

#[test]
fn checks_fixed_prefix_of_variadic_extern_calls() {
    let checked = pipeline(
        r#"
extern fn printf(fmt: &u8, ...);

fn main(flag: bool) i32 {
    _ = printf(flag, 1);
    var s = c"hello";
    printf(&s[0], &s[..]);
    var sp = &(&s[..]);
    printf(&s[0], sp.*);
    0
}
"#,
    );
    assert!(
        checked
            .diagnostics
            .iter()
            .any(|diagnostic| diagnostic.message.contains("call argument"))
    );
    assert!(
        checked
            .diagnostics
            .iter()
            .all(|diagnostic| !diagnostic.message.contains("variadic argument")),
        "{:?}",
        checked.diagnostics
    );
    assert!(
        checked
            .diagnostics
            .iter()
            .all(|diagnostic| !diagnostic.message.contains("argument count mismatch"))
    );
}
