// SPDX-License-Identifier: GPL-3.0-or-later
//! Stable, lazily indexed metadata published by compiled Nia packages.

use std::{
    collections::BTreeMap,
    io::{self, Cursor, Read},
};

use nia_compat::{COMPILER_VERSION, formats, toolchain};

/// Current package container schema.
pub const SCHEMA_VERSION: u32 = formats::PACKAGE_METADATA.schema;
/// Maximum accepted complete container size.
pub const MAX_PACKAGE_BYTES: usize = 64 * 1024 * 1024;
const MAX_STRING_BYTES: usize = 16 * 1024 * 1024;
const MAX_ITEMS: usize = 1_000_000;
const HEADER_BYTES: usize = 8 + 4 + 4 + 4;
const SECTION_ENTRY_BYTES: usize = 1 + 8 + 8 + 32;
const INTERFACE_MAGIC: &[u8; 8] = b"NIAINT01";
// Version 2 adds the declaration kind to stable definition identities.
const INTERFACE_SCHEMA: u32 = 2;
const TYPE_GRAPH_MAGIC: &[u8; 8] = b"NIATYP01";
const TYPE_GRAPH_SCHEMA: u32 = 1;

/// Relocation-independent identity of one package.
#[derive(Debug, Clone, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct PackageId {
    pub namespace: String,
    pub name: String,
    pub version: String,
}

/// Stable identity of a definition within a package module.
#[derive(Debug, Clone, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct DefinitionId {
    pub package: PackageId,
    pub module: String,
    pub name: String,
    pub kind: u8,
}

/// One package dependency and the interface section it consumed.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PackageDependency {
    pub package: PackageId,
    pub interface_hash: [u8; 32],
}

/// Public module declaration exported by a package.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ModuleInterface {
    pub path: String,
    pub interface_hash: [u8; 32],
}

/// One target-independent public declaration and its canonical signature.
///
/// The signature bytes are an opaque compiler-owned type graph for this
/// container layer. Their interpretation is versioned by the interface
/// section schema and must not depend on physical source paths.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct InterfaceRecord {
    pub definition: DefinitionId,
    pub signature: Vec<u8>,
}

/// Decoded target-independent interface section.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct InterfaceSection {
    pub records: Vec<InterfaceRecord>,
}

/// Target-independent type node used by published package interfaces.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum StableTypeNode {
    /// Primitive type tag owned by the language ABI.
    Primitive(u8),
    /// Named declaration resolved through a package-stable identity.
    Named(DefinitionId),
    /// Unit type.
    Unit,
    /// Never type.
    Never,
    /// Tuple whose elements refer to earlier nodes.
    Tuple(Vec<u32>),
    /// Array with a bounded constant length.
    Array { element: u32, length: u64 },
    /// Function signature with parameter and result node references.
    Function { parameters: Vec<u32>, result: u32 },
    /// Borrow/reference to an earlier node.
    Reference { target: u32, mutable: bool },
    /// Raw pointer to an earlier node.
    Pointer { target: u32, readonly: bool },
}

/// Canonical, bounded type graph for cross-package signature use.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StableTypeGraph {
    pub nodes: Vec<StableTypeNode>,
    pub roots: Vec<u32>,
}

impl StableTypeGraph {
    /// Validates bounds, node references, and canonical root ordering.
    pub fn validate(&self) -> Result<(), MetadataError> {
        if self.nodes.len() > MAX_ITEMS || self.roots.len() > MAX_ITEMS {
            return Err(MetadataError::TooManyItems);
        }
        let node_count = self.nodes.len() as u32;
        if self.roots.windows(2).any(|pair| pair[0] >= pair[1])
            || self.roots.iter().any(|root| *root >= node_count)
        {
            return Err(MetadataError::InvalidManifest);
        }
        for (index, node) in self.nodes.iter().enumerate() {
            let index = index as u32;
            let mut references = Vec::new();
            match node {
                StableTypeNode::Primitive(tag) if *tag == 0 => {
                    return Err(MetadataError::InvalidManifest);
                }
                StableTypeNode::Primitive(_) | StableTypeNode::Unit | StableTypeNode::Never => {}
                StableTypeNode::Named(definition) => validate_definition(definition)?,
                StableTypeNode::Tuple(elements) => references.extend(elements),
                StableTypeNode::Array { element, .. } => references.push(element),
                StableTypeNode::Function { parameters, result } => {
                    references.extend(parameters);
                    references.push(result);
                }
                StableTypeNode::Reference { target, .. } => references.push(target),
                StableTypeNode::Pointer { target, .. } => references.push(target),
            }
            if references.iter().any(|reference| **reference >= index) {
                return Err(MetadataError::InvalidManifest);
            }
            if references.len() > MAX_ITEMS {
                return Err(MetadataError::TooManyItems);
            }
        }
        Ok(())
    }
}

impl InterfaceSection {
    /// Validates bounded fields and canonical definition ordering.
    pub fn validate(&self) -> Result<(), MetadataError> {
        if self.records.len() > MAX_ITEMS {
            return Err(MetadataError::TooManyItems);
        }
        for record in &self.records {
            validate_definition(&record.definition)?;
            validate_bytes(&record.signature)?;
        }
        if self
            .records
            .windows(2)
            .any(|pair| pair[0].definition >= pair[1].definition)
        {
            return Err(MetadataError::InvalidManifest);
        }
        Ok(())
    }
}

/// Indexed target-independent declarations supplied by one compiled package.
///
/// Entries retain package-owned stable identities. Consumers must remap them
/// before constructing any session-local module, definition, or type handle.
#[derive(Debug, Clone)]
pub struct CompiledPackageInterface {
    manifest: PackageManifest,
    interface: InterfaceSection,
    record_indexes: BTreeMap<DefinitionId, usize>,
}

impl CompiledPackageInterface {
    /// Builds an indexed interface view after validating all manifest bindings.
    pub fn from_artifact(artifact: &PackageArtifact) -> Result<Self, MetadataError> {
        let interface = artifact.interface()?.unwrap_or(InterfaceSection {
            records: Vec::new(),
        });
        let record_indexes = interface
            .records
            .iter()
            .enumerate()
            .map(|(index, record)| (record.definition.clone(), index))
            .collect();
        Ok(Self {
            manifest: artifact.manifest().clone(),
            interface,
            record_indexes,
        })
    }

    /// Returns the validated package manifest.
    pub fn manifest(&self) -> &PackageManifest {
        &self.manifest
    }

    /// Returns canonical records in definition-identity order.
    pub fn records(&self) -> &[InterfaceRecord] {
        &self.interface.records
    }

    /// Resolves one stable definition identity without source loading.
    pub fn definition(&self, definition: &DefinitionId) -> Option<&InterfaceRecord> {
        self.record_indexes
            .get(definition)
            .and_then(|index| self.interface.records.get(*index))
    }

    /// Returns the package's public records for one stable module path.
    pub fn module_records(&self, module: &str) -> impl Iterator<Item = &InterfaceRecord> {
        self.interface
            .records
            .iter()
            .filter(move |record| record.definition.module == module)
    }
}

/// Kind of lazily loaded package payload.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
#[repr(u8)]
pub enum SectionKind {
    Interface = 1,
    Templates = 2,
    Native = 3,
}
impl SectionKind {
    fn decode(value: u8) -> Option<Self> {
        match value {
            1 => Some(Self::Interface),
            2 => Some(Self::Templates),
            3 => Some(Self::Native),
            _ => None,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct SectionEntry {
    kind: SectionKind,
    offset: u64,
    length: u64,
    hash: [u8; 32],
}

/// Target-independent package publication manifest.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PackageManifest {
    pub package: PackageId,
    pub compiler_version: String,
    pub std_schema: u32,
    pub schema_version: u32,
    pub dependencies: Vec<PackageDependency>,
    pub modules: Vec<ModuleInterface>,
}

impl PackageManifest {
    /// Constructs a manifest for the current compiler/toolchain identity.
    pub fn current(package: PackageId) -> Self {
        Self {
            package,
            compiler_version: COMPILER_VERSION.to_owned(),
            std_schema: toolchain::STANDARD_LIBRARY,
            schema_version: SCHEMA_VERSION,
            dependencies: Vec::new(),
            modules: Vec::new(),
        }
    }
    fn encode_into(&self, output: &mut Vec<u8>) -> Result<(), MetadataError> {
        self.validate()?;
        put_id(output, &self.package)?;
        put_string(output, &self.compiler_version)?;
        put_u32(output, self.std_schema);
        put_list_len(output, self.dependencies.len())?;
        for dependency in &self.dependencies {
            put_id(output, &dependency.package)?;
            output.extend_from_slice(&dependency.interface_hash);
        }
        put_list_len(output, self.modules.len())?;
        for module in &self.modules {
            put_string(output, &module.path)?;
            output.extend_from_slice(&module.interface_hash);
        }
        Ok(())
    }
    fn decode_from(cursor: &mut Cursor<&[u8]>) -> Result<Self, MetadataError> {
        let manifest = Self {
            package: get_id(cursor)?,
            compiler_version: get_string(cursor)?,
            std_schema: get_u32(cursor)?,
            schema_version: SCHEMA_VERSION,
            dependencies: get_dependencies(cursor)?,
            modules: get_modules(cursor)?,
        };
        manifest.validate()?;
        Ok(manifest)
    }
    /// Validates canonical ordering, uniqueness, and bounded field sizes.
    pub fn validate(&self) -> Result<(), MetadataError> {
        validate_id(&self.package)?;
        validate_string(&self.compiler_version)?;
        if self.schema_version != SCHEMA_VERSION {
            return Err(MetadataError::Schema(self.schema_version));
        }
        if self.dependencies.len() > MAX_ITEMS || self.modules.len() > MAX_ITEMS {
            return Err(MetadataError::TooManyItems);
        }
        for dependency in &self.dependencies {
            validate_id(&dependency.package)?;
        }
        if self
            .dependencies
            .windows(2)
            .any(|pair| pair[0].package >= pair[1].package)
            || self
                .modules
                .windows(2)
                .any(|pair| pair[0].path >= pair[1].path)
        {
            return Err(MetadataError::InvalidManifest);
        }
        for module in &self.modules {
            validate_string(&module.path)?;
        }
        Ok(())
    }
}

/// A package container with a decoded manifest and lazy section directory.
#[derive(Debug, Clone)]
pub struct PackageArtifact {
    bytes: Vec<u8>,
    manifest: PackageManifest,
    sections: Vec<SectionEntry>,
}

impl PackageArtifact {
    /// Opens a bounded container, decoding only its manifest and section index.
    pub fn open(bytes: Vec<u8>) -> Result<Self, MetadataError> {
        if bytes.len() > MAX_PACKAGE_BYTES {
            return Err(MetadataError::TooLarge);
        }
        let mut cursor = Cursor::new(bytes.as_slice());
        let mut magic = [0; 8];
        read_exact(&mut cursor, &mut magic)?;
        if magic != *formats::PACKAGE_METADATA.magic {
            return Err(MetadataError::BadMagic);
        }
        let schema = get_u32(&mut cursor)?;
        if schema != SCHEMA_VERSION {
            return Err(MetadataError::Schema(schema));
        }
        let manifest_len = bounded_len(get_u32(&mut cursor)?)?;
        let section_count = bounded_count(get_u32(&mut cursor)?)?;
        let manifest_start = cursor.position() as usize;
        let manifest_end = manifest_start
            .checked_add(manifest_len)
            .ok_or(MetadataError::TooLarge)?;
        if manifest_end > bytes.len() {
            return Err(MetadataError::Truncated);
        }
        let manifest =
            PackageManifest::decode_from(&mut Cursor::new(&bytes[manifest_start..manifest_end]))?;
        cursor.set_position(manifest_end as u64);
        let directory_end = manifest_end
            .checked_add(
                section_count
                    .checked_mul(SECTION_ENTRY_BYTES)
                    .ok_or(MetadataError::TooLarge)?,
            )
            .ok_or(MetadataError::TooLarge)?;
        if directory_end > bytes.len() {
            return Err(MetadataError::Truncated);
        }
        if section_count == 0 && directory_end != bytes.len() {
            return Err(MetadataError::InvalidManifest);
        }
        let mut sections = Vec::with_capacity(section_count);
        let mut previous = None;
        for _ in 0..section_count {
            let kind =
                SectionKind::decode(read_u8(&mut cursor)?).ok_or(MetadataError::InvalidManifest)?;
            let offset = get_u64(&mut cursor)?;
            let length = get_u64(&mut cursor)?;
            let mut hash = [0; 32];
            read_exact(&mut cursor, &mut hash)?;
            if previous.is_some_and(|value| value >= kind)
                || offset < directory_end as u64
                || length > MAX_PACKAGE_BYTES as u64
                || offset
                    .checked_add(length)
                    .is_none_or(|end| end > bytes.len() as u64)
            {
                return Err(MetadataError::InvalidManifest);
            }
            previous = Some(kind);
            sections.push(SectionEntry {
                kind,
                offset,
                length,
                hash,
            });
        }
        Ok(Self {
            bytes,
            manifest,
            sections,
        })
    }
    /// Returns the validated target-independent manifest.
    pub fn manifest(&self) -> &PackageManifest {
        &self.manifest
    }
    /// Returns a section's bytes after verifying its hash, without copying.
    pub fn section(&self, kind: SectionKind) -> Result<Option<&[u8]>, MetadataError> {
        let Some(entry) = self.sections.iter().find(|entry| entry.kind == kind) else {
            return Ok(None);
        };
        let start = usize::try_from(entry.offset).map_err(|_| MetadataError::TooLarge)?;
        let end =
            usize::try_from(entry.offset + entry.length).map_err(|_| MetadataError::TooLarge)?;
        let bytes = &self.bytes[start..end];
        if blake3::hash(bytes).as_bytes() != &entry.hash {
            return Err(MetadataError::Integrity);
        }
        Ok(Some(bytes))
    }

    /// Verifies every published section without retaining section payloads.
    pub fn validate_sections(&self) -> Result<(), MetadataError> {
        for entry in &self.sections {
            self.section(entry.kind)?
                .ok_or(MetadataError::InvalidManifest)?;
        }
        Ok(())
    }

    /// Decodes the optional target-independent declaration section.
    pub fn interface(&self) -> Result<Option<InterfaceSection>, MetadataError> {
        let interface = self
            .section(SectionKind::Interface)?
            .map(decode_interface)
            .transpose()?;
        if let Some(interface) = &interface {
            validate_interface_manifest(&self.manifest, interface)?;
        }
        Ok(interface)
    }
}

/// Encodes a manifest-only package artifact.
pub fn encode(manifest: &PackageManifest) -> Result<Vec<u8>, MetadataError> {
    encode_artifact(manifest, &[])
}
/// Encodes a package artifact with lazy sections in canonical order.
pub fn encode_artifact(
    manifest: &PackageManifest,
    sections: &[(SectionKind, &[u8])],
) -> Result<Vec<u8>, MetadataError> {
    manifest.validate()?;
    let mut manifest_bytes = Vec::new();
    manifest.encode_into(&mut manifest_bytes)?;
    let mut sections = sections.to_vec();
    sections.sort_by_key(|(kind, _)| *kind);
    if sections.windows(2).any(|pair| pair[0].0 == pair[1].0) {
        return Err(MetadataError::InvalidManifest);
    }
    let directory_end = HEADER_BYTES
        .checked_add(manifest_bytes.len())
        .and_then(|value| value.checked_add(sections.len().checked_mul(SECTION_ENTRY_BYTES)?))
        .ok_or(MetadataError::TooLarge)?;
    let mut output = Vec::with_capacity(directory_end);
    output.extend_from_slice(formats::PACKAGE_METADATA.magic);
    put_u32(&mut output, SCHEMA_VERSION);
    put_u32(
        &mut output,
        u32::try_from(manifest_bytes.len()).map_err(|_| MetadataError::TooLarge)?,
    );
    put_u32(
        &mut output,
        u32::try_from(sections.len()).map_err(|_| MetadataError::TooManyItems)?,
    );
    output.extend_from_slice(&manifest_bytes);
    let mut offset = directory_end as u64;
    for (kind, bytes) in &sections {
        let length = u64::try_from(bytes.len()).map_err(|_| MetadataError::TooLarge)?;
        output.push(*kind as u8);
        output.extend_from_slice(&offset.to_le_bytes());
        output.extend_from_slice(&length.to_le_bytes());
        output.extend_from_slice(blake3::hash(bytes).as_bytes());
        offset = offset.checked_add(length).ok_or(MetadataError::TooLarge)?;
    }
    for (_, bytes) in sections {
        output.extend_from_slice(bytes);
    }
    if output.len() > MAX_PACKAGE_BYTES {
        return Err(MetadataError::TooLarge);
    }
    Ok(output)
}

/// Returns the stable content hash used by package manifests for one section.
pub fn section_hash(bytes: &[u8]) -> [u8; 32] {
    *blake3::hash(bytes).as_bytes()
}

/// Computes the manifest hash for one module's public interface records.
pub fn interface_module_hash(
    interface: &InterfaceSection,
    module: &str,
) -> Result<[u8; 32], MetadataError> {
    let records = interface
        .records
        .iter()
        .filter(|record| record.definition.module == module)
        .cloned()
        .collect();
    Ok(section_hash(&encode_interface(&InterfaceSection {
        records,
    })?))
}
/// Decodes a complete manifest-only artifact.
pub fn decode(bytes: &[u8]) -> Result<PackageManifest, MetadataError> {
    Ok(PackageArtifact::open(bytes.to_vec())?.manifest)
}

/// Encodes a canonical target-independent declaration section.
pub fn encode_interface(section: &InterfaceSection) -> Result<Vec<u8>, MetadataError> {
    section.validate()?;
    let mut output = Vec::new();
    output.extend_from_slice(INTERFACE_MAGIC);
    put_u32(&mut output, INTERFACE_SCHEMA);
    put_list_len(&mut output, section.records.len())?;
    for record in &section.records {
        put_id(&mut output, &record.definition.package)?;
        put_string(&mut output, &record.definition.module)?;
        put_string(&mut output, &record.definition.name)?;
        output.push(record.definition.kind);
        put_bytes(&mut output, &record.signature)?;
    }
    if output.len() > MAX_PACKAGE_BYTES {
        return Err(MetadataError::TooLarge);
    }
    Ok(output)
}

/// Decodes and validates a target-independent declaration section.
pub fn decode_interface(bytes: &[u8]) -> Result<InterfaceSection, MetadataError> {
    if bytes.len() > MAX_PACKAGE_BYTES {
        return Err(MetadataError::TooLarge);
    }
    let mut cursor = Cursor::new(bytes);
    let mut magic = [0; 8];
    read_exact(&mut cursor, &mut magic)?;
    if magic != *INTERFACE_MAGIC {
        return Err(MetadataError::BadMagic);
    }
    let schema = get_u32(&mut cursor)?;
    if schema != INTERFACE_SCHEMA {
        return Err(MetadataError::Schema(schema));
    }
    let count = bounded_count(get_u32(&mut cursor)?)?;
    let mut records = Vec::with_capacity(count);
    for _ in 0..count {
        records.push(InterfaceRecord {
            definition: DefinitionId {
                package: get_id(&mut cursor)?,
                module: get_string(&mut cursor)?,
                name: get_string(&mut cursor)?,
                kind: read_u8(&mut cursor)?,
            },
            signature: get_bytes(&mut cursor)?,
        });
    }
    if cursor.position() != bytes.len() as u64 {
        return Err(MetadataError::InvalidManifest);
    }
    let section = InterfaceSection { records };
    section.validate()?;
    Ok(section)
}

/// Encodes a canonical stable type graph.
pub fn encode_type_graph(graph: &StableTypeGraph) -> Result<Vec<u8>, MetadataError> {
    graph.validate()?;
    let mut output = Vec::new();
    output.extend_from_slice(TYPE_GRAPH_MAGIC);
    put_u32(&mut output, TYPE_GRAPH_SCHEMA);
    put_list_len(&mut output, graph.nodes.len())?;
    put_list_len(&mut output, graph.roots.len())?;
    for root in &graph.roots {
        put_u32(&mut output, *root);
    }
    for node in &graph.nodes {
        match node {
            StableTypeNode::Primitive(tag) => {
                output.push(1);
                output.push(*tag);
            }
            StableTypeNode::Named(definition) => {
                output.push(2);
                put_definition(&mut output, definition)?;
            }
            StableTypeNode::Unit => output.push(3),
            StableTypeNode::Never => output.push(4),
            StableTypeNode::Tuple(elements) => {
                output.push(5);
                put_list_len(&mut output, elements.len())?;
                for element in elements {
                    put_u32(&mut output, *element);
                }
            }
            StableTypeNode::Array { element, length } => {
                output.push(6);
                put_u32(&mut output, *element);
                output.extend_from_slice(&length.to_le_bytes());
            }
            StableTypeNode::Function { parameters, result } => {
                output.push(7);
                put_list_len(&mut output, parameters.len())?;
                for parameter in parameters {
                    put_u32(&mut output, *parameter);
                }
                put_u32(&mut output, *result);
            }
            StableTypeNode::Reference { target, mutable } => {
                output.push(8);
                put_u32(&mut output, *target);
                output.push(u8::from(*mutable));
            }
            StableTypeNode::Pointer { target, readonly } => {
                output.push(9);
                put_u32(&mut output, *target);
                output.push(u8::from(*readonly));
            }
        }
    }
    if output.len() > MAX_PACKAGE_BYTES {
        return Err(MetadataError::TooLarge);
    }
    Ok(output)
}

/// Decodes and validates a canonical stable type graph.
pub fn decode_type_graph(bytes: &[u8]) -> Result<StableTypeGraph, MetadataError> {
    if bytes.len() > MAX_PACKAGE_BYTES {
        return Err(MetadataError::TooLarge);
    }
    let mut cursor = Cursor::new(bytes);
    let mut magic = [0; 8];
    read_exact(&mut cursor, &mut magic)?;
    if magic != *TYPE_GRAPH_MAGIC {
        return Err(MetadataError::BadMagic);
    }
    let schema = get_u32(&mut cursor)?;
    if schema != TYPE_GRAPH_SCHEMA {
        return Err(MetadataError::Schema(schema));
    }
    let node_count = bounded_count(get_u32(&mut cursor)?)?;
    let root_count = bounded_count(get_u32(&mut cursor)?)?;
    let roots = (0..root_count)
        .map(|_| get_u32(&mut cursor))
        .collect::<Result<Vec<_>, _>>()?;
    let mut nodes = Vec::with_capacity(node_count);
    for _ in 0..node_count {
        let tag = read_u8(&mut cursor)?;
        nodes.push(match tag {
            1 => StableTypeNode::Primitive(read_u8(&mut cursor)?),
            2 => StableTypeNode::Named(read_definition(&mut cursor)?),
            3 => StableTypeNode::Unit,
            4 => StableTypeNode::Never,
            5 => StableTypeNode::Tuple(read_refs(&mut cursor)?),
            6 => StableTypeNode::Array {
                element: get_u32(&mut cursor)?,
                length: get_u64(&mut cursor)?,
            },
            7 => StableTypeNode::Function {
                parameters: read_refs(&mut cursor)?,
                result: get_u32(&mut cursor)?,
            },
            8 => StableTypeNode::Reference {
                target: get_u32(&mut cursor)?,
                mutable: match read_u8(&mut cursor)? {
                    0 => false,
                    1 => true,
                    _ => return Err(MetadataError::InvalidManifest),
                },
            },
            9 => StableTypeNode::Pointer {
                target: get_u32(&mut cursor)?,
                readonly: match read_u8(&mut cursor)? {
                    0 => false,
                    1 => true,
                    _ => return Err(MetadataError::InvalidManifest),
                },
            },
            _ => return Err(MetadataError::InvalidManifest),
        });
    }
    if cursor.position() != bytes.len() as u64 {
        return Err(MetadataError::InvalidManifest);
    }
    let graph = StableTypeGraph { nodes, roots };
    graph.validate()?;
    Ok(graph)
}

fn validate_interface_manifest(
    manifest: &PackageManifest,
    interface: &InterfaceSection,
) -> Result<(), MetadataError> {
    for record in &interface.records {
        if record.definition.package != manifest.package
            || manifest
                .modules
                .binary_search_by(|module| module.path.cmp(&record.definition.module))
                .is_err()
        {
            return Err(MetadataError::InvalidManifest);
        }
    }
    for module in &manifest.modules {
        if interface_module_hash(interface, &module.path)? != module.interface_hash {
            return Err(MetadataError::Integrity);
        }
    }
    Ok(())
}

/// Errors returned by package metadata validation and decoding.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum MetadataError {
    BadMagic,
    Schema(u32),
    TooLarge,
    TooManyItems,
    Truncated,
    InvalidString,
    InvalidManifest,
    Integrity,
    Io,
}
impl std::fmt::Display for MetadataError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "package metadata error: {self:?}")
    }
}
impl std::error::Error for MetadataError {}

fn validate_id(id: &PackageId) -> Result<(), MetadataError> {
    validate_string(&id.namespace)?;
    validate_string(&id.name)?;
    validate_string(&id.version)
}
fn validate_definition(definition: &DefinitionId) -> Result<(), MetadataError> {
    validate_id(&definition.package)?;
    validate_string(&definition.module)?;
    validate_string(&definition.name)?;
    (definition.kind != 0)
        .then_some(())
        .ok_or(MetadataError::InvalidManifest)
}
fn validate_string(value: &str) -> Result<(), MetadataError> {
    if value.is_empty() || value.len() > MAX_STRING_BYTES || value.as_bytes().contains(&0) {
        Err(MetadataError::InvalidString)
    } else {
        Ok(())
    }
}
fn put_id(output: &mut Vec<u8>, id: &PackageId) -> Result<(), MetadataError> {
    put_string(output, &id.namespace)?;
    put_string(output, &id.name)?;
    put_string(output, &id.version)
}
fn put_definition(output: &mut Vec<u8>, definition: &DefinitionId) -> Result<(), MetadataError> {
    validate_definition(definition)?;
    put_id(output, &definition.package)?;
    put_string(output, &definition.module)?;
    put_string(output, &definition.name)?;
    output.push(definition.kind);
    Ok(())
}
fn read_definition(cursor: &mut Cursor<&[u8]>) -> Result<DefinitionId, MetadataError> {
    let definition = DefinitionId {
        package: get_id(cursor)?,
        module: get_string(cursor)?,
        name: get_string(cursor)?,
        kind: read_u8(cursor)?,
    };
    validate_definition(&definition)?;
    Ok(definition)
}
fn read_refs(cursor: &mut Cursor<&[u8]>) -> Result<Vec<u32>, MetadataError> {
    let count = bounded_count(get_u32(cursor)?)?;
    (0..count).map(|_| get_u32(cursor)).collect()
}
fn get_id(cursor: &mut Cursor<&[u8]>) -> Result<PackageId, MetadataError> {
    Ok(PackageId {
        namespace: get_string(cursor)?,
        name: get_string(cursor)?,
        version: get_string(cursor)?,
    })
}
fn put_string(output: &mut Vec<u8>, value: &str) -> Result<(), MetadataError> {
    validate_string(value)?;
    put_u32(
        output,
        u32::try_from(value.len()).map_err(|_| MetadataError::TooLarge)?,
    );
    output.extend_from_slice(value.as_bytes());
    Ok(())
}
fn validate_bytes(value: &[u8]) -> Result<(), MetadataError> {
    if value.len() > MAX_STRING_BYTES {
        Err(MetadataError::TooLarge)
    } else {
        Ok(())
    }
}
fn put_bytes(output: &mut Vec<u8>, value: &[u8]) -> Result<(), MetadataError> {
    validate_bytes(value)?;
    put_u32(
        output,
        u32::try_from(value.len()).map_err(|_| MetadataError::TooLarge)?,
    );
    output.extend_from_slice(value);
    Ok(())
}
fn get_bytes(cursor: &mut Cursor<&[u8]>) -> Result<Vec<u8>, MetadataError> {
    let length = bounded_len(get_u32(cursor)?)?;
    let mut bytes = vec![0; length];
    read_exact(cursor, &mut bytes)?;
    Ok(bytes)
}
fn get_string(cursor: &mut Cursor<&[u8]>) -> Result<String, MetadataError> {
    let length = bounded_len(get_u32(cursor)?)?;
    if length == 0 {
        return Err(MetadataError::InvalidString);
    }
    let mut bytes = vec![0; length];
    read_exact(cursor, &mut bytes)?;
    String::from_utf8(bytes).map_err(|_| MetadataError::InvalidString)
}
fn get_dependencies(cursor: &mut Cursor<&[u8]>) -> Result<Vec<PackageDependency>, MetadataError> {
    let count = bounded_count(get_u32(cursor)?)?;
    (0..count)
        .map(|_| {
            let package = get_id(cursor)?;
            let mut interface_hash = [0; 32];
            read_exact(cursor, &mut interface_hash)?;
            Ok(PackageDependency {
                package,
                interface_hash,
            })
        })
        .collect()
}
fn get_modules(cursor: &mut Cursor<&[u8]>) -> Result<Vec<ModuleInterface>, MetadataError> {
    let count = bounded_count(get_u32(cursor)?)?;
    (0..count)
        .map(|_| {
            let path = get_string(cursor)?;
            let mut interface_hash = [0; 32];
            read_exact(cursor, &mut interface_hash)?;
            Ok(ModuleInterface {
                path,
                interface_hash,
            })
        })
        .collect()
}
fn put_list_len(output: &mut Vec<u8>, count: usize) -> Result<(), MetadataError> {
    if count > MAX_ITEMS {
        return Err(MetadataError::TooManyItems);
    }
    put_u32(
        output,
        u32::try_from(count).map_err(|_| MetadataError::TooManyItems)?,
    );
    Ok(())
}
fn bounded_len(value: u32) -> Result<usize, MetadataError> {
    let value = usize::try_from(value).map_err(|_| MetadataError::TooLarge)?;
    (value <= MAX_PACKAGE_BYTES)
        .then_some(value)
        .ok_or(MetadataError::TooLarge)
}
fn bounded_count(value: u32) -> Result<usize, MetadataError> {
    let value = usize::try_from(value).map_err(|_| MetadataError::TooManyItems)?;
    (value <= MAX_ITEMS)
        .then_some(value)
        .ok_or(MetadataError::TooManyItems)
}
fn put_u32(output: &mut Vec<u8>, value: u32) {
    output.extend_from_slice(&value.to_le_bytes());
}
fn get_u32(cursor: &mut Cursor<&[u8]>) -> Result<u32, MetadataError> {
    let mut bytes = [0; 4];
    read_exact(cursor, &mut bytes)?;
    Ok(u32::from_le_bytes(bytes))
}
fn get_u64(cursor: &mut Cursor<&[u8]>) -> Result<u64, MetadataError> {
    let mut bytes = [0; 8];
    read_exact(cursor, &mut bytes)?;
    Ok(u64::from_le_bytes(bytes))
}
fn read_u8(cursor: &mut Cursor<&[u8]>) -> Result<u8, MetadataError> {
    let mut byte = [0; 1];
    read_exact(cursor, &mut byte)?;
    Ok(byte[0])
}
fn read_exact(cursor: &mut Cursor<&[u8]>, bytes: &mut [u8]) -> Result<(), MetadataError> {
    cursor.read_exact(bytes).map_err(|error| {
        if error.kind() == io::ErrorKind::UnexpectedEof {
            MetadataError::Truncated
        } else {
            MetadataError::Io
        }
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    fn sample() -> PackageManifest {
        let mut manifest = PackageManifest::current(PackageId {
            namespace: "nia".into(),
            name: "std".into(),
            version: "0.2".into(),
        });
        manifest.modules.push(ModuleInterface {
            path: "std/io".into(),
            interface_hash: [7; 32],
        });
        manifest
    }
    #[test]
    fn round_trip_and_lazy_sections() {
        let manifest = sample();
        let bytes = encode_artifact(
            &manifest,
            &[
                (SectionKind::Interface, b"interface"),
                (SectionKind::Templates, b"templates"),
            ],
        )
        .unwrap();
        let artifact = PackageArtifact::open(bytes).unwrap();
        assert_eq!(artifact.manifest(), &manifest);
        assert_eq!(
            artifact.section(SectionKind::Interface).unwrap(),
            Some(&b"interface"[..])
        );
        assert_eq!(artifact.section(SectionKind::Native).unwrap(), None);
    }
    #[test]
    fn rejects_corruption_and_noncanonical_manifest() {
        let manifest = sample();
        let mut bytes =
            encode_artifact(&manifest, &[(SectionKind::Interface, b"payload")]).unwrap();
        let artifact = PackageArtifact::open(bytes.clone()).unwrap();
        let offset = artifact.sections[0].offset as usize;
        bytes[offset] ^= 1;
        assert_eq!(
            PackageArtifact::open(bytes.clone())
                .unwrap()
                .section(SectionKind::Interface),
            Err(MetadataError::Integrity)
        );
        let mut trailing = encode(&manifest).unwrap();
        trailing.push(0);
        assert!(matches!(
            PackageArtifact::open(trailing),
            Err(MetadataError::InvalidManifest)
        ));
    }
    #[test]
    fn rejects_unsorted_duplicate_modules() {
        let mut manifest = sample();
        manifest.modules.push(ModuleInterface {
            path: "std/io".into(),
            interface_hash: [8; 32],
        });
        assert_eq!(encode(&manifest), Err(MetadataError::InvalidManifest));
    }

    #[test]
    fn interface_section_round_trips_canonical_records() {
        let package = sample().package;
        let section = InterfaceSection {
            records: vec![InterfaceRecord {
                definition: DefinitionId {
                    package,
                    module: "std/io".into(),
                    name: "write".into(),
                    kind: 2,
                },
                signature: b"fn(Text) Unit".to_vec(),
            }],
        };
        let bytes = encode_interface(&section).unwrap();
        assert_eq!(decode_interface(&bytes).unwrap(), section);
        let mut manifest = PackageManifest::current(PackageId {
            namespace: "nia".into(),
            name: "std".into(),
            version: "0.2".into(),
        });
        manifest.modules.push(ModuleInterface {
            path: "std/io".into(),
            interface_hash: interface_module_hash(&section, "std/io").unwrap(),
        });
        let artifact = PackageArtifact::open(
            encode_artifact(&manifest, &[(SectionKind::Interface, &bytes)]).unwrap(),
        )
        .unwrap();
        assert_eq!(artifact.interface().unwrap(), Some(section));
    }

    #[test]
    fn interface_section_rejects_noncanonical_or_trailing_bytes() {
        let package = sample().package;
        let first = InterfaceRecord {
            definition: DefinitionId {
                package: package.clone(),
                module: "m".into(),
                name: "a".into(),
                kind: 2,
            },
            signature: vec![1],
        };
        let second = InterfaceRecord {
            definition: DefinitionId {
                package,
                module: "m".into(),
                name: "a".into(),
                kind: 2,
            },
            signature: vec![2],
        };
        assert_eq!(
            encode_interface(&InterfaceSection {
                records: vec![first, second],
            }),
            Err(MetadataError::InvalidManifest)
        );
        let mut bytes = encode_interface(&InterfaceSection { records: vec![] }).unwrap();
        bytes.push(0);
        assert_eq!(
            decode_interface(&bytes),
            Err(MetadataError::InvalidManifest)
        );
    }

    #[test]
    fn stable_type_graph_round_trips_and_rejects_forward_references() {
        let package = sample().package;
        let named = DefinitionId {
            package,
            module: "std/io".into(),
            name: "Text".into(),
            kind: 5,
        };
        let graph = StableTypeGraph {
            nodes: vec![
                StableTypeNode::Primitive(1),
                StableTypeNode::Named(named),
                StableTypeNode::Reference {
                    target: 1,
                    mutable: false,
                },
                StableTypeNode::Function {
                    parameters: vec![0, 2],
                    result: 1,
                },
            ],
            roots: vec![3],
        };
        let bytes = encode_type_graph(&graph).unwrap();
        assert_eq!(decode_type_graph(&bytes).unwrap(), graph);

        let invalid = StableTypeGraph {
            nodes: vec![
                StableTypeNode::Reference {
                    target: 1,
                    mutable: false,
                },
                StableTypeNode::Unit,
            ],
            roots: vec![0],
        };
        assert_eq!(
            encode_type_graph(&invalid),
            Err(MetadataError::InvalidManifest)
        );
    }

    #[test]
    fn stable_type_graph_rejects_trailing_bytes() {
        let graph = StableTypeGraph {
            nodes: vec![StableTypeNode::Unit],
            roots: vec![0],
        };
        let mut bytes = encode_type_graph(&graph).unwrap();
        bytes.push(0);
        assert_eq!(
            decode_type_graph(&bytes),
            Err(MetadataError::InvalidManifest)
        );
    }
}
