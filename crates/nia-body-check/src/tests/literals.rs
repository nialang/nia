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
    var ok: u8 = 255;
    var too_large: u8 = 256;
    var negative: u8 = -1;
    var xs: [2]u8 = [0, 256];
    var b: Bytes = { first: 300 };
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
fn checks_float_literals_against_expected_float_types() {
    let checked = pipeline(
        r#"
fn take32(x: f32) f32 { x }

fn main() f64 {
    var a: f32 = 1.5;
    var b: f64 = 1e3;
    var too_large: f32 = 1e100;
    var wrong: i32 = 1.5;
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
    var default_int = 10;
    var default_float = 1.5;
    var explicit_byte: u8 = 10;
    var negative_byte: i8 = -1;
    var explicit_float: f32 = 1.5;
    _ = take_byte(3);
    _ = take32(2.5);
    default_int
}
"#,
    );
    assert!(checked.diagnostics.is_empty(), "{:?}", checked.diagnostics);
    assert!(
        checked
            .facts
            .local_types
            .values()
            .any(|ty| checked.ir.interner.get(*ty)
                == Some(&nia_ty::TyKind::Primitive(nia_ty::PrimitiveTy::I32))),
        "{:?}",
        checked.facts.local_types
    );
    assert!(
        checked
            .facts
            .local_types
            .values()
            .any(|ty| checked.ir.interner.get(*ty)
                == Some(&nia_ty::TyKind::Primitive(nia_ty::PrimitiveTy::F64))),
        "{:?}",
        checked.facts.local_types
    );
    assert!(
        checked
            .facts
            .local_types
            .values()
            .any(|ty| checked.ir.interner.get(*ty)
                == Some(&nia_ty::TyKind::Primitive(nia_ty::PrimitiveTy::U8))),
        "{:?}",
        checked.facts.local_types
    );
    assert!(
        checked
            .facts
            .local_types
            .values()
            .any(|ty| checked.ir.interner.get(*ty)
                == Some(&nia_ty::TyKind::Primitive(nia_ty::PrimitiveTy::F32))),
        "{:?}",
        checked.facts.local_types
    );
}

#[test]
fn infers_binary_numeric_literals_from_the_other_operand() {
    let checked = pipeline(
        r#"
let a = 10;
let ptr = & a;

fn main(x: usize) bool {
    var forward = a as usize == 0;
    var reverse = 0 == a as usize;
    var sum: usize = 1 + x;
    var expected_sum: usize = 1 + 2;
    var shifted: usize = 1 << 2;
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
    var from_binding: usize = if flag { 0 } else { x };
    var from_peer = if flag { 1 } else { x };
    from_return(flag) + from_binding + from_peer
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
    var a = 1u8;
    var b = 10usize;
    var c = 1.0f32;
    var d = 1e3f64;
    var e: i32 = 1u8;
    var f: u8 = 300u8;
    var g: f64 = 1.0f32;
    var h = 1foo;
    var i = 1.0foo;
    var j = 1.0usize;
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
    var hex = 0xffu8;
    var bin = 0b1010_0000u8;
    var oct = 0o755usize;
    var dec = 1_000usize;
    var too_large = 0x1_00u8;
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
    var a = 1_000.5f32;
    var b = 1.0e-3f64;
    var c: f64 = 2.5f32;
    var too_large = 1e100f32;
    var bad_integer_suffix = 1.0usize;
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
    var a: i64 = 1 as i64;
    var b: f64 = 1 as f64;
    var c: i32 = Color::Red as i32;
    var addr: usize = ptr as usize;
    var ptr2: &i32 = addr as &i32;
    var ptr3: &i32 = ptr as &i32;
    var bad1: bool = 1 as bool;
    var bad2: i32 = ptr as i32;
    var bad3: i32 = flag as i32;
    var fn_value = id;
    var fn_ptr = & id;
    var generic_ptr = & gid[i32];
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
