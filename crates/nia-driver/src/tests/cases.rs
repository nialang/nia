// SPDX-License-Identifier: GPL-3.0-or-later
use std::{fmt::Write, fs, path::Path};

use super::common::checked_program_from_output;

struct CheckSuite {
    name: &'static str,
    expects_errors: bool,
}

#[test]
fn check_cases_match_diagnostic_snapshots() {
    let _permit = nia_test_support::compiler_permit();
    let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/cases/check");
    let driver = crate::Driver::new();
    for suite in [
        CheckSuite {
            name: "pass",
            expects_errors: false,
        },
        CheckSuite {
            name: "fail",
            expects_errors: true,
        },
    ] {
        run_check_suite(&driver, &root, suite);
    }
}

fn run_check_suite(driver: &crate::Driver, root: &Path, suite: CheckSuite) {
    let mut cases = fs::read_dir(root.join(suite.name))
        .unwrap_or_else(|error| panic!("read {} cases: {error}", suite.name))
        .map(|entry| entry.expect("read case entry").path())
        .filter(|path| path.extension().is_some_and(|extension| extension == "nia"))
        .collect::<Vec<_>>();
    cases.sort();
    assert!(!cases.is_empty(), "{} suite must contain cases", suite.name);
    for source in cases {
        assert_check_case(driver, root, &source, suite.expects_errors);
    }
}

fn assert_check_case(driver: &crate::Driver, root: &Path, source: &Path, expects_errors: bool) {
    let program = checked_program_from_output(driver.check_entry(crate::CheckRequest::new(
        source.to_string_lossy().into_owned(),
    )));
    assert_eq!(
        nia_compiler_query::has_error_diagnostics(&program.diagnostics),
        expects_errors,
        "{} error classification mismatch\n{}",
        source.display(),
        diagnostic_snapshot(&program.diagnostics, root)
    );
    let snapshot_path = source.with_extension("snap");
    let expected = fs::read_to_string(&snapshot_path)
        .unwrap_or_else(|error| panic!("read {}: {error}", snapshot_path.display()));
    assert_eq!(
        diagnostic_snapshot(&program.diagnostics, root),
        expected,
        "{} diagnostic snapshot changed",
        source.display()
    );
}

fn diagnostic_snapshot(diagnostics: &[crate::ProgramDiagnostic], root: &Path) -> String {
    let mut records = Vec::new();
    for diagnostic in diagnostics {
        let mut record = String::new();
        let path = Path::new(diagnostic.path.as_str())
            .strip_prefix(root)
            .unwrap_or_else(|_| Path::new(diagnostic.path.as_str()));
        let _ = writeln!(record, "path: {}", path.display());
        let _ = writeln!(record, "code: {}", diagnostic.diagnostic.code.as_str());
        let _ = writeln!(record, "summary: {}", diagnostic.diagnostic.summary);
        for label in diagnostic.diagnostic.labels.iter() {
            let _ = writeln!(
                record,
                "label: {}..{} {:?} {:?} {}",
                label.span.start,
                label.span.end,
                label.style,
                label.span_source,
                label.message.as_deref().unwrap_or_default()
            );
        }
        for note in diagnostic.diagnostic.notes.iter() {
            let _ = writeln!(record, "note: {note}");
        }
        for help in diagnostic.diagnostic.help.iter() {
            let _ = writeln!(record, "help: {help}");
        }
        for related in diagnostic.diagnostic.related.iter() {
            let _ = writeln!(
                record,
                "related: {}..{} {}",
                related.span.start, related.span.end, related.message
            );
        }
        for field in diagnostic.diagnostic.debug.iter() {
            let _ = writeln!(record, "debug: {}={}", field.key, field.value);
        }
        records.push(record);
    }
    records.join("\n")
}
