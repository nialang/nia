// SPDX-License-Identifier: GPL-3.0-or-later
use super::common::*;
use crate::check_program;

#[test]
fn supertrait_impls_must_be_explicit() {
    let root = temp_dir("supertrait_impls_must_be_explicit");
    write(
        &root.join("main.nia"),
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

extend Point : Ranked {
    fn lt(& self, other: & Point) bool {
        self.x < other.x
    }
}

fn main() i32 { 0 }
"#,
    );

    let program = check_program(root.join("main.nia").to_string_lossy().into_owned());
    assert!(
        program.diagnostics.iter().any(|diagnostic| diagnostic
            .diagnostic
            .summary
            .contains("requires explicit implementation of supertrait `Same`")),
        "{:?}",
        program.diagnostics
    );
}

#[test]
fn where_bound_on_subtrait_exposes_supertrait_methods() {
    let root = temp_dir("where_bound_on_subtrait_exposes_supertrait_methods");
    write(
        &root.join("main.nia"),
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
    var a: Point = { x: 1 };
    var b: Point = { x: 1 };
    same_ord[Point](& a, & b)
}
"#,
    );

    let program = check_program(root.join("main.nia").to_string_lossy().into_owned());
    assert!(program.diagnostics.is_empty(), "{:?}", program.diagnostics);
}

#[test]
fn generic_supertraits_substitute_trait_arguments() {
    let root = temp_dir("generic_supertraits_substitute_trait_arguments");
    write(
        &root.join("main.nia"),
        r#"
trait Source[A] {
    type Item;

    fn get(& self) [Self as Source[A]]::Item;
}

trait SizedSource[A] : Source[A] {
    fn size(& self) usize;

    fn get_or(& self, fallback: [Self as Source[A]]::Item) [Self as Source[A]]::Item {
        if self.size() == 0 {
            fallback
        } else {
            self.get()
        }
    }
}

struct I32Source {
    value: i32,
}

extend I32Source : Source[i32] {
    type Item = i32;

    fn get(& self) i32 {
        self.value
    }
}

extend I32Source : SizedSource[i32] {
    fn size(& self) usize {
        1usize
    }
}

fn read_sized[S](value: & S, fallback: [S as Source[i32]]::Item) [S as Source[i32]]::Item
where S: SizedSource[i32] {
    value.get_or(fallback)
}

fn main() i32 {
    var source: I32Source = { value: 7 };
    read_sized[I32Source](& source, 9)
}
"#,
    );

    let program = check_program(root.join("main.nia").to_string_lossy().into_owned());
    assert!(program.diagnostics.is_empty(), "{:?}", program.diagnostics);
}

#[test]
fn generic_where_bound_trait_methods_dispatch_to_impl_instances() {
    let root = temp_dir("generic_where_bound_trait_methods_dispatch_to_impl_instances");
    write(
        &root.join("main.nia"),
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
    var a: Point = { x: 1 };
    var b: Point = { x: 1 };
    same[Point](& a, & b)
}
"#,
    );

    let program = check_program(root.join("main.nia").to_string_lossy().into_owned());
    assert!(program.diagnostics.is_empty(), "{:?}", program.diagnostics);
}

#[test]
fn trait_default_methods_are_used_when_impl_omits_method() {
    let root = temp_dir("trait_default_methods_are_used_when_impl_omits_method");
    write(
        &root.join("main.nia"),
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
    var a: Point = { x: 1 };
    var b: Point = { x: 2 };
    different[Point](& a, & b)
}
"#,
    );

    let program = check_program(root.join("main.nia").to_string_lossy().into_owned());
    assert!(program.diagnostics.is_empty(), "{:?}", program.diagnostics);
}
