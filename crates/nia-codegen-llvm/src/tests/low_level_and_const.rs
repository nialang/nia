// SPDX-License-Identifier: GPL-3.0-or-later
use super::common::*;

#[test]
fn rejects_bare_global_as_pointer_initializer() {
    let root = temp_dir("rejects_bare_global_as_pointer_initializer");
    let main = root.join("main.nia");
    std::fs::write(
        &main,
        r#"
static mut target: i32 = 1;
static p: &i32 = target;

fn main() i32 {
    target
}
"#,
    )
    .expect("write test source");

    let codegen = codegen_program(main.to_string_lossy().into_owned());
    assert!(
        codegen
            .diagnostics
            .iter()
            .any(|diagnostic| diagnostic.diagnostic.summary.contains("global initializer")),
        "{:?}",
        codegen.diagnostics
    );
}

#[test]
fn emits_scalar_promoted_allocation_for_union_relocation() {
    let root = temp_dir("emits_scalar_promoted_allocation_for_union_relocation");
    let main = root.join("main.nia");
    std::fs::write(
        &main,
        r#"
union Slot {
    pointer: &usize,
    integer: usize,
}

const slot: Slot = { pointer: &34usize };

fn main() bool {
    let left: Slot = slot;
    let right: Slot = slot;
    left.pointer == right.pointer
}
"#,
    )
    .expect("write test source");

    let codegen = codegen_program(main.to_string_lossy().into_owned());
    assert!(codegen.diagnostics.is_empty(), "{:?}", codegen.diagnostics);
    let output = emit_llvm_ir(&codegen.backend_lowering, &codegen.type_store);
    assert!(output.diagnostics.is_empty(), "{:?}", output.diagnostics);
    let ir = source_module_ir(&output, "main.nia");
    assert!(ir.contains("linkonce_odr constant i64 34"), "{ir}");
    assert!(ir.contains("store ptr @nia__promoted__"), "{ir}");
}

#[test]
fn keeps_distinct_scalar_promotion_origins_separate() {
    let root = temp_dir("keeps_distinct_scalar_promotion_origins_separate");
    let main = root.join("main.nia");
    std::fs::write(
        &main,
        r#"
union Slot {
    pointer: &usize,
    integer: usize,
}

const leftSlot: Slot = { pointer: &34usize };
const rightSlot: Slot = { pointer: &34usize };

fn main() bool {
    let left: Slot = leftSlot;
    let same: Slot = leftSlot;
    let right: Slot = rightSlot;
    left.pointer == same.pointer and left.pointer != right.pointer
}
"#,
    )
    .expect("write test source");

    let codegen = codegen_program(main.to_string_lossy().into_owned());
    assert!(codegen.diagnostics.is_empty(), "{:?}", codegen.diagnostics);
    let output = emit_llvm_ir(&codegen.backend_lowering, &codegen.type_store);
    assert!(output.diagnostics.is_empty(), "{:?}", output.diagnostics);
    let ir = source_module_ir(&output, "main.nia");
    assert_eq!(
        ir.matches("linkonce_odr constant i64 34").count(),
        2,
        "{ir}"
    );
}

#[test]
fn emits_imported_scalar_promotion_after_origin_module_is_ready() {
    let root = temp_dir("emits_imported_scalar_promotion_after_origin_module_is_ready");
    let main = root.join("main.nia");
    std::fs::write(
        root.join("defs.nia"),
        r#"
pub union Slot {
    pointer: &usize,
    integer: usize,
}

pub const slot: Slot = { pointer: &55usize };
"#,
    )
    .expect("write defs source");
    std::fs::write(
        &main,
        r#"
module defs;
using entry::defs;

fn main() usize {
    let value: defs::Slot = defs::slot;
    value.pointer.*
}
"#,
    )
    .expect("write main source");

    let codegen = codegen_program(main.to_string_lossy().into_owned());
    assert!(codegen.diagnostics.is_empty(), "{:?}", codegen.diagnostics);
    let output = emit_llvm_ir(&codegen.backend_lowering, &codegen.type_store);
    assert!(output.diagnostics.is_empty(), "{:?}", output.diagnostics);
    let ir = source_module_ir(&output, "main.nia");
    assert!(ir.contains("linkonce_odr constant i64 55"), "{ir}");
    assert!(ir.contains("store ptr @nia__promoted__"), "{ir}");
}

#[test]
fn emits_array_and_struct_promoted_allocation_constants() {
    let root = temp_dir("emits_array_and_struct_promoted_allocation_constants");
    let main = root.join("main.nia");
    std::fs::write(
        &main,
        r#"
struct Pair {
    left: u16,
    right: u16,
}

union ArraySlot {
    pointer: &[3]u8,
    integer: usize,
}

union TextSlot {
    pointer: &[2]char,
    integer: usize,
}

union PairSlot {
    pointer: &Pair,
    integer: usize,
}

const pairSlot: PairSlot = { pointer: &Pair{left: 5u16, right: 8u16} };
const arraySlot: ArraySlot = { pointer: &[3]u8[1, 2, 3] };
const bytesSlot: ArraySlot = { pointer: &b"abc" };
const textSlot: TextSlot = { pointer: &"hi" };

fn main() usize {
    let pair: PairSlot = pairSlot;
    let array: ArraySlot = arraySlot;
    let bytes: ArraySlot = bytesSlot;
    let text: TextSlot = textSlot;
    _ = text.pointer.*[0];
    pair.pointer.*.right as usize
        + array.pointer.*[2] as usize
        + bytes.pointer.*[1] as usize
}
"#,
    )
    .expect("write test source");

    let codegen = codegen_program(main.to_string_lossy().into_owned());
    assert!(codegen.diagnostics.is_empty(), "{:?}", codegen.diagnostics);
    let output = emit_llvm_ir(&codegen.backend_lowering, &codegen.type_store);
    assert!(output.diagnostics.is_empty(), "{:?}", output.diagnostics);
    let ir = source_module_ir(&output, "main.nia");
    assert!(ir.contains("linkonce_odr constant %nia__s"), "{ir}");
    assert!(ir.contains("linkonce_odr constant [3 x i8]"), "{ir}");
    assert!(ir.contains("linkonce_odr constant [2 x i32]"), "{ir}");
}

#[test]
fn rejects_zero_sized_promoted_allocation_until_identity_storage_exists() {
    let root = temp_dir("rejects_zero_sized_promoted_allocation_until_identity_storage_exists");
    let main = root.join("main.nia");
    std::fs::write(
        &main,
        r#"
struct Empty {}

union Slot {
    pointer: &Empty,
    integer: usize,
}

const slot: Slot = { pointer: &Empty{} };

fn main() bool {
    let value: Slot = slot;
    value.pointer == value.pointer
}
"#,
    )
    .expect("write test source");

    let codegen = codegen_program(main.to_string_lossy().into_owned());
    assert!(codegen.diagnostics.is_empty(), "{:?}", codegen.diagnostics);
    let output = emit_llvm_ir(&codegen.backend_lowering, &codegen.type_store);
    assert!(output.diagnostics.iter().any(|diagnostic| {
        diagnostic
            .summary
            .contains("zero-sized promoted allocation identity is not yet supported")
    }));
}

#[test]
fn emits_const_generic_function_and_nominal_array_instances() {
    let root = temp_dir("emits_const_generic_function_and_nominal_array_instances");
    let main = root.join("main.nia");
    std::fs::write(
        &main,
        r#"
struct Buffer[T, N: usize] {
    data: [N]T,
}

fn take[T, N: usize](items: [N]T) usize {
    items.len()
}

fn make4() Buffer[u8, 4] {
    { data: [1u8, 2u8, 3u8, 4u8] }
}

fn make8() Buffer[u8, 8] {
    { data: [1u8, 2u8, 3u8, 4u8, 5u8, 6u8, 7u8, 8u8] }
}

fn main() usize {
    let a = make4();
    let b = make8();
    take(a.data) + take(b.data)
}
"#,
    )
    .expect("write test source");

    let codegen = codegen_program(main.to_string_lossy().into_owned());
    assert!(codegen.diagnostics.is_empty(), "{:?}", codegen.diagnostics);

    let output = emit_llvm_ir(&codegen.backend_lowering, &codegen.type_store);
    assert!(output.diagnostics.is_empty(), "{:?}", output.diagnostics);
    let ir = &output.modules[0].ir;
    assert!(ir.contains("[4 x i8]"), "{ir}");
    assert!(ir.contains("[8 x i8]"), "{ir}");
}

#[test]
fn emits_inline_asm_inputs_outputs_and_clobbers() {
    let root = temp_dir("emits_inline_asm_inputs_outputs_and_clobbers");
    let main = root.join("main.nia");
    std::fs::write(
        &main,
        r#"
fn main() i32 {
    let mut value: i64 = 0;
    std::builtin::asm({
        code:
            b\\mov rax, rax
            \\add rax, 0
        ,
        outputs: { rax: value },
        inputs: { rax: 7 },
        clobbers: [b"memory"],
        options: [b"volatile"],
    });
    value as i32
}

"#,
    )
    .expect("write test source");

    let codegen = codegen_program(main.to_string_lossy().into_owned());
    assert!(codegen.diagnostics.is_empty(), "{:?}", codegen.diagnostics);

    let output = emit_llvm_ir(&codegen.backend_lowering, &codegen.type_store);
    assert!(output.diagnostics.is_empty(), "{:?}", output.diagnostics);
    let ir = &output.modules[0].ir;
    assert!(ir.contains("asm sideeffect"));
    assert!(ir.contains("mov rax, rax\\0Aadd rax, 0"));
    assert!(ir.contains("={rax},{rax},~{memory}"), "{ir}");
}

#[test]
fn emits_volatile_pointer_load_and_store() {
    let root = temp_dir("emits_volatile_pointer_load_and_store");
    let main = root.join("main.nia");
    std::fs::write(
        &main,
        r#"
extern fn read_reg(reg: ^u32) u32 {
    reg.*
}

extern fn write_reg(reg: ^mut u32, value: u32) void {
    reg.* = value;
}
"#,
    )
    .expect("write test source");

    let codegen = codegen_program(main.to_string_lossy().into_owned());
    assert!(codegen.diagnostics.is_empty(), "{:?}", codegen.diagnostics);

    let output = emit_llvm_ir(&codegen.backend_lowering, &codegen.type_store);
    assert!(output.diagnostics.is_empty(), "{:?}", output.diagnostics);
    let ir = &output.modules[0].ir;
    assert!(ir.contains("load volatile i32"), "{ir}");
    assert!(ir.contains("store volatile i32"), "{ir}");
}

#[test]
fn emits_trap_builtin_as_llvm_trap() {
    let root = temp_dir("emits_trap_builtin_as_llvm_trap");
    let main = root.join("main.nia");
    std::fs::write(
        &main,
        r#"
fn main() i32 {
    std::builtin::trap()
}

fn statement() void {
    std::builtin::trap();
}
"#,
    )
    .expect("write test source");

    let codegen = codegen_program(main.to_string_lossy().into_owned());
    assert!(codegen.diagnostics.is_empty(), "{:?}", codegen.diagnostics);

    let output = emit_llvm_ir(&codegen.backend_lowering, &codegen.type_store);
    assert!(output.diagnostics.is_empty(), "{:?}", output.diagnostics);
    let ir = &output.modules[0].ir;
    assert!(ir.contains("call void @llvm.trap()"), "{ir}");
    assert!(ir.contains("declare void @llvm.trap()"), "{ir}");
    assert!(ir.contains("unreachable"), "{ir}");
}

#[test]
fn emits_std_builtin_trap_as_llvm_trap() {
    let root = temp_dir("emits_std_builtin_trap_as_llvm_trap");
    let main = root.join("main.nia");
    std::fs::write(
        &main,
        r#"
fn main() i32 {
    std::builtin::trap()
}
"#,
    )
    .expect("write test source");

    let codegen = codegen_program(main.to_string_lossy().into_owned());
    assert!(codegen.diagnostics.is_empty(), "{:?}", codegen.diagnostics);

    let output = emit_llvm_ir(&codegen.backend_lowering, &codegen.type_store);
    assert!(output.diagnostics.is_empty(), "{:?}", output.diagnostics);
    let ir = &output.modules[0].ir;
    assert!(ir.contains("call void @llvm.trap()"), "{ir}");
    assert!(ir.contains("declare void @llvm.trap()"), "{ir}");
    assert!(ir.contains("unreachable"), "{ir}");
}

#[test]
fn const_function_trap_is_available_at_comptime_and_runtime() {
    let root = temp_dir("const_function_trap_is_available_at_comptime_and_runtime");
    let main = root.join("main.nia");
    std::fs::write(
        &main,
        r#"
const fn selected(flag: bool) usize {
    if flag {
        std::builtin::trap();
    }
    4
}

const n: usize = selected(false);

fn main(flag: bool) usize {
    selected(flag) + n
}
"#,
    )
    .expect("write test source");

    let codegen = codegen_program(main.to_string_lossy().into_owned());
    assert!(codegen.diagnostics.is_empty(), "{:?}", codegen.diagnostics);

    let output = emit_llvm_ir(&codegen.backend_lowering, &codegen.type_store);
    assert!(output.diagnostics.is_empty(), "{:?}", output.diagnostics);
    let ir = &output.modules[0].ir;
    assert!(ir.contains("call void @llvm.trap()"), "{ir}");
    assert!(ir.contains("declare void @llvm.trap()"), "{ir}");
    assert!(ir.contains("unreachable"), "{ir}");
}

#[test]
fn emits_union_storage_and_field_access() {
    let root = temp_dir("emits_union_storage_and_field_access");
    let main = root.join("main.nia");
    std::fs::write(
        &main,
        r#"
union Bits {
    i: i32,
    f: f32,
}

fn main() i32 {
    let mut bits: Bits = { i: 42 };
    bits.i
}
"#,
    )
    .expect("write test source");

    let codegen = codegen_program(main.to_string_lossy().into_owned());
    assert!(codegen.diagnostics.is_empty(), "{:?}", codegen.diagnostics);

    let output = emit_llvm_ir(&codegen.backend_lowering, &codegen.type_store);
    assert!(output.diagnostics.is_empty(), "{:?}", output.diagnostics);
    let ir = &output.modules[0].ir;
    assert_contains_mangled_symbol(ir, '%', "Bits");
    assert!(ir.contains("store i32 42"));
    assert!(ir.contains("ret i32"));
}

#[test]
fn emits_const_values_without_runtime_storage() {
    let root = temp_dir("emits_const_values_without_runtime_storage");
    let main = root.join("main.nia");
    std::fs::write(
        &main,
        r#"
const answer: i32 = 40 + 2;
static saved: i32 = answer;

fn main() i32 {
    const local: i32 = answer;
    local
}
"#,
    )
    .expect("write test source");

    let codegen = codegen_program(main.to_string_lossy().into_owned());
    assert!(codegen.diagnostics.is_empty(), "{:?}", codegen.diagnostics);

    let output = emit_llvm_ir(&codegen.backend_lowering, &codegen.type_store);
    assert!(output.diagnostics.is_empty(), "{:?}", output.diagnostics);
    let ir = &output.modules[0].ir;
    assert!(ir.contains("ret i32 42"), "{ir}");
    assert!(ir.contains("@nia__s"));
    assert!(!ir.contains("answer"), "{ir}");
    assert!(!ir.contains("local"), "{ir}");
}

#[test]
fn emits_runtime_storage_for_static_string_values() {
    let root = temp_dir("emits_runtime_storage_for_static_string_values");
    let main = root.join("main.nia");
    std::fs::write(
        &main,
        r#"
static bytes = b"paw\0";
static text = "nia";

fn first_byte(xs: &[u8]) i32 {
    xs[0] as i32
}

fn main() i32 {
    let mut byte_ptr: &u8 = &bytes[1];
    let mut byte_slice: &[u8] = &bytes;
    let mut char_slice: &[char] = &text;
    first_byte(&bytes) + byte_ptr.* as i32 + byte_slice.len() as i32 + char_slice.len() as i32
}
"#,
    )
    .expect("write test source");

    let codegen = codegen_program(main.to_string_lossy().into_owned());
    assert!(codegen.diagnostics.is_empty(), "{:?}", codegen.diagnostics);

    let output = emit_llvm_ir(&codegen.backend_lowering, &codegen.type_store);
    assert!(output.diagnostics.is_empty(), "{:?}", output.diagnostics);
    let ir = &output.modules[0].ir;
    assert!(ir.contains("[4 x i8] c\"paw\\00\""), "{ir}");
    assert!(ir.contains("[3 x i32] [i32 110, i32 105, i32 97]"), "{ir}");
    assert!(!ir.contains("cannot emit erroneous expression"), "{ir}");
}

#[test]
fn emits_arrays_sized_by_imported_const_values() {
    let root = temp_dir("emits_arrays_sized_by_imported_const_values");
    let main = root.join("main.nia");
    std::fs::write(
        &main,
        r#"
module config;
using entry::config;

fn main() i32 {
    let mut values: [config::width]i32 = [1, 2, 3, 4];
    values[3]
}
"#,
    )
    .expect("write main source");
    std::fs::write(
        root.join("config.nia"),
        r#"
pub const width: usize = 4;
"#,
    )
    .expect("write config source");

    let codegen = codegen_program(main.to_string_lossy().into_owned());
    assert!(codegen.diagnostics.is_empty(), "{:?}", codegen.diagnostics);
    let output = emit_llvm_ir(&codegen.backend_lowering, &codegen.type_store);
    assert!(output.diagnostics.is_empty(), "{:?}", output.diagnostics);
    assert!(output.modules[0].ir.contains("[4 x i32]"));
}

#[test]
fn emits_imported_struct_array_field_repeat_literals() {
    let root = temp_dir("emits_imported_struct_array_field_repeat_literals");
    let main = root.join("main.nia");
    std::fs::write(
        root.join("defs.nia"),
        r#"
pub const N: usize = 4;

pub struct Item {
    value: u32,
}

pub struct Boxed {
    items: [N]Item,
}

extend Item {
    pub fn zero() Item {
        { value: 0 }
    }
}
"#,
    )
    .expect("write defs source");
    std::fs::write(
        &main,
        r#"
module defs;
using entry::defs;
using defs::*;

fn literal_count() Boxed {
    {
        items: [Item::zero(); 4],
    }
}

fn imported_count() Boxed {
    {
        items: [Item::zero(); defs::N],
    }
}

fn main() i32 {
    let mut a = literal_count();
    let mut b = imported_count();
    a.items[0].value as i32 + b.items[0].value as i32
}
"#,
    )
    .expect("write main source");

    let codegen = codegen_program(main.to_string_lossy().into_owned());
    assert!(codegen.diagnostics.is_empty(), "{:?}", codegen.diagnostics);
    let output = emit_llvm_ir(&codegen.backend_lowering, &codegen.type_store);
    assert!(output.diagnostics.is_empty(), "{:?}", output.diagnostics);
    let ir = source_module_ir(&output, "main.nia");
    assert!(ir.contains("[4 x %"), "{ir:?}");
}

#[test]
fn emits_imported_generic_struct_with_imported_const_array_field_length() {
    let root = temp_dir("emits_imported_generic_struct_with_imported_const_array_field_length");
    let main = root.join("main.nia");
    std::fs::write(
        &main,
        r#"
module defs;
using entry::defs;
using defs::Boxed;

fn take(box: Boxed[u8]) u8 {
    box.values[2]
}

fn main() u8 {
    let mut box: Boxed[u8] = { values: [1, 2, 3] };
    take(box)
}
"#,
    )
    .expect("write main source");
    std::fs::write(
        root.join("defs.nia"),
        r#"
pub const N: usize = 3;

pub struct Boxed[T] {
    values: [N]T,
}
"#,
    )
    .expect("write defs source");

    let codegen = codegen_program(main.to_string_lossy().into_owned());
    assert!(codegen.diagnostics.is_empty(), "{:?}", codegen.diagnostics);
    let output = emit_llvm_ir(&codegen.backend_lowering, &codegen.type_store);
    assert!(output.diagnostics.is_empty(), "{:?}", output.diagnostics);
    assert!(output.modules[0].ir.contains("[3 x i8]"));
}

#[test]
fn emits_large_array_repeat_count_from_const_binding() {
    let root = temp_dir("emits_large_array_repeat_count_from_const_binding");
    let main = root.join("main.nia");
    std::fs::write(
        &main,
        r#"
const N: usize = 16;

fn main() i32 {
    let mut buffer: [N]u8 = [0u8; N];
    0
}
"#,
    )
    .expect("write test source");

    let codegen = codegen_program(main.to_string_lossy().into_owned());
    assert!(codegen.diagnostics.is_empty(), "{:?}", codegen.diagnostics);

    let output = emit_llvm_ir(&codegen.backend_lowering, &codegen.type_store);
    assert!(output.diagnostics.is_empty(), "{:?}", output.diagnostics);
    assert!(output.modules[0].ir.contains("[16 x i8]"));
}
