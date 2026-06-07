// SPDX-License-Identifier: GPL-3.0-or-later
use super::common::*;
use crate::check_program;
use nia_static_ir::StaticInit;

#[test]
fn public_comptime_values_are_visible_through_import_closure() {
    let root = temp_dir("public_comptime_values_are_visible_through_import_closure");
    write(
        &root.join("main.nia"),
        r#"
import .facade;

fn main() i32 {
    facade::answer
}
"#,
    );
    write(
        &root.join("facade.nia"),
        r#"
import .defs;
pub using defs::answer;
"#,
    );
    write(
        &root.join("defs.nia"),
        r#"
pub comptime let answer: i32 = 42;
"#,
    );

    let program = check_program(root.join("main.nia").to_string_lossy().into_owned());
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

    let program = check_program(root.join("main.nia").to_string_lossy().into_owned());
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
    assert_eq!(value.init, Some(StaticInit::Int(22)));
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

    let program = check_program(root.join("main.nia").to_string_lossy().into_owned());
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

comptime let bits: usize = @builtin().target.pointer_width;
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

comptime let bits: usize = @builtin().target.pointer_width;
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

comptime let bits: usize = @builtin().target.pointer_width;
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
fn comptime_function_switch_optional_payload_drives_array_lengths() {
    let root = temp_dir("comptime_function_switch_optional_payload_drives_array_lengths");
    write(
        &root.join("main.nia"),
        r#"
comptime fn unwrap(value: ?usize) usize {
    switch value {
        ?payload => payload,
        null => 1,
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
fn comptime_function_switch_error_payload_drives_array_lengths() {
    let root = temp_dir("comptime_function_switch_error_payload_drives_array_lengths");
    write(
        &root.join("main.nia"),
        r#"
comptime fn unwrap(value: usize!usize) usize {
    switch value {
        !payload => payload,
        err! => err,
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
fn function_body_comptime_if_uses_comptime_function_condition() {
    let root = temp_dir("function_body_comptime_if_uses_comptime_function_condition");
    write(
        &root.join("main.nia"),
        r#"
comptime fn is_native_word(bits: usize) bool {
    bits == @builtin().target.pointer_width
}

fn main() i32 {
    comptime if is_native_word(@builtin().target.pointer_width) {
        1
    } else {
        missing_name
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
import .config;

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
comptime let width: usize = switch some {
    ?payload => payload,
    null => 1,
};
comptime let fallback: usize = switch none {
    ?payload => payload,
    null => 2,
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
comptime let width: usize = switch ok {
    !payload => payload,
    err! => 0,
};
comptime let fallback: usize = switch err {
    !payload => payload,
    err_payload! => 2,
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
fn comptime_function_loop_statements_drive_array_lengths() {
    let root = temp_dir("comptime_function_loop_statements_drive_array_lengths");
    write(
        &root.join("main.nia"),
        r#"
comptime fn width() usize {
    loop {
        break;
    }
    8;
    return 6;
}

comptime let n: usize = width();

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
fn comptime_function_while_statements_drive_array_lengths() {
    let root = temp_dir("comptime_function_while_statements_drive_array_lengths");
    write(
        &root.join("main.nia"),
        r#"
comptime fn width(flag: bool) usize {
    while false {
        return 1;
    }
    while flag {
        break;
    }
    return 7;
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
fn comptime_function_if_statement_rejects_non_bool_condition() {
    let root = temp_dir("comptime_function_if_statement_rejects_non_bool_condition");
    write(
        &root.join("main.nia"),
        r#"
comptime fn width() usize {
    if 1usize {
        return 1usize;
    }
    return 2usize;
}

comptime let n: usize = width();
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
fn comptime_function_while_statement_rejects_non_bool_condition() {
    let root = temp_dir("comptime_function_while_statement_rejects_non_bool_condition");
    write(
        &root.join("main.nia"),
        r#"
comptime fn width() usize {
    while 1usize {
        return 1usize;
    }
    return 2usize;
}

comptime let n: usize = width();
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
fn comptime_statement_type_check_allows_loop_control_flow() {
    let root = temp_dir("comptime_statement_type_check_allows_loop_control_flow");
    write(
        &root.join("main.nia"),
        r#"
comptime fn width(flag: bool) usize {
    if flag {
        return 4usize;
    }
    var total: usize = 0;
    var value: usize = 0;
    while value < 4usize {
        if value == 2usize {
            value += 1;
            continue;
        }
        total += value;
        value += 1;
    }
    while true {
        break;
    }
    total
}

comptime let n: usize = width(false);

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
fn comptime_function_mutable_locals_drive_loop_array_lengths() {
    let root = temp_dir("comptime_function_mutable_locals_drive_loop_array_lengths");
    write(
        &root.join("main.nia"),
        r#"
comptime fn width() usize {
    var i: usize = 0;
    while true {
        if i == 6 {
            break;
        }
        i += 1;
    }
    i
}

comptime let n: usize = width();

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
fn comptime_function_integer_comparisons_drive_control_flow() {
    let root = temp_dir("comptime_function_integer_comparisons_drive_control_flow");
    write(
        &root.join("main.nia"),
        r#"
comptime fn width(limit: usize) usize {
    var i: usize = 0;
    var total: usize = 0;
    while i < limit {
        if i <= 1 {
            total += 1;
        }
        if i >= 3 {
            total += 2;
        }
        if i > 4 {
            total += 4;
        }
        i += 1;
    }
    total
}

comptime let n: usize = width(6);

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
fn comptime_function_supports_plain_local_assignment() {
    let root = temp_dir("comptime_function_supports_plain_local_assignment");
    write(
        &root.join("main.nia"),
        r#"
comptime fn width() usize {
    var i: usize = 2;
    i = 5;
    i *= 2;
    i
}

comptime let n: usize = width();

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
fn comptime_function_rejects_assignment_to_immutable_local() {
    let root = temp_dir("comptime_function_rejects_assignment_to_immutable_local");
    write(
        &root.join("main.nia"),
        r#"
comptime fn width() usize {
    let i: usize = 2;
    i = 5;
    i
}

comptime let n: usize = width();

fn main() i32 {
    var values: [n]i32 = [0; n];
    values.len() as i32
}
"#,
    );

    let program = check_program(root.join("main.nia").to_string_lossy().into_owned());
    assert!(
        program.diagnostics.iter().any(|diagnostic| diagnostic
            .diagnostic
            .summary
            .contains("cannot assign to immutable comptime local `i`")),
        "{:?}",
        program.diagnostics
    );
}

#[test]
fn comptime_function_rejects_assignment_value_type_mismatch() {
    let root = temp_dir("comptime_function_rejects_assignment_value_type_mismatch");
    write(
        &root.join("main.nia"),
        r#"
comptime fn width() usize {
    var i: usize = 2;
    i = true;
    i
}

comptime let n: usize = width();
"#,
    );

    let program = check_program(root.join("main.nia").to_string_lossy().into_owned());
    assert!(
        program.diagnostics.iter().any(|diagnostic| diagnostic
            .diagnostic
            .summary
            .contains("primitive type usize")),
        "{:?}",
        program.diagnostics
    );
}

#[test]
fn comptime_function_rejects_int_assignment_to_bool() {
    let root = temp_dir("comptime_function_rejects_int_assignment_to_bool");
    write(
        &root.join("main.nia"),
        r#"
comptime fn width() usize {
    var value: bool = true;
    value = 1usize;
    1usize
}

comptime let n: usize = width();
"#,
    );

    let program = check_program(root.join("main.nia").to_string_lossy().into_owned());
    assert!(
        program.diagnostics.iter().any(|diagnostic| diagnostic
            .diagnostic
            .summary
            .contains("primitive type bool")),
        "{:?}",
        program.diagnostics
    );
}

#[test]
fn comptime_function_rejects_array_assignment_shape_mismatch() {
    let root = temp_dir("comptime_function_rejects_array_assignment_shape_mismatch");
    write(
        &root.join("main.nia"),
        r#"
comptime fn width() usize {
    var values: [2]usize = [1usize, 2usize];
    values = true;
    values.len()
}

comptime let n: usize = width();
"#,
    );

    let program = check_program(root.join("main.nia").to_string_lossy().into_owned());
    assert!(
        program.diagnostics.iter().any(|diagnostic| diagnostic
            .diagnostic
            .summary
            .contains("expected array type")),
        "{:?}",
        program.diagnostics
    );
}

#[test]
fn comptime_function_rejects_array_assignment_length_mismatch() {
    let root = temp_dir("comptime_function_rejects_array_assignment_length_mismatch");
    write(
        &root.join("main.nia"),
        r#"
comptime fn width() usize {
    var values: [2]usize = [1usize, 2usize];
    values = [1usize; 3usize];
    values.len()
}

comptime let n: usize = width();
"#,
    );

    let program = check_program(root.join("main.nia").to_string_lossy().into_owned());
    assert!(
        program
            .diagnostics
            .iter()
            .any(|diagnostic| diagnostic.diagnostic.summary.contains("expected length 2")),
        "{:?}",
        program.diagnostics
    );
}

#[test]
fn comptime_function_rejects_optional_assignment_shape_mismatch() {
    let root = temp_dir("comptime_function_rejects_optional_assignment_shape_mismatch");
    write(
        &root.join("main.nia"),
        r#"
comptime fn width() usize {
    var value: ?usize = ?1usize;
    value = true;
    1usize
}

comptime let n: usize = width();
"#,
    );

    let program = check_program(root.join("main.nia").to_string_lossy().into_owned());
    assert!(
        program.diagnostics.iter().any(|diagnostic| diagnostic
            .diagnostic
            .summary
            .contains("expected optional type")),
        "{:?}",
        program.diagnostics
    );
}

#[test]
fn comptime_function_rejects_error_union_assignment_shape_mismatch() {
    let root = temp_dir("comptime_function_rejects_error_union_assignment_shape_mismatch");
    write(
        &root.join("main.nia"),
        r#"
comptime fn width() usize {
    var value: usize!usize = !1usize;
    value = true;
    1usize
}

comptime let n: usize = width();
"#,
    );

    let program = check_program(root.join("main.nia").to_string_lossy().into_owned());
    assert!(
        program.diagnostics.iter().any(|diagnostic| diagnostic
            .diagnostic
            .summary
            .contains("expected error union type")),
        "{:?}",
        program.diagnostics
    );
}

#[test]
fn comptime_function_rejects_struct_assignment_missing_field() {
    let root = temp_dir("comptime_function_rejects_struct_assignment_missing_field");
    write(
        &root.join("main.nia"),
        r#"
comptime fn width() usize {
    var config = {width: 4usize, enabled: true};
    config = {width: 8usize};
    config.width
}

comptime let n: usize = width();
"#,
    );

    let program = check_program(root.join("main.nia").to_string_lossy().into_owned());
    assert!(
        program.diagnostics.iter().any(|diagnostic| {
            diagnostic
                .diagnostic
                .summary
                .contains("comptime struct value is missing field `enabled`")
        }),
        "{:?}",
        program.diagnostics
    );
}

#[test]
fn comptime_function_rejects_nominal_struct_assignment_missing_field() {
    let root = temp_dir("comptime_function_rejects_nominal_struct_assignment_missing_field");
    write(
        &root.join("main.nia"),
        r#"
struct Point {
    x: usize,
    y: usize,
}

comptime fn width() usize {
    var p: Point = Point{x: 1usize, y: 2usize};
    p = Point{x: 3usize};
    p.x
}

comptime let n: usize = width();
"#,
    );

    let program = check_program(root.join("main.nia").to_string_lossy().into_owned());
    assert!(
        program.diagnostics.iter().any(|diagnostic| {
            diagnostic
                .diagnostic
                .summary
                .contains("comptime struct value is missing field `y`")
        }),
        "{:?}",
        program.diagnostics
    );
}

#[test]
fn comptime_function_rejects_struct_assignment_extra_field() {
    let root = temp_dir("comptime_function_rejects_struct_assignment_extra_field");
    write(
        &root.join("main.nia"),
        r#"
comptime fn width() usize {
    var config = {width: 4usize};
    config = {width: 8usize, enabled: true};
    config.width
}

comptime let n: usize = width();
"#,
    );

    let program = check_program(root.join("main.nia").to_string_lossy().into_owned());
    assert!(
        program.diagnostics.iter().any(|diagnostic| {
            diagnostic
                .diagnostic
                .summary
                .contains("comptime struct value has extra field `enabled`")
        }),
        "{:?}",
        program.diagnostics
    );
}

#[test]
fn comptime_function_rejects_nominal_struct_assignment_extra_field() {
    let root = temp_dir("comptime_function_rejects_nominal_struct_assignment_extra_field");
    write(
        &root.join("main.nia"),
        r#"
struct Point {
    x: usize,
    y: usize,
}

comptime fn width() usize {
    var p: Point = Point{x: 1usize, y: 2usize};
    p = Point{x: 3usize, y: 4usize, z: 5usize};
    p.x
}

comptime let n: usize = width();
"#,
    );

    let program = check_program(root.join("main.nia").to_string_lossy().into_owned());
    assert!(
        program.diagnostics.iter().any(|diagnostic| {
            diagnostic
                .diagnostic
                .summary
                .contains("comptime struct value has extra field `z`")
        }),
        "{:?}",
        program.diagnostics
    );
}

#[test]
fn comptime_function_mutates_struct_fields() {
    let root = temp_dir("comptime_function_mutates_struct_fields");
    write(
        &root.join("main.nia"),
        r#"
struct Point {
    x: usize,
    y: usize,
}

comptime fn width() usize {
    var p: Point = Point{x: 2, y: 3};
    p.x += 4;
    p.y = p.x + p.y;
    p.y
}

comptime let n: usize = width();

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
fn comptime_function_mutates_array_indexes() {
    let root = temp_dir("comptime_function_mutates_array_indexes");
    write(
        &root.join("main.nia"),
        r#"
comptime fn width() usize {
    var values: [4]usize = [1, 2, 3, 4];
    var i: usize = 0;
    while i < 4 {
        let value = i;
        values[i] += value;
        i += 1;
    }
    values[0] + values[1] + values[2] + values[3]
}

comptime let n: usize = width();

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
fn comptime_for_in_rejects_range_iter_method() {
    let root = temp_dir("comptime_for_in_rejects_range_iter_method");
    write(
        &root.join("main.nia"),
        r#"
comptime fn width() usize {
    var total: usize = 0;
    for value in (0usize..4usize).iter() {
        total += value;
    }
    total
}

comptime let n: usize = width();

fn main() i32 {
    var values: [n]i32 = [0; n];
    values.len() as i32
}
"#,
    );

    let program = check_program(root.join("main.nia").to_string_lossy().into_owned());
    assert!(
        program.diagnostics.iter().any(|diagnostic| diagnostic
            .diagnostic
            .summary
            .contains("comptime expression can only call `comptime fn`")),
        "{:?}",
        program.diagnostics
    );
}

#[test]
fn comptime_function_mutates_nested_aggregate_paths() {
    let root = temp_dir("comptime_function_mutates_nested_aggregate_paths");
    write(
        &root.join("main.nia"),
        r#"
struct Pair {
    values: [2]usize,
}

struct Config {
    pairs: [2]Pair,
}

comptime fn width() usize {
    var config: Config = Config{
        pairs: [
            Pair{values: [1, 2]},
            Pair{values: [3, 4]},
        ],
    };
    config.pairs[1].values[0] = 8;
    config.pairs[0].values[1] += config.pairs[1].values[0];
    config.pairs[0].values[1]
}

comptime let n: usize = width();

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
fn comptime_function_rejects_field_assignment_to_immutable_root() {
    let root = temp_dir("comptime_function_rejects_field_assignment_to_immutable_root");
    write(
        &root.join("main.nia"),
        r#"
struct Point {
    x: usize,
}

comptime fn width() usize {
    let p: Point = Point{x: 1};
    p.x = 2;
    p.x
}

comptime let n: usize = width();

fn main() i32 {
    var values: [n]i32 = [0; n];
    values.len() as i32
}
"#,
    );

    let program = check_program(root.join("main.nia").to_string_lossy().into_owned());
    assert!(
        program.diagnostics.iter().any(|diagnostic| diagnostic
            .diagnostic
            .summary
            .contains("cannot assign to immutable comptime local `p`")),
        "{:?}",
        program.diagnostics
    );
}

#[test]
fn comptime_function_rejects_field_assignment_value_type_mismatch() {
    let root = temp_dir("comptime_function_rejects_field_assignment_value_type_mismatch");
    write(
        &root.join("main.nia"),
        r#"
struct Point {
    x: usize,
}

comptime fn width() usize {
    var p: Point = Point{x: 1};
    p.x = true;
    p.x
}

comptime let n: usize = width();
"#,
    );

    let program = check_program(root.join("main.nia").to_string_lossy().into_owned());
    assert!(
        program.diagnostics.iter().any(|diagnostic| diagnostic
            .diagnostic
            .summary
            .contains("primitive type usize")),
        "{:?}",
        program.diagnostics
    );
}

#[test]
fn comptime_function_if_statement_flows_return_and_else_if() {
    let root = temp_dir("comptime_function_if_statement_flows_return_and_else_if");
    write(
        &root.join("main.nia"),
        r#"
comptime fn width(bits: usize) usize {
    if bits == 16 {
        return 2;
    } else if bits == 32 {
        return 4;
    }
    return 8;
}

comptime let n: usize = width(32);

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
fn comptime_function_for_in_arrays_require_iterator() {
    let root = temp_dir("comptime_function_for_in_arrays_require_iterator");
    write(
        &root.join("main.nia"),
        r#"
comptime fn width(values: [4]usize) usize {
    var total: usize = 0;
    for value in values {
        total += value;
    }
    total
}

comptime let n: usize = width([1, 2, 3, 4]);

fn main() i32 {
    var values: [n]i32 = [0; n];
    values.len() as i32
}
"#,
    );

    let program = check_program(root.join("main.nia").to_string_lossy().into_owned());
    assert!(
        program.diagnostics.iter().any(|diagnostic| diagnostic
            .diagnostic
            .summary
            .contains("comptime for-in expects an Iterator")),
        "{:?}",
        program.diagnostics
    );
}

#[test]
fn comptime_function_for_in_ranges_require_iterator() {
    let root = temp_dir("comptime_function_for_in_ranges_require_iterator");
    write(
        &root.join("main.nia"),
        r#"
comptime fn width() usize {
    var total: usize = 0;
    for value in 0usize..=5usize {
        if value == 2usize {
            continue;
        }
        if value == 5usize {
            break;
        }
        total += value;
    }
    total
}

comptime let n: usize = width();

fn main() i32 {
    var values: [n]i32 = [0; n];
    values.len() as i32
}
"#,
    );

    let program = check_program(root.join("main.nia").to_string_lossy().into_owned());
    assert!(
        program.diagnostics.iter().any(|diagnostic| diagnostic
            .diagnostic
            .summary
            .contains("comptime for-in expects an Iterator")),
        "{:?}",
        program.diagnostics
    );
}

#[test]
fn comptime_function_rejects_for_in_ranges_without_iterator() {
    let root = temp_dir("comptime_function_rejects_for_in_ranges_without_iterator");
    write(
        &root.join("main.nia"),
        r#"
comptime fn width() usize {
    var total: usize = 0;
    for value in ..5usize {
        total += value;
    }
    total
}

comptime let n: usize = width();

fn main() i32 {
    var values: [n]i32 = [0; n];
    values.len() as i32
}
"#,
    );

    let program = check_program(root.join("main.nia").to_string_lossy().into_owned());
    assert!(
        program.diagnostics.iter().any(|diagnostic| diagnostic
            .diagnostic
            .summary
            .contains("comptime for-in expects an Iterator")),
        "{:?}",
        program.diagnostics
    );
}

#[test]
fn comptime_function_for_in_iterator_execution_is_not_duck_typed() {
    let root = temp_dir("comptime_function_for_in_iterator_execution_is_not_duck_typed");
    write(
        &root.join("main.nia"),
        r#"
struct Counter {
    current: usize,
    end: usize,
}

extend Counter : Iterator {
    type Item = usize;

    comptime fn next(&mut self) ?usize {
        null
    }
}

comptime fn width() usize {
    var total: usize = 0;
    var iter = Counter{current: 0, end: 4};
    for value in iter {
        total += value;
    }
    total
}

comptime let n: usize = width();

fn main() i32 {
    var values: [n]i32 = [0; n];
    values.len() as i32
}
"#,
    );

    let program = check_program(root.join("main.nia").to_string_lossy().into_owned());
    assert!(
        program.diagnostics.iter().any(|diagnostic| diagnostic
            .diagnostic
            .summary
            .contains("comptime for-in Iterator execution is not implemented yet")),
        "{:?}",
        program.diagnostics
    );
}

#[test]
fn comptime_function_rejects_escaped_loop_control_flow() {
    let root = temp_dir("comptime_function_rejects_escaped_loop_control_flow");
    write(
        &root.join("main.nia"),
        r#"
comptime fn width() usize {
    break;
}

comptime let n: usize = width();

fn main() i32 {
    var values: [n]i32 = [0; n];
    values.len() as i32
}
"#,
    );

    let program = check_program(root.join("main.nia").to_string_lossy().into_owned());
    assert!(
        program.diagnostics.iter().any(|diagnostic| diagnostic
            .diagnostic
            .summary
            .contains("comptime loop control flow escaped its loop")),
        "{:?}",
        program.diagnostics
    );
}

#[test]
fn comptime_function_rejects_infinite_loop_statements() {
    let root = temp_dir("comptime_function_rejects_infinite_loop_statements");
    write(
        &root.join("main.nia"),
        r#"
comptime fn width() usize {
    loop {
        continue;
    }
    return 1;
}

comptime let n: usize = width();

fn main() i32 {
    var values: [n]i32 = [0; n];
    values.len() as i32
}
"#,
    );

    let program = check_program(root.join("main.nia").to_string_lossy().into_owned());
    assert!(
        program.diagnostics.iter().any(|diagnostic| diagnostic
            .diagnostic
            .summary
            .contains("comptime loop exceeded")),
        "{:?}",
        program.diagnostics
    );
}

#[test]
fn comptime_if_prunes_unselected_function_body_branch() {
    let root = temp_dir("comptime_if_prunes_unselected_function_body_branch");
    write(
        &root.join("main.nia"),
        r#"
fn main() i32 {
    comptime if true {
        1
    } else {
        missing_name
    }
}
"#,
    );

    let program = check_program(root.join("main.nia").to_string_lossy().into_owned());
    assert!(program.diagnostics.is_empty(), "{:?}", program.diagnostics);
}

#[test]
fn comptime_if_accepts_builtin_target_fields_in_function_body() {
    let root = temp_dir("comptime_if_accepts_builtin_target_fields_in_function_body");
    write(
        &root.join("main.nia"),
        r#"
fn main() i32 {
    comptime if @builtin().target.pointer_width == 64
        or @builtin().target.pointer_width == 32 {
        1
    } else {
        missing_name
    }
}
"#,
    );

    let program = check_program(root.join("main.nia").to_string_lossy().into_owned());
    assert!(program.diagnostics.is_empty(), "{:?}", program.diagnostics);
}

#[test]
fn builtin_target_fields_are_ordinary_comptime_values() {
    let root = temp_dir("builtin_target_fields_are_ordinary_comptime_values");
    write(
        &root.join("main.nia"),
        r#"
comptime let bits: usize = @builtin().target.pointer_width;
comptime let word_bytes: usize = bits / 8;

fn main() i32 {
    var bytes: [word_bytes]u8 = [0; word_bytes];
    bytes.len() as i32
}
"#,
    );

    let program = check_program(root.join("main.nia").to_string_lossy().into_owned());
    assert!(program.diagnostics.is_empty(), "{:?}", program.diagnostics);
}

#[test]
fn builtin_struct_cannot_be_bound_as_runtime_value() {
    let root = temp_dir("builtin_struct_cannot_be_bound_as_runtime_value");
    write(
        &root.join("main.nia"),
        r#"
fn main() i32 {
    let builtin = @builtin();
    0
}
"#,
    );

    let program = check_program(root.join("main.nia").to_string_lossy().into_owned());
    assert!(
        program.diagnostics.iter().any(|diagnostic| diagnostic
            .diagnostic
            .summary
            .contains("comptime-only value")),
        "{:?}",
        program.diagnostics
    );
}

#[test]
fn comptime_function_rejects_builtin_string_field_assignment_type_mismatch() {
    let root = temp_dir("comptime_function_rejects_builtin_string_field_assignment_type_mismatch");
    write(
        &root.join("main.nia"),
        r#"
comptime fn width() usize {
    comptime var builtin = @builtin();
    builtin.target.os = true;
    builtin.target.pointer_width
}

comptime let n: usize = width();
"#,
    );

    let program = check_program(root.join("main.nia").to_string_lossy().into_owned());
    assert!(
        program.diagnostics.iter().any(|diagnostic| diagnostic
            .diagnostic
            .summary
            .contains("expected string type")),
        "{:?}",
        program.diagnostics
    );
}

#[test]
fn comptime_function_rejects_structural_bool_field_assignment_type_mismatch() {
    let root = temp_dir("comptime_function_rejects_structural_bool_field_assignment_type_mismatch");
    write(
        &root.join("main.nia"),
        r#"
comptime fn width() usize {
    comptime var config = {enabled: true};
    config.enabled = 1usize;
    1usize
}

comptime let n: usize = width();
"#,
    );

    let program = check_program(root.join("main.nia").to_string_lossy().into_owned());
    assert!(
        program.diagnostics.iter().any(|diagnostic| diagnostic
            .diagnostic
            .summary
            .contains("primitive type bool")),
        "{:?}",
        program.diagnostics
    );
}

#[test]
fn comptime_if_rejects_old_target_predicate_builtins() {
    let root = temp_dir("comptime_if_rejects_old_target_predicate_builtins");
    write(
        &root.join("main.nia"),
        r#"
fn main() i32 {
    comptime if @target_os("linux") {
        1
    } else {
        0
    }
}
"#,
    );

    let program = check_program(root.join("main.nia").to_string_lossy().into_owned());
    assert!(
        program.diagnostics.iter().any(|diagnostic| diagnostic
            .diagnostic
            .summary
            .contains("unsupported builtin call in comptime expression")),
        "{:?}",
        program.diagnostics
    );
}

#[test]
fn imported_comptime_values_drive_array_lengths() {
    let root = temp_dir("imported_comptime_values_drive_array_lengths");
    write(
        &root.join("main.nia"),
        r#"
import .config;

fn main() i32 {
    var values: [config::width]i32 = [1, 2, 3, 4];
    values[config::width - 1]
}
"#,
    );
    write(
        &root.join("config.nia"),
        r#"
pub comptime let width: usize = 4;
"#,
    );

    let program = check_program(root.join("main.nia").to_string_lossy().into_owned());
    assert!(program.diagnostics.is_empty(), "{:?}", program.diagnostics);
}

#[test]
fn layout_builtins_are_comptime_values_for_concrete_types() {
    let root = temp_dir("layout_builtins_are_comptime_values_for_concrete_types");
    write(
        &root.join("main.nia"),
        r#"
struct Pair {
    a: u8,
    b: i32,
}

comptime let pair_size: usize = @size[Pair]();
comptime let pair_align: usize = @align[Pair]();

fn main() i32 {
    var bytes: [pair_size]u8 = [0; pair_size];
    bytes.len() as i32 + pair_align as i32
}
"#,
    );

    let program = check_program(root.join("main.nia").to_string_lossy().into_owned());
    assert!(program.diagnostics.is_empty(), "{:?}", program.diagnostics);
}

#[test]
fn generic_comptime_function_explicit_type_args_drive_array_lengths() {
    let root = temp_dir("generic_comptime_function_explicit_type_args_drive_array_lengths");
    write(
        &root.join("main.nia"),
        r#"
comptime fn id[T](value: T) T {
    value
}

comptime let n: usize = id[usize](4usize);

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
fn generic_comptime_function_infers_type_arg_from_suffixed_literal() {
    let root = temp_dir("generic_comptime_function_infers_type_arg_from_suffixed_literal");
    write(
        &root.join("main.nia"),
        r#"
comptime fn id[T](value: T) T {
    value
}

comptime let n: usize = id(4usize);

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
fn generic_comptime_function_infers_type_arg_from_integer_binary_expr() {
    let root = temp_dir("generic_comptime_function_infers_type_arg_from_integer_binary_expr");
    write(
        &root.join("main.nia"),
        r#"
comptime fn id[T](value: T) T {
    value
}

comptime let n: usize = id(4usize + 3usize);

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
fn generic_comptime_function_infers_type_arg_from_bool_binary_expr() {
    let root = temp_dir("generic_comptime_function_infers_type_arg_from_bool_binary_expr");
    write(
        &root.join("main.nia"),
        r#"
comptime fn id[T](value: T) T {
    value
}

comptime fn choose(value: bool) usize {
    if value {
        5usize
    } else {
        1usize
    }
}

comptime let n: usize = choose(id((4usize + 3usize) == 7usize));

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
fn generic_comptime_function_infers_type_arg_from_bool_literal_and_not() {
    let root = temp_dir("generic_comptime_function_infers_type_arg_from_bool_literal_and_not");
    write(
        &root.join("main.nia"),
        r#"
comptime fn id[T](value: T) T {
    value
}

comptime fn choose(value: bool) usize {
    if value {
        6usize
    } else {
        1usize
    }
}

comptime let a: bool = id(true);
comptime let b: bool = id(not false);
comptime let n: usize = choose(a and b);

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
fn generic_comptime_function_infers_type_arg_from_integer_negation() {
    let root = temp_dir("generic_comptime_function_infers_type_arg_from_integer_negation");
    write(
        &root.join("main.nia"),
        r#"
comptime fn id[T](value: T) T {
    value
}

comptime fn magnitude(value: i32) usize {
    if value == -4i32 {
        4usize
    } else {
        1usize
    }
}

comptime let n: usize = magnitude(id(-4i32));

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
fn generic_comptime_function_infers_type_arg_from_if_expression() {
    let root = temp_dir("generic_comptime_function_infers_type_arg_from_if_expression");
    write(
        &root.join("main.nia"),
        r#"
comptime fn id[T](value: T) T {
    value
}

comptime let n: usize = id(if true {
    4usize
} else {
    1usize
});

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
fn generic_comptime_function_infers_type_arg_from_contextual_if_expression() {
    let root = temp_dir("generic_comptime_function_infers_type_arg_from_contextual_if_expression");
    write(
        &root.join("main.nia"),
        r#"
comptime fn id[T](value: ?T) ?T {
    value
}

comptime fn unwrap(value: ?usize) usize {
    switch value {
        ?payload => payload,
        null => 1usize,
    }
}

comptime let n: usize = unwrap(id(if true {
    ?4usize
} else {
    null
}));

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
fn generic_comptime_function_infers_type_arg_from_if_block_tail_local() {
    let root = temp_dir("generic_comptime_function_infers_type_arg_from_if_block_tail_local");
    write(
        &root.join("main.nia"),
        r#"
comptime fn id[T](value: T) T {
    value
}

comptime let n: usize = id(if true {
    comptime let value: usize = 4usize;
    value
} else {
    comptime let value: usize = 1usize;
    value
});

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
fn generic_comptime_function_infers_type_arg_from_switch_expression() {
    let root = temp_dir("generic_comptime_function_infers_type_arg_from_switch_expression");
    write(
        &root.join("main.nia"),
        r#"
comptime fn id[T](value: T) T {
    value
}

comptime let n: usize = id(switch 2usize {
    1usize => 4usize,
    2usize => 8usize,
    _ => 1usize,
});

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
fn generic_comptime_function_rejects_mismatched_switch_pattern() {
    let root = temp_dir("generic_comptime_function_rejects_mismatched_switch_pattern");
    write(
        &root.join("main.nia"),
        r#"
comptime fn id[T](value: T) T {
    value
}

comptime let n: usize = id(switch 2usize {
    true => 4usize,
    _ => 1usize,
});
"#,
    );

    let program = check_program(root.join("main.nia").to_string_lossy().into_owned());
    assert!(
        program
            .diagnostics
            .iter()
            .any(|diagnostic| diagnostic.diagnostic.summary.contains("switch")),
        "{:?}",
        program.diagnostics
    );
}

#[test]
fn generic_comptime_function_rejects_non_integer_switch_range_pattern() {
    let root = temp_dir("generic_comptime_function_rejects_non_integer_switch_range_pattern");
    write(
        &root.join("main.nia"),
        r#"
comptime fn id[T](value: T) T {
    value
}

comptime let n: usize = id(switch true {
    0usize..2usize => 4usize,
    _ => 1usize,
});
"#,
    );

    let program = check_program(root.join("main.nia").to_string_lossy().into_owned());
    assert!(
        program
            .diagnostics
            .iter()
            .any(|diagnostic| diagnostic.diagnostic.summary.contains("switch")),
        "{:?}",
        program.diagnostics
    );
}

#[test]
fn generic_comptime_function_infers_type_arg_from_switch_optional_payload() {
    let root = temp_dir("generic_comptime_function_infers_type_arg_from_switch_optional_payload");
    write(
        &root.join("main.nia"),
        r#"
comptime fn id[T](value: T) T {
    value
}

comptime let value: ?usize = ?8usize;
comptime let n: usize = switch value {
    ?payload => id(payload),
    null => 1usize,
};

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
fn generic_comptime_function_infers_type_arg_from_switch_error_payloads() {
    let root = temp_dir("generic_comptime_function_infers_type_arg_from_switch_error_payloads");
    write(
        &root.join("main.nia"),
        r#"
comptime fn id[T](value: T) T {
    value
}

comptime let value: usize!usize = !8usize;
comptime let n: usize = switch value {
    !payload => id(payload),
    err! => id(err),
};

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
fn generic_comptime_function_infers_type_arg_from_array_literal() {
    let root = temp_dir("generic_comptime_function_infers_type_arg_from_array_literal");
    write(
        &root.join("main.nia"),
        r#"
comptime fn second[T](values: [2]T) T {
    values[1]
}

comptime let n: usize = second([4usize, 8usize]);

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
fn generic_comptime_function_infers_type_arg_from_array_repeat_literal() {
    let root = temp_dir("generic_comptime_function_infers_type_arg_from_array_repeat_literal");
    write(
        &root.join("main.nia"),
        r#"
comptime fn first[T](values: [2]T) T {
    values[0]
}

comptime let n: usize = first([8usize; 2]);

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
fn generic_comptime_function_rejects_non_integer_array_repeat_count() {
    let root = temp_dir("generic_comptime_function_rejects_non_integer_array_repeat_count");
    write(
        &root.join("main.nia"),
        r#"
comptime fn first[T](values: T) usize {
    values.len()
}

comptime let n: usize = first([8usize; true]);
"#,
    );

    let program = check_program(root.join("main.nia").to_string_lossy().into_owned());
    assert!(
        program
            .diagnostics
            .iter()
            .any(|diagnostic| diagnostic.diagnostic.summary.contains("array repeat count")),
        "{:?}",
        program.diagnostics
    );
}

#[test]
fn generic_comptime_function_infers_type_arg_from_contextual_array_literal() {
    let root = temp_dir("generic_comptime_function_infers_type_arg_from_contextual_array_literal");
    write(
        &root.join("main.nia"),
        r#"
comptime fn first_some[T](values: [2]?T) T {
    switch values[0] {
        ?payload => payload,
        null => values[1].?,
    }
}

comptime let n: usize = first_some([null, ?8usize]);

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
fn generic_comptime_function_infers_type_arg_from_struct_literal() {
    let root = temp_dir("generic_comptime_function_infers_type_arg_from_struct_literal");
    write(
        &root.join("main.nia"),
        r#"
struct Pair[T] {
    left: T,
    right: T,
}

comptime fn right[T](pair: Pair[T]) T {
    pair.right
}

comptime let n: usize = right({left: 4usize, right: 8usize});

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
fn generic_comptime_function_infers_type_arg_from_contextual_struct_literal() {
    let root = temp_dir("generic_comptime_function_infers_type_arg_from_contextual_struct_literal");
    write(
        &root.join("main.nia"),
        r#"
struct Slot[T] {
    primary: ?T,
    fallback: ?T,
}

comptime fn pick[T](slot: Slot[T]) T {
    switch slot.primary {
        ?payload => payload,
        null => slot.fallback.?,
    }
}

comptime let n: usize = pick({primary: null, fallback: ?8usize});

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
fn generic_comptime_function_infers_type_arg_from_struct_field() {
    let root = temp_dir("generic_comptime_function_infers_type_arg_from_struct_field");
    write(
        &root.join("main.nia"),
        r#"
struct Point {
    x: usize,
    y: usize,
}

comptime fn id[T](value: T) T {
    value
}

comptime let point: Point = Point{x: 4, y: 8};
comptime let n: usize = id(point.y);

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
fn imported_comptime_struct_field_has_runtime_type() {
    let root = temp_dir("imported_comptime_struct_field_has_runtime_type");
    write(
        &root.join("config.nia"),
        r#"
pub struct Point {
    x: usize,
    y: usize,
}

pub comptime let point: Point = Point{x: 4, y: 8};
"#,
    );
    write(
        &root.join("main.nia"),
        r#"
import .config;

comptime fn id[T](value: T) T {
    value
}

comptime let n: usize = id(config::point.x);

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
fn generic_comptime_function_substitutes_struct_field_types_in_body() {
    let root = temp_dir("generic_comptime_function_substitutes_struct_field_types_in_body");
    write(
        &root.join("main.nia"),
        r#"
struct Pair[T] {
    left: T,
    right: T,
}

comptime fn id[T](value: T) T {
    value
}

comptime fn right_id[T](pair: Pair[T]) T {
    id(pair.right)
}

comptime let n: usize = right_id({left: 4usize, right: 8usize});

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
fn generic_comptime_function_infers_type_arg_from_typed_comptime_value() {
    let root = temp_dir("generic_comptime_function_infers_type_arg_from_typed_comptime_value");
    write(
        &root.join("main.nia"),
        r#"
comptime fn id[T](value: T) T {
    value
}

comptime let width: usize = 4;
comptime let n: usize = id(width);

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
fn generic_comptime_function_infers_type_arg_from_inferred_comptime_value() {
    let root = temp_dir("generic_comptime_function_infers_type_arg_from_inferred_comptime_value");
    write(
        &root.join("main.nia"),
        r#"
comptime fn id[T](value: T) T {
    value
}

comptime let width = 4usize;
comptime let n: usize = id(width);

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
fn generic_comptime_function_infers_type_arg_from_local_inferred_comptime_value() {
    let root =
        temp_dir("generic_comptime_function_infers_type_arg_from_local_inferred_comptime_value");
    write(
        &root.join("main.nia"),
        r#"
comptime fn id[T](value: T) T {
    value
}

fn main() i32 {
    comptime let width = 4usize;
    comptime let n: usize = id(width);
    var values: [n]i32 = [0; n];
    values.len() as i32
}
"#,
    );

    let program = check_program(root.join("main.nia").to_string_lossy().into_owned());
    assert!(program.diagnostics.is_empty(), "{:?}", program.diagnostics);
}

#[test]
fn function_body_comptime_call_infers_generic_from_local_value() {
    let root = temp_dir("function_body_comptime_call_infers_generic_from_local_value");
    write(
        &root.join("main.nia"),
        r#"
comptime fn id[T](value: T) T {
    value
}

fn main() i32 {
    comptime let width = 4usize;
    comptime let n: usize = id(width);
    var values: [n]i32 = [0; n];
    values.len() as i32
}
"#,
    );

    let program = check_program(root.join("main.nia").to_string_lossy().into_owned());
    assert!(program.diagnostics.is_empty(), "{:?}", program.diagnostics);
}

#[test]
fn function_body_comptime_call_infers_generic_from_structural_field() {
    let root = temp_dir("function_body_comptime_call_infers_generic_from_structural_field");
    write(
        &root.join("main.nia"),
        r#"
comptime fn id[T](value: T) T {
    value
}

fn main() i32 {
    comptime let config = {target: {word_bits: 64usize}};
    comptime let n: usize = id(config.target.word_bits) / 8usize;
    var values: [n]i32 = [0; n];
    values.len() as i32
}
"#,
    );

    let program = check_program(root.join("main.nia").to_string_lossy().into_owned());
    assert!(program.diagnostics.is_empty(), "{:?}", program.diagnostics);
}

#[test]
fn function_body_imported_comptime_call_infers_generic_from_local_value() {
    let root = temp_dir("function_body_imported_comptime_call_infers_generic_from_local_value");
    write(
        &root.join("helpers.nia"),
        r#"
pub comptime fn id[T](value: T) T {
    value
}
"#,
    );
    write(
        &root.join("main.nia"),
        r#"
import .helpers;

fn main() i32 {
    comptime let width = 4usize;
    comptime let n: usize = helpers::id(width);
    var values: [n]i32 = [0; n];
    values.len() as i32
}
"#,
    );

    let program = check_program(root.join("main.nia").to_string_lossy().into_owned());
    assert!(program.diagnostics.is_empty(), "{:?}", program.diagnostics);
}

#[test]
fn imported_comptime_function_body_infers_generic_from_structural_local() {
    let root = temp_dir("imported_comptime_function_body_infers_generic_from_structural_local");
    write(
        &root.join("helpers.nia"),
        r#"
comptime fn id[T](value: T) T {
    value
}

pub comptime fn word_bytes() usize {
    comptime let configs = [{bits: 32usize}, {bits: 64usize}];
    id(configs[1].bits) / 8usize
}
"#,
    );
    write(
        &root.join("main.nia"),
        r#"
import .helpers;

fn main() i32 {
    comptime let n: usize = helpers::word_bytes();
    var values: [n]i32 = [0; n];
    values.len() as i32
}
"#,
    );

    let program = check_program(root.join("main.nia").to_string_lossy().into_owned());
    assert!(program.diagnostics.is_empty(), "{:?}", program.diagnostics);
}

#[test]
fn imported_inferred_comptime_value_has_runtime_type() {
    let root = temp_dir("imported_inferred_comptime_value_has_runtime_type");
    write(
        &root.join("config.nia"),
        r#"
pub comptime let width = 4usize;
"#,
    );
    write(
        &root.join("main.nia"),
        r#"
import .config;

comptime fn id[T](value: T) T {
    value
}

comptime let n: usize = id(config::width);

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
fn structural_comptime_struct_fields_have_typed_values() {
    let root = temp_dir("structural_comptime_struct_fields_have_typed_values");
    write(
        &root.join("main.nia"),
        r#"
comptime fn id[T](value: T) T {
    value
}

comptime let config = {width: 4usize, enabled: true};
comptime let n: usize = id(config.width);

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
fn nested_structural_comptime_struct_fields_have_typed_values() {
    let root = temp_dir("nested_structural_comptime_struct_fields_have_typed_values");
    write(
        &root.join("main.nia"),
        r#"
comptime fn id[T](value: T) T {
    value
}

comptime let config = {target: {word_bits: 64usize}};
comptime let n: usize = id(config.target.word_bits / 8usize);

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
fn imported_structural_comptime_struct_fields_have_typed_values() {
    let root = temp_dir("imported_structural_comptime_struct_fields_have_typed_values");
    write(
        &root.join("config.nia"),
        r#"
pub comptime let config = {width: 4usize};
"#,
    );
    write(
        &root.join("main.nia"),
        r#"
import .config;

comptime fn id[T](value: T) T {
    value
}

comptime let n: usize = id(config::config.width);

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
fn comptime_function_local_structural_struct_fields_have_typed_values() {
    let root = temp_dir("comptime_function_local_structural_struct_fields_have_typed_values");
    write(
        &root.join("main.nia"),
        r#"
comptime fn id[T](value: T) T {
    value
}

comptime fn width() usize {
    comptime let config = {target: {word_bits: 64usize}};
    id(config.target.word_bits) / 8usize
}

comptime let n: usize = width();

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
fn comptime_if_structural_struct_fields_have_typed_values() {
    let root = temp_dir("comptime_if_structural_struct_fields_have_typed_values");
    write(
        &root.join("main.nia"),
        r#"
comptime fn id[T](value: T) T {
    value
}

comptime let config = if true {
    {width: 4usize}
} else {
    {width: 8usize}
};
comptime let n: usize = id(config.width);

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
fn comptime_if_expr_is_an_ordinary_comptime_value() {
    let root = temp_dir("comptime_if_expr_is_an_ordinary_comptime_value");
    write(
        &root.join("main.nia"),
        r#"
comptime fn id[T](value: T) T {
    value
}

comptime let config = comptime if true {
    {width: 4usize}
} else {
    {width: 8usize}
};
comptime let n: usize = id(config.width);

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
fn comptime_if_expr_rejects_non_bool_condition() {
    let root = temp_dir("comptime_if_expr_rejects_non_bool_condition");
    write(
        &root.join("main.nia"),
        r#"
comptime fn id[T](value: T) T {
    value
}

comptime let n: usize = id(comptime if 1usize {
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
fn comptime_fn_returns_comptime_if_expr_value() {
    let root = temp_dir("comptime_fn_returns_comptime_if_expr_value");
    write(
        &root.join("main.nia"),
        r#"
comptime fn select() usize {
    comptime if true {
        4usize
    } else {
        8usize
    }
}

comptime let n: usize = select();

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

comptime let ch: char = choose_char('A');
comptime let n: usize = widen_byte(b'\n') + 1usize;

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
fn comptime_bitwise_not_evaluates_integer_values() {
    let root = temp_dir("comptime_bitwise_not_evaluates_integer_values");
    write(
        &root.join("main.nia"),
        r#"
comptime fn mask(value: usize) usize {
    ~value & 15usize
}

comptime let n: usize = mask(10usize);

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
fn comptime_casts_have_typed_values() {
    let root = temp_dir("comptime_casts_have_typed_values");
    write(
        &root.join("main.nia"),
        r#"
comptime fn id[T](value: T) T {
    value
}

comptime let n: usize = id(b'a' as usize);

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

comptime let narrow_value: usize = narrow(258usize);
comptime let signed_value: i32 = signed(255usize);

var narrow_global: i32 = narrow_value as i32;
var signed_global: i32 = signed_value;
"#,
    );

    let program = check_program(root.join("main.nia").to_string_lossy().into_owned());
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
    assert_eq!(narrow_global.init, Some(StaticInit::Int(2)));
    let signed_global = main_module
        .globals
        .iter()
        .find(|global| global.name == "signed_global")
        .expect("signed_global");
    assert_eq!(signed_global.init, Some(StaticInit::Int(-1)));
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

comptime let scaled: f64 = scale(3.25f64);
comptime let from_int: f64 = wide(4usize);
comptime let n: usize = if scaled > from_int {
    scaled as usize
} else {
    0usize
};

var value: i32 = n as i32;
"#,
    );

    let program = check_program(root.join("main.nia").to_string_lossy().into_owned());
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
    assert_eq!(value.init, Some(StaticInit::Int(7)));
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

comptime let n: usize = id(true as usize);
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

comptime let n: usize = id(({width: 4usize}) as usize);
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
    comptime var value: f64 = 1.5f64;
    value += 2.5f64;
    value *= 2.0f64;
    value as usize
}

comptime let n: usize = width();
var value: i32 = n as i32;
"#,
    );

    let program = check_program(root.join("main.nia").to_string_lossy().into_owned());
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
    assert_eq!(value.init, Some(StaticInit::Int(8)));
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

comptime let value: f64 = id(-1.5f64);
comptime let n: usize = if value < 0.0f64 { 4usize } else { 0usize };

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
fn generic_comptime_function_rejects_mismatched_equality_operands() {
    let root = temp_dir("generic_comptime_function_rejects_mismatched_equality_operands");
    write(
        &root.join("main.nia"),
        r#"
comptime fn id[T](value: T) T {
    value
}

comptime let value: bool = id(1usize == true);
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

comptime let value: bool = id(true and 1usize);
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

comptime let value: bool = id(not 1usize);
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
comptime let literal: f32 = 1e40f32;
comptime let casted: f32 = 1e40f64 as f32;
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
comptime let bytes: [2]u8 = [1u16, 300u16];
comptime let config = {values: [1u16, 300u16]};
comptime let selected: u8 = config.values[1];
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

comptime let packet: Packet[u8] = Packet[u8]{tag: 1u16, payload: 300u16};
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

pub comptime let packet: Packet[u8] = Packet[u8]{tag: 1u16, payload: 300u16};
"#,
    );
    write(
        &root.join("main.nia"),
        r#"
import .config;

comptime let selected: u8 = config::packet.payload;
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

comptime let text: [4]char = id("nia!");
comptime let n: usize = accept4(text);

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
fn comptime_array_len_method_evaluates_array_values() {
    let root = temp_dir("comptime_array_len_method_evaluates_array_values");
    write(
        &root.join("main.nia"),
        r#"
comptime fn total(values: [3]usize, text: [4]char) usize {
    values.len() + text.len() + values[1..].len()
}

comptime let n: usize = total([1usize, 2usize, 3usize], "nia!");

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
fn comptime_target_strings_compare_with_string_literals() {
    let root = temp_dir("comptime_target_strings_compare_with_string_literals");
    write(
        &root.join("main.nia"),
        r#"
comptime fn is_target_os(value: [5]char) bool {
    @builtin().target.os == value
}

comptime let n: usize = if is_target_os("linux") { 4usize } else { 2usize };

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
fn comptime_target_strings_pass_as_char_arrays() {
    let root = temp_dir("comptime_target_strings_pass_as_char_arrays");
    write(
        &root.join("main.nia"),
        r#"
comptime fn accept_linux(value: [5]char) usize {
    if value.len() == 5usize and value[0] == 'l' and value == "linux" {
        5usize
    } else {
        0usize
    }
}

comptime let n: usize = accept_linux(@builtin().target.os);

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
fn comptime_target_strings_validate_char_array_lengths() {
    let root = temp_dir("comptime_target_strings_validate_char_array_lengths");
    write(
        &root.join("main.nia"),
        r#"
comptime fn accept_short(value: [4]char) usize {
    value.len()
}

comptime let n: usize = accept_short(@builtin().target.os);
"#,
    );

    let program = check_program(root.join("main.nia").to_string_lossy().into_owned());
    assert!(
        program
            .diagnostics
            .iter()
            .any(|diagnostic| diagnostic.diagnostic.summary.contains("expected length 4")),
        "{:?}",
        program.diagnostics
    );
}

#[test]
fn typed_comptime_target_string_binding_is_a_char_array() {
    let root = temp_dir("typed_comptime_target_string_binding_is_a_char_array");
    write(
        &root.join("main.nia"),
        r#"
comptime let os: [5]char = @builtin().target.os;
comptime let n: usize = if os.len() == 5usize {
    os.len()
} else {
    0usize
};

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
fn imported_typed_comptime_target_string_binding_is_a_char_array() {
    let root = temp_dir("imported_typed_comptime_target_string_binding_is_a_char_array");
    write(
        &root.join("config.nia"),
        r#"
pub comptime let os: [5]char = @builtin().target.os;
"#,
    );
    write(
        &root.join("main.nia"),
        r#"
import .config;

comptime let n: usize = config::os.len();

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
fn comptime_function_returned_target_string_is_a_char_array() {
    let root = temp_dir("comptime_function_returned_target_string_is_a_char_array");
    write(
        &root.join("main.nia"),
        r#"
comptime fn target_os() [5]char {
    @builtin().target.os
}

comptime let os: [5]char = target_os();
comptime let n: usize = os.len();

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
fn comptime_function_returned_target_string_can_be_consumed_directly() {
    let root = temp_dir("comptime_function_returned_target_string_can_be_consumed_directly");
    write(
        &root.join("main.nia"),
        r#"
comptime fn target_os() [5]char {
    @builtin().target.os
}

comptime let n: usize = target_os().len();

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
fn comptime_function_returned_target_string_validates_char_array_length() {
    let root = temp_dir("comptime_function_returned_target_string_validates_char_array_length");
    write(
        &root.join("main.nia"),
        r#"
comptime fn target_os() [4]char {
    @builtin().target.os
}

comptime let os = target_os();
"#,
    );

    let program = check_program(root.join("main.nia").to_string_lossy().into_owned());
    assert!(
        program
            .diagnostics
            .iter()
            .any(|diagnostic| diagnostic.diagnostic.summary.contains("expected length 4")),
        "{:?}",
        program.diagnostics
    );
}

#[test]
fn comptime_for_in_array_binding_requires_iterator() {
    let root = temp_dir("comptime_for_in_array_binding_requires_iterator");
    write(
        &root.join("main.nia"),
        r#"
comptime fn total(values: [1][5]char) usize {
    var n: usize = 0usize;
    for os in values {
        n += os.len();
    }
    n
}

comptime let n: usize = total([@builtin().target.os]);

fn main() i32 {
    var values: [n]i32 = [0; n];
    values.len() as i32
}
"#,
    );

    let program = check_program(root.join("main.nia").to_string_lossy().into_owned());
    assert!(
        program.diagnostics.iter().any(|diagnostic| diagnostic
            .diagnostic
            .summary
            .contains("comptime for-in expects an Iterator")),
        "{:?}",
        program.diagnostics
    );
}

#[test]
fn comptime_switch_optional_payload_preserves_array_type() {
    let root = temp_dir("comptime_switch_optional_payload_preserves_array_type");
    write(
        &root.join("main.nia"),
        r#"
comptime fn target_os() ?[5]char {
    ?@builtin().target.os
}

comptime let n: usize = switch target_os() {
    ?os => os.len(),
    null => 0usize,
};

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
fn comptime_optional_constructor_projects_target_string_payload() {
    let root = temp_dir("comptime_optional_constructor_projects_target_string_payload");
    write(
        &root.join("main.nia"),
        r#"
comptime let os: ?[5]char = ?@builtin().target.os;
comptime let n: usize = switch os {
    ?payload => payload.len(),
    null => 0usize,
};

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
fn imported_comptime_function_return_validates_imported_array_length() {
    let root = temp_dir("imported_comptime_function_return_validates_imported_array_length");
    write(
        &root.join("config.nia"),
        r#"
pub comptime let os_len: usize = 5usize;

pub comptime fn target_os() [os_len]char {
    @builtin().target.os
}
"#,
    );
    write(
        &root.join("main.nia"),
        r#"
import .config;

comptime let n: usize = config::target_os().len();

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
fn imported_comptime_function_return_rejects_imported_array_length_mismatch() {
    let root = temp_dir("imported_comptime_function_return_rejects_imported_array_length_mismatch");
    write(
        &root.join("config.nia"),
        r#"
pub comptime let os_len: usize = 4usize;

pub comptime fn target_os() [os_len]char {
    @builtin().target.os
}
"#,
    );
    write(
        &root.join("main.nia"),
        r#"
import .config;

comptime let os = config::target_os();
"#,
    );

    let program = check_program(root.join("main.nia").to_string_lossy().into_owned());
    assert!(
        program
            .diagnostics
            .iter()
            .any(|diagnostic| diagnostic.diagnostic.summary.contains("expected length 4")),
        "{:?}",
        program.diagnostics
    );
}

#[test]
fn comptime_byte_and_c_string_literals_are_typed_arrays() {
    let root = temp_dir("comptime_byte_and_c_string_literals_are_typed_arrays");
    write(
        &root.join("main.nia"),
        r#"
comptime fn byte_score(value: [3]u8) usize {
    if value[0] == b'n' {
        value[2] as usize
    } else {
        0usize
    }
}

comptime fn c_score(value: [4]u8) usize {
    if value[3] == 0u8 {
        value[0] as usize
    } else {
        0usize
    }
}

comptime let n: usize = byte_score(b"nia") + c_score(c"nia");

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
fn comptime_string_literals_support_source_concatenation_and_multiline() {
    let root = temp_dir("comptime_string_literals_support_source_concatenation_and_multiline");
    write(
        &root.join("main.nia"),
        r#"
comptime fn char_score(value: [3]char) usize {
    if value[1] == '\x69' {
        3usize
    } else {
        0usize
    }
}

comptime fn byte_score(value: [3]u8) usize {
    if value[2] == b'a' {
        5usize
    } else {
        0usize
    }
}

comptime fn multiline_score(value: [11]char) usize {
    if value[5] == '\n' {
        7usize
    } else {
        0usize
    }
}

comptime let text: [3]char = "n" "ia";
comptime let bytes: [3]u8 = b"n" b"ia";
comptime let multiline: [11]char =
    \\hello
    \\world
;
comptime let byte_multiline: [11]u8 =
    b\\hello
    \\world
;
comptime let n: usize =
    char_score(text)
    + byte_score(bytes)
    + multiline_score(multiline)
    + byte_score(byte_multiline[8..]);

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
fn comptime_switch_structural_struct_fields_have_typed_values() {
    let root = temp_dir("comptime_switch_structural_struct_fields_have_typed_values");
    write(
        &root.join("main.nia"),
        r#"
comptime fn id[T](value: T) T {
    value
}

comptime let config = switch 1usize {
    1usize => ({width: 4usize}),
    _ => ({width: 8usize}),
};
comptime let n: usize = id(config.width);

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
fn structural_comptime_array_elements_have_typed_values() {
    let root = temp_dir("structural_comptime_array_elements_have_typed_values");
    write(
        &root.join("main.nia"),
        r#"
comptime fn id[T](value: T) T {
    value
}

comptime let configs = [{width: 4usize}, {width: 8usize}];
comptime let n: usize = id(configs[1].width);

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
fn structural_comptime_array_slices_have_typed_values() {
    let root = temp_dir("structural_comptime_array_slices_have_typed_values");
    write(
        &root.join("main.nia"),
        r#"
comptime fn id[T](value: T) T {
    value
}

comptime let configs = [{width: 4usize}, {width: 8usize}, {width: 16usize}];
comptime let selected = configs[1..=2];
comptime let n: usize = id(selected[1].width);

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
fn structural_comptime_array_repeat_elements_have_typed_values() {
    let root = temp_dir("structural_comptime_array_repeat_elements_have_typed_values");
    write(
        &root.join("main.nia"),
        r#"
comptime fn id[T](value: T) T {
    value
}

comptime let configs = [{width: 4usize}; 2usize];
comptime let n: usize = id(configs[1].width);

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
fn generic_comptime_function_infers_type_arg_from_nested_call_return_type() {
    let root = temp_dir("generic_comptime_function_infers_type_arg_from_nested_call_return_type");
    write(
        &root.join("main.nia"),
        r#"
comptime fn id[T](value: T) T {
    value
}

comptime fn array_len[T](value: T) usize {
    7usize
}

comptime let n: usize = array_len(id(4usize));

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
fn generic_comptime_function_infers_type_arg_from_nested_optional_call_return_type() {
    let root =
        temp_dir("generic_comptime_function_infers_type_arg_from_nested_optional_call_return_type");
    write(
        &root.join("main.nia"),
        r#"
comptime fn id[T](value: T) T {
    value
}

comptime fn some[T](value: T) ?T {
    ?value
}

comptime fn unwrap(value: ?usize) usize {
    switch value {
        ?payload => payload,
        null => 0usize,
    }
}

comptime let n: usize = unwrap(id(some(7usize)));

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
fn generic_comptime_function_infers_type_arg_from_try_payload_return_type() {
    let root = temp_dir("generic_comptime_function_infers_type_arg_from_try_payload_return_type");
    write(
        &root.join("main.nia"),
        r#"
comptime fn id[T](value: T) T {
    value
}

comptime fn use_try(value: ?usize) usize {
    id(value.?)
}

comptime let n: usize = use_try(?7usize);

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
fn generic_comptime_function_infers_type_arg_from_error_try_payload_return_type() {
    let root =
        temp_dir("generic_comptime_function_infers_type_arg_from_error_try_payload_return_type");
    write(
        &root.join("main.nia"),
        r#"
comptime fn id[T](value: T) T {
    value
}

comptime fn use_try(value: usize!usize) usize!usize {
    !id(value.?)
}

comptime let got: usize!usize = use_try(!7usize);
comptime let n: usize = switch got {
    !payload => payload,
    err! => err,
};

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
fn generic_comptime_function_infers_type_arg_from_builtin_target_field() {
    let root = temp_dir("generic_comptime_function_infers_type_arg_from_builtin_target_field");
    write(
        &root.join("main.nia"),
        r#"
comptime fn id[T](value: T) T {
    value
}

comptime let bits: usize = id(@builtin().target.pointer_width);
comptime let n: usize = bits / 8usize;

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
fn generic_comptime_function_infers_type_arg_from_bound_builtin_struct() {
    let root = temp_dir("generic_comptime_function_infers_type_arg_from_bound_builtin_struct");
    write(
        &root.join("main.nia"),
        r#"
comptime fn id[T](value: T) T {
    value
}

comptime let builtin = @builtin();
comptime let bits: usize = id(builtin.target.pointer_width);
comptime let n: usize = bits / 8usize;

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
fn generic_comptime_function_infers_type_arg_from_optional_constructor() {
    let root = temp_dir("generic_comptime_function_infers_type_arg_from_optional_constructor");
    write(
        &root.join("main.nia"),
        r#"
comptime fn id[T](value: T) T {
    value
}

comptime fn unwrap(value: ?usize) usize {
    switch value {
        ?payload => payload,
        null => 0usize,
    }
}

comptime let n: usize = unwrap(id(?7usize));

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
fn generic_comptime_function_infers_type_arg_from_error_success_constructor_context() {
    let root = temp_dir(
        "generic_comptime_function_infers_type_arg_from_error_success_constructor_context",
    );
    write(
        &root.join("main.nia"),
        r#"
comptime fn id[T](value: usize!T) usize!T {
    value
}

comptime fn unwrap(value: usize!usize) usize {
    switch value {
        !payload => payload,
        err! => err,
    }
}

comptime let n: usize = unwrap(id(!7usize));

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
fn generic_comptime_function_infers_type_arg_from_error_payload_constructor_context() {
    let root = temp_dir(
        "generic_comptime_function_infers_type_arg_from_error_payload_constructor_context",
    );
    write(
        &root.join("main.nia"),
        r#"
comptime fn id[T](value: T!usize) T!usize {
    value
}

comptime fn unwrap(value: usize!usize) usize {
    switch value {
        !payload => payload,
        err! => err,
    }
}

comptime let n: usize = unwrap(id(3usize!));

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
fn generic_comptime_function_infers_type_arg_from_typed_aggregate_literals() {
    let root = temp_dir("generic_comptime_function_infers_type_arg_from_typed_aggregate_literals");
    write(
        &root.join("main.nia"),
        r#"
struct Config {
    widths: [3]usize,
}

comptime fn id[T](value: T) T {
    value
}

comptime let config: Config = id(Config{widths: [2, 4, 8]});
comptime let widths: [3]usize = id([3]usize[2, 4, 8]);
comptime let width: usize = config.widths[1] + widths[0];

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
fn imported_generic_comptime_function_infers_type_arg_from_typed_value() {
    let root = temp_dir("imported_generic_comptime_function_infers_type_arg_from_typed_value");
    write(
        &root.join("main.nia"),
        r#"
import .identity;

comptime let width: usize = 4;
comptime let n: usize = identity::id(width);

fn main() i32 {
    var values: [n]i32 = [0; n];
    values.len() as i32
}
"#,
    );
    write(
        &root.join("identity.nia"),
        r#"
pub comptime fn id[T](value: T) T {
    value
}
"#,
    );

    let program = check_program(root.join("main.nia").to_string_lossy().into_owned());
    assert!(program.diagnostics.is_empty(), "{:?}", program.diagnostics);
}

#[test]
fn generic_comptime_function_reports_uninferred_type_args() {
    let root = temp_dir("generic_comptime_function_reports_uninferred_type_args");
    write(
        &root.join("main.nia"),
        r#"
comptime fn zero[T]() usize {
    0
}

comptime let n: usize = zero();

fn main() i32 {
    var values: [n]i32 = [0; n];
    values.len() as i32
}
"#,
    );

    let program = check_program(root.join("main.nia").to_string_lossy().into_owned());
    assert!(
        program.diagnostics.iter().any(|diagnostic| diagnostic
            .diagnostic
            .summary
            .contains("cannot infer comptime generic type argument `T`")),
        "{:?}",
        program.diagnostics
    );
}

#[test]
fn generic_comptime_function_substitutes_type_args_for_layout_builtins() {
    let root = temp_dir("generic_comptime_function_substitutes_type_args_for_layout_builtins");
    write(
        &root.join("main.nia"),
        r#"
struct Pair {
    a: u8,
    b: i32,
}

comptime fn size_of[T]() usize {
    @size[T]()
}

comptime let n: usize = size_of[Pair]();

fn main() i32 {
    var bytes: [n]u8 = [0; n];
    bytes.len() as i32
}
"#,
    );

    let program = check_program(root.join("main.nia").to_string_lossy().into_owned());
    assert!(program.diagnostics.is_empty(), "{:?}", program.diagnostics);
}

#[test]
fn imported_generic_comptime_function_substitutes_type_args_for_layout_builtins() {
    let root =
        temp_dir("imported_generic_comptime_function_substitutes_type_args_for_layout_builtins");
    write(
        &root.join("main.nia"),
        r#"
import .layout;

struct Pair {
    a: u8,
    b: i32,
}

comptime let n: usize = layout::size_of[Pair]();

fn main() i32 {
    var bytes: [n]u8 = [0; n];
    bytes.len() as i32
}
"#,
    );
    write(
        &root.join("layout.nia"),
        r#"
pub comptime fn size_of[T]() usize {
    @size[T]()
}
"#,
    );

    let program = check_program(root.join("main.nia").to_string_lossy().into_owned());
    assert!(program.diagnostics.is_empty(), "{:?}", program.diagnostics);
}

#[test]
fn imported_comptime_value_evaluates_layout_builtin_in_defining_module() {
    let root = temp_dir("imported_comptime_value_evaluates_layout_builtin_in_defining_module");
    write(
        &root.join("config.nia"),
        r#"
pub struct Pair {
    a: u8,
    b: i32,
}

pub comptime let pair_size: usize = @size[Pair]();
"#,
    );
    write(
        &root.join("main.nia"),
        r#"
import .config;

comptime let n: usize = config::pair_size;

fn main() i32 {
    var bytes: [n]u8 = [0; n];
    bytes.len() as i32
}
"#,
    );

    let program = check_program(root.join("main.nia").to_string_lossy().into_owned());
    assert!(program.diagnostics.is_empty(), "{:?}", program.diagnostics);
}

#[test]
fn layout_builtin_uses_imported_comptime_array_lengths() {
    let root = temp_dir("layout_builtin_uses_imported_comptime_array_lengths");
    write(
        &root.join("config.nia"),
        r#"
pub comptime let N: usize = 4usize;

pub struct Packet {
    bytes: [N]u8,
}
"#,
    );
    write(
        &root.join("main.nia"),
        r#"
import .config;

comptime let n: usize = @size[config::Packet]();

fn main() i32 {
    var bytes: [n]u8 = [0; n];
    bytes.len() as i32
}
"#,
    );

    let program = check_program(root.join("main.nia").to_string_lossy().into_owned());
    assert!(program.diagnostics.is_empty(), "{:?}", program.diagnostics);
}

#[test]
fn function_body_comptime_call_substitutes_type_args_for_layout_builtins() {
    let root = temp_dir("function_body_comptime_call_substitutes_type_args_for_layout_builtins");
    write(
        &root.join("main.nia"),
        r#"
struct Pair {
    a: u8,
    b: i32,
}

comptime fn size_of[T]() usize {
    @size[T]()
}

fn main() i32 {
    comptime let n: usize = size_of[Pair]();
    var bytes: [n]u8 = [0; n];
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
pub comptime let N: usize = 4;

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
import .defs;
using defs::*;

fn make() Boxed {
    {
        items: [Item::zero(); 4],
    }
}

fn main() i32 {
    var x = make();
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
pub comptime let N: usize = 4;

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
import .defs;
using defs::*;

fn make() Boxed {
    {
        items: [Item::zero(); defs::N],
    }
}

fn main() i32 {
    var x = make();
    x.items[0].value as i32
}
"#,
    );

    let program = check_program(root.join("main.nia").to_string_lossy().into_owned());
    assert!(program.diagnostics.is_empty(), "{:?}", program.diagnostics);
}

#[test]
fn comptime_dependency_cycles_are_diagnosed() {
    let root = temp_dir("comptime_dependency_cycles_are_diagnosed");
    write(
        &root.join("main.nia"),
        r#"
comptime let a: i32 = b;
comptime let b: i32 = a;

fn main() i32 { 0 }
"#,
    );

    let program = check_program(root.join("main.nia").to_string_lossy().into_owned());
    assert!(
        program.diagnostics.iter().any(|diagnostic| diagnostic
            .diagnostic
            .summary
            .contains("cyclic comptime dependency")),
        "{:?}",
        program.diagnostics
    );
}

#[test]
fn comptime_rejects_runtime_local_dependency() {
    let root = temp_dir("comptime_rejects_runtime_local_dependency");
    write(
        &root.join("main.nia"),
        r#"
fn main() i32 {
    var runtime = 4;
    comptime let n: usize = runtime;
    var values: [n]i32 = [1, 2, 3, 4];
    values[0]
}
"#,
    );

    let program = check_program(root.join("main.nia").to_string_lossy().into_owned());
    assert!(
        program.diagnostics.iter().any(|diagnostic| diagnostic
            .diagnostic
            .summary
            .contains("comptime expression can only use comptime bindings")),
        "{:?}",
        program.diagnostics
    );
}

#[test]
fn comptime_error_builtin_reports_message() {
    let root = temp_dir("comptime_error_builtin_reports_message");
    write(
        &root.join("main.nia"),
        r#"
comptime let n: usize = @error("unsupported target");

fn main() i32 { 0 }
"#,
    );

    let program = check_program(root.join("main.nia").to_string_lossy().into_owned());
    assert!(
        program
            .diagnostics
            .iter()
            .any(|diagnostic| diagnostic.diagnostic.summary.contains("unsupported target")),
        "{:?}",
        program.diagnostics
    );
}

#[test]
fn comptime_error_builtin_is_pruned_by_comptime_if() {
    let root = temp_dir("comptime_error_builtin_is_pruned_by_comptime_if");
    write(
        &root.join("main.nia"),
        r#"
comptime let n: usize = comptime if true {
    4usize
} else {
    @error("unreachable comptime branch")
};

fn main() usize { n }
"#,
    );

    let program = check_program(root.join("main.nia").to_string_lossy().into_owned());
    assert!(program.diagnostics.is_empty(), "{:?}", program.diagnostics);
}

#[test]
fn comptime_error_builtin_requires_string_message() {
    let root = temp_dir("comptime_error_builtin_requires_string_message");
    write(
        &root.join("main.nia"),
        r#"
comptime let n: usize = @error(10usize);

fn main() i32 { 0 }
"#,
    );

    let program = check_program(root.join("main.nia").to_string_lossy().into_owned());
    assert!(
        program.diagnostics.iter().any(|diagnostic| diagnostic
            .diagnostic
            .summary
            .contains("requires a comptime string message")),
        "{:?}",
        program.diagnostics
    );
}

#[test]
fn error_builtin_is_not_runtime_panic() {
    let root = temp_dir("error_builtin_is_not_runtime_panic");
    write(
        &root.join("main.nia"),
        r#"
fn main() usize {
    @error("runtime panic")
}
"#,
    );

    let program = check_program(root.join("main.nia").to_string_lossy().into_owned());
    assert!(
        program.diagnostics.iter().any(|diagnostic| diagnostic
            .diagnostic
            .summary
            .contains("can only be evaluated at comptime")),
        "{:?}",
        program.diagnostics
    );
}

#[test]
fn trap_builtin_is_never_typed() {
    let root = temp_dir("trap_builtin_is_never_typed");
    write(
        &root.join("main.nia"),
        r#"
fn main() i32 {
    @trap()
}
"#,
    );

    let program = check_program(root.join("main.nia").to_string_lossy().into_owned());
    assert!(program.diagnostics.is_empty(), "{:?}", program.diagnostics);
}

#[test]
fn trap_builtin_must_be_called() {
    let root = temp_dir("trap_builtin_must_be_called");
    write(
        &root.join("main.nia"),
        r#"
fn main() void {
    @trap;
}
"#,
    );

    let program = check_program(root.join("main.nia").to_string_lossy().into_owned());
    assert!(
        program.diagnostics.iter().any(|diagnostic| diagnostic
            .diagnostic
            .summary
            .contains("builtin `@trap` must be called")),
        "{:?}",
        program.diagnostics
    );
}

#[test]
fn trap_builtin_rejects_arguments() {
    let root = temp_dir("trap_builtin_rejects_arguments");
    write(
        &root.join("main.nia"),
        r#"
fn value_arg() void {
    @trap(1);
}

fn type_arg() void {
    @trap[usize]();
}
"#,
    );

    let program = check_program(root.join("main.nia").to_string_lossy().into_owned());
    assert!(
        program.diagnostics.iter().any(|diagnostic| diagnostic
            .diagnostic
            .summary
            .contains("builtin `@trap` does not take value arguments")),
        "{:?}",
        program.diagnostics
    );
    assert!(
        program.diagnostics.iter().any(|diagnostic| diagnostic
            .diagnostic
            .summary
            .contains("builtin `@trap` does not take a type argument")),
        "{:?}",
        program.diagnostics
    );
}
