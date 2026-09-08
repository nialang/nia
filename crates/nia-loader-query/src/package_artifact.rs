// SPDX-License-Identifier: GPL-3.0-or-later
//! Discovery and compatibility selection for compiled package metadata.

use nia_package_metadata::{MetadataError, PackageArtifact, PackageId, SCHEMA_VERSION};
use nia_toolchain::ToolchainLayout;
use std::{
    fs, io,
    path::{Path, PathBuf},
};

/// Conventional optional artifact location for a package root.
pub fn package_artifact_path(package_root: &Path) -> PathBuf {
    package_root.join(".nia-cache/package.niapkg")
}

/// Controls whether an absent or invalid artifact may fall back to source.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PackageArtifactRequest {
    /// Try the artifact and preserve source loading when it cannot be used.
    Optional(PathBuf),
    /// Require the artifact; absence or rejection is an error.
    Required(PathBuf),
}

impl PackageArtifactRequest {
    /// Returns the requested artifact path.
    pub fn path(&self) -> &Path {
        match self {
            Self::Optional(path) | Self::Required(path) => path,
        }
    }

    /// Returns whether failure must be surfaced to the caller.
    pub const fn is_required(&self) -> bool {
        matches!(self, Self::Required(_))
    }
}

/// Reason why an optional artifact was not selected.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PackageArtifactFallback {
    /// No artifact exists at the requested location.
    Missing,
    /// The artifact could not be decoded or its section integrity failed.
    InvalidMetadata(MetadataError),
    /// The artifact is valid but belongs to another compiler/package identity.
    Incompatible(PackageArtifactMismatch),
    /// Reading the artifact failed for a reason other than it being absent.
    Io(String),
}

/// Compatibility field that differed while selecting an artifact.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PackageArtifactMismatch {
    CompilerVersion {
        expected: String,
        found: String,
    },
    StandardLibrarySchema {
        expected: u32,
        found: u32,
    },
    MetadataSchema {
        expected: u32,
        found: u32,
    },
    Package {
        expected: PackageId,
        found: PackageId,
    },
}

/// Result of attempting to select one compiled package artifact.
#[derive(Debug, Clone)]
pub enum PackageArtifactLoad {
    /// The artifact was decoded and passed compatibility checks.
    Loaded {
        path: PathBuf,
        artifact: PackageArtifact,
    },
    /// Source loading should continue, with a diagnostic-quality reason.
    SourceFallback {
        path: PathBuf,
        reason: PackageArtifactFallback,
    },
}

/// Hard failure for a required artifact request.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PackageArtifactError {
    Missing {
        path: PathBuf,
    },
    InvalidMetadata {
        path: PathBuf,
        error: MetadataError,
    },
    Incompatible {
        path: PathBuf,
        mismatch: PackageArtifactMismatch,
    },
    Io {
        path: PathBuf,
        message: String,
    },
}

impl std::fmt::Display for PackageArtifactError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Missing { path } => write!(
                f,
                "compiled package artifact is missing: {}",
                path.display()
            ),
            Self::InvalidMetadata { path, error } => write!(
                f,
                "invalid compiled package artifact {}: {error}",
                path.display()
            ),
            Self::Incompatible { path, mismatch } => write!(
                f,
                "incompatible compiled package artifact {}: {mismatch:?}",
                path.display()
            ),
            Self::Io { path, message } => write!(
                f,
                "failed reading compiled package artifact {}: {message}",
                path.display()
            ),
        }
    }
}

impl std::error::Error for PackageArtifactError {}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct ArtifactCompatibility {
    compiler_version: String,
    std_schema: u32,
    metadata_schema: u32,
}

impl ArtifactCompatibility {
    pub(crate) fn current(toolchain: Option<&ToolchainLayout>) -> Self {
        match toolchain {
            Some(toolchain) => Self {
                compiler_version: toolchain.identity().compiler_version().to_owned(),
                std_schema: toolchain.identity().std_schema(),
                metadata_schema: toolchain.identity().package_metadata_schema(),
            },
            None => Self {
                compiler_version: nia_compat::COMPILER_VERSION.to_owned(),
                std_schema: nia_compat::toolchain::STANDARD_LIBRARY,
                metadata_schema: SCHEMA_VERSION,
            },
        }
    }
}

/// Loads and validates one artifact request without changing source loading.
pub(crate) fn load(
    request: &PackageArtifactRequest,
    expected_package: Option<&PackageId>,
    compatibility: &ArtifactCompatibility,
) -> Result<PackageArtifactLoad, PackageArtifactError> {
    let path = request.path().to_path_buf();
    let bytes = match fs::read(&path) {
        Ok(bytes) => bytes,
        Err(error) if error.kind() == io::ErrorKind::NotFound => {
            return fallback_or_error(request, PackageArtifactFallback::Missing);
        }
        Err(error) => {
            return fallback_or_error(request, PackageArtifactFallback::Io(error.to_string()));
        }
    };
    let artifact = match PackageArtifact::open(bytes) {
        Ok(artifact) => artifact,
        Err(error) => {
            return fallback_or_error(request, PackageArtifactFallback::InvalidMetadata(error));
        }
    };
    if let Err(error) = artifact.validate_sections() {
        return fallback_or_error(request, PackageArtifactFallback::InvalidMetadata(error));
    }
    if let Err(error) = artifact.interface() {
        return fallback_or_error(request, PackageArtifactFallback::InvalidMetadata(error));
    }
    let manifest = artifact.manifest();
    let mismatch = manifest
        .compiler_version
        .ne(&compatibility.compiler_version)
        .then(|| PackageArtifactMismatch::CompilerVersion {
            expected: compatibility.compiler_version.clone(),
            found: manifest.compiler_version.clone(),
        })
        .or_else(|| {
            (manifest.std_schema != compatibility.std_schema).then(|| {
                PackageArtifactMismatch::StandardLibrarySchema {
                    expected: compatibility.std_schema,
                    found: manifest.std_schema,
                }
            })
        })
        .or_else(|| {
            (manifest.schema_version != compatibility.metadata_schema).then(|| {
                PackageArtifactMismatch::MetadataSchema {
                    expected: compatibility.metadata_schema,
                    found: manifest.schema_version,
                }
            })
        })
        .or_else(|| {
            expected_package
                .filter(|expected| *expected != &manifest.package)
                .map(|expected| PackageArtifactMismatch::Package {
                    expected: expected.clone(),
                    found: manifest.package.clone(),
                })
        });
    if let Some(mismatch) = mismatch {
        return fallback_or_error(request, PackageArtifactFallback::Incompatible(mismatch));
    }
    Ok(PackageArtifactLoad::Loaded { path, artifact })
}

/// Selects a compiled package artifact with an optional package identity check.
pub fn select_package_artifact(
    request: &PackageArtifactRequest,
    expected_package: Option<&PackageId>,
    toolchain: Option<&ToolchainLayout>,
) -> Result<PackageArtifactLoad, PackageArtifactError> {
    load(
        request,
        expected_package,
        &ArtifactCompatibility::current(toolchain),
    )
}

fn fallback_or_error(
    request: &PackageArtifactRequest,
    reason: PackageArtifactFallback,
) -> Result<PackageArtifactLoad, PackageArtifactError> {
    if request.is_required() {
        let path = request.path().to_path_buf();
        return Err(match reason {
            PackageArtifactFallback::Missing => PackageArtifactError::Missing { path },
            PackageArtifactFallback::InvalidMetadata(error) => {
                PackageArtifactError::InvalidMetadata { path, error }
            }
            PackageArtifactFallback::Incompatible(mismatch) => {
                PackageArtifactError::Incompatible { path, mismatch }
            }
            PackageArtifactFallback::Io(message) => PackageArtifactError::Io { path, message },
        });
    }
    Ok(PackageArtifactLoad::SourceFallback {
        path: request.path().to_path_buf(),
        reason,
    })
}
