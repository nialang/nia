// SPDX-License-Identifier: GPL-3.0-or-later
use super::common::*;
use crate::{CheckRequest, Driver, DriverConfig};

#[test]
fn recovery_families_keep_independent_expression_errors() {
    let mut failures = Vec::new();
    for body in [
        "_ = missing(need(true));",
        "_ = (&missing)(need(true));",
        "_ = same(missing, need(true));",
        "_ = same((missing, 0), need(true));",
        "require(missing, true);",
        "require((missing, 0), true);",
        "_ = -missing; _ = need(true);",
        "_ = -(&missing); _ = need(true);",
        "_ = ~missing; _ = need(true);",
        "_ = ~(&missing); _ = need(true);",
        "_ = missing + need(true);",
        "_ = (&missing) + need(true);",
        "_ = missing << need(true);",
        "_ = (&missing) << need(true);",
        "_ = missing[need(true)];",
        "_ = (&missing)[need(true)];",
        "_ = missing.field; _ = need(true);",
        "_ = (&missing).field; _ = need(true);",
        "_ = missing.absent[i32](need(true));",
        "_ = (&missing).absent[i32](need(true));",
        "_ = missing.*; _ = need(true);",
        "_ = (&missing).*; _ = need(true);",
        "_ = &missing; _ = need(true);",
        "let (_, _) = missing; _ = need(true);",
        "let (_, _) = (missing, 0); _ = need(true);",
        "missing; _ = need(true);",
        "(missing, 0); _ = need(true);",
        "defer missing; _ = need(true);",
        "defer &missing; _ = need(true);",
        "for item in missing { _ = item; _ = need(true); }",
        "for item in (missing, 0) { _ = item; _ = need(true); }",
        "_ = ?missing; _ = need(true);",
        "_ = ?(&missing); _ = need(true);",
        "_ = missing.?; _ = need(true);",
        "_ = (?missing).?; _ = need(true);",
        "_ = std::builtin::extract(missing, need(true));",
        "_ = std::builtin::extract(&missing, need(true));",
        "_ = std::builtin::loadUnaligned[i32](missing); _ = need(true);",
        "_ = std::builtin::loadUnaligned[i32](&missing); _ = need(true);",
        "_ = missing.len(); _ = need(true);",
        "_ = (&missing).len(); _ = need(true);",
        "std::builtin::atomicStore[i32](missing, need(true), 0usize);",
        "std::builtin::atomicStore[i32](&missing, need(true), 0usize);",
        "_ = std::builtin::asm(.{ code: missing, inputs: .{ rax: need(true) } });",
        "_ = std::builtin::asm(.{ code: b\"\", inputs: .{ rax: &missing, rbx: need(true) } });",
        "let closure = \\ -> { _ = missing; need(true) }; _ = closure;",
        "let closure = \\ -> { _ = &missing; need(true) }; _ = closure;",
        "if missing { _ = need(true); }",
        "if (&missing) { _ = need(true); }",
    ] {
        check_recovery_pair(body, &mut failures);
    }
    for target in ["missing", "(missing, 0)"] {
        for pattern in [
            "?value",
            "!value",
            "value!",
            "(value, _)",
            "&value",
            "null",
            "0..3",
            "Point { value }",
            "Color::Red",
        ] {
            check_recovery_pair(
                &format!("match {target} {{ {pattern} => {{ _ = need(true); }} }}"),
                &mut failures,
            );
        }
    }
    assert!(failures.is_empty(), "{}", failures.join("\n"));
}

fn check_recovery_pair(body: &str, failures: &mut Vec<String>) {
    let source = format!(
        "fn need(value: i32) i32 {{ value }}\nfn same[T](a: T, b: T) T {{ _ = b; a }}\ntrait Marker {{}}\nfn require[T](value: T, fixed: i32) () where T: Marker {{ _ = value; _ = fixed; }}\nstruct Point {{ value: i32 }}\nenum Color {{ Red }}\nfn inspect() () {{ {body} }}\n"
    );
    let driver = test_driver();
    driver
        .set_source("main.nia", source.clone())
        .expect("source");
    let checked =
        checked_program_from_output(driver.check_all_modules(CheckRequest::new("main.nia")));
    let summaries = checked
        .diagnostics
        .iter()
        .map(|item| item.diagnostic.summary.as_str())
        .collect::<Vec<_>>();
    if summaries.len() != 2
        || summaries
            .iter()
            .filter(|summary| summary.contains("unknown value `missing`"))
            .count()
            != 1
        || summaries
            .iter()
            .filter(|summary| summary.contains("expected i32, got bool"))
            .count()
            != 1
    {
        failures.push(format!("{body}: {summaries:?}"));
    }
    for item in &checked.diagnostics {
        if item.diagnostic.summary.contains("expected i32, got bool") {
            let span = item
                .diagnostic
                .primary_span()
                .expect("independent error span");
            assert_eq!(&source[span.start..span.end], "true", "{body}");
        }
    }
}

#[test]
fn recovery_owner_boundaries_keep_independent_body_errors() {
    let mut failures = Vec::new();
    for declaration in [
        "fn inspect(value: Missing) () { _ = value; _ = need(true); }",
        "fn inspect(value: &Missing) () { _ = value; _ = need(true); }",
        "fn inspect() Missing { _ = need(true); 0 }",
        "fn inspect() &Missing { _ = need(true); &0 }",
        "fn inspect() i32 { _ = need(true); return missing; }",
        "fn inspect() &i32 { _ = need(true); return &missing; }",
        "fn inspect() () { let closure = \\ value: Missing -> { _ = value; need(true) }; _ = closure; }",
        "fn inspect() () { let closure = \\ value: &Missing -> { _ = value; need(true) }; _ = closure; }",
        "fn inspect() i32!i32 { _ = need(true); !missing }",
        "fn inspect() i32!&i32 { _ = need(true); !(&missing) }",
        "fn inspect() i32!i32 { _ = need(true); missing! }",
        "fn inspect() (&i32)!i32 { _ = need(true); (&missing)! }",
        "fn inspect() i32!i32 { _ = need(true); let value: i32!i32 = !missing; _ = value.?; !0 }",
    ] {
        let source = format!("const fn need(value: i32) i32 {{ value }}\n{declaration}\n");
        let driver = test_driver();
        driver
            .set_source("main.nia", source.clone())
            .expect("source");
        let checked =
            checked_program_from_output(driver.check_all_modules(CheckRequest::new("main.nia")));
        let summaries = checked
            .diagnostics
            .iter()
            .map(|item| item.diagnostic.summary.as_str())
            .collect::<Vec<_>>();
        if summaries.len() != 2
            || summaries
                .iter()
                .filter(|summary| {
                    summary.contains("unknown value `missing`")
                        || summary.contains("unknown type `Missing`")
                })
                .count()
                != 1
            || summaries
                .iter()
                .filter(|summary| summary.contains("expected i32, got bool"))
                .count()
                != 1
        {
            failures.push(format!("{declaration}: {summaries:?}"));
        }
    }
    assert!(failures.is_empty(), "{}", failures.join("\n"));
}

#[test]
fn global_recovery_keeps_independent_initializer_errors() {
    for kind in ["const", "static"] {
        for declaration in [
            "VALUE: Missing = need(true);",
            "VALUE: (Missing, i32) = (0, need(true));",
            "VALUE: (i32, i32) = (missing, need(true));",
        ] {
            let source =
                format!("const fn need(value: i32) i32 {{ value }}\n{kind} {declaration}\n");
            let driver = test_driver();
            driver
                .set_source("main.nia", source.clone())
                .expect("source");
            let checked = checked_program_from_output(
                driver.check_all_modules(CheckRequest::new("main.nia")),
            );
            for (message, text) in [
                (
                    if declaration.contains("Missing") {
                        "unknown type `Missing`"
                    } else {
                        "unknown value `missing`"
                    },
                    if declaration.contains("Missing") {
                        "Missing"
                    } else {
                        "missing"
                    },
                ),
                ("expected i32, got bool", "true"),
            ] {
                let diagnostics = checked
                    .diagnostics
                    .iter()
                    .filter(|item| item.diagnostic.summary.contains(message))
                    .collect::<Vec<_>>();
                assert_eq!(
                    diagnostics.len(),
                    1,
                    "{kind} {declaration}: {:?}",
                    checked.diagnostics
                );
                let span = diagnostics[0]
                    .diagnostic
                    .primary_span()
                    .expect("source span");
                assert_eq!(&source[span.start..span.end], text);
            }
        }
    }
}

#[test]
fn const_and_static_call_errors_keep_precise_argument_root() {
    for kind in ["const", "static"] {
        let source = format!(
            "const fn need(value: i32) i32 {{ value }}\n{kind} VALUE: Missing = need(true);\n"
        );
        let driver = test_driver();
        driver
            .set_source("main.nia", source.clone())
            .expect("source");
        let checked =
            checked_program_from_output(driver.check_all_modules(CheckRequest::new("main.nia")));
        let diagnostics = &checked.diagnostics;
        assert_eq!(
            diagnostics.len(),
            if kind == "static" { 3 } else { 2 },
            "{kind}: {diagnostics:?}"
        );
        assert!(diagnostics.iter().any(|item| {
            item.diagnostic.summary.contains("unknown type `Missing`")
                && item
                    .diagnostic
                    .primary_span()
                    .is_some_and(|span| &source[span.start..span.end] == "Missing")
        }));
        assert!(diagnostics.iter().any(|item| {
            item.diagnostic.summary.contains("expected i32, got bool")
                && item
                    .diagnostic
                    .primary_span()
                    .is_some_and(|span| &source[span.start..span.end] == "true")
        }));
        if kind == "static" {
            assert!(diagnostics.iter().any(|item| {
                item.diagnostic.summary
                    == "global initializer is not static data: function calls require const execution"
            }));
        }
    }
}

#[test]
fn static_call_restriction_is_independent_of_argument_types() {
    let driver = test_driver();
    driver
        .set_source(
            "main.nia",
            "const fn need(value: i32) i32 { value }\nstatic VALUE: i32 = need(1);\n",
        )
        .expect("source");
    let checked =
        checked_program_from_output(driver.check_all_modules(CheckRequest::new("main.nia")));
    assert_eq!(checked.diagnostics.len(), 1, "{:?}", checked.diagnostics);
    assert_eq!(
        checked.diagnostics[0].diagnostic.summary,
        "global initializer is not static data: function calls require const execution"
    );
}

#[test]
fn const_static_precedence_survives_edits_and_cache() {
    let root = temp_dir("const_static_precedence");
    let entry = root.join("main.nia");
    let entry_path = entry.to_string_lossy().into_owned();
    let cache = root.join("cache");
    let cached_driver = |verify_frontend_cache| {
        Driver::with_config(DriverConfig {
            artifact_cache_dir: Some(cache.clone()),
            verify_frontend_cache,
            ..DriverConfig::new(test_toolchain_layout())
        })
    };
    let incremental = cached_driver(false);
    for (declaration, expected) in [
        ("const VALUE: Missing = need(true);", 2),
        ("const VALUE: i32 = need(1);", 0),
        ("static VALUE: Missing = need(true);", 3),
        ("static VALUE: i32 = need(1);", 1),
        ("const VALUE: Missing = need(true);", 2),
    ] {
        let source = format!("const fn need(value: i32) i32 {{ value }}\n{declaration}\n");
        write(&entry, &source);
        incremental
            .set_source(entry_path.clone(), source.clone())
            .expect("edit");
        let check = |driver: &Driver| {
            checked_program_from_output(driver.check_all_modules(CheckRequest::new(&entry_path)))
        };
        let edited = check(&incremental);
        assert_eq!(
            edited.diagnostics.len(),
            expected,
            "{declaration}: {:?}",
            edited.diagnostics
        );
        assert!(
            edited.diagnostics.iter().all(|item| {
                item.diagnostic.summary != "const call argument does not match expected type"
                    && item.diagnostic.summary
                        != "global initializer is not representable as static data yet"
            }),
            "{declaration}: {:?}",
            edited.diagnostics
        );
        for other in [
            check(&test_driver()),
            check(&cached_driver(false)),
            check(&cached_driver(true)),
        ] {
            assert_eq!(edited.diagnostics, other.diagnostics, "{declaration}");
            assert_eq!(
                crate::render_program_diagnostics(&edited, None, None),
                crate::render_program_diagnostics(&other, None, None),
                "{declaration}"
            );
        }
    }
}

#[test]
fn specialized_const_roots_survive_report_precedence() {
    for (source, summary) in [
        (
            "trait Marker {}\nconst fn constrained[T]() usize where T: Marker { 7usize }\nconst WIDTH: usize = constrained[i32]();\n",
            "trait bound not satisfied for const function call",
        ),
        (
            "const PAYLOAD = std::builtin::embed(\"missing.bin\");\n",
            "failed to embed",
        ),
        (
            "const COUNT: u8 = 1u8 << 8u8;\n",
            "shift count is out of range in const expression",
        ),
    ] {
        let root = temp_dir("specialized_const_root");
        let entry = root.join("main.nia");
        write(&entry, source);
        let checked = checked_program_from_output(
            test_driver()
                .check_all_modules(CheckRequest::new(entry.to_string_lossy().into_owned())),
        );
        assert!(
            checked.diagnostics.iter().any(|item| {
                item.diagnostic.code.as_str() == "E0401"
                    && item.diagnostic.summary.contains(summary)
            }),
            "{source}: {:?}",
            checked.diagnostics
        );
    }
}

#[test]
fn aggregate_recovery_keeps_independent_initializer_errors() {
    for body in [
        "let value: Missing = .{ value: need(true) }; _ = value;",
        "let value: Box[Missing] = .{ value: need(true) }; _ = value;",
        "let value: &Missing = .{ value: need(true) }; _ = value;",
        "let value: [Missing; 1] = [need(true)]; _ = value;",
        "let value: Missing = [need(true)]; _ = value;",
        "let value: &Missing = [need(true)]; _ = value;",
        "let value: Missing = [.{ nested: need(true) }]; _ = value;",
        "let value: i32 = [need(true)]; _ = value;",
        "let value: (Missing, i32) = (0, need(true)); _ = value;",
        "let value: Missing = (.{ nested: need(true) }, 0); _ = value;",
        "let value: (Missing, i32) = (0, .{ nested: need(true) }); _ = value;",
        "let value: i32 = .{ value: need(true) }; _ = value;",
        "_ = .{ value: need(true) };",
        "let value: Missing = .{ value: .{ nested: need(true) } }; _ = value;",
    ] {
        let source = format!(
            "fn need(value: i32) i32 {{ value }}\nstruct Box[T] {{ value: T }}\nfn inspect() () {{ {body} }}\n"
        );
        let driver = test_driver();
        driver
            .set_source("main.nia", source.clone())
            .expect("source");
        let checked =
            checked_program_from_output(driver.check_all_modules(CheckRequest::new("main.nia")));
        let independent = checked
            .diagnostics
            .iter()
            .filter(|item| item.diagnostic.summary.contains("expected i32, got bool"))
            .collect::<Vec<_>>();
        assert_eq!(independent.len(), 1, "{body}: {:?}", checked.diagnostics);
        let span = independent[0]
            .diagnostic
            .primary_span()
            .expect("argument span");
        assert_eq!(&source[span.start..span.end], "true");
        if body.contains("Missing") {
            assert!(
                checked.diagnostics.iter().any(|item| {
                    item.diagnostic
                        .primary_span()
                        .is_some_and(|span| &source[span.start..span.end] == "Missing")
                }),
                "{body}: missing root: {:?}",
                checked.diagnostics
            );
            assert!(
                checked.diagnostics.iter().all(|item| {
                    !item.diagnostic.summary.contains("not nominal")
                        && !item.diagnostic.summary.contains("requires an expected")
                        && !item.diagnostic.summary.contains("not an array")
                }),
                "{body}: {:?}",
                checked.diagnostics
            );
        }
    }
}

#[test]
fn known_method_on_recovered_nominal_receiver_keeps_fixed_argument_error() {
    for receiver in ["Box[&Missing]", "Box[(Missing, i32)]"] {
        let source = format!(
            r#"
struct Box[T] {{ value: T }}
extend[T] Box[T] {{
    fn rank(self, value: i32) i32 {{ value }}
}}
fn inspect(value: {receiver}) () {{
    _ = value.rank(true);
}}
"#
        );
        let driver = test_driver();
        driver.set_source("main.nia", source).expect("source");
        let checked =
            checked_program_from_output(driver.check_all_modules(CheckRequest::new("main.nia")));
        let summaries = checked
            .diagnostics
            .iter()
            .map(|item| item.diagnostic.summary.as_str())
            .collect::<Vec<_>>();
        assert_eq!(
            summaries
                .iter()
                .filter(|summary| summary.contains("unknown type `Missing`"))
                .count(),
            1,
            "{summaries:?}"
        );
        assert_eq!(summaries.len(), 2, "{receiver}: {summaries:?}");
        assert_eq!(
            summaries
                .iter()
                .filter(|summary| summary.contains("expected i32, got bool"))
                .count(),
            1,
            "{summaries:?}"
        );
    }
}

#[test]
fn recovery_builtins_keep_independent_configuration_errors() {
    let mut failures = Vec::new();
    for (body, root, independent) in [
        (
            "_ = std::builtin::size[Missing](); _ = need(true);",
            "unknown type `Missing`",
            "expected i32, got bool",
        ),
        (
            "_ = std::builtin::size[&Missing](); _ = need(true);",
            "unknown type `Missing`",
            "expected i32, got bool",
        ),
        (
            "_ = std::builtin::align[(Missing, i32)](); _ = need(true);",
            "unknown type `Missing`",
            "expected i32, got bool",
        ),
        (
            "_ = std::builtin::atomicLoad[i32](missing, 3usize);",
            "unknown value `missing`",
            "atomic ordering `Release` is invalid for atomic load",
        ),
        (
            "_ = std::builtin::atomicLoad[i32](&missing, 3usize);",
            "unknown value `missing`",
            "atomic ordering `Release` is invalid for atomic load",
        ),
        (
            "_ = std::builtin::atomicRmw[i32](missing, 99usize, 0, 1usize);",
            "unknown value `missing`",
            "invalid atomic RMW operation `99`",
        ),
        (
            "_ = std::builtin::atomicRmw[i32](&missing, 99usize, 0, 1usize);",
            "unknown value `missing`",
            "invalid atomic RMW operation `99`",
        ),
        (
            "std::builtin::asm(.{ code: missing, options: [b\"unknown\"] });",
            "unknown value `missing`",
            "unknown `asm` option",
        ),
        (
            "std::builtin::asm(.{ code: b\"\", inputs: .{ rax: (missing, 0) }, options: [b\"unknown\"] });",
            "unknown value `missing`",
            "unknown `asm` option",
        ),
    ] {
        let source = format!("fn need(value: i32) i32 {{ value }}\nfn inspect() () {{ {body} }}\n");
        let driver = test_driver();
        driver.set_source("main.nia", source).expect("source");
        let checked =
            checked_program_from_output(driver.check_all_modules(CheckRequest::new("main.nia")));
        let summaries = checked
            .diagnostics
            .iter()
            .map(|item| item.diagnostic.summary.as_str())
            .collect::<Vec<_>>();
        if summaries.len() != 2
            || !summaries.iter().any(|summary| summary.contains(root))
            || !summaries
                .iter()
                .any(|summary| summary.contains(independent))
        {
            failures.push(format!("{body}: {summaries:?}"));
        }
    }
    assert!(failures.is_empty(), "{}", failures.join("\n"));
}

#[test]
fn nested_required_type_diagnostics_preserve_leaf_spans() {
    for ty in [
        "&Missing",
        "(Missing, i32)",
        "?Missing",
        "[Missing; 2]",
        "i32!Missing",
        "Missing!i32",
        "&fn(Missing) i32",
        "&fn(i32) Missing",
        "(Missing, Other)",
    ] {
        let source = format!("fn inspect() () {{ _ = std::builtin::size[{ty}](); }}");
        let driver = test_driver();
        driver
            .set_source("main.nia", source.clone())
            .expect("source");
        let checked =
            checked_program_from_output(driver.check_all_modules(CheckRequest::new("main.nia")));
        assert_eq!(
            checked.diagnostics.len(),
            if ty.contains("Other") { 2 } else { 1 },
            "{ty}: {:?}",
            checked.diagnostics
        );
        for item in &checked.diagnostics {
            assert_eq!(item.diagnostic.code.as_str(), "E0201", "{ty}: {item:?}");
            let span = item.diagnostic.primary_span().expect("leaf span");
            assert!(
                matches!(&source[span.start..span.end], "Missing" | "Other"),
                "{ty}: {item:?}"
            );
        }
    }
}

#[test]
fn nested_const_type_arguments_survive_edits_and_cache() {
    let root = temp_dir("nested_const_type_arguments");
    let entry = root.join("main.nia");
    let cache = root.join("cache");
    let cached_driver = |verify_frontend_cache| {
        Driver::with_config(DriverConfig {
            artifact_cache_dir: Some(cache.clone()),
            verify_frontend_cache,
            ..DriverConfig::new(test_toolchain_layout())
        })
    };
    let incremental = cached_driver(false);
    for ty in [
        "Buffer[COUNT]",
        "Wrap[Buffer[COUNT]]",
        "(Buffer[COUNT], &Missing)",
        "Buffer[COUNT]",
    ] {
        let source = format!(
            "const COUNT: usize = 2;\nstruct Buffer[N: usize] {{ values: [i32; N] }}\nstruct Wrap[T] {{ value: T }}\nfn inspect() () {{ _ = std::builtin::size[{ty}](); }}\n"
        );
        write(&entry, &source);
        incremental
            .set_source(entry.to_string_lossy().into_owned(), source.clone())
            .expect("edit");
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
            assert_eq!(edited.diagnostics, other.diagnostics, "{ty}");
            assert_eq!(
                crate::render_program_diagnostics(&edited, None, None),
                crate::render_program_diagnostics(&other, None, None),
                "{ty}"
            );
        }
        assert_eq!(
            edited.diagnostics.len(),
            usize::from(ty.contains("Missing")),
            "{ty}: {:?}",
            edited.diagnostics
        );
    }
}
