// SPDX-License-Identifier: GPL-3.0-or-later
use super::common::*;

#[test]
fn emits_receiver_method_calls() {
    let root = temp_dir("emits_receiver_method_calls");
    let main = root.join("main.nia");
    std::fs::write(
        &main,
        r#"
struct Cell {
    value: i32,
}

extend Cell {
    fn get(&const self) i32 {
        self.value
    }

    fn set(&self, value: i32) {
        self.value = value;
    }
}

fn main() i32 {
    var cell: Cell = { value: 1 };
    cell.set(42);
    cell.get()
}
"#,
    )
    .expect("write test source");

    let checked = nia_driver::check_program(main.to_string_lossy().into_owned());
    assert!(checked.diagnostics.is_empty(), "{:?}", checked.diagnostics);

    let output = emit_llvm_ir(&checked.backend_lowering.program);
    assert!(output.diagnostics.is_empty(), "{:?}", output.diagnostics);
    let ir = &output.modules[0].ir;
    assert!(ir.contains("define void @"));
    assert!(ir.contains("call void @"));
    assert!(ir.contains("call i32 @"));
    assert!(ir.contains("i32 42"));
    assert!(ir.contains("ret i32"));
}

#[test]
fn emits_builtin_place_trait_overload_calls() {
    let root = temp_dir("emits_builtin_place_trait_overload_calls");
    let main = root.join("main.nia");
    std::fs::write(
        &main,
        r#"
struct Cell {
    value: i32,
}

extend Cell : DerefConst {
    type Target = i32;

    fn deref_const(&const self) &const i32 {
        &const self.value
    }
}

extend Cell : Deref {
    type Target = i32;

    fn deref(&self) &i32 {
        &self.value
    }
}

extend Cell : IndexConst[usize] {
    type Output = i32;

    fn index_const(&const self, index: usize) &const i32 {
        &const self.value
    }
}

extend Cell : Index[usize] {
    type Output = i32;

    fn index(&self, index: usize) &i32 {
        &self.value
    }
}

fn main() i32 {
    var cell: Cell = { value: 1 };
    var first = cell.*;
    cell.* = 3;
    var second = cell[0];
    cell[0] = 5;
    first + second + cell.value
}
"#,
    )
    .expect("write test source");

    let checked = nia_driver::check_program(main.to_string_lossy().into_owned());
    assert!(checked.diagnostics.is_empty(), "{:?}", checked.diagnostics);

    let output = emit_llvm_ir(&checked.backend_lowering.program);
    assert!(output.diagnostics.is_empty(), "{:?}", output.diagnostics);
    let ir = &output.modules[0].ir;
    assert!(ir.contains("__deref_const"), "{ir}");
    assert!(ir.contains("__deref"), "{ir}");
    assert!(ir.contains("__index_const"), "{ir}");
    assert!(ir.contains("__index"), "{ir}");
    assert!(ir.contains("ret i32"), "{ir}");
}

#[test]
fn emits_trait_object_vtable_and_dynamic_dispatch() {
    let root = temp_dir("emits_trait_object_vtable_and_dynamic_dispatch");
    let main = root.join("main.nia");
    std::fs::write(
        &main,
        r#"
trait Source {
    fn add(&const self, rhs: i32) i32;
}

struct Counter {
    value: i32,
}

extend Counter : Source {
    fn add(&const self, rhs: i32) i32 {
        self.value + rhs
    }
}

fn read(source: &const Source) i32 {
    source.add(4)
}

fn main() i32 {
    var counter: Counter = { value: 8 };
    read(&const counter)
}
"#,
    )
    .expect("write test source");

    let checked = nia_driver::check_program(main.to_string_lossy().into_owned());
    assert!(checked.diagnostics.is_empty(), "{:?}", checked.diagnostics);

    let output = emit_llvm_ir(&checked.backend_lowering.program);
    assert!(output.diagnostics.is_empty(), "{:?}", output.diagnostics);
    let ir = &output.modules[0].ir;
    assert!(ir.contains("nia__vtable__"), "{ir}");
    assert!(ir.contains("vtable.fn"), "{ir}");
    assert!(ir.contains("call i32 %vtable.fn"), "{ir}");
}

#[test]
fn size_levels_deduplicate_repeated_trait_object_vtables() {
    let root = temp_dir("size_levels_deduplicate_repeated_trait_object_vtables");
    let main = root.join("main.nia");
    std::fs::write(
        &main,
        r#"
trait Source {
    fn add(&const self, rhs: i32) i32;
}

struct Counter {
    value: i32,
}

extend Counter : Source {
    fn add(&const self, rhs: i32) i32 {
        self.value + rhs
    }
}

fn read(source: &const Source) i32 {
    source.add(4)
}

fn left() i32 {
    var counter: Counter = { value: 8 };
    read(&const counter)
}

fn right() i32 {
    var counter: Counter = { value: 9 };
    read(&const counter)
}

fn main() i32 {
    left() + right()
}
"#,
    )
    .expect("write test source");

    for level in [
        nia_driver::NiaOptimizationLevel::Os,
        nia_driver::NiaOptimizationLevel::Oz,
    ] {
        let checked =
            nia_driver::check_program_with_options(main.to_string_lossy().into_owned(), level);
        assert!(checked.diagnostics.is_empty(), "{:?}", checked.diagnostics);

        let module = &checked.backend_lowering.program.modules[0];
        assert_eq!(module.trait_object_vtables.len(), 1, "{level:?}");
        assert_eq!(module.trait_object_vtables[0].entries.len(), 1, "{level:?}");

        let output = emit_llvm_ir(&checked.backend_lowering.program);
        assert!(output.diagnostics.is_empty(), "{:?}", output.diagnostics);
        let ir = &output.modules[0].ir;
        let vtable_defs = ir
            .lines()
            .filter(|line| line.starts_with("@nia__vtable__"))
            .count();
        assert_eq!(vtable_defs, 1, "{level:?}\n{ir}");
    }
}

#[test]
fn emits_trait_object_vtable_from_defer_tail_expr() {
    let root = temp_dir("emits_trait_object_vtable_from_defer_tail_expr");
    let main = root.join("main.nia");
    std::fs::write(
        &main,
        r#"
extern fn log(value: i32);

trait Source {
    fn add(&const self, rhs: i32) i32;
}

struct Counter {
    value: i32,
}

extend Counter : Source {
    fn add(&const self, rhs: i32) i32 {
        self.value + rhs
    }
}

fn read(source: &const Source) i32 {
    source.add(4)
}

fn main() i32 {
    var counter: Counter = { value: 8 };
    defer {
        log(read(&const counter))
    };
    0
}
"#,
    )
    .expect("write test source");

    let checked = nia_driver::check_program(main.to_string_lossy().into_owned());
    assert!(checked.diagnostics.is_empty(), "{:?}", checked.diagnostics);

    let output = emit_llvm_ir(&checked.backend_lowering.program);
    assert!(output.diagnostics.is_empty(), "{:?}", output.diagnostics);
    let ir = &output.modules[0].ir;
    assert!(ir.contains("nia__vtable__"), "{ir}");
    assert!(ir.contains("vtable.fn"), "{ir}");
    assert!(ir.contains("call i32 %vtable.fn"), "{ir}");
    assert!(ir.contains("call void @log"), "{ir}");
}

#[test]
fn emits_trait_object_supertrait_upcast_metadata_offset() {
    let root = temp_dir("emits_trait_object_supertrait_upcast_metadata_offset");
    let main = root.join("main.nia");
    std::fs::write(
        &main,
        r#"
trait Parent {
    fn parent(&const self) i32;
}

trait Child : Parent {
    fn child(&const self) i32;
}

struct Counter {
    value: i32,
}

extend Counter : Parent {
    fn parent(&const self) i32 {
        self.value
    }
}

extend Counter : Child {
    fn child(&const self) i32 {
        self.value + 1
    }
}

fn as_parent(child: &const Child) &const Parent {
    child
}

fn main() i32 {
    var counter: Counter = { value: 8 };
    var child: &const Child = &const counter;
    as_parent(child).parent()
}
"#,
    )
    .expect("write test source");

    let checked = nia_driver::check_program(main.to_string_lossy().into_owned());
    assert!(checked.diagnostics.is_empty(), "{:?}", checked.diagnostics);
    let output = emit_llvm_ir(&checked.backend_lowering.program);
    assert!(output.diagnostics.is_empty(), "{:?}", output.diagnostics);
    let ir = &output.modules[0].ir;
    assert!(ir.contains("traitobj.upcast.metadata.offset"), "{ir}");
    assert!(ir.contains("vtable.fn"), "{ir}");
}

#[test]
fn emits_trait_bound_generic_method_calls() {
    let root = temp_dir("emits_trait_bound_generic_method_calls");
    let main = root.join("main.nia");
    std::fs::write(
        &main,
        r#"
trait Same {
    fn eq(&const self, other: &const Self) bool;
}

struct Point {
    x: i32,
}

extend Point : Same {
    fn eq(&const self, other: &const Point) bool {
        self.x == other.x
    }
}

fn same[T](a: &const T, b: &const T) bool
where T: Same {
    a.eq(b)
}

fn main() bool {
    var a: Point = { x: 1 };
    var b: Point = { x: 1 };
    same[Point](&const a, &const b)
}
"#,
    )
    .expect("write test source");

    let checked = nia_driver::check_program(main.to_string_lossy().into_owned());
    assert!(checked.diagnostics.is_empty(), "{:?}", checked.diagnostics);

    let output = emit_llvm_ir(&checked.backend_lowering.program);
    assert!(output.diagnostics.is_empty(), "{:?}", output.diagnostics);
    let ir = &output.modules[0].ir;
    assert!(ir.contains("call i1 @"));
    assert!(ir.contains("ret i1"));
}

#[test]
fn emits_supertrait_method_calls_from_subtrait_bounds() {
    let root = temp_dir("emits_supertrait_method_calls_from_subtrait_bounds");
    let main = root.join("main.nia");
    std::fs::write(
        &main,
        r#"
trait Same {
    fn eq(&const self, other: &const Self) bool;
}

trait Ranked : Same {
    fn lt(&const self, other: &const Self) bool;
}

struct Point {
    x: i32,
}

extend Point : Same {
    fn eq(&const self, other: &const Point) bool {
        self.x == other.x
    }
}

extend Point : Ranked {
    fn lt(&const self, other: &const Point) bool {
        self.x < other.x
    }
}

fn same_ord[T](a: &const T, b: &const T) bool
where T: Ranked {
    a.eq(b)
}

fn main() bool {
    var a: Point = { x: 1 };
    var b: Point = { x: 1 };
    same_ord[Point](&const a, &const b)
}
"#,
    )
    .expect("write test source");

    let checked = nia_driver::check_program(main.to_string_lossy().into_owned());
    assert!(checked.diagnostics.is_empty(), "{:?}", checked.diagnostics);

    let output = emit_llvm_ir(&checked.backend_lowering.program);
    assert!(output.diagnostics.is_empty(), "{:?}", output.diagnostics);
    let ir = &output.modules[0].ir;
    assert!(ir.contains("call i1 @"), "{ir}");
    assert!(ir.contains("ret i1"), "{ir}");
}

#[test]
fn emits_associated_type_projection_instances() {
    let root = temp_dir("emits_associated_type_projection_instances");
    let main = root.join("main.nia");
    std::fs::write(
        &main,
        r#"
trait Source {
    type Item;

    fn get(&const self) [Self as Source]::Item;
}

struct Counter {
    value: i32,
}

extend Counter : Source {
    type Item = i32;

    fn get(&const self) i32 {
        self.value
    }
}

fn read[T](value: &const T) [T as Source]::Item
where T: Source {
    value.get()
}

fn main() i32 {
    var counter: Counter = { value: 42 };
    read[Counter](&const counter)
}
"#,
    )
    .expect("write test source");

    let checked = nia_driver::check_program(main.to_string_lossy().into_owned());
    assert!(checked.diagnostics.is_empty(), "{:?}", checked.diagnostics);

    let output = emit_llvm_ir(&checked.backend_lowering.program);
    assert!(output.diagnostics.is_empty(), "{:?}", output.diagnostics);
    let ir = &output.modules[0].ir;
    assert!(ir.contains("call i32 @"), "{ir}");
    assert!(ir.contains("ret i32"), "{ir}");
}

#[test]
fn emits_generic_associated_type_default_method_instances() {
    let root = temp_dir("emits_generic_associated_type_default_method_instances");
    let main = root.join("main.nia");
    std::fs::write(
        &main,
        r#"
trait Mapper[A, B] {
    type C;
    type D;

    fn map_c(&const self, a: A, b: B) [Self as Mapper[A, B]]::C;

    fn map_d(&const self, a: A, b: B, fallback: [Self as Mapper[A, B]]::D) [Self as Mapper[A, B]]::D {
        _ = self.map_c(a, b);
        fallback
    }
}

struct Pairer {
    seed: i32,
}

extend Pairer : Mapper[i32, i32] {
    type C = i32;
    type D = i32;

    fn map_c(&const self, a: i32, b: i32) i32 {
        self.seed + a + b
    }
}

fn mapped[T](value: &const T, fallback: [T as Mapper[i32, i32]]::D) [T as Mapper[i32, i32]]::D
where T: Mapper[i32, i32] {
    value.map_d(1, 2, fallback)
}

fn main() i32 {
    var p: Pairer = { seed: 3 };
    mapped[Pairer](&const p, 9)
}
"#,
    )
    .expect("write test source");

    let checked = nia_driver::check_program(main.to_string_lossy().into_owned());
    assert!(checked.diagnostics.is_empty(), "{:?}", checked.diagnostics);

    let output = emit_llvm_ir(&checked.backend_lowering.program);
    assert!(output.diagnostics.is_empty(), "{:?}", output.diagnostics);
    let ir = &output.modules[0].ir;
    assert!(ir.contains("call i32 @"), "{ir}");
    assert!(ir.contains("ret i32"), "{ir}");
}

#[test]
fn emits_trait_default_method_instances() {
    let root = temp_dir("emits_trait_default_method_instances");
    let main = root.join("main.nia");
    std::fs::write(
        &main,
        r#"
trait Same {
    fn eq(&const self, other: &const Self) bool;

    fn ne(&const self, other: &const Self) bool {
        !self.eq(other)
    }
}

struct Point {
    x: i32,
}

extend Point : Same {
    fn eq(&const self, other: &const Point) bool {
        self.x == other.x
    }
}

fn different[T](a: &const T, b: &const T) bool
where T: Same {
    a.ne(b)
}

fn main() bool {
    var a: Point = { x: 1 };
    var b: Point = { x: 2 };
    different[Point](&const a, &const b)
}
"#,
    )
    .expect("write test source");

    let checked = nia_driver::check_program(main.to_string_lossy().into_owned());
    assert!(checked.diagnostics.is_empty(), "{:?}", checked.diagnostics);

    let output = emit_llvm_ir(&checked.backend_lowering.program);
    assert!(output.diagnostics.is_empty(), "{:?}", output.diagnostics);
    let ir = &output.modules[0].ir;
    assert!(ir.contains("call i1 @"));
    assert!(ir.contains("xor i1"));
}
