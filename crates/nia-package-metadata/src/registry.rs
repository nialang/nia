// SPDX-License-Identifier: GPL-3.0-or-later
//! Strict registry index records and parsing.

use std::{
    fmt,
    fs::{self, OpenOptions},
    io::{self, Read, Write},
    path::{Component, Path, PathBuf},
    sync::{
        Arc,
        atomic::{AtomicU64, Ordering},
    },
};

use cap_std::fs::OpenOptions as CapOpenOptions;
use cap_std::{ambient_authority, fs::Dir};
use serde::Deserialize;

use crate::{PackageId, PackageVersion};

const INDEX_SCHEMA_VERSION: u32 = 1;
const MAX_INDEX_BYTES: usize = 1024 * 1024;
const MAX_INDEX_PACKAGES: usize = 100_000;
const PACKAGE_ARCHIVE_MAGIC: &[u8; 8] = b"NIAPKG\0\0";
const PACKAGE_ARCHIVE_SCHEMA: u32 = 1;
const MAX_PACKAGE_ARCHIVE_BYTES: usize = 256 * 1024 * 1024;
const MAX_PACKAGE_ARCHIVE_ENTRIES: usize = 100_000;
const MAX_PACKAGE_ARCHIVE_PATH_BYTES: usize = 4096;
static STAGING_SEQUENCE: AtomicU64 = AtomicU64::new(0);

/// One validated versioned registry index.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RegistryIndex {
    pub packages: Vec<RegistryIndexPackage>,
}

/// Immutable archive record for one published package release.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RegistryIndexPackage {
    pub package: PackageId,
    pub archive_url: String,
    pub archive_digest: [u8; 32],
}

/// One entry decoded from a v1 package archive.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RegistryArchiveEntry {
    pub path: String,
    pub kind: RegistryArchiveEntryKind,
    pub contents: Vec<u8>,
}

/// Validated on-disk registry cache rooted outside the package source tree.
#[derive(Debug, Clone)]
pub struct RegistryCache {
    root: PathBuf,
}

/// Configured registry endpoint used for index retrieval.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RegistryEndpoint {
    pub name: String,
    pub base_url: String,
}

/// Registry transport backed by a validated endpoint and content cache.
pub(super) type RegistryFetch = Arc<dyn Fn(&str) -> io::Result<Vec<u8>> + Send + Sync>;

#[derive(Clone)]
pub struct RegistryClient {
    endpoint: RegistryEndpoint,
    cache: RegistryCache,
    fetch: Option<RegistryFetch>,
}

impl fmt::Debug for RegistryClient {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("RegistryClient")
            .field("endpoint", &self.endpoint)
            .field("cache", &self.cache)
            .finish_non_exhaustive()
    }
}

impl RegistryClient {
    pub fn new(endpoint: RegistryEndpoint, cache: RegistryCache) -> Self {
        Self {
            endpoint,
            cache,
            fetch: None,
        }
    }

    pub(super) fn with_fetcher(
        endpoint: RegistryEndpoint,
        cache: RegistryCache,
        fetch: impl Fn(&str) -> io::Result<Vec<u8>> + Send + Sync + 'static,
    ) -> Self {
        Self {
            endpoint,
            cache,
            fetch: Some(Arc::new(fetch)),
        }
    }

    fn fetch(&self, url: &str) -> io::Result<Vec<u8>> {
        match &self.fetch {
            Some(fetch) => fetch(url),
            None => nia_git::fetch_https(url),
        }
    }

    /// Returns the cached index offline or fetches and validates the current index.
    pub fn index(&self, offline: bool) -> io::Result<RegistryIndex> {
        if offline {
            return self.cache.read_index(&self.endpoint.name);
        }
        let bytes = self.fetch(&self.endpoint.index_url())?;
        self.cache.write_index(&self.endpoint.name, &bytes)
    }

    /// Returns an archive from cache or fetches, verifies, and caches it.
    pub fn archive(&self, package: &RegistryIndexPackage, offline: bool) -> io::Result<Vec<u8>> {
        match self.cache.read_archive(&package.archive_digest) {
            Ok(bytes) => return Ok(bytes),
            Err(error) if error.kind() == io::ErrorKind::NotFound && offline => {
                return Err(io::Error::new(
                    io::ErrorKind::NotFound,
                    "registry package archive is not cached and offline mode forbids fetching",
                ));
            }
            Err(error) if error.kind() == io::ErrorKind::NotFound => {}
            Err(error) => return Err(error),
        }
        if offline {
            return Err(io::Error::new(
                io::ErrorKind::NotFound,
                "registry package archive is not cached and offline mode forbids fetching",
            ));
        }
        let bytes = self.fetch(&package.archive_url)?;
        self.cache.write_archive(&package.archive_digest, &bytes)?;
        Ok(bytes)
    }
}

impl RegistryEndpoint {
    /// Validates a registry name and HTTPS base URL.
    pub fn new(name: impl Into<String>, base_url: impl Into<String>) -> io::Result<Self> {
        let name = name.into();
        let base_url = base_url.into();
        validate_registry_name(&name)?;
        validate_registry_base_url(&base_url)?;
        Ok(Self { name, base_url })
    }

    /// Returns the canonical index URL beneath this endpoint.
    pub fn index_url(&self) -> String {
        format!("{}/index.toml", self.base_url.trim_end_matches('/'))
    }
}

impl RegistryCache {
    /// Creates a cache handle. The directory is created on first write.
    pub fn new(root: impl Into<PathBuf>) -> Self {
        Self { root: root.into() }
    }

    /// Reads and validates the cached index for a registry.
    pub fn read_index(&self, registry: &str) -> io::Result<RegistryIndex> {
        validate_registry_name(registry)?;
        let path = self.index_path(registry);
        let bytes = read_bounded(&path, MAX_INDEX_BYTES)?;
        RegistryIndex::parse_toml(&bytes)
    }

    /// Validates and atomically replaces a registry index cache entry.
    pub fn write_index(&self, registry: &str, bytes: &[u8]) -> io::Result<RegistryIndex> {
        validate_registry_name(registry)?;
        let index = RegistryIndex::parse_toml(bytes)?;
        let path = self.index_path(registry);
        write_atomic(&path, bytes)?;
        Ok(index)
    }

    /// Reads a digest-addressed archive after rechecking its complete digest.
    pub fn read_archive(&self, digest: &[u8; 32]) -> io::Result<Vec<u8>> {
        let bytes = read_bounded(&self.archive_path(digest), MAX_PACKAGE_ARCHIVE_BYTES)?;
        if blake3::hash(&bytes).as_bytes() != digest {
            return Err(invalid_index("cached registry archive digest mismatch"));
        }
        Ok(bytes)
    }

    /// Validates and atomically publishes a digest-addressed archive.
    pub fn write_archive(&self, digest: &[u8; 32], bytes: &[u8]) -> io::Result<()> {
        if blake3::hash(bytes).as_bytes() != digest {
            return Err(invalid_index("registry archive digest mismatch"));
        }
        decode_registry_archive(bytes, digest)?;
        write_atomic(&self.archive_path(digest), bytes)
    }

    fn index_path(&self, registry: &str) -> PathBuf {
        self.root.join("indexes").join(registry).join("index.toml")
    }

    fn archive_path(&self, digest: &[u8; 32]) -> PathBuf {
        self.root
            .join("archives")
            .join(format!("{}.nia", digest_hex(digest)))
    }
}

/// Supported entry types in the package archive format.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RegistryArchiveEntryKind {
    File,
    Directory,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct RawRegistryIndex {
    schema_version: u32,
    packages: Vec<RawRegistryIndexPackage>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct RawRegistryIndexPackage {
    namespace: String,
    name: String,
    version: String,
    archive_url: String,
    archive_digest: String,
}

impl RegistryIndex {
    /// Parses a bounded v1 TOML index and rejects noncanonical record order.
    pub fn parse_toml(input: &[u8]) -> io::Result<Self> {
        if input.len() > MAX_INDEX_BYTES {
            return Err(invalid_index("registry index exceeds the size limit"));
        }
        let raw: RawRegistryIndex = toml::from_slice(input).map_err(|error| {
            io::Error::new(
                io::ErrorKind::InvalidData,
                format!("invalid registry index: {error}"),
            )
        })?;
        if raw.schema_version != INDEX_SCHEMA_VERSION {
            return Err(invalid_index("unsupported registry index schema version"));
        }
        if raw.packages.len() > MAX_INDEX_PACKAGES {
            return Err(invalid_index("registry index has too many package records"));
        }

        let mut packages = Vec::with_capacity(raw.packages.len());
        let mut previous: Option<(String, String, String)> = None;
        for record in raw.packages {
            if !valid_package_segment(&record.namespace) || !valid_package_segment(&record.name) {
                return Err(invalid_index(
                    "registry index contains an invalid package identity",
                ));
            }
            let version = PackageVersion::parse(&record.version).map_err(|error| {
                io::Error::new(
                    io::ErrorKind::InvalidData,
                    format!("registry index contains an invalid SemVer version: {error}"),
                )
            })?;
            let key = (
                record.namespace.clone(),
                record.name.clone(),
                record.version.clone(),
            );
            if previous.as_ref().is_some_and(|previous| previous >= &key) {
                return Err(invalid_index(
                    "registry index package records must be uniquely sorted",
                ));
            }
            previous = Some(key);
            validate_archive_url(&record.archive_url)?;
            let archive_digest = parse_blake3_digest(&record.archive_digest)?;
            packages.push(RegistryIndexPackage {
                package: PackageId {
                    namespace: record.namespace,
                    name: record.name,
                    version,
                },
                archive_url: record.archive_url,
                archive_digest,
            });
        }
        Ok(Self { packages })
    }
}

/// Decodes and validates a digest-pinned Nia package archive.
pub fn decode_registry_archive(
    archive: &[u8],
    expected_digest: &[u8; 32],
) -> io::Result<Vec<RegistryArchiveEntry>> {
    if archive.len() > MAX_PACKAGE_ARCHIVE_BYTES {
        return Err(invalid_index(
            "registry package archive exceeds the size limit",
        ));
    }
    if blake3::hash(archive).as_bytes() != expected_digest {
        return Err(invalid_index("registry package archive digest mismatch"));
    }
    let mut reader = ArchiveReader::new(archive);
    if reader.take(8)? != PACKAGE_ARCHIVE_MAGIC {
        return Err(invalid_index("registry package archive has invalid magic"));
    }
    if reader.u32()? != PACKAGE_ARCHIVE_SCHEMA {
        return Err(invalid_index("unsupported registry package archive schema"));
    }
    let count = reader.u32()? as usize;
    if count > MAX_PACKAGE_ARCHIVE_ENTRIES {
        return Err(invalid_index(
            "registry package archive has too many entries",
        ));
    }
    let mut entries = Vec::with_capacity(count);
    let mut previous: Option<String> = None;
    for _ in 0..count {
        let kind = match reader.u8()? {
            0 => RegistryArchiveEntryKind::File,
            1 => RegistryArchiveEntryKind::Directory,
            _ => {
                return Err(invalid_index(
                    "registry package archive entry type is invalid",
                ));
            }
        };
        let path_len = reader.u32()? as usize;
        if path_len == 0 || path_len > MAX_PACKAGE_ARCHIVE_PATH_BYTES {
            return Err(invalid_index(
                "registry package archive path length is invalid",
            ));
        }
        let path = std::str::from_utf8(reader.take(path_len)?)
            .map_err(|_| invalid_index("registry package archive path is not UTF-8"))?
            .to_owned();
        validate_archive_path(&path)?;
        if previous.as_ref().is_some_and(|previous| previous >= &path) {
            return Err(invalid_index(
                "registry package archive paths must be uniquely sorted",
            ));
        }
        previous = Some(path.clone());
        let contents_len = usize::try_from(reader.u64()?)
            .map_err(|_| invalid_index("registry package archive file is too large"))?;
        if matches!(kind, RegistryArchiveEntryKind::Directory) && contents_len != 0 {
            return Err(invalid_index("registry package directory has contents"));
        }
        let contents = reader.take(contents_len)?.to_vec();
        entries.push(RegistryArchiveEntry {
            path,
            kind,
            contents,
        });
    }
    if !reader.is_empty() {
        return Err(invalid_index("registry package archive has trailing bytes"));
    }
    validate_archive_tree(&entries)?;
    Ok(entries)
}

/// Verifies and atomically extracts a package archive into a new directory.
pub fn extract_registry_archive(
    archive: &[u8],
    expected_digest: &[u8; 32],
    destination: &Path,
) -> io::Result<()> {
    let entries = decode_registry_archive(archive, expected_digest)?;
    let parent = destination.parent().unwrap_or_else(|| Path::new("."));
    fs::create_dir_all(parent)?;
    let destination_name = destination
        .file_name()
        .filter(|name| !name.is_empty())
        .ok_or_else(|| invalid_index("registry package destination has no name"))?;
    let parent_dir = Dir::open_ambient_dir(parent, ambient_authority())?;
    let staging_name = allocate_archive_staging(&parent_dir)?;
    let result = (|| {
        {
            let staging_dir = parent_dir.open_dir(&staging_name)?;
            write_archive_entries(&staging_dir, &entries)?;
        }
        match parent_dir.symlink_metadata(destination_name) {
            Ok(_) => {
                return Err(io::Error::new(
                    io::ErrorKind::AlreadyExists,
                    "registry package destination already exists",
                ));
            }
            Err(error) if error.kind() == io::ErrorKind::NotFound => {}
            Err(error) => return Err(error),
        }
        parent_dir.rename(&staging_name, &parent_dir, destination_name)?;
        Ok(())
    })();
    if result.is_err() {
        let _ = parent_dir.remove_dir_all(&staging_name);
    }
    result
}

fn allocate_archive_staging(parent: &Dir) -> io::Result<String> {
    for _ in 0..128 {
        let sequence = STAGING_SEQUENCE.fetch_add(1, Ordering::Relaxed);
        let name = format!(".nia-package-{}-{sequence}.tmp", std::process::id());
        match parent.create_dir(&name) {
            Ok(()) => return Ok(name),
            Err(error) if error.kind() == io::ErrorKind::AlreadyExists => continue,
            Err(error) => return Err(error),
        }
    }
    Err(io::Error::new(
        io::ErrorKind::AlreadyExists,
        "could not allocate registry package staging directory",
    ))
}

fn write_archive_entries(root: &Dir, entries: &[RegistryArchiveEntry]) -> io::Result<()> {
    for entry in entries {
        let mut components = entry.path.split('/').peekable();
        let mut directory = root.try_clone()?;
        while let Some(component) = components.next() {
            if components.peek().is_some() {
                match directory.open_dir(component) {
                    Ok(child) => directory = child,
                    Err(error) if error.kind() == io::ErrorKind::NotFound => {
                        directory.create_dir(component)?;
                        directory = directory.open_dir(component)?;
                    }
                    Err(error) => return Err(error),
                }
                continue;
            }
            match entry.kind {
                RegistryArchiveEntryKind::Directory => match directory.create_dir(component) {
                    Ok(()) => {}
                    Err(error) if error.kind() == io::ErrorKind::AlreadyExists => {
                        directory.open_dir(component)?;
                    }
                    Err(error) => return Err(error),
                },
                RegistryArchiveEntryKind::File => {
                    let mut options = CapOpenOptions::new();
                    options.write(true).create_new(true);
                    let mut file = directory.open_with(component, &options)?;
                    file.write_all(&entry.contents)?;
                    file.sync_all()?;
                }
            }
        }
    }
    Ok(())
}

fn validate_archive_path(path: &str) -> io::Result<()> {
    if path.starts_with('/') || path.contains(['\\', '\0', ':']) {
        return Err(invalid_index(
            "registry package archive path is not relative",
        ));
    }
    let parsed = Path::new(path);
    let components = parsed.components().collect::<Vec<_>>();
    if components.is_empty()
        || components
            .iter()
            .any(|component| !matches!(component, Component::Normal(_)))
        || components.iter().any(|component| {
            component
                .as_os_str()
                .to_str()
                .is_none_or(|part| part.is_empty() || part == "." || part == "..")
        })
        || path
            .split('/')
            .any(|part| part.is_empty() || part == "." || part == "..")
    {
        return Err(invalid_index("registry package archive path is invalid"));
    }
    Ok(())
}

fn valid_package_segment(segment: &str) -> bool {
    !segment.is_empty()
        && segment.len() <= 128
        && segment
            .bytes()
            .all(|byte| byte.is_ascii_lowercase() || byte.is_ascii_digit() || byte == b'-')
}

fn validate_registry_name(registry: &str) -> io::Result<()> {
    if registry.is_empty()
        || registry.len() > 64
        || !registry
            .bytes()
            .all(|byte| byte.is_ascii_lowercase() || byte.is_ascii_digit() || byte == b'-')
    {
        return Err(invalid_index("registry name is invalid"));
    }
    Ok(())
}

fn validate_registry_base_url(url: &str) -> io::Result<()> {
    let Some(rest) = url.strip_prefix("https://") else {
        return Err(invalid_index("registry endpoint must use HTTPS"));
    };
    if rest.is_empty()
        || rest.contains(['?', '#', '@', '\\'])
        || rest
            .bytes()
            .any(|byte| byte.is_ascii_control() || byte.is_ascii_whitespace())
    {
        return Err(invalid_index("registry endpoint URL is invalid"));
    }
    let (authority, path) = rest.split_once('/').unwrap_or((rest, ""));
    if authority.is_empty() {
        return Err(invalid_index("registry endpoint URL is invalid"));
    }
    let (host, port) = if let Some(bracketed) = authority.strip_prefix('[') {
        let (host, suffix) = bracketed
            .split_once(']')
            .ok_or_else(|| invalid_index("registry endpoint URL is invalid"))?;
        if host.parse::<std::net::Ipv6Addr>().is_err() {
            return Err(invalid_index("registry endpoint URL is invalid"));
        }
        (host, suffix.strip_prefix(':'))
    } else if let Some((host, port)) = authority.rsplit_once(':') {
        if host.contains(':') {
            return Err(invalid_index("registry endpoint URL is invalid"));
        }
        (host, Some(port))
    } else {
        (authority, None)
    };
    if host.is_empty()
        || (!authority.starts_with('[')
            && !host
                .bytes()
                .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'.' | b'-')))
    {
        return Err(invalid_index("registry endpoint URL is invalid"));
    }
    if let Some(port) = port
        && (port.is_empty()
            || !port.bytes().all(|byte| byte.is_ascii_digit())
            || port.parse::<u16>().map_or(true, |value| value == 0))
    {
        return Err(invalid_index("registry endpoint URL is invalid"));
    }
    if path
        .trim_end_matches('/')
        .split('/')
        .any(|segment| segment == ".." || segment.is_empty())
        && !path.is_empty()
    {
        return Err(invalid_index("registry endpoint URL is invalid"));
    }
    Ok(())
}

fn digest_hex(digest: &[u8; 32]) -> String {
    let mut text = String::with_capacity(64);
    for byte in digest {
        use std::fmt::Write as _;
        write!(text, "{byte:02x}").expect("writing to a String cannot fail");
    }
    text
}

fn read_bounded(path: &Path, limit: usize) -> io::Result<Vec<u8>> {
    let file = fs::File::open(path)?;
    let mut bytes = Vec::new();
    file.take((limit + 1) as u64).read_to_end(&mut bytes)?;
    if bytes.len() > limit {
        return Err(invalid_index("registry cache entry exceeds the size limit"));
    }
    Ok(bytes)
}

fn write_atomic(path: &Path, bytes: &[u8]) -> io::Result<()> {
    let parent = path.parent().unwrap_or_else(|| Path::new("."));
    fs::create_dir_all(parent)?;
    let (temporary, mut file) = allocate_cache_staging(parent)?;
    let result = (|| {
        file.write_all(bytes)?;
        file.sync_all()?;
        drop(file);
        if fs::symlink_metadata(path).is_ok() {
            fs::remove_file(path)?;
        }
        fs::rename(&temporary, path)
    })();
    if result.is_err() {
        let _ = fs::remove_file(&temporary);
    }
    result
}

fn allocate_cache_staging(parent: &Path) -> io::Result<(PathBuf, fs::File)> {
    for _ in 0..128 {
        let sequence = STAGING_SEQUENCE.fetch_add(1, Ordering::Relaxed);
        let path = parent.join(format!(".nia-cache-{}-{sequence}.tmp", std::process::id()));
        match OpenOptions::new().write(true).create_new(true).open(&path) {
            Ok(file) => return Ok((path, file)),
            Err(error) if error.kind() == io::ErrorKind::AlreadyExists => continue,
            Err(error) => return Err(error),
        }
    }
    Err(io::Error::new(
        io::ErrorKind::AlreadyExists,
        "could not allocate registry cache staging path",
    ))
}

fn validate_archive_tree(entries: &[RegistryArchiveEntry]) -> io::Result<()> {
    let kinds = entries
        .iter()
        .map(|entry| (entry.path.as_str(), entry.kind))
        .collect::<std::collections::BTreeMap<_, _>>();
    for entry in entries {
        let mut parent = Path::new(&entry.path).parent();
        while let Some(path) = parent {
            if path.as_os_str().is_empty() {
                break;
            }
            if let Some(RegistryArchiveEntryKind::File) =
                kinds.get(path.to_str().unwrap_or_default()).copied()
            {
                return Err(invalid_index(
                    "registry package archive file is used as a directory",
                ));
            }
            parent = path.parent();
        }
    }
    Ok(())
}

struct ArchiveReader<'a> {
    bytes: &'a [u8],
    offset: usize,
}

impl<'a> ArchiveReader<'a> {
    fn new(bytes: &'a [u8]) -> Self {
        Self { bytes, offset: 0 }
    }

    fn take(&mut self, count: usize) -> io::Result<&'a [u8]> {
        let end = self
            .offset
            .checked_add(count)
            .filter(|end| *end <= self.bytes.len())
            .ok_or_else(|| invalid_index("registry package archive is truncated"))?;
        let bytes = &self.bytes[self.offset..end];
        self.offset = end;
        Ok(bytes)
    }

    fn u8(&mut self) -> io::Result<u8> {
        Ok(self.take(1)?[0])
    }

    fn u32(&mut self) -> io::Result<u32> {
        Ok(u32::from_le_bytes(self.take(4)?.try_into().unwrap()))
    }

    fn u64(&mut self) -> io::Result<u64> {
        Ok(u64::from_le_bytes(self.take(8)?.try_into().unwrap()))
    }

    fn is_empty(&self) -> bool {
        self.offset == self.bytes.len()
    }
}

fn validate_archive_url(url: &str) -> io::Result<()> {
    let Some(authority_and_path) = url.strip_prefix("https://") else {
        return Err(invalid_index("registry archive URL must use HTTPS"));
    };
    let (authority, path) = authority_and_path
        .split_once('/')
        .unwrap_or((authority_and_path, ""));
    if authority.is_empty()
        || authority.contains('@')
        || url.contains(['?', '#'])
        || path
            .bytes()
            .any(|byte| byte.is_ascii_control() || byte.is_ascii_whitespace() || byte == b'\\')
    {
        return Err(invalid_index("registry archive URL is invalid"));
    }
    let (host, port) = if let Some(bracketed) = authority.strip_prefix('[') {
        let (host, suffix) = bracketed
            .split_once(']')
            .ok_or_else(|| invalid_index("registry archive URL is invalid"))?;
        if host.parse::<std::net::Ipv6Addr>().is_err() {
            return Err(invalid_index("registry archive URL is invalid"));
        }
        let port = if suffix.is_empty() {
            None
        } else {
            Some(
                suffix
                    .strip_prefix(':')
                    .ok_or_else(|| invalid_index("registry archive URL is invalid"))?,
            )
        };
        (host, port)
    } else if let Some((host, port)) = authority.rsplit_once(':') {
        if host.contains(':') {
            return Err(invalid_index("registry archive URL is invalid"));
        }
        (host, Some(port))
    } else {
        (authority, None)
    };
    let valid_host = if authority.starts_with('[') {
        host.parse::<std::net::Ipv6Addr>().is_ok()
    } else {
        !host.is_empty()
            && host
                .bytes()
                .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'.' | b'-'))
    };
    if !valid_host {
        return Err(invalid_index("registry archive URL is invalid"));
    }
    if let Some(port) = port {
        if port.is_empty() || !port.bytes().all(|byte| byte.is_ascii_digit()) {
            return Err(invalid_index("registry archive URL is invalid"));
        }
        if port.parse::<u16>().map_or(true, |port| port == 0) {
            return Err(invalid_index("registry archive URL is invalid"));
        }
    }
    Ok(())
}

fn parse_blake3_digest(value: &str) -> io::Result<[u8; 32]> {
    let Some(hex) = value.strip_prefix("blake3:") else {
        return Err(invalid_index("registry archive digest must use blake3"));
    };
    if hex.len() != 64
        || !hex
            .bytes()
            .all(|byte| byte.is_ascii_hexdigit() && !byte.is_ascii_uppercase())
    {
        return Err(invalid_index("registry archive digest is malformed"));
    }
    let mut digest = [0; 32];
    for (index, byte) in digest.iter_mut().enumerate() {
        *byte = u8::from_str_radix(&hex[index * 2..index * 2 + 2], 16)
            .map_err(|_| invalid_index("registry archive digest is malformed"))?;
    }
    Ok(digest)
}

fn invalid_index(message: &'static str) -> io::Error {
    io::Error::new(io::ErrorKind::InvalidData, message)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn archive(entries: &[(u8, &str, &[u8])]) -> Vec<u8> {
        let mut bytes = Vec::new();
        bytes.extend_from_slice(PACKAGE_ARCHIVE_MAGIC);
        bytes.extend_from_slice(&PACKAGE_ARCHIVE_SCHEMA.to_le_bytes());
        bytes.extend_from_slice(&(entries.len() as u32).to_le_bytes());
        for (kind, path, contents) in entries {
            bytes.push(*kind);
            bytes.extend_from_slice(&(path.len() as u32).to_le_bytes());
            bytes.extend_from_slice(path.as_bytes());
            bytes.extend_from_slice(&(contents.len() as u64).to_le_bytes());
            bytes.extend_from_slice(contents);
        }
        bytes
    }

    fn record(namespace: &str, name: &str, version: &str, url: &str) -> String {
        format!(
            "[[packages]]\nnamespace = \"{namespace}\"\nname = \"{name}\"\nversion = \"{version}\"\narchive_url = \"{url}\"\narchive_digest = \"blake3:{}\"\n",
            "ab".repeat(32)
        )
    }

    #[test]
    fn registry_index_parses_sorted_https_archive_records() {
        let input = format!(
            "schema_version = 1\n{}{}",
            record(
                "acme",
                "json",
                "1.0.0",
                "https://cdn.example.test/json-1.0.0.nia"
            ),
            record(
                "acme",
                "json",
                "1.1.0",
                "https://cdn.example.test/json-1.1.0.nia"
            )
        );
        let index = RegistryIndex::parse_toml(input.as_bytes()).unwrap();
        assert_eq!(index.packages.len(), 2);
        assert_eq!(index.packages[1].package.version.as_str(), "1.1.0");
        assert_eq!(index.packages[0].archive_digest, [0xab; 32]);
    }

    #[test]
    fn registry_index_fails_closed_on_schema_order_url_and_digest_errors() {
        assert!(RegistryIndex::parse_toml(b"schema_version = 2\npackages = []\n").is_err());
        assert!(
            RegistryIndex::parse_toml(b"schema_version = 1\npackages = []\nunknown = 1\n").is_err()
        );
        let unsorted = format!(
            "schema_version = 1\n{}{}",
            record("acme", "json", "2.0.0", "https://cdn.example.test/json"),
            record("acme", "json", "1.0.0", "https://cdn.example.test/json")
        );
        assert!(RegistryIndex::parse_toml(unsorted.as_bytes()).is_err());
        for (url, digest) in [
            (
                "http://cdn.example.test/json",
                format!("blake3:{}", "ab".repeat(32)),
            ),
            (
                "https://user@cdn.example.test/json",
                format!("blake3:{}", "ab".repeat(32)),
            ),
            (
                "https://cdn.example.test/json?mutable=1",
                format!("blake3:{}", "ab".repeat(32)),
            ),
            ("https://cdn.example.test/json", "sha256:deadbeef".into()),
        ] {
            let input = format!(
                "schema_version = 1\n{}",
                record("acme", "json", "1.0.0", url)
                    .replace(&format!("blake3:{}", "ab".repeat(32)), &digest)
            );
            assert!(RegistryIndex::parse_toml(input.as_bytes()).is_err());
        }
        assert!(
            RegistryIndex::parse_toml(
                format!(
                    "schema_version = 1\n{}",
                    record("acme", "json", "1.0.0", "https://[::1]:8443/json")
                )
                .as_bytes()
            )
            .is_ok()
        );
        for namespace in ["Acme", "acme/team", "_private", ""] {
            let input = format!(
                "schema_version = 1\n{}",
                record(namespace, "json", "1.0.0", "https://cdn.example.test/json")
            );
            assert!(RegistryIndex::parse_toml(input.as_bytes()).is_err());
        }
    }

    #[test]
    fn package_archive_checks_digest_bounds_paths_and_entry_kinds() {
        let good = archive(&[
            (1, "src", b""),
            (0, "src/lib.nia", b"pub fn answer() -> i32 = 42;\n"),
        ]);
        let digest = *blake3::hash(&good).as_bytes();
        let entries = decode_registry_archive(&good, &digest).unwrap();
        assert_eq!(entries.len(), 2);
        assert_eq!(entries[1].contents, b"pub fn answer() -> i32 = 42;\n");

        let wrong_digest = [0; 32];
        assert!(decode_registry_archive(&good, &wrong_digest).is_err());
        for bad in [
            archive(&[(0, "../outside", b"x")]),
            archive(&[(0, "src\\outside", b"x")]),
            archive(&[(0, "src//outside", b"x")]),
            archive(&[(2, "link", b"target")]),
            archive(&[(0, "file", b""), (0, "file/child", b"x")]),
        ] {
            let digest = *blake3::hash(&bad).as_bytes();
            assert!(decode_registry_archive(&bad, &digest).is_err());
        }
        let mut trailing = good.clone();
        trailing.push(0);
        assert!(decode_registry_archive(&trailing, &blake3::hash(&trailing).into()).is_err());
        assert!(decode_registry_archive(&good[..good.len() - 1], &digest).is_err());
    }

    #[test]
    fn package_archive_extracts_atomically_and_refuses_existing_destinations() {
        let root = std::env::temp_dir().join(format!(
            "nia-registry-archive-{}-{}",
            std::process::id(),
            STAGING_SEQUENCE.fetch_add(1, Ordering::Relaxed)
        ));
        let destination = root.join("cache/package");
        let bytes = archive(&[(0, "nia.toml", b"[package]\nname = \"demo\"\n")]);
        let digest = *blake3::hash(&bytes).as_bytes();
        extract_registry_archive(&bytes, &digest, &destination).unwrap();
        assert_eq!(
            fs::read(destination.join("nia.toml")).unwrap(),
            b"[package]\nname = \"demo\"\n"
        );
        assert_eq!(
            extract_registry_archive(&bytes, &digest, &destination)
                .unwrap_err()
                .kind(),
            io::ErrorKind::AlreadyExists
        );
        fs::remove_dir_all(root).unwrap();
    }

    #[cfg(unix)]
    #[test]
    fn archive_extraction_writes_through_open_staging_directory_after_replacement() {
        let root = std::env::temp_dir().join(format!(
            "nia-registry-stage-race-{}-{}",
            std::process::id(),
            STAGING_SEQUENCE.fetch_add(1, Ordering::Relaxed)
        ));
        let staging = root.join("staging");
        let moved = root.join("moved-staging");
        let outside = root.join("outside");
        fs::create_dir_all(&staging).unwrap();
        fs::create_dir(&outside).unwrap();
        let staging_dir = Dir::open_ambient_dir(&staging, ambient_authority()).unwrap();
        fs::rename(&staging, &moved).unwrap();
        std::os::unix::fs::symlink(&outside, &staging).unwrap();

        write_archive_entries(
            &staging_dir,
            &[RegistryArchiveEntry {
                path: "src/lib.nia".into(),
                kind: RegistryArchiveEntryKind::File,
                contents: b"inside staging root".to_vec(),
            }],
        )
        .unwrap();

        assert_eq!(
            fs::read(moved.join("src/lib.nia")).unwrap(),
            b"inside staging root"
        );
        assert!(fs::read_dir(&outside).unwrap().next().is_none());
        fs::remove_file(&staging).unwrap();
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn registry_cache_validates_and_rechecks_content_addressed_entries() {
        let root = std::env::temp_dir().join(format!(
            "nia-registry-cache-{}-{}",
            std::process::id(),
            STAGING_SEQUENCE.fetch_add(1, Ordering::Relaxed)
        ));
        let cache = RegistryCache::new(&root);
        let index_bytes = format!(
            "schema_version = 1\n{}",
            record("acme", "json", "1.0.0", "https://cdn.example.test/json")
        )
        .into_bytes();
        assert!(cache.write_index("nia", &index_bytes).is_ok());
        assert_eq!(cache.read_index("nia").unwrap().packages.len(), 1);
        let updated_index = b"schema_version = 1\npackages = []\n";
        assert!(cache.write_index("nia", updated_index).is_ok());
        assert!(cache.read_index("nia").unwrap().packages.is_empty());
        assert!(cache.read_index("Nia").is_err());

        let bytes = archive(&[(0, "nia.toml", b"[package]\nname = \"demo\"\n")]);
        let digest = *blake3::hash(&bytes).as_bytes();
        cache.write_archive(&digest, &bytes).unwrap();
        assert_eq!(cache.read_archive(&digest).unwrap(), bytes);
        assert!(cache.write_archive(&[0; 32], &bytes).is_err());
        fs::write(cache.archive_path(&digest), b"tampered").unwrap();
        assert!(cache.read_archive(&digest).is_err());
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn registry_endpoints_require_safe_https_urls() {
        let endpoint = RegistryEndpoint::new("nia", "https://packages.example.test/api/").unwrap();
        assert_eq!(
            endpoint.index_url(),
            "https://packages.example.test/api/index.toml"
        );
        assert!(RegistryEndpoint::new("Nia", "https://packages.example.test").is_err());
        for url in [
            "http://packages.example.test",
            "https://user@packages.example.test",
            "https://packages.example.test/api/../other",
            "https://packages.example.test/index?next=other",
            "https://packages.example.test/#fragment",
        ] {
            assert!(RegistryEndpoint::new("nia", url).is_err(), "{url}");
        }
    }

    #[test]
    fn registry_client_uses_only_validated_cache_in_offline_mode() {
        let root = std::env::temp_dir().join(format!(
            "nia-registry-client-{}-{}",
            std::process::id(),
            STAGING_SEQUENCE.fetch_add(1, Ordering::Relaxed)
        ));
        let cache = RegistryCache::new(&root);
        let endpoint = RegistryEndpoint::new("nia", "https://packages.example.test").unwrap();
        let client = RegistryClient::new(endpoint, cache.clone());
        assert_eq!(
            client.index(true).unwrap_err().kind(),
            io::ErrorKind::NotFound
        );
        let bytes = archive(&[(0, "nia.toml", b"[package]\nname = \"demo\"\n")]);
        let digest = *blake3::hash(&bytes).as_bytes();
        let package = RegistryIndexPackage {
            package: PackageId {
                namespace: "acme".into(),
                name: "demo".into(),
                version: PackageVersion::parse("1.0.0").unwrap(),
            },
            archive_url: "https://cdn.example.test/demo.nia".into(),
            archive_digest: digest,
        };
        assert_eq!(
            client.archive(&package, true).unwrap_err().kind(),
            io::ErrorKind::NotFound
        );
        cache.write_archive(&digest, &bytes).unwrap();
        assert_eq!(client.archive(&package, true).unwrap(), bytes);
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn registry_client_fetches_validated_resources_and_reuses_cache() {
        use std::sync::Mutex;

        let root = std::env::temp_dir().join(format!(
            "nia-registry-client-online-{}-{}",
            std::process::id(),
            STAGING_SEQUENCE.fetch_add(1, Ordering::Relaxed)
        ));
        let cache = RegistryCache::new(&root);
        let endpoint = RegistryEndpoint::new("nia", "https://packages.example.test").unwrap();
        let archive_bytes = archive(&[(0, "nia.toml", b"[package]\nname = \"demo\"\n")]);
        let archive_digest = *blake3::hash(&archive_bytes).as_bytes();
        let archive_url = "https://cdn.example.test/demo.nia";
        let index_bytes = format!(
            "schema_version = 1\n[[packages]]\nnamespace = \"acme\"\nname = \"demo\"\nversion = \"1.0.0\"\narchive_url = \"{archive_url}\"\narchive_digest = \"blake3:{}\"\n",
            digest_hex(&archive_digest)
        )
        .into_bytes();
        let expected_archive = archive_bytes.clone();
        let calls = Arc::new(Mutex::new(Vec::<String>::new()));
        let call_log = calls.clone();
        let client = RegistryClient::with_fetcher(endpoint, cache.clone(), move |url| {
            call_log.lock().unwrap().push(url.to_owned());
            match url {
                "https://packages.example.test/index.toml" => Ok(index_bytes.clone()),
                "https://cdn.example.test/demo.nia" => Ok(expected_archive.clone()),
                _ => Err(io::Error::new(io::ErrorKind::NotFound, "unexpected URL")),
            }
        });
        let index = client.index(false).unwrap();
        let package = &index.packages[0];
        assert_eq!(client.archive(package, false).unwrap(), archive_bytes);
        assert_eq!(
            calls.lock().unwrap().as_slice(),
            [
                "https://packages.example.test/index.toml",
                "https://cdn.example.test/demo.nia"
            ]
        );
        assert_eq!(client.index(true).unwrap().packages.len(), 1);
        assert_eq!(client.archive(package, true).unwrap(), archive_bytes);
        assert_eq!(calls.lock().unwrap().len(), 2);
        fs::remove_dir_all(root).unwrap();
    }
}
