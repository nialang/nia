// SPDX-License-Identifier: GPL-3.0-or-later
use super::*;

impl PersistentSignatureCache {
    pub(crate) fn new(root: PathBuf) -> Self {
        Self { root }
    }

    pub(crate) fn root(&self) -> &Path {
        &self.root
    }

    pub(crate) fn load_type_resolution(
        &self,
        identity: SignatureTypeResolutionIdentity<'_>,
        modules: &HashMap<String, ModuleId>,
        symbols: &SymbolTable,
        node_store: &nia_node_id::NodeStore,
    ) -> io::Result<SignatureTypeResolutionLookup> {
        let path = self.type_resolution_path(identity.key);
        let encoded = match read_signature_cache_entry(&path)? {
            SignatureCacheEntryRead::Bytes(encoded) => encoded,
            SignatureCacheEntryRead::Oversized => {
                retire_oversized(&path);
                return Ok(SignatureTypeResolutionLookup::Corrupt);
            }
            SignatureCacheEntryRead::NotFound => {
                return Ok(SignatureTypeResolutionLookup::NotFound);
            }
        };
        let Some(payload) = decode_entry(&encoded, identity) else {
            retire_corrupt(&path, &encoded);
            return Ok(SignatureTypeResolutionLookup::Corrupt);
        };
        let Some(resolution) = decode_type_resolution(
            payload,
            identity.source_version,
            identity.source_len,
            modules,
            symbols,
            node_store,
        ) else {
            retire_corrupt(&path, &encoded);
            return Ok(SignatureTypeResolutionLookup::Corrupt);
        };
        Ok(SignatureTypeResolutionLookup::Hit(Box::new(resolution)))
    }

    pub(crate) fn publish_type_resolution(
        &self,
        identity: SignatureTypeResolutionIdentity<'_>,
        resolution: &TypeResolution,
        module_paths: &HashMap<ModuleId, String>,
        symbols: &SymbolTable,
        replace: bool,
    ) -> io::Result<()> {
        if !resolution.diagnostics.is_empty() {
            return Ok(());
        }
        let path = self.type_resolution_path(identity.key);
        if !replace && path.is_file() {
            return Ok(());
        }
        let payload =
            encode_type_resolution(resolution, identity.source_version, module_paths, symbols)?;
        let encoded = encode_entry(identity, &payload);
        let parent = path
            .parent()
            .ok_or_else(|| io::Error::other("invalid signature cache path"))?;
        fs::create_dir_all(parent)?;
        atomic_publish(&path, &encoded, replace)
    }

    pub(crate) fn remove_type_resolution(&self, key: FrontendSignatureTypeResolutionCacheKey) {
        remove_cache_entry(&self.type_resolution_path(key));
    }

    pub(crate) fn load_type_lowering(
        &self,
        identity: SignatureTypeLoweringIdentity<'_>,
        modules: &HashMap<String, ModuleId>,
        symbols: &SymbolTable,
        type_store: &TypeStore,
    ) -> io::Result<SignatureTypeLoweringLookup> {
        let path = self.type_lowering_path(identity.key);
        let encoded = match read_signature_cache_entry(&path)? {
            SignatureCacheEntryRead::Bytes(encoded) => encoded,
            SignatureCacheEntryRead::Oversized => {
                retire_oversized(&path);
                return Ok(SignatureTypeLoweringLookup::Corrupt);
            }
            SignatureCacheEntryRead::NotFound => {
                return Ok(SignatureTypeLoweringLookup::NotFound);
            }
        };
        let Some(payload) = decode_type_lowering_entry(&encoded, identity) else {
            retire_corrupt(&path, &encoded);
            return Ok(SignatureTypeLoweringLookup::Corrupt);
        };
        let Some(module_id) = modules
            .get(identity.module.source_identity().normalized_path())
            .copied()
        else {
            retire_corrupt(&path, &encoded);
            return Ok(SignatureTypeLoweringLookup::Corrupt);
        };
        let Some(lowering) = decode_type_lowering(
            payload,
            identity.source_version,
            identity.source_len,
            modules,
            symbols,
            type_store,
            module_id,
        ) else {
            retire_corrupt(&path, &encoded);
            return Ok(SignatureTypeLoweringLookup::Corrupt);
        };
        Ok(SignatureTypeLoweringLookup::Hit(Box::new(lowering)))
    }

    pub(crate) fn publish_type_lowering(
        &self,
        identity: SignatureTypeLoweringIdentity<'_>,
        lowering: &TypeLowering,
        module_paths: &HashMap<ModuleId, String>,
        symbols: &SymbolTable,
        type_store: &TypeStore,
        replace: bool,
    ) -> io::Result<()> {
        if !lowering.diagnostics.is_empty()
            || !lowering.const_exprs.is_empty()
            || !lowering.const_expr_summaries.is_empty()
        {
            return Ok(());
        }
        let path = self.type_lowering_path(identity.key);
        if !replace && path.is_file() {
            return Ok(());
        }
        let payload = encode_type_lowering(
            lowering,
            identity.source_version,
            module_paths,
            symbols,
            type_store,
        )?;
        let encoded = encode_type_lowering_entry(identity, &payload);
        let parent = path
            .parent()
            .ok_or_else(|| io::Error::other("invalid signature cache path"))?;
        fs::create_dir_all(parent)?;
        atomic_publish(&path, &encoded, replace)
    }

    pub(crate) fn remove_type_lowering(&self, key: FrontendSignatureTypeLoweringCacheKey) {
        remove_cache_entry(&self.type_lowering_path(key));
    }

    pub(crate) fn load_item_signatures(
        &self,
        identity: SignatureItemSignaturesIdentity<'_>,
        modules: &HashMap<String, ModuleId>,
        symbols: &SymbolTable,
        type_store: &TypeStore,
    ) -> io::Result<SignatureItemSignaturesLookup> {
        let path = self.item_signatures_path(identity.key);
        let encoded = match read_signature_cache_entry(&path)? {
            SignatureCacheEntryRead::Bytes(encoded) => encoded,
            SignatureCacheEntryRead::Oversized => {
                retire_oversized(&path);
                return Ok(SignatureItemSignaturesLookup::Corrupt);
            }
            SignatureCacheEntryRead::NotFound => {
                return Ok(SignatureItemSignaturesLookup::NotFound);
            }
        };
        let Some(payload) = decode_item_signatures_entry(&encoded, identity) else {
            retire_corrupt(&path, &encoded);
            return Ok(SignatureItemSignaturesLookup::Corrupt);
        };
        let Some(module_id) = modules
            .get(identity.module.source_identity().normalized_path())
            .copied()
        else {
            retire_corrupt(&path, &encoded);
            return Ok(SignatureItemSignaturesLookup::Corrupt);
        };
        let Some(signatures) = decode_item_signatures(
            payload,
            identity.source_len,
            modules,
            symbols,
            type_store,
            module_id,
        ) else {
            retire_corrupt(&path, &encoded);
            return Ok(SignatureItemSignaturesLookup::Corrupt);
        };
        Ok(SignatureItemSignaturesLookup::Hit(Box::new(signatures)))
    }

    pub(crate) fn publish_item_signatures(
        &self,
        identity: SignatureItemSignaturesIdentity<'_>,
        signatures: &ItemSignatures,
        module_paths: &HashMap<ModuleId, String>,
        symbols: &SymbolTable,
        type_store: &TypeStore,
        replace: bool,
    ) -> io::Result<()> {
        if !signatures.diagnostics.is_empty() {
            return Ok(());
        }
        let path = self.item_signatures_path(identity.key);
        if !replace && path.is_file() {
            return Ok(());
        }
        let payload = encode_item_signatures(signatures, module_paths, symbols, type_store)?;
        let encoded = encode_item_signatures_entry(identity, &payload);
        let parent = path
            .parent()
            .ok_or_else(|| io::Error::other("invalid signature cache path"))?;
        fs::create_dir_all(parent)?;
        atomic_publish(&path, &encoded, replace)
    }

    pub(crate) fn remove_item_signatures(&self, key: FrontendSignatureItemSignaturesCacheKey) {
        remove_cache_entry(&self.item_signatures_path(key));
    }

    pub(crate) fn load_extension_validation_diagnostics(
        &self,
        identity: ExtensionValidationDiagnosticsIdentity<'_>,
    ) -> io::Result<ExtensionValidationDiagnosticsLookup> {
        let path = self.extension_validation_diagnostics_path(identity.key);
        let encoded = match read_signature_cache_entry(&path)? {
            SignatureCacheEntryRead::Bytes(encoded) => encoded,
            SignatureCacheEntryRead::Oversized => {
                retire_oversized(&path);
                return Ok(ExtensionValidationDiagnosticsLookup::Corrupt);
            }
            SignatureCacheEntryRead::NotFound => {
                return Ok(ExtensionValidationDiagnosticsLookup::NotFound);
            }
        };
        let Some(payload) = decode_extension_validation_diagnostics_entry(&encoded, identity)
        else {
            retire_corrupt(&path, &encoded);
            return Ok(ExtensionValidationDiagnosticsLookup::Corrupt);
        };
        let Some(diagnostics) = decode_stable_diagnostic_bundle(payload, identity.source_len)
        else {
            retire_corrupt(&path, &encoded);
            return Ok(ExtensionValidationDiagnosticsLookup::Corrupt);
        };
        Ok(ExtensionValidationDiagnosticsLookup::Hit(diagnostics))
    }

    pub(crate) fn publish_extension_validation_diagnostics(
        &self,
        identity: ExtensionValidationDiagnosticsIdentity<'_>,
        diagnostics: &[Diagnostic],
        replace: bool,
    ) -> io::Result<()> {
        let path = self.extension_validation_diagnostics_path(identity.key);
        if !replace && path.is_file() {
            return Ok(());
        }
        let payload = encode_stable_diagnostic_bundle(diagnostics, identity.source_len)
            .map_err(|error| io::Error::new(io::ErrorKind::InvalidInput, error))?;
        let encoded = encode_extension_validation_diagnostics_entry(identity, &payload);
        let parent = path
            .parent()
            .ok_or_else(|| io::Error::other("invalid signature cache path"))?;
        fs::create_dir_all(parent)?;
        atomic_publish(&path, &encoded, replace)
    }

    pub(crate) fn remove_extension_validation_diagnostics(
        &self,
        key: FrontendExtensionValidationDiagnosticsCacheKey,
    ) {
        remove_cache_entry(&self.extension_validation_diagnostics_path(key));
    }

    pub(crate) fn load_executable_value_ref_edges(
        &self,
        identity: ExecutableValueRefEdgesIdentity<'_>,
        modules: &HashMap<String, ModuleId>,
    ) -> io::Result<ExecutableValueRefEdgesLookup> {
        let path = self.executable_value_ref_edges_path(identity.key);
        let encoded = match read_signature_cache_entry(&path)? {
            SignatureCacheEntryRead::Bytes(encoded) => encoded,
            SignatureCacheEntryRead::Oversized => {
                retire_oversized(&path);
                return Ok(ExecutableValueRefEdgesLookup::Corrupt);
            }
            SignatureCacheEntryRead::NotFound => {
                return Ok(ExecutableValueRefEdgesLookup::NotFound);
            }
        };
        let Some(payload) = decode_executable_value_ref_edges_entry(&encoded, identity) else {
            retire_corrupt(&path, &encoded);
            return Ok(ExecutableValueRefEdgesLookup::Corrupt);
        };
        let Some(edges) = decode_executable_value_ref_edges(payload, modules) else {
            retire_corrupt(&path, &encoded);
            return Ok(ExecutableValueRefEdgesLookup::Corrupt);
        };
        Ok(ExecutableValueRefEdgesLookup::Hit(edges))
    }

    pub(crate) fn publish_executable_value_ref_edges(
        &self,
        identity: ExecutableValueRefEdgesIdentity<'_>,
        edges: &CachedExecutableValueRefEdges,
        module_paths: &HashMap<ModuleId, String>,
        replace: bool,
    ) -> io::Result<()> {
        let path = self.executable_value_ref_edges_path(identity.key);
        if !replace && path.is_file() {
            return Ok(());
        }
        let payload = encode_executable_value_ref_edges(edges, module_paths)?;
        let encoded = encode_executable_value_ref_edges_entry(identity, &payload);
        let parent = path
            .parent()
            .ok_or_else(|| io::Error::other("invalid signature cache path"))?;
        fs::create_dir_all(parent)?;
        atomic_publish(&path, &encoded, replace)
    }

    pub(crate) fn remove_executable_value_ref_edges(
        &self,
        key: FrontendExecutableValueRefEdgesCacheKey,
    ) {
        remove_cache_entry(&self.executable_value_ref_edges_path(key));
    }

    pub(crate) fn load_check_certificate(
        &self,
        identity: CheckCertificateIdentity<'_>,
    ) -> io::Result<CheckCertificateLookup> {
        let path = self.check_certificate_path(identity.key);
        let encoded = match read_signature_cache_entry(&path)? {
            SignatureCacheEntryRead::Bytes(encoded) => encoded,
            SignatureCacheEntryRead::Oversized => {
                retire_oversized(&path);
                return Ok(CheckCertificateLookup::Corrupt);
            }
            SignatureCacheEntryRead::NotFound => {
                return Ok(CheckCertificateLookup::NotFound);
            }
        };
        let Some(certificate) = decode_check_certificate(&encoded, identity) else {
            retire_corrupt(&path, &encoded);
            return Ok(CheckCertificateLookup::Corrupt);
        };
        Ok(CheckCertificateLookup::Hit(certificate))
    }

    pub(crate) fn publish_check_certificate(
        &self,
        identity: CheckCertificateIdentity<'_>,
        certificate: CachedCheckCertificate,
        replace: bool,
    ) -> io::Result<()> {
        let path = self.check_certificate_path(identity.key);
        if !replace && path.is_file() {
            return Ok(());
        }
        let encoded = match encode_check_certificate(identity, &certificate) {
            Ok(encoded) => encoded,
            Err(error) => {
                if replace {
                    remove_cache_entry(&path);
                }
                return Err(error);
            }
        };
        let parent = path
            .parent()
            .ok_or_else(|| io::Error::other("invalid signature cache path"))?;
        fs::create_dir_all(parent)?;
        atomic_publish(&path, &encoded, replace)
    }

    pub(crate) fn remove_check_certificate(&self, key: FrontendCheckCertificateCacheKey) {
        remove_cache_entry(&self.check_certificate_path(key));
    }

    pub(crate) fn type_resolution_path(
        &self,
        key: FrontendSignatureTypeResolutionCacheKey,
    ) -> PathBuf {
        let [first, second] = key.parts();
        self.root
            .join("artifacts")
            .join("frontend")
            .join("v3")
            .join("signature-type-resolutions")
            .join(format!("{first:016x}{second:016x}.str"))
    }

    pub(crate) fn type_lowering_path(&self, key: FrontendSignatureTypeLoweringCacheKey) -> PathBuf {
        let [first, second] = key.parts();
        self.root
            .join("artifacts")
            .join("frontend")
            .join("v3")
            .join("signature-type-lowerings")
            .join(format!("{first:016x}{second:016x}.stl"))
    }

    pub(crate) fn item_signatures_path(
        &self,
        key: FrontendSignatureItemSignaturesCacheKey,
    ) -> PathBuf {
        let [first, second] = key.parts();
        self.root
            .join("artifacts")
            .join("frontend")
            .join("v3")
            .join("signature-item-signatures")
            .join(format!("{first:016x}{second:016x}.sis"))
    }

    pub(crate) fn extension_validation_diagnostics_path(
        &self,
        key: FrontendExtensionValidationDiagnosticsCacheKey,
    ) -> PathBuf {
        let [first, second] = key.parts();
        self.root
            .join("artifacts")
            .join("frontend")
            .join("v3")
            .join("extension-validation-diagnostics")
            .join(format!("{first:016x}{second:016x}.evd"))
    }

    pub(crate) fn executable_value_ref_edges_path(
        &self,
        key: FrontendExecutableValueRefEdgesCacheKey,
    ) -> PathBuf {
        let [first, second] = key.parts();
        self.root
            .join("artifacts")
            .join("frontend")
            .join("v3")
            .join("executable-value-ref-edges")
            .join(format!("{first:016x}{second:016x}.erv"))
    }

    pub(crate) fn check_certificate_path(&self, key: FrontendCheckCertificateCacheKey) -> PathBuf {
        let [first, second] = key.parts();
        self.root
            .join("artifacts")
            .join("frontend")
            .join("v3")
            .join("check-certificates")
            .join(format!("{first:016x}{second:016x}.ccc"))
    }
}

fn atomic_publish(path: &Path, encoded: &[u8], replace: bool) -> io::Result<()> {
    validate_entry_size(encoded.len())?;
    let stage_id = STAGE_ID.fetch_add(1, Ordering::Relaxed);
    let staged = path.with_extension(format!("tmp-{}-{stage_id}", std::process::id()));
    if let Err(error) = write_staged_file(&staged, encoded) {
        let _ = fs::remove_file(&staged);
        return Err(error);
    }
    let result = (|| {
        let _lock = SignatureCacheMutationLock::acquire(path)?;
        if !replace && path.is_file() {
            return Ok(());
        }
        fs::rename(&staged, path)?;
        if let Some(parent) = path.parent()
            && let Ok(directory) = File::open(parent)
        {
            let _ = directory.sync_all();
        }
        Ok(())
    })();
    if result.is_err() || staged.exists() {
        let _ = fs::remove_file(&staged);
    }
    result
}

fn write_staged_file(staged: &Path, encoded: &[u8]) -> io::Result<()> {
    let mut file = OpenOptions::new()
        .create_new(true)
        .write(true)
        .open(staged)?;
    file.write_all(encoded).and_then(|()| file.sync_all())?;
    Ok(())
}

struct SignatureCacheMutationLock {
    _file: File,
}

impl SignatureCacheMutationLock {
    fn acquire(path: &Path) -> io::Result<Self> {
        let lock_path = path.with_extension("lock");
        let file = OpenOptions::new()
            .read(true)
            .write(true)
            .create(true)
            .truncate(false)
            .open(lock_path)?;
        file.lock()?;
        Ok(Self { _file: file })
    }
}

enum SignatureCacheEntryRead {
    NotFound,
    Oversized,
    Bytes(Vec<u8>),
}

/// Enforces the decoder's entry budget on the opened stream. The metadata
/// check avoids reading known oversized files, while `max + 1` also catches a
/// file that grows after metadata was observed without an unbounded allocation.
fn read_signature_cache_entry(path: &Path) -> io::Result<SignatureCacheEntryRead> {
    let file = match File::open(path) {
        Ok(file) => file,
        Err(error) if error.kind() == io::ErrorKind::NotFound => {
            return Ok(SignatureCacheEntryRead::NotFound);
        }
        Err(error) => return Err(error),
    };
    let metadata_len = file.metadata()?.len();
    let max_bytes = u64::try_from(MAX_ENTRY_BYTES).unwrap_or(u64::MAX);
    if metadata_len > max_bytes {
        return Ok(SignatureCacheEntryRead::Oversized);
    }
    let mut encoded = Vec::with_capacity(usize::try_from(metadata_len).unwrap_or(0));
    file.take(max_bytes.saturating_add(1))
        .read_to_end(&mut encoded)?;
    if encoded.len() > MAX_ENTRY_BYTES {
        Ok(SignatureCacheEntryRead::Oversized)
    } else {
        Ok(SignatureCacheEntryRead::Bytes(encoded))
    }
}

fn validate_entry_size(len: usize) -> io::Result<()> {
    if len > MAX_ENTRY_BYTES {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            "signature cache entry exceeds its size limit",
        ));
    }
    Ok(())
}

/// Deletes a corrupt observation only if the same bytes are still installed
/// after waiting for the per-key mutation lock. Publishers use the same lock,
/// so a replacement that won during decoding cannot be retired as stale.
fn retire_corrupt(path: &Path, observed: &[u8]) {
    let Ok(_lock) = SignatureCacheMutationLock::acquire(path) else {
        return;
    };
    if matches!(
        read_signature_cache_entry(path),
        Ok(SignatureCacheEntryRead::Bytes(current)) if current == observed
    ) {
        let _ = fs::remove_file(path);
    }
}

fn retire_oversized(path: &Path) {
    let Ok(_lock) = SignatureCacheMutationLock::acquire(path) else {
        return;
    };
    if matches!(
        read_signature_cache_entry(path),
        Ok(SignatureCacheEntryRead::Oversized)
    ) {
        let _ = fs::remove_file(path);
    }
}

fn remove_cache_entry(path: &Path) {
    let Ok(_lock) = SignatureCacheMutationLock::acquire(path) else {
        return;
    };
    let _ = fs::remove_file(path);
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicU64, Ordering};

    static TEST_SEQUENCE: AtomicU64 = AtomicU64::new(0);

    fn test_root(name: &str) -> PathBuf {
        std::env::temp_dir().join(format!(
            "nia-signature-cache-{name}-{}-{}",
            std::process::id(),
            TEST_SEQUENCE.fetch_add(1, Ordering::Relaxed)
        ))
    }

    #[test]
    fn entry_size_limit_is_shared_by_all_publishers() {
        validate_entry_size(MAX_ENTRY_BYTES).expect("limit itself is valid");
        let error = validate_entry_size(MAX_ENTRY_BYTES + 1).expect_err("oversized entry");
        assert_eq!(error.kind(), io::ErrorKind::InvalidInput);
    }

    #[test]
    fn oversized_entry_is_rejected_from_metadata_without_full_read() {
        let path = test_root("oversized");
        {
            let file = File::create(&path).expect("create sparse cache entry");
            file.set_len((MAX_ENTRY_BYTES + 1) as u64)
                .expect("extend sparse cache entry");
        }

        assert!(matches!(
            read_signature_cache_entry(&path).expect("read bounded cache entry"),
            SignatureCacheEntryRead::Oversized
        ));

        fs::remove_file(path).expect("remove sparse cache entry");
    }

    #[test]
    fn stale_corruption_retirement_preserves_replacement() {
        let root = test_root("stale-retirement");
        fs::create_dir_all(&root).expect("create cache root");
        let path = root.join("entry.bin");
        fs::write(&path, b"corrupt").expect("write corrupt entry");
        let SignatureCacheEntryRead::Bytes(observed) =
            read_signature_cache_entry(&path).expect("observe corrupt entry")
        else {
            panic!("expected observed cache bytes");
        };
        fs::write(&path, b"replacement").expect("publish replacement");

        retire_corrupt(&path, &observed);

        assert_eq!(fs::read(&path).expect("read replacement"), b"replacement");
        fs::remove_dir_all(root).expect("remove cache root");
    }

    #[test]
    fn oversized_retirement_preserves_bounded_replacement() {
        let root = test_root("oversized-retirement");
        fs::create_dir_all(&root).expect("create cache root");
        let path = root.join("entry.bin");
        {
            let file = File::create(&path).expect("create oversized entry");
            file.set_len((MAX_ENTRY_BYTES + 1) as u64)
                .expect("extend oversized entry");
        }
        assert!(matches!(
            read_signature_cache_entry(&path).expect("observe oversized entry"),
            SignatureCacheEntryRead::Oversized
        ));
        fs::write(&path, b"replacement").expect("publish replacement");

        retire_oversized(&path);

        assert_eq!(fs::read(&path).expect("read replacement"), b"replacement");
        fs::remove_dir_all(root).expect("remove cache root");
    }

    #[test]
    fn nonreplacing_publication_rechecks_under_mutation_lock() {
        let root = test_root("nonreplacing-publication");
        fs::create_dir_all(&root).expect("create cache root");
        let path = root.join("entry.bin");
        fs::write(&path, b"winner").expect("write winning entry");

        atomic_publish(&path, b"loser", false).expect("publish without replacement");

        assert_eq!(fs::read(&path).expect("read winning entry"), b"winner");
        fs::remove_dir_all(root).expect("remove cache root");
    }
}
