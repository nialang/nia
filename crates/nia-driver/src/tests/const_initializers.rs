// SPDX-License-Identifier: GPL-3.0-or-later
//! Who reports what about a constant initializer: body checking owns its
//! static rules, global and local alike; const evaluation reports what
//! executing it finds; a use of a constant that failed reports nothing more.
use super::common::*;

/// The error summaries and source files a check of `entry` reports.
fn errors(path: &std::path::Path) -> Vec<(String, String)> {
    check_program(path.to_string_lossy().into_owned())
        .diagnostics
        .iter()
        .filter(|d| d.diagnostic.severity() == nia_diagnostic::Severity::Error)
        .map(|d| {
            let file = std::path::Path::new(d.path.as_str())
                .file_name()
                .map(|name| name.to_string_lossy().into_owned())
                .unwrap_or_default();
            (file, d.diagnostic.summary.clone())
        })
        .collect()
}

fn error_summaries(name: &str, source: &str) -> Vec<String> {
    let root = temp_dir(name);
    let path = root.join("main.nia");
    write(&path, source);
    errors(&path)
        .into_iter()
        .map(|(_, summary)| summary)
        .collect()
}

/// A global and a local initializer are typed by the same rules and report
/// the same error once, worded for a const initializer.
#[test]
fn global_and_local_initializers_share_one_typing() {
    for (name, source) in [
        (
            "global_literal_range",
            "const K: u8 = 300;\nfun main() i32 { K as i32 }\n",
        ),
        (
            "local_literal_range",
            "fun main() i32 {\n    const K: u8 = 300;\n    K as i32\n}\n",
        ),
    ] {
        assert_eq!(
            error_summaries(name, source),
            ["integer literal 300 is out of range for u8 in const initializer"],
            "{name}"
        );
    }
    for (name, source) in [
        (
            "global_mismatch",
            "const K: bool = 1;\nfun main() i32 { 0 }\n",
        ),
        (
            "local_mismatch",
            "fun main() i32 {\n    const K: bool = 1;\n    0\n}\n",
        ),
    ] {
        let summaries = error_summaries(name, source);
        assert_eq!(summaries.len(), 1, "{name}: {summaries:?}");
        assert!(
            summaries[0].contains("const initializer"),
            "{name}: {summaries:?}"
        );
    }
}

/// A static rule that body checking reports is not reported again by const
/// analysis checking the same initializer; what evaluating it finds is.
#[test]
fn rejected_initializers_keep_only_their_evaluation_findings() {
    assert_eq!(
        error_summaries(
            "rejected_static_rule",
            "trait Marker {}\nconst fun constrained[T]() usize where T: Marker { 7usize }\n\
             const WIDTH: usize = constrained[i32]();\nfun main() i32 { 0 }\n",
        ),
        ["trait bound not satisfied: i32: Marker"]
    );
    let summaries = error_summaries(
        "rejected_with_evaluation_finding",
        "const K: bool = 1usize / 0usize;\nfun main() bool { K }\n",
    );
    assert_eq!(summaries.len(), 2, "{summaries:?}");
    assert!(summaries.iter().any(|s| s.contains("type mismatch")));
    assert!(summaries.iter().any(|s| s.contains("division by zero")));
}

/// A use of a constant that failed, as a repeat count, a length or an
/// operand, adds no error to the failure's root.
#[test]
fn uses_of_a_failed_constant_report_nothing_further() {
    assert_eq!(
        error_summaries(
            "failed_by_evaluation",
            "const n: usize = 1 / 0;\nfun main() i32 {\n    let values: [i32; n] = [0; n];\n    \
             values.len() as i32\n}\n",
        ),
        ["division by zero in const expression"]
    );
    assert_eq!(
        error_summaries(
            "failed_by_static_rule",
            "const fun zero[T]() usize { 0 }\nconst n: usize = zero();\nfun main() i32 {\n    \
             let values: [i32; n] = [0; n];\n    values.len() as i32\n}\n",
        ),
        ["cannot infer generic parameter `T`"]
    );
}

/// What checking another module's initializer finds is located in that
/// module's file, whichever module's check found it.
#[test]
fn foreign_initializer_errors_are_located_in_their_file() {
    let root = temp_dir("foreign_initializer_errors");
    write(
        &root.join("config.nia"),
        "pub struct Packet { tag: u8 }\npub const packet: Packet = Packet { tag: 1u16 };\n",
    );
    let main = root.join("main.nia");
    write(
        &main,
        "module config;\nusing root::config;\nconst tag: u8 = config::packet.tag;\n\
         fun main() i32 { tag as i32 }\n",
    );
    let errors = errors(&main);
    assert!(!errors.is_empty());
    for (file, summary) in &errors {
        assert_eq!(file, "config.nia", "{summary}");
    }
}

/// A call made while evaluating a constant runs at compile time: it is no
/// instance of the program, for a local constant as for a global one.
#[test]
fn const_only_calls_instantiate_nothing_at_runtime() {
    let root = temp_dir("const_only_calls");
    let path = root.join("main.nia");
    write(
        &path,
        r#"
const fun width[T]() usize
where T: Sized {
    std::builtin::size[T]()
}

const GLOBAL: usize = width[u16]();

fun main() i32 {
    const LOCAL: usize = width[u64]();
    (GLOBAL + LOCAL) as i32
}
"#,
    );
    let program = codegen_program(path.to_string_lossy().into_owned());
    assert_no_error_diagnostics(&program.diagnostics);
    let width = sym("width");
    let instances = program
        .backend_lowering
        .program
        .modules
        .iter()
        .flat_map(|module| &module.function_instances)
        .filter(|instance| instance.name == width)
        .count();
    assert_eq!(instances, 0);
}

/// `ptrMut` exists on any array place; one that is not writable is reported
/// as such, once, by body checking, at run time and in a `const fun` alike.
#[test]
fn unwritable_slice_pointer_receivers_are_reported_once() {
    for (name, source) in [
        (
            "runtime_let_receiver",
            "fun main() i32 {\n    let values: [usize; 2] = [1, 2];\n    let p = values.ptrMut();\n    0\n}\n",
        ),
        (
            "const_fun_let_receiver",
            "const fun invalid() usize {\n    let values: [usize; 2] = [1, 2];\n    \
             let p = values.ptrMut();\n    0\n}\nconst result: usize = invalid();\n\
             fun main() i32 { 0 }\n",
        ),
    ] {
        assert_eq!(
            error_summaries(name, source),
            ["receiver of slice method `ptrMut` is not assignable: local is let"],
            "{name}"
        );
    }
    assert!(
        error_summaries(
            "writable_receiver",
            "fun main() i32 {\n    let mut values: [usize; 2] = [1, 2];\n    \
             let p = values.ptrMut();\n    0\n}\n",
        )
        .is_empty()
    );
}

/// What executing another module's `const fun` finds is reported once, in
/// that module's file.
#[test]
fn foreign_const_fun_findings_are_located_in_their_file() {
    let root = temp_dir("foreign_const_fun_findings");
    write(
        &root.join("b.nia"),
        "pub const fun bad() usize {\n    let values: [usize; 2] = [1, 2];\n    \
         let p = values.ptrMut();\n    0\n}\n",
    );
    let main = root.join("main.nia");
    write(
        &main,
        "module b;\nusing root::b;\nconst x: usize = b::bad();\nfun main() i32 { x as i32 }\n",
    );
    // Checking every module reports `b`'s own error, and `main` reports it
    // in place of what executing the function found: one report entry.
    let program = check_program(main.to_string_lossy().into_owned());
    let items = program
        .diagnostics
        .iter()
        .map(|entry| ReportItem {
            path: entry.path.as_str(),
            diagnostic: &entry.diagnostic,
        })
        .collect::<Vec<_>>();
    let report = nia_diagnostic::build_diagnostic_report(
        &items,
        nia_diagnostic::DiagnosticReportConfig::default(),
    );
    let reported = report
        .entries()
        .iter()
        .map(|item| {
            let path =
                nia_diagnostic::DiagnosticReportItem::report_source_path(*item).unwrap_or_default();
            let file = std::path::Path::new(path)
                .file_name()
                .map(|name| name.to_string_lossy().into_owned())
                .unwrap_or_default();
            (file, item.diagnostic.summary.clone())
        })
        .collect::<Vec<_>>();
    assert_eq!(
        reported,
        [(
            "b.nia".to_owned(),
            "receiver of slice method `ptrMut` is not assignable: local is let".to_owned()
        )]
    );
}

struct ReportItem<'a> {
    path: &'a str,
    diagnostic: &'a nia_diagnostic::Diagnostic,
}

impl nia_diagnostic::DiagnosticReportItem for ReportItem<'_> {
    fn report_diagnostic(&self) -> &nia_diagnostic::Diagnostic {
        self.diagnostic
    }

    fn report_path(&self) -> Option<&str> {
        Some(self.path)
    }
}

/// Const evaluation executes only what types. A failure found typing a call
/// made during evaluation is reported once, not again when executing it;
/// and an unselected branch that would fail is neither typed nor executed.
#[test]
fn evaluation_failures_found_typing_a_call_are_reported_once() {
    for (name, source) in [
        (
            "repeat_count_underflow",
            "const fun r[N: usize]() usize {\n    return r[N - 1]();\n}\n\
             fun main() usize { let a = [0u8; r[0]()]; a.len() }\n",
        ),
        (
            "initializer_underflow",
            "const fun r[N: usize]() usize {\n    return r[N - 1]();\n}\n\
             const K: usize = r[0]();\nfun main() usize { K }\n",
        ),
    ] {
        assert_eq!(
            error_summaries(name, source),
            ["integer overflow in const subtraction"],
            "{name}"
        );
    }
    assert!(
        error_summaries(
            "unselected_recursion",
            "const fun r[N: usize]() usize {\n    if N == 0 {\n        return 1;\n    }\n    \
             return r[N - 1]() + 1;\n}\nfun main() usize { let a = [0u8; r[20]()]; a.len() }\n",
        )
        .is_empty()
    );
}
