// SPDX-License-Identifier: GPL-3.0-or-later
use super::common::*;

#[test]
fn emits_generic_struct_associated_function_instances() {
    let root = temp_dir("emits_generic_struct_associated_function_instances");
    let main = root.join("main.nia");
    std::fs::write(
        &main,
        r#"
struct Box[T] {
    value: T,
}

extend[T] Box[T] {
    fn make(value: T) Box[T] {
        { value: value }
    }
}

fn main() i32 {
    let mut b = Box[i32]::make(42);
    b.value
}
"#,
    )
    .expect("write test source");

    let codegen = codegen_program(main.to_string_lossy().into_owned());
    assert!(codegen.diagnostics.is_empty(), "{:?}", codegen.diagnostics);

    let output = emit_llvm_ir(&codegen.backend_lowering, &codegen.type_store);
    assert!(output.diagnostics.is_empty(), "{:?}", output.diagnostics);
    let ir = &output.modules[0].ir;
    let box_ty = mangled_symbol(ir, '%', 0, "Box__inst__t_i32");
    let make = mangled_symbol(ir, '@', 0, "make__inst__t_i32");
    assert!(ir.contains(&box_ty), "{ir}");
    assert!(ir.contains(&make), "{ir}");
    assert!(
        ir.contains(&format!("call void {make}(ptr %b, i32 42")),
        "{ir}"
    );
    assert!(
        ir.contains(&format!("define void {make}(ptr %0, i32 %1)")),
        "{ir}"
    );
    assert!(ir.contains("ret i32"));
}

#[test]
fn emits_structural_extension_method_calls() {
    let root = temp_dir("emits_structural_extension_method_calls");
    let main = root.join("main.nia");
    std::fs::write(
        &main,
        r#"
type RawPtr[T] = &T;

extend i32 {
    fn is_zero(self) bool {
        self == 0
    }
}

extend[T] RawPtr[T] {
    fn is_null(self) bool {
        self as usize == 0
    }
}

extend[T] & [T] {
    fn size(self) usize {
        self.len()
    }
}

extend[T] [3]T {
    fn first(self) T {
        self[0]
    }
}

extend &fn(i32) i32 {
    fn apply(self, value: i32) i32 {
        self(value)
    }
}

fn inc(value: i32) i32 {
    value + 1
}

fn main(ptr: &i32, xs: & [i32], triple: [3]i32) i32 {
    if 0.is_zero() {}
    if ptr.is_null() {}
    xs.size() as i32 + triple.first() + (& inc).apply(1)
}
"#,
    )
    .expect("write test source");

    let codegen = codegen_program(main.to_string_lossy().into_owned());
    assert!(codegen.diagnostics.is_empty(), "{:?}", codegen.diagnostics);

    let output = emit_llvm_ir(&codegen.backend_lowering, &codegen.type_store);
    assert!(output.diagnostics.is_empty(), "{:?}", output.diagnostics);
    let ir = &output.modules[0].ir;
    let is_zero = mangled_symbol(ir, '@', 0, "is_zero");
    let is_null = mangled_symbol(ir, '@', 0, "is_null__inst__t_i32");
    let size = mangled_symbol(ir, '@', 0, "size__inst__t_i32");
    let first = mangled_symbol(ir, '@', 0, "first__inst__t_i32");
    let apply = mangled_symbol(ir, '@', 0, "apply");
    assert!(ir.contains(&format!("call i1 {is_zero}")), "{ir}");
    assert!(ir.contains(&format!("call i1 {is_null}")), "{ir}");
    assert!(ir.contains(&format!("call i64 {size}")), "{ir}");
    assert!(ir.contains(&format!("call i32 {first}")), "{ir}");
    assert!(ir.contains(&format!("call i32 {apply}")), "{ir}");
}

#[test]
fn emits_imported_generic_structural_extension_method_calls() {
    let root = temp_dir("emits_imported_generic_structural_extension_method_calls");
    let main = root.join("main.nia");
    std::fs::write(
        &main,
        r#"
module share;
module ptr;
using entry::share;

extern fn read_readonly() &u8;

fn main(mut_ptr: &mut u8) i32 {
    let mut readonly_ptr = read_readonly();
    if mut_ptr.is_null() {
        return 1;
    }
    if readonly_ptr.is_null() {
        return 2;
    }
    0
}
"#,
    )
    .expect("write main source");
    std::fs::write(root.join("share.nia"), "using entry::ptr;").expect("write share source");
    std::fs::write(
        root.join("ptr.nia"),
        r#"
extend[T] &mut T {
    pub fn is_null(self) bool {
        self as usize == 0
    }
}

extend[T] &T {
    pub fn is_null(self) bool {
        self as usize == 0
    }
}
"#,
    )
    .expect("write ptr source");

    let codegen = codegen_program(main.to_string_lossy().into_owned());
    assert!(codegen.diagnostics.is_empty(), "{:?}", codegen.diagnostics);

    let output = emit_llvm_ir(&codegen.backend_lowering, &codegen.type_store);
    assert!(output.diagnostics.is_empty(), "{:?}", output.diagnostics);
    let ir = output
        .modules
        .iter()
        .map(|module| module.ir.as_str())
        .collect::<Vec<_>>()
        .join("\n");
    assert!(
        ir.contains(&backend_symbol_suffix("is_null__inst__t_u8")),
        "{ir}"
    );
    assert!(ir.contains("call i1 @"), "{ir}");
    assert!(ir.contains("define i1 @"), "{ir}");
}

#[test]
fn emits_specialized_associated_extension_function_calls() {
    let root = temp_dir("emits_specialized_associated_extension_function_calls");
    let main = root.join("main.nia");
    std::fs::write(
        &main,
        r#"
struct Box[T] {
    value: T,
}

extend[T] Box[T] {
    fn make(value: T) Box[T] {
        { value: value }
    }
}

extend Box[i32] {
    fn make(value: i32) Box[i32] {
        { value: value + 1 }
    }
}

fn main() i32 {
    let mut a = Box[i32]::make(41);
    let mut b = Box[bool]::make(true);
    if b.value { a.value } else { 0 }
}
"#,
    )
    .expect("write test source");

    let codegen = codegen_program(main.to_string_lossy().into_owned());
    assert!(codegen.diagnostics.is_empty(), "{:?}", codegen.diagnostics);

    let output = emit_llvm_ir(&codegen.backend_lowering, &codegen.type_store);
    assert!(output.diagnostics.is_empty(), "{:?}", output.diagnostics);
    let ir = &output.modules[0].ir;
    let specialized_make = mangled_symbol(ir, '@', 0, "make");
    let generic_make = mangled_symbol(ir, '@', 0, "make__inst__t_bool");
    assert!(
        ir.contains(&format!("call void {specialized_make}(ptr %a, i32 41")),
        "{ir}"
    );
    assert!(
        ir.contains(&format!("call void {generic_make}(ptr %b, i1 true")),
        "{ir}"
    );
}

#[test]
fn emits_associated_method_function_pointers() {
    let root = temp_dir("emits_associated_method_function_pointers");
    let main = root.join("main.nia");
    std::fs::write(
        &main,
        r#"
struct Point {
    x: i32,
}

extend Point {
    fn get(& self) i32 {
        self.x
    }
}

fn apply(p: & Point, f: &fn(& Point) i32) i32 {
    f(p)
}

fn main() i32 {
    let mut p: Point = { x: 42 };
    apply(& p, & Point::get)
}
"#,
    )
    .expect("write test source");

    let codegen = codegen_program(main.to_string_lossy().into_owned());
    assert!(codegen.diagnostics.is_empty(), "{:?}", codegen.diagnostics);

    let output = emit_llvm_ir(&codegen.backend_lowering, &codegen.type_store);
    assert!(output.diagnostics.is_empty(), "{:?}", output.diagnostics);
    let ir = &output.modules[0].ir;
    assert_contains_mangled_symbol(ir, '@', 0, "get");
    assert!(ir.contains("call i32 %"), "{ir}");
}

#[test]
fn emits_generic_associated_method_function_pointers() {
    let root = temp_dir("emits_generic_associated_method_function_pointers");
    let main = root.join("main.nia");
    std::fs::write(
        &main,
        r#"
struct Box[T] {
    value: T,
}

extend[T] Box[T] {
    fn get(& self) T {
        self.value
    }
}

fn apply(box: & Box[i32], f: &fn(& Box[i32]) i32) i32 {
    f(box)
}

fn main() i32 {
    let mut box: Box[i32] = { value: 42 };
    apply(& box, & Box[i32]::get)
}
"#,
    )
    .expect("write test source");

    let codegen = codegen_program(main.to_string_lossy().into_owned());
    assert!(codegen.diagnostics.is_empty(), "{:?}", codegen.diagnostics);

    let output = emit_llvm_ir(&codegen.backend_lowering, &codegen.type_store);
    assert!(output.diagnostics.is_empty(), "{:?}", output.diagnostics);
    let ir = &output.modules[0].ir;
    assert_contains_mangled_symbol(ir, '@', 0, "get__inst__t_i32");
    assert!(ir.contains("call i32 %"), "{ir}");
}

#[test]
fn emits_static_associated_method_function_pointer_initializers() {
    let root = temp_dir("emits_static_associated_method_function_pointer_initializers");
    let main = root.join("main.nia");
    std::fs::write(
        &main,
        r#"
struct Point {
    x: i32,
}

extend Point {
    fn get(& self) i32 {
        self.x
    }
}

static point_get: &fn(& Point) i32 = & Point::get;

fn main(p: & Point) i32 {
    point_get(p)
}
"#,
    )
    .expect("write test source");

    let codegen = codegen_program(main.to_string_lossy().into_owned());
    assert!(codegen.diagnostics.is_empty(), "{:?}", codegen.diagnostics);

    let output = emit_llvm_ir(&codegen.backend_lowering, &codegen.type_store);
    assert!(output.diagnostics.is_empty(), "{:?}", output.diagnostics);
    let ir = &output.modules[0].ir;
    let point_get = mangled_symbol(ir, '@', 0, "point_get");
    let get = mangled_symbol(ir, '@', 0, "get");
    assert!(
        ir.contains(&format!("{point_get} = constant ptr {get}")),
        "{ir}"
    );
}

#[test]
fn emits_function_pointer_type_alias_fields() {
    let root = temp_dir("emits_function_pointer_type_alias_fields");
    let main = root.join("main.nia");
    std::fs::write(
        &main,
        r#"
type StepFn = &fn(i32) i32;

struct Step {
    run: StepFn,
}

fn inc(value: i32) i32 {
    value + 1
}

fn main() i32 {
    let step = Step { run: &inc };
    step.run(41)
}
"#,
    )
    .expect("write test source");

    let codegen = codegen_program(main.to_string_lossy().into_owned());
    assert!(codegen.diagnostics.is_empty(), "{:?}", codegen.diagnostics);

    let output = emit_llvm_ir(&codegen.backend_lowering, &codegen.type_store);
    assert!(output.diagnostics.is_empty(), "{:?}", output.diagnostics);
    let ir = &output.modules[0].ir;
    assert!(ir.contains("call i32 %"), "{ir}");
}

#[test]
fn emits_structural_associated_calls_and_function_pointers() {
    let root = temp_dir("emits_structural_associated_calls_and_function_pointers");
    let main = root.join("main.nia");
    std::fs::write(
        &main,
        r#"
extend[T] &T {
    fn is_null(self) bool {
        self as usize == 0
    }

    fn zero() usize {
        0usize
    }
}

extend[T] [3]T {
    fn first(self) T {
        self[0]
    }
}

fn main(ptr: &u8, triple: [3]i32) i32 {
    let is_null: &fn(&u8) bool = & [&u8]::is_null;
    let zero: &fn() usize = & [&u8]::zero;
    if is_null(ptr) {}
    if [&u8]::is_null(ptr) {}
    [[3]i32]::first(triple) + zero() as i32
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
        ir.contains(&backend_symbol_suffix("is_null__inst__t_u8")),
        "{ir}"
    );
    assert!(
        ir.contains(&backend_symbol_suffix("zero__inst__t_u8")),
        "{ir}"
    );
    assert!(
        ir.contains(&backend_symbol_suffix("first__inst__t_i32")),
        "{ir}"
    );
    assert!(ir.contains("call i1 %"), "{ir}");
    assert!(ir.contains("call i1 @"), "{ir}");
    assert!(ir.contains("call i64 %"), "{ir}");
}

#[test]
fn emits_deep_pointer_structural_associated_calls_and_function_pointers() {
    let root = temp_dir("emits_deep_pointer_structural_associated_calls_and_function_pointers");
    let main = root.join("main.nia");
    std::fs::write(
        &main,
        r#"
extend &&&&&& &&i32 {
    fn is_null(self) bool {
        self as usize == 0
    }
}

fn main(ptr: &&&&&& &&i32) bool {
    let is_null: &fn(&&&&&& &&i32) bool = & [&&&&&& &&i32]::is_null;
    is_null(ptr) and [&&&&&& &&i32]::is_null(ptr)
}
"#,
    )
    .expect("write test source");

    let codegen = codegen_program(main.to_string_lossy().into_owned());
    assert!(codegen.diagnostics.is_empty(), "{:?}", codegen.diagnostics);

    let output = emit_llvm_ir(&codegen.backend_lowering, &codegen.type_store);
    assert!(output.diagnostics.is_empty(), "{:?}", output.diagnostics);
    let ir = &output.modules[0].ir;
    assert_contains_mangled_symbol(ir, '@', 0, "is_null");
    assert!(ir.contains("call i1 %"), "{ir}");
    assert!(ir.contains("call i1 @"), "{ir}");
}

#[test]
fn emits_numeric_literal_suffix_extension_method_calls() {
    let root = temp_dir("emits_numeric_literal_suffix_extension_method_calls");
    let main = root.join("main.nia");
    std::fs::write(
        &main,
        r#"
extend usize {
    fn plus_one(self) usize {
        self + 1usize
    }
}

fn main() usize {
    (10usize).plus_one()
}
"#,
    )
    .expect("write test source");

    let codegen = codegen_program(main.to_string_lossy().into_owned());
    assert!(codegen.diagnostics.is_empty(), "{:?}", codegen.diagnostics);

    let output = emit_llvm_ir(&codegen.backend_lowering, &codegen.type_store);
    assert!(output.diagnostics.is_empty(), "{:?}", output.diagnostics);
    let ir = &output.modules[0].ir;
    let plus_one = mangled_symbol(ir, '@', 0, "plus_one");
    assert!(ir.contains(&plus_one), "{ir}");
    assert!(ir.contains(&format!("call i64 {plus_one}(i64 10")), "{ir}");
}
