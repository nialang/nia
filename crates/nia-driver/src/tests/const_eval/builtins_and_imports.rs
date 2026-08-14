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
fn user_struct_fields_are_ordinary_const_values() {
    let root = temp_dir("user_struct_fields_are_ordinary_const_values");
    write(
        &root.join("main.nia"),
        r#"
struct Target { pointer_width: usize }
struct Config { target: Target }
const builtin = Config { target: Target { pointer_width: 64usize } };
const bits: usize = builtin.target.pointer_width;
const word_bytes: usize = bits / 8;

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
            .contains("expected expression")),
        "{:?}",
        program.diagnostics
    );
}

#[test]
fn builtin_functions_require_explicit_std_builtin_path_or_using() {
    let root = temp_dir("builtin_functions_require_explicit_std_builtin_path_or_using");
    std::fs::create_dir_all(root.join("assets")).expect("create assets dir");
    std::fs::write(root.join("assets/payload.bin"), b"nia").expect("write payload");
    write(
        &root.join("main.nia"),
        r#"
using std::builtin::{align, embed, offset, size};

struct Pair {
    tag: u8,
    value: u32,
}

const word_size: usize = size[usize]();
const word_align: usize = align[usize]();
const value_offset: usize = offset[Pair]("value");
const payload = embed("assets/payload.bin");
const payload_len: usize = payload.len();

fn main() usize {
    word_size + word_align + value_offset + payload_len
}
"#,
    );

    let program = check_program(root.join("main.nia").to_string_lossy().into_owned());
    assert!(program.diagnostics.is_empty(), "{:?}", program.diagnostics);
}

#[test]
fn builtin_traits_require_explicit_std_builtin_using() {
    let root = temp_dir("builtin_traits_require_explicit_std_builtin_using");
    write(
        &root.join("main.nia"),
        r#"
using std::builtin::Iterator;

struct Counter {
    next_value: usize,
    end: usize,
}

extend Counter : Iterator {
    type Item = usize;

    fn next(&mut self) ?usize {
        if self.next_value >= self.end {
            null
        } else {
            let value = self.next_value;
            self.next_value += 1usize;
            ?value
        }
    }
}

fn count[I](iter: I) usize
where I: Iterator {
    let mut values = iter;
    let mut total = 0usize;
    let mut done = false;
    while not done {
        switch values.next() {
            ?value => {
                _ = value;
                total += 1usize;
            },
            null => {
                done = true;
            },
        }
    }
    total
}

fn main() usize {
    count(Counter { next_value: 0usize, end: 3usize })
}
"#,
    );

    let program = check_program(root.join("main.nia").to_string_lossy().into_owned());
    assert!(program.diagnostics.is_empty(), "{:?}", program.diagnostics);
}

#[test]
fn local_type_alias_can_use_builtin_trait_names_without_ambient_conflict() {
    let root = temp_dir("local_type_alias_can_use_builtin_trait_names_without_ambient_conflict");
    write(
        &root.join("main.nia"),
        r#"
type Ptr[T] = &T;

fn read(value: Ptr[u8]) u8 {
    value.*
}
"#,
    );

    let program = check_program(root.join("main.nia").to_string_lossy().into_owned());
    assert!(program.diagnostics.is_empty(), "{:?}", program.diagnostics);
}

#[test]
fn const_function_rejects_nominal_array_field_assignment_type_mismatch() {
    let root = temp_dir("const_function_rejects_nominal_array_field_assignment_type_mismatch");
    write(
        &root.join("main.nia"),
        r#"
struct Target { os: [5]char, pointer_width: usize }
struct Config { target: Target }

const fn width() usize {
    let mut config = Config { target: Target { os: "linux", pointer_width: 64usize } };
    config.target.os = true;
    config.target.pointer_width
}

const n: usize = width();
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
fn const_function_rejects_nominal_bool_field_assignment_type_mismatch() {
    let root = temp_dir("const_function_rejects_nominal_bool_field_assignment_type_mismatch");
    write(
        &root.join("main.nia"),
        r#"
struct Config { enabled: bool }

const fn width() usize {
    let mut config = Config { enabled: true };
    config.enabled = 1usize;
    1usize
}

const n: usize = width();
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
            .contains("expected expression")),
        "{:?}",
        program.diagnostics
    );
}

#[test]
fn imported_const_values_drive_array_lengths() {
    let root = temp_dir("imported_const_values_drive_array_lengths");
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
pub const width: usize = 4;
"#,
    );

    let program = check_program(root.join("main.nia").to_string_lossy().into_owned());
    assert!(program.diagnostics.is_empty(), "{:?}", program.diagnostics);
}

#[test]
fn layout_builtins_are_const_values_for_concrete_types() {
    let root = temp_dir("layout_builtins_are_const_values_for_concrete_types");
    write(
        &root.join("main.nia"),
        r#"
struct Pair {
    a: u8,
    b: i32,
}

const pair_size: usize = std::builtin::size[Pair]();
const pair_align: usize = std::builtin::align[Pair]();

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
    std::fs::write(root.join("src/assets/payload.bin"), b"nia").expect("write payload");
    write(
        &root.join("src/main.nia"),
        r#"
const payload = std::builtin::embed("assets/payload.bin");

const fn score(bytes: [3]u8) usize {
    if bytes[0] == b'n' and bytes[1] == b'i' and bytes[2] == b'a' {
        bytes.len()
    } else {
        0usize
    }
}

const n: usize = score(payload);

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
const payload = std::builtin::embed("missing.bin");

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
    let payload = std::builtin::embed("payload.bin");
    payload.len() as i32
}
"#,
    );

    let program = check_program(root.join("main.nia").to_string_lossy().into_owned());
    assert!(
        program.diagnostics.iter().any(|diagnostic| diagnostic
            .diagnostic
            .summary
            .contains("builtin `embed` can only be evaluated at const")),
        "{:?}",
        program.diagnostics
    );
}

#[test]
fn unicode_scalar_conversion_evaluates_at_const() {
    let root = temp_dir("unicode_scalar_conversion_evaluates_at_const");
    write(
        &root.join("main.nia"),
        r#"
using std::unicode;

const scalar: ?char = unicode::fromScalarValue(0x03bb);
const surrogate: ?char = unicode::fromScalarValue(0xd800);
const width: usize = switch scalar {
    ?value => if (value as u32) == 0x03bb { 3 } else { 1 },
    null => 1,
};
const invalidWidth: usize = switch surrogate {
    ?value => if (value as u32) == 0xd800 { 1 } else { 1 },
    null => 2,
};

fn main() i32 {
    let values: [width + invalidWidth]u8 = [0; width + invalidWidth];
    values.len() as i32
}
"#,
    );

    let program = check_program(root.join("main.nia").to_string_lossy().into_owned());
    assert!(program.diagnostics.is_empty(), "{:?}", program.diagnostics);
}
