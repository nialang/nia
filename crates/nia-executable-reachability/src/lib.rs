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
mod type_matching;

use fact_owners::{
    builtin_trait_method_symbol, collect_reachable_fact_owner_modules,
    collect_reachable_fact_owner_modules_for_items,
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
    (
        state.reachability.functions.len(),
        state.reachability.globals.len(),
        state.reachability.modules.len(),
        state.reachability.type_modules.len(),
        state.reachable_traits.traits.len(),
        state.reachable_traits.methods.len(),
        state.reachable_traits.vtables.len(),
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

#[derive(Clone, Copy)]
pub struct ExecutableSignatureIndex<'a> {
    pub function: &'a dyn Fn(GlobalDefId) -> Option<Arc<ProgramFunctionSignature>>,
    pub struct_: &'a dyn Fn(GlobalDefId) -> Option<ProgramStructSignature>,
    pub union: &'a dyn Fn(GlobalDefId) -> Option<ProgramUnionSignature>,
    pub trait_: &'a dyn Fn(GlobalDefId) -> Option<ProgramTraitSignature>,
    pub trait_default_method:
        &'a dyn Fn(GlobalDefId) -> Option<(GlobalDefId, ProgramTraitSignature)>,
}

impl std::fmt::Debug for ExecutableSignatureIndex<'_> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ExecutableSignatureIndex")
            .field("function", &true)
            .field("struct_", &true)
            .field("union", &true)
            .field("trait_", &true)
            .finish()
    }
}

#[derive(Clone, Copy)]
struct GenericTraitReachabilityContext<'a> {
    modules_by_id: &'a HashMap<ModuleId, ReachableModuleInput<'a>>,
    program_signatures: ExecutableSignatureIndex<'a>,
    extension_index: &'a dyn ExecutableExtensionLookup,
}

fn extend_reachable_functions_from_bodies(
    module: &ReachableModuleInput<'_>,
    program_signatures: ExecutableSignatureIndex<'_>,
    reachability: &mut ExecutableReachability,
    pending_modules: &mut VecDeque<ModuleId>,
) {
    let refs = typed_executable_refs(module, &reachability.functions, &reachability.globals);
    for instantiation in &refs.generic_instantiations {
        add_reachable_function(
            instantiation.def_id,
            program_signatures,
            reachability,
            pending_modules,
        );
    }
    for def_id in refs.functions {
        add_reachable_function(def_id, program_signatures, reachability, pending_modules);
    }
    for def_id in refs.globals {
        reachability.insert_global_pending(def_id, pending_modules);
    }
}

fn collect_reachable_traits_for_modules(
    modules_by_id: &HashMap<ModuleId, ReachableModuleInput<'_>>,
    current_reachable_modules: &[ModuleId],
    reachable_functions: &HashSet<GlobalDefId>,
    reachable_globals: &HashSet<GlobalDefId>,
) -> ReachableTraitRefs {
    let mut reachable_traits = ReachableTraitRefs::default();
    for module in current_reachable_module_inputs(modules_by_id, current_reachable_modules) {
        collect_reachable_body_trait_ids(
            &module,
            reachable_functions,
            reachable_globals,
            &mut reachable_traits,
        );
    }
    reachable_traits
}

fn extend_reachable_traits_from_generic_instances(
    modules_by_id: &HashMap<ModuleId, ReachableModuleInput<'_>>,
    current_reachable_modules: &[ModuleId],
    program_signatures: ExecutableSignatureIndex<'_>,
    extension_index: &dyn ExecutableExtensionLookup,
    reachable_functions: &HashSet<GlobalDefId>,
    traits: &mut ReachableTraitRefs,
) {
    for def_id in reachable_functions {
        if !module_id_list_contains(current_reachable_modules, def_id.module_id) {
            continue;
        }
        let Some(module) = modules_by_id.get(&def_id.module_id) else {
            continue;
        };
        let mut executable_refs = typed_executable_refs_for_function(module, *def_id);
        for instantiation in executable_refs.generic_instantiations.drain(..) {
            let mut visited = HashSet::default();
            extend_reachable_traits_from_generic_instantiation(
                module.module_id,
                module.type_store,
                GenericTraitReachabilityContext {
                    modules_by_id,
                    program_signatures,
                    extension_index,
                },
                traits,
                &instantiation,
                &mut visited,
                &mut HashSet::new(),
            );
        }
    }
}

fn extend_reachable_traits_from_generic_instances_incremental(
    state: &mut IncrementalExecutableReachability,
    modules_by_id: &HashMap<ModuleId, ReachableModuleInput<'_>>,
    current_reachable_modules: &[ModuleId],
    program_signatures: ExecutableSignatureIndex<'_>,
    extension_index: &dyn ExecutableExtensionLookup,
) {
    let pending_functions = state
        .reachability
        .functions
        .iter()
        .copied()
        .filter(|def_id| module_id_list_contains(current_reachable_modules, def_id.module_id))
        .filter(|def_id| !state.scanned_generic_trait_functions.contains(def_id))
        .collect::<Vec<_>>();
    for def_id in pending_functions {
        let Some(module) = modules_by_id.get(&def_id.module_id) else {
            continue;
        };
        state.scanned_generic_trait_functions.insert(def_id);
        let mut executable_refs = typed_executable_refs_for_function(module, def_id);
        for instantiation in executable_refs.generic_instantiations.drain(..) {
            let mut visited = HashSet::default();
            extend_reachable_traits_from_generic_instantiation(
                module.module_id,
                module.type_store,
                GenericTraitReachabilityContext {
                    modules_by_id,
                    program_signatures,
                    extension_index,
                },
                &mut state.reachable_traits,
                &instantiation,
                &mut visited,
                &mut HashSet::new(),
            );
        }
    }
}

fn extend_reachable_traits_from_generic_instantiation(
    use_module_id: ModuleId,
    type_store: &TypeStore,
    context: GenericTraitReachabilityContext<'_>,
    traits: &mut ReachableTraitRefs,
    instantiation: &nia_sema_ir::GenericInstantiation,
    visited: &mut HashSet<ReachableGenericInstantiationKey>,
    active_defs: &mut HashSet<GlobalDefId>,
) {
    let GenericTraitReachabilityContext {
        modules_by_id,
        program_signatures,
        extension_index,
    } = context;
    if !visited.insert(reachable_generic_instantiation_key(instantiation)) {
        return;
    }
    if !active_defs.insert(instantiation.def_id) {
        return;
    }
    extend_reachable_traits_from_trait_default_instantiation(
        use_module_id,
        program_signatures,
        traits,
        instantiation,
    );
    let Some(signature) = (program_signatures.function)(instantiation.def_id) else {
        active_defs.remove(&instantiation.def_id);
        return;
    };
    let append = type_store.append_for_module(use_module_id);
    let types = ReachabilityTypeCx {
        store: type_store,
        append: &append,
    };
    let generics = if instantiation.generics.is_empty() && !instantiation.args.is_empty() {
        &signature.signature.generics
    } else {
        &instantiation.generics
    };
    let generic_substitutions = generics
        .iter()
        .cloned()
        .zip(instantiation.args.iter().copied())
        .collect::<SymbolMap<_>>();
    let self_ty = instantiation.self_arg;
    let substitutions = TypeSubstitutions::local(self_ty, &generic_substitutions);
    for predicate in &signature.signature.where_predicates {
        let Some(self_ty) = substitute_ty(types, predicate.ty, &substitutions) else {
            continue;
        };
        for bound in &predicate.bounds {
            let Some(trait_ty) = substitute_ty(types, bound.trait_ty, &substitutions) else {
                continue;
            };
            let Some((trait_id, trait_args)) = trait_id_and_args(type_store, trait_ty) else {
                continue;
            };
            insert_trait_and_supertrait_methods(
                program_signatures,
                type_store,
                traits,
                use_module_id,
                trait_id,
                self_ty,
                &trait_args,
            );
        }
    }
    extension_index.with_where_predicates_for_def(instantiation.def_id, &mut |predicates| {
        for predicate in predicates {
            let Some(self_ty) = substitute_ty(types, predicate.ty, &substitutions) else {
                continue;
            };
            for bound in &predicate.bounds {
                let Some(trait_ty) = substitute_ty(types, bound.trait_ty, &substitutions) else {
                    continue;
                };
                let Some((trait_id, trait_args)) = trait_id_and_args(type_store, trait_ty) else {
                    continue;
                };
                insert_trait_and_supertrait_methods(
                    program_signatures,
                    type_store,
                    traits,
                    use_module_id,
                    trait_id,
                    self_ty,
                    &trait_args,
                );
            }
        }
    });
    let Some(target_module) = modules_by_id.get(&instantiation.def_id.module_id) else {
        active_defs.remove(&instantiation.def_id);
        return;
    };
    let nested_instantiations = target_module
        .semantic_facts
        .function_facts
        .get(&instantiation.def_id)
        .into_iter()
        .flat_map(|facts| facts.generic_instantiations.iter())
        .chain(
            target_module
                .semantic_facts
                .generic_instantiations
                .iter()
                .filter(|nested| nested.source_def_id == Some(instantiation.def_id)),
        );
    for nested in nested_instantiations {
        let Some(nested_instantiation) =
            instantiate_nested_generic_instantiation(types, nested, &substitutions)
        else {
            continue;
        };
        extend_reachable_traits_from_generic_instantiation(
            use_module_id,
            type_store,
            context,
            traits,
            &nested_instantiation,
            visited,
            active_defs,
        );
    }
    active_defs.remove(&instantiation.def_id);
}

fn extend_reachable_traits_from_trait_default_instantiation(
    use_module_id: ModuleId,
    program_signatures: ExecutableSignatureIndex<'_>,
    traits: &mut ReachableTraitRefs,
    instantiation: &nia_sema_ir::GenericInstantiation,
) {
    let Some((trait_def, trait_signature)) =
        (program_signatures.trait_default_method)(instantiation.def_id)
    else {
        return;
    };
    let Some(_) = trait_signature
        .signature
        .methods
        .iter()
        .find(|method| method.def_id == instantiation.def_id.def_id && method.has_default)
    else {
        return;
    };
    let trait_id = TraitId::Source(trait_def);
    let Some(self_ty) = instantiation.self_arg else {
        return;
    };
    let trait_args = instantiation
        .args
        .iter()
        .take(trait_signature.signature.generics.len())
        .copied()
        .collect::<Vec<_>>();
    traits.insert_methods(
        use_module_id,
        trait_id,
        trait_signature
            .signature
            .methods
            .iter()
            .map(|method| ReachableTraitMethodName { name: method.name }),
        self_ty,
        &trait_args,
    );
}

fn insert_trait_and_supertrait_methods(
    program_signatures: ExecutableSignatureIndex<'_>,
    type_store: &TypeStore,
    traits: &mut ReachableTraitRefs,
    module_id: ModuleId,
    trait_id: TraitId,
    self_ty: InternedTyId,
    trait_args: &[InternedTyId],
) {
    let append = type_store.append_for_module(module_id);
    TraitMethodExpansion {
        program_signatures,
        types: ReachabilityTypeCx {
            store: type_store,
            append: &append,
        },
        traits,
        module_id,
        active_traits: HashSet::new(),
    }
    .insert(trait_id, self_ty, trait_args);
}

struct TraitMethodExpansion<'a, 'b> {
    program_signatures: ExecutableSignatureIndex<'a>,
    types: ReachabilityTypeCx<'b>,
    traits: &'b mut ReachableTraitRefs,
    module_id: ModuleId,
    active_traits: HashSet<TraitId>,
}

impl TraitMethodExpansion<'_, '_> {
    fn insert(&mut self, trait_id: TraitId, self_ty: InternedTyId, trait_args: &[InternedTyId]) {
        if !self.active_traits.insert(trait_id) {
            return;
        }
        match trait_id {
            TraitId::Builtin(builtin_trait) => {
                self.traits.insert_methods(
                    self.module_id,
                    trait_id,
                    builtin_trait
                        .required_methods()
                        .iter()
                        .filter_map(|method| builtin_trait_method_symbol(*method))
                        .map(|name| ReachableTraitMethodName { name }),
                    self_ty,
                    trait_args,
                );
                for supertrait in builtin_trait.supertraits() {
                    let supertrait_args = if supertrait.preserves_trait_args {
                        trait_args
                    } else {
                        &[]
                    };
                    self.insert(
                        TraitId::Builtin(supertrait.trait_id),
                        self_ty,
                        supertrait_args,
                    );
                }
            }
            TraitId::Source(trait_def) => {
                let Some(trait_signature) = (self.program_signatures.trait_)(trait_def) else {
                    self.active_traits.remove(&trait_id);
                    return;
                };
                self.traits.insert_methods(
                    self.module_id,
                    trait_id,
                    trait_signature
                        .signature
                        .methods
                        .iter()
                        .map(|method| ReachableTraitMethodName { name: method.name }),
                    self_ty,
                    trait_args,
                );
                for supertrait in &trait_signature.signature.supertraits {
                    let substitutions = trait_signature
                        .signature
                        .generics
                        .iter()
                        .copied()
                        .zip(trait_args.iter().copied())
                        .collect::<SymbolMap<_>>();
                    let substitutions = TypeSubstitutions::local(Some(self_ty), &substitutions);
                    let Some(supertrait_ty) =
                        substitute_ty(self.types, supertrait.ty, &substitutions)
                    else {
                        continue;
                    };
                    let Some((supertrait_id, supertrait_args)) =
                        trait_id_and_args(self.types.store, supertrait_ty)
                    else {
                        continue;
                    };
                    self.insert(supertrait_id, self_ty, &supertrait_args);
                }
            }
        }
        self.active_traits.remove(&trait_id);
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
struct ReachableGenericInstantiationKey {
    def_id: GlobalDefId,
    args: Vec<InternedTyId>,
    const_args: Vec<nia_ty::ConstGenericArg>,
}

fn reachable_generic_instantiation_key(
    instantiation: &nia_sema_ir::GenericInstantiation,
) -> ReachableGenericInstantiationKey {
    ReachableGenericInstantiationKey {
        def_id: instantiation.def_id,
        args: instantiation.args.clone(),
        const_args: instantiation.const_args.clone(),
    }
}

fn instantiate_nested_generic_instantiation(
    types: ReachabilityTypeCx<'_>,
    instantiation: &nia_sema_ir::GenericInstantiation,
    substitutions: &TypeSubstitutions<'_>,
) -> Option<nia_sema_ir::GenericInstantiation> {
    let self_arg = match instantiation.self_arg {
        Some(self_arg) => Some(substitute_ty(types, self_arg, substitutions)?),
        None => None,
    };
    let args = instantiation
        .args
        .iter()
        .map(|arg| substitute_ty(types, *arg, substitutions))
        .collect::<Option<Vec<_>>>()?;
    let const_args = instantiation
        .const_args
        .iter()
        .cloned()
        .map(|mut arg| {
            arg.ty = substitute_ty(types, arg.ty, substitutions)?;
            Some(arg)
        })
        .collect::<Option<Vec<_>>>()?;
    Some(nia_sema_ir::GenericInstantiation {
        def_id: instantiation.def_id,
        self_arg,
        args,
        const_args,
        generics: instantiation.generics.clone(),
        span: instantiation.span,
        source_def_id: instantiation.source_def_id,
    })
}

fn typed_executable_refs(
    module: &ReachableModuleInput<'_>,
    reachable_functions: &HashSet<GlobalDefId>,
    reachable_globals: &HashSet<GlobalDefId>,
) -> TypedExecutableRefs {
    typed_executable_refs_for_items(module, reachable_functions, reachable_globals)
}

fn typed_executable_refs_for_items(
    module: &ReachableModuleInput<'_>,
    functions: &HashSet<GlobalDefId>,
    globals: &HashSet<GlobalDefId>,
) -> TypedExecutableRefs {
    let refs = module.executable_refs.refs_for_items(functions, globals);
    typed_executable_refs_from_executable_refs(refs)
}

fn typed_executable_refs_for_function(
    module: &ReachableModuleInput<'_>,
    def_id: GlobalDefId,
) -> TypedExecutableRefs {
    let refs = module.executable_refs.refs_for_function(def_id);
    typed_executable_refs_from_executable_refs(refs)
}

fn collect_reachable_body_trait_ids(
    module: &ReachableModuleInput<'_>,
    reachable_functions: &HashSet<GlobalDefId>,
    reachable_globals: &HashSet<GlobalDefId>,
    traits: &mut ReachableTraitRefs,
) {
    let refs = typed_executable_refs_for_items(module, reachable_functions, reachable_globals);
    traits.extend(refs.traits);
}

fn typed_executable_refs_from_executable_refs(refs: ExecutableItemRefs) -> TypedExecutableRefs {
    let mut traits = ReachableTraitRefs::default();
    for trait_id in refs.trait_refs.traits {
        traits.insert_trait(trait_id);
    }
    for method in refs.trait_refs.methods {
        traits.insert_method(
            method.module_id,
            method.trait_id,
            method.method_name,
            method.self_ty,
            method.trait_args,
        );
    }
    for vtable in refs.trait_refs.vtables {
        traits.insert_vtable(
            vtable.module_id,
            vtable.trait_id,
            vtable.self_ty,
            vtable.trait_args,
        );
    }
    TypedExecutableRefs {
        functions: refs.functions,
        globals: refs.globals,
        traits,
        generic_instantiations: refs.generic_instantiations,
    }
}

#[derive(Default)]
struct TypedExecutableRefs {
    functions: HashSet<GlobalDefId>,
    globals: HashSet<GlobalDefId>,
    traits: ReachableTraitRefs,
    generic_instantiations: Vec<nia_sema_ir::GenericInstantiation>,
}

#[derive(Debug, Clone, Default)]
struct ReachableTraitRefs {
    traits: HashSet<TraitId>,
    methods: Vec<ReachableTraitMethod>,
    method_keys: HashSet<ReachableTraitMethodKey>,
    vtables: Vec<ReachableTraitVtable>,
    vtable_keys: HashSet<ReachableTraitVtableKey>,
}

#[derive(Debug, Clone)]
struct ReachableTraitMethod {
    module_id: ModuleId,
    trait_id: TraitId,
    method_name: SymbolId,
    self_ty: InternedTyId,
    trait_args: Vec<InternedTyId>,
}

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
struct ReachableTraitMethodKey {
    module_id: ModuleId,
    trait_id: TraitId,
    method_name: SymbolId,
    self_ty: InternedTyId,
    trait_args: Vec<InternedTyId>,
}

#[derive(Debug, Clone)]
struct ReachableTraitVtable {
    module_id: ModuleId,
    trait_id: TraitId,
    self_ty: InternedTyId,
    trait_args: Vec<InternedTyId>,
}

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
struct ReachableTraitVtableKey {
    module_id: ModuleId,
    trait_id: TraitId,
    self_ty: InternedTyId,
    trait_args: Vec<InternedTyId>,
}

struct ReachableTraitMethodName {
    name: SymbolId,
}

impl ReachableTraitRefs {
    fn extend(&mut self, refs: Self) {
        let ReachableTraitRefs {
            traits,
            methods,
            vtables,
            ..
        } = refs;
        self.traits.extend(traits);
        for method in methods {
            self.insert_method(
                method.module_id,
                method.trait_id,
                method.method_name,
                method.self_ty,
                method.trait_args,
            );
        }
        for vtable in vtables {
            self.insert_vtable(
                vtable.module_id,
                vtable.trait_id,
                vtable.self_ty,
                vtable.trait_args,
            );
        }
    }

    fn insert_trait(&mut self, trait_id: TraitId) {
        self.traits.insert(trait_id);
    }

    fn insert_method(
        &mut self,
        module_id: ModuleId,
        trait_id: TraitId,
        method_name: SymbolId,
        self_ty: InternedTyId,
        trait_args: Vec<InternedTyId>,
    ) {
        self.traits.insert(trait_id);
        if !self.method_keys.insert(ReachableTraitMethodKey {
            module_id,
            trait_id,
            method_name,
            self_ty,
            trait_args: trait_args.clone(),
        }) {
            return;
        };
        self.methods.push(ReachableTraitMethod {
            module_id,
            trait_id,
            method_name,
            self_ty,
            trait_args,
        });
    }

    fn insert_methods(
        &mut self,
        module_id: ModuleId,
        trait_id: TraitId,
        methods: impl IntoIterator<Item = ReachableTraitMethodName>,
        self_ty: InternedTyId,
        trait_args: &[InternedTyId],
    ) {
        for method in methods {
            self.insert_method(
                module_id,
                trait_id,
                method.name,
                self_ty,
                trait_args.to_vec(),
            );
        }
    }

    fn insert_vtable(
        &mut self,
        module_id: ModuleId,
        trait_id: TraitId,
        self_ty: InternedTyId,
        trait_args: Vec<InternedTyId>,
    ) {
        self.traits.insert(trait_id);
        if !self.vtable_keys.insert(ReachableTraitVtableKey {
            module_id,
            trait_id,
            self_ty,
            trait_args: trait_args.clone(),
        }) {
            return;
        }
        self.vtables.push(ReachableTraitVtable {
            module_id,
            trait_id,
            self_ty,
            trait_args,
        });
    }

    fn needs_method(&self, trait_id: TraitId, method_name: &SymbolId) -> bool {
        self.methods
            .iter()
            .any(|method| method.trait_id == trait_id && &method.method_name == method_name)
            || self
                .vtables
                .iter()
                .any(|vtable| vtable.trait_id == trait_id)
    }
}

struct DeferredModuleActivation<'a> {
    reachable_functions: &'a mut HashSet<GlobalDefId>,
    reachable_modules: &'a HashSet<ModuleId>,
    pending_module_set: &'a mut HashSet<ModuleId>,
    pending_modules: &'a mut VecDeque<ModuleId>,
}

impl DeferredModuleActivation<'_> {
    fn is_reachable_module(&self, module_id: ModuleId) -> bool {
        self.reachable_modules.contains(&module_id)
    }

    fn add_function(
        &mut self,
        def_id: GlobalDefId,
        program_signatures: ExecutableSignatureIndex<'_>,
    ) {
        if !reachable_function_has_runtime_body(def_id, program_signatures) {
            self.add_module(def_id.module_id);
            return;
        }
        if self.reachable_functions.insert(def_id) {
            self.add_module(def_id.module_id);
        }
    }

    fn add_module(&mut self, module_id: ModuleId) {
        if !self.reachable_modules.contains(&module_id) && self.pending_module_set.insert(module_id)
        {
            self.pending_modules.push_back(module_id);
        }
    }
}

fn extend_reachable_functions_from_traits(
    program_signatures: ExecutableSignatureIndex<'_>,
    extension_index: &dyn ExecutableExtensionLookup,
    modules_by_id: &HashMap<ModuleId, ReachableModuleInput<'_>>,
    reachable_traits: &mut ReachableTraitRefs,
    reachability: &mut ExecutableReachability,
    pending_modules: &mut VecDeque<ModuleId>,
) {
    let Some(type_store) = modules_by_id
        .values()
        .next()
        .map(|module| module.type_store)
    else {
        return;
    };
    for vtable in reachable_traits.vtables.clone() {
        insert_trait_and_supertrait_methods(
            program_signatures,
            type_store,
            reachable_traits,
            vtable.module_id,
            vtable.trait_id,
            vtable.self_ty,
            &vtable.trait_args,
        );
    }
    let reachable_modules = &reachability.modules;
    let mut pending_module_set = HashSet::new();
    let mut deferred_modules = DeferredModuleActivation {
        reachable_functions: &mut reachability.functions,
        reachable_modules,
        pending_module_set: &mut pending_module_set,
        pending_modules,
    };
    for trait_id in &reachable_traits.traits {
        let TraitId::Source(trait_def) = trait_id else {
            continue;
        };
        if !deferred_modules.is_reachable_module(trait_def.module_id) {
            continue;
        }
        let Some(trait_signature) = (program_signatures.trait_)(*trait_def) else {
            continue;
        };
        for method in &trait_signature.signature.methods {
            if method.has_default && reachable_traits.needs_method(*trait_id, &method.name) {
                deferred_modules.add_function(
                    GlobalDefId {
                        module_id: trait_def.module_id,
                        def_id: method.def_id,
                    },
                    program_signatures,
                );
            }
        }
    }
    for vtable in &reachable_traits.vtables {
        extension_index.for_each_method_for_trait(vtable.trait_id, &mut |method| {
            if !with_reachable_extension_method_match(
                ReachableExtensionMatchInput {
                    method,
                    trait_id: vtable.trait_id,
                    self_ty: vtable.self_ty,
                    trait_args: &vtable.trait_args,
                    use_module_id: vtable.module_id,
                    type_store,
                    extension_index,
                    modules_by_id,
                },
                &mut |_| {
                    deferred_modules.add_function(method.def_id, program_signatures);
                },
            ) {}
        });
    }
    let mut method_index = 0;
    while method_index < reachable_traits.methods.len() {
        let mut discovered_traits = ReachableTraitRefs::default();
        {
            let reachable = &reachable_traits.methods[method_index];
            extension_index.for_each_method_for_trait_method(
                reachable.trait_id,
                &reachable.method_name,
                &mut |method| {
                    if !with_reachable_extension_method_match(
                        ReachableExtensionMatchInput {
                            method,
                            trait_id: reachable.trait_id,
                            self_ty: reachable.self_ty,
                            trait_args: &reachable.trait_args,
                            use_module_id: reachable.module_id,
                            type_store,
                            extension_index,
                            modules_by_id,
                        },
                        &mut |matched| {
                            deferred_modules.add_function(method.def_id, program_signatures);
                            extend_reachable_trait_methods_from_impl_where_predicates(
                                program_signatures,
                                type_store,
                                &matched,
                                &reachable.method_name,
                                reachable.module_id,
                                &mut discovered_traits,
                            );
                        },
                    ) {}
                },
            );
        }
        method_index += 1;
        reachable_traits.extend(discovered_traits);
    }
}

fn extend_reachable_functions_from_traits_incremental(
    state: &mut IncrementalExecutableReachability,
    program_signatures: ExecutableSignatureIndex<'_>,
    extension_index: &dyn ExecutableExtensionLookup,
    modules_by_id: &HashMap<ModuleId, ReachableModuleInput<'_>>,
    pending_modules: &mut VecDeque<ModuleId>,
) {
    if state.trait_function_scan.methods == state.reachable_traits.methods.len()
        && state.trait_function_scan.vtables == state.reachable_traits.vtables.len()
    {
        return;
    }
    let Some(type_store) = modules_by_id
        .values()
        .next()
        .map(|module| module.type_store)
    else {
        return;
    };

    let mut pending_module_set = HashSet::new();
    let mut deferred_modules = DeferredModuleActivation {
        reachable_functions: &mut state.reachability.functions,
        reachable_modules: &state.reachability.modules,
        pending_module_set: &mut pending_module_set,
        pending_modules,
    };
    let mut vtable_index = state
        .trait_function_scan
        .vtables
        .min(state.reachable_traits.vtables.len());
    while vtable_index < state.reachable_traits.vtables.len() {
        let vtable = state.reachable_traits.vtables[vtable_index].clone();
        insert_trait_and_supertrait_methods(
            program_signatures,
            type_store,
            &mut state.reachable_traits,
            vtable.module_id,
            vtable.trait_id,
            vtable.self_ty,
            &vtable.trait_args,
        );
        add_reachable_default_trait_methods_for_vtable(
            program_signatures,
            &vtable,
            &mut deferred_modules,
        );
        extension_index.for_each_method_for_trait(vtable.trait_id, &mut |method| {
            if !with_reachable_extension_method_match(
                ReachableExtensionMatchInput {
                    method,
                    trait_id: vtable.trait_id,
                    self_ty: vtable.self_ty,
                    trait_args: &vtable.trait_args,
                    use_module_id: vtable.module_id,
                    type_store,
                    extension_index,
                    modules_by_id,
                },
                &mut |_| {
                    deferred_modules.add_function(method.def_id, program_signatures);
                },
            ) {}
        });
        vtable_index += 1;
    }

    let mut method_index = state
        .trait_function_scan
        .methods
        .min(state.reachable_traits.methods.len());
    while method_index < state.reachable_traits.methods.len() {
        let mut discovered_traits = ReachableTraitRefs::default();
        {
            let reachable = &state.reachable_traits.methods[method_index];
            add_reachable_default_trait_method_for_method(
                program_signatures,
                reachable,
                &mut deferred_modules,
            );
            extension_index.for_each_method_for_trait_method(
                reachable.trait_id,
                &reachable.method_name,
                &mut |method| {
                    if !with_reachable_extension_method_match(
                        ReachableExtensionMatchInput {
                            method,
                            trait_id: reachable.trait_id,
                            self_ty: reachable.self_ty,
                            trait_args: &reachable.trait_args,
                            use_module_id: reachable.module_id,
                            type_store,
                            extension_index,
                            modules_by_id,
                        },
                        &mut |matched| {
                            deferred_modules.add_function(method.def_id, program_signatures);
                            extend_reachable_trait_methods_from_impl_where_predicates(
                                program_signatures,
                                type_store,
                                &matched,
                                &reachable.method_name,
                                reachable.module_id,
                                &mut discovered_traits,
                            );
                        },
                    ) {}
                },
            );
        }
        method_index += 1;
        state.reachable_traits.extend(discovered_traits);
    }

    state.trait_function_scan.vtables = vtable_index;
    state.trait_function_scan.methods = method_index;
}

fn add_reachable_default_trait_method_for_method(
    program_signatures: ExecutableSignatureIndex<'_>,
    reachable: &ReachableTraitMethod,
    deferred_modules: &mut DeferredModuleActivation<'_>,
) {
    let TraitId::Source(trait_def) = reachable.trait_id else {
        return;
    };
    let Some(trait_signature) = (program_signatures.trait_)(trait_def) else {
        return;
    };
    for method in &trait_signature.signature.methods {
        if method.has_default && method.name == reachable.method_name {
            deferred_modules.add_function(
                GlobalDefId {
                    module_id: trait_def.module_id,
                    def_id: method.def_id,
                },
                program_signatures,
            );
        }
    }
}

fn add_reachable_default_trait_methods_for_vtable(
    program_signatures: ExecutableSignatureIndex<'_>,
    vtable: &ReachableTraitVtable,
    deferred_modules: &mut DeferredModuleActivation<'_>,
) {
    let TraitId::Source(trait_def) = vtable.trait_id else {
        return;
    };
    let Some(trait_signature) = (program_signatures.trait_)(trait_def) else {
        return;
    };
    for method in &trait_signature.signature.methods {
        if method.has_default {
            deferred_modules.add_function(
                GlobalDefId {
                    module_id: trait_def.module_id,
                    def_id: method.def_id,
                },
                program_signatures,
            );
        }
    }
}

fn add_reachable_function(
    def_id: GlobalDefId,
    program_signatures: ExecutableSignatureIndex<'_>,
    reachability: &mut ExecutableReachability,
    pending_modules: &mut VecDeque<ModuleId>,
) {
    if !reachable_function_has_runtime_body(def_id, program_signatures) {
        reachability.insert_module_pending(def_id.module_id, pending_modules);
        return;
    }
    reachability.insert_function_pending(def_id, pending_modules);
}

fn reachable_function_has_runtime_body(
    def_id: GlobalDefId,
    program_signatures: ExecutableSignatureIndex<'_>,
) -> bool {
    (program_signatures.function)(def_id)
        .map(|signature| !signature.signature.is_const && signature.signature.has_body)
        .or_else(|| {
            (program_signatures.trait_default_method)(def_id).map(|(_, trait_signature)| {
                trait_signature
                    .signature
                    .methods
                    .iter()
                    .any(|method| method.def_id == def_id.def_id && method.has_default)
            })
        })
        .unwrap_or(false)
}
