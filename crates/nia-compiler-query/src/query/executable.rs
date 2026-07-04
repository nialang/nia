// SPDX-License-Identifier: GPL-3.0-or-later
use super::*;
use nia_defs::DefKind;
use nia_executable_facts::{ExecutableModuleBodyRefs, ReachableModuleInput};
use std::cell::RefCell;
use std::collections::{HashMap, HashSet};

pub(super) struct ExecutableCheckCaches {
    pub(super) array_lengths: RefCell<HashMap<ModuleId, nia_comptime_check::ComptimeArrayLengths>>,
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
            reachability_function_signatures: RefCell::new(HashMap::new()),
            body_function_signatures: RefCell::new(HashMap::new()),
            global_initializers: RefCell::new(HashMap::new()),
        }
    }
}

#[derive(Clone)]
pub(super) struct ExecutableCheckedModuleState {
    pub(super) module: CheckedModule,
    pub(super) body_refs: ExecutableModuleBodyRefs,
    pub(super) checked_functions: HashSet<GlobalDefId>,
    pub(super) checked_globals: HashSet<GlobalDefId>,
}

impl ExecutableCheckedModuleState {
    pub(super) fn new(
        module: CheckedModule,
        checked_functions: HashSet<GlobalDefId>,
        checked_globals: HashSet<GlobalDefId>,
    ) -> Self {
        let body_refs = executable_body_refs_for_checked_module(&module);
        Self {
            module,
            body_refs,
            checked_functions,
            checked_globals,
        }
    }

    pub(super) fn reachable_input(&self) -> ReachableModuleInput<'_> {
        ReachableModuleInput {
            module_id: self.module.id,
            defs: &self.module.defs,
            body_ir: &self.module.body_ir,
            body_refs: &self.body_refs,
            semantic_facts: &self.module.semantic_facts,
            type_lowering: &self.module.type_lowering,
            type_normalization: &self.module.type_normalization,
        }
    }

    pub(super) fn extend(
        &mut self,
        increment: CheckedModule,
        checked_functions: HashSet<GlobalDefId>,
        checked_globals: HashSet<GlobalDefId>,
    ) {
        self.body_refs
            .extend(executable_body_refs_for_checked_module(&increment));
        self.module
            .value_resolution
            .node_names
            .extend(increment.value_resolution.node_names);
        self.module
            .value_resolution
            .node_qualified_values
            .extend(increment.value_resolution.node_qualified_values);
        self.module
            .value_resolution
            .node_builtin_associated_values
            .extend(increment.value_resolution.node_builtin_associated_values);
        self.module
            .value_resolution
            .node_variant_enums
            .extend(increment.value_resolution.node_variant_enums);
        self.module
            .value_resolution
            .node_qualified_type_prefixes
            .extend(increment.value_resolution.node_qualified_type_prefixes);
        self.module
            .value_resolution
            .diagnostics
            .extend(increment.value_resolution.diagnostics);

        self.module
            .local_resolution
            .node_local_defs
            .extend(increment.local_resolution.node_local_defs);
        self.module
            .local_resolution
            .node_uses
            .extend(increment.local_resolution.node_uses);
        self.module
            .local_resolution
            .diagnostics
            .extend(increment.local_resolution.diagnostics);

        self.module
            .semantic_uses
            .node_value_uses
            .extend(increment.semantic_uses.node_value_uses);
        self.module
            .semantic_uses
            .node_const_generic_uses
            .extend(increment.semantic_uses.node_const_generic_uses);
        self.module
            .semantic_uses
            .node_builtin_associated_values
            .extend(increment.semantic_uses.node_builtin_associated_values);
        self.module
            .semantic_uses
            .node_associated_comptime_projections
            .extend(increment.semantic_uses.node_associated_comptime_projections);
        self.module
            .semantic_uses
            .node_local_defs
            .extend(increment.semantic_uses.node_local_defs);
        self.module
            .semantic_uses
            .node_type_uses
            .extend(increment.semantic_uses.node_type_uses);

        merge_executable_interner_snapshot(
            &mut self.module.comptime.interner,
            increment.comptime.interner,
            "comptime",
        );
        self.module
            .comptime
            .values
            .extend(increment.comptime.values);
        self.module
            .comptime
            .typed_values
            .extend(increment.comptime.typed_values);
        self.module
            .comptime
            .enum_values
            .extend(increment.comptime.enum_values);
        self.module
            .comptime
            .typed_enum_values
            .extend(increment.comptime.typed_enum_values);
        self.module
            .comptime
            .array_lengths
            .extend(increment.comptime.array_lengths);
        self.module
            .comptime
            .diagnostics
            .extend(increment.comptime.diagnostics);

        merge_executable_interner_snapshot(
            &mut self.module.body_ir.interner,
            increment.body_ir.interner,
            "body",
        );
        self.module
            .body_ir
            .function_bodies
            .extend(increment.body_ir.function_bodies);
        self.module
            .body_ir
            .global_inits
            .extend(increment.body_ir.global_inits);
        self.module
            .semantic_facts
            .global_types
            .extend(increment.semantic_facts.global_types);
        self.module
            .semantic_facts
            .generic_instantiations
            .extend(increment.semantic_facts.generic_instantiations);
        self.module
            .semantic_facts
            .function_facts
            .extend(increment.semantic_facts.function_facts);
        self.module
            .semantic_facts
            .node_expr_types
            .extend(increment.semantic_facts.node_expr_types);
        self.module
            .semantic_facts
            .node_bracket_suffix_resolutions
            .extend(increment.semantic_facts.node_bracket_suffix_resolutions);
        self.module
            .semantic_facts
            .node_pointer_array_to_slice_coercions
            .extend(
                increment
                    .semantic_facts
                    .node_pointer_array_to_slice_coercions,
            );
        self.module
            .semantic_facts
            .node_trait_object_coercions
            .extend(increment.semantic_facts.node_trait_object_coercions);
        self.module
            .semantic_facts
            .node_trait_object_upcasts
            .extend(increment.semantic_facts.node_trait_object_upcasts);
        self.module
            .semantic_facts
            .node_builtin_values
            .extend(increment.semantic_facts.node_builtin_values);
        self.module
            .semantic_facts
            .node_builtin_associated_values
            .extend(increment.semantic_facts.node_builtin_associated_values);
        self.module
            .semantic_facts
            .node_associated_comptime_projections
            .extend(
                increment
                    .semantic_facts
                    .node_associated_comptime_projections,
            );
        self.module
            .semantic_facts
            .node_array_repeat_counts
            .extend(increment.semantic_facts.node_array_repeat_counts);
        self.module
            .semantic_facts
            .node_switch_pattern_values
            .extend(increment.semantic_facts.node_switch_pattern_values);
        self.module
            .semantic_facts
            .node_resolved_calls
            .extend(increment.semantic_facts.node_resolved_calls);
        self.module
            .semantic_facts
            .node_function_references
            .extend(increment.semantic_facts.node_function_references);
        self.module
            .body_diagnostics
            .extend(increment.body_diagnostics);
        self.module
            .flow_check
            .diagnostics
            .extend(increment.flow_check.diagnostics);
        self.module.layouts = increment.layouts;
        self.checked_functions.extend(checked_functions);
        self.checked_globals.extend(checked_globals);
    }
}

fn executable_body_refs_for_checked_module(module: &CheckedModule) -> ExecutableModuleBodyRefs {
    let input = ReachableModuleInput {
        module_id: module.id,
        defs: &module.defs,
        body_ir: &module.body_ir,
        body_refs: &ExecutableModuleBodyRefs::default(),
        semantic_facts: &module.semantic_facts,
        type_lowering: &module.type_lowering,
        type_normalization: &module.type_normalization,
    };
    nia_executable_facts::executable_module_body_refs(&input)
}

pub(super) fn reachable_module_inputs(
    checked_by_id: &HashMap<ModuleId, ExecutableCheckedModuleState>,
) -> Vec<ReachableModuleInput<'_>> {
    checked_by_id
        .values()
        .map(ExecutableCheckedModuleState::reachable_input)
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

pub(super) fn stale_executable_modules(
    db: &QueryDb<CompilerContext>,
    parse_ok: &[ModuleId],
    reachability: &nia_executable_reachability::ExecutableReachability,
    checked_by_id: &HashMap<ModuleId, ExecutableCheckedModuleState>,
) -> std::collections::VecDeque<ModuleId> {
    parse_ok
        .iter()
        .copied()
        .filter(|module_id| reachability.modules.contains(module_id))
        .filter(|module_id| executable_module_has_pending_body_items(db, *module_id, reachability))
        .filter(|module_id| executable_module_is_stale(*module_id, reachability, checked_by_id))
        .collect()
}

pub(super) fn executable_module_is_stale(
    module_id: ModuleId,
    reachability: &nia_executable_reachability::ExecutableReachability,
    checked_by_id: &HashMap<ModuleId, ExecutableCheckedModuleState>,
) -> bool {
    match checked_by_id.get(&module_id) {
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
                    .query(ModuleDefsQuery(def_id.module_id))
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
