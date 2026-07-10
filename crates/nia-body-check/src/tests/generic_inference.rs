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

fn main(box: Box[i32], ptr: & i32, flag: bool) i32 {
    let mut a: i32 = id(1);
    let mut b: i32 = unbox(box);
    let mut c: i32 = deref_id(ptr);
    _ = choose(1, flag);
    a + b + c
}
"#,
    );
    assert!(
        !checked
            .diagnostics
            .iter()
            .any(|diagnostic| diagnostic.summary.contains("binding initializer"))
    );
    assert!(checked.diagnostics.iter().any(|diagnostic| {
        diagnostic
            .summary
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
    let mut a: i32 = id(1);
    let mut b: usize = id(1);
    let mut c: i32 = choose(id(1), 2);
    _ = id(1);
    a + b as i32 + c + from_return()
}
"#,
    );
    assert!(
        !checked
            .diagnostics
            .iter()
            .any(|diagnostic| diagnostic.summary.contains("binding initializer")),
        "{:?}",
        checked.diagnostics
    );
    assert!(
        !checked
            .diagnostics
            .iter()
            .any(|diagnostic| diagnostic.summary.contains("function body")),
        "{:?}",
        checked.diagnostics
    );
    assert_eq!(
        checked
            .diagnostics
            .iter()
            .filter(|diagnostic| diagnostic
                .summary
                .contains("cannot infer generic parameter `T`"))
            .count(),
        0,
        "{:?}",
        checked.diagnostics
    );
}

#[test]
fn infers_remaining_generic_function_type_arguments_after_explicit_prefix() {
    let checked = pipeline(
        r#"
fn keep_first[T, U](left: T, right: U) T {
    _ = right;
    left
}

fn main() i32 {
    let mut a: i32 = keep_first[i32](7, true);
    let mut b: u8 = keep_first[u8](3u8, 123usize);
    a + b as i32
}
"#,
    );
    assert!(checked.diagnostics.is_empty(), "{:?}", checked.diagnostics);
}

#[test]
fn infers_comptime_generic_array_lengths_from_call_arguments() {
    let checked = pipeline(
        r#"
fn take_array[T, N: usize](xs: [N]T) usize {
    _ = xs;
    0usize
}

fn main(xs: [4]u8) usize {
    take_array(xs) + take_array[u8, 4](xs)
}
"#,
    );
    assert!(checked.diagnostics.is_empty(), "{:?}", checked.diagnostics);
    let const_instance_count = checked
        .facts
        .iter_generic_instantiations()
        .filter(|inst| !inst.const_args.is_empty())
        .count();
    assert_eq!(
        const_instance_count,
        2,
        "{:?}",
        checked
            .facts
            .iter_generic_instantiations()
            .collect::<Vec<_>>()
    );
}

#[test]
fn infers_comptime_generic_array_lengths_from_array_literal_arguments() {
    let checked = pipeline(
        r#"
fn take_array[T, N: usize](xs: [N]T) usize {
    _ = xs;
    0usize
}

fn main() usize {
    take_array([1u8, 2u8, 3u8, 4u8])
}
"#,
    );
    assert!(checked.diagnostics.is_empty(), "{:?}", checked.diagnostics);
    let const_instance = checked
        .facts
        .iter_generic_instantiations()
        .find(|inst| !inst.const_args.is_empty())
        .expect("const generic instantiation");
    assert_eq!(const_instance.const_args.len(), 1);
    assert!(matches!(
        const_instance.const_args[0].value,
        nia_ty::ConstGenericValue::Int(value) if value.bits() == 4
    ));
}

#[test]
fn infers_range_bound_literals_from_peer_bound_type() {
    let checked = pipeline(
        r#"
fn take_range[T](bounds: T..T) T {
    bounds.end()
}

fn main(count: usize) usize {
    let mut from_suffix = take_range(0..4usize);
    let mut from_variable = take_range(0..count);
    from_suffix + from_variable
}
"#,
    );
    assert!(checked.diagnostics.is_empty(), "{:?}", checked.diagnostics);
}

#[test]
fn infers_generic_function_input_from_where_predicate_impl_candidates() {
    let checked = pipeline(
        r#"
trait ParseFrom[Input] {
    fn parse_from(input: Input) Self;
}

fn parse[T, Input](input: Input) T
where T: ParseFrom[Input]
{
    [T]::parse_from(input)
}

extend i32 : ParseFrom[&[char]] {
    fn parse_from(input: &[char]) i32 {
        input.len() as i32
    }
}

extend i32 : ParseFrom[&[u8]] {
    fn parse_from(input: &[u8]) i32 {
        input.len() as i32
    }
}

fn main() i32 {
    parse[i32](&"abc")
}
"#,
    );
    assert!(checked.diagnostics.is_empty(), "{:?}", checked.diagnostics);
}

#[test]
fn checks_fixed_prefix_of_variadic_extern_calls() {
    let checked = pipeline(
        r#"
extern fn printf(fmt: &u8, ...);

fn main(flag: bool) i32 {
    _ = printf(flag, 1);
    let mut s = b"hello\0";
    printf(&s[0], s);
    let mut sp = &s;
    printf(&s[0], sp.*);
    0
}
"#,
    );
    assert!(
        checked
            .diagnostics
            .iter()
            .any(|diagnostic| diagnostic.summary.contains("call argument"))
    );
    assert!(
        checked
            .diagnostics
            .iter()
            .all(|diagnostic| !diagnostic.summary.contains("variadic argument")),
        "{:?}",
        checked.diagnostics
    );
    assert!(
        checked
            .diagnostics
            .iter()
            .all(|diagnostic| !diagnostic.summary.contains("argument count mismatch"))
    );
}
