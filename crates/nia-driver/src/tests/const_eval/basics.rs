// SPDX-License-Identifier: GPL-3.0-or-later
use super::*;

#[test]
fn public_const_values_are_visible_through_import_closure() {
    let root = temp_dir("public_const_values_are_visible_through_import_closure");
    write(
        &root.join("main.nia"),
        r#"
module facade;
module defs;
using entry::facade;

fn main() i32 {
    facade::answer
}
"#,
    );
    write(
        &root.join("facade.nia"),
        r#"
using entry::defs;
pub using defs::answer;
"#,
    );
    write(
        &root.join("defs.nia"),
        r#"
pub const answer: i32 = 42;
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
    assert!(main_module.globals.is_empty());
}

#[test]
fn const_values_drive_static_global_integer_initializers() {
    let root = temp_dir("const_values_drive_static_global_integer_initializers");
    write(
        &root.join("main.nia"),
        r#"
const base = 20;
static mut value: i32 = base + 2;
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
    assert_eq!(value.init, Some(static_int(22)));
}

#[test]
fn const_values_drive_array_lengths() {
    let root = temp_dir("const_values_drive_array_lengths");
    write(
        &root.join("main.nia"),
        r#"
pub const width: usize = 2 + 2;

fn main() i32 {
    const local_width: usize = width;
    let mut values: [local_width]i32 = [1, 2, 3, 4];
    values[3]
}
"#,
    );

    let program = check_program(root.join("main.nia").to_string_lossy().into_owned());
    assert!(program.diagnostics.is_empty(), "{:?}", program.diagnostics);
}

#[test]
fn const_functions_drive_array_lengths() {
    let root = temp_dir("const_functions_drive_array_lengths");
    write(
        &root.join("main.nia"),
        r#"
const fn width(base: usize) usize {
    let extra: usize = 2;
    return base + extra;
}

const n: usize = width(2);

fn main() i32 {
    let mut values: [n]i32 = [0; n];
    values.len() as i32
}
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
    assert!(
        main_module
            .functions
            .iter()
            .all(|function| function.name != sym("width")),
        "{:?}",
        main_module.functions
    );
}

#[test]
fn const_function_if_expression_drives_array_lengths() {
    let root = temp_dir("const_function_if_expression_drives_array_lengths");
    write(
        &root.join("main.nia"),
        r#"
const fn width(use_wide: bool) usize {
    if use_wide {
        let word: usize = 8;
        word
    } else {
        let word: usize = 4;
        word
    }
}

const bits: usize = 64usize;
const n: usize = width(bits == 64);

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
fn const_function_if_branch_return_drives_array_lengths() {
    let root = temp_dir("const_function_if_branch_return_drives_array_lengths");
    write(
        &root.join("main.nia"),
        r#"
const fn word_bytes(bits: usize) usize {
    if bits == 64 {
        return 8;
    } else {
        return 4;
    }
}

const bits: usize = 64usize;
const n: usize = word_bytes(bits);

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
fn const_function_return_expression_propagates_nested_return() {
    let root = temp_dir("const_function_return_expression_propagates_nested_return");
    write(
        &root.join("main.nia"),
        r#"
const fn width(use_wide: bool) usize {
    return if use_wide {
        return 8;
    } else {
        4
    };
}

const n: usize = width(true);

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
fn const_function_switch_expression_drives_array_lengths() {
    let root = temp_dir("const_function_switch_expression_drives_array_lengths");
    write(
        &root.join("main.nia"),
        r#"
const fn word_bytes(bits: usize) usize {
    switch bits {
        16 => 2,
        32 => 4,
        64 => 8,
        _ => 16,
    }
}

const bits: usize = 64usize;
const n: usize = word_bytes(bits);

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
fn const_function_switch_ranges_and_return_arms_drive_array_lengths() {
    let root = temp_dir("const_function_switch_ranges_and_return_arms_drive_array_lengths");
    write(
        &root.join("main.nia"),
        r#"
const fn bucket(value: usize) usize {
    switch value {
        0..4 => return 4,
        4..8 => 8,
        _ => return 16,
    }
}

const n: usize = bucket(6);

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
fn const_function_if_pattern_optional_payload_drives_array_lengths() {
    let root = temp_dir("const_function_if_pattern_optional_payload_drives_array_lengths");
    write(
        &root.join("main.nia"),
        r#"
const fn unwrap(value: ?usize) usize {
    switch value {
        ?payload => {
            payload
        },
        null => {
            1
        },
    }
}

const some: ?usize = ?8usize;
const n: usize = unwrap(some);

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
fn const_function_if_pattern_error_payload_drives_array_lengths() {
    let root = temp_dir("const_function_if_pattern_error_payload_drives_array_lengths");
    write(
        &root.join("main.nia"),
        r#"
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

const ok: usize!usize = !8;
const n: usize = unwrap(ok);

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
fn function_body_if_uses_const_function_condition() {
    let root = temp_dir("function_body_if_uses_const_function_condition");
    write(
        &root.join("main.nia"),
        r#"
const fn is_native_word(bits: usize) bool {
    bits == 64usize
}

fn main() i32 {
    const native: bool = is_native_word(64usize);
    if native {
        1
    } else {
        0
    }
}
"#,
    );

    let program = check_program(root.join("main.nia").to_string_lossy().into_owned());
    assert!(program.diagnostics.is_empty(), "{:?}", program.diagnostics);
}

#[test]
fn imported_const_functions_are_ordinary_const_values() {
    let root = temp_dir("imported_const_functions_are_ordinary_const_values");
    write(
        &root.join("main.nia"),
        r#"
module config;
using entry::config;

const width: usize = config::width(2);

fn main() i32 {
    let mut values: [width]i32 = [0; width];
    values.len() as i32
}
"#,
    );
    write(
        &root.join("config.nia"),
        r#"
pub const fn width(base: usize) usize {
    base + 2
}
"#,
    );

    let program = check_program(root.join("main.nia").to_string_lossy().into_owned());
    assert!(program.diagnostics.is_empty(), "{:?}", program.diagnostics);
}

#[test]
fn runtime_call_to_const_function_is_rejected() {
    let root = temp_dir("runtime_call_to_const_function_is_rejected");
    write(
        &root.join("main.nia"),
        r#"
const fn width() usize {
    4
}

fn main() i32 {
    width() as i32
}
"#,
    );

    let program = check_program(root.join("main.nia").to_string_lossy().into_owned());
    assert!(
        program.diagnostics.iter().any(|diagnostic| diagnostic
            .diagnostic
            .summary
            .contains("`const fn` can only be called from a const expression")),
        "{:?}",
        program.diagnostics
    );
}

#[test]
fn const_struct_values_drive_field_access() {
    let root = temp_dir("const_struct_values_drive_field_access");
    write(
        &root.join("main.nia"),
        r#"
struct Point {
    x: usize,
    y: usize,
}

const p: Point = Point{x: 2, y: 3};
const width: usize = p.x + p.y;

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
fn function_local_const_struct_values_drive_field_access() {
    let root = temp_dir("function_local_const_struct_values_drive_field_access");
    write(
        &root.join("main.nia"),
        r#"
struct Point {
    x: usize,
    y: usize,
}

fn main() i32 {
    const p: Point = Point{x: 4, y: 2};
    const width: usize = p.x + p.y;
    let mut values: [width]i32 = [0; width];
    values.len() as i32
}
"#,
    );

    let program = check_program(root.join("main.nia").to_string_lossy().into_owned());
    assert!(program.diagnostics.is_empty(), "{:?}", program.diagnostics);
}

#[test]
fn const_array_values_drive_index_access() {
    let root = temp_dir("const_array_values_drive_index_access");
    write(
        &root.join("main.nia"),
        r#"
const widths: [3]usize = [2, 4, 8];
const width: usize = widths[1];

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
fn function_local_structural_const_array_index_drives_field_access() {
    let root = temp_dir("function_local_structural_const_array_index_drives_field_access");
    write(
        &root.join("main.nia"),
        r#"
fn main() i32 {
    const configs = [{width: 2usize}, {width: 4usize}];
    const width: usize = configs[1].width;
    let mut values: [width]i32 = [0; width];
    values.len() as i32
}
"#,
    );

    let program = check_program(root.join("main.nia").to_string_lossy().into_owned());
    assert!(program.diagnostics.is_empty(), "{:?}", program.diagnostics);
}

#[test]
fn const_struct_array_fields_are_ordinary_values() {
    let root = temp_dir("const_struct_array_fields_are_ordinary_values");
    write(
        &root.join("main.nia"),
        r#"
struct Config {
    widths: [3]usize,
}

const config: Config = Config{widths: [2, 4, 8]};
const width: usize = config.widths[2];

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
fn const_functions_accept_array_values() {
    let root = temp_dir("const_functions_accept_array_values");
    write(
        &root.join("main.nia"),
        r#"
const fn pick(widths: [3]usize, index: usize) usize {
    widths[index]
}

const widths: [3]usize = [2, 4, 8];
const width: usize = pick(widths, 2);

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
fn const_array_slices_are_ordinary_const_values() {
    let root = temp_dir("const_array_slices_are_ordinary_const_values");
    write(
        &root.join("main.nia"),
        r#"
const fn pair_sum(values: [2]usize) usize {
    values[0] + values[1]
}

const values: [4]usize = [1, 2, 3, 4];
const middle: [2]usize = values[1..3];
const prefix: [2]usize = values[..2];
const suffix: [2]usize = values[2..];
const direct: usize = pair_sum(values[1..=2]);
const n: usize = pair_sum(middle) + pair_sum(prefix) + pair_sum(suffix) + direct;

fn main() i32 {
    let mut array: [n]i32 = [0; n];
    array.len() as i32
}
"#,
    );

    let program = check_program(root.join("main.nia").to_string_lossy().into_owned());
    assert!(program.diagnostics.is_empty(), "{:?}", program.diagnostics);
}

#[test]
fn const_function_try_propagates_optional_values() {
    let root = temp_dir("const_function_try_propagates_optional_values");
    write(
        &root.join("main.nia"),
        r#"
const fn add_one(value: ?usize) ?usize {
    let unwrapped: usize = value.?;
    if unwrapped == 7 {
        ?(unwrapped + 1)
    } else {
        ?1
    }
}

const some: ?usize = add_one(?7usize);
const none: ?usize = add_one(null);
const width: usize = switch some {
    ?payload => {
        payload
    },
    null => {
        1
    },
};
const fallback: usize = switch none {
    ?payload => {
        payload
    },
    null => {
        2
    },
};

fn main() i32 {
    let mut values: [width + fallback]i32 = [0; width + fallback];
    values.len() as i32
}
"#,
    );

    let program = check_program(root.join("main.nia").to_string_lossy().into_owned());
    assert!(program.diagnostics.is_empty(), "{:?}", program.diagnostics);
}

#[test]
fn const_function_try_propagates_error_values() {
    let root = temp_dir("const_function_try_propagates_error_values");
    write(
        &root.join("main.nia"),
        r#"
const fn add_one(value: usize!usize) usize!usize {
    !(value.? + 1)
}

const ok: usize!usize = add_one(!7usize);
const err: usize!usize = add_one(3usize!);
const width: usize = switch ok {
    !payload => {
        payload
    },
    err! => {
        0
    },
};
const fallback: usize = switch err {
    !payload => {
        payload
    },
    err_payload! => {
        2
    },
};

fn main() i32 {
    let mut values: [width + fallback]i32 = [0; width + fallback];
    values.len() as i32
}
"#,
    );

    let program = check_program(root.join("main.nia").to_string_lossy().into_owned());
    assert!(program.diagnostics.is_empty(), "{:?}", program.diagnostics);
}
