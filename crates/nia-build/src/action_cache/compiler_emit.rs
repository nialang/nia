// SPDX-License-Identifier: GPL-3.0-or-later

use std::{
    fs,
    io::{self, Cursor, Read, Write},
    path::{Path, PathBuf},
    sync::atomic::Ordering,
};

use nia_compiler_query::SourceContentFingerprint;
use nia_driver::{ExecutableCacheEnvironment, ExecutableCacheReference, SourceInputManifest};
use nia_query::{QueryFingerprint, QueryFingerprintBuilder};
use nia_source::SourceIdentity;
use nia_toolchain::ToolchainIdentity;

use super::{
    ActionCacheInvalidation, ActionCacheMissReason, CACHE_STAGE_SEQUENCE,
    compiler_check::{
        CompilerCheckCacheIdentity, FingerprintComponents, SourceRecord, action_fingerprint,
        bytes_fingerprint, integer_fingerprint, invalidations as compiler_invalidations, read_tag,
        source_records_fingerprint,
    },
    fingerprint_text, logical_path_identity, read_bytes, read_fingerprint, read_u64, write_bytes,
    write_fingerprint, write_text,
};
use crate::{ActionKey, PlanArtifact, PlanModule, PlanPackage, TargetSpec, lock::ScopedFileLock};

const MAGIC: &[u8; 8] = b"NIAKCE02";
const SCHEMA: &str = "v2";

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct EmitFingerprintComponents {
    compiler: FingerprintComponents,
    artifact: QueryFingerprint,
    output: QueryFingerprint,
    link_environment: QueryFingerprint,
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
}

impl CompilerEmitCacheIdentity {
    pub(crate) fn new(
        action: &ActionKey,
        artifact: &PlanArtifact,
        module: &PlanModule,
        packages: &[PlanPackage],
        target: &TargetSpec,
        manifest: &SourceInputManifest,
        toolchain: &ToolchainIdentity,
        link_environment: ExecutableCacheEnvironment,
    ) -> Option<Self> {
        let compiler = CompilerCheckCacheIdentity::new(
            action,
            module,
            packages,
            target,
            crate::Runtime::Freestanding,
            manifest,
            toolchain,
        )?;
        let output = logical_path_identity(&artifact.output);
        let artifact = artifact_identity(artifact);
        let link_environment = link_environment.encode().to_vec();
        let components = EmitFingerprintComponents {
            compiler: compiler.fingerprints.components,
            artifact: bytes_fingerprint("nia.build.compiler-emit.artifact.v1", &artifact),
            output: bytes_fingerprint("nia.build.compiler-emit.output.v1", &output),
            link_environment: bytes_fingerprint(
                "nia.build.compiler-emit.link-environment.v1",
                &link_environment,
            ),
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
        let encoded = match fs::read(&path) {
            Ok(encoded) => encoded,
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
        match fs::read(&path) {
            Ok(encoded)
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
            Ok(_) => Ok(()),
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
            .join(SCHEMA)
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
                let encoded = fs::read(path)?;
                if decode_entry(&encoded).is_some_and(|entry| {
                    entry_matches(&entry, identity) && entry.reference == reference
                }) {
                    Ok(())
                } else {
                    Err(io::Error::new(
                        io::ErrorKind::AlreadyExists,
                        "compiler-emit cache entry changed during publication",
                    ))
                }
            }
            Err(error) => Err(error),
        }
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
            .join("compiler-emits")
            .join(SCHEMA)
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
    reference: ExecutableCacheReference,
}

fn encode_entry(
    identity: &CompilerEmitCacheIdentity,
    reference: ExecutableCacheReference,
) -> Vec<u8> {
    let mut encoded = Vec::new();
    encoded.extend_from_slice(MAGIC);
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
    let reference = reference.encode();
    write_fingerprint(&mut encoded, reference_checksum(&reference));
    write_bytes(&mut encoded, &reference);
    encoded
}

fn decode_entry(encoded: &[u8]) -> Option<DecodedEntry> {
    let mut cursor = Cursor::new(encoded);
    let mut magic = [0; 8];
    cursor.read_exact(&mut magic).ok()?;
    (magic == *MAGIC).then_some(())?;
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
    let checksum = read_fingerprint(&mut cursor)?;
    let reference = read_bytes(&mut cursor, encoded.len())?;
    (usize::try_from(cursor.position()).ok()? == encoded.len()).then_some(())?;
    (reference_checksum(&reference) == checksum).then_some(())?;
    let reference = ExecutableCacheReference::decode(&reference)?;
    let compiler = FingerprintComponents {
        sources: source_records_fingerprint(&sources),
        module: bytes_fingerprint("nia.build.compiler-check.module.v1", &module),
        package_roots: bytes_fingerprint(
            "nia.build.compiler-check.package-roots.v1",
            &package_roots,
        ),
        target: bytes_fingerprint("nia.build.compiler-check.target.v1", &target),
        optimization: integer_fingerprint(
            "nia.build.compiler-check.optimization.v1",
            u64::from(optimization),
        ),
        runtime: integer_fingerprint("nia.build.compiler-check.runtime.v1", u64::from(runtime)),
        compiler: fingerprints.components.compiler.compiler,
        resource_layout: fingerprints.components.compiler.resource_layout,
        standard_library: fingerprints.components.compiler.standard_library,
        build_protocol: fingerprints.components.compiler.build_protocol,
    };
    let components = EmitFingerprintComponents {
        compiler,
        artifact: bytes_fingerprint("nia.build.compiler-emit.artifact.v1", &artifact),
        output: bytes_fingerprint("nia.build.compiler-emit.output.v1", &output),
        link_environment: bytes_fingerprint(
            "nia.build.compiler-emit.link-environment.v1",
            &link_environment,
        ),
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
    reasons
}

fn artifact_identity(artifact: &PlanArtifact) -> Vec<u8> {
    let mut encoded = Vec::new();
    write_text(&mut encoded, artifact.key.package().as_str());
    write_text(&mut encoded, artifact.key.name());
    write_text(&mut encoded, artifact.root_module.package().as_str());
    write_text(&mut encoded, artifact.root_module.name());
    encoded.push(match artifact.runtime {
        crate::Runtime::Bare => 0,
        crate::Runtime::Freestanding => 1,
    });
    encoded
}

fn combined_fingerprint(
    cache_key: QueryFingerprint,
    components: EmitFingerprintComponents,
) -> QueryFingerprint {
    let mut builder = QueryFingerprintBuilder::new("nia.build.compiler-emit.fingerprint.v1");
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
    ] {
        builder.write_fingerprint(component);
    }
    builder.finish()
}

fn reference_checksum(reference: &[u8]) -> QueryFingerprint {
    let mut builder = QueryFingerprintBuilder::new("nia.build.compiler-emit.reference.v1");
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
            module: bytes_fingerprint("nia.build.compiler-check.module.v1", &module),
            package_roots: bytes_fingerprint("nia.build.compiler-check.package-roots.v1", &[]),
            target: bytes_fingerprint("nia.build.compiler-check.target.v1", &target),
            optimization: integer_fingerprint(
                "nia.build.compiler-check.optimization.v1",
                u64::from(optimization),
            ),
            runtime: integer_fingerprint("nia.build.compiler-check.runtime.v1", u64::from(runtime)),
            compiler: fingerprint(6),
            resource_layout: fingerprint(7),
            standard_library: fingerprint(8),
            build_protocol: fingerprint(9),
        };
        let artifact = b"artifact".to_vec();
        let output = b"output".to_vec();
        let link_environment = vec![0; ExecutableCacheEnvironment::ENCODED_LEN];
        let components = EmitFingerprintComponents {
            compiler,
            artifact: bytes_fingerprint("nia.build.compiler-emit.artifact.v1", &artifact),
            output: bytes_fingerprint("nia.build.compiler-emit.output.v1", &output),
            link_environment: bytes_fingerprint(
                "nia.build.compiler-emit.link-environment.v1",
                &link_environment,
            ),
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
            "nia.build.compiler-emit.link-environment.v1",
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
}
