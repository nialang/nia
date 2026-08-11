// SPDX-License-Identifier: GPL-3.0-or-later

use std::{
    fs,
    io::{self, Cursor, Write},
    path::{Path, PathBuf},
    sync::atomic::Ordering,
};

use nia_compat::formats::{COMPILER_CHECK_CACHE, COMPILER_CHECK_ENTRY};
use nia_compiler_query::{SourceContentFingerprint, frontend_program_source_fingerprint};
use nia_driver::{SourceInputContent, SourceInputManifest};
use nia_imports::StableModuleKey;
use nia_query::{FingerprintDomain, QueryFingerprint, QueryFingerprintBuilder};
use nia_source::SourceIdentity;
use nia_toolchain::ToolchainIdentity;

use super::{
    ActionCacheInvalidation, ActionCacheMissReason, CACHE_STAGE_SEQUENCE, fingerprint_text,
    logical_path_identity, package_roots_identity, read_bytes, read_fingerprint, read_u64,
    write_bytes, write_fingerprint, write_text,
};
use crate::{
    ActionKey, OptimizationMode, PlanModule, PlanPackage, Runtime, TargetSpec, lock::ScopedFileLock,
};

pub(super) const COMPILER_CHECK_COMPILER_DOMAIN: FingerprintDomain =
    FingerprintDomain::new("nia.build.compiler-check.compiler.v1");
pub(super) const COMPILER_CHECK_RESOURCE_LAYOUT_DOMAIN: FingerprintDomain =
    FingerprintDomain::new("nia.build.compiler-check.resource-layout.v1");
pub(super) const COMPILER_CHECK_STANDARD_LIBRARY_DOMAIN: FingerprintDomain =
    FingerprintDomain::new("nia.build.compiler-check.standard-library.v1");
pub(super) const COMPILER_CHECK_BUILD_PROTOCOL_DOMAIN: FingerprintDomain =
    FingerprintDomain::new("nia.build.compiler-check.build-protocol.v1");
pub(super) const COMPILER_CHECK_MODULE_DOMAIN: FingerprintDomain =
    FingerprintDomain::new("nia.build.compiler-check.module.v1");
pub(super) const COMPILER_CHECK_PACKAGE_ROOTS_DOMAIN: FingerprintDomain =
    FingerprintDomain::new("nia.build.compiler-check.package-roots.v1");
pub(super) const COMPILER_CHECK_TARGET_DOMAIN: FingerprintDomain =
    FingerprintDomain::new("nia.build.compiler-check.target.v1");
pub(super) const COMPILER_CHECK_OPTIMIZATION_DOMAIN: FingerprintDomain =
    FingerprintDomain::new("nia.build.compiler-check.optimization.v1");
pub(super) const COMPILER_CHECK_RUNTIME_DOMAIN: FingerprintDomain =
    FingerprintDomain::new("nia.build.compiler-check.runtime.v1");
const COMPILER_CHECK_KEY_DOMAIN: FingerprintDomain =
    FingerprintDomain::new("nia.build.compiler-check.key.v1");
const COMPILER_CHECK_FINGERPRINT_DOMAIN: FingerprintDomain =
    FingerprintDomain::new("nia.build.compiler-check.fingerprint.v1");

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct ToolchainComponents {
    compiler: QueryFingerprint,
    resource_layout: QueryFingerprint,
    standard_library: QueryFingerprint,
    build_protocol: QueryFingerprint,
}

impl ToolchainComponents {
    fn new(identity: &ToolchainIdentity) -> Self {
        Self {
            compiler: text_fingerprint(COMPILER_CHECK_COMPILER_DOMAIN, identity.compiler_version()),
            resource_layout: integer_fingerprint(
                COMPILER_CHECK_RESOURCE_LAYOUT_DOMAIN,
                u64::from(identity.resource_layout_schema()),
            ),
            standard_library: integer_fingerprint(
                COMPILER_CHECK_STANDARD_LIBRARY_DOMAIN,
                u64::from(identity.std_schema()),
            ),
            build_protocol: integer_fingerprint(
                COMPILER_CHECK_BUILD_PROTOCOL_DOMAIN,
                u64::from(identity.build_protocol_schema()),
            ),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) struct FingerprintComponents {
    pub(super) sources: QueryFingerprint,
    pub(super) module: QueryFingerprint,
    pub(super) package_roots: QueryFingerprint,
    pub(super) target: QueryFingerprint,
    pub(super) optimization: QueryFingerprint,
    pub(super) runtime: QueryFingerprint,
    pub(super) compiler: QueryFingerprint,
    pub(super) resource_layout: QueryFingerprint,
    pub(super) standard_library: QueryFingerprint,
    pub(super) build_protocol: QueryFingerprint,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) struct FingerprintSet {
    pub(super) cache_key: QueryFingerprint,
    pub(super) fingerprint: QueryFingerprint,
    pub(super) components: FingerprintComponents,
}

struct FingerprintSetInput<'a> {
    module: &'a [u8],
    package_roots: &'a [u8],
    target: &'a [u8],
    optimization: u8,
    runtime: u8,
    sources: QueryFingerprint,
    toolchain: ToolchainComponents,
}

impl FingerprintSet {
    fn new(action: &ActionKey, input: FingerprintSetInput<'_>) -> Self {
        let cache_key = action_key_fingerprint(action);
        let components = FingerprintComponents {
            sources: input.sources,
            module: bytes_fingerprint(COMPILER_CHECK_MODULE_DOMAIN, input.module),
            package_roots: bytes_fingerprint(
                COMPILER_CHECK_PACKAGE_ROOTS_DOMAIN,
                input.package_roots,
            ),
            target: bytes_fingerprint(COMPILER_CHECK_TARGET_DOMAIN, input.target),
            optimization: integer_fingerprint(
                COMPILER_CHECK_OPTIMIZATION_DOMAIN,
                u64::from(input.optimization),
            ),
            runtime: integer_fingerprint(COMPILER_CHECK_RUNTIME_DOMAIN, u64::from(input.runtime)),
            compiler: input.toolchain.compiler,
            resource_layout: input.toolchain.resource_layout,
            standard_library: input.toolchain.standard_library,
            build_protocol: input.toolchain.build_protocol,
        };
        Self {
            cache_key,
            fingerprint: combined_fingerprint(cache_key, components),
            components,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) struct SourceRecord {
    pub(super) identity: String,
    pub(super) fingerprint: SourceContentFingerprint,
    pub(super) byte_len: usize,
}

#[derive(Debug, Clone)]
pub(crate) struct CompilerCheckCacheIdentity {
    pub(super) fingerprints: FingerprintSet,
    pub(super) action: Vec<u8>,
    pub(super) module: Vec<u8>,
    pub(super) package_roots: Vec<u8>,
    pub(super) target: Vec<u8>,
    pub(super) optimization: u8,
    pub(super) runtime: u8,
    pub(super) sources: Vec<SourceRecord>,
}

impl CompilerCheckCacheIdentity {
    pub(crate) fn new(
        action: &ActionKey,
        module: &PlanModule,
        packages: &[PlanPackage],
        target: &TargetSpec,
        runtime: Runtime,
        manifest: &SourceInputManifest,
        toolchain: &ToolchainIdentity,
    ) -> Option<Self> {
        let sources = source_records(manifest)?;
        let source_fingerprint = QueryFingerprint::from_parts(manifest.fingerprint()?.parts());
        let module_identity = module_identity(module);
        let package_roots = package_roots_identity(
            packages,
            std::iter::once(&module.root_source)
                .chain(module.imports.iter().map(|import| &import.path)),
        )?;
        let target_identity = target_identity(target);
        let optimization = optimization_tag(module.optimization);
        let runtime = runtime_tag(runtime);
        Some(Self {
            fingerprints: FingerprintSet::new(
                action,
                FingerprintSetInput {
                    module: &module_identity,
                    package_roots: &package_roots,
                    target: &target_identity,
                    optimization,
                    runtime,
                    sources: source_fingerprint,
                    toolchain: ToolchainComponents::new(toolchain),
                },
            ),
            action: action_identity(action),
            module: module_identity,
            package_roots,
            target: target_identity,
            optimization,
            runtime,
            sources,
        })
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum CompilerCheckCacheLookup {
    Hit,
    Miss(ActionCacheMissReason),
}

#[derive(Debug)]
pub(crate) struct CompilerCheckCache {
    root: PathBuf,
}

impl CompilerCheckCache {
    pub(crate) fn new(root: PathBuf) -> Self {
        Self { root }
    }

    pub(crate) fn lookup(
        &self,
        identity: &CompilerCheckCacheIdentity,
    ) -> io::Result<CompilerCheckCacheLookup> {
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
            return Ok(CompilerCheckCacheLookup::Miss(
                ActionCacheMissReason::Corrupt,
            ));
        };
        if !entry_matches(&entry, identity) || path != self.path(entry.fingerprints) {
            self.retire_corrupt(&path, &encoded)?;
            return Ok(CompilerCheckCacheLookup::Miss(
                ActionCacheMissReason::Corrupt,
            ));
        }
        Ok(CompilerCheckCacheLookup::Hit)
    }

    pub(crate) fn publish(&self, identity: &CompilerCheckCacheIdentity) -> io::Result<()> {
        if self.lookup(identity)? == CompilerCheckCacheLookup::Hit {
            return Ok(());
        }
        let path = self.path(identity.fingerprints);
        let parent = path
            .parent()
            .ok_or_else(|| io::Error::other("invalid compiler-check cache path"))?;
        fs::create_dir_all(parent)?;
        let staged = parent.join(format!(
            ".nia-compiler-check-cache-{}-{}.tmp",
            std::process::id(),
            CACHE_STAGE_SEQUENCE.fetch_add(1, Ordering::Relaxed)
        ));
        let encoded = encode_entry(identity);
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
        expected: &CompilerCheckCacheIdentity,
    ) -> io::Result<CompilerCheckCacheLookup> {
        let entries = match fs::read_dir(self.key_dir(expected.fingerprints.cache_key)) {
            Ok(entries) => entries,
            Err(error) if error.kind() == io::ErrorKind::NotFound => {
                return Ok(CompilerCheckCacheLookup::Miss(
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
            Ok(CompilerCheckCacheLookup::Miss(
                ActionCacheMissReason::Invalidated(reasons),
            ))
        } else if corrupt {
            Ok(CompilerCheckCacheLookup::Miss(
                ActionCacheMissReason::Corrupt,
            ))
        } else {
            Ok(CompilerCheckCacheLookup::Miss(
                ActionCacheMissReason::NotFound,
            ))
        }
    }

    fn key_dir(&self, cache_key: QueryFingerprint) -> PathBuf {
        self.root
            .join("actions")
            .join("compiler-checks")
            .join(COMPILER_CHECK_CACHE.path_component)
            .join(fingerprint_text(cache_key))
    }

    fn path(&self, fingerprints: FingerprintSet) -> PathBuf {
        self.key_dir(fingerprints.cache_key).join(format!(
            "{}.entry",
            fingerprint_text(fingerprints.fingerprint)
        ))
    }

    fn install_immutable_entry(
        &self,
        staged: &Path,
        path: &Path,
        identity: &CompilerCheckCacheIdentity,
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
            "compiler-check cache entry changed during publication",
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
        let mut builder = QueryFingerprintBuilder::new(super::ACTION_CACHE_MUTATION_LOCK_DOMAIN);
        builder.write_bytes(path.as_os_str().as_encoded_bytes());
        let lock = self
            .root
            .join("coordination")
            .join("action-cache-mutations")
            .join("compiler-checks")
            .join(COMPILER_CHECK_CACHE.path_component)
            .join(format!("{}.lock", fingerprint_text(builder.finish())));
        ScopedFileLock::acquire_interruptible(lock, || false)?
            .ok_or_else(|| io::Error::other("action-cache mutation lock was cancelled"))
    }
}

#[derive(Debug)]
struct DecodedEntry {
    fingerprints: FingerprintSet,
    action: Vec<u8>,
    module: Vec<u8>,
    package_roots: Vec<u8>,
    target: Vec<u8>,
    optimization: u8,
    runtime: u8,
    sources: Vec<SourceRecord>,
}

fn encode_entry(identity: &CompilerCheckCacheIdentity) -> Vec<u8> {
    let mut encoded = Vec::new();
    encoded.extend_from_slice(COMPILER_CHECK_ENTRY.magic);
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
    encoded
}

fn decode_entry(encoded: &[u8]) -> Option<DecodedEntry> {
    let mut cursor = Cursor::new(encoded);
    let mut magic = [0; 8];
    std::io::Read::read_exact(&mut cursor, &mut magic).ok()?;
    (magic == *COMPILER_CHECK_ENTRY.magic).then_some(())?;
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
    (usize::try_from(cursor.position()).ok()? == encoded.len()).then_some(())?;
    sources
        .windows(2)
        .all(|pair| pair[0].identity < pair[1].identity)
        .then_some(())?;
    let source_fingerprint = source_records_fingerprint(&sources);
    let components = FingerprintComponents {
        sources: source_fingerprint,
        module: bytes_fingerprint(COMPILER_CHECK_MODULE_DOMAIN, &module),
        package_roots: bytes_fingerprint(COMPILER_CHECK_PACKAGE_ROOTS_DOMAIN, &package_roots),
        target: bytes_fingerprint(COMPILER_CHECK_TARGET_DOMAIN, &target),
        optimization: integer_fingerprint(
            COMPILER_CHECK_OPTIMIZATION_DOMAIN,
            u64::from(optimization),
        ),
        runtime: integer_fingerprint(COMPILER_CHECK_RUNTIME_DOMAIN, u64::from(runtime)),
        compiler: fingerprints.components.compiler,
        resource_layout: fingerprints.components.resource_layout,
        standard_library: fingerprints.components.standard_library,
        build_protocol: fingerprints.components.build_protocol,
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
        })
}

fn write_fingerprint_set(encoded: &mut Vec<u8>, fingerprints: FingerprintSet) {
    for fingerprint in [
        fingerprints.cache_key,
        fingerprints.fingerprint,
        fingerprints.components.sources,
        fingerprints.components.module,
        fingerprints.components.package_roots,
        fingerprints.components.target,
        fingerprints.components.optimization,
        fingerprints.components.runtime,
        fingerprints.components.compiler,
        fingerprints.components.resource_layout,
        fingerprints.components.standard_library,
        fingerprints.components.build_protocol,
    ] {
        write_fingerprint(encoded, fingerprint);
    }
}

fn read_fingerprint_set(cursor: &mut Cursor<&[u8]>) -> Option<FingerprintSet> {
    Some(FingerprintSet {
        cache_key: read_fingerprint(cursor)?,
        fingerprint: read_fingerprint(cursor)?,
        components: FingerprintComponents {
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
    })
}

fn entry_matches(entry: &DecodedEntry, identity: &CompilerCheckCacheIdentity) -> bool {
    entry.fingerprints == identity.fingerprints
        && entry.action == identity.action
        && entry.module == identity.module
        && entry.package_roots == identity.package_roots
        && entry.target == identity.target
        && entry.optimization == identity.optimization
        && entry.runtime == identity.runtime
        && entry.sources == identity.sources
}

pub(super) fn invalidations(
    found: FingerprintComponents,
    expected: FingerprintComponents,
) -> Vec<ActionCacheInvalidation> {
    let mut reasons = Vec::new();
    for (changed, reason) in [
        (
            found.sources != expected.sources,
            ActionCacheInvalidation::Sources,
        ),
        (
            found.module != expected.module,
            ActionCacheInvalidation::Module,
        ),
        (
            found.package_roots != expected.package_roots,
            ActionCacheInvalidation::PackageRoots,
        ),
        (
            found.target != expected.target,
            ActionCacheInvalidation::Target,
        ),
        (
            found.optimization != expected.optimization,
            ActionCacheInvalidation::Optimization,
        ),
        (
            found.runtime != expected.runtime,
            ActionCacheInvalidation::Runtime,
        ),
        (
            found.compiler != expected.compiler,
            ActionCacheInvalidation::Compiler,
        ),
        (
            found.resource_layout != expected.resource_layout,
            ActionCacheInvalidation::ResourceLayout,
        ),
        (
            found.standard_library != expected.standard_library,
            ActionCacheInvalidation::StandardLibrary,
        ),
        (
            found.build_protocol != expected.build_protocol,
            ActionCacheInvalidation::BuildProtocol,
        ),
    ] {
        if changed {
            reasons.push(reason);
        }
    }
    reasons
}

fn source_records(manifest: &SourceInputManifest) -> Option<Vec<SourceRecord>> {
    manifest
        .sources()
        .iter()
        .map(|source| match source.content {
            SourceInputContent::Missing => None,
            SourceInputContent::Present {
                fingerprint,
                byte_len,
            } => Some(SourceRecord {
                identity: source.path.identity().normalized_path().to_string(),
                fingerprint,
                byte_len,
            }),
        })
        .collect()
}

pub(super) fn source_records_fingerprint(sources: &[SourceRecord]) -> QueryFingerprint {
    let modules = sources
        .iter()
        .map(|source| {
            (
                StableModuleKey::from_source_identity(SourceIdentity::new(&source.identity)),
                source.fingerprint,
                source.byte_len,
            )
        })
        .collect::<Vec<_>>();
    QueryFingerprint::from_parts(
        frontend_program_source_fingerprint(
            modules
                .iter()
                .map(|(module, fingerprint, len)| (module, *fingerprint, *len)),
        )
        .parts(),
    )
}

fn action_identity(action: &ActionKey) -> Vec<u8> {
    let mut encoded = Vec::new();
    write_text(&mut encoded, action.package().as_str());
    write_text(&mut encoded, action.name());
    encoded
}

fn action_key_fingerprint(action: &ActionKey) -> QueryFingerprint {
    let mut builder = QueryFingerprintBuilder::new(COMPILER_CHECK_KEY_DOMAIN);
    builder.write_str(action.package().as_str());
    builder.write_str(action.name());
    builder.finish()
}

pub(super) fn action_fingerprint(encoded: &[u8]) -> Option<QueryFingerprint> {
    let mut cursor = Cursor::new(encoded);
    let package = read_bytes(&mut cursor, encoded.len())?;
    let name = read_bytes(&mut cursor, encoded.len())?;
    (usize::try_from(cursor.position()).ok()? == encoded.len()).then_some(())?;
    let mut builder = QueryFingerprintBuilder::new(COMPILER_CHECK_KEY_DOMAIN);
    builder.write_bytes(&package);
    builder.write_bytes(&name);
    Some(builder.finish())
}

fn module_identity(module: &PlanModule) -> Vec<u8> {
    let mut encoded = Vec::new();
    write_text(&mut encoded, module.key.package().as_str());
    write_text(&mut encoded, module.key.name());
    write_bytes(&mut encoded, &logical_path_identity(&module.root_source));
    encoded.extend_from_slice(&(module.imports.len() as u64).to_le_bytes());
    for import in &module.imports {
        write_text(&mut encoded, &import.name);
        write_bytes(&mut encoded, &logical_path_identity(&import.path));
    }
    encoded
}

fn target_identity(target: &TargetSpec) -> Vec<u8> {
    let mut encoded = Vec::new();
    for field in [
        &target.arch,
        &target.vendor,
        &target.os,
        &target.env,
        &target.abi,
        &target.endian,
    ] {
        write_text(&mut encoded, field);
    }
    encoded.extend_from_slice(&u64::from(target.pointer_width).to_le_bytes());
    encoded
}

fn optimization_tag(optimization: OptimizationMode) -> u8 {
    match optimization {
        OptimizationMode::O0 => 0,
        OptimizationMode::O1 => 1,
        OptimizationMode::O2 => 2,
        OptimizationMode::O3 => 3,
        OptimizationMode::Os => 4,
        OptimizationMode::Oz => 5,
    }
}

fn runtime_tag(runtime: Runtime) -> u8 {
    match runtime {
        Runtime::Bare => 0,
        Runtime::Freestanding => 1,
    }
}

pub(super) fn read_tag(cursor: &mut Cursor<&[u8]>, exclusive_max: u8) -> Option<u8> {
    let position = usize::try_from(cursor.position()).ok()?;
    let value = *cursor.get_ref().get(position)?;
    cursor.set_position(cursor.position() + 1);
    (value < exclusive_max).then_some(value)
}

fn combined_fingerprint(
    cache_key: QueryFingerprint,
    components: FingerprintComponents,
) -> QueryFingerprint {
    let mut builder = QueryFingerprintBuilder::new(COMPILER_CHECK_FINGERPRINT_DOMAIN);
    builder.write_fingerprint(cache_key);
    for component in [
        components.sources,
        components.module,
        components.package_roots,
        components.target,
        components.optimization,
        components.runtime,
        components.compiler,
        components.resource_layout,
        components.standard_library,
        components.build_protocol,
    ] {
        builder.write_fingerprint(component);
    }
    builder.finish()
}

pub(super) fn bytes_fingerprint(domain: FingerprintDomain, bytes: &[u8]) -> QueryFingerprint {
    let mut builder = QueryFingerprintBuilder::new(domain);
    builder.write_bytes(bytes);
    builder.finish()
}

fn text_fingerprint(domain: FingerprintDomain, text: &str) -> QueryFingerprint {
    let mut builder = QueryFingerprintBuilder::new(domain);
    builder.write_str(text);
    builder.finish()
}

pub(super) fn integer_fingerprint(domain: FingerprintDomain, value: u64) -> QueryFingerprint {
    let mut builder = QueryFingerprintBuilder::new(domain);
    builder.write_u64(value);
    builder.finish()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{LogicalPath, LogicalPathRoot, ModuleKey, PackageKey};

    fn fingerprint(value: u64) -> QueryFingerprint {
        QueryFingerprint::from_parts([value, value])
    }

    fn identity() -> CompilerCheckCacheIdentity {
        let action = ActionKey::new(PackageKey::root(), "check").unwrap();
        let module = PlanModule {
            key: ModuleKey::new(PackageKey::root(), "app").unwrap(),
            root_source: LogicalPath::new(
                LogicalPathRoot::Package(PackageKey::root()),
                "src/main.nia",
            )
            .unwrap(),
            optimization: OptimizationMode::O2,
            imports: Vec::new(),
        };
        let target = TargetSpec {
            arch: "x86_64".to_string(),
            vendor: "unknown".to_string(),
            os: "linux".to_string(),
            env: String::new(),
            abi: String::new(),
            endian: "little".to_string(),
            pointer_width: 64,
        };
        let sources = vec![SourceRecord {
            identity: "build-package:root:/src/main.nia".to_string(),
            fingerprint: nia_compiler_query::source_content_fingerprint("fn main() i32 { 0 }"),
            byte_len: 19,
        }];
        let module_identity = module_identity(&module);
        let target_identity = target_identity(&target);
        let package_roots = vec![0, 0, 0, 0, 0, 0, 0, 0];
        let fingerprints = FingerprintSet::new(
            &action,
            FingerprintSetInput {
                module: &module_identity,
                package_roots: &package_roots,
                target: &target_identity,
                optimization: optimization_tag(module.optimization),
                runtime: runtime_tag(Runtime::Bare),
                sources: source_records_fingerprint(&sources),
                toolchain: ToolchainComponents {
                    compiler: fingerprint(1),
                    resource_layout: fingerprint(2),
                    standard_library: fingerprint(3),
                    build_protocol: fingerprint(4),
                },
            },
        );
        CompilerCheckCacheIdentity {
            fingerprints,
            action: action_identity(&action),
            module: module_identity,
            package_roots,
            target: target_identity,
            optimization: optimization_tag(module.optimization),
            runtime: runtime_tag(Runtime::Bare),
            sources,
        }
    }

    #[test]
    fn compiler_check_entry_round_trips_and_rejects_noncanonical_bytes() {
        let identity = identity();
        let encoded = encode_entry(&identity);
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
        assert!(decode_entry(&encode_entry(&mismatched)).is_none());
    }

    #[test]
    fn compiler_check_invalidation_is_component_exact() {
        let baseline = identity().fingerprints.components;
        for (changed, expected) in [
            (
                FingerprintComponents {
                    sources: fingerprint(10),
                    ..baseline
                },
                ActionCacheInvalidation::Sources,
            ),
            (
                FingerprintComponents {
                    module: fingerprint(10),
                    ..baseline
                },
                ActionCacheInvalidation::Module,
            ),
            (
                FingerprintComponents {
                    package_roots: fingerprint(10),
                    ..baseline
                },
                ActionCacheInvalidation::PackageRoots,
            ),
            (
                FingerprintComponents {
                    target: fingerprint(10),
                    ..baseline
                },
                ActionCacheInvalidation::Target,
            ),
            (
                FingerprintComponents {
                    optimization: fingerprint(10),
                    ..baseline
                },
                ActionCacheInvalidation::Optimization,
            ),
            (
                FingerprintComponents {
                    runtime: fingerprint(10),
                    ..baseline
                },
                ActionCacheInvalidation::Runtime,
            ),
        ] {
            assert_eq!(invalidations(baseline, changed), [expected]);
        }
    }
}
