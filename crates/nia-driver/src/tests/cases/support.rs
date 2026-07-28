// SPDX-License-Identifier: GPL-3.0-or-later
use std::{
    collections::BTreeMap,
    fmt::Write,
    fs,
    path::{Component, Path, PathBuf},
};

use crate::tests::common::checked_program_from_output;

pub(super) struct CaseManifest {
    pub(super) path: PathBuf,
    values: BTreeMap<String, String>,
}

pub(super) fn case_directories(root: &Path, suite: &str) -> Vec<PathBuf> {
    let mut cases = fs::read_dir(root)
        .unwrap_or_else(|error| panic!("read {} cases: {error}", root.display()))
        .map(|entry| entry.expect("read case entry").path())
        .collect::<Vec<_>>();
    cases.sort();
    assert!(!cases.is_empty(), "{suite} suite must contain cases");
    for case_root in &cases {
        assert!(
            case_root.is_dir(),
            "{suite} case {} must be a directory with case.meta",
            case_root.display()
        );
        assert!(
            case_root.join("case.meta").is_file(),
            "{suite} case {} must contain case.meta",
            case_root.display()
        );
    }
    cases
}

impl CaseManifest {
    pub(super) fn load(case_root: &Path) -> Self {
        let path = case_root.join("case.meta");
        let text = fs::read_to_string(&path)
            .unwrap_or_else(|error| panic!("read {}: {error}", path.display()));
        let mut values = BTreeMap::new();
        for (line_number, line) in text.lines().enumerate() {
            let line = line.trim();
            if line.is_empty() || line.starts_with('#') {
                continue;
            }
            let (key, value) = line.split_once('=').unwrap_or_else(|| {
                panic!(
                    "invalid {}:{}; expected key=value",
                    path.display(),
                    line_number + 1
                )
            });
            let key = key.trim();
            let value = value.trim();
            assert!(
                !key.is_empty() && !value.is_empty(),
                "empty key or value in {}:{}",
                path.display(),
                line_number + 1
            );
            assert!(
                values.insert(key.to_owned(), value.to_owned()).is_none(),
                "duplicate case key {key:?} in {}",
                path.display()
            );
        }
        Self { path, values }
    }

    pub(super) fn required(&mut self, key: &str) -> String {
        self.values
            .remove(key)
            .unwrap_or_else(|| panic!("{} must declare {key}", self.path.display()))
    }

    pub(super) fn expect(&mut self, key: &str, expected: &str) {
        let actual = self.required(key);
        assert_eq!(
            actual,
            expected,
            "{} must declare {key}={expected}",
            self.path.display()
        );
    }

    pub(super) fn required_usize(&mut self, key: &str) -> usize {
        let value = self.required(key);
        value.parse().unwrap_or_else(|error| {
            panic!(
                "{} must declare numeric {key}: {error}",
                self.path.display()
            )
        })
    }

    pub(super) fn required_i32(&mut self, key: &str) -> i32 {
        let value = self.required(key);
        value.parse().unwrap_or_else(|error| {
            panic!(
                "{} must declare numeric {key}: {error}",
                self.path.display()
            )
        })
    }

    pub(super) fn required_module_map(&mut self) -> BTreeMap<String, PathBuf> {
        let keys = self
            .values
            .keys()
            .filter(|key| key.starts_with("module."))
            .cloned()
            .collect::<Vec<_>>();
        let mut module_map = BTreeMap::new();
        for key in keys {
            let name = key
                .strip_prefix("module.")
                .expect("module map key prefix")
                .to_owned();
            assert!(
                !name.is_empty(),
                "{} contains an empty module map name",
                self.path.display()
            );
            let value = self.required(&key);
            let path = fixture_relative_path(&self.path, value);
            assert!(
                module_map.insert(name.clone(), path).is_none(),
                "{} contains duplicate module map name {name}",
                self.path.display()
            );
        }
        module_map
    }

    pub(super) fn finish(&self) {
        assert!(
            self.values.is_empty(),
            "unknown case keys in {}: {:?}",
            self.path.display(),
            self.values.keys().collect::<Vec<_>>()
        );
    }
}

pub(super) fn fixture_relative_path(manifest_path: &Path, value: String) -> PathBuf {
    let path = PathBuf::from(value);
    assert!(
        !path.is_absolute()
            && path
                .components()
                .all(|component| matches!(component, Component::Normal(_) | Component::CurDir)),
        "{} must use a relative fixture path without parent components",
        manifest_path.display()
    );
    path
}

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
