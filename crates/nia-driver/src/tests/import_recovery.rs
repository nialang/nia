// SPDX-License-Identifier: GPL-3.0-or-later
use super::common::*;
use crate::{CheckRequest, Driver, DriverConfig};
use nia_diagnostic::{decode_stable_diagnostic_bundle, encode_stable_diagnostic_bundle};
use std::path::Path;

#[test]
fn unavailable_import_module_keeps_the_loader_root() {
    for block in [false, true] {
        let root = temp_dir("unavailable_import_cause");
        let entry = root.join("main.nia");
        let consumer = root.join("consumer.nia");
        write(&entry, "module absent; module consumer; fn main() () {}\n");
        let source = r#"
using entry::absent::{Missing as Alias, missing as invoke};
fn need(value: i32) i32 { value }
fn inspect() () {
    let value: Alias = .{};
    _ = value;
    _ = invoke();
    _ = need(true);
}
"#;
        let source = if block {
            source
                .replace(
                    "using entry::absent::{Missing as Alias, missing as invoke};",
                    "",
                )
                .replace(
                    "fn inspect() () {",
                    "fn inspect() () { using entry::absent::{Missing as Alias, missing as invoke};",
                )
        } else {
            source.to_owned()
        };
        write(&consumer, &source);
        let checked = checked_program_from_output(
            test_driver()
                .check_all_modules(CheckRequest::new(entry.to_string_lossy().into_owned())),
        );
        let diagnostics = &checked.diagnostics;
        assert_eq!(
            diagnostics
                .iter()
                .filter(|item| item.diagnostic.code.as_str() == "E0102")
                .count(),
            1,
            "{diagnostics:?}"
        );
        assert!(
            !diagnostics
                .iter()
                .any(|item| item.diagnostic.summary.starts_with("`using ")),
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
        assert_eq!(
            diagnostics
                .iter()
                .filter(|item| item.diagnostic.cause.is_some())
                .count(),
            2,
            "{diagnostics:?}"
        );
        for item in diagnostics
            .iter()
            .filter(|item| item.diagnostic.cause.is_some())
        {
            let cause = item.diagnostic.cause.as_ref().unwrap();
            assert_eq!(
                diagnostics
                    .iter()
                    .filter(|root| {
                        root.path.as_str() == cause.source_path
                            && root.diagnostic.code.as_str() == cause.code
                            && root.diagnostic.primary_span() == Some(cause.span)
                    })
                    .count(),
                1,
                "{item:?}"
            );
        }
    }
}

#[test]
fn private_import_and_reexport_causes_identify_the_emitted_root() {
    for kind in ["module", "public", "block"] {
        let root = temp_dir("private_import_causes");
        let entry = root.join("main.nia");
        let api = root.join("api.nia");
        let consumer = root.join("consumer.nia");
        write(&entry, "pub module api; module consumer; fn main() () {}\n");
        write(&api, "struct Hidden {} fn hidden() i32 { 1 }\n");
        let directive = "using entry::api::{Hidden as Alias, hidden as invoke};";
        let body = "let value: Alias = .{}; _ = value; _ = invoke(); _ = invoke(); _ = need(true);";
        let source = match kind {
            "block" => format!("fn consume() () {{ {directive} {body} }}"),
            "public" => format!("pub {directive}\nfn consume() () {{ {body} }}"),
            _ => format!("{directive}\nfn consume() () {{ {body} }}"),
        };
        write(
            &consumer,
            &format!("{source}\nfn need(value: i32) i32 {{ value }}\n"),
        );
        let checked = checked_program_from_output(
            test_driver()
                .check_all_modules(CheckRequest::new(entry.to_string_lossy().into_owned())),
        );
        let diagnostics = &checked.diagnostics;
        assert_eq!(diagnostics.len(), 6, "{kind}: {diagnostics:?}");
        let linked = diagnostics
            .iter()
            .filter(|item| item.diagnostic.cause.is_some())
            .collect::<Vec<_>>();
        assert_eq!(linked.len(), 3, "{kind}: {diagnostics:?}");
        for item in linked {
            let cause = item.diagnostic.cause.as_ref().unwrap();
            let roots = diagnostics
                .iter()
                .filter(|root| {
                    root.path.as_str() == cause.source_path
                        && root.diagnostic.code.as_str() == cause.code
                        && root.diagnostic.primary_span() == Some(cause.span)
                        && root.diagnostic.cause.is_none()
                })
                .collect::<Vec<_>>();
            assert_eq!(roots.len(), 1, "{kind}: {item:?}");
            assert!(
                roots[0].diagnostic.related.iter().any(|related| {
                    related
                        .source_path
                        .as_deref()
                        .is_some_and(|path| Path::new(path) == api)
                }),
                "{kind}: {:?}",
                roots[0]
            );
        }
        let independent = diagnostics
            .iter()
            .find(|item| item.diagnostic.summary.contains("expected i32, got bool"))
            .unwrap();
        assert!(independent.diagnostic.cause.is_none());
    }
}

#[test]
fn aliased_group_import_causes_survive_edits_and_cache() {
    let root = temp_dir("aliased_group_import_causes");
    let entry = root.join("main.nia");
    let consumer = root.join("consumer.nia");
    write(&entry, "module api; module consumer; fn main() () {}\n");
    write(
        &root.join("api.nia"),
        "pub struct Present {} pub fn present() i32 { 1 }\n",
    );
    let failing = r#"using entry::api::{Missing as Alias, missing as invoke};
fn need(value: i32) i32 { value }
fn consume(value: Alias) Alias {
    _ = invoke();
    _ = invoke();
    _ = need(true);
    value
}
"#;
    let valid = failing
        .replace("Missing as", "Present as")
        .replace("missing as", "present as")
        .replace("need(true)", "need(1)");
    let shifted = format!("// moved import and use sites\n{failing}");
    let cache = root.join("cache");
    let cached_driver = |verify_frontend_cache| {
        Driver::with_config(DriverConfig {
            artifact_cache_dir: Some(cache.clone()),
            verify_frontend_cache,
            ..DriverConfig::new(test_toolchain_layout())
        })
    };
    let incremental = cached_driver(false);
    for (source, fails) in [(failing, true), (valid.as_str(), false), (&shifted, true)] {
        write(&consumer, source);
        incremental
            .set_source(consumer.to_string_lossy().into_owned(), source)
            .expect("edit importing module");
        let check = |driver: &Driver| {
            checked_program_from_output(
                driver.check_all_modules(CheckRequest::new(entry.to_string_lossy().into_owned())),
            )
        };
        let edited = check(&incremental);
        let clean = check(&test_driver());
        let warm = check(&cached_driver(false));
        let verified = check(&cached_driver(true));
        for other in [&clean, &warm, &verified] {
            assert_eq!(edited.diagnostics, other.diagnostics);
            assert_eq!(
                crate::render_program_diagnostics(&edited, None, None),
                crate::render_program_diagnostics(other, None, None)
            );
        }
        if !fails {
            assert_no_error_diagnostics(&edited.diagnostics);
            continue;
        }
        assert_eq!(edited.diagnostics.len(), 7, "{:?}", edited.diagnostics);
        let consequences = edited
            .diagnostics
            .iter()
            .filter(|item| item.diagnostic.cause.is_some())
            .collect::<Vec<_>>();
        assert_eq!(consequences.len(), 4, "{:?}", edited.diagnostics);
        for item in consequences {
            let cause = item.diagnostic.cause.as_ref().expect("cause");
            let roots = edited
                .diagnostics
                .iter()
                .filter(|root| {
                    root.path.as_str() == cause.source_path
                        && root.diagnostic.code.as_str() == cause.code
                        && root.diagnostic.primary_span() == Some(cause.span)
                        && root.diagnostic.cause.is_none()
                })
                .collect::<Vec<_>>();
            assert_eq!(roots.len(), 1, "{item:?}");
            let selected = if item.diagnostic.summary.contains("type `Alias`") {
                "Alias"
            } else {
                "invoke"
            };
            assert_eq!(cause.span.start, source.find(selected).expect("selector"));
            assert_eq!(&source[cause.span.start..cause.span.end], selected);
        }
        let independent = edited
            .diagnostics
            .iter()
            .find(|item| item.diagnostic.summary.contains("expected i32, got bool"))
            .unwrap_or_else(|| panic!("independent same-function error: {:?}", edited.diagnostics));
        assert!(independent.diagnostic.cause.is_none());
        let diagnostics = edited
            .diagnostics
            .iter()
            .map(|item| item.diagnostic.clone())
            .collect::<Vec<_>>();
        let encoded = encode_stable_diagnostic_bundle(&diagnostics, source.len())
            .expect("encode emitted causes");
        assert_eq!(
            decode_stable_diagnostic_bundle(&encoded, source.len()).expect("decode emitted causes"),
            diagnostics
        );
        let rendered = crate::render_program_diagnostics(&edited, None, None);
        assert_eq!(rendered.matches("related to root diagnostic").count(), 4);
    }
}
