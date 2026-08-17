// SPDX-License-Identifier: GPL-3.0-or-later
use super::common::*;

fn has_trait_object_coercion(module: &nia_compiler_query::CheckedModule) -> bool {
    module
        .semantic_facts
        .iter_node_trait_object_coercions()
        .next()
        .is_some()
}

fn trait_object_upcast_count(module: &nia_compiler_query::CheckedModule) -> usize {
    module
        .semantic_facts
        .iter_node_trait_object_upcasts()
        .count()
}

#[test]
fn trait_object_extension_methods_resolve_on_object_values() {
    let root = temp_dir("trait_object_extension_methods_resolve_on_object_values");
    write(
        &root.join("main.nia"),
        r#"
trait Allocator {
    fn alloc(&mut self) i32;
}

extend Allocator {
    fn alloc_twice(&mut self) i32 {
        self.alloc() + self.alloc()
    }
}

struct PageAllocator {
    value: i32,
}

extend PageAllocator : Allocator {
    fn alloc(&mut self) i32 {
        self.value
    }
}

fn use_allocator(allocator: &mut Allocator) i32 {
    allocator.alloc_twice()
}

fn main() i32 {
    let mut allocator = PageAllocator { value: 21 };
    use_allocator(&mut allocator)
}
"#,
    );

    let program = check_program(root.join("main.nia").to_string_lossy().into_owned());
    assert!(program.diagnostics.is_empty(), "{:?}", program.diagnostics);
}

#[test]
fn trait_object_extension_methods_infer_method_generics() {
    let root = temp_dir("trait_object_extension_methods_infer_method_generics");
    write(
        &root.join("main.nia"),
        r#"
trait Allocator {
    fn alloc(&mut self, len: usize) i32;
}

extend Allocator {
    fn alloc_slice[T](&mut self, len: usize) i32
    where T: Sized
    {
        self.alloc(len * std::builtin::size[T]())
    }
}

struct PageAllocator {
    value: i32,
}

extend PageAllocator : Allocator {
    fn alloc(&mut self, len: usize) i32 {
        self.value + len as i32
    }
}

fn use_allocator(allocator: &mut Allocator) i32 {
    allocator.alloc_slice[i32](4usize)
}

fn main() i32 {
    let mut allocator = PageAllocator { value: 21 };
    use_allocator(&mut allocator)
}
"#,
    );

    let program = check_program(root.join("main.nia").to_string_lossy().into_owned());
    assert!(program.diagnostics.is_empty(), "{:?}", program.diagnostics);
}

#[test]
fn readonly_trait_object_extension_methods_resolve_on_readonly_objects() {
    let root = temp_dir("readonly_trait_object_extension_methods_resolve_on_readonly_objects");
    write(
        &root.join("main.nia"),
        r#"
trait Source {
    fn get(&self) i32;
}

extend Source {
    fn get_plus_one(&self) i32 {
        self.get() + 1
    }
}

struct Counter {
    value: i32,
}

extend Counter : Source {
    fn get(&self) i32 {
        self.value
    }
}

fn read(source: &Source) i32 {
    source.get_plus_one()
}

fn main() i32 {
    let mut counter = Counter { value: 41 };
    read(&counter)
}
"#,
    );

    let program = check_program(root.join("main.nia").to_string_lossy().into_owned());
    assert!(program.diagnostics.is_empty(), "{:?}", program.diagnostics);
}

#[test]
fn bare_trait_is_still_not_a_value_type() {
    let root = temp_dir("bare_trait_is_still_not_a_value_type");
    write(
        &root.join("main.nia"),
        r#"
trait Allocator {}

fn main() i32 {
    let mut allocator: Allocator;
    0
}
"#,
    );

    let program = check_program(root.join("main.nia").to_string_lossy().into_owned());
    assert!(
        program
            .diagnostics
            .iter()
            .any(|diagnostic| diagnostic.diagnostic.code.as_str() == "E0202"
                && diagnostic
                    .diagnostic
                    .summary
                    .contains("trait types are not valid as values")),
        "{:?}",
        program.diagnostics
    );
}

#[test]
fn trait_object_supertrait_upcast_is_recorded() {
    let root = temp_dir("trait_object_supertrait_upcast_is_recorded");
    write(
        &root.join("main.nia"),
        r#"
trait Parent {}
trait Child : Parent {}

fn accept(parent: & Parent) () {}

fn use_child(child: & Child) () {
    accept(child)
}
"#,
    );

    let program = check_program(root.join("main.nia").to_string_lossy().into_owned());
    assert!(program.diagnostics.is_empty(), "{:?}", program.diagnostics);
    assert!(
        program
            .modules
            .iter()
            .any(|module| trait_object_upcast_count(module) > 0),
        "{:?}",
        program
            .modules
            .iter()
            .map(|module| {
                module
                    .semantic_facts
                    .iter_node_trait_object_upcasts()
                    .collect::<Vec<_>>()
            })
            .collect::<Vec<_>>()
    );
}

#[test]
fn trait_object_non_supertrait_upcast_is_rejected() {
    let root = temp_dir("trait_object_non_supertrait_upcast_is_rejected");
    write(
        &root.join("main.nia"),
        r#"
trait Parent {}
trait Other {}

fn accept(parent: & Parent) () {}

fn use_other(other: & Other) () {
    accept(other)
}
"#,
    );

    let program = check_program(root.join("main.nia").to_string_lossy().into_owned());
    assert!(
        program
            .diagnostics
            .iter()
            .any(|diagnostic| diagnostic.diagnostic.summary.contains("type mismatch")),
        "{:?}",
        program.diagnostics
    );
}

#[test]
fn concrete_pointer_coerces_to_trait_object_and_dispatches_method() {
    let root = temp_dir("concrete_pointer_coerces_to_trait_object_and_dispatches_method");
    write(
        &root.join("main.nia"),
        r#"
trait Source {
    fn get(& self) i32;
}

struct Counter {
    value: i32,
}

extend Counter : Source {
    fn get(& self) i32 {
        self.value
    }
}

fn read(source: & Source) i32 {
    source.get()
}

fn main() i32 {
    let mut counter = Counter { value: 8 };
    read(& counter)
}
"#,
    );

    let program = check_program(root.join("main.nia").to_string_lossy().into_owned());
    assert!(program.diagnostics.is_empty(), "{:?}", program.diagnostics);
    assert!(program.modules.iter().any(|module| {
        has_trait_object_coercion(module)
            && module
                .body_ir
                .function_bodies
                .values()
                .any(|body| body_contains_dynamic_trait_callee(body))
    }));
}

#[test]
fn const_generic_trait_object_preserves_impl_and_supertrait_instances() {
    let root = temp_dir("const_generic_trait_object_preserves_impl_and_supertrait_instances");
    write(
        &root.join("main.nia"),
        r#"
trait Base[N: usize] {
    fn value(& self) usize {
        8usize
    }
}

trait Scaled[N: usize] : Base[N] {
    fn doubled(& self) usize;
}

struct Meter {}

extend[N: usize] Meter : Base[N] {}

extend[N: usize] Meter : Scaled[N] {
    fn doubled(& self) usize {
        16usize
    }
}

fn read(value: & Scaled[8]) usize {
    value.value() + value.doubled()
}

fn main() usize {
    let meter = Meter {};
    read(& meter)
}
"#,
    );

    let program = codegen_program(root.join("main.nia").to_string_lossy().into_owned());
    assert!(program.diagnostics.is_empty(), "{:?}", program.diagnostics);
    let vtable = program
        .backend_lowering
        .program
        .modules
        .iter()
        .flat_map(|module| &module.trait_object_vtables)
        .next()
        .expect("const-generic trait-object vtable");
    assert_eq!(const_arg_bits(&vtable.trait_const_args), vec![8]);
    assert_eq!(vtable.entries.len(), 2);
    for entry in &vtable.entries {
        assert_eq!(const_arg_bits(&entry.trait_const_args), vec![8]);
        let nia_backend_ir::BackendTraitObjectVtableFunction::FunctionInstance {
            const_args, ..
        } = &entry.function
        else {
            panic!("const-generic vtable method must reference a function instance");
        };
        assert_eq!(const_arg_bits(const_args), vec![8]);
    }
}

fn const_arg_bits(args: &[nia_ty::ConstGenericArg]) -> Vec<u64> {
    args.iter()
        .map(|arg| match &arg.value {
            nia_ty::ConstGenericValue::Int(value) => value.bits() as u64,
            other => panic!("expected concrete integer const argument, got {other:?}"),
        })
        .collect()
}

#[test]
fn mutable_pointer_coerces_to_mutable_trait_object_and_dispatches_method() {
    let root = temp_dir("mutable_pointer_coerces_to_mutable_trait_object_and_dispatches_method");
    write(
        &root.join("main.nia"),
        r#"
trait CounterLike {
    fn bump(&mut self) i32;
}

struct Counter {
    value: i32,
}

extend Counter : CounterLike {
    fn bump(&mut self) i32 {
        self.value += 1;
        self.value
    }
}

fn bump(counter: &mut CounterLike) i32 {
    counter.bump()
}

fn main() i32 {
    let mut counter = Counter { value: 8 };
    bump(&mut counter)
}
"#,
    );

    let program = check_program(root.join("main.nia").to_string_lossy().into_owned());
    assert!(program.diagnostics.is_empty(), "{:?}", program.diagnostics);
    assert!(program.modules.iter().any(|module| {
        has_trait_object_coercion(module)
            && module
                .body_ir
                .function_bodies
                .values()
                .any(|body| body_contains_dynamic_trait_callee(body))
    }));
}

#[test]
fn array_literal_reference_elements_coerce_to_trait_objects() {
    let root = temp_dir("array_literal_reference_elements_coerce_to_trait_objects");
    write(
        &root.join("main.nia"),
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
    );

    let program = check_program(root.join("main.nia").to_string_lossy().into_owned());
    assert!(program.diagnostics.is_empty(), "{:?}", program.diagnostics);
    assert!(
        program
            .modules
            .iter()
            .any(|module| has_trait_object_coercion(module))
    );
}

#[test]
fn reference_coerces_to_readonly_trait_object_argument() {
    let root = temp_dir("reference_coerces_to_readonly_trait_object_argument");
    write(
        &root.join("main.nia"),
        r#"
trait Source {
    fn get(& self) i32;
}

extend i32 : Source {
    fn get(& self) i32 {
        self.*
    }
}

fn read(source: & Source) i32 {
    source.get()
}

fn main() i32 {
    read(&8)
}
"#,
    );

    let program = check_program(root.join("main.nia").to_string_lossy().into_owned());
    assert!(program.diagnostics.is_empty(), "{:?}", program.diagnostics);
    assert!(
        program
            .modules
            .iter()
            .any(|module| has_trait_object_coercion(module))
    );
}

#[test]
fn value_does_not_coerce_to_readonly_trait_object_argument() {
    let root = temp_dir("value_does_not_coerce_to_readonly_trait_object_argument");
    write(
        &root.join("main.nia"),
        r#"
trait Source {
    fn get(& self) i32;
}

extend i32 : Source {
    fn get(& self) i32 {
        self.*
    }
}

fn read(source: & Source) i32 {
    source.get()
}

fn main() i32 {
    read(8)
}
"#,
    );

    let program = check_program(root.join("main.nia").to_string_lossy().into_owned());
    assert!(
        program
            .diagnostics
            .iter()
            .any(|diagnostic| diagnostic.diagnostic.summary.contains("type mismatch")),
        "{:?}",
        program.diagnostics
    );
}

#[test]
fn imported_public_trait_impl_private_method_coerces_to_trait_object() {
    let root = temp_dir("imported_public_trait_impl_private_method_coerces_to_trait_object");
    write(
        &root.join("main.nia"),
        r#"
module fmt;
using entry::fmt;

fn read(source: &fmt::Format) i32 {
    source.format()
}

fn main() i32 {
    read(&8)
}
"#,
    );
    write(
        &root.join("fmt.nia"),
        r#"
pub trait Format {
    fn format(&self) i32;
}

extend i32 : Format {
    fn format(&self) i32 {
        self.*
    }
}
"#,
    );

    let program = check_program(root.join("main.nia").to_string_lossy().into_owned());
    assert!(program.diagnostics.is_empty(), "{:?}", program.diagnostics);
    assert!(program.modules.iter().any(|module| {
        has_trait_object_coercion(module)
            && module
                .body_ir
                .function_bodies
                .values()
                .any(|body| body_contains_dynamic_trait_callee(body))
    }));
}

#[test]
fn facade_import_trait_object_coercion_records_cross_module_vtable_instance() {
    let root = temp_dir("facade_import_trait_object_coercion_records_cross_module_vtable_instance");
    write(
        &root.join("main.nia"),
        r#"
module std;
module fmt;
using entry::std;

fn use_all(args: &[&std::fmt::Format[std::fmt::Error]]) i32 {
    args[0].format()
}

fn main() i32 {
    use_all(&[&10])
}
"#,
    );
    write(
        &root.join("std.nia"),
        r#"
using entry::fmt;

pub using {fmt};
"#,
    );
    write(
        &root.join("fmt.nia"),
        r#"
pub enum Error {
    Failed,
}

pub trait Format[E] {
    fn format(&self) i32;
}

extend[E] i32 : Format[E] {
    fn format(&self) i32 {
        self.*
    }
}
"#,
    );

    let program = codegen_program(root.join("main.nia").to_string_lossy().into_owned());
    assert!(program.diagnostics.is_empty(), "{:?}", program.diagnostics);
    let vtable_instance_refs = program
        .backend_lowering
        .program
        .modules
        .iter()
        .flat_map(|module| module.trait_object_vtables.iter())
        .flat_map(|vtable| vtable.entries.iter())
        .filter_map(|entry| match &entry.function {
            nia_backend_ir::BackendTraitObjectVtableFunction::FunctionInstance {
                def_id,
                arg_module_id,
                args,
                ..
            } => Some((*def_id, *arg_module_id, args.clone())),
            nia_backend_ir::BackendTraitObjectVtableFunction::Function(_) => None,
        })
        .collect::<Vec<_>>();
    assert!(!vtable_instance_refs.is_empty());
    for (def_id, _arg_module_id, args) in vtable_instance_refs {
        let matches = program
            .backend_lowering
            .program
            .modules
            .iter()
            .flat_map(|module| module.function_instances.iter())
            .filter(|instance| instance.def_id == def_id && instance.args.len() == args.len())
            .count();
        assert_eq!(
            matches,
            1,
            "expected one canonical function instance for {def_id:?} with {} args",
            args.len()
        );
    }
}

#[test]
fn value_does_not_coerce_to_mutable_trait_object_argument() {
    let root = temp_dir("value_does_not_coerce_to_mutable_trait_object_argument");
    write(
        &root.join("main.nia"),
        r#"
trait Source {
    fn get(& self) i32;
}

extend i32 : Source {
    fn get(& self) i32 {
        self.*
    }
}

fn read(source: &mut Source) i32 {
    source.get()
}

fn main() i32 {
    read(8)
}
"#,
    );

    let program = check_program(root.join("main.nia").to_string_lossy().into_owned());
    assert!(
        program
            .diagnostics
            .iter()
            .any(|diagnostic| diagnostic.diagnostic.summary.contains("type mismatch")),
        "{:?}",
        program.diagnostics
    );
}

#[test]
fn trait_object_method_call_wins_over_blanket_extension_methods() {
    let root = temp_dir("trait_object_method_call_wins_over_blanket_extension_methods");
    write(
        &root.join("main.nia"),
        r#"
trait Other {}

trait Writer[E] {
    fn write_fmt_bytes(&mut self, bytes: &[u8]) E!();
}

struct Formatter[E] {
    writer: &mut Writer[E],
}

extend[E] Formatter[E] {
    fn write_all(&mut self, bytes: &[u8]) E!() {
        self.writer.write_fmt_bytes(bytes)
    }

    fn write_byte(&mut self) E!() {
        let mut bytes: [u8; 1] = [0];
        self.write_all(&bytes[..]).?;
        !()
    }
}

extend[W] W : Writer[i32]
where W: Other
{
    fn write_fmt_bytes(&mut self, bytes: &[u8]) i32!() {
        _ = bytes;
        !()
    }
}
"#,
    );

    let program = check_program(root.join("main.nia").to_string_lossy().into_owned());
    assert!(program.diagnostics.is_empty(), "{:?}", program.diagnostics);
    assert!(
        program
            .modules
            .iter()
            .flat_map(|module| module.body_ir.function_bodies.values())
            .any(|body| body_contains_dynamic_trait_callee(body))
    );
}

#[test]
fn trait_object_methods_may_return_bound_associated_types() {
    let root = temp_dir("trait_object_methods_may_return_bound_associated_types");
    write(
        &root.join("main.nia"),
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

fn read(source: & Source[Item = i32]) i32 {
    source.get()
}

fn main() i32 {
    let mut counter = Counter { value: 42 };
    read(& counter)
}
"#,
    );

    let program = check_program(root.join("main.nia").to_string_lossy().into_owned());
    assert!(program.diagnostics.is_empty(), "{:?}", program.diagnostics);
    assert!(program.modules.iter().any(|module| {
        module
            .body_ir
            .function_bodies
            .values()
            .any(|body| body_contains_dynamic_trait_callee(body))
    }));
}

#[test]
fn trait_object_upcast_matches_explicit_supertrait_associated_type_bindings() {
    let root = temp_dir("trait_object_upcast_matches_explicit_supertrait_associated_type_bindings");
    write(
        &root.join("main.nia"),
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
        1usize
    }
}

extend Both : Child {
    fn child(& self) i32 {
        self.value + 1
    }
}

fn read_a(parent: & FatherA[Item = i32]) i32 {
    parent.a()
}

fn read_b(parent: & FatherB[Item = usize]) usize {
    parent.b()
}

fn from_child(child: & Child[
    [Self as FatherA]::Item = i32,
    [Self as FatherB]::Item = usize,
]) i32 {
    read_a(child) + read_b(child) as i32
}

fn main() i32 {
    let mut both = Both { value: 41 };
    from_child(& both)
}
"#,
    );

    let program = check_program(root.join("main.nia").to_string_lossy().into_owned());
    assert!(program.diagnostics.is_empty(), "{:?}", program.diagnostics);
    assert!(program.modules.iter().any(|module| {
        trait_object_upcast_count(module) >= 2
            && module
                .body_ir
                .function_bodies
                .values()
                .any(|body| body_contains_dynamic_trait_callee(body))
    }));
}

#[test]
fn trait_object_upcast_rejects_unbound_supertrait_associated_type_fakeref() {
    let root = temp_dir("trait_object_upcast_rejects_unbound_supertrait_associated_type_fakeref");
    write(
        &root.join("main.nia"),
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

fn read_b(parent: & FatherB[Item = usize]) usize {
    parent.b()
}

fn forged(child: & Child[[Self as FatherA]::Item = i32]) usize {
    read_b(child)
}

fn main() i32 { 0 }
"#,
    );

    let program = check_program(root.join("main.nia").to_string_lossy().into_owned());
    assert!(
        program
            .diagnostics
            .iter()
            .any(|diagnostic| diagnostic.diagnostic.summary.contains("type mismatch")),
        "{:?}",
        program.diagnostics
    );
}

#[test]
fn trait_object_rejects_non_receiver_methods() {
    let root = temp_dir("trait_object_rejects_non_receiver_methods");
    write(
        &root.join("main.nia"),
        r#"
trait Bad {
    fn make() i32;
}

fn read(bad: & Bad) i32 {
    _ = bad;
    0
}
"#,
    );

    let program = check_program(root.join("main.nia").to_string_lossy().into_owned());
    assert!(
        program
            .diagnostics
            .iter()
            .any(|diagnostic| diagnostic.diagnostic.summary.contains("not object safe")),
        "{:?}",
        program.diagnostics
    );
}

#[test]
fn trait_object_rejects_method_generics() {
    let root = temp_dir("trait_object_rejects_method_generics");
    write(
        &root.join("main.nia"),
        r#"
trait Bad {
    fn id[T](& self, value: T) T;
}

fn read(bad: & Bad) i32 {
    bad::id[i32](1)
}
"#,
    );

    let program = check_program(root.join("main.nia").to_string_lossy().into_owned());
    assert!(
        program
            .diagnostics
            .iter()
            .any(|diagnostic| diagnostic.diagnostic.summary.contains("not object safe"))
    );
}

#[test]
fn trait_object_rejects_self_outside_receiver() {
    let root = temp_dir("trait_object_rejects_self_outside_receiver");
    write(
        &root.join("main.nia"),
        r#"
trait Bad {
    fn same(& self, other: & Self) bool;
}

fn read(bad: & Bad) bool {
    bad::same(bad)
}
"#,
    );

    let program = check_program(root.join("main.nia").to_string_lossy().into_owned());
    assert!(
        program
            .diagnostics
            .iter()
            .any(|diagnostic| diagnostic.diagnostic.summary.contains("mentions `Self`")),
        "{:?}",
        program.diagnostics
    );
}

#[test]
fn readonly_trait_object_rejects_mutable_receiver_method() {
    let root = temp_dir("readonly_trait_object_rejects_mutable_receiver_method");
    write(
        &root.join("main.nia"),
        r#"
trait Mutate {
    fn set(&mut self, value: i32);
}

struct Cell {
    value: i32,
}

extend Cell : Mutate {
    fn set(&mut self, value: i32) {
        self.value = value;
    }
}

fn write_value(cell: & Mutate) {
    cell.set(1);
}

fn main() i32 { 0 }
"#,
    );

    let program = check_program(root.join("main.nia").to_string_lossy().into_owned());
    assert!(
        program.diagnostics.iter().any(|diagnostic| diagnostic
            .diagnostic
            .summary
            .contains("receiver cannot be matched through read-only `&Trait`")),
        "{:?}",
        program.diagnostics
    );
}
