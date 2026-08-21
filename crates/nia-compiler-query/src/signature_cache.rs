// SPDX-License-Identifier: GPL-3.0-or-later
//! Persistent frontend cache identities and stable binary codecs.
//!
//! Storage owns atomic filesystem publication. Codec modules own versioned
//! envelopes and bounded decoding; malformed or mismatched entries are cache
//! misses and never partially populate compiler state.

use std::{
    collections::{BTreeMap, HashMap, HashSet},
    fs::{self, File, OpenOptions},
    io::{self, Cursor, Read, Write},
    path::{Path, PathBuf},
    sync::atomic::{AtomicU64, Ordering},
};

use nia_compat::formats::{
    CHECK_CERTIFICATE, EXECUTABLE_VALUE_REF_EDGES, EXTENSION_VALIDATION_DIAGNOSTICS,
    SIGNATURE_ITEM_SIGNATURES, SIGNATURE_TYPE_LOWERING, SIGNATURE_TYPE_RESOLUTION,
};
use nia_diagnostic::{
    Diagnostic, decode_stable_diagnostic_bundle, encode_stable_diagnostic_bundle,
};
use nia_ids::{
    BuiltinConstValue, BuiltinFunction, BuiltinTrait, DefId, GlobalDefId, InternedTyId, ModuleId,
    ReceiverKind, TraitImplId, Visibility,
};
use nia_imports::StableModuleKey;
use nia_item_signatures::{self as item_signatures, ItemSignatures};
use nia_item_tree::SignatureItemSet;
use nia_node_id::{NodeChildPath, NodeMap, NodePosition, NodeSite, SyntaxKind, VersionedNodeKey};
use nia_query::{FingerprintDomain, QueryFingerprint, QueryFingerprintBuilder};
use nia_source::SourceVersion;
use nia_symbol_table::SymbolTable;
use nia_ty::{
    ArrayLenTy, AssociatedTypeBindingTy, BuiltinType, ConstGenericArg, ConstGenericValue, IntConst,
    LayoutBuiltin, PrimitiveTy, PrimitiveTypeSpelling, RangeTyKind, TraitId, TyKind, TypeStore,
};
use nia_type_lower::TypeLowering;
use nia_type_resolve::{TypeNameResolution, TypeResolution};

use crate::{
    FrontendCacheNamespace, FrontendCheckCertificateCacheKey, FrontendCheckInputFingerprint,
    FrontendCheckScope, FrontendExecutableValueRefEdgesCacheKey,
    FrontendExtensionValidationDiagnosticsCacheKey, FrontendProgramSourceFingerprint,
    FrontendSignatureItemSignaturesCacheKey, FrontendSignatureTypeLoweringCacheKey,
    FrontendSignatureTypeResolutionCacheKey,
};
use crate::{
    ProgramDiagnostic, program_diagnostic_bundle::decode_stable_program_diagnostic_bundle,
    program_diagnostic_bundle::encode_stable_program_diagnostic_bundle,
};

mod entry_codec;
mod executable_edges;
#[path = "signature_cache/item_signatures.rs"]
mod item_signature_codec;
mod storage;
mod syntax_resolution;
mod type_graph;

pub(crate) use entry_codec::*;
pub(crate) use executable_edges::*;
pub(crate) use item_signature_codec::*;
pub(crate) use syntax_resolution::*;
pub(crate) use type_graph::*;

const MAX_ENTRY_BYTES: usize = 64 * 1024 * 1024;
const MAX_SEQUENCE_LEN: usize = 1_000_000;
const TYPE_RESOLUTION_ENTRY_DOMAIN: FingerprintDomain =
    FingerprintDomain::new("nia.signature-type-resolution.entry.v1");
const TYPE_LOWERING_ENTRY_DOMAIN: FingerprintDomain =
    FingerprintDomain::new("nia.signature-type-lowering.entry.v1");
const ITEM_SIGNATURES_ENTRY_DOMAIN: FingerprintDomain =
    FingerprintDomain::new("nia.signature-item-signatures.entry.v1");
const EXTENSION_VALIDATION_DIAGNOSTICS_ENTRY_DOMAIN: FingerprintDomain =
    FingerprintDomain::new("nia.extension-validation-diagnostics.entry.v1");
const EXECUTABLE_VALUE_REF_EDGES_ENTRY_DOMAIN: FingerprintDomain =
    FingerprintDomain::new("nia.executable-value-ref-edges.entry.v1");
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
pub(crate) struct ExtensionValidationDiagnosticsIdentity<'a> {
    pub(crate) key: FrontendExtensionValidationDiagnosticsCacheKey,
    pub(crate) namespace: FrontendCacheNamespace,
    pub(crate) module: &'a StableModuleKey,
    pub(crate) program_sources: FrontendProgramSourceFingerprint,
    pub(crate) source_len: usize,
}

#[derive(Debug, Clone, Copy)]
pub(crate) struct ExecutableValueRefEdgesIdentity<'a> {
    pub(crate) key: FrontendExecutableValueRefEdgesCacheKey,
    pub(crate) namespace: FrontendCacheNamespace,
    pub(crate) module: &'a StableModuleKey,
    pub(crate) owner: DefId,
    pub(crate) program_sources: FrontendProgramSourceFingerprint,
}

#[derive(Debug, Clone, Copy)]
pub(crate) struct CheckCertificateIdentity<'a> {
    pub(crate) key: FrontendCheckCertificateCacheKey,
    pub(crate) namespace: FrontendCacheNamespace,
    pub(crate) entry: &'a StableModuleKey,
    pub(crate) input: FrontendCheckInputFingerprint,
    pub(crate) scope: FrontendCheckScope,
    pub(crate) source_lengths: &'a BTreeMap<String, usize>,
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub(crate) struct CachedExecutableValueRefEdges {
    pub(crate) functions: HashSet<GlobalDefId>,
    pub(crate) globals: HashSet<GlobalDefId>,
}

#[derive(Debug, Clone, PartialEq)]
pub(crate) struct CachedCheckCertificate {
    pub(crate) checked_body_count: usize,
    pub(crate) reachable_body_count: usize,
    pub(crate) diagnostics: Vec<ProgramDiagnostic>,
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
pub(crate) enum ExtensionValidationDiagnosticsLookup {
    Hit(Vec<Diagnostic>),
    NotFound,
    Corrupt,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum ExecutableValueRefEdgesLookup {
    Hit(CachedExecutableValueRefEdges),
    NotFound,
    Corrupt,
}

#[derive(Debug, Clone, PartialEq)]
pub(crate) enum CheckCertificateLookup {
    Hit(CachedCheckCertificate),
    NotFound,
    Corrupt,
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
    let mut builder = QueryFingerprintBuilder::new(TYPE_RESOLUTION_ENTRY_DOMAIN);
    builder.write_bytes(encoded);
    builder.finish()
}

fn type_lowering_checksum(encoded: &[u8]) -> QueryFingerprint {
    let mut builder = QueryFingerprintBuilder::new(TYPE_LOWERING_ENTRY_DOMAIN);
    builder.write_bytes(encoded);
    builder.finish()
}

fn item_signatures_checksum(encoded: &[u8]) -> QueryFingerprint {
    let mut builder = QueryFingerprintBuilder::new(ITEM_SIGNATURES_ENTRY_DOMAIN);
    builder.write_bytes(encoded);
    builder.finish()
}

fn extension_validation_diagnostics_checksum(encoded: &[u8]) -> QueryFingerprint {
    let mut builder = QueryFingerprintBuilder::new(EXTENSION_VALIDATION_DIAGNOSTICS_ENTRY_DOMAIN);
    builder.write_bytes(encoded);
    builder.finish()
}

fn executable_value_ref_edges_checksum(encoded: &[u8]) -> QueryFingerprint {
    let mut builder = QueryFingerprintBuilder::new(EXECUTABLE_VALUE_REF_EDGES_ENTRY_DOMAIN);
    builder.write_bytes(encoded);
    builder.finish()
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
    use nia_diagnostic::codes;
    use nia_ids::{ConstExprId, GlobalConstExprId, ModuleIdAllocator};
    use nia_source::{SourceId, SourceIdentity, SourcePath, SourceRevision};
    use std::time::{SystemTime, UNIX_EPOCH};

    include!("signature_cache/tests/test_support.rs");

    #[test]
    fn builtin_trait_cache_tags_keep_removed_slots_invalid() {
        for tag in [23_u8, 24, 25, 26, 27] {
            assert_eq!(read_builtin_trait(&mut Cursor::new([tag].as_slice())), None);
        }
    }

    #[test]
    fn builtin_function_and_const_cache_tags_roundtrip_exhaustively() {
        for value in BuiltinFunction::ALL {
            let tag = builtin_function_tag(value);
            assert_eq!(
                read_builtin_function(&mut Cursor::new([tag].as_slice())),
                Some(value)
            );
        }
        assert_eq!(
            read_builtin_function(&mut Cursor::new([26].as_slice())),
            None
        );

        for value in BuiltinConstValue::ALL {
            let tag = builtin_const_tag(value);
            assert_eq!(
                read_builtin_const(&mut Cursor::new([tag].as_slice())),
                Some(value)
            );
        }
        assert_eq!(read_builtin_const(&mut Cursor::new([7].as_slice())), None);
    }

    #[path = "check_certificate.rs"]
    mod check_certificate;

    #[path = "type_resolution.rs"]
    mod type_resolution;

    #[path = "item_signature_roundtrip.rs"]
    mod item_signature_roundtrip;

    #[path = "extension_contracts.rs"]
    mod extension_contracts;

    #[path = "executable_edges.rs"]
    mod executable_edges;

    #[path = "entry_identity_contracts.rs"]
    mod entry_identity_contracts;

    #[path = "type_lowering.rs"]
    mod type_lowering;
}
