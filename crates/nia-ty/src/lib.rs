// SPDX-License-Identifier: GPL-3.0-or-later
use std::collections::HashMap;

pub use nia_ids::{BuiltinTrait, LayoutBuiltin, TraitId};
use nia_ids::{GlobalConstExprId, GlobalDefId, InternedTyId, ModuleId, TyInternerIndex};

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub enum TyKind {
    Error,
    ComptimeOnly,
    Primitive(PrimitiveTy),
    Pointer {
        is_readonly: bool,
        elem: InternedTyId,
    },
    Slice {
        is_readonly: bool,
        elem: InternedTyId,
    },
    Array {
        len: ArrayLenTy,
        elem: InternedTyId,
    },
    Range {
        kind: RangeTyKind,
        bound: Option<InternedTyId>,
    },
    FunctionPointer {
        params: Vec<InternedTyId>,
        return_type: InternedTyId,
        is_variadic: bool,
    },
    Optional {
        elem: InternedTyId,
    },
    ErrorUnion {
        error: InternedTyId,
        value: InternedTyId,
    },
    Nominal {
        def_id: GlobalDefId,
        args: Vec<InternedTyId>,
    },
    BuiltinTrait {
        trait_id: BuiltinTrait,
        args: Vec<InternedTyId>,
    },
    TraitObject {
        is_readonly: bool,
        trait_id: TraitId,
        trait_args: Vec<InternedTyId>,
        associated_type_bindings: Vec<AssociatedTypeBindingTy>,
    },
    Projection {
        self_ty: InternedTyId,
        trait_id: TraitId,
        trait_args: Vec<InternedTyId>,
        name: String,
    },
    GenericParam(String),
}

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct AssociatedTypeBindingTy {
    pub trait_id: Option<TraitId>,
    pub trait_args: Vec<InternedTyId>,
    pub name: String,
    pub ty: InternedTyId,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum RangeTyKind {
    Exclusive,
    Inclusive,
    From,
    To,
    ToInclusive,
    Full,
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
    Builtin {
        builtin: LayoutBuiltin,
        ty: InternedTyId,
    },
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

pub fn import_type_into(
    target: &mut TyInterner,
    source: &TyInterner,
    ty: InternedTyId,
) -> InternedTyId {
    match source.get(ty) {
        Some(TyKind::Error) | None => target.error(),
        Some(TyKind::ComptimeOnly) => target.intern(TyKind::ComptimeOnly),
        Some(TyKind::Primitive(primitive)) => target.primitive(*primitive),
        Some(TyKind::GenericParam(name)) => target.intern(TyKind::GenericParam(name.clone())),
        Some(TyKind::Pointer { is_readonly, elem }) => {
            let elem = import_type_into(target, source, *elem);
            target.intern(TyKind::Pointer {
                is_readonly: *is_readonly,
                elem,
            })
        }
        Some(TyKind::Slice { is_readonly, elem }) => {
            let elem = import_type_into(target, source, *elem);
            target.intern(TyKind::Slice {
                is_readonly: *is_readonly,
                elem,
            })
        }
        Some(TyKind::Array { len, elem }) => {
            let len = import_array_len_into(target, source, len);
            let elem = import_type_into(target, source, *elem);
            target.intern(TyKind::Array { len, elem })
        }
        Some(TyKind::Range { kind, bound }) => {
            let bound = bound.map(|bound| import_type_into(target, source, bound));
            target.intern(TyKind::Range { kind: *kind, bound })
        }
        Some(TyKind::FunctionPointer {
            params,
            return_type,
            is_variadic,
        }) => {
            let params = params
                .iter()
                .map(|param| import_type_into(target, source, *param))
                .collect();
            let return_type = import_type_into(target, source, *return_type);
            target.intern(TyKind::FunctionPointer {
                params,
                return_type,
                is_variadic: *is_variadic,
            })
        }
        Some(TyKind::Optional { elem }) => {
            let elem = import_type_into(target, source, *elem);
            target.intern(TyKind::Optional { elem })
        }
        Some(TyKind::ErrorUnion { error, value }) => {
            let error = import_type_into(target, source, *error);
            let value = import_type_into(target, source, *value);
            target.intern(TyKind::ErrorUnion { error, value })
        }
        Some(TyKind::Nominal { def_id, args }) => {
            let args = args
                .iter()
                .map(|arg| import_type_into(target, source, *arg))
                .collect();
            target.intern(TyKind::Nominal {
                def_id: *def_id,
                args,
            })
        }
        Some(TyKind::BuiltinTrait { trait_id, args }) => {
            let args = args
                .iter()
                .map(|arg| import_type_into(target, source, *arg))
                .collect();
            target.intern(TyKind::BuiltinTrait {
                trait_id: *trait_id,
                args,
            })
        }
        Some(TyKind::TraitObject {
            is_readonly,
            trait_id,
            trait_args,
            associated_type_bindings,
        }) => {
            let trait_args = trait_args
                .iter()
                .map(|arg| import_type_into(target, source, *arg))
                .collect();
            let associated_type_bindings = associated_type_bindings
                .iter()
                .map(|binding| AssociatedTypeBindingTy {
                    trait_id: binding.trait_id,
                    trait_args: binding
                        .trait_args
                        .iter()
                        .map(|arg| import_type_into(target, source, *arg))
                        .collect(),
                    name: binding.name.clone(),
                    ty: import_type_into(target, source, binding.ty),
                })
                .collect();
            target.intern(TyKind::TraitObject {
                is_readonly: *is_readonly,
                trait_id: *trait_id,
                trait_args,
                associated_type_bindings,
            })
        }
        Some(TyKind::Projection {
            self_ty,
            trait_id,
            trait_args,
            name,
        }) => {
            let self_ty = import_type_into(target, source, *self_ty);
            let trait_args = trait_args
                .iter()
                .map(|arg| import_type_into(target, source, *arg))
                .collect();
            target.intern(TyKind::Projection {
                self_ty,
                trait_id: *trait_id,
                trait_args,
                name: name.clone(),
            })
        }
    }
}

fn import_array_len_into(
    target: &mut TyInterner,
    source: &TyInterner,
    len: &ArrayLenTy,
) -> ArrayLenTy {
    match len {
        ArrayLenTy::Builtin { builtin, ty } => ArrayLenTy::Builtin {
            builtin: *builtin,
            // Layout-builtin lengths carry a type operand; after cross-module import it must
            // point at the target interner just like ordinary array element types do.
            ty: import_type_into(target, source, *ty),
        },
        ArrayLenTy::Infer | ArrayLenTy::ConstValue(_) | ArrayLenTy::ConstExpr(_) => len.clone(),
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

    #[test]
    fn import_type_reinterns_layout_builtin_array_length_operand() {
        let mut source = TyInterner::new(ModuleId(0));
        let mut target = TyInterner::new(ModuleId(1));
        let source_i32 = source.primitive(PrimitiveTy::I32);
        let source_array = source.intern(TyKind::Array {
            len: ArrayLenTy::Builtin {
                builtin: LayoutBuiltin::Size,
                ty: source_i32,
            },
            elem: source_i32,
        });

        let imported = import_type_into(&mut target, &source, source_array);

        let Some(TyKind::Array {
            len:
                ArrayLenTy::Builtin {
                    ty: imported_len_ty,
                    ..
                },
            elem,
        }) = target.get(imported)
        else {
            panic!("expected imported array type");
        };
        assert_eq!(imported_len_ty.interner_id, target.interner_id());
        assert_eq!(*imported_len_ty, target.primitive(PrimitiveTy::I32));
        assert_eq!(*elem, target.primitive(PrimitiveTy::I32));
    }
}
