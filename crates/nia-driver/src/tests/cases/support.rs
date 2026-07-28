// SPDX-License-Identifier: GPL-3.0-or-later
use std::{fmt::Write, fs, path::Path};

use crate::tests::common::checked_program_from_output;

pub(super) fn case_expects_errors(manifest_path: &Path, key: &str, value: String) -> bool {
    match value.as_str() {
        "pass" => false,
        "fail" => true,
        _ => panic!(
            "{} must declare {key}=pass or {key}=fail",
            manifest_path.display()
        ),
    }
}

pub(super) fn copy_case_tree(source: &Path, destination: &Path) {
    fs::create_dir_all(destination)
        .unwrap_or_else(|error| panic!("create {}: {error}", destination.display()));
    for entry in
        fs::read_dir(source).unwrap_or_else(|error| panic!("read {}: {error}", source.display()))
    {
        let entry = entry.expect("read fixture entry");
        let source_path = entry.path();
        let destination_path = destination.join(entry.file_name());
        if source_path.is_dir() {
            copy_case_tree(&source_path, &destination_path);
        } else {
            fs::copy(&source_path, &destination_path).unwrap_or_else(|error| {
                panic!(
                    "copy fixture {} to {}: {error}",
                    source_path.display(),
                    destination_path.display()
                )
            });
        }
    }
}

pub(super) fn assert_check_case(
    driver: &crate::Driver,
    root: &Path,
    source: &Path,
    expects_errors: bool,
    snapshot_path: &Path,
) {
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
    let expected = fs::read_to_string(snapshot_path)
        .unwrap_or_else(|error| panic!("read {}: {error}", snapshot_path.display()));
    assert_eq!(
        diagnostic_snapshot(&program.diagnostics, root),
        expected,
        "{} diagnostic snapshot changed",
        source.display()
    );
}

pub(super) fn codegen_snapshot(program: &crate::CodegenProgram, root: &Path) -> String {
    let diagnostics = diagnostic_snapshot(&program.diagnostics, root);
    let mut snapshot = String::new();
    if diagnostics.is_empty() {
        snapshot.push_str("diagnostics: none\n");
    } else {
        snapshot.push_str("diagnostics:\n");
        snapshot.push_str(&diagnostics);
    }
    snapshot.push_str(&crate::optimization_report(program));
    snapshot
}

pub(super) fn diagnostic_snapshot(diagnostics: &[crate::ProgramDiagnostic], root: &Path) -> String {
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
