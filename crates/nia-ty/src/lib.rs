// SPDX-License-Identifier: GPL-3.0-or-later
use std::collections::HashMap;

use nia_ids::{GlobalDefId, TyId};

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub enum TyKind {
    Error,
    Primitive(PrimitiveTy),
    Pointer {
        is_const: bool,
        elem: TyId,
    },
    Slice {
        is_const: bool,
        elem: TyId,
    },
    Array {
        len: ArrayLenTy,
        elem: TyId,
    },
    FunctionPointer {
        params: Vec<TyId>,
        return_type: TyId,
        is_variadic: bool,
    },
    Nominal {
        def_id: GlobalDefId,
        args: Vec<TyId>,
    },
    GenericParam(String),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum PrimitiveTy {
    I8,
    I16,
    I32,
    I64,
    I128,
    Isize,
    U8,
    U16,
    U32,
    U64,
    U128,
    Usize,
    F32,
    F64,
    Bool,
    Char,
    Void,
    Never,
}

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub enum ArrayLenTy {
    Infer,
    ConstExpr(String),
    Builtin { name: String, ty: TyId },
}

#[derive(Debug, Clone, PartialEq)]
pub struct TyInterner {
    tys: Vec<TyKind>,
    map: HashMap<TyKind, TyId>,
}

impl Default for TyInterner {
    fn default() -> Self {
        let mut interner = Self {
            tys: Vec::new(),
            map: HashMap::new(),
        };
        interner.intern(TyKind::Error);
        for primitive in PrimitiveTy::ALL {
            interner.intern(TyKind::Primitive(primitive));
        }
        interner
    }
}

impl TyInterner {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn intern(&mut self, kind: TyKind) -> TyId {
        if let Some(id) = self.map.get(&kind) {
            return *id;
        }
        let id = TyId(self.tys.len() as u32);
        self.tys.push(kind.clone());
        self.map.insert(kind, id);
        id
    }

    pub fn get(&self, id: TyId) -> Option<&TyKind> {
        self.tys.get(id.0 as usize)
    }

    pub fn iter(&self) -> impl Iterator<Item = (TyId, &TyKind)> {
        self.tys
            .iter()
            .enumerate()
            .map(|(index, ty)| (TyId(index as u32), ty))
    }

    pub fn error(&self) -> TyId {
        TyId(0)
    }

    pub fn primitive(&self, primitive: PrimitiveTy) -> TyId {
        self.map
            .get(&TyKind::Primitive(primitive))
            .copied()
            .expect("primitive types are pre-interned")
    }

    pub fn len(&self) -> usize {
        self.tys.len()
    }

    pub fn is_empty(&self) -> bool {
        self.tys.is_empty()
    }
}

impl PrimitiveTy {
    pub const ALL: [Self; 18] = [
        Self::I8,
        Self::I16,
        Self::I32,
        Self::I64,
        Self::I128,
        Self::Isize,
        Self::U8,
        Self::U16,
        Self::U32,
        Self::U64,
        Self::U128,
        Self::Usize,
        Self::F32,
        Self::F64,
        Self::Bool,
        Self::Char,
        Self::Void,
        Self::Never,
    ];
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn interns_identical_types_once() {
        let mut interner = TyInterner::new();
        let a = interner.intern(TyKind::Primitive(PrimitiveTy::I32));
        let b = interner.intern(TyKind::Primitive(PrimitiveTy::I32));
        assert_eq!(a, b);
        assert_eq!(interner.len(), 19);
    }
}
