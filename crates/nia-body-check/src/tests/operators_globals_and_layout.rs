// SPDX-License-Identifier: GPL-3.0-or-later
use super::common::*;

fn integer_literal_tys(expr: &nia_body_ir::TypedExpr, out: &mut Vec<nia_ids::InternedTyId>) {
    match &expr.kind {
        nia_body_ir::TypedExprKind::Integer(_) => out.push(expr.ty),
        nia_body_ir::TypedExprKind::Call { args, .. } => {
            for arg in args {
                integer_literal_tys(arg, out);
            }
        }
        nia_body_ir::TypedExprKind::Cast { expr, .. }
        | nia_body_ir::TypedExprKind::Unary { expr, .. } => integer_literal_tys(expr, out),
        _ => {}
    }
}

fn main_tail_integer_literal_tys(checked: &BodyCheck) -> Vec<nia_ids::InternedTyId> {
    let body = checked
        .ir
        .function_bodies
        .values()
        .next()
        .expect("main body");
    let mut out = Vec::new();
    integer_literal_tys(body.tail.as_deref().expect("main tail"), &mut out);
    out
}

#[test]
fn checks_unary_operator_builtin_traits() {
    let checked = pipeline(
        r#"
fn main(flag: bool, bits: u32, x: i32) bool {
    let mut neg = -x;
    let mut flipped = ~bits;
    let mut logical = not flag;
    neg < 0 and flipped != bits and logical
}
"#,
    );
    assert!(checked.diagnostics.is_empty(), "{:?}", checked.diagnostics);
}

#[test]
fn lowers_const_value_binary_operator_suffix_literal_types() {
    let checked = pipeline(
        r#"
const group_width: usize = 8usize;

fn main() usize {
    group_width - 1usize
}
"#,
    );
    assert!(checked.diagnostics.is_empty(), "{:?}", checked.diagnostics);
    let usize_ty = checked
        .type_store
        .append_for_module(checked.module_id)
        .intern(nia_ty::TyKind::Primitive(nia_ty::PrimitiveTy::Usize));
    assert_eq!(main_tail_integer_literal_tys(&checked), vec![usize_ty]);
}

#[test]
fn lowers_negative_suffix_literal_cast_source_type() {
    let checked = pipeline(
        r#"
fn main() usize {
    (-1isize) as usize
}
"#,
    );
    assert!(checked.diagnostics.is_empty(), "{:?}", checked.diagnostics);
    let isize_ty = checked
        .type_store
        .append_for_module(checked.module_id)
        .intern(nia_ty::TyKind::Primitive(nia_ty::PrimitiveTy::Isize));
    assert_eq!(main_tail_integer_literal_tys(&checked), vec![isize_ty]);
}

#[test]
fn infers_unsuffixed_left_literal_from_binary_peer() {
    let checked = pipeline(
        r#"
fn divide(size: usize) usize {
    64 / size
}

fn compare(size: usize) bool {
    1 < size
}

fn equal(size: usize) bool {
    1 == size
}

fn divideFloat(value: f32) f32 {
    1.0 / value
}
"#,
    );
    assert!(checked.diagnostics.is_empty(), "{:?}", checked.diagnostics);
}

#[test]
fn does_not_infer_numeric_literal_from_non_numeric_binary_peer() {
    let checked = pipeline(
        r#"
fn main(flag: bool) bool {
    1 == flag
}
"#,
    );
    assert_eq!(checked.diagnostics.len(), 1, "{:?}", checked.diagnostics);
    assert!(
        checked.diagnostics[0]
            .summary
            .contains("trait bound not satisfied: i32: Eq[bool]"),
        "{:?}",
        checked.diagnostics
    );
}

#[test]
fn explicit_left_numeric_suffix_is_not_overridden_by_binary_peer() {
    let checked = pipeline(
        r#"
fn main(size: usize) u32 {
    64u32 / size
}
"#,
    );
    assert!(
        checked.diagnostics.iter().any(|diagnostic| diagnostic
            .summary
            .contains("trait bound not satisfied: u32: Div[usize]")),
        "{:?}",
        checked.diagnostics
    );
}

#[test]
fn checks_vector_operator_builtin_traits() {
    let checked = pipeline(
        r#"
fn add(lhs: u8x16, rhs: u8x16) u8x16 {
    lhs + rhs
}

fn bitwise(lhs: boolx16, rhs: boolx16) boolx16 {
    lhs & rhs
}

fn compare(lhs: f32x4, rhs: f32x4) boolx4 {
    lhs < rhs
}
"#,
    );

    assert!(checked.diagnostics.is_empty(), "{:?}", checked.diagnostics);
}

#[test]
fn rejects_unary_operators_without_builtin_trait_impls() {
    let checked = pipeline(
        r#"
fn main(flag: bool, x: i32) bool {
    let mut bad_bits = ~flag;
    let mut bad_not = not x;
    bad_bits or bad_not
}
"#,
    );
    assert!(
        checked.diagnostics.iter().any(|diagnostic| diagnostic
            .summary
            .contains("trait bound not satisfied: bool: BitNot")),
        "{:?}",
        checked.diagnostics
    );
    assert!(
        checked.diagnostics.iter().any(|diagnostic| diagnostic
            .summary
            .contains("trait bound not satisfied: i32: Not")),
        "{:?}",
        checked.diagnostics
    );
}

#[test]
fn char_supports_builtin_equality_and_ordering() {
    let checked = pipeline(
        r#"
fn main(a: char, b: char, n: u32) bool {
    let mut eq = a == b;
    let mut ne = a != b;
    let mut lt = a < b;
    let mut le = a <= b;
    let mut gt = a > b;
    let mut ge = a >= b;
    let mut bad = a == n;
    eq or ne or lt or le or gt or ge or bad
}
"#,
    );
    assert!(
        checked
            .diagnostics
            .iter()
            .any(|diagnostic| diagnostic.summary.contains("char: Eq[u32]")),
        "{:?}",
        checked.diagnostics
    );
    assert_eq!(
        checked
            .diagnostics
            .iter()
            .filter(|diagnostic| diagnostic.summary.contains("char: Eq[char]")
                || diagnostic.summary.contains("char: Ord[char]"))
            .count(),
        0,
        "{:?}",
        checked.diagnostics
    );
}

#[test]
fn generic_layout_builtins_require_sized_bound() {
    let checked = pipeline(
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
    assert!(checked.diagnostics.is_empty(), "{:?}", checked.diagnostics);

    let rejected = pipeline(
        r#"
fn bytes[T]() usize {
    std::builtin::size[T]()
}

fn main() usize {
    bytes[i32]()
}
"#,
    );
    assert!(
        rejected
            .diagnostics
            .iter()
            .any(|diagnostic| diagnostic.summary.contains("requires T: Sized")),
        "{:?}",
        rejected.diagnostics
    );
}

#[test]
fn checks_open_enum_integer_casts_and_switch_exhaustiveness() {
    let checked = pipeline(
        r#"
enum Flag: u8 {
    A,
    B,
    _,
}

enum Closed {
    A,
    B,
}

fn open_cast() Flag {
    3 as Flag
}

fn bad_open_cast() Flag {
    256 as Flag
}

fn closed_cast() Closed {
    3 as Closed
}

fn missing_open_default(flag: Flag) i32 {
    switch flag {
        Flag::A => return 1,
        Flag::B => return 2,
    }
    0
}

fn with_open_default(flag: Flag) i32 {
    switch flag {
        Flag::A => return 1,
        Flag::B => return 2,
        3 => return 3,
        256 => return 4,
        _ => return 0,
    }
    0
}

fn closed_integer_pattern(closed: Closed) i32 {
    switch closed {
        0 => return 0,
        _ => return 1,
    }
    0
}
"#,
    );
    assert_eq!(
        checked
            .diagnostics
            .iter()
            .filter(|diagnostic| diagnostic.summary.contains("invalid cast"))
            .count(),
        1,
        "{:?}",
        checked.diagnostics
    );
    assert_eq!(
        checked
            .diagnostics
            .iter()
            .filter(|diagnostic| diagnostic
                .summary
                .contains("non-exhaustive open enum switch"))
            .count(),
        1,
        "{:?}",
        checked.diagnostics
    );
    assert!(
        !checked
            .diagnostics
            .iter()
            .any(|diagnostic| diagnostic.summary.contains("i32 to Flag")),
        "{:?}",
        checked.diagnostics
    );
    assert_eq!(
        checked
            .diagnostics
            .iter()
            .filter(|diagnostic| diagnostic
                .summary
                .contains("out of range for Flag backing type"))
            .count(),
        2,
        "{:?}",
        checked.diagnostics
    );
    assert!(
        checked.diagnostics.iter().any(|diagnostic| diagnostic
            .summary
            .contains("type mismatch in switch pattern")),
        "{:?}",
        checked.diagnostics
    );
}

#[test]
fn checks_shift_operators() {
    let checked = pipeline(
        r#"
fn main(flag: bool, wide: u128, count: u32) i32 {
    let mut x = 1 << 3;
    let mut y = x >> 1;
    let mut high = wide >> count;
    let mut z = x << flag;
    let mut bad = flag << 1;
    _ = high;
    y + z + bad
}
"#,
    );
    assert!(
        checked.diagnostics.iter().any(|diagnostic| diagnostic
            .summary
            .contains("shift count must be an integer type, got bool")),
        "{:?}",
        checked.diagnostics
    );
    assert!(
        checked.diagnostics.iter().any(|diagnostic| diagnostic
            .summary
            .contains("trait bound not satisfied: bool: Shl[bool]")),
        "{:?}",
        checked.diagnostics
    );
}

#[test]
fn checks_global_initializers_and_inferred_global_types() {
    let checked = pipeline(
        r#"
static mut counter = 1;
static mut flag = true;
static limit = 10;
static mut bad: bool = 1;

fn main() i32 {
    counter = counter + 1;
    limit = 11;
    if flag { counter } else { limit }
}
"#,
    );
    assert!(
        checked
            .diagnostics
            .iter()
            .any(|diagnostic| diagnostic.summary.contains("global initializer"))
    );
    assert!(
        checked
            .diagnostics
            .iter()
            .any(|diagnostic| diagnostic.summary.contains("static is immutable"))
    );
    assert!(
        !checked
            .diagnostics
            .iter()
            .any(|diagnostic| diagnostic.summary.contains("global type is not available"))
    );
}
