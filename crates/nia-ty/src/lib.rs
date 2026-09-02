// SPDX-License-Identifier: GPL-3.0-or-later
//! Canonical type-store identities and structural type equivalence.
use nia_hash::FastHashMap;
pub use nia_ids::{BuiltinTrait, BuiltinType, LayoutBuiltin, TraitId};
use nia_ids::{
    ClosureId, GlobalConstExprId, GlobalDefId, InternedTyId, ModuleId, TypeStoreId, TypeStoreIndex,
};
use nia_span::Span;
use nia_symbol::SymbolId;
use std::sync::{Arc, Mutex, OnceLock};

mod substitution;

pub use substitution::{array_len_from_const_arg, substitute_ty, substitute_ty_with_closures};

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
/// Session-owned arena for canonical semantic types.
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
/// Module-scoped capability for appending types to a shared store.
pub struct TypeStoreAppend {
    core: Arc<TypeStoreCore>,
}

impl TypeStoreAppend {
    /// Interns a type, canonicalizing callable pointers into callable views.
    pub fn intern(&self, kind: TyKind) -> InternedTyId {
        if let TyKind::Pointer { is_readonly, elem } = &kind
            && let Some(TyKind::CallablePointee {
                params,
                return_type,
            }) = self.core.get(*elem)
        {
            return self.core.intern(&TyKind::Callable {
                is_readonly: *is_readonly,
                params: params.clone(),
                return_type: *return_type,
            });
        }
        self.core.intern(&kind)
    }

    /// Returns the canonical error type.
    pub fn error(&self) -> InternedTyId {
        self.intern(TyKind::Error)
    }

    /// Interns a primitive type.
    pub fn primitive(&self, primitive: PrimitiveTy) -> InternedTyId {
        self.intern(TyKind::Primitive(primitive))
    }

    /// Interns a builtin nominal type.
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
    /// Creates an empty type store with a fresh session identity.
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

    /// Returns this store's session identity.
    pub fn id(&self) -> TypeStoreId {
        self.id
    }

    /// Looks up a type handle, rejecting handles from another store.
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
/// Structural representation of every semantic type form.
pub enum TyKind {
    /// Error recovery type.
    Error,
    /// Compile-time-only value type.
    ConstOnly,
    /// Opaque type whose representation is intentionally unavailable.
    Opaque,
    /// Primitive scalar type.
    Primitive(PrimitiveTy),
    /// Ordered tuple elements.
    Tuple(Vec<InternedTyId>),
    /// Read-only or mutable pointer to an element type.
    Pointer {
        /// Whether writes through the pointer are forbidden.
        is_readonly: bool,
        /// Pointed-to type.
        elem: InternedTyId,
    },
    /// Pointer with volatile access semantics.
    VolatilePointer {
        /// Whether writes through the pointer are forbidden.
        is_readonly: bool,
        /// Pointed-to type.
        elem: InternedTyId,
    },
    /// Fat slice value carrying pointer and length.
    Slice {
        /// Whether the slice is read-only.
        is_readonly: bool,
        /// Element type.
        elem: InternedTyId,
    },
    /// Unsized slice pointee used behind a pointer.
    SlicePointee {
        /// Element type.
        elem: InternedTyId,
    },
    /// Fixed-length array.
    Array {
        /// Array length expression.
        len: ArrayLenTy,
        /// Element type.
        elem: InternedTyId,
    },
    /// Fixed-width SIMD vector.
    Vector {
        /// Scalar lane type.
        elem: PrimitiveTy,
        /// Number of lanes.
        lanes: u32,
    },
    /// Range value with an optional bound type.
    Range {
        /// Inclusive/exclusive bound shape.
        kind: RangeTyKind,
        /// Element bound type, when present.
        bound: Option<InternedTyId>,
    },
    /// Thin C-compatible function pointer.
    FunctionPointer {
        /// Parameter types.
        params: Vec<InternedTyId>,
        /// Return type.
        return_type: InternedTyId,
        /// Whether the function accepts variadic arguments.
        is_variadic: bool,
    },
    /// Callable closure view.
    Callable {
        /// Whether the callable is read-only.
        is_readonly: bool,
        /// Parameter types.
        params: Vec<InternedTyId>,
        /// Return type.
        return_type: InternedTyId,
    },
    /// Unsized callable state pointee.
    CallablePointee {
        /// Parameter types.
        params: Vec<InternedTyId>,
        /// Return type.
        return_type: InternedTyId,
    },
    /// Concrete closure state and its captured types.
    ClosureState {
        /// Stable closure identity.
        closure_id: ClosureId,
        /// Captured value types.
        captures: Vec<InternedTyId>,
        /// Parameter types.
        params: Vec<InternedTyId>,
        /// Return type.
        return_type: InternedTyId,
    },
    /// Optional value wrapper.
    Optional {
        /// Wrapped type.
        elem: InternedTyId,
    },
    /// Error/value union wrapper.
    ErrorUnion {
        /// Error payload type.
        error: InternedTyId,
        /// Success payload type.
        value: InternedTyId,
    },
    /// Nominal definition with type and const arguments.
    Nominal {
        /// Defining item identity.
        def_id: GlobalDefId,
        /// Type arguments in declaration order.
        args: Vec<InternedTyId>,
        /// Const arguments in declaration order.
        const_args: Vec<ConstGenericArg>,
    },
    /// Builtin nominal type supplied by the language runtime.
    BuiltinType(BuiltinType),
    /// Builtin trait application.
    BuiltinTrait {
        /// Builtin trait identity.
        trait_id: BuiltinTrait,
        /// Trait type arguments.
        args: Vec<InternedTyId>,
    },
    /// Sized trait-object value.
    TraitObject {
        /// Whether the object is read-only.
        is_readonly: bool,
        /// Source trait identity.
        trait_id: TraitId,
        /// Trait type arguments.
        trait_args: Vec<InternedTyId>,
        /// Trait const arguments.
        trait_const_args: Vec<ConstGenericArg>,
        /// Associated type bindings carried by the object.
        associated_type_bindings: Vec<AssociatedTypeBindingTy>,
    },
    /// Unsized trait-object pointee.
    TraitObjectPointee {
        /// Source trait identity.
        trait_id: TraitId,
        /// Trait type arguments.
        trait_args: Vec<InternedTyId>,
        /// Trait const arguments.
        trait_const_args: Vec<ConstGenericArg>,
        /// Associated type bindings carried by the object.
        associated_type_bindings: Vec<AssociatedTypeBindingTy>,
    },
    /// Associated type projection before normalization.
    Projection {
        /// Projection receiver type.
        self_ty: InternedTyId,
        /// Source trait identity.
        trait_id: TraitId,
        /// Trait type arguments.
        trait_args: Vec<InternedTyId>,
        /// Trait const arguments.
        trait_const_args: Vec<ConstGenericArg>,
        /// Associated type name.
        name: SymbolId,
    },
    /// Unresolved type generic parameter.
    GenericParam(SymbolId),
    /// Unresolved `Self` parameter.
    SelfParam,
}

impl TyKind {
    /// Returns whether this is the zero-element tuple/unit type.
    pub fn is_unit(&self) -> bool {
        matches!(self, Self::Tuple(elems) if elems.is_empty())
    }

    /// Visits every type handle nested in this structural type.
    pub fn visit_referenced_types(&self, mut visit: impl FnMut(InternedTyId)) {
        fn visit_const_args(args: &[ConstGenericArg], visit: &mut impl FnMut(InternedTyId)) {
            for arg in args {
                visit(arg.ty);
            }
        }
        match self {
            Self::Tuple(elems) => {
                for elem in elems {
                    visit(*elem);
                }
            }
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
            Self::Callable {
                params,
                return_type,
                ..
            }
            | Self::CallablePointee {
                params,
                return_type,
            } => {
                for param in params {
                    visit(*param);
                }
                visit(*return_type);
            }
            Self::ClosureState {
                captures,
                params,
                return_type,
                ..
            } => {
                for capture in captures {
                    visit(*capture);
                }
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
            | Self::Opaque
            | Self::Primitive(_)
            | Self::Vector { .. }
            | Self::BuiltinType(_)
            | Self::GenericParam(_)
            | Self::SelfParam => {}
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
/// Associated type binding carried by a trait object or projection.
pub struct AssociatedTypeBindingTy {
    /// Optional source trait identity for the binding key.
    pub trait_id: Option<TraitId>,
    /// Type arguments of the source trait.
    pub trait_args: Vec<InternedTyId>,
    /// Const arguments of the source trait.
    pub trait_const_args: Vec<ConstGenericArg>,
    /// Associated member name.
    pub name: SymbolId,
    /// Resolved associated type.
    pub ty: InternedTyId,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
/// Shape of bounds represented by a range type.
pub enum RangeTyKind {
    /// Both bounds present and end-exclusive.
    Exclusive,
    /// Both bounds present and end-inclusive.
    Inclusive,
    /// Only a start bound is present.
    From,
    /// Only an end bound is present and exclusive.
    To,
    /// Only an end bound is present and inclusive.
    ToInclusive,
    /// No bounds are present.
    Full,
}

impl RangeTyKind {
    /// Returns whether this range carries a start bound.
    pub const fn has_start_bound(self) -> bool {
        matches!(self, Self::Exclusive | Self::Inclusive | Self::From)
    }

    /// Returns whether this range carries an end bound.
    pub const fn has_end_bound(self) -> bool {
        matches!(
            self,
            Self::Exclusive | Self::Inclusive | Self::To | Self::ToInclusive
        )
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
/// Primitive scalar types understood by the language.
pub enum PrimitiveTy {
    /// Signed 8-bit integer.
    I8,
    /// Signed 16-bit integer.
    I16,
    /// Signed 32-bit integer.
    I32,
    /// Signed 64-bit integer.
    I64,
    /// Signed 128-bit integer.
    I128,
    /// Signed pointer-width integer.
    Isize,
    /// Unsigned 8-bit integer.
    U8,
    /// Unsigned 16-bit integer.
    U16,
    /// Unsigned 32-bit integer.
    U32,
    /// Unsigned 64-bit integer.
    U64,
    /// Unsigned 128-bit integer.
    U128,
    /// Unsigned pointer-width integer.
    Usize,
    /// 32-bit floating-point value.
    F32,
    /// 64-bit floating-point value.
    F64,
    /// Boolean value.
    Bool,
    /// Unicode scalar value.
    Char,
    /// Non-returning type.
    Never,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
/// Integer bits plus signedness, independent of a concrete primitive width.
pub struct IntConst {
    bits: u128,
    signed: bool,
}

impl IntConst {
    /// Constructs a signed value from an `i128`.
    pub fn from_i128(value: i128) -> Self {
        Self::signed_bits(value as u128)
    }

    /// Constructs a signed value from its raw two's-complement bits.
    pub fn signed_bits(bits: u128) -> Self {
        Self { bits, signed: true }
    }

    /// Constructs a signed integer constant.
    pub fn signed(value: i128) -> Self {
        Self::from_i128(value)
    }

    /// Constructs an unsigned integer constant.
    pub fn unsigned(bits: u128) -> Self {
        Self {
            bits,
            signed: false,
        }
    }

    /// Returns the raw integer bits.
    pub fn bits(self) -> u128 {
        self.bits
    }

    /// Returns whether the value is signed.
    pub fn is_signed(self) -> bool {
        self.signed
    }

    /// Converts to `i128` when the unsigned value fits.
    pub fn as_i128(self) -> Option<i128> {
        if self.signed {
            Some(self.bits as i128)
        } else {
            i128::try_from(self.bits).ok()
        }
    }

    /// Reports whether this value is representable by a primitive integer.
    pub fn fits_primitive_int(self, primitive: PrimitiveTy, pointer_width: u32) -> bool {
        let Some(bits) = primitive.integer_bits(pointer_width) else {
            return false;
        };
        if !(1..=u128::BITS).contains(&bits) {
            return false;
        }
        if primitive.is_signed_integer() {
            let Some(value) = self.as_i128() else {
                return false;
            };
            return bits == i128::BITS
                || (-(1_i128 << (bits - 1))..=(1_i128 << (bits - 1)) - 1).contains(&value);
        }
        let value = if self.is_signed() {
            self.as_i128().and_then(|value| u128::try_from(value).ok())
        } else {
            Some(self.bits())
        };
        value.is_some_and(|value| bits == u128::BITS || value < (1_u128 << bits))
    }

    /// Casts the value to a primitive integer width, preserving low bits.
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
/// Typed argument supplied to a const-generic parameter.
pub struct ConstGenericArg {
    /// Declared type of the argument.
    pub ty: InternedTyId,
    /// Semantic argument value or unresolved identity.
    pub value: ConstGenericValue,
}

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
/// Value identity of a const-generic argument.
pub enum ConstGenericValue {
    /// Unsubstituted const parameter.
    GenericParam(SymbolId),
    /// Unevaluated global const expression.
    ConstExpr(GlobalConstExprId),
    /// Integer value with explicit signedness.
    Int(IntConst),
    /// Boolean value.
    Bool(bool),
    /// Unicode scalar value.
    Char(char),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
/// Parsed primitive spelling, including fixed-width SIMD forms.
pub enum PrimitiveTypeSpelling {
    /// Scalar primitive spelling.
    Scalar(PrimitiveTy),
    /// Vector primitive spelling.
    Vector {
        /// Scalar lane type.
        elem: PrimitiveTy,
        /// Number of vector lanes.
        lanes: u32,
    },
}

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
/// Semantic form of a fixed array length.
pub enum ArrayLenTy {
    /// Length awaiting contextual inference.
    Infer,
    /// Unsubstituted const parameter.
    GenericParam(SymbolId),
    /// Evaluated non-negative length.
    ConstValue(u64),
    /// Unevaluated global const expression.
    ConstExpr(GlobalConstExprId),
    /// Target-dependent layout query used as a length.
    Builtin {
        /// Layout operation.
        builtin: LayoutBuiltin,
        /// Operand type.
        ty: InternedTyId,
    },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
/// Stable facts retained for an unevaluated const expression.
pub struct ConstExprSummary {
    /// Source location used for diagnostics.
    pub span: Span,
    /// Literal array length when it can be recovered without evaluation.
    pub literal_array_len: Option<u64>,
}

/// Context-dependent structural equivalence for interned types.
pub trait TypeEquivalence {
    /// Provides the type-store view used by [`Self::compute_same_type_for_equiv`].
    /// Implementors may compare handles from different stores, so structural
    /// variants must not rely on `InternedTyId` equality alone.
    fn ty_kind_for_equiv(&self, ty: InternedTyId) -> Option<&TyKind>;

    /// Compares array lengths, including evaluator-specific const-expression
    /// summaries that are unavailable to the shared type layer.
    fn same_array_len_for_equiv(&self, left: &ArrayLenTy, right: &ArrayLenTy) -> bool;

    /// Returns semantic type equivalence for two handles in the implementor's
    /// source/type context.
    fn same_type_for_equiv(&self, left: InternedTyId, right: InternedTyId) -> bool;

    /// Compares two ordered type argument lists structurally.
    fn same_type_args_for_equiv(&self, left: &[InternedTyId], right: &[InternedTyId]) -> bool {
        left.len() == right.len()
            && left
                .iter()
                .zip(right)
                .all(|(left, right)| self.same_type_for_equiv(*left, *right))
    }

    /// Compares typed const arguments using semantic integer bits.
    fn same_const_generic_args_for_equiv(
        &self,
        left: &[ConstGenericArg],
        right: &[ConstGenericArg],
    ) -> bool {
        left.len() == right.len()
            && left.iter().zip(right).all(|(left, right)| {
                self.same_type_for_equiv(left.ty, right.ty)
                    && match (&left.value, &right.value) {
                        (ConstGenericValue::Int(left), ConstGenericValue::Int(right)) => {
                            left.bits() == right.bits()
                        }
                        (left, right) => left == right,
                    }
            })
    }

    /// Computes structural equivalence after context-specific fast paths.
    fn compute_same_type_for_equiv(&self, left: InternedTyId, right: InternedTyId) -> bool {
        match (self.ty_kind_for_equiv(left), self.ty_kind_for_equiv(right)) {
            (Some(TyKind::Error), Some(TyKind::Error)) => true,
            (Some(TyKind::ConstOnly), Some(TyKind::ConstOnly)) => true,
            (Some(TyKind::Opaque), Some(TyKind::Opaque)) => true,
            (Some(TyKind::Primitive(left)), Some(TyKind::Primitive(right))) => left == right,
            (Some(TyKind::Tuple(left)), Some(TyKind::Tuple(right))) => {
                self.same_type_args_for_equiv(left, right)
            }
            (Some(TyKind::GenericParam(left)), Some(TyKind::GenericParam(right))) => left == right,
            (Some(TyKind::SelfParam), Some(TyKind::SelfParam)) => true,
            (Some(TyKind::BuiltinType(left)), Some(TyKind::BuiltinType(right))) => left == right,
            (
                Some(TyKind::Vector {
                    elem: left_elem,
                    lanes: left_lanes,
                }),
                Some(TyKind::Vector {
                    elem: right_elem,
                    lanes: right_lanes,
                }),
            ) => left_elem == right_elem && left_lanes == right_lanes,
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
                Some(TyKind::Callable {
                    is_readonly: left_readonly,
                    params: left_params,
                    return_type: left_return,
                }),
                Some(TyKind::Callable {
                    is_readonly: right_readonly,
                    params: right_params,
                    return_type: right_return,
                }),
            ) => {
                left_readonly == right_readonly
                    && self.same_type_args_for_equiv(left_params, right_params)
                    && self.same_type_for_equiv(*left_return, *right_return)
            }
            (
                Some(TyKind::CallablePointee {
                    params: left_params,
                    return_type: left_return,
                }),
                Some(TyKind::CallablePointee {
                    params: right_params,
                    return_type: right_return,
                }),
            ) => {
                self.same_type_args_for_equiv(left_params, right_params)
                    && self.same_type_for_equiv(*left_return, *right_return)
            }
            (
                Some(TyKind::ClosureState {
                    closure_id: left_id,
                    captures: left_captures,
                    params: left_params,
                    return_type: left_return,
                }),
                Some(TyKind::ClosureState {
                    closure_id: right_id,
                    captures: right_captures,
                    params: right_params,
                    return_type: right_return,
                }),
            ) => {
                left_id == right_id
                    && self.same_type_args_for_equiv(left_captures, right_captures)
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

    /// Compares associated bindings as an order-independent multiset.
    fn same_associated_type_bindings_for_equiv(
        &self,
        left: &[AssociatedTypeBindingTy],
        right: &[AssociatedTypeBindingTy],
    ) -> bool {
        if left.len() != right.len() {
            return false;
        }
        let mut used = vec![false; right.len()];
        left.iter().all(|left_binding| {
            let Some(index) = right.iter().enumerate().find_map(|(index, right_binding)| {
                (!used[index]
                    && self.same_associated_type_binding_key_for_equiv(left_binding, right_binding)
                    && self.same_type_for_equiv(left_binding.ty, right_binding.ty))
                .then_some(index)
            }) else {
                return false;
            };
            used[index] = true;
            true
        })
    }

    /// Compares the trait-instance key of two associated bindings.
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
    /// All primitive scalar types in canonical registry order.
    pub const ALL: [Self; 17] = [
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
        Self::Never,
    ];

    /// Parses a source-level primitive type name.
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
            "!" => Self::Never,
            _ => return None,
        })
    }

    /// Resolves a well-known symbol to a primitive type.
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
            nia_symbol::known::NEVER => Self::Never,
            _ => return None,
        })
    }

    /// Returns the canonical well-known symbol.
    pub fn symbol_id(self) -> SymbolId {
        match self {
            Self::I8 => nia_symbol::known::I8,
            Self::I16 => nia_symbol::known::I16,
            Self::I32 => nia_symbol::known::I32,
            Self::I64 => nia_symbol::known::I64,
            Self::I128 => nia_symbol::known::I128,
            Self::Isize => nia_symbol::known::ISIZE,
            Self::U8 => nia_symbol::known::U8,
            Self::U16 => nia_symbol::known::U16,
            Self::U32 => nia_symbol::known::U32,
            Self::U64 => nia_symbol::known::U64,
            Self::U128 => nia_symbol::known::U128,
            Self::Usize => nia_symbol::known::USIZE,
            Self::F32 => nia_symbol::known::F32,
            Self::F64 => nia_symbol::known::F64,
            Self::Bool => nia_symbol::known::BOOL,
            Self::Char => nia_symbol::known::CHAR,
            Self::Never => nia_symbol::known::NEVER,
        }
    }

    /// Returns the source-level spelling.
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
            Self::Never => "!",
        }
    }

    /// Parses a primitive that can be used as a vector lane.
    pub fn vector_element_from_name(name: &str) -> Option<Self> {
        match Self::from_name(name)? {
            primitive if primitive.is_vector_element() => Some(primitive),
            _ => None,
        }
    }

    /// Returns whether this primitive is valid as a vector lane.
    pub fn is_vector_element(self) -> bool {
        !matches!(self, Self::Char | Self::Never)
    }

    /// Returns whether this is an integer type.
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

    /// Returns whether this is a signed integer type.
    pub fn is_signed_integer(self) -> bool {
        matches!(
            self,
            Self::I8 | Self::I16 | Self::I32 | Self::I64 | Self::I128 | Self::Isize
        )
    }

    /// Returns the integer width, using the artifact width for pointer integers.
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

    /// Returns whether this is a floating-point type.
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
    /// Parses a scalar or fixed-vector primitive spelling.
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
    fn integer_representability_covers_signed_and_unsigned_endpoints() {
        assert!(IntConst::unsigned(u128::MAX).fits_primitive_int(PrimitiveTy::U128, 64));
        assert!(!IntConst::unsigned(u128::MAX).fits_primitive_int(PrimitiveTy::I128, 64));
        assert!(IntConst::signed(i128::MIN).fits_primitive_int(PrimitiveTy::I128, 64));
        assert!(!IntConst::signed(-1).fits_primitive_int(PrimitiveTy::U128, 64));
        assert!(IntConst::unsigned(255).fits_primitive_int(PrimitiveTy::U8, 64));
        assert!(!IntConst::unsigned(256).fits_primitive_int(PrimitiveTy::U8, 64));
        assert!(IntConst::unsigned(u32::MAX.into()).fits_primitive_int(PrimitiveTy::Usize, 32));
        assert!(!IntConst::unsigned(u64::MAX.into()).fits_primitive_int(PrimitiveTy::Usize, 32));
        assert!(!IntConst::unsigned(0).fits_primitive_int(PrimitiveTy::Usize, 0));
        assert!(!IntConst::unsigned(0).fits_primitive_int(PrimitiveTy::Usize, 129));
    }

    struct DualStoreEquivalence<'a> {
        left: &'a TypeStore,
        right: &'a TypeStore,
    }

    impl TypeEquivalence for DualStoreEquivalence<'_> {
        fn ty_kind_for_equiv(&self, ty: InternedTyId) -> Option<&TyKind> {
            self.left.get(ty).or_else(|| self.right.get(ty))
        }

        fn same_array_len_for_equiv(&self, left: &ArrayLenTy, right: &ArrayLenTy) -> bool {
            left == right
        }

        fn same_type_for_equiv(&self, left: InternedTyId, right: InternedTyId) -> bool {
            left == right || self.compute_same_type_for_equiv(left, right)
        }
    }

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
    fn structural_equivalence_covers_non_leaf_runtime_types_across_stores() {
        let left = TypeStore::new();
        let right = TypeStore::new();
        let module_id = nia_ids::ModuleIdAllocator::new().allocate();
        let left_append = left.append_for_module(module_id);
        let right_append = right.append_for_module(module_id);
        let left_i32 = left_append.primitive(PrimitiveTy::I32);
        let right_i32 = right_append.primitive(PrimitiveTy::I32);
        let closure_id = nia_ids::ClosureId {
            owner: nia_ids::GlobalDefId {
                module_id,
                def_id: nia_ids::DefId(7),
            },
            ordinal: 2,
        };
        let left_types = [
            left_append.intern(TyKind::ConstOnly),
            left_append.intern(TyKind::Vector {
                elem: PrimitiveTy::I32,
                lanes: 4,
            }),
            left_append.intern(TyKind::ClosureState {
                closure_id,
                captures: vec![left_i32],
                params: vec![left_i32],
                return_type: left_i32,
            }),
        ];
        let right_types = [
            right_append.intern(TyKind::ConstOnly),
            right_append.intern(TyKind::Vector {
                elem: PrimitiveTy::I32,
                lanes: 4,
            }),
            right_append.intern(TyKind::ClosureState {
                closure_id,
                captures: vec![right_i32],
                params: vec![right_i32],
                return_type: right_i32,
            }),
        ];
        let equivalence = DualStoreEquivalence {
            left: &left,
            right: &right,
        };

        for (left, right) in left_types.into_iter().zip(right_types) {
            assert!(equivalence.same_type_for_equiv(left, right));
        }
    }

    #[test]
    fn default_equivalence_matches_integer_const_bits_across_stores() {
        let left = TypeStore::new();
        let right = TypeStore::new();
        let module_id = nia_ids::ModuleIdAllocator::new().allocate();
        let left_append = left.append_for_module(module_id);
        let right_append = right.append_for_module(module_id);
        let left_usize = left_append.primitive(PrimitiveTy::Usize);
        let right_usize = right_append.primitive(PrimitiveTy::Usize);
        let def_id = nia_ids::GlobalDefId {
            module_id,
            def_id: nia_ids::DefId(9),
        };
        let left_ty = left_append.intern(TyKind::Nominal {
            def_id,
            args: Vec::new(),
            const_args: vec![ConstGenericArg {
                ty: left_usize,
                value: ConstGenericValue::Int(IntConst::signed(11)),
            }],
        });
        let right_ty = right_append.intern(TyKind::Nominal {
            def_id,
            args: Vec::new(),
            const_args: vec![ConstGenericArg {
                ty: right_usize,
                value: ConstGenericValue::Int(IntConst::unsigned(11)),
            }],
        });
        let equivalence = DualStoreEquivalence {
            left: &left,
            right: &right,
        };

        assert!(equivalence.same_type_for_equiv(left_ty, right_ty));
    }

    #[test]
    fn associated_binding_equivalence_matches_values_with_duplicate_keys() {
        let left = TypeStore::new();
        let right = TypeStore::new();
        let module_id = nia_ids::ModuleIdAllocator::new().allocate();
        let left_append = left.append_for_module(module_id);
        let right_append = right.append_for_module(module_id);
        let left_i32 = left_append.primitive(PrimitiveTy::I32);
        let left_bool = left_append.primitive(PrimitiveTy::Bool);
        let right_i32 = right_append.primitive(PrimitiveTy::I32);
        let right_bool = right_append.primitive(PrimitiveTy::Bool);
        let trait_id = TraitId::Source(nia_ids::GlobalDefId {
            module_id,
            def_id: nia_ids::DefId(8),
        });
        let name = SymbolId::from_stable_hash(8);
        let binding = |ty| AssociatedTypeBindingTy {
            name,
            trait_id: Some(trait_id),
            trait_args: Vec::new(),
            trait_const_args: Vec::new(),
            ty,
        };
        let left_ty = left_append.intern(TyKind::TraitObject {
            is_readonly: false,
            trait_id,
            trait_args: Vec::new(),
            trait_const_args: Vec::new(),
            associated_type_bindings: vec![binding(left_i32), binding(left_bool)],
        });
        let right_ty = right_append.intern(TyKind::TraitObject {
            is_readonly: false,
            trait_id,
            trait_args: Vec::new(),
            trait_const_args: Vec::new(),
            associated_type_bindings: vec![binding(right_bool), binding(right_i32)],
        });
        let equivalence = DualStoreEquivalence {
            left: &left,
            right: &right,
        };

        assert!(equivalence.same_type_for_equiv(left_ty, right_ty));
        let right_mismatch = right_append.intern(TyKind::TraitObject {
            is_readonly: false,
            trait_id,
            trait_args: Vec::new(),
            trait_const_args: Vec::new(),
            associated_type_bindings: vec![binding(right_i32), binding(right_i32)],
        });
        assert!(!equivalence.same_type_for_equiv(left_ty, right_mismatch));
    }

    #[test]
    fn tuple_identity_preserves_arity_and_element_order() {
        let store = TypeStore::new();
        let module_id = nia_ids::ModuleIdAllocator::new().allocate();
        let append = store.append_for_module(module_id);
        let i32_ty = append.primitive(PrimitiveTy::I32);
        let bool_ty = append.primitive(PrimitiveTy::Bool);
        let unit = append.intern(TyKind::Tuple(Vec::new()));
        let singleton = append.intern(TyKind::Tuple(vec![i32_ty]));
        let pair = append.intern(TyKind::Tuple(vec![i32_ty, bool_ty]));
        let reversed = append.intern(TyKind::Tuple(vec![bool_ty, i32_ty]));

        assert!(store.get(unit).is_some_and(TyKind::is_unit));
        assert_ne!(singleton, i32_ty);
        assert_ne!(pair, reversed);
        assert_eq!(store.get(singleton), Some(&TyKind::Tuple(vec![i32_ty])));
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
    fn pointers_to_callable_pointees_canonicalize_to_callable_views() {
        let store = TypeStore::new();
        let module_id = nia_ids::ModuleIdAllocator::new().allocate();
        let append = store.append_for_module(module_id);
        let i32_ty = append.primitive(PrimitiveTy::I32);
        let pointee = append.intern(TyKind::CallablePointee {
            params: vec![i32_ty],
            return_type: i32_ty,
        });
        let view = append.intern(TyKind::Pointer {
            is_readonly: true,
            elem: pointee,
        });

        assert_eq!(
            store.get(view),
            Some(&TyKind::Callable {
                is_readonly: true,
                params: vec![i32_ty],
                return_type: i32_ty,
            })
        );
    }

    #[test]
    fn array_length_substitution_rejects_negative_signed_values() {
        let store = TypeStore::new();
        let module_id = nia_ids::ModuleIdAllocator::new().allocate();
        let usize_ty = store
            .append_for_module(module_id)
            .primitive(PrimitiveTy::Usize);
        let argument = |value| ConstGenericArg {
            ty: usize_ty,
            value,
        };

        assert_eq!(
            array_len_from_const_arg(&argument(ConstGenericValue::Int(IntConst::signed(-1)))),
            None
        );
        assert_eq!(
            array_len_from_const_arg(&argument(ConstGenericValue::Int(IntConst::signed(3)))),
            Some(ArrayLenTy::ConstValue(3))
        );
        assert_eq!(
            array_len_from_const_arg(&argument(ConstGenericValue::Int(IntConst::unsigned(3)))),
            Some(ArrayLenTy::ConstValue(3))
        );
        assert_eq!(
            array_len_from_const_arg(&argument(ConstGenericValue::Int(IntConst::unsigned(
                u128::from(u64::MAX) + 1,
            )))),
            None
        );
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
