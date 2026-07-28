// SPDX-License-Identifier: GPL-3.0-or-later
use std::{path::Path, process::Command};

mod support;

use nia_test_support::{CaseManifest, TestWorkload, case_directories, fixture_relative_path};
use support::{CommandExt, CommandStatusExt, temp_dir};

#[test]
fn optimization_cases_match_expectations() {
    let _resources = nia_test_support::acquire_test_resources(TestWorkload::Build);
    let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/cases/optimization");
    for case_root in case_directories(&root, "optimization") {
        run_case(&case_root);
    }
}

fn run_case(case_root: &Path) {
    let mut manifest = CaseManifest::load(case_root);
    let manifest_path = manifest.path().to_owned();
    let mode = manifest.required("mode");
    manifest.expect("resource", TestWorkload::Build.as_str());
    let source = case_root.join(fixture_relative_path(
        &manifest_path,
        manifest.required("source"),
    ));
    let levels = manifest.required_list("levels");
    assert!(source.is_file(), "missing source {}", source.display());

    match mode.as_str() {
        "emit-execute-optimization" => {
            let exit_code = manifest.required_i32("exit-code");
            manifest.finish();
            run_emit_execute(&source, &levels, exit_code);
        }
        "emit-object-optimization" => {
            let minimum_bytes = manifest.required_usize("minimum-bytes");
            manifest.finish();
            run_emit_object(&source, &levels, minimum_bytes);
        }
        _ => panic!(
            "unknown optimization case mode {mode:?} in {}",
            manifest_path.display()
        ),
    }
}

fn run_emit_execute(source: &Path, levels: &[String], exit_code: i32) {
    let output_root = temp_dir("optimization_emit_execute");
    for level in levels {
        let executable = output_root.join(format!(
            "main_{}{}",
            level.trim_start_matches('-'),
            std::env::consts::EXE_SUFFIX
        ));
        let context = format!("run nia {level} emit --exe");
        let output = Command::new(env!("CARGO_BIN_EXE_nia"))
            .arg(level)
            .arg("emit")
            .arg("--exe")
            .arg(source)
            .arg("-o")
            .arg(&executable)
            .output_timeout_in_session(&context);
        assert!(
            output.status.success(),
            "{level} stderr:\n{}",
            String::from_utf8_lossy(&output.stderr)
        );

        let status = Command::new(&executable)
            .status_timeout(&format!("run emitted executable for {level}"));
        assert_eq!(status.code(), Some(exit_code), "{level}");
    }
}

fn run_emit_object(source: &Path, levels: &[String], minimum_bytes: usize) {
    let output_root = temp_dir("optimization_emit_object");
    for level in levels {
        let object = output_root.join(format!("main_{}.o", level.trim_start_matches('-')));
        let context = format!("run nia {level} emit --obj");
        let output = Command::new(env!("CARGO_BIN_EXE_nia"))
            .arg(level)
            .arg("emit")
            .arg("--obj")
            .arg(source)
            .arg("-o")
            .arg(&object)
            .output_timeout_in_session(&context);
        assert!(
            output.status.success(),
            "{level} stderr:\n{}",
            String::from_utf8_lossy(&output.stderr)
        );
        let bytes = std::fs::metadata(&object)
            .unwrap_or_else(|error| panic!("object metadata for {level}: {error}"))
            .len();
        assert!(
            bytes >= minimum_bytes as u64,
            "{level} produced {bytes} bytes, expected at least {minimum_bytes}"
        );
    }
}
