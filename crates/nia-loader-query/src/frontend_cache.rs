use std::{
    collections::{BTreeSet, HashSet},
    fs::{self, File, OpenOptions},
    io::{self, Cursor, Read, Write},
    path::{Path, PathBuf},
    sync::atomic::{AtomicU64, Ordering},
};

use nia_ast::PathSegmentKind;
use nia_compiler_query::{
    FrontendCacheNamespace, FrontendFacadeFactsCacheKey, FrontendModuleDependenciesCacheKey,
    FrontendModuleMapFingerprint, FrontendProviderSummaryCacheKey,
    FrontendPublicSurfaceFactsCacheKey, FrontendSourceCacheKey, ItemSignatureFingerprint,
    SourceContentFingerprint,
};
use nia_defs::{
    DefKind, ModuleUsing, PublicSurfaceDefFact, PublicSurfaceEnumScopeFact,
    PublicSurfaceModuleFacts, PublicSurfaceModuleScopeFacts, UsingGroupItem, UsingName,
    UsingPathSegment, UsingSelector,
};
use nia_imports::{ResolvedModuleDeclaration, StableModuleKey, Visibility};
use nia_provider_summary::{Provider, ProviderSummary, ProviderTarget, ProviderTypeRef};
use nia_query::{QueryFingerprint, QueryFingerprintBuilder};
use nia_symbol::{SymbolId, stable_hash};
use nia_symbol_table::SymbolTable;

use crate::facade_facts::{ModuleFacadeFacts, PublicReexportSource};
use crate::used_paths::{
    ExplicitUsingImport, ModuleDeclarations, UsedModulePath, UsedModulePathProcessing,
};

const DEPENDENCY_MANIFEST_MAGIC: &[u8; 8] = b"NIAFDM02";
const FACADE_FACTS_MAGIC: &[u8; 8] = b"NIAFFF02";
const MODULE_DEPENDENCIES_MAGIC: &[u8; 8] = b"NIAFMD02";
const PROVIDER_SUMMARY_MAGIC: &[u8; 8] = b"NIAFPS03";
const PUBLIC_SURFACE_FACTS_MAGIC: &[u8; 8] = b"NIAFPF01";
const FRONTEND_CACHE_SCHEMA: &str = "v3";
const MAX_CACHE_PAYLOAD_BYTES: usize = 64 * 1024 * 1024;
const MAX_CACHE_ENTRY_BYTES: usize = MAX_CACHE_PAYLOAD_BYTES + 1024 * 1024;
const MAX_CACHE_SEQUENCE_LEN: usize = 1_000_000;
const MAX_USING_SELECTOR_DEPTH: usize = 256;
static FRONTEND_CACHE_STAGE_ID: AtomicU64 = AtomicU64::new(0);

#[derive(Debug)]
pub(crate) struct PersistentFrontendCache {
    root: PathBuf,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum ProviderSummaryCacheLookup {
    Hit(ProviderSummary),
    NotFound,
    Corrupt,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum FacadeFactsCacheLookup {
    Hit(ModuleFacadeFacts),
    NotFound,
    Corrupt,
}

#[derive(Debug, Clone, PartialEq)]
pub(crate) enum ModuleDependenciesCacheLookup {
    Hit(ModuleDeclarations),
    NotFound,
    Corrupt,
}

#[derive(Debug, Clone, PartialEq)]
pub(crate) enum PublicSurfaceFactsCacheLookup {
    Hit(PublicSurfaceModuleFacts),
    NotFound,
    Corrupt,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct ModuleDependenciesSource {
    pub(crate) fingerprint: SourceContentFingerprint,
    pub(crate) len: usize,
}

impl ModuleDependenciesSource {
    pub(crate) fn new(fingerprint: SourceContentFingerprint, len: usize) -> Self {
        Self { fingerprint, len }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct PublicSurfaceFactsSource {
    pub(crate) fingerprint: SourceContentFingerprint,
    pub(crate) len: usize,
}

impl PublicSurfaceFactsSource {
    pub(crate) fn new(fingerprint: SourceContentFingerprint, len: usize) -> Self {
        Self { fingerprint, len }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum DependencyManifestCacheLookup {
    Hit(ItemSignatureFingerprint),
    NotFound,
    Corrupt,
}

impl PersistentFrontendCache {
    pub(crate) fn new(root: PathBuf) -> Self {
        Self { root }
    }

    pub(crate) fn load_provider_summary(
        &self,
        key: FrontendProviderSummaryCacheKey,
        namespace: FrontendCacheNamespace,
        module: &StableModuleKey,
        item_signature: ItemSignatureFingerprint,
        symbols: &SymbolTable,
    ) -> io::Result<ProviderSummaryCacheLookup> {
        let path = self.provider_summary_path(key);
        let encoded = match read_cache_entry(&path)? {
            Some(encoded) if encoded.len() <= MAX_CACHE_ENTRY_BYTES => encoded,
            Some(_) => {
                remove_corrupt(&path);
                return Ok(ProviderSummaryCacheLookup::Corrupt);
            }
            None => return Ok(ProviderSummaryCacheLookup::NotFound),
        };
        let Some(entry) = decode_provider_summary(&encoded) else {
            remove_corrupt(&path);
            return Ok(ProviderSummaryCacheLookup::Corrupt);
        };
        if entry.key != key.parts()
            || entry.namespace != namespace.parts()
            || entry.module != module.source_identity().normalized_path()
            || entry.item_signature != item_signature.parts()
            || path
                != self
                    .provider_summary_path(FrontendProviderSummaryCacheKey::from_parts(entry.key))
        {
            remove_corrupt(&path);
            return Ok(ProviderSummaryCacheLookup::Corrupt);
        }
        let Some(summary) = decode_provider_summary_payload(&entry.payload, symbols) else {
            remove_corrupt(&path);
            return Ok(ProviderSummaryCacheLookup::Corrupt);
        };
        Ok(ProviderSummaryCacheLookup::Hit(summary))
    }

    pub(crate) fn publish_provider_summary(
        &self,
        key: FrontendProviderSummaryCacheKey,
        namespace: FrontendCacheNamespace,
        module: &StableModuleKey,
        item_signature: ItemSignatureFingerprint,
        summary: &ProviderSummary,
        symbols: &SymbolTable,
    ) -> io::Result<()> {
        let path = self.provider_summary_path(key);
        if path.is_file() {
            return Ok(());
        }
        let parent = path
            .parent()
            .ok_or_else(|| io::Error::other("invalid frontend cache path"))?;
        fs::create_dir_all(parent)?;
        let staged = staged_path(&path);
        let encoded =
            encode_provider_summary(key, namespace, module, item_signature, summary, symbols)?;
        atomic_publish(&staged, &path, &encoded)
    }

    pub(crate) fn remove_provider_summary(&self, key: FrontendProviderSummaryCacheKey) {
        remove_corrupt(&self.provider_summary_path(key));
    }

    pub(crate) fn load_facade_facts(
        &self,
        key: FrontendFacadeFactsCacheKey,
        namespace: FrontendCacheNamespace,
        module: &StableModuleKey,
        item_signature: ItemSignatureFingerprint,
        module_map: FrontendModuleMapFingerprint,
        symbols: &SymbolTable,
    ) -> io::Result<FacadeFactsCacheLookup> {
        let path = self.facade_facts_path(key);
        let encoded = match read_cache_entry(&path)? {
            Some(encoded) if encoded.len() <= MAX_CACHE_ENTRY_BYTES => encoded,
            Some(_) => {
                remove_corrupt(&path);
                return Ok(FacadeFactsCacheLookup::Corrupt);
            }
            None => return Ok(FacadeFactsCacheLookup::NotFound),
        };
        let Some(entry) = decode_facade_facts(&encoded) else {
            remove_corrupt(&path);
            return Ok(FacadeFactsCacheLookup::Corrupt);
        };
        if entry.key != key.parts()
            || entry.namespace != namespace.parts()
            || entry.module != module.source_identity().normalized_path()
            || entry.item_signature != item_signature.parts()
            || entry.module_map != module_map.parts()
            || path != self.facade_facts_path(FrontendFacadeFactsCacheKey::from_parts(entry.key))
        {
            remove_corrupt(&path);
            return Ok(FacadeFactsCacheLookup::Corrupt);
        }
        let Some(facts) = decode_facade_facts_payload(&entry.payload, symbols) else {
            remove_corrupt(&path);
            return Ok(FacadeFactsCacheLookup::Corrupt);
        };
        Ok(FacadeFactsCacheLookup::Hit(facts))
    }

    pub(crate) fn publish_facade_facts(
        &self,
        namespace: FrontendCacheNamespace,
        module: &StableModuleKey,
        item_signature: ItemSignatureFingerprint,
        module_map: FrontendModuleMapFingerprint,
        facts: &ModuleFacadeFacts,
        symbols: &SymbolTable,
    ) -> io::Result<()> {
        let key = FrontendFacadeFactsCacheKey::new(namespace, module, item_signature, module_map);
        let path = self.facade_facts_path(key);
        if path.is_file() {
            return Ok(());
        }
        let parent = path
            .parent()
            .ok_or_else(|| io::Error::other("invalid frontend facade facts path"))?;
        fs::create_dir_all(parent)?;
        let staged = staged_path(&path);
        let encoded = encode_facade_facts(
            key,
            namespace,
            module,
            item_signature,
            module_map,
            facts,
            symbols,
        )?;
        atomic_publish(&staged, &path, &encoded)
    }

    pub(crate) fn remove_facade_facts(&self, key: FrontendFacadeFactsCacheKey) {
        remove_corrupt(&self.facade_facts_path(key));
    }

    pub(crate) fn load_module_dependencies(
        &self,
        key: FrontendModuleDependenciesCacheKey,
        namespace: FrontendCacheNamespace,
        module: &StableModuleKey,
        source: ModuleDependenciesSource,
        module_map: FrontendModuleMapFingerprint,
        symbols: &SymbolTable,
    ) -> io::Result<ModuleDependenciesCacheLookup> {
        let path = self.module_dependencies_path(key);
        let encoded = match read_cache_entry(&path)? {
            Some(encoded) if encoded.len() <= MAX_CACHE_ENTRY_BYTES => encoded,
            Some(_) => {
                remove_corrupt(&path);
                return Ok(ModuleDependenciesCacheLookup::Corrupt);
            }
            None => return Ok(ModuleDependenciesCacheLookup::NotFound),
        };
        let Some(entry) = decode_module_dependencies(&encoded) else {
            remove_corrupt(&path);
            return Ok(ModuleDependenciesCacheLookup::Corrupt);
        };
        if entry.key != key.parts()
            || entry.namespace != namespace.parts()
            || entry.module != module.source_identity().normalized_path()
            || entry.source != source.fingerprint.parts()
            || entry.source_len != source.len
            || entry.module_map != module_map.parts()
            || path
                != self.module_dependencies_path(FrontendModuleDependenciesCacheKey::from_parts(
                    entry.key,
                ))
        {
            remove_corrupt(&path);
            return Ok(ModuleDependenciesCacheLookup::Corrupt);
        }
        let Some(declarations) =
            decode_module_dependencies_payload(&entry.payload, source.len, symbols)
        else {
            remove_corrupt(&path);
            return Ok(ModuleDependenciesCacheLookup::Corrupt);
        };
        Ok(ModuleDependenciesCacheLookup::Hit(declarations))
    }

    pub(crate) fn publish_module_dependencies(
        &self,
        namespace: FrontendCacheNamespace,
        module: &StableModuleKey,
        source: ModuleDependenciesSource,
        module_map: FrontendModuleMapFingerprint,
        declarations: &ModuleDeclarations,
        symbols: &SymbolTable,
    ) -> io::Result<()> {
        if !declarations.diagnostics.is_empty() {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                "cannot cache module dependencies with diagnostics",
            ));
        }
        let key = FrontendModuleDependenciesCacheKey::new(
            namespace,
            module,
            source.fingerprint,
            module_map,
        );
        let path = self.module_dependencies_path(key);
        if path.is_file() {
            return Ok(());
        }
        let parent = path
            .parent()
            .ok_or_else(|| io::Error::other("invalid frontend module dependencies path"))?;
        fs::create_dir_all(parent)?;
        let staged = staged_path(&path);
        let encoded = encode_module_dependencies(
            key,
            namespace,
            module,
            source,
            module_map,
            declarations,
            symbols,
        )?;
        atomic_publish(&staged, &path, &encoded)
    }

    pub(crate) fn remove_module_dependencies(&self, key: FrontendModuleDependenciesCacheKey) {
        remove_corrupt(&self.module_dependencies_path(key));
    }

    pub(crate) fn load_public_surface_facts(
        &self,
        key: FrontendPublicSurfaceFactsCacheKey,
        namespace: FrontendCacheNamespace,
        module: &StableModuleKey,
        source: PublicSurfaceFactsSource,
        symbols: &SymbolTable,
    ) -> io::Result<PublicSurfaceFactsCacheLookup> {
        let path = self.public_surface_facts_path(key);
        let encoded = match read_cache_entry(&path)? {
            Some(encoded) if encoded.len() <= MAX_CACHE_ENTRY_BYTES => encoded,
            Some(_) => {
                remove_corrupt(&path);
                return Ok(PublicSurfaceFactsCacheLookup::Corrupt);
            }
            None => return Ok(PublicSurfaceFactsCacheLookup::NotFound),
        };
        let Some(entry) = decode_public_surface_facts(&encoded) else {
            remove_corrupt(&path);
            return Ok(PublicSurfaceFactsCacheLookup::Corrupt);
        };
        if entry.key != key.parts()
            || entry.namespace != namespace.parts()
            || entry.module != module.source_identity().normalized_path()
            || entry.source != source.fingerprint.parts()
            || entry.source_len != source.len
            || path
                != self.public_surface_facts_path(FrontendPublicSurfaceFactsCacheKey::from_parts(
                    entry.key,
                ))
        {
            remove_corrupt(&path);
            return Ok(PublicSurfaceFactsCacheLookup::Corrupt);
        }
        let Some(facts) = decode_public_surface_facts_payload(&entry.payload, source.len, symbols)
        else {
            remove_corrupt(&path);
            return Ok(PublicSurfaceFactsCacheLookup::Corrupt);
        };
        Ok(PublicSurfaceFactsCacheLookup::Hit(facts))
    }

    pub(crate) fn publish_public_surface_facts(
        &self,
        namespace: FrontendCacheNamespace,
        module: &StableModuleKey,
        source: PublicSurfaceFactsSource,
        facts: &PublicSurfaceModuleFacts,
        symbols: &SymbolTable,
    ) -> io::Result<()> {
        let key = FrontendPublicSurfaceFactsCacheKey::new(namespace, module, source.fingerprint);
        let path = self.public_surface_facts_path(key);
        if path.is_file() {
            return Ok(());
        }
        let parent = path
            .parent()
            .ok_or_else(|| io::Error::other("invalid frontend public surface facts path"))?;
        fs::create_dir_all(parent)?;
        let staged = staged_path(&path);
        let encoded = encode_public_surface_facts(key, namespace, module, source, facts, symbols)?;
        atomic_publish(&staged, &path, &encoded)
    }

    pub(crate) fn remove_public_surface_facts(&self, key: FrontendPublicSurfaceFactsCacheKey) {
        remove_corrupt(&self.public_surface_facts_path(key));
    }

    pub(crate) fn load_dependency_manifest(
        &self,
        key: FrontendSourceCacheKey,
        namespace: FrontendCacheNamespace,
        module: &StableModuleKey,
        source: SourceContentFingerprint,
    ) -> io::Result<DependencyManifestCacheLookup> {
        let path = self.dependency_manifest_path(key);
        let encoded = match read_cache_entry(&path)? {
            Some(encoded) if encoded.len() <= MAX_CACHE_ENTRY_BYTES => encoded,
            Some(_) => {
                remove_corrupt(&path);
                return Ok(DependencyManifestCacheLookup::Corrupt);
            }
            None => return Ok(DependencyManifestCacheLookup::NotFound),
        };
        let Some(entry) = decode_dependency_manifest(&encoded) else {
            remove_corrupt(&path);
            return Ok(DependencyManifestCacheLookup::Corrupt);
        };
        if entry.key != key.parts()
            || entry.namespace != namespace.parts()
            || entry.module != module.source_identity().normalized_path()
            || entry.source != source.parts()
            || path != self.dependency_manifest_path(FrontendSourceCacheKey::from_parts(entry.key))
        {
            remove_corrupt(&path);
            return Ok(DependencyManifestCacheLookup::Corrupt);
        }
        Ok(DependencyManifestCacheLookup::Hit(
            ItemSignatureFingerprint::from_parts(entry.item_signature),
        ))
    }

    pub(crate) fn publish_dependency_manifest(
        &self,
        key: FrontendSourceCacheKey,
        namespace: FrontendCacheNamespace,
        module: &StableModuleKey,
        source: SourceContentFingerprint,
        item_signature: ItemSignatureFingerprint,
    ) -> io::Result<()> {
        let path = self.dependency_manifest_path(key);
        if path.is_file() {
            return Ok(());
        }
        let parent = path
            .parent()
            .ok_or_else(|| io::Error::other("invalid frontend manifest path"))?;
        fs::create_dir_all(parent)?;
        let staged = staged_path(&path);
        let encoded = encode_dependency_manifest(key, namespace, module, source, item_signature);
        atomic_publish(&staged, &path, &encoded)
    }

    pub(crate) fn remove_dependency_manifest(&self, key: FrontendSourceCacheKey) {
        remove_corrupt(&self.dependency_manifest_path(key));
    }

    pub(crate) fn provider_summary_path(&self, key: FrontendProviderSummaryCacheKey) -> PathBuf {
        let [first, second] = key.parts();
        self.root
            .join("artifacts")
            .join("frontend")
            .join(FRONTEND_CACHE_SCHEMA)
            .join("provider-summaries")
            .join(format!("{first:016x}{second:016x}.fps"))
    }

    pub(crate) fn facade_facts_path(&self, key: FrontendFacadeFactsCacheKey) -> PathBuf {
        let [first, second] = key.parts();
        self.root
            .join("artifacts")
            .join("frontend")
            .join(FRONTEND_CACHE_SCHEMA)
            .join("facade-facts")
            .join(format!("{first:016x}{second:016x}.fff"))
    }

    pub(crate) fn module_dependencies_path(
        &self,
        key: FrontendModuleDependenciesCacheKey,
    ) -> PathBuf {
        let [first, second] = key.parts();
        self.root
            .join("artifacts")
            .join("frontend")
            .join(FRONTEND_CACHE_SCHEMA)
            .join("module-dependencies")
            .join(format!("{first:016x}{second:016x}.fmd"))
    }

    pub(crate) fn dependency_manifest_path(&self, key: FrontendSourceCacheKey) -> PathBuf {
        let [first, second] = key.parts();
        self.root
            .join("artifacts")
            .join("frontend")
            .join(FRONTEND_CACHE_SCHEMA)
            .join("dependency-manifests")
            .join(format!("{first:016x}{second:016x}.fdm"))
    }

    pub(crate) fn public_surface_facts_path(
        &self,
        key: FrontendPublicSurfaceFactsCacheKey,
    ) -> PathBuf {
        let [first, second] = key.parts();
        self.root
            .join("artifacts")
            .join("frontend")
            .join(FRONTEND_CACHE_SCHEMA)
            .join("public-surface-facts")
            .join(format!("{first:016x}{second:016x}.fpf"))
    }
}

fn read_cache_entry(path: &Path) -> io::Result<Option<Vec<u8>>> {
    let file = match File::open(path) {
        Ok(file) => file,
        Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(None),
        Err(error) => return Err(error),
    };
    let mut encoded = Vec::new();
    file.take((MAX_CACHE_ENTRY_BYTES + 1) as u64)
        .read_to_end(&mut encoded)?;
    Ok(Some(encoded))
}

fn remove_corrupt(path: &Path) {
    match fs::remove_file(path) {
        Ok(()) => {}
        Err(error) if error.kind() == io::ErrorKind::NotFound => {}
        Err(_) => {}
    }
}

fn staged_path(path: &Path) -> PathBuf {
    path.with_extension(format!(
        "tmp.{}.{}",
        std::process::id(),
        FRONTEND_CACHE_STAGE_ID.fetch_add(1, Ordering::Relaxed)
    ))
}

fn atomic_publish(staged: &Path, path: &Path, encoded: &[u8]) -> io::Result<()> {
    let result = (|| {
        let mut file = OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(staged)?;
        file.write_all(encoded)?;
        file.sync_all()?;
        drop(file);
        match fs::rename(staged, path) {
            Ok(()) => Ok(()),
            Err(_) if path.is_file() => Ok(()),
            Err(error) => Err(error),
        }
    })();
    if result.is_err() || staged.exists() {
        let _ = fs::remove_file(staged);
    }
    result
}

fn encode_dependency_manifest(
    key: FrontendSourceCacheKey,
    namespace: FrontendCacheNamespace,
    module: &StableModuleKey,
    source: SourceContentFingerprint,
    item_signature: ItemSignatureFingerprint,
) -> Vec<u8> {
    let payload = parts_bytes(item_signature.parts());
    let checksum = dependency_manifest_checksum(&payload);
    let mut encoded = Vec::with_capacity(112 + module.source_identity().normalized_path().len());
    encoded.extend_from_slice(DEPENDENCY_MANIFEST_MAGIC);
    write_parts(&mut encoded, key.parts());
    write_parts(&mut encoded, namespace.parts());
    write_string(&mut encoded, module.source_identity().normalized_path());
    write_parts(&mut encoded, source.parts());
    encoded.extend_from_slice(&(payload.len() as u64).to_le_bytes());
    write_parts(&mut encoded, checksum.parts());
    encoded.extend_from_slice(&payload);
    encoded
}

struct DecodedDependencyManifest {
    key: [u64; 2],
    namespace: [u64; 2],
    module: String,
    source: [u64; 2],
    item_signature: [u64; 2],
}

fn decode_dependency_manifest(encoded: &[u8]) -> Option<DecodedDependencyManifest> {
    let mut cursor = Cursor::new(encoded);
    let mut magic = [0; 8];
    cursor.read_exact(&mut magic).ok()?;
    (magic == *DEPENDENCY_MANIFEST_MAGIC).then_some(())?;
    let key = read_parts(&mut cursor)?;
    let namespace = read_parts(&mut cursor)?;
    let module = read_string(&mut cursor, encoded.len())?;
    let source = read_parts(&mut cursor)?;
    let payload_len = read_len(&mut cursor, MAX_CACHE_PAYLOAD_BYTES)?;
    (payload_len == 16).then_some(())?;
    let checksum = read_parts(&mut cursor)?;
    let mut payload = vec![0; payload_len];
    cursor.read_exact(&mut payload).ok()?;
    (cursor.position() as usize == encoded.len()).then_some(())?;
    (dependency_manifest_checksum(&payload).parts() == checksum).then_some(())?;
    let mut payload_cursor = Cursor::new(payload.as_slice());
    let item_signature = read_parts(&mut payload_cursor)?;
    Some(DecodedDependencyManifest {
        key,
        namespace,
        module,
        source,
        item_signature,
    })
}

fn encode_facade_facts(
    key: FrontendFacadeFactsCacheKey,
    namespace: FrontendCacheNamespace,
    module: &StableModuleKey,
    item_signature: ItemSignatureFingerprint,
    module_map: FrontendModuleMapFingerprint,
    facts: &ModuleFacadeFacts,
    symbols: &SymbolTable,
) -> io::Result<Vec<u8>> {
    let payload = encode_facade_facts_payload(facts, symbols)?;
    let checksum = facade_facts_checksum(&payload);
    let mut encoded =
        Vec::with_capacity(112 + module.source_identity().normalized_path().len() + payload.len());
    encoded.extend_from_slice(FACADE_FACTS_MAGIC);
    write_parts(&mut encoded, key.parts());
    write_parts(&mut encoded, namespace.parts());
    write_string(&mut encoded, module.source_identity().normalized_path());
    write_parts(&mut encoded, item_signature.parts());
    write_parts(&mut encoded, module_map.parts());
    encoded.extend_from_slice(&(payload.len() as u64).to_le_bytes());
    write_parts(&mut encoded, checksum.parts());
    encoded.extend_from_slice(&payload);
    Ok(encoded)
}

struct DecodedFacadeFacts {
    key: [u64; 2],
    namespace: [u64; 2],
    module: String,
    item_signature: [u64; 2],
    module_map: [u64; 2],
    payload: Vec<u8>,
}

fn decode_facade_facts(encoded: &[u8]) -> Option<DecodedFacadeFacts> {
    let mut cursor = Cursor::new(encoded);
    let mut magic = [0; 8];
    cursor.read_exact(&mut magic).ok()?;
    (magic == *FACADE_FACTS_MAGIC).then_some(())?;
    let key = read_parts(&mut cursor)?;
    let namespace = read_parts(&mut cursor)?;
    let module = read_string(&mut cursor, encoded.len())?;
    let item_signature = read_parts(&mut cursor)?;
    let module_map = read_parts(&mut cursor)?;
    let payload_len = read_len(&mut cursor, MAX_CACHE_PAYLOAD_BYTES)?;
    let checksum = read_parts(&mut cursor)?;
    let mut payload = vec![0; payload_len];
    cursor.read_exact(&mut payload).ok()?;
    (cursor.position() as usize == encoded.len()).then_some(())?;
    (facade_facts_checksum(&payload).parts() == checksum).then_some(())?;
    Some(DecodedFacadeFacts {
        key,
        namespace,
        module,
        item_signature,
        module_map,
        payload,
    })
}

fn encode_facade_facts_payload(
    facts: &ModuleFacadeFacts,
    symbols: &SymbolTable,
) -> io::Result<Vec<u8>> {
    let mut encoded = Vec::new();
    write_symbol_dictionary(&mut encoded, facade_fact_symbols(facts), symbols)?;
    let mut public_type_names = facts.public_type_names().collect::<Vec<_>>();
    public_type_names.sort_unstable();
    write_symbols(&mut encoded, &public_type_names);
    encoded.extend_from_slice(&(facts.public_reexports().len() as u64).to_le_bytes());
    for reexport in facts.public_reexports() {
        write_optional_symbol(&mut encoded, reexport.exposed_name);
        write_used_module_path(&mut encoded, &reexport.source);
    }
    encoded.extend_from_slice(&(facts.provider_source_paths().len() as u64).to_le_bytes());
    for path in facts.provider_source_paths() {
        write_used_module_path(&mut encoded, path);
    }
    Ok(encoded)
}

fn decode_facade_facts_payload(payload: &[u8], symbols: &SymbolTable) -> Option<ModuleFacadeFacts> {
    let mut cursor = Cursor::new(payload);
    let dictionary = read_symbol_dictionary(&mut cursor, payload.len())?;
    let public_type_names = read_symbols(&mut cursor)?;
    is_strictly_sorted(&public_type_names).then_some(())?;
    let reexport_len = read_len(&mut cursor, MAX_CACHE_SEQUENCE_LEN)?;
    let mut public_reexports = Vec::with_capacity(reexport_len);
    for _ in 0..reexport_len {
        public_reexports.push(PublicReexportSource {
            exposed_name: read_optional_symbol(&mut cursor)?,
            source: read_used_module_path(&mut cursor)?,
        });
    }
    is_strictly_sorted(&public_reexports).then_some(())?;
    let provider_len = read_len(&mut cursor, MAX_CACHE_SEQUENCE_LEN)?;
    let mut provider_source_paths = Vec::with_capacity(provider_len);
    for _ in 0..provider_len {
        provider_source_paths.push(read_used_module_path(&mut cursor)?);
    }
    is_strictly_sorted(&provider_source_paths).then_some(())?;
    (cursor.position() as usize == payload.len()).then_some(())?;
    let facts = ModuleFacadeFacts::from_cache_parts(
        public_type_names,
        public_reexports,
        provider_source_paths,
    );
    install_symbol_dictionary(&dictionary, facade_fact_symbols(&facts), symbols)?;
    Some(facts)
}

fn encode_module_dependencies(
    key: FrontendModuleDependenciesCacheKey,
    namespace: FrontendCacheNamespace,
    module: &StableModuleKey,
    source: ModuleDependenciesSource,
    module_map: FrontendModuleMapFingerprint,
    declarations: &ModuleDeclarations,
    symbols: &SymbolTable,
) -> io::Result<Vec<u8>> {
    let payload = encode_module_dependencies_payload(declarations, symbols)?;
    let checksum = module_dependencies_checksum(&payload);
    let mut encoded =
        Vec::with_capacity(120 + module.source_identity().normalized_path().len() + payload.len());
    encoded.extend_from_slice(MODULE_DEPENDENCIES_MAGIC);
    write_parts(&mut encoded, key.parts());
    write_parts(&mut encoded, namespace.parts());
    write_string(&mut encoded, module.source_identity().normalized_path());
    write_parts(&mut encoded, source.fingerprint.parts());
    encoded.extend_from_slice(&(source.len as u64).to_le_bytes());
    write_parts(&mut encoded, module_map.parts());
    encoded.extend_from_slice(&(payload.len() as u64).to_le_bytes());
    write_parts(&mut encoded, checksum.parts());
    encoded.extend_from_slice(&payload);
    Ok(encoded)
}

struct DecodedModuleDependencies {
    key: [u64; 2],
    namespace: [u64; 2],
    module: String,
    source: [u64; 2],
    source_len: usize,
    module_map: [u64; 2],
    payload: Vec<u8>,
}

fn decode_module_dependencies(encoded: &[u8]) -> Option<DecodedModuleDependencies> {
    let mut cursor = Cursor::new(encoded);
    let mut magic = [0; 8];
    cursor.read_exact(&mut magic).ok()?;
    (magic == *MODULE_DEPENDENCIES_MAGIC).then_some(())?;
    let key = read_parts(&mut cursor)?;
    let namespace = read_parts(&mut cursor)?;
    let module = read_string(&mut cursor, encoded.len())?;
    let source = read_parts(&mut cursor)?;
    let source_len = usize::try_from(read_u64(&mut cursor)?).ok()?;
    let module_map = read_parts(&mut cursor)?;
    let payload_len = read_len(&mut cursor, MAX_CACHE_PAYLOAD_BYTES)?;
    let checksum = read_parts(&mut cursor)?;
    let mut payload = vec![0; payload_len];
    cursor.read_exact(&mut payload).ok()?;
    (cursor.position() as usize == encoded.len()).then_some(())?;
    (module_dependencies_checksum(&payload).parts() == checksum).then_some(())?;
    Some(DecodedModuleDependencies {
        key,
        namespace,
        module,
        source,
        source_len,
        module_map,
        payload,
    })
}

fn encode_module_dependencies_payload(
    declarations: &ModuleDeclarations,
    symbols: &SymbolTable,
) -> io::Result<Vec<u8>> {
    let mut encoded = Vec::new();
    write_symbol_dictionary(
        &mut encoded,
        module_declaration_symbols(declarations),
        symbols,
    )?;
    encoded.extend_from_slice(&(declarations.declarations.len() as u64).to_le_bytes());
    for declaration in &declarations.declarations {
        encoded.extend_from_slice(&declaration.name.raw().to_le_bytes());
        write_visibility(&mut encoded, declaration.visibility);
        write_span(&mut encoded, declaration.span);
    }
    write_symbols(&mut encoded, &declarations.package_roots);
    encoded.extend_from_slice(&(declarations.used_module_paths.len() as u64).to_le_bytes());
    for path in &declarations.used_module_paths {
        write_used_module_path(&mut encoded, path);
    }
    encoded.extend_from_slice(&(declarations.explicit_imports.len() as u64).to_le_bytes());
    for import in &declarations.explicit_imports {
        write_span(&mut encoded, import.span);
        encoded.extend_from_slice(&import.alias.raw().to_le_bytes());
        write_used_module_path(&mut encoded, &import.path);
    }
    write_symbols(&mut encoded, &declarations.used_import_aliases);
    Ok(encoded)
}

fn decode_module_dependencies_payload(
    payload: &[u8],
    source_len: usize,
    symbols: &SymbolTable,
) -> Option<ModuleDeclarations> {
    let mut cursor = Cursor::new(payload);
    let dictionary = read_symbol_dictionary(&mut cursor, payload.len())?;
    let declaration_len = read_len(&mut cursor, MAX_CACHE_SEQUENCE_LEN)?;
    let mut declarations = Vec::with_capacity(declaration_len);
    let mut declaration_names = HashSet::with_capacity(declaration_len);
    for _ in 0..declaration_len {
        let name = read_symbol(&mut cursor)?;
        declaration_names.insert(name).then_some(())?;
        declarations.push(ResolvedModuleDeclaration {
            name,
            visibility: read_visibility(&mut cursor)?,
            span: read_span(&mut cursor, source_len)?,
        });
    }
    let package_roots = read_symbols(&mut cursor)?;
    is_strictly_sorted(&package_roots).then_some(())?;
    let used_path_len = read_len(&mut cursor, MAX_CACHE_SEQUENCE_LEN)?;
    let mut used_module_paths = Vec::with_capacity(used_path_len);
    for _ in 0..used_path_len {
        used_module_paths.push(read_used_module_path(&mut cursor)?);
    }
    is_strictly_sorted(&used_module_paths).then_some(())?;
    let import_len = read_len(&mut cursor, MAX_CACHE_SEQUENCE_LEN)?;
    let mut explicit_imports = Vec::with_capacity(import_len);
    for _ in 0..import_len {
        explicit_imports.push(ExplicitUsingImport {
            span: read_span(&mut cursor, source_len)?,
            alias: read_symbol(&mut cursor)?,
            path: read_used_module_path(&mut cursor)?,
        });
    }
    let used_import_aliases = read_symbols(&mut cursor)?;
    is_strictly_sorted(&used_import_aliases).then_some(())?;
    (cursor.position() as usize == payload.len()).then_some(())?;
    let declarations = ModuleDeclarations {
        declarations,
        package_roots,
        used_module_paths,
        explicit_imports,
        used_import_aliases,
        diagnostics: Vec::new(),
    };
    install_symbol_dictionary(
        &dictionary,
        module_declaration_symbols(&declarations),
        symbols,
    )?;
    Some(declarations)
}

fn encode_public_surface_facts(
    key: FrontendPublicSurfaceFactsCacheKey,
    namespace: FrontendCacheNamespace,
    module: &StableModuleKey,
    source: PublicSurfaceFactsSource,
    facts: &PublicSurfaceModuleFacts,
    symbols: &SymbolTable,
) -> io::Result<Vec<u8>> {
    let payload = encode_public_surface_facts_payload(facts, symbols, source.len)?;
    let checksum = public_surface_facts_checksum(&payload);
    let mut encoded =
        Vec::with_capacity(112 + module.source_identity().normalized_path().len() + payload.len());
    encoded.extend_from_slice(PUBLIC_SURFACE_FACTS_MAGIC);
    write_parts(&mut encoded, key.parts());
    write_parts(&mut encoded, namespace.parts());
    write_string(&mut encoded, module.source_identity().normalized_path());
    write_parts(&mut encoded, source.fingerprint.parts());
    encoded.extend_from_slice(&(source.len as u64).to_le_bytes());
    encoded.extend_from_slice(&(payload.len() as u64).to_le_bytes());
    write_parts(&mut encoded, checksum.parts());
    encoded.extend_from_slice(&payload);
    Ok(encoded)
}

struct DecodedPublicSurfaceFacts {
    key: [u64; 2],
    namespace: [u64; 2],
    module: String,
    source: [u64; 2],
    source_len: usize,
    payload: Vec<u8>,
}

fn decode_public_surface_facts(encoded: &[u8]) -> Option<DecodedPublicSurfaceFacts> {
    let mut cursor = Cursor::new(encoded);
    let mut magic = [0; 8];
    cursor.read_exact(&mut magic).ok()?;
    (magic == *PUBLIC_SURFACE_FACTS_MAGIC).then_some(())?;
    let key = read_parts(&mut cursor)?;
    let namespace = read_parts(&mut cursor)?;
    let module = read_string(&mut cursor, encoded.len())?;
    let source = read_parts(&mut cursor)?;
    let source_len = usize::try_from(read_u64(&mut cursor)?).ok()?;
    let payload_len = read_len(&mut cursor, MAX_CACHE_PAYLOAD_BYTES)?;
    let checksum = read_parts(&mut cursor)?;
    let mut payload = vec![0; payload_len];
    cursor.read_exact(&mut payload).ok()?;
    (cursor.position() as usize == encoded.len()).then_some(())?;
    (public_surface_facts_checksum(&payload).parts() == checksum).then_some(())?;
    Some(DecodedPublicSurfaceFacts {
        key,
        namespace,
        module,
        source,
        source_len,
        payload,
    })
}

fn encode_public_surface_facts_payload(
    facts: &PublicSurfaceModuleFacts,
    symbols: &SymbolTable,
    source_len: usize,
) -> io::Result<Vec<u8>> {
    validate_public_surface_facts(facts, source_len).ok_or_else(|| {
        io::Error::new(
            io::ErrorKind::InvalidInput,
            "cannot persist malformed public surface facts",
        )
    })?;
    let fact_symbols = public_surface_fact_symbols(facts)?;
    let mut encoded = Vec::new();
    write_symbol_dictionary(&mut encoded, fact_symbols, symbols)?;
    encoded.extend_from_slice(&(facts.defs.len() as u64).to_le_bytes());
    for def in &facts.defs {
        write_def_id(&mut encoded, def.id);
        encoded.extend_from_slice(&def.name.raw().to_le_bytes());
        write_def_kind(&mut encoded, def.kind);
        write_optional_def_id(&mut encoded, def.parent);
        write_visibility(&mut encoded, def.visibility);
        write_span(&mut encoded, def.span);
    }
    write_public_surface_scope_entries(&mut encoded, &facts.module_scope.modules);
    write_public_surface_scope_entries(&mut encoded, &facts.module_scope.types);
    write_public_surface_scope_entries(&mut encoded, &facts.module_scope.values);
    encoded.extend_from_slice(&(facts.enum_scopes.len() as u64).to_le_bytes());
    for scope in &facts.enum_scopes {
        write_def_id(&mut encoded, scope.owner);
        write_public_surface_scope_entries(&mut encoded, &scope.variants);
    }
    encoded.extend_from_slice(&(facts.module_usings.len() as u64).to_le_bytes());
    for using in &facts.module_usings {
        write_module_using(&mut encoded, using, 0)?;
    }
    Ok(encoded)
}

fn decode_public_surface_facts_payload(
    payload: &[u8],
    source_len: usize,
    symbols: &SymbolTable,
) -> Option<PublicSurfaceModuleFacts> {
    let mut cursor = Cursor::new(payload);
    let dictionary = read_symbol_dictionary(&mut cursor, payload.len())?;
    let def_len = read_len(&mut cursor, MAX_CACHE_SEQUENCE_LEN)?;
    let mut defs = Vec::with_capacity(def_len);
    for _ in 0..def_len {
        defs.push(PublicSurfaceDefFact {
            id: read_def_id(&mut cursor)?,
            name: read_symbol(&mut cursor)?,
            kind: read_def_kind(&mut cursor)?,
            parent: read_optional_def_id(&mut cursor)?,
            visibility: read_visibility(&mut cursor)?,
            span: read_span(&mut cursor, source_len)?,
        });
    }
    let module_scope = PublicSurfaceModuleScopeFacts {
        modules: read_public_surface_scope_entries(&mut cursor)?,
        types: read_public_surface_scope_entries(&mut cursor)?,
        values: read_public_surface_scope_entries(&mut cursor)?,
    };
    let enum_len = read_len(&mut cursor, MAX_CACHE_SEQUENCE_LEN)?;
    let mut enum_scopes = Vec::with_capacity(enum_len);
    for _ in 0..enum_len {
        enum_scopes.push(PublicSurfaceEnumScopeFact {
            owner: read_def_id(&mut cursor)?,
            variants: read_public_surface_scope_entries(&mut cursor)?,
        });
    }
    let using_len = read_len(&mut cursor, MAX_CACHE_SEQUENCE_LEN)?;
    let mut module_usings = Vec::with_capacity(using_len);
    for _ in 0..using_len {
        module_usings.push(read_module_using(&mut cursor, source_len, 0)?);
    }
    (cursor.position() as usize == payload.len()).then_some(())?;
    let facts = PublicSurfaceModuleFacts {
        defs,
        module_scope,
        enum_scopes,
        module_usings,
    };
    validate_public_surface_facts(&facts, source_len)?;
    install_symbol_dictionary(
        &dictionary,
        public_surface_fact_symbols(&facts).ok()?,
        symbols,
    )?;
    Some(facts)
}

fn validate_public_surface_facts(
    facts: &PublicSurfaceModuleFacts,
    source_len: usize,
) -> Option<()> {
    facts
        .defs
        .windows(2)
        .all(|pair| pair[0].id < pair[1].id)
        .then_some(())?;
    let defs = facts
        .defs
        .iter()
        .map(|def| (def.id, def))
        .collect::<std::collections::HashMap<_, _>>();
    for def in &facts.defs {
        valid_source_span(def.span, source_len).then_some(())?;
        def.parent
            .is_none_or(|parent| defs.contains_key(&parent))
            .then_some(())?;
    }
    validate_public_surface_scope_entries(&facts.module_scope.modules, &defs, |kind| {
        kind == DefKind::Module
    })?;
    validate_public_surface_scope_entries(&facts.module_scope.types, &defs, |kind| {
        matches!(
            kind,
            DefKind::Struct | DefKind::Union | DefKind::Trait | DefKind::Enum | DefKind::TypeAlias
        )
    })?;
    validate_public_surface_scope_entries(&facts.module_scope.values, &defs, |kind| {
        matches!(kind, DefKind::Function | DefKind::Global | DefKind::Const)
    })?;
    facts
        .enum_scopes
        .windows(2)
        .all(|pair| pair[0].owner < pair[1].owner)
        .then_some(())?;
    for scope in &facts.enum_scopes {
        (defs.get(&scope.owner)?.kind == DefKind::Enum).then_some(())?;
        validate_public_surface_scope_entries(&scope.variants, &defs, |kind| {
            kind == DefKind::EnumVariant
        })?;
        for (_, variant) in &scope.variants {
            (defs.get(variant)?.parent == Some(scope.owner)).then_some(())?;
        }
    }
    for using in &facts.module_usings {
        validate_module_using(using, source_len, 0)?;
    }
    Some(())
}

fn validate_module_using(using: &ModuleUsing, source_len: usize, depth: usize) -> Option<()> {
    (depth <= MAX_USING_SELECTOR_DEPTH).then_some(())?;
    valid_source_span(using.span, source_len).then_some(())?;
    for segment in &using.host {
        valid_source_span(segment.span, source_len).then_some(())?;
    }
    validate_using_selector(&using.selector, source_len, depth)
}

fn validate_using_selector(
    selector: &UsingSelector,
    source_len: usize,
    depth: usize,
) -> Option<()> {
    (depth <= MAX_USING_SELECTOR_DEPTH).then_some(())?;
    match selector {
        UsingSelector::Single(name) => validate_using_name(name, source_len),
        UsingSelector::Group(items) => {
            for item in items {
                match item {
                    UsingGroupItem::Name(name) => validate_using_name(name, source_len)?,
                    UsingGroupItem::Nested { host, selector } => {
                        for segment in host {
                            valid_source_span(segment.span, source_len).then_some(())?;
                        }
                        validate_using_selector(selector, source_len, depth + 1)?;
                    }
                }
            }
            Some(())
        }
        UsingSelector::Wildcard { span } => valid_source_span(*span, source_len).then_some(()),
        UsingSelector::SelfName => Some(()),
    }
}

fn validate_using_name(name: &UsingName, source_len: usize) -> Option<()> {
    valid_source_span(name.name_span, source_len).then_some(())?;
    match (name.alias, name.alias_span) {
        (Some(_), Some(alias_span)) => valid_source_span(alias_span, source_len).then_some(()),
        (None, None) => Some(()),
        _ => None,
    }
}

fn valid_source_span(span: nia_span::Span, source_len: usize) -> bool {
    span.start <= span.end && span.end <= source_len
}

fn validate_public_surface_scope_entries(
    entries: &[(SymbolId, nia_defs::DefId)],
    defs: &std::collections::HashMap<nia_defs::DefId, &PublicSurfaceDefFact>,
    valid_kind: impl Fn(DefKind) -> bool,
) -> Option<()> {
    entries
        .windows(2)
        .all(|pair| pair[0].0 < pair[1].0)
        .then_some(())?;
    for (name, def_id) in entries {
        let def = defs.get(def_id)?;
        (def.name == *name && valid_kind(def.kind)).then_some(())?;
    }
    Some(())
}

fn write_public_surface_scope_entries(
    encoded: &mut Vec<u8>,
    entries: &[(SymbolId, nia_defs::DefId)],
) {
    encoded.extend_from_slice(&(entries.len() as u64).to_le_bytes());
    for (name, def_id) in entries {
        encoded.extend_from_slice(&name.raw().to_le_bytes());
        write_def_id(encoded, *def_id);
    }
}

fn read_public_surface_scope_entries(
    cursor: &mut Cursor<&[u8]>,
) -> Option<Vec<(SymbolId, nia_defs::DefId)>> {
    let len = read_len(cursor, MAX_CACHE_SEQUENCE_LEN)?;
    let mut entries = Vec::with_capacity(len);
    for _ in 0..len {
        entries.push((read_symbol(cursor)?, read_def_id(cursor)?));
    }
    Some(entries)
}

fn write_def_id(encoded: &mut Vec<u8>, def_id: nia_defs::DefId) {
    encoded.extend_from_slice(&def_id.0.to_le_bytes());
}

fn read_def_id(cursor: &mut Cursor<&[u8]>) -> Option<nia_defs::DefId> {
    Some(nia_defs::DefId(read_u64(cursor)?))
}

fn write_optional_def_id(encoded: &mut Vec<u8>, def_id: Option<nia_defs::DefId>) {
    match def_id {
        Some(def_id) => {
            encoded.push(1);
            write_def_id(encoded, def_id);
        }
        None => encoded.push(0),
    }
}

fn read_optional_def_id(cursor: &mut Cursor<&[u8]>) -> Option<Option<nia_defs::DefId>> {
    match read_u8(cursor)? {
        0 => Some(None),
        1 => Some(Some(read_def_id(cursor)?)),
        _ => None,
    }
}

fn write_def_kind(encoded: &mut Vec<u8>, kind: DefKind) {
    encoded.push(match kind {
        DefKind::Module => 0,
        DefKind::Function => 1,
        DefKind::Global => 2,
        DefKind::Const => 3,
        DefKind::Struct => 4,
        DefKind::StructField => 5,
        DefKind::Union => 6,
        DefKind::UnionField => 7,
        DefKind::Trait => 8,
        DefKind::TraitAssociatedType => 9,
        DefKind::TraitMethod => 10,
        DefKind::Method => 11,
        DefKind::Enum => 12,
        DefKind::EnumVariant => 13,
        DefKind::TypeAlias => 14,
    });
}

fn read_def_kind(cursor: &mut Cursor<&[u8]>) -> Option<DefKind> {
    match read_u8(cursor)? {
        0 => Some(DefKind::Module),
        1 => Some(DefKind::Function),
        2 => Some(DefKind::Global),
        3 => Some(DefKind::Const),
        4 => Some(DefKind::Struct),
        5 => Some(DefKind::StructField),
        6 => Some(DefKind::Union),
        7 => Some(DefKind::UnionField),
        8 => Some(DefKind::Trait),
        9 => Some(DefKind::TraitAssociatedType),
        10 => Some(DefKind::TraitMethod),
        11 => Some(DefKind::Method),
        12 => Some(DefKind::Enum),
        13 => Some(DefKind::EnumVariant),
        14 => Some(DefKind::TypeAlias),
        _ => None,
    }
}

fn write_module_using(encoded: &mut Vec<u8>, using: &ModuleUsing, depth: usize) -> io::Result<()> {
    if depth > MAX_USING_SELECTOR_DEPTH {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            "public surface using selector is too deeply nested",
        ));
    }
    write_visibility(encoded, using.visibility);
    write_span(encoded, using.span);
    write_using_path_segments(encoded, &using.host);
    write_using_selector(encoded, &using.selector, depth)
}

fn read_module_using(
    cursor: &mut Cursor<&[u8]>,
    source_len: usize,
    depth: usize,
) -> Option<ModuleUsing> {
    (depth <= MAX_USING_SELECTOR_DEPTH).then_some(())?;
    Some(ModuleUsing {
        visibility: read_visibility(cursor)?,
        span: read_span(cursor, source_len)?,
        host: read_using_path_segments(cursor, source_len)?,
        selector: read_using_selector(cursor, source_len, depth)?,
    })
}

fn write_using_path_segments(encoded: &mut Vec<u8>, segments: &[UsingPathSegment]) {
    encoded.extend_from_slice(&(segments.len() as u64).to_le_bytes());
    for segment in segments {
        match segment.kind {
            PathSegmentKind::Name(name) => {
                encoded.push(0);
                encoded.extend_from_slice(&name.raw().to_le_bytes());
            }
            PathSegmentKind::Package => encoded.push(1),
            PathSegmentKind::Super => encoded.push(2),
            PathSegmentKind::SelfValue => encoded.push(3),
        }
        write_span(encoded, segment.span);
    }
}

fn read_using_path_segments(
    cursor: &mut Cursor<&[u8]>,
    source_len: usize,
) -> Option<Vec<UsingPathSegment>> {
    let len = read_len(cursor, MAX_CACHE_SEQUENCE_LEN)?;
    let mut segments = Vec::with_capacity(len);
    for _ in 0..len {
        let kind = match read_u8(cursor)? {
            0 => PathSegmentKind::Name(read_symbol(cursor)?),
            1 => PathSegmentKind::Package,
            2 => PathSegmentKind::Super,
            3 => PathSegmentKind::SelfValue,
            _ => return None,
        };
        segments.push(UsingPathSegment {
            kind,
            span: read_span(cursor, source_len)?,
        });
    }
    Some(segments)
}

fn write_using_selector(
    encoded: &mut Vec<u8>,
    selector: &UsingSelector,
    depth: usize,
) -> io::Result<()> {
    if depth > MAX_USING_SELECTOR_DEPTH {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            "public surface using selector is too deeply nested",
        ));
    }
    match selector {
        UsingSelector::Single(name) => {
            encoded.push(0);
            write_using_name(encoded, name);
        }
        UsingSelector::Group(items) => {
            encoded.push(1);
            encoded.extend_from_slice(&(items.len() as u64).to_le_bytes());
            for item in items {
                write_using_group_item(encoded, item, depth + 1)?;
            }
        }
        UsingSelector::Wildcard { span } => {
            encoded.push(2);
            write_span(encoded, *span);
        }
        UsingSelector::SelfName => encoded.push(3),
    }
    Ok(())
}

fn read_using_selector(
    cursor: &mut Cursor<&[u8]>,
    source_len: usize,
    depth: usize,
) -> Option<UsingSelector> {
    (depth <= MAX_USING_SELECTOR_DEPTH).then_some(())?;
    match read_u8(cursor)? {
        0 => Some(UsingSelector::Single(read_using_name(cursor, source_len)?)),
        1 => {
            let len = read_len(cursor, MAX_CACHE_SEQUENCE_LEN)?;
            let mut items = Vec::with_capacity(len);
            for _ in 0..len {
                items.push(read_using_group_item(cursor, source_len, depth + 1)?);
            }
            Some(UsingSelector::Group(items))
        }
        2 => Some(UsingSelector::Wildcard {
            span: read_span(cursor, source_len)?,
        }),
        3 => Some(UsingSelector::SelfName),
        _ => None,
    }
}

fn write_using_group_item(
    encoded: &mut Vec<u8>,
    item: &UsingGroupItem,
    depth: usize,
) -> io::Result<()> {
    match item {
        UsingGroupItem::Name(name) => {
            encoded.push(0);
            write_using_name(encoded, name);
        }
        UsingGroupItem::Nested { host, selector } => {
            encoded.push(1);
            write_using_path_segments(encoded, host);
            write_using_selector(encoded, selector, depth)?;
        }
    }
    Ok(())
}

fn read_using_group_item(
    cursor: &mut Cursor<&[u8]>,
    source_len: usize,
    depth: usize,
) -> Option<UsingGroupItem> {
    (depth <= MAX_USING_SELECTOR_DEPTH).then_some(())?;
    match read_u8(cursor)? {
        0 => Some(UsingGroupItem::Name(read_using_name(cursor, source_len)?)),
        1 => Some(UsingGroupItem::Nested {
            host: read_using_path_segments(cursor, source_len)?,
            selector: Box::new(read_using_selector(cursor, source_len, depth)?),
        }),
        _ => None,
    }
}

fn write_using_name(encoded: &mut Vec<u8>, name: &UsingName) {
    encoded.extend_from_slice(&name.name.raw().to_le_bytes());
    write_span(encoded, name.name_span);
    match (name.alias, name.alias_span) {
        (Some(alias), Some(alias_span)) => {
            encoded.push(1);
            encoded.extend_from_slice(&alias.raw().to_le_bytes());
            write_span(encoded, alias_span);
        }
        _ => encoded.push(0),
    }
}

fn read_using_name(cursor: &mut Cursor<&[u8]>, source_len: usize) -> Option<UsingName> {
    let name = read_symbol(cursor)?;
    let name_span = read_span(cursor, source_len)?;
    let (alias, alias_span) = match read_u8(cursor)? {
        0 => (None, None),
        1 => (
            Some(read_symbol(cursor)?),
            Some(read_span(cursor, source_len)?),
        ),
        _ => return None,
    };
    Some(UsingName {
        name,
        name_span,
        alias,
        alias_span,
    })
}

fn write_visibility(encoded: &mut Vec<u8>, visibility: Visibility) {
    encoded.push(match visibility {
        Visibility::Private => 0,
        Visibility::PublicSuper => 1,
        Visibility::PublicPkg => 2,
        Visibility::Public => 3,
    });
}

fn read_visibility(cursor: &mut Cursor<&[u8]>) -> Option<Visibility> {
    match read_u8(cursor)? {
        0 => Some(Visibility::Private),
        1 => Some(Visibility::PublicSuper),
        2 => Some(Visibility::PublicPkg),
        3 => Some(Visibility::Public),
        _ => None,
    }
}

fn write_span(encoded: &mut Vec<u8>, span: nia_span::Span) {
    encoded.extend_from_slice(&(span.start as u64).to_le_bytes());
    encoded.extend_from_slice(&(span.end as u64).to_le_bytes());
}

fn read_span(cursor: &mut Cursor<&[u8]>, source_len: usize) -> Option<nia_span::Span> {
    let start = usize::try_from(read_u64(cursor)?).ok()?;
    let end = usize::try_from(read_u64(cursor)?).ok()?;
    (start <= end && end <= source_len).then_some(nia_span::Span { start, end })
}

fn write_used_module_path(encoded: &mut Vec<u8>, path: &UsedModulePath) {
    let (tag, package, segments, include_declared_children, processing) = match path {
        UsedModulePath::Package {
            package,
            segments,
            include_declared_children,
            processing,
        } => (
            0,
            Some(*package),
            segments,
            *include_declared_children,
            processing,
        ),
        UsedModulePath::PackageRelative {
            segments,
            include_declared_children,
            processing,
        } => (1, None, segments, *include_declared_children, processing),
        UsedModulePath::ParentRelative {
            segments,
            include_declared_children,
            processing,
        } => (2, None, segments, *include_declared_children, processing),
        UsedModulePath::Local {
            segments,
            include_declared_children,
            processing,
        } => (3, None, segments, *include_declared_children, processing),
    };
    encoded.push(tag);
    if let Some(package) = package {
        encoded.extend_from_slice(&package.raw().to_le_bytes());
    }
    write_symbols(encoded, segments);
    encoded.push(u8::from(include_declared_children));
    write_used_module_path_processing(encoded, processing);
}

fn read_used_module_path(cursor: &mut Cursor<&[u8]>) -> Option<UsedModulePath> {
    let tag = read_u8(cursor)?;
    (tag <= 3).then_some(())?;
    let package = if tag == 0 {
        Some(read_symbol(cursor)?)
    } else {
        None
    };
    let segments = read_symbols(cursor)?;
    let include_declared_children = read_bool(cursor)?;
    let processing = read_used_module_path_processing(cursor)?;
    match tag {
        0 => Some(UsedModulePath::Package {
            package: package?,
            segments,
            include_declared_children,
            processing,
        }),
        1 => Some(UsedModulePath::PackageRelative {
            segments,
            include_declared_children,
            processing,
        }),
        2 => Some(UsedModulePath::ParentRelative {
            segments,
            include_declared_children,
            processing,
        }),
        3 => Some(UsedModulePath::Local {
            segments,
            include_declared_children,
            processing,
        }),
        _ => None,
    }
}

fn write_used_module_path_processing(encoded: &mut Vec<u8>, processing: &UsedModulePathProcessing) {
    match processing {
        UsedModulePathProcessing::Never => encoded.push(0),
        UsedModulePathProcessing::Always => encoded.push(1),
        UsedModulePathProcessing::IfSelectedItem => encoded.push(2),
        UsedModulePathProcessing::IfProvidesExtensions => encoded.push(3),
        UsedModulePathProcessing::IfProvidesTraitImpl { trait_name } => {
            encoded.push(4);
            encoded.extend_from_slice(&trait_name.raw().to_le_bytes());
        }
        UsedModulePathProcessing::IfProvidesImplicitTraitImpl { trait_name } => {
            encoded.push(5);
            encoded.extend_from_slice(&trait_name.raw().to_le_bytes());
        }
        UsedModulePathProcessing::IfProvidesTraitMethod {
            target_type_name,
            associated_name,
        } => {
            encoded.push(6);
            write_optional_symbol(encoded, *target_type_name);
            encoded.extend_from_slice(&associated_name.raw().to_le_bytes());
        }
    }
}

fn read_used_module_path_processing(
    cursor: &mut Cursor<&[u8]>,
) -> Option<UsedModulePathProcessing> {
    match read_u8(cursor)? {
        0 => Some(UsedModulePathProcessing::Never),
        1 => Some(UsedModulePathProcessing::Always),
        2 => Some(UsedModulePathProcessing::IfSelectedItem),
        3 => Some(UsedModulePathProcessing::IfProvidesExtensions),
        4 => Some(UsedModulePathProcessing::IfProvidesTraitImpl {
            trait_name: read_symbol(cursor)?,
        }),
        5 => Some(UsedModulePathProcessing::IfProvidesImplicitTraitImpl {
            trait_name: read_symbol(cursor)?,
        }),
        6 => Some(UsedModulePathProcessing::IfProvidesTraitMethod {
            target_type_name: read_optional_symbol(cursor)?,
            associated_name: read_symbol(cursor)?,
        }),
        _ => None,
    }
}

fn encode_provider_summary(
    key: FrontendProviderSummaryCacheKey,
    namespace: FrontendCacheNamespace,
    module: &StableModuleKey,
    item_signature: ItemSignatureFingerprint,
    summary: &ProviderSummary,
    symbols: &SymbolTable,
) -> io::Result<Vec<u8>> {
    let payload = encode_provider_summary_payload(summary, symbols)?;
    let checksum = payload_checksum(&payload);
    let mut encoded =
        Vec::with_capacity(96 + module.source_identity().normalized_path().len() + payload.len());
    encoded.extend_from_slice(PROVIDER_SUMMARY_MAGIC);
    write_parts(&mut encoded, key.parts());
    write_parts(&mut encoded, namespace.parts());
    write_string(&mut encoded, module.source_identity().normalized_path());
    write_parts(&mut encoded, item_signature.parts());
    encoded.extend_from_slice(&(payload.len() as u64).to_le_bytes());
    write_parts(&mut encoded, checksum.parts());
    encoded.extend_from_slice(&payload);
    Ok(encoded)
}

struct DecodedProviderSummary {
    key: [u64; 2],
    namespace: [u64; 2],
    module: String,
    item_signature: [u64; 2],
    payload: Vec<u8>,
}

fn decode_provider_summary(encoded: &[u8]) -> Option<DecodedProviderSummary> {
    let mut cursor = Cursor::new(encoded);
    let mut magic = [0; 8];
    cursor.read_exact(&mut magic).ok()?;
    (magic == *PROVIDER_SUMMARY_MAGIC).then_some(())?;
    let key = read_parts(&mut cursor)?;
    let namespace = read_parts(&mut cursor)?;
    let module = read_string(&mut cursor, encoded.len())?;
    let item_signature = read_parts(&mut cursor)?;
    let payload_len = read_len(&mut cursor, MAX_CACHE_PAYLOAD_BYTES)?;
    let checksum = read_parts(&mut cursor)?;
    let mut payload = vec![0; payload_len];
    cursor.read_exact(&mut payload).ok()?;
    (cursor.position() as usize == encoded.len()).then_some(())?;
    (payload_checksum(&payload).parts() == checksum).then_some(())?;
    Some(DecodedProviderSummary {
        key,
        namespace,
        module,
        item_signature,
        payload,
    })
}

fn encode_provider_summary_payload(
    summary: &ProviderSummary,
    symbols: &SymbolTable,
) -> io::Result<Vec<u8>> {
    let mut encoded = Vec::new();
    write_symbol_dictionary(&mut encoded, provider_summary_symbols(summary), symbols)?;
    encoded.extend_from_slice(&(summary.providers().len() as u64).to_le_bytes());
    for provider in summary.providers() {
        write_provider_type_ref(&mut encoded, &provider.target.ty);
        match &provider.trait_ref {
            Some(trait_ref) => {
                encoded.push(1);
                write_provider_type_ref(&mut encoded, trait_ref);
            }
            None => encoded.push(0),
        }
        write_symbols(&mut encoded, &provider.associated_methods);
        write_symbols(&mut encoded, &provider.associated_values);
    }
    Ok(encoded)
}

fn decode_provider_summary_payload(
    payload: &[u8],
    symbols: &SymbolTable,
) -> Option<ProviderSummary> {
    let mut cursor = Cursor::new(payload);
    let dictionary = read_symbol_dictionary(&mut cursor, payload.len())?;
    let provider_len = read_len(&mut cursor, MAX_CACHE_SEQUENCE_LEN)?;
    let mut providers = Vec::with_capacity(provider_len);
    for _ in 0..provider_len {
        let target = ProviderTarget {
            ty: read_provider_type_ref(&mut cursor)?,
        };
        let trait_ref = match read_u8(&mut cursor)? {
            0 => None,
            1 => Some(read_provider_type_ref(&mut cursor)?),
            _ => return None,
        };
        providers.push(Provider {
            target,
            trait_ref,
            associated_methods: read_symbols(&mut cursor)?,
            associated_values: read_symbols(&mut cursor)?,
        });
    }
    (cursor.position() as usize == payload.len()).then_some(())?;
    let summary = ProviderSummary::from_providers(providers);
    install_symbol_dictionary(&dictionary, provider_summary_symbols(&summary), symbols)?;
    Some(summary)
}

fn write_provider_type_ref(encoded: &mut Vec<u8>, ty: &ProviderTypeRef) {
    match ty.last_name {
        Some(symbol) => {
            encoded.push(1);
            encoded.extend_from_slice(&symbol.raw().to_le_bytes());
        }
        None => encoded.push(0),
    }
    encoded.push(u8::from(ty.is_generic_or_structural_target));
    encoded.push(u8::from(ty.semantic_is_conservative));
}

fn read_provider_type_ref(cursor: &mut Cursor<&[u8]>) -> Option<ProviderTypeRef> {
    let last_name = match read_u8(cursor)? {
        0 => None,
        1 => Some(SymbolId::from_stable_hash(read_u64(cursor)?)),
        _ => return None,
    };
    Some(ProviderTypeRef {
        last_name,
        is_generic_or_structural_target: read_bool(cursor)?,
        semantic_is_conservative: read_bool(cursor)?,
    })
}

fn provider_summary_symbols(summary: &ProviderSummary) -> BTreeSet<SymbolId> {
    let mut symbols = BTreeSet::new();
    for provider in summary.providers() {
        symbols.extend(provider.target.ty.last_name);
        symbols.extend(
            provider
                .trait_ref
                .as_ref()
                .and_then(|trait_ref| trait_ref.last_name),
        );
        symbols.extend(provider.associated_methods.iter().copied());
        symbols.extend(provider.associated_values.iter().copied());
    }
    symbols
}

fn facade_fact_symbols(facts: &ModuleFacadeFacts) -> BTreeSet<SymbolId> {
    let mut symbols = facts.public_type_names().collect::<BTreeSet<_>>();
    for reexport in facts.public_reexports() {
        symbols.extend(reexport.exposed_name);
        collect_used_module_path_symbols(&reexport.source, &mut symbols);
    }
    for path in facts.provider_source_paths() {
        collect_used_module_path_symbols(path, &mut symbols);
    }
    symbols
}

fn module_declaration_symbols(declarations: &ModuleDeclarations) -> BTreeSet<SymbolId> {
    let mut symbols = BTreeSet::new();
    symbols.extend(
        declarations
            .declarations
            .iter()
            .map(|declaration| declaration.name),
    );
    symbols.extend(declarations.package_roots.iter().copied());
    for path in &declarations.used_module_paths {
        collect_used_module_path_symbols(path, &mut symbols);
    }
    for import in &declarations.explicit_imports {
        symbols.insert(import.alias);
        collect_used_module_path_symbols(&import.path, &mut symbols);
    }
    symbols.extend(declarations.used_import_aliases.iter().copied());
    symbols
}

fn public_surface_fact_symbols(facts: &PublicSurfaceModuleFacts) -> io::Result<BTreeSet<SymbolId>> {
    let mut symbols = facts
        .defs
        .iter()
        .map(|def| def.name)
        .collect::<BTreeSet<_>>();
    for entries in [
        &facts.module_scope.modules,
        &facts.module_scope.types,
        &facts.module_scope.values,
    ] {
        symbols.extend(entries.iter().map(|(name, _)| *name));
    }
    for scope in &facts.enum_scopes {
        symbols.extend(scope.variants.iter().map(|(name, _)| *name));
    }
    for using in &facts.module_usings {
        collect_using_path_segment_symbols(&using.host, &mut symbols);
        collect_using_selector_symbols(&using.selector, &mut symbols, 0)?;
    }
    Ok(symbols)
}

fn collect_using_path_segment_symbols(
    segments: &[UsingPathSegment],
    symbols: &mut BTreeSet<SymbolId>,
) {
    symbols.extend(segments.iter().filter_map(|segment| match segment.kind {
        PathSegmentKind::Name(name) => Some(name),
        PathSegmentKind::Package | PathSegmentKind::Super | PathSegmentKind::SelfValue => None,
    }));
}

fn collect_using_selector_symbols(
    selector: &UsingSelector,
    symbols: &mut BTreeSet<SymbolId>,
    depth: usize,
) -> io::Result<()> {
    if depth > MAX_USING_SELECTOR_DEPTH {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            "public surface using selector is too deeply nested",
        ));
    }
    match selector {
        UsingSelector::Single(name) => collect_using_name_symbols(name, symbols)?,
        UsingSelector::Group(items) => {
            for item in items {
                match item {
                    UsingGroupItem::Name(name) => collect_using_name_symbols(name, symbols)?,
                    UsingGroupItem::Nested { host, selector } => {
                        collect_using_path_segment_symbols(host, symbols);
                        collect_using_selector_symbols(selector, symbols, depth + 1)?;
                    }
                }
            }
        }
        UsingSelector::Wildcard { .. } | UsingSelector::SelfName => {}
    }
    Ok(())
}

fn collect_using_name_symbols(
    name: &UsingName,
    symbols: &mut BTreeSet<SymbolId>,
) -> io::Result<()> {
    symbols.insert(name.name);
    match (name.alias, name.alias_span) {
        (Some(alias), Some(_)) => {
            symbols.insert(alias);
        }
        (None, None) => {}
        _ => {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                "public surface using alias and span disagree",
            ));
        }
    }
    Ok(())
}

fn collect_used_module_path_symbols(path: &UsedModulePath, symbols: &mut BTreeSet<SymbolId>) {
    let (package, segments, processing) = match path {
        UsedModulePath::Package {
            package,
            segments,
            processing,
            ..
        } => (Some(*package), segments, processing),
        UsedModulePath::PackageRelative {
            segments,
            processing,
            ..
        }
        | UsedModulePath::ParentRelative {
            segments,
            processing,
            ..
        }
        | UsedModulePath::Local {
            segments,
            processing,
            ..
        } => (None, segments, processing),
    };
    symbols.extend(package);
    symbols.extend(segments.iter().copied());
    match processing {
        UsedModulePathProcessing::Never
        | UsedModulePathProcessing::Always
        | UsedModulePathProcessing::IfSelectedItem
        | UsedModulePathProcessing::IfProvidesExtensions => {}
        UsedModulePathProcessing::IfProvidesTraitImpl { trait_name }
        | UsedModulePathProcessing::IfProvidesImplicitTraitImpl { trait_name } => {
            symbols.insert(*trait_name);
        }
        UsedModulePathProcessing::IfProvidesTraitMethod {
            target_type_name,
            associated_name,
        } => {
            symbols.extend(*target_type_name);
            symbols.insert(*associated_name);
        }
    }
}

fn write_symbol_dictionary(
    encoded: &mut Vec<u8>,
    symbols: BTreeSet<SymbolId>,
    table: &SymbolTable,
) -> io::Result<()> {
    encoded.extend_from_slice(&(symbols.len() as u64).to_le_bytes());
    for symbol in symbols {
        let text = table.resolve(symbol).ok_or_else(|| {
            io::Error::new(
                io::ErrorKind::InvalidInput,
                format!("cannot persist unresolved symbol {:#018x}", symbol.raw()),
            )
        })?;
        encoded.extend_from_slice(&symbol.raw().to_le_bytes());
        write_string(encoded, &text);
    }
    Ok(())
}

fn read_symbol_dictionary(
    cursor: &mut Cursor<&[u8]>,
    payload_len: usize,
) -> Option<Vec<(SymbolId, String)>> {
    let len = read_len(cursor, MAX_CACHE_SEQUENCE_LEN)?;
    let mut dictionary = Vec::with_capacity(len);
    for _ in 0..len {
        let symbol = SymbolId::from_stable_hash(read_u64(cursor)?);
        let text = read_string(cursor, payload_len)?;
        (symbol == SymbolId::from_stable_hash(stable_hash(&text))).then_some(())?;
        dictionary.push((symbol, text));
    }
    dictionary
        .windows(2)
        .all(|pair| pair[0].0 < pair[1].0)
        .then_some(dictionary)
}

fn install_symbol_dictionary(
    dictionary: &[(SymbolId, String)],
    expected: BTreeSet<SymbolId>,
    symbols: &SymbolTable,
) -> Option<()> {
    dictionary
        .iter()
        .map(|(symbol, _)| *symbol)
        .eq(expected)
        .then_some(())?;
    for (symbol, text) in dictionary {
        (symbols.intern(text).ok()? == *symbol).then_some(())?;
    }
    Some(())
}

fn write_symbols(encoded: &mut Vec<u8>, symbols: &[SymbolId]) {
    encoded.extend_from_slice(&(symbols.len() as u64).to_le_bytes());
    for symbol in symbols {
        encoded.extend_from_slice(&symbol.raw().to_le_bytes());
    }
}

fn read_symbols(cursor: &mut Cursor<&[u8]>) -> Option<Vec<SymbolId>> {
    let len = read_len(cursor, MAX_CACHE_SEQUENCE_LEN)?;
    let mut symbols = Vec::with_capacity(len);
    for _ in 0..len {
        symbols.push(SymbolId::from_stable_hash(read_u64(cursor)?));
    }
    Some(symbols)
}

fn write_optional_symbol(encoded: &mut Vec<u8>, symbol: Option<SymbolId>) {
    match symbol {
        Some(symbol) => {
            encoded.push(1);
            encoded.extend_from_slice(&symbol.raw().to_le_bytes());
        }
        None => encoded.push(0),
    }
}

fn read_optional_symbol(cursor: &mut Cursor<&[u8]>) -> Option<Option<SymbolId>> {
    match read_u8(cursor)? {
        0 => Some(None),
        1 => Some(Some(read_symbol(cursor)?)),
        _ => None,
    }
}

fn read_symbol(cursor: &mut Cursor<&[u8]>) -> Option<SymbolId> {
    Some(SymbolId::from_stable_hash(read_u64(cursor)?))
}

fn is_strictly_sorted<T: Ord>(values: &[T]) -> bool {
    values.windows(2).all(|pair| pair[0] < pair[1])
}

fn payload_checksum(payload: &[u8]) -> QueryFingerprint {
    let mut builder = QueryFingerprintBuilder::new("nia.frontend.provider-summary-payload.v1");
    builder.write_bytes(payload);
    builder.finish()
}

fn dependency_manifest_checksum(payload: &[u8]) -> QueryFingerprint {
    let mut builder = QueryFingerprintBuilder::new("nia.frontend.dependency-manifest-payload.v1");
    builder.write_bytes(payload);
    builder.finish()
}

fn facade_facts_checksum(payload: &[u8]) -> QueryFingerprint {
    let mut builder = QueryFingerprintBuilder::new("nia.frontend.facade-facts-payload.v1");
    builder.write_bytes(payload);
    builder.finish()
}

fn module_dependencies_checksum(payload: &[u8]) -> QueryFingerprint {
    let mut builder = QueryFingerprintBuilder::new("nia.frontend.module-dependencies-payload.v1");
    builder.write_bytes(payload);
    builder.finish()
}

fn public_surface_facts_checksum(payload: &[u8]) -> QueryFingerprint {
    let mut builder = QueryFingerprintBuilder::new("nia.frontend.public-surface-facts-payload.v1");
    builder.write_bytes(payload);
    builder.finish()
}

fn parts_bytes(parts: [u64; 2]) -> [u8; 16] {
    let mut bytes = [0; 16];
    bytes[..8].copy_from_slice(&parts[0].to_le_bytes());
    bytes[8..].copy_from_slice(&parts[1].to_le_bytes());
    bytes
}

fn write_parts(encoded: &mut Vec<u8>, parts: [u64; 2]) {
    for part in parts {
        encoded.extend_from_slice(&part.to_le_bytes());
    }
}

fn read_parts(cursor: &mut Cursor<&[u8]>) -> Option<[u64; 2]> {
    Some([read_u64(cursor)?, read_u64(cursor)?])
}

fn write_string(encoded: &mut Vec<u8>, value: &str) {
    encoded.extend_from_slice(&(value.len() as u64).to_le_bytes());
    encoded.extend_from_slice(value.as_bytes());
}

fn read_string(cursor: &mut Cursor<&[u8]>, encoded_len: usize) -> Option<String> {
    let len = read_len(cursor, encoded_len)?;
    let mut bytes = vec![0; len];
    cursor.read_exact(&mut bytes).ok()?;
    String::from_utf8(bytes).ok()
}

fn read_len(cursor: &mut Cursor<&[u8]>, limit: usize) -> Option<usize> {
    let len = usize::try_from(read_u64(cursor)?).ok()?;
    (len <= limit).then_some(len)
}

fn read_bool(cursor: &mut Cursor<&[u8]>) -> Option<bool> {
    match read_u8(cursor)? {
        0 => Some(false),
        1 => Some(true),
        _ => None,
    }
}

fn read_u8(cursor: &mut Cursor<&[u8]>) -> Option<u8> {
    let mut bytes = [0; 1];
    cursor.read_exact(&mut bytes).ok()?;
    Some(bytes[0])
}

fn read_u64(cursor: &mut Cursor<&[u8]>) -> Option<u64> {
    let mut bytes = [0; 8];
    cursor.read_exact(&mut bytes).ok()?;
    Some(u64::from_le_bytes(bytes))
}
