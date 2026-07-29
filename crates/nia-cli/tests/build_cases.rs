// SPDX-License-Identifier: GPL-3.0-or-later
use std::{path::Path, process::Command};

use nia_test_support::{
    CaseManifest, CommandExt, CommandStatusExt, TestWorkload, case_directories, copy_case_tree,
    fixture_relative_path,
};

#[test]
fn build_cases_match_expectations() {
    let _resources = nia_test_support::acquire_test_resources(TestWorkload::Build);
    let fixtures = Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/cases/build");
    let workspaces = std::env::temp_dir().join(format!("nia-build-cases-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&workspaces);

    for case_root in case_directories(&fixtures, "build") {
        let mut manifest = CaseManifest::load(&case_root);
        let manifest_path = manifest.path().to_owned();
        let mode = manifest.required("mode");
        manifest.expect("resource", TestWorkload::Build.as_str());
        let contract = manifest.required("contract");
        let step = manifest.required("step");
        let workspace = workspaces.join(
            case_root
                .file_name()
                .expect("build case directory has a name"),
        );
        copy_case_tree(&case_root, &workspace);

        let mut command = Command::new(env!("CARGO_BIN_EXE_nia"));
        command.arg("build");
        let command_root = if mode == "configured-build-success" {
            command.arg("--timings");
            let nested = workspace.join("src/nested");
            std::fs::create_dir_all(&nested).expect("create nested build case directory");
            nested
        } else {
            workspace.clone()
        };
        if step != "default" {
            command.arg(&step);
        }
        let output = command
            .arg("--root")
            .arg(&command_root)
            .output_timeout_in_session("run build metadata case");

        match mode.as_str() {
            "dependency-success" => {
                manifest.finish();
                assert_dependency_success(&contract, &workspace, &output);
            }
            "runner-error" => {
                let runner_status = manifest.required_i32("runner-status");
                let forbidden = manifest.required("forbidden");
                manifest.finish();
                assert_runner_error(
                    &contract,
                    runner_status,
                    &fixture_path_or_none(&manifest_path, &workspace, forbidden),
                    &output,
                );
            }
            "missing-script-error" => {
                manifest.finish();
                assert!(!output.status.success());
                let stderr = String::from_utf8_lossy(&output.stderr);
                assert!(stderr.contains("failed to find `build.nia`"), "{stderr}");
                assert!(
                    stderr.contains(&workspace.to_string_lossy().to_string()),
                    "{stderr}"
                );
            }
            "configured-build-success" => {
                let contracts = manifest.required_list("contracts");
                manifest.finish();
                assert_configured_build_success(&contracts, &workspace, &output);
            }
            "module-check-success" => {
                manifest.finish();
                assert!(
                    output.status.success(),
                    "stderr:\n{}",
                    String::from_utf8_lossy(&output.stderr)
                );
            }
            _ => panic!(
                "unknown build case mode {mode:?} in {}",
                manifest_path.display()
            ),
        }
    }
}

fn assert_configured_build_success(
    contracts: &[String],
    workspace: &Path,
    output: &std::process::Output,
) {
    assert_eq!(
        contracts,
        [
            "timings",
            "runner-context",
            "configured-output",
            "module-imports",
        ]
    );
    assert!(
        output.status.success(),
        "stderr:\n{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let stdout = String::from_utf8_lossy(&output.stdout);
    let stderr = String::from_utf8_lossy(&output.stderr);
    for expected in [
        format!("root={}\n", workspace.display()),
        format!("build={}\n", workspace.join(".nia-build").display()),
        format!("cache={}\n", workspace.join(".nia-cache").display()),
        format!("toolchain={}\n", env!("CARGO_BIN_EXE_nia")),
    ] {
        assert!(
            stdout.contains(&expected),
            "missing {expected:?} in {stdout}"
        );
    }
    assert!(stderr.contains("timing"), "{stderr}");
    assert!(
        !stderr.lines().any(|line| line.starts_with("error:")),
        "{stderr}"
    );
    assert!(workspace.join(".nia-build/runner").is_dir());
    assert!(workspace.join(".nia-cache").is_dir());
    assert!(workspace.join(".nia-build/custom-app").is_file());
    assert!(!workspace.join(".nia-build/app").exists());
    assert_eq!(
        Command::new(workspace.join(".nia-build/custom-app"))
            .status_timeout("run configured build target")
            .code(),
        Some(0)
    );

    let check = Command::new(env!("CARGO_BIN_EXE_nia"))
        .arg("build")
        .arg("check")
        .arg("--root")
        .arg(workspace)
        .output_timeout_in_session("run configured build check step");
    assert!(
        check.status.success(),
        "stderr:\n{}",
        String::from_utf8_lossy(&check.stderr)
    );
}

fn assert_dependency_success(contract: &str, workspace: &Path, output: &std::process::Output) {
    assert!(
        output.status.success(),
        "stderr:\n{}",
        String::from_utf8_lossy(&output.stderr)
    );
    match contract {
        "step-order" => assert_eq!(
            String::from_utf8_lossy(&output.stdout),
            "prepare\nbuild\ncheck\n"
        ),
        "executable-dependency" => {
            assert_eq!(String::from_utf8_lossy(&output.stdout), "verified\n");
            assert!(workspace.join(".nia-build/app").is_file());
        }
        _ => panic!("unknown dependency-success contract {contract:?}"),
    }
}

fn assert_runner_error(
    contract: &str,
    runner_status: i32,
    forbidden: &Option<std::path::PathBuf>,
    output: &std::process::Output,
) {
    assert!(!output.status.success());
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(stderr.contains("build runner"), "{stderr}");
    assert!(
        stderr.contains(&format!("exit status: {runner_status}")),
        "{stderr}"
    );
    if contract == "unknown-step" {
        assert!(stderr.contains("unknown build step `missing`"), "{stderr}");
    }
    if let Some(forbidden) = forbidden {
        assert!(
            !forbidden.exists(),
            "{} must not exist",
            forbidden.display()
        );
    }
}

fn fixture_path_or_none(
    manifest_path: &Path,
    workspace: &Path,
    value: String,
) -> Option<std::path::PathBuf> {
    (value != "none").then(|| workspace.join(fixture_relative_path(manifest_path, value)))
}
