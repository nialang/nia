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

fn take_array[T, N: usize](xs: [T; N]) usize {
    _ = xs;
    N
}

fn main(xs: [u8; 4]) i32 {
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
fn take_array[T, N: usize](xs: [T; N]) usize {
    _ = xs;
    0usize
}

fn main(xs: [u8; 4]) usize {
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
fn trait_object_vtable_instantiations_retain_const_arguments() {
    let checked = pipeline(
        r#"
trait Scaled[N: usize] {
    fn value(& self) usize { 8usize }
}

struct Meter {}

extend[N: usize] Meter : Scaled[N] {}

fn read(value: & Scaled[8]) usize {
    value.value()
}

fn main() usize {
    let meter = Meter {};
    read(& meter)
}
"#,
    );
    assert!(checked.diagnostics.is_empty(), "{:?}", checked.diagnostics);
    let const_instances = checked
        .facts
        .iter_generic_instantiations()
        .filter(|instantiation| !instantiation.const_args.is_empty())
        .collect::<Vec<_>>();
    assert_eq!(const_instances.len(), 1, "{const_instances:?}");
    assert!(const_instances.iter().all(|instantiation| matches!(
        instantiation.const_args.as_slice(),
        [nia_ty::ConstGenericArg {
            value: nia_ty::ConstGenericValue::Int(value),
            ..
        }] if value.bits() == 8
    )));
}

#[test]
fn infers_const_generic_array_lengths_from_array_literal_arguments() {
    let checked = pipeline(
        r#"
fn take_array[T, N: usize](xs: [T; N]) usize {
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
fn infers_type_and_const_generics_through_nominal_arguments() {
    let checked = pipeline(
        r#"
struct Buffer[T, N: usize] {
    value: T,
    items: [T; N],
}

fn inspect[T, N: usize](value: Buffer[T, N]) () {
    _ = value;
}

fn main(value: Buffer[i32, 4]) () {
    inspect(value)
}
"#,
    );

    assert!(checked.diagnostics.is_empty(), "{:?}", checked.diagnostics);
    let instance = checked
        .facts
        .iter_generic_instantiations()
        .find(|instance| !instance.const_args.is_empty())
        .expect("mixed type/const generic instantiation");
    assert_eq!(
        checked.type_store.get(instance.args[0]),
        Some(&nia_ty::TyKind::Primitive(nia_ty::PrimitiveTy::I32))
    );
    assert!(matches!(
        instance.const_args.as_slice(),
        [nia_ty::ConstGenericArg {
            value: nia_ty::ConstGenericValue::Int(value),
            ..
        }] if value.bits() == 4
    ));
}

#[test]
fn infers_type_generics_from_trait_object_associated_binding_keys() {
    let checked = pipeline(
        r#"
trait Parent[T] {
    type Item;
}

trait Child : Parent[i32] {}

fn inspect[T](value: &Child[[Self as Parent[T]]::Item = T]) () {
    _ = value;
}

fn main(value: &Child[[Self as Parent[i32]]::Item = i32]) () {
    inspect(value)
}
"#,
    );

    assert!(checked.diagnostics.is_empty(), "{:?}", checked.diagnostics);
    let instance = checked
        .facts
        .iter_generic_instantiations()
        .find(|instance| !instance.args.is_empty())
        .expect("associated-binding-key generic instantiation");
    assert_eq!(
        checked.type_store.get(instance.args[0]),
        Some(&nia_ty::TyKind::Primitive(nia_ty::PrimitiveTy::I32))
    );
}

#[test]
fn associated_binding_inference_skips_value_mismatch_candidates() {
    let checked = pipeline(
        r#"
trait Parent[T] {
    type Item;
}

trait Child : Parent[i32] + Parent[bool] {}

fn inspect[T, U](expected: U, value: &Child[
    [Self as Parent[T]]::Item = U,
    [Self as Parent[bool]]::Item = i32,
]) () {
    _ = (expected, value);
}

fn main(value: &Child[
    [Self as Parent[i32]]::Item = bool,
    [Self as Parent[bool]]::Item = i32,
]) () {
    inspect(0i32, value)
}
"#,
    );
    assert!(checked.diagnostics.is_empty(), "{:?}", checked.diagnostics);
    let instance = checked
        .facts
        .iter_generic_instantiations()
        .find(|instance| !instance.args.is_empty())
        .expect("associated-binding value candidate instantiation");
    assert_eq!(
        checked.type_store.get(instance.args[0]),
        Some(&nia_ty::TyKind::Primitive(nia_ty::PrimitiveTy::Bool))
    );
}

#[test]
fn infers_const_generic_lengths_through_tuple_arguments() {
    let checked = pipeline(
        r#"
fn tupleLen[N: usize](value: ([i32; N], bool)) usize {
    _ = value;
    N
}

fn main(values: [i32; 3]) usize {
    tupleLen((values, true))
}
"#,
    );

    assert!(checked.diagnostics.is_empty(), "{:?}", checked.diagnostics);
    let instance = checked
        .facts
        .iter_generic_instantiations()
        .find(|instance| !instance.const_args.is_empty())
        .expect("const generic tuple instantiation");
    assert!(matches!(
        instance.const_args.as_slice(),
        [nia_ty::ConstGenericArg {
            value: nia_ty::ConstGenericValue::Int(value),
            ..
        }] if value.bits() == 3
    ));
}

#[test]
fn infers_const_generic_lengths_through_callable_arguments() {
    let checked = pipeline(
        r#"
fn callableLen[N: usize](callback: &Fn([i32; N]) i32) usize {
    _ = callback;
    N
}

fn main(callback: &Fn([i32; 4]) i32) usize {
    callableLen(callback)
}
"#,
    );

    assert!(checked.diagnostics.is_empty(), "{:?}", checked.diagnostics);
    let instance = checked
        .facts
        .iter_generic_instantiations()
        .find(|instance| !instance.const_args.is_empty())
        .expect("const generic callable instantiation");
    assert!(matches!(
        instance.const_args.as_slice(),
        [nia_ty::ConstGenericArg {
            value: nia_ty::ConstGenericValue::Int(value),
            ..
        }] if value.bits() == 4
    ));
}

#[test]
fn reconstructed_trait_object_types_match_structurally() {
    let checked = pipeline(
        r#"
trait Consumer[T] {
    fn consume(&self, value: T) T;
}

fn identity[T](value: &Consumer[T]) &Consumer[T] {
    value
}

fn main(value: &Consumer[i32]) &Consumer[i32] {
    identity(value)
}
"#,
    );

    assert!(checked.diagnostics.is_empty(), "{:?}", checked.diagnostics);
}

#[test]
fn incompatible_pointer_does_not_seed_const_generic_length() {
    let checked = pipeline(
        r#"
fn selectLen[N: usize](first: &mut [i32; N], second: [i32; N]) usize {
    _ = first;
    _ = second;
    N
}

fn main(readonly: &[i32; 2], values: [i32; 3]) usize {
    selectLen(readonly, values)
}
"#,
    );

    assert!(!checked.diagnostics.is_empty());
    assert!(
        checked.diagnostics.iter().all(|diagnostic| !diagnostic
            .summary
            .contains("conflicting inferred value for const generic parameter")),
        "{:?}",
        checked.diagnostics
    );
}

#[test]
fn incompatible_late_tuple_field_does_not_seed_const_generic_length() {
    let checked = pipeline(
        r#"
fn tupleLen[N: usize](value: ([i32; N], &mut i32)) usize {
    _ = value;
    N
}

fn main(values: [i32; 2], readonly: &i32) usize {
    tupleLen((values, readonly))
}
"#,
    );

    assert!(!checked.diagnostics.is_empty());
    assert!(
        checked
            .facts
            .iter_generic_instantiations()
            .all(|instance| instance.const_args.is_empty()),
        "an incompatible tuple must not create a const-generic instance"
    );
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
    data: [u8; 3],
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
    let bytes: [u8; 3] = [1, 2, 3];
    let wrapped = Bytes { data: bytes };
    decode[i32, _](&"abc") + decode[i32, _](&bytes) + decode[i32, _](&wrapped.data)
}
"#,
    );
    assert!(checked.diagnostics.is_empty(), "{:?}", checked.diagnostics);
}

#[test]
fn rejects_call_when_where_bound_associated_type_binding_mismatches() {
    let checked = pipeline(
        r#"
trait Source {
    type Item;
}

struct Good {}
struct Bad {}

extend Good : Source {
    type Item = i32;
}

extend Bad : Source {
    type Item = bool;
}

fn require[T](value: T) ()
where T: Source[Item = i32]
{
    _ = value;
}

fn main(good: Good, bad: Bad) () {
    require(good);
    require(bad);
}
"#,
    );

    assert_eq!(
        checked
            .diagnostics
            .iter()
            .filter(|diagnostic| diagnostic
                .summary
                .contains("associated type binding not satisfied"))
            .count(),
        1,
        "{:?}",
        checked.diagnostics
    );
    assert!(checked.diagnostics.iter().any(|diagnostic| {
        diagnostic.summary.contains("Item") && diagnostic.summary.contains("expected i32, got bool")
    }));
}

#[test]
fn rejects_nominal_type_when_where_bound_associated_type_binding_mismatches() {
    let checked = pipeline(
        r#"
trait Source {
    type Item;
}

struct Good {}
struct Bad {}

extend Good : Source {
    type Item = i32;
}

extend Bad : Source {
    type Item = bool;
}

struct Holder[T]
where T: Source[Item = i32]
{
    value: T,
}

fn accept(value: Holder[Good]) () {
    _ = value;
}

fn reject(value: Holder[Bad]) () {
    _ = value;
}
"#,
    );

    assert_eq!(
        checked
            .diagnostics
            .iter()
            .filter(|diagnostic| diagnostic
                .summary
                .contains("trait bound not satisfied: Bad: Source"))
            .count(),
        1,
        "{:?}",
        checked.diagnostics
    );
    assert!(checked.diagnostics.iter().all(|diagnostic| {
        !diagnostic
            .summary
            .contains("trait bound not satisfied: Good: Source")
    }));
}

#[test]
fn malformed_trait_impl_type_arity_cannot_infer_where_candidate() {
    let checked = pipeline_with_program_trait_impls(
        r#"
trait Marker {}

extend i32 : Marker {}

fn make[T]() i32
where T: Marker
{
    0
}

fn main() i32 {
    make[_]()
}
"#,
        |trait_impls| {
            let impl_signature = trait_impls
                .iter_mut()
                .next()
                .expect("Marker implementation");
            impl_signature.trait_args.push(impl_signature.target_ty);
        },
    );

    assert!(
        checked.diagnostics.iter().any(|diagnostic| {
            diagnostic
                .summary
                .contains("cannot infer generic parameter `T`")
        }),
        "{:?}",
        checked.diagnostics
    );
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
    match (&"nia").parse[i32]() {
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
