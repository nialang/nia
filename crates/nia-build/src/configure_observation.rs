// SPDX-License-Identifier: GPL-3.0-or-later
//! Typed, deterministic inputs observed while configuring a build.
//!
//! Configure observations are deliberately separate from action inputs. They
//! describe what a build script was allowed to inspect before producing a
//! `BuildPlan`, and therefore form the future configure-cache contract.

use std::{
    fmt, fs, io,
    path::{Path, PathBuf},
};

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

/// Result of comparing a persisted observation with the current filesystem.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ObservationValidation {
    /// Every observed fact still has the recorded fingerprint.
    Match,
    /// An observed path exists but its fact changed.
    Changed {
        path: PathBuf,
        kind: ObservationKind,
    },
    /// An observed path cannot be read in the current checkout.
    Missing {
        path: PathBuf,
        kind: ObservationKind,
    },
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

    /// Recomputes this observation against a package root.
    pub fn refresh(&self, root: &Path) -> io::Result<Self> {
        match self.kind {
            ObservationKind::FileContents => observe_file_contents(root, self.path.clone()),
            ObservationKind::FileMetadata => observe_metadata(root, self.path.clone()),
            ObservationKind::DirectoryContents => {
                observe_directory_contents(root, self.path.clone())
            }
            ObservationKind::DirectoryMetadata => {
                observe_directory_metadata(root, self.path.clone())
            }
        }
    }
}

/// Records the contents of a package-relative regular file.
pub fn observe_file_contents(
    root: &Path,
    relative: impl Into<PathBuf>,
) -> io::Result<ConfigureObservation> {
    let relative = relative.into();
    let bytes = fs::read(root.join(&relative))?;
    ConfigureObservation::new(
        relative,
        ObservationKind::FileContents,
        *blake3::hash(&bytes).as_bytes(),
    )
    .map_err(|error| io::Error::new(io::ErrorKind::InvalidInput, error))
}

/// Records deterministic direct-entry names and metadata for a package-relative directory.
pub fn observe_directory_contents(
    root: &Path,
    relative: impl Into<PathBuf>,
) -> io::Result<ConfigureObservation> {
    let relative = relative.into();
    let directory = root.join(&relative);
    let mut entries = Vec::new();
    for entry in fs::read_dir(directory)? {
        let entry = entry?;
        let metadata = entry.metadata()?;
        entries.push((
            entry.file_name().to_string_lossy().into_owned(),
            metadata.is_dir(),
            metadata.len(),
        ));
    }
    entries.sort();
    let mut hasher = blake3::Hasher::new();
    for (name, is_dir, len) in entries {
        hasher.update(name.as_bytes());
        hasher.update(&[u8::from(is_dir)]);
        hasher.update(&len.to_le_bytes());
    }
    ConfigureObservation::new(
        relative,
        ObservationKind::DirectoryContents,
        *hasher.finalize().as_bytes(),
    )
    .map_err(|error| io::Error::new(io::ErrorKind::InvalidInput, error))
}

/// Records portable metadata for a package-relative filesystem entry.
pub fn observe_metadata(
    root: &Path,
    relative: impl Into<PathBuf>,
) -> io::Result<ConfigureObservation> {
    let relative = relative.into();
    let metadata = fs::metadata(root.join(&relative))?;
    let mut bytes = Vec::with_capacity(24);
    bytes.extend_from_slice(&metadata.len().to_le_bytes());
    bytes.push(u8::from(metadata.is_dir()));
    bytes.push(u8::from(metadata.is_file()));
    bytes.push(u8::from(metadata.is_symlink()));
    let modified = metadata
        .modified()
        .ok()
        .and_then(|time| time.duration_since(std::time::UNIX_EPOCH).ok());
    bytes.extend_from_slice(&modified.map_or(0, |value| value.as_secs()).to_le_bytes());
    bytes.extend_from_slice(
        &modified
            .map_or(0, |value| u64::from(value.subsec_nanos()))
            .to_le_bytes(),
    );
    ConfigureObservation::new(
        relative,
        ObservationKind::FileMetadata,
        *blake3::hash(&bytes).as_bytes(),
    )
    .map_err(|error| io::Error::new(io::ErrorKind::InvalidInput, error))
}

/// Records portable metadata for a package-relative directory.
pub fn observe_directory_metadata(
    root: &Path,
    relative: impl Into<PathBuf>,
) -> io::Result<ConfigureObservation> {
    let relative = relative.into();
    let metadata = fs::metadata(root.join(&relative))?;
    if !metadata.is_dir() {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            "directory metadata observation requires a directory",
        ));
    }
    let mut bytes = Vec::with_capacity(24);
    bytes.extend_from_slice(&metadata.len().to_le_bytes());
    let modified = metadata
        .modified()
        .ok()
        .and_then(|time| time.duration_since(std::time::UNIX_EPOCH).ok());
    bytes.extend_from_slice(&modified.map_or(0, |value| value.as_secs()).to_le_bytes());
    bytes.extend_from_slice(
        &modified
            .map_or(0, |value| u64::from(value.subsec_nanos()))
            .to_le_bytes(),
    );
    ConfigureObservation::new(
        relative,
        ObservationKind::DirectoryMetadata,
        *blake3::hash(&bytes).as_bytes(),
    )
    .map_err(|error| io::Error::new(io::ErrorKind::InvalidInput, error))
}

/// Adds a filesystem observation to a manifest, preserving canonical order.
pub fn observe_into(manifest: &mut ConfigureManifest, observation: ConfigureObservation) {
    manifest.push(observation);
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

    /// Compares every persisted observation with the current package root.
    pub fn validate(&self, root: &Path) -> ObservationValidation {
        for observation in &self.observations {
            match observation.refresh(root) {
                Ok(current) if current.fingerprint() == observation.fingerprint() => {}
                Ok(_) => {
                    return ObservationValidation::Changed {
                        path: observation.path.clone(),
                        kind: observation.kind,
                    };
                }
                Err(_) => {
                    return ObservationValidation::Missing {
                        path: observation.path.clone(),
                        kind: observation.kind,
                    };
                }
            }
        }
        ObservationValidation::Match
    }

    /// Returns a canonical fingerprint for this manifest.
    pub fn fingerprint(&self) -> [u8; 32] {
        let mut hasher = blake3::Hasher::new();
        hasher.update(b"nia.build.configure-manifest\0");
        hasher.update(&[match self.policy {
            ObservationPolicy::Disallow => 0,
            ObservationPolicy::Poison => 1,
            ObservationPolicy::Ignore => 2,
        }]);
        for observation in &self.observations {
            hasher.update(observation.path.to_string_lossy().as_bytes());
            hasher.update(&[match observation.kind {
                ObservationKind::FileContents => 0,
                ObservationKind::FileMetadata => 1,
                ObservationKind::DirectoryContents => 2,
                ObservationKind::DirectoryMetadata => 3,
            }]);
            hasher.update(&observation.fingerprint);
        }
        *hasher.finalize().as_bytes()
    }

    /// Encodes a bounded, versioned manifest record for the configure cache.
    pub fn encode(&self) -> Result<Vec<u8>, ObservationCodecError> {
        let mut payload = Vec::new();
        payload.push(match self.policy {
            ObservationPolicy::Disallow => 0,
            ObservationPolicy::Poison => 1,
            ObservationPolicy::Ignore => 2,
        });
        let count =
            u32::try_from(self.observations.len()).map_err(|_| ObservationCodecError::TooLarge)?;
        payload.extend_from_slice(&count.to_le_bytes());
        for observation in &self.observations {
            let path = observation.path.to_string_lossy();
            let path_len =
                u32::try_from(path.len()).map_err(|_| ObservationCodecError::TooLarge)?;
            payload.extend_from_slice(&path_len.to_le_bytes());
            payload.extend_from_slice(path.as_bytes());
            payload.push(match observation.kind {
                ObservationKind::FileContents => 0,
                ObservationKind::FileMetadata => 1,
                ObservationKind::DirectoryContents => 2,
                ObservationKind::DirectoryMetadata => 3,
            });
            payload.extend_from_slice(&observation.fingerprint);
        }
        let mut encoded = Vec::with_capacity(8 + 4 + 4 + 32 + payload.len());
        encoded.extend_from_slice(nia_compat::formats::CONFIGURE_MANIFEST.magic);
        encoded.extend_from_slice(
            &nia_compat::formats::CONFIGURE_MANIFEST
                .release_compatibility
                .to_le_bytes(),
        );
        encoded.extend_from_slice(
            &u32::try_from(payload.len())
                .map_err(|_| ObservationCodecError::TooLarge)?
                .to_le_bytes(),
        );
        encoded.extend_from_slice(blake3::hash(&payload).as_bytes());
        encoded.extend_from_slice(&payload);
        Ok(encoded)
    }

    /// Encodes the manifest as the observation payload carried by a build draft.
    pub fn draft_payload(&self) -> Result<Vec<u8>, ObservationCodecError> {
        self.encode()
    }
}

/// Failure decoding a persisted configure manifest.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ObservationCodecError {
    /// The payload exceeds the bounded cache record size.
    TooLarge,
    /// The payload does not match the registered format.
    Invalid,
}

impl ConfigureManifest {
    /// Decodes and validates a persisted configure manifest record.
    pub fn decode(encoded: &[u8]) -> Result<Self, ObservationCodecError> {
        const HEADER: usize = 8 + 4 + 4 + 32;
        if encoded.len() < HEADER || encoded.len() > 16 * 1024 * 1024 {
            return Err(ObservationCodecError::Invalid);
        }
        let format = nia_compat::formats::CONFIGURE_MANIFEST;
        if encoded[..8] != *format.magic
            || encoded[8..12] != format.release_compatibility.to_le_bytes()
        {
            return Err(ObservationCodecError::Invalid);
        }
        let length = u32::from_le_bytes(encoded[12..16].try_into().unwrap()) as usize;
        if length != encoded.len() - HEADER
            || encoded[16..48] != *blake3::hash(&encoded[HEADER..]).as_bytes()
        {
            return Err(ObservationCodecError::Invalid);
        }
        let payload = &encoded[HEADER..];
        let policy = match payload.first().copied() {
            Some(0) => ObservationPolicy::Disallow,
            Some(1) => ObservationPolicy::Poison,
            Some(2) => ObservationPolicy::Ignore,
            _ => return Err(ObservationCodecError::Invalid),
        };
        let mut cursor = 1usize;
        let count = read_u32(payload, &mut cursor)? as usize;
        let mut manifest = ConfigureManifest::new(policy);
        for _ in 0..count {
            let path_len = read_u32(payload, &mut cursor)? as usize;
            let path = std::str::from_utf8(
                payload
                    .get(
                        cursor
                            ..cursor
                                .checked_add(path_len)
                                .ok_or(ObservationCodecError::Invalid)?,
                    )
                    .ok_or(ObservationCodecError::Invalid)?,
            )
            .map_err(|_| ObservationCodecError::Invalid)?;
            cursor += path_len;
            let kind = match payload.get(cursor).copied() {
                Some(0) => ObservationKind::FileContents,
                Some(1) => ObservationKind::FileMetadata,
                Some(2) => ObservationKind::DirectoryContents,
                Some(3) => ObservationKind::DirectoryMetadata,
                _ => return Err(ObservationCodecError::Invalid),
            };
            cursor += 1;
            let fingerprint = payload
                .get(cursor..cursor + 32)
                .ok_or(ObservationCodecError::Invalid)?
                .try_into()
                .unwrap();
            cursor += 32;
            manifest.push(
                ConfigureObservation::new(path, kind, fingerprint)
                    .map_err(|_| ObservationCodecError::Invalid)?,
            );
        }
        (cursor == payload.len())
            .then_some(manifest)
            .ok_or(ObservationCodecError::Invalid)
    }
}

fn read_u32(payload: &[u8], cursor: &mut usize) -> Result<u32, ObservationCodecError> {
    let bytes = payload
        .get(
            *cursor
                ..cursor
                    .checked_add(4)
                    .ok_or(ObservationCodecError::Invalid)?,
        )
        .ok_or(ObservationCodecError::Invalid)?;
    *cursor += 4;
    Ok(u32::from_le_bytes(bytes.try_into().unwrap()))
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

    #[test]
    fn equivalent_manifests_have_stable_fingerprints() {
        let fingerprint = [3; 32];
        let mut first = ConfigureManifest::new(ObservationPolicy::Poison);
        let mut second = ConfigureManifest::new(ObservationPolicy::Poison);
        let observation =
            ConfigureObservation::new("src/main.nia", ObservationKind::FileContents, fingerprint)
                .unwrap();
        first.push(observation.clone());
        second.push(observation);
        assert_eq!(first.fingerprint(), second.fingerprint());
        second.push(
            ConfigureObservation::new("options", ObservationKind::FileMetadata, fingerprint)
                .unwrap(),
        );
        assert_ne!(first.fingerprint(), second.fingerprint());
    }

    #[test]
    fn manifest_record_round_trips_and_rejects_damage() {
        let mut manifest = ConfigureManifest::new(ObservationPolicy::Disallow);
        manifest.push(
            ConfigureObservation::new("src/main.nia", ObservationKind::FileContents, [9; 32])
                .unwrap(),
        );
        let encoded = manifest.encode().unwrap();
        assert_eq!(ConfigureManifest::decode(&encoded).unwrap(), manifest);
        let mut damaged = encoded.clone();
        *damaged.last_mut().unwrap() ^= 1;
        assert_eq!(
            ConfigureManifest::decode(&damaged),
            Err(ObservationCodecError::Invalid)
        );
    }

    #[test]
    fn filesystem_observations_are_stable() {
        let root = std::env::temp_dir().join(format!(
            "nia-observation-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        std::fs::create_dir_all(root.join("src")).unwrap();
        std::fs::write(root.join("src/input"), b"nia").unwrap();
        let first = observe_file_contents(&root, "src/input").unwrap();
        let second = observe_file_contents(&root, "src/input").unwrap();
        assert_eq!(first, second);
        let mut manifest = ConfigureManifest::new(ObservationPolicy::Disallow);
        observe_into(&mut manifest, first);
        assert!(!manifest.draft_payload().unwrap().is_empty());
        std::fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn persisted_observations_validate_current_filesystem_facts() {
        let root = std::env::temp_dir().join(format!(
            "nia-observation-validation-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        std::fs::create_dir_all(root.join("src")).unwrap();
        std::fs::write(root.join("src/input"), b"before").unwrap();
        let mut manifest = ConfigureManifest::new(ObservationPolicy::Disallow);
        manifest.push(observe_file_contents(&root, "src/input").unwrap());
        manifest.push(observe_directory_metadata(&root, "src").unwrap());
        assert_eq!(manifest.validate(&root), ObservationValidation::Match);
        std::fs::write(root.join("src/input"), b"after").unwrap();
        assert!(matches!(
            manifest.validate(&root),
            ObservationValidation::Changed { ref path, kind: ObservationKind::FileContents }
                if path == std::path::Path::new("src/input")
        ));
        std::fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn directory_metadata_has_a_distinct_observation_kind() {
        let root =
            std::env::temp_dir().join(format!("nia-observation-directory-{}", std::process::id()));
        std::fs::create_dir_all(root.join("src")).unwrap();
        let observation = observe_directory_metadata(&root, "src").unwrap();
        assert_eq!(observation.kind(), ObservationKind::DirectoryMetadata);
        std::fs::remove_dir_all(root).unwrap();
    }
}
