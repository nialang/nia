// SPDX-License-Identifier: GPL-3.0-or-later
use super::common::*;

#[test]
fn rejects_bare_global_as_pointer_initializer() {
    let root = temp_dir("rejects_bare_global_as_pointer_initializer");
    let main = root.join("main.nia");
    std::fs::write(
        &main,
        r#"
var target: i32 = 1;
let p: &i32 = target;

fn main() i32 {
    target
}
"#,
    )
    .expect("write test source");

    let checked = nia_driver::check_program(main.to_string_lossy().into_owned());
    assert!(
        checked
            .diagnostics
            .iter()
            .any(|diagnostic| diagnostic.diagnostic.summary.contains("global initializer")),
        "{:?}",
        checked.diagnostics
    );
}

#[test]
fn emits_inline_asm_inputs_outputs_and_clobbers() {
    let root = temp_dir("emits_inline_asm_inputs_outputs_and_clobbers");
    let main = root.join("main.nia");
    std::fs::write(
        &main,
        r#"
fn main() i32 {
    var value: i64 = 0;
    @asm({
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

    let checked = nia_driver::check_program(main.to_string_lossy().into_owned());
    assert!(checked.diagnostics.is_empty(), "{:?}", checked.diagnostics);

    let output = emit_llvm_ir(&checked.backend_lowering.program);
    assert!(output.diagnostics.is_empty(), "{:?}", output.diagnostics);
    let ir = &output.modules[0].ir;
    assert!(ir.contains("asm sideeffect"));
    assert!(ir.contains("mov rax, rax\\0Aadd rax, 0"));
    assert!(ir.contains("={rax},{rax},~{memory}"), "{ir}");
}

#[test]
fn emits_trap_builtin_as_llvm_trap() {
    let root = temp_dir("emits_trap_builtin_as_llvm_trap");
    let main = root.join("main.nia");
    std::fs::write(
        &main,
        r#"
fn main() i32 {
    @trap()
}

fn statement() void {
    @trap();
}
"#,
    )
    .expect("write test source");

    let checked = nia_driver::check_program(main.to_string_lossy().into_owned());
    assert!(checked.diagnostics.is_empty(), "{:?}", checked.diagnostics);

    let output = emit_llvm_ir(&checked.backend_lowering.program);
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
    var bits: Bits = { i: 42 };
    bits.i
}
"#,
    )
    .expect("write test source");

    let checked = nia_driver::check_program(main.to_string_lossy().into_owned());
    assert!(checked.diagnostics.is_empty(), "{:?}", checked.diagnostics);

    let output = emit_llvm_ir(&checked.backend_lowering.program);
    assert!(output.diagnostics.is_empty(), "{:?}", output.diagnostics);
    let ir = &output.modules[0].ir;
    assert!(ir.contains("%nia__m0__d0__Bits"));
    assert!(ir.contains("store i32 42"));
    assert!(ir.contains("ret i32"));
}

#[test]
fn emits_comptime_values_without_runtime_storage() {
    let root = temp_dir("emits_comptime_values_without_runtime_storage");
    let main = root.join("main.nia");
    std::fs::write(
        &main,
        r#"
comptime let answer: i32 = 40 + 2;
let saved: i32 = answer;

fn main() i32 {
    comptime let local: i32 = answer;
    local
}
"#,
    )
    .expect("write test source");

    let checked = nia_driver::check_program(main.to_string_lossy().into_owned());
    assert!(checked.diagnostics.is_empty(), "{:?}", checked.diagnostics);

    let output = emit_llvm_ir(&checked.backend_lowering.program);
    assert!(output.diagnostics.is_empty(), "{:?}", output.diagnostics);
    let ir = &output.modules[0].ir;
    assert!(ir.contains("ret i32 42"), "{ir}");
    assert!(ir.contains("@nia__m0__d"));
    assert!(!ir.contains("answer"), "{ir}");
    assert!(!ir.contains("local"), "{ir}");
}

#[test]
fn emits_arrays_sized_by_imported_comptime_values() {
    let root = temp_dir("emits_arrays_sized_by_imported_comptime_values");
    let main = root.join("main.nia");
    std::fs::write(
        &main,
        r#"
import .config;

fn main() i32 {
    var values: [config::width]i32 = [1, 2, 3, 4];
    values[3]
}
"#,
    )
    .expect("write main source");
    std::fs::write(
        root.join("config.nia"),
        r#"
pub comptime let width: usize = 4;
"#,
    )
    .expect("write config source");

    let checked = nia_driver::check_program(main.to_string_lossy().into_owned());
    assert!(checked.diagnostics.is_empty(), "{:?}", checked.diagnostics);
    let output = emit_llvm_ir(&checked.backend_lowering.program);
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
pub comptime let N: usize = 4;

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
import .defs;
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
    var a = literal_count();
    var b = imported_count();
    a.items[0].value as i32 + b.items[0].value as i32
}
"#,
    )
    .expect("write main source");

    let checked = nia_driver::check_program(main.to_string_lossy().into_owned());
    assert!(checked.diagnostics.is_empty(), "{:?}", checked.diagnostics);
    let output = emit_llvm_ir(&checked.backend_lowering.program);
    assert!(output.diagnostics.is_empty(), "{:?}", output.diagnostics);
    assert!(
        output.modules[0].ir.contains("[4 x %"),
        "{:?}",
        output.modules[0].ir
    );
}

#[test]
fn emits_large_array_repeat_count_from_comptime_binding() {
    let root = temp_dir("emits_large_array_repeat_count_from_comptime_binding");
    let main = root.join("main.nia");
    std::fs::write(
        &main,
        r#"
comptime let N: usize = 16;

fn main() i32 {
    var buffer: [N]u8 = [0u8; N];
    0
}
"#,
    )
    .expect("write test source");

    let checked = nia_driver::check_program(main.to_string_lossy().into_owned());
    assert!(checked.diagnostics.is_empty(), "{:?}", checked.diagnostics);

    let output = emit_llvm_ir(&checked.backend_lowering.program);
    assert!(output.diagnostics.is_empty(), "{:?}", output.diagnostics);
    assert!(output.modules[0].ir.contains("[16 x i8]"));
}
