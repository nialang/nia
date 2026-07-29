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
    assert!(source.is_file(), "missing source {}", source.display());

    match mode.as_str() {
        "emit-execute-optimization" => {
            let levels = manifest.required_list("levels");
            let exit_code = manifest.required_i32("exit-code");
            manifest.finish();
            run_emit_execute(&source, &levels, exit_code);
        }
        "emit-object-optimization" => {
            let levels = manifest.required_list("levels");
            let minimum_bytes = manifest.required_usize("minimum-bytes");
            manifest.finish();
            run_emit_object(&source, &levels, minimum_bytes);
        }
        "check-optimization-report" => {
            let levels = manifest.required_list("levels");
            manifest.finish();
            run_check_report(&source, &levels);
        }
        "emit-optimization-report" => {
            let backend_level = manifest.required("backend-level");
            let llvm_level = manifest.required("llvm-level");
            let object_level = manifest.required("object-level");
            manifest.finish();
            run_emit_reports(&source, &backend_level, &llvm_level, &object_level);
        }
        _ => panic!(
            "unknown optimization case mode {mode:?} in {}",
            manifest_path.display()
        ),
    }
}

fn run_check_report(source: &Path, levels: &[String]) {
    for level in levels {
        let output = Command::new(env!("CARGO_BIN_EXE_nia"))
            .arg(level)
            .arg("check")
            .arg(source)
            .arg("--opt-report")
            .output_timeout_without_resources(&format!("run nia {level} check --opt-report"));
        assert_success(level, &output);
        let stdout = String::from_utf8_lossy(&output.stdout);
        assert_contains(&stdout, "backend optimization report:", level);
        match level.as_str() {
            "-O2" => {
                for expected in [
                    "policy level=O2",
                    "inline=normal",
                    "specialize=normal",
                    "dedup_monomorphized_instances=true",
                    "prefer_size=false",
                    "llvm_codegen=default",
                    "llvm_size=default",
                    "enabled_module_passes=",
                    "inline-leaf-functions",
                    "remove-unused-functions",
                    "enabled_function_passes=",
                    "enabled_global_passes=simplify-static-init",
                    "changes=",
                    "remove-unused-local-bindings",
                    "global simplify-static-init",
                ] {
                    assert_contains(&stdout, expected, level);
                }
                assert!(!stdout.contains("changes=0"), "{level}: {stdout}");
            }
            "-O0" => {
                for expected in ["policy level=O0", "inline=never", "llvm_codegen=none"] {
                    assert_contains(&stdout, expected, level);
                }
            }
            "-Os" => {
                for expected in [
                    "policy level=Os",
                    "dedup_monomorphized_instances=true",
                    "prefer_size=true",
                    "llvm_codegen=default",
                    "llvm_size=small",
                ] {
                    assert_contains(&stdout, expected, level);
                }
            }
            _ => panic!("unsupported check optimization-report level {level}"),
        }
    }
}

fn run_emit_reports(source: &Path, backend_level: &str, llvm_level: &str, object_level: &str) {
    let backend = Command::new(env!("CARGO_BIN_EXE_nia"))
        .arg(backend_level)
        .arg("emit")
        .arg("--backend")
        .arg(source)
        .arg("--opt-report")
        .output_timeout_without_resources("run nia emit --backend --opt-report");
    assert_success(backend_level, &backend);
    assert_contains(
        &String::from_utf8_lossy(&backend.stdout),
        "BackendProgram",
        "backend stdout",
    );
    assert_report_on_stderr(
        &backend,
        backend_level,
        &[
            "llvm_codegen=less",
            "llvm_size=default",
            "enabled_module_passes=",
            "enabled_function_passes=",
            "changes=",
            "inline-leaf-functions",
        ],
    );

    let llvm = Command::new(env!("CARGO_BIN_EXE_nia"))
        .arg(llvm_level)
        .arg("emit")
        .arg("--llvm")
        .arg(source)
        .arg("--opt-report")
        .output_timeout_without_resources("run nia emit --llvm --opt-report");
    assert_success(llvm_level, &llvm);
    assert_contains(
        &String::from_utf8_lossy(&llvm.stdout),
        "define i32 @",
        "LLVM stdout",
    );
    assert_report_on_stderr(
        &llvm,
        llvm_level,
        &[
            "policy level=O1",
            "inline=small",
            "llvm_codegen=less",
            "llvm_size=default",
            "enabled_module_passes=",
            "enabled_function_passes=",
            "changes=",
            "inline-leaf-functions",
        ],
    );

    run_object_reports(source, object_level);
}

fn run_object_reports(source: &Path, level: &str) {
    let output_root = temp_dir("optimization_report_object");
    let placements = ["after-output", "before-source", "before-output"];
    for placement in placements {
        let object = output_root.join(format!("{placement}.o"));
        let mut command = Command::new(env!("CARGO_BIN_EXE_nia"));
        command.arg(level).arg("emit").arg("--obj");
        match placement {
            "after-output" => {
                command
                    .arg(source)
                    .arg("-o")
                    .arg(&object)
                    .arg("--opt-report");
            }
            "before-source" => {
                command
                    .arg("--opt-report")
                    .arg(source)
                    .arg("-o")
                    .arg(&object);
            }
            "before-output" => {
                command
                    .arg(source)
                    .arg("--opt-report")
                    .arg("-o")
                    .arg(&object);
            }
            _ => unreachable!(),
        }
        let output = command.output_timeout_without_resources(&format!(
            "run nia emit --obj --opt-report ({placement})"
        ));
        assert_success(placement, &output);
        assert_report_on_stderr(
            &output,
            placement,
            &["policy level=Os", "llvm_codegen=default", "llvm_size=small"],
        );
        assert!(
            std::fs::metadata(&object)
                .unwrap_or_else(|error| panic!("object metadata for {placement}: {error}"))
                .len()
                > 0,
            "{placement} produced an empty object"
        );
    }
}

fn assert_report_on_stderr(output: &std::process::Output, context: &str, expected: &[&str]) {
    let stdout = String::from_utf8_lossy(&output.stdout);
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        !stdout.contains("backend optimization report:"),
        "{context}: {stdout}"
    );
    assert_contains(&stderr, "backend optimization report:", context);
    for expected in expected {
        assert_contains(&stderr, expected, context);
    }
}

fn assert_success(context: &str, output: &std::process::Output) {
    assert!(
        output.status.success(),
        "{context} stderr:\n{}",
        String::from_utf8_lossy(&output.stderr)
    );
}

fn assert_contains(actual: &str, expected: &str, context: &str) {
    assert!(
        actual.contains(expected),
        "{context}: missing {expected:?} in {actual}"
    );
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
            .output_timeout_without_resources(&context);
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
            .output_timeout_without_resources(&context);
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
