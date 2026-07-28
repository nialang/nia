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
        let mode = manifest.required("mode");
        manifest.expect("resource", TestWorkload::Build.as_str());
        let source = case_root.join(fixture_relative_path(
            &manifest_path,
            manifest.required("source"),
        ));
        match mode.as_str() {
            "linker-selection-errors" => {
                let reserved_error = manifest.required("reserved-error");
                let missing_error = manifest.required("missing-error");
                let bare_runtime_error = manifest.required("bare-runtime-error");
                manifest.finish();
                run_selection_errors(
                    &source,
                    &reserved_error,
                    &missing_error,
                    &bare_runtime_error,
                );
            }
            "linker-invocation" => {
                manifest.expect("platform", "unix");
                let raw = manifest.required_list("raw-args");
                let structured = manifest.required_list("structured-args");
                manifest.finish();
                run_invocation(&source, &raw, &structured);
            }
            _ => panic!(
                "unknown linker case mode {mode:?} in {}",
                manifest_path.display()
            ),
        }
    }
}

fn run_selection_errors(
    source: &Path,
    reserved_error: &str,
    missing_error: &str,
    bare_runtime_error: &str,
) {
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

    let bare = Command::new(env!("CARGO_BIN_EXE_nia"))
        .arg("emit")
        .arg("--exe")
        .arg(source)
        .arg("--runtime")
        .arg("bare")
        .arg("-o")
        .arg(&output)
        .output_timeout_in_session("run bare executable runtime case");
    assert_error(&bare, bare_runtime_error);
}

#[cfg(unix)]
fn run_invocation(source: &Path, raw: &[String], structured: &[String]) {
    use std::os::unix::fs::PermissionsExt;

    let root = std::env::temp_dir().join(format!("nia-linker-invocation-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&root);
    std::fs::create_dir_all(&root).expect("create linker invocation directory");
    let linker = root.join("linker.sh");
    let args_log = root.join("linker.args");
    std::fs::write(
        &linker,
        format!(
            "#!/bin/sh\nprintf '%s\\n' \"$@\" > '{}'\nexit 0\n",
            args_log.display()
        ),
    )
    .expect("write mock linker");
    let mut permissions = std::fs::metadata(&linker)
        .expect("mock linker metadata")
        .permissions();
    permissions.set_mode(0o755);
    std::fs::set_permissions(&linker, permissions).expect("make mock linker executable");

    let executable = root.join("raw-main");
    let raw_output = Command::new(env!("CARGO_BIN_EXE_nia"))
        .env("NIA_LINKER", &linker)
        .arg("emit")
        .arg("--exe")
        .arg(source)
        .arg("--link-arg")
        .arg("-lc")
        .arg("--link-arg=-lm")
        .arg("--link-arg")
        .arg("-Olinker")
        .arg("-o")
        .arg(&executable)
        .output_timeout_in_session("run raw linker arguments case");
    assert_success(&raw_output);
    assert_args(&args_log, raw);

    let structured_output = Command::new(env!("CARGO_BIN_EXE_nia"))
        .env("NIA_LINKER", &linker)
        .arg("emit")
        .arg("--exe")
        .arg(source)
        .arg("--dynamic-linker")
        .arg("/loader")
        .arg("--linker")
        .arg(&linker)
        .arg("--linker-flavor")
        .arg("lld")
        .arg("-L")
        .arg("/native/lib")
        .arg("-l")
        .arg("native_api")
        .arg("--rpath")
        .arg("$ORIGIN")
        .arg("-o")
        .arg(root.join("structured-main"))
        .output_timeout_in_session("run structured linker arguments case");
    assert_success(&structured_output);
    assert_args(&args_log, structured);
}

#[cfg(not(unix))]
fn run_invocation(_source: &Path, _raw: &[String], _structured: &[String]) {}

fn assert_args(path: &Path, expected: &[String]) {
    let args = std::fs::read_to_string(path).expect("read mock linker arguments");
    for expected in expected {
        assert!(
            args.lines().any(|arg| arg == expected),
            "missing {expected:?} in {args}"
        );
    }
}

fn assert_success(output: &std::process::Output) {
    assert!(
        output.status.success(),
        "stderr:\n{}",
        String::from_utf8_lossy(&output.stderr)
    );
}

fn assert_error(output: &std::process::Output, expected: &str) {
    assert!(!output.status.success());
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        stderr.contains(expected),
        "missing {expected:?} in {stderr}"
    );
}
