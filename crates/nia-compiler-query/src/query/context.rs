// SPDX-License-Identifier: GPL-3.0-or-later
use super::{CompileRequest, CompilerQueryProviders, ExecutableFactSession};
use crate::{
    FrontendCheckCertificateCacheKey, FrontendCheckInputFingerprint, FrontendCheckScope,
    RuntimeModel, TimingMode,
};
use nia_ids::ModuleId;
use nia_imports::StableModuleKey;
use nia_opt::OptimizationPolicy;
use nia_source::{SourceIdentity, SourceVersion};
use nia_target_config::TargetConfig;
use std::{
    collections::{BTreeMap, HashMap},
    sync::{Arc, RwLock},
};

pub(super) struct CompilerContext {
    pub(super) inputs: Arc<RwLock<CompilerInputs>>,
    pub(super) loader_facts: Arc<dyn crate::LoaderFactProvider>,
    pub(super) providers: CompilerQueryProviders,
    pub(super) executable_fact_session: Arc<std::sync::Mutex<ExecutableFactSession>>,
    pub(super) executable_fact_scheduler: std::sync::Mutex<()>,
    pub(super) type_store: Arc<nia_ty::TypeStore>,
    pub(super) diagnostic_store: nia_diagnostic::DiagnosticStore,
    pub(super) node_store: nia_node_id::NodeStore,
    pub(super) signature_cache: Option<Arc<crate::signature_cache::PersistentSignatureCache>>,
    pub(super) verify_frontend_cache: bool,
    pub(super) provider_demand_rounds: std::sync::atomic::AtomicU64,
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
}

impl CompilerInputs {
    pub(super) fn new(request: CompileRequest) -> Self {
        Self {
            optimization: request.optimization.policy(),
            timings: request.timings,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) struct ExecutableFactEpoch {
    pub(super) entry_module: ModuleId,
    pub(super) runtime_root_modules: Vec<ModuleId>,
    pub(super) modules: Vec<(ModuleId, SourceVersion)>,
    pub(super) target: TargetConfig,
    pub(super) runtime: RuntimeModel,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) struct BodyActivationWorklist {
    pub(super) modules: Arc<HashMap<StableModuleKey, ModuleId>>,
}
