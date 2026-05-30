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
        CheckedProgram {
            graph: db.query(ModuleGraphQuery),
            imports: db.query(ImportAliasMapQuery),
            modules: db.query(CheckedModulesQuery),
            monomorphization: db.query(MonomorphizationQuery),
            backend_lowering: db.query(BackendLoweringQuery),
            diagnostics: db.query(ProgramDiagnosticsQuery),
        }
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
        db.context().loaded.graph.clone()
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
        db.context().loaded.imports.clone()
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
        db.context()
            .loaded
            .modules
            .iter()
            .filter(|module| module.parse_errors.is_empty())
            .map(|module| module.id)
            .collect()
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
        db.context()
            .loaded_module(self.0)
            .unwrap_or_else(|| panic!("missing loaded module {:?}", self.0))
            .clone()
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub(super) struct AllModulesQuery;

impl QueryKey<DriverContext> for AllModulesQuery {
    type Value = Vec<nia_ast::Module>;

    fn name() -> &'static str {
        "all_modules"
    }

    fn execute(&self, db: &QueryDb<DriverContext>) -> Self::Value {
        db.query(ParseOkModuleIdsQuery)
            .into_iter()
            .map(|module_id| db.query(LoadedModuleQuery(module_id)).module)
            .collect()
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
        let loaded = db.query(LoadedModuleQuery(self.0));
        nia_defs::collect_module_defs(loaded.id, &loaded.module)
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
        db.query_many(
            db.query(ParseOkModuleIdsQuery)
                .into_iter()
                .map(ModuleDefsQuery),
        )
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
        let defs = db.query(DefsByModuleQuery);
        let imports = db.query(ImportAliasMapQuery);
        let (surfaces, using_scopes, diagnostics) = compute_public_surfaces(&defs, &imports);
        PublicSurfaceQueryValue {
            surfaces,
            using_scopes,
            diagnostics,
        }
    }
}
