// SPDX-License-Identifier: GPL-3.0-or-later
//! Canonical type-store identities and structural type equivalence.
//!
//! [`TyKind`] is the compiler type model, and [`TypeStore`] is the session-wide
//! canonical store for it. Every compiler pass reads `InternedTyId` handles
//! from the store and publishes new types through a module-scoped
//! [`TypeStoreAppend`]. The store is append-only within a session, and every
//! handle is validated against the store identity it carries, so a handle from
//! another session is rejected before it is interpreted.
//!
//! Canonicalization maps each `TyKind` to one global slot, so the same
//! primitive or structural kind published from different modules receives the
//! same id. The kind arena is a sparse four-level `OnceLock` trie that is
//! lock-free to read once a slot is initialized.
//!
//! Callable interfaces have one extra canonicalization rule.
//! `TyKind::CallablePointee` is the unsized signature-bearing pointee and
//! `TyKind::Callable` is its sized dynamic view; publishing an ordinary pointer
//! to a callable pointee yields the corresponding callable view.
use nia_hash::FastHashMap;
pub use nia_ids::{BuiltinTrait, BuiltinType, LayoutBuiltin, TraitId};
use nia_ids::{
    ClosureId, GlobalConstExprId, GlobalDefId, InternedTyId, ModuleId, TypeStoreId, TypeStoreIndex,
};
use nia_span::Span;
use nia_symbol::SymbolId;
use parking_lot::Mutex;
use std::fmt;
use std::sync::{Arc, OnceLock};

mod const_expression;
mod substitution;

pub use const_expression::{
    ConstBinaryOp, ConstComputation, ConstComputationKind, ConstMatchArm, ConstPattern,
    ConstUnaryOp,
};

pub use substitution::{
    array_len_from_const_arg, array_len_from_const_value, const_generic_value_from_array_len,
    substitute_const_generic_arg, substitute_ty,
};

const TYPE_KIND_ARENA_FANOUT: usize = 256;

/// Calling convention carried by a thin function pointer type.
///
/// A function item's linkage determines this value when its address is taken.
/// Keeping it in the type prevents a C function pointer from being passed to a
/// Nia indirect call (or the other way around) after it has been stored.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum FunctionPointerAbi {
    /// The internal Nia calling convention.
    Nia,
    /// The target's C calling convention.
    C,
}

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
    fn insert(&self, index: u32, kind: Arc<TyKind>) -> nia_ice::IceResult<()> {
        let [root_index, trunk_index, branch_index, leaf_index] =
            index.to_be_bytes().map(usize::from);
        let trunk = self.roots[root_index].get_or_init(|| Box::new(empty_type_kind_level()));
        let branch = trunk[trunk_index].get_or_init(|| Box::new(empty_type_kind_level()));
        let leaf = branch[branch_index].get_or_init(|| Box::new(empty_type_kind_level()));
        leaf[leaf_index]
            .set(kind)
            .map_err(|_| nia_ice::Ice::new("type store kind slot was published twice"))?;
        Ok(())
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
    error: InternedTyId,
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
    fn intern(&self, kind: &TyKind) -> nia_ice::IceResult<InternedTyId> {
        let mut foreign_reference = None;
        kind.visit_referenced_types(|referenced| {
            if foreign_reference.is_none() && self.get(referenced).is_none() {
                foreign_reference = Some(referenced);
            }
        });
        if let Some(referenced) = foreign_reference {
            return Err(nia_ice::Ice::new(format!(
                "interned type references handle {referenced:?} outside its session type store"
            )));
        }
        let mut slots = self.slots.lock();
        if let Some(ty) = slots.canonical.get(kind) {
            return Ok(*ty);
        }
        let index = u32::try_from(slots.canonical.len())
            .map_err(|_| nia_ice::Ice::new("type store slot space exhausted"))?;
        let ty = InternedTyId::new(self.id, TypeStoreIndex::from_store_index(index));
        let kind = Arc::new(kind.clone());
        self.kinds.insert(index, Arc::clone(&kind))?;
        slots.canonical.insert(kind, ty);
        Ok(ty)
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
    error: InternedTyId,
}

impl TypeStoreAppend {
    /// Interns a type, canonicalizing callable pointers into callable views.
    pub fn intern(&self, kind: TyKind) -> nia_ice::IceResult<InternedTyId> {
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
        self.error
    }

    /// Interns a primitive type.
    pub fn primitive(&self, primitive: PrimitiveTy) -> nia_ice::IceResult<InternedTyId> {
        self.intern(TyKind::Primitive(primitive))
    }

    /// Interns a builtin nominal type.
    pub fn builtin_type(&self, builtin: BuiltinType) -> nia_ice::IceResult<InternedTyId> {
        self.intern(TyKind::BuiltinType(builtin))
    }
}

impl TypeStore {
    /// Creates an empty type store with a fresh session identity.
    pub fn new() -> nia_ice::IceResult<Self> {
        let id = TypeStoreId::fresh()?;
        let core = Arc::new(TypeStoreCore {
            id,
            slots: Mutex::new(TypeStoreSlots::default()),
            kinds: TypeKindArena::default(),
        });
        let error = core.intern(&TyKind::Error)?;
        Ok(Self { id, core, error })
    }

    /// Returns this store's session identity.
    pub fn id(&self) -> TypeStoreId {
        self.id
    }

    /// Returns the canonical recovery type guaranteed by store construction.
    pub fn error(&self) -> InternedTyId {
        self.error
    }

    /// Returns the number of canonical types currently interned in this store.
    pub fn len(&self) -> usize {
        self.core.slots.lock().canonical.len()
    }

    /// Returns whether this store has no canonical types.
    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }

    /// Looks up a type handle, rejecting handles from another store.
    pub fn get(&self, ty: InternedTyId) -> Option<&TyKind> {
        self.core.get(ty)
    }

    #[doc(hidden)]
    pub fn append_for_module(&self, _module_id: ModuleId) -> TypeStoreAppend {
        TypeStoreAppend {
            core: Arc::clone(&self.core),
            error: self.error,
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
    /// Thin function pointer with an explicit calling convention.
    FunctionPointer {
        /// Parameter types.
        params: Vec<InternedTyId>,
        /// Return type.
        return_type: InternedTyId,
        /// Whether the function accepts variadic arguments.
        is_variadic: bool,
        /// Calling convention used by indirect calls through this pointer.
        abi: FunctionPointerAbi,
    },
    /// A zero-sized, statically known function value.
    ///
    /// Unlike [`TyKind::FunctionPointer`], this preserves the exact function
    /// identity and concrete generic arguments. It is lowered directly to a
    /// function callee when used in a statically dispatched call.
    FunctionItem {
        /// Defining function identity.
        def_id: GlobalDefId,
        /// Module supplying generic argument resolution context.
        arg_module_id: ModuleId,
        /// Optional receiver substitution for an associated function item.
        self_arg: Option<InternedTyId>,
        /// Type arguments in canonical order.
        args: Vec<InternedTyId>,
        /// Const arguments paired with `args`.
        const_args: Vec<ConstGenericArg>,
        /// Parameter types after applying all substitutions.
        params: Vec<InternedTyId>,
        /// Return type after applying all substitutions.
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
        /// Concrete substitutions of the enclosing generic function.
        owner_instance: Option<ClosureOwnerInstance>,
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

/// Concrete owner context for a closure materialized from a generic function.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct ClosureOwnerInstance {
    /// Module supplying generic argument identities.
    pub arg_module_id: ModuleId,
    /// Optional receiver substitution.
    pub self_arg: Option<InternedTyId>,
    /// Type arguments in canonical order.
    pub args: Vec<InternedTyId>,
    /// Const arguments paired with `args`.
    pub const_args: Vec<ConstGenericArg>,
}

impl TyKind {
    /// Visits parameter uses stored directly in this node. Combine with
    /// `visit_referenced_types` to inspect a complete type, including const
    /// arguments on nominal, callable, projection, and trait-object nodes.
    pub fn visit_generic_parameters(
        &self,
        mut visit_type: impl FnMut(SymbolId),
        mut visit_const: impl FnMut(SymbolId),
    ) {
        fn const_args(args: &[ConstGenericArg], visit: &mut impl FnMut(SymbolId)) {
            for arg in args {
                arg.visit_const_parameters(&mut *visit);
            }
        }
        match self {
            Self::GenericParam(name) => visit_type(*name),
            Self::Array {
                len: ArrayLenTy::GenericParam(name),
                ..
            } => visit_const(*name),
            Self::Array {
                len: ArrayLenTy::Computation(expr),
                ..
            } => {
                for operand in expr.operands() {
                    operand.visit_const_parameters(&mut visit_const);
                }
            }
            Self::Nominal {
                const_args: args, ..
            }
            | Self::FunctionItem {
                const_args: args, ..
            }
            | Self::Projection {
                trait_const_args: args,
                ..
            } => const_args(args, &mut visit_const),
            Self::ClosureState { owner_instance, .. } => {
                if let Some(owner) = owner_instance {
                    const_args(&owner.const_args, &mut visit_const);
                }
            }
            Self::TraitObject {
                trait_const_args,
                associated_type_bindings,
                ..
            }
            | Self::TraitObjectPointee {
                trait_const_args,
                associated_type_bindings,
                ..
            } => {
                const_args(trait_const_args, &mut visit_const);
                for binding in associated_type_bindings {
                    const_args(&binding.trait_const_args, &mut visit_const);
                }
            }
            Self::Error
            | Self::ConstOnly
            | Self::Opaque
            | Self::Primitive(_)
            | Self::Tuple(_)
            | Self::Pointer { .. }
            | Self::VolatilePointer { .. }
            | Self::Slice { .. }
            | Self::SlicePointee { .. }
            | Self::Array { .. }
            | Self::Vector { .. }
            | Self::Range { .. }
            | Self::FunctionPointer { .. }
            | Self::Callable { .. }
            | Self::CallablePointee { .. }
            | Self::Optional { .. }
            | Self::ErrorUnion { .. }
            | Self::BuiltinType(_)
            | Self::BuiltinTrait { .. }
            | Self::SelfParam => {}
        }
    }

    /// Returns whether this is the zero-element tuple/unit type.
    pub fn is_unit(&self) -> bool {
        matches!(self, Self::Tuple(elems) if elems.is_empty())
    }

    /// Visits every type handle nested in this structural type.
    pub fn visit_referenced_types(&self, mut visit: impl FnMut(InternedTyId)) {
        fn visit_const_args(args: &[ConstGenericArg], visit: &mut impl FnMut(InternedTyId)) {
            for arg in args {
                arg.visit_referenced_types(&mut *visit);
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
                len.visit_referenced_types(&mut visit);
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
            Self::FunctionItem {
                args,
                const_args,
                params,
                return_type,
                self_arg,
                ..
            } => {
                if let Some(self_arg) = self_arg {
                    visit(*self_arg);
                }
                for arg in args {
                    visit(*arg);
                }
                visit_const_args(const_args, &mut visit);
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
                owner_instance,
                captures,
                params,
                return_type,
                ..
            } => {
                if let Some(owner) = owner_instance {
                    if let Some(self_arg) = owner.self_arg {
                        visit(self_arg);
                    }
                    for arg in &owner.args {
                        visit(*arg);
                    }
                    visit_const_args(&owner.const_args, &mut visit);
                }
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
    ///
    /// This is the semantics of an `as` conversion. A value that becomes a
    /// typed constant, such as a const argument, must instead be
    /// representable: see [`Self::in_primitive_int`].
    pub fn cast_to_primitive_int(self, primitive: PrimitiveTy, pointer_width: u32) -> Option<Self> {
        let width = primitive.integer_bits(pointer_width)?;
        if !(1..=u128::BITS).contains(&width) {
            return None;
        }
        let mask = integer_mask(width);
        let bits = self.bits & mask;
        if primitive.is_signed_integer() {
            // IntConst has no stored width: negative values must be canonical
            // 128-bit two's complement, rather than a narrow raw-bit pattern.
            let sign = 1_u128 << (width - 1);
            Some(Self::signed_bits(if bits & sign == 0 {
                bits
            } else {
                bits | !mask
            }))
        } else {
            Some(Self::unsigned(bits))
        }
    }

    /// The value as a constant of `primitive`, when it is representable: the
    /// canonical bits and signedness of that type. Unlike a cast it never
    /// wraps, so `300` is not a `u8` constant.
    pub fn in_primitive_int(self, primitive: PrimitiveTy, pointer_width: u32) -> Option<Self> {
        if !primitive.is_integer() || !self.fits_primitive_int(primitive, pointer_width) {
            return None;
        }
        self.cast_to_primitive_int(primitive, pointer_width)
    }
}

impl From<i128> for IntConst {
    fn from(value: i128) -> Self {
        Self::from_i128(value)
    }
}

impl fmt::Display for IntConst {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        if self.signed {
            write!(formatter, "{}", self.bits as i128)
        } else {
            write!(formatter, "{}", self.bits)
        }
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

impl ConstGenericArg {
    /// Structural equality independent of the stores owning embedded types.
    pub fn equivalent(
        &self,
        other: &Self,
        same_type: &mut dyn FnMut(InternedTyId, InternedTyId) -> bool,
    ) -> bool {
        same_type(self.ty, other.ty)
            && match (&self.value, &other.value) {
                (ConstGenericValue::Int(left), ConstGenericValue::Int(right)) => {
                    left.bits() == right.bits()
                }
                (
                    ConstGenericValue::Layout {
                        builtin: lb,
                        ty: lt,
                    },
                    ConstGenericValue::Layout {
                        builtin: rb,
                        ty: rt,
                    },
                ) => lb == rb && same_type(*lt, *rt),
                (ConstGenericValue::Computation(left), ConstGenericValue::Computation(right)) => {
                    left.equivalent(right, same_type)
                }
                (left, right) => left == right,
            }
    }

    /// Visits every type handle carried by this argument.
    ///
    /// Layout queries carry a second type, distinct from their `usize` type.
    pub fn visit_referenced_types(&self, mut visit: impl FnMut(InternedTyId)) {
        self.visit_types(&mut visit);
    }

    fn visit_types(&self, visit: &mut dyn FnMut(InternedTyId)) {
        visit(self.ty);
        match &self.value {
            ConstGenericValue::Layout { ty, .. } => visit(*ty),
            ConstGenericValue::Computation(expr) => expr.visit_referenced_types(visit),
            _ => {}
        }
    }

    /// Visits free const-parameter references, including computation operands.
    pub fn visit_const_parameters(&self, visit: &mut dyn FnMut(SymbolId)) {
        match &self.value {
            ConstGenericValue::GenericParam(name) => visit(*name),
            ConstGenericValue::Computation(expr) => {
                for arg in expr.operands() {
                    arg.visit_const_parameters(visit);
                }
            }
            _ => {}
        }
    }

    /// Maps every type handle carried by this argument.
    pub fn map_types(&mut self, mut map: impl FnMut(InternedTyId) -> InternedTyId) {
        match self.try_map_types::<std::convert::Infallible>(|ty| Ok(map(ty))) {
            Ok(()) => {}
            Err(never) => match never {},
        }
    }

    /// Fallibly maps every type handle carried by this argument.
    pub fn try_map_types<E>(
        &mut self,
        mut map: impl FnMut(InternedTyId) -> Result<InternedTyId, E>,
    ) -> Result<(), E> {
        self.try_map_types_with(&mut map)
    }

    fn try_map_types_with<E>(
        &mut self,
        map: &mut dyn FnMut(InternedTyId) -> Result<InternedTyId, E>,
    ) -> Result<(), E> {
        let ty = map(self.ty)?;
        let value = match &self.value {
            ConstGenericValue::Layout { builtin, ty } => ConstGenericValue::Layout {
                builtin: *builtin,
                ty: map(*ty)?,
            },
            ConstGenericValue::Computation(expr) => {
                let map = std::cell::RefCell::new(map);
                ConstGenericValue::Computation(expr.try_map(
                    |arg| {
                        let mut arg = arg.clone();
                        arg.try_map_types_with(&mut **map.borrow_mut())?;
                        Ok(arg)
                    },
                    |ty| map.borrow_mut()(ty),
                )?)
            }
            value => value.clone(),
        };
        self.ty = ty;
        self.value = value;
        Ok(())
    }

    /// Whether any type carried by this argument satisfies a predicate.
    pub fn any_type(&self, mut predicate: impl FnMut(InternedTyId) -> bool) -> bool {
        let mut found = false;
        self.visit_types(&mut |ty| found |= predicate(ty));
        found
    }

    /// Whether every type handle carried by this argument satisfies a predicate.
    pub fn all_types(&self, mut predicate: impl FnMut(InternedTyId) -> bool) -> bool {
        !self.any_type(|ty| !predicate(ty))
    }

    /// Whether the value or any type it carries depends on a generic parameter.
    pub fn contains_generic(&self, type_contains: impl FnMut(InternedTyId) -> bool) -> bool {
        let mut has_parameter = false;
        self.visit_const_parameters(&mut |_| has_parameter = true);
        has_parameter || self.any_type(type_contains)
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
/// Value identity of a const-generic argument.
pub enum ConstGenericValue {
    /// Unsubstituted const parameter.
    GenericParam(SymbolId),
    /// Typed residual computation; execution is deferred until substitution.
    Computation(Arc<ConstComputation>),
    /// Unevaluated global const expression.
    ConstExpr(GlobalConstExprId),
    /// Integer value with explicit signedness.
    Int(IntConst),
    /// Boolean value.
    Bool(bool),
    /// Unicode scalar value.
    Char(char),
    /// The size or alignment of a type, a `usize` that depends on generic
    /// parameters in `ty` and is a value in each instance. It is the
    /// const-argument form of [`ArrayLenTy::Builtin`], so a length such as
    /// `@size[T]()` binds a const parameter `N` like any other length.
    Layout {
        /// The layout query.
        builtin: LayoutBuiltin,
        /// The queried type.
        ty: InternedTyId,
    },
}

/// Why a scalar cannot be the value of a const of some primitive type.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ConstScalarError {
    /// The integer is not representable in the type: `300` for `u8`.
    OutOfRange {
        /// The integer.
        value: IntConst,
        /// The type it was given.
        ty: PrimitiveTy,
    },
    /// The integer is not a Unicode scalar value, for `char`.
    NotUnicodeScalar(IntConst),
    /// The scalar's kind is not the type's, such as `true` for `usize`.
    KindMismatch(PrimitiveTy),
}

impl fmt::Display for ConstScalarError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::OutOfRange { value, ty } => write!(
                formatter,
                "const integer value {value} is out of range for {}",
                ty.name()
            ),
            Self::NotUnicodeScalar(value) => {
                write!(
                    formatter,
                    "const integer value {value} is not a Unicode scalar"
                )
            }
            Self::KindMismatch(ty) => {
                write!(formatter, "const value does not have type {}", ty.name())
            }
        }
    }
}

impl ConstGenericValue {
    /// The value an integer gives a const of primitive type `ty`: an integer
    /// type takes its canonical form when representable, and `char` takes a
    /// Unicode scalar. Every phase that turns an evaluated or folded integer
    /// into a const argument or a residual operand uses this, so equal values
    /// of one type have one identity and nothing wraps silently.
    pub fn from_int(
        ty: PrimitiveTy,
        value: IntConst,
        pointer_width: u32,
    ) -> Result<Self, ConstScalarError> {
        if ty == PrimitiveTy::Char {
            return u32::try_from(
                value
                    .as_i128()
                    .ok_or(ConstScalarError::NotUnicodeScalar(value))?,
            )
            .ok()
            .and_then(char::from_u32)
            .map(Self::Char)
            .ok_or(ConstScalarError::NotUnicodeScalar(value));
        }
        if !ty.is_integer() {
            return Err(ConstScalarError::KindMismatch(ty));
        }
        value
            .in_primitive_int(ty, pointer_width)
            .map(Self::Int)
            .ok_or(ConstScalarError::OutOfRange { value, ty })
    }

    /// The value a boolean gives a const of primitive type `ty`.
    pub fn from_bool(ty: PrimitiveTy, value: bool) -> Result<Self, ConstScalarError> {
        if ty == PrimitiveTy::Bool {
            Ok(Self::Bool(value))
        } else {
            Err(ConstScalarError::KindMismatch(ty))
        }
    }

    /// The canonical closed form of this value for a const of type `ty`.
    /// An integer is re-derived through [`Self::from_int`], so values that
    /// different evaluators produced with different signedness agree.
    /// Symbolic and non-integer values are returned unchanged.
    pub fn canonical_for(
        self,
        ty: PrimitiveTy,
        pointer_width: u32,
    ) -> Result<Self, ConstScalarError> {
        match self {
            Self::Int(value) => Self::from_int(ty, value, pointer_width),
            value => Ok(value),
        }
    }
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
    /// Typed residual `usize` computation.
    Computation(Arc<ConstComputation>),
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

impl ArrayLenTy {
    /// Visits types carried by layout queries and computed lengths.
    pub fn visit_referenced_types(&self, mut visit: impl FnMut(InternedTyId)) {
        match self {
            Self::Builtin { ty, .. } => visit(*ty),
            Self::Computation(expr) => expr.visit_referenced_types(&mut visit),
            _ => {}
        }
    }
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
                left.equivalent(right, &mut |l, r| self.same_type_for_equiv(l, r))
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
                    abi: left_abi,
                }),
                Some(TyKind::FunctionPointer {
                    params: right_params,
                    return_type: right_return,
                    is_variadic: right_variadic,
                    abi: right_abi,
                }),
            ) => {
                left_abi == right_abi
                    && left_variadic == right_variadic
                    && self.same_type_args_for_equiv(left_params, right_params)
                    && self.same_type_for_equiv(*left_return, *right_return)
            }
            (
                Some(TyKind::FunctionItem {
                    def_id: left_def,
                    arg_module_id: _left_module,
                    self_arg: left_self,
                    args: left_args,
                    const_args: left_const_args,
                    params: left_params,
                    return_type: left_return,
                    is_variadic: left_variadic,
                }),
                Some(TyKind::FunctionItem {
                    def_id: right_def,
                    arg_module_id: _right_module,
                    self_arg: right_self,
                    args: right_args,
                    const_args: right_const_args,
                    params: right_params,
                    return_type: right_return,
                    is_variadic: right_variadic,
                }),
            ) => {
                left_def == right_def
                    && left_variadic == right_variadic
                    && match (left_self, right_self) {
                        (Some(left), Some(right)) => self.same_type_for_equiv(*left, *right),
                        (None, None) => true,
                        _ => false,
                    }
                    && self.same_type_args_for_equiv(left_args, right_args)
                    && self.same_const_generic_args_for_equiv(left_const_args, right_const_args)
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
                    owner_instance: left_owner,
                    captures: left_captures,
                    params: left_params,
                    return_type: left_return,
                }),
                Some(TyKind::ClosureState {
                    closure_id: right_id,
                    owner_instance: right_owner,
                    captures: right_captures,
                    params: right_params,
                    return_type: right_return,
                }),
            ) => {
                left_id == right_id
                    && left_owner == right_owner
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
    use nia_ids::ModuleIdAllocator;

    #[test]
    fn type_store_len_tracks_canonical_interning() {
        let store = TypeStore::new().expect("create type store");
        assert_eq!(store.len(), 1, "the recovery type is the initial slot");
        let module = ModuleIdAllocator::new()
            .expect("create module allocator")
            .allocate()
            .expect("allocate module");
        let append = store.append_for_module(module);
        let first = append
            .primitive(PrimitiveTy::I32)
            .expect("intern primitive type");
        assert_eq!(store.len(), 2);
        let duplicate = append
            .primitive(PrimitiveTy::I32)
            .expect("reuse primitive type");
        assert_eq!(first, duplicate);
        assert_eq!(store.len(), 2);
    }

    fn intern(append: &TypeStoreAppend, kind: TyKind) -> InternedTyId {
        append.intern(kind).expect("intern test type")
    }

    fn primitive(append: &TypeStoreAppend, ty: PrimitiveTy) -> InternedTyId {
        append.primitive(ty).expect("intern test primitive")
    }

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

    #[test]
    fn integer_display_uses_source_semantics_not_storage_debug() {
        assert_eq!(IntConst::signed(-7).to_string(), "-7");
        assert_eq!(IntConst::unsigned(7).to_string(), "7");
        assert_eq!(
            IntConst::unsigned(u128::MAX).to_string(),
            u128::MAX.to_string()
        );
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
            arena
                .insert(index, Arc::new(TyKind::Error))
                .expect("populate test arena slot");
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
        let store = TypeStore::new().expect("create type store");
        let module_id = nia_ids::ModuleIdAllocator::new()
            .expect("create module ID allocator")
            .allocate()
            .expect("allocate module ID");
        let append = store.append_for_module(module_id);
        let first = primitive(&append, PrimitiveTy::I32);
        let second = primitive(&append, PrimitiveTy::I32);

        assert_eq!(first, second);
        assert_eq!(store.get(first), Some(&TyKind::Primitive(PrimitiveTy::I32)));
    }

    #[test]
    fn structural_equivalence_covers_non_leaf_runtime_types_across_stores() {
        let left = TypeStore::new().expect("create type store");
        let right = TypeStore::new().expect("create type store");
        let module_id = nia_ids::ModuleIdAllocator::new()
            .expect("create module ID allocator")
            .allocate()
            .expect("allocate module ID");
        let left_append = left.append_for_module(module_id);
        let right_append = right.append_for_module(module_id);
        let left_i32 = primitive(&left_append, PrimitiveTy::I32);
        let right_i32 = primitive(&right_append, PrimitiveTy::I32);
        let closure_id = nia_ids::ClosureId {
            owner: nia_ids::GlobalDefId {
                module_id,
                def_id: nia_ids::DefId(7),
            },
            ordinal: 2,
        };
        let left_types = [
            intern(&left_append, TyKind::ConstOnly),
            intern(
                &left_append,
                TyKind::Vector {
                    elem: PrimitiveTy::I32,
                    lanes: 4,
                },
            ),
            intern(
                &left_append,
                TyKind::ClosureState {
                    closure_id,
                    owner_instance: None,
                    captures: vec![left_i32],
                    params: vec![left_i32],
                    return_type: left_i32,
                },
            ),
        ];
        let right_types = [
            intern(&right_append, TyKind::ConstOnly),
            intern(
                &right_append,
                TyKind::Vector {
                    elem: PrimitiveTy::I32,
                    lanes: 4,
                },
            ),
            intern(
                &right_append,
                TyKind::ClosureState {
                    closure_id,
                    owner_instance: None,
                    captures: vec![right_i32],
                    params: vec![right_i32],
                    return_type: right_i32,
                },
            ),
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
    fn referenced_types_include_closure_owner_instance_arguments() {
        let store = TypeStore::new().expect("create type store");
        let modules = nia_ids::ModuleIdAllocator::new().expect("create module ID allocator");
        let module = modules.allocate().expect("allocate module ID");
        let append = store.append_for_module(module);
        let i32_ty = primitive(&append, PrimitiveTy::I32);
        let bool_ty = primitive(&append, PrimitiveTy::Bool);
        let closure = intern(
            &append,
            TyKind::ClosureState {
                closure_id: nia_ids::ClosureId {
                    owner: nia_ids::GlobalDefId {
                        module_id: module,
                        def_id: nia_ids::DefId(8),
                    },
                    ordinal: 0,
                },
                owner_instance: Some(ClosureOwnerInstance {
                    arg_module_id: module,
                    self_arg: Some(bool_ty),
                    args: vec![i32_ty],
                    const_args: vec![ConstGenericArg {
                        ty: bool_ty,
                        value: ConstGenericValue::Int(IntConst::unsigned(1)),
                    }],
                }),
                captures: Vec::new(),
                params: Vec::new(),
                return_type: i32_ty,
            },
        );
        let mut referenced = Vec::new();
        store
            .get(closure)
            .expect("closure type")
            .visit_referenced_types(|ty| referenced.push(ty));

        assert_eq!(referenced, vec![bool_ty, i32_ty, bool_ty, i32_ty]);
    }

    #[test]
    fn const_argument_type_operations_include_layout_operands() {
        let store = TypeStore::new().expect("create type store");
        let module = ModuleIdAllocator::new()
            .expect("create module allocator")
            .allocate()
            .expect("allocate module");
        let append = store.append_for_module(module);
        let usize_ty = primitive(&append, PrimitiveTy::Usize);
        let generic = intern(
            &append,
            TyKind::GenericParam(SymbolId::from_stable_hash(40)),
        );
        let parameter = SymbolId::from_stable_hash(41);
        let symbolic = ConstGenericArg {
            ty: usize_ty,
            value: ConstGenericValue::GenericParam(parameter),
        };
        let layout = ConstGenericArg {
            ty: usize_ty,
            value: ConstGenericValue::Layout {
                builtin: LayoutBuiltin::Size,
                ty: generic,
            },
        };

        assert!(!symbolic.any_type(|ty| ty == generic));
        assert!(symbolic.contains_generic(|_| false));
        assert!(layout.any_type(|ty| ty == generic));
        assert!(layout.contains_generic(|ty| ty == generic));
        assert!(symbolic.all_types(|ty| ty == usize_ty));
        assert!(!layout.all_types(|ty| ty == usize_ty));
        assert!(layout.all_types(|ty| ty == usize_ty || ty == generic));
        let mut referenced = Vec::new();
        layout.visit_referenced_types(|ty| referenced.push(ty));
        assert_eq!(referenced, vec![usize_ty, generic]);

        let substituted = substitute_const_generic_arg(
            &symbolic,
            &|name| (*name == parameter).then_some(layout.clone()),
            &|ty| Ok::<_, std::convert::Infallible>(ty),
        )
        .expect("substitute layout argument");
        assert_eq!(
            substituted.value,
            ConstGenericValue::Layout {
                builtin: LayoutBuiltin::Size,
                ty: generic,
            }
        );
    }

    #[test]
    fn default_equivalence_matches_integer_const_bits_across_stores() {
        let left = TypeStore::new().expect("create type store");
        let right = TypeStore::new().expect("create type store");
        let module_id = nia_ids::ModuleIdAllocator::new()
            .expect("create module ID allocator")
            .allocate()
            .expect("allocate module ID");
        let left_append = left.append_for_module(module_id);
        let right_append = right.append_for_module(module_id);
        let left_usize = primitive(&left_append, PrimitiveTy::Usize);
        let right_usize = primitive(&right_append, PrimitiveTy::Usize);
        let def_id = nia_ids::GlobalDefId {
            module_id,
            def_id: nia_ids::DefId(9),
        };
        let left_ty = intern(
            &left_append,
            TyKind::Nominal {
                def_id,
                args: Vec::new(),
                const_args: vec![ConstGenericArg {
                    ty: left_usize,
                    value: ConstGenericValue::Int(IntConst::signed(11)),
                }],
            },
        );
        let right_ty = intern(
            &right_append,
            TyKind::Nominal {
                def_id,
                args: Vec::new(),
                const_args: vec![ConstGenericArg {
                    ty: right_usize,
                    value: ConstGenericValue::Int(IntConst::unsigned(11)),
                }],
            },
        );
        let equivalence = DualStoreEquivalence {
            left: &left,
            right: &right,
        };

        assert!(equivalence.same_type_for_equiv(left_ty, right_ty));
    }

    #[test]
    fn associated_binding_equivalence_matches_values_with_duplicate_keys() {
        let left = TypeStore::new().expect("create type store");
        let right = TypeStore::new().expect("create type store");
        let module_id = nia_ids::ModuleIdAllocator::new()
            .expect("create module ID allocator")
            .allocate()
            .expect("allocate module ID");
        let left_append = left.append_for_module(module_id);
        let right_append = right.append_for_module(module_id);
        let left_i32 = primitive(&left_append, PrimitiveTy::I32);
        let left_bool = primitive(&left_append, PrimitiveTy::Bool);
        let right_i32 = primitive(&right_append, PrimitiveTy::I32);
        let right_bool = primitive(&right_append, PrimitiveTy::Bool);
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
        let left_ty = intern(
            &left_append,
            TyKind::TraitObject {
                is_readonly: false,
                trait_id,
                trait_args: Vec::new(),
                trait_const_args: Vec::new(),
                associated_type_bindings: vec![binding(left_i32), binding(left_bool)],
            },
        );
        let right_ty = intern(
            &right_append,
            TyKind::TraitObject {
                is_readonly: false,
                trait_id,
                trait_args: Vec::new(),
                trait_const_args: Vec::new(),
                associated_type_bindings: vec![binding(right_bool), binding(right_i32)],
            },
        );
        let equivalence = DualStoreEquivalence {
            left: &left,
            right: &right,
        };

        assert!(equivalence.same_type_for_equiv(left_ty, right_ty));
        let right_mismatch = intern(
            &right_append,
            TyKind::TraitObject {
                is_readonly: false,
                trait_id,
                trait_args: Vec::new(),
                trait_const_args: Vec::new(),
                associated_type_bindings: vec![binding(right_i32), binding(right_i32)],
            },
        );
        assert!(!equivalence.same_type_for_equiv(left_ty, right_mismatch));
    }

    #[test]
    fn tuple_identity_preserves_arity_and_element_order() {
        let store = TypeStore::new().expect("create type store");
        let module_id = nia_ids::ModuleIdAllocator::new()
            .expect("create module ID allocator")
            .allocate()
            .expect("allocate module ID");
        let append = store.append_for_module(module_id);
        let i32_ty = primitive(&append, PrimitiveTy::I32);
        let bool_ty = primitive(&append, PrimitiveTy::Bool);
        let unit = intern(&append, TyKind::Tuple(Vec::new()));
        let singleton = intern(&append, TyKind::Tuple(vec![i32_ty]));
        let pair = intern(&append, TyKind::Tuple(vec![i32_ty, bool_ty]));
        let reversed = intern(&append, TyKind::Tuple(vec![bool_ty, i32_ty]));

        assert!(store.get(unit).is_some_and(TyKind::is_unit));
        assert_ne!(singleton, i32_ty);
        assert_ne!(pair, reversed);
        assert_eq!(store.get(singleton), Some(&TyKind::Tuple(vec![i32_ty])));
    }

    #[test]
    fn primitive_ids_resolve_to_canonical_kinds() {
        let store = TypeStore::new().expect("create type store");
        let module_id = nia_ids::ModuleIdAllocator::new()
            .expect("create module ID allocator")
            .allocate()
            .expect("allocate module ID");
        let append = store.append_for_module(module_id);

        for primitive_ty in PrimitiveTy::ALL {
            let id = primitive(&append, primitive_ty);
            assert_eq!(store.get(id), Some(&TyKind::Primitive(primitive_ty)));
        }
    }

    #[test]
    fn type_store_identity_rejects_foreign_session_handles() {
        let first = TypeStore::new().expect("create type store");
        let second = TypeStore::new().expect("create type store");
        let module_id = nia_ids::ModuleIdAllocator::new()
            .expect("create module ID allocator")
            .allocate()
            .expect("allocate module ID");
        let first_i32 = first
            .append_for_module(module_id)
            .primitive(PrimitiveTy::I32)
            .expect("intern first i32 type");
        let second_i32 = second
            .append_for_module(module_id)
            .primitive(PrimitiveTy::I32)
            .expect("intern second i32 type");

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
        let store = TypeStore::new().expect("create type store");
        let module_ids = nia_ids::ModuleIdAllocator::new().expect("create module ID allocator");
        let first = store.append_for_module(module_ids.allocate().expect("allocate module ID"));
        let second = store.append_for_module(module_ids.allocate().expect("allocate module ID"));
        let first_elem = primitive(&first, PrimitiveTy::U32);
        let second_elem = primitive(&second, PrimitiveTy::U32);
        let first_pointer = intern(
            &first,
            TyKind::Pointer {
                is_readonly: true,
                elem: first_elem,
            },
        );
        let second_pointer = intern(
            &second,
            TyKind::Pointer {
                is_readonly: true,
                elem: second_elem,
            },
        );

        assert_eq!(first_elem, second_elem);
        assert_eq!(first_pointer, second_pointer);
    }

    #[test]
    fn pointers_to_callable_pointees_canonicalize_to_callable_views() {
        let store = TypeStore::new().expect("create type store");
        let module_id = nia_ids::ModuleIdAllocator::new()
            .expect("create module ID allocator")
            .allocate()
            .expect("allocate module ID");
        let append = store.append_for_module(module_id);
        let i32_ty = primitive(&append, PrimitiveTy::I32);
        let pointee = intern(
            &append,
            TyKind::CallablePointee {
                params: vec![i32_ty],
                return_type: i32_ty,
            },
        );
        let view = intern(
            &append,
            TyKind::Pointer {
                is_readonly: true,
                elem: pointee,
            },
        );

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
        let store = TypeStore::new().expect("create type store");
        let module_id = nia_ids::ModuleIdAllocator::new()
            .expect("create module ID allocator")
            .allocate()
            .expect("allocate module ID");
        let usize_ty = store
            .append_for_module(module_id)
            .primitive(PrimitiveTy::Usize)
            .expect("intern usize type");
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
    fn interning_rejects_foreign_session_type_dependencies() {
        let local = TypeStore::new().expect("create type store");
        let foreign = TypeStore::new().expect("create type store");
        let module_ids = nia_ids::ModuleIdAllocator::new().expect("create module ID allocator");
        let foreign_ty = foreign
            .append_for_module(module_ids.allocate().expect("allocate module ID"))
            .primitive(PrimitiveTy::U32)
            .expect("intern foreign type");

        let error = local
            .append_for_module(module_ids.allocate().expect("allocate module ID"))
            .intern(TyKind::Pointer {
                is_readonly: true,
                elem: foreign_ty,
            })
            .expect_err("foreign session dependency must be rejected");
        assert!(error.message.contains("outside its session type store"));
        let local_module = module_ids.allocate().expect("allocate module ID");
        let append = local.append_for_module(local_module);
        let usize_ty = primitive(&append, PrimitiveTy::Usize);
        let error = append
            .intern(TyKind::Nominal {
                def_id: GlobalDefId {
                    module_id: local_module,
                    def_id: nia_ids::DefId(1),
                },
                args: Vec::new(),
                const_args: vec![ConstGenericArg {
                    ty: usize_ty,
                    value: ConstGenericValue::Layout {
                        builtin: LayoutBuiltin::Size,
                        ty: foreign_ty,
                    },
                }],
            })
            .expect_err("foreign layout operand must be rejected even with a local carrier type");
        assert!(error.message.contains("outside its session type store"));
    }

    #[test]
    fn interning_accepts_same_session_dependencies_from_another_module() {
        let store = TypeStore::new().expect("create type store");
        let module_ids = nia_ids::ModuleIdAllocator::new().expect("create module ID allocator");
        let foreign_module_id = module_ids.allocate().expect("allocate module ID");
        let local_module_id = module_ids.allocate().expect("allocate module ID");
        let foreign = store
            .append_for_module(foreign_module_id)
            .intern(TyKind::Nominal {
                def_id: GlobalDefId {
                    module_id: foreign_module_id,
                    def_id: nia_ids::DefId(1),
                },
                args: Vec::new(),
                const_args: Vec::new(),
            })
            .expect("intern foreign-module nominal type");
        let pointer = store
            .append_for_module(local_module_id)
            .intern(TyKind::Pointer {
                is_readonly: true,
                elem: foreign,
            })
            .expect("intern pointer to same-session type");

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
    fn integer_casts_preserve_values_and_sign_extend_narrow_results() {
        for (ty, width) in [
            (PrimitiveTy::I8, 8),
            (PrimitiveTy::I16, 16),
            (PrimitiveTy::I32, 32),
            (PrimitiveTy::I64, 64),
            (PrimitiveTy::I128, 128),
            (PrimitiveTy::Isize, 32),
            (PrimitiveTy::Isize, 64),
        ] {
            let min = if width == 128 {
                i128::MIN
            } else {
                -(1_i128 << (width - 1))
            };
            let max = if width == 128 {
                i128::MAX
            } else {
                (1_i128 << (width - 1)) - 1
            };
            for value in [min, -1, 0, 1, max] {
                let value = IntConst::signed(value);
                let cast = value
                    .cast_to_primitive_int(ty, width)
                    .expect("integer cast");
                assert_eq!(cast, value, "{ty:?}/{width}");
                assert!(cast.fits_primitive_int(ty, width));
            }
            let raw_sign = IntConst::unsigned(1_u128 << (width - 1));
            assert_eq!(
                raw_sign.cast_to_primitive_int(ty, width),
                Some(IntConst::signed(min))
            );
        }
        assert_eq!(
            IntConst::unsigned(255).cast_to_primitive_int(PrimitiveTy::I8, 64),
            Some(IntConst::signed(-1))
        );
        assert_eq!(
            IntConst::signed(-1).cast_to_primitive_int(PrimitiveTy::U8, 64),
            Some(IntConst::unsigned(255))
        );
        for width in [0, 129] {
            assert_eq!(
                IntConst::signed(-1).cast_to_primitive_int(PrimitiveTy::Isize, width),
                None
            );
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
        assert_eq!(
            PrimitiveTy::from_known_symbol(nia_symbol::known::USIZE),
            Some(PrimitiveTy::Usize)
        );
        assert_eq!(PrimitiveTy::from_known_symbol(nia_symbol::known::LEN), None);
    }
}
