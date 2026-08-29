//! Repository audits, evidence reports, and repeatable maintenance baselines.
pub mod audit;
pub mod baseline;
pub mod report;
pub mod system;

use std::fs;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{SystemTime, UNIX_EPOCH};

/// Result type used by maintenance commands and reports.
pub type MaintainResult<T> = Result<T, String>;

static NEXT_TEMPORARY_ID: AtomicU64 = AtomicU64::new(0);

/// Unique temporary directory removed on drop unless explicitly persisted.
pub struct TemporaryDirectory {
    path: Option<PathBuf>,
}

impl TemporaryDirectory {
    /// Creates a uniquely named directory below the operating-system temp root.
    pub fn new(prefix: &str) -> MaintainResult<Self> {
        let root = std::env::temp_dir();
        for _ in 0..100 {
            let sequence = NEXT_TEMPORARY_ID.fetch_add(1, Ordering::Relaxed);
            let nanos = SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .map_err(|error| format!("system clock precedes Unix epoch: {error}"))?
                .as_nanos();
            let path = root.join(format!("{prefix}{}-{nanos}-{sequence}", std::process::id()));
            match fs::create_dir(&path) {
                Ok(()) => return Ok(Self { path: Some(path) }),
                Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => continue,
                Err(error) => {
                    return Err(format!(
                        "failed to create temporary directory {}: {error}",
                        path.display()
                    ));
                }
            }
        }
        Err("failed to allocate a unique temporary directory".to_owned())
    }

    /// Returns the directory path while this owner remains temporary.
    pub fn path(&self) -> &Path {
        self.path
            .as_deref()
            .expect("temporary directory was persisted")
    }

    /// Transfers ownership of the directory path and disables cleanup on drop.
    pub fn persist(mut self) -> PathBuf {
        self.path.take().expect("temporary directory was persisted")
    }
}

impl Drop for TemporaryDirectory {
    fn drop(&mut self) {
        if let Some(path) = &self.path {
            let _ = fs::remove_dir_all(path);
        }
    }
}

/// Returns the canonical repository root containing this maintenance crate.
pub fn repository_root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("..")
        .canonicalize()
        .expect("nia-maintain must live under the repository root")
}

/// Resolves a relative path against the current process directory.
pub fn absolute_path(path: &Path) -> MaintainResult<PathBuf> {
    if path.is_absolute() {
        Ok(path.to_path_buf())
    } else {
        std::env::current_dir()
            .map(|current| current.join(path))
            .map_err(|error| format!("failed to resolve {}: {error}", path.display()))
    }
}

/// Parses a non-negative integer option value with contextual diagnostics.
pub fn parse_usize(value: &str, option: &str) -> MaintainResult<usize> {
    value
        .parse()
        .map_err(|_| format!("{option} requires a non-negative integer, found {value:?}"))
}

#[cfg(test)]
pub(crate) mod test_support {
    use std::fs;
    use std::path::{Path, PathBuf};
    use std::sync::atomic::{AtomicU64, Ordering};

    static NEXT_ID: AtomicU64 = AtomicU64::new(0);

    pub struct TestDirectory {
        path: PathBuf,
    }

    impl TestDirectory {
        pub fn new(name: &str) -> Self {
            let id = NEXT_ID.fetch_add(1, Ordering::Relaxed);
            let path = std::env::temp_dir()
                .join(format!("nia-maintain-{name}-{}-{id}", std::process::id()));
            fs::create_dir_all(&path).expect("create test directory");
            Self { path }
        }

        pub fn path(&self) -> &Path {
            &self.path
        }

        pub fn write(&self, relative: &str, contents: &str) {
            let path = self.path.join(relative);
            fs::create_dir_all(path.parent().expect("test file parent"))
                .expect("create test file parent");
            fs::write(path, contents).expect("write test file");
        }
    }

    impl Drop for TestDirectory {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.path);
        }
    }
}
