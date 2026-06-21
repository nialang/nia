// SPDX-License-Identifier: GPL-3.0-or-later
use super::*;

#[test]
fn generic_comptime_function_explicit_type_args_drive_array_lengths() {
    let root = temp_dir("generic_comptime_function_explicit_type_args_drive_array_lengths");
    write(
        &root.join("main.nia"),
        r#"
comptime fn id[T](value: T) T {
    value
}

comptime let n: usize = id[usize](4usize);

fn main() i32 {
    var values: [n]i32 = [0; n];
    values.len() as i32
}
"#,
    );

    let program = check_program(root.join("main.nia").to_string_lossy().into_owned());
    assert!(program.diagnostics.is_empty(), "{:?}", program.diagnostics);
}

#[test]
fn generic_comptime_function_infers_type_arg_from_suffixed_literal() {
    let root = temp_dir("generic_comptime_function_infers_type_arg_from_suffixed_literal");
    write(
        &root.join("main.nia"),
        r#"
comptime fn id[T](value: T) T {
    value
}

comptime let n: usize = id(4usize);

fn main() i32 {
    var values: [n]i32 = [0; n];
    values.len() as i32
}
"#,
    );

    let program = check_program(root.join("main.nia").to_string_lossy().into_owned());
    assert!(program.diagnostics.is_empty(), "{:?}", program.diagnostics);
}

#[test]
fn generic_comptime_function_infers_type_arg_from_integer_binary_expr() {
    let root = temp_dir("generic_comptime_function_infers_type_arg_from_integer_binary_expr");
    write(
        &root.join("main.nia"),
        r#"
comptime fn id[T](value: T) T {
    value
}

comptime let n: usize = id(4usize + 3usize);

fn main() i32 {
    var values: [n]i32 = [0; n];
    values.len() as i32
}
"#,
    );

    let program = check_program(root.join("main.nia").to_string_lossy().into_owned());
    assert!(program.diagnostics.is_empty(), "{:?}", program.diagnostics);
}

#[test]
fn generic_comptime_function_infers_type_arg_from_bool_binary_expr() {
    let root = temp_dir("generic_comptime_function_infers_type_arg_from_bool_binary_expr");
    write(
        &root.join("main.nia"),
        r#"
comptime fn id[T](value: T) T {
    value
}

comptime fn choose(value: bool) usize {
    if value {
        5usize
    } else {
        1usize
    }
}

comptime let n: usize = choose(id((4usize + 3usize) == 7usize));

fn main() i32 {
    var values: [n]i32 = [0; n];
    values.len() as i32
}
"#,
    );

    let program = check_program(root.join("main.nia").to_string_lossy().into_owned());
    assert!(program.diagnostics.is_empty(), "{:?}", program.diagnostics);
}

#[test]
fn generic_comptime_function_infers_type_arg_from_bool_literal_and_not() {
    let root = temp_dir("generic_comptime_function_infers_type_arg_from_bool_literal_and_not");
    write(
        &root.join("main.nia"),
        r#"
comptime fn id[T](value: T) T {
    value
}

comptime fn choose(value: bool) usize {
    if value {
        6usize
    } else {
        1usize
    }
}

comptime let a: bool = id(true);
comptime let b: bool = id(not false);
comptime let n: usize = choose(a and b);

fn main() i32 {
    var values: [n]i32 = [0; n];
    values.len() as i32
}
"#,
    );

    let program = check_program(root.join("main.nia").to_string_lossy().into_owned());
    assert!(program.diagnostics.is_empty(), "{:?}", program.diagnostics);
}

#[test]
fn generic_comptime_function_infers_type_arg_from_integer_negation() {
    let root = temp_dir("generic_comptime_function_infers_type_arg_from_integer_negation");
    write(
        &root.join("main.nia"),
        r#"
comptime fn id[T](value: T) T {
    value
}

comptime fn magnitude(value: i32) usize {
    if value == -4i32 {
        4usize
    } else {
        1usize
    }
}

comptime let n: usize = magnitude(id(-4i32));

fn main() i32 {
    var values: [n]i32 = [0; n];
    values.len() as i32
}
"#,
    );

    let program = check_program(root.join("main.nia").to_string_lossy().into_owned());
    assert!(program.diagnostics.is_empty(), "{:?}", program.diagnostics);
}

#[test]
fn generic_comptime_function_infers_type_arg_from_if_expression() {
    let root = temp_dir("generic_comptime_function_infers_type_arg_from_if_expression");
    write(
        &root.join("main.nia"),
        r#"
comptime fn id[T](value: T) T {
    value
}

comptime let n: usize = id(if true {
    4usize
} else {
    1usize
});

fn main() i32 {
    var values: [n]i32 = [0; n];
    values.len() as i32
}
"#,
    );

    let program = check_program(root.join("main.nia").to_string_lossy().into_owned());
    assert!(program.diagnostics.is_empty(), "{:?}", program.diagnostics);
}

#[test]
fn generic_comptime_function_infers_type_arg_from_contextual_if_expression() {
    let root = temp_dir("generic_comptime_function_infers_type_arg_from_contextual_if_expression");
    write(
        &root.join("main.nia"),
        r#"
comptime fn id[T](value: ?T) ?T {
    value
}

comptime fn unwrap(value: ?usize) usize {
    if let ?payload = value {
        payload
    } else null {
        1usize
    }
}

comptime let n: usize = unwrap(id(if true {
    ?4usize
} else {
    null
}));

fn main() i32 {
    var values: [n]i32 = [0; n];
    values.len() as i32
}
"#,
    );

    let program = check_program(root.join("main.nia").to_string_lossy().into_owned());
    assert!(program.diagnostics.is_empty(), "{:?}", program.diagnostics);
}

#[test]
fn generic_comptime_function_infers_type_arg_from_if_block_tail_local() {
    let root = temp_dir("generic_comptime_function_infers_type_arg_from_if_block_tail_local");
    write(
        &root.join("main.nia"),
        r#"
comptime fn id[T](value: T) T {
    value
}

comptime let n: usize = id(if true {
    comptime let value: usize = 4usize;
    value
} else {
    comptime let value: usize = 1usize;
    value
});

fn main() i32 {
    var values: [n]i32 = [0; n];
    values.len() as i32
}
"#,
    );

    let program = check_program(root.join("main.nia").to_string_lossy().into_owned());
    assert!(program.diagnostics.is_empty(), "{:?}", program.diagnostics);
}

#[test]
fn generic_comptime_function_infers_type_arg_from_switch_expression() {
    let root = temp_dir("generic_comptime_function_infers_type_arg_from_switch_expression");
    write(
        &root.join("main.nia"),
        r#"
comptime fn id[T](value: T) T {
    value
}

comptime let n: usize = id(switch 2usize {
    1usize => 4usize,
    2usize => 8usize,
    _ => 1usize,
});

fn main() i32 {
    var values: [n]i32 = [0; n];
    values.len() as i32
}
"#,
    );

    let program = check_program(root.join("main.nia").to_string_lossy().into_owned());
    assert!(program.diagnostics.is_empty(), "{:?}", program.diagnostics);
}

#[test]
fn generic_comptime_function_rejects_mismatched_switch_pattern() {
    let root = temp_dir("generic_comptime_function_rejects_mismatched_switch_pattern");
    write(
        &root.join("main.nia"),
        r#"
comptime fn id[T](value: T) T {
    value
}

comptime let n: usize = id(switch 2usize {
    true => 4usize,
    _ => 1usize,
});
"#,
    );

    let program = check_program(root.join("main.nia").to_string_lossy().into_owned());
    assert!(
        program
            .diagnostics
            .iter()
            .any(|diagnostic| diagnostic.diagnostic.summary.contains("switch")),
        "{:?}",
        program.diagnostics
    );
}

#[test]
fn generic_comptime_function_rejects_non_integer_switch_range_pattern() {
    let root = temp_dir("generic_comptime_function_rejects_non_integer_switch_range_pattern");
    write(
        &root.join("main.nia"),
        r#"
comptime fn id[T](value: T) T {
    value
}

comptime let n: usize = id(switch true {
    0usize..2usize => 4usize,
    _ => 1usize,
});
"#,
    );

    let program = check_program(root.join("main.nia").to_string_lossy().into_owned());
    assert!(
        program
            .diagnostics
            .iter()
            .any(|diagnostic| diagnostic.diagnostic.summary.contains("switch")),
        "{:?}",
        program.diagnostics
    );
}

#[test]
fn generic_comptime_function_infers_type_arg_from_if_pattern_optional_payload() {
    let root =
        temp_dir("generic_comptime_function_infers_type_arg_from_if_pattern_optional_payload");
    write(
        &root.join("main.nia"),
        r#"
comptime fn id[T](value: T) T {
    value
}

comptime let value: ?usize = ?8usize;
comptime let n: usize = if let ?payload = value {
    id(payload)
} else null {
    1usize
};

fn main() i32 {
    var values: [n]i32 = [0; n];
    values.len() as i32
}
"#,
    );

    let program = check_program(root.join("main.nia").to_string_lossy().into_owned());
    assert!(program.diagnostics.is_empty(), "{:?}", program.diagnostics);
}

#[test]
fn generic_comptime_function_infers_type_arg_from_if_pattern_error_payloads() {
    let root = temp_dir("generic_comptime_function_infers_type_arg_from_if_pattern_error_payloads");
    write(
        &root.join("main.nia"),
        r#"
comptime fn id[T](value: T) T {
    value
}

comptime let value: usize!usize = !8usize;
comptime let n: usize = if let !payload = value {
    id(payload)
} else err! {
    id(err)
};

fn main() i32 {
    var values: [n]i32 = [0; n];
    values.len() as i32
}
"#,
    );

    let program = check_program(root.join("main.nia").to_string_lossy().into_owned());
    assert!(program.diagnostics.is_empty(), "{:?}", program.diagnostics);
}

#[test]
fn generic_comptime_function_infers_type_arg_from_array_literal() {
    let root = temp_dir("generic_comptime_function_infers_type_arg_from_array_literal");
    write(
        &root.join("main.nia"),
        r#"
comptime fn second[T](values: [2]T) T {
    values[1]
}

comptime let n: usize = second([4usize, 8usize]);

fn main() i32 {
    var values: [n]i32 = [0; n];
    values.len() as i32
}
"#,
    );

    let program = check_program(root.join("main.nia").to_string_lossy().into_owned());
    assert!(program.diagnostics.is_empty(), "{:?}", program.diagnostics);
}

#[test]
fn generic_comptime_function_infers_type_arg_from_array_repeat_literal() {
    let root = temp_dir("generic_comptime_function_infers_type_arg_from_array_repeat_literal");
    write(
        &root.join("main.nia"),
        r#"
comptime fn first[T](values: [2]T) T {
    values[0]
}

comptime let n: usize = first([8usize; 2]);

fn main() i32 {
    var values: [n]i32 = [0; n];
    values.len() as i32
}
"#,
    );

    let program = check_program(root.join("main.nia").to_string_lossy().into_owned());
    assert!(program.diagnostics.is_empty(), "{:?}", program.diagnostics);
}

#[test]
fn generic_comptime_function_rejects_non_integer_array_repeat_count() {
    let root = temp_dir("generic_comptime_function_rejects_non_integer_array_repeat_count");
    write(
        &root.join("main.nia"),
        r#"
comptime fn first[T](values: T) usize {
    values.len()
}

comptime let n: usize = first([8usize; true]);
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
fn generic_comptime_function_infers_type_arg_from_contextual_array_literal() {
    let root = temp_dir("generic_comptime_function_infers_type_arg_from_contextual_array_literal");
    write(
        &root.join("main.nia"),
        r#"
comptime fn first_some[T](values: [2]?T) T {
    if let ?payload = values[0] {
        payload
    } else null {
        values[1].?
    }
}

comptime let n: usize = first_some([null, ?8usize]);

fn main() i32 {
    var values: [n]i32 = [0; n];
    values.len() as i32
}
"#,
    );

    let program = check_program(root.join("main.nia").to_string_lossy().into_owned());
    assert!(program.diagnostics.is_empty(), "{:?}", program.diagnostics);
}

#[test]
fn generic_comptime_function_infers_type_arg_from_struct_literal() {
    let root = temp_dir("generic_comptime_function_infers_type_arg_from_struct_literal");
    write(
        &root.join("main.nia"),
        r#"
struct Pair[T] {
    left: T,
    right: T,
}

comptime fn right[T](pair: Pair[T]) T {
    pair.right
}

comptime let n: usize = right({left: 4usize, right: 8usize});

fn main() i32 {
    var values: [n]i32 = [0; n];
    values.len() as i32
}
"#,
    );

    let program = check_program(root.join("main.nia").to_string_lossy().into_owned());
    assert!(program.diagnostics.is_empty(), "{:?}", program.diagnostics);
}

#[test]
fn generic_comptime_function_infers_type_arg_from_contextual_struct_literal() {
    let root = temp_dir("generic_comptime_function_infers_type_arg_from_contextual_struct_literal");
    write(
        &root.join("main.nia"),
        r#"
struct Slot[T] {
    primary: ?T,
    fallback: ?T,
}

comptime fn pick[T](slot: Slot[T]) T {
    if let ?payload = slot.primary {
        payload
    } else null {
        slot.fallback.?
    }
}

comptime let n: usize = pick({primary: null, fallback: ?8usize});

fn main() i32 {
    var values: [n]i32 = [0; n];
    values.len() as i32
}
"#,
    );

    let program = check_program(root.join("main.nia").to_string_lossy().into_owned());
    assert!(program.diagnostics.is_empty(), "{:?}", program.diagnostics);
}

#[test]
fn generic_comptime_function_infers_type_arg_from_struct_field() {
    let root = temp_dir("generic_comptime_function_infers_type_arg_from_struct_field");
    write(
        &root.join("main.nia"),
        r#"
struct Point {
    x: usize,
    y: usize,
}

comptime fn id[T](value: T) T {
    value
}

comptime let point: Point = Point{x: 4, y: 8};
comptime let n: usize = id(point.y);

fn main() i32 {
    var values: [n]i32 = [0; n];
    values.len() as i32
}
"#,
    );

    let program = check_program(root.join("main.nia").to_string_lossy().into_owned());
    assert!(program.diagnostics.is_empty(), "{:?}", program.diagnostics);
}

#[test]
fn imported_comptime_struct_field_has_runtime_type() {
    let root = temp_dir("imported_comptime_struct_field_has_runtime_type");
    write(
        &root.join("config.nia"),
        r#"
pub struct Point {
    x: usize,
    y: usize,
}

pub comptime let point: Point = Point{x: 4, y: 8};
"#,
    );
    write(
        &root.join("main.nia"),
        r#"
module config;
using entry::config;

comptime fn id[T](value: T) T {
    value
}

comptime let n: usize = id(config::point.x);

fn main() i32 {
    var values: [n]i32 = [0; n];
    values.len() as i32
}
"#,
    );

    let program = check_program(root.join("main.nia").to_string_lossy().into_owned());
    assert!(program.diagnostics.is_empty(), "{:?}", program.diagnostics);
}

#[test]
fn generic_comptime_function_substitutes_struct_field_types_in_body() {
    let root = temp_dir("generic_comptime_function_substitutes_struct_field_types_in_body");
    write(
        &root.join("main.nia"),
        r#"
struct Pair[T] {
    left: T,
    right: T,
}

comptime fn id[T](value: T) T {
    value
}

comptime fn right_id[T](pair: Pair[T]) T {
    id(pair.right)
}

comptime let n: usize = right_id({left: 4usize, right: 8usize});

fn main() i32 {
    var values: [n]i32 = [0; n];
    values.len() as i32
}
"#,
    );

    let program = check_program(root.join("main.nia").to_string_lossy().into_owned());
    assert!(program.diagnostics.is_empty(), "{:?}", program.diagnostics);
}

#[test]
fn generic_comptime_function_infers_type_arg_from_typed_comptime_value() {
    let root = temp_dir("generic_comptime_function_infers_type_arg_from_typed_comptime_value");
    write(
        &root.join("main.nia"),
        r#"
comptime fn id[T](value: T) T {
    value
}

comptime let width: usize = 4;
comptime let n: usize = id(width);

fn main() i32 {
    var values: [n]i32 = [0; n];
    values.len() as i32
}
"#,
    );

    let program = check_program(root.join("main.nia").to_string_lossy().into_owned());
    assert!(program.diagnostics.is_empty(), "{:?}", program.diagnostics);
}

#[test]
fn generic_comptime_function_infers_type_arg_from_inferred_comptime_value() {
    let root = temp_dir("generic_comptime_function_infers_type_arg_from_inferred_comptime_value");
    write(
        &root.join("main.nia"),
        r#"
comptime fn id[T](value: T) T {
    value
}

comptime let width = 4usize;
comptime let n: usize = id(width);

fn main() i32 {
    var values: [n]i32 = [0; n];
    values.len() as i32
}
"#,
    );

    let program = check_program(root.join("main.nia").to_string_lossy().into_owned());
    assert!(program.diagnostics.is_empty(), "{:?}", program.diagnostics);
}

#[test]
fn generic_comptime_function_infers_type_arg_from_local_inferred_comptime_value() {
    let root =
        temp_dir("generic_comptime_function_infers_type_arg_from_local_inferred_comptime_value");
    write(
        &root.join("main.nia"),
        r#"
comptime fn id[T](value: T) T {
    value
}

fn main() i32 {
    comptime let width = 4usize;
    comptime let n: usize = id(width);
    var values: [n]i32 = [0; n];
    values.len() as i32
}
"#,
    );

    let program = check_program(root.join("main.nia").to_string_lossy().into_owned());
    assert!(program.diagnostics.is_empty(), "{:?}", program.diagnostics);
}

#[test]
fn function_body_comptime_call_infers_generic_from_local_value() {
    let root = temp_dir("function_body_comptime_call_infers_generic_from_local_value");
    write(
        &root.join("main.nia"),
        r#"
comptime fn id[T](value: T) T {
    value
}

fn main() i32 {
    comptime let width = 4usize;
    comptime let n: usize = id(width);
    var values: [n]i32 = [0; n];
    values.len() as i32
}
"#,
    );

    let program = check_program(root.join("main.nia").to_string_lossy().into_owned());
    assert!(program.diagnostics.is_empty(), "{:?}", program.diagnostics);
}

#[test]
fn function_body_comptime_call_infers_generic_from_structural_field() {
    let root = temp_dir("function_body_comptime_call_infers_generic_from_structural_field");
    write(
        &root.join("main.nia"),
        r#"
comptime fn id[T](value: T) T {
    value
}

fn main() i32 {
    comptime let config = {target: {word_bits: 64usize}};
    comptime let n: usize = id(config.target.word_bits) / 8usize;
    var values: [n]i32 = [0; n];
    values.len() as i32
}
"#,
    );

    let program = check_program(root.join("main.nia").to_string_lossy().into_owned());
    assert!(program.diagnostics.is_empty(), "{:?}", program.diagnostics);
}

#[test]
fn function_body_imported_comptime_call_infers_generic_from_local_value() {
    let root = temp_dir("function_body_imported_comptime_call_infers_generic_from_local_value");
    write(
        &root.join("helpers.nia"),
        r#"
pub comptime fn id[T](value: T) T {
    value
}
"#,
    );
    write(
        &root.join("main.nia"),
        r#"
module helpers;
using entry::helpers;

fn main() i32 {
    comptime let width = 4usize;
    comptime let n: usize = helpers::id(width);
    var values: [n]i32 = [0; n];
    values.len() as i32
}
"#,
    );

    let program = check_program(root.join("main.nia").to_string_lossy().into_owned());
    assert!(program.diagnostics.is_empty(), "{:?}", program.diagnostics);
}

#[test]
fn imported_comptime_function_body_infers_generic_from_structural_local() {
    let root = temp_dir("imported_comptime_function_body_infers_generic_from_structural_local");
    write(
        &root.join("helpers.nia"),
        r#"
comptime fn id[T](value: T) T {
    value
}

pub comptime fn word_bytes() usize {
    comptime let configs = [{bits: 32usize}, {bits: 64usize}];
    id(configs[1].bits) / 8usize
}
"#,
    );
    write(
        &root.join("main.nia"),
        r#"
module helpers;
using entry::helpers;

fn main() i32 {
    comptime let n: usize = helpers::word_bytes();
    var values: [n]i32 = [0; n];
    values.len() as i32
}
"#,
    );

    let program = check_program(root.join("main.nia").to_string_lossy().into_owned());
    assert!(program.diagnostics.is_empty(), "{:?}", program.diagnostics);
}

#[test]
fn imported_inferred_comptime_value_has_runtime_type() {
    let root = temp_dir("imported_inferred_comptime_value_has_runtime_type");
    write(
        &root.join("config.nia"),
        r#"
pub comptime let width = 4usize;
"#,
    );
    write(
        &root.join("main.nia"),
        r#"
module config;
using entry::config;

comptime fn id[T](value: T) T {
    value
}

comptime let n: usize = id(config::width);

fn main() i32 {
    var values: [n]i32 = [0; n];
    values.len() as i32
}
"#,
    );

    let program = check_program(root.join("main.nia").to_string_lossy().into_owned());
    assert!(program.diagnostics.is_empty(), "{:?}", program.diagnostics);
}
