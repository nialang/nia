// SPDX-License-Identifier: GPL-3.0-or-later
use super::*;

pub(super) fn query_db(loaded: LoadedProgram) -> QueryDb<CompilerContext> {
    let loader_facts: Arc<dyn crate::LoaderFactProvider> = Arc::new(loaded.clone());
    let node_store = loader_facts.node_store();
    let inputs = Arc::new(RwLock::new(CompilerInputs::new(CompileRequest::new(
        loaded,
    ))));
    QueryDb::new_registered(
        CompilerContext {
            inputs,
            loader_facts,
            providers: CompilerQueryProviders::default(),
            executable_fact_session: Arc::new(std::sync::Mutex::new(
                ExecutableFactSession::default(),
            )),
            executable_fact_scheduler: std::sync::Mutex::new(()),
            type_store: Arc::new(nia_ty::TypeStore::new()),
            diagnostic_store: nia_diagnostic::DiagnosticStore::new(),
            node_store,
            signature_cache: None,
            verify_frontend_cache: false,
            provider_demand_rounds: std::sync::atomic::AtomicU64::new(0),
        },
        compiler_query_registry(),
    )
}

struct FingerprintedLoadedProgram {
    program: LoadedProgram,
    sources: HashMap<ModuleId, (crate::SourceContentFingerprint, usize)>,
}

impl crate::LoaderFactProvider for FingerprintedLoadedProgram {
    fn query_session(&self) -> Option<nia_query::QuerySession> {
        None
    }

    fn provider_facts(&self) -> QueryResult<crate::ProviderFactSnapshot> {
        self.program.provider_facts()
    }

    fn update_provider_demands(
        &self,
        demands: Vec<crate::ProviderDemand>,
    ) -> QueryResult<crate::ProviderGraphUpdate> {
        self.program.update_provider_demands(demands)
    }

    fn node_store(&self) -> nia_node_id::NodeStore {
        self.program.node_store()
    }

    fn module_graph(&self) -> QueryResult<nia_imports::ModuleGraphSnapshot> {
        self.program.module_graph()
    }

    fn loaded_module_source_identities(&self) -> QueryResult<Vec<SourceIdentity>> {
        self.program.loaded_module_source_identities()
    }

    fn module_path(&self, module_id: ModuleId) -> QueryResult<Option<SourcePath>> {
        self.program.module_path(module_id)
    }

    fn module_source_version(&self, module_id: ModuleId) -> QueryResult<Option<SourceVersion>> {
        self.program.module_source_version(module_id)
    }

    fn module_source_fingerprint(
        &self,
        module_id: ModuleId,
    ) -> QueryResult<Option<(crate::SourceContentFingerprint, usize)>> {
        Ok(self.sources.get(&module_id).copied())
    }

    fn module_provider_summary(
        &self,
        module_id: ModuleId,
    ) -> QueryResult<Option<nia_provider_summary::ProviderSummary>> {
        self.program.module_provider_summary(module_id)
    }

    fn module_origins(&self, module_id: ModuleId) -> QueryResult<Option<NodeOriginTable>> {
        self.program.module_origins(module_id)
    }

    fn module_parse_errors(&self, module_id: ModuleId) -> QueryResult<Option<Vec<ParseError>>> {
        self.program.module_parse_errors(module_id)
    }

    fn module_item_tree(&self, module_id: ModuleId) -> QueryResult<Option<ModuleItemTree>> {
        self.program.module_item_tree(module_id)
    }

    fn active_module_item_tree(
        &self,
        module_id: ModuleId,
        kind: ActiveModuleItemTreeFactKind,
    ) -> QueryResult<Option<ActiveModuleItemTree>> {
        self.program.active_module_item_tree(module_id, kind)
    }

    fn load_diagnostics(&self) -> QueryResult<Vec<ProgramDiagnostic>> {
        self.program.load_diagnostics()
    }

    fn symbols(&self) -> nia_symbol_table::SymbolTable {
        self.program.symbols()
    }

    fn target(&self) -> TargetConfig {
        self.program.target()
    }

    fn runtime(&self) -> RuntimeModel {
        self.program.runtime()
    }
}

pub(super) fn query_db_with_frontend_cache(
    loaded: LoadedProgram,
    sources: HashMap<ModuleId, (crate::SourceContentFingerprint, usize)>,
    root: PathBuf,
    verify: bool,
) -> QueryDb<CompilerContext> {
    let loader_facts: Arc<dyn crate::LoaderFactProvider> = Arc::new(FingerprintedLoadedProgram {
        program: loaded.clone(),
        sources,
    });
    let node_store = loader_facts.node_store();
    let inputs = Arc::new(RwLock::new(CompilerInputs::new(
        CompileRequest::new(loaded)
            .with_frontend_cache_dir(Some(root.clone()))
            .with_frontend_cache_verification(verify),
    )));
    QueryDb::new_registered(
        CompilerContext {
            inputs,
            loader_facts,
            providers: CompilerQueryProviders::default(),
            executable_fact_session: Arc::new(std::sync::Mutex::new(
                ExecutableFactSession::default(),
            )),
            executable_fact_scheduler: std::sync::Mutex::new(()),
            type_store: Arc::new(nia_ty::TypeStore::new()),
            diagnostic_store: nia_diagnostic::DiagnosticStore::new(),
            node_store,
            signature_cache: Some(Arc::new(
                crate::signature_cache::PersistentSignatureCache::new(root),
            )),
            verify_frontend_cache: verify,
            provider_demand_rounds: std::sync::atomic::AtomicU64::new(0),
        },
        compiler_query_registry(),
    )
}

pub(super) fn module_id_for_source_identity(
    db: &QueryDb<CompilerContext>,
    identity: &SourceIdentity,
) -> Option<ModuleId> {
    db.context()
        .loader_facts
        .module_graph()
        .expect("test module graph")
        .modules()
        .find_map(|module| {
            db.context()
                .loader_facts
                .module_path(module.id)
                .expect("test module path query")
                .is_some_and(|path| path.identity() == *identity)
                .then_some(module.id)
        })
}
