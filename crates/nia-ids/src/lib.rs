// SPDX-License-Identifier: GPL-3.0-or-later
//! Session-local semantic handles and the canonical builtin registry.
//!
//! Module and type handles carry an owner so values from independent compiler
//! sessions cannot alias. They are not persistence identities: persisted
//! products remap stable module/source keys into fresh handles. Builtin enums,
//! names, descriptors, and exhaustive `ALL` lists live here as one registry so
//! resolution, checking, const evaluation, and cache codecs share one domain.

#[derive(Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
/// Session-local identity of one loaded module.
///
/// The owner and generation fields prevent handles from independent compiler
/// sessions or allocator lifetimes from comparing equal by local index alone.
pub struct ModuleId {
    owner: u32,
    index: u32,
    generation: u32,
}

impl std::fmt::Debug for ModuleId {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_tuple("ModuleId")
            .field(&self.index)
            .finish()
    }
}

impl ModuleId {
    /// Returns the index assigned by this module's allocator.
    pub const fn local_index(self) -> u32 {
        self.index
    }
}

#[derive(Debug, Clone)]
/// Allocates module identities within one compiler session owner.
pub struct ModuleIdAllocator {
    owner: u32,
    next_index: u32,
}

impl ModuleIdAllocator {
    /// Creates an allocator with a fresh session owner identity.
    pub fn new() -> Self {
        use std::sync::atomic::{AtomicU32, Ordering};

        static NEXT_OWNER: AtomicU32 = AtomicU32::new(1);
        let owner = NEXT_OWNER
            .fetch_update(Ordering::Relaxed, Ordering::Relaxed, |owner| {
                owner.checked_add(1)
            })
            .expect("module owner identity space exhausted");
        Self {
            owner,
            next_index: 0,
        }
    }

    /// Allocates the next module identity from this session.
    pub fn allocate(&mut self) -> ModuleId {
        use std::sync::atomic::{AtomicU32, Ordering};

        static NEXT_GENERATION: AtomicU32 = AtomicU32::new(1);
        let index = self.next_index;
        self.next_index = self
            .next_index
            .checked_add(1)
            .expect("module identity space exhausted");
        let generation = NEXT_GENERATION
            .fetch_update(Ordering::Relaxed, Ordering::Relaxed, |generation| {
                generation.checked_add(1)
            })
            .expect("module generation identity space exhausted");
        ModuleId {
            owner: self.owner,
            index,
            generation,
        }
    }
}

impl Default for ModuleIdAllocator {
    fn default() -> Self {
        Self::new()
    }
}

impl PartialEq for ModuleIdAllocator {
    fn eq(&self, other: &Self) -> bool {
        self.owner == other.owner
    }
}

impl Eq for ModuleIdAllocator {}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
/// Identity of a definition within its owning module.
pub struct DefId(pub u64);

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
/// Definition identity qualified by its owning module.
pub struct GlobalDefId {
    /// Module containing the definition.
    pub module_id: ModuleId,
    /// Definition slot within the module.
    pub def_id: DefId,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
/// Identity of a trait implementation in semantic products.
pub struct TraitImplId(pub u64);

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
/// Identity of one lowered const expression within a module.
pub struct ConstExprId(pub u32);

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
/// Const-expression identity qualified by its owning module.
pub struct GlobalConstExprId {
    /// Module containing the expression.
    pub module_id: ModuleId,
    /// Expression slot within that module.
    pub const_expr_id: ConstExprId,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
/// Identity of a local binding within one resolved body.
pub struct LocalId(pub u32);

/// Stable semantic identity for an anonymous closure state within a function.
///
/// The owner identifies the containing source function and the ordinal is
/// assigned by deterministic body traversal. It is intentionally distinct
/// from a function definition id: a closure has state and an eventual entry
/// function, but is not itself a source-level item.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct ClosureId {
    /// Function that owns the closure state.
    pub owner: GlobalDefId,
    /// Deterministic ordinal in the owner's body traversal.
    pub ordinal: u32,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
/// Slot index in a session-local type store.
pub struct TypeStoreIndex(u32);

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
/// Identity of a type store session.
pub struct TypeStoreId(u32);

impl TypeStoreId {
    #[doc(hidden)]
    pub fn fresh() -> Self {
        use std::sync::atomic::{AtomicU32, Ordering};

        static NEXT_TYPE_STORE_ID: AtomicU32 = AtomicU32::new(1);
        let index = NEXT_TYPE_STORE_ID
            .fetch_update(Ordering::Relaxed, Ordering::Relaxed, |index| {
                index.checked_add(1)
            })
            .expect("type store identity space exhausted");
        Self(index)
    }
}

impl TypeStoreIndex {
    #[doc(hidden)]
    pub const fn from_store_index(index: u32) -> Self {
        Self(index)
    }

    /// Returns the numeric slot index.
    pub const fn index(self) -> u32 {
        self.0
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
/// Type identity qualified by the store that owns its slot.
pub struct InternedTyId {
    /// Owning type-store session.
    pub store_id: TypeStoreId,
    /// Slot within the owning store.
    pub index: TypeStoreIndex,
}

impl InternedTyId {
    /// Creates a qualified type identity from a store and slot.
    pub const fn new(store_id: TypeStoreId, index: TypeStoreIndex) -> Self {
        Self { store_id, index }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default)]
/// Visibility level assigned to a source declaration.
pub enum Visibility {
    #[default]
    /// Visible only in the declaring scope.
    Private,
    /// Visible to the declaring module's supermodule.
    PublicSuper,
    /// Visible throughout the declaring package.
    PublicPkg,
    /// Visible to importing packages.
    Public,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
/// Identity of either a source-defined or builtin trait.
pub enum TraitId {
    /// Source trait definition.
    Source(GlobalDefId),
    /// Compiler-provided builtin trait.
    Builtin(BuiltinTrait),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
/// Compiler-owned type names used by inline-assembly interfaces.
pub enum BuiltinType {
    /// Assembly configuration record type.
    AsmConfig,
    /// Assembly input record type.
    AsmInputs,
    /// Assembly output record type.
    AsmOutputs,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
/// Primitive type anchors recognized by semantic type lookup.
pub enum BuiltinTypeAnchor {
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
    /// Signed pointer-sized integer.
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
    /// Unsigned pointer-sized integer.
    Usize,
    /// 32-bit floating-point value.
    F32,
    /// 64-bit floating-point value.
    F64,
    /// Boolean value.
    Bool,
    /// Unicode scalar value.
    Char,
    /// Never-returning type.
    Never,
}

impl BuiltinType {
    /// All builtin assembly record types in stable order.
    pub const ALL: [Self; 3] = [Self::AsmConfig, Self::AsmInputs, Self::AsmOutputs];

    /// Parses a source-level builtin type name.
    pub fn from_name(name: &str) -> Option<Self> {
        match name {
            "AsmConfig" => Some(Self::AsmConfig),
            "AsmInputs" => Some(Self::AsmInputs),
            "AsmOutputs" => Some(Self::AsmOutputs),
            _ => None,
        }
    }

    /// Returns the canonical source-level name.
    pub fn name(self) -> &'static str {
        match self {
            Self::AsmConfig => "AsmConfig",
            Self::AsmInputs => "AsmInputs",
            Self::AsmOutputs => "AsmOutputs",
        }
    }
}

impl BuiltinTypeAnchor {
    /// All primitive anchors in canonical registry order.
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

    /// Parses a primitive type spelling.
    pub fn from_name(name: &str) -> Option<Self> {
        match name {
            "i8" => Some(Self::I8),
            "i16" => Some(Self::I16),
            "i32" => Some(Self::I32),
            "i64" => Some(Self::I64),
            "i128" => Some(Self::I128),
            "isize" => Some(Self::Isize),
            "u8" => Some(Self::U8),
            "u16" => Some(Self::U16),
            "u32" => Some(Self::U32),
            "u64" => Some(Self::U64),
            "u128" => Some(Self::U128),
            "usize" => Some(Self::Usize),
            "f32" => Some(Self::F32),
            "f64" => Some(Self::F64),
            "bool" => Some(Self::Bool),
            "char" => Some(Self::Char),
            "never" => Some(Self::Never),
            _ => None,
        }
    }

    /// Returns the canonical primitive type spelling.
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
            Self::Never => "never",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
/// Builtin operator, memory, iteration, and marker trait identity.
pub enum BuiltinTrait {
    /// Addition operator.
    Add,
    /// Subtraction operator.
    Sub,
    /// Multiplication operator.
    Mul,
    /// Division operator.
    Div,
    /// Remainder operator.
    Rem,
    /// Numeric negation operator.
    Neg,
    /// Boolean negation operator.
    Not,
    /// Bitwise complement operator.
    BitNot,
    /// Bitwise AND operator.
    BitAnd,
    /// Bitwise OR operator.
    BitOr,
    /// Bitwise XOR operator.
    BitXor,
    /// Left-shift operator.
    Shl,
    /// Right-shift operator.
    Shr,
    /// Equality comparison operator.
    Eq,
    /// Ordering comparison operator family.
    Ord,
    /// Sized-type marker trait.
    Sized,
    /// Unsized-type marker trait.
    Unsized,
    /// Immutable dereference trait.
    Deref,
    /// Mutable dereference trait.
    DerefMut,
    /// Immutable indexing trait.
    Index,
    /// Mutable indexing trait.
    IndexMut,
    /// Immutable slicing trait.
    Slice,
    /// Mutable slicing trait.
    SliceMut,
    /// Iterable collection protocol trait.
    Iterable,
    /// Iterator protocol trait.
    Iterator,
    /// SIMD vector trait.
    Simd,
    /// SIMD mask trait.
    SimdMask,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
/// Builtin value-producing operation identity.
pub enum ValueBuiltin {
    /// Constructs an error value.
    Error,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
/// Builtin compile-time target configuration value.
pub enum BuiltinConstValue {
    /// Target architecture name.
    TargetArch,
    /// Target vendor name.
    TargetVendor,
    /// Target operating-system name.
    TargetOs,
    /// Target environment name.
    TargetEnv,
    /// Target ABI name.
    TargetAbi,
    /// Target endianness name.
    TargetEndian,
    /// Target pointer width in bits.
    TargetPointerWidth,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
/// Builtin function and memory-operation identity.
pub enum BuiltinFunction {
    /// Constructs a compile-time error value.
    ConstError,
    /// Triggers a compile-time trap.
    Trap,
    /// Computes the size of a type.
    SizeOf,
    /// Computes the alignment of a type.
    AlignOf,
    /// Computes a field offset.
    Offset,
    /// Inline assembly operation.
    Asm,
    /// Copies a memory range.
    MemCopy,
    /// Moves a memory range.
    MemMove,
    /// Fills a memory range.
    MemSet,
    /// Performs an unaligned load.
    LoadUnaligned,
    /// Constructs a SIMD value from one lane.
    Splat,
    /// Extracts one SIMD lane.
    Extract,
    /// Inserts one SIMD lane.
    Insert,
    /// Builds a SIMD mask from a value.
    Bitmask,
    /// Counts trailing zero bits.
    Ctz,
    /// Counts leading zero bits.
    Clz,
    /// Counts set bits.
    Popcount,
    /// Performs an atomic load.
    AtomicLoad,
    /// Performs an atomic store.
    AtomicStore,
    /// Performs an atomic read-modify-write operation.
    AtomicRmw,
    /// Performs a strong compare-and-exchange.
    CmpxchgStrong,
    /// Performs a weak compare-and-exchange.
    CmpxchgWeak,
    /// Issues an atomic fence.
    Fence,
    /// Embeds an external resource.
    Embed,
    /// Converts a Unicode scalar value.
    CharFromU32,
    /// Returns the length of a slice.
    SliceLen,
}

impl BuiltinFunction {
    /// All builtin functions in canonical registry order.
    pub const ALL: [Self; 26] = [
        Self::ConstError,
        Self::Trap,
        Self::SizeOf,
        Self::AlignOf,
        Self::Offset,
        Self::Asm,
        Self::MemCopy,
        Self::MemMove,
        Self::MemSet,
        Self::LoadUnaligned,
        Self::Splat,
        Self::Extract,
        Self::Insert,
        Self::Bitmask,
        Self::Ctz,
        Self::Clz,
        Self::Popcount,
        Self::AtomicLoad,
        Self::AtomicStore,
        Self::AtomicRmw,
        Self::CmpxchgStrong,
        Self::CmpxchgWeak,
        Self::Fence,
        Self::Embed,
        Self::CharFromU32,
        Self::SliceLen,
    ];

    /// Parses the stable operation name used by `@[builtin("...")]`.
    pub fn from_name(name: &str) -> Option<Self> {
        match name {
            "error" => Some(Self::ConstError),
            "trap" => Some(Self::Trap),
            "size" => Some(Self::SizeOf),
            "align" => Some(Self::AlignOf),
            "offset" => Some(Self::Offset),
            "asm" => Some(Self::Asm),
            "memcpy" => Some(Self::MemCopy),
            "memmove" => Some(Self::MemMove),
            "memset" => Some(Self::MemSet),
            "load_unaligned" => Some(Self::LoadUnaligned),
            "splat" => Some(Self::Splat),
            "extract" => Some(Self::Extract),
            "insert" => Some(Self::Insert),
            "bitmask" => Some(Self::Bitmask),
            "ctz" => Some(Self::Ctz),
            "clz" => Some(Self::Clz),
            "popcount" => Some(Self::Popcount),
            "atomic_load" => Some(Self::AtomicLoad),
            "atomic_store" => Some(Self::AtomicStore),
            "atomic_rmw" => Some(Self::AtomicRmw),
            "cmpxchg_strong" => Some(Self::CmpxchgStrong),
            "cmpxchg_weak" => Some(Self::CmpxchgWeak),
            "fence" => Some(Self::Fence),
            "embed" => Some(Self::Embed),
            "charFromU32" => Some(Self::CharFromU32),
            "sliceLen" => Some(Self::SliceLen),
            _ => None,
        }
    }

    /// Returns the stable operation name used by `@[builtin("...")]`.
    pub fn name(self) -> &'static str {
        match self {
            Self::ConstError => "error",
            Self::Trap => "trap",
            Self::SizeOf => "size",
            Self::AlignOf => "align",
            Self::Offset => "offset",
            Self::Asm => "asm",
            Self::MemCopy => "memcpy",
            Self::MemMove => "memmove",
            Self::MemSet => "memset",
            Self::LoadUnaligned => "load_unaligned",
            Self::Splat => "splat",
            Self::Extract => "extract",
            Self::Insert => "insert",
            Self::Bitmask => "bitmask",
            Self::Ctz => "ctz",
            Self::Clz => "clz",
            Self::Popcount => "popcount",
            Self::AtomicLoad => "atomic_load",
            Self::AtomicStore => "atomic_store",
            Self::AtomicRmw => "atomic_rmw",
            Self::CmpxchgStrong => "cmpxchg_strong",
            Self::CmpxchgWeak => "cmpxchg_weak",
            Self::Fence => "fence",
            Self::Embed => "embed",
            Self::CharFromU32 => "charFromU32",
            Self::SliceLen => "sliceLen",
        }
    }

    /// Returns the public Nia spelling of this builtin function.
    pub fn source_name(self) -> &'static str {
        match self {
            Self::LoadUnaligned => "loadUnaligned",
            Self::AtomicLoad => "atomicLoad",
            Self::AtomicStore => "atomicStore",
            Self::AtomicRmw => "atomicRmw",
            Self::CmpxchgStrong => "cmpxchgStrong",
            Self::CmpxchgWeak => "cmpxchgWeak",
            _ => self.name(),
        }
    }

    /// Reports whether this operation is valid during const evaluation.
    pub const fn is_const_capable(self) -> bool {
        match self {
            Self::ConstError
            | Self::Trap
            | Self::SizeOf
            | Self::AlignOf
            | Self::Offset
            | Self::Embed
            | Self::CharFromU32
            | Self::SliceLen
            | Self::Splat
            | Self::Extract
            | Self::Insert
            | Self::Bitmask => true,
            Self::Asm
            | Self::MemCopy
            | Self::MemMove
            | Self::MemSet
            | Self::LoadUnaligned
            | Self::Ctz
            | Self::Clz
            | Self::Popcount
            | Self::AtomicLoad
            | Self::AtomicStore
            | Self::AtomicRmw
            | Self::CmpxchgStrong
            | Self::CmpxchgWeak
            | Self::Fence => false,
        }
    }
}

impl ValueBuiltin {
    /// All value-producing builtins in canonical order.
    pub const ALL: [Self; 1] = [Self::Error];

    /// Parses a canonical value-builtin name.
    pub fn from_name(name: &str) -> Option<Self> {
        match name {
            "error" => Some(Self::Error),
            _ => None,
        }
    }

    /// Returns the canonical source-level name.
    pub fn name(self) -> &'static str {
        match self {
            Self::Error => "error",
        }
    }
}

impl BuiltinConstValue {
    /// All target configuration values in canonical registry order.
    pub const ALL: [Self; 7] = [
        Self::TargetArch,
        Self::TargetVendor,
        Self::TargetOs,
        Self::TargetEnv,
        Self::TargetAbi,
        Self::TargetEndian,
        Self::TargetPointerWidth,
    ];

    /// Parses a dotted target configuration name.
    pub fn from_name(name: &str) -> Option<Self> {
        match name {
            "target.arch" => Some(Self::TargetArch),
            "target.vendor" => Some(Self::TargetVendor),
            "target.os" => Some(Self::TargetOs),
            "target.env" => Some(Self::TargetEnv),
            "target.abi" => Some(Self::TargetAbi),
            "target.endian" => Some(Self::TargetEndian),
            "target.pointer_width" => Some(Self::TargetPointerWidth),
            _ => None,
        }
    }

    /// Returns the canonical dotted configuration name.
    pub fn name(self) -> &'static str {
        match self {
            Self::TargetArch => "target.arch",
            Self::TargetVendor => "target.vendor",
            Self::TargetOs => "target.os",
            Self::TargetEnv => "target.env",
            Self::TargetAbi => "target.abi",
            Self::TargetEndian => "target.endian",
            Self::TargetPointerWidth => "target.pointer_width",
        }
    }

    /// Returns the final item component used by field lookup.
    pub fn item_name(self) -> &'static str {
        match self {
            Self::TargetArch => "arch",
            Self::TargetVendor => "vendor",
            Self::TargetOs => "os",
            Self::TargetEnv => "env",
            Self::TargetAbi => "abi",
            Self::TargetEndian => "endian",
            Self::TargetPointerWidth => "pointer_width",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
/// Type-layout query builtin identity.
pub enum LayoutBuiltin {
    /// Queries the size of a type.
    Size,
    /// Queries the alignment of a type.
    Align,
}

impl LayoutBuiltin {
    /// All layout builtins in canonical order.
    pub const ALL: [Self; 2] = [Self::Size, Self::Align];

    /// Parses a canonical layout builtin name.
    pub fn from_name(name: &str) -> Option<Self> {
        match name {
            "size" => Some(Self::Size),
            "align" => Some(Self::Align),
            _ => None,
        }
    }

    /// Returns the canonical source-level name.
    pub fn name(self) -> &'static str {
        match self {
            Self::Size => "size",
            Self::Align => "align",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
/// Required method identity for a builtin trait.
pub enum BuiltinTraitMethod {
    /// Addition method.
    Add,
    /// Subtraction method.
    Sub,
    /// Multiplication method.
    Mul,
    /// Division method.
    Div,
    /// Remainder method.
    Rem,
    /// Numeric negation method.
    Neg,
    /// Boolean negation method.
    Not,
    /// Bitwise complement method.
    BitNot,
    /// Bitwise AND method.
    BitAnd,
    /// Bitwise OR method.
    BitOr,
    /// Bitwise XOR method.
    BitXor,
    /// Left-shift method.
    Shl,
    /// Right-shift method.
    Shr,
    /// Equality method.
    Eq,
    /// Inequality method.
    Ne,
    /// Less-than comparison method.
    Lt,
    /// Less-than-or-equal comparison method.
    Le,
    /// Greater-than comparison method.
    Gt,
    /// Greater-than-or-equal comparison method.
    Ge,
    /// Immutable dereference method.
    Deref,
    /// Mutable dereference method.
    DerefMut,
    /// Immutable indexing method.
    Index,
    /// Mutable indexing method.
    IndexMut,
    /// Immutable slicing method.
    Slice,
    /// Mutable slicing method.
    SliceMut,
    /// Iterable protocol method.
    IterableIter,
    /// Iterator protocol method.
    IteratorNext,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
/// Receiver passing mode required by a builtin trait method.
pub enum ReceiverKind {
    /// Read-only reference receiver.
    RefReadOnly,
    /// Writable reference receiver.
    Ref,
    /// By-value receiver.
    Value,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
/// Associated type identity declared by a builtin trait.
pub enum BuiltinAssociatedType {
    /// Operator result type.
    Output,
    /// Dereference target type.
    Target,
    /// Iterator item type.
    Item,
    /// Iterable iterator type.
    Iter,
    /// SIMD lane type.
    Lane,
}

impl BuiltinAssociatedType {
    /// All builtin associated types in canonical order.
    pub const ALL: [Self; 5] = [
        Self::Output,
        Self::Target,
        Self::Item,
        Self::Iter,
        Self::Lane,
    ];

    /// Parses a canonical associated-type name.
    pub fn from_name(name: &str) -> Option<Self> {
        match name {
            "Output" => Some(Self::Output),
            "Target" => Some(Self::Target),
            "Item" => Some(Self::Item),
            "Iter" => Some(Self::Iter),
            "Lane" => Some(Self::Lane),
            _ => None,
        }
    }

    /// Returns the canonical source-level name.
    pub fn name(self) -> &'static str {
        match self {
            Self::Output => "Output",
            Self::Target => "Target",
            Self::Item => "Item",
            Self::Iter => "Iter",
            Self::Lane => "Lane",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
/// Associated const identity declared by a builtin trait.
pub enum BuiltinAssociatedConst {
    /// SIMD lane count constant.
    Lanes,
}

impl BuiltinAssociatedConst {
    /// All builtin associated consts in canonical order.
    pub const ALL: [Self; 1] = [Self::Lanes];

    /// Parses a canonical associated-const name.
    pub fn from_name(name: &str) -> Option<Self> {
        match name {
            "Lanes" => Some(Self::Lanes),
            _ => None,
        }
    }

    /// Returns the canonical source-level name.
    pub fn name(self) -> &'static str {
        match self {
            Self::Lanes => "Lanes",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
/// One builtin supertrait edge and its generic-argument policy.
pub struct BuiltinSupertrait {
    /// Supertrait identity.
    pub trait_id: BuiltinTrait,
    /// Whether the edge preserves the source trait arguments.
    pub preserves_trait_args: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
/// Semantic signature metadata for one builtin trait method.
pub struct BuiltinTraitMethodDescriptor {
    /// Canonical method name.
    pub name: &'static str,
    /// Owning builtin trait.
    pub trait_id: BuiltinTrait,
    /// Total declared parameter count, including the receiver.
    pub param_count: usize,
    /// Ordinary receiver passing mode.
    pub receiver_kind: ReceiverKind,
    /// Receiver mode when the method is used as a place operation.
    pub place_receiver_kind: Option<ReceiverKind>,
    /// Whether the method represents a value operator.
    pub is_value_operator: bool,
    /// Whether the method operates on a writable place.
    pub is_place_method: bool,
}

impl BuiltinTraitMethod {
    /// All builtin trait methods in canonical registry order.
    pub const ALL: [Self; 27] = [
        Self::Add,
        Self::Sub,
        Self::Mul,
        Self::Div,
        Self::Rem,
        Self::Neg,
        Self::Not,
        Self::BitNot,
        Self::BitAnd,
        Self::BitOr,
        Self::BitXor,
        Self::Shl,
        Self::Shr,
        Self::Eq,
        Self::Ne,
        Self::Lt,
        Self::Le,
        Self::Gt,
        Self::Ge,
        Self::Deref,
        Self::DerefMut,
        Self::Index,
        Self::IndexMut,
        Self::Slice,
        Self::SliceMut,
        Self::IterableIter,
        Self::IteratorNext,
    ];

    const DESCRIPTORS: &'static [(Self, BuiltinTraitMethodDescriptor)] = &[
        (
            Self::Add,
            BuiltinTraitMethodDescriptor::value_operator("add", BuiltinTrait::Add, 2),
        ),
        (
            Self::Sub,
            BuiltinTraitMethodDescriptor::value_operator("sub", BuiltinTrait::Sub, 2),
        ),
        (
            Self::Mul,
            BuiltinTraitMethodDescriptor::value_operator("mul", BuiltinTrait::Mul, 2),
        ),
        (
            Self::Div,
            BuiltinTraitMethodDescriptor::value_operator("div", BuiltinTrait::Div, 2),
        ),
        (
            Self::Rem,
            BuiltinTraitMethodDescriptor::value_operator("rem", BuiltinTrait::Rem, 2),
        ),
        (
            Self::Neg,
            BuiltinTraitMethodDescriptor::value_operator("neg", BuiltinTrait::Neg, 1),
        ),
        (
            Self::Not,
            BuiltinTraitMethodDescriptor::value_operator("logicalNot", BuiltinTrait::Not, 1),
        ),
        (
            Self::BitNot,
            BuiltinTraitMethodDescriptor::value_operator("bitNot", BuiltinTrait::BitNot, 1),
        ),
        (
            Self::BitAnd,
            BuiltinTraitMethodDescriptor::value_operator("bitAnd", BuiltinTrait::BitAnd, 2),
        ),
        (
            Self::BitOr,
            BuiltinTraitMethodDescriptor::value_operator("bitOr", BuiltinTrait::BitOr, 2),
        ),
        (
            Self::BitXor,
            BuiltinTraitMethodDescriptor::value_operator("bitXor", BuiltinTrait::BitXor, 2),
        ),
        (
            Self::Shl,
            BuiltinTraitMethodDescriptor::value_operator("shl", BuiltinTrait::Shl, 2),
        ),
        (
            Self::Shr,
            BuiltinTraitMethodDescriptor::value_operator("shr", BuiltinTrait::Shr, 2),
        ),
        (
            Self::Eq,
            BuiltinTraitMethodDescriptor::value_operator("eq", BuiltinTrait::Eq, 2),
        ),
        (
            Self::Ne,
            BuiltinTraitMethodDescriptor::value_operator("ne", BuiltinTrait::Eq, 2),
        ),
        (
            Self::Lt,
            BuiltinTraitMethodDescriptor::value_operator("lt", BuiltinTrait::Ord, 2),
        ),
        (
            Self::Le,
            BuiltinTraitMethodDescriptor::value_operator("le", BuiltinTrait::Ord, 2),
        ),
        (
            Self::Gt,
            BuiltinTraitMethodDescriptor::value_operator("gt", BuiltinTrait::Ord, 2),
        ),
        (
            Self::Ge,
            BuiltinTraitMethodDescriptor::value_operator("ge", BuiltinTrait::Ord, 2),
        ),
        (
            Self::Deref,
            BuiltinTraitMethodDescriptor::place(
                "deref",
                BuiltinTrait::Deref,
                1,
                ReceiverKind::RefReadOnly,
                Some(ReceiverKind::RefReadOnly),
            ),
        ),
        (
            Self::DerefMut,
            BuiltinTraitMethodDescriptor::place(
                "derefMut",
                BuiltinTrait::DerefMut,
                1,
                ReceiverKind::Value,
                Some(ReceiverKind::Ref),
            ),
        ),
        (
            Self::Index,
            BuiltinTraitMethodDescriptor::place(
                "index",
                BuiltinTrait::Index,
                2,
                ReceiverKind::RefReadOnly,
                Some(ReceiverKind::RefReadOnly),
            ),
        ),
        (
            Self::IndexMut,
            BuiltinTraitMethodDescriptor::place(
                "indexMut",
                BuiltinTrait::IndexMut,
                2,
                ReceiverKind::Value,
                Some(ReceiverKind::Ref),
            ),
        ),
        (
            Self::Slice,
            BuiltinTraitMethodDescriptor::place(
                "slice",
                BuiltinTrait::Slice,
                2,
                ReceiverKind::RefReadOnly,
                None,
            ),
        ),
        (
            Self::SliceMut,
            BuiltinTraitMethodDescriptor::place(
                "sliceMut",
                BuiltinTrait::SliceMut,
                2,
                ReceiverKind::Ref,
                None,
            ),
        ),
        (
            Self::IterableIter,
            BuiltinTraitMethodDescriptor::method(
                "iter",
                BuiltinTrait::Iterable,
                1,
                ReceiverKind::RefReadOnly,
            ),
        ),
        (
            Self::IteratorNext,
            BuiltinTraitMethodDescriptor::place(
                "next",
                BuiltinTrait::Iterator,
                1,
                ReceiverKind::Ref,
                None,
            ),
        ),
    ];

    /// Returns the complete signature descriptor for this method.
    pub fn descriptor(self) -> BuiltinTraitMethodDescriptor {
        Self::DESCRIPTORS
            .iter()
            .find_map(|(method, descriptor)| (*method == self).then_some(*descriptor))
            .expect("missing builtin trait method descriptor")
    }

    /// Parses a canonical trait-method name.
    pub fn from_name(name: &str) -> Option<Self> {
        Self::DESCRIPTORS
            .iter()
            .find_map(|(method, descriptor)| (descriptor.name == name).then_some(*method))
    }

    /// Returns the canonical method name.
    pub fn name(self) -> &'static str {
        self.descriptor().name
    }

    /// Returns the total declared parameter count, including the receiver.
    pub fn param_count(self) -> usize {
        self.descriptor().param_count
    }

    /// Returns the ordinary receiver passing mode.
    pub fn receiver_kind(self) -> ReceiverKind {
        self.descriptor().receiver_kind
    }

    /// Returns the receiver mode used for place operations, when applicable.
    pub fn place_receiver_kind(self) -> Option<ReceiverKind> {
        self.descriptor().place_receiver_kind
    }

    /// Returns the owning builtin trait identity.
    pub fn trait_id(self) -> BuiltinTrait {
        self.descriptor().trait_id
    }

    /// Reports whether this method is a value-level operator.
    pub fn is_value_operator(self) -> bool {
        self.descriptor().is_value_operator
    }

    /// Reports whether this method operates on a writable place.
    pub fn is_place_method(self) -> bool {
        self.descriptor().is_place_method
    }

    /// Whether the const evaluator has a representation for this intrinsic.
    ///
    /// The ordinary body checker still owns the type and trait obligations;
    /// this table only describes the second-stage operation capability.
    pub fn is_const_capable(self) -> bool {
        match self {
            Self::Add
            | Self::Sub
            | Self::Mul
            | Self::Div
            | Self::Rem
            | Self::Neg
            | Self::Not
            | Self::BitNot
            | Self::BitAnd
            | Self::BitOr
            | Self::BitXor
            | Self::Shl
            | Self::Shr
            | Self::Eq
            | Self::Ne
            | Self::Lt
            | Self::Le
            | Self::Gt
            | Self::Ge
            | Self::Deref
            | Self::Index
            | Self::IndexMut
            | Self::Slice
            | Self::SliceMut
            | Self::IterableIter
            | Self::IteratorNext => true,
            Self::DerefMut => false,
        }
    }
}

impl BuiltinTraitMethodDescriptor {
    const fn value_operator(
        name: &'static str,
        trait_id: BuiltinTrait,
        param_count: usize,
    ) -> Self {
        Self {
            name,
            trait_id,
            param_count,
            receiver_kind: ReceiverKind::Value,
            place_receiver_kind: None,
            is_value_operator: true,
            is_place_method: false,
        }
    }

    const fn place(
        name: &'static str,
        trait_id: BuiltinTrait,
        param_count: usize,
        receiver_kind: ReceiverKind,
        place_receiver_kind: Option<ReceiverKind>,
    ) -> Self {
        Self {
            name,
            trait_id,
            param_count,
            receiver_kind,
            place_receiver_kind,
            is_value_operator: false,
            is_place_method: true,
        }
    }

    const fn method(
        name: &'static str,
        trait_id: BuiltinTrait,
        param_count: usize,
        receiver_kind: ReceiverKind,
    ) -> Self {
        Self {
            name,
            trait_id,
            param_count,
            receiver_kind,
            place_receiver_kind: None,
            is_value_operator: false,
            is_place_method: false,
        }
    }
}

impl BuiltinTrait {
    /// Canonical name of the operator output associated type.
    pub const OUTPUT_ASSOC_TYPE: &'static str = "Output";
    /// Canonical name of the dereference target associated type.
    pub const TARGET_ASSOC_TYPE: &'static str = "Target";
    /// Canonical name of the iterator item associated type.
    pub const ITEM_ASSOC_TYPE: &'static str = "Item";
    /// Canonical name of the iterable iterator associated type.
    pub const ITER_ASSOC_TYPE: &'static str = "Iter";
    /// Canonical name of the SIMD lane associated type.
    pub const LANE_ASSOC_TYPE: &'static str = "Lane";
    /// Canonical name of the SIMD lane-count associated const.
    pub const LANES_ASSOC_CONST: &'static str = "Lanes";

    const ADD_METHODS: [BuiltinTraitMethod; 1] = [BuiltinTraitMethod::Add];
    const SUB_METHODS: [BuiltinTraitMethod; 1] = [BuiltinTraitMethod::Sub];
    const MUL_METHODS: [BuiltinTraitMethod; 1] = [BuiltinTraitMethod::Mul];
    const DIV_METHODS: [BuiltinTraitMethod; 1] = [BuiltinTraitMethod::Div];
    const REM_METHODS: [BuiltinTraitMethod; 1] = [BuiltinTraitMethod::Rem];
    const NEG_METHODS: [BuiltinTraitMethod; 1] = [BuiltinTraitMethod::Neg];
    const NOT_METHODS: [BuiltinTraitMethod; 1] = [BuiltinTraitMethod::Not];
    const BIT_NOT_METHODS: [BuiltinTraitMethod; 1] = [BuiltinTraitMethod::BitNot];
    const BIT_AND_METHODS: [BuiltinTraitMethod; 1] = [BuiltinTraitMethod::BitAnd];
    const BIT_OR_METHODS: [BuiltinTraitMethod; 1] = [BuiltinTraitMethod::BitOr];
    const BIT_XOR_METHODS: [BuiltinTraitMethod; 1] = [BuiltinTraitMethod::BitXor];
    const SHL_METHODS: [BuiltinTraitMethod; 1] = [BuiltinTraitMethod::Shl];
    const SHR_METHODS: [BuiltinTraitMethod; 1] = [BuiltinTraitMethod::Shr];
    const EQ_METHODS: [BuiltinTraitMethod; 2] = [BuiltinTraitMethod::Eq, BuiltinTraitMethod::Ne];
    const ORD_METHODS: [BuiltinTraitMethod; 4] = [
        BuiltinTraitMethod::Lt,
        BuiltinTraitMethod::Le,
        BuiltinTraitMethod::Gt,
        BuiltinTraitMethod::Ge,
    ];
    const NO_METHODS: [BuiltinTraitMethod; 0] = [];
    const DEREF_METHODS: [BuiltinTraitMethod; 1] = [BuiltinTraitMethod::Deref];
    const DEREF_MUT_METHODS: [BuiltinTraitMethod; 1] = [BuiltinTraitMethod::DerefMut];
    const INDEX_METHODS: [BuiltinTraitMethod; 1] = [BuiltinTraitMethod::Index];
    const INDEX_MUT_METHODS: [BuiltinTraitMethod; 1] = [BuiltinTraitMethod::IndexMut];
    const SLICE_METHODS: [BuiltinTraitMethod; 1] = [BuiltinTraitMethod::Slice];
    const SLICE_MUT_METHODS: [BuiltinTraitMethod; 1] = [BuiltinTraitMethod::SliceMut];
    const ITERABLE_METHODS: [BuiltinTraitMethod; 1] = [BuiltinTraitMethod::IterableIter];
    const ITERATOR_METHODS: [BuiltinTraitMethod; 1] = [BuiltinTraitMethod::IteratorNext];
    const OUTPUT_ASSOC_TYPES: [BuiltinAssociatedType; 1] = [BuiltinAssociatedType::Output];
    const TARGET_ASSOC_TYPES: [BuiltinAssociatedType; 1] = [BuiltinAssociatedType::Target];
    const ITEM_ASSOC_TYPES: [BuiltinAssociatedType; 1] = [BuiltinAssociatedType::Item];
    const LANE_ASSOC_TYPES: [BuiltinAssociatedType; 1] = [BuiltinAssociatedType::Lane];
    const ITERABLE_ASSOC_TYPES: [BuiltinAssociatedType; 2] =
        [BuiltinAssociatedType::Item, BuiltinAssociatedType::Iter];
    const NO_ASSOC_TYPES: [BuiltinAssociatedType; 0] = [];
    const LANES_ASSOC_CONSTS: [BuiltinAssociatedConst; 1] = [BuiltinAssociatedConst::Lanes];
    const NO_ASSOC_CONSTS: [BuiltinAssociatedConst; 0] = [];
    const DEREF_SUPERTRAITS: [BuiltinSupertrait; 1] = [BuiltinSupertrait {
        trait_id: Self::Deref,
        preserves_trait_args: false,
    }];
    const INDEX_SUPERTRAITS: [BuiltinSupertrait; 1] = [BuiltinSupertrait {
        trait_id: Self::Index,
        preserves_trait_args: true,
    }];
    const SLICE_SUPERTRAITS: [BuiltinSupertrait; 1] = [BuiltinSupertrait {
        trait_id: Self::Slice,
        preserves_trait_args: true,
    }];
    const SIMD_MASK_SUPERTRAITS: [BuiltinSupertrait; 1] = [BuiltinSupertrait {
        trait_id: Self::Simd,
        preserves_trait_args: false,
    }];
    const NO_SUPERTRAITS: [BuiltinSupertrait; 0] = [];

    /// All builtin traits in canonical registry order.
    pub const ALL: [Self; 27] = [
        Self::Add,
        Self::Sub,
        Self::Mul,
        Self::Div,
        Self::Rem,
        Self::Neg,
        Self::Not,
        Self::BitNot,
        Self::BitAnd,
        Self::BitOr,
        Self::BitXor,
        Self::Shl,
        Self::Shr,
        Self::Eq,
        Self::Ord,
        Self::Sized,
        Self::Unsized,
        Self::Deref,
        Self::DerefMut,
        Self::Index,
        Self::IndexMut,
        Self::Slice,
        Self::SliceMut,
        Self::Iterable,
        Self::Iterator,
        Self::Simd,
        Self::SimdMask,
    ];

    const DESCRIPTORS: &'static [(Self, BuiltinTraitDescriptor)] = &[
        Self::descriptor_entry(
            Self::Add,
            "Add",
            1,
            &Self::OUTPUT_ASSOC_TYPES,
            &Self::ADD_METHODS,
            &Self::NO_SUPERTRAITS,
        ),
        Self::descriptor_entry(
            Self::Sub,
            "Sub",
            1,
            &Self::OUTPUT_ASSOC_TYPES,
            &Self::SUB_METHODS,
            &Self::NO_SUPERTRAITS,
        ),
        Self::descriptor_entry(
            Self::Mul,
            "Mul",
            1,
            &Self::OUTPUT_ASSOC_TYPES,
            &Self::MUL_METHODS,
            &Self::NO_SUPERTRAITS,
        ),
        Self::descriptor_entry(
            Self::Div,
            "Div",
            1,
            &Self::OUTPUT_ASSOC_TYPES,
            &Self::DIV_METHODS,
            &Self::NO_SUPERTRAITS,
        ),
        Self::descriptor_entry(
            Self::Rem,
            "Rem",
            1,
            &Self::OUTPUT_ASSOC_TYPES,
            &Self::REM_METHODS,
            &Self::NO_SUPERTRAITS,
        ),
        Self::descriptor_entry(
            Self::Neg,
            "Neg",
            0,
            &Self::OUTPUT_ASSOC_TYPES,
            &Self::NEG_METHODS,
            &Self::NO_SUPERTRAITS,
        ),
        Self::descriptor_entry(
            Self::Not,
            "Not",
            0,
            &Self::NO_ASSOC_TYPES,
            &Self::NOT_METHODS,
            &Self::NO_SUPERTRAITS,
        ),
        Self::descriptor_entry(
            Self::BitNot,
            "BitNot",
            0,
            &Self::OUTPUT_ASSOC_TYPES,
            &Self::BIT_NOT_METHODS,
            &Self::NO_SUPERTRAITS,
        ),
        Self::descriptor_entry(
            Self::BitAnd,
            "BitAnd",
            1,
            &Self::OUTPUT_ASSOC_TYPES,
            &Self::BIT_AND_METHODS,
            &Self::NO_SUPERTRAITS,
        ),
        Self::descriptor_entry(
            Self::BitOr,
            "BitOr",
            1,
            &Self::OUTPUT_ASSOC_TYPES,
            &Self::BIT_OR_METHODS,
            &Self::NO_SUPERTRAITS,
        ),
        Self::descriptor_entry(
            Self::BitXor,
            "BitXor",
            1,
            &Self::OUTPUT_ASSOC_TYPES,
            &Self::BIT_XOR_METHODS,
            &Self::NO_SUPERTRAITS,
        ),
        Self::descriptor_entry(
            Self::Shl,
            "Shl",
            1,
            &Self::OUTPUT_ASSOC_TYPES,
            &Self::SHL_METHODS,
            &Self::NO_SUPERTRAITS,
        ),
        Self::descriptor_entry(
            Self::Shr,
            "Shr",
            1,
            &Self::OUTPUT_ASSOC_TYPES,
            &Self::SHR_METHODS,
            &Self::NO_SUPERTRAITS,
        ),
        Self::descriptor_entry(
            Self::Eq,
            "Eq",
            1,
            &Self::NO_ASSOC_TYPES,
            &Self::EQ_METHODS,
            &Self::NO_SUPERTRAITS,
        ),
        Self::descriptor_entry(
            Self::Ord,
            "Ord",
            1,
            &Self::NO_ASSOC_TYPES,
            &Self::ORD_METHODS,
            &Self::NO_SUPERTRAITS,
        ),
        Self::descriptor_entry(
            Self::Sized,
            "Sized",
            0,
            &Self::NO_ASSOC_TYPES,
            &Self::NO_METHODS,
            &Self::NO_SUPERTRAITS,
        ),
        Self::descriptor_entry(
            Self::Unsized,
            "Unsized",
            0,
            &Self::NO_ASSOC_TYPES,
            &Self::NO_METHODS,
            &Self::NO_SUPERTRAITS,
        ),
        Self::descriptor_entry(
            Self::Deref,
            "Deref",
            0,
            &Self::TARGET_ASSOC_TYPES,
            &Self::DEREF_METHODS,
            &Self::NO_SUPERTRAITS,
        ),
        Self::descriptor_entry(
            Self::DerefMut,
            "DerefMut",
            0,
            &Self::TARGET_ASSOC_TYPES,
            &Self::DEREF_MUT_METHODS,
            &Self::DEREF_SUPERTRAITS,
        ),
        Self::descriptor_entry(
            Self::Index,
            "Index",
            1,
            &Self::OUTPUT_ASSOC_TYPES,
            &Self::INDEX_METHODS,
            &Self::NO_SUPERTRAITS,
        ),
        Self::descriptor_entry(
            Self::IndexMut,
            "IndexMut",
            1,
            &Self::OUTPUT_ASSOC_TYPES,
            &Self::INDEX_MUT_METHODS,
            &Self::INDEX_SUPERTRAITS,
        ),
        Self::descriptor_entry(
            Self::Slice,
            "Slice",
            1,
            &Self::OUTPUT_ASSOC_TYPES,
            &Self::SLICE_METHODS,
            &Self::NO_SUPERTRAITS,
        ),
        Self::descriptor_entry(
            Self::SliceMut,
            "SliceMut",
            1,
            &Self::OUTPUT_ASSOC_TYPES,
            &Self::SLICE_MUT_METHODS,
            &Self::SLICE_SUPERTRAITS,
        ),
        Self::descriptor_entry(
            Self::Iterable,
            "Iterable",
            0,
            &Self::ITERABLE_ASSOC_TYPES,
            &Self::ITERABLE_METHODS,
            &Self::NO_SUPERTRAITS,
        ),
        Self::descriptor_entry(
            Self::Iterator,
            "Iterator",
            0,
            &Self::ITEM_ASSOC_TYPES,
            &Self::ITERATOR_METHODS,
            &Self::NO_SUPERTRAITS,
        ),
        Self::descriptor_entry(
            Self::Simd,
            "Simd",
            0,
            &Self::LANE_ASSOC_TYPES,
            &Self::NO_METHODS,
            &Self::NO_SUPERTRAITS,
        ),
        Self::descriptor_entry(
            Self::SimdMask,
            "SimdMask",
            0,
            &Self::NO_ASSOC_TYPES,
            &Self::NO_METHODS,
            &Self::SIMD_MASK_SUPERTRAITS,
        ),
    ];

    const fn descriptor_entry(
        trait_id: Self,
        name: &'static str,
        generic_count: usize,
        associated_types: &'static [BuiltinAssociatedType],
        required_methods: &'static [BuiltinTraitMethod],
        supertraits: &'static [BuiltinSupertrait],
    ) -> (Self, BuiltinTraitDescriptor) {
        (
            trait_id,
            BuiltinTraitDescriptor {
                name,
                generic_count,
                associated_types,
                required_methods,
                supertraits,
            },
        )
    }

    /// Returns the complete descriptor for this builtin trait.
    pub fn descriptor(self) -> BuiltinTraitDescriptor {
        Self::DESCRIPTORS
            .iter()
            .find_map(|(trait_id, descriptor)| (*trait_id == self).then_some(*descriptor))
            .expect("missing builtin trait descriptor")
    }

    /// Parses a canonical builtin trait name.
    pub fn from_name(name: &str) -> Option<Self> {
        Self::DESCRIPTORS
            .iter()
            .find_map(|(trait_id, descriptor)| (descriptor.name == name).then_some(*trait_id))
    }

    /// Returns the canonical trait name.
    pub fn name(self) -> &'static str {
        self.descriptor().name
    }

    /// Returns the number of generic type parameters.
    pub fn generic_count(self) -> usize {
        self.descriptor().generic_count
    }

    /// Reports whether this trait declares the named associated type.
    pub fn has_associated_type(self, name: &str) -> bool {
        self.associated_types()
            .iter()
            .any(|associated_type| associated_type.name() == name)
    }

    /// Returns this trait's associated types in declaration order.
    pub fn associated_types(self) -> &'static [BuiltinAssociatedType] {
        self.descriptor().associated_types
    }

    /// Reports whether this trait declares the named associated const.
    pub fn has_associated_const(self, name: &str) -> bool {
        self.associated_consts()
            .iter()
            .any(|associated_const| associated_const.name() == name)
    }

    /// Returns this trait's associated consts in declaration order.
    pub fn associated_consts(self) -> &'static [BuiltinAssociatedConst] {
        match self {
            Self::Simd => &Self::LANES_ASSOC_CONSTS,
            _ => &Self::NO_ASSOC_CONSTS,
        }
    }

    /// Returns required methods in declaration order.
    pub fn required_methods(self) -> &'static [BuiltinTraitMethod] {
        self.descriptor().required_methods
    }

    /// Returns supertrait edges in declaration order.
    pub fn supertraits(self) -> &'static [BuiltinSupertrait] {
        self.descriptor().supertraits
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
/// Complete semantic schema for one builtin trait.
pub struct BuiltinTraitDescriptor {
    /// Canonical trait name.
    pub name: &'static str,
    /// Number of generic type parameters.
    pub generic_count: usize,
    /// Associated types declared by the trait.
    pub associated_types: &'static [BuiltinAssociatedType],
    /// Required methods in declaration order.
    pub required_methods: &'static [BuiltinTraitMethod],
    /// Supertrait edges in declaration order.
    pub supertraits: &'static [BuiltinSupertrait],
}

#[cfg(test)]
#[path = "tests.rs"]
mod tests;
