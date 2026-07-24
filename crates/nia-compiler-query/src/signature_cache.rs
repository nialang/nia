use std::{
    collections::{HashMap, HashSet},
    fs::{self, File, OpenOptions},
    io::{self, Cursor, Read, Write},
    path::{Path, PathBuf},
    sync::atomic::{AtomicU64, Ordering},
};

use nia_ids::{BuiltinTrait, DefId, GlobalDefId, ModuleId};
use nia_imports::StableModuleKey;
use nia_item_tree::SignatureItemSet;
use nia_node_id::{NodeChildPath, NodeMap, NodePosition, NodeSite, SyntaxKind, VersionedNodeKey};
use nia_query::{QueryFingerprint, QueryFingerprintBuilder};
use nia_source::SourceVersion;
use nia_symbol_table::SymbolTable;
use nia_ty::{PrimitiveTy, PrimitiveTypeSpelling};
use nia_type_resolve::{TypeNameResolution, TypeResolution};

use crate::{
    FrontendCacheNamespace, FrontendProgramSourceFingerprint,
    FrontendSignatureTypeResolutionCacheKey,
};

const MAGIC: &[u8; 8] = b"NIASR001";
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

#[derive(Debug, Clone, PartialEq)]
pub(crate) enum SignatureTypeResolutionLookup {
    Hit(Box<TypeResolution>),
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
}

fn encode_entry(identity: SignatureTypeResolutionIdentity<'_>, payload: &[u8]) -> Vec<u8> {
    let mut encoded = Vec::new();
    encoded.extend_from_slice(MAGIC);
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
    if encoded.len() < MAGIC.len() + 16 {
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
    if &magic != MAGIC
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
    use nia_ids::ModuleIdAllocator;
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
