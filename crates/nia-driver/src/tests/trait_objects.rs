// SPDX-License-Identifier: GPL-3.0-or-later
use super::common::*;
use crate::check_program;

#[test]
fn trait_object_supertrait_upcast_is_recorded() {
    let root = temp_dir("trait_object_supertrait_upcast_is_recorded");
    write(
        &root.join("main.nia"),
        r#"
trait Parent {}
trait Child : Parent {}

fn accept(parent: & Parent) void {}

fn use_child(child: & Child) void {
    accept(child)
}
"#,
    );

    let program = check_program(root.join("main.nia").to_string_lossy().into_owned());
    assert!(program.diagnostics.is_empty(), "{:?}", program.diagnostics);
    assert!(
        program.modules.iter().any(|module| {
            !module.body_check.ir.trait_object_upcasts.is_empty()
                && !module.body_check.ir.node_trait_object_upcasts.is_empty()
        }),
        "{:?}",
        program
            .modules
            .iter()
            .map(|module| &module.body_check.ir.trait_object_upcasts)
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

fn accept(parent: & Parent) void {}

fn use_other(other: & Other) void {
    accept(other)
}
"#,
    );

    let program = check_program(root.join("main.nia").to_string_lossy().into_owned());
    assert!(
        program
            .diagnostics
            .iter()
            .any(|diagnostic| diagnostic.diagnostic.message.contains("type mismatch")),
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
    var counter: Counter = { value: 8 };
    read(& counter)
}
"#,
    );

    let program = check_program(root.join("main.nia").to_string_lossy().into_owned());
    assert!(program.diagnostics.is_empty(), "{:?}", program.diagnostics);
    assert!(program.modules.iter().any(|module| {
        !module.body_check.ir.trait_object_coercions.is_empty()
            && module
                .body_check
                .ir
                .function_bodies
                .values()
                .any(body_contains_dynamic_trait_callee)
    }));
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
    var counter: Counter = { value: 42 };
    read(& counter)
}
"#,
    );

    let program = check_program(root.join("main.nia").to_string_lossy().into_owned());
    assert!(program.diagnostics.is_empty(), "{:?}", program.diagnostics);
    assert!(program.modules.iter().any(|module| {
        module
            .body_check
            .ir
            .function_bodies
            .values()
            .any(body_contains_dynamic_trait_callee)
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
    var both: Both = { value: 41 };
    from_child(& both)
}
"#,
    );

    let program = check_program(root.join("main.nia").to_string_lossy().into_owned());
    assert!(program.diagnostics.is_empty(), "{:?}", program.diagnostics);
    assert!(program.modules.iter().any(|module| {
        module.body_check.ir.trait_object_upcasts.len() >= 2
            && module
                .body_check
                .ir
                .function_bodies
                .values()
                .any(body_contains_dynamic_trait_callee)
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
            .any(|diagnostic| diagnostic.diagnostic.message.contains("type mismatch")),
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
            .any(|diagnostic| diagnostic.diagnostic.message.contains("not object safe")),
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
    bad.id[i32](1)
}
"#,
    );

    let program = check_program(root.join("main.nia").to_string_lossy().into_owned());
    assert!(
        program
            .diagnostics
            .iter()
            .any(|diagnostic| diagnostic.diagnostic.message.contains("not object safe"))
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
    bad.same(bad)
}
"#,
    );

    let program = check_program(root.join("main.nia").to_string_lossy().into_owned());
    assert!(
        program
            .diagnostics
            .iter()
            .any(|diagnostic| diagnostic.diagnostic.message.contains("mentions `Self`")),
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
            .message
            .contains("receiver cannot be matched through read-only `&Trait`")),
        "{:?}",
        program.diagnostics
    );
}
