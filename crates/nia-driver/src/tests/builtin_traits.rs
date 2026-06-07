// SPDX-License-Identifier: GPL-3.0-or-later
use super::common::*;
use crate::check_program;

#[test]
fn builtin_operator_traits_allow_constrained_generic_arithmetic() {
    let root = temp_dir("builtin_operator_traits_allow_constrained_generic_arithmetic");
    write(
        &root.join("main.nia"),
        r#"
fn add_same[T](a: T, b: T) T
where T: Add[T, Output = T] {
    a + b
}

fn add_method[T](a: T, b: T) T
where T: Add[T, Output = T] {
    a.add(b)
}

fn add_associated[T](a: T, b: T) T
where T: Add[T, Output = T] {
    [T]::add(a, b)
}

fn main() i32 {
    add_same[i32](1, 2) + add_method[i32](3, 4) + add_associated[i32](5, 6)
}
"#,
    );

    let program = check_program(root.join("main.nia").to_string_lossy().into_owned());
    assert!(program.diagnostics.is_empty(), "{:?}", program.diagnostics);
}

#[test]
fn builtin_operator_traits_reject_unconstrained_generic_arithmetic() {
    let root = temp_dir("builtin_operator_traits_reject_unconstrained_generic_arithmetic");
    write(
        &root.join("main.nia"),
        r#"
fn add_bad[T](a: T, b: T) T {
    a + b
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
fn builtin_operator_projection_can_name_primitive_output() {
    let root = temp_dir("builtin_operator_projection_can_name_primitive_output");
    write(
        &root.join("main.nia"),
        r#"
fn add_i32(a: i32, b: i32) [i32 as Add[i32]]::Output {
    a + b
}

fn main() i32 {
    add_i32(1, 2)
}
"#,
    );

    let program = check_program(root.join("main.nia").to_string_lossy().into_owned());
    assert!(program.diagnostics.is_empty(), "{:?}", program.diagnostics);
}

#[test]
fn builtin_operator_traits_allow_struct_operator_impls() {
    let root = temp_dir("builtin_operator_traits_allow_struct_operator_impls");
    write(
        &root.join("main.nia"),
        r#"
struct Number {
    value: i32,
}

extend Number : Add[Number] {
    type Output = Number;

    fn add(self, rhs: Number) Number {
        { value: self.value + rhs.value }
    }
}

fn main() i32 {
    var one: Number = { value: 1 };
    var two: Number = { value: 2 };
    var three = one + two;
    var seven = three.add({ value: 4 });
    var nine = [Number]::add(seven, { value: 2 });
    nine.value
}
"#,
    );

    let program = check_program(root.join("main.nia").to_string_lossy().into_owned());
    assert!(program.diagnostics.is_empty(), "{:?}", program.diagnostics);
}

#[test]
fn builtin_operator_method_names_do_not_shadow_plain_extension_methods() {
    let root = temp_dir("builtin_operator_method_names_do_not_shadow_plain_extension_methods");
    write(
        &root.join("main.nia"),
        r#"
struct Number {
    value: i32,
}

extend Number {
    fn add(self, rhs: Number) Number {
        { value: self.value + rhs.value + 10 }
    }
}

fn main() i32 {
    var one: Number = { value: 1 };
    var two: Number = { value: 2 };
    one.add(two).value
}
"#,
    );

    let program = check_program(root.join("main.nia").to_string_lossy().into_owned());
    assert!(program.diagnostics.is_empty(), "{:?}", program.diagnostics);
}

#[test]
fn builtin_operator_traits_reject_struct_operator_without_impl() {
    let root = temp_dir("builtin_operator_traits_reject_struct_operator_without_impl");
    write(
        &root.join("main.nia"),
        r#"
struct Number {
    value: i32,
}

fn main() i32 {
    var one: Number = { value: 1 };
    var two: Number = { value: 2 };
    var three = one + two;
    three.value
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
fn builtin_comparison_traits_allow_constrained_generic_comparison() {
    let root = temp_dir("builtin_comparison_traits_allow_constrained_generic_comparison");
    write(
        &root.join("main.nia"),
        r#"
fn same[T](a: T, b: T) bool
where T: Eq[T] {
    a == b
}

fn ordered[T](a: T, b: T) bool
where T: Ord[T] {
    a <= b
}

fn main() bool {
    same[i32](1, 1) and ordered[i32](1, 2)
}
"#,
    );

    let program = check_program(root.join("main.nia").to_string_lossy().into_owned());
    assert!(program.diagnostics.is_empty(), "{:?}", program.diagnostics);
}

#[test]
fn builtin_comparison_traits_reject_unconstrained_generic_comparison() {
    let root = temp_dir("builtin_comparison_traits_reject_unconstrained_generic_comparison");
    write(
        &root.join("main.nia"),
        r#"
fn same_bad[T](a: T, b: T) bool {
    a == b
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
fn builtin_unary_traits_allow_constrained_generic_unary_ops() {
    let root = temp_dir("builtin_unary_traits_allow_constrained_generic_unary_ops");
    write(
        &root.join("main.nia"),
        r#"
fn neg[T](value: T) [T as Neg]::Output
where T: Neg {
    -value
}

fn invert[T](value: T) [T as BitNot]::Output
where T: BitNot {
    ~value
}

fn main() i32 {
    var a: i32 = neg[i32](1);
    var b: i32 = invert[i32](0);
    a + b
}
"#,
    );

    let program = check_program(root.join("main.nia").to_string_lossy().into_owned());
    assert!(program.diagnostics.is_empty(), "{:?}", program.diagnostics);
}

#[test]
fn builtin_sized_trait_allows_generic_layout_builtins() {
    let root = temp_dir("builtin_sized_trait_allows_generic_layout_builtins");
    write(
        &root.join("main.nia"),
        r#"
fn bytes[T]() usize
where T: Sized {
    @size[T]() + @align[T]()
}

fn main() usize {
    bytes[i32]()
}
"#,
    );

    let program = check_program(root.join("main.nia").to_string_lossy().into_owned());
    assert!(program.diagnostics.is_empty(), "{:?}", program.diagnostics);
}

#[test]
fn builtin_sized_trait_rejects_unconstrained_generic_layout_builtins() {
    let root = temp_dir("builtin_sized_trait_rejects_unconstrained_generic_layout_builtins");
    write(
        &root.join("main.nia"),
        r#"
fn bytes[T]() usize {
    @size[T]()
}

fn main() usize {
    bytes[i32]()
}
"#,
    );

    let program = check_program(root.join("main.nia").to_string_lossy().into_owned());
    assert!(
        program
            .diagnostics
            .iter()
            .any(|diagnostic| diagnostic.diagnostic.summary.contains("requires T: Sized")),
        "{:?}",
        program.diagnostics
    );
}

#[test]
fn extend_where_clause_allows_generic_layout_builtins_in_methods() {
    let root = temp_dir("extend_where_clause_allows_generic_layout_builtins_in_methods");
    write(
        &root.join("main.nia"),
        r#"
struct ArrayList[T] {
    ptr: &mut T,
    len: usize,
}

extend[T] ArrayList[T]
where T: Sized {
    fn elem_size(& self) usize {
        _ = self;
        @size[T]()
    }
}

fn main(list: &ArrayList[i32]) usize {
    list.elem_size()
}
"#,
    );

    let program = check_program(root.join("main.nia").to_string_lossy().into_owned());
    assert!(program.diagnostics.is_empty(), "{:?}", program.diagnostics);
}

#[test]
fn method_where_clause_allows_extra_generic_layout_builtins_in_extension_methods() {
    let root =
        temp_dir("method_where_clause_allows_extra_generic_layout_builtins_in_extension_methods");
    write(
        &root.join("main.nia"),
        r#"
struct ArrayList[T] {
    ptr: &mut T,
    len: usize,
}

extend[T] ArrayList[T] {
    fn other_size[U](& self) usize
    where U: Sized {
        _ = self;
        @size[U]()
    }
}

fn main(list: &ArrayList[i32]) usize {
    list.other_size[u8]()
}
"#,
    );

    let program = check_program(root.join("main.nia").to_string_lossy().into_owned());
    assert!(program.diagnostics.is_empty(), "{:?}", program.diagnostics);
}

#[test]
fn struct_where_clause_allows_generic_layout_builtins_in_structural_extension_methods() {
    let root = temp_dir(
        "struct_where_clause_allows_generic_layout_builtins_in_structural_extension_methods",
    );
    write(
        &root.join("main.nia"),
        r#"
struct ArrayList[T]
where T: Sized {
    ptr: &mut T,
    len: usize,
}

extend[T] ArrayList[T] {
    fn elem_size(& self) usize {
        _ = self;
        @size[T]()
    }
}

fn main(list: &ArrayList[i32]) usize {
    list.elem_size()
}
"#,
    );

    let program = check_program(root.join("main.nia").to_string_lossy().into_owned());
    assert!(program.diagnostics.is_empty(), "{:?}", program.diagnostics);
}

#[test]
fn builtin_place_traits_allow_constrained_generic_deref_index_and_slice() {
    let root = temp_dir("builtin_place_traits_allow_constrained_generic_deref_index_and_slice");
    write(
        &root.join("main.nia"),
        r##"
fn read_ptr[P](ptr: P) [P as DerefRead]::Target
where P: DerefRead {
    ptr.*
}

fn write_ptr[P](ptr: P, value: [P as Deref]::Target) void
where P: Deref {
    ptr.* = value;
}

fn read_index[C](items: C, index: usize) [C as IndexRead[usize]]::Output
where C: IndexRead[usize] {
    items[index]
}

fn write_index[C](items: C, index: usize, value: [C as Index[usize]]::Output) void
where C: Index[usize] {
    items[index] = value;
}

fn write_index_i32[C](items: C, index: i32, value: [C as Index[i32]]::Output) void
where C: Index[i32] {
    items[index] = value;
}

fn slice_read[S](items: S) [S as SliceRead[..]]::Output
where S: SliceRead[..] {
    & items[..]
}

fn slice_mut[S](items: S) [S as Slice[..]]::Output
where S: Slice[..] {
    &mut items[..]
}

fn main(ptr: &mut i32, ro: & [i32], rw: &mut [i32]) i32 {
    var x = read_ptr[&mut i32](ptr);
    write_ptr[&mut i32](ptr, x);
    var y = read_index[& [i32]](ro, 0);
    write_index[&mut [i32]](rw, 0, y);
    write_index_i32[&mut [i32]](rw, 0, y);
    var a = slice_read[& [i32]](ro);
    var b = slice_mut[&mut [i32]](rw);
    a.len() as i32 + b.len() as i32 + x + y
}
"##,
    );

    let program = check_program(root.join("main.nia").to_string_lossy().into_owned());
    assert!(program.diagnostics.is_empty(), "{:?}", program.diagnostics);
}

#[test]
fn builtin_len_method_and_get_ptr_traits_model_arrays_and_slices() {
    let root = temp_dir("builtin_len_method_and_get_ptr_traits_model_arrays_and_slices");
    write(
        &root.join("main.nia"),
        r##"
fn ptr_read_value[S](slice: S) [S as GetPtrRead]::Target
where S: GetPtrRead {
    var ptr = slice.get_ptr_read();
    ptr.*
}

fn ptr_value[S](slice: S) [S as GetPtr]::Target
where S: GetPtr {
    var ptr = slice.get_ptr();
    ptr.*
}

fn main(slice_read: & [usize], slice_mut: &mut [usize]) usize {
    var array: [4]i32 = [1, 2, 3, 4];
    array.len()
        + slice_read.len()
        + slice_mut.len()
        + ptr_read_value(slice_read)
        + ptr_value(slice_mut)
}
"##,
    );

    let program = check_program(root.join("main.nia").to_string_lossy().into_owned());
    assert!(program.diagnostics.is_empty(), "{:?}", program.diagnostics);
}

#[test]
fn builtin_get_ptr_traits_do_not_apply_to_arrays() {
    let root = temp_dir("builtin_get_ptr_traits_do_not_apply_to_arrays");
    write(
        &root.join("main.nia"),
        r#"
fn main() i32 {
    var array: [4]i32 = [1, 2, 3, 4];
    array.get_ptr_read().*
}
"#,
    );

    let program = check_program(root.join("main.nia").to_string_lossy().into_owned());
    assert!(
        program.diagnostics.iter().any(|diagnostic| diagnostic
            .diagnostic
            .summary
            .contains("field access base is not a struct or union value or pointer")),
        "{:?}",
        program.diagnostics
    );
}

#[test]
fn builtin_place_traits_reject_unconstrained_generic_operations() {
    let root = temp_dir("builtin_place_traits_reject_unconstrained_generic_operations");
    write(
        &root.join("main.nia"),
        r##"
fn bad_deref[T](value: T) void {
    _ = value.*;
}

fn bad_index[T](value: T) void {
    _ = value[0];
}

fn bad_slice[T](value: T) void {
    _ = & value[..];
}

fn main() i32 { 0 }
"##,
    );

    let program = check_program(root.join("main.nia").to_string_lossy().into_owned());
    let messages = program
        .diagnostics
        .iter()
        .map(|diagnostic| diagnostic.diagnostic.summary.as_str())
        .collect::<Vec<_>>();
    assert!(
        messages.iter().any(|message| message.contains("DerefRead")),
        "{:?}",
        program.diagnostics
    );
    assert!(
        messages.iter().any(|message| message.contains("IndexRead")),
        "{:?}",
        program.diagnostics
    );
    assert!(
        messages.iter().any(|message| message.contains("SliceRead")),
        "{:?}",
        program.diagnostics
    );
}

#[test]
fn builtin_deref_and_index_traits_allow_user_place_overloads() {
    let root = temp_dir("builtin_deref_and_index_traits_allow_user_place_overloads");
    write(
        &root.join("main.nia"),
        r##"
struct Cell {
    value: i32,
}

extend Cell : DerefRead {
    type Target = i32;

    fn deref_read(& self) & i32 {
        & self.value
    }
}

extend Cell : Deref {
    type Target = i32;

    fn deref(&mut self) &mut i32 {
        &mut self.value
    }
}

extend Cell : IndexRead[usize] {
    type Output = i32;

    fn index_read(& self, index: usize) & i32 {
        & self.value
    }
}

extend Cell : Index[usize] {
    type Output = i32;

    fn index(&mut self, index: usize) &mut i32 {
        &mut self.value
    }
}

fn read_deref[P](value: P) [P as DerefRead]::Target
where P: DerefRead {
    value.*
}

fn write_deref[P](value: P, next: [P as Deref]::Target) void
where P: Deref {
    value.* = next;
}

fn read_index[C](value: C) [C as IndexRead[usize]]::Output
where C: IndexRead[usize] {
    value[0]
}

fn write_index[C](value: C, next: [C as Index[usize]]::Output) void
where C: Index[usize] {
    value[0] = next;
}

fn main() i32 {
    var cell: Cell = { value: 1 };
    var first = read_deref[Cell](cell);
    write_deref[Cell](cell, 3);
    var second = read_index[Cell](cell);
    write_index[Cell](cell, 5);
    first + second + cell.value
}
"##,
    );

    let program = check_program(root.join("main.nia").to_string_lossy().into_owned());
    assert!(program.diagnostics.is_empty(), "{:?}", program.diagnostics);
}

#[test]
fn index_literal_inference_rejects_ambiguous_index_bounds() {
    let root = temp_dir("index_literal_inference_rejects_ambiguous_index_bounds");
    write(
        &root.join("main.nia"),
        r##"
struct Cell {
    value: i32,
}

extend Cell : IndexRead[usize] {
    type Output = i32;

    fn index_read(& self, index: usize) & i32 {
        & self.value
    }
}

extend Cell : IndexRead[i32] {
    type Output = i32;

    fn index_read(& self, index: i32) & i32 {
        & self.value
    }
}

fn read_index(value: Cell) i32 {
    value[0]
}

fn main() i32 {
    read_index({ value: 1 })
}
"##,
    );

    let program = check_program(root.join("main.nia").to_string_lossy().into_owned());
    assert!(
        program.diagnostics.iter().any(|diagnostic| diagnostic
            .diagnostic
            .summary
            .contains("ambiguous index literal type")),
        "{:?}",
        program.diagnostics
    );
}

#[test]
fn mutable_builtin_place_traits_require_read_supertrait_impls() {
    let root = temp_dir("mutable_builtin_place_traits_require_read_supertrait_impls");
    write(
        &root.join("main.nia"),
        r#"
struct Cell {
    value: i32,
}

extend Cell : Deref {
    type Target = i32;

    fn deref(&self) &i32 {
        &self.value
    }
}

extend Cell : Index[usize] {
    type Output = i32;

    fn index(&self, index: usize) &i32 {
        &self.value
    }
}

fn main() i32 { 0 }
"#,
    );

    let program = check_program(root.join("main.nia").to_string_lossy().into_owned());
    let messages = program
        .diagnostics
        .iter()
        .map(|diagnostic| diagnostic.diagnostic.summary.as_str())
        .collect::<Vec<_>>();
    assert!(
        messages
            .iter()
            .any(|message| message.contains("supertrait `DerefRead`")),
        "{:?}",
        program.diagnostics
    );
    assert!(
        messages
            .iter()
            .any(|message| message.contains("supertrait `IndexRead`")),
        "{:?}",
        program.diagnostics
    );
}

#[test]
fn builtin_slice_traits_allow_user_container_impls() {
    let root = temp_dir("builtin_slice_traits_allow_user_container_impls");
    write(
        &root.join("main.nia"),
        r#"
struct Cell {}

var backing: [3]i32 = [1, 2, 3];

extend Cell : SliceRead[..] {
    type Output = & [i32];

    fn slice_read(& self, range: ..) & [i32] {
        & backing[..]
    }
}

fn take[T](value: T) [T as SliceRead[..]]::Output
where T: SliceRead[..] {
    & value[..]
}

fn main(cell: Cell) i32 {
    var part = take(cell);
    part.len() as i32
}
"#,
    );

    let program = check_program(root.join("main.nia").to_string_lossy().into_owned());
    assert!(program.diagnostics.is_empty(), "{:?}", program.diagnostics);
}

#[test]
fn builtin_trait_impls_cannot_overlap_compiler_proven_impls() {
    let root = temp_dir("builtin_trait_impls_cannot_overlap_compiler_proven_impls");
    write(
        &root.join("main.nia"),
        r#"
extend[T] [T] : GetPtrRead {
    type Target = T;

    fn get_ptr_read(& self) & T {
        self.get_ptr_read()
    }
}

extend[T] [4]T : SliceRead[..] {
    type Output = & [T];

    fn slice_read(& self, range: ..) & [T] {
        self.slice_read(range)
    }
}

fn main() i32 { 0 }
"#,
    );

    let program = check_program(root.join("main.nia").to_string_lossy().into_owned());
    let overlap_count = program
        .diagnostics
        .iter()
        .filter(|diagnostic| {
            diagnostic
                .diagnostic
                .summary
                .contains("overlaps a compiler-proven implementation")
        })
        .count();
    assert_eq!(overlap_count, 1, "{:?}", program.diagnostics);
}

#[test]
fn len_is_not_a_builtin_trait() {
    let root = temp_dir("len_is_not_a_builtin_trait");
    write(
        &root.join("main.nia"),
        r#"
fn len_of[T](value: T) usize
where T: Len {
    value.len()
}

fn main() usize {
    var array = [1usize, 2usize, 3usize, 4usize];
    len_of(array)
}
"#,
    );

    let program = check_program(root.join("main.nia").to_string_lossy().into_owned());
    assert!(
        program
            .diagnostics
            .iter()
            .any(|diagnostic| { diagnostic.diagnostic.summary.contains("unknown type `Len`") }),
        "{:?}",
        program.diagnostics
    );
}

#[test]
fn builtin_slice_ranges_infer_usize_bounds() {
    let root = temp_dir("builtin_slice_ranges_infer_usize_bounds");
    write(
        &root.join("main.nia"),
        r#"
fn main() i32 {
    var items: [4]i32 = [1, 2, 3, 4];
    var part = & items[0..2];
    part.len() as i32
}
"#,
    );

    let program = check_program(root.join("main.nia").to_string_lossy().into_owned());
    assert!(program.diagnostics.is_empty(), "{:?}", program.diagnostics);
}

#[test]
fn builtin_slice_ranges_require_usize_bounds() {
    let root = temp_dir("builtin_slice_ranges_require_usize_bounds");
    write(
        &root.join("main.nia"),
        r#"
fn main() i32 {
    var items: [4]i32 = [1, 2, 3, 4];
    var end: i32 = 2;
    var part = & items[0..end];
    part.len() as i32
}
"#,
    );

    let program = check_program(root.join("main.nia").to_string_lossy().into_owned());
    assert!(
        program
            .diagnostics
            .iter()
            .any(|diagnostic| diagnostic.diagnostic.summary.contains("slice range end")),
        "{:?}",
        program.diagnostics
    );
}

#[test]
fn range_expressions_are_runtime_values() {
    let root = temp_dir("range_expressions_are_runtime_values");
    write(
        &root.join("main.nia"),
        r#"
fn main() i32 {
    var range: usize..usize = 0..2;
    0
}
"#,
    );

    let program = check_program(root.join("main.nia").to_string_lossy().into_owned());
    assert!(program.diagnostics.is_empty(), "{:?}", program.diagnostics);
}

#[test]
fn builtin_sized_trait_cannot_be_implemented_manually() {
    let root = temp_dir("builtin_sized_trait_cannot_be_implemented_manually");
    write(
        &root.join("main.nia"),
        r#"
struct Number {
    value: i32,
}

extend Number : Sized {}

fn main() i32 { 0 }
"#,
    );

    let program = check_program(root.join("main.nia").to_string_lossy().into_owned());
    assert!(
        program.diagnostics.iter().any(|diagnostic| diagnostic
            .diagnostic
            .summary
            .contains("overlaps a compiler-proven implementation")),
        "{:?}",
        program.diagnostics
    );
}

#[test]
fn builtin_unsized_trait_cannot_be_implemented_manually() {
    let root = temp_dir("builtin_unsized_trait_cannot_be_implemented_manually");
    write(
        &root.join("main.nia"),
        r#"
struct Number {
    value: i32,
}

extend Number : Unsized {}

fn main() i32 { 0 }
"#,
    );

    let program = check_program(root.join("main.nia").to_string_lossy().into_owned());
    assert!(
        program.diagnostics.iter().any(|diagnostic| diagnostic
            .diagnostic
            .summary
            .contains("overlaps a compiler-proven implementation")),
        "{:?}",
        program.diagnostics
    );
}

#[test]
fn builtin_iterator_requires_optional_item_return() {
    let root = temp_dir("builtin_iterator_requires_optional_item_return");
    write(
        &root.join("main.nia"),
        r#"
struct Counter {}

extend Counter : Iterator {
    type Item = i32;

    fn next(&mut self) i32 {
        1
    }
}

fn main() i32 { 0 }
"#,
    );

    let program = check_program(root.join("main.nia").to_string_lossy().into_owned());
    assert!(
        program
            .diagnostics
            .iter()
            .any(|diagnostic| diagnostic.diagnostic.summary.contains(
                "implementation of trait method `next` does not match the trait signature"
            )),
        "{:?}",
        program.diagnostics
    );
}

#[test]
fn builtin_iterator_requires_mutable_receiver() {
    let root = temp_dir("builtin_iterator_requires_mutable_receiver");
    write(
        &root.join("main.nia"),
        r#"
struct Counter {}

extend Counter : Iterator {
    type Item = i32;

    fn next(&self) ?i32 {
        null
    }
}

fn main() i32 { 0 }
"#,
    );

    let program = check_program(root.join("main.nia").to_string_lossy().into_owned());
    assert!(
        program
            .diagnostics
            .iter()
            .any(|diagnostic| diagnostic.diagnostic.summary.contains(
                "implementation of trait method `next` does not match the trait signature"
            )),
        "{:?}",
        program.diagnostics
    );
}
