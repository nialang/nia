// SPDX-License-Identifier: GPL-3.0-or-later
use std::{path::Path, process::Command};

use nia_test_support::{
    CaseManifest, CommandExt, TestWorkload, case_directories, fixture_relative_path,
};

#[test]
fn linker_cases_match_expectations() {
    let _resources = nia_test_support::acquire_test_resources(TestWorkload::Build);
    let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/cases/linker");
    for case_root in case_directories(&root, "linker") {
        let mut manifest = CaseManifest::load(&case_root);
        let manifest_path = manifest.path().to_owned();
        manifest.expect("mode", "linker-selection-errors");
        manifest.expect("resource", TestWorkload::Build.as_str());
        let source = case_root.join(fixture_relative_path(
            &manifest_path,
            manifest.required("source"),
        ));
        let reserved_error = manifest.required("reserved-error");
        let missing_error = manifest.required("missing-error");
        manifest.finish();
        run_selection_errors(&source, &reserved_error, &missing_error);
    }
}

fn run_selection_errors(source: &Path, reserved_error: &str, missing_error: &str) {
    let output = std::env::temp_dir().join(format!("nia-linker-case-{}", std::process::id()));
    let reserved = Command::new(env!("CARGO_BIN_EXE_nia"))
        .arg("emit")
        .arg("--exe")
        .arg(source)
        .arg("--linker-flavor")
        .arg("self-hosted-elf")
        .arg("-o")
        .arg(&output)
        .output_timeout_in_session("run reserved linker flavor case");
    assert_error(&reserved, reserved_error);

    let missing = Command::new(env!("CARGO_BIN_EXE_nia"))
        .env("PATH", "")
        .env_remove("NIA_LLD")
        .arg("emit")
        .arg("--exe")
        .arg(source)
        .arg("--linker-flavor")
        .arg("lld")
        .arg("-o")
        .arg(&output)
        .output_timeout_in_session("run missing LLD case");
    assert_error(&missing, missing_error);
}

fn assert_error(output: &std::process::Output, expected: &str) {
    assert!(!output.status.success());
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        stderr.contains(expected),
        "missing {expected:?} in {stderr}"
    );
}
