// SPDX-License-Identifier: GPL-3.0-or-later
//! Content-addressed cache for generated host build runners.

use std::fs::{self, OpenOptions};
use std::io::{self, Read, Write};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};

#[cfg(windows)]
use std::os::windows::fs::MetadataExt as _;

use nia_compat::{formats, toolchain::BUILD_PROTOCOL};
use nia_query::{FingerprintDomain, QueryFingerprintBuilder};

use crate::{
    BuildError, BuildInvocation, BuildRunnerSource,
    action_cache::{ensure_cache_directory, open_cache_entry, validate_cache_namespace},
    lock::{LockWaitObserver, ScopedFileLock},
};

const CACHE_MAGIC: &[u8; 8] = formats::RUNNER_EXECUTABLE_CACHE.magic;
const CACHE_HEADER_BYTES: usize = CACHE_MAGIC.len() + size_of::<u64>() + blake3::OUT_LEN;
const MAX_RUNNER_BYTES: usize = 256 * 1024 * 1024;
const MAX_RUNNER_INPUT_BYTES: usize = 16 * 1024 * 1024;
const MAX_RUNNER_INPUT_ENTRIES: usize = 1_000_000;
const MAX_RUNNER_INPUT_TREE_BYTES: u64 = 256 * 1024 * 1024;
const MAX_RUNNER_INPUT_DEPTH: usize = 256;
const STREAM_BYTES: usize = 64 * 1024;
const RUNNER_CACHE_DOMAIN: FingerprintDomain =
    FingerprintDomain::new("nia.build.runner-cache.framed");
static NEXT_TEMPORARY_FILE: AtomicU64 = AtomicU64::new(0);
const RUNNER_CACHE_MUTATION_LOCK_DOMAIN: FingerprintDomain =
    FingerprintDomain::new("nia.build.runner-cache-mutation-lock");

/// Canonical private cache record for a generated build runner.
fn cache_path(invocation: &BuildInvocation, key: &str) -> PathBuf {
    invocation
        .cache_dir
        .join("runner")
        .join(formats::RUNNER_CACHE.path_component)
        .join(format!("{key}.cache"))
}

pub(super) fn package_id(key: &str) -> nia_package_metadata::PackageId {
    nia_package_metadata::PackageId {
        namespace: "nia".to_string(),
        name: "build-runner".to_string(),
        // The content key gives generated runner symbols a stable package
        // identity without coupling that identity to physical cache paths.
        version: format!("0.0.0+{key}")
            .parse()
            .expect("runner cache key is a valid SemVer build identifier"),
    }
}

/// Restores a validated runner executable from the private build cache.
/// Invalid records are retired and treated as misses.
pub(super) fn restore_executable(
    invocation: &BuildInvocation,
    key: &str,
    observer: Option<&LockWaitObserver>,
) -> io::Result<bool> {
    let cached = cache_path(invocation, key);
    if let Err(error) = validate_cache_namespace(
        &invocation.cache_dir,
        cached.parent().expect("runner cache executable parent"),
    ) {
        if error.kind() == io::ErrorKind::NotFound {
            return Ok(false);
        }
        return Err(error);
    }
    let _lock = acquire_mutation_lock(invocation, &cached, observer)?;
    let metadata = match fs::symlink_metadata(&cached) {
        Ok(metadata) => metadata,
        Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(false),
        Err(error) => return Err(error),
    };
    #[cfg(windows)]
    let is_reparse = metadata.file_attributes()
        & windows_sys::Win32::Storage::FileSystem::FILE_ATTRIBUTE_REPARSE_POINT
        != 0;
    #[cfg(not(windows))]
    let is_reparse = false;
    if !metadata.is_file() || metadata.file_type().is_symlink() || is_reparse {
        retire(&cached);
        return Ok(false);
    }
    let length = metadata.len();
    let maximum = CACHE_HEADER_BYTES.saturating_add(MAX_RUNNER_BYTES) as u64;
    if length > maximum {
        retire(&cached);
        return Ok(false);
    }
    let bytes = match read_bounded_file(&cached, CACHE_HEADER_BYTES + MAX_RUNNER_BYTES) {
        Ok(bytes) => bytes,
        Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(false),
        Err(error) if error.kind() == io::ErrorKind::InvalidData => {
            retire(&cached);
            return Ok(false);
        }
        Err(error) => return Err(error),
    };
    let Some(executable) = decode_record(&bytes) else {
        retire(&cached);
        return Ok(false);
    };
    crate::create_real_directory(
        invocation
            .runner_executable
            .parent()
            .expect("runner executable parent"),
    )?;
    install_file(&invocation.runner_executable, executable, true)?;
    Ok(true)
}

/// Publishes the linked runner executable as a disposable private cache record.
pub(super) fn publish_executable(
    invocation: &BuildInvocation,
    key: &str,
    observer: Option<&LockWaitObserver>,
) -> io::Result<()> {
    let executable = read_bounded_file(&invocation.runner_executable, MAX_RUNNER_BYTES)?;
    let mut record = Vec::with_capacity(CACHE_HEADER_BYTES + executable.len());
    record.extend_from_slice(CACHE_MAGIC);
    record.extend_from_slice(&(executable.len() as u64).to_le_bytes());
    record.extend_from_slice(blake3::hash(&executable).as_bytes());
    record.extend_from_slice(&executable);
    let path = cache_path(invocation, key);
    ensure_cache_directory(
        &invocation.cache_dir,
        path.parent().expect("runner cache parent"),
    )?;
    let _lock = acquire_mutation_lock(invocation, &path, observer)?;
    install_file(&path, &record, false)
}

fn acquire_mutation_lock(
    invocation: &BuildInvocation,
    path: &Path,
    observer: Option<&LockWaitObserver>,
) -> io::Result<ScopedFileLock> {
    let mut builder = QueryFingerprintBuilder::new(RUNNER_CACHE_MUTATION_LOCK_DOMAIN);
    builder.write_bytes(path.as_os_str().as_encoded_bytes());
    let lock_root = invocation
        .cache_dir
        .join("coordination")
        .join("runner-cache");
    ensure_cache_directory(&invocation.cache_dir, &lock_root)?;
    let [first, second] = builder.finish().parts();
    let lock = lock_root.join(format!("{first:016x}{second:016x}.lock"));
    ScopedFileLock::acquire_interruptible_observed(
        lock,
        || false,
        |event| {
            if let Some(observer) = observer {
                observer.observe(event);
            }
        },
    )?
    .ok_or_else(|| io::Error::other("runner cache mutation lock was cancelled"))
}

fn decode_record(bytes: &[u8]) -> Option<&[u8]> {
    if bytes.get(..CACHE_MAGIC.len())? != CACHE_MAGIC {
        return None;
    }
    let length_offset = CACHE_MAGIC.len();
    let length = u64::from_le_bytes(
        bytes
            .get(length_offset..length_offset + size_of::<u64>())?
            .try_into()
            .ok()?,
    );
    let length = usize::try_from(length).ok()?;
    if length > MAX_RUNNER_BYTES {
        return None;
    }
    let checksum_offset = length_offset + size_of::<u64>();
    let payload_offset = checksum_offset + blake3::OUT_LEN;
    let payload = bytes.get(payload_offset..)?;
    if payload.len() != length
        || bytes.get(checksum_offset..payload_offset)? != blake3::hash(payload).as_bytes()
    {
        return None;
    }
    Some(payload)
}

fn install_file(path: &std::path::Path, bytes: &[u8], executable: bool) -> io::Result<()> {
    let parent = path
        .parent()
        .ok_or_else(|| io::Error::other("runner cache path has no parent"))?;
    let temporary = parent.join(format!(
        ".runner-cache-{}-{}",
        std::process::id(),
        NEXT_TEMPORARY_FILE.fetch_add(1, Ordering::Relaxed)
    ));
    let result = (|| {
        let mut file = OpenOptions::new()
            .create_new(true)
            .write(true)
            .open(&temporary)?;
        file.write_all(bytes)?;
        file.sync_all()?;
        mark_executable(&file, executable)?;
        drop(file);
        nia_compat::replace_path(&temporary, path)
    })();
    if result.is_err() {
        let _ = fs::remove_file(&temporary);
    }
    result
}

#[cfg(unix)]
fn mark_executable(file: &fs::File, executable: bool) -> io::Result<()> {
    use std::os::unix::fs::PermissionsExt as _;
    if executable {
        file.set_permissions(fs::Permissions::from_mode(0o755))?;
    }
    Ok(())
}

#[cfg(windows)]
fn mark_executable(_file: &fs::File, _executable: bool) -> io::Result<()> {
    // Windows determines executability from the PE image rather than mode
    // bits. The linked runner is already a native executable at this point.
    Ok(())
}

#[cfg(not(any(unix, windows)))]
fn mark_executable(_file: &fs::File, _executable: bool) -> io::Result<()> {
    Ok(())
}

fn retire(path: &std::path::Path) {
    let _ = fs::remove_file(path);
}

fn read_bounded_file(path: &std::path::Path, maximum: usize) -> io::Result<Vec<u8>> {
    let mut file = open_cache_entry(path)?;
    let metadata = file.metadata()?;
    if !metadata.is_file() || metadata.len() > maximum as u64 {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            "runner cache file exceeds its size limit or is not a regular file",
        ));
    }
    let expected = usize::try_from(metadata.len()).map_err(|_| {
        io::Error::new(
            io::ErrorKind::InvalidData,
            "runner cache file length does not fit in memory",
        )
    })?;
    let mut bytes = Vec::with_capacity(expected);
    Read::by_ref(&mut file)
        .take(maximum as u64 + 1)
        .read_to_end(&mut bytes)?;
    if bytes.len() > maximum || file.metadata()?.len() != metadata.len() {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            "runner cache file changed while it was read",
        ));
    }
    Ok(bytes)
}

fn hash_bounded_file(
    hasher: &mut blake3::Hasher,
    path: &std::path::Path,
    total_tree_bytes: Option<&mut u64>,
) -> io::Result<()> {
    let mut file = open_cache_entry(path)?;
    let metadata = file.metadata()?;
    if !metadata.is_file() || metadata.len() > MAX_RUNNER_INPUT_BYTES as u64 {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            "runner cache input exceeds its size limit or is not a regular file",
        ));
    }
    let mut remaining = metadata.len();
    if let Some(total_tree_bytes) = total_tree_bytes {
        *total_tree_bytes = total_tree_bytes.checked_add(remaining).ok_or_else(|| {
            io::Error::new(
                io::ErrorKind::InvalidData,
                "runner cache input tree size overflowed",
            )
        })?;
        if *total_tree_bytes > MAX_RUNNER_INPUT_TREE_BYTES {
            return Err(io::Error::new(
                io::ErrorKind::InvalidData,
                "runner cache input tree exceeds its size limit",
            ));
        }
    }
    hasher.update(&remaining.to_le_bytes());
    let mut buffer = [0; STREAM_BYTES];
    while remaining != 0 {
        let count = usize::try_from(remaining.min(buffer.len() as u64)).unwrap();
        file.read_exact(&mut buffer[..count])?;
        hasher.update(&buffer[..count]);
        remaining -= count as u64;
    }
    let mut trailing = [0; 1];
    if file.read(&mut trailing)? != 0 || file.metadata()?.len() != metadata.len() {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            "runner cache input changed while it was read",
        ));
    }
    Ok(())
}

pub(super) fn cache_key(
    invocation: &BuildInvocation,
    runner: &BuildRunnerSource,
) -> Result<String, BuildError> {
    let mut hasher = blake3::Hasher::new();
    hash_field(
        &mut hasher,
        b"domain",
        RUNNER_CACHE_DOMAIN.as_str().as_bytes(),
    );
    hash_field(&mut hasher, b"runner-source", runner.source.as_bytes());
    hash_file(&mut hasher, &invocation.build_script, runner)?;
    hash_field(
        &mut hasher,
        b"package-graph",
        &invocation.package_source_fingerprint,
    );
    hash_field(&mut hasher, b"tree", b"std-source-tree");
    hash_directory(
        &mut hasher,
        invocation.toolchain.resource_root().join("std"),
        runner,
    )?;
    for part in invocation.toolchain.identity().fingerprint().parts() {
        hasher.update(&part.to_le_bytes());
    }
    hash_field(
        &mut hasher,
        b"host-target",
        invocation.toolchain.host_target().name().as_bytes(),
    );
    hasher.update(&[
        profile_tag(invocation.profile),
        link_time_optimization_tag(invocation.link_time_optimization),
    ]);
    hasher.update(&BUILD_PROTOCOL.to_le_bytes());
    Ok(hasher.finalize().to_hex().to_string())
}

fn hash_file(
    hasher: &mut blake3::Hasher,
    path: &std::path::Path,
    runner: &BuildRunnerSource,
) -> Result<(), BuildError> {
    hash_field(
        hasher,
        b"file-name",
        path.file_name()
            .and_then(|name| name.to_str())
            .unwrap_or_default()
            .as_bytes(),
    );
    hash_bounded_file(hasher, path, None).map_err(|error| BuildError::CompileRunner {
        path: runner.path.clone(),
        source: runner.source.clone(),
        error: Box::new(crate::DriverError::Io {
            operation: "read runner cache input",
            path: path.to_path_buf(),
            error,
        }),
    })?;
    Ok(())
}

fn hash_directory(
    hasher: &mut blake3::Hasher,
    root: PathBuf,
    runner: &BuildRunnerSource,
) -> Result<(), BuildError> {
    let mut entries = Vec::new();
    let mut total_entries = 0;
    collect_files(&root, &mut entries, runner, 0, &mut total_entries)?;
    entries.sort();
    hasher.update(&(entries.len() as u64).to_le_bytes());
    let mut total_tree_bytes = 0;
    for path in entries {
        let relative = path.strip_prefix(&root).unwrap_or(&path);
        hash_field(
            hasher,
            b"relative-path",
            relative.to_string_lossy().as_bytes(),
        );
        hash_bounded_file(hasher, &path, Some(&mut total_tree_bytes)).map_err(|error| {
            BuildError::CompileRunner {
                path: runner.path.clone(),
                source: runner.source.clone(),
                error: Box::new(crate::DriverError::Io {
                    operation: "read runner cache input",
                    path: path.clone(),
                    error,
                }),
            }
        })?;
    }
    Ok(())
}

fn hash_field(hasher: &mut blake3::Hasher, tag: &[u8], value: &[u8]) {
    hasher.update(&(tag.len() as u64).to_le_bytes());
    hasher.update(tag);
    hasher.update(&(value.len() as u64).to_le_bytes());
    hasher.update(value);
}

fn collect_files(
    root: &std::path::Path,
    files: &mut Vec<PathBuf>,
    runner: &BuildRunnerSource,
    depth: usize,
    total_entries: &mut usize,
) -> Result<(), BuildError> {
    if depth > MAX_RUNNER_INPUT_DEPTH {
        return Err(BuildError::CompileRunner {
            path: runner.path.clone(),
            source: runner.source.clone(),
            error: Box::new(crate::DriverError::Io {
                operation: "walk runner cache input directory",
                path: root.to_path_buf(),
                error: io::Error::new(
                    io::ErrorKind::InvalidData,
                    "runner cache input directory exceeds its depth limit",
                ),
            }),
        });
    }
    let entries = fs::read_dir(root).map_err(|error| BuildError::CompileRunner {
        path: runner.path.clone(),
        source: runner.source.clone(),
        error: Box::new(crate::DriverError::Io {
            operation: "read runner cache input directory",
            path: root.to_path_buf(),
            error,
        }),
    })?;
    for entry in entries {
        let entry = entry.map_err(|error| BuildError::CompileRunner {
            path: runner.path.clone(),
            source: runner.source.clone(),
            error: Box::new(crate::DriverError::Io {
                operation: "read runner cache input directory",
                path: root.to_path_buf(),
                error,
            }),
        })?;
        *total_entries = total_entries
            .checked_add(1)
            .ok_or_else(|| BuildError::CompileRunner {
                path: runner.path.clone(),
                source: runner.source.clone(),
                error: Box::new(crate::DriverError::Io {
                    operation: "walk runner cache input directory",
                    path: root.to_path_buf(),
                    error: io::Error::new(
                        io::ErrorKind::InvalidData,
                        "runner cache input entry count overflowed",
                    ),
                }),
            })?;
        if *total_entries > MAX_RUNNER_INPUT_ENTRIES {
            return Err(BuildError::CompileRunner {
                path: runner.path.clone(),
                source: runner.source.clone(),
                error: Box::new(crate::DriverError::Io {
                    operation: "walk runner cache input directory",
                    path: root.to_path_buf(),
                    error: io::Error::new(
                        io::ErrorKind::InvalidData,
                        "runner cache input directory has too many entries",
                    ),
                }),
            });
        }
        let path = entry.path();
        let file_type = entry
            .file_type()
            .map_err(|error| BuildError::CompileRunner {
                path: runner.path.clone(),
                source: runner.source.clone(),
                error: Box::new(crate::DriverError::Io {
                    operation: "inspect runner cache input",
                    path: path.clone(),
                    error,
                }),
            })?;
        if file_type.is_dir() && entry.file_name() != ".nia-cache" {
            collect_files(&path, files, runner, depth + 1, total_entries)?;
        } else if file_type.is_file() {
            files.push(path);
        }
    }
    Ok(())
}

fn profile_tag(profile: nia_target_config::BuildProfile) -> u8 {
    match profile {
        nia_target_config::BuildProfile::Debug => 0,
        nia_target_config::BuildProfile::Release => 1,
    }
}

fn link_time_optimization_tag(policy: nia_driver::LinkTimeOptimization) -> u8 {
    match policy {
        nia_driver::LinkTimeOptimization::Off => 0,
        nia_driver::LinkTimeOptimization::Thin => 1,
        nia_driver::LinkTimeOptimization::Full => 2,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{BuildRunnerSource, BuildStepSelection, OptimizationMode};
    use nia_driver::TimingMode;
    use nia_target_config::BuildProfile;
    use nia_timing::TimingFormat;
    #[cfg(unix)]
    use std::os::unix::fs::PermissionsExt as _;
    use std::sync::Arc;

    fn invocation(root: &std::path::Path) -> BuildInvocation {
        let executable = root.join("bin/nia");
        let resources = root.join("lib");
        fs::create_dir_all(resources.join("std")).unwrap();
        fs::create_dir_all(resources.join("runtime")).unwrap();
        fs::create_dir_all(executable.parent().unwrap()).unwrap();
        fs::write(&executable, b"compiler").unwrap();
        fs::write(
            resources.join(nia_toolchain::RESOURCE_MANIFEST_NAME),
            nia_compat::toolchain_manifest(),
        )
        .unwrap();
        for name in nia_compat::TOOLCHAIN_PACKAGES {
            fs::write(
                resources.join(name).join("nia.toml"),
                nia_compat::toolchain_package_manifest(name),
            )
            .unwrap();
        }
        fs::write(resources.join("std/root.nia"), "").unwrap();
        fs::write(
            resources.join("runtime/root.nia"),
            "pub(root) module start;",
        )
        .unwrap();
        fs::write(resources.join("runtime/start.nia"), "").unwrap();
        let toolchain = Arc::new(
            nia_toolchain::ToolchainLayout::resolve(
                nia_toolchain::ToolchainLayoutRequest::explicit(&executable, resources),
            )
            .unwrap(),
        );
        let package_root = root.join("package");
        fs::create_dir_all(&package_root).unwrap();
        let build_script = package_root.join("build.nia");
        fs::write(&build_script, "").unwrap();
        BuildInvocation {
            toolchain,
            package_root: package_root.clone(),
            package_source_fingerprint: [0; 32],
            package_graph_report: nia_package_metadata::PackageGraphReport {
                resolved: Vec::new(),
                declared: Vec::new(),
            },
            build_script,
            build_dir: package_root.join(".nia-build"),
            cache_dir: package_root.join(".nia-cache"),
            runner_dir: package_root.join(".nia-build/runner"),
            runner_executable: package_root.join(".nia-build/runner/runner"),
            runner_config: package_root.join(".nia-build/runner/config"),
            plan_draft: package_root.join(".nia-build/plan.draft"),
            plan_path: package_root.join(".nia-build/plan"),
            step: BuildStepSelection::Default,
            test_filter: None,
            test_list: false,
            test_fail_fast: false,
            forward_output: true,
            timings: TimingMode::Off,
            timing_format: TimingFormat::Text,
            max_parallel_actions: None,
            optimization: OptimizationMode::O0,
            profile: BuildProfile::Debug,
            link_time_optimization: nia_driver::LinkTimeOptimization::Off,
        }
    }

    #[test]
    fn std_source_content_is_part_of_runner_cache_identity() {
        let root =
            std::env::temp_dir().join(format!("nia-runner-cache-key-{}", std::process::id()));
        let _ = fs::remove_dir_all(&root);
        let invocation = invocation(&root);
        let std_root = invocation.toolchain.resource_root().join("std/root.nia");
        fs::write(&std_root, b"pub fun first() () {}").unwrap();
        let runner = BuildRunnerSource {
            path: "runner.nia".into(),
            source: "using std;".into(),
        };
        let first = cache_key(&invocation, &runner).unwrap();
        fs::write(&std_root, b"pub fun second() () {}").unwrap();
        let second = cache_key(&invocation, &runner).unwrap();
        assert_ne!(first, second);
    }

    #[test]
    fn artifact_optimization_is_not_part_of_runner_cache_identity() {
        let root = std::env::temp_dir().join(format!(
            "nia-runner-cache-artifact-optimization-{}",
            std::process::id()
        ));
        let _ = fs::remove_dir_all(&root);
        let mut invocation = invocation(&root);
        let runner = BuildRunnerSource {
            path: "runner.nia".into(),
            source: "using std;".into(),
        };
        let unoptimized = cache_key(&invocation, &runner).unwrap();
        invocation.optimization = OptimizationMode::Oz;
        let optimized_artifact = cache_key(&invocation, &runner).unwrap();
        assert_eq!(unoptimized, optimized_artifact);

        invocation.profile = BuildProfile::Release;
        let release_profile = cache_key(&invocation, &runner).unwrap();
        assert_ne!(unoptimized, release_profile);

        invocation.profile = BuildProfile::Debug;
        invocation.link_time_optimization = nia_driver::LinkTimeOptimization::Thin;
        let thin_lto = cache_key(&invocation, &runner).unwrap();
        assert_ne!(unoptimized, thin_lto);

        invocation.link_time_optimization = nia_driver::LinkTimeOptimization::Full;
        let full_lto = cache_key(&invocation, &runner).unwrap();
        assert_ne!(thin_lto, full_lto);
    }

    #[test]
    fn runner_executable_cache_is_atomic_checked_and_executable() {
        let root = std::env::temp_dir().join(format!(
            "nia-runner-cache-executable-{}-{}",
            std::process::id(),
            formats::RUNNER_CACHE.path_component
        ));
        let _ = fs::remove_dir_all(&root);
        let invocation = invocation(&root);
        let key = "bundle-test";
        fs::create_dir_all(invocation.runner_executable.parent().unwrap()).unwrap();
        fs::write(&invocation.runner_executable, b"runner executable").unwrap();
        publish_executable(&invocation, key, None).expect("publish runner executable");
        fs::remove_file(&invocation.runner_executable).unwrap();
        assert!(restore_executable(&invocation, key, None).expect("restore runner executable"));
        assert_eq!(
            fs::read(&invocation.runner_executable).unwrap(),
            b"runner executable"
        );
        #[cfg(unix)]
        assert_ne!(
            fs::metadata(&invocation.runner_executable)
                .unwrap()
                .permissions()
                .mode()
                & 0o111,
            0
        );
        let cache = cache_path(&invocation, key);
        let mut corrupt = fs::read(&cache).unwrap();
        *corrupt.last_mut().unwrap() ^= 0xff;
        fs::write(&cache, corrupt).unwrap();
        assert!(!restore_executable(&invocation, key, None).expect("reject corrupt runner cache"));
        assert!(!cache.exists());
    }

    #[test]
    fn runner_cache_rejects_non_regular_entry_as_a_miss() {
        let root = std::env::temp_dir().join(format!(
            "nia-runner-cache-non-regular-{}-{}",
            std::process::id(),
            formats::RUNNER_CACHE.path_component
        ));
        let _ = fs::remove_dir_all(&root);
        let invocation = invocation(&root);
        let key = "non-regular";
        fs::create_dir_all(invocation.runner_executable.parent().unwrap()).unwrap();
        fs::write(&invocation.runner_executable, b"runner executable").unwrap();
        publish_executable(&invocation, key, None).expect("publish runner executable");
        fs::remove_file(&invocation.runner_executable).unwrap();

        let cache = cache_path(&invocation, key);
        fs::remove_file(&cache).unwrap();
        fs::create_dir(&cache).unwrap();

        assert!(
            !restore_executable(&invocation, key, None).expect("reject non-regular runner cache")
        );
        assert!(cache.exists(), "directories are not removed as files");
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn runner_cache_identity_frames_source_paths_and_contents() {
        let root = std::env::temp_dir().join(format!(
            "nia-runner-cache-framing-{}-{}",
            std::process::id(),
            formats::RUNNER_CACHE.path_component
        ));
        let _ = fs::remove_dir_all(&root);
        let invocation = invocation(&root);
        let std_root = invocation.toolchain.resource_root().join("std");
        let runner = BuildRunnerSource {
            path: "runner.nia".into(),
            source: "using std;".into(),
        };

        fs::write(std_root.join("a"), b"bc").unwrap();
        let first = cache_key(&invocation, &runner).unwrap();
        fs::remove_file(std_root.join("a")).unwrap();
        fs::write(std_root.join("ab"), b"c").unwrap();
        let second = cache_key(&invocation, &runner).unwrap();

        assert_ne!(first, second);
        fs::remove_dir_all(root).unwrap();
    }

    #[cfg(unix)]
    #[test]
    fn runner_cache_retires_symlink_as_a_miss() {
        use std::os::unix::fs::symlink;

        let root = std::env::temp_dir().join(format!(
            "nia-runner-cache-symlink-{}-{}",
            std::process::id(),
            formats::RUNNER_CACHE.path_component
        ));
        let _ = fs::remove_dir_all(&root);
        let invocation = invocation(&root);
        let key = "symlink";
        fs::create_dir_all(invocation.runner_executable.parent().unwrap()).unwrap();
        fs::write(&invocation.runner_executable, b"runner executable").unwrap();
        publish_executable(&invocation, key, None).expect("publish runner executable");
        fs::remove_file(&invocation.runner_executable).unwrap();

        let cache = cache_path(&invocation, key);
        let outside = root.join("outside.cache");
        fs::write(&outside, b"outside record").unwrap();
        fs::remove_file(&cache).unwrap();
        symlink(&outside, &cache).unwrap();

        assert!(!restore_executable(&invocation, key, None).expect("reject symlink runner cache"));
        assert!(!cache.exists(), "symlink cache record must be retired");
        assert!(outside.exists(), "retirement must not remove the target");
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn runner_cache_rejects_oversized_inputs_before_reading_them() {
        let root = std::env::temp_dir().join(format!(
            "nia-runner-cache-bounds-{}-{}",
            std::process::id(),
            formats::RUNNER_CACHE.path_component
        ));
        let _ = fs::remove_dir_all(&root);
        let invocation = invocation(&root);
        let runner = BuildRunnerSource {
            path: "runner.nia".into(),
            source: "using std;".into(),
        };

        let file = OpenOptions::new()
            .write(true)
            .open(&invocation.build_script)
            .unwrap();
        file.set_len((MAX_RUNNER_INPUT_BYTES + 1) as u64).unwrap();
        let error = cache_key(&invocation, &runner).unwrap_err();
        assert!(error.to_string().contains("size limit"), "{error}");

        fs::write(&invocation.build_script, b"build").unwrap();
        fs::create_dir_all(invocation.runner_executable.parent().unwrap()).unwrap();
        let executable = OpenOptions::new()
            .write(true)
            .create(true)
            .truncate(true)
            .open(&invocation.runner_executable)
            .unwrap();
        executable.set_len((MAX_RUNNER_BYTES + 1) as u64).unwrap();
        let error = publish_executable(&invocation, "oversized", None).unwrap_err();
        assert_eq!(error.kind(), io::ErrorKind::InvalidData);
    }

    #[test]
    fn runner_cache_rejects_oversized_source_trees() {
        let root = std::env::temp_dir().join(format!(
            "nia-runner-cache-tree-bounds-{}-{}",
            std::process::id(),
            formats::RUNNER_CACHE.path_component
        ));
        let _ = fs::remove_dir_all(&root);
        let invocation = invocation(&root);
        let std_root = invocation.toolchain.resource_root().join("std");
        for index in 0..=MAX_RUNNER_INPUT_TREE_BYTES / MAX_RUNNER_INPUT_BYTES as u64 {
            let file = OpenOptions::new()
                .write(true)
                .create_new(true)
                .open(std_root.join(format!("large-{index}")))
                .unwrap();
            file.set_len(MAX_RUNNER_INPUT_BYTES as u64).unwrap();
        }

        let runner = BuildRunnerSource {
            path: "runner.nia".into(),
            source: "using std;".into(),
        };
        let error = cache_key(&invocation, &runner).unwrap_err();
        assert!(
            error.to_string().contains("tree exceeds its size limit"),
            "{error}"
        );
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn runner_cache_rejects_excessive_source_tree_entries() {
        let root = std::env::temp_dir().join(format!(
            "nia-runner-cache-entry-bounds-{}-{}",
            std::process::id(),
            formats::RUNNER_CACHE.path_component
        ));
        let _ = fs::remove_dir_all(&root);
        let invocation = invocation(&root);
        let runner = BuildRunnerSource {
            path: "runner.nia".into(),
            source: "using std;".into(),
        };
        let mut files = Vec::new();
        let mut total_entries = MAX_RUNNER_INPUT_ENTRIES;
        let error = collect_files(
            &invocation.toolchain.resource_root().join("std"),
            &mut files,
            &runner,
            0,
            &mut total_entries,
        )
        .unwrap_err();

        assert!(error.to_string().contains("too many entries"), "{error}");
        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn resolved_package_graph_is_part_of_runner_cache_identity() {
        let root = std::env::temp_dir().join(format!(
            "nia-runner-cache-package-graph-{}",
            std::process::id()
        ));
        let _ = fs::remove_dir_all(&root);
        let mut invocation = invocation(&root);
        let runner = BuildRunnerSource {
            path: "runner.nia".into(),
            source: "using std;".into(),
        };
        let first = cache_key(&invocation, &runner).unwrap();
        invocation.package_source_fingerprint = [1; 32];
        let changed_dependency_graph = cache_key(&invocation, &runner).unwrap();

        assert_ne!(first, changed_dependency_graph);
        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn runner_cache_rejects_excessive_source_tree_depth() {
        let root = std::env::temp_dir().join(format!(
            "nia-runner-cache-depth-bounds-{}-{}",
            std::process::id(),
            formats::RUNNER_CACHE.path_component
        ));
        let _ = fs::remove_dir_all(&root);
        let invocation = invocation(&root);
        let runner = BuildRunnerSource {
            path: "runner.nia".into(),
            source: "using std;".into(),
        };
        let mut files = Vec::new();
        let mut total_entries = 0;
        let error = collect_files(
            &invocation.toolchain.resource_root().join("std"),
            &mut files,
            &runner,
            MAX_RUNNER_INPUT_DEPTH + 1,
            &mut total_entries,
        )
        .unwrap_err();

        assert!(error.to_string().contains("depth limit"), "{error}");
        let _ = fs::remove_dir_all(root);
    }
}
