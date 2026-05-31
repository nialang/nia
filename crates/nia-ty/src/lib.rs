// SPDX-License-Identifier: GPL-3.0-or-later
use std::collections::HashMap;

pub use nia_ids::{BuiltinTrait, TraitId};
use nia_ids::{GlobalConstExprId, GlobalDefId, InternedTyId, ModuleId, TyInternerIndex};

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub enum TyKind {
    Error,
    Primitive(PrimitiveTy),
    Pointer {
        is_const: bool,
        elem: InternedTyId,
    },
    Slice {
        is_const: bool,
        elem: InternedTyId,
    },
    Array {
        len: ArrayLenTy,
        elem: InternedTyId,
    },
    FunctionPointer {
        params: Vec<InternedTyId>,
        return_type: InternedTyId,
        is_variadic: bool,
    },
    Nominal {
        def_id: GlobalDefId,
        args: Vec<InternedTyId>,
    },
    BuiltinTrait {
        trait_id: BuiltinTrait,
        args: Vec<InternedTyId>,
    },
    Projection {
        self_ty: InternedTyId,
        trait_id: TraitId,
        trait_args: Vec<InternedTyId>,
        name: String,
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
    ConstValue(u64),
    ConstExpr(GlobalConstExprId),
    Builtin { name: String, ty: InternedTyId },
}

#[derive(Debug, Clone, PartialEq)]
pub struct TyInterner {
    module_id: ModuleId,
    tys: Vec<TyKind>,
    map: HashMap<TyKind, TyInternerIndex>,
    error_ty: TyInternerIndex,
    primitive_tys: HashMap<PrimitiveTy, TyInternerIndex>,
}

impl Default for TyInterner {
    fn default() -> Self {
        Self::new(ModuleId(0))
    }
}

impl TyInterner {
    pub fn new(module_id: ModuleId) -> Self {
        let mut interner = Self {
            module_id,
            tys: Vec::new(),
            map: HashMap::new(),
            error_ty: TyInternerIndex::from_interner_index(0),
            primitive_tys: HashMap::new(),
        };
        let error_ty = interner.intern_local(TyKind::Error);
        interner.error_ty = error_ty;
        for primitive in PrimitiveTy::ALL {
            let ty = interner.intern_local(TyKind::Primitive(primitive));
            interner.primitive_tys.insert(primitive, ty);
        }
        interner
    }

    pub fn interner_id(&self) -> ModuleId {
        self.module_id
    }

    pub fn intern(&mut self, kind: TyKind) -> InternedTyId {
        InternedTyId::new(self.module_id, self.intern_local(kind))
    }

    fn intern_local(&mut self, kind: TyKind) -> TyInternerIndex {
        if let Some(local_id) = self.map.get(&kind) {
            return *local_id;
        }
        let local_id = TyInternerIndex::from_interner_index(self.tys.len() as u32);
        self.tys.push(kind.clone());
        self.map.insert(kind, local_id);
        local_id
    }

    pub fn get(&self, id: InternedTyId) -> Option<&TyKind> {
        if id.interner_id != self.module_id {
            return None;
        }
        self.tys.get(id.index.index() as usize)
    }

    pub fn iter(&self) -> impl Iterator<Item = (InternedTyId, &TyKind)> {
        let module_id = self.module_id;
        self.tys.iter().enumerate().map(move |(index, ty)| {
            (
                InternedTyId::new(
                    module_id,
                    TyInternerIndex::from_interner_index(index as u32),
                ),
                ty,
            )
        })
    }

    pub fn error(&self) -> InternedTyId {
        InternedTyId::new(self.module_id, self.error_ty)
    }

    pub fn primitive(&self, primitive: PrimitiveTy) -> InternedTyId {
        InternedTyId::new(
            self.module_id,
            *self
                .primitive_tys
                .get(&primitive)
                .expect("primitive type must be preinterned"),
        )
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
        let mut interner = TyInterner::new(ModuleId(0));
        let a = interner.intern(TyKind::Primitive(PrimitiveTy::I32));
        let b = interner.intern(TyKind::Primitive(PrimitiveTy::I32));
        assert_eq!(a, b);
        assert_eq!(interner.len(), 19);
    }

    #[test]
    fn primitive_ids_match_preinterned_layout() {
        let interner = TyInterner::new(ModuleId(0));
        for primitive in PrimitiveTy::ALL {
            let id = interner.primitive(primitive);
            assert_eq!(interner.get(id), Some(&TyKind::Primitive(primitive)));
        }
    }
}
