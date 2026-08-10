// SPDX-License-Identifier: GPL-3.0-or-later
use std::collections::{HashMap, HashSet};

use nia_ast::{BinaryOp, UnaryOp};
use nia_ids::{
    BuiltinFunction, BuiltinTraitMethod, GlobalDefId, InternedTyId, LayoutBuiltin, LocalId,
    ModuleId, ReceiverKind,
};
use nia_node_id::{NodeMap, NodeMapBuilder, NodeStore, NodeStoreId, VersionedNodeKey};
use nia_span::Span;
use nia_symbol::SymbolId;
use nia_ty::{BuiltinTrait, ConstGenericArg, IntConst, PrimitiveTy, TraitId};

#[derive(Debug, Clone, PartialEq)]
pub struct SemanticUseTable {
    pub node_value_uses: NodeMap<SemanticValueUse>,
    pub node_const_generic_uses: NodeMap<SymbolId>,
    pub node_builtin_associated_values: NodeMap<BuiltinAssociatedValue>,
    pub node_associated_const_projections: NodeMap<AssociatedConstProjection>,
    pub node_local_defs: NodeMap<LocalId>,
    pub node_type_uses: NodeMap<InternedTyId>,
    pub node_type_prefixes: NodeMap<GlobalDefId>,
}

impl Default for SemanticUseTable {
    fn default() -> Self {
        Self::builder().finish()
    }
}

impl SemanticUseTable {
    pub fn builder() -> SemanticUseTableBuilder {
        SemanticUseTableBuilder::new()
    }

    pub fn builder_with_node_store(store: &NodeStore) -> SemanticUseTableBuilder {
        SemanticUseTableBuilder::with_node_store(store)
    }

    pub fn store_id(&self) -> NodeStoreId {
        self.node_value_uses.store_id()
    }

    pub fn node_store(&self) -> &NodeStore {
        self.node_value_uses.node_store()
    }

    pub fn node_value_use(&self, key: &VersionedNodeKey) -> Option<SemanticValueUse> {
        self.node_value_uses.get(key).copied()
    }

    pub fn node_builtin_associated_value(
        &self,
        key: &VersionedNodeKey,
    ) -> Option<BuiltinAssociatedValue> {
        self.node_builtin_associated_values.get(key).copied()
    }

    pub fn node_associated_const_projection(
        &self,
        key: &VersionedNodeKey,
    ) -> Option<&AssociatedConstProjection> {
        self.node_associated_const_projections.get(key)
    }

    pub fn node_const_generic_use(&self, key: &VersionedNodeKey) -> Option<&SymbolId> {
        self.node_const_generic_uses.get(key)
    }

    pub fn node_local_def(&self, key: &VersionedNodeKey) -> Option<LocalId> {
        self.node_local_defs.get(key).copied()
    }

    pub fn node_type_use(&self, key: &VersionedNodeKey) -> Option<InternedTyId> {
        self.node_type_uses.get(key).copied()
    }

    pub fn node_type_prefix(&self, key: &VersionedNodeKey) -> Option<GlobalDefId> {
        self.node_type_prefixes.get(key).copied()
    }
}

#[derive(Debug)]
pub struct SemanticUseTableBuilder {
    node_value_uses: NodeMapBuilder<SemanticValueUse>,
    node_const_generic_uses: NodeMapBuilder<SymbolId>,
    node_builtin_associated_values: NodeMapBuilder<BuiltinAssociatedValue>,
    node_associated_const_projections: NodeMapBuilder<AssociatedConstProjection>,
    node_local_defs: NodeMapBuilder<LocalId>,
    node_type_uses: NodeMapBuilder<InternedTyId>,
    node_type_prefixes: NodeMapBuilder<GlobalDefId>,
}

impl SemanticUseTableBuilder {
    pub fn new() -> Self {
        Self::with_node_store(&NodeStore::new())
    }

    pub fn with_node_store(store: &NodeStore) -> Self {
        Self {
            node_value_uses: NodeMap::builder(store),
            node_const_generic_uses: NodeMap::builder(store),
            node_builtin_associated_values: NodeMap::builder(store),
            node_associated_const_projections: NodeMap::builder(store),
            node_local_defs: NodeMap::builder(store),
            node_type_uses: NodeMap::builder(store),
            node_type_prefixes: NodeMap::builder(store),
        }
    }

    pub fn insert_node_local_value_use(&mut self, key: VersionedNodeKey, local_id: LocalId) {
        self.node_value_uses
            .insert(key, SemanticValueUse::Local(local_id));
    }

    pub fn insert_node_global_value_use(&mut self, key: VersionedNodeKey, global_id: GlobalDefId) {
        self.node_value_uses
            .insert_if_absent(key, SemanticValueUse::Global(global_id));
    }

    pub fn insert_node_const_generic_use(&mut self, key: VersionedNodeKey, name: SymbolId) {
        self.node_const_generic_uses.insert(key, name);
    }

    pub fn extend_node_const_generic_uses(
        &mut self,
        uses: impl IntoIterator<Item = (VersionedNodeKey, SymbolId)>,
    ) {
        self.node_const_generic_uses.extend(uses);
    }

    pub fn insert_node_builtin_associated_value(
        &mut self,
        key: VersionedNodeKey,
        value: BuiltinAssociatedValue,
    ) {
        self.node_builtin_associated_values.insert(key, value);
    }

    pub fn insert_node_associated_const_projection(
        &mut self,
        key: VersionedNodeKey,
        projection: AssociatedConstProjection,
    ) {
        self.node_associated_const_projections
            .insert(key, projection);
    }

    pub fn extend_node_associated_const_projections(
        &mut self,
        projections: impl IntoIterator<Item = (VersionedNodeKey, AssociatedConstProjection)>,
    ) {
        self.node_associated_const_projections.extend(projections);
    }

    pub fn extend_node_builtin_associated_values(
        &mut self,
        values: impl IntoIterator<Item = (VersionedNodeKey, BuiltinAssociatedValue)>,
    ) {
        self.node_builtin_associated_values.extend(values);
    }

    pub fn extend_node_global_value_uses(
        &mut self,
        value_uses: impl IntoIterator<Item = (VersionedNodeKey, GlobalDefId)>,
    ) {
        for (key, global_id) in value_uses {
            self.insert_node_global_value_use(key, global_id);
        }
    }

    pub fn insert_node_local_def(&mut self, key: VersionedNodeKey, local_id: LocalId) {
        self.node_local_defs.insert(key, local_id);
    }

    pub fn extend_node_local_defs(
        &mut self,
        local_defs: impl IntoIterator<Item = (VersionedNodeKey, LocalId)>,
    ) {
        self.node_local_defs.extend(local_defs);
    }

    pub fn insert_node_type_use(&mut self, key: VersionedNodeKey, ty: InternedTyId) {
        self.node_type_uses.insert(key, ty);
    }

    pub fn extend_node_type_uses(
        &mut self,
        type_uses: impl IntoIterator<Item = (VersionedNodeKey, InternedTyId)>,
    ) {
        self.node_type_uses.extend(type_uses);
    }

    pub fn insert_node_type_prefix(&mut self, key: VersionedNodeKey, def_id: GlobalDefId) {
        self.node_type_prefixes.insert(key, def_id);
    }

    pub fn extend_node_type_prefixes(
        &mut self,
        prefixes: impl IntoIterator<Item = (VersionedNodeKey, GlobalDefId)>,
    ) {
        self.node_type_prefixes.extend(prefixes);
    }

    pub fn finish(self) -> SemanticUseTable {
        SemanticUseTable {
            node_value_uses: self.node_value_uses.finish(),
            node_const_generic_uses: self.node_const_generic_uses.finish(),
            node_builtin_associated_values: self.node_builtin_associated_values.finish(),
            node_associated_const_projections: self.node_associated_const_projections.finish(),
            node_local_defs: self.node_local_defs.finish(),
            node_type_uses: self.node_type_uses.finish(),
            node_type_prefixes: self.node_type_prefixes.finish(),
        }
    }
}

impl Default for SemanticUseTableBuilder {
    fn default() -> Self {
        Self::new()
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SemanticValueUse {
    Local(LocalId),
    Global(GlobalDefId),
}

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct AssociatedConstProjection {
    pub self_ty: InternedTyId,
    pub trait_id: TraitId,
    pub trait_args: Vec<InternedTyId>,
    pub trait_const_args: Vec<ConstGenericArg>,
    pub name: SymbolId,
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

#[derive(Debug, Clone, PartialEq)]
pub struct SemanticFacts {
    pub global_types: HashMap<GlobalDefId, InternedTyId>,
    pub const_types: HashMap<GlobalDefId, InternedTyId>,
    /// Instantiations owned by module-level facts. Function body instantiations live in
    /// `function_facts`; use `iter_generic_instantiations` when both owners are relevant.
    pub generic_instantiations: Vec<GenericInstantiation>,
    pub function_facts: HashMap<GlobalDefId, FunctionSemanticFacts>,
    /// Node facts owned by module-level expressions. Function body node facts live in
    /// `function_facts`; use the `iter_node_*` methods when both owners are relevant.
    pub node_expr_types: NodeMap<InternedTyId>,
    pub node_bracket_suffix_resolutions: NodeMap<BracketSuffixResolution>,
    pub node_pointer_array_to_slice_coercions: NodeMap<PointerArrayToSliceCoercion>,
    pub node_trait_object_coercions: NodeMap<TraitObjectCoercion>,
    pub node_trait_object_upcasts: NodeMap<TraitObjectUpcast>,
    pub node_builtin_values: NodeMap<BuiltinValue>,
    pub node_builtin_associated_values: NodeMap<BuiltinAssociatedValue>,
    pub node_associated_const_projections: NodeMap<AssociatedConstProjection>,
    pub node_array_repeat_counts: NodeMap<u64>,
    pub node_pattern_values: NodeMap<i128>,
    pub node_resolved_calls: NodeMap<ResolvedCall>,
    pub node_function_references: NodeMap<FunctionReference>,
}

#[derive(Debug, Clone, PartialEq, Default)]
pub struct SemanticFactsBuilder {
    pub global_types: HashMap<GlobalDefId, InternedTyId>,
    pub const_types: HashMap<GlobalDefId, InternedTyId>,
    pub generic_instantiations: Vec<GenericInstantiation>,
    pub function_facts: HashMap<GlobalDefId, FunctionSemanticFacts>,
    pub node_expr_types: HashMap<VersionedNodeKey, InternedTyId>,
    pub node_bracket_suffix_resolutions: HashMap<VersionedNodeKey, BracketSuffixResolution>,
    pub node_pointer_array_to_slice_coercions:
        HashMap<VersionedNodeKey, PointerArrayToSliceCoercion>,
    pub node_trait_object_coercions: HashMap<VersionedNodeKey, TraitObjectCoercion>,
    pub node_trait_object_upcasts: HashMap<VersionedNodeKey, TraitObjectUpcast>,
    pub node_builtin_values: HashMap<VersionedNodeKey, BuiltinValue>,
    pub node_builtin_associated_values: HashMap<VersionedNodeKey, BuiltinAssociatedValue>,
    pub node_associated_const_projections: HashMap<VersionedNodeKey, AssociatedConstProjection>,
    pub node_array_repeat_counts: HashMap<VersionedNodeKey, u64>,
    pub node_pattern_values: HashMap<VersionedNodeKey, i128>,
    pub node_resolved_calls: HashMap<VersionedNodeKey, ResolvedCall>,
    pub node_function_references: HashMap<VersionedNodeKey, FunctionReference>,
}

impl Default for SemanticFacts {
    fn default() -> Self {
        SemanticFactsBuilder::default().finish(&NodeStore::new())
    }
}

impl SemanticFacts {
    pub fn with_node_store(store: &NodeStore) -> Self {
        SemanticFactsBuilder::default().finish(store)
    }

    pub fn store_id(&self) -> NodeStoreId {
        self.node_expr_types.store_id()
    }

    pub fn node_store(&self) -> &NodeStore {
        self.node_expr_types.node_store()
    }

    pub fn extend(&mut self, facts: Self) {
        let node_store = self.node_store().clone();
        self.global_types.extend(facts.global_types);
        self.const_types.extend(facts.const_types);
        self.generic_instantiations
            .extend(facts.generic_instantiations);
        self.function_facts.extend(
            facts
                .function_facts
                .into_iter()
                .map(|(def_id, facts)| (def_id, facts.into_node_store(&node_store))),
        );
        extend_node_map(&mut self.node_expr_types, facts.node_expr_types);
        extend_node_map(
            &mut self.node_bracket_suffix_resolutions,
            facts.node_bracket_suffix_resolutions,
        );
        extend_node_map(
            &mut self.node_pointer_array_to_slice_coercions,
            facts.node_pointer_array_to_slice_coercions,
        );
        extend_node_map(
            &mut self.node_trait_object_coercions,
            facts.node_trait_object_coercions,
        );
        extend_node_map(
            &mut self.node_trait_object_upcasts,
            facts.node_trait_object_upcasts,
        );
        extend_node_map(&mut self.node_builtin_values, facts.node_builtin_values);
        extend_node_map(
            &mut self.node_builtin_associated_values,
            facts.node_builtin_associated_values,
        );
        extend_node_map(
            &mut self.node_associated_const_projections,
            facts.node_associated_const_projections,
        );
        extend_node_map(
            &mut self.node_array_repeat_counts,
            facts.node_array_repeat_counts,
        );
        extend_node_map(&mut self.node_pattern_values, facts.node_pattern_values);
        extend_node_map(&mut self.node_resolved_calls, facts.node_resolved_calls);
        extend_node_map(
            &mut self.node_function_references,
            facts.node_function_references,
        );
    }

    pub fn into_builder(self) -> SemanticFactsBuilder {
        SemanticFactsBuilder {
            global_types: self.global_types,
            const_types: self.const_types,
            generic_instantiations: self.generic_instantiations,
            function_facts: self.function_facts,
            node_expr_types: self.node_expr_types.into_entries().collect(),
            node_bracket_suffix_resolutions: self
                .node_bracket_suffix_resolutions
                .into_entries()
                .collect(),
            node_pointer_array_to_slice_coercions: self
                .node_pointer_array_to_slice_coercions
                .into_entries()
                .collect(),
            node_trait_object_coercions: self.node_trait_object_coercions.into_entries().collect(),
            node_trait_object_upcasts: self.node_trait_object_upcasts.into_entries().collect(),
            node_builtin_values: self.node_builtin_values.into_entries().collect(),
            node_builtin_associated_values: self
                .node_builtin_associated_values
                .into_entries()
                .collect(),
            node_associated_const_projections: self
                .node_associated_const_projections
                .into_entries()
                .collect(),
            node_array_repeat_counts: self.node_array_repeat_counts.into_entries().collect(),
            node_pattern_values: self.node_pattern_values.into_entries().collect(),
            node_resolved_calls: self.node_resolved_calls.into_entries().collect(),
            node_function_references: self.node_function_references.into_entries().collect(),
        }
    }

    pub fn iter_generic_instantiations(&self) -> impl Iterator<Item = &GenericInstantiation> + '_ {
        self.generic_instantiations.iter().chain(
            self.function_facts
                .values()
                .flat_map(|facts| facts.generic_instantiations.iter()),
        )
    }

    pub fn node_expr_type(&self, key: &VersionedNodeKey) -> Option<InternedTyId> {
        self.node_expr_types.get(key).copied().or_else(|| {
            self.function_facts
                .values()
                .find_map(|facts| facts.node_expr_types.get(key).copied())
        })
    }

    pub fn iter_node_expr_types(
        &self,
    ) -> impl Iterator<Item = (VersionedNodeKey, &InternedTyId)> + '_ {
        self.node_expr_types.iter().chain(
            self.function_facts
                .values()
                .flat_map(|facts| facts.node_expr_types.iter()),
        )
    }

    pub fn iter_node_bracket_suffix_resolutions(
        &self,
    ) -> impl Iterator<Item = (VersionedNodeKey, &BracketSuffixResolution)> + '_ {
        self.node_bracket_suffix_resolutions.iter().chain(
            self.function_facts
                .values()
                .flat_map(|facts| facts.node_bracket_suffix_resolutions.iter()),
        )
    }

    pub fn iter_node_pointer_array_to_slice_coercions(
        &self,
    ) -> impl Iterator<Item = (VersionedNodeKey, &PointerArrayToSliceCoercion)> + '_ {
        self.node_pointer_array_to_slice_coercions.iter().chain(
            self.function_facts
                .values()
                .flat_map(|facts| facts.node_pointer_array_to_slice_coercions.iter()),
        )
    }

    pub fn iter_node_trait_object_coercions(
        &self,
    ) -> impl Iterator<Item = (VersionedNodeKey, &TraitObjectCoercion)> + '_ {
        self.node_trait_object_coercions.iter().chain(
            self.function_facts
                .values()
                .flat_map(|facts| facts.node_trait_object_coercions.iter()),
        )
    }

    pub fn iter_node_trait_object_upcasts(
        &self,
    ) -> impl Iterator<Item = (VersionedNodeKey, &TraitObjectUpcast)> + '_ {
        self.node_trait_object_upcasts.iter().chain(
            self.function_facts
                .values()
                .flat_map(|facts| facts.node_trait_object_upcasts.iter()),
        )
    }

    pub fn iter_node_builtin_values(
        &self,
    ) -> impl Iterator<Item = (VersionedNodeKey, &BuiltinValue)> + '_ {
        self.node_builtin_values.iter().chain(
            self.function_facts
                .values()
                .flat_map(|facts| facts.node_builtin_values.iter()),
        )
    }

    pub fn iter_node_associated_const_projections(
        &self,
    ) -> impl Iterator<Item = (VersionedNodeKey, &AssociatedConstProjection)> + '_ {
        self.node_associated_const_projections.iter().chain(
            self.function_facts
                .values()
                .flat_map(|facts| facts.node_associated_const_projections.iter()),
        )
    }

    pub fn iter_node_array_repeat_counts(
        &self,
    ) -> impl Iterator<Item = (VersionedNodeKey, &u64)> + '_ {
        self.node_array_repeat_counts.iter().chain(
            self.function_facts
                .values()
                .flat_map(|facts| facts.node_array_repeat_counts.iter()),
        )
    }

    pub fn iter_node_pattern_values(&self) -> impl Iterator<Item = (VersionedNodeKey, &i128)> + '_ {
        self.node_pattern_values.iter().chain(
            self.function_facts
                .values()
                .flat_map(|facts| facts.node_pattern_values.iter()),
        )
    }

    pub fn iter_node_resolved_calls(
        &self,
    ) -> impl Iterator<Item = (VersionedNodeKey, &ResolvedCall)> + '_ {
        self.node_resolved_calls.iter().chain(
            self.function_facts
                .values()
                .flat_map(|facts| facts.node_resolved_calls.iter()),
        )
    }

    pub fn iter_node_function_references(
        &self,
    ) -> impl Iterator<Item = (VersionedNodeKey, &FunctionReference)> + '_ {
        self.node_function_references.iter().chain(
            self.function_facts
                .values()
                .flat_map(|facts| facts.node_function_references.iter()),
        )
    }
}

impl SemanticFactsBuilder {
    pub fn retain_module_level_facts(&mut self) {
        self.generic_instantiations
            .retain(|instantiation| instantiation.source_def_id.is_none());
        for facts in self.function_facts.values() {
            for key in facts.node_expr_types.keys() {
                self.node_expr_types.remove(&key);
            }
            for key in facts.node_bracket_suffix_resolutions.keys() {
                self.node_bracket_suffix_resolutions.remove(&key);
            }
            for key in facts.node_pointer_array_to_slice_coercions.keys() {
                self.node_pointer_array_to_slice_coercions.remove(&key);
            }
            for key in facts.node_trait_object_coercions.keys() {
                self.node_trait_object_coercions.remove(&key);
            }
            for key in facts.node_trait_object_upcasts.keys() {
                self.node_trait_object_upcasts.remove(&key);
            }
            for key in facts.node_builtin_values.keys() {
                self.node_builtin_values.remove(&key);
            }
            for key in facts.node_associated_const_projections.keys() {
                self.node_associated_const_projections.remove(&key);
            }
            for key in facts.node_array_repeat_counts.keys() {
                self.node_array_repeat_counts.remove(&key);
            }
            for key in facts.node_pattern_values.keys() {
                self.node_pattern_values.remove(&key);
            }
            for key in facts.node_resolved_calls.keys() {
                self.node_resolved_calls.remove(&key);
            }
            for key in facts.node_function_references.keys() {
                self.node_function_references.remove(&key);
            }
        }
    }

    pub fn finish(self, store: &NodeStore) -> SemanticFacts {
        SemanticFacts {
            global_types: self.global_types,
            const_types: self.const_types,
            generic_instantiations: self.generic_instantiations,
            function_facts: self
                .function_facts
                .into_iter()
                .map(|(def_id, facts)| (def_id, facts.into_node_store(store)))
                .collect(),
            node_expr_types: node_map_from_entries(store, self.node_expr_types),
            node_bracket_suffix_resolutions: node_map_from_entries(
                store,
                self.node_bracket_suffix_resolutions,
            ),
            node_pointer_array_to_slice_coercions: node_map_from_entries(
                store,
                self.node_pointer_array_to_slice_coercions,
            ),
            node_trait_object_coercions: node_map_from_entries(
                store,
                self.node_trait_object_coercions,
            ),
            node_trait_object_upcasts: node_map_from_entries(store, self.node_trait_object_upcasts),
            node_builtin_values: node_map_from_entries(store, self.node_builtin_values),
            node_builtin_associated_values: node_map_from_entries(
                store,
                self.node_builtin_associated_values,
            ),
            node_associated_const_projections: node_map_from_entries(
                store,
                self.node_associated_const_projections,
            ),
            node_array_repeat_counts: node_map_from_entries(store, self.node_array_repeat_counts),
            node_pattern_values: node_map_from_entries(store, self.node_pattern_values),
            node_resolved_calls: node_map_from_entries(store, self.node_resolved_calls),
            node_function_references: node_map_from_entries(store, self.node_function_references),
        }
    }
}

#[derive(Debug, Clone, PartialEq)]
pub struct FunctionSemanticFacts {
    pub local_types: HashMap<LocalId, InternedTyId>,
    pub global_value_uses: HashSet<GlobalDefId>,
    pub generic_instantiations: Vec<GenericInstantiation>,
    pub node_expr_types: NodeMap<InternedTyId>,
    pub node_bracket_suffix_resolutions: NodeMap<BracketSuffixResolution>,
    pub node_pointer_array_to_slice_coercions: NodeMap<PointerArrayToSliceCoercion>,
    pub node_trait_object_coercions: NodeMap<TraitObjectCoercion>,
    pub node_trait_object_upcasts: NodeMap<TraitObjectUpcast>,
    pub node_builtin_values: NodeMap<BuiltinValue>,
    pub node_associated_const_projections: NodeMap<AssociatedConstProjection>,
    pub node_array_repeat_counts: NodeMap<u64>,
    pub node_pattern_values: NodeMap<i128>,
    pub node_resolved_calls: NodeMap<ResolvedCall>,
    pub node_function_references: NodeMap<FunctionReference>,
    pub trait_method_refs: Vec<SemanticTraitMethodRef>,
}

#[derive(Debug, Clone, PartialEq, Default)]
pub struct FunctionSemanticFactsBuilder {
    pub local_types: HashMap<LocalId, InternedTyId>,
    pub global_value_uses: HashSet<GlobalDefId>,
    pub generic_instantiations: Vec<GenericInstantiation>,
    pub node_expr_types: HashMap<VersionedNodeKey, InternedTyId>,
    pub node_bracket_suffix_resolutions: HashMap<VersionedNodeKey, BracketSuffixResolution>,
    pub node_pointer_array_to_slice_coercions:
        HashMap<VersionedNodeKey, PointerArrayToSliceCoercion>,
    pub node_trait_object_coercions: HashMap<VersionedNodeKey, TraitObjectCoercion>,
    pub node_trait_object_upcasts: HashMap<VersionedNodeKey, TraitObjectUpcast>,
    pub node_builtin_values: HashMap<VersionedNodeKey, BuiltinValue>,
    pub node_associated_const_projections: HashMap<VersionedNodeKey, AssociatedConstProjection>,
    pub node_array_repeat_counts: HashMap<VersionedNodeKey, u64>,
    pub node_pattern_values: HashMap<VersionedNodeKey, i128>,
    pub node_resolved_calls: HashMap<VersionedNodeKey, ResolvedCall>,
    pub node_function_references: HashMap<VersionedNodeKey, FunctionReference>,
    pub trait_method_refs: Vec<SemanticTraitMethodRef>,
}

impl Default for FunctionSemanticFacts {
    fn default() -> Self {
        FunctionSemanticFactsBuilder::default().finish(&NodeStore::new())
    }
}

impl FunctionSemanticFacts {
    pub fn store_id(&self) -> NodeStoreId {
        self.node_expr_types.store_id()
    }

    pub fn into_builder(self) -> FunctionSemanticFactsBuilder {
        FunctionSemanticFactsBuilder {
            local_types: self.local_types,
            global_value_uses: self.global_value_uses,
            generic_instantiations: self.generic_instantiations,
            node_expr_types: self.node_expr_types.into_entries().collect(),
            node_bracket_suffix_resolutions: self
                .node_bracket_suffix_resolutions
                .into_entries()
                .collect(),
            node_pointer_array_to_slice_coercions: self
                .node_pointer_array_to_slice_coercions
                .into_entries()
                .collect(),
            node_trait_object_coercions: self.node_trait_object_coercions.into_entries().collect(),
            node_trait_object_upcasts: self.node_trait_object_upcasts.into_entries().collect(),
            node_builtin_values: self.node_builtin_values.into_entries().collect(),
            node_associated_const_projections: self
                .node_associated_const_projections
                .into_entries()
                .collect(),
            node_array_repeat_counts: self.node_array_repeat_counts.into_entries().collect(),
            node_pattern_values: self.node_pattern_values.into_entries().collect(),
            node_resolved_calls: self.node_resolved_calls.into_entries().collect(),
            node_function_references: self.node_function_references.into_entries().collect(),
            trait_method_refs: self.trait_method_refs,
        }
    }

    fn into_node_store(self, store: &NodeStore) -> Self {
        if self.store_id() == store.id() {
            self
        } else {
            self.into_builder().finish(store)
        }
    }
}

impl FunctionSemanticFactsBuilder {
    pub fn finish(self, store: &NodeStore) -> FunctionSemanticFacts {
        FunctionSemanticFacts {
            local_types: self.local_types,
            global_value_uses: self.global_value_uses,
            generic_instantiations: self.generic_instantiations,
            node_expr_types: node_map_from_entries(store, self.node_expr_types),
            node_bracket_suffix_resolutions: node_map_from_entries(
                store,
                self.node_bracket_suffix_resolutions,
            ),
            node_pointer_array_to_slice_coercions: node_map_from_entries(
                store,
                self.node_pointer_array_to_slice_coercions,
            ),
            node_trait_object_coercions: node_map_from_entries(
                store,
                self.node_trait_object_coercions,
            ),
            node_trait_object_upcasts: node_map_from_entries(store, self.node_trait_object_upcasts),
            node_builtin_values: node_map_from_entries(store, self.node_builtin_values),
            node_associated_const_projections: node_map_from_entries(
                store,
                self.node_associated_const_projections,
            ),
            node_array_repeat_counts: node_map_from_entries(store, self.node_array_repeat_counts),
            node_pattern_values: node_map_from_entries(store, self.node_pattern_values),
            node_resolved_calls: node_map_from_entries(store, self.node_resolved_calls),
            node_function_references: node_map_from_entries(store, self.node_function_references),
            trait_method_refs: self.trait_method_refs,
        }
    }
}

fn node_map_from_entries<V>(
    store: &NodeStore,
    entries: HashMap<VersionedNodeKey, V>,
) -> NodeMap<V> {
    let mut builder = NodeMap::builder(store);
    builder.extend(entries);
    builder.finish()
}

fn extend_node_map<V>(target: &mut NodeMap<V>, source: NodeMap<V>) {
    let store = target.node_store().clone();
    let mut builder = std::mem::replace(target, NodeMap::with_store(&store)).into_builder();
    builder.extend_map(source);
    *target = builder.finish();
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SemanticTraitMethodRef {
    pub module_id: ModuleId,
    pub trait_id: TraitId,
    pub method_name: SymbolId,
    pub self_ty: InternedTyId,
    pub trait_args: Vec<InternedTyId>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum BuiltinValue {
    Int(IntConst),
    Usize(u64),
    Layout {
        builtin: LayoutBuiltin,
        ty: InternedTyId,
    },
    FieldOffset {
        ty: InternedTyId,
        field: GlobalDefId,
    },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BracketSuffixResolution {
    Index,
    GenericCall,
    TypePrefixInstantiation,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct PointerArrayToSliceCoercion {
    pub pointer_ty: InternedTyId,
    pub array_ty: InternedTyId,
    pub slice_ty: InternedTyId,
    pub is_readonly: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct TraitObjectCoercion {
    pub source_ty: InternedTyId,
    pub target_ty: InternedTyId,
    pub self_ty: InternedTyId,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct TraitObjectUpcast {
    pub source_ty: InternedTyId,
    pub target_ty: InternedTyId,
}

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct GenericInstantiation {
    pub def_id: GlobalDefId,
    pub self_arg: Option<InternedTyId>,
    pub args: Vec<InternedTyId>,
    pub const_args: Vec<ConstGenericArg>,
    pub generics: Vec<SymbolId>,
    pub span: Span,
    pub source_def_id: Option<GlobalDefId>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FunctionReference {
    pub def_id: GlobalDefId,
    pub arg_module_id: ModuleId,
    pub args: Vec<InternedTyId>,
    pub const_args: Vec<ConstGenericArg>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ResolvedCall {
    BuiltinFunction {
        builtin: BuiltinFunction,
        type_arg: Option<InternedTyId>,
    },
    Function(GlobalDefId),
    FunctionInstance {
        def_id: GlobalDefId,
        arg_module_id: ModuleId,
        args: Vec<InternedTyId>,
        const_args: Vec<ConstGenericArg>,
    },
    Method {
        def_id: GlobalDefId,
        args: Vec<InternedTyId>,
        receiver_kind: ReceiverKind,
    },
    TraitMethod {
        trait_id: GlobalDefId,
        method_id: GlobalDefId,
        method_name: SymbolId,
        self_ty: InternedTyId,
        trait_args: Vec<InternedTyId>,
        args: Vec<InternedTyId>,
        receiver_kind: ReceiverKind,
    },
    TraitAssociatedFunction {
        trait_id: GlobalDefId,
        method_id: GlobalDefId,
        method_name: SymbolId,
        self_ty: InternedTyId,
        trait_args: Vec<InternedTyId>,
        args: Vec<InternedTyId>,
    },
    DynamicTraitMethod {
        object_ty: InternedTyId,
        trait_id: TraitId,
        method_id: GlobalDefId,
        method_name: SymbolId,
        trait_args: Vec<InternedTyId>,
        slot: usize,
        params: Vec<InternedTyId>,
        return_type: InternedTyId,
        receiver_kind: ReceiverKind,
    },
    BuiltinTraitMethod {
        trait_id: BuiltinTrait,
        op: BuiltinOperatorOp,
        self_ty: InternedTyId,
        trait_args: Vec<InternedTyId>,
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
    Closure,
    Callable,
    FunctionPointer,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BuiltinMethod {
    SliceLen,
    SlicePtr,
    SlicePtrMut,
    Start,
    End,
    Iter,
}

impl BuiltinMethod {
    pub fn name(self) -> &'static str {
        match self {
            Self::SliceLen => "sliceLen",
            Self::SlicePtr => "ptr",
            Self::SlicePtrMut => "ptrMut",
            Self::Start => "start",
            Self::End => "end",
            Self::Iter => "iter",
        }
    }

    /// Whether the const evaluator implements this compiler intrinsic method.
    pub fn is_const_capable(self) -> bool {
        matches!(
            self,
            Self::SliceLen
                | Self::SlicePtr
                | Self::SlicePtrMut
                | Self::Start
                | Self::End
                | Self::Iter
        )
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BuiltinOperatorOp {
    Unary(UnaryOp),
    Binary(BinaryOp),
}

impl BuiltinOperatorOp {
    /// Value operators are lowered directly by the const evaluator. Reference
    /// and dereference operations are represented as ordinary unary AST forms,
    /// not as this value-operator dispatch.
    pub fn is_const_capable(self) -> bool {
        match self {
            Self::Unary(
                UnaryOp::Neg
                | UnaryOp::Not
                | UnaryOp::BitNot
                | UnaryOp::RefReadOnly
                | UnaryOp::Ref
                | UnaryOp::Deref,
            )
            | Self::Binary(
                BinaryOp::Mul
                | BinaryOp::Div
                | BinaryOp::Rem
                | BinaryOp::Add
                | BinaryOp::Sub
                | BinaryOp::Shl
                | BinaryOp::Shr
                | BinaryOp::Lt
                | BinaryOp::Le
                | BinaryOp::Gt
                | BinaryOp::Ge
                | BinaryOp::Eq
                | BinaryOp::Ne
                | BinaryOp::BitAnd
                | BinaryOp::BitXor
                | BinaryOp::BitOr
                | BinaryOp::And
                | BinaryOp::Or,
            ) => true,
        }
    }
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
            BuiltinTraitMethod::Deref
            | BuiltinTraitMethod::DerefMut
            | BuiltinTraitMethod::Index
            | BuiltinTraitMethod::IndexMut
            | BuiltinTraitMethod::Slice
            | BuiltinTraitMethod::SliceMut
            | BuiltinTraitMethod::IterableIter
            | BuiltinTraitMethod::IteratorNext => None,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use nia_ids::ModuleIdAllocator;
    use nia_node_id::{NodeChildPath, SyntaxKind};
    use nia_source::{SourceId, SourceRevision, SourceVersion};

    fn key() -> VersionedNodeKey {
        key_at(0)
    }

    fn key_at(step: u32) -> VersionedNodeKey {
        VersionedNodeKey::child_path(
            SourceVersion {
                id: SourceId(0),
                revision: SourceRevision::INITIAL,
            },
            SyntaxKind::Expr,
            NodeChildPath::from_steps([step]),
        )
    }

    #[test]
    fn semantic_use_builder_keeps_local_value_uses_over_globals() {
        let module_id = ModuleIdAllocator::new().allocate();
        let mut builder = SemanticUseTable::builder();
        let key = key();
        builder.insert_node_local_value_use(key.clone(), LocalId(2));
        builder.insert_node_global_value_use(
            key.clone(),
            GlobalDefId {
                module_id,
                def_id: nia_ids::DefId(3),
            },
        );

        let table = builder.finish();

        assert_eq!(
            table.node_value_use(&key),
            Some(SemanticValueUse::Local(LocalId(2)))
        );
    }

    #[test]
    fn semantic_use_maps_share_the_supplied_node_owner() {
        let store = NodeStore::new();
        let mut builder = SemanticUseTable::builder_with_node_store(&store);
        builder.insert_node_local_value_use(key(), LocalId(1));
        let table = builder.finish();

        assert_eq!(table.store_id(), store.id());
        assert_eq!(table.node_const_generic_uses.store_id(), store.id());
        assert_eq!(table.node_builtin_associated_values.store_id(), store.id());
        assert_eq!(
            table.node_associated_const_projections.store_id(),
            store.id()
        );
        assert_eq!(table.node_local_defs.store_id(), store.id());
        assert_eq!(table.node_type_uses.store_id(), store.id());
    }

    #[test]
    fn semantic_use_tables_compare_by_locator_across_node_owners() {
        let first_store = NodeStore::new();
        let second_store = NodeStore::new();
        let mut first = SemanticUseTable::builder_with_node_store(&first_store);
        let mut second = SemanticUseTable::builder_with_node_store(&second_store);
        first.insert_node_local_def(key(), LocalId(4));
        second.insert_node_local_def(key(), LocalId(4));

        assert_eq!(first.finish(), second.finish());
    }

    #[test]
    fn function_facts_freeze_and_thaw_node_maps_at_explicit_boundaries() {
        let module_id = ModuleIdAllocator::new().allocate();
        let type_store = nia_ty::TypeStore::new();
        let ty = type_store
            .append_for_module(module_id)
            .primitive(PrimitiveTy::I32);
        let first_store = NodeStore::new();
        let mut builder = FunctionSemanticFactsBuilder::default();
        builder.node_expr_types.insert(key(), ty);
        let facts = builder.finish(&first_store);

        assert_eq!(facts.store_id(), first_store.id());
        assert_eq!(
            facts.node_bracket_suffix_resolutions.store_id(),
            first_store.id()
        );
        assert_eq!(
            facts.node_pointer_array_to_slice_coercions.store_id(),
            first_store.id()
        );
        assert_eq!(
            facts.node_trait_object_coercions.store_id(),
            first_store.id()
        );
        assert_eq!(facts.node_trait_object_upcasts.store_id(), first_store.id());
        assert_eq!(facts.node_builtin_values.store_id(), first_store.id());
        assert_eq!(
            facts.node_associated_const_projections.store_id(),
            first_store.id()
        );
        assert_eq!(facts.node_array_repeat_counts.store_id(), first_store.id());
        assert_eq!(facts.node_pattern_values.store_id(), first_store.id());
        assert_eq!(facts.node_resolved_calls.store_id(), first_store.id());
        assert_eq!(facts.node_function_references.store_id(), first_store.id());
        assert_eq!(facts.node_expr_types.get(&key()), Some(&ty));

        let second_store = NodeStore::new();
        let rebuilt = facts.clone().into_builder().finish(&second_store);
        assert_ne!(facts.store_id(), rebuilt.store_id());
        assert_eq!(facts, rebuilt);
    }

    #[test]
    fn semantic_facts_freeze_merge_and_rehome_all_node_maps() {
        let module_id = ModuleIdAllocator::new().allocate();
        let type_store = nia_ty::TypeStore::new();
        let ty = type_store
            .append_for_module(module_id)
            .primitive(PrimitiveTy::I32);
        let first_store = NodeStore::new();
        let second_store = NodeStore::new();
        let mut first = SemanticFactsBuilder::default();
        first.node_expr_types.insert(key_at(0), ty);
        let mut second = SemanticFactsBuilder::default();
        second.node_expr_types.insert(key_at(1), ty);
        let mut function = FunctionSemanticFactsBuilder::default();
        function.node_expr_types.insert(key_at(2), ty);
        second.function_facts.insert(
            GlobalDefId {
                module_id,
                def_id: nia_ids::DefId(1),
            },
            function.finish(&second_store),
        );
        let mut facts = first.finish(&first_store);
        facts.extend(second.finish(&second_store));

        assert_eq!(facts.store_id(), first_store.id());
        assert_eq!(
            facts.node_bracket_suffix_resolutions.store_id(),
            first_store.id()
        );
        assert_eq!(
            facts.node_pointer_array_to_slice_coercions.store_id(),
            first_store.id()
        );
        assert_eq!(
            facts.node_trait_object_coercions.store_id(),
            first_store.id()
        );
        assert_eq!(facts.node_trait_object_upcasts.store_id(), first_store.id());
        assert_eq!(facts.node_builtin_values.store_id(), first_store.id());
        assert_eq!(
            facts.node_builtin_associated_values.store_id(),
            first_store.id()
        );
        assert_eq!(
            facts.node_associated_const_projections.store_id(),
            first_store.id()
        );
        assert_eq!(facts.node_array_repeat_counts.store_id(), first_store.id());
        assert_eq!(facts.node_pattern_values.store_id(), first_store.id());
        assert_eq!(facts.node_resolved_calls.store_id(), first_store.id());
        assert_eq!(facts.node_function_references.store_id(), first_store.id());
        assert_eq!(facts.node_expr_types.get(&key_at(0)), Some(&ty));
        assert_eq!(facts.node_expr_types.get(&key_at(1)), Some(&ty));
        assert!(
            facts
                .function_facts
                .values()
                .all(|function| function.store_id() == first_store.id())
        );

        let rebuilt = facts.clone().into_builder().finish(&second_store);
        assert_ne!(facts.store_id(), rebuilt.store_id());
        assert_eq!(facts, rebuilt);
    }
}
