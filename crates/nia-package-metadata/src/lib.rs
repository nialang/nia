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

use std::{
    collections::{BTreeMap, BTreeSet},
    fs::{self, File, OpenOptions},
    io::{self, Cursor, Read, Write},
    path::{Path, PathBuf},
};

use nia_compat::{COMPILER_VERSION, PROTOCOL_EPOCH as NIA_PROTOCOL_EPOCH, toolchain};
use nia_git::checkout_source;
use nia_query::FingerprintDomain;
use serde::{Deserialize, Serialize};

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
    pub version: String,
}

impl PackageId {
    /// Returns the canonical text used at compiler identity boundaries.
    pub fn canonical_text(&self) -> String {
        format!(
            "{}/{}@{}",
            canonical_component(&self.namespace),
            canonical_component(&self.name),
            canonical_component(&self.version)
        )
    }

    /// Returns the canonical identity of the toolchain standard library.
    pub fn standard_library() -> Self {
        Self {
            namespace: "nia".into(),
            name: "std".into(),
            version: format!("{}+std{}", COMPILER_VERSION, toolchain::STANDARD_LIBRARY),
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
pub const LOCKFILE_VERSION: u32 = 1;
const MAX_LOCKFILE_BYTES: usize = 4 * 1024 * 1024;

/// Reproducible package source selections recorded in `nia.lock`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PackageLock {
    pub version: u32,
    #[serde(default)]
    pub package: Vec<LockedPackage>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct LockedPackage {
    pub name: String,
    pub source: String,
    pub selector: String,
    pub rev: String,
    pub content_hash: String,
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
        for package in &self.package {
            validate_dependency_name(&package.name).map_err(|error| {
                io::Error::new(
                    error_kind,
                    format!("invalid lockfile package name: {error}"),
                )
            })?;
            if package.source.is_empty() || package.source.contains(['\r', '\n']) {
                return Err(io::Error::new(error_kind, "lockfile source is invalid"));
            }
            if package.selector.is_empty() || package.selector.contains(['\r', '\n']) {
                return Err(io::Error::new(error_kind, "lockfile selector is invalid"));
            }
            if package.rev.len() != 40 || !package.rev.bytes().all(|byte| byte.is_ascii_hexdigit())
            {
                return Err(io::Error::new(
                    error_kind,
                    "lockfile revision must be a full hexadecimal commit",
                ));
            }
            if !package.content_hash.starts_with("blake3:") || package.content_hash.len() != 71 {
                return Err(io::Error::new(
                    error_kind,
                    "lockfile content hash must be a blake3 digest",
                ));
            }
            if !package.content_hash[7..]
                .bytes()
                .all(|byte| byte.is_ascii_hexdigit())
            {
                return Err(io::Error::new(
                    error_kind,
                    "lockfile content hash must be hexadecimal",
                ));
            }
            if !identities.insert((
                package.name.as_str(),
                package.source.as_str(),
                package.selector.as_str(),
            )) {
                return Err(io::Error::new(
                    error_kind,
                    "lockfile contains duplicate package selectors",
                ));
            }
        }
        Ok(())
    }

    /// Read a bounded, strict TOML lockfile. A missing lockfile is empty.
    pub fn read(path: &Path) -> io::Result<Self> {
        let mut bytes = Vec::new();
        match File::open(path) {
            Ok(file) => file
                .take((MAX_LOCKFILE_BYTES + 1) as u64)
                .read_to_end(&mut bytes)?,
            Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(Self::default()),
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
        Ok(lock)
    }

    /// Atomically replace a lockfile, creating its parent directory as needed.
    pub fn write_atomic(&self, path: &Path) -> io::Result<()> {
        self.validate(io::ErrorKind::InvalidInput)?;
        let text = toml::to_string_pretty(self)
            .map_err(|error| io::Error::new(io::ErrorKind::InvalidData, error))?;
        let parent = path.parent().unwrap_or_else(|| Path::new("."));
        fs::create_dir_all(parent)?;
        let mut temporary = None;
        for suffix in 0..32 {
            let temp = parent.join(format!(".nia.lock.{}.{}.tmp", std::process::id(), suffix));
            match OpenOptions::new().create_new(true).write(true).open(&temp) {
                Ok(file) => {
                    temporary = Some((temp, file));
                    break;
                }
                Err(error) if error.kind() == io::ErrorKind::AlreadyExists => {}
                Err(error) => return Err(error),
            }
        }
        let (temp, mut file) = temporary.ok_or_else(|| {
            io::Error::new(
                io::ErrorKind::AlreadyExists,
                "could not allocate lockfile temporary path",
            )
        })?;
        file.write_all(text.as_bytes())?;
        file.sync_all()?;
        drop(file);
        if let Err(error) = fs::rename(&temp, path) {
            if error.kind() == io::ErrorKind::AlreadyExists {
                fs::remove_file(path)?;
                fs::rename(&temp, path)?;
            } else {
                let _ = fs::remove_file(&temp);
                return Err(error);
            }
        }
        Ok(())
    }
}

/// Resolves supported Git dependencies into an immutable, project-local cache.
///
/// Resolves supported Git dependencies into an immutable, project-local cache.
#[derive(Debug)]
struct PackageFetcher {
    cache_root: PathBuf,
    lock_path: PathBuf,
    lock: PackageLock,
    dirty: bool,
    state_lock: Option<File>,
}

impl PackageFetcher {
    fn for_root(root: &Path) -> io::Result<Self> {
        let state_root = workspace_state_root(root)?;
        let lock_path = state_root.join("nia.lock");
        Ok(Self {
            cache_root: state_root.join(".nia/packages"),
            lock: PackageLock::read(&lock_path)?,
            lock_path,
            dirty: false,
            state_lock: None,
        })
    }

    fn resolve(&mut self, owner: &Path, dependency: &PackageDependency) -> io::Result<PathBuf> {
        match &dependency.source {
            DependencySource::Path(path) => Ok(owner.join(path)),
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
                let selector = if rev.is_some() {
                    format!("rev:{}", rev.as_deref().unwrap_or_default())
                } else if let Some(tag) = tag {
                    format!("tag:{tag}")
                } else if let Some(branch) = branch {
                    format!("branch:{branch}")
                } else {
                    "head".to_string()
                };
                if let Some(entry) = self.lock.package.iter().find(|entry| {
                    entry.name == dependency.name
                        && entry.source == source
                        && entry.selector == selector
                }) {
                    let destination = self.cache_root.join(&entry.content_hash[7..]);
                    if destination.is_dir() {
                        let manifest = PackageManifest::read(&destination)?;
                        let identity = package_source_identity_with_selection(
                            &destination,
                            manifest.package.clone(),
                            &manifest.source,
                        )?;
                        if format!("blake3:{}", hex_digest(&identity.source_fingerprint))
                            == entry.content_hash
                        {
                            return Ok(destination);
                        }
                        return Err(io::Error::new(
                            io::ErrorKind::InvalidData,
                            format!(
                                "cached Git dependency `{}` failed its content hash check",
                                dependency.name
                            ),
                        ));
                    }
                }
                let locked_rev = self
                    .lock
                    .package
                    .iter()
                    .find(|entry| {
                        entry.name == dependency.name
                            && entry.source == source
                            && entry.selector == selector
                    })
                    .map(|entry| entry.rev.clone());
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
                if !destination.is_dir() {
                    fs::create_dir_all(&self.cache_root)?;
                    let staging = allocate_package_staging(&self.cache_root)?;
                    let fetched_revision = match checkout_source(
                        &source,
                        fetch_rev
                            .as_deref()
                            .or(requested.as_deref())
                            .unwrap_or("HEAD"),
                        &staging,
                    ) {
                        Ok(revision) => revision,
                        Err(error) => {
                            let _ = fs::remove_dir_all(&staging);
                            return Err(error);
                        }
                    };
                    if let Err(error) = fs::rename(&staging, &destination) {
                        let _ = fs::remove_dir_all(&staging);
                        if !destination.is_dir() {
                            return Err(error);
                        }
                    }
                    self.lock.package.retain(|entry| {
                        !(entry.name == dependency.name
                            && entry.source == source
                            && entry.selector == selector)
                    });
                    self.lock.package.push(LockedPackage {
                        name: dependency.name.clone(),
                        source: source.clone(),
                        selector: selector.clone(),
                        rev: fetched_revision,
                        content_hash: String::new(),
                    });
                }
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
                            && entry.source == source
                            && entry.selector == selector
                    })
                    .map(|entry| entry.rev.clone())
                    .ok_or_else(|| {
                        io::Error::new(io::ErrorKind::InvalidData, "Git revision was not recorded")
                    })?;
                let content_hash = format!("blake3:{}", hex_digest(&identity.source_fingerprint));
                let final_destination = self.cache_root.join(&content_hash[7..]);
                if final_destination != destination {
                    if final_destination.exists() {
                        let _ = fs::remove_dir_all(&destination);
                    } else {
                        fs::rename(&destination, &final_destination)?;
                    }
                }
                self.lock.package.retain(|entry| {
                    !(entry.name == dependency.name
                        && entry.source == source
                        && entry.selector == selector)
                });
                self.lock.package.push(LockedPackage {
                    name: dependency.name.clone(),
                    source,
                    selector,
                    rev,
                    content_hash,
                });
                self.lock.package.sort_by(|left, right| {
                    (&left.name, &left.source, &left.selector).cmp(&(
                        &right.name,
                        &right.source,
                        &right.selector,
                    ))
                });
                self.dirty = true;
                Ok(final_destination)
            }
        }
    }

    fn lock_state(&mut self) -> io::Result<()> {
        if self.state_lock.is_none() {
            let state_directory = self
                .cache_root
                .parent()
                .expect("package cache has a state directory");
            fs::create_dir_all(state_directory)?;
            let lock = OpenOptions::new()
                .create(true)
                .truncate(false)
                .read(true)
                .write(true)
                .open(state_directory.join("packages.lock"))?;
            lock.lock()?;
            self.lock = PackageLock::read(&self.lock_path)?;
            self.state_lock = Some(lock);
        }
        Ok(())
    }

    fn finish(self) -> io::Result<()> {
        if self.dirty {
            self.lock.write_atomic(&self.lock_path)?;
        }
        Ok(())
    }
}

fn allocate_package_staging(cache_root: &Path) -> io::Result<PathBuf> {
    static NEXT_STAGING_ID: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
    for _ in 0..32 {
        let sequence = NEXT_STAGING_ID.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        let path = cache_root.join(format!(".fetch-{}-{sequence}.tmp", std::process::id()));
        match fs::create_dir(&path) {
            Ok(()) => return Ok(path),
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
        let mut fetcher = PackageFetcher::for_root(root)?;
        let mut nodes = Vec::new();
        let mut pending = vec![(root.to_path_buf(), String::new(), true, Vec::new())];
        let mut seen = BTreeSet::new();
        let mut edges = Vec::new();
        while let Some((current, logical_root, is_root, ancestors)) = pending.pop() {
            let canonical = current.canonicalize()?;
            if ancestors.iter().any(|ancestor| ancestor == &canonical) {
                return Err(io::Error::new(
                    io::ErrorKind::InvalidData,
                    format!("package dependency cycle at `{}`", display_path(&canonical)),
                ));
            }
            if !seen.insert((canonical.clone(), logical_root.clone())) {
                continue;
            }
            if !is_root && !canonical.join("nia.toml").is_file() {
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
            for dependency in &manifest.dependencies {
                let child_root = if logical_root.is_empty() {
                    dependency.name.clone()
                } else {
                    format!("{logical_root}/{}", dependency.name)
                };
                if !options.includes(dependency.scope) {
                    let source_root = match &dependency.source {
                        DependencySource::Path(_) => {
                            dependency_path(&mut fetcher, &canonical, dependency)
                                .ok()
                                .and_then(|path| path.canonicalize().ok())
                        }
                        DependencySource::Git { .. } => None,
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
                let path = dependency_path(&mut fetcher, &canonical, dependency)
                    .and_then(|path| {
                        path.canonicalize().map_err(|error| {
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
                pending.push((path, child_root, false, next_ancestors));
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
    let path = root.join(library_path);
    let metadata = fs::symlink_metadata(&path).map_err(|error| {
        io::Error::new(
            io::ErrorKind::InvalidData,
            format!("package library entry `{library_path}` is unavailable: {error}"),
        )
    })?;
    if !metadata.is_file() || metadata.file_type().is_symlink() {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            format!("package library entry `{library_path}` must be a regular file"),
        ));
    }
    let canonical = path.canonicalize()?;
    if !canonical.starts_with(root) {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            format!("package library entry `{library_path}` escapes the package root"),
        ));
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
                version: "0.0.0".into(),
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
            version: resolve_inheritable(
                raw_package.version,
                workspace.as_ref().and_then(|ws| ws.package.version.clone()),
                "package.version",
            )?
            .unwrap_or_else(|| "0.0.0".into()),
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
        let canonical_root = root.canonicalize()?;
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
            if !member_path.join("nia.toml").is_file() {
                return Err(io::Error::new(
                    io::ErrorKind::InvalidData,
                    format!("workspace member `{member}` does not contain nia.toml"),
                ));
            }
            let physical_root = member_path.canonicalize()?;
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
                    if directory.join("nia.toml").is_file()
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
            root: root.canonicalize()?,
            members,
            default_members,
            exclude,
            package,
            dependencies,
        })
    }

    /// Finds the nearest workspace containing `package_root` as a declared member.
    pub fn discover_for_package(package_root: &Path) -> io::Result<Option<Self>> {
        let package_root = package_root.canonicalize()?;
        let mut cursor = package_root.parent();
        while let Some(root) = cursor {
            let manifest_path = root.join("nia.toml");
            if manifest_path.is_file() {
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
        let start = if start.is_file() {
            start.parent().unwrap_or(start).to_path_buf()
        } else {
            start.to_path_buf()
        };
        let start = start.canonicalize()?;
        let mut cursor = Some(start.as_path());
        let mut package_root = None;
        while let Some(root) = cursor {
            let manifest = root.join("nia.toml");
            if manifest.is_file() && read_raw_manifest(root)?.workspace.is_some() {
                if let Some(package_root) = package_root {
                    return Self::discover_for_package(package_root);
                }
                return Self::read(root).map(Some);
            }
            if package_root.is_none()
                && (manifest.is_file()
                    || root.join("build.nia").is_file()
                    || root.join("pkg.nia").is_file())
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
                root: root.canonicalize()?,
            });
        }
        Ok(members)
    }
}

fn workspace_state_root(package_root: &Path) -> io::Result<PathBuf> {
    if !package_root.join("nia.toml").is_file() {
        return package_root.canonicalize();
    }
    let raw = read_raw_manifest(package_root)?;
    if raw.workspace.is_some() {
        return package_root.canonicalize();
    }
    Ok(WorkspaceManifest::discover_for_package(package_root)?
        .map(|workspace| workspace.root)
        .unwrap_or(package_root.canonicalize()?))
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
        let relative = root
            .canonicalize()?
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
    }
    Ok((raw, workspace))
}

fn read_raw_manifest(root: &Path) -> io::Result<RawPackageManifest> {
    let path = root.join("nia.toml");
    let mut contents = Vec::new();
    File::open(path)?
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
    let package_root = package_root.canonicalize()?;
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
            if !path.is_dir() || !path.join("nia.toml").is_file() {
                continue;
            }
            let canonical = path.canonicalize()?;
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
    rev: Option<String>,
    #[serde(default)]
    tag: Option<String>,
    #[serde(default)]
    branch: Option<String>,
}

impl RawDependency {
    fn into_source(self, name: &str) -> io::Result<DependencySource> {
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

fn dependency_path(
    fetcher: &mut PackageFetcher,
    root: &Path,
    dependency: &PackageDependency,
) -> io::Result<std::path::PathBuf> {
    fetcher.resolve(root, dependency)
}

fn validate_package_id(package: &PackageId) -> io::Result<()> {
    if package.namespace.is_empty()
        || package.name.is_empty()
        || package.version.is_empty()
        || package.namespace.len() > 128
        || package.name.len() > 128
        || package.version.len() > 128
        || package.namespace.contains('/')
        || package.name.contains('/')
        || package.version.contains('\n')
    {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            "invalid package identity",
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
    let root = root.canonicalize()?;
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
    if root.join("nia.toml").is_file() && read_raw_manifest(&root)?.workspace.is_some() {
        let workspace = WorkspaceManifest::read(&root)?;
        for member in workspace.members {
            if member != "." {
                excludes.insert(validate_selection_path(&member)?);
            }
        }
    }
    let mut files = Vec::new();
    let mut seen_files = BTreeSet::new();
    for source_root in &selected_roots {
        let directory = if source_root == "." {
            root.clone()
        } else {
            root.join(source_root)
        };
        let canonical = directory.canonicalize()?;
        if !canonical.starts_with(&root) {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                "package source root escapes the package directory",
            ));
        }
        collect_source_files(&root, &directory, &excludes, &mut files, &mut seen_files)?;
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
    root: &Path,
    directory: &Path,
    excludes: &BTreeSet<String>,
    files: &mut Vec<PackageSourceFile>,
    seen_files: &mut BTreeSet<String>,
) -> io::Result<()> {
    let mut pending = vec![directory.to_path_buf()];
    while let Some(directory) = pending.pop() {
        for entry in std::fs::read_dir(&directory)? {
            let entry = entry?;
            let path = entry.path();
            let relative = path.strip_prefix(root).expect("source path under root");
            let name = relative
                .to_str()
                .ok_or_else(|| {
                    io::Error::new(
                        io::ErrorKind::InvalidData,
                        "package source paths must be valid UTF-8",
                    )
                })?
                .replace('\\', "/");
            let source_relative = path
                .strip_prefix(&directory)
                .expect("source path under selected source root");
            if excluded_source_component(source_relative)
                || excludes
                    .iter()
                    .any(|exclude| name == *exclude || name.starts_with(&format!("{exclude}/")))
            {
                continue;
            }
            let file_type = entry.file_type()?;
            if file_type.is_symlink() {
                continue;
            }
            if file_type.is_dir() {
                pending.push(path);
            } else if file_type.is_file() {
                if !seen_files.insert(name.clone()) {
                    continue;
                }
                if files.len() >= MAX_SOURCE_FILES {
                    return Err(io::Error::new(
                        io::ErrorKind::InvalidData,
                        "package source contains too many files",
                    ));
                }
                let (byte_len, fingerprint) = fingerprint_source_file(&path)?;
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

fn fingerprint_source_file(path: &Path) -> io::Result<(u64, [u8; 32])> {
    let mut file = File::open(path)?;
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

fn excluded_source_component(path: &Path) -> bool {
    static EXCLUDED: &[&str] = &[
        ".git",
        ".nia",
        ".nia-build",
        ".nia-cache",
        "nia.lock",
        "target",
        "build",
        "dist",
    ];
    path.components().any(|component| {
        let value = component.as_os_str().to_string_lossy();
        EXCLUDED.iter().any(|excluded| value == *excluded)
    })
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
    validate_string(&id.version)
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
    put_string(output, &id.version)
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
        version: get_string(cursor)?,
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
            version: "1.0.0".into(),
        }
    }

    #[test]
    fn package_canonical_text_escapes_component_delimiters_injectively() {
        let first = PackageId {
            namespace: "acme".into(),
            name: "lib@tools".into(),
            version: "1.0".into(),
        };
        let second = PackageId {
            namespace: "acme".into(),
            name: "lib".into(),
            version: "tools@1.0".into(),
        };

        assert_eq!(first.canonical_text(), "acme/lib%40tools@1.0");
        assert_eq!(second.canonical_text(), "acme/lib@tools%401.0");
        assert_ne!(first.canonical_text(), second.canonical_text());

        let escaped = PackageId {
            namespace: "acme/team".into(),
            name: "%40@\0\u{7f}".into(),
            version: "\u{e9}\u{4e2d}".into(),
        };
        assert_eq!(
            escaped.canonical_text(),
            "acme%2Fteam/%2540%40%00%7F@%C3%A9%E4%B8%AD"
        );
        assert_ne!(canonical_component("%40"), canonical_component("@"));
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
                source: "https://example.com/regex.git".into(),
                selector: "branch:main".into(),
                rev: "0123456789012345678901234567890123456789".into(),
                content_hash: format!("blake3:{}", "a".repeat(64)),
            }],
        };
        lock.write_atomic(&path).unwrap();
        assert_eq!(PackageLock::read(&path).unwrap(), lock);
        fs::write(&path, "version = 99\n").unwrap();
        assert_eq!(
            PackageLock::read(&path).unwrap_err().kind(),
            io::ErrorKind::InvalidData
        );
        let _ = fs::remove_dir_all(root);
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
            source: "https://example.com/regex.git".into(),
            selector: selector.into(),
            rev: "0123456789012345678901234567890123456789".into(),
            content_hash: format!("blake3:{}", "a".repeat(64)),
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
        fs::write(repository.join("nia.toml"), "[package]\nname = \"dep\"\n").unwrap();
        fs::write(repository.join("src/lib.nia"), "pub fn dependency() {}\n").unwrap();
        git(&["add", "nia.toml", "src"]);
        git(&["commit", "-qm", "fixture"]);
        let repository_url = format!(
            "file:///{}",
            repository.to_string_lossy().replace('\\', "/")
        );
        let old_entry = LockedPackage {
            name: "dep".into(),
            source: repository_url.clone(),
            selector: "tag:previous".into(),
            rev: "0123456789012345678901234567890123456789".into(),
            content_hash: format!("blake3:{}", "b".repeat(64)),
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
        let mut fetcher = PackageFetcher::for_root(&consumer).unwrap();
        fetcher.resolve(&consumer, &dependency).unwrap();
        assert!(fetcher.lock.package.contains(&old_entry));
        assert!(
            fetcher
                .lock
                .package
                .iter()
                .any(|entry| entry.name == "dep" && entry.selector == "branch:main")
        );

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
        let identity = package_source_identity(&root, package()).unwrap();
        assert!(identity.files.iter().all(|file| file.path != "linked.nia"));
        std::fs::remove_file(root.join("linked.nia")).unwrap();
        std::fs::remove_file(outside).unwrap();
        std::fs::remove_dir_all(root).unwrap();
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
        let default_identity = package_source_identity(&root, package.clone()).unwrap();
        assert!(default_identity.files.is_empty());

        let selected = PackageSourceSelection {
            source_roots: vec!["build".into(), "src/build".into()],
            excludes: Vec::new(),
        };
        let identity = package_source_identity_with_selection(&root, package, &selected).unwrap();
        let paths = identity
            .files
            .iter()
            .map(|file| file.path.as_str())
            .collect::<Vec<_>>();
        assert_eq!(paths, ["build/main.nia", "src/build/ignored.nia"]);

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
                format!("[package]\nname = \"demo\"\nprotocol-epoch = {epoch}\n"),
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
            "[package]\nname = \"root\"\n[dependencies]\nhelper = { path = \"../third:party\" }\n",
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
            "[package]\nname = \"root\"\n[dependencies]\nhelper = { path = \"/tmp/helper\" }\n",
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
        assert_eq!(manifest.package.version, "0.3.0-dev");
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
        assert_eq!(package.package.version, "1.2.3");
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
        std::fs::write(member.join("nia.toml"), "[package]\nname = \"app\"\n").unwrap();

        let fetcher = PackageFetcher::for_root(&member).unwrap();
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
        fs::write(cached.join("nia.toml"), "[package]\nname = \"cached\"\n").unwrap();
        assert!(PackageManifest::read(&cached).is_ok());
        std::fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn workspace_globs_exclusions_and_independent_nested_workspaces() {
        let root = std::env::temp_dir().join(format!("nia-workspace-globs-{}", std::process::id()));
        for name in ["app", "core", "excluded"] {
            fs::create_dir_all(root.join("packages").join(name)).unwrap();
            fs::write(
                root.join("packages").join(name).join("nia.toml"),
                format!("[package]\nname = \"{name}\"\n"),
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
            "[workspace]\n[package]\nname = \"excluded\"\n",
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
            "[workspace]\n[package]\nname = \"core\"\n",
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
                "[package]\nname = \"app\"\n",
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
            "[workspace]\nmembers = [\"crates/lib\"]\n[package]\nname = \"root\"\n",
        )
        .unwrap();
        std::fs::write(
            root.join("crates/lib/nia.toml"),
            "[package]\nname = \"lib\"\n",
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
        assert!(PackageManifest::read(&member).is_err());
        std::fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn manifest_accepts_git_sources_with_one_optional_revision_selector() {
        let root = std::env::temp_dir().join(format!("nia-git-manifest-{}", std::process::id()));
        std::fs::create_dir_all(&root).unwrap();
        std::fs::write(
            root.join("nia.toml"),
            "[package]\nname = \"root\"\n[build-dependencies]\njson = { git = \"https://example.test/json.git\", tag = \"v1\" }\n[dependencies]\nregex = { git = \"ssh://git@example.test/regex.git\", rev = \"0123456789abcdef0123456789abcdef01234567\" }\n",
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
            "[package]\nname = \"root\"\n[dependencies]\nregex = { git = \"git@example.test:org/regex.git\" }\n",
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
            "[package]\nname = \"root\"\n[dependencies]\ninvalid = { git = \"https://example.test/x.git\", rev = \"a\", branch = \"main\" }\n",
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
                format!("[package]\nname = \"root\"\n[dependencies]\n{declaration}\n"),
            )
            .unwrap();
            assert!(PackageManifest::read(&root).is_err(), "{declaration}");
        }
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
        std::fs::write(repository.join("nia.toml"), "[package]\nname = \"dep\"\n").unwrap();
        std::fs::write(repository.join("src/lib.nia"), "pub fn dependency() {}\n").unwrap();
        git(&["add", "nia.toml", "src"]);
        git(&["commit", "-qm", "fixture"]);
        let repository_url = format!(
            "file:///{}",
            repository.to_string_lossy().replace('\\', "/")
        );
        std::fs::write(
            consumer.join("nia.toml"),
            format!(
                "[package]\nname = \"consumer\"\n[dependencies]\ndep = {{ git = \"{repository_url}\", branch = \"main\" }}\n"
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
        assert_eq!(lock.package[0].selector, "branch:main");
        assert_eq!(lock.package[0].rev.len(), 40);
        assert!(lock.package[0].content_hash.starts_with("blake3:"));

        std::fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn local_package_graph_resolves_without_loading_sources() {
        let root = std::env::temp_dir().join(format!("nia-graph-{}", std::process::id()));
        let dep = root.join("dep");
        std::fs::create_dir_all(&dep).unwrap();
        std::fs::write(
            root.join("nia.toml"),
            "[package]\nname = \"root\"\n[dependencies]\ndep = { path = \"dep\" }\n",
        )
        .unwrap();
        std::fs::write(dep.join("nia.toml"), "[package]\nname = \"dep\"\n").unwrap();
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
            "[package]\nname = \"root\"\n[dependencies]\ndep = { path = \"dep\" }\n",
        )
        .unwrap();
        std::fs::write(
            dep.join("nia.toml"),
            "[package]\nname = \"dep\"\n[dependencies]\nroot = { path = \"..\" }\n",
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
            "[package]\nname = \"root\"\n[dependencies]\nmathlib = { path = \"vendor/mathlib\" }\n",
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
            "[package]\nname = \"root\"\n[dependencies]\nmathlib = { path = \"vendor/mathlib\" }\n",
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
    fn local_package_graph_names_nested_dependency_chain() {
        let root = std::env::temp_dir().join(format!("nia-nested-dep-{}", std::process::id()));
        let json = root.join("vendor/json");
        let regex = json.join("vendor/regex");
        std::fs::create_dir_all(&regex).unwrap();
        std::fs::write(
            root.join("nia.toml"),
            "[package]\nname = \"root\"\n[dependencies]\njson = { path = \"vendor/json\" }\n",
        )
        .unwrap();
        std::fs::write(
            json.join("nia.toml"),
            "[package]\nname = \"json\"\n[dependencies]\nregex = { path = \"vendor/regex\" }\n",
        )
        .unwrap();
        std::fs::write(
            regex.join("nia.toml"),
            "[package]\nname = \"regex\"\nunknown = 1\n",
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
        std::fs::write(root.join("nia.toml"), "[package]\nname = \"root\"\n[build-dependencies]\nbuild = { path = \"build\" }\n[dependencies]\nruntime = { path = \"runtime\" }\n[dev-dependencies]\ndevelopment = { path = \"development\" }\n").unwrap();
        for (dir, name) in [
            (&build, "build"),
            (&runtime, "runtime"),
            (&development, "development"),
        ] {
            std::fs::write(
                dir.join("nia.toml"),
                format!("[package]\nname = \"{name}\"\n"),
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
            "[package]\nname = \"root\"\n[dev-dependencies]\ndevelopment = { path = \"development\" }\n",
        )
        .unwrap();
        std::fs::write(
            development.join("nia.toml"),
            "[package]\nname = \"development\"\n",
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
            "[package]\nname = \"root\"\n[dependencies]\nruntime = { path = \"shared\" }\n[dev-dependencies]\ndevelopment = { path = \"shared\" }\n",
        )
        .unwrap();
        std::fs::write(shared.join("nia.toml"), "[package]\nname = \"shared\"\n").unwrap();

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
            "[package]\nname = \"root\"\n[dev-dependencies]\ndevelopment = { path = \"development\" }\n",
        )
        .unwrap();
        std::fs::create_dir_all(root.join("development")).unwrap();
        std::fs::write(
            root.join("development/nia.toml"),
            "[package]\nname = \"development\"\n",
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
            "[package]\nname = \"root\"\n[library]\npath = \"src/first.nia\"\n[source]\nroots = [\"src\"]\n",
        )
        .unwrap();
        std::fs::write(root.join("src/first.nia"), "pub const value: u8 = 1;").unwrap();
        std::fs::write(root.join("src/second.nia"), "pub const value: u8 = 1;").unwrap();
        let first = PackageGraph::resolve(&root, false).unwrap().fingerprint();

        std::fs::write(
            root.join("nia.toml"),
            "[package]\nname = \"root\"\n[library]\npath = \"src/second.nia\"\n[source]\nroots = [\"src\"]\n",
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
            "[package]\nname = \"root\"\n[library]\npath = \"../outside.nia\"\n",
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
            "[package]\nname = \"root\"\n[library]\npath = \"lib.nia\"\n[source]\nroots = [\"src\"]\n",
            "[package]\nname = \"root\"\n[library]\npath = \"src/lib.nia\"\n[source]\nroots = [\"src\"]\nexclude = [\"src/lib.nia\"]\n",
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
            "[package]\nname = \"root\"\n[library]\npath = \"src/lib.nia\"\n",
        )
        .unwrap();

        let error = PackageGraph::resolve(&root, false).unwrap_err();
        assert!(error.to_string().contains("must be a regular file"));

        std::fs::remove_dir_all(root).unwrap();
        std::fs::remove_file(outside).unwrap();
    }
}
