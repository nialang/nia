// SPDX-License-Identifier: GPL-3.0-or-later
//! Hermetic external-command identity, execution, and bounded diagnostics.

use super::*;

pub(super) struct ResolvedExternalCommand<'a> {
    pub(super) program: &'a str,
    pub(super) arguments: &'a [String],
    pub(super) working_directory: &'a Path,
    pub(super) environment_policy: crate::CommandEnvironmentPolicy,
    pub(super) environment: &'a [crate::EnvironmentInput],
}

pub(super) fn resolve_search_program(
    action: &PlanAction,
    name: &str,
    working_directory: &Path,
    environment: &[EnvironmentInput],
) -> Result<PathBuf, CoordinatorError> {
    let name_path = Path::new(name);
    if name_path.is_absolute() || name_path.components().count() > 1 {
        let candidate = if name_path.is_absolute() {
            name_path.to_path_buf()
        } else {
            working_directory.join(name_path)
        };
        return executable_candidate(&candidate).ok_or_else(|| {
            CoordinatorError::ExternalCommandIo {
                action: action.key.clone(),
                path: candidate,
                operation: "resolve",
                error: io::Error::new(io::ErrorKind::NotFound, "command program is not executable"),
            }
        });
    }
    // Declared PATH wins even when it is explicitly removed. Falling back to
    // the host is allowed only when the action did not declare PATH at all.
    let search_path = match environment.iter().find(|input| input.name == "PATH") {
        Some(input) => input.value.as_deref().map(std::ffi::OsString::from),
        None => std::env::var_os("PATH"),
    };
    if let Some(search_path) = search_path {
        for directory in std::env::split_paths(&search_path) {
            let directory = if directory.as_os_str().is_empty() {
                working_directory.to_path_buf()
            } else if directory.is_absolute() {
                directory
            } else {
                working_directory.join(directory)
            };
            if let Some(candidate) = executable_candidate(&directory.join(name)) {
                return Ok(candidate);
            }
        }
    }
    Err(CoordinatorError::ExternalCommandIo {
        action: action.key.clone(),
        path: PathBuf::from(name),
        operation: "resolve",
        error: io::Error::new(
            io::ErrorKind::NotFound,
            "command program was not found in PATH",
        ),
    })
}

fn executable_candidate(path: &Path) -> Option<PathBuf> {
    let metadata = fs::metadata(path).ok()?;
    if !metadata.is_file() {
        return None;
    }
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt as _;
        (metadata.permissions().mode() & 0o111 != 0).then(|| path.to_path_buf())
    }
    #[cfg(not(unix))]
    {
        Some(path.to_path_buf())
    }
}

pub(super) fn read_external_identity_file(
    action: &PlanAction,
    path: &Path,
    operation: &'static str,
) -> Result<Vec<u8>, CoordinatorError> {
    let metadata = fs::metadata(path).map_err(|error| CoordinatorError::ExternalCommandIo {
        action: action.key.clone(),
        path: path.to_path_buf(),
        operation,
        error,
    })?;
    if !metadata.is_file() {
        return Err(CoordinatorError::ExternalCommandIo {
            action: action.key.clone(),
            path: path.to_path_buf(),
            operation,
            error: io::Error::new(
                io::ErrorKind::InvalidData,
                "cache input must be a regular file",
            ),
        });
    }
    fs::read(path).map_err(|error| CoordinatorError::ExternalCommandIo {
        action: action.key.clone(),
        path: path.to_path_buf(),
        operation,
        error,
    })
}

pub(super) fn read_external_identity_input(
    action: &PlanAction,
    path: &Path,
    operation: &'static str,
) -> Result<Vec<u8>, CoordinatorError> {
    let metadata =
        fs::symlink_metadata(path).map_err(|error| CoordinatorError::ExternalCommandIo {
            action: action.key.clone(),
            path: path.to_path_buf(),
            operation,
            error,
        })?;
    if metadata.is_file() {
        return fs::read(path).map_err(|error| CoordinatorError::ExternalCommandIo {
            action: action.key.clone(),
            path: path.to_path_buf(),
            operation,
            error,
        });
    }
    if metadata.is_dir() {
        return read_external_identity_directory(action, path, operation);
    }
    Err(CoordinatorError::ExternalCommandIo {
        action: action.key.clone(),
        path: path.to_path_buf(),
        operation,
        error: io::Error::new(
            io::ErrorKind::InvalidData,
            "cache input must be a regular file or directory",
        ),
    })
}

fn read_external_identity_directory(
    action: &PlanAction,
    path: &Path,
    operation: &'static str,
) -> Result<Vec<u8>, CoordinatorError> {
    // Sort traversal so identity is independent of filesystem enumeration
    // order. Symlinks are rejected because their targets escape the declared
    // input tree and can change without changing the logical input path.
    let mut entries = fs::read_dir(path)
        .map_err(|error| CoordinatorError::ExternalCommandIo {
            action: action.key.clone(),
            path: path.to_path_buf(),
            operation,
            error,
        })?
        .collect::<Result<Vec<_>, _>>()
        .map_err(|error| CoordinatorError::ExternalCommandIo {
            action: action.key.clone(),
            path: path.to_path_buf(),
            operation,
            error,
        })?;
    entries.sort_by(|left, right| {
        left.file_name()
            .as_encoded_bytes()
            .cmp(right.file_name().as_encoded_bytes())
    });
    let mut encoded = b"NIA-DIR1\0".to_vec();
    encoded.extend_from_slice(&(entries.len() as u64).to_le_bytes());
    for entry in entries {
        let name = entry.file_name();
        let name_bytes = name.as_encoded_bytes();
        encoded.extend_from_slice(&(name_bytes.len() as u64).to_le_bytes());
        encoded.extend_from_slice(name_bytes);
        let child = entry.path();
        let metadata =
            fs::symlink_metadata(&child).map_err(|error| CoordinatorError::ExternalCommandIo {
                action: action.key.clone(),
                path: child.clone(),
                operation,
                error,
            })?;
        if metadata.is_file() {
            encoded.push(0);
            let bytes = fs::read(&child).map_err(|error| CoordinatorError::ExternalCommandIo {
                action: action.key.clone(),
                path: child,
                operation,
                error,
            })?;
            encoded.extend_from_slice(&(bytes.len() as u64).to_le_bytes());
            encoded.extend_from_slice(&bytes);
        } else if metadata.is_dir() {
            encoded.push(1);
            let nested = read_external_identity_directory(action, &child, operation)?;
            encoded.extend_from_slice(&(nested.len() as u64).to_le_bytes());
            encoded.extend_from_slice(&nested);
        } else {
            return Err(CoordinatorError::ExternalCommandIo {
                action: action.key.clone(),
                path: child,
                operation,
                error: io::Error::new(
                    io::ErrorKind::InvalidData,
                    "cache input tree contains a non-regular entry",
                ),
            });
        }
    }
    Ok(encoded)
}

pub(super) fn read_staged_external_outputs(
    action: &PlanAction,
    staged: &StagedOutputTransaction,
) -> Result<Vec<Vec<u8>>, CoordinatorError> {
    staged
        .outputs
        .iter()
        .map(|output| {
            let metadata = fs::symlink_metadata(&output.temporary).map_err(|error| {
                staged_output_io(
                    action,
                    &output.temporary,
                    "inspect command-produced",
                    error,
                    None,
                )
            })?;
            if !metadata.file_type().is_file() {
                return Err(staged_output_io(
                    action,
                    &output.temporary,
                    "cache non-file",
                    io::Error::new(
                        io::ErrorKind::InvalidData,
                        "external command output must be a regular file",
                    ),
                    None,
                ));
            }
            fs::read(&output.temporary).map_err(|error| {
                staged_output_io(
                    action,
                    &output.temporary,
                    "read command-produced",
                    error,
                    None,
                )
            })
        })
        .collect()
}

pub(super) fn restore_cached_external_outputs(
    action: &PlanAction,
    build_dir: &Path,
    resolved_outputs: &[(&LogicalPath, PathBuf)],
    payloads: &[Vec<u8>],
) -> Result<(), CoordinatorError> {
    if resolved_outputs.len() != payloads.len() {
        return Err(inconsistent(
            format!("action `{}`", action.key.name()),
            "matching cached external-command outputs".to_string(),
        ));
    }
    let staged = prepare_staged_outputs(action, build_dir, resolved_outputs)?;
    for (index, payload) in payloads.iter().enumerate() {
        let temporary = staged.outputs[index].temporary.clone();
        if let Err(error) = fs::write(&temporary, payload) {
            return cleanup_staged_outputs(
                action,
                staged,
                Some(Box::new(staged_output_io(
                    action,
                    &temporary,
                    "restore cached",
                    error,
                    None,
                ))),
            );
        }
    }
    publish_staged_outputs(action, staged)
}

#[derive(Clone, Copy)]
pub(super) struct ExternalExecutionPolicy<'a> {
    pub(super) timeout: Duration,
    pub(super) forward_output: bool,
    pub(super) cancellation: Option<&'a ActionCancellation>,
}

pub(super) fn execute_external_command(
    action: &PlanAction,
    request: ResolvedExternalCommand<'_>,
    policy: ExternalExecutionPolicy,
) -> Result<(), CoordinatorError> {
    let error = |failure| {
        CoordinatorError::ExternalCommand(Box::new(ExternalCommandError {
            action: action.key.clone(),
            program: request.program.to_string(),
            arguments: request.arguments.to_vec(),
            working_directory: request.working_directory.to_path_buf(),
            failure,
        }))
    };
    let mut command = Command::new(request.program);
    command
        .args(request.arguments)
        .current_dir(request.working_directory)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    if request.environment_policy == crate::CommandEnvironmentPolicy::Clear {
        command.env_clear();
    }
    for input in request.environment {
        match &input.value {
            Some(value) => {
                command.env(&input.name, value);
            }
            None => {
                command.env_remove(&input.name);
            }
        }
    }
    prepare_process_group(&mut command);
    let mut child = command
        .spawn()
        .map_err(|source| error(ExternalCommandFailure::Spawn { error: source }))?;
    let Some(stdout) = child.stdout.take() else {
        terminate_process_tree(&mut child);
        let _ = child.wait();
        return Err(error(ExternalCommandFailure::MissingPipe {
            stream: "stdout",
        }));
    };
    let Some(stderr) = child.stderr.take() else {
        terminate_process_tree(&mut child);
        let _ = child.wait();
        return Err(error(ExternalCommandFailure::MissingPipe {
            stream: "stderr",
        }));
    };
    // Capture workers drain both pipes concurrently and retain bounded tails;
    // this prevents pipe deadlock and unbounded failure diagnostics.
    let stdout_reader = match thread::Builder::new()
        .name("nia-build-stdout".to_string())
        .spawn(move || {
            capture_stream(
                stdout,
                CapturedStream::Stdout,
                policy.forward_output,
                EXTERNAL_OUTPUT_TAIL_BYTES,
            )
        }) {
        Ok(reader) => reader,
        Err(source) => {
            terminate_process_tree(&mut child);
            let _ = child.wait();
            return Err(error(ExternalCommandFailure::CaptureWorkerSpawn {
                stream: "stdout",
                error: source,
            }));
        }
    };
    let stderr_reader = match thread::Builder::new()
        .name("nia-build-stderr".to_string())
        .spawn(move || {
            capture_stream(
                stderr,
                CapturedStream::Stderr,
                policy.forward_output,
                EXTERNAL_OUTPUT_TAIL_BYTES,
            )
        }) {
        Ok(reader) => reader,
        Err(source) => {
            terminate_process_tree(&mut child);
            let _ = child.wait();
            let _ = stdout_reader.join();
            return Err(error(ExternalCommandFailure::CaptureWorkerSpawn {
                stream: "stderr",
                error: source,
            }));
        }
    };

    let started = Instant::now();
    let mut timed_out = false;
    let mut cancelled = false;
    let status = loop {
        if policy
            .cancellation
            .is_some_and(ActionCancellation::is_cancelled)
        {
            cancelled = true;
            terminate_process_tree(&mut child);
            break child.wait();
        }
        match child.try_wait() {
            Ok(Some(status)) => {
                terminate_process_descendants(child.id());
                break Ok(status);
            }
            Ok(None) if started.elapsed() >= policy.timeout => {
                timed_out = true;
                terminate_process_tree(&mut child);
                break child.wait();
            }
            Ok(None) => thread::sleep(EXTERNAL_WAIT_POLL),
            Err(source) => {
                terminate_process_tree(&mut child);
                let _ = child.wait();
                break Err(source);
            }
        }
    };
    let stdout = join_capture(stdout_reader, "stdout").map_err(&error)?;
    let stderr = join_capture(stderr_reader, "stderr").map_err(&error)?;
    if let Some(source) = stdout.error {
        return Err(error(ExternalCommandFailure::StreamIo {
            stream: "stdout",
            error: source,
        }));
    }
    if let Some(source) = stderr.error {
        return Err(error(ExternalCommandFailure::StreamIo {
            stream: "stderr",
            error: source,
        }));
    }
    let status = status.map_err(|source| error(ExternalCommandFailure::Wait { error: source }))?;
    if cancelled {
        return Err(error(ExternalCommandFailure::Cancelled {
            stdout: stdout.tail,
            stderr: stderr.tail,
        }));
    }
    if timed_out {
        return Err(error(ExternalCommandFailure::TimedOut {
            timeout: policy.timeout,
            stdout: stdout.tail,
            stderr: stderr.tail,
        }));
    }
    if !status.success() {
        return Err(error(ExternalCommandFailure::Exit {
            status,
            stdout: stdout.tail,
            stderr: stderr.tail,
        }));
    }
    Ok(())
}

fn join_capture(
    reader: thread::JoinHandle<StreamCapture>,
    stream: &'static str,
) -> Result<StreamCapture, ExternalCommandFailure> {
    reader
        .join()
        .map_err(|_| ExternalCommandFailure::CaptureThread { stream })
}

pub(super) fn display_external_command_error(
    f: &mut fmt::Formatter<'_>,
    details: &ExternalCommandError,
) -> fmt::Result {
    write!(
        f,
        "external command action `{}` in package `{}` failed to run `{:?}` with {} argument(s) in `{}`: ",
        details.action.name(),
        details.action.package().as_str(),
        details.program,
        details.arguments.len(),
        details.working_directory.display(),
    )?;
    match &details.failure {
        ExternalCommandFailure::Spawn { error } => write!(f, "spawn failed: {error}"),
        ExternalCommandFailure::MissingPipe { stream } => {
            write!(f, "coordinator did not retain the configured {stream} pipe")
        }
        ExternalCommandFailure::Wait { error } => write!(f, "wait failed: {error}"),
        ExternalCommandFailure::CaptureThread { stream } => {
            write!(f, "{stream} capture worker failed")
        }
        ExternalCommandFailure::CaptureWorkerSpawn { stream, error } => {
            write!(f, "failed to start {stream} capture worker: {error}")
        }
        ExternalCommandFailure::StreamIo { stream, error } => {
            write!(f, "{stream} capture/forward failed: {error}")
        }
        ExternalCommandFailure::TimedOut {
            timeout,
            stdout,
            stderr,
        } => {
            write!(f, "timed out after {timeout:?}")?;
            display_output_tails(f, stdout, stderr)
        }
        ExternalCommandFailure::Cancelled { stdout, stderr } => {
            f.write_str("cancelled after another build action failed")?;
            display_output_tails(f, stdout, stderr)
        }
        ExternalCommandFailure::Exit {
            status,
            stdout,
            stderr,
        } => {
            write!(f, "exited with status {status}")?;
            display_output_tails(f, stdout, stderr)
        }
    }
}

pub(super) fn is_cancellation_error(error: &CoordinatorError) -> bool {
    match error {
        CoordinatorError::Cancelled { .. } => true,
        CoordinatorError::ExternalCommand(details) => {
            matches!(details.failure, ExternalCommandFailure::Cancelled { .. })
        }
        CoordinatorError::StagedOutput {
            cause: Some(cause), ..
        } => is_cancellation_error(cause),
        _ => false,
    }
}

pub(super) fn display_output_tails(
    f: &mut fmt::Formatter<'_>,
    stdout: &[u8],
    stderr: &[u8],
) -> fmt::Result {
    if !stdout.is_empty() {
        write!(f, "\nstdout tail:\n{}", String::from_utf8_lossy(stdout))?;
    }
    if !stderr.is_empty() {
        write!(f, "\nstderr tail:\n{}", String::from_utf8_lossy(stderr))?;
    }
    Ok(())
}
