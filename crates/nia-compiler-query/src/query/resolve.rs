// SPDX-License-Identifier: GPL-3.0-or-later
use super::*;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub(super) struct ValueResolutionQuery(pub(super) ModuleId);

impl QueryKey<CompilerContext> for ValueResolutionQuery {
    type Value = ValueResolution;

    fn name() -> &'static str {
        "value_resolution"
    }

    fn execute(&self, db: &QueryDb<CompilerContext>) -> Self::Value {
        (db.context().providers.value_resolution)(db, self.0)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub(super) struct LocalResolutionQuery(pub(super) ModuleId);

impl QueryKey<CompilerContext> for LocalResolutionQuery {
    type Value = LocalResolution;

    fn name() -> &'static str {
        "local_resolution"
    }

    fn execute(&self, db: &QueryDb<CompilerContext>) -> Self::Value {
        (db.context().providers.local_resolution)(db, self.0)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub(super) struct SemanticUseTableQuery(pub(super) ModuleId);

impl QueryKey<CompilerContext> for SemanticUseTableQuery {
    type Value = nia_sema_ir::SemanticUseTable;

    fn name() -> &'static str {
        "semantic_use_table"
    }

    fn execute(&self, db: &QueryDb<CompilerContext>) -> Self::Value {
        (db.context().providers.semantic_use_table)(db, self.0)
    }
}
