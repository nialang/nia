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
        let loaded = db.query(LoadedModuleQuery(self.0));
        CheckedModule {
            id: loaded.id,
            path: loaded.path,
            defs: db.query(ModuleDefsQuery(self.0)),
            type_resolution: db.query(TypeResolutionQuery(self.0)),
            type_lowering: db.query(TypeLoweringQuery(self.0)),
            value_resolution: db.query(ValueResolutionQuery(self.0)),
            local_resolution: db.query(LocalResolutionQuery(self.0)),
            item_signatures: db.query(ItemSignaturesQuery(self.0)),
            type_normalization: db.query(TypeNormalizationQuery(self.0)),
            comptime: db.query(ComptimeQuery(self.0)),
            static_check: db.query(StaticCheckQuery(self.0)),
            layouts: db.query(LayoutsQuery(self.0)),
            abi_check: db.query(AbiCheckQuery(self.0)),
            flow_check: db.query(FlowCheckQuery(self.0)),
            body_check: db.query(BodyCheckQuery(self.0)),
        }
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
        db.query_many(
            db.query(ParseOkModuleIdsQuery)
                .into_iter()
                .map(CheckedModuleQuery),
        )
    }
}
