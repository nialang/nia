// SPDX-License-Identifier: GPL-3.0-or-later
use std::{path::Path, process::Command};

#[allow(dead_code, unused_imports)]
mod support;

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

        let mut command = support::nia_command();
        command.arg("build");
        let command_root = if mode == "configured-build-success" {
            command.arg("--timings=detail").arg("--timings-format=json");
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
            .output_timeout_without_resources("run build metadata case");

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
                    &workspace,
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
            "timings-json",
            "runner-context",
            "configured-output",
            "module-imports",
            "build-plan",
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
    let json_lines = stderr
        .lines()
        .filter(|line| line.starts_with('{') && line.contains("\"schema_version\":1"))
        .collect::<Vec<_>>();
    assert!(!json_lines.is_empty(), "{stderr}");
    assert!(
        json_lines
            .iter()
            .any(|line| line.contains("\"build.runner_executions\":1")),
        "{stderr}"
    );
    assert!(
        json_lines
            .iter()
            .any(|line| line.contains("\"llvm.units\"")),
        "{stderr}"
    );
    assert!(
        json_lines
            .iter()
            .any(|line| line.contains("\"build.steps_executed\":1")),
        "{stderr}"
    );
    assert!(
        json_lines
            .iter()
            .any(|line| line.contains("\"build.actions_executed\":1")),
        "{stderr}"
    );
    assert!(
        !stderr.lines().any(|line| line.starts_with("error:")),
        "{stderr}"
    );
    assert!(workspace.join(".nia-build/runner").is_dir());
    assert!(workspace.join(".nia-cache").is_dir());
    assert!(!workspace.join(".nia-build/build-plan.draft").exists());
    let plan_path = workspace.join(".nia-build/build-plan.bin");
    let plan = nia_build::read_build_plan(&plan_path).expect("decode published build plan");
    assert_eq!(plan.schema_version(), nia_build::BUILD_PLAN_SCHEMA_VERSION);
    assert_eq!(plan.root_package().as_str(), "root");
    assert_eq!(plan.packages().len(), 1);
    assert_eq!(plan.modules().len(), 1);
    assert_eq!(plan.modules()[0].key.name(), "app");
    assert_eq!(
        plan.modules()[0].root_source.protocol_path(),
        "src/main.nia"
    );
    assert_eq!(plan.modules()[0].imports.len(), 1);
    assert_eq!(plan.modules()[0].imports[0].name, "helper");
    assert_eq!(
        plan.modules()[0].imports[0].path.protocol_path(),
        "deps/helper.nia"
    );
    assert_eq!(plan.artifacts().len(), 1);
    assert_eq!(plan.artifacts()[0].key.name(), "app");
    assert_eq!(plan.artifacts()[0].root_module.name(), "app");
    assert_eq!(plan.artifacts()[0].output.protocol_path(), "custom-app");
    assert_eq!(plan.actions().len(), 2);
    assert!(matches!(
        plan.actions()[0].kind,
        nia_build::ActionKind::CompilerEmit { .. }
    ));
    assert!(matches!(
        plan.actions()[1].kind,
        nia_build::ActionKind::CompilerCheck { .. }
    ));
    assert_eq!(plan.steps().len(), 2);
    assert_eq!(
        plan.default_step().map(nia_build::StepKey::name),
        Some("build")
    );
    assert_eq!(
        plan.selected_step().map(nia_build::StepKey::name),
        Some("build")
    );
    assert!(workspace.join(".nia-build/custom-app").is_file());
    assert!(!workspace.join(".nia-build/app").exists());
    assert_eq!(
        Command::new(workspace.join(".nia-build/custom-app"))
            .status_timeout("run configured build target")
            .code(),
        Some(0)
    );

    let check = support::nia_command()
        .arg("build")
        .arg("check")
        .arg("--root")
        .arg(workspace)
        .output_timeout_without_resources("run configured build check step");
    assert!(
        check.status.success(),
        "stderr:\n{}",
        String::from_utf8_lossy(&check.stderr)
    );
    assert!(!workspace.join(".nia-build/build-plan.draft").exists());
    let checked_plan = nia_build::read_build_plan(&plan_path).expect("decode replaced build plan");
    assert_eq!(
        checked_plan.selected_step().map(nia_build::StepKey::name),
        Some("check")
    );
    let checked_bytes = std::fs::read(&plan_path).expect("read checked plan bytes");
    let unknown = support::nia_command()
        .arg("build")
        .arg("does-not-exist")
        .arg("--root")
        .arg(workspace)
        .output_timeout_without_resources("run unknown step after published plan");
    assert!(!unknown.status.success());
    assert!(!workspace.join(".nia-build/build-plan.draft").exists());
    assert_eq!(
        std::fs::read(&plan_path).expect("read plan after rejected build"),
        checked_bytes,
        "a failed runner must not replace the last canonical plan"
    );
}

fn assert_dependency_success(contract: &str, workspace: &Path, output: &std::process::Output) {
    assert!(
        output.status.success(),
        "stderr:\n{}",
        String::from_utf8_lossy(&output.stderr)
    );
    assert!(workspace.join(".nia-build/build-plan.bin").is_file());
    assert!(!workspace.join(".nia-build/build-plan.draft").exists());
    match contract {
        "step-order" => assert!(output.stdout.is_empty()),
        "executable-dependency" => {
            assert!(output.stdout.is_empty());
            assert!(workspace.join(".nia-build/app").is_file());
        }
        _ => panic!("unknown dependency-success contract {contract:?}"),
    }
}

fn assert_runner_error(
    contract: &str,
    runner_status: i32,
    workspace: &Path,
    forbidden: &Option<std::path::PathBuf>,
    output: &std::process::Output,
) {
    assert!(!output.status.success());
    assert!(!workspace.join(".nia-build/build-plan.draft").exists());
    assert!(!workspace.join(".nia-build/build-plan.bin").exists());
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
