// SPDX-License-Identifier: GPL-3.0-or-later
//! Shared build-plan fixtures for protocol and semantic validation tests.

use super::*;

pub(crate) fn module_key(name: &str) -> ModuleKey {
    ModuleKey::new(PackageKey::root(), name).unwrap()
}

pub(crate) fn artifact_key(name: &str) -> ArtifactKey {
    ArtifactKey::new(PackageKey::root(), name).unwrap()
}

pub(crate) fn action_key(name: &str) -> ActionKey {
    ActionKey::new(PackageKey::root(), name).unwrap()
}

pub(crate) fn step_key(name: &str) -> StepKey {
    StepKey::new(PackageKey::root(), name).unwrap()
}

pub(crate) fn target() -> TargetSpec {
    TargetSpec {
        arch: "x86_64".into(),
        vendor: "unknown".into(),
        os: "linux".into(),
        env: "gnu".into(),
        abi: "none".into(),
        endian: "little".into(),
        pointer_width: 64,
    }
}

pub(crate) fn draft(reverse: bool) -> BuildPlanDraft {
    let module_a = module_key("a");
    let module_b = module_key("b");
    let artifact = artifact_key("app");
    let check = action_key("check");
    let emit = action_key("emit");
    let check_step = step_key("check");
    let emit_step = step_key("emit");
    let mut modules = vec![
        PlanModule {
            key: module_a.clone(),
            root_source: LogicalPath::new(
                LogicalPathRoot::Package(PackageKey::root()),
                "src/a.nia",
            )
            .unwrap(),
            optimization: OptimizationMode::O2,
            imports: vec![],
        },
        PlanModule {
            key: module_b,
            root_source: LogicalPath::new(
                LogicalPathRoot::Package(PackageKey::root()),
                "src/b.nia",
            )
            .unwrap(),
            optimization: OptimizationMode::O0,
            imports: vec![],
        },
    ];
    let mut actions = vec![
        PlanAction {
            key: check.clone(),
            kind: ActionKind::CompilerCheck {
                module: module_a.clone(),
                target: target(),
                runtime: Runtime::Freestanding,
            },
        },
        PlanAction {
            key: emit.clone(),
            kind: ActionKind::CompilerEmit {
                artifact: artifact.clone(),
                target: target(),
                static_archives: Vec::new(),
            },
        },
    ];
    let mut steps = vec![
        PlanStep {
            key: check_step.clone(),
            action: check,
            dependencies: vec![],
        },
        PlanStep {
            key: emit_step.clone(),
            action: emit,
            dependencies: vec![check_step],
        },
    ];
    if reverse {
        modules.reverse();
        actions.reverse();
        steps.reverse();
    }
    BuildPlanDraft {
        root_package: PackageKey::root(),
        packages: vec![PlanPackage {
            key: PackageKey::root(),
            root: String::new(),
        }],
        host_target: target(),
        artifact_target: target(),
        modules,
        artifacts: vec![PlanArtifact {
            key: artifact,
            root_module: module_a,
            kind: PlanArtifactKind::Executable,
            output: LogicalPath::new(LogicalPathRoot::Build, "app").unwrap(),
            runtime: Runtime::Freestanding,
        }],
        actions,
        steps,
        default_step: Some(emit_step),
        selected_step: None,
    }
}

pub(crate) fn static_archive_link_draft() -> BuildPlanDraft {
    let mut value = draft(false);
    let support = artifact_key("support");
    let runtime = artifact_key("runtime");
    for (artifact, output) in [
        (support.clone(), "lib/libsupport.a"),
        (runtime.clone(), "lib/libruntime.a"),
    ] {
        value.artifacts.push(PlanArtifact {
            key: artifact.clone(),
            root_module: module_key("b"),
            kind: PlanArtifactKind::StaticArchive,
            output: LogicalPath::new(LogicalPathRoot::Build, output).unwrap(),
            runtime: Runtime::Bare,
        });
        value.actions.push(PlanAction {
            key: action_key(&format!("emit-{}", artifact.name())),
            kind: ActionKind::CompilerEmit {
                artifact: artifact.clone(),
                target: target(),
                static_archives: Vec::new(),
            },
        });
        value.steps.push(PlanStep {
            key: step_key(&format!("emit-{}", artifact.name())),
            action: action_key(&format!("emit-{}", artifact.name())),
            dependencies: Vec::new(),
        });
    }
    let executable_emit = value
        .actions
        .iter_mut()
        .find(|action| action.key.name() == "emit")
        .unwrap();
    let ActionKind::CompilerEmit {
        static_archives, ..
    } = &mut executable_emit.kind
    else {
        unreachable!()
    };
    *static_archives = vec![runtime, support];
    value
        .steps
        .iter_mut()
        .find(|step| step.key.name() == "emit")
        .unwrap()
        .dependencies
        .extend([step_key("emit-runtime"), step_key("emit-support")]);
    value
}

pub(crate) fn generated_source_draft(
    generated_path: &str,
    consumer_dependencies: Vec<&str>,
) -> BuildPlanDraft {
    let module = module_key("generated");
    let generate = action_key("generate");
    let check = action_key("check");
    let generate_step = step_key("generate");
    let check_step = step_key("check");
    BuildPlanDraft {
        root_package: PackageKey::root(),
        packages: vec![PlanPackage {
            key: PackageKey::root(),
            root: String::new(),
        }],
        host_target: target(),
        artifact_target: target(),
        modules: vec![PlanModule {
            key: module.clone(),
            root_source: LogicalPath::new(LogicalPathRoot::Build, "generated/root.nia").unwrap(),
            optimization: OptimizationMode::O2,
            imports: Vec::new(),
        }],
        artifacts: Vec::new(),
        actions: vec![
            PlanAction {
                key: generate,
                kind: ActionKind::GeneratedFile {
                    output: LogicalPath::new(LogicalPathRoot::Build, generated_path).unwrap(),
                    contents: b"pub fn generated() () {}\n".to_vec(),
                },
            },
            PlanAction {
                key: check.clone(),
                kind: ActionKind::CompilerCheck {
                    module,
                    target: target(),
                    runtime: Runtime::Freestanding,
                },
            },
        ],
        steps: vec![
            PlanStep {
                key: generate_step,
                action: action_key("generate"),
                dependencies: Vec::new(),
            },
            PlanStep {
                key: check_step.clone(),
                action: check,
                dependencies: consumer_dependencies.into_iter().map(step_key).collect(),
            },
        ],
        default_step: Some(check_step),
        selected_step: None,
    }
}
