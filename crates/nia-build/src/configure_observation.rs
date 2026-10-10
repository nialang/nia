// SPDX-License-Identifier: GPL-3.0-or-later
//! Typed, deterministic inputs observed while configuring a build.
//!
//! Configure observations are deliberately separate from action inputs. They
//! describe what a build script was allowed to inspect before producing a
//! `BuildPlan`, and therefore form the future configure-cache contract.

use std::{
    ffi::OsStr,
    fmt, fs, io,
    io::Read,
    path::{Path, PathBuf},
    process::{Command, Stdio},
    time::{Duration, Instant},
};

use nia_query::FingerprintDomain;

const CONFIGURE_MANIFEST_DOMAIN: FingerprintDomain =
    FingerprintDomain::new("nia.build.configure-manifest");
const MAX_MANIFEST_BYTES: usize = 16 * 1024 * 1024;
pub(crate) const MAX_DIRECTORY_BYTES: usize = 1024 * 1024;
const MAX_DIRECTORY_ENTRIES: usize = 16 * 1024;

/// Policy for host observations that are not explicitly declared by a build.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum ObservationPolicy {
    /// Reject an undeclared observation as a non-hermetic configuration.
    Disallow,
    /// Record the observation and force a cache miss on every reuse attempt.
    Poison,
    /// Permit the observation without making it part of cache identity.
    Ignore,
}

/// The kind of filesystem fact a configuration observed.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum ObservationKind {
    /// File bytes and stable relative identity.
    FileContents,
    /// File metadata such as length and modification identity.
    FileMetadata,
    /// Sorted direct directory entries.
    DirectoryContents,
    /// Directory metadata without enumerating children.
    DirectoryMetadata,
    /// Whether a package-relative file-system entry exists and its entry kind.
    ///
    /// This is the explicit absent-value observation used by configuration
    /// probes that intentionally branch on an optional file.
    FilePresence,
    /// Whether a package-relative directory exists.
    DirectoryPresence,
    /// The UTF-8 value of a named host environment variable, including an
    /// explicit fingerprint for an absent variable.
    EnvironmentValue,
    /// A bounded command result with its explicit argument request.
    CommandResult,
    /// The repository state reported by Git.
    GitState,
    /// A resolved tool executable identity and version output.
    ToolIdentity,
}

impl fmt::Display for ObservationKind {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(match self {
            Self::FileContents => "file-contents",
            Self::FileMetadata => "file-metadata",
            Self::DirectoryContents => "directory-contents",
            Self::DirectoryMetadata => "directory-metadata",
            Self::FilePresence => "file-presence",
            Self::DirectoryPresence => "directory-presence",
            Self::EnvironmentValue => "environment-value",
            Self::CommandResult => "command-result",
            Self::GitState => "git-state",
            Self::ToolIdentity => "tool-identity",
        })
    }
}

/// A normalized filesystem observation recorded by a build runner.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct ConfigureObservation {
    path: PathBuf,
    kind: ObservationKind,
    fingerprint: [u8; 32],
    request: Vec<u8>,
}

/// Result of comparing a persisted observation with the current filesystem.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ObservationValidation {
    /// Every observed fact still has the recorded fingerprint.
    Match,
    /// An observed path exists but its fact changed.
    Changed {
        path: PathBuf,
        kind: ObservationKind,
    },
    /// An observed path cannot be read in the current checkout.
    Missing {
        path: PathBuf,
        kind: ObservationKind,
    },
}

impl ConfigureObservation {
    /// Creates an observation with a non-empty, relative logical path.
    pub fn new(
        path: impl Into<PathBuf>,
        kind: ObservationKind,
        fingerprint: [u8; 32],
    ) -> Result<Self, ObservationError> {
        let path = path.into();
        if path.as_os_str().is_empty() {
            return Err(ObservationError::EmptyPath);
        }
        let text = path
            .to_str()
            .ok_or_else(|| ObservationError::NonUtf8Path(path.clone()))?;
        if kind == ObservationKind::EnvironmentValue {
            if text.contains('\0') || text.contains('=') {
                return Err(ObservationError::InvalidEnvironmentName(path));
            }
            return Ok(Self {
                path,
                kind,
                fingerprint,
                request: Vec::new(),
            });
        }
        if path.is_absolute()
            || text.starts_with('/')
            || text.starts_with('\\')
            || text.as_bytes().get(1).is_some_and(|byte| *byte == b':')
        {
            return Err(ObservationError::AbsolutePath(path));
        }
        if path.components().any(|component| {
            matches!(
                component,
                std::path::Component::ParentDir | std::path::Component::CurDir
            )
        }) {
            return Err(ObservationError::NonCanonicalPath(path));
        }
        Ok(Self {
            path,
            kind,
            fingerprint,
            request: Vec::new(),
        })
    }

    /// Creates an observation carrying an explicit host request.
    pub fn new_with_request(
        path: impl Into<PathBuf>,
        kind: ObservationKind,
        fingerprint: [u8; 32],
        request: Vec<u8>,
    ) -> Result<Self, ObservationError> {
        let mut observation = Self::new(path, kind, fingerprint)?;
        if !matches!(
            kind,
            ObservationKind::CommandResult
                | ObservationKind::GitState
                | ObservationKind::ToolIdentity
        ) {
            return Err(ObservationError::InvalidRequest);
        }
        observation.request = request;
        Ok(observation)
    }

    /// Returns the normalized relative path.
    pub fn path(&self) -> &std::path::Path {
        &self.path
    }

    /// Returns the observed fact kind.
    pub fn kind(&self) -> ObservationKind {
        self.kind
    }

    /// Returns the content-addressed fact fingerprint.
    pub fn fingerprint(&self) -> &[u8; 32] {
        &self.fingerprint
    }

    /// Returns the explicit host request carried by command-like observations.
    pub fn request(&self) -> &[u8] {
        &self.request
    }

    /// Recomputes this observation against a package root.
    pub fn refresh(&self, root: &Path) -> io::Result<Self> {
        match self.kind {
            ObservationKind::FileContents => observe_file_contents(root, self.path.clone()),
            ObservationKind::FileMetadata => observe_metadata(root, self.path.clone()),
            ObservationKind::DirectoryContents => {
                observe_directory_contents(root, self.path.clone())
            }
            ObservationKind::DirectoryMetadata => {
                observe_directory_metadata(root, self.path.clone())
            }
            ObservationKind::FilePresence => observe_file_presence(root, self.path.clone()),
            ObservationKind::DirectoryPresence => {
                observe_directory_presence(root, self.path.clone())
            }
            ObservationKind::EnvironmentValue => observe_environment(self.path.clone()),
            ObservationKind::CommandResult => observe_command(&self.path, &self.request, root),
            ObservationKind::GitState => observe_git_state(root, &self.request),
            ObservationKind::ToolIdentity => observe_tool(&self.path, &self.request, root),
        }
    }
}

/// Records the contents of a package-relative regular file.
pub fn observe_file_contents(
    root: &Path,
    relative: impl Into<PathBuf>,
) -> io::Result<ConfigureObservation> {
    let relative = relative.into();
    let path = resolve_observation_path(root, &relative)?;
    let bytes = fs::read(path)?;
    ConfigureObservation::new(
        relative,
        ObservationKind::FileContents,
        *blake3::hash(&bytes).as_bytes(),
    )
    .map_err(|error| io::Error::new(io::ErrorKind::InvalidInput, error))
}

/// Records deterministic direct-entry names and metadata for a package-relative directory.
pub fn observe_directory_contents(
    root: &Path,
    relative: impl Into<PathBuf>,
) -> io::Result<ConfigureObservation> {
    let relative = relative.into();
    let entries = read_directory_facts(root, &relative)?;
    directory_observation(relative, &entries)
}

pub(crate) type DirectoryFact = (Vec<u8>, u8, u64);

pub(crate) fn read_directory_facts(root: &Path, relative: &Path) -> io::Result<Vec<DirectoryFact>> {
    let directory = resolve_observation_path(root, relative)?;
    let mut entries = Vec::new();
    let mut size = 4usize;
    for entry in fs::read_dir(directory)? {
        let entry = entry?;
        let metadata = fs::symlink_metadata(entry.path())?;
        let name = entry.file_name();
        #[cfg(unix)]
        let name = {
            use std::os::unix::ffi::OsStrExt;
            name.as_bytes().to_vec()
        };
        #[cfg(not(unix))]
        let name = name
            .to_str()
            .ok_or_else(|| {
                io::Error::new(
                    io::ErrorKind::InvalidData,
                    "directory entry name is not UTF-8",
                )
            })?
            .as_bytes()
            .to_vec();
        size = size
            .checked_add(13 + name.len())
            .filter(|size| *size <= MAX_DIRECTORY_BYTES)
            .ok_or_else(|| {
                io::Error::new(
                    io::ErrorKind::InvalidData,
                    "directory observation is too large",
                )
            })?;
        if entries.len() >= MAX_DIRECTORY_ENTRIES {
            return Err(io::Error::new(
                io::ErrorKind::InvalidData,
                "directory observation has too many entries",
            ));
        }
        entries.push((
            name,
            directory_entry_kind(metadata.file_type()),
            metadata.len(),
        ));
    }
    entries.sort();
    Ok(entries)
}

fn directory_entry_kind(kind: fs::FileType) -> u8 {
    if kind.is_symlink() {
        return 5;
    }
    if kind.is_dir() {
        return 3;
    }
    if kind.is_file() {
        return 6;
    }
    #[cfg(unix)]
    {
        use std::os::unix::fs::FileTypeExt;
        if kind.is_block_device() {
            return 1;
        }
        if kind.is_char_device() {
            return 2;
        }
        if kind.is_fifo() {
            return 4;
        }
        if kind.is_socket() {
            return 7;
        }
    }
    255
}

pub(crate) fn directory_observation(
    relative: PathBuf,
    entries: &[DirectoryFact],
) -> io::Result<ConfigureObservation> {
    let mut hasher = blake3::Hasher::new();
    hasher.update(&(entries.len() as u32).to_le_bytes());
    for (name, kind, len) in entries {
        hasher.update(&(name.len() as u32).to_le_bytes());
        hasher.update(name);
        hasher.update(&[*kind]);
        hasher.update(&len.to_le_bytes());
    }
    ConfigureObservation::new(
        relative,
        ObservationKind::DirectoryContents,
        *hasher.finalize().as_bytes(),
    )
    .map_err(|error| io::Error::new(io::ErrorKind::InvalidInput, error))
}

/// Records portable metadata for a package-relative filesystem entry.
pub fn observe_metadata(
    root: &Path,
    relative: impl Into<PathBuf>,
) -> io::Result<ConfigureObservation> {
    let relative = relative.into();
    let metadata = fs::metadata(resolve_observation_path(root, &relative)?)?;
    let mut bytes = Vec::with_capacity(24);
    bytes.extend_from_slice(&metadata.len().to_le_bytes());
    bytes.push(u8::from(metadata.is_dir()));
    bytes.push(u8::from(metadata.is_file()));
    bytes.push(u8::from(metadata.is_symlink()));
    let modified = metadata
        .modified()
        .ok()
        .and_then(|time| time.duration_since(std::time::UNIX_EPOCH).ok());
    bytes.extend_from_slice(&modified.map_or(0, |value| value.as_secs()).to_le_bytes());
    bytes.extend_from_slice(
        &modified
            .map_or(0, |value| u64::from(value.subsec_nanos()))
            .to_le_bytes(),
    );
    ConfigureObservation::new(
        relative,
        ObservationKind::FileMetadata,
        *blake3::hash(&bytes).as_bytes(),
    )
    .map_err(|error| io::Error::new(io::ErrorKind::InvalidInput, error))
}

/// Records portable metadata for a package-relative directory.
pub fn observe_directory_metadata(
    root: &Path,
    relative: impl Into<PathBuf>,
) -> io::Result<ConfigureObservation> {
    let relative = relative.into();
    let metadata = fs::metadata(resolve_observation_path(root, &relative)?)?;
    if !metadata.is_dir() {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            "directory metadata observation requires a directory",
        ));
    }
    let mut bytes = Vec::with_capacity(24);
    bytes.extend_from_slice(&metadata.len().to_le_bytes());
    let modified = metadata
        .modified()
        .ok()
        .and_then(|time| time.duration_since(std::time::UNIX_EPOCH).ok());
    bytes.extend_from_slice(&modified.map_or(0, |value| value.as_secs()).to_le_bytes());
    bytes.extend_from_slice(
        &modified
            .map_or(0, |value| u64::from(value.subsec_nanos()))
            .to_le_bytes(),
    );
    ConfigureObservation::new(
        relative,
        ObservationKind::DirectoryMetadata,
        *blake3::hash(&bytes).as_bytes(),
    )
    .map_err(|error| io::Error::new(io::ErrorKind::InvalidInput, error))
}

/// Records whether a package-relative entry exists and, when present, its
/// filesystem kind. Missing entries are a valid observation rather than an
/// I/O failure, so callers can safely configure optional inputs.
pub fn observe_file_presence(
    root: &Path,
    relative: impl Into<PathBuf>,
) -> io::Result<ConfigureObservation> {
    observe_presence(root, relative.into(), ObservationKind::FilePresence)
}

/// Records whether a package-relative directory exists. A present non-directory
/// entry is recorded as a distinct state and therefore invalidates the probe.
pub fn observe_directory_presence(
    root: &Path,
    relative: impl Into<PathBuf>,
) -> io::Result<ConfigureObservation> {
    observe_presence(root, relative.into(), ObservationKind::DirectoryPresence)
}

/// Records a UTF-8 environment value. An absent variable is a valid fact and
/// receives a distinct fingerprint from an empty value.
pub fn observe_environment(name: impl Into<PathBuf>) -> io::Result<ConfigureObservation> {
    let name = name.into();
    ConfigureObservation::new(name.clone(), ObservationKind::EnvironmentValue, [0; 32])
        .map_err(|error| io::Error::new(io::ErrorKind::InvalidInput, error))?;
    let value = std::env::var(name.as_os_str());
    observe_environment_value(name, value)
}

fn observe_environment_value(
    name: PathBuf,
    value: Result<String, std::env::VarError>,
) -> io::Result<ConfigureObservation> {
    let value = match value {
        Ok(value) => {
            let mut bytes = Vec::with_capacity(1 + value.len());
            bytes.push(1);
            bytes.extend_from_slice(value.as_bytes());
            bytes
        }
        Err(std::env::VarError::NotPresent) => vec![0],
        Err(std::env::VarError::NotUnicode(_)) => {
            return Err(io::Error::new(
                io::ErrorKind::InvalidData,
                "configure environment observation is not valid UTF-8",
            ));
        }
    };
    let fingerprint = *blake3::hash(&value).as_bytes();
    ConfigureObservation::new(name, ObservationKind::EnvironmentValue, fingerprint)
        .map_err(|error| io::Error::new(io::ErrorKind::InvalidInput, error))
}

const MAX_PROBE_OUTPUT: usize = 64 * 1024;
const PROBE_TIMEOUT: Duration = Duration::from_secs(5);

fn observe_command(path: &Path, request: &[u8], root: &Path) -> io::Result<ConfigureObservation> {
    let (program, arguments) = decode_command_request(request)?;
    let result = run_probe_process(&program, &arguments, root)?;
    let fingerprint = probe_fingerprint(request, &result);
    ConfigureObservation::new_with_request(
        path.to_path_buf(),
        ObservationKind::CommandResult,
        fingerprint,
        request.to_vec(),
    )
    .map_err(|error| io::Error::new(io::ErrorKind::InvalidInput, error))
}

fn observe_git_state(root: &Path, request: &[u8]) -> io::Result<ConfigureObservation> {
    if !request.is_empty() {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            "Git probe request is not empty",
        ));
    }
    let result = run_probe_process(
        "git",
        &["status".into(), "--porcelain=v1".into(), "--branch".into()],
        root,
    )?;
    let fingerprint = probe_fingerprint(request, &result);
    ConfigureObservation::new_with_request(
        "git-state",
        ObservationKind::GitState,
        fingerprint,
        Vec::new(),
    )
    .map_err(|error| io::Error::new(io::ErrorKind::InvalidInput, error))
}

fn observe_tool(path: &Path, request: &[u8], root: &Path) -> io::Result<ConfigureObservation> {
    let program = std::str::from_utf8(request)
        .map_err(|_| io::Error::new(io::ErrorKind::InvalidData, "tool probe name is not UTF-8"))?;
    if program.is_empty() {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            "tool probe name is empty",
        ));
    }
    let result = run_probe_process(program, &["--version".into()], root)?;
    let fingerprint = probe_fingerprint(request, &result);
    ConfigureObservation::new_with_request(
        path.to_path_buf(),
        ObservationKind::ToolIdentity,
        fingerprint,
        request.to_vec(),
    )
    .map_err(|error| io::Error::new(io::ErrorKind::InvalidInput, error))
}

struct ProbeProcessResult {
    executable: Vec<u8>,
    status: i32,
    stdout: Vec<u8>,
    stderr: Vec<u8>,
}

fn probe_fingerprint(request: &[u8], result: &ProbeProcessResult) -> [u8; 32] {
    let mut hasher = blake3::Hasher::new();
    hasher.update(request);
    hasher.update(&result.executable);
    hasher.update(&result.status.to_le_bytes());
    hasher.update(&(result.stdout.len() as u32).to_le_bytes());
    hasher.update(&result.stdout);
    hasher.update(&(result.stderr.len() as u32).to_le_bytes());
    hasher.update(&result.stderr);
    *hasher.finalize().as_bytes()
}

fn decode_command_request(request: &[u8]) -> io::Result<(String, Vec<String>)> {
    let mut cursor = 0usize;
    let program = read_probe_text(request, &mut cursor)?;
    let count = read_probe_u32(request, &mut cursor)? as usize;
    if count > 256 {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            "too many command arguments",
        ));
    }
    let mut arguments = Vec::with_capacity(count);
    for _ in 0..count {
        arguments.push(read_probe_text(request, &mut cursor)?);
    }
    if cursor != request.len() || program.is_empty() {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            "invalid command probe request",
        ));
    }
    Ok((program, arguments))
}

fn read_probe_u32(bytes: &[u8], cursor: &mut usize) -> io::Result<u32> {
    let end = cursor.checked_add(4).ok_or_else(invalid_probe_request)?;
    let value = u32::from_le_bytes(
        bytes
            .get(*cursor..end)
            .ok_or_else(invalid_probe_request)?
            .try_into()
            .unwrap(),
    );
    *cursor = end;
    Ok(value)
}

fn read_probe_text(bytes: &[u8], cursor: &mut usize) -> io::Result<String> {
    let len = read_probe_u32(bytes, cursor)? as usize;
    let end = cursor.checked_add(len).ok_or_else(invalid_probe_request)?;
    let text = std::str::from_utf8(bytes.get(*cursor..end).ok_or_else(invalid_probe_request)?)
        .map_err(|_| invalid_probe_request())?
        .to_owned();
    *cursor = end;
    Ok(text)
}

fn invalid_probe_request() -> io::Error {
    io::Error::new(io::ErrorKind::InvalidData, "invalid probe request")
}

fn run_probe_process(
    program: &str,
    arguments: &[String],
    root: &Path,
) -> io::Result<ProbeProcessResult> {
    let executable_path = resolve_probe_executable(program, root)?;
    let executable = probe_executable_identity(&executable_path)?;
    let mut command = Command::new(&executable_path);
    command
        .args(arguments)
        .current_dir(root)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::null());
    crate::process_output::prepare_process_group(&mut command);
    let mut child = command.spawn()?;
    let stdout = child
        .stdout
        .take()
        .ok_or_else(|| io::Error::other("probe stdout pipe missing"))?;
    let stdout_thread = std::thread::spawn(|| bounded_probe_reader(stdout));
    let deadline = Instant::now() + PROBE_TIMEOUT;
    let status = loop {
        if let Some(status) = child.try_wait()? {
            break status;
        }
        if Instant::now() >= deadline {
            crate::process_output::terminate_process_tree(&mut child);
            let _ = child.wait();
            return Err(io::Error::new(
                io::ErrorKind::TimedOut,
                "configuration probe timed out",
            ));
        }
        std::thread::sleep(Duration::from_millis(10));
    };
    let stdout = stdout_thread
        .join()
        .map_err(|_| io::Error::other("probe stdout reader failed"))??;
    #[cfg(unix)]
    use std::os::unix::process::ExitStatusExt;
    let status = status.code().unwrap_or({
        #[cfg(unix)]
        {
            -status.signal().unwrap_or(1)
        }
        #[cfg(not(unix))]
        {
            -1
        }
    });
    Ok(ProbeProcessResult {
        executable,
        status,
        stdout,
        stderr: Vec::new(),
    })
}

fn resolve_probe_executable(program: &str, root: &Path) -> io::Result<PathBuf> {
    let search_path = std::env::var_os("PATH").unwrap_or_default();
    resolve_probe_executable_in_path(program, root, &search_path)
}

fn resolve_probe_executable_in_path(
    program: &str,
    root: &Path,
    search_path: &OsStr,
) -> io::Result<PathBuf> {
    let program_path = Path::new(program);
    if program_path.is_absolute() || program_path.components().count() > 1 {
        let candidate = if program_path.is_absolute() {
            program_path.to_path_buf()
        } else {
            root.join(program_path)
        };
        return executable_probe_candidate(&candidate);
    }

    for directory in std::env::split_paths(search_path) {
        let directory = if directory.as_os_str().is_empty() {
            root.to_path_buf()
        } else if directory.is_absolute() {
            directory
        } else {
            root.join(directory)
        };
        #[cfg(windows)]
        let candidates = probe_windows_candidates(&directory, program_path);
        #[cfg(not(windows))]
        let candidates = vec![directory.join(program_path)];
        for candidate in candidates {
            if let Ok(path) = executable_probe_candidate(&candidate) {
                return Ok(path);
            }
        }
    }
    Err(io::Error::new(
        io::ErrorKind::NotFound,
        format!("configuration probe program `{program}` was not found in PATH"),
    ))
}

fn executable_probe_candidate(path: &Path) -> io::Result<PathBuf> {
    let canonical = path.canonicalize()?;
    let metadata = fs::metadata(&canonical)?;
    if !metadata.is_file() {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            "configuration probe program is not a file",
        ));
    }
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt as _;
        if metadata.permissions().mode() & 0o111 == 0 {
            return Err(io::Error::new(
                io::ErrorKind::PermissionDenied,
                "configuration probe program is not executable",
            ));
        }
    }
    Ok(canonical)
}

#[cfg(windows)]
fn probe_windows_candidates(directory: &Path, program: &Path) -> Vec<PathBuf> {
    if program.extension().is_some() {
        return vec![directory.join(program)];
    }
    let extensions = std::env::var_os("PATHEXT").unwrap_or_else(|| ".COM;.EXE;.BAT;.CMD".into());
    let mut candidates = vec![directory.join(program)];
    candidates.extend(
        extensions
            .to_string_lossy()
            .split(';')
            .filter(|extension| !extension.is_empty())
            .map(|extension| {
                directory
                    .join(program)
                    .with_extension(extension.trim_start_matches('.'))
            }),
    );
    candidates
}

fn probe_executable_identity(path: &Path) -> io::Result<Vec<u8>> {
    let mut file = fs::File::open(path)?;
    let metadata = file.metadata()?;
    if !metadata.is_file() {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            "configuration probe executable changed type",
        ));
    }
    let mut hasher = blake3::Hasher::new();
    let mut remaining = metadata.len();
    let mut buffer = [0u8; 64 * 1024];
    while remaining != 0 {
        let count = usize::try_from(remaining.min(buffer.len() as u64)).unwrap();
        file.read_exact(&mut buffer[..count])?;
        hasher.update(&buffer[..count]);
        remaining -= count as u64;
    }
    let mut trailing = [0u8; 1];
    if file.read(&mut trailing)? != 0 {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            "configuration probe executable changed while it was read",
        ));
    }
    let path = path.to_str().ok_or_else(|| {
        io::Error::new(
            io::ErrorKind::InvalidData,
            "configuration probe executable path is not UTF-8",
        )
    })?;
    let mut identity = Vec::with_capacity(path.len() + 40);
    identity.extend_from_slice(&(path.len() as u32).to_le_bytes());
    identity.extend_from_slice(path.as_bytes());
    identity.extend_from_slice(&metadata.len().to_le_bytes());
    identity.extend_from_slice(hasher.finalize().as_bytes());
    Ok(identity)
}

fn bounded_probe_reader(mut reader: impl io::Read) -> io::Result<Vec<u8>> {
    let mut output = Vec::new();
    let mut buffer = [0u8; 4096];
    loop {
        let count = reader.read(&mut buffer)?;
        if count == 0 {
            break;
        }
        if output.len().saturating_add(count) > MAX_PROBE_OUTPUT {
            return Err(io::Error::new(
                io::ErrorKind::InvalidData,
                "configuration probe output is too large",
            ));
        }
        output.extend_from_slice(&buffer[..count]);
    }
    Ok(output)
}

pub(crate) fn validate_probe_result(
    kind: ObservationKind,
    path: &str,
    evidence: &[u8],
    root: &Path,
) -> io::Result<ConfigureObservation> {
    let mut cursor = 0usize;
    let request_len = read_probe_u32(evidence, &mut cursor)? as usize;
    let request_end = cursor
        .checked_add(request_len)
        .ok_or_else(invalid_probe_request)?;
    let request = evidence
        .get(cursor..request_end)
        .ok_or_else(invalid_probe_request)?;
    cursor = request_end;
    let status_bytes = evidence
        .get(cursor..cursor + 4)
        .ok_or_else(invalid_probe_request)?;
    let expected_status = i32::from_le_bytes(status_bytes.try_into().unwrap());
    cursor += 4;
    let stdout_len = read_probe_u32(evidence, &mut cursor)? as usize;
    let stdout_end = cursor
        .checked_add(stdout_len)
        .ok_or_else(invalid_probe_request)?;
    let expected_stdout = evidence
        .get(cursor..stdout_end)
        .ok_or_else(invalid_probe_request)?;
    cursor = stdout_end;
    let stderr_len = read_probe_u32(evidence, &mut cursor)? as usize;
    let stderr_end = cursor
        .checked_add(stderr_len)
        .ok_or_else(invalid_probe_request)?;
    let expected_stderr = evidence
        .get(cursor..stderr_end)
        .ok_or_else(invalid_probe_request)?;
    if cursor != evidence.len() || stdout_len > MAX_PROBE_OUTPUT || stderr_len > MAX_PROBE_OUTPUT {
        return Err(invalid_probe_request());
    }
    let (program, arguments) = match kind {
        ObservationKind::CommandResult => decode_command_request(request)?,
        ObservationKind::GitState => {
            if !request.is_empty() {
                return Err(invalid_probe_request());
            }
            (
                "git".to_owned(),
                vec!["status".into(), "--porcelain=v1".into(), "--branch".into()],
            )
        }
        ObservationKind::ToolIdentity => {
            let program = std::str::from_utf8(request).map_err(|_| invalid_probe_request())?;
            (program.to_owned(), vec!["--version".into()])
        }
        _ => return Err(invalid_probe_request()),
    };
    let actual = run_probe_process(&program, &arguments, root)?;
    if actual.status != expected_status
        || actual.stdout != expected_stdout
        || actual.stderr != expected_stderr
    {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            format!("runner probe changed before validation: {path}"),
        ));
    }
    ConfigureObservation::new_with_request(
        path,
        kind,
        probe_fingerprint(request, &actual),
        request.to_vec(),
    )
    .map_err(|error| io::Error::new(io::ErrorKind::InvalidInput, error))
}

pub(crate) fn observe_presence(
    root: &Path,
    relative: PathBuf,
    kind: ObservationKind,
) -> io::Result<ConfigureObservation> {
    let path = resolve_observation_candidate(root, &relative)?;
    let state = match fs::symlink_metadata(path) {
        Ok(metadata) => {
            if metadata.is_dir() {
                2
            } else if metadata.is_file() {
                1
            } else {
                3
            }
        }
        Err(error) if error.kind() == io::ErrorKind::NotFound => 0,
        Err(error) => return Err(error),
    };
    let bytes = [state, u8::from(kind == ObservationKind::DirectoryPresence)];
    ConfigureObservation::new(relative, kind, *blake3::hash(&bytes).as_bytes())
        .map_err(|error| io::Error::new(io::ErrorKind::InvalidInput, error))
}

pub(crate) fn resolve_observation_path(root: &Path, relative: &Path) -> io::Result<PathBuf> {
    ConfigureObservation::new(
        relative.to_path_buf(),
        ObservationKind::FileMetadata,
        [0; 32],
    )
    .map_err(|error| io::Error::new(io::ErrorKind::InvalidInput, error))?;
    let canonical_root = root.canonicalize()?;
    let candidate = root.join(relative).canonicalize()?;
    if !candidate.starts_with(&canonical_root) {
        return Err(io::Error::new(
            io::ErrorKind::PermissionDenied,
            "configure observation path escapes the package root",
        ));
    }
    Ok(candidate)
}

/// Resolves an observation candidate while allowing missing components.
/// Every existing parent is canonicalized before the candidate is
/// accepted, so a symlinked parent cannot escape the package root.
fn resolve_observation_candidate(root: &Path, relative: &Path) -> io::Result<PathBuf> {
    ConfigureObservation::new(
        relative.to_path_buf(),
        ObservationKind::FileMetadata,
        [0; 32],
    )
    .map_err(|error| io::Error::new(io::ErrorKind::InvalidInput, error))?;
    let canonical_root = root.canonicalize()?;
    let candidate = root.join(relative);
    let mut existing = candidate.clone();
    let mut missing = Vec::new();
    let canonical_existing = loop {
        match existing.canonicalize() {
            Ok(path) => break path,
            Err(error) if error.kind() == io::ErrorKind::NotFound => {
                // A dangling symlink is not an absent input: its target may
                // escape the root even though canonicalization cannot find it.
                match fs::symlink_metadata(&existing) {
                    Ok(metadata) if metadata.file_type().is_symlink() => {
                        return Err(io::Error::new(
                            io::ErrorKind::PermissionDenied,
                            "configure observation cannot resolve a dangling symlink",
                        ));
                    }
                    Err(error) if error.kind() != io::ErrorKind::NotFound => return Err(error),
                    _ => {}
                }
                let name = existing.file_name().ok_or_else(|| {
                    io::Error::new(io::ErrorKind::InvalidInput, "observation path has no name")
                })?;
                missing.push(name.to_os_string());
                existing.pop();
            }
            Err(error) => return Err(error),
        }
    };
    if !canonical_existing.starts_with(&canonical_root) {
        return Err(io::Error::new(
            io::ErrorKind::PermissionDenied,
            "configure observation path escapes the package root",
        ));
    }
    let mut resolved = canonical_existing;
    for name in missing.iter().rev() {
        resolved.push(name);
    }
    Ok(resolved)
}

/// Adds a filesystem observation to a manifest, preserving canonical order.
pub fn observe_into(
    manifest: &mut ConfigureManifest,
    observation: ConfigureObservation,
) -> Result<(), ObservationCodecError> {
    manifest.push(observation)
}

/// A deterministic configuration observation manifest.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ConfigureManifest {
    policy: ObservationPolicy,
    observations: Vec<ConfigureObservation>,
}

impl ConfigureManifest {
    /// Creates an empty manifest using the supplied undeclared-observation policy.
    pub fn new(policy: ObservationPolicy) -> Self {
        Self {
            policy,
            observations: Vec::new(),
        }
    }

    /// Adds an observation and keeps the manifest in canonical order.
    pub fn push(&mut self, observation: ConfigureObservation) -> Result<(), ObservationCodecError> {
        let key = (&observation.path, observation.kind);
        let index = match self
            .observations
            .binary_search_by(|previous| (&previous.path, previous.kind).cmp(&key))
        {
            Ok(_) => return Err(ObservationCodecError::Invalid),
            Err(index) => index,
        };
        self.observations.insert(index, observation);
        Ok(())
    }

    /// Returns the policy applied to undeclared observations.
    pub fn policy(&self) -> ObservationPolicy {
        self.policy
    }

    /// Returns observations in deterministic path/kind order.
    pub fn observations(&self) -> &[ConfigureObservation] {
        &self.observations
    }

    /// Compares every persisted observation with the current package root.
    pub fn validate(&self, root: &Path) -> ObservationValidation {
        for observation in &self.observations {
            match observation.refresh(root) {
                Ok(current) if current.fingerprint() == observation.fingerprint() => {}
                Ok(_) => {
                    return ObservationValidation::Changed {
                        path: observation.path.clone(),
                        kind: observation.kind,
                    };
                }
                Err(_) => {
                    return ObservationValidation::Missing {
                        path: observation.path.clone(),
                        kind: observation.kind,
                    };
                }
            }
        }
        ObservationValidation::Match
    }

    /// Returns a canonical fingerprint for this manifest.
    pub fn fingerprint(&self) -> [u8; 32] {
        let mut hasher = blake3::Hasher::new();
        hasher.update(CONFIGURE_MANIFEST_DOMAIN.as_str().as_bytes());
        hasher.update(b"\0");
        hasher.update(&[match self.policy {
            ObservationPolicy::Disallow => 0,
            ObservationPolicy::Poison => 1,
            ObservationPolicy::Ignore => 2,
        }]);
        hasher.update(&(self.observations.len() as u32).to_le_bytes());
        for observation in &self.observations {
            let path = observation
                .path
                .to_str()
                .expect("validated UTF-8 observation path");
            hasher.update(&(path.len() as u32).to_le_bytes());
            hasher.update(path.as_bytes());
            hasher.update(&[match observation.kind {
                ObservationKind::FileContents => 0,
                ObservationKind::FileMetadata => 1,
                ObservationKind::DirectoryContents => 2,
                ObservationKind::DirectoryMetadata => 3,
                ObservationKind::FilePresence => 4,
                ObservationKind::DirectoryPresence => 5,
                ObservationKind::EnvironmentValue => 6,
                ObservationKind::CommandResult => 7,
                ObservationKind::GitState => 8,
                ObservationKind::ToolIdentity => 9,
            }]);
            hasher.update(&(observation.request.len() as u32).to_le_bytes());
            hasher.update(&observation.request);
            hasher.update(&observation.fingerprint);
        }
        *hasher.finalize().as_bytes()
    }

    /// Encodes a bounded, versioned manifest record for the configure cache.
    pub fn encode(&self) -> Result<Vec<u8>, ObservationCodecError> {
        let total = self
            .observations
            .iter()
            .try_fold(48usize + 5, |total, observation| {
                total
                    .checked_add(4 + 1 + 4 + 32)
                    .and_then(|total| total.checked_add(observation.path.as_os_str().len()))
                    .and_then(|total| total.checked_add(observation.request.len()))
                    .filter(|total| *total <= MAX_MANIFEST_BYTES)
                    .ok_or(ObservationCodecError::TooLarge)
            })?;
        let mut payload = Vec::with_capacity(total - 48);
        payload.push(match self.policy {
            ObservationPolicy::Disallow => 0,
            ObservationPolicy::Poison => 1,
            ObservationPolicy::Ignore => 2,
        });
        let count =
            u32::try_from(self.observations.len()).map_err(|_| ObservationCodecError::TooLarge)?;
        payload.extend_from_slice(&count.to_le_bytes());
        for observation in &self.observations {
            let path = observation
                .path
                .to_str()
                .ok_or(ObservationCodecError::Invalid)?;
            let path_len =
                u32::try_from(path.len()).map_err(|_| ObservationCodecError::TooLarge)?;
            payload.extend_from_slice(&path_len.to_le_bytes());
            payload.extend_from_slice(path.as_bytes());
            payload.push(match observation.kind {
                ObservationKind::FileContents => 0,
                ObservationKind::FileMetadata => 1,
                ObservationKind::DirectoryContents => 2,
                ObservationKind::DirectoryMetadata => 3,
                ObservationKind::FilePresence => 4,
                ObservationKind::DirectoryPresence => 5,
                ObservationKind::EnvironmentValue => 6,
                ObservationKind::CommandResult => 7,
                ObservationKind::GitState => 8,
                ObservationKind::ToolIdentity => 9,
            });
            let request_len = u32::try_from(observation.request.len())
                .map_err(|_| ObservationCodecError::TooLarge)?;
            payload.extend_from_slice(&request_len.to_le_bytes());
            payload.extend_from_slice(&observation.request);
            payload.extend_from_slice(&observation.fingerprint);
        }
        let mut encoded = Vec::with_capacity(8 + 4 + 4 + 32 + payload.len());
        encoded.extend_from_slice(nia_compat::formats::CONFIGURE_MANIFEST.magic);
        encoded.extend_from_slice(
            &nia_compat::formats::CONFIGURE_MANIFEST
                .protocol_epoch
                .to_le_bytes(),
        );
        encoded.extend_from_slice(
            &u32::try_from(payload.len())
                .map_err(|_| ObservationCodecError::TooLarge)?
                .to_le_bytes(),
        );
        encoded.extend_from_slice(blake3::hash(&payload).as_bytes());
        encoded.extend_from_slice(&payload);
        Ok(encoded)
    }

    /// Encodes the manifest as the observation payload carried by a build draft.
    pub fn draft_payload(&self) -> Result<Vec<u8>, ObservationCodecError> {
        self.encode()
    }
}

/// Failure decoding a persisted configure manifest.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ObservationCodecError {
    /// The payload exceeds the bounded cache record size.
    TooLarge,
    /// The payload does not match the registered format.
    Invalid,
}

impl ConfigureManifest {
    /// Decodes and validates a persisted configure manifest record.
    pub fn decode(encoded: &[u8]) -> Result<Self, ObservationCodecError> {
        const HEADER: usize = 8 + 4 + 4 + 32;
        if encoded.len() < HEADER || encoded.len() > MAX_MANIFEST_BYTES {
            return Err(ObservationCodecError::Invalid);
        }
        let format = nia_compat::formats::CONFIGURE_MANIFEST;
        if encoded[..8] != *format.magic || encoded[8..12] != format.protocol_epoch.to_le_bytes() {
            return Err(ObservationCodecError::Invalid);
        }
        let length = u32::from_le_bytes(encoded[12..16].try_into().unwrap()) as usize;
        if length != encoded.len() - HEADER
            || encoded[16..48] != *blake3::hash(&encoded[HEADER..]).as_bytes()
        {
            return Err(ObservationCodecError::Invalid);
        }
        let payload = &encoded[HEADER..];
        let policy = match payload.first().copied() {
            Some(0) => ObservationPolicy::Disallow,
            Some(1) => ObservationPolicy::Poison,
            Some(2) => ObservationPolicy::Ignore,
            _ => return Err(ObservationCodecError::Invalid),
        };
        let mut cursor = 1usize;
        let count = read_u32(payload, &mut cursor)? as usize;
        let mut manifest = ConfigureManifest::new(policy);
        for _ in 0..count {
            let path_len = read_u32(payload, &mut cursor)? as usize;
            let path = std::str::from_utf8(
                payload
                    .get(
                        cursor
                            ..cursor
                                .checked_add(path_len)
                                .ok_or(ObservationCodecError::Invalid)?,
                    )
                    .ok_or(ObservationCodecError::Invalid)?,
            )
            .map_err(|_| ObservationCodecError::Invalid)?;
            cursor += path_len;
            let kind = match payload.get(cursor).copied() {
                Some(0) => ObservationKind::FileContents,
                Some(1) => ObservationKind::FileMetadata,
                Some(2) => ObservationKind::DirectoryContents,
                Some(3) => ObservationKind::DirectoryMetadata,
                Some(4) => ObservationKind::FilePresence,
                Some(5) => ObservationKind::DirectoryPresence,
                Some(6) => ObservationKind::EnvironmentValue,
                Some(7) => ObservationKind::CommandResult,
                Some(8) => ObservationKind::GitState,
                Some(9) => ObservationKind::ToolIdentity,
                _ => return Err(ObservationCodecError::Invalid),
            };
            cursor += 1;
            let request_len = read_u32(payload, &mut cursor)? as usize;
            let request_end = cursor
                .checked_add(request_len)
                .ok_or(ObservationCodecError::Invalid)?;
            let request = payload
                .get(cursor..request_end)
                .ok_or(ObservationCodecError::Invalid)?
                .to_vec();
            cursor = request_end;
            let fingerprint = payload
                .get(cursor..cursor + 32)
                .ok_or(ObservationCodecError::Invalid)?
                .try_into()
                .unwrap();
            cursor += 32;
            let observation = if request.is_empty()
                && !matches!(
                    kind,
                    ObservationKind::CommandResult
                        | ObservationKind::GitState
                        | ObservationKind::ToolIdentity
                ) {
                ConfigureObservation::new(path, kind, fingerprint)
            } else {
                ConfigureObservation::new_with_request(path, kind, fingerprint, request)
            }
            .map_err(|_| ObservationCodecError::Invalid)?;
            if manifest.observations.last().is_some_and(|previous| {
                previous >= &observation
                    || (previous.path == observation.path && previous.kind == observation.kind)
            }) {
                return Err(ObservationCodecError::Invalid);
            }
            manifest.observations.push(observation);
        }
        (cursor == payload.len())
            .then_some(manifest)
            .ok_or(ObservationCodecError::Invalid)
    }
}

fn read_u32(payload: &[u8], cursor: &mut usize) -> Result<u32, ObservationCodecError> {
    let bytes = payload
        .get(
            *cursor
                ..cursor
                    .checked_add(4)
                    .ok_or(ObservationCodecError::Invalid)?,
        )
        .ok_or(ObservationCodecError::Invalid)?;
    *cursor += 4;
    Ok(u32::from_le_bytes(bytes.try_into().unwrap()))
}

/// Rejection reason for a configure observation that cannot cross the cache boundary.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ObservationError {
    /// The observation path was empty.
    EmptyPath,
    /// Persisted logical names must round-trip through UTF-8.
    NonUtf8Path(PathBuf),
    /// Absolute host paths cannot become portable configure identities.
    AbsolutePath(PathBuf),
    /// `.` and `..` components are not canonical identities.
    NonCanonicalPath(PathBuf),
    /// Environment names cannot contain NUL or `=` separators.
    InvalidEnvironmentName(PathBuf),
    /// A request payload is valid only for command-like observations.
    InvalidRequest,
}

impl fmt::Display for ObservationError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::EmptyPath => f.write_str("configure observation path is empty"),
            Self::NonUtf8Path(path) => write!(
                f,
                "configure observation name `{}` is not valid UTF-8",
                path.display()
            ),
            Self::AbsolutePath(path) => write!(
                f,
                "configure observation path `{}` must be relative",
                path.display()
            ),
            Self::NonCanonicalPath(path) => write!(
                f,
                "configure observation path `{}` is not canonical",
                path.display()
            ),
            Self::InvalidEnvironmentName(name) => write!(
                f,
                "configure environment name `{}` contains NUL or `=`",
                name.display()
            ),
            Self::InvalidRequest => f.write_str("configure observation request is invalid"),
        }
    }
}

impl std::error::Error for ObservationError {}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn manifest_is_sorted_and_deduplicated() {
        let mut manifest = ConfigureManifest::new(ObservationPolicy::Poison);
        let fingerprint = [7; 32];
        manifest
            .push(
                ConfigureObservation::new("src/b.nia", ObservationKind::FileContents, fingerprint)
                    .unwrap(),
            )
            .unwrap();
        manifest
            .push(
                ConfigureObservation::new("src/a.nia", ObservationKind::FileContents, fingerprint)
                    .unwrap(),
            )
            .unwrap();
        assert_eq!(
            manifest.push(
                ConfigureObservation::new("src/b.nia", ObservationKind::FileContents, fingerprint)
                    .unwrap(),
            ),
            Err(ObservationCodecError::Invalid)
        );
        assert_eq!(
            manifest
                .observations()
                .iter()
                .map(|observation| observation.path().to_string_lossy().into_owned())
                .collect::<Vec<_>>(),
            ["src/a.nia", "src/b.nia"]
        );
    }

    #[test]
    fn observation_paths_must_be_relative_and_canonical() {
        let fingerprint = [0; 32];
        assert_eq!(
            ConfigureObservation::new("", ObservationKind::FileContents, fingerprint),
            Err(ObservationError::EmptyPath)
        );
        assert!(matches!(
            ConfigureObservation::new("/tmp/input", ObservationKind::FileContents, fingerprint),
            Err(ObservationError::AbsolutePath(_))
        ));
        assert!(matches!(
            ConfigureObservation::new("src/../input", ObservationKind::FileContents, fingerprint),
            Err(ObservationError::NonCanonicalPath(_))
        ));
    }

    #[test]
    fn command_probe_resolves_and_fingerprints_the_path_executable() {
        let root = std::env::temp_dir().join(format!(
            "nia-probe-path-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        let bin = root.join("bin");
        fs::create_dir_all(&bin).unwrap();
        let name = if cfg!(windows) {
            "nia-probe-test.exe"
        } else {
            "nia-probe-test"
        };
        let executable = bin.join(name);
        fs::copy(std::env::current_exe().unwrap(), &executable).unwrap();
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt as _;
            let mut permissions = fs::metadata(&executable).unwrap().permissions();
            permissions.set_mode(0o755);
            fs::set_permissions(&executable, permissions).unwrap();
        }
        let search_path = std::env::join_paths([&bin]).unwrap();
        let resolved =
            resolve_probe_executable_in_path("nia-probe-test", &root, &search_path).unwrap();
        assert_eq!(resolved, executable.canonicalize().unwrap());
        let first_identity = probe_executable_identity(&resolved).unwrap();
        let first_fingerprint = probe_fingerprint(
            b"probe-request",
            &ProbeProcessResult {
                executable: first_identity,
                status: 0,
                stdout: b"same version".to_vec(),
                stderr: Vec::new(),
            },
        );
        fs::write(&executable, b"changed executable identity").unwrap();
        let second_identity = probe_executable_identity(&resolved).unwrap();
        let second_fingerprint = probe_fingerprint(
            b"probe-request",
            &ProbeProcessResult {
                executable: second_identity,
                status: 0,
                stdout: b"same version".to_vec(),
                stderr: Vec::new(),
            },
        );
        assert_ne!(first_fingerprint, second_fingerprint);
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn equivalent_manifests_have_stable_fingerprints() {
        let fingerprint = [3; 32];
        let mut first = ConfigureManifest::new(ObservationPolicy::Poison);
        let mut second = ConfigureManifest::new(ObservationPolicy::Poison);
        let observation =
            ConfigureObservation::new("src/main.nia", ObservationKind::FileContents, fingerprint)
                .unwrap();
        first.push(observation.clone()).unwrap();
        second.push(observation).unwrap();
        assert_eq!(first.fingerprint(), second.fingerprint());
        second
            .push(
                ConfigureObservation::new("options", ObservationKind::FileMetadata, fingerprint)
                    .unwrap(),
            )
            .unwrap();
        assert_ne!(first.fingerprint(), second.fingerprint());
    }

    #[test]
    fn manifest_record_round_trips_and_rejects_damage() {
        let mut manifest = ConfigureManifest::new(ObservationPolicy::Disallow);
        manifest
            .push(
                ConfigureObservation::new("src/main.nia", ObservationKind::FileContents, [9; 32])
                    .unwrap(),
            )
            .unwrap();
        let encoded = manifest.encode().unwrap();
        assert_eq!(ConfigureManifest::decode(&encoded).unwrap(), manifest);
        let mut damaged = encoded.clone();
        *damaged.last_mut().unwrap() ^= 1;
        assert_eq!(
            ConfigureManifest::decode(&damaged),
            Err(ObservationCodecError::Invalid)
        );

        let record = &encoded[53..];
        let mut payload = encoded[48..53].to_vec();
        payload[..4].copy_from_slice(&2u32.to_le_bytes());
        payload.extend_from_slice(record);
        payload.extend_from_slice(record);
        let mut duplicate = encoded[..16].to_vec();
        duplicate[12..16].copy_from_slice(&(payload.len() as u32).to_le_bytes());
        duplicate.extend_from_slice(blake3::hash(&payload).as_bytes());
        duplicate.extend_from_slice(&payload);
        assert_eq!(
            ConfigureManifest::decode(&duplicate),
            Err(ObservationCodecError::Invalid)
        );
    }

    #[test]
    fn filesystem_observations_are_stable() {
        let root = std::env::temp_dir().join(format!(
            "nia-observation-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        std::fs::create_dir_all(root.join("src")).unwrap();
        std::fs::write(root.join("src/input"), b"nia").unwrap();
        let first = observe_file_contents(&root, "src/input").unwrap();
        let second = observe_file_contents(&root, "src/input").unwrap();
        assert_eq!(first, second);
        let mut manifest = ConfigureManifest::new(ObservationPolicy::Disallow);
        observe_into(&mut manifest, first).unwrap();
        assert!(!manifest.draft_payload().unwrap().is_empty());
        std::fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn persisted_observations_validate_current_filesystem_facts() {
        let root = std::env::temp_dir().join(format!(
            "nia-observation-validation-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        std::fs::create_dir_all(root.join("src")).unwrap();
        std::fs::write(root.join("src/input"), b"before").unwrap();
        let mut manifest = ConfigureManifest::new(ObservationPolicy::Disallow);
        manifest
            .push(observe_file_contents(&root, "src/input").unwrap())
            .unwrap();
        manifest
            .push(observe_directory_metadata(&root, "src").unwrap())
            .unwrap();
        assert_eq!(manifest.validate(&root), ObservationValidation::Match);
        std::fs::write(root.join("src/input"), b"after").unwrap();
        assert!(matches!(
            manifest.validate(&root),
            ObservationValidation::Changed { ref path, kind: ObservationKind::FileContents }
                if path == std::path::Path::new("src/input")
        ));
        std::fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn directory_metadata_has_a_distinct_observation_kind() {
        let root =
            std::env::temp_dir().join(format!("nia-observation-directory-{}", std::process::id()));
        std::fs::create_dir_all(root.join("src")).unwrap();
        let observation = observe_directory_metadata(&root, "src").unwrap();
        assert_eq!(observation.kind(), ObservationKind::DirectoryMetadata);
        std::fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn presence_observations_record_absence_and_invalidate_when_created() {
        let root = std::env::temp_dir().join(format!(
            "nia-observation-presence-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        std::fs::create_dir_all(root.join("inputs")).unwrap();
        let absent = observe_file_presence(&root, "inputs/options").unwrap();
        assert_eq!(absent.kind(), ObservationKind::FilePresence);
        let mut manifest = ConfigureManifest::new(ObservationPolicy::Disallow);
        manifest.push(absent).unwrap();
        assert_eq!(manifest.validate(&root), ObservationValidation::Match);
        std::fs::write(root.join("inputs/options"), b"enabled").unwrap();
        assert!(matches!(
            manifest.validate(&root),
            ObservationValidation::Changed { ref path, kind: ObservationKind::FilePresence }
                if path == std::path::Path::new("inputs/options")
        ));
        std::fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn directory_presence_distinguishes_file_from_directory() {
        let root = std::env::temp_dir().join(format!(
            "nia-observation-directory-presence-{}",
            std::process::id()
        ));
        std::fs::create_dir_all(&root).unwrap();
        let mut manifest = ConfigureManifest::new(ObservationPolicy::Disallow);
        manifest
            .push(observe_directory_presence(&root, "optional").unwrap())
            .unwrap();
        assert_eq!(manifest.validate(&root), ObservationValidation::Match);
        std::fs::write(root.join("optional"), b"file").unwrap();
        assert!(matches!(
            manifest.validate(&root),
            ObservationValidation::Changed {
                kind: ObservationKind::DirectoryPresence,
                ..
            }
        ));
        std::fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn environment_observations_distinguish_absent_empty_and_changed_values() {
        let name = format!("NIA_CONFIGURE_ENV_{}", std::process::id());
        let absent =
            observe_environment_value(PathBuf::from(&name), Err(std::env::VarError::NotPresent))
                .unwrap();
        let empty = observe_environment_value(PathBuf::from(&name), Ok(String::new())).unwrap();
        let value =
            observe_environment_value(PathBuf::from(&name), Ok(String::from("enabled"))).unwrap();
        assert_ne!(absent.fingerprint(), empty.fingerprint());
        assert_ne!(empty.fingerprint(), value.fingerprint());
        assert_eq!(absent.kind(), ObservationKind::EnvironmentValue);
    }

    #[cfg(unix)]
    #[test]
    fn observations_reject_symlink_paths_outside_the_package_root() {
        use std::os::unix::fs::symlink;
        let root =
            std::env::temp_dir().join(format!("nia-observation-symlink-{}", std::process::id()));
        let outside =
            std::env::temp_dir().join(format!("nia-observation-outside-{}", std::process::id()));
        std::fs::create_dir_all(&root).unwrap();
        std::fs::write(&outside, b"outside").unwrap();
        symlink(&outside, root.join("input")).unwrap();
        let error = observe_file_contents(&root, "input").unwrap_err();
        assert_eq!(error.kind(), io::ErrorKind::PermissionDenied);
        std::fs::remove_file(root.join("input")).unwrap();
        std::fs::remove_file(outside).unwrap();
        std::fs::remove_dir_all(root).unwrap();
    }
}
