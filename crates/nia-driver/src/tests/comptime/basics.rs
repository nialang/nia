// SPDX-License-Identifier: GPL-3.0-or-later
use super::*;

#[test]
fn public_comptime_values_are_visible_through_import_closure() {
    let root = temp_dir("public_comptime_values_are_visible_through_import_closure");
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
pub comptime let answer: i32 = 42;
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
fn comptime_values_drive_static_global_integer_initializers() {
    let root = temp_dir("comptime_values_drive_static_global_integer_initializers");
    write(
        &root.join("main.nia"),
        r#"
comptime let base = 20;
var value: i32 = base + 2;
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
    assert_eq!(value.init, Some(static_int(22)));
}

#[test]
fn comptime_values_drive_array_lengths() {
    let root = temp_dir("comptime_values_drive_array_lengths");
    write(
        &root.join("main.nia"),
        r#"
pub comptime let width: usize = 2 + 2;

fn main() i32 {
    comptime let local_width: usize = width;
    var values: [local_width]i32 = [1, 2, 3, 4];
    values[3]
}
"#,
    );

    let program = check_program(root.join("main.nia").to_string_lossy().into_owned());
    assert!(program.diagnostics.is_empty(), "{:?}", program.diagnostics);
}

#[test]
fn comptime_functions_drive_array_lengths() {
    let root = temp_dir("comptime_functions_drive_array_lengths");
    write(
        &root.join("main.nia"),
        r#"
comptime fn width(base: usize) usize {
    let extra: usize = 2;
    return base + extra;
}

comptime let n: usize = width(2);

fn main() i32 {
    var values: [n]i32 = [0; n];
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
            .all(|function| function.name != "width"),
        "{:?}",
        main_module.functions
    );
}

#[test]
fn comptime_function_if_expression_drives_array_lengths() {
    let root = temp_dir("comptime_function_if_expression_drives_array_lengths");
    write(
        &root.join("main.nia"),
        r#"
comptime fn width(use_wide: bool) usize {
    if use_wide {
        let word: usize = 8;
        word
    } else {
        let word: usize = 4;
        word
    }
}

comptime let bits: usize = 64usize;
comptime let n: usize = width(bits == 64);

fn main() i32 {
    var values: [n]i32 = [0; n];
    values.len() as i32
}
"#,
    );

    let program = check_program(root.join("main.nia").to_string_lossy().into_owned());
    assert!(program.diagnostics.is_empty(), "{:?}", program.diagnostics);
}

#[test]
fn comptime_function_if_branch_return_drives_array_lengths() {
    let root = temp_dir("comptime_function_if_branch_return_drives_array_lengths");
    write(
        &root.join("main.nia"),
        r#"
comptime fn word_bytes(bits: usize) usize {
    if bits == 64 {
        return 8;
    } else {
        return 4;
    }
}

comptime let bits: usize = 64usize;
comptime let n: usize = word_bytes(bits);

fn main() i32 {
    var values: [n]i32 = [0; n];
    values.len() as i32
}
"#,
    );

    let program = check_program(root.join("main.nia").to_string_lossy().into_owned());
    assert!(program.diagnostics.is_empty(), "{:?}", program.diagnostics);
}

#[test]
fn comptime_function_return_expression_propagates_nested_return() {
    let root = temp_dir("comptime_function_return_expression_propagates_nested_return");
    write(
        &root.join("main.nia"),
        r#"
comptime fn width(use_wide: bool) usize {
    return if use_wide {
        return 8;
    } else {
        4
    };
}

comptime let n: usize = width(true);

fn main() i32 {
    var values: [n]i32 = [0; n];
    values.len() as i32
}
"#,
    );

    let program = check_program(root.join("main.nia").to_string_lossy().into_owned());
    assert!(program.diagnostics.is_empty(), "{:?}", program.diagnostics);
}

#[test]
fn comptime_function_switch_expression_drives_array_lengths() {
    let root = temp_dir("comptime_function_switch_expression_drives_array_lengths");
    write(
        &root.join("main.nia"),
        r#"
comptime fn word_bytes(bits: usize) usize {
    switch bits {
        16 => 2,
        32 => 4,
        64 => 8,
        _ => 16,
    }
}

comptime let bits: usize = 64usize;
comptime let n: usize = word_bytes(bits);

fn main() i32 {
    var values: [n]i32 = [0; n];
    values.len() as i32
}
"#,
    );

    let program = check_program(root.join("main.nia").to_string_lossy().into_owned());
    assert!(program.diagnostics.is_empty(), "{:?}", program.diagnostics);
}

#[test]
fn comptime_function_switch_ranges_and_return_arms_drive_array_lengths() {
    let root = temp_dir("comptime_function_switch_ranges_and_return_arms_drive_array_lengths");
    write(
        &root.join("main.nia"),
        r#"
comptime fn bucket(value: usize) usize {
    switch value {
        0..4 => return 4,
        4..8 => 8,
        _ => return 16,
    }
}

comptime let n: usize = bucket(6);

fn main() i32 {
    var values: [n]i32 = [0; n];
    values.len() as i32
}
"#,
    );

    let program = check_program(root.join("main.nia").to_string_lossy().into_owned());
    assert!(program.diagnostics.is_empty(), "{:?}", program.diagnostics);
}

#[test]
fn comptime_function_if_pattern_optional_payload_drives_array_lengths() {
    let root = temp_dir("comptime_function_if_pattern_optional_payload_drives_array_lengths");
    write(
        &root.join("main.nia"),
        r#"
comptime fn unwrap(value: ?usize) usize {
    if let ?payload = value {
        payload
    } else null {
        1
    }
}

comptime let some: ?usize = ?8usize;
comptime let n: usize = unwrap(some);

fn main() i32 {
    var values: [n]i32 = [0; n];
    values.len() as i32
}
"#,
    );

    let program = check_program(root.join("main.nia").to_string_lossy().into_owned());
    assert!(program.diagnostics.is_empty(), "{:?}", program.diagnostics);
}

#[test]
fn comptime_function_if_pattern_error_payload_drives_array_lengths() {
    let root = temp_dir("comptime_function_if_pattern_error_payload_drives_array_lengths");
    write(
        &root.join("main.nia"),
        r#"
comptime fn unwrap(value: usize!usize) usize {
    if let !payload = value {
        payload
    } else err! {
        err
    }
}

comptime let ok: usize!usize = !8;
comptime let n: usize = unwrap(ok);

fn main() i32 {
    var values: [n]i32 = [0; n];
    values.len() as i32
}
"#,
    );

    let program = check_program(root.join("main.nia").to_string_lossy().into_owned());
    assert!(program.diagnostics.is_empty(), "{:?}", program.diagnostics);
}

#[test]
fn function_body_if_uses_comptime_function_condition() {
    let root = temp_dir("function_body_if_uses_comptime_function_condition");
    write(
        &root.join("main.nia"),
        r#"
comptime fn is_native_word(bits: usize) bool {
    bits == 64usize
}

fn main() i32 {
    comptime let native: bool = is_native_word(64usize);
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
fn imported_comptime_functions_are_ordinary_comptime_values() {
    let root = temp_dir("imported_comptime_functions_are_ordinary_comptime_values");
    write(
        &root.join("main.nia"),
        r#"
module config;
using entry::config;

comptime let width: usize = config::width(2);

fn main() i32 {
    var values: [width]i32 = [0; width];
    values.len() as i32
}
"#,
    );
    write(
        &root.join("config.nia"),
        r#"
pub comptime fn width(base: usize) usize {
    base + 2
}
"#,
    );

    let program = check_program(root.join("main.nia").to_string_lossy().into_owned());
    assert!(program.diagnostics.is_empty(), "{:?}", program.diagnostics);
}

#[test]
fn runtime_call_to_comptime_function_is_rejected() {
    let root = temp_dir("runtime_call_to_comptime_function_is_rejected");
    write(
        &root.join("main.nia"),
        r#"
comptime fn width() usize {
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
            .contains("`comptime fn` can only be called from a comptime expression")),
        "{:?}",
        program.diagnostics
    );
}

#[test]
fn comptime_struct_values_drive_field_access() {
    let root = temp_dir("comptime_struct_values_drive_field_access");
    write(
        &root.join("main.nia"),
        r#"
struct Point {
    x: usize,
    y: usize,
}

comptime let p: Point = Point{x: 2, y: 3};
comptime let width: usize = p.x + p.y;

fn main() i32 {
    var values: [width]i32 = [0; width];
    values.len() as i32
}
"#,
    );

    let program = check_program(root.join("main.nia").to_string_lossy().into_owned());
    assert!(program.diagnostics.is_empty(), "{:?}", program.diagnostics);
}

#[test]
fn function_local_comptime_struct_values_drive_field_access() {
    let root = temp_dir("function_local_comptime_struct_values_drive_field_access");
    write(
        &root.join("main.nia"),
        r#"
struct Point {
    x: usize,
    y: usize,
}

fn main() i32 {
    comptime let p: Point = Point{x: 4, y: 2};
    comptime let width: usize = p.x + p.y;
    var values: [width]i32 = [0; width];
    values.len() as i32
}
"#,
    );

    let program = check_program(root.join("main.nia").to_string_lossy().into_owned());
    assert!(program.diagnostics.is_empty(), "{:?}", program.diagnostics);
}

#[test]
fn comptime_array_values_drive_index_access() {
    let root = temp_dir("comptime_array_values_drive_index_access");
    write(
        &root.join("main.nia"),
        r#"
comptime let widths: [3]usize = [2, 4, 8];
comptime let width: usize = widths[1];

fn main() i32 {
    var values: [width]i32 = [0; width];
    values.len() as i32
}
"#,
    );

    let program = check_program(root.join("main.nia").to_string_lossy().into_owned());
    assert!(program.diagnostics.is_empty(), "{:?}", program.diagnostics);
}

#[test]
fn function_local_structural_comptime_array_index_drives_field_access() {
    let root = temp_dir("function_local_structural_comptime_array_index_drives_field_access");
    write(
        &root.join("main.nia"),
        r#"
fn main() i32 {
    comptime let configs = [{width: 2usize}, {width: 4usize}];
    comptime let width: usize = configs[1].width;
    var values: [width]i32 = [0; width];
    values.len() as i32
}
"#,
    );

    let program = check_program(root.join("main.nia").to_string_lossy().into_owned());
    assert!(program.diagnostics.is_empty(), "{:?}", program.diagnostics);
}

#[test]
fn comptime_struct_array_fields_are_ordinary_values() {
    let root = temp_dir("comptime_struct_array_fields_are_ordinary_values");
    write(
        &root.join("main.nia"),
        r#"
struct Config {
    widths: [3]usize,
}

comptime let config: Config = Config{widths: [2, 4, 8]};
comptime let width: usize = config.widths[2];

fn main() i32 {
    var values: [width]i32 = [0; width];
    values.len() as i32
}
"#,
    );

    let program = check_program(root.join("main.nia").to_string_lossy().into_owned());
    assert!(program.diagnostics.is_empty(), "{:?}", program.diagnostics);
}

#[test]
fn comptime_functions_accept_array_values() {
    let root = temp_dir("comptime_functions_accept_array_values");
    write(
        &root.join("main.nia"),
        r#"
comptime fn pick(widths: [3]usize, index: usize) usize {
    widths[index]
}

comptime let widths: [3]usize = [2, 4, 8];
comptime let width: usize = pick(widths, 2);

fn main() i32 {
    var values: [width]i32 = [0; width];
    values.len() as i32
}
"#,
    );

    let program = check_program(root.join("main.nia").to_string_lossy().into_owned());
    assert!(program.diagnostics.is_empty(), "{:?}", program.diagnostics);
}

#[test]
fn comptime_array_slices_are_ordinary_comptime_values() {
    let root = temp_dir("comptime_array_slices_are_ordinary_comptime_values");
    write(
        &root.join("main.nia"),
        r#"
comptime fn pair_sum(values: [2]usize) usize {
    values[0] + values[1]
}

comptime let values: [4]usize = [1, 2, 3, 4];
comptime let middle: [2]usize = values[1..3];
comptime let prefix: [2]usize = values[..2];
comptime let suffix: [2]usize = values[2..];
comptime let direct: usize = pair_sum(values[1..=2]);
comptime let n: usize = pair_sum(middle) + pair_sum(prefix) + pair_sum(suffix) + direct;

fn main() i32 {
    var array: [n]i32 = [0; n];
    array.len() as i32
}
"#,
    );

    let program = check_program(root.join("main.nia").to_string_lossy().into_owned());
    assert!(program.diagnostics.is_empty(), "{:?}", program.diagnostics);
}

#[test]
fn comptime_function_try_propagates_optional_values() {
    let root = temp_dir("comptime_function_try_propagates_optional_values");
    write(
        &root.join("main.nia"),
        r#"
comptime fn add_one(value: ?usize) ?usize {
    let unwrapped: usize = value.?;
    if unwrapped == 7 {
        ?(unwrapped + 1)
    } else {
        ?1
    }
}

comptime let some: ?usize = add_one(?7usize);
comptime let none: ?usize = add_one(null);
comptime let width: usize = if let ?payload = some {
    payload
} else null {
    1
};
comptime let fallback: usize = if let ?payload = none {
    payload
} else null {
    2
};

fn main() i32 {
    var values: [width + fallback]i32 = [0; width + fallback];
    values.len() as i32
}
"#,
    );

    let program = check_program(root.join("main.nia").to_string_lossy().into_owned());
    assert!(program.diagnostics.is_empty(), "{:?}", program.diagnostics);
}

#[test]
fn comptime_function_try_propagates_error_values() {
    let root = temp_dir("comptime_function_try_propagates_error_values");
    write(
        &root.join("main.nia"),
        r#"
comptime fn add_one(value: usize!usize) usize!usize {
    !(value.? + 1)
}

comptime let ok: usize!usize = add_one(!7usize);
comptime let err: usize!usize = add_one(3usize!);
comptime let width: usize = if let !payload = ok {
    payload
} else err! {
    0
};
comptime let fallback: usize = if let !payload = err {
    payload
} else err_payload! {
    2
};

fn main() i32 {
    var values: [width + fallback]i32 = [0; width + fallback];
    values.len() as i32
}
"#,
    );

    let program = check_program(root.join("main.nia").to_string_lossy().into_owned());
    assert!(program.diagnostics.is_empty(), "{:?}", program.diagnostics);
}
