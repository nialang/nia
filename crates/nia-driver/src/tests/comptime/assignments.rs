// SPDX-License-Identifier: GPL-3.0-or-later
use super::*;

#[test]
fn comptime_function_rejects_assignment_to_immutable_local() {
    let root = temp_dir("comptime_function_rejects_assignment_to_immutable_local");
    write(
        &root.join("main.nia"),
        r#"
comptime fn width() usize {
    let i: usize = 2;
    i = 5;
    i
}

comptime n: usize = width();

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
            .contains("cannot assign to immutable comptime local `i`")),
        "{:?}",
        program.diagnostics
    );
}

#[test]
fn comptime_function_rejects_assignment_value_type_mismatch() {
    let root = temp_dir("comptime_function_rejects_assignment_value_type_mismatch");
    write(
        &root.join("main.nia"),
        r#"
comptime fn width() usize {
    let mut i: usize = 2;
    i = true;
    i
}

comptime n: usize = width();
"#,
    );

    let program = check_program(root.join("main.nia").to_string_lossy().into_owned());
    assert!(
        program.diagnostics.iter().any(|diagnostic| diagnostic
            .diagnostic
            .summary
            .contains("primitive type usize")),
        "{:?}",
        program.diagnostics
    );
}

#[test]
fn comptime_function_rejects_int_assignment_to_bool() {
    let root = temp_dir("comptime_function_rejects_int_assignment_to_bool");
    write(
        &root.join("main.nia"),
        r#"
comptime fn width() usize {
    let mut value: bool = true;
    value = 1usize;
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
fn comptime_function_rejects_array_assignment_shape_mismatch() {
    let root = temp_dir("comptime_function_rejects_array_assignment_shape_mismatch");
    write(
        &root.join("main.nia"),
        r#"
comptime fn width() usize {
    let mut values: [2]usize = [1usize, 2usize];
    values = true;
    values.len()
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
fn comptime_function_rejects_array_assignment_length_mismatch() {
    let root = temp_dir("comptime_function_rejects_array_assignment_length_mismatch");
    write(
        &root.join("main.nia"),
        r#"
comptime fn width() usize {
    let mut values: [2]usize = [1usize, 2usize];
    values = [1usize; 3usize];
    values.len()
}

comptime n: usize = width();
"#,
    );

    let program = check_program(root.join("main.nia").to_string_lossy().into_owned());
    assert!(
        program
            .diagnostics
            .iter()
            .any(|diagnostic| diagnostic.diagnostic.summary.contains("expected length 2")),
        "{:?}",
        program.diagnostics
    );
}

#[test]
fn comptime_function_rejects_optional_assignment_shape_mismatch() {
    let root = temp_dir("comptime_function_rejects_optional_assignment_shape_mismatch");
    write(
        &root.join("main.nia"),
        r#"
comptime fn width() usize {
    let mut value: ?usize = ?1usize;
    value = true;
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
            .contains("expected optional type")),
        "{:?}",
        program.diagnostics
    );
}

#[test]
fn comptime_function_rejects_error_union_assignment_shape_mismatch() {
    let root = temp_dir("comptime_function_rejects_error_union_assignment_shape_mismatch");
    write(
        &root.join("main.nia"),
        r#"
comptime fn width() usize {
    let mut value: usize!usize = !1usize;
    value = true;
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
            .contains("expected error union type")),
        "{:?}",
        program.diagnostics
    );
}

#[test]
fn comptime_function_rejects_struct_assignment_missing_field() {
    let root = temp_dir("comptime_function_rejects_struct_assignment_missing_field");
    write(
        &root.join("main.nia"),
        r#"
comptime fn width() usize {
    let mut config = {width: 4usize, enabled: true};
    config = {width: 8usize};
    config.width
}

comptime n: usize = width();
"#,
    );

    let program = check_program(root.join("main.nia").to_string_lossy().into_owned());
    assert!(
        program.diagnostics.iter().any(|diagnostic| {
            diagnostic
                .diagnostic
                .summary
                .contains("comptime struct value is missing field `enabled`")
        }),
        "{:?}",
        program.diagnostics
    );
}

#[test]
fn comptime_function_rejects_nominal_struct_assignment_missing_field() {
    let root = temp_dir("comptime_function_rejects_nominal_struct_assignment_missing_field");
    write(
        &root.join("main.nia"),
        r#"
struct Point {
    x: usize,
    y: usize,
}

comptime fn width() usize {
    let mut p: Point = Point{x: 1usize, y: 2usize};
    p = Point{x: 3usize};
    p.x
}

comptime n: usize = width();
"#,
    );

    let program = check_program(root.join("main.nia").to_string_lossy().into_owned());
    assert!(
        program.diagnostics.iter().any(|diagnostic| {
            diagnostic
                .diagnostic
                .summary
                .contains("comptime struct value is missing field `y`")
        }),
        "{:?}",
        program.diagnostics
    );
}

#[test]
fn comptime_function_rejects_struct_assignment_extra_field() {
    let root = temp_dir("comptime_function_rejects_struct_assignment_extra_field");
    write(
        &root.join("main.nia"),
        r#"
comptime fn width() usize {
    let mut config = {width: 4usize};
    config = {width: 8usize, enabled: true};
    config.width
}

comptime n: usize = width();
"#,
    );

    let program = check_program(root.join("main.nia").to_string_lossy().into_owned());
    assert!(
        program.diagnostics.iter().any(|diagnostic| {
            diagnostic
                .diagnostic
                .summary
                .contains("comptime struct value has extra field `enabled`")
        }),
        "{:?}",
        program.diagnostics
    );
}

#[test]
fn comptime_function_rejects_nominal_struct_assignment_extra_field() {
    let root = temp_dir("comptime_function_rejects_nominal_struct_assignment_extra_field");
    write(
        &root.join("main.nia"),
        r#"
struct Point {
    x: usize,
    y: usize,
}

comptime fn width() usize {
    let mut p: Point = Point{x: 1usize, y: 2usize};
    p = Point{x: 3usize, y: 4usize, z: 5usize};
    p.x
}

comptime n: usize = width();
"#,
    );

    let program = check_program(root.join("main.nia").to_string_lossy().into_owned());
    assert!(
        program.diagnostics.iter().any(|diagnostic| {
            diagnostic
                .diagnostic
                .summary
                .contains("comptime struct value has extra field `z`")
        }),
        "{:?}",
        program.diagnostics
    );
}

#[test]
fn comptime_function_mutates_struct_fields() {
    let root = temp_dir("comptime_function_mutates_struct_fields");
    write(
        &root.join("main.nia"),
        r#"
struct Point {
    x: usize,
    y: usize,
}

comptime fn width() usize {
    let mut p: Point = Point{x: 2, y: 3};
    p.x += 4;
    p.y = p.x + p.y;
    p.y
}

comptime n: usize = width();

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
fn comptime_function_mutates_array_indexes() {
    let root = temp_dir("comptime_function_mutates_array_indexes");
    write(
        &root.join("main.nia"),
        r#"
comptime fn width() usize {
    let mut values: [4]usize = [1, 2, 3, 4];
    let mut i: usize = 0;
    while i < 4 {
        let value = i;
        values[i] += value;
        i += 1;
    }
    values[0] + values[1] + values[2] + values[3]
}

comptime n: usize = width();

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
fn comptime_for_in_rejects_range_iter_method() {
    let root = temp_dir("comptime_for_in_rejects_range_iter_method");
    write(
        &root.join("main.nia"),
        r#"
comptime fn width() usize {
    let mut total: usize = 0;
    for value in (0usize..4usize).iter() {
        total += value;
    }
    total
}

comptime n: usize = width();

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
            .contains("comptime expression can only call `comptime fn`")),
        "{:?}",
        program.diagnostics
    );
}

#[test]
fn comptime_function_mutates_nested_aggregate_paths() {
    let root = temp_dir("comptime_function_mutates_nested_aggregate_paths");
    write(
        &root.join("main.nia"),
        r#"
struct Pair {
    values: [2]usize,
}

struct Config {
    pairs: [2]Pair,
}

comptime fn width() usize {
    let mut config: Config = Config{
        pairs: [
            Pair{values: [1, 2]},
            Pair{values: [3, 4]},
        ],
    };
    config.pairs[1].values[0] = 8;
    config.pairs[0].values[1] += config.pairs[1].values[0];
    config.pairs[0].values[1]
}

comptime n: usize = width();

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
fn comptime_function_rejects_field_assignment_to_immutable_root() {
    let root = temp_dir("comptime_function_rejects_field_assignment_to_immutable_root");
    write(
        &root.join("main.nia"),
        r#"
struct Point {
    x: usize,
}

comptime fn width() usize {
    let p: Point = Point{x: 1};
    p.x = 2;
    p.x
}

comptime n: usize = width();

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
            .contains("cannot assign to immutable comptime local `p`")),
        "{:?}",
        program.diagnostics
    );
}

#[test]
fn comptime_function_rejects_field_assignment_value_type_mismatch() {
    let root = temp_dir("comptime_function_rejects_field_assignment_value_type_mismatch");
    write(
        &root.join("main.nia"),
        r#"
struct Point {
    x: usize,
}

comptime fn width() usize {
    let mut p: Point = Point{x: 1};
    p.x = true;
    p.x
}

comptime n: usize = width();
"#,
    );

    let program = check_program(root.join("main.nia").to_string_lossy().into_owned());
    assert!(
        program.diagnostics.iter().any(|diagnostic| diagnostic
            .diagnostic
            .summary
            .contains("primitive type usize")),
        "{:?}",
        program.diagnostics
    );
}

#[test]
fn comptime_function_if_statement_flows_return_and_else_if() {
    let root = temp_dir("comptime_function_if_statement_flows_return_and_else_if");
    write(
        &root.join("main.nia"),
        r#"
comptime fn width(bits: usize) usize {
    if bits == 16 {
        return 2;
    } else if bits == 32 {
        return 4;
    }
    return 8;
}

comptime n: usize = width(32);

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
fn comptime_function_for_in_arrays_require_iterator() {
    let root = temp_dir("comptime_function_for_in_arrays_require_iterator");
    write(
        &root.join("main.nia"),
        r#"
comptime fn width(values: [4]usize) usize {
    let mut total: usize = 0;
    for value in values {
        total += value;
    }
    total
}

comptime n: usize = width([1, 2, 3, 4]);

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
            .contains("comptime for-in expects an Iterator")),
        "{:?}",
        program.diagnostics
    );
}

#[test]
fn comptime_function_for_in_ranges_require_iterator() {
    let root = temp_dir("comptime_function_for_in_ranges_require_iterator");
    write(
        &root.join("main.nia"),
        r#"
comptime fn width() usize {
    let mut total: usize = 0;
    for value in 0usize..=5usize {
        if value == 2usize {
            continue;
        }
        if value == 5usize {
            break;
        }
        total += value;
    }
    total
}

comptime n: usize = width();

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
            .contains("comptime for-in expects an Iterator")),
        "{:?}",
        program.diagnostics
    );
}

#[test]
fn comptime_function_rejects_for_in_ranges_without_iterator() {
    let root = temp_dir("comptime_function_rejects_for_in_ranges_without_iterator");
    write(
        &root.join("main.nia"),
        r#"
comptime fn width() usize {
    let mut total: usize = 0;
    for value in ..5usize {
        total += value;
    }
    total
}

comptime n: usize = width();

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
            .contains("comptime for-in expects an Iterator")),
        "{:?}",
        program.diagnostics
    );
}

#[test]
fn comptime_function_for_in_iterator_execution_is_not_duck_typed() {
    let root = temp_dir("comptime_function_for_in_iterator_execution_is_not_duck_typed");
    write(
        &root.join("main.nia"),
        r#"
struct Counter {
    current: usize,
    end: usize,
}

extend Counter : Iterator {
    type Item = usize;

    comptime fn next(&mut self) ?usize {
        null
    }
}

comptime fn width() usize {
    let mut total: usize = 0;
    let mut iter = Counter{current: 0, end: 4};
    for value in iter {
        total += value;
    }
    total
}

comptime n: usize = width();

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
            .contains("comptime for-in Iterator execution is not implemented yet")),
        "{:?}",
        program.diagnostics
    );
}

#[test]
fn comptime_function_rejects_escaped_loop_control_flow() {
    let root = temp_dir("comptime_function_rejects_escaped_loop_control_flow");
    write(
        &root.join("main.nia"),
        r#"
comptime fn width() usize {
    break;
}

comptime n: usize = width();

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
            .contains("comptime loop control flow escaped its loop")),
        "{:?}",
        program.diagnostics
    );
}

#[test]
fn comptime_function_rejects_infinite_loop_statements() {
    let root = temp_dir("comptime_function_rejects_infinite_loop_statements");
    write(
        &root.join("main.nia"),
        r#"
comptime fn width() usize {
    loop {
        continue;
    }
    return 1;
}

comptime n: usize = width();

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
            .contains("comptime loop exceeded")),
        "{:?}",
        program.diagnostics
    );
}
