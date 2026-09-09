// SPDX-License-Identifier: GPL-3.0-or-later
//! Stable, lazily indexed metadata published by compiled Nia packages.

use std::{
    collections::BTreeMap,
    io::{self, Cursor, Read},
    sync::Arc,
};

use nia_compat::{COMPILER_VERSION, formats, toolchain};

/// Current package container schema.
pub const SCHEMA_VERSION: u32 = formats::PACKAGE_METADATA.schema;
/// Maximum accepted complete container size.
pub const MAX_PACKAGE_BYTES: usize = 64 * 1024 * 1024;
const MAX_STRING_BYTES: usize = 16 * 1024 * 1024;
const MAX_ITEMS: usize = 1_000_000;
const MAX_DEFINITION_DEPTH: usize = 256;
const HEADER_BYTES: usize = 8 + 4 + 4 + 4;
const SECTION_ENTRY_BYTES: usize = 1 + 8 + 8 + 32;
const INTERFACE_MAGIC: &[u8; 8] = b"NIAINT01";
// Version 6 adds a stable declaration disambiguator for overloads.
const INTERFACE_SCHEMA: u32 = 6;
const TYPE_GRAPH_MAGIC: &[u8; 8] = b"NIATYP01";
const TYPE_GRAPH_SCHEMA: u32 = 4;
const DECLARATION_MAGIC: &[u8; 9] = b"NIADECL01";
const SIGNATURE_MAGIC: &[u8; 8] = b"NIASIG01";
const SIGNATURE_SCHEMA: u32 = 7;
const TEMPLATE_MAGIC: &[u8; 8] = b"NIATPL01";
const TEMPLATE_SCHEMA: u32 = 3;
const TEMPLATE_SUMMARY_MAGIC: &[u8; 8] = b"NIASUM01";
const TEMPLATE_SUMMARY_SCHEMA: u32 = 1;
const NATIVE_MAGIC: &[u8; 8] = b"NIANAT01";
const NATIVE_SCHEMA: u32 = 2;
const PUBLIC_SURFACE_MAGIC: &[u8; 8] = b"NIAPUB01";
const PUBLIC_SURFACE_SCHEMA: u32 = 2;

/// Relocation-independent identity of one package.
#[derive(Debug, Clone, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct PackageId {
    pub namespace: String,
    pub name: String,
    pub version: String,
}

impl PackageId {
    /// Returns the canonical identity published by the toolchain standard
    /// library artifact.
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

/// One target-independent public declaration and references into its canonical
/// signature type graph.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct InterfaceRecord {
    pub definition: DefinitionId,
    /// Canonical declaration facts owned by the compiler interface protocol.
    pub declaration: Vec<u8>,
    /// Strictly sorted indices into the package type-graph section.
    pub type_roots: Vec<u32>,
}

/// Stable signature facts for one published definition.
///
/// The record deliberately contains only package-owned identities and indexes
/// into the canonical [`StableTypeGraph`].  Session-local `DefId`,
/// `InternedTyId`, symbols, and source spans must never cross this boundary.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SignatureRecord {
    pub definition: DefinitionId,
    /// Definition kind tag (the same canonical domain as `InterfaceRecord`).
    pub kind: u8,
    /// Canonical signature flags. Unknown bits are rejected by validation.
    pub flags: u32,
    /// Strictly sorted indexes into the package type-graph section.
    pub type_roots: Vec<u32>,
    /// Public members owned by this declaration. Members repeat their stable
    /// identities so consumers can build aggregate/trait indexes without
    /// loading dependency source.
    pub members: Vec<SignatureMember>,
    pub generic_params: Vec<SignatureGenericParam>,
    pub where_predicates: Vec<SignatureWherePredicate>,
    /// Complete declaration payload for consumers that need ordered
    /// parameters, fields, variants, or value types. The legacy root/member
    /// indexes remain available for aggregate indexes and are validated
    /// against this typed payload when present.
    pub payload: Option<SignaturePayload>,
}

/// Typed, target-independent declaration details used by downstream semantic
/// queries. Every type reference is an index into the package type graph.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SignaturePayload {
    Function {
        params: Vec<SignatureParameter>,
        return_type: u32,
        attributes: Vec<u32>,
    },
    Aggregate {
        fields: Vec<SignatureField>,
    },
    Enum {
        backing_type: u32,
        variants: Vec<SignatureVariant>,
    },
    TypeAlias {
        target: u32,
    },
    Value {
        explicit_type: Option<u32>,
        /// Stable builtin-const tag. Valid only for const declarations.
        builtin: Option<u32>,
    },
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SignatureParameter {
    pub name: Option<String>,
    /// `0` means no receiver; `1..=3` use the canonical ReceiverKind tags.
    pub receiver: u8,
    pub type_root: u32,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SignatureField {
    pub definition: DefinitionId,
    pub name: String,
    pub type_root: u32,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SignatureVariant {
    pub definition: DefinitionId,
    pub name: String,
    pub payload: SignatureVariantPayload,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SignatureVariantPayload {
    Unit,
    Tuple(Vec<u32>),
    Named(Vec<SignatureField>),
}

/// Stable generic parameter fact used by declaration signatures.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SignatureGenericParam {
    /// Stable parameter spelling in declaration order.
    pub name: String,
    pub kind: u8,
    /// Declared const parameter type, when `kind == 1`.
    pub type_root: Option<u32>,
}

/// Stable where-clause predicate.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SignatureWherePredicate {
    pub type_root: u32,
    pub bounds: Vec<SignatureWhereBound>,
}

/// Stable trait bound in a where predicate.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SignatureWhereBound {
    pub trait_root: u32,
    pub associated_type_bindings: Vec<SignatureAssociatedTypeBinding>,
}

/// Stable associated-type equality binding.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SignatureAssociatedTypeBinding {
    pub name: String,
    pub type_root: u32,
}

/// Stable declaration member facts used by aggregate and trait consumers.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SignatureMember {
    pub definition: DefinitionId,
    pub name: String,
    pub kind: u8,
    pub flags: u32,
    pub type_roots: Vec<u32>,
}

/// Stable trait declaration facts. All type references are indexes into the
/// package type graph; members carry their canonical nested definition IDs.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SignatureTraitRecord {
    pub definition: DefinitionId,
    pub supertraits: Vec<SignatureWhereBound>,
    pub members: Vec<SignatureMember>,
    /// Stable builtin-trait tag.
    pub builtin: Option<u32>,
}

/// Stable trait/inherent extension implementation facts. `impl_id` is the
/// compiler's package-stable implementation identity, not a session handle.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SignatureExtensionRecord {
    /// Canonical module that declares this implementation.
    pub module: ModuleId,
    pub impl_id: u64,
    pub target_root: u32,
    pub trait_root: Option<u32>,
    pub generic_params: Vec<SignatureGenericParam>,
    pub where_predicates: Vec<SignatureWherePredicate>,
    pub members: Vec<SignatureMember>,
    /// Associated type bindings defined by the implementation.
    pub associated_types: Vec<SignatureAssociatedType>,
    /// Canonical builtin implementation identity.
    pub builtin: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SignatureAssociatedType {
    pub name: String,
    pub type_root: u32,
}

/// Target-independent signature facts published by one package.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct SignatureSection {
    pub records: Vec<SignatureRecord>,
    pub traits: Vec<SignatureTraitRecord>,
    pub extensions: Vec<SignatureExtensionRecord>,
}

/// Signature has a checked body available to downstream consumers.
pub const SIGNATURE_FLAG_HAS_BODY: u32 = 1 << 0;
/// Signature is externally linked.
pub const SIGNATURE_FLAG_EXTERN: u32 = 1 << 1;
/// Signature is const-evaluable.
pub const SIGNATURE_FLAG_CONST: u32 = 1 << 2;
/// Signature accepts variadic arguments.
pub const SIGNATURE_FLAG_VARIADIC: u32 = 1 << 3;
/// Signature is an open enum declaration.
pub const SIGNATURE_FLAG_OPEN: u32 = 1 << 4;
/// Signature represents tuple-like aggregate construction.
pub const SIGNATURE_FLAG_TUPLE: u32 = 1 << 5;
/// Global storage permits mutation.
pub const SIGNATURE_FLAG_MUTABLE: u32 = 1 << 6;
const SIGNATURE_FLAGS_MASK: u32 = SIGNATURE_FLAG_HAS_BODY
    | SIGNATURE_FLAG_EXTERN
    | SIGNATURE_FLAG_CONST
    | SIGNATURE_FLAG_VARIADIC
    | SIGNATURE_FLAG_OPEN
    | SIGNATURE_FLAG_TUPLE
    | SIGNATURE_FLAG_MUTABLE;

fn validate_signature_payload(
    kind: u8,
    payload: Option<&SignaturePayload>,
) -> Result<(), MetadataError> {
    let Some(payload) = payload else {
        return Ok(());
    };
    let valid = match (kind, payload) {
        (
            2 | 11 | 12,
            SignaturePayload::Function {
                params, attributes, ..
            },
        ) => {
            params.len() <= MAX_ITEMS
                && attributes.len() <= MAX_ITEMS
                && params.iter().all(|param| param.receiver <= 3)
                && attributes
                    .iter()
                    .all(|attribute| matches!(*attribute, 1 | 2 | 0x101..=0x11b))
        }
        (5 | 7, SignaturePayload::Aggregate { fields }) => fields.len() <= MAX_ITEMS,
        (13, SignaturePayload::Enum { variants, .. }) => variants.len() <= MAX_ITEMS,
        (16, SignaturePayload::TypeAlias { .. }) => true,
        (3, SignaturePayload::Value { builtin: None, .. }) => true,
        (4, SignaturePayload::Value { builtin, .. }) => {
            builtin.is_none_or(|tag| matches!(tag, 1..=7))
        }
        _ => false,
    };
    if !valid {
        return Err(MetadataError::InvalidManifest);
    }
    match payload {
        SignaturePayload::Function { params, .. } => {
            for param in params {
                if let Some(name) = &param.name {
                    validate_string(name)?;
                }
            }
        }
        SignaturePayload::Aggregate { fields } => validate_signature_fields(fields)?,
        SignaturePayload::Enum { variants, .. } => {
            let mut seen = std::collections::BTreeSet::new();
            for variant in variants {
                validate_definition(&variant.definition)?;
                validate_string(&variant.name)?;
                if variant.name != variant.definition.name {
                    return Err(MetadataError::InvalidManifest);
                }
                if !seen.insert(variant.definition.clone()) {
                    return Err(MetadataError::InvalidManifest);
                }
                match &variant.payload {
                    SignatureVariantPayload::Unit => {}
                    SignatureVariantPayload::Tuple(types) => {
                        if types.len() > MAX_ITEMS {
                            return Err(MetadataError::TooManyItems);
                        }
                    }
                    SignatureVariantPayload::Named(fields) => validate_signature_fields(fields)?,
                }
            }
        }
        SignaturePayload::TypeAlias { .. } | SignaturePayload::Value { .. } => {}
    }
    Ok(())
}

fn validate_signature_fields(fields: &[SignatureField]) -> Result<(), MetadataError> {
    if fields.len() > MAX_ITEMS {
        return Err(MetadataError::TooManyItems);
    }
    let mut seen = std::collections::BTreeSet::new();
    for field in fields {
        validate_definition(&field.definition)?;
        validate_string(&field.name)?;
        if field.name != field.definition.name || !seen.insert(field.definition.clone()) {
            return Err(MetadataError::InvalidManifest);
        }
    }
    Ok(())
}

fn signature_payload_roots(payload: &SignaturePayload) -> Vec<u32> {
    let mut roots = Vec::new();
    match payload {
        SignaturePayload::Function {
            params,
            return_type,
            ..
        } => {
            roots.extend(params.iter().map(|param| param.type_root));
            roots.push(*return_type);
        }
        SignaturePayload::Aggregate { fields } => {
            roots.extend(fields.iter().map(|field| field.type_root));
        }
        SignaturePayload::Enum {
            backing_type,
            variants,
        } => {
            roots.push(*backing_type);
            for variant in variants {
                match &variant.payload {
                    SignatureVariantPayload::Unit => {}
                    SignatureVariantPayload::Tuple(types) => roots.extend(types.iter().copied()),
                    SignatureVariantPayload::Named(fields) => {
                        roots.extend(fields.iter().map(|field| field.type_root));
                    }
                }
            }
        }
        SignaturePayload::TypeAlias { target } => roots.push(*target),
        SignaturePayload::Value { explicit_type, .. } => {
            roots.extend(explicit_type.iter().copied());
        }
    }
    roots
}

impl SignatureSection {
    /// Returns a canonical trait declaration by definition identity.
    pub fn trait_record(&self, definition: &DefinitionId) -> Option<&SignatureTraitRecord> {
        self.traits
            .binary_search_by(|record| record.definition.cmp(definition))
            .ok()
            .map(|index| &self.traits[index])
    }

    /// Returns extension implementations targeting one canonical type root.
    pub fn extensions_for_target_root(
        &self,
        target_root: u32,
    ) -> impl Iterator<Item = &SignatureExtensionRecord> {
        self.extensions
            .iter()
            .filter(move |record| record.target_root == target_root)
    }

    /// Returns extension implementations for one canonical trait root.
    pub fn extensions_for_trait_root(
        &self,
        trait_root: u32,
    ) -> impl Iterator<Item = &SignatureExtensionRecord> {
        self.extensions
            .iter()
            .filter(move |record| record.trait_root == Some(trait_root))
    }

    pub fn validate(&self) -> Result<(), MetadataError> {
        if self.records.len() > MAX_ITEMS {
            return Err(MetadataError::TooManyItems);
        }
        for record in &self.records {
            validate_definition(&record.definition)?;
            if !(1..=16).contains(&record.kind)
                || record.flags & !SIGNATURE_FLAGS_MASK != 0
                || record.kind != record.definition.kind
                || !signature_flags_valid_for_kind(record.kind, record.flags)
            {
                return Err(MetadataError::InvalidManifest);
            }
            if record.type_roots.windows(2).any(|pair| pair[0] >= pair[1]) {
                return Err(MetadataError::InvalidManifest);
            }
            validate_signature_generic_params(&record.generic_params)?;
            validate_signature_where_predicates(&record.where_predicates)?;
            validate_signature_payload(record.kind, record.payload.as_ref())?;
            if record
                .members
                .windows(2)
                .any(|pair| pair[0].definition >= pair[1].definition)
            {
                return Err(MetadataError::InvalidManifest);
            }
            if record.members.len() > MAX_ITEMS {
                return Err(MetadataError::TooManyItems);
            }
            for member in &record.members {
                validate_definition(&member.definition)?;
                validate_string(&member.name)?;
                if member.kind != member.definition.kind
                    || member.flags & !SIGNATURE_FLAGS_MASK != 0
                    || member.type_roots.windows(2).any(|pair| pair[0] >= pair[1])
                    || member.type_roots.len() > MAX_ITEMS
                    || member.name != member.definition.name
                    || member.definition.owner.as_deref() != Some(&record.definition)
                {
                    return Err(MetadataError::InvalidManifest);
                }
            }
        }
        if self
            .records
            .windows(2)
            .any(|pair| pair[0].definition >= pair[1].definition)
        {
            return Err(MetadataError::InvalidManifest);
        }
        if self
            .traits
            .windows(2)
            .any(|pair| pair[0].definition >= pair[1].definition)
            || self.extensions.windows(2).any(|pair| {
                (&pair[0].module, pair[0].impl_id) >= (&pair[1].module, pair[1].impl_id)
            })
        {
            return Err(MetadataError::InvalidManifest);
        }
        for trait_record in &self.traits {
            validate_definition(&trait_record.definition)?;
            if trait_record.definition.kind != 9
                || !self
                    .records
                    .iter()
                    .any(|record| record.definition == trait_record.definition && record.kind == 9)
                || trait_record.supertraits.len() > MAX_ITEMS
                || trait_record
                    .builtin
                    .is_some_and(|tag| !matches!(tag, 1..=28))
            {
                return Err(MetadataError::InvalidManifest);
            }
            validate_signature_where_bounds(&trait_record.supertraits)?;
            validate_signature_members(&trait_record.definition, &trait_record.members)?;
        }
        for extension in &self.extensions {
            validate_module_id(&extension.module)?;
            if extension.impl_id == 0
                || extension.where_predicates.len() > MAX_ITEMS
                || extension.members.len() > MAX_ITEMS
                || extension.associated_types.len() > MAX_ITEMS
            {
                return Err(MetadataError::InvalidManifest);
            }
            validate_signature_generic_params(&extension.generic_params)?;
            validate_signature_where_predicates(&extension.where_predicates)?;
            if let Some(builtin) = &extension.builtin {
                validate_string(builtin)?;
            }
            let mut associated_names = std::collections::BTreeSet::new();
            for associated in &extension.associated_types {
                validate_string(&associated.name)?;
                if !associated_names.insert(&associated.name) {
                    return Err(MetadataError::InvalidManifest);
                }
            }
            let owner = extension
                .members
                .first()
                .and_then(|member| member.definition.owner.as_deref());
            if extension
                .members
                .iter()
                .any(|member| member.definition.owner.as_deref() != owner)
            {
                return Err(MetadataError::InvalidManifest);
            }
            for member in &extension.members {
                validate_definition(&member.definition)?;
                validate_string(&member.name)?;
                if member.definition.module != extension.module
                    || member.name != member.definition.name
                    || member.kind != member.definition.kind
                    || member.flags & !SIGNATURE_FLAGS_MASK != 0
                    || member.type_roots.windows(2).any(|p| p[0] >= p[1])
                {
                    return Err(MetadataError::InvalidManifest);
                }
            }
        }
        Ok(())
    }

    pub fn validate_type_roots(&self, graph: &StableTypeGraph) -> Result<(), MetadataError> {
        self.validate()?;
        let node_count = graph.nodes.len() as u32;
        if self
            .records
            .iter()
            .flat_map(|record| record.type_roots.iter())
            .any(|root| *root >= node_count)
            || self
                .records
                .iter()
                .filter_map(|record| record.payload.as_ref())
                .any(|payload| {
                    signature_payload_roots(payload)
                        .into_iter()
                        .any(|root| root >= node_count)
                })
            || self
                .records
                .iter()
                .flat_map(|record| record.members.iter())
                .flat_map(|member| member.type_roots.iter())
                .any(|root| *root >= node_count)
            || self.records.iter().any(|record| {
                record
                    .generic_params
                    .iter()
                    .filter_map(|param| param.type_root)
                    .any(|root| root >= node_count)
                    || signature_where_predicates_have_invalid_root(
                        &record.where_predicates,
                        node_count,
                    )
            })
            || self
                .traits
                .iter()
                .flat_map(|record| record.supertraits.iter())
                .any(|bound| signature_where_bound_has_invalid_root(bound, node_count))
            || self.extensions.iter().any(|record| {
                signature_where_predicates_have_invalid_root(&record.where_predicates, node_count)
            })
            || self.extensions.iter().any(|record| {
                record.target_root >= node_count
                    || record.trait_root.is_some_and(|root| root >= node_count)
            })
            || self
                .extensions
                .iter()
                .flat_map(|record| record.generic_params.iter())
                .filter_map(|param| param.type_root)
                .any(|root| root >= node_count)
            || self
                .traits
                .iter()
                .flat_map(|record| record.members.iter())
                .flat_map(|member| member.type_roots.iter())
                .any(|root| *root >= node_count)
            || self
                .extensions
                .iter()
                .flat_map(|record| record.members.iter())
                .flat_map(|member| member.type_roots.iter())
                .any(|root| *root >= node_count)
            || self
                .extensions
                .iter()
                .flat_map(|record| record.associated_types.iter())
                .any(|associated| associated.type_root >= node_count)
        {
            return Err(MetadataError::InvalidManifest);
        }
        Ok(())
    }
}

fn signature_flags_valid_for_kind(kind: u8, flags: u32) -> bool {
    let allowed = match kind {
        2 | 11 | 12 => {
            SIGNATURE_FLAG_HAS_BODY
                | SIGNATURE_FLAG_EXTERN
                | SIGNATURE_FLAG_CONST
                | SIGNATURE_FLAG_VARIADIC
        }
        3 => SIGNATURE_FLAG_EXTERN | SIGNATURE_FLAG_MUTABLE,
        4 => SIGNATURE_FLAG_CONST,
        5 => SIGNATURE_FLAG_EXTERN | SIGNATURE_FLAG_TUPLE,
        7 => SIGNATURE_FLAG_EXTERN,
        13 => SIGNATURE_FLAG_OPEN,
        _ => 0,
    };
    flags & !allowed == 0
}

fn signature_where_bound_has_invalid_root(bound: &SignatureWhereBound, node_count: u32) -> bool {
    bound.trait_root >= node_count
        || bound
            .associated_type_bindings
            .iter()
            .any(|binding| binding.type_root >= node_count)
}

fn signature_where_predicates_have_invalid_root(
    predicates: &[SignatureWherePredicate],
    node_count: u32,
) -> bool {
    predicates.iter().any(|predicate| {
        predicate.type_root >= node_count
            || predicate
                .bounds
                .iter()
                .any(|bound| signature_where_bound_has_invalid_root(bound, node_count))
    })
}

/// Checked generic/const body retained for downstream specialization.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TemplateRecord {
    /// Stable identity of the definition owning this template.
    pub definition: DefinitionId,
    /// Number of ABI parameters represented by the summary index domain.
    pub parameter_count: u32,
    /// Compiler-owned checked template payload.
    pub body: Vec<u8>,
    /// Compositional semantic summary used before body materialization.
    pub summary: Vec<u8>,
}

/// Canonical template section for public generic, inline, and CTFE items.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TemplateSection {
    pub records: Vec<TemplateRecord>,
}

/// Canonical, compositional semantic summary attached to a checked template.
/// Parameter indexes are zero-based and each vector is strictly ascending.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct TemplateSummary {
    pub returned_parameters: Vec<u32>,
    pub escaping_parameters: Vec<u32>,
    pub returned_captured_address_parameters: Vec<u32>,
    pub escaping_captured_address_parameters: Vec<u32>,
}

impl TemplateSummary {
    pub fn validate(&self) -> Result<(), MetadataError> {
        for values in [
            &self.returned_parameters,
            &self.escaping_parameters,
            &self.returned_captured_address_parameters,
            &self.escaping_captured_address_parameters,
        ] {
            if values.len() > MAX_ITEMS || values.windows(2).any(|pair| pair[0] >= pair[1]) {
                return Err(MetadataError::InvalidManifest);
            }
        }
        Ok(())
    }
}

/// Explicit target identity attached to target-specific native products.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord)]
pub struct NativeTarget {
    pub arch: String,
    pub vendor: String,
    pub os: String,
    pub env: String,
    pub abi: String,
    pub endian: String,
    pub pointer_width: u32,
}

/// One target/profile-specific native object retained by a package artifact.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NativeObject {
    pub key: String,
    /// Stable code-generation fingerprint of this object.
    pub fingerprint: [u64; 2],
    pub bytes: Vec<u8>,
}

/// Canonical target-native package payload.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NativeSection {
    pub target: NativeTarget,
    pub profile: u8,
    pub optimization: u8,
    pub objects: Vec<NativeObject>,
}

/// One stable export in a package module's public surface.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PublicSurfaceExport {
    /// Exported spelling in the containing module.
    pub name: String,
    /// Namespace tag: `0` for values, `1` for types.
    pub namespace: u8,
    /// Stable definition reached by this export (including re-exports).
    pub target: DefinitionId,
    /// Optional enum definition owning a variant export.
    pub parent_enum: Option<DefinitionId>,
    /// Provenance tag: `0` direct declaration, `1` public using.
    pub source: u8,
}

/// Complete, resolved public surfaces for a compiled package.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PublicSurfaceSection {
    pub package: PackageId,
    /// Canonical module path and its exported names.
    pub modules: Vec<PublicSurfaceModule>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PublicSurfaceModule {
    pub path: String,
    /// Exported child module names and canonical module identities.
    pub modules: Vec<(String, ModuleId)>,
    pub exports: Vec<PublicSurfaceExport>,
}

impl PublicSurfaceSection {
    pub fn validate(&self) -> Result<(), MetadataError> {
        validate_id(&self.package)?;
        if self.modules.len() > MAX_ITEMS {
            return Err(MetadataError::TooManyItems);
        }
        for module in &self.modules {
            validate_string(&module.path)?;
            for (name, target) in &module.modules {
                validate_string(name)?;
                validate_id(&target.package)?;
                validate_string(&target.path)?;
            }
            for export in &module.exports {
                validate_string(&export.name)?;
                if export.namespace > 1 || export.source > 1 {
                    return Err(MetadataError::InvalidManifest);
                }
                validate_definition(&export.target)?;
                if let Some(parent) = &export.parent_enum {
                    validate_definition(parent)?;
                }
            }
            if module.modules.windows(2).any(|w| w[0].0 >= w[1].0)
                || module.exports.windows(2).any(|w| {
                    (w[0].name.as_str(), w[0].namespace) >= (w[1].name.as_str(), w[1].namespace)
                })
            {
                return Err(MetadataError::InvalidManifest);
            }
        }
        if self.modules.windows(2).any(|w| w[0].path >= w[1].path) {
            return Err(MetadataError::InvalidManifest);
        }
        Ok(())
    }
}

impl NativeSection {
    pub fn validate(&self) -> Result<(), MetadataError> {
        validate_target(&self.target)?;
        if self.profile > 1 || self.optimization > 5 {
            return Err(MetadataError::InvalidManifest);
        }
        if self.objects.len() > MAX_ITEMS {
            return Err(MetadataError::TooManyItems);
        }
        for object in &self.objects {
            validate_string(&object.key)?;
            validate_bytes(&object.bytes)?;
            if object.bytes.is_empty() {
                return Err(MetadataError::InvalidManifest);
            }
        }
        if self
            .objects
            .windows(2)
            .any(|pair| pair[0].key >= pair[1].key)
        {
            return Err(MetadataError::InvalidManifest);
        }
        Ok(())
    }
}

impl TemplateSection {
    pub fn validate(&self) -> Result<(), MetadataError> {
        if self.records.len() > MAX_ITEMS {
            return Err(MetadataError::TooManyItems);
        }
        for record in &self.records {
            validate_definition(&record.definition)?;
            validate_bytes(&record.body)?;
            validate_bytes(&record.summary)?;
            if record.body.is_empty() || record.summary.is_empty() {
                return Err(MetadataError::InvalidManifest);
            }
            let summary = decode_template_summary(&record.summary)?;
            if summary
                .returned_parameters
                .iter()
                .chain(summary.escaping_parameters.iter())
                .chain(summary.returned_captured_address_parameters.iter())
                .chain(summary.escaping_captured_address_parameters.iter())
                .any(|index| *index >= record.parameter_count)
            {
                return Err(MetadataError::InvalidManifest);
            }
        }
        if self
            .records
            .windows(2)
            .find(|pair| pair[0].definition >= pair[1].definition)
            .is_some()
        {
            return Err(MetadataError::InvalidManifest);
        }
        Ok(())
    }
}

/// Decoded canonical declaration facts carried by an interface record.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StableDeclaration {
    /// Definition kind tag from the package identity domain.
    pub kind: u8,
    /// Visibility tag (`0` private through `3` public).
    pub visibility: u8,
    /// Stable symbol identities of declared generic parameters.
    pub generics: Vec<u64>,
}

/// Decodes the compiler-owned declaration payload used in interface records.
pub fn decode_declaration(bytes: &[u8]) -> Result<StableDeclaration, MetadataError> {
    if bytes.len() > MAX_PACKAGE_BYTES {
        return Err(MetadataError::TooLarge);
    }
    let mut cursor = Cursor::new(bytes);
    let mut magic = [0; DECLARATION_MAGIC.len()];
    read_exact(&mut cursor, &mut magic)?;
    if magic != *DECLARATION_MAGIC {
        return Err(MetadataError::InvalidManifest);
    }
    let kind = read_u8(&mut cursor)?;
    if !(1..=16).contains(&kind) {
        return Err(MetadataError::InvalidManifest);
    }
    let visibility = read_u8(&mut cursor)?;
    if visibility > 3 {
        return Err(MetadataError::InvalidManifest);
    }
    let count = bounded_count(get_u32(&mut cursor)?)?;
    let mut generics = Vec::with_capacity(count);
    for _ in 0..count {
        generics.push(get_u64(&mut cursor)?);
    }
    if cursor.position() != bytes.len() as u64 {
        return Err(MetadataError::InvalidManifest);
    }
    Ok(StableDeclaration {
        kind,
        visibility,
        generics,
    })
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
                    if *trait_id == 255 {
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
        StableTraitId::Builtin(tag) if *tag <= 27 => Ok(()),
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

impl InterfaceSection {
    /// Validates bounded fields and canonical definition ordering.
    pub fn validate(&self) -> Result<(), MetadataError> {
        if self.records.len() > MAX_ITEMS {
            return Err(MetadataError::TooManyItems);
        }
        for record in &self.records {
            validate_definition(&record.definition)?;
            validate_bytes(&record.declaration)?;
            if record.type_roots.windows(2).any(|pair| pair[0] >= pair[1]) {
                return Err(MetadataError::InvalidManifest);
            }
        }
        if self
            .records
            .windows(2)
            .find(|pair| pair[0].definition >= pair[1].definition)
            .is_some()
        {
            return Err(MetadataError::InvalidManifest);
        }
        Ok(())
    }

    /// Validates record type-root references against a canonical graph.
    pub fn validate_type_roots(&self, graph: &StableTypeGraph) -> Result<(), MetadataError> {
        self.validate()?;
        let node_count = graph.nodes.len() as u32;
        if self
            .records
            .iter()
            .flat_map(|record| record.type_roots.iter())
            .any(|root| *root >= node_count)
        {
            return Err(MetadataError::InvalidManifest);
        }
        Ok(())
    }
}

fn validate_signature_generic_params(
    params: &[SignatureGenericParam],
) -> Result<(), MetadataError> {
    let mut names = std::collections::BTreeSet::new();
    for param in params {
        validate_string(&param.name)?;
        if param.kind > 1
            || !names.insert(&param.name)
            || (param.kind == 0 && param.type_root.is_some())
            || (param.kind == 1 && param.type_root.is_none())
        {
            return Err(MetadataError::InvalidManifest);
        }
    }
    Ok(())
}

fn validate_signature_where_bounds(bounds: &[SignatureWhereBound]) -> Result<(), MetadataError> {
    if bounds.len() > MAX_ITEMS {
        return Err(MetadataError::TooManyItems);
    }
    for bound in bounds {
        if bound.associated_type_bindings.len() > MAX_ITEMS {
            return Err(MetadataError::TooManyItems);
        }
        for binding in &bound.associated_type_bindings {
            validate_string(&binding.name)?;
        }
    }
    Ok(())
}

fn validate_signature_where_predicates(
    predicates: &[SignatureWherePredicate],
) -> Result<(), MetadataError> {
    if predicates.len() > MAX_ITEMS {
        return Err(MetadataError::TooManyItems);
    }
    for predicate in predicates {
        validate_signature_where_bounds(&predicate.bounds)?;
    }
    Ok(())
}

fn validate_signature_members(
    owner: &DefinitionId,
    members: &[SignatureMember],
) -> Result<(), MetadataError> {
    if members.len() > MAX_ITEMS
        || members
            .windows(2)
            .any(|pair| pair[0].definition >= pair[1].definition)
    {
        return Err(MetadataError::InvalidManifest);
    }
    for member in members {
        validate_definition(&member.definition)?;
        validate_string(&member.name)?;
        if member.name != member.definition.name
            || member.kind != member.definition.kind
            || member.flags & !SIGNATURE_FLAGS_MASK != 0
            || member.type_roots.windows(2).any(|pair| pair[0] >= pair[1])
            || member.definition.owner.as_deref() != Some(owner)
        {
            return Err(MetadataError::InvalidManifest);
        }
    }
    Ok(())
}

/// Indexed target-independent declarations supplied by one compiled package.
///
/// Entries retain package-owned stable identities. Consumers must remap them
/// before constructing any session-local module, definition, or type handle.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CompiledPackageInterface {
    manifest: Arc<PackageManifest>,
    interface: Arc<InterfaceSection>,
    type_graph: Option<Arc<StableTypeGraph>>,
    templates: Option<Arc<TemplateSection>>,
    native: Option<Arc<NativeSection>>,
    public_surface: Option<Arc<PublicSurfaceSection>>,
    signatures: Option<Arc<SignatureSection>>,
    record_indexes: Arc<BTreeMap<DefinitionId, usize>>,
}

impl CompiledPackageInterface {
    /// Builds an indexed interface view after validating all manifest bindings.
    pub fn from_artifact(artifact: &PackageArtifact) -> Result<Self, MetadataError> {
        let interface = artifact.interface()?.unwrap_or(InterfaceSection {
            records: Vec::new(),
        });
        let type_graph = artifact.type_graph()?;
        let templates = artifact.templates()?;
        let native = artifact.native()?;
        let public_surface = artifact.public_surface()?;
        let signatures = artifact.signatures()?;
        if let Some(surface) = &public_surface {
            surface.validate()?;
            if surface.package != artifact.manifest().package {
                return Err(MetadataError::InvalidManifest);
            }
            if surface.modules.len() != artifact.manifest().modules.len()
                || surface.modules.iter().any(|module| {
                    !artifact
                        .manifest()
                        .modules
                        .iter()
                        .any(|m| m.path == module.path)
                })
            {
                return Err(MetadataError::InvalidManifest);
            }
            let mut allowed_packages = std::collections::BTreeSet::new();
            allowed_packages.insert(artifact.manifest().package.clone());
            allowed_packages.extend(
                artifact
                    .manifest()
                    .dependencies
                    .iter()
                    .map(|d| d.package.clone()),
            );
            for module in &surface.modules {
                for (_, child) in &module.modules {
                    if !allowed_packages.contains(&child.package) {
                        return Err(MetadataError::InvalidManifest);
                    }
                    if child.package == artifact.manifest().package
                        && !artifact
                            .manifest()
                            .modules
                            .iter()
                            .any(|m| m.path == child.path)
                    {
                        return Err(MetadataError::InvalidManifest);
                    }
                }
                for export in &module.exports {
                    if !allowed_packages.contains(&export.target.module.package) {
                        return Err(MetadataError::InvalidManifest);
                    }
                    if export.target.module.package == artifact.manifest().package
                        && !artifact
                            .manifest()
                            .modules
                            .iter()
                            .any(|m| m.path == export.target.module.path)
                    {
                        return Err(MetadataError::InvalidManifest);
                    }
                    if export.source == 0
                        && export.target.module.package != artifact.manifest().package
                    {
                        return Err(MetadataError::InvalidManifest);
                    }
                    if let Some(parent) = &export.parent_enum {
                        if !allowed_packages.contains(&parent.module.package) {
                            return Err(MetadataError::InvalidManifest);
                        }
                        if parent.module.package == artifact.manifest().package
                            && !artifact
                                .manifest()
                                .modules
                                .iter()
                                .any(|m| m.path == parent.module.path)
                        {
                            return Err(MetadataError::InvalidManifest);
                        }
                    }
                }
            }
        }
        if let Some(templates) = &templates {
            templates.validate()?;
            if templates
                .records
                .iter()
                .any(|record| record.definition.module.package != artifact.manifest().package)
            {
                return Err(MetadataError::InvalidManifest);
            }
        }
        if let Some(signatures) = &signatures {
            signatures.validate()?;
            if signatures
                .records
                .iter()
                .any(|record| record.definition.module.package != artifact.manifest().package)
                || signatures
                    .traits
                    .iter()
                    .any(|record| record.definition.module.package != artifact.manifest().package)
                || signatures.extensions.iter().any(|record| {
                    record.members.iter().any(|member| {
                        member.definition.module.package != artifact.manifest().package
                    })
                })
            {
                return Err(MetadataError::InvalidManifest);
            }
        }
        if let Some(signatures) = &signatures {
            signatures.validate()?;
            if signatures.records.iter().any(|record| {
                record.definition.module.package != artifact.manifest().package
                    || !interface
                        .records
                        .iter()
                        .any(|item| item.definition == record.definition)
                    || record.members.iter().any(|member| {
                        member.definition.module.package != artifact.manifest().package
                            || !interface
                                .records
                                .iter()
                                .any(|item| item.definition == member.definition)
                    })
            }) || signatures.traits.iter().any(|record| {
                record.definition.module.package != artifact.manifest().package
                    || !interface
                        .records
                        .iter()
                        .any(|item| item.definition == record.definition)
                    || record.members.iter().any(|member| {
                        !interface
                            .records
                            .iter()
                            .any(|item| item.definition == member.definition)
                    })
            }) || signatures.extensions.iter().any(|record| {
                record.module.package != artifact.manifest().package
                    || !artifact
                        .manifest()
                        .modules
                        .iter()
                        .any(|module| module.path == record.module.path)
                    || record.members.iter().any(|member| {
                        member.definition.module.package != artifact.manifest().package
                            || !interface
                                .records
                                .iter()
                                .any(|item| item.definition == member.definition)
                    })
            }) {
                return Err(MetadataError::InvalidManifest);
            }
        }
        if let Some(graph) = &type_graph {
            interface.validate_type_roots(graph)?;
            if let Some(signatures) = &signatures {
                signatures.validate_type_roots(graph)?;
            }
        } else {
            if interface
                .records
                .iter()
                .any(|record| !record.type_roots.is_empty())
                || signatures.as_ref().is_some_and(|section| {
                    section
                        .records
                        .iter()
                        .any(|record| !record.type_roots.is_empty())
                        || section.traits.iter().any(|record| {
                            !record.supertraits.is_empty()
                                || record
                                    .members
                                    .iter()
                                    .any(|member| !member.type_roots.is_empty())
                        })
                        || !section.extensions.is_empty()
                })
            {
                return Err(MetadataError::InvalidManifest);
            }
        }
        let record_indexes = interface
            .records
            .iter()
            .enumerate()
            .map(|(index, record)| (record.definition.clone(), index))
            .collect();
        Ok(Self {
            manifest: Arc::new(artifact.manifest().clone()),
            interface: Arc::new(interface),
            type_graph: type_graph.map(Arc::new),
            templates: templates.map(Arc::new),
            native: native.map(Arc::new),
            public_surface: public_surface.map(Arc::new),
            signatures: signatures.map(Arc::new),
            record_indexes: Arc::new(record_indexes),
        })
    }

    /// Returns the validated package manifest.
    pub fn manifest(&self) -> &PackageManifest {
        &self.manifest
    }

    /// Returns canonical module identities declared by this package.
    pub fn module_identities(&self) -> impl Iterator<Item = ModuleId> + '_ {
        self.manifest.modules.iter().map(|module| ModuleId {
            package: self.manifest.package.clone(),
            path: module.path.clone(),
        })
    }

    /// Resolves one canonical module identity to its manifest record.
    pub fn module(&self, identity: &ModuleId) -> Option<&ModuleInterface> {
        (identity.package == self.manifest.package)
            .then(|| {
                self.manifest
                    .modules
                    .iter()
                    .find(|module| module.path == identity.path)
            })
            .flatten()
    }

    /// Returns canonical records in definition-identity order.
    pub fn records(&self) -> &[InterfaceRecord] {
        &self.interface.records
    }

    /// Returns the canonical signature type graph, when published.
    pub fn type_graph(&self) -> Option<&StableTypeGraph> {
        self.type_graph.as_ref().map(|graph| &**graph)
    }

    /// Returns the optional checked template section.
    pub fn templates(&self) -> Option<&TemplateSection> {
        self.templates.as_ref().map(|templates| &**templates)
    }

    /// Returns the optional target-specific native payload.
    pub fn native(&self) -> Option<&NativeSection> {
        self.native.as_ref().map(|native| &**native)
    }

    pub fn public_surface(&self) -> Option<&PublicSurfaceSection> {
        self.public_surface.as_ref().map(|surface| &**surface)
    }

    /// Returns the optional target-independent signature section.
    pub fn signatures(&self) -> Option<&SignatureSection> {
        self.signatures.as_ref().map(|section| &**section)
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
            .filter(move |record| record.definition.module.path == module)
    }
}

/// Kind of lazily loaded package payload.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
#[repr(u8)]
pub enum SectionKind {
    Interface = 1,
    Templates = 2,
    Native = 3,
    TypeGraph = 4,
    PublicSurface = 5,
    Signatures = 6,
}
impl SectionKind {
    fn decode(value: u8) -> Option<Self> {
        match value {
            1 => Some(Self::Interface),
            2 => Some(Self::Templates),
            3 => Some(Self::Native),
            4 => Some(Self::TypeGraph),
            5 => Some(Self::PublicSurface),
            6 => Some(Self::Signatures),
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
            if dependency.package == self.package {
                return Err(MetadataError::InvalidManifest);
            }
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
    bytes: Arc<Vec<u8>>,
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
            bytes: Arc::new(bytes),
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

    /// Decodes the optional canonical stable type-graph section.
    pub fn type_graph(&self) -> Result<Option<StableTypeGraph>, MetadataError> {
        self.section(SectionKind::TypeGraph)?
            .map(decode_type_graph)
            .transpose()
    }

    /// Decodes the optional checked generic/const template section.
    pub fn templates(&self) -> Result<Option<TemplateSection>, MetadataError> {
        self.section(SectionKind::Templates)?
            .map(decode_templates)
            .transpose()
    }

    /// Decodes the optional target-specific native section.
    pub fn native(&self) -> Result<Option<NativeSection>, MetadataError> {
        self.section(SectionKind::Native)?
            .map(decode_native)
            .transpose()
    }

    /// Decodes the optional complete resolved public-surface section.
    pub fn public_surface(&self) -> Result<Option<PublicSurfaceSection>, MetadataError> {
        self.section(SectionKind::PublicSurface)?
            .map(decode_public_surface)
            .transpose()
    }

    /// Decodes the optional target-independent signature section.
    pub fn signatures(&self) -> Result<Option<SignatureSection>, MetadataError> {
        self.section(SectionKind::Signatures)?
            .map(decode_signatures)
            .transpose()
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
        .filter(|record| record.definition.module.path == module)
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
        put_definition(&mut output, &record.definition)?;
        put_bytes(&mut output, &record.declaration)?;
        put_list_len(&mut output, record.type_roots.len())?;
        for root in &record.type_roots {
            put_u32(&mut output, *root);
        }
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
        let definition = read_definition(&mut cursor)?;
        records.push(InterfaceRecord {
            definition,
            declaration: get_bytes(&mut cursor)?,
            type_roots: read_refs(&mut cursor)?,
        });
    }
    if cursor.position() != bytes.len() as u64 {
        return Err(MetadataError::InvalidManifest);
    }
    let section = InterfaceSection { records };
    section.validate()?;
    Ok(section)
}

/// Encodes canonical target-independent signature facts.
fn put_signature_payload(
    output: &mut Vec<u8>,
    payload: Option<&SignaturePayload>,
) -> Result<(), MetadataError> {
    let Some(payload) = payload else {
        output.push(0);
        return Ok(());
    };
    output.push(1);
    match payload {
        SignaturePayload::Function {
            params,
            return_type,
            attributes,
        } => {
            output.push(1);
            put_list_len(output, params.len())?;
            for param in params {
                match &param.name {
                    Some(name) => {
                        output.push(1);
                        put_string(output, name)?;
                    }
                    None => output.push(0),
                }
                output.push(param.receiver);
                put_u32(output, param.type_root);
            }
            put_u32(output, *return_type);
            put_list_len(output, attributes.len())?;
            for attribute in attributes {
                put_u32(output, *attribute);
            }
        }
        SignaturePayload::Aggregate { fields } => {
            output.push(2);
            put_signature_fields(output, fields)?;
        }
        SignaturePayload::Enum {
            backing_type,
            variants,
        } => {
            output.push(3);
            put_u32(output, *backing_type);
            put_list_len(output, variants.len())?;
            for variant in variants {
                put_definition(output, &variant.definition)?;
                put_string(output, &variant.name)?;
                match &variant.payload {
                    SignatureVariantPayload::Unit => output.push(0),
                    SignatureVariantPayload::Tuple(types) => {
                        output.push(1);
                        put_refs(output, types)?;
                    }
                    SignatureVariantPayload::Named(fields) => {
                        output.push(2);
                        put_signature_fields(output, fields)?;
                    }
                }
            }
        }
        SignaturePayload::TypeAlias { target } => {
            output.push(4);
            put_u32(output, *target);
        }
        SignaturePayload::Value {
            explicit_type,
            builtin,
        } => {
            output.push(5);
            match explicit_type {
                Some(root) => {
                    output.push(1);
                    put_u32(output, *root);
                }
                None => output.push(0),
            }
            match builtin {
                Some(tag) => {
                    output.push(1);
                    put_u32(output, *tag);
                }
                None => output.push(0),
            }
        }
    }
    Ok(())
}

fn put_signature_fields(
    output: &mut Vec<u8>,
    fields: &[SignatureField],
) -> Result<(), MetadataError> {
    put_list_len(output, fields.len())?;
    for field in fields {
        put_definition(output, &field.definition)?;
        put_string(output, &field.name)?;
        put_u32(output, field.type_root);
    }
    Ok(())
}

fn read_signature_payload(
    cursor: &mut Cursor<&[u8]>,
) -> Result<Option<SignaturePayload>, MetadataError> {
    match read_u8(cursor)? {
        0 => Ok(None),
        1 => Ok(Some(match read_u8(cursor)? {
            1 => {
                let count = bounded_count(get_u32(cursor)?)?;
                let mut params = Vec::with_capacity(count);
                for _ in 0..count {
                    let name = match read_u8(cursor)? {
                        0 => None,
                        1 => Some(get_string(cursor)?),
                        _ => return Err(MetadataError::InvalidManifest),
                    };
                    params.push(SignatureParameter {
                        name,
                        receiver: read_u8(cursor)?,
                        type_root: get_u32(cursor)?,
                    });
                }
                let return_type = get_u32(cursor)?;
                let attr_count = bounded_count(get_u32(cursor)?)?;
                let mut attributes = Vec::with_capacity(attr_count);
                for _ in 0..attr_count {
                    attributes.push(get_u32(cursor)?);
                }
                SignaturePayload::Function {
                    params,
                    return_type,
                    attributes,
                }
            }
            2 => SignaturePayload::Aggregate {
                fields: read_signature_fields(cursor)?,
            },
            3 => {
                let backing_type = get_u32(cursor)?;
                let count = bounded_count(get_u32(cursor)?)?;
                let mut variants = Vec::with_capacity(count);
                for _ in 0..count {
                    let definition = read_definition(cursor)?;
                    let name = get_string(cursor)?;
                    let payload = match read_u8(cursor)? {
                        0 => SignatureVariantPayload::Unit,
                        1 => SignatureVariantPayload::Tuple(read_refs(cursor)?),
                        2 => SignatureVariantPayload::Named(read_signature_fields(cursor)?),
                        _ => return Err(MetadataError::InvalidManifest),
                    };
                    variants.push(SignatureVariant {
                        definition,
                        name,
                        payload,
                    });
                }
                SignaturePayload::Enum {
                    backing_type,
                    variants,
                }
            }
            4 => SignaturePayload::TypeAlias {
                target: get_u32(cursor)?,
            },
            5 => SignaturePayload::Value {
                explicit_type: match read_u8(cursor)? {
                    0 => None,
                    1 => Some(get_u32(cursor)?),
                    _ => return Err(MetadataError::InvalidManifest),
                },
                builtin: match read_u8(cursor)? {
                    0 => None,
                    1 => Some(get_u32(cursor)?),
                    _ => return Err(MetadataError::InvalidManifest),
                },
            },
            _ => return Err(MetadataError::InvalidManifest),
        })),
        _ => Err(MetadataError::InvalidManifest),
    }
}

fn read_signature_fields(cursor: &mut Cursor<&[u8]>) -> Result<Vec<SignatureField>, MetadataError> {
    let count = bounded_count(get_u32(cursor)?)?;
    let mut fields = Vec::with_capacity(count);
    for _ in 0..count {
        fields.push(SignatureField {
            definition: read_definition(cursor)?,
            name: get_string(cursor)?,
            type_root: get_u32(cursor)?,
        });
    }
    Ok(fields)
}

/// Encodes canonical target-independent signature facts.
pub fn encode_signatures(section: &SignatureSection) -> Result<Vec<u8>, MetadataError> {
    section.validate()?;
    let mut output = Vec::new();
    output.extend_from_slice(SIGNATURE_MAGIC);
    put_u32(&mut output, SIGNATURE_SCHEMA);
    put_list_len(&mut output, section.records.len())?;
    for record in &section.records {
        put_definition(&mut output, &record.definition)?;
        output.push(record.kind);
        put_u32(&mut output, record.flags);
        put_list_len(&mut output, record.type_roots.len())?;
        for root in &record.type_roots {
            put_u32(&mut output, *root);
        }
        put_list_len(&mut output, record.members.len())?;
        for member in &record.members {
            put_definition(&mut output, &member.definition)?;
            put_string(&mut output, &member.name)?;
            output.push(member.kind);
            put_u32(&mut output, member.flags);
            put_list_len(&mut output, member.type_roots.len())?;
            for root in &member.type_roots {
                put_u32(&mut output, *root);
            }
        }
        put_generic_params(&mut output, &record.generic_params)?;
        put_where_predicates(&mut output, &record.where_predicates)?;
        put_signature_payload(&mut output, record.payload.as_ref())?;
    }
    put_list_len(&mut output, section.traits.len())?;
    for record in &section.traits {
        put_definition(&mut output, &record.definition)?;
        put_list_len(&mut output, record.supertraits.len())?;
        for supertrait in &record.supertraits {
            put_where_bound(&mut output, supertrait)?;
        }
        put_members(&mut output, &record.members)?;
        put_optional_u32(&mut output, record.builtin);
    }
    put_list_len(&mut output, section.extensions.len())?;
    for record in &section.extensions {
        put_module_id(&mut output, &record.module)?;
        output.extend_from_slice(&record.impl_id.to_le_bytes());
        put_u32(&mut output, record.target_root);
        match record.trait_root {
            Some(root) => {
                output.push(1);
                put_u32(&mut output, root);
            }
            None => output.push(0),
        }
        put_generic_params(&mut output, &record.generic_params)?;
        put_where_predicates(&mut output, &record.where_predicates)?;
        put_members(&mut output, &record.members)?;
        put_list_len(&mut output, record.associated_types.len())?;
        for associated in &record.associated_types {
            put_string(&mut output, &associated.name)?;
            put_u32(&mut output, associated.type_root);
        }
        match &record.builtin {
            Some(builtin) => {
                output.push(1);
                put_string(&mut output, builtin)?;
            }
            None => output.push(0),
        }
    }
    if output.len() > MAX_PACKAGE_BYTES {
        return Err(MetadataError::TooLarge);
    }
    Ok(output)
}

/// Decodes and validates canonical target-independent signature facts.
pub fn decode_signatures(bytes: &[u8]) -> Result<SignatureSection, MetadataError> {
    if bytes.len() > MAX_PACKAGE_BYTES {
        return Err(MetadataError::TooLarge);
    }
    let mut cursor = Cursor::new(bytes);
    let mut magic = [0; 8];
    read_exact(&mut cursor, &mut magic)?;
    if magic != *SIGNATURE_MAGIC {
        return Err(MetadataError::BadMagic);
    }
    let schema = get_u32(&mut cursor)?;
    if schema != SIGNATURE_SCHEMA {
        return Err(MetadataError::Schema(schema));
    }
    let count = bounded_count(get_u32(&mut cursor)?)?;
    let mut records = Vec::with_capacity(count);
    for _ in 0..count {
        let definition = read_definition(&mut cursor)?;
        let kind = read_u8(&mut cursor)?;
        let flags = get_u32(&mut cursor)?;
        let type_roots = read_refs(&mut cursor)?;
        let member_len = bounded_count(get_u32(&mut cursor)?)?;
        let mut members = Vec::with_capacity(member_len);
        for _ in 0..member_len {
            let definition = read_definition(&mut cursor)?;
            let name = get_string(&mut cursor)?;
            let kind = read_u8(&mut cursor)?;
            let flags = get_u32(&mut cursor)?;
            let type_roots = read_refs(&mut cursor)?;
            members.push(SignatureMember {
                definition,
                name,
                kind,
                flags,
                type_roots,
            });
        }
        records.push(SignatureRecord {
            definition,
            kind,
            flags,
            type_roots,
            members,
            generic_params: read_generic_params(&mut cursor)?,
            where_predicates: read_where_predicates(&mut cursor)?,
            payload: read_signature_payload(&mut cursor)?,
        });
    }
    let trait_len = bounded_count(get_u32(&mut cursor)?)?;
    let mut traits = Vec::with_capacity(trait_len);
    for _ in 0..trait_len {
        let definition = read_definition(&mut cursor)?;
        let supertrait_count = bounded_count(get_u32(&mut cursor)?)?;
        let mut supertraits = Vec::with_capacity(supertrait_count);
        for _ in 0..supertrait_count {
            supertraits.push(read_where_bound(&mut cursor)?);
        }
        traits.push(SignatureTraitRecord {
            definition,
            supertraits,
            members: read_members(&mut cursor)?,
            builtin: read_optional_u32(&mut cursor)?,
        });
    }
    let extension_len = bounded_count(get_u32(&mut cursor)?)?;
    let mut extensions = Vec::with_capacity(extension_len);
    for _ in 0..extension_len {
        let module = read_module_id(&mut cursor)?;
        let impl_id = get_u64(&mut cursor)?;
        let target_root = get_u32(&mut cursor)?;
        let trait_root = match read_u8(&mut cursor)? {
            0 => None,
            1 => Some(get_u32(&mut cursor)?),
            _ => return Err(MetadataError::InvalidManifest),
        };
        extensions.push(SignatureExtensionRecord {
            module,
            impl_id,
            target_root,
            trait_root,
            generic_params: read_generic_params(&mut cursor)?,
            where_predicates: read_where_predicates(&mut cursor)?,
            members: read_members(&mut cursor)?,
            associated_types: {
                let count = bounded_count(get_u32(&mut cursor)?)?;
                let mut values = Vec::with_capacity(count);
                for _ in 0..count {
                    values.push(SignatureAssociatedType {
                        name: get_string(&mut cursor)?,
                        type_root: get_u32(&mut cursor)?,
                    });
                }
                values
            },
            builtin: match read_u8(&mut cursor)? {
                0 => None,
                1 => Some(get_string(&mut cursor)?),
                _ => return Err(MetadataError::InvalidManifest),
            },
        });
    }
    if cursor.position() != bytes.len() as u64 {
        return Err(MetadataError::InvalidManifest);
    }
    let section = SignatureSection {
        records,
        traits,
        extensions,
    };
    section.validate()?;
    Ok(section)
}

/// Encodes canonical checked generic/const templates.
pub fn encode_templates(section: &TemplateSection) -> Result<Vec<u8>, MetadataError> {
    section.validate()?;
    let mut output = Vec::new();
    output.extend_from_slice(TEMPLATE_MAGIC);
    put_u32(&mut output, TEMPLATE_SCHEMA);
    put_list_len(&mut output, section.records.len())?;
    for record in &section.records {
        put_definition(&mut output, &record.definition)?;
        put_u32(&mut output, record.parameter_count);
        put_bytes(&mut output, &record.body)?;
        put_bytes(&mut output, &record.summary)?;
    }
    if output.len() > MAX_PACKAGE_BYTES {
        return Err(MetadataError::TooLarge);
    }
    Ok(output)
}

/// Decodes and validates canonical checked generic/const templates.
pub fn decode_templates(bytes: &[u8]) -> Result<TemplateSection, MetadataError> {
    if bytes.len() > MAX_PACKAGE_BYTES {
        return Err(MetadataError::TooLarge);
    }
    let mut cursor = Cursor::new(bytes);
    let mut magic = [0; 8];
    read_exact(&mut cursor, &mut magic)?;
    if magic != *TEMPLATE_MAGIC {
        return Err(MetadataError::BadMagic);
    }
    let schema = get_u32(&mut cursor)?;
    if schema != TEMPLATE_SCHEMA {
        return Err(MetadataError::Schema(schema));
    }
    let count = bounded_count(get_u32(&mut cursor)?)?;
    let mut records = Vec::with_capacity(count);
    for _ in 0..count {
        records.push(TemplateRecord {
            definition: read_definition(&mut cursor)?,
            parameter_count: get_u32(&mut cursor)?,
            body: get_bytes(&mut cursor)?,
            summary: get_bytes(&mut cursor)?,
        });
    }
    if cursor.position() != bytes.len() as u64 {
        return Err(MetadataError::InvalidManifest);
    }
    let section = TemplateSection { records };
    section.validate()?;
    Ok(section)
}

/// Encodes a canonical template semantic summary.
pub fn encode_template_summary(summary: &TemplateSummary) -> Result<Vec<u8>, MetadataError> {
    summary.validate()?;
    let mut output = Vec::new();
    output.extend_from_slice(TEMPLATE_SUMMARY_MAGIC);
    put_u32(&mut output, TEMPLATE_SUMMARY_SCHEMA);
    for values in [
        &summary.returned_parameters,
        &summary.escaping_parameters,
        &summary.returned_captured_address_parameters,
        &summary.escaping_captured_address_parameters,
    ] {
        put_list_len(&mut output, values.len())?;
        for value in values {
            put_u32(&mut output, *value);
        }
    }
    Ok(output)
}

/// Decodes and validates a canonical template semantic summary.
pub fn decode_template_summary(bytes: &[u8]) -> Result<TemplateSummary, MetadataError> {
    if bytes.len() > MAX_PACKAGE_BYTES {
        return Err(MetadataError::TooLarge);
    }
    let mut cursor = Cursor::new(bytes);
    let mut magic = [0; 8];
    read_exact(&mut cursor, &mut magic)?;
    if magic != *TEMPLATE_SUMMARY_MAGIC {
        return Err(MetadataError::BadMagic);
    }
    if get_u32(&mut cursor)? != TEMPLATE_SUMMARY_SCHEMA {
        return Err(MetadataError::Schema(TEMPLATE_SUMMARY_SCHEMA));
    }
    let read_values = |cursor: &mut Cursor<&[u8]>| -> Result<Vec<u32>, MetadataError> {
        let count = bounded_count(get_u32(cursor)?)?;
        let mut values = Vec::with_capacity(count);
        for _ in 0..count {
            values.push(get_u32(cursor)?);
        }
        Ok(values)
    };
    let summary = TemplateSummary {
        returned_parameters: read_values(&mut cursor)?,
        escaping_parameters: read_values(&mut cursor)?,
        returned_captured_address_parameters: read_values(&mut cursor)?,
        escaping_captured_address_parameters: read_values(&mut cursor)?,
    };
    if cursor.position() != bytes.len() as u64 {
        return Err(MetadataError::InvalidManifest);
    }
    summary.validate()?;
    Ok(summary)
}

/// Encodes complete resolved public surfaces without source/session identities.
pub fn encode_public_surface(section: &PublicSurfaceSection) -> Result<Vec<u8>, MetadataError> {
    section.validate()?;
    let mut output = Vec::new();
    output.extend_from_slice(PUBLIC_SURFACE_MAGIC);
    put_u32(&mut output, PUBLIC_SURFACE_SCHEMA);
    put_id(&mut output, &section.package)?;
    put_list_len(&mut output, section.modules.len())?;
    for module in &section.modules {
        put_string(&mut output, &module.path)?;
        put_list_len(&mut output, module.modules.len())?;
        for (name, target) in &module.modules {
            put_string(&mut output, name)?;
            put_id(&mut output, &target.package)?;
            put_string(&mut output, &target.path)?;
        }
        put_list_len(&mut output, module.exports.len())?;
        for export in &module.exports {
            put_string(&mut output, &export.name)?;
            output.push(export.namespace);
            put_definition(&mut output, &export.target)?;
            match &export.parent_enum {
                Some(parent) => {
                    output.push(1);
                    put_definition(&mut output, parent)?;
                }
                None => output.push(0),
            }
            output.push(export.source);
        }
    }
    if output.len() > MAX_PACKAGE_BYTES {
        return Err(MetadataError::TooLarge);
    }
    Ok(output)
}

/// Decodes and strictly validates complete resolved public surfaces.
pub fn decode_public_surface(bytes: &[u8]) -> Result<PublicSurfaceSection, MetadataError> {
    if bytes.len() > MAX_PACKAGE_BYTES {
        return Err(MetadataError::TooLarge);
    }
    let mut cursor = Cursor::new(bytes);
    let mut magic = [0; PUBLIC_SURFACE_MAGIC.len()];
    read_exact(&mut cursor, &mut magic)?;
    if magic != *PUBLIC_SURFACE_MAGIC {
        return Err(MetadataError::BadMagic);
    }
    let schema = get_u32(&mut cursor)?;
    if schema != PUBLIC_SURFACE_SCHEMA {
        return Err(MetadataError::Schema(schema));
    }
    let package = get_id(&mut cursor)?;
    let count = bounded_count(get_u32(&mut cursor)?)?;
    let mut modules = Vec::with_capacity(count);
    for _ in 0..count {
        let path = get_string(&mut cursor)?;
        let module_count = bounded_count(get_u32(&mut cursor)?)?;
        let mut child_modules = Vec::with_capacity(module_count);
        for _ in 0..module_count {
            child_modules.push((
                get_string(&mut cursor)?,
                ModuleId {
                    package: get_id(&mut cursor)?,
                    path: get_string(&mut cursor)?,
                },
            ));
        }
        let export_count = bounded_count(get_u32(&mut cursor)?)?;
        let mut exports = Vec::with_capacity(export_count);
        for _ in 0..export_count {
            let name = get_string(&mut cursor)?;
            let namespace = read_u8(&mut cursor)?;
            let target = read_definition(&mut cursor)?;
            let parent_enum = match read_u8(&mut cursor)? {
                0 => None,
                1 => Some(read_definition(&mut cursor)?),
                _ => return Err(MetadataError::InvalidManifest),
            };
            let source = read_u8(&mut cursor)?;
            exports.push(PublicSurfaceExport {
                name,
                namespace,
                target,
                parent_enum,
                source,
            });
        }
        modules.push(PublicSurfaceModule {
            path,
            modules: child_modules,
            exports,
        });
    }
    if cursor.position() != bytes.len() as u64 {
        return Err(MetadataError::InvalidManifest);
    }
    let section = PublicSurfaceSection { package, modules };
    section.validate()?;
    Ok(section)
}

/// Encodes a canonical target-native section.
pub fn encode_native(section: &NativeSection) -> Result<Vec<u8>, MetadataError> {
    section.validate()?;
    let mut output = Vec::new();
    output.extend_from_slice(NATIVE_MAGIC);
    put_u32(&mut output, NATIVE_SCHEMA);
    put_target(&mut output, &section.target)?;
    output.push(section.profile);
    output.push(section.optimization);
    put_list_len(&mut output, section.objects.len())?;
    for object in &section.objects {
        put_string(&mut output, &object.key)?;
        output.extend_from_slice(&object.fingerprint[0].to_le_bytes());
        output.extend_from_slice(&object.fingerprint[1].to_le_bytes());
        put_bytes(&mut output, &object.bytes)?;
    }
    Ok(output)
}

/// Decodes and validates a canonical target-native section.
pub fn decode_native(bytes: &[u8]) -> Result<NativeSection, MetadataError> {
    let mut cursor = Cursor::new(bytes);
    let mut magic = [0; NATIVE_MAGIC.len()];
    read_exact(&mut cursor, &mut magic)?;
    if magic != *NATIVE_MAGIC {
        return Err(MetadataError::InvalidManifest);
    }
    let schema = get_u32(&mut cursor)?;
    if schema != NATIVE_SCHEMA {
        return Err(MetadataError::Schema(schema));
    }
    let target = get_target(&mut cursor)?;
    let profile = read_u8(&mut cursor)?;
    let optimization = read_u8(&mut cursor)?;
    let count = bounded_count(get_u32(&mut cursor)?)?;
    let mut objects = Vec::with_capacity(count);
    for _ in 0..count {
        objects.push(NativeObject {
            key: get_string(&mut cursor)?,
            fingerprint: [get_u64(&mut cursor)?, get_u64(&mut cursor)?],
            bytes: get_bytes(&mut cursor)?,
        });
    }
    if cursor.position() != bytes.len() as u64 {
        return Err(MetadataError::InvalidManifest);
    }
    let section = NativeSection {
        target,
        profile,
        optimization,
        objects,
    };
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
                    if tag > 27 {
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
        if record.definition.module.package != manifest.package
            || manifest
                .modules
                .binary_search_by(|module| module.path.cmp(&record.definition.module.path))
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
fn validate_target(target: &NativeTarget) -> Result<(), MetadataError> {
    validate_string(&target.arch)?;
    validate_string(&target.vendor)?;
    validate_string(&target.os)?;
    validate_optional_string(&target.env)?;
    validate_optional_string(&target.abi)?;
    validate_string(&target.endian)?;
    if !matches!(target.pointer_width, 8 | 16 | 32 | 64 | 128) {
        return Err(MetadataError::InvalidManifest);
    }
    Ok(())
}
fn put_target(output: &mut Vec<u8>, target: &NativeTarget) -> Result<(), MetadataError> {
    validate_target(target)?;
    put_string(output, &target.arch)?;
    put_string(output, &target.vendor)?;
    put_string(output, &target.os)?;
    put_optional_string(output, &target.env)?;
    put_optional_string(output, &target.abi)?;
    put_string(output, &target.endian)?;
    put_u32(output, target.pointer_width);
    Ok(())
}
fn get_target(cursor: &mut Cursor<&[u8]>) -> Result<NativeTarget, MetadataError> {
    let target = NativeTarget {
        arch: get_string(cursor)?,
        vendor: get_string(cursor)?,
        os: get_string(cursor)?,
        env: get_optional_string(cursor)?,
        abi: get_optional_string(cursor)?,
        endian: get_string(cursor)?,
        pointer_width: get_u32(cursor)?,
    };
    validate_target(&target)?;
    Ok(target)
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
fn validate_optional_string(value: &str) -> Result<(), MetadataError> {
    if value.is_empty() {
        Ok(())
    } else {
        validate_string(value)
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
            (tag <= 27)
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

fn put_generic_params(
    output: &mut Vec<u8>,
    params: &[SignatureGenericParam],
) -> Result<(), MetadataError> {
    put_list_len(output, params.len())?;
    for param in params {
        put_string(output, &param.name)?;
        output.push(param.kind);
        match param.type_root {
            Some(root) => {
                output.push(1);
                put_u32(output, root);
            }
            None => output.push(0),
        }
    }
    Ok(())
}

fn read_generic_params(
    cursor: &mut Cursor<&[u8]>,
) -> Result<Vec<SignatureGenericParam>, MetadataError> {
    let count = bounded_count(get_u32(cursor)?)?;
    let mut params = Vec::with_capacity(count);
    for _ in 0..count {
        let name = get_string(cursor)?;
        let kind = read_u8(cursor)?;
        let type_root = match read_u8(cursor)? {
            0 => None,
            1 => Some(get_u32(cursor)?),
            _ => return Err(MetadataError::InvalidManifest),
        };
        params.push(SignatureGenericParam {
            name,
            kind,
            type_root,
        });
    }
    Ok(params)
}

fn put_where_predicates(
    output: &mut Vec<u8>,
    predicates: &[SignatureWherePredicate],
) -> Result<(), MetadataError> {
    put_list_len(output, predicates.len())?;
    for predicate in predicates {
        put_u32(output, predicate.type_root);
        put_list_len(output, predicate.bounds.len())?;
        for bound in &predicate.bounds {
            put_where_bound(output, bound)?;
        }
    }
    Ok(())
}

fn read_where_predicates(
    cursor: &mut Cursor<&[u8]>,
) -> Result<Vec<SignatureWherePredicate>, MetadataError> {
    let count = bounded_count(get_u32(cursor)?)?;
    let mut predicates = Vec::with_capacity(count);
    for _ in 0..count {
        let type_root = get_u32(cursor)?;
        let bound_count = bounded_count(get_u32(cursor)?)?;
        let mut bounds = Vec::with_capacity(bound_count);
        for _ in 0..bound_count {
            bounds.push(read_where_bound(cursor)?);
        }
        predicates.push(SignatureWherePredicate { type_root, bounds });
    }
    Ok(predicates)
}

fn put_where_bound(output: &mut Vec<u8>, bound: &SignatureWhereBound) -> Result<(), MetadataError> {
    put_u32(output, bound.trait_root);
    put_list_len(output, bound.associated_type_bindings.len())?;
    for binding in &bound.associated_type_bindings {
        put_string(output, &binding.name)?;
        put_u32(output, binding.type_root);
    }
    Ok(())
}

fn read_where_bound(cursor: &mut Cursor<&[u8]>) -> Result<SignatureWhereBound, MetadataError> {
    let trait_root = get_u32(cursor)?;
    let binding_count = bounded_count(get_u32(cursor)?)?;
    let mut associated_type_bindings = Vec::with_capacity(binding_count);
    for _ in 0..binding_count {
        associated_type_bindings.push(SignatureAssociatedTypeBinding {
            name: get_string(cursor)?,
            type_root: get_u32(cursor)?,
        });
    }
    Ok(SignatureWhereBound {
        trait_root,
        associated_type_bindings,
    })
}

fn put_optional_u32(output: &mut Vec<u8>, value: Option<u32>) {
    match value {
        Some(value) => {
            output.push(1);
            put_u32(output, value);
        }
        None => output.push(0),
    }
}

fn read_optional_u32(cursor: &mut Cursor<&[u8]>) -> Result<Option<u32>, MetadataError> {
    match read_u8(cursor)? {
        0 => Ok(None),
        1 => Ok(Some(get_u32(cursor)?)),
        _ => Err(MetadataError::InvalidManifest),
    }
}

fn put_members(output: &mut Vec<u8>, members: &[SignatureMember]) -> Result<(), MetadataError> {
    put_list_len(output, members.len())?;
    for member in members {
        put_definition(output, &member.definition)?;
        put_string(output, &member.name)?;
        output.push(member.kind);
        put_u32(output, member.flags);
        put_refs(output, &member.type_roots)?;
    }
    Ok(())
}

fn read_members(cursor: &mut Cursor<&[u8]>) -> Result<Vec<SignatureMember>, MetadataError> {
    let count = bounded_count(get_u32(cursor)?)?;
    let mut members = Vec::with_capacity(count);
    for _ in 0..count {
        members.push(SignatureMember {
            definition: read_definition(cursor)?,
            name: get_string(cursor)?,
            kind: read_u8(cursor)?,
            flags: get_u32(cursor)?,
            type_roots: read_refs(cursor)?,
        });
    }
    Ok(members)
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
fn put_optional_string(output: &mut Vec<u8>, value: &str) -> Result<(), MetadataError> {
    validate_optional_string(value)?;
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
fn get_optional_string(cursor: &mut Cursor<&[u8]>) -> Result<String, MetadataError> {
    let length = bounded_len(get_u32(cursor)?)?;
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
    fn manifest_rejects_self_dependency() {
        let mut manifest = sample();
        manifest.dependencies.push(PackageDependency {
            package: manifest.package.clone(),
            interface_hash: [1; 32],
        });
        assert_eq!(encode(&manifest), Err(MetadataError::InvalidManifest));
    }

    #[test]
    fn native_section_round_trips_target_identity_and_objects() {
        let section = NativeSection {
            target: NativeTarget {
                arch: "x86_64".into(),
                vendor: "unknown".into(),
                os: "linux".into(),
                env: "gnu".into(),
                abi: "".into(),
                endian: "little".into(),
                pointer_width: 64,
            },
            profile: 1,
            optimization: 2,
            objects: vec![NativeObject {
                key: "unit-0".into(),
                fingerprint: [1, 2],
                bytes: vec![0, 1, 2, 3],
            }],
        };
        let bytes = encode_native(&section).unwrap();
        assert_eq!(decode_native(&bytes).unwrap(), section);
        let mut trailing = bytes;
        trailing.push(0);
        assert_eq!(
            decode_native(&trailing),
            Err(MetadataError::InvalidManifest)
        );
    }

    #[test]
    fn type_graph_section_is_decoded_and_indexed() {
        let manifest = sample();
        let graph = StableTypeGraph {
            nodes: vec![StableTypeNode::Primitive(3)],
            roots: vec![0],
        };
        let graph_bytes = encode_type_graph(&graph).unwrap();
        let bytes = encode_artifact(&manifest, &[(SectionKind::TypeGraph, &graph_bytes)]).unwrap();
        let artifact = PackageArtifact::open(bytes).unwrap();
        assert_eq!(artifact.type_graph().unwrap(), Some(graph.clone()));
        let indexed = CompiledPackageInterface::from_artifact(&artifact).unwrap();
        assert_eq!(indexed.type_graph(), Some(&graph));
    }

    #[test]
    fn compiled_interface_exposes_package_qualified_module_identities() {
        let mut manifest = PackageManifest::current(PackageId {
            namespace: "example".into(),
            name: "demo".into(),
            version: "1.0.0".into(),
        });
        manifest.modules.push(ModuleInterface {
            path: "src/lib.nia".into(),
            interface_hash: interface_module_hash(
                &InterfaceSection { records: vec![] },
                "src/lib.nia",
            )
            .unwrap(),
        });
        let artifact = PackageArtifact::open(
            encode_artifact(
                &manifest,
                &[(
                    SectionKind::Interface,
                    &encode_interface(&InterfaceSection { records: vec![] }).unwrap(),
                )],
            )
            .unwrap(),
        )
        .unwrap();
        let indexed = CompiledPackageInterface::from_artifact(&artifact).unwrap();
        let modules = indexed.module_identities().collect::<Vec<_>>();
        assert_eq!(modules.len(), 1);
        assert_eq!(modules[0].package, manifest.package);
        assert_eq!(modules[0].path, "src/lib.nia");
        assert!(indexed.module(&modules[0]).is_some());
    }

    #[test]
    fn applied_nominal_type_graph_round_trips_const_arguments() {
        let package = sample().package;
        let graph = StableTypeGraph {
            nodes: vec![
                StableTypeNode::Primitive(12),
                StableTypeNode::NamedApplied {
                    definition: DefinitionId {
                        module: ModuleId {
                            package,
                            path: "m".into(),
                        },
                        name: "Array".into(),
                        kind: 5,
                        disambiguator: 0,
                        owner: None,
                    },
                    arguments: Vec::new(),
                    const_arguments: vec![StableConstArg {
                        ty: 0,
                        value: StableConstValue::Integer {
                            bits: 4,
                            signed: false,
                        },
                    }],
                },
            ],
            roots: vec![1],
        };
        assert_eq!(
            decode_type_graph(&encode_type_graph(&graph).unwrap()).unwrap(),
            graph
        );
    }

    #[test]
    fn template_section_round_trips_and_rejects_unsorted_records() {
        let package = sample().package;
        let section = TemplateSection {
            records: vec![TemplateRecord {
                definition: DefinitionId {
                    module: ModuleId {
                        package,
                        path: "std/io".into(),
                    },
                    name: "write".into(),
                    kind: 2,
                    disambiguator: 0,
                    owner: None,
                },
                parameter_count: 0,
                body: vec![1, 2, 3],
                summary: encode_template_summary(&TemplateSummary::default()).unwrap(),
            }],
        };
        let bytes = encode_templates(&section).unwrap();
        assert_eq!(decode_templates(&bytes).unwrap(), section);
        let mut invalid = section.clone();
        invalid.records.push(invalid.records[0].clone());
        assert_eq!(
            encode_templates(&invalid),
            Err(MetadataError::InvalidManifest)
        );
        let mut incomplete = section;
        incomplete.records[0].body.clear();
        assert_eq!(
            encode_templates(&incomplete),
            Err(MetadataError::InvalidManifest)
        );
    }

    #[test]
    fn signature_section_round_trips_and_rejects_unknown_flags() {
        let record = SignatureRecord {
            definition: DefinitionId {
                module: ModuleId {
                    package: sample().package,
                    path: "m".into(),
                },
                name: "f".into(),
                kind: 2,
                disambiguator: 0,
                owner: None,
            },
            kind: 2,
            flags: SIGNATURE_FLAG_HAS_BODY | SIGNATURE_FLAG_CONST,
            type_roots: vec![0, 2],
            members: Vec::new(),
            generic_params: Vec::new(),
            where_predicates: Vec::new(),
            payload: Some(SignaturePayload::Function {
                params: vec![SignatureParameter {
                    name: Some("value".into()),
                    receiver: 0,
                    type_root: 0,
                }],
                return_type: 2,
                attributes: vec![1, 0x101],
            }),
        };
        let section = SignatureSection {
            records: vec![record],
            traits: Vec::new(),
            extensions: Vec::new(),
        };
        let bytes = encode_signatures(&section).unwrap();
        assert_eq!(decode_signatures(&bytes).unwrap(), section);
        let mut invalid = section.clone();
        invalid.records[0].flags |= 1 << 31;
        assert_eq!(
            encode_signatures(&invalid),
            Err(MetadataError::InvalidManifest)
        );
        invalid = section.clone();
        invalid.records[0].kind = 3;
        assert_eq!(
            encode_signatures(&invalid),
            Err(MetadataError::InvalidManifest)
        );
    }

    #[test]
    fn signature_section_rejects_unsorted_roots_and_trailing_bytes() {
        let definition = DefinitionId {
            module: ModuleId {
                package: sample().package,
                path: "m".into(),
            },
            name: "f".into(),
            kind: 2,
            disambiguator: 0,
            owner: None,
        };
        let invalid = SignatureSection {
            records: vec![SignatureRecord {
                definition,
                kind: 2,
                flags: 0,
                type_roots: vec![2, 1],
                members: Vec::new(),
                generic_params: Vec::new(),
                where_predicates: Vec::new(),
                payload: None,
            }],
            traits: Vec::new(),
            extensions: Vec::new(),
        };
        assert_eq!(
            encode_signatures(&invalid),
            Err(MetadataError::InvalidManifest)
        );
        let mut bytes = encode_signatures(&SignatureSection::default()).unwrap();
        bytes.push(0);
        assert_eq!(
            decode_signatures(&bytes),
            Err(MetadataError::InvalidManifest)
        );
    }

    #[test]
    fn signature_section_round_trips_nested_members() {
        let package = sample().package;
        let parent = DefinitionId {
            module: ModuleId {
                package: package.clone(),
                path: "m".into(),
            },
            name: "User".into(),
            kind: 5,
            disambiguator: 0,
            owner: None,
        };
        let child = DefinitionId {
            module: parent.module.clone(),
            name: "field".into(),
            kind: 6,
            disambiguator: 0,
            owner: Some(Box::new(parent.clone())),
        };
        let section = SignatureSection {
            records: vec![SignatureRecord {
                definition: parent.clone(),
                kind: 5,
                flags: 0,
                type_roots: Vec::new(),
                members: vec![SignatureMember {
                    definition: child,
                    name: "field".into(),
                    kind: 6,
                    flags: 0,
                    type_roots: vec![0],
                }],
                generic_params: Vec::new(),
                where_predicates: Vec::new(),
                payload: None,
            }],
            traits: Vec::new(),
            extensions: Vec::new(),
        };
        let graph = StableTypeGraph {
            nodes: vec![StableTypeNode::Primitive(3)],
            roots: vec![0],
        };
        section.validate_type_roots(&graph).unwrap();
        assert_eq!(
            decode_signatures(&encode_signatures(&section).unwrap()).unwrap(),
            section
        );
    }

    #[test]
    fn signature_section_round_trips_trait_and_extension_indexes() {
        let package = sample().package;
        let trait_definition = DefinitionId {
            module: ModuleId {
                package: package.clone(),
                path: "m".into(),
            },
            name: "Display".into(),
            kind: 9,
            disambiguator: 0,
            owner: None,
        };
        let section = SignatureSection {
            records: vec![SignatureRecord {
                definition: trait_definition.clone(),
                kind: 9,
                flags: 0,
                type_roots: Vec::new(),
                members: Vec::new(),
                generic_params: vec![SignatureGenericParam {
                    name: "T".into(),
                    kind: 0,
                    type_root: None,
                }],
                where_predicates: Vec::new(),
                payload: None,
            }],
            traits: vec![SignatureTraitRecord {
                definition: trait_definition,
                supertraits: vec![SignatureWhereBound {
                    trait_root: 2,
                    associated_type_bindings: vec![SignatureAssociatedTypeBinding {
                        name: "Item".into(),
                        type_root: 0,
                    }],
                }],
                members: Vec::new(),
                builtin: Some(1),
            }],
            extensions: vec![SignatureExtensionRecord {
                module: ModuleId {
                    package: package.clone(),
                    path: "m".into(),
                },
                impl_id: 7,
                target_root: 0,
                trait_root: Some(2),
                generic_params: Vec::new(),
                where_predicates: vec![SignatureWherePredicate {
                    type_root: 1,
                    bounds: vec![SignatureWhereBound {
                        trait_root: 2,
                        associated_type_bindings: Vec::new(),
                    }],
                }],
                members: Vec::new(),
                associated_types: Vec::new(),
                builtin: Some("test.Display".into()),
            }],
        };
        let bytes = encode_signatures(&section).unwrap();
        assert_eq!(decode_signatures(&bytes).unwrap(), section);
        let graph = StableTypeGraph {
            nodes: vec![
                StableTypeNode::Primitive(1),
                StableTypeNode::Primitive(2),
                StableTypeNode::Primitive(3),
            ],
            roots: vec![0, 1, 2],
        };
        section.validate_type_roots(&graph).unwrap();
        assert!(
            section
                .trait_record(&section.traits[0].definition)
                .is_some()
        );
        assert_eq!(section.extensions_for_target_root(0).count(), 1);
        assert_eq!(section.extensions_for_trait_root(2).count(), 1);
    }

    #[test]
    fn compiled_interface_decodes_signature_section_lazily() {
        let section = SignatureSection::default();
        let bytes = encode_signatures(&section).unwrap();
        let artifact = PackageArtifact::open(
            encode_artifact(&sample(), &[(SectionKind::Signatures, &bytes)]).unwrap(),
        )
        .unwrap();
        let indexed = CompiledPackageInterface::from_artifact(&artifact).unwrap();
        assert_eq!(indexed.signatures(), Some(&section));
    }

    #[test]
    fn compiled_interface_rejects_signature_without_interface_record() {
        let package = sample().package;
        let section = SignatureSection {
            records: vec![SignatureRecord {
                definition: DefinitionId {
                    module: ModuleId {
                        package,
                        path: "m".into(),
                    },
                    name: "missing".into(),
                    kind: 2,
                    disambiguator: 0,
                    owner: None,
                },
                kind: 2,
                flags: 0,
                type_roots: Vec::new(),
                members: Vec::new(),
                generic_params: Vec::new(),
                where_predicates: Vec::new(),
                payload: None,
            }],
            traits: Vec::new(),
            extensions: Vec::new(),
        };
        let bytes = encode_signatures(&section).unwrap();
        let artifact = PackageArtifact::open(
            encode_artifact(&sample(), &[(SectionKind::Signatures, &bytes)]).unwrap(),
        )
        .unwrap();
        assert_eq!(
            CompiledPackageInterface::from_artifact(&artifact),
            Err(MetadataError::InvalidManifest)
        );
    }

    #[test]
    fn template_summary_round_trips_and_rejects_unsorted_parameters() {
        let summary = TemplateSummary {
            returned_parameters: vec![0, 2],
            escaping_parameters: vec![1],
            returned_captured_address_parameters: vec![],
            escaping_captured_address_parameters: vec![3, 5],
        };
        let bytes = encode_template_summary(&summary).unwrap();
        assert_eq!(decode_template_summary(&bytes).unwrap(), summary);
        let invalid = TemplateSummary {
            returned_parameters: vec![2, 1],
            ..summary
        };
        assert_eq!(
            encode_template_summary(&invalid),
            Err(MetadataError::InvalidManifest)
        );
    }

    #[test]
    fn template_section_rejects_non_summary_payloads() {
        let section = TemplateSection {
            records: vec![TemplateRecord {
                definition: DefinitionId {
                    module: ModuleId {
                        package: sample().package,
                        path: "m".into(),
                    },
                    name: "generic".into(),
                    kind: 2,
                    disambiguator: 0,
                    owner: None,
                },
                parameter_count: 0,
                body: vec![1],
                summary: b"opaque summary".to_vec(),
            }],
        };
        assert_eq!(section.validate(), Err(MetadataError::BadMagic));
    }

    #[test]
    fn template_section_rejects_summary_parameter_out_of_range() {
        let definition = DefinitionId {
            module: ModuleId {
                package: sample().package,
                path: "m".into(),
            },
            name: "generic".into(),
            kind: 2,
            disambiguator: 0,
            owner: None,
        };
        let summary = encode_template_summary(&TemplateSummary {
            returned_parameters: vec![1],
            ..Default::default()
        })
        .unwrap();
        let section = TemplateSection {
            records: vec![TemplateRecord {
                definition,
                parameter_count: 1,
                body: vec![1],
                summary,
            }],
        };
        assert_eq!(section.validate(), Err(MetadataError::InvalidManifest));
    }

    #[test]
    fn compiled_interface_decodes_templates_lazily() {
        let section = TemplateSection { records: vec![] };
        let bytes = encode_templates(&section).unwrap();
        let artifact = PackageArtifact::open(
            encode_artifact(&sample(), &[(SectionKind::Templates, &bytes)]).unwrap(),
        )
        .unwrap();
        let indexed = CompiledPackageInterface::from_artifact(&artifact).unwrap();
        assert_eq!(indexed.templates(), Some(&section));
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
                    module: ModuleId {
                        package,
                        path: "std/io".into(),
                    },
                    name: "write".into(),
                    kind: 2,
                    disambiguator: 0,
                    owner: None,
                },
                declaration: b"fn(Text) Unit".to_vec(),
                type_roots: Vec::new(),
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
                module: ModuleId {
                    package: package.clone(),
                    path: "m".into(),
                },
                name: "a".into(),
                kind: 2,
                disambiguator: 0,
                owner: None,
            },
            declaration: vec![1],
            type_roots: Vec::new(),
        };
        let second = InterfaceRecord {
            definition: DefinitionId {
                module: ModuleId {
                    package,
                    path: "m".into(),
                },
                name: "a".into(),
                kind: 2,
                disambiguator: 0,
                owner: None,
            },
            declaration: vec![2],
            type_roots: Vec::new(),
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
    fn interface_section_accepts_unpublished_parent_identity() {
        let package = sample().package;
        let child = DefinitionId {
            module: ModuleId {
                package: package.clone(),
                path: "m".into(),
            },
            name: "field".into(),
            kind: 6,
            disambiguator: 0,
            owner: Some(Box::new(DefinitionId {
                module: ModuleId {
                    package: package.clone(),
                    path: "m".into(),
                },
                name: "Missing".into(),
                kind: 5,
                disambiguator: 0,
                owner: None,
            })),
        };
        let section = InterfaceSection {
            records: vec![InterfaceRecord {
                definition: child,
                declaration: vec![1],
                type_roots: Vec::new(),
            }],
        };
        assert_eq!(section.validate(), Ok(()));
    }

    #[test]
    fn definition_owner_chain_has_a_bounded_depth() {
        let package = sample().package;
        let module = ModuleId {
            package,
            path: "m".into(),
        };
        let mut owner = None;
        for index in (0..=MAX_DEFINITION_DEPTH).rev() {
            owner = Some(Box::new(DefinitionId {
                module: module.clone(),
                name: format!("T{index}"),
                kind: 5,
                disambiguator: index as u64,
                owner,
            }));
        }
        let definition = DefinitionId {
            module,
            name: "field".into(),
            kind: 6,
            disambiguator: 0,
            owner,
        };
        assert_eq!(
            validate_definition(&definition),
            Err(MetadataError::TooManyItems)
        );
    }

    #[test]
    fn stable_type_graph_round_trips_and_rejects_forward_references() {
        let package = sample().package;
        let named = DefinitionId {
            module: ModuleId {
                package,
                path: "std/io".into(),
            },
            name: "Text".into(),
            kind: 5,
            disambiguator: 0,
            owner: None,
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
    fn stable_type_graph_round_trips_builtin_and_container_forms() {
        let graph = StableTypeGraph {
            nodes: vec![
                StableTypeNode::Primitive(3),
                StableTypeNode::VolatilePointer {
                    target: 0,
                    readonly: true,
                },
                StableTypeNode::Slice {
                    target: 0,
                    readonly: false,
                },
                StableTypeNode::Optional { element: 2 },
                StableTypeNode::ErrorUnion { error: 0, value: 3 },
                StableTypeNode::Vector {
                    element: 3,
                    lanes: 4,
                },
                StableTypeNode::Range {
                    kind: 5,
                    bound: None,
                },
                StableTypeNode::Callable {
                    parameters: vec![0, 1],
                    result: 3,
                    readonly: true,
                },
                StableTypeNode::BuiltinType(0),
                StableTypeNode::BuiltinTrait {
                    trait_id: 24,
                    arguments: vec![0],
                },
                StableTypeNode::SelfParam,
                StableTypeNode::Opaque,
            ],
            roots: vec![4, 5, 7, 9, 10, 11],
        };
        let bytes = encode_type_graph(&graph).unwrap();
        assert_eq!(decode_type_graph(&bytes).unwrap(), graph);
    }

    #[test]
    fn stable_type_graph_round_trips_trait_object_and_projection_forms() {
        let graph = StableTypeGraph {
            nodes: vec![
                StableTypeNode::Primitive(3),
                StableTypeNode::TraitObject {
                    readonly: false,
                    trait_id: StableTraitId::Builtin(24),
                    trait_arguments: vec![0],
                    trait_const_arguments: vec![],
                    associated_type_bindings: vec![StableAssociatedTypeBinding {
                        trait_id: None,
                        trait_arguments: vec![],
                        trait_const_arguments: vec![],
                        name: 42,
                        ty: 0,
                    }],
                },
                StableTypeNode::Projection {
                    self_ty: 0,
                    trait_id: StableTraitId::Builtin(24),
                    trait_arguments: vec![],
                    trait_const_arguments: vec![],
                    name: 42,
                },
            ],
            roots: vec![1, 2],
        };
        let bytes = encode_type_graph(&graph).unwrap();
        assert_eq!(decode_type_graph(&bytes).unwrap(), graph);
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

    #[test]
    fn stable_declaration_decoder_round_trips_compiler_payload() {
        let mut bytes = Vec::from(&b"NIADECL01"[..]);
        bytes.extend_from_slice(&[2, 3]);
        bytes.extend_from_slice(&2u32.to_le_bytes());
        bytes.extend_from_slice(&11u64.to_le_bytes());
        bytes.extend_from_slice(&22u64.to_le_bytes());
        assert_eq!(
            decode_declaration(&bytes).unwrap(),
            StableDeclaration {
                kind: 2,
                visibility: 3,
                generics: vec![11, 22],
            }
        );
    }

    #[test]
    fn stable_declaration_decoder_rejects_invalid_payloads() {
        let mut bytes = Vec::from(&b"NIADECL01"[..]);
        bytes.extend_from_slice(&[0, 3]);
        bytes.extend_from_slice(&0u32.to_le_bytes());
        assert!(matches!(
            decode_declaration(&bytes),
            Err(MetadataError::InvalidManifest)
        ));
    }

    #[test]
    fn public_surface_round_trips_and_rejects_unsorted_exports() {
        let package = PackageId {
            namespace: "acme".into(),
            name: "demo".into(),
            version: "1".into(),
        };
        let module = ModuleId {
            package: package.clone(),
            path: "main".into(),
        };
        let definition = DefinitionId {
            module: module.clone(),
            name: "run".into(),
            kind: 2,
            disambiguator: 0,
            owner: None,
        };
        let section = PublicSurfaceSection {
            package: package.clone(),
            modules: vec![PublicSurfaceModule {
                path: "main".into(),
                modules: vec![],
                exports: vec![PublicSurfaceExport {
                    name: "run".into(),
                    namespace: 0,
                    target: definition,
                    parent_enum: None,
                    source: 0,
                }],
            }],
        };
        let bytes = encode_public_surface(&section).unwrap();
        assert_eq!(decode_public_surface(&bytes).unwrap(), section);
        let mut invalid = section.clone();
        let duplicate = invalid.modules[0].exports[0].clone();
        invalid.modules[0].exports.push(duplicate);
        assert_eq!(
            encode_public_surface(&invalid),
            Err(MetadataError::InvalidManifest)
        );
        let mut conflicting = section;
        conflicting.modules[0].exports.push(PublicSurfaceExport {
            name: "run".into(),
            namespace: 0,
            target: DefinitionId {
                module: module,
                name: "other".into(),
                kind: 2,
                disambiguator: 0,
                owner: None,
            },
            parent_enum: None,
            source: 0,
        });
        conflicting.modules[0]
            .exports
            .sort_by(|a, b| (a.name.as_str(), a.namespace).cmp(&(b.name.as_str(), b.namespace)));
        assert_eq!(
            encode_public_surface(&conflicting),
            Err(MetadataError::InvalidManifest)
        );
    }

    #[test]
    fn compiled_interface_rejects_public_surface_package_mismatch() {
        let manifest = sample();
        let foreign = PackageId {
            namespace: "other".into(),
            name: "foreign".into(),
            version: "1".into(),
        };
        let section = PublicSurfaceSection {
            package: foreign,
            modules: vec![],
        };
        let bytes = encode_public_surface(&section).unwrap();
        let artifact = PackageArtifact::open(
            encode_artifact(&manifest, &[(SectionKind::PublicSurface, &bytes)]).unwrap(),
        )
        .unwrap();
        assert_eq!(
            CompiledPackageInterface::from_artifact(&artifact),
            Err(MetadataError::InvalidManifest)
        );
    }
}
