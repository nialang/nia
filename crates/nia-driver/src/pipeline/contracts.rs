// SPDX-License-Identifier: GPL-3.0-or-later
//! Stable contracts shared by the driver pipeline stages.
//!
//! These types describe provenance and cache identity. They deliberately do
//! not own compilation, emission, or linking behavior; those phases consume
//! the contracts from the orchestration module.

use std::{mem::size_of, path::PathBuf};

use nia_compiler_query::CheckedProgram;
use nia_linker::{
    ArchiveCacheKey, ArchiveEnvironmentFingerprint, ArchiveFingerprint,
    ArchiveFingerprintComponents, ArchiveFingerprintSet, LinkResultCacheKey,
    LinkResultEnvironmentFingerprint, LinkResultFingerprint, LinkResultFingerprintComponents,
    LinkResultFingerprintSet,
};
use nia_loader_query::SourceInputManifest;
use nia_target_config::TargetConfig;
use nia_toolchain::ToolchainLayout;

use super::ExecutableArtifact;

/// Checked program paired with the exact source closure used to produce it.
#[derive(Debug, Clone, PartialEq)]
pub struct CheckedProgramWithSourceManifest {
    /// Semantic checked program.
    pub program: CheckedProgram,
    /// Final loader source-input manifest.
    pub source_manifest: SourceInputManifest,
}

const EXECUTABLE_CACHE_REFERENCE_LEN: usize = 12 * size_of::<u64>();
const EXECUTABLE_CACHE_ENVIRONMENT_LEN: usize = 6 * size_of::<u64>();
const STATIC_ARCHIVE_CACHE_REFERENCE_LEN: usize = 12 * size_of::<u64>();
const STATIC_ARCHIVE_CACHE_ENVIRONMENT_LEN: usize = size_of::<[u64; 8]>();

/// Environment fingerprint encoded alongside executable cache references.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ExecutableCacheEnvironment {
    pub(super) fingerprint: LinkResultEnvironmentFingerprint,
}

impl ExecutableCacheEnvironment {
    /// Fixed wire length of the environment fingerprint.
    pub const ENCODED_LEN: usize = EXECUTABLE_CACHE_ENVIRONMENT_LEN;

    /// Encodes target, linker, and option fingerprints in stable order.
    pub fn encode(self) -> [u8; Self::ENCODED_LEN] {
        let mut encoded = [0; Self::ENCODED_LEN];
        let mut offset = 0;
        for fingerprint in [
            self.fingerprint.target.parts(),
            self.fingerprint.linker.parts(),
            self.fingerprint.options.parts(),
        ] {
            for part in fingerprint {
                encoded[offset..offset + size_of::<u64>()].copy_from_slice(&part.to_le_bytes());
                offset += size_of::<u64>();
            }
        }
        encoded
    }
}

/// Complete executable cache identity reference.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ExecutableCacheReference {
    pub(super) fingerprints: LinkResultFingerprintSet,
}

impl ExecutableCacheReference {
    /// Fixed wire length of the cache reference.
    pub const ENCODED_LEN: usize = EXECUTABLE_CACHE_REFERENCE_LEN;

    /// Encodes all cache-key and component fingerprints.
    pub fn encode(self) -> [u8; Self::ENCODED_LEN] {
        let mut encoded = [0; Self::ENCODED_LEN];
        let mut offset = 0;
        for fingerprint in [
            self.fingerprints.cache_key.parts(),
            self.fingerprints.components.inputs.parts(),
            self.fingerprints.components.toolchain.parts(),
            self.fingerprints.components.target.parts(),
            self.fingerprints.components.linker.parts(),
            self.fingerprints.components.options.parts(),
        ] {
            for part in fingerprint {
                encoded[offset..offset + size_of::<u64>()].copy_from_slice(&part.to_le_bytes());
                offset += size_of::<u64>();
            }
        }
        encoded
    }

    /// Decodes an exact-length cache reference, rejecting malformed lengths.
    pub fn decode(encoded: &[u8]) -> Option<Self> {
        (encoded.len() == Self::ENCODED_LEN).then_some(())?;
        // `ENCODED_LEN` is an exact multiple of the chunk width, so the length
        // checked above leaves no trailing bytes.
        let (chunks, _) = encoded.as_chunks::<{ size_of::<u64>() }>();
        let mut parts = chunks.iter().copied().map(u64::from_le_bytes);
        let mut fingerprint = || Some([parts.next()?, parts.next()?]);
        let cache_key = LinkResultCacheKey::from_parts(fingerprint()?);
        let components = LinkResultFingerprintComponents {
            inputs: LinkResultFingerprint::from_parts(fingerprint()?),
            toolchain: LinkResultFingerprint::from_parts(fingerprint()?),
            target: LinkResultFingerprint::from_parts(fingerprint()?),
            linker: LinkResultFingerprint::from_parts(fingerprint()?),
            options: LinkResultFingerprint::from_parts(fingerprint()?),
        };
        Some(Self {
            fingerprints: LinkResultFingerprintSet::new(cache_key, components),
        })
    }
}

impl From<LinkResultFingerprintSet> for ExecutableCacheReference {
    fn from(fingerprints: LinkResultFingerprintSet) -> Self {
        Self { fingerprints }
    }
}

/// Environment fingerprint encoded alongside static archive references.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct StaticArchiveCacheEnvironment {
    pub(super) fingerprint: ArchiveEnvironmentFingerprint,
}

impl StaticArchiveCacheEnvironment {
    /// Fixed wire length of the archive environment fingerprint.
    pub const ENCODED_LEN: usize = STATIC_ARCHIVE_CACHE_ENVIRONMENT_LEN;

    /// Encodes toolchain, target, tool, and option fingerprints.
    pub fn encode(self) -> [u8; Self::ENCODED_LEN] {
        let mut encoded = [0; Self::ENCODED_LEN];
        let mut offset = 0;
        for fingerprint in [
            self.fingerprint.toolchain,
            self.fingerprint.target,
            self.fingerprint.tool,
            self.fingerprint.options,
        ] {
            for part in fingerprint.parts() {
                encoded[offset..offset + size_of::<u64>()].copy_from_slice(&part.to_le_bytes());
                offset += size_of::<u64>();
            }
        }
        encoded
    }
}

/// Complete static archive cache identity reference.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct StaticArchiveCacheReference {
    pub(super) fingerprints: ArchiveFingerprintSet,
}

impl StaticArchiveCacheReference {
    /// Fixed wire length of the archive cache reference.
    pub const ENCODED_LEN: usize = STATIC_ARCHIVE_CACHE_REFERENCE_LEN;

    /// Encodes all cache-key and component fingerprints.
    pub fn encode(self) -> [u8; Self::ENCODED_LEN] {
        let mut encoded = [0; Self::ENCODED_LEN];
        let mut offset = 0;
        for fingerprint in [
            ArchiveFingerprint::from_parts(self.fingerprints.cache_key.parts()),
            self.fingerprints.components.inputs,
            self.fingerprints.components.toolchain,
            self.fingerprints.components.target,
            self.fingerprints.components.tool,
            self.fingerprints.components.options,
        ] {
            for part in fingerprint.parts() {
                encoded[offset..offset + size_of::<u64>()].copy_from_slice(&part.to_le_bytes());
                offset += size_of::<u64>();
            }
        }
        encoded
    }

    /// Decodes an exact-length archive cache reference.
    pub fn decode(encoded: &[u8]) -> Option<Self> {
        (encoded.len() == Self::ENCODED_LEN).then_some(())?;
        // `ENCODED_LEN` is an exact multiple of the chunk width, so the length
        // checked above leaves no trailing bytes.
        let (chunks, _) = encoded.as_chunks::<{ size_of::<u64>() }>();
        let mut parts = chunks.iter().copied().map(u64::from_le_bytes);
        let mut fingerprint = || Some([parts.next()?, parts.next()?]);
        let cache_key = ArchiveCacheKey::from_parts(fingerprint()?);
        let components = ArchiveFingerprintComponents {
            inputs: ArchiveFingerprint::from_parts(fingerprint()?),
            toolchain: ArchiveFingerprint::from_parts(fingerprint()?),
            target: ArchiveFingerprint::from_parts(fingerprint()?),
            tool: ArchiveFingerprint::from_parts(fingerprint()?),
            options: ArchiveFingerprint::from_parts(fingerprint()?),
        };
        Some(Self {
            fingerprints: ArchiveFingerprintSet::new(cache_key, components),
        })
    }
}

impl From<ArchiveFingerprintSet> for StaticArchiveCacheReference {
    fn from(fingerprints: ArchiveFingerprintSet) -> Self {
        Self { fingerprints }
    }
}

/// Outcome of restoring a static archive from its cache.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum StaticArchiveCacheRestore {
    /// Valid matching artifact was restored.
    Hit,
    /// No cache entry exists.
    NotFound,
    /// Entry exists but one identity component is stale.
    Invalidated,
    /// Entry failed decoding or validation.
    Corrupt,
    /// Cache read failed.
    ReadError,
    /// Cache is disabled for this request.
    Disabled,
}

/// Outcome of restoring an executable from its cache.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ExecutableCacheRestore {
    /// Valid matching artifact was restored.
    Hit,
    /// No cache entry exists.
    NotFound,
    /// Entry exists but one identity component is stale.
    Invalidated,
    /// Entry failed decoding or validation.
    Corrupt,
    /// Cache read failed.
    ReadError,
    /// Cache is disabled for this request.
    Disabled,
}

/// Linked executable paired with the exact source closure used to link it.
#[derive(Debug, Clone, PartialEq)]
pub struct LinkedExecutableWithSourceManifest {
    /// Linked artifact.
    pub artifact: ExecutableArtifact,
    /// Final source-input manifest.
    pub source_manifest: SourceInputManifest,
}

/// Driver-wide toolchain, target, cache, and verification configuration.
#[derive(Debug, Clone)]
pub struct DriverConfig {
    /// Toolchain layout used for compilation and linking.
    pub toolchain: std::sync::Arc<ToolchainLayout>,
    /// Target configuration for emitted artifacts.
    pub artifact_target: TargetConfig,
    /// Optional persistent artifact-cache directory.
    pub artifact_cache_dir: Option<PathBuf>,
    /// Whether frontend cache hits receive semantic verification.
    pub verify_frontend_cache: bool,
}

impl DriverConfig {
    /// Creates configuration using the toolchain's artifact target.
    pub fn new(toolchain: std::sync::Arc<ToolchainLayout>) -> Self {
        let artifact_target = toolchain.artifact_target().clone();
        Self {
            toolchain,
            artifact_target,
            artifact_cache_dir: None,
            verify_frontend_cache: false,
        }
    }

    /// Overrides the artifact target while retaining the toolchain.
    pub fn with_artifact_target(mut self, artifact_target: TargetConfig) -> Self {
        self.artifact_target = artifact_target;
        self
    }
}
