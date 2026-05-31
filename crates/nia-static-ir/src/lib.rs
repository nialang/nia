// SPDX-License-Identifier: GPL-3.0-or-later
use nia_ids::{GlobalDefId, InternedTyId};

#[derive(Debug, Clone, PartialEq)]
pub enum StaticInit {
    Zero,
    Int(i128),
    Float(String),
    Bool(bool),
    Char(u32),
    Byte(u8),
    Chars(Vec<u32>),
    Bytes(Vec<u8>),
    Array(Vec<StaticInit>),
    Repeat {
        value: Box<StaticInit>,
        count: u64,
    },
    Struct(Vec<StaticFieldInit>),
    NullPtr,
    AddrOfGlobal {
        global: GlobalDefId,
        path: Vec<StaticAddressElem>,
    },
    AddrOfFunction {
        function: GlobalDefId,
        args: Vec<InternedTyId>,
    },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum StaticAddressElem {
    Field(GlobalDefId),
    Index(u64),
    Error,
}

#[derive(Debug, Clone, PartialEq)]
pub struct StaticFieldInit {
    pub field: Option<GlobalDefId>,
    pub value: StaticInit,
}
