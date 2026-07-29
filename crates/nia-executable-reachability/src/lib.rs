// SPDX-License-Identifier: GPL-3.0-or-later
use std::collections::{HashMap, HashSet, VecDeque};
use std::sync::Arc;

use nia_defs::ExtensionMethods;
pub use nia_executable_facts::{
    ExecutableItemRefs, ReachableModuleInput, filter_semantic_facts_for_reachable_functions,
    filter_semantic_facts_for_reachable_items,
};
use nia_ids::{
    BuiltinTrait, BuiltinTraitMethod, GlobalDefId, InternedTyId, ModuleId, TraitId, TraitImplId,
};
use nia_item_signatures::{
    ProgramFunctionSignature, ProgramStructSignature, ProgramTraitImplSignature,
    ProgramTraitSignature, ProgramUnionSignature,
};
use nia_sema_ir::FunctionSemanticFacts;
use nia_symbol::{SymbolId, SymbolMap, known};
use nia_ty::{AssociatedTypeBindingTy, TyKind, TypeStore, TypeStoreAppend};

mod extension_index;
mod fact_owners;
mod inputs;
mod model;
mod signatures;
mod trait_closure;
mod type_matching;

use fact_owners::{
    builtin_trait_method_symbol, collect_reachable_fact_owner_modules,
    collect_reachable_fact_owner_modules_for_items,
};
use trait_closure::{
    ReachableTraitMethodName, ReachableTraitRefs, add_reachable_function,
    collect_reachable_body_trait_ids, collect_reachable_traits_for_modules,
    extend_reachable_functions_from_bodies, extend_reachable_functions_from_traits,
    extend_reachable_functions_from_traits_incremental,
    extend_reachable_traits_from_generic_instances,
    extend_reachable_traits_from_generic_instances_incremental, typed_executable_refs_for_items,
};
use type_matching::{
    ReachabilityTypeCx, ReachableExtensionMatchInput, TypeSubstitutions,
    extend_reachable_trait_methods_from_impl_where_predicates, substitute_ty, trait_id_and_args,
    with_reachable_extension_method_match,
};

pub use extension_index::{ExecutableExtensionIndex, ExecutableExtensionLookup};
pub use inputs::{
    CheckedModuleReachabilityInput, ExecutableExtensionSources, ExecutableReachabilityInput,
    ExecutableRootDefs,
};
pub use model::{
    ExecutableModuleReachability, ExecutableReachability, ExecutableReachabilityByModule,
    ExecutableReachabilityStats,
};
pub use signatures::ExecutableSignatureIndex;

#[derive(Debug, Clone, Default)]
pub struct IncrementalExecutableReachability {
    reachability: ExecutableReachability,
    scanned_functions: HashSet<GlobalDefId>,
    scanned_globals: HashSet<GlobalDefId>,
    scanned_generic_trait_functions: HashSet<GlobalDefId>,
    trait_function_scan: ReachableTraitFunctionScan,
    reachable_traits: ReachableTraitRefs,
}

impl IncrementalExecutableReachability {
    pub fn reachability(&self) -> &ExecutableReachability {
        &self.reachability
    }

    pub fn reachability_mut(&mut self) -> &mut ExecutableReachability {
        &mut self.reachability
    }

    pub fn into_reachability(self) -> ExecutableReachability {
        self.reachability
    }
}

pub fn compute_executable_reachability_with_seed_and_extension_index(
    seed: Option<&ExecutableReachability>,
    input: ExecutableReachabilityInput<'_>,
    extension_index: &dyn ExecutableExtensionLookup,
) -> ExecutableReachability {
    let ExecutableReachabilityInput {
        parse_ok,
        entry_module,
        root_defs,
        program_signatures,
        modules,
    } = input;
    let modules_by_id = modules
        .iter()
        .map(|module| (module.module_id, *module))
        .collect::<HashMap<_, _>>();
    let mut reachability = ExecutableReachability {
        modules: seed.map(|seed| seed.modules.clone()).unwrap_or_default(),
        type_modules: seed
            .map(|seed| seed.type_modules.clone())
            .unwrap_or_default(),
        functions: HashSet::default(),
        globals: seed.map(|seed| seed.globals.clone()).unwrap_or_default(),
        stats: ExecutableReachabilityStats::default(),
    };
    let mut pending_seed_modules = VecDeque::new();
    for def_id in seed
        .into_iter()
        .flat_map(|seed| seed.functions.iter().copied())
        .chain(root_defs.functions.iter().copied())
    {
        add_reachable_function(
            def_id,
            program_signatures,
            &mut reachability,
            &mut pending_seed_modules,
        );
    }
    reachability.insert_globals(root_defs.globals.iter().copied());
    reachability.insert_module(entry_module);

    let parse_ok_set = parse_ok.iter().copied().collect::<HashSet<_>>();
    loop {
        let before = reachability.change_key();
        let current_reachable_modules =
            current_reachable_module_ids(&modules_by_id, reachability.modules());
        let mut reachable_traits = collect_reachable_traits_for_modules(
            &modules_by_id,
            &current_reachable_modules,
            reachability.functions(),
            reachability.globals(),
        );
        extend_reachable_traits_from_generic_instances(
            &modules_by_id,
            &current_reachable_modules,
            program_signatures,
            extension_index,
            reachability.functions(),
            &mut reachable_traits,
        );
        for module in current_reachable_module_inputs(&modules_by_id, &current_reachable_modules) {
            let mut pending_modules = VecDeque::new();
            extend_reachable_functions_from_bodies(
                &module,
                program_signatures,
                &mut reachability,
                &mut pending_modules,
            );
            collect_reachable_fact_owner_modules(
                &module,
                program_signatures,
                &reachability.functions,
                &reachability.globals,
                &mut reachability.type_modules,
                &mut reachable_traits,
            );
        }
        let mut pending_modules = VecDeque::new();
        extend_reachable_functions_from_traits(
            program_signatures,
            extension_index,
            &modules_by_id,
            &mut reachable_traits,
            &mut reachability,
            &mut pending_modules,
        );
        while let Some(module_id) = pending_modules.pop_front() {
            if !parse_ok_set.contains(&module_id) {
                continue;
            }
            reachability.insert_module(module_id);
        }
        if before == reachability.change_key() {
            break;
        }
    }

    let stats = reachability_stats(&modules_by_id, reachability.functions());
    reachability.set_stats(stats);
    reachability
}

pub fn compute_executable_reachability_incremental_with_extension_index(
    state: &mut IncrementalExecutableReachability,
    input: ExecutableReachabilityInput<'_>,
    extension_index: &dyn ExecutableExtensionLookup,
) {
    compute_executable_reachability_incremental_with_timings(
        state,
        input,
        extension_index,
        nia_timing::TimingMode::Off,
    )
}

pub fn compute_executable_reachability_incremental_with_timings(
    state: &mut IncrementalExecutableReachability,
    input: ExecutableReachabilityInput<'_>,
    extension_index: &dyn ExecutableExtensionLookup,
    timings: nia_timing::TimingMode,
) {
    let ExecutableReachabilityInput {
        parse_ok,
        entry_module,
        root_defs,
        program_signatures,
        modules,
    } = input;
    let modules_by_id = time_reachability_stage(timings, "incremental.modules_by_id", None, || {
        modules
            .iter()
            .map(|module| (module.module_id, *module))
            .collect::<HashMap<_, _>>()
    });
    let parse_ok_set = time_reachability_stage(timings, "incremental.parse_ok", None, || {
        parse_ok.iter().copied().collect::<HashSet<_>>()
    });
    let mut pending_modules = VecDeque::new();
    time_reachability_stage(timings, "incremental.roots", None, || {
        for def_id in root_defs.functions.iter().copied() {
            add_reachable_function(
                def_id,
                program_signatures,
                &mut state.reachability,
                &mut pending_modules,
            );
        }
        state
            .reachability
            .insert_globals(root_defs.globals.iter().copied());
        state
            .reachability
            .insert_module_pending(entry_module, &mut pending_modules);
    });

    loop {
        let before = incremental_reachability_key(state);
        let current_reachable_modules =
            time_reachability_stage(timings, "incremental.current_modules", None, || {
                current_reachable_module_ids(&modules_by_id, &state.reachability.modules)
            });
        for module in current_reachable_module_inputs(&modules_by_id, &current_reachable_modules) {
            let mut pending_modules = VecDeque::new();
            time_reachability_stage(
                timings,
                "incremental.unscanned_items",
                Some(module.module_id),
                || {
                    extend_reachability_from_unscanned_items(
                        state,
                        &module,
                        program_signatures,
                        &mut pending_modules,
                    );
                },
            );
            while let Some(module_id) = pending_modules.pop_front() {
                if parse_ok_set.contains(&module_id) {
                    state.reachability.insert_module(module_id);
                }
            }
        }
        let current_reachable_modules =
            time_reachability_stage(timings, "incremental.current_modules", None, || {
                current_reachable_module_ids(&modules_by_id, &state.reachability.modules)
            });
        time_reachability_stage(timings, "incremental.generic_traits", None, || {
            extend_reachable_traits_from_generic_instances_incremental(
                state,
                &modules_by_id,
                &current_reachable_modules,
                program_signatures,
                extension_index,
            );
        });
        let mut pending_modules = VecDeque::new();
        time_reachability_stage(timings, "incremental.trait_functions", None, || {
            extend_reachable_functions_from_traits_incremental(
                state,
                program_signatures,
                extension_index,
                &modules_by_id,
                &mut pending_modules,
            );
        });
        while let Some(module_id) = pending_modules.pop_front() {
            if parse_ok_set.contains(&module_id) {
                state.reachability.insert_module(module_id);
            }
        }
        if before == incremental_reachability_key(state) {
            break;
        }
    }

    state.reachability.stats = reachability_stats(&modules_by_id, &state.reachability.functions);
}

pub fn extend_incremental_executable_reachability_from_checked_module(
    state: &mut IncrementalExecutableReachability,
    input: CheckedModuleReachabilityInput<'_>,
    extensions: ExecutableExtensionSources<'_>,
) -> ExecutableReachability {
    let extension_index = ExecutableExtensionIndex::new(extensions.methods, extensions.trait_impls);
    extend_incremental_executable_reachability_from_checked_module_with_extension_index(
        state,
        input,
        &extension_index,
    );
    state.reachability.clone()
}

pub fn extend_incremental_executable_reachability_from_checked_module_with_extension_index(
    state: &mut IncrementalExecutableReachability,
    input: CheckedModuleReachabilityInput<'_>,
    extension_index: &dyn ExecutableExtensionLookup,
) {
    extend_incremental_executable_reachability_from_checked_module_with_timings(
        state,
        input,
        extension_index,
        nia_timing::TimingMode::Off,
    )
}

pub fn extend_incremental_executable_reachability_from_checked_module_with_timings(
    state: &mut IncrementalExecutableReachability,
    input: CheckedModuleReachabilityInput<'_>,
    extension_index: &dyn ExecutableExtensionLookup,
    timings: nia_timing::TimingMode,
) {
    let CheckedModuleReachabilityInput {
        parse_ok,
        program_signatures,
        module,
        checked_functions,
        modules_by_id,
    } = input;
    let parse_ok_set = time_reachability_stage(
        timings,
        "incremental.parse_ok",
        Some(module.module_id),
        || parse_ok.iter().copied().collect::<HashSet<_>>(),
    );
    for def_id in checked_functions {
        state.scanned_generic_trait_functions.remove(def_id);
    }

    loop {
        let before = incremental_reachability_key(state);
        let mut pending_modules = VecDeque::new();
        time_reachability_stage(
            timings,
            "incremental.unscanned_items",
            Some(module.module_id),
            || {
                extend_reachability_from_unscanned_items(
                    state,
                    &module,
                    program_signatures,
                    &mut pending_modules,
                );
            },
        );
        while let Some(module_id) = pending_modules.pop_front() {
            if parse_ok_set.contains(&module_id) {
                state.reachability.insert_module(module_id);
            }
        }
        let current_reachable_modules = time_reachability_stage(
            timings,
            "incremental.current_modules",
            Some(module.module_id),
            || current_reachable_module_ids(modules_by_id, &state.reachability.modules),
        );
        time_reachability_stage(
            timings,
            "incremental.generic_traits",
            Some(module.module_id),
            || {
                extend_reachable_traits_from_generic_instances_incremental(
                    state,
                    modules_by_id,
                    &current_reachable_modules,
                    program_signatures,
                    extension_index,
                );
            },
        );
        let mut pending_modules = VecDeque::new();
        time_reachability_stage(
            timings,
            "incremental.trait_functions",
            Some(module.module_id),
            || {
                extend_reachable_functions_from_traits_incremental(
                    state,
                    program_signatures,
                    extension_index,
                    modules_by_id,
                    &mut pending_modules,
                );
            },
        );
        while let Some(module_id) = pending_modules.pop_front() {
            if parse_ok_set.contains(&module_id) {
                state.reachability.insert_module(module_id);
            }
        }

        if before == incremental_reachability_key(state) {
            break;
        }
    }
}

fn time_reachability_stage<T>(
    timings: nia_timing::TimingMode,
    name: &str,
    module_id: Option<ModuleId>,
    f: impl FnOnce() -> T,
) -> T {
    let Some(module_id) = module_id else {
        return nia_timing::time_query(timings, name, f);
    };
    nia_timing::time_query(timings, &format!("{name}[{module_id:?}]"), f)
}

fn incremental_reachability_key(
    state: &IncrementalExecutableReachability,
) -> (usize, usize, usize, usize, usize, usize, usize) {
    let (trait_count, method_count, vtable_count) = state.reachable_traits.counts();
    (
        state.reachability.functions.len(),
        state.reachability.globals.len(),
        state.reachability.modules.len(),
        state.reachability.type_modules.len(),
        trait_count,
        method_count,
        vtable_count,
    )
}

#[derive(Debug, Clone, Copy, Default)]
struct ReachableTraitFunctionScan {
    methods: usize,
    vtables: usize,
}

fn reachability_stats(
    modules_by_id: &HashMap<ModuleId, ReachableModuleInput<'_>>,
    reachable_functions: &HashSet<GlobalDefId>,
) -> ExecutableReachabilityStats {
    ExecutableReachabilityStats {
        checked_modules: modules_by_id.len(),
        checked_bodies: modules_by_id
            .values()
            .map(|module| module.body_ir.function_bodies.len())
            .sum(),
        reachable_bodies: modules_by_id
            .values()
            .map(|module| {
                module
                    .body_ir
                    .function_bodies
                    .keys()
                    .filter(|def_id| reachable_functions.contains(def_id))
                    .count()
            })
            .sum(),
    }
}

fn current_reachable_module_ids(
    modules_by_id: &HashMap<ModuleId, ReachableModuleInput<'_>>,
    reachable_modules: &HashSet<ModuleId>,
) -> Vec<ModuleId> {
    let mut ids = modules_by_id
        .keys()
        .copied()
        .filter(|module_id| reachable_modules.contains(module_id))
        .collect::<Vec<_>>();
    ids.sort_unstable();
    ids
}

fn current_reachable_module_inputs<'a>(
    modules_by_id: &'a HashMap<ModuleId, ReachableModuleInput<'a>>,
    current_reachable_modules: &'a [ModuleId],
) -> impl Iterator<Item = ReachableModuleInput<'a>> + 'a {
    current_reachable_modules
        .iter()
        .filter_map(|module_id| modules_by_id.get(module_id).copied())
}

fn module_id_list_contains(module_ids: &[ModuleId], module_id: ModuleId) -> bool {
    module_ids.binary_search(&module_id).is_ok()
}

fn extend_reachability_from_unscanned_items(
    state: &mut IncrementalExecutableReachability,
    module: &ReachableModuleInput<'_>,
    program_signatures: ExecutableSignatureIndex<'_>,
    pending_modules: &mut VecDeque<ModuleId>,
) {
    let present_functions = state
        .reachability
        .functions
        .iter()
        .copied()
        .filter(|def_id| def_id.module_id == module.module_id)
        .filter(|def_id| module.executable_refs.functions.contains_key(def_id))
        .filter(|def_id| !state.scanned_functions.contains(def_id))
        .collect::<HashSet<_>>();
    let present_globals = state
        .reachability
        .globals
        .iter()
        .copied()
        .filter(|def_id| def_id.module_id == module.module_id)
        .filter(|def_id| module.executable_refs.globals.contains_key(def_id))
        .filter(|def_id| !state.scanned_globals.contains(def_id))
        .collect::<HashSet<_>>();
    if present_functions.is_empty() && present_globals.is_empty() {
        return;
    }

    let refs = typed_executable_refs_for_items(module, &present_functions, &present_globals);
    for instantiation in &refs.generic_instantiations {
        add_reachable_function(
            instantiation.def_id,
            program_signatures,
            &mut state.reachability,
            pending_modules,
        );
    }
    for def_id in refs.functions {
        add_reachable_function(
            def_id,
            program_signatures,
            &mut state.reachability,
            pending_modules,
        );
    }
    for def_id in refs.globals {
        state
            .reachability
            .insert_global_pending(def_id, pending_modules);
    }
    state.reachable_traits.extend(refs.traits);
    collect_reachable_fact_owner_modules_for_items(
        module,
        program_signatures,
        &present_functions,
        &present_globals,
        &mut state.reachability.type_modules,
        &mut state.reachable_traits,
    );
    state.scanned_functions.extend(present_functions);
    state.scanned_globals.extend(present_globals);
}

pub fn extend_executable_reachability_from_checked_module(
    reachability: &mut ExecutableReachability,
    program_signatures: ExecutableSignatureIndex<'_>,
    extension_methods: &ExtensionMethods,
    trait_impls: &[ProgramTraitImplSignature],
    module: ReachableModuleInput<'_>,
    checked_modules: &[ReachableModuleInput<'_>],
) -> bool {
    let extension_index = ExecutableExtensionIndex::new(extension_methods, trait_impls);
    extend_executable_reachability_from_checked_module_with_extension_index(
        reachability,
        program_signatures,
        &extension_index,
        module,
        checked_modules,
    )
}

pub fn extend_executable_reachability_from_checked_module_with_extension_index(
    reachability: &mut ExecutableReachability,
    program_signatures: ExecutableSignatureIndex<'_>,
    extension_index: &dyn ExecutableExtensionLookup,
    module: ReachableModuleInput<'_>,
    checked_modules: &[ReachableModuleInput<'_>],
) -> bool {
    let before = reachability.change_key();
    let mut pending_modules = VecDeque::new();
    extend_reachable_functions_from_bodies(
        &module,
        program_signatures,
        reachability,
        &mut pending_modules,
    );
    let mut reachable_traits = ReachableTraitRefs::default();
    collect_reachable_body_trait_ids(
        &module,
        &reachability.functions,
        &reachability.globals,
        &mut reachable_traits,
    );
    let mut modules_by_id = checked_modules
        .iter()
        .map(|checked_module| (checked_module.module_id, *checked_module))
        .collect::<HashMap<_, _>>();
    modules_by_id.insert(module.module_id, module);
    let current_reachable_modules =
        current_reachable_module_ids(&modules_by_id, &reachability.modules);
    extend_reachable_traits_from_generic_instances(
        &modules_by_id,
        &current_reachable_modules,
        program_signatures,
        extension_index,
        &reachability.functions,
        &mut reachable_traits,
    );
    collect_reachable_fact_owner_modules(
        &module,
        program_signatures,
        &reachability.functions,
        &reachability.globals,
        &mut reachability.type_modules,
        &mut reachable_traits,
    );
    extend_reachable_functions_from_traits(
        program_signatures,
        extension_index,
        &modules_by_id,
        &mut reachable_traits,
        reachability,
        &mut pending_modules,
    );
    before != reachability.change_key()
}
