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
            .ir
            .builtin_values
            .values()
            .any(|value| *value == BuiltinValue::Usize(8))
    );
    assert!(
        checked
            .ir
            .builtin_values
            .values()
            .any(|value| *value == BuiltinValue::Usize(4))
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
fn read(xs: &const [i32]) i32 {
    xs[0]
}

fn write(xs: &[i32]) i32 {
    xs[0] = 10;
    xs[0]
}

fn main() i32 {
    var xs: [4]i32 = [1, 2, 3, 4];
    var s = &const xs[..];
    var t = &const xs[1..=2];
    var p = &const xs[0];
    var single = &const p[..];
    _ = s.get_ptr_const();
    xs.len() as i32 + s.len() as i32 + t.len() as i32 + single.len() as i32 + read(s)
}
"#,
    );
    assert!(checked.diagnostics.is_empty(), "{:?}", checked.diagnostics);
}

#[test]
fn rejects_bare_range_index_and_readonly_slice_assignment() {
    let checked = pipeline(
        r#"
fn main(xs: &const [i32]) i32 {
    var y = xs[..];
    xs[0] = 1;
    0
}
"#,
    );
    assert!(
        checked.diagnostics.iter().any(|diagnostic| {
            diagnostic
                .message
                .contains("range index expression must be borrowed")
        }),
        "{:?}",
        checked.diagnostics
    );
    assert!(
        checked
            .diagnostics
            .iter()
            .any(|diagnostic| { diagnostic.message.contains("slice is const") }),
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
            .filter(|diagnostic| diagnostic.message.contains("array literal element"))
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
            .filter(|diagnostic| diagnostic.message.contains("array literal length mismatch"))
            .count(),
        2
    );
    assert!(
        !checked
            .diagnostics
            .iter()
            .any(|diagnostic| diagnostic.message.contains("binding initializer"))
    );
    assert!(
        !checked
            .diagnostics
            .iter()
            .any(|diagnostic| diagnostic.message.contains("call argument"))
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
            .filter(|diagnostic| diagnostic.message.contains("array literal element"))
            .count(),
        1,
        "{:?}",
        checked.diagnostics
    );
    assert!(
        !checked.diagnostics.iter().any(|diagnostic| diagnostic
            .message
            .contains("array literal requires an expected")),
        "{:?}",
        checked.diagnostics
    );
    assert!(
        !checked
            .diagnostics
            .iter()
            .any(|diagnostic| diagnostic.message.contains("call argument")),
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
            .message
            .contains("array repeat count is not a valid constant")
            && diagnostic.message.contains("division by zero")
    }));
}

#[test]
fn checks_large_array_repeat_count_from_comptime_binding() {
    let checked = pipeline(
        r#"
comptime N: usize = 1048576;

fn main() i32 {
    var buffer: [N]u8 = [0u8; N];
    0
}
"#,
    );
    assert!(checked.diagnostics.is_empty(), "{:?}", checked.diagnostics);
}

#[test]
fn checks_text_byte_and_c_string_literal_types() {
    let checked = pipeline(
        r#"
fn main() i32 {
    var text: [3]char = "中a\n";
    var adjacent_text: [9]char = "中" "" "a\n" "" "b" "c" "" "done";
    var inferred_text: [_]char = "hi";
    var multiline: [11]char =
        \\hello
        \\world
    ;
    var byte_multiline: [11]u8 =
        b\\hello
        \\world
    ;
    var c_multiline: [12]u8 =
        c\\hello
        \\world
    ;
    var bytes: [4]u8 = b"nia\0";
    var adjacent_bytes: [4]u8 = b"" b"n" b"" b"i" b"" b"a" b"" b"\0";
    var cstr: [4]u8 = c"nia";
    var adjacent_cstr: [4]u8 = c"" c"n" c"" c"i" c"" c"a" c"" c"";
    var wrong_text_len: [2]char = "中a\n";
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
            .filter(|diagnostic| diagnostic.message.contains("binding initializer"))
            .count(),
        3
    );
    assert!(
        checked
            .diagnostics
            .iter()
            .any(|diagnostic| diagnostic.message.contains("cannot cast u32 to char")),
        "{:?}",
        checked.diagnostics
    );
    assert!(
        !checked
            .diagnostics
            .iter()
            .any(|diagnostic| diagnostic.message.contains("text"))
    );
}

#[test]
fn checks_index_and_address_of_array_elements() {
    let checked = pipeline(
        r#"
extern fn puts(ptr: &u8) i32;

const hello = c"hello";

fn main(flag: bool) i32 {
    var xs: [2]u8 = [1, 2];
    var p: &u8 = &xs[0];
    var c: &u8 = &hello[0];
    _ = puts(&hello[0]);
    _ = xs[flag];
    0
}
"#,
    );
    assert!(
        !checked
            .diagnostics
            .iter()
            .any(|diagnostic| diagnostic.message.contains("binding initializer"))
    );
    assert!(
        !checked
            .diagnostics
            .iter()
            .any(|diagnostic| diagnostic.message.contains("call argument"))
    );
    assert!(
        checked
            .diagnostics
            .iter()
            .any(|diagnostic| diagnostic.message.contains("index"))
    );
}

#[test]
fn checks_array_to_slice_coercions_and_rvalue_reference_targets() {
    let checked = pipeline(
        r#"
fn take(xs: &const [i32]) i32 {
    xs.len() as i32
}

fn mutate(xs: &[i32]) i32 {
    xs[0] = 9;
    xs[0]
}

fn bytes(xs: &const [u8]) i32 {
    xs.len() as i32
}

fn main() i32 {
    var ro: &const [i32] = [1, 2, 3];
    var rw: &[i32] = [4, 5];
    var arr: [2]i32 = [6, 7];
    var from_place: &const [i32] = arr;
    var from_string: &const [u8] = c"hi";
    _ = take([1, 2, 3]);
    _ = mutate([4, 5]);
    _ = bytes(c"hi");

    var int_ptr: &i32 = &10;
    var sum_ptr: &i32 = &(1 + 2);
    var call_ptr: &i32 = &make();
    var temp_slice: &const [i32] = &const [1, 2, 3][..];
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
            .any(|diagnostic| diagnostic.message.contains("binding initializer")),
        "{:?}",
        checked.diagnostics
    );
    assert!(
        !checked
            .diagnostics
            .iter()
            .any(|diagnostic| diagnostic.message.contains("call argument")),
        "{:?}",
        checked.diagnostics
    );
    assert!(
        !checked
            .diagnostics
            .iter()
            .any(|diagnostic| diagnostic.message.contains("reference target")),
        "{:?}",
        checked.diagnostics
    );
    assert!(
        !checked
            .diagnostics
            .iter()
            .any(|diagnostic| diagnostic.message.contains("slice target")),
        "{:?}",
        checked.diagnostics
    );
    assert!(
        checked.ir.array_to_slice_coercions.len() >= 6,
        "{:?}",
        checked.ir.array_to_slice_coercions
    );
}

#[test]
fn checks_c_string_literal_pointer_coercions() {
    let checked = pipeline(
        r#"
extern fn printf(fmt: &const u8, ...);

fn takes_const(ptr: &const u8) i32 {
    ptr.* as i32
}

fn takes_mut(ptr: &u8) i32 {
    ptr.* = b'H';
    ptr.* as i32
}

fn main() i32 {
    var rw: &u8 = c"hello";
    var ro: &const u8 = c"world";
    var adjacent: &const u8 = c"hello, " c"world";
    _ = printf(c"hello, world\n");
    _ = printf(
        c"  #  Type      Offset             VirtAddr           FileSiz"
        c"            MemSiz             Flags Align\n"
    );
    _ = takes_const(
        c\\multi
        \\line
    );
    takes_mut(rw) + takes_const(ro) + takes_const(adjacent)
}
"#,
    );
    assert!(checked.diagnostics.is_empty(), "{:?}", checked.diagnostics);
    assert_eq!(checked.ir.c_string_pointer_coercions.len(), 6);
}

#[test]
fn rejects_non_c_string_literal_pointer_coercions() {
    let checked = pipeline(
        r#"
fn main() void {
    var bytes: [4]u8 = [1, 2, 3, 0];
    var byte_ptr: &const u8 = b"hello";
    var array_ptr: &const u8 = bytes;
    _ = byte_ptr;
    _ = array_ptr;
}
"#,
    );
    let messages = checked
        .diagnostics
        .iter()
        .map(|diagnostic| diagnostic.message.as_str())
        .collect::<Vec<_>>();
    assert!(
        messages
            .iter()
            .filter(|message| message.contains("expected &const u8"))
            .count()
            >= 2,
        "{messages:?}"
    );
    assert!(checked.ir.c_string_pointer_coercions.is_empty());
}
