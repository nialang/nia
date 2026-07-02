// SPDX-License-Identifier: GPL-3.0-or-later
use crate::{
    CheckedModule, CheckedProgram, CodegenProgram, LoadedModule, LoadedProgram, ProgramDiagnostic,
    RuntimeModel, TimingMode, module_diagnostics,
    program_signatures::{
        ExtensionMethodIndexModuleInput, ExtensionModuleInput, ModuleSignatureInput,
        VisibleExtensionsForModule, VisibleExtensionsInput, VisibleTypeSignatures,
        collect_extension_associated_value_index, collect_extension_method_index,
        collect_extension_methods, collect_program_comptimes, collect_program_enums,
        collect_program_functions_excluding, collect_program_globals, collect_program_structs,
        collect_program_trait_impls, collect_program_traits, collect_program_type_aliases,
        collect_program_unions, visible_extensions_for_module,
    },
    public_surface::compute_public_surfaces,
};
use nia_backend_lower::BackendLowerModuleInput;
use nia_comptime_check::{ComptimeCheck, ComptimeModuleLowering};
use nia_defs::{DefCollection, ModuleUsingScope, PublicSurfaces};
use nia_diagnostic::{Diagnostic, codes};
use nia_ids::{GlobalConstExprId, GlobalDefId, InternedTyId, ModuleId};
use nia_imports::ModuleGraph;
use nia_item_signatures::{
    ItemSignatures, ProgramComptimeSignature, ProgramEnumSignature, ProgramFunctionSignature,
    ProgramGlobalSignature, ProgramStructSignature, ProgramTraitSignature,
    ProgramTypeAliasSignature, ProgramUnionSignature, StructSignature, UnionSignature,
};
use nia_item_tree::{ActiveModuleItemTree, ModuleItemTree};
use nia_local_resolve::LocalResolution;
use nia_monomorphize::MonomorphizeModuleInput;
use nia_node_id::NodeOriginTable;
use nia_opt::{NiaOptimizationLevel, OptimizationPolicy};
use nia_parser::ParseError;
use nia_query::{QueryDb, QueryError, QueryFrame, QueryKey, QueryTrace};
use nia_source::{SourceIdentity, SourcePath, SourceVersion};
use nia_span::Span;
use nia_target_config::TargetConfig;
use nia_ty::{ArrayLenTy, TyKind};
use nia_type_lower::TypeLowering;
use nia_type_normalize::TypeNormalization;
use nia_type_resolve::TypeResolution;
use nia_value_resolve::ValueResolution;
use std::{
    collections::{HashMap, HashSet},
    env, fs, io,
    path::{Path, PathBuf},
    sync::{Arc, RwLock},
    thread,
    time::{Duration, Instant},
};

mod backend_lowering;
mod base;
mod checked;
mod checks;
mod diagnostics;
mod executable;
mod program;
mod providers;
mod resolve;
mod types;

use backend_lowering::*;
use base::*;
use checked::*;
use checks::*;
use diagnostics::*;
use executable::*;
use program::*;
use providers::*;
use resolve::*;
use types::*;

type ExtensionMethodsValue = Arc<ExtensionMethodsQueryValue>;
type ExtensionMethodIndexValue = Arc<ExtensionMethodIndexQueryValue>;
type ExtensionMethodSetValue = Arc<ExtensionMethodSetQueryValue>;
type ExtensionAssociatedValuesValue = Arc<ExtensionAssociatedValuesQueryValue>;
type VisibleExtensionsValue = Arc<VisibleExtensionsForModule>;

#[derive(Debug, Clone)]
pub struct CompileRequest {
    pub loaded: LoadedProgram,
    pub optimization: NiaOptimizationLevel,
    pub timings: TimingMode,
}

impl CompileRequest {
    pub fn new(loaded: LoadedProgram) -> Self {
        Self {
            loaded,
            optimization: NiaOptimizationLevel::default(),
            timings: TimingMode::Off,
        }
    }

    pub fn with_optimization(mut self, optimization: NiaOptimizationLevel) -> Self {
        self.optimization = optimization;
        self
    }

    pub fn with_timings(mut self, timings: TimingMode) -> Self {
        self.timings = timings;
        self
    }
}

#[derive(Clone)]
pub struct CompilerDatabase {
    db: QueryDb<CompilerContext>,
    inputs: Arc<RwLock<CompilerInputs>>,
}

impl CompilerDatabase {
    pub fn new(request: CompileRequest) -> Self {
        compiler_database_with_providers(request, CompilerQueryProviders::default())
    }

    pub fn check_program(&self) -> CheckedProgram {
        let _permit = compiler_work_permit();
        match std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            self.db.try_query(CheckedProgramQuery)
        })) {
            Ok(Ok(checked)) => checked,
            Ok(Err(err)) => checked_program_from_query_error(
                self.current_graph(),
                self.current_optimization(),
                err,
            ),
            Err(payload) => match payload.downcast::<QueryError>() {
                Ok(err) => checked_program_from_query_error(
                    self.current_graph(),
                    self.current_optimization(),
                    *err,
                ),
                Err(payload) => std::panic::resume_unwind(payload),
            },
        }
    }

    pub fn entry_check_program(&self) -> CheckedProgram {
        let _permit = compiler_work_permit();
        match std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            self.db.try_query(EntryCheckedProgramQuery)
        })) {
            Ok(Ok(checked)) => checked,
            Ok(Err(err)) => checked_program_from_query_error(
                self.current_graph(),
                self.current_optimization(),
                err,
            ),
            Err(payload) => match payload.downcast::<QueryError>() {
                Ok(err) => checked_program_from_query_error(
                    self.current_graph(),
                    self.current_optimization(),
                    *err,
                ),
                Err(payload) => std::panic::resume_unwind(payload),
            },
        }
    }

    pub fn codegen_program(&self) -> CodegenProgram {
        let _permit = compiler_work_permit();
        match std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            self.db.try_query(CodegenProgramQuery)
        })) {
            Ok(Ok(codegen)) => codegen,
            Ok(Err(err)) => codegen_program_from_query_error(
                self.current_graph(),
                self.current_optimization(),
                err,
            ),
            Err(payload) => match payload.downcast::<QueryError>() {
                Ok(err) => codegen_program_from_query_error(
                    self.current_graph(),
                    self.current_optimization(),
                    *err,
                ),
                Err(payload) => std::panic::resume_unwind(payload),
            },
        }
    }

    pub fn update(&self, request: CompileRequest) -> CompilerInvalidation {
        let new_inputs = CompilerInputs::new(request);
        let diff = {
            let mut inputs = self.inputs.write().expect("compiler input lock poisoned");
            let diff = CompilerInputDiff::between(&inputs, &new_inputs);
            *inputs = new_inputs;
            diff
        };
        self.invalidate_inputs(diff)
    }

    pub fn query_trace(&self) -> QueryTrace {
        self.db.query_trace()
    }

    fn current_graph(&self) -> ModuleGraph {
        self.inputs
            .read()
            .expect("compiler input lock poisoned")
            .loaded
            .graph
            .clone()
    }

    fn current_optimization(&self) -> OptimizationPolicy {
        self.inputs
            .read()
            .expect("compiler input lock poisoned")
            .optimization
    }

    fn invalidate_inputs(&self, diff: CompilerInputDiff) -> CompilerInvalidation {
        let mut invalidation = CompilerInvalidation::default();
        if diff.graph_changed {
            invalidation.extend(self.db.invalidate(ModuleGraphQuery));
        }
        if diff.loaded_modules_changed {
            invalidation.extend(self.db.invalidate(LoadedModulesQuery));
        }
        if diff.loaded_diagnostics_changed {
            invalidation.extend(self.db.invalidate(ProgramLoadDiagnosticsQuery));
        }
        if diff.target_changed {
            invalidation.extend(self.db.invalidate(CompilerTargetQuery));
        }
        if diff.runtime_changed {
            invalidation.extend(self.db.invalidate(CompilerRuntimeQuery));
        }
        if diff.optimization_changed {
            invalidation.extend(self.db.invalidate(CompilerOptimizationQuery));
        }
        if diff.timings_changed {
            invalidation.extend(self.db.invalidate(CompilerTimingsQuery));
        }
        for module in diff.changed_modules {
            for module_id in module.ids {
                if module.path {
                    invalidation.extend(self.db.invalidate(ModulePathQuery(module_id)));
                }
                if module.source_version {
                    invalidation.extend(self.db.invalidate(ModuleSourceVersionQuery(module_id)));
                }
                if module.full_item_tree {
                    invalidation
                        .extend(self.db.invalidate(FullModuleItemTreeInputQuery(module_id)));
                }
                if module.origins {
                    invalidation.extend(self.db.invalidate(ModuleOriginsQuery(module_id)));
                }
                if module.parse_errors {
                    invalidation.extend(self.db.invalidate(ModuleParseErrorsQuery(module_id)));
                }
                if module.item_tree {
                    invalidation.extend(self.db.invalidate(ModuleItemTreeInputQuery(module_id)));
                }
                if module.declaration_item_tree {
                    invalidation.extend(
                        self.db
                            .invalidate(DeclarationModuleItemTreeInputQuery(module_id)),
                    );
                }
                if module.active_item_tree {
                    invalidation.extend(
                        self.db
                            .invalidate(ActiveModuleItemTreeInputQuery(module_id)),
                    );
                }
                if module.declaration_active_item_tree {
                    invalidation.extend(
                        self.db
                            .invalidate(DeclarationActiveModuleItemTreeInputQuery(module_id)),
                    );
                }
                if module.signature_function_items {
                    invalidation.extend(self.db.invalidate(SignatureItemTreeQuery(
                        module_id,
                        nia_item_tree::SignatureItemSet::Functions,
                    )));
                }
                if module.signature_extension_function_items {
                    invalidation.extend(self.db.invalidate(SignatureItemTreeQuery(
                        module_id,
                        nia_item_tree::SignatureItemSet::ExtensionFunctions,
                    )));
                }
                if module.signature_value_items {
                    invalidation.extend(self.db.invalidate(SignatureItemTreeQuery(
                        module_id,
                        nia_item_tree::SignatureItemSet::Values,
                    )));
                }
                if module.signature_type_items {
                    invalidation.extend(self.db.invalidate(SignatureItemTreeQuery(
                        module_id,
                        nia_item_tree::SignatureItemSet::Types,
                    )));
                }
                if module.signature_trait_items {
                    invalidation.extend(self.db.invalidate(SignatureItemTreeQuery(
                        module_id,
                        nia_item_tree::SignatureItemSet::Traits,
                    )));
                }
                if module.full_active_item_tree {
                    invalidation.extend(
                        self.db
                            .invalidate(FullActiveModuleItemTreeInputQuery(module_id)),
                    );
                }
            }
        }
        invalidation
    }
}

pub fn compiler_work_permit() -> CompilerWorkPermit {
    if !compiler_check_slots_enabled() {
        return CompilerWorkPermit { slot: None };
    }
    CompilerWorkPermit {
        slot: Some(acquire_compiler_check_slot()),
    }
}

fn compiler_check_slots_enabled() -> bool {
    env_limit("NIA_COMPILER_CHECK_LIMIT").is_some()
}

pub struct CompilerWorkPermit {
    slot: Option<PathBuf>,
}

impl Drop for CompilerWorkPermit {
    fn drop(&mut self) {
        if let Some(slot) = self.slot.take() {
            let _ = fs::remove_dir_all(slot);
        }
    }
}

fn acquire_compiler_check_slot() -> PathBuf {
    const PERMIT_TIMEOUT: Duration = Duration::from_secs(5 * 60);
    const STALE_AFTER: Duration = Duration::from_secs(15 * 60);

    let root = compiler_check_slot_root();
    fs::create_dir_all(&root).expect("create compiler check slot root");
    let start = Instant::now();
    let mut sleep = Duration::from_millis(10);
    loop {
        for index in 0..compiler_check_limit() {
            let slot = root.join(index.to_string());
            match fs::create_dir(&slot) {
                Ok(()) => {
                    write_process_owner(&slot);
                    return slot;
                }
                Err(error) if error.kind() == io::ErrorKind::AlreadyExists => {
                    reclaim_stale_compiler_check_slot(&slot, STALE_AFTER);
                }
                Err(error) => panic!("create compiler check slot {}: {error}", slot.display()),
            }
        }
        if start.elapsed() >= PERMIT_TIMEOUT {
            panic!(
                "timed out after {PERMIT_TIMEOUT:?} waiting for compiler check slot in {}",
                root.display()
            );
        }
        thread::sleep(sleep);
        sleep = (sleep * 2).min(Duration::from_millis(250));
    }
}

fn compiler_check_limit() -> usize {
    const MAX_CHECKS: usize = 4;
    const BYTES_PER_CHECK: usize = 1024 * 1024 * 1024;

    if let Some(limit) = env_limit("NIA_COMPILER_CHECK_LIMIT") {
        return limit;
    }
    let cpu_limit = available_parallelism().clamp(1, MAX_CHECKS);
    let memory_limit = memory_limited_parallelism(BYTES_PER_CHECK).unwrap_or(cpu_limit);
    cpu_limit.min(memory_limit).clamp(1, MAX_CHECKS)
}

fn env_limit(name: &str) -> Option<usize> {
    env::var(name)
        .ok()
        .and_then(|value| value.parse::<usize>().ok())
        .filter(|value| *value > 0)
}

fn compiler_check_slot_root() -> PathBuf {
    let mut root = env::temp_dir();
    root.push("nia_compiler_check_slots");
    root.push(env!("CARGO_MANIFEST_DIR").replace(std::path::MAIN_SEPARATOR, "_"));
    root
}

fn reclaim_stale_compiler_check_slot(slot: &Path, stale_after: Duration) {
    if compiler_check_slot_owner_is_alive(slot) {
        return;
    }
    if compiler_check_slot_owner_is_unknown(slot)
        && !compiler_check_slot_is_stale_by_age(slot, stale_after)
    {
        return;
    }
    let _ = fs::remove_dir_all(slot);
}

fn compiler_check_slot_owner_is_unknown(slot: &Path) -> bool {
    read_process_owner(slot).is_none()
}

fn compiler_check_slot_owner_is_alive(slot: &Path) -> bool {
    let Some((pid, start_time)) = read_process_owner(slot) else {
        return false;
    };
    process_is_alive(pid, start_time)
}

fn write_process_owner(slot: &Path) {
    let pid = std::process::id();
    let start_time = process_start_time(pid).unwrap_or(0);
    let _ = fs::write(slot.join("owner"), format!("{pid} {start_time}"));
}

fn read_process_owner(slot: &Path) -> Option<(u32, u64)> {
    let owner = fs::read_to_string(slot.join("owner")).ok()?;
    let mut parts = owner.split_whitespace();
    let pid = parts.next()?.parse().ok()?;
    let start_time = parts.next()?.parse().ok()?;
    Some((pid, start_time))
}

fn compiler_check_slot_is_stale_by_age(slot: &Path, stale_after: Duration) -> bool {
    fs::metadata(slot)
        .and_then(|metadata| metadata.modified())
        .ok()
        .and_then(|modified| modified.elapsed().ok())
        .is_some_and(|elapsed| elapsed >= stale_after)
}

#[cfg(target_os = "linux")]
fn process_is_alive(pid: u32, expected_start_time: u64) -> bool {
    let Some(start_time) = process_start_time(pid) else {
        return false;
    };
    expected_start_time == 0 || start_time == expected_start_time
}

#[cfg(not(target_os = "linux"))]
fn process_is_alive(_pid: u32, _expected_start_time: u64) -> bool {
    true
}

#[cfg(target_os = "linux")]
fn process_start_time(pid: u32) -> Option<u64> {
    let stat = fs::read_to_string(
        std::path::Path::new("/proc")
            .join(pid.to_string())
            .join("stat"),
    )
    .ok()?;
    stat.rsplit_once(") ")?
        .1
        .split_whitespace()
        .nth(19)?
        .parse()
        .ok()
}

#[cfg(not(target_os = "linux"))]
fn process_start_time(_pid: u32) -> Option<u64> {
    None
}

fn available_parallelism() -> usize {
    thread::available_parallelism()
        .map(usize::from)
        .unwrap_or(1)
}

fn memory_limited_parallelism(bytes_per_slot: usize) -> Option<usize> {
    let mem_available_kb = linux_mem_available_kb()?;
    let available_bytes = mem_available_kb.saturating_mul(1024);
    Some((available_bytes / bytes_per_slot).max(1))
}

#[cfg(target_os = "linux")]
fn linux_mem_available_kb() -> Option<usize> {
    let meminfo = fs::read_to_string("/proc/meminfo").ok()?;
    for line in meminfo.lines() {
        let Some(rest) = line.strip_prefix("MemAvailable:") else {
            continue;
        };
        return rest.split_whitespace().next()?.parse().ok();
    }
    None
}

#[cfg(not(target_os = "linux"))]
fn linux_mem_available_kb() -> Option<usize> {
    None
}

impl std::fmt::Debug for CompilerDatabase {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let inputs = self.inputs.read().expect("compiler input lock poisoned");
        f.debug_struct("CompilerDatabase")
            .field("graph", &inputs.loaded.graph)
            .field("optimization", &inputs.optimization)
            .finish_non_exhaustive()
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct CompilerInvalidation {
    pub invalidated: Vec<QueryFrame>,
}

impl CompilerInvalidation {
    fn extend(&mut self, invalidation: nia_query::QueryInvalidation) {
        for frame in invalidation.invalidated {
            if !self.invalidated.contains(&frame) {
                self.invalidated.push(frame);
            }
        }
    }
}

fn compiler_database_with_providers(
    request: CompileRequest,
    providers: CompilerQueryProviders,
) -> CompilerDatabase {
    let inputs = Arc::new(RwLock::new(CompilerInputs::new(request)));
    let db = QueryDb::new(CompilerContext {
        inputs: inputs.clone(),
        providers,
    });
    CompilerDatabase { db, inputs }
}

fn checked_program_from_query_error(
    graph: ModuleGraph,
    optimization: OptimizationPolicy,
    err: QueryError,
) -> CheckedProgram {
    CheckedProgram {
        graph,
        optimization,
        modules: Vec::new(),
        diagnostics: vec![ProgramDiagnostic {
            path: SourcePath::new("<query>"),
            diagnostic: query_error_diagnostic(err),
        }],
    }
}

fn codegen_program_from_query_error(
    graph: ModuleGraph,
    optimization: OptimizationPolicy,
    err: QueryError,
) -> CodegenProgram {
    CodegenProgram {
        graph,
        optimization,
        modules: Vec::new(),
        monomorphization: nia_monomorphize::Monomorphization {
            instances: Vec::new(),
            type_interners: std::collections::HashMap::new(),
            diagnostics: Vec::new(),
        },
        backend_lowering: nia_backend_lower::BackendLowering {
            program: nia_backend_ir::BackendProgram {
                modules: Vec::new(),
            },
            optimization,
            optimization_report: nia_backend_lower::BackendOptimizationReport::default(),
            diagnostics: Vec::new(),
        },
        diagnostics: vec![ProgramDiagnostic {
            path: SourcePath::new("<query>"),
            diagnostic: query_error_diagnostic(err),
        }],
    }
}

fn query_error_diagnostic(err: QueryError) -> Diagnostic {
    match err {
        QueryError::Cycle { cycle } => {
            let mut message = String::from("query cycle detected");
            for frame in cycle {
                message.push_str("\n  ");
                message.push_str(&frame.description);
            }
            Diagnostic::internal_error(codes::QUERY_ENGINE, message)
                .primary_fallback(Span::default(), "query cycle has no source span")
                .finish()
        }
        QueryError::InvalidInput { query, message } => {
            let message = format!("invalid query input for {}: {message}", query.description);
            Diagnostic::internal_error(codes::QUERY_ENGINE, message)
                .primary_fallback(Span::default(), "query input has no source span")
                .finish()
        }
    }
}

struct CompilerContext {
    inputs: Arc<RwLock<CompilerInputs>>,
    providers: CompilerQueryProviders,
}

#[derive(Debug, Clone)]
struct CompilerInputs {
    loaded: LoadedProgram,
    modules_by_id: HashMap<ModuleId, usize>,
    modules_by_source_identity: HashMap<SourceIdentity, usize>,
    target: TargetConfig,
    runtime: crate::RuntimeModel,
    optimization: OptimizationPolicy,
    timings: TimingMode,
}

impl CompilerInputs {
    fn new(request: CompileRequest) -> Self {
        let loaded = request.loaded;
        validate_loaded_module_identities(&loaded);
        let modules_by_id = index_loaded_modules(&loaded);
        let modules_by_source_identity = index_loaded_module_identities(&loaded);
        Self {
            target: loaded.target.clone(),
            runtime: loaded.runtime,
            loaded,
            modules_by_id,
            modules_by_source_identity,
            optimization: request.optimization.policy(),
            timings: request.timings,
        }
    }
}

fn validate_loaded_module_identities(loaded: &LoadedProgram) {
    for module in &loaded.modules {
        let expected = module.path.identity();
        if module.source_identity != expected {
            panic!(
                "Nia ICE: loaded module {:?} has source identity `{}` but path `{}` implies `{}`",
                module.id,
                module.source_identity.normalized_path(),
                module.path.as_str(),
                expected.normalized_path()
            );
        }
    }
}

impl CompilerContext {
    fn module_field<T, K>(
        &self,
        db: &QueryDb<CompilerContext>,
        key: &K,
        module_id: ModuleId,
        field: impl FnOnce(&LoadedModule) -> T,
    ) -> T
    where
        K: QueryKey<CompilerContext>,
    {
        let inputs = self.inputs.read().expect("compiler input lock poisoned");
        let Some(module) = inputs.loaded_module(module_id) else {
            db.invalid_input(key, format!("missing loaded module {module_id:?}"));
        };
        field(module)
    }

    fn loaded_modules(&self) -> Vec<ModuleId> {
        self.inputs
            .read()
            .expect("compiler input lock poisoned")
            .loaded
            .modules
            .iter()
            .map(|module| module.id)
            .collect()
    }

    fn loaded_module(&self, module_id: ModuleId) -> Option<LoadedModule> {
        let inputs = self.inputs.read().expect("compiler input lock poisoned");
        inputs
            .modules_by_id
            .get(&module_id)
            .and_then(|index| inputs.loaded.modules.get(*index))
            .cloned()
    }

    fn module_path(&self, db: &QueryDb<CompilerContext>, module_id: ModuleId) -> SourcePath {
        self.module_field(db, &ModulePathQuery(module_id), module_id, |module| {
            module.path.clone()
        })
    }

    fn module_source_version(
        &self,
        db: &QueryDb<CompilerContext>,
        module_id: ModuleId,
    ) -> SourceVersion {
        self.module_field(
            db,
            &ModuleSourceVersionQuery(module_id),
            module_id,
            |module| module.source_version,
        )
    }

    fn module_origins(
        &self,
        db: &QueryDb<CompilerContext>,
        module_id: ModuleId,
    ) -> NodeOriginTable {
        self.module_field(db, &ModuleOriginsQuery(module_id), module_id, |module| {
            module.origins.clone()
        })
    }

    fn module_parse_errors(
        &self,
        db: &QueryDb<CompilerContext>,
        module_id: ModuleId,
    ) -> Vec<ParseError> {
        self.module_field(
            db,
            &ModuleParseErrorsQuery(module_id),
            module_id,
            |module| module.parse_errors.clone(),
        )
    }

    fn module_item_tree(
        &self,
        db: &QueryDb<CompilerContext>,
        module_id: ModuleId,
    ) -> ModuleItemTree {
        self.module_field(
            db,
            &ModuleItemTreeInputQuery(module_id),
            module_id,
            |module| module.item_tree.clone(),
        )
    }

    fn declaration_module_item_tree(
        &self,
        db: &QueryDb<CompilerContext>,
        module_id: ModuleId,
    ) -> ModuleItemTree {
        self.module_field(
            db,
            &DeclarationModuleItemTreeInputQuery(module_id),
            module_id,
            |module| module.item_tree.clone(),
        )
    }

    fn active_module_item_tree(
        &self,
        db: &QueryDb<CompilerContext>,
        module_id: ModuleId,
    ) -> ActiveModuleItemTree {
        self.module_field(
            db,
            &ActiveModuleItemTreeInputQuery(module_id),
            module_id,
            |module| module.active_item_tree.clone(),
        )
    }

    fn declaration_active_module_item_tree(
        &self,
        db: &QueryDb<CompilerContext>,
        module_id: ModuleId,
    ) -> ActiveModuleItemTree {
        self.module_field(
            db,
            &DeclarationActiveModuleItemTreeInputQuery(module_id),
            module_id,
            |module| module.active_item_tree.clone(),
        )
    }

    fn signature_item_tree(
        &self,
        db: &QueryDb<CompilerContext>,
        module_id: ModuleId,
        set: nia_item_tree::SignatureItemSet,
    ) -> ActiveModuleItemTree {
        self.module_field(
            db,
            &SignatureItemTreeQuery(module_id, set),
            module_id,
            |module| module.active_item_tree.signature_items(set),
        )
    }

    fn path_for_module(&self, module_id: ModuleId) -> SourcePath {
        self.loaded_module(module_id)
            .unwrap_or_else(|| panic!("Nia ICE: missing loaded module {module_id:?}"))
            .path
            .clone()
    }

    fn module_graph(&self) -> ModuleGraph {
        self.inputs
            .read()
            .expect("compiler input lock poisoned")
            .loaded
            .graph
            .clone()
    }

    fn load_diagnostics(&self) -> Vec<ProgramDiagnostic> {
        self.inputs
            .read()
            .expect("compiler input lock poisoned")
            .loaded
            .diagnostics
            .clone()
    }

    fn target(&self) -> TargetConfig {
        self.inputs
            .read()
            .expect("compiler input lock poisoned")
            .target
            .clone()
    }

    fn runtime(&self) -> crate::RuntimeModel {
        self.inputs
            .read()
            .expect("compiler input lock poisoned")
            .runtime
    }

    fn optimization(&self) -> OptimizationPolicy {
        self.inputs
            .read()
            .expect("compiler input lock poisoned")
            .optimization
    }

    fn timings(&self) -> TimingMode {
        self.inputs
            .read()
            .expect("compiler input lock poisoned")
            .timings
    }
}

fn index_loaded_modules(loaded: &LoadedProgram) -> HashMap<ModuleId, usize> {
    let mut modules_by_id = HashMap::new();
    for (index, module) in loaded.modules.iter().enumerate() {
        if let Some(existing) = modules_by_id.insert(module.id, index) {
            panic!(
                "Nia ICE: duplicate loaded module id {:?} at indexes {existing} and {index}",
                module.id
            );
        }
    }
    modules_by_id
}

fn index_loaded_module_identities(loaded: &LoadedProgram) -> HashMap<SourceIdentity, usize> {
    let mut modules_by_source_identity = HashMap::new();
    for (index, module) in loaded.modules.iter().enumerate() {
        if let Some(existing) =
            modules_by_source_identity.insert(module.source_identity.clone(), index)
        {
            panic!(
                "Nia ICE: duplicate source identity `{}` for loaded modules {:?} and {:?}",
                module.source_identity.normalized_path(),
                loaded.modules[existing].id,
                module.id
            );
        }
    }
    modules_by_source_identity
}

#[derive(Debug, Default)]
struct CompilerInputDiff {
    graph_changed: bool,
    loaded_modules_changed: bool,
    loaded_diagnostics_changed: bool,
    target_changed: bool,
    runtime_changed: bool,
    optimization_changed: bool,
    timings_changed: bool,
    changed_modules: Vec<ChangedModuleInput>,
}

impl CompilerInputDiff {
    fn between(old: &CompilerInputs, new: &CompilerInputs) -> Self {
        let changed_modules = changed_loaded_modules(old, new);
        Self {
            graph_changed: old.loaded.graph != new.loaded.graph,
            loaded_modules_changed: loaded_module_ids(old) != loaded_module_ids(new)
                || loaded_module_identity_assignments(old)
                    != loaded_module_identity_assignments(new),
            loaded_diagnostics_changed: old.loaded.diagnostics != new.loaded.diagnostics,
            target_changed: old.target != new.target,
            runtime_changed: old.runtime != new.runtime,
            optimization_changed: old.optimization != new.optimization,
            timings_changed: old.timings != new.timings,
            changed_modules,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct ChangedModuleInput {
    ids: Vec<ModuleId>,
    path: bool,
    source_identity: bool,
    source_version: bool,
    origins: bool,
    parse_errors: bool,
    item_tree: bool,
    declaration_item_tree: bool,
    full_item_tree: bool,
    active_item_tree: bool,
    declaration_active_item_tree: bool,
    signature_function_items: bool,
    signature_extension_function_items: bool,
    signature_value_items: bool,
    signature_type_items: bool,
    signature_trait_items: bool,
    full_active_item_tree: bool,
}

impl ChangedModuleInput {
    fn between_source_identity(
        old: Option<&LoadedModule>,
        new: Option<&LoadedModule>,
    ) -> Option<Self> {
        let ids = changed_module_ids(old, new);
        if ids.is_empty() {
            return None;
        }

        let changed = match (old, new) {
            (Some(old), Some(new)) if old.id == new.id => Self {
                ids,
                path: old.path != new.path,
                source_identity: old.source_identity != new.source_identity,
                source_version: old.source_version != new.source_version,
                origins: old.origins != new.origins,
                parse_errors: old.parse_errors != new.parse_errors,
                item_tree: !old.item_tree.definition_eq(&new.item_tree),
                declaration_item_tree: !old.item_tree.declaration_eq(&new.item_tree),
                full_item_tree: old.item_tree != new.item_tree,
                active_item_tree: !old.active_item_tree.definition_eq(&new.active_item_tree),
                declaration_active_item_tree: !old
                    .active_item_tree
                    .declaration_eq(&new.active_item_tree),
                signature_function_items: !old
                    .active_item_tree
                    .signature_items(nia_item_tree::SignatureItemSet::Functions)
                    .declaration_eq(
                        &new.active_item_tree
                            .signature_items(nia_item_tree::SignatureItemSet::Functions),
                    ),
                signature_extension_function_items: !old
                    .active_item_tree
                    .signature_items(nia_item_tree::SignatureItemSet::ExtensionFunctions)
                    .declaration_eq(
                        &new.active_item_tree
                            .signature_items(nia_item_tree::SignatureItemSet::ExtensionFunctions),
                    ),
                signature_value_items: !old
                    .active_item_tree
                    .signature_items(nia_item_tree::SignatureItemSet::Values)
                    .declaration_eq(
                        &new.active_item_tree
                            .signature_items(nia_item_tree::SignatureItemSet::Values),
                    ),
                signature_type_items: !old
                    .active_item_tree
                    .signature_items(nia_item_tree::SignatureItemSet::Types)
                    .declaration_eq(
                        &new.active_item_tree
                            .signature_items(nia_item_tree::SignatureItemSet::Types),
                    ),
                signature_trait_items: !old
                    .active_item_tree
                    .signature_items(nia_item_tree::SignatureItemSet::Traits)
                    .declaration_eq(
                        &new.active_item_tree
                            .signature_items(nia_item_tree::SignatureItemSet::Traits),
                    ),
                full_active_item_tree: old.active_item_tree != new.active_item_tree,
            },
            (Some(_), Some(_)) => Self::all_inputs_changed(ids),
            (Some(_), None) | (None, Some(_)) => Self {
                ids,
                path: true,
                source_identity: true,
                source_version: true,
                origins: true,
                parse_errors: true,
                item_tree: true,
                declaration_item_tree: true,
                full_item_tree: true,
                active_item_tree: true,
                declaration_active_item_tree: true,
                signature_function_items: true,
                signature_extension_function_items: true,
                signature_value_items: true,
                signature_type_items: true,
                signature_trait_items: true,
                full_active_item_tree: true,
            },
            (None, None) => return None,
        };
        if changed.path
            || changed.source_identity
            || changed.source_version
            || changed.origins
            || changed.parse_errors
            || changed.item_tree
            || changed.declaration_item_tree
            || changed.full_item_tree
            || changed.active_item_tree
            || changed.declaration_active_item_tree
            || changed.signature_function_items
            || changed.signature_extension_function_items
            || changed.signature_value_items
            || changed.signature_type_items
            || changed.signature_trait_items
            || changed.full_active_item_tree
        {
            Some(changed)
        } else {
            None
        }
    }

    fn all_inputs_changed(ids: Vec<ModuleId>) -> Self {
        Self {
            ids,
            path: true,
            source_identity: true,
            source_version: true,
            origins: true,
            parse_errors: true,
            item_tree: true,
            declaration_item_tree: true,
            full_item_tree: true,
            active_item_tree: true,
            declaration_active_item_tree: true,
            signature_function_items: true,
            signature_extension_function_items: true,
            signature_value_items: true,
            signature_type_items: true,
            signature_trait_items: true,
            full_active_item_tree: true,
        }
    }
}

fn changed_loaded_modules(old: &CompilerInputs, new: &CompilerInputs) -> Vec<ChangedModuleInput> {
    let source_identities = old
        .loaded
        .modules
        .iter()
        .map(|module| module.source_identity.clone())
        .chain(
            new.loaded
                .modules
                .iter()
                .map(|module| module.source_identity.clone()),
        )
        .collect::<HashSet<_>>();
    let mut changed = source_identities
        .into_iter()
        .filter_map(|source_identity| {
            ChangedModuleInput::between_source_identity(
                old.loaded_module_by_source_identity(&source_identity),
                new.loaded_module_by_source_identity(&source_identity),
            )
        })
        .collect::<Vec<_>>();
    changed.sort_by_key(|module| module.ids.first().copied().unwrap_or(ModuleId(u32::MAX)).0);
    changed
}

fn changed_module_ids(old: Option<&LoadedModule>, new: Option<&LoadedModule>) -> Vec<ModuleId> {
    let mut ids = Vec::new();
    if let Some(module) = old {
        ids.push(module.id);
    }
    if let Some(module) = new {
        ids.push(module.id);
    }
    ids.sort();
    ids.dedup();
    ids
}

fn loaded_module_ids(inputs: &CompilerInputs) -> Vec<ModuleId> {
    inputs
        .loaded
        .modules
        .iter()
        .map(|module| module.id)
        .collect()
}

fn loaded_module_identity_assignments(inputs: &CompilerInputs) -> Vec<(ModuleId, SourceIdentity)> {
    let mut assignments = inputs
        .loaded
        .modules
        .iter()
        .map(|module| (module.id, module.source_identity.clone()))
        .collect::<Vec<_>>();
    assignments.sort_by_key(|(id, _)| *id);
    assignments
}

impl CompilerInputs {
    fn loaded_module(&self, module_id: ModuleId) -> Option<&LoadedModule> {
        self.modules_by_id
            .get(&module_id)
            .and_then(|index| self.loaded.modules.get(*index))
    }

    fn loaded_module_by_source_identity(
        &self,
        source_identity: &SourceIdentity,
    ) -> Option<&LoadedModule> {
        self.modules_by_source_identity
            .get(source_identity)
            .and_then(|index| self.loaded.modules.get(*index))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::RuntimeModel;
    use nia_sema_ir::SemanticValueUse;
    use nia_source::{SourceId, SourceRevision};

    fn loaded_program_with_modules(modules: Vec<LoadedModule>) -> LoadedProgram {
        LoadedProgram {
            graph: ModuleGraph::new(SourcePath::new("main.nia")),
            target: TargetConfig::host(),
            runtime: RuntimeModel::Bare,
            modules,
            diagnostics: Vec::new(),
        }
    }

    fn loaded_program_with_entry_child(
        entry: LoadedModule,
        child_name: &str,
        child: LoadedModule,
    ) -> LoadedProgram {
        let mut graph = ModuleGraph::new(entry.path.clone());
        graph
            .intern_declared_child(
                entry.id,
                child_name,
                nia_ids::Visibility::Public,
                Span::default(),
            )
            .expect("intern child module");
        LoadedProgram {
            graph,
            target: TargetConfig::host(),
            runtime: RuntimeModel::Bare,
            modules: vec![entry, child],
            diagnostics: Vec::new(),
        }
    }

    fn loaded_module(id: ModuleId, path: &str, source: &str) -> LoadedModule {
        loaded_module_with_revision(id, path, source, SourceRevision::INITIAL)
    }

    fn loaded_module_with_revision(
        id: ModuleId,
        path: &str,
        source: &str,
        revision: SourceRevision,
    ) -> LoadedModule {
        let source_version = nia_source::SourceVersion {
            id: SourceId(id.0),
            revision,
        };
        let syntax = nia_syntax::parse_source(source, Some(source_version));
        let (module, parse_errors, origins) = nia_parser::parse_module_syntax_with_origins(&syntax);
        assert!(parse_errors.is_empty(), "{parse_errors:?}");
        let item_tree = ModuleItemTree::from_module(&module);
        LoadedModule {
            id,
            path: SourcePath::new(path),
            source_identity: SourcePath::new(path).identity(),
            source_version,
            item_tree: item_tree.clone(),
            active_item_tree: ActiveModuleItemTree::new(
                item_tree.items.clone(),
                Default::default(),
            ),
            parse_errors,
            origins,
        }
    }

    fn query_db(loaded: LoadedProgram) -> QueryDb<CompilerContext> {
        let inputs = Arc::new(RwLock::new(CompilerInputs::new(CompileRequest::new(
            loaded,
        ))));
        QueryDb::new(CompilerContext {
            inputs,
            providers: CompilerQueryProviders::default(),
        })
    }

    fn module_id_for_source_identity(
        db: &QueryDb<CompilerContext>,
        identity: &SourceIdentity,
    ) -> Option<ModuleId> {
        let inputs = db
            .context()
            .inputs
            .read()
            .expect("compiler input lock poisoned");
        inputs
            .loaded_module_by_source_identity(identity)
            .map(|module| module.id)
    }

    fn query_executions(trace: &QueryTrace, name: &'static str) -> usize {
        trace
            .queries
            .iter()
            .filter(|query| query.frame.name == name)
            .map(|query| query.stats.executions)
            .sum()
    }

    fn query_cache_hits(trace: &QueryTrace, name: &'static str) -> usize {
        trace
            .queries
            .iter()
            .filter(|query| query.frame.name == name)
            .map(|query| query.stats.cache_hits)
            .sum()
    }

    fn is_body_signature_query(name: &str) -> bool {
        matches!(
            name,
            "program_body_function_signatures"
                | "program_body_value_signatures"
                | "program_body_type_signatures"
                | "program_body_trait_signatures"
        )
    }

    fn depends_on_body_signature_query(trace: &QueryTrace, from: &str) -> bool {
        trace.dependencies.iter().any(|dependency| {
            dependency.from.name == from && is_body_signature_query(dependency.to.name)
        })
    }

    fn assert_query_executions_unchanged(
        before: &QueryTrace,
        after: &QueryTrace,
        name: &'static str,
    ) {
        assert_eq!(
            query_executions(before, name),
            query_executions(after, name),
            "{name} should have been reused"
        );
    }

    #[test]
    fn public_options_flow_through_compiler_query_context() {
        for level in [
            NiaOptimizationLevel::O0,
            NiaOptimizationLevel::O1,
            NiaOptimizationLevel::O2,
            NiaOptimizationLevel::O3,
            NiaOptimizationLevel::Os,
            NiaOptimizationLevel::Oz,
        ] {
            let loaded = loaded_program_with_modules(vec![loaded_module(
                ModuleId(0),
                "main.nia",
                r#"
static zeroes: [4]i32 = [0; 4];

fn main() i32 {
    zeroes[0]
}
"#,
            )]);
            let checked =
                CompilerDatabase::new(CompileRequest::new(loaded).with_optimization(level))
                    .codegen_program();
            let policy = level.policy();

            assert!(
                checked.diagnostics.is_empty(),
                "{level:?}: {:?}",
                checked.diagnostics
            );
            assert_eq!(checked.optimization, policy, "{level:?}");
            assert_eq!(checked.backend_lowering.optimization, policy, "{level:?}");
            assert_eq!(
                checked
                    .backend_lowering
                    .optimization_report
                    .enabled_global_passes,
                if policy.prefer_size
                    || policy.const_fold.at_least(nia_opt::OptimizationDepth::Full)
                {
                    vec!["simplify-static-init"]
                } else {
                    Vec::new()
                },
                "{level:?}"
            );
        }
    }

    #[test]
    fn compiler_database_exposes_query_trace() {
        let database =
            CompilerDatabase::new(CompileRequest::new(loaded_program_with_modules(vec![
                loaded_module(ModuleId(0), "main.nia", "fn main() i32 { 0 }"),
            ])));

        let checked = database.check_program();
        let trace = database.query_trace();

        assert!(checked.diagnostics.is_empty(), "{:?}", checked.diagnostics);
        assert!(trace.dependencies.iter().any(|dependency| {
            dependency.from.name == "checked_program" && dependency.to.name == "checked_modules"
        }));
    }

    #[test]
    fn compiler_inputs_index_modules_by_source_identity() {
        let loaded = loaded_program_with_modules(vec![
            loaded_module(ModuleId(0), "main.nia", "fn main() i32 { 0 }"),
            loaded_module(ModuleId(1), "pkg/root.nia", "pub fn value() i32 { 1 }"),
        ]);
        let db = query_db(loaded);

        assert_eq!(
            module_id_for_source_identity(&db, &SourcePath::new("pkg/root.nia").identity()),
            Some(ModuleId(1))
        );
    }

    #[test]
    #[should_panic(expected = "Nia ICE: duplicate loaded module id")]
    fn compiler_inputs_reject_duplicate_module_ids() {
        let _ = CompilerInputs::new(CompileRequest::new(loaded_program_with_modules(vec![
            loaded_module(ModuleId(0), "main.nia", "fn main() i32 { 0 }"),
            loaded_module(ModuleId(0), "other.nia", "pub fn value() i32 { 1 }"),
        ])));
    }

    #[test]
    #[should_panic(expected = "Nia ICE: duplicate source identity")]
    fn compiler_inputs_reject_duplicate_source_identities() {
        let _ = CompilerInputs::new(CompileRequest::new(loaded_program_with_modules(vec![
            loaded_module(ModuleId(0), "main.nia", "fn main() i32 { 0 }"),
            loaded_module(ModuleId(1), "main.nia", "pub fn value() i32 { 1 }"),
        ])));
    }

    #[test]
    #[should_panic(expected = "Nia ICE: loaded module")]
    fn compiler_inputs_reject_path_identity_mismatch() {
        let mut module = loaded_module(ModuleId(0), "main.nia", "fn main() i32 { 0 }");
        module.source_identity = SourcePath::new("other.nia").identity();

        let _ = CompilerInputs::new(CompileRequest::new(loaded_program_with_modules(vec![
            module,
        ])));
    }

    #[test]
    fn loaded_module_reorder_invalidates_list_without_field_changes() {
        let old = CompilerInputs::new(CompileRequest::new(loaded_program_with_modules(vec![
            loaded_module(ModuleId(0), "main.nia", "fn main() i32 { 0 }"),
            loaded_module(ModuleId(1), "pkg/root.nia", "pub fn value() i32 { 1 }"),
        ])));
        let new = CompilerInputs::new(CompileRequest::new(loaded_program_with_modules(vec![
            loaded_module(ModuleId(1), "pkg/root.nia", "pub fn value() i32 { 1 }"),
            loaded_module(ModuleId(0), "main.nia", "fn main() i32 { 0 }"),
        ])));

        let diff = CompilerInputDiff::between(&old, &new);

        assert!(diff.loaded_modules_changed);
        assert_eq!(diff.changed_modules, Vec::new());
    }

    #[test]
    fn stable_source_identity_with_new_module_id_invalidates_old_key_and_recomputes_new_key() {
        let old = CompilerInputs::new(CompileRequest::new(loaded_program_with_modules(vec![
            loaded_module(
                ModuleId(0),
                "main.nia",
                "pub struct S { value: i32 } fn main() i32 { 0 }",
            ),
        ])));
        let new = CompilerInputs::new(CompileRequest::new(loaded_program_with_modules(vec![
            loaded_module(
                ModuleId(7),
                "main.nia",
                "pub struct S { value: i32 } fn main() i32 { 0 }",
            ),
        ])));
        let diff = CompilerInputDiff::between(&old, &new);

        assert!(diff.loaded_modules_changed);
        assert_eq!(diff.changed_modules.len(), 1);
        assert_eq!(diff.changed_modules[0].ids, vec![ModuleId(0), ModuleId(7)]);

        let database =
            CompilerDatabase::new(CompileRequest::new(loaded_program_with_modules(vec![
                loaded_module(
                    ModuleId(0),
                    "main.nia",
                    "pub struct S { value: i32 } fn main() i32 { 0 }",
                ),
            ])));

        let first = database.check_program();
        assert!(first.diagnostics.is_empty(), "{:?}", first.diagnostics);

        let invalidation = database.update(CompileRequest::new(loaded_program_with_modules(vec![
            loaded_module(
                ModuleId(7),
                "main.nia",
                "pub struct S { value: i32 } fn main() i32 { 0 }",
            ),
        ])));
        let invalidated = invalidation
            .invalidated
            .iter()
            .map(|frame| frame.description.as_str())
            .collect::<Vec<_>>();

        assert!(
            invalidated.contains(&"module_path(ModuleId(0))"),
            "{invalidated:?}"
        );
        assert!(
            invalidated.contains(&"checked_module::CheckedModuleQuery(ModuleId(0))"),
            "{invalidated:?}"
        );
        assert!(
            invalidated.contains(&"loaded_modules::LoadedModulesQuery"),
            "{invalidated:?}"
        );

        let second = database.check_program();
        assert!(second.diagnostics.is_empty(), "{:?}", second.diagnostics);
        assert_eq!(second.modules[0].id, ModuleId(7));
    }

    #[test]
    fn same_module_id_with_new_source_identity_is_replacement() {
        let old = CompilerInputs::new(CompileRequest::new(loaded_program_with_modules(vec![
            loaded_module(ModuleId(0), "main.nia", "fn main() i32 { 0 }"),
        ])));
        let new = CompilerInputs::new(CompileRequest::new(loaded_program_with_modules(vec![
            loaded_module(ModuleId(0), "other.nia", "fn main() i32 { 0 }"),
        ])));

        let diff = CompilerInputDiff::between(&old, &new);

        assert!(diff.loaded_modules_changed);
        assert_eq!(diff.changed_modules.len(), 2);
        assert!(diff.changed_modules.iter().all(|module| {
            module.ids == vec![ModuleId(0)]
                && module.path
                && module.source_identity
                && module.source_version
                && module.item_tree
                && module.full_item_tree
        }));
    }

    #[test]
    fn compiler_database_update_invalidates_changed_module_field_inputs() {
        let database =
            CompilerDatabase::new(CompileRequest::new(loaded_program_with_modules(vec![
                loaded_module(ModuleId(0), "main.nia", "fn main() i32 { 0 }"),
            ])));

        let first = database.check_program();
        assert!(first.diagnostics.is_empty(), "{:?}", first.diagnostics);

        let invalidation = database.update(CompileRequest::new(loaded_program_with_modules(vec![
            loaded_module_with_revision(
                ModuleId(0),
                "main.nia",
                "fn main() i32 { true }",
                SourceRevision(1),
            ),
        ])));
        let invalidated = invalidation
            .invalidated
            .iter()
            .map(|frame| frame.name)
            .collect::<Vec<_>>();

        assert!(
            invalidated.contains(&"module_source_version"),
            "{invalidated:?}"
        );
        assert!(
            !invalidated.contains(&"module_item_tree_input"),
            "{invalidated:?}"
        );
        assert!(
            invalidated.contains(&"full_module_item_tree_input"),
            "{invalidated:?}"
        );
        assert!(invalidated.contains(&"checked_program"), "{invalidated:?}");

        let second = database.check_program();
        assert!(!second.diagnostics.is_empty());
        assert!(
            database
                .query_trace()
                .dependencies
                .iter()
                .any(|dependency| {
                    dependency.from.name == "parse_ok_module_ids"
                        && dependency.to.name == "loaded_modules"
                })
        );
    }

    #[test]
    fn revision_only_update_keeps_declaration_and_type_queries_cached() {
        let source = "pub struct S { value: i32 } fn main() i32 { let value: i32 = 0; value }";
        let database =
            CompilerDatabase::new(CompileRequest::new(loaded_program_with_modules(vec![
                loaded_module_with_revision(ModuleId(0), "main.nia", source, SourceRevision(0)),
            ])));

        let first = database.check_program();
        assert!(first.diagnostics.is_empty(), "{:?}", first.diagnostics);

        let invalidation = database.update(CompileRequest::new(loaded_program_with_modules(vec![
            loaded_module_with_revision(ModuleId(0), "main.nia", source, SourceRevision(1)),
        ])));
        let invalidated = invalidation
            .invalidated
            .iter()
            .map(|frame| frame.name)
            .collect::<Vec<_>>();

        assert!(
            invalidated.contains(&"module_source_version"),
            "{invalidated:?}"
        );
        assert!(
            invalidated.contains(&"full_module_item_tree_input"),
            "{invalidated:?}"
        );
        assert!(invalidated.contains(&"body_check"), "{invalidated:?}");
        assert!(
            !invalidated.contains(&"module_item_tree_input"),
            "{invalidated:?}"
        );
        assert!(
            !invalidated.contains(&"declaration_type_lowering"),
            "{invalidated:?}"
        );
        assert!(!invalidated.contains(&"item_signatures"), "{invalidated:?}");
        assert!(
            !invalidated.iter().any(|name| is_body_signature_query(name)),
            "{invalidated:?}"
        );
        let before_second_check = database.query_trace();

        let second = database.check_program();
        assert!(second.diagnostics.is_empty(), "{:?}", second.diagnostics);
        let after_second_check = database.query_trace();

        assert_eq!(
            query_executions(&before_second_check, "declaration_type_lowering"),
            query_executions(&after_second_check, "declaration_type_lowering"),
        );
        assert_eq!(
            query_executions(&before_second_check, "item_signatures"),
            query_executions(&after_second_check, "item_signatures"),
        );
        assert!(
            query_cache_hits(&after_second_check, "item_signatures")
                > query_cache_hits(&before_second_check, "item_signatures"),
        );
    }

    #[test]
    fn function_body_update_keeps_public_surface_cached() {
        let database =
            CompilerDatabase::new(CompileRequest::new(loaded_program_with_modules(vec![
                loaded_module(
                    ModuleId(0),
                    "main.nia",
                    "pub struct S { value: i32 } fn main() i32 { 0 }",
                ),
            ])));

        let first = database.check_program();
        assert!(first.diagnostics.is_empty(), "{:?}", first.diagnostics);

        let invalidation = database.update(CompileRequest::new(loaded_program_with_modules(vec![
            loaded_module_with_revision(
                ModuleId(0),
                "main.nia",
                "pub struct S { value: i32 } fn main() i32 { 1 }",
                SourceRevision(1),
            ),
        ])));
        let invalidated = invalidation
            .invalidated
            .iter()
            .map(|frame| frame.name)
            .collect::<Vec<_>>();

        assert!(
            invalidated.contains(&"full_module_item_tree_input"),
            "{invalidated:?}"
        );
        assert!(
            !invalidated.contains(&"module_item_tree_input"),
            "{invalidated:?}"
        );
        assert!(invalidated.contains(&"body_check"), "{invalidated:?}");
        assert!(!invalidated.contains(&"loaded_modules"), "{invalidated:?}");
        assert!(!invalidated.contains(&"public_surface"), "{invalidated:?}");

        let second = database.check_program();
        assert!(second.diagnostics.is_empty(), "{:?}", second.diagnostics);
    }

    #[test]
    fn function_body_type_update_keeps_signature_queries_cached() {
        let database =
            CompilerDatabase::new(CompileRequest::new(loaded_program_with_modules(vec![
                loaded_module(
                    ModuleId(0),
                    "main.nia",
                    "pub struct S { value: i32 } fn main() i32 { let value: i32 = 0; value }",
                ),
            ])));

        let first = database.check_program();
        assert!(first.diagnostics.is_empty(), "{:?}", first.diagnostics);

        let invalidation = database.update(CompileRequest::new(loaded_program_with_modules(vec![
            loaded_module_with_revision(
                ModuleId(0),
                "main.nia",
                "pub struct S { value: i32 } fn main() i32 { let value: u8 = 0; value as i32 }",
                SourceRevision(1),
            ),
        ])));
        let invalidated = invalidation
            .invalidated
            .iter()
            .map(|frame| frame.name)
            .collect::<Vec<_>>();

        assert!(
            invalidated.contains(&"full_module_item_tree_input"),
            "{invalidated:?}"
        );
        assert!(invalidated.contains(&"type_lowering"), "{invalidated:?}");
        assert!(invalidated.contains(&"body_check"), "{invalidated:?}");
        assert!(
            !invalidated.contains(&"declaration_type_lowering"),
            "{invalidated:?}"
        );
        assert!(!invalidated.contains(&"item_signatures"), "{invalidated:?}");
        assert!(
            !invalidated.iter().any(|name| is_body_signature_query(name)),
            "{invalidated:?}"
        );
        assert!(
            !invalidated.contains(&"extension_methods"),
            "{invalidated:?}"
        );

        let second = database.check_program();
        assert!(second.diagnostics.is_empty(), "{:?}", second.diagnostics);
    }

    #[test]
    fn body_local_type_update_reuses_program_body_signature_indexes() {
        let database =
            CompilerDatabase::new(CompileRequest::new(loaded_program_with_modules(vec![
                loaded_module(
                    ModuleId(0),
                    "main.nia",
                    "pub struct S { value: i32 } fn main() i32 { let value: i32 = 0; value }",
                ),
            ])));

        let first = database.check_program();
        assert!(first.diagnostics.is_empty(), "{:?}", first.diagnostics);

        let invalidation = database.update(CompileRequest::new(loaded_program_with_modules(vec![
            loaded_module_with_revision(
                ModuleId(0),
                "main.nia",
                "pub struct S { value: i32 } fn main() i32 { let value: u8 = 0; value as i32 }",
                SourceRevision(1),
            ),
        ])));
        let invalidated = invalidation
            .invalidated
            .iter()
            .map(|frame| frame.name)
            .collect::<Vec<_>>();

        assert!(invalidated.contains(&"type_lowering"), "{invalidated:?}");
        assert!(
            !invalidated.iter().any(|name| is_body_signature_query(name)),
            "{invalidated:?}"
        );
        let before_second_check = database.query_trace();

        let second = database.check_program();
        assert!(second.diagnostics.is_empty(), "{:?}", second.diagnostics);
        let after_second_check = database.query_trace();

        assert_query_executions_unchanged(
            &before_second_check,
            &after_second_check,
            "program_body_function_signatures",
        );
        assert_query_executions_unchanged(
            &before_second_check,
            &after_second_check,
            "program_body_value_signatures",
        );
        assert_query_executions_unchanged(
            &before_second_check,
            &after_second_check,
            "program_body_type_signatures",
        );
        assert_query_executions_unchanged(
            &before_second_check,
            &after_second_check,
            "program_body_trait_signatures",
        );
    }

    #[test]
    fn function_signature_update_keeps_definition_queries_cached() {
        let database =
            CompilerDatabase::new(CompileRequest::new(loaded_program_with_modules(vec![
                loaded_module(
                    ModuleId(0),
                    "main.nia",
                    "pub struct S { value: i32 } fn helper() i32 { 1 } fn main() i32 { helper() }",
                ),
            ])));

        let first = database.codegen_program();
        assert!(first.diagnostics.is_empty(), "{:?}", first.diagnostics);

        let invalidation = database.update(CompileRequest::new(loaded_program_with_modules(vec![
            loaded_module_with_revision(
                ModuleId(0),
                "main.nia",
                "pub struct S { value: i32 } fn helper() u8 { 1 } fn main() i32 { helper() as i32 }",
                SourceRevision(1),
            ),
        ])));
        let invalidated = invalidation
            .invalidated
            .iter()
            .map(|frame| frame.name)
            .collect::<Vec<_>>();

        assert!(
            invalidated.contains(&"declaration_module_item_tree_input"),
            "{invalidated:?}"
        );
        assert!(
            invalidated.contains(&"declaration_type_lowering"),
            "{invalidated:?}"
        );
        assert!(
            invalidated.contains(&"program_body_function_signatures"),
            "{invalidated:?}"
        );
        assert!(
            !invalidated.contains(&"program_body_value_signatures"),
            "{invalidated:?}"
        );
        assert!(
            !invalidated.contains(&"program_body_type_signatures"),
            "{invalidated:?}"
        );
        assert!(
            !invalidated.contains(&"program_body_trait_signatures"),
            "{invalidated:?}"
        );
        assert!(
            !invalidated.contains(&"extension_methods"),
            "{invalidated:?}"
        );
        assert!(
            invalidated.contains(&"program_backend_signatures"),
            "{invalidated:?}"
        );
        assert!(
            !invalidated.contains(&"module_item_tree_input"),
            "{invalidated:?}"
        );
        assert!(!invalidated.contains(&"module_defs"), "{invalidated:?}");
        assert!(!invalidated.contains(&"public_surface"), "{invalidated:?}");
    }

    #[test]
    fn function_body_type_update_keeps_signature_program_type_context_cached() {
        let database =
            CompilerDatabase::new(CompileRequest::new(loaded_program_with_modules(vec![
                loaded_module(
                    ModuleId(0),
                    "main.nia",
                    "fn main() i32 { let value: i32 = 0; value }",
                ),
                loaded_module(ModuleId(1), "helper.nia", "fn helper() i32 { 1 }"),
            ])));

        let first = database.check_program();
        assert!(first.diagnostics.is_empty(), "{:?}", first.diagnostics);

        let invalidation = database.update(CompileRequest::new(loaded_program_with_modules(vec![
            loaded_module_with_revision(
                ModuleId(0),
                "main.nia",
                "fn main() i32 { let value: u8 = 0; value as i32 }",
                SourceRevision(1),
            ),
            loaded_module(ModuleId(1), "helper.nia", "fn helper() i32 { 1 }"),
        ])));
        let invalidated = invalidation
            .invalidated
            .iter()
            .map(|frame| frame.name)
            .collect::<Vec<_>>();

        assert!(invalidated.contains(&"type_lowering"), "{invalidated:?}");
        assert!(
            !invalidated.contains(&"declaration_type_lowering"),
            "{invalidated:?}"
        );
        let before_second_check = database.query_trace();

        let second = database.check_program();
        assert!(second.diagnostics.is_empty(), "{:?}", second.diagnostics);
        let after_second_check = database.query_trace();

        assert_eq!(
            query_executions(&before_second_check, "signature_type_normalization"),
            query_executions(&after_second_check, "signature_type_normalization"),
        );
    }

    #[test]
    fn source_identity_update_invalidates_module_dependent_queries() {
        let database =
            CompilerDatabase::new(CompileRequest::new(loaded_program_with_modules(vec![
                loaded_module(
                    ModuleId(0),
                    "main.nia",
                    "pub struct S { value: i32 } fn main() i32 { 0 }",
                ),
            ])));

        let first = database.check_program();
        assert!(first.diagnostics.is_empty(), "{:?}", first.diagnostics);

        let invalidation = database.update(CompileRequest::new(loaded_program_with_modules(vec![
            loaded_module(
                ModuleId(0),
                "renamed.nia",
                "pub struct S { value: i32 } fn main() i32 { 0 }",
            ),
        ])));
        let invalidated = invalidation
            .invalidated
            .iter()
            .map(|frame| frame.name)
            .collect::<Vec<_>>();

        assert!(invalidated.contains(&"loaded_modules"), "{invalidated:?}");
        assert!(invalidated.contains(&"checked_module"), "{invalidated:?}");
        assert!(invalidated.contains(&"public_surface"), "{invalidated:?}");
        assert!(invalidated.contains(&"module_path"), "{invalidated:?}");

        let second = database.check_program();
        assert!(second.diagnostics.is_empty(), "{:?}", second.diagnostics);
        assert_eq!(second.modules[0].path.as_str(), "renamed.nia");
    }

    #[test]
    fn source_identity_change_invalidates_loaded_module_list() {
        let database =
            CompilerDatabase::new(CompileRequest::new(loaded_program_with_modules(vec![
                loaded_module(ModuleId(0), "main.nia", "fn main() i32 { 0 }"),
            ])));
        let _ = database.check_program();

        let invalidation = database.update(CompileRequest::new(loaded_program_with_modules(vec![
            loaded_module(ModuleId(0), "other.nia", "fn main() i32 { 0 }"),
        ])));
        let invalidated = invalidation
            .invalidated
            .iter()
            .map(|frame| frame.name)
            .collect::<Vec<_>>();

        assert!(invalidated.contains(&"loaded_modules"), "{invalidated:?}");
        assert!(invalidated.contains(&"module_path"), "{invalidated:?}");
    }

    #[test]
    fn compiler_query_providers_can_override_query_execution() {
        fn no_parse_ok_modules(_: &QueryDb<CompilerContext>) -> Vec<ModuleId> {
            Vec::new()
        }

        let providers = CompilerQueryProviders {
            parse_ok_module_ids: no_parse_ok_modules,
            ..CompilerQueryProviders::default()
        };
        let checked = compiler_database_with_providers(
            CompileRequest::new(loaded_program_with_modules(vec![loaded_module(
                ModuleId(0),
                "main.nia",
                "fn main() i32 { 0 }",
            )])),
            providers,
        )
        .codegen_program();

        assert!(checked.modules.is_empty());
    }

    #[test]
    fn missing_loaded_module_id_becomes_query_diagnostic() {
        fn unknown_parse_ok_module(_: &QueryDb<CompilerContext>) -> Vec<ModuleId> {
            vec![ModuleId(99)]
        }

        let providers = CompilerQueryProviders {
            parse_ok_module_ids: unknown_parse_ok_module,
            ..CompilerQueryProviders::default()
        };
        let policy = NiaOptimizationLevel::Oz.policy();
        let checked = compiler_database_with_providers(
            CompileRequest::new(loaded_program_with_modules(vec![loaded_module(
                ModuleId(0),
                "main.nia",
                "fn main() i32 { 0 }",
            )]))
            .with_optimization(NiaOptimizationLevel::Oz),
            providers,
        )
        .codegen_program();

        assert!(checked.modules.is_empty());
        assert_eq!(checked.optimization, policy);
        assert_eq!(checked.backend_lowering.optimization, policy);
        assert_eq!(checked.diagnostics.len(), 1);
        assert!(
            checked.diagnostics[0]
                .diagnostic
                .summary
                .contains("missing loaded module ModuleId(99)")
        );
    }

    #[test]
    fn program_body_signature_queries_use_precise_module_signature_queries() {
        let loaded = loaded_program_with_modules(vec![loaded_module(
            ModuleId(0),
            "main.nia",
            "struct S { value: i32 }",
        )]);
        let db = query_db(loaded);

        let _ = db.query(ProgramBodyFunctionSignaturesQuery);
        let _ = db.query(ProgramBodyValueSignaturesQuery);
        let _ = db.query(ProgramBodyTypeSignaturesQuery);
        let _ = db.query(ProgramBodyTraitSignaturesQuery);
        let trace = db.query_trace();

        assert!(trace.dependencies.iter().any(|dependency| {
            dependency.from.name == "program_body_function_signatures"
                && dependency.to.name == "signature_item_signatures"
        }));
        assert!(trace.dependencies.iter().any(|dependency| {
            dependency.from.name == "program_body_value_signatures"
                && dependency.to.name == "signature_item_signatures"
        }));
        assert!(trace.dependencies.iter().any(|dependency| {
            dependency.from.name == "program_body_type_signatures"
                && dependency.to.name == "signature_item_signatures"
        }));
        assert!(trace.dependencies.iter().any(|dependency| {
            dependency.from.name == "program_body_trait_signatures"
                && dependency.to.name == "program_trait_solving_signatures"
        }));
        assert!(!trace.dependencies.iter().any(|dependency| {
            is_body_signature_query(dependency.from.name)
                && matches!(
                    dependency.to.name,
                    "type_lowering" | "declaration_type_lowering" | "item_signatures"
                )
        }));
        assert!(!trace.dependencies.iter().any(|dependency| {
            dependency.from.name == "program_body_type_signatures"
                && dependency.to.name == "program_trait_solving_signatures"
        }));
    }

    #[test]
    fn program_codegen_signature_queries_use_precise_module_signature_queries() {
        let loaded = loaded_program_with_modules(vec![loaded_module(
            ModuleId(0),
            "main.nia",
            "struct S { value: i32 } trait T { fn get(self) i32; } fn helper() i32 { 1 }",
        )]);
        let db = query_db(loaded);

        let _ = db.query(ProgramVisibleTypeSignaturesQuery);
        let _ = db.query(ProgramBackendSignaturesQuery);
        let trace = db.query_trace();

        for query in [
            "program_visible_type_signatures",
            "program_backend_signatures",
        ] {
            assert!(!trace.dependencies.iter().any(|dependency| {
                dependency.from.name == query
                    && matches!(
                        dependency.to.name,
                        "item_signatures" | "declaration_type_lowering"
                    )
            }));
        }
        assert!(trace.dependencies.iter().any(|dependency| {
            dependency.from.name == "program_backend_signatures"
                && dependency.to.name == "signature_item_signatures"
        }));
        assert!(!trace.dependencies.iter().any(|dependency| {
            matches!(
                dependency.from.name,
                "program_executable_reachability_signatures" | "program_executable_signatures"
            )
        }));
    }

    #[test]
    fn layout_uses_full_type_module_signatures_and_array_lengths_without_body_products() {
        let loaded = loaded_program_with_modules(vec![loaded_module(
            ModuleId(0),
            "main.nia",
            "struct S { value: i32 } fn helper() i32 { 1 }",
        )]);
        let db = query_db(loaded);

        let layouts = db.query(LayoutsQuery(ModuleId(0)));
        let trace = db.query_trace();

        assert!(layouts.diagnostics.is_empty(), "{:?}", layouts.diagnostics);
        assert!(trace.dependencies.iter().any(|dependency| {
            dependency.from.name == "layouts" && dependency.to.name == "layout_type_normalization"
        }));
        assert!(trace.dependencies.iter().any(|dependency| {
            dependency.from.name == "layouts" && dependency.to.name == "comptime_array_lengths"
        }));
        assert!(trace.dependencies.iter().any(|dependency| {
            dependency.from.name == "layouts" && dependency.to.name == "item_signatures"
        }));
        assert!(!trace.dependencies.iter().any(|dependency| {
            dependency.from.name == "layouts"
                && matches!(
                    dependency.to.name,
                    "type_normalization" | "comptime" | "body_check"
                )
        }));
    }

    #[test]
    fn abi_check_uses_abi_signature_index_not_body_signatures() {
        let loaded = loaded_program_with_modules(vec![loaded_module(
            ModuleId(0),
            "main.nia",
            "extern struct S { value: i32 } extern fn take(value: S) void;",
        )]);
        let db = query_db(loaded);

        let _ = db.query(AbiCheckQuery(ModuleId(0)));
        let trace = db.query_trace();

        assert!(trace.dependencies.iter().any(|dependency| {
            dependency.from.name == "abi_check" && dependency.to.name == "program_abi_signatures"
        }));
        assert!(trace.dependencies.iter().any(|dependency| {
            dependency.from.name == "program_abi_signatures"
                && dependency.to.name == "signature_item_signatures"
        }));
        assert!(trace.dependencies.iter().any(|dependency| {
            dependency.from.name == "abi_check" && dependency.to.name == "signature_item_signatures"
        }));
        assert!(!trace.dependencies.iter().any(|dependency| {
            matches!(dependency.from.name, "abi_check" | "program_abi_signatures")
                && matches!(dependency.to.name, "item_signatures" | "type_normalization")
        }));
        assert!(!depends_on_body_signature_query(&trace, "abi_check"));
    }

    #[test]
    fn public_surface_query_uses_module_defs_queries() {
        let loaded = loaded_program_with_modules(vec![loaded_module(
            ModuleId(0),
            "main.nia",
            "pub struct S { value: i32 }",
        )]);
        let db = query_db(loaded);

        let _ = db.query(PublicSurfaceQuery);
        let trace = db.query_trace();

        assert!(trace.dependencies.iter().any(|dependency| {
            dependency.from.name == "defs_by_module" && dependency.to.name == "module_defs"
        }));
        assert!(trace.dependencies.iter().any(|dependency| {
            dependency.from.name == "public_surface" && dependency.to.name == "defs_by_module"
        }));
    }

    #[test]
    fn module_defs_query_uses_active_item_tree_query() {
        let loaded = loaded_program_with_modules(vec![loaded_module(
            ModuleId(0),
            "main.nia",
            "pub struct S { value: i32 }",
        )]);
        let db = query_db(loaded);

        let _ = db.query(ModuleDefsQuery(ModuleId(0)));
        let trace = db.query_trace();

        assert!(trace.dependencies.iter().any(|dependency| {
            dependency.from.name == "module_defs" && dependency.to.name == "active_module_item_tree"
        }));
        assert!(trace.dependencies.iter().any(|dependency| {
            dependency.from.name == "active_module_item_tree"
                && dependency.to.name == "module_item_tree"
        }));
        assert!(trace.dependencies.iter().any(|dependency| {
            dependency.from.name == "module_item_tree"
                && dependency.to.name == "module_item_tree_input"
        }));
    }

    #[test]
    fn extension_queries_use_module_semantic_queries() {
        let loaded = loaded_program_with_modules(vec![loaded_module(
            ModuleId(0),
            "main.nia",
            "struct S { value: i32 } extend S { pub fn make(value: i32) S { { value: value } } }",
        )]);
        let db = query_db(loaded);

        let _ = db.query(ExtensionMethodsQuery);
        let _ = db.query(ExtensionMethodIndexQuery);
        let trace = db.query_trace();

        assert!(trace.dependencies.iter().any(|dependency| {
            dependency.from.name == "extension_methods"
                && dependency.to.name == "extension_method_set"
        }));
        assert!(trace.dependencies.iter().any(|dependency| {
            dependency.from.name == "extension_methods"
                && dependency.to.name == "extension_associated_values"
        }));
        for query in ["extension_method_set"] {
            assert!(trace.dependencies.iter().any(|dependency| {
                dependency.from.name == query && dependency.to.name == "module_defs"
            }));
            assert!(trace.dependencies.iter().any(|dependency| {
                dependency.from.name == query && dependency.to.name == "signature_item_signatures"
            }));
            assert!(trace.dependencies.iter().any(|dependency| {
                dependency.from.name == query && dependency.to.name == "signature_type_lowering"
            }));
            assert!(trace.dependencies.iter().any(|dependency| {
                dependency.from.name == query
                    && dependency.to.name == "signature_type_normalization"
            }));
            assert!(!trace.dependencies.iter().any(|dependency| {
                dependency.from.name == query
                    && matches!(
                        dependency.to.name,
                        "item_signatures" | "declaration_type_lowering"
                    )
            }));
            assert!(!trace.dependencies.iter().any(|dependency| {
                dependency.from.name == query && dependency.to.name == "active_module_item_tree"
            }));
            assert!(!trace.dependencies.iter().any(|dependency| {
                dependency.from.name == query && dependency.to.name == "full_module_defs"
            }));
            assert!(!trace.dependencies.iter().any(|dependency| {
                dependency.from.name == query && dependency.to.name == "program_type_normalizations"
            }));
        }
        assert!(trace.dependencies.iter().any(|dependency| {
            dependency.from.name == "extension_method_index"
                && dependency.to.name == "signature_item_signatures"
        }));
        assert!(trace.dependencies.iter().any(|dependency| {
            dependency.from.name == "extension_method_index"
                && dependency.to.name == "signature_type_normalization"
        }));
        assert!(!trace.dependencies.iter().any(|dependency| {
            dependency.from.name == "extension_method_index"
                && matches!(
                    dependency.to.name,
                    "item_signatures" | "declaration_type_lowering"
                )
        }));
        assert!(trace.dependencies.iter().any(|dependency| {
            dependency.from.name == "extension_associated_values"
                && dependency.to.name == "signature_item_signatures"
        }));
        assert!(trace.dependencies.iter().any(|dependency| {
            dependency.from.name == "extension_associated_values"
                && dependency.to.name == "signature_type_normalization"
        }));
        assert!(!trace.dependencies.iter().any(|dependency| {
            dependency.from.name == "extension_associated_values"
                && matches!(
                    dependency.to.name,
                    "item_signatures" | "declaration_type_lowering"
                )
        }));
        assert!(!trace.dependencies.iter().any(|dependency| {
            dependency.from.name == "extension_method_index"
                && matches!(
                    dependency.to.name,
                    "extension_method_set" | "program_trait_solving_signatures"
                )
        }));
        assert!(!trace.dependencies.iter().any(|dependency| {
            dependency.from.name == "declaration_type_lowering"
                && dependency.to.name == "program_defs_by_id"
        }));
        assert!(!trace.dependencies.iter().any(|dependency| {
            dependency.from.name == "declaration_type_lowering"
                && dependency.to.name == "program_full_defs_by_id"
        }));
    }

    #[test]
    fn body_sensitive_resolution_uses_full_active_item_tree_query() {
        let loaded = loaded_program_with_modules(vec![loaded_module(
            ModuleId(0),
            "main.nia",
            "fn main() i32 { let value = 1; value }",
        )]);
        let db = query_db(loaded);

        let _ = db.query(ValueResolutionQuery(ModuleId(0)));
        let trace = db.query_trace();

        assert!(trace.dependencies.iter().any(|dependency| {
            dependency.from.name == "value_resolution"
                && dependency.to.name == "full_active_module_item_tree"
        }));
        assert!(trace.dependencies.iter().any(|dependency| {
            dependency.from.name == "full_active_module_item_tree"
                && dependency.to.name == "full_module_item_tree"
        }));
        assert!(trace.dependencies.iter().any(|dependency| {
            dependency.from.name == "full_module_item_tree"
                && dependency.to.name == "full_module_item_tree_input"
        }));
    }

    #[test]
    fn flow_check_uses_full_active_item_tree_query() {
        let loaded = loaded_program_with_modules(vec![loaded_module(
            ModuleId(0),
            "main.nia",
            "fn main() i32 { return 1; }",
        )]);
        let db = query_db(loaded);

        let _ = db.query(FlowCheckQuery(ModuleId(0)));
        let trace = db.query_trace();

        assert!(trace.dependencies.iter().any(|dependency| {
            dependency.from.name == "flow_check"
                && dependency.to.name == "full_active_module_item_tree"
        }));
        assert!(trace.dependencies.iter().any(|dependency| {
            dependency.from.name == "flow_check"
                && dependency.to.name == "signature_item_signatures"
        }));
        assert!(!trace.dependencies.iter().any(|dependency| {
            dependency.from.name == "flow_check"
                && matches!(dependency.to.name, "item_signatures" | "type_lowering")
        }));
    }

    #[test]
    fn static_check_uses_full_active_item_tree_query() {
        let loaded = loaded_program_with_modules(vec![loaded_module(
            ModuleId(0),
            "main.nia",
            "static mut global: i32 = 1; fn main() i32 { global }",
        )]);
        let db = query_db(loaded);

        let _ = db.query(StaticCheckQuery(ModuleId(0)));
        let trace = db.query_trace();

        assert!(trace.dependencies.iter().any(|dependency| {
            dependency.from.name == "static_check"
                && dependency.to.name == "full_active_module_item_tree"
        }));
        assert!(trace.dependencies.iter().any(|dependency| {
            dependency.from.name == "static_check"
                && dependency.to.name == "signature_item_signatures"
        }));
        assert!(trace.dependencies.iter().any(|dependency| {
            dependency.from.name == "static_check" && dependency.to.name == "comptime_values"
        }));
        assert!(!trace.dependencies.iter().any(|dependency| {
            dependency.from.name == "static_check"
                && matches!(dependency.to.name, "item_signatures" | "comptime")
        }));
        assert!(
            !trace
                .dependencies
                .iter()
                .any(|dependency| dependency.from.name == "static_check"
                    && dependency.to.name == "program_comptime")
        );
        assert!(
            !trace
                .dependencies
                .iter()
                .any(|dependency| dependency.from.name == "static_check"
                    && dependency.to.name == "program_full_defs_by_id")
        );
    }

    #[test]
    fn body_check_collects_local_signature_subsets_with_full_type_lowering() {
        let loaded = loaded_program_with_modules(vec![loaded_module(
            ModuleId(0),
            "main.nia",
            "struct S { value: i32 } static mut global: i32 = 1; fn main() i32 { global }",
        )]);
        let db = query_db(loaded);

        let _ = db.query(BodyCheckQuery(ModuleId(0)));
        let trace = db.query_trace();

        assert!(trace.dependencies.iter().any(|dependency| {
            dependency.from.name == "body_check" && dependency.to.name == "type_lowering"
        }));
        assert!(trace.dependencies.iter().any(|dependency| {
            dependency.from.name == "body_check" && dependency.to.name == "signature_item_tree"
        }));
        assert!(trace.dependencies.iter().any(|dependency| {
            dependency.from.name == "body_check"
                && dependency.to.name == "signature_item_tree"
                && dependency.to.description.contains("ExtensionFunctions")
        }));
        assert!(trace.dependencies.iter().any(|dependency| {
            dependency.from.name == "body_check" && dependency.to.name == "comptime_values"
        }));
        assert!(trace.dependencies.iter().any(|dependency| {
            dependency.from.name == "body_check" && dependency.to.name == "comptime_array_lengths"
        }));
        assert!(!trace.dependencies.iter().any(|dependency| {
            dependency.from.name == "body_check"
                && matches!(dependency.to.name, "item_signatures" | "comptime")
        }));
        assert!(!trace.dependencies.iter().any(|dependency| {
            dependency.from.name == "body_check"
                && dependency.to.name == "signature_item_signatures"
                && dependency.to.description.contains("ExtensionFunctions")
        }));
    }

    #[test]
    fn body_check_imports_full_lowering_types_before_working_interner_lookup() {
        let loaded = loaded_program_with_modules(vec![loaded_module(
            ModuleId(0),
            "main.nia",
            r#"
struct Item {
    state: i32,
}

fn set(items: &mut [Item], index: usize, state: i32) void {
    items[index].state = state;
}

fn main() i32 {
    let mut items: [2]Item = [
        { state: 1 },
        { state: 2 },
    ];
    set(&mut items[..], 1usize, 9);
    items[1].state
}
"#,
        )]);
        let db = query_db(loaded);

        let checked = db.query(BodyCheckQuery(ModuleId(0)));

        assert!(checked.diagnostics.is_empty(), "{:?}", checked.diagnostics);
    }

    #[test]
    fn visible_extensions_use_signature_type_normalization_query() {
        let loaded = loaded_program_with_modules(vec![loaded_module(
            ModuleId(0),
            "main.nia",
            "struct S { value: i32 } extend S { pub fn make(value: i32) S { { value: value } } }",
        )]);
        let db = query_db(loaded);

        let _ = db.query(VisibleExtensionsQuery(ModuleId(0)));
        let trace = db.query_trace();

        assert!(trace.dependencies.iter().any(|dependency| {
            dependency.from.name == "visible_extensions"
                && dependency.to.name == "signature_type_normalization"
        }));
        assert!(!trace.dependencies.iter().any(|dependency| {
            dependency.from.name == "visible_extensions"
                && dependency.to.name == "program_defs_by_id"
        }));
        assert!(trace.dependencies.iter().any(|dependency| {
            dependency.from.name == "visible_extensions" && dependency.to.name == "module_defs"
        }));
        assert!(!trace.dependencies.iter().any(|dependency| {
            dependency.from.name == "visible_extensions"
                && dependency.to.name == "program_full_defs_by_id"
        }));
        assert!(!trace.dependencies.iter().any(|dependency| {
            dependency.from.name == "visible_extensions"
                && dependency.to.name == "program_type_normalizations"
        }));
        assert!(trace.dependencies.iter().any(|dependency| {
            dependency.from.name == "visible_extensions"
                && dependency.to.name == "program_visible_type_signatures"
        }));
        assert!(!depends_on_body_signature_query(
            &trace,
            "visible_extensions"
        ));
    }

    #[test]
    fn comptime_uses_precise_program_context_queries() {
        let loaded = loaded_program_with_modules(vec![loaded_module(
            ModuleId(0),
            "main.nia",
            "comptime VALUE = 1; fn main() i32 { VALUE }",
        )]);
        let db = query_db(loaded);

        let _ = db.query(ComptimeQuery(ModuleId(0)));
        let trace = db.query_trace();

        assert!(trace.dependencies.iter().any(|dependency| {
            dependency.from.name == "comptime" && dependency.to.name == "comptime_module"
        }));
        assert!(trace.dependencies.iter().any(|dependency| {
            dependency.from.name == "comptime" && dependency.to.name == "comptime_array_lengths"
        }));
        assert!(trace.dependencies.iter().any(|dependency| {
            dependency.from.name == "comptime" && dependency.to.name == "comptime_enum_values"
        }));
        assert!(trace.dependencies.iter().any(|dependency| {
            dependency.from.name == "comptime_array_lengths"
                && dependency.to.name == "comptime_module"
        }));
        assert!(trace.dependencies.iter().any(|dependency| {
            dependency.from.name == "comptime_enum_values"
                && dependency.to.name == "comptime_array_lengths"
        }));
        assert!(!trace.dependencies.iter().any(|dependency| {
            dependency.from.name == "comptime" && dependency.to.name == "program_full_defs_by_id"
        }));
        assert!(
            !trace
                .dependencies
                .iter()
                .any(|dependency| dependency.to.name == "program_comptime_modules")
        );
        assert!(
            !trace
                .dependencies
                .iter()
                .any(|dependency| dependency.to.name == "program_item_signatures")
        );
        assert!(trace.dependencies.iter().any(|dependency| {
            dependency.from.name == "comptime"
                && dependency.to.name == "program_trait_solving_signatures"
        }));
        assert!(!depends_on_body_signature_query(&trace, "comptime"));
        assert!(trace.dependencies.iter().any(|dependency| {
            dependency.from.name == "comptime" && dependency.to.name == "full_module_defs"
        }));
    }

    #[test]
    fn monomorphization_uses_trait_solving_signature_index() {
        let loaded = loaded_program_with_modules(vec![loaded_module(
            ModuleId(0),
            "main.nia",
            "pub fn main() i32 { 1 }",
        )]);
        let db = query_db(loaded);

        let _ = db.query(MonomorphizationQuery);
        let trace = db.query_trace();

        assert!(trace.dependencies.iter().any(|dependency| {
            dependency.from.name == "monomorphization"
                && dependency.to.name == "program_trait_solving_signatures"
        }));
        assert!(!depends_on_body_signature_query(&trace, "monomorphization"));
    }

    #[test]
    fn executable_reachability_uses_lazy_signature_resolvers() {
        let mut loaded = loaded_program_with_modules(vec![loaded_module(
            ModuleId(0),
            "main.nia",
            "pub fn main() i32 { 1 }",
        )]);
        loaded.runtime = RuntimeModel::FreestandingExecutable;
        let db = query_db(loaded);

        let _ = db.query(ExecutableCheckedModulesQuery);
        let trace = db.query_trace();

        assert!(!trace.dependencies.iter().any(|dependency| {
            dependency.from.name == "executable_checked_modules"
                && dependency.to.name == "program_executable_reachability_signatures"
        }));
        assert!(trace.dependencies.iter().any(|dependency| {
            dependency.from.name == "executable_checked_modules"
                && dependency.to.name == "signature_item_signatures"
        }));
        assert!(!depends_on_body_signature_query(
            &trace,
            "executable_checked_modules"
        ));
    }

    #[test]
    fn executable_checked_program_uses_lazy_extension_method_index() {
        let mut loaded = loaded_program_with_modules(vec![loaded_module(
            ModuleId(0),
            "main.nia",
            "struct S { value: i32 } extend S { pub fn make(value: i32) S { { value: value } } } pub fn main() i32 { 1 }",
        )]);
        loaded.runtime = RuntimeModel::FreestandingExecutable;
        let db = query_db(loaded);

        let checked = db.query(CodegenProgramQuery);
        let trace = db.query_trace();

        assert!(checked.diagnostics.is_empty(), "{:?}", checked.diagnostics);
        assert!(trace.dependencies.iter().any(|dependency| {
            dependency.from.name == "executable_checked_modules"
                && dependency.to.name == "extension_method_index"
        }));
        assert!(!trace.dependencies.iter().any(|dependency| {
            dependency.from.name == "checked_program"
                && dependency.to.name == "extension_method_set"
        }));
    }

    #[test]
    fn executable_visible_extensions_follow_facade_provider_chains() {
        let main = loaded_module(
            ModuleId(0),
            "main.nia",
            r#"
module facade;
using entry::facade;

fn main() i32 {
    let init = facade::Init::init();
    let args = init.args();
    let mut iter = args.iter();
    if ?value = iter.next() {
        value
    } or null {
        0
    }
}
"#,
        );
        let facade = loaded_module(
            ModuleId(1),
            "facade.nia",
            r#"
module args_impl;
module init_impl;
module types;

pub using self::types::{Args, ArgsIter, Init};
"#,
        );
        let init_impl = loaded_module(
            ModuleId(2),
            "facade/init_impl.nia",
            r#"
using entry::facade::types::{Args, Init};

extend Init {
    pub fn init() Init {
        {}
    }

    pub fn args(&self) Args {
        Args {}
    }
}
"#,
        );
        let args_impl = loaded_module(
            ModuleId(3),
            "facade/args_impl.nia",
            r#"
using entry::facade::types::{Args, ArgsIter};

extend Args {
    pub fn iter(&self) ArgsIter {
        ArgsIter {}
    }
}

extend ArgsIter {
    pub fn next(&mut self) ?i32 {
        ?42
    }
}
"#,
        );
        let types = loaded_module(
            ModuleId(4),
            "facade/types.nia",
            r#"
pub struct Init {}
pub struct Args {}
pub struct ArgsIter {}
"#,
        );
        let mut graph = ModuleGraph::new(main.path.clone());
        graph
            .intern_declared_child(
                main.id,
                "facade",
                nia_ids::Visibility::Public,
                Span::default(),
            )
            .expect("intern facade module");
        graph
            .intern_declared_child(
                facade.id,
                "args_impl",
                nia_ids::Visibility::Private,
                Span::default(),
            )
            .expect("intern args_impl module");
        graph
            .intern_declared_child(
                facade.id,
                "init_impl",
                nia_ids::Visibility::Private,
                Span::default(),
            )
            .expect("intern init_impl module");
        graph
            .intern_declared_child(
                facade.id,
                "types",
                nia_ids::Visibility::Public,
                Span::default(),
            )
            .expect("intern types module");
        let loaded = LoadedProgram {
            graph,
            target: TargetConfig::host(),
            runtime: RuntimeModel::FreestandingExecutable,
            modules: vec![main, facade, init_impl, args_impl, types],
            diagnostics: Vec::new(),
        };
        let db = query_db(loaded);

        let checked = db.query(CodegenProgramQuery);

        assert!(checked.diagnostics.is_empty(), "{:?}", checked.diagnostics);
    }

    #[test]
    fn executable_reachability_keeps_matched_trait_impl_method_bodies() {
        let main = loaded_module(
            ModuleId(0),
            "main.nia",
            r#"
module parse;
using entry::parse;

pub fn main() i32 {
    parse::parse[i32, parse::Input](parse::Input {})
}
"#,
        );
        let parse = loaded_module(
            ModuleId(1),
            "parse.nia",
            r#"
pub struct Input {}

pub trait ParseFrom[Input] {
    fn parse_from(input: Input) Self;
}

pub fn parse[T, Input](input: Input) T
where T: ParseFrom[Input]
{
    [T]::parse_from(input)
}

extend i32 : ParseFrom[Input] {
    fn parse_from(input: Input) i32 {
        _ = input;
        42
    }
}
"#,
        );
        let mut graph = ModuleGraph::new(main.path.clone());
        graph
            .intern_declared_child(
                main.id,
                "parse",
                nia_ids::Visibility::Public,
                Span::default(),
            )
            .expect("intern parse module");
        let loaded = LoadedProgram {
            graph,
            target: TargetConfig::host(),
            runtime: RuntimeModel::FreestandingExecutable,
            modules: vec![main, parse],
            diagnostics: Vec::new(),
        };
        let db = query_db(loaded);

        let checked = db.query(ExecutableCheckedModulesQuery);
        let parse_module = checked
            .iter()
            .find(|module| module.id == ModuleId(1))
            .expect("parse module should be executable-reachable");
        let parse_from = parse_module
            .defs
            .defs
            .iter()
            .find_map(|(def_id, def)| {
                (def.name == "parse_from" && def.kind == nia_defs::DefKind::Method).then_some(
                    GlobalDefId {
                        module_id: ModuleId(1),
                        def_id,
                    },
                )
            })
            .expect("impl parse_from method should be defined");

        assert!(
            parse_module
                .body_ir
                .function_bodies
                .contains_key(&parse_from),
            "matched trait impl method body should be retained for executable codegen"
        );
    }

    #[test]
    fn comptime_module_uses_full_active_item_tree_query() {
        let loaded = loaded_program_with_modules(vec![loaded_module(
            ModuleId(0),
            "main.nia",
            "comptime fn value() usize { 1 } comptime VALUE = value();",
        )]);
        let db = query_db(loaded);

        let _ = db.query(ComptimeModuleQuery(ModuleId(0)));
        let trace = db.query_trace();

        assert!(trace.dependencies.iter().any(|dependency| {
            dependency.from.name == "comptime_module"
                && dependency.to.name == "full_active_module_item_tree"
        }));
    }

    #[test]
    fn semantic_use_table_query_combines_value_local_and_type_resolution() {
        let source = "static VALUE: i32 = 1; fn main() i32 { let mut local: i32 = VALUE; local }";
        let loaded =
            loaded_program_with_modules(vec![loaded_module(ModuleId(0), "main.nia", source)]);
        let db = query_db(loaded);

        let table = db.query(SemanticUseTableQuery(ModuleId(0)));
        let trace = db.query_trace();

        assert!(trace.dependencies.iter().any(|dependency| {
            dependency.from.name == "semantic_use_table" && dependency.to.name == "value_resolution"
        }));
        assert!(trace.dependencies.iter().any(|dependency| {
            dependency.from.name == "semantic_use_table" && dependency.to.name == "local_resolution"
        }));
        assert!(trace.dependencies.iter().any(|dependency| {
            dependency.from.name == "semantic_use_table" && dependency.to.name == "type_lowering"
        }));

        assert!(matches!(
            table
                .node_value_uses
                .values()
                .find(|value_use| matches!(value_use, SemanticValueUse::Global(_))),
            Some(SemanticValueUse::Global(_))
        ));

        assert!(matches!(
            table
                .node_value_uses
                .values()
                .find(|value_use| matches!(value_use, SemanticValueUse::Local(_))),
            Some(SemanticValueUse::Local(_))
        ));

        assert!(!table.node_type_uses.is_empty());
    }

    #[test]
    fn checked_module_exposes_semantic_use_table_product() {
        let source = "fn main() i32 { let mut local: i32 = 1; local }";
        let checked =
            CompilerDatabase::new(CompileRequest::new(loaded_program_with_modules(vec![
                loaded_module(ModuleId(0), "main.nia", source),
            ])))
            .check_program();

        assert!(checked.diagnostics.is_empty(), "{:?}", checked.diagnostics);
        let module = checked.modules.first().expect("checked module");
        assert!(matches!(
            module
                .semantic_uses
                .node_value_uses
                .values()
                .find(|value_use| matches!(value_use, SemanticValueUse::Local(_))),
            Some(SemanticValueUse::Local(_))
        ));
    }

    #[test]
    fn backend_lowering_uses_checked_module_body_ir() {
        let loaded = loaded_program_with_modules(vec![loaded_module(
            ModuleId(0),
            "main.nia",
            "fn main() i32 { static value: i32 = 1; value }",
        )]);
        let db = query_db(loaded);

        let _ = db.query(BackendLoweringQuery);
        let trace = db.query_trace();

        assert!(trace.dependencies.iter().any(|dependency| {
            dependency.from.name == "backend_lowering" && dependency.to.name == "checked_modules"
        }));
        assert!(trace.dependencies.iter().any(|dependency| {
            dependency.from.name == "backend_lowering"
                && dependency.to.name == "full_active_module_item_tree"
        }));
        assert!(trace.dependencies.iter().any(|dependency| {
            dependency.from.name == "checked_module" && dependency.to.name == "body_check"
        }));
        assert!(!trace.dependencies.iter().any(|dependency| {
            dependency.from.name == "checked_module" && dependency.to.name == "item_signatures"
        }));
        assert!(trace.dependencies.iter().any(|dependency| {
            dependency.from.name == "backend_lowering"
                && dependency.to.name == "signature_item_tree"
        }));
        assert!(!trace.dependencies.iter().any(|dependency| {
            dependency.from.name == "backend_lowering"
                && dependency.to.name == "program_full_defs_by_id"
        }));
        assert!(trace.dependencies.iter().any(|dependency| {
            dependency.from.name == "backend_lowering"
                && dependency.to.name == "program_backend_signatures"
        }));
        assert!(!trace.dependencies.iter().any(|dependency| {
            dependency.from.name == "backend_lowering"
                && dependency.to.name == "comptime_enum_values"
        }));
        assert!(!depends_on_body_signature_query(&trace, "backend_lowering"));
        assert!(!trace.dependencies.iter().any(|dependency| {
            dependency.from.name == "backend_lowering"
                && dependency.to.name == "program_type_normalizations"
        }));
    }

    #[test]
    fn executable_checked_modules_reuse_filtered_comptime_inputs() {
        let mut loaded = loaded_program_with_modules(vec![loaded_module(
            ModuleId(0),
            "main.nia",
            r#"
comptime fn len() usize {
    4
}

fn unused() i32 {
    missing_symbol
}

fn main() i32 {
    let mut values: [len()]i32 = [0; len()];
    values.len() as i32
}
"#,
        )]);
        loaded.runtime = RuntimeModel::FreestandingExecutable;
        let db = query_db(loaded);

        let modules = db.query(ExecutableCheckedModulesQuery);
        let module = modules
            .iter()
            .find(|module| module.id == ModuleId(0))
            .expect("entry module should be executable-reachable");
        let trace = db.query_trace();

        assert!(
            module.body_diagnostics.is_empty(),
            "reachable comptime functions must remain available to executable body checking: {:?}",
            module.body_diagnostics
        );
        assert!(
            module
                .comptime
                .array_lengths
                .values()
                .any(|length| *length == 4),
            "filtered executable comptime phases should retain reachable array lengths"
        );
        assert!(!trace.dependencies.iter().any(|dependency| {
            dependency.from.name == "executable_body_check"
                && matches!(
                    dependency.to.name,
                    "comptime_values" | "comptime_array_lengths" | "comptime_typed_facts"
                )
        }));
        assert!(!trace.dependencies.iter().any(|dependency| {
            dependency.from.name == "executable_body_check"
                && dependency.to.name == "program_trait_solving_signatures"
        }));
        assert!(!trace.dependencies.iter().any(|dependency| {
            dependency.from.name == "executable_checked_modules"
                && matches!(dependency.to.name, "comptime" | "comptime_enum_values")
        }));
    }

    #[test]
    fn executable_body_check_follows_same_module_call_closure() {
        let mut loaded = loaded_program_with_modules(vec![loaded_module(
            ModuleId(0),
            "main.nia",
            r#"
fn f3() i32 {
    3
}

fn f2() i32 {
    f3()
}

fn f1() i32 {
    f2()
}

fn main() i32 {
    f1()
}
"#,
        )]);
        loaded.runtime = RuntimeModel::FreestandingExecutable;
        let db = query_db(loaded);

        let modules = db.query(ExecutableCheckedModulesQuery);
        let module = modules
            .iter()
            .find(|module| module.id == ModuleId(0))
            .expect("entry module should be executable-reachable");
        assert_eq!(
            module.body_ir.function_bodies.len(),
            4,
            "same-module executable body check should retain the whole call closure"
        );
        assert!(
            module.body_diagnostics.is_empty(),
            "same-module executable call closure should check without diagnostics: {:?}",
            module.body_diagnostics
        );
    }

    #[test]
    fn executable_filtered_comptime_resolves_forwarded_array_len_values() {
        let main = loaded_module(
            ModuleId(0),
            "main.nia",
            r#"
module facade;
using entry::facade;

fn main() i32 {
    let mut values: [facade::LEN]u8 = [0; facade::LEN];
    values.len() as i32
}
"#,
        );
        let facade = loaded_module(
            ModuleId(1),
            "facade.nia",
            r#"
module raw;
using self::raw;

pub comptime LEN: usize = raw::LEN;
"#,
        );
        let raw = loaded_module(
            ModuleId(2),
            "facade/raw.nia",
            r#"
pub comptime LEN: usize = 4usize;
"#,
        );
        let mut graph = ModuleGraph::new(main.path.clone());
        graph
            .intern_declared_child(
                main.id,
                "facade",
                nia_ids::Visibility::Public,
                Span::default(),
            )
            .expect("intern facade module");
        graph
            .intern_declared_child(
                facade.id,
                "raw",
                nia_ids::Visibility::Public,
                Span::default(),
            )
            .expect("intern raw module");
        let loaded = LoadedProgram {
            graph,
            target: TargetConfig::host(),
            runtime: RuntimeModel::FreestandingExecutable,
            modules: vec![main, facade, raw],
            diagnostics: Vec::new(),
        };
        let db = query_db(loaded);

        let modules = db.query(ExecutableCheckedModulesQuery);
        let entry = modules
            .iter()
            .find(|module| module.id == ModuleId(0))
            .expect("entry module should be executable-reachable");

        assert!(
            entry.body_diagnostics.is_empty(),
            "filtered executable body checking should resolve forwarded comptime array lengths: {:?}",
            entry.body_diagnostics
        );
        assert!(
            entry
                .comptime
                .array_lengths
                .values()
                .any(|length| *length == 4),
            "filtered executable comptime should evaluate forwarded array length"
        );
    }

    #[test]
    fn executable_filtered_comptime_resolves_local_forwarded_array_len_in_method_body() {
        let main = loaded_module(
            ModuleId(0),
            "main.nia",
            r#"
module raw;
using entry::raw;

comptime LEN: usize = raw::LEN;

struct Box {}

extend Box {
    fn value(&self) usize {
        let mut values: [LEN]u8 = [_]u8[0; LEN];
        values.len()
    }
}

fn main() usize {
    let box = Box {};
    box.value()
}
"#,
        );
        let raw = loaded_module(
            ModuleId(1),
            "raw.nia",
            r#"
pub comptime LEN: usize = 4usize;
"#,
        );
        let mut graph = ModuleGraph::new(main.path.clone());
        graph
            .intern_declared_child(main.id, "raw", nia_ids::Visibility::Public, Span::default())
            .expect("intern raw module");
        let loaded = LoadedProgram {
            graph,
            target: TargetConfig::host(),
            runtime: RuntimeModel::FreestandingExecutable,
            modules: vec![main, raw],
            diagnostics: Vec::new(),
        };
        let db = query_db(loaded);

        let modules = db.query(ExecutableCheckedModulesQuery);
        let entry = modules
            .iter()
            .find(|module| module.id == ModuleId(0))
            .expect("entry module should be executable-reachable");

        assert!(
            entry.body_diagnostics.is_empty(),
            "filtered executable body checking should resolve local forwarded array lengths used in method bodies: {:?}",
            entry.body_diagnostics
        );
        assert!(
            entry
                .comptime
                .array_lengths
                .values()
                .any(|length| *length == 4),
            "filtered executable comptime should evaluate local forwarded method-body array length"
        );
    }

    #[test]
    fn executable_incremental_body_check_preserves_extension_method_receiver_types() {
        let main = loaded_module(
            ModuleId(0),
            "main.nia",
            r#"
module writer;
using entry::writer;

fn main() i32 {
    let mut sink = writer::Sink::init();
    if !value = sink.write(b"ok") {
        value as i32
    } or error! {
        0
    }
}
"#,
        );
        let writer = loaded_module(
            ModuleId(1),
            "writer.nia",
            r#"
pub trait Writer {
    type Error;

    fn short_write(&self) Error;

    fn write(&mut self, bytes: &[u8]) Error!usize;
}

pub enum WriteError: i32 {
    Short = 1,
    _,
}

pub struct Sink {}

extend Sink {
    pub fn init() Sink {
        {}
    }
}

extend Sink : Writer {
    type Error = WriteError;

    pub fn short_write(&self) Error {
        WriteError::Short
    }

    pub fn write(&mut self, bytes: &[u8]) Error!usize {
        if bytes.len() == 0 {
            return self.short_write()!;
        }
        !bytes.len()
    }
}
"#,
        );
        let mut graph = ModuleGraph::new(main.path.clone());
        graph
            .intern_declared_child(
                main.id,
                "writer",
                nia_ids::Visibility::Public,
                Span::default(),
            )
            .expect("intern writer module");
        let loaded = LoadedProgram {
            graph,
            target: TargetConfig::host(),
            runtime: RuntimeModel::FreestandingExecutable,
            modules: vec![main, writer],
            diagnostics: Vec::new(),
        };
        let db = query_db(loaded);

        let modules = db.query(ExecutableCheckedModulesQuery);
        let writer = modules
            .iter()
            .find(|module| module.id == ModuleId(1))
            .expect("writer module should be executable-reachable");
        let write_def = writer
            .defs
            .defs
            .iter()
            .find_map(|(def_id, def)| {
                (def.name == "write" && def.kind == nia_defs::DefKind::Method).then_some(def_id)
            })
            .expect("write method should be defined");
        let write_id = GlobalDefId {
            module_id: ModuleId(1),
            def_id: write_def,
        };
        let write_body = writer
            .body_ir
            .function_bodies
            .get(&write_id)
            .expect("write method should have a checked body");
        let self_ty = write_body
            .locals
            .iter()
            .find(|local| local.name == "self" && local.kind == nia_body_ir::TypedLocalKind::Param)
            .map(|local| local.ty)
            .expect("write method should have a self param");

        assert!(
            !matches!(writer.body_ir.interner.get(self_ty), Some(TyKind::Error)),
            "reachable extension method receiver/params should not collapse to error types"
        );
    }

    #[test]
    fn trait_signature_subset_resolves_local_extend_target_types() {
        let loaded = loaded_program_with_modules(vec![loaded_module(
            ModuleId(0),
            "main.nia",
            r#"
trait Writer {
    type Error;
    fn write(&mut self) Error!void;
}

enum WriteError: i32 {
    Bad = 1,
    _,
}

struct Sink {}

extend Sink : Writer {
    type Error = WriteError;

    fn write(&mut self) Error!void {
        !{}
    }
}
"#,
        )]);
        let db = query_db(loaded);

        let signatures = db.query(SignatureItemSignaturesQuery(
            ModuleId(0),
            nia_item_tree::SignatureItemSet::Traits,
        ));
        let lowering = db.query(SignatureTypeLoweringQuery(
            ModuleId(0),
            nia_item_tree::SignatureItemSet::Traits,
        ));
        let impl_signature = signatures
            .trait_impls
            .iter()
            .find(|impl_signature| !impl_signature.methods.is_empty())
            .expect("trait impl should be collected");

        assert!(
            !matches!(
                lowering.interner.get(impl_signature.target_ty),
                Some(TyKind::Error)
            ),
            "trait signature subset should resolve local extend target types"
        );
    }

    #[test]
    fn trait_signature_subset_resolves_imported_extend_target_types() {
        let main = loaded_module(
            ModuleId(0),
            "main.nia",
            r#"
module platform;
using entry::platform;

trait IntoError[Target] {
    fn into_error(self) Target;
}

enum Error: i32 {
    Bad = 1,
    _,
}

extend platform::Errno : IntoError[Error] {
    fn into_error(self) Error {
        Error::Bad
    }
}
"#,
        );
        let platform = loaded_module(
            ModuleId(1),
            "platform.nia",
            r#"
pub enum Errno: i32 {
    Bad = 1,
    _,
}
"#,
        );
        let mut graph = ModuleGraph::new(main.path.clone());
        graph
            .intern_declared_child(
                main.id,
                "platform",
                nia_ids::Visibility::Public,
                Span::default(),
            )
            .expect("intern platform module");
        let loaded = LoadedProgram {
            graph,
            target: TargetConfig::host(),
            runtime: RuntimeModel::FreestandingExecutable,
            modules: vec![main, platform],
            diagnostics: Vec::new(),
        };
        let db = query_db(loaded);

        let signatures = db.query(SignatureItemSignaturesQuery(
            ModuleId(0),
            nia_item_tree::SignatureItemSet::Traits,
        ));
        let lowering = db.query(SignatureTypeLoweringQuery(
            ModuleId(0),
            nia_item_tree::SignatureItemSet::Traits,
        ));
        let impl_signature = signatures
            .trait_impls
            .iter()
            .find(|impl_signature| !impl_signature.methods.is_empty())
            .expect("trait impl should be collected");

        assert!(
            !matches!(
                lowering.interner.get(impl_signature.target_ty),
                Some(TyKind::Error)
            ),
            "trait signature subset should resolve imported extend target types"
        );
    }

    #[test]
    fn trait_signature_subset_resolves_reexported_extend_target_types() {
        let main = loaded_module(
            ModuleId(0),
            "main.nia",
            r#"
module platform;
using entry::platform;

trait IntoError[Target] {
    fn into_error(self) Target;
}

enum Error: i32 {
    Bad = 1,
    _,
}

extend platform::Errno : IntoError[Error] {
    fn into_error(self) Error {
        Error::Bad
    }
}
"#,
        );
        let platform = loaded_module(
            ModuleId(1),
            "platform.nia",
            r#"
module types;
using entry::platform::types;

pub using types::{Errno};
"#,
        );
        let types = loaded_module(
            ModuleId(2),
            "types.nia",
            r#"
pub enum Errno: i32 {
    Bad = 1,
    _,
}
"#,
        );
        let mut graph = ModuleGraph::new(main.path.clone());
        graph
            .intern_declared_child(
                main.id,
                "platform",
                nia_ids::Visibility::Public,
                Span::default(),
            )
            .expect("intern platform module");
        graph
            .intern_declared_child(
                platform.id,
                "types",
                nia_ids::Visibility::Public,
                Span::default(),
            )
            .expect("intern platform types module");
        let loaded = LoadedProgram {
            graph,
            target: TargetConfig::host(),
            runtime: RuntimeModel::FreestandingExecutable,
            modules: vec![main, platform, types],
            diagnostics: Vec::new(),
        };
        let db = query_db(loaded);

        let signatures = db.query(SignatureItemSignaturesQuery(
            ModuleId(0),
            nia_item_tree::SignatureItemSet::Traits,
        ));
        let lowering = db.query(SignatureTypeLoweringQuery(
            ModuleId(0),
            nia_item_tree::SignatureItemSet::Traits,
        ));
        let impl_signature = signatures
            .trait_impls
            .iter()
            .find(|impl_signature| !impl_signature.methods.is_empty())
            .expect("trait impl should be collected");

        assert!(
            !matches!(
                lowering.interner.get(impl_signature.target_ty),
                Some(TyKind::Error)
            ),
            "trait signature subset should resolve re-exported extend target types"
        );
    }

    #[test]
    fn executable_incremental_body_check_preserves_reexported_trait_witness_receiver_types() {
        let main = loaded_module(
            ModuleId(0),
            "main.nia",
            r#"
module platform;
using entry::platform;

trait IntoError[Target] {
    fn into_error(self) Target;
}

extend[T, Source, Target] Source!T
where Source: IntoError[Target]
{
    fn cast_error(self) Target!T {
        if !ok = self {
            !ok
        } or error! {
            error.into_error()!
        }
    }
}

enum Error: i32 {
    Bad = 1,
    _,
}

extend platform::Errno : IntoError[Error] {
    fn into_error(self) Error {
        Error::Bad
    }
}

fn fail() platform::Errno!i32 {
    platform::Errno::Bad!
}

fn main() Error!i32 {
    fail().cast_error()
}
"#,
        );
        let platform = loaded_module(
            ModuleId(1),
            "platform.nia",
            r#"
module types;
using entry::platform::types;

pub using types::{Errno};
"#,
        );
        let types = loaded_module(
            ModuleId(2),
            "types.nia",
            r#"
pub enum Errno: i32 {
    Bad = 1,
    _,
}
"#,
        );
        let mut graph = ModuleGraph::new(main.path.clone());
        graph
            .intern_declared_child(
                main.id,
                "platform",
                nia_ids::Visibility::Public,
                Span::default(),
            )
            .expect("intern platform module");
        graph
            .intern_declared_child(
                platform.id,
                "types",
                nia_ids::Visibility::Public,
                Span::default(),
            )
            .expect("intern platform types module");
        let loaded = LoadedProgram {
            graph,
            target: TargetConfig::host(),
            runtime: RuntimeModel::FreestandingExecutable,
            modules: vec![main, platform, types],
            diagnostics: Vec::new(),
        };
        let db = query_db(loaded);

        let modules = db.query(ExecutableCheckedModulesQuery);
        let module = modules
            .iter()
            .find(|module| module.id == ModuleId(0))
            .expect("entry module should be executable-reachable");
        assert!(
            module.body_diagnostics.is_empty(),
            "generic extension wrapper diagnostics should stay clean: {:?}",
            module.body_diagnostics
        );
        let into_error = module
            .defs
            .defs
            .iter()
            .find_map(|(def_id, def)| {
                (def.name == "into_error" && def.kind == nia_defs::DefKind::Method).then_some(
                    GlobalDefId {
                        module_id: ModuleId(0),
                        def_id,
                    },
                )
            })
            .expect("into_error method should be defined");
        let body = module
            .body_ir
            .function_bodies
            .get(&into_error)
            .expect("into_error should have a checked body");
        let self_ty = body
            .locals
            .iter()
            .find(|local| local.name == "self" && local.kind == nia_body_ir::TypedLocalKind::Param)
            .map(|local| local.ty)
            .expect("into_error should have a self param");

        assert!(
            !matches!(module.body_ir.interner.get(self_ty), Some(TyKind::Error)),
            "re-exported trait witness receiver should not collapse to error"
        );
    }

    #[test]
    fn executable_reachability_expands_where_predicates_through_generic_extension_wrappers() {
        let main = loaded_module(
            ModuleId(0),
            "main.nia",
            r#"
module error;
module facade;
using entry::error;
using entry::facade;

enum Error: i32 {
    Bad = 1,
    _,
}

struct Source {
    value: i32,
}

struct Target {
    value: i32,
}

extend Source : error::IntoError[Target] {
    fn into_error(self) Target {
        Target { value: self.value }
    }
}

fn main() i32 {
    let value: Source!i32 = Source { value: 1 }!;
    if !ok = value.cast_error() {
        ok
    } or error! {
        error.value
    }
}
"#,
        );
        let error = loaded_module(
            ModuleId(1),
            "error.nia",
            r#"
pub trait IntoError[Target] {
    fn into_error(self) Target;
}

extend[T, Source, Target] Source!T
where Source: IntoError[Target]
{
    pub fn cast_error(self) Target!T {
        if !ok = self {
            !ok
        } or error! {
            error.into_error()!
        }
    }
}
"#,
        );
        let facade = loaded_module(
            ModuleId(2),
            "facade.nia",
            r#"
using entry::error;
"#,
        );
        let mut graph = ModuleGraph::new(main.path.clone());
        graph
            .intern_declared_child(
                main.id,
                "error",
                nia_ids::Visibility::Public,
                Span::default(),
            )
            .expect("intern error module");
        graph
            .intern_declared_child(
                main.id,
                "facade",
                nia_ids::Visibility::Public,
                Span::default(),
            )
            .expect("intern facade module");
        let loaded = LoadedProgram {
            graph,
            target: TargetConfig::host(),
            runtime: RuntimeModel::FreestandingExecutable,
            modules: vec![main, error, facade],
            diagnostics: Vec::new(),
        };
        let db = query_db(loaded);

        let modules = db.query(ExecutableCheckedModulesQuery);
        let module = modules
            .iter()
            .find(|module| module.id == ModuleId(0))
            .expect("entry module should be executable-reachable");
        let into_error = module
            .defs
            .defs
            .iter()
            .find_map(|(def_id, def)| {
                (def.name == "into_error" && def.kind == nia_defs::DefKind::Method).then_some(
                    GlobalDefId {
                        module_id: ModuleId(0),
                        def_id,
                    },
                )
            })
            .expect("into_error method should be defined");

        assert!(
            module.body_ir.function_bodies.contains_key(&into_error),
            "generic extension wrappers should make where-predicate trait witnesses executable-reachable"
        );
    }

    #[test]
    fn executable_reachability_expands_generic_trait_calls_to_cross_module_impl_bodies() {
        let main = loaded_module(
            ModuleId(0),
            "main.nia",
            r#"
module error;
module impls;
using entry::error;
using entry::impls;

fn main() i32 {
    let value: impls::Source!i32 = impls::Source { value: 1 }!;
    if !ok = value.cast_error() {
        ok
    } or error! {
        error.value
    }
}
"#,
        );
        let error = loaded_module(
            ModuleId(1),
            "error.nia",
            r#"
pub trait IntoError[Target] {
    fn into_error(self) Target;
}

extend[T, Source, Target] Source!T
where Source: IntoError[Target]
{
    pub fn cast_error(self) Target!T {
        if !ok = self {
            !ok
        } or error! {
            error.into_error()!
        }
    }
}
"#,
        );
        let impls = loaded_module(
            ModuleId(2),
            "impls.nia",
            r#"
using entry::error;

pub struct Source {
    value: i32,
}

pub struct Target {
    value: i32,
}

extend Source : error::IntoError[Target] {
    fn into_error(self) Target {
        Target { value: self.value }
    }
}
"#,
        );
        let mut graph = ModuleGraph::new(main.path.clone());
        graph
            .intern_declared_child(
                main.id,
                "error",
                nia_ids::Visibility::Public,
                Span::default(),
            )
            .expect("intern error module");
        graph
            .intern_declared_child(
                main.id,
                "impls",
                nia_ids::Visibility::Public,
                Span::default(),
            )
            .expect("intern impls module");
        let loaded = LoadedProgram {
            graph,
            target: TargetConfig::host(),
            runtime: RuntimeModel::FreestandingExecutable,
            modules: vec![main, error, impls],
            diagnostics: Vec::new(),
        };
        let db = query_db(loaded);

        let modules = db.query(ExecutableCheckedModulesQuery);
        let module = modules
            .iter()
            .find(|module| module.id == ModuleId(2))
            .expect("impl module should be executable-reachable");
        let into_error = module
            .defs
            .defs
            .iter()
            .find_map(|(def_id, def)| {
                (def.name == "into_error" && def.kind == nia_defs::DefKind::Method).then_some(
                    GlobalDefId {
                        module_id: ModuleId(2),
                        def_id,
                    },
                )
            })
            .expect("cross-module into_error method should be defined");

        assert!(
            module.body_ir.function_bodies.contains_key(&into_error),
            "generic trait calls should make cross-module impl method bodies executable-reachable"
        );
    }

    #[test]
    fn executable_reachability_expands_generic_trait_calls_from_incremental_wrapper_bodies() {
        let main = loaded_module(
            ModuleId(0),
            "main.nia",
            r#"
module error;
module impls;
using entry::error;
using entry::impls;

fn main() i32 {
    let value: impls::Source!i32 = impls::Source { value: 1 }!;
    if !ok = value.as_target_error() {
        ok
    } or error! {
        error.value
    }
}
"#,
        );
        let error = loaded_module(
            ModuleId(1),
            "error.nia",
            r#"
pub trait IntoError[Target] {
    fn into_error(self) Target;
}

extend[T, Source, Target] Source!T
where Source: IntoError[Target]
{
    pub fn cast_error(self) Target!T {
        if !ok = self {
            !ok
        } or error! {
            error.into_error()!
        }
    }
}
"#,
        );
        let impls = loaded_module(
            ModuleId(2),
            "impls.nia",
            r#"
using entry::error;

pub struct Source {
    value: i32,
}

pub struct Target {
    value: i32,
}

extend Source : error::IntoError[Target] {
    fn into_error(self) Target {
        Target { value: self.value }
    }
}

extend[T] Source!T {
    pub fn as_target_error(self) Target!T {
        self.cast_error()
    }
}
"#,
        );
        let mut graph = ModuleGraph::new(main.path.clone());
        graph
            .intern_declared_child(
                main.id,
                "error",
                nia_ids::Visibility::Public,
                Span::default(),
            )
            .expect("intern error module");
        graph
            .intern_declared_child(
                main.id,
                "impls",
                nia_ids::Visibility::Public,
                Span::default(),
            )
            .expect("intern impls module");
        let loaded = LoadedProgram {
            graph,
            target: TargetConfig::host(),
            runtime: RuntimeModel::FreestandingExecutable,
            modules: vec![main, error, impls],
            diagnostics: Vec::new(),
        };
        let db = query_db(loaded);

        let modules = db.query(ExecutableCheckedModulesQuery);
        let module = modules
            .iter()
            .find(|module| module.id == ModuleId(2))
            .expect("impl module should be executable-reachable");
        let into_error = module
            .defs
            .defs
            .iter()
            .find_map(|(def_id, def)| {
                (def.name == "into_error" && def.kind == nia_defs::DefKind::Method).then_some(
                    GlobalDefId {
                        module_id: ModuleId(2),
                        def_id,
                    },
                )
            })
            .expect("cross-module into_error method should be defined");

        assert!(
            module.body_ir.function_bodies.contains_key(&into_error),
            "generic wrapper bodies checked after incremental reachability must still expand their trait witnesses"
        );
    }

    #[test]
    fn executable_checked_modules_include_reachable_builtin_trait_witness_bodies() {
        let mut loaded = loaded_program_with_modules(vec![loaded_module(
            ModuleId(0),
            "main.nia",
            r#"
struct Counter {
    current: i32,
    end: i32,
}

extend Counter : Iterator {
    type Item = i32;

    fn next(&mut self) ?i32 {
        if self.current >= self.end {
            null
        } else {
            let value = self.current;
            self.current += 1;
            ?value
        }
    }
}

fn main() i32 {
    let mut total = 0;
    let mut iter = Counter { current: 0, end: 3 };
    for value in iter {
        total += value;
    }
    total
}
"#,
        )]);
        loaded.runtime = RuntimeModel::FreestandingExecutable;
        let db = query_db(loaded);

        let modules = db.query(ExecutableCheckedModulesQuery);
        let module = modules
            .iter()
            .find(|module| module.id == ModuleId(0))
            .expect("entry module should be executable-reachable");
        let next = module
            .defs
            .defs
            .iter()
            .find_map(|(def_id, def)| {
                (def.kind == nia_defs::DefKind::Method && def.name == "next").then_some(
                    GlobalDefId {
                        module_id: ModuleId(0),
                        def_id,
                    },
                )
            })
            .expect("Iterator witness method");

        assert!(
            module.body_ir.function_bodies.contains_key(&next),
            "executable body checking must include builtin trait witness bodies"
        );
    }

    #[test]
    fn executable_checked_modules_do_not_body_check_unmatched_builtin_trait_witnesses() {
        let mut loaded = loaded_program_with_modules(vec![loaded_module(
            ModuleId(0),
            "main.nia",
            r#"
struct Counter {
    current: i32,
    end: i32,
}

extend Counter : Iterator {
    type Item = i32;

    fn next(&mut self) ?i32 {
        if self.current >= self.end {
            null
        } else {
            let value = self.current;
            self.current += 1;
            ?value
        }
    }
}

struct Unused {}

extend Unused : Iterator {
    type Item = i32;

    fn next(&mut self) ?i32 {
        ?missing_symbol
    }
}

fn main() i32 {
    let mut total = 0;
    let mut iter = Counter { current: 0, end: 3 };
    for value in iter {
        total += value;
    }
    total
}
"#,
        )]);
        loaded.runtime = RuntimeModel::FreestandingExecutable;
        let db = query_db(loaded);

        let modules = db.query(ExecutableCheckedModulesQuery);
        let module = modules
            .iter()
            .find(|module| module.id == ModuleId(0))
            .expect("entry module should be executable-reachable");
        let unused_next = module
            .defs
            .defs
            .iter()
            .filter_map(|(def_id, def)| {
                (def.kind == nia_defs::DefKind::Method && def.name == "next").then_some(
                    GlobalDefId {
                        module_id: ModuleId(0),
                        def_id,
                    },
                )
            })
            .filter(|def_id| !module.body_ir.function_bodies.contains_key(def_id))
            .next()
            .expect("unmatched Iterator witness method");

        assert!(
            !module.body_ir.function_bodies.contains_key(&unused_next),
            "executable reachability should not include builtin trait witnesses for unmatched receiver types"
        );
        assert!(
            module.body_diagnostics.is_empty(),
            "unmatched builtin trait witness diagnostics should not block executable checking: {:?}",
            module.body_diagnostics
        );
    }

    #[test]
    fn executable_checked_modules_do_not_body_check_unused_trait_witness_methods() {
        let mut loaded = loaded_program_with_modules(vec![loaded_module(
            ModuleId(0),
            "main.nia",
            r#"
trait Ops {
    fn used(self) i32;
    fn unused(self) i32;
}

struct Value {}

extend Value : Ops {
    fn used(self) i32 {
        1
    }

    fn unused(self) i32 {
        missing_symbol
    }
}

fn main() i32 {
    let value = Value {};
    value.used()
}
"#,
        )]);
        loaded.runtime = RuntimeModel::FreestandingExecutable;
        let db = query_db(loaded);

        let modules = db.query(ExecutableCheckedModulesQuery);
        let module = modules
            .iter()
            .find(|module| module.id == ModuleId(0))
            .expect("entry module should be executable-reachable");
        let unused = module
            .defs
            .defs
            .iter()
            .find_map(|(def_id, def)| {
                (def.kind == nia_defs::DefKind::Method && def.name == "unused").then_some(
                    GlobalDefId {
                        module_id: ModuleId(0),
                        def_id,
                    },
                )
            })
            .expect("unused witness method");

        assert!(
            !module.body_ir.function_bodies.contains_key(&unused),
            "executable body checking should not include unused trait witness bodies"
        );
    }

    #[test]
    fn executable_checked_modules_include_trait_witnesses_required_by_generic_where_predicates() {
        let mut loaded = loaded_program_with_modules(vec![loaded_module(
            ModuleId(0),
            "main.nia",
            r#"
trait IntoError[Target] {
    fn into_error(self) Target;
}

extend[T, Source, Target] Source!T
where Source: IntoError[Target]
{
    fn cast_error(self) Target!T {
        if !ok = self {
            !ok
        } or error! {
            error.into_error()!
        }
    }
}

struct Source {
    value: i32,
}

struct Target {
    value: i32,
}

extend Source : IntoError[Target] {
    fn into_error(self) Target {
        Target { value: self.value }
    }
}

struct Unused {}

extend Unused : IntoError[Target] {
    fn into_error(self) Target {
        missing_symbol
    }
}

fn main() i32 {
    let value: Source!i32 = Source { value: 1 }!;
    if !ok = value.cast_error() {
        ok
    } or error! {
        error.value
    }
}
"#,
        )]);
        loaded.runtime = RuntimeModel::FreestandingExecutable;
        let db = query_db(loaded);

        let modules = db.query(ExecutableCheckedModulesQuery);
        let module = modules
            .iter()
            .find(|module| module.id == ModuleId(0))
            .expect("entry module should be executable-reachable");
        let into_error_methods = module
            .defs
            .defs
            .iter()
            .filter_map(|(def_id, def)| {
                (def.kind == nia_defs::DefKind::Method && def.name == "into_error").then_some(
                    GlobalDefId {
                        module_id: ModuleId(0),
                        def_id,
                    },
                )
            })
            .collect::<Vec<_>>();
        let reachable_into_error_count = into_error_methods
            .iter()
            .filter(|def_id| module.body_ir.function_bodies.contains_key(def_id))
            .count();

        assert_eq!(
            reachable_into_error_count, 1,
            "generic where-predicate closure should include only the matching IntoError witness"
        );
        assert!(
            module.body_diagnostics.is_empty(),
            "unmatched IntoError witness diagnostics should not block executable checking: {:?}",
            module.body_diagnostics
        );
    }

    #[test]
    fn executable_checked_modules_include_trait_witnesses_required_by_default_method_body() {
        let mut loaded = loaded_program_with_modules(vec![loaded_module(
            ModuleId(0),
            "main.nia",
            r#"
trait Writer {
    type Error;

    fn short_write(&self) Error;

    fn write(&mut self) Error!usize;

    fn write_all(&mut self) Error!void {
        let n = self.write().?;
        if n == 0usize {
            return self.short_write()!;
        }
        !{}
    }
}

struct FileWriter {
    value: i32,
}

extend FileWriter : Writer {
    type Error = i32;

    fn short_write(&self) Error {
        1
    }

    fn write(&mut self) Error!usize {
        self.value = 2;
        !1usize
    }
}

struct Unused {}

extend Unused : Writer {
    type Error = i32;

    fn short_write(&self) Error {
        missing_symbol
    }

    fn write(&mut self) Error!usize {
        missing_symbol
    }
}

fn main() i32!i32 {
    let mut writer = FileWriter { value: 0 };
    writer.write_all().?;
    !writer.value
}
"#,
        )]);
        loaded.runtime = RuntimeModel::FreestandingExecutable;
        let db = query_db(loaded);

        let modules = db.query(ExecutableCheckedModulesQuery);
        let module = modules
            .iter()
            .find(|module| module.id == ModuleId(0))
            .expect("entry module should be executable-reachable");
        let checked_witness_names = module
            .defs
            .defs
            .iter()
            .filter_map(|(def_id, def)| {
                (def.kind == nia_defs::DefKind::Method
                    && module.body_ir.function_bodies.contains_key(&GlobalDefId {
                        module_id: ModuleId(0),
                        def_id,
                    }))
                .then_some(def.name.as_str())
            })
            .collect::<Vec<_>>();

        assert!(
            checked_witness_names.contains(&"write"),
            "default method reachability should include concrete write witness: {checked_witness_names:?}"
        );
        assert!(
            checked_witness_names.contains(&"short_write"),
            "default method reachability should include concrete short_write witness: {checked_witness_names:?}"
        );
        assert!(
            module.body_diagnostics.is_empty(),
            "unmatched Writer witness diagnostics should not block executable checking: {:?}",
            module.body_diagnostics
        );
    }

    #[test]
    fn executable_checked_modules_do_not_body_check_unreachable_globals() {
        let mut loaded = loaded_program_with_modules(vec![loaded_module(
            ModuleId(0),
            "main.nia",
            r#"
static unused = missing_symbol;

fn main() i32 {
    0
}
"#,
        )]);
        loaded.runtime = RuntimeModel::FreestandingExecutable;
        let db = query_db(loaded);

        let modules = db.query(ExecutableCheckedModulesQuery);
        let module = modules
            .iter()
            .find(|module| module.id == ModuleId(0))
            .expect("entry module should be executable-reachable");
        let unused = module
            .defs
            .defs
            .iter()
            .find_map(|(def_id, def)| {
                (def.kind == nia_defs::DefKind::Global && def.name == "unused").then_some(
                    GlobalDefId {
                        module_id: ModuleId(0),
                        def_id,
                    },
                )
            })
            .expect("unused global");

        assert!(
            module.body_diagnostics.is_empty(),
            "unreachable global body diagnostics should not block executable checking: {:?}",
            module.body_diagnostics
        );
        assert!(
            !module.body_ir.global_inits.contains_key(&unused),
            "unreachable global initializers should not be retained for executable codegen"
        );
    }

    #[test]
    fn executable_backend_lowering_skips_unreachable_recursive_aggregates() {
        let mut loaded = loaded_program_with_modules(vec![loaded_module(
            ModuleId(0),
            "main.nia",
            r#"
struct Recursive {
    next: Recursive,
}

fn unused(value: Recursive) i32 {
    1
}

fn main() i32 {
    0
}
"#,
        )]);
        loaded.runtime = RuntimeModel::FreestandingExecutable;
        let db = query_db(loaded);

        let modules = db.query(ExecutableCheckedModulesQuery);
        let module = modules
            .iter()
            .find(|module| module.id == ModuleId(0))
            .expect("entry module should be executable-reachable");
        assert!(
            module.layouts.diagnostics.is_empty(),
            "unreachable recursive aggregate should not force layout diagnostics: {:?}",
            module.layouts.diagnostics
        );

        let backend_lowering = db.query(BackendLoweringQuery);
        let backend_module = backend_lowering
            .program
            .modules
            .iter()
            .find(|module| module.id == ModuleId(0))
            .expect("entry module should be backend-lowered");
        assert!(
            backend_module
                .structs
                .iter()
                .all(|item| item.name != "Recursive"),
            "unreachable recursive aggregate should not be lowered for codegen"
        );
    }

    #[test]
    fn executable_backend_lowering_imports_external_extension_owner_where_predicates() {
        let main = loaded_module(
            ModuleId(0),
            "main.nia",
            r#"
module ext;
module bounds;
using entry::ext;
using entry::bounds;

fn main() i32 {
    let value = ext::Box[bounds::Token]::init(bounds::Token {});
    value.get()
}
"#,
        );
        let ext = loaded_module(
            ModuleId(1),
            "ext.nia",
            r#"
using entry::bounds;

pub struct Box[T]
where T: bounds::Marker
{
    value: T,
}

extend[T] Box[T]
where T: bounds::Marker
{
    pub fn init(value: T) Box[T] {
        { value: value }
    }

    pub fn get(self) i32 {
        1
    }
}
"#,
        );
        let bounds = loaded_module(
            ModuleId(2),
            "bounds.nia",
            r#"
pub trait Marker {}

pub struct Token {}

extend Token : Marker {}
"#,
        );
        let mut graph = ModuleGraph::new(main.path.clone());
        graph
            .intern_declared_child(main.id, "ext", nia_ids::Visibility::Public, Span::default())
            .expect("intern ext module");
        graph
            .intern_declared_child(
                main.id,
                "bounds",
                nia_ids::Visibility::Public,
                Span::default(),
            )
            .expect("intern bounds module");
        let loaded = LoadedProgram {
            graph,
            target: TargetConfig::host(),
            runtime: RuntimeModel::FreestandingExecutable,
            modules: vec![main, ext, bounds],
            diagnostics: Vec::new(),
        };
        let db = query_db(loaded);

        let backend_lowering = db.query(BackendLoweringQuery);

        assert!(
            backend_lowering.diagnostics.is_empty(),
            "backend lowering should import external extension owner predicates without diagnostics: {:?}",
            backend_lowering.diagnostics
        );
    }

    #[test]
    fn executable_checked_modules_keep_type_owner_modules_type_only() {
        let entry = loaded_module(
            ModuleId(0),
            "main.nia",
            r#"
module types;
using entry::types;

fn main(value: types::Used) i32 {
    value.value
}
"#,
        );
        let types = loaded_module(
            ModuleId(1),
            "types.nia",
            r#"
pub struct Used {
    value: i32,
}

pub fn unused_bad() i32 {
    missing_symbol
}
"#,
        );
        let mut loaded = loaded_program_with_entry_child(entry, "types", types);
        loaded.runtime = RuntimeModel::FreestandingExecutable;
        let db = query_db(loaded);

        let modules = db.query(ExecutableCheckedModulesQuery);
        let type_module = modules
            .iter()
            .find(|module| module.id == ModuleId(1))
            .expect("type owner module should be present for backend type lookup");
        assert!(
            type_module.executable_type_only,
            "type owner module should not be treated as an executable body module"
        );
        assert!(
            type_module.body_ir.function_bodies.is_empty(),
            "type owner module should not retain or check function bodies"
        );

        let trace = db.query_trace();
        assert!(
            !trace.queries.iter().any(|query| {
                query.frame.name == "executable_body_check"
                    && query.frame.description.contains("ModuleId(1)")
                    && query.stats.executions > 0
            }),
            "type owner module should not be executable-body-checked: {:?}",
            trace
                .queries
                .iter()
                .filter(|query| query.frame.name == "executable_body_check")
                .collect::<Vec<_>>()
        );
    }

    #[test]
    fn executable_checked_modules_do_not_body_check_modules_for_generic_metadata_only() {
        let main = loaded_module(
            ModuleId(0),
            "main.nia",
            r#"
module helper;
using entry::helper;

fn main() i32 {
    helper::id[i32](1)
}
"#,
        );
        let helper = loaded_module(
            ModuleId(1),
            "helper.nia",
            r#"
pub fn id[T](value: T) T {
    value
}

fn unused_bad() i32 {
    missing_symbol
}
"#,
        );
        let mut loaded = loaded_program_with_entry_child(main, "helper", helper);
        loaded.runtime = RuntimeModel::FreestandingExecutable;
        let db = query_db(loaded);

        let modules = db.query(ExecutableCheckedModulesQuery);
        let helper_module = modules
            .iter()
            .find(|module| module.id == ModuleId(1))
            .expect("called generic function owner should be executable-reachable");
        let unused_bad = helper_module
            .defs
            .defs
            .iter()
            .find_map(|(def_id, def)| {
                (def.kind == nia_defs::DefKind::Function && def.name == "unused_bad").then_some(
                    GlobalDefId {
                        module_id: ModuleId(1),
                        def_id,
                    },
                )
            })
            .expect("unused function");

        assert!(
            helper_module.body_diagnostics.is_empty(),
            "unused function in a generic callee module should not be body-checked: {:?}",
            helper_module.body_diagnostics
        );
        assert!(
            !helper_module
                .body_ir
                .function_bodies
                .contains_key(&unused_bad),
            "reachable generic metadata should not retain unrelated function bodies"
        );
    }

    #[test]
    fn executable_checked_modules_include_reachable_global_initializers() {
        let mut loaded = loaded_program_with_modules(vec![loaded_module(
            ModuleId(0),
            "main.nia",
            r#"
static used: i32 = 1;

fn main() i32 {
    used
}
"#,
        )]);
        loaded.runtime = RuntimeModel::FreestandingExecutable;
        let db = query_db(loaded);

        let modules = db.query(ExecutableCheckedModulesQuery);
        let module = modules
            .iter()
            .find(|module| module.id == ModuleId(0))
            .expect("entry module should be executable-reachable");
        let used = module
            .defs
            .defs
            .iter()
            .find_map(|(def_id, def)| {
                (def.kind == nia_defs::DefKind::Global && def.name == "used").then_some(
                    GlobalDefId {
                        module_id: ModuleId(0),
                        def_id,
                    },
                )
            })
            .expect("used global");

        assert!(
            module.body_ir.global_inits.contains_key(&used),
            "reachable global initializers must be retained for executable codegen"
        );
    }

    #[test]
    fn executable_checked_modules_include_reachable_local_static_initializers() {
        let mut loaded = loaded_program_with_modules(vec![loaded_module(
            ModuleId(0),
            "main.nia",
            r#"
fn option_arg() &u8 {
    static text = b"-O2\0";
    &text[0]
}

fn main() i32 {
    _ = option_arg();
    0
}
"#,
        )]);
        loaded.runtime = RuntimeModel::FreestandingExecutable;
        let db = query_db(loaded);

        let modules = db.query(ExecutableCheckedModulesQuery);
        let module = modules
            .iter()
            .find(|module| module.id == ModuleId(0))
            .expect("entry module should be executable-reachable");
        let text = module
            .defs
            .defs
            .iter()
            .find_map(|(def_id, def)| {
                (def.kind == nia_defs::DefKind::Global && def.name == "text").then_some(
                    GlobalDefId {
                        module_id: ModuleId(0),
                        def_id,
                    },
                )
            })
            .expect("local static global");

        assert!(
            module.body_ir.global_inits.contains_key(&text),
            "reachable local static initializers must be retained for executable codegen"
        );
    }

    #[test]
    fn executable_checked_modules_include_reachable_extension_method_local_static_initializers() {
        let mut loaded = loaded_program_with_modules(vec![loaded_module(
            ModuleId(0),
            "main.nia",
            r#"
enum Mode: i32 {
    O2 = 2,
}

extend Mode {
    fn argv(self) &u8 {
        static o2 = b"-O2\0";
        switch self {
            Mode::O2 => &o2[0],
            _ => &o2[0],
        }
    }
}

fn main() i32 {
    _ = Mode::O2.argv();
    0
}
"#,
        )]);
        loaded.runtime = RuntimeModel::FreestandingExecutable;
        let db = query_db(loaded);

        let modules = db.query(ExecutableCheckedModulesQuery);
        let module = modules
            .iter()
            .find(|module| module.id == ModuleId(0))
            .expect("entry module should be executable-reachable");
        let o2 = module
            .defs
            .defs
            .iter()
            .find_map(|(def_id, def)| {
                (def.kind == nia_defs::DefKind::Global && def.name == "o2").then_some(GlobalDefId {
                    module_id: ModuleId(0),
                    def_id,
                })
            })
            .expect("local static global");

        assert!(
            module.body_ir.global_inits.contains_key(&o2),
            "reachable extension method local static initializers must be retained for executable codegen"
        );
    }

    #[test]
    fn executable_checked_modules_include_cross_module_extension_method_local_static_initializers()
    {
        let main = loaded_module(
            ModuleId(0),
            "main.nia",
            r#"
using helper::Mode;

fn main() i32 {
    _ = Mode::O2.argv();
    0
}
"#,
        );
        let helper = loaded_module(
            ModuleId(1),
            "helper.nia",
            r#"
pub enum Mode: i32 {
    O2 = 2,
}

extend Mode {
    pub fn argv(self) &u8 {
        static o2 = b"-O2\0";
        switch self {
            Mode::O2 => &o2[0],
            _ => &o2[0],
        }
    }
}
"#,
        );
        let mut loaded = loaded_program_with_entry_child(main, "helper", helper);
        loaded.runtime = RuntimeModel::FreestandingExecutable;
        let db = query_db(loaded);

        let modules = db.query(ExecutableCheckedModulesQuery);
        let module = modules
            .iter()
            .find(|module| module.id == ModuleId(1))
            .expect("helper module should be executable-reachable");
        let o2 = module
            .defs
            .defs
            .iter()
            .find_map(|(def_id, def)| {
                (def.kind == nia_defs::DefKind::Global && def.name == "o2").then_some(GlobalDefId {
                    module_id: ModuleId(1),
                    def_id,
                })
            })
            .expect("local static global");

        assert!(
            module.body_ir.global_inits.contains_key(&o2),
            "reachable cross-module extension method local static initializers must be retained for executable codegen"
        );
        assert!(
            module
                .executable_reachable_globals
                .as_ref()
                .is_some_and(|globals| globals.contains(&o2)),
            "reachable local static should be recorded in executable_reachable_globals: {:?}",
            module.executable_reachable_globals
        );

        let backend = db.query(BackendLoweringQuery);
        let backend_module = backend
            .program
            .modules
            .iter()
            .find(|module| module.id == ModuleId(1))
            .expect("helper backend module");
        assert!(
            backend_module
                .globals
                .iter()
                .any(|global| global.def_id == o2 && global.init.is_some()),
            "reachable cross-module extension method local static must lower as a backend global"
        );
    }

    #[test]
    fn executable_checked_modules_do_not_flow_check_unreachable_functions() {
        let mut loaded = loaded_program_with_modules(vec![loaded_module(
            ModuleId(0),
            "main.nia",
            r#"
fn unused() i32 {
}

fn main() i32 {
    0
}
"#,
        )]);
        loaded.runtime = RuntimeModel::FreestandingExecutable;
        let db = query_db(loaded);

        let modules = db.query(ExecutableCheckedModulesQuery);
        let module = modules
            .iter()
            .find(|module| module.id == ModuleId(0))
            .expect("entry module should be executable-reachable");

        assert!(
            module.flow_check.diagnostics.is_empty(),
            "unreachable function flow diagnostics should not block executable checking: {:?}",
            module.flow_check.diagnostics
        );
    }

    #[test]
    fn executable_checked_modules_do_not_body_check_unreachable_loaded_modules() {
        let entry = loaded_module(
            ModuleId(0),
            "main.nia",
            r#"
pub module unused;

fn main() i32 {
    0
}
"#,
        );
        let unused = loaded_module(
            ModuleId(1),
            "unused.nia",
            r#"
pub fn expensive_or_invalid() i32 {
    missing_symbol
}
"#,
        );
        let mut loaded = loaded_program_with_entry_child(entry, "unused", unused);
        loaded.runtime = RuntimeModel::FreestandingExecutable;
        let db = query_db(loaded);

        let modules = db.query(ExecutableCheckedModulesQuery);
        let trace = db.query_trace();

        assert!(
            modules.iter().all(|module| module.id != ModuleId(1)),
            "unreachable module should not be kept for executable codegen"
        );
        assert!(
            !trace.queries.iter().any(|query| {
                query.frame.name == "body_check"
                    && query.frame.description.contains("ModuleId(1)")
                    && query.stats.executions > 0
            }),
            "unreachable module should not be body-checked: {:?}",
            trace
                .queries
                .iter()
                .filter(|query| query.frame.name == "body_check")
                .collect::<Vec<_>>()
        );
    }

    #[test]
    fn body_check_uses_comptime_semantic_modules_not_ast_module_map() {
        let loaded = loaded_program_with_modules(vec![loaded_module(
            ModuleId(0),
            "main.nia",
            "comptime N: usize = 4; fn main() i32 { let mut values: [N]i32 = [0; N]; values.len() as i32 }",
        )]);
        let db = query_db(loaded);

        let _ = db.query(BodyCheckQuery(ModuleId(0)));
        let trace = db.query_trace();

        assert!(trace.dependencies.iter().any(|dependency| {
            dependency.from.name == "body_check" && dependency.to.name == "comptime_module"
        }));
        assert!(trace.dependencies.iter().any(|dependency| {
            dependency.from.name == "body_check" && dependency.to.name == "comptime_values"
        }));
        assert!(trace.dependencies.iter().any(|dependency| {
            dependency.from.name == "body_check" && dependency.to.name == "comptime_array_lengths"
        }));
        assert!(trace.dependencies.iter().any(|dependency| {
            dependency.from.name == "body_check"
                && dependency.to.name == "full_active_module_item_tree"
        }));
        assert!(!trace.dependencies.iter().any(|dependency| {
            dependency.from.name == "body_check" && dependency.to.name == "comptime"
        }));
        assert!(
            !trace
                .dependencies
                .iter()
                .any(|dependency| dependency.to.name == "program_comptime_modules")
        );
        assert!(
            !trace
                .dependencies
                .iter()
                .any(|dependency| dependency.to.name == "program_modules_by_id")
        );
        assert!(
            !trace
                .dependencies
                .iter()
                .any(|dependency| dependency.to.name == "program_item_signatures")
        );
    }

    #[test]
    fn invalidates_semantic_queries_after_public_surface_dependency_changes() {
        let loaded = loaded_program_with_modules(vec![loaded_module(
            ModuleId(0),
            "main.nia",
            "pub struct S { value: i32 } fn main() i32 { 0 }",
        )]);
        let db = query_db(loaded);

        let _ = db.query(TypeResolutionQuery(ModuleId(0)));
        let invalidation = db.invalidate(ModuleDefsQuery(ModuleId(0)));
        let invalidated = invalidation
            .invalidated
            .iter()
            .map(|frame| frame.name)
            .collect::<Vec<_>>();

        assert!(invalidated.contains(&"module_defs"), "{invalidated:?}");
        assert!(invalidated.contains(&"defs_by_module"), "{invalidated:?}");
        assert!(invalidated.contains(&"public_surface"), "{invalidated:?}");
        assert!(invalidated.contains(&"type_resolution"), "{invalidated:?}");

        let _ = db.query(TypeResolutionQuery(ModuleId(0)));
    }

    #[test]
    fn invalidates_module_defs_after_item_tree_changes() {
        let loaded = loaded_program_with_modules(vec![loaded_module(
            ModuleId(0),
            "main.nia",
            "pub struct S { value: i32 }",
        )]);
        let db = query_db(loaded);

        let _ = db.query(ModuleDefsQuery(ModuleId(0)));
        let invalidation = db.invalidate(ModuleItemTreeInputQuery(ModuleId(0)));
        let invalidated = invalidation
            .invalidated
            .iter()
            .map(|frame| frame.name)
            .collect::<Vec<_>>();

        assert!(
            invalidated.contains(&"module_item_tree_input"),
            "{invalidated:?}"
        );
        assert!(invalidated.contains(&"module_item_tree"), "{invalidated:?}");
        assert!(
            invalidated.contains(&"active_module_item_tree"),
            "{invalidated:?}"
        );
        assert!(invalidated.contains(&"module_defs"), "{invalidated:?}");
    }
}
