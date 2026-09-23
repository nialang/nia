// SPDX-License-Identifier: GPL-3.0-or-later
//! Freeze-time canonicalization and graph validation.
//!
//! The model types remain in the crate root; this module owns the transition
//! from a mutable draft to the deterministic, executable plan boundary.

use std::collections::{BTreeMap, BTreeSet};

use super::*;

pub(super) use super::actions::canonicalize_actions;
pub(super) use super::dependencies::{
    validate_artifact_dependencies, validate_build_input_dependencies,
    validate_generated_source_dependencies, validate_output_ownership,
};

pub(super) fn validate_target(target: &TargetSpec, role: &'static str) -> Result<(), PlanError> {
    if target.arch.is_empty() || target.os.is_empty() {
        return Err(PlanError::InvalidTarget {
            role,
            reason: "architecture and operating system must be named",
        });
    }
    if [
        &target.arch,
        &target.vendor,
        &target.os,
        &target.env,
        &target.abi,
        &target.endian,
    ]
    .into_iter()
    .any(|value| value.contains('\0'))
    {
        return Err(PlanError::InvalidTarget {
            role,
            reason: "target field contains NUL",
        });
    }
    if !matches!(target.endian.as_str(), "little" | "big") {
        return Err(PlanError::InvalidTarget {
            role,
            reason: "endianness must be `little` or `big`",
        });
    }
    if !matches!(target.pointer_width, 8 | 16 | 32 | 64 | 128) {
        return Err(PlanError::InvalidTarget {
            role,
            reason: "unsupported pointer width",
        });
    }
    Ok(())
}

pub(super) fn canonicalize_packages(
    packages: &mut [PlanPackage],
    root_package: &PackageKey,
) -> Result<(), PlanError> {
    packages.sort_by(|left, right| left.key.cmp(&right.key));
    reject_duplicate_by(
        packages,
        |package| package.key.clone(),
        PlanError::DuplicatePackage,
    )?;
    if !packages.iter().any(|package| &package.key == root_package) {
        return Err(PlanError::MissingPackage(root_package.clone()));
    }
    for package in packages {
        let is_root = &package.key == root_package;
        let invalid = |error| PlanError::InvalidPackageRoot {
            package: package.key.clone(),
            error,
        };
        if is_root && !package.root.is_empty() {
            return Err(invalid(PackageRootError::RootPackageMustBeEmpty));
        }
        if !is_root && package.root.is_empty() {
            return Err(invalid(PackageRootError::ExternalPackageMustBeNonempty));
        }
        LogicalPath::new(LogicalPathRoot::Build, &package.root)
            .map_err(|error| invalid(PackageRootError::InvalidPath(error)))?;
    }
    Ok(())
}

pub(super) fn validate_package_references(draft: &BuildPlanDraft) -> Result<(), PlanError> {
    let packages: BTreeSet<_> = draft.packages.iter().map(|package| &package.key).collect();
    let require = |package: &PackageKey| {
        if packages.contains(package) {
            Ok(())
        } else {
            Err(PlanError::MissingPackage(package.clone()))
        }
    };
    for module in &draft.modules {
        require(module.key.package())?;
        validate_path_package(&module.root_source, &require)?;
        for import in &module.imports {
            validate_path_package(&import.path, &require)?;
        }
    }
    for artifact in &draft.artifacts {
        require(artifact.key.package())?;
        require(artifact.root_module.package())?;
        validate_path_package(&artifact.output, &require)?;
    }
    for action in &draft.actions {
        require(action.key.package())?;
        match &action.kind {
            ActionKind::CompilerCheck { module, .. } => require(module.package())?,
            ActionKind::CompilerEmit {
                artifact,
                static_archives,
                ..
            } => {
                require(artifact.package())?;
                for archive in static_archives {
                    require(archive.package())?;
                }
            }
            ActionKind::ExternalCommand(command) | ActionKind::TestExecutable(command) => {
                let CommandAction {
                    program,
                    working_directory,
                    inputs,
                    outputs,
                    ..
                } = command;
                if let CommandProgram::Path(path) = program {
                    validate_path_package(path, &require)?;
                }
                validate_path_package(working_directory, &require)?;
                for path in inputs.iter().chain(outputs) {
                    validate_path_package(path, &require)?;
                }
            }
            ActionKind::GeneratedFile { output, .. } => {
                validate_path_package(output, &require)?;
            }
            ActionKind::InstallArtifact {
                artifact,
                destination,
            } => {
                require(artifact.package())?;
                validate_path_package(destination, &require)?;
            }
            ActionKind::Aggregate | ActionKind::Uncacheable { .. } => {}
        }
    }
    for step in &draft.steps {
        require(step.key.package())?;
        require(step.action.package())?;
        for dependency in &step.dependencies {
            require(dependency.package())?;
        }
    }
    if let Some(step) = &draft.default_step {
        require(step.package())?;
    }
    if let Some(step) = &draft.selected_step {
        require(step.package())?;
    }
    Ok(())
}

fn validate_path_package(
    path: &LogicalPath,
    require: &impl Fn(&PackageKey) -> Result<(), PlanError>,
) -> Result<(), PlanError> {
    match path.root() {
        LogicalPathRoot::Package(package) => require(package),
        LogicalPathRoot::Artifact(artifact) => require(artifact.package()),
        _ => Ok(()),
    }
}

pub(super) fn canonicalize_modules(modules: &mut [PlanModule]) -> Result<(), PlanError> {
    modules.sort_by(|left, right| left.key.cmp(&right.key));
    reject_duplicate_by(
        modules,
        |module| module.key.clone(),
        PlanError::DuplicateModule,
    )?;
    for module in modules {
        module.imports.sort();
        for pair in module.imports.windows(2) {
            if pair[0].name == pair[1].name {
                return Err(PlanError::DuplicateImport {
                    module: module.key.clone(),
                    name: pair[0].name.clone(),
                });
            }
        }
    }
    Ok(())
}

pub(super) fn canonicalize_artifacts(
    artifacts: &mut [PlanArtifact],
    modules: &[PlanModule],
) -> Result<(), PlanError> {
    artifacts.sort_by(|left, right| left.key.cmp(&right.key));
    reject_duplicate_by(
        artifacts,
        |artifact| artifact.key.clone(),
        PlanError::DuplicateArtifact,
    )?;
    let module_keys: BTreeSet<_> = modules.iter().map(|module| &module.key).collect();
    for artifact in artifacts {
        if !module_keys.contains(&artifact.root_module) {
            return Err(PlanError::MissingModule {
                owner: format!("artifact {}", artifact.key.name()),
                module: artifact.root_module.clone(),
            });
        }
    }
    Ok(())
}

pub(super) fn canonicalize_steps(
    steps: &mut [PlanStep],
    actions: &[PlanAction],
) -> Result<(), PlanError> {
    steps.sort_by(|left, right| left.key.cmp(&right.key));
    reject_duplicate_by(steps, |step| step.key.clone(), PlanError::DuplicateStep)?;
    let action_keys: BTreeSet<_> = actions.iter().map(|action| &action.key).collect();
    let step_keys: BTreeSet<_> = steps.iter().map(|step| step.key.clone()).collect();
    for step in steps {
        if !action_keys.contains(&step.action) {
            return Err(PlanError::MissingAction {
                step: step.key.clone(),
                action: step.action.clone(),
            });
        }
        step.dependencies.sort();
        step.dependencies.dedup();
        for dependency in &step.dependencies {
            if !step_keys.contains(dependency) {
                return Err(PlanError::MissingStep {
                    owner: format!("step {}", step.key.name()),
                    step: dependency.clone(),
                });
            }
        }
    }
    Ok(())
}

pub(super) fn validate_step_selection(draft: &BuildPlanDraft) -> Result<(), PlanError> {
    let keys: BTreeSet<_> = draft.steps.iter().map(|step| &step.key).collect();
    if !draft.steps.is_empty() && draft.default_step.is_none() && draft.selected_step.is_none() {
        return Err(PlanError::MissingDefaultStep);
    }
    for (owner, selected) in [
        ("default", &draft.default_step),
        ("selected", &draft.selected_step),
    ] {
        if let Some(step) = selected
            && !keys.contains(step)
        {
            return Err(PlanError::MissingStep {
                owner: owner.to_string(),
                step: step.clone(),
            });
        }
    }
    Ok(())
}

pub(super) fn validate_step_cycles(steps: &[PlanStep]) -> Result<(), PlanError> {
    let mut indegree: BTreeMap<_, usize> = steps.iter().map(|step| (step.key.clone(), 0)).collect();
    let mut dependents: BTreeMap<StepKey, Vec<StepKey>> = BTreeMap::new();
    for step in steps {
        for dependency in &step.dependencies {
            let Some(degree) = indegree.get_mut(&step.key) else {
                return Err(PlanError::MissingStep {
                    owner: "cycle validator".to_string(),
                    step: step.key.clone(),
                });
            };
            *degree += 1;
            dependents
                .entry(dependency.clone())
                .or_default()
                .push(step.key.clone());
        }
    }
    let mut ready: BTreeSet<_> = indegree
        .iter()
        .filter_map(|(key, degree)| (*degree == 0).then_some(key.clone()))
        .collect();
    let mut visited = 0;
    while let Some(key) = ready.pop_first() {
        visited += 1;
        if let Some(items) = dependents.get(&key) {
            for dependent in items {
                let Some(degree) = indegree.get_mut(dependent) else {
                    return Err(PlanError::MissingStep {
                        owner: "cycle validator dependent".to_string(),
                        step: dependent.clone(),
                    });
                };
                *degree -= 1;
                if *degree == 0 {
                    ready.insert(dependent.clone());
                }
            }
        }
    }
    if visited != steps.len() {
        return Err(PlanError::StepCycle(
            indegree
                .into_iter()
                .filter_map(|(key, degree)| (degree != 0).then_some(key))
                .collect(),
        ));
    }
    Ok(())
}

pub(crate) fn reject_duplicate_by<T, K: Ord + Clone>(
    values: &[T],
    key: impl Fn(&T) -> K,
    error: impl Fn(K) -> PlanError,
) -> Result<(), PlanError> {
    for pair in values.windows(2) {
        let left = key(&pair[0]);
        let right = key(&pair[1]);
        if left == right {
            return Err(error(left));
        }
    }
    Ok(())
}
