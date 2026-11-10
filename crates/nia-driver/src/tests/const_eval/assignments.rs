// SPDX-License-Identifier: GPL-3.0-or-later
use super::*;

#[test]
fn const_function_rejects_assignment_to_immutable_local() {
    let root = temp_dir("const_function_rejects_assignment_to_immutable_local");
    write(
        &root.join("main.nia"),
        r#"
const fun width() usize {
    let i: usize = 2;
    i = 5;
    i
}

const n: usize = width();

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
            .contains("cannot assign to immutable const local `i`")),
        "{:?}",
        program.diagnostics
    );
}

#[test]
fn const_function_rejects_assignment_to_unannotated_immutable_local() {
    let root = temp_dir("const_function_rejects_assignment_to_unannotated_immutable_local");
    write(
        &root.join("main.nia"),
        r#"
const fun width() i32 {
    let value = 2;
    value += 3;
    value
}

const n: i32 = width();

fun main() i32 { n }
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
fn const_function_rejects_assignment_value_type_mismatch() {
    let root = temp_dir("const_function_rejects_assignment_value_type_mismatch");
    write(
        &root.join("main.nia"),
        r#"
const fun width() usize {
    let mut i: usize = 2;
    i = true;
    i
}

const n: usize = width();
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
fn const_function_rejects_int_assignment_to_bool() {
    let root = temp_dir("const_function_rejects_int_assignment_to_bool");
    write(
        &root.join("main.nia"),
        r#"
const fun width() usize {
    let mut value: bool = true;
    value = 1usize;
    1usize
}

const n: usize = width();
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
fn const_function_rejects_array_assignment_shape_mismatch() {
    let root = temp_dir("const_function_rejects_array_assignment_shape_mismatch");
    write(
        &root.join("main.nia"),
        r#"
const fun width() usize {
    let mut values: [usize; 2] = [1usize, 2usize];
    values = true;
    values.len()
}

const n: usize = width();
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
fn const_function_rejects_array_assignment_length_mismatch() {
    let root = temp_dir("const_function_rejects_array_assignment_length_mismatch");
    write(
        &root.join("main.nia"),
        r#"
const fun width() usize {
    let mut values: [usize; 2] = [1usize, 2usize];
    values = [1usize; 3usize];
    values.len()
}

const n: usize = width();
"#,
    );

    // Type checking rejects the assignment; the one cause is reported once.
    let program = check_program(root.join("main.nia").to_string_lossy().into_owned());
    let summaries = program
        .diagnostics
        .iter()
        .map(|diagnostic| diagnostic.diagnostic.summary.as_str())
        .collect::<Vec<_>>();
    assert_eq!(
        summaries,
        ["const array literal length mismatch: expected 2, got 3"],
        "{:?}",
        program.diagnostics
    );
}

#[test]
fn const_function_rejects_optional_assignment_shape_mismatch() {
    let root = temp_dir("const_function_rejects_optional_assignment_shape_mismatch");
    write(
        &root.join("main.nia"),
        r#"
const fun width() usize {
    let mut value: ?usize = ?1usize;
    value = true;
    1usize
}

const n: usize = width();
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
fn const_function_rejects_error_union_assignment_shape_mismatch() {
    let root = temp_dir("const_function_rejects_error_union_assignment_shape_mismatch");
    write(
        &root.join("main.nia"),
        r#"
const fun width() usize {
    let mut value: usize!usize = !1usize;
    value = true;
    1usize
}

const n: usize = width();
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
fn const_function_rejects_nominal_assignment_missing_field_without_annotation() {
    let root =
        temp_dir("const_function_rejects_nominal_assignment_missing_field_without_annotation");
    write(
        &root.join("main.nia"),
        r#"
struct Config { width: usize, enabled: bool }

const fun width() usize {
    let mut config = Config { width: 4usize, enabled: true };
    config = Config { width: 8usize };
    config.width
}

const n: usize = width();
"#,
    );

    let program = check_program(root.join("main.nia").to_string_lossy().into_owned());
    assert!(
        program.diagnostics.iter().any(|diagnostic| {
            diagnostic
                .diagnostic
                .summary
                .contains("missing const struct field `enabled`")
        }),
        "{:?}",
        program.diagnostics
    );
}

#[test]
fn const_function_rejects_nominal_struct_assignment_missing_field() {
    let root = temp_dir("const_function_rejects_nominal_struct_assignment_missing_field");
    write(
        &root.join("main.nia"),
        r#"
struct Point {
    x: usize,
    y: usize,
}

const fun width() usize {
    let mut p: Point = Point{x: 1usize, y: 2usize};
    p = Point{x: 3usize};
    p.x
}

const n: usize = width();
"#,
    );

    let program = check_program(root.join("main.nia").to_string_lossy().into_owned());
    assert!(
        program.diagnostics.iter().any(|diagnostic| {
            diagnostic
                .diagnostic
                .summary
                .contains("missing const struct field `y`")
        }),
        "{:?}",
        program.diagnostics
    );
}

#[test]
fn const_function_rejects_nominal_assignment_extra_field_without_annotation() {
    let root = temp_dir("const_function_rejects_nominal_assignment_extra_field_without_annotation");
    write(
        &root.join("main.nia"),
        r#"
struct Config { width: usize }

const fun width() usize {
    let mut config = Config { width: 4usize };
    config = Config { width: 8usize, enabled: true };
    config.width
}

const n: usize = width();
"#,
    );

    let program = check_program(root.join("main.nia").to_string_lossy().into_owned());
    assert!(
        program.diagnostics.iter().any(|diagnostic| {
            diagnostic
                .diagnostic
                .summary
                .contains("unknown const struct field `enabled`")
        }),
        "{:?}",
        program.diagnostics
    );
}

#[test]
fn const_function_rejects_nominal_struct_assignment_extra_field() {
    let root = temp_dir("const_function_rejects_nominal_struct_assignment_extra_field");
    write(
        &root.join("main.nia"),
        r#"
struct Point {
    x: usize,
    y: usize,
}

const fun width() usize {
    let mut p: Point = Point{x: 1usize, y: 2usize};
    p = Point{x: 3usize, y: 4usize, z: 5usize};
    p.x
}

const n: usize = width();
"#,
    );

    let program = check_program(root.join("main.nia").to_string_lossy().into_owned());
    assert!(
        program.diagnostics.iter().any(|diagnostic| {
            diagnostic
                .diagnostic
                .summary
                .contains("unknown const struct field `z`")
        }),
        "{:?}",
        program.diagnostics
    );
}

#[test]
fn const_function_mutates_struct_fields() {
    let root = temp_dir("const_function_mutates_struct_fields");
    write(
        &root.join("main.nia"),
        r#"
struct Point {
    x: usize,
    y: usize,
}

const fun width() usize {
    let mut p: Point = Point{x: 2, y: 3};
    p.x += 4;
    p.y = p.x + p.y;
    p.y
}

const n: usize = width();

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
fn const_function_mutates_array_indexes() {
    let root = temp_dir("const_function_mutates_array_indexes");
    write(
        &root.join("main.nia"),
        r#"
const fun width() usize {
    let mut values: [usize; 4] = [1, 2, 3, 4];
    let mut i: usize = 0;
    while i < 4 {
        let value = i;
        values[i] += value;
        i += 1;
    }
    values[0] + values[1] + values[2] + values[3]
}

const n: usize = width();

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
fn const_function_infers_mutability_for_unannotated_locals() {
    let root = temp_dir("const_function_infers_mutability_for_unannotated_locals");
    write(
        &root.join("main.nia"),
        r#"
const fun direct() i32 {
    let mut value = 1;
    value += 2;
    value
}

const fun throughPointer() i32 {
    let mut value = 1;
    let pointer = &mut value;
    pointer.* += 2;
    value
}

const directValue: i32 = direct();
const pointerValue: i32 = throughPointer();

fun main() i32 { directValue + pointerValue }
"#,
    );

    let program = check_program(root.join("main.nia").to_string_lossy().into_owned());
    assert!(program.diagnostics.is_empty(), "{:?}", program.diagnostics);
}

#[test]
fn const_function_infers_unannotated_aggregate_places_and_rejects_default_overflow() {
    let root =
        temp_dir("const_function_infers_unannotated_array_places_and_rejects_default_overflow");
    write(
        &root.join("main.nia"),
        r#"
const fun throughElementPointer() i32 {
    let mut values = [1, 2, 3];
    let pointer = &mut values[1];
    pointer.* = 9;
    values[1]
}

const fun throughSlicePointer() i32 {
    let mut values = [1, 2, 3];
    let slice = &mut values[0..2];
    slice[1] = 7;
    values[1]
}

const fun inferredBinaryLocal() i32 {
    let mut value = -1 + 2;
    let pointer = &mut value;
    pointer.* += 4;
    value
}

const fun inferredTupleLocal() i32 {
    let mut pair = (1, 2);
    let pointer = &mut pair.0;
    pointer.* = 9;
    pair.0
}

const fun inferredRepeatLocal() i32 {
    let mut values = [1; 3];
    let pointer = &mut values[1];
    pointer.* = 9;
    values[1]
}

const fun inferredRangeLocal() i32 {
    let range = 1..3;
    range.start() + range.end()
}

const fun inferredFloatLocal() f64 {
    let mut value = 1.0;
    let pointer = &mut value;
    pointer.* += 2.0;
    value
}

const fun inferredMixedIntegerArray() u64 {
    let mut values = [1, 2u64];
    let pointer = &mut values[0];
    pointer.* = 9;
    values[0] + values[1]
}

const fun inferredMixedFloatArray() f32 {
    let mut values = [1.0, 2.0f32];
    let pointer = &mut values[0];
    pointer.* = 9.0;
    values[0] + values[1]
}

const fun inferredRepeatedFloatArray() f64 {
    let mut values = [1.0; 2];
    let pointer = &mut values[1];
    pointer.* = 4.0;
    values[0] + values[1]
}

const fun inferredIfPeer() usize {
    let value = if true { 1 } else { 2usize };
    value
}

const fun inferredIfDefault() i32 {
    let value = if true { 1 } else { 2 };
    value
}

const fun inferredMatchPeer(flag: bool) usize {
    match flag {
        true => 1,
        false => 2usize,
    }
}

const fun inferredNestedArray() i32 {
    let mut values = [[1, 2], [3, 4]];
    let pointer = &mut values[1][0];
    pointer.* = 9;
    values[1][0]
}

struct InferredPoint {
    value: i32,
}

struct InferredOuter {
    point: InferredPoint,
}

const fun inferredAggregateArray() i32 {
    let values = [InferredPoint { value: 1 }, .{ value: 2 }];
    values[1].value
}

const fun inferredAggregateTuple() i32 {
    let values: (InferredPoint, InferredPoint) = (InferredPoint { value: 3 }, .{ value: 4 });
    values.1.value
}

const fun inferredNestedAggregate() i32 {
    let value: InferredOuter = InferredOuter { point: .{ value: 5 } };
    value.point.value
}

const fun readPoint(point: InferredPoint) i32 {
    point.value
}

const fun inferredCallAggregate() i32 {
    readPoint(.{ value: 6 })
}

const elementValue: i32 = throughElementPointer();
const sliceValue: i32 = throughSlicePointer();
const binaryValue: i32 = inferredBinaryLocal();
const tupleValue: i32 = inferredTupleLocal();
const repeatValue: i32 = inferredRepeatLocal();
const rangeValue: i32 = inferredRangeLocal();
const floatValue: f64 = inferredFloatLocal();
const mixedIntegerValue: u64 = inferredMixedIntegerArray();
const mixedFloatValue: f32 = inferredMixedFloatArray();
const repeatedFloatValue: f64 = inferredRepeatedFloatArray();
const ifPeerValue: usize = inferredIfPeer();
const ifDefaultValue: i32 = inferredIfDefault();
const matchPeerValue: usize = inferredMatchPeer(true);
const nestedArrayValue: i32 = inferredNestedArray();
const aggregateArrayValue: i32 = inferredAggregateArray();
const aggregateTupleValue: i32 = inferredAggregateTuple();
const nestedAggregateValue: i32 = inferredNestedAggregate();
const callAggregateValue: i32 = inferredCallAggregate();

fun main() i32 {
    (elementValue
        + sliceValue
        + binaryValue
        + tupleValue
        + repeatValue
        + rangeValue
        + mixedIntegerValue as i32
        + mixedFloatValue as i32
        + repeatedFloatValue as i32
        + ifPeerValue as i32
        + ifDefaultValue
        + matchPeerValue as i32
        + nestedArrayValue
        + aggregateArrayValue
        + aggregateTupleValue
        + nestedAggregateValue
        + callAggregateValue) as i32
}
"#,
    );

    let program = check_program(root.join("main.nia").to_string_lossy().into_owned());
    assert!(program.diagnostics.is_empty(), "{:?}", program.diagnostics);

    let overflow_root = temp_dir("const_function_rejects_default_integer_overflow");
    write(
        &overflow_root.join("main.nia"),
        r#"
const fun value() i32 {
    let value = 3000000000;
    value
}

const result: i32 = value();
fun main() i32 { result }
"#,
    );
    let overflow_program = check_program(
        overflow_root
            .join("main.nia")
            .to_string_lossy()
            .into_owned(),
    );
    assert!(
        overflow_program
            .diagnostics
            .iter()
            .any(|diagnostic| diagnostic.diagnostic.summary.contains("out of range")),
        "{:?}",
        overflow_program.diagnostics
    );
}

#[test]
fn const_compound_assignment_evaluates_index_once() {
    let root = temp_dir("const_compound_assignment_evaluates_index_once");
    write(
        &root.join("main.nia"),
        r#"
const fun width() usize {
    let mut values: [usize; 2] = [4, 9];
    let mut calls = 0usize;
    values[{
        calls += 1;
        0usize
    }] += 1;
    values[0] * 100 + values[1] * 10 + calls
}

const n: usize = width();

fun main() i32 {
    let values: [i32; 591] = [0; n];
    values.len() as i32
}
"#,
    );

    let program = check_program(root.join("main.nia").to_string_lossy().into_owned());
    assert!(program.diagnostics.is_empty(), "{:?}", program.diagnostics);
}

#[test]
fn const_for_in_accepts_range_iter_method() {
    let root = temp_dir("const_for_in_accepts_range_iter_method");
    write(
        &root.join("main.nia"),
        r#"
using std::iter;

const fun width() usize {
    let mut total: usize = 0;
    for value in (0usize..4usize).iter() {
        total += value;
    }
    total
}

const n: usize = width();
const values: [i32; 6] = [0; n];

fun main() i32 {
    values.len() as i32
}
"#,
    );

    let program = check_program(root.join("main.nia").to_string_lossy().into_owned());
    assert!(program.diagnostics.is_empty(), "{:?}", program.diagnostics);
}

#[test]
fn const_function_mutates_nested_aggregate_paths() {
    let root = temp_dir("const_function_mutates_nested_aggregate_paths");
    write(
        &root.join("main.nia"),
        r#"
struct Pair {
    values: [usize; 2],
}

struct Config {
    pairs: [Pair; 2],
}

const fun width() usize {
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

const n: usize = width();

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
fn const_function_rejects_field_assignment_to_immutable_root() {
    let root = temp_dir("const_function_rejects_field_assignment_to_immutable_root");
    write(
        &root.join("main.nia"),
        r#"
struct Point {
    x: usize,
}

const fun width() usize {
    let p: Point = Point{x: 1};
    p.x = 2;
    p.x
}

const n: usize = width();

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
            .contains("cannot assign to immutable const local `p`")),
        "{:?}",
        program.diagnostics
    );
}

#[test]
fn const_function_rejects_field_assignment_value_type_mismatch() {
    let root = temp_dir("const_function_rejects_field_assignment_value_type_mismatch");
    write(
        &root.join("main.nia"),
        r#"
struct Point {
    x: usize,
}

const fun width() usize {
    let mut p: Point = Point{x: 1};
    p.x = true;
    p.x
}

const n: usize = width();
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
fn const_function_if_statement_flows_return_and_else_if() {
    let root = temp_dir("const_function_if_statement_flows_return_and_else_if");
    write(
        &root.join("main.nia"),
        r#"
const fun width(bits: usize) usize {
    if bits == 16 {
        return 2;
    } else if bits == 32 {
        return 4;
    }
    return 8;
}

const n: usize = width(32);

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
fn const_function_for_in_arrays_require_iterator() {
    let root = temp_dir("const_function_for_in_arrays_require_iterator");
    write(
        &root.join("main.nia"),
        r#"
const fun width(values: [usize; 4]) usize {
    let mut total: usize = 0;
    for value in values {
        total += value;
    }
    total
}

const n: usize = width([1, 2, 3, 4]);

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
fn const_function_for_in_runs_inclusive_ranges() {
    let root = temp_dir("const_function_for_in_runs_inclusive_ranges");
    write(
        &root.join("main.nia"),
        r#"
const fun width() usize {
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

const n: usize = width();
// 0 + 1 + 3 + 4: 2 is skipped and the loop stops at 5.
const values: [i32; 8] = [0; n];

fun main() i32 {
    values.len() as i32
}
"#,
    );

    // The range provider's `iter` and `next` are `const fun`, found without
    // an import because trait implementations apply wherever the trait does.
    let program = check_program(root.join("main.nia").to_string_lossy().into_owned());
    assert!(program.diagnostics.is_empty(), "{:?}", program.diagnostics);
}

#[test]
fn const_function_rejects_for_in_ranges_without_iterator() {
    let root = temp_dir("const_function_rejects_for_in_ranges_without_iterator");
    write(
        &root.join("main.nia"),
        r#"
const fun width() usize {
    let mut total: usize = 0;
    for value in ..5usize {
        total += value;
    }
    total
}

const n: usize = width();

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
fn const_function_for_in_executes_iterator_trait() {
    let root = temp_dir("const_function_for_in_executes_iterator_trait");
    write(
        &root.join("main.nia"),
        r#"
struct Counter {
    current: usize,
    end: usize,
}

extend Counter : Iterator {
    type Item = usize;

    const fun next(&mut self) ?usize {
        if self.current >= self.end {
            null
        } else {
            let value = self.current;
            self.current += 1;
            ?value
        }
    }
}

extend Counter {
    const fun next(&mut self) ?usize {
        null
    }
}

const fun width() usize {
    let mut total: usize = 0;
    let mut iter = Counter{current: 0, end: 4};
    for value in iter {
        if value == 1 {
            continue;
        }
        if value == 3 {
            break;
        }
        total += value;
    }
    total
}

const n: usize = width();

fun main() i32 {
    let values: [i32; 2] = [0; n];
    values.len() as i32
}
"#,
    );

    let program = check_program(root.join("main.nia").to_string_lossy().into_owned());
    assert!(program.diagnostics.is_empty(), "{:?}", program.diagnostics);
}

#[test]
fn const_function_for_in_executes_iterable_trait() {
    let root = temp_dir("const_function_for_in_executes_iterable_trait");
    write(
        &root.join("main.nia"),
        r#"
struct Counter {
    current: usize,
    end: usize,
}

extend Counter : Iterator {
    type Item = usize;

    const fun next(&mut self) ?usize {
        if self.current >= self.end {
            null
        } else {
            let value = self.current;
            self.current += 1;
            ?value
        }
    }
}

struct Bounds {
    start: usize,
    end: usize,
}

extend Bounds : Iterable {
    type Item = usize;
    type Iter = Counter;

    const fun iter(&self) Counter {
        Counter { current: self.start, end: self.end }
    }
}

const fun width() usize {
    let mut total = 0usize;
    let bounds = Bounds { start: 2, end: 5 };
    let mut direct = bounds.iter();
    match direct.next() {
        ?value => {
            total += value;
        },
        null => {},
    }
    for value in bounds {
        total += value;
    }
    total
}

const n: usize = width();

fun main() i32 {
    let values: [i32; 11] = [0; n];
    values.len() as i32
}
"#,
    );

    let program = check_program(root.join("main.nia").to_string_lossy().into_owned());
    assert!(program.diagnostics.is_empty(), "{:?}", program.diagnostics);
}

#[test]
fn imported_generic_const_iterable_executes_trait_witnesses() {
    let root = temp_dir("imported_generic_const_iterable_executes_trait_witnesses");
    write(
        &root.join("main.nia"),
        r#"
module pair;
using root::pair;

const fun total(values: pair::Pair[usize]) usize {
    let mut result: usize = 0;
    for value in values {
        result += value;
    }
    result
}

const n: usize = total(pair::pair(4, 6));

fun main() i32 {
    let values: [i32; 10] = [0; n];
    values.len() as i32
}
"#,
    );
    write(
        &root.join("pair.nia"),
        r#"
pub struct Pair[T] {
    first: T,
    second: T,
}

pub struct PairIter[T] {
    first: T,
    second: T,
    index: usize,
}

extend[T] PairIter[T] : Iterator {
    type Item = T;

    pub const fun next(&mut self) ?T {
        match self.index {
            0usize => {
                self.index += 1;
                ?self.first
            },
            1usize => {
                self.index += 1;
                ?self.second
            },
            _ => null,
        }
    }
}

extend[T] Pair[T] : Iterable {
    type Item = T;
    type Iter = PairIter[T];

    pub const fun iter(&self) PairIter[T] {
        PairIter[T] { first: self.first, second: self.second, index: 0 }
    }
}

pub const fun pair[T](first: T, second: T) Pair[T] {
    Pair[T] { first: first, second: second }
}
"#,
    );

    let program = check_program(root.join("main.nia").to_string_lossy().into_owned());
    assert!(program.diagnostics.is_empty(), "{:?}", program.diagnostics);
}

#[test]
fn const_iterator_witness_is_found_without_importing_its_module() {
    let root = temp_dir("const_iterator_witness_is_found_without_importing_its_module");
    write(
        &root.join("main.nia"),
        r#"
module counter;

const fun total(iter: counter::Counter) usize {
    let mut result: usize = 0;
    for value in iter {
        result += value;
    }
    result
}

fun main() i32 { 0 }
"#,
    );
    write(
        &root.join("counter.nia"),
        r#"
pub struct Counter {
    current: usize,
    end: usize,
}

extend Counter : Iterator {
    type Item = usize;

    pub const fun next(&mut self) ?usize {
        null
    }
}
"#,
    );

    let program = check_program(root.join("main.nia").to_string_lossy().into_owned());
    // Trait implementations are global facts: `counter`'s `Iterator` impl
    // proves `Counter: Iterable` although `main` never imports `counter`.
    assert!(program.diagnostics.is_empty(), "{:?}", program.diagnostics);
}

#[test]
fn unused_const_function_rejects_imported_runtime_only_iterator_witness() {
    let root = temp_dir("unused_const_function_rejects_imported_runtime_only_iterator_witness");
    write(
        &root.join("main.nia"),
        r#"
module counter;
using root::counter;

const fun total(iter: counter::Counter) usize {
    let mut result: usize = 0;
    for value in iter {
        result += value;
    }
    result
}

const fun first(iter: counter::DirectCounter) usize {
    let mut values = iter;
    match values.next() {
        ?value => value,
        null => 0,
    }
}

fun main() i32 { 0 }
"#,
    );
    write(
        &root.join("counter.nia"),
        r#"
pub struct Counter {
    current: usize,
    end: usize,
}

pub struct DirectCounter {
    current: usize,
    end: usize,
}

extend Counter : Iterator {
    type Item = usize;

    pub fun next(&mut self) ?usize {
        if self.current >= self.end {
            null
        } else {
            let value = self.current;
            self.current += 1;
            ?value
        }
    }
}

extend DirectCounter : Iterator {
    type Item = usize;

    pub fun next(&mut self) ?usize {
        if self.current >= self.end {
            null
        } else {
            let value = self.current;
            self.current += 1;
            ?value
        }
    }
}

extend Counter {
    pub const fun next(&mut self) ?usize {
        null
    }
}
"#,
    );

    let program = check_program(root.join("main.nia").to_string_lossy().into_owned());
    assert!(
        program
            .diagnostics
            .iter()
            .any(|diagnostic| diagnostic.diagnostic.summary.contains(
                "`Iterator::next` trait witness used by const for-in must be declared `const fun`"
            )),
        "{:?}",
        program.diagnostics
    );
    assert!(
        program.diagnostics.iter().any(|diagnostic| diagnostic
            .diagnostic
            .summary
            .contains("`Iterator::next` trait witness used during const evaluation must be declared `const fun`")),
        "{:?}",
        program.diagnostics
    );
}

#[test]
fn unused_const_function_rejects_imported_runtime_only_iterable_witness() {
    let root = temp_dir("unused_const_function_rejects_imported_runtime_only_iterable_witness");
    write(
        &root.join("main.nia"),
        r#"
module bounds;
using root::bounds;

const fun total(values: bounds::Bounds) usize {
    let mut result: usize = 0;
    for value in values {
        result += value;
    }
    result
}

const fun iterator(values: bounds::Bounds) bounds::Counter {
    values.iter()
}

fun main() i32 { 0 }
"#,
    );
    write(
        &root.join("bounds.nia"),
        r#"
pub struct Counter {
    current: usize,
    end: usize,
}

extend Counter : Iterator {
    type Item = usize;

    pub const fun next(&mut self) ?usize {
        if self.current >= self.end {
            null
        } else {
            let value = self.current;
            self.current += 1;
            ?value
        }
    }
}

pub struct Bounds {
    start: usize,
    end: usize,
}

extend Bounds : Iterable {
    type Item = usize;
    type Iter = Counter;

    pub fun iter(&self) Counter {
        Counter { current: self.start, end: self.end }
    }
}
"#,
    );

    let program = check_program(root.join("main.nia").to_string_lossy().into_owned());
    assert!(
        program
            .diagnostics
            .iter()
            .any(|diagnostic| diagnostic.diagnostic.summary.contains(
                "`Iterable::iter` trait witness used by const for-in must be declared `const fun`"
            )),
        "{:?}",
        program.diagnostics
    );
    assert!(
        program.diagnostics.iter().any(|diagnostic| diagnostic
            .diagnostic
            .summary
            .contains("`Iterable::iter` trait witness used during const evaluation must be declared `const fun`")),
        "{:?}",
        program.diagnostics
    );
}

#[test]
fn const_mutable_receiver_writes_back_nested_place_once() {
    let root = temp_dir("const_mutable_receiver_writes_back_nested_place_once");
    write(
        &root.join("main.nia"),
        r#"
struct Counter {
    value: usize,
}

extend Counter {
    const fun bump(&mut self) usize {
        self.value += 1;
        self.value
    }
}

struct State {
    counters: [Counter; 2],
}

const fun width() usize {
    let mut state = State {
        counters: [Counter { value: 2 }, Counter { value: 5 }],
    };
    let mut index = 0usize;
    let bumped = state.counters[{
        index += 1;
        0usize
    }].bump();
    state.counters[0].value * 1000 + state.counters[1].value * 100 + index * 10 + bumped
}

const n: usize = width();

fun main() i32 {
    let values: [i32; 3513] = [0; n];
    values.len() as i32
}
"#,
    );

    let program = check_program(root.join("main.nia").to_string_lossy().into_owned());
    assert!(program.diagnostics.is_empty(), "{:?}", program.diagnostics);
}

#[test]
fn const_function_rejects_escaped_loop_control_flow() {
    let root = temp_dir("const_function_rejects_escaped_loop_control_flow");
    write(
        &root.join("main.nia"),
        r#"
const fun width() usize {
    break;
}

const n: usize = width();

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
            .contains("const loop control flow escaped its loop")),
        "{:?}",
        program.diagnostics
    );
}

#[test]
fn const_function_rejects_infinite_loop_statements() {
    let root = temp_dir("const_function_rejects_infinite_loop_statements");
    write(
        &root.join("main.nia"),
        r#"
const fun width() usize {
    loop {
        continue;
    }
    return 1;
}

const n: usize = width();

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
            .contains("const loop exceeded")),
        "{:?}",
        program.diagnostics
    );
}

#[test]
fn const_methods_receive_self_as_a_pointer() {
    let root = temp_dir("const_methods_receive_self_as_a_pointer");
    write(
        &root.join("main.nia"),
        r#"
struct Pair {
    a: usize,
    b: usize,
}

extend Pair {
    const fun sum(&self) usize { self.a + self.b }
    // `&self` is a read-only pointer, as at runtime.
    const fun copy(&self) Pair { self.* }
    const fun swapped(self) Pair { .{ a: self.b, b: self.a } }
}

const fun build() usize {
    let pair = Pair { a: 2, b: 5 };
    pair.copy().swapped().a * 10 + pair.sum()
}

const n: usize = build();
const values: [i32; 57] = [0; n];

fun main() i32 {
    values.len() as i32
}
"#,
    );

    let program = check_program(root.join("main.nia").to_string_lossy().into_owned());
    assert!(program.diagnostics.is_empty(), "{:?}", program.diagnostics);
}

#[test]
fn const_function_if_without_else_completes_with_unit() {
    let root = temp_dir("const_function_if_without_else_completes_with_unit");
    write(
        &root.join("main.nia"),
        r#"
const fun firstSquareOver(limit: u64) u64 {
    for value in 1u64.. {
        if value * value > limit {
            return value;
        }
    }
    0
}

const n: usize = firstSquareOver(50) as usize;
const values: [i32; 8] = [0; n];

fun main() i32 {
    values.len() as i32
}
"#,
    );

    let program = check_program(root.join("main.nia").to_string_lossy().into_owned());
    assert!(program.diagnostics.is_empty(), "{:?}", program.diagnostics);
}

#[test]
fn const_calls_reach_methods_of_other_modules_by_path() {
    let root = temp_dir("const_calls_reach_methods_of_other_modules_by_path");
    write(
        &root.join("main.nia"),
        r#"
module spans;

// No `using` imports the methods; a path call needs none.
const a: usize = spans::Flat::direct(7).start;
const b: usize = spans::Span[usize]::viaHelper(3..5).start;
const values: [i32; 10] = [0; a + b];

fun main() i32 {
    values.len() as i32
}
"#,
    );
    write(
        &root.join("spans.nia"),
        r#"
pub struct Flat {
    start: usize,
}

extend Flat {
    pub const fun direct(start: usize) Flat { .{ start } }
}

pub struct Span[T] {
    start: T,
    end: T,
}

extend[T] Span[T] {
    const fun fromBounds(bounds: T..T) Span[T] { .{ start: bounds.start(), end: bounds.end() } }
    pub const fun viaHelper(bounds: T..T) Span[T] { .fromBounds(bounds) }
}
"#,
    );

    let program = check_program(root.join("main.nia").to_string_lossy().into_owned());
    assert!(program.diagnostics.is_empty(), "{:?}", program.diagnostics);
}
