// SPDX-License-Identifier: GPL-3.0-or-later
//! Compiler-emit bindings to Driver-owned executable products.
//!
//! Identity extends the compiler-check manifest with artifact, output, archive
//! input, and link-environment components. The record stores a typed Driver
//! cache reference, not executable bytes, and invalid referents fall back to
//! ordinary compilation after retiring only this binding.

use std::{
    collections::BTreeSet,
    fs,
    io::{self, Cursor, Read, Write},
    path::{Path, PathBuf},
    sync::atomic::Ordering,
};

use nia_compat::formats::{COMPILER_EMIT_CACHE, COMPILER_EMIT_ENTRY};
use nia_compiler_query::SourceContentFingerprint;
use nia_driver::{ExecutableCacheEnvironment, ExecutableCacheReference, SourceInputManifest};
use nia_query::{FingerprintDomain, QueryFingerprint, QueryFingerprintBuilder};
use nia_source::SourceIdentity;
use nia_toolchain::ToolchainIdentity;

use super::{
    ActionCacheInvalidation, ActionCacheMissReason, BoundedCacheEntry, CACHE_STAGE_SEQUENCE,
    MAX_COMPILER_CACHE_ENTRY_BYTES,
    compiler_check::{
        COMPILER_CHECK_MODULE_DOMAIN, COMPILER_CHECK_OPTIMIZATION_DOMAIN,
        COMPILER_CHECK_PACKAGE_ROOTS_DOMAIN, COMPILER_CHECK_RUNTIME_DOMAIN,
        COMPILER_CHECK_TARGET_DOMAIN, CompilerCheckCacheIdentity, FingerprintComponents,
        SourceRecord, action_fingerprint, bytes_fingerprint, integer_fingerprint,
        invalidations as compiler_invalidations, read_tag, source_records_fingerprint,
    },
    fingerprint_text, logical_path_identity, read_bounded_compiler_cache_entry, read_bytes,
    read_fingerprint, read_u64, validate_compiler_cache_entry_size, write_bytes, write_fingerprint,
    write_text,
};
use crate::{ActionKey, PlanArtifact, PlanModule, PlanPackage, TargetSpec, lock::ScopedFileLock};

const COMPILER_EMIT_LINK_INPUT_CONTENT_DOMAIN: FingerprintDomain =
    FingerprintDomain::new("nia.build.compiler-emit.link-input-content.v1");
const COMPILER_EMIT_ARTIFACT_DOMAIN: FingerprintDomain =
    FingerprintDomain::new("nia.build.compiler-emit.artifact.v1");
const COMPILER_EMIT_OUTPUT_DOMAIN: FingerprintDomain =
    FingerprintDomain::new("nia.build.compiler-emit.output.v1");
const COMPILER_EMIT_LINK_ENVIRONMENT_DOMAIN: FingerprintDomain =
    FingerprintDomain::new("nia.build.compiler-emit.link-environment.v1");
const COMPILER_EMIT_LINK_INPUTS_DOMAIN: FingerprintDomain =
    FingerprintDomain::new("nia.build.compiler-emit.link-inputs.v1");
const COMPILER_EMIT_FINGERPRINT_DOMAIN: FingerprintDomain =
    FingerprintDomain::new("nia.build.compiler-emit.fingerprint.v1");
const COMPILER_EMIT_REFERENCE_DOMAIN: FingerprintDomain =
    FingerprintDomain::new("nia.build.compiler-emit.reference.v1");

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct EmitFingerprintComponents {
    compiler: FingerprintComponents,
    artifact: QueryFingerprint,
    output: QueryFingerprint,
    link_environment: QueryFingerprint,
    link_inputs: QueryFingerprint,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct EmitFingerprintSet {
    cache_key: QueryFingerprint,
    fingerprint: QueryFingerprint,
    components: EmitFingerprintComponents,
}

#[derive(Debug, Clone)]
pub(crate) struct CompilerEmitCacheIdentity {
    fingerprints: EmitFingerprintSet,
    action: Vec<u8>,
    module: Vec<u8>,
    package_roots: Vec<u8>,
    target: Vec<u8>,
    optimization: u8,
    runtime: u8,
    sources: Vec<SourceRecord>,
    artifact: Vec<u8>,
    output: Vec<u8>,
    link_environment: Vec<u8>,
    link_inputs: Vec<u8>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct CompilerEmitCacheLinkInput {
    artifact: crate::ArtifactKey,
    fingerprint: QueryFingerprint,
    byte_len: usize,
}

impl CompilerEmitCacheLinkInput {
    #[cfg(test)]
    pub(crate) fn from_bytes(artifact: crate::ArtifactKey, bytes: &[u8]) -> Self {
        Self {
            artifact,
            fingerprint: bytes_fingerprint(COMPILER_EMIT_LINK_INPUT_CONTENT_DOMAIN, bytes),
            byte_len: bytes.len(),
        }
    }

    /// Streams the archive identity using the opened handle's observed size.
    /// Keeping the byte length in the registered encoding preserves existing
    /// cache keys while eliminating the archive-sized coordinator buffer.
    pub(crate) fn from_reader(
        artifact: crate::ArtifactKey,
        reader: &mut impl Read,
        length: u64,
    ) -> io::Result<Self> {
        let byte_len = usize::try_from(length).map_err(|_| {
            io::Error::new(
                io::ErrorKind::InvalidData,
                "static archive is too large for this host",
            )
        })?;
        let mut fingerprint = QueryFingerprintBuilder::new(COMPILER_EMIT_LINK_INPUT_CONTENT_DOMAIN);
        let mut writer = fingerprint.bytes_writer(length);
        let mut buffer = [0; 64 * 1024];
        let mut remaining = length;
        while remaining != 0 {
            let chunk_len = usize::try_from(remaining.min(buffer.len() as u64)).unwrap();
            reader.read_exact(&mut buffer[..chunk_len])?;
            writer.write_chunk(&buffer[..chunk_len])?;
            remaining -= chunk_len as u64;
        }
        writer.finish()?;
        let mut trailing = [0; 1];
        if reader.read(&mut trailing)? != 0 {
            return Err(io::Error::new(
                io::ErrorKind::InvalidData,
                "static archive grew while it was fingerprinted",
            ));
        }
        Ok(Self {
            artifact,
            fingerprint: fingerprint.finish(),
            byte_len,
        })
    }
}

pub(crate) struct CompilerEmitCacheIdentityInput<'a> {
    pub(crate) action: &'a ActionKey,
    pub(crate) artifact: &'a PlanArtifact,
    pub(crate) module: &'a PlanModule,
    pub(crate) packages: &'a [PlanPackage],
    pub(crate) target: &'a TargetSpec,
    pub(crate) manifest: &'a SourceInputManifest,
    pub(crate) toolchain: &'a ToolchainIdentity,
    pub(crate) link_environment: ExecutableCacheEnvironment,
    pub(crate) link_inputs: &'a [CompilerEmitCacheLinkInput],
}

impl CompilerEmitCacheIdentity {
    pub(crate) fn new(input: CompilerEmitCacheIdentityInput<'_>) -> Option<Self> {
        let compiler = CompilerCheckCacheIdentity::new(
            input.action,
            input.module,
            input.packages,
            input.target,
            crate::Runtime::Freestanding,
            input.manifest,
            input.toolchain,
        )?;
        let output = logical_path_identity(&input.artifact.output);
        let artifact = artifact_identity(input.artifact);
        let link_environment = input.link_environment.encode().to_vec();
        let link_inputs = link_inputs_identity(input.link_inputs);
        let components = EmitFingerprintComponents {
            compiler: compiler.fingerprints.components,
            artifact: bytes_fingerprint(COMPILER_EMIT_ARTIFACT_DOMAIN, &artifact),
            output: bytes_fingerprint(COMPILER_EMIT_OUTPUT_DOMAIN, &output),
            link_environment: bytes_fingerprint(
                COMPILER_EMIT_LINK_ENVIRONMENT_DOMAIN,
                &link_environment,
            ),
            link_inputs: bytes_fingerprint(COMPILER_EMIT_LINK_INPUTS_DOMAIN, &link_inputs),
        };
        let fingerprints = EmitFingerprintSet {
            cache_key: compiler.fingerprints.cache_key,
            fingerprint: combined_fingerprint(compiler.fingerprints.cache_key, components),
            components,
        };
        Some(Self {
            fingerprints,
            action: compiler.action,
            module: compiler.module,
            package_roots: compiler.package_roots,
            target: compiler.target,
            optimization: compiler.optimization,
            runtime: compiler.runtime,
            sources: compiler.sources,
            artifact,
            output,
            link_environment,
            link_inputs,
        })
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum CompilerEmitCacheLookup {
    Hit(ExecutableCacheReference),
    Miss(ActionCacheMissReason),
}

#[derive(Debug)]
pub(crate) struct CompilerEmitCache {
    root: PathBuf,
}

impl CompilerEmitCache {
    pub(crate) fn new(root: PathBuf) -> Self {
        Self { root }
    }

    pub(crate) fn lookup(
        &self,
        identity: &CompilerEmitCacheIdentity,
    ) -> io::Result<CompilerEmitCacheLookup> {
        let path = self.path(identity.fingerprints);
        let encoded = match read_bounded_compiler_cache_entry(&path) {
            Ok(BoundedCacheEntry::Bytes(encoded)) => encoded,
            Ok(BoundedCacheEntry::Oversized) => {
                self.retire_oversized(&path)?;
                return Ok(CompilerEmitCacheLookup::Miss(
                    ActionCacheMissReason::Corrupt,
                ));
            }
            Err(error) if error.kind() == io::ErrorKind::NotFound => {
                return self.lookup_invalidation(identity);
            }
            Err(error) => return Err(error),
        };
        let Some(entry) = decode_entry(&encoded) else {
            self.retire_corrupt(&path, &encoded)?;
            return Ok(CompilerEmitCacheLookup::Miss(
                ActionCacheMissReason::Corrupt,
            ));
        };
        if !entry_matches(&entry, identity) || path != self.path(entry.fingerprints) {
            self.retire_corrupt(&path, &encoded)?;
            return Ok(CompilerEmitCacheLookup::Miss(
                ActionCacheMissReason::Corrupt,
            ));
        }
        Ok(CompilerEmitCacheLookup::Hit(entry.reference))
    }

    pub(crate) fn publish(
        &self,
        identity: &CompilerEmitCacheIdentity,
        reference: ExecutableCacheReference,
    ) -> io::Result<()> {
        if let CompilerEmitCacheLookup::Hit(found) = self.lookup(identity)? {
            return if found == reference {
                Ok(())
            } else {
                Err(io::Error::new(
                    io::ErrorKind::AlreadyExists,
                    "compiler-emit cache identity is already bound to another executable",
                ))
            };
        }
        let path = self.path(identity.fingerprints);
        let parent = path
            .parent()
            .ok_or_else(|| io::Error::other("invalid compiler-emit cache path"))?;
        fs::create_dir_all(parent)?;
        let staged = parent.join(format!(
            ".nia-compiler-emit-cache-{}-{}.tmp",
            std::process::id(),
            CACHE_STAGE_SEQUENCE.fetch_add(1, Ordering::Relaxed)
        ));
        let encoded = encode_entry(identity, reference);
        validate_compiler_cache_entry_size(&encoded)?;
        let result = (|| {
            let mut file = fs::OpenOptions::new()
                .write(true)
                .create_new(true)
                .open(&staged)?;
            file.write_all(&encoded)?;
            file.sync_all()?;
            drop(file);
            self.install_immutable_entry(&staged, &path, identity, reference)?;
            fs::File::open(parent)?.sync_all()
        })();
        if result.is_err() || staged.exists() {
            let _ = fs::remove_file(&staged);
        }
        result
    }

    pub(crate) fn retire(
        &self,
        identity: &CompilerEmitCacheIdentity,
        reference: ExecutableCacheReference,
    ) -> io::Result<()> {
        let path = self.path(identity.fingerprints);
        let _lock = self.acquire_mutation_lock(&path)?;
        match read_bounded_compiler_cache_entry(&path) {
            Ok(BoundedCacheEntry::Bytes(encoded))
                if decode_entry(&encoded).is_some_and(|entry| {
                    entry_matches(&entry, identity) && entry.reference == reference
                }) =>
            {
                match fs::remove_file(path) {
                    Ok(()) => Ok(()),
                    Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(()),
                    Err(error) => Err(error),
                }
            }
            Ok(BoundedCacheEntry::Oversized) => match fs::remove_file(path) {
                Ok(()) => Ok(()),
                Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(()),
                Err(error) => Err(error),
            },
            Ok(BoundedCacheEntry::Bytes(_)) => Ok(()),
            Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(()),
            Err(error) => Err(error),
        }
    }

    fn lookup_invalidation(
        &self,
        expected: &CompilerEmitCacheIdentity,
    ) -> io::Result<CompilerEmitCacheLookup> {
        let entries = match fs::read_dir(self.key_dir(expected.fingerprints.cache_key)) {
            Ok(entries) => entries,
            Err(error) if error.kind() == io::ErrorKind::NotFound => {
                return Ok(CompilerEmitCacheLookup::Miss(
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
            let encoded = match read_bounded_compiler_cache_entry(&path)? {
                BoundedCacheEntry::Bytes(encoded) => encoded,
                BoundedCacheEntry::Oversized => {
                    self.retire_oversized(&path)?;
                    corrupt = true;
                    continue;
                }
            };
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
            Ok(CompilerEmitCacheLookup::Miss(
                ActionCacheMissReason::Invalidated(reasons),
            ))
        } else if corrupt {
            Ok(CompilerEmitCacheLookup::Miss(
                ActionCacheMissReason::Corrupt,
            ))
        } else {
            Ok(CompilerEmitCacheLookup::Miss(
                ActionCacheMissReason::NotFound,
            ))
        }
    }

    fn key_dir(&self, cache_key: QueryFingerprint) -> PathBuf {
        self.root
            .join("actions")
            .join("compiler-emits")
            .join(COMPILER_EMIT_CACHE.path_component)
            .join(fingerprint_text(cache_key))
    }

    fn path(&self, fingerprints: EmitFingerprintSet) -> PathBuf {
        self.key_dir(fingerprints.cache_key).join(format!(
            "{}.entry",
            fingerprint_text(fingerprints.fingerprint)
        ))
    }

    fn install_immutable_entry(
        &self,
        staged: &Path,
        path: &Path,
        identity: &CompilerEmitCacheIdentity,
        reference: ExecutableCacheReference,
    ) -> io::Result<()> {
        let _lock = self.acquire_mutation_lock(path)?;
        match fs::hard_link(staged, path) {
            Ok(()) => Ok(()),
            Err(error) if error.kind() == io::ErrorKind::AlreadyExists => {
                match read_bounded_compiler_cache_entry(path)? {
                    BoundedCacheEntry::Bytes(encoded)
                        if decode_entry(&encoded).is_some_and(|entry| {
                            entry_matches(&entry, identity) && entry.reference == reference
                        }) =>
                    {
                        Ok(())
                    }
                    BoundedCacheEntry::Bytes(_) | BoundedCacheEntry::Oversized => {
                        Err(io::Error::new(
                            io::ErrorKind::AlreadyExists,
                            "compiler-emit cache entry changed during publication",
                        ))
                    }
                }
            }
            Err(error) => Err(error),
        }
    }

    fn retire_corrupt(&self, path: &Path, observed: &[u8]) -> io::Result<()> {
        let _lock = self.acquire_mutation_lock(path)?;
        match read_bounded_compiler_cache_entry(path) {
            Ok(BoundedCacheEntry::Bytes(current)) if current == observed => {
                match fs::remove_file(path) {
                    Ok(()) => Ok(()),
                    Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(()),
                    Err(error) => Err(error),
                }
            }
            Ok(BoundedCacheEntry::Oversized) => match fs::remove_file(path) {
                Ok(()) => Ok(()),
                Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(()),
                Err(error) => Err(error),
            },
            Ok(BoundedCacheEntry::Bytes(_)) => Ok(()),
            Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(()),
            Err(error) => Err(error),
        }
    }

    fn retire_oversized(&self, path: &Path) -> io::Result<()> {
        let _lock = self.acquire_mutation_lock(path)?;
        match fs::metadata(path) {
            Ok(metadata) if metadata.len() > MAX_COMPILER_CACHE_ENTRY_BYTES as u64 => {
                match fs::remove_file(path) {
                    Ok(()) => Ok(()),
                    Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(()),
                    Err(error) => Err(error),
                }
            }
            Ok(_) => Ok(()),
            Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(()),
            Err(error) => Err(error),
        }
    }

    fn acquire_mutation_lock(&self, path: &Path) -> io::Result<ScopedFileLock> {
        let mut builder = QueryFingerprintBuilder::new(super::ACTION_CACHE_MUTATION_LOCK_DOMAIN);
        builder.write_bytes(path.as_os_str().as_encoded_bytes());
        let lock = self
            .root
            .join("coordination")
            .join("action-cache-mutations")
            .join("compiler-emits")
            .join(COMPILER_EMIT_CACHE.path_component)
            .join(format!("{}.lock", fingerprint_text(builder.finish())));
        ScopedFileLock::acquire_interruptible(lock, || false)?
            .ok_or_else(|| io::Error::other("action-cache mutation lock was cancelled"))
    }
}

#[derive(Debug)]
struct DecodedEntry {
    fingerprints: EmitFingerprintSet,
    action: Vec<u8>,
    module: Vec<u8>,
    package_roots: Vec<u8>,
    target: Vec<u8>,
    optimization: u8,
    runtime: u8,
    sources: Vec<SourceRecord>,
    artifact: Vec<u8>,
    output: Vec<u8>,
    link_environment: Vec<u8>,
    link_inputs: Vec<u8>,
    reference: ExecutableCacheReference,
}

fn encode_entry(
    identity: &CompilerEmitCacheIdentity,
    reference: ExecutableCacheReference,
) -> Vec<u8> {
    let mut encoded = Vec::new();
    encoded.extend_from_slice(COMPILER_EMIT_ENTRY.magic);
    write_fingerprint_set(&mut encoded, identity.fingerprints);
    write_bytes(&mut encoded, &identity.action);
    write_bytes(&mut encoded, &identity.module);
    write_bytes(&mut encoded, &identity.package_roots);
    write_bytes(&mut encoded, &identity.target);
    encoded.push(identity.optimization);
    encoded.push(identity.runtime);
    encoded.extend_from_slice(&(identity.sources.len() as u64).to_le_bytes());
    for source in &identity.sources {
        write_text(&mut encoded, &source.identity);
        write_fingerprint(
            &mut encoded,
            QueryFingerprint::from_parts(source.fingerprint.parts()),
        );
        encoded.extend_from_slice(&(source.byte_len as u64).to_le_bytes());
    }
    write_bytes(&mut encoded, &identity.artifact);
    write_bytes(&mut encoded, &identity.output);
    write_bytes(&mut encoded, &identity.link_environment);
    write_bytes(&mut encoded, &identity.link_inputs);
    let reference = reference.encode();
    write_fingerprint(&mut encoded, reference_checksum(&reference));
    write_bytes(&mut encoded, &reference);
    encoded
}

fn decode_entry(encoded: &[u8]) -> Option<DecodedEntry> {
    let mut cursor = Cursor::new(encoded);
    let mut magic = [0; 8];
    cursor.read_exact(&mut magic).ok()?;
    (magic == *COMPILER_EMIT_ENTRY.magic).then_some(())?;
    let fingerprints = read_fingerprint_set(&mut cursor)?;
    let action = read_bytes(&mut cursor, encoded.len())?;
    let module = read_bytes(&mut cursor, encoded.len())?;
    let package_roots = read_bytes(&mut cursor, encoded.len())?;
    let target = read_bytes(&mut cursor, encoded.len())?;
    let optimization = read_tag(&mut cursor, 6)?;
    let runtime = read_tag(&mut cursor, 2)?;
    let source_len = usize::try_from(read_u64(&mut cursor)?).ok()?;
    (source_len <= encoded.len()).then_some(())?;
    let mut sources = Vec::with_capacity(source_len);
    for _ in 0..source_len {
        let identity = String::from_utf8(read_bytes(&mut cursor, encoded.len())?).ok()?;
        (SourceIdentity::new(&identity).normalized_path() == identity).then_some(())?;
        let fingerprint =
            SourceContentFingerprint::from_parts(read_fingerprint(&mut cursor)?.parts());
        let byte_len = usize::try_from(read_u64(&mut cursor)?).ok()?;
        sources.push(SourceRecord {
            identity,
            fingerprint,
            byte_len,
        });
    }
    sources
        .windows(2)
        .all(|pair| pair[0].identity < pair[1].identity)
        .then_some(())?;
    let artifact = read_bytes(&mut cursor, encoded.len())?;
    let output = read_bytes(&mut cursor, encoded.len())?;
    let link_environment = read_bytes(&mut cursor, encoded.len())?;
    (link_environment.len() == ExecutableCacheEnvironment::ENCODED_LEN).then_some(())?;
    let link_inputs = read_bytes(&mut cursor, encoded.len())?;
    validate_link_inputs_identity(&link_inputs)?;
    let checksum = read_fingerprint(&mut cursor)?;
    let reference = read_bytes(&mut cursor, encoded.len())?;
    (usize::try_from(cursor.position()).ok()? == encoded.len()).then_some(())?;
    (reference_checksum(&reference) == checksum).then_some(())?;
    let reference = ExecutableCacheReference::decode(&reference)?;
    let compiler = FingerprintComponents {
        sources: source_records_fingerprint(&sources),
        module: bytes_fingerprint(COMPILER_CHECK_MODULE_DOMAIN, &module),
        package_roots: bytes_fingerprint(COMPILER_CHECK_PACKAGE_ROOTS_DOMAIN, &package_roots),
        target: bytes_fingerprint(COMPILER_CHECK_TARGET_DOMAIN, &target),
        optimization: integer_fingerprint(
            COMPILER_CHECK_OPTIMIZATION_DOMAIN,
            u64::from(optimization),
        ),
        runtime: integer_fingerprint(COMPILER_CHECK_RUNTIME_DOMAIN, u64::from(runtime)),
        compiler: fingerprints.components.compiler.compiler,
        resource_layout: fingerprints.components.compiler.resource_layout,
        standard_library: fingerprints.components.compiler.standard_library,
        build_protocol: fingerprints.components.compiler.build_protocol,
    };
    let components = EmitFingerprintComponents {
        compiler,
        artifact: bytes_fingerprint(COMPILER_EMIT_ARTIFACT_DOMAIN, &artifact),
        output: bytes_fingerprint(COMPILER_EMIT_OUTPUT_DOMAIN, &output),
        link_environment: bytes_fingerprint(
            COMPILER_EMIT_LINK_ENVIRONMENT_DOMAIN,
            &link_environment,
        ),
        link_inputs: bytes_fingerprint(COMPILER_EMIT_LINK_INPUTS_DOMAIN, &link_inputs),
    };
    (components == fingerprints.components).then_some(())?;
    (action_fingerprint(&action)? == fingerprints.cache_key).then_some(())?;
    (combined_fingerprint(fingerprints.cache_key, components) == fingerprints.fingerprint)
        .then_some(DecodedEntry {
            fingerprints,
            action,
            module,
            package_roots,
            target,
            optimization,
            runtime,
            sources,
            artifact,
            output,
            link_environment,
            link_inputs,
            reference,
        })
}

fn write_fingerprint_set(encoded: &mut Vec<u8>, fingerprints: EmitFingerprintSet) {
    for fingerprint in [
        fingerprints.cache_key,
        fingerprints.fingerprint,
        fingerprints.components.compiler.sources,
        fingerprints.components.compiler.module,
        fingerprints.components.compiler.package_roots,
        fingerprints.components.compiler.target,
        fingerprints.components.compiler.optimization,
        fingerprints.components.compiler.runtime,
        fingerprints.components.compiler.compiler,
        fingerprints.components.compiler.resource_layout,
        fingerprints.components.compiler.standard_library,
        fingerprints.components.compiler.build_protocol,
        fingerprints.components.artifact,
        fingerprints.components.output,
        fingerprints.components.link_environment,
        fingerprints.components.link_inputs,
    ] {
        write_fingerprint(encoded, fingerprint);
    }
}

fn read_fingerprint_set(cursor: &mut Cursor<&[u8]>) -> Option<EmitFingerprintSet> {
    Some(EmitFingerprintSet {
        cache_key: read_fingerprint(cursor)?,
        fingerprint: read_fingerprint(cursor)?,
        components: EmitFingerprintComponents {
            compiler: FingerprintComponents {
                sources: read_fingerprint(cursor)?,
                module: read_fingerprint(cursor)?,
                package_roots: read_fingerprint(cursor)?,
                target: read_fingerprint(cursor)?,
                optimization: read_fingerprint(cursor)?,
                runtime: read_fingerprint(cursor)?,
                compiler: read_fingerprint(cursor)?,
                resource_layout: read_fingerprint(cursor)?,
                standard_library: read_fingerprint(cursor)?,
                build_protocol: read_fingerprint(cursor)?,
            },
            artifact: read_fingerprint(cursor)?,
            output: read_fingerprint(cursor)?,
            link_environment: read_fingerprint(cursor)?,
            link_inputs: read_fingerprint(cursor)?,
        },
    })
}

fn entry_matches(entry: &DecodedEntry, identity: &CompilerEmitCacheIdentity) -> bool {
    entry.fingerprints == identity.fingerprints
        && entry.action == identity.action
        && entry.module == identity.module
        && entry.package_roots == identity.package_roots
        && entry.target == identity.target
        && entry.optimization == identity.optimization
        && entry.runtime == identity.runtime
        && entry.sources == identity.sources
        && entry.artifact == identity.artifact
        && entry.output == identity.output
        && entry.link_environment == identity.link_environment
        && entry.link_inputs == identity.link_inputs
}

fn invalidations(
    found: EmitFingerprintComponents,
    expected: EmitFingerprintComponents,
) -> Vec<ActionCacheInvalidation> {
    let mut reasons = compiler_invalidations(found.compiler, expected.compiler);
    if found.artifact != expected.artifact {
        reasons.push(ActionCacheInvalidation::Artifact);
    }
    if found.output != expected.output {
        reasons.push(ActionCacheInvalidation::Output);
    }
    if found.link_environment != expected.link_environment {
        reasons.push(ActionCacheInvalidation::Linker);
    }
    if found.link_inputs != expected.link_inputs {
        reasons.push(ActionCacheInvalidation::Inputs);
    }
    reasons
}

fn artifact_identity(artifact: &PlanArtifact) -> Vec<u8> {
    let mut encoded = Vec::new();
    write_text(&mut encoded, artifact.key.package().as_str());
    write_text(&mut encoded, artifact.key.name());
    write_text(&mut encoded, artifact.root_module.package().as_str());
    write_text(&mut encoded, artifact.root_module.name());
    encoded.push(match artifact.kind {
        crate::PlanArtifactKind::Executable => 0,
        crate::PlanArtifactKind::ObjectSet => 1,
        crate::PlanArtifactKind::StaticArchive => 2,
    });
    encoded.push(match artifact.runtime {
        crate::Runtime::Bare => 0,
        crate::Runtime::Freestanding => 1,
    });
    encoded
}

fn link_inputs_identity(inputs: &[CompilerEmitCacheLinkInput]) -> Vec<u8> {
    let mut encoded = Vec::new();
    encoded.extend_from_slice(&(inputs.len() as u64).to_le_bytes());
    for input in inputs {
        write_text(&mut encoded, input.artifact.package().as_str());
        write_text(&mut encoded, input.artifact.name());
        write_fingerprint(&mut encoded, input.fingerprint);
        encoded.extend_from_slice(&(input.byte_len as u64).to_le_bytes());
    }
    encoded
}

fn validate_link_inputs_identity(encoded: &[u8]) -> Option<()> {
    let mut cursor = Cursor::new(encoded);
    let count = usize::try_from(read_u64(&mut cursor)?).ok()?;
    (count <= encoded.len()).then_some(())?;
    let mut seen = BTreeSet::new();
    for _ in 0..count {
        let package = String::from_utf8(read_bytes(&mut cursor, encoded.len())?).ok()?;
        let name = String::from_utf8(read_bytes(&mut cursor, encoded.len())?).ok()?;
        let key = crate::ArtifactKey::new(crate::PackageKey::new(package).ok()?, name).ok()?;
        seen.insert(key).then_some(())?;
        read_fingerprint(&mut cursor)?;
        usize::try_from(read_u64(&mut cursor)?).ok()?;
    }
    (usize::try_from(cursor.position()).ok()? == encoded.len()).then_some(())
}

fn combined_fingerprint(
    cache_key: QueryFingerprint,
    components: EmitFingerprintComponents,
) -> QueryFingerprint {
    let mut builder = QueryFingerprintBuilder::new(COMPILER_EMIT_FINGERPRINT_DOMAIN);
    builder.write_fingerprint(cache_key);
    for component in [
        components.compiler.sources,
        components.compiler.module,
        components.compiler.package_roots,
        components.compiler.target,
        components.compiler.optimization,
        components.compiler.runtime,
        components.compiler.compiler,
        components.compiler.resource_layout,
        components.compiler.standard_library,
        components.compiler.build_protocol,
        components.artifact,
        components.output,
        components.link_environment,
        components.link_inputs,
    ] {
        builder.write_fingerprint(component);
    }
    builder.finish()
}

fn reference_checksum(reference: &[u8]) -> QueryFingerprint {
    let mut builder = QueryFingerprintBuilder::new(COMPILER_EMIT_REFERENCE_DOMAIN);
    builder.write_bytes(reference);
    builder.finish()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn fingerprint(value: u64) -> QueryFingerprint {
        QueryFingerprint::from_parts([value, value])
    }

    fn reference(value: u8) -> ExecutableCacheReference {
        ExecutableCacheReference::decode(&[value; ExecutableCacheReference::ENCODED_LEN])
            .expect("fixed-width executable cache reference")
    }

    fn link_artifact() -> crate::ArtifactKey {
        crate::ArtifactKey::new(crate::PackageKey::new("root").unwrap(), "support").unwrap()
    }

    #[test]
    fn streamed_link_input_identity_matches_bytes_and_rejects_length_changes() {
        let expected = CompilerEmitCacheLinkInput::from_bytes(link_artifact(), b"archive");
        let streamed = CompilerEmitCacheLinkInput::from_reader(
            link_artifact(),
            &mut Cursor::new(b"archive"),
            7,
        )
        .expect("stream archive identity");
        assert_eq!(streamed, expected);

        let growth = CompilerEmitCacheLinkInput::from_reader(
            link_artifact(),
            &mut Cursor::new(b"archive!"),
            7,
        )
        .expect_err("archive growth must be rejected");
        assert_eq!(growth.kind(), io::ErrorKind::InvalidData);

        let truncation = CompilerEmitCacheLinkInput::from_reader(
            link_artifact(),
            &mut Cursor::new(b"archive"),
            8,
        )
        .expect_err("archive truncation must be rejected");
        assert_eq!(truncation.kind(), io::ErrorKind::UnexpectedEof);
    }

    fn identity() -> CompilerEmitCacheIdentity {
        let mut action = Vec::new();
        write_text(&mut action, "root");
        write_text(&mut action, "emit");
        let module = b"module".to_vec();
        let target = b"target".to_vec();
        let optimization = 2;
        let runtime = 1;
        let sources = vec![SourceRecord {
            identity: "build-package:root:/src/main.nia".to_string(),
            fingerprint: SourceContentFingerprint::from_parts(fingerprint(1).parts()),
            byte_len: 19,
        }];
        let compiler = FingerprintComponents {
            sources: source_records_fingerprint(&sources),
            module: bytes_fingerprint(COMPILER_CHECK_MODULE_DOMAIN, &module),
            package_roots: bytes_fingerprint(COMPILER_CHECK_PACKAGE_ROOTS_DOMAIN, &[]),
            target: bytes_fingerprint(COMPILER_CHECK_TARGET_DOMAIN, &target),
            optimization: integer_fingerprint(
                COMPILER_CHECK_OPTIMIZATION_DOMAIN,
                u64::from(optimization),
            ),
            runtime: integer_fingerprint(COMPILER_CHECK_RUNTIME_DOMAIN, u64::from(runtime)),
            compiler: fingerprint(6),
            resource_layout: fingerprint(7),
            standard_library: fingerprint(8),
            build_protocol: fingerprint(9),
        };
        let artifact = b"artifact".to_vec();
        let output = b"output".to_vec();
        let link_environment = vec![0; ExecutableCacheEnvironment::ENCODED_LEN];
        let link_inputs = link_inputs_identity(&[CompilerEmitCacheLinkInput::from_bytes(
            link_artifact(),
            b"archive",
        )]);
        let components = EmitFingerprintComponents {
            compiler,
            artifact: bytes_fingerprint(COMPILER_EMIT_ARTIFACT_DOMAIN, &artifact),
            output: bytes_fingerprint(COMPILER_EMIT_OUTPUT_DOMAIN, &output),
            link_environment: bytes_fingerprint(
                COMPILER_EMIT_LINK_ENVIRONMENT_DOMAIN,
                &link_environment,
            ),
            link_inputs: bytes_fingerprint(COMPILER_EMIT_LINK_INPUTS_DOMAIN, &link_inputs),
        };
        let cache_key = action_fingerprint(&action).expect("canonical action identity");
        CompilerEmitCacheIdentity {
            fingerprints: EmitFingerprintSet {
                cache_key,
                fingerprint: combined_fingerprint(cache_key, components),
                components,
            },
            action,
            module,
            package_roots: Vec::new(),
            target,
            optimization,
            runtime,
            sources,
            artifact,
            output,
            link_environment,
            link_inputs,
        }
    }

    #[test]
    fn compiler_emit_entry_round_trips_and_rejects_noncanonical_bytes() {
        let identity = identity();
        let encoded = encode_entry(&identity, reference(0));
        assert!(decode_entry(&encoded).is_some_and(|entry| entry_matches(&entry, &identity)));

        for end in 0..encoded.len() {
            assert!(
                decode_entry(&encoded[..end]).is_none(),
                "accepted {end}-byte prefix"
            );
        }

        let mut trailing = encoded.clone();
        trailing.push(0);
        assert!(decode_entry(&trailing).is_none());

        let mut mismatched = identity.clone();
        mismatched.action[8] ^= 1;
        assert!(decode_entry(&encode_entry(&mismatched, reference(0))).is_none());

        let mut damaged_reference = encoded;
        let reference_start = damaged_reference.len() - ExecutableCacheReference::ENCODED_LEN;
        damaged_reference[reference_start] ^= 1;
        assert!(decode_entry(&damaged_reference).is_none());

        let mut malformed_environment = identity;
        malformed_environment.link_environment.pop();
        malformed_environment
            .fingerprints
            .components
            .link_environment = bytes_fingerprint(
            COMPILER_EMIT_LINK_ENVIRONMENT_DOMAIN,
            &malformed_environment.link_environment,
        );
        malformed_environment.fingerprints.fingerprint = combined_fingerprint(
            malformed_environment.fingerprints.cache_key,
            malformed_environment.fingerprints.components,
        );
        assert!(decode_entry(&encode_entry(&malformed_environment, reference(0))).is_none());
    }

    #[test]
    fn compiler_emit_invalidation_is_component_exact() {
        let baseline = identity().fingerprints.components;
        for (changed, expected) in [
            (
                EmitFingerprintComponents {
                    compiler: FingerprintComponents {
                        sources: fingerprint(20),
                        ..baseline.compiler
                    },
                    ..baseline
                },
                ActionCacheInvalidation::Sources,
            ),
            (
                EmitFingerprintComponents {
                    compiler: FingerprintComponents {
                        package_roots: fingerprint(20),
                        ..baseline.compiler
                    },
                    ..baseline
                },
                ActionCacheInvalidation::PackageRoots,
            ),
            (
                EmitFingerprintComponents {
                    artifact: fingerprint(20),
                    ..baseline
                },
                ActionCacheInvalidation::Artifact,
            ),
            (
                EmitFingerprintComponents {
                    output: fingerprint(20),
                    ..baseline
                },
                ActionCacheInvalidation::Output,
            ),
            (
                EmitFingerprintComponents {
                    link_environment: fingerprint(20),
                    ..baseline
                },
                ActionCacheInvalidation::Linker,
            ),
            (
                EmitFingerprintComponents {
                    link_inputs: fingerprint(20),
                    ..baseline
                },
                ActionCacheInvalidation::Inputs,
            ),
        ] {
            assert_eq!(invalidations(baseline, changed), [expected]);
        }
    }

    #[test]
    fn compiler_emit_binding_requires_explicit_retirement_before_replacement() {
        let root = std::env::temp_dir().join(format!(
            "nia-compiler-emit-cache-binding-{}-{}",
            std::process::id(),
            CACHE_STAGE_SEQUENCE.fetch_add(1, Ordering::Relaxed)
        ));
        let cache = CompilerEmitCache::new(root.clone());
        let identity = identity();
        let first = reference(1);
        let replacement = reference(2);

        cache.publish(&identity, first).expect("publish binding");
        assert_eq!(
            cache.lookup(&identity).expect("lookup binding"),
            CompilerEmitCacheLookup::Hit(first)
        );
        assert_eq!(
            cache
                .publish(&identity, replacement)
                .expect_err("replace live binding")
                .kind(),
            io::ErrorKind::AlreadyExists
        );
        cache.retire(&identity, first).expect("retire binding");
        cache
            .publish(&identity, replacement)
            .expect("publish replacement");
        assert_eq!(
            cache.lookup(&identity).expect("lookup replacement"),
            CompilerEmitCacheLookup::Hit(replacement)
        );

        fs::remove_dir_all(root).expect("remove compiler emit cache fixture");
    }

    #[test]
    fn compiler_emit_corruption_republishes_and_stale_retirement_preserves_it() {
        let root = std::env::temp_dir().join(format!(
            "nia-compiler-emit-cache-replacement-{}-{}",
            std::process::id(),
            CACHE_STAGE_SEQUENCE.fetch_add(1, Ordering::Relaxed)
        ));
        let cache = CompilerEmitCache::new(root.clone());
        let identity = identity();
        let reference = reference(1);
        cache
            .publish(&identity, reference)
            .expect("publish emit entry");
        assert_eq!(
            cache.lookup(&identity).expect("load emit entry"),
            CompilerEmitCacheLookup::Hit(reference)
        );

        let path = cache.path(identity.fingerprints);
        let observed = b"corrupt emit entry".to_vec();
        fs::write(&path, &observed).expect("install corrupt emit entry");
        assert_eq!(
            cache.lookup(&identity).expect("load corrupt emit entry"),
            CompilerEmitCacheLookup::Miss(ActionCacheMissReason::Corrupt)
        );
        assert!(!path.exists());

        cache
            .publish(&identity, reference)
            .expect("republish emit entry");
        cache
            .retire_corrupt(&path, &observed)
            .expect("retire stale emit observation");
        assert_eq!(
            cache
                .lookup(&identity)
                .expect("load replacement emit entry"),
            CompilerEmitCacheLookup::Hit(reference)
        );

        fs::remove_dir_all(root).expect("remove compiler emit cache fixture");
    }

    #[test]
    fn compiler_emit_lookup_retires_oversized_entries_without_reading_them() {
        let identity = identity();
        let root = std::env::temp_dir().join(format!(
            "nia-compiler-emit-cache-oversized-{}-{}",
            std::process::id(),
            CACHE_STAGE_SEQUENCE.fetch_add(1, Ordering::Relaxed)
        ));
        let cache = CompilerEmitCache::new(root.clone());
        let path = cache.path(identity.fingerprints);
        fs::create_dir_all(path.parent().expect("cache parent")).expect("create cache parent");
        let file = fs::File::create(&path).expect("create oversized entry");
        file.set_len((MAX_COMPILER_CACHE_ENTRY_BYTES + 1) as u64)
            .expect("size oversized entry");

        assert_eq!(
            cache.lookup(&identity).expect("lookup oversized entry"),
            CompilerEmitCacheLookup::Miss(ActionCacheMissReason::Corrupt)
        );
        assert!(!path.exists(), "oversized entry must be retired");

        fs::remove_dir_all(root).expect("remove compiler emit cache fixture");
    }
}
