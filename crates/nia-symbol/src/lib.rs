// SPDX-License-Identifier: GPL-3.0-or-later
//! Stable symbol identities and the well-known language symbol registry.
//!
//! Symbols are represented by stable 64-bit hashes. The registry provides
//! canonical identities for syntax keywords, builtin functions, trait methods,
//! and target/configuration names without retaining source strings in semantic
//! products.
use std::{fmt, sync::Arc};

/// Fast map keyed by a [`SymbolId`].
pub type SymbolMap<T> = nia_hash::FastHashMap<SymbolId, T>;
/// Fast set of [`SymbolId`] values.
pub type SymbolSet = nia_hash::FastHashSet<SymbolId>;

/// Resolves a symbol identity back to display text when available.
pub trait SymbolText {
    /// Returns known text for `symbol`, or `None` for an unknown identity.
    fn symbol_text(&self, symbol: SymbolId) -> Option<Arc<str>>;
}

/// Returns resolved symbol text, or a deterministic unresolved marker.
pub fn symbol_text_or_unresolved(symbols: &dyn SymbolText, symbol: SymbolId) -> String {
    symbols
        .symbol_text(symbol)
        .map(|text| text.to_string())
        .unwrap_or_else(|| unresolved_symbol_text(symbol))
}

/// Resolves through an optional symbol provider, falling back to an identity marker.
pub fn symbol_text_from_optional_resolver(
    symbols: Option<&dyn SymbolText>,
    symbol: SymbolId,
) -> String {
    match symbols {
        Some(symbols) => symbol_text_or_unresolved(symbols, symbol),
        None => unresolved_symbol_text(symbol),
    }
}

/// Resolves an optional symbol while preserving `None`.
pub fn optional_symbol_text_or_unresolved(
    symbols: &dyn SymbolText,
    symbol: Option<SymbolId>,
) -> Option<String> {
    symbol.map(|symbol| symbol_text_or_unresolved(symbols, symbol))
}

/// Formats a symbol that has no known source text.
pub fn unresolved_symbol_text(symbol: SymbolId) -> String {
    format!("<unresolved symbol {:#018x}>", symbol.raw())
}

/// Returns the stable textual key used by persisted symbol consumers.
pub fn symbol_identity_key(symbol: SymbolId) -> String {
    format!("sym:{:016x}", symbol.raw())
}

/// Returns registered text when known, otherwise the stable identity key.
pub fn known_symbol_text_or_identity(symbol: SymbolId) -> String {
    known::WELL_KNOWN
        .iter()
        .find_map(|(known, text)| (*known == symbol).then_some((*text).to_string()))
        .unwrap_or_else(|| symbol_identity_key(symbol))
}

#[derive(Debug, Clone, Copy, Default)]
/// Resolver backed by the built-in [`known`] symbol registry.
pub struct KnownSymbolText;

impl SymbolText for KnownSymbolText {
    fn symbol_text(&self, symbol: SymbolId) -> Option<Arc<str>> {
        known::WELL_KNOWN
            .iter()
            .find_map(|(known, text)| (*known == symbol).then_some(Arc::<str>::from(*text)))
    }
}

#[derive(Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
/// Stable identity of one interned source symbol.
pub struct SymbolId(u64);

impl SymbolId {
    /// Identity of the empty symbol text.
    pub const EMPTY: Self = Self(stable_hash(""));

    /// Creates an identity from a previously computed stable hash.
    pub const fn from_stable_hash(hash: u64) -> Self {
        Self(hash)
    }

    /// Returns the raw stable hash value.
    pub const fn raw(self) -> u64 {
        self.0
    }

    /// Reports whether this identity is [`Self::EMPTY`].
    pub const fn is_empty(self) -> bool {
        self.0 == Self::EMPTY.0
    }
}

impl Default for SymbolId {
    fn default() -> Self {
        Self::EMPTY
    }
}

impl fmt::Debug for SymbolId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "SymbolId({:#018x})", self.0)
    }
}

pub mod known {
    //! Canonical symbols shared by parser, semantic, and builtin registries.
    use super::{SymbolId, stable_hash};

    macro_rules! known_symbol {
        ($name:ident, $text:literal) => {
            #[doc = concat!("Well-known symbol for `", $text, "`.")]
            pub const $name: SymbolId = SymbolId::from_stable_hash(stable_hash($text));
        };
    }

    known_symbol!(EMPTY, "");

    known_symbol!(ENTRY, "entry");
    known_symbol!(MODULE, "module");
    known_symbol!(BUILTIN, "builtin");
    known_symbol!(STD, "std");
    known_symbol!(NAKED, "naked");

    known_symbol!(MAIN, "main");
    known_symbol!(START_ENTRY, "_start");
    known_symbol!(START, "start");
    known_symbol!(TRUE, "true");
    known_symbol!(FALSE, "false");
    known_symbol!(FREESTANDING, "freestanding");
    known_symbol!(LINUX, "linux");
    known_symbol!(X86_64, "x86_64");

    known_symbol!(BOOL, "bool");
    known_symbol!(CHAR, "char");
    known_symbol!(NEVER, "never");

    known_symbol!(OUTPUT, "Output");
    known_symbol!(TARGET, "Target");
    known_symbol!(ITEM, "Item");
    known_symbol!(ITER, "Iter");
    known_symbol!(LANE, "Lane");
    known_symbol!(LANES, "Lanes");
    known_symbol!(FORMAT, "format");
    known_symbol!(SHOW, "show");
    known_symbol!(VALUE, "value");

    known_symbol!(ADD_TRAIT, "Add");
    known_symbol!(SUB_TRAIT, "Sub");
    known_symbol!(MUL_TRAIT, "Mul");
    known_symbol!(DIV_TRAIT, "Div");
    known_symbol!(REM_TRAIT, "Rem");
    known_symbol!(NEG_TRAIT, "Neg");
    known_symbol!(NOT_TRAIT, "Not");
    known_symbol!(BIT_NOT_TRAIT, "BitNot");
    known_symbol!(BIT_AND_TRAIT, "BitAnd");
    known_symbol!(BIT_OR_TRAIT, "BitOr");
    known_symbol!(BIT_XOR_TRAIT, "BitXor");
    known_symbol!(SHL_TRAIT, "Shl");
    known_symbol!(SHR_TRAIT, "Shr");
    known_symbol!(EQ_TRAIT, "Eq");
    known_symbol!(ORD_TRAIT, "Ord");
    known_symbol!(SIZED_TRAIT, "Sized");
    known_symbol!(UNSIZED_TRAIT, "Unsized");
    known_symbol!(DEREF_TRAIT, "Deref");
    known_symbol!(DEREF_MUT_TRAIT, "DerefMut");
    known_symbol!(INDEX_TRAIT, "Index");
    known_symbol!(INDEX_MUT_TRAIT, "IndexMut");
    known_symbol!(SLICE_TRAIT, "Slice");
    known_symbol!(SLICE_MUT_TRAIT, "SliceMut");
    known_symbol!(LEN_TYPE, "Len");
    known_symbol!(ITERABLE_TRAIT, "Iterable");
    known_symbol!(ITERATOR_TRAIT, "Iterator");
    known_symbol!(INTO_ERROR_TRAIT, "IntoError");
    known_symbol!(SIMD_TRAIT, "Simd");
    known_symbol!(SIMD_MASK_TRAIT, "SimdMask");

    known_symbol!(ADD, "add");
    known_symbol!(SUB, "sub");
    known_symbol!(MUL, "mul");
    known_symbol!(DIV, "div");
    known_symbol!(REM, "rem");
    known_symbol!(NEG, "neg");
    known_symbol!(LOGICAL_NOT, "logical_not");
    known_symbol!(BIT_NOT, "bit_not");
    known_symbol!(BIT_AND, "bit_and");
    known_symbol!(BIT_OR, "bit_or");
    known_symbol!(BIT_XOR, "bit_xor");
    known_symbol!(SHL, "shl");
    known_symbol!(SHR, "shr");
    known_symbol!(EQ, "eq");
    known_symbol!(NE, "ne");
    known_symbol!(LT, "lt");
    known_symbol!(LE, "le");
    known_symbol!(GT, "gt");
    known_symbol!(GE, "ge");
    known_symbol!(DEREF, "deref");
    known_symbol!(DEREF_MUT, "deref_mut");
    known_symbol!(INDEX, "index");
    known_symbol!(INDEX_MUT, "index_mut");
    known_symbol!(SLICE, "slice");
    known_symbol!(SLICE_MUT, "slice_mut");
    known_symbol!(PTR, "ptr");
    known_symbol!(PTR_MUT, "ptrMut");
    known_symbol!(LEN, "len");
    known_symbol!(END, "end");
    known_symbol!(ITER_METHOD, "iter");
    known_symbol!(NEXT, "next");
    known_symbol!(INTO_ERROR, "into_error");
    known_symbol!(MIN, "MIN");
    known_symbol!(MAX, "MAX");

    known_symbol!(ERROR, "error");
    known_symbol!(TRAP, "trap");
    known_symbol!(SIZE, "size");
    known_symbol!(ALIGN, "align");
    known_symbol!(OFFSET, "offset");
    known_symbol!(ASM, "asm");
    known_symbol!(CODE, "code");
    known_symbol!(INPUTS, "inputs");
    known_symbol!(OUTPUTS_LOWER, "outputs");
    known_symbol!(CLOBBERS, "clobbers");
    known_symbol!(OPTIONS, "options");
    known_symbol!(REG, "reg");
    known_symbol!(FREG, "freg");
    known_symbol!(VOLATILE, "volatile");
    known_symbol!(MEMCPY, "memcpy");
    known_symbol!(MEMMOVE, "memmove");
    known_symbol!(MEMSET, "memset");
    known_symbol!(LOAD_UNALIGNED, "load_unaligned");
    known_symbol!(SPLAT, "splat");
    known_symbol!(EXTRACT, "extract");
    known_symbol!(INSERT, "insert");
    known_symbol!(BITMASK, "bitmask");
    known_symbol!(CTZ, "ctz");
    known_symbol!(CLZ, "clz");
    known_symbol!(POPCOUNT, "popcount");
    known_symbol!(ATOMIC_LOAD, "atomic_load");
    known_symbol!(ATOMIC_STORE, "atomic_store");
    known_symbol!(ATOMIC_RMW, "atomic_rmw");
    known_symbol!(CMPXCHG_STRONG, "cmpxchg_strong");
    known_symbol!(CMPXCHG_WEAK, "cmpxchg_weak");
    known_symbol!(FENCE, "fence");
    known_symbol!(EMBED, "embed");
    known_symbol!(CHAR_FROM_U32, "charFromU32");
    known_symbol!(SLICE_LEN, "sliceLen");

    known_symbol!(ASM_CONFIG, "AsmConfig");
    known_symbol!(ASM_INPUTS, "AsmInputs");
    known_symbol!(ASM_OUTPUTS, "AsmOutputs");
    known_symbol!(I8, "i8");
    known_symbol!(I16, "i16");
    known_symbol!(I32, "i32");
    known_symbol!(I64, "i64");
    known_symbol!(I128, "i128");
    known_symbol!(ISIZE, "isize");
    known_symbol!(U8, "u8");
    known_symbol!(U16, "u16");
    known_symbol!(U32, "u32");
    known_symbol!(U64, "u64");
    known_symbol!(U128, "u128");
    known_symbol!(USIZE, "usize");
    known_symbol!(F32, "f32");
    known_symbol!(F64, "f64");

    known_symbol!(TARGET_ARCH, "target.arch");
    known_symbol!(TARGET_VENDOR, "target.vendor");
    known_symbol!(TARGET_OS, "target.os");
    known_symbol!(TARGET_ENV, "target.env");
    known_symbol!(TARGET_ABI, "target.abi");
    known_symbol!(TARGET_ENDIAN, "target.endian");
    known_symbol!(TARGET_POINTER_WIDTH, "target.pointer_width");
    known_symbol!(ARCH, "arch");
    known_symbol!(VENDOR, "vendor");
    known_symbol!(OS, "os");
    known_symbol!(ENV, "env");
    known_symbol!(ABI, "abi");
    known_symbol!(ENDIAN, "endian");
    known_symbol!(POINTER_WIDTH, "pointer_width");

    /// Complete text-to-identity registry used by deterministic resolvers.
    pub const WELL_KNOWN: &[(SymbolId, &str)] = &[
        (EMPTY, ""),
        (ENTRY, "entry"),
        (MODULE, "module"),
        (BUILTIN, "builtin"),
        (STD, "std"),
        (NAKED, "naked"),
        (MAIN, "main"),
        (START_ENTRY, "_start"),
        (START, "start"),
        (TRUE, "true"),
        (FALSE, "false"),
        (FREESTANDING, "freestanding"),
        (LINUX, "linux"),
        (X86_64, "x86_64"),
        (BOOL, "bool"),
        (CHAR, "char"),
        (NEVER, "never"),
        (OUTPUT, "Output"),
        (TARGET, "Target"),
        (ITEM, "Item"),
        (ITER, "Iter"),
        (LANE, "Lane"),
        (LANES, "Lanes"),
        (FORMAT, "format"),
        (SHOW, "show"),
        (VALUE, "value"),
        (ADD_TRAIT, "Add"),
        (SUB_TRAIT, "Sub"),
        (MUL_TRAIT, "Mul"),
        (DIV_TRAIT, "Div"),
        (REM_TRAIT, "Rem"),
        (NEG_TRAIT, "Neg"),
        (NOT_TRAIT, "Not"),
        (BIT_NOT_TRAIT, "BitNot"),
        (BIT_AND_TRAIT, "BitAnd"),
        (BIT_OR_TRAIT, "BitOr"),
        (BIT_XOR_TRAIT, "BitXor"),
        (SHL_TRAIT, "Shl"),
        (SHR_TRAIT, "Shr"),
        (EQ_TRAIT, "Eq"),
        (ORD_TRAIT, "Ord"),
        (SIZED_TRAIT, "Sized"),
        (UNSIZED_TRAIT, "Unsized"),
        (DEREF_TRAIT, "Deref"),
        (DEREF_MUT_TRAIT, "DerefMut"),
        (INDEX_TRAIT, "Index"),
        (INDEX_MUT_TRAIT, "IndexMut"),
        (SLICE_TRAIT, "Slice"),
        (SLICE_MUT_TRAIT, "SliceMut"),
        (LEN_TYPE, "Len"),
        (ITERABLE_TRAIT, "Iterable"),
        (ITERATOR_TRAIT, "Iterator"),
        (INTO_ERROR_TRAIT, "IntoError"),
        (SIMD_TRAIT, "Simd"),
        (SIMD_MASK_TRAIT, "SimdMask"),
        (ADD, "add"),
        (SUB, "sub"),
        (MUL, "mul"),
        (DIV, "div"),
        (REM, "rem"),
        (NEG, "neg"),
        (LOGICAL_NOT, "logical_not"),
        (BIT_NOT, "bit_not"),
        (BIT_AND, "bit_and"),
        (BIT_OR, "bit_or"),
        (BIT_XOR, "bit_xor"),
        (SHL, "shl"),
        (SHR, "shr"),
        (EQ, "eq"),
        (NE, "ne"),
        (LT, "lt"),
        (LE, "le"),
        (GT, "gt"),
        (GE, "ge"),
        (DEREF, "deref"),
        (DEREF_MUT, "deref_mut"),
        (INDEX, "index"),
        (INDEX_MUT, "index_mut"),
        (SLICE, "slice"),
        (SLICE_MUT, "slice_mut"),
        (PTR, "ptr"),
        (PTR_MUT, "ptrMut"),
        (LEN, "len"),
        (END, "end"),
        (ITER_METHOD, "iter"),
        (NEXT, "next"),
        (INTO_ERROR, "into_error"),
        (MIN, "MIN"),
        (MAX, "MAX"),
        (ERROR, "error"),
        (TRAP, "trap"),
        (SIZE, "size"),
        (ALIGN, "align"),
        (OFFSET, "offset"),
        (ASM, "asm"),
        (CODE, "code"),
        (INPUTS, "inputs"),
        (OUTPUTS_LOWER, "outputs"),
        (CLOBBERS, "clobbers"),
        (OPTIONS, "options"),
        (REG, "reg"),
        (FREG, "freg"),
        (VOLATILE, "volatile"),
        (MEMCPY, "memcpy"),
        (MEMMOVE, "memmove"),
        (MEMSET, "memset"),
        (LOAD_UNALIGNED, "load_unaligned"),
        (SPLAT, "splat"),
        (EXTRACT, "extract"),
        (INSERT, "insert"),
        (BITMASK, "bitmask"),
        (CTZ, "ctz"),
        (CLZ, "clz"),
        (POPCOUNT, "popcount"),
        (ATOMIC_LOAD, "atomic_load"),
        (ATOMIC_STORE, "atomic_store"),
        (ATOMIC_RMW, "atomic_rmw"),
        (CMPXCHG_STRONG, "cmpxchg_strong"),
        (CMPXCHG_WEAK, "cmpxchg_weak"),
        (FENCE, "fence"),
        (EMBED, "embed"),
        (CHAR_FROM_U32, "charFromU32"),
        (SLICE_LEN, "sliceLen"),
        (ASM_CONFIG, "AsmConfig"),
        (ASM_INPUTS, "AsmInputs"),
        (ASM_OUTPUTS, "AsmOutputs"),
        (I8, "i8"),
        (I16, "i16"),
        (I32, "i32"),
        (I64, "i64"),
        (I128, "i128"),
        (ISIZE, "isize"),
        (U8, "u8"),
        (U16, "u16"),
        (U32, "u32"),
        (U64, "u64"),
        (U128, "u128"),
        (USIZE, "usize"),
        (F32, "f32"),
        (F64, "f64"),
        (TARGET_ARCH, "target.arch"),
        (TARGET_VENDOR, "target.vendor"),
        (TARGET_OS, "target.os"),
        (TARGET_ENV, "target.env"),
        (TARGET_ABI, "target.abi"),
        (TARGET_ENDIAN, "target.endian"),
        (TARGET_POINTER_WIDTH, "target.pointer_width"),
        (ARCH, "arch"),
        (VENDOR, "vendor"),
        (OS, "os"),
        (ENV, "env"),
        (ABI, "abi"),
        (ENDIAN, "endian"),
        (POINTER_WIDTH, "pointer_width"),
    ];

    /// Returns [`ENTRY`].
    pub const fn entry() -> SymbolId {
        ENTRY
    }

    /// Returns [`MODULE`].
    pub const fn module() -> SymbolId {
        MODULE
    }

    /// Returns [`BUILTIN`].
    pub const fn builtin() -> SymbolId {
        BUILTIN
    }

    /// Returns [`STD`].
    pub const fn std() -> SymbolId {
        STD
    }
}

/// Computes the append-only FNV-1a hash used for symbol identities.
pub const fn stable_hash(text: &str) -> u64 {
    const OFFSET: u64 = 0xcbf29ce484222325;
    const PRIME: u64 = 0x00000100000001b3;

    let bytes = text.as_bytes();
    let mut hash = OFFSET;
    let mut index = 0;
    while index < bytes.len() {
        hash ^= bytes[index] as u64;
        hash = hash.wrapping_mul(PRIME);
        index += 1;
    }
    hash
}

/// Looks up a canonical symbol identity by its registered text.
pub fn symbol_for_known_text(text: &str) -> Option<SymbolId> {
    known::WELL_KNOWN
        .iter()
        .find_map(|(symbol, known_text)| (*known_text == text).then_some(*symbol))
}

/// Converts a builtin descriptor into its canonical symbol identity.
pub trait ToSymbolId {
    /// Returns the registered symbol for this builtin descriptor.
    fn symbol_id(self) -> SymbolId;
}

impl ToSymbolId for nia_ids::BuiltinFunction {
    fn symbol_id(self) -> SymbolId {
        match self {
            Self::ConstError => known::ERROR,
            Self::Trap => known::TRAP,
            Self::SizeOf => known::SIZE,
            Self::AlignOf => known::ALIGN,
            Self::Offset => known::OFFSET,
            Self::Asm => known::ASM,
            Self::MemCopy => known::MEMCPY,
            Self::MemMove => known::MEMMOVE,
            Self::MemSet => known::MEMSET,
            Self::LoadUnaligned => known::LOAD_UNALIGNED,
            Self::Splat => known::SPLAT,
            Self::Extract => known::EXTRACT,
            Self::Insert => known::INSERT,
            Self::Bitmask => known::BITMASK,
            Self::Ctz => known::CTZ,
            Self::Clz => known::CLZ,
            Self::Popcount => known::POPCOUNT,
            Self::AtomicLoad => known::ATOMIC_LOAD,
            Self::AtomicStore => known::ATOMIC_STORE,
            Self::AtomicRmw => known::ATOMIC_RMW,
            Self::CmpxchgStrong => known::CMPXCHG_STRONG,
            Self::CmpxchgWeak => known::CMPXCHG_WEAK,
            Self::Fence => known::FENCE,
            Self::Embed => known::EMBED,
            Self::CharFromU32 => known::CHAR_FROM_U32,
            Self::SliceLen => known::SLICE_LEN,
        }
    }
}

impl ToSymbolId for nia_ids::BuiltinTraitMethod {
    fn symbol_id(self) -> SymbolId {
        match self {
            Self::Add => known::ADD,
            Self::Sub => known::SUB,
            Self::Mul => known::MUL,
            Self::Div => known::DIV,
            Self::Rem => known::REM,
            Self::Neg => known::NEG,
            Self::Not => known::LOGICAL_NOT,
            Self::BitNot => known::BIT_NOT,
            Self::BitAnd => known::BIT_AND,
            Self::BitOr => known::BIT_OR,
            Self::BitXor => known::BIT_XOR,
            Self::Shl => known::SHL,
            Self::Shr => known::SHR,
            Self::Eq => known::EQ,
            Self::Ne => known::NE,
            Self::Lt => known::LT,
            Self::Le => known::LE,
            Self::Gt => known::GT,
            Self::Ge => known::GE,
            Self::Deref => known::DEREF,
            Self::DerefMut => known::DEREF_MUT,
            Self::Index => known::INDEX,
            Self::IndexMut => known::INDEX_MUT,
            Self::Slice => known::SLICE,
            Self::SliceMut => known::SLICE_MUT,
            Self::IterableIter => known::ITER_METHOD,
            Self::IteratorNext => known::NEXT,
        }
    }
}

impl ToSymbolId for nia_ids::BuiltinTrait {
    fn symbol_id(self) -> SymbolId {
        match self {
            Self::Add => known::ADD_TRAIT,
            Self::Sub => known::SUB_TRAIT,
            Self::Mul => known::MUL_TRAIT,
            Self::Div => known::DIV_TRAIT,
            Self::Rem => known::REM_TRAIT,
            Self::Neg => known::NEG_TRAIT,
            Self::Not => known::NOT_TRAIT,
            Self::BitNot => known::BIT_NOT_TRAIT,
            Self::BitAnd => known::BIT_AND_TRAIT,
            Self::BitOr => known::BIT_OR_TRAIT,
            Self::BitXor => known::BIT_XOR_TRAIT,
            Self::Shl => known::SHL_TRAIT,
            Self::Shr => known::SHR_TRAIT,
            Self::Eq => known::EQ_TRAIT,
            Self::Ord => known::ORD_TRAIT,
            Self::Sized => known::SIZED_TRAIT,
            Self::Unsized => known::UNSIZED_TRAIT,
            Self::Deref => known::DEREF_TRAIT,
            Self::DerefMut => known::DEREF_MUT_TRAIT,
            Self::Index => known::INDEX_TRAIT,
            Self::IndexMut => known::INDEX_MUT_TRAIT,
            Self::Slice => known::SLICE_TRAIT,
            Self::SliceMut => known::SLICE_MUT_TRAIT,
            Self::Iterable => known::ITERABLE_TRAIT,
            Self::Iterator => known::ITERATOR_TRAIT,
            Self::Simd => known::SIMD_TRAIT,
            Self::SimdMask => known::SIMD_MASK_TRAIT,
        }
    }
}

impl ToSymbolId for nia_ids::BuiltinAssociatedType {
    fn symbol_id(self) -> SymbolId {
        match self {
            Self::Output => known::OUTPUT,
            Self::Target => known::TARGET,
            Self::Item => known::ITEM,
            Self::Iter => known::ITER,
            Self::Lane => known::LANE,
        }
    }
}

impl ToSymbolId for nia_ids::BuiltinAssociatedConst {
    fn symbol_id(self) -> SymbolId {
        match self {
            Self::Lanes => known::LANES,
        }
    }
}

impl ToSymbolId for nia_ids::BuiltinType {
    fn symbol_id(self) -> SymbolId {
        match self {
            Self::AsmConfig => known::ASM_CONFIG,
            Self::AsmInputs => known::ASM_INPUTS,
            Self::AsmOutputs => known::ASM_OUTPUTS,
        }
    }
}

impl ToSymbolId for nia_ids::BuiltinTypeAnchor {
    fn symbol_id(self) -> SymbolId {
        match self {
            Self::I8 => known::I8,
            Self::I16 => known::I16,
            Self::I32 => known::I32,
            Self::I64 => known::I64,
            Self::I128 => known::I128,
            Self::Isize => known::ISIZE,
            Self::U8 => known::U8,
            Self::U16 => known::U16,
            Self::U32 => known::U32,
            Self::U64 => known::U64,
            Self::U128 => known::U128,
            Self::Usize => known::USIZE,
            Self::F32 => known::F32,
            Self::F64 => known::F64,
            Self::Bool => known::BOOL,
            Self::Char => known::CHAR,
            Self::Never => known::NEVER,
        }
    }
}

impl ToSymbolId for nia_ids::LayoutBuiltin {
    fn symbol_id(self) -> SymbolId {
        match self {
            Self::Size => known::SIZE,
            Self::Align => known::ALIGN,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn stable_hash_is_content_based() {
        assert_eq!(SymbolId::from_stable_hash(stable_hash("std")), known::std());
        assert_ne!(known::std(), known::entry());
    }

    #[test]
    fn known_texts_have_unique_ids() {
        let mut seen = SymbolSet::default();
        for (symbol, text) in known::WELL_KNOWN {
            assert_eq!(*symbol, SymbolId::from_stable_hash(stable_hash(text)));
            assert!(seen.insert(*symbol), "duplicate known symbol for `{text}`");
        }
    }
}
