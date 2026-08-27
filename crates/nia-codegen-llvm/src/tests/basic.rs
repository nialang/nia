// SPDX-License-Identifier: GPL-3.0-or-later
use super::common::*;

use std::{
    io,
    sync::{
        Arc,
        atomic::{AtomicUsize, Ordering},
    },
};

struct AlwaysHitObjectCache {
    loads: AtomicUsize,
    publishes: AtomicUsize,
}

impl crate::ObjectWorkProductCache for AlwaysHitObjectCache {
    fn load(
        &self,
        _key: &CodegenUnitKey,
        _fingerprints: crate::CodegenUnitFingerprintSet,
    ) -> io::Result<crate::ObjectWorkProductLookup> {
        self.loads.fetch_add(1, Ordering::Relaxed);
        Ok(crate::ObjectWorkProductLookup::Hit(
            b"cached-object".to_vec(),
        ))
    }

    fn publish(
        &self,
        _key: &CodegenUnitKey,
        _fingerprints: crate::CodegenUnitFingerprintSet,
        _bytes: &[u8],
    ) -> io::Result<()> {
        self.publishes.fetch_add(1, Ordering::Relaxed);
        Ok(())
    }
}

#[test]
fn codegen_ice_boundary_converts_panic_to_diagnostic() {
    let output = catch_llvm_codegen_ice(|| panic!("Nia ICE (LLVM): invalid value kind"));

    assert!(output.modules.is_empty());
    assert_eq!(output.diagnostics.len(), 1);
    let diagnostic = &output.diagnostics[0];
    assert_eq!(diagnostic.category, DiagnosticCategory::Internal);
    assert_eq!(diagnostic.code.as_str(), "I0001");
    assert!(diagnostic.summary.contains("invalid value kind"));
    assert!(
        diagnostic
            .notes
            .iter()
            .any(|note| note.contains("compiler bug"))
    );
}

#[test]
fn native_object_cache_hit_skips_emission_and_publish() {
    let root = temp_dir("native_object_cache_hit_skips_emission_and_publish");
    let main = root.join("main.nia");
    std::fs::write(&main, "fn main() i32 { 1 }").expect("write source");
    let codegen = codegen_program(main.to_string_lossy().into_owned());
    assert!(codegen.diagnostics.is_empty(), "{:?}", codegen.diagnostics);
    let cache = Arc::new(AlwaysHitObjectCache {
        loads: AtomicUsize::new(0),
        publishes: AtomicUsize::new(0),
    });

    let output = crate::emit_native_objects(
        Arc::clone(&codegen.backend_lowering),
        Arc::clone(&codegen.type_store),
        &nia_query::QuerySession::new(),
        LlvmCodegenOptions::default(),
        Some(cache.clone()),
    );

    assert!(output.diagnostics.is_empty(), "{:?}", output.diagnostics);
    assert_eq!(output.link_inputs.len(), 1);
    assert_eq!(
        output.link_inputs.as_slice()[0].object.bytes,
        b"cached-object"
    );
    assert_eq!(cache.loads.load(Ordering::Relaxed), 1);
    assert_eq!(cache.publishes.load(Ordering::Relaxed), 0);
}

#[test]
fn emits_only_referenced_declarations_for_codegen_program() {
    let root = temp_dir("emits_declarations_for_codegen_program");
    let main = root.join("main.nia");
    std::fs::write(
        &main,
        r#"
extern fn puts(s: &u8) i32;
static hello: [u8; 6] = b"hello\0";

extern struct Point {
    x: i32,
    y: i32,
}

extern fn use_point(p: Point) i32;

fn main() i32 {
    _ = puts(&hello[0]);
    let mut x = 40;
    let mut y = 2;
    x + y
}
"#,
    )
    .expect("write test source");

    let codegen = codegen_program(main.to_string_lossy().into_owned());
    assert!(codegen.diagnostics.is_empty(), "{:?}", codegen.diagnostics);

    let output = emit_llvm_ir(&codegen.backend_lowering, &codegen.type_store);
    assert!(output.diagnostics.is_empty(), "{:?}", output.diagnostics);
    let ir = &output.modules[0].ir;
    assert!(ir.contains("declare i32 @puts"));
    assert!(!ir.contains("@use_point"), "{ir}");
    assert!(ir.contains("@nia__s"));
    assert!(!ir.contains("%nia__s"), "{ir}");
    assert!(ir.contains("define i32 @"));
    assert!(ir.contains("alloca i32"));
    assert!(ir.contains("store i32 40"));
    assert!(ir.contains("llvm.sadd.with.overflow.i32"));
    assert!(ir.contains("ret i32"));
}

#[test]
fn emits_primitive_vector_param_and_return_types() {
    let root = temp_dir("emits_primitive_vector_param_and_return_types");
    let main = root.join("main.nia");
    std::fs::write(
        &main,
        r#"
fn id(v: u8x16) u8x16 {
    v
}
"#,
    )
    .expect("write test source");

    let codegen = codegen_program(main.to_string_lossy().into_owned());
    assert!(codegen.diagnostics.is_empty(), "{:?}", codegen.diagnostics);

    let output = emit_llvm_ir(&codegen.backend_lowering, &codegen.type_store);
    assert!(output.diagnostics.is_empty(), "{:?}", output.diagnostics);
    let ir = &output.modules[0].ir;
    assert!(
        ir.contains("define <16 x i8> @"),
        "expected vector return type in IR:\n{ir}"
    );
    assert!(
        ir.contains("<16 x i8> %"),
        "expected vector parameter type in IR:\n{ir}"
    );
}

#[test]
fn emits_splat_builtin() {
    let root = temp_dir("emits_splat_builtin");
    let main = root.join("main.nia");
    std::fs::write(
        &main,
        r#"
fn make(value: u8) u8x16 {
    std::builtin::splat[u8x16](value)
}
"#,
    )
    .expect("write test source");

    let codegen = codegen_program(main.to_string_lossy().into_owned());
    assert!(codegen.diagnostics.is_empty(), "{:?}", codegen.diagnostics);

    let output = emit_llvm_ir(&codegen.backend_lowering, &codegen.type_store);
    assert!(output.diagnostics.is_empty(), "{:?}", output.diagnostics);
    let ir = &output.modules[0].ir;
    assert!(
        ir.contains("define <16 x i8> @"),
        "expected vector return type in IR:\n{ir}"
    );
    assert!(ir.contains("i8 %"), "expected scalar parameter:\n{ir}");
    assert!(
        ir.contains("shufflevector"),
        "expected splat shuffle:\n{ir}"
    );
    assert!(ir.contains("<16 x i32>"), "expected vector mask:\n{ir}");
}

#[test]
fn emits_vector_lane_builtins() {
    let root = temp_dir("emits_vector_lane_builtins");
    let main = root.join("main.nia");
    std::fs::write(
        &main,
        r#"
fn lane(v: u8x16, i: usize) u8 {
    std::builtin::extract(v, i)
}

fn changed(v: u8x16, i: usize, x: u8) u8x16 {
    std::builtin::insert(v, i, x)
}
"#,
    )
    .expect("write test source");

    let codegen = codegen_program(main.to_string_lossy().into_owned());
    assert!(codegen.diagnostics.is_empty(), "{:?}", codegen.diagnostics);

    let output = emit_llvm_ir(&codegen.backend_lowering, &codegen.type_store);
    assert!(output.diagnostics.is_empty(), "{:?}", output.diagnostics);
    let ir = &output.modules[0].ir;
    assert!(
        ir.contains("extractelement <16 x i8>"),
        "expected vector lane extract:\n{ir}"
    );
    assert!(
        ir.contains("insertelement <16 x i8>"),
        "expected vector lane insert:\n{ir}"
    );
    assert!(
        ir.matches("icmp ult i64").count() >= 2
            && ir.contains("extract.trap")
            && ir.contains("insert.trap")
            && ir.contains("llvm.trap"),
        "expected checked SIMD lane indexes:\n{ir}"
    );
}

#[test]
fn vector_union_storage_uses_shared_native_alignment() {
    let root = temp_dir("vector_union_storage_uses_shared_native_alignment");
    let main = root.join("main.nia");
    std::fs::write(
        &main,
        r#"
union WideSlot {
    vector: u8x32,
    bytes: [u8; 32],
}

fn wrap(vector: u8x32) WideSlot {
    WideSlot { vector }
}

fn read(slot: WideSlot) u8x32 {
    slot.vector
}
"#,
    )
    .expect("write test source");

    let codegen = codegen_program(main.to_string_lossy().into_owned());
    assert!(codegen.diagnostics.is_empty(), "{:?}", codegen.diagnostics);

    let output = emit_llvm_ir(&codegen.backend_lowering, &codegen.type_store);
    assert!(output.diagnostics.is_empty(), "{:?}", output.diagnostics);
    let ir = &output.modules[0].ir;
    assert!(
        ir.contains("type { <32 x i8> }")
            && ir.contains("store <32 x i8>")
            && ir.contains("align 32"),
        "expected 32-byte vector union storage alignment:\n{ir}"
    );
}

#[test]
fn emits_vector_builtin_operators() {
    let root = temp_dir("emits_vector_builtin_operators");
    let main = root.join("main.nia");
    std::fs::write(
        &main,
        r#"
fn add_u8(lhs: u8x16, rhs: u8x16) u8x16 {
    lhs + rhs
}

fn sub_i8(lhs: i8x16, rhs: i8x16) i8x16 {
    lhs - rhs
}

fn mul_u8(lhs: u8x16, rhs: u8x16) u8x16 {
    lhs * rhs
}

fn neg_i8(value: i8x16) i8x16 {
    -value
}

fn and_mask(lhs: boolx16, rhs: boolx16) boolx16 {
    lhs & rhs
}

fn cmp_f32(lhs: f32x4, rhs: f32x4) boolx4 {
    lhs < rhs
}
"#,
    )
    .expect("write test source");

    let codegen = codegen_program(main.to_string_lossy().into_owned());
    assert!(codegen.diagnostics.is_empty(), "{:?}", codegen.diagnostics);

    let output = emit_llvm_ir(&codegen.backend_lowering, &codegen.type_store);
    assert!(output.diagnostics.is_empty(), "{:?}", output.diagnostics);
    let ir = &output.modules[0].ir;
    assert!(
        ir.contains("llvm.uadd.with.overflow.v16i8"),
        "expected checked vector add:\n{ir}"
    );
    assert!(
        ir.contains("llvm.ssub.with.overflow.v16i8"),
        "expected checked vector sub and negation:\n{ir}"
    );
    assert!(
        ir.contains("llvm.umul.with.overflow.v16i8"),
        "expected checked vector multiply:\n{ir}"
    );
    assert!(
        ir.contains("bitcast <16 x i1>")
            && ir.contains("arith.overflow.any")
            && ir.contains("arith.trap"),
        "expected any-lane overflow trap:\n{ir}"
    );
    assert!(ir.contains("and <16 x i1>"), "expected vector and:\n{ir}");
    assert!(
        ir.contains("fcmp olt <4 x float>"),
        "expected vector float compare:\n{ir}"
    );
    assert!(
        ir.contains("define <4 x i1> @"),
        "expected vector comparison mask return:\n{ir}"
    );
}

#[test]
fn emits_vector_bitmask_builtin() {
    let root = temp_dir("emits_vector_bitmask_builtin");
    let main = root.join("main.nia");
    std::fs::write(
        &main,
        r#"
fn matching(v: u8x16, tag: u8) usize {
    std::builtin::bitmask(v == std::builtin::splat[u8x16](tag))
}
"#,
    )
    .expect("write test source");

    let codegen = codegen_program(main.to_string_lossy().into_owned());
    assert!(codegen.diagnostics.is_empty(), "{:?}", codegen.diagnostics);

    let output = emit_llvm_ir(&codegen.backend_lowering, &codegen.type_store);
    assert!(output.diagnostics.is_empty(), "{:?}", output.diagnostics);
    let ir = &output.modules[0].ir;
    assert!(
        ir.contains("icmp eq <16 x i8>"),
        "expected vector integer compare:\n{ir}"
    );
    assert!(
        ir.contains("bitcast <16 x i1>"),
        "expected vector mask bitcast:\n{ir}"
    );
    assert!(
        ir.contains("zext i16"),
        "expected mask widening to usize:\n{ir}"
    );
}

#[test]
fn emits_bit_intrinsic_builtins() {
    let root = temp_dir("emits_bit_intrinsic_builtins");
    let main = root.join("main.nia");
    std::fs::write(
        &main,
        r#"
fn scan(mask: usize) usize {
    std::builtin::ctz[usize](mask) + std::builtin::clz[usize](mask) + std::builtin::popcount[usize](mask)
}
"#,
    )
    .expect("write test source");

    let codegen = codegen_program(main.to_string_lossy().into_owned());
    assert!(codegen.diagnostics.is_empty(), "{:?}", codegen.diagnostics);

    let output = emit_llvm_ir(&codegen.backend_lowering, &codegen.type_store);
    assert!(output.diagnostics.is_empty(), "{:?}", output.diagnostics);
    let ir = &output.modules[0].ir;
    assert!(
        ir.contains("@llvm.cttz.i64"),
        "expected ctz intrinsic:\n{ir}"
    );
    assert!(
        ir.contains("@llvm.ctlz.i64"),
        "expected clz intrinsic:\n{ir}"
    );
    assert!(
        ir.contains("@llvm.ctpop.i64"),
        "expected popcount intrinsic:\n{ir}"
    );
    assert!(
        ir.contains("call i64 @llvm.cttz.i64(i64 %") && ir.contains("i1 false"),
        "expected zero-defined ctz call:\n{ir}"
    );
}

#[test]
fn emits_wide_integer_shift_with_narrow_count() {
    let root = temp_dir("emits_wide_integer_shift_with_narrow_count");
    let main = root.join("main.nia");
    std::fs::write(
        &main,
        r#"
fn high(value: u128, count: u32) u128 {
    value >> count
}

fn low(value: u128, count: u32) u128 {
    value << count
}
"#,
    )
    .expect("write test source");

    let codegen = codegen_program(main.to_string_lossy().into_owned());
    assert!(codegen.diagnostics.is_empty(), "{:?}", codegen.diagnostics);

    let output = emit_llvm_ir(&codegen.backend_lowering, &codegen.type_store);
    assert!(output.diagnostics.is_empty(), "{:?}", output.diagnostics);
    let ir = &output.modules[0].ir;
    assert!(
        ir.contains("zext i32") && ir.contains("to i128"),
        "expected shift count extension:\n{ir}"
    );
    assert!(
        ir.contains("lshr i128"),
        "expected logical right shift:\n{ir}"
    );
    assert!(
        ir.contains("shl i256"),
        "expected checked wide left shift:\n{ir}"
    );
}

#[test]
fn scalar_integer_shifts_validate_counts_and_left_overflow() {
    let root = temp_dir("scalar_integer_shifts_validate_counts_and_left_overflow");
    let main = root.join("main.nia");
    std::fs::write(
        &main,
        r#"
fn unsignedLeft(lhs: u8, count: u128) u8 { lhs << count }
fn signedLeft(lhs: i8, count: i16) i8 { lhs << count }
fn signedRight(lhs: i8, count: i16) i8 { lhs >> count }
fn unsignedRight(lhs: u8, count: u16) u8 { lhs >> count }

fn compoundLeft(lhs: u8, count: u16) u8 {
    let mut value = lhs;
    value <<= count;
    value
}

fn compoundRight(lhs: i8, count: i16) i8 {
    let mut value = lhs;
    value >>= count;
    value
}
"#,
    )
    .expect("write test source");

    let codegen = codegen_program(main.to_string_lossy().into_owned());
    assert!(codegen.diagnostics.is_empty(), "{:?}", codegen.diagnostics);

    let output = emit_llvm_ir(&codegen.backend_lowering, &codegen.type_store);
    assert!(output.diagnostics.is_empty(), "{:?}", output.diagnostics);
    let ir = &output.modules[0].ir;
    assert!(
        ir.contains("icmp uge i128") && ir.contains(", 8"),
        "wide counts must be checked before truncation:\n{ir}"
    );
    assert!(
        ir.contains("icmp slt i16") && ir.contains("shift.count.negative"),
        "signed negative counts must trap:\n{ir}"
    );
    assert!(
        ir.contains("shift.count.trap") && ir.contains("call void @llvm.trap()"),
        "invalid counts must reach a trap block:\n{ir}"
    );
    assert!(
        ir.contains("shl i16")
            && ir.contains("shift.result.overflow")
            && ir.contains("shift.overflow.trap"),
        "left shifts must check an exact widened result:\n{ir}"
    );
    assert!(
        ir.contains("ashr i8"),
        "expected arithmetic right shift:\n{ir}"
    );
    assert!(
        ir.contains("lshr i8"),
        "expected logical right shift:\n{ir}"
    );
    assert!(
        ir.matches("shift.count.out_of_range").count() >= 6,
        "ordinary and compound shifts must share count validation:\n{ir}"
    );
}

#[test]
fn vector_integer_shifts_trap_on_any_invalid_lane() {
    let root = temp_dir("vector_integer_shifts_trap_on_any_invalid_lane");
    let main = root.join("main.nia");
    std::fs::write(
        &main,
        r#"
fn unsignedLeft(lhs: u8x16, rhs: u8x16) u8x16 { lhs << rhs }
fn signedLeft(lhs: i8x16, rhs: i8x16) i8x16 { lhs << rhs }
fn unsignedRight(lhs: u8x16, rhs: u8x16) u8x16 { lhs >> rhs }
fn signedRight(lhs: i8x16, rhs: i8x16) i8x16 { lhs >> rhs }

fn compoundLeft(lhs: u8x16, rhs: u8x16) u8x16 {
    let mut value = lhs;
    value <<= rhs;
    value
}
"#,
    )
    .expect("write test source");

    let codegen = codegen_program(main.to_string_lossy().into_owned());
    assert!(codegen.diagnostics.is_empty(), "{:?}", codegen.diagnostics);

    let output = emit_llvm_ir(&codegen.backend_lowering, &codegen.type_store);
    assert!(output.diagnostics.is_empty(), "{:?}", output.diagnostics);
    let ir = &output.modules[0].ir;
    assert!(
        ir.contains("icmp uge <16 x i8>")
            && ir.contains("shift.count.invalid.any")
            && ir.contains("shift.count.trap"),
        "expected any-lane vector count validation:\n{ir}"
    );
    assert!(
        ir.contains("icmp slt <16 x i8>") && ir.contains("shift.count.negative"),
        "expected signed vector count validation:\n{ir}"
    );
    assert!(
        ir.contains("shl <16 x i16>")
            && ir.contains("shift.result.overflow.any")
            && ir.contains("shift.overflow.trap"),
        "expected exact widened vector left shift:\n{ir}"
    );
    assert!(
        ir.contains("ashr <16 x i8>"),
        "expected arithmetic vector right shift:\n{ir}"
    );
    assert!(
        ir.contains("lshr <16 x i8>"),
        "expected logical vector right shift:\n{ir}"
    );
    assert!(
        ir.matches("shift.result.overflow.any").count() >= 3,
        "compound left shift must share vector checks:\n{ir}"
    );
}

#[test]
fn integer_division_and_remainder_trap_before_llvm_undefined_behavior() {
    let root = temp_dir("integer_division_and_remainder_trap_before_llvm_undefined_behavior");
    let main = root.join("main.nia");
    std::fs::write(
        &main,
        r#"
fn signedDiv(lhs: i32, rhs: i32) i32 {
    lhs / rhs
}

fn signedRem(lhs: i32, rhs: i32) i32 {
    lhs % rhs
}

fn unsignedDiv(lhs: u32, rhs: u32) u32 {
    lhs / rhs
}

fn unsignedRem(lhs: u32, rhs: u32) u32 {
    lhs % rhs
}

fn compoundDiv(lhs: i32, rhs: i32) i32 {
    let mut value = lhs;
    value /= rhs;
    value
}

fn compoundRem(lhs: u32, rhs: u32) u32 {
    let mut value = lhs;
    value %= rhs;
    value
}
"#,
    )
    .expect("write test source");

    let codegen = codegen_program(main.to_string_lossy().into_owned());
    assert!(codegen.diagnostics.is_empty(), "{:?}", codegen.diagnostics);

    let output = emit_llvm_ir(&codegen.backend_lowering, &codegen.type_store);
    assert!(output.diagnostics.is_empty(), "{:?}", output.diagnostics);
    let ir = &output.modules[0].ir;
    assert!(ir.contains("divrem.zero"), "expected zero checks:\n{ir}");
    assert!(
        ir.contains("divrem.min"),
        "expected signed-min checks:\n{ir}"
    );
    assert!(
        ir.contains("divrem.negative_one"),
        "expected signed negative-one checks:\n{ir}"
    );
    assert!(
        ir.contains("divrem.trap") && ir.contains("call void @llvm.trap()"),
        "expected checked div/rem trap blocks:\n{ir}"
    );
    assert!(ir.contains("sdiv i32"), "expected signed division:\n{ir}");
    assert!(ir.contains("srem i32"), "expected signed remainder:\n{ir}");
    assert!(ir.contains("udiv i32"), "expected unsigned division:\n{ir}");
    assert!(
        ir.contains("urem i32"),
        "expected unsigned remainder:\n{ir}"
    );
}

#[test]
fn vector_integer_division_and_remainder_trap_on_any_invalid_lane() {
    let root = temp_dir("vector_integer_division_and_remainder_trap_on_any_invalid_lane");
    let main = root.join("main.nia");
    std::fs::write(
        &main,
        r#"
fn signedDiv(lhs: i8x16, rhs: i8x16) i8x16 { lhs / rhs }
fn signedRem(lhs: i8x16, rhs: i8x16) i8x16 { lhs % rhs }
fn unsignedDiv(lhs: u8x16, rhs: u8x16) u8x16 { lhs / rhs }
fn unsignedRem(lhs: u8x16, rhs: u8x16) u8x16 { lhs % rhs }

fn compoundDiv(lhs: i8x16, rhs: i8x16) i8x16 {
    let mut value = lhs;
    value /= rhs;
    value
}
"#,
    )
    .expect("write test source");

    let codegen = codegen_program(main.to_string_lossy().into_owned());
    assert!(codegen.diagnostics.is_empty(), "{:?}", codegen.diagnostics);

    let output = emit_llvm_ir(&codegen.backend_lowering, &codegen.type_store);
    assert!(output.diagnostics.is_empty(), "{:?}", output.diagnostics);
    let ir = &output.modules[0].ir;
    assert!(
        ir.contains("icmp eq <16 x i8>")
            && ir.contains("divrem.min")
            && ir.contains("divrem.negative_one"),
        "expected lane-wise vector div/rem checks:\n{ir}"
    );
    assert!(
        ir.contains("divrem.traps.any")
            && ir.contains("bitcast <16 x i1>")
            && ir.contains("divrem.trap"),
        "expected any-lane vector div/rem trap:\n{ir}"
    );
    assert!(ir.contains("sdiv <16 x i8>"), "expected signed div:\n{ir}");
    assert!(ir.contains("srem <16 x i8>"), "expected signed rem:\n{ir}");
    assert!(
        ir.contains("udiv <16 x i8>"),
        "expected unsigned div:\n{ir}"
    );
    assert!(
        ir.contains("urem <16 x i8>"),
        "expected unsigned rem:\n{ir}"
    );
    assert!(
        ir.matches("divrem.traps.any").count() >= 5,
        "compound div must share vector checks:\n{ir}"
    );
}

#[test]
fn scalar_integer_arithmetic_traps_on_overflow() {
    let root = temp_dir("scalar_integer_arithmetic_traps_on_overflow");
    let main = root.join("main.nia");
    std::fs::write(
        &main,
        r#"
fn signedAdd(lhs: i8, rhs: i8) i8 { lhs + rhs }
fn unsignedAdd(lhs: u8, rhs: u8) u8 { lhs + rhs }
fn signedSub(lhs: i8, rhs: i8) i8 { lhs - rhs }
fn unsignedSub(lhs: u8, rhs: u8) u8 { lhs - rhs }
fn signedMul(lhs: i8, rhs: i8) i8 { lhs * rhs }
fn unsignedMul(lhs: u8, rhs: u8) u8 { lhs * rhs }
fn signedNeg(value: i8) i8 { -value }

fn compoundAdd(lhs: u8, rhs: u8) u8 {
    let mut value = lhs;
    value += rhs;
    value
}

fn main() i32 {
    signedAdd(1i8, 2i8) as i32
        + unsignedAdd(1u8, 2u8) as i32
        + signedSub(3i8, 1i8) as i32
        + unsignedSub(3u8, 1u8) as i32
        + signedMul(2i8, 3i8) as i32
        + unsignedMul(2u8, 3u8) as i32
        + signedNeg(1i8) as i32
        + compoundAdd(1u8, 2u8) as i32
}
"#,
    )
    .expect("write test source");

    let codegen = codegen_program(main.to_string_lossy().into_owned());
    assert!(codegen.diagnostics.is_empty(), "{:?}", codegen.diagnostics);

    let output = emit_llvm_ir(&codegen.backend_lowering, &codegen.type_store);
    assert!(output.diagnostics.is_empty(), "{:?}", output.diagnostics);
    let ir = output
        .modules
        .iter()
        .map(|module| module.ir.as_str())
        .collect::<String>();
    for intrinsic in [
        "llvm.sadd.with.overflow.i8",
        "llvm.uadd.with.overflow.i8",
        "llvm.ssub.with.overflow.i8",
        "llvm.usub.with.overflow.i8",
        "llvm.smul.with.overflow.i8",
        "llvm.umul.with.overflow.i8",
    ] {
        assert!(ir.contains(intrinsic), "missing {intrinsic}:\n{ir}");
    }
    assert!(
        ir.contains("arith.trap") && ir.contains("call void @llvm.trap()"),
        "expected checked arithmetic trap blocks:\n{ir}"
    );
    assert!(
        !ir.contains("sub i8 0"),
        "integer negation must use checked subtraction:\n{ir}"
    );
}

#[test]
fn emits_compiler_builtins_object_only_when_reachable_ir_needs_it() {
    let root = temp_dir("emits_compiler_builtins_object_only_when_reachable_ir_needs_it");
    let plain = root.join("plain.nia");
    std::fs::write(
        &plain,
        r#"
fn main() i32 {
    40 + 2
}
"#,
    )
    .expect("write plain source");

    let codegen = codegen_program(plain.to_string_lossy().into_owned());
    assert!(codegen.diagnostics.is_empty(), "{:?}", codegen.diagnostics);
    let output = emit_native_objects(
        &codegen.backend_lowering,
        &codegen.type_store,
        LlvmCodegenOptions::default(),
    );
    assert!(output.diagnostics.is_empty(), "{:?}", output.diagnostics);
    assert!(
        !output
            .link_inputs
            .as_slice()
            .iter()
            .any(|input| input.object.name == "nia.compiler_builtins"),
        "compiler builtins should not be emitted for programs that do not need lowered libcalls"
    );
    assert_eq!(
        output.link_inputs.len(),
        1,
        "empty declaration-only backend modules should not produce native object files"
    );

    let wide = root.join("wide.nia");
    std::fs::write(
        &wide,
        r#"
fn divrem(value: u128, by: u128) u128 {
    (value / by) + (value % by)
}
"#,
    )
    .expect("write wide source");

    let codegen = codegen_program(wide.to_string_lossy().into_owned());
    assert!(codegen.diagnostics.is_empty(), "{:?}", codegen.diagnostics);
    let output = emit_native_objects(
        &codegen.backend_lowering,
        &codegen.type_store,
        LlvmCodegenOptions::default(),
    );
    assert!(output.diagnostics.is_empty(), "{:?}", output.diagnostics);
    assert_eq!(
        output
            .link_inputs
            .as_slice()
            .iter()
            .filter(|input| input.object.name == "nia.compiler_builtins")
            .count(),
        1,
        "u128 division should request exactly one compiler builtins object"
    );
    assert!(
        output
            .link_inputs
            .as_slice()
            .iter()
            .any(|input| input.object.name == "nia.compiler_builtins"),
        "expected compiler builtins object in {:?}",
        output
            .link_inputs
            .as_slice()
            .iter()
            .map(|input| &input.object.name)
            .collect::<Vec<_>>()
    );
    assert_eq!(
        output
            .link_inputs
            .as_slice()
            .iter()
            .find(|input| input.object.name == "nia.compiler_builtins")
            .map(|input| input.object.unit),
        Some(CodegenUnitId::CompilerBuiltins)
    );
}

#[test]
fn rejects_external_symbols_owned_by_required_compiler_builtins() {
    let root = temp_dir("rejects_external_symbols_owned_by_required_compiler_builtins");
    let plain = root.join("plain.nia");
    std::fs::write(
        &plain,
        r#"
extern fn __udivti3(lhs: u128, rhs: u128) u128 {
    _ = rhs;
    lhs
}
extern static mut __umodti3: u128;

fn main() i32 { 0 }
"#,
    )
    .expect("write unreserved builtin-name source");

    let codegen = codegen_program(plain.to_string_lossy().into_owned());
    assert!(codegen.diagnostics.is_empty(), "{:?}", codegen.diagnostics);
    let output = emit_native_objects(
        &codegen.backend_lowering,
        &codegen.type_store,
        LlvmCodegenOptions::default(),
    );
    assert!(
        output.diagnostics.is_empty(),
        "unused builtin names must remain available to externs: {:?}",
        output.diagnostics
    );

    let wide = root.join("wide.nia");
    std::fs::write(
        &wide,
        r#"
extern fn __udivti3(lhs: u128, rhs: u128) u128 {
    _ = rhs;
    lhs
}
extern static mut __umodti3: u128;

fn divrem(value: u128, by: u128) u128 {
    (value / by) + (value % by)
}
"#,
    )
    .expect("write reserved builtin-name source");

    let codegen = codegen_program(wide.to_string_lossy().into_owned());
    assert!(codegen.diagnostics.is_empty(), "{:?}", codegen.diagnostics);
    let output = emit_native_objects(
        &codegen.backend_lowering,
        &codegen.type_store,
        LlvmCodegenOptions::default(),
    );
    assert!(output.link_inputs.is_empty());
    for expected in [
        "extern function reuses `__udivti3` already owned by compiler builtin",
        "extern global reuses `__umodti3` already owned by compiler builtin",
    ] {
        assert!(
            has_internal_diagnostic(&output.diagnostics, codes::INVALID_BACKEND_IR, expected),
            "missing `{expected}` in {:?}",
            output.diagnostics
        );
    }
}

#[test]
fn emits_compiler_builtins_for_wide_float_casts() {
    let root = temp_dir("emits_compiler_builtins_for_wide_float_casts");
    let main = root.join("main.nia");
    std::fs::write(
        &main,
        r#"
fn from32(value: f32) u128 {
    value as u128
}

fn from64(value: f64) u128 {
    value as u128
}

fn signedFrom32(value: f32) i128 {
    value as i128
}

fn signedFrom64(value: f64) i128 {
    value as i128
}

fn to32(value: u128) f32 {
    value as f32
}

fn to64(value: u128) f64 {
    value as f64
}

fn signedTo32(value: i128) f32 {
    value as f32
}

fn signedTo64(value: i128) f64 {
    value as f64
}
"#,
    )
    .expect("write float-to-u128 source");

    let codegen = codegen_program(main.to_string_lossy().into_owned());
    assert!(codegen.diagnostics.is_empty(), "{:?}", codegen.diagnostics);
    let output = emit_native_objects(
        &codegen.backend_lowering,
        &codegen.type_store,
        LlvmCodegenOptions::default(),
    );
    assert!(output.diagnostics.is_empty(), "{:?}", output.diagnostics);
    assert_eq!(
        output
            .link_inputs
            .as_slice()
            .iter()
            .filter(|input| input.object.unit == CodegenUnitId::CompilerBuiltins)
            .count(),
        1,
        "signed and unsigned f32/f64 casts must share one compiler-builtins object"
    );
}

#[test]
fn emits_unaligned_vector_load_builtin() {
    let root = temp_dir("emits_unaligned_vector_load_builtin");
    let main = root.join("main.nia");
    std::fs::write(
        &main,
        r#"
fn load(ptr: &u8) u8x8 {
    std::builtin::load_unaligned[u8x8](ptr)
}
"#,
    )
    .expect("write test source");

    let codegen = codegen_program(main.to_string_lossy().into_owned());
    assert!(codegen.diagnostics.is_empty(), "{:?}", codegen.diagnostics);

    let output = emit_llvm_ir(&codegen.backend_lowering, &codegen.type_store);
    assert!(output.diagnostics.is_empty(), "{:?}", output.diagnostics);
    let ir = &output.modules[0].ir;
    assert!(
        ir.contains("load <8 x i8>, ptr %") && ir.contains("align 1"),
        "expected explicit align-1 vector load:\n{ir}"
    );
}
