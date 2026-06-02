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
    DerefConst,
    Deref,
    IndexConst,
    Index,
    SliceConst,
    Slice,
    GetPtrConst,
    GetPtr,
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
    DerefConst,
    Deref,
    IndexConst,
    Index,
    SliceConst,
    Slice,
    GetPtrConst,
    GetPtr,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum BuiltinReceiverKind {
    RefConst,
    Ref,
    Value,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum BuiltinAssociatedType {
    Output,
    Target,
}

impl BuiltinAssociatedType {
    pub fn from_name(name: &str) -> Option<Self> {
        match name {
            "Output" => Some(Self::Output),
            "Target" => Some(Self::Target),
            _ => None,
        }
    }

    pub fn name(self) -> &'static str {
        match self {
            Self::Output => "Output",
            Self::Target => "Target",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct BuiltinSupertrait {
    pub trait_id: BuiltinTrait,
    pub preserves_trait_args: bool,
}

impl BuiltinTraitMethod {
    pub fn from_name(name: &str) -> Option<Self> {
        match name {
            "add" => Some(Self::Add),
            "sub" => Some(Self::Sub),
            "mul" => Some(Self::Mul),
            "div" => Some(Self::Div),
            "rem" => Some(Self::Rem),
            "neg" => Some(Self::Neg),
            "not" => Some(Self::Not),
            "bit_not" => Some(Self::BitNot),
            "bit_and" => Some(Self::BitAnd),
            "bit_or" => Some(Self::BitOr),
            "bit_xor" => Some(Self::BitXor),
            "shl" => Some(Self::Shl),
            "shr" => Some(Self::Shr),
            "eq" => Some(Self::Eq),
            "ne" => Some(Self::Ne),
            "lt" => Some(Self::Lt),
            "le" => Some(Self::Le),
            "gt" => Some(Self::Gt),
            "ge" => Some(Self::Ge),
            "deref_const" => Some(Self::DerefConst),
            "deref" => Some(Self::Deref),
            "index_const" => Some(Self::IndexConst),
            "index" => Some(Self::Index),
            "slice_const" => Some(Self::SliceConst),
            "slice" => Some(Self::Slice),
            "get_ptr_const" => Some(Self::GetPtrConst),
            "get_ptr" => Some(Self::GetPtr),
            _ => None,
        }
    }

    pub fn name(self) -> &'static str {
        match self {
            Self::Add => "add",
            Self::Sub => "sub",
            Self::Mul => "mul",
            Self::Div => "div",
            Self::Rem => "rem",
            Self::Neg => "neg",
            Self::Not => "not",
            Self::BitNot => "bit_not",
            Self::BitAnd => "bit_and",
            Self::BitOr => "bit_or",
            Self::BitXor => "bit_xor",
            Self::Shl => "shl",
            Self::Shr => "shr",
            Self::Eq => "eq",
            Self::Ne => "ne",
            Self::Lt => "lt",
            Self::Le => "le",
            Self::Gt => "gt",
            Self::Ge => "ge",
            Self::DerefConst => "deref_const",
            Self::Deref => "deref",
            Self::IndexConst => "index_const",
            Self::Index => "index",
            Self::SliceConst => "slice_const",
            Self::Slice => "slice",
            Self::GetPtrConst => "get_ptr_const",
            Self::GetPtr => "get_ptr",
        }
    }

    pub fn param_count(self) -> usize {
        match self {
            Self::Neg
            | Self::Not
            | Self::BitNot
            | Self::DerefConst
            | Self::Deref
            | Self::GetPtrConst
            | Self::GetPtr => 1,
            Self::SliceConst | Self::Slice => 2,
            _ => 2,
        }
    }

    pub fn receiver_kind(self) -> BuiltinReceiverKind {
        match self {
            Self::DerefConst | Self::IndexConst | Self::SliceConst | Self::GetPtrConst => {
                BuiltinReceiverKind::RefConst
            }
            Self::Slice | Self::GetPtr => BuiltinReceiverKind::Ref,
            _ => BuiltinReceiverKind::Value,
        }
    }

    pub fn place_receiver_kind(self) -> Option<BuiltinReceiverKind> {
        match self {
            Self::DerefConst | Self::IndexConst => Some(BuiltinReceiverKind::RefConst),
            Self::Deref | Self::Index => Some(BuiltinReceiverKind::Ref),
            _ => None,
        }
    }

    pub fn trait_id(self) -> BuiltinTrait {
        match self {
            Self::Add => BuiltinTrait::Add,
            Self::Sub => BuiltinTrait::Sub,
            Self::Mul => BuiltinTrait::Mul,
            Self::Div => BuiltinTrait::Div,
            Self::Rem => BuiltinTrait::Rem,
            Self::Neg => BuiltinTrait::Neg,
            Self::Not => BuiltinTrait::Not,
            Self::BitNot => BuiltinTrait::BitNot,
            Self::BitAnd => BuiltinTrait::BitAnd,
            Self::BitOr => BuiltinTrait::BitOr,
            Self::BitXor => BuiltinTrait::BitXor,
            Self::Shl => BuiltinTrait::Shl,
            Self::Shr => BuiltinTrait::Shr,
            Self::Eq | Self::Ne => BuiltinTrait::Eq,
            Self::Lt | Self::Le | Self::Gt | Self::Ge => BuiltinTrait::Ord,
            Self::DerefConst => BuiltinTrait::DerefConst,
            Self::Deref => BuiltinTrait::Deref,
            Self::IndexConst => BuiltinTrait::IndexConst,
            Self::Index => BuiltinTrait::Index,
            Self::SliceConst => BuiltinTrait::SliceConst,
            Self::Slice => BuiltinTrait::Slice,
            Self::GetPtrConst => BuiltinTrait::GetPtrConst,
            Self::GetPtr => BuiltinTrait::GetPtr,
        }
    }

    pub fn is_value_operator(self) -> bool {
        matches!(
            self,
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
        )
    }

    pub fn is_place_method(self) -> bool {
        matches!(
            self,
            Self::SliceConst | Self::Slice | Self::GetPtrConst | Self::GetPtr
        )
    }
}

impl BuiltinTrait {
    pub const OUTPUT_ASSOC_TYPE: &'static str = "Output";
    pub const TARGET_ASSOC_TYPE: &'static str = "Target";

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
    const DEREF_CONST_METHODS: [BuiltinTraitMethod; 1] = [BuiltinTraitMethod::DerefConst];
    const DEREF_METHODS: [BuiltinTraitMethod; 1] = [BuiltinTraitMethod::Deref];
    const INDEX_CONST_METHODS: [BuiltinTraitMethod; 1] = [BuiltinTraitMethod::IndexConst];
    const INDEX_METHODS: [BuiltinTraitMethod; 1] = [BuiltinTraitMethod::Index];
    const SLICE_CONST_METHODS: [BuiltinTraitMethod; 1] = [BuiltinTraitMethod::SliceConst];
    const SLICE_METHODS: [BuiltinTraitMethod; 1] = [BuiltinTraitMethod::Slice];
    const GET_PTR_CONST_METHODS: [BuiltinTraitMethod; 1] = [BuiltinTraitMethod::GetPtrConst];
    const GET_PTR_METHODS: [BuiltinTraitMethod; 1] = [BuiltinTraitMethod::GetPtr];
    const OUTPUT_ASSOC_TYPES: [BuiltinAssociatedType; 1] = [BuiltinAssociatedType::Output];
    const TARGET_ASSOC_TYPES: [BuiltinAssociatedType; 1] = [BuiltinAssociatedType::Target];
    const NO_ASSOC_TYPES: [BuiltinAssociatedType; 0] = [];
    const DEREF_SUPERTRAITS: [BuiltinSupertrait; 1] = [BuiltinSupertrait {
        trait_id: Self::DerefConst,
        preserves_trait_args: false,
    }];
    const INDEX_SUPERTRAITS: [BuiltinSupertrait; 1] = [BuiltinSupertrait {
        trait_id: Self::IndexConst,
        preserves_trait_args: true,
    }];
    const SLICE_SUPERTRAITS: [BuiltinSupertrait; 1] = [BuiltinSupertrait {
        trait_id: Self::SliceConst,
        preserves_trait_args: true,
    }];
    const GET_PTR_SUPERTRAITS: [BuiltinSupertrait; 1] = [BuiltinSupertrait {
        trait_id: Self::GetPtrConst,
        preserves_trait_args: false,
    }];
    const NO_SUPERTRAITS: [BuiltinSupertrait; 0] = [];

    pub const ALL: [Self; 24] = [
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
        Self::DerefConst,
        Self::Deref,
        Self::IndexConst,
        Self::Index,
        Self::SliceConst,
        Self::Slice,
        Self::GetPtrConst,
        Self::GetPtr,
    ];

    pub fn from_name(name: &str) -> Option<Self> {
        match name {
            "Add" => Some(Self::Add),
            "Sub" => Some(Self::Sub),
            "Mul" => Some(Self::Mul),
            "Div" => Some(Self::Div),
            "Rem" => Some(Self::Rem),
            "Neg" => Some(Self::Neg),
            "Not" => Some(Self::Not),
            "BitNot" => Some(Self::BitNot),
            "BitAnd" => Some(Self::BitAnd),
            "BitOr" => Some(Self::BitOr),
            "BitXor" => Some(Self::BitXor),
            "Shl" => Some(Self::Shl),
            "Shr" => Some(Self::Shr),
            "Eq" => Some(Self::Eq),
            "Ord" => Some(Self::Ord),
            "Sized" => Some(Self::Sized),
            "DerefConst" => Some(Self::DerefConst),
            "Deref" => Some(Self::Deref),
            "IndexConst" => Some(Self::IndexConst),
            "Index" => Some(Self::Index),
            "SliceConst" => Some(Self::SliceConst),
            "Slice" => Some(Self::Slice),
            "GetPtrConst" => Some(Self::GetPtrConst),
            "GetPtr" => Some(Self::GetPtr),
            _ => None,
        }
    }

    pub fn name(self) -> &'static str {
        match self {
            Self::Add => "Add",
            Self::Sub => "Sub",
            Self::Mul => "Mul",
            Self::Div => "Div",
            Self::Rem => "Rem",
            Self::Neg => "Neg",
            Self::Not => "Not",
            Self::BitNot => "BitNot",
            Self::BitAnd => "BitAnd",
            Self::BitOr => "BitOr",
            Self::BitXor => "BitXor",
            Self::Shl => "Shl",
            Self::Shr => "Shr",
            Self::Eq => "Eq",
            Self::Ord => "Ord",
            Self::Sized => "Sized",
            Self::DerefConst => "DerefConst",
            Self::Deref => "Deref",
            Self::IndexConst => "IndexConst",
            Self::Index => "Index",
            Self::SliceConst => "SliceConst",
            Self::Slice => "Slice",
            Self::GetPtrConst => "GetPtrConst",
            Self::GetPtr => "GetPtr",
        }
    }

    pub fn generic_count(self) -> usize {
        match self {
            Self::Add
            | Self::Sub
            | Self::Mul
            | Self::Div
            | Self::Rem
            | Self::BitAnd
            | Self::BitOr
            | Self::BitXor
            | Self::Shl
            | Self::Shr
            | Self::Eq
            | Self::Ord
            | Self::IndexConst
            | Self::Index
            | Self::SliceConst
            | Self::Slice => 1,
            Self::Neg
            | Self::Not
            | Self::BitNot
            | Self::Sized
            | Self::DerefConst
            | Self::Deref
            | Self::GetPtrConst
            | Self::GetPtr => 0,
        }
    }

    pub fn has_associated_type(self, name: &str) -> bool {
        self.associated_types()
            .iter()
            .any(|associated_type| associated_type.name() == name)
    }

    pub fn associated_types(self) -> &'static [BuiltinAssociatedType] {
        match self {
            Self::Add
            | Self::Sub
            | Self::Mul
            | Self::Div
            | Self::Rem
            | Self::Neg
            | Self::BitNot
            | Self::BitAnd
            | Self::BitOr
            | Self::BitXor
            | Self::Shl
            | Self::Shr
            | Self::IndexConst
            | Self::Index
            | Self::SliceConst
            | Self::Slice => &Self::OUTPUT_ASSOC_TYPES,
            Self::DerefConst | Self::Deref | Self::GetPtrConst | Self::GetPtr => {
                &Self::TARGET_ASSOC_TYPES
            }
            Self::Not | Self::Eq | Self::Ord | Self::Sized => &Self::NO_ASSOC_TYPES,
        }
    }

    pub fn required_methods(self) -> &'static [BuiltinTraitMethod] {
        match self {
            Self::Add => &Self::ADD_METHODS,
            Self::Sub => &Self::SUB_METHODS,
            Self::Mul => &Self::MUL_METHODS,
            Self::Div => &Self::DIV_METHODS,
            Self::Rem => &Self::REM_METHODS,
            Self::Neg => &Self::NEG_METHODS,
            Self::Not => &Self::NOT_METHODS,
            Self::BitNot => &Self::BIT_NOT_METHODS,
            Self::BitAnd => &Self::BIT_AND_METHODS,
            Self::BitOr => &Self::BIT_OR_METHODS,
            Self::BitXor => &Self::BIT_XOR_METHODS,
            Self::Shl => &Self::SHL_METHODS,
            Self::Shr => &Self::SHR_METHODS,
            Self::Eq => &Self::EQ_METHODS,
            Self::Ord => &Self::ORD_METHODS,
            Self::Sized => &Self::NO_METHODS,
            Self::DerefConst => &Self::DEREF_CONST_METHODS,
            Self::Deref => &Self::DEREF_METHODS,
            Self::IndexConst => &Self::INDEX_CONST_METHODS,
            Self::Index => &Self::INDEX_METHODS,
            Self::SliceConst => &Self::SLICE_CONST_METHODS,
            Self::Slice => &Self::SLICE_METHODS,
            Self::GetPtrConst => &Self::GET_PTR_CONST_METHODS,
            Self::GetPtr => &Self::GET_PTR_METHODS,
        }
    }

    pub fn supertraits(self) -> &'static [BuiltinSupertrait] {
        match self {
            Self::Deref => &Self::DEREF_SUPERTRAITS,
            Self::Index => &Self::INDEX_SUPERTRAITS,
            Self::Slice => &Self::SLICE_SUPERTRAITS,
            Self::GetPtr => &Self::GET_PTR_SUPERTRAITS,
            _ => &Self::NO_SUPERTRAITS,
        }
    }
}
