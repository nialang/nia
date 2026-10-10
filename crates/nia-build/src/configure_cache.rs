// SPDX-License-Identifier: GPL-3.0-or-later
//! Persistent cache records for validated configuration identities.

use std::{
    fs,
    io::{self, Read},
    path::{Path, PathBuf},
};

use nia_compat::formats;
use nia_query::FingerprintDomain;

use crate::{BuildInvocation, ConfigureManifest, ObservationValidation};

const HEADER_BYTES: usize = 8 + 4 + 4 + 4 + 32;
const MAX_RECORD_BYTES: usize = 16 * 1024 * 1024;
const INVOCATION_IDENTITY_DOMAIN: FingerprintDomain =
    FingerprintDomain::new("nia.build.invocation");
const CONFIGURE_CACHE_DOMAIN: FingerprintDomain =
    FingerprintDomain::new("nia.build.configure-cache");

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
    encoded.extend_from_slice(INVOCATION_IDENTITY_DOMAIN.as_str().as_bytes());
    encoded.extend_from_slice(b"\0\x01");
    write_identity_field(&mut encoded, &invocation.package_source_fingerprint);
    write_identity_field(
        &mut encoded,
        invocation.toolchain.host_target().name().as_bytes(),
    );
    write_identity_field(
        &mut encoded,
        invocation.toolchain.artifact_target().name().as_bytes(),
    );
    encoded.push(profile_tag(invocation.profile));
    encoded.push(optimization_tag(invocation.optimization));
    encoded.push(link_time_optimization_tag(
        invocation.link_time_optimization,
    ));
    encoded.push(compilation_mode_tag(invocation.compilation_mode));
    match &invocation.step {
        crate::BuildStepSelection::Default => encoded.push(0),
        crate::BuildStepSelection::Tests => encoded.push(1),
        crate::BuildStepSelection::Named(name) => {
            encoded.push(2);
            write_identity_field(&mut encoded, name.as_bytes());
        }
    }
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

fn optimization_tag(mode: nia_build_plan::OptimizationMode) -> u8 {
    match mode {
        nia_build_plan::OptimizationMode::O0 => 0,
        nia_build_plan::OptimizationMode::O1 => 1,
        nia_build_plan::OptimizationMode::O2 => 2,
        nia_build_plan::OptimizationMode::O3 => 3,
        nia_build_plan::OptimizationMode::Os => 4,
        nia_build_plan::OptimizationMode::Oz => 5,
    }
}

fn link_time_optimization_tag(mode: nia_driver::LinkTimeOptimization) -> u8 {
    match mode {
        nia_driver::LinkTimeOptimization::Off => 0,
        nia_driver::LinkTimeOptimization::Thin => 1,
        nia_driver::LinkTimeOptimization::Full => 2,
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

impl std::fmt::Display for ConfigureCacheMissReason {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::NotFound => formatter.write_str("not found"),
            Self::ObservationChanged { path } => {
                write!(formatter, "observation changed: {}", path.display())
            }
            Self::ObservationMissing { path } => {
                write!(formatter, "observation missing: {}", path.display())
            }
            Self::Poisoned => formatter.write_str("poisoned observation policy"),
        }
    }
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
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ConfigureCacheCandidate {
    /// No matching record was found for the runner and invocation identity.
    None,
    /// At least one record matched the runner and invocation identity.
    Present,
    /// A matching record was found but could not be reused.
    Miss(ConfigureCacheMissReason),
}

impl std::fmt::Display for ConfigureCacheCandidate {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::None => formatter.write_str("none"),
            Self::Present => formatter.write_str("present"),
            Self::Miss(reason) => write!(formatter, "miss: {reason}"),
        }
    }
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
        hasher.update(CONFIGURE_CACHE_DOMAIN.as_str().as_bytes());
        hasher.update(b"\0");
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
            .join(formats::CONFIGURE_MANIFEST.protocol_epoch.to_string())
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
        let total = HEADER_BYTES
            .checked_add(record.plan.len())
            .and_then(|size| size.checked_add(manifest.len()))
            .filter(|size| *size <= MAX_RECORD_BYTES)
            .ok_or_else(|| io::Error::other("configure record too large"))?;
        let mut bytes = Vec::with_capacity(total);
        bytes.extend_from_slice(formats::CONFIGURE_RECORD.magic);
        bytes.extend_from_slice(&formats::CONFIGURE_RECORD.protocol_epoch.to_le_bytes());
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
        read_record(&path)
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
        let namespace_root = self
            .root
            .join(formats::CONFIGURE_MANIFEST.protocol_epoch.to_string());
        let entries = match record_paths(&namespace_root) {
            Ok(entries) => entries,
            Err(error) if error.kind() == io::ErrorKind::NotFound => {
                return Ok(ConfigureCacheLookup::Miss(
                    ConfigureCacheMissReason::NotFound,
                ));
            }
            Err(error) => return Err(error),
        };
        let mut poisoned = false;
        let mut changed = None;
        let mut missing = None;
        for path in entries {
            let record = match read_record(&path) {
                Ok(Some(record)) => record,
                _ => continue,
            };
            let candidate = ConfigureIdentity::new(runner, invocation, &record.manifest);
            if candidate.key()
                != path
                    .file_stem()
                    .and_then(|value| value.to_str())
                    .unwrap_or("")
            {
                continue;
            }
            if record.manifest.policy() == crate::ObservationPolicy::Poison
                || record.manifest.observations().is_empty()
            {
                poisoned = true;
                continue;
            }
            match record.manifest.validate(root) {
                crate::ObservationValidation::Match => {
                    return Ok(ConfigureCacheLookup::Hit(record));
                }
                crate::ObservationValidation::Changed { path, .. } => {
                    changed.get_or_insert(path);
                }
                crate::ObservationValidation::Missing { path, .. } => {
                    missing.get_or_insert(path);
                }
            }
        }
        Ok(ConfigureCacheLookup::Miss(if let Some(path) = changed {
            ConfigureCacheMissReason::ObservationChanged { path }
        } else if let Some(path) = missing {
            ConfigureCacheMissReason::ObservationMissing { path }
        } else if poisoned {
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
        let root = self
            .root
            .join(formats::CONFIGURE_MANIFEST.protocol_epoch.to_string());
        let entries = match record_paths(&root) {
            Ok(entries) => entries,
            Err(error) if error.kind() == io::ErrorKind::NotFound => {
                return Ok(ConfigureCacheCandidate::None);
            }
            Err(error) => return Err(error),
        };
        for path in entries {
            let record = match read_record(&path) {
                Ok(Some(record)) => record,
                _ => continue,
            };
            let candidate = ConfigureIdentity::new(runner, invocation, &record.manifest);
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

fn record_paths(root: &Path) -> io::Result<Vec<PathBuf>> {
    let mut paths = Vec::new();
    for entry in fs::read_dir(root)? {
        let entry = entry?;
        if entry.file_type()?.is_file()
            && entry.path().extension().and_then(|value| value.to_str()) == Some("manifest")
        {
            paths.push(entry.path());
        }
    }
    paths.sort();
    Ok(paths)
}

fn read_record(path: &Path) -> io::Result<Option<ConfigureRecord>> {
    let file = match fs::File::open(path) {
        Ok(file) => file,
        Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(None),
        Err(error) => return Err(error),
    };
    let metadata = file.metadata()?;
    if !metadata.is_file() || metadata.len() > MAX_RECORD_BYTES as u64 {
        let _ = fs::remove_file(path);
        return Ok(None);
    }
    let mut bytes = Vec::new();
    file.take(MAX_RECORD_BYTES as u64 + 1)
        .read_to_end(&mut bytes)?;
    let record = decode_record(&bytes);
    if record.is_none() {
        let _ = fs::remove_file(path);
    }
    Ok(record)
}

fn decode_record(bytes: &[u8]) -> Option<ConfigureRecord> {
    if bytes.len() < HEADER_BYTES
        || bytes.len() > MAX_RECORD_BYTES
        || &bytes[..8] != formats::CONFIGURE_RECORD.magic
        || bytes[8..12] != formats::CONFIGURE_RECORD.protocol_epoch.to_le_bytes()
    {
        return None;
    }
    let plan_len = u32::from_le_bytes(bytes[12..16].try_into().ok()?) as usize;
    let manifest_len = u32::from_le_bytes(bytes[16..20].try_into().ok()?) as usize;
    let plan_end = HEADER_BYTES.checked_add(plan_len)?;
    let end = plan_end.checked_add(manifest_len)?;
    if end != bytes.len()
        || bytes[20..HEADER_BYTES] != *blake3::hash(bytes.get(HEADER_BYTES..plan_end)?).as_bytes()
    {
        return None;
    }
    Some(ConfigureRecord {
        plan: bytes[HEADER_BYTES..plan_end].to_vec(),
        manifest: ConfigureManifest::decode(&bytes[plan_end..end]).ok()?,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{ConfigureObservation, ObservationKind, ObservationPolicy};

    #[test]
    fn identity_changes_with_invocation_and_observations() {
        let mut manifest = ConfigureManifest::new(ObservationPolicy::Poison);
        manifest
            .push(
                ConfigureObservation::new("input", ObservationKind::FileContents, [1; 32]).unwrap(),
            )
            .unwrap();
        let first = ConfigureIdentity::new("runner-a", "target-a", &manifest).key();
        let second = ConfigureIdentity::new("runner-a", "target-b", &manifest).key();
        assert_ne!(first, second);
        manifest
            .push(
                ConfigureObservation::new("other", ObservationKind::FileContents, [1; 32]).unwrap(),
            )
            .unwrap();
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
        manifest
            .push(
                ConfigureObservation::new("build.nia", ObservationKind::FileContents, [4; 32])
                    .unwrap(),
            )
            .unwrap();
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
        manifest.push(observation).unwrap();
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
    fn presence_observations_invalidate_create_and_remove_transitions() {
        let checkout = std::env::temp_dir().join(format!(
            "nia-configure-presence-cache-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        std::fs::create_dir_all(&checkout).unwrap();
        let cache = ConfigureCache {
            root: checkout.join("cache"),
        };

        let absent = crate::observe_file_presence(&checkout, "optional/input").unwrap();
        let mut absent_manifest = ConfigureManifest::new(ObservationPolicy::Disallow);
        absent_manifest.push(absent).unwrap();
        let absent_identity = ConfigureIdentity::new("runner", "invocation", &absent_manifest);
        cache
            .publish(
                &absent_identity,
                &ConfigureRecord {
                    plan: b"absent".to_vec(),
                    manifest: absent_manifest,
                },
            )
            .unwrap();
        assert!(matches!(
            cache.lookup_validated(&absent_identity, &checkout).unwrap(),
            ConfigureCacheLookup::Hit(_)
        ));

        std::fs::create_dir_all(checkout.join("optional")).unwrap();
        std::fs::write(checkout.join("optional/input"), b"present").unwrap();
        assert!(matches!(
            cache.lookup_validated(&absent_identity, &checkout).unwrap(),
            ConfigureCacheLookup::Miss(ConfigureCacheMissReason::ObservationChanged { .. })
        ));
        std::fs::remove_file(checkout.join("optional/input")).unwrap();
        assert!(matches!(
            cache.lookup_validated(&absent_identity, &checkout).unwrap(),
            ConfigureCacheLookup::Hit(_)
        ));

        std::fs::write(checkout.join("optional/input"), b"present").unwrap();
        let present = crate::observe_file_presence(&checkout, "optional/input").unwrap();
        let mut present_manifest = ConfigureManifest::new(ObservationPolicy::Disallow);
        present_manifest.push(present).unwrap();
        let present_identity = ConfigureIdentity::new("runner", "invocation", &present_manifest);
        cache
            .publish(
                &present_identity,
                &ConfigureRecord {
                    plan: b"present".to_vec(),
                    manifest: present_manifest,
                },
            )
            .unwrap();
        std::fs::remove_file(checkout.join("optional/input")).unwrap();
        assert!(matches!(
            cache
                .lookup_validated(&present_identity, &checkout)
                .unwrap(),
            ConfigureCacheLookup::Miss(ConfigureCacheMissReason::ObservationChanged { .. })
        ));
        std::fs::remove_dir_all(checkout).unwrap();
    }

    #[test]
    fn reusable_lookup_skips_changed_history_and_finds_valid_candidate() {
        let checkout = std::env::temp_dir().join(format!(
            "nia-configure-candidates-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        std::fs::create_dir_all(&checkout).unwrap();
        std::fs::create_dir_all(checkout.join("inputs")).unwrap();
        std::fs::write(checkout.join("inputs/changed"), b"before").unwrap();
        std::fs::write(checkout.join("inputs/valid"), b"stable").unwrap();
        let cache = ConfigureCache {
            root: checkout.join("cache"),
        };

        let changed_observation =
            crate::observe_file_contents(&checkout, "inputs/changed").unwrap();
        let mut changed_manifest = ConfigureManifest::new(ObservationPolicy::Disallow);
        changed_manifest.push(changed_observation).unwrap();
        let changed_identity = ConfigureIdentity::new("runner", "invocation", &changed_manifest);
        cache
            .publish(
                &changed_identity,
                &ConfigureRecord {
                    plan: b"changed".to_vec(),
                    manifest: changed_manifest,
                },
            )
            .unwrap();

        let valid_observation = crate::observe_file_contents(&checkout, "inputs/valid").unwrap();
        let mut valid_manifest = ConfigureManifest::new(ObservationPolicy::Disallow);
        valid_manifest.push(valid_observation).unwrap();
        let valid_identity = ConfigureIdentity::new("runner", "invocation", &valid_manifest);
        cache
            .publish(
                &valid_identity,
                &ConfigureRecord {
                    plan: b"valid".to_vec(),
                    manifest: valid_manifest,
                },
            )
            .unwrap();

        std::fs::write(checkout.join("inputs/changed"), b"after").unwrap();
        let lookup = cache
            .lookup_reusable("runner", "invocation", &checkout)
            .unwrap();
        assert!(matches!(
            lookup,
            ConfigureCacheLookup::Hit(ConfigureRecord { plan, .. }) if plan == b"valid"
        ));
        std::fs::remove_dir_all(checkout).unwrap();
    }

    #[test]
    fn environment_observations_invalidate_absent_empty_and_changed_values() {
        use nia_test_support::CommandExt;

        let checkout = nia_test_support::test_dir("configure-environment");
        for value in [None, Some(""), Some("enabled"), Some("changed"), None] {
            let mut command = std::process::Command::new(std::env::current_exe().unwrap());
            command
                .args([
                    "--exact",
                    "configure_cache::tests::environment_cache_worker",
                    "--ignored",
                    "--nocapture",
                ])
                .env("NIA_TEST_CONFIGURE_ENV_ROOT", checkout.as_ref() as &Path);
            match value {
                Some(value) => {
                    command.env("NIA_TEST_CONFIGURE_ENV", value);
                }
                None => {
                    command.env_remove("NIA_TEST_CONFIGURE_ENV");
                }
            }
            let output = command.output_timeout_for_runtime("configure environment cache worker");
            assert!(
                output.status.success(),
                "{}",
                String::from_utf8_lossy(&output.stderr)
            );
        }
    }

    #[test]
    #[ignore = "spawned with isolated environment by the cache test"]
    fn environment_cache_worker() {
        let checkout = PathBuf::from(std::env::var_os("NIA_TEST_CONFIGURE_ENV_ROOT").unwrap());
        let cache = ConfigureCache {
            root: checkout.join("cache"),
        };
        let name = "NIA_TEST_CONFIGURE_ENV";
        let current = crate::observe_environment(name).unwrap();
        if let Some(previous) = read_record(&checkout.join("previous.manifest")).unwrap() {
            let identity = ConfigureIdentity::new("runner", "invocation", &previous.manifest);
            let expected = if previous.manifest.observations()[0] == current {
                ConfigureCacheLookup::Hit(previous.clone())
            } else {
                ConfigureCacheLookup::Miss(ConfigureCacheMissReason::ObservationChanged {
                    path: PathBuf::from(name),
                })
            };
            assert_eq!(
                cache.lookup_validated(&identity, &checkout).unwrap(),
                expected
            );
        }
        let mut manifest = ConfigureManifest::new(ObservationPolicy::Disallow);
        manifest.push(current).unwrap();
        let identity = ConfigureIdentity::new("runner", "invocation", &manifest);
        let record = ConfigureRecord {
            plan: b"plan".to_vec(),
            manifest,
        };
        cache.publish(&identity, &record).unwrap();
        assert_eq!(
            cache.lookup_validated(&identity, &checkout).unwrap(),
            ConfigureCacheLookup::Hit(record)
        );
        fs::copy(cache.path(&identity), checkout.join("previous.manifest")).unwrap();
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

    #[test]
    fn configure_miss_reasons_have_stable_text() {
        assert_eq!(
            ConfigureCacheMissReason::ObservationChanged {
                path: PathBuf::from("inputs/options"),
            }
            .to_string(),
            "observation changed: inputs/options"
        );
        assert_eq!(
            ConfigureCacheMissReason::Poisoned.to_string(),
            "poisoned observation policy"
        );
        assert_eq!(
            ConfigureCacheCandidate::Miss(ConfigureCacheMissReason::ObservationMissing {
                path: PathBuf::from("inputs/options"),
            })
            .to_string(),
            "miss: observation missing: inputs/options"
        );
    }
}
