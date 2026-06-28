// SPDX-License-Identifier: GPL-3.0-or-later
use super::*;

#[test]
fn conditional_attribute_prunes_unselected_function_body_statement() {
    let root = temp_dir("conditional_attribute_prunes_unselected_function_body_statement");
    write(
        &root.join("main.nia"),
        r#"
fn main() i32 {
    @[if false]
    _ = missing_name;
    1
}
"#,
    );

    let program = check_program(root.join("main.nia").to_string_lossy().into_owned());
    assert!(program.diagnostics.is_empty(), "{:?}", program.diagnostics);
}

#[test]
fn conditional_attribute_accepts_target_fields() {
    let root = temp_dir("conditional_attribute_accepts_target_fields");
    write(
        &root.join("main.nia"),
        r#"
@[if pointer_width == 64 or pointer_width == 32]
fn selected() i32 { 1 }

fn main() i32 { selected() }
"#,
    );

    let program = check_program(root.join("main.nia").to_string_lossy().into_owned());
    assert!(program.diagnostics.is_empty(), "{:?}", program.diagnostics);
}

#[test]
fn user_struct_fields_are_ordinary_comptime_values() {
    let root = temp_dir("user_struct_fields_are_ordinary_comptime_values");
    write(
        &root.join("main.nia"),
        r#"
comptime builtin = {target: {pointer_width: 64usize}};
comptime bits: usize = builtin.target.pointer_width;
comptime word_bytes: usize = bits / 8;

fn main() i32 {
    let mut bytes: [word_bytes]u8 = [0; word_bytes];
    bytes.len() as i32
}
"#,
    );

    let program = check_program(root.join("main.nia").to_string_lossy().into_owned());
    assert!(program.diagnostics.is_empty(), "{:?}", program.diagnostics);
}

#[test]
fn removed_builtin_struct_is_rejected() {
    let root = temp_dir("removed_builtin_struct_is_rejected");
    write(
        &root.join("main.nia"),
        r#"
fn main() i32 {
    let builtin = @builtin();
    _ = builtin;
    0
}
"#,
    );

    let program = check_program(root.join("main.nia").to_string_lossy().into_owned());
    assert!(
        program.diagnostics.iter().any(|diagnostic| diagnostic
            .diagnostic
            .summary
            .contains("unknown builtin `@builtin`")),
        "{:?}",
        program.diagnostics
    );
}

#[test]
fn comptime_function_rejects_structural_array_field_assignment_type_mismatch() {
    let root =
        temp_dir("comptime_function_rejects_structural_array_field_assignment_type_mismatch");
    write(
        &root.join("main.nia"),
        r#"
comptime fn width() usize {
    comptime mut config = {target: {os: "linux".*, pointer_width: 64usize}};
    config.target.os = true;
    config.target.pointer_width
}

comptime n: usize = width();
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
fn comptime_function_rejects_structural_bool_field_assignment_type_mismatch() {
    let root = temp_dir("comptime_function_rejects_structural_bool_field_assignment_type_mismatch");
    write(
        &root.join("main.nia"),
        r#"
comptime fn width() usize {
    comptime mut config = {enabled: true};
    config.enabled = 1usize;
    1usize
}

comptime n: usize = width();
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
fn old_target_predicate_builtins_are_rejected() {
    let root = temp_dir("old_target_predicate_builtins_are_rejected");
    write(
        &root.join("main.nia"),
        r#"
fn main() i32 {
    if @target_os("linux") {
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
            .contains("unknown builtin `@target_os`")),
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
using entry::config;

fn main() i32 {
    let mut values: [config::width]i32 = [1, 2, 3, 4];
    values[config::width - 1]
}
"#,
    );
    write(
        &root.join("config.nia"),
        r#"
pub comptime width: usize = 4;
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

comptime pair_size: usize = @size[Pair]();
comptime pair_align: usize = @align[Pair]();

fn main() i32 {
    let mut bytes: [pair_size]u8 = [0; pair_size];
    bytes.len() as i32 + pair_align as i32
}
"#,
    );

    let program = check_program(root.join("main.nia").to_string_lossy().into_owned());
    assert!(program.diagnostics.is_empty(), "{:?}", program.diagnostics);
}

#[test]
fn embed_reads_bytes_relative_to_source_file() {
    let root = temp_dir("embed_reads_bytes_relative_to_source_file");
    std::fs::create_dir_all(root.join("src/assets")).expect("create assets dir");
    std::fs::write(root.join("src/assets/payload.bin"), [b'n', b'i', b'a']).expect("write payload");
    write(
        &root.join("src/main.nia"),
        r#"
comptime payload = @embed("assets/payload.bin");

comptime fn score(bytes: [3]u8) usize {
    if bytes[0] == b'n' and bytes[1] == b'i' and bytes[2] == b'a' {
        bytes.len()
    } else {
        0usize
    }
}

comptime n: usize = score(payload.*);

fn main() i32 {
    let mut values: [n]u8 = [0; n];
    values.len() as i32
}
"#,
    );

    let program = check_program(root.join("src/main.nia").to_string_lossy().into_owned());
    assert!(program.diagnostics.is_empty(), "{:?}", program.diagnostics);
}

#[test]
fn embed_reports_missing_file() {
    let root = temp_dir("embed_reports_missing_file");
    write(
        &root.join("main.nia"),
        r#"
comptime payload = @embed("missing.bin");

fn main() i32 {
    payload.len() as i32
}
"#,
    );

    let program = check_program(root.join("main.nia").to_string_lossy().into_owned());
    assert!(
        program
            .diagnostics
            .iter()
            .any(|diagnostic| diagnostic.diagnostic.summary.contains("failed to embed")),
        "{:?}",
        program.diagnostics
    );
}

#[test]
fn embed_is_rejected_in_runtime_body() {
    let root = temp_dir("embed_is_rejected_in_runtime_body");
    std::fs::write(root.join("payload.bin"), [1u8, 2u8, 3u8]).expect("write payload");
    write(
        &root.join("main.nia"),
        r#"
fn main() i32 {
    let payload = @embed("payload.bin");
    payload.len() as i32
}
"#,
    );

    let program = check_program(root.join("main.nia").to_string_lossy().into_owned());
    assert!(
        program.diagnostics.iter().any(|diagnostic| diagnostic
            .diagnostic
            .summary
            .contains("builtin `@embed` can only be evaluated at comptime")),
        "{:?}",
        program.diagnostics
    );
}
