// SPDX-License-Identifier: GPL-3.0-or-later
//! Typed, deterministic inputs observed while configuring a build.
//!
//! Configure observations are deliberately separate from action inputs. They
//! describe what a build script was allowed to inspect before producing a
//! `BuildPlan`, and therefore form the future configure-cache contract.

use std::{fmt, path::PathBuf};

/// Policy for host observations that are not explicitly declared by a build.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum ObservationPolicy {
    /// Reject an undeclared observation as a non-hermetic configuration.
    Disallow,
    /// Record the observation and force a cache miss on every reuse attempt.
    Poison,
    /// Permit the observation without making it part of cache identity.
    Ignore,
}

/// The kind of filesystem fact a configuration observed.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum ObservationKind {
    /// File bytes and stable relative identity.
    FileContents,
    /// File metadata such as length and modification identity.
    FileMetadata,
    /// Sorted direct directory entries.
    DirectoryContents,
    /// Directory metadata without enumerating children.
    DirectoryMetadata,
}

/// A normalized filesystem observation recorded by a build runner.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct ConfigureObservation {
    path: PathBuf,
    kind: ObservationKind,
    fingerprint: [u8; 32],
}

impl ConfigureObservation {
    /// Creates an observation with a non-empty, relative logical path.
    pub fn new(
        path: impl Into<PathBuf>,
        kind: ObservationKind,
        fingerprint: [u8; 32],
    ) -> Result<Self, ObservationError> {
        let path = path.into();
        if path.as_os_str().is_empty() {
            return Err(ObservationError::EmptyPath);
        }
        let text = path.to_string_lossy();
        if path.is_absolute()
            || text.starts_with('/')
            || text.starts_with('\\')
            || text.as_bytes().get(1).is_some_and(|byte| *byte == b':')
        {
            return Err(ObservationError::AbsolutePath(path));
        }
        if path.components().any(|component| {
            matches!(
                component,
                std::path::Component::ParentDir | std::path::Component::CurDir
            )
        }) {
            return Err(ObservationError::NonCanonicalPath(path));
        }
        Ok(Self {
            path,
            kind,
            fingerprint,
        })
    }

    /// Returns the normalized relative path.
    pub fn path(&self) -> &std::path::Path {
        &self.path
    }

    /// Returns the observed fact kind.
    pub fn kind(&self) -> ObservationKind {
        self.kind
    }

    /// Returns the content-addressed fact fingerprint.
    pub fn fingerprint(&self) -> &[u8; 32] {
        &self.fingerprint
    }
}

/// A deterministic configuration observation manifest.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ConfigureManifest {
    policy: ObservationPolicy,
    observations: Vec<ConfigureObservation>,
}

impl ConfigureManifest {
    /// Creates an empty manifest using the supplied undeclared-observation policy.
    pub fn new(policy: ObservationPolicy) -> Self {
        Self {
            policy,
            observations: Vec::new(),
        }
    }

    /// Adds an observation and keeps the manifest in canonical order.
    pub fn push(&mut self, observation: ConfigureObservation) {
        self.observations.push(observation);
        self.observations.sort();
        self.observations.dedup();
    }

    /// Returns the policy applied to undeclared observations.
    pub fn policy(&self) -> ObservationPolicy {
        self.policy
    }

    /// Returns observations in deterministic path/kind order.
    pub fn observations(&self) -> &[ConfigureObservation] {
        &self.observations
    }
}

/// Rejection reason for a configure observation that cannot cross the cache boundary.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ObservationError {
    /// The observation path was empty.
    EmptyPath,
    /// Absolute host paths cannot become portable configure identities.
    AbsolutePath(PathBuf),
    /// `.` and `..` components are not canonical identities.
    NonCanonicalPath(PathBuf),
}

impl fmt::Display for ObservationError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::EmptyPath => f.write_str("configure observation path is empty"),
            Self::AbsolutePath(path) => write!(
                f,
                "configure observation path `{}` must be relative",
                path.display()
            ),
            Self::NonCanonicalPath(path) => write!(
                f,
                "configure observation path `{}` is not canonical",
                path.display()
            ),
        }
    }
}

impl std::error::Error for ObservationError {}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn manifest_is_sorted_and_deduplicated() {
        let mut manifest = ConfigureManifest::new(ObservationPolicy::Poison);
        let fingerprint = [7; 32];
        manifest.push(
            ConfigureObservation::new("src/b.nia", ObservationKind::FileContents, fingerprint)
                .unwrap(),
        );
        manifest.push(
            ConfigureObservation::new("src/a.nia", ObservationKind::FileContents, fingerprint)
                .unwrap(),
        );
        manifest.push(
            ConfigureObservation::new("src/b.nia", ObservationKind::FileContents, fingerprint)
                .unwrap(),
        );
        assert_eq!(
            manifest
                .observations()
                .iter()
                .map(|observation| observation.path().to_string_lossy().into_owned())
                .collect::<Vec<_>>(),
            ["src/a.nia", "src/b.nia"]
        );
    }

    #[test]
    fn observation_paths_must_be_relative_and_canonical() {
        let fingerprint = [0; 32];
        assert_eq!(
            ConfigureObservation::new("", ObservationKind::FileContents, fingerprint),
            Err(ObservationError::EmptyPath)
        );
        assert!(matches!(
            ConfigureObservation::new("/tmp/input", ObservationKind::FileContents, fingerprint),
            Err(ObservationError::AbsolutePath(_))
        ));
        assert!(matches!(
            ConfigureObservation::new("src/../input", ObservationKind::FileContents, fingerprint),
            Err(ObservationError::NonCanonicalPath(_))
        ));
    }
}
