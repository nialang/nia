// SPDX-License-Identifier: GPL-3.0-or-later
use super::*;
use std::collections::HashMap;
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub(super) struct CheckedProgramQuery;

impl QueryKey<DriverContext> for CheckedProgramQuery {
    type Value = CheckedProgram;

    fn name() -> &'static str {
        "checked_program"
    }

    fn execute(&self, db: &QueryDb<DriverContext>) -> Self::Value {
        (db.context().providers.checked_program)(db)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub(super) struct ModuleGraphQuery;

impl QueryKey<DriverContext> for ModuleGraphQuery {
    type Value = ModuleGraph;

    fn name() -> &'static str {
        "module_graph"
    }

    fn execute(&self, db: &QueryDb<DriverContext>) -> Self::Value {
        (db.context().providers.module_graph)(db)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub(super) struct ImportAliasMapQuery;

impl QueryKey<DriverContext> for ImportAliasMapQuery {
    type Value = ImportAliasMap;

    fn name() -> &'static str {
        "import_alias_map"
    }

    fn execute(&self, db: &QueryDb<DriverContext>) -> Self::Value {
        (db.context().providers.import_alias_map)(db)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub(super) struct ParseOkModuleIdsQuery;

impl QueryKey<DriverContext> for ParseOkModuleIdsQuery {
    type Value = Vec<ModuleId>;

    fn name() -> &'static str {
        "parse_ok_module_ids"
    }

    fn execute(&self, db: &QueryDb<DriverContext>) -> Self::Value {
        (db.context().providers.parse_ok_module_ids)(db)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub(super) struct LoadedModuleQuery(pub(super) ModuleId);

impl QueryKey<DriverContext> for LoadedModuleQuery {
    type Value = LoadedModule;

    fn name() -> &'static str {
        "loaded_module"
    }

    fn execute(&self, db: &QueryDb<DriverContext>) -> Self::Value {
        (db.context().providers.loaded_module)(db, self.0)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub(super) struct ModuleDefsQuery(pub(super) ModuleId);

impl QueryKey<DriverContext> for ModuleDefsQuery {
    type Value = DefCollection;

    fn name() -> &'static str {
        "module_defs"
    }

    fn execute(&self, db: &QueryDb<DriverContext>) -> Self::Value {
        (db.context().providers.module_defs)(db, self.0)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub(super) struct DefsByModuleQuery;

impl QueryKey<DriverContext> for DefsByModuleQuery {
    type Value = Vec<DefCollection>;

    fn name() -> &'static str {
        "defs_by_module"
    }

    fn execute(&self, db: &QueryDb<DriverContext>) -> Self::Value {
        (db.context().providers.defs_by_module)(db)
    }
}

#[derive(Debug, Clone, PartialEq)]
pub(super) struct PublicSurfaceQueryValue {
    pub(super) surfaces: PublicSurfaces,
    pub(super) using_scopes: HashMap<ModuleId, ModuleUsingScope>,
    pub(super) diagnostics: Vec<(ModuleId, Diagnostic)>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub(super) struct PublicSurfaceQuery;

impl QueryKey<DriverContext> for PublicSurfaceQuery {
    type Value = PublicSurfaceQueryValue;

    fn name() -> &'static str {
        "public_surface"
    }

    fn execute(&self, db: &QueryDb<DriverContext>) -> Self::Value {
        (db.context().providers.public_surface)(db)
    }
}
