// SPDX-License-Identifier: GPL-3.0-or-later
use super::*;

#[test]
fn nominal_const_struct_fields_have_typed_values() {
    let root = temp_dir("nominal_const_struct_fields_have_typed_values");
    write(
        &root.join("main.nia"),
        r#"
const fn id[T](value: T) T {
    value
}

struct Config { width: usize, enabled: bool }
const config = Config { width: 4usize, enabled: true };
const n: usize = id(config.width);

fn main() i32 {
    let mut values: [i32; n] = [0; n];
    values.len() as i32
}
"#,
    );

    let program = check_program(root.join("main.nia").to_string_lossy().into_owned());
    assert!(program.diagnostics.is_empty(), "{:?}", program.diagnostics);
}

#[test]
fn nested_nominal_const_struct_fields_have_typed_values() {
    let root = temp_dir("nested_nominal_const_struct_fields_have_typed_values");
    write(
        &root.join("main.nia"),
        r#"
const fn id[T](value: T) T {
    value
}

struct Target { word_bits: usize }
struct Config { target: Target }
const config = Config { target: Target { word_bits: 64usize } };
const n: usize = id(config.target.word_bits / 8usize);

fn main() i32 {
    let mut values: [i32; n] = [0; n];
    values.len() as i32
}
"#,
    );

    let program = check_program(root.join("main.nia").to_string_lossy().into_owned());
    assert!(program.diagnostics.is_empty(), "{:?}", program.diagnostics);
}

#[test]
fn imported_nominal_const_struct_fields_have_typed_values() {
    let root = temp_dir("imported_nominal_const_struct_fields_have_typed_values");
    write(
        &root.join("config.nia"),
        r#"
pub struct Config { width: usize }
pub const config = Config { width: 4usize };
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
    let mut values: [i32; n] = [0; n];
    values.len() as i32
}
"#,
    );

    let program = check_program(root.join("main.nia").to_string_lossy().into_owned());
    assert!(program.diagnostics.is_empty(), "{:?}", program.diagnostics);
}

#[test]
fn const_function_local_nominal_struct_fields_have_typed_values() {
    let root = temp_dir("const_function_local_nominal_struct_fields_have_typed_values");
    write(
        &root.join("main.nia"),
        r#"
const fn id[T](value: T) T {
    value
}

struct Target { word_bits: usize }
struct Config { target: Target }

const fn width() usize {
    const config = Config { target: Target { word_bits: 64usize } };
    id(config.target.word_bits) / 8usize
}

const n: usize = width();

fn main() i32 {
    let mut values: [i32; n] = [0; n];
    values.len() as i32
}
"#,
    );

    let program = check_program(root.join("main.nia").to_string_lossy().into_owned());
    assert!(program.diagnostics.is_empty(), "{:?}", program.diagnostics);
}

#[test]
fn const_if_nominal_struct_fields_have_typed_values() {
    let root = temp_dir("const_if_nominal_struct_fields_have_typed_values");
    write(
        &root.join("main.nia"),
        r#"
const fn id[T](value: T) T {
    value
}

struct Config { width: usize }

const config = if true {
    Config { width: 4usize }
} else {
    Config { width: 8usize }
};
const n: usize = id(config.width);

fn main() i32 {
    let mut values: [i32; n] = [0; n];
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

struct Config { width: usize }

const config = if true {
    Config { width: 4usize }
} else {
    Config { width: 8usize }
};
const n: usize = id(config.width);

fn main() i32 {
    let mut values: [i32; n] = [0; n];
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
    let mut values: [i32; n] = [0; n];
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
    let mut values: [i32; n] = [0; n];
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
    let mut values: [i32; n] = [0; n];
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
    let mut values: [i32; n] = [0; n];
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
fn bit_counting_builtins_are_const_evaluable_at_target_width() {
    let root = temp_dir("bit_counting_builtins_are_const_evaluable_at_target_width");
    write(
        &root.join("main.nia"),
        r#"
const fn trailing(value: u8) u8 {
    std::builtin::ctz[u8](value)
}

const trailing_zero: u8 = std::builtin::ctz[u8](0u8);
const leading_zero: u8 = std::builtin::clz[u8](0u8);
const leading_one: u8 = std::builtin::clz[u8](1u8);
const trailing_eight: u8 = trailing(8u8);
const signed_leading: i8 = std::builtin::clz[i8](-2i8);
const signed_popcount: i8 = std::builtin::popcount[i8](-1i8);
const signed_wide_zero: i128 = std::builtin::ctz[i128](0i128);
const zero_popcount: usize = std::builtin::popcount[usize](0usize);

static mut trailing_zero_global: u8 = trailing_zero;
static mut leading_zero_global: u8 = leading_zero;
static mut leading_one_global: u8 = leading_one;
static mut trailing_eight_global: u8 = trailing_eight;
static mut signed_leading_global: i8 = signed_leading;
static mut signed_popcount_global: i8 = signed_popcount;
static mut signed_wide_zero_global: i128 = signed_wide_zero;
static mut zero_popcount_global: usize = zero_popcount;
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
    let global_init = |name| {
        main_module
            .globals
            .iter()
            .find(|global| global.name == sym(name))
            .and_then(|global| global.init.clone())
    };
    assert_eq!(
        global_init("trailing_zero_global"),
        Some(StaticInit::Int(IntConst::unsigned(8)))
    );
    assert_eq!(
        global_init("leading_zero_global"),
        Some(StaticInit::Int(IntConst::unsigned(8)))
    );
    assert_eq!(
        global_init("leading_one_global"),
        Some(StaticInit::Int(IntConst::unsigned(7)))
    );
    assert_eq!(
        global_init("trailing_eight_global"),
        Some(StaticInit::Int(IntConst::unsigned(3)))
    );
    assert_eq!(
        global_init("signed_leading_global"),
        Some(StaticInit::Int(IntConst::from_i128(0)))
    );
    assert_eq!(
        global_init("signed_popcount_global"),
        Some(StaticInit::Int(IntConst::from_i128(8)))
    );
    assert_eq!(
        global_init("signed_wide_zero_global"),
        Some(StaticInit::Int(IntConst::from_i128(128)))
    );
    assert_eq!(
        global_init("zero_popcount_global"),
        Some(StaticInit::Int(IntConst::unsigned(0)))
    );
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
fn inferred_const_float_suffixes_reach_driver_checking() {
    let root = temp_dir("inferred_const_float_suffixes_reach_driver_checking");
    write(
        &root.join("main.nia"),
        r#"
const fraction = 1.0f32;
const exponent = 1e3f32;
const value: f32 = fraction + exponent;
"#,
    );

    let program = check_program(root.join("main.nia").to_string_lossy().into_owned());
    assert!(program.diagnostics.is_empty(), "{:?}", program.diagnostics);
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
fn generic_const_function_rejects_nominal_cast_operand() {
    let root = temp_dir("generic_const_function_rejects_nominal_cast_operand");
    write(
        &root.join("main.nia"),
        r#"
const fn id[T](value: T) T {
    value
}

struct Config { width: usize }
const n: usize = id((Config { width: 4usize }) as usize);
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
fn resolved_f32_operations_round_each_const_intermediate() {
    let root = temp_dir("resolved_f32_operations_round_each_const_intermediate");
    write(
        &root.join("main.nia"),
        r#"
const fn rounded_binary() usize {
    (16777216.0f32 + 1.0f32) as usize
}

const fn rounded_up_binary() usize {
    (16777216.0f32 + 3.0f32) as usize
}

const fn rounded_compound() usize {
    let mut value: f32 = 16777216.0f32;
    value += 1.0f32;
    value as usize
}

const binary = rounded_binary();
const rounded_up = rounded_up_binary();
const compound = rounded_compound();

static mut binary_global: usize = binary;
static mut rounded_up_global: usize = rounded_up;
static mut compound_global: usize = compound;
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
    let global_init = |name| {
        main_module
            .globals
            .iter()
            .find(|global| global.name == sym(name))
            .unwrap_or_else(|| panic!("{name} global"))
            .init
            .clone()
    };
    assert_eq!(
        global_init("binary_global"),
        Some(StaticInit::Int(IntConst::unsigned(16_777_216)))
    );
    assert_eq!(
        global_init("rounded_up_global"),
        Some(StaticInit::Int(IntConst::unsigned(16_777_220)))
    );
    assert_eq!(
        global_init("compound_global"),
        Some(StaticInit::Int(IntConst::unsigned(16_777_216)))
    );
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
    let mut values: [i32; n] = [0; n];
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
const bytes: [u8; 2] = [1u16, 300u16];
struct Config { values: [u8; 2] }
const config = Config { values: [1u16, 300u16] };
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
const fn accept4(value: [char; 4]) usize {
    if value[0] == 'n' {
        4usize
    } else {
        0usize
    }
}

const fn id[T](value: T) T {
    value
}

const text: [char; 4] = id("nia!");
const n: usize = accept4(text);

fn main() i32 {
    let mut values: [i32; n] = [0; n];
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
const fn total(values: [usize; 3], text: [char; 4]) usize {
    values.len() + text.len() + (&values[1..]).len()
}

const n: usize = total([1usize, 2usize, 3usize], "nia!");

fn main() i32 {
    let mut values: [i32; n] = [0; n];
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

const n: usize = width(Window { start: 2usize, end: 7usize });

fn main() i32 {
    let value = Window { start: 4usize, end: 9usize };
    let mut values: [i32; n] = [0; n];
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

const fn arrayLen[T, N: usize](value: [T; N]) usize {
    value.len()
}

const n: usize = arrayLen(
    [0u8; std::builtin::size[Header]()]
);

fn main() i32 {
    let values: [u8; std::builtin::size[Header]()] =
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
    let mut values: [i32; n] = [0; n];
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

const fn read_array(values: [usize; 3]) usize {
    first(&values[..])
}

const fn read_mut_array() usize {
    let mut values: [usize; 2] = [4, 5];
    first_mut(&mut values[..])
}

const n: usize = read_array([7usize, 8usize, 9usize]) + read_mut_array();

fn main() i32 {
    let mut values: [i32; n] = [0; n];
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

const fn projected(values: [usize; 2]) usize {
    (&values[..]).ptr()
}

const n: usize = projected([1usize, 2usize]);

fn main() i32 {
    let mut values: [i32; n] = [0; n];
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
    let values: [usize; 0] = [];
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
    let mut values: [i32; n] = [0, 0];
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
