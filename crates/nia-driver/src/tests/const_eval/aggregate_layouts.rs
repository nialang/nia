// SPDX-License-Identifier: GPL-3.0-or-later
use super::*;

#[test]
fn const_switch_structural_struct_fields_have_typed_values() {
    let root = temp_dir("const_switch_structural_struct_fields_have_typed_values");
    write(
        &root.join("main.nia"),
        r#"
const fn id[T](value: T) T {
    value
}

const config = switch 1usize {
    1usize => ({width: 4usize}),
    _ => ({width: 8usize}),
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
fn structural_const_array_elements_have_typed_values() {
    let root = temp_dir("structural_const_array_elements_have_typed_values");
    write(
        &root.join("main.nia"),
        r#"
const fn id[T](value: T) T {
    value
}

const configs = [{width: 4usize}, {width: 8usize}];
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
fn structural_const_array_slices_have_typed_values() {
    let root = temp_dir("structural_const_array_slices_have_typed_values");
    write(
        &root.join("main.nia"),
        r#"
const fn id[T](value: T) T {
    value
}

const configs = [{width: 4usize}, {width: 8usize}, {width: 16usize}];
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
fn structural_const_array_repeat_elements_have_typed_values() {
    let root = temp_dir("structural_const_array_repeat_elements_have_typed_values");
    write(
        &root.join("main.nia"),
        r#"
const fn id[T](value: T) T {
    value
}

const configs = [{width: 4usize}; 2usize];
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
    if ?payload = value {
        payload
    } or null {
        0usize
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

const fn use_try(value: ?usize) usize {
    id(value.?)
}

const n: usize = use_try(?7usize);

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
const n: usize = if !payload = got {
    payload
} or err! {
    err
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

const config = {target: {pointer_width: 64usize}};
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

const builtin = {target: {pointer_width: 64usize}};
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
    if ?payload = value {
        payload
    } or null {
        0usize
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
    if !payload = value {
        payload
    } or err! {
        err
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
    if !payload = value {
        payload
    } or err! {
        err
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

const fn size_of[T]() usize {
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
pub const fn size_of[T]() usize {
    std::builtin::size[T]()
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

const fn size_of[T]() usize {
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
        { value: 0 }
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
    {
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
        { value: 0 }
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
    {
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
