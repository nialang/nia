// SPDX-License-Identifier: GPL-3.0-or-later
use super::common::*;

#[test]
fn const_generic_supertrait_projection_substitutes_array_lengths() {
    let root = temp_dir("const_generic_supertrait_projection_substitutes_array_lengths");
    write(
        &root.join("main.nia"),
        r#"
trait ArraySource[N: usize] {
    type Item;

    fn values(& self) [N][Self as ArraySource[N]]::Item;
}

trait ArraySummary[N: usize] : ArraySource[N] {
    fn len(& self) usize {
        N
    }

    fn first_or(& self, fallback: [Self as ArraySource[N]]::Item) [Self as ArraySource[N]]::Item {
        if self.len() == 0usize {
            fallback
        } else {
            self.values()[0]
        }
    }
}

struct Buffer[T, N: usize] {
    values: [N]T,
}

extend[T, N: usize] Buffer[T, N] : ArraySource[N] {
    type Item = T;

    fn values(& self) [N]T {
        self.values
    }
}

extend[T, N: usize] Buffer[T, N] : ArraySummary[N] {}

fn read[S, N: usize](source: & S, fallback: [S as ArraySource[N]]::Item) [S as ArraySource[N]]::Item
where S: ArraySummary[N] {
    source.first_or(fallback)
}

fn main() i32 {
    let buffer: Buffer[i32, 3] = { values: [7, 8, 9] };
    read[Buffer[i32, 3], 3](& buffer, 99)
}
"#,
    );

    let program = check_program(root.join("main.nia").to_string_lossy().into_owned());
    assert!(program.diagnostics.is_empty(), "{:?}", program.diagnostics);
}

#[test]
fn const_generic_assoc_projection_rejects_unconstrained_literal_fallback() {
    let root = temp_dir("const_generic_assoc_projection_rejects_unconstrained_literal_fallback");
    write(
        &root.join("main.nia"),
        r#"
trait ArraySource[N: usize] {
    type Item;
}

trait ArraySummary[N: usize] : ArraySource[N] {
    fn first_or(& self, fallback: [Self as ArraySource[N]]::Item) [Self as ArraySource[N]]::Item {
        fallback
    }
}

fn read[S, N: usize](source: & S) [S as ArraySource[N]]::Item
where S: ArraySummary[N] {
    source.first_or(99)
}
"#,
    );

    let program = check_program(root.join("main.nia").to_string_lossy().into_owned());
    assert!(
        program
            .diagnostics
            .iter()
            .any(|diagnostic| diagnostic.diagnostic.summary.contains("type mismatch")),
        "{:?}",
        program.diagnostics
    );
}

#[test]
fn const_generic_assoc_projection_rejects_mismatched_lengths() {
    let root = temp_dir("const_generic_assoc_projection_rejects_mismatched_lengths");
    write(
        &root.join("main.nia"),
        r#"
trait ArraySource[N: usize] {
    type Item;

    fn values(& self) [N][Self as ArraySource[N]]::Item;
}

struct Buffer[T, N: usize] {
    values: [N]T,
}

extend[T, N: usize] Buffer[T, N] : ArraySource[N] {
    type Item = i32;

    fn values(& self) [N]i32 {
        self.values
    }
}

fn wrong[S, N: usize](source: & S) [N][S as ArraySource[N]]::Item
where S: ArraySource[N] {
    let values: [N][S as ArraySource[N]]::Item = source.values();
    let bad: [4][S as ArraySource[N]]::Item = values;
    bad
}

fn main(buffer: Buffer[i32, 3]) [3]i32 {
    wrong[Buffer[i32, 3], 3](& buffer)
}
"#,
    );

    let program = check_program(root.join("main.nia").to_string_lossy().into_owned());
    assert!(
        program
            .diagnostics
            .iter()
            .any(|diagnostic| diagnostic.diagnostic.summary.contains("type mismatch")),
        "{:?}",
        program.diagnostics
    );
}

#[test]
fn const_generic_impl_target_pattern_infers_array_lengths() {
    let root = temp_dir("const_generic_impl_target_pattern_infers_array_lengths");
    write(
        &root.join("main.nia"),
        r#"
trait HasLen {
    fn width(& self) usize;
}

extend[T, N: usize] [N]T : HasLen {
    fn width(& self) usize {
        N
    }
}

fn read_len[A](value: & A) usize
where A: HasLen {
    value.width()
}

fn main() i32 {
    let values: [3]i32 = [1, 2, 3];
    read_len(& values) as i32
}
"#,
    );

    let program = check_program(root.join("main.nia").to_string_lossy().into_owned());
    assert!(program.diagnostics.is_empty(), "{:?}", program.diagnostics);
}

#[test]
fn const_generic_comptime_fn_result_can_drive_nominal_arg_and_extend_value() {
    let root = temp_dir("const_generic_comptime_fn_result_can_drive_nominal_arg_and_extend_value");
    write(
        &root.join("main.nia"),
        r#"
comptime fn plus_one(value: usize) usize {
    value + 1usize
}

struct Buffer[T, N: usize] {
    values: [N]T,
}

extend[T, N: usize] Buffer[T, N] {
    pub comptime WIDTH: usize = N;

    pub fn len(& self) usize {
        Buffer[T, N]::WIDTH
    }
}

comptime WIDTH: usize = plus_one(3usize);

fn main() i32 {
    let buffer: Buffer[i32, WIDTH] = { values: [1, 2, 3, 4] };
    buffer.len() as i32
}
"#,
    );

    let program = check_program(root.join("main.nia").to_string_lossy().into_owned());
    assert!(program.diagnostics.is_empty(), "{:?}", program.diagnostics);
}

#[test]
fn const_generic_extend_comptime_can_call_comptime_fn_per_instance() {
    let root = temp_dir("const_generic_extend_comptime_can_call_comptime_fn_per_instance");
    write(
        &root.join("main.nia"),
        r#"
comptime fn double(value: usize) usize {
    value * 2usize
}

struct Buffer[T, N: usize] {
    values: [N]T,
}

extend[T, N: usize] Buffer[T, N] {
    pub comptime DOUBLE_WIDTH: usize = double(N);
}

fn main() usize {
    Buffer[i32, 2]::DOUBLE_WIDTH + Buffer[i32, 5]::DOUBLE_WIDTH
}
"#,
    );

    let program = check_program(root.join("main.nia").to_string_lossy().into_owned());
    assert!(program.diagnostics.is_empty(), "{:?}", program.diagnostics);
}

#[test]
fn const_generic_instances_keep_assoc_comptime_values_separate() {
    let root = temp_dir("const_generic_instances_keep_assoc_comptime_values_separate");
    write(
        &root.join("main.nia"),
        r#"
struct Buffer[T, N: usize] {
    values: [N]T,
}

extend[T, N: usize] Buffer[T, N] {
    pub comptime WIDTH: usize = N;
}

fn main() usize {
    Buffer[i32, 2]::WIDTH + Buffer[i32, 5]::WIDTH
}
"#,
    );

    let program = check_program(root.join("main.nia").to_string_lossy().into_owned());
    assert!(program.diagnostics.is_empty(), "{:?}", program.diagnostics);
}

#[test]
fn const_generic_fake_refs_do_not_runtime_materialize_comptime_values() {
    let root = temp_dir("const_generic_fake_refs_do_not_runtime_materialize_comptime_values");
    write(
        &root.join("main.nia"),
        r#"
comptime fn width[N: usize]() usize {
    let value: usize = N;
    value
}

struct Buffer[T, N: usize] {
    values: [N]T,
}

fn make[T, N: usize](value: T) Buffer[T, N] {
    Buffer[T, N] { values: [value; width[N]()] }
}

fn main() i32 {
    let buffer = make[i32, 4](3);
    buffer.values[3]
}
"#,
    );

    let program = check_program(root.join("main.nia").to_string_lossy().into_owned());
    assert!(program.diagnostics.is_empty(), "{:?}", program.diagnostics);
}

#[test]
fn const_generic_supertrait_arguments_remain_distinct_for_projection() {
    let root = temp_dir("const_generic_supertrait_arguments_remain_distinct_for_projection");
    write(
        &root.join("main.nia"),
        r#"
trait Source[N: usize] {
    type Item;
}

trait SizedSource[N: usize] : Source[N] {}

struct Source4 {}

extend Source4 : Source[4] {
    type Item = i32;
}

extend Source4 : SizedSource[4] {}

fn bad[S](value: [S as Source[3]]::Item) [S as Source[3]]::Item
where S: SizedSource[4] {
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
fn const_generic_impl_pattern_rejects_conflicting_array_lengths() {
    let root = temp_dir("const_generic_impl_pattern_rejects_conflicting_array_lengths");
    write(
        &root.join("main.nia"),
        r#"
trait HasLength[N: usize] {}

extend[T, N: usize] [N]T : HasLength[N] {}

fn require_len[A, N: usize](value: & A)
where A: HasLength[N] {
    _ = value;
}

fn main() i32 {
    let values: [3]i32 = [1, 2, 3];
    require_len[_, 4](& values);
    0
}
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
fn const_generic_multi_param_impl_pattern_keeps_substitutions_independent() {
    let root = temp_dir("const_generic_multi_param_impl_pattern_keeps_substitutions_independent");
    write(
        &root.join("main.nia"),
        r#"
trait MatrixShape {
    fn rows(& self) usize;
    fn cols(& self) usize;
}

struct Matrix[T, R: usize, C: usize] {
    values: [R][C]T,
}

extend[T, R: usize, C: usize] Matrix[T, R, C] : MatrixShape {
    fn rows(& self) usize {
        R
    }

    fn cols(& self) usize {
        C
    }
}

fn shape[M](matrix: & M) usize
where M: MatrixShape {
    matrix.rows() * 10usize + matrix.cols()
}

fn main() usize {
    let matrix: Matrix[i32, 2, 3] = { values: [[1, 2, 3], [4, 5, 6]] };
    shape(& matrix)
}
"#,
    );

    let program = check_program(root.join("main.nia").to_string_lossy().into_owned());
    assert!(program.diagnostics.is_empty(), "{:?}", program.diagnostics);
}

#[test]
fn const_generic_comptime_arg_expression_normalizes_for_trait_matching() {
    let root = temp_dir("const_generic_comptime_arg_expression_normalizes_for_trait_matching");
    write(
        &root.join("main.nia"),
        r#"
comptime fn plus_one(value: usize) usize {
    value + 1usize
}

trait Width[N: usize] {
    fn width(& self) usize {
        N
    }
}

struct Buffer[N: usize] {
    values: [N]i32,
}

extend[N: usize] Buffer[N] : Width[N] {}

comptime THREE: usize = plus_one(2usize);

fn read[T](value: & T) usize
where T: Width[THREE] {
    value.width()
}

fn main() usize {
    let buffer: Buffer[3] = { values: [1, 2, 3] };
    read(& buffer)
}
"#,
    );

    let program = check_program(root.join("main.nia").to_string_lossy().into_owned());
    assert!(program.diagnostics.is_empty(), "{:?}", program.diagnostics);
}

#[test]
fn const_generic_assoc_type_projection_keeps_each_const_instance_separate() {
    let root = temp_dir("const_generic_assoc_type_projection_keeps_each_const_instance_separate");
    write(
        &root.join("main.nia"),
        r#"
trait Slot[N: usize] {
    type Item;
}

struct Store {}

extend Store : Slot[2] {
    type Item = i32;
}

extend Store : Slot[4] {
    type Item = usize;
}

fn pick2(value: [Store as Slot[2]]::Item) i32 {
    value
}

fn pick4(value: [Store as Slot[4]]::Item) usize {
    value
}

fn main() i32 {
    pick2(7) + pick4(8usize) as i32
}
"#,
    );

    let program = check_program(root.join("main.nia").to_string_lossy().into_owned());
    assert!(program.diagnostics.is_empty(), "{:?}", program.diagnostics);
}

#[test]
fn const_generic_assoc_type_projection_rejects_cross_instance_rewrite() {
    let root = temp_dir("const_generic_assoc_type_projection_rejects_cross_instance_rewrite");
    write(
        &root.join("main.nia"),
        r#"
trait Slot[N: usize] {
    type Item;
}

struct Store {}

extend Store : Slot[2] {
    type Item = i32;
}

extend Store : Slot[4] {
    type Item = usize;
}

fn bad(value: [Store as Slot[2]]::Item) [Store as Slot[4]]::Item {
    value
}

fn main() i32 { 0 }
"#,
    );

    let program = check_program(root.join("main.nia").to_string_lossy().into_owned());
    assert!(
        program
            .diagnostics
            .iter()
            .any(|diagnostic| diagnostic.diagnostic.summary.contains("type mismatch")),
        "{:?}",
        program.diagnostics
    );
}

#[test]
fn const_generic_type_position_accepts_comptime_expression_args() {
    let root = temp_dir("const_generic_type_position_accepts_comptime_expression_args");
    write(
        &root.join("main.nia"),
        r#"
comptime fn plus_one(value: usize) usize {
    value + 1usize
}

struct Buffer[N: usize] {
    values: [N]i32,
}

fn main() i32 {
    let buffer: Buffer[plus_one(2usize)] = { values: [1, 2, 3] };
    buffer.values[2]
}
"#,
    );

    let program = check_program(root.join("main.nia").to_string_lossy().into_owned());
    assert!(program.diagnostics.is_empty(), "{:?}", program.diagnostics);
}

#[test]
fn const_generic_call_position_accepts_comptime_expression_args() {
    let root = temp_dir("const_generic_call_position_accepts_comptime_expression_args");
    write(
        &root.join("main.nia"),
        r#"
comptime fn plus_one(value: usize) usize {
    value + 1usize
}

fn width[N: usize]() usize {
    N
}

fn main() usize {
    width[plus_one(2usize)]()
}
"#,
    );

    let program = check_program(root.join("main.nia").to_string_lossy().into_owned());
    assert!(program.diagnostics.is_empty(), "{:?}", program.diagnostics);
}

#[test]
fn const_generic_type_position_accepts_imported_comptime_expression_args() {
    let root = temp_dir("const_generic_type_position_accepts_imported_comptime_expression_args");
    write(
        &root.join("config.nia"),
        r#"
pub comptime WIDTH: usize = 3usize;

pub comptime fn plus_one(value: usize) usize {
    value + 1usize
}
"#,
    );
    write(
        &root.join("main.nia"),
        r#"
module config;
using entry::config;

struct Buffer[N: usize] {
    values: [N]i32,
}

fn main() i32 {
    let left: Buffer[config::WIDTH] = { values: [1, 2, 3] };
    let right: Buffer[config::plus_one(2usize)] = { values: [4, 5, 6] };
    left.values[2] + right.values[2]
}
"#,
    );

    let program = check_program(root.join("main.nia").to_string_lossy().into_owned());
    assert!(program.diagnostics.is_empty(), "{:?}", program.diagnostics);
}

#[test]
fn const_generic_call_position_rejects_non_usize_arg() {
    let root = temp_dir("const_generic_call_position_rejects_non_usize_arg");
    write(
        &root.join("main.nia"),
        r#"
fn width[N: usize]() usize {
    N
}

fn main() usize {
    width[true]()
}
"#,
    );

    let program = check_program(root.join("main.nia").to_string_lossy().into_owned());
    assert!(
        program
            .diagnostics
            .iter()
            .any(|diagnostic| diagnostic.diagnostic.summary.contains("comptime value")),
        "{:?}",
        program.diagnostics
    );
}

#[test]
fn const_generic_call_position_rejects_out_of_range_usize_arg() {
    let root = temp_dir("const_generic_call_position_rejects_out_of_range_usize_arg");
    write(
        &root.join("main.nia"),
        r#"
fn width[N: usize]() usize {
    N
}

fn main() usize {
    width[-1]()
}
"#,
    );

    let program = check_program(root.join("main.nia").to_string_lossy().into_owned());
    assert!(
        program
            .diagnostics
            .iter()
            .any(|diagnostic| diagnostic.diagnostic.summary.contains("comptime generic")),
        "{:?}",
        program.diagnostics
    );
}

#[test]
fn const_generic_trait_impls_accept_bool_args_without_colliding() {
    let root = temp_dir("const_generic_trait_impls_accept_bool_args_without_colliding");
    write(
        &root.join("main.nia"),
        r#"
trait Flagged[FLAG: bool] {
    fn value(& self) usize;
}

struct Token {}

extend Token : Flagged[true] {
    fn value(& self) usize {
        1usize
    }
}

extend Token : Flagged[false] {
    fn value(& self) usize {
        2usize
    }
}

fn read_true[T](value: & T) usize
where T: Flagged[true] {
    value.value()
}

fn read_false[T](value: & T) usize
where T: Flagged[false] {
    value.value()
}

fn main() usize {
    let token = Token {};
    read_true(& token) * 10usize + read_false(& token)
}
"#,
    );

    let program = check_program(root.join("main.nia").to_string_lossy().into_owned());
    assert!(program.diagnostics.is_empty(), "{:?}", program.diagnostics);
}

#[test]
fn const_generic_trait_impl_rejects_bool_arg_mismatch() {
    let root = temp_dir("const_generic_trait_impl_rejects_bool_arg_mismatch");
    write(
        &root.join("main.nia"),
        r#"
trait Flagged[FLAG: bool] {}

struct Token {}

extend Token : Flagged[true] {}

fn require_false[T](value: & T)
where T: Flagged[false] {
    _ = value;
}

fn main() i32 {
    let token = Token {};
    require_false(& token);
    0
}
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
fn const_generic_projection_rejects_cross_bool_instance_rewrite() {
    let root = temp_dir("const_generic_projection_rejects_cross_bool_instance_rewrite");
    write(
        &root.join("main.nia"),
        r#"
trait Slot[FLAG: bool] {
    type Item;
}

struct Store {}

extend Store : Slot[true] {
    type Item = i32;
}

extend Store : Slot[false] {
    type Item = usize;
}

fn bad(value: [Store as Slot[true]]::Item) [Store as Slot[false]]::Item {
    value
}

fn main() i32 { 0 }
"#,
    );

    let program = check_program(root.join("main.nia").to_string_lossy().into_owned());
    assert!(
        program
            .diagnostics
            .iter()
            .any(|diagnostic| diagnostic.diagnostic.summary.contains("type mismatch")),
        "{:?}",
        program.diagnostics
    );
}

#[test]
fn const_generic_bool_expression_normalizes_for_trait_matching() {
    let root = temp_dir("const_generic_bool_expression_normalizes_for_trait_matching");
    write(
        &root.join("main.nia"),
        r#"
comptime fn yes() bool {
    true
}

trait Flagged[FLAG: bool] {
    fn value(& self) usize;
}

struct Token {}

extend Token : Flagged[true] {
    fn value(& self) usize {
        7usize
    }
}

comptime ENABLED: bool = yes();

fn read[T](value: & T) usize
where T: Flagged[ENABLED] {
    value.value()
}

fn main() usize {
    let token = Token {};
    read(& token)
}
"#,
    );

    let program = check_program(root.join("main.nia").to_string_lossy().into_owned());
    assert!(program.diagnostics.is_empty(), "{:?}", program.diagnostics);
}

#[test]
fn const_generic_char_expression_normalizes_for_trait_matching() {
    let root = temp_dir("const_generic_char_expression_normalizes_for_trait_matching");
    write(
        &root.join("main.nia"),
        r#"
comptime fn marker() char {
    'N'
}

trait Tagged[TAG: char] {
    fn value(& self) usize;
}

struct Token {}

extend Token : Tagged['N'] {
    fn value(& self) usize {
        9usize
    }
}

comptime TAG: char = marker();

fn read[T](value: & T) usize
where T: Tagged[TAG] {
    value.value()
}

fn main() usize {
    let token = Token {};
    read(& token)
}
"#,
    );

    let program = check_program(root.join("main.nia").to_string_lossy().into_owned());
    assert!(program.diagnostics.is_empty(), "{:?}", program.diagnostics);
}

#[test]
fn const_generic_expression_normalizes_for_extension_method_specificity() {
    let root = temp_dir("const_generic_expression_normalizes_for_extension_method_specificity");
    write(
        &root.join("main.nia"),
        r#"
comptime fn plus_one(value: usize) usize {
    value + 1usize
}

struct Buffer[N: usize] {
    values: [N]i32,
}

extend[N: usize] Buffer[N] {
    fn rank(& self) i32 {
        1
    }
}

extend Buffer[plus_one(2usize)] {
    fn rank(& self) i32 {
        2
    }
}

fn main() i32 {
    let buffer: Buffer[3] = { values: [1, 2, 3] };
    buffer.rank()
}
"#,
    );

    let program = check_program(root.join("main.nia").to_string_lossy().into_owned());
    assert!(program.diagnostics.is_empty(), "{:?}", program.diagnostics);
}

#[test]
fn const_generic_expression_normalizes_for_projection_equivalence() {
    let root = temp_dir("const_generic_expression_normalizes_for_projection_equivalence");
    write(
        &root.join("main.nia"),
        r#"
comptime fn plus_one(value: usize) usize {
    value + 1usize
}

trait Slot[N: usize] {
    type Item;
}

struct Store {}

extend Store : Slot[3] {
    type Item = i32;
}

comptime WIDTH: usize = plus_one(2usize);

fn read[S](value: [S as Slot[3]]::Item) [S as Slot[WIDTH]]::Item
where S: Slot[3] {
    value
}

fn main() i32 {
    read[Store](7)
}
"#,
    );

    let program = check_program(root.join("main.nia").to_string_lossy().into_owned());
    assert!(program.diagnostics.is_empty(), "{:?}", program.diagnostics);
}

#[test]
fn const_generic_u8_expression_rejects_out_of_range_value() {
    let root = temp_dir("const_generic_u8_expression_rejects_out_of_range_value");
    write(
        &root.join("main.nia"),
        r#"
comptime fn too_large() usize {
    256usize
}

fn value[N: u8]() u8 {
    N
}

fn main() u8 {
    value[too_large()]()
}
"#,
    );

    let program = check_program(root.join("main.nia").to_string_lossy().into_owned());
    assert!(
        program
            .diagnostics
            .iter()
            .any(|diagnostic| diagnostic.diagnostic.summary.contains("out of range")),
        "{:?}",
        program.diagnostics
    );
}

#[test]
fn const_generic_rejects_conflicting_repeated_inferred_lengths() {
    let root = temp_dir("const_generic_rejects_conflicting_repeated_inferred_lengths");
    write(
        &root.join("main.nia"),
        r#"
trait SameLen[N: usize] {}

struct Pair[A, B] {
    left: A,
    right: B,
}

extend[T, U, N: usize] Pair[[N]T, [N]U] : SameLen[N] {}

fn require_same_len[P, N: usize](pair: & P)
where P: SameLen[N] {
    _ = pair;
}

fn main() i32 {
    let pair: Pair[[2]i32, [3]i32] = { left: [1, 2], right: [3, 4, 5] };
    require_same_len(& pair);
    0
}
"#,
    );

    let program = check_program(root.join("main.nia").to_string_lossy().into_owned());
    assert!(
        program.diagnostics.iter().any(|diagnostic| diagnostic
            .diagnostic
            .summary
            .contains("cannot infer comptime generic parameter")),
        "{:?}",
        program.diagnostics
    );
}

#[test]
fn ambiguous_generic_arg_reports_value_error_only_when_comptime_param_requires_it() {
    let root =
        temp_dir("ambiguous_generic_arg_reports_value_error_only_when_comptime_param_requires_it");
    write(
        &root.join("main.nia"),
        r#"
struct Buffer[N: usize] {
    values: [N]i32,
}

pub fn take(value: Buffer[MISSING]) void {
    _ = value;
}

fn main() i32 {
    0
}
"#,
    );

    let program = check_program(root.join("main.nia").to_string_lossy().into_owned());
    assert!(
        program.diagnostics.iter().any(|diagnostic| diagnostic
            .diagnostic
            .summary
            .contains("failed to resolve comptime name")),
        "{:?}",
        program.diagnostics
    );
    assert!(
        !program
            .diagnostics
            .iter()
            .any(|diagnostic| diagnostic.diagnostic.summary.contains("unknown type")),
        "{:?}",
        program.diagnostics
    );
}
