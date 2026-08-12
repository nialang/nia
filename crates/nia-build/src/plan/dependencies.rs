// SPDX-License-Identifier: GPL-3.0-or-later
//! Build graph dependency and output ownership validation.
//!
//! Declared artifact and build-root reads are scheduling edges, not only cache
//! identity. These validators connect each read to a unique output owner and
//! require that owner in every consumer step's transitive dependency closure.

use super::*;

pub(super) fn validate_artifact_dependencies(
    actions: &[PlanAction],
    steps: &[PlanStep],
) -> Result<(), PlanError> {
    let action_by_key: BTreeMap<_, _> = actions
        .iter()
        .map(|action| (&action.key, &action.kind))
        .collect();
    let step_by_key: BTreeMap<_, _> = steps.iter().map(|step| (&step.key, step)).collect();
    for step in steps {
        let mut required = BTreeMap::<ArtifactKey, &'static str>::new();
        match action_by_key.get(&step.action).copied() {
            Some(ActionKind::ExternalCommand {
                program, inputs, ..
            }) => {
                if let CommandProgram::Path(program) = program
                    && let LogicalPathRoot::Artifact(artifact) = program.root()
                {
                    if !program.components().is_empty() {
                        return Err(PlanError::InvalidCommand {
                            action: step.action.clone(),
                            reason: "artifact program path must name the artifact root",
                        });
                    }
                    required.insert(
                        artifact.clone(),
                        "artifact program has no compiler emit dependency",
                    );
                }
                for input in inputs {
                    let LogicalPathRoot::Artifact(artifact) = input.root() else {
                        continue;
                    };
                    if !input.components().is_empty() {
                        return Err(PlanError::InvalidCommand {
                            action: step.action.clone(),
                            reason: "artifact input path must name the artifact root",
                        });
                    }
                    required
                        .entry(artifact.clone())
                        .or_insert("artifact input has no compiler emit dependency");
                }
            }
            Some(ActionKind::CompilerEmit {
                static_archives, ..
            }) => {
                for archive in static_archives {
                    required.insert(
                        archive.clone(),
                        "linked static archive has no compiler emit dependency",
                    );
                }
            }
            Some(ActionKind::InstallArtifact { artifact, .. }) => {
                required.insert(
                    artifact.clone(),
                    "artifact install has no compiler emit dependency",
                );
            }
            _ => continue,
        }
        if required.is_empty() {
            continue;
        }

        let dependency_actions = dependency_action_closure(step, &step_by_key)?;
        for (artifact, reason) in required {
            let produced = dependency_actions.iter().any(|action| {
                matches!(
                    action_by_key.get(action),
                    Some(ActionKind::CompilerEmit { artifact: produced, .. })
                        if produced == &artifact
                )
            });
            if !produced {
                return Err(PlanError::InvalidCommand {
                    action: step.action.clone(),
                    reason,
                });
            }
        }
    }
    Ok(())
}

pub(super) fn dependency_action_closure(
    step: &PlanStep,
    step_by_key: &BTreeMap<&StepKey, &PlanStep>,
) -> Result<BTreeSet<ActionKey>, PlanError> {
    let mut pending = step.dependencies.clone();
    let mut visited = BTreeSet::new();
    let mut actions = BTreeSet::new();
    while let Some(dependency_key) = pending.pop() {
        if !visited.insert(dependency_key.clone()) {
            continue;
        }
        let Some(dependency) = step_by_key.get(&dependency_key) else {
            return Err(PlanError::MissingStep {
                owner: format!("step {} dependency closure", step.key.name()),
                step: dependency_key,
            });
        };
        actions.insert(dependency.action.clone());
        pending.extend(dependency.dependencies.iter().cloned());
    }
    Ok(actions)
}

pub(super) fn validate_build_input_dependencies(draft: &BuildPlanDraft) -> Result<(), PlanError> {
    let artifacts: BTreeMap<_, _> = draft
        .artifacts
        .iter()
        .map(|artifact| (&artifact.key, artifact))
        .collect();
    let mut producers = Vec::<(ActionOutput<'_>, &ActionKey)>::new();
    for action in &draft.actions {
        for output in action_outputs(action, &artifacts)? {
            producers.push((output, &action.key));
        }
    }
    let steps_by_action: BTreeMap<_, Vec<&PlanStep>> =
        draft.steps.iter().fold(BTreeMap::new(), |mut steps, step| {
            steps.entry(&step.action).or_default().push(step);
            steps
        });
    let step_by_key: BTreeMap<_, _> = draft.steps.iter().map(|step| (&step.key, step)).collect();

    for action in &draft.actions {
        let ActionKind::ExternalCommand {
            program,
            working_directory,
            inputs,
            ..
        } = &action.kind
        else {
            continue;
        };
        let Some(consumer_steps) = steps_by_action.get(&action.key) else {
            continue;
        };
        let build_program = match program {
            CommandProgram::Path(path) if matches!(path.root(), LogicalPathRoot::Build) => {
                Some(path)
            }
            _ => None,
        };
        // The empty Build root is invocation-owned infrastructure. Any deeper
        // working directory must already exist when the process is spawned,
        // so it is a scheduling dependency just like a program or input path.
        let build_working_directory = (matches!(working_directory.root(), LogicalPathRoot::Build)
            && !working_directory.is_empty())
        .then_some(working_directory);
        let build_inputs = inputs
            .iter()
            .filter(|input| matches!(input.root(), LogicalPathRoot::Build));
        for input in build_program
            .into_iter()
            .chain(build_working_directory)
            .chain(build_inputs)
        {
            let input_producers = producers
                .iter()
                .filter(|(output, _)| output.produces(input))
                .map(|(_, action)| *action)
                .collect::<Vec<_>>();
            if input_producers.is_empty() {
                return Err(PlanError::MissingBuildInputProducer {
                    action: Box::new(action.key.clone()),
                    path: Box::new(input.clone()),
                });
            }
            // A declared input is a scheduling edge as well as a cache key.
            // Check every step because one action may be exposed through
            // several steps with independently incomplete dependency closures.
            for consumer_step in consumer_steps {
                let dependency_actions = dependency_action_closure(consumer_step, &step_by_key)?;
                for producer in &input_producers {
                    if !dependency_actions.contains(*producer) {
                        return Err(PlanError::BuildInputProducerOutsideClosure {
                            action: Box::new(action.key.clone()),
                            path: Box::new(input.clone()),
                            producer: Box::new((*producer).clone()),
                        });
                    }
                }
            }
        }
    }
    Ok(())
}

pub(super) fn validate_generated_source_dependencies(
    draft: &BuildPlanDraft,
) -> Result<(), PlanError> {
    let modules: BTreeMap<_, _> = draft
        .modules
        .iter()
        .map(|module| (&module.key, module))
        .collect();
    let artifacts: BTreeMap<_, _> = draft
        .artifacts
        .iter()
        .map(|artifact| (&artifact.key, &artifact.root_module))
        .collect();
    let steps_by_action: BTreeMap<_, Vec<&PlanStep>> =
        draft.steps.iter().fold(BTreeMap::new(), |mut steps, step| {
            steps.entry(&step.action).or_default().push(step);
            steps
        });
    let mut producers = BTreeMap::<&LogicalPath, &ActionKey>::new();
    for action in &draft.actions {
        match &action.kind {
            ActionKind::GeneratedFile { output, .. } => {
                producers.insert(output, &action.key);
            }
            ActionKind::ExternalCommand { outputs, .. } => {
                for output in outputs {
                    producers.insert(output, &action.key);
                }
            }
            _ => {}
        }
    }
    let step_by_key: BTreeMap<_, _> = draft.steps.iter().map(|step| (&step.key, step)).collect();

    for action in &draft.actions {
        let module_key = match &action.kind {
            ActionKind::CompilerCheck { module, .. } => Some(module),
            ActionKind::CompilerEmit { artifact, .. } => artifacts.get(artifact).copied(),
            _ => None,
        };
        let Some(module_key) = module_key else {
            continue;
        };
        let Some(consumer_steps) = steps_by_action.get(&action.key) else {
            continue;
        };
        let Some(module) = modules.get(module_key) else {
            return Err(PlanError::MissingModule {
                owner: format!("action {}", action.key.name()),
                module: (*module_key).clone(),
            });
        };
        for module_path in std::iter::once(&module.root_source)
            .chain(module.imports.iter().map(|import| &import.path))
        {
            if !matches!(module_path.root(), LogicalPathRoot::Build) {
                continue;
            }
            let Some(producer) = producers.get(module_path).copied() else {
                return Err(PlanError::MissingGeneratedSourceProducer {
                    action: Box::new(action.key.clone()),
                    module: Box::new((*module_key).clone()),
                    path: Box::new(module_path.clone()),
                });
            };
            if !steps_by_action.contains_key(producer) {
                return Err(PlanError::GeneratedSourceProducerOutsideClosure {
                    action: Box::new(action.key.clone()),
                    module: Box::new((*module_key).clone()),
                    path: Box::new(module_path.clone()),
                    producer: Box::new((*producer).clone()),
                });
            }
            for consumer_step in consumer_steps {
                let dependency_actions = dependency_action_closure(consumer_step, &step_by_key)?;
                if !dependency_actions.contains(producer) {
                    return Err(PlanError::GeneratedSourceProducerOutsideClosure {
                        action: Box::new(action.key.clone()),
                        module: Box::new((*module_key).clone()),
                        path: Box::new(module_path.clone()),
                        producer: Box::new((*producer).clone()),
                    });
                }
            }
        }
    }
    Ok(())
}

pub(super) fn validate_output_ownership(
    actions: &[PlanAction],
    artifacts: &[PlanArtifact],
) -> Result<(), PlanError> {
    let artifacts: BTreeMap<_, _> = artifacts
        .iter()
        .map(|artifact| (&artifact.key, artifact))
        .collect();
    let mut owners: BTreeMap<LogicalPath, ActionKey> = BTreeMap::new();
    for action in actions {
        for output in action_outputs(action, &artifacts)? {
            let output = output.path;
            if output.is_empty()
                || !matches!(output.root(), LogicalPathRoot::Build)
                || output.components().first().is_some_and(|component| {
                    component == crate::output_recovery::OUTPUT_TRANSACTION_DIRECTORY
                })
            {
                return Err(PlanError::InvalidOutput {
                    action: action.key.clone(),
                    path: output.clone(),
                });
            }
            // Output ownership is hierarchical: owning a path also owns every
            // descendant. Exact-path locks cannot serialize actions whose
            // separately declared outputs overlap in the physical build tree.
            if let Some((owned, first)) = owners.iter().find(|(owned, _)| owned.overlaps(output)) {
                let collision = if owned.components().len() >= output.components().len() {
                    (*owned).clone()
                } else {
                    output.clone()
                };
                return Err(PlanError::OutputCollision(Box::new(OutputCollision {
                    path: collision,
                    first: first.clone(),
                    second: action.key.clone(),
                })));
            }
            owners.insert(output.clone(), action.key.clone());
        }
    }
    Ok(())
}

#[derive(Clone, Copy)]
struct ActionOutput<'a> {
    path: &'a LogicalPath,
    is_directory: bool,
}

impl ActionOutput<'_> {
    fn produces(self, input: &LogicalPath) -> bool {
        self.path == input
            || (self.is_directory
                && self.path.root() == input.root()
                && input.components().starts_with(self.path.components()))
    }
}

fn action_outputs<'a>(
    action: &'a PlanAction,
    artifacts: &BTreeMap<&ArtifactKey, &'a PlanArtifact>,
) -> Result<Vec<ActionOutput<'a>>, PlanError> {
    match &action.kind {
        ActionKind::CompilerEmit { artifact, .. } => {
            let Some(emitted) = artifacts.get(artifact) else {
                return Err(PlanError::MissingArtifact {
                    action: action.key.clone(),
                    artifact: artifact.clone(),
                });
            };
            Ok(vec![ActionOutput {
                path: &emitted.output,
                is_directory: emitted.kind == PlanArtifactKind::ObjectSet,
            }])
        }
        ActionKind::ExternalCommand { outputs, .. } => Ok(outputs
            .iter()
            .map(|path| ActionOutput {
                path,
                is_directory: false,
            })
            .collect()),
        ActionKind::GeneratedFile { output, .. } => Ok(vec![ActionOutput {
            path: output,
            is_directory: false,
        }]),
        ActionKind::InstallArtifact { destination, .. } => Ok(vec![ActionOutput {
            path: destination,
            is_directory: false,
        }]),
        _ => Ok(Vec::new()),
    }
}
