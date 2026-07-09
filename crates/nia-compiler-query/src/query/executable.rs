// SPDX-License-Identifier: GPL-3.0-or-later
use super::*;
use nia_defs::DefKind;
use nia_executable_facts::{ExecutableModuleRefs, ReachableModuleInput};
use std::cell::RefCell;
use std::collections::{HashMap, HashSet};

pub(super) struct ExecutableCheckCaches {
    pub(super) array_lengths: RefCell<HashMap<ModuleId, nia_comptime_check::ComptimeArrayLengths>>,
    pub(super) body_resolution_inputs: RefCell<HashMap<ModuleId, BodyCheckResolutionInputs>>,
    pub(super) reachability_function_signatures:
        RefCell<HashMap<GlobalDefId, std::sync::Arc<ProgramFunctionSignature>>>,
    pub(super) body_function_signatures: RefCell<HashMap<GlobalDefId, ProgramFunctionSignature>>,
    pub(super) global_initializers:
        RefCell<HashMap<GlobalDefId, Option<nia_comptime_ir::ResolvedComptimeExpr>>>,
}

impl Default for ExecutableCheckCaches {
    fn default() -> Self {
        Self {
            array_lengths: RefCell::new(HashMap::new()),
            body_resolution_inputs: RefCell::new(HashMap::new()),
            reachability_function_signatures: RefCell::new(HashMap::new()),
            body_function_signatures: RefCell::new(HashMap::new()),
            global_initializers: RefCell::new(HashMap::new()),
        }
    }
}

#[derive(Clone)]
pub(super) struct ExecutableFactModuleState {
    pub(super) module_id: ModuleId,
    pub(super) defs: DefCollection,
    pub(super) body_ir: nia_body_ir::BodyIr,
    pub(super) semantic_facts: nia_sema_ir::SemanticFacts,
    pub(super) type_lowering: nia_type_lower::TypeLowering,
    pub(super) type_normalization: nia_type_normalize::TypeNormalization,
    pub(super) executable_refs: ExecutableModuleRefs,
    pub(super) checked_functions: HashSet<GlobalDefId>,
    pub(super) checked_globals: HashSet<GlobalDefId>,
}

impl ExecutableFactModuleState {
    pub(super) fn new(
        db: &QueryDb<CompilerContext>,
        module_id: ModuleId,
        body_check: BodyCheckWithResolutionInputs,
        checked_globals: HashSet<GlobalDefId>,
    ) -> Self {
        let BodyCheckWithResolutionInputs {
            body_check,
            inputs: _,
            comptime: _,
        } = body_check;
        let nia_body_check::BodyCheck {
            ir,
            facts,
            checked_functions,
            diagnostics: _,
        } = body_check;
        let mut state = Self {
            module_id,
            defs: db.query(FullModuleDefsQuery(module_id)),
            body_ir: ir,
            semantic_facts: facts,
            type_lowering: db.query(TypeLoweringQuery(module_id)),
            type_normalization: db.query(TypeNormalizationQuery(module_id)),
            executable_refs: ExecutableModuleRefs::default(),
            checked_functions,
            checked_globals,
        };
        state.executable_refs = executable_module_refs_for_fact_state(&state);
        state
    }

    pub(super) fn reachable_input(&self) -> ReachableModuleInput<'_> {
        ReachableModuleInput {
            module_id: self.module_id,
            defs: &self.defs,
            body_ir: &self.body_ir,
            executable_refs: &self.executable_refs,
            semantic_facts: &self.semantic_facts,
            type_lowering: &self.type_lowering,
            type_normalization: &self.type_normalization,
        }
    }

    pub(super) fn extend(
        &mut self,
        increment: BodyCheckWithResolutionInputs,
        checked_globals: HashSet<GlobalDefId>,
    ) {
        let BodyCheckWithResolutionInputs {
            body_check,
            inputs: _,
            comptime: _,
        } = increment;
        let nia_body_check::BodyCheck {
            mut ir,
            facts,
            checked_functions,
            diagnostics: _,
        } = body_check;
        let executable_refs = executable_module_refs_for_increment(
            self.module_id,
            &self.defs,
            &ir,
            &facts,
            &self.type_lowering,
            &self.type_normalization,
        );
        merge_executable_interner_snapshot(&mut self.body_ir.interner, ir.interner, "fact");
        self.body_ir
            .function_bodies
            .extend(ir.function_bodies.drain());
        self.body_ir.global_inits.extend(ir.global_inits.drain());
        self.semantic_facts.extend(facts);
        self.executable_refs.extend(executable_refs);
        self.checked_functions.extend(checked_functions);
        self.checked_globals.extend(checked_globals);
    }
}

fn executable_module_refs_for_fact_state(
    state: &ExecutableFactModuleState,
) -> ExecutableModuleRefs {
    let input = state.reachable_input();
    let mut refs = nia_executable_facts::executable_module_refs_from_typed_ir(&input);
    refs.extend(nia_executable_facts::executable_module_refs_from_semantic_facts(&input));
    refs
}

fn executable_module_refs_for_increment(
    module_id: ModuleId,
    defs: &DefCollection,
    body_ir: &nia_body_ir::BodyIr,
    semantic_facts: &nia_sema_ir::SemanticFacts,
    type_lowering: &nia_type_lower::TypeLowering,
    type_normalization: &nia_type_normalize::TypeNormalization,
) -> ExecutableModuleRefs {
    let empty_refs = ExecutableModuleRefs::default();
    let input = ReachableModuleInput {
        module_id,
        defs,
        body_ir,
        executable_refs: &empty_refs,
        semantic_facts,
        type_lowering,
        type_normalization,
    };
    let mut refs = nia_executable_facts::executable_module_refs_from_typed_ir(&input);
    refs.extend(nia_executable_facts::executable_module_refs_from_semantic_facts(&input));
    refs
}

pub(super) fn reachable_fact_module_inputs(
    fact_by_id: &HashMap<ModuleId, ExecutableFactModuleState>,
) -> Vec<ReachableModuleInput<'_>> {
    fact_by_id
        .values()
        .map(ExecutableFactModuleState::reachable_input)
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
    fact_by_id: &HashMap<ModuleId, ExecutableFactModuleState>,
) -> std::collections::VecDeque<ModuleId> {
    parse_ok
        .iter()
        .copied()
        .filter(|module_id| reachability.modules.contains(module_id))
        .filter(|module_id| executable_module_has_pending_body_items(db, *module_id, reachability))
        .filter(|module_id| executable_fact_module_is_stale(*module_id, reachability, fact_by_id))
        .collect()
}

pub(super) fn executable_fact_module_is_stale(
    module_id: ModuleId,
    reachability: &nia_executable_reachability::ExecutableReachability,
    fact_by_id: &HashMap<ModuleId, ExecutableFactModuleState>,
) -> bool {
    match fact_by_id.get(&module_id) {
        Some(state) => {
            reachability.functions.iter().any(|def_id| {
                def_id.module_id == module_id && !state.checked_functions.contains(def_id)
            }) || reachability.globals.iter().any(|def_id| {
                def_id.module_id == module_id && !state.checked_globals.contains(def_id)
            })
        }
        None => true,
    }
}

pub(super) fn executable_module_has_pending_body_items(
    db: &QueryDb<CompilerContext>,
    module_id: ModuleId,
    reachability: &nia_executable_reachability::ExecutableReachability,
) -> bool {
    reachability
        .functions
        .iter()
        .any(|def_id| def_id.module_id == module_id)
        || reachability.globals.iter().any(|def_id| {
            def_id.module_id == module_id
                && db
                    .query_shared(ModuleDefsQuery(def_id.module_id))
                    .defs
                    .get(def_id.def_id)
                    .is_some_and(|def| def.kind == DefKind::Global)
        })
}

pub(super) fn debug_executable_reachability_enabled() -> bool {
    std::env::var_os("NIA_DEBUG_EXEC_REACHABILITY").is_some()
}

pub(super) struct ExecutableRoundDebug<'a> {
    pub(super) module_id: ModuleId,
    pub(super) module_path: &'a SourcePath,
    pub(super) requested_function_names: Vec<String>,
    pub(super) checked_function_names: Vec<String>,
    pub(super) requested_functions: usize,
    pub(super) new_functions: usize,
    pub(super) new_globals: usize,
    pub(super) checked_functions_total: usize,
    pub(super) checked_globals_total: usize,
    pub(super) reachable_functions_total: usize,
    pub(super) reachable_globals_total: usize,
    pub(super) reachable_modules_total: usize,
    pub(super) type_modules_total: usize,
}

pub(super) fn print_executable_round_debug(info: ExecutableRoundDebug<'_>) {
    if !debug_executable_reachability_enabled() {
        return;
    }
    eprintln!(
        "debug executable_reachability module={:?} path={} requested_functions={} new_functions={} new_globals={} checked_functions={} checked_globals={} reachable_functions={} reachable_globals={} reachable_modules={} type_modules={}",
        info.module_id,
        info.module_path.as_str(),
        info.requested_functions,
        info.new_functions,
        info.new_globals,
        info.checked_functions_total,
        info.checked_globals_total,
        info.reachable_functions_total,
        info.reachable_globals_total,
        info.reachable_modules_total,
        info.type_modules_total,
    );
    if !info.requested_function_names.is_empty() || !info.checked_function_names.is_empty() {
        eprintln!(
            "debug executable_reachability.functions module={:?} requested=[{}] checked=[{}]",
            info.module_id,
            info.requested_function_names.join(", "),
            info.checked_function_names.join(", "),
        );
    }
}

fn merge_executable_interner_snapshot(
    current: &mut nia_ty::TyInterner,
    increment: nia_ty::TyInterner,
    source: &str,
) {
    if current.interner_id() != increment.interner_id() {
        *current = increment;
    } else if current.is_prefix_of(&increment) {
        *current = increment;
    } else if increment.is_prefix_of(current) {
    } else {
        panic!(
            "Nia ICE: executable {source} type interner snapshots share id {:?} but are not prefix-compatible",
            current.interner_id()
        );
    }
}
