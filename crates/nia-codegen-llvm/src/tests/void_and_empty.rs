// SPDX-License-Identifier: GPL-3.0-or-later
use super::common::*;

#[test]
fn emits_tuple_returns_patterns_and_unit_values() {
    let root = temp_dir("emits_tuple_returns_patterns_and_unit_values");
    let main = root.join("main.nia");
    std::fs::write(
        &main,
        r#"
fn make_pair() (i32, bool) {
    (40, true)
}

fn make_single() (i32,) {
    (2,)
}

fn explicit_unit() () {
    ()
}

fn implicit_unit() {
    explicit_unit()
}

fn classify(pair: (i32, (bool, i32))) i32 {
    if pair is (40, (true, value)) {
        value
    } else {
        match pair {
            (left, (false, right)) => left + right,
            (_, (_, fallback)) => fallback,
        }
    }
}

fn main() i32 {
    implicit_unit();
    let (left, enabled) = make_pair();
    let (right,) = make_single();
    let mut nested = ((left, right), ());
    nested.0.0 = nested.0.0 + nested.0.1;
    let answer = nested.0.0;
    if enabled { answer + classify((40, (true, 1))) } else { 0 }
}
"#,
    )
    .expect("write test source");

    let codegen = codegen_program(main.to_string_lossy().into_owned());
    assert!(codegen.diagnostics.is_empty(), "{:?}", codegen.diagnostics);

    let output = emit_llvm_ir(&codegen.backend_lowering, &codegen.type_store);
    assert!(output.diagnostics.is_empty(), "{:?}", output.diagnostics);
    let ir = &output.modules[0].ir;
    assert!(ir.contains("define void @"), "{ir}");
    assert!(ir.contains("tuple.field"), "{ir}");
    assert!(ir.contains("tuple.place.field.ptr"), "{ir}");
    assert!(ir.contains("ret i32"), "{ir}");
}

#[test]
fn emits_void_values_and_empty_structs() {
    let root = temp_dir("emits_void_values_and_empty_structs");
    let main = root.join("main.nia");
    std::fs::write(
        &main,
        r#"
struct Empty {}

fn sink(p: &opaque) {}

fn main() i32 {
    let mut unit: () = {};
    let mut empty = Empty {};
    let mut value: i32 = 7;
    sink(&value as &opaque);
    0
}
"#,
    )
    .expect("write test source");

    let codegen = codegen_program(main.to_string_lossy().into_owned());
    assert!(codegen.diagnostics.is_empty(), "{:?}", codegen.diagnostics);

    let output = emit_llvm_ir(&codegen.backend_lowering, &codegen.type_store);
    assert!(output.diagnostics.is_empty(), "{:?}", output.diagnostics);
    let ir = &output.modules[0].ir;
    assert!(ir.contains("define void @"));
    assert!(ir.contains("call void @"));
    assert!(ir.contains("ret i32 0"));
}

#[test]
fn emits_empty_struct_literals_in_runtime_contexts() {
    let root = temp_dir("emits_empty_struct_literals_in_runtime_contexts");
    let main = root.join("main.nia");
    std::fs::write(
        &main,
        r#"
module defs;
using entry::defs;

struct LocalEmpty {}

fn take_local(value: LocalEmpty) i32 {
    1
}

fn main() i32 {
    let mut local = LocalEmpty {};
    let mut imported = defs::Empty {};
    take_local(local) + defs::take(imported)
}
"#,
    )
    .expect("write main source");
    std::fs::write(
        root.join("defs.nia"),
        r#"
pub struct Empty {}

pub fn take(value: Empty) i32 {
    2
}
"#,
    )
    .expect("write defs source");

    let codegen = codegen_program(main.to_string_lossy().into_owned());
    assert!(codegen.diagnostics.is_empty(), "{:?}", codegen.diagnostics);

    let output = emit_llvm_ir(&codegen.backend_lowering, &codegen.type_store);
    assert!(output.diagnostics.is_empty(), "{:?}", output.diagnostics);
    let main_ir = output
        .modules
        .iter()
        .find(|module| module.name.ends_with("main.nia"))
        .expect("main module IR");
    let take = mangled_symbol(&main_ir.ir, '@', "take");
    let take_local = mangled_symbol(&main_ir.ir, '@', "take_local");
    assert!(
        main_ir.ir.contains(&format!("call i32 {take_local}()")),
        "{}",
        main_ir.ir
    );
    assert!(
        main_ir.ir.contains(&format!("call i32 {take}()")),
        "{}",
        main_ir.ir
    );
}

#[test]
fn emits_empty_struct_literals_in_return_and_call_contexts() {
    let root = temp_dir("emits_empty_struct_literals_in_return_and_call_contexts");
    let main = root.join("main.nia");
    std::fs::write(
        &main,
        r#"
module defs;
using entry::defs;

struct LocalEmpty {}

fn make_local() LocalEmpty {
    {}
}

fn take_local(value: LocalEmpty) i32 {
    3
}

fn main() i32 {
    take_local({}) + defs::take({}) + take_local(make_local()) + defs::take(defs::make())
}
"#,
    )
    .expect("write main source");
    std::fs::write(
        root.join("defs.nia"),
        r#"
pub struct Empty {}

pub fn make() Empty {
    {}
}

pub fn take(value: Empty) i32 {
    5
}
"#,
    )
    .expect("write defs source");

    let codegen = codegen_program(main.to_string_lossy().into_owned());
    assert!(codegen.diagnostics.is_empty(), "{:?}", codegen.diagnostics);

    let output = emit_llvm_ir(&codegen.backend_lowering, &codegen.type_store);
    assert!(output.diagnostics.is_empty(), "{:?}", output.diagnostics);
    let main_ir = output
        .modules
        .iter()
        .find(|module| module.name.ends_with("main.nia"))
        .expect("main module IR");
    let take_local = mangled_symbol(&main_ir.ir, '@', "take_local");
    let take = mangled_symbol(&main_ir.ir, '@', "take");
    assert!(
        main_ir.ir.contains(&format!("call i32 {take_local}()")),
        "{}",
        main_ir.ir
    );
    assert!(
        main_ir.ir.contains(&format!("call i32 {take}()")),
        "{}",
        main_ir.ir
    );
}

#[test]
fn emits_zero_sized_local_assignment_and_return_without_payload_loads() {
    let root = temp_dir("emits_zero_sized_local_assignment_and_return_without_payload_loads");
    let main = root.join("main.nia");
    std::fs::write(
        &main,
        r#"
struct Empty {}

fn id(value: Empty) Empty {
    let mut out = Empty {};
    out = value;
    out
}

fn main() i32 {
    _ = id({});
    0
}
"#,
    )
    .expect("write test source");

    let codegen = codegen_program(main.to_string_lossy().into_owned());
    assert!(codegen.diagnostics.is_empty(), "{:?}", codegen.diagnostics);

    let output = emit_llvm_ir(&codegen.backend_lowering, &codegen.type_store);
    assert!(output.diagnostics.is_empty(), "{:?}", output.diagnostics);
    let ir = &output.modules[0].ir;
    assert!(ir.contains("define void @"), "{ir}");
    assert!(ir.contains("ret void"), "{ir}");
    assert!(!ir.contains("load %"), "{ir}");
}

#[test]
fn emits_generic_empty_struct_literals_across_modules() {
    let root = temp_dir("emits_generic_empty_struct_literals_across_modules");
    let main = root.join("main.nia");
    std::fs::write(
        &main,
        r#"
module defs;
using entry::defs;

struct LocalBox[T] {}

fn take_local(value: LocalBox[i32]) i32 {
    7
}

fn main() i32 {
    let mut local = LocalBox[i32] {};
    let mut imported = defs::Box[i32] {};
    take_local(local) + defs::take(imported) + defs::take({})
}
"#,
    )
    .expect("write main source");
    std::fs::write(
        root.join("defs.nia"),
        r#"
pub struct Box[T] {}

pub fn take(value: Box[i32]) i32 {
    11
}
"#,
    )
    .expect("write defs source");

    let codegen = codegen_program(main.to_string_lossy().into_owned());
    assert!(codegen.diagnostics.is_empty(), "{:?}", codegen.diagnostics);

    let output = emit_llvm_ir(&codegen.backend_lowering, &codegen.type_store);
    assert!(output.diagnostics.is_empty(), "{:?}", output.diagnostics);
    let main_ir = output
        .modules
        .iter()
        .find(|module| module.name.ends_with("main.nia"))
        .expect("main module IR");
    let take_local = mangled_symbol(&main_ir.ir, '@', "take_local");
    let take = mangled_symbol(&main_ir.ir, '@', "take");
    assert!(
        main_ir.ir.contains(&format!("call i32 {take_local}()")),
        "{}",
        main_ir.ir
    );
    assert!(
        main_ir.ir.contains(&format!("call i32 {take}()")),
        "{}",
        main_ir.ir
    );
}

#[test]
fn emits_error_union_void_success_values() {
    let root = temp_dir("emits_error_union_void_success_values");
    let main = root.join("main.nia");
    std::fs::write(
        &main,
        r#"
enum Error: i32 {
    Fail = 1,
}

fn ok() Error!() {
    !()
}

fn main() i32 {
    match ok() {
        !value => {
            _ = value;
            0
        },
        error! => {
            1
        },
    }
}
"#,
    )
    .expect("write test source");

    let codegen = codegen_program(main.to_string_lossy().into_owned());
    assert!(codegen.diagnostics.is_empty(), "{:?}", codegen.diagnostics);

    let output = emit_llvm_ir(&codegen.backend_lowering, &codegen.type_store);
    assert!(output.diagnostics.is_empty(), "{:?}", output.diagnostics);
}

#[test]
fn emits_ref_receiver_method_call_on_aggregate_rvalue() {
    let root = temp_dir("emits_ref_receiver_method_call_on_aggregate_rvalue");
    let main = root.join("main.nia");
    std::fs::write(
        &main,
        r#"
struct Pair {
    left: i32,
    right: i32,
}

extend Pair {
    fn sum(&self) i32 {
        self.left + self.right
    }
}

fn make() Pair {
    Pair { left: 20, right: 22 }
}

fn main() i32 {
    make().sum()
}
"#,
    )
    .expect("write test source");

    let codegen = codegen_program(main.to_string_lossy().into_owned());
    assert!(codegen.diagnostics.is_empty(), "{:?}", codegen.diagnostics);

    let output = emit_llvm_ir(&codegen.backend_lowering, &codegen.type_store);
    assert!(output.diagnostics.is_empty(), "{:?}", output.diagnostics);
}

#[test]
fn emits_discarded_void_calls() {
    let root = temp_dir("emits_discarded_void_calls");
    let main = root.join("main.nia");
    std::fs::write(
        &main,
        r#"
extern fn printf(fmt: & u8, ...);

fn effect() {}
fn value() i32 { 7 }

fn main() i32 {
    let fmt = b"ok\n\0";
    _ = effect();
    _ = printf(&fmt[0]);
    _ = value();
    0
}
"#,
    )
    .expect("write test source");

    let codegen = codegen_program(main.to_string_lossy().into_owned());
    assert!(codegen.diagnostics.is_empty(), "{:?}", codegen.diagnostics);

    let output = emit_llvm_ir(&codegen.backend_lowering, &codegen.type_store);
    assert!(output.diagnostics.is_empty(), "{:?}", output.diagnostics);
    let ir = &output.modules[0].ir;
    let effect = mangled_symbol(ir, '@', "effect");
    let value = mangled_symbol(ir, '@', "value");
    assert!(ir.contains("declare void @printf"));
    assert!(ir.contains("call void (ptr, ...) @printf"));
    assert!(ir.contains(&format!("call void {effect}")), "{ir}");
    assert!(ir.contains(&format!("call i32 {value}")), "{ir}");
}

#[test]
fn emits_addresses_for_zero_sized_locals() {
    let root = temp_dir("emits_addresses_for_zero_sized_locals");
    let main = root.join("main.nia");
    std::fs::write(
        &main,
        r#"
struct Empty {}

extern fn observe(value: &Empty);

fn main() i32 {
    let mut empty = Empty {};
    observe(&empty);
    0
}
"#,
    )
    .expect("write test source");

    let codegen = codegen_program(main.to_string_lossy().into_owned());
    assert!(codegen.diagnostics.is_empty(), "{:?}", codegen.diagnostics);

    let output = emit_llvm_ir(&codegen.backend_lowering, &codegen.type_store);
    assert!(output.diagnostics.is_empty(), "{:?}", output.diagnostics);
    let ir = &output.modules[0].ir;
    assert!(ir.contains("%zst.local = alloca i8"), "{ir}");
    assert!(ir.contains("call void @observe(ptr %zst.local)"), "{ir}");
}

#[test]
fn passes_zero_sized_method_receivers_by_address() {
    let root = temp_dir("passes_zero_sized_method_receivers_by_address");
    let main = root.join("main.nia");
    std::fs::write(
        &main,
        r#"
struct Empty {}

extend Empty {
    fn value(&mut self) i32 {
        _ = self;
        7
    }
}

fn main() i32 {
    let mut empty = Empty {};
    empty.value()
}
"#,
    )
    .expect("write test source");

    let codegen = codegen_program(main.to_string_lossy().into_owned());
    assert!(codegen.diagnostics.is_empty(), "{:?}", codegen.diagnostics);

    let output = emit_llvm_ir(&codegen.backend_lowering, &codegen.type_store);
    assert!(output.diagnostics.is_empty(), "{:?}", output.diagnostics);
    let ir = &output.modules[0].ir;
    assert!(ir.contains("%zst.local = alloca i8"), "{ir}");
    assert!(ir.contains("call i32 @"), "{ir}");
}

#[test]
fn preserves_effects_inside_zero_sized_aggregate_literals() {
    let root = temp_dir("preserves_effects_inside_zero_sized_aggregate_literals");
    let main = root.join("main.nia");
    std::fs::write(
        &main,
        r#"
struct Wrap {
    value: (),
}

extern fn log(value: i32);

fn effect(value: i32) () {
    log(value);
}

fn take(value: Wrap) () {}
fn take_array(value: [(); 2]) () {}

fn main() i32 {
    let mut local = Wrap { value: effect(1) };
    take(Wrap { value: effect(2) });
    take_array([effect(3), effect(4)]);
    take_array([effect(5); 2]);
    0
}
"#,
    )
    .expect("write test source");

    let codegen = codegen_program(main.to_string_lossy().into_owned());
    assert!(codegen.diagnostics.is_empty(), "{:?}", codegen.diagnostics);

    let output = emit_llvm_ir(&codegen.backend_lowering, &codegen.type_store);
    assert!(output.diagnostics.is_empty(), "{:?}", output.diagnostics);
    let ir = &output.modules[0].ir;
    let effect = mangled_symbol(ir, '@', "effect");
    let take = mangled_symbol(ir, '@', "take");
    let take_array = mangled_symbol(ir, '@', "take_array");
    for value in 1..=5 {
        assert!(
            ir.contains(&format!("call void {effect}(i32 {value})")),
            "{ir}"
        );
    }
    assert_eq!(ir.matches(&format!("call void {effect}(i32 5)")).count(), 2);
    assert!(ir.contains(&format!("call void {take}()")), "{ir}");
    assert!(ir.contains(&format!("call void {take_array}()")), "{ir}");
}
