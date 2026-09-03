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
            .contains("type mismatch in function body")),
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
    assert_eq!(
        program
            .diagnostics
            .iter()
            .filter(|diagnostic| diagnostic
                .diagnostic
                .summary
                .contains("const expression can only call `const fn`"))
            .count(),
        1,
        "{:?}",
        program.diagnostics
    );
}

#[test]
fn unused_const_function_rejects_runtime_only_builtin() {
    let root = temp_dir("unused_const_function_rejects_runtime_only_builtin");
    write(
        &root.join("main.nia"),
        r#"
const fn wrongBuiltin() u32 {
    std::builtin::popcount[u32](1u32)
}

fn main() i32 { 0 }
"#,
    );

    let program = check_program(root.join("main.nia").to_string_lossy().into_owned());
    assert!(
        program.diagnostics.iter().any(|diagnostic| diagnostic
            .diagnostic
            .summary
            .contains("builtin `popcount` is not available during const evaluation")),
        "{:?}",
        program.diagnostics
    );
}

#[test]
fn unused_const_function_rejects_function_pointer_values_and_calls() {
    let root = temp_dir("unused_const_function_rejects_function_pointer_values_and_calls");
    write(
        &root.join("main.nia"),
        r#"
const fn increment(value: usize) usize {
    value + 1
}

const fn storesFunctionPointer() usize {
    let callback = & increment;
    1
}

const fn callsFunctionPointer() usize {
    let callback = & increment;
    callback(1)
}

fn main() i32 { 0 }
"#,
    );

    let program = check_program(root.join("main.nia").to_string_lossy().into_owned());
    assert_eq!(
        program
            .diagnostics
            .iter()
            .filter(|diagnostic| diagnostic
                .diagnostic
                .summary
                .contains("function pointer values are not available during const evaluation"))
            .count(),
        2,
        "{:?}",
        program.diagnostics
    );
    assert!(
        program.diagnostics.iter().any(|diagnostic| diagnostic
            .diagnostic
            .summary
            .contains("indirect function calls are not available during const evaluation")),
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
            .contains("type mismatch in binding initializer")),
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
            .contains("assignment target is not assignable: local is let")),
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
            .contains("type mismatch in assignment")),
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
            .contains("trait bound not satisfied")),
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
            .contains("assignment target is not assignable: slice is read-only")),
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
    let mut values: [usize; 2] = [1, 2];
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
            .contains("trait bound not satisfied: bool: Add[bool]")),
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
            .contains("type mismatch in function body")),
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
                .contains("condition: expected bool"))
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
            .contains("trait bound not satisfied: usize: Not")),
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
                .contains("trait bound not satisfied"))
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
            .contains("type mismatch in if branches")),
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
fn unused_const_function_rejects_match_pattern_and_checks_arm_body() {
    let root = temp_dir("unused_const_function_rejects_match_pattern_and_checks_arm_body");
    write(
        &root.join("main.nia"),
        r#"
fn runtimeOnly() usize {
    1
}

const fn wrongSwitch(value: usize) usize {
    match value {
        true => runtimeOnly(),
        _ => 0,
    }
}

fn main() i32 { 0 }
"#,
    );

    let program = check_program(root.join("main.nia").to_string_lossy().into_owned());
    assert!(
        program.diagnostics.iter().any(|diagnostic| diagnostic
            .diagnostic
            .summary
            .contains("type mismatch in match pattern")),
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
fn unused_const_function_rejects_incompatible_match_arm_types() {
    let root = temp_dir("unused_const_function_rejects_incompatible_match_arm_types");
    write(
        &root.join("main.nia"),
        r#"
const fn wrongSwitch(value: usize) usize {
    match value {
        0usize => 1usize,
        _ => false,
    }
}
fn main() i32 { 0 }
"#,
    );

    let program = check_program(root.join("main.nia").to_string_lossy().into_owned());
    assert!(
        program.diagnostics.iter().any(|diagnostic| diagnostic
            .diagnostic
            .summary
            .contains("type mismatch in match arms")),
        "{:?}",
        program.diagnostics
    );
}

#[test]
fn unused_const_function_checks_match_body_when_pattern_type_is_unknown() {
    let root = temp_dir("unused_const_function_checks_match_body_when_pattern_type_is_unknown");
    write(
        &root.join("main.nia"),
        r#"
fn runtimeOnly() usize {
    1
}

const fn unresolvedSwitch(value: usize) usize {
    match value {
        0 => runtimeOnly(),
        _ => 0,
    }
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
fn unused_const_function_audits_all_array_literal_elements_and_length() {
    let root = temp_dir("unused_const_function_audits_all_array_literal_elements_and_length");
    write(
        &root.join("main.nia"),
        r#"
fn runtimeOnly() usize {
    1
}

const fn wrongArray() usize {
    let values: [usize; 2] = [true, runtimeOnly(), 3usize];
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
            .contains("type mismatch in array literal element")),
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
    assert!(
        program.diagnostics.iter().any(|diagnostic| diagnostic
            .diagnostic
            .summary
            .contains("array literal length mismatch: expected 2, got 3")),
        "{:?}",
        program.diagnostics
    );
}

#[test]
fn unused_const_function_rejects_non_integer_array_repeat_count() {
    let root = temp_dir("unused_const_function_rejects_non_integer_array_repeat_count");
    write(
        &root.join("main.nia"),
        r#"
const fn wrongRepeat() usize {
    let values: [usize; 2] = [1usize; true];
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
            .any(|diagnostic| diagnostic.diagnostic.summary.contains("array repeat count")),
        "{:?}",
        program.diagnostics
    );
}

#[test]
fn unused_const_function_rejects_array_literal_in_non_array_context() {
    let root = temp_dir("unused_const_function_rejects_array_literal_in_non_array_context");
    write(
        &root.join("main.nia"),
        r#"
const fn wrongArrayShape() usize {
    let value: usize = [1usize];
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
            .contains("type mismatch in binding initializer")),
        "{:?}",
        program.diagnostics
    );
}

#[test]
fn unused_generic_const_function_accepts_contextual_array_elements() {
    let root = temp_dir("unused_generic_const_function_accepts_contextual_array_elements");
    write(
        &root.join("main.nia"),
        r#"
const fn copy[T](values: [T; 2]) [T; 2] {
    [values[0usize], values[1usize]]
}

fn main() i32 { 0 }
"#,
    );

    let program = check_program(root.join("main.nia").to_string_lossy().into_owned());
    assert!(program.diagnostics.is_empty(), "{:?}", program.diagnostics);
}

#[test]
fn unused_const_function_rejects_invalid_array_indexes() {
    let root = temp_dir("unused_const_function_rejects_invalid_array_indexes");
    write(
        &root.join("main.nia"),
        r#"
fn runtimeOnly() usize {
    1
}

const fn wrongIndexes() usize {
    let values: [usize; 2] = [1usize, 2usize];
    values[true];
    values[3usize];
    values[-1];
    0usize[runtimeOnly()];
    0
}

fn main() i32 { 0 }
"#,
    );

    let program = check_program(root.join("main.nia").to_string_lossy().into_owned());
    for message in [
        "trait bound not satisfied: [usize; 2]: Index[bool]",
        "trait bound not satisfied: usize: Index[usize]",
        "const expression can only call `const fn`",
    ] {
        assert!(
            program
                .diagnostics
                .iter()
                .any(|diagnostic| diagnostic.diagnostic.summary.contains(message)),
            "missing {message:?}: {:?}",
            program.diagnostics
        );
    }
}

#[test]
fn unused_const_function_rejects_invalid_array_slices() {
    let root = temp_dir("unused_const_function_rejects_invalid_array_slices");
    write(
        &root.join("main.nia"),
        r#"
fn runtimeOnly() usize {
    1
}

const fn wrongSlices(start: usize) usize {
    let values: [usize; 2] = [1usize, 2usize];
    values[true..runtimeOnly()];
    values[-1..1];
    values[0..-1];
    values[start..-1];
    values[2usize..1usize];
    values[1usize..3usize];
    values[0usize..=18446744073709551615usize];
    0usize[0usize..1usize];
    let narrow: [usize; 1] = values[0usize..2usize];
    0
}

fn main() i32 { 0 }
"#,
    );

    let program = check_program(root.join("main.nia").to_string_lossy().into_owned());
    for message in [
        "type mismatch in slice range start: expected usize, got bool",
        "const expression can only call `const fn`",
        "range index expression must be taken as a slice pointer",
        "type mismatch in binding initializer",
    ] {
        assert!(
            program
                .diagnostics
                .iter()
                .any(|diagnostic| diagnostic.diagnostic.summary.contains(message)),
            "missing {message:?}: {:?}",
            program.diagnostics
        );
    }
}

#[test]
fn unused_const_function_accepts_parameter_dependent_index() {
    let root = temp_dir("unused_const_function_accepts_parameter_dependent_index");
    write(
        &root.join("main.nia"),
        r#"
const fn inspect(
    values: [usize; 4],
    index: usize,
) usize {
    values[index]
}

fn main() i32 { 0 }
"#,
    );

    let program = check_program(root.join("main.nia").to_string_lossy().into_owned());
    assert!(program.diagnostics.is_empty(), "{:?}", program.diagnostics);
}

#[test]
fn unused_generic_const_function_defers_trait_based_index_and_slice_types() {
    let root = temp_dir("unused_generic_const_function_defers_trait_based_index_and_slice_types");
    write(
        &root.join("main.nia"),
        r#"
const fn inspectIndex[T](value: T) usize
where T: Index[bool] {
    let selected = value[true];
    0
}

fn main() i32 { 0 }
"#,
    );

    let program = check_program(root.join("main.nia").to_string_lossy().into_owned());
    assert!(program.diagnostics.is_empty(), "{:?}", program.diagnostics);
}

#[test]
fn unused_const_function_audits_all_nominal_struct_fields() {
    let root = temp_dir("unused_const_function_audits_all_nominal_struct_fields");
    write(
        &root.join("main.nia"),
        r#"
struct Point {
    x: usize,
    y: usize,
}

fn runtimeOnly() usize {
    1
}

const fn wrongStruct() usize {
    Point{x: true, z: runtimeOnly()};
    0
}

fn main() i32 { 0 }
"#,
    );

    let program = check_program(root.join("main.nia").to_string_lossy().into_owned());
    for message in [
        "unknown struct field `z`",
        "missing struct field `y`",
        "type mismatch in struct literal field",
        "const expression can only call `const fn`",
    ] {
        assert!(
            program
                .diagnostics
                .iter()
                .any(|diagnostic| diagnostic.diagnostic.summary.contains(message)),
            "missing {message:?}: {:?}",
            program.diagnostics
        );
    }
}

#[test]
fn unused_const_function_audits_duplicate_struct_field_value() {
    let root = temp_dir("unused_const_function_audits_duplicate_struct_field_value");
    write(
        &root.join("main.nia"),
        r#"
fn runtimeOnly() usize {
    1
}

struct Value {
    value: usize,
}

const fn wrongStruct() usize {
    let value = Value {value: 1usize, value: runtimeOnly()};
    0
}

fn main() i32 { 0 }
"#,
    );

    let program = check_program(root.join("main.nia").to_string_lossy().into_owned());
    for message in [
        "duplicate struct field `value`",
        "const expression can only call `const fn`",
    ] {
        assert!(
            program
                .diagnostics
                .iter()
                .any(|diagnostic| diagnostic.diagnostic.summary.contains(message)),
            "missing {message:?}: {:?}",
            program.diagnostics
        );
    }
}

#[test]
fn unused_const_function_rejects_struct_literal_in_non_struct_context() {
    let root = temp_dir("unused_const_function_rejects_struct_literal_in_non_struct_context");
    write(
        &root.join("main.nia"),
        r#"
const fn wrongStructShape() usize {
    let value = usize {inner: 1usize};
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
            .contains("aggregate literal type is not nominal")),
        "{:?}",
        program.diagnostics
    );
}

#[test]
fn unused_generic_const_function_accepts_nominal_struct_fields() {
    let root = temp_dir("unused_generic_const_function_accepts_nominal_struct_fields");
    write(
        &root.join("main.nia"),
        r#"
struct Box[T] {
    value: T,
}

const fn wrap[T](value: T) Box[T] {
    Box[T] { value }
}

fn main() i32 { 0 }
"#,
    );

    let program = check_program(root.join("main.nia").to_string_lossy().into_owned());
    assert!(program.diagnostics.is_empty(), "{:?}", program.diagnostics);
}

#[test]
fn unused_const_function_audits_all_enum_payload_values() {
    let root = temp_dir("unused_const_function_audits_all_enum_payload_values");
    write(
        &root.join("main.nia"),
        r#"
enum Event {
    Closed,
    Data(usize, usize),
    Resize { width: usize, height: usize },
}

fn runtimeOnly() usize {
    1
}

const fn wrongEnums() usize {
    let a: Event = Event::Data(true, runtimeOnly(), 3usize);
    let b: Event = Event::Closed(runtimeOnly());
    let c: Event = Event::Resize { width: true, extra: runtimeOnly() };
    let d: Event = Event::Data { value: runtimeOnly() };
    let e: Event = Event::Resize {
        width: 1usize,
        width: runtimeOnly(),
        height: 2usize,
    };
    0
}

fn main() i32 { 0 }
"#,
    );

    let program = check_program(root.join("main.nia").to_string_lossy().into_owned());
    for message in [
        "enum variant `Data` expects 2 payload values, found 3",
        "type mismatch in enum variant payload",
        "enum variant `Closed` expects no payload",
        "unknown payload field `extra`",
        "missing payload field `height` for variant `Resize`",
        "enum variant `Data` does not have a named payload",
        "duplicate payload field `width`",
        "const expression can only call `const fn`",
    ] {
        assert!(
            program
                .diagnostics
                .iter()
                .any(|diagnostic| diagnostic.diagnostic.summary.contains(message)),
            "missing {message:?}: {:?}",
            program.diagnostics
        );
    }
}

#[test]
fn unused_const_functions_accept_enum_payload_construction() {
    let root = temp_dir("unused_const_functions_accept_enum_payload_construction");
    write(
        &root.join("main.nia"),
        r#"
enum Event {
    Data(usize, usize),
    Resize { width: usize, height: usize },
}

const fn data(value: usize) Event {
    Event::Data(value, 2usize)
}

const fn resize(width: usize, height: usize) Event {
    Event::Resize { width: width, height: height }
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
fn const_integer_division_and_remainder_reject_zero_divisors() {
    let root = temp_dir("const_integer_division_and_remainder_reject_zero_divisors");
    write(
        &root.join("main.nia"),
        r#"
const divideByZero: i32 = 1 / 0;
const remainderByZero: i32 = 1 % 0;

fn main() i32 { 0 }
"#,
    );

    let program = check_program(root.join("main.nia").to_string_lossy().into_owned());
    assert!(
        program.diagnostics.iter().any(|diagnostic| diagnostic
            .diagnostic
            .summary
            .contains("division by zero in const expression")),
        "{:?}",
        program.diagnostics
    );
    assert!(
        program.diagnostics.iter().any(|diagnostic| diagnostic
            .diagnostic
            .summary
            .contains("remainder by zero in const expression")),
        "{:?}",
        program.diagnostics
    );
}

#[test]
fn const_integer_arithmetic_rejects_narrow_intermediate_overflow() {
    let root = temp_dir("const_integer_arithmetic_rejects_narrow_intermediate_overflow");
    write(
        &root.join("main.nia"),
        r#"
const hiddenAddOverflow: u8 = (255u8 + 1u8) - 1u8;
const hiddenMulOverflow: i32 = (2147483647i32 * 2i32) / 2i32;
const contextualOverflow: u8 = (255 + 1) - 1;

const fn overflowInReturnContext() u8 {
    (255 + 1) - 1
}

const contextualFunctionOverflow: u8 = overflowInReturnContext();
const negationOverflow: i8 = -(-128i8);

const fn compoundAssignmentOverflow() u8 {
    let mut value: u8 = 255u8;
    value += 1u8;
    value
}

struct ByteBox { value: u8 }

const fn compoundFieldOverflow() u8 {
    let mut value: ByteBox = ByteBox { value: 255u8 };
    value.value += 1u8;
    value.value
}

const fn compoundIndexOverflow() u8 {
    let mut values: [u8; 1] = [255u8];
    values[0] += 1u8;
    values[0]
}

const compoundOverflow: u8 = compoundAssignmentOverflow();
const compoundField: u8 = compoundFieldOverflow();
const compoundIndex: u8 = compoundIndexOverflow();

fn main() i32 { 0 }
"#,
    );

    let program = check_program(root.join("main.nia").to_string_lossy().into_owned());
    assert!(
        program.diagnostics.iter().any(|diagnostic| diagnostic
            .diagnostic
            .summary
            .contains("integer overflow in const addition")),
        "{:?}",
        program.diagnostics
    );
    assert!(
        program.diagnostics.iter().any(|diagnostic| diagnostic
            .diagnostic
            .summary
            .contains("integer overflow in const multiplication")),
        "{:?}",
        program.diagnostics
    );
    assert!(
        program
            .diagnostics
            .iter()
            .filter(|diagnostic| diagnostic
                .diagnostic
                .summary
                .contains("integer overflow in const addition"))
            .count()
            >= 6,
        "{:?}",
        program.diagnostics
    );
    assert!(
        program.diagnostics.iter().any(|diagnostic| diagnostic
            .diagnostic
            .summary
            .contains("integer overflow in const negation")),
        "{:?}",
        program.diagnostics
    );
}

#[test]
fn generic_const_integer_arithmetic_uses_substituted_width() {
    let root = temp_dir("generic_const_integer_arithmetic_uses_substituted_width");
    write(
        &root.join("main.nia"),
        r#"
const fn add[T](lhs: T, rhs: T) T
where T: Add[T, Output = T] {
    lhs + rhs
}

const overflow: u8 = add[u8](255u8, 1u8);

fn main() i32 { 0 }
"#,
    );

    let program = check_program(root.join("main.nia").to_string_lossy().into_owned());
    assert!(
        program.diagnostics.iter().any(|diagnostic| diagnostic
            .diagnostic
            .summary
            .contains("integer overflow in const addition")),
        "{:?}",
        program.diagnostics
    );
}

#[test]
fn imported_const_integer_arithmetic_uses_definition_module_types() {
    let root = temp_dir("imported_const_integer_arithmetic_uses_definition_module_types");
    write(
        &root.join("math.nia"),
        r#"
pub const fn add(lhs: u8, rhs: u8) u8 {
    lhs + rhs
}
"#,
    );
    write(
        &root.join("main.nia"),
        r#"
module math;
using entry::math;

const overflow: u8 = math::add(255u8, 1u8);

fn main() i32 { 0 }
"#,
    );

    let program = check_program(root.join("main.nia").to_string_lossy().into_owned());
    assert!(
        program.diagnostics.iter().any(|diagnostic| diagnostic
            .diagnostic
            .summary
            .contains("integer overflow in const addition")),
        "{:?}",
        program.diagnostics
    );
}

#[test]
fn const_integer_shifts_use_concrete_operand_width() {
    let root = temp_dir("const_integer_shifts_use_concrete_operand_width");
    write(
        &root.join("main.nia"),
        r#"
const countOverflow: u8 = 1u8 << 8u8;
const valueOverflow: u8 = (128u8 << 1u8) >> 1u8;
const signedOverflow: i8 = (64i8 << 1i8) >> 1i8;
const contextualOverflow: u8 = (128 << 1) >> 1;

fn main() i32 { 0 }
"#,
    );

    let program = check_program(root.join("main.nia").to_string_lossy().into_owned());
    assert!(
        program.diagnostics.iter().any(|diagnostic| diagnostic
            .diagnostic
            .summary
            .contains("shift count is out of range in const expression")),
        "{:?}",
        program.diagnostics
    );
    assert_eq!(
        program
            .diagnostics
            .iter()
            .filter(|diagnostic| diagnostic
                .diagnostic
                .summary
                .contains("integer overflow in const left shift"))
            .count(),
        3,
        "{:?}",
        program.diagnostics
    );
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
    let mut values: [i32; n] = [1, 2, 3, 4];
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
fn const_trap_is_pruned_by_an_unselected_branch() {
    let root = temp_dir("const_trap_is_pruned_by_an_unselected_branch");
    write(
        &root.join("main.nia"),
        r#"
const fn selected(flag: bool) usize {
    if flag {
        std::builtin::trap();
    }
    4
}

const n: usize = selected(false);

fn main(flag: bool) usize { selected(flag) + n }
"#,
    );

    let program = check_program(root.join("main.nia").to_string_lossy().into_owned());
    assert!(program.diagnostics.is_empty(), "{:?}", program.diagnostics);
}

#[test]
fn const_trap_reports_only_when_evaluated() {
    let root = temp_dir("const_trap_reports_only_when_evaluated");
    write(
        &root.join("main.nia"),
        r#"
const fn selected(flag: bool) usize {
    if flag {
        std::builtin::trap();
    }
    4
}

const n: usize = selected(true);

fn main() usize { n }
"#,
    );

    let program = check_program(root.join("main.nia").to_string_lossy().into_owned());
    assert!(
        program.diagnostics.iter().any(|diagnostic| diagnostic
            .diagnostic
            .summary
            .contains("builtin `trap` reached during const evaluation")),
        "{:?}",
        program.diagnostics
    );
}

#[test]
fn trap_builtin_must_be_called() {
    let root = temp_dir("trap_builtin_must_be_called");
    write(
        &root.join("main.nia"),
        r#"
fn main() () {
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
fn value_arg() () {
    std::builtin::trap(1);
}

fn type_arg() () {
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

#[test]
fn unused_const_function_uses_shared_optional_and_error_union_constructor_checks() {
    let root =
        temp_dir("unused_const_function_uses_shared_optional_and_error_union_constructor_checks");
    write(
        &root.join("main.nia"),
        r#"
fn runtimeOnly() usize {
    1
}

const fn invalidConstructors() () {
    let optional: ?usize = ?true;
    let success: usize!usize = !false;
    let failure: usize!usize = true!;
    let hiddenCall: ?usize = ?runtimeOnly();
}

fn main() i32 { 0 }
"#,
    );

    let program = check_program(root.join("main.nia").to_string_lossy().into_owned());
    for message in [
        "type mismatch in optional value",
        "type mismatch in error-union success value",
        "type mismatch in error-union error value",
        "const expression can only call `const fn`",
    ] {
        assert!(
            program
                .diagnostics
                .iter()
                .any(|diagnostic| diagnostic.diagnostic.summary.contains(message)),
            "missing {message:?}: {:?}",
            program.diagnostics
        );
    }
}

#[test]
fn unused_const_function_accepts_scalar_union_value_operations() {
    let root = temp_dir("unused_const_function_accepts_scalar_union_value_operations");
    write(
        &root.join("main.nia"),
        r#"
union Bits {
    integer: usize,
    flag: bool,
}

const fn inspect() usize {
    let bits = Bits { integer: 1 };
    bits.integer
}

fn main() i32 { 0 }
"#,
    );

    let program = check_program(root.join("main.nia").to_string_lossy().into_owned());
    assert!(program.diagnostics.is_empty(), "{:?}", program.diagnostics);
}

#[test]
fn unused_const_function_accepts_scalar_array_union_fields() {
    let root = temp_dir("unused_const_function_accepts_scalar_array_union_fields");
    write(
        &root.join("main.nia"),
        r#"
union Payload {
    bytes: [u8; 2],
    integer: u16,
}

const fn inspect() u16 {
    let payload = Payload { integer: 1 };
    payload.integer
}

fn main() i32 { 0 }
"#,
    );

    let program = check_program(root.join("main.nia").to_string_lossy().into_owned());
    assert!(program.diagnostics.is_empty(), "{:?}", program.diagnostics);
}

#[test]
fn unused_const_function_accepts_nested_pointer_fields_with_relocations() {
    let root = temp_dir("unused_const_function_accepts_nested_pointer_fields_with_relocations");
    write(
        &root.join("main.nia"),
        r#"
struct Header {
    value: &u16,
}

union Payload {
    header: Header,
    integer: u16,
}

const fn inspect() u16 {
    let payload = Payload { integer: 1 };
    payload.integer
}

fn main() i32 { 0 }
"#,
    );

    let program = check_program(root.join("main.nia").to_string_lossy().into_owned());
    assert!(program.diagnostics.is_empty(), "{:?}", program.diagnostics);
}

#[test]
fn unused_const_function_uses_shared_propagation_checks() {
    let root = temp_dir("unused_const_function_uses_shared_propagation_checks");
    write(
        &root.join("main.nia"),
        r#"
const fn invalidOptional(value: ?usize) usize {
    value.?
}

const fn invalidError(value: usize!usize) ?usize {
    ?value.?
}

const fn invalidOperand(value: usize) usize {
    value.?
}

fn main() i32 { 0 }
"#,
    );

    let program = check_program(root.join("main.nia").to_string_lossy().into_owned());
    for message in [
        "optional propagation requires an optional function return type",
        "error propagation requires an error union function return type",
        "`.?` requires optional or error union operand",
    ] {
        assert!(
            program
                .diagnostics
                .iter()
                .any(|diagnostic| diagnostic.diagnostic.summary.contains(message)),
            "missing {message:?}: {:?}",
            program.diagnostics
        );
    }
}

#[test]
fn unused_const_function_audits_invalid_recursive_patterns_and_arm_calls() {
    let root = temp_dir("unused_const_function_audits_invalid_recursive_patterns_and_arm_calls");
    write(
        &root.join("main.nia"),
        r#"
fn runtimeOnly() usize {
    1
}

const fn invalidPatterns(value: usize) usize {
    match value {
        ?payload => runtimeOnly(),
        !payload => runtimeOnly(),
        error! => runtimeOnly(),
        _ => 0,
    }
}

fn main() i32 { 0 }
"#,
    );

    let program = check_program(root.join("main.nia").to_string_lossy().into_owned());
    for message in [
        "requires an optional target",
        "requires an error union target",
        "const expression can only call `const fn`",
    ] {
        assert!(
            program
                .diagnostics
                .iter()
                .any(|diagnostic| diagnostic.diagnostic.summary.contains(message)),
            "missing {message:?}: {:?}",
            program.diagnostics
        );
    }
}

#[test]
fn unused_const_function_rejects_runtime_into_error_witness() {
    let root = temp_dir("unused_const_function_rejects_runtime_into_error_witness");
    write(
        &root.join("main.nia"),
        r#"
trait IntoError[Target] {
    fn intoError(self) Target;
}

struct SourceError {}
struct TargetError {}

extend SourceError : IntoError[TargetError] {
    fn intoError(self) TargetError {
        {}
    }
}

const fn propagate(value: SourceError!usize) TargetError!usize {
    !(value.?)
}

fn main() i32 { 0 }
"#,
    );

    let program = check_program(root.join("main.nia").to_string_lossy().into_owned());
    assert!(
        program.diagnostics.iter().any(|diagnostic| diagnostic
            .diagnostic
            .summary
            .contains("requires `intoError` to be declared `const fn`")),
        "{:?}",
        program.diagnostics
    );
}
