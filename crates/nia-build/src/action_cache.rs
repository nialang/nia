// SPDX-License-Identifier: GPL-3.0-or-later
//! Build-action cache records and typed invalidation reasons.
//!
//! This cache owns build bindings and generated/external payloads. Compiler
//! objects, archives, and link products remain in their Driver or linker-owned
//! caches; compiler emit records retain typed references rather than duplicate
//! those products. Cache failure is always a miss, never an output bypass.

use std::{
    collections::BTreeSet,
    fs,
    io::{self, Cursor, Read, Write},
    path::{Path, PathBuf},
    sync::atomic::{AtomicU64, Ordering},
};

use nia_compat::formats::{GENERATED_FILE_CACHE, GENERATED_FILE_ENTRY};
use nia_query::{
    FingerprintDomain, QueryFingerprint, QueryFingerprintBuilder, QueryFingerprintBytesWriter,
};
use nia_toolchain::ToolchainIdentity;

use crate::{
    ActionKey, ArtifactKey, LogicalPath, LogicalPathRoot, lock::ScopedFileLock,
    plan::MAX_PLAN_STRING_BYTES,
};

mod compiler_check;
mod compiler_emit;
mod external_command;

pub(crate) use compiler_check::{
    CompilerCheckCache, CompilerCheckCacheIdentity, CompilerCheckCacheLookup,
};
pub(crate) use compiler_emit::{
    CompilerEmitCache, CompilerEmitCacheIdentity, CompilerEmitCacheIdentityInput,
    CompilerEmitCacheLinkInput, CompilerEmitCacheLookup,
};
pub(crate) use external_command::{
    ExternalCommandCache, ExternalCommandCacheIdentity, ExternalCommandCacheLookup,
};

static CACHE_STAGE_SEQUENCE: AtomicU64 = AtomicU64::new(0);
/// Compiler action-cache entries contain identities and typed references, not
/// object payloads. Keep their budget aligned with compiler persistence files.
const MAX_COMPILER_CACHE_ENTRY_BYTES: usize = 64 * 1024 * 1024;
const GENERATED_FILE_COMPILER_DOMAIN: FingerprintDomain =
    FingerprintDomain::new("nia.build.generated-file-compiler.v1");
const GENERATED_FILE_RESOURCE_LAYOUT_DOMAIN: FingerprintDomain =
    FingerprintDomain::new("nia.build.generated-file-resource-layout.v1");
const GENERATED_FILE_STANDARD_LIBRARY_DOMAIN: FingerprintDomain =
    FingerprintDomain::new("nia.build.generated-file-standard-library.v1");
const GENERATED_FILE_BUILD_PROTOCOL_DOMAIN: FingerprintDomain =
    FingerprintDomain::new("nia.build.generated-file-build-protocol.v1");
const GENERATED_FILE_FINGERPRINT_DOMAIN: FingerprintDomain =
    FingerprintDomain::new("nia.build.generated-file-fingerprint.v1");
const ACTION_CACHE_MUTATION_LOCK_DOMAIN: FingerprintDomain =
    FingerprintDomain::new("nia.build.action-cache-mutation-lock.v1");
const GENERATED_FILE_KEY_DOMAIN: FingerprintDomain =
    FingerprintDomain::new("nia.build.generated-file-key.v1");
const GENERATED_FILE_CONTENTS_DOMAIN: FingerprintDomain =
    FingerprintDomain::new("nia.build.generated-file-contents.v1");
const GENERATED_FILE_OUTPUT_DOMAIN: FingerprintDomain =
    FingerprintDomain::new("nia.build.generated-file-output.v1");
const GENERATED_FILE_PAYLOAD_DOMAIN: FingerprintDomain =
    FingerprintDomain::new("nia.build.generated-file-payload.v1");
const GENERATED_FILE_STREAM_BUFFER_BYTES: usize = 64 * 1024;
// An artifact-rooted logical path contains at most package, artifact, and path
// strings, each bounded by the canonical build-plan codec.
const MAX_GENERATED_FILE_OUTPUT_IDENTITY_BYTES: usize = 3 * MAX_PLAN_STRING_BYTES + 25;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ActionCacheReport {
    pub action: ActionKey,
    pub outcome: ActionCacheOutcome,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ActionCacheOutcome {
    Hit,
    Miss(ActionCacheMissReason),
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ActionCacheMissReason {
    NotFound,
    Invalidated(Vec<ActionCacheInvalidation>),
    Uncacheable,
    Corrupt,
    ReadError,
    WriteError,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum ActionCacheInvalidation {
    Command,
    ExternalTool,
    Environment,
    Inputs,
    Dependencies,
    WorkingDirectory,
    PackageRoots,
    Contents,
    Artifact,
    Sources,
    Module,
    Target,
    Optimization,
    Runtime,
    Linker,
    Output,
    Compiler,
    ResourceLayout,
    StandardLibrary,
    BuildProtocol,
}

enum BoundedCacheEntry {
    Bytes(Vec<u8>),
    Oversized,
}

/// Reads at most `max_bytes + 1` bytes so growth after the metadata check cannot
/// turn a bounded cache lookup into an unbounded allocation.
fn read_bounded_cache_entry(path: &Path, max_bytes: usize) -> io::Result<BoundedCacheEntry> {
    let mut file = fs::File::open(path)?;
    let metadata_len = file.metadata()?.len();
    let max_bytes_u64 = u64::try_from(max_bytes).unwrap_or(u64::MAX);
    if metadata_len > max_bytes_u64 {
        return Ok(BoundedCacheEntry::Oversized);
    }
    let mut encoded = Vec::with_capacity(usize::try_from(metadata_len).unwrap_or(0));
    let read_limit = max_bytes_u64.saturating_add(1);
    Read::by_ref(&mut file)
        .take(read_limit)
        .read_to_end(&mut encoded)?;
    if encoded.len() > max_bytes {
        Ok(BoundedCacheEntry::Oversized)
    } else {
        Ok(BoundedCacheEntry::Bytes(encoded))
    }
}

fn read_bounded_compiler_cache_entry(path: &Path) -> io::Result<BoundedCacheEntry> {
    read_bounded_cache_entry(path, MAX_COMPILER_CACHE_ENTRY_BYTES)
}

fn validate_compiler_cache_entry_size(encoded: &[u8]) -> io::Result<()> {
    if encoded.len() > MAX_COMPILER_CACHE_ENTRY_BYTES {
        Err(io::Error::new(
            io::ErrorKind::InvalidData,
            format!(
                "compiler action-cache entry exceeds the {} byte limit",
                MAX_COMPILER_CACHE_ENTRY_BYTES
            ),
        ))
    } else {
        Ok(())
    }
}

pub(super) fn package_roots_identity<'a>(
    packages: &[crate::PlanPackage],
    paths: impl IntoIterator<Item = &'a crate::LogicalPath>,
) -> Option<Vec<u8>> {
    let keys = paths
        .into_iter()
        .filter_map(|path| match path.root() {
            crate::LogicalPathRoot::Package(package) => Some(package),
            _ => None,
        })
        .collect::<BTreeSet<_>>();
    let mut encoded = Vec::new();
    encoded.extend_from_slice(&(keys.len() as u64).to_le_bytes());
    for key in keys {
        let package = packages.iter().find(|package| &package.key == key)?;
        write_text(&mut encoded, key.as_str());
        write_text(&mut encoded, &package.root);
    }
    Some(encoded)
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct GeneratedFileFingerprintComponents {
    contents: QueryFingerprint,
    output: QueryFingerprint,
    compiler: QueryFingerprint,
    resource_layout: QueryFingerprint,
    standard_library: QueryFingerprint,
    build_protocol: QueryFingerprint,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct GeneratedFileToolchainComponents {
    compiler: QueryFingerprint,
    resource_layout: QueryFingerprint,
    standard_library: QueryFingerprint,
    build_protocol: QueryFingerprint,
}

impl GeneratedFileToolchainComponents {
    fn new(identity: &ToolchainIdentity) -> Self {
        Self {
            compiler: text_component(GENERATED_FILE_COMPILER_DOMAIN, identity.compiler_version()),
            resource_layout: integer_component(
                GENERATED_FILE_RESOURCE_LAYOUT_DOMAIN,
                identity.resource_layout_schema(),
            ),
            standard_library: integer_component(
                GENERATED_FILE_STANDARD_LIBRARY_DOMAIN,
                identity.std_schema(),
            ),
            build_protocol: integer_component(
                GENERATED_FILE_BUILD_PROTOCOL_DOMAIN,
                identity.build_protocol_schema(),
            ),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct GeneratedFileFingerprintSet {
    cache_key: QueryFingerprint,
    fingerprint: QueryFingerprint,
    components: GeneratedFileFingerprintComponents,
}

impl GeneratedFileFingerprintSet {
    fn new(
        action: &ActionKey,
        output: &LogicalPath,
        contents: &[u8],
        toolchain: GeneratedFileToolchainComponents,
    ) -> Self {
        let cache_key = action_key_fingerprint(action);
        let components = GeneratedFileFingerprintComponents {
            contents: contents_fingerprint(contents),
            output: output_fingerprint(&logical_path_identity(output)),
            compiler: toolchain.compiler,
            resource_layout: toolchain.resource_layout,
            standard_library: toolchain.standard_library,
            build_protocol: toolchain.build_protocol,
        };
        let mut fingerprint = QueryFingerprintBuilder::new(GENERATED_FILE_FINGERPRINT_DOMAIN);
        fingerprint.write_fingerprint(cache_key);
        fingerprint.write_fingerprint(components.contents);
        fingerprint.write_fingerprint(components.output);
        fingerprint.write_fingerprint(components.compiler);
        fingerprint.write_fingerprint(components.resource_layout);
        fingerprint.write_fingerprint(components.standard_library);
        fingerprint.write_fingerprint(components.build_protocol);
        Self {
            cache_key,
            fingerprint: fingerprint.finish(),
            components,
        }
    }
}

#[derive(Debug, Clone)]
pub(crate) struct GeneratedFileCacheIdentity {
    fingerprints: GeneratedFileFingerprintSet,
    action: Vec<u8>,
    output: Vec<u8>,
    payload_len: usize,
}

impl GeneratedFileCacheIdentity {
    pub(crate) fn new(
        action: &ActionKey,
        output: &LogicalPath,
        contents: &[u8],
        toolchain: &ToolchainIdentity,
    ) -> Self {
        Self::with_toolchain_components(
            action,
            output,
            contents,
            GeneratedFileToolchainComponents::new(toolchain),
        )
    }

    fn with_toolchain_components(
        action: &ActionKey,
        output: &LogicalPath,
        contents: &[u8],
        toolchain: GeneratedFileToolchainComponents,
    ) -> Self {
        Self {
            fingerprints: GeneratedFileFingerprintSet::new(action, output, contents, toolchain),
            action: action_identity(action),
            output: logical_path_identity(output),
            payload_len: contents.len(),
        }
    }

    /// A generated-file identity fingerprints the exact bytes later published,
    /// so its valid record has one precise size rather than a global payload cap.
    fn encoded_len(&self) -> io::Result<usize> {
        generated_file_entry_overhead(self)
            .checked_add(self.payload_len)
            .ok_or_else(|| io::Error::new(io::ErrorKind::InvalidInput, "cache entry is too large"))
    }

    fn validate_payload(&self, payload: &[u8]) -> io::Result<()> {
        if payload.len() != self.payload_len {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                "generated-file cache payload length does not match its identity",
            ));
        }
        if contents_fingerprint(payload) != self.fingerprints.components.contents {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                "generated-file cache payload contents do not match its identity",
            ));
        }
        Ok(())
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum GeneratedFileCacheLookup {
    Hit(Vec<u8>),
    Miss(ActionCacheMissReason),
}

#[derive(Debug)]
pub(crate) struct GeneratedFileCache {
    root: PathBuf,
}

impl GeneratedFileCache {
    pub(crate) fn new(root: PathBuf) -> Self {
        Self { root }
    }

    pub(crate) fn lookup(
        &self,
        identity: &GeneratedFileCacheIdentity,
    ) -> io::Result<GeneratedFileCacheLookup> {
        let path = self.path(identity.fingerprints);
        let max_bytes = identity.encoded_len()?;
        let encoded = match read_bounded_cache_entry(&path, max_bytes) {
            Ok(BoundedCacheEntry::Bytes(encoded)) => encoded,
            Ok(BoundedCacheEntry::Oversized) => {
                self.retire_bounded_corrupt(&path, &BoundedCacheEntry::Oversized, max_bytes)?;
                return Ok(GeneratedFileCacheLookup::Miss(
                    ActionCacheMissReason::Corrupt,
                ));
            }
            Err(error) if error.kind() == io::ErrorKind::NotFound => {
                return self.lookup_invalidation(identity);
            }
            Err(error) => return Err(error),
        };
        let Some(entry) = decode_entry(&encoded) else {
            self.retire_bounded_corrupt(&path, &BoundedCacheEntry::Bytes(encoded), max_bytes)?;
            return Ok(GeneratedFileCacheLookup::Miss(
                ActionCacheMissReason::Corrupt,
            ));
        };
        if !entry_matches(&entry, identity) || path != self.path(entry.fingerprints) {
            self.retire_bounded_corrupt(&path, &BoundedCacheEntry::Bytes(encoded), max_bytes)?;
            return Ok(GeneratedFileCacheLookup::Miss(
                ActionCacheMissReason::Corrupt,
            ));
        }
        Ok(GeneratedFileCacheLookup::Hit(entry.payload))
    }

    pub(crate) fn publish(
        &self,
        identity: &GeneratedFileCacheIdentity,
        payload: &[u8],
    ) -> io::Result<()> {
        identity.validate_payload(payload)?;
        if matches!(self.lookup(identity)?, GeneratedFileCacheLookup::Hit(_)) {
            return Ok(());
        }
        let path = self.path(identity.fingerprints);
        let parent = path
            .parent()
            .ok_or_else(|| io::Error::other("invalid generated-file cache path"))?;
        fs::create_dir_all(parent)?;
        let staged = parent.join(format!(
            ".nia-generated-cache-{}-{}.tmp",
            std::process::id(),
            CACHE_STAGE_SEQUENCE.fetch_add(1, Ordering::Relaxed)
        ));
        let encoded = encode_entry(identity, payload);
        let result = (|| {
            let mut file = fs::OpenOptions::new()
                .write(true)
                .create_new(true)
                .open(&staged)?;
            file.write_all(&encoded)?;
            file.sync_all()?;
            drop(file);
            self.install_immutable_entry(&staged, &path, identity)?;
            fs::File::open(parent)?.sync_all()
        })();
        if result.is_err() || staged.exists() {
            let _ = fs::remove_file(&staged);
        }
        result
    }

    fn lookup_invalidation(
        &self,
        expected: &GeneratedFileCacheIdentity,
    ) -> io::Result<GeneratedFileCacheLookup> {
        let directory = self.key_dir(expected.fingerprints.cache_key);
        let entries = match fs::read_dir(&directory) {
            Ok(entries) => entries,
            Err(error) if error.kind() == io::ErrorKind::NotFound => {
                return Ok(GeneratedFileCacheLookup::Miss(
                    ActionCacheMissReason::NotFound,
                ));
            }
            Err(error) => return Err(error),
        };
        let mut nearest = None::<(usize, QueryFingerprint, Vec<ActionCacheInvalidation>)>;
        let mut corrupt = false;
        for entry in entries {
            let path = entry?.path();
            if path.extension().and_then(|value| value.to_str()) != Some("entry") {
                continue;
            }
            let entry = match scan_generated_file_entry(&path, expected) {
                Ok(Some(entry)) => entry,
                Ok(None) => {
                    self.retire_scanned_corrupt(&path, expected)?;
                    corrupt = true;
                    continue;
                }
                Err(error) if error.kind() == io::ErrorKind::NotFound => continue,
                Err(error) => return Err(error),
            };
            if !self.scanned_entry_is_valid_at(&path, &entry, expected) {
                self.retire_scanned_corrupt(&path, expected)?;
                corrupt = true;
                continue;
            }
            if let Some(payload) = entry.payload {
                return Ok(GeneratedFileCacheLookup::Hit(payload));
            }
            let reasons = invalidations(
                entry.fingerprints.components,
                expected.fingerprints.components,
            );
            let candidate = (reasons.len(), entry.fingerprints.fingerprint, reasons);
            if nearest
                .as_ref()
                .is_none_or(|current| (candidate.0, candidate.1) < (current.0, current.1))
            {
                nearest = Some(candidate);
            }
        }
        if let Some((_, _, reasons)) = nearest {
            Ok(GeneratedFileCacheLookup::Miss(
                ActionCacheMissReason::Invalidated(reasons),
            ))
        } else if corrupt {
            Ok(GeneratedFileCacheLookup::Miss(
                ActionCacheMissReason::Corrupt,
            ))
        } else {
            Ok(GeneratedFileCacheLookup::Miss(
                ActionCacheMissReason::NotFound,
            ))
        }
    }

    fn scanned_entry_is_valid_at(
        &self,
        path: &Path,
        entry: &ScannedEntry,
        expected: &GeneratedFileCacheIdentity,
    ) -> bool {
        entry.fingerprints.cache_key == expected.fingerprints.cache_key
            && path == self.path(entry.fingerprints)
            // Equal fingerprints require exact action/output identity and the
            // expected payload length; the scanner retains payload only then.
            && (entry.fingerprints != expected.fingerprints || entry.payload.is_some())
    }

    fn key_dir(&self, cache_key: QueryFingerprint) -> PathBuf {
        self.root
            .join("actions")
            .join("generated-files")
            .join(GENERATED_FILE_CACHE.path_component)
            .join(fingerprint_text(cache_key))
    }

    fn path(&self, fingerprints: GeneratedFileFingerprintSet) -> PathBuf {
        self.key_dir(fingerprints.cache_key).join(format!(
            "{}.entry",
            fingerprint_text(fingerprints.fingerprint)
        ))
    }

    fn install_immutable_entry(
        &self,
        staged: &Path,
        path: &Path,
        identity: &GeneratedFileCacheIdentity,
    ) -> io::Result<()> {
        let _lock = self.acquire_mutation_lock(path)?;
        let max_bytes = identity.encoded_len()?;
        for _ in 0..4 {
            match fs::hard_link(staged, path) {
                Ok(()) => return Ok(()),
                Err(error) if error.kind() == io::ErrorKind::AlreadyExists => {
                    match read_bounded_cache_entry(path, max_bytes) {
                        Ok(BoundedCacheEntry::Bytes(encoded))
                            if decode_entry(&encoded)
                                .is_some_and(|entry| entry_matches(&entry, identity)) =>
                        {
                            return Ok(());
                        }
                        Ok(BoundedCacheEntry::Bytes(_) | BoundedCacheEntry::Oversized) => {}
                        Err(error) if error.kind() == io::ErrorKind::NotFound => continue,
                        Err(error) => return Err(error),
                    }
                    match fs::remove_file(path) {
                        Ok(()) => {}
                        Err(error) if error.kind() == io::ErrorKind::NotFound => {}
                        Err(error) => return Err(error),
                    }
                }
                Err(error) => return Err(error),
            }
        }
        Err(io::Error::new(
            io::ErrorKind::AlreadyExists,
            "generated-file cache entry changed during publication",
        ))
    }

    fn retire_bounded_corrupt(
        &self,
        path: &Path,
        observed: &BoundedCacheEntry,
        max_bytes: usize,
    ) -> io::Result<()> {
        let _lock = self.acquire_mutation_lock(path)?;
        let unchanged = match (read_bounded_cache_entry(path, max_bytes), observed) {
            (Ok(BoundedCacheEntry::Bytes(current)), BoundedCacheEntry::Bytes(observed)) => {
                current == *observed
            }
            (Ok(BoundedCacheEntry::Oversized), BoundedCacheEntry::Oversized) => true,
            (Ok(BoundedCacheEntry::Bytes(_) | BoundedCacheEntry::Oversized), _) => false,
            (Err(error), _) if error.kind() == io::ErrorKind::NotFound => return Ok(()),
            (Err(error), _) => return Err(error),
        };
        if unchanged {
            match fs::remove_file(path) {
                Ok(()) => Ok(()),
                Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(()),
                Err(error) => Err(error),
            }
        } else {
            Ok(())
        }
    }

    fn retire_scanned_corrupt(
        &self,
        path: &Path,
        expected: &GeneratedFileCacheIdentity,
    ) -> io::Result<()> {
        let _lock = self.acquire_mutation_lock(path)?;
        let current_is_valid = match scan_generated_file_entry(path, expected) {
            Ok(Some(entry)) => self.scanned_entry_is_valid_at(path, &entry, expected),
            Ok(None) => false,
            Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(()),
            Err(error) => return Err(error),
        };
        if current_is_valid {
            Ok(())
        } else {
            match fs::remove_file(path) {
                Ok(()) => Ok(()),
                Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(()),
                Err(error) => Err(error),
            }
        }
    }

    fn acquire_mutation_lock(&self, path: &Path) -> io::Result<ScopedFileLock> {
        let mut builder = QueryFingerprintBuilder::new(ACTION_CACHE_MUTATION_LOCK_DOMAIN);
        builder.write_bytes(path.as_os_str().as_encoded_bytes());
        let lock = self
            .root
            .join("coordination")
            .join("action-cache-mutations")
            .join(GENERATED_FILE_CACHE.path_component)
            .join(format!("{}.lock", fingerprint_text(builder.finish())));
        ScopedFileLock::acquire_interruptible(lock, || false)?
            .ok_or_else(|| io::Error::other("action-cache mutation lock was cancelled"))
    }
}

fn invalidations(
    found: GeneratedFileFingerprintComponents,
    expected: GeneratedFileFingerprintComponents,
) -> Vec<ActionCacheInvalidation> {
    let mut reasons = Vec::new();
    if found.contents != expected.contents {
        reasons.push(ActionCacheInvalidation::Contents);
    }
    if found.output != expected.output {
        reasons.push(ActionCacheInvalidation::Output);
    }
    if found.compiler != expected.compiler {
        reasons.push(ActionCacheInvalidation::Compiler);
    }
    if found.resource_layout != expected.resource_layout {
        reasons.push(ActionCacheInvalidation::ResourceLayout);
    }
    if found.standard_library != expected.standard_library {
        reasons.push(ActionCacheInvalidation::StandardLibrary);
    }
    if found.build_protocol != expected.build_protocol {
        reasons.push(ActionCacheInvalidation::BuildProtocol);
    }
    reasons
}

fn entry_matches(entry: &DecodedEntry, identity: &GeneratedFileCacheIdentity) -> bool {
    entry.fingerprints == identity.fingerprints
        && entry.action == identity.action
        && entry.output == identity.output
        && entry.payload.len() == identity.payload_len
}

fn generated_file_entry_overhead(identity: &GeneratedFileCacheIdentity) -> usize {
    // Eight identity fingerprints, two length-prefixed identities, the payload
    // length, and the payload checksum precede the payload bytes.
    GENERATED_FILE_ENTRY.magic.len()
        + 8 * 16
        + 8
        + identity.action.len()
        + 8
        + identity.output.len()
        + 8
        + 16
}

fn encode_entry(identity: &GeneratedFileCacheIdentity, payload: &[u8]) -> Vec<u8> {
    let checksum = payload_checksum(payload);
    let fingerprints = identity.fingerprints;
    let mut encoded =
        Vec::with_capacity(generated_file_entry_overhead(identity).saturating_add(payload.len()));
    encoded.extend_from_slice(GENERATED_FILE_ENTRY.magic);
    for fingerprint in [
        fingerprints.cache_key,
        fingerprints.fingerprint,
        fingerprints.components.contents,
        fingerprints.components.output,
        fingerprints.components.compiler,
        fingerprints.components.resource_layout,
        fingerprints.components.standard_library,
        fingerprints.components.build_protocol,
    ] {
        write_fingerprint(&mut encoded, fingerprint);
    }
    write_bytes(&mut encoded, &identity.action);
    write_bytes(&mut encoded, &identity.output);
    encoded.extend_from_slice(&(payload.len() as u64).to_le_bytes());
    write_fingerprint(&mut encoded, checksum);
    encoded.extend_from_slice(payload);
    debug_assert_eq!(
        encoded.len(),
        generated_file_entry_overhead(identity) + payload.len()
    );
    encoded
}

struct DecodedEntry {
    fingerprints: GeneratedFileFingerprintSet,
    action: Vec<u8>,
    output: Vec<u8>,
    payload: Vec<u8>,
}

struct ScannedEntry {
    fingerprints: GeneratedFileFingerprintSet,
    payload: Option<Vec<u8>>,
}

/// Validates an invalidation candidate without materializing its payload.
/// Payload bytes are retained only for an exact identity that appeared after
/// the direct lookup missed; all other candidates are checksummed as a stream.
fn scan_generated_file_entry(
    path: &Path,
    expected: &GeneratedFileCacheIdentity,
) -> io::Result<Option<ScannedEntry>> {
    let mut file = fs::File::open(path)?;
    let metadata_len = file.metadata()?.len();
    let mut magic = [0; 8];
    if !read_exact_or_corrupt(&mut file, &mut magic)? || magic != *GENERATED_FILE_ENTRY.magic {
        return Ok(None);
    }
    let Some(cache_key) = read_stream_fingerprint(&mut file)? else {
        return Ok(None);
    };
    let Some(fingerprint) = read_stream_fingerprint(&mut file)? else {
        return Ok(None);
    };
    let Some(contents) = read_stream_fingerprint(&mut file)? else {
        return Ok(None);
    };
    let Some(output_fingerprint) = read_stream_fingerprint(&mut file)? else {
        return Ok(None);
    };
    let Some(compiler) = read_stream_fingerprint(&mut file)? else {
        return Ok(None);
    };
    let Some(resource_layout) = read_stream_fingerprint(&mut file)? else {
        return Ok(None);
    };
    let Some(standard_library) = read_stream_fingerprint(&mut file)? else {
        return Ok(None);
    };
    let Some(build_protocol) = read_stream_fingerprint(&mut file)? else {
        return Ok(None);
    };
    let components = GeneratedFileFingerprintComponents {
        contents,
        output: output_fingerprint,
        compiler,
        resource_layout,
        standard_library,
        build_protocol,
    };
    let fingerprints = GeneratedFileFingerprintSet {
        cache_key,
        fingerprint,
        components,
    };
    let mut rebuilt = QueryFingerprintBuilder::new(GENERATED_FILE_FINGERPRINT_DOMAIN);
    rebuilt.write_fingerprint(cache_key);
    rebuilt.write_fingerprint(contents);
    rebuilt.write_fingerprint(output_fingerprint);
    rebuilt.write_fingerprint(compiler);
    rebuilt.write_fingerprint(resource_layout);
    rebuilt.write_fingerprint(standard_library);
    rebuilt.write_fingerprint(build_protocol);
    if rebuilt.finish() != fingerprint {
        return Ok(None);
    }

    let mut consumed = u64::try_from(GENERATED_FILE_ENTRY.magic.len() + 8 * 16).unwrap();
    let Some(action_len) = read_stream_u64(&mut file)? else {
        return Ok(None);
    };
    consumed = match consumed.checked_add(8) {
        Some(consumed) => consumed,
        None => return Ok(None),
    };
    if action_len != u64::try_from(expected.action.len()).unwrap_or(u64::MAX)
        || !encoded_field_fits(&mut consumed, action_len, metadata_len)
    {
        return Ok(None);
    }
    let mut action = vec![0; expected.action.len()];
    if !read_exact_or_corrupt(&mut file, &mut action)?
        || action != expected.action
        || action_fingerprint(&action) != Some(cache_key)
    {
        return Ok(None);
    }

    let Some(output_len) = read_stream_u64(&mut file)? else {
        return Ok(None);
    };
    consumed = match consumed.checked_add(8) {
        Some(consumed) => consumed,
        None => return Ok(None),
    };
    if output_len > u64::try_from(MAX_GENERATED_FILE_OUTPUT_IDENTITY_BYTES).unwrap_or(u64::MAX)
        || !encoded_field_fits(&mut consumed, output_len, metadata_len)
    {
        return Ok(None);
    }
    let mut output_builder = QueryFingerprintBuilder::new(GENERATED_FILE_OUTPUT_DOMAIN);
    let mut output_writer = output_builder.bytes_writer(output_len);
    let mut output_writers = [&mut output_writer];
    let Some(output_matches) = stream_bytes(
        &mut file,
        output_len,
        &mut output_writers,
        Some(&expected.output),
        None,
    )?
    else {
        return Ok(None);
    };
    output_writer.finish()?;
    if output_builder.finish() != output_fingerprint {
        return Ok(None);
    }

    let Some(payload_len) = read_stream_u64(&mut file)? else {
        return Ok(None);
    };
    let Some(payload_checksum) = read_stream_fingerprint(&mut file)? else {
        return Ok(None);
    };
    consumed = match consumed.checked_add(8 + 16) {
        Some(consumed) => consumed,
        None => return Ok(None),
    };
    if consumed.checked_add(payload_len) != Some(metadata_len) {
        return Ok(None);
    }

    let exact = fingerprints == expected.fingerprints
        && output_matches
        && payload_len == u64::try_from(expected.payload_len).unwrap_or(u64::MAX);
    let mut payload = exact.then(|| Vec::with_capacity(expected.payload_len));
    let mut checksum_builder = QueryFingerprintBuilder::new(GENERATED_FILE_PAYLOAD_DOMAIN);
    let mut contents_builder = QueryFingerprintBuilder::new(GENERATED_FILE_CONTENTS_DOMAIN);
    let mut checksum_writer = checksum_builder.bytes_writer(payload_len);
    let mut contents_writer = contents_builder.bytes_writer(payload_len);
    let mut payload_writers = [&mut checksum_writer, &mut contents_writer];
    if stream_bytes(
        &mut file,
        payload_len,
        &mut payload_writers,
        None,
        payload.as_mut(),
    )?
    .is_none()
    {
        return Ok(None);
    }
    checksum_writer.finish()?;
    contents_writer.finish()?;
    if checksum_builder.finish() != payload_checksum
        || contents_builder.finish() != contents
        || stream_has_trailing_byte(&mut file)?
    {
        return Ok(None);
    }
    Ok(Some(ScannedEntry {
        fingerprints,
        payload,
    }))
}

fn encoded_field_fits(consumed: &mut u64, length: u64, encoded_len: u64) -> bool {
    let Some(end) = consumed.checked_add(length) else {
        return false;
    };
    if end > encoded_len {
        return false;
    }
    *consumed = end;
    true
}

fn read_exact_or_corrupt(reader: &mut impl Read, bytes: &mut [u8]) -> io::Result<bool> {
    match reader.read_exact(bytes) {
        Ok(()) => Ok(true),
        Err(error) if error.kind() == io::ErrorKind::UnexpectedEof => Ok(false),
        Err(error) => Err(error),
    }
}

fn read_stream_u64(reader: &mut impl Read) -> io::Result<Option<u64>> {
    let mut bytes = [0; 8];
    read_exact_or_corrupt(reader, &mut bytes)
        .map(|complete| complete.then(|| u64::from_le_bytes(bytes)))
}

fn read_stream_fingerprint(reader: &mut impl Read) -> io::Result<Option<QueryFingerprint>> {
    let Some(first) = read_stream_u64(reader)? else {
        return Ok(None);
    };
    let Some(second) = read_stream_u64(reader)? else {
        return Ok(None);
    };
    Ok(Some(QueryFingerprint::from_parts([first, second])))
}

fn stream_bytes(
    reader: &mut impl Read,
    length: u64,
    writers: &mut [&mut QueryFingerprintBytesWriter<'_>],
    expected: Option<&[u8]>,
    mut retained: Option<&mut Vec<u8>>,
) -> io::Result<Option<bool>> {
    let mut buffer = [0; GENERATED_FILE_STREAM_BUFFER_BYTES];
    let mut remaining = length;
    let mut offset = 0usize;
    let mut matches =
        expected.is_none_or(|expected| u64::try_from(expected.len()).unwrap_or(u64::MAX) == length);
    while remaining != 0 {
        let chunk_len = usize::try_from(remaining.min(buffer.len() as u64)).unwrap();
        let chunk = &mut buffer[..chunk_len];
        if !read_exact_or_corrupt(reader, chunk)? {
            return Ok(None);
        }
        for writer in writers.iter_mut() {
            writer.write_chunk(chunk)?;
        }
        if let Some(expected) = expected
            && matches
        {
            matches = expected
                .get(offset..offset + chunk_len)
                .is_some_and(|expected| expected == chunk);
        }
        if let Some(retained) = retained.as_deref_mut() {
            retained.extend_from_slice(chunk);
        }
        remaining -= chunk_len as u64;
        offset = offset.saturating_add(chunk_len);
    }
    Ok(Some(matches))
}

fn stream_has_trailing_byte(reader: &mut impl Read) -> io::Result<bool> {
    let mut byte = [0; 1];
    loop {
        match reader.read(&mut byte) {
            Ok(0) => return Ok(false),
            Ok(_) => return Ok(true),
            Err(error) if error.kind() == io::ErrorKind::Interrupted => {}
            Err(error) => return Err(error),
        }
    }
}

fn decode_entry(encoded: &[u8]) -> Option<DecodedEntry> {
    let mut cursor = Cursor::new(encoded);
    let mut magic = [0; 8];
    cursor.read_exact(&mut magic).ok()?;
    (magic == *GENERATED_FILE_ENTRY.magic).then_some(())?;
    let cache_key = read_fingerprint(&mut cursor)?;
    let fingerprint = read_fingerprint(&mut cursor)?;
    let components = GeneratedFileFingerprintComponents {
        contents: read_fingerprint(&mut cursor)?,
        output: read_fingerprint(&mut cursor)?,
        compiler: read_fingerprint(&mut cursor)?,
        resource_layout: read_fingerprint(&mut cursor)?,
        standard_library: read_fingerprint(&mut cursor)?,
        build_protocol: read_fingerprint(&mut cursor)?,
    };
    let fingerprints = GeneratedFileFingerprintSet {
        cache_key,
        fingerprint,
        components,
    };
    let mut expected_fingerprint = QueryFingerprintBuilder::new(GENERATED_FILE_FINGERPRINT_DOMAIN);
    expected_fingerprint.write_fingerprint(cache_key);
    expected_fingerprint.write_fingerprint(components.contents);
    expected_fingerprint.write_fingerprint(components.output);
    expected_fingerprint.write_fingerprint(components.compiler);
    expected_fingerprint.write_fingerprint(components.resource_layout);
    expected_fingerprint.write_fingerprint(components.standard_library);
    expected_fingerprint.write_fingerprint(components.build_protocol);
    (expected_fingerprint.finish() == fingerprint).then_some(())?;
    let action = read_bytes(&mut cursor, encoded.len())?;
    (action_fingerprint(&action)? == cache_key).then_some(())?;
    let output = read_bytes(&mut cursor, encoded.len())?;
    (output_fingerprint(&output) == components.output).then_some(())?;
    let payload_len = usize::try_from(read_u64(&mut cursor)?).ok()?;
    let checksum = read_fingerprint(&mut cursor)?;
    let position = usize::try_from(cursor.position()).ok()?;
    (encoded.len().checked_sub(position)? == payload_len).then_some(())?;
    let payload = encoded[position..].to_vec();
    (payload_checksum(&payload) == checksum).then_some(())?;
    (contents_fingerprint(&payload) == components.contents).then_some(DecodedEntry {
        fingerprints,
        action,
        output,
        payload,
    })
}

fn action_identity(action: &ActionKey) -> Vec<u8> {
    let mut encoded = Vec::new();
    write_text(&mut encoded, action.package().as_str());
    write_text(&mut encoded, action.name());
    encoded
}

fn action_key_fingerprint(action: &ActionKey) -> QueryFingerprint {
    let mut builder = QueryFingerprintBuilder::new(GENERATED_FILE_KEY_DOMAIN);
    builder.write_str(action.package().as_str());
    builder.write_str(action.name());
    builder.finish()
}

fn action_fingerprint(identity: &[u8]) -> Option<QueryFingerprint> {
    let mut cursor = Cursor::new(identity);
    let package = read_bytes(&mut cursor, identity.len())?;
    let name = read_bytes(&mut cursor, identity.len())?;
    (usize::try_from(cursor.position()).ok()? == identity.len()).then_some(())?;
    let mut builder = QueryFingerprintBuilder::new(GENERATED_FILE_KEY_DOMAIN);
    builder.write_bytes(&package);
    builder.write_bytes(&name);
    Some(builder.finish())
}

fn contents_fingerprint(contents: &[u8]) -> QueryFingerprint {
    let mut builder = QueryFingerprintBuilder::new(GENERATED_FILE_CONTENTS_DOMAIN);
    builder.write_bytes(contents);
    builder.finish()
}

fn output_fingerprint(identity: &[u8]) -> QueryFingerprint {
    let mut builder = QueryFingerprintBuilder::new(GENERATED_FILE_OUTPUT_DOMAIN);
    builder.write_bytes(identity);
    builder.finish()
}

fn text_component(domain: FingerprintDomain, value: &str) -> QueryFingerprint {
    let mut builder = QueryFingerprintBuilder::new(domain);
    builder.write_str(value);
    builder.finish()
}

fn integer_component(domain: FingerprintDomain, value: u32) -> QueryFingerprint {
    let mut builder = QueryFingerprintBuilder::new(domain);
    builder.write_u64(u64::from(value));
    builder.finish()
}

fn logical_path_identity(path: &LogicalPath) -> Vec<u8> {
    let mut encoded = Vec::new();
    match path.root() {
        LogicalPathRoot::Package(package) => {
            encoded.push(0);
            write_text(&mut encoded, package.as_str());
        }
        LogicalPathRoot::Build => encoded.push(1),
        LogicalPathRoot::Cache => encoded.push(2),
        LogicalPathRoot::Toolchain => encoded.push(3),
        LogicalPathRoot::Artifact(artifact) => {
            encoded.push(4);
            encode_artifact(&mut encoded, artifact);
        }
    }
    write_text(&mut encoded, &path.protocol_path());
    encoded
}

fn encode_artifact(encoded: &mut Vec<u8>, artifact: &ArtifactKey) {
    write_text(encoded, artifact.package().as_str());
    write_text(encoded, artifact.name());
}

fn write_text(encoded: &mut Vec<u8>, text: &str) {
    write_bytes(encoded, text.as_bytes());
}

fn write_bytes(encoded: &mut Vec<u8>, bytes: &[u8]) {
    encoded.extend_from_slice(&(bytes.len() as u64).to_le_bytes());
    encoded.extend_from_slice(bytes);
}

fn read_bytes(cursor: &mut Cursor<&[u8]>, encoded_len: usize) -> Option<Vec<u8>> {
    let length = usize::try_from(read_u64(cursor)?).ok()?;
    let position = usize::try_from(cursor.position()).ok()?;
    (length <= encoded_len.checked_sub(position)?).then_some(())?;
    let mut bytes = vec![0; length];
    cursor.read_exact(&mut bytes).ok()?;
    Some(bytes)
}

fn write_fingerprint(encoded: &mut Vec<u8>, fingerprint: QueryFingerprint) {
    for part in fingerprint.parts() {
        encoded.extend_from_slice(&part.to_le_bytes());
    }
}

fn read_fingerprint(cursor: &mut Cursor<&[u8]>) -> Option<QueryFingerprint> {
    Some(QueryFingerprint::from_parts([
        read_u64(cursor)?,
        read_u64(cursor)?,
    ]))
}

fn read_u64(cursor: &mut Cursor<&[u8]>) -> Option<u64> {
    let mut bytes = [0; 8];
    cursor.read_exact(&mut bytes).ok()?;
    Some(u64::from_le_bytes(bytes))
}

fn payload_checksum(bytes: &[u8]) -> QueryFingerprint {
    let mut builder = QueryFingerprintBuilder::new(GENERATED_FILE_PAYLOAD_DOMAIN);
    builder.write_bytes(bytes);
    builder.finish()
}

fn fingerprint_text(fingerprint: QueryFingerprint) -> String {
    let [first, second] = fingerprint.parts();
    format!("{first:016x}{second:016x}")
}

#[cfg(test)]
#[path = "action_cache/process_tests.rs"]
mod process_tests;

#[cfg(test)]
mod tests {
    use std::sync::{Arc, Barrier};

    use super::*;
    use crate::PackageKey;

    fn test_root(name: &str) -> PathBuf {
        std::env::temp_dir().join(format!(
            "nia-build-action-cache-{name}-{}-{}",
            std::process::id(),
            CACHE_STAGE_SEQUENCE.fetch_add(1, Ordering::Relaxed)
        ))
    }

    fn action() -> ActionKey {
        ActionKey::new(PackageKey::root(), "generate").expect("action key")
    }

    fn output(path: &str) -> LogicalPath {
        LogicalPath::new(LogicalPathRoot::Build, path).expect("logical output")
    }

    fn identity(
        output: &LogicalPath,
        contents: &[u8],
        toolchain: GeneratedFileToolchainComponents,
    ) -> GeneratedFileCacheIdentity {
        GeneratedFileCacheIdentity::with_toolchain_components(
            &action(),
            output,
            contents,
            toolchain,
        )
    }

    fn toolchain() -> GeneratedFileToolchainComponents {
        GeneratedFileToolchainComponents {
            compiler: QueryFingerprint::from_parts([1, 1]),
            resource_layout: QueryFingerprint::from_parts([1, 2]),
            standard_library: QueryFingerprint::from_parts([1, 3]),
            build_protocol: QueryFingerprint::from_parts([1, 4]),
        }
    }

    #[test]
    fn generated_file_entry_round_trips() {
        let root = test_root("round-trip");
        let cache = GeneratedFileCache::new(root.clone());
        let identity = identity(&output("generated/source.nia"), b"source", toolchain());

        assert_eq!(
            cache.lookup(&identity).expect("cold lookup"),
            GeneratedFileCacheLookup::Miss(ActionCacheMissReason::NotFound)
        );
        cache.publish(&identity, b"source").expect("publish");
        assert_eq!(
            cache.lookup(&identity).expect("warm lookup"),
            GeneratedFileCacheLookup::Hit(b"source".to_vec())
        );

        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn generated_file_invalidation_is_component_exact() {
        let root = test_root("invalidation");
        let cache = GeneratedFileCache::new(root.clone());
        let logical_output = output("generated/source.nia");
        let baseline_toolchain = toolchain();
        let baseline = identity(&logical_output, b"source", baseline_toolchain);
        cache.publish(&baseline, b"source").expect("publish");

        let mut changed_compiler = baseline_toolchain;
        changed_compiler.compiler = QueryFingerprint::from_parts([2, 1]);
        let mut changed_resource_layout = baseline_toolchain;
        changed_resource_layout.resource_layout = QueryFingerprint::from_parts([2, 2]);
        let mut changed_standard_library = baseline_toolchain;
        changed_standard_library.standard_library = QueryFingerprint::from_parts([2, 3]);
        let mut changed_build_protocol = baseline_toolchain;
        changed_build_protocol.build_protocol = QueryFingerprint::from_parts([2, 4]);

        for (changed, expected) in [
            (
                identity(&logical_output, b"changed", baseline_toolchain),
                ActionCacheInvalidation::Contents,
            ),
            (
                identity(
                    &output("generated/other.nia"),
                    b"source",
                    baseline_toolchain,
                ),
                ActionCacheInvalidation::Output,
            ),
            (
                identity(&logical_output, b"source", changed_compiler),
                ActionCacheInvalidation::Compiler,
            ),
            (
                identity(&logical_output, b"source", changed_resource_layout),
                ActionCacheInvalidation::ResourceLayout,
            ),
            (
                identity(&logical_output, b"source", changed_standard_library),
                ActionCacheInvalidation::StandardLibrary,
            ),
            (
                identity(&logical_output, b"source", changed_build_protocol),
                ActionCacheInvalidation::BuildProtocol,
            ),
        ] {
            assert_eq!(
                cache.lookup(&changed).expect("invalidation lookup"),
                GeneratedFileCacheLookup::Miss(ActionCacheMissReason::Invalidated(vec![expected]))
            );
        }

        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn invalidation_scan_streams_nonmatching_generated_payloads() {
        let root = test_root("streamed-invalidation");
        let cache = GeneratedFileCache::new(root.clone());
        let logical_output = output("generated/large.bin");
        let payload = vec![0x5a; 4 * 1024 * 1024];
        let baseline = identity(&logical_output, &payload, toolchain());
        cache.publish(&baseline, &payload).expect("publish");
        let changed = identity(&logical_output, b"changed", toolchain());

        let scanned = scan_generated_file_entry(&cache.path(baseline.fingerprints), &changed)
            .expect("scan candidate")
            .expect("valid candidate");
        assert!(
            scanned.payload.is_none(),
            "an invalidation candidate must not retain its payload"
        );
        assert_eq!(
            cache.lookup(&changed).expect("invalidation lookup"),
            GeneratedFileCacheLookup::Miss(ActionCacheMissReason::Invalidated(vec![
                ActionCacheInvalidation::Contents,
            ]))
        );

        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn invalidation_scan_rejects_untrusted_identity_lengths_before_allocating() {
        let root = test_root("invalid-invalidation-length");
        let cache = GeneratedFileCache::new(root.clone());
        let logical_output = output("generated/source.nia");
        let baseline = identity(&logical_output, b"source", toolchain());
        cache.publish(&baseline, b"source").expect("publish");
        let path = cache.path(baseline.fingerprints);
        let mut encoded = fs::read(&path).expect("read entry");
        let action_length_offset = GENERATED_FILE_ENTRY.magic.len() + 8 * 16;
        encoded[action_length_offset..action_length_offset + 8]
            .copy_from_slice(&u64::MAX.to_le_bytes());
        fs::write(&path, encoded).expect("forge action length");
        let changed = identity(&logical_output, b"changed", toolchain());

        assert_eq!(
            cache.lookup(&changed).expect("corrupt invalidation lookup"),
            GeneratedFileCacheLookup::Miss(ActionCacheMissReason::Corrupt)
        );
        assert!(!path.exists(), "malformed candidate must be retired");

        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn invalidation_scan_bounds_persisted_output_identity_bytes() {
        let root = test_root("invalid-output-identity-length");
        let cache = GeneratedFileCache::new(root.clone());
        let logical_output = output("generated/source.nia");
        let baseline = identity(&logical_output, b"source", toolchain());
        cache.publish(&baseline, b"source").expect("publish");
        let path = cache.path(baseline.fingerprints);
        let mut encoded = fs::read(&path).expect("read entry");
        let output_length_offset =
            GENERATED_FILE_ENTRY.magic.len() + 8 * 16 + 8 + baseline.action.len();
        encoded[output_length_offset..output_length_offset + 8].copy_from_slice(
            &u64::try_from(MAX_GENERATED_FILE_OUTPUT_IDENTITY_BYTES + 1)
                .unwrap()
                .to_le_bytes(),
        );
        fs::write(&path, encoded).expect("forge output identity length");
        let changed = identity(&logical_output, b"changed", toolchain());

        assert_eq!(
            cache.lookup(&changed).expect("corrupt invalidation lookup"),
            GeneratedFileCacheLookup::Miss(ActionCacheMissReason::Corrupt)
        );
        assert!(
            !path.exists(),
            "oversized identity candidate must be retired"
        );

        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn corrupt_generated_file_entry_is_retired() {
        let root = test_root("corrupt");
        let cache = GeneratedFileCache::new(root.clone());
        let identity = identity(&output("generated/source.nia"), b"source", toolchain());
        cache.publish(&identity, b"source").expect("publish");
        let path = cache.path(identity.fingerprints);
        let mut encoded = fs::read(&path).expect("read entry");
        *encoded.last_mut().expect("payload") ^= 0xff;
        fs::write(&path, encoded).expect("corrupt entry");

        assert_eq!(
            cache.lookup(&identity).expect("corrupt lookup"),
            GeneratedFileCacheLookup::Miss(ActionCacheMissReason::Corrupt)
        );
        assert!(!path.exists(), "corrupt entry must be retired");
        cache.publish(&identity, b"source").expect("republish");
        assert!(matches!(
            cache.lookup(&identity).expect("repaired lookup"),
            GeneratedFileCacheLookup::Hit(_)
        ));

        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn oversized_exact_generated_file_entry_is_retired_without_full_read() {
        let root = test_root("oversized-exact-entry");
        let cache = GeneratedFileCache::new(root.clone());
        let identity = identity(&output("generated/source.nia"), b"source", toolchain());
        let path = cache.path(identity.fingerprints);
        fs::create_dir_all(path.parent().expect("entry directory"))
            .expect("create entry directory");
        let file = fs::File::create(&path).expect("create sparse entry");
        file.set_len(u64::try_from(identity.encoded_len().expect("encoded length") + 1).unwrap())
            .expect("extend sparse entry");

        assert_eq!(
            cache.lookup(&identity).expect("oversized lookup"),
            GeneratedFileCacheLookup::Miss(ActionCacheMissReason::Corrupt)
        );
        assert!(!path.exists(), "oversized exact entry must be retired");

        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn generated_file_publication_requires_identity_payload() {
        let root = test_root("payload-identity");
        let cache = GeneratedFileCache::new(root.clone());
        let identity = identity(&output("generated/source.nia"), b"source", toolchain());

        let length_error = cache
            .publish(&identity, b"source!")
            .expect_err("mismatched payload length");
        assert_eq!(length_error.kind(), io::ErrorKind::InvalidInput);
        let contents_error = cache
            .publish(&identity, b"sourcf")
            .expect_err("mismatched payload contents");
        assert_eq!(contents_error.kind(), io::ErrorKind::InvalidInput);
        assert!(!cache.path(identity.fingerprints).exists());

        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn stale_corruption_retirement_does_not_remove_republished_entry() {
        let root = test_root("stale-retirement");
        let cache = GeneratedFileCache::new(root.clone());
        let identity = identity(&output("generated/source.nia"), b"source", toolchain());
        cache.publish(&identity, b"source").expect("publish");
        let path = cache.path(identity.fingerprints);
        let mut observed = fs::read(&path).expect("read entry");
        *observed.last_mut().expect("payload") ^= 0xff;
        fs::write(&path, &observed).expect("corrupt entry");
        fs::remove_file(&path).expect("retire old entry");
        cache.publish(&identity, b"source").expect("republish");

        cache
            .retire_bounded_corrupt(
                &path,
                &BoundedCacheEntry::Bytes(observed),
                identity.encoded_len().expect("encoded length"),
            )
            .expect("stale retirement");

        assert_eq!(
            cache.lookup(&identity).expect("lookup"),
            GeneratedFileCacheLookup::Hit(b"source".to_vec())
        );
        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn duplicate_publishers_keep_one_complete_immutable_entry() {
        let root = test_root("duplicate-publishers");
        let identity = identity(&output("generated/source.nia"), b"source", toolchain());
        let barrier = Arc::new(Barrier::new(8));
        let handles = (0..8)
            .map(|_| {
                let root = root.clone();
                let identity = identity.clone();
                let barrier = Arc::clone(&barrier);
                std::thread::spawn(move || {
                    barrier.wait();
                    GeneratedFileCache::new(root)
                        .publish(&identity, b"source")
                        .expect("concurrent publish");
                })
            })
            .collect::<Vec<_>>();
        for handle in handles {
            handle.join().expect("publisher");
        }

        let cache = GeneratedFileCache::new(root.clone());
        assert_eq!(
            cache.lookup(&identity).expect("lookup"),
            GeneratedFileCacheLookup::Hit(b"source".to_vec())
        );
        let entries = fs::read_dir(cache.key_dir(identity.fingerprints.cache_key))
            .expect("entry directory")
            .filter_map(Result::ok)
            .filter(|entry| {
                entry.path().extension().and_then(|value| value.to_str()) == Some("entry")
            })
            .count();
        assert_eq!(entries, 1);

        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn concurrent_reader_never_accepts_partial_publication() {
        let root = test_root("concurrent-reader");
        let payload = vec![0x5a; 4 * 1024 * 1024];
        let identity = identity(&output("generated/large.bin"), &payload, toolchain());
        let barrier = Arc::new(Barrier::new(2));
        let writer_root = root.clone();
        let writer_identity = identity.clone();
        let writer_payload = payload.clone();
        let writer_barrier = Arc::clone(&barrier);
        let writer = std::thread::spawn(move || {
            writer_barrier.wait();
            GeneratedFileCache::new(writer_root)
                .publish(&writer_identity, &writer_payload)
                .expect("publish");
        });

        barrier.wait();
        let cache = GeneratedFileCache::new(root.clone());
        for _ in 0..1_000 {
            match cache.lookup(&identity).expect("concurrent lookup") {
                GeneratedFileCacheLookup::Hit(found) => {
                    assert_eq!(found, payload);
                    break;
                }
                GeneratedFileCacheLookup::Miss(ActionCacheMissReason::NotFound) => {}
                unexpected => panic!("reader observed non-atomic cache state: {unexpected:?}"),
            }
        }
        writer.join().expect("writer");
        assert_eq!(
            cache.lookup(&identity).expect("accepted lookup"),
            GeneratedFileCacheLookup::Hit(payload)
        );

        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn invalidation_scan_recovers_an_entry_published_after_direct_miss() {
        let root = test_root("invalidation-scan-publish-race");
        let cache = GeneratedFileCache::new(root.clone());
        let identity = identity(&output("generated/source.nia"), b"source", toolchain());
        cache.publish(&identity, b"source").expect("publish");

        assert_eq!(
            cache
                .lookup_invalidation(&identity)
                .expect("invalidation scan"),
            GeneratedFileCacheLookup::Hit(b"source".to_vec())
        );

        let _ = fs::remove_dir_all(root);
    }
}
