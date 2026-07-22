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

    fn execute(&self, db: &QueryDb<CompilerContext>) -> Self::Value {
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

    fn execute(&self, db: &QueryDb<CompilerContext>) -> Self::Value {
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

    fn execute(&self, db: &QueryDb<CompilerContext>) -> Self::Value {
        (db.context().providers.monomorphization)(db)
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
