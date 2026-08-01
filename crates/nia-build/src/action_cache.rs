// SPDX-License-Identifier: GPL-3.0-or-later

use std::{
    fs,
    io::{self, Cursor, Read, Write},
    path::{Path, PathBuf},
    sync::atomic::{AtomicU64, Ordering},
};

use nia_query::{QueryFingerprint, QueryFingerprintBuilder};
use nia_toolchain::ToolchainIdentity;

use crate::{ActionKey, ArtifactKey, LogicalPath, LogicalPathRoot, lock::ScopedFileLock};

const GENERATED_FILE_MAGIC: &[u8; 8] = b"NIAGEN01";
const GENERATED_FILE_SCHEMA: &str = "v1";
static CACHE_STAGE_SEQUENCE: AtomicU64 = AtomicU64::new(0);

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
    Corrupt,
    ReadError,
    WriteError,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum ActionCacheInvalidation {
    Contents,
    Output,
    Compiler,
    ResourceLayout,
    StandardLibrary,
    BuildProtocol,
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
            compiler: text_component(
                "nia.build.generated-file-compiler.v1",
                identity.compiler_version(),
            ),
            resource_layout: integer_component(
                "nia.build.generated-file-resource-layout.v1",
                identity.resource_layout_schema(),
            ),
            standard_library: integer_component(
                "nia.build.generated-file-standard-library.v1",
                identity.std_schema(),
            ),
            build_protocol: integer_component(
                "nia.build.generated-file-build-protocol.v1",
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
        let mut fingerprint =
            QueryFingerprintBuilder::new("nia.build.generated-file-fingerprint.v1");
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
        }
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
        let encoded = match fs::read(&path) {
            Ok(encoded) => encoded,
            Err(error) if error.kind() == io::ErrorKind::NotFound => {
                return self.lookup_invalidation(identity);
            }
            Err(error) => return Err(error),
        };
        let Some(entry) = decode_entry(&encoded) else {
            self.retire_corrupt(&path, &encoded)?;
            return Ok(GeneratedFileCacheLookup::Miss(
                ActionCacheMissReason::Corrupt,
            ));
        };
        if !entry_matches(&entry, identity) || path != self.path(entry.fingerprints) {
            self.retire_corrupt(&path, &encoded)?;
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
            let encoded = fs::read(&path)?;
            let Some(entry) = decode_entry(&encoded) else {
                self.retire_corrupt(&path, &encoded)?;
                corrupt = true;
                continue;
            };
            if entry.fingerprints.cache_key != expected.fingerprints.cache_key
                || path != self.path(entry.fingerprints)
            {
                self.retire_corrupt(&path, &encoded)?;
                corrupt = true;
                continue;
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

    fn key_dir(&self, cache_key: QueryFingerprint) -> PathBuf {
        self.root
            .join("actions")
            .join("generated-files")
            .join(GENERATED_FILE_SCHEMA)
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
        for _ in 0..4 {
            match fs::hard_link(staged, path) {
                Ok(()) => return Ok(()),
                Err(error) if error.kind() == io::ErrorKind::AlreadyExists => {
                    let encoded = fs::read(path)?;
                    if decode_entry(&encoded).is_some_and(|entry| entry_matches(&entry, identity)) {
                        return Ok(());
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

    fn retire_corrupt(&self, path: &Path, observed: &[u8]) -> io::Result<()> {
        let _lock = self.acquire_mutation_lock(path)?;
        match fs::read(path) {
            Ok(current) if current == observed => match fs::remove_file(path) {
                Ok(()) => Ok(()),
                Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(()),
                Err(error) => Err(error),
            },
            Ok(_) => Ok(()),
            Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(()),
            Err(error) => Err(error),
        }
    }

    fn acquire_mutation_lock(&self, path: &Path) -> io::Result<ScopedFileLock> {
        let mut builder = QueryFingerprintBuilder::new("nia.build.action-cache-mutation-lock.v1");
        builder.write_bytes(path.as_os_str().as_encoded_bytes());
        let lock = self
            .root
            .join("coordination")
            .join("action-cache-mutations")
            .join(GENERATED_FILE_SCHEMA)
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
}

fn encode_entry(identity: &GeneratedFileCacheIdentity, payload: &[u8]) -> Vec<u8> {
    let checksum = payload_checksum(payload);
    let fingerprints = identity.fingerprints;
    let mut encoded =
        Vec::with_capacity(120 + identity.action.len() + identity.output.len() + payload.len());
    encoded.extend_from_slice(GENERATED_FILE_MAGIC);
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
    encoded
}

struct DecodedEntry {
    fingerprints: GeneratedFileFingerprintSet,
    action: Vec<u8>,
    output: Vec<u8>,
    payload: Vec<u8>,
}

fn decode_entry(encoded: &[u8]) -> Option<DecodedEntry> {
    let mut cursor = Cursor::new(encoded);
    let mut magic = [0; 8];
    cursor.read_exact(&mut magic).ok()?;
    (magic == *GENERATED_FILE_MAGIC).then_some(())?;
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
    let mut expected_fingerprint =
        QueryFingerprintBuilder::new("nia.build.generated-file-fingerprint.v1");
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
    let mut builder = QueryFingerprintBuilder::new("nia.build.generated-file-key.v1");
    builder.write_str(action.package().as_str());
    builder.write_str(action.name());
    builder.finish()
}

fn action_fingerprint(identity: &[u8]) -> Option<QueryFingerprint> {
    let mut cursor = Cursor::new(identity);
    let package = read_bytes(&mut cursor, identity.len())?;
    let name = read_bytes(&mut cursor, identity.len())?;
    (usize::try_from(cursor.position()).ok()? == identity.len()).then_some(())?;
    let mut builder = QueryFingerprintBuilder::new("nia.build.generated-file-key.v1");
    builder.write_bytes(&package);
    builder.write_bytes(&name);
    Some(builder.finish())
}

fn contents_fingerprint(contents: &[u8]) -> QueryFingerprint {
    let mut builder = QueryFingerprintBuilder::new("nia.build.generated-file-contents.v1");
    builder.write_bytes(contents);
    builder.finish()
}

fn output_fingerprint(identity: &[u8]) -> QueryFingerprint {
    let mut builder = QueryFingerprintBuilder::new("nia.build.generated-file-output.v1");
    builder.write_bytes(identity);
    builder.finish()
}

fn text_component(domain: &str, value: &str) -> QueryFingerprint {
    let mut builder = QueryFingerprintBuilder::new(domain);
    builder.write_str(value);
    builder.finish()
}

fn integer_component(domain: &str, value: u32) -> QueryFingerprint {
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
    let mut builder = QueryFingerprintBuilder::new("nia.build.generated-file-payload.v1");
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
            .retire_corrupt(&path, &observed)
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
}
