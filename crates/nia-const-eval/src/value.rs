use nia_ids::{GlobalDefId, InternedTyId, LocalId, ModuleId};
use nia_span::Span;
use nia_symbol::SymbolId;
use nia_ty::IntConst;
use std::collections::BTreeMap;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
/// Stable identity of mutable storage owned by one const-evaluation module.
pub struct ConstAllocationId {
    module_id: ModuleId,
    serial: u64,
}

impl ConstAllocationId {
    /// Creates an allocation identity from its owner module and local serial.
    pub const fn new(module_id: ModuleId, serial: u64) -> Self {
        Self { module_id, serial }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
/// Source identity assigned to an immutable frozen allocation.
///
/// Frozen pointers compare by this origin rather than recursively comparing
/// their pointee contents, preserving pointer identity for equal values.
pub struct ConstAllocationOrigin {
    module_id: Option<ModuleId>,
    span: Span,
}

impl ConstAllocationOrigin {
    /// Creates an origin at `span`, optionally owned by a module.
    pub const fn new(module_id: Option<ModuleId>, span: Span) -> Self {
        Self { module_id, span }
    }

    /// Returns the module that created the allocation, when known.
    pub const fn module_id(self) -> Option<ModuleId> {
        self.module_id
    }

    /// Returns the source span that identifies the allocation site.
    pub const fn span(self) -> Span {
        self.span
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
/// One projection from an allocation root to a nested const place.
pub enum ConstPointerPathElem {
    /// Selects a named aggregate field.
    Field(SymbolId),
    /// Selects an indexed sequence element.
    Index(usize),
}

#[derive(Debug, Clone)]
/// Pointer representation used during const evaluation.
pub enum ConstPointerValue {
    /// Immutable snapshot whose identity is its allocation origin.
    Frozen {
        /// Stable identity of the frozen allocation.
        origin: ConstAllocationOrigin,
        /// Whether writes through this pointer are forbidden.
        is_readonly: bool,
        /// Snapshot retained for dereferencing during evaluation.
        pointee: Box<ConstValue>,
    },
    /// Address of a live interpreter allocation and projection path.
    Place {
        /// Root allocation identity.
        allocation: ConstAllocationId,
        /// Projections from the allocation root to the addressed value.
        path: Vec<ConstPointerPathElem>,
    },
}

impl PartialEq for ConstPointerValue {
    fn eq(&self, other: &Self) -> bool {
        match (self, other) {
            (Self::Frozen { origin: lhs, .. }, Self::Frozen { origin: rhs, .. }) => lhs == rhs,
            (
                Self::Place {
                    allocation: lhs_allocation,
                    path: lhs_path,
                },
                Self::Place {
                    allocation: rhs_allocation,
                    path: rhs_path,
                },
            ) => lhs_allocation == rhs_allocation && lhs_path == rhs_path,
            _ => false,
        }
    }
}

#[derive(Debug, Clone, PartialEq)]
/// Runtime value produced by the const interpreter.
pub enum ConstValue {
    /// Arbitrary-width integer value.
    Int(IntConst),
    /// Floating-point value represented as `f64` by the interpreter.
    Float(f64),
    /// Boolean value.
    Bool(bool),
    /// Owned UTF-8 string value.
    String(String),
    /// Frozen or live-place pointer.
    Pointer(ConstPointerValue),
    /// Positional tuple elements.
    Tuple(Vec<ConstValue>),
    /// Homogeneous array elements.
    Array(Vec<ConstValue>),
    /// Fixed-lane vector elements.
    Vector(Vec<ConstValue>),
    /// Integer range value.
    Range(ConstRangeValue),
    /// Named structural fields.
    Struct(BTreeMap<SymbolId, ConstValue>),
    /// Raw union storage with ABI metadata.
    Union(ConstUnionValue),
    /// Enum discriminant and optional payload.
    Enum {
        /// Stable identity of the selected variant.
        variant: GlobalDefId,
        /// Payload encoded by the variant's shape.
        payload: ConstEnumPayload,
    },
    /// Optional payload, where `None` represents `null`.
    Optional(Option<Box<ConstValue>>),
    /// Success or error payload of an error union.
    ErrorUnion(Result<Box<ConstValue>, Box<ConstValue>>),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
/// Byte order used to encode scalar values into aggregate storage.
pub enum ConstEndianness {
    /// Least-significant byte first.
    Little,
    /// Most-significant byte first.
    Big,
}

impl ConstEndianness {
    /// Parses the target configuration's canonical endianness name.
    pub fn from_target_name(name: &str) -> Option<Self> {
        match name {
            "little" => Some(Self::Little),
            "big" => Some(Self::Big),
            _ => None,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
/// Scalar storage representation accepted by const ABI encoding.
pub enum ConstScalarType {
    /// Integer with an explicit bit width and signed interpretation.
    Integer {
        /// Storage width in bits.
        bits: u32,
        /// Whether decoded values use signed interpretation.
        signed: bool,
    },
    /// IEEE-754 binary32 storage.
    Float32,
    /// IEEE-754 binary64 storage.
    Float64,
    /// One-byte boolean storage restricted to zero or one.
    Bool,
    /// Four-byte Unicode scalar storage.
    Char,
}

impl ConstScalarType {
    /// Returns the exact byte width accepted by const ABI encoding.
    ///
    /// Integer storage must be non-zero, byte-aligned, and fit the evaluator's
    /// `u128` backing representation. Rejecting malformed descriptors here
    /// keeps every aggregate/vector caller from reconstructing those bounds.
    pub const fn byte_len(self) -> Option<usize> {
        match self {
            Self::Integer { bits, .. } if bits > 0 && bits <= 128 && bits % 8 == 0 => {
                Some((bits / 8) as usize)
            }
            Self::Integer { .. } => None,
            Self::Float32 | Self::Char => Some(4),
            Self::Float64 => Some(8),
            Self::Bool => Some(1),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
/// Target-specific ABI description used to encode union fields.
pub enum ConstAbiType {
    /// Scalar storage.
    Scalar(ConstScalarType),
    /// Pointer-sized storage represented by a relocation.
    Pointer {
        /// Pointer width in bytes for the target artifact.
        size: usize,
        /// Type of the relocated pointee.
        pointee: InternedTyId,
    },
    /// Contiguous fixed-length array storage.
    Array {
        /// ABI of each element.
        element: Box<ConstAbiType>,
        /// Number of elements.
        len: usize,
    },
    /// Fixed-size vector storage.
    Vector {
        /// Scalar representation of each lane.
        lane: ConstScalarType,
        /// Number of lanes.
        lanes: usize,
        /// Total target ABI size, including any padding.
        size: usize,
    },
    /// Named fields at explicit target ABI offsets.
    Struct {
        /// Fields in ABI layout order.
        fields: Vec<ConstAbiField>,
        /// Total target ABI size, including padding.
        size: usize,
    },
    /// Overlapping named field representations.
    Union {
        /// ABI of each selectable field.
        fields: BTreeMap<SymbolId, ConstAbiType>,
        /// Shared storage size.
        size: usize,
    },
}

#[derive(Debug, Clone, PartialEq, Eq)]
/// One named field in a const ABI struct layout.
pub struct ConstAbiField {
    /// Source-level field identity.
    pub name: SymbolId,
    /// Byte offset from the struct storage start.
    pub offset: usize,
    /// ABI representation of the field.
    pub ty: ConstAbiType,
}

impl ConstAbiType {
    /// Returns the encoded byte length, rejecting overflow or malformed scalars.
    pub fn byte_len(&self) -> Option<usize> {
        match self {
            Self::Scalar(scalar) => scalar.byte_len(),
            Self::Pointer { size, .. } => Some(*size),
            Self::Array { element, len } => element.byte_len()?.checked_mul(*len),
            Self::Vector { size, .. } => Some(*size),
            Self::Struct { size, .. } => Some(*size),
            Self::Union { size, .. } => Some(*size),
        }
    }
}

struct EncodedAbiValue {
    bytes: Vec<u8>,
    initialized: Vec<bool>,
    relocations: Vec<ConstRelocation>,
}

#[derive(Debug, Clone, PartialEq)]
/// Pointer relocation embedded in raw const storage.
///
/// The covered byte range is unavailable as ordinary initialized bytes; it is
/// decoded only when the requested pointer field exactly matches the
/// relocation's offset, width, and pointee type.
pub struct ConstRelocation {
    offset: usize,
    width: usize,
    pointee: InternedTyId,
    pointer: ConstPointerValue,
}

impl ConstRelocation {
    /// Returns the byte offset of the relocation.
    pub const fn offset(&self) -> usize {
        self.offset
    }

    /// Returns the target pointer width in bytes.
    pub const fn width(&self) -> usize {
        self.width
    }

    /// Returns the relocated const pointer.
    pub const fn pointer(&self) -> &ConstPointerValue {
        &self.pointer
    }

    /// Returns the expected pointee type.
    pub const fn pointee(&self) -> InternedTyId {
        self.pointee
    }
}

#[derive(Debug, Clone, PartialEq)]
/// Raw storage backing a const union value.
///
/// Initialization bits distinguish written bytes from padding or unwritten
/// tails. Pointer bytes are represented by relocations instead of fabricated
/// integer addresses. Writing a field replaces overlapping relocations and
/// initialization state while preserving the untouched storage tail.
pub struct ConstUnionValue {
    fields: BTreeMap<SymbolId, ConstAbiType>,
    bytes: Vec<u8>,
    initialized: Vec<bool>,
    relocations: Vec<ConstRelocation>,
    endianness: ConstEndianness,
}

impl ConstUnionValue {
    /// Creates union storage and writes its initial active field.
    pub fn new(
        fields: BTreeMap<SymbolId, ConstAbiType>,
        storage_size: usize,
        initial_field: SymbolId,
        value: ConstValue,
        endianness: ConstEndianness,
    ) -> Result<Self, String> {
        let mut union = Self {
            fields,
            bytes: vec![0; storage_size],
            initialized: vec![false; storage_size],
            relocations: Vec::new(),
            endianness,
        };
        union.write(initial_field, value)?;
        Ok(union)
    }

    /// Decodes `field` from the current raw storage.
    pub fn read(&self, field: SymbolId) -> Result<ConstValue, String> {
        let abi = self
            .fields
            .get(&field)
            .ok_or_else(|| "unknown const union field".to_string())?;
        let len = abi
            .byte_len()
            .ok_or_else(|| "const union field size is not representable".to_string())?;
        if len > self.bytes.len() {
            return Err("const union field exceeds its storage".to_string());
        }
        decode_abi_value(
            abi,
            &self.bytes[..len],
            &self.initialized[..len],
            &relocations_for_subrange(&self.relocations, 0, len)?,
            self.endianness,
        )
    }

    /// Encodes `value` into `field`, replacing overlapping storage state.
    pub fn write(&mut self, field: SymbolId, value: ConstValue) -> Result<(), String> {
        let abi = self
            .fields
            .get(&field)
            .ok_or_else(|| "unknown const union field".to_string())?;
        let encoded = encode_abi_value(abi, value, self.endianness)?;
        if encoded.bytes.len() > self.bytes.len() {
            return Err("const union field exceeds its storage".to_string());
        }
        let written_end = encoded.bytes.len();
        let mut initialized = self.initialized.clone();
        for relocation in &self.relocations {
            let relocation_end = relocation.offset + relocation.width;
            if relocation.offset < written_end {
                initialized[relocation.offset..relocation_end].fill(false);
            }
        }
        initialized[..written_end].copy_from_slice(&encoded.initialized);
        let mut relocations = self
            .relocations
            .iter()
            .filter(|relocation| relocation.offset >= written_end)
            .cloned()
            .collect::<Vec<_>>();
        relocations.extend(encoded.relocations);
        relocations.sort_by_key(|relocation| relocation.offset);
        validate_relocations(&relocations, self.bytes.len(), &initialized)?;

        self.bytes[..written_end].copy_from_slice(&encoded.bytes);
        self.initialized = initialized;
        self.relocations = relocations;
        Ok(())
    }

    /// Returns the raw storage bytes.
    pub fn bytes(&self) -> &[u8] {
        &self.bytes
    }

    /// Returns one initialization bit per storage byte.
    pub fn initialized(&self) -> &[bool] {
        &self.initialized
    }

    /// Returns pointer relocations sorted by byte offset.
    pub fn relocations(&self) -> &[ConstRelocation] {
        &self.relocations
    }

    /// Verifies that stored metadata still matches the owning type's ABI.
    pub fn validate_abi(
        &self,
        fields: &BTreeMap<SymbolId, ConstAbiType>,
        storage_size: usize,
        endianness: ConstEndianness,
    ) -> Result<(), String> {
        if self.fields != *fields {
            return Err("const union storage has the wrong field ABI".to_string());
        }
        if self.bytes.len() != storage_size || self.initialized.len() != storage_size {
            return Err("const union storage has the wrong size".to_string());
        }
        if self.endianness != endianness {
            return Err("const union storage has the wrong endianness".to_string());
        }
        validate_relocations(&self.relocations, storage_size, &self.initialized)?;
        Ok(())
    }
}

fn encode_abi_value(
    abi: &ConstAbiType,
    value: ConstValue,
    endianness: ConstEndianness,
) -> Result<EncodedAbiValue, String> {
    match (abi, value) {
        (ConstAbiType::Scalar(scalar), value) => {
            let bytes = encode_scalar(*scalar, value, endianness)?;
            let initialized = vec![true; bytes.len()];
            Ok(EncodedAbiValue {
                bytes,
                initialized,
                relocations: Vec::new(),
            })
        }
        (ConstAbiType::Pointer { size, pointee }, ConstValue::Pointer(pointer)) => {
            if *size == 0 {
                return Err("const union pointer field has zero-sized storage".to_string());
            }
            Ok(EncodedAbiValue {
                bytes: vec![0; *size],
                initialized: vec![true; *size],
                relocations: vec![ConstRelocation {
                    offset: 0,
                    width: *size,
                    pointee: *pointee,
                    pointer,
                }],
            })
        }
        (ConstAbiType::Pointer { .. }, _) => {
            Err("const union field value does not match its pointer type".to_string())
        }
        (ConstAbiType::Array { element, len }, ConstValue::Array(values)) => {
            if values.len() != *len {
                return Err("const union array field has the wrong length".to_string());
            }
            let capacity = abi
                .byte_len()
                .ok_or_else(|| "const union array field size is not representable".to_string())?;
            let mut bytes = Vec::with_capacity(capacity);
            let mut initialized = Vec::with_capacity(capacity);
            let mut relocations = Vec::new();
            for value in values {
                let encoded = encode_abi_value(element, value, endianness)?;
                let offset = bytes.len();
                relocations.extend(shift_relocations(encoded.relocations, offset)?);
                bytes.extend(encoded.bytes);
                initialized.extend(encoded.initialized);
            }
            Ok(EncodedAbiValue {
                bytes,
                initialized,
                relocations,
            })
        }
        (ConstAbiType::Array { .. }, _) => {
            Err("const union field value does not match its array type".to_string())
        }
        (ConstAbiType::Vector { lane, lanes, size }, ConstValue::Vector(values)) => {
            encode_vector(*lane, *lanes, *size, values, endianness)
        }
        (ConstAbiType::Vector { .. }, _) => {
            Err("const union field value does not match its vector type".to_string())
        }
        (ConstAbiType::Struct { fields, size }, ConstValue::Struct(mut values)) => {
            let mut bytes = vec![0; *size];
            let mut initialized = vec![false; *size];
            let mut relocations = Vec::new();
            for field in fields {
                let value = values
                    .remove(&field.name)
                    .ok_or_else(|| "const union struct field is missing".to_string())?;
                let encoded = encode_abi_value(&field.ty, value, endianness)?;
                let end = field
                    .offset
                    .checked_add(encoded.bytes.len())
                    .filter(|end| *end <= *size)
                    .ok_or_else(|| "const union struct field exceeds its layout".to_string())?;
                bytes[field.offset..end].copy_from_slice(&encoded.bytes);
                initialized[field.offset..end].copy_from_slice(&encoded.initialized);
                relocations.extend(shift_relocations(encoded.relocations, field.offset)?);
            }
            if !values.is_empty() {
                return Err("const union struct value has unknown fields".to_string());
            }
            Ok(EncodedAbiValue {
                bytes,
                initialized,
                relocations,
            })
        }
        (ConstAbiType::Struct { .. }, _) => {
            Err("const union field value does not match its struct type".to_string())
        }
        (ConstAbiType::Union { fields, size }, ConstValue::Union(value)) => {
            value.validate_abi(fields, *size, endianness)?;
            Ok(EncodedAbiValue {
                bytes: value.bytes,
                initialized: value.initialized,
                relocations: value.relocations,
            })
        }
        (ConstAbiType::Union { .. }, _) => {
            Err("const union field value does not match its union type".to_string())
        }
    }
}

fn decode_abi_value(
    abi: &ConstAbiType,
    bytes: &[u8],
    initialized: &[bool],
    relocations: &[ConstRelocation],
    endianness: ConstEndianness,
) -> Result<ConstValue, String> {
    if bytes.len() != initialized.len() {
        return Err("const union field storage metadata is inconsistent".to_string());
    }
    match abi {
        ConstAbiType::Scalar(scalar) => {
            if !relocations.is_empty() {
                return Err(
                    "const union scalar field reinterprets pointer relocation storage".to_string(),
                );
            }
            if initialized.iter().any(|byte| !byte) {
                return Err("const union field reads uninitialized storage".to_string());
            }
            decode_scalar(*scalar, bytes, endianness)
        }
        ConstAbiType::Pointer { size, .. } => {
            if bytes.len() != *size {
                return Err("const union pointer field storage has the wrong length".to_string());
            }
            if initialized.iter().any(|byte| !byte) {
                return Err("const union pointer field reads uninitialized storage".to_string());
            }
            let [relocation] = relocations else {
                return Err(
                    "const union pointer field requires one exact pointer relocation".to_string(),
                );
            };
            if relocation.offset != 0 || relocation.width != *size {
                return Err(
                    "const union pointer field requires one exact pointer relocation".to_string(),
                );
            }
            Ok(ConstValue::Pointer(relocation.pointer.clone()))
        }
        ConstAbiType::Array { element, len } => {
            let element_len = element
                .byte_len()
                .ok_or_else(|| "const union array element size is not representable".to_string())?;
            let expected_len = element_len
                .checked_mul(*len)
                .ok_or_else(|| "const union array field size is not representable".to_string())?;
            if bytes.len() != expected_len {
                return Err("const union array field storage has the wrong length".to_string());
            }
            let mut values = Vec::with_capacity(*len);
            for index in 0..*len {
                let start = index * element_len;
                let end = start + element_len;
                values.push(decode_abi_value(
                    element,
                    &bytes[start..end],
                    &initialized[start..end],
                    &relocations_for_subrange(relocations, start, element_len)?,
                    endianness,
                )?);
            }
            Ok(ConstValue::Array(values))
        }
        ConstAbiType::Vector { lane, lanes, size } => {
            if !relocations.is_empty() {
                return Err(
                    "const union vector field reinterprets pointer relocation storage".to_string(),
                );
            }
            decode_vector(*lane, *lanes, *size, bytes, initialized, endianness)
        }
        ConstAbiType::Struct { fields, size } => {
            if bytes.len() != *size {
                return Err("const union struct field storage has the wrong length".to_string());
            }
            let mut values = BTreeMap::new();
            for field in fields {
                let field_len = field.ty.byte_len().ok_or_else(|| {
                    "const union struct field size is not representable".to_string()
                })?;
                let end = field
                    .offset
                    .checked_add(field_len)
                    .filter(|end| *end <= *size)
                    .ok_or_else(|| "const union struct field exceeds its layout".to_string())?;
                values.insert(
                    field.name,
                    decode_abi_value(
                        &field.ty,
                        &bytes[field.offset..end],
                        &initialized[field.offset..end],
                        &relocations_for_subrange(relocations, field.offset, field_len)?,
                        endianness,
                    )?,
                );
            }
            Ok(ConstValue::Struct(values))
        }
        ConstAbiType::Union { fields, size } => {
            if bytes.len() != *size {
                return Err("const union field storage has the wrong length".to_string());
            }
            Ok(ConstValue::Union(ConstUnionValue {
                fields: fields.clone(),
                bytes: bytes.to_vec(),
                initialized: initialized.to_vec(),
                relocations: relocations.to_vec(),
                endianness,
            }))
        }
    }
}

fn shift_relocations(
    relocations: Vec<ConstRelocation>,
    offset: usize,
) -> Result<Vec<ConstRelocation>, String> {
    relocations
        .into_iter()
        .map(|mut relocation| {
            relocation.offset = relocation
                .offset
                .checked_add(offset)
                .ok_or_else(|| "const union relocation offset is not representable".to_string())?;
            Ok(relocation)
        })
        .collect()
}

fn relocations_for_subrange(
    relocations: &[ConstRelocation],
    start: usize,
    len: usize,
) -> Result<Vec<ConstRelocation>, String> {
    let end = start
        .checked_add(len)
        .ok_or_else(|| "const union relocation range is not representable".to_string())?;
    let mut selected = Vec::new();
    for relocation in relocations {
        let relocation_end = relocation
            .offset
            .checked_add(relocation.width)
            .ok_or_else(|| "const union relocation range is not representable".to_string())?;
        if relocation.offset >= end || relocation_end <= start {
            continue;
        }
        if relocation.offset < start || relocation_end > end {
            return Err("const union field reads only part of a pointer relocation".to_string());
        }
        let mut relocation = relocation.clone();
        relocation.offset -= start;
        selected.push(relocation);
    }
    Ok(selected)
}

fn validate_relocations(
    relocations: &[ConstRelocation],
    storage_size: usize,
    initialized: &[bool],
) -> Result<(), String> {
    let mut previous_end = 0;
    for relocation in relocations {
        let end = relocation
            .offset
            .checked_add(relocation.width)
            .filter(|end| relocation.width > 0 && *end <= storage_size)
            .ok_or_else(|| "const union relocation exceeds its storage".to_string())?;
        if relocation.offset < previous_end {
            return Err("const union relocations overlap".to_string());
        }
        if initialized[relocation.offset..end].iter().any(|byte| !byte) {
            return Err("const union relocation covers uninitialized storage".to_string());
        }
        previous_end = end;
    }
    Ok(())
}

fn vector_store_len(lane: ConstScalarType, lanes: usize) -> Option<usize> {
    if lane == ConstScalarType::Bool {
        lanes.checked_add(7)?.checked_div(8)
    } else {
        lane.byte_len()?.checked_mul(lanes)
    }
}

fn encode_vector(
    lane: ConstScalarType,
    lanes: usize,
    size: usize,
    values: Vec<ConstValue>,
    endianness: ConstEndianness,
) -> Result<EncodedAbiValue, String> {
    if values.len() != lanes {
        return Err("const union vector field has the wrong lane count".to_string());
    }
    let store_len = vector_store_len(lane, lanes)
        .filter(|store_len| *store_len <= size)
        .ok_or_else(|| "const union vector field exceeds its layout".to_string())?;
    let mut bytes = vec![0; size];
    let mut initialized = vec![false; size];
    if lane == ConstScalarType::Bool {
        for (index, value) in values.into_iter().enumerate() {
            let ConstValue::Bool(value) = value else {
                return Err("const union vector lane does not match its bool type".to_string());
            };
            if value {
                let byte = match endianness {
                    ConstEndianness::Little => index / 8,
                    ConstEndianness::Big => store_len - 1 - index / 8,
                };
                bytes[byte] |= 1 << (index % 8);
            }
        }
        initialized[..store_len].fill(true);
        return Ok(EncodedAbiValue {
            bytes,
            initialized,
            relocations: Vec::new(),
        });
    }
    let lane_len = lane
        .byte_len()
        .ok_or_else(|| "const union vector lane has an invalid scalar width".to_string())?;
    for (index, value) in values.into_iter().enumerate() {
        let encoded = encode_scalar(lane, value, endianness)?;
        let start = index * lane_len;
        bytes[start..start + lane_len].copy_from_slice(&encoded);
        initialized[start..start + lane_len].fill(true);
    }
    Ok(EncodedAbiValue {
        bytes,
        initialized,
        relocations: Vec::new(),
    })
}

fn decode_vector(
    lane: ConstScalarType,
    lanes: usize,
    size: usize,
    bytes: &[u8],
    initialized: &[bool],
    endianness: ConstEndianness,
) -> Result<ConstValue, String> {
    if bytes.len() != size {
        return Err("const union vector field storage has the wrong length".to_string());
    }
    let store_len = vector_store_len(lane, lanes)
        .filter(|store_len| *store_len <= size)
        .ok_or_else(|| "const union vector field exceeds its layout".to_string())?;
    if initialized[..store_len].iter().any(|byte| !byte) {
        return Err("const union vector field reads uninitialized storage".to_string());
    }
    let mut values = Vec::with_capacity(lanes);
    if lane == ConstScalarType::Bool {
        for index in 0..lanes {
            let byte = match endianness {
                ConstEndianness::Little => index / 8,
                ConstEndianness::Big => store_len - 1 - index / 8,
            };
            values.push(ConstValue::Bool(bytes[byte] & (1 << (index % 8)) != 0));
        }
        return Ok(ConstValue::Vector(values));
    }
    let lane_len = lane
        .byte_len()
        .ok_or_else(|| "const union vector lane has an invalid scalar width".to_string())?;
    for index in 0..lanes {
        let start = index * lane_len;
        values.push(decode_scalar(
            lane,
            &bytes[start..start + lane_len],
            endianness,
        )?);
    }
    Ok(ConstValue::Vector(values))
}

fn encode_scalar(
    scalar: ConstScalarType,
    value: ConstValue,
    endianness: ConstEndianness,
) -> Result<Vec<u8>, String> {
    let scalar_len = scalar
        .byte_len()
        .ok_or_else(|| "const union integer field has an invalid scalar width".to_string())?;
    let (raw, len) = match (scalar, value) {
        (ConstScalarType::Integer { bits, signed }, ConstValue::Int(value)) => {
            if !integer_fits(value, bits, signed) {
                return Err("const union integer field value is out of range".to_string());
            }
            (value.bits(), scalar_len)
        }
        (ConstScalarType::Float32, ConstValue::Float(value)) => {
            let value = value as f32;
            if !value.is_finite() {
                return Err("const union f32 field value is out of range".to_string());
            }
            (u128::from(value.to_bits()), 4)
        }
        (ConstScalarType::Float64, ConstValue::Float(value)) if value.is_finite() => {
            (u128::from(value.to_bits()), 8)
        }
        (ConstScalarType::Float64, ConstValue::Float(_)) => {
            return Err("const union f64 field value is out of range".to_string());
        }
        (ConstScalarType::Bool, ConstValue::Bool(value)) => (u128::from(value), 1),
        (ConstScalarType::Char, ConstValue::Int(value))
            if u32::try_from(value.bits())
                .ok()
                .and_then(char::from_u32)
                .is_some() =>
        {
            (value.bits(), 4)
        }
        (ConstScalarType::Char, ConstValue::Int(_)) => {
            return Err("const union char field value is invalid".to_string());
        }
        _ => return Err("const union field value does not match its scalar type".to_string()),
    };
    let bytes = match endianness {
        ConstEndianness::Little => raw.to_le_bytes()[..len].to_vec(),
        ConstEndianness::Big => raw.to_be_bytes()[16 - len..].to_vec(),
    };
    Ok(bytes)
}

fn integer_fits(value: IntConst, bits: u32, signed: bool) -> bool {
    if bits == 0 || bits > 128 {
        return false;
    }
    if signed {
        if value.is_signed() {
            let value = value.as_i128().expect("signed const integer");
            if bits == 128 {
                return true;
            }
            let limit = 1i128 << (bits - 1);
            value >= -limit && value < limit
        } else if bits == 128 {
            value.bits() <= i128::MAX as u128
        } else {
            value.bits() < (1u128 << (bits - 1))
        }
    } else if value.is_signed() && value.as_i128().is_some_and(|value| value < 0) {
        false
    } else if bits == 128 {
        true
    } else {
        value.bits() < (1u128 << bits)
    }
}

fn decode_scalar(
    scalar: ConstScalarType,
    bytes: &[u8],
    endianness: ConstEndianness,
) -> Result<ConstValue, String> {
    let expected_len = scalar
        .byte_len()
        .ok_or_else(|| "const union integer field has an invalid scalar width".to_string())?;
    if bytes.len() != expected_len {
        return Err("const union scalar field storage has the wrong length".to_string());
    }
    let mut raw_bytes = [0u8; 16];
    match endianness {
        ConstEndianness::Little => raw_bytes[..bytes.len()].copy_from_slice(bytes),
        ConstEndianness::Big => raw_bytes[16 - bytes.len()..].copy_from_slice(bytes),
    }
    let raw = match endianness {
        ConstEndianness::Little => u128::from_le_bytes(raw_bytes),
        ConstEndianness::Big => u128::from_be_bytes(raw_bytes),
    };
    match scalar {
        ConstScalarType::Integer { bits, signed } => {
            let value = if signed {
                let shift = 128 - bits;
                IntConst::from_i128(((raw << shift) as i128) >> shift)
            } else {
                IntConst::unsigned(raw)
            };
            Ok(ConstValue::Int(value))
        }
        ConstScalarType::Float32 => Ok(ConstValue::Float(f64::from(f32::from_bits(raw as u32)))),
        ConstScalarType::Float64 => Ok(ConstValue::Float(f64::from_bits(raw as u64))),
        ConstScalarType::Bool => match raw {
            0 => Ok(ConstValue::Bool(false)),
            1 => Ok(ConstValue::Bool(true)),
            _ => Err("const union field has an invalid bool representation".to_string()),
        },
        ConstScalarType::Char => {
            let scalar = u32::try_from(raw)
                .ok()
                .and_then(char::from_u32)
                .ok_or_else(|| {
                    "const union field has an invalid char representation".to_string()
                })?;
            Ok(ConstValue::Int(IntConst::unsigned(scalar as u128)))
        }
    }
}

#[derive(Debug, Clone, PartialEq)]
/// Result of a resolved call, including receiver writeback when required.
pub struct ResolvedConstCallOutput {
    /// Function return value.
    pub value: ConstValue,
    /// Updated mutable receiver value to write back to its original place.
    pub mutable_receiver: Option<ConstValue>,
}

#[derive(Debug, Clone, PartialEq)]
/// Interpreter-owned iterator state and its semantic iterator type.
pub struct ResolvedConstIterator {
    /// Runtime iterator type.
    pub ty: InternedTyId,
    /// Current iterator state value.
    pub value: ConstValue,
}

#[derive(Debug, Clone, PartialEq, Eq)]
/// Resolved writable place rooted at a function-local binding.
pub struct ResolvedConstPlace {
    /// Root local identity.
    pub local_id: LocalId,
    /// Projections from the local to the addressed nested value.
    pub path: Vec<ResolvedConstPlaceElem>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
/// One projection within a resolved writable const place.
pub enum ResolvedConstPlaceElem {
    /// Selects a named aggregate field.
    Field(SymbolId),
    /// Selects an indexed sequence element.
    Index(usize),
}

#[derive(Debug, Clone, PartialEq)]
/// Payload shape of a const enum variant.
pub enum ConstEnumPayload {
    /// Variant without a payload.
    Unit,
    /// Positional variant payload.
    Tuple(Vec<ConstValue>),
    /// Named variant payload.
    Named(BTreeMap<SymbolId, ConstValue>),
}

#[derive(Debug, Clone, PartialEq)]
/// Integer range used by const range expressions and patterns.
pub struct ConstRangeValue {
    /// Inclusive lower bound, or no lower bound.
    pub start: Option<IntConst>,
    /// Upper bound, interpreted according to `inclusive`.
    pub end: Option<IntConst>,
    /// Whether the upper bound is included.
    pub inclusive: bool,
}
