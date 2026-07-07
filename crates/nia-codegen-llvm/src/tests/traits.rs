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
    fn get(& self) i32 {
        self.value
    }

    fn set(&self, value: i32) {
        self.value = value;
    }
}

fn main() i32 {
    let mut cell: Cell = { value: 1 };
    cell.set(42);
    cell.get()
}
"#,
    )
    .expect("write test source");

    let codegen = codegen_program(main.to_string_lossy().into_owned());
    assert!(codegen.diagnostics.is_empty(), "{:?}", codegen.diagnostics);

    let output = emit_llvm_ir(&codegen.backend_lowering.program);
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

extend Cell : Deref {
    type Target = i32;

    fn deref(& self) & i32 {
        & self.value
    }
}

extend Cell : DerefMut {
    type Target = i32;

    fn deref_mut(&mut self) &mut i32 {
        &mut self.value
    }
}

extend Cell : Index[usize] {
    type Output = i32;

    fn index(& self, index: usize) & i32 {
        & self.value
    }
}

extend Cell : IndexMut[usize] {
    type Output = i32;

    fn index_mut(&mut self, index: usize) &mut i32 {
        &mut self.value
    }
}

fn main() i32 {
    let mut cell: Cell = { value: 1 };
    let mut first = cell.*;
    cell.* = 3;
    let mut second = cell[0];
    cell[0] = 5;
    first + second + cell.value
}
"#,
    )
    .expect("write test source");

    let codegen = codegen_program(main.to_string_lossy().into_owned());
    assert!(codegen.diagnostics.is_empty(), "{:?}", codegen.diagnostics);

    let output = emit_llvm_ir(&codegen.backend_lowering.program);
    assert!(output.diagnostics.is_empty(), "{:?}", output.diagnostics);
    let ir = &output.modules[0].ir;
    assert!(ir.contains("__deref"), "{ir}");
    assert!(ir.contains("__deref_mut"), "{ir}");
    assert!(ir.contains("__index"), "{ir}");
    assert!(ir.contains("__index_mut"), "{ir}");
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
    fn add(& self, rhs: i32) i32;
}

struct Counter {
    value: i32,
}

extend Counter : Source {
    fn add(& self, rhs: i32) i32 {
        self.value + rhs
    }
}

fn read(source: & Source) i32 {
    source.add(4)
}

fn main() i32 {
    let mut counter: Counter = { value: 8 };
    read(& counter)
}
"#,
    )
    .expect("write test source");

    let codegen = codegen_program(main.to_string_lossy().into_owned());
    assert!(codegen.diagnostics.is_empty(), "{:?}", codegen.diagnostics);

    let output = emit_llvm_ir(&codegen.backend_lowering.program);
    assert!(output.diagnostics.is_empty(), "{:?}", output.diagnostics);
    let ir = &output.modules[0].ir;
    assert!(ir.contains("nia__vtable__"), "{ir}");
    assert!(ir.contains("vtable.fn"), "{ir}");
    assert!(ir.contains("call i32 %vtable.fn"), "{ir}");
}

#[test]
fn emits_array_reference_element_trait_object_coercions() {
    let root = temp_dir("emits_array_reference_element_trait_object_coercions");
    let main = root.join("main.nia");
    std::fs::write(
        &main,
        r#"
trait Source {
    fn get(& self) i32;
}

extend i32 : Source {
    fn get(& self) i32 {
        self.*
    }
}

fn read_all(sources: & [ & Source]) i32 {
    sources[0].get()
}

fn main() i32 {
    read_all(&[&8])
}
"#,
    )
    .expect("write test source");

    let codegen = codegen_program(main.to_string_lossy().into_owned());
    assert!(codegen.diagnostics.is_empty(), "{:?}", codegen.diagnostics);

    let output = emit_llvm_ir(&codegen.backend_lowering.program);
    assert!(output.diagnostics.is_empty(), "{:?}", output.diagnostics);
    let ir = &output.modules[0].ir;
    assert!(ir.contains("fir.tmp."), "{ir}");
    assert!(ir.contains("nia__vtable__"), "{ir}");
    assert!(ir.contains("call i32 %vtable.fn"), "{ir}");
}

#[test]
fn emits_slice_trait_object_coercions_through_adapter() {
    let root = temp_dir("emits_slice_trait_object_coercions_through_adapter");
    let main = root.join("main.nia");
    std::fs::write(
        &main,
        r#"
trait Source {
    fn get(& self) i32;
}

extend[T] [T] : Source {
    fn get(& self) i32 {
        self.len() as i32
    }
}

fn read(source: & Source) i32 {
    source.get()
}

fn main() i32 {
    let mut values: [3]i32 = [1, 2, 3];
    read(&values[..])
}
"#,
    )
    .expect("write test source");

    let codegen = codegen_program(main.to_string_lossy().into_owned());
    assert!(codegen.diagnostics.is_empty(), "{:?}", codegen.diagnostics);

    let output = emit_llvm_ir(&codegen.backend_lowering.program);
    assert!(output.diagnostics.is_empty(), "{:?}", output.diagnostics);
    let ir = &output.modules[0].ir;
    assert!(ir.contains("traitobj.self"), "{ir}");
    assert!(ir.contains("nia__traitobj_adapter__"), "{ir}");
    assert!(ir.contains("define internal"), "{ir}");
    assert!(ir.contains("call i32 %vtable.fn"), "{ir}");
}

#[test]
fn slice_trait_object_default_method_preserves_receiver_abi() {
    let root = temp_dir("slice_trait_object_default_method_preserves_receiver_abi");
    let main = root.join("main.nia");
    std::fs::write(
        &main,
        r#"
trait Source {
    fn get(& self) i32;

    fn get_plus(& self, rhs: i32) i32 {
        self.get() + rhs
    }
}

extend[T] [T] : Source {
    fn get(& self) i32 {
        self.len() as i32
    }
}

fn read(source: & Source) i32 {
    source.get_plus(4)
}

fn main() i32 {
    let mut values: [3]i32 = [1, 2, 3];
    read(&values[..])
}
"#,
    )
    .expect("write test source");

    let codegen = codegen_program(main.to_string_lossy().into_owned());
    assert!(codegen.diagnostics.is_empty(), "{:?}", codegen.diagnostics);

    let output = emit_llvm_ir(&codegen.backend_lowering.program);
    assert!(output.diagnostics.is_empty(), "{:?}", output.diagnostics);
    let ir = &output.modules[0].ir;
    assert!(ir.contains("traitobj.self"), "{ir}");
    assert!(ir.contains("nia__traitobj_adapter__"), "{ir}");
    assert!(ir.contains("call i32 %vtable.fn"), "{ir}");
}

#[test]
fn slice_trait_object_uses_more_specific_impl_method() {
    let root = temp_dir("slice_trait_object_uses_more_specific_impl_method");
    let main = root.join("main.nia");
    std::fs::write(
        &main,
        r#"
trait Source {
    fn get(& self) i32;
}

extend[T] [T] : Source {
    fn get(& self) i32 {
        1
    }
}

extend [char] : Source {
    fn get(& self) i32 {
        2
    }
}

fn read(source: & Source) i32 {
    source.get()
}

fn main() i32 {
    let text = "nia";
    read(&text[..])
}
"#,
    )
    .expect("write test source");

    let codegen = codegen_program(main.to_string_lossy().into_owned());
    assert!(codegen.diagnostics.is_empty(), "{:?}", codegen.diagnostics);

    let output = emit_llvm_ir(&codegen.backend_lowering.program);
    assert!(output.diagnostics.is_empty(), "{:?}", output.diagnostics);
    let ir = &output.modules[0].ir;
    assert!(ir.contains("nia__traitobj_adapter__"), "{ir}");
    assert!(ir.contains("ret i32 2"), "{ir}");
}

#[test]
fn slice_trait_object_adapter_preserves_value_argument_abi() {
    let root = temp_dir("slice_trait_object_adapter_preserves_value_argument_abi");
    let main = root.join("main.nia");
    std::fs::write(
        &main,
        r#"
struct Empty {}

trait Source {
    fn add(& self, empty: Empty, rhs: i32) i32;
}

extend[T] [T] : Source {
    fn add(& self, empty: Empty, rhs: i32) i32 {
        _ = empty;
        self.len() as i32 + rhs
    }
}

fn read(source: & Source) i32 {
    source.add({}, 4)
}

fn main() i32 {
    let mut values: [3]i32 = [1, 2, 3];
    read(&values[..])
}
"#,
    )
    .expect("write test source");

    let codegen = codegen_program(main.to_string_lossy().into_owned());
    assert!(codegen.diagnostics.is_empty(), "{:?}", codegen.diagnostics);

    let output = emit_llvm_ir(&codegen.backend_lowering.program);
    assert!(output.diagnostics.is_empty(), "{:?}", output.diagnostics);
    let ir = &output.modules[0].ir;
    assert!(ir.contains("nia__traitobj_adapter__"), "{ir}");
    assert!(ir.contains("call i32 %vtable.fn(ptr %"), "{ir}");
}

#[test]
fn size_levels_deduplicate_repeated_trait_object_vtables() {
    let root = temp_dir("size_levels_deduplicate_repeated_trait_object_vtables");
    let main = root.join("main.nia");
    std::fs::write(
        &main,
        r#"
trait Source {
    fn add(& self, rhs: i32) i32;
}

struct Counter {
    value: i32,
}

extend Counter : Source {
    fn add(& self, rhs: i32) i32 {
        self.value + rhs
    }
}

fn read(source: & Source) i32 {
    source.add(4)
}

fn left() i32 {
    let mut counter: Counter = { value: 8 };
    read(& counter)
}

fn right() i32 {
    let mut counter: Counter = { value: 9 };
    read(& counter)
}

fn main() i32 {
    left() + right()
}
"#,
    )
    .expect("write test source");

    for level in [NiaOptimizationLevel::Os, NiaOptimizationLevel::Oz] {
        let codegen = codegen_program_with_options(main.to_string_lossy().into_owned(), level);
        assert!(codegen.diagnostics.is_empty(), "{:?}", codegen.diagnostics);

        let module = &codegen.backend_lowering.program.modules[0];
        assert_eq!(module.trait_object_vtables.len(), 1, "{level:?}");
        assert_eq!(module.trait_object_vtables[0].entries.len(), 1, "{level:?}");

        let output = emit_llvm_ir(&codegen.backend_lowering.program);
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
    fn add(& self, rhs: i32) i32;
}

struct Counter {
    value: i32,
}

extend Counter : Source {
    fn add(& self, rhs: i32) i32 {
        self.value + rhs
    }
}

fn read(source: & Source) i32 {
    source.add(4)
}

fn main() i32 {
    let mut counter: Counter = { value: 8 };
    defer {
        log(read(& counter))
    };
    0
}
"#,
    )
    .expect("write test source");

    let codegen = codegen_program(main.to_string_lossy().into_owned());
    assert!(codegen.diagnostics.is_empty(), "{:?}", codegen.diagnostics);

    let output = emit_llvm_ir(&codegen.backend_lowering.program);
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
    fn parent(& self) i32;
}

trait Child : Parent {
    fn child(& self) i32;
}

struct Counter {
    value: i32,
}

extend Counter : Parent {
    fn parent(& self) i32 {
        self.value
    }
}

extend Counter : Child {
    fn child(& self) i32 {
        self.value + 1
    }
}

fn as_parent(child: & Child) & Parent {
    child
}

fn main() i32 {
    let mut counter: Counter = { value: 8 };
    let mut child: & Child = & counter;
    as_parent(child).parent()
}
"#,
    )
    .expect("write test source");

    let codegen = codegen_program(main.to_string_lossy().into_owned());
    assert!(codegen.diagnostics.is_empty(), "{:?}", codegen.diagnostics);
    let output = emit_llvm_ir(&codegen.backend_lowering.program);
    assert!(output.diagnostics.is_empty(), "{:?}", output.diagnostics);
    let ir = &output.modules[0].ir;
    assert!(ir.contains("traitobj.upcast.metadata.offset"), "{ir}");
    assert!(ir.contains("vtable.fn"), "{ir}");
}

#[test]
fn emits_trait_object_upcast_to_second_supertrait_with_assoc_bindings() {
    let root = temp_dir("emits_trait_object_upcast_to_second_supertrait_with_assoc_bindings");
    let main = root.join("main.nia");
    std::fs::write(
        &main,
        r#"
trait FatherA {
    type Item;

    fn a(& self) [Self as FatherA]::Item;
}

trait FatherB {
    type Item;

    fn b(& self) [Self as FatherB]::Item;
}

trait Child : FatherA + FatherB {
    fn child(& self) i32;
}

struct Both {
    value: i32,
}

extend Both : FatherA {
    type Item = i32;

    fn a(& self) i32 {
        self.value
    }
}

extend Both : FatherB {
    type Item = usize;

    fn b(& self) usize {
        2usize
    }
}

extend Both : Child {
    fn child(& self) i32 {
        self.value
    }
}

fn as_b(child: & Child[
    [Self as FatherA]::Item = i32,
    [Self as FatherB]::Item = usize,
]) & FatherB[Item = usize] {
    child
}

fn main() usize {
    let mut both: Both = { value: 8 };
    let mut child: & Child[
        [Self as FatherA]::Item = i32,
        [Self as FatherB]::Item = usize,
    ] = & both;
    as_b(child).b()
}
"#,
    )
    .expect("write test source");

    let codegen = codegen_program(main.to_string_lossy().into_owned());
    assert!(codegen.diagnostics.is_empty(), "{:?}", codegen.diagnostics);
    let output = emit_llvm_ir(&codegen.backend_lowering.program);
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
    fn eq(& self, other: & Self) bool;
}

struct Point {
    x: i32,
}

extend Point : Same {
    fn eq(& self, other: & Point) bool {
        self.x == other.x
    }
}

fn same[T](a: & T, b: & T) bool
where T: Same {
    a.eq(b)
}

fn main() bool {
    let mut a: Point = { x: 1 };
    let mut b: Point = { x: 1 };
    same[Point](& a, & b)
}
"#,
    )
    .expect("write test source");

    let codegen = codegen_program(main.to_string_lossy().into_owned());
    assert!(codegen.diagnostics.is_empty(), "{:?}", codegen.diagnostics);

    let output = emit_llvm_ir(&codegen.backend_lowering.program);
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
    fn eq(& self, other: & Self) bool;
}

trait Ranked : Same {
    fn lt(& self, other: & Self) bool;
}

struct Point {
    x: i32,
}

extend Point : Same {
    fn eq(& self, other: & Point) bool {
        self.x == other.x
    }
}

extend Point : Ranked {
    fn lt(& self, other: & Point) bool {
        self.x < other.x
    }
}

fn same_ord[T](a: & T, b: & T) bool
where T: Ranked {
    a.eq(b)
}

fn main() bool {
    let mut a: Point = { x: 1 };
    let mut b: Point = { x: 1 };
    same_ord[Point](& a, & b)
}
"#,
    )
    .expect("write test source");

    let codegen = codegen_program(main.to_string_lossy().into_owned());
    assert!(codegen.diagnostics.is_empty(), "{:?}", codegen.diagnostics);

    let output = emit_llvm_ir(&codegen.backend_lowering.program);
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

    fn get(& self) [Self as Source]::Item;
}

struct Counter {
    value: i32,
}

extend Counter : Source {
    type Item = i32;

    fn get(& self) i32 {
        self.value
    }
}

fn read[T](value: & T) [T as Source]::Item
where T: Source {
    value.get()
}

fn main() i32 {
    let mut counter: Counter = { value: 42 };
    read[Counter](& counter)
}
"#,
    )
    .expect("write test source");

    let codegen = codegen_program(main.to_string_lossy().into_owned());
    assert!(codegen.diagnostics.is_empty(), "{:?}", codegen.diagnostics);

    let output = emit_llvm_ir(&codegen.backend_lowering.program);
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

    fn map_c(& self, a: A, b: B) [Self as Mapper[A, B]]::C;

    fn map_d(& self, a: A, b: B, fallback: [Self as Mapper[A, B]]::D) [Self as Mapper[A, B]]::D {
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

    fn map_c(& self, a: i32, b: i32) i32 {
        self.seed + a + b
    }
}

fn mapped[T](value: & T, fallback: [T as Mapper[i32, i32]]::D) [T as Mapper[i32, i32]]::D
where T: Mapper[i32, i32] {
    value.map_d(1, 2, fallback)
}

fn main() i32 {
    let mut p: Pairer = { seed: 3 };
    mapped[Pairer](& p, 9)
}
"#,
    )
    .expect("write test source");

    let codegen = codegen_program(main.to_string_lossy().into_owned());
    assert!(codegen.diagnostics.is_empty(), "{:?}", codegen.diagnostics);

    let output = emit_llvm_ir(&codegen.backend_lowering.program);
    assert!(output.diagnostics.is_empty(), "{:?}", output.diagnostics);
    let ir = &output.modules[0].ir;
    assert!(ir.contains("call i32 @"), "{ir}");
    assert!(ir.contains("ret i32"), "{ir}");
}

#[test]
fn emits_mut_receiver_default_trait_method_with_associated_error() {
    let root = temp_dir("emits_mut_receiver_default_trait_method_with_associated_error");
    let main = root.join("main.nia");
    std::fs::write(
        &main,
        r#"
trait Writer {
    type Error;

    fn write(&mut self, bytes: &[u8]) Error!usize;

    fn write_all(&mut self, bytes: &[u8]) Error!void {
        _ = self.write(bytes).?;
        !{}
    }
}

trait FormatWriter {
    type Error;

    fn write_fmt_bytes(&mut self, bytes: &[u8]) Error!void;
}

extend[W] W : FormatWriter
where W: Writer
{
    type Error = [W as Writer]::Error;

    fn write_fmt_bytes(&mut self, bytes: &[u8]) Error!void {
        self.write_all(bytes)
    }
}

enum Error: i32 {
    Bad = 1,
    _,
}

struct Sink {
    count: usize,
}

extend Sink : Writer {
    type Error = Error;

    fn write(&mut self, bytes: &[u8]) Error!usize {
        self.count += bytes.len();
        !bytes.len()
    }
}

fn use_format[W](writer: &mut W, bytes: &[u8]) [W as FormatWriter]::Error!void
where W: FormatWriter
{
    writer.write_fmt_bytes(bytes)
}

fn main() i32 {
    let mut sink: Sink = { count: 0 };
    if !ok = use_format[Sink](&mut sink, &b"ok") {
        _ = ok;
        sink.count as i32
    } or error! {
        1
    }
}
"#,
    )
    .expect("write test source");

    let codegen = codegen_program(main.to_string_lossy().into_owned());
    assert!(codegen.diagnostics.is_empty(), "{:?}", codegen.diagnostics);

    let instance_symbols = codegen
        .backend_lowering
        .program
        .modules
        .iter()
        .flat_map(|module| module.function_instances.iter())
        .map(|instance| instance.symbol.as_str())
        .collect::<Vec<_>>();
    assert!(
        !instance_symbols
            .iter()
            .any(|symbol| symbol.contains("__inst__ptr__nom__Sink")),
        "{instance_symbols:#?}"
    );
    assert!(
        instance_symbols
            .iter()
            .any(|symbol| symbol.contains("write_all__inst__t_self_nom__")),
        "{instance_symbols:#?}"
    );
    assert!(
        instance_symbols
            .iter()
            .any(|symbol| symbol.contains("write_fmt_bytes__inst__t_nom__")),
        "{instance_symbols:#?}"
    );

    let output = emit_llvm_ir(&codegen.backend_lowering.program);
    assert!(output.diagnostics.is_empty(), "{:?}", output.diagnostics);
    let ir = &output.modules[0].ir;
    assert!(ir.contains("write_all__inst__"), "{ir}");
    assert!(ir.contains("write_fmt_bytes__inst__"), "{ir}");
    assert!(ir.contains("ret i32"), "{ir}");
}

#[test]
fn concrete_trait_impl_methods_resolve_local_associated_type_names() {
    let root = temp_dir("concrete_trait_impl_methods_resolve_local_associated_type_names");
    let main = root.join("main.nia");
    std::fs::write(
        &main,
        r#"
trait Writer {
    type Error;

    fn short_write(&self) Error;

    fn write(&mut self, bytes: &[u8]) Error!usize;
}

enum WriteError: i32 {
    Short = 1,
    _,
}

struct Sink {}

extend Sink : Writer {
    type Error = WriteError;

    fn short_write(&self) Error {
        WriteError::Short
    }

    fn write(&mut self, bytes: &[u8]) Error!usize {
        if bytes.len() == 0 {
            return self.short_write()!;
        }
        !bytes.len()
    }
}

fn main() i32 {
    let mut sink = Sink {};
    if !value = sink.write(&b"ok") {
        value as i32
    } or error! {
        0
    }
}
"#,
    )
    .expect("write test source");

    let codegen = codegen_program(main.to_string_lossy().into_owned());
    assert!(codegen.diagnostics.is_empty(), "{:?}", codegen.diagnostics);

    let output = emit_llvm_ir(&codegen.backend_lowering.program);
    assert!(output.diagnostics.is_empty(), "{:?}", output.diagnostics);
}

#[test]
fn emits_trait_default_method_instances() {
    let root = temp_dir("emits_trait_default_method_instances");
    let main = root.join("main.nia");
    std::fs::write(
        &main,
        r#"
trait Same {
    fn eq(& self, other: & Self) bool;

    fn ne(& self, other: & Self) bool {
        not self.eq(other)
    }
}

struct Point {
    x: i32,
}

extend Point : Same {
    fn eq(& self, other: & Point) bool {
        self.x == other.x
    }
}

fn different[T](a: & T, b: & T) bool
where T: Same {
    a.ne(b)
}

fn main() bool {
    let mut a: Point = { x: 1 };
    let mut b: Point = { x: 2 };
    different[Point](& a, & b)
}
"#,
    )
    .expect("write test source");

    let codegen = codegen_program(main.to_string_lossy().into_owned());
    assert!(codegen.diagnostics.is_empty(), "{:?}", codegen.diagnostics);

    let output = emit_llvm_ir(&codegen.backend_lowering.program);
    assert!(output.diagnostics.is_empty(), "{:?}", output.diagnostics);
    let ir = &output.modules[0].ir;
    assert!(ir.contains("call i1 @"));
    assert!(ir.contains("xor i1"));
}
