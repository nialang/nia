// SPDX-License-Identifier: GPL-3.0-or-later

use super::{BuildPlan, PlanCodecError, codec::MAX_PLAN_BYTES};
use std::{
    fmt, fs,
    io::{self, Read, Write},
    path::{Path, PathBuf},
    sync::atomic::{AtomicU64, Ordering},
};

static NEXT_TEMP_ID: AtomicU64 = AtomicU64::new(1);

#[derive(Debug)]
pub enum PlanHandoffError {
    Encode(PlanCodecError),
    MissingParent {
        path: PathBuf,
    },
    CreateTemporary {
        path: PathBuf,
        error: io::Error,
    },
    WriteTemporary {
        path: PathBuf,
        error: io::Error,
    },
    SyncTemporary {
        path: PathBuf,
        error: io::Error,
    },
    Publish {
        from: PathBuf,
        to: PathBuf,
        error: io::Error,
    },
    SyncDirectory {
        path: PathBuf,
        error: io::Error,
    },
    Open {
        path: PathBuf,
        error: io::Error,
    },
    Read {
        path: PathBuf,
        error: io::Error,
    },
    Decode {
        path: PathBuf,
        error: PlanCodecError,
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

pub fn publish_build_plan(path: &Path, plan: &BuildPlan) -> Result<(), PlanHandoffError> {
    let encoded = plan.encode().map_err(PlanHandoffError::Encode)?;
    let parent = path
        .parent()
        .filter(|parent| !parent.as_os_str().is_empty())
        .ok_or_else(|| PlanHandoffError::MissingParent {
            path: path.to_path_buf(),
        })?;
    let (temporary_path, mut temporary) = create_temporary(parent, path)?;
    let mut cleanup = TemporaryCleanup::new(temporary_path.clone());
    temporary
        .write_all(&encoded)
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
    fs::File::open(parent)
        .and_then(|directory| directory.sync_all())
        .map_err(|error| PlanHandoffError::SyncDirectory {
            path: parent.to_path_buf(),
            error,
        })
}

pub fn read_build_plan(path: &Path) -> Result<BuildPlan, PlanHandoffError> {
    let file = fs::File::open(path).map_err(|error| PlanHandoffError::Open {
        path: path.to_path_buf(),
        error,
    })?;
    let mut bytes = Vec::new();
    file.take((MAX_PLAN_BYTES + 1) as u64)
        .read_to_end(&mut bytes)
        .map_err(|error| PlanHandoffError::Read {
            path: path.to_path_buf(),
            error,
        })?;
    BuildPlan::decode(&bytes).map_err(|error| PlanHandoffError::Decode {
        path: path.to_path_buf(),
        error,
    })
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
    use super::super::tests::draft;
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
        let first = BuildPlan::freeze(draft(false)).unwrap();
        publish_build_plan(&path, &first).unwrap();
        assert_eq!(read_build_plan(&path).unwrap(), first);

        let mut changed = draft(false);
        changed.selected_step = changed.default_step.clone();
        let second = BuildPlan::freeze(changed).unwrap();
        publish_build_plan(&path, &second).unwrap();
        assert_eq!(read_build_plan(&path).unwrap(), second);
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
        let plan = BuildPlan::freeze(draft(false)).unwrap();
        assert!(matches!(
            publish_build_plan(&destination, &plan),
            Err(PlanHandoffError::Publish { .. })
        ));
        assert_eq!(fs::read_dir(&dir).unwrap().count(), 1);
        fs::remove_dir_all(dir).unwrap();
    }
}
