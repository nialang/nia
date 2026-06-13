// SPDX-License-Identifier: GPL-3.0-or-later
use super::common::*;

#[test]
fn emits_imported_open_enum_as_error_union_payload() {
    let root = temp_dir("emits_imported_open_enum_as_error_union_payload");
    let main = root.join("main.nia");
    std::fs::write(
        &main,
        r#"
module errors;
using root::errors;

fn maybe(flag: bool) errors::Error!i32 {
    if flag {
        !40
    } else {
        errors::Error::Io!
    }
}

fn add_two(flag: bool) errors::Error!i32 {
    var value = maybe(flag).?;
    !(value + 2)
}

fn main() i32 {
    if let !value = add_two(true) {
        value
    } else error! {
        error as i32
    }
}
"#,
    )
    .expect("write main source");
    std::fs::write(
        root.join("errors.nia"),
        r#"
pub enum Error: i32 {
    Io = 5,
    _,
}
"#,
    )
    .expect("write errors source");

    let checked = nia_driver::check_program(main.to_string_lossy().into_owned());
    assert!(checked.diagnostics.is_empty(), "{:?}", checked.diagnostics);

    let output = emit_llvm_ir(&checked.backend_lowering.program);
    assert!(output.diagnostics.is_empty(), "{:?}", output.diagnostics);
}

#[test]
fn emits_local_struct_with_imported_nominal_field() {
    let root = temp_dir("emits_local_struct_with_imported_nominal_field");
    let main = root.join("main.nia");
    std::fs::write(
        &main,
        r#"
module defs;
using root::defs;
using defs::Item;

struct HoldsItem {
    item: Item,
}

fn make() HoldsItem {
    {
        item: Item::zero(),
    }
}

fn main() i32 {
    var held = make();
    held.item.value
}
"#,
    )
    .expect("write main source");
    std::fs::write(
        root.join("defs.nia"),
        r#"
pub struct Item {
    value: i32,
}

extend Item {
    pub fn zero() Item {
        { value: 0 }
    }
}
"#,
    )
    .expect("write defs source");

    let checked = nia_driver::check_program(main.to_string_lossy().into_owned());
    assert!(checked.diagnostics.is_empty(), "{:?}", checked.diagnostics);

    let output = emit_llvm_ir(&checked.backend_lowering.program);
    assert!(output.diagnostics.is_empty(), "{:?}", output.diagnostics);
    let main_ir = output
        .modules
        .iter()
        .find(|module| module.name.ends_with("main.nia"))
        .expect("main module IR");
    assert!(main_ir.ir.contains("__HoldsItem"));
    assert!(main_ir.ir.contains("__Item"));
    assert!(main_ir.ir.contains("ret i32"));
}

#[test]
fn emits_local_struct_array_field_when_module_has_import() {
    let root = temp_dir("emits_local_struct_array_field_when_module_has_import");
    let main = root.join("main.nia");
    std::fs::write(
        &main,
        r#"
module empty;
using root::empty;

struct S {
    x: i32,
}

struct T {
    xs: [256]S,
}

fn main() i32 {
    var t: T = { xs: [{ x: 0 }; 256] };
    t.xs[255].x
}
"#,
    )
    .expect("write main source");
    std::fs::write(
        root.join("empty.nia"),
        r#"
pub fn value() i32 {
    0
}
"#,
    )
    .expect("write empty source");

    let checked = nia_driver::check_program(main.to_string_lossy().into_owned());
    assert!(checked.diagnostics.is_empty(), "{:?}", checked.diagnostics);

    let output = emit_llvm_ir(&checked.backend_lowering.program);
    assert!(output.diagnostics.is_empty(), "{:?}", output.diagnostics);
    let main_ir = output
        .modules
        .iter()
        .find(|module| module.name.ends_with("main.nia"))
        .expect("main module IR");
    assert!(main_ir.ir.contains("[256 x %nia__m0__d"), "{}", main_ir.ir);
}

#[test]
fn emits_imported_struct_array_field_expression_edges() {
    let root = temp_dir("emits_imported_struct_array_field_expression_edges");
    let main = root.join("main.nia");
    std::fs::write(
        &main,
        r#"
module defs;
using root::defs;

fn main() i32 {
    var bag = defs::make_bag();
    var i: usize = 2;
    bag.items[i] = defs::make_item(5);
    var tail = & bag.items[1..=2];
    bag.items.len() as i32 + tail.len() as i32 + bag.items[i].value
}
"#,
    )
    .expect("write main source");
    std::fs::write(
        root.join("defs.nia"),
        r#"
pub struct Item {
    value: i32,
}

pub struct Bag {
    items: [4]Item,
}

pub fn make_item(value: i32) Item {
    { value: value }
}

pub fn make_bag() Bag {
    { items: [make_item(1); 4] }
}
"#,
    )
    .expect("write defs source");

    let checked = nia_driver::check_program(main.to_string_lossy().into_owned());
    assert!(checked.diagnostics.is_empty(), "{:?}", checked.diagnostics);

    let output = emit_llvm_ir(&checked.backend_lowering.program);
    assert!(output.diagnostics.is_empty(), "{:?}", output.diagnostics);
    let main_ir = output
        .modules
        .iter()
        .find(|module| module.name.ends_with("main.nia"))
        .expect("main module IR");
    assert!(main_ir.ir.contains("getelementptr"), "{}", main_ir.ir);
    assert!(main_ir.ir.contains("ret i32"), "{}", main_ir.ir);
}

#[test]
fn emits_imported_aggregate_function_pointer_call_abi() {
    let root = temp_dir("emits_imported_aggregate_function_pointer_call_abi");
    let main = root.join("main.nia");
    std::fs::write(
        &main,
        r#"
module defs;
using root::defs;

fn main() i32 {
    var callback: &fn(defs::Pair) defs::Pair = & defs::id_pair;
    var pair = callback(defs::make_pair(2, 5));
    pair.a + pair.b
}
"#,
    )
    .expect("write main source");
    std::fs::write(
        root.join("defs.nia"),
        r#"
pub struct Pair {
    a: i32,
    b: i32,
}

pub fn make_pair(a: i32, b: i32) Pair {
    { a: a, b: b }
}

pub fn id_pair(pair: Pair) Pair {
    pair
}
"#,
    )
    .expect("write defs source");

    let checked = nia_driver::check_program(main.to_string_lossy().into_owned());
    assert!(checked.diagnostics.is_empty(), "{:?}", checked.diagnostics);

    let output = emit_llvm_ir(&checked.backend_lowering.program);
    assert!(output.diagnostics.is_empty(), "{:?}", output.diagnostics);
    let main_ir = output
        .modules
        .iter()
        .find(|module| module.name.ends_with("main.nia"))
        .expect("main module IR");
    assert!(main_ir.ir.contains("call void %"), "{}", main_ir.ir);
    assert!(main_ir.ir.contains("ret i32"), "{}", main_ir.ir);
}

#[test]
fn emits_imported_enum_variant_values_and_switch_patterns() {
    let root = temp_dir("emits_imported_enum_variant_values_and_switch_patterns");
    let main = root.join("main.nia");
    std::fs::write(
        &main,
        r#"
module module_enum_defs;
using root::module_enum_defs;

using module_enum_defs::Mode;

fn main() i32 {
    var box = module_enum_defs::make_box();
    switch box.mode {
        module_enum_defs::Mode::A => Mode::A as u8 as i32,
        module_enum_defs::Mode::B => 2,
        _ => 3,
    }
}
"#,
    )
    .expect("write main source");
    std::fs::write(
        root.join("module_enum_defs.nia"),
        r#"
pub enum Mode: u8 {
    A,
    B,
}

pub struct Box {
    mode: Mode,
}

pub fn make_box() Box {
    { mode: Mode::A }
}
"#,
    )
    .expect("write module source");

    let checked = nia_driver::check_program(main.to_string_lossy().into_owned());
    assert!(checked.diagnostics.is_empty(), "{:?}", checked.diagnostics);

    let output = emit_llvm_ir(&checked.backend_lowering.program);
    assert!(output.diagnostics.is_empty(), "{:?}", output.diagnostics);
    let main_ir = output
        .modules
        .iter()
        .find(|module| module.name.ends_with("main.nia"))
        .expect("main module IR");
    assert!(main_ir.ir.contains("switch i8"), "{}", main_ir.ir);
}

#[test]
fn emits_enum_switch_with_only_returning_arms() {
    let root = temp_dir("emits_enum_switch_with_only_returning_arms");
    let main = root.join("main.nia");
    std::fs::write(
        &main,
        r#"
module module_enum_defs;
using root::module_enum_defs;

fn main() i32 {
    var box = module_enum_defs::make_box();
    switch box.mode {
        module_enum_defs::Mode::A => return 1,
        module_enum_defs::Mode::B => return 2,
        _ => return 3,
    }
}
"#,
    )
    .expect("write main source");
    std::fs::write(
        root.join("module_enum_defs.nia"),
        r#"
pub enum Mode: u8 {
    A,
    B,
}

pub struct Box {
    mode: Mode,
}

pub fn make_box() Box {
    { mode: Mode::A }
}
"#,
    )
    .expect("write module source");

    let checked = nia_driver::check_program(main.to_string_lossy().into_owned());
    assert!(checked.diagnostics.is_empty(), "{:?}", checked.diagnostics);

    let output = emit_llvm_ir(&checked.backend_lowering.program);
    assert!(output.diagnostics.is_empty(), "{:?}", output.diagnostics);
    let main_ir = output
        .modules
        .iter()
        .find(|module| module.name.ends_with("main.nia"))
        .expect("main module IR");
    assert!(main_ir.ir.contains("switch i8"), "{}", main_ir.ir);
    assert!(main_ir.ir.contains("ret i32 1"), "{}", main_ir.ir);
    assert!(main_ir.ir.contains("ret i32 2"), "{}", main_ir.ir);
    assert!(main_ir.ir.contains("ret i32 3"), "{}", main_ir.ir);
}

#[test]
fn emits_exhaustive_local_enum_switch_with_only_returning_arms() {
    let root = temp_dir("emits_exhaustive_local_enum_switch_with_only_returning_arms");
    let main = root.join("main.nia");
    std::fs::write(
        &main,
        r#"
enum Mode: u8 {
    A,
    B,
}

fn mode() Mode {
    Mode::A
}

fn main() i32 {
    switch mode() {
        Mode::A => return 10,
        Mode::B => return 20,
    }
}
"#,
    )
    .expect("write main source");

    let checked = nia_driver::check_program(main.to_string_lossy().into_owned());
    assert!(checked.diagnostics.is_empty(), "{:?}", checked.diagnostics);

    let output = emit_llvm_ir(&checked.backend_lowering.program);
    assert!(output.diagnostics.is_empty(), "{:?}", output.diagnostics);
    let ir = &output.modules[0].ir;
    assert!(ir.contains("switch i8"), "{}", ir);
    assert!(ir.contains("ret i32 10"), "{}", ir);
    assert!(ir.contains("ret i32 20"), "{}", ir);
}

#[test]
fn emits_imported_enum_variant_widening_cast() {
    let root = temp_dir("emits_imported_enum_variant_widening_cast");
    let main = root.join("main.nia");
    std::fs::write(
        &main,
        r#"
module defs;
using root::defs;

fn main() i32 {
    defs::Mode::B as i32
}
"#,
    )
    .expect("write main source");
    std::fs::write(
        root.join("defs.nia"),
        r#"
pub enum Mode: u8 {
    A,
    B,
}
"#,
    )
    .expect("write defs source");

    let checked = nia_driver::check_program(main.to_string_lossy().into_owned());
    assert!(checked.diagnostics.is_empty(), "{:?}", checked.diagnostics);

    let output = emit_llvm_ir(&checked.backend_lowering.program);
    assert!(output.diagnostics.is_empty(), "{:?}", output.diagnostics);
    let main_ir = output
        .modules
        .iter()
        .find(|module| module.name.ends_with("main.nia"))
        .expect("main module IR");
    assert!(main_ir.ir.contains("ret i32 1"), "{}", main_ir.ir);
}

#[test]
fn emits_using_imported_type_associated_function_call() {
    let root = temp_dir("emits_using_imported_type_associated_function_call");
    let main = root.join("main.nia");
    std::fs::write(
        &main,
        r#"
module module_assoc_defs;
using root::module_assoc_defs;

using module_assoc_defs::Box;

fn main() i32 {
    var box = Box::make(42);
    box.value
}
"#,
    )
    .expect("write main source");
    std::fs::write(
        root.join("module_assoc_defs.nia"),
        r#"
pub struct Box {
    value: i32,
}

extend Box {
    pub fn make(value: i32) Box {
        { value: value }
    }
}
"#,
    )
    .expect("write module source");

    let checked = nia_driver::check_program(main.to_string_lossy().into_owned());
    assert!(checked.diagnostics.is_empty(), "{:?}", checked.diagnostics);

    let output = emit_llvm_ir(&checked.backend_lowering.program);
    assert!(output.diagnostics.is_empty(), "{:?}", output.diagnostics);
    let main_ir = output
        .modules
        .iter()
        .find(|module| module.name.ends_with("main.nia"))
        .expect("main module IR");
    assert!(
        main_ir.ir.contains("call void @nia__m1__"),
        "{}",
        main_ir.ir
    );
}

#[test]
fn emits_size_builtin_when_module_has_import() {
    let root = temp_dir("emits_size_builtin_when_module_has_import");
    let main = root.join("main.nia");
    std::fs::write(
        &main,
        r#"
module empty;
using root::empty;

struct S {
    x: i32,
}

fn main() i32 {
    @size[S]() as i32
}
"#,
    )
    .expect("write main source");
    std::fs::write(
        root.join("empty.nia"),
        r#"
pub fn value() i32 {
    0
}
"#,
    )
    .expect("write empty source");

    let checked = nia_driver::check_program(main.to_string_lossy().into_owned());
    assert!(checked.diagnostics.is_empty(), "{:?}", checked.diagnostics);

    let output = emit_llvm_ir(&checked.backend_lowering.program);
    assert!(output.diagnostics.is_empty(), "{:?}", output.diagnostics);
    let main_ir = output
        .modules
        .iter()
        .find(|module| module.name.ends_with("main.nia"))
        .expect("main module IR");
    assert!(main_ir.ir.contains("ret i32 4"), "{}", main_ir.ir);
}

#[test]
fn emits_imported_array_length_size_builtin() {
    let root = temp_dir("emits_imported_array_length_size_builtin");
    let main = root.join("main.nia");
    std::fs::write(
        &main,
        r#"
module defs;
using root::defs;

fn main() i32 {
    var b = defs::make_box();
    b.bytes.len() as i32
}
"#,
    )
    .expect("write main source");
    std::fs::write(
        root.join("defs.nia"),
        r#"
pub struct Header {
    a: i32,
    b: i32,
}

pub struct Box {
    bytes: [@size[Header]()]u8,
}

pub fn make_box() Box {
    { bytes: [0u8, 0u8, 0u8, 0u8, 0u8, 0u8, 0u8, 0u8] }
}
"#,
    )
    .expect("write defs source");

    let checked = nia_driver::check_program(main.to_string_lossy().into_owned());
    assert!(checked.diagnostics.is_empty(), "{:?}", checked.diagnostics);

    let output = emit_llvm_ir(&checked.backend_lowering.program);
    assert!(output.diagnostics.is_empty(), "{:?}", output.diagnostics);
    let main_ir = output
        .modules
        .iter()
        .find(|module| module.name.ends_with("main.nia"))
        .expect("main module IR");
    assert!(main_ir.ir.contains("ret i32 8"), "{}", main_ir.ir);
}
