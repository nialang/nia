// SPDX-License-Identifier: GPL-3.0-or-later
mod facade_facts;
mod graph;
mod provider_facts;
mod provider_loading;
mod queries;
mod used_paths;

#[cfg(test)]
mod tests;

use nia_compiler_query::{LoadedProgram, LoaderFactProvider, ProviderDemand};
use nia_imports::ModuleMap;
use nia_query::{QueryDb, QuerySession};
use nia_source::{SourceDatabase, SourceFile, SourcePath};
use nia_symbol_table::SymbolTable;
use nia_target_config::TargetConfig;
use provider_facts::{ProviderDemandsQuery, ProviderFactStore};
use queries::{LoadedProgramQuery, SourceTextQuery};
use std::{collections::HashSet, path::Path, sync::Arc};

fn loader_query_registry() -> nia_query::QueryRegistry {
    let mut registry = nia_query::QueryRegistry::new();
    macro_rules! register {
        ($($key:ty),+ $(,)?) => {
            $(registry.register::<LoaderContext, $key>();)+
        };
    }
    register!(
        graph::ModuleGraphQuery,
        graph::ModuleGraphRevisionQuery,
        queries::LoadDiagnosticsQuery,
        queries::ActiveModuleItemTreeFactQuery,
        queries::LoadedModuleQuery,
        queries::LoadedProgramQuery,
        queries::ModuleDeclarationsQuery,
        queries::ModuleFacadeFactsQuery,
        queries::ModuleItemTreeFactQuery,
        queries::ModuleOriginsFactQuery,
        queries::ModuleParseErrorsFactQuery,
        queries::ParsedModuleQuery,
        provider_facts::ProviderDemandsQuery,
        queries::ProviderSummaryQuery,
        queries::SourceStatusQuery,
        queries::SourceTextQuery,
        queries::SyntaxModuleQuery,
    );
    registry
}

pub fn load_program(entry_path: impl Into<String>) -> LoadedProgram {
    load_program_with_map(entry_path, ModuleMap::default())
}

pub fn load_program_with_map(
    entry_path: impl Into<String>,
    module_map: ModuleMap,
) -> LoadedProgram {
    load_program_with_map_and_entry_runtime(entry_path, module_map, EntryRuntime::None)
}

pub fn load_program_with_map_and_entry_runtime(
    entry_path: impl Into<String>,
    module_map: ModuleMap,
    entry_runtime: EntryRuntime,
) -> LoadedProgram {
    load_program_request(
        LoadRequest::new(entry_path)
            .with_module_map(module_map)
            .with_entry_runtime(entry_runtime),
    )
}

pub fn load_program_request(request: LoadRequest) -> LoadedProgram {
    LoaderDatabase::new(request).load_program()
}

#[derive(Clone)]
pub struct LoaderDatabase {
    db: QueryDb<LoaderContext>,
    sources: SourceDatabase,
}

impl LoaderDatabase {
    pub fn new(request: LoadRequest) -> Self {
        Self::new_in_session(request, QuerySession::new())
    }

    pub fn new_in_session(request: LoadRequest, session: QuerySession) -> Self {
        let entry_path = SourcePath::new(request.entry_path);
        let package_roots_with_used_paths = if request.package_root_used_paths {
            request.module_map.entries().map(|(name, _)| name).collect()
        } else {
            HashSet::new()
        };
        let module_map = effective_module_map(&entry_path, request.module_map);
        let sources = request.sources;
        let symbols = SymbolTable::new();
        let db = QueryDb::new_registered_in_session(
            LoaderContext {
                entry_path,
                module_map,
                sources: sources.clone(),
                node_store: nia_node_id::NodeStore::new(),
                symbols,
                target: request.target,
                entry_runtime: request.entry_runtime,
                package_roots_with_used_paths,
                provider_facts: ProviderFactStore::default(),
            },
            loader_query_registry(),
            session,
        );
        Self { db, sources }
    }

    pub fn query_session(&self) -> QuerySession {
        self.db.session()
    }

    pub fn load_program(&self) -> LoadedProgram {
        self.db.get(LoadedProgramQuery).as_ref().clone()
    }

    pub fn sources(&self) -> &SourceDatabase {
        &self.sources
    }

    pub fn set_source(&self, path: impl Into<String>, text: impl Into<Arc<str>>) -> SourceFile {
        let path = SourcePath::new(path.into());
        let file = self.sources.set_source(path.clone(), text);
        self.reset_provider_facts();
        self.db.invalidate(SourceTextQuery(file.id));
        file
    }

    pub fn invalidate_source(&self, path: impl Into<String>) -> nia_query::QueryInvalidation {
        let path = SourcePath::new(path.into());
        let source_id = self.sources.id_for_path(&path);
        self.reset_provider_facts();
        self.db.invalidate(SourceTextQuery(source_id))
    }

    pub fn query_trace(&self) -> nia_query::QueryTrace {
        self.db.query_trace()
    }

    pub fn update_provider_demands(
        &self,
        demands: impl IntoIterator<Item = ProviderDemand>,
    ) -> ProviderDemandUpdate {
        let demands = demands.into_iter().collect::<Vec<_>>();
        let all_known = self.db.context().provider_facts.contains_all(&demands);
        if all_known {
            return ProviderDemandUpdate::NoNewDemands;
        }
        let previous_graph = self.db.get(graph::ModuleGraphQuery);
        let added = self.db.context().provider_facts.insert_new(demands);
        if added.is_empty() {
            return ProviderDemandUpdate::NoNewDemands;
        }
        self.db.invalidate(ProviderDemandsQuery);
        let graph = self.db.get(graph::ModuleGraphQuery);
        let revision = self.db.get(ProviderDemandsQuery).revision();
        if graph == previous_graph {
            ProviderDemandUpdate::GraphUnchanged {
                revision,
                new_demands: added,
            }
        } else {
            ProviderDemandUpdate::GraphChanged {
                revision,
                new_demands: added,
            }
        }
    }

    fn reset_provider_facts(&self) {
        if self.db.context().provider_facts.clear() {
            self.db.invalidate(ProviderDemandsQuery);
        }
    }

    fn source_id_for_module(
        &self,
        module_id: nia_imports::ModuleId,
    ) -> Option<nia_source::SourceId> {
        let graph = self.db.get(graph::ModuleGraphQuery);
        let module = graph.get(module_id)?;
        Some(self.sources.id_for_path(&module.path))
    }
}

impl LoaderFactProvider for LoaderDatabase {
    fn loaded_program(&self) -> LoadedProgram {
        self.load_program()
    }

    fn provider_fact_revision(&self) -> nia_compiler_query::ProviderFactRevision {
        self.db.get(ProviderDemandsQuery).revision()
    }

    fn module_graph(&self) -> nia_imports::ModuleGraphSnapshot {
        self.db.get(graph::ModuleGraphQuery).as_ref().clone()
    }

    fn loaded_module_source_identities(&self) -> Vec<nia_source::SourceIdentity> {
        self.db
            .get(graph::ModuleGraphQuery)
            .modules()
            .map(|module| module.path.identity())
            .collect()
    }

    fn module_path(&self, module_id: nia_imports::ModuleId) -> Option<SourcePath> {
        let graph = self.db.get(graph::ModuleGraphQuery);
        graph.get(module_id).map(|module| module.path.clone())
    }

    fn module_source_version(
        &self,
        module_id: nia_imports::ModuleId,
    ) -> Option<nia_source::SourceVersion> {
        let graph = self.db.get(graph::ModuleGraphQuery);
        let module = graph.get(module_id)?;
        let source_id = self.sources.id_for_path(&module.path);
        match *self.db.get(queries::SourceStatusQuery(source_id)) {
            queries::SourceStatus::Present(version) => Some(version),
            queries::SourceStatus::Missing => None,
        }
    }

    fn module_provider_summary(
        &self,
        module_id: nia_imports::ModuleId,
    ) -> Option<nia_provider_summary::ProviderSummary> {
        let graph = self.db.get(graph::ModuleGraphQuery);
        let module = graph.get(module_id)?;
        Some(
            self.db
                .get(queries::provider_summary_query(&self.db, &module.path))
                .as_ref()
                .clone(),
        )
    }

    fn module_origins(
        &self,
        module_id: nia_imports::ModuleId,
    ) -> Option<nia_node_id::NodeOriginTable> {
        let source_id = self.source_id_for_module(module_id)?;
        Some(
            self.db
                .get(queries::ModuleOriginsFactQuery(source_id))
                .as_ref()
                .clone(),
        )
    }

    fn module_parse_errors(
        &self,
        module_id: nia_imports::ModuleId,
    ) -> Option<Vec<nia_parser::ParseError>> {
        let source_id = self.source_id_for_module(module_id)?;
        Some(
            self.db
                .get(queries::ModuleParseErrorsFactQuery(source_id))
                .as_ref()
                .clone(),
        )
    }

    fn module_item_tree(
        &self,
        module_id: nia_imports::ModuleId,
    ) -> Option<nia_item_tree::ModuleItemTree> {
        let source_id = self.source_id_for_module(module_id)?;
        Some(
            self.db
                .get(queries::ModuleItemTreeFactQuery(source_id))
                .as_ref()
                .clone(),
        )
    }

    fn active_module_item_tree(
        &self,
        module_id: nia_imports::ModuleId,
        kind: nia_compiler_query::ActiveModuleItemTreeFactKind,
    ) -> Option<nia_item_tree::ActiveModuleItemTree> {
        let source_id = self.source_id_for_module(module_id)?;
        Some(
            self.db
                .get(queries::ActiveModuleItemTreeFactQuery(source_id, kind))
                .as_ref()
                .clone(),
        )
    }

    fn load_diagnostics(&self) -> Vec<nia_compiler_query::ProgramDiagnostic> {
        self.db.get(queries::LoadDiagnosticsQuery).as_ref().clone()
    }

    fn symbols(&self) -> SymbolTable {
        self.db.context().symbols.clone()
    }

    fn target(&self) -> TargetConfig {
        self.db.context().target.clone()
    }

    fn runtime(&self) -> nia_compiler_query::RuntimeModel {
        queries::runtime_model(self.db.context().entry_runtime)
    }
}

#[derive(Debug, Clone, PartialEq)]
pub enum ProviderDemandUpdate {
    NoNewDemands,
    GraphUnchanged {
        revision: nia_compiler_query::ProviderFactRevision,
        new_demands: HashSet<ProviderDemand>,
    },
    GraphChanged {
        revision: nia_compiler_query::ProviderFactRevision,
        new_demands: HashSet<ProviderDemand>,
    },
}

#[derive(Debug, Clone)]
pub struct LoadRequest {
    pub entry_path: String,
    pub module_map: ModuleMap,
    pub sources: SourceDatabase,
    pub target: TargetConfig,
    pub entry_runtime: EntryRuntime,
    pub package_root_used_paths: bool,
}

impl LoadRequest {
    pub fn new(entry_path: impl Into<String>) -> Self {
        Self {
            entry_path: entry_path.into(),
            module_map: ModuleMap::default(),
            sources: SourceDatabase::new(),
            target: TargetConfig::host(),
            entry_runtime: EntryRuntime::None,
            package_root_used_paths: false,
        }
    }

    pub fn with_module_map(mut self, module_map: ModuleMap) -> Self {
        self.module_map = module_map;
        self
    }

    pub fn with_sources(mut self, sources: SourceDatabase) -> Self {
        self.sources = sources;
        self
    }

    pub fn with_target(mut self, target: TargetConfig) -> Self {
        self.target = target;
        self
    }

    pub fn with_entry_runtime(mut self, entry_runtime: EntryRuntime) -> Self {
        self.entry_runtime = entry_runtime;
        self
    }

    pub fn with_package_root_used_paths(mut self, package_root_used_paths: bool) -> Self {
        self.package_root_used_paths = package_root_used_paths;
        self
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default)]
pub enum EntryRuntime {
    #[default]
    None,
    Freestanding,
}

#[cfg(test)]
fn load_program_from_sources(
    entry_path: impl Into<String>,
    module_map: ModuleMap,
    sources: SourceDatabase,
) -> LoadedProgram {
    load_program_request(
        LoadRequest::new(entry_path)
            .with_module_map(module_map)
            .with_sources(sources),
    )
}

#[cfg(test)]
fn load_program_trace(
    entry_path: impl Into<String>,
    module_map: ModuleMap,
) -> nia_query::QueryTrace {
    let entry_path = SourcePath::new(entry_path.into());
    let module_map = effective_module_map(&entry_path, module_map);
    let db = QueryDb::new_registered(
        LoaderContext {
            entry_path,
            module_map,
            sources: SourceDatabase::new(),
            node_store: nia_node_id::NodeStore::new(),
            symbols: SymbolTable::new(),
            target: TargetConfig::host(),
            entry_runtime: EntryRuntime::None,
            package_roots_with_used_paths: HashSet::new(),
            provider_facts: ProviderFactStore::default(),
        },
        loader_query_registry(),
    );
    let _program = db.get(LoadedProgramQuery);
    db.query_trace()
}

fn effective_module_map(entry_path: &SourcePath, module_map: ModuleMap) -> ModuleMap {
    module_map
        .with_entry(entry_path.clone())
        .with_default_std(default_std_module_path())
}

fn default_std_module_path() -> SourcePath {
    let manifest_dir = Path::new(env!("CARGO_MANIFEST_DIR"));
    let workspace_root = manifest_dir
        .parent()
        .and_then(Path::parent)
        .unwrap_or(manifest_dir);
    SourcePath::new(
        workspace_root
            .join("lib/std.nia")
            .to_string_lossy()
            .into_owned(),
    )
}

pub(crate) struct LoaderContext {
    pub(crate) entry_path: SourcePath,
    pub(crate) module_map: ModuleMap,
    pub(crate) sources: SourceDatabase,
    pub(crate) node_store: nia_node_id::NodeStore,
    pub(crate) symbols: SymbolTable,
    pub(crate) target: TargetConfig,
    pub(crate) entry_runtime: EntryRuntime,
    pub(crate) package_roots_with_used_paths: HashSet<nia_symbol::SymbolId>,
    pub(crate) provider_facts: ProviderFactStore,
}
