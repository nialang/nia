// SPDX-License-Identifier: GPL-3.0-or-later
use super::common::*;

#[test]
fn records_size_and_align_builtin_values() {
    let checked = pipeline(
        r#"
struct Pair {
    a: u8,
    b: i32,
}

fn main() usize {
    var size = @size[Pair]();
    var align = @align[Pair]();
    size + align
}
"#,
    );
    assert!(checked.diagnostics.is_empty(), "{:?}", checked.diagnostics);
    assert!(
        checked
            .facts
            .node_builtin_values
            .values()
            .any(|value| *value == BuiltinValue::Usize(8))
    );
    assert!(
        checked
            .facts
            .node_builtin_values
            .values()
            .any(|value| *value == BuiltinValue::Usize(4))
    );
}

#[test]
fn records_field_offset_builtin_values() {
    let checked = pipeline(
        r#"
extern struct Pair {
    a: u8,
    b: u32,
}

union Bits {
    i: i32,
    f: f32,
}

fn main() usize {
    var b = @offset[Pair]("b");
    var f = @offset[Bits]("f");
    b + f
}
"#,
    );
    assert!(checked.diagnostics.is_empty(), "{:?}", checked.diagnostics);
    assert!(
        checked
            .facts
            .node_builtin_values
            .values()
            .any(|value| *value == BuiltinValue::Usize(4))
    );
    assert!(
        checked
            .facts
            .node_builtin_values
            .values()
            .any(|value| *value == BuiltinValue::Usize(0))
    );
    let body = checked
        .ir
        .function_bodies
        .values()
        .next()
        .expect("main body");
    assert!(body.stmts.iter().any(|stmt| {
        matches!(
            stmt.kind,
            nia_body_ir::TypedStmtKind::Binding(nia_body_ir::TypedBinding {
                value: Some(nia_body_ir::TypedExpr {
                    kind: nia_body_ir::TypedExprKind::BuiltinValue(
                        nia_body_ir::BuiltinConst::Usize(4)
                    ),
                    ..
                }),
                ..
            })
        )
    }));
}

#[test]
fn rejects_invalid_field_offset_builtins() {
    let checked = pipeline(
        r#"
struct Pair {
    a: u8,
}

fn missing() usize {
    @offset[Pair]("b")
}

fn non_aggregate() usize {
    @offset[u32]("x")
}

fn non_string() usize {
    @offset[Pair](0)
}
"#,
    );
    assert!(
        checked
            .diagnostics
            .iter()
            .any(|diagnostic| diagnostic.summary.contains("has no field `b`")),
        "{:?}",
        checked.diagnostics
    );
    assert!(
        checked
            .diagnostics
            .iter()
            .any(|diagnostic| diagnostic.summary.contains("requires a struct or union")),
        "{:?}",
        checked.diagnostics
    );
    assert!(
        checked.diagnostics.iter().any(|diagnostic| diagnostic
            .summary
            .contains("field name must be a string literal")),
        "{:?}",
        checked.diagnostics
    );
}

#[test]
fn accepts_layout_builtins_as_array_lengths() {
    let checked = pipeline(
        r#"
struct Pair {
    a: u8,
    b: i32,
}

fn take_size(xs: [@size[Pair]()]u8) u8 {
    xs[0]
}

fn main() u8 {
    var exact: [@size[Pair]()]u8 = [b'\0'; 8];
    var aligned: [@align[Pair]()]u8 = [b'\0'; 4];
    _ = take_size(exact);
    aligned[0]
}
"#,
    );
    assert!(checked.diagnostics.is_empty(), "{:?}", checked.diagnostics);
}

#[test]
fn checks_slices_len_ptr_and_indexing() {
    let checked = pipeline(
        r#"
fn read(xs: & [i32]) i32 {
    xs[0]
}

fn write(xs: &mut [i32]) i32 {
    xs[0] = 10;
    xs[0]
}

fn main() i32 {
    var xs: [4]i32 = [1, 2, 3, 4];
    var s = & xs[..];
    var t = & xs[1..=2];
    var p = & xs[0];
    var single = & p[..];
    _ = s.get_ptr_read();
    xs.len() as i32 + s.len() as i32 + t.len() as i32 + single.len() as i32 + read(s)
}
"#,
    );
    assert!(checked.diagnostics.is_empty(), "{:?}", checked.diagnostics);
}

#[test]
fn checks_memory_intrinsic_builtins() {
    let checked = pipeline(
        r#"
fn main() void {
    var dst: [4]u8 = [0, 0, 0, 0];
    let src: [4]u8 = [1, 2, 3, 4];
    @memcpy(&mut dst[..], &src[..]);
    @memmove(&mut dst[1..], &dst[0..3]);
    @memset(&mut dst[..], 0);
}
"#,
    );
    assert!(checked.diagnostics.is_empty(), "{:?}", checked.diagnostics);
    let body = checked
        .ir
        .function_bodies
        .values()
        .next()
        .expect("main body");
    let intrinsic_count = body
        .stmts
        .iter()
        .filter(|stmt| {
            matches!(
                stmt.kind,
                nia_body_ir::TypedStmtKind::Expr(nia_body_ir::TypedExpr {
                    kind: nia_body_ir::TypedExprKind::MemoryIntrinsic(_),
                    ..
                })
            )
        })
        .count();
    assert_eq!(intrinsic_count, 3);
}

#[test]
fn rejects_invalid_memory_intrinsic_builtins() {
    let checked = pipeline(
        r#"
fn readonly(xs: & [u8]) void {
    @memcpy(xs, xs);
}

fn memset_non_byte() void {
    var xs: [2]i32 = [1, 2];
    @memset(&mut xs[..], 0);
}
"#,
    );
    assert!(
        checked
            .diagnostics
            .iter()
            .any(|diagnostic| diagnostic.summary.contains("destination must be mutable")),
        "{:?}",
        checked.diagnostics
    );
    assert!(
        checked
            .diagnostics
            .iter()
            .any(|diagnostic| diagnostic.summary.contains("destination element")),
        "{:?}",
        checked.diagnostics
    );
}

#[test]
fn rejects_bare_range_index_and_readonly_slice_assignment() {
    let checked = pipeline(
        r#"
fn main(xs: & [i32]) i32 {
    var y = xs[..];
    xs[0] = 1;
    0
}
"#,
    );
    assert!(
        checked.diagnostics.iter().any(|diagnostic| {
            diagnostic
                .summary
                .contains("range index expression must be taken as a slice pointer")
        }),
        "{:?}",
        checked.diagnostics
    );
    assert!(
        checked
            .diagnostics
            .iter()
            .any(|diagnostic| { diagnostic.summary.contains("slice is read-only") }),
        "{:?}",
        checked.diagnostics
    );
}

#[test]
fn checks_array_literal_element_types() {
    let checked = pipeline(
        r#"
fn main(flag: bool) i32 {
    var xs: [2]i32 = [1, flag];
    var ys: [3]i32 = [flag; 3];
    0
}
"#,
    );
    assert_eq!(
        checked
            .diagnostics
            .iter()
            .filter(|diagnostic| diagnostic.summary.contains("array literal element"))
            .count(),
        2
    );
}

#[test]
fn checks_array_literal_lengths_and_inferred_lengths() {
    let checked = pipeline(
        r#"
fn take_pair(xs: [2]i32) i32 {
    xs[0]
}

fn main() i32 {
    var inferred: [2]i32 = [1, 2];
    var repeated: [3]i32 = [1; 3];
    var too_many: [2]i32 = [1, 2, 3];
    var bad_repeat: [2]i32 = [1; 3];
    take_pair([1, 2])
}
"#,
    );
    assert_eq!(
        checked
            .diagnostics
            .iter()
            .filter(|diagnostic| diagnostic.summary.contains("array literal length mismatch"))
            .count(),
        2
    );
    assert!(
        !checked
            .diagnostics
            .iter()
            .any(|diagnostic| diagnostic.summary.contains("binding initializer"))
    );
    assert!(
        !checked
            .diagnostics
            .iter()
            .any(|diagnostic| diagnostic.summary.contains("call argument"))
    );
}

#[test]
fn infers_unannotated_array_literal_bindings() {
    let checked = pipeline(
        r#"
var global_xs = [1, 2, 3];

fn take_triplet(xs: [3]i32) i32 {
    xs[2]
}

fn take_matrix(xs: [2][2]i32) i32 {
    xs[1][0]
}

struct Box[T] {
    value: T,
}

fn take_box_matrix(xs: [2][2]Box[i32]) i32 {
    xs[1][0].value
}

fn main() i32 {
    var xs = [1, 2, 3];
    var repeated = [1; 3];
    var anchored = [1, xs[0], 3];
    var matrix = [[1, 2], [3, 4]];
    var typed_matrix = [2][2]Box[i32][
        [Box[i32] { value: 1 }, Box[i32] { value: 2 }],
        [Box[i32] { value: 3 }, Box[i32] { value: 4 }],
    ];
    var bad = [xs[0], true];
    _ = take_triplet(global_xs);
    _ = take_triplet(xs);
    _ = take_triplet(repeated);
    _ = take_triplet(anchored);
    _ = take_matrix(matrix);
    _ = take_box_matrix(typed_matrix);
    0
}
"#,
    );
    assert_eq!(
        checked
            .diagnostics
            .iter()
            .filter(|diagnostic| diagnostic.summary.contains("array literal element"))
            .count(),
        1,
        "{:?}",
        checked.diagnostics
    );
    assert!(
        !checked.diagnostics.iter().any(|diagnostic| diagnostic
            .summary
            .contains("array literal requires an expected")),
        "{:?}",
        checked.diagnostics
    );
    assert!(
        !checked
            .diagnostics
            .iter()
            .any(|diagnostic| diagnostic.summary.contains("call argument")),
        "{:?}",
        checked.diagnostics
    );
}

#[test]
fn reports_invalid_array_repeat_count() {
    let checked = pipeline(
        r#"
fn main() i32 {
    var bad: [2]i32 = [1; 1 / 0];
    0
}
"#,
    );
    assert!(checked.diagnostics.iter().any(|diagnostic| {
        diagnostic
            .summary
            .contains("array repeat count is not a valid constant")
            && diagnostic.summary.contains("division by zero")
    }));
}

#[test]
fn checks_large_array_repeat_count_from_comptime_binding() {
    let checked = pipeline(
        r#"
comptime let N: usize = 1048576;

fn main() i32 {
    var buffer: [N]u8 = [0u8; N];
    0
}
"#,
    );
    assert!(checked.diagnostics.is_empty(), "{:?}", checked.diagnostics);
    assert!(
        checked
            .facts
            .node_array_repeat_counts
            .values()
            .any(|count| *count == 1048576),
        "{:?}",
        checked.facts.node_array_repeat_counts
    );
}

#[test]
fn checks_text_and_byte_string_literal_types() {
    let checked = pipeline(
        r#"
fn main() i32 {
    var text: [3]char = "中a\n".*;
    var adjacent_text: [9]char = ("中" "" "a\n" "" "b" "c" "" "done").*;
    var inferred_text: [_]char = "hi".*;
    var multiline: [11]char = (
        \\hello
        \\world
    ).*;
    var byte_multiline: [11]u8 = (
        b\\hello
        \\world
    ).*;
    var bytes: [4]u8 = b"nia\0".*;
    var adjacent_bytes: [4]u8 = (b"" b"n" b"" b"i" b"" b"a" b"" b"\0").*;
    var nul_terminated: [4]u8 = b"nia\0".*;
    var adjacent_nul_terminated: [4]u8 = (b"" b"n" b"" b"i" b"" b"a" b"" b"\0").*;
    var wrong_text_len: [2]char = "中a\n".*;
    var bad_bytes: [3]u8 = "nia";
    var byte: u8 = b'a';
    var ch: char = 'a';
    var code: u32 = ch as u32;
    var bad_char: char = code as char;
    var bad_byte: u8 = 'a';
    0
}
"#,
    );
    assert_eq!(
        checked
            .diagnostics
            .iter()
            .filter(|diagnostic| diagnostic.summary.contains("binding initializer"))
            .count(),
        3
    );
    assert!(
        checked
            .diagnostics
            .iter()
            .any(|diagnostic| diagnostic.summary.contains("cannot cast u32 to char")),
        "{:?}",
        checked.diagnostics
    );
    assert!(
        !checked
            .diagnostics
            .iter()
            .any(|diagnostic| diagnostic.summary.contains("text"))
    );
}

#[test]
fn checks_index_and_address_of_array_elements() {
    let checked = pipeline(
        r#"
extern fn puts(ptr: &u8) i32;

let hello = b"hello\0";

fn main(flag: bool) i32 {
    var xs: [2]u8 = [1, 2];
    var p: &u8 = &xs[0];
    var c: &u8 = &(hello.*[0]);
    _ = puts(&(hello.*[0]));
    _ = xs[flag];
    0
}
"#,
    );
    assert!(
        !checked
            .diagnostics
            .iter()
            .any(|diagnostic| diagnostic.summary.contains("binding initializer"))
    );
    assert!(
        !checked
            .diagnostics
            .iter()
            .any(|diagnostic| diagnostic.summary.contains("call argument"))
    );
    assert!(
        checked
            .diagnostics
            .iter()
            .any(|diagnostic| diagnostic.summary.contains("index"))
    );
}

#[test]
fn checks_literal_and_pointer_array_to_slice_coercions_and_rvalue_reference_targets() {
    let checked = pipeline(
        r#"
fn take(xs: & [i32]) i32 {
    xs.len() as i32
}

fn mutate(xs: &mut [i32]) i32 {
    xs[0] = 9;
    xs[0]
}

fn bytes(xs: & [u8]) i32 {
    xs.len() as i32
}

fn main() i32 {
    var ro: & [char] = "abc";
    var rb: & [u8] = b"abc";
    var rc: & [u8] = b"hi\0";
    var cast_text: & [char] = "abc" as &[char];
    var cast_bytes: & [u8] = b"abc" as &[u8];
    var cast_cbytes: & [u8] = b"hi\0" as &[u8];
    var arr: [2]i32 = [6, 7];
    var from_place: & [i32] = &arr;
    var cast_from_place: & [i32] = &arr as &[i32];
    var from_string: & [u8] = b"hi\0";
    var literal_ptr: &u8 = b"hi\0".get_ptr_read();
    _ = take(&[1, 2, 3]);
    _ = mutate(&mut [4, 5]);
    _ = bytes(b"hi\0");
    _ = literal_ptr;

    var int_ptr: &i32 = &10;
    var sum_ptr: &i32 = &(1 + 2);
    var call_ptr: &i32 = &make();
    var temp_slice: & [i32] = & [1, 2, 3][..];
    0
}

fn make() i32 {
    42
}
"#,
    );
    assert!(
        !checked
            .diagnostics
            .iter()
            .any(|diagnostic| diagnostic.summary.contains("binding initializer")),
        "{:?}",
        checked.diagnostics
    );
    assert!(
        !checked
            .diagnostics
            .iter()
            .any(|diagnostic| diagnostic.summary.contains("call argument")),
        "{:?}",
        checked.diagnostics
    );
    assert!(
        !checked
            .diagnostics
            .iter()
            .any(|diagnostic| diagnostic.summary.contains("reference target")),
        "{:?}",
        checked.diagnostics
    );
    assert!(
        !checked
            .diagnostics
            .iter()
            .any(|diagnostic| diagnostic.summary.contains("slice target")),
        "{:?}",
        checked.diagnostics
    );
    assert!(
        checked.facts.node_pointer_array_to_slice_coercions.len() >= 3,
        "{:?}",
        checked.facts.node_pointer_array_to_slice_coercions
    );
}

#[test]
fn rejects_general_array_value_to_slice_coercions() {
    let checked = pipeline(
        r#"
fn take(xs: &[i32]) i32 {
    xs.len() as i32
}

fn main() i32 {
    var arr: [2]i32 = [1, 2];
    var from_place: &[i32] = arr;
    var from_literal: &[i32] = [3, 4];
    var cast_place = arr as &[i32];
    var cast_literal = [2]i32[7, 8] as &[i32];
    take(arr) + take([5, 6])
}
"#,
    );
    let messages = checked
        .diagnostics
        .iter()
        .map(|diagnostic| diagnostic.summary.as_str())
        .collect::<Vec<_>>();
    assert!(
        messages
            .iter()
            .filter(|message| message.contains("expected &[i32], got [2]i32"))
            .count()
            >= 3,
        "{messages:?}"
    );
    assert!(
        messages
            .iter()
            .filter(|message| message.contains("invalid cast: cannot cast [2]i32 to &[i32]"))
            .count()
            >= 2,
        "{messages:?}"
    );
}

#[test]
fn coerces_mutable_references_and_slices_to_readonly_expected_types() {
    let checked = pipeline(
        r#"
fn read_ptr(x: &i32) i32 {
    x.*
}

fn read_slice(xs: &[i32]) i32 {
    xs[0]
}

fn generic_ptr[T](x: &T) T {
    x.*
}

fn generic_slice[T](xs: &[T]) T {
    xs[0]
}

fn main(mut_ptr: &mut i32, mut_slice: &mut [i32]) i32 {
    var ro_ptr: &i32 = mut_ptr;
    var ro_slice: &[i32] = mut_slice;
    read_ptr(mut_ptr)
        + read_slice(mut_slice)
        + generic_ptr(mut_ptr)
        + generic_slice(mut_slice)
        + ro_ptr.*
        + ro_slice[0]
}
"#,
    );
    assert!(checked.diagnostics.is_empty(), "{:?}", checked.diagnostics);
}

#[test]
fn rejects_readonly_references_and_slices_for_mutable_expected_types() {
    let checked = pipeline(
        r#"
fn write_ptr(x: &mut i32) void {
    x.* = 1;
}

fn write_slice(xs: &mut [i32]) void {
    xs[0] = 1;
}

fn main(ro_ptr: &i32, ro_slice: &[i32]) void {
    write_ptr(ro_ptr);
    write_slice(ro_slice);
}
"#,
    );
    assert!(
        checked
            .diagnostics
            .iter()
            .any(|diagnostic| diagnostic.summary.contains("expected &mut i32, got &i32")),
        "{:?}",
        checked.diagnostics
    );
    assert!(
        checked.diagnostics.iter().any(|diagnostic| diagnostic
            .summary
            .contains("expected &mut [i32], got &[i32]")),
        "{:?}",
        checked.diagnostics
    );
}

#[test]
fn rejects_array_pointer_to_element_pointer_coercions() {
    let checked = pipeline(
        r#"
fn main() void {
    var bytes: [4]u8 = [1, 2, 3, 0];
    var byte_ptr: &u8 = b"hello";
    var array_ptr: &u8 = bytes;
    _ = byte_ptr;
    _ = array_ptr;
}
"#,
    );
    let messages = checked
        .diagnostics
        .iter()
        .map(|diagnostic| diagnostic.summary.as_str())
        .collect::<Vec<_>>();
    assert!(
        messages
            .iter()
            .filter(|message| message.contains("expected &u8"))
            .count()
            >= 2,
        "{messages:?}"
    );
}
