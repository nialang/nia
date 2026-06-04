// SPDX-License-Identifier: GPL-3.0-or-later
use super::common::*;
use crate::check_program;

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
            .message
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
            .message
            .contains("cannot assign to immutable comptime local `i`")),
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
    for value in 0..4 {
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
            .message
            .contains("cannot assign to immutable comptime local `p`")),
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
fn comptime_function_for_in_arrays_drive_array_lengths() {
    let root = temp_dir("comptime_function_for_in_arrays_drive_array_lengths");
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
    assert!(program.diagnostics.is_empty(), "{:?}", program.diagnostics);
}

#[test]
fn comptime_function_for_in_ranges_drive_array_lengths() {
    let root = temp_dir("comptime_function_for_in_ranges_drive_array_lengths");
    write(
        &root.join("main.nia"),
        r#"
comptime fn width() usize {
    var total: usize = 0;
    for value in 0..=5 {
        if value == 2 {
            continue;
        }
        if value == 5 {
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
    assert!(program.diagnostics.is_empty(), "{:?}", program.diagnostics);
}

#[test]
fn comptime_function_rejects_for_in_ranges_without_start_bound() {
    let root = temp_dir("comptime_function_rejects_for_in_ranges_without_start_bound");
    write(
        &root.join("main.nia"),
        r#"
comptime fn width() usize {
    var total: usize = 0;
    for value in ..5 {
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
            .message
            .contains("comptime for-in range requires a start bound")),
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
            .message
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
            .message
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
            .message
            .contains("comptime-only value")),
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
            .message
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
            .message
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
            .message
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
            .message
            .contains("comptime expression can only use comptime bindings")),
        "{:?}",
        program.diagnostics
    );
}
