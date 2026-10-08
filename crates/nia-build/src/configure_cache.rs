// SPDX-License-Identifier: GPL-3.0-or-later
//! Persistent cache records for validated configuration identities.

use std::{fs, io, path::PathBuf};

use nia_compat::formats;

use crate::{BuildInvocation, ConfigureManifest, ObservationCodecError, ObservationValidation};

const MAGIC: &[u8; 8] = b"NIACREC\0";
const HEADER_BYTES: usize = 8 + 4 + 4 + 4 + 32;
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

/// A cached configuration witness containing the canonical frozen plan bytes
/// and the observation manifest that produced them.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ConfigureRecord {
    /// Canonical encoded `BuildPlan` bytes. These are decoded and frozen again
    /// by the caller before execution.
    pub plan: Vec<u8>,
    /// Observations declared while configuring the plan.
    pub manifest: ConfigureManifest,
}

/// Builds the invocation portion of the configure identity from typed policy fields.
/// Fields use length-prefixed UTF-8 values and explicit option tags, so display
/// formatting and delimiter characters cannot change the identity contract.
pub fn invocation_identity(invocation: &BuildInvocation) -> String {
    let mut encoded = Vec::new();
    encoded.extend_from_slice(b"nia.build.invocation\0\x01");
    write_identity_field(&mut encoded, &invocation.package_source_fingerprint);
    write_identity_field(
        &mut encoded,
        invocation.toolchain.host_target().name().as_bytes(),
    );
    write_identity_field(
        &mut encoded,
        invocation.toolchain.artifact_target().name().as_bytes(),
    );
    write_identity_field(&mut encoded, format!("{:?}", invocation.profile).as_bytes());
    write_identity_field(
        &mut encoded,
        format!("{:?}", invocation.optimization).as_bytes(),
    );
    write_identity_field(
        &mut encoded,
        format!("{:?}", invocation.link_time_optimization).as_bytes(),
    );
    write_identity_field(
        &mut encoded,
        format!("{:?}", invocation.compilation_mode).as_bytes(),
    );
    write_identity_field(&mut encoded, format!("{:?}", invocation.step).as_bytes());
    write_optional_identity_field(&mut encoded, invocation.test_filter.as_deref());
    encoded.push(u8::from(invocation.test_list));
    encoded.push(u8::from(invocation.test_fail_fast));
    blake3::hash(&encoded).to_hex().to_string()
}

fn write_identity_field(encoded: &mut Vec<u8>, value: &[u8]) {
    encoded.extend_from_slice(&(value.len() as u64).to_le_bytes());
    encoded.extend_from_slice(value);
}

fn write_optional_identity_field(encoded: &mut Vec<u8>, value: Option<&str>) {
    match value {
        Some(value) => {
            encoded.push(1);
            write_identity_field(encoded, value.as_bytes());
        }
        None => encoded.push(0),
    }
}

/// Returns the canonical identity of a frozen plan for configure diagnostics.
pub fn plan_identity(plan: &nia_build_plan::BuildPlan) -> String {
    let bytes = plan.encode().unwrap_or_default();
    blake3::hash(&bytes).to_hex().to_string()
}

/// Result of consulting the configure cache.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ConfigureCacheStatus {
    /// No valid record exists for the requested identity.
    Miss,
    /// A valid record was found and can be used as an observation witness.
    Hit,
}

/// Reason a configure-cache record cannot be reused for the current checkout.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ConfigureCacheMissReason {
    /// No record exists for the exact runner/invocation/observation key.
    NotFound,
    /// A persisted observation no longer matches the filesystem.
    ObservationChanged { path: PathBuf },
    /// A persisted observation path is unavailable.
    ObservationMissing { path: PathBuf },
    /// The runner did not provide reusable, bounded observations.
    Poisoned,
}

/// Validated configure-cache lookup result.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ConfigureCacheLookup {
    /// A record whose observations match the current package root.
    Hit(ConfigureRecord),
    /// The record cannot be used for this invocation.
    Miss(ConfigureCacheMissReason),
}

/// Reports whether a cache record exists for the runner/invocation pair,
/// without treating an unknown observation set as a reusable plan.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ConfigureCacheCandidate {
    None,
    Present,
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
        record: &ConfigureRecord,
    ) -> io::Result<()> {
        let manifest = record
            .manifest
            .encode()
            .map_err(|_| io::Error::other("configure manifest too large"))?;
        let plan_len = u32::try_from(record.plan.len())
            .map_err(|_| io::Error::other("configure plan too large"))?;
        let manifest_len = u32::try_from(manifest.len())
            .map_err(|_| io::Error::other("configure manifest too large"))?;
        let mut bytes = Vec::with_capacity(8 + 4 + 4 + 32 + 4 + record.plan.len() + manifest.len());
        bytes.extend_from_slice(MAGIC);
        bytes.extend_from_slice(&1u32.to_le_bytes());
        bytes.extend_from_slice(&plan_len.to_le_bytes());
        bytes.extend_from_slice(&manifest_len.to_le_bytes());
        bytes.extend_from_slice(&blake3::hash(&record.plan).as_bytes()[..]);
        bytes.extend_from_slice(&record.plan);
        bytes.extend_from_slice(&manifest);
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
    pub fn lookup(&self, identity: &ConfigureIdentity) -> io::Result<Option<ConfigureRecord>> {
        let path = self.path(identity);
        let bytes = match fs::read(&path) {
            Ok(bytes) => bytes,
            Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(None),
            Err(error) => return Err(error),
        };
        if bytes.len() > MAX_RECORD_BYTES
            || bytes.len() < HEADER_BYTES
            || &bytes[..8] != MAGIC
            || u32::from_le_bytes(bytes[8..12].try_into().unwrap()) != 1
        {
            let _ = fs::remove_file(&path);
            return Ok(None);
        }
        let plan_len = u32::from_le_bytes(bytes[12..16].try_into().unwrap()) as usize;
        let manifest_len = u32::from_le_bytes(bytes[16..20].try_into().unwrap()) as usize;
        let payload_start = 52usize;
        let plan_end = payload_start
            .checked_add(plan_len)
            .ok_or_else(|| io::Error::other("configure record overflow"))?;
        let manifest_end = plan_end
            .checked_add(manifest_len)
            .ok_or_else(|| io::Error::other("configure record overflow"))?;
        if manifest_end != bytes.len()
            || bytes[20..52] != *blake3::hash(&bytes[payload_start..plan_end]).as_bytes()
        {
            let _ = fs::remove_file(&path);
            return Ok(None);
        }
        match ConfigureManifest::decode(&bytes[plan_end..manifest_end]) {
            Ok(manifest) => Ok(Some(ConfigureRecord {
                plan: bytes[payload_start..plan_end].to_vec(),
                manifest,
            })),
            Err(ObservationCodecError::Invalid | ObservationCodecError::TooLarge) => {
                let _ = fs::remove_file(&path);
                Ok(None)
            }
        }
    }

    /// Publishes a plan/manifest witness only after the plan has been decoded
    /// and semantically frozen by the caller.
    pub fn publish_plan(
        &self,
        identity: &ConfigureIdentity,
        plan: &nia_build_plan::BuildPlan,
        manifest: &ConfigureManifest,
    ) -> io::Result<()> {
        let plan = plan
            .encode()
            .map_err(|_| io::Error::other("configure plan encoding failed"))?;
        self.publish(
            identity,
            &ConfigureRecord {
                plan,
                manifest: manifest.clone(),
            },
        )
    }

    /// Decodes the cached plan through the registered semantic validator.
    pub fn decode_plan(
        record: &ConfigureRecord,
    ) -> Result<nia_build_plan::BuildPlan, nia_build_plan::PlanCodecError> {
        nia_build_plan::decode_build_plan(&record.plan)
    }

    /// Publishes a decoded runner draft as a configure witness.
    pub fn publish_draft(
        &self,
        identity: &ConfigureIdentity,
        draft: &nia_build_plan::BuildDraft,
    ) -> io::Result<()> {
        let manifest = if draft.observations.is_empty() {
            ConfigureManifest::new(crate::ObservationPolicy::Poison)
        } else {
            ConfigureManifest::decode(&draft.observations).map_err(|_| {
                io::Error::new(io::ErrorKind::InvalidData, "invalid draft observations")
            })?
        };
        self.publish_plan(identity, &draft.plan, &manifest)
    }

    /// Returns whether a manifest record exists without exposing it as an
    /// execution input. The caller must still configure and validate a plan.
    pub fn status(&self, identity: &ConfigureIdentity) -> io::Result<ConfigureCacheStatus> {
        Ok(if self.lookup(identity)?.is_some() {
            ConfigureCacheStatus::Hit
        } else {
            ConfigureCacheStatus::Miss
        })
    }

    /// Looks up a record and validates its observation facts against `root`.
    pub fn lookup_validated(
        &self,
        identity: &ConfigureIdentity,
        root: &std::path::Path,
    ) -> io::Result<ConfigureCacheLookup> {
        let Some(record) = self.lookup(identity)? else {
            return Ok(ConfigureCacheLookup::Miss(
                ConfigureCacheMissReason::NotFound,
            ));
        };
        let result = match record.manifest.validate(root) {
            ObservationValidation::Match => ConfigureCacheLookup::Hit(record),
            ObservationValidation::Changed { path, .. } => {
                ConfigureCacheLookup::Miss(ConfigureCacheMissReason::ObservationChanged { path })
            }
            ObservationValidation::Missing { path, .. } => {
                ConfigureCacheLookup::Miss(ConfigureCacheMissReason::ObservationMissing { path })
            }
        };
        Ok(result)
    }

    /// Finds a record for the runner/invocation domains and validates its
    /// observations against the current package root.
    ///
    /// The observation fingerprint is not known before configuration, so this
    /// scans only the bounded configure namespace and accepts an entry whose
    /// manifest recomputes to the exact content-addressed key. Poisoned or
    /// empty manifests never become alternate execution inputs.
    pub fn lookup_reusable(
        &self,
        runner: &str,
        invocation: &str,
        root: &std::path::Path,
    ) -> io::Result<ConfigureCacheLookup> {
        let namespace_root = self.root.join(
            formats::CONFIGURE_MANIFEST
                .release_compatibility
                .to_string(),
        );
        let entries = match fs::read_dir(&namespace_root) {
            Ok(entries) => entries,
            Err(error) if error.kind() == io::ErrorKind::NotFound => {
                return Ok(ConfigureCacheLookup::Miss(
                    ConfigureCacheMissReason::NotFound,
                ));
            }
            Err(error) => return Err(error),
        };
        let mut poisoned = false;
        for entry in entries {
            let path = entry?.path();
            if path.extension().and_then(|value| value.to_str()) != Some("manifest") {
                continue;
            }
            let bytes = match fs::read(&path) {
                Ok(bytes) => bytes,
                Err(_) => continue,
            };
            let Some(record_view) = decode_record(&bytes) else {
                continue;
            };
            let manifest = match ConfigureManifest::decode(record_view.manifest) {
                Ok(manifest) => manifest,
                Err(_) => continue,
            };
            if manifest.policy() == crate::ObservationPolicy::Poison
                || manifest.observations().is_empty()
            {
                poisoned = true;
                continue;
            }
            let candidate = ConfigureIdentity::new(runner, invocation, &manifest);
            if candidate.key()
                != path
                    .file_stem()
                    .and_then(|value| value.to_str())
                    .unwrap_or("")
            {
                continue;
            }
            let Some(record) = self.lookup(&candidate)? else {
                continue;
            };
            return Ok(match record.manifest.validate(root) {
                crate::ObservationValidation::Match => ConfigureCacheLookup::Hit(record),
                crate::ObservationValidation::Changed { path, .. } => {
                    ConfigureCacheLookup::Miss(ConfigureCacheMissReason::ObservationChanged {
                        path,
                    })
                }
                crate::ObservationValidation::Missing { path, .. } => {
                    ConfigureCacheLookup::Miss(ConfigureCacheMissReason::ObservationMissing {
                        path,
                    })
                }
            });
        }
        Ok(ConfigureCacheLookup::Miss(if poisoned {
            ConfigureCacheMissReason::Poisoned
        } else {
            ConfigureCacheMissReason::NotFound
        }))
    }

    /// Finds whether any valid record belongs to the same runner and
    /// invocation domains. Observation fingerprints remain the final lookup
    /// key and are never inferred from this candidate result.
    pub fn candidate_status(
        &self,
        runner: &str,
        invocation: &str,
    ) -> io::Result<ConfigureCacheCandidate> {
        let root = self.root.join(
            formats::CONFIGURE_MANIFEST
                .release_compatibility
                .to_string(),
        );
        let entries = match fs::read_dir(root) {
            Ok(entries) => entries,
            Err(error) if error.kind() == io::ErrorKind::NotFound => {
                return Ok(ConfigureCacheCandidate::None);
            }
            Err(error) => return Err(error),
        };
        for entry in entries {
            let path = entry?.path();
            if path.extension().and_then(|value| value.to_str()) != Some("manifest") {
                continue;
            }
            let bytes = match fs::read(&path) {
                Ok(bytes) => bytes,
                Err(_) => continue,
            };
            let Some(record) = decode_record(&bytes) else {
                continue;
            };
            let manifest = match ConfigureManifest::decode(record.manifest) {
                Ok(manifest) => manifest,
                Err(_) => continue,
            };
            let candidate = ConfigureIdentity::new(runner, invocation, &manifest);
            if candidate.key()
                == path
                    .file_stem()
                    .and_then(|value| value.to_str())
                    .unwrap_or("")
            {
                return Ok(ConfigureCacheCandidate::Present);
            }
        }
        Ok(ConfigureCacheCandidate::None)
    }
}

fn decode_record(bytes: &[u8]) -> Option<ConfigureRecordView<'_>> {
    if bytes.len() < HEADER_BYTES || &bytes[..8] != MAGIC {
        return None;
    }
    let plan_len = u32::from_le_bytes(bytes[12..16].try_into().ok()?) as usize;
    let manifest_len = u32::from_le_bytes(bytes[16..20].try_into().ok()?) as usize;
    let plan_end = 52usize.checked_add(plan_len)?;
    let end = plan_end.checked_add(manifest_len)?;
    if end != bytes.len() || bytes[20..52] != *blake3::hash(&bytes[52..plan_end]).as_bytes() {
        return None;
    }
    Some(ConfigureRecordView {
        manifest: &bytes[plan_end..end],
    })
}

struct ConfigureRecordView<'a> {
    manifest: &'a [u8],
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
        let record = ConfigureRecord {
            plan: b"validated-plan".to_vec(),
            manifest: manifest.clone(),
        };
        cache.publish(&identity, &record).unwrap();
        assert_eq!(cache.lookup(&identity).unwrap(), Some(record));
        let path = cache.path(&identity);
        let mut bytes = std::fs::read(&path).unwrap();
        *bytes.last_mut().unwrap() ^= 1;
        std::fs::write(&path, bytes).unwrap();
        assert_eq!(cache.lookup(&identity).unwrap(), None);
        assert!(!path.exists());
        std::fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn status_distinguishes_a_valid_record_from_a_miss() {
        let root = std::env::temp_dir().join(format!(
            "nia-configure-status-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        std::fs::create_dir_all(&root).unwrap();
        let cache = ConfigureCache { root: root.clone() };
        let manifest = ConfigureManifest::new(ObservationPolicy::Poison);
        let identity = ConfigureIdentity::new("runner", "invocation", &manifest);
        assert_eq!(cache.status(&identity).unwrap(), ConfigureCacheStatus::Miss);
        cache
            .publish(
                &identity,
                &ConfigureRecord {
                    plan: b"validated-plan".to_vec(),
                    manifest: manifest.clone(),
                },
            )
            .unwrap();
        assert_eq!(cache.status(&identity).unwrap(), ConfigureCacheStatus::Hit);
        std::fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn validated_lookup_reports_observation_changes() {
        let checkout = std::env::temp_dir().join(format!(
            "nia-configure-validated-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        std::fs::create_dir_all(&checkout).unwrap();
        std::fs::write(checkout.join("input"), b"before").unwrap();
        let observation = crate::observe_file_contents(&checkout, "input").unwrap();
        let mut manifest = ConfigureManifest::new(ObservationPolicy::Disallow);
        manifest.push(observation);
        let cache_root = checkout.join("cache");
        let cache = ConfigureCache { root: cache_root };
        let identity = ConfigureIdentity::new("runner", "invocation", &manifest);
        cache
            .publish(
                &identity,
                &ConfigureRecord {
                    plan: b"plan".to_vec(),
                    manifest,
                },
            )
            .unwrap();
        assert!(matches!(
            cache.lookup_validated(&identity, &checkout).unwrap(),
            ConfigureCacheLookup::Hit(_)
        ));
        std::fs::write(checkout.join("input"), b"after").unwrap();
        assert!(matches!(
            cache.lookup_validated(&identity, &checkout).unwrap(),
            ConfigureCacheLookup::Miss(ConfigureCacheMissReason::ObservationChanged { .. })
        ));
        std::fs::remove_dir_all(checkout).unwrap();
    }

    #[test]
    fn reusable_lookup_rejects_poisoned_records() {
        let root = std::env::temp_dir().join(format!(
            "nia-configure-poisoned-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        std::fs::create_dir_all(&root).unwrap();
        let cache = ConfigureCache {
            root: root.join("cache"),
        };
        let manifest = ConfigureManifest::new(ObservationPolicy::Poison);
        let identity = ConfigureIdentity::new("runner", "invocation", &manifest);
        cache
            .publish(
                &identity,
                &ConfigureRecord {
                    plan: b"plan".to_vec(),
                    manifest,
                },
            )
            .unwrap();
        assert!(matches!(
            cache
                .lookup_reusable("runner", "invocation", &root)
                .unwrap(),
            ConfigureCacheLookup::Miss(ConfigureCacheMissReason::Poisoned)
        ));
        std::fs::remove_dir_all(root).unwrap();
    }
}
