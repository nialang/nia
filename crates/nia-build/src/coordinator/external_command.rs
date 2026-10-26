// SPDX-License-Identifier: GPL-3.0-or-later
//! External-command identity, execution, and bounded diagnostics.

use super::*;
use std::collections::{BTreeSet, HashMap, HashSet};
use std::io::Read;

#[cfg(unix)]
use std::os::unix::fs::OpenOptionsExt as _;
#[cfg(windows)]
use std::os::windows::fs::OpenOptionsExt as _;

#[derive(Clone, Copy)]
enum ExternalIdentityKind {
    Tool,
    Input,
}

const EXTERNAL_DIRECTORY_IDENTITY_STREAM_BYTES: usize = 64 * 1024;
const EXTERNAL_DIRECTORY_IDENTITY_MAGIC: &[u8] = b"NIA-DIR1\0";
const MAX_EXTERNAL_DIRECTORY_IDENTITY_BYTES: u64 = 64 * 1024 * 1024;
const MAX_EXTERNAL_DIRECTORY_DEPTH: usize = 256;
const MAX_EXTERNAL_DIRECTORY_ENTRIES: usize = 1_000_000;
const MAX_ISOLATED_INPUT_BYTES: u64 = 64 * 1024 * 1024;
static INPUT_ISOLATION_SEQUENCE: AtomicU64 = AtomicU64::new(0);

struct ExternalDirectoryIdentityPlan {
    path: PathBuf,
    encoded_len: u64,
    entries: Vec<ExternalDirectoryIdentityEntry>,
}

struct ExternalDirectoryIdentityEntry {
    name: Vec<u8>,
    kind: ExternalDirectoryIdentityEntryKind,
}

enum ExternalDirectoryIdentityEntryKind {
    File { path: PathBuf, length: u64 },
    Directory(Box<ExternalDirectoryIdentityPlan>),
}

pub(super) struct ResolvedExternalCommand<'a> {
    pub(super) program: &'a str,
    pub(super) arguments: &'a [String],
    pub(super) working_directory: &'a Path,
    pub(super) environment_policy: crate::CommandEnvironmentPolicy,
    pub(super) environment: &'a [crate::EnvironmentInput],
    pub(super) stdin: Option<&'a Path>,
}

pub(super) fn resolve_search_program(
    action: &PlanAction,
    name: &str,
    working_directory: &Path,
    environment: &[EnvironmentInput],
) -> Result<PathBuf, CoordinatorError> {
    use crate::program_search::{ProgramLocation, ProgramSearch, find_program, program_location};

    // A declared variable wins even when it is explicitly removed. Falling
    // back to the host is allowed only when the action did not declare it.
    let declared = |variable: &str| match environment
        .iter()
        .find(|input| environment_name_matches(input.name.as_str(), variable))
    {
        Some(input) => input.value.as_deref().map(std::ffi::OsString::from),
        None => std::env::var_os(variable),
    };
    let search_path = declared("PATH");
    let pathext = declared("PATHEXT");
    let search = ProgramSearch {
        base: working_directory,
        path: search_path.as_deref(),
        pathext: pathext.as_deref(),
    };
    if let Some(candidate) = find_program(name, &search, executable_candidate) {
        return Ok(candidate);
    }
    let (path, message) = match program_location(name, working_directory) {
        ProgramLocation::Explicit(path) => (path, "command program is not executable"),
        ProgramLocation::Searched => (PathBuf::from(name), "command program was not found in PATH"),
    };
    Err(CoordinatorError::ExternalCommandIo {
        action: action.key.clone(),
        path,
        operation: "resolve",
        error: io::Error::new(io::ErrorKind::NotFound, message),
    })
}

#[inline]
fn environment_name_matches(left: &str, right: &str) -> bool {
    #[cfg(windows)]
    {
        left.eq_ignore_ascii_case(right)
    }
    #[cfg(not(windows))]
    {
        left == right
    }
}

fn executable_candidate(path: &Path) -> Option<PathBuf> {
    let metadata = fs::metadata(path).ok()?;
    crate::program_search::is_executable_file(&metadata).then(|| path.to_path_buf())
}

pub(super) fn read_external_identity_file(
    action: &PlanAction,
    path: &Path,
    operation: &'static str,
) -> Result<ExternalCommandContentIdentity, CoordinatorError> {
    read_external_identity_regular_file(action, path, operation, ExternalIdentityKind::Tool, false)
}

pub(super) fn read_external_identity_input(
    action: &PlanAction,
    path: &Path,
    operation: &'static str,
) -> Result<ExternalCommandContentIdentity, CoordinatorError> {
    let metadata =
        fs::symlink_metadata(path).map_err(|error| CoordinatorError::ExternalCommandIo {
            action: action.key.clone(),
            path: path.to_path_buf(),
            operation,
            error,
        })?;
    if is_reparse_point(&metadata) {
        return Err(CoordinatorError::ExternalCommandIo {
            action: action.key.clone(),
            path: path.to_path_buf(),
            operation,
            error: io::Error::new(
                io::ErrorKind::InvalidData,
                "cache input cannot be a symbolic link or reparse point",
            ),
        });
    }
    if metadata.is_file() {
        return read_external_identity_regular_file(
            action,
            path,
            operation,
            ExternalIdentityKind::Input,
            true,
        );
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

fn read_external_identity_regular_file(
    action: &PlanAction,
    path: &Path,
    operation: &'static str,
    kind: ExternalIdentityKind,
    reject_symlink: bool,
) -> Result<ExternalCommandContentIdentity, CoordinatorError> {
    let result = (|| -> io::Result<ExternalCommandContentIdentity> {
        let mut options = fs::OpenOptions::new();
        options.read(true);
        #[cfg(unix)]
        if reject_symlink {
            options.custom_flags(libc::O_NOFOLLOW);
        }
        #[cfg(windows)]
        if reject_symlink {
            options.custom_flags(
                windows_sys::Win32::Storage::FileSystem::FILE_FLAG_OPEN_REPARSE_POINT,
            );
        }
        let mut file = options.open(path)?;
        let metadata = file.metadata()?;
        if !metadata.is_file() || (reject_symlink && is_reparse_point(&metadata)) {
            return Err(io::Error::new(
                io::ErrorKind::InvalidData,
                "cache identity must be a regular, non-link file",
            ));
        }
        match kind {
            ExternalIdentityKind::Tool => {
                ExternalCommandContentIdentity::tool_from_reader(&mut file, metadata.len())
            }
            ExternalIdentityKind::Input => {
                ExternalCommandContentIdentity::input_from_reader(&mut file, metadata.len())
            }
        }
    })();
    result.map_err(|error| CoordinatorError::ExternalCommandIo {
        action: action.key.clone(),
        path: path.to_path_buf(),
        operation,
        error,
    })
}

fn read_external_identity_directory(
    action: &PlanAction,
    path: &Path,
    operation: &'static str,
) -> Result<ExternalCommandContentIdentity, CoordinatorError> {
    let result = (|| -> io::Result<ExternalCommandContentIdentity> {
        let plan = plan_external_identity_directory(path, 0)?;
        ExternalCommandContentIdentity::input_from_encoder(plan.encoded_len, |writer| {
            let mut buffer = [0; EXTERNAL_DIRECTORY_IDENTITY_STREAM_BYTES];
            stream_external_identity_directory(&plan, writer, &mut buffer)
        })
    })();
    result.map_err(|error| CoordinatorError::ExternalCommandIo {
        action: action.key.clone(),
        path: path.to_path_buf(),
        operation,
        error,
    })
}

/// Plans the exact legacy directory encoding without retaining file payloads.
/// The second pass can therefore stream the same bytes into the fingerprint
/// while using memory proportional to directory metadata rather than tree data.
fn plan_external_identity_directory(
    path: &Path,
    depth: usize,
) -> io::Result<ExternalDirectoryIdentityPlan> {
    if depth >= MAX_EXTERNAL_DIRECTORY_DEPTH {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            "cache input directory is nested too deeply",
        ));
    }
    let metadata = fs::symlink_metadata(path)?;
    if is_reparse_point(&metadata) || !metadata.is_dir() {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            format!("cache input directory `{}` changed type", path.display()),
        ));
    }
    // Sort traversal so identity is independent of filesystem enumeration
    // order. Symlinks are rejected because their targets escape the declared
    // input tree and can change without changing the logical input path.
    let mut entries = Vec::new();
    for entry in fs::read_dir(path)? {
        if entries.len() >= MAX_EXTERNAL_DIRECTORY_ENTRIES {
            return Err(io::Error::new(
                io::ErrorKind::InvalidData,
                "cache input directory has too many entries",
            ));
        }
        entries.push(entry?);
    }
    entries.sort_by(|left, right| {
        left.file_name()
            .as_encoded_bytes()
            .cmp(right.file_name().as_encoded_bytes())
    });
    let mut encoded_len = u64::try_from(EXTERNAL_DIRECTORY_IDENTITY_MAGIC.len() + 8)
        .map_err(|_| io::Error::new(io::ErrorKind::InvalidData, "directory header is too large"))?;
    let mut planned = Vec::with_capacity(entries.len());
    for entry in entries {
        let name = entry.file_name();
        let name = name.as_encoded_bytes().to_vec();
        let name_len = u64::try_from(name.len()).map_err(|_| {
            io::Error::new(
                io::ErrorKind::InvalidData,
                "directory entry name is too large",
            )
        })?;
        encoded_len = checked_directory_identity_len(encoded_len, 8)?;
        encoded_len = checked_directory_identity_len(encoded_len, name_len)?;
        encoded_len = checked_directory_identity_len(encoded_len, 1)?;
        let child = entry.path();
        let metadata = fs::symlink_metadata(&child)?;
        if is_reparse_point(&metadata) {
            return Err(io::Error::new(
                io::ErrorKind::InvalidData,
                format!(
                    "cache input tree contains a symbolic link or reparse point `{}`",
                    child.display()
                ),
            ));
        } else if metadata.is_file() {
            encoded_len = checked_directory_identity_len(encoded_len, 8)?;
            encoded_len = checked_directory_identity_len(encoded_len, metadata.len())?;
            planned.push(ExternalDirectoryIdentityEntry {
                name,
                kind: ExternalDirectoryIdentityEntryKind::File {
                    path: child,
                    length: metadata.len(),
                },
            });
        } else if metadata.is_dir() {
            let nested = plan_external_identity_directory(&child, depth + 1)?;
            encoded_len = checked_directory_identity_len(encoded_len, 8)?;
            encoded_len = checked_directory_identity_len(encoded_len, nested.encoded_len)?;
            planned.push(ExternalDirectoryIdentityEntry {
                name,
                kind: ExternalDirectoryIdentityEntryKind::Directory(Box::new(nested)),
            });
        } else {
            return Err(io::Error::new(
                io::ErrorKind::InvalidData,
                format!(
                    "cache input tree contains non-regular entry `{}`",
                    child.display()
                ),
            ));
        }
    }
    Ok(ExternalDirectoryIdentityPlan {
        path: path.to_path_buf(),
        encoded_len,
        entries: planned,
    })
}

fn checked_directory_identity_len(current: u64, added: u64) -> io::Result<u64> {
    let length = current.checked_add(added).ok_or_else(|| {
        io::Error::new(
            io::ErrorKind::InvalidData,
            "cache input directory identity is too large",
        )
    })?;
    if length > MAX_EXTERNAL_DIRECTORY_IDENTITY_BYTES {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            "cache input directory identity is too large",
        ));
    }
    Ok(length)
}

fn stream_external_identity_directory(
    plan: &ExternalDirectoryIdentityPlan,
    writer: &mut QueryFingerprintBytesWriter<'_>,
    buffer: &mut [u8],
) -> io::Result<()> {
    let current_names = sorted_external_directory_names(&plan.path)?;
    if !current_names
        .iter()
        .map(Vec::as_slice)
        .eq(plan.entries.iter().map(|entry| entry.name.as_slice()))
    {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            format!(
                "cache input directory `{}` changed while it was fingerprinted",
                plan.path.display()
            ),
        ));
    }
    writer.write_chunk(EXTERNAL_DIRECTORY_IDENTITY_MAGIC)?;
    writer.write_chunk(&(plan.entries.len() as u64).to_le_bytes())?;
    for entry in &plan.entries {
        writer.write_chunk(&(entry.name.len() as u64).to_le_bytes())?;
        writer.write_chunk(&entry.name)?;
        match &entry.kind {
            ExternalDirectoryIdentityEntryKind::File { path, length } => {
                writer.write_chunk(&[0])?;
                writer.write_chunk(&length.to_le_bytes())?;
                stream_external_identity_file(path, *length, writer, buffer)?;
            }
            ExternalDirectoryIdentityEntryKind::Directory(nested) => {
                writer.write_chunk(&[1])?;
                writer.write_chunk(&nested.encoded_len.to_le_bytes())?;
                stream_external_identity_directory(nested, writer, buffer)?;
            }
        }
    }
    Ok(())
}

fn sorted_external_directory_names(path: &Path) -> io::Result<Vec<Vec<u8>>> {
    let metadata = fs::symlink_metadata(path)?;
    if is_reparse_point(&metadata) || !metadata.is_dir() {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            format!("cache input directory `{}` changed type", path.display()),
        ));
    }
    let mut names = Vec::new();
    for entry in fs::read_dir(path)? {
        if names.len() >= MAX_EXTERNAL_DIRECTORY_ENTRIES {
            return Err(io::Error::new(
                io::ErrorKind::InvalidData,
                "cache input directory has too many entries",
            ));
        }
        names.push(entry?.file_name().as_encoded_bytes().to_vec());
    }
    names.sort();
    Ok(names)
}

fn is_reparse_point(metadata: &fs::Metadata) -> bool {
    #[cfg(windows)]
    {
        use std::os::windows::fs::MetadataExt as _;

        metadata.file_attributes()
            & windows_sys::Win32::Storage::FileSystem::FILE_ATTRIBUTE_REPARSE_POINT
            != 0
    }
    #[cfg(not(windows))]
    {
        let _ = metadata;
        false
    }
}

fn stream_external_identity_file(
    path: &Path,
    expected_len: u64,
    writer: &mut QueryFingerprintBytesWriter<'_>,
    buffer: &mut [u8],
) -> io::Result<()> {
    let mut options = fs::OpenOptions::new();
    options.read(true);
    #[cfg(unix)]
    options.custom_flags(libc::O_NOFOLLOW);
    #[cfg(windows)]
    options.custom_flags(windows_sys::Win32::Storage::FileSystem::FILE_FLAG_OPEN_REPARSE_POINT);
    let mut file = options.open(path)?;
    let metadata = file.metadata()?;
    if !metadata.is_file() || is_reparse_point(&metadata) || metadata.len() != expected_len {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            format!(
                "cache input file `{}` changed while it was fingerprinted",
                path.display()
            ),
        ));
    }
    let mut remaining = expected_len;
    while remaining != 0 {
        let chunk_len = crate::stream_chunk_len(remaining, buffer.len());
        file.read_exact(&mut buffer[..chunk_len])?;
        writer.write_chunk(&buffer[..chunk_len])?;
        remaining -= chunk_len as u64;
    }
    let mut trailing = [0; 1];
    if file.read(&mut trailing)? != 0 {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            format!(
                "cache input file `{}` grew while it was fingerprinted",
                path.display()
            ),
        ));
    }
    Ok(())
}

pub(super) fn open_staged_external_outputs(
    action: &PlanAction,
    staged: &StagedOutputTransaction,
) -> Result<Vec<ExternalCommandCacheOutput>, CoordinatorError> {
    staged
        .outputs
        .iter()
        .map(|output| {
            open_staged_external_output(&output.temporary).map_err(|error| {
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

fn open_staged_external_output(path: &Path) -> io::Result<ExternalCommandCacheOutput> {
    let mut options = fs::OpenOptions::new();
    options.read(true);
    #[cfg(unix)]
    options.custom_flags(libc::O_NOFOLLOW);
    ExternalCommandCacheOutput::from_file(options.open(path)?)
}

pub(super) fn restore_cached_external_outputs(
    action: &PlanAction,
    build_dir: &Path,
    resolved_outputs: &[(&LogicalPath, PathBuf)],
    mut hit: ExternalCommandCacheHit,
) -> Result<(), CoordinatorError> {
    if resolved_outputs.len() != hit.output_count() {
        return Err(inconsistent(
            format!("action `{}`", action.key.name()),
            "matching cached external-command outputs".to_string(),
        ));
    }
    let staged = prepare_staged_outputs(action, build_dir, resolved_outputs)?;
    for index in 0..hit.output_count() {
        let temporary = staged.outputs[index].temporary.clone();
        let restored = fs::File::create(&temporary)
            .and_then(|mut output| hit.write_payload(index, &mut output));
        if let Err(error) = restored {
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
    pub(super) detect_undeclared_outputs: bool,
    /// Physical final and staged paths that the command is allowed to write.
    pub(super) declared_output_paths: &'a [PathBuf],
}

/// Provides cacheable commands with a filesystem view containing only their
/// declared inputs. Relative access to an undeclared package or build file
/// therefore fails in the child process instead of silently becoming an
/// untracked cache input.
struct DeclaredInputIsolation {
    root: PathBuf,
    mappings: Vec<(PathBuf, PathBuf)>,
}

impl DeclaredInputIsolation {
    fn create(
        action: &PlanAction,
        working_directory: &Path,
        inputs: &[PathBuf],
    ) -> Result<Self, CoordinatorError> {
        let root = std::env::temp_dir().join(format!(
            "nia-command-inputs-{}-{}-{}",
            std::process::id(),
            action.key.name(),
            INPUT_ISOLATION_SEQUENCE.fetch_add(1, Ordering::Relaxed),
        ));
        fs::create_dir(&root).map_err(|error| CoordinatorError::ExternalCommandIo {
            action: action.key.clone(),
            path: root.clone(),
            operation: "create command input isolation",
            error,
        })?;
        let result = (|| {
            let mut mappings = Vec::with_capacity(inputs.len());
            let mut entries_seen = 0;
            let mut bytes_copied = 0;
            // Copy an ancestor before any of its declared descendants. A
            // command may declare both a directory and a file below it, and
            // the resulting view must not depend on declaration order.
            let mut ordered_inputs = inputs.iter().enumerate().collect::<Vec<_>>();
            ordered_inputs.sort_by(|(left_index, left), (right_index, right)| {
                left.components()
                    .count()
                    .cmp(&right.components().count())
                    .then_with(|| left.cmp(right))
                    .then_with(|| left_index.cmp(right_index))
            });
            for (index, input) in ordered_inputs {
                let destination = if let Ok(relative) = input.strip_prefix(working_directory) {
                    root.join(relative)
                } else {
                    root.join(".nia-inputs").join(index.to_string()).join(
                        input.file_name().ok_or_else(|| {
                            io::Error::new(
                                io::ErrorKind::InvalidInput,
                                "declared command input has no file name",
                            )
                        })?,
                    )
                };
                copy_isolated_path(input, &destination, 0, &mut entries_seen, &mut bytes_copied)?;
                mappings.push((input.clone(), destination));
            }
            Ok(Self {
                root: root.clone(),
                mappings,
            })
        })();
        match result {
            Ok(isolation) => Ok(isolation),
            Err(error) => {
                let _ = fs::remove_dir_all(&root);
                Err(CoordinatorError::ExternalCommandIo {
                    action: action.key.clone(),
                    path: root,
                    operation: "copy command declared inputs",
                    error,
                })
            }
        }
    }

    fn map_path(&self, path: &Path) -> PathBuf {
        self.mappings
            .iter()
            .find_map(|(source, destination)| {
                path.strip_prefix(source).ok().map(|relative| {
                    if relative.as_os_str().is_empty() {
                        destination.clone()
                    } else {
                        destination.join(relative)
                    }
                })
            })
            .unwrap_or_else(|| path.to_path_buf())
    }
}

impl Drop for DeclaredInputIsolation {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.root);
    }
}

fn map_isolated_arguments(
    isolation: &DeclaredInputIsolation,
    arguments: &[String],
    input_argument_indices: &[usize],
) -> Vec<String> {
    arguments
        .iter()
        .enumerate()
        .map(|(index, argument)| {
            if input_argument_indices.contains(&index) {
                isolation
                    .map_path(Path::new(argument))
                    .to_string_lossy()
                    .into_owned()
            } else {
                argument.clone()
            }
        })
        .collect()
}

fn copy_isolated_path(
    source: &Path,
    destination: &Path,
    depth: usize,
    entries_seen: &mut usize,
    bytes_copied: &mut u64,
) -> io::Result<()> {
    if depth > MAX_EXTERNAL_DIRECTORY_DEPTH {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            "declared command input exceeds the isolation depth limit",
        ));
    }
    *entries_seen += 1;
    if *entries_seen > MAX_EXTERNAL_DIRECTORY_ENTRIES {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            "declared command inputs exceed the isolation entry limit",
        ));
    }
    let metadata = fs::symlink_metadata(source)?;
    if is_reparse_point(&metadata) {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            "declared command input cannot be a link or reparse point",
        ));
    }
    if metadata.is_dir() {
        fs::create_dir_all(destination)?;
        for entry in fs::read_dir(source)? {
            let entry = entry?;
            copy_isolated_path(
                &entry.path(),
                &destination.join(entry.file_name()),
                depth + 1,
                entries_seen,
                bytes_copied,
            )?;
        }
        fs::set_permissions(destination, metadata.permissions())?;
    } else if metadata.is_file() {
        *bytes_copied = bytes_copied
            .checked_add(metadata.len())
            .filter(|bytes| *bytes <= MAX_ISOLATED_INPUT_BYTES)
            .ok_or_else(|| {
                io::Error::new(
                    io::ErrorKind::InvalidData,
                    "declared command inputs exceed the isolation byte limit",
                )
            })?;
        if let Some(parent) = destination.parent() {
            fs::create_dir_all(parent)?;
        }
        fs::copy(source, destination)?;
        fs::set_permissions(destination, metadata.permissions())?;
    } else {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            "declared command input is not a regular file or directory",
        ));
    }
    Ok(())
}

pub(super) fn execute_external_command_with_input_isolation(
    action: &PlanAction,
    request: ResolvedExternalCommand<'_>,
    policy: ExternalExecutionPolicy,
    inputs: &[PathBuf],
    input_argument_indices: &[usize],
    output_writer: Option<Arc<dyn crate::ProcessOutputWriter>>,
) -> Result<(), CoordinatorError> {
    let isolation = DeclaredInputIsolation::create(action, request.working_directory, inputs)?;
    let arguments = map_isolated_arguments(&isolation, request.arguments, input_argument_indices);
    let stdin = request.stdin.map(|path| isolation.map_path(path));
    let working_directory = isolation.root.clone();
    execute_external_command_with_writer(
        action,
        ResolvedExternalCommand {
            program: request.program,
            arguments: &arguments,
            working_directory: &working_directory,
            environment_policy: request.environment_policy,
            environment: request.environment,
            stdin: stdin.as_deref(),
        },
        policy,
        output_writer,
    )
}

#[cfg(test)]
pub(super) fn execute_external_command(
    action: &PlanAction,
    request: ResolvedExternalCommand<'_>,
    policy: ExternalExecutionPolicy,
) -> Result<(), CoordinatorError> {
    execute_external_command_with_writer(action, request, policy, None)
}

pub(super) fn execute_external_command_with_writer(
    action: &PlanAction,
    request: ResolvedExternalCommand<'_>,
    policy: ExternalExecutionPolicy,
    output_writer: Option<Arc<dyn crate::ProcessOutputWriter>>,
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

    // Capture file state before execution for undeclared output detection
    let (declared_outputs, pre_execution_state) = if policy.detect_undeclared_outputs {
        let declared_outputs = policy.declared_output_paths.iter().cloned().collect();
        let pre_state = collect_directory_state(
            &action.key,
            request.working_directory,
            "snapshot command outputs",
        )?;
        (declared_outputs, Some(pre_state))
    } else {
        (HashSet::new(), None)
    };

    let mut command = Command::new(request.program);
    command
        .args(request.arguments)
        .current_dir(request.working_directory)
        .stdin(match request.stdin {
            Some(path) => Stdio::from(fs::File::open(path).map_err(|source| {
                CoordinatorError::ExternalCommandIo {
                    action: action.key.clone(),
                    path: path.to_path_buf(),
                    operation: "open stdin",
                    error: source,
                }
            })?),
            None => Stdio::null(),
        })
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
    let stderr_output_writer = output_writer.clone();
    let stdout_reader = match thread::Builder::new()
        .name("nia-build-stdout".to_string())
        .spawn(move || {
            capture_stream(
                stdout,
                CapturedStream::Stdout,
                policy.forward_output,
                EXTERNAL_OUTPUT_TAIL_BYTES,
                output_writer,
            )
        }) {
        Ok(reader) => reader,
        Err(source) => {
            terminate_process_tree(&mut child);
            let _ = child.wait();
            return Err(error(ExternalCommandFailure::CaptureWorkerSpawn {
                stream: "stdout",
                error: source,
                stdout: Vec::new(),
                stderr: Vec::new(),
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
                stderr_output_writer,
            )
        }) {
        Ok(reader) => reader,
        Err(source) => {
            terminate_process_tree(&mut child);
            let _ = child.wait();
            let stdout = join_capture_tail(stdout_reader);
            return Err(error(ExternalCommandFailure::CaptureWorkerSpawn {
                stream: "stderr",
                error: source,
                stdout,
                stderr: Vec::new(),
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
    let result = finish_external_command(
        status,
        stdout_reader,
        stderr_reader,
        timed_out.then_some(policy.timeout),
        cancelled,
    );

    // Check for undeclared outputs after successful command execution
    match result {
        Ok((stdout, stderr)) => {
            if let Some(pre_state) = pre_execution_state {
                let (paths, count) = detect_undeclared_outputs(
                    &action.key,
                    request.working_directory,
                    &declared_outputs,
                    &pre_state,
                )?;
                if count != 0 {
                    return Err(error(ExternalCommandFailure::UndeclaredOutputs {
                        paths,
                        count,
                        stdout,
                        stderr,
                    }));
                }
            }
            Ok(())
        }
        Err(failure) => Err(error(failure)),
    }
}

fn finish_external_command(
    status: io::Result<ExitStatus>,
    stdout_reader: thread::JoinHandle<StreamCapture>,
    stderr_reader: thread::JoinHandle<StreamCapture>,
    timeout: Option<Duration>,
    cancelled: bool,
) -> Result<(Vec<u8>, Vec<u8>), ExternalCommandFailure> {
    let output = join_captures(stdout_reader, stderr_reader);
    let stdout = output.stdout;
    let stderr = output.stderr;
    if let Some(failure) = output.failure {
        return Err(match failure {
            CaptureFailure::Thread { stream } => ExternalCommandFailure::CaptureThread {
                stream,
                stdout,
                stderr,
            },
            CaptureFailure::Io { stream, error } => ExternalCommandFailure::StreamIo {
                stream,
                error,
                stdout,
                stderr,
            },
        });
    }
    let status = match status {
        Ok(status) => status,
        Err(error) => {
            return Err(ExternalCommandFailure::Wait {
                error,
                stdout,
                stderr,
            });
        }
    };
    if cancelled {
        return Err(ExternalCommandFailure::Cancelled { stdout, stderr });
    }
    if let Some(timeout) = timeout {
        return Err(ExternalCommandFailure::TimedOut {
            timeout,
            stdout,
            stderr,
        });
    }
    if !status.success() {
        return Err(ExternalCommandFailure::Exit {
            status,
            stdout,
            stderr,
        });
    }
    Ok((stdout, stderr))
}

pub(super) fn display_external_command_error(
    f: &mut fmt::Formatter<'_>,
    details: &ExternalCommandError,
) -> fmt::Result {
    write!(
        f,
        "external command action `{}` in package `{}` failed to run `{}` with {} argument(s) in `{}`: ",
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
        ExternalCommandFailure::Wait {
            error,
            stdout,
            stderr,
        } => {
            write!(f, "wait failed: {error}")?;
            display_output_tails(f, stdout, stderr)
        }
        ExternalCommandFailure::CaptureThread {
            stream,
            stdout,
            stderr,
        } => {
            write!(f, "{stream} capture worker failed")?;
            display_output_tails(f, stdout, stderr)
        }
        ExternalCommandFailure::CaptureWorkerSpawn {
            stream,
            error,
            stdout,
            stderr,
        } => {
            write!(f, "failed to start {stream} capture worker: {error}")?;
            display_output_tails(f, stdout, stderr)
        }
        ExternalCommandFailure::StreamIo {
            stream,
            error,
            stdout,
            stderr,
        } => {
            write!(f, "{stream} capture/forward failed: {error}")?;
            display_output_tails(f, stdout, stderr)
        }
        ExternalCommandFailure::TimedOut {
            timeout,
            stdout,
            stderr,
        } => {
            write!(f, "timed out after {}", display_duration(*timeout))?;
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
            write!(f, "{}", nia_diagnostic::describe_exit_status(*status))?;
            display_output_tails(f, stdout, stderr)
        }
        ExternalCommandFailure::UndeclaredOutputs {
            paths,
            count,
            stdout,
            stderr,
        } => {
            write!(f, "changed {count} undeclared output(s): ")?;
            for (i, path) in paths.iter().enumerate() {
                if i > 0 {
                    write!(f, ", ")?;
                }
                write!(f, "`{}`", path.display())?;
                if i >= 4 && *count > 5 {
                    write!(f, ", ... and {} more", count - 5)?;
                    break;
                }
            }
            display_output_tails(f, stdout, stderr)
        }
    }
}

pub(super) fn is_cancellation_error(error: &CoordinatorError) -> bool {
    match error {
        CoordinatorError::Internal(_) => false,
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{
        BuildError,
        process_output::tests::{capture_worker, success_status},
        report::tests::assert_operational_report,
    };

    #[test]
    fn completion_failures_preserve_captured_output_in_all_report_contexts() {
        for phase in ["stdout", "stderr", "wait", "worker"] {
            for context in ["action", "suite", "cleanup"] {
                let stdout = if phase == "worker" {
                    thread::spawn(|| panic!("injected stdout capture panic"))
                } else {
                    capture_worker(CapturedStream::Stdout, phase == "stdout")
                };
                let stderr = capture_worker(CapturedStream::Stderr, phase == "stderr");
                let status = if phase == "wait" {
                    Err(io::Error::other("injected wait failure"))
                } else {
                    Ok(success_status())
                };
                let failure =
                    finish_external_command(status, stdout, stderr, None, false).unwrap_err();
                let reason = match (&failure, phase) {
                    (ExternalCommandFailure::StreamIo { stream, .. }, "stdout" | "stderr") => {
                        assert_eq!(*stream, phase);
                        format!("{phase} capture/forward failed: injected read failure")
                    }
                    (ExternalCommandFailure::Wait { .. }, "wait") => {
                        "wait failed: injected wait failure".to_string()
                    }
                    (
                        ExternalCommandFailure::CaptureThread {
                            stream: "stdout", ..
                        },
                        "worker",
                    ) => "stdout capture worker failed".to_string(),
                    _ => panic!("unexpected failure: {failure:?}"),
                };
                let action = ActionKey::new(PackageKey::root(), "capture-check").unwrap();
                let error = CoordinatorError::ExternalCommand(Box::new(ExternalCommandError {
                    action: action.clone(),
                    program: "test-tool".into(),
                    arguments: Vec::new(),
                    working_directory: PathBuf::from("work"),
                    failure,
                }));
                let mut markers = vec![reason.as_str(), "stderr-tail"];
                if phase != "worker" {
                    markers.push("stdout-tail");
                }
                let display = error.to_string();
                for marker in &markers {
                    assert!(display.contains(marker), "{display}");
                }
                let error = match context {
                    "suite" => CoordinatorError::Failures(vec![ActionFailure {
                        action,
                        test: true,
                        error: Box::new(error),
                    }]),
                    "cleanup" => CoordinatorError::StagedOutput {
                        action,
                        path: PathBuf::from("staged"),
                        operation: "remove",
                        error: io::Error::other("injected cleanup failure"),
                        cause: Some(Box::new(error)),
                    },
                    _ => error,
                };
                assert_operational_report(
                    &BuildError::ExecuteBuildPlan {
                        error: Box::new(error),
                    },
                    "E0705",
                    &markers,
                );
            }
        }
    }

    #[test]
    fn completion_preserves_success_timeout_and_cancellation_precedence() {
        for (timeout, cancelled) in [
            (None, false),
            (Some(Duration::from_secs(2)), false),
            (Some(Duration::from_secs(2)), true),
        ] {
            let result = finish_external_command(
                Ok(success_status()),
                capture_worker(CapturedStream::Stdout, false),
                capture_worker(CapturedStream::Stderr, false),
                timeout,
                cancelled,
            );
            match (result, timeout, cancelled) {
                (Ok((_stdout, _stderr)), None, false) => {}
                (
                    Err(ExternalCommandFailure::TimedOut {
                        timeout,
                        stdout,
                        stderr,
                    }),
                    Some(expected),
                    false,
                ) => {
                    assert_eq!(timeout, expected);
                    assert_eq!(stdout, b"stdout-tail");
                    assert_eq!(stderr, b"stderr-tail");
                }
                (Err(ExternalCommandFailure::Cancelled { stdout, stderr }), _, true) => {
                    assert_eq!(stdout, b"stdout-tail");
                    assert_eq!(stderr, b"stderr-tail");
                }
                other => panic!("unexpected result: {other:?}"),
            }
        }
    }

    #[test]
    fn directory_identity_length_has_a_hard_bound() {
        assert!(
            checked_directory_identity_len(MAX_EXTERNAL_DIRECTORY_IDENTITY_BYTES - 1, 1).is_ok()
        );
        let error = checked_directory_identity_len(MAX_EXTERNAL_DIRECTORY_IDENTITY_BYTES, 1)
            .expect_err("directory identity must be bounded");
        assert_eq!(error.kind(), io::ErrorKind::InvalidData);
    }

    #[test]
    fn undeclared_output_detection_distinguishes_created_and_declared_files() {
        use std::time::SystemTime;

        let root = std::env::temp_dir().join(format!(
            "nia-build-output-scan-{}-{}",
            std::process::id(),
            SystemTime::now()
                .duration_since(SystemTime::UNIX_EPOCH)
                .expect("test clock must follow the Unix epoch")
                .as_nanos()
        ));
        fs::create_dir_all(&root).expect("create output scan root");
        let declared = root.join("declared.txt");
        let declared_directory = root.join("declared-directory");
        let unexpected = root.join("unexpected.txt");
        let deleted = root.join("deleted.txt");
        let created = root.join("created.txt");
        fs::write(&declared, b"before").expect("write declared file");
        fs::write(&unexpected, b"before").expect("write existing undeclared file");
        fs::write(&deleted, b"deleted").expect("write file to delete");
        fs::create_dir_all(&declared_directory).expect("create declared directory");
        let action = ActionKey::new(PackageKey::root(), "scan-test").unwrap();
        let before = collect_directory_state(&action, &root, "test output snapshot")
            .expect("capture output scan state");
        fs::write(&declared, b"after").expect("update declared file");
        fs::write(declared_directory.join("nested.txt"), b"declared")
            .expect("write nested declared file");
        fs::write(&unexpected, b"after!").expect("modify undeclared file");
        fs::remove_file(&deleted).expect("delete undeclared file");
        fs::write(&created, b"created").expect("create undeclared file");

        let undeclared = detect_undeclared_outputs(
            &action,
            &root,
            &HashSet::from([declared, declared_directory]),
            &before,
        )
        .expect("scan output state");
        assert_eq!(undeclared, (vec![created, deleted, unexpected], 3));
        let _ = fs::remove_dir_all(root);
    }

    #[cfg(unix)]
    #[test]
    fn input_isolation_preserves_executable_permissions() {
        use std::os::unix::fs::PermissionsExt;

        let root = std::env::temp_dir().join(format!(
            "nia-build-input-isolation-{}-{}",
            std::process::id(),
            INPUT_ISOLATION_SEQUENCE.fetch_add(1, Ordering::Relaxed)
        ));
        fs::create_dir_all(&root).expect("create isolation test root");
        let source = root.join("tool");
        fs::write(&source, b"#!/bin/sh\nexit 0\n").expect("write executable input");
        fs::set_permissions(&source, fs::Permissions::from_mode(0o755))
            .expect("set executable input mode");
        let action = PlanAction {
            key: ActionKey::new(PackageKey::root(), "isolation-test").unwrap(),
            kind: ActionKind::Aggregate,
        };
        let isolation = DeclaredInputIsolation::create(&action, &root, &[source.clone()])
            .expect("create input isolation");
        let copied = isolation.map_path(&source);
        assert_ne!(
            fs::metadata(copied).unwrap().permissions().mode() & 0o111,
            0
        );
        drop(isolation);
        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn input_isolation_is_independent_of_overlapping_input_order() {
        let root = std::env::temp_dir().join(format!(
            "nia-build-input-isolation-overlap-{}-{}",
            std::process::id(),
            INPUT_ISOLATION_SEQUENCE.fetch_add(1, Ordering::Relaxed)
        ));
        fs::create_dir_all(root.join("inputs")).expect("create overlap input directory");
        let directory = root.join("inputs");
        let nested = directory.join("nested.txt");
        fs::write(&nested, b"declared").expect("write nested input");
        let action = PlanAction {
            key: ActionKey::new(PackageKey::root(), "isolation-overlap").unwrap(),
            kind: ActionKind::Aggregate,
        };
        // The file is deliberately listed first. The isolation builder sorts
        // the ancestor directory ahead of it before copying.
        let isolation =
            DeclaredInputIsolation::create(&action, &root, &[nested.clone(), directory.clone()])
                .expect("create overlapping input isolation");
        let copied = isolation.map_path(&nested);
        assert_eq!(fs::read(&copied).unwrap(), b"declared");
        drop(isolation);
        assert!(!copied.exists(), "temporary input view must be cleaned up");
        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn input_isolation_maps_only_typed_input_arguments() {
        let root = std::env::temp_dir().join(format!(
            "nia-build-input-isolation-arguments-{}-{}",
            std::process::id(),
            INPUT_ISOLATION_SEQUENCE.fetch_add(1, Ordering::Relaxed)
        ));
        fs::create_dir_all(&root).expect("create argument isolation root");
        let source = root.join("input.txt");
        fs::write(&source, b"declared").expect("write argument input");
        let action = PlanAction {
            key: ActionKey::new(PackageKey::root(), "isolation-arguments").unwrap(),
            kind: ActionKind::Aggregate,
        };
        let isolation =
            DeclaredInputIsolation::create(&action, &root, std::slice::from_ref(&source))
                .expect("create argument isolation");
        let arguments = vec![source.to_string_lossy().into_owned(); 2];
        let mapped = map_isolated_arguments(&isolation, &arguments, &[1]);
        assert_eq!(mapped[0], arguments[0]);
        assert_ne!(mapped[1], arguments[1]);
        assert_eq!(fs::read(&mapped[1]).unwrap(), b"declared");
        drop(isolation);
        let _ = fs::remove_dir_all(root);
    }
}

/// Content snapshot for undeclared-output detection.
#[derive(Debug, Clone)]
struct FileStateSnapshot {
    files: HashMap<PathBuf, [u8; blake3::OUT_LEN]>,
}

/// Recursively collects content fingerprints of files under a directory.
fn collect_directory_state(
    action: &ActionKey,
    root: &Path,
    operation: &'static str,
) -> Result<FileStateSnapshot, CoordinatorError> {
    let mut files = HashMap::new();
    let mut entries_seen = 0;
    collect_directory_state_recursive(root, root, &mut files, &mut entries_seen, 0).map_err(
        |error| CoordinatorError::ExternalCommandIo {
            action: action.clone(),
            path: root.to_path_buf(),
            operation,
            error,
        },
    )?;
    Ok(FileStateSnapshot { files })
}

fn collect_directory_state_recursive(
    root: &Path,
    current: &Path,
    files: &mut HashMap<PathBuf, [u8; blake3::OUT_LEN]>,
    entries_seen: &mut usize,
    depth: usize,
) -> io::Result<()> {
    if depth > MAX_EXTERNAL_DIRECTORY_DEPTH {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            "command working directory exceeds the output scan depth limit",
        ));
    }
    let entries = fs::read_dir(current)?;
    for entry in entries {
        *entries_seen += 1;
        if *entries_seen > MAX_EXTERNAL_DIRECTORY_ENTRIES {
            return Err(io::Error::new(
                io::ErrorKind::InvalidData,
                "command working directory exceeds the output scan entry limit",
            ));
        }
        let entry = entry?;
        let path = entry.path();
        // Do not follow links while inspecting a command's working tree. A
        // link can point outside the declared working directory and must not
        // turn that external tree into an observed output set.
        let metadata = fs::symlink_metadata(&path)?;
        if is_reparse_point(&metadata) {
            continue;
        }
        if metadata.is_file() {
            let relative = path.strip_prefix(root).map_err(|_| {
                io::Error::new(
                    io::ErrorKind::InvalidData,
                    "command output path escaped its working directory",
                )
            })?;
            let mut file = fs::File::open(&path)?;
            let mut hasher = blake3::Hasher::new();
            let mut buffer = [0; 16 * 1024];
            loop {
                let bytes_read = file.read(&mut buffer)?;
                if bytes_read == 0 {
                    break;
                }
                hasher.update(&buffer[..bytes_read]);
            }
            files.insert(relative.to_path_buf(), *hasher.finalize().as_bytes());
        } else if metadata.is_dir() {
            collect_directory_state_recursive(root, &path, files, entries_seen, depth + 1)?;
        }
    }
    Ok(())
}

/// Detects file changes after command execution that were not declared in outputs.
fn detect_undeclared_outputs(
    action: &ActionKey,
    working_directory: &Path,
    declared_outputs: &HashSet<PathBuf>,
    pre_state: &FileStateSnapshot,
) -> Result<(Vec<PathBuf>, usize), CoordinatorError> {
    let post_state = collect_directory_state(action, working_directory, "scan command outputs")?;
    let mut samples = BTreeSet::new();
    let mut count = 0;

    for (path, post_fingerprint) in &post_state.files {
        let absolute = working_directory.join(path);
        if declared_outputs
            .iter()
            .any(|declared| absolute == *declared || absolute.starts_with(declared))
        {
            continue;
        }
        let changed = match pre_state.files.get(path) {
            Some(pre_fingerprint) => post_fingerprint != pre_fingerprint,
            None => true,
        };
        if changed {
            count += 1;
            if samples.len() < 10 {
                samples.insert(absolute);
            } else if samples.first().is_some_and(|first| absolute < *first) {
                samples.pop_first();
                samples.insert(absolute);
            }
        }
    }

    for path in pre_state.files.keys() {
        let absolute = working_directory.join(path);
        if !post_state.files.contains_key(path)
            && !declared_outputs
                .iter()
                .any(|declared| absolute == *declared || absolute.starts_with(declared))
        {
            count += 1;
            if samples.len() < 10 {
                samples.insert(absolute);
            } else if samples.first().is_some_and(|first| absolute < *first) {
                samples.pop_first();
                samples.insert(absolute);
            }
        }
    }

    Ok((samples.into_iter().collect(), count))
}
