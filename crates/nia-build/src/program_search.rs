//! Program lookup shared by external command steps and configuration probes.
//!
//! Every program a build launches by name is resolved here, so a step, its
//! cache identity, and a probe naming the same program agree on the file:
//!
//! - a name containing a path separator names that file, relative to the
//!   caller's base directory;
//! - a bare name is searched in the caller's `PATH`, where an empty or relative
//!   entry is resolved against the base directory;
//! - on Windows a bare name without an extension also tries each `PATHEXT`
//!   extension, in order, after the name itself.
//!
//! Callers supply `PATH` and `PATHEXT` explicitly so a command's declared
//! environment governs its own lookup.

use std::ffi::OsStr;
use std::path::{Path, PathBuf};

/// Extensions tried on Windows when `PATHEXT` is unset.
#[cfg(windows)]
const DEFAULT_PATHEXT: &str = ".COM;.EXE;.BAT;.CMD";

/// The lookup inputs a caller controls.
pub(crate) struct ProgramSearch<'a> {
    /// Directory that relative names and relative `PATH` entries resolve
    /// against.
    pub(crate) base: &'a Path,
    /// The `PATH` value to search, or `None` when it is unset.
    pub(crate) path: Option<&'a OsStr>,
    /// The `PATHEXT` value, or `None` for the platform default. Ignored
    /// outside Windows.
    pub(crate) pathext: Option<&'a OsStr>,
}

/// Where a program name points before candidates are checked.
pub(crate) enum ProgramLocation {
    /// The name contains a separator and names exactly this file.
    Explicit(PathBuf),
    /// The name is bare and was searched in `PATH`.
    Searched,
}

/// Classifies `name` and returns the location it names, without touching the
/// filesystem.
pub(crate) fn program_location(name: &str, base: &Path) -> ProgramLocation {
    let path = Path::new(name);
    if path.is_absolute() {
        ProgramLocation::Explicit(path.to_path_buf())
    } else if path.components().count() > 1 {
        ProgramLocation::Explicit(base.join(path))
    } else {
        ProgramLocation::Searched
    }
}

/// Returns the candidate files for `name`, in lookup order. `accept` decides
/// whether a candidate is a usable program; the first accepted path wins.
pub(crate) fn find_program<T>(
    name: &str,
    search: &ProgramSearch<'_>,
    mut accept: impl FnMut(&Path) -> Option<T>,
) -> Option<T> {
    match program_location(name, search.base) {
        ProgramLocation::Explicit(path) => accept(&path),
        ProgramLocation::Searched => {
            let path = search.path?;
            for directory in std::env::split_paths(path) {
                let directory = if directory.as_os_str().is_empty() {
                    search.base.to_path_buf()
                } else if directory.is_absolute() {
                    directory
                } else {
                    search.base.join(directory)
                };
                for candidate in directory_candidates(&directory, name, search.pathext) {
                    if let Some(found) = accept(&candidate) {
                        return Some(found);
                    }
                }
            }
            None
        }
    }
}

#[cfg(windows)]
fn directory_candidates(directory: &Path, name: &str, pathext: Option<&OsStr>) -> Vec<PathBuf> {
    let base = directory.join(name);
    if Path::new(name).extension().is_some() {
        return vec![base];
    }
    let extensions = pathext.map_or_else(
        || DEFAULT_PATHEXT.to_owned(),
        |value| value.to_string_lossy().into_owned(),
    );
    let mut candidates = vec![base];
    for extension in extensions
        .split(';')
        .filter(|extension| !extension.is_empty())
    {
        // Append rather than replace: `tool.v2` plus `.EXE` is `tool.v2.EXE`.
        let mut candidate = std::ffi::OsString::from(directory.join(name));
        candidate.push(extension);
        candidates.push(PathBuf::from(candidate));
    }
    candidates
}

#[cfg(not(windows))]
fn directory_candidates(directory: &Path, name: &str, _pathext: Option<&OsStr>) -> Vec<PathBuf> {
    vec![directory.join(name)]
}

/// Reports whether metadata describes a file the host can execute.
pub(crate) fn is_executable_file(metadata: &std::fs::Metadata) -> bool {
    if !metadata.is_file() {
        return false;
    }
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt as _;
        metadata.permissions().mode() & 0o111 != 0
    }
    #[cfg(not(unix))]
    {
        true
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    struct Scratch(PathBuf);

    impl Scratch {
        fn new(name: &str) -> Self {
            static SEQUENCE: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
            let path = std::env::temp_dir().join(format!(
                "nia-program-search-{name}-{}-{}",
                std::process::id(),
                SEQUENCE.fetch_add(1, std::sync::atomic::Ordering::Relaxed)
            ));
            std::fs::create_dir_all(&path).unwrap();
            Self(path)
        }

        fn program(&self, relative: &str) -> PathBuf {
            let path = self.0.join(relative);
            std::fs::create_dir_all(path.parent().unwrap()).unwrap();
            std::fs::write(&path, b"").unwrap();
            #[cfg(unix)]
            {
                use std::os::unix::fs::PermissionsExt as _;
                std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o755)).unwrap();
            }
            path
        }
    }

    impl Drop for Scratch {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.0);
        }
    }

    fn find(
        name: &str,
        base: &Path,
        path: Option<&OsStr>,
        pathext: Option<&OsStr>,
    ) -> Option<PathBuf> {
        find_program(
            name,
            &ProgramSearch {
                base,
                path,
                pathext,
            },
            |candidate| {
                let metadata = std::fs::metadata(candidate).ok()?;
                is_executable_file(&metadata).then(|| candidate.to_path_buf())
            },
        )
    }

    #[test]
    fn separator_names_resolve_against_the_base_without_searching() {
        let scratch = Scratch::new("explicit");
        let tool = scratch.program("tools/tool");
        let elsewhere = scratch.program("bin/tool");
        let path = elsewhere.parent().unwrap().as_os_str().to_owned();
        assert_eq!(
            find("tools/tool", &scratch.0, Some(&path), None),
            Some(tool)
        );
        assert_eq!(find("tools/missing", &scratch.0, Some(&path), None), None);
    }

    #[test]
    fn bare_names_search_path_in_order_and_resolve_relative_entries() {
        let scratch = Scratch::new("search");
        let first = scratch.program("first/tool");
        scratch.program("second/tool");
        let path = std::env::join_paths(["first", "second"]).unwrap();
        assert_eq!(find("tool", &scratch.0, Some(&path), None), Some(first));
        assert_eq!(find("tool", &scratch.0, None, None), None);
    }

    #[test]
    #[cfg(windows)]
    fn windows_bare_names_try_pathext_after_the_name() {
        let scratch = Scratch::new("pathext");
        let exe = scratch.program("bin/tool.EXE");
        let path = exe.parent().unwrap().as_os_str().to_owned();
        assert_eq!(
            find("tool", &scratch.0, Some(&path), None),
            Some(exe.clone())
        );
        let only_cmd = std::ffi::OsString::from(".CMD");
        assert_eq!(find("tool", &scratch.0, Some(&path), Some(&only_cmd)), None);
        // A name that already has an extension is used as written.
        assert_eq!(
            find("tool.EXE", &scratch.0, Some(&path), Some(&only_cmd)),
            Some(exe)
        );
    }
}
