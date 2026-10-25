// SPDX-License-Identifier: GPL-3.0-or-later
use super::common::*;

#[test]
fn const_generic_supertrait_projection_substitutes_array_lengths() {
    let root = temp_dir("const_generic_supertrait_projection_substitutes_array_lengths");
    write(
        &root.join("main.nia"),
        r#"
using std::builtin::Len;
trait ArraySource[N: usize] {
    type Item;

    fun values(& self) [[Self as ArraySource[N]]::Item; N];
}

trait ArraySummary[N: usize] : ArraySource[N] {
    fun len(& self) usize {
        N
    }

    fun first_or(& self, fallback: [Self as ArraySource[N]]::Item) [Self as ArraySource[N]]::Item {
        if self.len() == 0usize {
            fallback
        } else {
            self.values()[0]
        }
    }
}

struct Buffer[T, N: usize] {
    values: [T; N],
}

extend[T, N: usize] Buffer[T, N] : ArraySource[N] {
    type Item = T;

    fun values(& self) [T; N] {
        self.values
    }
}

extend[T, N: usize] Buffer[T, N] : ArraySummary[N] {}

fun read[S, N: usize](source: & S, fallback: [S as ArraySource[N]]::Item) [S as ArraySource[N]]::Item
where S: ArraySummary[N] {
    source.first_or(fallback)
}

fun main() i32 {
    let buffer = Buffer[i32, 3] { values: [7, 8, 9] };
    read[Buffer[i32, 3], 3](& buffer, 99)
}
"#,
    );

    let program = check_program(root.join("main.nia").to_string_lossy().into_owned());
    assert!(program.diagnostics.is_empty(), "{:?}", program.diagnostics);
}

#[test]
fn const_generic_default_trait_method_codegen_keeps_owner_assumption() {
    let root = temp_dir("const_generic_default_trait_method_codegen_keeps_owner_assumption");
    write(
        &root.join("main.nia"),
        r#"
trait Source[N: usize] {
    fun value(& self) usize;
}

trait Summary[N: usize] : Source[N] {
    fun total(& self) usize {
        self.value() + N
    }
}

struct Meter[N: usize] {
    value: usize,
}

extend[N: usize] Meter[N] : Source[N] {
    fun value(& self) usize {
        self.value
    }
}

extend[N: usize] Meter[N] : Summary[N] {}

fun main() usize {
    let meter = Meter[8] { value: 34usize };
    meter.total()
}
"#,
    );

    let program = codegen_program(root.join("main.nia").to_string_lossy().into_owned());
    assert!(program.diagnostics.is_empty(), "{:?}", program.diagnostics);
    assert!(
        program
            .backend_lowering
            .program
            .modules
            .iter()
            .any(|module| {
                module.function_instances.iter().any(|instance| {
                    matches!(
                        instance.const_args.as_slice(),
                        [nia_ty::ConstGenericArg {
                            value: nia_ty::ConstGenericValue::Int(value),
                            ..
                        }] if value.bits() == 8
                    )
                })
            })
    );
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
    fun first_or(& self, fallback: [Self as ArraySource[N]]::Item) [Self as ArraySource[N]]::Item {
        fallback
    }
}

fun read[S, N: usize](source: & S) [S as ArraySource[N]]::Item
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

    fun values(& self) [[Self as ArraySource[N]]::Item; N];
}

struct Buffer[T, N: usize] {
    values: [T; N],
}

extend[T, N: usize] Buffer[T, N] : ArraySource[N] {
    type Item = i32;

    fun values(& self) [i32; N] {
        self.values
    }
}

fun wrong[S, N: usize](source: & S) [[S as ArraySource[N]]::Item; N]
where S: ArraySource[N] {
    let values: [[S as ArraySource[N]]::Item; N] = source.values();
    let bad: [[S as ArraySource[N]]::Item; 4] = values;
    bad
}

fun main(buffer: Buffer[i32, 3]) [i32; 3] {
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
    fun width(& self) usize;
}

extend[T, N: usize] [T; N] : HasLen {
    fun width(& self) usize {
        N
    }
}

fun read_len[A](value: & A) usize
where A: HasLen {
    value.width()
}

fun main() i32 {
    let values: [i32; 3] = [1, 2, 3];
    read_len(& values) as i32
}
"#,
    );

    let program = check_program(root.join("main.nia").to_string_lossy().into_owned());
    assert!(program.diagnostics.is_empty(), "{:?}", program.diagnostics);
    // Checking alone exercises trait selection in sema; codegen ensures the
    // selected const-generic trait instance survives backend dispatch too.
    let codegen = codegen_program(root.join("main.nia").to_string_lossy().into_owned());
    assert!(codegen.diagnostics.is_empty(), "{:?}", codegen.diagnostics);
}

#[test]
fn codegen_materializes_const_generic_trait_method_impl_arguments() {
    let root = temp_dir("codegen_materializes_const_generic_trait_method_impl_arguments");
    write(
        &root.join("main.nia"),
        r#"
trait Make[N: usize] {
    fun make(& self) [u8; N];
}

struct Box {}

extend[N: usize] Box : Make[N] {
    fun make(& self) [u8; N] {
        [0u8; N]
    }
}

fun build[N: usize](value: & Box) [u8; N]
where Box: Make[N] {
    value.make()
}

fun main(value: & Box) [u8; 3] {
    build[3](value)
}
"#,
    );

    let program = codegen_program(root.join("main.nia").to_string_lossy().into_owned());
    assert!(program.diagnostics.is_empty(), "{:?}", program.diagnostics);
    assert!(
        program
            .backend_lowering
            .program
            .modules
            .iter()
            .flat_map(|module| &module.function_instances)
            .any(|instance| instance.name == sym("make") && instance.const_args.len() == 1)
    );
}

#[test]
fn codegen_preserves_interleaved_function_generic_order() {
    let root = temp_dir("codegen_preserves_interleaved_function_generic_order");
    write(
        &root.join("main.nia"),
        r#"
fun choose[T, N: usize, U](left: T, right: U) U {
    right
}

fun main() i64 {
    choose[i32, 3, i64](1, 9i64)
}
"#,
    );

    let program = codegen_program(root.join("main.nia").to_string_lossy().into_owned());
    assert!(program.diagnostics.is_empty(), "{:?}", program.diagnostics);
    assert!(
        program
            .backend_lowering
            .program
            .modules
            .iter()
            .flat_map(|module| &module.function_instances)
            .any(|instance| {
                instance.name == sym("choose")
                    && instance.args.len() == 2
                    && instance.const_args.len() == 1
                    && matches!(
                        program.type_store.get(instance.return_type),
                        Some(nia_ty::TyKind::Primitive(nia_ty::PrimitiveTy::I64))
                    )
            })
    );
}

#[test]
fn const_generic_const_fn_result_can_drive_nominal_arg_and_extend_value() {
    let root = temp_dir("const_generic_const_fn_result_can_drive_nominal_arg_and_extend_value");
    write(
        &root.join("main.nia"),
        r#"
using std::builtin::Len;
const fun plus_one(value: usize) usize {
    value + 1usize
}

struct Buffer[T, N: usize] {
    values: [T; N],
}

extend[T, N: usize] Buffer[T, N] {
    pub const WIDTH: usize = N;

    pub fun len(& self) usize {
        Buffer[T, N]::WIDTH
    }
}

const WIDTH: usize = plus_one(3usize);

fun main() i32 {
    let buffer = Buffer[i32, WIDTH] { values: [1, 2, 3, 4] };
    buffer.len() as i32
}
"#,
    );

    let program = check_program(root.join("main.nia").to_string_lossy().into_owned());
    assert!(program.diagnostics.is_empty(), "{:?}", program.diagnostics);
}

#[test]
fn const_generic_extend_const_can_call_const_fn_per_instance() {
    let root = temp_dir("const_generic_extend_const_can_call_const_fn_per_instance");
    write(
        &root.join("main.nia"),
        r#"
const fun double(value: usize) usize {
    value * 2usize
}

struct Buffer[T, N: usize] {
    values: [T; N],
}

extend[T, N: usize] Buffer[T, N] {
    pub const DOUBLE_WIDTH: usize = double(N);
}

fun main() usize {
    Buffer[i32, 2]::DOUBLE_WIDTH + Buffer[i32, 5]::DOUBLE_WIDTH
}
"#,
    );

    let program = check_program(root.join("main.nia").to_string_lossy().into_owned());
    assert!(program.diagnostics.is_empty(), "{:?}", program.diagnostics);
}

#[test]
fn const_generic_instances_keep_assoc_const_values_separate() {
    let root = temp_dir("const_generic_instances_keep_assoc_const_values_separate");
    write(
        &root.join("main.nia"),
        r#"
struct Buffer[T, N: usize] {
    values: [T; N],
}

extend[T, N: usize] Buffer[T, N] {
    pub const WIDTH: usize = N;
}

fun main() usize {
    Buffer[i32, 2]::WIDTH + Buffer[i32, 5]::WIDTH
}
"#,
    );

    let program = check_program(root.join("main.nia").to_string_lossy().into_owned());
    assert!(program.diagnostics.is_empty(), "{:?}", program.diagnostics);
}

#[test]
fn const_generic_fake_refs_do_not_runtime_materialize_const_values() {
    let root = temp_dir("const_generic_fake_refs_do_not_runtime_materialize_const_values");
    write(
        &root.join("main.nia"),
        r#"
const fun width[N: usize]() usize {
    let value: usize = N;
    value
}

struct Buffer[T, N: usize] {
    values: [T; N],
}

fun make[T, N: usize](value: T) Buffer[T, N] {
    Buffer[T, N] { values: [value; width[N]()] }
}

fun main() i32 {
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

fun bad[S](value: [S as Source[3]]::Item) [S as Source[3]]::Item
where S: SizedSource[4] {
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
fn const_generic_impl_pattern_rejects_conflicting_array_lengths() {
    let root = temp_dir("const_generic_impl_pattern_rejects_conflicting_array_lengths");
    write(
        &root.join("main.nia"),
        r#"
trait HasLength[N: usize] {}

extend[T, N: usize] [T; N] : HasLength[N] {}

fun require_len[A, N: usize](value: & A)
where A: HasLength[N] {
    _ = value;
}

fun main() i32 {
    let values: [i32; 3] = [1, 2, 3];
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
    fun rows(& self) usize;
    fun cols(& self) usize;
}

struct Matrix[T, R: usize, C: usize] {
    values: [[T; C]; R],
}

extend[T, R: usize, C: usize] Matrix[T, R, C] : MatrixShape {
    fun rows(& self) usize {
        R
    }

    fun cols(& self) usize {
        C
    }
}

fun shape[M](matrix: & M) usize
where M: MatrixShape {
    matrix.rows() * 10usize + matrix.cols()
}

fun main() usize {
    let matrix = Matrix[i32, 2, 3] { values: [[1, 2, 3], [4, 5, 6]] };
    shape(& matrix)
}
"#,
    );

    let program = check_program(root.join("main.nia").to_string_lossy().into_owned());
    assert!(program.diagnostics.is_empty(), "{:?}", program.diagnostics);
}

#[test]
fn const_generic_const_arg_expression_normalizes_for_trait_matching() {
    let root = temp_dir("const_generic_const_arg_expression_normalizes_for_trait_matching");
    write(
        &root.join("main.nia"),
        r#"
const fun plus_one(value: usize) usize {
    value + 1usize
}

trait Width[N: usize] {
    fun width(& self) usize {
        N
    }
}

struct Buffer[N: usize] {
    values: [i32; N],
}

extend[N: usize] Buffer[N] : Width[N] {}

const THREE: usize = plus_one(2usize);

fun read[T](value: & T) usize
where T: Width[THREE] {
    value.width()
}

fun main() usize {
    let buffer = Buffer[3] { values: [1, 2, 3] };
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

fun pick2(value: [Store as Slot[2]]::Item) i32 {
    value
}

fun pick4(value: [Store as Slot[4]]::Item) usize {
    value
}

fun main() i32 {
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

fun bad(value: [Store as Slot[2]]::Item) [Store as Slot[4]]::Item {
    value
}

fun main() i32 { 0 }
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
fn const_generic_type_position_accepts_const_expression_args() {
    let root = temp_dir("const_generic_type_position_accepts_const_expression_args");
    write(
        &root.join("main.nia"),
        r#"
const fun plus_one(value: usize) usize {
    value + 1usize
}

struct Buffer[N: usize] {
    values: [i32; N],
}

fun main() i32 {
    let buffer = Buffer[plus_one(2usize)] { values: [1, 2, 3] };
    buffer.values[2]
}
"#,
    );

    let program = check_program(root.join("main.nia").to_string_lossy().into_owned());
    assert!(program.diagnostics.is_empty(), "{:?}", program.diagnostics);
}

#[test]
fn const_generic_call_position_accepts_const_expression_args() {
    let root = temp_dir("const_generic_call_position_accepts_const_expression_args");
    write(
        &root.join("main.nia"),
        r#"
const fun plus_one(value: usize) usize {
    value + 1usize
}

fun width[N: usize]() usize {
    N
}

fun main() usize {
    width[plus_one(2usize)]()
}
"#,
    );

    let program = check_program(root.join("main.nia").to_string_lossy().into_owned());
    assert!(program.diagnostics.is_empty(), "{:?}", program.diagnostics);
}

#[test]
fn nested_const_generic_calls_preserve_the_caller_instance() {
    let root = temp_dir("nested_const_generic_calls_preserve_the_caller_instance");
    write(
        &root.join("main.nia"),
        r#"
fun inner[N: usize]() usize {
    N
}

fun outer[N: usize]() usize {
    inner[N]()
}

fun main() usize {
    outer[3]()
}
"#,
    );

    let program = check_program(root.join("main.nia").to_string_lossy().into_owned());
    assert!(program.diagnostics.is_empty(), "{:?}", program.diagnostics);
}

#[test]
fn const_generic_type_position_accepts_imported_const_expression_args() {
    let root = temp_dir("const_generic_type_position_accepts_imported_const_expression_args");
    write(
        &root.join("config.nia"),
        r#"
pub const WIDTH: usize = 3usize;

pub const fun plus_one(value: usize) usize {
    value + 1usize
}
"#,
    );
    write(
        &root.join("main.nia"),
        r#"
module config;
using root::config;

struct Buffer[N: usize] {
    values: [i32; N],
}

fun main() i32 {
    let left = Buffer[config::WIDTH] { values: [1, 2, 3] };
    let right = Buffer[config::plus_one(2usize)] { values: [4, 5, 6] };
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
fun width[N: usize]() usize {
    N
}

fun main() usize {
    width[true]()
}
"#,
    );

    let program = check_program(root.join("main.nia").to_string_lossy().into_owned());
    assert!(
        program
            .diagnostics
            .iter()
            .any(|diagnostic| diagnostic.diagnostic.summary.contains("const value")),
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
fun width[N: usize]() usize {
    N
}

fun main() usize {
    width[-1]()
}
"#,
    );

    let program = check_program(root.join("main.nia").to_string_lossy().into_owned());
    assert!(
        program
            .diagnostics
            .iter()
            .any(|diagnostic| diagnostic.diagnostic.summary.contains("const generic")),
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
    fun value(& self) usize;
}

struct Token {}

extend Token : Flagged[true] {
    fun value(& self) usize {
        1usize
    }
}

extend Token : Flagged[false] {
    fun value(& self) usize {
        2usize
    }
}

fun read_true[T](value: & T) usize
where T: Flagged[true] {
    value.value()
}

fun read_false[T](value: & T) usize
where T: Flagged[false] {
    value.value()
}

fun main() usize {
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

fun require_false[T](value: & T)
where T: Flagged[false] {
    _ = value;
}

fun main() i32 {
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

fun bad(value: [Store as Slot[true]]::Item) [Store as Slot[false]]::Item {
    value
}

fun main() i32 { 0 }
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
const fun yes() bool {
    true
}

trait Flagged[FLAG: bool] {
    fun value(& self) usize;
}

struct Token {}

extend Token : Flagged[true] {
    fun value(& self) usize {
        7usize
    }
}

const ENABLED: bool = yes();

fun read[T](value: & T) usize
where T: Flagged[ENABLED] {
    value.value()
}

fun main() usize {
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
const fun marker() char {
    'N'
}

trait Tagged[TAG: char] {
    fun value(& self) usize;
}

struct Token {}

extend Token : Tagged['N'] {
    fun value(& self) usize {
        9usize
    }
}

const TAG: char = marker();

fun read[T](value: & T) usize
where T: Tagged[TAG] {
    value.value()
}

fun main() usize {
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
const fun plus_one(value: usize) usize {
    value + 1usize
}

struct Buffer[N: usize] {
    values: [i32; N],
}

extend[N: usize] Buffer[N] {
    fun rank(& self) i32 {
        1
    }
}

extend Buffer[plus_one(2usize)] {
    fun rank(& self) i32 {
        2
    }
}

fun main() i32 {
    let buffer = Buffer[3] { values: [1, 2, 3] };
    buffer.rank()
}
"#,
    );

    let program = codegen_program(root.join("main.nia").to_string_lossy().into_owned());
    assert!(program.diagnostics.is_empty(), "{:?}", program.diagnostics);
}

#[test]
fn const_generic_expression_normalizes_for_projection_equivalence() {
    let root = temp_dir("const_generic_expression_normalizes_for_projection_equivalence");
    write(
        &root.join("main.nia"),
        r#"
const fun plus_one(value: usize) usize {
    value + 1usize
}

trait Slot[N: usize] {
    type Item;
}

struct Store {}

extend Store : Slot[3] {
    type Item = i32;
}

const WIDTH: usize = plus_one(2usize);

fun read[S](value: [S as Slot[3]]::Item) [S as Slot[WIDTH]]::Item
where S: Slot[3] {
    value
}

fun main() i32 {
    read[Store](7)
}
"#,
    );

    let program = check_program(root.join("main.nia").to_string_lossy().into_owned());
    assert!(program.diagnostics.is_empty(), "{:?}", program.diagnostics);
}

#[test]
fn repeated_const_parameter_impl_specializes_the_complete_trait_header() {
    let root = temp_dir("repeated_const_parameter_impl_specializes_the_complete_trait_header");
    write(
        &root.join("main.nia"),
        r#"
trait Select[N: usize] {
    type Item;
}

struct Buffer[N: usize] {}

extend[A: usize, B: usize] Buffer[A] : Select[B] {
    type Item = i32;
}

extend[N: usize] Buffer[N] : Select[N] {
    type Item = i64;
}

fun selected(value: [Buffer[3] as Select[3]]::Item) i64 {
    value
}

fun main() i64 {
    selected(7i64)
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
const fun too_large() usize {
    256usize
}

fun value[N: u8]() u8 {
    N
}

fun main() u8 {
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

extend[T, U, N: usize] Pair[[T; N], [U; N]] : SameLen[N] {}

fun require_same_len[P, N: usize](pair: & P)
where P: SameLen[N] {
    _ = pair;
}

fun main() i32 {
    let pair = Pair[[i32; 2], [i32; 3]] { left: [1, 2], right: [3, 4, 5] };
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
            .contains("cannot infer const generic parameter")),
        "{:?}",
        program.diagnostics
    );
}

#[test]
fn ambiguous_generic_arg_reports_value_error_only_when_const_param_requires_it() {
    let root =
        temp_dir("ambiguous_generic_arg_reports_value_error_only_when_const_param_requires_it");
    write(
        &root.join("main.nia"),
        r#"
struct Buffer[N: usize] {
    values: [i32; N],
}

pub fun take(value: Buffer[MISSING]) () {
    _ = value;
}

fun main() i32 {
    0
}
"#,
    );

    let program = check_program(root.join("main.nia").to_string_lossy().into_owned());
    assert!(
        program.diagnostics.iter().any(|diagnostic| diagnostic
            .diagnostic
            .summary
            .contains("failed to resolve const name")),
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

#[test]
fn imported_const_generic_layout_builtins_resolve_after_instantiation() {
    let root = temp_dir("imported_const_generic_layout_builtins_resolve_after_instantiation");
    write(
        &root.join("types.nia"),
        r#"
pub struct Packet[N: usize] {
    marker: u8,
    values: [u32; N],
}
"#,
    );
    write(
        &root.join("main.nia"),
        r#"
module types;
using root::types;

const fun packet_size[N: usize]() usize
where types::Packet[N]: Sized {
    std::builtin::size[types::Packet[N]]()
}

const fun marker_offset[N: usize]() usize {
    std::builtin::offset[types::Packet[N]]("marker")
}

const SIZE: usize = packet_size[3]();
const OFFSET: usize = marker_offset[3]();

fun main() usize {
    SIZE + OFFSET
}
"#,
    );

    let program = check_program(root.join("main.nia").to_string_lossy().into_owned());
    assert!(program.diagnostics.is_empty(), "{:?}", program.diagnostics);
}

#[test]
fn trait_method_prefers_concrete_const_argument_impl() {
    let root = temp_dir("trait_method_prefers_concrete_const_argument_impl");
    write(
        &root.join("main.nia"),
        r#"
trait Rank[N: usize] {
    fun rank(&self) i32;
}

struct Box {}

extend[N: usize] Box : Rank[N] {
    fun rank(&self) i32 {
        1
    }
}

extend Box : Rank[3] {
    fun rank(&self) i32 {
        3
    }
}

fun main(box: &Box) i32 {
    box.rank()
}
"#,
    );

    let program = check_program(root.join("main.nia").to_string_lossy().into_owned());
    assert!(program.diagnostics.is_empty(), "{:?}", program.diagnostics);
}

#[test]
fn expected_return_selects_trait_method_const_instance() {
    let root = temp_dir("expected_return_selects_trait_method_const_instance");
    write(
        &root.join("main.nia"),
        r#"
trait Make[N: usize] {
    fun make(&self) [u8; N];
}

struct Box {}

extend Box : Make[2] {
    fun make(&self) [u8; 2] {
        [1u8, 2u8]
    }
}

extend Box : Make[3] {
    fun make(&self) [u8; 3] {
        [1u8, 2u8, 3u8]
    }
}

fun main(box: &Box) [u8; 3] {
    box.make()
}
"#,
    );

    let program = check_program(root.join("main.nia").to_string_lossy().into_owned());
    assert!(program.diagnostics.is_empty(), "{:?}", program.diagnostics);
}

#[test]
fn associated_type_substitutes_const_argument_inferred_from_impl_target() {
    let root = temp_dir("associated_type_substitutes_const_argument_inferred_from_impl_target");
    write(
        &root.join("main.nia"),
        r#"
trait Storage {
    type Bytes;
}

struct Box[N: usize] {
    bytes: [u8; N],
}

extend[N: usize] Box[N] : Storage {
    type Bytes = [u8; N];
}

fun third(bytes: [Box[3] as Storage]::Bytes) u8 {
    bytes[2]
}

fun main() i32 {
    third([1u8, 2u8, 3u8]) as i32
}
"#,
    );

    let program = check_program(root.join("main.nia").to_string_lossy().into_owned());
    assert!(program.diagnostics.is_empty(), "{:?}", program.diagnostics);
}
