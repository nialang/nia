// SPDX-License-Identifier: GPL-3.0-or-later
use super::common::*;

#[test]
fn emits_cross_module_function_pointer_type_alias_fields() {
    let root = temp_dir("emits_cross_module_function_pointer_type_alias_fields");
    let main = root.join("main.nia");
    std::fs::write(
        &main,
        r#"
module build;
module types;
using entry::build;
using entry::types;

fn inc(value: i32) i32 {
    value + 1
}

fn main() i32 {
    let mut build = build::Build::init();
    build.step(&"run", &inc);
    let step = types::Step::init(&"run", &inc);
    step.run(41) + build.run(0)
}
"#,
    )
    .expect("write main source");
    std::fs::write(
        root.join("types.nia"),
        r#"
pub type StepFn = &fn(i32) i32;

pub struct Step {
    name: &[char],
    run: StepFn,
}

extend Step {
    pub fn init(name: &[char], run: StepFn) Step {
        { name: name, run: run }
    }
}
"#,
    )
    .expect("write types source");
    std::fs::write(
        root.join("build.nia"),
        r#"
using entry::types;

pub struct Build {
    registered: types::Step,
}

extend Build {
    pub fn init() Build {
        {
            registered: types::Step::init(&"noop", &noop),
        }
    }

    pub fn step(&mut self, name: &[char], run: types::StepFn) void {
        self.registered = types::Step::init(name, run);
    }

    pub fn run(&self, value: i32) i32 {
        (self.registered.run)(value)
    }
}

fn noop(value: i32) i32 {
    value
}
"#,
    )
    .expect("write build source");

    let codegen = codegen_program(main.to_string_lossy().into_owned());
    assert!(codegen.diagnostics.is_empty(), "{:?}", codegen.diagnostics);

    let output = emit_llvm_ir(&codegen.backend_lowering.program);
    assert!(output.diagnostics.is_empty(), "{:?}", output.diagnostics);
    let ir = output
        .modules
        .iter()
        .map(|module| module.ir.as_str())
        .collect::<Vec<_>>()
        .join("\n");
    assert!(ir.contains("call i32 %"), "{ir}");
}

#[test]
fn emits_imported_open_enum_as_error_union_payload() {
    let root = temp_dir("emits_imported_open_enum_as_error_union_payload");
    let main = root.join("main.nia");
    std::fs::write(
        &main,
        r#"
module errors;
using entry::errors;

fn maybe(flag: bool) errors::Error!i32 {
    if flag {
        !40
    } else {
        errors::Error::Io!
    }
}

fn add_two(flag: bool) errors::Error!i32 {
    let mut value = maybe(flag).?;
    !(value + 2)
}

fn main() i32 {
    if !value = add_two(true) {
        value
    } or error! {
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

    let codegen = codegen_program(main.to_string_lossy().into_owned());
    assert!(codegen.diagnostics.is_empty(), "{:?}", codegen.diagnostics);

    let output = emit_llvm_ir(&codegen.backend_lowering.program);
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
using entry::defs;
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
    let mut held = make();
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

    let codegen = codegen_program(main.to_string_lossy().into_owned());
    assert!(codegen.diagnostics.is_empty(), "{:?}", codegen.diagnostics);

    let output = emit_llvm_ir(&codegen.backend_lowering.program);
    assert!(output.diagnostics.is_empty(), "{:?}", output.diagnostics);
    let main_ir = output
        .modules
        .iter()
        .find(|module| module.name.ends_with("main.nia"))
        .expect("main module IR");
    assert!(main_ir.ir.contains(&backend_symbol_suffix("HoldsItem")));
    assert!(main_ir.ir.contains(&backend_symbol_suffix("Item")));
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
using entry::empty;

struct S {
    x: i32,
}

struct T {
    xs: [256]S,
}

fn main() i32 {
    let mut t: T = { xs: [{ x: 0 }; 256] };
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

    let codegen = codegen_program(main.to_string_lossy().into_owned());
    assert!(codegen.diagnostics.is_empty(), "{:?}", codegen.diagnostics);

    let output = emit_llvm_ir(&codegen.backend_lowering.program);
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
using entry::defs;

fn main() i32 {
    let mut bag = defs::make_bag();
    let mut i: usize = 2;
    bag.items[i] = defs::make_item(5);
    let mut tail = & bag.items[1..=2];
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

    let codegen = codegen_program(main.to_string_lossy().into_owned());
    assert!(codegen.diagnostics.is_empty(), "{:?}", codegen.diagnostics);

    let output = emit_llvm_ir(&codegen.backend_lowering.program);
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
using entry::defs;

fn main() i32 {
    let callback: &fn(defs::Pair) defs::Pair = & defs::id_pair;
    let mut pair = callback(defs::make_pair(2, 5));
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

    let codegen = codegen_program(main.to_string_lossy().into_owned());
    assert!(codegen.diagnostics.is_empty(), "{:?}", codegen.diagnostics);

    let output = emit_llvm_ir(&codegen.backend_lowering.program);
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
using entry::module_enum_defs;

using module_enum_defs::Mode;

fn main() i32 {
    let mut box = module_enum_defs::make_box();
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

    let codegen = codegen_program(main.to_string_lossy().into_owned());
    assert!(codegen.diagnostics.is_empty(), "{:?}", codegen.diagnostics);

    let output = emit_llvm_ir(&codegen.backend_lowering.program);
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
using entry::module_enum_defs;

fn main() i32 {
    let mut box = module_enum_defs::make_box();
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

    let codegen = codegen_program(main.to_string_lossy().into_owned());
    assert!(codegen.diagnostics.is_empty(), "{:?}", codegen.diagnostics);

    let output = emit_llvm_ir(&codegen.backend_lowering.program);
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

    let codegen = codegen_program(main.to_string_lossy().into_owned());
    assert!(codegen.diagnostics.is_empty(), "{:?}", codegen.diagnostics);

    let output = emit_llvm_ir(&codegen.backend_lowering.program);
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
using entry::defs;

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

    let codegen = codegen_program(main.to_string_lossy().into_owned());
    assert!(codegen.diagnostics.is_empty(), "{:?}", codegen.diagnostics);

    let output = emit_llvm_ir(&codegen.backend_lowering.program);
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
using entry::module_assoc_defs;

using module_assoc_defs::Box;

fn main() i32 {
    let mut box = Box::make(42);
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

    let codegen = codegen_program(main.to_string_lossy().into_owned());
    assert!(codegen.diagnostics.is_empty(), "{:?}", codegen.diagnostics);

    let output = emit_llvm_ir(&codegen.backend_lowering.program);
    assert!(output.diagnostics.is_empty(), "{:?}", output.diagnostics);
    let main_ir = output
        .modules
        .iter()
        .find(|module| module.name.ends_with("main.nia"))
        .expect("main module IR");
    let make = mangled_symbol_any_module(&main_ir.ir, '@', "make");
    assert!(
        main_ir.ir.contains(&format!("call void {make}")),
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
using entry::empty;

struct S {
    x: i32,
}

fn main() i32 {
    std::builtin::size[S]() as i32
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

    let codegen = codegen_program(main.to_string_lossy().into_owned());
    assert!(codegen.diagnostics.is_empty(), "{:?}", codegen.diagnostics);

    let output = emit_llvm_ir(&codegen.backend_lowering.program);
    assert!(output.diagnostics.is_empty(), "{:?}", output.diagnostics);
    let main_ir = output
        .modules
        .iter()
        .find(|module| module.name.ends_with("main.nia"))
        .expect("main module IR");
    assert!(main_ir.ir.contains("ret i32 4"), "{}", main_ir.ir);
}

#[test]
fn emits_field_offset_builtin() {
    let root = temp_dir("emits_field_offset_builtin");
    let main = root.join("main.nia");
    std::fs::write(
        &main,
        r#"
extern struct Pair {
    a: u8,
    b: u32,
}

fn main() usize {
    std::builtin::offset[Pair]("b")
}
"#,
    )
    .expect("write main source");

    let codegen = codegen_program(main.to_string_lossy().into_owned());
    assert!(codegen.diagnostics.is_empty(), "{:?}", codegen.diagnostics);

    let output = emit_llvm_ir(&codegen.backend_lowering.program);
    assert!(output.diagnostics.is_empty(), "{:?}", output.diagnostics);
    let main_ir = output
        .modules
        .iter()
        .find(|module| module.name.ends_with("main.nia"))
        .expect("main module IR");
    assert!(main_ir.ir.contains("ret i64 4"), "{}", main_ir.ir);
}

#[test]
fn emits_imported_array_length_size_builtin() {
    let root = temp_dir("emits_imported_array_length_size_builtin");
    let main = root.join("main.nia");
    std::fs::write(
        &main,
        r#"
module defs;
using entry::defs;

fn main() i32 {
    let mut b = defs::make_box();
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
    bytes: [std::builtin::size[Header]()]u8,
}

pub fn make_box() Box {
    { bytes: [0u8, 0u8, 0u8, 0u8, 0u8, 0u8, 0u8, 0u8] }
}
"#,
    )
    .expect("write defs source");

    let codegen = codegen_program(main.to_string_lossy().into_owned());
    assert!(codegen.diagnostics.is_empty(), "{:?}", codegen.diagnostics);

    let output = emit_llvm_ir(&codegen.backend_lowering.program);
    assert!(output.diagnostics.is_empty(), "{:?}", output.diagnostics);
    let main_ir = output
        .modules
        .iter()
        .find(|module| module.name.ends_with("main.nia"))
        .expect("main module IR");
    assert!(main_ir.ir.contains("ret i32 8"), "{}", main_ir.ir);
}
