// SPDX-License-Identifier: GPL-3.0-or-later
use super::*;

#[test]
fn structural_comptime_struct_fields_have_typed_values() {
    let root = temp_dir("structural_comptime_struct_fields_have_typed_values");
    write(
        &root.join("main.nia"),
        r#"
comptime fn id[T](value: T) T {
    value
}

comptime config = {width: 4usize, enabled: true};
comptime n: usize = id(config.width);

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
fn nested_structural_comptime_struct_fields_have_typed_values() {
    let root = temp_dir("nested_structural_comptime_struct_fields_have_typed_values");
    write(
        &root.join("main.nia"),
        r#"
comptime fn id[T](value: T) T {
    value
}

comptime config = {target: {word_bits: 64usize}};
comptime n: usize = id(config.target.word_bits / 8usize);

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
fn imported_structural_comptime_struct_fields_have_typed_values() {
    let root = temp_dir("imported_structural_comptime_struct_fields_have_typed_values");
    write(
        &root.join("config.nia"),
        r#"
pub comptime config = {width: 4usize};
"#,
    );
    write(
        &root.join("main.nia"),
        r#"
module config;
using entry::config;

comptime fn id[T](value: T) T {
    value
}

comptime n: usize = id(config::config.width);

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
fn comptime_function_local_structural_struct_fields_have_typed_values() {
    let root = temp_dir("comptime_function_local_structural_struct_fields_have_typed_values");
    write(
        &root.join("main.nia"),
        r#"
comptime fn id[T](value: T) T {
    value
}

comptime fn width() usize {
    comptime config = {target: {word_bits: 64usize}};
    id(config.target.word_bits) / 8usize
}

comptime n: usize = width();

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
fn comptime_if_structural_struct_fields_have_typed_values() {
    let root = temp_dir("comptime_if_structural_struct_fields_have_typed_values");
    write(
        &root.join("main.nia"),
        r#"
comptime fn id[T](value: T) T {
    value
}

comptime config = if true {
    {width: 4usize}
} else {
    {width: 8usize}
};
comptime n: usize = id(config.width);

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
fn if_expr_is_an_ordinary_comptime_value() {
    let root = temp_dir("if_expr_is_an_ordinary_comptime_value");
    write(
        &root.join("main.nia"),
        r#"
comptime fn id[T](value: T) T {
    value
}

comptime config = if true {
    {width: 4usize}
} else {
    {width: 8usize}
};
comptime n: usize = id(config.width);

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
comptime fn id[T](value: T) T {
    value
}

comptime n: usize = id(if 1usize {
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
fn comptime_fn_returns_if_expr_value() {
    let root = temp_dir("comptime_fn_returns_if_expr_value");
    write(
        &root.join("main.nia"),
        r#"
comptime fn select() usize {
    if true {
        4usize
    } else {
        8usize
    }
}

comptime n: usize = select();

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
fn comptime_char_literals_are_typed_scalar_values() {
    let root = temp_dir("comptime_char_literals_are_typed_scalar_values");
    write(
        &root.join("main.nia"),
        r#"
comptime fn choose_char(value: char) char {
    value
}

comptime fn widen_byte(value: u8) usize {
    value as usize
}

comptime ch: char = choose_char('A');
comptime n: usize = widen_byte(b'\n') + 1usize;

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
fn comptime_bitwise_not_evaluates_integer_values() {
    let root = temp_dir("comptime_bitwise_not_evaluates_integer_values");
    write(
        &root.join("main.nia"),
        r#"
comptime fn mask(value: usize) usize {
    ~value & 15usize
}

comptime n: usize = mask(10usize);

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
fn comptime_casts_have_typed_values() {
    let root = temp_dir("comptime_casts_have_typed_values");
    write(
        &root.join("main.nia"),
        r#"
comptime fn id[T](value: T) T {
    value
}

comptime n: usize = id(b'a' as usize);

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
fn comptime_integer_casts_convert_values() {
    let root = temp_dir("comptime_integer_casts_convert_values");
    write(
        &root.join("main.nia"),
        r#"
comptime fn narrow(value: usize) usize {
    (value as u8) as usize
}

comptime fn signed(value: usize) i32 {
    (value as i8) as i32
}

comptime narrow_value: usize = narrow(258usize);
comptime signed_value: i32 = signed(255usize);

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
        .find(|global| global.name == "narrow_global")
        .expect("narrow_global");
    assert_eq!(narrow_global.init, Some(static_int(2)));
    let signed_global = main_module
        .globals
        .iter()
        .find(|global| global.name == "signed_global")
        .expect("signed_global");
    assert_eq!(signed_global.init, Some(static_int(-1)));
}

#[test]
fn comptime_float_values_drive_casts_and_conditions() {
    let root = temp_dir("comptime_float_values_drive_casts_and_conditions");
    write(
        &root.join("main.nia"),
        r#"
comptime fn scale(value: f64) f64 {
    value * 2.0f64 + 0.5f64
}

comptime fn wide(value: usize) f64 {
    value as f64
}

comptime scaled: f64 = scale(3.25f64);
comptime from_int: f64 = wide(4usize);
comptime n: usize = if scaled > from_int {
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
        .find(|global| global.name == "value")
        .expect("value global");
    assert_eq!(value.init, Some(static_int(7)));
}

#[test]
fn generic_comptime_function_rejects_non_numeric_cast_operand() {
    let root = temp_dir("generic_comptime_function_rejects_non_numeric_cast_operand");
    write(
        &root.join("main.nia"),
        r#"
comptime fn id[T](value: T) T {
    value
}

comptime n: usize = id(true as usize);
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
fn generic_comptime_function_rejects_structural_cast_operand() {
    let root = temp_dir("generic_comptime_function_rejects_structural_cast_operand");
    write(
        &root.join("main.nia"),
        r#"
comptime fn id[T](value: T) T {
    value
}

comptime n: usize = id(({width: 4usize}) as usize);
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
fn comptime_float_compound_assignments_update_values() {
    let root = temp_dir("comptime_float_compound_assignments_update_values");
    write(
        &root.join("main.nia"),
        r#"
comptime fn width() usize {
    comptime mut value: f64 = 1.5f64;
    value += 2.5f64;
    value *= 2.0f64;
    value as usize
}

comptime n: usize = width();
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
        .find(|global| global.name == "value")
        .expect("value global");
    assert_eq!(value.init, Some(static_int(8)));
}

#[test]
fn generic_comptime_function_infers_type_arg_from_negative_float() {
    let root = temp_dir("generic_comptime_function_infers_type_arg_from_negative_float");
    write(
        &root.join("main.nia"),
        r#"
comptime fn id[T](value: T) T {
    value
}

comptime value: f64 = id(-1.5f64);
comptime n: usize = if value < 0.0f64 { 4usize } else { 0usize };

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
fn generic_comptime_function_rejects_mismatched_equality_operands() {
    let root = temp_dir("generic_comptime_function_rejects_mismatched_equality_operands");
    write(
        &root.join("main.nia"),
        r#"
comptime fn id[T](value: T) T {
    value
}

comptime value: bool = id(1usize == true);
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
fn generic_comptime_function_rejects_non_bool_logic_operands() {
    let root = temp_dir("generic_comptime_function_rejects_non_bool_logic_operands");
    write(
        &root.join("main.nia"),
        r#"
comptime fn id[T](value: T) T {
    value
}

comptime value: bool = id(true and 1usize);
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
fn generic_comptime_function_rejects_non_bool_not_operand() {
    let root = temp_dir("generic_comptime_function_rejects_non_bool_not_operand");
    write(
        &root.join("main.nia"),
        r#"
comptime fn id[T](value: T) T {
    value
}

comptime value: bool = id(not 1usize);
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
fn comptime_float_values_validate_target_range() {
    let root = temp_dir("comptime_float_values_validate_target_range");
    write(
        &root.join("main.nia"),
        r#"
comptime literal: f32 = 1e40f32;
comptime casted: f32 = 1e40f64 as f32;
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
fn comptime_nested_values_validate_primitive_ranges() {
    let root = temp_dir("comptime_nested_values_validate_primitive_ranges");
    write(
        &root.join("main.nia"),
        r#"
comptime bytes: [2]u8 = [1u16, 300u16];
comptime config = {values: [1u16, 300u16]};
comptime selected: u8 = config.values[1];
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
fn comptime_nominal_struct_values_validate_field_ranges() {
    let root = temp_dir("comptime_nominal_struct_values_validate_field_ranges");
    write(
        &root.join("main.nia"),
        r#"
struct Packet[T] {
    tag: u8,
    payload: T,
}

comptime packet: Packet[u8] = Packet[u8]{tag: 1u16, payload: 300u16};
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
fn imported_comptime_nominal_struct_values_validate_field_ranges() {
    let root = temp_dir("imported_comptime_nominal_struct_values_validate_field_ranges");
    write(
        &root.join("config.nia"),
        r#"
pub struct Packet[T] {
    tag: u8,
    payload: T,
}

pub comptime packet: Packet[u8] = Packet[u8]{tag: 1u16, payload: 300u16};
"#,
    );
    write(
        &root.join("main.nia"),
        r#"
module config;
using entry::config;

comptime selected: u8 = config::packet.payload;
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
fn comptime_string_literals_are_typed_arrays() {
    let root = temp_dir("comptime_string_literals_are_typed_arrays");
    write(
        &root.join("main.nia"),
        r#"
comptime fn accept4(value: [4]char) usize {
    if value[0] == 'n' {
        4usize
    } else {
        0usize
    }
}

comptime fn id[T](value: T) T {
    value
}

comptime text: [4]char = id("nia!".*);
comptime n: usize = accept4(text);

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
fn comptime_array_len_method_evaluates_array_values() {
    let root = temp_dir("comptime_array_len_method_evaluates_array_values");
    write(
        &root.join("main.nia"),
        r#"
comptime fn total(values: [3]usize, text: [4]char) usize {
    values.len() + text.len() + values[1..].len()
}

comptime n: usize = total([1usize, 2usize, 3usize], "nia!".*);

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
fn comptime_range_start_and_end_methods_evaluate_bounds() {
    let root = temp_dir("comptime_range_start_and_end_methods_evaluate_bounds");
    write(
        &root.join("main.nia"),
        r#"
comptime fn total() usize {
    let both = 2usize..5usize;
    let from = 7usize..;
    let to = ..11usize;
    both.start() + both.end() + from.start() + to.end()
}

comptime n: usize = total();

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
fn comptime_range_start_and_end_require_present_bounds() {
    let root = temp_dir("comptime_range_start_and_end_require_present_bounds");
    write(
        &root.join("main.nia"),
        r#"
comptime bad_end: usize = (1usize..).end();
comptime bad_start: usize = (..2usize).start();
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
                .contains("comptime range does not have")
        })
        .count();
    assert_eq!(count, 2, "{:?}", program.diagnostics);
}
