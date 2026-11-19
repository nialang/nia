// SPDX-License-Identifier: GPL-3.0-or-later
//! Durable handoff for a validated canonical build plan.
//!
//! Publication writes and syncs a same-directory temporary file, renames it
//! atomically, and syncs the parent. The already decoded plan remains execution
//! truth; `build-plan.bin` is the last completed durable observation.

use super::{
    BuildDraft, BuildDraftError, BuildPlan, MAX_DRAFT_BYTES, PlanCodecError, runner_plan_checksum,
};
use nia_compat::formats::CONFIGURED_PLAN;
use nia_target::CpuLevel;
use nia_target_config::BuildProfile;
use std::{
    fmt, fs,
    io::{self, Read, Write},
    path::{Path, PathBuf},
    sync::atomic::{AtomicU64, Ordering},
};

static NEXT_TEMP_ID: AtomicU64 = AtomicU64::new(1);

/// Failure while durably publishing or reading a canonical build plan.
#[derive(Debug)]
pub enum PlanHandoffError {
    /// Canonical plan encoding failed.
    Encode(BuildDraftError),
    /// The destination path has no parent directory.
    MissingParent {
        /// Path whose parent was required.
        path: PathBuf,
    },
    /// Temporary handoff file creation failed.
    CreateTemporary {
        /// Temporary path that could not be created.
        path: PathBuf,
        /// Underlying filesystem error.
        error: io::Error,
    },
    /// Writing the temporary handoff file failed.
    WriteTemporary {
        /// Temporary path that could not be written.
        path: PathBuf,
        /// Underlying filesystem error.
        error: io::Error,
    },
    /// Syncing the temporary handoff file failed.
    SyncTemporary {
        /// Temporary path that could not be synced.
        path: PathBuf,
        /// Underlying filesystem error.
        error: io::Error,
    },
    /// Atomic publication of the temporary file failed.
    Publish {
        /// Temporary source path.
        from: PathBuf,
        /// Durable destination path.
        to: PathBuf,
        /// Underlying filesystem error.
        error: io::Error,
    },
    /// Syncing the parent directory after publication failed.
    SyncDirectory {
        /// Parent directory that could not be synced.
        path: PathBuf,
        /// Underlying filesystem error.
        error: io::Error,
    },
    /// Opening a persisted plan failed.
    Open {
        /// Plan path that could not be opened.
        path: PathBuf,
        /// Underlying filesystem error.
        error: io::Error,
    },
    /// Reading a persisted plan failed.
    Read {
        /// Plan path that could not be read.
        path: PathBuf,
        /// Underlying filesystem error.
        error: io::Error,
    },
    /// Decoding a persisted plan failed.
    Decode {
        /// Plan path containing invalid bytes.
        path: PathBuf,
        /// Codec error describing the invalid payload.
        error: BuildDraftError,
    },
}

impl fmt::Display for PlanHandoffError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Encode(error) => write!(f, "failed to encode build plan: {error}"),
            Self::MissingParent { path } => {
                write!(f, "build-plan path `{}` has no parent", path.display())
            }
            Self::CreateTemporary { path, error } => write!(
                f,
                "failed to create temporary build plan `{}`: {error}",
                path.display()
            ),
            Self::WriteTemporary { path, error } => write!(
                f,
                "failed to write temporary build plan `{}`: {error}",
                path.display()
            ),
            Self::SyncTemporary { path, error } => write!(
                f,
                "failed to sync temporary build plan `{}`: {error}",
                path.display()
            ),
            Self::Publish { from, to, error } => write!(
                f,
                "failed to publish build plan `{}` to `{}`: {error}",
                from.display(),
                to.display()
            ),
            Self::SyncDirectory { path, error } => write!(
                f,
                "failed to sync build-plan directory `{}`: {error}",
                path.display()
            ),
            Self::Open { path, error } => {
                write!(f, "failed to open build plan `{}`: {error}", path.display())
            }
            Self::Read { path, error } => {
                write!(f, "failed to read build plan `{}`: {error}", path.display())
            }
            Self::Decode { path, error } => write!(
                f,
                "failed to decode build plan `{}`: {error}",
                path.display()
            ),
        }
    }
}

impl std::error::Error for PlanHandoffError {}

/// Invocation defaults applied to the compiler actions in a published plan.
#[derive(Debug, Default, Clone, Copy, PartialEq, Eq)]
pub struct PlanConfiguration {
    /// Profile used for conditional source selection.
    pub profile: BuildProfile,
    /// Default CPU level for actions compiling for the plan's artifact target.
    pub cpu: CpuLevel,
}

impl PlanConfiguration {
    /// An explicit action override wins. Only the default artifact target inherits the CPU.
    pub fn action_cpu(
        self,
        plan: &BuildPlan,
        target: &nia_target::TargetConfig,
        action: &crate::ArtifactConfiguration,
    ) -> CpuLevel {
        action.cpu.unwrap_or_else(|| {
            if target == plan.artifact_target() {
                self.cpu
            } else {
                CpuLevel::Baseline
            }
        })
    }
}

const CONFIGURED_HEADER_BYTES: usize = 22;

/// Atomically publishes the plan and its configuration observations together.
pub fn publish_build_plan(
    path: &Path,
    draft: &BuildDraft,
    configuration: PlanConfiguration,
) -> Result<(), PlanHandoffError> {
    let encoded = draft.encode().map_err(PlanHandoffError::Encode)?;
    let profile = match configuration.profile {
        BuildProfile::Debug => 0,
        BuildProfile::Release => 1,
    };
    let cpu = match configuration.cpu {
        CpuLevel::Baseline => 0,
        CpuLevel::X86_64V2 => 1,
        CpuLevel::X86_64V3 => 2,
        CpuLevel::X86_64V4 => 3,
        CpuLevel::Native => 4,
    };
    let mut payload = Vec::with_capacity(CONFIGURED_HEADER_BYTES + encoded.len());
    payload.extend_from_slice(CONFIGURED_PLAN.magic);
    payload.extend_from_slice(&CONFIGURED_PLAN.protocol_epoch.to_le_bytes());
    payload.extend_from_slice(&[profile, cpu]);
    payload.extend_from_slice(&encoded);
    let checksum = runner_plan_checksum(&payload);
    payload.extend_from_slice(&checksum.to_le_bytes());
    let parent = path
        .parent()
        .filter(|parent| !parent.as_os_str().is_empty())
        .ok_or_else(|| PlanHandoffError::MissingParent {
            path: path.to_path_buf(),
        })?;
    let (temporary_path, mut temporary) = create_temporary(parent, path)?;
    let mut cleanup = TemporaryCleanup::new(temporary_path.clone());
    temporary
        .write_all(&payload)
        .map_err(|error| PlanHandoffError::WriteTemporary {
            path: temporary_path.clone(),
            error,
        })?;
    temporary
        .sync_all()
        .map_err(|error| PlanHandoffError::SyncTemporary {
            path: temporary_path.clone(),
            error,
        })?;
    drop(temporary);
    fs::rename(&temporary_path, path).map_err(|error| PlanHandoffError::Publish {
        from: temporary_path.clone(),
        to: path.to_path_buf(),
        error,
    })?;
    cleanup.published = true;
    sync_parent_directory(parent).map_err(|error| PlanHandoffError::SyncDirectory {
        path: parent.to_path_buf(),
        error,
    })
}

#[cfg(not(windows))]
fn sync_parent_directory(parent: &Path) -> io::Result<()> {
    fs::File::open(parent).and_then(|directory| directory.sync_all())
}

#[cfg(windows)]
fn sync_parent_directory(_parent: &Path) -> io::Result<()> {
    // Windows does not expose a portable directory fsync operation. The
    // temporary file is flushed before rename; the rename itself is the
    // durable publication boundary supported by the Win32 filesystem API.
    Ok(())
}

/// Reads the plan from a bounded, validated configuration handoff.
pub fn read_build_plan(path: &Path) -> Result<BuildPlan, PlanHandoffError> {
    Ok(read_configured_plan(path)?.0.plan)
}

/// Reads the durable plan, observations, and compiler configuration.
pub fn read_configured_plan(
    path: &Path,
) -> Result<(BuildDraft, PlanConfiguration), PlanHandoffError> {
    let bytes = read_handoff(path, MAX_DRAFT_BYTES + CONFIGURED_HEADER_BYTES)?;
    decode_configured_plan(&bytes).map_err(|error| PlanHandoffError::Decode {
        path: path.to_path_buf(),
        error,
    })
}

fn decode_configured_plan(
    bytes: &[u8],
) -> Result<(BuildDraft, PlanConfiguration), BuildDraftError> {
    let invalid = BuildDraftError::InvalidEnvelope;
    if bytes.len() < CONFIGURED_HEADER_BYTES
        || bytes[..8] != *CONFIGURED_PLAN.magic
        || bytes[8..12] != CONFIGURED_PLAN.protocol_epoch.to_le_bytes()
    {
        return Err(invalid);
    }
    let checksum_offset = bytes.len() - 8;
    if bytes[checksum_offset..] != runner_plan_checksum(&bytes[..checksum_offset]).to_le_bytes() {
        return Err(invalid);
    }
    let profile = match bytes[12] {
        0 => BuildProfile::Debug,
        1 => BuildProfile::Release,
        _ => return Err(invalid),
    };
    let cpu = match bytes[13] {
        0 => CpuLevel::Baseline,
        1 => CpuLevel::X86_64V2,
        2 => CpuLevel::X86_64V3,
        3 => CpuLevel::X86_64V4,
        4 => CpuLevel::Native,
        _ => return Err(invalid),
    };
    let draft = BuildDraft::decode(&bytes[14..checksum_offset])?;
    Ok((draft, PlanConfiguration { profile, cpu }))
}

/// Reads the transient draft produced by the build runner.
pub fn read_build_plan_draft(path: &Path) -> Result<BuildDraft, PlanHandoffError> {
    let bytes = read_handoff(path, MAX_DRAFT_BYTES)?;
    BuildDraft::decode(&bytes).map_err(|error| PlanHandoffError::Decode {
        path: path.to_path_buf(),
        error,
    })
}

fn read_handoff(path: &Path, limit: usize) -> Result<Vec<u8>, PlanHandoffError> {
    let file = fs::File::open(path).map_err(|error| PlanHandoffError::Open {
        path: path.to_path_buf(),
        error,
    })?;
    let mut bytes = Vec::new();
    file.take((limit + 1) as u64)
        .read_to_end(&mut bytes)
        .map_err(|error| PlanHandoffError::Read {
            path: path.to_path_buf(),
            error,
        })?;
    Ok(bytes)
}

/// Reads a canonical plan payload without changing the durable handoff path.
/// The returned plan is always decoded through semantic freeze.
pub fn decode_build_plan(bytes: &[u8]) -> Result<BuildPlan, PlanCodecError> {
    BuildPlan::decode(bytes)
}

fn create_temporary(
    parent: &Path,
    destination: &Path,
) -> Result<(PathBuf, fs::File), PlanHandoffError> {
    let file_name = destination
        .file_name()
        .ok_or_else(|| PlanHandoffError::MissingParent {
            path: destination.to_path_buf(),
        })?;
    for _ in 0..128 {
        let id = NEXT_TEMP_ID.fetch_add(1, Ordering::Relaxed);
        let name = format!(
            ".{}.tmp-{}-{id}",
            file_name.to_string_lossy(),
            std::process::id()
        );
        let path = parent.join(name);
        match fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&path)
        {
            Ok(file) => return Ok((path, file)),
            Err(error) if error.kind() == io::ErrorKind::AlreadyExists => continue,
            Err(error) => return Err(PlanHandoffError::CreateTemporary { path, error }),
        }
    }
    let path = parent.join(format!(".{}.tmp", file_name.to_string_lossy()));
    Err(PlanHandoffError::CreateTemporary {
        path,
        error: io::Error::new(
            io::ErrorKind::AlreadyExists,
            "temporary build-plan namespace exhausted",
        ),
    })
}

struct TemporaryCleanup {
    path: PathBuf,
    published: bool,
}

impl TemporaryCleanup {
    fn new(path: PathBuf) -> Self {
        Self {
            path,
            published: false,
        }
    }
}

impl Drop for TemporaryCleanup {
    fn drop(&mut self) {
        if !self.published {
            let _ = fs::remove_file(&self.path);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::super::test_support::draft;
    use super::*;
    use std::sync::atomic::{AtomicU64, Ordering};

    static NEXT_TEST: AtomicU64 = AtomicU64::new(1);

    fn test_dir(name: &str) -> PathBuf {
        let id = NEXT_TEST.fetch_add(1, Ordering::Relaxed);
        let path =
            std::env::temp_dir().join(format!("nia-plan-{name}-{}-{id}", std::process::id()));
        fs::create_dir_all(&path).unwrap();
        path
    }

    #[test]
    fn atomic_handoff_round_trips_and_replaces_previous_plan() {
        let dir = test_dir("replace");
        let path = dir.join("build.plan");
        let first = BuildDraft {
            plan: BuildPlan::freeze(draft(false)).unwrap(),
            observations: b"first observation payload".to_vec(),
        };
        let debug = PlanConfiguration::default();
        publish_build_plan(&path, &first, debug).unwrap();
        assert_eq!(read_configured_plan(&path).unwrap(), (first, debug));

        let mut changed = draft(false);
        changed.selected_step = changed.default_step.clone();
        let second = BuildDraft {
            plan: BuildPlan::freeze(changed).unwrap(),
            observations: b"second observation payload".to_vec(),
        };
        let release = PlanConfiguration {
            profile: BuildProfile::Release,
            cpu: CpuLevel::Native,
        };
        publish_build_plan(&path, &second, release).unwrap();
        assert_eq!(read_configured_plan(&path).unwrap(), (second, release));
        assert_eq!(fs::read_dir(&dir).unwrap().count(), 1);
        fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn reader_rejects_corrupt_published_content() {
        let dir = test_dir("corrupt");
        let path = dir.join("build.plan");
        fs::write(&path, b"not a plan").unwrap();
        assert!(matches!(
            read_build_plan(&path),
            Err(PlanHandoffError::Decode { .. })
        ));
        fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn failed_publication_removes_temporary_file() {
        let dir = test_dir("cleanup");
        let destination = dir.join("build.plan");
        fs::create_dir(&destination).unwrap();
        let draft = BuildDraft {
            plan: BuildPlan::freeze(draft(false)).unwrap(),
            observations: Vec::new(),
        };
        assert!(matches!(
            publish_build_plan(&destination, &draft, PlanConfiguration::default()),
            Err(PlanHandoffError::Publish { .. })
        ));
        assert_eq!(fs::read_dir(&dir).unwrap().count(), 1);
        fs::remove_dir_all(dir).unwrap();
    }
}
