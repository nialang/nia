// SPDX-License-Identifier: GPL-3.0-or-later
use super::*;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub(super) struct FullBodyResolutionInputsQuery(pub(super) ModuleId);

impl QueryKey<CompilerContext> for FullBodyResolutionInputsQuery {
    type Value = BodyCheckResolutionInputs;

    const FINGERPRINT: QueryFingerprintPolicy = QueryFingerprintPolicy::SemanticValue;

    fn name() -> &'static str {
        "full_body_resolution_inputs"
    }

    fn description(&self) -> String {
        format!("full_body_resolution_inputs({:?})", self.0)
    }

    fn execute_result(&self, db: &QueryDb<CompilerContext>) -> QueryResult<Self::Value> {
        full_body_check_resolution_inputs(db, self.0)
    }

    fn values_equal(&self, old: &Self::Value, new: &Self::Value) -> bool {
        old == new
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub(super) struct ValueResolutionQuery(pub(super) ModuleId);

impl QueryKey<CompilerContext> for ValueResolutionQuery {
    type Value = ModuleValueResolution;

    const FINGERPRINT: QueryFingerprintPolicy = QueryFingerprintPolicy::SemanticValue;

    fn name() -> &'static str {
        "value_resolution"
    }

    fn execute_result(&self, db: &QueryDb<CompilerContext>) -> QueryResult<Self::Value> {
        (db.context().providers.value_resolution)(db, self.0)
    }

    fn values_equal(&self, old: &Self::Value, new: &Self::Value) -> bool {
        old == new
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub(super) struct LocalResolutionQuery(pub(super) ModuleId);

impl QueryKey<CompilerContext> for LocalResolutionQuery {
    type Value = ModuleLocalResolution;

    const FINGERPRINT: QueryFingerprintPolicy = QueryFingerprintPolicy::SemanticValue;

    fn name() -> &'static str {
        "local_resolution"
    }

    fn execute_result(&self, db: &QueryDb<CompilerContext>) -> QueryResult<Self::Value> {
        (db.context().providers.local_resolution)(db, self.0)
    }

    fn values_equal(&self, old: &Self::Value, new: &Self::Value) -> bool {
        old == new
    }
}

/// Constructor generic-argument diagnostics for a whole module.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub(super) struct ConstructorCheckQuery(pub(super) ModuleId);

impl QueryKey<CompilerContext> for ConstructorCheckQuery {
    type Value = nia_diagnostic::DiagnosticBundle;

    const FINGERPRINT: QueryFingerprintPolicy = QueryFingerprintPolicy::SemanticValue;

    fn name() -> &'static str {
        "constructor_check"
    }

    fn execute_result(&self, db: &QueryDb<CompilerContext>) -> QueryResult<Self::Value> {
        (db.context().providers.constructor_check)(db, self.0)
    }

    fn values_equal(&self, old: &Self::Value, new: &Self::Value) -> bool {
        old == new
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub(super) struct SemanticUseTableQuery(pub(super) ModuleId);

impl QueryKey<CompilerContext> for SemanticUseTableQuery {
    type Value = nia_sema_ir::SemanticUseTable;

    const FINGERPRINT: QueryFingerprintPolicy = QueryFingerprintPolicy::SemanticValue;

    fn name() -> &'static str {
        "semantic_use_table"
    }

    fn execute_result(&self, db: &QueryDb<CompilerContext>) -> QueryResult<Self::Value> {
        (db.context().providers.semantic_use_table)(db, self.0)
    }

    fn values_equal(&self, old: &Self::Value, new: &Self::Value) -> bool {
        old == new
    }
}
