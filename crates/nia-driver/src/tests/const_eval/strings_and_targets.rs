// SPDX-License-Identifier: GPL-3.0-or-later
use super::*;

#[test]
fn const_char_arrays_compare_with_string_literals() {
    let root = temp_dir("const_char_arrays_compare_with_string_literals");
    write(
        &root.join("main.nia"),
        r#"
const fn is_sample_os(value: [5]char) bool {
    "linux" == value
}

const n: usize = if is_sample_os("linux") { 4usize } else { 2usize };

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
fn const_char_arrays_pass_as_char_arrays() {
    let root = temp_dir("const_char_arrays_pass_as_char_arrays");
    write(
        &root.join("main.nia"),
        r#"
const fn accept_linux(value: [5]char) usize {
    if value.len() == 5usize and value[0] == 'l' and value == "linux" {
        5usize
    } else {
        0usize
    }
}

const n: usize = accept_linux("linux");

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
fn const_char_arrays_validate_char_array_lengths() {
    let root = temp_dir("const_char_arrays_validate_char_array_lengths");
    write(
        &root.join("main.nia"),
        r#"
const fn accept_short(value: [4]char) usize {
    value.len()
}

const n: usize = accept_short("linux");
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
fn typed_const_char_array_binding_is_a_char_array() {
    let root = temp_dir("typed_const_char_array_binding_is_a_char_array");
    write(
        &root.join("main.nia"),
        r#"
const os: [5]char = "linux";
const n: usize = if os.len() == 5usize {
    os.len()
} else {
    0usize
};

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
fn imported_typed_const_char_array_binding_is_a_char_array() {
    let root = temp_dir("imported_typed_const_char_array_binding_is_a_char_array");
    write(
        &root.join("config.nia"),
        r#"
pub const os: [5]char = "linux";
"#,
    );
    write(
        &root.join("main.nia"),
        r#"
module config;
using entry::config;

const n: usize = config::os.len();

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
fn const_function_returned_char_array_is_a_char_array() {
    let root = temp_dir("const_function_returned_char_array_is_a_char_array");
    write(
        &root.join("main.nia"),
        r#"
const fn sample_os() [5]char {
    "linux"
}

const os: [5]char = sample_os();
const n: usize = os.len();

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
fn const_function_returned_char_array_can_be_consumed_directly() {
    let root = temp_dir("const_function_returned_char_array_can_be_consumed_directly");
    write(
        &root.join("main.nia"),
        r#"
const fn sample_os() [5]char {
    "linux"
}

const n: usize = sample_os().len();

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
fn const_function_returned_char_array_validates_char_array_length() {
    let root = temp_dir("const_function_returned_char_array_validates_char_array_length");
    write(
        &root.join("main.nia"),
        r#"
const fn sample_os() [4]char {
    "linux"
}

const os = sample_os();
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
fn const_for_in_array_binding_requires_iterator() {
    let root = temp_dir("const_for_in_array_binding_requires_iterator");
    write(
        &root.join("main.nia"),
        r#"
const fn total(values: [1][5]char) usize {
    let mut n: usize = 0usize;
    for os in values {
        n += os.len();
    }
    n
}

const n: usize = total(["linux"]);

fn main() i32 {
    let mut values: [n]i32 = [0; n];
    values.len() as i32
}
"#,
    );

    let program = check_program(root.join("main.nia").to_string_lossy().into_owned());
    assert!(
        program.diagnostics.iter().any(|diagnostic| diagnostic
            .diagnostic
            .summary
            .contains("const for-in expects an Iterator")),
        "{:?}",
        program.diagnostics
    );
}

#[test]
fn const_if_pattern_optional_payload_preserves_array_type() {
    let root = temp_dir("const_if_pattern_optional_payload_preserves_array_type");
    write(
        &root.join("main.nia"),
        r#"
const fn sample_os() ?[5]char {
    ?"linux"
}

const n: usize = if ?os = sample_os() {
    os.len()
} or null {
    0usize
};

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
fn const_optional_constructor_projects_char_array_payload() {
    let root = temp_dir("const_optional_constructor_projects_char_array_payload");
    write(
        &root.join("main.nia"),
        r#"
const os = ?"linux";
const n: usize = if ?payload = os {
    payload.len()
} or null {
    0usize
};

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
fn imported_const_function_return_validates_imported_array_length() {
    let root = temp_dir("imported_const_function_return_validates_imported_array_length");
    write(
        &root.join("config.nia"),
        r#"
pub const os_len: usize = 5usize;

pub const fn sample_os() [os_len]char {
    "linux"
}
"#,
    );
    write(
        &root.join("main.nia"),
        r#"
module config;
using entry::config;

const n: usize = config::sample_os().len();

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
fn imported_const_function_return_rejects_imported_array_length_mismatch() {
    let root = temp_dir("imported_const_function_return_rejects_imported_array_length_mismatch");
    write(
        &root.join("config.nia"),
        r#"
pub const os_len: usize = 4usize;

pub const fn sample_os() [os_len]char {
    "linux"
}
"#,
    );
    write(
        &root.join("main.nia"),
        r#"
module config;
using entry::config;

const os = config::sample_os();
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
fn const_byte_string_literals_are_typed_arrays() {
    let root = temp_dir("const_byte_string_literals_are_typed_arrays");
    write(
        &root.join("main.nia"),
        r#"
const fn byte_score(value: [3]u8) usize {
    if value[0] == b'n' {
        value[2] as usize
    } else {
        0usize
    }
}

const fn c_score(value: [4]u8) usize {
    if value[3] == 0u8 {
        value[0] as usize
    } else {
        0usize
    }
}

const n: usize = byte_score(b"nia") + c_score(b"nia\0");

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
fn const_string_literals_support_source_concatenation_and_multiline() {
    let root = temp_dir("const_string_literals_support_source_concatenation_and_multiline");
    write(
        &root.join("main.nia"),
        r#"
const fn char_score(value: [3]char) usize {
    if value[1] == '\x69' {
        3usize
    } else {
        0usize
    }
}

const fn byte_score(value: [3]u8) usize {
    if value[2] == b'a' {
        5usize
    } else {
        0usize
    }
}

const fn multiline_score(value: [11]char) usize {
    if value[5] == '\n' {
        7usize
    } else {
        0usize
    }
}

const text: [3]char = "n" "ia";
const bytes: [3]u8 = b"n" b"ia";
const multiline: [11]char = (
    \\hello
    \\world
);
const byte_multiline: [11]u8 = (
    b\\hello
    \\world
);
const n: usize =
    char_score(text)
    + byte_score(bytes)
    + multiline_score(multiline)
    + byte_score(byte_multiline[8..]);

fn main() i32 {
    let mut values: [n]i32 = [0; n];
    values.len() as i32
}
"#,
    );

    let program = check_program(root.join("main.nia").to_string_lossy().into_owned());
    assert!(program.diagnostics.is_empty(), "{:?}", program.diagnostics);
}
