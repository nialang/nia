// SPDX-License-Identifier: GPL-3.0-or-later
use std::collections::HashMap;

use nia_ids::{GlobalDefId, InternedTyId, LayoutBuiltin, LocalId};
use nia_node_id::NodeKey;
use nia_span::Span;

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
    pub function_references: HashMap<Span, FunctionReference>,
    pub generic_instantiations: Vec<GenericInstantiation>,
    pub node_expr_types: HashMap<NodeKey, InternedTyId>,
    pub node_bracket_suffix_resolutions: HashMap<NodeKey, BracketSuffixResolution>,
    pub node_array_to_slice_coercions: HashMap<NodeKey, ArrayToSliceCoercion>,
    pub node_c_string_pointer_coercions: HashMap<NodeKey, CStringPointerCoercion>,
    pub node_trait_object_coercions: HashMap<NodeKey, TraitObjectCoercion>,
    pub node_trait_object_upcasts: HashMap<NodeKey, TraitObjectUpcast>,
    pub node_builtin_values: HashMap<NodeKey, BuiltinValue>,
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
