// SPDX-License-Identifier: GPL-3.0-or-later
mod facade_facts;
mod graph;
mod provider_loading;
mod queries;
mod used_paths;

#[cfg(test)]
mod tests;

use nia_compiler_query::LoadedProgram;
use nia_imports::ModuleMap;
use nia_query::QueryDb;
use nia_source::{SourceDatabase, SourceFile, SourcePath};
use nia_symbol_table::SymbolTable;
use nia_target_config::TargetConfig;
use queries::{LoadedProgramQuery, SourceTextQuery};
use std::{path::Path, sync::Arc};

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
        let entry_path = SourcePath::new(request.entry_path);
        let module_map = effective_module_map(&entry_path, request.module_map);
        let sources = request.sources;
        let symbols = SymbolTable::new();
        let db = QueryDb::new(LoaderContext {
            entry_path,
            module_map,
            sources: sources.clone(),
            symbols,
            target: request.target,
            entry_runtime: request.entry_runtime,
            package_root_used_paths: request.package_root_used_paths,
        });
        Self { db, sources }
    }

    pub fn load_program(&self) -> LoadedProgram {
        self.db.query(LoadedProgramQuery)
    }

    pub fn sources(&self) -> &SourceDatabase {
        &self.sources
    }

    pub fn set_source(&self, path: impl Into<String>, text: impl Into<Arc<str>>) -> SourceFile {
        let path = SourcePath::new(path.into());
        let file = self.sources.set_source(path.clone(), text);
        self.db.invalidate(SourceTextQuery(path));
        file
    }

    pub fn invalidate_source(&self, path: impl Into<String>) -> nia_query::QueryInvalidation {
        self.db
            .invalidate(SourceTextQuery(SourcePath::new(path.into())))
    }

    pub fn query_trace(&self) -> nia_query::QueryTrace {
        self.db.query_trace()
    }
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
    let db = QueryDb::new(LoaderContext {
        entry_path,
        module_map,
        sources: SourceDatabase::new(),
        symbols: SymbolTable::new(),
        target: TargetConfig::host(),
        entry_runtime: EntryRuntime::None,
        package_root_used_paths: false,
    });
    let _ = db.query(LoadedProgramQuery);
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
    pub(crate) symbols: SymbolTable,
    pub(crate) target: TargetConfig,
    pub(crate) entry_runtime: EntryRuntime,
    pub(crate) package_root_used_paths: bool,
}
