// SPDX-License-Identifier: GPL-3.0-or-later
use std::collections::HashMap;

use nia_ast::{BinaryOp, UnaryOp};
use nia_ids::{BuiltinTraitMethod, GlobalDefId, InternedTyId, LayoutBuiltin, LocalId};
use nia_node_id::NodeKey;
use nia_span::Span;
use nia_ty::{BuiltinTrait, TraitId};

#[derive(Debug, Clone, PartialEq, Default)]
pub struct SemanticUseTable {
    pub value_uses: HashMap<Span, SemanticValueUse>,
    pub local_defs: HashMap<Span, LocalId>,
    pub type_uses: HashMap<Span, InternedTyId>,
}

impl SemanticUseTable {
    pub fn value_use(&self, span: Span) -> Option<SemanticValueUse> {
        self.value_uses.get(&span).copied()
    }

    pub fn local_def(&self, span: Span) -> Option<LocalId> {
        self.local_defs.get(&span).copied()
    }

    pub fn type_use(&self, span: Span) -> Option<InternedTyId> {
        self.type_uses.get(&span).copied()
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SemanticValueUse {
    Local(LocalId),
    Global(GlobalDefId),
}

#[derive(Debug, Clone, PartialEq, Default)]
pub struct SemanticFacts {
    pub expr_types: HashMap<Span, InternedTyId>,
    pub bracket_suffix_resolutions: HashMap<Span, BracketSuffixResolution>,
    pub array_to_slice_coercions: HashMap<Span, ArrayToSliceCoercion>,
    pub c_string_pointer_coercions: HashMap<Span, CStringPointerCoercion>,
    pub trait_object_coercions: HashMap<Span, TraitObjectCoercion>,
    pub trait_object_upcasts: HashMap<Span, TraitObjectUpcast>,
    pub local_types: HashMap<LocalId, InternedTyId>,
    pub comptime_if_selections: HashMap<Span, ComptimeIfSelection>,
    pub builtin_values: HashMap<Span, BuiltinValue>,
    pub array_repeat_counts: HashMap<Span, u64>,
    pub switch_pattern_values: HashMap<Span, i128>,
    pub resolved_calls: HashMap<Span, ResolvedCall>,
    pub function_references: HashMap<Span, FunctionReference>,
    pub generic_instantiations: Vec<GenericInstantiation>,
    pub node_expr_types: HashMap<NodeKey, InternedTyId>,
    pub node_bracket_suffix_resolutions: HashMap<NodeKey, BracketSuffixResolution>,
    pub node_array_to_slice_coercions: HashMap<NodeKey, ArrayToSliceCoercion>,
    pub node_c_string_pointer_coercions: HashMap<NodeKey, CStringPointerCoercion>,
    pub node_trait_object_coercions: HashMap<NodeKey, TraitObjectCoercion>,
    pub node_trait_object_upcasts: HashMap<NodeKey, TraitObjectUpcast>,
    pub node_builtin_values: HashMap<NodeKey, BuiltinValue>,
    pub node_resolved_calls: HashMap<NodeKey, ResolvedCall>,
    pub node_function_references: HashMap<NodeKey, FunctionReference>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ComptimeIfSelection {
    Then,
    Else,
    None,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum BuiltinValue {
    Usize(u64),
    Layout {
        builtin: LayoutBuiltin,
        ty: InternedTyId,
    },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BracketSuffixResolution {
    Index,
    GenericCall,
    TypePrefixInstantiation,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ArrayToSliceCoercion {
    pub array_ty: InternedTyId,
    pub slice_ty: InternedTyId,
    pub is_readonly: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct CStringPointerCoercion {
    pub array_ty: InternedTyId,
    pub pointer_ty: InternedTyId,
    pub is_readonly: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct TraitObjectCoercion {
    pub source_ty: InternedTyId,
    pub target_ty: InternedTyId,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct TraitObjectUpcast {
    pub source_ty: InternedTyId,
    pub target_ty: InternedTyId,
}

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct GenericInstantiation {
    pub def_id: GlobalDefId,
    pub args: Vec<InternedTyId>,
    pub generics: Vec<String>,
    pub span: Span,
    pub source_def_id: Option<GlobalDefId>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FunctionReference {
    pub def_id: GlobalDefId,
    pub args: Vec<InternedTyId>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ResolvedCall {
    Function(GlobalDefId),
    FunctionInstance {
        def_id: GlobalDefId,
        args: Vec<InternedTyId>,
    },
    Method {
        def_id: GlobalDefId,
        args: Vec<InternedTyId>,
    },
    TraitMethod {
        trait_id: GlobalDefId,
        method_id: GlobalDefId,
        method_name: String,
        self_ty: InternedTyId,
        trait_args: Vec<InternedTyId>,
        args: Vec<InternedTyId>,
    },
    DynamicTraitMethod {
        object_ty: InternedTyId,
        trait_id: TraitId,
        method_id: GlobalDefId,
        method_name: String,
        trait_args: Vec<InternedTyId>,
        slot: usize,
        params: Vec<InternedTyId>,
        return_type: InternedTyId,
    },
    BuiltinTraitMethod {
        trait_id: BuiltinTrait,
        op: BuiltinOperatorOp,
    },
    BuiltinMethod {
        method: BuiltinMethod,
        self_ty: InternedTyId,
    },
    BuiltinPlaceMethod {
        trait_id: BuiltinTrait,
        method: BuiltinTraitMethod,
        self_ty: InternedTyId,
        trait_args: Vec<InternedTyId>,
    },
    FunctionPointer,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BuiltinMethod {
    Len,
    RangeIter,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BuiltinOperatorOp {
    Unary(UnaryOp),
    Binary(BinaryOp),
}

impl BuiltinOperatorOp {
    pub fn trait_id(self) -> Option<BuiltinTrait> {
        match self {
            Self::Unary(op) => match op {
                UnaryOp::Neg => Some(BuiltinTrait::Neg),
                UnaryOp::Not => Some(BuiltinTrait::Not),
                UnaryOp::BitNot => Some(BuiltinTrait::BitNot),
                UnaryOp::RefReadOnly | UnaryOp::Ref | UnaryOp::Deref => None,
            },
            Self::Binary(op) => match op {
                BinaryOp::Add => Some(BuiltinTrait::Add),
                BinaryOp::Sub => Some(BuiltinTrait::Sub),
                BinaryOp::Mul => Some(BuiltinTrait::Mul),
                BinaryOp::Div => Some(BuiltinTrait::Div),
                BinaryOp::Rem => Some(BuiltinTrait::Rem),
                BinaryOp::BitAnd => Some(BuiltinTrait::BitAnd),
                BinaryOp::BitOr => Some(BuiltinTrait::BitOr),
                BinaryOp::BitXor => Some(BuiltinTrait::BitXor),
                BinaryOp::Shl => Some(BuiltinTrait::Shl),
                BinaryOp::Shr => Some(BuiltinTrait::Shr),
                BinaryOp::Eq | BinaryOp::Ne => Some(BuiltinTrait::Eq),
                BinaryOp::Lt | BinaryOp::Le | BinaryOp::Gt | BinaryOp::Ge => {
                    Some(BuiltinTrait::Ord)
                }
                BinaryOp::And | BinaryOp::Or => None,
            },
        }
    }

    pub fn method(self) -> Option<BuiltinTraitMethod> {
        match self {
            Self::Unary(op) => match op {
                UnaryOp::Neg => Some(BuiltinTraitMethod::Neg),
                UnaryOp::Not => Some(BuiltinTraitMethod::Not),
                UnaryOp::BitNot => Some(BuiltinTraitMethod::BitNot),
                UnaryOp::RefReadOnly | UnaryOp::Ref | UnaryOp::Deref => None,
            },
            Self::Binary(op) => match op {
                BinaryOp::Add => Some(BuiltinTraitMethod::Add),
                BinaryOp::Sub => Some(BuiltinTraitMethod::Sub),
                BinaryOp::Mul => Some(BuiltinTraitMethod::Mul),
                BinaryOp::Div => Some(BuiltinTraitMethod::Div),
                BinaryOp::Rem => Some(BuiltinTraitMethod::Rem),
                BinaryOp::BitAnd => Some(BuiltinTraitMethod::BitAnd),
                BinaryOp::BitOr => Some(BuiltinTraitMethod::BitOr),
                BinaryOp::BitXor => Some(BuiltinTraitMethod::BitXor),
                BinaryOp::Shl => Some(BuiltinTraitMethod::Shl),
                BinaryOp::Shr => Some(BuiltinTraitMethod::Shr),
                BinaryOp::Eq => Some(BuiltinTraitMethod::Eq),
                BinaryOp::Ne => Some(BuiltinTraitMethod::Ne),
                BinaryOp::Lt => Some(BuiltinTraitMethod::Lt),
                BinaryOp::Le => Some(BuiltinTraitMethod::Le),
                BinaryOp::Gt => Some(BuiltinTraitMethod::Gt),
                BinaryOp::Ge => Some(BuiltinTraitMethod::Ge),
                BinaryOp::And | BinaryOp::Or => None,
            },
        }
    }

    pub fn from_method(method: BuiltinTraitMethod) -> Option<Self> {
        match method {
            BuiltinTraitMethod::Add => Some(Self::Binary(BinaryOp::Add)),
            BuiltinTraitMethod::Sub => Some(Self::Binary(BinaryOp::Sub)),
            BuiltinTraitMethod::Mul => Some(Self::Binary(BinaryOp::Mul)),
            BuiltinTraitMethod::Div => Some(Self::Binary(BinaryOp::Div)),
            BuiltinTraitMethod::Rem => Some(Self::Binary(BinaryOp::Rem)),
            BuiltinTraitMethod::Neg => Some(Self::Unary(UnaryOp::Neg)),
            BuiltinTraitMethod::Not => Some(Self::Unary(UnaryOp::Not)),
            BuiltinTraitMethod::BitNot => Some(Self::Unary(UnaryOp::BitNot)),
            BuiltinTraitMethod::BitAnd => Some(Self::Binary(BinaryOp::BitAnd)),
            BuiltinTraitMethod::BitOr => Some(Self::Binary(BinaryOp::BitOr)),
            BuiltinTraitMethod::BitXor => Some(Self::Binary(BinaryOp::BitXor)),
            BuiltinTraitMethod::Shl => Some(Self::Binary(BinaryOp::Shl)),
            BuiltinTraitMethod::Shr => Some(Self::Binary(BinaryOp::Shr)),
            BuiltinTraitMethod::Eq => Some(Self::Binary(BinaryOp::Eq)),
            BuiltinTraitMethod::Ne => Some(Self::Binary(BinaryOp::Ne)),
            BuiltinTraitMethod::Lt => Some(Self::Binary(BinaryOp::Lt)),
            BuiltinTraitMethod::Le => Some(Self::Binary(BinaryOp::Le)),
            BuiltinTraitMethod::Gt => Some(Self::Binary(BinaryOp::Gt)),
            BuiltinTraitMethod::Ge => Some(Self::Binary(BinaryOp::Ge)),
            BuiltinTraitMethod::DerefRead
            | BuiltinTraitMethod::Deref
            | BuiltinTraitMethod::IndexRead
            | BuiltinTraitMethod::Index
            | BuiltinTraitMethod::SliceRead
            | BuiltinTraitMethod::Slice
            | BuiltinTraitMethod::GetPtrRead
            | BuiltinTraitMethod::GetPtr => None,
        }
    }
}
