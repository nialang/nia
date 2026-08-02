// SPDX-License-Identifier: GPL-3.0-or-later
use std::{path::Path, process::Command};

#[allow(dead_code, unused_imports)]
mod support;

use nia_test_support::{
    CaseManifest, CommandExt, CommandStatusExt, TestWorkload, case_directories, copy_case_tree,
    fixture_relative_path,
};

macro_rules! build_cases {
    ($($name:ident),+ $(,)?) => {
        const BUILD_CASE_NAMES: &[&str] = &[$(stringify!($name)),+];

        $(
            #[test]
            fn $name() {
                run_build_case(stringify!($name));
            }
        )+
    };
}

build_cases!(
    bare_runtime,
    configured_optimization,
    configured_success,
    dependency_cycle,
    duplicate_module,
    duplicate_target,
    executable_dependency,
    invalid_output,
    invalid_target,
    missing_default,
    missing_script,
    step_order,
    unknown_step,
    unselected_dependency_cycle,
);

#[test]
fn configured_build_fixtures_have_independent_tests() {
    let fixtures = Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/cases/build");
    let configured = case_directories(&fixtures, "build")
        .into_iter()
        .map(|path| {
            path.file_name()
                .expect("build case directory has a name")
                .to_string_lossy()
                .into_owned()
        })
        .collect::<Vec<_>>();
    assert_eq!(
        configured,
        BUILD_CASE_NAMES
            .iter()
            .map(|name| (*name).to_string())
            .collect::<Vec<_>>()
    );
}

fn run_build_case(name: &str) {
    let fixtures = Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/cases/build");
    let case_root = fixtures.join(name);
    assert!(case_root.is_dir(), "build case {name:?} is not a fixture");
    let mut manifest = CaseManifest::load(&case_root);
    let manifest_path = manifest.path().to_owned();
    let mode = manifest.required("mode");
    let workload = match manifest.required("resource").as_str() {
        "compiler" => TestWorkload::Compiler,
        "build" => TestWorkload::Build,
        resource => panic!("unknown build case resource {resource:?}"),
    };
    let _resources = nia_test_support::acquire_test_resources(workload);
    let contract = manifest.required("contract");
    let step = manifest.required("step");
    let workspace = support::temp_dir(&format!("build-case-{name}"));
    copy_case_tree(&case_root, &workspace);

    let mut command = support::nia_command();
    command.arg("build");
    if contract == "step-order" {
        command.arg("--jobs=1");
    }
    let command_root = if mode == "configured-build-success" {
        command.arg("--timings=detail").arg("--timings-format=json");
        let nested = workspace.join("src/nested");
        std::fs::create_dir_all(&nested).expect("create nested build case directory");
        nested
    } else {
        workspace.to_path_buf()
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
                stderr.contains(workspace.to_string_lossy().as_ref()),
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
            "external-run",
            "staged-output",
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
        "run=roadmap\n".to_string(),
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
            .any(|line| line.contains("\"build.steps_executed\":3")),
        "{stderr}"
    );
    assert!(
        json_lines
            .iter()
            .any(|line| line.contains("\"build.actions_executed\":3")),
        "{stderr}"
    );
    assert!(
        !stderr.lines().any(|line| line.starts_with("error:")),
        "{stderr}"
    );
    assert!(workspace.join(".nia-build/runner").is_dir());
    assert!(workspace.join(".nia-cache").is_dir());
    assert_no_transient_runner_files(workspace);
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
    assert_eq!(plan.actions().len(), 4);
    assert!(matches!(
        plan.actions()[0].kind,
        nia_build::ActionKind::CompilerEmit { .. }
    ));
    assert!(matches!(
        plan.actions()[1].kind,
        nia_build::ActionKind::CompilerCheck { .. }
    ));
    let run_action = plan
        .actions()
        .iter()
        .find(|action| action.key.name() == "run")
        .expect("run action");
    match &run_action.kind {
        nia_build::ActionKind::ExternalCommand {
            resource_class,
            environment_policy,
            cache_policy,
            program,
            arguments,
            working_directory,
            environment,
            inputs,
            outputs,
        } => {
            assert_eq!(
                *resource_class,
                nia_build::ActionResourceClass::Conservative
            );
            assert_eq!(
                *environment_policy,
                nia_build::CommandEnvironmentPolicy::Inherit
            );
            assert_eq!(*cache_policy, nia_build::CommandCachePolicy::Uncacheable);
            assert!(matches!(
                program,
                nia_build::CommandProgram::Path(path)
                    if matches!(
                        path.root(),
                        nia_build::LogicalPathRoot::Artifact(artifact)
                            if artifact.name() == "app"
                    ) && path.components().is_empty()
            ));
            assert_eq!(
                arguments,
                &[nia_build::CommandArgument::Literal("roadmap".to_string())]
            );
            assert!(matches!(
                working_directory.root(),
                nia_build::LogicalPathRoot::Package(package) if package.as_str() == "root"
            ));
            assert!(working_directory.components().is_empty());
            assert!(environment.is_empty());
            assert!(inputs.is_empty());
            assert!(outputs.is_empty());
        }
        other => panic!("expected external run action, found {other:?}"),
    }
    let tool_action = plan
        .actions()
        .iter()
        .find(|action| action.key.name() == "tool")
        .expect("tool action");
    match &tool_action.kind {
        nia_build::ActionKind::ExternalCommand {
            resource_class,
            environment_policy,
            cache_policy,
            program,
            arguments,
            working_directory,
            environment,
            inputs,
            outputs,
        } => {
            assert_eq!(*resource_class, nia_build::ActionResourceClass::Io);
            assert_eq!(
                *environment_policy,
                nia_build::CommandEnvironmentPolicy::Clear
            );
            assert_eq!(*cache_policy, nia_build::CommandCachePolicy::DeclaredInputs);
            assert_eq!(
                program,
                &nia_build::CommandProgram::Search("sh".to_string())
            );
            assert_eq!(
                arguments,
                &[
                    nia_build::CommandArgument::Literal("-c".to_string()),
                    nia_build::CommandArgument::Literal(
                        "test \"$MODE\" = fixture && test -s \"$4\" && tr a-z A-Z < \"$1\" > \"$2\" && printf 'source=tool-input\\n' > \"$3\""
                            .to_string()
                    ),
                    nia_build::CommandArgument::Literal("nia-build-tool".to_string()),
                    nia_build::CommandArgument::InputPath(inputs[0].clone()),
                    nia_build::CommandArgument::OutputPath(outputs[1].clone()),
                    nia_build::CommandArgument::OutputPath(outputs[0].clone()),
                    nia_build::CommandArgument::InputPath(inputs[1].clone()),
                ]
            );
            assert!(matches!(
                working_directory.root(),
                nia_build::LogicalPathRoot::Package(package) if package.as_str() == "root"
            ));
            assert!(working_directory.components().is_empty());
            assert_eq!(
                environment,
                &[nia_build::EnvironmentInput {
                    name: "MODE".to_string(),
                    value: Some("fixture".to_string()),
                }]
            );
            assert_eq!(inputs.len(), 2);
            assert!(matches!(
                inputs[0].root(),
                nia_build::LogicalPathRoot::Package(package) if package.as_str() == "root"
            ));
            assert_eq!(inputs[0].protocol_path(), "tool-input.txt");
            assert!(matches!(
                inputs[1].root(),
                nia_build::LogicalPathRoot::Artifact(artifact) if artifact.name() == "app"
            ));
            assert!(inputs[1].components().is_empty());
            assert_eq!(outputs.len(), 2);
            assert!(matches!(
                outputs[0].root(),
                nia_build::LogicalPathRoot::Build
            ));
            assert_eq!(outputs[0].protocol_path(), "transformed.meta");
            assert_eq!(outputs[1].protocol_path(), "transformed.txt");
        }
        other => panic!("expected staged external tool action, found {other:?}"),
    }
    assert_eq!(plan.steps().len(), 4);
    let run_step = plan
        .steps()
        .iter()
        .find(|step| step.key.name() == "run")
        .expect("run step");
    assert_eq!(
        run_step
            .dependencies
            .iter()
            .map(nia_build::StepKey::name)
            .collect::<Vec<_>>(),
        ["build"]
    );
    let tool_step = plan
        .steps()
        .iter()
        .find(|step| step.key.name() == "tool")
        .expect("tool step");
    assert_eq!(
        tool_step
            .dependencies
            .iter()
            .map(nia_build::StepKey::name)
            .collect::<Vec<_>>(),
        ["build", "run"]
    );
    assert_eq!(
        plan.default_step().map(nia_build::StepKey::name),
        Some("tool")
    );
    assert_eq!(
        plan.selected_step().map(nia_build::StepKey::name),
        Some("tool")
    );
    assert!(workspace.join(".nia-build/custom-app").is_file());
    assert!(!workspace.join(".nia-build/app").exists());
    assert_eq!(
        std::fs::read(workspace.join(".nia-build/transformed.txt")).unwrap(),
        b"ROADMAP\n"
    );
    assert_eq!(
        std::fs::read(workspace.join(".nia-build/transformed.meta")).unwrap(),
        b"source=tool-input\n"
    );
    assert!(
        std::fs::read_dir(workspace.join(".nia-build"))
            .unwrap()
            .all(|entry| !entry
                .unwrap()
                .file_name()
                .to_string_lossy()
                .starts_with(".nia-command-"))
    );
    assert_eq!(
        Command::new(workspace.join(".nia-build/custom-app"))
            .arg("roadmap")
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
    assert_no_transient_runner_files(workspace);
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
    assert_no_transient_runner_files(workspace);
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
    assert_no_transient_runner_files(workspace);
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
    assert_no_transient_runner_files(workspace);
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

fn assert_no_transient_runner_files(workspace: &Path) {
    let build_dir = workspace.join(".nia-build");
    if build_dir.is_dir() {
        assert!(std::fs::read_dir(&build_dir).unwrap().all(|entry| {
            !entry
                .unwrap()
                .file_name()
                .to_string_lossy()
                .starts_with(".build-plan-")
        }));
    }
    let runner_dir = build_dir.join("runner");
    if runner_dir.is_dir() {
        assert!(std::fs::read_dir(runner_dir).unwrap().all(|entry| {
            !entry
                .unwrap()
                .file_name()
                .to_string_lossy()
                .starts_with("nia-build-runner-")
        }));
    }
}

fn fixture_path_or_none(
    manifest_path: &Path,
    workspace: &Path,
    value: String,
) -> Option<std::path::PathBuf> {
    (value != "none").then(|| workspace.join(fixture_relative_path(manifest_path, value)))
}
