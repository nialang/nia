// SPDX-License-Identifier: GPL-3.0-or-later
use super::*;

#[derive(Debug, Clone, PartialEq)]
pub(in crate::query) struct ExecutableValueRefItemInput {
    pub(in crate::query) active_item_tree: Arc<ActiveModuleItemTree>,
    pub(in crate::query) item_index: usize,
    pub(in crate::query) owner_node_key: nia_node_id::VersionedNodeKey,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub(super) struct CheckedModuleQuery(pub(super) ModuleId);

impl QueryKey<CompilerContext> for CheckedModuleQuery {
    type Value = CheckedModule;

    fn name() -> &'static str {
        "checked_module"
    }

    fn execute(&self, db: &QueryDb<CompilerContext>) -> Self::Value {
        (db.context().providers.checked_module)(db, self.0)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub(super) struct CheckedModuleIdsQuery;

impl QueryKey<CompilerContext> for CheckedModuleIdsQuery {
    type Value = Vec<ModuleId>;

    fn name() -> &'static str {
        "checked_module_ids"
    }

    fn execute(&self, db: &QueryDb<CompilerContext>) -> Self::Value {
        (db.context().providers.checked_module_ids)(db)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub(super) struct ExecutableCheckedModuleSetQuery;

impl QueryKey<CompilerContext> for ExecutableCheckedModuleSetQuery {
    type Value = ExecutableCheckedModuleSet;

    fn name() -> &'static str {
        "executable_checked_module_set"
    }

    fn execute(&self, db: &QueryDb<CompilerContext>) -> Self::Value {
        provide_executable_checked_module_set(db)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub(super) struct ExecutableValueRefItemQuery(pub(super) GlobalDefId);

impl QueryKey<CompilerContext> for ExecutableValueRefItemQuery {
    type Value = Option<Arc<ExecutableValueRefItemInput>>;

    fn name() -> &'static str {
        "executable_value_ref_item"
    }

    fn execute(&self, db: &QueryDb<CompilerContext>) -> Self::Value {
        db.context().executable_value_ref_item(self.0)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub(super) struct ExecutableValueRefEdgesQuery(pub(super) GlobalDefId);

impl QueryKey<CompilerContext> for ExecutableValueRefEdgesQuery {
    type Value = ExecutableValueRefEdges;

    fn name() -> &'static str {
        "executable_value_ref_edges"
    }

    fn execute(&self, db: &QueryDb<CompilerContext>) -> Self::Value {
        provide_executable_value_ref_edges(db, self.0)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
#[cfg(test)]
pub(super) struct ExecutableCheckedModulesQuery;

#[cfg(test)]
impl QueryKey<CompilerContext> for ExecutableCheckedModulesQuery {
    type Value = Vec<Arc<CheckedModule>>;

    fn name() -> &'static str {
        "executable_checked_modules"
    }

    fn execute(&self, db: &QueryDb<CompilerContext>) -> Self::Value {
        provide_executable_checked_modules(db)
    }
}
