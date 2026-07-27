// SPDX-License-Identifier: GPL-3.0-or-later
use super::*;

struct TestLoaderContext {
    program: RwLock<LoadedProgram>,
    provider_facts: RwLock<crate::ProviderFactSnapshot>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
struct TestLoadedProgramQuery;

impl QueryKey<TestLoaderContext> for TestLoadedProgramQuery {
    type Value = LoadedProgram;

    const FINGERPRINT: QueryFingerprintPolicy = QueryFingerprintPolicy::SemanticValue;

    fn name() -> &'static str {
        "test_loaded_program"
    }

    fn execute_result(&self, db: &QueryDb<TestLoaderContext>) -> QueryResult<Self::Value> {
        Ok(db
            .context()
            .program
            .read()
            .expect("test loader program lock poisoned")
            .clone())
    }

    fn values_equal(&self, old: &Self::Value, new: &Self::Value) -> bool {
        old == new
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
struct TestProviderFactsQuery;

impl QueryKey<TestLoaderContext> for TestProviderFactsQuery {
    type Value = crate::ProviderFactSnapshot;

    const FINGERPRINT: QueryFingerprintPolicy = QueryFingerprintPolicy::SemanticValue;

    fn name() -> &'static str {
        "test_provider_facts"
    }

    fn execute_result(&self, db: &QueryDb<TestLoaderContext>) -> QueryResult<Self::Value> {
        Ok(db
            .context()
            .provider_facts
            .read()
            .expect("test provider facts lock poisoned")
            .clone())
    }

    fn values_equal(&self, old: &Self::Value, new: &Self::Value) -> bool {
        old == new
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
enum TestLoaderFactKey {
    Graph,
    LoadedModuleSourceIdentities,
    ModulePath(ModuleId),
    ModuleSourceVersion(ModuleId),
    ModuleProviderSummary(ModuleId),
    ModuleOrigins(ModuleId),
    ModuleParseErrors(ModuleId),
    ModuleItemTree(ModuleId),
    ActiveModuleItemTree(ModuleId, ActiveModuleItemTreeFactKind),
    LoadDiagnostics,
    Target,
    Runtime,
}

#[derive(Debug, Clone, PartialEq)]
enum TestLoaderFactValue {
    Graph(ModuleGraphSnapshot),
    LoadedModuleSourceIdentities(Vec<SourceIdentity>),
    ModulePath(Option<SourcePath>),
    ModuleSourceVersion(Option<SourceVersion>),
    ModuleProviderSummary(Option<nia_provider_summary::ProviderSummary>),
    ModuleOrigins(Option<NodeOriginTable>),
    ModuleParseErrors(Option<Vec<ParseError>>),
    ModuleItemTree(Option<ModuleItemTree>),
    ActiveModuleItemTree(Option<ActiveModuleItemTree>),
    LoadDiagnostics(Vec<ProgramDiagnostic>),
    Target(TargetConfig),
    Runtime(RuntimeModel),
}

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
struct TestLoaderFactQuery(TestLoaderFactKey);

impl QueryKey<TestLoaderContext> for TestLoaderFactQuery {
    type Value = TestLoaderFactValue;

    const FINGERPRINT: QueryFingerprintPolicy = QueryFingerprintPolicy::SemanticValue;

    fn name() -> &'static str {
        "test_loader_fact"
    }

    fn execute_result(&self, db: &QueryDb<TestLoaderContext>) -> QueryResult<Self::Value> {
        let program = db.get(TestLoadedProgramQuery)?;
        let module = |module_id| program.modules.iter().find(|module| module.id == module_id);
        Ok(match self.0 {
            TestLoaderFactKey::Graph => Self::Value::Graph(program.graph.clone()),
            TestLoaderFactKey::LoadedModuleSourceIdentities => {
                Self::Value::LoadedModuleSourceIdentities(
                    program
                        .modules
                        .iter()
                        .map(|module| module.source_identity.clone())
                        .collect(),
                )
            }
            TestLoaderFactKey::ModulePath(module_id) => {
                Self::Value::ModulePath(module(module_id).map(|module| module.path.clone()))
            }
            TestLoaderFactKey::ModuleSourceVersion(module_id) => Self::Value::ModuleSourceVersion(
                module(module_id).map(|module| module.source_version),
            ),
            TestLoaderFactKey::ModuleProviderSummary(module_id) => {
                Self::Value::ModuleProviderSummary(
                    module(module_id).map(|module| module.provider_summary.clone()),
                )
            }
            TestLoaderFactKey::ModuleOrigins(module_id) => {
                Self::Value::ModuleOrigins(module(module_id).map(|module| module.origins.clone()))
            }
            TestLoaderFactKey::ModuleParseErrors(module_id) => Self::Value::ModuleParseErrors(
                module(module_id).map(|module| module.parse_errors.clone()),
            ),
            TestLoaderFactKey::ModuleItemTree(module_id) => Self::Value::ModuleItemTree(
                module(module_id).map(|module| module.item_tree.clone()),
            ),
            TestLoaderFactKey::ActiveModuleItemTree(module_id, kind) => {
                let tree = module(module_id).map(|module| match kind {
                    ActiveModuleItemTreeFactKind::Signature(set) => {
                        module.active_item_tree.signature_items(set)
                    }
                    ActiveModuleItemTreeFactKind::ConstSignature => {
                        module.active_item_tree.const_signature_items()
                    }
                    ActiveModuleItemTreeFactKind::Full => module.active_item_tree.clone(),
                });
                Self::Value::ActiveModuleItemTree(tree)
            }
            TestLoaderFactKey::LoadDiagnostics => {
                Self::Value::LoadDiagnostics(program.diagnostics.clone())
            }
            TestLoaderFactKey::Target => Self::Value::Target(program.target.clone()),
            TestLoaderFactKey::Runtime => Self::Value::Runtime(program.runtime),
        })
    }

    fn values_equal(&self, old: &Self::Value, new: &Self::Value) -> bool {
        old == new
    }
}

#[derive(Clone)]
pub(super) struct TestLoaderFacts {
    db: QueryDb<TestLoaderContext>,
}

impl TestLoaderFacts {
    pub(super) fn new(program: LoadedProgram, provider_facts: crate::ProviderFactSnapshot) -> Self {
        let mut registry = nia_query::QueryRegistry::new();
        registry.register::<TestLoaderContext, TestLoadedProgramQuery>();
        registry.register::<TestLoaderContext, TestProviderFactsQuery>();
        registry.register::<TestLoaderContext, TestLoaderFactQuery>();
        Self {
            db: QueryDb::new_registered(
                TestLoaderContext {
                    program: RwLock::new(program),
                    provider_facts: RwLock::new(provider_facts),
                },
                registry,
            ),
        }
    }

    fn program(&self) -> Arc<LoadedProgram> {
        self.db.expect_get(TestLoadedProgramQuery)
    }

    fn fact(&self, key: TestLoaderFactKey) -> Arc<TestLoaderFactValue> {
        self.db.expect_get(TestLoaderFactQuery(key))
    }

    pub(super) fn replace_program(&self, program: LoadedProgram) -> nia_query::QueryInvalidation {
        let mut current = self
            .db
            .context()
            .program
            .write()
            .expect("test loader program lock poisoned");
        if *current == program {
            return nia_query::QueryInvalidation::default();
        }
        *current = program;
        drop(current);
        self.db.invalidate(TestLoadedProgramQuery)
    }

    pub(super) fn replace_provider_facts(
        &self,
        provider_facts: crate::ProviderFactSnapshot,
    ) -> nia_query::QueryInvalidation {
        let mut current = self
            .db
            .context()
            .provider_facts
            .write()
            .expect("test provider facts lock poisoned");
        if *current == provider_facts {
            return nia_query::QueryInvalidation::default();
        }
        *current = provider_facts;
        drop(current);
        self.db.invalidate(TestProviderFactsQuery)
    }
}

impl crate::LoaderFactProvider for TestLoaderFacts {
    fn query_session(&self) -> Option<nia_query::QuerySession> {
        Some(self.db.session())
    }

    fn provider_facts(&self) -> QueryResult<crate::ProviderFactSnapshot> {
        Ok(self.db.get(TestProviderFactsQuery)?.as_ref().clone())
    }

    fn update_provider_demands(
        &self,
        _demands: Vec<crate::ProviderDemand>,
    ) -> QueryResult<crate::ProviderGraphUpdate> {
        Ok(crate::ProviderGraphUpdate::Stable)
    }

    fn node_store(&self) -> nia_node_id::NodeStore {
        self.program()
            .modules
            .first()
            .map(|module| module.origins.node_store().clone())
            .unwrap_or_default()
    }

    fn module_graph(&self) -> QueryResult<ModuleGraphSnapshot> {
        let fact = self.fact(TestLoaderFactKey::Graph);
        let TestLoaderFactValue::Graph(graph) = fact.as_ref() else {
            unreachable!()
        };
        Ok(graph.clone())
    }

    fn loaded_module_source_identities(&self) -> QueryResult<Vec<SourceIdentity>> {
        let fact = self.fact(TestLoaderFactKey::LoadedModuleSourceIdentities);
        let TestLoaderFactValue::LoadedModuleSourceIdentities(identities) = fact.as_ref() else {
            unreachable!()
        };
        Ok(identities.clone())
    }

    fn module_path(&self, module_id: ModuleId) -> QueryResult<Option<SourcePath>> {
        let fact = self.fact(TestLoaderFactKey::ModulePath(module_id));
        let TestLoaderFactValue::ModulePath(path) = fact.as_ref() else {
            unreachable!()
        };
        Ok(path.clone())
    }

    fn module_source_version(&self, module_id: ModuleId) -> QueryResult<Option<SourceVersion>> {
        let fact = self.fact(TestLoaderFactKey::ModuleSourceVersion(module_id));
        let TestLoaderFactValue::ModuleSourceVersion(version) = fact.as_ref() else {
            unreachable!()
        };
        Ok(*version)
    }

    fn module_source_fingerprint(
        &self,
        _module_id: ModuleId,
    ) -> QueryResult<Option<(crate::SourceContentFingerprint, usize)>> {
        Ok(None)
    }

    fn module_provider_summary(
        &self,
        module_id: ModuleId,
    ) -> QueryResult<Option<nia_provider_summary::ProviderSummary>> {
        let fact = self.fact(TestLoaderFactKey::ModuleProviderSummary(module_id));
        let TestLoaderFactValue::ModuleProviderSummary(summary) = fact.as_ref() else {
            unreachable!()
        };
        Ok(summary.clone())
    }

    fn module_origins(&self, module_id: ModuleId) -> QueryResult<Option<NodeOriginTable>> {
        let fact = self.fact(TestLoaderFactKey::ModuleOrigins(module_id));
        let TestLoaderFactValue::ModuleOrigins(origins) = fact.as_ref() else {
            unreachable!()
        };
        Ok(origins.clone())
    }

    fn module_parse_errors(&self, module_id: ModuleId) -> QueryResult<Option<Vec<ParseError>>> {
        let fact = self.fact(TestLoaderFactKey::ModuleParseErrors(module_id));
        let TestLoaderFactValue::ModuleParseErrors(errors) = fact.as_ref() else {
            unreachable!()
        };
        Ok(errors.clone())
    }

    fn module_item_tree(&self, module_id: ModuleId) -> QueryResult<Option<ModuleItemTree>> {
        let fact = self.fact(TestLoaderFactKey::ModuleItemTree(module_id));
        let TestLoaderFactValue::ModuleItemTree(tree) = fact.as_ref() else {
            unreachable!()
        };
        Ok(tree.clone())
    }

    fn active_module_item_tree(
        &self,
        module_id: ModuleId,
        kind: ActiveModuleItemTreeFactKind,
    ) -> QueryResult<Option<ActiveModuleItemTree>> {
        let fact = self.fact(TestLoaderFactKey::ActiveModuleItemTree(module_id, kind));
        let TestLoaderFactValue::ActiveModuleItemTree(tree) = fact.as_ref() else {
            unreachable!()
        };
        Ok(tree.clone())
    }

    fn load_diagnostics(&self) -> QueryResult<Vec<ProgramDiagnostic>> {
        let fact = self.fact(TestLoaderFactKey::LoadDiagnostics);
        let TestLoaderFactValue::LoadDiagnostics(diagnostics) = fact.as_ref() else {
            unreachable!()
        };
        Ok(diagnostics.clone())
    }

    fn symbols(&self) -> nia_symbol_table::SymbolTable {
        self.program().symbols.clone()
    }

    fn target(&self) -> TargetConfig {
        let fact = self.fact(TestLoaderFactKey::Target);
        let TestLoaderFactValue::Target(target) = fact.as_ref() else {
            unreachable!()
        };
        target.clone()
    }

    fn runtime(&self) -> RuntimeModel {
        let fact = self.fact(TestLoaderFactKey::Runtime);
        let TestLoaderFactValue::Runtime(runtime) = fact.as_ref() else {
            unreachable!()
        };
        *runtime
    }
}
