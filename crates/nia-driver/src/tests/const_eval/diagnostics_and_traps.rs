// SPDX-License-Identifier: GPL-3.0-or-later
use super::*;

#[test]
fn const_initializer_type_errors_have_one_owner_and_do_not_cascade() {
    for (name, source) in [
        ("global_bool", "const K: bool = 1; fun main() bool { K }"),
        ("local_bool", "fun main() bool { const K: bool = 1; K }"),
        ("global_float", "const K: i32 = 1.5; fun main() i32 { K }"),
        ("local_float", "fun main() i32 { const K: i32 = 1.5; K }"),
        (
            "global_enum",
            "enum E { A } enum F { A } const K: E = F::A; fun main() E { K }",
        ),
        (
            "local_enum",
            "enum E { A } enum F { A } fun main() E { const K: E = F::A; K }",
        ),
        ("local_range", "fun main() u32 { const K: u32 = -1; K }"),
        ("global_range", "const K: u32 = -1; fun main() u32 { K }"),
        (
            "local_signed_range",
            "fun main() i8 { const K: i8 = -129; K }",
        ),
        (
            "global_signed_range",
            "const K: i8 = -129; fun main() i8 { K }",
        ),
        (
            "global_string_length",
            "const K: [char; 3] = \"ab\"; fun main() usize { K.len() }",
        ),
        (
            "local_string_length",
            "fun main() usize { const K: [char; 3] = \"ab\"; K.len() }",
        ),
        (
            "global_call_return",
            "const fun f() bool { true } const K: u8 = f(); fun main() u8 { K }",
        ),
        (
            "local_call_return",
            "const fun f() bool { true } fun main() u8 { const K: u8 = f(); K }",
        ),
    ] {
        let root = temp_dir(name);
        let path = root.join("main.nia");
        write(&path, source);
        for (diagnostics, suppressed_downstream) in [
            {
                let checked = check_program(path.to_string_lossy().into_owned());
                (checked.diagnostics, checked.suppressed_downstream)
            },
            {
                let codegen = codegen_program(path.to_string_lossy().into_owned());
                (codegen.diagnostics, codegen.suppressed_downstream)
            },
        ] {
            if name == "global_bool" || name == "local_bool" {
                assert_eq!(suppressed_downstream, 0, "{name}");
            }
            let errors = diagnostics
                .iter()
                .filter(|d| d.diagnostic.severity() == nia_diagnostic::Severity::Error)
                .collect::<Vec<_>>();
            assert_eq!(errors.len(), 1, "{name}: {diagnostics:?}");
            assert_eq!(
                errors[0].diagnostic.code.as_str(),
                "E0301",
                "{name}: {diagnostics:?}"
            );
        }
    }
}

#[test]
fn const_initializer_type_error_keeps_an_independent_evaluation_error() {
    let root = temp_dir("const_initializer_type_error_keeps_an_independent_evaluation_error");
    let path = root.join("main.nia");
    write(
        &path,
        "const K: bool = 1usize / 0usize; fun main() bool { K }",
    );

    for diagnostics in [
        check_program(path.to_string_lossy().into_owned()).diagnostics,
        codegen_program(path.to_string_lossy().into_owned()).diagnostics,
    ] {
        let errors = diagnostics
            .iter()
            .filter(|diagnostic| {
                diagnostic.diagnostic.severity() == nia_diagnostic::Severity::Error
            })
            .collect::<Vec<_>>();
        assert_eq!(errors.len(), 2, "{diagnostics:?}");
        assert!(
            errors
                .iter()
                .any(|error| error.diagnostic.summary.contains("type mismatch")),
            "{diagnostics:?}"
        );
        assert!(
            errors
                .iter()
                .any(|error| error.diagnostic.summary.contains("division by zero")),
            "{diagnostics:?}"
        );
    }
}

#[test]
fn const_type_failure_keeps_independent_evaluation_errors() {
    let root = temp_dir("const_type_failure_keeps_independent_evaluation_errors");
    let path = root.join("main.nia");
    write(
        &path,
        "const K: bool = 1; const BAD: usize = 1usize / 0usize; fun main() bool { K }",
    );
    let program = check_program(path.to_string_lossy().into_owned());
    assert!(
        program
            .diagnostics
            .iter()
            .any(|d| d.diagnostic.summary.contains("type mismatch")),
        "{:?}",
        program.diagnostics
    );
    assert!(
        program
            .diagnostics
            .iter()
            .any(|d| d.diagnostic.summary.contains("division by zero")),
        "{:?}",
        program.diagnostics
    );
}

#[test]
fn unused_const_function_rejects_wrong_return_type() {
    let root = temp_dir("unused_const_function_rejects_wrong_return_type");
    write(
        &root.join("main.nia"),
        r#"
const fun wrongReturn() usize {
    true
}

fun main() i32 { 0 }
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
fun runtimeOnly() usize {
    1
}

const fun wrongCall() usize {
    runtimeOnly();
    2
}

fun main() i32 { 0 }
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
                .contains("const expression can only call `const fun`"))
            .count(),
        1,
        "{:?}",
        program.diagnostics
    );
}

#[test]
fn unused_const_function_rejects_runtime_only_atomic_builtin() {
    let root = temp_dir("unused_const_function_rejects_runtime_only_atomic_builtin");
    write(
        &root.join("main.nia"),
        r#"
const fun wrongBuiltin() u32 {
    let value: u32 = 1;
    std::builtin::atomicLoad[u32](&value, 0usize)
}

fun main() i32 { 0 }
"#,
    );

    let program = check_program(root.join("main.nia").to_string_lossy().into_owned());
    assert!(
        program.diagnostics.iter().any(|diagnostic| diagnostic
            .diagnostic
            .summary
            .contains("builtin `atomicLoad` is not available during const evaluation")),
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
const fun increment(value: usize) usize {
    value + 1
}

const fun storesFunctionPointer() usize {
    let callback = & increment;
    1
}

const fun callsFunctionPointer() usize {
    let callback = & increment;
    callback(1)
}

fun main() i32 { 0 }
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
                .contains("callable values are not available during const evaluation"))
            .count(),
        2,
        "{:?}",
        program.diagnostics
    );
    assert_eq!(program.diagnostics.len(), 2, "{:?}", program.diagnostics);
}

#[test]
fn unused_const_function_rejects_bare_callable_values() {
    let root = temp_dir("unused_const_function_rejects_bare_callable_values");
    write(
        &root.join("main.nia"),
        r#"
fun increment(value: usize) usize {
    value + 1
}


struct Point {
    value: usize,
}

extend Point {
    fun get(& self) usize {
        self.value
    }
}

const fun storesBareFunction() usize {
    let callback = increment;
    1
}

const fun storesBareMethod() usize {
    let callback = Point::get;
    1
}

const fun storesClosure() usize {
    let callback = \value: usize -> value + 1;
    1
}

fun main() i32 { 0 }
"#,
    );

    let program = check_program(root.join("main.nia").to_string_lossy().into_owned());
    assert_eq!(program.diagnostics.len(), 3, "{:#?}", program.diagnostics);
    assert!(
        program
            .diagnostics
            .iter()
            .all(|diagnostic| diagnostic.diagnostic.summary
                == "callable values are not available during const evaluation"),
        "{:#?}",
        program.diagnostics
    );
}

#[test]
fn const_callable_capability_does_not_abort_independent_semantic_checks() {
    let root = temp_dir("const_callable_capability_does_not_abort_independent_semantic_checks");
    write(
        &root.join("main.nia"),
        r#"
module callback;
using root::callback;
const fun inspect() usize {
    let pointer: &fun(usize) usize = callback::increment;
    let view: &Fun(usize) usize = callback::increment;
    let closure: &Fun(usize) usize = \value -> value + 1;
    if false {
        let base = 1usize; let captured = \[base] value: usize -> base + value;
        _ = callback::runtimeOnly();
    }
    true
}
fun main() i32 { 0 }
"#,
    );
    write(
        &root.join("callback.nia"),
        r#"
pub const fun increment(value: usize) usize { value + 1 }
pub fun runtimeOnly() usize { 0 }
"#,
    );
    let program = check_program(root.join("main.nia").to_string_lossy().into_owned());
    assert_eq!(program.diagnostics.len(), 6, "{:#?}", program.diagnostics);
    assert_eq!(
        program
            .diagnostics
            .iter()
            .filter(|entry| entry.diagnostic.summary
                == "callable values are not available during const evaluation")
            .count(),
        4,
        "{:#?}",
        program.diagnostics
    );
    assert!(program.diagnostics.iter().any(|entry| {
        entry
            .diagnostic
            .summary
            .contains("const expression can only call `const fun`")
    }));
    assert!(program.diagnostics.iter().any(|entry| {
        entry
            .diagnostic
            .summary
            .contains("type mismatch in function body")
    }));
}

#[test]
fn const_global_callable_values_use_the_shared_capability() {
    let cases = [
        "const callback = increment;",
        "const callback: &fun(usize) usize = increment;",
        "const callback: &fun(usize) usize = &increment;",
        "const callback: &Fun(usize) usize = increment;",
        "const callback = Point::get;",
        "const callback = &Point::get;",
        "const callback: &fun(usize) Event = Event::Data;",
        "const callback: &fun(usize) Event = &Event::Data;",
        "const callback: &fun(usize) Event = .Data;",
        "const callback: &fun(usize) Event = &.Data;",
        "const callback: &Fun(usize) Event = Event::Data;",
        "const callbacks: [&fun(usize) usize; 1] = [increment];",
        "const callbacks: (&fun(usize) usize, bool) = (increment, true);",
    ];
    for (index, case) in cases.iter().enumerate() {
        let root = temp_dir(&format!("const_global_callable_values_{index}"));
        write(
            &root.join("main.nia"),
            &format!(
                r#"
const fun increment(value: usize) usize {{ value + 1 }}
struct Point {{ value: usize }}
extend Point {{ const fun get(& self) usize {{ self.value }} }}
enum Event {{ Empty, Data(usize) }}
{case}
fun main() i32 {{ 0 }}
"#
            ),
        );
        let program = check_program(root.join("main.nia").to_string_lossy().into_owned());
        assert!(!program.diagnostics.is_empty(), "{case}");
        assert!(
            program
                .diagnostics
                .iter()
                .all(|entry| entry.diagnostic.summary
                    == "callable values are not available during const evaluation"),
            "{case}: {:#?}",
            program.diagnostics
        );
    }
}

#[test]
fn unused_const_function_rejects_runtime_call_in_unselected_branch() {
    let root = temp_dir("unused_const_function_rejects_runtime_call_in_unselected_branch");
    write(
        &root.join("main.nia"),
        r#"
fun runtimeOnly() usize {
    1
}

const fun wrongCall() usize {
    if false {
        return runtimeOnly();
    }
    2
}

fun main() i32 { 0 }
"#,
    );

    let program = check_program(root.join("main.nia").to_string_lossy().into_owned());
    assert!(
        program.diagnostics.iter().any(|diagnostic| diagnostic
            .diagnostic
            .summary
            .contains("const expression can only call `const fun`")),
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
    fun runtimeOnly(self) usize {
        self.inner
    }
}

const fun wrongCall() usize {
    let mut result: usize = 0;
    result = Value{inner: 1}.runtimeOnly();
    result
}

fun main() i32 { 0 }
"#,
    );

    let program = check_program(root.join("main.nia").to_string_lossy().into_owned());
    assert!(
        program.diagnostics.iter().any(|diagnostic| diagnostic
            .diagnostic
            .summary
            .contains("const expression can only call `const fun`")),
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
const fun wrongLocal() usize {
    let value: usize = true;
    0
}

fun main() i32 { 0 }
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
const fun wrongAssignment() usize {
    let value: usize = 1;
    value = 2;
    value
}

fun main() i32 { 0 }
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
const fun wrongAssignment() usize {
    let mut value: usize = 1;
    value = true;
    value
}

fun main() i32 { 0 }
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
const fun wrongAssignment(index: usize) usize {
    let mut value: usize = 1;
    value[index] = 2;
    value
}

fun main() i32 { 0 }
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
const fun wrongAssignment(values: &[usize]) usize {
    let mut view: &[usize] = values;
    view[0] = 2;
    view[0]
}

fun main() i32 { 0 }
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
const fun update(index: usize) usize {
    let mut values: [usize; 2] = [1, 2];
    values[index] += 1;
    values[0]
}

fun main() i32 { 0 }
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
const fun replace[T](value: T, replacement: T) T {
    let mut result: T = value;
    result = replacement;
    result
}

fun main() i32 { 0 }
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
const fun wrongAssignment() usize {
    let mut value: bool = true;
    value += false;
    0
}

fun main() i32 { 0 }
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
const fun wrongTail() usize {
    let mut value: usize = 1;
    value = 2
}

fun main() i32 { 0 }
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
fun runtimeOnly() usize {
    1
}

const fun wrongCondition() usize {
    if 1usize {
        return runtimeOnly();
    }
    while 2usize {}
    0
}

fun main() i32 { 0 }
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
}

#[test]
fn unused_const_function_rejects_invalid_builtin_unary_operand() {
    let root = temp_dir("unused_const_function_rejects_invalid_builtin_unary_operand");
    write(
        &root.join("main.nia"),
        r#"
const fun wrongNot() bool {
    not 1usize
}

fun main() i32 { 0 }
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
const fun wrongAdd() usize {
    true + false;
    0
}

const fun wrongEquality() bool {
    1usize == true
}

fun main() i32 { 0 }
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
const fun wrongBranches() usize {
    if true { 1usize } else { false }
}

fun main() i32 { 0 }
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
const fun add[T](lhs: T, rhs: T) T
where T: Add[T, Output = T] {
    lhs + rhs
}

fun main() i32 { 0 }
"#,
    );

    let program = check_program(root.join("main.nia").to_string_lossy().into_owned());
    assert!(program.diagnostics.is_empty(), "{:?}", program.diagnostics);
}

#[test]
fn const_call_rejects_unsatisfied_generic_where_bound() {
    let root = temp_dir("const_call_rejects_unsatisfied_generic_where_bound");
    write(
        &root.join("main.nia"),
        r#"
trait Marker {}

const fun constrained[T]() usize
where T: Marker {
    7usize
}

const width: usize = constrained[i32]();

fun main() i32 { width as i32 }
"#,
    );

    let program = check_program(root.join("main.nia").to_string_lossy().into_owned());
    assert!(
        program.diagnostics.iter().any(|diagnostic| diagnostic
            .diagnostic
            .summary
            .contains("trait bound not satisfied for const function call")),
        "{:?}",
        program.diagnostics
    );
    assert!(
        program.diagnostics.iter().any(|diagnostic| {
            let summary = &diagnostic.diagnostic.summary;
            summary.contains("const function call")
                && summary.contains("visible implementation of `Marker` for `i32`")
        }),
        "{:?}",
        program.diagnostics
    );
}

#[test]
fn const_associated_obligation_retains_member_and_type_evidence() {
    let root = temp_dir("const_associated_obligation_retains_member_and_type_evidence");
    write(
        &root.join("main.nia"),
        r#"
trait Source { type Item; }
struct Wrong {}
extend Wrong : Source { type Item = bool; }

const fun constrained[T]() usize
where T: Source[Item = i32] {
    7usize
}

const width: usize = constrained[Wrong]();
fun main() i32 { width as i32 }
"#,
    );
    let program = check_program(root.join("main.nia").to_string_lossy().into_owned());
    assert!(
        program.diagnostics.iter().any(|diagnostic| {
            let summary = &diagnostic.diagnostic.summary;
            summary.contains("const function call")
                && summary.contains("Source::Item")
                && summary.contains("`bool`")
                && summary.contains("requires `i32`")
        }),
        "{:?}",
        program.diagnostics
    );
}

#[test]
fn const_call_forwards_enclosing_generic_where_bound() {
    let root = temp_dir("const_call_forwards_enclosing_generic_where_bound");
    write(
        &root.join("main.nia"),
        r#"
trait Marker {}

const fun constrained[T]() usize
where T: Marker {
    7usize
}

const fun forwarding[T]() usize
where T: Marker {
    constrained[T]()
}

fun main() i32 { 0 }
"#,
    );

    let program = check_program(root.join("main.nia").to_string_lossy().into_owned());
    assert_no_error_diagnostics(&program.diagnostics);
}

#[test]
fn imported_const_call_rejects_unsatisfied_generic_where_bound() {
    let root = temp_dir("imported_const_call_rejects_unsatisfied_generic_where_bound");
    write(
        &root.join("main.nia"),
        r#"
module api;
using root::api;

const width: usize = api::constrained[i32]();

fun main() i32 { width as i32 }
"#,
    );
    write(
        &root.join("api.nia"),
        r#"
pub trait Marker {}

pub const fun constrained[T]() usize
where T: Marker {
    7usize
}
"#,
    );

    let program = check_program(root.join("main.nia").to_string_lossy().into_owned());
    assert!(
        program.diagnostics.iter().any(|diagnostic| diagnostic
            .diagnostic
            .summary
            .contains("trait bound not satisfied for const function call")),
        "{:?}",
        program.diagnostics
    );
}

#[test]
fn unused_const_function_rejects_match_pattern_and_checks_arm_body() {
    let root = temp_dir("unused_const_function_rejects_match_pattern_and_checks_arm_body");
    write(
        &root.join("main.nia"),
        r#"
fun runtimeOnly() usize {
    1
}

const fun wrongSwitch(value: usize) usize {
    match value {
        true => runtimeOnly(),
        _ => 0,
    }
}

fun main() i32 { 0 }
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
}

#[test]
fn unused_const_function_rejects_incompatible_match_arm_types() {
    let root = temp_dir("unused_const_function_rejects_incompatible_match_arm_types");
    write(
        &root.join("main.nia"),
        r#"
const fun wrongSwitch(value: usize) usize {
    match value {
        0usize => 1usize,
        _ => false,
    }
}
fun main() i32 { 0 }
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
fun runtimeOnly() usize {
    1
}

const fun unresolvedSwitch(value: usize) usize {
    match value {
        0 => runtimeOnly(),
        _ => 0,
    }
}

fun main() i32 { 0 }
"#,
    );

    let program = check_program(root.join("main.nia").to_string_lossy().into_owned());
    assert!(
        program.diagnostics.iter().any(|diagnostic| diagnostic
            .diagnostic
            .summary
            .contains("const expression can only call `const fun`")),
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
fun runtimeOnly() usize {
    1
}

const fun wrongArray() usize {
    let values: [usize; 2] = [true, runtimeOnly(), 3usize];
    0
}

fun main() i32 { 0 }
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
const fun wrongRepeat() usize {
    let values: [usize; 2] = [1usize; true];
    0
}

fun main() i32 { 0 }
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
const fun wrongArrayShape() usize {
    let value: usize = [1usize];
    0
}

fun main() i32 { 0 }
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
const fun copy[T](values: [T; 2]) [T; 2] {
    [values[0usize], values[1usize]]
}

fun main() i32 { 0 }
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
fun runtimeOnly() usize {
    1
}

const fun wrongIndexes() usize {
    let values: [usize; 2] = [1usize, 2usize];
    values[true];
    values[3usize];
    values[-1];
    0usize[runtimeOnly()];
    0
}

fun main() i32 { 0 }
"#,
    );

    let program = check_program(root.join("main.nia").to_string_lossy().into_owned());
    for message in [
        "trait bound not satisfied: [usize; 2]: Index[bool]",
        "trait bound not satisfied: usize: Index[usize]",
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
fun runtimeOnly() usize {
    1
}

const fun wrongSlices(start: usize) usize {
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

fun main() i32 { 0 }
"#,
    );

    let program = check_program(root.join("main.nia").to_string_lossy().into_owned());
    for message in [
        "type mismatch in slice range start: expected usize, got bool",
        "range index expression must be taken as a slice pointer",
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
const fun inspect(
    values: [usize; 4],
    index: usize,
) usize {
    values[index]
}

fun main() i32 { 0 }
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
const fun inspectIndex[T](value: T) usize
where T: Index[bool] {
    let selected = value[true];
    0
}

fun main() i32 { 0 }
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

fun runtimeOnly() usize {
    1
}

const fun wrongStruct() usize {
    Point{x: true, z: runtimeOnly()};
    0
}

fun main() i32 { 0 }
"#,
    );

    let program = check_program(root.join("main.nia").to_string_lossy().into_owned());
    for message in [
        "unknown struct field `z`",
        "missing struct field `y`",
        "type mismatch in struct literal field",
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
fun runtimeOnly() usize {
    1
}

struct Value {
    value: usize,
}

const fun wrongStruct() usize {
    let value = Value {value: 1usize, value: runtimeOnly()};
    0
}

fun main() i32 { 0 }
"#,
    );

    let program = check_program(root.join("main.nia").to_string_lossy().into_owned());
    for message in [
        "duplicate struct field `value`",
        "const expression can only call `const fun`",
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
const fun wrongStructShape() usize {
    let value = usize {inner: 1usize};
    0
}

fun main() i32 { 0 }
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

const fun wrap[T](value: T) Box[T] {
    Box[T] { value }
}

fun main() i32 { 0 }
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

fun runtimeOnly() usize {
    1
}

const fun wrongEnums() usize {
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

fun main() i32 { 0 }
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

const fun data(value: usize) Event {
    Event::Data(value, 2usize)
}

const fun resize(width: usize, height: usize) Event {
    Event::Resize { width: width, height: height }
}

fun main() i32 { 0 }
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
const fun recurse(value: usize) usize {
    recurse(value + 1)
}

const result: usize = recurse(0);

fun main() i32 { 0 }
"#,
    );

    let program = check_program(root.join("main.nia").to_string_lossy().into_owned());
    let call_depth_limit = if cfg!(windows) { 128 } else { 256 };
    assert!(
        program
            .diagnostics
            .iter()
            .any(
                |diagnostic| diagnostic.diagnostic.summary.contains(&format!(
                    "const evaluation exceeded the {call_depth_limit} call depth limit"
                ))
            ),
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
const fun countdown(value: usize) usize {
    if value == 0 {
        return 0;
    }
    countdown(value - 1) + 1
}

const result: usize = countdown(32);

fun main() i32 {
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

fun main() i32 { 0 }
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

const fun overflowInReturnContext() u8 {
    (255 + 1) - 1
}

const contextualFunctionOverflow: u8 = overflowInReturnContext();
const negationOverflow: i8 = -(-128i8);

const fun compoundAssignmentOverflow() u8 {
    let mut value: u8 = 255u8;
    value += 1u8;
    value
}

struct ByteBox { value: u8 }

const fun compoundFieldOverflow() u8 {
    let mut value: ByteBox = ByteBox { value: 255u8 };
    value.value += 1u8;
    value.value
}

const fun compoundIndexOverflow() u8 {
    let mut values: [u8; 1] = [255u8];
    values[0] += 1u8;
    values[0]
}

const compoundOverflow: u8 = compoundAssignmentOverflow();
const compoundField: u8 = compoundFieldOverflow();
const compoundIndex: u8 = compoundIndexOverflow();

fun main() i32 { 0 }
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
const fun add[T](lhs: T, rhs: T) T
where T: Add[T, Output = T] {
    lhs + rhs
}

const overflow: u8 = add[u8](255u8, 1u8);

fun main() i32 { 0 }
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
pub const fun add(lhs: u8, rhs: u8) u8 {
    lhs + rhs
}
"#,
    );
    write(
        &root.join("main.nia"),
        r#"
module math;
using root::math;

const overflow: u8 = math::add(255u8, 1u8);

fun main() i32 { 0 }
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
const signed128Overflow: i128 = 1i128 << 127i128;
const unsigned128Overflow: u128 = 340282366920938463463374607431768211455u128 << 1u128;
const signed128Boundary: i128 = -1i128 << 127i128;
const unsigned128Boundary: u128 = 1u128 << 127u128;

fun main() i32 { 0 }
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
        5,
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

fun main() i32 { 0 }
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
fun main() i32 {
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

fun main() i32 { 0 }
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
const fun selected() usize {
    if true {
        return 4usize;
    }
    std::builtin::error("unreachable const branch")
}

const n: usize = selected();

fun main() usize { n }
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

fun main() i32 { 0 }
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
fun main() usize {
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
fun main() i32 {
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
const fun selected(flag: bool) usize {
    if flag {
        std::builtin::trap();
    }
    4
}

const n: usize = selected(false);

fun main(flag: bool) usize { selected(flag) + n }
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
const fun selected(flag: bool) usize {
    if flag {
        std::builtin::trap();
    }
    4
}

const n: usize = selected(true);

fun main() usize { n }
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
fun main() () {
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
fun value_arg() () {
    std::builtin::trap(1);
}

fun type_arg() () {
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
fun runtimeOnly() usize {
    1
}

const fun invalidConstructors() () {
    let optional: ?usize = ?true;
    let success: usize!usize = !false;
    let failure: usize!usize = true!;
    let hiddenCall: ?usize = ?runtimeOnly();
}

fun main() i32 { 0 }
"#,
    );

    let program = check_program(root.join("main.nia").to_string_lossy().into_owned());
    for message in [
        "type mismatch in optional value",
        "type mismatch in error-union success value",
        "type mismatch in error-union error value",
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

const fun inspect() usize {
    let bits = Bits { integer: 1 };
    bits.integer
}

fun main() i32 { 0 }
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

const fun inspect() u16 {
    let payload = Payload { integer: 1 };
    payload.integer
}

fun main() i32 { 0 }
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

const fun inspect() u16 {
    let payload = Payload { integer: 1 };
    payload.integer
}

fun main() i32 { 0 }
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
const fun invalidOptional(value: ?usize) usize {
    value.?
}

const fun invalidError(value: usize!usize) ?usize {
    ?value.?
}

const fun invalidOperand(value: usize) usize {
    value.?
}

fun main() i32 { 0 }
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
fun runtimeOnly() usize {
    1
}

const fun invalidPatterns(value: usize) usize {
    match value {
        ?payload => runtimeOnly(),
        !payload => runtimeOnly(),
        error! => runtimeOnly(),
        _ => 0,
    }
}

fun main() i32 { 0 }
"#,
    );

    let program = check_program(root.join("main.nia").to_string_lossy().into_owned());
    for message in [
        "requires an optional target",
        "requires an error union target",
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
struct SourceError {}
struct TargetError {}

extend SourceError : std::builtin::IntoError[TargetError] {
    fun intoError(self) TargetError {
        {}
    }
}

const fun propagate(value: SourceError!usize) TargetError!usize {
    !(value.?)
}

fun main() i32 { 0 }
"#,
    );

    let program = check_program(root.join("main.nia").to_string_lossy().into_owned());
    assert!(
        program.diagnostics.iter().any(|diagnostic| diagnostic
            .diagnostic
            .summary
            .contains("requires `intoError` to be declared `const fun`")),
        "{:?}",
        program.diagnostics
    );
}

#[test]
fn const_capability_preserves_direct_calls_and_uninhabited_callback_payloads() {
    let root = temp_dir("const_direct_calls_and_empty_callback_payloads");
    write(
        &root.join("main.nia"),
        r#"
const fun increment(value: usize) usize { value + 1 }
struct Point { value: usize }
extend Point { const fun get(& self) usize { self.value } }
enum Event { Empty, Data(usize) }
const absent: ?&fun(usize) usize = null;
const empty: [&fun(usize) usize; 0] = [];
const fun evaluate() usize {
    let absent: ?&Fun(usize) usize = null;
    let empty: [&fun(usize) usize; 0] = [];
    _ = absent;
    _ = empty;
    let point = Point { value: increment(2) };
    match Event::Data(point.get()) {
        .Data(value) => value,
        .Empty => 0,
    }
}
const result: usize = evaluate();
static computed: usize = result;
fun main() i32 { 0 }
"#,
    );
    let program = check_program(root.join("main.nia").to_string_lossy().into_owned());
    assert!(program.diagnostics.is_empty(), "{:#?}", program.diagnostics);
    let codegen = codegen_program(root.join("main.nia").to_string_lossy().into_owned());
    assert!(codegen.diagnostics.is_empty(), "{:#?}", codegen.diagnostics);
    let global = codegen
        .backend_lowering
        .program
        .modules
        .iter()
        .flat_map(|module| &module.globals)
        .find(|global| global.name == sym("computed"))
        .expect("computed global");
    assert_eq!(global.init, Some(StaticInit::Int(IntConst::unsigned(3))));
}

#[test]
fn const_discard_preserves_side_effects_and_control_flow() {
    let root = temp_dir("const_discard_effects");
    write(
        &root.join("main.nia"),
        r#"
const fun effects() i32 {
    let mut count = 1;
    _ = { count += 2; 9 };
    let unit: () = (_ = { count += 4; 10 });
    count
}
const fun exits() i32 {
    _ = { return 17; };
    0
}
const effectsValue: i32 = effects();
const exitsValue: i32 = exits();
static effectsResult: i32 = effectsValue;
static exitsResult: i32 = exitsValue;
fun main() i32 { 0 }
"#,
    );
    let program = codegen_program(root.join("main.nia").to_string_lossy().into_owned());
    assert!(program.diagnostics.is_empty(), "{:#?}", program.diagnostics);
    for (name, expected) in [("effectsResult", 7), ("exitsResult", 17)] {
        let global = program
            .backend_lowering
            .program
            .modules
            .iter()
            .flat_map(|module| &module.globals)
            .find(|global| global.name == sym(name))
            .expect("global");
        assert_eq!(global.init, Some(static_int(expected)));
    }
}

#[test]
fn const_match_patterns_use_imported_call_result_types() {
    let root = temp_dir("const_match_imported_call_results");
    write(
        &root.join("values.nia"),
        r#"
pub enum Event { Empty, Data(usize) }
pub enum Envelope { Event(Event) }
pub const fun make() Event { Event::Data(7) }
pub struct Source {}
extend Source { pub const fun make(& self) Event { Event::Data(11) } }
"#,
    );
    write(
        &root.join("main.nia"),
        r#"
module values;
using root::values;
const fun evaluate() usize {
    let source = values::Source {};
    let first = match values::make() { .Data(value) => value, .Empty => 0, };
    let second = match source.make() { .Data(value) => value, .Empty => 0, };
    let third = match values::Event::Data(13) { .Data(value) => value, .Empty => 0, };
    let nested = values::Envelope::Event(values::Event::Data(17));
    let fourth = match nested {
        .Event(event) => match event { .Data(value) => value, .Empty => 0, },
    };
    first + second + third + fourth
}
const result: usize = evaluate();
static computed: usize = result;
fun main() i32 { 0 }
"#,
    );
    let program = codegen_program(root.join("main.nia").to_string_lossy().into_owned());
    assert!(program.diagnostics.is_empty(), "{:#?}", program.diagnostics);
    let global = program
        .backend_lowering
        .program
        .modules
        .iter()
        .flat_map(|module| &module.globals)
        .find(|global| global.name == sym("computed"))
        .expect("global");
    assert_eq!(global.init, Some(StaticInit::Int(IntConst::unsigned(48))));
}

#[test]
fn const_global_closure_preserves_the_ordinary_owner_diagnostic() {
    let root = temp_dir("const_global_closure_owner");
    write(
        &root.join("main.nia"),
        r#"
const callback = \value: usize -> value + 1;
fun main() i32 { 0 }
"#,
    );
    let program = check_program(root.join("main.nia").to_string_lossy().into_owned());
    let summaries = program
        .diagnostics
        .iter()
        .map(|entry| entry.diagnostic.summary.as_str())
        .collect::<Vec<_>>();
    assert_eq!(summaries.len(), 2, "{summaries:?}");
    for expected in [
        "callable values are not available during const evaluation",
        "closure expressions are only supported inside function bodies",
    ] {
        assert!(summaries.contains(&expected), "{summaries:?}");
    }
}

#[test]
fn const_qualified_aggregates_propagate_imported_field_context() {
    let root = temp_dir("const_qualified_aggregate_context");
    write(
        &root.join("values.nia"),
        r#"
pub struct Cell[T] { value: T }
pub struct Bundle { cell: Cell[usize] }
pub union Slot { value: usize, bytes: [u8; 8] }
"#,
    );
    write(
        &root.join("main.nia"),
        r#"
module values;
using root::values;
const fun evaluate() usize {
    let bundle = values::Bundle { cell: .{ value: 5 } };
    let cell = values::Cell[usize] { value: 7 };
    let slot = values::Slot { value: 11 };
    let bundles = [
        values::Bundle { cell: .{ value: 13 } },
        .{ cell: .{ value: 17 } },
    ];
    let fromLocal = [bundle, .{ cell: .{ value: 19 } }];
    bundle.cell.value + cell.value + slot.value
        + bundles[0].cell.value + bundles[1].cell.value + fromLocal[1].cell.value
}
const result: usize = evaluate();
static computed: usize = result;
fun main() i32 { 0 }
"#,
    );
    let program = codegen_program(root.join("main.nia").to_string_lossy().into_owned());
    assert!(program.diagnostics.is_empty(), "{:#?}", program.diagnostics);
    let global = program
        .backend_lowering
        .program
        .modules
        .iter()
        .flat_map(|module| &module.globals)
        .find(|global| global.name == sym("computed"))
        .expect("computed global");
    assert_eq!(global.init, Some(StaticInit::Int(IntConst::unsigned(72))));
}

/// A constant whose initializer fails is reported once, at its root. Its
/// uses, and constants computed from it, are consequences of that error:
/// they reported "const value is not available during body check" and
/// "failed to evaluate resolved const global" in addition.
#[test]
fn failed_constants_report_only_their_root() {
    let root = temp_dir("failed_constants_report_only_their_root");
    write(
        &root.join("dep.nia"),
        "pub const BAD: usize = std::builtin::error(\"dependency root\");\n",
    );
    for (name, source, expected) in [
        (
            "direct",
            "const Y: usize = std::builtin::error(\"root\");\nfun main() i32 { Y as i32 + Y as i32 }\n",
            "root",
        ),
        (
            "through a function",
            "const fun f() usize { std::builtin::error(\"root\") }\nconst Y: usize = f();\nfun main() i32 { Y as i32 }\n",
            "root",
        ),
        (
            "dependent constant",
            "const A: usize = std::builtin::error(\"root\");\nconst B: usize = A + 1;\nconst C: usize = B * 2;\nfun main() i32 { (B + C) as i32 }\n",
            "root",
        ),
        (
            "local constant",
            "fun main() i32 { const L: usize = std::builtin::error(\"root\"); L as i32 }\n",
            "root",
        ),
        (
            "other module",
            "module dep;\nfun main() i32 { dep::BAD as i32 + dep::BAD as i32 }\n",
            "dependency root",
        ),
    ] {
        let path = root.join("main.nia");
        write(&path, source);
        let program = check_program(path.to_string_lossy().into_owned());
        let errors = program
            .diagnostics
            .iter()
            .filter(|d| d.diagnostic.severity() == nia_diagnostic::Severity::Error)
            .map(|d| d.diagnostic.summary.as_str())
            .collect::<Vec<_>>();
        assert_eq!(errors, [expected], "{name}");
    }
}

/// A constant whose declared type, or whose enum constructor, is already
/// reported as wrong is not lowered against it. Its initializer and its uses
/// added "unsupported const expression" and "const value is not available
/// during body check" to the root error.
#[test]
fn constants_of_erroneous_types_report_only_their_root() {
    let root = temp_dir("constants_of_erroneous_types_report_only_their_root");
    for (name, source, expected) in [
        (
            "generic alias without arguments",
            "enum E[T] { A(T), B }\ntype P[T] = E[T];\nconst X: P = .B;\nfun main() i32 { match X { .A(_) => 1, .B => 0 } }\n",
            "generic argument count mismatch for `P`: expected 1, got 0",
        ),
        (
            "generic constructor without arguments",
            "enum E[T] { A(T), B }\nconst M: i32 = match E::A(5i32) { .A(v) => v, .B => 0 };\nfun main() i32 { M }\n",
            "generic argument count mismatch for `E`: expected 1, got 0",
        ),
        (
            "untyped unit constant of such a constructor",
            "enum E[T] { A(T), B }
const K = E::B;
const M: i32 = match K { .A(_) => 1, .B => 0 };
fun main() i32 { M }
",
            "generic argument count mismatch for `E`: expected 1, got 0",
        ),
        (
            "untyped constant of such a constructor",
            "enum E[T] { A(T), B }\nconst K = E::A(5i32);\nconst M: i32 = match K { .A(v) => v, .B => 0 };\nfun main() i32 { M }\n",
            "generic argument count mismatch for `E`: expected 1, got 0",
        ),
    ] {
        let path = root.join("main.nia");
        write(&path, source);
        let program = check_program(path.to_string_lossy().into_owned());
        let errors = program
            .diagnostics
            .iter()
            .filter(|d| d.diagnostic.severity() == nia_diagnostic::Severity::Error)
            .map(|d| d.diagnostic.summary.as_str())
            .collect::<Vec<_>>();
        assert_eq!(errors, [expected], "{name}");
    }
}
