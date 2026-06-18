use nia_ty::IntConst;
use std::collections::BTreeMap;

#[derive(Debug, Clone, PartialEq)]
pub enum ComptimeValue {
    Int(IntConst),
    Float(f64),
    Bool(bool),
    String(String),
    Pointer(Box<ComptimeValue>),
    Array(Vec<ComptimeValue>),
    Range(ComptimeRangeValue),
    Struct(BTreeMap<String, ComptimeValue>),
    Optional(Option<Box<ComptimeValue>>),
    ErrorUnion(Result<Box<ComptimeValue>, Box<ComptimeValue>>),
}

#[derive(Debug, Clone, PartialEq)]
pub struct ComptimeRangeValue {
    pub start: Option<IntConst>,
    pub end: Option<IntConst>,
    pub inclusive: bool,
}
