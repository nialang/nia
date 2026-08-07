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
    if contract == "module-optimization" {
        command.arg("-Os");
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
            assert_module_optimization_inheritance(&workspace);
        }
        _ => panic!(
            "unknown build case mode {mode:?} in {}",
            manifest_path.display()
        ),
    }
}

fn assert_module_optimization_inheritance(workspace: &Path) {
    let plan = nia_build::read_build_plan(&workspace.join(".nia-build/build-plan.bin"))
        .expect("decode optimization build plan");
    assert_eq!(plan.modules().len(), 2);
    assert_eq!(plan.modules()[0].key.name(), "app");
    assert_eq!(
        plan.modules()[0].optimization,
        nia_build::OptimizationMode::Os
    );
    assert_eq!(plan.modules()[1].key.name(), "override");
    assert_eq!(
        plan.modules()[1].optimization,
        nia_build::OptimizationMode::O0
    );
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
        "host=roadmap\n".to_string(),
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
            .any(|line| line.contains("\"build.steps_executed\":10")),
        "{stderr}"
    );
    assert!(
        json_lines
            .iter()
            .any(|line| line.contains("\"build.actions_executed\":10")),
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
    assert_eq!(plan.packages().len(), 2);
    let assets = plan
        .packages()
        .iter()
        .find(|package| package.key.as_str() == "assets")
        .expect("declared local package");
    assert_eq!(assets.root, "packages/assets");
    let root = plan
        .packages()
        .iter()
        .find(|package| package.key.as_str() == "root")
        .expect("root package");
    assert!(root.root.is_empty());
    assert_eq!(plan.modules().len(), 3);
    assert_eq!(plan.modules()[0].key.name(), "app");
    assert_eq!(
        plan.modules()[0].optimization,
        nia_build::OptimizationMode::O0
    );
    assert_eq!(
        plan.modules()[0].root_source.protocol_path(),
        "src/main.nia"
    );
    assert!(matches!(
        plan.modules()[0].root_source.root(),
        nia_build::LogicalPathRoot::Package(package) if package.as_str() == "assets"
    ));
    assert_eq!(plan.modules()[1].key.name(), "host-tool");
    assert_eq!(
        plan.modules()[1].root_source.protocol_path(),
        "src/host_tool.nia"
    );
    assert_eq!(plan.modules()[2].key.name(), "worker");
    assert_eq!(
        plan.modules()[2].optimization,
        nia_build::OptimizationMode::O0
    );
    assert_eq!(
        plan.modules()[2].root_source.protocol_path(),
        "generated/worker.nia"
    );
    assert!(matches!(
        plan.modules()[2].root_source.root(),
        nia_build::LogicalPathRoot::Build
    ));
    assert_eq!(plan.modules()[0].imports.len(), 2);
    let helper_import = plan.modules()[0]
        .imports
        .iter()
        .find(|import| import.name == "helper")
        .expect("generated helper import");
    assert_eq!(helper_import.path.protocol_path(), "generated/helper.nia");
    assert!(matches!(
        helper_import.path.root(),
        nia_build::LogicalPathRoot::Build
    ));
    let asset_helper_import = plan.modules()[0]
        .imports
        .iter()
        .find(|import| import.name == "assetHelper")
        .expect("package helper import");
    assert_eq!(asset_helper_import.path.protocol_path(), "helper.nia");
    assert!(matches!(
        asset_helper_import.path.root(),
        nia_build::LogicalPathRoot::Package(package) if package.as_str() == "assets"
    ));
    assert_eq!(plan.artifacts().len(), 4);
    let app_artifact = plan
        .artifacts()
        .iter()
        .find(|artifact| artifact.key.name() == "app")
        .expect("app artifact");
    assert_eq!(app_artifact.root_module.name(), "app");
    assert_eq!(app_artifact.output.protocol_path(), "custom-app");
    let host_artifact = plan
        .artifacts()
        .iter()
        .find(|artifact| artifact.key.name() == "host-tool")
        .expect("host artifact");
    assert_eq!(host_artifact.root_module.name(), "host-tool");
    assert_eq!(host_artifact.output.protocol_path(), "custom-host-tool");
    let object_artifact = plan
        .artifacts()
        .iter()
        .find(|artifact| artifact.key.name() == "objects")
        .expect("object artifact");
    assert_eq!(object_artifact.root_module.name(), "app");
    assert_eq!(object_artifact.output.protocol_path(), "custom-objects");
    assert_eq!(object_artifact.kind, nia_build::PlanArtifactKind::ObjectSet);
    let worker_artifact = plan
        .artifacts()
        .iter()
        .find(|artifact| artifact.key.name() == "worker")
        .expect("worker artifact");
    assert_eq!(worker_artifact.root_module.name(), "worker");
    assert_eq!(worker_artifact.output.protocol_path(), "custom-worker");
    assert_eq!(plan.actions().len(), 11);
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
                        "test \"$MODE\" = fixture && test -s \"$4\" && test -s \"$5\" && tr a-z A-Z < \"$1\" > \"$2\" && printf 'source=tool-input\\n' > \"$3\""
                            .to_string()
                    ),
                    nia_build::CommandArgument::Literal("nia-build-tool".to_string()),
                    nia_build::CommandArgument::InputPath(inputs[0].clone()),
                    nia_build::CommandArgument::OutputPath(outputs[1].clone()),
                    nia_build::CommandArgument::OutputPath(outputs[0].clone()),
                    nia_build::CommandArgument::InputPath(inputs[1].clone()),
                    nia_build::CommandArgument::InputPath(inputs[2].clone()),
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
            assert_eq!(inputs.len(), 3);
            assert!(matches!(
                inputs[0].root(),
                nia_build::LogicalPathRoot::Package(package) if package.as_str() == "assets"
            ));
            assert_eq!(inputs[0].protocol_path(), "tool-input.txt");
            assert!(matches!(
                inputs[1].root(),
                nia_build::LogicalPathRoot::Artifact(artifact) if artifact.name() == "app"
            ));
            assert!(inputs[1].components().is_empty());
            assert!(matches!(
                inputs[2].root(),
                nia_build::LogicalPathRoot::Artifact(artifact) if artifact.name() == "worker"
            ));
            assert!(inputs[2].components().is_empty());
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
    let host_emit_action = plan
        .actions()
        .iter()
        .find(|action| action.key.name() == "host-tool")
        .expect("host tool emit action");
    assert!(matches!(
        &host_emit_action.kind,
        nia_build::ActionKind::CompilerEmit { artifact, target }
            if artifact.name() == "host-tool" && target == plan.host_target()
    ));
    let host_run_action = plan
        .actions()
        .iter()
        .find(|action| action.key.name() == "run-host-tool")
        .expect("host tool run action");
    assert!(matches!(
        &host_run_action.kind,
        nia_build::ActionKind::ExternalCommand {
            program: nia_build::CommandProgram::Path(path),
            ..
        } if matches!(
            path.root(),
            nia_build::LogicalPathRoot::Artifact(artifact) if artifact.name() == "host-tool"
        ) && path.components().is_empty()
    ));
    let install_action = plan
        .actions()
        .iter()
        .find(|action| action.key.name() == "install")
        .expect("install action");
    assert!(matches!(
        &install_action.kind,
        nia_build::ActionKind::InstallArtifact { artifact, destination }
            if artifact.name() == "app"
                && matches!(destination.root(), nia_build::LogicalPathRoot::Build)
                && destination.protocol_path() == "install/custom-app"
    ));
    assert_eq!(plan.steps().len(), 11);
    let build_step = plan
        .steps()
        .iter()
        .find(|step| step.key.name() == "build")
        .expect("app emit step");
    assert_eq!(
        build_step
            .dependencies
            .iter()
            .map(nia_build::StepKey::name)
            .collect::<Vec<_>>(),
        ["generate-helper"]
    );
    let check_step = plan
        .steps()
        .iter()
        .find(|step| step.key.name() == "check")
        .expect("app check step");
    assert_eq!(
        check_step
            .dependencies
            .iter()
            .map(nia_build::StepKey::name)
            .collect::<Vec<_>>(),
        ["generate-helper"]
    );
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
        [
            "build",
            "install",
            "objects",
            "run",
            "run-host-tool",
            "worker"
        ]
    );
    let host_run_step = plan
        .steps()
        .iter()
        .find(|step| step.key.name() == "run-host-tool")
        .expect("host tool run step");
    assert_eq!(
        host_run_step
            .dependencies
            .iter()
            .map(nia_build::StepKey::name)
            .collect::<Vec<_>>(),
        ["host-tool"]
    );
    let install_step = plan
        .steps()
        .iter()
        .find(|step| step.key.name() == "install")
        .expect("install step");
    assert_eq!(
        install_step
            .dependencies
            .iter()
            .map(nia_build::StepKey::name)
            .collect::<Vec<_>>(),
        ["build"]
    );
    let worker_step = plan
        .steps()
        .iter()
        .find(|step| step.key.name() == "worker")
        .expect("worker emit step");
    assert_eq!(
        worker_step
            .dependencies
            .iter()
            .map(nia_build::StepKey::name)
            .collect::<Vec<_>>(),
        ["generate-worker"]
    );
    let object_step = plan
        .steps()
        .iter()
        .find(|step| step.key.name() == "objects")
        .expect("object emit step");
    assert_eq!(
        object_step
            .dependencies
            .iter()
            .map(nia_build::StepKey::name)
            .collect::<Vec<_>>(),
        ["generate-helper"]
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
    assert!(workspace.join(".nia-build/install/custom-app").is_file());
    assert!(!workspace.join(".nia-build/app").exists());
    assert!(workspace.join(".nia-build/custom-worker").is_file());
    assert!(workspace.join(".nia-build/custom-host-tool").is_file());
    assert!(workspace.join(".nia-build/custom-objects").is_dir());
    assert!(
        std::fs::read_dir(workspace.join(".nia-build/custom-objects"))
            .unwrap()
            .any(|entry| entry.unwrap().path().is_file())
    );
    assert!(workspace.join(".nia-build/generated/helper.nia").is_file());
    assert!(workspace.join(".nia-build/generated/worker.nia").is_file());
    assert!(!workspace.join(".nia-build/worker").exists());
    assert_eq!(
        std::fs::read(workspace.join(".nia-build/transformed.txt")).unwrap(),
        b"EXTERNAL ROADMAP\n"
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
    assert_eq!(
        Command::new(workspace.join(".nia-build/install/custom-app"))
            .arg("roadmap")
            .status_timeout("run installed configured build target")
            .code(),
        Some(0)
    );
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        assert_ne!(
            std::fs::metadata(workspace.join(".nia-build/install/custom-app"))
                .unwrap()
                .permissions()
                .mode()
                & 0o111,
            0
        );
    }
    assert_eq!(
        Command::new(workspace.join(".nia-build/custom-worker"))
            .status_timeout("run configured worker artifact")
            .code(),
        Some(0)
    );
    assert_eq!(
        Command::new(workspace.join(".nia-build/custom-host-tool"))
            .arg("roadmap")
            .status_timeout("run configured host tool artifact")
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
