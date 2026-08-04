use nia_ids::{GlobalDefId, InternedTyId, LocalId};
use nia_symbol::SymbolId;
use nia_ty::IntConst;
use std::collections::BTreeMap;

#[derive(Debug, Clone, PartialEq)]
pub enum ConstValue {
    Int(IntConst),
    Float(f64),
    Bool(bool),
    String(String),
    Pointer(Box<ConstValue>),
    Array(Vec<ConstValue>),
    Range(ConstRangeValue),
    Struct(BTreeMap<SymbolId, ConstValue>),
    Union(ConstUnionValue),
    Enum {
        variant: GlobalDefId,
        payload: ConstEnumPayload,
    },
    Optional(Option<Box<ConstValue>>),
    ErrorUnion(Result<Box<ConstValue>, Box<ConstValue>>),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ConstEndianness {
    Little,
    Big,
}

impl ConstEndianness {
    pub fn from_target_name(name: &str) -> Option<Self> {
        match name {
            "little" => Some(Self::Little),
            "big" => Some(Self::Big),
            _ => None,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ConstScalarType {
    Integer { bits: u32, signed: bool },
    Float32,
    Float64,
    Bool,
    Char,
}

impl ConstScalarType {
    pub const fn byte_len(self) -> usize {
        match self {
            Self::Integer { bits, .. } => (bits / 8) as usize,
            Self::Float32 | Self::Char => 4,
            Self::Float64 => 8,
            Self::Bool => 1,
        }
    }
}

#[derive(Debug, Clone, PartialEq)]
pub struct ConstUnionValue {
    fields: BTreeMap<SymbolId, ConstScalarType>,
    bytes: Vec<u8>,
    initialized: Vec<bool>,
    active_field: SymbolId,
    endianness: ConstEndianness,
}

impl ConstUnionValue {
    pub fn new(
        fields: BTreeMap<SymbolId, ConstScalarType>,
        storage_size: usize,
        active_field: SymbolId,
        value: ConstValue,
        endianness: ConstEndianness,
    ) -> Result<Self, String> {
        let mut union = Self {
            fields,
            bytes: vec![0; storage_size],
            initialized: vec![false; storage_size],
            active_field,
            endianness,
        };
        union.write(active_field, value)?;
        Ok(union)
    }

    pub fn read(&self, field: SymbolId) -> Result<ConstValue, String> {
        let scalar = self
            .fields
            .get(&field)
            .copied()
            .ok_or_else(|| "unknown const union field".to_string())?;
        let len = scalar.byte_len();
        if len > self.bytes.len() || self.initialized[..len].iter().any(|byte| !byte) {
            return Err("const union field reads uninitialized storage".to_string());
        }
        decode_scalar(scalar, &self.bytes[..len], self.endianness)
    }

    pub fn write(&mut self, field: SymbolId, value: ConstValue) -> Result<(), String> {
        let scalar = self
            .fields
            .get(&field)
            .copied()
            .ok_or_else(|| "unknown const union field".to_string())?;
        let encoded = encode_scalar(scalar, value, self.endianness)?;
        if encoded.len() > self.bytes.len() {
            return Err("const union field exceeds its storage".to_string());
        }
        self.bytes[..encoded.len()].copy_from_slice(&encoded);
        self.initialized[..encoded.len()].fill(true);
        self.active_field = field;
        Ok(())
    }

    pub fn active_field(&self) -> SymbolId {
        self.active_field
    }

    pub fn fields(&self) -> &BTreeMap<SymbolId, ConstScalarType> {
        &self.fields
    }

    pub fn bytes(&self) -> &[u8] {
        &self.bytes
    }

    pub fn initialized(&self) -> &[bool] {
        &self.initialized
    }
}

fn encode_scalar(
    scalar: ConstScalarType,
    value: ConstValue,
    endianness: ConstEndianness,
) -> Result<Vec<u8>, String> {
    let (raw, len) = match (scalar, value) {
        (ConstScalarType::Integer { bits, signed }, ConstValue::Int(value)) => {
            if !integer_fits(value, bits, signed) {
                return Err("const union integer field value is out of range".to_string());
            }
            (value.bits(), (bits / 8) as usize)
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
pub struct ResolvedConstCallOutput {
    pub value: ConstValue,
    pub mutable_receiver: Option<ConstValue>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct ResolvedConstIterator {
    pub ty: InternedTyId,
    pub value: ConstValue,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ResolvedConstPlace {
    pub local_id: LocalId,
    pub path: Vec<ResolvedConstPlaceElem>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ResolvedConstPlaceElem {
    Field(SymbolId),
    Index(usize),
}

#[derive(Debug, Clone, PartialEq)]
pub enum ConstEnumPayload {
    Unit,
    Tuple(Vec<ConstValue>),
    Named(BTreeMap<SymbolId, ConstValue>),
}

#[derive(Debug, Clone, PartialEq)]
pub struct ConstRangeValue {
    pub start: Option<IntConst>,
    pub end: Option<IntConst>,
    pub inclusive: bool,
}
