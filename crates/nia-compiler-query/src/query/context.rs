// SPDX-License-Identifier: GPL-3.0-or-later
use super::frontend_cache_publication::PendingFrontendCachePublications;
use super::{
    ActiveModuleItemTreeInputQuery, CompileRequest, CompilerQueryProviders,
    DeclarationActiveModuleItemTreeInputQuery, DeclarationModuleItemTreeInputQuery,
    ExecutableFactSession, ExtensionProviderSummaryQuery, FullActiveModuleItemTreeInputQuery,
    FullModuleItemTreeInputQuery, LoadedModulesQuery, ModuleItemTreeInputQuery, ModuleOriginsQuery,
    ModuleParseErrorsQuery, ModulePathQuery, ModuleSourceVersionQuery, ModuleUnusedImportsQuery,
    SignatureConstItemTreeQuery, SignatureItemTreeQuery,
};
use crate::{
    ActiveModuleItemTreeFactKind, CodegenScope, FrontendCheckCertificateCacheKey,
    FrontendCheckInputFingerprint, FrontendCheckScope, RuntimeSpec, TimingMode,
};
use nia_ids::ModuleId;
use nia_imports::{ModuleGraphSnapshot, StableModuleKey};
use nia_item_tree::{ActiveModuleItemTree, ModuleItemTree};
use nia_loader_contract::UnusedUsingImport;
use nia_opt::OptimizationPolicy;
use nia_parser::ParseError;
use nia_query::{QueryDb, QueryError, QueryResult};
use nia_source::{SourceIdentity, SourceVersion};
use nia_target_config::TargetConfig;
use parking_lot::{Mutex, RwLock};
use std::{
    collections::{BTreeMap, HashMap},
    sync::Arc,
};

pub(super) struct CompilerContext {
    pub(super) inputs: Arc<RwLock<CompilerInputs>>,
    pub(super) observed_graph: Mutex<ModuleGraphSnapshot>,
    pub(super) loader_facts: Arc<dyn crate::LoaderFactProvider>,
    pub(super) providers: CompilerQueryProviders,
    pub(super) executable_fact_session: Arc<Mutex<ExecutableFactSession>>,
    pub(super) executable_fact_scheduler: Mutex<()>,
    pub(super) type_store: Arc<nia_ty::TypeStore>,
    pub(super) diagnostic_store: nia_diagnostic::DiagnosticStore,
    pub(super) node_store: nia_node_id::NodeStore,
    pub(super) signature_cache: Option<Arc<crate::signature_cache::PersistentSignatureCache>>,
    pub(super) verify_frontend_cache: bool,
    pub(super) provider_settlement_scheduler: Mutex<()>,
    pub(super) frontend_cache_publications: Mutex<Option<PendingFrontendCachePublications>>,
    pub(super) provider_demand_rounds: std::sync::atomic::AtomicU64,
}

impl CompilerContext {
    pub(super) fn loader_facts(&self) -> &dyn crate::LoaderFactProvider {
        self.loader_facts.as_ref()
    }

    pub(super) fn type_store(&self) -> &nia_ty::TypeStore {
        &self.type_store
    }

    pub(super) fn node_store(&self) -> &nia_node_id::NodeStore {
        &self.node_store
    }

    pub(super) fn module_path(
        &self,
        db: &QueryDb<CompilerContext>,
        module_id: ModuleId,
    ) -> QueryResult<nia_source::SourcePath> {
        self.loader_facts().module_path(module_id)?.ok_or_else(|| {
            db.invalid_input(
                &ModulePathQuery(module_id),
                format!("missing loaded module {module_id:?}"),
            )
        })
    }

    pub(super) fn module_source_version(
        &self,
        db: &QueryDb<CompilerContext>,
        module_id: ModuleId,
    ) -> QueryResult<SourceVersion> {
        self.loader_facts()
            .module_source_version(module_id)?
            .ok_or_else(|| {
                db.invalid_input(
                    &ModuleSourceVersionQuery(module_id),
                    format!("missing loaded module {module_id:?}"),
                )
            })
    }

    pub(super) fn module_origins(
        &self,
        db: &QueryDb<CompilerContext>,
        module_id: ModuleId,
    ) -> QueryResult<nia_node_id::NodeOriginTable> {
        self.loader_facts()
            .module_origins(module_id)?
            .ok_or_else(|| {
                db.invalid_input(
                    &ModuleOriginsQuery(module_id),
                    format!("missing loaded module {module_id:?}"),
                )
            })
    }

    pub(super) fn module_parse_errors(
        &self,
        db: &QueryDb<CompilerContext>,
        module_id: ModuleId,
    ) -> QueryResult<Vec<ParseError>> {
        self.loader_facts()
            .module_parse_errors(module_id)?
            .ok_or_else(|| {
                db.invalid_input(
                    &ModuleParseErrorsQuery(module_id),
                    format!("missing loaded module {module_id:?}"),
                )
            })
    }

    pub(super) fn module_unused_imports(
        &self,
        db: &QueryDb<CompilerContext>,
        module_id: ModuleId,
    ) -> QueryResult<Vec<UnusedUsingImport>> {
        self.loader_facts()
            .module_unused_imports(module_id)?
            .ok_or_else(|| {
                db.invalid_input(
                    &ModuleUnusedImportsQuery(module_id),
                    format!("missing loaded module {module_id:?}"),
                )
            })
    }

    pub(super) fn module_item_tree(
        &self,
        db: &QueryDb<CompilerContext>,
        module_id: ModuleId,
    ) -> QueryResult<ModuleItemTree> {
        self.loader_facts()
            .module_item_tree(module_id)?
            .ok_or_else(|| {
                db.invalid_input(
                    &ModuleItemTreeInputQuery(module_id),
                    format!("missing loaded module {module_id:?}"),
                )
            })
    }

    pub(super) fn declaration_module_item_tree(
        &self,
        db: &QueryDb<CompilerContext>,
        module_id: ModuleId,
    ) -> QueryResult<ModuleItemTree> {
        self.loader_facts()
            .module_item_tree(module_id)?
            .ok_or_else(|| {
                db.invalid_input(
                    &DeclarationModuleItemTreeInputQuery(module_id),
                    format!("missing loaded module {module_id:?}"),
                )
            })
    }

    pub(super) fn full_module_item_tree(
        &self,
        db: &QueryDb<CompilerContext>,
        module_id: ModuleId,
    ) -> QueryResult<ModuleItemTree> {
        self.loader_facts()
            .module_item_tree(module_id)?
            .ok_or_else(|| {
                db.invalid_input(
                    &FullModuleItemTreeInputQuery(module_id),
                    format!("missing loaded module {module_id:?}"),
                )
            })
    }

    pub(super) fn active_module_item_tree(
        &self,
        db: &QueryDb<CompilerContext>,
        module_id: ModuleId,
    ) -> QueryResult<ActiveModuleItemTree> {
        self.active_item_tree(db, module_id, &ActiveModuleItemTreeInputQuery(module_id))
    }

    pub(super) fn declaration_active_module_item_tree(
        &self,
        db: &QueryDb<CompilerContext>,
        module_id: ModuleId,
    ) -> QueryResult<ActiveModuleItemTree> {
        self.active_item_tree(
            db,
            module_id,
            &DeclarationActiveModuleItemTreeInputQuery(module_id),
        )
    }

    pub(super) fn full_active_module_item_tree(
        &self,
        db: &QueryDb<CompilerContext>,
        module_id: ModuleId,
    ) -> QueryResult<ActiveModuleItemTree> {
        self.active_item_tree(
            db,
            module_id,
            &FullActiveModuleItemTreeInputQuery(module_id),
        )
    }

    fn active_item_tree(
        &self,
        db: &QueryDb<CompilerContext>,
        module_id: ModuleId,
        key: &impl nia_query::QueryKey<CompilerContext>,
    ) -> QueryResult<ActiveModuleItemTree> {
        self.loader_facts()
            .active_module_item_tree(module_id, ActiveModuleItemTreeFactKind::Full)?
            .ok_or_else(|| db.invalid_input(key, format!("missing loaded module {module_id:?}")))
    }

    pub(super) fn signature_item_tree(
        &self,
        db: &QueryDb<CompilerContext>,
        module_id: ModuleId,
        set: nia_item_tree::SignatureItemSet,
    ) -> QueryResult<ActiveModuleItemTree> {
        self.loader_facts()
            .active_module_item_tree(module_id, ActiveModuleItemTreeFactKind::Signature(set))?
            .ok_or_else(|| {
                db.invalid_input(
                    &SignatureItemTreeQuery(module_id, set),
                    format!("missing loaded module {module_id:?}"),
                )
            })
    }

    pub(super) fn signature_const_item_tree(
        &self,
        db: &QueryDb<CompilerContext>,
        module_id: ModuleId,
    ) -> QueryResult<ActiveModuleItemTree> {
        self.loader_facts()
            .active_module_item_tree(module_id, ActiveModuleItemTreeFactKind::ConstSignature)?
            .ok_or_else(|| {
                db.invalid_input(
                    &SignatureConstItemTreeQuery(module_id),
                    format!("missing loaded module {module_id:?}"),
                )
            })
    }

    pub(super) fn module_provider_summary(
        &self,
        db: &QueryDb<CompilerContext>,
        module_id: ModuleId,
    ) -> QueryResult<nia_provider_summary::ProviderSummary> {
        self.loader_facts()
            .module_provider_summary(module_id)?
            .ok_or_else(|| {
                db.invalid_input(
                    &ExtensionProviderSummaryQuery(module_id),
                    format!("missing loaded module {module_id:?}"),
                )
            })
    }

    pub(super) fn symbols(&self) -> nia_symbol_table::SymbolTable {
        self.loader_facts().symbols()
    }

    pub(super) fn provider_fact_worklist(&self) -> QueryResult<crate::ProviderFactSnapshot> {
        self.loader_facts().provider_facts()
    }

    pub(super) fn optimization(&self) -> OptimizationPolicy {
        self.inputs.read().optimization
    }

    pub(super) fn codegen_scope(&self) -> crate::CodegenScope {
        self.inputs.read().codegen_scope
    }

    pub(super) fn timings(&self) -> TimingMode {
        self.inputs.read().timings
    }

    pub(super) fn frontend_cache_namespace(&self) -> crate::FrontendCacheNamespace {
        crate::FrontendCacheNamespace::for_toolchain_with_profile_and_mode(
            &self.loader_facts.target(),
            self.loader_facts.runtime(),
            self.loader_facts.profile(),
            self.loader_facts.compilation_mode(),
            self.loader_facts.toolchain_identity(),
        )
    }

    pub(super) fn current_package(&self) -> Option<nia_package_metadata::PackageId> {
        self.inputs.read().current_package.clone()
    }

    pub(super) fn frontend_program_sources(
        &self,
        db: &QueryDb<CompilerContext>,
    ) -> QueryResult<Option<FrontendProgramSources>> {
        let modules = db.get(LoadedModulesQuery)?;
        let module_ids = resolve_stable_module_sequence_from_current_inputs(db, &modules)?;
        let mut by_module = HashMap::new();
        let mut module_by_path = HashMap::new();
        let mut path_by_module = HashMap::new();
        let mut fingerprint_inputs = Vec::new();
        for module_id in module_ids {
            let path = db.get(ModulePathQuery(module_id))?;
            // A program with an unreadable module has a load error and no
            // complete source identity, so it bypasses the frontend cache.
            let (Some(version), Some((source, len))) = (
                self.loader_facts.module_source_version(module_id)?,
                self.loader_facts.module_source_fingerprint(module_id)?,
            ) else {
                return Ok(None);
            };
            let module = StableModuleKey::from_source_identity(path.identity());
            let normalized_path = module.source_identity().normalized_path().to_string();
            if module_by_path
                .insert(normalized_path.clone(), module_id)
                .is_some()
                || path_by_module.insert(module_id, normalized_path).is_some()
            {
                return Ok(None);
            }
            fingerprint_inputs.push((module.clone(), source, len));
            by_module.insert(
                module_id,
                FrontendProgramSource {
                    module,
                    version,
                    len,
                },
            );
        }
        let fingerprint = crate::frontend_program_source_fingerprint(
            fingerprint_inputs
                .iter()
                .map(|(module, source, len)| (module, *source, *len)),
        );
        let sources = FrontendProgramSources {
            fingerprint,
            by_module,
            module_by_path,
            path_by_module,
        };
        self.observe_frontend_program_sources(&sources);
        Ok(Some(sources))
    }

    pub(super) fn stable_module_sequence(
        &self,
        module_ids: impl IntoIterator<Item = ModuleId>,
    ) -> QueryResult<StableModuleSequence> {
        let graph = self.loader_facts.module_graph()?;
        let mut identities = Vec::new();
        for module_id in module_ids {
            if graph.get(module_id).is_none() {
                return Err(QueryError::internal(format!(
                    "module {module_id:?} is missing from loaded module graph"
                )));
            }
            let Some(key) = graph.stable_key(module_id) else {
                return Err(QueryError::internal(format!(
                    "loaded module {module_id:?} has no stable key"
                )));
            };
            identities.push(key.source_identity().clone());
        }
        Ok(StableModuleSequence::from_source_identities(identities))
    }

    pub(super) fn resolve_stable_module_sequence(
        &self,
        sequence: &StableModuleSequence,
    ) -> QueryResult<Vec<ModuleId>> {
        let graph = self.loader_facts.module_graph()?;
        let mut module_ids = Vec::with_capacity(sequence.keys.len());
        for key in &sequence.keys {
            if let Some(module_id) = graph.module_id_for_stable_key(key) {
                module_ids.push(module_id);
                continue;
            }
            // A graph update can preserve the source identity while allocating a
            // fresh stable-key package/relationship. Keep the compatibility path
            // for that transition; the common unchanged-graph case uses the
            // indexed lookup above and avoids repeated loader path clones.
            let mut current = None;
            for module in graph.modules() {
                if self
                    .loader_facts
                    .module_path(module.id)?
                    .is_some_and(|path| path.identity() == *key.source_identity())
                {
                    current = Some(module.id);
                    break;
                }
            }
            let Some(module_id) = current else {
                return Err(QueryError::internal(format!(
                    "stable loaded module `{}` is missing from current loader facts",
                    key.source_identity().normalized_path()
                )));
            };
            module_ids.push(module_id);
        }
        Ok(module_ids)
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) struct FrontendProgramSource {
    pub(super) module: StableModuleKey,
    pub(super) version: SourceVersion,
    pub(super) len: usize,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) struct FrontendProgramSources {
    pub(super) fingerprint: crate::FrontendProgramSourceFingerprint,
    pub(super) by_module: HashMap<ModuleId, FrontendProgramSource>,
    pub(super) module_by_path: HashMap<String, ModuleId>,
    pub(super) path_by_module: HashMap<ModuleId, String>,
}

#[derive(Debug, Clone)]
pub(super) struct CheckCertificateContext {
    pub(super) namespace: crate::FrontendCacheNamespace,
    pub(super) entry: StableModuleKey,
    pub(super) input: FrontendCheckInputFingerprint,
    pub(super) scope: FrontendCheckScope,
    pub(super) source_lengths: BTreeMap<String, usize>,
}

impl CheckCertificateContext {
    pub(super) fn key(&self) -> FrontendCheckCertificateCacheKey {
        FrontendCheckCertificateCacheKey::new(self.namespace, &self.entry, self.input, self.scope)
    }

    pub(super) fn identity(&self) -> crate::signature_cache::CheckCertificateIdentity<'_> {
        crate::signature_cache::CheckCertificateIdentity {
            key: self.key(),
            namespace: self.namespace,
            entry: &self.entry,
            input: self.input,
            scope: self.scope,
            source_lengths: &self.source_lengths,
        }
    }
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub(super) struct StableModuleSequence {
    pub(super) keys: Vec<StableModuleKey>,
}

impl StableModuleSequence {
    pub(super) fn from_source_identities(
        source_identities: impl IntoIterator<Item = SourceIdentity>,
    ) -> Self {
        Self {
            keys: source_identities
                .into_iter()
                .map(StableModuleKey::from_source_identity)
                .collect(),
        }
    }
}

pub(super) fn stable_module_sequence(
    db: &QueryDb<CompilerContext>,
    module_ids: impl IntoIterator<Item = ModuleId>,
) -> QueryResult<StableModuleSequence> {
    db.context().stable_module_sequence(module_ids)
}

pub(super) fn resolve_stable_module_sequence_from_current_inputs(
    db: &QueryDb<CompilerContext>,
    sequence: &StableModuleSequence,
) -> QueryResult<Vec<ModuleId>> {
    db.context().resolve_stable_module_sequence(sequence)
}

pub(super) fn resolve_stable_module_sequence(
    db: &QueryDb<CompilerContext>,
    sequence: &StableModuleSequence,
) -> QueryResult<Vec<ModuleId>> {
    let _graph = db.get(super::ModuleGraphQuery)?;
    db.context().resolve_stable_module_sequence(sequence)
}

#[derive(Debug, Clone)]
pub(super) struct CompilerInputs {
    pub(super) optimization: OptimizationPolicy,
    pub(super) timings: TimingMode,
    pub(super) codegen_scope: CodegenScope,
    pub(super) current_package: Option<nia_package_metadata::PackageId>,
}

impl CompilerInputs {
    pub(super) fn new(request: CompileRequest) -> Self {
        Self {
            optimization: request.optimization.policy(),
            timings: request.timings,
            codegen_scope: request.codegen_scope,
            current_package: request.current_package,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) struct ExecutableFactEpoch {
    pub(super) entry_module: ModuleId,
    pub(super) runtime_root_modules: Vec<ModuleId>,
    pub(super) target: TargetConfig,
    pub(super) runtime: RuntimeSpec,
    pub(super) codegen_scope: CodegenScope,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) struct BodyActivationWorklist {
    pub(super) modules: Arc<HashMap<StableModuleKey, ModuleId>>,
}
