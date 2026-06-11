// SPDX-License-Identifier: GPL-3.0-or-later
use super::*;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub(super) struct CheckedModuleQuery(pub(super) ModuleId);

impl QueryKey<DriverContext> for CheckedModuleQuery {
    type Value = CheckedModule;

    fn name() -> &'static str {
        "checked_module"
    }

    fn execute(&self, db: &QueryDb<DriverContext>) -> Self::Value {
        (db.context().providers.checked_module)(db, self.0)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub(super) struct CheckedModulesQuery;

impl QueryKey<DriverContext> for CheckedModulesQuery {
    type Value = Vec<CheckedModule>;

    fn name() -> &'static str {
        "checked_modules"
    }

    fn execute(&self, db: &QueryDb<DriverContext>) -> Self::Value {
        (db.context().providers.checked_modules)(db)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub(super) struct ExecutableCheckedModulesQuery;

impl QueryKey<DriverContext> for ExecutableCheckedModulesQuery {
    type Value = Vec<CheckedModule>;

    fn name() -> &'static str {
        "executable_checked_modules"
    }

    fn execute(&self, db: &QueryDb<DriverContext>) -> Self::Value {
        provide_executable_checked_modules(db)
    }
}
