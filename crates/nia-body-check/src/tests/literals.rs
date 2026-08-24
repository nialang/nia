// SPDX-License-Identifier: GPL-3.0-or-later
use super::common::*;

#[test]
fn checks_integer_literal_ranges_against_expected_types() {
    let checked = pipeline(
        r#"
struct Bytes {
    first: u8,
}

fn take_byte(x: u8) u8 { x }

fn ret_good() u8 { 255 }
fn ret_bad() u8 { 256 }

fn main() u8 {
    let mut ok: u8 = 255;
    let mut too_large: u8 = 256;
    let mut negative: u8 = -1;
    let mut xs: [u8; 2] = [0, 256];
    let mut b = Bytes { first: 300 };
    _ = take_byte(128);
    _ = take_byte(999);
    ret_good()
}
"#,
    );
    let range_errors = checked
        .diagnostics
        .iter()
        .filter(|diagnostic| diagnostic.summary.contains("out of range for u8"))
        .count();
    assert_eq!(range_errors, 6, "{:?}", checked.diagnostics);
    assert!(
        !checked
            .diagnostics
            .iter()
            .any(|diagnostic| diagnostic.summary.contains("type mismatch"))
    );
}

#[test]
fn accepts_i128_min_with_contextual_and_explicit_literal_types() {
    let checked = pipeline(
        r#"
const CONTEXTUAL_MIN: i128 = -170141183460469231731687303715884105728;
const EXPLICIT_MIN: i128 = -170141183460469231731687303715884105728i128;

fn tooSmall() () {
    let value = -170141183460469231731687303715884105729i128;
}

fn main() i128 {
    CONTEXTUAL_MIN + EXPLICIT_MIN
}
"#,
    );

    let range_errors = checked
        .diagnostics
        .iter()
        .filter(|diagnostic| diagnostic.summary.contains("out of range for i128"))
        .count();
    assert_eq!(range_errors, 1, "{:?}", checked.diagnostics);
}

#[test]
fn checks_float_literals_against_expected_float_types() {
    let checked = pipeline(
        r#"
fn take32(x: f32) f32 { x }

fn main() f64 {
    let mut a: f32 = 1.5;
    let mut b: f64 = 1e3;
    let mut too_large: f32 = 1e100;
    let mut wrong: i32 = 1.5;
    _ = take32(2.5);
    _ = take32(1e100);
    b
}
"#,
    );
    assert!(
        checked
            .diagnostics
            .iter()
            .any(|diagnostic| diagnostic.summary.contains("out of range for F32")),
        "{:?}",
        checked.diagnostics
    );
    assert!(
        checked
            .diagnostics
            .iter()
            .any(|diagnostic| diagnostic.summary.contains("binding initializer")),
        "{:?}",
        checked.diagnostics
    );
    assert!(
        checked
            .diagnostics
            .iter()
            .any(|diagnostic| diagnostic.summary.contains("call argument")),
        "{:?}",
        checked.diagnostics
    );
}

#[test]
fn defaults_unconstrained_numeric_literals_and_honors_expected_types() {
    let checked = pipeline(
        r#"
fn take_byte(x: u8) u8 { x }
fn take32(x: f32) f32 { x }

fn main() i32 {
    let mut default_int = 10;
    let mut default_float = 1.5;
    let mut explicit_byte: u8 = 10;
    let mut negative_byte: i8 = -1;
    let mut explicit_float: f32 = 1.5;
    _ = take_byte(3);
    _ = take32(2.5);
    default_int
}
"#,
    );
    assert!(checked.diagnostics.is_empty(), "{:?}", checked.diagnostics);
    let local_types = checked
        .facts
        .function_facts
        .values()
        .flat_map(|facts| facts.local_types.values())
        .copied()
        .collect::<Vec<_>>();
    assert!(
        local_types.iter().any(|ty| checked.type_store.get(*ty)
            == Some(&nia_ty::TyKind::Primitive(nia_ty::PrimitiveTy::I32))),
        "{:?}",
        local_types
    );
    assert!(
        local_types.iter().any(|ty| checked.type_store.get(*ty)
            == Some(&nia_ty::TyKind::Primitive(nia_ty::PrimitiveTy::F64))),
        "{:?}",
        local_types
    );
    assert!(
        local_types.iter().any(|ty| checked.type_store.get(*ty)
            == Some(&nia_ty::TyKind::Primitive(nia_ty::PrimitiveTy::U8))),
        "{:?}",
        local_types
    );
    assert!(
        local_types.iter().any(|ty| checked.type_store.get(*ty)
            == Some(&nia_ty::TyKind::Primitive(nia_ty::PrimitiveTy::F32))),
        "{:?}",
        local_types
    );
}

#[test]
fn infers_binary_numeric_literals_from_the_other_operand() {
    let checked = pipeline(
        r#"
static a = 10;
static ptr = & a;

fn main(x: usize) bool {
    let mut forward = a as usize == 0;
    let mut reverse = 0 == a as usize;
    let mut sum: usize = 1 + x;
    let mut expected_sum: usize = 1 + 2;
    let mut shifted: usize = 1 << 2;
    forward and reverse and sum == x + 1 and expected_sum == 3 and shifted == 4
}
"#,
    );
    assert!(checked.diagnostics.is_empty(), "{:?}", checked.diagnostics);
}

#[test]
fn infers_if_branch_numeric_literals_from_expected_and_peer_types() {
    let checked = pipeline(
        r#"
fn from_return(flag: bool) usize {
    if flag { 1 } else { 2 }
}

fn main(flag: bool, x: usize) usize {
    let mut from_binding: usize = if flag { 0 } else { x };
    let mut from_peer = if flag { 1 } else { x };
    from_return(flag) + from_binding + from_peer
}
"#,
    );
    assert!(checked.diagnostics.is_empty(), "{:?}", checked.diagnostics);
}

#[test]
fn infers_wrapped_numeric_literals_from_return_types() {
    let checked = pipeline(
        r#"
fn optionalIndex(flag: bool) ?usize {
    if flag {
        return ?0;
    }
    null
}

fn successByte() i32!u8 {
    !1
}

fn failureByte() u8!i32 {
    2!
}

fn main(flag: bool) i32 {
    _ = optionalIndex(flag);
    _ = successByte();
    _ = failureByte();
    0
}
"#,
    );
    assert!(checked.diagnostics.is_empty(), "{:?}", checked.diagnostics);
}

#[test]
fn allows_never_if_branches_to_converge_to_the_other_branch_type() {
    let checked = pipeline(
        r#"
fn then_return_else_unit(flag: bool) () {
    if flag {
        return;
    } else {
        let mut x = 1;
    }
}

fn chained_returns(byte: u8) () {
    if byte == b'/' {
        return;
    } else if byte == b'*' {
        return;
    } else {
    }
}

fn then_return_else_value(flag: bool) i32 {
    if flag {
        return 1;
    } else {
        2
    }
}

fn then_value_else_return(flag: bool) i32 {
    if flag {
        1
    } else {
        return 2;
    }
}

fn numeric_then_else_return(flag: bool, x: usize) usize {
    if flag { 1 } else { return x; }
}
"#,
    );
    assert!(checked.diagnostics.is_empty(), "{:?}", checked.diagnostics);
}

#[test]
fn checks_numeric_literal_suffixes() {
    let checked = pipeline(
        r#"
fn take_usize(value: usize) usize { value }
fn take_f32(value: f32) f32 { value }

fn main() usize {
    let mut a = 1u8;
    let mut b = 10usize;
    let mut c = 1.0f32;
    let mut d = 1e3f64;
    let mut e: i32 = 1u8;
    let mut f: u8 = 300u8;
    let mut g: f64 = 1.0f32;
    let mut h = 1foo;
    let mut i = 1.0foo;
    let mut j = 1.0usize;
    take_usize(b) + take_f32(c) as usize + a as usize + d as usize
}
"#,
    );
    assert!(
        checked
            .diagnostics
            .iter()
            .any(|diagnostic| diagnostic.summary.contains("binding initializer")),
        "{:?}",
        checked.diagnostics
    );
    assert!(
        checked
            .diagnostics
            .iter()
            .any(|diagnostic| diagnostic.summary.contains("out of range for u8")),
        "{:?}",
        checked.diagnostics
    );
    assert!(
        checked.diagnostics.iter().any(|diagnostic| diagnostic
            .summary
            .contains("invalid integer literal suffix `foo`")),
        "{:?}",
        checked.diagnostics
    );
    assert!(
        checked.diagnostics.iter().any(|diagnostic| diagnostic
            .summary
            .contains("invalid float literal suffix `foo`")),
        "{:?}",
        checked.diagnostics
    );
    assert!(
        checked.diagnostics.iter().any(|diagnostic| diagnostic
            .summary
            .contains("invalid float literal suffix `usize`")),
        "{:?}",
        checked.diagnostics
    );
}

#[test]
fn checks_integer_literal_suffixes_with_radix_and_separators() {
    let checked = pipeline(
        r#"
fn take_u8(value: u8) usize { value as usize }
fn take_usize(value: usize) usize { value }

fn main() usize {
    let mut hex = 0xffu8;
    let mut bin = 0b1010_0000u8;
    let mut oct = 0o755usize;
    let mut dec = 1_000usize;
    let mut too_large = 0x1_00u8;
    take_u8(hex) + take_u8(bin) + take_usize(oct) + take_usize(dec)
}
"#,
    );
    assert!(
        checked
            .diagnostics
            .iter()
            .any(|diagnostic| diagnostic.summary.contains("out of range for u8")),
        "{:?}",
        checked.diagnostics
    );
    assert!(
        checked.diagnostics.iter().all(|diagnostic| !diagnostic
            .summary
            .contains("invalid integer literal suffix")),
        "{:?}",
        checked.diagnostics
    );
}

#[test]
fn checks_float_literal_suffixes_with_exponents_and_separators() {
    let checked = pipeline(
        r#"
fn take_f32(value: f32) usize { value as usize }
fn take_f64(value: f64) usize { value as usize }

fn main() usize {
    let mut a = 1_000.5f32;
    let mut b = 1.0e-3f64;
    let mut c: f64 = 2.5f32;
    let mut too_large = 1e100f32;
    let mut bad_integer_suffix = 1.0usize;
    take_f32(a) + take_f64(b) + take_f64(c)
}
"#,
    );
    assert!(
        checked
            .diagnostics
            .iter()
            .any(|diagnostic| diagnostic.summary.contains("out of range for F32")),
        "{:?}",
        checked.diagnostics
    );
    assert!(
        checked.diagnostics.iter().any(|diagnostic| diagnostic
            .summary
            .contains("invalid float literal suffix `usize`")),
        "{:?}",
        checked.diagnostics
    );
}

#[test]
fn checks_cast_rules_and_function_item_pointers() {
    let checked = pipeline(
        r#"
enum Color: u8 {
    Red,
}

fn id(x: i32) i32 { x }
fn gid[T](x: T) T { x }

fn main(ptr: & u8, other: & i32, flag: bool) i32 {
    let mut a: i64 = 1 as i64;
    let mut b: f64 = 1 as f64;
    let mut c: i32 = Color::Red as i32;
    let mut addr: usize = ptr as usize;
    let mut ptr2: &i32 = addr as &i32;
    let mut ptr3: &i32 = ptr as &i32;
    let mut bad1: bool = 1 as bool;
    let mut bad2: i32 = ptr as i32;
    let mut bad3: i32 = flag as i32;
    let mut fn_value = id;
    let mut fn_ptr = & id;
    let mut generic_ptr = & gid[i32];
    _ = fn_ptr(1);
    _ = generic_ptr(2);
    a as i32 + c + ptr2.* + ptr3.*
}
"#,
    );
    assert_eq!(
        checked
            .diagnostics
            .iter()
            .filter(|diagnostic| diagnostic.summary.contains("invalid cast"))
            .count(),
        3,
        "{:?}",
        checked.diagnostics
    );
    assert_eq!(
        checked
            .diagnostics
            .iter()
            .filter(|diagnostic| diagnostic
                .summary
                .contains("function values are not supported"))
            .count(),
        1,
        "{:?}",
        checked.diagnostics
    );
}
