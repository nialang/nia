// SPDX-License-Identifier: GPL-3.0-or-later
use super::*;

#[test]
fn generic_const_function_explicit_type_args_drive_array_lengths() {
    let root = temp_dir("generic_const_function_explicit_type_args_drive_array_lengths");
    write(
        &root.join("main.nia"),
        r#"
using std::builtin::Len;
const fun id[T](value: T) T {
    value
}

const n: usize = id[usize](4usize);

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
fn generic_const_function_infers_type_arg_from_suffixed_literal() {
    let root = temp_dir("generic_const_function_infers_type_arg_from_suffixed_literal");
    write(
        &root.join("main.nia"),
        r#"
using std::builtin::Len;
const fun id[T](value: T) T {
    value
}

const n: usize = id(4usize);

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
fn generic_const_function_infers_type_arg_from_integer_binary_expr() {
    let root = temp_dir("generic_const_function_infers_type_arg_from_integer_binary_expr");
    write(
        &root.join("main.nia"),
        r#"
using std::builtin::Len;
const fun id[T](value: T) T {
    value
}

const n: usize = id(4usize + 3usize);

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
fn generic_const_function_infers_type_arg_from_bool_binary_expr() {
    let root = temp_dir("generic_const_function_infers_type_arg_from_bool_binary_expr");
    write(
        &root.join("main.nia"),
        r#"
using std::builtin::Len;
const fun id[T](value: T) T {
    value
}

const fun choose(value: bool) usize {
    if value {
        5usize
    } else {
        1usize
    }
}

const n: usize = choose(id((4usize + 3usize) == 7usize));

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
fn generic_const_function_infers_type_arg_from_bool_literal_and_not() {
    let root = temp_dir("generic_const_function_infers_type_arg_from_bool_literal_and_not");
    write(
        &root.join("main.nia"),
        r#"
using std::builtin::Len;
const fun id[T](value: T) T {
    value
}

const fun choose(value: bool) usize {
    if value {
        6usize
    } else {
        1usize
    }
}

const a: bool = id(true);
const b: bool = id(not false);
const n: usize = choose(a and b);

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
fn generic_const_function_infers_type_arg_from_integer_negation() {
    let root = temp_dir("generic_const_function_infers_type_arg_from_integer_negation");
    write(
        &root.join("main.nia"),
        r#"
using std::builtin::Len;
const fun id[T](value: T) T {
    value
}

const fun magnitude(value: i32) usize {
    if value == -4i32 {
        4usize
    } else {
        1usize
    }
}

const n: usize = magnitude(id(-4i32));

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
fn generic_const_function_infers_type_arg_from_if_expression() {
    let root = temp_dir("generic_const_function_infers_type_arg_from_if_expression");
    write(
        &root.join("main.nia"),
        r#"
using std::builtin::Len;
const fun id[T](value: T) T {
    value
}

const n: usize = id(if true {
    4usize
} else {
    1usize
});

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
fn generic_const_function_infers_type_arg_from_contextual_if_expression() {
    let root = temp_dir("generic_const_function_infers_type_arg_from_contextual_if_expression");
    write(
        &root.join("main.nia"),
        r#"
using std::builtin::Len;
const fun id[T](value: ?T) ?T {
    value
}

const fun unwrap(value: ?usize) usize {
    match value {
        ?payload => {
            payload
        },
        null => {
            1usize
        },
    }
}

const n: usize = unwrap(id(if true {
    ?4usize
} else {
    null
}));

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
fn generic_const_function_infers_type_arg_from_if_block_tail_local() {
    let root = temp_dir("generic_const_function_infers_type_arg_from_if_block_tail_local");
    write(
        &root.join("main.nia"),
        r#"
using std::builtin::Len;
const fun id[T](value: T) T {
    value
}

const n: usize = id(if true {
    const value: usize = 4usize;
    value
} else {
    const value: usize = 1usize;
    value
});

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
fn generic_const_function_infers_type_arg_from_match_expression() {
    let root = temp_dir("generic_const_function_infers_type_arg_from_match_expression");
    write(
        &root.join("main.nia"),
        r#"
using std::builtin::Len;
const fun id[T](value: T) T {
    value
}

const n: usize = id(match 2usize {
    1usize => 4usize,
    2usize => 8usize,
    _ => 1usize,
});

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
fn generic_const_function_rejects_mismatched_match_pattern() {
    let root = temp_dir("generic_const_function_rejects_mismatched_match_pattern");
    write(
        &root.join("main.nia"),
        r#"
const fun id[T](value: T) T {
    value
}

const n: usize = id(match 2usize {
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
            .any(|diagnostic| diagnostic.diagnostic.summary.contains("match")),
        "{:?}",
        program.diagnostics
    );
}

#[test]
fn generic_const_function_rejects_non_integer_match_range_pattern() {
    let root = temp_dir("generic_const_function_rejects_non_integer_match_range_pattern");
    write(
        &root.join("main.nia"),
        r#"
const fun id[T](value: T) T {
    value
}

const n: usize = id(match true {
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
            .any(|diagnostic| diagnostic.diagnostic.summary.contains("match")),
        "{:?}",
        program.diagnostics
    );
}

#[test]
fn generic_const_function_infers_type_arg_from_if_pattern_optional_payload() {
    let root = temp_dir("generic_const_function_infers_type_arg_from_if_pattern_optional_payload");
    write(
        &root.join("main.nia"),
        r#"
using std::builtin::Len;
const fun id[T](value: T) T {
    value
}

const value: ?usize = ?8usize;
const n: usize = match value {
    ?payload => {
        id(payload)
    },
    null => {
        1usize
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
fn generic_const_function_infers_type_arg_from_if_pattern_error_payloads() {
    let root = temp_dir("generic_const_function_infers_type_arg_from_if_pattern_error_payloads");
    write(
        &root.join("main.nia"),
        r#"
using std::builtin::Len;
const fun id[T](value: T) T {
    value
}

const value: usize!usize = !8usize;
const n: usize = match value {
    !payload => {
        id(payload)
    },
    err! => {
        id(err)
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
fn generic_const_function_infers_type_arg_from_array_literal() {
    let root = temp_dir("generic_const_function_infers_type_arg_from_array_literal");
    write(
        &root.join("main.nia"),
        r#"
using std::builtin::Len;
const fun second[T](values: [T; 2]) T {
    values[1]
}

const n: usize = second([4usize, 8usize]);

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
fn generic_const_function_infers_type_arg_from_array_repeat_literal() {
    let root = temp_dir("generic_const_function_infers_type_arg_from_array_repeat_literal");
    write(
        &root.join("main.nia"),
        r#"
using std::builtin::Len;
const fun first[T](values: [T; 2]) T {
    values[0]
}

const n: usize = first([8usize; 2]);

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
fn generic_const_function_rejects_non_integer_array_repeat_count() {
    let root = temp_dir("generic_const_function_rejects_non_integer_array_repeat_count");
    write(
        &root.join("main.nia"),
        r#"
using std::builtin::Len;
const fun first[T](values: T) usize {
    values.len()
}

const n: usize = first([8usize; true]);
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
fn generic_const_function_infers_type_arg_from_contextual_array_literal() {
    let root = temp_dir("generic_const_function_infers_type_arg_from_contextual_array_literal");
    write(
        &root.join("main.nia"),
        r#"
using std::builtin::Len;
const fun first_some[T](values: [?T; 2]) T {
    match values[0] {
        ?payload => {
            payload
        },
        null => {
            match values[1] {
                ?payload => payload,
                null => std::builtin::error("missing fallback"),
            }
        },
    }
}

const n: usize = first_some([null, ?8usize]);

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
fn generic_const_function_infers_type_arg_from_struct_literal() {
    let root = temp_dir("generic_const_function_infers_type_arg_from_struct_literal");
    write(
        &root.join("main.nia"),
        r#"
using std::builtin::Len;
struct Pair[T] {
    left: T,
    right: T,
}

const fun right[T](pair: Pair[T]) T {
    pair.right
}

const n: usize = right(Pair[usize] { left: 4usize, right: 8usize });

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
fn generic_const_function_infers_type_arg_from_contextual_struct_literal() {
    let root = temp_dir("generic_const_function_infers_type_arg_from_contextual_struct_literal");
    write(
        &root.join("main.nia"),
        r#"
using std::builtin::Len;
struct Slot[T] {
    primary: ?T,
    fallback: ?T,
}

const fun pick[T](slot: Slot[T]) T {
    match slot.primary {
        ?payload => {
            payload
        },
        null => {
            match slot.fallback {
                ?payload => payload,
                null => std::builtin::error("missing fallback"),
            }
        },
    }
}

const n: usize = pick(Slot[usize] { primary: null, fallback: ?8usize });

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
fn generic_const_function_infers_type_arg_from_struct_field() {
    let root = temp_dir("generic_const_function_infers_type_arg_from_struct_field");
    write(
        &root.join("main.nia"),
        r#"
using std::builtin::Len;
struct Point {
    x: usize,
    y: usize,
}

const fun id[T](value: T) T {
    value
}

const point: Point = Point{x: 4, y: 8};
const n: usize = id(point.y);

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
fn imported_const_struct_field_has_runtime_type() {
    let root = temp_dir("imported_const_struct_field_has_runtime_type");
    write(
        &root.join("config.nia"),
        r#"
pub struct Point {
    x: usize,
    y: usize,
}

pub const point: Point = Point{x: 4, y: 8};
"#,
    );
    write(
        &root.join("main.nia"),
        r#"
using std::builtin::Len;
module config;
using root::config;

const fun id[T](value: T) T {
    value
}

const n: usize = id(config::point.x);

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
fn generic_const_function_substitutes_struct_field_types_in_body() {
    let root = temp_dir("generic_const_function_substitutes_struct_field_types_in_body");
    write(
        &root.join("main.nia"),
        r#"
using std::builtin::Len;
struct Pair[T] {
    left: T,
    right: T,
}

const fun id[T](value: T) T {
    value
}

const fun right_id[T](pair: Pair[T]) T {
    id(pair.right)
}

const n: usize = right_id(Pair[usize] { left: 4usize, right: 8usize });

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
fn generic_const_function_infers_type_arg_from_typed_const_value() {
    let root = temp_dir("generic_const_function_infers_type_arg_from_typed_const_value");
    write(
        &root.join("main.nia"),
        r#"
using std::builtin::Len;
const fun id[T](value: T) T {
    value
}

const width: usize = 4;
const n: usize = id(width);

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
fn generic_const_function_infers_type_arg_from_inferred_const_value() {
    let root = temp_dir("generic_const_function_infers_type_arg_from_inferred_const_value");
    write(
        &root.join("main.nia"),
        r#"
using std::builtin::Len;
const fun id[T](value: T) T {
    value
}

const width = 4usize;
const n: usize = id(width);

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
fn generic_const_function_infers_type_arg_from_local_inferred_const_value() {
    let root = temp_dir("generic_const_function_infers_type_arg_from_local_inferred_const_value");
    write(
        &root.join("main.nia"),
        r#"
using std::builtin::Len;
const fun id[T](value: T) T {
    value
}

fun main() i32 {
    const width = 4usize;
    const n: usize = id(width);
    let mut values: [i32; n] = [0; n];
    values.len() as i32
}
"#,
    );

    let program = check_program(root.join("main.nia").to_string_lossy().into_owned());
    assert!(program.diagnostics.is_empty(), "{:?}", program.diagnostics);
}

#[test]
fn function_body_const_call_infers_generic_from_local_value() {
    let root = temp_dir("function_body_const_call_infers_generic_from_local_value");
    write(
        &root.join("main.nia"),
        r#"
using std::builtin::Len;
const fun id[T](value: T) T {
    value
}

fun main() i32 {
    const width = 4usize;
    const n: usize = id(width);
    let mut values: [i32; n] = [0; n];
    values.len() as i32
}
"#,
    );

    let program = check_program(root.join("main.nia").to_string_lossy().into_owned());
    assert!(program.diagnostics.is_empty(), "{:?}", program.diagnostics);
}

#[test]
fn function_body_const_call_infers_generic_from_nominal_field() {
    let root = temp_dir("function_body_const_call_infers_generic_from_nominal_field");
    write(
        &root.join("main.nia"),
        r#"
using std::builtin::Len;
const fun id[T](value: T) T {
    value
}

struct Target { word_bits: usize }
struct Config { target: Target }

fun main() i32 {
    const config = Config { target: Target { word_bits: 64usize } };
    const n: usize = id(config.target.word_bits) / 8usize;
    let mut values: [i32; n] = [0; n];
    values.len() as i32
}
"#,
    );

    let program = check_program(root.join("main.nia").to_string_lossy().into_owned());
    assert!(program.diagnostics.is_empty(), "{:?}", program.diagnostics);
}

#[test]
fn function_body_imported_const_call_infers_generic_from_local_value() {
    let root = temp_dir("function_body_imported_const_call_infers_generic_from_local_value");
    write(
        &root.join("helpers.nia"),
        r#"
pub const fun id[T](value: T) T {
    value
}
"#,
    );
    write(
        &root.join("main.nia"),
        r#"
using std::builtin::Len;
module helpers;
using root::helpers;

fun main() i32 {
    const width = 4usize;
    const n: usize = helpers::id(width);
    let mut values: [i32; n] = [0; n];
    values.len() as i32
}
"#,
    );

    let program = check_program(root.join("main.nia").to_string_lossy().into_owned());
    assert!(program.diagnostics.is_empty(), "{:?}", program.diagnostics);
}

#[test]
fn imported_const_function_body_infers_generic_from_nominal_local() {
    let root = temp_dir("imported_const_function_body_infers_generic_from_nominal_local");
    write(
        &root.join("helpers.nia"),
        r#"
const fun id[T](value: T) T {
    value
}

struct Config { bits: usize }

pub const fun word_bytes() usize {
    const configs = [Config { bits: 32usize }, Config { bits: 64usize }];
    id(configs[1].bits) / 8usize
}
"#,
    );
    write(
        &root.join("main.nia"),
        r#"
using std::builtin::Len;
module helpers;
using root::helpers;

fun main() i32 {
    const n: usize = helpers::word_bytes();
    let mut values: [i32; n] = [0; n];
    values.len() as i32
}
"#,
    );

    let program = check_program(root.join("main.nia").to_string_lossy().into_owned());
    assert!(program.diagnostics.is_empty(), "{:?}", program.diagnostics);
}

#[test]
fn imported_inferred_const_value_has_runtime_type() {
    let root = temp_dir("imported_inferred_const_value_has_runtime_type");
    write(
        &root.join("config.nia"),
        r#"
pub const width = 4usize;
"#,
    );
    write(
        &root.join("main.nia"),
        r#"
using std::builtin::Len;
module config;
using root::config;

const fun id[T](value: T) T {
    value
}

const n: usize = id(config::width);

fun main() i32 {
    let mut values: [i32; n] = [0; n];
    values.len() as i32
}
"#,
    );

    let program = check_program(root.join("main.nia").to_string_lossy().into_owned());
    assert!(program.diagnostics.is_empty(), "{:?}", program.diagnostics);
}
