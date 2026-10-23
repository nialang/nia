// SPDX-License-Identifier: GPL-3.0-or-later
//! Relocation-independent package identities and stable compiler type graphs.
//!
//! A [`PackageId`] names a package by namespace, name, and version, and the
//! toolchain standard library has a canonical identity of its own. A
//! [`PackageSourceIdentity`] fingerprints only the declared package content:
//! files are selected by [`PackageSourceSelection`], recorded by normalized
//! relative path, and hashed with BLAKE3, so moving a checkout does not change
//! the identity. Package manifests and their dependencies resolve into a
//! bounded [`PackageGraph`] without loading source modules; a
//! [`DependencyScope`] policy decides which declared edges are selected, and
//! [`PackageGraphReport`] explains those decisions.
//!
//! Stable module and definition identities, and the canonical
//! [`StableTypeGraph`], are the session-independent forms that compiler-owned
//! sections embed for cross-package linkage and caches. Their encoders and
//! decoders are strict and bounded, and every payload is tied to the protocol
//! epoch.
//!
//! The version, registry, lockfile, and source-identity contract is specified
//! in this crate's `README.md`.

use std::{
    cmp::Ordering,
    collections::{BTreeMap, BTreeSet},
    ffi::OsStr,
    fmt,
    fs::File,
    io::{self, Cursor, Read},
    path::{Path, PathBuf},
    str::FromStr,
};

use cap_std::fs::{Dir, OpenOptions as CapOpenOptions};
use nia_compat::{COMPILER_VERSION, PROTOCOL_EPOCH as NIA_PROTOCOL_EPOCH, toolchain};
use nia_git::checkout_source;
use nia_query::FingerprintDomain;
use registry::{
    RegistryFetch, open_cache_directory, open_cache_root, sync_open_directory, write_atomic_path,
};
use semver::Version;
use serde::{Deserialize, Serialize};

mod registry;
pub use registry::{
    RegistryArchiveEntry, RegistryArchiveEntryKind, RegistryCache, RegistryClient,
    RegistryEndpoint, RegistryIndex, RegistryIndexPackage, decode_registry_archive,
    extract_registry_archive,
};

/// Protocol epoch for stable compiler-owned metadata.
pub const PROTOCOL_EPOCH: u32 = NIA_PROTOCOL_EPOCH;
/// Maximum accepted stable metadata payload size.
pub const MAX_STABLE_METADATA_BYTES: usize = 64 * 1024 * 1024;
const MAX_STRING_BYTES: usize = 16 * 1024 * 1024;
const MAX_ITEMS: usize = 1_000_000;
const MAX_SOURCE_FILES: usize = MAX_ITEMS;
const MAX_MANIFEST_BYTES: usize = 1024 * 1024;
const MAX_MANIFEST_ENTRIES: usize = 1024;
const MAX_DEFINITION_DEPTH: usize = 256;
const PACKAGE_GRAPH_DOMAIN: FingerprintDomain = FingerprintDomain::new("nia.toml.graph");
const PACKAGE_SOURCE_DOMAIN: FingerprintDomain = FingerprintDomain::new("nia.toml.source");
const TYPE_GRAPH_MAGIC: &[u8; 8] = b"NIATYP\0\0";
const TYPE_GRAPH_SCHEMA: u32 = PROTOCOL_EPOCH;

/// Relocation-independent identity of one package.
#[derive(Debug, Clone, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct PackageId {
    pub namespace: String,
    pub name: String,
    pub version: PackageVersion,
}

/// A parsed SemVer package release. Build metadata remains part of package
/// identity even though SemVer precedence ignores it.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct PackageVersion {
    parsed: Version,
    text: String,
}

/// A parsed SemVer requirement for selecting a package release.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PackageVersionRequirement(semver::VersionReq);

impl PackageVersionRequirement {
    /// Parses a standard SemVer requirement expression.
    pub fn parse(value: &str) -> Result<Self, semver::Error> {
        let trimmed = value.trim_start();
        if trimmed
            .chars()
            .next()
            .is_some_and(|character| character.is_ascii_digit())
            && !trimmed.contains(['*', 'x', 'X'])
        {
            // semver accepts a bare partial version as an implicit caret
            // requirement. Nia requires callers to choose that policy
            // explicitly, so preserve the parser's error type while rejecting
            // the shorthand at the metadata boundary.
            return Err(semver::VersionReq::parse("?").expect_err("invalid requirement"));
        }
        semver::VersionReq::parse(value).map(Self)
    }

    /// Returns whether the requirement admits a package version.
    pub fn matches(&self, version: &PackageVersion) -> bool {
        self.0.matches(&version.parsed)
    }
}

impl FromStr for PackageVersionRequirement {
    type Err = semver::Error;

    fn from_str(value: &str) -> Result<Self, Self::Err> {
        Self::parse(value)
    }
}

impl fmt::Display for PackageVersionRequirement {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        self.0.fmt(formatter)
    }
}

/// Registry package name used as the key for single-version resolution.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord)]
pub struct RegistryPackageName {
    pub namespace: String,
    pub name: String,
}

/// One registry constraint and the dependency path that introduced it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RegistryVersionConstraint {
    pub package: RegistryPackageName,
    pub requirement: PackageVersionRequirement,
    pub requiring_package: PackageId,
    pub dependency_chain: Vec<String>,
}

/// Selects one version per registry package from a complete candidate set.
/// Stable releases are preferred; prereleases are considered only when no
/// stable candidate satisfies every constraint.
pub fn solve_registry_versions(
    constraints: &[RegistryVersionConstraint],
    candidates: &[PackageId],
) -> io::Result<BTreeMap<RegistryPackageName, PackageId>> {
    let mut grouped = BTreeMap::<RegistryPackageName, Vec<&RegistryVersionConstraint>>::new();
    for constraint in constraints {
        grouped
            .entry(constraint.package.clone())
            .or_default()
            .push(constraint);
    }

    let mut selected = BTreeMap::new();
    for (package, package_constraints) in grouped {
        let matches_all = |candidate: &&PackageId| {
            candidate.namespace == package.namespace
                && candidate.name == package.name
                && package_constraints
                    .iter()
                    .all(|constraint| constraint.requirement.matches(&candidate.version))
        };
        let stable = candidates
            .iter()
            .filter(matches_all)
            .filter(|candidate| candidate.version.parsed.pre.is_empty())
            .max_by(|left, right| {
                left.version
                    .parsed
                    .cmp_precedence(&right.version.parsed)
                    .then_with(|| left.version.as_str().cmp(right.version.as_str()))
            });
        let prerelease = || {
            candidates.iter().filter(matches_all).max_by(|left, right| {
                left.version
                    .parsed
                    .cmp_precedence(&right.version.parsed)
                    .then_with(|| left.version.as_str().cmp(right.version.as_str()))
            })
        };
        if let Some(candidate) = stable.or_else(prerelease) {
            selected.insert(package, candidate.clone());
        } else {
            let mut details = package_constraints
                .iter()
                .map(|constraint| {
                    let chain = if constraint.dependency_chain.is_empty() {
                        constraint.requiring_package.canonical_text()
                    } else {
                        constraint.dependency_chain.join(" -> ")
                    };
                    format!(
                        "{} requires {} ({})",
                        constraint.requiring_package.canonical_text(),
                        constraint.requirement,
                        chain
                    )
                })
                .collect::<Vec<_>>();
            details.sort();
            details.dedup();
            return Err(io::Error::new(
                io::ErrorKind::InvalidData,
                format!(
                    "no single version of {}/{} satisfies all registry constraints: {}",
                    package.namespace,
                    package.name,
                    details.join("; ")
                ),
            ));
        }
    }
    Ok(selected)
}

impl PackageVersion {
    /// Parses a strict SemVer 2.0.0 release.
    pub fn parse(value: &str) -> Result<Self, semver::Error> {
        let parsed = Version::parse(value)?;
        Ok(Self {
            parsed,
            text: value.to_owned(),
        })
    }

    /// Returns the canonical SemVer text used in manifests and metadata.
    pub fn as_str(&self) -> &str {
        &self.text
    }
}

impl FromStr for PackageVersion {
    type Err = semver::Error;

    fn from_str(value: &str) -> Result<Self, Self::Err> {
        Self::parse(value)
    }
}

impl fmt::Display for PackageVersion {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(&self.text)
    }
}

impl Ord for PackageVersion {
    fn cmp(&self, other: &Self) -> Ordering {
        self.parsed
            .cmp(&other.parsed)
            .then_with(|| self.text.cmp(&other.text))
    }
}

impl PartialOrd for PackageVersion {
    fn partial_cmp(&self, other: &Self) -> Option<Ordering> {
        Some(self.cmp(other))
    }
}

impl PackageId {
    /// Returns the canonical text used at compiler identity boundaries.
    pub fn canonical_text(&self) -> String {
        format!(
            "{}/{}@{}",
            canonical_component(&self.namespace),
            canonical_component(&self.name),
            canonical_component(self.version.as_str())
        )
    }

    /// Returns the canonical identity of the toolchain standard library.
    pub fn standard_library() -> Self {
        Self {
            namespace: "nia".into(),
            name: "std".into(),
            version: PackageVersion::parse(&format!(
                "{}+std{}",
                COMPILER_VERSION,
                toolchain::STANDARD_LIBRARY
            ))
            .expect("compiler standard library identity is valid SemVer"),
        }
    }
}

fn canonical_component(value: &str) -> String {
    let mut canonical = String::with_capacity(value.len());
    for byte in value.bytes() {
        if matches!(byte, b'%' | b'/' | b'@' | 0..=0x1f | 0x7f..=0xff) {
            use std::fmt::Write as _;
            write!(canonical, "%{byte:02X}").expect("writing to a string cannot fail");
        } else {
            canonical.push(char::from(byte));
        }
    }
    canonical
}

/// Local package source identity used by build and package caches.
///
/// The identity covers only declared package content. Paths are stored as
/// normalized relative names, so moving a checkout does not change it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PackageSourceIdentity {
    /// Stable package identity independent of the checkout location.
    pub package: PackageId,
    /// BLAKE3 fingerprint of the canonical source manifest.
    pub source_fingerprint: [u8; 32],
    /// Files included in the source manifest, sorted by relative path.
    pub files: Vec<PackageSourceFile>,
}

/// One regular file included in a package source identity.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PackageSourceFile {
    /// Slash-separated path relative to the package root.
    pub path: String,
    /// File length in bytes.
    pub byte_len: u64,
    /// BLAKE3 content fingerprint.
    pub fingerprint: [u8; 32],
}

/// Local package source-selection rules applied before fingerprinting.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PackageSourceSelection {
    /// Relative directories included in the package content identity.
    pub source_roots: Vec<String>,
    /// Relative paths excluded from the package content identity.
    pub excludes: Vec<String>,
}

/// Dependency scope used by package resolution.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum DependencyScope {
    Build,
    Runtime,
    Development,
}

/// Where a dependency is obtained from.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum DependencySource {
    Path(String),
    Git {
        url: String,
        rev: Option<String>,
        tag: Option<String>,
        branch: Option<String>,
    },
    Registry {
        registry: String,
        package: RegistryPackageName,
        requirement: PackageVersionRequirement,
    },
}

/// A package dependency and the compilation environment that can use it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PackageDependency {
    pub name: String,
    pub source: DependencySource,
    pub scope: DependencyScope,
}

/// Dependency scope policy for constructing a package graph.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct PackageGraphOptions {
    pub include_development: bool,
}

impl PackageGraphOptions {
    fn includes(self, scope: DependencyScope) -> bool {
        match scope {
            DependencyScope::Build | DependencyScope::Runtime => true,
            DependencyScope::Development => self.include_development,
        }
    }
}

/// Declarative local package manifest used to name and select package content.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PackageManifest {
    /// Stable package namespace/name identity.
    pub package: PackageId,
    /// Exact compiler protocol epoch required by this package.
    pub protocol_epoch: Option<u32>,
    /// Module file exported by this package, when it defines a library entry.
    pub library_path: Option<String>,
    /// Source selection rules.
    pub source: PackageSourceSelection,
    /// Dependencies selected for this package.
    pub dependencies: Vec<PackageDependency>,
}

/// Workspace membership and shared declarations from a workspace root.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WorkspaceManifest {
    pub root: PathBuf,
    pub members: Vec<String>,
    pub default_members: Option<Vec<String>>,
    pub exclude: Vec<String>,
    pub package: WorkspacePackageFields,
    pub dependencies: BTreeMap<String, DependencySource>,
}

/// One validated package selected from a workspace member list.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WorkspaceMember {
    pub name: String,
    pub root: PathBuf,
}

/// Package identity fields available for explicit member inheritance.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct WorkspacePackageFields {
    pub namespace: Option<String>,
    pub version: Option<String>,
    pub protocol_epoch: Option<u32>,
}

/// Resolves local path dependencies without loading their source modules.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PackageGraphNode {
    pub manifest: PackageManifest,
    /// Stable dependency-qualified node name, not a filesystem path.
    pub root: String,
    pub source_fingerprint: [u8; 32],
}

/// A bounded package graph assembled from local manifests.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PackageGraph {
    /// Dependency-scope policy used to produce this graph.
    pub options: PackageGraphOptions,
    pub nodes: Vec<PackageGraphNode>,
    /// Declared edges and their actual resolver-selected targets.
    pub dependencies: Vec<PackageDependencyReport>,
}

/// A stable summary of package-graph selection decisions.
///
/// The package graph is intentionally resolved without loading source
/// modules. This report lets callers explain which dependency scopes were
/// eligible and which package roots were actually selected.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PackageGraphReport {
    /// Dependency scopes enabled while resolving the graph.
    pub options: PackageGraphOptions,
    /// Package nodes selected by the resolver, in canonical root order.
    pub resolved: Vec<PackageGraphNode>,
    /// Declared dependencies from every resolved package, including scopes
    /// that were not selected by `options`.
    pub declared: Vec<PackageDependencyReport>,
}

/// One declared dependency and whether its target entered the selected graph.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PackageDependencyReport {
    /// Package root that owns the declaration.
    pub owner_root: String,
    /// Original dependency declaration.
    pub dependency: PackageDependency,
    /// Canonical package root selected for this dependency, when resolved.
    pub resolved_root: Option<String>,
    /// Canonical checkout path when the declared target currently exists.
    ///
    /// This is invocation-local evidence used to match build-plan package
    /// roots back to manifest dependency declarations. It is not part of package
    /// identity and may be present even when the dependency was filtered.
    pub source_root: Option<std::path::PathBuf>,
}

/// Current on-disk lockfile schema.
pub const LOCKFILE_VERSION: u32 = 2;
const MAX_LOCKFILE_BYTES: usize = 4 * 1024 * 1024;

/// Reproducible package source selections recorded in `nia.lock`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PackageLock {
    pub version: u32,
    #[serde(default, rename = "packages")]
    pub package: Vec<LockedPackage>,
}

#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "kebab-case", deny_unknown_fields)]
pub enum LockedSource {
    Path {
        owner: String,
        path: String,
        namespace: String,
        package: String,
        version: String,
        source_fingerprint: String,
    },
    Git {
        url: String,
        selector: String,
        revision: String,
        source_fingerprint: String,
    },
    Registry {
        registry: String,
        namespace: String,
        package: String,
        version: String,
        archive_digest: String,
        source_fingerprint: String,
    },
}

#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct LockedPackage {
    pub name: String,
    pub source: LockedSource,
}

impl LockedSource {
    fn reachability_identity(&self) -> Option<String> {
        match self {
            Self::Path { owner, path, .. } => Some(format!("path\0{owner}\0{path}")),
            Self::Git { url, selector, .. } => Some(format!("git\0{url}\0{selector}")),
            Self::Registry {
                registry,
                namespace,
                package,
                ..
            } => Some(format!("registry\0{registry}\0{namespace}\0{package}")),
        }
    }

    fn git_identity(&self) -> Option<(&str, &str)> {
        match self {
            Self::Git { url, selector, .. } => Some((url, selector)),
            Self::Path { .. } | Self::Registry { .. } => None,
        }
    }

    fn update_kind(&self) -> Option<LockUpdateKind> {
        match self {
            Self::Git { .. } => Some(LockUpdateKind::Git),
            Self::Registry { .. } => Some(LockUpdateKind::Registry),
            Self::Path { .. } => None,
        }
    }

    fn update_value(&self) -> Option<&str> {
        match self {
            Self::Git { revision, .. } => Some(revision),
            Self::Registry { version, .. } => Some(version),
            Self::Path { .. } => None,
        }
    }

    fn update_selector(&self) -> Option<String> {
        match self {
            Self::Git { selector, .. } => Some(selector.clone()),
            Self::Registry {
                registry,
                namespace,
                package,
                ..
            } => Some(format!("registry:{registry}/{namespace}/{package}")),
            Self::Path { .. } => None,
        }
    }

    fn source_fingerprint(&self) -> Option<&str> {
        match self {
            Self::Git {
                source_fingerprint, ..
            }
            | Self::Registry {
                source_fingerprint, ..
            }
            | Self::Path {
                source_fingerprint, ..
            } => Some(source_fingerprint),
        }
    }
}

fn same_locked_selector(left: &LockedPackage, right: &LockedPackage) -> bool {
    if left.name != right.name
        && !(matches!(left.source, LockedSource::Registry { .. })
            && matches!(right.source, LockedSource::Registry { .. }))
    {
        return false;
    }
    match (&left.source, &right.source) {
        (
            LockedSource::Git {
                url: left_url,
                selector: left_selector,
                ..
            },
            LockedSource::Git {
                url: right_url,
                selector: right_selector,
                ..
            },
        ) => left_url == right_url && left_selector == right_selector,
        (
            LockedSource::Registry {
                registry: left_registry,
                namespace: left_namespace,
                package: left_package,
                ..
            },
            LockedSource::Registry {
                registry: right_registry,
                namespace: right_namespace,
                package: right_package,
                ..
            },
        ) => {
            left_registry == right_registry
                && left_namespace == right_namespace
                && left_package == right_package
        }
        (
            LockedSource::Path {
                owner: left_owner,
                path: left_path,
                ..
            },
            LockedSource::Path {
                owner: right_owner,
                path: right_path,
                ..
            },
        ) => left_owner == right_owner && left_path == right_path,
        _ => false,
    }
}

impl Default for PackageLock {
    fn default() -> Self {
        Self {
            version: LOCKFILE_VERSION,
            package: Vec::new(),
        }
    }
}

impl PackageLock {
    fn validate(&self, error_kind: io::ErrorKind) -> io::Result<()> {
        if self.version != LOCKFILE_VERSION {
            return Err(io::Error::new(
                error_kind,
                format!("unsupported nia.lock version {}", self.version),
            ));
        }
        let mut identities = BTreeSet::new();
        let mut registry_versions = BTreeMap::<(&str, &str, &str), &str>::new();
        let mut registry_bindings = BTreeMap::<(&str, &str, &str), (&str, &str, &str)>::new();
        let mut registry_sources = BTreeMap::<(&str, &str), &str>::new();
        for package in &self.package {
            validate_dependency_name(&package.name).map_err(|error| {
                io::Error::new(
                    error_kind,
                    format!("invalid lockfile package name: {error}"),
                )
            })?;
            let identity = match &package.source {
                LockedSource::Git {
                    url,
                    selector,
                    revision,
                    source_fingerprint,
                } => {
                    if url.is_empty() || url.contains(['\r', '\n']) {
                        return Err(io::Error::new(error_kind, "lockfile Git URL is invalid"));
                    }
                    let canonical_url = canonical_git_source(url).map_err(|error| {
                        io::Error::new(error_kind, format!("lockfile Git URL is invalid: {error}"))
                    })?;
                    if canonical_url != *url {
                        return Err(io::Error::new(
                            error_kind,
                            "lockfile Git URL is not canonical",
                        ));
                    }
                    if selector.is_empty() || selector.contains(['\r', '\n']) {
                        return Err(io::Error::new(
                            error_kind,
                            "lockfile Git selector is invalid",
                        ));
                    }
                    if revision.len() != 40
                        || !revision.bytes().all(|byte| byte.is_ascii_hexdigit())
                    {
                        return Err(io::Error::new(
                            error_kind,
                            "lockfile Git revision must be a full hexadecimal commit",
                        ));
                    }
                    validate_blake3_text(source_fingerprint, "source fingerprint", error_kind)?;
                    format!("git\0{url}\0{selector}")
                }
                LockedSource::Registry {
                    registry,
                    namespace,
                    package,
                    version,
                    archive_digest,
                    source_fingerprint,
                } => {
                    validate_registry_package_segment(namespace, "lockfile")
                        .map_err(|error| io::Error::new(error_kind, error))?;
                    validate_registry_package_segment(package, "lockfile")
                        .map_err(|error| io::Error::new(error_kind, error))?;
                    if registry.is_empty()
                        || registry.len() > 64
                        || !registry.bytes().all(|byte| {
                            byte.is_ascii_lowercase() || byte.is_ascii_digit() || byte == b'-'
                        })
                    {
                        return Err(io::Error::new(error_kind, "lockfile registry is invalid"));
                    }
                    PackageVersion::parse(version).map_err(|error| {
                        io::Error::new(
                            error_kind,
                            format!("lockfile package version is invalid: {error}"),
                        )
                    })?;
                    validate_blake3_text(archive_digest, "archive digest", error_kind)?;
                    validate_blake3_text(source_fingerprint, "source fingerprint", error_kind)?;
                    if registry_sources
                        .insert((namespace, package), registry)
                        .is_some_and(|previous| previous != registry)
                    {
                        return Err(io::Error::new(
                            error_kind,
                            "lockfile selects one registry package from multiple registries",
                        ));
                    }
                    if registry_versions
                        .insert((registry, namespace, package), version)
                        .is_some_and(|previous| previous != version)
                    {
                        return Err(io::Error::new(
                            error_kind,
                            "lockfile selects multiple versions of one registry package",
                        ));
                    }
                    if registry_bindings
                        .insert(
                            (registry, namespace, package),
                            (version, archive_digest, source_fingerprint),
                        )
                        .is_some_and(|previous| {
                            previous != (version, archive_digest, source_fingerprint)
                        })
                    {
                        return Err(io::Error::new(
                            error_kind,
                            "lockfile binds one registry package to multiple archive or source fingerprints",
                        ));
                    }
                    format!("registry\0{registry}\0{namespace}\0{package}")
                }
                LockedSource::Path {
                    owner,
                    path,
                    namespace,
                    package,
                    version,
                    source_fingerprint,
                } => {
                    if owner.is_empty()
                        || owner.bytes().any(|byte| byte < 0x20 || byte == 0x7f)
                        || path.is_empty()
                        || path.contains(['\r', '\n', '\0'])
                        || Path::new(path).is_absolute()
                        || path.starts_with('/')
                        || path.starts_with('\\')
                        || path.as_bytes().get(1).is_some_and(|byte| *byte == b':')
                    {
                        return Err(io::Error::new(
                            error_kind,
                            "lockfile path source is invalid",
                        ));
                    }
                    let version = PackageVersion::parse(version).map_err(|error| {
                        io::Error::new(
                            error_kind,
                            format!("lockfile package version is invalid: {error}"),
                        )
                    })?;
                    validate_package_id(&PackageId {
                        namespace: namespace.clone(),
                        name: package.clone(),
                        version,
                    })
                    .map_err(|error| io::Error::new(error_kind, error))?;
                    validate_blake3_text(source_fingerprint, "source fingerprint", error_kind)?;
                    format!("path\0{owner}\0{path}")
                }
            };
            if !identities.insert((package.name.as_str(), identity)) {
                return Err(io::Error::new(
                    error_kind,
                    "lockfile contains duplicate package source identities",
                ));
            }
        }
        Ok(())
    }

    /// Read a bounded, strict TOML lockfile. A missing lockfile is empty.
    pub fn read(path: &Path) -> io::Result<Self> {
        Self::read_snapshot(path).map(|(lock, _)| lock)
    }

    /// Retains validated original bytes for transaction rollback.
    fn read_snapshot(path: &Path) -> io::Result<(Self, Option<Vec<u8>>)> {
        let mut bytes = Vec::new();
        match open_regular_file(path, "package lockfile") {
            Ok(file) => file
                .take((MAX_LOCKFILE_BYTES + 1) as u64)
                .read_to_end(&mut bytes)?,
            Err(error) if error.kind() == io::ErrorKind::NotFound => {
                return Ok((Self::default(), None));
            }
            Err(error) => return Err(error),
        };
        if bytes.len() > MAX_LOCKFILE_BYTES {
            return Err(io::Error::new(
                io::ErrorKind::InvalidData,
                "package lockfile is too large",
            ));
        }
        let text = std::str::from_utf8(&bytes)
            .map_err(|error| io::Error::new(io::ErrorKind::InvalidData, error))?;
        let lock: Self = toml::from_str(text)
            .map_err(|error| io::Error::new(io::ErrorKind::InvalidData, error))?;
        lock.validate(io::ErrorKind::InvalidData)?;
        Ok((lock, Some(bytes)))
    }

    /// Atomically replace a lockfile, creating its parent directory as needed.
    pub fn write_atomic(&self, path: &Path) -> io::Result<()> {
        self.validate(io::ErrorKind::InvalidInput)?;
        let text = toml::to_string_pretty(self)
            .map_err(|error| io::Error::new(io::ErrorKind::InvalidData, error))?;
        write_atomic_path(path, text.as_bytes())
    }
}

fn open_regular_file(path: &Path, label: &'static str) -> io::Result<File> {
    let parent_path = path.parent().unwrap_or_else(|| Path::new("."));
    let parent = open_cache_root(parent_path, false)?;
    let name = path
        .file_name()
        .filter(|name| !name.is_empty())
        .ok_or_else(|| {
            io::Error::new(
                io::ErrorKind::InvalidInput,
                format!("{label} path has no name"),
            )
        })?;
    let mut options = CapOpenOptions::new();
    options.read(true);
    #[cfg(unix)]
    {
        use cap_std::fs::OpenOptionsExt as _;
        options.custom_flags(libc::O_NOFOLLOW);
    }
    #[cfg(windows)]
    {
        use cap_std::fs::OpenOptionsExt as _;
        use windows_sys::Win32::Storage::FileSystem::{
            FILE_FLAG_OPEN_REPARSE_POINT, FILE_SHARE_DELETE, FILE_SHARE_READ, FILE_SHARE_WRITE,
        };
        options
            .custom_flags(FILE_FLAG_OPEN_REPARSE_POINT)
            .share_mode(FILE_SHARE_READ | FILE_SHARE_WRITE | FILE_SHARE_DELETE);
    }
    let file = parent.open_with(name, &options)?.into_std();
    let metadata = file.metadata()?;
    if nia_compat::is_link_or_reparse_point(&metadata) || !metadata.is_file() {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            format!("{label} must be a regular, non-link file"),
        ));
    }
    Ok(file)
}

fn regular_file_exists(path: &Path, label: &'static str) -> io::Result<bool> {
    match open_regular_file(path, label) {
        Ok(_) => Ok(true),
        Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(false),
        Err(error) => Err(error),
    }
}

fn open_package_state_lock(path: &Path) -> io::Result<File> {
    let parent_path = path.parent().unwrap_or_else(|| Path::new("."));
    let parent = open_cache_root(parent_path, true)?;
    let name = path
        .file_name()
        .filter(|name| !name.is_empty())
        .ok_or_else(|| {
            io::Error::new(
                io::ErrorKind::InvalidInput,
                "package state lock has no name",
            )
        })?;
    let mut options = CapOpenOptions::new();
    options.read(true).write(true).create(true);
    #[cfg(unix)]
    {
        use cap_std::fs::OpenOptionsExt as _;
        options.custom_flags(libc::O_NOFOLLOW);
    }
    #[cfg(windows)]
    {
        use cap_std::fs::OpenOptionsExt as _;
        use windows_sys::Win32::Storage::FileSystem::FILE_FLAG_OPEN_REPARSE_POINT;
        options.custom_flags(FILE_FLAG_OPEN_REPARSE_POINT);
    }
    let file = parent.open_with(name, &options)?.into_std();
    let metadata = file.metadata()?;
    if nia_compat::is_link_or_reparse_point(&metadata) || !metadata.is_file() {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            "package state lock must be a regular, non-link file",
        ));
    }
    Ok(file)
}

fn canonical_real_directory(path: &Path) -> io::Result<PathBuf> {
    open_cache_root(path, false)?;
    path.canonicalize()
}

fn validate_blake3_text(value: &str, label: &str, kind: io::ErrorKind) -> io::Result<()> {
    if !value.starts_with("blake3:")
        || value.len() != 71
        || !value[7..]
            .bytes()
            .all(|byte| byte.is_ascii_hexdigit() && !byte.is_ascii_uppercase())
    {
        return Err(io::Error::new(
            kind,
            format!("lockfile {label} must be a blake3 digest"),
        ));
    }
    Ok(())
}

fn parse_blake3_lock_digest(value: &str) -> io::Result<[u8; 32]> {
    let mut digest = [0; 32];
    for (index, byte) in digest.iter_mut().enumerate() {
        *byte = u8::from_str_radix(&value[7 + index * 2..9 + index * 2], 16).map_err(|_| {
            io::Error::new(io::ErrorKind::InvalidData, "lockfile digest is invalid")
        })?;
    }
    Ok(digest)
}

/// How resolution may change `nia.lock` and the package cache.
///
/// The policy never changes which packages are selected, so it is not part of
/// [`PackageGraphOptions`] or the graph fingerprint.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct LockPolicy {
    /// Fail instead of adding, changing, or removing a `nia.lock` entry.
    pub locked: bool,
    /// Fail instead of fetching a Git dependency that is not cached.
    pub offline: bool,
}

/// Resolves supported Git dependencies into an immutable, project-local cache.
struct PackageFetcher {
    cache_root: PathBuf,
    lock_path: PathBuf,
    lock: PackageLock,
    /// Entries as read from disk, for `--locked` and for skipping no-op writes.
    original: Vec<LockedPackage>,
    policy: LockPolicy,
    dirty: bool,
    state_lock: Option<File>,
    transaction_lock: Option<PathBuf>,
    registries: BTreeMap<String, RegistryEndpoint>,
    registry_overrides: BTreeMap<RegistryPackageName, PackageId>,
    registry_selected: BTreeMap<RegistryPackageName, (String, PackageId)>,
    registry_sources: BTreeMap<RegistryPackageName, String>,
    registry_constraints: BTreeMap<RegistryPackageName, Vec<RegistryVersionConstraint>>,
    registry_candidates: BTreeMap<RegistryPackageName, Vec<PackageId>>,
    registry_aliases: BTreeMap<RegistryPackageName, String>,
    registry_restart: Option<(RegistryPackageName, PackageId)>,
    refresh_names: Option<BTreeSet<String>>,
    registry_fetch: Option<RegistryFetch>,
}

struct RegistryResolution {
    registries: BTreeMap<String, RegistryEndpoint>,
    overrides: BTreeMap<RegistryPackageName, PackageId>,
    refresh_names: Option<BTreeSet<String>>,
    fetch: Option<RegistryFetch>,
    transaction_lock: Option<PathBuf>,
    attempt: usize,
}

impl fmt::Debug for PackageFetcher {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("PackageFetcher")
            .field("cache_root", &self.cache_root)
            .field("lock_path", &self.lock_path)
            .field("policy", &self.policy)
            .field("dirty", &self.dirty)
            .finish_non_exhaustive()
    }
}

impl PackageFetcher {
    #[cfg(test)]
    fn for_root(root: &Path, policy: LockPolicy) -> io::Result<Self> {
        Self::for_root_with_registries(
            root,
            policy,
            BTreeMap::new(),
            BTreeMap::new(),
            None,
            None,
            None,
        )
    }

    fn for_root_with_registries(
        root: &Path,
        policy: LockPolicy,
        registries: BTreeMap<String, RegistryEndpoint>,
        registry_overrides: BTreeMap<RegistryPackageName, PackageId>,
        refresh_names: Option<BTreeSet<String>>,
        registry_fetch: Option<RegistryFetch>,
        transaction_lock: Option<PathBuf>,
    ) -> io::Result<Self> {
        let state_root = workspace_state_root(root)?;
        let lock_path = state_root.join("nia.lock");
        let lock = PackageLock::read(&lock_path)?;
        Ok(Self {
            cache_root: state_root.join(".nia/packages"),
            original: lock.package.clone(),
            lock,
            lock_path,
            policy,
            dirty: false,
            state_lock: None,
            transaction_lock,
            registries,
            registry_overrides,
            registry_selected: BTreeMap::new(),
            registry_sources: BTreeMap::new(),
            registry_constraints: BTreeMap::new(),
            registry_candidates: BTreeMap::new(),
            registry_aliases: BTreeMap::new(),
            registry_restart: None,
            refresh_names,
            registry_fetch,
        })
    }

    fn package_cache(&self, create: bool) -> io::Result<Option<Dir>> {
        match open_cache_root(&self.cache_root, create) {
            Ok(directory) => Ok(Some(directory)),
            Err(error) if error.kind() == io::ErrorKind::NotFound && !create => Ok(None),
            Err(error) => Err(error),
        }
    }

    fn cached_package(&self, cache: &Dir, name: &str) -> io::Result<Option<PathBuf>> {
        match open_cache_directory(cache, name, false) {
            Ok(_) => Ok(Some(self.cache_root.join(name))),
            Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(None),
            Err(error) => Err(error),
        }
    }

    fn resolve(
        &mut self,
        owner_root: &Path,
        owner: &PackageId,
        dependency: &PackageDependency,
        dependency_chain: &[String],
    ) -> io::Result<PathBuf> {
        match &dependency.source {
            DependencySource::Path(path) => {
                let unresolved = owner_root.join(path);
                match open_cache_root(&unresolved, false) {
                    Ok(_) => {}
                    Err(error) if error.kind() == io::ErrorKind::NotFound => {
                        return Ok(unresolved);
                    }
                    Err(error) => return Err(error),
                };
                let destination = canonical_real_directory(&unresolved)?;
                if !regular_file_exists(&destination.join("nia.toml"), "package manifest")? {
                    return Ok(destination);
                }
                self.lock_state()?;
                let manifest = PackageManifest::read(&destination)?;
                let identity = package_source_identity_with_selection(
                    &destination,
                    manifest.package.clone(),
                    &manifest.source,
                )?;
                let source_fingerprint =
                    format!("blake3:{}", hex_digest(&identity.source_fingerprint));
                let owner_identity = owner.canonical_text();
                let expected = LockedPackage {
                    name: dependency.name.clone(),
                    source: LockedSource::Path {
                        owner: owner_identity,
                        path: path.clone(),
                        namespace: manifest.package.namespace.clone(),
                        package: manifest.package.name.clone(),
                        version: manifest.package.version.as_str().to_owned(),
                        source_fingerprint: source_fingerprint.clone(),
                    },
                };
                if let Some(entry) = self
                    .lock
                    .package
                    .iter()
                    .find(|entry| same_locked_selector(entry, &expected))
                {
                    if entry != &expected {
                        if self.policy.locked {
                            return Err(io::Error::new(
                                io::ErrorKind::InvalidInput,
                                format!(
                                    "path dependency {} changed, and locked mode forbids updating nia.lock",
                                    dependency.name
                                ),
                            ));
                        }
                        self.lock
                            .package
                            .retain(|entry| !same_locked_selector(entry, &expected));
                        self.lock.package.push(expected);
                        self.lock.package.sort();
                        self.dirty = true;
                    }
                } else if self.policy.locked {
                    return Err(io::Error::new(
                        io::ErrorKind::InvalidInput,
                        format!(
                            "path dependency {} is not recorded in nia.lock, and locked mode forbids adding it",
                            dependency.name
                        ),
                    ));
                } else {
                    self.lock
                        .package
                        .retain(|entry| !same_locked_selector(entry, &expected));
                    self.lock.package.push(expected);
                    self.lock.package.sort();
                    self.dirty = true;
                }
                Ok(destination)
            }
            DependencySource::Registry {
                registry,
                package,
                requirement,
            } => self.resolve_registry(
                owner,
                dependency,
                dependency_chain,
                registry,
                package,
                requirement,
            ),
            DependencySource::Git {
                url,
                rev,
                tag,
                branch,
            } => {
                self.lock_state()?;
                let source = canonical_git_source(url)?;
                let requested = rev
                    .clone()
                    .or_else(|| tag.clone())
                    .or_else(|| branch.clone());
                let selector = git_lock_selector(rev, tag, branch);
                let refresh = self
                    .refresh_names
                    .as_ref()
                    .is_some_and(|names| names.is_empty() || names.contains(&dependency.name));
                let mut cache_directory = self.package_cache(false)?;
                if let Some(entry) = self.lock.package.iter().find(|entry| {
                    entry.name == dependency.name
                        && matches!(
                            &entry.source,
                            LockedSource::Git { url, selector: locked_selector, .. }
                                if url == &source && locked_selector == &selector
                        )
                }) && !refresh
                {
                    let LockedSource::Git {
                        source_fingerprint, ..
                    } = &entry.source
                    else {
                        unreachable!()
                    };
                    let destination = cache_directory
                        .as_ref()
                        .map(|cache| self.cached_package(cache, &source_fingerprint[7..]))
                        .transpose()?
                        .flatten();
                    if let Some(destination) = destination {
                        let manifest = PackageManifest::read(&destination)?;
                        let identity = package_source_identity_with_selection(
                            &destination,
                            manifest.package.clone(),
                            &manifest.source,
                        )?;
                        if format!("blake3:{}", hex_digest(&identity.source_fingerprint))
                            == *source_fingerprint
                        {
                            return Ok(destination);
                        }
                        return Err(io::Error::new(
                            io::ErrorKind::InvalidData,
                            format!(
                                "cached Git dependency `{}` at `{}` does not match the content hash in nia.lock; delete that directory to fetch the locked commit again",
                                dependency.name,
                                display_path(&destination)
                            ),
                        ));
                    }
                }
                let locked_rev = (!refresh)
                    .then(|| {
                        self.lock
                            .package
                            .iter()
                            .find(|entry| {
                                entry.name == dependency.name
                                    && matches!(
                                        &entry.source,
                                        LockedSource::Git { url, selector: locked_selector, .. }
                                            if url == &source && locked_selector == &selector
                                    )
                            })
                            .and_then(|entry| match &entry.source {
                                LockedSource::Git { revision, .. } => Some(revision.clone()),
                                LockedSource::Path { .. } | LockedSource::Registry { .. } => None,
                            })
                    })
                    .flatten();
                let fetch_rev = rev.clone().or(locked_rev);
                let key = blake3::hash(
                    format!(
                        "{source}\0{}",
                        fetch_rev
                            .as_deref()
                            .or(requested.as_deref())
                            .unwrap_or("HEAD")
                    )
                    .as_bytes(),
                );
                let destination = self.cache_root.join(key.to_hex().as_str());
                let cached_destination = cache_directory
                    .as_ref()
                    .map(|cache| self.cached_package(cache, key.to_hex().as_str()))
                    .transpose()?
                    .flatten();
                if cached_destination.is_none() {
                    if self.policy.locked {
                        let message = if fetch_rev.is_some() {
                            format!(
                                "locked Git dependency `{}` is not in the package cache, and `--locked` forbids fetching it",
                                dependency.name
                            )
                        } else {
                            format!(
                                "Git dependency `{}` ({selector}) is not recorded in nia.lock, and `--locked` forbids adding it",
                                dependency.name
                            )
                        };
                        return Err(io::Error::new(
                            if fetch_rev.is_some() {
                                io::ErrorKind::NotFound
                            } else {
                                io::ErrorKind::InvalidInput
                            },
                            message,
                        ));
                    }
                    if self.policy.offline {
                        return Err(io::Error::new(
                            io::ErrorKind::NotFound,
                            format!(
                                "Git dependency `{}` is not in the package cache, and `--offline` forbids fetching it",
                                dependency.name
                            ),
                        ));
                    }
                    let cache = match cache_directory.take() {
                        Some(cache) => cache,
                        None => open_cache_root(&self.cache_root, true)?,
                    };
                    let staging_name = allocate_package_staging(&cache)?;
                    let staging_directory = open_cache_directory(&cache, &staging_name, false)?;
                    let checkout = checkout_source(
                        &source,
                        fetch_rev
                            .as_deref()
                            .or(requested.as_deref())
                            .unwrap_or("HEAD"),
                        &staging_directory,
                    );
                    drop(staging_directory);
                    let fetched_revision = match checkout {
                        Ok(revision) => revision,
                        Err(error) => {
                            let _ = cache.remove_dir_all(&staging_name);
                            return Err(error);
                        }
                    };
                    let destination_name = key.to_hex().to_string();
                    if let Err(error) = cache.rename(&staging_name, &cache, &destination_name) {
                        let _ = cache.remove_dir_all(&staging_name);
                        if self.cached_package(&cache, &destination_name)?.is_none() {
                            return Err(error);
                        }
                    }
                    cache_directory = Some(cache);
                    self.lock.package.retain(|entry| {
                        !(entry.name == dependency.name
                            && matches!(
                                &entry.source,
                                LockedSource::Git { url, selector: locked_selector, .. }
                                    if url == &source && locked_selector == &selector
                            ))
                    });
                    self.lock.package.push(LockedPackage {
                        name: dependency.name.clone(),
                        source: LockedSource::Git {
                            url: source.clone(),
                            selector: selector.clone(),
                            revision: fetched_revision,
                            source_fingerprint: String::new(),
                        },
                    });
                }
                let destination = cached_destination.unwrap_or(destination);
                let manifest = PackageManifest::read(&destination)?;
                let identity = package_source_identity_with_selection(
                    &destination,
                    manifest.package.clone(),
                    &manifest.source,
                )?;
                let rev = self
                    .lock
                    .package
                    .iter()
                    .find(|entry| {
                        entry.name == dependency.name
                            && matches!(
                                &entry.source,
                                LockedSource::Git { url, selector: locked_selector, .. }
                                    if url == &source && locked_selector == &selector
                            )
                    })
                    .and_then(|entry| match &entry.source {
                        LockedSource::Git { revision, .. } => Some(revision.clone()),
                        LockedSource::Path { .. } | LockedSource::Registry { .. } => None,
                    })
                    .ok_or_else(|| {
                        io::Error::new(io::ErrorKind::InvalidData, "Git revision was not recorded")
                    })?;
                let source_fingerprint =
                    format!("blake3:{}", hex_digest(&identity.source_fingerprint));
                let final_destination = self.cache_root.join(&source_fingerprint[7..]);
                if final_destination != destination {
                    let cache = cache_directory
                        .as_ref()
                        .expect("package cache directory was opened");
                    if self
                        .cached_package(cache, &source_fingerprint[7..])?
                        .is_some()
                    {
                        let destination_name = destination
                            .file_name()
                            .expect("package cache path has a final component");
                        let _ = cache.remove_dir_all(destination_name);
                    } else {
                        let source_name = destination
                            .file_name()
                            .expect("package cache path has a final component");
                        cache.rename(source_name, cache, &source_fingerprint[7..])?;
                    }
                }
                self.lock.package.retain(|entry| {
                    !(entry.name == dependency.name
                        && matches!(
                            &entry.source,
                            LockedSource::Git { url, selector: locked_selector, .. }
                                if url == &source && locked_selector == &selector
                        ))
                });
                self.lock.package.push(LockedPackage {
                    name: dependency.name.clone(),
                    source: LockedSource::Git {
                        url: source,
                        selector,
                        revision: rev,
                        source_fingerprint,
                    },
                });
                self.lock.package.sort_by(|left, right| {
                    (&left.name, &left.source).cmp(&(&right.name, &right.source))
                });
                self.dirty = true;
                Ok(final_destination)
            }
        }
    }

    fn resolve_registry(
        &mut self,
        owner: &PackageId,
        dependency: &PackageDependency,
        dependency_chain: &[String],
        registry: &str,
        package: &RegistryPackageName,
        requirement: &PackageVersionRequirement,
    ) -> io::Result<PathBuf> {
        self.lock_state()?;
        record_registry_source(&mut self.registry_sources, package, registry)?;
        self.registry_constraints
            .entry(package.clone())
            .or_default()
            .push(RegistryVersionConstraint {
                package: package.clone(),
                requirement: requirement.clone(),
                requiring_package: owner.clone(),
                dependency_chain: dependency_chain.to_vec(),
            });
        let refresh = self
            .refresh_names
            .as_ref()
            .is_some_and(|names| names.is_empty() || names.contains(&dependency.name));
        let state_root = self
            .lock_path
            .parent()
            .ok_or_else(|| io::Error::other("registry state directory is unavailable"))?;
        let cache = RegistryCache::new(state_root.join(".nia/registry"));
        let locked = self.lock.package.iter().find_map(|entry| {
            let LockedSource::Registry {
                registry: locked_registry,
                namespace,
                package: locked_package,
                version,
                archive_digest,
                source_fingerprint,
            } = &entry.source
            else {
                return None;
            };
            (locked_registry == registry
                && namespace == &package.namespace
                && locked_package == &package.name)
                .then(|| {
                    (
                        PackageId {
                            namespace: namespace.clone(),
                            name: locked_package.clone(),
                            version: PackageVersion::parse(version)
                                .expect("validated lock version"),
                        },
                        parse_blake3_lock_digest(archive_digest).expect("validated lock digest"),
                        source_fingerprint.clone(),
                    )
                })
        });
        let locked_cache_hit = !refresh
            && locked
                .as_ref()
                .is_some_and(|(_, digest, _)| cache.read_archive(digest).is_ok());
        let (selected, archive_digest, locked_fingerprint, archive) = if self.policy.locked
            || (locked_cache_hit
                && locked.as_ref().is_some_and(|(selected, ..)| {
                    self.registry_constraints[package]
                        .iter()
                        .all(|constraint| constraint.requirement.matches(&selected.version))
                        && self
                            .registry_overrides
                            .get(package)
                            .is_none_or(|version| version == selected)
                })) {
            let (selected, digest, source_fingerprint) = locked.ok_or_else(|| {
                io::Error::new(
                    io::ErrorKind::InvalidInput,
                    format!(
                        "registry dependency `{}` is not recorded in nia.lock",
                        dependency.name
                    ),
                )
            })?;
            if !requirement.matches(&selected.version) {
                return Err(io::Error::new(
                    io::ErrorKind::InvalidInput,
                    format!(
                        "locked registry version {} does not satisfy dependency `{}` requirement {}",
                        selected.version, dependency.name, requirement
                    ),
                ));
            }
            let archive = cache.read_archive(&digest)?;
            (selected, digest, Some(source_fingerprint), archive)
        } else {
            let endpoint = self.registries.get(registry).cloned().ok_or_else(|| {
                io::Error::new(
                    io::ErrorKind::NotFound,
                    format!("registry `{registry}` is not configured"),
                )
            })?;
            let client = match &self.registry_fetch {
                Some(fetch) => {
                    let fetch = fetch.clone();
                    RegistryClient::with_fetcher(endpoint, cache.clone(), move |url| fetch(url))
                }
                None => RegistryClient::new(endpoint, cache.clone()),
            };
            let index = client.index(self.policy.offline)?;
            let candidates = index
                .packages
                .iter()
                .filter(|record| {
                    record.package.namespace == package.namespace
                        && record.package.name == package.name
                })
                .map(|record| record.package.clone())
                .collect::<Vec<_>>();
            if candidates.is_empty() {
                return Err(io::Error::new(
                    io::ErrorKind::NotFound,
                    format!(
                        "registry `{registry}` has no release for {}/{}",
                        package.namespace, package.name
                    ),
                ));
            }
            self.registry_candidates
                .entry(package.clone())
                .or_insert_with(|| candidates.clone());
            let solved = solve_registry_versions(
                self.registry_constraints
                    .get(package)
                    .expect("constraint inserted"),
                self.registry_candidates
                    .get(package)
                    .expect("candidates inserted"),
            )?
            .remove(package)
            .expect("constraint has a selected package");
            let selected = self
                .registry_overrides
                .get(package)
                .filter(|version| {
                    candidates.iter().any(|candidate| candidate == *version)
                        && self
                            .registry_constraints
                            .get(package)
                            .is_some_and(|constraints| {
                                constraints.iter().all(|constraint| {
                                    constraint.requirement.matches(&version.version)
                                })
                            })
                })
                .cloned()
                .or_else(|| {
                    locked
                        .as_ref()
                        .map(|(version, ..)| version)
                        .filter(|version| {
                            candidates.iter().any(|candidate| candidate == *version)
                                && self.registry_constraints.get(package).is_some_and(
                                    |constraints| {
                                        constraints.iter().all(|constraint| {
                                            constraint.requirement.matches(&version.version)
                                        })
                                    },
                                )
                        })
                        .cloned()
                })
                .unwrap_or(solved);
            let record = index
                .packages
                .iter()
                .find(|record| record.package == selected)
                .ok_or_else(|| {
                    io::Error::new(
                        io::ErrorKind::InvalidData,
                        "selected registry release is missing from the index",
                    )
                })?;
            let archive = client.archive(record, self.policy.offline)?;
            (selected, record.archive_digest, None, archive)
        };
        if let Some((previous_registry, previous)) = self.registry_selected.get(package)
            && (previous_registry != registry || previous != &selected)
        {
            self.registry_restart = Some((package.clone(), selected.clone()));
        }
        self.registry_selected
            .insert(package.clone(), (registry.to_owned(), selected.clone()));
        let cache = open_cache_root(&self.cache_root, true)?;
        let staging_name = allocate_package_staging(&cache)?;
        let staging = self.cache_root.join(&staging_name);
        let staging_directory = open_cache_directory(&cache, &staging_name, false)?;
        let temporary = staging.join("package");
        let result = (|| {
            extract_registry_archive(&archive, &archive_digest, &temporary)?;
            let manifest = PackageManifest::read(&temporary)?;
            if manifest.package != selected {
                return Err(io::Error::new(
                    io::ErrorKind::InvalidData,
                    format!(
                        "registry archive identity {} does not match index release {}",
                        manifest.package.canonical_text(),
                        selected.canonical_text()
                    ),
                ));
            }
            let identity = package_source_identity_with_selection(
                &temporary,
                manifest.package.clone(),
                &manifest.source,
            )?;
            let source_fingerprint = format!("blake3:{}", hex_digest(&identity.source_fingerprint));
            if let Some(expected) = locked_fingerprint.as_deref()
                && expected != source_fingerprint
            {
                return Err(io::Error::new(
                    io::ErrorKind::InvalidData,
                    format!(
                        "registry archive for `{}` does not match its locked source fingerprint",
                        dependency.name
                    ),
                ));
            }
            let destination = self.cache_root.join(&source_fingerprint[7..]);
            match cache.symlink_metadata(&source_fingerprint[7..]) {
                Err(error) if error.kind() == io::ErrorKind::NotFound => {
                    staging_directory.rename("package", &cache, &source_fingerprint[7..])?;
                }
                Err(error) => return Err(error),
                Ok(_) => {
                    open_cache_directory(&cache, &source_fingerprint[7..], false)?;
                    let cached_manifest = PackageManifest::read(&destination)?;
                    let cached_identity = package_source_identity_with_selection(
                        &destination,
                        cached_manifest.package.clone(),
                        &cached_manifest.source,
                    )?;
                    let cached_fingerprint =
                        format!("blake3:{}", hex_digest(&cached_identity.source_fingerprint));
                    if cached_manifest.package != selected
                        || cached_fingerprint != source_fingerprint
                    {
                        return Err(io::Error::new(
                            io::ErrorKind::InvalidData,
                            format!(
                                "cached registry dependency `{}` does not match its content address; delete `{}` to fetch it again",
                                dependency.name,
                                display_path(&destination)
                            ),
                        ));
                    }
                    staging_directory.remove_dir_all("package")?;
                }
            }
            self.record_registry_lock(
                dependency,
                registry,
                &selected,
                &archive_digest,
                &source_fingerprint,
            )?;
            Ok(destination)
        })();
        drop(staging_directory);
        let _ = cache.remove_dir_all(&staging_name);
        result
    }

    fn record_registry_lock(
        &mut self,
        dependency: &PackageDependency,
        registry: &str,
        package: &PackageId,
        archive_digest: &[u8; 32],
        source_fingerprint: &str,
    ) -> io::Result<()> {
        let alias = self
            .registry_aliases
            .entry(RegistryPackageName {
                namespace: package.namespace.clone(),
                name: package.name.clone(),
            })
            .and_modify(|alias| {
                if dependency.name < *alias {
                    *alias = dependency.name.clone();
                }
            })
            .or_insert_with(|| dependency.name.clone())
            .clone();
        let expected = LockedPackage {
            name: alias,
            source: LockedSource::Registry {
                registry: registry.to_owned(),
                namespace: package.namespace.clone(),
                package: package.name.clone(),
                version: package.version.as_str().to_owned(),
                archive_digest: format!("blake3:{}", hex_digest(archive_digest)),
                source_fingerprint: source_fingerprint.to_owned(),
            },
        };
        if let Some(entry) = self
            .lock
            .package
            .iter()
            .find(|entry| same_locked_selector(entry, &expected))
        {
            if entry.source == expected.source && entry.name == expected.name {
                return Ok(());
            }
            if entry.source == expected.source && self.policy.locked {
                return Ok(());
            }
            if entry.source != expected.source
                && matches!(
                    &entry.source,
                    LockedSource::Registry { version, .. }
                        if version == package.version.as_str()
                )
            {
                return Err(io::Error::new(
                    io::ErrorKind::InvalidData,
                    format!(
                        "registry dependency `{}` does not match its nia.lock source record",
                        dependency.name
                    ),
                ));
            }
        }
        if self.policy.locked {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                format!(
                    "registry dependency `{}` is not recorded in nia.lock",
                    dependency.name
                ),
            ));
        }
        self.lock
            .package
            .retain(|entry| !same_locked_selector(entry, &expected));
        self.lock.package.push(expected);
        self.lock.package.sort();
        self.dirty = true;
        Ok(())
    }

    fn lock_state(&mut self) -> io::Result<()> {
        if self.state_lock.is_none() {
            let state_directory = self
                .cache_root
                .parent()
                .expect("package cache has a state directory");
            let state_lock_path = self
                .transaction_lock
                .clone()
                .unwrap_or_else(|| state_directory.join("packages.lock"));
            let lock = open_package_state_lock(&state_lock_path)?;
            if self.transaction_lock.is_none() {
                lock.lock()?;
            }
            self.lock = PackageLock::read(&self.lock_path)?;
            self.original = self.lock.package.clone();
            self.state_lock = Some(lock);
        }
        Ok(())
    }

    /// Drops lock entries that no manifest under the state root can reach.
    ///
    /// The walk covers every workspace member and every dependency scope and
    /// follows Git dependencies through their cached checkouts, so building
    /// one member or a non-test build never drops another member's or a
    /// development dependency's entry. When any part of that graph cannot be
    /// read offline, the lock is left unchanged.
    fn prune_unreachable(&mut self) -> io::Result<()> {
        if self.lock.package.is_empty() {
            return Ok(());
        }
        let Some(state_root) = self.lock_path.parent().map(Path::to_path_buf) else {
            return Ok(());
        };
        self.lock_state()?;
        let Some(reachable) = self.reachable_lock_keys(&state_root) else {
            return Ok(());
        };
        let before = self.lock.package.len();
        self.lock
            .package
            .retain(|entry| match entry.source.git_identity() {
                Some((source, selector)) => {
                    reachable.contains(&(entry.name.clone(), format!("git\0{source}\0{selector}")))
                }
                None => entry
                    .source
                    .reachability_identity()
                    .is_none_or(|identity| reachable.contains(&(entry.name.clone(), identity))),
            });
        if self.lock.package.len() != before {
            self.dirty = true;
        }
        Ok(())
    }

    fn reachable_lock_keys(&self, state_root: &Path) -> Option<BTreeSet<(String, String)>> {
        let mut reachable = BTreeSet::new();
        if !regular_file_exists(&state_root.join("nia.toml"), "package manifest").ok()? {
            // A root package without a manifest declares no dependencies.
            return Some(reachable);
        }
        let mut pending = Vec::new();
        let raw = read_raw_manifest(state_root).ok()?;
        if raw.workspace.is_some() {
            let workspace = WorkspaceManifest::read(state_root).ok()?;
            pending.extend(
                workspace
                    .member_packages()
                    .ok()?
                    .into_iter()
                    .map(|member| member.root),
            );
            if raw.package.is_some() {
                pending.push(state_root.to_path_buf());
            }
        } else {
            pending.push(state_root.to_path_buf());
        }
        let mut seen = BTreeSet::new();
        while let Some(root) = pending.pop() {
            let root = canonical_real_directory(&root).ok()?;
            if !seen.insert(root.clone()) {
                continue;
            }
            if seen.len() > 1024
                || !regular_file_exists(&root.join("nia.toml"), "package manifest").ok()?
            {
                return None;
            }
            let manifest = PackageManifest::read(&root).ok()?;
            for dependency in &manifest.dependencies {
                match &dependency.source {
                    DependencySource::Path(path) => {
                        let owner = manifest.package.canonical_text();
                        let entry = self.lock.package.iter().find(|entry| {
                            matches!(
                                &entry.source,
                                LockedSource::Path {
                                    owner: locked_owner,
                                    path: locked_path,
                                    ..
                                } if locked_owner == &owner && locked_path == path
                            )
                        })?;
                        pending.push(root.join(path));
                        reachable
                            .insert((entry.name.clone(), entry.source.reachability_identity()?));
                    }
                    DependencySource::Git {
                        url,
                        rev,
                        tag,
                        branch,
                    } => {
                        let source = canonical_git_source(url).ok()?;
                        let selector = git_lock_selector(rev, tag, branch);
                        let entry = self.lock.package.iter().find(|entry| {
                            entry.name == dependency.name
                                && entry.source.git_identity().is_some_and(
                                    |(url, locked_selector)| {
                                        url == source && locked_selector == selector
                                    },
                                )
                        })?;
                        pending.push(
                            self.cache_root
                                .join(entry.source.source_fingerprint()?.get(7..)?),
                        );
                        reachable.insert((
                            dependency.name.clone(),
                            format!("git\0{source}\0{selector}"),
                        ));
                    }
                    DependencySource::Registry {
                        registry, package, ..
                    } => {
                        let entry = self.lock.package.iter().find(|entry| {
                            matches!(
                                            &entry.source,
                                            LockedSource::Registry {
                                                registry: locked_registry,
                                                namespace,
                                                package: locked_package,
                                                ..
                                            } if locked_registry == registry
                            && namespace == &package.namespace
                                                && locked_package == &package.name
                                        )
                        })?;
                        pending.push(
                            self.cache_root
                                .join(entry.source.source_fingerprint()?.get(7..)?),
                        );
                        reachable
                            .insert((entry.name.clone(), entry.source.reachability_identity()?));
                    }
                }
            }
        }
        Some(reachable)
    }

    fn finish(self) -> io::Result<()> {
        if !self.dirty {
            return Ok(());
        }
        let mut before = self.original.clone();
        let mut after = self.lock.package.clone();
        before.sort();
        after.sort();
        if before == after {
            return Ok(());
        }
        if self.policy.locked {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                "nia.lock needs to be updated, but `--locked` forbids changing it",
            ));
        }
        self.lock.write_atomic(&self.lock_path)
    }
}

fn record_registry_source(
    sources: &mut BTreeMap<RegistryPackageName, String>,
    package: &RegistryPackageName,
    registry: &str,
) -> io::Result<()> {
    if let Some(previous) = sources.get(package) {
        if previous != registry {
            return Err(io::Error::new(
                io::ErrorKind::InvalidData,
                format!(
                    "registry package {}/{} is requested from both `{previous}` and `{registry}`",
                    package.namespace, package.name
                ),
            ));
        }
    } else {
        sources.insert(package.clone(), registry.to_owned());
    }
    Ok(())
}

/// One `nia.lock` entry changed by [`update_lock`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LockUpdate {
    pub name: String,
    pub selector: String,
    pub kind: LockUpdateKind,
    /// Selected revision or version; `None` when the entry was added.
    pub old_value: Option<String>,
    /// Selected revision or version; `None` when the entry was removed.
    pub new_value: Option<String>,
}

/// Source family reported by [`LockUpdate`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LockUpdateKind {
    Git,
    Registry,
}

/// Fetches the named Git and registry dependencies again, or every locked
/// dependency when `names` is empty, and rewrites `nia.lock`.
///
/// `start` is any directory inside a package or workspace. Every package that
/// shares the lock file is resolved with all dependency scopes, so updating
/// from one member refreshes entries for the whole workspace.
pub fn update_lock(start: &Path, names: &[String]) -> io::Result<Vec<LockUpdate>> {
    update_lock_inner(start, names, None)
}

#[cfg(test)]
fn update_lock_with_registry_fetcher(
    start: &Path,
    names: &[String],
    fetch: RegistryFetch,
) -> io::Result<Vec<LockUpdate>> {
    update_lock_inner(start, names, Some(fetch))
}

fn update_lock_inner(
    start: &Path,
    names: &[String],
    registry_fetch: Option<RegistryFetch>,
) -> io::Result<Vec<LockUpdate>> {
    let start = canonical_real_directory(start)?;
    let mut manifest_root = None;
    for directory in start.ancestors() {
        if regular_file_exists(&directory.join("nia.toml"), "package manifest")? {
            manifest_root = Some(directory);
            break;
        }
    }
    let manifest_root = manifest_root.ok_or_else(|| {
        io::Error::new(
            io::ErrorKind::NotFound,
            format!("no nia.toml found at or above `{}`", display_path(&start)),
        )
    })?;
    let state_root = workspace_state_root(manifest_root)?;
    let transaction_lock_path = state_root.join(".nia/packages.lock");
    let transaction_lock = open_package_state_lock(&transaction_lock_path)?;
    transaction_lock.lock()?;
    let lock_path = state_root.join("nia.lock");
    let (original_lock, original_bytes) = PackageLock::read_snapshot(&lock_path)?;
    let before = original_lock.package.clone();
    let updates = with_lockfile_rollback(&lock_path, original_bytes.as_deref(), || {
        let raw = read_raw_manifest(&state_root)?;
        let mut roots = Vec::new();
        if raw.workspace.is_some() {
            let workspace = WorkspaceManifest::read(&state_root)?;
            roots.extend(
                workspace
                    .member_packages()?
                    .into_iter()
                    .map(|member| (member.root, member.name)),
            );
            if raw.package.is_some() && !roots.iter().any(|(root, _)| root == &state_root) {
                roots.push((state_root.clone(), String::new()));
            }
        } else {
            roots.push((state_root.clone(), String::new()));
        }
        let registries = read_registry_endpoints(&state_root)?;
        let refresh_names = names.iter().cloned().collect::<BTreeSet<_>>();
        let graph = PackageGraph::resolve_for_update(
            roots,
            PackageGraphOptions {
                include_development: true,
            },
            registries,
            refresh_names,
            registry_fetch.clone(),
            transaction_lock_path.clone(),
        )?;
        for name in names {
            let registry_alias = graph.dependencies.iter().any(|report| {
                report.dependency.name == *name
                    && matches!(
                        &report.dependency.source,
                        DependencySource::Registry { registry, package, .. }
                            if before.iter().any(|entry| matches!(
                                &entry.source,
                                LockedSource::Registry {
                                    registry: locked_registry,
                                    namespace,
                                    package: locked_package,
                                    ..
                                } if locked_registry == registry
                                    && namespace == &package.namespace
                                    && locked_package == &package.name
                            ))
                    )
            });
            if !before
                .iter()
                .any(|entry| &entry.name == name && entry.source.update_kind().is_some())
                && !registry_alias
            {
                return Err(io::Error::new(
                    io::ErrorKind::InvalidInput,
                    format!("`{name}` is not a locked Git or registry dependency"),
                ));
            }
        }
        let after = PackageLock::read(&lock_path)?.package;
        let mut updates = collect_lock_updates(&before, &after);
        if !names.is_empty() {
            let mut requested_registry_aliases = BTreeMap::new();
            for name in names {
                if let Some(dependency) = graph
                    .dependencies
                    .iter()
                    .map(|report| &report.dependency)
                    .find(|dependency| dependency.name == *name)
                    && let DependencySource::Registry {
                        registry, package, ..
                    } = &dependency.source
                {
                    requested_registry_aliases.insert(
                        format!("registry:{registry}/{}/{}", package.namespace, package.name),
                        name.clone(),
                    );
                }
            }
            for update in &mut updates {
                if update.kind == LockUpdateKind::Registry
                    && let Some(alias) = requested_registry_aliases.get(&update.selector)
                {
                    update.name = alias.clone();
                }
            }
        }
        Ok(updates)
    })?;
    drop(transaction_lock);
    Ok(updates)
}

fn with_lockfile_rollback<T>(
    path: &Path,
    original_bytes: Option<&[u8]>,
    update: impl FnOnce() -> io::Result<T>,
) -> io::Result<T> {
    match update() {
        Ok(result) => Ok(result),
        Err(error) => {
            restore_lockfile_snapshot(path, original_bytes).map_err(|restore_error| {
                io::Error::new(
                    restore_error.kind(),
                    format!("{error}; additionally failed to restore nia.lock: {restore_error}"),
                )
            })?;
            Err(error)
        }
    }
}

fn restore_lockfile_snapshot(path: &Path, original_bytes: Option<&[u8]>) -> io::Result<()> {
    if let Some(bytes) = original_bytes {
        return write_atomic_path(path, bytes);
    }
    let parent = open_cache_root(path.parent().unwrap_or_else(|| Path::new(".")), false)?;
    let name = path.file_name().ok_or_else(|| {
        io::Error::new(
            io::ErrorKind::InvalidInput,
            "package lockfile path has no name",
        )
    })?;
    match parent.remove_file(name) {
        Ok(()) => sync_open_directory(&parent),
        Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(()),
        Err(error) => Err(error),
    }
}

fn collect_lock_updates(before: &[LockedPackage], after: &[LockedPackage]) -> Vec<LockUpdate> {
    let mut updates = Vec::new();
    for old in before {
        let (Some(kind), Some(selector), Some(old_value)) = (
            old.source.update_kind(),
            old.source.update_selector(),
            old.source.update_value(),
        ) else {
            continue;
        };
        let new = after.iter().find(|entry| same_locked_selector(entry, old));
        let new_value = new.and_then(|entry| entry.source.update_value());
        if new_value != Some(old_value) {
            updates.push(LockUpdate {
                name: old.name.clone(),
                selector,
                kind,
                old_value: Some(old_value.to_owned()),
                new_value: new_value.map(str::to_owned),
            });
        }
    }
    for new in after {
        let (Some(kind), Some(selector), Some(new_value)) = (
            new.source.update_kind(),
            new.source.update_selector(),
            new.source.update_value(),
        ) else {
            continue;
        };
        if !before.iter().any(|entry| same_locked_selector(entry, new)) {
            updates.push(LockUpdate {
                name: new.name.clone(),
                selector,
                kind,
                old_value: None,
                new_value: Some(new_value.to_owned()),
            });
        }
    }
    updates
        .sort_by(|left, right| (&left.name, &left.selector).cmp(&(&right.name, &right.selector)));
    updates
}

/// The `nia.lock` selector recorded for a Git dependency.
fn git_lock_selector(
    rev: &Option<String>,
    tag: &Option<String>,
    branch: &Option<String>,
) -> String {
    if let Some(rev) = rev {
        format!("rev:{rev}")
    } else if let Some(tag) = tag {
        format!("tag:{tag}")
    } else if let Some(branch) = branch {
        format!("branch:{branch}")
    } else {
        "head".to_string()
    }
}

fn allocate_package_staging(cache_root: &Dir) -> io::Result<String> {
    static NEXT_STAGING_ID: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
    for _ in 0..32 {
        let sequence = NEXT_STAGING_ID.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        let name = format!(".fetch-{}-{sequence}.tmp", std::process::id());
        match cache_root.create_dir(&name) {
            Ok(()) => return Ok(name),
            Err(error) if error.kind() == io::ErrorKind::AlreadyExists => {}
            Err(error) => return Err(error),
        }
    }
    Err(io::Error::new(
        io::ErrorKind::AlreadyExists,
        "could not allocate package staging directory",
    ))
}

fn canonical_git_source(url: &str) -> io::Result<String> {
    let mut canonical = url.trim_end_matches('/').to_string();
    if matches!(
        canonical.split_once("://").map(|(scheme, _)| scheme),
        Some("http" | "https" | "git")
    ) && canonical
        .split_once("://")
        .and_then(|(_, rest)| rest.split('/').next())
        .is_some_and(|authority| authority.contains('@'))
    {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            "Git credentials are not allowed in dependency URLs",
        ));
    }
    if let Some((scheme, rest)) = canonical.split_once("://") {
        canonical = format!("{}://{}", scheme.to_ascii_lowercase(), rest);
    }
    Ok(canonical)
}

fn hex_digest(bytes: &[u8; 32]) -> String {
    let mut result = String::with_capacity(64);
    for byte in bytes {
        use std::fmt::Write as _;
        let _ = write!(result, "{byte:02x}");
    }
    result
}

impl PackageGraph {
    /// Discovers the root package and selected dependency classes.
    pub fn resolve(root: &Path, include_development: bool) -> io::Result<Self> {
        Self::resolve_with_options(
            root,
            PackageGraphOptions {
                include_development,
            },
        )
    }

    /// Discovers a package graph under an explicit dependency-scope policy.
    pub fn resolve_with_options(root: &Path, options: PackageGraphOptions) -> io::Result<Self> {
        Self::resolve_with_policy(root, options, LockPolicy::default())
    }

    /// Discovers a package graph, changing `nia.lock` only as `policy` allows.
    pub fn resolve_with_policy(
        root: &Path,
        options: PackageGraphOptions,
        policy: LockPolicy,
    ) -> io::Result<Self> {
        let registries = read_registry_endpoints(root)?;
        Self::resolve_with_policy_and_registries(root, options, policy, registries)
    }

    /// Resolves a package graph with explicit registry endpoint configuration.
    pub fn resolve_with_policy_and_registries(
        root: &Path,
        options: PackageGraphOptions,
        policy: LockPolicy,
        registries: BTreeMap<String, RegistryEndpoint>,
    ) -> io::Result<Self> {
        Self::resolve_with_registry_overrides(
            root,
            options,
            policy,
            RegistryResolution {
                registries,
                overrides: BTreeMap::new(),
                refresh_names: None,
                fetch: None,
                transaction_lock: None,
                attempt: 0,
            },
        )
    }

    /// Resolves one graph containing every selected workspace member.
    ///
    /// This intersects registry constraints across member roots before a
    /// caller starts work for any individual member.
    pub fn resolve_members_with_policy(
        members: &[WorkspaceMember],
        options: PackageGraphOptions,
        policy: LockPolicy,
    ) -> io::Result<Self> {
        let Some(first) = members.first() else {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                "package graph requires at least one workspace member",
            ));
        };
        let roots = members
            .iter()
            .map(|member| (member.root.clone(), member.name.clone()))
            .collect();
        let state_root = workspace_state_root(&first.root)?;
        for member in members.iter().skip(1) {
            if workspace_state_root(&member.root)? != state_root {
                return Err(io::Error::new(
                    io::ErrorKind::InvalidInput,
                    "workspace members do not share a package state root",
                ));
            }
        }
        let declared_members = WorkspaceManifest::read(&state_root)?.member_packages()?;
        for member in members {
            let root = canonical_real_directory(&member.root)?;
            if !declared_members
                .iter()
                .any(|declared| declared.name == member.name && declared.root == root)
            {
                return Err(io::Error::new(
                    io::ErrorKind::InvalidInput,
                    format!(
                        "package `{}` is not a member of this workspace",
                        member.name
                    ),
                ));
            }
        }
        let registries = read_registry_endpoints(&first.root)?;
        Self::resolve_roots_with_registry_overrides(
            roots,
            options,
            policy,
            RegistryResolution {
                registries,
                overrides: BTreeMap::new(),
                refresh_names: None,
                fetch: None,
                transaction_lock: None,
                attempt: 0,
            },
        )
    }

    fn resolve_for_update(
        roots: Vec<(PathBuf, String)>,
        options: PackageGraphOptions,
        registries: BTreeMap<String, RegistryEndpoint>,
        refresh_names: BTreeSet<String>,
        registry_fetch: Option<RegistryFetch>,
        transaction_lock: PathBuf,
    ) -> io::Result<Self> {
        Self::resolve_roots_with_registry_overrides(
            roots,
            options,
            LockPolicy::default(),
            RegistryResolution {
                registries,
                overrides: BTreeMap::new(),
                refresh_names: Some(refresh_names),
                fetch: registry_fetch,
                transaction_lock: Some(transaction_lock),
                attempt: 0,
            },
        )
    }

    fn resolve_with_registry_overrides(
        root: &Path,
        options: PackageGraphOptions,
        policy: LockPolicy,
        resolution: RegistryResolution,
    ) -> io::Result<Self> {
        Self::resolve_roots_with_registry_overrides(
            vec![(root.to_path_buf(), String::new())],
            options,
            policy,
            resolution,
        )
    }

    fn resolve_roots_with_registry_overrides(
        roots: Vec<(PathBuf, String)>,
        options: PackageGraphOptions,
        policy: LockPolicy,
        resolution: RegistryResolution,
    ) -> io::Result<Self> {
        let root = roots
            .first()
            .map(|(root, _)| root.as_path())
            .ok_or_else(|| {
                io::Error::new(
                    io::ErrorKind::InvalidInput,
                    "package graph requires at least one root",
                )
            })?;
        let mut fetcher = PackageFetcher::for_root_with_registries(
            root,
            policy,
            resolution.registries.clone(),
            resolution.overrides,
            resolution.refresh_names.clone(),
            resolution.fetch.clone(),
            resolution.transaction_lock.clone(),
        )?;
        let mut nodes = Vec::new();
        let mut pending = roots
            .iter()
            .map(|(root, logical_root)| {
                (
                    root.clone(),
                    logical_root.clone(),
                    true,
                    Vec::new(),
                    Vec::new(),
                )
            })
            .collect::<Vec<_>>();
        let mut seen = BTreeSet::new();
        let mut edges = Vec::new();
        while let Some((current, logical_root, is_root, ancestors, chain)) = pending.pop() {
            open_cache_root(&current, false)
                .map_err(|error| dependency_context(&logical_root, error))?;
            let canonical = canonical_real_directory(&current)?;
            if ancestors.iter().any(|ancestor| ancestor == &canonical) {
                return Err(io::Error::new(
                    io::ErrorKind::InvalidData,
                    format!("package dependency cycle at `{}`", display_path(&canonical)),
                ));
            }
            if !seen.insert((canonical.clone(), logical_root.clone())) {
                continue;
            }
            if !is_root && !regular_file_exists(&canonical.join("nia.toml"), "package manifest")? {
                return Err(io::Error::new(
                    io::ErrorKind::NotFound,
                    format!(
                        "dependency `{logical_root}` at `{}` does not contain nia.toml",
                        display_path(&canonical)
                    ),
                ));
            }
            let in_node = |error| dependency_context(&logical_root, error);
            let manifest = PackageManifest::read(&canonical)
                .or_else(|error| {
                    if error.kind() == io::ErrorKind::NotFound && is_root {
                        Ok(PackageManifest::implicit(
                            canonical
                                .file_name()
                                .and_then(|v| v.to_str())
                                .unwrap_or("root"),
                        ))
                    } else {
                        Err(error)
                    }
                })
                .map_err(in_node)?;
            manifest.validate_protocol_epoch().map_err(in_node)?;
            if let Some(library_path) = &manifest.library_path {
                validate_library_entry(&canonical, library_path).map_err(in_node)?;
            }
            let source = package_source_identity_with_selection(
                &canonical,
                manifest.package.clone(),
                &manifest.source,
            )
            .map_err(in_node)?;
            let package_chain = if chain.is_empty() {
                vec![manifest.package.canonical_text()]
            } else {
                chain
            };
            for dependency in &manifest.dependencies {
                let child_root = if logical_root.is_empty() {
                    dependency.name.clone()
                } else {
                    format!("{logical_root}/{}", dependency.name)
                };
                if !options.includes(dependency.scope) {
                    let source_root = match &dependency.source {
                        DependencySource::Path(path) => {
                            canonical_real_directory(&canonical.join(path)).ok()
                        }
                        DependencySource::Git { .. } | DependencySource::Registry { .. } => None,
                    };
                    edges.push((
                        logical_root.clone(),
                        dependency.clone(),
                        child_root,
                        None,
                        source_root,
                    ));
                    continue;
                }
                let mut dependency_chain = package_chain.clone();
                dependency_chain.push(dependency.name.clone());
                let path = fetcher
                    .resolve(&canonical, &manifest.package, dependency, &dependency_chain)
                    .and_then(|path| {
                        canonical_real_directory(&path).map_err(|error| {
                            io::Error::new(
                                error.kind(),
                                format!("`{}`: {error}", display_path(&path)),
                            )
                        })
                    })
                    .map_err(|error| dependency_context(&child_root, error))?;
                edges.push((
                    logical_root.clone(),
                    dependency.clone(),
                    child_root.clone(),
                    Some(path.clone()),
                    Some(path.clone()),
                ));
                let mut next_ancestors = ancestors.clone();
                next_ancestors.push(canonical.clone());
                pending.push((path, child_root, false, next_ancestors, dependency_chain));
            }
            nodes.push(PackageGraphNode {
                manifest,
                root: logical_root,
                source_fingerprint: source.source_fingerprint,
            });
            if nodes.len() > 1024 {
                return Err(io::Error::new(
                    io::ErrorKind::InvalidData,
                    "package graph is too large",
                ));
            }
        }
        nodes.sort_by(|left, right| left.root.cmp(&right.root));
        let mut dependencies = edges
            .into_iter()
            .map(
                |(owner_root, dependency, target_root, target, source_root)| {
                    PackageDependencyReport {
                        owner_root,
                        dependency,
                        resolved_root: target.map(|_| target_root),
                        source_root,
                    }
                },
            )
            .collect::<Vec<_>>();
        dependencies.sort_by(|left, right| {
            (&left.owner_root, &left.dependency.name)
                .cmp(&(&right.owner_root, &right.dependency.name))
        });
        if let Some((package, selected)) = fetcher.registry_restart.take() {
            if resolution.attempt >= 64 {
                return Err(io::Error::new(
                    io::ErrorKind::InvalidData,
                    format!(
                        "registry dependency resolution did not converge for {}/{}",
                        package.namespace, package.name
                    ),
                ));
            }
            let mut next_fetcher = fetcher;
            next_fetcher.state_lock.take();
            let mut overrides = std::mem::take(&mut next_fetcher.registry_overrides);
            overrides.insert(package, selected);
            drop(next_fetcher);
            return Self::resolve_roots_with_registry_overrides(
                roots,
                options,
                policy,
                RegistryResolution {
                    overrides,
                    attempt: resolution.attempt + 1,
                    ..resolution
                },
            );
        }
        fetcher.prune_unreachable()?;
        fetcher.finish()?;
        Ok(Self {
            options,
            nodes,
            dependencies,
        })
    }

    /// Returns the canonical identity of the resolved nodes and selection policy.
    pub fn fingerprint(&self) -> [u8; 32] {
        let mut encoded = Vec::new();
        encoded.extend_from_slice(PACKAGE_GRAPH_DOMAIN.as_str().as_bytes());
        encoded.extend_from_slice(b"\0\x01");
        encoded.push(u8::from(self.options.include_development));
        for node in &self.nodes {
            write_fingerprint_field(&mut encoded, node.root.as_bytes());
            write_fingerprint_field(
                &mut encoded,
                node.manifest.package.canonical_text().as_bytes(),
            );
            match node.manifest.protocol_epoch {
                Some(epoch) => {
                    encoded.push(1);
                    encoded.extend_from_slice(&epoch.to_le_bytes());
                }
                None => encoded.push(0),
            }
            match &node.manifest.library_path {
                Some(path) => {
                    encoded.push(1);
                    write_fingerprint_field(&mut encoded, path.as_bytes());
                }
                None => encoded.push(0),
            }
            encoded
                .extend_from_slice(&(node.manifest.source.source_roots.len() as u64).to_le_bytes());
            for root in &node.manifest.source.source_roots {
                write_fingerprint_field(&mut encoded, root.as_bytes());
            }
            encoded.extend_from_slice(&(node.manifest.source.excludes.len() as u64).to_le_bytes());
            for exclude in &node.manifest.source.excludes {
                write_fingerprint_field(&mut encoded, exclude.as_bytes());
            }
            encoded.extend_from_slice(&node.source_fingerprint);
            encoded.extend_from_slice(&(node.manifest.dependencies.len() as u64).to_le_bytes());
            for dependency in &node.manifest.dependencies {
                write_fingerprint_field(&mut encoded, dependency.name.as_bytes());
                match &dependency.source {
                    DependencySource::Path(path) => {
                        encoded.push(0);
                        write_fingerprint_field(&mut encoded, path.as_bytes());
                    }
                    DependencySource::Git {
                        url,
                        rev,
                        tag,
                        branch,
                    } => {
                        encoded.push(1);
                        write_fingerprint_field(&mut encoded, url.as_bytes());
                        for reference in [rev, tag, branch] {
                            if let Some(reference) = reference {
                                encoded.push(1);
                                write_fingerprint_field(&mut encoded, reference.as_bytes());
                            } else {
                                encoded.push(0);
                            }
                        }
                    }
                    DependencySource::Registry {
                        registry,
                        package,
                        requirement,
                    } => {
                        encoded.push(2);
                        write_fingerprint_field(&mut encoded, registry.as_bytes());
                        write_fingerprint_field(&mut encoded, package.namespace.as_bytes());
                        write_fingerprint_field(&mut encoded, package.name.as_bytes());
                        write_fingerprint_field(&mut encoded, requirement.to_string().as_bytes());
                    }
                }
                encoded.push(match dependency.scope {
                    DependencyScope::Build => 0,
                    DependencyScope::Runtime => 1,
                    DependencyScope::Development => 2,
                });
            }
        }
        *blake3::hash(&encoded).as_bytes()
    }

    /// Returns the resolver's actual dependency selection decisions.
    pub fn report(&self) -> PackageGraphReport {
        PackageGraphReport {
            options: self.options,
            resolved: self.nodes.clone(),
            declared: self.dependencies.clone(),
        }
    }
}

fn write_fingerprint_field(encoded: &mut Vec<u8>, value: &[u8]) {
    encoded.extend_from_slice(&(value.len() as u64).to_le_bytes());
    encoded.extend_from_slice(value);
}

fn validate_library_entry(root: &Path, library_path: &str) -> io::Result<()> {
    let mut components = Path::new(library_path)
        .components()
        .filter_map(|component| match component {
            std::path::Component::Normal(name) => Some(name),
            _ => None,
        })
        .peekable();
    let mut directory = open_cache_root(root, false)?;
    while let Some(name) = components.next() {
        if components.peek().is_some() {
            let name = name.to_str().ok_or_else(|| {
                io::Error::new(
                    io::ErrorKind::InvalidData,
                    "package library path is not UTF-8",
                )
            })?;
            directory = open_cache_directory(&directory, name, false).map_err(|error| {
                io::Error::new(
                    io::ErrorKind::InvalidData,
                    format!(
                        "package library entry `{library_path}` must use real directories: {error}"
                    ),
                )
            })?;
            continue;
        }
        if open_source_file(&directory, name)?.is_none() {
            return Err(io::Error::new(
                io::ErrorKind::InvalidData,
                format!("package library entry `{library_path}` must be a regular file"),
            ));
        }
    }
    Ok(())
}

fn validate_library_selection(
    library_path: &str,
    selection: &PackageSourceSelection,
) -> io::Result<()> {
    let selected = selection.source_roots.iter().any(|root| {
        root == "."
            || library_path == root
            || library_path
                .strip_prefix(root)
                .is_some_and(|suffix| suffix.starts_with('/'))
    });
    let excluded = selection.excludes.iter().any(|exclude| {
        library_path == exclude
            || library_path
                .strip_prefix(exclude)
                .is_some_and(|suffix| suffix.starts_with('/'))
    });
    if !selected || excluded {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            "package library path must be included by source roots and not excluded",
        ));
    }
    Ok(())
}

impl PackageManifest {
    /// Creates the implicit manifest used when no manifest file exists.
    pub fn implicit(root_name: impl Into<String>) -> Self {
        Self {
            package: PackageId {
                namespace: "local".into(),
                name: root_name.into(),
                version: PackageVersion::parse("0.0.0").expect("static package version"),
            },
            protocol_epoch: None,
            library_path: None,
            source: PackageSourceSelection::default(),
            dependencies: Vec::new(),
        }
    }

    /// Reads a bounded `nia.toml` package manifest.
    pub fn read(root: &Path) -> io::Result<Self> {
        let (raw, workspace) = read_package_manifest(root)?;
        let raw_package = raw.package.ok_or_else(|| {
            io::Error::new(
                io::ErrorKind::InvalidData,
                "workspace root does not declare a package",
            )
        })?;
        let mut manifest = Self::implicit(
            root.file_name()
                .and_then(|name| name.to_str())
                .unwrap_or("root")
                .to_ascii_lowercase(),
        );
        let version = resolve_inheritable(
            raw_package.version,
            workspace.as_ref().and_then(|ws| ws.package.version.clone()),
            "package.version",
        )?
        .ok_or_else(|| {
            io::Error::new(
                io::ErrorKind::InvalidData,
                "package.version must be declared or explicitly inherited from workspace.package",
            )
        })?;
        manifest.package = PackageId {
            namespace: resolve_inheritable(
                raw_package.namespace,
                workspace
                    .as_ref()
                    .and_then(|ws| ws.package.namespace.clone()),
                "package.namespace",
            )?
            .unwrap_or_else(|| "local".into()),
            name: raw_package.name,
            version: PackageVersion::parse(&version).map_err(|error| {
                io::Error::new(
                    io::ErrorKind::InvalidData,
                    format!("package.version must be a valid SemVer 2.0.0 version: {error}"),
                )
            })?,
        };
        manifest.protocol_epoch = resolve_inheritable(
            raw_package.protocol_epoch,
            workspace.as_ref().and_then(|ws| ws.package.protocol_epoch),
            "package.protocol-epoch",
        )?;
        manifest.library_path = raw
            .library
            .path
            .map(|path| validate_selection_path(&path))
            .transpose()
            .map_err(|error| {
                io::Error::new(
                    io::ErrorKind::InvalidData,
                    format!("invalid package library path: {error}"),
                )
            })?;
        manifest.source = PackageSourceSelection {
            source_roots: if raw.source.roots.is_empty() {
                vec![".".into()]
            } else {
                raw.source.roots
            },
            excludes: raw.source.exclude,
        };
        if let Some(library_path) = &manifest.library_path {
            validate_library_selection(library_path, &manifest.source)?;
        }
        let mut dependencies = Vec::new();
        for (scope, declarations) in [
            (DependencyScope::Build, raw.build_dependencies),
            (DependencyScope::Runtime, raw.dependencies),
            (DependencyScope::Development, raw.dev_dependencies),
        ] {
            for (name, declaration) in declarations {
                let source = match declaration.workspace {
                    Some(true) => {
                        declaration.validate_workspace_reference(&name)?;
                        let source = workspace
                            .as_ref()
                            .and_then(|ws| ws.dependencies.get(&name))
                            .cloned()
                            .ok_or_else(|| {
                                io::Error::new(
                                    io::ErrorKind::InvalidData,
                                    format!(
                                        "dependency `{name}` inherits from `workspace.dependencies`, but that entry is not defined"
                                    ),
                                )
                            })?;
                        rebase_workspace_dependency(source, root, workspace.as_ref().unwrap())?
                    }
                    Some(false) => {
                        return Err(io::Error::new(
                            io::ErrorKind::InvalidData,
                            "dependency `workspace` must be true when specified",
                        ));
                    }
                    None => declaration.into_source(&name)?,
                };
                validate_dependency_name(&name)?;
                dependencies.push(PackageDependency {
                    name,
                    source,
                    scope,
                });
            }
        }
        if manifest.source.source_roots.len() > MAX_MANIFEST_ENTRIES
            || manifest.source.excludes.len() > MAX_MANIFEST_ENTRIES
            || dependencies.len() > MAX_MANIFEST_ENTRIES
        {
            return Err(io::Error::new(
                io::ErrorKind::InvalidData,
                "package manifest has too many declarations",
            ));
        }
        dependencies.sort_by(|left, right| left.name.cmp(&right.name));
        let mut names = BTreeSet::new();
        for dependency in &dependencies {
            if !names.insert(&dependency.name) {
                return Err(io::Error::new(
                    io::ErrorKind::InvalidData,
                    "dependency names must be unique across dependency scopes",
                ));
            }
        }
        manifest.dependencies = dependencies;
        validate_package_id(&manifest.package)?;
        Ok(manifest)
    }

    /// Checks the manifest's exact compiler protocol epoch.
    pub fn validate_protocol_epoch(&self) -> io::Result<()> {
        if let Some(required) = self.protocol_epoch
            && required != PROTOCOL_EPOCH
        {
            return Err(io::Error::new(
                io::ErrorKind::InvalidData,
                format!(
                    "package requires protocol epoch {required}, current compiler is protocol epoch {PROTOCOL_EPOCH}"
                ),
            ));
        }
        Ok(())
    }
}

impl WorkspaceManifest {
    /// Reads and validates a workspace manifest at its root directory.
    pub fn read(root: &Path) -> io::Result<Self> {
        let raw = read_raw_manifest(root)?;
        let workspace = raw.workspace.ok_or_else(|| {
            io::Error::new(
                io::ErrorKind::InvalidData,
                "manifest does not declare a `[workspace]` table",
            )
        })?;
        if workspace.members.len() > MAX_MANIFEST_ENTRIES
            || workspace.exclude.len() > MAX_MANIFEST_ENTRIES
            || workspace.dependencies.len() > MAX_MANIFEST_ENTRIES
            || workspace
                .default_members
                .as_ref()
                .is_some_and(|members| members.len() > MAX_MANIFEST_ENTRIES)
        {
            return Err(io::Error::new(
                io::ErrorKind::InvalidData,
                "workspace manifest has too many declarations",
            ));
        }
        let canonical_root = canonical_real_directory(root)?;
        let exclude = workspace
            .exclude
            .iter()
            .map(|path| {
                let path = normalize_workspace_path(path, "workspace exclusion")?;
                glob::Pattern::new(&path)
                    .map_err(|error| io::Error::new(io::ErrorKind::InvalidData, error))?;
                Ok(path)
            })
            .collect::<io::Result<Vec<_>>>()?;
        let mut members =
            expand_workspace_paths(&canonical_root, &workspace.members, "workspace member")?;
        members.retain(|member| !workspace_path_excluded(member, &exclude));
        if members.is_empty() && raw.package.is_none() {
            return Err(io::Error::new(
                io::ErrorKind::InvalidData,
                "virtual workspace must declare at least one member",
            ));
        }
        if raw.package.is_some() && workspace_path_excluded(".", &exclude) {
            return Err(io::Error::new(
                io::ErrorKind::InvalidData,
                "workspace root package cannot be excluded",
            ));
        }
        if raw.package.is_some() && !members.iter().any(|member| member == ".") {
            members.push(".".to_owned());
            members.sort();
        }
        let mut physical_members = BTreeSet::new();
        for member in &members {
            let member_path = if member == "." {
                canonical_root.clone()
            } else {
                canonical_root.join(member)
            };
            open_cache_root(&member_path, false).map_err(|error| {
                io::Error::new(
                    error.kind(),
                    format!("workspace member `{member}` is not a real directory: {error}"),
                )
            })?;
            if !regular_file_exists(&member_path.join("nia.toml"), "package manifest")? {
                return Err(io::Error::new(
                    io::ErrorKind::InvalidData,
                    format!("workspace member `{member}` does not contain nia.toml"),
                ));
            }
            let physical_root = canonical_real_directory(&member_path)?;
            if !physical_root.starts_with(&canonical_root) {
                return Err(io::Error::new(
                    io::ErrorKind::InvalidData,
                    format!("workspace member `{member}` resolves outside the workspace root"),
                ));
            }
            if !physical_members.insert(physical_root.clone()) {
                return Err(io::Error::new(
                    io::ErrorKind::InvalidData,
                    format!("workspace member `{member}` aliases another member"),
                ));
            }
            if member != "." {
                let mut cursor = Some(member_path.as_path());
                while let Some(directory) = cursor.filter(|path| *path != canonical_root) {
                    if regular_file_exists(&directory.join("nia.toml"), "package manifest")?
                        && read_raw_manifest(directory)?.workspace.is_some()
                    {
                        return Err(io::Error::new(
                            io::ErrorKind::InvalidData,
                            format!("workspace member `{member}` belongs to a nested workspace"),
                        ));
                    }
                    cursor = directory.parent();
                }
            }
        }
        let default_members = workspace
            .default_members
            .map(|paths| {
                expand_workspace_paths(&canonical_root, &paths, "workspace default member")
            })
            .transpose()?;
        if let Some(defaults) = &default_members {
            if defaults.is_empty() {
                return Err(io::Error::new(
                    io::ErrorKind::InvalidData,
                    "workspace.default-members must select at least one member",
                ));
            }
            for member in defaults {
                if !members.contains(member) && !(member == "." && raw.package.is_some()) {
                    return Err(io::Error::new(
                        io::ErrorKind::InvalidData,
                        format!("default workspace member `{member}` is not a member"),
                    ));
                }
            }
        }
        let package = workspace
            .package
            .map_or_else(WorkspacePackageFields::default, |package| {
                WorkspacePackageFields {
                    namespace: package.namespace,
                    version: package.version,
                    protocol_epoch: package.protocol_epoch,
                }
            });
        let mut dependencies = BTreeMap::new();
        for (name, dependency) in workspace.dependencies {
            validate_dependency_name(&name)?;
            if dependency.workspace.is_some() {
                return Err(io::Error::new(
                    io::ErrorKind::InvalidData,
                    format!("workspace dependency `{name}` cannot inherit another dependency"),
                ));
            }
            dependencies.insert(name.clone(), dependency.into_source(&name)?);
        }
        Ok(Self {
            root: canonical_root,
            members,
            default_members,
            exclude,
            package,
            dependencies,
        })
    }

    /// Finds the nearest workspace containing `package_root` as a declared member.
    pub fn discover_for_package(package_root: &Path) -> io::Result<Option<Self>> {
        let package_root = canonical_real_directory(package_root)?;
        let mut cursor = package_root.parent();
        while let Some(root) = cursor {
            let manifest_path = root.join("nia.toml");
            if regular_file_exists(&manifest_path, "package manifest")? {
                let raw = read_raw_manifest(root)?;
                if raw.workspace.is_some() {
                    let workspace = Self::read(root)?;
                    let relative = package_root.strip_prefix(&workspace.root).map_err(|_| {
                        io::Error::new(io::ErrorKind::InvalidData, "invalid workspace member path")
                    })?;
                    let relative = relative.to_string_lossy().replace('\\', "/");
                    let relative = if relative.is_empty() { "." } else { &relative };
                    if Path::new(relative).starts_with(".nia")
                        || workspace_path_excluded(relative, &workspace.exclude)
                    {
                        return Ok(None);
                    }
                    if workspace.members.iter().any(|member| member == relative) {
                        return Ok(Some(workspace));
                    }
                    return Err(io::Error::new(
                        io::ErrorKind::InvalidData,
                        format!(
                            "package `{relative}` is under a workspace but is not listed in `workspace.members`"
                        ),
                    ));
                }
            }
            cursor = root.parent();
        }
        Ok(None)
    }

    /// Finds the nearest workspace at or above a directory.
    pub fn discover(start: &Path) -> io::Result<Option<Self>> {
        let start = if canonical_real_directory(start).is_ok() {
            start.to_path_buf()
        } else {
            let parent = start.parent().unwrap_or_else(|| Path::new("."));
            open_cache_root(parent, false)?;
            open_regular_file(start, "workspace discovery input")?;
            parent.to_path_buf()
        };
        let start = canonical_real_directory(&start)?;
        let mut cursor = Some(start.as_path());
        let mut package_root = None;
        while let Some(root) = cursor {
            let manifest = root.join("nia.toml");
            let has_manifest = regular_file_exists(&manifest, "package manifest")?;
            if has_manifest && read_raw_manifest(root)?.workspace.is_some() {
                if let Some(package_root) = package_root {
                    return Self::discover_for_package(package_root);
                }
                return Self::read(root).map(Some);
            }
            if package_root.is_none()
                && (has_manifest
                    || regular_file_exists(&root.join("build.nia"), "build script")?
                    || regular_file_exists(&root.join("pkg.nia"), "package script")?)
            {
                package_root = Some(root);
            }
            cursor = root.parent();
        }
        Ok(None)
    }

    /// Loads all declared member packages and rejects ambiguous package names.
    pub fn member_packages(&self) -> io::Result<Vec<WorkspaceMember>> {
        let mut members = Vec::with_capacity(self.members.len());
        let mut names = BTreeSet::new();
        for member in &self.members {
            let root = if member == "." {
                self.root.clone()
            } else {
                self.root.join(member)
            };
            let manifest = PackageManifest::read(&root)?;
            if !names.insert(manifest.package.name.clone()) {
                return Err(io::Error::new(
                    io::ErrorKind::InvalidData,
                    format!(
                        "workspace contains more than one package named `{}`",
                        manifest.package.name
                    ),
                ));
            }
            members.push(WorkspaceMember {
                name: manifest.package.name,
                root: canonical_real_directory(&root)?,
            });
        }
        Ok(members)
    }
}

fn workspace_state_root(package_root: &Path) -> io::Result<PathBuf> {
    let package_root = canonical_real_directory(package_root)?;
    if !regular_file_exists(&package_root.join("nia.toml"), "package manifest")? {
        return Ok(package_root);
    }
    let raw = read_raw_manifest(&package_root)?;
    if raw.workspace.is_some() {
        return Ok(package_root);
    }
    Ok(WorkspaceManifest::discover_for_package(&package_root)?
        .map(|workspace| workspace.root)
        .unwrap_or(package_root))
}

fn read_package_manifest(
    root: &Path,
) -> io::Result<(RawPackageManifest, Option<WorkspaceManifest>)> {
    let raw = read_raw_manifest(root)?;
    let workspace = if raw.workspace.is_some() {
        Some(WorkspaceManifest::read(root)?)
    } else {
        WorkspaceManifest::discover_for_package(root)?
    };
    if let Some(workspace) = &workspace {
        let relative = canonical_real_directory(root)?
            .strip_prefix(&workspace.root)
            .map_err(|_| {
                io::Error::new(
                    io::ErrorKind::InvalidData,
                    "package is outside its workspace",
                )
            })?
            .to_string_lossy()
            .replace('\\', "/");
        let relative = if relative.is_empty() { "." } else { &relative };
        if relative != "."
            && (!workspace.members.iter().any(|member| member == relative)
                || workspace_path_excluded(relative, &workspace.exclude))
        {
            return Err(io::Error::new(
                io::ErrorKind::InvalidData,
                format!("package `{relative}` is not a member of workspace"),
            ));
        }
        // Registry endpoints are read only from the root that owns `nia.lock`;
        // a member table would otherwise be silently ignored.
        if relative != "." && !raw.registries.is_empty() {
            return Err(io::Error::new(
                io::ErrorKind::InvalidData,
                format!(
                    "workspace member `{relative}` declares `[registries]`; configure registries in the workspace root nia.toml"
                ),
            ));
        }
    }
    Ok((raw, workspace))
}

fn read_raw_manifest(root: &Path) -> io::Result<RawPackageManifest> {
    let path = root.join("nia.toml");
    let mut contents = Vec::new();
    open_regular_file(&path, "package manifest")?
        .take((MAX_MANIFEST_BYTES + 1) as u64)
        .read_to_end(&mut contents)?;
    if contents.len() > MAX_MANIFEST_BYTES {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            "package manifest is too large",
        ));
    }
    let text = std::str::from_utf8(&contents)
        .map_err(|error| io::Error::new(io::ErrorKind::InvalidData, error))?;
    toml::from_str(text).map_err(|error| io::Error::new(io::ErrorKind::InvalidData, error))
}

fn read_registry_endpoints(root: &Path) -> io::Result<BTreeMap<String, RegistryEndpoint>> {
    let state_root = workspace_state_root(root)?;
    let raw = match read_raw_manifest(&state_root) {
        Ok(raw) => raw,
        Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(BTreeMap::new()),
        Err(error) => return Err(error),
    };
    raw.registries
        .into_iter()
        .map(|(name, url)| {
            RegistryEndpoint::new(name.clone(), url).map(|endpoint| (name, endpoint))
        })
        .collect()
}

fn resolve_inheritable<T>(
    value: Option<Inheritable<T>>,
    inherited: Option<T>,
    field: &str,
) -> io::Result<Option<T>> {
    match value {
        Some(Inheritable::Value(value)) => Ok(Some(value)),
        Some(Inheritable::Workspace { workspace: true }) => inherited.map(Some).ok_or_else(|| {
            io::Error::new(
                io::ErrorKind::InvalidData,
                format!("`{field}` inherits from workspace.package, but that field is not defined"),
            )
        }),
        Some(Inheritable::Workspace { workspace: false }) => Err(io::Error::new(
            io::ErrorKind::InvalidData,
            format!("`{field}.workspace` must be true when specified"),
        )),
        None => Ok(None),
    }
}

fn rebase_workspace_dependency(
    source: DependencySource,
    package_root: &Path,
    workspace: &WorkspaceManifest,
) -> io::Result<DependencySource> {
    let DependencySource::Path(path) = source else {
        return Ok(source);
    };
    let target = normalize_path(&workspace.root.join(path));
    let package_root = canonical_real_directory(package_root)?;
    let from = package_root.components().collect::<Vec<_>>();
    let to = target.components().collect::<Vec<_>>();
    let common = from
        .iter()
        .zip(&to)
        .take_while(|(left, right)| left == right)
        .count();
    let mut relative = PathBuf::new();
    for _ in common..from.len() {
        relative.push("..");
    }
    for component in &to[common..] {
        relative.push(component.as_os_str());
    }
    let relative = relative.to_str().ok_or_else(|| {
        io::Error::new(
            io::ErrorKind::InvalidData,
            "workspace dependency path is not UTF-8",
        )
    })?;
    Ok(DependencySource::Path(relative.replace('\\', "/")))
}

fn normalize_path(path: &Path) -> PathBuf {
    let mut output = PathBuf::new();
    for component in path.components() {
        match component {
            std::path::Component::CurDir => {}
            std::path::Component::ParentDir => {
                if !output.pop() {
                    output.push(component.as_os_str());
                }
            }
            other => output.push(other.as_os_str()),
        }
    }
    output
}

fn normalize_workspace_path(path: &str, role: &str) -> io::Result<String> {
    let path = path.replace('\\', "/");
    let value = Path::new(&path);
    if path.is_empty()
        || value.is_absolute()
        || value.components().any(|component| {
            matches!(
                component,
                std::path::Component::ParentDir | std::path::Component::RootDir
            )
        })
        || path.starts_with('\\')
        || path.as_bytes().get(1).is_some_and(|byte| *byte == b':')
    {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            format!("invalid {role} path `{path}`"),
        ));
    }
    let normalized = value
        .components()
        .filter_map(|component| match component {
            std::path::Component::Normal(segment) => segment.to_str(),
            _ => None,
        })
        .collect::<Vec<_>>()
        .join("/");
    Ok(if normalized.is_empty() {
        ".".to_owned()
    } else {
        normalized
    })
}

fn expand_workspace_paths(root: &Path, patterns: &[String], role: &str) -> io::Result<Vec<String>> {
    let mut matches = BTreeSet::new();
    for pattern in patterns {
        let pattern = normalize_workspace_path(pattern, role)?;
        let has_wildcard = pattern
            .bytes()
            .any(|byte| matches!(byte, b'*' | b'?' | b'['));
        let paths = if has_wildcard {
            let root_text = root.to_str().ok_or_else(|| {
                io::Error::new(
                    io::ErrorKind::InvalidData,
                    format!("{role} pattern is not UTF-8"),
                )
            })?;
            let root_text = if let Some(unc) = root_text.strip_prefix(r"\\?\UNC\") {
                format!("//{unc}")
            } else {
                root_text
                    .strip_prefix(r"\\?\")
                    .unwrap_or(root_text)
                    .to_owned()
            };
            let full_pattern = format!(
                "{}/{}",
                glob::Pattern::escape(&root_text.replace('\\', "/")),
                pattern
            );
            glob::glob(&full_pattern)
                .map_err(|error| io::Error::new(io::ErrorKind::InvalidData, error))?
                .take(MAX_MANIFEST_ENTRIES + 1)
                .map(|path| path.map_err(|error| io::Error::new(io::ErrorKind::InvalidData, error)))
                .collect::<io::Result<Vec<_>>>()?
        } else if pattern == "." {
            vec![root.to_path_buf()]
        } else {
            vec![root.join(&pattern)]
        };
        if paths.len() > MAX_MANIFEST_ENTRIES {
            return Err(io::Error::new(
                io::ErrorKind::InvalidData,
                "workspace pattern expands to too many paths",
            ));
        }
        let mut found = false;
        for path in paths {
            match open_cache_root(&path, false) {
                Ok(_) => {}
                Err(error) if error.kind() == io::ErrorKind::NotFound => continue,
                Err(error) => {
                    return Err(io::Error::new(
                        error.kind(),
                        format!("{role} `{pattern}` is not a real directory: {error}"),
                    ));
                }
            }
            match read_raw_manifest(&path) {
                Ok(_) => {}
                Err(error) if error.kind() == io::ErrorKind::NotFound => continue,
                Err(error) => return Err(error),
            }
            let canonical = canonical_real_directory(&path)?;
            let relative = canonical.strip_prefix(root).map_err(|_| {
                io::Error::new(
                    io::ErrorKind::InvalidData,
                    format!("{role} `{pattern}` resolves outside the workspace root"),
                )
            })?;
            let relative = relative.to_string_lossy().replace('\\', "/");
            let relative = if relative.is_empty() {
                ".".to_owned()
            } else {
                relative
            };
            matches.insert(relative);
            if matches.len() > MAX_MANIFEST_ENTRIES {
                return Err(io::Error::new(
                    io::ErrorKind::InvalidData,
                    "workspace has too many members",
                ));
            }
            found = true;
        }
        if !found {
            return Err(io::Error::new(
                io::ErrorKind::InvalidData,
                format!("{role} pattern `{pattern}` does not match a package manifest"),
            ));
        }
    }
    Ok(matches.into_iter().collect())
}

fn workspace_path_excluded(path: &str, patterns: &[String]) -> bool {
    patterns.iter().any(|pattern| {
        glob::Pattern::new(pattern).is_ok_and(|pattern| {
            Path::new(path).ancestors().any(|ancestor| {
                pattern.matches_path_with(
                    ancestor,
                    glob::MatchOptions {
                        require_literal_separator: true,
                        ..glob::MatchOptions::new()
                    },
                )
            })
        })
    })
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct RawPackageManifest {
    #[serde(default)]
    package: Option<RawPackageIdentity>,
    #[serde(default)]
    workspace: Option<RawWorkspace>,
    #[serde(default)]
    source: RawPackageSource,
    #[serde(default)]
    library: RawPackageLibrary,
    #[serde(default)]
    registries: BTreeMap<String, String>,
    #[serde(default, rename = "build-dependencies")]
    build_dependencies: BTreeMap<String, RawDependency>,
    #[serde(default)]
    dependencies: BTreeMap<String, RawDependency>,
    #[serde(default, rename = "dev-dependencies")]
    dev_dependencies: BTreeMap<String, RawDependency>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct RawPackageIdentity {
    name: String,
    #[serde(default)]
    namespace: Option<Inheritable<String>>,
    #[serde(default)]
    version: Option<Inheritable<String>>,
    #[serde(default, rename = "protocol-epoch")]
    protocol_epoch: Option<Inheritable<u32>>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct RawWorkspace {
    #[serde(default)]
    members: Vec<String>,
    #[serde(default, rename = "default-members")]
    default_members: Option<Vec<String>>,
    #[serde(default)]
    exclude: Vec<String>,
    #[serde(default)]
    package: Option<RawWorkspacePackage>,
    #[serde(default)]
    dependencies: BTreeMap<String, RawDependency>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct RawWorkspacePackage {
    #[serde(default)]
    namespace: Option<String>,
    #[serde(default)]
    version: Option<String>,
    #[serde(default, rename = "protocol-epoch")]
    protocol_epoch: Option<u32>,
}

#[derive(Deserialize)]
#[serde(untagged, deny_unknown_fields)]
enum Inheritable<T> {
    Value(T),
    Workspace { workspace: bool },
}

#[derive(Deserialize, Default)]
#[serde(deny_unknown_fields)]
struct RawPackageSource {
    #[serde(default)]
    roots: Vec<String>,
    #[serde(default)]
    exclude: Vec<String>,
}

#[derive(Deserialize, Default)]
#[serde(deny_unknown_fields)]
struct RawPackageLibrary {
    #[serde(default)]
    path: Option<String>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct RawDependency {
    #[serde(default)]
    workspace: Option<bool>,
    #[serde(default)]
    path: Option<String>,
    #[serde(default)]
    git: Option<String>,
    #[serde(default)]
    registry: Option<String>,
    #[serde(default)]
    package: Option<String>,
    #[serde(default)]
    version: Option<String>,
    #[serde(default)]
    rev: Option<String>,
    #[serde(default)]
    tag: Option<String>,
    #[serde(default)]
    branch: Option<String>,
}

impl RawDependency {
    fn validate_workspace_reference(&self, name: &str) -> io::Result<()> {
        if self.path.is_some()
            || self.git.is_some()
            || self.registry.is_some()
            || self.package.is_some()
            || self.version.is_some()
            || self.rev.is_some()
            || self.tag.is_some()
            || self.branch.is_some()
        {
            return Err(io::Error::new(
                io::ErrorKind::InvalidData,
                format!(
                    "dependency `{name}` cannot combine workspace inheritance with source fields"
                ),
            ));
        }
        Ok(())
    }

    fn into_source(self, name: &str) -> io::Result<DependencySource> {
        if let Some(registry) = self.registry {
            if self.path.is_some()
                || self.git.is_some()
                || self.rev.is_some()
                || self.tag.is_some()
                || self.branch.is_some()
            {
                return Err(io::Error::new(
                    io::ErrorKind::InvalidData,
                    format!("registry dependency `{name}` cannot combine sources or Git selectors"),
                ));
            }
            let package = self.package.as_deref().ok_or_else(|| {
                io::Error::new(
                    io::ErrorKind::InvalidData,
                    format!("registry dependency `{name}` must declare a `namespace/name` package"),
                )
            })?;
            let (namespace, package_name) = package.split_once('/').ok_or_else(|| {
                io::Error::new(
                    io::ErrorKind::InvalidData,
                    format!("registry package for dependency `{name}` must use `namespace/name`"),
                )
            })?;
            validate_registry_package_segment(namespace, name)?;
            validate_registry_package_segment(package_name, name)?;
            if package_name.contains('/') {
                return Err(io::Error::new(
                    io::ErrorKind::InvalidData,
                    format!("registry package for dependency `{name}` must use `namespace/name`"),
                ));
            }
            let package = RegistryPackageName {
                namespace: namespace.to_owned(),
                name: package_name.to_owned(),
            };
            if registry.is_empty()
                || registry.len() > 64
                || !registry
                    .bytes()
                    .all(|byte| byte.is_ascii_lowercase() || byte.is_ascii_digit() || byte == b'-')
            {
                return Err(io::Error::new(
                    io::ErrorKind::InvalidData,
                    format!("invalid registry name for dependency `{name}`"),
                ));
            }
            let requirement = self
                .version
                .as_deref()
                .ok_or_else(|| {
                    io::Error::new(
                        io::ErrorKind::InvalidData,
                        format!("registry dependency `{name}` must declare a version requirement"),
                    )
                })?
                .parse::<PackageVersionRequirement>()
                .map_err(|error| {
                    io::Error::new(
                        io::ErrorKind::InvalidData,
                        format!("invalid version requirement for dependency `{name}`: {error}"),
                    )
                })?;
            return Ok(DependencySource::Registry {
                registry,
                package,
                requirement,
            });
        }
        if self.version.is_some() || self.package.is_some() {
            return Err(io::Error::new(
                io::ErrorKind::InvalidData,
                format!(
                    "dependency `{name}` may only specify `package` and `version` with `registry`"
                ),
            ));
        }
        match (self.path, self.git) {
            (Some(path), None)
                if self.rev.is_none() && self.tag.is_none() && self.branch.is_none() =>
            {
                if path.is_empty()
                    || Path::new(&path).is_absolute()
                    || path.starts_with('/')
                    || path.starts_with('\\')
                    || path.as_bytes().get(1).is_some_and(|byte| *byte == b':')
                {
                    return Err(io::Error::new(
                        io::ErrorKind::InvalidData,
                        format!("invalid path source for dependency `{name}`"),
                    ));
                }
                Ok(DependencySource::Path(path))
            }
            (None, Some(url)) => {
                let selector_count = [
                    self.rev.is_some(),
                    self.tag.is_some(),
                    self.branch.is_some(),
                ]
                .into_iter()
                .filter(|selected| *selected)
                .count();
                let scp_style = url.starts_with("git@")
                    && url
                        .split_once(':')
                        .is_some_and(|(_, path)| !path.is_empty() && !path.starts_with('/'));
                if (!scp_style
                    && !matches!(
                        url.split_once(":").map(|(scheme, _)| scheme),
                        Some("https" | "http" | "ssh" | "git" | "file")
                    ))
                    || selector_count > 1
                    || (!scp_style
                        && matches!(
                            url.split_once("://").map(|(scheme, _)| scheme),
                            Some("http" | "https" | "git")
                        )
                        && url
                            .split_once("://")
                            .and_then(|(_, rest)| rest.split('/').next())
                            .is_some_and(|authority| authority.contains('@')))
                {
                    return Err(io::Error::new(
                        io::ErrorKind::InvalidData,
                        format!("invalid Git source for dependency `{name}`"),
                    ));
                }
                if let Some(rev) = self.rev.as_deref() {
                    validate_git_revision(rev, name)?;
                }
                if let Some(tag) = self.tag.as_deref() {
                    validate_git_ref(tag, "tag", name)?;
                }
                if let Some(branch) = self.branch.as_deref() {
                    validate_git_ref(branch, "branch", name)?;
                }
                Ok(DependencySource::Git {
                    url: if scp_style {
                        let (host, path) = url.split_once(':').expect("scp-style URL has a colon");
                        format!("ssh://{host}/{path}")
                    } else {
                        url
                    },
                    rev: self.rev,
                    tag: self.tag,
                    branch: self.branch,
                })
            }
            _ => Err(io::Error::new(
                io::ErrorKind::InvalidData,
                format!("dependency `{name}` must specify exactly one of `path` or `git`"),
            )),
        }
    }
}

fn validate_registry_package_segment(segment: &str, dependency: &str) -> io::Result<()> {
    if segment.is_empty()
        || segment.len() > 128
        || !segment
            .bytes()
            .all(|byte| byte.is_ascii_lowercase() || byte.is_ascii_digit() || byte == b'-')
    {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            format!("invalid registry package identity for dependency `{dependency}`"),
        ));
    }
    Ok(())
}

fn validate_git_revision(revision: &str, name: &str) -> io::Result<()> {
    if revision.len() != 40 || !revision.bytes().all(|byte| byte.is_ascii_hexdigit()) {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            format!("Git revision for dependency `{name}` must be a full hexadecimal commit"),
        ));
    }
    Ok(())
}

fn validate_git_ref(value: &str, kind: &str, name: &str) -> io::Result<()> {
    if value.is_empty()
        || value.len() > 1024
        || value.bytes().any(|byte| byte < 0x20 || byte == 0x7f)
        || value.contains(['\\', ':', '?', '*', '[', '^'])
        || value.contains("..")
        || value.contains("@{")
        || value.starts_with('/')
        || value.ends_with('/')
        || value.ends_with('.')
    {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            format!("invalid Git {kind} selector for dependency `{name}`"),
        ));
    }
    Ok(())
}

fn validate_dependency_name(name: &str) -> io::Result<()> {
    if name.is_empty()
        || name.len() > 128
        || !name
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'_' | b'-'))
    {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            "invalid dependency name",
        ));
    }
    Ok(())
}

/// Names the dependency chain (`json/regex`) that a package graph error came
/// from. The root package is reported by its caller, so its errors pass through.
fn dependency_context(logical_root: &str, error: io::Error) -> io::Error {
    if logical_root.is_empty() {
        return error;
    }
    io::Error::new(
        error.kind(),
        format!("dependency `{logical_root}`: {error}"),
    )
}

/// Formats a canonical path for diagnostics without the Windows verbatim
/// prefix that `canonicalize` adds.
fn display_path(path: &Path) -> String {
    let text = path.display().to_string();
    if let Some(share) = text.strip_prefix(r"\\?\UNC\") {
        return format!(r"\\{share}");
    }
    text.strip_prefix(r"\\?\")
        .map(str::to_owned)
        .unwrap_or(text)
}

fn validate_package_id(package: &PackageId) -> io::Result<()> {
    if package.namespace.is_empty()
        || package.name.is_empty()
        || package.namespace.len() > 128
        || package.name.len() > 128
        || package.namespace.contains('/')
        || package.name.contains('/')
        || package.version.as_str().len() > 128
    {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            format!(
                "invalid package identity: version {:?} must be a valid SemVer 2.0.0 version",
                package.version
            ),
        ));
    }
    Ok(())
}

impl Default for PackageSourceSelection {
    fn default() -> Self {
        Self {
            source_roots: vec![".".into()],
            excludes: Vec::new(),
        }
    }
}

/// Computes a deterministic source identity from a package directory.
///
/// `target`, cache, temporary, VCS, and installation directories are excluded
/// by default. Symlinks and non-regular files are ignored rather than
/// following host-specific targets into the package identity.
pub fn package_source_identity(
    root: &Path,
    package: PackageId,
) -> io::Result<PackageSourceIdentity> {
    package_source_identity_with_selection(root, package, &PackageSourceSelection::default())
}

/// Computes a source identity using explicit relative roots and exclusions.
pub fn package_source_identity_with_selection(
    root: &Path,
    package: PackageId,
    selection: &PackageSourceSelection,
) -> io::Result<PackageSourceIdentity> {
    let root = canonical_real_directory(root)?;
    let mut selected_roots = selection
        .source_roots
        .iter()
        .map(|value| validate_selection_path(value))
        .collect::<io::Result<Vec<_>>>()?;
    selected_roots.sort();
    selected_roots.dedup();
    if selected_roots.is_empty() {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            "package has no source roots",
        ));
    }
    let mut excludes = selection
        .excludes
        .iter()
        .map(|value| validate_selection_path(value))
        .collect::<io::Result<BTreeSet<_>>>()?;
    if regular_file_exists(&root.join("nia.toml"), "package manifest")?
        && read_raw_manifest(&root)?.workspace.is_some()
    {
        let workspace = WorkspaceManifest::read(&root)?;
        for member in workspace.members {
            if member != "." {
                excludes.insert(validate_selection_path(&member)?);
            }
        }
    }
    let root_directory = open_cache_root(&root, false)?;
    let mut files = Vec::new();
    let mut seen_files = BTreeSet::new();
    for source_root in &selected_roots {
        let Some(directory) = open_selected_source_root(&root_directory, source_root)? else {
            continue;
        };
        collect_source_files(
            &directory,
            if source_root == "." { "" } else { source_root },
            &excludes,
            &mut files,
            &mut seen_files,
        )?;
    }
    files.sort_by(|left, right| left.path.cmp(&right.path));
    files.dedup_by(|left, right| left.path == right.path);
    let mut encoded = Vec::new();
    encoded.extend_from_slice(PACKAGE_SOURCE_DOMAIN.as_str().as_bytes());
    encoded.extend_from_slice(b"\0\x01");
    encoded.extend_from_slice(package.canonical_text().as_bytes());
    encoded.push(0);
    encoded.extend_from_slice(&(selected_roots.len() as u64).to_le_bytes());
    for source_root in &selected_roots {
        encoded.extend_from_slice(&(source_root.len() as u64).to_le_bytes());
        encoded.extend_from_slice(source_root.as_bytes());
    }
    encoded.extend_from_slice(&(excludes.len() as u64).to_le_bytes());
    for exclude in &excludes {
        encoded.extend_from_slice(&(exclude.len() as u64).to_le_bytes());
        encoded.extend_from_slice(exclude.as_bytes());
    }
    for file in &files {
        encoded.extend_from_slice(&(file.path.len() as u64).to_le_bytes());
        encoded.extend_from_slice(file.path.as_bytes());
        encoded.extend_from_slice(&file.byte_len.to_le_bytes());
        encoded.extend_from_slice(&file.fingerprint);
    }
    Ok(PackageSourceIdentity {
        package,
        source_fingerprint: *blake3::hash(&encoded).as_bytes(),
        files,
    })
}

fn open_selected_source_root(root: &Dir, source_root: &str) -> io::Result<Option<Dir>> {
    if source_root == "." {
        return root.try_clone().map(Some);
    }
    let mut directory = root.try_clone()?;
    for component in Path::new(source_root).components() {
        let std::path::Component::Normal(name) = component else {
            continue;
        };
        let metadata = match directory.symlink_metadata(name) {
            Ok(metadata) => metadata,
            Err(error) if error.kind() == io::ErrorKind::NotFound => return Err(error),
            Err(error) => return Err(error),
        };
        if metadata.is_symlink() {
            return Ok(None);
        }
        if !metadata.is_dir() {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                "package source root must be a directory",
            ));
        }
        let name = name.to_str().ok_or_else(|| {
            io::Error::new(
                io::ErrorKind::InvalidInput,
                "package source selection paths must be valid UTF-8",
            )
        })?;
        directory = match open_cache_directory(&directory, name, false) {
            Ok(directory) => directory,
            Err(error) if error.kind() == io::ErrorKind::InvalidData => return Ok(None),
            Err(error) => return Err(error),
        };
    }
    Ok(Some(directory))
}

fn validate_selection_path(value: &str) -> io::Result<String> {
    let path = Path::new(value);
    if value.is_empty() || path.is_absolute() || value.contains(':') || value.contains('\\') {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            "package source selection paths must be normalized relative paths",
        ));
    }
    let mut components = Vec::new();
    for component in path.components() {
        match component {
            std::path::Component::Normal(component) => {
                let component = component.to_str().ok_or_else(|| {
                    io::Error::new(
                        io::ErrorKind::InvalidInput,
                        "package source selection paths must be valid UTF-8",
                    )
                })?;
                components.push(component);
            }
            std::path::Component::CurDir => {}
            std::path::Component::ParentDir
            | std::path::Component::RootDir
            | std::path::Component::Prefix(_) => {
                return Err(io::Error::new(
                    io::ErrorKind::InvalidInput,
                    "package source selection paths must be normalized relative paths",
                ));
            }
        }
    }
    if components.is_empty() {
        Ok(".".into())
    } else {
        Ok(components.join("/"))
    }
}

fn collect_source_files(
    directory: &Dir,
    prefix: &str,
    excludes: &BTreeSet<String>,
    files: &mut Vec<PackageSourceFile>,
    seen_files: &mut BTreeSet<String>,
) -> io::Result<()> {
    let mut pending = vec![(directory.try_clone()?, prefix.to_owned())];
    while let Some((directory, prefix)) = pending.pop() {
        for entry in directory.entries()? {
            let entry = entry?;
            let entry_name = entry.file_name();
            let entry_name_text = entry_name.to_str().ok_or_else(|| {
                io::Error::new(
                    io::ErrorKind::InvalidData,
                    "package source paths must be valid UTF-8",
                )
            })?;
            let name = if prefix.is_empty() {
                entry_name_text.to_owned()
            } else {
                format!("{prefix}/{entry_name_text}")
            };
            let relative = Path::new(&name);
            if excluded_source_entry(relative)
                || excludes
                    .iter()
                    .any(|exclude| name == *exclude || name.starts_with(&format!("{exclude}/")))
            {
                continue;
            }
            let metadata = directory.symlink_metadata(&entry_name)?;
            if metadata.is_symlink() {
                continue;
            }
            if metadata.is_dir() {
                let child = match open_cache_directory(&directory, entry_name_text, false) {
                    Ok(child) => child,
                    Err(error) if error.kind() == io::ErrorKind::NotFound => continue,
                    Err(error) if error.kind() == io::ErrorKind::InvalidData => continue,
                    Err(error) => {
                        let current = directory.symlink_metadata(&entry_name)?;
                        if current.is_symlink() {
                            continue;
                        }
                        return Err(error);
                    }
                };
                pending.push((child, name));
            } else if metadata.is_file() {
                if !seen_files.insert(name.clone()) {
                    continue;
                }
                if files.len() >= MAX_SOURCE_FILES {
                    return Err(io::Error::new(
                        io::ErrorKind::InvalidData,
                        "package source contains too many files",
                    ));
                }
                let Some(file) = open_source_file(&directory, &entry_name)? else {
                    continue;
                };
                let (byte_len, fingerprint) = fingerprint_source_file(file)?;
                files.push(PackageSourceFile {
                    path: name,
                    byte_len,
                    fingerprint,
                });
            }
        }
    }
    Ok(())
}

fn open_source_file(directory: &Dir, name: &OsStr) -> io::Result<Option<File>> {
    let mut options = CapOpenOptions::new();
    options.read(true);
    #[cfg(unix)]
    {
        use cap_std::fs::OpenOptionsExt as _;
        options.custom_flags(libc::O_NOFOLLOW);
    }
    #[cfg(windows)]
    {
        use cap_std::fs::OpenOptionsExt as _;
        use windows_sys::Win32::Storage::FileSystem::{
            FILE_FLAG_OPEN_REPARSE_POINT, FILE_SHARE_DELETE, FILE_SHARE_READ, FILE_SHARE_WRITE,
        };
        options
            .custom_flags(FILE_FLAG_OPEN_REPARSE_POINT)
            .share_mode(FILE_SHARE_READ | FILE_SHARE_WRITE | FILE_SHARE_DELETE);
    }
    let file = match directory.open_with(name, &options) {
        Ok(file) => file.into_std(),
        Err(error) => {
            if let Ok(metadata) = directory.symlink_metadata(name)
                && metadata.is_symlink()
            {
                return Ok(None);
            }
            return Err(error);
        }
    };
    let metadata = file.metadata()?;
    if nia_compat::is_link_or_reparse_point(&metadata) || !metadata.is_file() {
        return Ok(None);
    }
    Ok(Some(file))
}

fn fingerprint_source_file(mut file: File) -> io::Result<(u64, [u8; 32])> {
    let mut hasher = blake3::Hasher::new();
    let mut buffer = [0; 64 * 1024];
    let mut byte_len = 0u64;
    loop {
        let read = file.read(&mut buffer)?;
        if read == 0 {
            break;
        }
        byte_len = byte_len.checked_add(read as u64).ok_or_else(|| {
            io::Error::new(io::ErrorKind::InvalidData, "source file is too large")
        })?;
        hasher.update(&buffer[..read]);
    }
    Ok((byte_len, *hasher.finalize().as_bytes()))
}

/// Whether a package-relative entry is skipped by default.
///
/// VCS, Nia state, and lock files are never package content. Conventional
/// output directories of other tools are skipped only directly under the
/// package root: a nested `build/` or `target/` can hold child modules, which
/// the identity must cover because they are compiled. A selected source root
/// is never itself checked, so `roots = ["build"]` still selects that tree.
fn excluded_source_entry(relative: &Path) -> bool {
    static EXCLUDED_ANYWHERE: &[&str] = &[".git", ".nia", ".nia-build", ".nia-cache", "nia.lock"];
    static EXCLUDED_AT_ROOT: &[&str] = &["target", "build", "dist"];
    let Some(name) = relative.file_name().map(|name| name.to_string_lossy()) else {
        return false;
    };
    EXCLUDED_ANYWHERE.contains(&name.as_ref())
        || (relative.components().count() == 1 && EXCLUDED_AT_ROOT.contains(&name.as_ref()))
}

/// Relocation-independent identity of one module within a package.
#[derive(Debug, Clone, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct ModuleId {
    pub package: PackageId,
    pub path: String,
}

/// Stable identity of a definition within a package module.
#[derive(Debug, Clone, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct DefinitionId {
    pub module: ModuleId,
    pub name: String,
    pub kind: u8,
    /// Stable session-independent declaration token. Definitions with the
    /// same module/name/kind/owner (for example overloaded methods) retain
    /// distinct identities through this field.
    pub disambiguator: u64,
    /// Canonical containing definition for nested members.
    pub owner: Option<Box<DefinitionId>>,
}

/// Encodes one stable module identity for embedding in another versioned
/// compiler-owned section. The enclosing section owns framing and schema;
/// this function owns the canonical package/module representation.
pub fn encode_module_id(module: &ModuleId) -> Result<Vec<u8>, MetadataError> {
    let mut output = Vec::new();
    put_module_id(&mut output, module)?;
    Ok(output)
}

/// Decodes one complete embedded stable module identity.
pub fn decode_module_id(bytes: &[u8]) -> Result<ModuleId, MetadataError> {
    if bytes.len() > MAX_STABLE_METADATA_BYTES {
        return Err(MetadataError::TooLarge);
    }
    let mut cursor = Cursor::new(bytes);
    let module = read_module_id(&mut cursor)?;
    if cursor.position() != bytes.len() as u64 {
        return Err(MetadataError::InvalidManifest);
    }
    Ok(module)
}

/// Encodes one stable definition identity for embedding in another versioned
/// compiler-owned section, including its complete canonical owner chain.
pub fn encode_definition_id(definition: &DefinitionId) -> Result<Vec<u8>, MetadataError> {
    validate_definition(definition)?;
    let mut output = Vec::new();
    put_definition(&mut output, definition)?;
    Ok(output)
}

/// Decodes one complete embedded stable definition identity and validates its
/// package, module, name, kind, disambiguator, and bounded owner chain.
pub fn decode_definition_id(bytes: &[u8]) -> Result<DefinitionId, MetadataError> {
    if bytes.len() > MAX_STABLE_METADATA_BYTES {
        return Err(MetadataError::TooLarge);
    }
    let mut cursor = Cursor::new(bytes);
    let definition = read_definition(&mut cursor)?;
    if cursor.position() != bytes.len() as u64 {
        return Err(MetadataError::InvalidManifest);
    }
    validate_definition(&definition)?;
    Ok(definition)
}

/// Stable const-generic argument used by applied nominal type nodes.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct StableConstArg {
    pub ty: u32,
    pub value: StableConstValue,
}

/// Relocation-independent value of a const-generic argument.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub enum StableConstValue {
    GenericParam(u64),
    Integer { bits: u128, signed: bool },
    Bool(bool),
    Char(char),
}

/// Stable identity of a source-defined or compiler-provided trait.
#[derive(Debug, Clone, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub enum StableTraitId {
    Source(DefinitionId),
    Builtin(u8),
}

/// Associated type binding carried by a stable trait object or projection.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct StableAssociatedTypeBinding {
    pub trait_id: Option<StableTraitId>,
    pub trait_arguments: Vec<u32>,
    pub trait_const_arguments: Vec<StableConstArg>,
    pub name: u64,
    pub ty: u32,
}

/// Stable owner substitutions for a materialized closure state.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct StableClosureOwnerInstance {
    pub arg_module: ModuleId,
    pub self_arg: Option<u32>,
    pub args: Vec<u32>,
    pub const_args: Vec<StableConstArg>,
}

/// Canonical array length expression accepted by package interfaces.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub enum StableArrayLength {
    ConstValue(u64),
    GenericParam(u64),
}

/// Relocation-independent type node used by stable compiler interfaces.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum StableTypeNode {
    /// Primitive type tag owned by the language ABI.
    Primitive(u8),
    /// Named declaration resolved through a package-stable identity.
    Named(DefinitionId),
    /// Named declaration applied to earlier type-graph arguments.
    NamedApplied {
        definition: DefinitionId,
        arguments: Vec<u32>,
        const_arguments: Vec<StableConstArg>,
    },
    /// Unit type.
    Unit,
    /// Never type.
    Never,
    /// Tuple whose elements refer to earlier nodes.
    Tuple(Vec<u32>),
    /// Array with a bounded constant length.
    Array {
        element: u32,
        length: StableArrayLength,
    },
    /// Function signature with parameter and result node references.
    Function {
        parameters: Vec<u32>,
        result: u32,
        /// Whether indirect calls use the target C calling convention.
        is_extern: bool,
    },
    /// Borrow/reference to an earlier node.
    Reference { target: u32, mutable: bool },
    /// Raw pointer to an earlier node.
    Pointer { target: u32, readonly: bool },
    /// Generic parameter identified by the stable symbol identity of its
    /// declaration name. Symbol identities are content-addressed and do not
    /// contain session-local handles.
    GenericParam(u64),
    /// Error recovery type.
    Error,
    /// Compile-time-only type.
    ConstOnly,
    /// Opaque type with intentionally hidden representation.
    Opaque,
    /// Volatile pointer to an earlier node.
    VolatilePointer { target: u32, readonly: bool },
    /// Fat slice value.
    Slice { target: u32, readonly: bool },
    /// Unsized slice pointee.
    SlicePointee { target: u32 },
    /// Fixed-width SIMD vector (primitive lane tag plus lane count).
    Vector { element: u8, lanes: u32 },
    /// Range value with an optional bound node.
    Range { kind: u8, bound: Option<u32> },
    /// Optional value wrapper.
    Optional { element: u32 },
    /// Error/value union wrapper.
    ErrorUnion { error: u32, value: u32 },
    /// Readonly or mutable callable closure view.
    Callable {
        parameters: Vec<u32>,
        result: u32,
        readonly: bool,
    },
    /// Unsized callable state pointee.
    CallablePointee { parameters: Vec<u32>, result: u32 },
    /// Unresolved `Self` parameter.
    SelfParam,
    /// Compiler-provided builtin nominal type.
    BuiltinType(u8),
    /// Compiler-provided builtin trait application.
    BuiltinTrait { trait_id: u8, arguments: Vec<u32> },
    /// Sized trait-object value.
    TraitObject {
        readonly: bool,
        trait_id: StableTraitId,
        trait_arguments: Vec<u32>,
        trait_const_arguments: Vec<StableConstArg>,
        associated_type_bindings: Vec<StableAssociatedTypeBinding>,
    },
    /// Unsized trait-object pointee.
    TraitObjectPointee {
        trait_id: StableTraitId,
        trait_arguments: Vec<u32>,
        trait_const_arguments: Vec<StableConstArg>,
        associated_type_bindings: Vec<StableAssociatedTypeBinding>,
    },
    /// Associated type projection before normalization.
    Projection {
        self_ty: u32,
        trait_id: StableTraitId,
        trait_arguments: Vec<u32>,
        trait_const_arguments: Vec<StableConstArg>,
        name: u64,
    },
    /// Anonymous closure state owned by a stable function definition.
    ClosureState {
        owner: DefinitionId,
        ordinal: u32,
        owner_instance: Option<StableClosureOwnerInstance>,
        captures: Vec<u32>,
        parameters: Vec<u32>,
        result: u32,
    },
}

/// Canonical, bounded type graph for stable cross-package type identities.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StableTypeGraph {
    pub nodes: Vec<StableTypeNode>,
    pub roots: Vec<u32>,
}

impl StableTypeGraph {
    /// Validates bounds, node references, and canonical root ordering.
    pub fn validate(&self) -> Result<(), MetadataError> {
        if self.nodes.len() > MAX_ITEMS || self.roots.len() > MAX_ITEMS {
            return Err(MetadataError::TooManyItems);
        }
        let node_count = self.nodes.len() as u32;
        if self.roots.windows(2).any(|pair| pair[0] >= pair[1])
            || self.roots.iter().any(|root| *root >= node_count)
        {
            return Err(MetadataError::InvalidManifest);
        }
        for (index, node) in self.nodes.iter().enumerate() {
            let index = index as u32;
            let mut references = Vec::new();
            match node {
                StableTypeNode::Primitive(tag) if *tag == 0 => {
                    return Err(MetadataError::InvalidManifest);
                }
                StableTypeNode::Primitive(_)
                | StableTypeNode::Unit
                | StableTypeNode::Never
                | StableTypeNode::Error
                | StableTypeNode::ConstOnly
                | StableTypeNode::Opaque
                | StableTypeNode::SelfParam
                | StableTypeNode::BuiltinType(_) => {}
                StableTypeNode::Named(definition) => validate_definition(definition)?,
                StableTypeNode::NamedApplied {
                    definition,
                    arguments,
                    const_arguments,
                } => {
                    validate_definition(definition)?;
                    references.extend(arguments);
                    validate_stable_const_arguments(const_arguments)?;
                    references.extend(const_arguments.iter().map(|argument| &argument.ty));
                }
                StableTypeNode::Tuple(elements) => references.extend(elements),
                StableTypeNode::Array { element, length } => {
                    references.push(element);
                    if let StableArrayLength::GenericParam(_) = length {
                        // Generic parameter identities are self-contained.
                    }
                }
                StableTypeNode::Function {
                    parameters, result, ..
                } => {
                    references.extend(parameters);
                    references.push(result);
                }
                StableTypeNode::Reference { target, .. } => references.push(target),
                StableTypeNode::Pointer { target, .. } => references.push(target),
                StableTypeNode::VolatilePointer { target, .. }
                | StableTypeNode::Slice { target, .. }
                | StableTypeNode::SlicePointee { target }
                | StableTypeNode::Optional { element: target } => references.push(target),
                StableTypeNode::ErrorUnion { error, value } => {
                    references.push(error);
                    references.push(value);
                }
                StableTypeNode::Range { kind, bound } => {
                    if *kind > 5 {
                        return Err(MetadataError::InvalidManifest);
                    }
                    if let Some(bound) = bound {
                        references.push(bound);
                    }
                }
                StableTypeNode::Vector { element, lanes } => {
                    if !(1..=16).contains(element) || *lanes == 0 {
                        return Err(MetadataError::InvalidManifest);
                    }
                }
                StableTypeNode::Callable {
                    parameters, result, ..
                }
                | StableTypeNode::CallablePointee { parameters, result } => {
                    references.extend(parameters);
                    references.push(result);
                }
                StableTypeNode::BuiltinTrait {
                    trait_id,
                    arguments,
                } => {
                    if nia_ids::BuiltinTrait::from_stable_tag(u32::from(*trait_id)).is_none() {
                        return Err(MetadataError::InvalidManifest);
                    }
                    references.extend(arguments);
                }
                StableTypeNode::TraitObject {
                    trait_id,
                    trait_arguments,
                    trait_const_arguments,
                    associated_type_bindings,
                    ..
                }
                | StableTypeNode::TraitObjectPointee {
                    trait_id,
                    trait_arguments,
                    trait_const_arguments,
                    associated_type_bindings,
                } => {
                    validate_stable_trait_id(trait_id)?;
                    references.extend(trait_arguments);
                    validate_stable_const_arguments(trait_const_arguments)?;
                    references.extend(trait_const_arguments.iter().map(|argument| &argument.ty));
                    if associated_type_bindings.len() > MAX_ITEMS {
                        return Err(MetadataError::TooManyItems);
                    }
                    for binding in associated_type_bindings {
                        if let Some(trait_id) = &binding.trait_id {
                            validate_stable_trait_id(trait_id)?;
                        }
                        references.extend(&binding.trait_arguments);
                        validate_stable_const_arguments(&binding.trait_const_arguments)?;
                        references.extend(
                            binding
                                .trait_const_arguments
                                .iter()
                                .map(|argument| &argument.ty),
                        );
                        references.push(&binding.ty);
                    }
                }
                StableTypeNode::Projection {
                    self_ty,
                    trait_id,
                    trait_arguments,
                    trait_const_arguments,
                    ..
                } => {
                    validate_stable_trait_id(trait_id)?;
                    references.push(self_ty);
                    references.extend(trait_arguments);
                    validate_stable_const_arguments(trait_const_arguments)?;
                    references.extend(trait_const_arguments.iter().map(|argument| &argument.ty));
                }
                StableTypeNode::ClosureState {
                    owner,
                    owner_instance,
                    captures,
                    parameters,
                    result,
                    ..
                } => {
                    validate_definition(owner)?;
                    if let Some(instance) = owner_instance {
                        validate_module_id(&instance.arg_module)?;
                        references.extend(&instance.self_arg);
                        references.extend(&instance.args);
                        validate_stable_const_arguments(&instance.const_args)?;
                        references.extend(instance.const_args.iter().map(|arg| &arg.ty));
                    }
                    references.extend(captures);
                    references.extend(parameters);
                    references.push(result);
                }
                StableTypeNode::GenericParam(_) => {}
            }
            if references.iter().any(|reference| **reference >= index) {
                return Err(MetadataError::InvalidManifest);
            }
            if references.len() > MAX_ITEMS {
                return Err(MetadataError::TooManyItems);
            }
        }
        Ok(())
    }
}

fn validate_stable_trait_id(trait_id: &StableTraitId) -> Result<(), MetadataError> {
    match trait_id {
        StableTraitId::Source(definition) => validate_definition(definition),
        StableTraitId::Builtin(tag)
            if nia_ids::BuiltinTrait::from_stable_tag(u32::from(*tag)).is_some() =>
        {
            Ok(())
        }
        StableTraitId::Builtin(_) => Err(MetadataError::InvalidManifest),
    }
}

fn validate_stable_const_arguments(arguments: &[StableConstArg]) -> Result<(), MetadataError> {
    if arguments.len() > MAX_ITEMS {
        Err(MetadataError::TooManyItems)
    } else {
        Ok(())
    }
}

/// Encodes and validates a canonical stable type graph.
pub fn encode_type_graph(graph: &StableTypeGraph) -> Result<Vec<u8>, MetadataError> {
    graph.validate()?;
    let mut output = Vec::new();
    output.extend_from_slice(TYPE_GRAPH_MAGIC);
    put_u32(&mut output, TYPE_GRAPH_SCHEMA);
    put_list_len(&mut output, graph.nodes.len())?;
    put_list_len(&mut output, graph.roots.len())?;
    for root in &graph.roots {
        put_u32(&mut output, *root);
    }
    for node in &graph.nodes {
        match node {
            StableTypeNode::Primitive(tag) => {
                output.push(1);
                output.push(*tag);
            }
            StableTypeNode::Named(definition) => {
                output.push(2);
                put_definition(&mut output, definition)?;
            }
            StableTypeNode::NamedApplied {
                definition,
                arguments,
                const_arguments,
            } => {
                output.push(11);
                put_definition(&mut output, definition)?;
                put_list_len(&mut output, arguments.len())?;
                for argument in arguments {
                    put_u32(&mut output, *argument);
                }
                put_list_len(&mut output, const_arguments.len())?;
                for argument in const_arguments {
                    put_stable_const_arg(&mut output, argument);
                }
            }
            StableTypeNode::Unit => output.push(3),
            StableTypeNode::Never => output.push(4),
            StableTypeNode::Tuple(elements) => {
                output.push(5);
                put_list_len(&mut output, elements.len())?;
                for element in elements {
                    put_u32(&mut output, *element);
                }
            }
            StableTypeNode::Array { element, length } => {
                output.push(6);
                put_u32(&mut output, *element);
                match length {
                    StableArrayLength::ConstValue(value) => {
                        output.push(0);
                        output.extend_from_slice(&value.to_le_bytes());
                    }
                    StableArrayLength::GenericParam(hash) => {
                        output.push(1);
                        output.extend_from_slice(&hash.to_le_bytes());
                    }
                }
            }
            StableTypeNode::Function {
                parameters,
                result,
                is_extern,
            } => {
                output.push(7);
                put_list_len(&mut output, parameters.len())?;
                for parameter in parameters {
                    put_u32(&mut output, *parameter);
                }
                put_u32(&mut output, *result);
                output.push(u8::from(*is_extern));
            }
            StableTypeNode::Reference { target, mutable } => {
                output.push(8);
                put_u32(&mut output, *target);
                output.push(u8::from(*mutable));
            }
            StableTypeNode::Pointer { target, readonly } => {
                output.push(9);
                put_u32(&mut output, *target);
                output.push(u8::from(*readonly));
            }
            StableTypeNode::GenericParam(index) => {
                output.push(10);
                output.extend_from_slice(&index.to_le_bytes());
            }
            StableTypeNode::Error => output.push(12),
            StableTypeNode::ConstOnly => output.push(13),
            StableTypeNode::Opaque => output.push(14),
            StableTypeNode::VolatilePointer { target, readonly } => {
                output.push(15);
                put_u32(&mut output, *target);
                output.push(u8::from(*readonly));
            }
            StableTypeNode::Slice { target, readonly } => {
                output.push(16);
                put_u32(&mut output, *target);
                output.push(u8::from(*readonly));
            }
            StableTypeNode::SlicePointee { target } => {
                output.push(17);
                put_u32(&mut output, *target);
            }
            StableTypeNode::Vector { element, lanes } => {
                output.push(18);
                output.push(*element);
                put_u32(&mut output, *lanes);
            }
            StableTypeNode::Range { kind, bound } => {
                output.push(19);
                output.push(*kind);
                match bound {
                    Some(bound) => {
                        output.push(1);
                        put_u32(&mut output, *bound);
                    }
                    None => output.push(0),
                }
            }
            StableTypeNode::Optional { element } => {
                output.push(20);
                put_u32(&mut output, *element);
            }
            StableTypeNode::ErrorUnion { error, value } => {
                output.push(21);
                put_u32(&mut output, *error);
                put_u32(&mut output, *value);
            }
            StableTypeNode::Callable {
                parameters,
                result,
                readonly,
            } => {
                output.push(22);
                put_list_len(&mut output, parameters.len())?;
                for parameter in parameters {
                    put_u32(&mut output, *parameter);
                }
                put_u32(&mut output, *result);
                output.push(u8::from(*readonly));
            }
            StableTypeNode::CallablePointee { parameters, result } => {
                output.push(23);
                put_list_len(&mut output, parameters.len())?;
                for parameter in parameters {
                    put_u32(&mut output, *parameter);
                }
                put_u32(&mut output, *result);
            }
            StableTypeNode::SelfParam => output.push(24),
            StableTypeNode::BuiltinType(tag) => {
                output.push(25);
                output.push(*tag);
            }
            StableTypeNode::BuiltinTrait {
                trait_id,
                arguments,
            } => {
                output.push(26);
                output.push(*trait_id);
                put_list_len(&mut output, arguments.len())?;
                for argument in arguments {
                    put_u32(&mut output, *argument);
                }
            }
            StableTypeNode::TraitObject {
                readonly,
                trait_id,
                trait_arguments,
                trait_const_arguments,
                associated_type_bindings,
            } => {
                output.push(27);
                output.push(u8::from(*readonly));
                put_stable_trait_id(&mut output, trait_id)?;
                put_refs(&mut output, trait_arguments)?;
                put_stable_const_args(&mut output, trait_const_arguments)?;
                put_stable_associated_bindings(&mut output, associated_type_bindings)?;
            }
            StableTypeNode::TraitObjectPointee {
                trait_id,
                trait_arguments,
                trait_const_arguments,
                associated_type_bindings,
            } => {
                output.push(28);
                put_stable_trait_id(&mut output, trait_id)?;
                put_refs(&mut output, trait_arguments)?;
                put_stable_const_args(&mut output, trait_const_arguments)?;
                put_stable_associated_bindings(&mut output, associated_type_bindings)?;
            }
            StableTypeNode::Projection {
                self_ty,
                trait_id,
                trait_arguments,
                trait_const_arguments,
                name,
            } => {
                output.push(29);
                put_u32(&mut output, *self_ty);
                put_stable_trait_id(&mut output, trait_id)?;
                put_refs(&mut output, trait_arguments)?;
                put_stable_const_args(&mut output, trait_const_arguments)?;
                output.extend_from_slice(&name.to_le_bytes());
            }
            StableTypeNode::ClosureState {
                owner,
                ordinal,
                owner_instance,
                captures,
                parameters,
                result,
            } => {
                output.push(30);
                put_definition(&mut output, owner)?;
                put_u32(&mut output, *ordinal);
                match owner_instance {
                    Some(instance) => {
                        output.push(1);
                        put_module_id(&mut output, &instance.arg_module)?;
                        match instance.self_arg {
                            Some(ty) => {
                                output.push(1);
                                put_u32(&mut output, ty);
                            }
                            None => output.push(0),
                        }
                        put_refs(&mut output, &instance.args)?;
                        put_stable_const_args(&mut output, &instance.const_args)?;
                    }
                    None => output.push(0),
                }
                put_refs(&mut output, captures)?;
                put_refs(&mut output, parameters)?;
                put_u32(&mut output, *result);
            }
        }
    }
    if output.len() > MAX_STABLE_METADATA_BYTES {
        return Err(MetadataError::TooLarge);
    }
    Ok(output)
}

/// Decodes and validates a canonical stable type graph.
pub fn decode_type_graph(bytes: &[u8]) -> Result<StableTypeGraph, MetadataError> {
    if bytes.len() > MAX_STABLE_METADATA_BYTES {
        return Err(MetadataError::TooLarge);
    }
    let mut cursor = Cursor::new(bytes);
    let mut magic = [0; 8];
    read_exact(&mut cursor, &mut magic)?;
    if magic != *TYPE_GRAPH_MAGIC {
        return Err(MetadataError::BadMagic);
    }
    let schema = get_u32(&mut cursor)?;
    if schema != TYPE_GRAPH_SCHEMA {
        return Err(MetadataError::Schema(schema));
    }
    let node_count = bounded_count(get_u32(&mut cursor)?)?;
    let root_count = bounded_count(get_u32(&mut cursor)?)?;
    let roots = (0..root_count)
        .map(|_| get_u32(&mut cursor))
        .collect::<Result<Vec<_>, _>>()?;
    let mut nodes = Vec::with_capacity(node_count);
    for _ in 0..node_count {
        let tag = read_u8(&mut cursor)?;
        nodes.push(match tag {
            1 => StableTypeNode::Primitive(read_u8(&mut cursor)?),
            2 => StableTypeNode::Named(read_definition(&mut cursor)?),
            11 => StableTypeNode::NamedApplied {
                definition: read_definition(&mut cursor)?,
                arguments: read_refs(&mut cursor)?,
                const_arguments: {
                    let count = bounded_count(get_u32(&mut cursor)?)?;
                    (0..count)
                        .map(|_| read_stable_const_arg(&mut cursor))
                        .collect::<Result<Vec<_>, _>>()?
                },
            },
            3 => StableTypeNode::Unit,
            4 => StableTypeNode::Never,
            5 => StableTypeNode::Tuple(read_refs(&mut cursor)?),
            6 => StableTypeNode::Array {
                element: get_u32(&mut cursor)?,
                length: match read_u8(&mut cursor)? {
                    0 => StableArrayLength::ConstValue(get_u64(&mut cursor)?),
                    1 => StableArrayLength::GenericParam(get_u64(&mut cursor)?),
                    _ => return Err(MetadataError::InvalidManifest),
                },
            },
            7 => StableTypeNode::Function {
                parameters: read_refs(&mut cursor)?,
                result: get_u32(&mut cursor)?,
                is_extern: match read_u8(&mut cursor)? {
                    0 => false,
                    1 => true,
                    _ => return Err(MetadataError::InvalidManifest),
                },
            },
            8 => StableTypeNode::Reference {
                target: get_u32(&mut cursor)?,
                mutable: match read_u8(&mut cursor)? {
                    0 => false,
                    1 => true,
                    _ => return Err(MetadataError::InvalidManifest),
                },
            },
            9 => StableTypeNode::Pointer {
                target: get_u32(&mut cursor)?,
                readonly: match read_u8(&mut cursor)? {
                    0 => false,
                    1 => true,
                    _ => return Err(MetadataError::InvalidManifest),
                },
            },
            10 => StableTypeNode::GenericParam(get_u64(&mut cursor)?),
            12 => StableTypeNode::Error,
            13 => StableTypeNode::ConstOnly,
            14 => StableTypeNode::Opaque,
            15 => StableTypeNode::VolatilePointer {
                target: get_u32(&mut cursor)?,
                readonly: match read_u8(&mut cursor)? {
                    0 => false,
                    1 => true,
                    _ => return Err(MetadataError::InvalidManifest),
                },
            },
            16 => StableTypeNode::Slice {
                target: get_u32(&mut cursor)?,
                readonly: match read_u8(&mut cursor)? {
                    0 => false,
                    1 => true,
                    _ => return Err(MetadataError::InvalidManifest),
                },
            },
            17 => StableTypeNode::SlicePointee {
                target: get_u32(&mut cursor)?,
            },
            18 => StableTypeNode::Vector {
                element: read_u8(&mut cursor)?,
                lanes: get_u32(&mut cursor)?,
            },
            19 => StableTypeNode::Range {
                kind: read_u8(&mut cursor)?,
                bound: match read_u8(&mut cursor)? {
                    0 => None,
                    1 => Some(get_u32(&mut cursor)?),
                    _ => return Err(MetadataError::InvalidManifest),
                },
            },
            20 => StableTypeNode::Optional {
                element: get_u32(&mut cursor)?,
            },
            21 => StableTypeNode::ErrorUnion {
                error: get_u32(&mut cursor)?,
                value: get_u32(&mut cursor)?,
            },
            22 => StableTypeNode::Callable {
                parameters: read_refs(&mut cursor)?,
                result: get_u32(&mut cursor)?,
                readonly: match read_u8(&mut cursor)? {
                    0 => false,
                    1 => true,
                    _ => return Err(MetadataError::InvalidManifest),
                },
            },
            23 => StableTypeNode::CallablePointee {
                parameters: read_refs(&mut cursor)?,
                result: get_u32(&mut cursor)?,
            },
            24 => StableTypeNode::SelfParam,
            25 => {
                let tag = read_u8(&mut cursor)?;
                if tag > 2 {
                    return Err(MetadataError::InvalidManifest);
                }
                StableTypeNode::BuiltinType(tag)
            }
            26 => StableTypeNode::BuiltinTrait {
                trait_id: {
                    let tag = read_u8(&mut cursor)?;
                    if nia_ids::BuiltinTrait::from_stable_tag(u32::from(tag)).is_none() {
                        return Err(MetadataError::InvalidManifest);
                    }
                    tag
                },
                arguments: read_refs(&mut cursor)?,
            },
            27 => StableTypeNode::TraitObject {
                readonly: match read_u8(&mut cursor)? {
                    0 => false,
                    1 => true,
                    _ => return Err(MetadataError::InvalidManifest),
                },
                trait_id: read_stable_trait_id(&mut cursor)?,
                trait_arguments: read_refs(&mut cursor)?,
                trait_const_arguments: read_stable_const_args(&mut cursor)?,
                associated_type_bindings: read_stable_associated_bindings(&mut cursor)?,
            },
            28 => StableTypeNode::TraitObjectPointee {
                trait_id: read_stable_trait_id(&mut cursor)?,
                trait_arguments: read_refs(&mut cursor)?,
                trait_const_arguments: read_stable_const_args(&mut cursor)?,
                associated_type_bindings: read_stable_associated_bindings(&mut cursor)?,
            },
            29 => StableTypeNode::Projection {
                self_ty: get_u32(&mut cursor)?,
                trait_id: read_stable_trait_id(&mut cursor)?,
                trait_arguments: read_refs(&mut cursor)?,
                trait_const_arguments: read_stable_const_args(&mut cursor)?,
                name: get_u64(&mut cursor)?,
            },
            30 => StableTypeNode::ClosureState {
                owner: read_definition(&mut cursor)?,
                ordinal: get_u32(&mut cursor)?,
                owner_instance: match read_u8(&mut cursor)? {
                    0 => None,
                    1 => Some(StableClosureOwnerInstance {
                        arg_module: read_module_id(&mut cursor)?,
                        self_arg: match read_u8(&mut cursor)? {
                            0 => None,
                            1 => Some(get_u32(&mut cursor)?),
                            _ => return Err(MetadataError::InvalidManifest),
                        },
                        args: read_refs(&mut cursor)?,
                        const_args: read_stable_const_args(&mut cursor)?,
                    }),
                    _ => return Err(MetadataError::InvalidManifest),
                },
                captures: read_refs(&mut cursor)?,
                parameters: read_refs(&mut cursor)?,
                result: get_u32(&mut cursor)?,
            },
            _ => return Err(MetadataError::InvalidManifest),
        });
    }
    if cursor.position() != bytes.len() as u64 {
        return Err(MetadataError::InvalidManifest);
    }
    let graph = StableTypeGraph { nodes, roots };
    graph.validate()?;
    Ok(graph)
}

/// Errors returned by stable metadata validation and decoding.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum MetadataError {
    BadMagic,
    Schema(u32),
    TooLarge,
    TooManyItems,
    Truncated,
    InvalidString,
    InvalidManifest,
    Io,
}
impl std::fmt::Display for MetadataError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        if let Self::Schema(schema) = self {
            return write!(f, "unsupported stable metadata schema version {schema}");
        }
        let message = match self {
            Self::BadMagic => "invalid stable metadata header",
            Self::Schema(_) => unreachable!("schema errors are formatted above"),
            Self::TooLarge => "stable metadata exceeds the size limit",
            Self::TooManyItems => "stable metadata contains too many items",
            Self::Truncated => "stable metadata is truncated",
            Self::InvalidString => "stable metadata contains invalid text",
            Self::InvalidManifest => "stable metadata manifest is invalid",
            Self::Io => "could not read stable metadata",
        };
        f.write_str(message)
    }
}
impl std::error::Error for MetadataError {}

fn validate_id(id: &PackageId) -> Result<(), MetadataError> {
    validate_string(&id.namespace)?;
    validate_string(&id.name)?;
    validate_string(id.version.as_str())?;
    if id.version.as_str().len() > 128 {
        return Err(MetadataError::InvalidString);
    }
    Ok(())
}

fn validate_definition(definition: &DefinitionId) -> Result<(), MetadataError> {
    validate_definition_with_depth(definition, 0)
}

fn validate_module_id(module: &ModuleId) -> Result<(), MetadataError> {
    validate_id(&module.package)?;
    validate_string(&module.path)
}

fn validate_definition_with_depth(
    definition: &DefinitionId,
    depth: usize,
) -> Result<(), MetadataError> {
    if depth >= MAX_DEFINITION_DEPTH {
        return Err(MetadataError::TooManyItems);
    }
    validate_id(&definition.module.package)?;
    validate_string(&definition.module.path)?;
    validate_string(&definition.name)?;
    if let Some(owner) = &definition.owner {
        if owner.module != definition.module || owner.as_ref() == definition {
            return Err(MetadataError::InvalidManifest);
        }
        validate_definition_with_depth(owner, depth + 1)?;
    }
    (definition.kind != 0)
        .then_some(())
        .ok_or(MetadataError::InvalidManifest)
}
fn validate_string(value: &str) -> Result<(), MetadataError> {
    if value.is_empty() || value.len() > MAX_STRING_BYTES || value.as_bytes().contains(&0) {
        Err(MetadataError::InvalidString)
    } else {
        Ok(())
    }
}

fn put_id(output: &mut Vec<u8>, id: &PackageId) -> Result<(), MetadataError> {
    put_string(output, &id.namespace)?;
    put_string(output, &id.name)?;
    put_string(output, id.version.as_str())
}
fn put_module_id(output: &mut Vec<u8>, module: &ModuleId) -> Result<(), MetadataError> {
    validate_module_id(module)?;
    put_id(output, &module.package)?;
    put_string(output, &module.path)
}
fn read_module_id(cursor: &mut Cursor<&[u8]>) -> Result<ModuleId, MetadataError> {
    let module = ModuleId {
        package: get_id(cursor)?,
        path: get_string(cursor)?,
    };
    validate_module_id(&module)?;
    Ok(module)
}
fn put_definition(output: &mut Vec<u8>, definition: &DefinitionId) -> Result<(), MetadataError> {
    validate_definition(definition)?;
    put_id(output, &definition.module.package)?;
    put_string(output, &definition.module.path)?;
    put_string(output, &definition.name)?;
    output.push(definition.kind);
    output.extend_from_slice(&definition.disambiguator.to_le_bytes());
    match &definition.owner {
        Some(owner) => {
            output.push(1);
            put_definition(output, owner)?;
        }
        None => output.push(0),
    }
    Ok(())
}
fn read_definition(cursor: &mut Cursor<&[u8]>) -> Result<DefinitionId, MetadataError> {
    read_definition_with_depth(cursor, 0)
}

fn read_definition_with_depth(
    cursor: &mut Cursor<&[u8]>,
    depth: usize,
) -> Result<DefinitionId, MetadataError> {
    if depth >= MAX_DEFINITION_DEPTH {
        return Err(MetadataError::TooManyItems);
    }
    let definition = DefinitionId {
        module: ModuleId {
            package: get_id(cursor)?,
            path: get_string(cursor)?,
        },
        name: get_string(cursor)?,
        kind: read_u8(cursor)?,
        disambiguator: get_u64(cursor)?,
        owner: match read_u8(cursor)? {
            0 => None,
            1 => Some(Box::new(read_definition_with_depth(cursor, depth + 1)?)),
            _ => return Err(MetadataError::InvalidManifest),
        },
    };
    validate_definition(&definition)?;
    Ok(definition)
}
fn read_refs(cursor: &mut Cursor<&[u8]>) -> Result<Vec<u32>, MetadataError> {
    let count = bounded_count(get_u32(cursor)?)?;
    (0..count).map(|_| get_u32(cursor)).collect()
}

fn put_refs(output: &mut Vec<u8>, refs: &[u32]) -> Result<(), MetadataError> {
    put_list_len(output, refs.len())?;
    for reference in refs {
        put_u32(output, *reference);
    }
    Ok(())
}

fn put_stable_const_arg(output: &mut Vec<u8>, argument: &StableConstArg) {
    put_u32(output, argument.ty);
    match &argument.value {
        StableConstValue::GenericParam(hash) => {
            output.push(0);
            output.extend_from_slice(&hash.to_le_bytes());
        }
        StableConstValue::Integer { bits, signed } => {
            output.push(1);
            output.extend_from_slice(&bits.to_le_bytes());
            output.push(u8::from(*signed));
        }
        StableConstValue::Bool(value) => {
            output.push(2);
            output.push(u8::from(*value));
        }
        StableConstValue::Char(value) => {
            output.push(3);
            output.extend_from_slice(&u32::from(*value).to_le_bytes());
        }
    }
}

fn put_stable_const_args(
    output: &mut Vec<u8>,
    arguments: &[StableConstArg],
) -> Result<(), MetadataError> {
    put_list_len(output, arguments.len())?;
    for argument in arguments {
        put_stable_const_arg(output, argument);
    }
    Ok(())
}

fn read_stable_const_arg(cursor: &mut Cursor<&[u8]>) -> Result<StableConstArg, MetadataError> {
    let ty = get_u32(cursor)?;
    let value = match read_u8(cursor)? {
        0 => StableConstValue::GenericParam(get_u64(cursor)?),
        1 => StableConstValue::Integer {
            bits: get_u128(cursor)?,
            signed: match read_u8(cursor)? {
                0 => false,
                1 => true,
                _ => return Err(MetadataError::InvalidManifest),
            },
        },
        2 => StableConstValue::Bool(match read_u8(cursor)? {
            0 => false,
            1 => true,
            _ => return Err(MetadataError::InvalidManifest),
        }),
        3 => char::from_u32(get_u32(cursor)?)
            .map(StableConstValue::Char)
            .ok_or(MetadataError::InvalidManifest)?,
        _ => return Err(MetadataError::InvalidManifest),
    };
    Ok(StableConstArg { ty, value })
}

fn read_stable_const_args(
    cursor: &mut Cursor<&[u8]>,
) -> Result<Vec<StableConstArg>, MetadataError> {
    let count = bounded_count(get_u32(cursor)?)?;
    (0..count).map(|_| read_stable_const_arg(cursor)).collect()
}

fn put_stable_trait_id(
    output: &mut Vec<u8>,
    trait_id: &StableTraitId,
) -> Result<(), MetadataError> {
    match trait_id {
        StableTraitId::Source(definition) => {
            output.push(0);
            put_definition(output, definition)?;
        }
        StableTraitId::Builtin(tag) => {
            output.push(1);
            output.push(*tag);
        }
    }
    Ok(())
}

fn read_stable_trait_id(cursor: &mut Cursor<&[u8]>) -> Result<StableTraitId, MetadataError> {
    match read_u8(cursor)? {
        0 => Ok(StableTraitId::Source(read_definition(cursor)?)),
        1 => {
            let tag = read_u8(cursor)?;
            nia_ids::BuiltinTrait::from_stable_tag(u32::from(tag))
                .is_some()
                .then_some(StableTraitId::Builtin(tag))
                .ok_or(MetadataError::InvalidManifest)
        }
        _ => Err(MetadataError::InvalidManifest),
    }
}

fn put_stable_associated_bindings(
    output: &mut Vec<u8>,
    bindings: &[StableAssociatedTypeBinding],
) -> Result<(), MetadataError> {
    put_list_len(output, bindings.len())?;
    for binding in bindings {
        match &binding.trait_id {
            Some(trait_id) => {
                output.push(1);
                put_stable_trait_id(output, trait_id)?;
            }
            None => output.push(0),
        }
        put_refs(output, &binding.trait_arguments)?;
        put_stable_const_args(output, &binding.trait_const_arguments)?;
        output.extend_from_slice(&binding.name.to_le_bytes());
        put_u32(output, binding.ty);
    }
    Ok(())
}

fn read_stable_associated_bindings(
    cursor: &mut Cursor<&[u8]>,
) -> Result<Vec<StableAssociatedTypeBinding>, MetadataError> {
    let count = bounded_count(get_u32(cursor)?)?;
    (0..count)
        .map(|_| {
            let trait_id = match read_u8(cursor)? {
                0 => None,
                1 => Some(read_stable_trait_id(cursor)?),
                _ => return Err(MetadataError::InvalidManifest),
            };
            Ok(StableAssociatedTypeBinding {
                trait_id,
                trait_arguments: read_refs(cursor)?,
                trait_const_arguments: read_stable_const_args(cursor)?,
                name: get_u64(cursor)?,
                ty: get_u32(cursor)?,
            })
        })
        .collect()
}

fn get_id(cursor: &mut Cursor<&[u8]>) -> Result<PackageId, MetadataError> {
    Ok(PackageId {
        namespace: get_string(cursor)?,
        name: get_string(cursor)?,
        version: get_string(cursor)?
            .parse()
            .map_err(|_| MetadataError::InvalidManifest)?,
    })
}
fn put_string(output: &mut Vec<u8>, value: &str) -> Result<(), MetadataError> {
    validate_string(value)?;
    put_u32(
        output,
        u32::try_from(value.len()).map_err(|_| MetadataError::TooLarge)?,
    );
    output.extend_from_slice(value.as_bytes());
    Ok(())
}

fn get_string(cursor: &mut Cursor<&[u8]>) -> Result<String, MetadataError> {
    let length = bounded_len(get_u32(cursor)?)?;
    if length == 0 {
        return Err(MetadataError::InvalidString);
    }
    let mut bytes = vec![0; length];
    read_exact(cursor, &mut bytes)?;
    String::from_utf8(bytes).map_err(|_| MetadataError::InvalidString)
}

fn put_list_len(output: &mut Vec<u8>, count: usize) -> Result<(), MetadataError> {
    if count > MAX_ITEMS {
        return Err(MetadataError::TooManyItems);
    }
    put_u32(
        output,
        u32::try_from(count).map_err(|_| MetadataError::TooManyItems)?,
    );
    Ok(())
}
fn bounded_len(value: u32) -> Result<usize, MetadataError> {
    let value = usize::try_from(value).map_err(|_| MetadataError::TooLarge)?;
    (value <= MAX_STABLE_METADATA_BYTES)
        .then_some(value)
        .ok_or(MetadataError::TooLarge)
}
fn bounded_count(value: u32) -> Result<usize, MetadataError> {
    let value = usize::try_from(value).map_err(|_| MetadataError::TooManyItems)?;
    (value <= MAX_ITEMS)
        .then_some(value)
        .ok_or(MetadataError::TooManyItems)
}
fn put_u32(output: &mut Vec<u8>, value: u32) {
    output.extend_from_slice(&value.to_le_bytes());
}
fn get_u32(cursor: &mut Cursor<&[u8]>) -> Result<u32, MetadataError> {
    let mut bytes = [0; 4];
    read_exact(cursor, &mut bytes)?;
    Ok(u32::from_le_bytes(bytes))
}
fn get_u64(cursor: &mut Cursor<&[u8]>) -> Result<u64, MetadataError> {
    let mut bytes = [0; 8];
    read_exact(cursor, &mut bytes)?;
    Ok(u64::from_le_bytes(bytes))
}
fn get_u128(cursor: &mut Cursor<&[u8]>) -> Result<u128, MetadataError> {
    let mut bytes = [0; 16];
    read_exact(cursor, &mut bytes)?;
    Ok(u128::from_le_bytes(bytes))
}
fn read_u8(cursor: &mut Cursor<&[u8]>) -> Result<u8, MetadataError> {
    let mut byte = [0; 1];
    read_exact(cursor, &mut byte)?;
    Ok(byte[0])
}
fn read_exact(cursor: &mut Cursor<&[u8]>, bytes: &mut [u8]) -> Result<(), MetadataError> {
    cursor.read_exact(bytes).map_err(|error| {
        if error.kind() == io::ErrorKind::UnexpectedEof {
            MetadataError::Truncated
        } else {
            MetadataError::Io
        }
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs::{self, OpenOptions};

    #[test]
    fn registry_package_identity_cannot_mix_registry_sources() {
        let package = RegistryPackageName {
            namespace: "acme".into(),
            name: "json".into(),
        };
        let mut sources = BTreeMap::new();

        record_registry_source(&mut sources, &package, "public").unwrap();
        record_registry_source(&mut sources, &package, "public").unwrap();
        let error = record_registry_source(&mut sources, &package, "internal").unwrap_err();

        assert_eq!(error.kind(), io::ErrorKind::InvalidData);
        assert!(error.to_string().contains("acme/json"));
        assert!(error.to_string().contains("`public` and `internal`"));
    }

    #[test]
    fn metadata_errors_use_semantic_messages() {
        assert_eq!(
            MetadataError::BadMagic.to_string(),
            "invalid stable metadata header"
        );
        assert_eq!(
            MetadataError::Schema(99).to_string(),
            "unsupported stable metadata schema version 99"
        );
        assert_eq!(
            MetadataError::InvalidManifest.to_string(),
            "stable metadata manifest is invalid"
        );
        assert!(!MetadataError::Schema(99).to_string().contains("Schema("));
    }

    fn package() -> PackageId {
        PackageId {
            namespace: "example".into(),
            name: "identity".into(),
            version: "1.0.0".parse().unwrap(),
        }
    }

    #[test]
    fn package_canonical_text_escapes_component_delimiters_injectively() {
        let first = PackageId {
            namespace: "acme".into(),
            name: "lib@tools".into(),
            version: "1.0.0".parse().unwrap(),
        };
        let second = PackageId {
            namespace: "acme".into(),
            name: "lib".into(),
            version: "1.0.0".parse().unwrap(),
        };

        assert_eq!(first.canonical_text(), "acme/lib%40tools@1.0.0");
        assert_eq!(second.canonical_text(), "acme/lib@1.0.0");
        assert_ne!(first.canonical_text(), second.canonical_text());

        let escaped = PackageId {
            namespace: "acme/team".into(),
            name: "%40@\0\u{7f}".into(),
            version: "1.0.0".parse().unwrap(),
        };
        assert_eq!(escaped.canonical_text(), "acme%2Fteam/%2540%40%00%7F@1.0.0");
        assert_ne!(canonical_component("%40"), canonical_component("@"));
    }

    #[test]
    fn package_identity_requires_strict_semver_for_every_source() {
        for version in ["0.3.0-dev", "1.2.3+metadata", "1.2.3-alpha.1"] {
            let id = PackageId {
                namespace: "example".into(),
                name: "valid".into(),
                version: PackageVersion::parse(version).unwrap(),
            };
            assert!(validate_package_id(&id).is_ok(), "{version}");
            assert!(validate_id(&id).is_ok(), "{version}");
        }

        for version in ["", "1", "1.2", "01.2.3", "1.2.3-01", "v1.2.3"] {
            assert!(PackageVersion::parse(version).is_err(), "{version:?}");
        }

        assert!(Version::parse(PackageId::standard_library().version.as_str()).is_ok());

        let first = PackageVersion::parse("1.2.3+one").unwrap();
        let second = PackageVersion::parse("1.2.3+two").unwrap();
        assert_ne!(first, second);
        assert_ne!(first.cmp(&second), Ordering::Equal);
    }

    #[test]
    fn package_version_requirements_follow_semver_prerelease_rules() {
        let requirement = PackageVersionRequirement::parse("^1.4").unwrap();
        assert!(requirement.matches(&PackageVersion::parse("1.4.0").unwrap()));
        assert!(requirement.matches(&PackageVersion::parse("1.9.2").unwrap()));
        assert!(!requirement.matches(&PackageVersion::parse("2.0.0").unwrap()));
        assert!(!requirement.matches(&PackageVersion::parse("1.5.0-beta.1").unwrap()));

        let prerelease = PackageVersionRequirement::parse("=1.5.0-beta.1").unwrap();
        assert!(prerelease.matches(&PackageVersion::parse("1.5.0-beta.1").unwrap()));
        assert!(!prerelease.matches(&PackageVersion::parse("1.5.0-beta.2").unwrap()));
        assert!(PackageVersionRequirement::parse("1.4").is_err());
        assert!(PackageVersionRequirement::parse("1.4.*").is_ok());
        assert!(PackageVersionRequirement::parse("not a requirement").is_err());
    }

    #[test]
    fn lockfile_round_trips_and_rejects_unknown_schema() {
        let root = std::env::temp_dir().join(format!("nia-lock-roundtrip-{}", std::process::id()));
        let _ = fs::remove_dir_all(&root);
        fs::create_dir_all(&root).unwrap();
        let path = root.join("nia.lock");
        let lock = PackageLock {
            version: LOCKFILE_VERSION,
            package: vec![LockedPackage {
                name: "regex".into(),
                source: LockedSource::Git {
                    url: "https://example.com/regex.git".into(),
                    selector: "branch:main".into(),
                    revision: "0123456789012345678901234567890123456789".into(),
                    source_fingerprint: format!("blake3:{}", "a".repeat(64)),
                },
            }],
        };
        lock.write_atomic(&path).unwrap();
        assert_eq!(PackageLock::read(&path).unwrap(), lock);
        fs::write(&path, "version = 99\n").unwrap();
        assert_eq!(
            PackageLock::read(&path).unwrap_err().kind(),
            io::ErrorKind::InvalidData
        );
        fs::write(&path, "version = 1\npackage = []\n").unwrap();
        assert!(
            PackageLock::read(&path).is_err(),
            "lock schema v1 is obsolete"
        );
        fs::write(
            &path,
            r#"version = 2
[[package]]
name = "regex"
[package.source]
kind = "git"
url = "https://example.com/regex.git/"
selector = "branch:main"
revision = "0123456789012345678901234567890123456789"
source_fingerprint = "blake3:aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa"
"#,
        )
        .unwrap();
        assert!(
            PackageLock::read(&path).is_err(),
            "Git URLs must be canonical"
        );
        fs::write(
            &path,
            r#"version = 2
[[package]]
name = "regex"
[package.source]
kind = "path"
owner = "local/app@1.0.0"
path = "/outside"
namespace = "local"
package = "regex"
version = "1.0.0"
source_fingerprint = "blake3:aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa"
"#,
        )
        .unwrap();
        assert!(
            PackageLock::read(&path).is_err(),
            "path lock roots must be relative"
        );
        let _ = fs::remove_dir_all(root);
    }

    #[cfg(unix)]
    #[test]
    fn lockfile_rejects_symlinked_reads_and_writes() {
        use std::os::unix::fs::symlink;

        let root = std::env::temp_dir().join(format!(
            "nia-lock-symlink-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        fs::create_dir_all(&root).unwrap();
        let target = root.join("outside.lock");
        let link = root.join("nia.lock");
        fs::write(&target, "version = 2\npackage = []\n").unwrap();
        symlink(&target, &link).unwrap();

        assert!(PackageLock::read(&link).is_err());
        assert!(PackageLock::default().write_atomic(&link).is_err());
        assert_eq!(
            fs::read_to_string(target).unwrap(),
            "version = 2\npackage = []\n"
        );

        fs::remove_file(link).unwrap();
        fs::remove_dir_all(root).unwrap();
    }

    #[cfg(windows)]
    #[test]
    fn lockfile_rejects_junctioned_reads_and_writes() {
        let root = std::env::temp_dir().join(format!(
            "nia-lock-junction-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        let outside = root.join("outside");
        let link = root.join("nia.lock");
        fs::create_dir_all(&outside).unwrap();
        let output = std::process::Command::new("cmd")
            .args(["/d", "/c", "mklink", "/J"])
            .arg(&link)
            .arg(&outside)
            .output()
            .unwrap();
        assert!(output.status.success(), "{output:?}");

        assert!(PackageLock::read(&link).is_err());
        assert!(PackageLock::default().write_atomic(&link).is_err());
        let parent_link = root.join("linked-parent");
        let output = std::process::Command::new("cmd")
            .args(["/d", "/c", "mklink", "/J"])
            .arg(&parent_link)
            .arg(&outside)
            .output()
            .unwrap();
        assert!(output.status.success(), "{output:?}");
        assert!(PackageLock::read(&parent_link.join("nia.lock")).is_err());
        assert!(
            PackageLock::default()
                .write_atomic(&parent_link.join("nia.lock"))
                .is_err()
        );
        assert!(fs::read_dir(&outside).unwrap().next().is_none());

        fs::remove_dir(parent_link).unwrap();
        fs::remove_dir(link).unwrap();
        fs::remove_dir_all(root).unwrap();
    }

    #[cfg(unix)]
    #[test]
    fn lockfile_rejects_symlinked_parent_writes() {
        use std::os::unix::fs::symlink;

        let root = std::env::temp_dir().join(format!(
            "nia-lock-parent-link-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        let outside = root.join("outside");
        let link = root.join("linked-parent");
        fs::create_dir_all(&outside).unwrap();
        symlink(&outside, &link).unwrap();

        assert!(PackageLock::read(&link.join("nia.lock")).is_err());
        assert!(
            PackageLock::default()
                .write_atomic(&link.join("nia.lock"))
                .is_err()
        );
        assert!(fs::read_dir(&outside).unwrap().next().is_none());

        fs::remove_file(link).unwrap();
        fs::remove_dir_all(root).unwrap();
    }

    #[cfg(unix)]
    #[test]
    fn package_state_lock_rejects_linked_directories_and_files() {
        use std::os::unix::fs::symlink;

        let root = std::env::temp_dir().join(format!(
            "nia-state-lock-links-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        let outside = root.join("outside");
        let linked_directory = root.join("linked-state");
        fs::create_dir_all(&outside).unwrap();
        symlink(&outside, &linked_directory).unwrap();
        assert!(open_package_state_lock(&linked_directory.join("packages.lock")).is_err());
        assert!(fs::read_dir(&outside).unwrap().next().is_none());

        let target = outside.join("lock-target");
        let linked_file = root.join("linked.lock");
        fs::write(&target, b"untouched").unwrap();
        symlink(&target, &linked_file).unwrap();
        assert!(open_package_state_lock(&linked_file).is_err());
        assert_eq!(fs::read(&target).unwrap(), b"untouched");

        fs::remove_file(linked_file).unwrap();
        fs::remove_file(linked_directory).unwrap();
        fs::remove_dir_all(root).unwrap();
    }

    #[cfg(windows)]
    #[test]
    fn package_state_lock_rejects_junctioned_directories() {
        let root = std::env::temp_dir().join(format!(
            "nia-state-lock-junction-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        let outside = root.join("outside");
        let linked_directory = root.join("linked-state");
        fs::create_dir_all(&outside).unwrap();
        let output = std::process::Command::new("cmd")
            .args(["/d", "/c", "mklink", "/J"])
            .arg(&linked_directory)
            .arg(&outside)
            .output()
            .unwrap();
        assert!(output.status.success(), "{output:?}");

        assert!(open_package_state_lock(&linked_directory.join("packages.lock")).is_err());
        assert!(fs::read_dir(&outside).unwrap().next().is_none());

        fs::remove_dir(linked_directory).unwrap();
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn registry_lock_records_pin_release_archive_and_source_fingerprint() {
        let root = std::env::temp_dir().join(format!(
            "nia-registry-lock-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        fs::create_dir_all(&root).unwrap();
        let path = root.join("nia.lock");
        let entry = LockedPackage {
            name: "json".into(),
            source: LockedSource::Registry {
                registry: "nia".into(),
                namespace: "acme".into(),
                package: "json".into(),
                version: "1.4.2".into(),
                archive_digest: format!("blake3:{}", "a".repeat(64)),
                source_fingerprint: format!("blake3:{}", "b".repeat(64)),
            },
        };
        let lock = PackageLock {
            version: LOCKFILE_VERSION,
            package: vec![entry],
        };
        lock.write_atomic(&path).unwrap();
        assert_eq!(PackageLock::read(&path).unwrap(), lock);
        let mut invalid = lock;
        let LockedSource::Registry { version, .. } = &mut invalid.package[0].source else {
            unreachable!();
        };
        *version = "1.4".into();
        assert!(invalid.write_atomic(&path).is_err());
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn lockfile_rejects_registry_package_coordinates_from_multiple_registries() {
        let entry = |name: &str, registry: &str| LockedPackage {
            name: name.into(),
            source: LockedSource::Registry {
                registry: registry.into(),
                namespace: "acme".into(),
                package: "json".into(),
                version: "1.0.0".into(),
                archive_digest: format!("blake3:{}", "a".repeat(64)),
                source_fingerprint: format!("blake3:{}", "b".repeat(64)),
            },
        };
        let lock = PackageLock {
            version: LOCKFILE_VERSION,
            package: vec![entry("public_json", "nia"), entry("internal_json", "corp")],
        };

        let error = lock.validate(io::ErrorKind::InvalidInput).unwrap_err();

        assert!(
            error
                .to_string()
                .contains("one registry package from multiple registries")
        );
    }

    #[test]
    fn lockfile_rejects_registry_aliases_with_different_content_bindings() {
        let entry = |name: &str, archive_byte: char, source_byte: char| LockedPackage {
            name: name.into(),
            source: LockedSource::Registry {
                registry: "nia".into(),
                namespace: "acme".into(),
                package: "json".into(),
                version: "1.0.0".into(),
                archive_digest: format!("blake3:{}", archive_byte.to_string().repeat(64)),
                source_fingerprint: format!("blake3:{}", source_byte.to_string().repeat(64)),
            },
        };
        let error = PackageLock {
            version: LOCKFILE_VERSION,
            package: vec![entry("json", 'a', 'b'), entry("json_alias", 'c', 'd')],
        }
        .validate(io::ErrorKind::InvalidInput)
        .unwrap_err();
        assert!(
            error
                .to_string()
                .contains("multiple archive or source fingerprints")
        );
    }

    #[test]
    fn lockfile_allows_distinct_selectors_and_rejects_duplicate_identity() {
        let root = std::env::temp_dir().join(format!(
            "nia-lock-selectors-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        fs::create_dir_all(&root).unwrap();
        let path = root.join("nia.lock");
        let entry = |selector: &str| LockedPackage {
            name: "regex".into(),
            source: LockedSource::Git {
                url: "https://example.com/regex.git".into(),
                selector: selector.into(),
                revision: "0123456789012345678901234567890123456789".into(),
                source_fingerprint: format!("blake3:{}", "a".repeat(64)),
            },
        };
        let lock = PackageLock {
            version: LOCKFILE_VERSION,
            package: vec![entry("tag:v1"), entry("branch:main")],
        };
        lock.write_atomic(&path).unwrap();
        assert_eq!(PackageLock::read(&path).unwrap(), lock);

        let duplicate = PackageLock {
            version: LOCKFILE_VERSION,
            package: vec![entry("branch:main"), entry("branch:main")],
        };
        assert_eq!(
            duplicate.write_atomic(&path).unwrap_err().kind(),
            io::ErrorKind::InvalidInput
        );
        let duplicate_text = toml::to_string(&duplicate).unwrap();
        fs::write(&path, duplicate_text).unwrap();
        assert_eq!(
            PackageLock::read(&path).unwrap_err().kind(),
            io::ErrorKind::InvalidData
        );
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn fetching_selector_preserves_other_lock_entries_for_same_source() {
        use std::process::Command;

        let nonce = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let root = std::env::temp_dir().join(format!(
            "nia-fetch-selectors-{}-{nonce}",
            std::process::id()
        ));
        let repository = root.join("upstream");
        let consumer = root.join("consumer");
        fs::create_dir_all(repository.join("src")).unwrap();
        fs::create_dir_all(&consumer).unwrap();
        let git = |args: &[&str]| {
            let output = Command::new("git")
                .args(args)
                .current_dir(&repository)
                .output()
                .expect("Git executable is required for the test fixture");
            assert!(
                output.status.success(),
                "{}",
                String::from_utf8_lossy(&output.stderr)
            );
            String::from_utf8_lossy(&output.stdout).trim().to_string()
        };
        git(&["-c", "init.defaultBranch=main", "init", "-q"]);
        git(&["config", "user.email", "nia-test@example.invalid"]);
        git(&["config", "user.name", "Nia Test"]);
        fs::write(
            repository.join("nia.toml"),
            "[package]\nname = \"dep\"\nversion = \"1.0.0\"\n",
        )
        .unwrap();
        fs::write(repository.join("src/lib.nia"), "pub fun dependency() {}\n").unwrap();
        git(&["add", "nia.toml", "src"]);
        git(&["commit", "-qm", "fixture"]);
        let repository_url = format!(
            "file:///{}",
            repository.to_string_lossy().replace('\\', "/")
        );
        let old_entry = LockedPackage {
            name: "dep".into(),
            source: LockedSource::Git {
                url: repository_url.clone(),
                selector: "tag:previous".into(),
                revision: "0123456789012345678901234567890123456789".into(),
                source_fingerprint: format!("blake3:{}", "b".repeat(64)),
            },
        };
        PackageLock {
            version: LOCKFILE_VERSION,
            package: vec![old_entry.clone()],
        }
        .write_atomic(&consumer.join("nia.lock"))
        .unwrap();

        let dependency = PackageDependency {
            name: "dep".into(),
            source: DependencySource::Git {
                url: repository_url,
                rev: None,
                tag: None,
                branch: Some("main".into()),
            },
            scope: DependencyScope::Runtime,
        };
        let mut fetcher = PackageFetcher::for_root(&consumer, LockPolicy::default()).unwrap();
        fetcher
            .resolve(
                &consumer,
                &PackageManifest::implicit("consumer").package,
                &dependency,
                &[],
            )
            .unwrap();
        assert!(fetcher.lock.package.contains(&old_entry));
        assert!(fetcher.lock.package.iter().any(|entry| {
            entry.name == "dep"
                && matches!(
                    &entry.source,
                    LockedSource::Git { selector, .. } if selector == "branch:main"
                )
        }));

        fs::remove_dir_all(root).unwrap();
    }

    fn definition(name: &str, kind: u8, owner: Option<DefinitionId>) -> DefinitionId {
        DefinitionId {
            module: ModuleId {
                package: package(),
                path: "src/lib.nia".into(),
            },
            name: name.into(),
            kind,
            disambiguator: u64::from(kind),
            owner: owner.map(Box::new),
        }
    }

    #[test]
    fn stable_identity_codecs_round_trip_nested_definitions() {
        let module = ModuleId {
            package: package(),
            path: "src/lib.nia".into(),
        };
        let owner = definition("Container", 5, None);
        let member = definition("field", 6, Some(owner));

        assert_eq!(
            decode_module_id(&encode_module_id(&module).unwrap()).unwrap(),
            module
        );
        assert_eq!(
            decode_definition_id(&encode_definition_id(&member).unwrap()).unwrap(),
            member
        );
    }

    #[test]
    fn stable_identity_codecs_reject_trailing_bytes_and_cross_module_owners() {
        let mut bytes = encode_module_id(&ModuleId {
            package: package(),
            path: "src/lib.nia".into(),
        })
        .unwrap();
        bytes.push(0);
        assert_eq!(
            decode_module_id(&bytes),
            Err(MetadataError::InvalidManifest)
        );

        let mut member = definition("field", 6, None);
        member.owner = Some(Box::new(DefinitionId {
            module: ModuleId {
                package: package(),
                path: "src/other.nia".into(),
            },
            name: "Container".into(),
            kind: 5,
            disambiguator: 5,
            owner: None,
        }));
        assert_eq!(
            encode_definition_id(&member),
            Err(MetadataError::InvalidManifest)
        );
    }

    #[test]
    fn stable_type_graph_round_trips_identity_rich_nodes() {
        let owner = definition("call", 2, None);
        let graph = StableTypeGraph {
            nodes: vec![
                StableTypeNode::Primitive(3),
                StableTypeNode::Named(owner.clone()),
                StableTypeNode::TraitObject {
                    readonly: true,
                    trait_id: StableTraitId::Source(owner.clone()),
                    trait_arguments: vec![0],
                    trait_const_arguments: vec![],
                    associated_type_bindings: vec![StableAssociatedTypeBinding {
                        trait_id: None,
                        trait_arguments: vec![],
                        trait_const_arguments: vec![],
                        name: 42,
                        ty: 1,
                    }],
                },
                StableTypeNode::ClosureState {
                    owner,
                    ordinal: 1,
                    owner_instance: None,
                    captures: vec![0],
                    parameters: vec![1],
                    result: 2,
                },
            ],
            roots: vec![3],
        };

        let bytes = encode_type_graph(&graph).unwrap();
        assert_eq!(decode_type_graph(&bytes).unwrap(), graph);
    }

    #[test]
    fn stable_type_graph_rejects_forward_references_and_trailing_bytes() {
        let forward = StableTypeGraph {
            nodes: vec![
                StableTypeNode::Pointer {
                    target: 1,
                    readonly: true,
                },
                StableTypeNode::Primitive(3),
            ],
            roots: vec![0],
        };
        assert_eq!(forward.validate(), Err(MetadataError::InvalidManifest));

        let graph = StableTypeGraph {
            nodes: vec![StableTypeNode::Primitive(3)],
            roots: vec![0],
        };
        let mut bytes = encode_type_graph(&graph).unwrap();
        bytes.push(0);
        assert_eq!(
            decode_type_graph(&bytes),
            Err(MetadataError::InvalidManifest)
        );
    }

    #[test]
    fn package_source_identity_is_stable_and_excludes_generated_trees() {
        let root = std::env::temp_dir().join(format!(
            "nia-package-source-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        std::fs::create_dir_all(root.join("src/nested")).unwrap();
        std::fs::create_dir_all(root.join("target")).unwrap();
        std::fs::create_dir_all(root.join(".nia-cache")).unwrap();
        std::fs::write(root.join("src/main.nia"), "main").unwrap();
        std::fs::write(root.join("src/nested/lib.nia"), "lib").unwrap();
        std::fs::write(root.join("target/generated"), "one").unwrap();
        std::fs::write(root.join(".nia-cache/old"), "one").unwrap();
        let package = package();
        let first = package_source_identity(&root, package.clone()).unwrap();
        std::fs::write(root.join("target/generated"), "two").unwrap();
        std::fs::write(root.join(".nia-cache/old"), "two").unwrap();
        let second = package_source_identity(&root, package.clone()).unwrap();
        assert_eq!(first.source_fingerprint, second.source_fingerprint);
        assert_eq!(first.files, second.files);
        assert_eq!(first.files.len(), 2);
        std::fs::write(root.join("src/main.nia"), "changed").unwrap();
        let changed = package_source_identity(&root, package.clone()).unwrap();
        assert_ne!(first.source_fingerprint, changed.source_fingerprint);
        std::fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn package_source_identity_streams_large_files() {
        let root = std::env::temp_dir().join(format!(
            "nia-package-source-large-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        std::fs::create_dir_all(&root).unwrap();
        let contents = vec![b'x'; 128 * 1024 + 17];
        let path = root.join("large.nia");
        std::fs::write(&path, &contents).unwrap();

        let identity = package_source_identity(&root, package()).unwrap();

        assert_eq!(identity.files.len(), 1);
        assert_eq!(identity.files[0].byte_len, contents.len() as u64);
        assert_eq!(
            identity.files[0].fingerprint,
            *blake3::hash(&contents).as_bytes()
        );
        std::fs::remove_dir_all(root).unwrap();
    }

    #[cfg(unix)]
    #[test]
    fn package_source_identity_does_not_follow_symlinks() {
        use std::os::unix::fs::symlink;
        let root = std::env::temp_dir().join(format!("nia-package-symlink-{}", std::process::id()));
        let outside =
            std::env::temp_dir().join(format!("nia-package-outside-{}", std::process::id()));
        std::fs::create_dir_all(&root).unwrap();
        std::fs::write(&outside, b"outside").unwrap();
        symlink(&outside, root.join("linked.nia")).unwrap();
        let root_directory = open_cache_root(&root.canonicalize().unwrap(), false).unwrap();
        assert!(
            open_source_file(&root_directory, OsStr::new("linked.nia"))
                .unwrap()
                .is_none()
        );
        let identity = package_source_identity(&root, package()).unwrap();
        assert!(identity.files.iter().all(|file| file.path != "linked.nia"));
        std::fs::remove_file(root.join("linked.nia")).unwrap();
        std::fs::remove_file(outside).unwrap();
        std::fs::remove_dir_all(root).unwrap();
    }

    #[cfg(unix)]
    #[test]
    fn package_source_identity_ignores_linked_source_roots() {
        use std::os::unix::fs::symlink;

        let root = std::env::temp_dir().join(format!(
            "nia-package-linked-source-roots-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        std::fs::create_dir_all(root.join("actual")).unwrap();
        std::fs::create_dir_all(root.join("nested/actual")).unwrap();
        std::fs::write(root.join("actual/main.nia"), "actual").unwrap();
        std::fs::write(root.join("nested/actual/lib.nia"), "nested").unwrap();
        symlink(root.join("actual"), root.join("linked")).unwrap();
        symlink(root.join("nested/actual"), root.join("nested/linked")).unwrap();

        let identity = package_source_identity_with_selection(
            &root,
            package(),
            &PackageSourceSelection {
                source_roots: vec!["linked".into(), "nested/linked".into()],
                excludes: Vec::new(),
            },
        )
        .unwrap();

        assert!(identity.files.is_empty());
        std::fs::remove_dir_all(root).unwrap();
    }

    #[cfg(windows)]
    #[test]
    fn package_source_identity_ignores_junctioned_source_roots() {
        let root = std::env::temp_dir().join(format!(
            "nia-package-junctioned-source-roots-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        std::fs::create_dir_all(root.join("actual")).unwrap();
        std::fs::create_dir_all(root.join("nested/actual")).unwrap();
        std::fs::write(root.join("actual/main.nia"), "actual").unwrap();
        std::fs::write(root.join("nested/actual/lib.nia"), "nested").unwrap();
        for (link, target) in [
            (root.join("linked"), root.join("actual")),
            (
                root.join("nested").join("linked"),
                root.join("nested").join("actual"),
            ),
        ] {
            let output = std::process::Command::new("cmd")
                .args(["/d", "/c", "mklink", "/J"])
                .arg(&link)
                .arg(&target)
                .output()
                .unwrap();
            assert!(output.status.success(), "{output:?}");
        }

        let identity = package_source_identity_with_selection(
            &root,
            package(),
            &PackageSourceSelection {
                source_roots: vec!["linked".into(), "nested/linked".into()],
                excludes: Vec::new(),
            },
        )
        .unwrap();

        assert!(identity.files.is_empty());
        std::fs::remove_dir(root.join("linked")).unwrap();
        std::fs::remove_dir(root.join("nested").join("linked")).unwrap();
        std::fs::remove_dir_all(root).unwrap();
    }

    #[cfg(unix)]
    #[test]
    fn package_source_identity_rejects_linked_package_roots() {
        use std::os::unix::fs::symlink;

        let root = lock_test_root("linked-source-root");
        let actual = root.join("actual");
        let linked = root.join("linked");
        fs::create_dir_all(&actual).unwrap();
        fs::write(actual.join("main.nia"), "main").unwrap();
        symlink(&actual, &linked).unwrap();

        assert!(package_source_identity(&linked, package()).is_err());
        fs::remove_file(linked).unwrap();
        fs::remove_dir_all(root).unwrap();
    }

    #[cfg(windows)]
    #[test]
    fn package_source_identity_rejects_junctioned_package_roots() {
        let root = lock_test_root("junctioned-source-root");
        let actual = root.join("actual");
        let linked = root.join("linked");
        fs::create_dir_all(&actual).unwrap();
        fs::write(actual.join("main.nia"), "main").unwrap();
        let output = std::process::Command::new("cmd")
            .args(["/d", "/c", "mklink", "/J"])
            .arg(&linked)
            .arg(&actual)
            .output()
            .unwrap();
        assert!(output.status.success(), "{output:?}");

        assert!(package_source_identity(&linked, package()).is_err());
        fs::remove_dir(linked).unwrap();
        fs::remove_dir_all(root).unwrap();
    }

    #[cfg(unix)]
    #[test]
    fn package_source_identity_rejects_non_utf8_paths() {
        use std::os::unix::ffi::OsStrExt;

        let root = std::env::temp_dir().join(format!(
            "nia-package-non-utf8-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        std::fs::create_dir_all(&root).unwrap();
        let invalid_name = std::ffi::OsStr::from_bytes(b"invalid-\xff.nia");
        std::fs::write(root.join(invalid_name), b"source").unwrap();

        let error = package_source_identity(&root, package()).unwrap_err();
        assert_eq!(error.kind(), io::ErrorKind::InvalidData);
        assert_eq!(
            error.to_string(),
            "package source paths must be valid UTF-8"
        );

        std::fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn selected_source_roots_and_excludes_are_canonical_inputs() {
        let root =
            std::env::temp_dir().join(format!("nia-package-selection-{}", std::process::id()));
        std::fs::create_dir_all(root.join("src")).unwrap();
        std::fs::create_dir_all(root.join("examples")).unwrap();
        std::fs::write(root.join("src/lib.nia"), "lib").unwrap();
        std::fs::write(root.join("examples/demo.nia"), "demo").unwrap();
        let package = package();
        let selection = PackageSourceSelection {
            source_roots: vec![".".into(), "examples".into(), "src/".into(), "src".into()],
            excludes: vec!["src/generated/".into(), "src/generated".into()],
        };
        let equivalent = PackageSourceSelection {
            source_roots: vec![".".into(), "src".into(), "examples".into()],
            excludes: vec!["src/generated".into()],
        };
        let first =
            package_source_identity_with_selection(&root, package.clone(), &selection).unwrap();
        let second =
            package_source_identity_with_selection(&root, package.clone(), &equivalent).unwrap();
        assert_eq!(first.source_fingerprint, second.source_fingerprint);
        assert_eq!(first.files.len(), 2);
        std::fs::write(root.join("examples/demo.nia"), "changed").unwrap();
        let changed = package_source_identity_with_selection(&root, package, &equivalent).unwrap();
        assert_ne!(first.source_fingerprint, changed.source_fingerprint);
        std::fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn source_selection_normalizes_redundant_components() {
        let root = std::env::temp_dir().join(format!(
            "nia-package-selection-normalize-{}",
            std::process::id()
        ));
        std::fs::create_dir_all(root.join("src/generated")).unwrap();
        std::fs::write(root.join("src/lib.nia"), "lib").unwrap();
        std::fs::write(root.join("src/generated/out.nia"), "generated").unwrap();

        let canonical = PackageSourceSelection {
            source_roots: vec!["src".into()],
            excludes: vec!["src/generated".into()],
        };
        let redundant = PackageSourceSelection {
            source_roots: vec!["./src//".into()],
            excludes: vec!["src/./generated//".into()],
        };
        let first = package_source_identity_with_selection(&root, package(), &canonical).unwrap();
        let second = package_source_identity_with_selection(&root, package(), &redundant).unwrap();
        assert_eq!(first.source_fingerprint, second.source_fingerprint);
        assert_eq!(first.files, second.files);

        for invalid in ["../src", "src/../lib", "/tmp/src", "C:/src", "src\\lib"] {
            let selection = PackageSourceSelection {
                source_roots: vec![invalid.into()],
                excludes: Vec::new(),
            };
            assert!(package_source_identity_with_selection(&root, package(), &selection).is_err());
        }
        std::fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn source_identity_covers_nested_output_named_modules() {
        let root = std::env::temp_dir().join(format!(
            "nia-package-nested-output-names-{}",
            std::process::id()
        ));
        for directory in [
            "src/lib/build",
            "src/target",
            "src/dist",
            "target",
            "build",
            "src/.git",
        ] {
            std::fs::create_dir_all(root.join(directory)).unwrap();
        }
        std::fs::write(root.join("src/lib/build/util.nia"), "one").unwrap();
        std::fs::write(root.join("src/target/x.nia"), "x").unwrap();
        std::fs::write(root.join("src/dist/y.nia"), "y").unwrap();
        std::fs::write(root.join("target/out"), "out").unwrap();
        std::fs::write(root.join("build/out"), "out").unwrap();
        std::fs::write(root.join("src/.git/HEAD"), "head").unwrap();
        std::fs::write(root.join("src/nia.lock"), "lock").unwrap();

        let first = package_source_identity(&root, package()).unwrap();
        let paths = first
            .files
            .iter()
            .map(|file| file.path.as_str())
            .collect::<Vec<_>>();
        assert_eq!(
            paths,
            [
                "src/dist/y.nia",
                "src/lib/build/util.nia",
                "src/target/x.nia"
            ]
        );
        std::fs::write(root.join("src/lib/build/util.nia"), "two").unwrap();
        let changed = package_source_identity(&root, package()).unwrap();
        assert_ne!(first.source_fingerprint, changed.source_fingerprint);
        std::fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn explicitly_selected_reserved_source_root_is_included() {
        let root = std::env::temp_dir().join(format!(
            "nia-package-reserved-source-root-{}",
            std::process::id()
        ));
        std::fs::create_dir_all(root.join("build/nested/target")).unwrap();
        std::fs::create_dir_all(root.join("src/build")).unwrap();
        std::fs::write(root.join("build/main.nia"), "main").unwrap();
        std::fs::write(root.join("build/nested/target/generated.nia"), "generated").unwrap();
        std::fs::write(root.join("src/build/ignored.nia"), "ignored").unwrap();

        let package = package();
        let paths = |identity: &PackageSourceIdentity| {
            identity
                .files
                .iter()
                .map(|file| file.path.clone())
                .collect::<Vec<_>>()
        };
        // Only the package-root `build/` is skipped; nested ones are modules.
        let default_identity = package_source_identity(&root, package.clone()).unwrap();
        assert_eq!(paths(&default_identity), ["src/build/ignored.nia"]);

        let selected = PackageSourceSelection {
            source_roots: vec!["build".into(), "src/build".into()],
            excludes: Vec::new(),
        };
        let identity = package_source_identity_with_selection(&root, package, &selected).unwrap();
        assert_eq!(
            paths(&identity),
            [
                "build/main.nia",
                "build/nested/target/generated.nia",
                "src/build/ignored.nia"
            ]
        );

        std::fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn source_identity_keeps_root_and_exclude_lists_distinct() {
        let root = std::env::temp_dir().join(format!(
            "nia-package-selection-boundary-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        std::fs::create_dir_all(root.join("a")).unwrap();
        std::fs::create_dir_all(root.join("b")).unwrap();
        let first = PackageSourceSelection {
            source_roots: vec!["a".into()],
            excludes: vec!["b".into(), "c".into()],
        };
        let second = PackageSourceSelection {
            source_roots: vec!["a".into(), "b".into()],
            excludes: vec!["c".into()],
        };
        let first = package_source_identity_with_selection(&root, package(), &first).unwrap();
        let second = package_source_identity_with_selection(&root, package(), &second).unwrap();
        assert_ne!(first.source_fingerprint, second.source_fingerprint);
        std::fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn local_manifest_reads_identity_selection_and_exact_protocol_epoch() {
        let root = std::env::temp_dir().join(format!("nia-manifest-{}", std::process::id()));
        std::fs::create_dir_all(&root).unwrap();
        std::fs::write(
            root.join("nia.toml"),
            format!(
                "[package]\nnamespace = \"acme\"\nname = \"demo\"\nversion = \"1.2.3\"\nprotocol-epoch = {PROTOCOL_EPOCH}\n[source]\nroots = [\"src\"]\nexclude = [\"src/generated\"]\n[build-dependencies]\nhelper = {{ path = \"../helper\" }}\n"
            ),
        )
        .unwrap();
        let manifest = PackageManifest::read(&root).unwrap();
        assert_eq!(manifest.package.canonical_text(), "acme/demo@1.2.3");
        assert_eq!(manifest.source.source_roots, vec!["src"]);
        assert_eq!(manifest.source.excludes, vec!["src/generated"]);
        assert_eq!(manifest.dependencies[0].scope, DependencyScope::Build);
        manifest.validate_protocol_epoch().unwrap();
        for epoch in [PROTOCOL_EPOCH - 1, PROTOCOL_EPOCH + 1] {
            std::fs::write(
                root.join("nia.toml"),
                format!(
                    "[package]\nname = \"demo\"\nversion = \"1.0.0\"\nprotocol-epoch = {epoch}\n"
                ),
            )
            .unwrap();
            assert!(
                PackageManifest::read(&root)
                    .unwrap()
                    .validate_protocol_epoch()
                    .is_err()
            );
        }
        std::fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn local_manifest_requires_an_explicit_package_version() {
        let root = std::env::temp_dir().join(format!(
            "nia-manifest-version-required-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        std::fs::create_dir_all(&root).unwrap();
        std::fs::write(root.join("nia.toml"), "[package]\nname = \"demo\"\n").unwrap();
        let error = PackageManifest::read(&root).unwrap_err();
        assert_eq!(error.kind(), io::ErrorKind::InvalidData);
        assert!(error.to_string().contains("package.version"));
        std::fs::remove_dir_all(root).unwrap();
    }

    #[cfg(unix)]
    #[test]
    fn package_manifest_rejects_symlinked_manifest_files() {
        use std::os::unix::fs::symlink;

        let root = std::env::temp_dir().join(format!(
            "nia-manifest-symlink-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        std::fs::create_dir_all(&root).unwrap();
        let target = root.join("outside.toml");
        std::fs::write(&target, "[package]\nname = \"demo\"\nversion = \"1.0.0\"\n").unwrap();
        symlink(&target, root.join("nia.toml")).unwrap();

        assert!(PackageManifest::read(&root).is_err());

        std::fs::remove_file(root.join("nia.toml")).unwrap();
        std::fs::remove_dir_all(root).unwrap();
    }

    #[cfg(windows)]
    #[test]
    fn package_manifest_rejects_junctioned_manifest_files() {
        let root = std::env::temp_dir().join(format!(
            "nia-manifest-junction-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        let target = root.join("outside");
        let link = root.join("nia.toml");
        std::fs::create_dir_all(&target).unwrap();
        std::fs::write(
            target.join("manifest.toml"),
            "[package]\nname = \"demo\"\nversion = \"1.0.0\"\n",
        )
        .unwrap();
        let output = std::process::Command::new("cmd")
            .args(["/d", "/c", "mklink", "/J"])
            .arg(&link)
            .arg(&target)
            .output()
            .unwrap();
        assert!(output.status.success(), "{output:?}");

        assert!(PackageManifest::read(&root).is_err());

        std::fs::remove_dir(link).unwrap();
        std::fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn local_manifest_enforces_byte_limit_before_utf8_decoding() {
        let root = std::env::temp_dir().join(format!(
            "nia-manifest-bounds-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        std::fs::create_dir_all(&root).unwrap();
        let path = root.join("nia.toml");
        let mut contents = vec![b'#'; MAX_MANIFEST_BYTES];
        std::fs::write(&path, &contents).unwrap();
        assert!(PackageManifest::read(&root).is_err());

        contents.push(0xff);
        std::fs::write(&path, &contents).unwrap();
        let error = PackageManifest::read(&root).unwrap_err();
        assert_eq!(error.kind(), io::ErrorKind::InvalidData);
        assert_eq!(error.to_string(), "package manifest is too large");

        std::fs::write(&path, [0xff]).unwrap();
        assert_eq!(
            PackageManifest::read(&root).unwrap_err().kind(),
            io::ErrorKind::InvalidData
        );
        std::fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn dependency_parser_keeps_colons_in_relative_paths_and_rejects_absolute_paths() {
        let root = std::env::temp_dir().join(format!("nia-dependency-path-{}", std::process::id()));
        std::fs::create_dir_all(&root).unwrap();
        std::fs::write(
            root.join("nia.toml"),
            "[package]\nname = \"root\"\nversion = \"1.0.0\"\n[dependencies]\nhelper = { path = \"../third:party\" }\n",
        )
        .unwrap();
        let manifest = PackageManifest::read(&root).unwrap();
        assert_eq!(manifest.dependencies[0].name, "helper");
        assert_eq!(
            manifest.dependencies[0].source,
            DependencySource::Path("../third:party".into())
        );
        std::fs::write(
            root.join("nia.toml"),
            "[package]\nname = \"root\"\nversion = \"1.0.0\"\n[dependencies]\nhelper = { path = \"/tmp/helper\" }\n",
        )
        .unwrap();
        assert!(PackageManifest::read(&root).is_err());
        std::fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn workspace_members_inherit_package_fields_and_dependencies_explicitly() {
        let root = std::env::temp_dir().join(format!(
            "nia-workspace-inheritance-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        let member = root.join("packages/app");
        std::fs::create_dir_all(&member).unwrap();
        std::fs::write(
            root.join("nia.toml"),
            "[workspace]\nmembers = [\"packages/app\"]\n[workspace.package]\nnamespace = \"example\"\nversion = \"0.3.0-dev\"\nprotocol-epoch = 1\n[workspace.dependencies]\njson = { path = \"vendor/json\" }\n",
        )
        .unwrap();
        std::fs::write(
            member.join("nia.toml"),
            "[package]\nname = \"app\"\nnamespace.workspace = true\nversion.workspace = true\nprotocol-epoch.workspace = true\n[dependencies]\njson.workspace = true\n",
        )
        .unwrap();

        let workspace = WorkspaceManifest::read(&root).unwrap();
        assert_eq!(workspace.members, ["packages/app"]);
        assert_eq!(workspace.package.namespace.as_deref(), Some("example"));
        let manifest = PackageManifest::read(&member).unwrap();
        assert_eq!(manifest.package.namespace, "example");
        assert_eq!(manifest.package.version.as_str(), "0.3.0-dev");
        assert_eq!(manifest.protocol_epoch, Some(1));
        assert_eq!(manifest.dependencies[0].name, "json");
        assert_eq!(
            manifest.dependencies[0].source,
            DependencySource::Path("../../vendor/json".into())
        );
        assert_eq!(
            WorkspaceManifest::discover_for_package(&member)
                .unwrap()
                .unwrap()
                .root,
            root.canonicalize().unwrap()
        );
        std::fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn workspace_members_cannot_configure_registries() {
        let root = std::env::temp_dir().join(format!(
            "nia-workspace-member-registries-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        let member = root.join("app");
        std::fs::create_dir_all(&member).unwrap();
        std::fs::write(root.join("nia.toml"), "[workspace]\nmembers = [\"app\"]\n").unwrap();
        std::fs::write(
            member.join("nia.toml"),
            "[package]\nname = \"app\"\nversion = \"1.0.0\"\n[registries]\nnia = \"https://packages.example.test\"\n",
        )
        .unwrap();

        let error = PackageManifest::read(&member).unwrap_err();
        assert_eq!(error.kind(), io::ErrorKind::InvalidData);
        assert!(
            error.to_string().contains("workspace root nia.toml"),
            "{error}"
        );

        // The same table is accepted on the workspace root.
        std::fs::write(
            root.join("nia.toml"),
            "[workspace]\nmembers = [\"app\"]\n[registries]\nnia = \"https://packages.example.test\"\n",
        )
        .unwrap();
        std::fs::write(
            member.join("nia.toml"),
            "[package]\nname = \"app\"\nversion = \"1.0.0\"\n",
        )
        .unwrap();
        PackageManifest::read(&member).unwrap();
        std::fs::remove_dir_all(root).unwrap();
    }

    #[cfg(unix)]
    #[test]
    fn workspace_members_reject_linked_directories() {
        use std::os::unix::fs::symlink;

        let root = lock_test_root("linked-workspace-member");
        let member_parent = root.join("packages");
        let outside = root.join("outside");
        let linked_member = member_parent.join("app");
        fs::create_dir_all(&member_parent).unwrap();
        fs::create_dir_all(&outside).unwrap();
        fs::write(
            root.join("nia.toml"),
            "[workspace]\nmembers = [\"packages/app\"]\n",
        )
        .unwrap();
        fs::write(
            outside.join("nia.toml"),
            "[package]\nname = \"app\"\nversion = \"1.0.0\"\n",
        )
        .unwrap();
        symlink(&outside, &linked_member).unwrap();

        assert!(WorkspaceManifest::read(&root).is_err());
        assert!(!root.join("nia.lock").exists());
        fs::remove_file(linked_member).unwrap();
        fs::remove_dir_all(root).unwrap();
    }

    #[cfg(unix)]
    #[test]
    fn package_entry_points_reject_linked_start_directories() {
        use std::os::unix::fs::symlink;

        let root = lock_test_root("linked-package-entry");
        let actual = root.join("actual");
        let linked = root.join("linked");
        fs::create_dir_all(&actual).unwrap();
        fs::write(
            actual.join("nia.toml"),
            "[package]\nname = \"app\"\nversion = \"1.0.0\"\n",
        )
        .unwrap();
        symlink(&actual, &linked).unwrap();

        assert!(WorkspaceManifest::discover(&linked).is_err());
        assert!(WorkspaceManifest::discover_for_package(&linked).is_err());
        assert!(update_lock(&linked, &[]).is_err());
        assert!(!actual.join("nia.lock").exists());
        assert!(!actual.join(".nia/packages.lock").exists());

        fs::remove_file(linked).unwrap();
        fs::remove_dir_all(root).unwrap();
    }

    #[cfg(windows)]
    #[test]
    fn package_entry_points_reject_junctioned_start_directories() {
        let root = lock_test_root("junctioned-package-entry");
        let actual = root.join("actual");
        let linked = root.join("linked");
        fs::create_dir_all(&actual).unwrap();
        fs::write(
            actual.join("nia.toml"),
            "[package]\nname = \"app\"\nversion = \"1.0.0\"\n",
        )
        .unwrap();
        let output = std::process::Command::new("cmd")
            .args(["/d", "/c", "mklink", "/J"])
            .arg(&linked)
            .arg(&actual)
            .output()
            .unwrap();
        assert!(output.status.success(), "{output:?}");

        assert!(WorkspaceManifest::discover(&linked).is_err());
        assert!(WorkspaceManifest::discover_for_package(&linked).is_err());
        assert!(update_lock(&linked, &[]).is_err());
        assert!(!actual.join("nia.lock").exists());
        assert!(!actual.join(".nia/packages.lock").exists());

        fs::remove_dir(linked).unwrap();
        fs::remove_dir_all(root).unwrap();
    }

    #[cfg(windows)]
    #[test]
    fn workspace_members_reject_junctioned_directories() {
        let root = lock_test_root("junctioned-workspace-member");
        let member_parent = root.join("packages");
        let outside = root.join("outside");
        let linked_member = member_parent.join("app");
        fs::create_dir_all(&member_parent).unwrap();
        fs::create_dir_all(&outside).unwrap();
        fs::write(
            root.join("nia.toml"),
            "[workspace]\nmembers = [\"packages/app\"]\n",
        )
        .unwrap();
        fs::write(
            outside.join("nia.toml"),
            "[package]\nname = \"app\"\nversion = \"1.0.0\"\n",
        )
        .unwrap();
        let output = std::process::Command::new("cmd")
            .args(["/d", "/c", "mklink", "/J"])
            .arg(&linked_member)
            .arg(&outside)
            .output()
            .unwrap();
        assert!(output.status.success(), "{output:?}");

        assert!(WorkspaceManifest::read(&root).is_err());
        assert!(!root.join("nia.lock").exists());
        fs::remove_dir(linked_member).unwrap();
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn workspace_can_also_be_a_package_and_inherit_its_own_fields() {
        let root = std::env::temp_dir().join(format!(
            "nia-workspace-root-package-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        std::fs::create_dir_all(&root).unwrap();
        std::fs::write(
            root.join("nia.toml"),
            "[workspace]\nmembers = [\".\"]\ndefault-members = [\".\"]\n[workspace.package]\nnamespace = \"example\"\nversion = \"1.2.3\"\n[package]\nname = \"root-app\"\nnamespace.workspace = true\nversion.workspace = true\n",
        )
        .unwrap();

        let workspace = WorkspaceManifest::read(&root).unwrap();
        let package = PackageManifest::read(&root).unwrap();
        assert_eq!(
            workspace.default_members.as_deref(),
            Some([".".to_owned()].as_slice())
        );
        assert_eq!(package.package.namespace, "example");
        assert_eq!(package.package.version.as_str(), "1.2.3");
        std::fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn workspace_members_share_lock_and_package_cache_roots() {
        let root = std::env::temp_dir().join(format!(
            "nia-workspace-state-root-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        let member = root.join("crates/app");
        std::fs::create_dir_all(&member).unwrap();
        std::fs::write(
            root.join("nia.toml"),
            "[workspace]\nmembers = [\"crates/app\"]\n",
        )
        .unwrap();
        std::fs::write(
            member.join("nia.toml"),
            "[package]\nname = \"app\"\nversion = \"1.0.0\"\n",
        )
        .unwrap();

        let fetcher = PackageFetcher::for_root(&member, LockPolicy::default()).unwrap();
        assert_eq!(
            fetcher.lock_path,
            root.canonicalize().unwrap().join("nia.lock")
        );
        assert_eq!(
            fetcher.cache_root,
            root.canonicalize().unwrap().join(".nia/packages")
        );
        let cached = fetcher.cache_root.join("cached");
        fs::create_dir_all(&cached).unwrap();
        fs::write(
            cached.join("nia.toml"),
            "[package]\nname = \"cached\"\nversion = \"1.0.0\"\n",
        )
        .unwrap();
        assert!(PackageManifest::read(&cached).is_ok());
        std::fs::remove_dir_all(root).unwrap();
    }

    #[cfg(unix)]
    #[test]
    fn package_cache_rejects_linked_roots_and_entries() {
        let root = lock_test_root("package-cache-links");
        fs::create_dir_all(&root).unwrap();
        fs::write(
            root.join("nia.toml"),
            "[package]\nname = \"app\"\nversion = \"1.0.0\"\n",
        )
        .unwrap();
        let fetcher = PackageFetcher::for_root(&root, LockPolicy::default()).unwrap();
        let state = fetcher.cache_root.parent().unwrap();
        let outside = root.join("outside");
        fs::create_dir_all(&outside).unwrap();
        fs::create_dir_all(state).unwrap();
        std::os::unix::fs::symlink(&outside, &fetcher.cache_root).unwrap();
        assert!(fetcher.package_cache(true).is_err());
        assert!(fs::read_dir(&outside).unwrap().next().is_none());

        fs::remove_file(&fetcher.cache_root).unwrap();
        fs::create_dir(&fetcher.cache_root).unwrap();
        std::os::unix::fs::symlink(&outside, fetcher.cache_root.join("cached")).unwrap();
        let cache = fetcher.package_cache(false).unwrap().unwrap();
        assert!(fetcher.cached_package(&cache, "cached").is_err());
        assert!(fs::read_dir(&outside).unwrap().next().is_none());
        fs::remove_file(fetcher.cache_root.join("cached")).unwrap();
        fs::remove_dir_all(root).unwrap();
    }

    #[cfg(windows)]
    #[test]
    fn package_cache_rejects_junctioned_roots_and_entries() {
        let root = lock_test_root("package-cache-junctions");
        fs::create_dir_all(&root).unwrap();
        fs::write(
            root.join("nia.toml"),
            "[package]\nname = \"app\"\nversion = \"1.0.0\"\n",
        )
        .unwrap();
        let fetcher = PackageFetcher::for_root(&root, LockPolicy::default()).unwrap();
        let state = fetcher.cache_root.parent().unwrap();
        let outside = root.join("outside");
        fs::create_dir_all(&outside).unwrap();
        fs::create_dir_all(state).unwrap();
        let output = std::process::Command::new("cmd")
            .args(["/d", "/c", "mklink", "/J"])
            .arg(&fetcher.cache_root)
            .arg(&outside)
            .output()
            .unwrap();
        assert!(output.status.success(), "{output:?}");
        assert!(fetcher.package_cache(true).is_err());
        assert!(fs::read_dir(&outside).unwrap().next().is_none());

        fs::remove_dir(&fetcher.cache_root).unwrap();
        fs::create_dir(&fetcher.cache_root).unwrap();
        let entry = fetcher.cache_root.join("cached");
        let output = std::process::Command::new("cmd")
            .args(["/d", "/c", "mklink", "/J"])
            .arg(&entry)
            .arg(&outside)
            .output()
            .unwrap();
        assert!(output.status.success(), "{output:?}");
        let cache = fetcher.package_cache(false).unwrap().unwrap();
        assert!(fetcher.cached_package(&cache, "cached").is_err());
        assert!(fs::read_dir(&outside).unwrap().next().is_none());
        drop(cache);
        fs::remove_dir(entry).unwrap();
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn workspace_member_graph_resolves_all_declared_roots_together() {
        let root = lock_test_root("workspace-member-graph");
        for name in ["left", "right"] {
            let member = root.join(name);
            fs::create_dir_all(&member).unwrap();
            fs::write(
                member.join("nia.toml"),
                format!("[package]\nname = \"{name}\"\nversion = \"1.0.0\"\n"),
            )
            .unwrap();
        }
        fs::write(
            root.join("nia.toml"),
            "[workspace]\nmembers = [\"left\", \"right\"]\n",
        )
        .unwrap();
        let members = WorkspaceManifest::read(&root)
            .unwrap()
            .member_packages()
            .unwrap();

        let graph = PackageGraph::resolve_members_with_policy(
            &members,
            PackageGraphOptions::default(),
            LockPolicy::default(),
        )
        .unwrap();

        assert_eq!(
            graph
                .nodes
                .iter()
                .map(|node| (node.root.as_str(), node.manifest.package.name.as_str()))
                .collect::<Vec<_>>(),
            [("left", "left"), ("right", "right")]
        );
        let outsider = WorkspaceMember {
            name: "outsider".into(),
            root: root.clone(),
        };
        let error = PackageGraph::resolve_members_with_policy(
            &[outsider],
            PackageGraphOptions::default(),
            LockPolicy::default(),
        )
        .unwrap_err();
        assert!(error.to_string().contains("not a member of this workspace"));
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn workspace_globs_exclusions_and_independent_nested_workspaces() {
        let root = std::env::temp_dir().join(format!("nia-workspace-globs-{}", std::process::id()));
        for name in ["app", "core", "excluded"] {
            fs::create_dir_all(root.join("packages").join(name)).unwrap();
            fs::write(
                root.join("packages").join(name).join("nia.toml"),
                format!("[package]\nname = \"{name}\"\nversion = \"1.0.0\"\n"),
            )
            .unwrap();
        }
        fs::write(root.join("nia.toml"), "[workspace]\nmembers = [\"packages/*\"]\ndefault-members = [\"packages/a*\"]\nexclude = [\"packages/excluded\"]\n").unwrap();
        let workspace = WorkspaceManifest::read(&root).unwrap();
        assert_eq!(workspace.members, ["packages/app", "packages/core"]);
        assert_eq!(workspace.default_members.unwrap(), ["packages/app"]);
        assert!(
            WorkspaceManifest::discover(&root.join("packages/excluded"))
                .unwrap()
                .is_none()
        );
        assert!(PackageManifest::read(&root.join("packages/excluded")).is_ok());

        fs::write(
            root.join("packages/excluded/nia.toml"),
            "[workspace]\n[package]\nname = \"excluded\"\nversion = \"1.0.0\"\n",
        )
        .unwrap();
        assert_eq!(
            WorkspaceManifest::discover(&root.join("packages/excluded"))
                .unwrap()
                .unwrap()
                .root,
            root.join("packages/excluded").canonicalize().unwrap()
        );
        fs::write(
            root.join("packages/core/nia.toml"),
            "[workspace]\n[package]\nname = \"core\"\nversion = \"1.0.0\"\n",
        )
        .unwrap();
        assert!(
            WorkspaceManifest::read(&root)
                .unwrap_err()
                .to_string()
                .contains("nested workspace")
        );
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn workspace_rejects_unlisted_packages_invalid_patterns_and_ambiguous_names() {
        let root =
            std::env::temp_dir().join(format!("nia-workspace-invalid-{}", std::process::id()));
        for name in ["app", "other"] {
            fs::create_dir_all(root.join(name)).unwrap();
            fs::write(
                root.join(name).join("nia.toml"),
                "[package]\nname = \"app\"\nversion = \"1.0.0\"\n",
            )
            .unwrap();
        }
        fs::write(root.join("nia.toml"), "[workspace]\nmembers = [\"app\"]\n").unwrap();
        assert!(
            WorkspaceManifest::discover(&root.join("other"))
                .unwrap_err()
                .to_string()
                .contains("not listed")
        );
        for pattern in ["../app", "..\\app", "[", "absent/*"] {
            fs::write(
                root.join("nia.toml"),
                format!("[workspace]\nmembers = ['{pattern}']\n"),
            )
            .unwrap();
            assert!(WorkspaceManifest::read(&root).is_err(), "{pattern}");
        }
        fs::write(
            root.join("nia.toml"),
            "[workspace]\nmembers = [\"app\", \"other\"]\n",
        )
        .unwrap();
        assert!(
            WorkspaceManifest::read(&root)
                .unwrap()
                .member_packages()
                .unwrap_err()
                .to_string()
                .contains("more than one package")
        );
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn root_package_source_identity_excludes_workspace_member_trees() {
        let root = std::env::temp_dir().join(format!(
            "nia-workspace-source-identity-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        std::fs::create_dir_all(root.join("src")).unwrap();
        std::fs::create_dir_all(root.join("crates/lib/src")).unwrap();
        std::fs::write(
            root.join("nia.toml"),
            "[workspace]\nmembers = [\"crates/lib\"]\n[package]\nname = \"root\"\nversion = \"1.0.0\"\n",
        )
        .unwrap();
        std::fs::write(
            root.join("crates/lib/nia.toml"),
            "[package]\nname = \"lib\"\nversion = \"1.0.0\"\n",
        )
        .unwrap();
        std::fs::write(root.join("src/main.nia"), "root").unwrap();
        std::fs::write(root.join("crates/lib/src/lib.nia"), "one").unwrap();
        let first = package_source_identity(&root, package()).unwrap();
        std::fs::write(root.join("crates/lib/src/lib.nia"), "two").unwrap();
        let second = package_source_identity(&root, package()).unwrap();
        assert_eq!(first.source_fingerprint, second.source_fingerprint);
        assert!(
            !first
                .files
                .iter()
                .any(|file| file.path.starts_with("crates/lib/"))
        );
        std::fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn workspace_inheritance_requires_an_explicit_existing_value() {
        let root = std::env::temp_dir().join(format!(
            "nia-workspace-invalid-inheritance-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        let member = root.join("app");
        std::fs::create_dir_all(&member).unwrap();
        std::fs::write(
            root.join("nia.toml"),
            "[workspace]\nmembers = [\"app\"]\n[workspace.package]\nnamespace = \"example\"\n",
        )
        .unwrap();
        std::fs::write(
            member.join("nia.toml"),
            "[package]\nname = \"app\"\nversion.workspace = true\n",
        )
        .unwrap();
        let error = PackageManifest::read(&member).unwrap_err();
        assert!(
            error.to_string().contains("workspace.package"),
            "unexpected error: {error}"
        );

        std::fs::write(
            member.join("nia.toml"),
            "[package]\nname = \"app\"\nversion.workspace = false\n",
        )
        .unwrap();
        let error = PackageManifest::read(&member).unwrap_err();
        assert!(
            error
                .to_string()
                .contains("`package.version.workspace` must be true")
        );
        std::fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn workspace_dependency_references_reject_source_overrides() {
        let root = lock_test_root("workspace-dependency-source-overrides");
        let member = root.join("app");
        fs::create_dir_all(&member).unwrap();
        fs::write(
            root.join("nia.toml"),
            "[workspace]\nmembers = [\"app\"]\n[workspace.dependencies]\njson = { path = \"vendor/json\" }\n",
        )
        .unwrap();
        for scope in ["build-dependencies", "dependencies", "dev-dependencies"] {
            for override_field in [
                "path = \"other\"",
                "git = \"https://example.test/json.git\"",
                "registry = \"nia\"",
                "package = \"acme/json\"",
                "version = \"^1.0\"",
                "rev = \"0123456789abcdef0123456789abcdef01234567\"",
                "tag = \"v1.0.0\"",
                "branch = \"main\"",
            ] {
                fs::write(
                    member.join("nia.toml"),
                    format!(
                        "[package]\nname = \"app\"\nversion = \"1.0.0\"\n[{scope}]\njson = {{ workspace = true, {override_field} }}\n"
                    ),
                )
                .unwrap();
                let error = PackageManifest::read(&member).unwrap_err();
                assert_eq!(error.kind(), io::ErrorKind::InvalidData);
                assert!(
                    error
                        .to_string()
                        .contains("cannot combine workspace inheritance"),
                    "{scope}: {override_field}: {error}"
                );
            }
        }
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn manifest_accepts_git_sources_with_one_optional_revision_selector() {
        let root = std::env::temp_dir().join(format!("nia-git-manifest-{}", std::process::id()));
        std::fs::create_dir_all(&root).unwrap();
        std::fs::write(
            root.join("nia.toml"),
            "[package]\nname = \"root\"\nversion = \"1.0.0\"\n[build-dependencies]\njson = { git = \"https://example.test/json.git\", tag = \"v1\" }\n[dependencies]\nregex = { git = \"ssh://git@example.test/regex.git\", rev = \"0123456789abcdef0123456789abcdef01234567\" }\n",
        )
        .unwrap();
        let manifest = PackageManifest::read(&root).unwrap();
        assert_eq!(
            manifest.dependencies[0].source,
            DependencySource::Git {
                url: "https://example.test/json.git".into(),
                rev: None,
                tag: Some("v1".into()),
                branch: None,
            }
        );
        assert_eq!(manifest.dependencies[0].scope, DependencyScope::Build);
        assert_eq!(manifest.dependencies[1].scope, DependencyScope::Runtime);
        std::fs::write(
            root.join("nia.toml"),
            "[package]\nname = \"root\"\nversion = \"1.0.0\"\n[dependencies]\nregex = { git = \"git@example.test:org/regex.git\" }\n",
        )
        .unwrap();
        let manifest = PackageManifest::read(&root).unwrap();
        assert_eq!(
            manifest.dependencies[0].source,
            DependencySource::Git {
                url: "ssh://git@example.test/org/regex.git".into(),
                rev: None,
                tag: None,
                branch: None,
            }
        );
        std::fs::write(
            root.join("nia.toml"),
            "[package]\nname = \"root\"\nversion = \"1.0.0\"\n[dependencies]\ninvalid = { git = \"https://example.test/x.git\", rev = \"a\", branch = \"main\" }\n",
        )
        .unwrap();
        assert!(PackageManifest::read(&root).is_err());
        std::fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn manifest_rejects_malformed_git_revision_and_refs() {
        let root = std::env::temp_dir().join(format!(
            "nia-git-manifest-invalid-selectors-{}",
            std::process::id()
        ));
        std::fs::create_dir_all(&root).unwrap();
        for declaration in [
            "dep = { git = \"https://example.test/dep.git\", rev = \"abc\" }",
            "dep = { git = \"https://example.test/dep.git\", branch = \"../main\" }",
            "dep = { git = \"https://example.test/dep.git\", tag = \"release@{1}\" }",
            "dep = { git = \"https://example.test/dep.git\", branch = \"main\\\\next\" }",
        ] {
            std::fs::write(
                root.join("nia.toml"),
                format!("[package]\nname = \"root\"\nversion = \"1.0.0\"\n[dependencies]\n{declaration}\n"),
            )
            .unwrap();
            assert!(PackageManifest::read(&root).is_err(), "{declaration}");
        }
        std::fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn registry_manifest_requires_one_named_registry_and_semver_requirement() {
        let root = std::env::temp_dir().join(format!(
            "nia-registry-manifest-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        std::fs::create_dir_all(&root).unwrap();
        std::fs::write(
            root.join("nia.toml"),
            "[package]\nname = \"root\"\nversion = \"1.0.0\"\n[dependencies]\njson = { registry = \"nia\", package = \"acme/json\", version = \"^1.4\" }\n",
        )
        .unwrap();
        let manifest = PackageManifest::read(&root).unwrap();
        assert_eq!(
            manifest.dependencies[0].source,
            DependencySource::Registry {
                registry: "nia".into(),
                package: RegistryPackageName {
                    namespace: "acme".into(),
                    name: "json".into(),
                },
                requirement: PackageVersionRequirement::parse("^1.4").unwrap(),
            }
        );

        for declaration in [
            "json = { registry = \"nia\", package = \"acme/json\" }",
            "json = { registry = \"nia\", package = \"acme/json\", version = \"latest\" }",
            "json = { registry = \"Nia\", package = \"acme/json\", version = \"^1.4\" }",
            "json = { registry = \"nia\", package = \"acme/json\", version = \"^1.4\", path = \"../json\" }",
            "json = { registry = \"nia\", version = \"^1.4\" }",
            "json = { registry = \"nia\", package = \"Acme/json\", version = \"^1.4\" }",
            "json = { registry = \"nia\", package = \"acme\", version = \"^1.4\" }",
            "json = { version = \"^1.4\" }",
            "json = { path = \"../json\", version = \"^1.4\" }",
        ] {
            std::fs::write(
                root.join("nia.toml"),
                format!("[package]\nname = \"root\"\nversion = \"1.0.0\"\n[dependencies]\n{declaration}\n"),
            )
            .unwrap();
            assert!(PackageManifest::read(&root).is_err(), "{declaration}");
        }
        std::fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn package_graph_resolves_registry_dependency_from_offline_cache() {
        let root = std::env::temp_dir().join(format!(
            "nia-registry-graph-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        let consumer = root.join("consumer");
        fs::create_dir_all(&consumer).unwrap();
        fs::write(
            consumer.join("nia.toml"),
            "[package]\nname = \"app\"\nversion = \"1.0.0\"\n[dependencies]\njson = { registry = \"nia\", package = \"acme/json\", version = \"^1.0\" }\n[registries]\n nia = \"https://registry.example.test\"\n",
        )
        .unwrap();
        let archive = {
            let entries = [
                (
                    0u8,
                    "nia.toml",
                    b"[package]\nnamespace = \"acme\"\nname = \"json\"\nversion = \"1.0.0\"\n"
                        .as_slice(),
                ),
                (
                    0u8,
                    "src/lib.nia",
                    b"pub fun value() -> i32 = 1;\n".as_slice(),
                ),
            ];
            let mut bytes = Vec::new();
            bytes.extend_from_slice(b"NIAPKG\0\0");
            bytes.extend_from_slice(&1u32.to_le_bytes());
            bytes.extend_from_slice(&(entries.len() as u32).to_le_bytes());
            for (kind, path, contents) in entries {
                bytes.push(kind);
                bytes.extend_from_slice(&(path.len() as u32).to_le_bytes());
                bytes.extend_from_slice(path.as_bytes());
                bytes.extend_from_slice(&(contents.len() as u64).to_le_bytes());
                bytes.extend_from_slice(contents);
            }
            bytes
        };
        let digest = *blake3::hash(&archive).as_bytes();
        let digest_text = format!("blake3:{}", hex_digest(&digest));
        let index = format!(
            "schema_version = 1\n[[packages]]\nnamespace = \"acme\"\nname = \"json\"\nversion = \"1.0.0\"\narchive_url = \"https://cdn.example.test/json.nia\"\narchive_digest = \"{digest_text}\"\n"
        );
        let cache = RegistryCache::new(consumer.join(".nia/registry"));
        cache.write_index("nia", index.as_bytes()).unwrap();
        cache.write_archive(&digest, &archive).unwrap();
        let graph = PackageGraph::resolve_with_policy(
            &consumer,
            PackageGraphOptions::default(),
            LockPolicy {
                locked: false,
                offline: true,
            },
        )
        .unwrap();
        assert!(
            graph
                .nodes
                .iter()
                .any(|node| node.manifest.package.name == "json")
        );
        let lock = PackageLock::read(&consumer.join("nia.lock")).unwrap();
        assert!(lock.package.iter().any(|entry| {
            matches!(
                &entry.source,
                LockedSource::Registry {
                    registry,
                    namespace,
                    package,
                    version,
                    archive_digest,
                    ..
                } if registry == "nia"
                    && namespace == "acme"
                    && package == "json"
                    && version == "1.0.0"
                    && archive_digest == &digest_text
            )
        }));
        fs::remove_file(consumer.join(".nia/registry/indexes/nia/index.toml")).unwrap();
        let lock_bytes = fs::read(consumer.join("nia.lock")).unwrap();
        PackageGraph::resolve(&consumer, false).unwrap();
        assert_eq!(fs::read(consumer.join("nia.lock")).unwrap(), lock_bytes);
        let locked_graph = PackageGraph::resolve_with_policy_and_registries(
            &consumer,
            PackageGraphOptions::default(),
            LockPolicy {
                locked: true,
                offline: false,
            },
            BTreeMap::new(),
        )
        .unwrap();
        assert!(
            locked_graph
                .nodes
                .iter()
                .any(|node| node.manifest.package.name == "json")
        );
        let source_fingerprint = match &lock.package[0].source {
            LockedSource::Registry {
                source_fingerprint, ..
            } => source_fingerprint,
            _ => panic!("expected registry lock entry"),
        };
        let cached_package = consumer
            .join(".nia/packages")
            .join(&source_fingerprint[7..]);
        fs::write(
            cached_package.join("src/lib.nia"),
            "pub fun value() -> i32 = 2;\n",
        )
        .unwrap();
        assert!(
            PackageGraph::resolve_with_policy_and_registries(
                &consumer,
                PackageGraphOptions::default(),
                LockPolicy {
                    locked: true,
                    offline: false,
                },
                BTreeMap::new(),
            )
            .is_err()
        );
        fs::write(
            consumer.join("nia.toml"),
            "[package]\nname = \"app\"\nversion = \"0.0.0\"\n",
        )
        .unwrap();
        PackageGraph::resolve(&consumer, false).unwrap();
        assert!(
            PackageLock::read(&consumer.join("nia.lock"))
                .unwrap()
                .package
                .is_empty()
        );
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn package_graph_restarts_when_later_registry_constraints_change_selection() {
        check_registry_graph_with_late_constraints(false);
    }

    #[test]
    fn package_graph_restarts_with_cached_registry_constraints() {
        check_registry_graph_with_late_constraints(true);
    }

    fn check_registry_graph_with_late_constraints(warm_cache: bool) {
        let root = std::env::temp_dir().join(format!(
            "nia-registry-backtracking-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        let consumer = root.join("consumer");
        fs::create_dir_all(&consumer).unwrap();
        fs::write(
            consumer.join("nia.toml"),
            "[package]\nname = \"app\"\nversion = \"1.0.0\"\n[dependencies]\na = { registry = \"nia\", package = \"acme/a\", version = \"^1.0\" }\nz = { registry = \"nia\", package = \"acme/z\", version = \"^1.0\" }\n[registries]\nnia = \"https://registry.example.test\"\n",
        )
        .unwrap();
        let archive = |name: &str, version: &str, dependency: Option<&str>| {
            let manifest = match dependency {
                Some(requirement) => format!(
                    "[package]\nnamespace = \"acme\"\nname = \"{name}\"\nversion = \"{version}\"\n[dependencies]\nshared = {{ registry = \"nia\", package = \"acme/shared\", version = \"{requirement}\" }}\n"
                ),
                None => format!(
                    "[package]\nnamespace = \"acme\"\nname = \"{name}\"\nversion = \"{version}\"\n"
                ),
            };
            let entries = [
                (0u8, "nia.toml", manifest.as_bytes()),
                (
                    0u8,
                    "src/lib.nia",
                    b"pub fun value() -> i32 = 1;".as_slice(),
                ),
            ];
            let mut bytes = Vec::new();
            bytes.extend_from_slice(b"NIAPKG\0\0");
            bytes.extend_from_slice(&1u32.to_le_bytes());
            bytes.extend_from_slice(&(entries.len() as u32).to_le_bytes());
            for (kind, path, contents) in entries {
                bytes.push(kind);
                bytes.extend_from_slice(&(path.len() as u32).to_le_bytes());
                bytes.extend_from_slice(path.as_bytes());
                bytes.extend_from_slice(&(contents.len() as u64).to_le_bytes());
                bytes.extend_from_slice(contents);
            }
            bytes
        };
        let releases = [
            ("a", "1.0.0", Some(">=1.0.0, <1.1.0")),
            ("shared", "1.0.0", None),
            ("shared", "1.1.0", None),
            ("z", "1.0.0", Some("^1.0")),
        ];
        let cache = RegistryCache::new(consumer.join(".nia/registry"));
        let mut records = Vec::new();
        for (name, version, dependency) in releases {
            let bytes = archive(name, version, dependency);
            let digest = *blake3::hash(&bytes).as_bytes();
            let digest_text = format!("blake3:{}", hex_digest(&digest));
            cache.write_archive(&digest, &bytes).unwrap();
            records.push((name, version, digest_text));
        }
        records.sort_by_key(|(name, version, _)| (*name, *version));
        let mut index = String::from("schema_version = 1\n");
        for (name, version, digest) in records {
            index.push_str(&format!(
                "[[packages]]\nnamespace = \"acme\"\nname = \"{name}\"\nversion = \"{version}\"\narchive_url = \"https://cdn.example.test/{name}-{version}.nia\"\narchive_digest = \"{digest}\"\n"
            ));
        }
        cache.write_index("nia", index.as_bytes()).unwrap();

        if warm_cache {
            let manifest_path = consumer.join("nia.toml");
            let manifest = fs::read(&manifest_path).unwrap();
            fs::write(
                &manifest_path,
                "[package]\nname = \"app\"\nversion = \"1.0.0\"\n[dependencies]\nz = { registry = \"nia\", package = \"acme/z\", version = \"^1.0\" }\n[registries]\nnia = \"https://registry.example.test\"\n",
            )
            .unwrap();
            let graph = PackageGraph::resolve_with_policy(
                &consumer,
                PackageGraphOptions::default(),
                LockPolicy {
                    locked: false,
                    offline: true,
                },
            )
            .unwrap();
            assert!(graph.nodes.iter().any(|node| {
                node.manifest.package.name == "shared"
                    && node.manifest.package.version.as_str() == "1.1.0"
            }));
            fs::write(manifest_path, manifest).unwrap();
        }

        let graph = PackageGraph::resolve_with_policy(
            &consumer,
            PackageGraphOptions::default(),
            LockPolicy {
                locked: false,
                offline: true,
            },
        )
        .unwrap();
        let shared_versions = graph
            .nodes
            .iter()
            .filter(|node| node.manifest.package.name == "shared")
            .map(|node| node.manifest.package.version.as_str())
            .collect::<BTreeSet<_>>();
        assert_eq!(shared_versions, BTreeSet::from(["1.0.0"]));
        let lock = PackageLock::read(&consumer.join("nia.lock")).unwrap();
        assert_eq!(
            lock.package
                .iter()
                .filter_map(|entry| match &entry.source {
                    LockedSource::Registry {
                        package, version, ..
                    } if package == "shared" => {
                        Some(version.as_str())
                    }
                    _ => None,
                })
                .collect::<Vec<_>>(),
            ["1.0.0"]
        );
        let lock_path = consumer.join("nia.lock");
        let original_lock = fs::read(&lock_path).unwrap();
        let manifest_path = consumer.join("nia.toml");
        let manifest = fs::read_to_string(&manifest_path).unwrap().replace(
            "[registries]",
            "shared = { registry = \"nia\", package = \"acme/shared\", version = \">=1.1.0\" }\n[registries]",
        );
        fs::write(manifest_path, manifest).unwrap();
        let error = PackageGraph::resolve_with_policy(
            &consumer,
            PackageGraphOptions::default(),
            LockPolicy {
                locked: false,
                offline: true,
            },
        )
        .unwrap_err();
        let message = error.to_string();
        assert!(
            message.contains("no single version of acme/shared"),
            "{message}"
        );
        assert!(message.contains("app@1.0.0 requires >=1.1.0"), "{message}");
        assert!(
            message.contains("acme/a@1.0.0 requires >=1.0.0, <1.1.0"),
            "{message}"
        );
        assert!(message.contains("acme/z@1.0.0 requires ^1.0"), "{message}");
        assert!(message.contains(" -> a -> shared"), "{message}");
        assert!(message.contains(" -> z -> shared"), "{message}");
        assert_eq!(fs::read(&lock_path).unwrap(), original_lock);
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn registry_requirements_are_part_of_the_canonical_graph_fingerprint() {
        let graph = |package_name: &str, requirement: &str| PackageGraph {
            options: PackageGraphOptions::default(),
            nodes: vec![PackageGraphNode {
                manifest: PackageManifest {
                    package: package(),
                    protocol_epoch: None,
                    library_path: None,
                    source: PackageSourceSelection::default(),
                    dependencies: vec![PackageDependency {
                        name: "json".into(),
                        source: DependencySource::Registry {
                            registry: "nia".into(),
                            package: RegistryPackageName {
                                namespace: "acme".into(),
                                name: package_name.into(),
                            },
                            requirement: PackageVersionRequirement::parse(requirement).unwrap(),
                        },
                        scope: DependencyScope::Runtime,
                    }],
                },
                root: String::new(),
                source_fingerprint: [0; 32],
            }],
            dependencies: Vec::new(),
        };

        assert_ne!(
            graph("json", "^1.4").fingerprint(),
            graph("json", "^2.0").fingerprint()
        );
        assert_ne!(
            graph("json", "^1.4").fingerprint(),
            graph("serde", "^1.4").fingerprint()
        );
    }

    #[test]
    fn registry_solver_intersects_constraints_and_prefers_highest_stable_release() {
        let package = RegistryPackageName {
            namespace: "acme".into(),
            name: "json".into(),
        };
        let requiring_package = |name: &str| PackageId {
            namespace: "acme".into(),
            name: name.into(),
            version: PackageVersion::parse("1.0.0").unwrap(),
        };
        let constraint =
            |name: &str, requirement: &str, chain: &[&str]| RegistryVersionConstraint {
                package: package.clone(),
                requirement: PackageVersionRequirement::parse(requirement).unwrap(),
                requiring_package: requiring_package(name),
                dependency_chain: chain.iter().map(|part| (*part).into()).collect(),
            };
        let candidates =
            ["1.4.0", "1.6.2", "1.9.0", "2.0.0", "1.7.0-beta.1"].map(|version| PackageId {
                namespace: package.namespace.clone(),
                name: package.name.clone(),
                version: PackageVersion::parse(version).unwrap(),
            });
        let solved = solve_registry_versions(
            &[
                constraint("app", "^1.4", &["app", "json"]),
                constraint("tools", ">=1.6, <2.0", &["app", "tools", "json"]),
            ],
            &candidates,
        )
        .unwrap();
        assert_eq!(solved[&package].version.as_str(), "1.9.0");

        let metadata_candidates = [
            PackageId {
                namespace: package.namespace.clone(),
                name: package.name.clone(),
                version: PackageVersion::parse("1.9.0+one").unwrap(),
            },
            PackageId {
                namespace: package.namespace.clone(),
                name: package.name.clone(),
                version: PackageVersion::parse("1.9.0+two").unwrap(),
            },
        ];
        let reversed_candidates = [
            metadata_candidates[1].clone(),
            metadata_candidates[0].clone(),
        ];
        for candidates in [&metadata_candidates[..], &reversed_candidates[..]] {
            let selected =
                solve_registry_versions(&[constraint("app", "^1.4", &["app", "json"])], candidates)
                    .unwrap();
            assert_eq!(selected[&package].version.as_str(), "1.9.0+two");
        }
        let numeric_build_metadata = [
            PackageId {
                namespace: package.namespace.clone(),
                name: package.name.clone(),
                version: PackageVersion::parse("1.9.0+9").unwrap(),
            },
            PackageId {
                namespace: package.namespace.clone(),
                name: package.name.clone(),
                version: PackageVersion::parse("1.9.0+10").unwrap(),
            },
        ];
        let selected = solve_registry_versions(
            &[constraint("app", "^1.4", &["app", "json"])],
            &numeric_build_metadata,
        )
        .unwrap();
        assert_eq!(selected[&package].version.as_str(), "1.9.0+9");
    }

    #[test]
    fn registry_solver_only_admits_explicit_prereleases_and_reports_all_chains() {
        let package = RegistryPackageName {
            namespace: "acme".into(),
            name: "json".into(),
        };
        let requiring = PackageId {
            namespace: "acme".into(),
            name: "app".into(),
            version: PackageVersion::parse("1.0.0").unwrap(),
        };
        let constraint = |requirement: &str, chain: &[&str]| RegistryVersionConstraint {
            package: package.clone(),
            requirement: PackageVersionRequirement::parse(requirement).unwrap(),
            requiring_package: requiring.clone(),
            dependency_chain: chain.iter().map(|part| (*part).into()).collect(),
        };
        let candidate = |version: &str| PackageId {
            namespace: package.namespace.clone(),
            name: package.name.clone(),
            version: PackageVersion::parse(version).unwrap(),
        };

        let selected = solve_registry_versions(
            &[constraint("=1.5.0-beta.1", &["app", "json"])],
            &[candidate("1.5.0-beta.1"), candidate("1.4.0")],
        )
        .unwrap();
        assert_eq!(selected[&package].version.as_str(), "1.5.0-beta.1");

        let error = solve_registry_versions(
            &[
                constraint("^1.0", &["app", "first", "json"]),
                constraint("^2.0", &["app", "second", "json"]),
            ],
            &[candidate("1.8.0"), candidate("2.3.0")],
        )
        .unwrap_err()
        .to_string();
        assert!(error.contains("app -> first -> json"));
        assert!(error.contains("app -> second -> json"));
    }

    /// Creates a one-commit Git repository holding package `name` and returns
    /// its `file:///` URL.
    fn local_git_package(parent: &Path, name: &str) -> String {
        use std::process::Command;

        let repository = parent.join(name);
        std::fs::create_dir_all(repository.join("src")).unwrap();
        let git = |args: &[&str]| {
            let output = Command::new("git")
                .args(args)
                .current_dir(&repository)
                .output()
                .expect("Git executable is required for the test fixture");
            assert!(
                output.status.success(),
                "{}",
                String::from_utf8_lossy(&output.stderr)
            );
        };
        git(&["-c", "init.defaultBranch=main", "init", "-q"]);
        git(&["config", "user.email", "nia-test@example.invalid"]);
        git(&["config", "user.name", "Nia Test"]);
        std::fs::write(
            repository.join("nia.toml"),
            format!("[package]\nname = \"{name}\"\nversion = \"1.0.0\"\n"),
        )
        .unwrap();
        std::fs::write(repository.join("src/lib.nia"), "pub fun dependency() {}\n").unwrap();
        git(&["add", "nia.toml", "src"]);
        git(&["commit", "-qm", "fixture"]);
        format!(
            "file:///{}",
            repository.to_string_lossy().replace('\\', "/")
        )
    }

    /// Adds a commit to a repository made by [`local_git_package`].
    fn commit_upstream_change(repository: &Path, contents: &str) {
        use std::process::Command;

        std::fs::write(repository.join("src/lib.nia"), contents).unwrap();
        for args in [&["add", "src"][..], &["commit", "-qm", "change"][..]] {
            let output = Command::new("git")
                .args(args)
                .current_dir(repository)
                .output()
                .expect("Git executable is required for the test fixture");
            assert!(
                output.status.success(),
                "{}",
                String::from_utf8_lossy(&output.stderr)
            );
        }
    }

    fn locked_rev(root: &Path, name: &str) -> String {
        let lock = PackageLock::read(&root.join("nia.lock")).unwrap();
        let entry = lock
            .package
            .into_iter()
            .find(|entry| entry.name == name)
            .unwrap();
        match entry.source {
            LockedSource::Git { revision, .. } => revision,
            LockedSource::Path { .. } | LockedSource::Registry { .. } => {
                panic!("expected Git lock entry")
            }
        }
    }

    #[test]
    fn update_lock_refetches_moved_selectors() {
        let root = lock_test_root("update");
        let upstream = root.join("upstream");
        let first = local_git_package(&upstream, "first");
        let second = local_git_package(&upstream, "second");
        let consumer = root.join("consumer");
        std::fs::create_dir_all(&consumer).unwrap();
        std::fs::write(
            consumer.join("nia.toml"),
            format!(
                "[package]\nname = \"consumer\"\nversion = \"1.0.0\"\n[dependencies]\nfirst = {{ git = \"{first}\", branch = \"main\" }}\nsecond = {{ git = \"{second}\" }}\n"
            ),
        )
        .unwrap();
        PackageGraph::resolve(&consumer, false).unwrap();
        let first_rev = locked_rev(&consumer, "first");
        let second_rev = locked_rev(&consumer, "second");
        commit_upstream_change(&upstream.join("first"), "pub fun changed() {}\n");
        commit_upstream_change(&upstream.join("second"), "pub fun changed() {}\n");

        // An ordinary resolve keeps the locked commits.
        PackageGraph::resolve(&consumer, false).unwrap();
        assert_eq!(locked_rev(&consumer, "first"), first_rev);

        // Updating one name leaves the other entry alone.
        let updates = update_lock(&consumer, &["first".into()]).unwrap();
        assert_eq!(updates.len(), 1, "{updates:?}");
        assert_eq!(updates[0].name, "first");
        assert_eq!(updates[0].old_value.as_deref(), Some(first_rev.as_str()));
        assert_ne!(locked_rev(&consumer, "first"), first_rev);
        assert_eq!(locked_rev(&consumer, "second"), second_rev);

        // Updating everything picks up the rest, and a second run is a no-op.
        let updates = update_lock(&consumer, &[]).unwrap();
        assert_eq!(
            updates
                .iter()
                .map(|update| update.name.as_str())
                .collect::<Vec<_>>(),
            ["second"]
        );
        assert!(update_lock(&consumer, &[]).unwrap().is_empty());

        let error = update_lock(&consumer, &["missing".into()]).unwrap_err();
        assert!(error.to_string().contains("`missing`"), "{error}");
        std::fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn lock_updates_report_registry_version_changes() {
        let source = |version: &str| LockedPackage {
            name: "json".into(),
            source: LockedSource::Registry {
                registry: "nia".into(),
                namespace: "acme".into(),
                package: "json".into(),
                version: version.into(),
                archive_digest: format!("blake3:{}", "00".repeat(32)),
                source_fingerprint: format!("blake3:{}", "11".repeat(32)),
            },
        };
        let updates = collect_lock_updates(&[source("1.0.0")], &[source("1.1.0")]);
        assert_eq!(updates.len(), 1);
        assert_eq!(updates[0].kind, LockUpdateKind::Registry);
        assert_eq!(updates[0].selector, "registry:nia/acme/json");
        assert_eq!(updates[0].old_value.as_deref(), Some("1.0.0"));
        assert_eq!(updates[0].new_value.as_deref(), Some("1.1.0"));
    }

    #[test]
    fn registry_update_fetches_a_new_release_and_rewrites_the_lock() {
        use std::sync::{Arc, Mutex, mpsc};

        let root = lock_test_root("registry-update");
        std::fs::create_dir_all(&root).unwrap();
        std::fs::write(
            root.join("nia.toml"),
            "[package]\nname = \"app\"\nversion = \"1.0.0\"\n[dependencies]\njson = { registry = \"nia\", package = \"acme/json\", version = \"^1.0\" }\n[registries]\nnia = \"https://packages.example.test\"\n",
        )
        .unwrap();
        let make_archive = |version: &str, value: u8| {
            let manifest = format!(
                "[package]\nnamespace = \"acme\"\nname = \"json\"\nversion = \"{version}\"\n"
            );
            let entries = [
                ("nia.toml", manifest.as_bytes()),
                ("src/lib.nia", &[value][..]),
            ];
            let mut archive = Vec::new();
            archive.extend_from_slice(b"NIAPKG\0\0");
            archive.extend_from_slice(&1u32.to_le_bytes());
            archive.extend_from_slice(&(entries.len() as u32).to_le_bytes());
            for (path, contents) in entries {
                archive.push(0);
                archive.extend_from_slice(&(path.len() as u32).to_le_bytes());
                archive.extend_from_slice(path.as_bytes());
                archive.extend_from_slice(&(contents.len() as u64).to_le_bytes());
                archive.extend_from_slice(contents);
            }
            archive
        };
        let archive_1 = make_archive("1.0.0", 1);
        let archive_2 = make_archive("1.1.0", 2);
        let digest_1 = *blake3::hash(&archive_1).as_bytes();
        let digest_2 = *blake3::hash(&archive_2).as_bytes();
        let index = |version: &str, digest: &[u8; 32]| {
            format!(
                "schema_version = 1\n[[packages]]\nnamespace = \"acme\"\nname = \"json\"\nversion = \"{version}\"\narchive_url = \"https://cdn.example.test/json-{version}.nia\"\narchive_digest = \"blake3:{}\"\n",
                hex_digest(digest)
            )
            .into_bytes()
        };
        let state = Arc::new(Mutex::new(index("1.0.0", &digest_1)));
        let lock_probe = Arc::new(Mutex::new(None));
        let archives = Arc::new(BTreeMap::from([
            (
                "https://cdn.example.test/json-1.0.0.nia".to_owned(),
                archive_1,
            ),
            (
                "https://cdn.example.test/json-1.1.0.nia".to_owned(),
                archive_2,
            ),
        ]));
        let fetch: RegistryFetch = {
            let state = state.clone();
            let archives = archives.clone();
            let lock_probe = lock_probe.clone();
            let lock_path = root.join(".nia/packages.lock");
            Arc::new(move |url: &str| {
                if url == "https://packages.example.test/index.toml" {
                    let mut probe = lock_probe.lock().unwrap();
                    if probe.is_none() {
                        let (started_tx, started_rx) = mpsc::channel();
                        let (acquired_tx, acquired_rx) = mpsc::channel();
                        let lock_path = lock_path.clone();
                        let contender = std::thread::spawn(move || {
                            let lock = OpenOptions::new()
                                .create(true)
                                .truncate(false)
                                .read(true)
                                .write(true)
                                .open(lock_path)?;
                            started_tx.send(()).unwrap();
                            lock.lock()?;
                            acquired_tx.send(()).unwrap();
                            Ok::<_, io::Error>(())
                        });
                        started_rx.recv().unwrap();
                        assert!(
                            matches!(
                                acquired_rx.recv_timeout(std::time::Duration::from_millis(50)),
                                Err(mpsc::RecvTimeoutError::Timeout)
                            ),
                            "another resolver acquired the package state lock during update"
                        );
                        *probe = Some((acquired_rx, contender));
                    }
                    return Ok(state.lock().unwrap().clone());
                }
                if let Some(archive) = archives.get(url) {
                    return Ok(archive.clone());
                }
                Err(io::Error::new(
                    io::ErrorKind::NotFound,
                    "unexpected registry URL",
                ))
            })
        };

        let initial = update_lock_with_registry_fetcher(&root, &[], fetch.clone()).unwrap();
        let (acquired, contender) = lock_probe.lock().unwrap().take().unwrap();
        contender.join().unwrap().unwrap();
        acquired.recv().unwrap();
        assert_eq!(initial.len(), 1);
        assert_eq!(initial[0].new_value.as_deref(), Some("1.0.0"));
        let first_lock = PackageLock::read(&root.join("nia.lock")).unwrap();
        let first_fingerprint = match &first_lock.package[0].source {
            LockedSource::Registry {
                version,
                source_fingerprint,
                ..
            } => {
                assert_eq!(version, "1.0.0");
                source_fingerprint.clone()
            }
            _ => panic!("expected registry lock entry"),
        };

        *state.lock().unwrap() = index("1.1.0", &digest_2);
        let updates = update_lock_with_registry_fetcher(&root, &[], fetch).unwrap();
        assert_eq!(updates.len(), 1, "{updates:?}");
        assert_eq!(updates[0].kind, LockUpdateKind::Registry);
        assert_eq!(updates[0].old_value.as_deref(), Some("1.0.0"));
        assert_eq!(updates[0].new_value.as_deref(), Some("1.1.0"));

        let updated_lock = PackageLock::read(&root.join("nia.lock")).unwrap();
        let (version, fingerprint, archive_digest) = match &updated_lock.package[0].source {
            LockedSource::Registry {
                version,
                source_fingerprint,
                archive_digest,
                ..
            } => (version, source_fingerprint, archive_digest),
            _ => panic!("expected registry lock entry"),
        };
        assert_eq!(version, "1.1.0");
        assert_ne!(fingerprint, &first_fingerprint);
        assert_eq!(archive_digest, &format!("blake3:{}", hex_digest(&digest_2)));
        let package = root.join(".nia/packages").join(&fingerprint[7..]);
        assert_eq!(
            PackageManifest::read(&package)
                .unwrap()
                .package
                .version
                .as_str(),
            "1.1.0"
        );
        assert_eq!(std::fs::read(package.join("src/lib.nia")).unwrap(), [2]);
        assert!(
            std::fs::read_dir(root.join(".nia/packages"))
                .unwrap()
                .all(|entry| !entry
                    .unwrap()
                    .file_name()
                    .to_string_lossy()
                    .starts_with(".fetch-"))
        );
        std::fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn registry_update_keeps_explicit_refresh_across_aliases() {
        use std::sync::{Arc, Mutex};

        let root = lock_test_root("registry-update-aliases");
        std::fs::create_dir_all(&root).unwrap();
        std::fs::write(
            root.join("nia.toml"),
            "[package]\nname = \"app\"\nversion = \"1.0.0\"\n[dependencies]\nalpha = { registry = \"nia\", package = \"acme/json\", version = \"^1.0\" }\nbeta = { registry = \"nia\", package = \"acme/json\", version = \"^1.0\" }\n[registries]\nnia = \"https://packages.example.test\"\n",
        )
        .unwrap();
        let make_archive = |version: &str| {
            let manifest = format!(
                "[package]\nnamespace = \"acme\"\nname = \"json\"\nversion = \"{version}\"\n"
            );
            let entries = [
                ("nia.toml", manifest.as_bytes()),
                ("src/lib.nia", b"x".as_slice()),
            ];
            let mut archive = Vec::new();
            archive.extend_from_slice(b"NIAPKG\0\0");
            archive.extend_from_slice(&1u32.to_le_bytes());
            archive.extend_from_slice(&(entries.len() as u32).to_le_bytes());
            for (path, contents) in entries {
                archive.push(0);
                archive.extend_from_slice(&(path.len() as u32).to_le_bytes());
                archive.extend_from_slice(path.as_bytes());
                archive.extend_from_slice(&(contents.len() as u64).to_le_bytes());
                archive.extend_from_slice(contents);
            }
            archive
        };
        let archive_1 = make_archive("1.0.0");
        let archive_2 = make_archive("1.1.0");
        let digest_1 = *blake3::hash(&archive_1).as_bytes();
        let digest_2 = *blake3::hash(&archive_2).as_bytes();
        let index = |version: &str, digest: &[u8; 32]| {
            format!(
                "schema_version = 1\n[[packages]]\nnamespace = \"acme\"\nname = \"json\"\nversion = \"{version}\"\narchive_url = \"https://cdn.example.test/json-{version}.nia\"\narchive_digest = \"blake3:{}\"\n",
                hex_digest(digest)
            )
            .into_bytes()
        };
        let state = Arc::new(Mutex::new(index("1.0.0", &digest_1)));
        let archives = Arc::new(BTreeMap::from([
            (
                "https://cdn.example.test/json-1.0.0.nia".to_owned(),
                archive_1,
            ),
            (
                "https://cdn.example.test/json-1.1.0.nia".to_owned(),
                archive_2,
            ),
        ]));
        let fetch: RegistryFetch = {
            let state = state.clone();
            let archives = archives.clone();
            Arc::new(move |url: &str| {
                if url == "https://packages.example.test/index.toml" {
                    return Ok(state.lock().unwrap().clone());
                }
                archives.get(url).cloned().ok_or_else(|| {
                    io::Error::new(io::ErrorKind::NotFound, "unexpected registry URL")
                })
            })
        };

        let initial = update_lock_with_registry_fetcher(&root, &[], fetch.clone()).unwrap();
        assert_eq!(initial.len(), 1);
        let initial_lock = PackageLock::read(&root.join("nia.lock")).unwrap();
        assert_eq!(initial_lock.package.len(), 1);
        assert!(initial_lock.package.iter().all(|entry| matches!(
            &entry.source,
            LockedSource::Registry { version, .. } if version == "1.0.0"
        )));
        assert_eq!(initial_lock.package[0].name, "alpha");

        *state.lock().unwrap() = index("1.1.0", &digest_2);
        let updates =
            update_lock_with_registry_fetcher(&root, &["beta".into()], fetch.clone()).unwrap();
        assert_eq!(updates.len(), 1, "{updates:?}");
        assert_eq!(updates[0].name, "beta");
        let lock = PackageLock::read(&root.join("nia.lock")).unwrap();
        assert!(lock.package.iter().all(|entry| matches!(
            &entry.source,
            LockedSource::Registry { version, .. } if version == "1.1.0"
        )));
        std::fs::write(
            root.join("nia.toml"),
            "[package]\nname = \"app\"\nversion = \"1.0.0\"\n[dependencies]\nbeta = { registry = \"nia\", package = \"acme/json\", version = \"^1.0\" }\n[registries]\nnia = \"https://packages.example.test\"\n",
        )
        .unwrap();
        PackageGraph::resolve_with_policy(
            &root,
            PackageGraphOptions::default(),
            LockPolicy {
                locked: true,
                offline: false,
            },
        )
        .unwrap();
        update_lock_with_registry_fetcher(&root, &[], fetch).unwrap();
        let lock = PackageLock::read(&root.join("nia.lock")).unwrap();
        assert_eq!(lock.package[0].name, "beta");
        std::fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn failed_registry_update_restores_the_original_lockfile() {
        let root = lock_test_root("registry-update-rollback");
        std::fs::create_dir_all(&root).unwrap();
        std::fs::write(
            root.join("nia.toml"),
            "[package]\nname = \"app\"\nversion = \"1.0.0\"\n[dependencies]\njson = { registry = \"nia\", package = \"acme/json\", version = \"^1.0\" }\n[registries]\nnia = \"http://invalid.example.test\"\n",
        )
        .unwrap();
        let lock_path = root.join("nia.lock");
        PackageLock {
            version: LOCKFILE_VERSION,
            package: vec![LockedPackage {
                name: "json".into(),
                source: LockedSource::Registry {
                    registry: "nia".into(),
                    namespace: "acme".into(),
                    package: "json".into(),
                    version: "1.0.0".into(),
                    archive_digest: format!("blake3:{}", "00".repeat(32)),
                    source_fingerprint: format!("blake3:{}", "11".repeat(32)),
                },
            }],
        }
        .write_atomic(&lock_path)
        .unwrap();
        let original = format!(
            "# Preserve this lockfile exactly when update fails.\r\n{}",
            std::fs::read_to_string(&lock_path)
                .unwrap()
                .replace("version = 2", "version=2")
        );
        std::fs::write(&lock_path, original.as_bytes()).unwrap();

        let error = update_lock(&root, &[]).unwrap_err();

        assert!(error.to_string().contains("HTTPS"), "{error}");
        assert_eq!(std::fs::read(&lock_path).unwrap(), original.as_bytes());
        std::fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn failed_update_without_a_lockfile_preserves_absence() {
        let root = lock_test_root("registry-update-no-lock");
        std::fs::create_dir_all(&root).unwrap();
        std::fs::write(
            root.join("nia.toml"),
            "[package]\nname = \"app\"\nversion = \"1.0.0\"\n[dependencies]\njson = { registry = \"nia\", package = \"acme/json\", version = \"^1.0\" }\n[registries]\nnia = \"http://invalid.example.test\"\n",
        )
        .unwrap();
        let lock_path = root.join("nia.lock");
        let error = update_lock(&root, &[]).unwrap_err();
        assert!(error.to_string().contains("HTTPS"), "{error}");
        assert!(!lock_path.exists());
        std::fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn update_rolls_back_failures_after_lockfile_publication() {
        for original in [
            None,
            Some(b"# original\r\nversion=2\r\npackages=[]\r\n".as_slice()),
        ] {
            let root = lock_test_root("update-published-lock-rollback");
            fs::create_dir_all(&root).unwrap();
            let lock_path = root.join("nia.lock");
            if let Some(bytes) = original {
                fs::write(&lock_path, bytes).unwrap();
            }
            let (_, original_bytes) = PackageLock::read_snapshot(&lock_path).unwrap();
            let error = with_lockfile_rollback(&lock_path, original_bytes.as_deref(), || {
                PackageLock::default().write_atomic(&lock_path)?;
                assert!(lock_path.is_file());
                Err::<(), _>(io::Error::other("injected failure after lock publication"))
            })
            .unwrap_err();
            assert_eq!(error.to_string(), "injected failure after lock publication");
            match original {
                Some(bytes) => assert_eq!(fs::read(&lock_path).unwrap(), bytes),
                None => assert!(!lock_path.exists()),
            }
            fs::remove_dir_all(root).unwrap();
        }
    }

    #[test]
    fn workspace_update_intersects_registry_constraints_across_members() {
        use std::sync::Arc;

        let root = lock_test_root("workspace-registry-conflict");
        let left = root.join("left");
        let right = root.join("right");
        fs::create_dir_all(&left).unwrap();
        fs::create_dir_all(&right).unwrap();
        fs::write(
            root.join("nia.toml"),
            "[workspace]\nmembers = [\"left\", \"right\"]\n[registries]\nnia = \"https://packages.example.test\"\n",
        )
        .unwrap();
        for (member_root, name, requirement) in [(&left, "left", "^1.0"), (&right, "right", "^2.0")]
        {
            fs::write(
                member_root.join("nia.toml"),
                format!(
                    "[package]\nname = \"{name}\"\nversion = \"1.0.0\"\n[dependencies]\njson = {{ registry = \"nia\", package = \"acme/json\", version = \"{requirement}\" }}\n"
                ),
            )
            .unwrap();
        }

        let mut archives = BTreeMap::new();
        let mut records = String::from("schema_version = 1\n");
        for version in ["1.5.0", "2.1.0"] {
            let manifest = format!(
                "[package]\nnamespace = \"acme\"\nname = \"json\"\nversion = \"{version}\"\n"
            );
            let entries = [
                ("nia.toml", manifest.as_bytes()),
                ("src/lib.nia", b"pub fun value() -> i32 = 1;".as_slice()),
            ];
            let mut archive = Vec::new();
            archive.extend_from_slice(b"NIAPKG\0\0");
            archive.extend_from_slice(&1u32.to_le_bytes());
            archive.extend_from_slice(&(entries.len() as u32).to_le_bytes());
            for (path, contents) in entries {
                archive.push(0);
                archive.extend_from_slice(&(path.len() as u32).to_le_bytes());
                archive.extend_from_slice(path.as_bytes());
                archive.extend_from_slice(&(contents.len() as u64).to_le_bytes());
                archive.extend_from_slice(contents);
            }
            let digest = *blake3::hash(&archive).as_bytes();
            let url = format!("https://cdn.example.test/json-{version}.nia");
            archives.insert(url.clone(), archive);
            records.push_str(&format!(
                "[[packages]]\nnamespace = \"acme\"\nname = \"json\"\nversion = \"{version}\"\narchive_url = \"{url}\"\narchive_digest = \"blake3:{}\"\n",
                hex_digest(&digest)
            ));
        }
        let index_url = "https://packages.example.test/index.toml";
        let index = records.into_bytes();
        let fetch: RegistryFetch = Arc::new(move |url| {
            if url == index_url {
                return Ok(index.clone());
            }
            archives
                .get(url)
                .cloned()
                .ok_or_else(|| io::Error::new(io::ErrorKind::NotFound, "unexpected registry URL"))
        });
        let lock_path = root.join("nia.lock");
        PackageLock::default().write_atomic(&lock_path).unwrap();
        let original_lock = fs::read(&lock_path).unwrap();

        let error = update_lock_with_registry_fetcher(&root, &[], fetch.clone()).unwrap_err();

        let message = error.to_string();
        assert!(message.contains("no single version"), "{message}");
        assert!(message.contains("left@1.0.0 -> json"), "{message}");
        assert!(message.contains("right@1.0.0 -> json"), "{message}");
        assert_eq!(fs::read(&lock_path).unwrap(), original_lock);

        fs::write(
            right.join("nia.toml"),
            "[package]\nname = \"right\"\nversion = \"1.0.0\"\n[dependencies]\njson = { registry = \"nia\", package = \"acme/json\", version = \"^1.0\" }\n",
        )
        .unwrap();
        let updates = update_lock_with_registry_fetcher(&root, &[], fetch).unwrap();
        assert_eq!(updates.len(), 1, "{updates:?}");
        assert_eq!(updates[0].new_value.as_deref(), Some("1.5.0"));
        let lock = PackageLock::read(&lock_path).unwrap();
        assert_eq!(lock.package.len(), 1);
        let LockedSource::Registry { version, .. } = &lock.package[0].source else {
            panic!("expected one registry source entry")
        };
        assert_eq!(version, "1.5.0");
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn locked_policy_rejects_lockfile_changes() {
        let root = lock_test_root("locked");
        let upstream = root.join("upstream");
        let first = local_git_package(&upstream, "first");
        let second = local_git_package(&upstream, "second");
        let consumer = root.join("consumer");
        std::fs::create_dir_all(&consumer).unwrap();
        let manifest = |dependencies: &str| {
            std::fs::write(
                consumer.join("nia.toml"),
                format!("[package]\nname = \"consumer\"\nversion = \"1.0.0\"\n[dependencies]\n{dependencies}"),
            )
            .unwrap();
        };
        let locked = LockPolicy {
            locked: true,
            offline: false,
        };
        let options = PackageGraphOptions::default();

        // A dependency missing from the lock cannot be added.
        manifest(&format!("first = {{ git = \"{first}\" }}\n"));
        let error = PackageGraph::resolve_with_policy(&consumer, options, locked).unwrap_err();
        assert!(error.to_string().contains("--locked"), "{error}");
        assert!(!consumer.join("nia.lock").exists());

        // A matching lock resolves from cache but cannot fetch a missing checkout.
        PackageGraph::resolve(&consumer, false).unwrap();
        let lock = std::fs::read(consumer.join("nia.lock")).unwrap();
        PackageGraph::resolve_with_policy(&consumer, options, locked).unwrap();
        std::fs::remove_dir_all(consumer.join(".nia/packages")).unwrap();
        let error = PackageGraph::resolve_with_policy(&consumer, options, locked).unwrap_err();
        assert!(error.to_string().contains("--locked"), "{error}");
        assert_eq!(std::fs::read(consumer.join("nia.lock")).unwrap(), lock);

        // Neither adding nor removing an entry is allowed.
        manifest(&format!(
            "first = {{ git = \"{first}\" }}\nsecond = {{ git = \"{second}\" }}\n"
        ));
        assert!(PackageGraph::resolve_with_policy(&consumer, options, locked).is_err());
        manifest("");
        let error = PackageGraph::resolve_with_policy(&consumer, options, locked).unwrap_err();
        assert!(error.to_string().contains("--locked"), "{error}");
        assert_eq!(std::fs::read(consumer.join("nia.lock")).unwrap(), lock);
        std::fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn offline_policy_uses_only_the_package_cache() {
        let root = lock_test_root("offline");
        let first = local_git_package(&root.join("upstream"), "first");
        let consumer = root.join("consumer");
        std::fs::create_dir_all(&consumer).unwrap();
        std::fs::write(
            consumer.join("nia.toml"),
            format!(
                "[package]\nname = \"consumer\"\nversion = \"1.0.0\"\n[dependencies]\nfirst = {{ git = \"{first}\" }}\n"
            ),
        )
        .unwrap();
        let offline = LockPolicy {
            locked: false,
            offline: true,
        };
        let options = PackageGraphOptions::default();

        let error = PackageGraph::resolve_with_policy(&consumer, options, offline).unwrap_err();
        assert!(error.to_string().contains("--offline"), "{error}");

        PackageGraph::resolve(&consumer, false).unwrap();
        PackageGraph::resolve_with_policy(&consumer, options, offline).unwrap();
        std::fs::remove_dir_all(consumer.join(".nia/packages")).unwrap();
        let error = PackageGraph::resolve_with_policy(&consumer, options, offline).unwrap_err();
        assert!(error.to_string().contains("--offline"), "{error}");
        std::fs::remove_dir_all(root).unwrap();
    }

    fn lock_test_root(label: &str) -> PathBuf {
        let nonce = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        std::env::temp_dir().join(format!("nia-lock-{label}-{}-{nonce}", std::process::id()))
    }

    fn locked_names(root: &Path) -> Vec<String> {
        PackageLock::read(&root.join("nia.lock"))
            .unwrap()
            .package
            .into_iter()
            .map(|entry| entry.name)
            .collect()
    }

    #[test]
    fn package_graph_prunes_lock_entries_for_removed_dependencies() {
        let root = lock_test_root("prune");
        let upstream = root.join("upstream");
        let first = local_git_package(&upstream, "first");
        let second = local_git_package(&upstream, "second");
        let consumer = root.join("consumer");
        std::fs::create_dir_all(&consumer).unwrap();
        let manifest = |dependencies: &str| {
            std::fs::write(
                consumer.join("nia.toml"),
                format!("[package]\nname = \"consumer\"\nversion = \"1.0.0\"\n[dependencies]\n{dependencies}"),
            )
            .unwrap();
        };
        manifest(&format!(
            "first = {{ git = \"{first}\" }}\nsecond = {{ git = \"{second}\" }}\n"
        ));
        PackageGraph::resolve(&consumer, false).unwrap();
        assert_eq!(locked_names(&consumer), ["first", "second"]);

        manifest(&format!("first = {{ git = \"{first}\" }}\n"));
        PackageGraph::resolve(&consumer, false).unwrap();
        assert_eq!(locked_names(&consumer), ["first"]);

        manifest("");
        PackageGraph::resolve(&consumer, false).unwrap();
        assert!(locked_names(&consumer).is_empty());
        std::fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn package_graph_keeps_development_entries_outside_test_builds() {
        let root = lock_test_root("dev");
        let fixtures = local_git_package(&root.join("upstream"), "fixtures");
        let consumer = root.join("consumer");
        std::fs::create_dir_all(&consumer).unwrap();
        std::fs::write(
            consumer.join("nia.toml"),
            format!(
                "[package]\nname = \"consumer\"\nversion = \"1.0.0\"\n[dev-dependencies]\nfixtures = {{ git = \"{fixtures}\" }}\n"
            ),
        )
        .unwrap();
        PackageGraph::resolve(&consumer, true).unwrap();
        assert_eq!(locked_names(&consumer), ["fixtures"]);

        PackageGraph::resolve(&consumer, false).unwrap();
        assert_eq!(locked_names(&consumer), ["fixtures"]);
        std::fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn package_graph_keeps_other_workspace_member_entries() {
        let root = lock_test_root("workspace");
        let upstream = root.join("upstream");
        let left_dep = local_git_package(&upstream, "leftdep");
        let right_dep = local_git_package(&upstream, "rightdep");
        let workspace = root.join("workspace");
        for (member, dependency, url) in [
            ("left", "leftdep", &left_dep),
            ("right", "rightdep", &right_dep),
        ] {
            std::fs::create_dir_all(workspace.join(member)).unwrap();
            std::fs::write(
                workspace.join(member).join("nia.toml"),
                format!(
                    "[package]\nname = \"{member}\"\nversion = \"1.0.0\"\n[dependencies]\n{dependency} = {{ git = \"{url}\" }}\n"
                ),
            )
            .unwrap();
        }
        std::fs::write(
            workspace.join("nia.toml"),
            "[workspace]\nmembers = [\"left\", \"right\"]\n",
        )
        .unwrap();
        PackageGraph::resolve(&workspace.join("left"), false).unwrap();
        PackageGraph::resolve(&workspace.join("right"), false).unwrap();
        assert_eq!(locked_names(&workspace), ["leftdep", "rightdep"]);

        PackageGraph::resolve(&workspace.join("left"), false).unwrap();
        assert_eq!(locked_names(&workspace), ["leftdep", "rightdep"]);
        std::fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn package_graph_fetches_and_locks_a_local_git_dependency() {
        use std::process::Command;

        let nonce = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let root = std::env::temp_dir().join(format!(
            "nia-package-git-graph-{}-{nonce}",
            std::process::id()
        ));
        let repository = root.join("upstream");
        let consumer = root.join("consumer");
        std::fs::create_dir_all(repository.join("src")).unwrap();
        std::fs::create_dir_all(&consumer).unwrap();
        let git = |args: &[&str]| {
            let output = Command::new("git")
                .args(args)
                .current_dir(&repository)
                .output()
                .expect("Git executable is required for the test fixture");
            assert!(
                output.status.success(),
                "{}",
                String::from_utf8_lossy(&output.stderr)
            );
        };
        git(&["-c", "init.defaultBranch=main", "init", "-q"]);
        git(&["config", "user.email", "nia-test@example.invalid"]);
        git(&["config", "user.name", "Nia Test"]);
        std::fs::write(
            repository.join("nia.toml"),
            "[package]\nname = \"dep\"\nversion = \"1.0.0\"\n",
        )
        .unwrap();
        std::fs::write(repository.join("src/lib.nia"), "pub fun dependency() {}\n").unwrap();
        git(&["add", "nia.toml", "src"]);
        git(&["commit", "-qm", "fixture"]);
        let repository_url = format!(
            "file:///{}",
            repository.to_string_lossy().replace('\\', "/")
        );
        std::fs::write(
            consumer.join("nia.toml"),
            format!(
                "[package]\nname = \"consumer\"\nversion = \"1.0.0\"\n[dependencies]\ndep = {{ git = \"{repository_url}\", branch = \"main\" }}\n"
            ),
        )
        .unwrap();

        let graph = PackageGraph::resolve(&consumer, false).unwrap();
        assert_eq!(graph.nodes.len(), 2);
        assert!(
            graph
                .nodes
                .iter()
                .any(|node| node.manifest.package.name == "dep")
        );
        let dependency_cache = consumer.join(".nia/packages");
        assert!(dependency_cache.is_dir());
        assert!(
            std::fs::read_dir(dependency_cache)
                .unwrap()
                .next()
                .is_some()
        );
        let lock = PackageLock::read(&consumer.join("nia.lock")).unwrap();
        assert_eq!(lock.package.len(), 1);
        let LockedSource::Git {
            selector,
            revision,
            source_fingerprint,
            ..
        } = &lock.package[0].source
        else {
            panic!("expected a Git lock entry");
        };
        assert_eq!(selector, "branch:main");
        assert_eq!(revision.len(), 40);
        assert!(source_fingerprint.starts_with("blake3:"));

        std::fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn local_package_graph_resolves_without_loading_sources() {
        let root = std::env::temp_dir().join(format!("nia-graph-{}", std::process::id()));
        let dep = root.join("dep");
        std::fs::create_dir_all(&dep).unwrap();
        std::fs::write(
            root.join("nia.toml"),
            "[package]\nname = \"root\"\nversion = \"1.0.0\"\n[dependencies]\ndep = { path = \"dep\" }\n",
        )
        .unwrap();
        std::fs::write(
            dep.join("nia.toml"),
            "[package]\nname = \"dep\"\nversion = \"1.0.0\"\n",
        )
        .unwrap();
        let graph = PackageGraph::resolve(&root, false).unwrap();
        assert_eq!(graph.nodes.len(), 2);
        assert!(
            graph
                .nodes
                .iter()
                .any(|node| node.manifest.package.name == "dep")
        );
        let first_fingerprint = graph.fingerprint();
        std::fs::write(dep.join("src.nia"), "changed").unwrap();
        let changed = PackageGraph::resolve(&root, false).unwrap();
        assert_ne!(first_fingerprint, changed.fingerprint());
        std::fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn local_package_graph_reports_dependency_cycles() {
        let root = std::env::temp_dir().join(format!("nia-cycle-{}", std::process::id()));
        let dep = root.join("dep");
        std::fs::create_dir_all(&dep).unwrap();
        std::fs::write(
            root.join("nia.toml"),
            "[package]\nname = \"root\"\nversion = \"1.0.0\"\n[dependencies]\ndep = { path = \"dep\" }\n",
        )
        .unwrap();
        std::fs::write(
            dep.join("nia.toml"),
            "[package]\nname = \"dep\"\nversion = \"1.0.0\"\n[dependencies]\nroot = { path = \"..\" }\n",
        )
        .unwrap();
        let error = PackageGraph::resolve(&root, false).unwrap_err();
        assert!(error.to_string().contains("dependency cycle"));
        std::fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn local_package_graph_names_dependency_without_manifest() {
        let root = std::env::temp_dir().join(format!("nia-no-manifest-{}", std::process::id()));
        std::fs::create_dir_all(root.join("vendor/mathlib")).unwrap();
        std::fs::write(
            root.join("nia.toml"),
            "[package]\nname = \"root\"\nversion = \"1.0.0\"\n[dependencies]\nmathlib = { path = \"vendor/mathlib\" }\n",
        )
        .unwrap();
        let error = PackageGraph::resolve(&root, false).unwrap_err();
        assert_eq!(error.kind(), io::ErrorKind::NotFound);
        let message = error.to_string();
        assert!(message.contains("dependency `mathlib`"), "{message}");
        assert!(message.contains("does not contain nia.toml"), "{message}");
        std::fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn local_package_graph_names_missing_dependency_path() {
        let root = std::env::temp_dir().join(format!("nia-missing-dep-{}", std::process::id()));
        std::fs::create_dir_all(&root).unwrap();
        std::fs::write(
            root.join("nia.toml"),
            "[package]\nname = \"root\"\nversion = \"1.0.0\"\n[dependencies]\nmathlib = { path = \"vendor/mathlib\" }\n",
        )
        .unwrap();
        let error = PackageGraph::resolve(&root, false).unwrap_err();
        assert_eq!(error.kind(), io::ErrorKind::NotFound);
        let message = error.to_string();
        assert!(message.contains("dependency `mathlib`"), "{message}");
        assert!(message.contains("mathlib`:"), "{message}");
        std::fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn path_dependencies_are_locked_by_owner_path_and_source_fingerprint() {
        let root = lock_test_root("path-lock");
        let consumer = root.join("consumer");
        let dependency = root.join("vendor/json");
        fs::create_dir_all(consumer.join("src")).unwrap();
        fs::create_dir_all(dependency.join("src")).unwrap();
        fs::write(
            consumer.join("nia.toml"),
            "[package]\nname = \"app\"\nversion = \"1.0.0\"\n[dependencies]\njson = { path = \"../vendor/json\" }\n",
        )
        .unwrap();
        fs::write(
            dependency.join("nia.toml"),
            "[package]\nname = \"json\"\nversion = \"1.0.0\"\n",
        )
        .unwrap();
        fs::write(
            dependency.join("src/lib.nia"),
            "pub fun value() -> i32 = 1;\n",
        )
        .unwrap();

        PackageGraph::resolve(&consumer, false).unwrap();
        let lock_path = consumer.join("nia.lock");
        let first_lock = fs::read(&lock_path).unwrap();
        let lock = PackageLock::read(&lock_path).unwrap();
        assert!(matches!(
            &lock.package[0].source,
            LockedSource::Path {
                owner,
                path,
                namespace,
                package,
                version,
                source_fingerprint,
            } if owner == "local/app@1.0.0"
                && path == "../vendor/json"
                && namespace == "local"
                && package == "json"
                && version == "1.0.0"
                && source_fingerprint.starts_with("blake3:")
        ));

        let locked = LockPolicy {
            locked: true,
            offline: true,
        };
        PackageGraph::resolve_with_policy(&consumer, PackageGraphOptions::default(), locked)
            .unwrap();
        fs::write(
            dependency.join("src/lib.nia"),
            "pub fun value() -> i32 = 2;\n",
        )
        .unwrap();
        assert!(
            PackageGraph::resolve_with_policy(&consumer, PackageGraphOptions::default(), locked)
                .is_err()
        );
        assert_eq!(fs::read(&lock_path).unwrap(), first_lock);
        PackageGraph::resolve(&consumer, false).unwrap();
        assert_ne!(fs::read(&lock_path).unwrap(), first_lock);
        fs::remove_dir_all(root).unwrap();
    }

    #[cfg(unix)]
    #[test]
    fn path_dependencies_reject_linked_package_roots() {
        use std::os::unix::fs::symlink;

        let root = lock_test_root("linked-path-dependency");
        let consumer = root.join("consumer");
        let outside = root.join("outside");
        let link = consumer.join("vendor").join("json");
        fs::create_dir_all(link.parent().unwrap()).unwrap();
        fs::create_dir_all(&outside).unwrap();
        fs::write(
            consumer.join("nia.toml"),
            "[package]\nname = \"app\"\nversion = \"1.0.0\"\n[dependencies]\njson = { path = \"vendor/json\" }\n",
        )
        .unwrap();
        fs::write(
            outside.join("nia.toml"),
            "[package]\nname = \"json\"\nversion = \"1.0.0\"\n",
        )
        .unwrap();
        symlink(&outside, &link).unwrap();

        assert!(PackageGraph::resolve(&consumer, false).is_err());
        assert!(!consumer.join("nia.lock").exists());
        fs::remove_file(link).unwrap();
        fs::remove_dir_all(root).unwrap();
    }

    #[cfg(unix)]
    #[test]
    fn package_graph_rejects_linked_root_directories() {
        use std::os::unix::fs::symlink;

        let root = lock_test_root("linked-graph-root");
        let actual = root.join("actual");
        let linked = root.join("linked");
        fs::create_dir_all(&actual).unwrap();
        fs::write(
            actual.join("nia.toml"),
            "[package]\nname = \"app\"\nversion = \"1.0.0\"\n",
        )
        .unwrap();
        symlink(&actual, &linked).unwrap();

        assert!(PackageGraph::resolve(&linked, false).is_err());
        assert!(!actual.join("nia.lock").exists());
        fs::remove_file(linked).unwrap();
        fs::remove_dir_all(root).unwrap();
    }

    #[cfg(windows)]
    #[test]
    fn path_dependencies_reject_junctioned_package_roots() {
        let root = lock_test_root("junctioned-path-dependency");
        let consumer = root.join("consumer");
        let outside = root.join("outside");
        let link = consumer.join("vendor").join("json");
        fs::create_dir_all(link.parent().unwrap()).unwrap();
        fs::create_dir_all(&outside).unwrap();
        fs::write(
            consumer.join("nia.toml"),
            "[package]\nname = \"app\"\nversion = \"1.0.0\"\n[dependencies]\njson = { path = \"vendor/json\" }\n",
        )
        .unwrap();
        fs::write(
            outside.join("nia.toml"),
            "[package]\nname = \"json\"\nversion = \"1.0.0\"\n",
        )
        .unwrap();
        let output = std::process::Command::new("cmd")
            .args(["/d", "/c", "mklink", "/J"])
            .arg(&link)
            .arg(&outside)
            .output()
            .unwrap();
        assert!(output.status.success(), "{output:?}");

        assert!(PackageGraph::resolve(&consumer, false).is_err());
        assert!(!consumer.join("nia.lock").exists());
        fs::remove_dir(link).unwrap();
        fs::remove_dir_all(root).unwrap();
    }

    #[cfg(windows)]
    #[test]
    fn package_graph_rejects_junctioned_root_directories() {
        let root = lock_test_root("junctioned-graph-root");
        let actual = root.join("actual");
        let linked = root.join("linked");
        fs::create_dir_all(&actual).unwrap();
        fs::write(
            actual.join("nia.toml"),
            "[package]\nname = \"app\"\nversion = \"1.0.0\"\n",
        )
        .unwrap();
        let output = std::process::Command::new("cmd")
            .args(["/d", "/c", "mklink", "/J"])
            .arg(&linked)
            .arg(&actual)
            .output()
            .unwrap();
        assert!(output.status.success(), "{output:?}");

        assert!(PackageGraph::resolve(&linked, false).is_err());
        assert!(!actual.join("nia.lock").exists());
        fs::remove_dir(linked).unwrap();
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn local_package_graph_names_nested_dependency_chain() {
        let root = std::env::temp_dir().join(format!("nia-nested-dep-{}", std::process::id()));
        let json = root.join("vendor/json");
        let regex = json.join("vendor/regex");
        std::fs::create_dir_all(&regex).unwrap();
        std::fs::write(
            root.join("nia.toml"),
            "[package]\nname = \"root\"\nversion = \"1.0.0\"\n[dependencies]\njson = { path = \"vendor/json\" }\n",
        )
        .unwrap();
        std::fs::write(
            json.join("nia.toml"),
            "[package]\nname = \"json\"\nversion = \"1.0.0\"\n[dependencies]\nregex = { path = \"vendor/regex\" }\n",
        )
        .unwrap();
        std::fs::write(
            regex.join("nia.toml"),
            "[package]\nname = \"regex\"\nversion = \"1.0.0\"\nunknown = 1\n",
        )
        .unwrap();
        let error = PackageGraph::resolve(&root, false).unwrap_err();
        assert_eq!(error.kind(), io::ErrorKind::InvalidData);
        assert!(
            error.to_string().contains("dependency `json/regex`"),
            "{error}"
        );
        std::fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn dependency_scopes_require_explicit_development_selection() {
        let root = std::env::temp_dir().join(format!("nia-scopes-{}", std::process::id()));
        let build = root.join("build");
        let runtime = root.join("runtime");
        let development = root.join("development");
        std::fs::create_dir_all(&build).unwrap();
        std::fs::create_dir_all(&runtime).unwrap();
        std::fs::create_dir_all(&development).unwrap();
        std::fs::write(root.join("nia.toml"), "[package]\nname = \"root\"\nversion = \"1.0.0\"\n[build-dependencies]\nbuild = { path = \"build\" }\n[dependencies]\nruntime = { path = \"runtime\" }\n[dev-dependencies]\ndevelopment = { path = \"development\" }\n").unwrap();
        for (dir, name) in [
            (&build, "build"),
            (&runtime, "runtime"),
            (&development, "development"),
        ] {
            std::fs::write(
                dir.join("nia.toml"),
                format!("[package]\nname = \"{name}\"\nversion = \"1.0.0\"\n"),
            )
            .unwrap();
        }
        let ordinary = PackageGraph::resolve(&root, false).unwrap();
        assert_eq!(ordinary.nodes.len(), 3);
        let tests = PackageGraph::resolve(&root, true).unwrap();
        assert_eq!(tests.nodes.len(), 4);
        assert!(tests.nodes.iter().any(|node| node.root == "development"));
        std::fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn graph_report_preserves_filtered_dependency_decisions() {
        let root = std::env::temp_dir().join(format!("nia-report-{}", std::process::id()));
        let development = root.join("development");
        std::fs::create_dir_all(&development).unwrap();
        std::fs::write(
            root.join("nia.toml"),
            "[package]\nname = \"root\"\nversion = \"1.0.0\"\n[dev-dependencies]\ndevelopment = { path = \"development\" }\n",
        )
        .unwrap();
        std::fs::write(
            development.join("nia.toml"),
            "[package]\nname = \"development\"\nversion = \"1.0.0\"\n",
        )
        .unwrap();
        let graph = PackageGraph::resolve(&root, false).unwrap();
        let report = graph.report();
        assert_eq!(report.resolved.len(), 1);
        assert_eq!(report.declared.len(), 1);
        assert_eq!(
            report.declared[0].dependency.scope,
            DependencyScope::Development
        );
        assert_eq!(report.declared[0].resolved_root, None);
        assert_eq!(
            report.declared[0].source_root,
            Some(development.canonicalize().unwrap())
        );
        let selected = PackageGraph::resolve_with_options(
            &root,
            PackageGraphOptions {
                include_development: true,
            },
        )
        .unwrap();
        assert_eq!(
            selected.report().declared[0].resolved_root.as_deref(),
            Some("development")
        );
        std::fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn shared_checkout_keeps_each_dependency_class_decision() {
        let root = std::env::temp_dir().join(format!(
            "nia-shared-checkout-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        let shared = root.join("shared");
        std::fs::create_dir_all(&shared).unwrap();
        std::fs::write(
            root.join("nia.toml"),
            "[package]\nname = \"root\"\nversion = \"1.0.0\"\n[dependencies]\nruntime = { path = \"shared\" }\n[dev-dependencies]\ndevelopment = { path = \"shared\" }\n",
        )
        .unwrap();
        std::fs::write(
            shared.join("nia.toml"),
            "[package]\nname = \"shared\"\nversion = \"1.0.0\"\n",
        )
        .unwrap();

        let graph =
            PackageGraph::resolve_with_options(&root, PackageGraphOptions::default()).unwrap();
        assert_eq!(
            graph.nodes.len(),
            2,
            "one physical checkout is one graph node"
        );
        let report = graph.report();
        let runtime = report
            .declared
            .iter()
            .find(|edge| edge.dependency.name == "runtime")
            .unwrap();
        let optional = report
            .declared
            .iter()
            .find(|edge| edge.dependency.name == "development")
            .unwrap();
        let shared_root = graph
            .nodes
            .iter()
            .find(|node| !node.root.is_empty())
            .unwrap()
            .root
            .clone();
        assert_eq!(runtime.resolved_root, Some(shared_root));
        assert_eq!(optional.resolved_root, None);
        assert_eq!(runtime.source_root, optional.source_root);

        let selected = PackageGraph::resolve_with_options(
            &root,
            PackageGraphOptions {
                include_development: true,
            },
        )
        .unwrap();
        assert!(selected.nodes.iter().any(|node| node.root == "runtime"));
        assert!(selected.nodes.iter().any(|node| node.root == "development"));
        let selected_edges = selected.report().declared;
        assert_eq!(
            selected_edges
                .iter()
                .find(|edge| edge.dependency.name == "runtime")
                .and_then(|edge| edge.resolved_root.as_deref()),
            Some("runtime")
        );
        assert_eq!(
            selected_edges
                .iter()
                .find(|edge| edge.dependency.name == "development")
                .and_then(|edge| edge.resolved_root.as_deref()),
            Some("development")
        );

        std::fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn package_graph_fingerprint_includes_selection_policy() {
        let root = std::env::temp_dir().join(format!(
            "nia-graph-policy-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        std::fs::create_dir_all(&root).unwrap();
        std::fs::write(
            root.join("nia.toml"),
            "[package]\nname = \"root\"\nversion = \"1.0.0\"\n[dev-dependencies]\ndevelopment = { path = \"development\" }\n",
        )
        .unwrap();
        std::fs::create_dir_all(root.join("development")).unwrap();
        std::fs::write(
            root.join("development/nia.toml"),
            "[package]\nname = \"development\"\nversion = \"1.0.0\"\n",
        )
        .unwrap();

        let ordinary =
            PackageGraph::resolve_with_options(&root, PackageGraphOptions::default()).unwrap();
        let selected = PackageGraph::resolve_with_options(
            &root,
            PackageGraphOptions {
                include_development: true,
            },
        )
        .unwrap();
        assert_ne!(ordinary.fingerprint(), selected.fingerprint());

        std::fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn package_graph_fingerprint_includes_library_manifest_semantics() {
        let root = std::env::temp_dir().join(format!(
            "nia-graph-library-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        std::fs::create_dir_all(root.join("src")).unwrap();
        std::fs::write(
            root.join("nia.toml"),
            "[package]\nname = \"root\"\nversion = \"1.0.0\"\n[library]\npath = \"src/first.nia\"\n[source]\nroots = [\"src\"]\n",
        )
        .unwrap();
        std::fs::write(root.join("src/first.nia"), "pub const value: u8 = 1;").unwrap();
        std::fs::write(root.join("src/second.nia"), "pub const value: u8 = 1;").unwrap();
        let first = PackageGraph::resolve(&root, false).unwrap().fingerprint();

        std::fs::write(
            root.join("nia.toml"),
            "[package]\nname = \"root\"\nversion = \"1.0.0\"\n[library]\npath = \"src/second.nia\"\n[source]\nroots = [\"src\"]\n",
        )
        .unwrap();
        let second = PackageGraph::resolve(&root, false).unwrap().fingerprint();

        assert_ne!(first, second);
        std::fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn package_manifest_rejects_library_paths_outside_the_package() {
        let root = std::env::temp_dir().join(format!(
            "nia-manifest-library-path-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        std::fs::create_dir_all(&root).unwrap();
        std::fs::write(
            root.join("nia.toml"),
            "[package]\nname = \"root\"\nversion = \"1.0.0\"\n[library]\npath = \"../outside.nia\"\n",
        )
        .unwrap();

        let error = PackageManifest::read(&root).unwrap_err();
        assert_eq!(error.kind(), io::ErrorKind::InvalidData);
        assert!(error.to_string().contains("invalid package library path"));

        std::fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn package_manifest_requires_library_entry_in_selected_sources() {
        let root = std::env::temp_dir().join(format!(
            "nia-manifest-library-selection-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        std::fs::create_dir_all(&root).unwrap();
        for manifest in [
            "[package]\nname = \"root\"\nversion = \"1.0.0\"\n[library]\npath = \"lib.nia\"\n[source]\nroots = [\"src\"]\n",
            "[package]\nname = \"root\"\nversion = \"1.0.0\"\n[library]\npath = \"src/lib.nia\"\n[source]\nroots = [\"src\"]\nexclude = [\"src/lib.nia\"]\n",
        ] {
            std::fs::write(root.join("nia.toml"), manifest).unwrap();
            let error = PackageManifest::read(&root).unwrap_err();
            assert_eq!(error.kind(), io::ErrorKind::InvalidData);
            assert!(error.to_string().contains("included by source roots"));
        }
        std::fs::remove_dir_all(root).unwrap();
    }

    #[cfg(unix)]
    #[test]
    fn package_graph_rejects_library_symlinks_outside_the_package() {
        use std::os::unix::fs::symlink;

        let root = std::env::temp_dir().join(format!(
            "nia-graph-library-symlink-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        std::fs::create_dir_all(root.join("src")).unwrap();
        let outside = root.with_extension("outside.nia");
        std::fs::write(&outside, "pub const value: u8 = 1;").unwrap();
        symlink(&outside, root.join("src/lib.nia")).unwrap();
        std::fs::write(
            root.join("nia.toml"),
            "[package]\nname = \"root\"\nversion = \"1.0.0\"\n[library]\npath = \"src/lib.nia\"\n",
        )
        .unwrap();

        let error = PackageGraph::resolve(&root, false).unwrap_err();
        assert!(error.to_string().contains("must be a regular file"));

        std::fs::remove_dir_all(root).unwrap();
        std::fs::remove_file(outside).unwrap();
    }

    #[cfg(unix)]
    #[test]
    fn package_graph_rejects_library_paths_through_linked_directories() {
        use std::os::unix::fs::symlink;

        let root = lock_test_root("linked-library-directory");
        std::fs::create_dir_all(root.join("src")).unwrap();
        std::fs::create_dir_all(root.join("real")).unwrap();
        std::fs::write(root.join("real/lib.nia"), "pub const value: u8 = 1;").unwrap();
        symlink(root.join("real"), root.join("src/linked")).unwrap();
        std::fs::write(
            root.join("nia.toml"),
            "[package]\nname = \"root\"\nversion = \"1.0.0\"\n[library]\npath = \"src/linked/lib.nia\"\n",
        )
        .unwrap();

        let error = PackageGraph::resolve(&root, false).unwrap_err();
        assert!(error.to_string().contains("must use real directories"));

        std::fs::remove_dir_all(root).unwrap();
    }

    #[cfg(windows)]
    #[test]
    fn package_graph_rejects_library_paths_through_junctioned_directories() {
        let root = lock_test_root("junctioned-library-directory");
        std::fs::create_dir_all(root.join("src")).unwrap();
        std::fs::create_dir_all(root.join("real")).unwrap();
        std::fs::write(root.join("real/lib.nia"), "pub const value: u8 = 1;").unwrap();
        let link = root.join("src").join("linked");
        let output = std::process::Command::new("powershell")
            .args([
                "-NoProfile",
                "-Command",
                "New-Item",
                "-ItemType",
                "Junction",
                "-Path",
            ])
            .arg(&link)
            .args(["-Target"])
            .arg(root.join("real"))
            .output()
            .unwrap();
        assert!(output.status.success(), "{output:?}");
        std::fs::write(
            root.join("nia.toml"),
            "[package]\nname = \"root\"\nversion = \"1.0.0\"\n[library]\npath = \"src/linked/lib.nia\"\n",
        )
        .unwrap();

        let error = PackageGraph::resolve(&root, false).unwrap_err();
        assert!(error.to_string().contains("must use real directories"));

        std::fs::remove_dir(link).unwrap();
        std::fs::remove_dir_all(root).unwrap();
    }
}
