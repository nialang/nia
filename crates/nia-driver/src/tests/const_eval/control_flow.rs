// SPDX-License-Identifier: GPL-3.0-or-later
use super::*;

#[test]
fn const_function_loop_statements_drive_array_lengths() {
    let root = temp_dir("const_function_loop_statements_drive_array_lengths");
    write(
        &root.join("main.nia"),
        r#"
const fn width() usize {
    loop {
        break;
    }
    let ignored = 8;
    return 6;
}

const n: usize = width();

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
fn const_function_while_statements_drive_array_lengths() {
    let root = temp_dir("const_function_while_statements_drive_array_lengths");
    write(
        &root.join("main.nia"),
        r#"
const fn width(flag: bool) usize {
    while false {
        return 1;
    }
    while flag {
        break;
    }
    return 7;
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
fn const_function_if_statement_rejects_non_bool_condition() {
    let root = temp_dir("const_function_if_statement_rejects_non_bool_condition");
    write(
        &root.join("main.nia"),
        r#"
const fn width() usize {
    if 1usize {
        return 1usize;
    }
    return 2usize;
}

const n: usize = width();
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
fn const_function_while_statement_rejects_non_bool_condition() {
    let root = temp_dir("const_function_while_statement_rejects_non_bool_condition");
    write(
        &root.join("main.nia"),
        r#"
const fn width() usize {
    while 1usize {
        return 1usize;
    }
    return 2usize;
}

const n: usize = width();
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
fn const_statement_type_check_allows_loop_control_flow() {
    let root = temp_dir("const_statement_type_check_allows_loop_control_flow");
    write(
        &root.join("main.nia"),
        r#"
const fn width(flag: bool) usize {
    if flag {
        return 4usize;
    }
    let mut total: usize = 0;
    let mut value: usize = 0;
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

const n: usize = width(false);

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
fn const_function_mutable_locals_drive_loop_array_lengths() {
    let root = temp_dir("const_function_mutable_locals_drive_loop_array_lengths");
    write(
        &root.join("main.nia"),
        r#"
const fn width() usize {
    let mut i: usize = 0;
    while true {
        if i == 6 {
            break;
        }
        i += 1;
    }
    i
}

const n: usize = width();

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
fn const_function_integer_comparisons_drive_control_flow() {
    let root = temp_dir("const_function_integer_comparisons_drive_control_flow");
    write(
        &root.join("main.nia"),
        r#"
const fn width(limit: usize) usize {
    let mut i: usize = 0;
    let mut total: usize = 0;
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

const n: usize = width(6);

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
fn const_function_supports_plain_local_assignment() {
    let root = temp_dir("const_function_supports_plain_local_assignment");
    write(
        &root.join("main.nia"),
        r#"
const fn width() usize {
    let mut i: usize = 2;
    i = 5;
    i *= 2;
    i
}

const n: usize = width();

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
fn const_initializer_block_uses_transient_mutable_locals() {
    let root = temp_dir("const_initializer_block_uses_transient_mutable_locals");
    write(
        &root.join("main.nia"),
        r#"
const n: usize = {
    let mut value: usize = 0;
    while value < 4usize {
        value += 1usize;
    }
    value
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
