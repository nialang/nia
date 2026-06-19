// SPDX-License-Identifier: GPL-3.0-or-later
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct ModuleId(pub u32);

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct DefId(pub u32);

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct GlobalDefId {
    pub module_id: ModuleId,
    pub def_id: DefId,
}

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
pub struct TyInternerIndex(u32);

impl TyInternerIndex {
    #[doc(hidden)]
    pub const fn from_interner_index(index: u32) -> Self {
        Self(index)
    }

    pub const fn index(self) -> u32 {
        self.0
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct InternedTyId {
    pub interner_id: ModuleId,
    pub index: TyInternerIndex,
}

impl InternedTyId {
    pub const fn new(interner_id: ModuleId, index: TyInternerIndex) -> Self {
        Self { interner_id, index }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum TraitId {
    Source(GlobalDefId),
    Builtin(BuiltinTrait),
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
    DerefRead,
    Deref,
    IndexRead,
    Index,
    SliceRead,
    Slice,
    GetPtrRead,
    GetPtr,
    Len,
    Start,
    End,
    Iterator,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum ValueBuiltin {
    Error,
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
    DerefRead,
    Deref,
    IndexRead,
    Index,
    SliceRead,
    Slice,
    GetPtrRead,
    GetPtr,
    Len,
    Start,
    End,
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
}

impl BuiltinAssociatedType {
    pub fn from_name(name: &str) -> Option<Self> {
        match name {
            "Output" => Some(Self::Output),
            "Target" => Some(Self::Target),
            "Item" => Some(Self::Item),
            _ => None,
        }
    }

    pub fn name(self) -> &'static str {
        match self {
            Self::Output => "Output",
            Self::Target => "Target",
            Self::Item => "Item",
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
            BuiltinTraitMethodDescriptor::value_operator("not", BuiltinTrait::Not, 1),
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
            Self::DerefRead,
            BuiltinTraitMethodDescriptor::place(
                "deref_read",
                BuiltinTrait::DerefRead,
                1,
                ReceiverKind::RefReadOnly,
                Some(ReceiverKind::RefReadOnly),
            ),
        ),
        (
            Self::Deref,
            BuiltinTraitMethodDescriptor::place(
                "deref",
                BuiltinTrait::Deref,
                1,
                ReceiverKind::Value,
                Some(ReceiverKind::Ref),
            ),
        ),
        (
            Self::IndexRead,
            BuiltinTraitMethodDescriptor::place(
                "index_read",
                BuiltinTrait::IndexRead,
                2,
                ReceiverKind::RefReadOnly,
                Some(ReceiverKind::RefReadOnly),
            ),
        ),
        (
            Self::Index,
            BuiltinTraitMethodDescriptor::place(
                "index",
                BuiltinTrait::Index,
                2,
                ReceiverKind::Value,
                Some(ReceiverKind::Ref),
            ),
        ),
        (
            Self::SliceRead,
            BuiltinTraitMethodDescriptor::place(
                "slice_read",
                BuiltinTrait::SliceRead,
                2,
                ReceiverKind::RefReadOnly,
                None,
            ),
        ),
        (
            Self::Slice,
            BuiltinTraitMethodDescriptor::place(
                "slice",
                BuiltinTrait::Slice,
                2,
                ReceiverKind::Ref,
                None,
            ),
        ),
        (
            Self::GetPtrRead,
            BuiltinTraitMethodDescriptor::place(
                "get_ptr_read",
                BuiltinTrait::GetPtrRead,
                1,
                ReceiverKind::RefReadOnly,
                None,
            ),
        ),
        (
            Self::GetPtr,
            BuiltinTraitMethodDescriptor::place(
                "get_ptr",
                BuiltinTrait::GetPtr,
                1,
                ReceiverKind::Ref,
                None,
            ),
        ),
        (
            Self::Len,
            BuiltinTraitMethodDescriptor::method(
                "len",
                BuiltinTrait::Len,
                1,
                ReceiverKind::RefReadOnly,
            ),
        ),
        (
            Self::Start,
            BuiltinTraitMethodDescriptor::method(
                "start",
                BuiltinTrait::Start,
                1,
                ReceiverKind::RefReadOnly,
            ),
        ),
        (
            Self::End,
            BuiltinTraitMethodDescriptor::method(
                "end",
                BuiltinTrait::End,
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
    const DEREF_READ_METHODS: [BuiltinTraitMethod; 1] = [BuiltinTraitMethod::DerefRead];
    const DEREF_METHODS: [BuiltinTraitMethod; 1] = [BuiltinTraitMethod::Deref];
    const INDEX_READ_METHODS: [BuiltinTraitMethod; 1] = [BuiltinTraitMethod::IndexRead];
    const INDEX_METHODS: [BuiltinTraitMethod; 1] = [BuiltinTraitMethod::Index];
    const SLICE_READ_METHODS: [BuiltinTraitMethod; 1] = [BuiltinTraitMethod::SliceRead];
    const SLICE_METHODS: [BuiltinTraitMethod; 1] = [BuiltinTraitMethod::Slice];
    const GET_PTR_READ_METHODS: [BuiltinTraitMethod; 1] = [BuiltinTraitMethod::GetPtrRead];
    const GET_PTR_METHODS: [BuiltinTraitMethod; 1] = [BuiltinTraitMethod::GetPtr];
    const LEN_METHODS: [BuiltinTraitMethod; 1] = [BuiltinTraitMethod::Len];
    const START_METHODS: [BuiltinTraitMethod; 1] = [BuiltinTraitMethod::Start];
    const END_METHODS: [BuiltinTraitMethod; 1] = [BuiltinTraitMethod::End];
    const ITERATOR_METHODS: [BuiltinTraitMethod; 1] = [BuiltinTraitMethod::IteratorNext];
    const OUTPUT_ASSOC_TYPES: [BuiltinAssociatedType; 1] = [BuiltinAssociatedType::Output];
    const TARGET_ASSOC_TYPES: [BuiltinAssociatedType; 1] = [BuiltinAssociatedType::Target];
    const ITEM_ASSOC_TYPES: [BuiltinAssociatedType; 1] = [BuiltinAssociatedType::Item];
    const NO_ASSOC_TYPES: [BuiltinAssociatedType; 0] = [];
    const DEREF_SUPERTRAITS: [BuiltinSupertrait; 1] = [BuiltinSupertrait {
        trait_id: Self::DerefRead,
        preserves_trait_args: false,
    }];
    const INDEX_SUPERTRAITS: [BuiltinSupertrait; 1] = [BuiltinSupertrait {
        trait_id: Self::IndexRead,
        preserves_trait_args: true,
    }];
    const SLICE_SUPERTRAITS: [BuiltinSupertrait; 1] = [BuiltinSupertrait {
        trait_id: Self::SliceRead,
        preserves_trait_args: true,
    }];
    const GET_PTR_SUPERTRAITS: [BuiltinSupertrait; 1] = [BuiltinSupertrait {
        trait_id: Self::GetPtrRead,
        preserves_trait_args: false,
    }];
    const NO_SUPERTRAITS: [BuiltinSupertrait; 0] = [];

    pub const ALL: [Self; 29] = [
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
        Self::DerefRead,
        Self::Deref,
        Self::IndexRead,
        Self::Index,
        Self::SliceRead,
        Self::Slice,
        Self::GetPtrRead,
        Self::GetPtr,
        Self::Len,
        Self::Start,
        Self::End,
        Self::Iterator,
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
            Self::DerefRead,
            "DerefRead",
            0,
            &Self::TARGET_ASSOC_TYPES,
            &Self::DEREF_READ_METHODS,
            &Self::NO_SUPERTRAITS,
        ),
        Self::descriptor_entry(
            Self::Deref,
            "Deref",
            0,
            &Self::TARGET_ASSOC_TYPES,
            &Self::DEREF_METHODS,
            &Self::DEREF_SUPERTRAITS,
        ),
        Self::descriptor_entry(
            Self::IndexRead,
            "IndexRead",
            1,
            &Self::OUTPUT_ASSOC_TYPES,
            &Self::INDEX_READ_METHODS,
            &Self::NO_SUPERTRAITS,
        ),
        Self::descriptor_entry(
            Self::Index,
            "Index",
            1,
            &Self::OUTPUT_ASSOC_TYPES,
            &Self::INDEX_METHODS,
            &Self::INDEX_SUPERTRAITS,
        ),
        Self::descriptor_entry(
            Self::SliceRead,
            "SliceRead",
            1,
            &Self::OUTPUT_ASSOC_TYPES,
            &Self::SLICE_READ_METHODS,
            &Self::NO_SUPERTRAITS,
        ),
        Self::descriptor_entry(
            Self::Slice,
            "Slice",
            1,
            &Self::OUTPUT_ASSOC_TYPES,
            &Self::SLICE_METHODS,
            &Self::SLICE_SUPERTRAITS,
        ),
        Self::descriptor_entry(
            Self::GetPtrRead,
            "GetPtrRead",
            0,
            &Self::TARGET_ASSOC_TYPES,
            &Self::GET_PTR_READ_METHODS,
            &Self::NO_SUPERTRAITS,
        ),
        Self::descriptor_entry(
            Self::GetPtr,
            "GetPtr",
            0,
            &Self::TARGET_ASSOC_TYPES,
            &Self::GET_PTR_METHODS,
            &Self::GET_PTR_SUPERTRAITS,
        ),
        Self::descriptor_entry(
            Self::Len,
            "Len",
            0,
            &Self::NO_ASSOC_TYPES,
            &Self::LEN_METHODS,
            &Self::NO_SUPERTRAITS,
        ),
        Self::descriptor_entry(
            Self::Start,
            "Start",
            0,
            &Self::OUTPUT_ASSOC_TYPES,
            &Self::START_METHODS,
            &Self::NO_SUPERTRAITS,
        ),
        Self::descriptor_entry(
            Self::End,
            "End",
            0,
            &Self::OUTPUT_ASSOC_TYPES,
            &Self::END_METHODS,
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
