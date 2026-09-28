// SPDX-License-Identifier: GPL-3.0-or-later
use super::common::*;
use crate::{CheckRequest, Driver, DriverConfig};

#[test]
fn block_imports_failed_namespace_hides_a_declared_module() {
    let root = temp_dir("block_failed_namespace_shadow");
    let entry = root.join("main.nia");
    write(
        &root.join("api.nia"),
        "pub fn value() i32 { 1 } pub struct Point {}\n",
    );
    write(
        &entry,
        r#"
module api;
fn inspect() () {
    using entry::api::missing as api;
    _ = api::value();
    let value: api::Point = .{};
    _ = value;
}
"#,
    );
    let checked = checked_program_from_output(
        test_driver().check_all_modules(CheckRequest::new(entry.to_string_lossy().into_owned())),
    );
    assert_eq!(checked.diagnostics.len(), 3, "{:?}", checked.diagnostics);
    let root = checked
        .diagnostics
        .iter()
        .find(|item| item.diagnostic.cause.is_none())
        .unwrap();
    let linked = checked
        .diagnostics
        .iter()
        .filter_map(|item| item.diagnostic.cause.as_ref())
        .collect::<Vec<_>>();
    assert_eq!(linked.len(), 2, "{:?}", checked.diagnostics);
    for cause in linked {
        assert_eq!(cause.source_path, root.path.as_str());
        assert_eq!(cause.code, root.diagnostic.code.as_str());
        assert_eq!(Some(cause.span), root.diagnostic.primary_span());
    }
}

#[test]
fn block_imports_reach_types_values_consts_and_codegen() {
    let root = temp_dir("block_imports");
    let entry = root.join("main.nia");
    write(
        &root.join("api.nia"),
        r#"
pub struct Point { x: i32 }
pub enum Color { Red }
pub const WIDTH: usize = 2;
pub fn value() i32 { 7 }
"#,
    );
    write(
        &root.join("other.nia"),
        "pub fn value() bool { true } pub struct Point { flag: bool }\n",
    );
    write(
        &entry,
        r#"
module api;
module other;
struct Alias { wrong: bool }
enum Color { Blue }
fn get() bool { false }
const WIDTH: bool = false;
const fn width() usize { using entry::api::WIDTH; WIDTH }
fn array() [i32; width()] { [1, 2] }
pub fn main() i32 {
    using entry::api::{Point as Alias, Color, value as get, WIDTH};
    using Color::Red as Shade;
    using entry::api as model;
    let point: Alias = .{ x: get() };
    let color: Color = Shade;
    let qualified_color: Color = Color::Red;
    let values: [i32; WIDTH] = array();
    _ = color;
    _ = qualified_color;
    _ = values;
    {
        using entry::other::value as get;
        using entry::other as api;
        using api::value as through_alias;
        let flag: bool = get();
        let module_flag: bool = api::value();
        let imported_flag: bool = through_alias();
        let aliased_point: api::Point = .{ flag: true };
        _ = flag;
        _ = module_flag;
        _ = imported_flag;
        _ = aliased_point;
        _ = model::value();
    }
    let original: i32 = api::value();
    _ = original;
    let number: i32 = get();
    point.x + number
}
"#,
    );
    let driver = test_driver();
    let request = || CheckRequest::new(entry.to_string_lossy().into_owned());
    driver
        .check_all_modules(request())
        .result
        .expect("block imports check");
    driver
        .codegen(request())
        .result
        .expect("block imports generate code");
}

#[test]
fn block_imports_do_not_leak_and_keep_failed_alias_causes() {
    let root = temp_dir("block_import_causes");
    let entry = root.join("main.nia");
    write(&root.join("api.nia"), "pub fn value() i32 { 7 }\n");
    write(
        &entry,
        r#"
module api;
fn need(value: i32) i32 { value }
fn inspect() () {
    { using entry::api::value as nested; _ = nested(); }
    { _ = nested(); }
    _ = nested();
    {
        using entry::api::{Missing as Alias, missing as invoke};
        let value: Alias = .{};
        let nested_type: Alias::Member = .{};
        _ = value;
        _ = nested_type;
        _ = Alias::call();
        _ = invoke();
        _ = invoke();
        _ = need(true);
    }
}
"#,
    );
    let checked = checked_program_from_output(
        test_driver().check_all_modules(CheckRequest::new(entry.to_string_lossy().into_owned())),
    );
    let diagnostics = &checked.diagnostics;
    assert_eq!(
        diagnostics
            .iter()
            .filter(|item| item.diagnostic.summary.contains("unknown value `nested`"))
            .count(),
        2,
        "{diagnostics:?}"
    );
    assert_eq!(
        diagnostics
            .iter()
            .filter(|item| item.diagnostic.summary.contains("expected i32, got bool"))
            .count(),
        1,
        "{diagnostics:?}"
    );
    let consequences = diagnostics
        .iter()
        .filter(|item| item.diagnostic.cause.is_some())
        .collect::<Vec<_>>();
    assert_eq!(consequences.len(), 5, "{diagnostics:?}");
    for item in consequences {
        let cause = item.diagnostic.cause.as_ref().unwrap();
        assert_eq!(
            diagnostics
                .iter()
                .filter(|root| {
                    root.path.as_str() == cause.source_path
                        && root.diagnostic.code.as_str() == cause.code
                        && root.diagnostic.primary_span() == Some(cause.span)
                        && root.diagnostic.cause.is_none()
                })
                .count(),
            1,
            "{item:?}"
        );
    }
}

#[test]
fn block_imports_wildcards_and_duplicates_keep_namespace_boundaries() {
    let root = temp_dir("block_import_duplicates");
    let entry = root.join("main.nia");
    write(
        &root.join("api.nia"),
        "pub struct Item {} pub fn value() i32 { 1 } pub enum Color { Red, Blue }\n",
    );
    write(
        &entry,
        r#"
module api;
fn inspect() () {
    using entry::api::{Item as shared, value as shared};
    let item: shared = .{};
    _ = item;
    _ = shared();
    {
        using entry::api::*;
        using Color::*;
        let red: Color = Red;
        let blue: Color = Blue;
        _ = red;
        _ = blue;
        using entry::api::value;
    }
}
"#,
    );
    let checked = checked_program_from_output(
        test_driver().check_all_modules(CheckRequest::new(entry.to_string_lossy().into_owned())),
    );
    assert_eq!(checked.diagnostics.len(), 1, "{:?}", checked.diagnostics);
    assert!(
        checked.diagnostics[0]
            .diagnostic
            .summary
            .contains("duplicate using name `value`")
    );
    assert_eq!(checked.diagnostics[0].diagnostic.related.len(), 2);
}

#[test]
fn block_imports_causes_and_scope_edits_match_clean_and_cached_checks() {
    let root = temp_dir("block_import_edits");
    let entry = root.join("main.nia");
    write(
        &root.join("api.nia"),
        "pub struct Present {} pub fn present() i32 { 1 }\n",
    );
    let failing = r#"module api;
fn need(value: i32) i32 { value }
fn main() () {
    using entry::api::{Missing as Alias, missing as invoke};
    let value: Alias = .{};
    _ = value;
    _ = invoke();
    _ = invoke();
    _ = need(true);
}
"#;
    let valid = failing
        .replace("Missing as", "Present as")
        .replace("missing as", "present as")
        .replace("need(true)", "need(1)");
    let nested = valid
        .replace("    using entry", "    { using entry")
        .replace("present as invoke};", "present as invoke}; }");
    let shifted = format!("// move all import and consumer spans\n{failing}");
    let cache = root.join("cache");
    let cached_driver = |verify_frontend_cache| {
        Driver::with_config(DriverConfig {
            artifact_cache_dir: Some(cache.clone()),
            verify_frontend_cache,
            ..DriverConfig::new(test_toolchain_layout())
        })
    };
    let incremental = cached_driver(false);
    for (source, roots, consequences) in [
        (failing, 3, 3),
        (valid.as_str(), 0, 0),
        (nested.as_str(), 3, 0),
        (shifted.as_str(), 3, 3),
    ] {
        write(&entry, source);
        incremental
            .set_source(entry.to_string_lossy().into_owned(), source)
            .expect("edit block scope");
        let check = |driver: &Driver| {
            checked_program_from_output(
                driver.check_all_modules(CheckRequest::new(entry.to_string_lossy().into_owned())),
            )
        };
        let edited = check(&incremental);
        for other in [
            check(&test_driver()),
            check(&cached_driver(false)),
            check(&cached_driver(true)),
        ] {
            assert_eq!(edited.diagnostics, other.diagnostics);
            assert_eq!(
                crate::render_program_diagnostics(&edited, None, None),
                crate::render_program_diagnostics(&other, None, None)
            );
        }
        assert_eq!(
            edited.diagnostics.len(),
            roots + consequences,
            "{:?}",
            edited.diagnostics
        );
        let linked = edited
            .diagnostics
            .iter()
            .filter(|item| item.diagnostic.cause.is_some())
            .collect::<Vec<_>>();
        assert_eq!(linked.len(), consequences, "{:?}", edited.diagnostics);
        for item in linked {
            let cause = item.diagnostic.cause.as_ref().unwrap();
            assert_eq!(
                edited
                    .diagnostics
                    .iter()
                    .filter(|root| {
                        root.path.as_str() == cause.source_path
                            && root.diagnostic.code.as_str() == cause.code
                            && root.diagnostic.primary_span() == Some(cause.span)
                            && root.diagnostic.cause.is_none()
                    })
                    .count(),
                1,
                "{item:?}"
            );
        }
    }
}
