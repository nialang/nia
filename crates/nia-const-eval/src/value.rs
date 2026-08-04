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
    Enum {
        variant: GlobalDefId,
        payload: ConstEnumPayload,
    },
    Optional(Option<Box<ConstValue>>),
    ErrorUnion(Result<Box<ConstValue>, Box<ConstValue>>),
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
