// SPDX-License-Identifier: GPL-3.0-or-later
//! Typed build-action execution and cache interaction.
//!
//! Every action acquires its complete canonical output-lock set before cache
//! restore or execution. Compiler cache lookup uses a precheck manifest, while
//! publication uses the manifest returned by the completed compiler request.

use super::*;

#[derive(Clone)]
pub(super) struct DriverActionExecutor {
    plan: Arc<BuildPlan>,
    invocation: Arc<BuildInvocation>,
    drivers: Arc<BTreeMap<TargetSpec, Arc<Driver>>>,
}

impl DriverActionExecutor {
    pub(super) fn new(plan: BuildPlan, invocation: BuildInvocation) -> Self {
        let plan = Arc::new(plan);
        let invocation = Arc::new(invocation);
        let mut drivers = BTreeMap::new();
        for action in plan.actions() {
            let target = match &action.kind {
                ActionKind::CompilerCheck { target, .. }
                | ActionKind::CompilerEmit { target, .. } => target,
                _ => continue,
            };
            drivers.entry(target.clone()).or_insert_with(|| {
                Arc::new(Driver::with_config(
                    DriverConfig {
                        artifact_cache_dir: Some(invocation.cache_dir.clone()),
                        ..DriverConfig::new(Arc::clone(&invocation.toolchain))
                    }
                    .with_artifact_target(target_config(target)),
                ))
            });
        }
        Self {
            plan,
            invocation,
            drivers: Arc::new(drivers),
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
            ActionKind::ExternalCommand {
                resource_class: _,
                environment_policy,
                cache_policy,
                program,
                arguments,
                working_directory,
                environment,
                inputs,
                outputs,
            } => {
                return self.execute_external_command_action(
                    action,
                    *environment_policy,
                    *cache_policy,
                    program,
                    arguments,
                    working_directory,
                    environment,
                    inputs,
                    outputs,
                    cancellation,
                );
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

    #[allow(clippy::too_many_arguments)]
    fn execute_external_command_action(
        &self,
        action: &PlanAction,
        environment_policy: CommandEnvironmentPolicy,
        cache_policy: CommandCachePolicy,
        program: &CommandProgram,
        arguments: &[CommandArgument],
        logical_working_directory: &LogicalPath,
        environment: &[EnvironmentInput],
        inputs: &[LogicalPath],
        outputs: &[LogicalPath],
        cancellation: &ActionCancellation,
    ) -> Result<Option<ActionCacheOutcome>, CoordinatorError> {
        let working_directory = self.resolve_path(action, logical_working_directory)?;
        let resolved_inputs = inputs
            .iter()
            .map(|input| self.resolve_path(action, input).map(|path| (input, path)))
            .collect::<Result<Vec<_>, _>>()?;
        let resolved_outputs = outputs
            .iter()
            .map(|output| self.resolve_path(action, output).map(|path| (output, path)))
            .collect::<Result<Vec<_>, _>>()?;
        let cacheable = cache_policy == CommandCachePolicy::DeclaredInputs;
        let resolved_program = match program {
            CommandProgram::Path(path) => self.resolve_path(action, path)?,
            CommandProgram::Search(name) if cacheable => {
                resolve_search_program(action, name, &working_directory, environment)?
            }
            CommandProgram::Search(name) => PathBuf::from(name),
        };
        let program_text = path_text(action, &resolved_program)?;

        let cache = ExternalCommandCache::new(self.invocation.cache_dir.clone());
        let mut cache_identity = if cacheable {
            Some(self.external_command_cache_identity(
                action,
                program,
                arguments,
                logical_working_directory,
                environment,
                &resolved_inputs,
                outputs,
                &resolved_program,
            )?)
        } else {
            None
        };
        let mut miss_reason = None;
        if let Some(identity) = cache_identity.as_ref() {
            match cache.lookup(identity) {
                Ok(ExternalCommandCacheLookup::Hit(payloads)) => {
                    restore_cached_external_outputs(
                        action,
                        &self.invocation.build_dir,
                        &resolved_outputs,
                        &payloads,
                    )?;
                    return Ok(Some(ActionCacheOutcome::Hit));
                }
                Ok(ExternalCommandCacheLookup::Miss(reason)) => miss_reason = Some(reason),
                Err(_) => miss_reason = Some(ActionCacheMissReason::ReadError),
            }
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
        let execution = execute_external_command(
            action,
            ResolvedExternalCommand {
                program: &program_text,
                arguments: &resolved_arguments,
                working_directory: &working_directory,
                environment_policy,
                environment,
            },
            ExternalExecutionPolicy {
                timeout: EXTERNAL_COMMAND_TIMEOUT,
                forward_output: true,
                cancellation: Some(cancellation),
            },
        );
        let payloads = match (execution, staged.as_ref()) {
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
                match read_staged_external_outputs(action, staged_output) {
                    Ok(payloads) => Some(payloads),
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
            return Ok(None);
        };
        let current_identity = match self.external_command_cache_identity(
            action,
            program,
            arguments,
            logical_working_directory,
            environment,
            &resolved_inputs,
            outputs,
            &resolved_program,
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
            match cache.publish(&identity, payloads.as_deref().unwrap_or_default()) {
                Ok(()) => miss_reason.unwrap_or(ActionCacheMissReason::NotFound),
                Err(_) => ActionCacheMissReason::WriteError,
            }
        };
        Ok(Some(ActionCacheOutcome::Miss(reason)))
    }

    #[allow(clippy::too_many_arguments)]
    fn external_command_cache_identity(
        &self,
        action: &PlanAction,
        program: &CommandProgram,
        arguments: &[CommandArgument],
        working_directory: &LogicalPath,
        environment: &[EnvironmentInput],
        resolved_inputs: &[(&LogicalPath, PathBuf)],
        outputs: &[LogicalPath],
        resolved_program: &Path,
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
            program,
            arguments,
            working_directory,
            environment,
            &inputs,
            outputs,
            self.plan.packages(),
            tool_contents,
            self.invocation.toolchain.identity(),
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
        target: &TargetSpec,
        runtime: Runtime,
    ) -> Result<Option<ActionCacheOutcome>, CoordinatorError> {
        let module = find_module(self.plan.modules(), module_key).ok_or_else(|| {
            inconsistent(
                format!("action `{}`", action.key.name()),
                format!("module `{}`", module_key.name()),
            )
        })?;
        let request = self.check_request(action, module_key, runtime)?;
        let driver = self.driver(action, target)?;
        // The cheap manifest is sufficient for lookup. A successful check may
        // discover a different provider closure, so only its returned manifest
        // is authoritative for the record that gets published below.
        let precheck_manifest = driver
            .source_input_manifest(&request)
            .result
            .map_err(|error| CoordinatorError::Driver {
                action: action.key.clone(),
                error: Box::new(error),
            })?;
        let cache = CompilerCheckCache::new(self.invocation.cache_dir.clone());
        let precheck_identity = CompilerCheckCacheIdentity::new(
            &action.key,
            module,
            self.plan.packages(),
            target,
            runtime,
            &precheck_manifest,
            self.invocation.toolchain.identity(),
        );
        let miss_reason = match precheck_identity.as_ref() {
            None => ActionCacheMissReason::Uncacheable,
            Some(identity) => match cache.lookup(identity) {
                Ok(CompilerCheckCacheLookup::Hit) => {
                    return Ok(Some(ActionCacheOutcome::Hit));
                }
                Ok(CompilerCheckCacheLookup::Miss(reason)) => reason,
                Err(_) => ActionCacheMissReason::ReadError,
            },
        };
        let checked = driver
            .check_entry_with_source_manifest(request)
            .result
            .map_err(|error| CoordinatorError::Driver {
                action: action.key.clone(),
                error: Box::new(error),
            })?;
        if !checked.program.diagnostics.is_empty() {
            return Ok(Some(ActionCacheOutcome::Miss(
                ActionCacheMissReason::Uncacheable,
            )));
        }
        let Some(final_identity) = CompilerCheckCacheIdentity::new(
            &action.key,
            module,
            self.plan.packages(),
            target,
            runtime,
            &checked.source_manifest,
            self.invocation.toolchain.identity(),
        ) else {
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
        target: &TargetSpec,
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
        let request = self
            .check_request(action, &artifact.root_module, artifact.runtime)?
            .with_runtime(DriverRuntime::Freestanding);
        let output = self.resolve_path(action, &artifact.output)?;
        let driver = self.driver(action, target)?;
        let mut cache_link_inputs = Vec::with_capacity(static_archives.len());
        let mut linker_inputs = Vec::with_capacity(static_archives.len());
        for archive_key in static_archives {
            let archive = self.artifact(action, archive_key)?;
            let path = self.resolve_path(action, &archive.output)?;
            let bytes =
                fs::read(&path).map_err(|error| CoordinatorError::StaticArchiveLinkInputIo {
                    action: action.key.clone(),
                    path: path.clone(),
                    operation: "read",
                    error,
                })?;
            cache_link_inputs.push(CompilerEmitCacheLinkInput::from_bytes(
                archive_key.clone(),
                &bytes,
            ));
            linker_inputs.push(StaticArchiveLinkInput::from_bytes(
                archive_key.package().as_str(),
                archive_key.name(),
                path,
                &bytes,
            ));
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
        let cache = CompilerEmitCache::new(self.invocation.cache_dir.clone());
        let precheck_identity = link_environment.and_then(|environment| {
            CompilerEmitCacheIdentity::new(CompilerEmitCacheIdentityInput {
                action: &action.key,
                artifact,
                module,
                packages: self.plan.packages(),
                target,
                manifest: &precheck_manifest,
                toolchain: self.invocation.toolchain.identity(),
                link_environment: environment,
                link_inputs: &cache_link_inputs,
            })
        });
        let miss_reason = match precheck_identity.as_ref() {
            None => ActionCacheMissReason::Uncacheable,
            Some(identity) => match cache.lookup(identity) {
                Ok(CompilerEmitCacheLookup::Hit(reference)) => {
                    let reason = match driver.restore_executable_cache(reference, &output) {
                        ExecutableCacheRestore::Hit => {
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
        let linked = driver
            .link_executable_with_source_manifest(
                LinkExecutableRequest::new(request, output).with_link_options(link_options.clone()),
            )
            .result
            .map_err(|error| CoordinatorError::Driver {
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
            manifest: &linked.source_manifest,
            toolchain: self.invocation.toolchain.identity(),
            link_environment,
            link_inputs: &cache_link_inputs,
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
        target: &TargetSpec,
    ) -> Result<Option<ActionCacheOutcome>, CoordinatorError> {
        let _module = find_module(self.plan.modules(), &artifact.root_module).ok_or_else(|| {
            inconsistent(
                format!("action `{}`", action.key.name()),
                format!("module `{}`", artifact.root_module.name()),
            )
        })?;
        let request = self
            .check_request(action, &artifact.root_module, artifact.runtime)?
            .with_runtime(runtime_mode(artifact.runtime));
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
        target: &TargetSpec,
    ) -> Result<Option<ActionCacheOutcome>, CoordinatorError> {
        let _module = find_module(self.plan.modules(), &artifact.root_module).ok_or_else(|| {
            inconsistent(
                format!("action `{}`", action.key.name()),
                format!("module `{}`", artifact.root_module.name()),
            )
        })?;
        let request = self
            .check_request(action, &artifact.root_module, artifact.runtime)?
            .with_runtime(runtime_mode(artifact.runtime));
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
        let archived = driver
            .archive_static_library_from_objects(&emitted, temporary, ArchiveOptions::default())
            .result
            .map_err(|error| CoordinatorError::Driver {
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
        let cache = GeneratedFileCache::new(self.invocation.cache_dir.clone());
        let identity = GeneratedFileCacheIdentity::new(
            &action.key,
            logical_output,
            contents,
            self.invocation.toolchain.identity(),
        );
        let lookup = match cache.lookup(&identity) {
            Ok(lookup) => lookup,
            Err(_) => {
                write_generated_file(action, &output, contents)?;
                return Ok(Some(ActionCacheOutcome::Miss(
                    ActionCacheMissReason::ReadError,
                )));
            }
        };
        match lookup {
            GeneratedFileCacheLookup::Hit(payload) => {
                write_generated_file(action, &output, &payload)?;
                Ok(Some(ActionCacheOutcome::Hit))
            }
            GeneratedFileCacheLookup::Miss(reason) => {
                write_generated_file(action, &output, contents)?;
                let reason = match cache.publish(&identity, contents) {
                    Ok(()) => reason,
                    Err(_) => ActionCacheMissReason::WriteError,
                };
                Ok(Some(ActionCacheOutcome::Miss(reason)))
            }
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
        if let Err(error) = fs::File::open(&temporary).and_then(|file| file.sync_all()) {
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
            ActionKind::ExternalCommand { outputs, .. } => outputs.iter().collect(),
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
            let Some(output_lock) =
                ScopedFileLock::acquire_interruptible(lock.clone(), || cancellation.is_cancelled())
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

    pub(super) fn check_request(
        &self,
        action: &PlanAction,
        module_key: &ModuleKey,
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
        for import in &module.imports {
            let path = self.resolve_source_path(action, &import.path)?;
            module_map
                .try_insert(&import.name, path)
                .map_err(|reason| {
                    CoordinatorError::InvalidModuleImport(Box::new(InvalidModuleImport {
                        action: action.key.clone(),
                        module: module.key.clone(),
                        name: import.name.clone(),
                        reason,
                    }))
                })?;
        }
        Ok(CheckRequest::from_source_path(entry)
            .with_module_map(module_map)
            .with_optimization(optimization(module.optimization))
            .with_timings(self.invocation.timings)
            .with_runtime(runtime_mode(runtime)))
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
        target: &TargetSpec,
    ) -> Result<&Driver, CoordinatorError> {
        self.drivers.get(target).map(Arc::as_ref).ok_or_else(|| {
            inconsistent(
                format!("action `{}`", action.key.name()),
                format!("compiler driver for target `{}`", display_target(target)),
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
                let mut root = self.invocation.package_root.clone();
                if !package.root.is_empty() {
                    root.extend(package.root.split('/'));
                }
                root
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
