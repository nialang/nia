// SPDX-License-Identifier: GPL-3.0-or-later
use std::collections::HashMap;

use nia_ast::{BinaryOp, ReceiverKind, UnaryOp};
use nia_ids::{BuiltinTraitMethod, GlobalDefId, InternedTyId, LayoutBuiltin, LocalId, ModuleId};
use nia_node_id::NodeKey;
use nia_span::Span;
use nia_ty::{BuiltinTrait, IntConst, PrimitiveTy, TraitId};

#[derive(Debug, Clone, PartialEq, Default)]
pub struct SemanticUseTable {
    pub node_value_uses: HashMap<NodeKey, SemanticValueUse>,
    pub node_builtin_associated_values: HashMap<NodeKey, BuiltinAssociatedValue>,
    pub node_local_defs: HashMap<NodeKey, LocalId>,
    pub node_type_uses: HashMap<NodeKey, InternedTyId>,
}

impl SemanticUseTable {
    pub fn builder() -> SemanticUseTableBuilder {
        SemanticUseTableBuilder::new()
    }

    pub fn node_value_use(&self, key: &NodeKey) -> Option<SemanticValueUse> {
        self.node_value_uses.get(key).copied()
    }

    pub fn node_builtin_associated_value(&self, key: &NodeKey) -> Option<BuiltinAssociatedValue> {
        self.node_builtin_associated_values.get(key).copied()
    }

    pub fn node_local_def(&self, key: &NodeKey) -> Option<LocalId> {
        self.node_local_defs.get(key).copied()
    }

    pub fn node_type_use(&self, key: &NodeKey) -> Option<InternedTyId> {
        self.node_type_uses.get(key).copied()
    }
}

#[derive(Debug, Clone, PartialEq, Default)]
pub struct SemanticUseTableBuilder {
    table: SemanticUseTable,
}

impl SemanticUseTableBuilder {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn insert_node_local_value_use(&mut self, key: NodeKey, local_id: LocalId) {
        self.table
            .node_value_uses
            .insert(key, SemanticValueUse::Local(local_id));
    }

    pub fn insert_node_global_value_use(&mut self, key: NodeKey, global_id: GlobalDefId) {
        self.table
            .node_value_uses
            .entry(key)
            .or_insert(SemanticValueUse::Global(global_id));
    }

    pub fn insert_node_builtin_associated_value(
        &mut self,
        key: NodeKey,
        value: BuiltinAssociatedValue,
    ) {
        self.table.node_builtin_associated_values.insert(key, value);
    }

    pub fn extend_node_builtin_associated_values(
        &mut self,
        values: impl IntoIterator<Item = (NodeKey, BuiltinAssociatedValue)>,
    ) {
        self.table.node_builtin_associated_values.extend(values);
    }

    pub fn extend_node_global_value_uses(
        &mut self,
        value_uses: impl IntoIterator<Item = (NodeKey, GlobalDefId)>,
    ) {
        for (key, global_id) in value_uses {
            self.insert_node_global_value_use(key, global_id);
        }
    }

    pub fn insert_node_local_def(&mut self, key: NodeKey, local_id: LocalId) {
        self.table.node_local_defs.insert(key, local_id);
    }

    pub fn extend_node_local_defs(
        &mut self,
        local_defs: impl IntoIterator<Item = (NodeKey, LocalId)>,
    ) {
        self.table.node_local_defs.extend(local_defs);
    }

    pub fn insert_node_type_use(&mut self, key: NodeKey, ty: InternedTyId) {
        self.table.node_type_uses.insert(key, ty);
    }

    pub fn extend_node_type_uses(
        &mut self,
        type_uses: impl IntoIterator<Item = (NodeKey, InternedTyId)>,
    ) {
        self.table.node_type_uses.extend(type_uses);
    }

    pub fn finish(self) -> SemanticUseTable {
        self.table
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SemanticValueUse {
    Local(LocalId),
    Global(GlobalDefId),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BuiltinAssociatedValue {
    PrimitiveIntLimit {
        primitive: PrimitiveTy,
        kind: PrimitiveIntLimit,
    },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PrimitiveIntLimit {
    Min,
    Max,
}

impl PrimitiveIntLimit {
    pub fn value(self, primitive: PrimitiveTy, pointer_width: u32) -> Option<IntConst> {
        let (min, max) = primitive_int_range(primitive, pointer_width)?;
        Some(match self {
            PrimitiveIntLimit::Min => min,
            PrimitiveIntLimit::Max => max,
        })
    }
}

pub fn supports_primitive_int_limit(primitive: PrimitiveTy) -> bool {
    primitive_int_range(primitive, 64).is_some()
}

fn primitive_int_range(primitive: PrimitiveTy, pointer_width: u32) -> Option<(IntConst, IntConst)> {
    match primitive {
        PrimitiveTy::I8 => Some(signed_int_range(8)),
        PrimitiveTy::I16 => Some(signed_int_range(16)),
        PrimitiveTy::I32 => Some(signed_int_range(32)),
        PrimitiveTy::I64 => Some(signed_int_range(64)),
        PrimitiveTy::I128 => Some(signed_int_range(128)),
        PrimitiveTy::Isize => signed_integer_range(pointer_width),
        PrimitiveTy::U8 => Some(unsigned_int_range(8)),
        PrimitiveTy::U16 => Some(unsigned_int_range(16)),
        PrimitiveTy::U32 => Some(unsigned_int_range(32)),
        PrimitiveTy::U64 => Some(unsigned_int_range(64)),
        PrimitiveTy::U128 => Some(unsigned_int_range(128)),
        PrimitiveTy::Usize => unsigned_integer_range(pointer_width),
        PrimitiveTy::F32
        | PrimitiveTy::F64
        | PrimitiveTy::Bool
        | PrimitiveTy::Char
        | PrimitiveTy::Void
        | PrimitiveTy::Never => None,
    }
}

fn signed_integer_range(bits: u32) -> Option<(IntConst, IntConst)> {
    match bits {
        1..=128 => Some(signed_int_range(bits)),
        _ => None,
    }
}

fn unsigned_integer_range(bits: u32) -> Option<(IntConst, IntConst)> {
    match bits {
        1..=128 => Some(unsigned_int_range(bits)),
        _ => None,
    }
}

fn signed_int_range(bits: u32) -> (IntConst, IntConst) {
    let min_bits = 1u128 << (bits - 1);
    let mask = int_mask(bits);
    (
        IntConst::signed_bits(min_bits),
        IntConst::signed_bits(mask ^ min_bits),
    )
}

fn unsigned_int_range(bits: u32) -> (IntConst, IntConst) {
    (IntConst::unsigned(0), IntConst::unsigned(int_mask(bits)))
}

fn int_mask(bits: u32) -> u128 {
    if bits == 128 {
        u128::MAX
    } else {
        (1u128 << bits) - 1
    }
}

#[derive(Debug, Clone, PartialEq, Default)]
pub struct SemanticFacts {
    pub local_types: HashMap<LocalId, InternedTyId>,
    pub generic_instantiations: Vec<GenericInstantiation>,
    pub function_facts: HashMap<GlobalDefId, FunctionSemanticFacts>,
    pub node_expr_types: HashMap<NodeKey, InternedTyId>,
    pub node_bracket_suffix_resolutions: HashMap<NodeKey, BracketSuffixResolution>,
    pub node_array_to_slice_coercions: HashMap<NodeKey, ArrayToSliceCoercion>,
    pub node_c_string_pointer_coercions: HashMap<NodeKey, CStringPointerCoercion>,
    pub node_trait_object_coercions: HashMap<NodeKey, TraitObjectCoercion>,
    pub node_trait_object_upcasts: HashMap<NodeKey, TraitObjectUpcast>,
    pub node_comptime_if_selections: HashMap<NodeKey, ComptimeIfSelection>,
    pub node_builtin_values: HashMap<NodeKey, BuiltinValue>,
    pub node_builtin_associated_values: HashMap<NodeKey, BuiltinAssociatedValue>,
    pub node_array_repeat_counts: HashMap<NodeKey, u64>,
    pub node_switch_pattern_values: HashMap<NodeKey, i128>,
    pub node_resolved_calls: HashMap<NodeKey, ResolvedCall>,
    pub node_function_references: HashMap<NodeKey, FunctionReference>,
}

#[derive(Debug, Clone, PartialEq, Default)]
pub struct FunctionSemanticFacts {
    pub local_types: HashMap<LocalId, InternedTyId>,
    pub generic_instantiations: Vec<GenericInstantiation>,
    pub node_expr_types: HashMap<NodeKey, InternedTyId>,
    pub node_bracket_suffix_resolutions: HashMap<NodeKey, BracketSuffixResolution>,
    pub node_array_to_slice_coercions: HashMap<NodeKey, ArrayToSliceCoercion>,
    pub node_c_string_pointer_coercions: HashMap<NodeKey, CStringPointerCoercion>,
    pub node_trait_object_coercions: HashMap<NodeKey, TraitObjectCoercion>,
    pub node_trait_object_upcasts: HashMap<NodeKey, TraitObjectUpcast>,
    pub node_comptime_if_selections: HashMap<NodeKey, ComptimeIfSelection>,
    pub node_builtin_values: HashMap<NodeKey, BuiltinValue>,
    pub node_array_repeat_counts: HashMap<NodeKey, u64>,
    pub node_switch_pattern_values: HashMap<NodeKey, i128>,
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
    Int(IntConst),
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
    pub arg_module_id: ModuleId,
    pub args: Vec<InternedTyId>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ResolvedCall {
    Function(GlobalDefId),
    FunctionInstance {
        def_id: GlobalDefId,
        arg_module_id: ModuleId,
        args: Vec<InternedTyId>,
    },
    Method {
        def_id: GlobalDefId,
        args: Vec<InternedTyId>,
        receiver_kind: ReceiverKind,
    },
    TraitMethod {
        trait_id: GlobalDefId,
        method_id: GlobalDefId,
        method_name: String,
        self_ty: InternedTyId,
        trait_args: Vec<InternedTyId>,
        args: Vec<InternedTyId>,
        receiver_kind: ReceiverKind,
    },
    TraitAssociatedFunction {
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
        receiver_kind: ReceiverKind,
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
    Start,
    End,
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
            | BuiltinTraitMethod::GetPtr
            | BuiltinTraitMethod::Len
            | BuiltinTraitMethod::Start
            | BuiltinTraitMethod::End
            | BuiltinTraitMethod::IteratorNext => None,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use nia_ids::ModuleId;
    use nia_node_id::{NodeChildPath, SyntaxKind};
    use nia_source::{SourceId, SourceRevision, SourceVersion};

    fn key() -> NodeKey {
        NodeKey::child_path(
            SourceVersion {
                id: SourceId(0),
                revision: SourceRevision::INITIAL,
            },
            SyntaxKind::Expr,
            NodeChildPath::from_steps([0]),
        )
    }

    #[test]
    fn semantic_use_builder_keeps_local_value_uses_over_globals() {
        let mut builder = SemanticUseTable::builder();
        let key = key();
        builder.insert_node_local_value_use(key.clone(), LocalId(2));
        builder.insert_node_global_value_use(
            key.clone(),
            GlobalDefId {
                module_id: ModuleId(1),
                def_id: nia_ids::DefId(3),
            },
        );

        let table = builder.finish();

        assert_eq!(
            table.node_value_use(&key),
            Some(SemanticValueUse::Local(LocalId(2)))
        );
    }
}
