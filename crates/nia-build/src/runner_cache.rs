// SPDX-License-Identifier: GPL-3.0-or-later
//! Content-addressed cache for generated host build runners.

use std::fs;
use std::io;
use std::path::PathBuf;

use nia_compat::toolchain::BUILD_PROTOCOL;

use crate::{BuildError, BuildInvocation, BuildRunnerSource, OptimizationMode};

const CACHE_SCHEMA: &str = "v3";

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
        // Package versions follow the canonical semver contract. Keep the
        // content-addressed cache key in build metadata so every runner still
        // has a unique package identity without producing an invalid manifest.
        version: format!("0.0.0+{key}"),
    }
}

/// Validates an ordinary runner package artifact without interpreting its
/// semantic sections. Invalid products are retired and treated as misses.
pub(super) fn restore_package(invocation: &BuildInvocation, key: &str) -> io::Result<bool> {
    let cached = package_path(invocation, key);
    let bytes = match fs::read(&cached) {
        Ok(bytes) => bytes,
        Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(false),
        Err(error) => return Err(error),
    };
    let valid = nia_package_metadata::PackageArtifact::open(bytes)
        .and_then(|artifact| {
            artifact.validate_sections()?;
            if artifact.native()?.is_none() {
                return Err(nia_package_metadata::MetadataError::InvalidManifest);
            }
            Ok(())
        })
        .is_ok();
    if !valid {
        let _ = fs::remove_file(&cached);
    }
    Ok(valid)
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
    fn runner_package_artifact_publication_is_atomic_and_validated() {
        let root = std::env::temp_dir().join(format!(
            "nia-runner-cache-package-{}-{}",
            std::process::id(),
            CACHE_SCHEMA
        ));
        let _ = fs::remove_dir_all(&root);
        let invocation = invocation(&root);
        let key = "bundle-test";
        let manifest = nia_package_metadata::PackageManifest::current(
            package_id(key),
            nia_package_metadata::CompilationTarget {
                arch: "x86_64".into(), vendor: "unknown".into(), os: "linux".into(),
                env: "gnu".into(), abi: "".into(), endian: "little".into(), pointer_width: 64,
            },
            0, 0,
        );
        let native = nia_package_metadata::NativeSection { variants: vec![
            nia_package_metadata::NativeVariant { optimization: 0, objects: vec![
                nia_package_metadata::NativeObject {
                    owner: nia_package_metadata::NativeObjectOwner::CompilerBuiltins,
                    key: "builtins".into(), fingerprint: [1, 2], bytes: vec![1],
                }
            ] }
        ]};
        let native_bytes = nia_package_metadata::encode_native(&native).unwrap();
        let bytes = nia_package_metadata::encode_artifact(
            &manifest,
            &[(nia_package_metadata::SectionKind::Native, &native_bytes)],
        ).unwrap();
        let artifact = package_path(&invocation, key);
        fs::create_dir_all(artifact.parent().unwrap()).unwrap();
        fs::write(&artifact, bytes).unwrap();
        assert!(restore_package(&invocation, key).expect("restore runner package"));
        let mut corrupt = fs::read(&artifact).unwrap();
        *corrupt.last_mut().unwrap() ^= 0xff;
        fs::write(&artifact, corrupt).unwrap();
        assert!(!restore_package(&invocation, key).expect("reject corrupt runner package"));
        assert!(!artifact.exists());
    }
}
