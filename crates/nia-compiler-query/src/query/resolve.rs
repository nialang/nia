// SPDX-License-Identifier: GPL-3.0-or-later
use super::*;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub(super) struct ValueResolutionQuery(pub(super) ModuleId);

impl QueryKey<DriverContext> for ValueResolutionQuery {
    type Value = ValueResolution;

    fn name() -> &'static str {
        "value_resolution"
    }

    fn execute(&self, db: &QueryDb<DriverContext>) -> Self::Value {
        let loaded = db.query(LoadedModuleQuery(self.0));
        let defs = db.query(ModuleDefsQuery(self.0));
        let all_defs = db.query(DefsByModuleQuery);
        let imports = db.query(ImportAliasMapQuery);
        let public = db.query(PublicSurfaceQuery);
        let empty_using = ModuleUsingScope::default();
        let using_scope = public.using_scopes.get(&self.0).unwrap_or(&empty_using);
        nia_value_resolve::resolve_module_values_with_context(
            &loaded.module,
            &defs,
            &imports,
            &all_defs,
            &public.surfaces,
            using_scope,
        )
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub(super) struct LocalResolutionQuery(pub(super) ModuleId);

impl QueryKey<DriverContext> for LocalResolutionQuery {
    type Value = LocalResolution;

    fn name() -> &'static str {
        "local_resolution"
    }

    fn execute(&self, db: &QueryDb<DriverContext>) -> Self::Value {
        let loaded = db.query(LoadedModuleQuery(self.0));
        let defs = db.query(ModuleDefsQuery(self.0));
        let values = db.query(ValueResolutionQuery(self.0));
        nia_local_resolve::resolve_module_locals(&loaded.module, &defs, &values)
    }
}
