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

fn accept(parent: &const Parent) void {}

fn use_child(child: &const Child) void {
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

fn accept(parent: &const Parent) void {}

fn use_other(other: &const Other) void {
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
    fn get(&const self) i32;
}

struct Counter {
    value: i32,
}

extend Counter : Source {
    fn get(&const self) i32 {
        self.value
    }
}

fn read(source: &const Source) i32 {
    source.get()
}

fn main() i32 {
    var counter: Counter = { value: 8 };
    read(&const counter)
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
                .any(|body| body_contains_dynamic_trait_callee(body))
    }));
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

fn read(bad: &const Bad) i32 {
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
    fn id[T](&const self, value: T) T;
}

fn read(bad: &const Bad) i32 {
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
    fn same(&const self, other: &const Self) bool;
}

fn read(bad: &const Bad) bool {
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
fn const_trait_object_rejects_mutable_receiver_method() {
    let root = temp_dir("const_trait_object_rejects_mutable_receiver_method");
    write(
        &root.join("main.nia"),
        r#"
trait Mutate {
    fn set(&self, value: i32);
}

struct Cell {
    value: i32,
}

extend Cell : Mutate {
    fn set(&self, value: i32) {
        self.value = value;
    }
}

fn write_value(cell: &const Mutate) {
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
            .contains("receiver cannot be matched through `&const Trait`")),
        "{:?}",
        program.diagnostics
    );
}
