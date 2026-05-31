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
}

impl BuiltinTraitMethod {
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
        }
    }

    pub fn param_count(self) -> usize {
        match self {
            Self::Neg | Self::Not | Self::BitNot => 1,
            _ => 2,
        }
    }
}

impl BuiltinTrait {
    pub const OUTPUT_ASSOC_TYPE: &'static str = "Output";

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

    pub const ALL: [Self; 16] = [
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
            | Self::Ord => 1,
            Self::Neg | Self::Not | Self::BitNot | Self::Sized => 0,
        }
    }

    pub fn has_associated_type(self, name: &str) -> bool {
        matches!(
            self,
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
        ) && name == Self::OUTPUT_ASSOC_TYPE
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
        }
    }
}
