// SPDX-License-Identifier: GPL-3.0-or-later
use super::*;
use crate::ProviderDemand;
use nia_defs::DefKind;
use nia_executable_facts::{ExecutableModuleRefs, ReachableModuleInput};
use std::cell::RefCell;
use std::collections::{HashMap, HashSet};

pub(super) struct ExecutableCheckCaches {
    pub(super) array_lengths: RefCell<HashMap<ModuleId, nia_const_check::ConstArrayLengths>>,
    pub(super) body_resolution_inputs: RefCell<HashMap<ModuleId, BodyCheckResolutionInputs>>,
    pub(super) reachability_function_signatures:
        RefCell<HashMap<GlobalDefId, std::sync::Arc<ProgramFunctionSignature>>>,
    pub(super) body_function_signatures: RefCell<HashMap<GlobalDefId, ProgramFunctionSignature>>,
    pub(super) global_initializers:
        RefCell<HashMap<GlobalDefId, Option<nia_const_ir::ResolvedConstExpr>>>,
    pub(super) const_modules: RefCell<HashMap<ModuleId, ConstModuleLowering>>,
}

impl Default for ExecutableCheckCaches {
    fn default() -> Self {
        Self {
            array_lengths: RefCell::new(HashMap::new()),
            body_resolution_inputs: RefCell::new(HashMap::new()),
            reachability_function_signatures: RefCell::new(HashMap::new()),
            body_function_signatures: RefCell::new(HashMap::new()),
            global_initializers: RefCell::new(HashMap::new()),
            const_modules: RefCell::new(HashMap::new()),
        }
    }
}

impl ExecutableCheckCaches {
    fn retain_modules(&mut self, modules: &HashSet<ModuleId>) {
        self.array_lengths
            .get_mut()
            .retain(|module_id, _| modules.contains(module_id));
        self.body_resolution_inputs
            .get_mut()
            .retain(|module_id, _| modules.contains(module_id));
        self.reachability_function_signatures
            .get_mut()
            .retain(|def_id, _| modules.contains(&def_id.module_id));
        self.body_function_signatures
            .get_mut()
            .retain(|def_id, _| modules.contains(&def_id.module_id));
        self.global_initializers
            .get_mut()
            .retain(|def_id, _| modules.contains(&def_id.module_id));
        self.const_modules
            .get_mut()
            .retain(|module_id, _| modules.contains(module_id));
    }
}

#[derive(Clone)]
pub(super) struct ExecutableFactModuleState {
    pub(super) module_id: ModuleId,
    pub(super) defs: Arc<DefCollection>,
    pub(super) body_ir: nia_body_ir::BodyIr,
    pub(super) static_init_refs: HashMap<GlobalDefId, nia_static_ir::StaticInitRefs>,
    pub(super) semantic_facts: nia_sema_ir::SemanticFacts,
    pub(super) provider_demands: HashSet<ProviderDemand>,
    pub(super) provider_demands_by_function: HashMap<GlobalDefId, HashSet<ProviderDemand>>,
    pub(super) unowned_provider_demands: HashSet<ProviderDemand>,
    pub(super) executable_refs: ExecutableModuleRefs,
    pub(super) checked_functions: HashSet<GlobalDefId>,
    pub(super) checked_globals: HashSet<GlobalDefId>,
    pub(super) diagnostic_owners: Vec<Option<GlobalDefId>>,
    pub(super) diagnostics: Vec<nia_diagnostic::Diagnostic>,
}

#[derive(Default)]
pub(super) struct ExecutableFactSession {
    pub(super) epoch: Option<ExecutableFactEpoch>,
    pub(super) modules: HashMap<ModuleId, ExecutableFactModuleState>,
    pub(super) reachability: nia_executable_reachability::IncrementalExecutableReachability,
    pub(super) caches: ExecutableCheckCaches,
    pub(super) applied_provider_fact_revision: Option<crate::ProviderFactRevision>,
    pub(super) applied_provider_changes: HashSet<ProviderDemand>,
    pub(super) applied_body_activations: HashSet<nia_imports::StableModuleKey>,
}

impl ExecutableFactSession {
    pub(super) fn enter_epoch(&mut self, epoch: &ExecutableFactEpoch) {
        if self.epoch.as_ref() == Some(epoch) {
            return;
        }
        *self = Self {
            epoch: Some(epoch.clone()),
            ..Self::default()
        };
    }

    pub(super) fn apply_body_activation_worklist(&mut self, worklist: &BodyActivationWorklist) {
        let pending_activations = worklist
            .modules
            .iter()
            .filter(|(stable_key, _)| !self.applied_body_activations.contains(*stable_key))
            .map(|(stable_key, module_id)| (stable_key.clone(), *module_id))
            .collect::<Vec<_>>();
        if pending_activations.is_empty() {
            return;
        }
        let pending_module_ids = pending_activations
            .iter()
            .map(|(_, module_id)| *module_id)
            .collect::<HashSet<_>>();
        self.reachability = Default::default();
        let mut retained_modules = HashSet::new();
        self.modules.retain(|module_id, _| {
            let retained = !pending_module_ids.contains(module_id);
            if retained {
                retained_modules.insert(*module_id);
            }
            retained
        });
        self.caches.retain_modules(&retained_modules);
        self.applied_body_activations.extend(
            pending_activations
                .into_iter()
                .map(|(stable_key, _)| stable_key),
        );
    }

    pub(super) fn apply_provider_fact_worklist(
        &mut self,
        worklist: &crate::ProviderFactSnapshot,
        type_store: &nia_ty::TypeStore,
    ) {
        if self.applied_provider_fact_revision == Some(worklist.revision()) {
            return;
        }
        let reset = self
            .applied_provider_fact_revision
            .is_some_and(|previous| worklist.reset_revision().is_newer_than(previous));
        if reset {
            let epoch = self.epoch.clone();
            *self = Self {
                epoch,
                ..Self::default()
            };
        }
        let pending_changes = worklist
            .demands()
            .difference(&self.applied_provider_changes)
            .cloned()
            .collect::<HashSet<_>>();
        if !pending_changes.is_empty() {
            self.reachability = Default::default();
            let mut retained_modules = HashSet::new();
            self.modules.retain(|module_id, state| {
                let retained = state.invalidate_provider_changes(&pending_changes, type_store);
                if retained {
                    retained_modules.insert(*module_id);
                }
                retained
            });
            self.caches.retain_modules(&retained_modules);
            self.applied_provider_changes.extend(pending_changes);
        }
        self.applied_provider_fact_revision = Some(worklist.revision());
    }
}

impl ExecutableFactModuleState {
    pub(super) fn invalidate_provider_changes(
        &mut self,
        provider_changes: &HashSet<ProviderDemand>,
        type_store: &nia_ty::TypeStore,
    ) -> bool {
        if self.unowned_provider_demands.iter().any(|demand| {
            provider_change_invalidates_facts(demand) && provider_changes.contains(demand)
        }) {
            return false;
        }
        let invalidated_functions = self
            .provider_demands_by_function
            .iter()
            .filter(|(_, demands)| {
                demands.iter().any(|demand| {
                    provider_change_invalidates_facts(demand) && provider_changes.contains(demand)
                })
            })
            .map(|(function, _)| *function)
            .collect::<HashSet<_>>();
        if invalidated_functions.is_empty() {
            return true;
        }
        for function in &invalidated_functions {
            self.body_ir.function_bodies.remove(function);
            self.semantic_facts.function_facts.remove(function);
            self.checked_functions.remove(function);
            self.provider_demands_by_function.remove(function);
        }
        let mut diagnostic_index = 0usize;
        self.diagnostics.retain(|_| {
            let retain = self
                .diagnostic_owners
                .get(diagnostic_index)
                .copied()
                .flatten()
                .is_none_or(|owner| !invalidated_functions.contains(&owner));
            diagnostic_index += 1;
            retain
        });
        self.diagnostic_owners
            .retain(|owner| owner.is_none_or(|owner| !invalidated_functions.contains(&owner)));
        let local_statics =
            self.defs
                .defs
                .iter()
                .filter_map(|(def_id, def)| {
                    let parent = def.parent.map(|parent| GlobalDefId {
                        module_id: self.module_id,
                        def_id: parent,
                    })?;
                    (def.kind == DefKind::Global && invalidated_functions.contains(&parent))
                        .then_some(GlobalDefId {
                            module_id: self.module_id,
                            def_id,
                        })
                })
                .collect::<Vec<_>>();
        for global in local_statics {
            self.body_ir.global_inits.remove(&global);
            self.static_init_refs.remove(&global);
            self.checked_globals.remove(&global);
        }
        self.rebuild_provider_demands();
        self.executable_refs = executable_module_refs_for_fact_state(self, type_store);
        true
    }

    fn rebuild_provider_demands(&mut self) {
        self.provider_demands = self.unowned_provider_demands.clone();
        self.provider_demands.extend(
            self.provider_demands_by_function
                .values()
                .flat_map(|demands| demands.iter().cloned()),
        );
    }

    pub(super) fn new(
        db: &QueryDb<CompilerContext>,
        module_id: ModuleId,
        body_check: BodyCheckWithResolutionInputs,
        checked_globals: HashSet<GlobalDefId>,
    ) -> QueryResult<Self> {
        let BodyCheckWithResolutionInputs {
            body_check,
            inputs: _,
            const_eval: _,
        } = body_check;
        let nia_body_check::BodyCheck {
            ir,
            facts,
            static_init_refs,
            provider_demands,
            provider_demands_by_function,
            checked_functions,
            diagnostic_owners,
            diagnostics,
        } = body_check;
        let owned_provider_demands = provider_demands_by_function
            .values()
            .flat_map(|demands| demands.iter().cloned())
            .collect::<HashSet<_>>();
        let unowned_provider_demands = provider_demands
            .difference(&owned_provider_demands)
            .cloned()
            .collect();
        let mut state = Self {
            module_id,
            defs: full_module_defs_semantic(db, module_id)?,
            body_ir: Arc::unwrap_or_clone(ir),
            static_init_refs,
            semantic_facts: Arc::unwrap_or_clone(facts),
            provider_demands: Arc::unwrap_or_clone(provider_demands),
            provider_demands_by_function,
            unowned_provider_demands,
            executable_refs: ExecutableModuleRefs::default(),
            checked_functions,
            checked_globals,
            diagnostic_owners,
            diagnostics: Arc::unwrap_or_clone(diagnostics),
        };
        state.executable_refs =
            executable_module_refs_for_fact_state(&state, &db.context().type_store);
        Ok(state)
    }

    pub(super) fn reachable_input<'a>(
        &'a self,
        type_store: &'a nia_ty::TypeStore,
    ) -> ReachableModuleInput<'a> {
        ReachableModuleInput {
            module_id: self.module_id,
            defs: &self.defs,
            type_store,
            body_ir: &self.body_ir,
            executable_refs: &self.executable_refs,
            semantic_facts: &self.semantic_facts,
        }
    }

    pub(super) fn extend(
        &mut self,
        increment: BodyCheckWithResolutionInputs,
        checked_globals: HashSet<GlobalDefId>,
        type_store: &nia_ty::TypeStore,
    ) {
        let BodyCheckWithResolutionInputs {
            body_check,
            inputs: _,
            const_eval: _,
        } = increment;
        let nia_body_check::BodyCheck {
            ir,
            facts,
            static_init_refs,
            provider_demands,
            provider_demands_by_function,
            checked_functions,
            diagnostic_owners,
            diagnostics,
        } = body_check;
        let mut ir = Arc::unwrap_or_clone(ir);
        let facts = Arc::unwrap_or_clone(facts);
        let executable_refs = executable_module_refs_for_increment(
            self.module_id,
            &self.defs,
            type_store,
            &ir,
            &facts,
            &static_init_refs,
        );
        self.body_ir
            .function_bodies
            .extend(ir.function_bodies.drain());
        debug_assert!(ir.global_inits.is_empty());
        self.static_init_refs.extend(static_init_refs);
        self.semantic_facts.extend(facts);
        let owned_provider_demands = provider_demands_by_function
            .values()
            .flat_map(|demands| demands.iter().cloned())
            .collect::<HashSet<_>>();
        self.unowned_provider_demands.extend(
            Arc::unwrap_or_clone(provider_demands)
                .difference(&owned_provider_demands)
                .cloned(),
        );
        for (function, demands) in provider_demands_by_function {
            self.provider_demands_by_function
                .entry(function)
                .or_default()
                .extend(demands);
        }
        self.diagnostic_owners.extend(diagnostic_owners);
        self.rebuild_provider_demands();
        self.executable_refs.extend(executable_refs);
        self.checked_functions.extend(checked_functions);
        self.checked_globals.extend(checked_globals);
        self.diagnostics.extend(Arc::unwrap_or_clone(diagnostics));
    }
}

fn provider_change_invalidates_facts(demand: &ProviderDemand) -> bool {
    demand.request.invalidates_resolved_body_facts()
}

pub(super) fn unchecked_executable_items(
    reachability_by_module: &nia_executable_reachability::ExecutableReachabilityByModule,
    module_id: ModuleId,
    fact_by_id: &HashMap<ModuleId, ExecutableFactModuleState>,
) -> (HashSet<GlobalDefId>, HashSet<GlobalDefId>) {
    let Some(items) = reachability_by_module.get(module_id) else {
        return (HashSet::new(), HashSet::new());
    };
    let already_checked_functions = fact_by_id
        .get(&module_id)
        .map(|state| &state.checked_functions);
    let already_checked_globals = fact_by_id
        .get(&module_id)
        .map(|state| &state.checked_globals);
    let functions = items
        .functions
        .iter()
        .copied()
        .filter(|def_id| already_checked_functions.is_none_or(|checked| !checked.contains(def_id)))
        .collect();
    let globals = items
        .globals
        .iter()
        .copied()
        .filter(|def_id| already_checked_globals.is_none_or(|checked| !checked.contains(def_id)))
        .collect();
    (functions, globals)
}

pub(super) fn executable_reachability_has_pending_body_items(
    db: &QueryDb<CompilerContext>,
    reachability_by_module: &nia_executable_reachability::ExecutableReachabilityByModule,
    module_id: ModuleId,
) -> QueryResult<bool> {
    let query_failure = RefCell::new(None);
    let has_pending = reachability_by_module.get(module_id).is_some_and(|items| {
        items.has_body_items(|def_id| match is_runtime_global_def(db, def_id) {
            Ok(is_runtime) => is_runtime,
            Err(error) => {
                let mut failure = query_failure.borrow_mut();
                if failure.is_none() {
                    *failure = Some(error);
                }
                false
            }
        })
    });
    match query_failure.into_inner() {
        Some(error) => Err(error),
        None => Ok(has_pending),
    }
}

pub(super) fn executable_reachable_body_modules(
    db: &QueryDb<CompilerContext>,
    reachability_by_module: &nia_executable_reachability::ExecutableReachabilityByModule,
) -> QueryResult<HashSet<ModuleId>> {
    let query_failure = RefCell::new(None);
    let modules =
        reachability_by_module.reachable_body_modules(|def_id| {
            match is_runtime_global_def(db, def_id) {
                Ok(is_runtime) => is_runtime,
                Err(error) => {
                    let mut failure = query_failure.borrow_mut();
                    if failure.is_none() {
                        *failure = Some(error);
                    }
                    false
                }
            }
        });
    match query_failure.into_inner() {
        Some(error) => Err(error),
        None => Ok(modules),
    }
}

pub(super) fn is_runtime_global_def(
    db: &QueryDb<CompilerContext>,
    def_id: GlobalDefId,
) -> QueryResult<bool> {
    Ok(module_defs_semantic(db, def_id.module_id)?
        .defs
        .get(def_id.def_id)
        .is_some_and(|def| def.kind == DefKind::Global))
}

fn executable_module_refs_for_fact_state(
    state: &ExecutableFactModuleState,
    type_store: &nia_ty::TypeStore,
) -> ExecutableModuleRefs {
    let input = state.reachable_input(type_store);
    let mut refs = nia_executable_facts::executable_module_refs_from_typed_ir(&input);
    refs.extend(nia_executable_facts::executable_module_refs_from_semantic_facts(&input));
    refs.extend(executable_module_refs_from_static_init_refs(
        &state.static_init_refs,
    ));
    refs
}

fn executable_module_refs_for_increment(
    module_id: ModuleId,
    defs: &DefCollection,
    type_store: &nia_ty::TypeStore,
    body_ir: &nia_body_ir::BodyIr,
    semantic_facts: &nia_sema_ir::SemanticFacts,
    static_init_refs: &HashMap<GlobalDefId, nia_static_ir::StaticInitRefs>,
) -> ExecutableModuleRefs {
    let empty_refs = ExecutableModuleRefs::default();
    let input = ReachableModuleInput {
        module_id,
        defs,
        type_store,
        body_ir,
        executable_refs: &empty_refs,
        semantic_facts,
    };
    let mut refs = nia_executable_facts::executable_module_refs_from_typed_ir(&input);
    refs.extend(nia_executable_facts::executable_module_refs_from_semantic_facts(&input));
    refs.extend(executable_module_refs_from_static_init_refs(
        static_init_refs,
    ));
    refs
}

fn executable_module_refs_from_static_init_refs(
    refs_by_global: &HashMap<GlobalDefId, nia_static_ir::StaticInitRefs>,
) -> ExecutableModuleRefs {
    ExecutableModuleRefs {
        functions: HashMap::new(),
        globals: refs_by_global
            .iter()
            .map(|(def_id, refs)| {
                (
                    *def_id,
                    nia_executable_facts::ExecutableItemRefs {
                        functions: refs.functions.clone(),
                        globals: refs.globals.clone(),
                        ..nia_executable_facts::ExecutableItemRefs::default()
                    },
                )
            })
            .collect(),
    }
}

pub(super) fn reachable_fact_module_inputs<'a>(
    fact_by_id: &'a HashMap<ModuleId, ExecutableFactModuleState>,
    type_store: &'a nia_ty::TypeStore,
) -> Vec<ReachableModuleInput<'a>> {
    fact_by_id
        .values()
        .map(|state| state.reachable_input(type_store))
        .collect()
}

pub(super) fn reachable_module_inputs_by_id<'a>(
    inputs: &'a [ReachableModuleInput<'a>],
) -> HashMap<ModuleId, ReachableModuleInput<'a>> {
    inputs
        .iter()
        .copied()
        .map(|input| (input.module_id, input))
        .collect()
}

pub(super) fn stale_executable_fact_modules(
    db: &QueryDb<CompilerContext>,
    parse_ok: &[ModuleId],
    reachability: &nia_executable_reachability::ExecutableReachability,
    reachability_by_module: &nia_executable_reachability::ExecutableReachabilityByModule,
    fact_by_id: &HashMap<ModuleId, ExecutableFactModuleState>,
) -> QueryResult<std::collections::VecDeque<ModuleId>> {
    let mut stale = std::collections::VecDeque::new();
    for module_id in parse_ok
        .iter()
        .copied()
        .filter(|module_id| reachability.modules().contains(module_id))
    {
        if executable_reachability_has_pending_body_items(db, reachability_by_module, module_id)?
            && executable_fact_module_is_stale(module_id, reachability_by_module, fact_by_id)
        {
            stale.push_back(module_id);
        }
    }
    Ok(stale)
}

fn executable_fact_module_is_stale(
    module_id: ModuleId,
    reachability_by_module: &nia_executable_reachability::ExecutableReachabilityByModule,
    fact_by_id: &HashMap<ModuleId, ExecutableFactModuleState>,
) -> bool {
    let Some(items) = reachability_by_module.get(module_id) else {
        return false;
    };
    match fact_by_id.get(&module_id) {
        Some(state) => {
            items
                .functions
                .iter()
                .any(|def_id| !state.checked_functions.contains(def_id))
                || items
                    .globals
                    .iter()
                    .any(|def_id| !state.checked_globals.contains(def_id))
        }
        None => true,
    }
}
