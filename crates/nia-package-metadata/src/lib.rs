// SPDX-License-Identifier: GPL-3.0-or-later
//! Relocation-independent package identities and stable compiler type graphs.

use std::io::{self, Cursor, Read};

use nia_compat::{COMPILER_VERSION, RELEASE_COMPATIBILITY as NIA_RELEASE_COMPATIBILITY, toolchain};

/// Compatibility epoch for stable compiler-owned metadata.
pub const RELEASE_COMPATIBILITY: u32 = NIA_RELEASE_COMPATIBILITY;
/// Maximum accepted stable metadata payload size.
pub const MAX_STABLE_METADATA_BYTES: usize = 64 * 1024 * 1024;
const MAX_STRING_BYTES: usize = 16 * 1024 * 1024;
const MAX_ITEMS: usize = 1_000_000;
const MAX_DEFINITION_DEPTH: usize = 256;
const TYPE_GRAPH_MAGIC: &[u8; 8] = b"NIATYP\0\0";
const TYPE_GRAPH_SCHEMA: u32 = RELEASE_COMPATIBILITY;

/// Relocation-independent identity of one package.
#[derive(Debug, Clone, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct PackageId {
    pub namespace: String,
    pub name: String,
    pub version: String,
}

impl PackageId {
    /// Returns the canonical text used at compiler identity boundaries.
    pub fn canonical_text(&self) -> String {
        format!("{}/{}@{}", self.namespace, self.name, self.version)
    }

    /// Returns the canonical identity of the toolchain standard library.
    pub fn standard_library() -> Self {
        Self {
            namespace: "nia".into(),
            name: "std".into(),
            version: format!("{}+std{}", COMPILER_VERSION, toolchain::STANDARD_LIBRARY),
        }
    }
}

/// Relocation-independent identity of one module within a package.
#[derive(Debug, Clone, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct ModuleId {
    pub package: PackageId,
    pub path: String,
}

/// Stable identity of a definition within a package module.
#[derive(Debug, Clone, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct DefinitionId {
    pub module: ModuleId,
    pub name: String,
    pub kind: u8,
    /// Stable session-independent declaration token. Definitions with the
    /// same module/name/kind/owner (for example overloaded methods) retain
    /// distinct identities through this field.
    pub disambiguator: u64,
    /// Canonical containing definition for nested members.
    pub owner: Option<Box<DefinitionId>>,
}

/// Encodes one stable module identity for embedding in another versioned
/// compiler-owned section. The enclosing section owns framing and schema;
/// this function owns the canonical package/module representation.
pub fn encode_module_id(module: &ModuleId) -> Result<Vec<u8>, MetadataError> {
    let mut output = Vec::new();
    put_module_id(&mut output, module)?;
    Ok(output)
}

/// Decodes one complete embedded stable module identity.
pub fn decode_module_id(bytes: &[u8]) -> Result<ModuleId, MetadataError> {
    if bytes.len() > MAX_STABLE_METADATA_BYTES {
        return Err(MetadataError::TooLarge);
    }
    let mut cursor = Cursor::new(bytes);
    let module = read_module_id(&mut cursor)?;
    if cursor.position() != bytes.len() as u64 {
        return Err(MetadataError::InvalidManifest);
    }
    Ok(module)
}

/// Encodes one stable definition identity for embedding in another versioned
/// compiler-owned section, including its complete canonical owner chain.
pub fn encode_definition_id(definition: &DefinitionId) -> Result<Vec<u8>, MetadataError> {
    validate_definition(definition)?;
    let mut output = Vec::new();
    put_definition(&mut output, definition)?;
    Ok(output)
}

/// Decodes one complete embedded stable definition identity and validates its
/// package, module, name, kind, disambiguator, and bounded owner chain.
pub fn decode_definition_id(bytes: &[u8]) -> Result<DefinitionId, MetadataError> {
    if bytes.len() > MAX_STABLE_METADATA_BYTES {
        return Err(MetadataError::TooLarge);
    }
    let mut cursor = Cursor::new(bytes);
    let definition = read_definition(&mut cursor)?;
    if cursor.position() != bytes.len() as u64 {
        return Err(MetadataError::InvalidManifest);
    }
    validate_definition(&definition)?;
    Ok(definition)
}

/// Stable const-generic argument used by applied nominal type nodes.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct StableConstArg {
    pub ty: u32,
    pub value: StableConstValue,
}

/// Relocation-independent value of a const-generic argument.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub enum StableConstValue {
    GenericParam(u64),
    Integer { bits: u128, signed: bool },
    Bool(bool),
    Char(char),
}

/// Stable identity of a source-defined or compiler-provided trait.
#[derive(Debug, Clone, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub enum StableTraitId {
    Source(DefinitionId),
    Builtin(u8),
}

/// Associated type binding carried by a stable trait object or projection.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct StableAssociatedTypeBinding {
    pub trait_id: Option<StableTraitId>,
    pub trait_arguments: Vec<u32>,
    pub trait_const_arguments: Vec<StableConstArg>,
    pub name: u64,
    pub ty: u32,
}

/// Canonical array length expression accepted by package interfaces.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub enum StableArrayLength {
    ConstValue(u64),
    GenericParam(u64),
}

/// Relocation-independent type node used by stable compiler interfaces.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum StableTypeNode {
    /// Primitive type tag owned by the language ABI.
    Primitive(u8),
    /// Named declaration resolved through a package-stable identity.
    Named(DefinitionId),
    /// Named declaration applied to earlier type-graph arguments.
    NamedApplied {
        definition: DefinitionId,
        arguments: Vec<u32>,
        const_arguments: Vec<StableConstArg>,
    },
    /// Unit type.
    Unit,
    /// Never type.
    Never,
    /// Tuple whose elements refer to earlier nodes.
    Tuple(Vec<u32>),
    /// Array with a bounded constant length.
    Array {
        element: u32,
        length: StableArrayLength,
    },
    /// Function signature with parameter and result node references.
    Function { parameters: Vec<u32>, result: u32 },
    /// Borrow/reference to an earlier node.
    Reference { target: u32, mutable: bool },
    /// Raw pointer to an earlier node.
    Pointer { target: u32, readonly: bool },
    /// Generic parameter identified by the stable symbol identity of its
    /// declaration name. Symbol identities are content-addressed and do not
    /// contain session-local handles.
    GenericParam(u64),
    /// Error recovery type.
    Error,
    /// Compile-time-only type.
    ConstOnly,
    /// Opaque type with intentionally hidden representation.
    Opaque,
    /// Volatile pointer to an earlier node.
    VolatilePointer { target: u32, readonly: bool },
    /// Fat slice value.
    Slice { target: u32, readonly: bool },
    /// Unsized slice pointee.
    SlicePointee { target: u32 },
    /// Fixed-width SIMD vector (primitive lane tag plus lane count).
    Vector { element: u8, lanes: u32 },
    /// Range value with an optional bound node.
    Range { kind: u8, bound: Option<u32> },
    /// Optional value wrapper.
    Optional { element: u32 },
    /// Error/value union wrapper.
    ErrorUnion { error: u32, value: u32 },
    /// Readonly or mutable callable closure view.
    Callable {
        parameters: Vec<u32>,
        result: u32,
        readonly: bool,
    },
    /// Unsized callable state pointee.
    CallablePointee { parameters: Vec<u32>, result: u32 },
    /// Unresolved `Self` parameter.
    SelfParam,
    /// Compiler-provided builtin nominal type.
    BuiltinType(u8),
    /// Compiler-provided builtin trait application.
    BuiltinTrait { trait_id: u8, arguments: Vec<u32> },
    /// Sized trait-object value.
    TraitObject {
        readonly: bool,
        trait_id: StableTraitId,
        trait_arguments: Vec<u32>,
        trait_const_arguments: Vec<StableConstArg>,
        associated_type_bindings: Vec<StableAssociatedTypeBinding>,
    },
    /// Unsized trait-object pointee.
    TraitObjectPointee {
        trait_id: StableTraitId,
        trait_arguments: Vec<u32>,
        trait_const_arguments: Vec<StableConstArg>,
        associated_type_bindings: Vec<StableAssociatedTypeBinding>,
    },
    /// Associated type projection before normalization.
    Projection {
        self_ty: u32,
        trait_id: StableTraitId,
        trait_arguments: Vec<u32>,
        trait_const_arguments: Vec<StableConstArg>,
        name: u64,
    },
    /// Anonymous closure state owned by a stable function definition.
    ClosureState {
        owner: DefinitionId,
        ordinal: u32,
        captures: Vec<u32>,
        parameters: Vec<u32>,
        result: u32,
    },
}

/// Canonical, bounded type graph for stable cross-package type identities.
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
                StableTypeNode::Primitive(_)
                | StableTypeNode::Unit
                | StableTypeNode::Never
                | StableTypeNode::Error
                | StableTypeNode::ConstOnly
                | StableTypeNode::Opaque
                | StableTypeNode::SelfParam
                | StableTypeNode::BuiltinType(_) => {}
                StableTypeNode::Named(definition) => validate_definition(definition)?,
                StableTypeNode::NamedApplied {
                    definition,
                    arguments,
                    const_arguments,
                } => {
                    validate_definition(definition)?;
                    references.extend(arguments);
                    validate_stable_const_arguments(const_arguments)?;
                    references.extend(const_arguments.iter().map(|argument| &argument.ty));
                }
                StableTypeNode::Tuple(elements) => references.extend(elements),
                StableTypeNode::Array { element, length } => {
                    references.push(element);
                    if let StableArrayLength::GenericParam(_) = length {
                        // Generic parameter identities are self-contained.
                    }
                }
                StableTypeNode::Function { parameters, result } => {
                    references.extend(parameters);
                    references.push(result);
                }
                StableTypeNode::Reference { target, .. } => references.push(target),
                StableTypeNode::Pointer { target, .. } => references.push(target),
                StableTypeNode::VolatilePointer { target, .. }
                | StableTypeNode::Slice { target, .. }
                | StableTypeNode::SlicePointee { target }
                | StableTypeNode::Optional { element: target } => references.push(target),
                StableTypeNode::ErrorUnion { error, value } => {
                    references.push(error);
                    references.push(value);
                }
                StableTypeNode::Range { kind, bound } => {
                    if *kind > 5 {
                        return Err(MetadataError::InvalidManifest);
                    }
                    if let Some(bound) = bound {
                        references.push(bound);
                    }
                }
                StableTypeNode::Vector { element, lanes } => {
                    if !(1..=16).contains(element) || *lanes == 0 {
                        return Err(MetadataError::InvalidManifest);
                    }
                }
                StableTypeNode::Callable {
                    parameters, result, ..
                }
                | StableTypeNode::CallablePointee { parameters, result } => {
                    references.extend(parameters);
                    references.push(result);
                }
                StableTypeNode::BuiltinTrait {
                    trait_id,
                    arguments,
                } => {
                    if nia_ids::BuiltinTrait::from_stable_tag(u32::from(*trait_id)).is_none() {
                        return Err(MetadataError::InvalidManifest);
                    }
                    references.extend(arguments);
                }
                StableTypeNode::TraitObject {
                    trait_id,
                    trait_arguments,
                    trait_const_arguments,
                    associated_type_bindings,
                    ..
                }
                | StableTypeNode::TraitObjectPointee {
                    trait_id,
                    trait_arguments,
                    trait_const_arguments,
                    associated_type_bindings,
                } => {
                    validate_stable_trait_id(trait_id)?;
                    references.extend(trait_arguments);
                    validate_stable_const_arguments(trait_const_arguments)?;
                    references.extend(trait_const_arguments.iter().map(|argument| &argument.ty));
                    if associated_type_bindings.len() > MAX_ITEMS {
                        return Err(MetadataError::TooManyItems);
                    }
                    for binding in associated_type_bindings {
                        if let Some(trait_id) = &binding.trait_id {
                            validate_stable_trait_id(trait_id)?;
                        }
                        references.extend(&binding.trait_arguments);
                        validate_stable_const_arguments(&binding.trait_const_arguments)?;
                        references.extend(
                            binding
                                .trait_const_arguments
                                .iter()
                                .map(|argument| &argument.ty),
                        );
                        references.push(&binding.ty);
                    }
                }
                StableTypeNode::Projection {
                    self_ty,
                    trait_id,
                    trait_arguments,
                    trait_const_arguments,
                    ..
                } => {
                    validate_stable_trait_id(trait_id)?;
                    references.push(self_ty);
                    references.extend(trait_arguments);
                    validate_stable_const_arguments(trait_const_arguments)?;
                    references.extend(trait_const_arguments.iter().map(|argument| &argument.ty));
                }
                StableTypeNode::ClosureState {
                    owner,
                    captures,
                    parameters,
                    result,
                    ..
                } => {
                    validate_definition(owner)?;
                    references.extend(captures);
                    references.extend(parameters);
                    references.push(result);
                }
                StableTypeNode::GenericParam(_) => {}
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

fn validate_stable_trait_id(trait_id: &StableTraitId) -> Result<(), MetadataError> {
    match trait_id {
        StableTraitId::Source(definition) => validate_definition(definition),
        StableTraitId::Builtin(tag)
            if nia_ids::BuiltinTrait::from_stable_tag(u32::from(*tag)).is_some() =>
        {
            Ok(())
        }
        StableTraitId::Builtin(_) => Err(MetadataError::InvalidManifest),
    }
}

fn validate_stable_const_arguments(arguments: &[StableConstArg]) -> Result<(), MetadataError> {
    if arguments.len() > MAX_ITEMS {
        Err(MetadataError::TooManyItems)
    } else {
        Ok(())
    }
}

/// Encodes and validates a canonical stable type graph.
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
            StableTypeNode::NamedApplied {
                definition,
                arguments,
                const_arguments,
            } => {
                output.push(11);
                put_definition(&mut output, definition)?;
                put_list_len(&mut output, arguments.len())?;
                for argument in arguments {
                    put_u32(&mut output, *argument);
                }
                put_list_len(&mut output, const_arguments.len())?;
                for argument in const_arguments {
                    put_stable_const_arg(&mut output, argument);
                }
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
                match length {
                    StableArrayLength::ConstValue(value) => {
                        output.push(0);
                        output.extend_from_slice(&value.to_le_bytes());
                    }
                    StableArrayLength::GenericParam(hash) => {
                        output.push(1);
                        output.extend_from_slice(&hash.to_le_bytes());
                    }
                }
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
            StableTypeNode::GenericParam(index) => {
                output.push(10);
                output.extend_from_slice(&index.to_le_bytes());
            }
            StableTypeNode::Error => output.push(12),
            StableTypeNode::ConstOnly => output.push(13),
            StableTypeNode::Opaque => output.push(14),
            StableTypeNode::VolatilePointer { target, readonly } => {
                output.push(15);
                put_u32(&mut output, *target);
                output.push(u8::from(*readonly));
            }
            StableTypeNode::Slice { target, readonly } => {
                output.push(16);
                put_u32(&mut output, *target);
                output.push(u8::from(*readonly));
            }
            StableTypeNode::SlicePointee { target } => {
                output.push(17);
                put_u32(&mut output, *target);
            }
            StableTypeNode::Vector { element, lanes } => {
                output.push(18);
                output.push(*element);
                put_u32(&mut output, *lanes);
            }
            StableTypeNode::Range { kind, bound } => {
                output.push(19);
                output.push(*kind);
                match bound {
                    Some(bound) => {
                        output.push(1);
                        put_u32(&mut output, *bound);
                    }
                    None => output.push(0),
                }
            }
            StableTypeNode::Optional { element } => {
                output.push(20);
                put_u32(&mut output, *element);
            }
            StableTypeNode::ErrorUnion { error, value } => {
                output.push(21);
                put_u32(&mut output, *error);
                put_u32(&mut output, *value);
            }
            StableTypeNode::Callable {
                parameters,
                result,
                readonly,
            } => {
                output.push(22);
                put_list_len(&mut output, parameters.len())?;
                for parameter in parameters {
                    put_u32(&mut output, *parameter);
                }
                put_u32(&mut output, *result);
                output.push(u8::from(*readonly));
            }
            StableTypeNode::CallablePointee { parameters, result } => {
                output.push(23);
                put_list_len(&mut output, parameters.len())?;
                for parameter in parameters {
                    put_u32(&mut output, *parameter);
                }
                put_u32(&mut output, *result);
            }
            StableTypeNode::SelfParam => output.push(24),
            StableTypeNode::BuiltinType(tag) => {
                output.push(25);
                output.push(*tag);
            }
            StableTypeNode::BuiltinTrait {
                trait_id,
                arguments,
            } => {
                output.push(26);
                output.push(*trait_id);
                put_list_len(&mut output, arguments.len())?;
                for argument in arguments {
                    put_u32(&mut output, *argument);
                }
            }
            StableTypeNode::TraitObject {
                readonly,
                trait_id,
                trait_arguments,
                trait_const_arguments,
                associated_type_bindings,
            } => {
                output.push(27);
                output.push(u8::from(*readonly));
                put_stable_trait_id(&mut output, trait_id)?;
                put_refs(&mut output, trait_arguments)?;
                put_stable_const_args(&mut output, trait_const_arguments)?;
                put_stable_associated_bindings(&mut output, associated_type_bindings)?;
            }
            StableTypeNode::TraitObjectPointee {
                trait_id,
                trait_arguments,
                trait_const_arguments,
                associated_type_bindings,
            } => {
                output.push(28);
                put_stable_trait_id(&mut output, trait_id)?;
                put_refs(&mut output, trait_arguments)?;
                put_stable_const_args(&mut output, trait_const_arguments)?;
                put_stable_associated_bindings(&mut output, associated_type_bindings)?;
            }
            StableTypeNode::Projection {
                self_ty,
                trait_id,
                trait_arguments,
                trait_const_arguments,
                name,
            } => {
                output.push(29);
                put_u32(&mut output, *self_ty);
                put_stable_trait_id(&mut output, trait_id)?;
                put_refs(&mut output, trait_arguments)?;
                put_stable_const_args(&mut output, trait_const_arguments)?;
                output.extend_from_slice(&name.to_le_bytes());
            }
            StableTypeNode::ClosureState {
                owner,
                ordinal,
                captures,
                parameters,
                result,
            } => {
                output.push(30);
                put_definition(&mut output, owner)?;
                put_u32(&mut output, *ordinal);
                put_refs(&mut output, captures)?;
                put_refs(&mut output, parameters)?;
                put_u32(&mut output, *result);
            }
        }
    }
    if output.len() > MAX_STABLE_METADATA_BYTES {
        return Err(MetadataError::TooLarge);
    }
    Ok(output)
}

/// Decodes and validates a canonical stable type graph.
pub fn decode_type_graph(bytes: &[u8]) -> Result<StableTypeGraph, MetadataError> {
    if bytes.len() > MAX_STABLE_METADATA_BYTES {
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
            11 => StableTypeNode::NamedApplied {
                definition: read_definition(&mut cursor)?,
                arguments: read_refs(&mut cursor)?,
                const_arguments: {
                    let count = bounded_count(get_u32(&mut cursor)?)?;
                    (0..count)
                        .map(|_| read_stable_const_arg(&mut cursor))
                        .collect::<Result<Vec<_>, _>>()?
                },
            },
            3 => StableTypeNode::Unit,
            4 => StableTypeNode::Never,
            5 => StableTypeNode::Tuple(read_refs(&mut cursor)?),
            6 => StableTypeNode::Array {
                element: get_u32(&mut cursor)?,
                length: match read_u8(&mut cursor)? {
                    0 => StableArrayLength::ConstValue(get_u64(&mut cursor)?),
                    1 => StableArrayLength::GenericParam(get_u64(&mut cursor)?),
                    _ => return Err(MetadataError::InvalidManifest),
                },
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
            10 => StableTypeNode::GenericParam(get_u64(&mut cursor)?),
            12 => StableTypeNode::Error,
            13 => StableTypeNode::ConstOnly,
            14 => StableTypeNode::Opaque,
            15 => StableTypeNode::VolatilePointer {
                target: get_u32(&mut cursor)?,
                readonly: match read_u8(&mut cursor)? {
                    0 => false,
                    1 => true,
                    _ => return Err(MetadataError::InvalidManifest),
                },
            },
            16 => StableTypeNode::Slice {
                target: get_u32(&mut cursor)?,
                readonly: match read_u8(&mut cursor)? {
                    0 => false,
                    1 => true,
                    _ => return Err(MetadataError::InvalidManifest),
                },
            },
            17 => StableTypeNode::SlicePointee {
                target: get_u32(&mut cursor)?,
            },
            18 => StableTypeNode::Vector {
                element: read_u8(&mut cursor)?,
                lanes: get_u32(&mut cursor)?,
            },
            19 => StableTypeNode::Range {
                kind: read_u8(&mut cursor)?,
                bound: match read_u8(&mut cursor)? {
                    0 => None,
                    1 => Some(get_u32(&mut cursor)?),
                    _ => return Err(MetadataError::InvalidManifest),
                },
            },
            20 => StableTypeNode::Optional {
                element: get_u32(&mut cursor)?,
            },
            21 => StableTypeNode::ErrorUnion {
                error: get_u32(&mut cursor)?,
                value: get_u32(&mut cursor)?,
            },
            22 => StableTypeNode::Callable {
                parameters: read_refs(&mut cursor)?,
                result: get_u32(&mut cursor)?,
                readonly: match read_u8(&mut cursor)? {
                    0 => false,
                    1 => true,
                    _ => return Err(MetadataError::InvalidManifest),
                },
            },
            23 => StableTypeNode::CallablePointee {
                parameters: read_refs(&mut cursor)?,
                result: get_u32(&mut cursor)?,
            },
            24 => StableTypeNode::SelfParam,
            25 => {
                let tag = read_u8(&mut cursor)?;
                if tag > 2 {
                    return Err(MetadataError::InvalidManifest);
                }
                StableTypeNode::BuiltinType(tag)
            }
            26 => StableTypeNode::BuiltinTrait {
                trait_id: {
                    let tag = read_u8(&mut cursor)?;
                    if nia_ids::BuiltinTrait::from_stable_tag(u32::from(tag)).is_none() {
                        return Err(MetadataError::InvalidManifest);
                    }
                    tag
                },
                arguments: read_refs(&mut cursor)?,
            },
            27 => StableTypeNode::TraitObject {
                readonly: match read_u8(&mut cursor)? {
                    0 => false,
                    1 => true,
                    _ => return Err(MetadataError::InvalidManifest),
                },
                trait_id: read_stable_trait_id(&mut cursor)?,
                trait_arguments: read_refs(&mut cursor)?,
                trait_const_arguments: read_stable_const_args(&mut cursor)?,
                associated_type_bindings: read_stable_associated_bindings(&mut cursor)?,
            },
            28 => StableTypeNode::TraitObjectPointee {
                trait_id: read_stable_trait_id(&mut cursor)?,
                trait_arguments: read_refs(&mut cursor)?,
                trait_const_arguments: read_stable_const_args(&mut cursor)?,
                associated_type_bindings: read_stable_associated_bindings(&mut cursor)?,
            },
            29 => StableTypeNode::Projection {
                self_ty: get_u32(&mut cursor)?,
                trait_id: read_stable_trait_id(&mut cursor)?,
                trait_arguments: read_refs(&mut cursor)?,
                trait_const_arguments: read_stable_const_args(&mut cursor)?,
                name: get_u64(&mut cursor)?,
            },
            30 => StableTypeNode::ClosureState {
                owner: read_definition(&mut cursor)?,
                ordinal: get_u32(&mut cursor)?,
                captures: read_refs(&mut cursor)?,
                parameters: read_refs(&mut cursor)?,
                result: get_u32(&mut cursor)?,
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

/// Errors returned by stable metadata validation and decoding.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum MetadataError {
    BadMagic,
    Schema(u32),
    TooLarge,
    TooManyItems,
    Truncated,
    InvalidString,
    InvalidManifest,
    Io,
}
impl std::fmt::Display for MetadataError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let message = match self {
            Self::BadMagic => "invalid stable metadata header",
            Self::Schema(_) => "unsupported stable metadata schema",
            Self::TooLarge => "stable metadata exceeds the size limit",
            Self::TooManyItems => "stable metadata contains too many items",
            Self::Truncated => "stable metadata is truncated",
            Self::InvalidString => "stable metadata contains invalid text",
            Self::InvalidManifest => "stable metadata manifest is invalid",
            Self::Io => "could not read stable metadata",
        };
        f.write_str(message)
    }
}
impl std::error::Error for MetadataError {}

fn validate_id(id: &PackageId) -> Result<(), MetadataError> {
    validate_string(&id.namespace)?;
    validate_string(&id.name)?;
    validate_string(&id.version)
}

fn validate_definition(definition: &DefinitionId) -> Result<(), MetadataError> {
    validate_definition_with_depth(definition, 0)
}

fn validate_module_id(module: &ModuleId) -> Result<(), MetadataError> {
    validate_id(&module.package)?;
    validate_string(&module.path)
}

fn validate_definition_with_depth(
    definition: &DefinitionId,
    depth: usize,
) -> Result<(), MetadataError> {
    if depth >= MAX_DEFINITION_DEPTH {
        return Err(MetadataError::TooManyItems);
    }
    validate_id(&definition.module.package)?;
    validate_string(&definition.module.path)?;
    validate_string(&definition.name)?;
    if let Some(owner) = &definition.owner {
        if owner.module != definition.module || owner.as_ref() == definition {
            return Err(MetadataError::InvalidManifest);
        }
        validate_definition_with_depth(owner, depth + 1)?;
    }
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
fn put_module_id(output: &mut Vec<u8>, module: &ModuleId) -> Result<(), MetadataError> {
    validate_module_id(module)?;
    put_id(output, &module.package)?;
    put_string(output, &module.path)
}
fn read_module_id(cursor: &mut Cursor<&[u8]>) -> Result<ModuleId, MetadataError> {
    let module = ModuleId {
        package: get_id(cursor)?,
        path: get_string(cursor)?,
    };
    validate_module_id(&module)?;
    Ok(module)
}
fn put_definition(output: &mut Vec<u8>, definition: &DefinitionId) -> Result<(), MetadataError> {
    validate_definition(definition)?;
    put_id(output, &definition.module.package)?;
    put_string(output, &definition.module.path)?;
    put_string(output, &definition.name)?;
    output.push(definition.kind);
    output.extend_from_slice(&definition.disambiguator.to_le_bytes());
    match &definition.owner {
        Some(owner) => {
            output.push(1);
            put_definition(output, owner)?;
        }
        None => output.push(0),
    }
    Ok(())
}
fn read_definition(cursor: &mut Cursor<&[u8]>) -> Result<DefinitionId, MetadataError> {
    read_definition_with_depth(cursor, 0)
}

fn read_definition_with_depth(
    cursor: &mut Cursor<&[u8]>,
    depth: usize,
) -> Result<DefinitionId, MetadataError> {
    if depth >= MAX_DEFINITION_DEPTH {
        return Err(MetadataError::TooManyItems);
    }
    let definition = DefinitionId {
        module: ModuleId {
            package: get_id(cursor)?,
            path: get_string(cursor)?,
        },
        name: get_string(cursor)?,
        kind: read_u8(cursor)?,
        disambiguator: get_u64(cursor)?,
        owner: match read_u8(cursor)? {
            0 => None,
            1 => Some(Box::new(read_definition_with_depth(cursor, depth + 1)?)),
            _ => return Err(MetadataError::InvalidManifest),
        },
    };
    validate_definition(&definition)?;
    Ok(definition)
}
fn read_refs(cursor: &mut Cursor<&[u8]>) -> Result<Vec<u32>, MetadataError> {
    let count = bounded_count(get_u32(cursor)?)?;
    (0..count).map(|_| get_u32(cursor)).collect()
}

fn put_refs(output: &mut Vec<u8>, refs: &[u32]) -> Result<(), MetadataError> {
    put_list_len(output, refs.len())?;
    for reference in refs {
        put_u32(output, *reference);
    }
    Ok(())
}

fn put_stable_const_arg(output: &mut Vec<u8>, argument: &StableConstArg) {
    put_u32(output, argument.ty);
    match &argument.value {
        StableConstValue::GenericParam(hash) => {
            output.push(0);
            output.extend_from_slice(&hash.to_le_bytes());
        }
        StableConstValue::Integer { bits, signed } => {
            output.push(1);
            output.extend_from_slice(&bits.to_le_bytes());
            output.push(u8::from(*signed));
        }
        StableConstValue::Bool(value) => {
            output.push(2);
            output.push(u8::from(*value));
        }
        StableConstValue::Char(value) => {
            output.push(3);
            output.extend_from_slice(&u32::from(*value).to_le_bytes());
        }
    }
}

fn put_stable_const_args(
    output: &mut Vec<u8>,
    arguments: &[StableConstArg],
) -> Result<(), MetadataError> {
    put_list_len(output, arguments.len())?;
    for argument in arguments {
        put_stable_const_arg(output, argument);
    }
    Ok(())
}

fn read_stable_const_arg(cursor: &mut Cursor<&[u8]>) -> Result<StableConstArg, MetadataError> {
    let ty = get_u32(cursor)?;
    let value = match read_u8(cursor)? {
        0 => StableConstValue::GenericParam(get_u64(cursor)?),
        1 => StableConstValue::Integer {
            bits: get_u128(cursor)?,
            signed: match read_u8(cursor)? {
                0 => false,
                1 => true,
                _ => return Err(MetadataError::InvalidManifest),
            },
        },
        2 => StableConstValue::Bool(match read_u8(cursor)? {
            0 => false,
            1 => true,
            _ => return Err(MetadataError::InvalidManifest),
        }),
        3 => char::from_u32(get_u32(cursor)?)
            .map(StableConstValue::Char)
            .ok_or(MetadataError::InvalidManifest)?,
        _ => return Err(MetadataError::InvalidManifest),
    };
    Ok(StableConstArg { ty, value })
}

fn read_stable_const_args(
    cursor: &mut Cursor<&[u8]>,
) -> Result<Vec<StableConstArg>, MetadataError> {
    let count = bounded_count(get_u32(cursor)?)?;
    (0..count).map(|_| read_stable_const_arg(cursor)).collect()
}

fn put_stable_trait_id(
    output: &mut Vec<u8>,
    trait_id: &StableTraitId,
) -> Result<(), MetadataError> {
    match trait_id {
        StableTraitId::Source(definition) => {
            output.push(0);
            put_definition(output, definition)?;
        }
        StableTraitId::Builtin(tag) => {
            output.push(1);
            output.push(*tag);
        }
    }
    Ok(())
}

fn read_stable_trait_id(cursor: &mut Cursor<&[u8]>) -> Result<StableTraitId, MetadataError> {
    match read_u8(cursor)? {
        0 => Ok(StableTraitId::Source(read_definition(cursor)?)),
        1 => {
            let tag = read_u8(cursor)?;
            nia_ids::BuiltinTrait::from_stable_tag(u32::from(tag))
                .is_some()
                .then_some(StableTraitId::Builtin(tag))
                .ok_or(MetadataError::InvalidManifest)
        }
        _ => Err(MetadataError::InvalidManifest),
    }
}

fn put_stable_associated_bindings(
    output: &mut Vec<u8>,
    bindings: &[StableAssociatedTypeBinding],
) -> Result<(), MetadataError> {
    put_list_len(output, bindings.len())?;
    for binding in bindings {
        match &binding.trait_id {
            Some(trait_id) => {
                output.push(1);
                put_stable_trait_id(output, trait_id)?;
            }
            None => output.push(0),
        }
        put_refs(output, &binding.trait_arguments)?;
        put_stable_const_args(output, &binding.trait_const_arguments)?;
        output.extend_from_slice(&binding.name.to_le_bytes());
        put_u32(output, binding.ty);
    }
    Ok(())
}

fn read_stable_associated_bindings(
    cursor: &mut Cursor<&[u8]>,
) -> Result<Vec<StableAssociatedTypeBinding>, MetadataError> {
    let count = bounded_count(get_u32(cursor)?)?;
    (0..count)
        .map(|_| {
            let trait_id = match read_u8(cursor)? {
                0 => None,
                1 => Some(read_stable_trait_id(cursor)?),
                _ => return Err(MetadataError::InvalidManifest),
            };
            Ok(StableAssociatedTypeBinding {
                trait_id,
                trait_arguments: read_refs(cursor)?,
                trait_const_arguments: read_stable_const_args(cursor)?,
                name: get_u64(cursor)?,
                ty: get_u32(cursor)?,
            })
        })
        .collect()
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

fn get_string(cursor: &mut Cursor<&[u8]>) -> Result<String, MetadataError> {
    let length = bounded_len(get_u32(cursor)?)?;
    if length == 0 {
        return Err(MetadataError::InvalidString);
    }
    let mut bytes = vec![0; length];
    read_exact(cursor, &mut bytes)?;
    String::from_utf8(bytes).map_err(|_| MetadataError::InvalidString)
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
    (value <= MAX_STABLE_METADATA_BYTES)
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
fn get_u128(cursor: &mut Cursor<&[u8]>) -> Result<u128, MetadataError> {
    let mut bytes = [0; 16];
    read_exact(cursor, &mut bytes)?;
    Ok(u128::from_le_bytes(bytes))
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

    #[test]
    fn metadata_errors_use_semantic_messages() {
        assert_eq!(
            MetadataError::BadMagic.to_string(),
            "invalid stable metadata header"
        );
        assert_eq!(
            MetadataError::Schema(99).to_string(),
            "unsupported stable metadata schema"
        );
        assert_eq!(
            MetadataError::InvalidManifest.to_string(),
            "stable metadata manifest is invalid"
        );
        assert!(!MetadataError::Schema(99).to_string().contains("Schema("));
    }

    fn package() -> PackageId {
        PackageId {
            namespace: "example".into(),
            name: "identity".into(),
            version: "1.0.0".into(),
        }
    }

    fn definition(name: &str, kind: u8, owner: Option<DefinitionId>) -> DefinitionId {
        DefinitionId {
            module: ModuleId {
                package: package(),
                path: "src/lib.nia".into(),
            },
            name: name.into(),
            kind,
            disambiguator: u64::from(kind),
            owner: owner.map(Box::new),
        }
    }

    #[test]
    fn stable_identity_codecs_round_trip_nested_definitions() {
        let module = ModuleId {
            package: package(),
            path: "src/lib.nia".into(),
        };
        let owner = definition("Container", 5, None);
        let member = definition("field", 6, Some(owner));

        assert_eq!(
            decode_module_id(&encode_module_id(&module).unwrap()).unwrap(),
            module
        );
        assert_eq!(
            decode_definition_id(&encode_definition_id(&member).unwrap()).unwrap(),
            member
        );
    }

    #[test]
    fn stable_identity_codecs_reject_trailing_bytes_and_cross_module_owners() {
        let mut bytes = encode_module_id(&ModuleId {
            package: package(),
            path: "src/lib.nia".into(),
        })
        .unwrap();
        bytes.push(0);
        assert_eq!(
            decode_module_id(&bytes),
            Err(MetadataError::InvalidManifest)
        );

        let mut member = definition("field", 6, None);
        member.owner = Some(Box::new(DefinitionId {
            module: ModuleId {
                package: package(),
                path: "src/other.nia".into(),
            },
            name: "Container".into(),
            kind: 5,
            disambiguator: 5,
            owner: None,
        }));
        assert_eq!(
            encode_definition_id(&member),
            Err(MetadataError::InvalidManifest)
        );
    }

    #[test]
    fn stable_type_graph_round_trips_identity_rich_nodes() {
        let owner = definition("call", 2, None);
        let graph = StableTypeGraph {
            nodes: vec![
                StableTypeNode::Primitive(3),
                StableTypeNode::Named(owner.clone()),
                StableTypeNode::TraitObject {
                    readonly: true,
                    trait_id: StableTraitId::Source(owner.clone()),
                    trait_arguments: vec![0],
                    trait_const_arguments: vec![],
                    associated_type_bindings: vec![StableAssociatedTypeBinding {
                        trait_id: None,
                        trait_arguments: vec![],
                        trait_const_arguments: vec![],
                        name: 42,
                        ty: 1,
                    }],
                },
                StableTypeNode::ClosureState {
                    owner,
                    ordinal: 1,
                    captures: vec![0],
                    parameters: vec![1],
                    result: 2,
                },
            ],
            roots: vec![3],
        };

        let bytes = encode_type_graph(&graph).unwrap();
        assert_eq!(decode_type_graph(&bytes).unwrap(), graph);
    }

    #[test]
    fn stable_type_graph_rejects_forward_references_and_trailing_bytes() {
        let forward = StableTypeGraph {
            nodes: vec![
                StableTypeNode::Pointer {
                    target: 1,
                    readonly: true,
                },
                StableTypeNode::Primitive(3),
            ],
            roots: vec![0],
        };
        assert_eq!(forward.validate(), Err(MetadataError::InvalidManifest));

        let graph = StableTypeGraph {
            nodes: vec![StableTypeNode::Primitive(3)],
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
