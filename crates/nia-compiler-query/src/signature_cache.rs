use std::{
    collections::{HashMap, HashSet},
    fs::{self, File, OpenOptions},
    io::{self, Cursor, Read, Write},
    path::{Path, PathBuf},
    sync::atomic::{AtomicU64, Ordering},
};

use nia_ids::{
    BuiltinConstValue, BuiltinFunction, BuiltinTrait, DefId, GlobalDefId, InternedTyId, ModuleId,
    ReceiverKind, TraitImplId, Visibility,
};
use nia_imports::StableModuleKey;
use nia_item_signatures::{self as item_signatures, ItemSignatures};
use nia_item_tree::SignatureItemSet;
use nia_node_id::{NodeChildPath, NodeMap, NodePosition, NodeSite, SyntaxKind, VersionedNodeKey};
use nia_query::{QueryFingerprint, QueryFingerprintBuilder};
use nia_source::SourceVersion;
use nia_symbol_table::SymbolTable;
use nia_ty::{
    ArrayLenTy, AssociatedTypeBindingTy, BuiltinType, ConstGenericArg, ConstGenericValue, IntConst,
    LayoutBuiltin, PrimitiveTy, PrimitiveTypeSpelling, RangeTyKind, TraitId, TyKind, TypeStore,
};
use nia_type_lower::TypeLowering;
use nia_type_resolve::{TypeNameResolution, TypeResolution};

use crate::{
    FrontendCacheNamespace, FrontendExtensionTraitSolvingFactsCacheKey,
    FrontendProgramSourceFingerprint, FrontendSignatureItemSignaturesCacheKey,
    FrontendSignatureTypeLoweringCacheKey, FrontendSignatureTypeResolutionCacheKey,
};

const TYPE_RESOLUTION_MAGIC: &[u8; 8] = b"NIASR001";
const TYPE_LOWERING_MAGIC: &[u8; 8] = b"NIASL001";
const ITEM_SIGNATURES_MAGIC: &[u8; 8] = b"NIASI002";
const EXTENSION_TRAIT_FACTS_MAGIC: &[u8; 8] = b"NIAET001";
const MAX_ENTRY_BYTES: usize = 64 * 1024 * 1024;
const MAX_SEQUENCE_LEN: usize = 1_000_000;
static STAGE_ID: AtomicU64 = AtomicU64::new(0);

#[derive(Debug)]
pub(crate) struct PersistentSignatureCache {
    root: PathBuf,
}

#[derive(Debug, Clone, Copy)]
pub(crate) struct SignatureTypeResolutionIdentity<'a> {
    pub(crate) key: FrontendSignatureTypeResolutionCacheKey,
    pub(crate) namespace: FrontendCacheNamespace,
    pub(crate) module: &'a StableModuleKey,
    pub(crate) set: SignatureItemSet,
    pub(crate) program_sources: FrontendProgramSourceFingerprint,
    pub(crate) source_version: SourceVersion,
    pub(crate) source_len: usize,
}

#[derive(Debug, Clone, Copy)]
pub(crate) struct SignatureTypeLoweringIdentity<'a> {
    pub(crate) key: FrontendSignatureTypeLoweringCacheKey,
    pub(crate) namespace: FrontendCacheNamespace,
    pub(crate) module: &'a StableModuleKey,
    pub(crate) set: SignatureItemSet,
    pub(crate) program_sources: FrontendProgramSourceFingerprint,
    pub(crate) source_version: SourceVersion,
    pub(crate) source_len: usize,
}

#[derive(Debug, Clone, Copy)]
pub(crate) struct SignatureItemSignaturesIdentity<'a> {
    pub(crate) key: FrontendSignatureItemSignaturesCacheKey,
    pub(crate) namespace: FrontendCacheNamespace,
    pub(crate) module: &'a StableModuleKey,
    pub(crate) set: SignatureItemSet,
    pub(crate) program_sources: FrontendProgramSourceFingerprint,
    pub(crate) source_len: usize,
}

#[derive(Debug, Clone, Copy)]
pub(crate) struct ExtensionTraitSolvingFactsIdentity<'a> {
    pub(crate) key: FrontendExtensionTraitSolvingFactsCacheKey,
    pub(crate) namespace: FrontendCacheNamespace,
    pub(crate) module: &'a StableModuleKey,
    pub(crate) program_sources: FrontendProgramSourceFingerprint,
    pub(crate) source_len: usize,
}

#[derive(Debug, Clone, PartialEq)]
pub(crate) struct CachedExtensionTraitSolvingFacts {
    pub(crate) trait_impls: Vec<item_signatures::ProgramTraitImplSignature>,
    pub(crate) invalid_trait_impl_method_ids: HashSet<GlobalDefId>,
}

#[derive(Debug, Clone, PartialEq)]
pub(crate) enum SignatureTypeResolutionLookup {
    Hit(Box<TypeResolution>),
    NotFound,
    Corrupt,
}

#[derive(Debug, Clone, PartialEq)]
pub(crate) enum SignatureTypeLoweringLookup {
    Hit(Box<TypeLowering>),
    NotFound,
    Corrupt,
}

#[derive(Debug, Clone, PartialEq)]
pub(crate) enum SignatureItemSignaturesLookup {
    Hit(Box<ItemSignatures>),
    NotFound,
    Corrupt,
}

#[derive(Debug, Clone, PartialEq)]
pub(crate) enum ExtensionTraitSolvingFactsLookup {
    Hit(Box<CachedExtensionTraitSolvingFacts>),
    NotFound,
    Corrupt,
}

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
}

fn encode_entry(identity: SignatureTypeResolutionIdentity<'_>, payload: &[u8]) -> Vec<u8> {
    let mut encoded = Vec::new();
    encoded.extend_from_slice(TYPE_RESOLUTION_MAGIC);
    write_parts(&mut encoded, identity.key.parts());
    write_parts(&mut encoded, identity.namespace.parts());
    write_parts(&mut encoded, identity.program_sources.parts());
    write_string(
        &mut encoded,
        identity.module.source_identity().normalized_path(),
    );
    encoded.push(signature_set_tag(identity.set));
    write_u64(&mut encoded, identity.source_len as u64);
    write_u64(&mut encoded, payload.len() as u64);
    encoded.extend_from_slice(payload);
    let checksum = checksum(&encoded);
    write_parts(&mut encoded, checksum.parts());
    encoded
}

fn decode_entry<'a>(
    encoded: &'a [u8],
    identity: SignatureTypeResolutionIdentity<'_>,
) -> Option<&'a [u8]> {
    if encoded.len() < TYPE_RESOLUTION_MAGIC.len() + 16 {
        return None;
    }
    let checksum_offset = encoded.len().checked_sub(16)?;
    let expected_checksum = checksum(&encoded[..checksum_offset]);
    let mut checksum_cursor = Cursor::new(&encoded[checksum_offset..]);
    if read_parts(&mut checksum_cursor)? != expected_checksum.parts() {
        return None;
    }
    let mut cursor = Cursor::new(&encoded[..checksum_offset]);
    let mut magic = [0_u8; 8];
    cursor.read_exact(&mut magic).ok()?;
    if &magic != TYPE_RESOLUTION_MAGIC
        || read_parts(&mut cursor)? != identity.key.parts()
        || read_parts(&mut cursor)? != identity.namespace.parts()
        || read_parts(&mut cursor)? != identity.program_sources.parts()
        || read_string(&mut cursor, encoded.len())?
            != identity.module.source_identity().normalized_path()
        || read_u8(&mut cursor)? != signature_set_tag(identity.set)
        || usize::try_from(read_u64(&mut cursor)?).ok()? != identity.source_len
    {
        return None;
    }
    let payload_len = usize::try_from(read_u64(&mut cursor)?).ok()?;
    if payload_len > MAX_ENTRY_BYTES {
        return None;
    }
    let start = usize::try_from(cursor.position()).ok()?;
    let end = start.checked_add(payload_len)?;
    (end == checksum_offset).then(|| &encoded[start..end])
}

fn encode_type_lowering_entry(
    identity: SignatureTypeLoweringIdentity<'_>,
    payload: &[u8],
) -> Vec<u8> {
    let mut encoded = Vec::new();
    encoded.extend_from_slice(TYPE_LOWERING_MAGIC);
    write_parts(&mut encoded, identity.key.parts());
    write_parts(&mut encoded, identity.namespace.parts());
    write_parts(&mut encoded, identity.program_sources.parts());
    write_string(
        &mut encoded,
        identity.module.source_identity().normalized_path(),
    );
    encoded.push(signature_set_tag(identity.set));
    write_u64(&mut encoded, identity.source_len as u64);
    write_u64(&mut encoded, payload.len() as u64);
    encoded.extend_from_slice(payload);
    let checksum = type_lowering_checksum(&encoded);
    write_parts(&mut encoded, checksum.parts());
    encoded
}

fn decode_type_lowering_entry<'a>(
    encoded: &'a [u8],
    identity: SignatureTypeLoweringIdentity<'_>,
) -> Option<&'a [u8]> {
    if encoded.len() < TYPE_LOWERING_MAGIC.len() + 16 {
        return None;
    }
    let checksum_offset = encoded.len().checked_sub(16)?;
    let expected_checksum = type_lowering_checksum(&encoded[..checksum_offset]);
    let mut checksum_cursor = Cursor::new(&encoded[checksum_offset..]);
    if read_parts(&mut checksum_cursor)? != expected_checksum.parts() {
        return None;
    }
    let mut cursor = Cursor::new(&encoded[..checksum_offset]);
    let mut magic = [0_u8; 8];
    cursor.read_exact(&mut magic).ok()?;
    if &magic != TYPE_LOWERING_MAGIC
        || read_parts(&mut cursor)? != identity.key.parts()
        || read_parts(&mut cursor)? != identity.namespace.parts()
        || read_parts(&mut cursor)? != identity.program_sources.parts()
        || read_string(&mut cursor, encoded.len())?
            != identity.module.source_identity().normalized_path()
        || read_u8(&mut cursor)? != signature_set_tag(identity.set)
        || usize::try_from(read_u64(&mut cursor)?).ok()? != identity.source_len
    {
        return None;
    }
    let payload_len = usize::try_from(read_u64(&mut cursor)?).ok()?;
    if payload_len > MAX_ENTRY_BYTES {
        return None;
    }
    let start = usize::try_from(cursor.position()).ok()?;
    let end = start.checked_add(payload_len)?;
    (end == checksum_offset).then(|| &encoded[start..end])
}

fn encode_item_signatures_entry(
    identity: SignatureItemSignaturesIdentity<'_>,
    payload: &[u8],
) -> Vec<u8> {
    let mut encoded = Vec::new();
    encoded.extend_from_slice(ITEM_SIGNATURES_MAGIC);
    write_parts(&mut encoded, identity.key.parts());
    write_parts(&mut encoded, identity.namespace.parts());
    write_parts(&mut encoded, identity.program_sources.parts());
    write_string(
        &mut encoded,
        identity.module.source_identity().normalized_path(),
    );
    encoded.push(signature_set_tag(identity.set));
    write_u64(&mut encoded, identity.source_len as u64);
    write_u64(&mut encoded, payload.len() as u64);
    encoded.extend_from_slice(payload);
    let checksum = item_signatures_checksum(&encoded);
    write_parts(&mut encoded, checksum.parts());
    encoded
}

fn decode_item_signatures_entry<'a>(
    encoded: &'a [u8],
    identity: SignatureItemSignaturesIdentity<'_>,
) -> Option<&'a [u8]> {
    if encoded.len() < ITEM_SIGNATURES_MAGIC.len() + 16 {
        return None;
    }
    let checksum_offset = encoded.len().checked_sub(16)?;
    let expected_checksum = item_signatures_checksum(&encoded[..checksum_offset]);
    let mut checksum_cursor = Cursor::new(&encoded[checksum_offset..]);
    if read_parts(&mut checksum_cursor)? != expected_checksum.parts() {
        return None;
    }
    let mut cursor = Cursor::new(&encoded[..checksum_offset]);
    let mut magic = [0_u8; 8];
    cursor.read_exact(&mut magic).ok()?;
    if &magic != ITEM_SIGNATURES_MAGIC
        || read_parts(&mut cursor)? != identity.key.parts()
        || read_parts(&mut cursor)? != identity.namespace.parts()
        || read_parts(&mut cursor)? != identity.program_sources.parts()
        || read_string(&mut cursor, encoded.len())?
            != identity.module.source_identity().normalized_path()
        || read_u8(&mut cursor)? != signature_set_tag(identity.set)
        || usize::try_from(read_u64(&mut cursor)?).ok()? != identity.source_len
    {
        return None;
    }
    let payload_len = usize::try_from(read_u64(&mut cursor)?).ok()?;
    if payload_len > MAX_ENTRY_BYTES {
        return None;
    }
    let start = usize::try_from(cursor.position()).ok()?;
    let end = start.checked_add(payload_len)?;
    (end == checksum_offset).then(|| &encoded[start..end])
}

fn encode_extension_trait_solving_facts_entry(
    identity: ExtensionTraitSolvingFactsIdentity<'_>,
    payload: &[u8],
) -> Vec<u8> {
    let mut encoded = Vec::new();
    encoded.extend_from_slice(EXTENSION_TRAIT_FACTS_MAGIC);
    write_parts(&mut encoded, identity.key.parts());
    write_parts(&mut encoded, identity.namespace.parts());
    write_parts(&mut encoded, identity.program_sources.parts());
    write_string(
        &mut encoded,
        identity.module.source_identity().normalized_path(),
    );
    write_u64(&mut encoded, identity.source_len as u64);
    write_u64(&mut encoded, payload.len() as u64);
    encoded.extend_from_slice(payload);
    let checksum = extension_trait_solving_facts_checksum(&encoded);
    write_parts(&mut encoded, checksum.parts());
    encoded
}

fn decode_extension_trait_solving_facts_entry<'a>(
    encoded: &'a [u8],
    identity: ExtensionTraitSolvingFactsIdentity<'_>,
) -> Option<&'a [u8]> {
    if encoded.len() < EXTENSION_TRAIT_FACTS_MAGIC.len() + 16 {
        return None;
    }
    let checksum_offset = encoded.len().checked_sub(16)?;
    let expected_checksum = extension_trait_solving_facts_checksum(&encoded[..checksum_offset]);
    let mut checksum_cursor = Cursor::new(&encoded[checksum_offset..]);
    if read_parts(&mut checksum_cursor)? != expected_checksum.parts() {
        return None;
    }
    let mut cursor = Cursor::new(&encoded[..checksum_offset]);
    let mut magic = [0_u8; 8];
    cursor.read_exact(&mut magic).ok()?;
    if &magic != EXTENSION_TRAIT_FACTS_MAGIC
        || read_parts(&mut cursor)? != identity.key.parts()
        || read_parts(&mut cursor)? != identity.namespace.parts()
        || read_parts(&mut cursor)? != identity.program_sources.parts()
        || read_string(&mut cursor, encoded.len())?
            != identity.module.source_identity().normalized_path()
        || usize::try_from(read_u64(&mut cursor)?).ok()? != identity.source_len
    {
        return None;
    }
    let payload_len = usize::try_from(read_u64(&mut cursor)?).ok()?;
    if payload_len > MAX_ENTRY_BYTES {
        return None;
    }
    let start = usize::try_from(cursor.position()).ok()?;
    let end = start.checked_add(payload_len)?;
    (end == checksum_offset).then(|| &encoded[start..end])
}

fn encode_type_resolution(
    resolution: &TypeResolution,
    source_version: SourceVersion,
    module_paths: &HashMap<ModuleId, String>,
    symbols: &SymbolTable,
) -> io::Result<Vec<u8>> {
    if !resolution.diagnostics.is_empty() {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            "cannot cache type resolution diagnostics",
        ));
    }
    let mut encoded = Vec::new();
    write_sorted_entries(
        &mut encoded,
        resolution.node_type_names.iter().map(|(site, value)| {
            let mut entry = Vec::new();
            write_node_site(&mut entry, site, source_version.id)?;
            write_type_name_resolution(&mut entry, *value, module_paths)?;
            Ok(entry)
        }),
    )?;
    write_sorted_entries(
        &mut encoded,
        resolution
            .node_qualified_type_names
            .iter()
            .map(|(site, value)| {
                let mut entry = Vec::new();
                write_node_site(&mut entry, site, source_version.id)?;
                write_global_def(&mut entry, *value, module_paths)?;
                Ok(entry)
            }),
    )?;
    write_sorted_entries(
        &mut encoded,
        resolution
            .node_const_generic_names
            .iter()
            .map(|(key, symbol)| {
                if key.source_version() != source_version {
                    return Err(io::Error::new(
                        io::ErrorKind::InvalidInput,
                        "type resolution node belongs to another source version",
                    ));
                }
                let mut entry = Vec::new();
                write_node_site(&mut entry, key.site(), source_version.id)?;
                let text = symbols.resolve(*symbol).ok_or_else(|| {
                    io::Error::new(io::ErrorKind::InvalidInput, "unresolved cached symbol")
                })?;
                write_string(&mut entry, &text);
                Ok(entry)
            }),
    )?;
    Ok(encoded)
}

fn decode_type_resolution(
    encoded: &[u8],
    source_version: SourceVersion,
    source_len: usize,
    modules: &HashMap<String, ModuleId>,
    symbols: &SymbolTable,
    node_store: &nia_node_id::NodeStore,
) -> Option<TypeResolution> {
    let mut cursor = Cursor::new(encoded);
    let mut node_type_names = HashMap::new();
    for entry in read_entries(&mut cursor, encoded.len())? {
        let mut entry = Cursor::new(entry);
        let site = read_node_site(&mut entry, source_version.id, source_len)?;
        let value = read_type_name_resolution(&mut entry, modules)?;
        if entry.position() as usize != entry.get_ref().len()
            || node_type_names.insert(site, value).is_some()
        {
            return None;
        }
    }
    let mut node_qualified_type_names = HashMap::new();
    for entry in read_entries(&mut cursor, encoded.len())? {
        let mut entry = Cursor::new(entry);
        let site = read_node_site(&mut entry, source_version.id, source_len)?;
        let value = read_global_def(&mut entry, modules)?;
        if entry.position() as usize != entry.get_ref().len()
            || node_qualified_type_names.insert(site, value).is_some()
        {
            return None;
        }
    }
    let mut node_const_generic_names = NodeMap::builder(node_store);
    let mut seen_const_nodes = HashSet::new();
    for entry in read_entries(&mut cursor, encoded.len())? {
        let mut entry = Cursor::new(entry);
        let site = read_node_site(&mut entry, source_version.id, source_len)?;
        let text = read_string(&mut entry, encoded.len())?;
        let symbol = symbols.intern(&text).ok()?;
        if entry.position() as usize != entry.get_ref().len()
            || !seen_const_nodes.insert(site.clone())
        {
            return None;
        }
        node_const_generic_names.insert(
            VersionedNodeKey {
                site,
                revision: source_version.revision,
            },
            symbol,
        );
    }
    if cursor.position() as usize != encoded.len() {
        return None;
    }
    Some(TypeResolution {
        node_type_names,
        node_qualified_type_names,
        node_const_generic_names: node_const_generic_names.finish(),
        diagnostics: Vec::new(),
    })
}

fn encode_item_signatures(
    signatures: &ItemSignatures,
    module_paths: &HashMap<ModuleId, String>,
    symbols: &SymbolTable,
    type_store: &TypeStore,
) -> io::Result<Vec<u8>> {
    if !signatures.diagnostics.is_empty() {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            "cannot cache item signature diagnostics",
        ));
    }
    let mut graph = TypeGraphEncoder {
        type_store,
        module_paths,
        symbols,
        indexes: HashMap::new(),
        visiting: HashSet::new(),
        nodes: Vec::new(),
    };
    let mut body = Vec::new();
    write_def_map(&mut body, &signatures.functions, |encoded, signature| {
        write_function_signature(encoded, signature, &mut graph)
    })?;
    write_def_map(&mut body, &signatures.structs, |encoded, signature| {
        write_struct_signature(encoded, signature, &mut graph)
    })?;
    write_def_map(&mut body, &signatures.unions, |encoded, signature| {
        write_union_signature(encoded, signature, &mut graph)
    })?;
    write_def_map(&mut body, &signatures.traits, |encoded, signature| {
        write_trait_signature(encoded, signature, &mut graph)
    })?;
    write_u64(&mut body, signatures.trait_impls.len() as u64);
    for signature in &signatures.trait_impls {
        write_trait_impl_signature(&mut body, signature, &mut graph)?;
    }
    write_def_map(&mut body, &signatures.enums, |encoded, signature| {
        write_enum_signature(encoded, signature, &mut graph)
    })?;
    write_def_map(&mut body, &signatures.type_aliases, |encoded, signature| {
        write_type_alias_signature(encoded, signature, &mut graph)
    })?;
    write_def_map(&mut body, &signatures.globals, |encoded, signature| {
        write_global_signature(encoded, signature, &mut graph)
    })?;
    write_def_map(&mut body, &signatures.consts, |encoded, signature| {
        write_const_signature(encoded, signature, &mut graph)
    })?;

    let mut encoded = Vec::new();
    write_type_graph(&mut encoded, graph.nodes);
    encoded.extend_from_slice(&body);
    Ok(encoded)
}

fn decode_item_signatures(
    encoded: &[u8],
    source_len: usize,
    modules: &HashMap<String, ModuleId>,
    symbols: &SymbolTable,
    type_store: &TypeStore,
    module_id: ModuleId,
) -> Option<ItemSignatures> {
    let mut cursor = Cursor::new(encoded);
    let types = read_type_graph(
        &mut cursor,
        encoded.len(),
        modules,
        symbols,
        type_store,
        module_id,
    )?;
    let functions = read_def_map(&mut cursor, |cursor| {
        read_function_signature(cursor, &types, symbols, source_len)
    })?;
    let structs = read_def_map(&mut cursor, |cursor| {
        read_struct_signature(cursor, &types, symbols, source_len)
    })?;
    let unions = read_def_map(&mut cursor, |cursor| {
        read_union_signature(cursor, &types, symbols, source_len)
    })?;
    let traits = read_def_map(&mut cursor, |cursor| {
        read_trait_signature(cursor, &types, symbols, source_len)
    })?;
    let trait_impl_len = read_len(&mut cursor, MAX_SEQUENCE_LEN)?;
    let mut trait_impls = Vec::with_capacity(trait_impl_len);
    let mut trait_impl_ids = HashSet::new();
    for _ in 0..trait_impl_len {
        let signature = read_trait_impl_signature(&mut cursor, &types, symbols, source_len)?;
        if !trait_impl_ids.insert(signature.impl_id) {
            return None;
        }
        trait_impls.push(signature);
    }
    let enums = read_def_map(&mut cursor, |cursor| {
        read_enum_signature(cursor, &types, symbols, source_len)
    })?;
    let type_aliases = read_def_map(&mut cursor, |cursor| {
        read_type_alias_signature(cursor, &types, symbols, source_len)
    })?;
    let globals = read_def_map(&mut cursor, |cursor| {
        read_global_signature(cursor, &types, source_len)
    })?;
    let consts = read_def_map(&mut cursor, |cursor| {
        read_const_signature(cursor, &types, source_len)
    })?;
    if cursor.position() as usize != encoded.len() {
        return None;
    }
    Some(ItemSignatures {
        functions,
        structs,
        unions,
        traits,
        trait_impls,
        enums,
        type_aliases,
        globals,
        consts,
        diagnostics: Vec::new(),
    })
}

fn encode_extension_trait_solving_facts(
    facts: &CachedExtensionTraitSolvingFacts,
    module: &StableModuleKey,
    module_paths: &HashMap<ModuleId, String>,
    symbols: &SymbolTable,
    type_store: &TypeStore,
) -> io::Result<Vec<u8>> {
    let module_path = module.source_identity().normalized_path();
    let mut matching_modules = module_paths
        .iter()
        .filter_map(|(module_id, path)| (path == module_path).then_some(*module_id));
    let module_id = matching_modules.next().ok_or_else(|| {
        io::Error::new(
            io::ErrorKind::InvalidInput,
            "extension facts module is absent from current program",
        )
    })?;
    if matching_modules.next().is_some()
        || facts
            .trait_impls
            .iter()
            .any(|signature| signature.module_id != module_id)
        || facts
            .invalid_trait_impl_method_ids
            .iter()
            .any(|def_id| def_id.module_id != module_id)
    {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            "extension facts contain a foreign module owner",
        ));
    }

    let mut graph = TypeGraphEncoder {
        type_store,
        module_paths,
        symbols,
        indexes: HashMap::new(),
        visiting: HashSet::new(),
        nodes: Vec::new(),
    };
    let mut body = Vec::new();
    write_u64(&mut body, facts.trait_impls.len() as u64);
    let mut impl_ids = HashSet::new();
    for signature in &facts.trait_impls {
        if !impl_ids.insert(signature.impl_id) {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                "duplicate extension trait impl identity",
            ));
        }
        write_program_trait_impl_signature(&mut body, signature, &mut graph)?;
    }
    let mut invalid_methods = facts
        .invalid_trait_impl_method_ids
        .iter()
        .map(|def_id| def_id.def_id)
        .collect::<Vec<_>>();
    invalid_methods.sort_unstable_by_key(|def_id| def_id.0);
    write_u64(&mut body, invalid_methods.len() as u64);
    for def_id in invalid_methods {
        write_u64(&mut body, def_id.0);
    }

    let mut encoded = Vec::new();
    write_type_graph(&mut encoded, graph.nodes);
    encoded.extend_from_slice(&body);
    Ok(encoded)
}

fn decode_extension_trait_solving_facts(
    encoded: &[u8],
    source_len: usize,
    modules: &HashMap<String, ModuleId>,
    symbols: &SymbolTable,
    type_store: &TypeStore,
    module_id: ModuleId,
) -> Option<CachedExtensionTraitSolvingFacts> {
    let mut cursor = Cursor::new(encoded);
    let types = read_type_graph(
        &mut cursor,
        encoded.len(),
        modules,
        symbols,
        type_store,
        module_id,
    )?;
    let trait_impl_len = read_len(&mut cursor, MAX_SEQUENCE_LEN)?;
    let mut trait_impls = Vec::with_capacity(trait_impl_len);
    let mut impl_ids = HashSet::new();
    for _ in 0..trait_impl_len {
        let signature = read_program_trait_impl_signature(
            &mut cursor,
            &types,
            symbols,
            modules,
            module_id,
            source_len,
        )?;
        if !impl_ids.insert(signature.impl_id) {
            return None;
        }
        trait_impls.push(signature);
    }
    let invalid_len = read_len(&mut cursor, MAX_SEQUENCE_LEN)?;
    let mut invalid_trait_impl_method_ids = HashSet::with_capacity(invalid_len);
    let mut previous = None;
    for _ in 0..invalid_len {
        let def_id = DefId(read_u64(&mut cursor)?);
        if previous.is_some_and(|previous| previous >= def_id)
            || !invalid_trait_impl_method_ids.insert(GlobalDefId { module_id, def_id })
        {
            return None;
        }
        previous = Some(def_id);
    }
    if cursor.position() as usize != encoded.len() {
        return None;
    }
    Some(CachedExtensionTraitSolvingFacts {
        trait_impls,
        invalid_trait_impl_method_ids,
    })
}

fn write_program_trait_impl_signature(
    encoded: &mut Vec<u8>,
    signature: &item_signatures::ProgramTraitImplSignature,
    graph: &mut TypeGraphEncoder<'_>,
) -> io::Result<()> {
    write_u64(encoded, signature.impl_id.0);
    write_optional_string(encoded, signature.builtin.as_deref());
    write_symbols(encoded, &signature.generics, graph)?;
    write_type_index(encoded, graph.intern(signature.target_ty)?);
    write_trait_id(encoded, signature.trait_id, graph.module_paths)?;
    write_types(encoded, &signature.trait_args, graph)?;
    write_const_args(encoded, &signature.trait_const_args, graph)?;
    write_where_predicates(encoded, &signature.where_predicates, graph)?;
    write_u64(encoded, signature.associated_types.len() as u64);
    for associated in &signature.associated_types {
        graph.write_symbol(encoded, associated.name)?;
        write_type_index(encoded, graph.intern(associated.ty)?);
        write_span(encoded, associated.span);
    }
    write_u64(encoded, signature.associated_values.len() as u64);
    for associated in &signature.associated_values {
        write_u64(encoded, associated.def_id.0);
        graph.write_symbol(encoded, associated.name)?;
        encoded.push(visibility_tag(associated.visibility));
        write_span(encoded, associated.span);
    }
    Ok(())
}

fn read_program_trait_impl_signature(
    cursor: &mut Cursor<&[u8]>,
    types: &[InternedTyId],
    symbols: &SymbolTable,
    modules: &HashMap<String, ModuleId>,
    module_id: ModuleId,
    source_len: usize,
) -> Option<item_signatures::ProgramTraitImplSignature> {
    let impl_id = TraitImplId(read_u64(cursor)?);
    let builtin = read_optional_string(cursor)?;
    let generics = read_symbols(cursor, symbols)?;
    let target_ty = read_type_index(cursor, types)?;
    let trait_id = read_trait_id(cursor, modules)?;
    let trait_args = read_types(cursor, types)?;
    let trait_const_args = read_const_args(cursor, types, symbols)?;
    let where_predicates = read_where_predicates(cursor, types, symbols, source_len)?;
    let associated_type_len = read_len(cursor, MAX_SEQUENCE_LEN)?;
    let mut associated_types = Vec::with_capacity(associated_type_len);
    let mut associated_type_names = HashSet::new();
    for _ in 0..associated_type_len {
        let associated = item_signatures::TraitImplAssociatedTypeSignature {
            name: read_symbol(cursor, symbols)?,
            ty: read_type_index(cursor, types)?,
            span: read_span(cursor, source_len)?,
        };
        if !associated_type_names.insert(associated.name) {
            return None;
        }
        associated_types.push(associated);
    }
    let associated_value_len = read_len(cursor, MAX_SEQUENCE_LEN)?;
    let mut associated_values = Vec::with_capacity(associated_value_len);
    let mut associated_value_ids = HashSet::new();
    for _ in 0..associated_value_len {
        let associated = item_signatures::TraitImplAssociatedValueSignature {
            def_id: DefId(read_u64(cursor)?),
            name: read_symbol(cursor, symbols)?,
            visibility: read_visibility(cursor)?,
            span: read_span(cursor, source_len)?,
        };
        if !associated_value_ids.insert(associated.def_id) {
            return None;
        }
        associated_values.push(associated);
    }
    Some(item_signatures::ProgramTraitImplSignature {
        module_id,
        impl_id,
        builtin,
        generics,
        target_ty,
        trait_id,
        trait_args,
        trait_const_args,
        where_predicates,
        associated_types,
        associated_values,
    })
}

fn write_def_map<T>(
    encoded: &mut Vec<u8>,
    values: &HashMap<DefId, T>,
    mut write_value: impl FnMut(&mut Vec<u8>, &T) -> io::Result<()>,
) -> io::Result<()> {
    let mut values = values.iter().collect::<Vec<_>>();
    values.sort_unstable_by_key(|(def_id, _)| def_id.0);
    write_u64(encoded, values.len() as u64);
    for (def_id, value) in values {
        write_u64(encoded, def_id.0);
        write_value(encoded, value)?;
    }
    Ok(())
}

fn read_def_map<T>(
    cursor: &mut Cursor<&[u8]>,
    mut read_value: impl FnMut(&mut Cursor<&[u8]>) -> Option<T>,
) -> Option<HashMap<DefId, T>> {
    let len = read_len(cursor, MAX_SEQUENCE_LEN)?;
    let mut values = HashMap::with_capacity(len);
    let mut previous = None;
    for _ in 0..len {
        let def_id = DefId(read_u64(cursor)?);
        if previous.is_some_and(|previous| previous >= def_id) {
            return None;
        }
        previous = Some(def_id);
        if values.insert(def_id, read_value(cursor)?).is_some() {
            return None;
        }
    }
    Some(values)
}

fn write_function_signature(
    encoded: &mut Vec<u8>,
    signature: &item_signatures::FunctionSignature,
    graph: &mut TypeGraphEncoder<'_>,
) -> io::Result<()> {
    graph.write_symbol(encoded, signature.name)?;
    write_symbols(encoded, &signature.generics, graph)?;
    write_u64(encoded, signature.generic_params.len() as u64);
    for param in &signature.generic_params {
        graph.write_symbol(encoded, param.name)?;
        match &param.kind {
            item_signatures::GenericParamSignatureKind::Type => encoded.push(0),
            item_signatures::GenericParamSignatureKind::Const { ty } => {
                encoded.push(1);
                write_type_index(encoded, graph.intern(*ty)?);
            }
        }
    }
    write_where_predicates(encoded, &signature.where_predicates, graph)?;
    write_u64(encoded, signature.params.len() as u64);
    for param in &signature.params {
        write_optional_symbol(encoded, param.name, graph)?;
        write_optional_receiver(encoded, param.receiver);
        write_type_index(encoded, graph.intern(param.ty)?);
        write_span(encoded, param.span);
    }
    write_type_index(encoded, graph.intern(signature.return_type)?);
    write_bool(encoded, signature.is_extern);
    write_bool(encoded, signature.is_const);
    write_bool(encoded, signature.is_variadic);
    write_u64(encoded, signature.attributes.len() as u64);
    for attribute in &signature.attributes {
        match attribute {
            item_signatures::FunctionAttribute::Naked => encoded.push(0),
            item_signatures::FunctionAttribute::Builtin(builtin) => {
                encoded.push(1);
                encoded.push(builtin_function_tag(*builtin));
            }
        }
    }
    write_bool(encoded, signature.has_body);
    write_span(encoded, signature.span);
    Ok(())
}

fn read_function_signature(
    cursor: &mut Cursor<&[u8]>,
    types: &[InternedTyId],
    symbols: &SymbolTable,
    source_len: usize,
) -> Option<item_signatures::FunctionSignature> {
    let function_name = read_symbol(cursor, symbols)?;
    let generics = read_symbols(cursor, symbols)?;
    let generic_len = read_len(cursor, MAX_SEQUENCE_LEN)?;
    let mut generic_params = Vec::with_capacity(generic_len);
    for _ in 0..generic_len {
        let name = read_symbol(cursor, symbols)?;
        let kind = match read_u8(cursor)? {
            0 => item_signatures::GenericParamSignatureKind::Type,
            1 => item_signatures::GenericParamSignatureKind::Const {
                ty: read_type_index(cursor, types)?,
            },
            _ => return None,
        };
        generic_params.push(item_signatures::GenericParamSignature { name, kind });
    }
    let where_predicates = read_where_predicates(cursor, types, symbols, source_len)?;
    let param_len = read_len(cursor, MAX_SEQUENCE_LEN)?;
    let mut params = Vec::with_capacity(param_len);
    for _ in 0..param_len {
        params.push(item_signatures::ParamSignature {
            name: read_optional_symbol(cursor, symbols)?,
            receiver: read_optional_receiver(cursor)?,
            ty: read_type_index(cursor, types)?,
            span: read_span(cursor, source_len)?,
        });
    }
    let return_type = read_type_index(cursor, types)?;
    let is_extern = read_bool(cursor)?;
    let is_const = read_bool(cursor)?;
    let is_variadic = read_bool(cursor)?;
    let attribute_len = read_len(cursor, MAX_SEQUENCE_LEN)?;
    let mut attributes = Vec::with_capacity(attribute_len);
    for _ in 0..attribute_len {
        attributes.push(match read_u8(cursor)? {
            0 => item_signatures::FunctionAttribute::Naked,
            1 => item_signatures::FunctionAttribute::Builtin(read_builtin_function(cursor)?),
            _ => return None,
        });
    }
    Some(item_signatures::FunctionSignature {
        name: function_name,
        generics,
        generic_params,
        where_predicates,
        params,
        return_type,
        is_extern,
        is_const,
        is_variadic,
        attributes,
        has_body: read_bool(cursor)?,
        span: read_span(cursor, source_len)?,
    })
}

fn write_struct_signature(
    encoded: &mut Vec<u8>,
    signature: &item_signatures::StructSignature,
    graph: &mut TypeGraphEncoder<'_>,
) -> io::Result<()> {
    write_symbols(encoded, &signature.generics, graph)?;
    write_where_predicates(encoded, &signature.where_predicates, graph)?;
    write_fields(encoded, &signature.fields, graph)?;
    write_bool(encoded, signature.is_extern);
    write_span(encoded, signature.span);
    Ok(())
}

fn read_struct_signature(
    cursor: &mut Cursor<&[u8]>,
    types: &[InternedTyId],
    symbols: &SymbolTable,
    source_len: usize,
) -> Option<item_signatures::StructSignature> {
    Some(item_signatures::StructSignature {
        generics: read_symbols(cursor, symbols)?,
        where_predicates: read_where_predicates(cursor, types, symbols, source_len)?,
        fields: read_fields(cursor, types, symbols, source_len)?,
        is_extern: read_bool(cursor)?,
        span: read_span(cursor, source_len)?,
    })
}

fn write_union_signature(
    encoded: &mut Vec<u8>,
    signature: &item_signatures::UnionSignature,
    graph: &mut TypeGraphEncoder<'_>,
) -> io::Result<()> {
    write_symbols(encoded, &signature.generics, graph)?;
    write_where_predicates(encoded, &signature.where_predicates, graph)?;
    write_fields(encoded, &signature.fields, graph)?;
    write_bool(encoded, signature.is_extern);
    write_span(encoded, signature.span);
    Ok(())
}

fn read_union_signature(
    cursor: &mut Cursor<&[u8]>,
    types: &[InternedTyId],
    symbols: &SymbolTable,
    source_len: usize,
) -> Option<item_signatures::UnionSignature> {
    Some(item_signatures::UnionSignature {
        generics: read_symbols(cursor, symbols)?,
        where_predicates: read_where_predicates(cursor, types, symbols, source_len)?,
        fields: read_fields(cursor, types, symbols, source_len)?,
        is_extern: read_bool(cursor)?,
        span: read_span(cursor, source_len)?,
    })
}

fn write_trait_signature(
    encoded: &mut Vec<u8>,
    signature: &item_signatures::TraitSignature,
    graph: &mut TypeGraphEncoder<'_>,
) -> io::Result<()> {
    write_symbols(encoded, &signature.generics, graph)?;
    write_where_predicates(encoded, &signature.where_predicates, graph)?;
    write_u64(encoded, signature.supertraits.len() as u64);
    for supertrait in &signature.supertraits {
        write_type_index(encoded, graph.intern(supertrait.ty)?);
        write_span(encoded, supertrait.span);
    }
    write_u64(encoded, signature.associated_types.len() as u64);
    for associated in &signature.associated_types {
        write_u64(encoded, associated.def_id.0);
        graph.write_symbol(encoded, associated.name)?;
        write_span(encoded, associated.span);
    }
    write_u64(encoded, signature.associated_values.len() as u64);
    for associated in &signature.associated_values {
        write_u64(encoded, associated.def_id.0);
        graph.write_symbol(encoded, associated.name)?;
        write_type_index(encoded, graph.intern(associated.ty)?);
        write_span(encoded, associated.span);
    }
    write_u64(encoded, signature.methods.len() as u64);
    for method in &signature.methods {
        write_u64(encoded, method.def_id.0);
        graph.write_symbol(encoded, method.name)?;
        write_function_signature(encoded, &method.signature, graph)?;
        write_bool(encoded, method.has_default);
        write_span(encoded, method.span);
    }
    match signature.builtin {
        Some(builtin) => {
            encoded.push(1);
            encoded.push(builtin_trait_tag(builtin));
        }
        None => encoded.push(0),
    }
    write_span(encoded, signature.span);
    Ok(())
}

fn read_trait_signature(
    cursor: &mut Cursor<&[u8]>,
    types: &[InternedTyId],
    symbols: &SymbolTable,
    source_len: usize,
) -> Option<item_signatures::TraitSignature> {
    let generics = read_symbols(cursor, symbols)?;
    let where_predicates = read_where_predicates(cursor, types, symbols, source_len)?;
    let supertrait_len = read_len(cursor, MAX_SEQUENCE_LEN)?;
    let mut supertraits = Vec::with_capacity(supertrait_len);
    for _ in 0..supertrait_len {
        supertraits.push(item_signatures::TraitSupertraitSignature {
            ty: read_type_index(cursor, types)?,
            span: read_span(cursor, source_len)?,
        });
    }
    let associated_type_len = read_len(cursor, MAX_SEQUENCE_LEN)?;
    let mut associated_types = Vec::with_capacity(associated_type_len);
    let mut associated_type_ids = HashSet::new();
    for _ in 0..associated_type_len {
        let value = item_signatures::TraitAssociatedTypeSignature {
            def_id: DefId(read_u64(cursor)?),
            name: read_symbol(cursor, symbols)?,
            span: read_span(cursor, source_len)?,
        };
        if !associated_type_ids.insert(value.def_id) {
            return None;
        }
        associated_types.push(value);
    }
    let associated_value_len = read_len(cursor, MAX_SEQUENCE_LEN)?;
    let mut associated_values = Vec::with_capacity(associated_value_len);
    let mut associated_value_ids = HashSet::new();
    for _ in 0..associated_value_len {
        let value = item_signatures::TraitAssociatedValueSignature {
            def_id: DefId(read_u64(cursor)?),
            name: read_symbol(cursor, symbols)?,
            ty: read_type_index(cursor, types)?,
            span: read_span(cursor, source_len)?,
        };
        if !associated_value_ids.insert(value.def_id) {
            return None;
        }
        associated_values.push(value);
    }
    let method_len = read_len(cursor, MAX_SEQUENCE_LEN)?;
    let mut methods = Vec::with_capacity(method_len);
    let mut method_ids = HashSet::new();
    for _ in 0..method_len {
        let method = item_signatures::TraitMethodSignature {
            def_id: DefId(read_u64(cursor)?),
            name: read_symbol(cursor, symbols)?,
            signature: read_function_signature(cursor, types, symbols, source_len)?,
            has_default: read_bool(cursor)?,
            span: read_span(cursor, source_len)?,
        };
        if !method_ids.insert(method.def_id) {
            return None;
        }
        methods.push(method);
    }
    let builtin = match read_u8(cursor)? {
        0 => None,
        1 => Some(read_builtin_trait(cursor)?),
        _ => return None,
    };
    Some(item_signatures::TraitSignature {
        generics,
        where_predicates,
        supertraits,
        associated_types,
        associated_values,
        methods,
        builtin,
        span: read_span(cursor, source_len)?,
    })
}

fn write_trait_impl_signature(
    encoded: &mut Vec<u8>,
    signature: &item_signatures::TraitImplSignature,
    graph: &mut TypeGraphEncoder<'_>,
) -> io::Result<()> {
    write_u64(encoded, signature.impl_id.0);
    write_optional_string(encoded, signature.builtin.as_deref());
    write_symbols(encoded, &signature.generics, graph)?;
    write_type_index(encoded, graph.intern(signature.target_ty)?);
    write_optional_type(encoded, signature.trait_ty, graph)?;
    write_optional_span(encoded, signature.trait_span);
    write_where_predicates(encoded, &signature.where_predicates, graph)?;
    write_u64(encoded, signature.associated_types.len() as u64);
    for associated in &signature.associated_types {
        graph.write_symbol(encoded, associated.name)?;
        write_type_index(encoded, graph.intern(associated.ty)?);
        write_span(encoded, associated.span);
    }
    write_u64(encoded, signature.associated_values.len() as u64);
    for associated in &signature.associated_values {
        write_u64(encoded, associated.def_id.0);
        graph.write_symbol(encoded, associated.name)?;
        encoded.push(visibility_tag(associated.visibility));
        write_span(encoded, associated.span);
    }
    write_u64(encoded, signature.methods.len() as u64);
    for method in &signature.methods {
        write_u64(encoded, method.def_id.0);
        graph.write_symbol(encoded, method.name)?;
        encoded.push(visibility_tag(method.visibility));
        write_span(encoded, method.span);
    }
    write_span(encoded, signature.span);
    Ok(())
}

fn read_trait_impl_signature(
    cursor: &mut Cursor<&[u8]>,
    types: &[InternedTyId],
    symbols: &SymbolTable,
    source_len: usize,
) -> Option<item_signatures::TraitImplSignature> {
    let impl_id = TraitImplId(read_u64(cursor)?);
    let builtin = read_optional_string(cursor)?;
    let generics = read_symbols(cursor, symbols)?;
    let target_ty = read_type_index(cursor, types)?;
    let trait_ty = read_optional_type(cursor, types)?;
    let trait_span = read_optional_span(cursor, source_len)?;
    if trait_ty.is_some() != trait_span.is_some() {
        return None;
    }
    let where_predicates = read_where_predicates(cursor, types, symbols, source_len)?;
    let associated_type_len = read_len(cursor, MAX_SEQUENCE_LEN)?;
    let mut associated_types = Vec::with_capacity(associated_type_len);
    for _ in 0..associated_type_len {
        associated_types.push(item_signatures::TraitImplAssociatedTypeSignature {
            name: read_symbol(cursor, symbols)?,
            ty: read_type_index(cursor, types)?,
            span: read_span(cursor, source_len)?,
        });
    }
    let associated_value_len = read_len(cursor, MAX_SEQUENCE_LEN)?;
    let mut associated_values = Vec::with_capacity(associated_value_len);
    let mut associated_value_ids = HashSet::new();
    for _ in 0..associated_value_len {
        let value = item_signatures::TraitImplAssociatedValueSignature {
            def_id: DefId(read_u64(cursor)?),
            name: read_symbol(cursor, symbols)?,
            visibility: read_visibility(cursor)?,
            span: read_span(cursor, source_len)?,
        };
        if !associated_value_ids.insert(value.def_id) {
            return None;
        }
        associated_values.push(value);
    }
    let method_len = read_len(cursor, MAX_SEQUENCE_LEN)?;
    let mut methods = Vec::with_capacity(method_len);
    let mut method_ids = HashSet::new();
    for _ in 0..method_len {
        let method = item_signatures::TraitImplMethodSignature {
            def_id: DefId(read_u64(cursor)?),
            name: read_symbol(cursor, symbols)?,
            visibility: read_visibility(cursor)?,
            span: read_span(cursor, source_len)?,
        };
        if !method_ids.insert(method.def_id) {
            return None;
        }
        methods.push(method);
    }
    Some(item_signatures::TraitImplSignature {
        impl_id,
        builtin,
        generics,
        target_ty,
        trait_ty,
        trait_span,
        where_predicates,
        associated_types,
        associated_values,
        methods,
        span: read_span(cursor, source_len)?,
    })
}

fn write_enum_signature(
    encoded: &mut Vec<u8>,
    signature: &item_signatures::EnumSignature,
    graph: &mut TypeGraphEncoder<'_>,
) -> io::Result<()> {
    write_type_index(encoded, graph.intern(signature.backing_type)?);
    write_bool(encoded, signature.is_open);
    write_u64(encoded, signature.variants.len() as u64);
    for variant in &signature.variants {
        write_u64(encoded, variant.def_id.0);
        graph.write_symbol(encoded, variant.name)?;
        write_span(encoded, variant.span);
    }
    write_span(encoded, signature.span);
    Ok(())
}

fn read_enum_signature(
    cursor: &mut Cursor<&[u8]>,
    types: &[InternedTyId],
    symbols: &SymbolTable,
    source_len: usize,
) -> Option<item_signatures::EnumSignature> {
    let backing_type = read_type_index(cursor, types)?;
    let is_open = read_bool(cursor)?;
    let variant_len = read_len(cursor, MAX_SEQUENCE_LEN)?;
    let mut variants = Vec::with_capacity(variant_len);
    let mut variant_ids = HashSet::new();
    for _ in 0..variant_len {
        let variant = item_signatures::EnumVariantSignature {
            def_id: DefId(read_u64(cursor)?),
            name: read_symbol(cursor, symbols)?,
            span: read_span(cursor, source_len)?,
        };
        if !variant_ids.insert(variant.def_id) {
            return None;
        }
        variants.push(variant);
    }
    Some(item_signatures::EnumSignature {
        backing_type,
        is_open,
        variants,
        span: read_span(cursor, source_len)?,
    })
}

fn write_type_alias_signature(
    encoded: &mut Vec<u8>,
    signature: &item_signatures::TypeAliasSignature,
    graph: &mut TypeGraphEncoder<'_>,
) -> io::Result<()> {
    write_symbols(encoded, &signature.generics, graph)?;
    write_type_index(encoded, graph.intern(signature.target)?);
    write_span(encoded, signature.span);
    Ok(())
}

fn read_type_alias_signature(
    cursor: &mut Cursor<&[u8]>,
    types: &[InternedTyId],
    symbols: &SymbolTable,
    source_len: usize,
) -> Option<item_signatures::TypeAliasSignature> {
    Some(item_signatures::TypeAliasSignature {
        generics: read_symbols(cursor, symbols)?,
        target: read_type_index(cursor, types)?,
        span: read_span(cursor, source_len)?,
    })
}

fn write_global_signature(
    encoded: &mut Vec<u8>,
    signature: &item_signatures::GlobalSignature,
    graph: &mut TypeGraphEncoder<'_>,
) -> io::Result<()> {
    write_optional_type(encoded, signature.explicit_type, graph)?;
    write_bool(encoded, signature.is_mutable);
    write_bool(encoded, signature.is_extern);
    write_span(encoded, signature.span);
    Ok(())
}

fn read_global_signature(
    cursor: &mut Cursor<&[u8]>,
    types: &[InternedTyId],
    source_len: usize,
) -> Option<item_signatures::GlobalSignature> {
    Some(item_signatures::GlobalSignature {
        explicit_type: read_optional_type(cursor, types)?,
        is_mutable: read_bool(cursor)?,
        is_extern: read_bool(cursor)?,
        span: read_span(cursor, source_len)?,
    })
}

fn write_const_signature(
    encoded: &mut Vec<u8>,
    signature: &item_signatures::ConstSignature,
    graph: &mut TypeGraphEncoder<'_>,
) -> io::Result<()> {
    write_optional_type(encoded, signature.explicit_type, graph)?;
    match signature.builtin {
        Some(builtin) => {
            encoded.push(1);
            encoded.push(builtin_const_tag(builtin));
        }
        None => encoded.push(0),
    }
    write_span(encoded, signature.span);
    Ok(())
}

fn read_const_signature(
    cursor: &mut Cursor<&[u8]>,
    types: &[InternedTyId],
    source_len: usize,
) -> Option<item_signatures::ConstSignature> {
    let explicit_type = read_optional_type(cursor, types)?;
    let builtin = match read_u8(cursor)? {
        0 => None,
        1 => Some(read_builtin_const(cursor)?),
        _ => return None,
    };
    Some(item_signatures::ConstSignature {
        explicit_type,
        builtin,
        span: read_span(cursor, source_len)?,
    })
}

fn write_fields(
    encoded: &mut Vec<u8>,
    fields: &[item_signatures::FieldSignature],
    graph: &mut TypeGraphEncoder<'_>,
) -> io::Result<()> {
    write_u64(encoded, fields.len() as u64);
    for field in fields {
        write_u64(encoded, field.def_id.0);
        graph.write_symbol(encoded, field.name)?;
        write_type_index(encoded, graph.intern(field.ty)?);
        write_span(encoded, field.span);
    }
    Ok(())
}

fn read_fields(
    cursor: &mut Cursor<&[u8]>,
    types: &[InternedTyId],
    symbols: &SymbolTable,
    source_len: usize,
) -> Option<Vec<item_signatures::FieldSignature>> {
    let len = read_len(cursor, MAX_SEQUENCE_LEN)?;
    let mut fields = Vec::with_capacity(len);
    let mut ids = HashSet::new();
    for _ in 0..len {
        let field = item_signatures::FieldSignature {
            def_id: DefId(read_u64(cursor)?),
            name: read_symbol(cursor, symbols)?,
            ty: read_type_index(cursor, types)?,
            span: read_span(cursor, source_len)?,
        };
        if !ids.insert(field.def_id) {
            return None;
        }
        fields.push(field);
    }
    Some(fields)
}

fn write_where_predicates(
    encoded: &mut Vec<u8>,
    predicates: &[item_signatures::WherePredicateSignature],
    graph: &mut TypeGraphEncoder<'_>,
) -> io::Result<()> {
    write_u64(encoded, predicates.len() as u64);
    for predicate in predicates {
        write_type_index(encoded, graph.intern(predicate.ty)?);
        write_u64(encoded, predicate.bounds.len() as u64);
        for bound in &predicate.bounds {
            write_type_index(encoded, graph.intern(bound.trait_ty)?);
            write_u64(encoded, bound.associated_type_bindings.len() as u64);
            for binding in &bound.associated_type_bindings {
                graph.write_symbol(encoded, binding.name)?;
                write_type_index(encoded, graph.intern(binding.ty)?);
                write_span(encoded, binding.span);
            }
            write_span(encoded, bound.span);
        }
        write_span(encoded, predicate.span);
    }
    Ok(())
}

fn read_where_predicates(
    cursor: &mut Cursor<&[u8]>,
    types: &[InternedTyId],
    symbols: &SymbolTable,
    source_len: usize,
) -> Option<Vec<item_signatures::WherePredicateSignature>> {
    let len = read_len(cursor, MAX_SEQUENCE_LEN)?;
    let mut predicates = Vec::with_capacity(len);
    for _ in 0..len {
        let ty = read_type_index(cursor, types)?;
        let bound_len = read_len(cursor, MAX_SEQUENCE_LEN)?;
        let mut bounds = Vec::with_capacity(bound_len);
        for _ in 0..bound_len {
            let trait_ty = read_type_index(cursor, types)?;
            let binding_len = read_len(cursor, MAX_SEQUENCE_LEN)?;
            let mut associated_type_bindings = Vec::with_capacity(binding_len);
            for _ in 0..binding_len {
                associated_type_bindings.push(item_signatures::AssociatedTypeBindingSignature {
                    name: read_symbol(cursor, symbols)?,
                    ty: read_type_index(cursor, types)?,
                    span: read_span(cursor, source_len)?,
                });
            }
            bounds.push(item_signatures::WhereBoundSignature {
                trait_ty,
                associated_type_bindings,
                span: read_span(cursor, source_len)?,
            });
        }
        predicates.push(item_signatures::WherePredicateSignature {
            ty,
            bounds,
            span: read_span(cursor, source_len)?,
        });
    }
    Some(predicates)
}

fn write_symbols(
    encoded: &mut Vec<u8>,
    symbols: &[nia_symbol::SymbolId],
    graph: &TypeGraphEncoder<'_>,
) -> io::Result<()> {
    write_u64(encoded, symbols.len() as u64);
    for symbol in symbols {
        graph.write_symbol(encoded, *symbol)?;
    }
    Ok(())
}

fn read_symbols(
    cursor: &mut Cursor<&[u8]>,
    symbols: &SymbolTable,
) -> Option<Vec<nia_symbol::SymbolId>> {
    let len = read_len(cursor, MAX_SEQUENCE_LEN)?;
    (0..len).map(|_| read_symbol(cursor, symbols)).collect()
}

fn write_optional_symbol(
    encoded: &mut Vec<u8>,
    symbol: Option<nia_symbol::SymbolId>,
    graph: &TypeGraphEncoder<'_>,
) -> io::Result<()> {
    match symbol {
        Some(symbol) => {
            encoded.push(1);
            graph.write_symbol(encoded, symbol)?;
        }
        None => encoded.push(0),
    }
    Ok(())
}

fn read_optional_symbol(
    cursor: &mut Cursor<&[u8]>,
    symbols: &SymbolTable,
) -> Option<Option<nia_symbol::SymbolId>> {
    match read_u8(cursor)? {
        0 => Some(None),
        1 => Some(Some(read_symbol(cursor, symbols)?)),
        _ => None,
    }
}

fn write_span(encoded: &mut Vec<u8>, span: nia_span::Span) {
    write_u64(encoded, span.start as u64);
    write_u64(encoded, span.end as u64);
}

fn read_span(cursor: &mut Cursor<&[u8]>, source_len: usize) -> Option<nia_span::Span> {
    let start = usize::try_from(read_u64(cursor)?).ok()?;
    let end = usize::try_from(read_u64(cursor)?).ok()?;
    (start <= end && end <= source_len).then(|| nia_span::Span::new(start, end))
}

fn write_optional_span(encoded: &mut Vec<u8>, span: Option<nia_span::Span>) {
    match span {
        Some(span) => {
            encoded.push(1);
            write_span(encoded, span);
        }
        None => encoded.push(0),
    }
}

fn read_optional_span(
    cursor: &mut Cursor<&[u8]>,
    source_len: usize,
) -> Option<Option<nia_span::Span>> {
    match read_u8(cursor)? {
        0 => Some(None),
        1 => Some(Some(read_span(cursor, source_len)?)),
        _ => None,
    }
}

fn write_optional_string(encoded: &mut Vec<u8>, value: Option<&str>) {
    match value {
        Some(value) => {
            encoded.push(1);
            write_string(encoded, value);
        }
        None => encoded.push(0),
    }
}

fn read_optional_string(cursor: &mut Cursor<&[u8]>) -> Option<Option<String>> {
    match read_u8(cursor)? {
        0 => Some(None),
        1 => Some(Some(read_string(cursor, cursor.get_ref().len())?)),
        _ => None,
    }
}

fn write_optional_receiver(encoded: &mut Vec<u8>, receiver: Option<ReceiverKind>) {
    encoded.push(match receiver {
        None => 0,
        Some(ReceiverKind::RefReadOnly) => 1,
        Some(ReceiverKind::Ref) => 2,
        Some(ReceiverKind::Value) => 3,
    });
}

fn read_optional_receiver(cursor: &mut Cursor<&[u8]>) -> Option<Option<ReceiverKind>> {
    match read_u8(cursor)? {
        0 => Some(None),
        1 => Some(Some(ReceiverKind::RefReadOnly)),
        2 => Some(Some(ReceiverKind::Ref)),
        3 => Some(Some(ReceiverKind::Value)),
        _ => None,
    }
}

fn visibility_tag(visibility: Visibility) -> u8 {
    match visibility {
        Visibility::Private => 0,
        Visibility::PublicSuper => 1,
        Visibility::PublicPkg => 2,
        Visibility::Public => 3,
    }
}

fn read_visibility(cursor: &mut Cursor<&[u8]>) -> Option<Visibility> {
    Some(match read_u8(cursor)? {
        0 => Visibility::Private,
        1 => Visibility::PublicSuper,
        2 => Visibility::PublicPkg,
        3 => Visibility::Public,
        _ => return None,
    })
}

fn builtin_function_tag(value: BuiltinFunction) -> u8 {
    BuiltinFunction::ALL
        .iter()
        .position(|candidate| *candidate == value)
        .expect("all builtin functions have stable tags") as u8
}

fn read_builtin_function(cursor: &mut Cursor<&[u8]>) -> Option<BuiltinFunction> {
    BuiltinFunction::ALL.get(read_u8(cursor)? as usize).copied()
}

const BUILTIN_CONSTS: [BuiltinConstValue; 7] = [
    BuiltinConstValue::TargetArch,
    BuiltinConstValue::TargetVendor,
    BuiltinConstValue::TargetOs,
    BuiltinConstValue::TargetEnv,
    BuiltinConstValue::TargetAbi,
    BuiltinConstValue::TargetEndian,
    BuiltinConstValue::TargetPointerWidth,
];

fn builtin_const_tag(value: BuiltinConstValue) -> u8 {
    BUILTIN_CONSTS
        .iter()
        .position(|candidate| *candidate == value)
        .expect("all builtin consts have stable tags") as u8
}

fn read_builtin_const(cursor: &mut Cursor<&[u8]>) -> Option<BuiltinConstValue> {
    BUILTIN_CONSTS.get(read_u8(cursor)? as usize).copied()
}

fn encode_type_lowering(
    lowering: &TypeLowering,
    source_version: SourceVersion,
    module_paths: &HashMap<ModuleId, String>,
    symbols: &SymbolTable,
    type_store: &TypeStore,
) -> io::Result<Vec<u8>> {
    if !lowering.diagnostics.is_empty()
        || !lowering.const_exprs.is_empty()
        || !lowering.const_expr_summaries.is_empty()
    {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            "cannot cache signature lowering with diagnostics or const expressions",
        ));
    }
    let mut uses = lowering
        .type_uses
        .iter()
        .map(|(site, ty)| {
            let mut encoded_site = Vec::new();
            write_node_site(&mut encoded_site, site, source_version.id)?;
            Ok((encoded_site, *ty))
        })
        .collect::<io::Result<Vec<_>>>()?;
    uses.sort_unstable_by(|left, right| left.0.cmp(&right.0));

    let mut graph = TypeGraphEncoder {
        type_store,
        module_paths,
        symbols,
        indexes: HashMap::new(),
        visiting: HashSet::new(),
        nodes: Vec::new(),
    };
    let uses = uses
        .into_iter()
        .map(|(site, ty)| Ok((site, graph.intern(ty)?)))
        .collect::<io::Result<Vec<_>>>()?;

    let mut encoded = Vec::new();
    write_type_graph(&mut encoded, graph.nodes);
    write_u64(&mut encoded, uses.len() as u64);
    for (site, index) in uses {
        write_u64(&mut encoded, site.len().saturating_add(8) as u64);
        encoded.extend_from_slice(&site);
        write_u64(&mut encoded, index as u64);
    }
    Ok(encoded)
}

fn decode_type_lowering(
    encoded: &[u8],
    source_version: SourceVersion,
    source_len: usize,
    modules: &HashMap<String, ModuleId>,
    symbols: &SymbolTable,
    type_store: &TypeStore,
    module_id: ModuleId,
) -> Option<TypeLowering> {
    let mut cursor = Cursor::new(encoded);
    let types = read_type_graph(
        &mut cursor,
        encoded.len(),
        modules,
        symbols,
        type_store,
        module_id,
    )?;

    let mut type_uses = HashMap::new();
    for entry in read_entries(&mut cursor, encoded.len())? {
        let mut entry = Cursor::new(entry);
        let site = read_node_site(&mut entry, source_version.id, source_len)?;
        let ty = *types.get(usize::try_from(read_u64(&mut entry)?).ok()?)?;
        if entry.position() as usize != entry.get_ref().len()
            || type_uses.insert(site, ty).is_some()
        {
            return None;
        }
    }
    if cursor.position() as usize != encoded.len() {
        return None;
    }
    Some(TypeLowering {
        type_uses,
        const_exprs: HashMap::new(),
        const_expr_summaries: HashMap::new(),
        diagnostics: Vec::new(),
    })
}

fn write_type_graph(encoded: &mut Vec<u8>, nodes: Vec<Vec<u8>>) {
    write_u64(encoded, nodes.len() as u64);
    for node in nodes {
        write_u64(encoded, node.len() as u64);
        encoded.extend_from_slice(&node);
    }
}

fn read_type_graph(
    cursor: &mut Cursor<&[u8]>,
    encoded_len: usize,
    modules: &HashMap<String, ModuleId>,
    symbols: &SymbolTable,
    type_store: &TypeStore,
    module_id: ModuleId,
) -> Option<Vec<InternedTyId>> {
    let node_entries = read_entries(cursor, encoded_len)?;
    let append = type_store.append_for_module(module_id);
    let mut types = Vec::with_capacity(node_entries.len());
    for entry in node_entries {
        let mut entry = Cursor::new(entry);
        let kind = read_ty_kind(&mut entry, &types, modules, symbols)?;
        if entry.position() as usize != entry.get_ref().len() {
            return None;
        }
        types.push(append.intern(kind));
    }
    Some(types)
}

struct TypeGraphEncoder<'a> {
    type_store: &'a TypeStore,
    module_paths: &'a HashMap<ModuleId, String>,
    symbols: &'a SymbolTable,
    indexes: HashMap<InternedTyId, u32>,
    visiting: HashSet<InternedTyId>,
    nodes: Vec<Vec<u8>>,
}

impl TypeGraphEncoder<'_> {
    fn intern(&mut self, ty: InternedTyId) -> io::Result<u32> {
        if let Some(index) = self.indexes.get(&ty) {
            return Ok(*index);
        }
        if !self.visiting.insert(ty) {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                "cyclic type graph cannot be cached",
            ));
        }
        let kind = self.type_store.get(ty).cloned().ok_or_else(|| {
            io::Error::new(
                io::ErrorKind::InvalidInput,
                "signature lowering type belongs to another type store",
            )
        })?;
        let mut encoded = Vec::new();
        write_ty_kind(&mut encoded, &kind, self)?;
        self.visiting.remove(&ty);
        let index = u32::try_from(self.nodes.len())
            .map_err(|_| io::Error::new(io::ErrorKind::InvalidInput, "type graph too large"))?;
        self.nodes.push(encoded);
        self.indexes.insert(ty, index);
        Ok(index)
    }

    fn write_symbol(&self, encoded: &mut Vec<u8>, symbol: nia_symbol::SymbolId) -> io::Result<()> {
        let text = self.symbols.resolve(symbol).ok_or_else(|| {
            io::Error::new(io::ErrorKind::InvalidInput, "unresolved cached type symbol")
        })?;
        write_string(encoded, &text);
        Ok(())
    }
}

fn write_ty_kind(
    encoded: &mut Vec<u8>,
    kind: &TyKind,
    graph: &mut TypeGraphEncoder<'_>,
) -> io::Result<()> {
    match kind {
        TyKind::Error => encoded.push(0),
        TyKind::ConstOnly => encoded.push(1),
        TyKind::Primitive(primitive) => {
            encoded.push(2);
            encoded.push(primitive_tag(*primitive));
        }
        TyKind::Pointer { is_readonly, elem } => {
            encoded.push(3);
            write_bool(encoded, *is_readonly);
            write_type_index(encoded, graph.intern(*elem)?);
        }
        TyKind::VolatilePointer { is_readonly, elem } => {
            encoded.push(4);
            write_bool(encoded, *is_readonly);
            write_type_index(encoded, graph.intern(*elem)?);
        }
        TyKind::Slice { is_readonly, elem } => {
            encoded.push(5);
            write_bool(encoded, *is_readonly);
            write_type_index(encoded, graph.intern(*elem)?);
        }
        TyKind::SlicePointee { elem } => {
            encoded.push(6);
            write_type_index(encoded, graph.intern(*elem)?);
        }
        TyKind::Array { len, elem } => {
            encoded.push(7);
            write_array_len(encoded, len, graph)?;
            write_type_index(encoded, graph.intern(*elem)?);
        }
        TyKind::Vector { elem, lanes } => {
            encoded.push(8);
            encoded.push(primitive_tag(*elem));
            write_u32(encoded, *lanes);
        }
        TyKind::Range { kind, bound } => {
            encoded.push(9);
            encoded.push(range_kind_tag(*kind));
            write_optional_type(encoded, *bound, graph)?;
        }
        TyKind::FunctionPointer {
            params,
            return_type,
            is_variadic,
        } => {
            encoded.push(10);
            write_types(encoded, params, graph)?;
            write_type_index(encoded, graph.intern(*return_type)?);
            write_bool(encoded, *is_variadic);
        }
        TyKind::Optional { elem } => {
            encoded.push(11);
            write_type_index(encoded, graph.intern(*elem)?);
        }
        TyKind::ErrorUnion { error, value } => {
            encoded.push(12);
            write_type_index(encoded, graph.intern(*error)?);
            write_type_index(encoded, graph.intern(*value)?);
        }
        TyKind::Nominal {
            def_id,
            args,
            const_args,
        } => {
            encoded.push(13);
            write_global_def(encoded, *def_id, graph.module_paths)?;
            write_types(encoded, args, graph)?;
            write_const_args(encoded, const_args, graph)?;
        }
        TyKind::BuiltinType(builtin) => {
            encoded.push(14);
            encoded.push(builtin_type_tag(*builtin));
        }
        TyKind::BuiltinTrait { trait_id, args } => {
            encoded.push(15);
            encoded.push(builtin_trait_tag(*trait_id));
            write_types(encoded, args, graph)?;
        }
        TyKind::TraitObject {
            is_readonly,
            trait_id,
            trait_args,
            trait_const_args,
            associated_type_bindings,
        } => {
            encoded.push(16);
            write_bool(encoded, *is_readonly);
            write_trait_id(encoded, *trait_id, graph.module_paths)?;
            write_types(encoded, trait_args, graph)?;
            write_const_args(encoded, trait_const_args, graph)?;
            write_associated_bindings(encoded, associated_type_bindings, graph)?;
        }
        TyKind::TraitObjectPointee {
            trait_id,
            trait_args,
            trait_const_args,
            associated_type_bindings,
        } => {
            encoded.push(17);
            write_trait_id(encoded, *trait_id, graph.module_paths)?;
            write_types(encoded, trait_args, graph)?;
            write_const_args(encoded, trait_const_args, graph)?;
            write_associated_bindings(encoded, associated_type_bindings, graph)?;
        }
        TyKind::Projection {
            self_ty,
            trait_id,
            trait_args,
            trait_const_args,
            name,
        } => {
            encoded.push(18);
            write_type_index(encoded, graph.intern(*self_ty)?);
            write_trait_id(encoded, *trait_id, graph.module_paths)?;
            write_types(encoded, trait_args, graph)?;
            write_const_args(encoded, trait_const_args, graph)?;
            graph.write_symbol(encoded, *name)?;
        }
        TyKind::GenericParam(name) => {
            encoded.push(19);
            graph.write_symbol(encoded, *name)?;
        }
        TyKind::SelfParam => encoded.push(20),
    }
    Ok(())
}

fn read_ty_kind(
    cursor: &mut Cursor<&[u8]>,
    types: &[InternedTyId],
    modules: &HashMap<String, ModuleId>,
    symbols: &SymbolTable,
) -> Option<TyKind> {
    Some(match read_u8(cursor)? {
        0 => TyKind::Error,
        1 => TyKind::ConstOnly,
        2 => TyKind::Primitive(read_primitive(cursor)?),
        3 => TyKind::Pointer {
            is_readonly: read_bool(cursor)?,
            elem: read_type_index(cursor, types)?,
        },
        4 => TyKind::VolatilePointer {
            is_readonly: read_bool(cursor)?,
            elem: read_type_index(cursor, types)?,
        },
        5 => TyKind::Slice {
            is_readonly: read_bool(cursor)?,
            elem: read_type_index(cursor, types)?,
        },
        6 => TyKind::SlicePointee {
            elem: read_type_index(cursor, types)?,
        },
        7 => TyKind::Array {
            len: read_array_len(cursor, types, symbols)?,
            elem: read_type_index(cursor, types)?,
        },
        8 => TyKind::Vector {
            elem: read_primitive(cursor)?,
            lanes: read_u32(cursor)?,
        },
        9 => TyKind::Range {
            kind: read_range_kind(cursor)?,
            bound: read_optional_type(cursor, types)?,
        },
        10 => TyKind::FunctionPointer {
            params: read_types(cursor, types)?,
            return_type: read_type_index(cursor, types)?,
            is_variadic: read_bool(cursor)?,
        },
        11 => TyKind::Optional {
            elem: read_type_index(cursor, types)?,
        },
        12 => TyKind::ErrorUnion {
            error: read_type_index(cursor, types)?,
            value: read_type_index(cursor, types)?,
        },
        13 => TyKind::Nominal {
            def_id: read_global_def(cursor, modules)?,
            args: read_types(cursor, types)?,
            const_args: read_const_args(cursor, types, symbols)?,
        },
        14 => TyKind::BuiltinType(read_builtin_type(cursor)?),
        15 => TyKind::BuiltinTrait {
            trait_id: read_builtin_trait(cursor)?,
            args: read_types(cursor, types)?,
        },
        16 => TyKind::TraitObject {
            is_readonly: read_bool(cursor)?,
            trait_id: read_trait_id(cursor, modules)?,
            trait_args: read_types(cursor, types)?,
            trait_const_args: read_const_args(cursor, types, symbols)?,
            associated_type_bindings: read_associated_bindings(cursor, types, modules, symbols)?,
        },
        17 => TyKind::TraitObjectPointee {
            trait_id: read_trait_id(cursor, modules)?,
            trait_args: read_types(cursor, types)?,
            trait_const_args: read_const_args(cursor, types, symbols)?,
            associated_type_bindings: read_associated_bindings(cursor, types, modules, symbols)?,
        },
        18 => TyKind::Projection {
            self_ty: read_type_index(cursor, types)?,
            trait_id: read_trait_id(cursor, modules)?,
            trait_args: read_types(cursor, types)?,
            trait_const_args: read_const_args(cursor, types, symbols)?,
            name: read_symbol(cursor, symbols)?,
        },
        19 => TyKind::GenericParam(read_symbol(cursor, symbols)?),
        20 => TyKind::SelfParam,
        _ => return None,
    })
}

fn write_types(
    encoded: &mut Vec<u8>,
    types: &[InternedTyId],
    graph: &mut TypeGraphEncoder<'_>,
) -> io::Result<()> {
    write_u64(encoded, types.len() as u64);
    for ty in types {
        write_type_index(encoded, graph.intern(*ty)?);
    }
    Ok(())
}

fn read_types(cursor: &mut Cursor<&[u8]>, types: &[InternedTyId]) -> Option<Vec<InternedTyId>> {
    let len = read_len(cursor, MAX_SEQUENCE_LEN)?;
    (0..len).map(|_| read_type_index(cursor, types)).collect()
}

fn write_optional_type(
    encoded: &mut Vec<u8>,
    ty: Option<InternedTyId>,
    graph: &mut TypeGraphEncoder<'_>,
) -> io::Result<()> {
    match ty {
        Some(ty) => {
            encoded.push(1);
            write_type_index(encoded, graph.intern(ty)?);
        }
        None => encoded.push(0),
    }
    Ok(())
}

fn read_optional_type(
    cursor: &mut Cursor<&[u8]>,
    types: &[InternedTyId],
) -> Option<Option<InternedTyId>> {
    match read_u8(cursor)? {
        0 => Some(None),
        1 => Some(Some(read_type_index(cursor, types)?)),
        _ => None,
    }
}

fn write_array_len(
    encoded: &mut Vec<u8>,
    len: &ArrayLenTy,
    graph: &mut TypeGraphEncoder<'_>,
) -> io::Result<()> {
    match len {
        ArrayLenTy::Infer => encoded.push(0),
        ArrayLenTy::GenericParam(name) => {
            encoded.push(1);
            graph.write_symbol(encoded, *name)?;
        }
        ArrayLenTy::ConstValue(value) => {
            encoded.push(2);
            write_u64(encoded, *value);
        }
        ArrayLenTy::ConstExpr(_) => {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                "const-expression array length is not cacheable",
            ));
        }
        ArrayLenTy::Builtin { builtin, ty } => {
            encoded.push(3);
            encoded.push(layout_builtin_tag(*builtin));
            write_type_index(encoded, graph.intern(*ty)?);
        }
    }
    Ok(())
}

fn read_array_len(
    cursor: &mut Cursor<&[u8]>,
    types: &[InternedTyId],
    symbols: &SymbolTable,
) -> Option<ArrayLenTy> {
    Some(match read_u8(cursor)? {
        0 => ArrayLenTy::Infer,
        1 => ArrayLenTy::GenericParam(read_symbol(cursor, symbols)?),
        2 => ArrayLenTy::ConstValue(read_u64(cursor)?),
        3 => ArrayLenTy::Builtin {
            builtin: read_layout_builtin(cursor)?,
            ty: read_type_index(cursor, types)?,
        },
        _ => return None,
    })
}

fn write_const_args(
    encoded: &mut Vec<u8>,
    args: &[ConstGenericArg],
    graph: &mut TypeGraphEncoder<'_>,
) -> io::Result<()> {
    write_u64(encoded, args.len() as u64);
    for arg in args {
        write_type_index(encoded, graph.intern(arg.ty)?);
        match arg.value {
            ConstGenericValue::GenericParam(name) => {
                encoded.push(0);
                graph.write_symbol(encoded, name)?;
            }
            ConstGenericValue::ConstExpr(_) => {
                return Err(io::Error::new(
                    io::ErrorKind::InvalidInput,
                    "const-expression generic argument is not cacheable",
                ));
            }
            ConstGenericValue::Int(value) => {
                encoded.push(1);
                write_u128(encoded, value.bits());
                write_bool(encoded, value.is_signed());
            }
            ConstGenericValue::Bool(value) => {
                encoded.push(2);
                write_bool(encoded, value);
            }
            ConstGenericValue::Char(value) => {
                encoded.push(3);
                write_u32(encoded, value as u32);
            }
        }
    }
    Ok(())
}

fn read_const_args(
    cursor: &mut Cursor<&[u8]>,
    types: &[InternedTyId],
    symbols: &SymbolTable,
) -> Option<Vec<ConstGenericArg>> {
    let len = read_len(cursor, MAX_SEQUENCE_LEN)?;
    let mut args = Vec::with_capacity(len);
    for _ in 0..len {
        let ty = read_type_index(cursor, types)?;
        let value = match read_u8(cursor)? {
            0 => ConstGenericValue::GenericParam(read_symbol(cursor, symbols)?),
            1 => {
                let bits = read_u128(cursor)?;
                ConstGenericValue::Int(if read_bool(cursor)? {
                    IntConst::signed_bits(bits)
                } else {
                    IntConst::unsigned(bits)
                })
            }
            2 => ConstGenericValue::Bool(read_bool(cursor)?),
            3 => ConstGenericValue::Char(char::from_u32(read_u32(cursor)?)?),
            _ => return None,
        };
        args.push(ConstGenericArg { ty, value });
    }
    Some(args)
}

fn write_associated_bindings(
    encoded: &mut Vec<u8>,
    bindings: &[AssociatedTypeBindingTy],
    graph: &mut TypeGraphEncoder<'_>,
) -> io::Result<()> {
    write_u64(encoded, bindings.len() as u64);
    for binding in bindings {
        match binding.trait_id {
            Some(trait_id) => {
                encoded.push(1);
                write_trait_id(encoded, trait_id, graph.module_paths)?;
            }
            None => encoded.push(0),
        }
        write_types(encoded, &binding.trait_args, graph)?;
        write_const_args(encoded, &binding.trait_const_args, graph)?;
        graph.write_symbol(encoded, binding.name)?;
        write_type_index(encoded, graph.intern(binding.ty)?);
    }
    Ok(())
}

fn read_associated_bindings(
    cursor: &mut Cursor<&[u8]>,
    types: &[InternedTyId],
    modules: &HashMap<String, ModuleId>,
    symbols: &SymbolTable,
) -> Option<Vec<AssociatedTypeBindingTy>> {
    let len = read_len(cursor, MAX_SEQUENCE_LEN)?;
    let mut bindings = Vec::with_capacity(len);
    for _ in 0..len {
        let trait_id = match read_u8(cursor)? {
            0 => None,
            1 => Some(read_trait_id(cursor, modules)?),
            _ => return None,
        };
        bindings.push(AssociatedTypeBindingTy {
            trait_id,
            trait_args: read_types(cursor, types)?,
            trait_const_args: read_const_args(cursor, types, symbols)?,
            name: read_symbol(cursor, symbols)?,
            ty: read_type_index(cursor, types)?,
        });
    }
    Some(bindings)
}

fn write_trait_id(
    encoded: &mut Vec<u8>,
    trait_id: TraitId,
    module_paths: &HashMap<ModuleId, String>,
) -> io::Result<()> {
    match trait_id {
        TraitId::Source(def_id) => {
            encoded.push(0);
            write_global_def(encoded, def_id, module_paths)?;
        }
        TraitId::Builtin(builtin) => {
            encoded.push(1);
            encoded.push(builtin_trait_tag(builtin));
        }
    }
    Ok(())
}

fn read_trait_id(
    cursor: &mut Cursor<&[u8]>,
    modules: &HashMap<String, ModuleId>,
) -> Option<TraitId> {
    match read_u8(cursor)? {
        0 => Some(TraitId::Source(read_global_def(cursor, modules)?)),
        1 => Some(TraitId::Builtin(read_builtin_trait(cursor)?)),
        _ => None,
    }
}

fn read_symbol(cursor: &mut Cursor<&[u8]>, symbols: &SymbolTable) -> Option<nia_symbol::SymbolId> {
    let text = read_string(cursor, cursor.get_ref().len())?;
    symbols.intern(&text).ok()
}

fn write_type_index(encoded: &mut Vec<u8>, index: u32) {
    write_u64(encoded, u64::from(index));
}

fn read_type_index(cursor: &mut Cursor<&[u8]>, types: &[InternedTyId]) -> Option<InternedTyId> {
    types.get(usize::try_from(read_u64(cursor)?).ok()?).copied()
}

fn write_bool(encoded: &mut Vec<u8>, value: bool) {
    encoded.push(u8::from(value));
}

fn read_bool(cursor: &mut Cursor<&[u8]>) -> Option<bool> {
    match read_u8(cursor)? {
        0 => Some(false),
        1 => Some(true),
        _ => None,
    }
}

fn range_kind_tag(value: RangeTyKind) -> u8 {
    match value {
        RangeTyKind::Exclusive => 0,
        RangeTyKind::Inclusive => 1,
        RangeTyKind::From => 2,
        RangeTyKind::To => 3,
        RangeTyKind::ToInclusive => 4,
        RangeTyKind::Full => 5,
    }
}

fn read_range_kind(cursor: &mut Cursor<&[u8]>) -> Option<RangeTyKind> {
    Some(match read_u8(cursor)? {
        0 => RangeTyKind::Exclusive,
        1 => RangeTyKind::Inclusive,
        2 => RangeTyKind::From,
        3 => RangeTyKind::To,
        4 => RangeTyKind::ToInclusive,
        5 => RangeTyKind::Full,
        _ => return None,
    })
}

fn layout_builtin_tag(value: LayoutBuiltin) -> u8 {
    match value {
        LayoutBuiltin::Size => 0,
        LayoutBuiltin::Align => 1,
    }
}

fn read_layout_builtin(cursor: &mut Cursor<&[u8]>) -> Option<LayoutBuiltin> {
    Some(match read_u8(cursor)? {
        0 => LayoutBuiltin::Size,
        1 => LayoutBuiltin::Align,
        _ => return None,
    })
}

fn builtin_type_tag(value: BuiltinType) -> u8 {
    match value {
        BuiltinType::AsmConfig => 0,
    }
}

fn read_builtin_type(cursor: &mut Cursor<&[u8]>) -> Option<BuiltinType> {
    match read_u8(cursor)? {
        0 => Some(BuiltinType::AsmConfig),
        _ => None,
    }
}

fn write_sorted_entries(
    encoded: &mut Vec<u8>,
    entries: impl IntoIterator<Item = io::Result<Vec<u8>>>,
) -> io::Result<()> {
    let mut entries = entries.into_iter().collect::<io::Result<Vec<_>>>()?;
    entries.sort_unstable();
    write_u64(encoded, entries.len() as u64);
    for entry in entries {
        write_u64(encoded, entry.len() as u64);
        encoded.extend_from_slice(&entry);
    }
    Ok(())
}

fn read_entries<'a>(cursor: &mut Cursor<&'a [u8]>, encoded_len: usize) -> Option<Vec<&'a [u8]>> {
    let len = read_len(cursor, MAX_SEQUENCE_LEN)?;
    let mut entries = Vec::with_capacity(len);
    for _ in 0..len {
        let entry_len = read_len(cursor, encoded_len)?;
        let start = usize::try_from(cursor.position()).ok()?;
        let end = start.checked_add(entry_len)?;
        let entry = cursor.get_ref().get(start..end)?;
        cursor.set_position(end as u64);
        entries.push(entry);
    }
    Some(entries)
}

fn write_node_site(
    encoded: &mut Vec<u8>,
    site: &NodeSite,
    source_id: nia_source::SourceId,
) -> io::Result<()> {
    if site.source_id != source_id {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            "type resolution node belongs to another source",
        ));
    }
    encoded.push(syntax_kind_tag(site.kind));
    match &site.position {
        NodePosition::Span(span) => {
            encoded.push(0);
            write_u64(encoded, span.start as u64);
            write_u64(encoded, span.end as u64);
        }
        NodePosition::ChildPath(path) => {
            encoded.push(1);
            write_child_path(encoded, path);
        }
        NodePosition::ChildPathRange { start, end } => {
            encoded.push(2);
            write_child_path(encoded, start);
            write_child_path(encoded, end);
        }
    }
    Ok(())
}

fn read_node_site(
    cursor: &mut Cursor<&[u8]>,
    source_id: nia_source::SourceId,
    source_len: usize,
) -> Option<NodeSite> {
    let kind = read_syntax_kind(cursor)?;
    let position = match read_u8(cursor)? {
        0 => {
            let start = usize::try_from(read_u64(cursor)?).ok()?;
            let end = usize::try_from(read_u64(cursor)?).ok()?;
            if start > end || end > source_len {
                return None;
            }
            NodePosition::Span(nia_span::Span::new(start, end))
        }
        1 => NodePosition::ChildPath(read_child_path(cursor)?),
        2 => NodePosition::ChildPathRange {
            start: read_child_path(cursor)?,
            end: read_child_path(cursor)?,
        },
        _ => return None,
    };
    Some(NodeSite {
        source_id,
        kind,
        position,
    })
}

fn write_child_path(encoded: &mut Vec<u8>, path: &NodeChildPath) {
    write_u64(encoded, path.steps().len() as u64);
    for step in path.steps() {
        encoded.extend_from_slice(&step.to_le_bytes());
    }
}

fn read_child_path(cursor: &mut Cursor<&[u8]>) -> Option<NodeChildPath> {
    let len = read_len(cursor, MAX_SEQUENCE_LEN)?;
    let mut steps = Vec::with_capacity(len);
    for _ in 0..len {
        let mut bytes = [0_u8; 4];
        cursor.read_exact(&mut bytes).ok()?;
        steps.push(u32::from_le_bytes(bytes));
    }
    Some(NodeChildPath::from_steps(steps))
}

fn write_type_name_resolution(
    encoded: &mut Vec<u8>,
    value: TypeNameResolution,
    module_paths: &HashMap<ModuleId, String>,
) -> io::Result<()> {
    match value {
        TypeNameResolution::Primitive(spelling) => {
            encoded.push(0);
            write_primitive_spelling(encoded, spelling);
        }
        TypeNameResolution::BuiltinTrait(value) => {
            encoded.push(1);
            encoded.push(builtin_trait_tag(value));
        }
        TypeNameResolution::Def(value) => {
            encoded.push(2);
            write_u64(encoded, value.0);
        }
        TypeNameResolution::External(value) => {
            encoded.push(3);
            write_global_def(encoded, value, module_paths)?;
        }
        TypeNameResolution::GenericParam => encoded.push(4),
        TypeNameResolution::AssociatedType => encoded.push(5),
        TypeNameResolution::Error => encoded.push(6),
    }
    Ok(())
}

fn read_type_name_resolution(
    cursor: &mut Cursor<&[u8]>,
    modules: &HashMap<String, ModuleId>,
) -> Option<TypeNameResolution> {
    Some(match read_u8(cursor)? {
        0 => TypeNameResolution::Primitive(read_primitive_spelling(cursor)?),
        1 => TypeNameResolution::BuiltinTrait(read_builtin_trait(cursor)?),
        2 => TypeNameResolution::Def(DefId(read_u64(cursor)?)),
        3 => TypeNameResolution::External(read_global_def(cursor, modules)?),
        4 => TypeNameResolution::GenericParam,
        5 => TypeNameResolution::AssociatedType,
        6 => TypeNameResolution::Error,
        _ => return None,
    })
}

fn write_global_def(
    encoded: &mut Vec<u8>,
    value: GlobalDefId,
    module_paths: &HashMap<ModuleId, String>,
) -> io::Result<()> {
    let path = module_paths.get(&value.module_id).ok_or_else(|| {
        io::Error::new(
            io::ErrorKind::InvalidInput,
            "global def module is not loaded",
        )
    })?;
    write_string(encoded, path);
    write_u64(encoded, value.def_id.0);
    Ok(())
}

fn read_global_def(
    cursor: &mut Cursor<&[u8]>,
    modules: &HashMap<String, ModuleId>,
) -> Option<GlobalDefId> {
    let path = read_string(cursor, cursor.get_ref().len())?;
    Some(GlobalDefId {
        module_id: *modules.get(&path)?,
        def_id: DefId(read_u64(cursor)?),
    })
}

fn write_primitive_spelling(encoded: &mut Vec<u8>, spelling: PrimitiveTypeSpelling) {
    match spelling {
        PrimitiveTypeSpelling::Scalar(primitive) => {
            encoded.push(0);
            encoded.push(primitive_tag(primitive));
        }
        PrimitiveTypeSpelling::Vector { elem, lanes } => {
            encoded.push(1);
            encoded.push(primitive_tag(elem));
            encoded.extend_from_slice(&lanes.to_le_bytes());
        }
    }
}

fn read_primitive_spelling(cursor: &mut Cursor<&[u8]>) -> Option<PrimitiveTypeSpelling> {
    match read_u8(cursor)? {
        0 => Some(PrimitiveTypeSpelling::Scalar(read_primitive(cursor)?)),
        1 => {
            let elem = read_primitive(cursor)?;
            let mut bytes = [0_u8; 4];
            cursor.read_exact(&mut bytes).ok()?;
            Some(PrimitiveTypeSpelling::Vector {
                elem,
                lanes: u32::from_le_bytes(bytes),
            })
        }
        _ => None,
    }
}

fn primitive_tag(value: PrimitiveTy) -> u8 {
    match value {
        PrimitiveTy::I8 => 0,
        PrimitiveTy::I16 => 1,
        PrimitiveTy::I32 => 2,
        PrimitiveTy::I64 => 3,
        PrimitiveTy::I128 => 4,
        PrimitiveTy::Isize => 5,
        PrimitiveTy::U8 => 6,
        PrimitiveTy::U16 => 7,
        PrimitiveTy::U32 => 8,
        PrimitiveTy::U64 => 9,
        PrimitiveTy::U128 => 10,
        PrimitiveTy::Usize => 11,
        PrimitiveTy::F32 => 12,
        PrimitiveTy::F64 => 13,
        PrimitiveTy::Bool => 14,
        PrimitiveTy::Char => 15,
        PrimitiveTy::Void => 16,
        PrimitiveTy::Never => 17,
    }
}

fn read_primitive(cursor: &mut Cursor<&[u8]>) -> Option<PrimitiveTy> {
    Some(match read_u8(cursor)? {
        0 => PrimitiveTy::I8,
        1 => PrimitiveTy::I16,
        2 => PrimitiveTy::I32,
        3 => PrimitiveTy::I64,
        4 => PrimitiveTy::I128,
        5 => PrimitiveTy::Isize,
        6 => PrimitiveTy::U8,
        7 => PrimitiveTy::U16,
        8 => PrimitiveTy::U32,
        9 => PrimitiveTy::U64,
        10 => PrimitiveTy::U128,
        11 => PrimitiveTy::Usize,
        12 => PrimitiveTy::F32,
        13 => PrimitiveTy::F64,
        14 => PrimitiveTy::Bool,
        15 => PrimitiveTy::Char,
        16 => PrimitiveTy::Void,
        17 => PrimitiveTy::Never,
        _ => return None,
    })
}

const BUILTIN_TRAITS: [BuiltinTrait; 33] = [
    BuiltinTrait::Add,
    BuiltinTrait::Sub,
    BuiltinTrait::Mul,
    BuiltinTrait::Div,
    BuiltinTrait::Rem,
    BuiltinTrait::Neg,
    BuiltinTrait::Not,
    BuiltinTrait::BitNot,
    BuiltinTrait::BitAnd,
    BuiltinTrait::BitOr,
    BuiltinTrait::BitXor,
    BuiltinTrait::Shl,
    BuiltinTrait::Shr,
    BuiltinTrait::Eq,
    BuiltinTrait::Ord,
    BuiltinTrait::Sized,
    BuiltinTrait::Unsized,
    BuiltinTrait::Deref,
    BuiltinTrait::DerefMut,
    BuiltinTrait::Index,
    BuiltinTrait::IndexMut,
    BuiltinTrait::Slice,
    BuiltinTrait::SliceMut,
    BuiltinTrait::Ptr,
    BuiltinTrait::PtrMut,
    BuiltinTrait::Len,
    BuiltinTrait::Start,
    BuiltinTrait::End,
    BuiltinTrait::Char,
    BuiltinTrait::Iterable,
    BuiltinTrait::Iterator,
    BuiltinTrait::Simd,
    BuiltinTrait::SimdMask,
];

fn builtin_trait_tag(value: BuiltinTrait) -> u8 {
    BUILTIN_TRAITS
        .iter()
        .position(|candidate| *candidate == value)
        .expect("all builtin traits have stable tags") as u8
}

fn read_builtin_trait(cursor: &mut Cursor<&[u8]>) -> Option<BuiltinTrait> {
    BUILTIN_TRAITS.get(read_u8(cursor)? as usize).copied()
}

fn syntax_kind_tag(value: SyntaxKind) -> u8 {
    match value {
        SyntaxKind::Module => 0,
        SyntaxKind::Item => 1,
        SyntaxKind::Stmt => 2,
        SyntaxKind::Expr => 3,
        SyntaxKind::Type => 4,
        SyntaxKind::Pattern => 5,
        SyntaxKind::Param => 6,
        SyntaxKind::Syntax => 7,
        SyntaxKind::Token => 8,
    }
}

fn read_syntax_kind(cursor: &mut Cursor<&[u8]>) -> Option<SyntaxKind> {
    Some(match read_u8(cursor)? {
        0 => SyntaxKind::Module,
        1 => SyntaxKind::Item,
        2 => SyntaxKind::Stmt,
        3 => SyntaxKind::Expr,
        4 => SyntaxKind::Type,
        5 => SyntaxKind::Pattern,
        6 => SyntaxKind::Param,
        7 => SyntaxKind::Syntax,
        8 => SyntaxKind::Token,
        _ => return None,
    })
}

fn signature_set_tag(value: SignatureItemSet) -> u8 {
    match value {
        SignatureItemSet::Functions => 0,
        SignatureItemSet::ExtensionFunctions => 1,
        SignatureItemSet::Values => 2,
        SignatureItemSet::Types => 3,
        SignatureItemSet::Traits => 4,
    }
}

fn checksum(encoded: &[u8]) -> QueryFingerprint {
    let mut builder = QueryFingerprintBuilder::new("nia.signature-type-resolution.entry.v1");
    builder.write_bytes(encoded);
    builder.finish()
}

fn type_lowering_checksum(encoded: &[u8]) -> QueryFingerprint {
    let mut builder = QueryFingerprintBuilder::new("nia.signature-type-lowering.entry.v1");
    builder.write_bytes(encoded);
    builder.finish()
}

fn item_signatures_checksum(encoded: &[u8]) -> QueryFingerprint {
    let mut builder = QueryFingerprintBuilder::new("nia.signature-item-signatures.entry.v1");
    builder.write_bytes(encoded);
    builder.finish()
}

fn extension_trait_solving_facts_checksum(encoded: &[u8]) -> QueryFingerprint {
    let mut builder = QueryFingerprintBuilder::new("nia.extension-trait-solving-facts.entry.v1");
    builder.write_bytes(encoded);
    builder.finish()
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

fn write_parts(encoded: &mut Vec<u8>, parts: [u64; 2]) {
    write_u64(encoded, parts[0]);
    write_u64(encoded, parts[1]);
}

fn read_parts(cursor: &mut Cursor<&[u8]>) -> Option<[u64; 2]> {
    Some([read_u64(cursor)?, read_u64(cursor)?])
}

fn write_string(encoded: &mut Vec<u8>, value: &str) {
    write_u64(encoded, value.len() as u64);
    encoded.extend_from_slice(value.as_bytes());
}

fn read_string(cursor: &mut Cursor<&[u8]>, limit: usize) -> Option<String> {
    let len = read_len(cursor, limit)?;
    let start = usize::try_from(cursor.position()).ok()?;
    let end = start.checked_add(len)?;
    let value = std::str::from_utf8(cursor.get_ref().get(start..end)?).ok()?;
    cursor.set_position(end as u64);
    Some(value.to_string())
}

fn read_len(cursor: &mut Cursor<&[u8]>, limit: usize) -> Option<usize> {
    let len = usize::try_from(read_u64(cursor)?).ok()?;
    (len <= limit).then_some(len)
}

fn write_u64(encoded: &mut Vec<u8>, value: u64) {
    encoded.extend_from_slice(&value.to_le_bytes());
}

fn write_u32(encoded: &mut Vec<u8>, value: u32) {
    encoded.extend_from_slice(&value.to_le_bytes());
}

fn read_u32(cursor: &mut Cursor<&[u8]>) -> Option<u32> {
    let mut bytes = [0_u8; 4];
    cursor.read_exact(&mut bytes).ok()?;
    Some(u32::from_le_bytes(bytes))
}

fn write_u128(encoded: &mut Vec<u8>, value: u128) {
    encoded.extend_from_slice(&value.to_le_bytes());
}

fn read_u128(cursor: &mut Cursor<&[u8]>) -> Option<u128> {
    let mut bytes = [0_u8; 16];
    cursor.read_exact(&mut bytes).ok()?;
    Some(u128::from_le_bytes(bytes))
}

fn read_u64(cursor: &mut Cursor<&[u8]>) -> Option<u64> {
    let mut bytes = [0_u8; 8];
    cursor.read_exact(&mut bytes).ok()?;
    Some(u64::from_le_bytes(bytes))
}

fn read_u8(cursor: &mut Cursor<&[u8]>) -> Option<u8> {
    let mut byte = [0_u8; 1];
    cursor.read_exact(&mut byte).ok()?;
    Some(byte[0])
}

#[cfg(test)]
mod tests {
    use super::*;
    use nia_ids::{ConstExprId, GlobalConstExprId, ModuleIdAllocator};
    use nia_source::{SourceId, SourceIdentity, SourceRevision};
    use std::time::{SystemTime, UNIX_EPOCH};

    #[test]
    fn type_resolution_rehydrates_current_source_module_and_symbol_owners() {
        let root = temp_dir("type_resolution_rehydrate");
        let cache = PersistentSignatureCache::new(root.clone());
        let module = StableModuleKey::from_source_identity(SourceIdentity::new("src/main.nia"));
        let dependency = StableModuleKey::from_source_identity(SourceIdentity::new("src/dep.nia"));
        let source = crate::source_content_fingerprint("type Value = dep::Value");
        let dependency_source = crate::source_content_fingerprint("pub struct Value {}");
        let program_sources = crate::frontend_program_source_fingerprint([
            (&module, source, 23),
            (&dependency, dependency_source, 19),
        ]);
        let namespace = crate::FrontendCacheNamespace::new(
            &nia_target_config::TargetConfig::host(),
            crate::RuntimeModel::Bare,
        );
        let key = crate::FrontendSignatureTypeResolutionCacheKey::new(
            namespace,
            &module,
            SignatureItemSet::Types,
            program_sources,
        );

        let mut old_ids = ModuleIdAllocator::new();
        let old_module = old_ids.allocate();
        let old_dependency = old_ids.allocate();
        let old_version = SourceVersion {
            id: SourceId(3),
            revision: SourceRevision(7),
        };
        let old_store = nia_node_id::NodeStore::new();
        let old_symbols = SymbolTable::new();
        let generic = old_symbols.intern("Length").expect("intern symbol");
        let type_site = NodeSite {
            source_id: old_version.id,
            kind: SyntaxKind::Type,
            position: NodePosition::Span(nia_span::Span::new(5, 10)),
        };
        let qualified_site = NodeSite {
            source_id: old_version.id,
            kind: SyntaxKind::Type,
            position: NodePosition::ChildPath(NodeChildPath::from_steps(vec![1, 2, 3])),
        };
        let const_site = NodeSite {
            source_id: old_version.id,
            kind: SyntaxKind::Expr,
            position: NodePosition::ChildPathRange {
                start: NodeChildPath::from_steps(vec![4]),
                end: NodeChildPath::from_steps(vec![5]),
            },
        };
        let mut const_names = NodeMap::builder(&old_store);
        const_names.insert(
            VersionedNodeKey {
                site: const_site.clone(),
                revision: old_version.revision,
            },
            generic,
        );
        let resolution = TypeResolution {
            node_type_names: HashMap::from([
                (
                    type_site.clone(),
                    TypeNameResolution::Primitive(PrimitiveTypeSpelling::Scalar(
                        PrimitiveTy::Usize,
                    )),
                ),
                (
                    qualified_site.clone(),
                    TypeNameResolution::External(GlobalDefId {
                        module_id: old_dependency,
                        def_id: DefId(41),
                    }),
                ),
            ]),
            node_qualified_type_names: HashMap::from([(
                qualified_site,
                GlobalDefId {
                    module_id: old_dependency,
                    def_id: DefId(41),
                },
            )]),
            node_const_generic_names: const_names.finish(),
            diagnostics: Vec::new(),
        };
        let old_paths = HashMap::from([
            (old_module, "src/main.nia".to_string()),
            (old_dependency, "src/dep.nia".to_string()),
        ]);
        cache
            .publish_type_resolution(
                SignatureTypeResolutionIdentity {
                    key,
                    namespace,
                    module: &module,
                    set: SignatureItemSet::Types,
                    program_sources,
                    source_version: old_version,
                    source_len: 23,
                },
                &resolution,
                &old_paths,
                &old_symbols,
                false,
            )
            .expect("publish cache entry");

        let mut new_ids = ModuleIdAllocator::new();
        let new_dependency = new_ids.allocate();
        let new_module = new_ids.allocate();
        let new_version = SourceVersion {
            id: SourceId(90),
            revision: SourceRevision(2),
        };
        let new_store = nia_node_id::NodeStore::new();
        let new_symbols = SymbolTable::new();
        let modules = HashMap::from([
            ("src/main.nia".to_string(), new_module),
            ("src/dep.nia".to_string(), new_dependency),
        ]);
        let loaded = cache
            .load_type_resolution(
                SignatureTypeResolutionIdentity {
                    key,
                    namespace,
                    module: &module,
                    set: SignatureItemSet::Types,
                    program_sources,
                    source_version: new_version,
                    source_len: 23,
                },
                &modules,
                &new_symbols,
                &new_store,
            )
            .expect("load cache entry");
        let SignatureTypeResolutionLookup::Hit(loaded) = loaded else {
            panic!("expected cache hit");
        };
        assert!(
            loaded
                .node_type_names
                .keys()
                .all(|site| site.source_id == new_version.id)
        );
        assert_eq!(
            loaded.node_type_names.get(&NodeSite {
                source_id: new_version.id,
                kind: SyntaxKind::Type,
                position: NodePosition::ChildPath(NodeChildPath::from_steps(vec![1, 2, 3])),
            }),
            Some(&TypeNameResolution::External(GlobalDefId {
                module_id: new_dependency,
                def_id: DefId(41),
            }))
        );
        let new_const_key = VersionedNodeKey {
            site: NodeSite {
                source_id: new_version.id,
                ..const_site
            },
            revision: new_version.revision,
        };
        let loaded_generic = loaded
            .node_const_generic_names
            .get(&new_const_key)
            .copied()
            .expect("rehydrated const generic");
        assert_eq!(
            new_symbols.resolve(loaded_generic).as_deref(),
            Some("Length")
        );
        assert_eq!(loaded.node_const_generic_names.store_id(), new_store.id());

        let path = cache.type_resolution_path(key);
        let mut corrupt = fs::read(&path).expect("read entry");
        corrupt[0] ^= 0xff;
        fs::write(&path, corrupt).expect("corrupt entry");
        assert_eq!(
            cache
                .load_type_resolution(
                    SignatureTypeResolutionIdentity {
                        key,
                        namespace,
                        module: &module,
                        set: SignatureItemSet::Types,
                        program_sources,
                        source_version: new_version,
                        source_len: 23,
                    },
                    &modules,
                    &new_symbols,
                    &new_store,
                )
                .expect("load corrupt entry"),
            SignatureTypeResolutionLookup::Corrupt
        );
        assert!(!path.exists());
        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn item_signatures_roundtrip_rehydrates_all_stable_fields() {
        let root = temp_dir("item_signatures_rehydrate");
        let cache = PersistentSignatureCache::new(root.clone());
        let module = StableModuleKey::from_source_identity(SourceIdentity::new("src/main.nia"));
        let dependency = StableModuleKey::from_source_identity(SourceIdentity::new("src/dep.nia"));
        let source = crate::source_content_fingerprint("item signature fixture");
        let dependency_source = crate::source_content_fingerprint("dependency fixture");
        let program_sources = crate::frontend_program_source_fingerprint([
            (&module, source, 512),
            (&dependency, dependency_source, 128),
        ]);
        let namespace = crate::FrontendCacheNamespace::new(
            &nia_target_config::TargetConfig::host(),
            crate::RuntimeModel::Bare,
        );
        let key = crate::FrontendSignatureItemSignaturesCacheKey::new(
            namespace,
            &module,
            SignatureItemSet::Types,
            program_sources,
        );

        let mut old_ids = ModuleIdAllocator::new();
        let old_module = old_ids.allocate();
        let old_dependency = old_ids.allocate();
        let old_store = TypeStore::new();
        let append = old_store.append_for_module(old_module);
        let old_symbols = SymbolTable::new();
        let symbol = |text| old_symbols.intern(text).expect("intern fixture symbol");
        let value_name = symbol("value");
        let item_name = symbol("Item");
        let generic_name = symbol("T");
        let primitive = append.intern(TyKind::Primitive(PrimitiveTy::I32));
        let generic = append.intern(TyKind::GenericParam(generic_name));
        let nominal = append.intern(TyKind::Nominal {
            def_id: GlobalDefId {
                module_id: old_dependency,
                def_id: DefId(70),
            },
            args: vec![generic],
            const_args: Vec::new(),
        });
        let trait_ty = append.intern(TyKind::BuiltinTrait {
            trait_id: BuiltinTrait::Iterator,
            args: vec![generic],
        });
        let span = nia_span::Span::new(1, 8);
        let where_predicates = vec![item_signatures::WherePredicateSignature {
            ty: generic,
            bounds: vec![item_signatures::WhereBoundSignature {
                trait_ty,
                associated_type_bindings: vec![item_signatures::AssociatedTypeBindingSignature {
                    name: item_name,
                    ty: primitive,
                    span,
                }],
                span,
            }],
            span,
        }];
        let function = item_signatures::FunctionSignature {
            name: old_symbols.intern("transform").expect("intern function"),
            generics: vec![generic_name],
            generic_params: vec![item_signatures::GenericParamSignature {
                name: generic_name,
                kind: item_signatures::GenericParamSignatureKind::Const { ty: primitive },
            }],
            where_predicates: where_predicates.clone(),
            params: vec![item_signatures::ParamSignature {
                name: Some(value_name),
                receiver: Some(ReceiverKind::RefReadOnly),
                ty: nominal,
                span,
            }],
            return_type: primitive,
            is_extern: false,
            is_const: true,
            is_variadic: false,
            attributes: vec![item_signatures::FunctionAttribute::Builtin(
                BuiltinFunction::SizeOf,
            )],
            has_body: true,
            span,
        };
        let field = item_signatures::FieldSignature {
            def_id: DefId(11),
            name: value_name,
            ty: generic,
            span,
        };
        let signatures = ItemSignatures {
            functions: HashMap::from([(DefId(1), function.clone())]),
            structs: HashMap::from([(
                DefId(2),
                item_signatures::StructSignature {
                    generics: vec![generic_name],
                    where_predicates: where_predicates.clone(),
                    fields: vec![field.clone()],
                    is_extern: false,
                    span,
                },
            )]),
            unions: HashMap::from([(
                DefId(3),
                item_signatures::UnionSignature {
                    generics: Vec::new(),
                    where_predicates: Vec::new(),
                    fields: vec![field],
                    is_extern: true,
                    span,
                },
            )]),
            traits: HashMap::from([(
                DefId(4),
                item_signatures::TraitSignature {
                    generics: vec![generic_name],
                    where_predicates: where_predicates.clone(),
                    supertraits: vec![item_signatures::TraitSupertraitSignature {
                        ty: trait_ty,
                        span,
                    }],
                    associated_types: vec![item_signatures::TraitAssociatedTypeSignature {
                        def_id: DefId(41),
                        name: item_name,
                        span,
                    }],
                    associated_values: vec![item_signatures::TraitAssociatedValueSignature {
                        def_id: DefId(42),
                        name: value_name,
                        ty: primitive,
                        span,
                    }],
                    methods: vec![item_signatures::TraitMethodSignature {
                        def_id: DefId(43),
                        name: symbol("next"),
                        signature: function,
                        has_default: true,
                        span,
                    }],
                    builtin: Some(BuiltinTrait::Iterator),
                    span,
                },
            )]),
            trait_impls: vec![item_signatures::TraitImplSignature {
                impl_id: TraitImplId(51),
                builtin: Some("iterator".to_string()),
                generics: vec![generic_name],
                target_ty: nominal,
                trait_ty: Some(trait_ty),
                trait_span: Some(span),
                where_predicates,
                associated_types: vec![item_signatures::TraitImplAssociatedTypeSignature {
                    name: item_name,
                    ty: primitive,
                    span,
                }],
                associated_values: vec![item_signatures::TraitImplAssociatedValueSignature {
                    def_id: DefId(52),
                    name: value_name,
                    visibility: Visibility::PublicPkg,
                    span,
                }],
                methods: vec![item_signatures::TraitImplMethodSignature {
                    def_id: DefId(53),
                    name: symbol("next"),
                    visibility: Visibility::Public,
                    span,
                }],
                span,
            }],
            enums: HashMap::from([(
                DefId(5),
                item_signatures::EnumSignature {
                    backing_type: primitive,
                    is_open: true,
                    variants: vec![item_signatures::EnumVariantSignature {
                        def_id: DefId(54),
                        name: symbol("First"),
                        span,
                    }],
                    span,
                },
            )]),
            type_aliases: HashMap::from([(
                DefId(6),
                item_signatures::TypeAliasSignature {
                    generics: vec![generic_name],
                    target: nominal,
                    span,
                },
            )]),
            globals: HashMap::from([(
                DefId(7),
                item_signatures::GlobalSignature {
                    explicit_type: Some(primitive),
                    is_mutable: true,
                    is_extern: false,
                    span,
                },
            )]),
            consts: HashMap::from([(
                DefId(8),
                item_signatures::ConstSignature {
                    explicit_type: Some(primitive),
                    builtin: Some(BuiltinConstValue::TargetPointerWidth),
                    span,
                },
            )]),
            diagnostics: Vec::new(),
        };
        let old_paths = HashMap::from([
            (old_module, "src/main.nia".to_string()),
            (old_dependency, "src/dep.nia".to_string()),
        ]);
        let old_payload = encode_item_signatures(&signatures, &old_paths, &old_symbols, &old_store)
            .expect("encode old signatures");
        cache
            .publish_item_signatures(
                SignatureItemSignaturesIdentity {
                    key,
                    namespace,
                    module: &module,
                    set: SignatureItemSet::Types,
                    program_sources,
                    source_len: 512,
                },
                &signatures,
                &old_paths,
                &old_symbols,
                &old_store,
                false,
            )
            .expect("publish signatures");

        let mut new_ids = ModuleIdAllocator::new();
        let new_dependency = new_ids.allocate();
        let new_module = new_ids.allocate();
        let new_store = TypeStore::new();
        let new_symbols = SymbolTable::new();
        let modules = HashMap::from([
            ("src/main.nia".to_string(), new_module),
            ("src/dep.nia".to_string(), new_dependency),
        ]);
        let loaded = cache
            .load_item_signatures(
                SignatureItemSignaturesIdentity {
                    key,
                    namespace,
                    module: &module,
                    set: SignatureItemSet::Types,
                    program_sources,
                    source_len: 512,
                },
                &modules,
                &new_symbols,
                &new_store,
            )
            .expect("load signatures");
        let SignatureItemSignaturesLookup::Hit(loaded) = loaded else {
            panic!("expected signatures cache hit");
        };
        assert!(
            loaded
                .type_roots()
                .iter()
                .all(|ty| ty.store_id == new_store.id())
        );
        let new_paths = HashMap::from([
            (new_module, "src/main.nia".to_string()),
            (new_dependency, "src/dep.nia".to_string()),
        ]);
        let new_payload = encode_item_signatures(&loaded, &new_paths, &new_symbols, &new_store)
            .expect("encode rehydrated signatures");
        assert_eq!(old_payload, new_payload);

        let path = cache.item_signatures_path(key);
        let mut corrupt = fs::read(&path).expect("read signatures entry");
        corrupt[0] ^= 0xff;
        fs::write(&path, corrupt).expect("corrupt signatures entry");
        assert_eq!(
            cache
                .load_item_signatures(
                    SignatureItemSignaturesIdentity {
                        key,
                        namespace,
                        module: &module,
                        set: SignatureItemSet::Types,
                        program_sources,
                        source_len: 512,
                    },
                    &modules,
                    &new_symbols,
                    &new_store,
                )
                .expect("load corrupt signatures"),
            SignatureItemSignaturesLookup::Corrupt
        );
        assert!(!path.exists());
        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn extension_trait_facts_roundtrip_rehydrates_stable_owners() {
        let root = temp_dir("extension_trait_facts_rehydrate");
        let cache = PersistentSignatureCache::new(root.clone());
        let module = StableModuleKey::from_source_identity(SourceIdentity::new("src/main.nia"));
        let dependency = StableModuleKey::from_source_identity(SourceIdentity::new("src/dep.nia"));
        let source = crate::source_content_fingerprint("extension trait facts fixture");
        let dependency_source = crate::source_content_fingerprint("dependency fixture");
        let program_sources = crate::frontend_program_source_fingerprint([
            (&module, source, 512),
            (&dependency, dependency_source, 128),
        ]);
        let namespace = crate::FrontendCacheNamespace::new(
            &nia_target_config::TargetConfig::host(),
            crate::RuntimeModel::Bare,
        );
        let key = crate::FrontendExtensionTraitSolvingFactsCacheKey::new(
            namespace,
            &module,
            program_sources,
        );

        let mut old_ids = ModuleIdAllocator::new();
        let old_module = old_ids.allocate();
        let old_dependency = old_ids.allocate();
        let old_store = TypeStore::new();
        let append = old_store.append_for_module(old_module);
        let old_symbols = SymbolTable::new();
        let generic_name = old_symbols.intern("Item").expect("intern generic");
        let associated_name = old_symbols.intern("Output").expect("intern associated");
        let value_name = old_symbols.intern("VALUE").expect("intern value");
        let primitive = append.intern(TyKind::Primitive(PrimitiveTy::Usize));
        let generic = append.intern(TyKind::GenericParam(generic_name));
        let target_ty = append.intern(TyKind::Nominal {
            def_id: GlobalDefId {
                module_id: old_dependency,
                def_id: DefId(31),
            },
            args: vec![generic],
            const_args: Vec::new(),
        });
        let trait_id = TraitId::Source(GlobalDefId {
            module_id: old_dependency,
            def_id: DefId(37),
        });
        let facts = CachedExtensionTraitSolvingFacts {
            trait_impls: vec![item_signatures::ProgramTraitImplSignature {
                module_id: old_module,
                impl_id: TraitImplId(41),
                builtin: Some("iterator".to_string()),
                generics: vec![generic_name],
                target_ty,
                trait_id,
                trait_args: vec![primitive],
                trait_const_args: vec![ConstGenericArg {
                    ty: primitive,
                    value: ConstGenericValue::Int(IntConst::unsigned(8)),
                }],
                where_predicates: vec![item_signatures::WherePredicateSignature {
                    ty: generic,
                    bounds: vec![item_signatures::WhereBoundSignature {
                        trait_ty: primitive,
                        associated_type_bindings: vec![
                            item_signatures::AssociatedTypeBindingSignature {
                                name: associated_name,
                                ty: target_ty,
                                span: nia_span::Span::new(20, 30),
                            },
                        ],
                        span: nia_span::Span::new(15, 31),
                    }],
                    span: nia_span::Span::new(10, 32),
                }],
                associated_types: vec![item_signatures::TraitImplAssociatedTypeSignature {
                    name: associated_name,
                    ty: target_ty,
                    span: nia_span::Span::new(40, 50),
                }],
                associated_values: vec![item_signatures::TraitImplAssociatedValueSignature {
                    def_id: DefId(43),
                    name: value_name,
                    visibility: Visibility::PublicPkg,
                    span: nia_span::Span::new(60, 70),
                }],
            }],
            invalid_trait_impl_method_ids: HashSet::from([GlobalDefId {
                module_id: old_module,
                def_id: DefId(47),
            }]),
        };
        let old_paths = HashMap::from([
            (old_module, "src/main.nia".to_string()),
            (old_dependency, "src/dep.nia".to_string()),
        ]);
        let old_payload = encode_extension_trait_solving_facts(
            &facts,
            &module,
            &old_paths,
            &old_symbols,
            &old_store,
        )
        .expect("encode old extension facts");
        cache
            .publish_extension_trait_solving_facts(
                ExtensionTraitSolvingFactsIdentity {
                    key,
                    namespace,
                    module: &module,
                    program_sources,
                    source_len: 512,
                },
                &facts,
                &old_paths,
                &old_symbols,
                &old_store,
                false,
            )
            .expect("publish extension facts");

        let mut new_ids = ModuleIdAllocator::new();
        let new_dependency = new_ids.allocate();
        let new_module = new_ids.allocate();
        let new_store = TypeStore::new();
        let new_symbols = SymbolTable::new();
        let modules = HashMap::from([
            ("src/main.nia".to_string(), new_module),
            ("src/dep.nia".to_string(), new_dependency),
        ]);
        let loaded = cache
            .load_extension_trait_solving_facts(
                ExtensionTraitSolvingFactsIdentity {
                    key,
                    namespace,
                    module: &module,
                    program_sources,
                    source_len: 512,
                },
                &modules,
                &new_symbols,
                &new_store,
            )
            .expect("load extension facts");
        let ExtensionTraitSolvingFactsLookup::Hit(loaded) = loaded else {
            panic!("expected extension facts cache hit");
        };
        assert_eq!(loaded.trait_impls[0].module_id, new_module);
        assert!(loaded.invalid_trait_impl_method_ids.contains(&GlobalDefId {
            module_id: new_module,
            def_id: DefId(47),
        }));
        assert!(matches!(
            loaded.trait_impls[0].trait_id,
            TraitId::Source(def_id) if def_id.module_id == new_dependency
        ));
        assert!(loaded.trait_impls.iter().all(|signature| {
            signature.target_ty.store_id == new_store.id()
                && signature
                    .trait_args
                    .iter()
                    .all(|ty| ty.store_id == new_store.id())
        }));
        assert_eq!(new_symbols.resolve(generic_name).as_deref(), Some("Item"));
        assert_eq!(
            new_symbols.resolve(associated_name).as_deref(),
            Some("Output")
        );
        assert_eq!(new_symbols.resolve(value_name).as_deref(), Some("VALUE"));
        let new_paths = HashMap::from([
            (new_module, "src/main.nia".to_string()),
            (new_dependency, "src/dep.nia".to_string()),
        ]);
        let new_payload = encode_extension_trait_solving_facts(
            &loaded,
            &module,
            &new_paths,
            &new_symbols,
            &new_store,
        )
        .expect("encode rehydrated extension facts");
        assert_eq!(old_payload, new_payload);

        let path = cache.extension_trait_solving_facts_path(key);
        let mut corrupt = fs::read(&path).expect("read extension facts entry");
        corrupt[0] ^= 0xff;
        fs::write(&path, corrupt).expect("corrupt extension facts entry");
        assert_eq!(
            cache
                .load_extension_trait_solving_facts(
                    ExtensionTraitSolvingFactsIdentity {
                        key,
                        namespace,
                        module: &module,
                        program_sources,
                        source_len: 512,
                    },
                    &modules,
                    &new_symbols,
                    &new_store,
                )
                .expect("load corrupt extension facts"),
            ExtensionTraitSolvingFactsLookup::Corrupt
        );
        assert!(!path.exists());
        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn type_lowering_roundtrip_rehydrates_canonical_type_graph() {
        let root = temp_dir("type_lowering_rehydrate");
        let cache = PersistentSignatureCache::new(root.clone());
        let module = StableModuleKey::from_source_identity(SourceIdentity::new("src/main.nia"));
        let dependency = StableModuleKey::from_source_identity(SourceIdentity::new("src/dep.nia"));
        let source = crate::source_content_fingerprint("signature lowering fixture");
        let dependency_source = crate::source_content_fingerprint("dependency fixture");
        let program_sources = crate::frontend_program_source_fingerprint([
            (&module, source, 512),
            (&dependency, dependency_source, 128),
        ]);
        let namespace = crate::FrontendCacheNamespace::new(
            &nia_target_config::TargetConfig::host(),
            crate::RuntimeModel::Bare,
        );
        let key = crate::FrontendSignatureTypeLoweringCacheKey::new(
            namespace,
            &module,
            SignatureItemSet::Types,
            program_sources,
        );

        let mut old_ids = ModuleIdAllocator::new();
        let old_module = old_ids.allocate();
        let old_dependency = old_ids.allocate();
        let old_version = SourceVersion {
            id: SourceId(4),
            revision: SourceRevision(7),
        };
        let old_store = TypeStore::new();
        let append = old_store.append_for_module(old_module);
        let old_symbols = SymbolTable::new();
        let length = old_symbols.intern("Length").expect("intern length");
        let item = old_symbols.intern("Item").expect("intern item");
        let primitive = append.intern(TyKind::Primitive(PrimitiveTy::Usize));
        let generic = append.intern(TyKind::GenericParam(length));
        let pointer = append.intern(TyKind::Pointer {
            is_readonly: true,
            elem: primitive,
        });
        let array = append.intern(TyKind::Array {
            len: ArrayLenTy::Builtin {
                builtin: LayoutBuiltin::Align,
                ty: pointer,
            },
            elem: generic,
        });
        let nominal = append.intern(TyKind::Nominal {
            def_id: GlobalDefId {
                module_id: old_dependency,
                def_id: DefId(31),
            },
            args: vec![array],
            const_args: vec![
                ConstGenericArg {
                    ty: primitive,
                    value: ConstGenericValue::GenericParam(length),
                },
                ConstGenericArg {
                    ty: primitive,
                    value: ConstGenericValue::Int(IntConst::unsigned(129)),
                },
                ConstGenericArg {
                    ty: primitive,
                    value: ConstGenericValue::Bool(true),
                },
                ConstGenericArg {
                    ty: primitive,
                    value: ConstGenericValue::Char('λ'),
                },
            ],
        });
        let trait_id = TraitId::Source(GlobalDefId {
            module_id: old_dependency,
            def_id: DefId(37),
        });
        let object = append.intern(TyKind::TraitObject {
            is_readonly: false,
            trait_id,
            trait_args: vec![nominal],
            trait_const_args: Vec::new(),
            associated_type_bindings: vec![AssociatedTypeBindingTy {
                trait_id: Some(TraitId::Builtin(BuiltinTrait::Iterator)),
                trait_args: vec![primitive],
                trait_const_args: Vec::new(),
                name: item,
                ty: array,
            }],
        });
        let projection = append.intern(TyKind::Projection {
            self_ty: object,
            trait_id,
            trait_args: vec![nominal],
            trait_const_args: Vec::new(),
            name: item,
        });
        let function = append.intern(TyKind::FunctionPointer {
            params: vec![projection, append.intern(TyKind::SelfParam)],
            return_type: append.intern(TyKind::Optional { elem: nominal }),
            is_variadic: true,
        });
        let roots = [
            function,
            append.intern(TyKind::ConstOnly),
            append.intern(TyKind::VolatilePointer {
                is_readonly: false,
                elem: primitive,
            }),
            append.intern(TyKind::Slice {
                is_readonly: true,
                elem: nominal,
            }),
            append.intern(TyKind::SlicePointee { elem: primitive }),
            append.intern(TyKind::Vector {
                elem: PrimitiveTy::I32,
                lanes: 8,
            }),
            append.intern(TyKind::Range {
                kind: RangeTyKind::ToInclusive,
                bound: Some(primitive),
            }),
            append.intern(TyKind::ErrorUnion {
                error: append.intern(TyKind::Error),
                value: nominal,
            }),
            append.intern(TyKind::BuiltinType(BuiltinType::AsmConfig)),
            append.intern(TyKind::BuiltinTrait {
                trait_id: BuiltinTrait::Sized,
                args: vec![nominal],
            }),
            append.intern(TyKind::TraitObjectPointee {
                trait_id,
                trait_args: vec![primitive],
                trait_const_args: Vec::new(),
                associated_type_bindings: Vec::new(),
            }),
        ];
        let type_uses = roots
            .into_iter()
            .enumerate()
            .map(|(index, ty)| {
                (
                    NodeSite {
                        source_id: old_version.id,
                        kind: SyntaxKind::Type,
                        position: NodePosition::Span(nia_span::Span::new(index * 4, index * 4 + 3)),
                    },
                    ty,
                )
            })
            .collect();
        let lowering = TypeLowering {
            type_uses,
            const_exprs: HashMap::new(),
            const_expr_summaries: HashMap::new(),
            diagnostics: Vec::new(),
        };
        let old_paths = HashMap::from([
            (old_module, "src/main.nia".to_string()),
            (old_dependency, "src/dep.nia".to_string()),
        ]);
        let old_payload =
            encode_type_lowering(&lowering, old_version, &old_paths, &old_symbols, &old_store)
                .expect("encode old lowering");
        cache
            .publish_type_lowering(
                SignatureTypeLoweringIdentity {
                    key,
                    namespace,
                    module: &module,
                    set: SignatureItemSet::Types,
                    program_sources,
                    source_version: old_version,
                    source_len: 512,
                },
                &lowering,
                &old_paths,
                &old_symbols,
                &old_store,
                false,
            )
            .expect("publish lowering");

        let mut new_ids = ModuleIdAllocator::new();
        let new_dependency = new_ids.allocate();
        let new_module = new_ids.allocate();
        let new_version = SourceVersion {
            id: SourceId(90),
            revision: SourceRevision(2),
        };
        let new_store = TypeStore::new();
        let new_symbols = SymbolTable::new();
        let modules = HashMap::from([
            ("src/main.nia".to_string(), new_module),
            ("src/dep.nia".to_string(), new_dependency),
        ]);
        let loaded = cache
            .load_type_lowering(
                SignatureTypeLoweringIdentity {
                    key,
                    namespace,
                    module: &module,
                    set: SignatureItemSet::Types,
                    program_sources,
                    source_version: new_version,
                    source_len: 512,
                },
                &modules,
                &new_symbols,
                &new_store,
            )
            .expect("load lowering");
        let SignatureTypeLoweringLookup::Hit(loaded) = loaded else {
            panic!("expected lowering cache hit");
        };
        assert!(
            loaded
                .type_uses
                .keys()
                .all(|site| site.source_id == new_version.id)
        );
        assert!(
            loaded
                .type_uses
                .values()
                .all(|ty| ty.store_id == new_store.id())
        );
        assert_eq!(new_symbols.resolve(length).as_deref(), Some("Length"));
        assert_eq!(new_symbols.resolve(item).as_deref(), Some("Item"));
        let new_paths = HashMap::from([
            (new_module, "src/main.nia".to_string()),
            (new_dependency, "src/dep.nia".to_string()),
        ]);
        let new_payload =
            encode_type_lowering(&loaded, new_version, &new_paths, &new_symbols, &new_store)
                .expect("encode rehydrated lowering");
        assert_eq!(old_payload, new_payload);

        let path = cache.type_lowering_path(key);
        let mut corrupt = fs::read(&path).expect("read lowering entry");
        corrupt[0] ^= 0xff;
        fs::write(&path, corrupt).expect("corrupt lowering entry");
        assert_eq!(
            cache
                .load_type_lowering(
                    SignatureTypeLoweringIdentity {
                        key,
                        namespace,
                        module: &module,
                        set: SignatureItemSet::Types,
                        program_sources,
                        source_version: new_version,
                        source_len: 512,
                    },
                    &modules,
                    &new_symbols,
                    &new_store,
                )
                .expect("load corrupt lowering"),
            SignatureTypeLoweringLookup::Corrupt
        );
        assert!(!path.exists());
        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn type_lowering_rejects_const_expression_handles() {
        let mut ids = ModuleIdAllocator::new();
        let module_id = ids.allocate();
        let store = TypeStore::new();
        let append = store.append_for_module(module_id);
        let primitive = append.intern(TyKind::Primitive(PrimitiveTy::Usize));
        let array = append.intern(TyKind::Array {
            len: ArrayLenTy::ConstExpr(GlobalConstExprId {
                module_id,
                const_expr_id: ConstExprId(3),
            }),
            elem: primitive,
        });
        let version = SourceVersion {
            id: SourceId(2),
            revision: SourceRevision::INITIAL,
        };
        let lowering = TypeLowering {
            type_uses: HashMap::from([(
                NodeSite {
                    source_id: version.id,
                    kind: SyntaxKind::Type,
                    position: NodePosition::Span(nia_span::Span::new(0, 2)),
                },
                array,
            )]),
            const_exprs: HashMap::new(),
            const_expr_summaries: HashMap::new(),
            diagnostics: Vec::new(),
        };
        assert!(
            encode_type_lowering(
                &lowering,
                version,
                &HashMap::from([(module_id, "src/main.nia".to_string())]),
                &SymbolTable::new(),
                &store,
            )
            .is_err()
        );
    }

    fn temp_dir(name: &str) -> PathBuf {
        std::env::temp_dir().join(format!(
            "nia_signature_cache_{name}_{}_{}",
            std::process::id(),
            SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .expect("system clock")
                .as_nanos()
        ))
    }
}
