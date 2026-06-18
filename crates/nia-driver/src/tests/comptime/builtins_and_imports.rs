// SPDX-License-Identifier: GPL-3.0-or-later
use super::*;

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
module config;
using root::config;

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
