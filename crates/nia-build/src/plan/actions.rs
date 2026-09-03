// SPDX-License-Identifier: GPL-3.0-or-later
//! Action canonicalization and local semantic validation.
//!
//! This module validates relationships contained within action declarations.
//! Cross-step scheduling edges and output ownership remain in `dependencies`.

use super::*;

pub(super) fn canonicalize_actions(
    actions: &mut [PlanAction],
    modules: &[PlanModule],
    artifacts: &[PlanArtifact],
    host_target: &TargetSpec,
    artifact_target: &TargetSpec,
) -> Result<(), PlanError> {
    actions.sort_by(|left, right| left.key.cmp(&right.key));
    reject_duplicate_by(
        actions,
        |action| action.key.clone(),
        PlanError::DuplicateAction,
    )?;
    let module_keys: BTreeSet<_> = modules.iter().map(|module| &module.key).collect();
    let artifact_keys: BTreeSet<_> = artifacts.iter().map(|artifact| &artifact.key).collect();
    let artifacts_by_key: BTreeMap<_, _> = artifacts
        .iter()
        .map(|artifact| (&artifact.key, artifact))
        .collect();
    let mut emit_targets = BTreeMap::new();
    for action in actions.iter() {
        let ActionKind::CompilerEmit {
            artifact, target, ..
        } = &action.kind
        else {
            continue;
        };
        // Artifact identity denotes one published value. Multiple emitters
        // would otherwise be silently collapsed in this target index and only
        // fail later through incidental output ownership ordering.
        if emit_targets
            .insert(artifact.clone(), target.clone())
            .is_some()
        {
            return Err(PlanError::InvalidArtifactUse {
                action: action.key.clone(),
                artifact: artifact.clone(),
                reason: "artifact has multiple compiler emit actions",
            });
        }
    }
    for action in actions {
        let compiler_target = match &action.kind {
            ActionKind::CompilerCheck { target, .. } | ActionKind::CompilerEmit { target, .. } => {
                Some(target)
            }
            _ => None,
        };
        if let Some(target) = compiler_target
            && target != host_target
            && target != artifact_target
        {
            // The two plan targets define the complete driver set authorized
            // by the invocation. Per-action targets may select either role,
            // but must not smuggle an unvalidated third toolchain target in.
            return Err(PlanError::InvalidActionTarget(Box::new(
                InvalidActionTarget {
                    action: action.key.clone(),
                    target: target.clone(),
                },
            )));
        }
        match &mut action.kind {
            ActionKind::CompilerCheck { module, .. } if !module_keys.contains(module) => {
                return Err(PlanError::MissingModule {
                    owner: format!("action {}", action.key.name()),
                    module: module.clone(),
                });
            }
            ActionKind::CompilerEmit { artifact, .. } if !artifact_keys.contains(artifact) => {
                return Err(PlanError::MissingArtifact {
                    action: action.key.clone(),
                    artifact: artifact.clone(),
                });
            }
            ActionKind::CompilerEmit {
                artifact,
                target,
                static_archives,
            } => {
                let Some(emitted) = artifacts_by_key.get(artifact) else {
                    return Err(PlanError::MissingArtifact {
                        action: action.key.clone(),
                        artifact: artifact.clone(),
                    });
                };
                if emitted.kind != PlanArtifactKind::Executable && !static_archives.is_empty() {
                    return Err(PlanError::InvalidArtifactUse {
                        action: action.key.clone(),
                        artifact: artifact.clone(),
                        reason: "only executable artifacts can link static archives",
                    });
                }
                let mut seen = BTreeSet::new();
                for archive in static_archives.iter() {
                    let Some(linked) = artifacts_by_key.get(archive) else {
                        return Err(PlanError::MissingArtifact {
                            action: action.key.clone(),
                            artifact: archive.clone(),
                        });
                    };
                    if linked.kind != PlanArtifactKind::StaticArchive {
                        return Err(PlanError::InvalidArtifactUse {
                            action: action.key.clone(),
                            artifact: archive.clone(),
                            reason: "executable link inputs must be static archives",
                        });
                    }
                    if !seen.insert(archive.clone()) {
                        return Err(PlanError::InvalidArtifactUse {
                            action: action.key.clone(),
                            artifact: archive.clone(),
                            reason: "duplicate static archive link input",
                        });
                    }
                    let target_matches = emit_targets
                        .get(archive)
                        .is_some_and(|produced_target| produced_target == target);
                    if !target_matches {
                        return Err(PlanError::InvalidArtifactUse {
                            action: action.key.clone(),
                            artifact: archive.clone(),
                            reason: "linked static archive has no emit action for the executable target",
                        });
                    }
                }
            }
            ActionKind::InstallArtifact { artifact, .. } if !artifact_keys.contains(artifact) => {
                return Err(PlanError::MissingArtifact {
                    action: action.key.clone(),
                    artifact: artifact.clone(),
                });
            }
            ActionKind::InstallArtifact { artifact, .. } => {
                let Some(installed) = artifacts_by_key.get(artifact) else {
                    return Err(PlanError::MissingArtifact {
                        action: action.key.clone(),
                        artifact: artifact.clone(),
                    });
                };
                if !matches!(
                    installed.kind,
                    PlanArtifactKind::Executable | PlanArtifactKind::StaticArchive
                ) {
                    return Err(PlanError::InvalidArtifactUse {
                        action: action.key.clone(),
                        artifact: artifact.clone(),
                        reason: "only file artifacts can be installed",
                    });
                }
            }
            ActionKind::ExternalCommand {
                program,
                arguments,
                working_directory,
                environment_policy,
                cache_policy,
                environment,
                inputs,
                outputs,
                ..
            }
            | ActionKind::TestExecutable {
                program,
                arguments,
                working_directory,
                environment_policy,
                cache_policy,
                environment,
                inputs,
                outputs,
                ..
            } => {
                validate_external_command_artifacts(
                    &action.key,
                    program,
                    working_directory,
                    inputs,
                    &artifacts_by_key,
                    &emit_targets,
                    host_target,
                )?;
                if matches!(program, CommandProgram::Search(name) if name.is_empty()) {
                    return Err(PlanError::InvalidCommand {
                        action: action.key.clone(),
                        reason: "empty program",
                    });
                }
                if matches!(program, CommandProgram::Search(name) if name.contains('\0')) {
                    return Err(PlanError::InvalidCommand {
                        action: action.key.clone(),
                        reason: "program contains NUL",
                    });
                }
                if arguments.iter().any(
                    |argument| matches!(argument, CommandArgument::Literal(value) if value.contains('\0')),
                ) {
                    return Err(PlanError::InvalidCommand {
                        action: action.key.clone(),
                        reason: "argument contains NUL",
                    });
                }
                environment.sort();
                for input in environment.iter() {
                    if input.name.is_empty()
                        || input.name.contains(['\0', '='])
                        || input
                            .value
                            .as_ref()
                            .is_some_and(|value| value.contains('\0'))
                    {
                        return Err(PlanError::InvalidCommand {
                            action: action.key.clone(),
                            reason: "invalid environment input",
                        });
                    }
                }
                if environment
                    .windows(2)
                    .any(|pair| pair[0].name == pair[1].name)
                {
                    return Err(PlanError::InvalidCommand {
                        action: action.key.clone(),
                        reason: "duplicate environment input",
                    });
                }
                if *cache_policy == CommandCachePolicy::DeclaredInputs
                    && *environment_policy != CommandEnvironmentPolicy::Clear
                {
                    return Err(PlanError::InvalidCommand {
                        action: action.key.clone(),
                        reason: "cacheable command must clear inherited environment",
                    });
                }
                if *cache_policy == CommandCachePolicy::DeclaredInputs && outputs.is_empty() {
                    return Err(PlanError::InvalidCommand {
                        action: action.key.clone(),
                        reason: "cacheable command must declare an output",
                    });
                }
                inputs.sort();
                outputs.sort();
                inputs.dedup();
                outputs.dedup();
                for argument in arguments.iter() {
                    let (path, declared, reason) = match argument {
                        CommandArgument::Literal(_) => continue,
                        CommandArgument::InputPath(path) => (
                            path,
                            inputs.as_slice(),
                            "input argument path is not declared as an input",
                        ),
                        CommandArgument::OutputPath(path) => (
                            path,
                            outputs.as_slice(),
                            "output argument path is not declared as an output",
                        ),
                    };
                    if declared.binary_search(path).is_err() {
                        return Err(PlanError::InvalidCommand {
                            action: action.key.clone(),
                            reason,
                        });
                    }
                }
                if outputs.iter().any(|output| {
                    !arguments.iter().any(
                        |argument| matches!(argument, CommandArgument::OutputPath(path) if path == output),
                    )
                }) {
                    return Err(PlanError::InvalidCommand {
                        action: action.key.clone(),
                        reason: "declared output has no staged command argument",
                    });
                }
            }
            _ => {}
        }
    }
    Ok(())
}

fn validate_external_command_artifacts(
    action: &ActionKey,
    program: &CommandProgram,
    working_directory: &LogicalPath,
    inputs: &[LogicalPath],
    artifacts: &BTreeMap<&ArtifactKey, &PlanArtifact>,
    emit_targets: &BTreeMap<ArtifactKey, TargetSpec>,
    host_target: &TargetSpec,
) -> Result<(), PlanError> {
    if let CommandProgram::Path(program) = program
        && let LogicalPathRoot::Artifact(artifact) = program.root()
    {
        require_artifact_root(
            action,
            program,
            "artifact program path must name the artifact root",
        )?;
        require_artifact_kind(
            action,
            artifact,
            artifacts,
            PlanArtifactKind::Executable,
            "external command programs must be executable artifacts",
        )?;
        // Artifact programs execute during the build, so their producer must
        // target the build host even when the final artifact target differs.
        if emit_targets
            .get(artifact)
            .is_some_and(|target| target != host_target)
        {
            return Err(PlanError::InvalidArtifactUse {
                action: action.clone(),
                artifact: artifact.clone(),
                reason: "external command programs must be emitted for the host target",
            });
        }
    }
    if let LogicalPathRoot::Artifact(artifact) = working_directory.root() {
        require_artifact_root(
            action,
            working_directory,
            "artifact working directory must name the artifact root",
        )?;
        // Only ObjectSet artifacts publish a directory. Treating a file
        // artifact as a cwd would defer a type error to process spawn.
        require_artifact_kind(
            action,
            artifact,
            artifacts,
            PlanArtifactKind::ObjectSet,
            "external command working directories must be object-set artifacts",
        )?;
    }
    for input in inputs {
        let LogicalPathRoot::Artifact(artifact) = input.root() else {
            continue;
        };
        require_artifact_root(
            action,
            input,
            "artifact input path must name the artifact root",
        )?;
        require_artifact(action, artifact, artifacts)?;
    }
    Ok(())
}

fn require_artifact_root(
    action: &ActionKey,
    path: &LogicalPath,
    reason: &'static str,
) -> Result<(), PlanError> {
    if path.components().is_empty() {
        return Ok(());
    }
    Err(PlanError::InvalidCommand {
        action: action.clone(),
        reason,
    })
}

fn require_artifact_kind(
    action: &ActionKey,
    artifact: &ArtifactKey,
    artifacts: &BTreeMap<&ArtifactKey, &PlanArtifact>,
    expected: PlanArtifactKind,
    reason: &'static str,
) -> Result<(), PlanError> {
    let declared = require_artifact(action, artifact, artifacts)?;
    if declared.kind != expected {
        return Err(PlanError::InvalidArtifactUse {
            action: action.clone(),
            artifact: artifact.clone(),
            reason,
        });
    }
    Ok(())
}

fn require_artifact<'a>(
    action: &ActionKey,
    artifact: &ArtifactKey,
    artifacts: &BTreeMap<&ArtifactKey, &'a PlanArtifact>,
) -> Result<&'a PlanArtifact, PlanError> {
    artifacts
        .get(artifact)
        .copied()
        .ok_or_else(|| PlanError::MissingArtifact {
            action: action.clone(),
            artifact: artifact.clone(),
        })
}
