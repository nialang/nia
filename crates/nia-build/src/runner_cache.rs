// SPDX-License-Identifier: GPL-3.0-or-later
//! Content-addressed cache for generated host build runners.

use std::fs::{self, OpenOptions};
use std::io::{self, Write};
use std::os::unix::fs::PermissionsExt;
use std::path::PathBuf;
use std::sync::atomic::{AtomicU64, Ordering};

use nia_compat::toolchain::BUILD_PROTOCOL;
use nia_query::FingerprintDomain;

use crate::{BuildError, BuildInvocation, BuildRunnerSource, OptimizationMode};

const CACHE_SCHEMA: &str = "v5";
const CACHE_MAGIC: &[u8; 8] = b"NIARUN\0\0";
const CACHE_HEADER_BYTES: usize = CACHE_MAGIC.len() + size_of::<u64>() + blake3::OUT_LEN;
const MAX_RUNNER_BYTES: usize = 256 * 1024 * 1024;
const RUNNER_CACHE_DOMAIN: FingerprintDomain = FingerprintDomain::new("nia.build.runner-cache");
static NEXT_TEMPORARY_FILE: AtomicU64 = AtomicU64::new(0);

/// Canonical private cache record for a generated build runner.
fn cache_path(invocation: &BuildInvocation, key: &str) -> PathBuf {
    invocation
        .cache_dir
        .join("runner")
        .join(CACHE_SCHEMA)
        .join(format!("{key}.cache"))
}

pub(super) fn package_id(key: &str) -> nia_package_metadata::PackageId {
    nia_package_metadata::PackageId {
        namespace: "nia".to_string(),
        name: "build-runner".to_string(),
        // The content key gives generated runner symbols a stable package
        // identity without coupling that identity to physical cache paths.
        version: format!("0.0.0+{key}"),
    }
}

/// Restores a validated runner executable from the private build cache.
/// Invalid records are retired and treated as misses.
pub(super) fn restore_executable(invocation: &BuildInvocation, key: &str) -> io::Result<bool> {
    let cached = cache_path(invocation, key);
    let length = match cached.metadata() {
        Ok(metadata) => metadata.len(),
        Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(false),
        Err(error) => return Err(error),
    };
    let maximum = CACHE_HEADER_BYTES.saturating_add(MAX_RUNNER_BYTES) as u64;
    if length > maximum {
        retire(&cached);
        return Ok(false);
    }
    let bytes = match fs::read(&cached) {
        Ok(bytes) => bytes,
        Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(false),
        Err(error) => return Err(error),
    };
    let Some(executable) = decode_record(&bytes) else {
        retire(&cached);
        return Ok(false);
    };
    install_file(&invocation.runner_executable, executable, true)?;
    Ok(true)
}

/// Publishes the linked runner executable as a disposable private cache record.
pub(super) fn publish_executable(invocation: &BuildInvocation, key: &str) -> io::Result<()> {
    let executable = fs::read(&invocation.runner_executable)?;
    if executable.len() > MAX_RUNNER_BYTES {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            "build runner executable exceeds cache size limit",
        ));
    }
    let mut record = Vec::with_capacity(CACHE_HEADER_BYTES + executable.len());
    record.extend_from_slice(CACHE_MAGIC);
    record.extend_from_slice(&(executable.len() as u64).to_le_bytes());
    record.extend_from_slice(blake3::hash(&executable).as_bytes());
    record.extend_from_slice(&executable);
    install_file(&cache_path(invocation, key), &record, false)
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
    fs::create_dir_all(parent)?;
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
        if executable {
            file.set_permissions(fs::Permissions::from_mode(0o755))?;
        }
        drop(file);
        fs::rename(&temporary, path)
    })();
    if result.is_err() {
        let _ = fs::remove_file(&temporary);
    }
    result
}

fn retire(path: &std::path::Path) {
    let _ = fs::remove_file(path);
}

pub(super) fn cache_key(
    invocation: &BuildInvocation,
    runner: &BuildRunnerSource,
) -> Result<String, BuildError> {
    let mut hasher = blake3::Hasher::new();
    // The cache schema and identity inputs are versioned together. Bumping
    // this domain forces a miss whenever the key construction changes,
    // without relying on stale products from an older implementation.
    hasher.update(RUNNER_CACHE_DOMAIN.as_str().as_bytes());
    hasher.update(runner.source.as_bytes());
    hash_file(&mut hasher, &invocation.build_script, runner)?;
    hasher.update(b"std-source-tree");
    hash_directory(
        &mut hasher,
        invocation.toolchain.resource_root().join("std"),
        runner,
    )?;
    for part in invocation.toolchain.identity().fingerprint().parts() {
        hasher.update(&part.to_le_bytes());
    }
    hasher.update(invocation.toolchain.host_target().arch.as_bytes());
    hasher.update(invocation.toolchain.host_target().vendor.as_bytes());
    hasher.update(invocation.toolchain.host_target().os.as_bytes());
    hasher.update(invocation.toolchain.host_target().env.as_bytes());
    hasher.update(invocation.toolchain.host_target().abi.as_bytes());
    hasher.update(invocation.toolchain.host_target().endian.as_bytes());
    hasher.update(
        &invocation
            .toolchain
            .host_target()
            .pointer_width
            .to_le_bytes(),
    );
    hasher.update(&[
        profile_tag(invocation.profile),
        compilation_mode_tag(invocation.compilation_mode),
    ]);
    hasher.update(&[optimization_tag(invocation.optimization)]);
    hasher.update(&BUILD_PROTOCOL.to_le_bytes());
    Ok(hasher.finalize().to_hex().to_string())
}

fn hash_file(
    hasher: &mut blake3::Hasher,
    path: &std::path::Path,
    runner: &BuildRunnerSource,
) -> Result<(), BuildError> {
    let bytes = fs::read(path).map_err(|error| BuildError::CompileRunner {
        path: runner.path.clone(),
        source: runner.source.clone(),
        error: Box::new(crate::DriverError::Io {
            operation: "read runner cache input",
            path: path.to_path_buf(),
            error,
        }),
    })?;
    hasher.update(
        path.file_name()
            .and_then(|name| name.to_str())
            .unwrap_or_default()
            .as_bytes(),
    );
    hasher.update(&bytes);
    Ok(())
}

fn hash_directory(
    hasher: &mut blake3::Hasher,
    root: PathBuf,
    runner: &BuildRunnerSource,
) -> Result<(), BuildError> {
    let mut entries = Vec::new();
    collect_files(&root, &mut entries, runner)?;
    entries.sort();
    for path in entries {
        let relative = path.strip_prefix(&root).unwrap_or(&path);
        hasher.update(relative.to_string_lossy().as_bytes());
        let bytes = fs::read(&path).map_err(|error| BuildError::CompileRunner {
            path: runner.path.clone(),
            source: runner.source.clone(),
            error: Box::new(crate::DriverError::Io {
                operation: "read runner cache input",
                path: path.clone(),
                error,
            }),
        })?;
        hasher.update(&bytes);
    }
    Ok(())
}

fn collect_files(
    root: &std::path::Path,
    files: &mut Vec<PathBuf>,
    runner: &BuildRunnerSource,
) -> Result<(), BuildError> {
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
            collect_files(&path, files, runner)?;
        } else if file_type.is_file() {
            files.push(path);
        }
    }
    Ok(())
}

fn optimization_tag(mode: OptimizationMode) -> u8 {
    match mode {
        OptimizationMode::O0 => 0,
        OptimizationMode::O1 => 1,
        OptimizationMode::O2 => 2,
        OptimizationMode::O3 => 3,
        OptimizationMode::Os => 4,
        OptimizationMode::Oz => 5,
    }
}

fn profile_tag(profile: nia_target_config::BuildProfile) -> u8 {
    match profile {
        nia_target_config::BuildProfile::Debug => 0,
        nia_target_config::BuildProfile::Release => 1,
    }
}

fn compilation_mode_tag(mode: nia_target_config::CompilationMode) -> u8 {
    match mode {
        nia_target_config::CompilationMode::Normal => 0,
        nia_target_config::CompilationMode::Test => 1,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{BuildRunnerSource, BuildStepSelection};
    use nia_driver::TimingMode;
    use nia_target_config::{BuildProfile, CompilationMode};
    use nia_timing::TimingFormat;
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
        fs::write(resources.join("std/pkg.nia"), "").unwrap();
        fs::write(resources.join("runtime/pkg.nia"), "pub(pkg) module start;").unwrap();
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
            timings: TimingMode::Off,
            timing_format: TimingFormat::Text,
            max_parallel_actions: None,
            optimization: OptimizationMode::O0,
            profile: BuildProfile::Debug,
            compilation_mode: CompilationMode::Normal,
        }
    }

    #[test]
    fn std_source_content_is_part_of_runner_cache_identity() {
        let root =
            std::env::temp_dir().join(format!("nia-runner-cache-key-{}", std::process::id()));
        let _ = fs::remove_dir_all(&root);
        let invocation = invocation(&root);
        let std_root = invocation.toolchain.resource_root().join("std/pkg.nia");
        fs::write(&std_root, b"pub fn first() () {}").unwrap();
        let runner = BuildRunnerSource {
            path: "runner.nia".into(),
            source: "using std;".into(),
        };
        let first = cache_key(&invocation, &runner).unwrap();
        fs::write(&std_root, b"pub fn second() () {}").unwrap();
        let second = cache_key(&invocation, &runner).unwrap();
        assert_ne!(first, second);
    }

    #[test]
    fn runner_executable_cache_is_atomic_checked_and_executable() {
        let root = std::env::temp_dir().join(format!(
            "nia-runner-cache-executable-{}-{}",
            std::process::id(),
            CACHE_SCHEMA
        ));
        let _ = fs::remove_dir_all(&root);
        let invocation = invocation(&root);
        let key = "bundle-test";
        fs::create_dir_all(invocation.runner_executable.parent().unwrap()).unwrap();
        fs::write(&invocation.runner_executable, b"runner executable").unwrap();
        publish_executable(&invocation, key).expect("publish runner executable");
        fs::remove_file(&invocation.runner_executable).unwrap();
        assert!(restore_executable(&invocation, key).expect("restore runner executable"));
        assert_eq!(
            fs::read(&invocation.runner_executable).unwrap(),
            b"runner executable"
        );
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
        assert!(!restore_executable(&invocation, key).expect("reject corrupt runner cache"));
        assert!(!cache.exists());
    }
}
