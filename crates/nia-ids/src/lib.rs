// SPDX-License-Identifier: GPL-3.0-or-later
#[derive(Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
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
    pub const fn local_index(self) -> u32 {
        self.index
    }
}

#[derive(Debug, Clone)]
pub struct ModuleIdAllocator {
    owner: u32,
    next_index: u32,
}

impl ModuleIdAllocator {
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
pub struct DefId(pub u64);

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct GlobalDefId {
    pub module_id: ModuleId,
    pub def_id: DefId,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct TraitImplId(pub u64);

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct ConstExprId(pub u32);

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct GlobalConstExprId {
    pub module_id: ModuleId,
    pub const_expr_id: ConstExprId,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct LocalId(pub u32);

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct TypeStoreIndex(u32);

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
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

    pub const fn index(self) -> u32 {
        self.0
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct InternedTyId {
    pub store_id: TypeStoreId,
    pub index: TypeStoreIndex,
}

impl InternedTyId {
    pub const fn new(store_id: TypeStoreId, index: TypeStoreIndex) -> Self {
        Self { store_id, index }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default)]
pub enum Visibility {
    #[default]
    Private,
    PublicSuper,
    PublicPkg,
    Public,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum TraitId {
    Source(GlobalDefId),
    Builtin(BuiltinTrait),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum BuiltinType {
    AsmConfig,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum BuiltinTypeAnchor {
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

impl BuiltinType {
    pub const ALL: [Self; 1] = [Self::AsmConfig];

    pub fn from_name(name: &str) -> Option<Self> {
        match name {
            "AsmConfig" => Some(Self::AsmConfig),
            _ => None,
        }
    }

    pub fn name(self) -> &'static str {
        match self {
            Self::AsmConfig => "AsmConfig",
        }
    }
}

impl BuiltinTypeAnchor {
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
            "void" => Some(Self::Void),
            "never" => Some(Self::Never),
            _ => None,
        }
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
            Self::Never => "never",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum BuiltinTrait {
    Add,
    Sub,
    Mul,
    Div,
    Rem,
    Neg,
    Not,
    BitNot,
    BitAnd,
    BitOr,
    BitXor,
    Shl,
    Shr,
    Eq,
    Ord,
    Sized,
    Unsized,
    Deref,
    DerefMut,
    Index,
    IndexMut,
    Slice,
    SliceMut,
    Iterable,
    Iterator,
    Simd,
    SimdMask,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum ValueBuiltin {
    Error,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum BuiltinConstValue {
    TargetArch,
    TargetVendor,
    TargetOs,
    TargetEnv,
    TargetAbi,
    TargetEndian,
    TargetPointerWidth,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum BuiltinFunction {
    ConstError,
    Trap,
    SizeOf,
    AlignOf,
    Offset,
    Asm,
    MemCopy,
    MemMove,
    MemSet,
    LoadUnaligned,
    Splat,
    Extract,
    Insert,
    Bitmask,
    Ctz,
    Clz,
    Popcount,
    AtomicLoad,
    AtomicStore,
    AtomicRmw,
    CmpxchgStrong,
    CmpxchgWeak,
    Fence,
    Embed,
    CharFromU32,
    SliceLen,
}

impl BuiltinFunction {
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
    pub fn from_name(name: &str) -> Option<Self> {
        match name {
            "error" => Some(Self::Error),
            _ => None,
        }
    }

    pub fn name(self) -> &'static str {
        match self {
            Self::Error => "error",
        }
    }
}

impl BuiltinConstValue {
    pub const ALL: [Self; 7] = [
        Self::TargetArch,
        Self::TargetVendor,
        Self::TargetOs,
        Self::TargetEnv,
        Self::TargetAbi,
        Self::TargetEndian,
        Self::TargetPointerWidth,
    ];

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
pub enum LayoutBuiltin {
    Size,
    Align,
}

impl LayoutBuiltin {
    pub fn from_name(name: &str) -> Option<Self> {
        match name {
            "size" => Some(Self::Size),
            "align" => Some(Self::Align),
            _ => None,
        }
    }

    pub fn name(self) -> &'static str {
        match self {
            Self::Size => "size",
            Self::Align => "align",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum BuiltinTraitMethod {
    Add,
    Sub,
    Mul,
    Div,
    Rem,
    Neg,
    Not,
    BitNot,
    BitAnd,
    BitOr,
    BitXor,
    Shl,
    Shr,
    Eq,
    Ne,
    Lt,
    Le,
    Gt,
    Ge,
    Deref,
    DerefMut,
    Index,
    IndexMut,
    Slice,
    SliceMut,
    IterableIter,
    IteratorNext,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum ReceiverKind {
    RefReadOnly,
    Ref,
    Value,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum BuiltinAssociatedType {
    Output,
    Target,
    Item,
    Iter,
    Lane,
}

impl BuiltinAssociatedType {
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
pub enum BuiltinAssociatedConst {
    Lanes,
}

impl BuiltinAssociatedConst {
    pub fn from_name(name: &str) -> Option<Self> {
        match name {
            "Lanes" => Some(Self::Lanes),
            _ => None,
        }
    }

    pub fn name(self) -> &'static str {
        match self {
            Self::Lanes => "Lanes",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct BuiltinSupertrait {
    pub trait_id: BuiltinTrait,
    pub preserves_trait_args: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct BuiltinTraitMethodDescriptor {
    pub name: &'static str,
    pub trait_id: BuiltinTrait,
    pub param_count: usize,
    pub receiver_kind: ReceiverKind,
    pub place_receiver_kind: Option<ReceiverKind>,
    pub is_value_operator: bool,
    pub is_place_method: bool,
}

impl BuiltinTraitMethod {
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
            BuiltinTraitMethodDescriptor::value_operator("logical_not", BuiltinTrait::Not, 1),
        ),
        (
            Self::BitNot,
            BuiltinTraitMethodDescriptor::value_operator("bit_not", BuiltinTrait::BitNot, 1),
        ),
        (
            Self::BitAnd,
            BuiltinTraitMethodDescriptor::value_operator("bit_and", BuiltinTrait::BitAnd, 2),
        ),
        (
            Self::BitOr,
            BuiltinTraitMethodDescriptor::value_operator("bit_or", BuiltinTrait::BitOr, 2),
        ),
        (
            Self::BitXor,
            BuiltinTraitMethodDescriptor::value_operator("bit_xor", BuiltinTrait::BitXor, 2),
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
                "deref_mut",
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
                "index_mut",
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
                "slice_mut",
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

    pub fn descriptor(self) -> BuiltinTraitMethodDescriptor {
        Self::DESCRIPTORS
            .iter()
            .find_map(|(method, descriptor)| (*method == self).then_some(*descriptor))
            .expect("missing builtin trait method descriptor")
    }

    pub fn from_name(name: &str) -> Option<Self> {
        Self::DESCRIPTORS
            .iter()
            .find_map(|(method, descriptor)| (descriptor.name == name).then_some(*method))
    }

    pub fn name(self) -> &'static str {
        self.descriptor().name
    }

    pub fn param_count(self) -> usize {
        self.descriptor().param_count
    }

    pub fn receiver_kind(self) -> ReceiverKind {
        self.descriptor().receiver_kind
    }

    pub fn place_receiver_kind(self) -> Option<ReceiverKind> {
        self.descriptor().place_receiver_kind
    }

    pub fn trait_id(self) -> BuiltinTrait {
        self.descriptor().trait_id
    }

    pub fn is_value_operator(self) -> bool {
        self.descriptor().is_value_operator
    }

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
    pub const OUTPUT_ASSOC_TYPE: &'static str = "Output";
    pub const TARGET_ASSOC_TYPE: &'static str = "Target";
    pub const ITEM_ASSOC_TYPE: &'static str = "Item";
    pub const ITER_ASSOC_TYPE: &'static str = "Iter";
    pub const LANE_ASSOC_TYPE: &'static str = "Lane";
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

    pub fn descriptor(self) -> BuiltinTraitDescriptor {
        Self::DESCRIPTORS
            .iter()
            .find_map(|(trait_id, descriptor)| (*trait_id == self).then_some(*descriptor))
            .expect("missing builtin trait descriptor")
    }

    pub fn from_name(name: &str) -> Option<Self> {
        Self::DESCRIPTORS
            .iter()
            .find_map(|(trait_id, descriptor)| (descriptor.name == name).then_some(*trait_id))
    }

    pub fn name(self) -> &'static str {
        self.descriptor().name
    }

    pub fn generic_count(self) -> usize {
        self.descriptor().generic_count
    }

    pub fn has_associated_type(self, name: &str) -> bool {
        self.associated_types()
            .iter()
            .any(|associated_type| associated_type.name() == name)
    }

    pub fn associated_types(self) -> &'static [BuiltinAssociatedType] {
        self.descriptor().associated_types
    }

    pub fn has_associated_const(self, name: &str) -> bool {
        self.associated_consts()
            .iter()
            .any(|associated_const| associated_const.name() == name)
    }

    pub fn associated_consts(self) -> &'static [BuiltinAssociatedConst] {
        match self {
            Self::Simd => &Self::LANES_ASSOC_CONSTS,
            _ => &Self::NO_ASSOC_CONSTS,
        }
    }

    pub fn required_methods(self) -> &'static [BuiltinTraitMethod] {
        self.descriptor().required_methods
    }

    pub fn supertraits(self) -> &'static [BuiltinSupertrait] {
        self.descriptor().supertraits
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct BuiltinTraitDescriptor {
    pub name: &'static str,
    pub generic_count: usize,
    pub associated_types: &'static [BuiltinAssociatedType],
    pub required_methods: &'static [BuiltinTraitMethod],
    pub supertraits: &'static [BuiltinSupertrait],
}

#[cfg(test)]
#[path = "tests.rs"]
mod tests;
