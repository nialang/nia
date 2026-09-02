// SPDX-License-Identifier: GPL-3.0-or-later
use crate::resolve::unresolved_error;
use crate::{ConstLowerError, ConstNameResolution};
use nia_ids::{InternedTyId, LocalId};
use nia_node_id::VersionedNodeKey;
use nia_sema_ir::{SemanticUseTable, SemanticValueUse};
use nia_span::Span;
use nia_symbol::{SymbolId, symbol_text_from_optional_resolver};
use nia_symbol_table::SymbolTable;
use std::collections::HashMap;

/// Optional semantic inputs for early lowering.
///
/// Missing tables are intentional: early IR records unresolved identities so
/// clients can lower syntax before the semantic pipeline is complete.
#[derive(Clone, Copy, Default)]
pub struct EarlyConstLowerInputs<'a> {
    /// Optional semantic use table for resolving names and locals.
    pub semantic_uses: Option<&'a SemanticUseTable>,
    /// Optional symbol table for synthesized diagnostic names.
    pub symbols: Option<&'a SymbolTable>,
    /// Type identities assigned to omitted aggregate constructors.
    pub omitted_aggregate_types: Option<&'a HashMap<VersionedNodeKey, InternedTyId>>,
    /// Variant identities assigned to omitted enum members.
    pub omitted_members: Option<&'a HashMap<VersionedNodeKey, nia_ids::GlobalDefId>>,
}

impl<'a> EarlyConstLowerInputs<'a> {
    /// Creates inputs with no semantic providers.
    pub fn new() -> Self {
        Self::default()
    }

    /// Adds semantic-use facts to the lowering context.
    pub fn with_semantic_uses(mut self, semantic_uses: &'a SemanticUseTable) -> Self {
        self.semantic_uses = Some(semantic_uses);
        self
    }

    /// Adds a symbol table for synthesized names.
    pub fn with_symbols(mut self, symbols: &'a SymbolTable) -> Self {
        self.symbols = Some(symbols);
        self
    }

    /// Adds semantic identities for omitted constructors.
    pub fn with_omitted_constructor_maps(
        mut self,
        aggregate_types: &'a HashMap<VersionedNodeKey, InternedTyId>,
        members: &'a HashMap<VersionedNodeKey, nia_ids::GlobalDefId>,
    ) -> Self {
        self.omitted_aggregate_types = Some(aggregate_types);
        self.omitted_members = Some(members);
        self
    }
}

/// Required semantic inputs for producing resolved const IR.
///
/// The symbol table remains optional because most symbols are already interned;
/// it is required only when lowering syntax that creates a symbol dynamically,
/// such as the implicit `self` name or `offset`'s string field argument.
#[derive(Clone, Copy)]
pub struct ResolvedConstLowerInputs<'a> {
    /// Required semantic use facts for identity-complete lowering.
    pub semantic_uses: &'a SemanticUseTable,
    /// Optional symbol table for synthesized names.
    pub symbols: Option<&'a SymbolTable>,
    /// Type identities assigned to omitted aggregate constructors.
    pub omitted_aggregate_types: Option<&'a HashMap<VersionedNodeKey, InternedTyId>>,
    /// Variant identities assigned to omitted enum members.
    pub omitted_members: Option<&'a HashMap<VersionedNodeKey, nia_ids::GlobalDefId>>,
}

impl<'a> ResolvedConstLowerInputs<'a> {
    /// Creates resolved-lowering inputs from semantic-use facts.
    pub fn new(semantic_uses: &'a SemanticUseTable) -> Self {
        Self {
            semantic_uses,
            symbols: None,
            omitted_aggregate_types: None,
            omitted_members: None,
        }
    }

    /// Adds a symbol table for synthesized names.
    pub fn with_symbols(mut self, symbols: &'a SymbolTable) -> Self {
        self.symbols = Some(symbols);
        self
    }

    /// Adds semantic identities for omitted constructors.
    pub fn with_omitted_constructor_maps(
        mut self,
        aggregate_types: &'a HashMap<VersionedNodeKey, InternedTyId>,
        members: &'a HashMap<VersionedNodeKey, nia_ids::GlobalDefId>,
    ) -> Self {
        self.omitted_aggregate_types = Some(aggregate_types);
        self.omitted_members = Some(members);
        self
    }
}

pub(super) trait ConstLowerContext {
    fn has_semantic_facts(&self) -> bool;

    fn probe_name_resolution(&self, key: &VersionedNodeKey) -> Option<ConstNameResolution>;

    fn probe_type_id(&self, key: &VersionedNodeKey) -> Option<InternedTyId>;

    fn probe_type_prefix(&self, key: &VersionedNodeKey) -> Option<nia_ids::GlobalDefId>;

    fn probe_omitted_aggregate_type(&self, key: &VersionedNodeKey) -> Option<InternedTyId>;

    fn probe_omitted_member(&self, key: &VersionedNodeKey) -> Option<nia_ids::GlobalDefId>;

    fn resolve_name(
        &self,
        key: &VersionedNodeKey,
        span: Span,
    ) -> Result<Option<ConstNameResolution>, ConstLowerError>;

    fn lower_local_use(
        &self,
        key: &VersionedNodeKey,
        span: Span,
    ) -> Result<Option<LocalId>, ConstLowerError>;

    fn lower_local_id(
        &self,
        key: &VersionedNodeKey,
        span: Span,
    ) -> Result<Option<LocalId>, ConstLowerError>;

    fn lower_type_id(
        &self,
        key: &VersionedNodeKey,
        span: Span,
    ) -> Result<Option<InternedTyId>, ConstLowerError>;

    fn intern_name(&self, text: &str, span: Span) -> Result<Option<SymbolId>, ConstLowerError>;

    fn symbol_name(&self, symbol: SymbolId) -> String {
        symbol_text_from_optional_resolver(None, symbol)
    }
}

// A syntax node can acquire several semantic identities over the pipeline.
// Keep this precedence centralized so early and resolved lowering cannot
// disagree about associated constants, const generics, or ordinary values.
fn semantic_name_resolution(
    semantic_uses: &SemanticUseTable,
    key: &VersionedNodeKey,
) -> Option<ConstNameResolution> {
    semantic_uses
        .node_associated_const_projection(key)
        .cloned()
        .map(ConstNameResolution::AssociatedConstProjection)
        .or_else(|| {
            semantic_uses
                .node_builtin_associated_value(key)
                .map(ConstNameResolution::BuiltinAssociatedValue)
        })
        .or_else(|| {
            semantic_uses
                .node_const_generic_use(key)
                .map(|name| ConstNameResolution::GenericParam(*name))
        })
        .or_else(|| {
            semantic_uses
                .node_value_use(key)
                .map(ConstNameResolution::from)
        })
}

impl ConstLowerContext for EarlyConstLowerInputs<'_> {
    fn has_semantic_facts(&self) -> bool {
        self.semantic_uses.is_some()
    }

    fn probe_name_resolution(&self, key: &VersionedNodeKey) -> Option<ConstNameResolution> {
        self.semantic_uses
            .and_then(|semantic_uses| semantic_name_resolution(semantic_uses, key))
    }

    fn probe_type_id(&self, key: &VersionedNodeKey) -> Option<InternedTyId> {
        self.semantic_uses
            .and_then(|semantic_uses| semantic_uses.node_type_use(key))
    }

    fn probe_type_prefix(&self, key: &VersionedNodeKey) -> Option<nia_ids::GlobalDefId> {
        self.semantic_uses
            .and_then(|semantic_uses| semantic_uses.node_type_prefix(key))
    }

    fn probe_omitted_aggregate_type(&self, key: &VersionedNodeKey) -> Option<InternedTyId> {
        self.omitted_aggregate_types
            .and_then(|map| map.get(key).copied())
    }

    fn probe_omitted_member(&self, key: &VersionedNodeKey) -> Option<nia_ids::GlobalDefId> {
        self.omitted_members.and_then(|map| map.get(key).copied())
    }

    fn resolve_name(
        &self,
        key: &VersionedNodeKey,
        _span: Span,
    ) -> Result<Option<ConstNameResolution>, ConstLowerError> {
        Ok(self.probe_name_resolution(key))
    }

    fn lower_local_use(
        &self,
        key: &VersionedNodeKey,
        _span: Span,
    ) -> Result<Option<LocalId>, ConstLowerError> {
        Ok(self
            .semantic_uses
            .and_then(|semantic_uses| semantic_uses.node_value_use(key))
            .and_then(|value_use| match value_use {
                SemanticValueUse::Local(local_id) => Some(local_id),
                SemanticValueUse::Global(_) => None,
            }))
    }

    fn lower_local_id(
        &self,
        key: &VersionedNodeKey,
        _span: Span,
    ) -> Result<Option<LocalId>, ConstLowerError> {
        Ok(self
            .semantic_uses
            .and_then(|semantic_uses| semantic_uses.node_local_def(key)))
    }

    fn lower_type_id(
        &self,
        key: &VersionedNodeKey,
        _span: Span,
    ) -> Result<Option<InternedTyId>, ConstLowerError> {
        Ok(self
            .semantic_uses
            .and_then(|semantic_uses| semantic_uses.node_type_use(key)))
    }

    fn intern_name(&self, text: &str, span: Span) -> Result<Option<SymbolId>, ConstLowerError> {
        self.symbols
            .map(|symbols| {
                symbols.intern(text).map_err(|collision| ConstLowerError {
                    span,
                    message: collision.to_string(),
                })
            })
            .transpose()
    }

    fn symbol_name(&self, symbol: SymbolId) -> String {
        symbol_text_from_optional_resolver(self.symbols.map(|symbols| symbols as _), symbol)
    }
}

impl ConstLowerContext for ResolvedConstLowerInputs<'_> {
    fn has_semantic_facts(&self) -> bool {
        true
    }

    fn probe_name_resolution(&self, key: &VersionedNodeKey) -> Option<ConstNameResolution> {
        semantic_name_resolution(self.semantic_uses, key)
    }

    fn probe_type_id(&self, key: &VersionedNodeKey) -> Option<InternedTyId> {
        self.semantic_uses.node_type_use(key)
    }

    fn probe_type_prefix(&self, key: &VersionedNodeKey) -> Option<nia_ids::GlobalDefId> {
        self.semantic_uses.node_type_prefix(key)
    }

    fn probe_omitted_aggregate_type(&self, key: &VersionedNodeKey) -> Option<InternedTyId> {
        self.omitted_aggregate_types
            .and_then(|map| map.get(key).copied())
    }

    fn probe_omitted_member(&self, key: &VersionedNodeKey) -> Option<nia_ids::GlobalDefId> {
        self.omitted_members.and_then(|map| map.get(key).copied())
    }

    fn resolve_name(
        &self,
        key: &VersionedNodeKey,
        span: Span,
    ) -> Result<Option<ConstNameResolution>, ConstLowerError> {
        self.probe_name_resolution(key)
            .map(Some)
            .ok_or_else(|| unresolved_error(span, "const name"))
    }

    fn lower_local_use(
        &self,
        key: &VersionedNodeKey,
        span: Span,
    ) -> Result<Option<LocalId>, ConstLowerError> {
        match self.semantic_uses.node_value_use(key) {
            Some(SemanticValueUse::Local(local_id)) => Ok(Some(local_id)),
            Some(SemanticValueUse::Global(_)) | None => {
                Err(unresolved_error(span, "const assignment target"))
            }
        }
    }

    fn lower_local_id(
        &self,
        key: &VersionedNodeKey,
        span: Span,
    ) -> Result<Option<LocalId>, ConstLowerError> {
        self.semantic_uses
            .node_local_def(key)
            .map(Some)
            .ok_or_else(|| unresolved_error(span, "const local binding"))
    }

    fn lower_type_id(
        &self,
        key: &VersionedNodeKey,
        span: Span,
    ) -> Result<Option<InternedTyId>, ConstLowerError> {
        self.semantic_uses
            .node_type_use(key)
            .map(Some)
            .ok_or_else(|| unresolved_error(span, "const type"))
    }

    fn intern_name(&self, text: &str, span: Span) -> Result<Option<SymbolId>, ConstLowerError> {
        let Some(symbols) = self.symbols else {
            return Err(ConstLowerError {
                span,
                message: "const lowering requires a symbol table for dynamic field names"
                    .to_string(),
            });
        };
        symbols
            .intern(text)
            .map(Some)
            .map_err(|collision| ConstLowerError {
                span,
                message: collision.to_string(),
            })
    }

    fn symbol_name(&self, symbol: SymbolId) -> String {
        symbol_text_from_optional_resolver(self.symbols.map(|symbols| symbols as _), symbol)
    }
}
