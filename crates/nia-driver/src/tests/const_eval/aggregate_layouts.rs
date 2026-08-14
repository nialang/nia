// SPDX-License-Identifier: GPL-3.0-or-later
use super::*;

#[test]
fn payload_enum_construction_and_matching_are_const_evaluable() {
    let root = temp_dir("payload_enum_construction_and_matching_are_const_evaluable");
    write(
        &root.join("main.nia"),
        r#"
enum Event {
    Closed,
    Data(usize),
    Move(usize, usize),
    Resize { width: usize, height: usize },
}

const fn score(event: Event) usize {
    switch event {
        Event::Closed => 0usize,
        Event::Data(value) => value,
        Event::Move(x, y) => x + y,
        Event::Resize { width, height: h } => width + h,
    }
}

const n: usize = score(Event::Closed)
    + score(Event::Data(2usize))
    + score(Event::Move(3usize, 4usize))
    + score(Event::Resize { height: 5usize, width: 6usize });

fn main() i32 {
    let values: [n]i32 = [0; n];
    values.len() as i32
}
"#,
    );

    let program = check_program(root.join("main.nia").to_string_lossy().into_owned());
    assert!(program.diagnostics.is_empty(), "{:?}", program.diagnostics);
}

#[test]
fn const_switch_nominal_struct_fields_have_typed_values() {
    let root = temp_dir("const_switch_nominal_struct_fields_have_typed_values");
    write(
        &root.join("main.nia"),
        r#"
const fn id[T](value: T) T {
    value
}

struct Config { width: usize }

const config = switch 1usize {
    1usize => Config { width: 4usize },
    _ => Config { width: 8usize },
};
const n: usize = id(config.width);

fn main() i32 {
    let mut values: [n]i32 = [0; n];
    values.len() as i32
}
"#,
    );

    let program = check_program(root.join("main.nia").to_string_lossy().into_owned());
    assert!(program.diagnostics.is_empty(), "{:?}", program.diagnostics);
}

#[test]
fn nominal_const_array_elements_have_typed_values() {
    let root = temp_dir("nominal_const_array_elements_have_typed_values");
    write(
        &root.join("main.nia"),
        r#"
const fn id[T](value: T) T {
    value
}

struct Config { width: usize }
const configs = [Config { width: 4usize }, Config { width: 8usize }];
const n: usize = id(configs[1].width);

fn main() i32 {
    let mut values: [n]i32 = [0; n];
    values.len() as i32
}
"#,
    );

    let program = check_program(root.join("main.nia").to_string_lossy().into_owned());
    assert!(program.diagnostics.is_empty(), "{:?}", program.diagnostics);
}

#[test]
fn nominal_const_array_slices_have_typed_values() {
    let root = temp_dir("nominal_const_array_slices_have_typed_values");
    write(
        &root.join("main.nia"),
        r#"
const fn id[T](value: T) T {
    value
}

struct Config { width: usize }
const configs = [Config { width: 4usize }, Config { width: 8usize }, Config { width: 16usize }];
const selected = configs[1..=2];
const n: usize = id(selected[1].width);

fn main() i32 {
    let mut values: [n]i32 = [0; n];
    values.len() as i32
}
"#,
    );

    let program = check_program(root.join("main.nia").to_string_lossy().into_owned());
    assert!(program.diagnostics.is_empty(), "{:?}", program.diagnostics);
}

#[test]
fn nominal_const_array_repeat_elements_have_typed_values() {
    let root = temp_dir("nominal_const_array_repeat_elements_have_typed_values");
    write(
        &root.join("main.nia"),
        r#"
const fn id[T](value: T) T {
    value
}

struct Config { width: usize }
const configs = [Config { width: 4usize }; 2usize];
const n: usize = id(configs[1].width);

fn main() i32 {
    let mut values: [n]i32 = [0; n];
    values.len() as i32
}
"#,
    );

    let program = check_program(root.join("main.nia").to_string_lossy().into_owned());
    assert!(program.diagnostics.is_empty(), "{:?}", program.diagnostics);
}

#[test]
fn generic_const_function_infers_type_arg_from_nested_call_return_type() {
    let root = temp_dir("generic_const_function_infers_type_arg_from_nested_call_return_type");
    write(
        &root.join("main.nia"),
        r#"
const fn id[T](value: T) T {
    value
}

const fn array_len[T](value: T) usize {
    7usize
}

const n: usize = array_len(id(4usize));

fn main() i32 {
    let mut values: [n]i32 = [0; n];
    values.len() as i32
}
"#,
    );

    let program = check_program(root.join("main.nia").to_string_lossy().into_owned());
    assert!(program.diagnostics.is_empty(), "{:?}", program.diagnostics);
}

#[test]
fn generic_const_function_infers_type_arg_from_nested_optional_call_return_type() {
    let root =
        temp_dir("generic_const_function_infers_type_arg_from_nested_optional_call_return_type");
    write(
        &root.join("main.nia"),
        r#"
const fn id[T](value: T) T {
    value
}

const fn some[T](value: T) ?T {
    ?value
}

const fn unwrap(value: ?usize) usize {
    switch value {
        ?payload => {
            payload
        },
        null => {
            0usize
        },
    }
}

const n: usize = unwrap(id(some(7usize)));

fn main() i32 {
    let mut values: [n]i32 = [0; n];
    values.len() as i32
}
"#,
    );

    let program = check_program(root.join("main.nia").to_string_lossy().into_owned());
    assert!(program.diagnostics.is_empty(), "{:?}", program.diagnostics);
}

#[test]
fn generic_const_function_infers_type_arg_from_try_payload_return_type() {
    let root = temp_dir("generic_const_function_infers_type_arg_from_try_payload_return_type");
    write(
        &root.join("main.nia"),
        r#"
const fn id[T](value: T) T {
    value
}

const fn use_try(value: ?usize) ?usize {
    ?id(value.?)
}

const n: usize = switch use_try(?7usize) {
    ?value => value,
    null => 0usize,
};

fn main() i32 {
    let mut values: [n]i32 = [0; n];
    values.len() as i32
}
"#,
    );

    let program = check_program(root.join("main.nia").to_string_lossy().into_owned());
    assert!(program.diagnostics.is_empty(), "{:?}", program.diagnostics);
}

#[test]
fn generic_const_function_infers_type_arg_from_error_try_payload_return_type() {
    let root =
        temp_dir("generic_const_function_infers_type_arg_from_error_try_payload_return_type");
    write(
        &root.join("main.nia"),
        r#"
const fn id[T](value: T) T {
    value
}

const fn use_try(value: usize!usize) usize!usize {
    !id(value.?)
}

const got: usize!usize = use_try(!7usize);
const n: usize = switch got {
    !payload => {
        payload
    },
    err! => {
        err
    },
};

fn main() i32 {
    let mut values: [n]i32 = [0; n];
    values.len() as i32
}
"#,
    );

    let program = check_program(root.join("main.nia").to_string_lossy().into_owned());
    assert!(program.diagnostics.is_empty(), "{:?}", program.diagnostics);
}

#[test]
fn generic_const_function_infers_type_arg_from_integer_field() {
    let root = temp_dir("generic_const_function_infers_type_arg_from_integer_field");
    write(
        &root.join("main.nia"),
        r#"
const fn id[T](value: T) T {
    value
}

struct Target { pointer_width: usize }
struct Config { target: Target }
const config = Config { target: Target { pointer_width: 64usize } };
const bits: usize = id(config.target.pointer_width);
const n: usize = bits / 8usize;

fn main() i32 {
    let mut values: [n]i32 = [0; n];
    values.len() as i32
}
"#,
    );

    let program = check_program(root.join("main.nia").to_string_lossy().into_owned());
    assert!(program.diagnostics.is_empty(), "{:?}", program.diagnostics);
}

#[test]
fn generic_const_function_infers_type_arg_from_bound_struct() {
    let root = temp_dir("generic_const_function_infers_type_arg_from_bound_struct");
    write(
        &root.join("main.nia"),
        r#"
const fn id[T](value: T) T {
    value
}

struct Target { pointer_width: usize }
struct Config { target: Target }
const builtin = Config { target: Target { pointer_width: 64usize } };
const bits: usize = id(builtin.target.pointer_width);
const n: usize = bits / 8usize;

fn main() i32 {
    let mut values: [n]i32 = [0; n];
    values.len() as i32
}
"#,
    );

    let program = check_program(root.join("main.nia").to_string_lossy().into_owned());
    assert!(program.diagnostics.is_empty(), "{:?}", program.diagnostics);
}

#[test]
fn generic_const_function_infers_type_arg_from_optional_constructor() {
    let root = temp_dir("generic_const_function_infers_type_arg_from_optional_constructor");
    write(
        &root.join("main.nia"),
        r#"
const fn id[T](value: T) T {
    value
}

const fn unwrap(value: ?usize) usize {
    switch value {
        ?payload => {
            payload
        },
        null => {
            0usize
        },
    }
}

const n: usize = unwrap(id(?7usize));

fn main() i32 {
    let mut values: [n]i32 = [0; n];
    values.len() as i32
}
"#,
    );

    let program = check_program(root.join("main.nia").to_string_lossy().into_owned());
    assert!(program.diagnostics.is_empty(), "{:?}", program.diagnostics);
}

#[test]
fn generic_const_function_infers_type_arg_from_error_success_constructor_context() {
    let root =
        temp_dir("generic_const_function_infers_type_arg_from_error_success_constructor_context");
    write(
        &root.join("main.nia"),
        r#"
const fn id[T](value: usize!T) usize!T {
    value
}

const fn unwrap(value: usize!usize) usize {
    switch value {
        !payload => {
            payload
        },
        err! => {
            err
        },
    }
}

const n: usize = unwrap(id(!7usize));

fn main() i32 {
    let mut values: [n]i32 = [0; n];
    values.len() as i32
}
"#,
    );

    let program = check_program(root.join("main.nia").to_string_lossy().into_owned());
    assert!(program.diagnostics.is_empty(), "{:?}", program.diagnostics);
}

#[test]
fn generic_const_function_infers_type_arg_from_error_payload_constructor_context() {
    let root =
        temp_dir("generic_const_function_infers_type_arg_from_error_payload_constructor_context");
    write(
        &root.join("main.nia"),
        r#"
const fn id[T](value: T!usize) T!usize {
    value
}

const fn unwrap(value: usize!usize) usize {
    switch value {
        !payload => {
            payload
        },
        err! => {
            err
        },
    }
}

const n: usize = unwrap(id(3usize!));

fn main() i32 {
    let mut values: [n]i32 = [0; n];
    values.len() as i32
}
"#,
    );

    let program = check_program(root.join("main.nia").to_string_lossy().into_owned());
    assert!(program.diagnostics.is_empty(), "{:?}", program.diagnostics);
}

#[test]
fn generic_const_function_infers_type_arg_from_typed_aggregate_literals() {
    let root = temp_dir("generic_const_function_infers_type_arg_from_typed_aggregate_literals");
    write(
        &root.join("main.nia"),
        r#"
struct Config {
    widths: [3]usize,
}

const fn id[T](value: T) T {
    value
}

const config: Config = id(Config{widths: [2, 4, 8]});
const widths: [3]usize = id([3]usize[2, 4, 8]);
const width: usize = config.widths[1] + widths[0];

fn main() i32 {
    let mut values: [width]i32 = [0; width];
    values.len() as i32
}
"#,
    );

    let program = check_program(root.join("main.nia").to_string_lossy().into_owned());
    assert!(program.diagnostics.is_empty(), "{:?}", program.diagnostics);
}

#[test]
fn imported_generic_const_function_infers_type_arg_from_typed_value() {
    let root = temp_dir("imported_generic_const_function_infers_type_arg_from_typed_value");
    write(
        &root.join("main.nia"),
        r#"
module identity;
using entry::identity;

const width: usize = 4;
const n: usize = identity::id(width);

fn main() i32 {
    let mut values: [n]i32 = [0; n];
    values.len() as i32
}
"#,
    );
    write(
        &root.join("identity.nia"),
        r#"
pub const fn id[T](value: T) T {
    value
}
"#,
    );

    let program = check_program(root.join("main.nia").to_string_lossy().into_owned());
    assert!(program.diagnostics.is_empty(), "{:?}", program.diagnostics);
}

#[test]
fn generic_const_function_reports_uninferred_type_args() {
    let root = temp_dir("generic_const_function_reports_uninferred_type_args");
    write(
        &root.join("main.nia"),
        r#"
const fn zero[T]() usize {
    0
}

const n: usize = zero();

fn main() i32 {
    let mut values: [n]i32 = [0; n];
    values.len() as i32
}
"#,
    );

    let program = check_program(root.join("main.nia").to_string_lossy().into_owned());
    assert!(
        program.diagnostics.iter().any(|diagnostic| diagnostic
            .diagnostic
            .summary
            .contains("cannot infer const generic type argument `T`")),
        "{:?}",
        program.diagnostics
    );
}

#[test]
fn generic_const_function_substitutes_type_args_for_layout_builtins() {
    let root = temp_dir("generic_const_function_substitutes_type_args_for_layout_builtins");
    write(
        &root.join("main.nia"),
        r#"
struct Pair {
    a: u8,
    b: i32,
}

const fn size_of[T]() usize
where T: Sized {
    std::builtin::size[T]()
}

const n: usize = size_of[Pair]();

fn main() i32 {
    let mut bytes: [n]u8 = [0; n];
    bytes.len() as i32
}
"#,
    );

    let program = check_program(root.join("main.nia").to_string_lossy().into_owned());
    assert!(program.diagnostics.is_empty(), "{:?}", program.diagnostics);
}

#[test]
fn imported_generic_const_function_substitutes_type_args_for_layout_builtins() {
    let root =
        temp_dir("imported_generic_const_function_substitutes_type_args_for_layout_builtins");
    write(
        &root.join("main.nia"),
        r#"
module layout;
using entry::layout;

struct Pair {
    a: u8,
    b: i32,
}

const n: usize = layout::size_of[Pair]();

fn main() i32 {
    let mut bytes: [n]u8 = [0; n];
    bytes.len() as i32
}
"#,
    );
    write(
        &root.join("layout.nia"),
        r#"
pub const fn size_of[T]() usize
where T: Sized {
    std::builtin::size[T]()
}
"#,
    );

    let program = check_program(root.join("main.nia").to_string_lossy().into_owned());
    assert!(program.diagnostics.is_empty(), "{:?}", program.diagnostics);
}

#[test]
fn imported_generic_scalar_union_is_shared_across_const_and_runtime_calls() {
    let root = temp_dir("imported_generic_scalar_union_is_shared_across_const_and_runtime_calls");
    write(
        &root.join("main.nia"),
        r#"
module bits;
using entry::bits;

const compileSlot: bits::ScalarSlot[f32] = bits::slot[f32](1.0);
const compileBits: u32 = bits::readBits[f32](compileSlot);

fn main() i32 {
    let runtimeSlot: bits::ScalarSlot[f32] = bits::slot[f32](1.0);
    if compileBits == 1065353216
        and bits::readBits[f32](runtimeSlot) == compileBits {
        0
    } else {
        1
    }
}
"#,
    );
    write(
        &root.join("bits.nia"),
        r#"
pub union ScalarSlot[T] {
    value: T,
    bits: u32,
}

pub const fn slot[T](value: T) ScalarSlot[T] {
    ScalarSlot[T] { value }
}

pub const fn readBits[T](slot: ScalarSlot[T]) u32 {
    slot.bits
}
"#,
    );

    let program = check_program(root.join("main.nia").to_string_lossy().into_owned());
    assert!(program.diagnostics.is_empty(), "{:?}", program.diagnostics);
}

#[test]
fn imported_generic_struct_union_preserves_padding_across_const_and_runtime_calls() {
    let root =
        temp_dir("imported_generic_struct_union_preserves_padding_across_const_and_runtime_calls");
    write(
        &root.join("main.nia"),
        r#"
module bits;
using entry::bits;

const compileBytes: [5]u8 = bits::encode[u32](bits::Pair[u32] { marker: 170, value: 287454020 });
const compilePair: bits::Pair[u32] = bits::decode[u32](compileBytes);

fn main() i32 {
    let runtimeBytes = bits::encode[u32](bits::Pair[u32] { marker: 170, value: 287454020 });
    let runtimePair = bits::decode[u32](runtimeBytes);
    if compileBytes[4] == 170
        and compilePair.value == 287454020
        and runtimeBytes[0] == compileBytes[0]
        and runtimePair.marker == compilePair.marker {
        0
    } else {
        1
    }
}
"#,
    );
    write(
        &root.join("bits.nia"),
        r#"
pub struct Pair[T] {
    marker: u8,
    value: T,
}

pub union PairSlot[T] {
    value: Pair[T],
    prefix: [5]u8,
}

pub const fn encode[T](value: Pair[T]) [5]u8 {
    let slot = PairSlot[T] { value: value };
    slot.prefix
}

pub const fn decode[T](bytes: [5]u8) Pair[T] {
    let slot = PairSlot[T] { prefix: bytes };
    slot.value
}
"#,
    );

    let program = check_program(root.join("main.nia").to_string_lossy().into_owned());
    assert!(program.diagnostics.is_empty(), "{:?}", program.diagnostics);
}

#[test]
fn imported_generic_nested_union_preserves_storage_across_const_and_runtime_calls() {
    let root =
        temp_dir("imported_generic_nested_union_preserves_storage_across_const_and_runtime_calls");
    write(
        &root.join("main.nia"),
        r#"
module bits;
using entry::bits;

const compileSlot: bits::Outer[u32] = bits::layered[u32](1144201745, 26197);
const compileBytes: [4]u8 = bits::readBytes[u32](compileSlot);

fn main() i32 {
    let runtimeSlot = bits::layered[u32](1144201745, 26197);
    let runtimeBytes = bits::readBytes[u32](runtimeSlot);
    let materializedBytes = bits::readBytes[u32](compileSlot);
    if compileBytes[0] == 85
        and compileBytes[1] == 102
        and compileBytes[2] == 51
        and compileBytes[3] == 68
        and runtimeBytes[3] == compileBytes[3]
        and materializedBytes[2] == compileBytes[2] {
        0
    } else {
        1
    }
}
"#,
    );
    write(
        &root.join("bits.nia"),
        r#"
pub union Inner[T] {
    wide: T,
    narrow: u16,
}

pub union Outer[T] {
    inner: Inner[T],
    bytes: [4]u8,
}

pub const fn layered[T](wide: T, narrow: u16) Outer[T] {
    let mut inner = Inner[T] { wide: wide };
    inner.narrow = narrow;
    Outer[T] { inner }
}

pub const fn readBytes[T](slot: Outer[T]) [4]u8 {
    slot.bytes
}
"#,
    );

    let program = check_program(root.join("main.nia").to_string_lossy().into_owned());
    assert!(program.diagnostics.is_empty(), "{:?}", program.diagnostics);
}

#[test]
fn imported_generic_vector_union_is_shared_across_const_and_runtime_calls() {
    let root = temp_dir("imported_generic_vector_union_is_shared_across_const_and_runtime_calls");
    write(
        &root.join("main.nia"),
        r#"
module bits;
using entry::bits;

const compileVector: u16x2 = std::builtin::insert(
    std::builtin::splat[u16x2](4386),
    1,
    13124,
);
const compileSlot: bits::VectorSlot[u16x2] = bits::slot[u16x2](compileVector);
const compileBytes: [4]u8 = bits::readBytes[u16x2](compileSlot);
const decoded: u16x2 = bits::decode[u16x2](compileBytes);

fn main() i32 {
    let runtimeVector = std::builtin::insert(
        std::builtin::splat[u16x2](4386),
        1,
        13124,
    );
    let runtimeBytes = bits::encode[u16x2](runtimeVector);
    let materializedBytes = bits::readBytes[u16x2](compileSlot);
    if compileBytes[0] == 34
        and compileBytes[1] == 17
        and compileBytes[2] == 68
        and compileBytes[3] == 51
        and std::builtin::extract(decoded, 1) == 13124
        and runtimeBytes[2] == compileBytes[2]
        and materializedBytes[3] == compileBytes[3] {
        0
    } else {
        1
    }
}
"#,
    );
    write(
        &root.join("bits.nia"),
        r#"
pub union VectorSlot[V] {
    value: V,
    bytes: [4]u8,
}

pub const fn slot[V](value: V) VectorSlot[V] {
    VectorSlot[V] { value }
}

pub const fn encode[V](value: V) [4]u8 {
    let slot = VectorSlot[V] { value: value };
    slot.bytes
}

pub const fn decode[V](bytes: [4]u8) V {
    let slot = VectorSlot[V] { bytes: bytes };
    slot.value
}

pub const fn readBytes[V](slot: VectorSlot[V]) [4]u8 {
    slot.bytes
}
"#,
    );

    let program = check_program(root.join("main.nia").to_string_lossy().into_owned());
    assert!(program.diagnostics.is_empty(), "{:?}", program.diagnostics);
}

#[test]
fn imported_const_value_evaluates_layout_builtin_in_defining_module() {
    let root = temp_dir("imported_const_value_evaluates_layout_builtin_in_defining_module");
    write(
        &root.join("config.nia"),
        r#"
pub struct Pair {
    a: u8,
    b: i32,
}

pub const pair_size: usize = std::builtin::size[Pair]();
"#,
    );
    write(
        &root.join("main.nia"),
        r#"
module config;
using entry::config;

const n: usize = config::pair_size;

fn main() i32 {
    let mut bytes: [n]u8 = [0; n];
    bytes.len() as i32
}
"#,
    );

    let program = check_program(root.join("main.nia").to_string_lossy().into_owned());
    assert!(program.diagnostics.is_empty(), "{:?}", program.diagnostics);
}

#[test]
fn layout_builtin_uses_imported_const_array_lengths() {
    let root = temp_dir("layout_builtin_uses_imported_const_array_lengths");
    write(
        &root.join("config.nia"),
        r#"
pub const N: usize = 4usize;

pub struct Packet {
    bytes: [N]u8,
}
"#,
    );
    write(
        &root.join("main.nia"),
        r#"
module config;
using entry::config;

const n: usize = std::builtin::size[config::Packet]();

fn main() i32 {
    let mut bytes: [n]u8 = [0; n];
    bytes.len() as i32
}
"#,
    );

    let program = check_program(root.join("main.nia").to_string_lossy().into_owned());
    assert!(program.diagnostics.is_empty(), "{:?}", program.diagnostics);
}

#[test]
fn function_body_const_call_substitutes_type_args_for_layout_builtins() {
    let root = temp_dir("function_body_const_call_substitutes_type_args_for_layout_builtins");
    write(
        &root.join("main.nia"),
        r#"
struct Pair {
    a: u8,
    b: i32,
}

const fn size_of[T]() usize
where T: Sized {
    std::builtin::size[T]()
}

fn main() i32 {
    const n: usize = size_of[Pair]();
    let mut bytes: [n]u8 = [0; n];
    bytes.len() as i32
}
"#,
    );

    let program = check_program(root.join("main.nia").to_string_lossy().into_owned());
    assert!(program.diagnostics.is_empty(), "{:?}", program.diagnostics);
}

#[test]
fn imported_struct_field_array_length_accepts_literal_repeat_count() {
    let root = temp_dir("imported_struct_field_array_length_accepts_literal_repeat_count");
    write(
        &root.join("defs.nia"),
        r#"
pub const N: usize = 4;

pub struct Item {
    value: u32,
}

pub struct Boxed {
    items: [N]Item,
}

extend Item {
    pub fn zero() Item {
        Self { value: 0 }
    }
}
"#,
    );
    write(
        &root.join("main.nia"),
        r#"
module defs;
using entry::defs;
using defs::*;

fn make() Boxed {
    Boxed {
        items: [Item::zero(); 4],
    }
}

fn main() i32 {
    let mut x = make();
    x.items[0].value as i32
}
"#,
    );

    let program = check_program(root.join("main.nia").to_string_lossy().into_owned());
    assert!(program.diagnostics.is_empty(), "{:?}", program.diagnostics);
}

#[test]
fn imported_struct_field_array_length_accepts_imported_repeat_count() {
    let root = temp_dir("imported_struct_field_array_length_accepts_imported_repeat_count");
    write(
        &root.join("defs.nia"),
        r#"
pub const N: usize = 4;

pub struct Item {
    value: u32,
}

pub struct Boxed {
    items: [N]Item,
}

extend Item {
    pub fn zero() Item {
        Self { value: 0 }
    }
}
"#,
    );
    write(
        &root.join("main.nia"),
        r#"
module defs;
using entry::defs;
using defs::*;

fn make() Boxed {
    Boxed {
        items: [Item::zero(); defs::N],
    }
}

fn main() i32 {
    let mut x = make();
    x.items[0].value as i32
}
"#,
    );

    let program = check_program(root.join("main.nia").to_string_lossy().into_owned());
    assert!(program.diagnostics.is_empty(), "{:?}", program.diagnostics);
}
