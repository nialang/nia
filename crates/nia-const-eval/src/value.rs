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
    Optional(Option<Box<ConstValue>>),
    ErrorUnion(Result<Box<ConstValue>, Box<ConstValue>>),
}

#[derive(Debug, Clone, PartialEq)]
pub struct ConstRangeValue {
    pub start: Option<IntConst>,
    pub end: Option<IntConst>,
    pub inclusive: bool,
}
