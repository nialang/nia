// SPDX-License-Identifier: GPL-3.0-or-later
use super::common::*;

#[test]
fn recursive_associated_projection_normalization_terminates() {
    let checked = pipeline(
        r#"
trait Source[N: usize] {
    type Item;
}

struct Box[N: usize] {}

extend[N: usize] Box[N] : Source[N] {
    type Item = [Self as Source[N]]::Item;
}

fun main() i32 {
    let value: [Box[3] as Source[3]]::Item = 0;
    value
}
"#,
    );
    assert!(
        checked
            .diagnostics
            .iter()
            .all(|diagnostic| !diagnostic.summary.contains("stack overflow")),
        "recursive projections must terminate without overflowing: {:?}",
        checked.diagnostics
    );
}

#[test]
fn infers_generic_function_type_arguments_from_call_arguments() {
    let checked = pipeline(
        r#"
struct Box[T] {
    value: T,
}

fun id[T](value: T) T { value }
fun unbox[T](box: Box[T]) T { box.value }
fun deref_id[T](value: &T) T { value.* }
fun choose[T](left: T, right: T) T { left }

fun main(box: Box[i32], ptr: & i32, flag: bool) i32 {
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
fun id[T](value: T) T { value }
fun choose[T](left: T, right: T) T { left }

fun from_return() i32 {
    id(1)
}

fun main() i32 {
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
fn infers_generic_from_array_layout_builtin_and_marker() {
    let checked = pipeline(
        r#"
fun inspect[T](value: [u8; std::builtin::size[T]()], marker: T) T {
    _ = value;
    marker
}

fun main() i32 {
    inspect([1u8, 2u8, 3u8, 4u8], 1i32)
}
"#,
    );
    assert!(checked.diagnostics.is_empty(), "{:?}", checked.diagnostics);
}

#[test]
fn infers_generic_type_from_array_layout_builtin_operand() {
    let checked = pipeline(
        r#"
fun inspect[T](value: [u8; std::builtin::size[T]()]) usize {
    _ = value;
    0usize
}

fun caller(value: [u8; std::builtin::size[i32]()]) usize {
    inspect(value)
}

fun main(value: [u8; std::builtin::size[i32]()]) usize {
    caller(value)
}
"#,
    );
    assert!(
        checked.diagnostics.iter().all(|diagnostic| {
            !diagnostic
                .summary
                .contains("cannot infer generic parameter `T`")
        }),
        "{:?}",
        checked.diagnostics
    );
}

#[test]
fn rejects_array_layout_builtin_length_mismatch_after_type_inference() {
    let checked = pipeline(
        r#"
fun inspect[T](value: [u8; std::builtin::size[T]()], marker: T) T {
    _ = value;
    marker
}

fun main() i32 {
    inspect([1u8, 2u8, 3u8], 1i32)
}
"#,
    );
    assert!(!checked.diagnostics.is_empty(), "{:?}", checked.diagnostics);
}

#[test]
fn reports_missing_generic_type_hidden_in_array_layout_builtin() {
    let checked = pipeline(
        r#"
fun inspect[T](value: [u8; std::builtin::size[T]()]) usize {
    _ = value;
    0usize
}

fun main() usize {
    inspect([1u8, 2u8, 3u8, 4u8])
}
"#,
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
    assert!(
        checked.diagnostics.iter().all(|diagnostic| {
            !diagnostic
                .summary
                .contains("array length is not a valid constant")
        }),
        "{:?}",
        checked.diagnostics
    );
    let diagnostic = checked
        .diagnostics
        .iter()
        .find(|diagnostic| {
            diagnostic
                .summary
                .contains("cannot infer generic parameter `T`")
        })
        .expect("missing generic diagnostic");
    assert!(
        diagnostic
            .notes
            .iter()
            .any(|note| { note.contains("not determined by the call arguments") })
    );
    assert!(
        diagnostic
            .help
            .iter()
            .any(|help| { help.contains("provide an explicit type argument for `T`") })
    );
}

#[test]
fn explains_conflicting_generic_inference_with_actionable_help() {
    let checked = pipeline(
        r#"
fun choose[T](left: T, right: T) T { left }

fun main() i32 {
    choose(1, true)
}
"#,
    );
    let diagnostic = checked
        .diagnostics
        .iter()
        .find(|diagnostic| {
            diagnostic
                .summary
                .contains("conflicting inferred type for generic parameter `T`")
        })
        .expect("conflicting generic diagnostic");
    assert!(
        diagnostic
            .notes
            .iter()
            .any(|note| { note.contains("already inferred as") && note.contains("requires") })
    );
    assert!(
        diagnostic
            .help
            .iter()
            .any(|help| { help.contains("make the arguments agree") })
    );
}

#[test]
fn explains_conflicting_const_generic_inference_with_values_and_help() {
    let checked = pipeline(
        r#"
fun same[N: usize](left: [i32; N], right: [i32; N]) () {
    _ = left;
    _ = right;
}

fun main(left: [i32; 2], right: [i32; 3]) () {
    same(left, right)
}
"#,
    );
    let diagnostic = checked
        .diagnostics
        .iter()
        .find(|diagnostic| {
            diagnostic
                .summary
                .contains("conflicting inferred value for const generic parameter `N`")
        })
        .expect("conflicting const generic diagnostic");
    assert!(
        diagnostic
            .notes
            .iter()
            .any(|note| { note.contains("inferred as `2`") && note.contains("requires `3`") })
    );
    assert!(
        diagnostic
            .help
            .iter()
            .any(|help| { help.contains("provide an explicit const argument for `N`") })
    );
}

#[test]
fn rejects_implicit_trailing_generic_argument_inference() {
    let checked = pipeline(
        r#"
fun keep_first[T, U](left: T, right: U) T {
    _ = right;
    left
}

fun main() i32 {
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
fun keep_first[T, U](left: T, right: U) T {
    _ = right;
    left
}

fun take_array[T, N: usize](xs: [T; N]) usize {
    _ = xs;
    N
}

fun main(xs: [u8; 4]) i32 {
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
fun take_array[T, N: usize](xs: [T; N]) usize {
    _ = xs;
    0usize
}

fun main(xs: [u8; 4]) usize {
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
    fun value(& self) usize { 8usize }
}

struct Meter {}

extend[N: usize] Meter : Scaled[N] {}

fun read(value: & Scaled[8]) usize {
    value.value()
}

fun main() usize {
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
fn trait_object_vtable_instantiations_include_inherited_default_methods() {
    let checked = pipeline(
        r#"
trait Base[N: usize] {
    fun value(& self) usize { N }
}

trait Child[N: usize] : Base[N] {
    fun child(& self) usize { 1usize }
}

struct Meter {}

extend[N: usize] Meter : Base[N] {}
extend[N: usize] Meter : Child[N] {}

fun read(value: & Child[8]) usize {
    value.value() + value.child()
}

fun main() usize {
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
    assert_eq!(const_instances.len(), 2, "{const_instances:?}");
    assert!(const_instances.iter().all(|instantiation| matches!(
        instantiation.const_args.as_slice(),
        [nia_ty::ConstGenericArg {
            value: nia_ty::ConstGenericValue::Int(value),
            ..
        }] if value.bits() == 8
    )));
}

#[test]
fn trait_object_vtable_instantiations_deduplicate_diamond_supertraits() {
    let checked = pipeline(
        r#"
trait Base[N: usize] {
    fun base(& self) usize { N }
}

type BaseAlias[N: usize] = Base[N];

trait Left[N: usize] : BaseAlias[N] {
    fun left(& self) usize { 1usize }
}

trait Right[N: usize] : Base[N] {
    fun right(& self) usize { 2usize }
}

trait Root[N: usize] : Left[N] + Right[N] {
    fun root(& self) usize { 3usize }
}

struct Meter {}

extend[N: usize] Meter : Base[N] {}
extend[N: usize] Meter : Left[N] {}
extend[N: usize] Meter : Right[N] {}
extend[N: usize] Meter : Root[N] {}

fun read(value: & Root[8]) usize {
    value.left() + value.right() + value.root()
}

fun main() usize {
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
    assert_eq!(const_instances.len(), 4, "{const_instances:?}");
    assert!(const_instances.iter().all(|instantiation| matches!(
        instantiation.const_args.as_slice(),
        [nia_ty::ConstGenericArg {
            value: nia_ty::ConstGenericValue::Int(value),
            ..
        }] if value.bits() == 8
    )));
}

#[test]
fn trait_object_vtable_guards_compare_const_values_semantically() {
    let checked = pipeline(
        r#"
trait Base[N: usize] {
    fun base(& self) usize { N }
}

type BaseAlias[N: usize] = Base[N];

trait Left[N: usize] : Base[N] {
    fun left(& self) usize { 1usize }
}

trait Right[N: usize] : BaseAlias[3usize] {
    fun right(& self) usize { 2usize }
}

trait Root[N: usize] : Left[N] + Right[N] {
    fun root(& self) usize { 3usize }
}

struct Meter {}

extend[N: usize] Meter : Base[N] {}
extend[N: usize] Meter : Left[N] {}
extend[N: usize] Meter : Right[N] {}
extend[N: usize] Meter : Root[N] {}

fun read(value: & Root[3]) usize {
    value.base() + value.left() + value.right() + value.root()
}

fun main() usize {
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
    assert_eq!(const_instances.len(), 4, "{const_instances:?}");
    assert!(const_instances.iter().all(|instantiation| matches!(
        instantiation.const_args.as_slice(),
        [nia_ty::ConstGenericArg {
            value: nia_ty::ConstGenericValue::Int(value),
            ..
        }] if value.bits() == 3
    )));
}

#[test]
fn infers_const_generic_array_lengths_from_array_literal_arguments() {
    let checked = pipeline(
        r#"
fun take_array[T, N: usize](xs: [T; N]) usize {
    _ = xs;
    0usize
}

fun main() usize {
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

fun inspect[T, N: usize](value: Buffer[T, N]) () {
    _ = value;
}

fun main(value: Buffer[i32, 4]) () {
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

fun inspect[T](value: &Child[[Self as Parent[T]]::Item = T]) () {
    _ = value;
}

fun main(value: &Child[[Self as Parent[i32]]::Item = i32]) () {
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

trait Child : Parent[i32] + Parent[bool] + Parent[usize] {}

fun inspect[T, U](expected: U, value: &Child[
    [Self as Parent[T]]::Item = U,
    [Self as Parent[i32]]::Item = bool,
    [Self as Parent[usize]]::Item = bool,
]) () {
    _ = (expected, value);
}

fun main(value: &Child[
    [Self as Parent[i32]]::Item = bool,
    [Self as Parent[bool]]::Item = i32,
    [Self as Parent[usize]]::Item = bool,
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
fn associated_binding_inference_does_not_reuse_candidates() {
    let checked = pipeline(
        r#"
trait Slot[T] {
    type Item;
}

trait Both : Slot[i32] + Slot[bool] {}

struct Store {}

extend Store : Slot[i32] {
    type Item = bool;
}

extend Store : Slot[bool] {
    type Item = bool;
}

extend Store : Both {}

fun inspect[T](value: &Both[
    [Self as Slot[T]]::Item = bool,
    [Self as Slot[i32]]::Item = bool,
]) usize {
    _ = value;
    7
}

fun main() usize {
    let store = Store {};
    let value: &Both[
        [Self as Slot[i32]]::Item = bool,
        [Self as Slot[bool]]::Item = bool,
    ] = &store;
    inspect(value)
}
"#,
    );
    assert!(
        checked.diagnostics.is_empty(),
        "inference must backtrack to a distinct associated binding candidate: {:?}",
        checked.diagnostics
    );
}

#[test]
fn infers_const_generic_lengths_through_tuple_arguments() {
    let checked = pipeline(
        r#"
fun tupleLen[N: usize](value: ([i32; N], bool)) usize {
    _ = value;
    N
}

fun main(values: [i32; 3]) usize {
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
fun callableLen[N: usize](callback: &Fun([i32; N]) i32) usize {
    _ = callback;
    N
}

fun main(callback: &Fun([i32; 4]) i32) usize {
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
    fun consume(&self, value: T) T;
}

fun identity[T](value: &Consumer[T]) &Consumer[T] {
    value
}

fun main(value: &Consumer[i32]) &Consumer[i32] {
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
fun selectLen[N: usize](first: &mut [i32; N], second: [i32; N]) usize {
    _ = first;
    _ = second;
    N
}

fun main(readonly: &[i32; 2], values: [i32; 3]) usize {
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
fun tupleLen[N: usize](value: ([i32; N], &mut i32)) usize {
    _ = value;
    N
}

fun main(values: [i32; 2], readonly: &i32) usize {
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
fun take_range[T](bounds: T..T) T {
    bounds.end()
}

fun main(count: usize) usize {
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
    fun decode(input: Input) Self;
}

struct Wrapped {}

struct Bytes {
    data: [u8; 3],
}

fun decode[T, Input](input: Input) T
where T: DecodeFrom[Input]
{
    [T]::decode(input)
}

extend i32 : DecodeFrom[&[char]] {
    fun decode(input: &[char]) i32 {
        input.len() as i32
    }
}

extend i32 : DecodeFrom[&[u8]] {
    fun decode(input: &[u8]) i32 {
        input.len() as i32
    }
}

extend i32 : DecodeFrom[Wrapped] {
    fun decode(input: Wrapped) i32 {
        _ = input;
        0
    }
}

fun main() i32 {
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

fun require[T](value: T) ()
where T: Source[Item = i32]
{
    _ = value;
}

fun main(good: Good, bad: Bad) () {
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
    let diagnostic = checked
        .diagnostics
        .iter()
        .find(|diagnostic| {
            diagnostic
                .summary
                .contains("Source::Item expected i32, got bool")
        })
        .expect("associated type mismatch diagnostic");
    assert!(diagnostic.notes.iter().any(|note| {
        note.contains("resolves `Source::Item` as `bool`") && note.contains("requires `i32`")
    }));
    assert!(
        diagnostic
            .help
            .iter()
            .any(|help| { help.contains("change the associated type definition") })
    );
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

fun accept(value: Holder[Good]) () {
    _ = value;
}

fun reject(value: Holder[Bad]) () {
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

fun make[T]() i32
where T: Marker
{
    0
}

fun main() i32 {
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
fn infers_generic_from_where_bound_associated_type_binding() {
    let checked = pipeline(
        r#"
trait Source {
    type Item;
}

struct Box[T] {
    value: T,
}

extend[T] Box[T] : Source {
    type Item = T;
}

fun infer_item[S, Item](source: S) i32
where S: Source[Item = Item]
{
    _ = source;
    0
}

fun main(source: Box[i32]) i32 {
    infer_item[_, _](source)
}
"#,
    );

    assert!(checked.diagnostics.is_empty(), "{:?}", checked.diagnostics);
}

#[test]
fn accepts_where_bound_associated_tuple_type() {
    let checked = pipeline(
        r#"
trait Source {
    type Item;
}

struct Pair {}

extend Pair : Source {
    type Item = (i32, bool);
}

fun require[T](value: T) ()
where T: Source[Item = (i32, bool)]
{
    _ = value;
}

fun main(pair: Pair) () {
    require(pair);
}
"#,
    );

    assert!(checked.diagnostics.is_empty(), "{:?}", checked.diagnostics);
}

#[test]
fn accepts_where_bound_associated_tuple_with_semantic_const_types() {
    let checked = pipeline(
        r#"
trait Source {
    type Item;
}

struct Box[N: usize] {}
struct Pair {}

extend Pair : Source {
    type Item = (Box[3], bool);
}

fun require[T](value: T) ()
where T: Source[Item = (Box[3usize], bool)]
{
    _ = value;
}

fun main(pair: Pair) () {
    require(pair);
}
"#,
    );

    assert!(checked.diagnostics.is_empty(), "{:?}", checked.diagnostics);
}

#[test]
fn associated_type_binding_candidate_does_not_leak_partial_inference() {
    let checked = pipeline(
        r#"
trait Source {
    type Item;
}

struct Box[T] {
    value: T,
}

extend[T] Box[T] : Source {
    type Item = T;
}

fun reject_pair[S, Item](source: S) i32
where S: Source[Item = (Item, Item)]
{
    _ = source;
    0
}

fun main(pair: Box[(i32, bool)]) i32 {
    reject_pair[_, _](pair)
}
"#,
    );

    assert_eq!(
        checked
            .diagnostics
            .iter()
            .filter(|diagnostic| diagnostic
                .summary
                .contains("cannot infer generic parameter `Item`"))
            .count(),
        1,
        "{:?}",
        checked.diagnostics
    );
}

#[test]
fn supertrait_associated_binding_is_available_in_default_method() {
    let checked = pipeline(
        r#"
trait Parent {
    type Item;
}

trait Child : Parent[Item = i32] {
    fun value(&self) i32 {
        let item: [Self as Parent]::Item = 7;
        item
    }
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
    fun from(input: Input) i32!Self;
}

extend[Unit] [Unit]
where Unit: Sized
{
    fun parse[T](&self) i32!T
    where T: From[&[Unit]]
    {
        [T]::from(self)
    }
}

extend i32 : From[&[char]] {
    fun from(input: &[char]) i32!i32 {
        if input.len() == 0 {
            1!
        } else {
            !(input.len() as i32)
        }
    }
}

fun main() i32 {
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
extern fun printf(fmt: &u8, ...);

fun main(flag: bool) i32 {
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

#[test]
fn infers_generic_element_from_string_slice_pointer_coercion() {
    let checked = pipeline(
        r#"
fun first[T](value: &[T]) T {
    value[0]
}

fun main() char {
    first(&"nia")
}
"#,
    );
    assert!(checked.diagnostics.is_empty(), "{:?}", checked.diagnostics);
}

#[test]
fn infers_generic_element_from_byte_string_slice_pointer_coercion() {
    let checked = pipeline(
        r#"
fun first[T](value: &[T]) T {
    value[0]
}

fun main() u8 {
    first(&b"nia")
}
"#,
    );
    assert!(checked.diagnostics.is_empty(), "{:?}", checked.diagnostics);
}

#[test]
fn infers_generic_element_from_mutable_array_slice_pointer() {
    let checked = pipeline(
        r#"
fun first[T](value: &mut [T]) T {
    value[0]
}

fun main(values: [i32; 2]) i32 {
    first(&mut values)
}
"#,
    );
    assert!(checked.diagnostics.is_empty(), "{:?}", checked.diagnostics);
}

#[test]
fn rejects_readonly_array_pointer_for_mutable_slice_parameter() {
    let checked = pipeline(
        r#"
fun first[T](value: &mut [T]) T {
    value[0]
}

fun main(values: [i32; 2]) i32 {
    first(&values)
}
"#,
    );
    assert!(!checked.diagnostics.is_empty());
}

#[test]
fn structural_recovery_does_not_publish_generic_shape_cascade() {
    let checked = pipeline(
        r#"
fun choose[T](left: T, right: T) T { left }

fun main() () {
    _ = choose((missing, 1i32), (true, false));
}
"#,
    );
    let summaries = checked
        .diagnostics
        .iter()
        .map(|diagnostic| diagnostic.summary.as_str())
        .collect::<Vec<_>>();
    assert!(
        summaries
            .iter()
            .any(|summary| summary.contains("unknown value `missing`")),
        "{summaries:?}"
    );
    assert!(
        summaries.iter().all(|summary| {
            !summary.contains("conflicting inferred type")
                && !summary.contains("type mismatch in tuple element")
        }),
        "{summaries:?}"
    );
}
