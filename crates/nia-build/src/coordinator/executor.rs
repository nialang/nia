// SPDX-License-Identifier: GPL-3.0-or-later
//! Typed build-action execution and cache interaction.
//!
//! Every action acquires its complete canonical output-lock set before cache
//! restore or execution. Compiler cache lookup uses a precheck manifest, while
//! publication uses the manifest returned by the completed compiler request.

use super::*;
use crate::action_cache::CompilerCheckCacheIdentityInput;
use crate::lock::{LockWaitEvent, LockWaitObserver};
use crate::progress::{ProgressActivity, ProgressSession, ProgressWaitKind, ProgressWaitOutcome};
use crate::{insert_module_root, manifest_module_roots};
use nia_package_metadata::DependencyScope;
use std::io::{Seek as _, SeekFrom};

#[cfg(unix)]
use std::os::unix::fs::OpenOptionsExt as _;

#[derive(Clone)]
pub(super) struct DriverActionExecutor {
    plan: Arc<BuildPlan>,
    invocation: Arc<BuildInvocation>,
    drivers: Arc<BTreeMap<TargetConfig, Arc<Driver>>>,
    package_dependency_selection: Arc<BTreeMap<PackageKey, bool>>,
    process_output_writer: Option<Arc<dyn crate::ProcessOutputWriter>>,
    progress: Option<ProgressSession>,
}

impl DriverActionExecutor {
    #[cfg(test)]
    pub(super) fn new(plan: BuildPlan, invocation: BuildInvocation) -> Self {
        Self::new_with_output_writer(plan, invocation, None)
    }

    #[cfg(test)]
    pub(super) fn new_with_output_writer(
        plan: BuildPlan,
        invocation: BuildInvocation,
        process_output_writer: Option<Arc<dyn crate::ProcessOutputWriter>>,
    ) -> Self {
        Self::new_with_output_writer_and_progress(plan, invocation, process_output_writer, None)
    }

    pub(super) fn new_with_output_writer_and_progress(
        plan: BuildPlan,
        invocation: BuildInvocation,
        process_output_writer: Option<Arc<dyn crate::ProcessOutputWriter>>,
        progress: Option<ProgressSession>,
    ) -> Self {
        let plan = Arc::new(plan);
        let invocation = Arc::new(invocation);
        let mut drivers = BTreeMap::new();
        for action in plan.actions() {
            let target = match &action.kind {
                ActionKind::CompilerCheck { target, .. }
                | ActionKind::CompilerEmit { target, .. } => target,
                _ => continue,
            };
            drivers.entry(*target).or_insert_with(|| {
                Arc::new(Driver::with_config(
                    DriverConfig {
                        artifact_cache_dir: Some(invocation.cache_dir.clone()),
                        ..DriverConfig::new(Arc::clone(&invocation.toolchain))
                    }
                    .with_artifact_target(*target),
                ))
            });
        }
        let package_dependency_selection = super::package_dependency_selection(&plan, &invocation);
        Self {
            plan,
            invocation,
            drivers: Arc::new(drivers),
            package_dependency_selection: Arc::new(package_dependency_selection),
            process_output_writer,
            progress,
        }
    }

    pub(super) fn execute(
        &self,
        action: &PlanAction,
        cancellation: &ActionCancellation,
    ) -> Result<Option<ActionCacheOutcome>, CoordinatorError> {
        // Locks cover cache restoration as well as fresh publication, so a hit
        // and a miss cannot race to make different bytes visible.
        let Some(_output_locks) = self.acquire_output_locks(action, cancellation)? else {
            return Err(CoordinatorError::Cancelled {
                action: action.key.clone(),
            });
        };
        if cancellation.is_cancelled() {
            return Err(CoordinatorError::Cancelled {
                action: action.key.clone(),
            });
        }
        self.execute_with_output_ownership(action, cancellation)
    }

    fn execute_with_output_ownership(
        &self,
        action: &PlanAction,
        cancellation: &ActionCancellation,
    ) -> Result<Option<ActionCacheOutcome>, CoordinatorError> {
        let result = match &action.kind {
            ActionKind::Aggregate => Ok(()),
            ActionKind::CompilerCheck {
                module,
                target,
                runtime,
            } => {
                return self.execute_compiler_check(action, module, target, *runtime);
            }
            ActionKind::CompilerEmit {
                artifact,
                target,
                static_archives,
            } => {
                return self.execute_compiler_emit(action, artifact, target, static_archives);
            }
            ActionKind::ExternalCommand(command) => {
                return self.execute_external_command_action(action, command, cancellation);
            }
            ActionKind::TestExecutable(command) => {
                return self.execute_test_suite_action(action, command, cancellation);
            }
            ActionKind::GeneratedFile { output, contents } => {
                return self.execute_generated_file(action, output, contents);
            }
            ActionKind::InstallArtifact {
                artifact,
                destination,
            } => {
                return self.execute_install_artifact(action, artifact, destination);
            }
            ActionKind::Uncacheable { .. } => Err(unsupported(action, "uncacheable")),
        };
        result.map(|()| None)
    }

    fn execute_test_suite_action(
        &self,
        action: &PlanAction,
        command: &CommandAction,
        cancellation: &ActionCancellation,
    ) -> Result<Option<ActionCacheOutcome>, CoordinatorError> {
        if command.cache_policy != CommandCachePolicy::SuccessfulTests {
            return self.execute_external_command_action(action, command, cancellation);
        }
        let Some(identity) = self.test_suite_cache_identity(action, command)? else {
            return self.execute_external_command_action(action, command, cancellation);
        };
        let cache = TestSuiteCache::new(self.invocation.cache_dir.clone());
        let _cache_lock = match cache.acquire_interruptible_observed(
            identity,
            || cancellation.is_cancelled(),
            self.lock_observer(action, ProgressWaitKind::CacheLock),
        ) {
            Ok(Some(lock)) => lock,
            Ok(None) => {
                return Err(CoordinatorError::Cancelled {
                    action: action.key.clone(),
                });
            }
            Err(_) => {
                self.execute_external_command_action(action, command, cancellation)?;
                return Ok(Some(ActionCacheOutcome::Miss(
                    ActionCacheMissReason::WriteError,
                )));
            }
        };
        // Inputs can change while another invocation owns this identity's lock.
        // Never accept its completion record using the pre-wait observation.
        let Some(current_identity) = self.test_suite_cache_identity(action, command)? else {
            return self.execute_external_command_action(action, command, cancellation);
        };
        if current_identity != identity {
            return self.execute_external_command_action(action, command, cancellation);
        }
        if cancellation.is_cancelled() {
            return Err(CoordinatorError::Cancelled {
                action: action.key.clone(),
            });
        }
        let miss_reason = match cache.lookup_locked(identity) {
            Ok(TestSuiteCacheLookup::Hit) => {
                self.observe_cache_decision(action, ActionCacheOutcome::Hit);
                return Ok(Some(ActionCacheOutcome::Hit));
            }
            Ok(TestSuiteCacheLookup::Miss(reason)) => reason,
            Err(_) => ActionCacheMissReason::ReadError,
        };
        self.observe_cache_decision(action, ActionCacheOutcome::Miss(miss_reason.clone()));
        self.execute_external_command_action(action, command, cancellation)?;
        if cancellation.is_cancelled() {
            return Err(CoordinatorError::Cancelled {
                action: action.key.clone(),
            });
        }
        let Some(current_identity) = self.test_suite_cache_identity(action, command)? else {
            return Ok(Some(ActionCacheOutcome::Miss(
                ActionCacheMissReason::Uncacheable,
            )));
        };
        if current_identity != identity {
            return Ok(Some(ActionCacheOutcome::Miss(
                ActionCacheMissReason::Uncacheable,
            )));
        }
        let reason = match cache.publish_locked(identity) {
            Ok(()) => miss_reason,
            Err(_) => ActionCacheMissReason::WriteError,
        };
        Ok(Some(ActionCacheOutcome::Miss(reason)))
    }

    fn test_suite_cache_identity(
        &self,
        action: &PlanAction,
        command: &CommandAction,
    ) -> Result<Option<TestSuiteCacheIdentity>, CoordinatorError> {
        let CommandProgram::Path(program) = &command.program else {
            return Ok(None);
        };
        let LogicalPathRoot::Artifact(artifact_key) = program.root() else {
            return Ok(None);
        };
        let artifact = self.artifact(action, artifact_key)?;
        let module = find_module(self.plan.modules(), &artifact.root_module).ok_or_else(|| {
            inconsistent(
                format!("action `{}`", action.key.name()),
                "artifact root module".to_string(),
            )
        })?;
        let program_path = self.resolve_path(action, program)?;
        let tool = read_external_identity_file(action, &program_path, "read test executable")?;
        let target = self.plan.host_target();
        let request =
            self.check_request(action, &artifact.root_module, target, artifact.runtime)?;
        // A suite owns its source closure. Sharing the target's mutable driver
        // lets concurrent suites replace each other's active loader entry.
        let driver = Driver::with_config(
            DriverConfig {
                artifact_cache_dir: Some(self.invocation.cache_dir.clone()),
                ..DriverConfig::new(Arc::clone(&self.invocation.toolchain))
            }
            .with_artifact_target(*target),
        );
        let manifest = driver
            .source_input_manifest(&request)
            .result
            .map_err(|error| CoordinatorError::Driver {
                action: action.key.clone(),
                error: Box::new(error),
            })?;
        if manifest.fingerprint().is_none() {
            return Ok(None);
        }
        let environment =
            effective_test_suite_environment(command.environment_policy, &command.environment);
        let Some(environment) = environment else {
            return Ok(None);
        };
        let mut encoded = Vec::new();
        append_test_suite_action_identity(&mut encoded, action, command, artifact);
        append_test_suite_bytes(&mut encoded, target.name().as_bytes());
        encoded.push(profile_tag(self.invocation.profile));
        encoded.push(compilation_mode_tag(self.invocation.compilation_mode));
        encoded.push(optimization_tag(module.optimization));
        encoded.push(runtime_tag(artifact.runtime));
        encoded.push(link_time_optimization_tag(
            self.invocation.link_time_optimization,
        ));
        let mut package_paths = vec![
            program,
            &command.working_directory,
            &artifact.output,
            &module.root_source,
        ];
        package_paths.extend(
            module
                .imports
                .iter()
                .filter(|import| self.package_dependency_selected(import.path.root()))
                .map(|import| &import.path),
        );
        package_paths.extend(command.inputs.iter());
        for input in &command.inputs {
            if let LogicalPathRoot::Artifact(key) = input.root() {
                package_paths.push(&self.artifact(action, key)?.output);
            }
        }
        let Some(package_roots) =
            crate::action_cache::package_roots_identity(self.plan.packages(), package_paths)
        else {
            return Ok(None);
        };
        append_test_suite_bytes(&mut encoded, &package_roots);
        append_test_suite_bytes(&mut encoded, &(command.inputs.len() as u64).to_le_bytes());
        for input in &command.inputs {
            append_test_suite_path(&mut encoded, input);
            let resolved = self.resolve_path(action, input)?;
            let contents = read_external_identity_input(action, &resolved, "read test input")?;
            contents.append_to(&mut encoded);
        }
        let toolchain = self.invocation.toolchain.identity();
        append_test_suite_bytes(&mut encoded, toolchain.compiler_version().as_bytes());
        encoded.extend_from_slice(&toolchain.protocol_epoch().to_le_bytes());
        tool.append_to(&mut encoded);
        append_test_suite_bytes(&mut encoded, &(environment.len() as u64).to_le_bytes());
        for (name, value) in environment {
            append_test_suite_bytes(&mut encoded, &name);
            append_test_suite_bytes(&mut encoded, &value);
        }
        append_test_suite_bytes(
            &mut encoded,
            &(manifest.sources().len() as u64).to_le_bytes(),
        );
        for source in manifest.sources() {
            append_test_suite_bytes(
                &mut encoded,
                source.path.identity().normalized_path().as_bytes(),
            );
            match source.content {
                SourceInputContent::Missing => return Ok(None),
                SourceInputContent::Present {
                    fingerprint,
                    byte_len,
                } => {
                    for part in fingerprint.parts() {
                        encoded.extend_from_slice(&part.to_le_bytes());
                    }
                    encoded.extend_from_slice(&(byte_len as u64).to_le_bytes());
                }
            }
        }
        Ok(Some(TestSuiteCacheIdentity::from_bytes(&encoded)))
    }

    fn execute_external_command_action(
        &self,
        action: &PlanAction,
        command: &CommandAction,
        cancellation: &ActionCancellation,
    ) -> Result<Option<ActionCacheOutcome>, CoordinatorError> {
        let CommandAction {
            resource_class: _,
            environment_policy,
            cache_policy,
            program,
            arguments,
            working_directory: logical_working_directory,
            environment,
            inputs,
            outputs,
            stdin,
        } = command;
        let (environment_policy, cache_policy) = (*environment_policy, *cache_policy);
        let working_directory = self.resolve_path(action, logical_working_directory)?;
        let resolved_inputs = inputs
            .iter()
            .map(|input| self.resolve_path(action, input).map(|path| (input, path)))
            .collect::<Result<Vec<_>, _>>()?;
        let resolved_stdin = match stdin {
            CommandStdin::Null => None,
            CommandStdin::InputPath(path) => Some(
                resolved_inputs
                    .iter()
                    .find(|(input, _)| *input == path)
                    .map(|(_, resolved)| resolved.as_path())
                    .ok_or_else(|| {
                        inconsistent(
                            format!("action `{}`", action.key.name()),
                            "declared command stdin binding".to_string(),
                        )
                    })?,
            ),
        };
        let resolved_outputs = outputs
            .iter()
            .map(|output| self.resolve_path(action, output).map(|path| (output, path)))
            .collect::<Result<Vec<_>, _>>()?;
        let cacheable = !matches!(
            cache_policy,
            CommandCachePolicy::Uncacheable | CommandCachePolicy::SuccessfulTests
        );
        // Every searched program resolves through the same lookup, so a
        // command's declared `PATH`/`PATHEXT` select the file it runs whether
        // or not the step is cached.
        let resolved_program = match program {
            CommandProgram::Path(path) => self.resolve_path(action, path)?,
            CommandProgram::Search(name) => {
                resolve_search_program(action, name, &working_directory, environment)?
            }
        };
        let program_text = path_text(action, &resolved_program)?;

        let cache = ExternalCommandCache::new_with_lock_observer(
            self.invocation.cache_dir.clone(),
            self.cache_lock_observer(action),
        );
        let environment_snapshot = (cache_policy == CommandCachePolicy::FullDeclaredEnvironment)
            .then(snapshot_process_environment)
            .flatten();
        let cacheable = if cache_policy == CommandCachePolicy::FullDeclaredEnvironment {
            environment_snapshot.is_some()
        } else {
            cacheable
        };
        let mut cache_identity = if cacheable {
            Some(self.external_command_cache_identity(
                action,
                command,
                &resolved_inputs,
                &resolved_program,
                environment_snapshot.as_deref(),
            )?)
        } else {
            None
        };
        let mut miss_reason = None;
        if let Some(identity) = cache_identity.as_ref() {
            match cache.lookup(identity) {
                Ok(ExternalCommandCacheLookup::Hit(hit)) => {
                    self.observe_cache_decision(action, ActionCacheOutcome::Hit);
                    restore_cached_external_outputs(
                        action,
                        &self.invocation.build_dir,
                        &resolved_outputs,
                        hit,
                    )?;
                    return Ok(Some(ActionCacheOutcome::Hit));
                }
                Ok(ExternalCommandCacheLookup::Miss(reason)) => miss_reason = Some(reason),
                Err(_) => miss_reason = Some(ActionCacheMissReason::ReadError),
            }
        }

        if cache_policy != CommandCachePolicy::SuccessfulTests {
            self.observe_cache_decision(
                action,
                ActionCacheOutcome::Miss(
                    miss_reason
                        .clone()
                        .unwrap_or(ActionCacheMissReason::Uncacheable),
                ),
            );
        }
        let mut staged = if resolved_outputs.is_empty() {
            None
        } else {
            Some(prepare_staged_outputs(
                action,
                &self.invocation.build_dir,
                &resolved_outputs,
            )?)
        };
        let resolved_arguments = arguments
            .iter()
            .map(|argument| match argument {
                CommandArgument::Literal(value) => Ok(value.clone()),
                CommandArgument::InputPath(path) => {
                    let (_, resolved) = resolved_inputs
                        .iter()
                        .find(|(input, _)| *input == path)
                        .ok_or_else(|| {
                            inconsistent(
                                format!("action `{}`", action.key.name()),
                                "declared command input binding".to_string(),
                            )
                        })?;
                    path_text(action, resolved)
                }
                CommandArgument::OutputPath(path) => {
                    let Some(index) = resolved_outputs
                        .iter()
                        .position(|(output, _)| *output == path)
                    else {
                        return Err(inconsistent(
                            format!("action `{}`", action.key.name()),
                            "matching command output binding".to_string(),
                        ));
                    };
                    let staged = staged.as_ref().ok_or_else(|| {
                        inconsistent(
                            format!("action `{}`", action.key.name()),
                            "declared command output transaction".to_string(),
                        )
                    })?;
                    path_text(action, &staged.outputs[index].temporary)
                }
            })
            .collect::<Result<Vec<_>, CoordinatorError>>();
        let resolved_arguments = match resolved_arguments {
            Ok(arguments) => arguments,
            Err(cause) => {
                return match staged.take() {
                    Some(staged) => cleanup_staged_outputs(action, staged, Some(Box::new(cause))),
                    None => Err(cause),
                }
                .map(|()| None);
            }
        };
        let execution = execute_external_command_with_writer(
            action,
            ResolvedExternalCommand {
                program: &program_text,
                arguments: &resolved_arguments,
                working_directory: &working_directory,
                environment_policy,
                environment,
                stdin: resolved_stdin,
            },
            ExternalExecutionPolicy {
                timeout: EXTERNAL_COMMAND_TIMEOUT,
                forward_output: self.invocation.forward_output,
                cancellation: Some(cancellation),
            },
            self.process_output_writer.clone(),
        );
        let mut cache_outputs = match (execution, staged.as_ref()) {
            (Ok(()), Some(_)) if cancellation.is_cancelled() => {
                let cause = CoordinatorError::ExternalCommand(Box::new(ExternalCommandError {
                    action: action.key.clone(),
                    program: program_text,
                    arguments: resolved_arguments,
                    working_directory,
                    failure: ExternalCommandFailure::Cancelled {
                        stdout: Vec::new(),
                        stderr: Vec::new(),
                    },
                }));
                let staged = take_staged_output_transaction(action, &mut staged)?;
                return cleanup_staged_outputs(action, staged, Some(Box::new(cause)))
                    .map(|()| None);
            }
            (Ok(()), Some(staged_output)) if cacheable => {
                match open_staged_external_outputs(action, staged_output) {
                    Ok(outputs) => Some(outputs),
                    Err(cause) => {
                        let staged = take_staged_output_transaction(action, &mut staged)?;
                        return cleanup_staged_outputs(action, staged, Some(Box::new(cause)))
                            .map(|()| None);
                    }
                }
            }
            (Ok(()), Some(_)) => None,
            (Ok(()), None) => None,
            (Err(cause), Some(_)) => {
                let staged = take_staged_output_transaction(action, &mut staged)?;
                return cleanup_staged_outputs(action, staged, Some(Box::new(cause)))
                    .map(|()| None);
            }
            (Err(cause), None) => return Err(cause),
        };
        if let Some(staged) = staged.take() {
            publish_staged_outputs(action, staged)?;
        }
        let Some(identity) = cache_identity.take() else {
            return Ok((cache_policy != CommandCachePolicy::Uncacheable)
                .then_some(ActionCacheOutcome::Miss(ActionCacheMissReason::Uncacheable)));
        };
        let current_identity = match self.external_command_cache_identity(
            action,
            command,
            &resolved_inputs,
            &resolved_program,
            environment_snapshot.as_deref(),
        ) {
            Ok(identity) => identity,
            Err(_) => {
                return Ok(Some(ActionCacheOutcome::Miss(
                    ActionCacheMissReason::Uncacheable,
                )));
            }
        };
        let reason = if current_identity != identity {
            ActionCacheMissReason::Uncacheable
        } else {
            match cache.publish(&identity, cache_outputs.as_deref_mut().unwrap_or_default()) {
                Ok(()) => miss_reason.unwrap_or(ActionCacheMissReason::NotFound),
                Err(_) => ActionCacheMissReason::WriteError,
            }
        };
        Ok(Some(ActionCacheOutcome::Miss(reason)))
    }

    fn external_command_cache_identity(
        &self,
        action: &PlanAction,
        command: &CommandAction,
        resolved_inputs: &[(&LogicalPath, PathBuf)],
        resolved_program: &Path,
        environment_snapshot: Option<&[(Vec<u8>, Vec<u8>)]>,
    ) -> Result<ExternalCommandCacheIdentity, CoordinatorError> {
        let tool_contents = read_external_identity_file(action, resolved_program, "read tool")?;
        let inputs = resolved_inputs
            .iter()
            .map(|(logical, path)| {
                read_external_identity_input(action, path, "read declared")
                    .map(|contents| ((*logical).clone(), contents))
            })
            .collect::<Result<Vec<_>, _>>()?;
        ExternalCommandCacheIdentity::new(
            &action.key,
            command,
            &inputs,
            self.plan.packages(),
            tool_contents,
            self.invocation.toolchain.identity(),
            environment_snapshot,
        )
        .ok_or_else(|| {
            inconsistent(
                format!("action `{}`", action.key.name()),
                "package roots".to_string(),
            )
        })
    }

    fn execute_compiler_check(
        &self,
        action: &PlanAction,
        module_key: &ModuleKey,
        target: &TargetConfig,
        runtime: Runtime,
    ) -> Result<Option<ActionCacheOutcome>, CoordinatorError> {
        let module = find_module(self.plan.modules(), module_key).ok_or_else(|| {
            inconsistent(
                format!("action `{}`", action.key.name()),
                format!("module `{}`", module_key.name()),
            )
        })?;
        let request = self.check_request(action, module_key, target, runtime)?;
        let driver = self.driver(action, target)?;
        // The cheap manifest is sufficient for lookup. A successful check may
        // discover a different provider closure, so only its returned manifest
        // is authoritative for the record that gets published below.
        let manifest_activity = self
            .progress
            .as_ref()
            .map(|progress| progress.activity(action.key.clone(), ProgressActivity::Checking));
        let precheck_manifest = driver.source_input_manifest(&request).result;
        let manifest_success = precheck_manifest.is_ok();
        if let Some(activity) = manifest_activity {
            activity.finish(manifest_success);
        }
        let precheck_manifest = precheck_manifest.map_err(|error| CoordinatorError::Driver {
            action: action.key.clone(),
            error: Box::new(error),
        })?;
        let cache = CompilerCheckCache::new_with_lock_observer(
            self.invocation.cache_dir.clone(),
            self.cache_lock_observer(action),
        );
        let precheck_identity = CompilerCheckCacheIdentity::new(CompilerCheckCacheIdentityInput {
            action: &action.key,
            module,
            packages: self.plan.packages(),
            target,
            profile: self.invocation.profile,
            compilation_mode: self.invocation.compilation_mode,
            runtime,
            manifest: &precheck_manifest,
            toolchain: self.invocation.toolchain.identity(),
        });
        let miss_reason = match precheck_identity.as_ref() {
            None => ActionCacheMissReason::Uncacheable,
            Some(identity) => match cache.lookup(identity) {
                Ok(CompilerCheckCacheLookup::Hit) => {
                    self.observe_cache_decision(action, ActionCacheOutcome::Hit);
                    return Ok(Some(ActionCacheOutcome::Hit));
                }
                Ok(CompilerCheckCacheLookup::Miss(reason)) => reason,
                Err(_) => ActionCacheMissReason::ReadError,
            },
        };
        self.observe_cache_decision(action, ActionCacheOutcome::Miss(miss_reason.clone()));
        let check_activity = self
            .progress
            .as_ref()
            .map(|progress| progress.activity(action.key.clone(), ProgressActivity::Checking));
        let checked = driver.check_entry_with_source_manifest(request).result;
        let check_success = checked.is_ok();
        if let Some(activity) = check_activity {
            activity.finish(check_success);
        }
        let checked = checked.map_err(|error| CoordinatorError::Driver {
            action: action.key.clone(),
            error: Box::new(error),
        })?;
        if !checked.program.diagnostics.is_empty() {
            return Ok(Some(ActionCacheOutcome::Miss(
                ActionCacheMissReason::Uncacheable,
            )));
        }
        let Some(final_identity) =
            CompilerCheckCacheIdentity::new(CompilerCheckCacheIdentityInput {
                action: &action.key,
                module,
                packages: self.plan.packages(),
                target,
                profile: self.invocation.profile,
                compilation_mode: self.invocation.compilation_mode,
                runtime,
                manifest: &checked.source_manifest,
                toolchain: self.invocation.toolchain.identity(),
            })
        else {
            return Ok(Some(ActionCacheOutcome::Miss(
                ActionCacheMissReason::Uncacheable,
            )));
        };
        let reason = match cache.publish(&final_identity) {
            Ok(()) => miss_reason,
            Err(_) => ActionCacheMissReason::WriteError,
        };
        Ok(Some(ActionCacheOutcome::Miss(reason)))
    }

    fn execute_compiler_emit(
        &self,
        action: &PlanAction,
        artifact_key: &ArtifactKey,
        target: &TargetConfig,
        static_archives: &[ArtifactKey],
    ) -> Result<Option<ActionCacheOutcome>, CoordinatorError> {
        let artifact = self.artifact(action, artifact_key)?;
        match artifact.kind {
            PlanArtifactKind::ObjectSet => {
                return self.execute_object_set_emit(action, artifact, target);
            }
            PlanArtifactKind::StaticArchive => {
                return self.execute_static_archive_emit(action, artifact, target);
            }
            PlanArtifactKind::Executable => {}
        }
        let module = find_module(self.plan.modules(), &artifact.root_module).ok_or_else(|| {
            inconsistent(
                format!("action `{}`", action.key.name()),
                format!("module `{}`", artifact.root_module.name()),
            )
        })?;
        let request =
            self.check_request(action, &artifact.root_module, target, artifact.runtime)?;
        let output = self.resolve_path(action, &artifact.output)?;
        let driver = self.driver(action, target)?;
        let mut cache_link_inputs = Vec::with_capacity(static_archives.len());
        let mut linker_inputs = Vec::with_capacity(static_archives.len());
        for archive_key in static_archives {
            let archive = self.artifact(action, archive_key)?;
            let path = self.resolve_path(action, &archive.output)?;
            let mut options = fs::OpenOptions::new();
            options.read(true);
            #[cfg(unix)]
            options.custom_flags(libc::O_NOFOLLOW);
            let mut file = options.open(&path).map_err(|error| {
                CoordinatorError::StaticArchiveLinkInputIo {
                    action: action.key.clone(),
                    path: path.clone(),
                    operation: "read",
                    error,
                }
            })?;
            let metadata =
                file.metadata()
                    .map_err(|error| CoordinatorError::StaticArchiveLinkInputIo {
                        action: action.key.clone(),
                        path: path.clone(),
                        operation: "inspect",
                        error,
                    })?;
            if !metadata.is_file() {
                return Err(CoordinatorError::StaticArchiveLinkInputIo {
                    action: action.key.clone(),
                    path,
                    operation: "inspect",
                    error: io::Error::new(
                        io::ErrorKind::InvalidData,
                        "static archive link input must be a regular file",
                    ),
                });
            }
            let length = metadata.len();
            let cache_input =
                CompilerEmitCacheLinkInput::from_reader(archive_key.clone(), &mut file, length)
                    .map_err(|error| CoordinatorError::StaticArchiveLinkInputIo {
                        action: action.key.clone(),
                        path: path.clone(),
                        operation: "fingerprint",
                        error,
                    })?;
            file.seek(SeekFrom::Start(0)).map_err(|error| {
                CoordinatorError::StaticArchiveLinkInputIo {
                    action: action.key.clone(),
                    path: path.clone(),
                    operation: "rewind",
                    error,
                }
            })?;
            let linker_input = StaticArchiveLinkInput::from_reader(
                archive_key.package().as_str(),
                archive_key.name(),
                path.clone(),
                &mut file,
                length,
            )
            .map_err(|error| CoordinatorError::StaticArchiveLinkInputIo {
                action: action.key.clone(),
                path,
                operation: "fingerprint",
                error,
            })?;
            cache_link_inputs.push(cache_input);
            linker_inputs.push(linker_input);
        }
        let link_options = LinkOptions::default().with_static_archives(linker_inputs);
        // Lookup may use the precheck closure; publication must bind the
        // executable to the finalized source closure returned after linking.
        let precheck_manifest = driver
            .source_input_manifest(&request)
            .result
            .map_err(|error| CoordinatorError::Driver {
                action: action.key.clone(),
                error: Box::new(error),
            })?;
        let link_environment = driver.executable_cache_environment_for(&link_options);
        let cache = CompilerEmitCache::new_with_lock_observer(
            self.invocation.cache_dir.clone(),
            self.cache_lock_observer(action),
        );
        let precheck_identity = link_environment.and_then(|environment| {
            CompilerEmitCacheIdentity::new(CompilerEmitCacheIdentityInput {
                action: &action.key,
                artifact,
                module,
                packages: self.plan.packages(),
                target,
                profile: self.invocation.profile,
                compilation_mode: self.invocation.compilation_mode,
                runtime: artifact.runtime,
                manifest: &precheck_manifest,
                toolchain: self.invocation.toolchain.identity(),
                link_environment: environment,
                link_inputs: &cache_link_inputs,
                link_time_optimization: self.invocation.link_time_optimization,
            })
        });
        let miss_reason = match precheck_identity.as_ref() {
            None => ActionCacheMissReason::Uncacheable,
            Some(identity) => match cache.lookup(identity) {
                Ok(CompilerEmitCacheLookup::Hit(reference)) => {
                    let reason = match driver.restore_executable_cache(reference, &output) {
                        ExecutableCacheRestore::Hit => {
                            self.observe_cache_decision(action, ActionCacheOutcome::Hit);
                            return Ok(Some(ActionCacheOutcome::Hit));
                        }
                        ExecutableCacheRestore::NotFound => ActionCacheMissReason::NotFound,
                        ExecutableCacheRestore::Invalidated => {
                            ActionCacheMissReason::Invalidated(vec![
                                crate::ActionCacheInvalidation::Linker,
                            ])
                        }
                        ExecutableCacheRestore::Corrupt => ActionCacheMissReason::Corrupt,
                        ExecutableCacheRestore::ReadError => ActionCacheMissReason::ReadError,
                        ExecutableCacheRestore::Disabled => ActionCacheMissReason::Uncacheable,
                    };
                    if cache.retire(identity, reference).is_err() {
                        ActionCacheMissReason::ReadError
                    } else {
                        reason
                    }
                }
                Ok(CompilerEmitCacheLookup::Miss(reason)) => reason,
                Err(_) => ActionCacheMissReason::ReadError,
            },
        };
        self.observe_cache_decision(action, ActionCacheOutcome::Miss(miss_reason.clone()));
        let link_activity = self.progress.as_ref().map(|progress| {
            progress.activity(action.key.clone(), ProgressActivity::CompilingAndLinking)
        });
        let linked = driver
            .link_executable_with_source_manifest(
                LinkExecutableRequest::new(request, output)
                    .with_link_options(link_options.clone())
                    .with_link_time_optimization(self.invocation.link_time_optimization),
            )
            .result;
        let link_success = linked.is_ok();
        if let Some(activity) = link_activity {
            activity.finish(link_success);
        }
        let linked = linked.map_err(|error| CoordinatorError::Driver {
            action: action.key.clone(),
            error: Box::new(error),
        })?;
        if !linked.artifact.diagnostics.is_empty() {
            return Ok(Some(ActionCacheOutcome::Miss(
                ActionCacheMissReason::Uncacheable,
            )));
        }
        let Some(reference) = linked.artifact.cache_reference else {
            return Ok(Some(ActionCacheOutcome::Miss(
                ActionCacheMissReason::Uncacheable,
            )));
        };
        let Some(link_environment) = driver.executable_cache_environment_for(&link_options) else {
            return Ok(Some(ActionCacheOutcome::Miss(
                ActionCacheMissReason::Uncacheable,
            )));
        };
        let Some(final_identity) = CompilerEmitCacheIdentity::new(CompilerEmitCacheIdentityInput {
            action: &action.key,
            artifact,
            module,
            packages: self.plan.packages(),
            target,
            profile: self.invocation.profile,
            compilation_mode: self.invocation.compilation_mode,
            runtime: artifact.runtime,
            manifest: &linked.source_manifest,
            toolchain: self.invocation.toolchain.identity(),
            link_environment,
            link_inputs: &cache_link_inputs,
            link_time_optimization: self.invocation.link_time_optimization,
        }) else {
            return Ok(Some(ActionCacheOutcome::Miss(
                ActionCacheMissReason::Uncacheable,
            )));
        };
        let reason = match cache.publish(&final_identity, reference) {
            Ok(()) => miss_reason,
            Err(_) => ActionCacheMissReason::WriteError,
        };
        Ok(Some(ActionCacheOutcome::Miss(reason)))
    }

    fn execute_object_set_emit(
        &self,
        action: &PlanAction,
        artifact: &PlanArtifact,
        target: &TargetConfig,
    ) -> Result<Option<ActionCacheOutcome>, CoordinatorError> {
        let _module = find_module(self.plan.modules(), &artifact.root_module).ok_or_else(|| {
            inconsistent(
                format!("action `{}`", action.key.name()),
                format!("module `{}`", artifact.root_module.name()),
            )
        })?;
        let request =
            self.check_request(action, &artifact.root_module, target, artifact.runtime)?;
        let output = self.resolve_path(action, &artifact.output)?;
        let driver = self.driver(action, target)?;
        let emit_activity = self
            .progress
            .as_ref()
            .map(|progress| progress.activity(action.key.clone(), ProgressActivity::Codegen));
        let emitted = driver
            .emit_native_objects(EmitObjectRequest::new(request))
            .result;
        let emit_success = emitted.is_ok();
        if let Some(activity) = emit_activity {
            activity.finish(emit_success);
        }
        let emitted = emitted.map_err(|error| CoordinatorError::Driver {
            action: action.key.clone(),
            error: Box::new(error),
        })?;
        let resolved = [ResolvedTransactionOutput {
            logical: &artifact.output,
            destination: output,
            kind: TransactionOutputKind::Directory,
        }];
        let staged = prepare_typed_staged_outputs(action, &self.invocation.build_dir, &resolved)?;
        let temporary = staged.outputs[0].temporary.clone();
        let written = driver
            .write_native_objects_from_artifact(&emitted, ObjectOutput::Directory(temporary))
            .result
            .map_err(|error| CoordinatorError::Driver {
                action: action.key.clone(),
                error: Box::new(error),
            });
        if let Err(error) = written {
            return cleanup_staged_outputs(action, staged, Some(Box::new(error))).map(|()| None);
        }
        publish_staged_outputs(action, staged)?;
        Ok(None)
    }

    fn execute_static_archive_emit(
        &self,
        action: &PlanAction,
        artifact: &PlanArtifact,
        target: &TargetConfig,
    ) -> Result<Option<ActionCacheOutcome>, CoordinatorError> {
        let _module = find_module(self.plan.modules(), &artifact.root_module).ok_or_else(|| {
            inconsistent(
                format!("action `{}`", action.key.name()),
                format!("module `{}`", artifact.root_module.name()),
            )
        })?;
        let request =
            self.check_request(action, &artifact.root_module, target, artifact.runtime)?;
        let output = self.resolve_path(action, &artifact.output)?;
        let driver = self.driver(action, target)?;
        let emitted = driver
            .emit_native_objects(EmitObjectRequest::new(request))
            .result
            .map_err(|error| CoordinatorError::Driver {
                action: action.key.clone(),
                error: Box::new(error),
            })?;
        let resolved = [ResolvedTransactionOutput {
            logical: &artifact.output,
            destination: output,
            kind: TransactionOutputKind::File,
        }];
        let staged = prepare_typed_staged_outputs(action, &self.invocation.build_dir, &resolved)?;
        let temporary = staged.outputs[0].temporary.clone();
        let archive_activity = self
            .progress
            .as_ref()
            .map(|progress| progress.activity(action.key.clone(), ProgressActivity::Linking));
        let archived = driver
            .archive_static_library_from_objects(&emitted, temporary, ArchiveOptions::default())
            .result;
        let archive_success = archived.is_ok();
        if let Some(activity) = archive_activity {
            activity.finish(archive_success);
        }
        let archived = archived.map_err(|error| CoordinatorError::Driver {
            action: action.key.clone(),
            error: Box::new(error),
        });
        if let Err(error) = archived {
            return cleanup_staged_outputs(action, staged, Some(Box::new(error))).map(|()| None);
        }
        publish_staged_outputs(action, staged)?;
        Ok(None)
    }

    fn execute_generated_file(
        &self,
        action: &PlanAction,
        logical_output: &LogicalPath,
        contents: &[u8],
    ) -> Result<Option<ActionCacheOutcome>, CoordinatorError> {
        let output = self.resolve_path(action, logical_output)?;
        let cache = GeneratedFileCache::new_with_lock_observer(
            self.invocation.cache_dir.clone(),
            self.cache_lock_observer(action),
        );
        let identity = GeneratedFileCacheIdentity::new(
            &action.key,
            logical_output,
            contents,
            self.invocation.toolchain.identity(),
        );
        let lookup = match cache.lookup(&identity) {
            Ok(lookup) => lookup,
            Err(_) => {
                self.observe_cache_decision(
                    action,
                    ActionCacheOutcome::Miss(ActionCacheMissReason::ReadError),
                );
                write_generated_file(action, &self.invocation.build_dir, &output, contents)?;
                return Ok(Some(ActionCacheOutcome::Miss(
                    ActionCacheMissReason::ReadError,
                )));
            }
        };
        match lookup {
            GeneratedFileCacheLookup::Hit(payload) => {
                self.observe_cache_decision(action, ActionCacheOutcome::Hit);
                write_generated_file(action, &self.invocation.build_dir, &output, &payload)?;
                Ok(Some(ActionCacheOutcome::Hit))
            }
            GeneratedFileCacheLookup::Miss(reason) => {
                self.observe_cache_decision(action, ActionCacheOutcome::Miss(reason.clone()));
                write_generated_file(action, &self.invocation.build_dir, &output, contents)?;
                let reason = match cache.publish(&identity, contents) {
                    Ok(()) => reason,
                    Err(_) => ActionCacheMissReason::WriteError,
                };
                Ok(Some(ActionCacheOutcome::Miss(reason)))
            }
        }
    }

    fn observe_cache_decision(&self, action: &PlanAction, cache: ActionCacheOutcome) {
        if let Some(progress) = &self.progress {
            progress.emit(crate::ProgressEventKind::ActionCacheDecision {
                action: action.key.clone(),
                cache,
            });
        }
    }

    fn execute_install_artifact(
        &self,
        action: &PlanAction,
        artifact_key: &ArtifactKey,
        logical_destination: &LogicalPath,
    ) -> Result<Option<ActionCacheOutcome>, CoordinatorError> {
        let artifact = self.artifact(action, artifact_key)?;
        if !matches!(
            artifact.kind,
            PlanArtifactKind::Executable | PlanArtifactKind::StaticArchive
        ) {
            return Err(unsupported(action, "install-non-file-artifact"));
        }
        let source = self.resolve_path(action, &artifact.output)?;
        let destination = self.resolve_path(action, logical_destination)?;
        let resolved = [(logical_destination, destination)];
        let staged = prepare_staged_outputs(action, &self.invocation.build_dir, &resolved)?;
        let temporary = staged.outputs[0].temporary.clone();
        if let Err(error) = fs::copy(&source, &temporary) {
            return cleanup_staged_outputs(
                action,
                staged,
                Some(Box::new(install_artifact_io(
                    action, &source, "copy", error,
                ))),
            )
            .map(|()| None);
        }
        if let Err(error) = nia_compat::sync_file(&temporary) {
            return cleanup_staged_outputs(
                action,
                staged,
                Some(Box::new(install_artifact_io(
                    action,
                    &temporary,
                    "sync staged",
                    error,
                ))),
            )
            .map(|()| None);
        }
        publish_staged_outputs(action, staged)?;
        Ok(None)
    }

    fn acquire_output_locks(
        &self,
        action: &PlanAction,
        cancellation: &ActionCancellation,
    ) -> Result<Option<Vec<ScopedFileLock>>, CoordinatorError> {
        let mut outputs: Vec<&LogicalPath> = match &action.kind {
            ActionKind::CompilerEmit { artifact, .. } => {
                vec![&self.artifact(action, artifact)?.output]
            }
            ActionKind::ExternalCommand(CommandAction { outputs, .. })
            | ActionKind::TestExecutable(CommandAction { outputs, .. }) => outputs.iter().collect(),
            ActionKind::GeneratedFile { output, .. } => vec![output],
            ActionKind::InstallArtifact { destination, .. } => vec![destination],
            _ => Vec::new(),
        };
        // Canonical acquisition order prevents two multi-output actions from
        // deadlocking while still allowing disjoint actions to proceed.
        outputs.sort();
        outputs.dedup();
        let mut acquired = Vec::with_capacity(outputs.len());
        for output in outputs {
            let resolved = self.resolve_path(action, output)?;
            let lock = output_lock_path(&self.invocation.cache_dir, output);
            let Some(output_lock) = ScopedFileLock::acquire_interruptible_observed(
                lock.clone(),
                || cancellation.is_cancelled(),
                self.lock_observer(action, ProgressWaitKind::OutputLock),
            )
            .map_err(|error| CoordinatorError::AcquireOutputLock {
                action: action.key.clone(),
                output: resolved,
                lock,
                error,
            })?
            else {
                return Ok(None);
            };
            acquired.push(output_lock);
        }
        Ok(Some(acquired))
    }

    fn lock_observer(
        &self,
        action: &PlanAction,
        kind: ProgressWaitKind,
    ) -> impl FnMut(LockWaitEvent) + '_ {
        let action = action.key.clone();
        let mut wait = None;
        move |event| match event {
            LockWaitEvent::Contended => {
                wait = self
                    .progress
                    .as_ref()
                    .map(|progress| progress.wait(action.clone(), kind));
            }
            event => {
                if let Some(wait) = wait.take() {
                    wait.finish(match event {
                        LockWaitEvent::Acquired => ProgressWaitOutcome::Acquired,
                        LockWaitEvent::Cancelled => ProgressWaitOutcome::Cancelled,
                        _ => ProgressWaitOutcome::Failed,
                    });
                }
            }
        }
    }

    fn cache_lock_observer(&self, action: &PlanAction) -> Option<LockWaitObserver> {
        let progress = self.progress.clone()?;
        let action = action.key.clone();
        let wait = Arc::new(std::sync::Mutex::new(None));
        let wait_for_callback = Arc::clone(&wait);
        Some(LockWaitObserver::new(Arc::new(move |event| {
            let Ok(mut current) = wait_for_callback.lock() else {
                return;
            };
            match event {
                LockWaitEvent::Contended => {
                    if current.is_none() {
                        *current = Some(progress.wait(action.clone(), ProgressWaitKind::CacheLock));
                    }
                }
                LockWaitEvent::Acquired | LockWaitEvent::Cancelled | LockWaitEvent::Failed => {
                    if let Some(wait) = current.take() {
                        wait.finish(match event {
                            LockWaitEvent::Acquired => ProgressWaitOutcome::Acquired,
                            LockWaitEvent::Cancelled => ProgressWaitOutcome::Cancelled,
                            LockWaitEvent::Failed => ProgressWaitOutcome::Failed,
                            LockWaitEvent::Contended => unreachable!(),
                        });
                    }
                }
            }
        })))
    }

    pub(super) fn check_request(
        &self,
        action: &PlanAction,
        module_key: &ModuleKey,
        target: &TargetConfig,
        runtime: Runtime,
    ) -> Result<CheckRequest, CoordinatorError> {
        let module = find_module(self.plan.modules(), module_key).ok_or_else(|| {
            inconsistent(
                format!("action `{}`", action.key.name()),
                format!("module `{}`", module_key.name()),
            )
        })?;
        let entry = self.resolve_source_path(action, &module.root_source)?;
        let mut module_map = ModuleMap::new();
        if module.key.package() == self.plan.root_package() {
            let imports = manifest_module_roots(
                &self.invocation.package_graph_report,
                &[DependencyScope::Runtime, DependencyScope::Development],
                self.invocation.compilation_mode,
            )
            .map_err(|error| CoordinatorError::Internal(nia_ice::Ice::new(error.to_string())))?;
            for (name, path) in imports {
                insert_module_root(&mut module_map, &name, path).map_err(|reason| {
                    CoordinatorError::InvalidModuleImport(Box::new(InvalidModuleImport {
                        action: action.key.clone(),
                        module: module.key.clone(),
                        name,
                        reason,
                    }))
                })?;
            }
        }
        for import in &module.imports {
            if !self.package_dependency_selected(import.path.root()) {
                continue;
            }
            let path = self.resolve_source_path(action, &import.path)?;
            insert_module_root(&mut module_map, &import.name, path).map_err(|reason| {
                CoordinatorError::InvalidModuleImport(Box::new(InvalidModuleImport {
                    action: action.key.clone(),
                    module: module.key.clone(),
                    name: import.name.clone(),
                    reason,
                }))
            })?;
        }
        let target = *target;
        let runtime =
            runtime_spec(runtime, &self.invocation.toolchain, &target).map_err(|error| {
                CoordinatorError::Driver {
                    action: action.key.clone(),
                    error: Box::new(DriverError::Runtime(error)),
                }
            })?;
        Ok(CheckRequest::from_source_path(entry)
            .with_module_map(module_map)
            .with_package_graph_options(self.invocation.package_graph_report.options)
            .with_optimization(optimization(module.optimization))
            .with_profile(self.invocation.profile)
            .with_compilation_mode(self.invocation.compilation_mode)
            .with_timings(self.invocation.timings)
            .with_runtime(runtime))
    }

    fn package_dependency_selected(&self, root: &LogicalPathRoot) -> bool {
        let LogicalPathRoot::Package(package_key) = root else {
            return true;
        };
        if package_key == self.plan.root_package() {
            return true;
        }
        self.package_dependency_selection
            .get(package_key)
            .copied()
            .unwrap_or(true)
    }

    fn artifact(
        &self,
        action: &PlanAction,
        key: &ArtifactKey,
    ) -> Result<&PlanArtifact, CoordinatorError> {
        find_artifact(self.plan.artifacts(), key).ok_or_else(|| {
            inconsistent(
                format!("action `{}`", action.key.name()),
                format!("artifact `{}`", key.name()),
            )
        })
    }

    fn driver(
        &self,
        action: &PlanAction,
        target: &TargetConfig,
    ) -> Result<&Driver, CoordinatorError> {
        self.drivers.get(target).map(Arc::as_ref).ok_or_else(|| {
            inconsistent(
                format!("action `{}`", action.key.name()),
                format!("compiler driver for target `{}`", target),
            )
        })
    }

    fn resolve_source_path(
        &self,
        action: &PlanAction,
        logical: &LogicalPath,
    ) -> Result<SourcePath, CoordinatorError> {
        let path = self.resolve_path(action, logical)?;
        let text = path.to_str().ok_or_else(|| CoordinatorError::NonUtf8Path {
            action: action.key.clone(),
            path: path.clone(),
        })?;
        let protocol_path = logical.protocol_path();
        let identity = match logical.root() {
            LogicalPathRoot::Package(package) => {
                format!("build-package:{}:/{protocol_path}", package.as_str())
            }
            LogicalPathRoot::Build => format!(
                "build-output:{}:/{protocol_path}",
                self.plan.root_package().as_str()
            ),
            LogicalPathRoot::Cache => format!(
                "build-cache:{}:/{protocol_path}",
                self.plan.root_package().as_str()
            ),
            LogicalPathRoot::Toolchain => format!("toolchain:/{protocol_path}"),
            LogicalPathRoot::Artifact(artifact) => format!(
                "build-artifact:{}:{}:/{protocol_path}",
                artifact.package().as_str(),
                artifact.name()
            ),
        };
        Ok(SourcePath::with_identity(text, identity))
    }

    fn resolve_path(
        &self,
        action: &PlanAction,
        logical: &LogicalPath,
    ) -> Result<PathBuf, CoordinatorError> {
        let mut path = match logical.root() {
            LogicalPathRoot::Package(package) => {
                let package = self
                    .plan
                    .packages()
                    .iter()
                    .find(|candidate| &candidate.key == package)
                    .ok_or_else(|| CoordinatorError::UnmappedPackage {
                        action: action.key.clone(),
                        package: package.clone(),
                    })?;
                crate::package_source_root(&self.invocation, package).ok_or_else(|| {
                    CoordinatorError::UnmappedPackage {
                        action: action.key.clone(),
                        package: package.key.clone(),
                    }
                })?
            }
            LogicalPathRoot::Build => self.invocation.build_dir.clone(),
            LogicalPathRoot::Cache => self.invocation.cache_dir.clone(),
            LogicalPathRoot::Toolchain => self.invocation.toolchain.resource_root().to_path_buf(),
            LogicalPathRoot::Artifact(key) => {
                let artifact = self.artifact(action, key)?;
                self.resolve_path(action, &artifact.output)?
            }
        };
        for component in logical.components() {
            path.push(component);
        }
        Ok(path)
    }
}

#[cfg(test)]
mod package_manifest_import_tests {
    use super::*;
    use nia_package_metadata::PackageGraphReport;

    #[test]
    fn manifest_import_roots_are_selected_for_their_owner_and_scope() {
        let unique = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let root = std::env::temp_dir().join(format!(
            "nia-manifest-imports-{}-{unique}",
            std::process::id()
        ));
        let runtime = root.join("runtime");
        let build = root.join("build");
        let development = root.join("development");
        for package in [&runtime, &build, &development] {
            std::fs::create_dir_all(package.join("src")).unwrap();
            std::fs::write(
                package.join("nia.toml"),
                format!(
                    "[package]\nname = \"{}\"\n[library]\npath = \"src/lib.nia\"\n",
                    package.file_name().unwrap().to_string_lossy()
                ),
            )
            .unwrap();
            std::fs::write(package.join("src/lib.nia"), "pub const value: u8 = 1;\n").unwrap();
        }
        let edge = |name: &str, scope, package: &std::path::Path| {
            nia_package_metadata::PackageDependencyReport {
                owner_root: "owner".into(),
                dependency: nia_package_metadata::PackageDependency {
                    name: name.into(),
                    source: nia_package_metadata::DependencySource::Path(name.into()),
                    scope,
                },
                resolved_root: Some(format!("owner/{name}")),
                source_root: Some(package.canonicalize().unwrap()),
            }
        };
        let report = PackageGraphReport {
            options: nia_package_metadata::PackageGraphOptions {
                include_development: true,
            },
            resolved: Vec::new(),
            declared: vec![
                edge("runtime", DependencyScope::Runtime, &runtime),
                edge("build", DependencyScope::Build, &build),
                edge("development", DependencyScope::Development, &development),
            ],
        };

        let source_scopes = [DependencyScope::Runtime, DependencyScope::Development];
        let ordinary =
            manifest_module_roots(&report, &source_scopes, CompilationMode::Normal).unwrap();
        assert_eq!(ordinary.len(), 1);
        assert_eq!(ordinary[0].0, "runtime");
        assert!(ordinary[0].1.as_str().ends_with("runtime/src/lib.nia"));
        assert_eq!(
            ordinary[0].1.identity().normalized_path(),
            "build-package:owner/runtime:/src/lib.nia"
        );

        let tests = manifest_module_roots(&report, &source_scopes, CompilationMode::Test).unwrap();
        assert_eq!(tests.len(), 2);
        assert!(tests.iter().any(|(name, _)| name == "runtime"));
        assert!(tests.iter().any(|(name, _)| name == "development"));
        std::fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn conflicting_manifest_module_aliases_are_rejected() {
        let mut module_map = ModuleMap::new();
        insert_module_root(
            &mut module_map,
            "shared",
            SourcePath::with_identity("/packages/first/src/lib.nia", "package:first:/src/lib.nia"),
        )
        .unwrap();
        assert!(
            insert_module_root(
                &mut module_map,
                "shared",
                SourcePath::with_identity(
                    "/packages/second/src/lib.nia",
                    "package:second:/src/lib.nia"
                ),
            )
            .unwrap_err()
            .contains("multiple package instances")
        );
    }
}

fn append_test_suite_bytes(encoded: &mut Vec<u8>, value: &[u8]) {
    encoded.extend_from_slice(&(value.len() as u64).to_le_bytes());
    encoded.extend_from_slice(value);
}

fn append_test_suite_path(encoded: &mut Vec<u8>, path: &LogicalPath) {
    match path.root() {
        LogicalPathRoot::Package(package) => {
            encoded.push(0);
            append_test_suite_bytes(encoded, package.as_str().as_bytes());
        }
        LogicalPathRoot::Build => encoded.push(1),
        LogicalPathRoot::Cache => encoded.push(2),
        LogicalPathRoot::Toolchain => encoded.push(3),
        LogicalPathRoot::Artifact(artifact) => {
            encoded.push(4);
            append_test_suite_bytes(encoded, artifact.package().as_str().as_bytes());
            append_test_suite_bytes(encoded, artifact.name().as_bytes());
        }
    }
    append_test_suite_bytes(encoded, path.protocol_path().as_bytes());
}

fn append_test_suite_action_identity(
    encoded: &mut Vec<u8>,
    action: &PlanAction,
    command: &CommandAction,
    artifact: &PlanArtifact,
) {
    append_test_suite_bytes(encoded, action.key.package().as_str().as_bytes());
    append_test_suite_bytes(encoded, action.key.name().as_bytes());
    append_test_suite_bytes(encoded, artifact.key.package().as_str().as_bytes());
    append_test_suite_bytes(encoded, artifact.key.name().as_bytes());
    append_test_suite_bytes(encoded, artifact.root_module.package().as_str().as_bytes());
    append_test_suite_bytes(encoded, artifact.root_module.name().as_bytes());
    append_test_suite_path(encoded, &artifact.output);
    encoded.push(artifact_kind_tag(&artifact.kind));
    append_test_suite_path(encoded, &command.working_directory);
    match &command.stdin {
        CommandStdin::Null => encoded.push(0),
        CommandStdin::InputPath(path) => {
            encoded.push(1);
            append_test_suite_path(encoded, path);
        }
    }
    match &command.program {
        CommandProgram::Path(path) => {
            encoded.push(0);
            append_test_suite_path(encoded, path);
        }
        CommandProgram::Search(name) => {
            encoded.push(1);
            append_test_suite_bytes(encoded, name.as_bytes());
        }
    }
    append_test_suite_bytes(encoded, &(command.arguments.len() as u64).to_le_bytes());
    for argument in &command.arguments {
        match argument {
            CommandArgument::Literal(value) => {
                encoded.push(0);
                append_test_suite_bytes(encoded, value.as_bytes());
            }
            CommandArgument::InputPath(path) => {
                encoded.push(1);
                append_test_suite_path(encoded, path);
            }
            CommandArgument::OutputPath(path) => {
                encoded.push(2);
                append_test_suite_path(encoded, path);
            }
        }
    }
    append_test_suite_bytes(encoded, &(command.outputs.len() as u64).to_le_bytes());
    for output in &command.outputs {
        append_test_suite_path(encoded, output);
    }
    encoded.push(match command.environment_policy {
        CommandEnvironmentPolicy::Inherit => 0,
        CommandEnvironmentPolicy::Clear => 1,
    });
    append_test_suite_bytes(encoded, &(command.environment.len() as u64).to_le_bytes());
    for input in &command.environment {
        append_test_suite_bytes(encoded, input.name.as_bytes());
        match &input.value {
            Some(value) => {
                encoded.push(1);
                append_test_suite_bytes(encoded, value.as_bytes());
            }
            None => encoded.push(0),
        }
    }
}

fn artifact_kind_tag(kind: &PlanArtifactKind) -> u8 {
    match kind {
        PlanArtifactKind::Executable => 0,
        PlanArtifactKind::ObjectSet => 1,
        PlanArtifactKind::StaticArchive => 2,
    }
}

fn profile_tag(profile: BuildProfile) -> u8 {
    match profile {
        BuildProfile::Debug => 0,
        BuildProfile::Release => 1,
    }
}

fn compilation_mode_tag(mode: CompilationMode) -> u8 {
    match mode {
        CompilationMode::Normal => 0,
        CompilationMode::Test => 1,
    }
}

fn optimization_tag(optimization: OptimizationMode) -> u8 {
    match optimization {
        OptimizationMode::O0 => 0,
        OptimizationMode::O1 => 1,
        OptimizationMode::O2 => 2,
        OptimizationMode::O3 => 3,
        OptimizationMode::Os => 4,
        OptimizationMode::Oz => 5,
    }
}

fn runtime_tag(runtime: Runtime) -> u8 {
    match runtime {
        Runtime::Bare => 0,
        Runtime::Freestanding => 1,
    }
}

fn link_time_optimization_tag(policy: LinkTimeOptimization) -> u8 {
    match policy {
        LinkTimeOptimization::Off => 0,
        LinkTimeOptimization::Thin => 1,
        LinkTimeOptimization::Full => 2,
    }
}

const MAX_COMMAND_ENVIRONMENT_SNAPSHOT_BYTES: usize = 1024 * 1024;

fn snapshot_process_environment() -> Option<Vec<(Vec<u8>, Vec<u8>)>> {
    snapshot_environment(std::env::vars_os())
}

fn effective_test_suite_environment(
    policy: CommandEnvironmentPolicy,
    inputs: &[EnvironmentInput],
) -> Option<Vec<(Vec<u8>, Vec<u8>)>> {
    let mut entries = match policy {
        CommandEnvironmentPolicy::Inherit => snapshot_process_environment()?,
        CommandEnvironmentPolicy::Clear => Vec::new(),
    };
    for input in inputs {
        let name = environment_name_bytes(input.name.as_bytes())?;
        entries.retain(|(existing, _)| existing.as_slice() != name.as_slice());
        if let Some(value) = &input.value {
            entries.push((name, value.as_bytes().to_vec()));
        }
    }
    let mut size = 0usize;
    for (name, value) in &entries {
        size = size
            .checked_add(16)?
            .checked_add(name.len())?
            .checked_add(value.len())?;
        if size > MAX_COMMAND_ENVIRONMENT_SNAPSHOT_BYTES {
            return None;
        }
    }
    entries.sort();
    Some(entries)
}

fn snapshot_environment(
    environment: impl IntoIterator<Item = (std::ffi::OsString, std::ffi::OsString)>,
) -> Option<Vec<(Vec<u8>, Vec<u8>)>> {
    let mut entries = Vec::new();
    let mut size = 0usize;
    for (name, value) in environment {
        let name = environment_name_bytes(name.as_encoded_bytes())?;
        let value = value.as_encoded_bytes();
        size = size
            .checked_add(16)?
            .checked_add(name.len())?
            .checked_add(value.len())?;
        if size > MAX_COMMAND_ENVIRONMENT_SNAPSHOT_BYTES {
            return None;
        }
        entries.push((name, value.to_vec()));
    }
    entries.sort();
    Some(entries)
}

fn environment_name_bytes(name: &[u8]) -> Option<Vec<u8>> {
    #[cfg(windows)]
    {
        // Windows environment lookup is case-insensitive and OsString uses
        // WTF-8 bytes, while plan declarations are UTF-8. Keep caching only
        // for ASCII names where one canonical representation is unambiguous.
        if !name.is_ascii() {
            return None;
        }
        Some(name.to_ascii_uppercase())
    }
    #[cfg(not(windows))]
    {
        Some(name.to_vec())
    }
}

#[cfg(test)]
mod environment_snapshot_tests {
    use super::*;
    use std::ffi::OsString;

    #[test]
    fn snapshot_sorts_entries_and_distinguishes_absent_from_empty() {
        let snapshot = snapshot_environment([
            (OsString::from("Z"), OsString::from("last")),
            (OsString::from("A"), OsString::new()),
        ])
        .unwrap();
        assert_eq!(
            snapshot,
            vec![
                (b"A".to_vec(), Vec::new()),
                (b"Z".to_vec(), b"last".to_vec())
            ]
        );
        assert_ne!(
            snapshot_environment([]),
            snapshot_environment([(OsString::from("A"), OsString::new())])
        );
    }

    #[test]
    fn snapshot_enforces_the_encoded_budget_and_stops_at_overflow() {
        let value = OsString::from("x".repeat(MAX_COMMAND_ENVIRONMENT_SNAPSHOT_BYTES - 17));
        assert!(snapshot_environment([(OsString::from("A"), value.clone())]).is_some());
        let mut value = value;
        value.push("x");
        let entries =
            std::iter::once((OsString::from("A"), value)).chain(std::iter::from_fn(|| {
                panic!("must stop after exceeding the budget")
            }));
        assert!(snapshot_environment(entries).is_none());
    }

    #[test]
    fn effective_environment_applies_overrides_and_removals() {
        let inherited = effective_test_suite_environment(
            CommandEnvironmentPolicy::Inherit,
            &[
                EnvironmentInput {
                    name: "NIA_TEST_SUITE_OVERRIDE".to_string(),
                    value: Some("override".to_string()),
                },
                EnvironmentInput {
                    name: "NIA_TEST_SUITE_REMOVED".to_string(),
                    value: None,
                },
            ],
        )
        .unwrap();
        assert_eq!(
            inherited
                .iter()
                .find(|(name, _)| name == b"NIA_TEST_SUITE_OVERRIDE")
                .map(|(_, value)| value.as_slice()),
            Some(b"override".as_slice())
        );
        assert!(
            !inherited
                .iter()
                .any(|(name, _)| name == b"NIA_TEST_SUITE_REMOVED")
        );

        let cleared = effective_test_suite_environment(
            CommandEnvironmentPolicy::Clear,
            &[EnvironmentInput {
                name: "NIA_TEST_SUITE_OVERRIDE".to_string(),
                value: Some("value".to_string()),
            }],
        )
        .unwrap();
        assert_eq!(
            cleared,
            vec![(b"NIA_TEST_SUITE_OVERRIDE".to_vec(), b"value".to_vec())]
        );
    }
}
