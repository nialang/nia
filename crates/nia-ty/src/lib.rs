// SPDX-License-Identifier: GPL-3.0-or-later
use std::collections::HashMap;

pub use nia_ids::{BuiltinTrait, LayoutBuiltin, TraitId};
use nia_ids::{GlobalConstExprId, GlobalDefId, InternedTyId, ModuleId, TyInternerIndex};
use nia_span::Span;

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub enum TyKind {
    Error,
    ComptimeOnly,
    Primitive(PrimitiveTy),
    Pointer {
        is_readonly: bool,
        elem: InternedTyId,
    },
    VolatilePointer {
        is_readonly: bool,
        elem: InternedTyId,
    },
    Slice {
        is_readonly: bool,
        elem: InternedTyId,
    },
    SlicePointee {
        elem: InternedTyId,
    },
    Array {
        len: ArrayLenTy,
        elem: InternedTyId,
    },
    Vector {
        elem: PrimitiveTy,
        lanes: u32,
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
    TraitObjectPointee {
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

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct IntConst {
    bits: u128,
    signed: bool,
}

impl IntConst {
    pub fn from_i128(value: i128) -> Self {
        Self::signed_bits(value as u128)
    }

    pub fn signed_bits(bits: u128) -> Self {
        Self { bits, signed: true }
    }

    pub fn signed(value: i128) -> Self {
        Self::from_i128(value)
    }

    pub fn unsigned(bits: u128) -> Self {
        Self {
            bits,
            signed: false,
        }
    }

    pub fn bits(self) -> u128 {
        self.bits
    }

    pub fn is_signed(self) -> bool {
        self.signed
    }

    pub fn as_i128(self) -> Option<i128> {
        if self.signed {
            Some(self.bits as i128)
        } else {
            i128::try_from(self.bits).ok()
        }
    }

    pub fn cast_to_primitive_int(self, primitive: PrimitiveTy, pointer_width: u32) -> Option<Self> {
        let bits = primitive.integer_bits(pointer_width)?;
        let mask = integer_mask(bits);
        let bits = self.bits & mask;
        if primitive.is_signed_integer() {
            Some(Self::signed_bits(bits))
        } else {
            Some(Self::unsigned(bits))
        }
    }
}

impl From<i128> for IntConst {
    fn from(value: i128) -> Self {
        Self::from_i128(value)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PrimitiveTypeSpelling {
    Scalar(PrimitiveTy),
    Vector { elem: PrimitiveTy, lanes: u32 },
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

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ConstExprSummary {
    pub span: Span,
    pub literal_array_len: Option<u64>,
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
        Some(TyKind::VolatilePointer { is_readonly, elem }) => {
            let elem = import_type_into(target, source, *elem);
            target.intern(TyKind::VolatilePointer {
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
        Some(TyKind::SlicePointee { elem }) => {
            let elem = import_type_into(target, source, *elem);
            target.intern(TyKind::SlicePointee { elem })
        }
        Some(TyKind::Array { len, elem }) => {
            let len = import_array_len_into(target, source, len);
            let elem = import_type_into(target, source, *elem);
            target.intern(TyKind::Array { len, elem })
        }
        Some(TyKind::Vector { elem, lanes }) => target.intern(TyKind::Vector {
            elem: *elem,
            lanes: *lanes,
        }),
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
        Some(TyKind::TraitObjectPointee {
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
            target.intern(TyKind::TraitObjectPointee {
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
            // Layout-builtin lengths carry a type operand; after cross-module copying it must
            // point at the target interner just like ordinary array element types do.
            ty: import_type_into(target, source, *ty),
        },
        ArrayLenTy::Infer | ArrayLenTy::ConstValue(_) | ArrayLenTy::ConstExpr(_) => len.clone(),
    }
}

pub trait TypeEquivalence {
    fn ty_kind_for_equiv(&self, ty: InternedTyId) -> Option<&TyKind>;
    fn same_array_len_for_equiv(&self, left: &ArrayLenTy, right: &ArrayLenTy) -> bool;
    fn same_type_for_equiv(&self, left: InternedTyId, right: InternedTyId) -> bool;

    fn same_type_args_for_equiv(&self, left: &[InternedTyId], right: &[InternedTyId]) -> bool {
        left.len() == right.len()
            && left
                .iter()
                .zip(right)
                .all(|(left, right)| self.same_type_for_equiv(*left, *right))
    }

    fn compute_same_type_for_equiv(&self, left: InternedTyId, right: InternedTyId) -> bool {
        match (self.ty_kind_for_equiv(left), self.ty_kind_for_equiv(right)) {
            (Some(TyKind::Error), Some(TyKind::Error)) => true,
            (Some(TyKind::Primitive(left)), Some(TyKind::Primitive(right))) => left == right,
            (Some(TyKind::GenericParam(left)), Some(TyKind::GenericParam(right))) => left == right,
            (
                Some(TyKind::Pointer {
                    is_readonly: left_const,
                    elem: left_elem,
                }),
                Some(TyKind::Pointer {
                    is_readonly: right_const,
                    elem: right_elem,
                }),
            )
            | (
                Some(TyKind::VolatilePointer {
                    is_readonly: left_const,
                    elem: left_elem,
                }),
                Some(TyKind::VolatilePointer {
                    is_readonly: right_const,
                    elem: right_elem,
                }),
            )
            | (
                Some(TyKind::Slice {
                    is_readonly: left_const,
                    elem: left_elem,
                }),
                Some(TyKind::Slice {
                    is_readonly: right_const,
                    elem: right_elem,
                }),
            ) => left_const == right_const && self.same_type_for_equiv(*left_elem, *right_elem),
            (
                Some(TyKind::SlicePointee { elem: left_elem }),
                Some(TyKind::SlicePointee { elem: right_elem }),
            ) => self.same_type_for_equiv(*left_elem, *right_elem),
            (
                Some(TyKind::Array {
                    len: left_len,
                    elem: left_elem,
                }),
                Some(TyKind::Array {
                    len: right_len,
                    elem: right_elem,
                }),
            ) => {
                self.same_array_len_for_equiv(left_len, right_len)
                    && self.same_type_for_equiv(*left_elem, *right_elem)
            }
            (
                Some(TyKind::Range {
                    kind: left_kind,
                    bound: left_bound,
                }),
                Some(TyKind::Range {
                    kind: right_kind,
                    bound: right_bound,
                }),
            ) => {
                left_kind == right_kind
                    && match (left_bound, right_bound) {
                        (Some(left), Some(right)) => self.same_type_for_equiv(*left, *right),
                        (None, None) => true,
                        _ => false,
                    }
            }
            (
                Some(TyKind::FunctionPointer {
                    params: left_params,
                    return_type: left_return,
                    is_variadic: left_variadic,
                }),
                Some(TyKind::FunctionPointer {
                    params: right_params,
                    return_type: right_return,
                    is_variadic: right_variadic,
                }),
            ) => {
                left_variadic == right_variadic
                    && self.same_type_args_for_equiv(left_params, right_params)
                    && self.same_type_for_equiv(*left_return, *right_return)
            }
            (
                Some(TyKind::Nominal {
                    def_id: left_def,
                    args: left_args,
                }),
                Some(TyKind::Nominal {
                    def_id: right_def,
                    args: right_args,
                }),
            ) => left_def == right_def && self.same_type_args_for_equiv(left_args, right_args),
            (
                Some(TyKind::BuiltinTrait {
                    trait_id: left_trait,
                    args: left_args,
                }),
                Some(TyKind::BuiltinTrait {
                    trait_id: right_trait,
                    args: right_args,
                }),
            ) => left_trait == right_trait && self.same_type_args_for_equiv(left_args, right_args),
            (Some(TyKind::Optional { elem: left }), Some(TyKind::Optional { elem: right })) => {
                self.same_type_for_equiv(*left, *right)
            }
            (
                Some(TyKind::ErrorUnion {
                    error: left_error,
                    value: left_value,
                }),
                Some(TyKind::ErrorUnion {
                    error: right_error,
                    value: right_value,
                }),
            ) => {
                self.same_type_for_equiv(*left_error, *right_error)
                    && self.same_type_for_equiv(*left_value, *right_value)
            }
            (
                Some(TyKind::TraitObject {
                    is_readonly: left_const,
                    trait_id: left_trait,
                    trait_args: left_args,
                    associated_type_bindings: left_bindings,
                }),
                Some(TyKind::TraitObject {
                    is_readonly: right_const,
                    trait_id: right_trait,
                    trait_args: right_args,
                    associated_type_bindings: right_bindings,
                }),
            ) => {
                left_const == right_const
                    && left_trait == right_trait
                    && self.same_type_args_for_equiv(left_args, right_args)
                    && self.same_associated_type_bindings_for_equiv(left_bindings, right_bindings)
            }
            (
                Some(TyKind::TraitObjectPointee {
                    trait_id: left_trait,
                    trait_args: left_args,
                    associated_type_bindings: left_bindings,
                }),
                Some(TyKind::TraitObjectPointee {
                    trait_id: right_trait,
                    trait_args: right_args,
                    associated_type_bindings: right_bindings,
                }),
            ) => {
                left_trait == right_trait
                    && self.same_type_args_for_equiv(left_args, right_args)
                    && self.same_associated_type_bindings_for_equiv(left_bindings, right_bindings)
            }
            (
                Some(TyKind::Projection {
                    self_ty: left_self,
                    trait_id: left_trait,
                    trait_args: left_args,
                    name: left_name,
                }),
                Some(TyKind::Projection {
                    self_ty: right_self,
                    trait_id: right_trait,
                    trait_args: right_args,
                    name: right_name,
                }),
            ) => {
                left_trait == right_trait
                    && left_name == right_name
                    && self.same_type_for_equiv(*left_self, *right_self)
                    && self.same_type_args_for_equiv(left_args, right_args)
            }
            _ => false,
        }
    }

    fn same_associated_type_bindings_for_equiv(
        &self,
        left: &[AssociatedTypeBindingTy],
        right: &[AssociatedTypeBindingTy],
    ) -> bool {
        left.len() == right.len()
            && left.iter().all(|left_binding| {
                right
                    .iter()
                    .find(|right_binding| {
                        self.same_associated_type_binding_key_for_equiv(left_binding, right_binding)
                    })
                    .is_some_and(|right_binding| {
                        self.same_type_for_equiv(left_binding.ty, right_binding.ty)
                    })
            })
    }

    fn same_associated_type_binding_key_for_equiv(
        &self,
        left: &AssociatedTypeBindingTy,
        right: &AssociatedTypeBindingTy,
    ) -> bool {
        left.name == right.name
            && left.trait_id == right.trait_id
            && self.same_type_args_for_equiv(&left.trait_args, &right.trait_args)
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

    pub fn from_name(name: &str) -> Option<Self> {
        Some(match name {
            "i8" => Self::I8,
            "i16" => Self::I16,
            "i32" => Self::I32,
            "i64" => Self::I64,
            "i128" => Self::I128,
            "isize" => Self::Isize,
            "u8" => Self::U8,
            "u16" => Self::U16,
            "u32" => Self::U32,
            "u64" => Self::U64,
            "u128" => Self::U128,
            "usize" => Self::Usize,
            "f32" => Self::F32,
            "f64" => Self::F64,
            "bool" => Self::Bool,
            "char" => Self::Char,
            "void" => Self::Void,
            "!" => Self::Never,
            _ => return None,
        })
    }

    pub fn name(self) -> &'static str {
        match self {
            Self::I8 => "i8",
            Self::I16 => "i16",
            Self::I32 => "i32",
            Self::I64 => "i64",
            Self::I128 => "i128",
            Self::Isize => "isize",
            Self::U8 => "u8",
            Self::U16 => "u16",
            Self::U32 => "u32",
            Self::U64 => "u64",
            Self::U128 => "u128",
            Self::Usize => "usize",
            Self::F32 => "f32",
            Self::F64 => "f64",
            Self::Bool => "bool",
            Self::Char => "char",
            Self::Void => "void",
            Self::Never => "!",
        }
    }

    pub fn vector_element_from_name(name: &str) -> Option<Self> {
        match Self::from_name(name)? {
            primitive if primitive.is_vector_element() => Some(primitive),
            _ => None,
        }
    }

    pub fn is_vector_element(self) -> bool {
        !matches!(self, Self::Char | Self::Void | Self::Never)
    }

    pub fn is_integer(self) -> bool {
        matches!(
            self,
            Self::I8
                | Self::I16
                | Self::I32
                | Self::I64
                | Self::I128
                | Self::Isize
                | Self::U8
                | Self::U16
                | Self::U32
                | Self::U64
                | Self::U128
                | Self::Usize
        )
    }

    pub fn is_signed_integer(self) -> bool {
        matches!(
            self,
            Self::I8 | Self::I16 | Self::I32 | Self::I64 | Self::I128 | Self::Isize
        )
    }

    pub fn integer_bits(self, pointer_width: u32) -> Option<u32> {
        match self {
            Self::I8 | Self::U8 => Some(8),
            Self::I16 | Self::U16 => Some(16),
            Self::I32 | Self::U32 => Some(32),
            Self::I64 | Self::U64 => Some(64),
            Self::I128 | Self::U128 => Some(128),
            Self::Isize | Self::Usize => Some(pointer_width),
            _ => None,
        }
    }

    pub fn is_float(self) -> bool {
        matches!(self, Self::F32 | Self::F64)
    }
}

fn integer_mask(bits: u32) -> u128 {
    if bits >= u128::BITS {
        u128::MAX
    } else {
        (1u128 << bits) - 1
    }
}

impl PrimitiveTypeSpelling {
    pub fn from_name(name: &str) -> Option<Self> {
        if let Some(primitive) = PrimitiveTy::from_name(name) {
            return Some(Self::Scalar(primitive));
        }
        vector_type_spelling(name)
    }
}

fn vector_type_spelling(name: &str) -> Option<PrimitiveTypeSpelling> {
    let (elem, lane_text) = if let Some(rest) = name.strip_prefix("boolx") {
        (PrimitiveTy::Bool, rest)
    } else {
        let split = name.rfind('x')?;
        let elem_text = &name[..split];
        let lane_text = &name[(split + 1)..];
        let elem = PrimitiveTy::vector_element_from_name(elem_text)?;
        if elem == PrimitiveTy::Bool {
            return None;
        }
        (elem, lane_text)
    };
    if lane_text.is_empty() || !lane_text.bytes().all(|byte| byte.is_ascii_digit()) {
        return None;
    }
    let lanes = lane_text.parse::<u32>().ok()?;
    if lanes == 0 {
        return None;
    }
    Some(PrimitiveTypeSpelling::Vector { elem, lanes })
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
    fn primitive_type_spelling_resolves_scalar_and_vector_names() {
        assert_eq!(
            PrimitiveTypeSpelling::from_name("i32"),
            Some(PrimitiveTypeSpelling::Scalar(PrimitiveTy::I32))
        );
        assert_eq!(
            PrimitiveTypeSpelling::from_name("u8x16"),
            Some(PrimitiveTypeSpelling::Vector {
                elem: PrimitiveTy::U8,
                lanes: 16,
            })
        );
        assert_eq!(
            PrimitiveTypeSpelling::from_name("boolx4"),
            Some(PrimitiveTypeSpelling::Vector {
                elem: PrimitiveTy::Bool,
                lanes: 4,
            })
        );

        assert_eq!(PrimitiveTypeSpelling::from_name("boolx"), None);
        assert_eq!(PrimitiveTypeSpelling::from_name("boolx0"), None);
        assert_eq!(PrimitiveTypeSpelling::from_name("charx4"), None);
        assert_eq!(PrimitiveTypeSpelling::from_name("voidx4"), None);
        assert_eq!(PrimitiveTypeSpelling::from_name("!x4"), None);
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
