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
        let encoded = match fs::read(&path) {
            Ok(encoded) if encoded.len() <= MAX_ENTRY_BYTES => encoded,
            Ok(_) => {
                remove_corrupt(&path);
                return Ok(SignatureTypeResolutionLookup::Corrupt);
            }
            Err(error) if error.kind() == io::ErrorKind::NotFound => {
                return Ok(SignatureTypeResolutionLookup::NotFound);
            }
            Err(error) => return Err(error),
        };
        let Some(payload) = decode_entry(&encoded, identity) else {
            remove_corrupt(&path);
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
            remove_corrupt(&path);
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
        atomic_publish(&path, &encoded)
    }

    pub(crate) fn remove_type_resolution(&self, key: FrontendSignatureTypeResolutionCacheKey) {
        remove_corrupt(&self.type_resolution_path(key));
    }

    pub(crate) fn load_type_lowering(
        &self,
        identity: SignatureTypeLoweringIdentity<'_>,
        modules: &HashMap<String, ModuleId>,
        symbols: &SymbolTable,
        type_store: &TypeStore,
    ) -> io::Result<SignatureTypeLoweringLookup> {
        let path = self.type_lowering_path(identity.key);
        let encoded = match fs::read(&path) {
            Ok(encoded) if encoded.len() <= MAX_ENTRY_BYTES => encoded,
            Ok(_) => {
                remove_corrupt(&path);
                return Ok(SignatureTypeLoweringLookup::Corrupt);
            }
            Err(error) if error.kind() == io::ErrorKind::NotFound => {
                return Ok(SignatureTypeLoweringLookup::NotFound);
            }
            Err(error) => return Err(error),
        };
        let Some(payload) = decode_type_lowering_entry(&encoded, identity) else {
            remove_corrupt(&path);
            return Ok(SignatureTypeLoweringLookup::Corrupt);
        };
        let Some(module_id) = modules
            .get(identity.module.source_identity().normalized_path())
            .copied()
        else {
            remove_corrupt(&path);
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
            remove_corrupt(&path);
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
        atomic_publish(&path, &encoded)
    }

    pub(crate) fn remove_type_lowering(&self, key: FrontendSignatureTypeLoweringCacheKey) {
        remove_corrupt(&self.type_lowering_path(key));
    }

    pub(crate) fn load_item_signatures(
        &self,
        identity: SignatureItemSignaturesIdentity<'_>,
        modules: &HashMap<String, ModuleId>,
        symbols: &SymbolTable,
        type_store: &TypeStore,
    ) -> io::Result<SignatureItemSignaturesLookup> {
        let path = self.item_signatures_path(identity.key);
        let encoded = match fs::read(&path) {
            Ok(encoded) if encoded.len() <= MAX_ENTRY_BYTES => encoded,
            Ok(_) => {
                remove_corrupt(&path);
                return Ok(SignatureItemSignaturesLookup::Corrupt);
            }
            Err(error) if error.kind() == io::ErrorKind::NotFound => {
                return Ok(SignatureItemSignaturesLookup::NotFound);
            }
            Err(error) => return Err(error),
        };
        let Some(payload) = decode_item_signatures_entry(&encoded, identity) else {
            remove_corrupt(&path);
            return Ok(SignatureItemSignaturesLookup::Corrupt);
        };
        let Some(module_id) = modules
            .get(identity.module.source_identity().normalized_path())
            .copied()
        else {
            remove_corrupt(&path);
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
            remove_corrupt(&path);
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
        atomic_publish(&path, &encoded)
    }

    pub(crate) fn remove_item_signatures(&self, key: FrontendSignatureItemSignaturesCacheKey) {
        remove_corrupt(&self.item_signatures_path(key));
    }

    pub(crate) fn load_extension_trait_solving_facts(
        &self,
        identity: ExtensionTraitSolvingFactsIdentity<'_>,
        modules: &HashMap<String, ModuleId>,
        symbols: &SymbolTable,
        type_store: &TypeStore,
    ) -> io::Result<ExtensionTraitSolvingFactsLookup> {
        let path = self.extension_trait_solving_facts_path(identity.key);
        let encoded = match fs::read(&path) {
            Ok(encoded) if encoded.len() <= MAX_ENTRY_BYTES => encoded,
            Ok(_) => {
                remove_corrupt(&path);
                return Ok(ExtensionTraitSolvingFactsLookup::Corrupt);
            }
            Err(error) if error.kind() == io::ErrorKind::NotFound => {
                return Ok(ExtensionTraitSolvingFactsLookup::NotFound);
            }
            Err(error) => return Err(error),
        };
        let Some(payload) = decode_extension_trait_solving_facts_entry(&encoded, identity) else {
            remove_corrupt(&path);
            return Ok(ExtensionTraitSolvingFactsLookup::Corrupt);
        };
        let Some(module_id) = modules
            .get(identity.module.source_identity().normalized_path())
            .copied()
        else {
            remove_corrupt(&path);
            return Ok(ExtensionTraitSolvingFactsLookup::Corrupt);
        };
        let Some(facts) = decode_extension_trait_solving_facts(
            payload,
            identity.source_len,
            modules,
            symbols,
            type_store,
            module_id,
        ) else {
            remove_corrupt(&path);
            return Ok(ExtensionTraitSolvingFactsLookup::Corrupt);
        };
        Ok(ExtensionTraitSolvingFactsLookup::Hit(Box::new(facts)))
    }

    pub(crate) fn publish_extension_trait_solving_facts(
        &self,
        identity: ExtensionTraitSolvingFactsIdentity<'_>,
        facts: &CachedExtensionTraitSolvingFacts,
        module_paths: &HashMap<ModuleId, String>,
        symbols: &SymbolTable,
        type_store: &TypeStore,
        replace: bool,
    ) -> io::Result<()> {
        let path = self.extension_trait_solving_facts_path(identity.key);
        if !replace && path.is_file() {
            return Ok(());
        }
        let payload = encode_extension_trait_solving_facts(
            facts,
            identity.module,
            module_paths,
            symbols,
            type_store,
        )?;
        let encoded = encode_extension_trait_solving_facts_entry(identity, &payload);
        let parent = path
            .parent()
            .ok_or_else(|| io::Error::other("invalid signature cache path"))?;
        fs::create_dir_all(parent)?;
        atomic_publish(&path, &encoded)
    }

    pub(crate) fn remove_extension_trait_solving_facts(
        &self,
        key: FrontendExtensionTraitSolvingFactsCacheKey,
    ) {
        remove_corrupt(&self.extension_trait_solving_facts_path(key));
    }

    pub(crate) fn load_extension_validation_diagnostics(
        &self,
        identity: ExtensionValidationDiagnosticsIdentity<'_>,
    ) -> io::Result<ExtensionValidationDiagnosticsLookup> {
        let path = self.extension_validation_diagnostics_path(identity.key);
        let encoded = match fs::read(&path) {
            Ok(encoded) if encoded.len() <= MAX_ENTRY_BYTES => encoded,
            Ok(_) => {
                remove_corrupt(&path);
                return Ok(ExtensionValidationDiagnosticsLookup::Corrupt);
            }
            Err(error) if error.kind() == io::ErrorKind::NotFound => {
                return Ok(ExtensionValidationDiagnosticsLookup::NotFound);
            }
            Err(error) => return Err(error),
        };
        let Some(payload) = decode_extension_validation_diagnostics_entry(&encoded, identity)
        else {
            remove_corrupt(&path);
            return Ok(ExtensionValidationDiagnosticsLookup::Corrupt);
        };
        let Some(diagnostics) = decode_stable_diagnostic_bundle(payload, identity.source_len)
        else {
            remove_corrupt(&path);
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
        atomic_publish(&path, &encoded)
    }

    pub(crate) fn remove_extension_validation_diagnostics(
        &self,
        key: FrontendExtensionValidationDiagnosticsCacheKey,
    ) {
        remove_corrupt(&self.extension_validation_diagnostics_path(key));
    }

    pub(crate) fn load_executable_value_ref_edges(
        &self,
        identity: ExecutableValueRefEdgesIdentity<'_>,
        modules: &HashMap<String, ModuleId>,
    ) -> io::Result<ExecutableValueRefEdgesLookup> {
        let path = self.executable_value_ref_edges_path(identity.key);
        let encoded = match fs::read(&path) {
            Ok(encoded) if encoded.len() <= MAX_ENTRY_BYTES => encoded,
            Ok(_) => {
                remove_corrupt(&path);
                return Ok(ExecutableValueRefEdgesLookup::Corrupt);
            }
            Err(error) if error.kind() == io::ErrorKind::NotFound => {
                return Ok(ExecutableValueRefEdgesLookup::NotFound);
            }
            Err(error) => return Err(error),
        };
        let Some(payload) = decode_executable_value_ref_edges_entry(&encoded, identity) else {
            remove_corrupt(&path);
            return Ok(ExecutableValueRefEdgesLookup::Corrupt);
        };
        let Some(edges) = decode_executable_value_ref_edges(payload, modules) else {
            remove_corrupt(&path);
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
        atomic_publish(&path, &encoded)
    }

    pub(crate) fn remove_executable_value_ref_edges(
        &self,
        key: FrontendExecutableValueRefEdgesCacheKey,
    ) {
        remove_corrupt(&self.executable_value_ref_edges_path(key));
    }

    pub(crate) fn load_check_certificate(
        &self,
        identity: CheckCertificateIdentity<'_>,
    ) -> io::Result<CheckCertificateLookup> {
        let path = self.check_certificate_path(identity.key);
        let encoded = match fs::read(&path) {
            Ok(encoded) if encoded.len() <= MAX_ENTRY_BYTES => encoded,
            Ok(_) => {
                remove_corrupt(&path);
                return Ok(CheckCertificateLookup::Corrupt);
            }
            Err(error) if error.kind() == io::ErrorKind::NotFound => {
                return Ok(CheckCertificateLookup::NotFound);
            }
            Err(error) => return Err(error),
        };
        let Some(certificate) = decode_check_certificate(&encoded, identity) else {
            remove_corrupt(&path);
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
                    remove_corrupt(&path);
                }
                return Err(error);
            }
        };
        let parent = path
            .parent()
            .ok_or_else(|| io::Error::other("invalid signature cache path"))?;
        fs::create_dir_all(parent)?;
        atomic_publish(&path, &encoded)
    }

    pub(crate) fn remove_check_certificate(&self, key: FrontendCheckCertificateCacheKey) {
        remove_corrupt(&self.check_certificate_path(key));
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

    pub(crate) fn extension_trait_solving_facts_path(
        &self,
        key: FrontendExtensionTraitSolvingFactsCacheKey,
    ) -> PathBuf {
        let [first, second] = key.parts();
        self.root
            .join("artifacts")
            .join("frontend")
            .join("v3")
            .join("extension-trait-solving-facts")
            .join(format!("{first:016x}{second:016x}.ets"))
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

fn atomic_publish(path: &Path, encoded: &[u8]) -> io::Result<()> {
    let stage_id = STAGE_ID.fetch_add(1, Ordering::Relaxed);
    let staged = path.with_extension(format!("tmp-{}-{stage_id}", std::process::id()));
    let mut file = OpenOptions::new()
        .create_new(true)
        .write(true)
        .open(&staged)?;
    if let Err(error) = file.write_all(encoded).and_then(|()| file.sync_all()) {
        let _ = fs::remove_file(&staged);
        return Err(error);
    }
    drop(file);
    if let Err(error) = fs::rename(&staged, path) {
        let _ = fs::remove_file(&staged);
        return Err(error);
    }
    if let Some(parent) = path.parent()
        && let Ok(directory) = File::open(parent)
    {
        let _ = directory.sync_all();
    }
    Ok(())
}

fn remove_corrupt(path: &Path) {
    let _ = fs::remove_file(path);
}
