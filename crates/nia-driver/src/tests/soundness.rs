// SPDX-License-Identifier: GPL-3.0-or-later
use super::common::*;
use crate::check_program;

#[test]
fn open_projection_does_not_forge_concrete_type() {
    let root = temp_dir("open_projection_does_not_forge_concrete_type");
    write(
        &root.join("main.nia"),
        r#"
trait TypeIs[T] {
    type Is;
}

struct I32Proof {}

extend I32Proof : TypeIs[i32] {
    type Is = i32;
}

fn rewrite[R, RW](value: [RW as TypeIs[R]]::Is) R
where RW: TypeIs[R] {
    value
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
fn open_projection_does_not_forge_pointer_deref() {
    let root = temp_dir("open_projection_does_not_forge_pointer_deref");
    write(
        &root.join("main.nia"),
        r#"
trait RefLike {
    type Ref;
}

struct I32Ref {}

extend I32Ref : RefLike {
    type Ref = & i32;
}

fn read_open[T](value: [T as RefLike]::Ref) i32
where T: RefLike {
    value.*
}

fn read_concrete(value: [I32Ref as RefLike]::Ref) i32 {
    value.*
}

fn main() i32 {
    var value: i32 = 5;
    read_concrete(& value)
}
"#,
    );

    let program = check_program(root.join("main.nia").to_string_lossy().into_owned());
    assert!(
        program
            .diagnostics
            .iter()
            .any(|diagnostic| diagnostic.diagnostic.message.contains("DerefRead")),
        "{:?}",
        program.diagnostics
    );
}

#[test]
fn supertrait_bound_does_not_infer_parent_associated_type_equality() {
    let root = temp_dir("supertrait_bound_does_not_infer_parent_associated_type_equality");
    write(
        &root.join("main.nia"),
        r#"
trait TypeIs[T] {
    type Is;
}

trait Lift[T] : TypeIs[T] {}

struct I32Lift {}

extend I32Lift : TypeIs[i32] {
    type Is = i32;
}

extend I32Lift : Lift[i32] {}

fn rewrite[R, RW](value: [RW as TypeIs[R]]::Is) R
where RW: Lift[R] {
    value
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
fn generic_supertrait_arguments_remain_distinct_for_projection() {
    let root = temp_dir("generic_supertrait_arguments_remain_distinct_for_projection");
    write(
        &root.join("main.nia"),
        r#"
trait Source[T] {
    type Item;
}

trait SizedSource[T] : Source[T] {}

struct Source64 {}

extend Source64 : Source[i64] {
    type Item = i64;
}

extend Source64 : SizedSource[i64] {}

fn bad[S](value: [S as Source[i32]]::Item) [S as Source[i32]]::Item
where S: SizedSource[i64] {
    value
}

fn main() i32 { 0 }
"#,
    );

    let program = check_program(root.join("main.nia").to_string_lossy().into_owned());
    assert!(
        program.diagnostics.iter().any(|diagnostic| diagnostic
            .diagnostic
            .message
            .contains("trait bound not satisfied")),
        "{:?}",
        program.diagnostics
    );
}

#[test]
fn cross_module_associated_type_projection_resolves_impl_definition() {
    let root = temp_dir("cross_module_associated_type_projection_resolves_impl_definition");
    write(
        &root.join("traits.nia"),
        r#"
pub trait Source {
    type Item;

    fn get(& self) [Self as Source]::Item;
}
"#,
    );
    write(
        &root.join("main.nia"),
        r#"
import .traits;

struct Counter {
    value: i32,
}

extend Counter : traits::Source {
    type Item = i32;

    fn get(& self) i32 {
        self.value
    }
}

fn read[T](value: & T) [T as traits::Source]::Item
where T: traits::Source {
    value.get()
}

fn main() i32 {
    var counter: Counter = { value: 8 };
    read[Counter](& counter)
}
"#,
    );

    let program = check_program(root.join("main.nia").to_string_lossy().into_owned());
    assert!(program.diagnostics.is_empty(), "{:?}", program.diagnostics);
}
