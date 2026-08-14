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
fn rejects_implicit_trailing_generic_argument_inference() {
    let checked = pipeline(
        r#"
fn keep_first[T, U](left: T, right: U) T {
    _ = right;
    left
}

fn main() i32 {
    keep_first[i32](7, true)
}
"#,
    );
    assert!(checked.diagnostics.iter().any(|diagnostic| {
        diagnostic
            .summary
            .contains("generic argument count mismatch: expected 2, got 1")
    }));
    assert!(checked.diagnostics.iter().any(|diagnostic| {
        diagnostic
            .summary
            .contains("use `_` for arguments that should be inferred")
    }));
}

#[test]
fn explicit_generic_inference_placeholders_cover_type_and_const_parameters() {
    let checked = pipeline(
        r#"
fn keep_first[T, U](left: T, right: U) T {
    _ = right;
    left
}

fn take_array[T, N: usize](xs: [N]T) usize {
    _ = xs;
    N
}

fn main(xs: [4]u8) i32 {
    keep_first[i32, _](7, true) + take_array[_, _](xs) as i32
}
"#,
    );
    assert!(checked.diagnostics.is_empty(), "{:?}", checked.diagnostics);
}

#[test]
fn infers_const_generic_array_lengths_from_call_arguments() {
    let checked = pipeline(
        r#"
fn take_array[T, N: usize](xs: [N]T) usize {
    _ = xs;
    0usize
}

fn main(xs: [4]u8) usize {
    take_array(xs) + take_array[u8, 4](xs) + take_array[u8, _](xs)
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
        3,
        "{:?}",
        checked
            .facts
            .iter_generic_instantiations()
            .collect::<Vec<_>>()
    );
}

#[test]
fn infers_const_generic_array_lengths_from_array_literal_arguments() {
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
    let checked = pipeline_with_len_provider(
        r#"
trait DecodeFrom[Input] {
    fn decode(input: Input) Self;
}

struct Wrapped {}

struct Bytes {
    data: [3]u8,
}

fn decode[T, Input](input: Input) T
where T: DecodeFrom[Input]
{
    [T]::decode(input)
}

extend i32 : DecodeFrom[&[char]] {
    fn decode(input: &[char]) i32 {
        input.len() as i32
    }
}

extend i32 : DecodeFrom[&[u8]] {
    fn decode(input: &[u8]) i32 {
        input.len() as i32
    }
}

extend i32 : DecodeFrom[Wrapped] {
    fn decode(input: Wrapped) i32 {
        _ = input;
        0
    }
}

fn main() i32 {
    let bytes: [3]u8 = [1, 2, 3];
    let wrapped = Bytes { data: bytes };
    decode[i32, _](&"abc") + decode[i32, _](&bytes) + decode[i32, _](&wrapped.data)
}
"#,
    );
    assert!(checked.diagnostics.is_empty(), "{:?}", checked.diagnostics);
}

#[test]
fn resolves_receiver_parse_facade_through_result_protocol() {
    let checked = pipeline_with_len_provider(
        r#"
trait From[Input] {
    fn from(input: Input) i32!Self;
}

extend[Unit] [Unit]
where Unit: Sized
{
    fn parse[T](&self) i32!T
    where T: From[&[Unit]]
    {
        [T]::from(self)
    }
}

extend i32 : From[&[char]] {
    fn from(input: &[char]) i32!i32 {
        if input.len() == 0 {
            1!
        } else {
            !(input.len() as i32)
        }
    }
}

fn main() i32 {
    switch (&"nia").parse[i32]() {
        !value => value,
        _ => 0,
    }
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
