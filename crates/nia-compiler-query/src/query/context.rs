// SPDX-License-Identifier: GPL-3.0-or-later
use super::frontend_cache_publication::PendingFrontendCachePublications;
use super::{
    CompileRequest, CompilerQueryProviders, ExecutableFactSession, LoadedModulesQuery,
    ModulePathQuery, ModuleSourceVersionQuery, resolve_stable_module_sequence_from_current_inputs,
};
use crate::{
    CodegenScope, FrontendCheckCertificateCacheKey, FrontendCheckInputFingerprint,
    FrontendCheckScope, RuntimeSpec, TimingMode,
};
use nia_ids::ModuleId;
use nia_imports::{ModuleGraphSnapshot, StableModuleKey};
use nia_opt::OptimizationPolicy;
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
            let version = *db.get(ModuleSourceVersionQuery(module_id))?;
            let Some((source, len)) = self.loader_facts.module_source_fingerprint(module_id)?
            else {
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
