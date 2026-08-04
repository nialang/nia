// SPDX-License-Identifier: GPL-3.0-or-later
use super::*;

#[test]
fn unused_const_function_rejects_wrong_return_type() {
    let root = temp_dir("unused_const_function_rejects_wrong_return_type");
    write(
        &root.join("main.nia"),
        r#"
const fn wrongReturn() usize {
    true
}

fn main() i32 { 0 }
"#,
    );

    let program = check_program(root.join("main.nia").to_string_lossy().into_owned());
    assert!(
        program.diagnostics.iter().any(|diagnostic| diagnostic
            .diagnostic
            .summary
            .contains("const function body does not match its declared type")),
        "{:?}",
        program.diagnostics
    );
}

#[test]
fn unused_const_function_rejects_runtime_call_in_expression_statement() {
    let root = temp_dir("unused_const_function_rejects_runtime_call_in_expression_statement");
    write(
        &root.join("main.nia"),
        r#"
fn runtimeOnly() usize {
    1
}

const fn wrongCall() usize {
    runtimeOnly();
    2
}

fn main() i32 { 0 }
"#,
    );

    let program = check_program(root.join("main.nia").to_string_lossy().into_owned());
    assert!(
        program.diagnostics.iter().any(|diagnostic| diagnostic
            .diagnostic
            .summary
            .contains("const expression can only call `const fn`")),
        "{:?}",
        program.diagnostics
    );
}

#[test]
fn unused_const_function_rejects_runtime_call_in_unselected_branch() {
    let root = temp_dir("unused_const_function_rejects_runtime_call_in_unselected_branch");
    write(
        &root.join("main.nia"),
        r#"
fn runtimeOnly() usize {
    1
}

const fn wrongCall() usize {
    if false {
        return runtimeOnly();
    }
    2
}

fn main() i32 { 0 }
"#,
    );

    let program = check_program(root.join("main.nia").to_string_lossy().into_owned());
    assert!(
        program.diagnostics.iter().any(|diagnostic| diagnostic
            .diagnostic
            .summary
            .contains("const expression can only call `const fn`")),
        "{:?}",
        program.diagnostics
    );
}

#[test]
fn unused_const_function_rejects_runtime_method_in_assignment() {
    let root = temp_dir("unused_const_function_rejects_runtime_method_in_assignment");
    write(
        &root.join("main.nia"),
        r#"
struct Value {
    inner: usize,
}

extend Value {
    fn runtimeOnly(self) usize {
        self.inner
    }
}

const fn wrongCall() usize {
    let mut result: usize = 0;
    result = Value{inner: 1}.runtimeOnly();
    result
}

fn main() i32 { 0 }
"#,
    );

    let program = check_program(root.join("main.nia").to_string_lossy().into_owned());
    assert!(
        program.diagnostics.iter().any(|diagnostic| diagnostic
            .diagnostic
            .summary
            .contains("const expression can only call `const fn`")),
        "{:?}",
        program.diagnostics
    );
}

#[test]
fn unused_const_function_rejects_local_initializer_type_mismatch() {
    let root = temp_dir("unused_const_function_rejects_local_initializer_type_mismatch");
    write(
        &root.join("main.nia"),
        r#"
const fn wrongLocal() usize {
    let value: usize = true;
    0
}

fn main() i32 { 0 }
"#,
    );

    let program = check_program(root.join("main.nia").to_string_lossy().into_owned());
    assert!(
        program.diagnostics.iter().any(|diagnostic| diagnostic
            .diagnostic
            .summary
            .contains("const binding initializer does not match its declared type")),
        "{:?}",
        program.diagnostics
    );
}

#[test]
fn unused_const_function_rejects_assignment_to_immutable_local() {
    let root = temp_dir("unused_const_function_rejects_assignment_to_immutable_local");
    write(
        &root.join("main.nia"),
        r#"
const fn wrongAssignment() usize {
    let value: usize = 1;
    value = 2;
    value
}

fn main() i32 { 0 }
"#,
    );

    let program = check_program(root.join("main.nia").to_string_lossy().into_owned());
    assert!(
        program.diagnostics.iter().any(|diagnostic| diagnostic
            .diagnostic
            .summary
            .contains("cannot assign to immutable const local `value`")),
        "{:?}",
        program.diagnostics
    );
}

#[test]
fn unused_const_function_rejects_assignment_type_mismatch() {
    let root = temp_dir("unused_const_function_rejects_assignment_type_mismatch");
    write(
        &root.join("main.nia"),
        r#"
const fn wrongAssignment() usize {
    let mut value: usize = 1;
    value = true;
    value
}

fn main() i32 { 0 }
"#,
    );

    let program = check_program(root.join("main.nia").to_string_lossy().into_owned());
    assert!(
        program.diagnostics.iter().any(|diagnostic| diagnostic
            .diagnostic
            .summary
            .contains("const assignment value does not match the target type")),
        "{:?}",
        program.diagnostics
    );
}

#[test]
fn unused_const_function_rejects_invalid_assignment_path() {
    let root = temp_dir("unused_const_function_rejects_invalid_assignment_path");
    write(
        &root.join("main.nia"),
        r#"
const fn wrongAssignment(index: usize) usize {
    let mut value: usize = 1;
    value[index] = 2;
    value
}

fn main() i32 { 0 }
"#,
    );

    let program = check_program(root.join("main.nia").to_string_lossy().into_owned());
    assert!(
        program.diagnostics.iter().any(|diagnostic| diagnostic
            .diagnostic
            .summary
            .contains("invalid const assignment target path")),
        "{:?}",
        program.diagnostics
    );
}

#[test]
fn unused_const_function_rejects_readonly_slice_assignment() {
    let root = temp_dir("unused_const_function_rejects_readonly_slice_assignment");
    write(
        &root.join("main.nia"),
        r#"
const fn wrongAssignment(values: &[usize]) usize {
    let mut view: &[usize] = values;
    view[0] = 2;
    view[0]
}

fn main() i32 { 0 }
"#,
    );

    let program = check_program(root.join("main.nia").to_string_lossy().into_owned());
    assert!(
        program.diagnostics.iter().any(|diagnostic| diagnostic
            .diagnostic
            .summary
            .contains("invalid const assignment target path")),
        "{:?}",
        program.diagnostics
    );
}

#[test]
fn unused_const_function_accepts_parameter_indexed_assignment() {
    let root = temp_dir("unused_const_function_accepts_parameter_indexed_assignment");
    write(
        &root.join("main.nia"),
        r#"
const fn update(index: usize) usize {
    let mut values: [2]usize = [1, 2];
    values[index] += 1;
    values[0]
}

fn main() i32 { 0 }
"#,
    );

    let program = check_program(root.join("main.nia").to_string_lossy().into_owned());
    assert!(program.diagnostics.is_empty(), "{:?}", program.diagnostics);
}

#[test]
fn unused_generic_const_function_accepts_plain_assignment() {
    let root = temp_dir("unused_generic_const_function_accepts_plain_assignment");
    write(
        &root.join("main.nia"),
        r#"
const fn replace[T](value: T, replacement: T) T {
    let mut result: T = value;
    result = replacement;
    result
}

fn main() i32 { 0 }
"#,
    );

    let program = check_program(root.join("main.nia").to_string_lossy().into_owned());
    assert!(program.diagnostics.is_empty(), "{:?}", program.diagnostics);
}

#[test]
fn unused_const_function_rejects_non_numeric_compound_assignment() {
    let root = temp_dir("unused_const_function_rejects_non_numeric_compound_assignment");
    write(
        &root.join("main.nia"),
        r#"
const fn wrongAssignment() usize {
    let mut value: bool = true;
    value += false;
    0
}

fn main() i32 { 0 }
"#,
    );

    let program = check_program(root.join("main.nia").to_string_lossy().into_owned());
    assert!(
        program.diagnostics.iter().any(|diagnostic| diagnostic
            .diagnostic
            .summary
            .contains("const compound assignment requires compatible numeric operands")),
        "{:?}",
        program.diagnostics
    );
}

#[test]
fn unused_const_function_checks_assignment_tail_as_void() {
    let root = temp_dir("unused_const_function_checks_assignment_tail_as_void");
    write(
        &root.join("main.nia"),
        r#"
const fn wrongTail() usize {
    let mut value: usize = 1;
    value = 2
}

fn main() i32 { 0 }
"#,
    );

    let program = check_program(root.join("main.nia").to_string_lossy().into_owned());
    assert!(
        program.diagnostics.iter().any(|diagnostic| diagnostic
            .diagnostic
            .summary
            .contains("const function body does not match its declared type")),
        "{:?}",
        program.diagnostics
    );
}

#[test]
fn unused_const_function_rejects_non_bool_conditions() {
    let root = temp_dir("unused_const_function_rejects_non_bool_conditions");
    write(
        &root.join("main.nia"),
        r#"
fn runtimeOnly() usize {
    1
}

const fn wrongCondition() usize {
    if 1usize {
        return runtimeOnly();
    }
    while 2usize {}
    0
}

fn main() i32 { 0 }
"#,
    );

    let program = check_program(root.join("main.nia").to_string_lossy().into_owned());
    assert!(
        program
            .diagnostics
            .iter()
            .filter(|diagnostic| diagnostic
                .diagnostic
                .summary
                .contains("const condition must have type bool"))
            .count()
            >= 2,
        "{:?}",
        program.diagnostics
    );
    assert!(
        program.diagnostics.iter().any(|diagnostic| diagnostic
            .diagnostic
            .summary
            .contains("const expression can only call `const fn`")),
        "{:?}",
        program.diagnostics
    );
}

#[test]
fn unused_const_function_rejects_invalid_builtin_unary_operand() {
    let root = temp_dir("unused_const_function_rejects_invalid_builtin_unary_operand");
    write(
        &root.join("main.nia"),
        r#"
const fn wrongNot() bool {
    not 1usize
}

fn main() i32 { 0 }
"#,
    );

    let program = check_program(root.join("main.nia").to_string_lossy().into_owned());
    assert!(
        program.diagnostics.iter().any(|diagnostic| diagnostic
            .diagnostic
            .summary
            .contains("const logical not requires a bool operand")),
        "{:?}",
        program.diagnostics
    );
}

#[test]
fn unused_const_function_rejects_invalid_builtin_binary_operands() {
    let root = temp_dir("unused_const_function_rejects_invalid_builtin_binary_operands");
    write(
        &root.join("main.nia"),
        r#"
const fn wrongAdd() usize {
    true + false;
    0
}

const fn wrongEquality() bool {
    1usize == true
}

fn main() i32 { 0 }
"#,
    );

    let program = check_program(root.join("main.nia").to_string_lossy().into_owned());
    assert!(
        program
            .diagnostics
            .iter()
            .filter(|diagnostic| diagnostic
                .diagnostic
                .summary
                .contains("const operator has incompatible operand types"))
            .count()
            >= 2,
        "{:?}",
        program.diagnostics
    );
}

#[test]
fn unused_const_function_rejects_incompatible_if_branch_types() {
    let root = temp_dir("unused_const_function_rejects_incompatible_if_branch_types");
    write(
        &root.join("main.nia"),
        r#"
const fn wrongBranches() usize {
    if true { 1usize } else { false }
}

fn main() i32 { 0 }
"#,
    );

    let program = check_program(root.join("main.nia").to_string_lossy().into_owned());
    assert!(
        program.diagnostics.iter().any(|diagnostic| diagnostic
            .diagnostic
            .summary
            .contains("const if branches have incompatible types")),
        "{:?}",
        program.diagnostics
    );
}

#[test]
fn unused_generic_const_function_defers_constrained_operator_types() {
    let root = temp_dir("unused_generic_const_function_defers_constrained_operator_types");
    write(
        &root.join("main.nia"),
        r#"
const fn add[T](lhs: T, rhs: T) T
where T: Add[T, Output = T] {
    lhs + rhs
}

fn main() i32 { 0 }
"#,
    );

    let program = check_program(root.join("main.nia").to_string_lossy().into_owned());
    assert!(program.diagnostics.is_empty(), "{:?}", program.diagnostics);
}

#[test]
fn recursive_const_evaluation_has_a_call_depth_limit() {
    let root = temp_dir("recursive_const_evaluation_has_a_call_depth_limit");
    write(
        &root.join("main.nia"),
        r#"
const fn recurse(value: usize) usize {
    recurse(value + 1)
}

const result: usize = recurse(0);

fn main() i32 { 0 }
"#,
    );

    let program = check_program(root.join("main.nia").to_string_lossy().into_owned());
    assert!(
        program.diagnostics.iter().any(|diagnostic| diagnostic
            .diagnostic
            .summary
            .contains("const evaluation exceeded the 256 call depth limit")),
        "{:?}",
        program.diagnostics
    );
}

#[test]
fn terminating_recursive_const_function_remains_valid() {
    let root = temp_dir("terminating_recursive_const_function_remains_valid");
    write(
        &root.join("main.nia"),
        r#"
const fn countdown(value: usize) usize {
    if value == 0 {
        return 0;
    }
    countdown(value - 1) + 1
}

const result: usize = countdown(32);

fn main() i32 {
    result as i32
}
"#,
    );

    let program = check_program(root.join("main.nia").to_string_lossy().into_owned());
    assert!(program.diagnostics.is_empty(), "{:?}", program.diagnostics);
}

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
