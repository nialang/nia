// SPDX-License-Identifier: GPL-3.0-or-later
//! Content-addressed cache for generated host build runners.

use std::fs;
use std::io::{self, Read, Write};
use std::path::PathBuf;

use nia_compat::toolchain::BUILD_PROTOCOL;

use crate::{BuildError, BuildInvocation, BuildRunnerSource, OptimizationMode};

const CACHE_SCHEMA: &str = "v2";
const MAX_RUNNER_BYTES: u64 = 256 * 1024 * 1024;
const RUNNER_BUNDLE_MAGIC: &[u8; 8] = b"NIARUNR2";
const RUNNER_BUNDLE_HEADER_BYTES: usize = 8 + 32 + 8;

/// Canonical compiled-package product path for a generated build runner.
pub(super) fn package_path(invocation: &BuildInvocation, key: &str) -> PathBuf {
    invocation
        .cache_dir
        .join("runner")
        .join(CACHE_SCHEMA)
        .join(format!("{key}.niapkg"))
}

pub(super) fn package_id(key: &str) -> nia_package_metadata::PackageId {
    nia_package_metadata::PackageId {
        namespace: "nia".to_string(),
        name: "build-runner".to_string(),
        version: key.to_string(),
    }
}

pub(super) fn cache_key(
    invocation: &BuildInvocation,
    runner: &BuildRunnerSource,
) -> Result<String, BuildError> {
    let mut hasher = blake3::Hasher::new();
    hasher.update(b"nia.build.runner-cache.v1");
    hasher.update(runner.source.as_bytes());
    hash_file(&mut hasher, &invocation.build_script, runner)?;
    let std_artifact = invocation.toolchain.std_package_artifact(
        invocation.toolchain.host_target(),
        invocation.profile,
        invocation.compilation_mode,
    );
    if std_artifact.is_file() {
        hasher.update(b"std-package-artifact");
        hash_file(&mut hasher, &std_artifact, runner)?;
    } else {
        hasher.update(b"std-source-tree");
        hash_directory(
            &mut hasher,
            invocation.toolchain.resource_root().join("std"),
            runner,
        )?;
    }
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
        if file_type.is_dir() {
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

fn paths(invocation: &BuildInvocation, key: &str) -> (PathBuf, PathBuf) {
    let root = invocation.cache_dir.join("runner").join(CACHE_SCHEMA);
    (root.join(format!("{key}.bundle")), root)
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
    fn std_artifact_content_is_part_of_runner_cache_identity() {
        let root =
            std::env::temp_dir().join(format!("nia-runner-cache-key-{}", std::process::id()));
        let _ = fs::remove_dir_all(&root);
        let invocation = invocation(&root);
        let artifact = invocation.toolchain.std_package_artifact(
            invocation.toolchain.host_target(),
            invocation.profile,
            invocation.compilation_mode,
        );
        fs::create_dir_all(artifact.parent().unwrap()).unwrap();
        fs::write(&artifact, b"artifact-v1").unwrap();
        let runner = BuildRunnerSource {
            path: "runner.nia".into(),
            source: "using std;".into(),
        };
        let first = cache_key(&invocation, &runner).unwrap();
        fs::write(&artifact, b"artifact-v2").unwrap();
        let second = cache_key(&invocation, &runner).unwrap();
        assert_ne!(first, second);
    }

    #[test]
    fn runner_bundle_publication_and_restore_are_atomic_and_validated() {
        let root = std::env::temp_dir().join(format!(
            "nia-runner-cache-bundle-{}-{}",
            std::process::id(),
            CACHE_SCHEMA
        ));
        let _ = fs::remove_dir_all(&root);
        let invocation = invocation(&root);
        fs::create_dir_all(invocation.runner_executable.parent().unwrap()).unwrap();
        fs::write(&invocation.runner_executable, b"runner-bytes").unwrap();
        let key = "bundle-test";
        publish(&invocation, key).expect("publish runner bundle");
        let (bundle, _) = paths(&invocation, key);
        assert!(bundle.is_file());
        fs::remove_file(&invocation.runner_executable).unwrap();
        assert!(restore(&invocation, key).expect("restore runner bundle"));
        assert_eq!(
            fs::read(&invocation.runner_executable).unwrap(),
            b"runner-bytes"
        );

        let mut corrupt = fs::read(&bundle).unwrap();
        *corrupt.last_mut().unwrap() ^= 0xff;
        fs::write(&bundle, corrupt).unwrap();
        fs::remove_file(&invocation.runner_executable).unwrap();
        assert!(!restore(&invocation, key).expect("reject corrupt runner bundle"));
        assert!(!bundle.exists());
    }
}

pub(super) fn restore(invocation: &BuildInvocation, key: &str) -> io::Result<bool> {
    let (cached, _) = paths(invocation, key);
    let metadata = match fs::metadata(&cached) {
        Ok(metadata)
            if metadata.is_file()
                && metadata.len() > RUNNER_BUNDLE_HEADER_BYTES as u64
                && metadata.len() <= MAX_RUNNER_BYTES + RUNNER_BUNDLE_HEADER_BYTES as u64 =>
        {
            metadata
        }
        Ok(_) => {
            let _ = fs::remove_file(&cached);
            return Ok(false);
        }
        Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(false),
        Err(error) => return Err(error),
    };
    let mut file = fs::File::open(&cached)?;
    let mut bytes = Vec::with_capacity(metadata.len() as usize);
    file.read_to_end(&mut bytes)?;
    if bytes.len() < RUNNER_BUNDLE_HEADER_BYTES
        || &bytes[..RUNNER_BUNDLE_MAGIC.len()] != RUNNER_BUNDLE_MAGIC
    {
        let _ = fs::remove_file(&cached);
        return Ok(false);
    }
    let digest_start = RUNNER_BUNDLE_MAGIC.len();
    let digest_end = digest_start + 32;
    let len_start = digest_end;
    let len_end = len_start + 8;
    let expected_len = u64::from_le_bytes(bytes[len_start..len_end].try_into().unwrap());
    let payload = &bytes[len_end..];
    let valid_len = expected_len == payload.len() as u64 && expected_len <= MAX_RUNNER_BYTES;
    let actual = blake3::hash(payload);
    if !valid_len || bytes[digest_start..digest_end] != *actual.as_bytes() || payload.is_empty() {
        let _ = fs::remove_file(&cached);
        return Ok(false);
    }
    if let Some(parent) = invocation.runner_executable.parent() {
        fs::create_dir_all(parent)?;
    }
    fs::write(&invocation.runner_executable, payload)?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let mut permissions = fs::metadata(&cached)?.permissions();
        permissions.set_mode(0o755);
        fs::set_permissions(&invocation.runner_executable, permissions)?;
    }
    Ok(true)
}

pub(super) fn publish(invocation: &BuildInvocation, key: &str) -> io::Result<()> {
    let source = &invocation.runner_executable;
    let metadata = fs::metadata(source)?;
    if !metadata.is_file() || metadata.len() == 0 || metadata.len() > MAX_RUNNER_BYTES {
        return Ok(());
    }
    let bytes = fs::read(source)?;
    let digest = blake3::hash(&bytes);
    let (cached, _) = paths(invocation, key);
    let parent = cached.parent().expect("runner cache path has parent");
    fs::create_dir_all(parent)?;
    let temp = parent.join(format!(".{}.{}.tmp", key, std::process::id()));
    let mut bundle = Vec::with_capacity(RUNNER_BUNDLE_HEADER_BYTES + bytes.len());
    bundle.extend_from_slice(RUNNER_BUNDLE_MAGIC);
    bundle.extend_from_slice(digest.as_bytes());
    bundle.extend_from_slice(&(bytes.len() as u64).to_le_bytes());
    bundle.extend_from_slice(&bytes);
    {
        let mut file = fs::File::create(&temp)?;
        file.write_all(&bundle)?;
        file.sync_all()?;
    }
    match fs::rename(&temp, &cached) {
        Ok(()) => {}
        Err(error) if error.kind() == io::ErrorKind::AlreadyExists => {
            let _ = fs::remove_file(&temp);
        }
        Err(error) => return Err(error),
    }
    Ok(())
}
