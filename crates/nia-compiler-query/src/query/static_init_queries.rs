// SPDX-License-Identifier: GPL-3.0-or-later
use super::*;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub(super) struct ExecutableStaticInitQuery(pub(super) GlobalDefId);

impl QueryKey<CompilerContext> for ExecutableStaticInitQuery {
    type Value = Option<Arc<nia_static_ir::StaticInit>>;

    const FINGERPRINT: QueryFingerprintPolicy = QueryFingerprintPolicy::SemanticValue;

    fn name() -> &'static str {
        "executable_static_init"
    }

    fn description(&self) -> String {
        format!("executable_static_init({:?})", self.0)
    }

    fn execute(&self, db: &QueryDb<CompilerContext>) -> Self::Value {
        db.get(ExecutableCheckedModuleFactsQuery)
            .modules
            .iter()
            .find(|module| module.id == self.0.module_id)
            .and_then(|module| module.body_ir.global_inits.get(&self.0))
            .cloned()
    }

    fn values_equal(&self, old: &Self::Value, new: &Self::Value) -> bool {
        old == new
    }
}

#[derive(Debug, Clone)]
pub(super) struct StaticInitHandle {
    pub(super) def_id: GlobalDefId,
    pub(super) value: Arc<Option<Arc<nia_static_ir::StaticInit>>>,
}
