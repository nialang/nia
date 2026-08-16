// SPDX-License-Identifier: GPL-3.0-or-later
use super::common::*;

#[test]
fn using_brings_imported_function_into_scope() {
    let root = temp_dir("using_brings_imported_function_into_scope");
    write(
        &root.join("main.nia"),
        r#"
module math;
using entry::math;
using math::add;

fn main() i32 {
    add(40, 2)
}
"#,
    );
    write(
        &root.join("math.nia"),
        r#"pub fn add(a: i32, b: i32) i32 { a + b }"#,
    );

    let program = check_program(root.join("main.nia").to_string_lossy().into_owned());
    assert_no_error_diagnostics(&program.diagnostics);
}

#[test]
fn using_supports_group_and_rename() {
    let root = temp_dir("using_supports_group_and_rename");
    write(
        &root.join("main.nia"),
        r#"
module math;
using entry::math;
using math::{add, sub as minus};

fn main() i32 {
    add(40, minus(4, 2))
}
"#,
    );
    write(
        &root.join("math.nia"),
        r#"
pub fn add(a: i32, b: i32) i32 { a + b }
pub fn sub(a: i32, b: i32) i32 { a - b }
"#,
    );

    let program = check_program(root.join("main.nia").to_string_lossy().into_owned());
    assert_no_error_diagnostics(&program.diagnostics);
}

#[test]
fn using_group_supports_nested_enum_wildcard() {
    let root = temp_dir("using_group_supports_nested_enum_wildcard");
    write(
        &root.join("main.nia"),
        r#"
module math;
using entry::math;
using math::{add, sub as minus, Operator::*};

fn main(flag: bool) math::Operator {
    let mut n = add(40, minus(4, 2));
    if flag { Add } else { Sub }
}
"#,
    );
    write(
        &root.join("math.nia"),
        r#"
pub enum Operator: u8 { Add, Sub }
pub fn add(a: i32, b: i32) i32 { a + b }
pub fn sub(a: i32, b: i32) i32 { a - b }
"#,
    );

    let program = check_program(root.join("main.nia").to_string_lossy().into_owned());
    assert_no_error_diagnostics(&program.diagnostics);
}

#[test]
fn using_wildcard_imports_public_surface() {
    let root = temp_dir("using_wildcard_imports_public_surface");
    write(
        &root.join("main.nia"),
        r#"
module math;
using entry::math;
using math::*;

fn main(p: Point) i32 {
    add(p.x, p.y)
}
"#,
    );
    write(
        &root.join("math.nia"),
        r#"
pub struct Point { x: i32, y: i32 }
pub fn add(a: i32, b: i32) i32 { a + b }
"#,
    );

    let program = check_program(root.join("main.nia").to_string_lossy().into_owned());
    assert_no_error_diagnostics(&program.diagnostics);
}

#[test]
fn using_wildcard_imports_pub_using_reexports() {
    let root = temp_dir("using_wildcard_imports_pub_using_reexports");
    write(
        &root.join("main.nia"),
        r#"
module facade;
module impl;
using entry::facade;
using facade::*;

fn main(p: Point) i32 {
    add(p.x, p.y)
}
"#,
    );
    write(
        &root.join("facade.nia"),
        r#"
using entry::impl;
pub using impl::*;
"#,
    );
    write(
        &root.join("impl.nia"),
        r#"
pub struct Point { x: i32, y: i32 }
pub fn add(a: i32, b: i32) i32 { a + b }
"#,
    );

    let program = check_program(root.join("main.nia").to_string_lossy().into_owned());
    assert_no_error_diagnostics(&program.diagnostics);
}

#[test]
fn pub_using_wildcard_does_not_reexport_non_public_child_modules() {
    let root = temp_dir("pub_using_wildcard_does_not_reexport_non_public_child_modules");
    write(
        &root.join("main.nia"),
        r#"
module parent;
using entry::parent;

fn main() i32 {
    parent::facade::internal::secret()
}
"#,
    );
    write(
        &root.join("parent.nia"),
        r#"
pub module facade;
pub(super) module source;
"#,
    );
    std::fs::create_dir_all(root.join("parent")).expect("create parent dir");
    write(
        &root.join("parent/facade.nia"),
        r#"
using super::source;
pub using source::*;
"#,
    );
    write(
        &root.join("parent/source.nia"),
        r#"
pub(super) module internal;
pub fn visible() i32 { 1 }
"#,
    );
    std::fs::create_dir_all(root.join("parent/source")).expect("create source dir");
    write(
        &root.join("parent/source/internal.nia"),
        r#"
pub fn secret() i32 { 42 }
"#,
    );

    let program = check_program(root.join("main.nia").to_string_lossy().into_owned());
    assert!(
        program.diagnostics.iter().any(|diagnostic| diagnostic
            .diagnostic
            .summary
            .contains("unknown namespace `internal`")
            || diagnostic
                .diagnostic
                .summary
                .contains("unknown value `internal`")),
        "{:?}",
        program.diagnostics
    );
}

#[test]
fn pub_using_module_namespace_is_visible_downstream() {
    let root = temp_dir("pub_using_module_namespace_is_visible_downstream");
    write(
        &root.join("main.nia"),
        r#"
module facade;
module impl;
using entry::facade;

fn main() i32 {
    facade::impl::add(40, 2)
}
"#,
    );
    write(
        &root.join("facade.nia"),
        r#"
using entry::impl;
pub using impl;
"#,
    );
    write(
        &root.join("impl.nia"),
        r#"pub fn add(a: i32, b: i32) i32 { a + b }"#,
    );

    let program = check_program(root.join("main.nia").to_string_lossy().into_owned());
    assert_no_error_diagnostics(&program.diagnostics);
}

#[test]
fn using_wildcard_brings_reexported_module_namespace_into_scope() {
    let root = temp_dir("using_wildcard_brings_reexported_module_namespace_into_scope");
    write(
        &root.join("main.nia"),
        r#"
module facade;
module impl;
using entry::facade;
using facade::*;

fn main() i32 {
    impl::add(40, 2)
}
"#,
    );
    write(
        &root.join("facade.nia"),
        r#"
using entry::impl;
pub using impl;
"#,
    );
    write(
        &root.join("impl.nia"),
        r#"pub fn add(a: i32, b: i32) i32 { a + b }"#,
    );

    let program = check_program(root.join("main.nia").to_string_lossy().into_owned());
    assert_no_error_diagnostics(&program.diagnostics);
}

#[test]
fn pub_using_reexports_for_downstream_modules() {
    let root = temp_dir("pub_using_reexports_for_downstream_modules");
    write(
        &root.join("main.nia"),
        r#"
module facade;
module impl;
using entry::facade;

fn main() i32 {
    facade::add(40, 2)
}
"#,
    );
    write(
        &root.join("facade.nia"),
        r#"
using entry::impl;
pub using impl::add;
"#,
    );
    write(
        &root.join("impl.nia"),
        r#"pub fn add(a: i32, b: i32) i32 { a + b }"#,
    );

    let program = check_program(root.join("main.nia").to_string_lossy().into_owned());
    assert_no_error_diagnostics(&program.diagnostics);
}

#[test]
fn pub_using_group_supports_nested_enum_wildcard() {
    let root = temp_dir("pub_using_group_supports_nested_enum_wildcard");
    write(
        &root.join("main.nia"),
        r#"
module facade;
module math;
using entry::facade;

fn main(flag: bool) facade::Operator {
    let mut n = facade::add(40, facade::minus(4, 2));
    if flag { facade::Add } else { facade::Sub }
}
"#,
    );
    write(
        &root.join("facade.nia"),
        r#"
using entry::math;
pub using math::{Operator, add, sub as minus, Operator::*};
"#,
    );
    write(
        &root.join("math.nia"),
        r#"
pub enum Operator: u8 { Add, Sub }
pub fn add(a: i32, b: i32) i32 { a + b }
pub fn sub(a: i32, b: i32) i32 { a - b }
"#,
    );

    let program = check_program(root.join("main.nia").to_string_lossy().into_owned());
    assert_no_error_diagnostics(&program.diagnostics);
}

#[test]
fn pub_using_root_group_reexports_modules_items_and_variants() {
    let root = temp_dir("pub_using_root_group_reexports_modules_items_and_variants");
    write(
        &root.join("main.nia"),
        r#"
module facade;
module math;
module palette;
using entry::facade;

fn main(flag: bool) facade::palette::Color {
    let mut n = facade::add(40, 2);
    if flag { facade::Red } else { facade::DDD }
}
"#,
    );
    write(
        &root.join("facade.nia"),
        r#"
using entry::math;
using entry::palette;
pub using {math, math::add, palette, palette::Color::{Red, DDD}};
"#,
    );
    write(
        &root.join("math.nia"),
        r#"pub fn add(a: i32, b: i32) i32 { a + b }"#,
    );
    write(
        &root.join("palette.nia"),
        r#"pub enum Color: u8 { Red, DDD }"#,
    );

    let program = check_program(root.join("main.nia").to_string_lossy().into_owned());
    assert_no_error_diagnostics(&program.diagnostics);
}

#[test]
fn using_supports_deep_nested_module_groups() {
    let root = temp_dir("using_supports_deep_nested_module_groups");
    write(
        &root.join("main.nia"),
        r#"
module rootmod;
module a;
module b;
module c;
module d;
module e;
module f;
module h;
using entry::rootmod;
using rootmod::a::{b::c::foo, d::e::{f::goo, g}, h::Color::*};

fn main(flag: bool) rootmod::a::h::Color {
    let mut n = foo(40, 2) + goo(1) + g(2);
    if flag { Red } else { Blue }
}
"#,
    );
    write(
        &root.join("rootmod.nia"),
        r#"
using entry::a;
pub using a;
"#,
    );
    write(
        &root.join("a.nia"),
        r#"
using entry::b;
using entry::d;
using entry::h;
pub using {b, d, h};
"#,
    );
    write(
        &root.join("b.nia"),
        r#"
using entry::c;
pub using c;
"#,
    );
    write(
        &root.join("c.nia"),
        r#"pub fn foo(a: i32, b: i32) i32 { a + b }"#,
    );
    write(
        &root.join("d.nia"),
        r#"
using entry::e;
pub using e;
"#,
    );
    write(
        &root.join("e.nia"),
        r#"
using entry::f;
pub using f;
pub fn g(a: i32) i32 { a + 3 }
"#,
    );
    write(&root.join("f.nia"), r#"pub fn goo(a: i32) i32 { a + 4 }"#);
    write(&root.join("h.nia"), r#"pub enum Color: u8 { Red, Blue }"#);

    let program = check_program(root.join("main.nia").to_string_lossy().into_owned());
    assert_no_error_diagnostics(&program.diagnostics);
}

#[test]
fn using_unknown_name_reports_diagnostic() {
    let root = temp_dir("using_unknown_name_reports_diagnostic");
    write(
        &root.join("main.nia"),
        r#"
module math;
using entry::math;
using math::missing;

fn main() i32 { 0 }
"#,
    );
    write(
        &root.join("math.nia"),
        r#"pub fn add(a: i32, b: i32) i32 { a + b }"#,
    );

    let program = check_program(root.join("main.nia").to_string_lossy().into_owned());
    assert!(
        program.diagnostics.iter().any(|diagnostic| {
            diagnostic
                .diagnostic
                .summary
                .contains("could not be resolved")
        }),
        "{:?}",
        program.diagnostics
    );
}

#[test]
fn pub_using_module_path_reexports_module_namespace() {
    let root = temp_dir("pub_using_module_path_reexports_module_namespace");
    write(
        &root.join("main.nia"),
        r#"
module math;
module facade;
using entry::facade;

fn main() i32 {
    facade::math::add(40, 2)
}
"#,
    );
    write(
        &root.join("facade.nia"),
        r#"
pub using entry::math;
"#,
    );
    write(
        &root.join("math.nia"),
        r#"pub fn add(a: i32, b: i32) i32 { a + b }"#,
    );

    let program = check_program(root.join("main.nia").to_string_lossy().into_owned());
    assert_no_error_diagnostics(&program.diagnostics);
}

#[test]
fn using_local_enum_variant_brings_bare_name() {
    let root = temp_dir("using_local_enum_variant_brings_bare_name");
    write(
        &root.join("main.nia"),
        r#"
enum Color: u8 { Red, Black }
using Color::Red;

fn main() Color { Red }
"#,
    );

    let program = check_program(root.join("main.nia").to_string_lossy().into_owned());
    assert_no_error_diagnostics(&program.diagnostics);
}

#[test]
fn using_local_enum_wildcard_brings_all_variants() {
    let root = temp_dir("using_local_enum_wildcard_brings_all_variants");
    write(
        &root.join("main.nia"),
        r#"
enum Color: u8 { Red, Black, Green }
using Color::*;

fn pick(flag: bool) Color {
    if flag { Red } else { Black }
}

fn main() Color { pick(true) }
"#,
    );

    let program = check_program(root.join("main.nia").to_string_lossy().into_owned());
    assert_no_error_diagnostics(&program.diagnostics);
}

#[test]
fn using_cross_module_enum_variant_three_segments() {
    let root = temp_dir("using_cross_module_enum_variant_three_segments");
    write(
        &root.join("main.nia"),
        r#"
module palette;
using entry::palette;
using palette::Color::{Red, Black as Dark};

fn main() palette::Color {
    let mut c: palette::Color = Red;
    Dark
}
"#,
    );
    write(
        &root.join("palette.nia"),
        r#"
pub enum Color: u8 { Red, Black, Green }
"#,
    );

    let program = check_program(root.join("main.nia").to_string_lossy().into_owned());
    assert_no_error_diagnostics(&program.diagnostics);
}

#[test]
fn pub_using_enum_variant_reexports_for_downstream() {
    let root = temp_dir("pub_using_enum_variant_reexports_for_downstream");
    write(
        &root.join("main.nia"),
        r#"
module facade;
module palette;
using entry::facade;

fn main() facade::Color {
    facade::Red
}
"#,
    );
    write(
        &root.join("facade.nia"),
        r#"
using entry::palette;
pub using palette::Color;
pub using palette::Color::Red;
"#,
    );
    write(
        &root.join("palette.nia"),
        r#"
pub enum Color: u8 { Red, Black, Green }
"#,
    );

    let program = check_program(root.join("main.nia").to_string_lossy().into_owned());
    assert_no_error_diagnostics(&program.diagnostics);
}

#[test]
fn using_unknown_enum_variant_reports_diagnostic() {
    let root = temp_dir("using_unknown_enum_variant_reports_diagnostic");
    write(
        &root.join("main.nia"),
        r#"
enum Color: u8 { Red, Black }
using Color::Purple;

fn main() Color { Red }
"#,
    );

    let program = check_program(root.join("main.nia").to_string_lossy().into_owned());
    assert!(
        program.diagnostics.iter().any(|diagnostic| diagnostic
            .diagnostic
            .summary
            .contains("unknown enum variant")),
        "{:?}",
        program.diagnostics
    );
}

#[test]
fn qualified_cross_module_enum_variant_access() {
    let root = temp_dir("qualified_cross_module_enum_variant_access");
    write(
        &root.join("main.nia"),
        r#"
module palette;
using entry::palette;

fn main() palette::Color {
    let mut c: palette::Color = palette::Color::Red;
    palette::Color::Black
}
"#,
    );
    write(
        &root.join("palette.nia"),
        r#"pub enum Color: u8 { Red, Black, Green }"#,
    );

    let program = check_program(root.join("main.nia").to_string_lossy().into_owned());
    assert_no_error_diagnostics(&program.diagnostics);
}

#[test]
fn lowers_cross_module_enum_equality_as_intrinsic_operator() {
    let root = temp_dir("lowers_cross_module_enum_equality_as_intrinsic_operator");
    write(
        &root.join("main.nia"),
        r#"
module palette;
using entry::palette;

fn same(a: palette::Color, b: palette::Color) bool {
    a == b
}
"#,
    );
    write(
        &root.join("palette.nia"),
        r#"pub enum Color: u8 { Red, Black, Green }"#,
    );

    let program = codegen_program(root.join("main.nia").to_string_lossy().into_owned());
    assert_no_error_diagnostics(&program.diagnostics);
    let main_module = program
        .backend_lowering
        .program
        .modules
        .iter()
        .find(|module| module.name.ends_with("main.nia"))
        .expect("main module");
    let same = main_module
        .functions
        .iter()
        .find(|function| function.name == sym("same"))
        .expect("same function");
    let body = same.function_body.as_ref().expect("same body");
    assert!(
        function_body_contains_builtin_eq(body),
        "{:#?}",
        same.function_body
    );
}

#[test]
fn using_imported_type_supports_enum_variants_and_associated_functions() {
    let root = temp_dir("using_imported_type_supports_enum_variants_and_associated_functions");
    write(
        &root.join("main.nia"),
        r#"
module defs;
using entry::defs;

using defs::{Box, Mode};

fn main() i32 {
    let mut box = Box::make(Mode::A);
    box.mode as u8 as i32
}
"#,
    );
    write(
        &root.join("defs.nia"),
        r#"
pub enum Mode: u8 {
    A,
    B,
}

pub struct Box {
    mode: Mode,
}

extend Box {
    pub fn make(mode: Mode) Box {
        Self { mode }
    }
}
"#,
    );

    let program = check_program(root.join("main.nia").to_string_lossy().into_owned());
    assert_no_error_diagnostics(&program.diagnostics);
}

#[test]
fn match_exhaustive_over_cross_module_enum() {
    let root = temp_dir("match_exhaustive_over_cross_module_enum");
    write(
        &root.join("main.nia"),
        r#"
module palette;
using entry::palette;

fn pick(c: palette::Color) i32 {
    match c {
        palette::Color::Red => return 0,
        palette::Color::Black => return 1,
        palette::Color::Green => return 2,
    }
    -1
}

fn main() i32 { pick(palette::Color::Red) }
"#,
    );
    write(
        &root.join("palette.nia"),
        r#"pub enum Color: u8 { Red, Black, Green }"#,
    );

    let program = check_program(root.join("main.nia").to_string_lossy().into_owned());
    assert_no_error_diagnostics(&program.diagnostics);
}

#[test]
fn match_over_cross_module_enum_reports_missing_variants() {
    let root = temp_dir("match_over_cross_module_enum_reports_missing_variants");
    write(
        &root.join("main.nia"),
        r#"
module palette;
using entry::palette;

fn pick(c: palette::Color) i32 {
    match c {
        palette::Color::Red => return 0,
    }
    -1
}

fn main() i32 { pick(palette::Color::Red) }
"#,
    );
    write(
        &root.join("palette.nia"),
        r#"pub enum Color: u8 { Red, Black, Green }"#,
    );

    let program = check_program(root.join("main.nia").to_string_lossy().into_owned());
    assert!(
        program.diagnostics.iter().any(|diagnostic| diagnostic
            .diagnostic
            .summary
            .contains("non-exhaustive matched, missing pattern: `Black`")),
        "{:?}",
        program.diagnostics
    );
}

#[test]
fn uses_cross_module_public_union() {
    let root = temp_dir("uses_cross_module_public_union");
    write(
        &root.join("main.nia"),
        r#"
module bits;
using entry::bits;

fn main() i32 {
    let mut value = bits::Bits { i: 7 };
    value.i
}
"#,
    );
    write(
        &root.join("bits.nia"),
        r#"pub union Bits { i: i32, f: f32 }"#,
    );

    let program = check_program(root.join("main.nia").to_string_lossy().into_owned());
    assert_no_error_diagnostics(&program.diagnostics);
}

#[test]
fn rejects_cross_module_nia_types_at_extern_abi_boundaries() {
    let root = temp_dir("rejects_cross_module_nia_types_at_extern_abi_boundaries");
    write(
        &root.join("main.nia"),
        r#"
module types;
using entry::types;

extern fn bad_struct(point: types::Point);
extern fn bad_union(bits: types::Bits);
extern fn bad_enum(color: types::Color);
"#,
    );
    write(
        &root.join("types.nia"),
        r#"
pub struct Point { x: i32 }
pub union Bits { i: i32 }
pub enum Color: u8 { Red }
"#,
    );

    let program = check_program(root.join("main.nia").to_string_lossy().into_owned());
    for expected in [
        "normal Nia struct by value",
        "union by value",
        "enum directly",
    ] {
        assert!(
            program
                .diagnostics
                .iter()
                .any(|diagnostic| diagnostic.diagnostic.summary.contains(expected)),
            "{expected}: {:?}",
            program.diagnostics
        );
    }
}

#[test]
fn rejects_cross_module_aliases_to_invalid_extern_abi_types() {
    let root = temp_dir("rejects_cross_module_aliases_to_invalid_extern_abi_types");
    write(
        &root.join("main.nia"),
        r#"
module types;
using entry::types;

extern fn bad_flag(flag: types::Flag);
extern fn bad_generic(flag: types::Identity[bool]);
"#,
    );
    write(
        &root.join("types.nia"),
        r#"
pub type Flag = bool;
pub type Identity[T] = T;
"#,
    );

    let program = check_program(root.join("main.nia").to_string_lossy().into_owned());
    assert_eq!(
        program
            .diagnostics
            .iter()
            .filter(|diagnostic| diagnostic
                .diagnostic
                .summary
                .contains("cannot use `bool` directly"))
            .count(),
        2,
        "{:?}",
        program.diagnostics
    );
}
