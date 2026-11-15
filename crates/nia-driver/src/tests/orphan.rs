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

fun main() i32 {
    impls::marker();
    0
}
"#,
    );
    write(&root.join("dep/root.nia"), "pub module types;");
    std::fs::create_dir_all(root.join("dep")).expect("create type package directory");
    write(
        &root.join("dep/types.nia"),
        "pub struct Widget { value: i32 }",
    );
    write(&root.join("traits/root.nia"), "pub module show;");
    std::fs::create_dir_all(root.join("traits")).expect("create trait package directory");
    write(
        &root.join("traits/show.nia"),
        "pub trait Show { fun show(&self) i32; }",
    );
    write(&root.join("extra/root.nia"), "pub module impls;");
    std::fs::create_dir_all(root.join("extra")).expect("create third package directory");
    write(
        &root.join("extra/impls.nia"),
        r#"
using dep::types;
using traits::show;

extend types::Widget : show::Show {
    fun show(&self) i32 { self.value }
}

pub fun marker() () {}
"#,
    );

    let mut module_map = ModuleMap::new();
    module_map
        .insert(
            "dep",
            SourcePath::new(root.join("dep/root.nia").to_string_lossy()),
        )
        .expect("insert type package");
    module_map
        .insert(
            "traits",
            SourcePath::new(root.join("traits/root.nia").to_string_lossy()),
        )
        .expect("insert trait package");
    module_map
        .insert(
            "extra",
            SourcePath::new(root.join("extra/root.nia").to_string_lossy()),
        )
        .expect("insert third package");

    let checked = check_program_with_map(root.join("main.nia").to_string_lossy(), module_map);
    assert!(
        checked.diagnostics.iter().any(|diagnostic| {
            diagnostic
                .diagnostic
                .summary
                .contains("trait implementation violates the orphan rule")
        }),
        "missing orphan diagnostic: {:?}",
        checked.diagnostics
    );
}

#[test]
fn builtin_trait_impl_in_a_third_package_is_rejected() {
    let root = temp_dir("builtin_trait_impl_in_a_third_package_is_rejected");
    write(
        &root.join("main.nia"),
        r#"
using extra::impls;

fun main() i32 {
    impls::marker();
    0
}
"#,
    );
    write(&root.join("dep/root.nia"), "pub module types;");
    std::fs::create_dir_all(root.join("dep")).expect("create type package directory");
    write(
        &root.join("dep/types.nia"),
        "pub struct Widget { value: usize }",
    );
    write(&root.join("extra/root.nia"), "pub module impls;");
    std::fs::create_dir_all(root.join("extra")).expect("create third package directory");
    write(
        &root.join("extra/impls.nia"),
        r#"
using dep::types;
using std::builtin::Len;

extend types::Widget : Len {
    fun len(&self) usize { self.value }
}

pub fun marker() () {}
"#,
    );

    let mut module_map = ModuleMap::new();
    module_map
        .insert(
            "dep",
            SourcePath::new(root.join("dep/root.nia").to_string_lossy()),
        )
        .expect("insert type package");
    module_map
        .insert(
            "extra",
            SourcePath::new(root.join("extra/root.nia").to_string_lossy()),
        )
        .expect("insert third package");

    let checked = check_program_with_map(root.join("main.nia").to_string_lossy(), module_map);
    assert!(
        checked.diagnostics.iter().any(|diagnostic| {
            diagnostic
                .diagnostic
                .summary
                .contains("trait implementation violates the orphan rule")
        }),
        "missing orphan diagnostic: {:?}",
        checked.diagnostics
    );
}

#[test]
fn structural_trait_impl_without_a_local_type_is_rejected() {
    let root = temp_dir("structural_trait_impl_without_a_local_type_is_rejected");
    write(
        &root.join("main.nia"),
        r#"
using traits::show;
using extra::impls;

fun main(value: &[i32]) i32 {
    value.show()
}

"#,
    );
    write(&root.join("traits/root.nia"), "pub module show;");
    std::fs::create_dir_all(root.join("traits")).expect("create trait package directory");
    write(
        &root.join("traits/show.nia"),
        "pub trait Show { fun show(&self) i32; }",
    );
    write(&root.join("extra/root.nia"), "pub module impls;");
    std::fs::create_dir_all(root.join("extra")).expect("create third package directory");
    write(
        &root.join("extra/impls.nia"),
        r#"
using traits::show;

extend &[i32] : show::Show {
    fun show(&self) i32 { 0 }
}

"#,
    );

    let mut module_map = ModuleMap::new();
    module_map
        .insert(
            "traits",
            SourcePath::new(root.join("traits/root.nia").to_string_lossy()),
        )
        .expect("insert trait package");
    module_map
        .insert(
            "extra",
            SourcePath::new(root.join("extra/root.nia").to_string_lossy()),
        )
        .expect("insert third package");

    let checked = check_program_with_map(root.join("main.nia").to_string_lossy(), module_map);
    assert!(
        checked.diagnostics.iter().any(|diagnostic| {
            diagnostic
                .diagnostic
                .summary
                .contains("trait implementation violates the orphan rule")
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

fun main() () {}
"#,
    );
    let checked = check_program(root.join("main.nia").to_string_lossy());
    assert!(
        checked.diagnostics.iter().any(|diagnostic| {
            diagnostic
                .diagnostic
                .summary
                .contains("duplicate trait implementation header")
        }),
        "missing overlap diagnostic: {:?}",
        checked.diagnostics
    );
}

#[test]
fn renamed_generic_parameters_do_not_make_duplicate_headers_distinct() {
    let root = temp_dir("renamed_generic_parameters_do_not_make_duplicate_headers_distinct");
    write(
        &root.join("main.nia"),
        r#"
trait Marker[T] {}
struct Value {}
extend[T] Value : Marker[T] {}
extend[U] Value : Marker[U] {}
fun main() () {}
"#,
    );
    let checked = check_program(root.join("main.nia").to_string_lossy());
    assert!(
        checked.diagnostics.iter().any(|diagnostic| diagnostic
            .diagnostic
            .summary
            .contains("duplicate trait implementation header")),
        "missing duplicate header diagnostic: {:?}",
        checked.diagnostics
    );
}

#[test]
fn conditional_trait_impls_with_the_same_pattern_are_selected_by_obligations() {
    let root =
        temp_dir("conditional_trait_impls_with_the_same_pattern_are_selected_by_obligations");
    write(
        &root.join("main.nia"),
        r#"
trait Left {}
trait Right {}
trait Marker { fun marker(&self) i32; }
struct Value {}
extend Value : Left {}
extend[T] T : Marker where T: Left {
    fun marker(&self) i32 { 1 }
}
extend[T] T : Marker where T: Right {
    fun marker(&self) i32 { 2 }
}
fun main() i32 { Value {}.marker() }
"#,
    );
    let checked = check_program(root.join("main.nia").to_string_lossy());
    assert!(
        checked.diagnostics.is_empty(),
        "unexpected diagnostics: {:?}",
        checked.diagnostics
    );
}

#[test]
fn generic_and_concrete_trait_impl_headers_allow_specialization() {
    let root = temp_dir("generic_and_concrete_trait_impl_headers_allow_specialization");
    write(
        &root.join("main.nia"),
        r#"
trait Marker[T] {}
struct Value {}

extend[T] Value : Marker[T] {}
extend Value : Marker[i32] {}

fun main() () {}
"#,
    );
    let checked = check_program(root.join("main.nia").to_string_lossy());
    assert!(
        checked.diagnostics.is_empty(),
        "unexpected diagnostics: {:?}",
        checked.diagnostics
    );
}

/// A main file that loads the implementations of `coherence_packages`.
const MAIN_USING_IMPLS: &str = r#"
using extra::impls;

fun main() i32 {
    impls::marker();
    0
}
"#;

/// Packages for the orphan tests below: `dep` defines types, `traits` a
/// generic trait, and `extra` a type of its own and an implementation that
/// uses it as the trait argument.
fn coherence_packages(root: &std::path::Path, extra_impls: &str) -> ModuleMap {
    write(&root.join("dep/root.nia"), "pub module types;");
    write(
        &root.join("dep/types.nia"),
        "pub struct Widget { value: i32 }\npub struct Box[T] { value: T }",
    );
    write(&root.join("traits/root.nia"), "pub module convert;");
    write(
        &root.join("traits/convert.nia"),
        "pub trait Convert[To] { fun convert(&self) To; }",
    );
    write(
        &root.join("extra/root.nia"),
        "pub module wrap;\npub module impls;",
    );
    write(
        &root.join("extra/wrap.nia"),
        "pub struct Wrapper { value: i32 }",
    );
    // `marker` lets a test load the implementations' module by using it.
    write(
        &root.join("extra/impls.nia"),
        &format!(
            "{extra_impls}
pub fun marker() () {{}}
"
        ),
    );
    let mut module_map = ModuleMap::new();
    for package in ["dep", "traits", "extra"] {
        module_map
            .insert(
                package,
                SourcePath::new(root.join(package).join("root.nia").to_string_lossy()),
            )
            .expect("insert package");
    }
    module_map
}

fn orphan_summaries(checked: &nia_compiler_query::CheckedProgramAnalysis) -> Vec<String> {
    checked
        .diagnostics
        .iter()
        .map(|diagnostic| diagnostic.diagnostic.summary.clone())
        .collect()
}

/// A package that owns a trait argument may implement a foreign trait for a
/// foreign type, and a package that never imports the implementation's
/// module finds it through the argument's package.
#[test]
fn trait_argument_owner_may_implement_and_is_searched() {
    let root = temp_dir("trait_argument_owner_may_implement_and_is_searched");
    write(
        &root.join("main.nia"),
        r#"
using dep::types;
using traits::convert;
using extra::wrap;

fun use[T](value: T) i32
where T: convert::Convert[wrap::Wrapper]
{
    value.convert().value
}

fun main() i32 {
    use(types::Widget { value: 3 })
}
"#,
    );
    let module_map = coherence_packages(
        &root,
        r#"
using dep::types;
using traits::convert;
using root::wrap;

extend types::Widget : convert::Convert[wrap::Wrapper] {
    fun convert(&self) wrap::Wrapper {
        .{ value: self.value }
    }
}
"#,
    );
    let checked = check_program_with_map(root.join("main.nia").to_string_lossy(), module_map);
    assert!(
        checked.diagnostics.is_empty(),
        "{:?}",
        orphan_summaries(&checked)
    );
}

/// A type parameter before the first local type could stand for a type
/// another package owns, which could then implement the same goal.
#[test]
fn uncovered_type_parameter_before_the_local_type_is_rejected() {
    let root = temp_dir("uncovered_type_parameter_before_the_local_type_is_rejected");
    write(&root.join("main.nia"), "fun main() i32 { 0 }\n");
    let module_map = coherence_packages(
        &root,
        r#"
using traits::convert;
using root::wrap;

extend[T] T : convert::Convert[wrap::Wrapper] {
    fun convert(&self) wrap::Wrapper {
        .{ value: 0 }
    }
}
"#,
    );
    let main = root.join("main.nia");
    write(&main, MAIN_USING_IMPLS);
    let checked = check_program_with_map(main.to_string_lossy(), module_map);
    let summaries = orphan_summaries(&checked);
    assert!(
        summaries
            .iter()
            .any(|summary| summary == "type parameter `T` must be covered by a local type"),
        "{summaries:?}"
    );
}

/// A parameter inside another type is covered, and a pointer to a local
/// type is local, as Rust's fundamental references are.
#[test]
fn covered_parameters_and_pointers_to_local_types_are_allowed() {
    let root = temp_dir("covered_parameters_and_pointers_to_local_types_are_allowed");
    let module_map = coherence_packages(
        &root,
        r#"
using dep::types;
using traits::convert;
using root::wrap;

extend[T] types::Box[T] : convert::Convert[wrap::Wrapper] {
    fun convert(&self) wrap::Wrapper {
        .{ value: 1 }
    }
}

extend &wrap::Wrapper : convert::Convert[i32] {
    fun convert(&self) i32 {
        self.value
    }
}
"#,
    );
    let main = root.join("main.nia");
    write(&main, MAIN_USING_IMPLS);
    let checked = check_program_with_map(main.to_string_lossy(), module_map);
    assert!(
        checked.diagnostics.is_empty(),
        "{:?}",
        orphan_summaries(&checked)
    );
}

/// A user's error type absorbs a dependency's error through `IntoError`
/// declared in the user's package, so `.?` converts it.
#[test]
fn user_error_types_absorb_dependency_errors() {
    let root = temp_dir("user_error_types_absorb_dependency_errors");
    write(
        &root.join("dep/root.nia"),
        "pub enum Error {\n    Bad,\n}\n\npub fun fail() Error!i32 {\n    Error::Bad!\n}\n",
    );
    let source = root.join("main.nia");
    write(
        &source,
        r#"
using std::process;

enum AppError {
    Dep(dep::Error),
}

extend dep::Error : std::builtin::IntoError[AppError] {
    fun intoError(self) AppError {
        .Dep(self)
    }
}

fun run() AppError!i32 {
    let value = dep::fail().?;
    !value
}

pub fun main(init: process::Init) process::ExitCode!() {
    _ = init;
    match run() {
        AppError::Dep(dep::Error::Bad)! => !(),
        _ => process::ExitCode(1)!,
    }
}
"#,
    );
    let mut module_map = ModuleMap::new();
    module_map
        .insert(
            "dep",
            SourcePath::new(root.join("dep/root.nia").to_string_lossy()),
        )
        .expect("insert dependency");
    let artifact = test_driver()
        .link_executable(crate::LinkExecutableRequest::new(
            crate::CheckRequest::new(source.to_string_lossy().into_owned())
                .with_module_map(module_map)
                .with_runtime(test_freestanding_runtime()),
            root.join("runner.exe"),
        ))
        .result
        .expect("conversion into a local error must link");
    let status = std::process::Command::new(&artifact.path)
        .status()
        .expect("run the conversion");
    assert_eq!(status.code(), Some(0));
}
