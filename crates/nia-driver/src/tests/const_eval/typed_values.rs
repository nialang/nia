// SPDX-License-Identifier: GPL-3.0-or-later
use super::*;

#[test]
fn structural_const_struct_fields_have_typed_values() {
    let root = temp_dir("structural_const_struct_fields_have_typed_values");
    write(
        &root.join("main.nia"),
        r#"
const fn id[T](value: T) T {
    value
}

const config = {width: 4usize, enabled: true};
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
fn nested_structural_const_struct_fields_have_typed_values() {
    let root = temp_dir("nested_structural_const_struct_fields_have_typed_values");
    write(
        &root.join("main.nia"),
        r#"
const fn id[T](value: T) T {
    value
}

const config = {target: {word_bits: 64usize}};
const n: usize = id(config.target.word_bits / 8usize);

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
fn imported_structural_const_struct_fields_have_typed_values() {
    let root = temp_dir("imported_structural_const_struct_fields_have_typed_values");
    write(
        &root.join("config.nia"),
        r#"
pub const config = {width: 4usize};
"#,
    );
    write(
        &root.join("main.nia"),
        r#"
module config;
using entry::config;

const fn id[T](value: T) T {
    value
}

const n: usize = id(config::config.width);

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
fn const_function_local_structural_struct_fields_have_typed_values() {
    let root = temp_dir("const_function_local_structural_struct_fields_have_typed_values");
    write(
        &root.join("main.nia"),
        r#"
const fn id[T](value: T) T {
    value
}

const fn width() usize {
    const config = {target: {word_bits: 64usize}};
    id(config.target.word_bits) / 8usize
}

const n: usize = width();

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
fn const_if_structural_struct_fields_have_typed_values() {
    let root = temp_dir("const_if_structural_struct_fields_have_typed_values");
    write(
        &root.join("main.nia"),
        r#"
const fn id[T](value: T) T {
    value
}

const config = if true {
    {width: 4usize}
} else {
    {width: 8usize}
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
fn if_expr_is_an_ordinary_const_value() {
    let root = temp_dir("if_expr_is_an_ordinary_const_value");
    write(
        &root.join("main.nia"),
        r#"
const fn id[T](value: T) T {
    value
}

const config = if true {
    {width: 4usize}
} else {
    {width: 8usize}
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
fn if_expr_rejects_non_bool_condition() {
    let root = temp_dir("if_expr_rejects_non_bool_condition");
    write(
        &root.join("main.nia"),
        r#"
const fn id[T](value: T) T {
    value
}

const n: usize = id(if 1usize {
    4usize
} else {
    8usize
});
"#,
    );

    let program = check_program(root.join("main.nia").to_string_lossy().into_owned());
    assert!(
        program
            .diagnostics
            .iter()
            .any(|diagnostic| diagnostic.diagnostic.summary.contains("bool")),
        "{:?}",
        program.diagnostics
    );
}

#[test]
fn const_fn_returns_if_expr_value() {
    let root = temp_dir("const_fn_returns_if_expr_value");
    write(
        &root.join("main.nia"),
        r#"
const fn select() usize {
    if true {
        4usize
    } else {
        8usize
    }
}

const n: usize = select();

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
fn const_char_literals_are_typed_scalar_values() {
    let root = temp_dir("const_char_literals_are_typed_scalar_values");
    write(
        &root.join("main.nia"),
        r#"
const fn choose_char(value: char) char {
    value
}

const fn widen_byte(value: u8) usize {
    value as usize
}

const ch: char = choose_char('A');
const n: usize = widen_byte(b'\n') + 1usize;

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
fn const_bitwise_not_evaluates_integer_values() {
    let root = temp_dir("const_bitwise_not_evaluates_integer_values");
    write(
        &root.join("main.nia"),
        r#"
const fn mask(value: usize) usize {
    ~value & 15usize
}

const n: usize = mask(10usize);

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
fn const_casts_have_typed_values() {
    let root = temp_dir("const_casts_have_typed_values");
    write(
        &root.join("main.nia"),
        r#"
const fn id[T](value: T) T {
    value
}

const n: usize = id(b'a' as usize);

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
fn const_integer_casts_convert_values() {
    let root = temp_dir("const_integer_casts_convert_values");
    write(
        &root.join("main.nia"),
        r#"
const fn narrow(value: usize) usize {
    (value as u8) as usize
}

const fn signed(value: usize) i32 {
    (value as i8) as i32
}

const narrow_value: usize = narrow(258usize);
const signed_value: i32 = signed(255usize);

static mut narrow_global: i32 = narrow_value as i32;
static mut signed_global: i32 = signed_value;
"#,
    );

    let program = codegen_program(root.join("main.nia").to_string_lossy().into_owned());
    assert!(program.diagnostics.is_empty(), "{:?}", program.diagnostics);
    let main_module = program
        .backend_lowering
        .program
        .modules
        .iter()
        .find(|module| module.name.ends_with("main.nia"))
        .expect("main module");
    let narrow_global = main_module
        .globals
        .iter()
        .find(|global| global.name == sym("narrow_global"))
        .expect("narrow_global");
    assert_eq!(narrow_global.init, Some(static_int(2)));
    let signed_global = main_module
        .globals
        .iter()
        .find(|global| global.name == sym("signed_global"))
        .expect("signed_global");
    assert_eq!(signed_global.init, Some(static_int(-1)));
}

#[test]
fn const_float_values_drive_casts_and_conditions() {
    let root = temp_dir("const_float_values_drive_casts_and_conditions");
    write(
        &root.join("main.nia"),
        r#"
const fn scale(value: f64) f64 {
    value * 2.0f64 + 0.5f64
}

const fn wide(value: usize) f64 {
    value as f64
}

const scaled: f64 = scale(3.25f64);
const from_int: f64 = wide(4usize);
const n: usize = if scaled > from_int {
    scaled as usize
} else {
    0usize
};

static mut value: i32 = n as i32;
"#,
    );

    let program = codegen_program(root.join("main.nia").to_string_lossy().into_owned());
    assert!(program.diagnostics.is_empty(), "{:?}", program.diagnostics);
    let main_module = program
        .backend_lowering
        .program
        .modules
        .iter()
        .find(|module| module.name.ends_with("main.nia"))
        .expect("main module");
    let value = main_module
        .globals
        .iter()
        .find(|global| global.name == sym("value"))
        .expect("value global");
    assert_eq!(value.init, Some(static_int(7)));
}

#[test]
fn generic_const_function_rejects_non_numeric_cast_operand() {
    let root = temp_dir("generic_const_function_rejects_non_numeric_cast_operand");
    write(
        &root.join("main.nia"),
        r#"
const fn id[T](value: T) T {
    value
}

const n: usize = id(true as usize);
"#,
    );

    let program = check_program(root.join("main.nia").to_string_lossy().into_owned());
    assert!(
        program
            .diagnostics
            .iter()
            .any(|diagnostic| diagnostic.diagnostic.summary.contains("cast")),
        "{:?}",
        program.diagnostics
    );
}

#[test]
fn generic_const_function_rejects_structural_cast_operand() {
    let root = temp_dir("generic_const_function_rejects_structural_cast_operand");
    write(
        &root.join("main.nia"),
        r#"
const fn id[T](value: T) T {
    value
}

const n: usize = id(({width: 4usize}) as usize);
"#,
    );

    let program = check_program(root.join("main.nia").to_string_lossy().into_owned());
    assert!(
        program
            .diagnostics
            .iter()
            .any(|diagnostic| diagnostic.diagnostic.summary.contains("cast")),
        "{:?}",
        program.diagnostics
    );
}

#[test]
fn const_float_compound_assignments_update_values() {
    let root = temp_dir("const_float_compound_assignments_update_values");
    write(
        &root.join("main.nia"),
        r#"
const fn width() usize {
    let mut value: f64 = 1.5f64;
    value += 2.5f64;
    value *= 2.0f64;
    value as usize
}

const n: usize = width();
static mut value: i32 = n as i32;
"#,
    );

    let program = codegen_program(root.join("main.nia").to_string_lossy().into_owned());
    assert!(program.diagnostics.is_empty(), "{:?}", program.diagnostics);
    let main_module = program
        .backend_lowering
        .program
        .modules
        .iter()
        .find(|module| module.name.ends_with("main.nia"))
        .expect("main module");
    let value = main_module
        .globals
        .iter()
        .find(|global| global.name == sym("value"))
        .expect("value global");
    assert_eq!(value.init, Some(static_int(8)));
}

#[test]
fn generic_const_function_infers_type_arg_from_negative_float() {
    let root = temp_dir("generic_const_function_infers_type_arg_from_negative_float");
    write(
        &root.join("main.nia"),
        r#"
const fn id[T](value: T) T {
    value
}

const value: f64 = id(-1.5f64);
const n: usize = if value < 0.0f64 { 4usize } else { 0usize };

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
fn generic_const_function_rejects_mismatched_equality_operands() {
    let root = temp_dir("generic_const_function_rejects_mismatched_equality_operands");
    write(
        &root.join("main.nia"),
        r#"
const fn id[T](value: T) T {
    value
}

const value: bool = id(1usize == true);
"#,
    );

    let program = check_program(root.join("main.nia").to_string_lossy().into_owned());
    assert!(
        program.diagnostics.iter().any(|diagnostic| diagnostic
            .diagnostic
            .summary
            .contains("matching operand types")),
        "{:?}",
        program.diagnostics
    );
}

#[test]
fn generic_const_function_rejects_non_bool_logic_operands() {
    let root = temp_dir("generic_const_function_rejects_non_bool_logic_operands");
    write(
        &root.join("main.nia"),
        r#"
const fn id[T](value: T) T {
    value
}

const value: bool = id(true and 1usize);
"#,
    );

    let program = check_program(root.join("main.nia").to_string_lossy().into_owned());
    assert!(
        program
            .diagnostics
            .iter()
            .any(|diagnostic| diagnostic.diagnostic.summary.contains("bool")),
        "{:?}",
        program.diagnostics
    );
}

#[test]
fn generic_const_function_rejects_non_bool_not_operand() {
    let root = temp_dir("generic_const_function_rejects_non_bool_not_operand");
    write(
        &root.join("main.nia"),
        r#"
const fn id[T](value: T) T {
    value
}

const value: bool = id(not 1usize);
"#,
    );

    let program = check_program(root.join("main.nia").to_string_lossy().into_owned());
    assert!(
        program
            .diagnostics
            .iter()
            .any(|diagnostic| diagnostic.diagnostic.summary.contains("bool")),
        "{:?}",
        program.diagnostics
    );
}

#[test]
fn const_float_values_validate_target_range() {
    let root = temp_dir("const_float_values_validate_target_range");
    write(
        &root.join("main.nia"),
        r#"
const literal: f32 = 1e40f32;
const casted: f32 = 1e40f64 as f32;
"#,
    );

    let program = check_program(root.join("main.nia").to_string_lossy().into_owned());
    assert!(
        program.diagnostics.iter().any(|diagnostic| diagnostic
            .diagnostic
            .summary
            .contains("out of range for f32")),
        "{:?}",
        program.diagnostics
    );
    assert!(
        program.diagnostics.iter().any(|diagnostic| diagnostic
            .diagnostic
            .summary
            .contains("cannot be represented as `f32`")),
        "{:?}",
        program.diagnostics
    );
}

#[test]
fn const_nested_values_validate_primitive_ranges() {
    let root = temp_dir("const_nested_values_validate_primitive_ranges");
    write(
        &root.join("main.nia"),
        r#"
const bytes: [2]u8 = [1u16, 300u16];
const config = {values: [1u16, 300u16]};
const selected: u8 = config.values[1];
"#,
    );

    let program = check_program(root.join("main.nia").to_string_lossy().into_owned());
    let count = program
        .diagnostics
        .iter()
        .filter(|diagnostic| {
            diagnostic
                .diagnostic
                .summary
                .contains("out of range for u8")
        })
        .count();
    assert!(count >= 2, "{:?}", program.diagnostics);
}

#[test]
fn const_nominal_struct_values_validate_field_ranges() {
    let root = temp_dir("const_nominal_struct_values_validate_field_ranges");
    write(
        &root.join("main.nia"),
        r#"
struct Packet[T] {
    tag: u8,
    payload: T,
}

const packet: Packet[u8] = Packet[u8]{tag: 1u16, payload: 300u16};
"#,
    );

    let program = check_program(root.join("main.nia").to_string_lossy().into_owned());
    let count = program
        .diagnostics
        .iter()
        .filter(|diagnostic| {
            diagnostic
                .diagnostic
                .summary
                .contains("out of range for u8")
        })
        .count();
    assert!(count >= 1, "{:?}", program.diagnostics);
}

#[test]
fn imported_const_nominal_struct_values_validate_field_ranges() {
    let root = temp_dir("imported_const_nominal_struct_values_validate_field_ranges");
    write(
        &root.join("config.nia"),
        r#"
pub struct Packet[T] {
    tag: u8,
    payload: T,
}

pub const packet: Packet[u8] = Packet[u8]{tag: 1u16, payload: 300u16};
"#,
    );
    write(
        &root.join("main.nia"),
        r#"
module config;
using entry::config;

const selected: u8 = config::packet.payload;
"#,
    );

    let program = check_program(root.join("main.nia").to_string_lossy().into_owned());
    assert!(
        program.diagnostics.iter().any(|diagnostic| diagnostic
            .diagnostic
            .summary
            .contains("out of range for u8")),
        "{:?}",
        program.diagnostics
    );
}

#[test]
fn const_string_literals_are_typed_arrays() {
    let root = temp_dir("const_string_literals_are_typed_arrays");
    write(
        &root.join("main.nia"),
        r#"
const fn accept4(value: [4]char) usize {
    if value[0] == 'n' {
        4usize
    } else {
        0usize
    }
}

const fn id[T](value: T) T {
    value
}

const text: [4]char = id("nia!");
const n: usize = accept4(text);

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
fn const_array_len_method_evaluates_array_values() {
    let root = temp_dir("const_array_len_method_evaluates_array_values");
    write(
        &root.join("main.nia"),
        r#"
const fn total(values: [3]usize, text: [4]char) usize {
    values.len() + text.len() + (&values[1..]).len()
}

const n: usize = total([1usize, 2usize, 3usize], "nia!");

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
fn ordinary_user_len_impl_runs_at_comptime_and_runtime() {
    let root = temp_dir("ordinary_user_len_impl_runs_at_comptime_and_runtime");
    write(
        &root.join("main.nia"),
        r#"
struct Window {
    start: usize,
    end: usize,
}

extend Window : Len {
    const fn len(&self) usize {
        self.end - self.start
    }
}

const fn width(value: Window) usize {
    value.len()
}

const n: usize = width({ start: 2usize, end: 7usize });

fn main() i32 {
    let value: Window = { start: 4usize, end: 9usize };
    let mut values: [n]i32 = [0; n];
    (value.len() + values.len()) as i32
}
"#,
    );

    let program = check_program(root.join("main.nia").to_string_lossy().into_owned());
    assert!(program.diagnostics.is_empty(), "{:?}", program.diagnostics);
}

#[test]
fn layout_array_length_infers_const_generic_at_comptime_and_runtime() {
    let root = temp_dir("layout_array_length_infers_const_generic_at_comptime_and_runtime");
    write(
        &root.join("main.nia"),
        r#"
struct Header {
    first: i32,
    second: i32,
}

const fn arrayLen[T, N: usize](value: [N]T) usize {
    value.len()
}

const n: usize = arrayLen(
    [std::builtin::size[Header]()]u8[0u8; std::builtin::size[Header]()]
);

fn main() i32 {
    let values: [std::builtin::size[Header]()]u8 =
        [0u8; std::builtin::size[Header]()];
    (n + arrayLen(values)) as i32
}
"#,
    );

    let program = codegen_program(root.join("main.nia").to_string_lossy().into_owned());
    assert!(program.diagnostics.is_empty(), "{:?}", program.diagnostics);
}

#[test]
fn const_range_start_and_end_methods_evaluate_bounds() {
    let root = temp_dir("const_range_start_and_end_methods_evaluate_bounds");
    write(
        &root.join("main.nia"),
        r#"
const fn total() usize {
    let both = 2usize..5usize;
    let from = 7usize..;
    let to = ..11usize;
    both.start() + both.end() + from.start() + to.end()
}

const n: usize = total();

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
fn const_slice_pointer_methods_project_the_first_element() {
    let root = temp_dir("const_slice_pointer_methods_project_the_first_element");
    write(
        &root.join("main.nia"),
        r#"
const fn first(values: &[usize]) usize {
    let mut ptr = values.ptr();
    ptr.*
}

const fn first_mut(values: &mut [usize]) usize {
    let mut ptr = values.ptrMut();
    ptr.*
}

const fn read_array(values: [3]usize) usize {
    first(&values[..])
}

const fn read_mut_array() usize {
    let mut values: [2]usize = [4, 5];
    first_mut(&mut values[..])
}

const n: usize = read_array([7usize, 8usize, 9usize]) + read_mut_array();

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
fn const_extension_takes_priority_over_slice_pointer_fallback() {
    let root = temp_dir("const_extension_takes_priority_over_slice_pointer_fallback");
    write(
        &root.join("main.nia"),
        r#"
extend[T] [T] {
    const fn ptr(&self) usize {
        6usize
    }
}

const fn projected(values: [2]usize) usize {
    (&values[..]).ptr()
}

const n: usize = projected([1usize, 2usize]);

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
fn const_pointer_projection_rejects_empty_slice_without_fabricating_provenance() {
    let root =
        temp_dir("const_pointer_projection_rejects_empty_slice_without_fabricating_provenance");
    write(
        &root.join("main.nia"),
        r#"
const fn project() usize {
    let values: [0]usize = [];
    let pointer = (&values[..]).ptr();
    0usize
}

const n: usize = project();
"#,
    );

    let program = check_program(root.join("main.nia").to_string_lossy().into_owned());
    assert!(
        program.diagnostics.iter().any(|diagnostic| diagnostic
            .diagnostic
            .summary
            .contains("const slice pointer method cannot project an empty slice")),
        "{:?}",
        program.diagnostics
    );
}

#[test]
fn const_extension_takes_priority_over_builtin_range_method() {
    let root = temp_dir("const_extension_takes_priority_over_builtin_range_method");
    write(
        &root.join("main.nia"),
        r#"
extend[T] T..T {
    const fn start(&self) T {
        self.end()
    }
}

const n: usize = (1usize..2usize).start();

fn main() i32 {
    let mut values: [n]i32 = [0, 0];
    values.len() as i32
}
"#,
    );

    let program = check_program(root.join("main.nia").to_string_lossy().into_owned());
    assert!(program.diagnostics.is_empty(), "{:?}", program.diagnostics);
}

#[test]
fn runtime_extension_blocks_builtin_range_fallback_at_const() {
    let root = temp_dir("runtime_extension_blocks_builtin_range_fallback_at_const");
    write(
        &root.join("main.nia"),
        r#"
extend usize..usize {
    fn start(&self) usize {
        2usize
    }
}

const n: usize = (1usize..2usize).start();
"#,
    );

    let program = check_program(root.join("main.nia").to_string_lossy().into_owned());
    assert!(
        program.diagnostics.iter().any(|diagnostic| diagnostic
            .diagnostic
            .summary
            .contains("const expression can only call `const fn`")),
        "{:?}",
        program.diagnostics
    );
}

#[test]
fn const_range_start_and_end_require_present_bounds() {
    let root = temp_dir("const_range_start_and_end_require_present_bounds");
    write(
        &root.join("main.nia"),
        r#"
const bad_end: usize = (1usize..).end();
const bad_start: usize = (..2usize).start();
"#,
    );

    let program = check_program(root.join("main.nia").to_string_lossy().into_owned());
    let count = program
        .diagnostics
        .iter()
        .filter(|diagnostic| {
            diagnostic
                .diagnostic
                .summary
                .contains("const range does not have")
        })
        .count();
    assert_eq!(count, 2, "{:?}", program.diagnostics);
}
