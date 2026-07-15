// SPDX-License-Identifier: GPL-3.0-or-later
use super::*;

#[test]
fn const_dependency_cycles_are_diagnosed() {
    let root = temp_dir("const_dependency_cycles_are_diagnosed");
    write(
        &root.join("main.nia"),
        r#"
const a: i32 = b;
const b: i32 = a;

fn main() i32 { 0 }
"#,
    );

    let program = check_program(root.join("main.nia").to_string_lossy().into_owned());
    assert!(
        program.diagnostics.iter().any(|diagnostic| diagnostic
            .diagnostic
            .summary
            .contains("cyclic const dependency")),
        "{:?}",
        program.diagnostics
    );
}

#[test]
fn const_rejects_runtime_local_dependency() {
    let root = temp_dir("const_rejects_runtime_local_dependency");
    write(
        &root.join("main.nia"),
        r#"
fn main() i32 {
    let mut runtime = 4;
    const n: usize = runtime;
    let mut values: [n]i32 = [1, 2, 3, 4];
    values[0]
}
"#,
    );

    let program = check_program(root.join("main.nia").to_string_lossy().into_owned());
    assert!(
        program.diagnostics.iter().any(|diagnostic| diagnostic
            .diagnostic
            .summary
            .contains("const expression can only use const bindings")),
        "{:?}",
        program.diagnostics
    );
}

#[test]
fn const_error_builtin_reports_message() {
    let root = temp_dir("const_error_builtin_reports_message");
    write(
        &root.join("main.nia"),
        r#"
const n: usize = std::builtin::error("unsupported target");

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
fn const_error_builtin_is_pruned_by_const_if_expression() {
    let root = temp_dir("const_error_builtin_is_pruned_by_const_if_expression");
    write(
        &root.join("main.nia"),
        r#"
const fn selected() usize {
    if true {
        return 4usize;
    }
    std::builtin::error("unreachable const branch")
}

const n: usize = selected();

fn main() usize { n }
"#,
    );

    let program = check_program(root.join("main.nia").to_string_lossy().into_owned());
    assert!(program.diagnostics.is_empty(), "{:?}", program.diagnostics);
}

#[test]
fn const_error_builtin_requires_string_message() {
    let root = temp_dir("const_error_builtin_requires_string_message");
    write(
        &root.join("main.nia"),
        r#"
const n: usize = std::builtin::error(10usize);

fn main() i32 { 0 }
"#,
    );

    let program = check_program(root.join("main.nia").to_string_lossy().into_owned());
    assert!(
        program.diagnostics.iter().any(|diagnostic| diagnostic
            .diagnostic
            .summary
            .contains("requires a const string message")),
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
    std::builtin::error("runtime panic")
}
"#,
    );

    let program = check_program(root.join("main.nia").to_string_lossy().into_owned());
    assert!(
        program.diagnostics.iter().any(|diagnostic| diagnostic
            .diagnostic
            .summary
            .contains("can only be evaluated at const")),
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
    std::builtin::trap()
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
    std::builtin::trap;
}
"#,
    );

    let program = check_program(root.join("main.nia").to_string_lossy().into_owned());
    assert!(
        program.diagnostics.iter().any(|diagnostic| diagnostic
            .diagnostic
            .summary
            .contains("builtin `trap` must be called")),
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
    std::builtin::trap(1);
}

fn type_arg() void {
    std::builtin::trap[usize]();
}
"#,
    );

    let program = check_program(root.join("main.nia").to_string_lossy().into_owned());
    assert!(
        program.diagnostics.iter().any(|diagnostic| diagnostic
            .diagnostic
            .summary
            .contains("builtin `trap` does not take value arguments")),
        "{:?}",
        program.diagnostics
    );
    assert!(
        program.diagnostics.iter().any(|diagnostic| diagnostic
            .diagnostic
            .summary
            .contains("builtin `trap` does not take a type argument")),
        "{:?}",
        program.diagnostics
    );
}
