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
use nia_imports::ModuleGraph;
use nia_item_signatures::{
    ProgramFunctionSignature, ProgramStructSignature, ProgramTraitImplSignature,
    ProgramTraitSignature, ProgramUnionSignature,
};
use nia_sema_ir::FunctionSemanticFacts;
use nia_symbol::{SymbolId, SymbolMap, known};
use nia_ty::{AssociatedTypeBindingTy, TyInterner, TyKind};

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ExecutableReachability {
    pub modules: HashSet<ModuleId>,
    pub type_modules: HashSet<ModuleId>,
    pub functions: HashSet<GlobalDefId>,
    pub globals: HashSet<GlobalDefId>,
    pub stats: ExecutableReachabilityStats,
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct ExecutableReachabilityStats {
    pub checked_modules: usize,
    pub checked_bodies: usize,
    pub reachable_bodies: usize,
}

pub struct ExecutableExtensionIndex<'a> {
    by_trait: HashMap<TraitId, Vec<&'a nia_defs::ExtensionMethod>>,
    by_trait_method: HashMap<(TraitId, SymbolId), Vec<&'a nia_defs::ExtensionMethod>>,
    where_predicates_by_def: HashMap<GlobalDefId, &'a [nia_defs::WherePredicateSignature]>,
    trait_impls_by_key: HashMap<(ModuleId, TraitImplId, TraitId), &'a ProgramTraitImplSignature>,
}

pub trait ExecutableExtensionLookup {
    fn for_each_method_for_trait(
        &self,
        trait_id: TraitId,
        f: &mut dyn FnMut(&nia_defs::ExtensionMethod),
    );

    fn for_each_method_for_trait_method(
        &self,
        trait_id: TraitId,
        method_name: &SymbolId,
        f: &mut dyn FnMut(&nia_defs::ExtensionMethod),
    );

    fn with_where_predicates_for_def(
        &self,
        def_id: GlobalDefId,
        f: &mut dyn FnMut(&[nia_defs::WherePredicateSignature]),
    );

    fn with_trait_impl_for_method(
        &self,
        method: &nia_defs::ExtensionMethod,
        trait_id: TraitId,
        f: &mut dyn FnMut(&ProgramTraitImplSignature),
    ) -> bool;
}

impl<'a> ExecutableExtensionIndex<'a> {
    pub fn new(
        extension_methods: &'a ExtensionMethods,
        trait_impls: &'a [ProgramTraitImplSignature],
    ) -> Self {
        let mut by_trait = HashMap::<TraitId, Vec<&'a nia_defs::ExtensionMethod>>::new();
        let mut by_trait_method =
            HashMap::<(TraitId, SymbolId), Vec<&'a nia_defs::ExtensionMethod>>::new();
        let mut where_predicates_by_def =
            HashMap::<GlobalDefId, &'a [nia_defs::WherePredicateSignature]>::new();
        let trait_impls_by_key = trait_impls
            .iter()
            .map(|impl_signature| {
                (
                    (
                        impl_signature.module_id,
                        impl_signature.impl_id,
                        impl_signature.trait_id,
                    ),
                    impl_signature,
                )
            })
            .collect::<HashMap<_, _>>();
        for method in extension_methods.all_methods() {
            where_predicates_by_def.insert(method.def_id, method.where_predicates.as_slice());
            if let Some(trait_id) = method.trait_id {
                by_trait.entry(trait_id).or_default().push(method);
                by_trait_method
                    .entry((trait_id, method.name.clone()))
                    .or_default()
                    .push(method);
            }
        }
        Self {
            by_trait,
            by_trait_method,
            where_predicates_by_def,
            trait_impls_by_key,
        }
    }
}

impl ExecutableExtensionLookup for ExecutableExtensionIndex<'_> {
    fn for_each_method_for_trait(
        &self,
        trait_id: TraitId,
        f: &mut dyn FnMut(&nia_defs::ExtensionMethod),
    ) {
        if let Some(methods) = self.by_trait.get(&trait_id) {
            for method in methods {
                f(method);
            }
        }
    }

    fn for_each_method_for_trait_method(
        &self,
        trait_id: TraitId,
        method_name: &SymbolId,
        f: &mut dyn FnMut(&nia_defs::ExtensionMethod),
    ) {
        if let Some(methods) = self.by_trait_method.get(&(trait_id, *method_name)) {
            for method in methods {
                f(method);
            }
        }
    }

    fn with_where_predicates_for_def(
        &self,
        def_id: GlobalDefId,
        f: &mut dyn FnMut(&[nia_defs::WherePredicateSignature]),
    ) {
        let predicates = self
            .where_predicates_by_def
            .get(&def_id)
            .copied()
            .unwrap_or(&[]);
        f(predicates);
    }

    fn with_trait_impl_for_method(
        &self,
        method: &nia_defs::ExtensionMethod,
        trait_id: TraitId,
        f: &mut dyn FnMut(&ProgramTraitImplSignature),
    ) -> bool {
        let Some(signature) =
            self.trait_impls_by_key
                .get(&(method.def_id.module_id, method.impl_id, trait_id))
        else {
            return false;
        };
        f(signature);
        true
    }
}

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

    pub fn replace_reachability(&mut self, reachability: ExecutableReachability) {
        self.reachability = reachability;
    }

    pub fn into_reachability(self) -> ExecutableReachability {
        self.reachability
    }
}

#[derive(Clone, Copy)]
pub struct ExecutableRootDefs<'a> {
    pub named_function: &'a dyn Fn(ModuleId, SymbolId) -> Option<GlobalDefId>,
    pub module_functions: &'a dyn Fn(ModuleId) -> Vec<GlobalDefId>,
}

impl std::fmt::Debug for ExecutableRootDefs<'_> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ExecutableRootDefs")
            .field("named_function", &true)
            .field("module_functions", &true)
            .finish()
    }
}

pub fn compute_executable_reachability(
    parse_ok: &[ModuleId],
    graph: &ModuleGraph,
    root_defs: ExecutableRootDefs<'_>,
    program_signatures: ExecutableSignatureIndex<'_>,
    extension_methods: &ExtensionMethods,
    trait_impls: &[ProgramTraitImplSignature],
    modules: &[ReachableModuleInput<'_>],
) -> ExecutableReachability {
    compute_executable_reachability_with_seed(
        None,
        parse_ok,
        graph,
        root_defs,
        program_signatures,
        extension_methods,
        trait_impls,
        modules,
    )
}

pub fn compute_executable_reachability_with_seed(
    seed: Option<&ExecutableReachability>,
    parse_ok: &[ModuleId],
    graph: &ModuleGraph,
    root_defs: ExecutableRootDefs<'_>,
    program_signatures: ExecutableSignatureIndex<'_>,
    extension_methods: &ExtensionMethods,
    trait_impls: &[ProgramTraitImplSignature],
    modules: &[ReachableModuleInput<'_>],
) -> ExecutableReachability {
    let extension_index = ExecutableExtensionIndex::new(extension_methods, trait_impls);
    compute_executable_reachability_with_seed_and_extension_index(
        seed,
        parse_ok,
        graph,
        root_defs,
        program_signatures,
        &extension_index,
        modules,
    )
}

pub fn compute_executable_reachability_with_seed_and_extension_index(
    seed: Option<&ExecutableReachability>,
    parse_ok: &[ModuleId],
    graph: &ModuleGraph,
    root_defs: ExecutableRootDefs<'_>,
    program_signatures: ExecutableSignatureIndex<'_>,
    extension_index: &dyn ExecutableExtensionLookup,
    modules: &[ReachableModuleInput<'_>],
) -> ExecutableReachability {
    let modules_by_id = modules
        .iter()
        .map(|module| (module.module_id, *module))
        .collect::<HashMap<_, _>>();
    let mut reachable_functions = HashSet::new();
    let mut reachable_globals = seed.map(|seed| seed.globals.clone()).unwrap_or_default();
    let mut reachable_modules = seed.map(|seed| seed.modules.clone()).unwrap_or_default();
    let mut reachable_type_modules = seed
        .map(|seed| seed.type_modules.clone())
        .unwrap_or_default();
    let mut pending_seed_modules = VecDeque::new();
    for def_id in seed
        .into_iter()
        .flat_map(|seed| seed.functions.iter().copied())
        .chain(executable_root_functions(graph, root_defs))
    {
        add_reachable_function(
            def_id,
            program_signatures,
            &mut reachable_functions,
            &mut reachable_modules,
            &mut pending_seed_modules,
        );
    }
    reachable_modules.extend(reachable_functions.iter().map(|def_id| def_id.module_id));
    add_reachable_module(graph.entry(), &mut reachable_modules, &mut VecDeque::new());

    let parse_ok_set = parse_ok.iter().copied().collect::<HashSet<_>>();
    loop {
        let before = (
            reachable_functions.len(),
            reachable_globals.len(),
            reachable_modules.len(),
            reachable_type_modules.len(),
        );
        let current_reachable_modules = reachable_modules.clone();
        let mut reachable_traits = collect_reachable_traits_for_modules(
            &modules_by_id,
            &current_reachable_modules,
            &reachable_functions,
            &reachable_globals,
        );
        extend_reachable_traits_from_generic_instances(
            &modules_by_id,
            &current_reachable_modules,
            program_signatures,
            extension_index,
            &reachable_functions,
            &mut reachable_traits,
        );
        for module in modules_by_id
            .values()
            .filter(|module| current_reachable_modules.contains(&module.module_id))
        {
            let mut pending_modules = VecDeque::new();
            extend_reachable_functions_from_bodies(
                module,
                program_signatures,
                &mut reachable_functions,
                &mut reachable_globals,
                &mut reachable_modules,
                &mut pending_modules,
            );
            collect_reachable_fact_owner_modules(
                module,
                program_signatures,
                &reachable_functions,
                &reachable_globals,
                &mut reachable_type_modules,
                &mut reachable_traits,
            );
        }
        let mut pending_modules = VecDeque::new();
        extend_reachable_functions_from_traits(
            program_signatures,
            extension_index,
            &modules_by_id,
            &mut reachable_traits,
            &reachable_modules,
            &mut reachable_functions,
            &mut pending_modules,
        );
        while let Some(module_id) = pending_modules.pop_front() {
            if !parse_ok_set.contains(&module_id) {
                continue;
            }
            reachable_modules.insert(module_id);
        }
        if before
            == (
                reachable_functions.len(),
                reachable_globals.len(),
                reachable_modules.len(),
                reachable_type_modules.len(),
            )
        {
            break;
        }
    }

    let stats = reachability_stats(&modules_by_id, &reachable_functions);

    ExecutableReachability {
        modules: reachable_modules,
        type_modules: reachable_type_modules,
        functions: reachable_functions,
        globals: reachable_globals,
        stats,
    }
}

pub fn compute_executable_reachability_incremental(
    state: &mut IncrementalExecutableReachability,
    parse_ok: &[ModuleId],
    graph: &ModuleGraph,
    root_defs: ExecutableRootDefs<'_>,
    program_signatures: ExecutableSignatureIndex<'_>,
    extension_methods: &ExtensionMethods,
    trait_impls: &[ProgramTraitImplSignature],
    modules: &[ReachableModuleInput<'_>],
) -> ExecutableReachability {
    let extension_index = ExecutableExtensionIndex::new(extension_methods, trait_impls);
    compute_executable_reachability_incremental_with_extension_index(
        state,
        parse_ok,
        graph,
        root_defs,
        program_signatures,
        &extension_index,
        modules,
    )
}

pub fn compute_executable_reachability_incremental_with_extension_index(
    state: &mut IncrementalExecutableReachability,
    parse_ok: &[ModuleId],
    graph: &ModuleGraph,
    root_defs: ExecutableRootDefs<'_>,
    program_signatures: ExecutableSignatureIndex<'_>,
    extension_index: &dyn ExecutableExtensionLookup,
    modules: &[ReachableModuleInput<'_>],
) -> ExecutableReachability {
    compute_executable_reachability_incremental_with_timings(
        state,
        parse_ok,
        graph,
        root_defs,
        program_signatures,
        extension_index,
        modules,
        nia_timing::TimingMode::Off,
    )
}

#[expect(clippy::too_many_arguments)]
pub fn compute_executable_reachability_incremental_with_timings(
    state: &mut IncrementalExecutableReachability,
    parse_ok: &[ModuleId],
    graph: &ModuleGraph,
    root_defs: ExecutableRootDefs<'_>,
    program_signatures: ExecutableSignatureIndex<'_>,
    extension_index: &dyn ExecutableExtensionLookup,
    modules: &[ReachableModuleInput<'_>],
    timings: nia_timing::TimingMode,
) -> ExecutableReachability {
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
        for def_id in executable_root_functions(graph, root_defs) {
            add_reachable_function(
                def_id,
                program_signatures,
                &mut state.reachability.functions,
                &mut state.reachability.modules,
                &mut pending_modules,
            );
        }
        add_reachable_module(
            graph.entry(),
            &mut state.reachability.modules,
            &mut pending_modules,
        );
    });

    loop {
        let before = incremental_reachability_key(state);
        let current_reachable_modules =
            time_reachability_stage(timings, "incremental.current_modules", None, || {
                modules_by_id
                    .keys()
                    .copied()
                    .filter(|module_id| state.reachability.modules.contains(module_id))
                    .collect::<HashSet<_>>()
            });
        for module in modules_by_id
            .values()
            .filter(|module| current_reachable_modules.contains(&module.module_id))
        {
            let mut pending_modules = VecDeque::new();
            time_reachability_stage(
                timings,
                "incremental.unscanned_items",
                Some(module.module_id),
                || {
                    extend_reachability_from_unscanned_items(
                        state,
                        module,
                        program_signatures,
                        &mut pending_modules,
                    );
                },
            );
            while let Some(module_id) = pending_modules.pop_front() {
                if parse_ok_set.contains(&module_id) {
                    state.reachability.modules.insert(module_id);
                }
            }
        }
        let current_reachable_modules =
            time_reachability_stage(timings, "incremental.current_modules", None, || {
                modules_by_id
                    .keys()
                    .copied()
                    .filter(|module_id| state.reachability.modules.contains(module_id))
                    .collect::<HashSet<_>>()
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
                state.reachability.modules.insert(module_id);
            }
        }
        if before == incremental_reachability_key(state) {
            break;
        }
    }

    state.reachability.stats = reachability_stats(&modules_by_id, &state.reachability.functions);
    state.reachability.clone()
}

pub fn extend_incremental_executable_reachability_from_checked_module(
    state: &mut IncrementalExecutableReachability,
    parse_ok: &[ModuleId],
    program_signatures: ExecutableSignatureIndex<'_>,
    extension_methods: &ExtensionMethods,
    trait_impls: &[ProgramTraitImplSignature],
    module: ReachableModuleInput<'_>,
    checked_functions: &HashSet<GlobalDefId>,
    modules_by_id: &HashMap<ModuleId, ReachableModuleInput<'_>>,
) -> ExecutableReachability {
    let extension_index = ExecutableExtensionIndex::new(extension_methods, trait_impls);
    extend_incremental_executable_reachability_from_checked_module_with_extension_index(
        state,
        parse_ok,
        program_signatures,
        &extension_index,
        module,
        checked_functions,
        modules_by_id,
    )
}

pub fn extend_incremental_executable_reachability_from_checked_module_with_extension_index(
    state: &mut IncrementalExecutableReachability,
    parse_ok: &[ModuleId],
    program_signatures: ExecutableSignatureIndex<'_>,
    extension_index: &dyn ExecutableExtensionLookup,
    module: ReachableModuleInput<'_>,
    checked_functions: &HashSet<GlobalDefId>,
    modules_by_id: &HashMap<ModuleId, ReachableModuleInput<'_>>,
) -> ExecutableReachability {
    extend_incremental_executable_reachability_from_checked_module_with_timings(
        state,
        parse_ok,
        program_signatures,
        extension_index,
        module,
        checked_functions,
        modules_by_id,
        nia_timing::TimingMode::Off,
    )
}

#[expect(clippy::too_many_arguments)]
pub fn extend_incremental_executable_reachability_from_checked_module_with_timings(
    state: &mut IncrementalExecutableReachability,
    parse_ok: &[ModuleId],
    program_signatures: ExecutableSignatureIndex<'_>,
    extension_index: &dyn ExecutableExtensionLookup,
    module: ReachableModuleInput<'_>,
    checked_functions: &HashSet<GlobalDefId>,
    modules_by_id: &HashMap<ModuleId, ReachableModuleInput<'_>>,
    timings: nia_timing::TimingMode,
) -> ExecutableReachability {
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
                state.reachability.modules.insert(module_id);
            }
        }
        let current_reachable_modules = time_reachability_stage(
            timings,
            "incremental.current_modules",
            Some(module.module_id),
            || {
                modules_by_id
                    .keys()
                    .copied()
                    .filter(|module_id| state.reachability.modules.contains(module_id))
                    .collect::<HashSet<_>>()
            },
        );
        time_reachability_stage(
            timings,
            "incremental.generic_traits",
            Some(module.module_id),
            || {
                extend_reachable_traits_from_generic_instances_incremental(
                    state,
                    &modules_by_id,
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
                    &modules_by_id,
                    &mut pending_modules,
                );
            },
        );
        while let Some(module_id) = pending_modules.pop_front() {
            if parse_ok_set.contains(&module_id) {
                state.reachability.modules.insert(module_id);
            }
        }

        if before == incremental_reachability_key(state) {
            break;
        }
    }

    state.reachability.clone()
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
    for def_id in refs.functions {
        add_reachable_function(
            def_id,
            program_signatures,
            &mut state.reachability.functions,
            &mut state.reachability.modules,
            pending_modules,
        );
    }
    for def_id in refs.globals {
        if state.reachability.globals.insert(def_id) {
            add_reachable_module(
                def_id.module_id,
                &mut state.reachability.modules,
                pending_modules,
            );
        }
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
    let before = (
        reachability.functions.len(),
        reachability.globals.len(),
        reachability.modules.len(),
        reachability.type_modules.len(),
    );
    let mut pending_modules = VecDeque::new();
    extend_reachable_functions_from_bodies(
        &module,
        program_signatures,
        &mut reachability.functions,
        &mut reachability.globals,
        &mut reachability.modules,
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
    let current_reachable_modules = modules_by_id
        .keys()
        .copied()
        .filter(|module_id| reachability.modules.contains(module_id))
        .collect::<HashSet<_>>();
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
        &reachability.modules,
        &mut reachability.functions,
        &mut pending_modules,
    );
    before
        != (
            reachability.functions.len(),
            reachability.globals.len(),
            reachability.modules.len(),
            reachability.type_modules.len(),
        )
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

fn executable_root_functions(
    graph: &ModuleGraph,
    root_defs: ExecutableRootDefs<'_>,
) -> HashSet<GlobalDefId> {
    let mut roots = HashSet::new();
    if let Some(main) = (root_defs.named_function)(graph.entry(), known::MAIN) {
        roots.insert(main);
    }
    if let Some(start_module) = freestanding_start_module(graph)
        && let Some(start) = (root_defs.named_function)(start_module, known::START_ENTRY)
    {
        roots.insert(start);
        roots.extend((root_defs.module_functions)(start_module));
    }
    roots
}

fn extend_reachable_functions_from_bodies(
    module: &ReachableModuleInput<'_>,
    program_signatures: ExecutableSignatureIndex<'_>,
    reachable_functions: &mut HashSet<GlobalDefId>,
    reachable_globals: &mut HashSet<GlobalDefId>,
    reachable_modules: &mut HashSet<ModuleId>,
    pending_modules: &mut VecDeque<ModuleId>,
) {
    let refs = typed_executable_refs(module, reachable_functions, reachable_globals);
    for def_id in refs.functions {
        add_reachable_function(
            def_id,
            program_signatures,
            reachable_functions,
            reachable_modules,
            pending_modules,
        );
    }
    for def_id in refs.globals {
        if reachable_globals.insert(def_id) {
            add_reachable_module(def_id.module_id, reachable_modules, pending_modules);
        }
    }
}

fn collect_reachable_traits_for_modules(
    modules_by_id: &HashMap<ModuleId, ReachableModuleInput<'_>>,
    current_reachable_modules: &HashSet<ModuleId>,
    reachable_functions: &HashSet<GlobalDefId>,
    reachable_globals: &HashSet<GlobalDefId>,
) -> ReachableTraitRefs {
    let mut reachable_traits = ReachableTraitRefs::default();
    for module in modules_by_id
        .values()
        .filter(|module| current_reachable_modules.contains(&module.module_id))
    {
        collect_reachable_body_trait_ids(
            module,
            reachable_functions,
            reachable_globals,
            &mut reachable_traits,
        );
    }
    reachable_traits
}

fn extend_reachable_traits_from_generic_instances(
    modules_by_id: &HashMap<ModuleId, ReachableModuleInput<'_>>,
    current_reachable_modules: &HashSet<ModuleId>,
    program_signatures: ExecutableSignatureIndex<'_>,
    extension_index: &dyn ExecutableExtensionLookup,
    reachable_functions: &HashSet<GlobalDefId>,
    traits: &mut ReachableTraitRefs,
) {
    for def_id in reachable_functions {
        if !current_reachable_modules.contains(&def_id.module_id) {
            continue;
        }
        let Some(module) = modules_by_id.get(&def_id.module_id) else {
            continue;
        };
        let mut executable_refs =
            typed_executable_refs_for_items(module, &HashSet::from([*def_id]), &HashSet::new());
        for instantiation in executable_refs.generic_instantiations.drain(..) {
            let mut visited = HashSet::new();
            extend_reachable_traits_from_generic_instantiation(
                module.module_id,
                &module.body_ir.interner,
                modules_by_id,
                program_signatures,
                extension_index,
                traits,
                &instantiation,
                &mut visited,
            );
        }
        let Some(function_facts) = module.semantic_facts.function_facts.get(def_id) else {
            continue;
        };
        for instantiation in &function_facts.generic_instantiations {
            let mut visited = HashSet::new();
            extend_reachable_traits_from_generic_instantiation(
                module.module_id,
                &module.body_ir.interner,
                modules_by_id,
                program_signatures,
                extension_index,
                traits,
                instantiation,
                &mut visited,
            );
        }
    }
}

fn extend_reachable_traits_from_generic_instances_incremental(
    state: &mut IncrementalExecutableReachability,
    modules_by_id: &HashMap<ModuleId, ReachableModuleInput<'_>>,
    current_reachable_modules: &HashSet<ModuleId>,
    program_signatures: ExecutableSignatureIndex<'_>,
    extension_index: &dyn ExecutableExtensionLookup,
) {
    let pending_functions = state
        .reachability
        .functions
        .iter()
        .copied()
        .filter(|def_id| current_reachable_modules.contains(&def_id.module_id))
        .filter(|def_id| !state.scanned_generic_trait_functions.contains(def_id))
        .collect::<Vec<_>>();
    for def_id in pending_functions {
        let Some(module) = modules_by_id.get(&def_id.module_id) else {
            continue;
        };
        let Some(function_facts) = module.semantic_facts.function_facts.get(&def_id) else {
            continue;
        };
        state.scanned_generic_trait_functions.insert(def_id);
        let mut executable_refs =
            typed_executable_refs_for_items(module, &HashSet::from([def_id]), &HashSet::new());
        for instantiation in executable_refs.generic_instantiations.drain(..) {
            let mut visited = HashSet::new();
            extend_reachable_traits_from_generic_instantiation(
                module.module_id,
                &module.body_ir.interner,
                modules_by_id,
                program_signatures,
                extension_index,
                &mut state.reachable_traits,
                &instantiation,
                &mut visited,
            );
        }
        for instantiation in &function_facts.generic_instantiations {
            let mut visited = HashSet::new();
            extend_reachable_traits_from_generic_instantiation(
                module.module_id,
                &module.body_ir.interner,
                modules_by_id,
                program_signatures,
                extension_index,
                &mut state.reachable_traits,
                instantiation,
                &mut visited,
            );
        }
    }
}

fn extend_reachable_traits_from_generic_instantiation(
    use_module_id: ModuleId,
    arg_interner: &TyInterner,
    modules_by_id: &HashMap<ModuleId, ReachableModuleInput<'_>>,
    program_signatures: ExecutableSignatureIndex<'_>,
    extension_index: &dyn ExecutableExtensionLookup,
    traits: &mut ReachableTraitRefs,
    instantiation: &nia_sema_ir::GenericInstantiation,
    visited: &mut HashSet<ReachableGenericInstantiationKey>,
) {
    if !visited.insert(reachable_generic_instantiation_key(
        arg_interner,
        instantiation,
    )) {
        return;
    }
    extend_reachable_traits_from_trait_default_instantiation(
        use_module_id,
        arg_interner,
        program_signatures,
        traits,
        instantiation,
    );
    let Some(signature) = (program_signatures.function)(instantiation.def_id) else {
        return;
    };
    let mut signature_interner = signature.interner.clone();
    let generics = if instantiation.generics.is_empty() && !instantiation.args.is_empty() {
        &signature.signature.generics
    } else {
        &instantiation.generics
    };
    let generic_substitutions = generics
        .iter()
        .cloned()
        .zip(instantiation.args.iter().copied())
        .filter_map(|(generic, arg)| {
            nia_ty::try_import_type_into(&mut signature_interner, arg_interner, arg)
                .ok()
                .map(|arg| (generic, arg))
        })
        .collect::<SymbolMap<_>>();
    let self_ty = instantiation.self_arg.and_then(|self_arg| {
        nia_ty::try_import_type_into(&mut signature_interner, arg_interner, self_arg).ok()
    });
    let substitutions = TypeSubstitutions {
        self_ty,
        generics: &generic_substitutions,
    };
    for predicate in &signature.signature.where_predicates {
        let mut substituted_interner = signature_interner.clone();
        let Some(self_ty) = substitute_ty(&mut substituted_interner, predicate.ty, &substitutions)
        else {
            continue;
        };
        for bound in &predicate.bounds {
            let Some(trait_ty) =
                substitute_ty(&mut substituted_interner, bound.trait_ty, &substitutions)
            else {
                continue;
            };
            let Some((trait_id, trait_args)) = trait_id_and_args(&substituted_interner, trait_ty)
            else {
                continue;
            };
            match trait_id {
                TraitId::Source(trait_def) => {
                    if let Some(trait_signature) = (program_signatures.trait_)(trait_def) {
                        for method in &trait_signature.signature.methods {
                            traits.insert_method_with_interner(
                                use_module_id,
                                trait_id,
                                method.name.clone(),
                                self_ty,
                                trait_args.clone(),
                                Some(substituted_interner.clone()),
                            );
                        }
                    }
                }
                TraitId::Builtin(builtin_trait) => {
                    for method in builtin_trait.required_methods() {
                        if let Some(method_name) = builtin_trait_method_symbol(*method) {
                            traits.insert_method_with_interner(
                                use_module_id,
                                trait_id,
                                method_name,
                                self_ty,
                                trait_args.clone(),
                                Some(substituted_interner.clone()),
                            );
                        }
                    }
                }
            }
        }
    }
    extension_index.with_where_predicates_for_def(instantiation.def_id, &mut |predicates| {
        for predicate in predicates {
            let mut substituted_interner = signature_interner.clone();
            let Some(self_ty) =
                substitute_ty(&mut substituted_interner, predicate.ty, &substitutions)
            else {
                continue;
            };
            for bound in &predicate.bounds {
                let Some(trait_ty) =
                    substitute_ty(&mut substituted_interner, bound.trait_ty, &substitutions)
                else {
                    continue;
                };
                let Some((trait_id, trait_args)) =
                    trait_id_and_args(&substituted_interner, trait_ty)
                else {
                    continue;
                };
                match trait_id {
                    TraitId::Source(trait_def) => {
                        if let Some(trait_signature) = (program_signatures.trait_)(trait_def) {
                            for method in &trait_signature.signature.methods {
                                traits.insert_method_with_interner(
                                    use_module_id,
                                    trait_id,
                                    method.name,
                                    self_ty,
                                    trait_args.clone(),
                                    Some(substituted_interner.clone()),
                                );
                            }
                        }
                    }
                    TraitId::Builtin(builtin_trait) => {
                        for method in builtin_trait.required_methods() {
                            if let Some(method_name) = builtin_trait_method_symbol(*method) {
                                traits.insert_method_with_interner(
                                    use_module_id,
                                    trait_id,
                                    method_name,
                                    self_ty,
                                    trait_args.clone(),
                                    Some(substituted_interner.clone()),
                                );
                            }
                        }
                    }
                }
            }
        }
    });
    let Some(target_module) = modules_by_id.get(&instantiation.def_id.module_id) else {
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
        let mut nested_interner = signature_interner.clone();
        let Some(nested_instantiation) = instantiate_nested_generic_instantiation(
            &mut nested_interner,
            &target_module.body_ir.interner,
            nested,
            &substitutions,
        ) else {
            continue;
        };
        extend_reachable_traits_from_generic_instantiation(
            use_module_id,
            &nested_interner,
            modules_by_id,
            program_signatures,
            extension_index,
            traits,
            &nested_instantiation,
            visited,
        );
    }
}

fn extend_reachable_traits_from_trait_default_instantiation(
    use_module_id: ModuleId,
    arg_interner: &TyInterner,
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
    let mut method_interner = trait_signature.interner.clone();
    let Ok(self_ty) = nia_ty::try_import_type_into(&mut method_interner, arg_interner, self_ty)
    else {
        return;
    };
    let trait_args = instantiation
        .args
        .iter()
        .take(trait_signature.signature.generics.len())
        .map(|arg| nia_ty::try_import_type_into(&mut method_interner, arg_interner, *arg))
        .collect::<Result<Vec<_>, _>>();
    let Ok(trait_args) = trait_args else {
        return;
    };
    for method in &trait_signature.signature.methods {
        traits.insert_method_with_interner(
            use_module_id,
            trait_id,
            method.name.clone(),
            self_ty,
            trait_args.clone(),
            Some(method_interner.clone()),
        );
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
struct ReachableGenericInstantiationKey {
    def_id: GlobalDefId,
    args: Vec<Option<TyKind>>,
    const_args: Vec<nia_ty::ConstGenericArg>,
}

fn reachable_generic_instantiation_key(
    interner: &TyInterner,
    instantiation: &nia_sema_ir::GenericInstantiation,
) -> ReachableGenericInstantiationKey {
    ReachableGenericInstantiationKey {
        def_id: instantiation.def_id,
        args: instantiation
            .args
            .iter()
            .map(|arg| interner.get(*arg).cloned())
            .collect(),
        const_args: instantiation.const_args.clone(),
    }
}

fn instantiate_nested_generic_instantiation(
    target_interner: &mut TyInterner,
    source_interner: &TyInterner,
    instantiation: &nia_sema_ir::GenericInstantiation,
    substitutions: &TypeSubstitutions<'_>,
) -> Option<nia_sema_ir::GenericInstantiation> {
    let self_arg = match instantiation.self_arg {
        Some(self_arg) => {
            let imported =
                nia_ty::try_import_type_into(target_interner, source_interner, self_arg).ok()?;
            Some(substitute_ty(target_interner, imported, substitutions)?)
        }
        None => None,
    };
    let args = instantiation
        .args
        .iter()
        .map(|arg| {
            let imported =
                nia_ty::try_import_type_into(target_interner, source_interner, *arg).ok()?;
            substitute_ty(target_interner, imported, substitutions)
        })
        .collect::<Option<Vec<_>>>()?;
    let const_args = instantiation
        .const_args
        .iter()
        .cloned()
        .map(|mut arg| {
            arg.ty = nia_ty::try_import_type_into(target_interner, source_interner, arg.ty).ok()?;
            arg.ty = substitute_ty(target_interner, arg.ty, substitutions)?;
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
    typed_executable_refs_from_executable_refs(module, refs)
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

fn typed_executable_refs_from_executable_refs(
    _module: &ReachableModuleInput<'_>,
    refs: ExecutableItemRefs,
) -> TypedExecutableRefs {
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
    raw_method_keys: HashSet<ReachableTraitRawMethodKey>,
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
    interner: Option<TyInterner>,
}

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
struct ReachableTraitRawMethodKey {
    module_id: ModuleId,
    trait_id: TraitId,
    method_name: SymbolId,
    self_ty: InternedTyId,
    trait_args: Vec<InternedTyId>,
}

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
struct ReachableTraitMethodKey {
    trait_id: TraitId,
    method_name: SymbolId,
    self_ty: TyKind,
    trait_args: Vec<TyKind>,
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

impl ReachableTraitRefs {
    fn extend(&mut self, refs: Self) {
        self.traits.extend(refs.traits);
        for method in refs.methods {
            self.insert_method_with_interner(
                method.module_id,
                method.trait_id,
                method.method_name,
                method.self_ty,
                method.trait_args,
                method.interner,
            );
        }
        for vtable in refs.vtables {
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
        self.insert_method_with_interner(
            module_id,
            trait_id,
            method_name,
            self_ty,
            trait_args,
            None,
        );
    }

    fn insert_method_with_interner(
        &mut self,
        module_id: ModuleId,
        trait_id: TraitId,
        method_name: SymbolId,
        self_ty: InternedTyId,
        trait_args: Vec<InternedTyId>,
        interner: Option<TyInterner>,
    ) {
        self.traits.insert(trait_id);
        let key_interner = interner.as_ref();
        let key_interner = match key_interner {
            Some(interner) => interner,
            None => {
                if !self.raw_method_keys.insert(ReachableTraitRawMethodKey {
                    module_id,
                    trait_id,
                    method_name: method_name.clone(),
                    self_ty,
                    trait_args: trait_args.clone(),
                }) {
                    return;
                }
                return self.methods.push(ReachableTraitMethod {
                    module_id,
                    trait_id,
                    method_name,
                    self_ty,
                    trait_args,
                    interner,
                });
            }
        };
        let Some(self_ty_key) = key_interner.get(self_ty).cloned() else {
            return;
        };
        let Some(trait_arg_keys) = trait_args
            .iter()
            .map(|arg| key_interner.get(*arg).cloned())
            .collect::<Option<Vec<_>>>()
        else {
            return;
        };
        if !self.method_keys.insert(ReachableTraitMethodKey {
            trait_id,
            method_name: method_name.clone(),
            self_ty: self_ty_key,
            trait_args: trait_arg_keys,
        }) {
            return;
        }
        self.methods.push(ReachableTraitMethod {
            module_id,
            trait_id,
            method_name,
            self_ty,
            trait_args,
            interner,
        });
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

fn extend_reachable_functions_from_traits(
    program_signatures: ExecutableSignatureIndex<'_>,
    extension_index: &dyn ExecutableExtensionLookup,
    modules_by_id: &HashMap<ModuleId, ReachableModuleInput<'_>>,
    reachable_traits: &mut ReachableTraitRefs,
    reachable_modules: &HashSet<ModuleId>,
    reachable_functions: &mut HashSet<GlobalDefId>,
    pending_modules: &mut VecDeque<ModuleId>,
) {
    let mut modules = reachable_modules.clone();
    for trait_id in &reachable_traits.traits {
        let TraitId::Source(trait_def) = trait_id else {
            continue;
        };
        if !reachable_modules.contains(&trait_def.module_id) {
            continue;
        }
        let Some(trait_signature) = (program_signatures.trait_)(*trait_def) else {
            continue;
        };
        for method in &trait_signature.signature.methods {
            if method.has_default && reachable_traits.needs_method(*trait_id, &method.name) {
                add_reachable_function(
                    GlobalDefId {
                        module_id: trait_def.module_id,
                        def_id: method.def_id,
                    },
                    program_signatures,
                    reachable_functions,
                    &mut modules,
                    pending_modules,
                );
            }
        }
    }
    for vtable in &reachable_traits.vtables {
        extension_index.for_each_method_for_trait(vtable.trait_id, &mut |method| {
            if reachable_extension_method_match(
                method,
                vtable.trait_id,
                vtable.self_ty,
                &vtable.trait_args,
                vtable.module_id,
                None,
                extension_index,
                modules_by_id,
            )
            .is_none()
            {
                return;
            }
            add_reachable_function(
                method.def_id,
                program_signatures,
                reachable_functions,
                &mut modules,
                pending_modules,
            );
        });
    }
    let mut method_index = 0;
    while method_index < reachable_traits.methods.len() {
        let reachable = reachable_traits.methods[method_index].clone();
        method_index += 1;
        extension_index.for_each_method_for_trait_method(
            reachable.trait_id,
            &reachable.method_name,
            &mut |method| {
                let Some(matched) = reachable_extension_method_match(
                    method,
                    reachable.trait_id,
                    reachable.self_ty,
                    &reachable.trait_args,
                    reachable.module_id,
                    reachable.interner.as_ref(),
                    extension_index,
                    modules_by_id,
                ) else {
                    return;
                };
                add_reachable_function(
                    method.def_id,
                    program_signatures,
                    reachable_functions,
                    &mut modules,
                    pending_modules,
                );
                extend_reachable_trait_methods_from_impl_where_predicates(
                    program_signatures,
                    &matched,
                    &reachable.method_name,
                    reachable.module_id,
                    reachable_traits,
                );
            },
        );
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

    let mut modules = state.reachability.modules.clone();
    let mut vtable_index = state
        .trait_function_scan
        .vtables
        .min(state.reachable_traits.vtables.len());
    while vtable_index < state.reachable_traits.vtables.len() {
        let vtable = state.reachable_traits.vtables[vtable_index].clone();
        vtable_index += 1;
        add_reachable_default_trait_methods_for_vtable(
            program_signatures,
            &vtable,
            &state.reachability.modules,
            &mut state.reachability.functions,
            &mut modules,
            pending_modules,
        );
        extension_index.for_each_method_for_trait(vtable.trait_id, &mut |method| {
            if reachable_extension_method_match(
                method,
                vtable.trait_id,
                vtable.self_ty,
                &vtable.trait_args,
                vtable.module_id,
                None,
                extension_index,
                modules_by_id,
            )
            .is_none()
            {
                return;
            }
            add_reachable_function(
                method.def_id,
                program_signatures,
                &mut state.reachability.functions,
                &mut modules,
                pending_modules,
            );
        });
    }

    let mut method_index = state
        .trait_function_scan
        .methods
        .min(state.reachable_traits.methods.len());
    while method_index < state.reachable_traits.methods.len() {
        let reachable = state.reachable_traits.methods[method_index].clone();
        method_index += 1;
        add_reachable_default_trait_method_for_method(
            program_signatures,
            &reachable,
            &state.reachability.modules,
            &mut state.reachability.functions,
            &mut modules,
            pending_modules,
        );
        extension_index.for_each_method_for_trait_method(
            reachable.trait_id,
            &reachable.method_name,
            &mut |method| {
                let Some(matched) = reachable_extension_method_match(
                    method,
                    reachable.trait_id,
                    reachable.self_ty,
                    &reachable.trait_args,
                    reachable.module_id,
                    reachable.interner.as_ref(),
                    extension_index,
                    modules_by_id,
                ) else {
                    return;
                };
                add_reachable_function(
                    method.def_id,
                    program_signatures,
                    &mut state.reachability.functions,
                    &mut modules,
                    pending_modules,
                );
                extend_reachable_trait_methods_from_impl_where_predicates(
                    program_signatures,
                    &matched,
                    &reachable.method_name,
                    reachable.module_id,
                    &mut state.reachable_traits,
                );
            },
        );
    }

    state.trait_function_scan.vtables = vtable_index;
    state.trait_function_scan.methods = method_index;
}

fn add_reachable_default_trait_method_for_method(
    program_signatures: ExecutableSignatureIndex<'_>,
    reachable: &ReachableTraitMethod,
    _reachable_modules: &HashSet<ModuleId>,
    reachable_functions: &mut HashSet<GlobalDefId>,
    modules: &mut HashSet<ModuleId>,
    pending_modules: &mut VecDeque<ModuleId>,
) {
    let TraitId::Source(trait_def) = reachable.trait_id else {
        return;
    };
    let Some(trait_signature) = (program_signatures.trait_)(trait_def) else {
        return;
    };
    for method in &trait_signature.signature.methods {
        if method.has_default && method.name == reachable.method_name {
            add_reachable_function(
                GlobalDefId {
                    module_id: trait_def.module_id,
                    def_id: method.def_id,
                },
                program_signatures,
                reachable_functions,
                modules,
                pending_modules,
            );
        }
    }
}

fn add_reachable_default_trait_methods_for_vtable(
    program_signatures: ExecutableSignatureIndex<'_>,
    vtable: &ReachableTraitVtable,
    _reachable_modules: &HashSet<ModuleId>,
    reachable_functions: &mut HashSet<GlobalDefId>,
    modules: &mut HashSet<ModuleId>,
    pending_modules: &mut VecDeque<ModuleId>,
) {
    let TraitId::Source(trait_def) = vtable.trait_id else {
        return;
    };
    let Some(trait_signature) = (program_signatures.trait_)(trait_def) else {
        return;
    };
    for method in &trait_signature.signature.methods {
        if method.has_default {
            add_reachable_function(
                GlobalDefId {
                    module_id: trait_def.module_id,
                    def_id: method.def_id,
                },
                program_signatures,
                reachable_functions,
                modules,
                pending_modules,
            );
        }
    }
}

#[derive(Debug)]
struct ReachableExtensionMethodMatch {
    impl_signature: ProgramTraitImplSignature,
    interner: TyInterner,
    substitutions: SymbolMap<InternedTyId>,
}

fn reachable_extension_method_match(
    method: &nia_defs::ExtensionMethod,
    trait_id: TraitId,
    self_ty: InternedTyId,
    trait_args: &[InternedTyId],
    use_module_id: ModuleId,
    use_interner_override: Option<&TyInterner>,
    extension_index: &dyn ExecutableExtensionLookup,
    modules_by_id: &HashMap<ModuleId, ReachableModuleInput<'_>>,
) -> Option<ReachableExtensionMethodMatch> {
    if method.trait_args.len() != trait_args.len() {
        return None;
    }
    let mut impl_signature = None;
    if !extension_index.with_trait_impl_for_method(method, trait_id, &mut |signature| {
        impl_signature = Some(signature.clone());
    }) {
        return None;
    };
    let impl_signature = impl_signature?;
    if impl_signature.trait_args.len() != trait_args.len() {
        return None;
    }
    let mut interner = if let Some(interner) = use_interner_override {
        interner.clone()
    } else if let Some(use_module) = modules_by_id.get(&use_module_id) {
        use_module.body_ir.interner.clone()
    } else {
        return None;
    };
    let Ok(target_ty) = nia_ty::try_import_type_into(
        &mut interner,
        &impl_signature.interner,
        impl_signature.target_ty,
    ) else {
        return None;
    };
    let Ok(imported_trait_args) = impl_signature
        .trait_args
        .iter()
        .map(|arg| nia_ty::try_import_type_into(&mut interner, &impl_signature.interner, *arg))
        .collect::<Result<Vec<_>, _>>()
    else {
        return None;
    };
    let mut substitutions = SymbolMap::default();
    if !match_type_pattern(&interner, target_ty, self_ty, &mut substitutions) {
        return None;
    }
    if !imported_trait_args
        .iter()
        .zip(trait_args)
        .all(|(pattern, actual)| {
            match_type_pattern(&interner, *pattern, *actual, &mut substitutions)
        })
    {
        return None;
    }
    Some(ReachableExtensionMethodMatch {
        impl_signature,
        interner,
        substitutions,
    })
}

fn extend_reachable_trait_methods_from_impl_where_predicates(
    program_signatures: ExecutableSignatureIndex<'_>,
    matched: &ReachableExtensionMethodMatch,
    fallback_method_name: &SymbolId,
    module_id: ModuleId,
    traits: &mut ReachableTraitRefs,
) {
    for predicate in &matched.impl_signature.where_predicates {
        let mut interner = matched.interner.clone();
        let Ok(predicate_ty) = nia_ty::try_import_type_into(
            &mut interner,
            &matched.impl_signature.interner,
            predicate.ty,
        ) else {
            continue;
        };
        let substitutions = TypeSubstitutions::generics(&matched.substitutions);
        let Some(self_ty) = substitute_ty(&mut interner, predicate_ty, &substitutions) else {
            continue;
        };
        for bound in &predicate.bounds {
            let Ok(trait_ty) = nia_ty::try_import_type_into(
                &mut interner,
                &matched.impl_signature.interner,
                bound.trait_ty,
            ) else {
                continue;
            };
            let Some(trait_ty) = substitute_ty(&mut interner, trait_ty, &substitutions) else {
                continue;
            };
            let Some((trait_id, trait_args)) = trait_id_and_args(&interner, trait_ty) else {
                continue;
            };
            if let TraitId::Source(trait_def) = trait_id
                && let Some(trait_signature) = (program_signatures.trait_)(trait_def)
            {
                for method in &trait_signature.signature.methods {
                    traits.insert_method_with_interner(
                        module_id,
                        trait_id,
                        method.name.clone(),
                        self_ty,
                        trait_args.clone(),
                        Some(interner.clone()),
                    );
                }
                continue;
            }
            traits.insert_method_with_interner(
                module_id,
                trait_id,
                fallback_method_name.clone(),
                self_ty,
                trait_args,
                Some(interner.clone()),
            );
        }
    }
}

fn add_reachable_function(
    def_id: GlobalDefId,
    program_signatures: ExecutableSignatureIndex<'_>,
    reachable_functions: &mut HashSet<GlobalDefId>,
    reachable_modules: &mut HashSet<ModuleId>,
    pending_modules: &mut VecDeque<ModuleId>,
) {
    let has_runtime_body = (program_signatures.function)(def_id)
        .map(|signature| !signature.signature.is_comptime && signature.signature.has_body)
        .or_else(|| {
            (program_signatures.trait_default_method)(def_id).map(|(_, trait_signature)| {
                trait_signature
                    .signature
                    .methods
                    .iter()
                    .any(|method| method.def_id == def_id.def_id && method.has_default)
            })
        });
    if !has_runtime_body.unwrap_or(false) {
        add_reachable_module(def_id.module_id, reachable_modules, pending_modules);
        return;
    }
    if reachable_functions.insert(def_id) {
        add_reachable_module(def_id.module_id, reachable_modules, pending_modules);
    }
}

fn match_type_pattern(
    interner: &TyInterner,
    pattern: InternedTyId,
    actual: InternedTyId,
    substitutions: &mut SymbolMap<InternedTyId>,
) -> bool {
    let Some(pattern_ty) = interner.get(pattern) else {
        return false;
    };
    match pattern_ty {
        TyKind::GenericParam(name) => {
            if let Some(existing) = substitutions.get(name).copied() {
                types_equivalent(interner, existing, actual)
            } else {
                substitutions.insert(name.clone(), actual);
                true
            }
        }
        TyKind::SelfParam => matches!(interner.get(actual), Some(TyKind::SelfParam)),
        TyKind::Primitive(pattern_primitive) => {
            matches!(interner.get(actual), Some(TyKind::Primitive(actual_primitive)) if pattern_primitive == actual_primitive)
        }
        TyKind::BuiltinType(pattern_builtin) => {
            matches!(interner.get(actual), Some(TyKind::BuiltinType(actual_builtin)) if pattern_builtin == actual_builtin)
        }
        TyKind::Vector {
            elem: pattern_elem,
            lanes: pattern_lanes,
        } => {
            matches!(interner.get(actual), Some(TyKind::Vector { elem, lanes }) if elem == pattern_elem && lanes == pattern_lanes)
        }
        TyKind::Pointer { is_readonly, elem } => match interner.get(actual) {
            Some(TyKind::Pointer {
                is_readonly: actual_readonly,
                elem: actual_elem,
            }) if is_readonly == actual_readonly => {
                match_type_pattern(interner, *elem, *actual_elem, substitutions)
            }
            _ => false,
        },
        TyKind::VolatilePointer { is_readonly, elem } => match interner.get(actual) {
            Some(TyKind::VolatilePointer {
                is_readonly: actual_readonly,
                elem: actual_elem,
            }) if is_readonly == actual_readonly => {
                match_type_pattern(interner, *elem, *actual_elem, substitutions)
            }
            _ => false,
        },
        TyKind::Slice { is_readonly, elem } => match interner.get(actual) {
            Some(TyKind::Slice {
                is_readonly: actual_readonly,
                elem: actual_elem,
            }) if is_readonly == actual_readonly => {
                match_type_pattern(interner, *elem, *actual_elem, substitutions)
            }
            _ => false,
        },
        TyKind::SlicePointee { elem } => match interner.get(actual) {
            Some(TyKind::SlicePointee { elem: actual_elem }) => {
                match_type_pattern(interner, *elem, *actual_elem, substitutions)
            }
            _ => false,
        },
        TyKind::Array { len, elem } => match interner.get(actual) {
            Some(TyKind::Array {
                len: actual_len,
                elem: actual_elem,
            }) if len == actual_len => {
                match_type_pattern(interner, *elem, *actual_elem, substitutions)
            }
            _ => false,
        },
        TyKind::Range { kind, bound } => match interner.get(actual) {
            Some(TyKind::Range {
                kind: actual_kind,
                bound: actual_bound,
            }) if kind == actual_kind => match (bound, actual_bound) {
                (Some(bound), Some(actual_bound)) => {
                    match_type_pattern(interner, *bound, *actual_bound, substitutions)
                }
                (None, None) => true,
                _ => false,
            },
            _ => false,
        },
        TyKind::FunctionPointer {
            params,
            return_type,
            is_variadic,
        } => match interner.get(actual) {
            Some(TyKind::FunctionPointer {
                params: actual_params,
                return_type: actual_return,
                is_variadic: actual_variadic,
            }) if is_variadic == actual_variadic && params.len() == actual_params.len() => {
                params
                    .iter()
                    .zip(actual_params)
                    .all(|(param, actual_param)| {
                        match_type_pattern(interner, *param, *actual_param, substitutions)
                    })
                    && match_type_pattern(interner, *return_type, *actual_return, substitutions)
            }
            _ => false,
        },
        TyKind::Optional { elem } => match interner.get(actual) {
            Some(TyKind::Optional { elem: actual_elem }) => {
                match_type_pattern(interner, *elem, *actual_elem, substitutions)
            }
            _ => false,
        },
        TyKind::ErrorUnion { error, value } => match interner.get(actual) {
            Some(TyKind::ErrorUnion {
                error: actual_error,
                value: actual_value,
            }) => {
                match_type_pattern(interner, *error, *actual_error, substitutions)
                    && match_type_pattern(interner, *value, *actual_value, substitutions)
            }
            _ => false,
        },
        TyKind::Nominal {
            def_id,
            args,
            const_args,
        } => match interner.get(actual) {
            Some(TyKind::Nominal {
                def_id: actual_def_id,
                args: actual_args,
                const_args: actual_const_args,
            }) if def_id == actual_def_id
                && args.len() == actual_args.len()
                && const_args == actual_const_args =>
            {
                args.iter().zip(actual_args).all(|(arg, actual_arg)| {
                    match_type_pattern(interner, *arg, *actual_arg, substitutions)
                })
            }
            _ => false,
        },
        TyKind::BuiltinTrait { trait_id, args } => match interner.get(actual) {
            Some(TyKind::BuiltinTrait {
                trait_id: actual_trait_id,
                args: actual_args,
            }) if trait_id == actual_trait_id && args.len() == actual_args.len() => {
                args.iter().zip(actual_args).all(|(arg, actual_arg)| {
                    match_type_pattern(interner, *arg, *actual_arg, substitutions)
                })
            }
            _ => false,
        },
        TyKind::TraitObject { .. }
        | TyKind::TraitObjectPointee { .. }
        | TyKind::Projection { .. } => types_equivalent(interner, pattern, actual),
        TyKind::Error | TyKind::ComptimeOnly => true,
    }
}

fn types_equivalent(interner: &TyInterner, left: InternedTyId, right: InternedTyId) -> bool {
    left == right || interner.get(left) == interner.get(right)
}

fn trait_id_and_args(
    interner: &TyInterner,
    ty: InternedTyId,
) -> Option<(TraitId, Vec<InternedTyId>)> {
    match interner.get(ty)? {
        TyKind::Nominal { def_id, args, .. } => Some((TraitId::Source(*def_id), args.clone())),
        TyKind::BuiltinTrait { trait_id, args } => {
            Some((TraitId::Builtin(*trait_id), args.clone()))
        }
        _ => None,
    }
}

#[derive(Clone, Copy)]
struct TypeSubstitutions<'a> {
    self_ty: Option<InternedTyId>,
    generics: &'a SymbolMap<InternedTyId>,
}

impl<'a> TypeSubstitutions<'a> {
    fn generics(generics: &'a SymbolMap<InternedTyId>) -> Self {
        Self {
            self_ty: None,
            generics,
        }
    }
}

fn substitute_ty(
    interner: &mut TyInterner,
    ty: InternedTyId,
    substitutions: &TypeSubstitutions<'_>,
) -> Option<InternedTyId> {
    let kind = interner.get(ty)?.clone();
    match kind {
        TyKind::GenericParam(name) => substitutions.generics.get(&name).copied().or(Some(ty)),
        TyKind::SelfParam => substitutions.self_ty.or(Some(ty)),
        TyKind::Pointer { is_readonly, elem } => {
            let elem = substitute_ty(interner, elem, substitutions)?;
            Some(interner.intern(TyKind::Pointer { is_readonly, elem }))
        }
        TyKind::VolatilePointer { is_readonly, elem } => {
            let elem = substitute_ty(interner, elem, substitutions)?;
            Some(interner.intern(TyKind::VolatilePointer { is_readonly, elem }))
        }
        TyKind::Slice { is_readonly, elem } => {
            let elem = substitute_ty(interner, elem, substitutions)?;
            Some(interner.intern(TyKind::Slice { is_readonly, elem }))
        }
        TyKind::SlicePointee { elem } => {
            let elem = substitute_ty(interner, elem, substitutions)?;
            Some(interner.intern(TyKind::SlicePointee { elem }))
        }
        TyKind::Array { len, elem } => {
            let elem = substitute_ty(interner, elem, substitutions)?;
            Some(interner.intern(TyKind::Array { len, elem }))
        }
        TyKind::Range { kind, bound } => {
            let bound = match bound {
                Some(bound) => Some(substitute_ty(interner, bound, substitutions)?),
                None => None,
            };
            Some(interner.intern(TyKind::Range { kind, bound }))
        }
        TyKind::FunctionPointer {
            params,
            return_type,
            is_variadic,
        } => {
            let params = params
                .into_iter()
                .map(|param| substitute_ty(interner, param, substitutions))
                .collect::<Option<Vec<_>>>()?;
            let return_type = substitute_ty(interner, return_type, substitutions)?;
            Some(interner.intern(TyKind::FunctionPointer {
                params,
                return_type,
                is_variadic,
            }))
        }
        TyKind::Optional { elem } => {
            let elem = substitute_ty(interner, elem, substitutions)?;
            Some(interner.intern(TyKind::Optional { elem }))
        }
        TyKind::ErrorUnion { error, value } => {
            let error = substitute_ty(interner, error, substitutions)?;
            let value = substitute_ty(interner, value, substitutions)?;
            Some(interner.intern(TyKind::ErrorUnion { error, value }))
        }
        TyKind::Nominal {
            def_id,
            args,
            const_args,
        } => {
            let args = args
                .into_iter()
                .map(|arg| substitute_ty(interner, arg, substitutions))
                .collect::<Option<Vec<_>>>()?;
            let const_args = const_args
                .into_iter()
                .map(|mut arg| {
                    arg.ty = substitute_ty(interner, arg.ty, substitutions)?;
                    Some(arg)
                })
                .collect::<Option<Vec<_>>>()?;
            Some(interner.intern(TyKind::Nominal {
                def_id,
                args,
                const_args,
            }))
        }
        TyKind::BuiltinTrait { trait_id, args } => {
            let args = args
                .into_iter()
                .map(|arg| substitute_ty(interner, arg, substitutions))
                .collect::<Option<Vec<_>>>()?;
            Some(interner.intern(TyKind::BuiltinTrait { trait_id, args }))
        }
        TyKind::TraitObject {
            is_readonly,
            trait_id,
            trait_args,
            trait_const_args,
            associated_type_bindings,
        } => {
            let trait_args = trait_args
                .into_iter()
                .map(|arg| substitute_ty(interner, arg, substitutions))
                .collect::<Option<Vec<_>>>()?;
            let trait_const_args = trait_const_args
                .into_iter()
                .map(|mut arg| {
                    arg.ty = substitute_ty(interner, arg.ty, substitutions)?;
                    Some(arg)
                })
                .collect::<Option<Vec<_>>>()?;
            let associated_type_bindings = substitute_associated_type_bindings(
                interner,
                associated_type_bindings,
                substitutions,
            )?;
            Some(interner.intern(TyKind::TraitObject {
                is_readonly,
                trait_id,
                trait_args,
                trait_const_args,
                associated_type_bindings,
            }))
        }
        TyKind::TraitObjectPointee {
            trait_id,
            trait_args,
            trait_const_args,
            associated_type_bindings,
        } => {
            let trait_args = trait_args
                .into_iter()
                .map(|arg| substitute_ty(interner, arg, substitutions))
                .collect::<Option<Vec<_>>>()?;
            let trait_const_args = trait_const_args
                .into_iter()
                .map(|mut arg| {
                    arg.ty = substitute_ty(interner, arg.ty, substitutions)?;
                    Some(arg)
                })
                .collect::<Option<Vec<_>>>()?;
            let associated_type_bindings = substitute_associated_type_bindings(
                interner,
                associated_type_bindings,
                substitutions,
            )?;
            Some(interner.intern(TyKind::TraitObjectPointee {
                trait_id,
                trait_args,
                trait_const_args,
                associated_type_bindings,
            }))
        }
        TyKind::Projection {
            self_ty,
            trait_id,
            trait_args,
            trait_const_args,
            name,
        } => {
            let self_ty = substitute_ty(interner, self_ty, substitutions)?;
            let trait_args = trait_args
                .into_iter()
                .map(|arg| substitute_ty(interner, arg, substitutions))
                .collect::<Option<Vec<_>>>()?;
            let trait_const_args = trait_const_args
                .into_iter()
                .map(|mut arg| {
                    arg.ty = substitute_ty(interner, arg.ty, substitutions)?;
                    Some(arg)
                })
                .collect::<Option<Vec<_>>>()?;
            Some(interner.intern(TyKind::Projection {
                self_ty,
                trait_id,
                trait_args,
                trait_const_args,
                name,
            }))
        }
        TyKind::Error
        | TyKind::ComptimeOnly
        | TyKind::Primitive(_)
        | TyKind::BuiltinType(_)
        | TyKind::Vector { .. } => Some(ty),
    }
}

fn substitute_associated_type_bindings(
    interner: &mut TyInterner,
    bindings: Vec<AssociatedTypeBindingTy>,
    substitutions: &TypeSubstitutions<'_>,
) -> Option<Vec<AssociatedTypeBindingTy>> {
    bindings
        .into_iter()
        .map(|binding| {
            let trait_args = binding
                .trait_args
                .into_iter()
                .map(|arg| substitute_ty(interner, arg, substitutions))
                .collect::<Option<Vec<_>>>()?;
            let trait_const_args = binding
                .trait_const_args
                .into_iter()
                .map(|mut arg| {
                    arg.ty = substitute_ty(interner, arg.ty, substitutions)?;
                    Some(arg)
                })
                .collect::<Option<Vec<_>>>()?;
            let ty = substitute_ty(interner, binding.ty, substitutions)?;
            Some(AssociatedTypeBindingTy {
                trait_id: binding.trait_id,
                trait_args,
                trait_const_args,
                name: binding.name,
                ty,
            })
        })
        .collect()
}

fn add_reachable_module(
    module_id: ModuleId,
    reachable_modules: &mut HashSet<ModuleId>,
    pending_modules: &mut VecDeque<ModuleId>,
) {
    if reachable_modules.insert(module_id) {
        pending_modules.push_back(module_id);
    }
}

fn add_reachable_type_module(module_id: ModuleId, type_modules: &mut HashSet<ModuleId>) {
    type_modules.insert(module_id);
}

fn freestanding_start_module(graph: &ModuleGraph) -> Option<ModuleId> {
    graph.module_id_for_module_path(&nia_imports::ModulePath {
        package: known::std(),
        segments: vec![
            known::START,
            known::FREESTANDING,
            known::LINUX,
            known::X86_64,
        ],
    })
}

fn collect_reachable_fact_owner_modules(
    module: &ReachableModuleInput<'_>,
    program_signatures: ExecutableSignatureIndex<'_>,
    reachable_functions: &HashSet<GlobalDefId>,
    reachable_globals: &HashSet<GlobalDefId>,
    type_modules: &mut HashSet<ModuleId>,
    traits: &mut ReachableTraitRefs,
) {
    collect_reachable_fact_owner_modules_for_items(
        module,
        program_signatures,
        reachable_functions,
        reachable_globals,
        type_modules,
        traits,
    );
}

fn collect_reachable_fact_owner_modules_for_items(
    module: &ReachableModuleInput<'_>,
    program_signatures: ExecutableSignatureIndex<'_>,
    functions: &HashSet<GlobalDefId>,
    globals: &HashSet<GlobalDefId>,
    type_modules: &mut HashSet<ModuleId>,
    traits: &mut ReachableTraitRefs,
) {
    let mut type_ids = Vec::new();
    for def_id in functions
        .iter()
        .filter(|def_id| def_id.module_id == module.module_id)
    {
        let Some(function_facts) = module.semantic_facts.function_facts.get(def_id) else {
            continue;
        };
        collect_function_fact_owner_modules(
            module.module_id,
            function_facts,
            type_modules,
            traits,
            &mut type_ids,
        );
    }
    for def_id in globals
        .iter()
        .filter(|def_id| def_id.module_id == module.module_id)
    {
        if let Some(ty) = module.semantic_facts.global_types.get(def_id) {
            type_ids.push(*ty);
        }
    }
    collect_ty_ids_owner_modules(
        type_ids,
        program_signatures,
        &module.body_ir.interner,
        &module.type_lowering.interner,
        &module.type_normalization.interner,
        type_modules,
        traits,
    );
}

fn collect_where_predicate_type_ids(
    predicates: &[nia_defs::WherePredicateSignature],
    type_ids: &mut Vec<InternedTyId>,
) {
    for predicate in predicates {
        type_ids.push(predicate.ty);
        for bound in &predicate.bounds {
            type_ids.push(bound.trait_ty);
            type_ids.extend(
                bound
                    .associated_type_bindings
                    .iter()
                    .map(|binding| binding.ty),
            );
        }
    }
}

fn collect_function_fact_owner_modules(
    module_id: ModuleId,
    facts: &FunctionSemanticFacts,
    type_modules: &mut HashSet<ModuleId>,
    traits: &mut ReachableTraitRefs,
    type_ids: &mut Vec<InternedTyId>,
) {
    type_ids.extend(facts.local_types.values().copied());
    type_ids.extend(facts.node_expr_types.values().copied());
    for instantiation in &facts.generic_instantiations {
        type_ids.extend(instantiation.args.iter().copied());
        type_ids.extend(instantiation.const_args.iter().map(|arg| arg.ty));
    }
    for coercion in facts.node_pointer_array_to_slice_coercions.values() {
        type_ids.extend([coercion.pointer_ty, coercion.array_ty, coercion.slice_ty]);
    }
    for coercion in facts.node_trait_object_coercions.values() {
        type_ids.extend([coercion.source_ty, coercion.target_ty]);
    }
    for upcast in facts.node_trait_object_upcasts.values() {
        type_ids.extend([upcast.source_ty, upcast.target_ty]);
    }
    for value in facts.node_builtin_values.values() {
        match value {
            nia_sema_ir::BuiltinValue::Layout { ty, .. }
            | nia_sema_ir::BuiltinValue::FieldOffset { ty, .. } => type_ids.push(*ty),
            _ => {}
        }
    }
    for call in facts.node_resolved_calls.values() {
        collect_resolved_call_owner_modules(module_id, call, type_modules, traits, type_ids);
    }
    for reference in facts.node_function_references.values() {
        type_ids.extend(reference.args.iter().copied());
        type_ids.extend(reference.const_args.iter().map(|arg| arg.ty));
    }
    for reference in &facts.trait_method_refs {
        traits.insert_method(
            reference.module_id,
            reference.trait_id,
            reference.method_name,
            reference.self_ty,
            reference.trait_args.clone(),
        );
        type_ids.push(reference.self_ty);
        type_ids.extend(reference.trait_args.iter().copied());
    }
}

fn collect_resolved_call_owner_modules(
    module_id: ModuleId,
    call: &nia_sema_ir::ResolvedCall,
    type_modules: &mut HashSet<ModuleId>,
    traits: &mut ReachableTraitRefs,
    type_ids: &mut Vec<InternedTyId>,
) {
    match call {
        nia_sema_ir::ResolvedCall::BuiltinFunction { .. } => {}
        nia_sema_ir::ResolvedCall::Function(_) => {}
        nia_sema_ir::ResolvedCall::FunctionInstance {
            args, const_args, ..
        } => {
            type_ids.extend(args.iter().copied());
            type_ids.extend(const_args.iter().map(|arg| arg.ty));
        }
        nia_sema_ir::ResolvedCall::Method { args, .. } => {
            type_ids.extend(args.iter().copied());
        }
        nia_sema_ir::ResolvedCall::TraitMethod {
            trait_id,
            self_ty,
            trait_args,
            args,
            ..
        } => {
            collect_trait_id_owner_module(TraitId::Source(*trait_id), type_modules, traits);
            type_ids.push(*self_ty);
            type_ids.extend(trait_args.iter().copied());
            type_ids.extend(args.iter().copied());
        }
        nia_sema_ir::ResolvedCall::TraitAssociatedFunction {
            trait_id,
            self_ty,
            trait_args,
            args,
            ..
        } => {
            collect_trait_id_owner_module(TraitId::Source(*trait_id), type_modules, traits);
            type_ids.push(*self_ty);
            type_ids.extend(trait_args.iter().copied());
            type_ids.extend(args.iter().copied());
        }
        nia_sema_ir::ResolvedCall::DynamicTraitMethod {
            object_ty,
            trait_id,
            trait_args,
            params,
            return_type,
            ..
        } => {
            collect_trait_id_owner_module(*trait_id, type_modules, traits);
            type_ids.push(*object_ty);
            type_ids.extend(trait_args.iter().copied());
            type_ids.extend(params.iter().copied());
            type_ids.push(*return_type);
        }
        nia_sema_ir::ResolvedCall::BuiltinTraitMethod {
            trait_id,
            op,
            self_ty,
            trait_args,
        } => {
            traits.insert_trait(TraitId::Builtin(*trait_id));
            if let Some(method) = op.method()
                && let Some(method_name) = builtin_trait_method_symbol(method)
            {
                traits.insert_method(
                    module_id,
                    TraitId::Builtin(*trait_id),
                    method_name,
                    *self_ty,
                    trait_args.clone(),
                );
            }
            type_ids.push(*self_ty);
            type_ids.extend(trait_args.iter().copied());
        }
        nia_sema_ir::ResolvedCall::BuiltinMethod { method, self_ty } => {
            if let Some((trait_id, trait_method)) = semantic_builtin_method_trait(*method) {
                if let Some(method_name) = builtin_trait_method_symbol(trait_method) {
                    traits.insert_method(
                        module_id,
                        TraitId::Builtin(trait_id),
                        method_name,
                        *self_ty,
                        Vec::new(),
                    );
                }
            }
            type_ids.push(*self_ty);
        }
        nia_sema_ir::ResolvedCall::BuiltinPlaceMethod {
            trait_id,
            method,
            self_ty,
            trait_args,
            ..
        } => {
            let _ = method;
            traits.insert_trait(TraitId::Builtin(*trait_id));
            type_ids.push(*self_ty);
            type_ids.extend(trait_args.iter().copied());
        }
        nia_sema_ir::ResolvedCall::FunctionPointer => {}
    }
}

fn semantic_builtin_method_trait(
    method: nia_sema_ir::BuiltinMethod,
) -> Option<(BuiltinTrait, BuiltinTraitMethod)> {
    match method {
        nia_sema_ir::BuiltinMethod::Len => Some((BuiltinTrait::Len, BuiltinTraitMethod::Len)),
        nia_sema_ir::BuiltinMethod::Start => Some((BuiltinTrait::Start, BuiltinTraitMethod::Start)),
        nia_sema_ir::BuiltinMethod::End => Some((BuiltinTrait::End, BuiltinTraitMethod::End)),
        nia_sema_ir::BuiltinMethod::Char => Some((BuiltinTrait::Char, BuiltinTraitMethod::Char)),
        nia_sema_ir::BuiltinMethod::Iter => {
            Some((BuiltinTrait::Iterable, BuiltinTraitMethod::IterableIter))
        }
    }
}

fn collect_ty_ids_owner_modules<'a>(
    tys: impl IntoIterator<Item = InternedTyId>,
    program_signatures: ExecutableSignatureIndex<'a>,
    body_interner: &TyInterner,
    type_lowering_interner: &TyInterner,
    normalization_interner: &TyInterner,
    type_modules: &mut HashSet<ModuleId>,
    traits: &mut ReachableTraitRefs,
) {
    let mut pending = tys
        .into_iter()
        .map(|ty| PendingTy {
            ty,
            interner: None,
            owned_interner: None,
        })
        .collect::<VecDeque<_>>();
    let mut seen = HashSet::new();
    while let Some(pending_ty) = pending.pop_front() {
        let ty_id = pending_ty.ty;
        add_reachable_type_module(type_owner(ty_id).module_id(), type_modules);
        let interner_id = pending_ty
            .interner
            .map(TyInterner::interner_id)
            .or_else(|| {
                pending_ty
                    .owned_interner
                    .as_ref()
                    .map(TyInterner::interner_id)
            });
        if !seen.insert((ty_id, interner_id)) {
            continue;
        }
        let ty = if let Some(interner) = pending_ty.interner {
            interner.get(ty_id)
        } else if let Some(interner) = pending_ty.owned_interner.as_ref() {
            interner.get(ty_id)
        } else {
            body_interner
                .get(ty_id)
                .or_else(|| type_lowering_interner.get(ty_id))
                .or_else(|| normalization_interner.get(ty_id))
        };
        let Some(ty) = ty else { continue };
        collect_ty_owner_modules(ty, program_signatures, &mut pending, type_modules, traits);
    }
}

#[derive(Clone)]
struct PendingTy<'a> {
    ty: InternedTyId,
    interner: Option<&'a TyInterner>,
    owned_interner: Option<TyInterner>,
}

fn type_owner(ty: InternedTyId) -> nia_ids::TypeOwner {
    ty.owner()
}

fn collect_ty_owner_modules<'a>(
    ty: &TyKind,
    program_signatures: ExecutableSignatureIndex<'a>,
    type_ids: &mut VecDeque<PendingTy<'a>>,
    type_modules: &mut HashSet<ModuleId>,
    traits: &mut ReachableTraitRefs,
) {
    match ty {
        TyKind::Nominal { def_id, args, .. } => {
            add_reachable_type_module(def_id.module_id, type_modules);
            push_tys(type_ids, args.iter().copied());
            collect_nominal_signature_owner_type_ids(*def_id, program_signatures, type_ids);
        }
        TyKind::Pointer { elem, .. }
        | TyKind::VolatilePointer { elem, .. }
        | TyKind::Slice { elem, .. }
        | TyKind::SlicePointee { elem }
        | TyKind::Optional { elem } => {
            push_ty(type_ids, *elem);
        }
        TyKind::Array { len, elem } => {
            push_ty(type_ids, *elem);
            collect_array_len_owner_modules(len, type_ids);
        }
        TyKind::Range { bound, .. } => {
            if let Some(bound) = bound {
                push_ty(type_ids, *bound);
            }
        }
        TyKind::FunctionPointer {
            params,
            return_type,
            ..
        } => {
            push_tys(type_ids, params.iter().copied());
            push_ty(type_ids, *return_type);
        }
        TyKind::ErrorUnion { error, value } => {
            push_ty(type_ids, *error);
            push_ty(type_ids, *value);
        }
        TyKind::TraitObject {
            trait_id,
            trait_args,
            trait_const_args,
            associated_type_bindings,
            ..
        }
        | TyKind::TraitObjectPointee {
            trait_id,
            trait_args,
            trait_const_args,
            associated_type_bindings,
        } => {
            collect_trait_id_owner_module(*trait_id, type_modules, traits);
            push_tys(type_ids, trait_args.iter().copied());
            push_tys(type_ids, trait_const_args.iter().map(|arg| arg.ty));
            collect_associated_binding_owner_modules(
                associated_type_bindings,
                type_ids,
                type_modules,
                traits,
            );
        }
        TyKind::Projection {
            self_ty,
            trait_id,
            trait_args,
            trait_const_args,
            ..
        } => {
            push_ty(type_ids, *self_ty);
            collect_trait_id_owner_module(*trait_id, type_modules, traits);
            push_tys(type_ids, trait_args.iter().copied());
            push_tys(type_ids, trait_const_args.iter().map(|arg| arg.ty));
        }
        TyKind::BuiltinTrait { args, .. } => push_tys(type_ids, args.iter().copied()),
        TyKind::Error
        | TyKind::ComptimeOnly
        | TyKind::Primitive(_)
        | TyKind::BuiltinType(_)
        | TyKind::Vector { .. }
        | TyKind::SelfParam
        | TyKind::GenericParam(_) => {}
    }
}

fn builtin_trait_method_symbol(method: BuiltinTraitMethod) -> Option<SymbolId> {
    match method {
        BuiltinTraitMethod::Add => Some(known::ADD),
        BuiltinTraitMethod::Sub => Some(known::SUB),
        BuiltinTraitMethod::Mul => Some(known::MUL),
        BuiltinTraitMethod::Div => Some(known::DIV),
        BuiltinTraitMethod::Rem => Some(known::REM),
        BuiltinTraitMethod::Neg => Some(known::NEG),
        BuiltinTraitMethod::Not => Some(known::LOGICAL_NOT),
        BuiltinTraitMethod::BitNot => Some(known::BIT_NOT),
        BuiltinTraitMethod::BitAnd => Some(known::BIT_AND),
        BuiltinTraitMethod::BitOr => Some(known::BIT_OR),
        BuiltinTraitMethod::BitXor => Some(known::BIT_XOR),
        BuiltinTraitMethod::Shl => Some(known::SHL),
        BuiltinTraitMethod::Shr => Some(known::SHR),
        BuiltinTraitMethod::Eq => Some(known::EQ),
        BuiltinTraitMethod::Ne => Some(known::NE),
        BuiltinTraitMethod::Lt => Some(known::LT),
        BuiltinTraitMethod::Le => Some(known::LE),
        BuiltinTraitMethod::Gt => Some(known::GT),
        BuiltinTraitMethod::Ge => Some(known::GE),
        BuiltinTraitMethod::Deref => Some(known::DEREF),
        BuiltinTraitMethod::DerefMut => Some(known::DEREF_MUT),
        BuiltinTraitMethod::Index => Some(known::INDEX),
        BuiltinTraitMethod::IndexMut => Some(known::INDEX_MUT),
        BuiltinTraitMethod::Slice => Some(known::SLICE),
        BuiltinTraitMethod::SliceMut => Some(known::SLICE_MUT),
        BuiltinTraitMethod::Ptr => Some(known::PTR),
        BuiltinTraitMethod::PtrMut => Some(known::PTR_MUT),
        BuiltinTraitMethod::Len => Some(known::LEN),
        BuiltinTraitMethod::Start => Some(known::START),
        BuiltinTraitMethod::End => Some(known::END),
        BuiltinTraitMethod::Char => Some(known::CHAR),
        BuiltinTraitMethod::IteratorNext => Some(known::NEXT),
        BuiltinTraitMethod::IterableIter => Some(known::ITER_METHOD),
    }
}

fn collect_nominal_signature_owner_type_ids<'a>(
    def_id: GlobalDefId,
    program_signatures: ExecutableSignatureIndex<'a>,
    type_ids: &mut VecDeque<PendingTy<'a>>,
) {
    if let Some(signature) = (program_signatures.struct_)(def_id) {
        push_owned_program_tys(
            type_ids,
            signature.signature.fields.iter().map(|field| field.ty),
            &signature.interner,
        );
        collect_owned_where_predicate_type_ids_deque(
            &signature.signature.where_predicates,
            type_ids,
            &signature.interner,
        );
    }
    if let Some(signature) = (program_signatures.union)(def_id) {
        push_owned_program_tys(
            type_ids,
            signature.signature.fields.iter().map(|field| field.ty),
            &signature.interner,
        );
        collect_owned_where_predicate_type_ids_deque(
            &signature.signature.where_predicates,
            type_ids,
            &signature.interner,
        );
    }
}

fn collect_owned_where_predicate_type_ids_deque(
    predicates: &[nia_defs::WherePredicateSignature],
    type_ids: &mut VecDeque<PendingTy<'_>>,
    interner: &TyInterner,
) {
    let mut collected = Vec::new();
    collect_where_predicate_type_ids(predicates, &mut collected);
    push_owned_program_tys(type_ids, collected, interner);
}

fn collect_array_len_owner_modules(
    len: &nia_ty::ArrayLenTy,
    type_ids: &mut VecDeque<PendingTy<'_>>,
) {
    if let nia_ty::ArrayLenTy::Builtin { ty, .. } = len {
        push_ty(type_ids, *ty);
    }
}

fn collect_trait_id_owner_module(
    trait_id: TraitId,
    type_modules: &mut HashSet<ModuleId>,
    traits: &mut ReachableTraitRefs,
) {
    traits.insert_trait(trait_id);
    if let TraitId::Source(def_id) = trait_id {
        add_reachable_type_module(def_id.module_id, type_modules);
    }
}

fn collect_associated_binding_owner_modules<'a>(
    bindings: &[AssociatedTypeBindingTy],
    type_ids: &mut VecDeque<PendingTy<'a>>,
    type_modules: &mut HashSet<ModuleId>,
    traits: &mut ReachableTraitRefs,
) {
    for binding in bindings {
        if let Some(trait_id) = binding.trait_id {
            collect_trait_id_owner_module(trait_id, type_modules, traits);
        }
        push_tys(type_ids, binding.trait_args.iter().copied());
        push_tys(type_ids, binding.trait_const_args.iter().map(|arg| arg.ty));
        push_ty(type_ids, binding.ty);
    }
}

fn push_ty(type_ids: &mut VecDeque<PendingTy<'_>>, ty: InternedTyId) {
    type_ids.push_back(PendingTy {
        ty,
        interner: None,
        owned_interner: None,
    });
}

fn push_tys(type_ids: &mut VecDeque<PendingTy<'_>>, tys: impl IntoIterator<Item = InternedTyId>) {
    type_ids.extend(tys.into_iter().map(|ty| PendingTy {
        ty,
        interner: None,
        owned_interner: None,
    }));
}

fn push_owned_program_tys(
    type_ids: &mut VecDeque<PendingTy<'_>>,
    tys: impl IntoIterator<Item = InternedTyId>,
    interner: &TyInterner,
) {
    type_ids.extend(tys.into_iter().map(|ty| PendingTy {
        ty,
        interner: None,
        owned_interner: Some(interner.clone()),
    }));
}
