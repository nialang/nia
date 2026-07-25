// SPDX-License-Identifier: GPL-3.0-or-later
use super::*;

#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) struct BackendModuleSourceItemPlan {
    pub(super) functions: Vec<GlobalDefId>,
    pub(super) globals: Vec<GlobalDefId>,
    pub(super) structs: Vec<GlobalDefId>,
    pub(super) unions: Vec<GlobalDefId>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub(super) struct BackendModuleSourceItemPlanQuery(pub(super) ModuleId);

impl QueryKey<CompilerContext> for BackendModuleSourceItemPlanQuery {
    type Value = BackendModuleSourceItemPlan;

    const FINGERPRINT: QueryFingerprintPolicy = QueryFingerprintPolicy::SemanticValue;

    fn name() -> &'static str {
        "backend_module_source_item_plan"
    }

    fn description(&self) -> String {
        format!("backend_module_source_item_plan({:?})", self.0)
    }

    fn execute_result(&self, db: &QueryDb<CompilerContext>) -> QueryResult<Self::Value> {
        provide_backend_module_source_item_plan(db, self.0)
    }

    fn values_equal(&self, old: &Self::Value, new: &Self::Value) -> bool {
        old == new
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) struct BackendModuleFunctionInstancePlan {
    pub(super) instances: Vec<nia_backend_lower::BackendFunctionInstancePlan>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub(super) struct BackendModuleFunctionInstancePlanQuery(pub(super) ModuleId);

impl QueryKey<CompilerContext> for BackendModuleFunctionInstancePlanQuery {
    type Value = BackendModuleFunctionInstancePlan;

    const FINGERPRINT: QueryFingerprintPolicy = QueryFingerprintPolicy::SemanticValue;

    fn name() -> &'static str {
        "backend_module_function_instance_plan"
    }

    fn description(&self) -> String {
        format!("backend_module_function_instance_plan({:?})", self.0)
    }

    fn execute_result(&self, db: &QueryDb<CompilerContext>) -> QueryResult<Self::Value> {
        provide_backend_module_function_instance_plan(db, self.0)
    }

    fn values_equal(&self, old: &Self::Value, new: &Self::Value) -> bool {
        old == new
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub(super) struct MonomorphizationQuery;

impl QueryKey<CompilerContext> for MonomorphizationQuery {
    type Value = nia_monomorphize::Monomorphization;

    fn name() -> &'static str {
        "monomorphization"
    }

    fn execute_result(&self, db: &QueryDb<CompilerContext>) -> QueryResult<Self::Value> {
        (db.context().providers.monomorphization)(db)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub(super) struct BackendLoweringInputsQuery;

impl QueryKey<CompilerContext> for BackendLoweringInputsQuery {
    type Value = Result<BackendLoweringInputs, Vec<Diagnostic>>;

    fn name() -> &'static str {
        "backend_lowering_inputs"
    }

    fn execute_result(&self, db: &QueryDb<CompilerContext>) -> QueryResult<Self::Value> {
        provide_backend_lowering_inputs(db)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub(super) struct BackendFinalizationTaskContextQuery;

impl QueryKey<CompilerContext> for BackendFinalizationTaskContextQuery {
    type Value = BackendFinalizationTaskContext;

    fn name() -> &'static str {
        "backend_finalization_task_context"
    }

    fn execute(&self, db: &QueryDb<CompilerContext>) -> Self::Value {
        provide_backend_finalization_task_context(db)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub(super) struct BackendItemPlanQuery;

impl QueryKey<CompilerContext> for BackendItemPlanQuery {
    type Value = nia_backend_lower::BackendItemPlan;

    const STORAGE: QueryStoragePolicy = QueryStoragePolicy::SingleConsumerOwned;

    fn name() -> &'static str {
        "backend_item_plan"
    }

    fn execute(&self, db: &QueryDb<CompilerContext>) -> Self::Value {
        provide_backend_item_plan(db)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub(super) struct BackendModuleItemPlanQuery(pub(super) ModuleId);

impl QueryKey<CompilerContext> for BackendModuleItemPlanQuery {
    type Value = nia_backend_lower::BackendModuleItemPlan;

    const STORAGE: QueryStoragePolicy = QueryStoragePolicy::SingleConsumerOwned;
    const PROVIDER: QueryProviderPolicy = QueryProviderPolicy::ExternallyPublished;

    fn name() -> &'static str {
        "backend_module_item_plan"
    }

    fn description(&self) -> String {
        format!("backend_module_item_plan({:?})", self.0)
    }

    fn execute(&self, _db: &QueryDb<CompilerContext>) -> Self::Value {
        unreachable!("backend module item plans are published by backend lowering")
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub(super) struct BackendModuleFinalizationQuery {
    pub(super) module_id: ModuleId,
    pub(super) position: usize,
}

impl QueryKey<CompilerContext> for BackendModuleFinalizationQuery {
    type Value = nia_backend_lower::BackendModuleFinalization;

    const STORAGE: QueryStoragePolicy = QueryStoragePolicy::SingleConsumerOwned;

    fn name() -> &'static str {
        "backend_module_finalization"
    }

    fn description(&self) -> String {
        format!(
            "backend_module_finalization({:?}, {})",
            self.module_id, self.position
        )
    }

    fn execute(&self, db: &QueryDb<CompilerContext>) -> Self::Value {
        provide_backend_module_finalization(db, *self)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub(super) struct BackendLoweringQuery;

impl QueryKey<CompilerContext> for BackendLoweringQuery {
    type Value = nia_backend_lower::BackendLowering;

    fn name() -> &'static str {
        "backend_lowering"
    }

    fn execute(&self, db: &QueryDb<CompilerContext>) -> Self::Value {
        (db.context().providers.backend_lowering)(db)
    }
}
