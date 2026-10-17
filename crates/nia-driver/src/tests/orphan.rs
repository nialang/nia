// SPDX-License-Identifier: GPL-3.0-or-later
use super::common::*;
use crate::{ModuleMap, SourcePath};

#[test]
fn trait_impl_in_a_third_package_is_rejected() {
    let root = temp_dir("trait_impl_in_a_third_package_is_rejected");
    write(
        &root.join("main.nia"),
        r#"
using extra::impls;

fn main() i32 {
    impls::marker();
    0
}
"#,
    );
    write(&root.join("dep.nia"), "pub module types;");
    std::fs::create_dir_all(root.join("dep")).expect("create type package directory");
    write(
        &root.join("dep/types.nia"),
        "pub struct Widget { value: i32 }",
    );
    write(&root.join("traits.nia"), "pub module show;");
    std::fs::create_dir_all(root.join("traits")).expect("create trait package directory");
    write(
        &root.join("traits/show.nia"),
        "pub trait Show { fn show(&self) i32; }",
    );
    write(&root.join("extra.nia"), "pub module impls;");
    std::fs::create_dir_all(root.join("extra")).expect("create third package directory");
    write(
        &root.join("extra/impls.nia"),
        r#"
using dep::types;
using traits::show;

extend types::Widget : show::Show {
    fn show(&self) i32 { self.value }
}

pub fn marker() () {}
"#,
    );

    let mut module_map = ModuleMap::new();
    module_map
        .insert(
            "dep",
            SourcePath::new(root.join("dep.nia").to_string_lossy()),
        )
        .expect("insert type package");
    module_map
        .insert(
            "traits",
            SourcePath::new(root.join("traits.nia").to_string_lossy()),
        )
        .expect("insert trait package");
    module_map
        .insert(
            "extra",
            SourcePath::new(root.join("extra.nia").to_string_lossy()),
        )
        .expect("insert third package");

    let checked = check_program_with_map(root.join("main.nia").to_string_lossy(), module_map);
    assert!(
        checked.diagnostics.iter().any(|diagnostic| {
            diagnostic
                .diagnostic
                .summary
                .contains("trait implementation violates the package orphan rule")
        }),
        "missing orphan diagnostic: {:?}",
        checked.diagnostics
    );
}

#[test]
fn structural_trait_impl_must_be_declared_in_the_trait_package() {
    let root = temp_dir("structural_trait_impl_must_be_declared_in_the_trait_package");
    write(
        &root.join("main.nia"),
        r#"
using traits::show;
using extra::impls;

fn main(value: &[i32]) i32 {
    value.show()
}

"#,
    );
    write(&root.join("traits.nia"), "pub module show;");
    std::fs::create_dir_all(root.join("traits")).expect("create trait package directory");
    write(
        &root.join("traits/show.nia"),
        "pub trait Show { fn show(&self) i32; }",
    );
    write(&root.join("extra.nia"), "pub module impls;");
    std::fs::create_dir_all(root.join("extra")).expect("create third package directory");
    write(
        &root.join("extra/impls.nia"),
        r#"
using traits::show;

extend &[i32] : show::Show {
    fn show(&self) i32 { 0 }
}

"#,
    );

    let mut module_map = ModuleMap::new();
    module_map
        .insert(
            "traits",
            SourcePath::new(root.join("traits.nia").to_string_lossy()),
        )
        .expect("insert trait package");
    module_map
        .insert(
            "extra",
            SourcePath::new(root.join("extra.nia").to_string_lossy()),
        )
        .expect("insert third package");

    let checked = check_program_with_map(root.join("main.nia").to_string_lossy(), module_map);
    assert!(
        checked.diagnostics.iter().any(|diagnostic| {
            diagnostic
                .diagnostic
                .summary
                .contains("structural trait implementations must be declared in the trait package")
        }),
        "missing structural orphan diagnostic: {:?}",
        checked.diagnostics
    );
}

#[test]
fn duplicate_trait_impl_headers_are_rejected() {
    let root = temp_dir("duplicate_trait_impl_headers_are_rejected");
    write(
        &root.join("main.nia"),
        r#"
trait Marker {}
struct Value {}

extend Value : Marker {}
extend Value : Marker {}

fn main() () {}
"#,
    );
    let checked = check_program(root.join("main.nia").to_string_lossy());
    assert!(
        checked.diagnostics.iter().any(|diagnostic| {
            diagnostic
                .diagnostic
                .summary
                .contains("overlapping trait implementations for the same target and trait")
        }),
        "missing overlap diagnostic: {:?}",
        checked.diagnostics
    );
}

#[test]
fn generic_and_concrete_trait_impl_headers_are_rejected_as_overlapping() {
    let root = temp_dir("generic_and_concrete_trait_impl_headers_are_rejected_as_overlapping");
    write(
        &root.join("main.nia"),
        r#"
trait Marker[T] {}
struct Value {}

extend[T] Value : Marker[T] {}
extend Value : Marker[i32] {}

fn main() () {}
"#,
    );
    let checked = check_program(root.join("main.nia").to_string_lossy());
    assert!(
        checked.diagnostics.iter().any(|diagnostic| {
            diagnostic
                .diagnostic
                .summary
                .contains("overlapping trait implementations for the same target and trait")
        }),
        "missing generic overlap diagnostic: {:?}",
        checked.diagnostics
    );
}
