// SPDX-License-Identifier: GPL-3.0-or-later
use super::*;

#[test]
fn const_char_arrays_compare_with_string_literals() {
    let root = temp_dir("const_char_arrays_compare_with_string_literals");
    write(
        &root.join("main.nia"),
        r#"
const fun is_sample_os(value: [char; 5]) bool {
    value[0] == 'l' and value[1] == 'i' and value[2] == 'n'
        and value[3] == 'u' and value[4] == 'x'
}

const n: usize = if is_sample_os("linux") { 4usize } else { 2usize };

fun main() i32 {
    let mut values: [i32; n] = [0; n];
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
const fun accept_linux(value: [char; 5]) usize {
    if value.len() == 5usize and value[0] == 'l' and value[1] == 'i'
        and value[2] == 'n' and value[3] == 'u' and value[4] == 'x' {
        5usize
    } else {
        0usize
    }
}

const n: usize = accept_linux("linux");

fun main() i32 {
    let mut values: [i32; n] = [0; n];
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
const fun accept_short(value: [char; 4]) usize {
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
const os: [char; 5] = "linux";
const n: usize = if os.len() == 5usize {
    os.len()
} else {
    0usize
};

fun main() i32 {
    let mut values: [i32; n] = [0; n];
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
pub const os: [char; 5] = "linux";
"#,
    );
    write(
        &root.join("main.nia"),
        r#"
module config;
using entry::config;

const n: usize = config::os.len();

fun main() i32 {
    let mut values: [i32; n] = [0; n];
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
const fun sample_os() [char; 5] {
    "linux"
}

const os: [char; 5] = sample_os();
const n: usize = os.len();

fun main() i32 {
    let mut values: [i32; n] = [0; n];
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
const fun sample_os() [char; 5] {
    "linux"
}

const n: usize = sample_os().len();

fun main() i32 {
    let mut values: [i32; n] = [0; n];
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
const fun sample_os() [char; 4] {
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
const fun total(values: [[char; 5]; 1]) usize {
    let mut n: usize = 0usize;
    for os in values {
        n += os.len();
    }
    n
}

const n: usize = total(["linux"]);

fun main() i32 {
    let mut values: [i32; n] = [0; n];
    values.len() as i32
}
"#,
    );

    let program = check_program(root.join("main.nia").to_string_lossy().into_owned());
    assert!(
        program.diagnostics.iter().any(|diagnostic| diagnostic
            .diagnostic
            .summary
            .contains("for-in expects an Iterable")),
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
const fun sample_os() ?[char; 5] {
    ?"linux"
}

const n: usize = match sample_os() {
    ?os => {
        os.len()
    },
    null => {
        0usize
    },
};

fun main() i32 {
    let mut values: [i32; n] = [0; n];
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
const n: usize = match os {
    ?payload => {
        payload.len()
    },
    null => {
        0usize
    },
};

fun main() i32 {
    let mut values: [i32; n] = [0; n];
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

pub const fun sample_os() [char; os_len] {
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

fun main() i32 {
    let mut values: [i32; n] = [0; n];
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

pub const fun sample_os() [char; os_len] {
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
const fun byte_score(value: [u8; 3]) usize {
    if value[0] == b'n' {
        value[2] as usize
    } else {
        0usize
    }
}

const fun c_score(value: [u8; 4]) usize {
    if value[3] == 0u8 {
        value[0] as usize
    } else {
        0usize
    }
}

const n: usize = byte_score(b"nia") + c_score(b"nia\0");

fun main() i32 {
    let mut values: [i32; n] = [0; n];
    values.len() as i32
}
"#,
    );

    let program = check_program(root.join("main.nia").to_string_lossy().into_owned());
    assert!(program.diagnostics.is_empty(), "{:?}", program.diagnostics);
}

#[test]
fn const_string_literals_support_source_concatenation_and_newlines() {
    let root = temp_dir("const_string_literals_support_source_concatenation_and_newlines");
    write(
        &root.join("main.nia"),
        r#"
const fun char_score(value: [char; 3]) usize {
    if value[1] == '\x69' {
        3usize
    } else {
        0usize
    }
}

const fun byte_score(value: [u8; 3]) usize {
    if value[2] == b'a' {
        5usize
    } else {
        0usize
    }
}

const fun newline_score(value: [char; 11]) usize {
    if value[5] == '\n' {
        7usize
    } else {
        0usize
    }
}

const text: [char; 3] = "n" "ia";
const bytes: [u8; 3] = b"n" b"ia";
const split_text: [char; 11] = (
    "hello\n"
    "world"
);
const split_bytes: [u8; 11] = (
    b"hello\n"
    b"world"
);
const n: usize =
    char_score(text)
    + byte_score(bytes)
    + newline_score(split_text)
    + byte_score(split_bytes[8..]);

fun main() i32 {
    let mut values: [i32; n] = [0; n];
    values.len() as i32
}
"#,
    );

    let program = check_program(root.join("main.nia").to_string_lossy().into_owned());
    assert!(program.diagnostics.is_empty(), "{:?}", program.diagnostics);
}
