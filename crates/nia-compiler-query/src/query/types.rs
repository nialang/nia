// SPDX-License-Identifier: GPL-3.0-or-later
use super::*;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub(super) struct FrontendProgramSourcesQuery;

impl QueryKey<CompilerContext> for FrontendProgramSourcesQuery {
    type Value = Option<FrontendProgramSources>;

    fn name() -> &'static str {
        "frontend_program_sources"
    }

    fn execute_result(&self, db: &QueryDb<CompilerContext>) -> QueryResult<Self::Value> {
        db.context().frontend_program_sources(db)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub(super) struct TypeResolutionQuery(pub(super) ModuleId);

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub(super) struct DeclarationTypeResolutionQuery(pub(super) ModuleId);

impl QueryKey<CompilerContext> for TypeResolutionQuery {
    type Value = TypeResolution;

    fn name() -> &'static str {
        "type_resolution"
    }

    fn execute_result(&self, db: &QueryDb<CompilerContext>) -> QueryResult<Self::Value> {
        (db.context().providers.type_resolution)(db, self.0)
    }
}

impl QueryKey<CompilerContext> for DeclarationTypeResolutionQuery {
    type Value = TypeResolution;

    fn name() -> &'static str {
        "declaration_type_resolution"
    }

    fn description(&self) -> String {
        format!("declaration_type_resolution({:?})", self.0)
    }

    fn execute_result(&self, db: &QueryDb<CompilerContext>) -> QueryResult<Self::Value> {
        (db.context().providers.declaration_type_resolution)(db, self.0)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub(super) struct TypeLoweringQuery(pub(super) ModuleId);

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub(super) struct DeclarationTypeLoweringQuery(pub(super) ModuleId);

impl QueryKey<CompilerContext> for TypeLoweringQuery {
    type Value = TypeLowering;

    fn name() -> &'static str {
        "type_lowering"
    }

    fn description(&self) -> String {
        format!("type_lowering({:?})", self.0)
    }

    fn execute_result(&self, db: &QueryDb<CompilerContext>) -> QueryResult<Self::Value> {
        (db.context().providers.type_lowering)(db, self.0)
    }
}

impl QueryKey<CompilerContext> for DeclarationTypeLoweringQuery {
    type Value = TypeLowering;

    fn name() -> &'static str {
        "declaration_type_lowering"
    }

    fn description(&self) -> String {
        format!("declaration_type_lowering({:?})", self.0)
    }

    fn execute_result(&self, db: &QueryDb<CompilerContext>) -> QueryResult<Self::Value> {
        (db.context().providers.declaration_type_lowering)(db, self.0)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub(super) struct ItemSignaturesQuery(pub(super) ModuleId);

impl QueryKey<CompilerContext> for ItemSignaturesQuery {
    type Value = ItemSignatures;

    fn name() -> &'static str {
        "item_signatures"
    }

    fn description(&self) -> String {
        format!("item_signatures({:?})", self.0)
    }

    fn execute_result(&self, db: &QueryDb<CompilerContext>) -> QueryResult<Self::Value> {
        (db.context().providers.item_signatures)(db, self.0)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub(super) struct SignatureItemTreeQuery(
    pub(super) ModuleId,
    pub(super) nia_item_tree::SignatureItemSet,
);

impl QueryKey<CompilerContext> for SignatureItemTreeQuery {
    type Value = ActiveModuleItemTree;

    const FINGERPRINT: QueryFingerprintPolicy = QueryFingerprintPolicy::SemanticValue;

    fn name() -> &'static str {
        "signature_item_tree"
    }

    fn description(&self) -> String {
        format!("signature_item_tree({:?}, {:?})", self.0, self.1)
    }

    fn execute_result(&self, db: &QueryDb<CompilerContext>) -> QueryResult<Self::Value> {
        db.context().signature_item_tree(db, self.0, self.1)
    }

    fn values_equal(&self, old: &Self::Value, new: &Self::Value) -> bool {
        old == new
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub(super) struct SignatureTypeResolutionQuery(
    pub(super) ModuleId,
    pub(super) nia_item_tree::SignatureItemSet,
);

impl QueryKey<CompilerContext> for SignatureTypeResolutionQuery {
    type Value = TypeResolution;

    fn name() -> &'static str {
        "signature_type_resolution"
    }

    fn description(&self) -> String {
        format!("signature_type_resolution({:?}, {:?})", self.0, self.1)
    }

    fn execute_result(&self, db: &QueryDb<CompilerContext>) -> QueryResult<Self::Value> {
        (db.context().providers.signature_type_resolution)(db, self.0, self.1)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub(super) struct SignatureTypeLoweringQuery(
    pub(super) ModuleId,
    pub(super) nia_item_tree::SignatureItemSet,
);

impl QueryKey<CompilerContext> for SignatureTypeLoweringQuery {
    type Value = TypeLowering;

    fn name() -> &'static str {
        "signature_type_lowering"
    }

    fn description(&self) -> String {
        format!("signature_type_lowering({:?}, {:?})", self.0, self.1)
    }

    fn execute_result(&self, db: &QueryDb<CompilerContext>) -> QueryResult<Self::Value> {
        (db.context().providers.signature_type_lowering)(db, self.0, self.1)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub(super) struct SignatureItemSignaturesQuery(
    pub(super) ModuleId,
    pub(super) nia_item_tree::SignatureItemSet,
);

impl QueryKey<CompilerContext> for SignatureItemSignaturesQuery {
    type Value = ItemSignatures;

    fn name() -> &'static str {
        "signature_item_signatures"
    }

    fn description(&self) -> String {
        format!("signature_item_signatures({:?}, {:?})", self.0, self.1)
    }

    fn execute_result(&self, db: &QueryDb<CompilerContext>) -> QueryResult<Self::Value> {
        (db.context().providers.signature_item_signatures)(db, self.0, self.1)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub(super) struct SignatureTypeNormalizationQuery(
    pub(super) ModuleId,
    pub(super) nia_item_tree::SignatureItemSet,
);

impl QueryKey<CompilerContext> for SignatureTypeNormalizationQuery {
    type Value = TypeNormalization;

    fn name() -> &'static str {
        "signature_type_normalization"
    }

    fn description(&self) -> String {
        format!("signature_type_normalization({:?}, {:?})", self.0, self.1)
    }

    fn execute_result(&self, db: &QueryDb<CompilerContext>) -> QueryResult<Self::Value> {
        (db.context().providers.signature_type_normalization)(db, self.0, self.1)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub(super) struct SignatureConstItemTreeQuery(pub(super) ModuleId);

impl QueryKey<CompilerContext> for SignatureConstItemTreeQuery {
    type Value = ActiveModuleItemTree;

    const FINGERPRINT: QueryFingerprintPolicy = QueryFingerprintPolicy::SemanticValue;

    fn name() -> &'static str {
        "signature_const_item_tree"
    }

    fn description(&self) -> String {
        format!("signature_const_item_tree({:?})", self.0)
    }

    fn execute_result(&self, db: &QueryDb<CompilerContext>) -> QueryResult<Self::Value> {
        db.context().signature_const_item_tree(db, self.0)
    }

    fn values_equal(&self, old: &Self::Value, new: &Self::Value) -> bool {
        old == new
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub(super) struct SignatureConstTypeResolutionQuery(pub(super) ModuleId);

impl QueryKey<CompilerContext> for SignatureConstTypeResolutionQuery {
    type Value = TypeResolution;

    fn name() -> &'static str {
        "signature_const_type_resolution"
    }

    fn description(&self) -> String {
        format!("signature_const_type_resolution({:?})", self.0)
    }

    fn execute_result(&self, db: &QueryDb<CompilerContext>) -> QueryResult<Self::Value> {
        (db.context().providers.signature_const_type_resolution)(db, self.0)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub(super) struct SignatureConstTypeLoweringQuery(pub(super) ModuleId);

impl QueryKey<CompilerContext> for SignatureConstTypeLoweringQuery {
    type Value = TypeLowering;

    fn name() -> &'static str {
        "signature_const_type_lowering"
    }

    fn description(&self) -> String {
        format!("signature_const_type_lowering({:?})", self.0)
    }

    fn execute_result(&self, db: &QueryDb<CompilerContext>) -> QueryResult<Self::Value> {
        (db.context().providers.signature_const_type_lowering)(db, self.0)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub(super) struct SignatureConstItemSignaturesQuery(pub(super) ModuleId);

impl QueryKey<CompilerContext> for SignatureConstItemSignaturesQuery {
    type Value = ItemSignatures;

    fn name() -> &'static str {
        "signature_const_item_signatures"
    }

    fn description(&self) -> String {
        format!("signature_const_item_signatures({:?})", self.0)
    }

    fn execute_result(&self, db: &QueryDb<CompilerContext>) -> QueryResult<Self::Value> {
        (db.context().providers.signature_const_item_signatures)(db, self.0)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub(super) struct SignatureConstTypeNormalizationQuery(pub(super) ModuleId);

impl QueryKey<CompilerContext> for SignatureConstTypeNormalizationQuery {
    type Value = TypeNormalization;

    fn name() -> &'static str {
        "signature_const_type_normalization"
    }

    fn description(&self) -> String {
        format!("signature_const_type_normalization({:?})", self.0)
    }

    fn execute_result(&self, db: &QueryDb<CompilerContext>) -> QueryResult<Self::Value> {
        (db.context().providers.signature_const_type_normalization)(db, self.0)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub(super) struct SignatureConstModuleQuery(pub(super) ModuleId);

impl QueryKey<CompilerContext> for SignatureConstModuleQuery {
    type Value = ConstModuleLowering;

    fn name() -> &'static str {
        "signature_const_module"
    }

    fn description(&self) -> String {
        format!("signature_const_module({:?})", self.0)
    }

    fn execute_result(&self, db: &QueryDb<CompilerContext>) -> QueryResult<Self::Value> {
        (db.context().providers.signature_const_module)(db, self.0)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub(super) struct TypeNormalizationQuery(pub(super) ModuleId);

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub(super) struct LayoutTypeNormalizationQuery(pub(super) ModuleId);

impl QueryKey<CompilerContext> for TypeNormalizationQuery {
    type Value = TypeNormalization;

    fn name() -> &'static str {
        "type_normalization"
    }

    fn execute_result(&self, db: &QueryDb<CompilerContext>) -> QueryResult<Self::Value> {
        (db.context().providers.type_normalization)(db, self.0)
    }
}

impl QueryKey<CompilerContext> for LayoutTypeNormalizationQuery {
    type Value = TypeNormalization;

    fn name() -> &'static str {
        "layout_type_normalization"
    }

    fn description(&self) -> String {
        format!("layout_type_normalization({:?})", self.0)
    }

    fn execute_result(&self, db: &QueryDb<CompilerContext>) -> QueryResult<Self::Value> {
        (db.context().providers.layout_type_normalization)(db, self.0)
    }
}
