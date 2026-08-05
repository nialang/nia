// SPDX-License-Identifier: GPL-3.0-or-later
use super::common::*;

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
    let mut one: Number = { value: 1 };
    let mut two: Number = { value: 2 };
    let mut three = one + two;
    let mut seven = three.add({ value: 4 });
    let mut nine = [Number]::add(seven, { value: 2 });
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
    let mut one: Number = { value: 1 };
    let mut two: Number = { value: 2 };
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
    let mut one: Number = { value: 1 };
    let mut two: Number = { value: 2 };
    let mut three = one + two;
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
    let mut a: i32 = neg[i32](1);
    let mut b: i32 = invert[i32](0);
    a + b
}
"#,
    );

    let program = check_program(root.join("main.nia").to_string_lossy().into_owned());
    assert!(program.diagnostics.is_empty(), "{:?}", program.diagnostics);
}

#[test]
fn builtin_not_trait_uses_non_keyword_method_name() {
    let root = temp_dir("builtin_not_trait_uses_non_keyword_method_name");
    write(
        &root.join("main.nia"),
        r#"
struct Flag {
    value: bool,
}

extend Flag : Not {
    fn logical_not(self) bool {
        not self.value
    }
}

fn flip[T](value: T) bool
where T: Not {
    not value
}

fn main() bool {
    flip(Flag { value: false })
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
    std::builtin::size[T]() + std::builtin::align[T]()
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
    std::builtin::size[T]()
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
        std::builtin::size[T]()
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
        std::builtin::size[U]()
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
        std::builtin::size[T]()
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
fn read_ptr[P](ptr: P) [P as Deref]::Target
where P: Deref {
    ptr.*
}

fn write_ptr[P](ptr: P, value: [P as DerefMut]::Target) void
where P: DerefMut {
    ptr.* = value;
}

fn read_index[C](items: C, index: usize) [C as Index[usize]]::Output
where C: Index[usize] {
    items[index]
}

fn write_index[C](items: C, index: usize, value: [C as IndexMut[usize]]::Output) void
where C: IndexMut[usize] {
    items[index] = value;
}

fn write_index_i32[C](items: C, index: i32, value: [C as IndexMut[i32]]::Output) void
where C: IndexMut[i32] {
    items[index] = value;
}

fn slice[S](items: S) [S as Slice[..]]::Output
where S: Slice[..] {
    & items[..]
}

fn slice_mut[S](items: S) [S as SliceMut[..]]::Output
where S: SliceMut[..] {
    &mut items[..]
}

fn main(ptr: &mut i32, ro: & [i32], rw: &mut [i32]) i32 {
    let mut x = read_ptr[&mut i32](ptr);
    write_ptr[&mut i32](ptr, x);
    let mut y = read_index[& [i32]](ro, 0);
    write_index[&mut [i32]](rw, 0, y);
    write_index_i32[&mut [i32]](rw, 0, y);
    let mut a = slice[& [i32]](ro);
    let mut b = slice_mut[&mut [i32]](rw);
    a.len() as i32 + b.len() as i32 + x + y
}
"##,
    );

    let program = check_program(root.join("main.nia").to_string_lossy().into_owned());
    assert!(program.diagnostics.is_empty(), "{:?}", program.diagnostics);
}

#[test]
fn len_method_and_builtin_ptr_traits_model_arrays_and_slices() {
    let root = temp_dir("len_method_and_builtin_ptr_traits_model_arrays_and_slices");
    write(
        &root.join("main.nia"),
        r##"
fn ptr_read_value[S](slice: S) [S as Ptr]::Target
where S: Ptr {
    let mut ptr = slice.ptr();
    ptr.*
}

fn ptr_value[S](slice: S) [S as PtrMut]::Target
where S: PtrMut {
    let mut ptr = slice.ptr_mut();
    ptr.*
}

fn main(slice: & [usize], slice_mut: &mut [usize]) usize {
    let mut array: [4]i32 = [1, 2, 3, 4];
    let mut literal_ptr = (&b"nia\0").ptr();
    array.len()
        + slice.len()
        + slice_mut.len()
        + ptr_read_value(slice)
        + ptr_value(slice_mut)
        + literal_ptr[0] as usize
}
"##,
    );

    let program = check_program(root.join("main.nia").to_string_lossy().into_owned());
    assert!(program.diagnostics.is_empty(), "{:?}", program.diagnostics);
}

#[test]
fn builtin_ptr_traits_do_not_apply_to_arrays() {
    let root = temp_dir("builtin_ptr_traits_do_not_apply_to_arrays");
    write(
        &root.join("main.nia"),
        r#"
fn main() i32 {
    let mut array: [4]i32 = [1, 2, 3, 4];
    array.ptr().*
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
        messages.iter().any(|message| message.contains("Deref")),
        "{:?}",
        program.diagnostics
    );
    assert!(
        messages.iter().any(|message| message.contains("Index")),
        "{:?}",
        program.diagnostics
    );
    assert!(
        messages.iter().any(|message| message.contains("Slice")),
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

extend Cell : Deref {
    type Target = i32;

    fn deref(& self) & i32 {
        & self.value
    }
}

extend Cell : DerefMut {
    type Target = i32;

    fn deref_mut(&mut self) &mut i32 {
        &mut self.value
    }
}

extend Cell : Index[usize] {
    type Output = i32;

    fn index(& self, index: usize) & i32 {
        & self.value
    }
}

extend Cell : IndexMut[usize] {
    type Output = i32;

    fn index_mut(&mut self, index: usize) &mut i32 {
        &mut self.value
    }
}

fn read_deref[P](value: P) [P as Deref]::Target
where P: Deref {
    value.*
}

fn write_deref[P](value: P, next: [P as DerefMut]::Target) void
where P: DerefMut {
    value.* = next;
}

fn read_index[C](value: C) [C as Index[usize]]::Output
where C: Index[usize] {
    value[0]
}

fn write_index[C](value: C, next: [C as IndexMut[usize]]::Output) void
where C: IndexMut[usize] {
    value[0] = next;
}

fn main() i32 {
    let mut cell: Cell = { value: 1 };
    let mut first = read_deref[Cell](cell);
    write_deref[Cell](cell, 3);
    let mut second = read_index[Cell](cell);
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

extend Cell : Index[usize] {
    type Output = i32;

    fn index(& self, index: usize) & i32 {
        & self.value
    }
}

extend Cell : Index[i32] {
    type Output = i32;

    fn index(& self, index: i32) & i32 {
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

extend Cell : DerefMut {
    type Target = i32;

    fn deref_mut(&mut self) &mut i32 {
        &mut self.value
    }
}

extend Cell : IndexMut[usize] {
    type Output = i32;

    fn index_mut(&mut self, index: usize) &mut i32 {
        &mut self.value
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
            .any(|message| message.contains("supertrait `Deref`")),
        "{:?}",
        program.diagnostics
    );
    assert!(
        messages
            .iter()
            .any(|message| message.contains("supertrait `Index`")),
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

static mut backing: [3]i32 = [1, 2, 3];

extend Cell : Slice[..] {
    type Output = & [i32];

    fn slice(& self, range: ..) & [i32] {
        & backing[..]
    }
}

fn take[T](value: T) [T as Slice[..]]::Output
where T: Slice[..] {
    & value[..]
}

fn main(cell: Cell) i32 {
    let mut part = take(cell);
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
extend[T] [T] : Ptr {
    type Target = T;

    fn ptr(& self) & T {
        self.ptr()
    }
}

extend[T] [4]T : Slice[..] {
    type Output = & [T];

    fn slice(& self, range: ..) & [T] {
        self.slice(range)
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
fn ordinary_len_trait_constrains_array_and_slice_length() {
    let root = temp_dir("ordinary_len_trait_constrains_array_and_slice_length");
    write(
        &root.join("main.nia"),
        r#"
fn len_of[T](value: T) usize
where T: Len {
    value.len()
}

fn main(slice: & [usize]) usize {
    let mut array = [1usize, 2usize, 3usize, 4usize];
    len_of(array) + slice.len()
}
"#,
    );

    let program = check_program(root.join("main.nia").to_string_lossy().into_owned());
    assert!(program.diagnostics.is_empty(), "{:?}", program.diagnostics);
}

#[test]
fn imported_generic_len_bound_uses_the_demand_loaded_source_trait() {
    let root = temp_dir("imported_generic_len_bound_uses_the_demand_loaded_source_trait");
    write(
        &root.join("api.nia"),
        r#"
pub fn lenOf[T](value: T) usize
where T: Len {
    value.len()
}
"#,
    );
    write(
        &root.join("main.nia"),
        r#"
module api;
using entry::api;

fn main(slice: &[u8]) usize {
    let array = [1u8, 2u8, 3u8];
    api::lenOf(array) + slice.len()
}
"#,
    );

    let program = check_program(root.join("main.nia").to_string_lossy().into_owned());
    assert!(program.diagnostics.is_empty(), "{:?}", program.diagnostics);
}

#[test]
fn unicode_char_api_models_checked_scalar_conversion() {
    let root = temp_dir("unicode_char_api_models_checked_scalar_conversion");
    write(
        &root.join("main.nia"),
        r#"
using std::unicode;

fn main() i32 {
    let a = switch unicode::fromScalarValue(65) {
        ?ch => {
            ch
        },
        null => {
            return 1;
        },
    };
    let b = switch unicode::fromScalarValue(0x10ffff) {
        ?ch => {
            ch
        },
        null => {
            return 2;
        },
    };
    if a.codepoint() != 65 or b.codepoint() != 0x10ffff {
        return 3;
    }
    switch unicode::fromScalarValue(0xd800) {
        ?ch => {
            _ = ch;
            return 4;
        },
        null => {},
    }
    0
}
"#,
    );

    let program = check_program(root.join("main.nia").to_string_lossy().into_owned());
    assert!(program.diagnostics.is_empty(), "{:?}", program.diagnostics);
}

#[test]
fn simd_builtins_execute_in_const_and_materialize_at_runtime() {
    let root = temp_dir("simd_builtins_execute_in_const_and_materialize_at_runtime");
    write(
        &root.join("main.nia"),
        r#"
const filled: u8x4 = std::builtin::splat[u8x4](3);
const changed: u8x4 = std::builtin::insert(filled, 2, 9);
const lane: u8 = std::builtin::extract(changed, 2);
const mask: boolx4 = std::builtin::insert(
    std::builtin::splat[boolx4](false),
    1,
    true,
);
const bits: usize = std::builtin::bitmask(mask);

fn main() bool {
    lane == 9
        and bits == 2
        and std::builtin::extract(changed, 0) == 3
        and std::builtin::extract(changed, 2) == 9
}
"#,
    );

    let program = check_program(root.join("main.nia").to_string_lossy().into_owned());
    assert!(program.diagnostics.is_empty(), "{:?}", program.diagnostics);
}

#[test]
fn const_simd_lane_access_rejects_out_of_range_indexes() {
    let root = temp_dir("const_simd_lane_access_rejects_out_of_range_indexes");
    write(
        &root.join("main.nia"),
        r#"
const invalid: u8 = std::builtin::extract(
    std::builtin::splat[u8x4](0),
    4,
);

fn main() u8 { invalid }
"#,
    );

    let program = check_program(root.join("main.nia").to_string_lossy().into_owned());
    assert!(
        program.diagnostics.iter().any(|diagnostic| diagnostic
            .diagnostic
            .summary
            .contains("lane index 4 is out of range for 4 lanes")),
        "{:?}",
        program.diagnostics
    );
}

#[test]
fn char_conversion_builtin_requires_u32() {
    let root = temp_dir("char_conversion_builtin_requires_u32");
    write(
        &root.join("main.nia"),
        r#"
fn main() ?char {
    std::builtin::charFromU32(65usize)
}
"#,
    );

    let program = check_program(root.join("main.nia").to_string_lossy().into_owned());
    assert!(
        program.diagnostics.iter().any(|diagnostic| {
            diagnostic
                .diagnostic
                .summary
                .contains("Unicode scalar value")
                && diagnostic.diagnostic.summary.contains("u32")
        }),
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
    let mut items: [4]i32 = [1, 2, 3, 4];
    let mut part = & items[0..2];
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
    let mut items: [4]i32 = [1, 2, 3, 4];
    let mut end: i32 = 2;
    let mut part = & items[0..end];
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
    let mut range: usize..usize = 0..2;
    0
}
"#,
    );

    let program = check_program(root.join("main.nia").to_string_lossy().into_owned());
    assert!(program.diagnostics.is_empty(), "{:?}", program.diagnostics);
}

#[test]
fn range_start_and_end_methods_expose_present_bounds() {
    let root = temp_dir("range_start_and_end_methods_expose_present_bounds");
    write(
        &root.join("main.nia"),
        r#"
fn main() usize {
    let exclusive = 2usize..5usize;
    let inclusive = 3usize..=7usize;
    let from = 3usize..;
    let to = ..7usize;
    let toInclusive = ..=9usize;
    exclusive.start() + exclusive.end()
        + inclusive.start() + inclusive.end()
        + from.start() + to.end() + toInclusive.end()
}
"#,
    );

    let program = check_program(root.join("main.nia").to_string_lossy().into_owned());
    assert!(program.diagnostics.is_empty(), "{:?}", program.diagnostics);
}

#[test]
fn range_bounds_only_expose_present_start_or_end() {
    let root = temp_dir("range_bounds_only_expose_present_start_or_end");
    write(
        &root.join("main.nia"),
        r#"
fn main() usize {
    let from = 1usize..;
    let to = ..2usize;
    _ = from.end();
    _ = to.start();
    0usize
}
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
                .contains("field access base is not a struct or union value or pointer"))
            .count(),
        2,
        "{:?}",
        program.diagnostics
    );
    assert!(
        program.diagnostics.iter().all(|diagnostic| !diagnostic
            .diagnostic
            .summary
            .contains("trait bound not satisfied")),
        "{:?}",
        program.diagnostics
    );
}

#[test]
fn ordinary_extension_takes_priority_over_builtin_range_method() {
    let root = temp_dir("ordinary_extension_takes_priority_over_builtin_range_method");
    write(
        &root.join("main.nia"),
        r#"
extend[T] T..T {
    fn start(&self, replacement: T) T {
        replacement
    }
}

fn main() usize {
    (1usize..2usize).start(9usize)
}
"#,
    );

    let program = check_program(root.join("main.nia").to_string_lossy().into_owned());
    assert!(program.diagnostics.is_empty(), "{:?}", program.diagnostics);
}

#[test]
fn ordinary_len_trait_allows_user_impls() {
    let root = temp_dir("ordinary_len_trait_allows_user_impls");
    write(
        &root.join("main.nia"),
        r#"
struct Window {
    lo: usize,
    hi: usize,
}

extend Window : Len {
    const fn len(& self) usize {
        self.hi - self.lo
    }
}

fn len_of[T](value: T) usize
where T: Len {
    value.len()
}

fn main() usize {
    let window: Window = { lo: 3usize, hi: 9usize };
    len_of(window)
}
"#,
    );

    let program = check_program(root.join("main.nia").to_string_lossy().into_owned());
    assert!(program.diagnostics.is_empty(), "{:?}", program.diagnostics);
}

#[test]
fn ordinary_len_trait_validates_method_signatures() {
    let root = temp_dir("ordinary_len_trait_validates_method_signatures");
    write(
        &root.join("main.nia"),
        r#"
struct BadLen {}

extend BadLen : Len {
    const fn len(self) i32 {
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
                "implementation of trait method `len` does not match the trait signature"
            )),
        "{:?}",
        program.diagnostics
    );
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
