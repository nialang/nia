// SPDX-License-Identifier: GPL-3.0-or-later
use super::common::*;
use crate::check_program;

#[test]
fn trait_impl_methods_are_checked_against_trait_requirements() {
    let root = temp_dir("trait_impl_methods_are_checked_against_trait_requirements");
    write(
        &root.join("main.nia"),
        r#"
trait Show {
    fn show(&const self) i32;
}

struct Point {
    x: i32,
}

extend Point : Show {
    fn show(&const self) i32 {
        self.x
    }
}

fn main() i32 {
    var point: Point = { x: 7 };
    point.show()
}
"#,
    );

    let program = check_program(root.join("main.nia").to_string_lossy().into_owned());
    assert!(program.diagnostics.is_empty(), "{:?}", program.diagnostics);
}

#[test]
fn trait_impl_rejects_extra_missing_and_mismatched_methods() {
    let root = temp_dir("trait_impl_rejects_extra_missing_and_mismatched_methods");
    write(
        &root.join("main.nia"),
        r#"
trait Show {
    fn show(&const self) i32;
    fn size(&const self) i32;
}

struct Point {
    x: i32,
}

extend Point : Show {
    fn show(&self) i32 {
        self.x
    }

    fn debug(&const self) i32 {
        self.x
    }
}

fn main() i32 {
    0
}
"#,
    );

    let program = check_program(root.join("main.nia").to_string_lossy().into_owned());
    assert!(
        program.diagnostics.iter().any(|diagnostic| diagnostic
            .diagnostic
            .message
            .contains("does not match the trait signature")),
        "{:?}",
        program.diagnostics
    );
    assert!(
        program.diagnostics.iter().any(|diagnostic| diagnostic
            .diagnostic
            .message
            .contains("is not a member of implemented trait")),
        "{:?}",
        program.diagnostics
    );
    assert!(
        program.diagnostics.iter().any(|diagnostic| diagnostic
            .diagnostic
            .message
            .contains("missing implementation for trait method `size`")),
        "{:?}",
        program.diagnostics
    );
}

#[test]
fn trait_impl_substitutes_self_in_required_signatures() {
    let root = temp_dir("trait_impl_substitutes_self_in_required_signatures");
    write(
        &root.join("main.nia"),
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

fn main() bool {
    var a: Point = { x: 1 };
    var b: Point = { x: 1 };
    a.eq(&const b)
}
"#,
    );

    let program = check_program(root.join("main.nia").to_string_lossy().into_owned());
    assert!(program.diagnostics.is_empty(), "{:?}", program.diagnostics);
}

#[test]
fn cross_module_trait_impls_are_checked() {
    let root = temp_dir("cross_module_trait_impls_are_checked");
    write(
        &root.join("main.nia"),
        r#"
import .traits;

struct Point {
    x: i32,
}

extend Point : traits::Show {
    fn show(&const self) i32 {
        self.x
    }

    fn debug(&const self) i32 {
        self.x
    }
}

fn main() i32 {
    0
}
"#,
    );
    write(
        &root.join("traits.nia"),
        r#"
pub trait Show {
    fn show(&const self) i32;
    fn size(&const self) i32;
}
"#,
    );

    let program = check_program(root.join("main.nia").to_string_lossy().into_owned());
    assert!(
        program.diagnostics.iter().any(|diagnostic| diagnostic
            .diagnostic
            .message
            .contains("is not a member of implemented trait")),
        "{:?}",
        program.diagnostics
    );
    assert!(
        program.diagnostics.iter().any(|diagnostic| diagnostic
            .diagnostic
            .message
            .contains("missing implementation for trait method `size`")),
        "{:?}",
        program.diagnostics
    );
}
