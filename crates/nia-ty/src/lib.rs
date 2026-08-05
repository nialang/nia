// SPDX-License-Identifier: GPL-3.0-or-later
use nia_hash::FastHashMap;
pub use nia_ids::{BuiltinTrait, BuiltinType, LayoutBuiltin, TraitId};
use nia_ids::{
    GlobalConstExprId, GlobalDefId, InternedTyId, ModuleId, TypeStoreId, TypeStoreIndex,
};
use nia_span::Span;
use nia_symbol::SymbolId;
use std::sync::{Arc, Mutex, OnceLock};

mod substitution;

pub use substitution::substitute_ty;

const TYPE_KIND_ARENA_FANOUT: usize = 256;

type TypeKindLeaf = [OnceLock<Arc<TyKind>>; TYPE_KIND_ARENA_FANOUT];
type TypeKindBranch = [OnceLock<Box<TypeKindLeaf>>; TYPE_KIND_ARENA_FANOUT];
type TypeKindTrunk = [OnceLock<Box<TypeKindBranch>>; TYPE_KIND_ARENA_FANOUT];

fn empty_type_kind_level<T>() -> [OnceLock<T>; TYPE_KIND_ARENA_FANOUT] {
    std::array::from_fn(|_| OnceLock::new())
}

#[derive(Debug)]
struct TypeKindArena {
    roots: [OnceLock<Box<TypeKindTrunk>>; TYPE_KIND_ARENA_FANOUT],
}

impl Default for TypeKindArena {
    fn default() -> Self {
        Self {
            roots: empty_type_kind_level(),
        }
    }
}

impl TypeKindArena {
    fn insert(&self, index: u32, kind: Arc<TyKind>) {
        let [root_index, trunk_index, branch_index, leaf_index] =
            index.to_be_bytes().map(usize::from);
        let trunk = self.roots[root_index].get_or_init(|| Box::new(empty_type_kind_level()));
        let branch = trunk[trunk_index].get_or_init(|| Box::new(empty_type_kind_level()));
        let leaf = branch[branch_index].get_or_init(|| Box::new(empty_type_kind_level()));
        assert!(
            leaf[leaf_index].set(kind).is_ok(),
            "Nia ICE: type store kind slot was published twice"
        );
    }

    fn get(&self, index: u32) -> Option<&TyKind> {
        let [root_index, trunk_index, branch_index, leaf_index] =
            index.to_be_bytes().map(usize::from);
        self.roots[root_index]
            .get()?
            .get(trunk_index)?
            .get()?
            .get(branch_index)?
            .get()?
            .get(leaf_index)?
            .get()
            .map(Arc::as_ref)
    }
}

#[derive(Debug)]
pub struct TypeStore {
    id: TypeStoreId,
    core: Arc<TypeStoreCore>,
}

#[derive(Debug)]
struct TypeStoreCore {
    id: TypeStoreId,
    slots: Mutex<TypeStoreSlots>,
    kinds: TypeKindArena,
}

#[derive(Debug, Default)]
struct TypeStoreSlots {
    canonical: FastHashMap<Arc<TyKind>, InternedTyId>,
}

impl TypeStoreCore {
    fn intern(&self, kind: &TyKind) -> InternedTyId {
        kind.visit_referenced_types(|referenced| {
            assert!(
                self.get(referenced).is_some(),
                "Nia ICE: interned type references a handle outside its session type store"
            );
        });
        let mut slots = self.slots.lock().expect("type store slots lock poisoned");
        if let Some(ty) = slots.canonical.get(kind) {
            return *ty;
        }
        let index = u32::try_from(slots.canonical.len()).expect("type store slot space exhausted");
        let ty = InternedTyId::new(self.id, TypeStoreIndex::from_store_index(index));
        let kind = Arc::new(kind.clone());
        self.kinds.insert(index, Arc::clone(&kind));
        slots.canonical.insert(kind, ty);
        ty
    }

    fn get(&self, ty: InternedTyId) -> Option<&TyKind> {
        if ty.store_id != self.id {
            return None;
        }
        self.kinds.get(ty.index.index())
    }
}

impl PartialEq for TypeStore {
    fn eq(&self, other: &Self) -> bool {
        self.id == other.id
    }
}

impl Eq for TypeStore {}

#[derive(Clone)]
pub struct TypeStoreAppend {
    core: Arc<TypeStoreCore>,
}

impl TypeStoreAppend {
    pub fn intern(&self, kind: TyKind) -> InternedTyId {
        self.core.intern(&kind)
    }

    pub fn error(&self) -> InternedTyId {
        self.intern(TyKind::Error)
    }

    pub fn primitive(&self, primitive: PrimitiveTy) -> InternedTyId {
        self.intern(TyKind::Primitive(primitive))
    }

    pub fn builtin_type(&self, builtin: BuiltinType) -> InternedTyId {
        self.intern(TyKind::BuiltinType(builtin))
    }
}

impl Default for TypeStore {
    fn default() -> Self {
        Self::new()
    }
}

impl TypeStore {
    pub fn new() -> Self {
        let id = TypeStoreId::fresh();
        Self {
            id,
            core: Arc::new(TypeStoreCore {
                id,
                slots: Mutex::new(TypeStoreSlots::default()),
                kinds: TypeKindArena::default(),
            }),
        }
    }

    pub fn id(&self) -> TypeStoreId {
        self.id
    }

    pub fn get(&self, ty: InternedTyId) -> Option<&TyKind> {
        self.core.get(ty)
    }

    #[doc(hidden)]
    pub fn append_for_module(&self, _module_id: ModuleId) -> TypeStoreAppend {
        TypeStoreAppend {
            core: Arc::clone(&self.core),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub enum TyKind {
    Error,
    ConstOnly,
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
        const_args: Vec<ConstGenericArg>,
    },
    BuiltinType(BuiltinType),
    BuiltinTrait {
        trait_id: BuiltinTrait,
        args: Vec<InternedTyId>,
    },
    TraitObject {
        is_readonly: bool,
        trait_id: TraitId,
        trait_args: Vec<InternedTyId>,
        trait_const_args: Vec<ConstGenericArg>,
        associated_type_bindings: Vec<AssociatedTypeBindingTy>,
    },
    TraitObjectPointee {
        trait_id: TraitId,
        trait_args: Vec<InternedTyId>,
        trait_const_args: Vec<ConstGenericArg>,
        associated_type_bindings: Vec<AssociatedTypeBindingTy>,
    },
    Projection {
        self_ty: InternedTyId,
        trait_id: TraitId,
        trait_args: Vec<InternedTyId>,
        trait_const_args: Vec<ConstGenericArg>,
        name: SymbolId,
    },
    GenericParam(SymbolId),
    SelfParam,
}

impl TyKind {
    pub fn visit_referenced_types(&self, mut visit: impl FnMut(InternedTyId)) {
        fn visit_const_args(args: &[ConstGenericArg], visit: &mut impl FnMut(InternedTyId)) {
            for arg in args {
                visit(arg.ty);
            }
        }
        match self {
            Self::Pointer { elem, .. }
            | Self::VolatilePointer { elem, .. }
            | Self::Slice { elem, .. }
            | Self::SlicePointee { elem }
            | Self::Optional { elem } => visit(*elem),
            Self::Array { len, elem } => {
                if let ArrayLenTy::Builtin { ty, .. } = len {
                    visit(*ty);
                }
                visit(*elem);
            }
            Self::Range { bound, .. } => {
                if let Some(bound) = bound {
                    visit(*bound);
                }
            }
            Self::FunctionPointer {
                params,
                return_type,
                ..
            } => {
                for param in params {
                    visit(*param);
                }
                visit(*return_type);
            }
            Self::ErrorUnion { error, value } => {
                visit(*error);
                visit(*value);
            }
            Self::Nominal {
                args, const_args, ..
            } => {
                for arg in args {
                    visit(*arg);
                }
                visit_const_args(const_args, &mut visit);
            }
            Self::BuiltinTrait { args, .. } => {
                for arg in args {
                    visit(*arg);
                }
            }
            Self::TraitObject {
                trait_args,
                trait_const_args,
                associated_type_bindings,
                ..
            }
            | Self::TraitObjectPointee {
                trait_args,
                trait_const_args,
                associated_type_bindings,
                ..
            } => {
                for arg in trait_args {
                    visit(*arg);
                }
                visit_const_args(trait_const_args, &mut visit);
                for binding in associated_type_bindings {
                    for arg in &binding.trait_args {
                        visit(*arg);
                    }
                    visit_const_args(&binding.trait_const_args, &mut visit);
                    visit(binding.ty);
                }
            }
            Self::Projection {
                self_ty,
                trait_args,
                trait_const_args,
                ..
            } => {
                visit(*self_ty);
                for arg in trait_args {
                    visit(*arg);
                }
                visit_const_args(trait_const_args, &mut visit);
            }
            Self::Error
            | Self::ConstOnly
            | Self::Primitive(_)
            | Self::Vector { .. }
            | Self::BuiltinType(_)
            | Self::GenericParam(_)
            | Self::SelfParam => {}
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct AssociatedTypeBindingTy {
    pub trait_id: Option<TraitId>,
    pub trait_args: Vec<InternedTyId>,
    pub trait_const_args: Vec<ConstGenericArg>,
    pub name: SymbolId,
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

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
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

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct ConstGenericArg {
    pub ty: InternedTyId,
    pub value: ConstGenericValue,
}

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub enum ConstGenericValue {
    GenericParam(SymbolId),
    ConstExpr(GlobalConstExprId),
    Int(IntConst),
    Bool(bool),
    Char(char),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PrimitiveTypeSpelling {
    Scalar(PrimitiveTy),
    Vector { elem: PrimitiveTy, lanes: u32 },
}

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub enum ArrayLenTy {
    Infer,
    GenericParam(SymbolId),
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

    fn same_const_generic_args_for_equiv(
        &self,
        left: &[ConstGenericArg],
        right: &[ConstGenericArg],
    ) -> bool {
        left.len() == right.len()
            && left.iter().zip(right).all(|(left, right)| {
                self.same_type_for_equiv(left.ty, right.ty) && left.value == right.value
            })
    }

    fn compute_same_type_for_equiv(&self, left: InternedTyId, right: InternedTyId) -> bool {
        match (self.ty_kind_for_equiv(left), self.ty_kind_for_equiv(right)) {
            (Some(TyKind::Error), Some(TyKind::Error)) => true,
            (Some(TyKind::Primitive(left)), Some(TyKind::Primitive(right))) => left == right,
            (Some(TyKind::GenericParam(left)), Some(TyKind::GenericParam(right))) => left == right,
            (Some(TyKind::SelfParam), Some(TyKind::SelfParam)) => true,
            (Some(TyKind::BuiltinType(left)), Some(TyKind::BuiltinType(right))) => left == right,
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
                    const_args: left_const_args,
                }),
                Some(TyKind::Nominal {
                    def_id: right_def,
                    args: right_args,
                    const_args: right_const_args,
                }),
            ) => {
                left_def == right_def
                    && self.same_type_args_for_equiv(left_args, right_args)
                    && self.same_const_generic_args_for_equiv(left_const_args, right_const_args)
            }
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
                    trait_const_args: left_const_args,
                    associated_type_bindings: left_bindings,
                }),
                Some(TyKind::TraitObject {
                    is_readonly: right_const,
                    trait_id: right_trait,
                    trait_args: right_args,
                    trait_const_args: right_const_args,
                    associated_type_bindings: right_bindings,
                }),
            ) => {
                left_const == right_const
                    && left_trait == right_trait
                    && self.same_type_args_for_equiv(left_args, right_args)
                    && self.same_const_generic_args_for_equiv(left_const_args, right_const_args)
                    && self.same_associated_type_bindings_for_equiv(left_bindings, right_bindings)
            }
            (
                Some(TyKind::TraitObjectPointee {
                    trait_id: left_trait,
                    trait_args: left_args,
                    trait_const_args: left_const_args,
                    associated_type_bindings: left_bindings,
                }),
                Some(TyKind::TraitObjectPointee {
                    trait_id: right_trait,
                    trait_args: right_args,
                    trait_const_args: right_const_args,
                    associated_type_bindings: right_bindings,
                }),
            ) => {
                left_trait == right_trait
                    && self.same_type_args_for_equiv(left_args, right_args)
                    && self.same_const_generic_args_for_equiv(left_const_args, right_const_args)
                    && self.same_associated_type_bindings_for_equiv(left_bindings, right_bindings)
            }
            (
                Some(TyKind::Projection {
                    self_ty: left_self,
                    trait_id: left_trait,
                    trait_args: left_args,
                    trait_const_args: left_const_args,
                    name: left_name,
                }),
                Some(TyKind::Projection {
                    self_ty: right_self,
                    trait_id: right_trait,
                    trait_args: right_args,
                    trait_const_args: right_const_args,
                    name: right_name,
                }),
            ) => {
                left_trait == right_trait
                    && left_name == right_name
                    && self.same_type_for_equiv(*left_self, *right_self)
                    && self.same_type_args_for_equiv(left_args, right_args)
                    && self.same_const_generic_args_for_equiv(left_const_args, right_const_args)
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
            && self
                .same_const_generic_args_for_equiv(&left.trait_const_args, &right.trait_const_args)
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

    pub fn from_known_symbol(name: SymbolId) -> Option<Self> {
        Some(match name {
            nia_symbol::known::I8 => Self::I8,
            nia_symbol::known::I16 => Self::I16,
            nia_symbol::known::I32 => Self::I32,
            nia_symbol::known::I64 => Self::I64,
            nia_symbol::known::I128 => Self::I128,
            nia_symbol::known::ISIZE => Self::Isize,
            nia_symbol::known::U8 => Self::U8,
            nia_symbol::known::U16 => Self::U16,
            nia_symbol::known::U32 => Self::U32,
            nia_symbol::known::U64 => Self::U64,
            nia_symbol::known::U128 => Self::U128,
            nia_symbol::known::USIZE => Self::Usize,
            nia_symbol::known::F32 => Self::F32,
            nia_symbol::known::F64 => Self::F64,
            nia_symbol::known::BOOL => Self::Bool,
            nia_symbol::known::CHAR => Self::Char,
            nia_symbol::known::VOID => Self::Void,
            nia_symbol::known::NEVER => Self::Never,
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
    fn type_kind_arena_resolves_sparse_u32_slot_boundaries() {
        let arena = TypeKindArena::default();
        let populated = [0, 255, 256, 65_535, 65_536, u32::MAX];

        for index in populated {
            arena.insert(index, Arc::new(TyKind::Error));
        }

        for index in populated {
            assert_eq!(arena.get(index), Some(&TyKind::Error));
        }
        for index in [1, 254, 257, 65_534, 65_537, u32::MAX - 1] {
            assert_eq!(arena.get(index), None);
        }
    }

    #[test]
    fn interns_identical_types_once() {
        let store = TypeStore::new();
        let module_id = nia_ids::ModuleIdAllocator::new().allocate();
        let append = store.append_for_module(module_id);
        let first = append.primitive(PrimitiveTy::I32);
        let second = append.primitive(PrimitiveTy::I32);

        assert_eq!(first, second);
        assert_eq!(store.get(first), Some(&TyKind::Primitive(PrimitiveTy::I32)));
    }

    #[test]
    fn primitive_ids_resolve_to_canonical_kinds() {
        let store = TypeStore::new();
        let module_id = nia_ids::ModuleIdAllocator::new().allocate();
        let append = store.append_for_module(module_id);

        for primitive in PrimitiveTy::ALL {
            let id = append.primitive(primitive);
            assert_eq!(store.get(id), Some(&TyKind::Primitive(primitive)));
        }
    }

    #[test]
    fn type_store_identity_rejects_foreign_session_handles() {
        let first = TypeStore::new();
        let second = TypeStore::new();
        let module_id = nia_ids::ModuleIdAllocator::new().allocate();
        let first_i32 = first
            .append_for_module(module_id)
            .primitive(PrimitiveTy::I32);
        let second_i32 = second
            .append_for_module(module_id)
            .primitive(PrimitiveTy::I32);

        assert_ne!(first.id(), second.id());
        assert_ne!(first_i32, second_i32);
        assert_eq!(
            first.get(first_i32),
            Some(&TyKind::Primitive(PrimitiveTy::I32))
        );
        assert_eq!(first.get(second_i32), None);
        assert_eq!(second.get(first_i32), None);
        assert_eq!(
            second.get(second_i32),
            Some(&TyKind::Primitive(PrimitiveTy::I32))
        );
    }

    #[test]
    fn module_append_capabilities_share_canonical_ids() {
        let store = TypeStore::new();
        let mut module_ids = nia_ids::ModuleIdAllocator::new();
        let first = store.append_for_module(module_ids.allocate());
        let second = store.append_for_module(module_ids.allocate());
        let first_elem = first.primitive(PrimitiveTy::U32);
        let second_elem = second.primitive(PrimitiveTy::U32);
        let first_pointer = first.intern(TyKind::Pointer {
            is_readonly: true,
            elem: first_elem,
        });
        let second_pointer = second.intern(TyKind::Pointer {
            is_readonly: true,
            elem: second_elem,
        });

        assert_eq!(first_elem, second_elem);
        assert_eq!(first_pointer, second_pointer);
    }

    #[test]
    #[should_panic(expected = "outside its session type store")]
    fn interning_rejects_foreign_session_type_dependencies() {
        let local = TypeStore::new();
        let foreign = TypeStore::new();
        let mut module_ids = nia_ids::ModuleIdAllocator::new();
        let foreign_ty = foreign
            .append_for_module(module_ids.allocate())
            .primitive(PrimitiveTy::U32);

        local
            .append_for_module(module_ids.allocate())
            .intern(TyKind::Pointer {
                is_readonly: true,
                elem: foreign_ty,
            });
    }

    #[test]
    fn interning_accepts_same_session_dependencies_from_another_module() {
        let store = TypeStore::new();
        let mut module_ids = nia_ids::ModuleIdAllocator::new();
        let foreign_module_id = module_ids.allocate();
        let local_module_id = module_ids.allocate();
        let foreign = store
            .append_for_module(foreign_module_id)
            .intern(TyKind::Nominal {
                def_id: GlobalDefId {
                    module_id: foreign_module_id,
                    def_id: nia_ids::DefId(1),
                },
                args: Vec::new(),
                const_args: Vec::new(),
            });
        let pointer = store
            .append_for_module(local_module_id)
            .intern(TyKind::Pointer {
                is_readonly: true,
                elem: foreign,
            });

        assert_eq!(
            store.get(pointer),
            Some(&TyKind::Pointer {
                is_readonly: true,
                elem: foreign,
            })
        );
    }

    #[test]
    fn session_type_handle_is_word_sized() {
        assert_eq!(
            std::mem::size_of::<InternedTyId>(),
            std::mem::size_of::<u64>()
        );
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
        assert_eq!(
            PrimitiveTy::from_known_symbol(nia_symbol::known::USIZE),
            Some(PrimitiveTy::Usize)
        );
        assert_eq!(PrimitiveTy::from_known_symbol(nia_symbol::known::LEN), None);
    }
}
