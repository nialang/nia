// SPDX-License-Identifier: GPL-3.0-or-later
//! Persistent cache records for validated configuration identities.

use std::{fs, io, path::PathBuf};

use nia_compat::formats;

use crate::{BuildInvocation, ConfigureManifest, ObservationCodecError};

const MAGIC: &[u8; 8] = formats::CONFIGURE_MANIFEST.magic;
const HEADER_BYTES: usize = 8 + 4 + 4 + 32;
const MAX_RECORD_BYTES: usize = 16 * 1024 * 1024;

/// Inputs that determine whether a configured plan may be reused.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ConfigureIdentity {
    /// Runner executable cache identity.
    pub runner: String,
    /// Invocation target and policy identity.
    pub invocation: String,
    /// Canonical observation manifest fingerprint.
    pub observations: [u8; 32],
}

impl ConfigureIdentity {
    /// Creates a content-addressed configure identity.
    pub fn new(
        runner: impl Into<String>,
        invocation: impl Into<String>,
        manifest: &ConfigureManifest,
    ) -> Self {
        Self {
            runner: runner.into(),
            invocation: invocation.into(),
            observations: manifest.fingerprint(),
        }
    }

    /// Returns the canonical cache key.
    pub fn key(&self) -> String {
        let mut hasher = blake3::Hasher::new();
        hasher.update(b"nia.build.configure-cache\0");
        hasher.update(self.runner.as_bytes());
        hasher.update(&[0]);
        hasher.update(self.invocation.as_bytes());
        hasher.update(&[0]);
        hasher.update(&self.observations);
        hasher.finalize().to_hex().to_string()
    }
}

/// Private persistent configure cache owner.
#[derive(Debug, Clone)]
pub struct ConfigureCache {
    root: PathBuf,
}

impl ConfigureCache {
    /// Creates a cache below the invocation's persistent cache directory.
    pub fn new(invocation: &BuildInvocation) -> Self {
        Self {
            root: invocation.cache_dir.join("configure"),
        }
    }

    fn path(&self, identity: &ConfigureIdentity) -> PathBuf {
        self.root
            .join(
                formats::CONFIGURE_MANIFEST
                    .release_compatibility
                    .to_string(),
            )
            .join(format!("{}.manifest", identity.key()))
    }

    /// Stores a validated observation manifest atomically.
    pub fn publish(
        &self,
        identity: &ConfigureIdentity,
        manifest: &ConfigureManifest,
    ) -> io::Result<()> {
        let bytes = manifest
            .encode()
            .map_err(|_| io::Error::other("configure manifest too large"))?;
        let path = self.path(identity);
        let parent = path.parent().expect("configure cache path has parent");
        fs::create_dir_all(parent)?;
        let temporary = parent.join(format!(
            ".{}.tmp-{}",
            path.file_name().unwrap().to_string_lossy(),
            std::process::id()
        ));
        fs::write(&temporary, &bytes)?;
        nia_compat::replace_path(&temporary, &path)
    }

    /// Reads and validates a manifest; corruption is reported as a cache miss.
    pub fn lookup(&self, identity: &ConfigureIdentity) -> io::Result<Option<ConfigureManifest>> {
        let path = self.path(identity);
        let bytes = match fs::read(&path) {
            Ok(bytes) => bytes,
            Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(None),
            Err(error) => return Err(error),
        };
        if bytes.len() > MAX_RECORD_BYTES || bytes.len() < HEADER_BYTES || &bytes[..8] != MAGIC {
            let _ = fs::remove_file(&path);
            return Ok(None);
        }
        match ConfigureManifest::decode(&bytes) {
            Ok(manifest) => Ok(Some(manifest)),
            Err(ObservationCodecError::Invalid | ObservationCodecError::TooLarge) => {
                let _ = fs::remove_file(&path);
                Ok(None)
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{ConfigureObservation, ObservationKind, ObservationPolicy};
    #[test]
    fn identity_changes_with_invocation_and_observations() {
        let mut manifest = ConfigureManifest::new(ObservationPolicy::Poison);
        manifest.push(
            ConfigureObservation::new("input", ObservationKind::FileContents, [1; 32]).unwrap(),
        );
        let first = ConfigureIdentity::new("runner-a", "target-a", &manifest).key();
        let second = ConfigureIdentity::new("runner-a", "target-b", &manifest).key();
        assert_ne!(first, second);
        manifest.push(
            ConfigureObservation::new("other", ObservationKind::FileContents, [1; 32]).unwrap(),
        );
        assert_ne!(
            first,
            ConfigureIdentity::new("runner-a", "target-a", &manifest).key()
        );
    }

    #[test]
    fn cache_publishes_and_restores_a_validated_manifest() {
        let root = std::env::temp_dir().join(format!(
            "nia-configure-cache-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        std::fs::create_dir_all(&root).unwrap();
        let cache = ConfigureCache { root: root.clone() };
        let mut manifest = ConfigureManifest::new(ObservationPolicy::Poison);
        manifest.push(
            ConfigureObservation::new("build.nia", ObservationKind::FileContents, [4; 32]).unwrap(),
        );
        let identity = ConfigureIdentity::new("runner", "invocation", &manifest);
        cache.publish(&identity, &manifest).unwrap();
        assert_eq!(cache.lookup(&identity).unwrap(), Some(manifest));
        let path = cache.path(&identity);
        let mut bytes = std::fs::read(&path).unwrap();
        *bytes.last_mut().unwrap() ^= 1;
        std::fs::write(&path, bytes).unwrap();
        assert_eq!(cache.lookup(&identity).unwrap(), None);
        assert!(!path.exists());
        std::fs::remove_dir_all(root).unwrap();
    }
}
