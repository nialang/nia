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
use nia_ty::{AssociatedTypeBindingTy, TyInterner, TyKind};

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ExecutableReachability {
    modules: HashSet<ModuleId>,
    type_modules: HashSet<ModuleId>,
    functions: HashSet<GlobalDefId>,
    globals: HashSet<GlobalDefId>,
    stats: ExecutableReachabilityStats,
}

impl ExecutableReachability {
    pub fn modules(&self) -> &HashSet<ModuleId> {
        &self.modules
    }

    pub fn type_modules(&self) -> &HashSet<ModuleId> {
        &self.type_modules
    }

    pub fn functions(&self) -> &HashSet<GlobalDefId> {
        &self.functions
    }

    pub fn globals(&self) -> &HashSet<GlobalDefId> {
        &self.globals
    }

    pub fn stats(&self) -> ExecutableReachabilityStats {
        self.stats
    }

    pub fn by_module(&self) -> ExecutableReachabilityByModule {
        ExecutableReachabilityByModule::new(self)
    }

    pub fn insert_function(&mut self, def_id: GlobalDefId) -> bool {
        let changed = self.functions.insert(def_id);
        self.modules.insert(def_id.module_id);
        changed
    }

    pub fn insert_functions(&mut self, def_ids: impl IntoIterator<Item = GlobalDefId>) -> bool {
        let mut changed = false;
        for def_id in def_ids {
            changed |= self.insert_function(def_id);
        }
        changed
    }

    pub fn insert_global(&mut self, def_id: GlobalDefId) -> bool {
        let changed = self.globals.insert(def_id);
        self.modules.insert(def_id.module_id);
        changed
    }

    pub fn insert_globals(&mut self, def_ids: impl IntoIterator<Item = GlobalDefId>) -> bool {
        let mut changed = false;
        for def_id in def_ids {
            changed |= self.insert_global(def_id);
        }
        changed
    }

    fn insert_module(&mut self, module_id: ModuleId) -> bool {
        self.modules.insert(module_id)
    }

    fn insert_module_pending(
        &mut self,
        module_id: ModuleId,
        pending_modules: &mut VecDeque<ModuleId>,
    ) -> bool {
        if self.modules.insert(module_id) {
            pending_modules.push_back(module_id);
            true
        } else {
            false
        }
    }

    fn insert_function_pending(
        &mut self,
        def_id: GlobalDefId,
        pending_modules: &mut VecDeque<ModuleId>,
    ) -> bool {
        if self.functions.insert(def_id) {
            self.insert_module_pending(def_id.module_id, pending_modules);
            true
        } else {
            false
        }
    }

    fn insert_global_pending(
        &mut self,
        def_id: GlobalDefId,
        pending_modules: &mut VecDeque<ModuleId>,
    ) -> bool {
        if self.globals.insert(def_id) {
            self.insert_module_pending(def_id.module_id, pending_modules);
            true
        } else {
            false
        }
    }

    fn set_stats(&mut self, stats: ExecutableReachabilityStats) {
        self.stats = stats;
    }

    fn change_key(&self) -> (usize, usize, usize, usize) {
        (
            self.functions.len(),
            self.globals.len(),
            self.modules.len(),
            self.type_modules.len(),
        )
    }
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ExecutableModuleReachability {
    pub functions: HashSet<GlobalDefId>,
    pub globals: HashSet<GlobalDefId>,
}

impl ExecutableModuleReachability {
    pub fn has_body_items(&self, is_runtime_global: impl FnMut(GlobalDefId) -> bool) -> bool {
        !self.functions.is_empty() || self.globals.iter().copied().any(is_runtime_global)
    }
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ExecutableReachabilityByModule {
    modules: HashMap<ModuleId, ExecutableModuleReachability>,
}

impl ExecutableReachabilityByModule {
    pub fn new(reachability: &ExecutableReachability) -> Self {
        let mut modules = HashMap::<ModuleId, ExecutableModuleReachability>::new();
        for def_id in reachability.functions.iter().copied() {
            modules
                .entry(def_id.module_id)
                .or_default()
                .functions
                .insert(def_id);
        }
        for def_id in reachability.globals.iter().copied() {
            modules
                .entry(def_id.module_id)
                .or_default()
                .globals
                .insert(def_id);
        }
        Self { modules }
    }

    pub fn get(&self, module_id: ModuleId) -> Option<&ExecutableModuleReachability> {
        self.modules.get(&module_id)
    }

    pub fn reachable_body_modules(
        &self,
        mut is_runtime_global: impl FnMut(GlobalDefId) -> bool,
    ) -> HashSet<ModuleId> {
        self.modules
            .iter()
            .filter_map(|(module_id, items)| {
                items
                    .has_body_items(&mut is_runtime_global)
                    .then_some(*module_id)
            })
            .collect()
    }
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct ExecutableReachabilityStats {
    pub checked_modules: usize,
    pub checked_bodies: usize,
    pub reachable_bodies: usize,
}

pub struct ExecutableExtensionIndex<'a> {
    by_trait: nia_hash::FastHashMap<TraitId, Vec<&'a nia_defs::ExtensionMethod>>,
    by_trait_method: nia_hash::FastHashMap<(TraitId, SymbolId), Vec<&'a nia_defs::ExtensionMethod>>,
    where_predicates_by_def:
        nia_hash::FastHashMap<GlobalDefId, &'a [nia_defs::WherePredicateSignature]>,
    trait_impls_by_key:
        nia_hash::FastHashMap<(ModuleId, TraitImplId, TraitId), &'a ProgramTraitImplSignature>,
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
        let mut by_trait =
            nia_hash::FastHashMap::<TraitId, Vec<&'a nia_defs::ExtensionMethod>>::default();
        let mut by_trait_method = nia_hash::FastHashMap::<
            (TraitId, SymbolId),
            Vec<&'a nia_defs::ExtensionMethod>,
        >::default();
        let mut where_predicates_by_def =
            nia_hash::FastHashMap::<GlobalDefId, &'a [nia_defs::WherePredicateSignature]>::default(
            );
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
            .collect::<nia_hash::FastHashMap<_, _>>();
        for method in extension_methods.all_methods() {
            where_predicates_by_def.insert(method.def_id, method.where_predicates.as_slice());
            if let Some(trait_id) = method.trait_id {
                by_trait.entry(trait_id).or_default().push(method);
                by_trait_method
                    .entry((trait_id, method.name))
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

    pub fn reachability_mut(&mut self) -> &mut ExecutableReachability {
        &mut self.reachability
    }

    pub fn into_reachability(self) -> ExecutableReachability {
        self.reachability
    }
}

#[derive(Clone, Copy)]
pub struct ExecutableRootDefs<'a> {
    pub functions: &'a [GlobalDefId],
    pub globals: &'a [GlobalDefId],
}

impl std::fmt::Debug for ExecutableRootDefs<'_> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ExecutableRootDefs")
            .field("functions", &self.functions)
            .field("globals", &self.globals)
            .finish()
    }
}

pub struct ExecutableExtensionSources<'a> {
    pub methods: &'a ExtensionMethods,
    pub trait_impls: &'a [ProgramTraitImplSignature],
}

pub struct ExecutableReachabilityInput<'a> {
    pub parse_ok: &'a [ModuleId],
    pub entry_module: ModuleId,
    pub root_defs: ExecutableRootDefs<'a>,
    pub program_signatures: ExecutableSignatureIndex<'a>,
    pub modules: &'a [ReachableModuleInput<'a>],
}

pub struct CheckedModuleReachabilityInput<'a> {
    pub parse_ok: &'a [ModuleId],
    pub program_signatures: ExecutableSignatureIndex<'a>,
    pub module: ReachableModuleInput<'a>,
    pub checked_functions: &'a HashSet<GlobalDefId>,
    pub modules_by_id: &'a HashMap<ModuleId, ReachableModuleInput<'a>>,
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
                module.interner,
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
                module.interner,
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
    arg_interner: &TyInterner,
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
    if !visited.insert(reachable_generic_instantiation_key(
        arg_interner,
        instantiation,
    )) {
        return;
    }
    if !active_defs.insert(instantiation.def_id) {
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
        active_defs.remove(&instantiation.def_id);
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
    let substitutions = TypeSubstitutions::local(self_ty, &generic_substitutions);
    for predicate in &signature.signature.where_predicates {
        let mut substituted_interner = signature_interner.clone();
        let Some(self_ty) = substitute_ty(&mut substituted_interner, predicate.ty, &substitutions)
        else {
            continue;
        };
        for bound in &predicate.bounds {
            let mut bound_interner = substituted_interner.clone();
            let Some(trait_ty) = substitute_ty(&mut bound_interner, bound.trait_ty, &substitutions)
            else {
                continue;
            };
            let Some((trait_id, trait_args)) = trait_id_and_args(&bound_interner, trait_ty) else {
                continue;
            };
            insert_trait_and_supertrait_methods(
                program_signatures,
                traits,
                use_module_id,
                trait_id,
                self_ty,
                &trait_args,
                bound_interner,
            );
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
                let mut bound_interner = substituted_interner.clone();
                let Some(trait_ty) =
                    substitute_ty(&mut bound_interner, bound.trait_ty, &substitutions)
                else {
                    continue;
                };
                let Some((trait_id, trait_args)) = trait_id_and_args(&bound_interner, trait_ty)
                else {
                    continue;
                };
                insert_trait_and_supertrait_methods(
                    program_signatures,
                    traits,
                    use_module_id,
                    trait_id,
                    self_ty,
                    &trait_args,
                    bound_interner,
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
        let mut nested_interner = signature_interner.clone();
        let Some(nested_instantiation) = instantiate_nested_generic_instantiation(
            &mut nested_interner,
            target_module.interner,
            nested,
            &substitutions,
        ) else {
            continue;
        };
        extend_reachable_traits_from_generic_instantiation(
            use_module_id,
            &nested_interner,
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
    traits.insert_methods_with_interner(
        use_module_id,
        trait_id,
        trait_signature
            .signature
            .methods
            .iter()
            .map(|method| ReachableTraitMethodName { name: method.name }),
        self_ty,
        &trait_args,
        method_interner,
    );
}

fn insert_trait_and_supertrait_methods(
    program_signatures: ExecutableSignatureIndex<'_>,
    traits: &mut ReachableTraitRefs,
    module_id: ModuleId,
    trait_id: TraitId,
    self_ty: InternedTyId,
    trait_args: &[InternedTyId],
    interner: TyInterner,
) {
    TraitMethodExpansion {
        program_signatures,
        traits,
        module_id,
        active_traits: HashSet::new(),
    }
    .insert(trait_id, self_ty, trait_args, interner);
}

struct TraitMethodExpansion<'a, 'b> {
    program_signatures: ExecutableSignatureIndex<'a>,
    traits: &'b mut ReachableTraitRefs,
    module_id: ModuleId,
    active_traits: HashSet<TraitId>,
}

impl TraitMethodExpansion<'_, '_> {
    fn insert(
        &mut self,
        trait_id: TraitId,
        self_ty: InternedTyId,
        trait_args: &[InternedTyId],
        interner: TyInterner,
    ) {
        if !self.active_traits.insert(trait_id) {
            return;
        }
        match trait_id {
            TraitId::Builtin(builtin_trait) => {
                self.traits.insert_methods_with_interner(
                    self.module_id,
                    trait_id,
                    builtin_trait
                        .required_methods()
                        .iter()
                        .filter_map(|method| builtin_trait_method_symbol(*method))
                        .map(|name| ReachableTraitMethodName { name }),
                    self_ty,
                    trait_args,
                    interner.clone(),
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
                        interner.clone(),
                    );
                }
            }
            TraitId::Source(trait_def) => {
                let Some(trait_signature) = (self.program_signatures.trait_)(trait_def) else {
                    self.active_traits.remove(&trait_id);
                    return;
                };
                self.traits.insert_methods_with_interner(
                    self.module_id,
                    trait_id,
                    trait_signature
                        .signature
                        .methods
                        .iter()
                        .map(|method| ReachableTraitMethodName { name: method.name }),
                    self_ty,
                    trait_args,
                    interner.clone(),
                );
                for supertrait in &trait_signature.signature.supertraits {
                    let mut supertrait_interner = trait_signature.interner.clone();
                    let Ok(imported_self_ty) =
                        nia_ty::try_import_type_into(&mut supertrait_interner, &interner, self_ty)
                    else {
                        continue;
                    };
                    let imported_trait_args = trait_args
                        .iter()
                        .map(|arg| {
                            nia_ty::try_import_type_into(&mut supertrait_interner, &interner, *arg)
                                .ok()
                        })
                        .collect::<Option<Vec<_>>>();
                    let Some(imported_trait_args) = imported_trait_args else {
                        continue;
                    };
                    let substitutions = trait_signature
                        .signature
                        .generics
                        .iter()
                        .copied()
                        .zip(imported_trait_args)
                        .collect::<SymbolMap<_>>();
                    let substitutions =
                        TypeSubstitutions::local(Some(imported_self_ty), &substitutions);
                    let Some(supertrait_ty) =
                        substitute_ty(&mut supertrait_interner, supertrait.ty, &substitutions)
                    else {
                        continue;
                    };
                    let Some((supertrait_id, supertrait_args)) =
                        trait_id_and_args(&supertrait_interner, supertrait_ty)
                    else {
                        continue;
                    };
                    self.insert(
                        supertrait_id,
                        imported_self_ty,
                        &supertrait_args,
                        supertrait_interner,
                    );
                }
            }
        }
        self.active_traits.remove(&trait_id);
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

fn typed_executable_refs_for_function(
    module: &ReachableModuleInput<'_>,
    def_id: GlobalDefId,
) -> TypedExecutableRefs {
    let refs = module.executable_refs.refs_for_function(def_id);
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
    module: &ReachableModuleInput<'_>,
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
        traits.insert_vtable_with_interner(
            vtable.module_id,
            vtable.trait_id,
            vtable.self_ty,
            vtable.trait_args,
            Some(module.interner.clone()),
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
    interner_contexts: Vec<TyInterner>,
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
    interner: Option<ReachableTraitInternerId>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
struct ReachableTraitInternerId(usize);

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
    interner: Option<ReachableTraitInternerId>,
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
            interner_contexts,
            vtables,
            ..
        } = refs;
        self.traits.extend(traits);
        let mut interner_contexts = interner_contexts.into_iter().map(Some).collect::<Vec<_>>();
        let mut remapped_interner_ids: Vec<Option<ReachableTraitInternerId>> =
            vec![None; interner_contexts.len()];
        for method in methods {
            let Some(source_interner) = method.interner else {
                self.insert_method(
                    method.module_id,
                    method.trait_id,
                    method.method_name,
                    method.self_ty,
                    method.trait_args,
                );
                continue;
            };
            let key = if let Some(interner) = remapped_interner_ids
                .get(source_interner.0)
                .and_then(|id| *id)
                .and_then(|id| self.interner_contexts.get(id.0))
            {
                Self::method_key_for_interner(
                    method.trait_id,
                    &method.method_name,
                    method.self_ty,
                    &method.trait_args,
                    interner,
                )
            } else {
                let Some(interner) = interner_contexts
                    .get(source_interner.0)
                    .and_then(Option::as_ref)
                else {
                    continue;
                };
                Self::method_key_for_interner(
                    method.trait_id,
                    &method.method_name,
                    method.self_ty,
                    &method.trait_args,
                    interner,
                )
            };
            let Some(key) = key else {
                continue;
            };
            self.traits.insert(method.trait_id);
            if !self.method_keys.insert(key) {
                continue;
            }
            let interner = match remapped_interner_ids
                .get(source_interner.0)
                .and_then(|id| *id)
            {
                Some(id) => id,
                None => {
                    let Some(interner) = interner_contexts
                        .get_mut(source_interner.0)
                        .and_then(Option::take)
                    else {
                        continue;
                    };
                    let id = ReachableTraitInternerId(self.interner_contexts.len());
                    self.interner_contexts.push(interner);
                    remapped_interner_ids[source_interner.0] = Some(id);
                    id
                }
            };
            self.methods.push(ReachableTraitMethod {
                interner: Some(interner),
                ..method
            });
        }
        for vtable in vtables {
            let interner = match vtable.interner {
                Some(source_interner) => match remapped_interner_ids
                    .get(source_interner.0)
                    .and_then(|id| *id)
                {
                    Some(id) => Some(id),
                    None => {
                        let Some(interner) = interner_contexts
                            .get_mut(source_interner.0)
                            .and_then(Option::take)
                        else {
                            continue;
                        };
                        let id = ReachableTraitInternerId(self.interner_contexts.len());
                        self.interner_contexts.push(interner);
                        remapped_interner_ids[source_interner.0] = Some(id);
                        Some(id)
                    }
                },
                None => None,
            };
            self.insert_vtable_with_interner_id(vtable, interner);
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
        let interner = match interner {
            Some(interner) => interner,
            None => {
                if !self.raw_method_keys.insert(ReachableTraitRawMethodKey {
                    module_id,
                    trait_id,
                    method_name,
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
                    interner: None,
                });
            }
        };
        let Some(key) =
            Self::method_key_for_interner(trait_id, &method_name, self_ty, &trait_args, &interner)
        else {
            return;
        };
        if !self.method_keys.insert(key) {
            return;
        }
        let interner = {
            let id = ReachableTraitInternerId(self.interner_contexts.len());
            self.interner_contexts.push(interner);
            id
        };
        self.methods.push(ReachableTraitMethod {
            module_id,
            trait_id,
            method_name,
            self_ty,
            trait_args,
            interner: Some(interner),
        });
    }

    fn insert_methods_with_interner(
        &mut self,
        module_id: ModuleId,
        trait_id: TraitId,
        methods: impl IntoIterator<Item = ReachableTraitMethodName>,
        self_ty: InternedTyId,
        trait_args: &[InternedTyId],
        interner: TyInterner,
    ) {
        self.traits.insert(trait_id);
        let mut pending_methods = Vec::new();
        for method in methods {
            let Some(key) = Self::method_key_for_interner(
                trait_id,
                &method.name,
                self_ty,
                trait_args,
                &interner,
            ) else {
                continue;
            };
            if !self.method_keys.insert(key) {
                continue;
            }
            pending_methods.push(ReachableTraitMethod {
                module_id,
                trait_id,
                method_name: method.name,
                self_ty,
                trait_args: trait_args.to_vec(),
                interner: None,
            });
        }
        if pending_methods.is_empty() {
            return;
        }
        let interner = {
            let id = ReachableTraitInternerId(self.interner_contexts.len());
            self.interner_contexts.push(interner);
            id
        };
        for mut method in pending_methods {
            method.interner = Some(interner);
            self.methods.push(method);
        }
    }

    fn method_key_for_interner(
        trait_id: TraitId,
        method_name: &SymbolId,
        self_ty: InternedTyId,
        trait_args: &[InternedTyId],
        interner: &TyInterner,
    ) -> Option<ReachableTraitMethodKey> {
        let self_ty = interner.get(self_ty).cloned()?;
        let trait_args = trait_args
            .iter()
            .map(|arg| interner.get(*arg).cloned())
            .collect::<Option<Vec<_>>>()?;
        Some(ReachableTraitMethodKey {
            trait_id,
            method_name: *method_name,
            self_ty,
            trait_args,
        })
    }

    fn insert_vtable_with_interner(
        &mut self,
        module_id: ModuleId,
        trait_id: TraitId,
        self_ty: InternedTyId,
        trait_args: Vec<InternedTyId>,
        interner: Option<TyInterner>,
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
            interner: interner.map(|interner| {
                let id = ReachableTraitInternerId(self.interner_contexts.len());
                self.interner_contexts.push(interner);
                id
            }),
        });
    }

    fn insert_vtable_with_interner_id(
        &mut self,
        vtable: ReachableTraitVtable,
        interner: Option<ReachableTraitInternerId>,
    ) {
        self.traits.insert(vtable.trait_id);
        if !self.vtable_keys.insert(ReachableTraitVtableKey {
            module_id: vtable.module_id,
            trait_id: vtable.trait_id,
            self_ty: vtable.self_ty,
            trait_args: vtable.trait_args.clone(),
        }) {
            return;
        }
        self.vtables
            .push(ReachableTraitVtable { interner, ..vtable });
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

    fn interner_context(&self, id: Option<ReachableTraitInternerId>) -> Option<&TyInterner> {
        id.and_then(|id| self.interner_contexts.get(id.0))
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
    for vtable in reachable_traits.vtables.clone() {
        let Some(vtable_interner) = reachable_traits.interner_context(vtable.interner).cloned()
        else {
            continue;
        };
        insert_trait_and_supertrait_methods(
            program_signatures,
            reachable_traits,
            vtable.module_id,
            vtable.trait_id,
            vtable.self_ty,
            &vtable.trait_args,
            vtable_interner,
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
        let vtable_interner = reachable_traits.interner_context(vtable.interner);
        extension_index.for_each_method_for_trait(vtable.trait_id, &mut |method| {
            if !with_reachable_extension_method_match(
                ReachableExtensionMatchInput {
                    method,
                    trait_id: vtable.trait_id,
                    self_ty: vtable.self_ty,
                    trait_args: &vtable.trait_args,
                    use_module_id: vtable.module_id,
                    use_interner_override: vtable_interner,
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
            let reachable_interner = reachable_traits.interner_context(reachable.interner);
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
                            use_interner_override: reachable_interner,
                            extension_index,
                            modules_by_id,
                        },
                        &mut |matched| {
                            deferred_modules.add_function(method.def_id, program_signatures);
                            extend_reachable_trait_methods_from_impl_where_predicates(
                                program_signatures,
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
        let vtable_interner = state
            .reachable_traits
            .interner_context(vtable.interner)
            .cloned();
        if let Some(vtable_interner) = vtable_interner.as_ref() {
            insert_trait_and_supertrait_methods(
                program_signatures,
                &mut state.reachable_traits,
                vtable.module_id,
                vtable.trait_id,
                vtable.self_ty,
                &vtable.trait_args,
                vtable_interner.clone(),
            );
        }
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
                    use_interner_override: vtable_interner.as_ref(),
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
            let reachable_interner = state.reachable_traits.interner_context(reachable.interner);
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
                            use_interner_override: reachable_interner,
                            extension_index,
                            modules_by_id,
                        },
                        &mut |matched| {
                            deferred_modules.add_function(method.def_id, program_signatures);
                            extend_reachable_trait_methods_from_impl_where_predicates(
                                program_signatures,
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

#[derive(Debug)]
struct ReachableExtensionMethodMatch<'a> {
    impl_signature: &'a ProgramTraitImplSignature,
    substitutions: SymbolMap<SubstitutionTy<'a>>,
}

#[derive(Debug, Clone, Copy)]
struct TypedTyRef<'a> {
    interner: &'a TyInterner,
    ty: InternedTyId,
}

impl<'a> TypedTyRef<'a> {
    fn kind(self) -> Option<&'a TyKind> {
        self.interner.get(self.ty)
    }
}

#[derive(Debug, Clone, Copy)]
enum SubstitutionTy<'a> {
    External(TypedTyRef<'a>),
}

struct ReachableExtensionMatchInput<'a> {
    method: &'a nia_defs::ExtensionMethod,
    trait_id: TraitId,
    self_ty: InternedTyId,
    trait_args: &'a [InternedTyId],
    use_module_id: ModuleId,
    use_interner_override: Option<&'a TyInterner>,
    extension_index: &'a dyn ExecutableExtensionLookup,
    modules_by_id: &'a HashMap<ModuleId, ReachableModuleInput<'a>>,
}

fn with_reachable_extension_method_match(
    input: ReachableExtensionMatchInput<'_>,
    f: &mut dyn FnMut(ReachableExtensionMethodMatch<'_>),
) -> bool {
    let ReachableExtensionMatchInput {
        method,
        trait_id,
        self_ty,
        trait_args,
        use_module_id,
        use_interner_override,
        extension_index,
        modules_by_id,
    } = input;
    if method.trait_args.len() != trait_args.len() {
        return false;
    }
    let mut matched = false;
    extension_index.with_trait_impl_for_method(method, trait_id, &mut |impl_signature| {
        if impl_signature.trait_args.len() != trait_args.len() {
            return;
        }
        let use_interner = if let Some(interner) = use_interner_override {
            interner
        } else if let Some(use_module) = modules_by_id.get(&use_module_id) {
            use_module.interner
        } else {
            return;
        };
        let self_ref = TypedTyRef {
            interner: use_interner,
            ty: self_ty,
        };
        let pointee_ref = typed_pointer_elem_ref(self_ref);
        let direct = match_reachable_extension_impl(
            TypedTyRef {
                interner: &impl_signature.interner,
                ty: impl_signature.target_ty,
            },
            impl_signature.trait_args.iter().map(|ty| TypedTyRef {
                interner: &impl_signature.interner,
                ty: *ty,
            }),
            self_ref,
            trait_args.iter().map(|ty| TypedTyRef {
                interner: use_interner,
                ty: *ty,
            }),
        );
        let pointee = direct.is_none().then(|| {
            match_reachable_extension_impl(
                TypedTyRef {
                    interner: &impl_signature.interner,
                    ty: impl_signature.target_ty,
                },
                impl_signature.trait_args.iter().map(|ty| TypedTyRef {
                    interner: &impl_signature.interner,
                    ty: *ty,
                }),
                pointee_ref?,
                trait_args.iter().map(|ty| TypedTyRef {
                    interner: use_interner,
                    ty: *ty,
                }),
            )
        });
        let Some(substitutions) = direct.or_else(|| pointee.flatten()) else {
            return;
        };
        matched = true;
        f(ReachableExtensionMethodMatch {
            impl_signature,
            substitutions,
        });
    });
    matched
}

fn match_reachable_extension_impl<'a>(
    impl_target: TypedTyRef<'a>,
    impl_trait_args: impl IntoIterator<Item = TypedTyRef<'a>>,
    self_ty: TypedTyRef<'a>,
    trait_args: impl IntoIterator<Item = TypedTyRef<'a>>,
) -> Option<SymbolMap<SubstitutionTy<'a>>> {
    let mut substitutions = SymbolMap::default();
    if !match_type_pattern(impl_target, self_ty, &mut substitutions) {
        return None;
    }
    let matches_trait_args = impl_trait_args
        .into_iter()
        .zip(trait_args)
        .all(|(pattern, actual)| match_type_pattern(pattern, actual, &mut substitutions));
    matches_trait_args.then_some(substitutions)
}

fn extend_reachable_trait_methods_from_impl_where_predicates(
    program_signatures: ExecutableSignatureIndex<'_>,
    matched: &ReachableExtensionMethodMatch,
    fallback_method_name: &SymbolId,
    module_id: ModuleId,
    traits: &mut ReachableTraitRefs,
) {
    for predicate in &matched.impl_signature.where_predicates {
        let mut interner = matched.impl_signature.interner.clone();
        let Ok(predicate_ty) = nia_ty::try_import_type_into(
            &mut interner,
            &matched.impl_signature.interner,
            predicate.ty,
        ) else {
            continue;
        };
        let substitutions = TypeSubstitutions::typed_generics(&matched.substitutions);
        let Some(self_ty) = substitute_ty(&mut interner, predicate_ty, &substitutions) else {
            continue;
        };
        for bound in &predicate.bounds {
            let mut bound_interner = interner.clone();
            let Ok(trait_ty) = nia_ty::try_import_type_into(
                &mut bound_interner,
                &matched.impl_signature.interner,
                bound.trait_ty,
            ) else {
                continue;
            };
            let Some(trait_ty) = substitute_ty(&mut bound_interner, trait_ty, &substitutions)
            else {
                continue;
            };
            let Some((trait_id, trait_args)) = trait_id_and_args(&bound_interner, trait_ty) else {
                continue;
            };
            if let TraitId::Source(trait_def) = trait_id
                && let Some(trait_signature) = (program_signatures.trait_)(trait_def)
            {
                traits.insert_methods_with_interner(
                    module_id,
                    trait_id,
                    trait_signature
                        .signature
                        .methods
                        .iter()
                        .map(|method| ReachableTraitMethodName { name: method.name }),
                    self_ty,
                    &trait_args,
                    bound_interner,
                );
                continue;
            }
            traits.insert_method_with_interner(
                module_id,
                trait_id,
                *fallback_method_name,
                self_ty,
                trait_args,
                Some(bound_interner),
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

fn match_type_pattern<'a>(
    pattern: TypedTyRef<'a>,
    actual: TypedTyRef<'a>,
    substitutions: &mut SymbolMap<SubstitutionTy<'a>>,
) -> bool {
    let Some(pattern_ty) = pattern.kind() else {
        return false;
    };
    match pattern_ty {
        TyKind::GenericParam(name) => {
            if let Some(existing) = substitutions.get(name).copied() {
                substitution_ty_equivalent(existing, actual)
            } else {
                substitutions.insert(*name, SubstitutionTy::External(actual));
                true
            }
        }
        TyKind::SelfParam => matches!(actual.kind(), Some(TyKind::SelfParam)),
        TyKind::Primitive(pattern_primitive) => {
            matches!(actual.kind(), Some(TyKind::Primitive(actual_primitive)) if pattern_primitive == actual_primitive)
        }
        TyKind::BuiltinType(pattern_builtin) => {
            matches!(actual.kind(), Some(TyKind::BuiltinType(actual_builtin)) if pattern_builtin == actual_builtin)
        }
        TyKind::Vector {
            elem: pattern_elem,
            lanes: pattern_lanes,
        } => {
            matches!(actual.kind(), Some(TyKind::Vector { elem, lanes }) if elem == pattern_elem && lanes == pattern_lanes)
        }
        TyKind::Pointer { is_readonly, elem } => match actual.kind() {
            Some(TyKind::Pointer {
                is_readonly: actual_readonly,
                elem: actual_elem,
            }) if is_readonly == actual_readonly => match_type_pattern(
                TypedTyRef {
                    interner: pattern.interner,
                    ty: *elem,
                },
                TypedTyRef {
                    interner: actual.interner,
                    ty: *actual_elem,
                },
                substitutions,
            ),
            _ => false,
        },
        TyKind::VolatilePointer { is_readonly, elem } => match actual.kind() {
            Some(TyKind::VolatilePointer {
                is_readonly: actual_readonly,
                elem: actual_elem,
            }) if is_readonly == actual_readonly => match_type_pattern(
                TypedTyRef {
                    interner: pattern.interner,
                    ty: *elem,
                },
                TypedTyRef {
                    interner: actual.interner,
                    ty: *actual_elem,
                },
                substitutions,
            ),
            _ => false,
        },
        TyKind::Slice { is_readonly, elem } => match actual.kind() {
            Some(TyKind::Slice {
                is_readonly: actual_readonly,
                elem: actual_elem,
            }) if is_readonly == actual_readonly => match_type_pattern(
                TypedTyRef {
                    interner: pattern.interner,
                    ty: *elem,
                },
                TypedTyRef {
                    interner: actual.interner,
                    ty: *actual_elem,
                },
                substitutions,
            ),
            _ => false,
        },
        TyKind::SlicePointee { elem } => match actual.kind() {
            Some(TyKind::SlicePointee { elem: actual_elem }) => match_type_pattern(
                TypedTyRef {
                    interner: pattern.interner,
                    ty: *elem,
                },
                TypedTyRef {
                    interner: actual.interner,
                    ty: *actual_elem,
                },
                substitutions,
            ),
            _ => false,
        },
        TyKind::Array { len, elem } => match actual.kind() {
            Some(TyKind::Array {
                len: actual_len,
                elem: actual_elem,
            }) if len == actual_len => match_type_pattern(
                TypedTyRef {
                    interner: pattern.interner,
                    ty: *elem,
                },
                TypedTyRef {
                    interner: actual.interner,
                    ty: *actual_elem,
                },
                substitutions,
            ),
            _ => false,
        },
        TyKind::Range { kind, bound } => match actual.kind() {
            Some(TyKind::Range {
                kind: actual_kind,
                bound: actual_bound,
            }) if kind == actual_kind => match (bound, actual_bound) {
                (Some(bound), Some(actual_bound)) => match_type_pattern(
                    TypedTyRef {
                        interner: pattern.interner,
                        ty: *bound,
                    },
                    TypedTyRef {
                        interner: actual.interner,
                        ty: *actual_bound,
                    },
                    substitutions,
                ),
                (None, None) => true,
                _ => false,
            },
            _ => false,
        },
        TyKind::FunctionPointer {
            params,
            return_type,
            is_variadic,
        } => match actual.kind() {
            Some(TyKind::FunctionPointer {
                params: actual_params,
                return_type: actual_return,
                is_variadic: actual_variadic,
            }) if is_variadic == actual_variadic && params.len() == actual_params.len() => {
                params
                    .iter()
                    .zip(actual_params)
                    .all(|(param, actual_param)| {
                        match_type_pattern(
                            TypedTyRef {
                                interner: pattern.interner,
                                ty: *param,
                            },
                            TypedTyRef {
                                interner: actual.interner,
                                ty: *actual_param,
                            },
                            substitutions,
                        )
                    })
                    && match_type_pattern(
                        TypedTyRef {
                            interner: pattern.interner,
                            ty: *return_type,
                        },
                        TypedTyRef {
                            interner: actual.interner,
                            ty: *actual_return,
                        },
                        substitutions,
                    )
            }
            _ => false,
        },
        TyKind::Optional { elem } => match actual.kind() {
            Some(TyKind::Optional { elem: actual_elem }) => match_type_pattern(
                TypedTyRef {
                    interner: pattern.interner,
                    ty: *elem,
                },
                TypedTyRef {
                    interner: actual.interner,
                    ty: *actual_elem,
                },
                substitutions,
            ),
            _ => false,
        },
        TyKind::ErrorUnion { error, value } => match actual.kind() {
            Some(TyKind::ErrorUnion {
                error: actual_error,
                value: actual_value,
            }) => {
                match_type_pattern(
                    TypedTyRef {
                        interner: pattern.interner,
                        ty: *error,
                    },
                    TypedTyRef {
                        interner: actual.interner,
                        ty: *actual_error,
                    },
                    substitutions,
                ) && match_type_pattern(
                    TypedTyRef {
                        interner: pattern.interner,
                        ty: *value,
                    },
                    TypedTyRef {
                        interner: actual.interner,
                        ty: *actual_value,
                    },
                    substitutions,
                )
            }
            _ => false,
        },
        TyKind::Nominal {
            def_id,
            args,
            const_args,
        } => match actual.kind() {
            Some(TyKind::Nominal {
                def_id: actual_def_id,
                args: actual_args,
                const_args: actual_const_args,
            }) if def_id == actual_def_id
                && args.len() == actual_args.len()
                && const_args == actual_const_args =>
            {
                args.iter().zip(actual_args).all(|(arg, actual_arg)| {
                    match_type_pattern(
                        TypedTyRef {
                            interner: pattern.interner,
                            ty: *arg,
                        },
                        TypedTyRef {
                            interner: actual.interner,
                            ty: *actual_arg,
                        },
                        substitutions,
                    )
                })
            }
            _ => false,
        },
        TyKind::BuiltinTrait { trait_id, args } => match actual.kind() {
            Some(TyKind::BuiltinTrait {
                trait_id: actual_trait_id,
                args: actual_args,
            }) if trait_id == actual_trait_id && args.len() == actual_args.len() => {
                args.iter().zip(actual_args).all(|(arg, actual_arg)| {
                    match_type_pattern(
                        TypedTyRef {
                            interner: pattern.interner,
                            ty: *arg,
                        },
                        TypedTyRef {
                            interner: actual.interner,
                            ty: *actual_arg,
                        },
                        substitutions,
                    )
                })
            }
            _ => false,
        },
        TyKind::TraitObject { .. }
        | TyKind::TraitObjectPointee { .. }
        | TyKind::Projection { .. } => typed_refs_equivalent(pattern, actual),
        TyKind::Error | TyKind::ConstOnly => true,
    }
}

fn substitution_ty_equivalent(existing: SubstitutionTy<'_>, actual: TypedTyRef<'_>) -> bool {
    match existing {
        SubstitutionTy::External(existing) => typed_refs_equivalent(existing, actual),
    }
}

fn typed_pointer_elem_ref(ty: TypedTyRef<'_>) -> Option<TypedTyRef<'_>> {
    match ty.kind() {
        Some(TyKind::Pointer { elem, .. }) => Some(TypedTyRef {
            interner: ty.interner,
            ty: *elem,
        }),
        _ => None,
    }
}

fn typed_refs_equivalent(left: TypedTyRef<'_>, right: TypedTyRef<'_>) -> bool {
    if left.ty == right.ty && left.interner.interner_id() == right.interner.interner_id() {
        return true;
    }
    typed_refs_structurally_equivalent(left, right)
}

fn typed_refs_structurally_equivalent(left: TypedTyRef<'_>, right: TypedTyRef<'_>) -> bool {
    match (left.kind(), right.kind()) {
        (Some(TyKind::Error), Some(TyKind::Error)) => true,
        (Some(TyKind::ConstOnly), Some(TyKind::ConstOnly)) => true,
        (Some(TyKind::Primitive(left)), Some(TyKind::Primitive(right))) => left == right,
        (Some(TyKind::BuiltinType(left)), Some(TyKind::BuiltinType(right))) => left == right,
        (Some(TyKind::GenericParam(left)), Some(TyKind::GenericParam(right))) => left == right,
        (Some(TyKind::SelfParam), Some(TyKind::SelfParam)) => true,
        (
            Some(TyKind::Pointer {
                is_readonly: left_readonly,
                elem: left_elem,
            }),
            Some(TyKind::Pointer {
                is_readonly: right_readonly,
                elem: right_elem,
            }),
        )
        | (
            Some(TyKind::VolatilePointer {
                is_readonly: left_readonly,
                elem: left_elem,
            }),
            Some(TyKind::VolatilePointer {
                is_readonly: right_readonly,
                elem: right_elem,
            }),
        )
        | (
            Some(TyKind::Slice {
                is_readonly: left_readonly,
                elem: left_elem,
            }),
            Some(TyKind::Slice {
                is_readonly: right_readonly,
                elem: right_elem,
            }),
        ) => {
            left_readonly == right_readonly
                && typed_refs_equivalent(
                    TypedTyRef {
                        interner: left.interner,
                        ty: *left_elem,
                    },
                    TypedTyRef {
                        interner: right.interner,
                        ty: *right_elem,
                    },
                )
        }
        (
            Some(TyKind::SlicePointee { elem: left_elem }),
            Some(TyKind::SlicePointee { elem: right_elem }),
        )
        | (
            Some(TyKind::Optional { elem: left_elem }),
            Some(TyKind::Optional { elem: right_elem }),
        ) => typed_refs_equivalent(
            TypedTyRef {
                interner: left.interner,
                ty: *left_elem,
            },
            TypedTyRef {
                interner: right.interner,
                ty: *right_elem,
            },
        ),
        (
            Some(TyKind::Array {
                len: left_len,
                elem: left_elem,
            }),
            Some(TyKind::Array {
                len: right_len,
                elem: right_elem,
            }),
        ) => {
            array_lens_equivalent(left.interner, left_len, right.interner, right_len)
                && typed_refs_equivalent(
                    TypedTyRef {
                        interner: left.interner,
                        ty: *left_elem,
                    },
                    TypedTyRef {
                        interner: right.interner,
                        ty: *right_elem,
                    },
                )
        }
        (
            Some(TyKind::Vector {
                elem: left_elem,
                lanes: left_lanes,
            }),
            Some(TyKind::Vector {
                elem: right_elem,
                lanes: right_lanes,
            }),
        ) => left_elem == right_elem && left_lanes == right_lanes,
        (
            Some(TyKind::Range {
                kind: left_kind,
                bound: left_bound,
            }),
            Some(TyKind::Range {
                kind: right_kind,
                bound: right_bound,
            }),
        ) => {
            left_kind == right_kind
                && optional_typed_refs_equivalent(
                    left.interner,
                    *left_bound,
                    right.interner,
                    *right_bound,
                )
        }
        (
            Some(TyKind::FunctionPointer {
                params: left_params,
                return_type: left_return,
                is_variadic: left_variadic,
            }),
            Some(TyKind::FunctionPointer {
                params: right_params,
                return_type: right_return,
                is_variadic: right_variadic,
            }),
        ) => {
            left_variadic == right_variadic
                && typed_ref_slices_equivalent(
                    left.interner,
                    left_params,
                    right.interner,
                    right_params,
                )
                && typed_refs_equivalent(
                    TypedTyRef {
                        interner: left.interner,
                        ty: *left_return,
                    },
                    TypedTyRef {
                        interner: right.interner,
                        ty: *right_return,
                    },
                )
        }
        (
            Some(TyKind::ErrorUnion {
                error: left_error,
                value: left_value,
            }),
            Some(TyKind::ErrorUnion {
                error: right_error,
                value: right_value,
            }),
        ) => {
            typed_refs_equivalent(
                TypedTyRef {
                    interner: left.interner,
                    ty: *left_error,
                },
                TypedTyRef {
                    interner: right.interner,
                    ty: *right_error,
                },
            ) && typed_refs_equivalent(
                TypedTyRef {
                    interner: left.interner,
                    ty: *left_value,
                },
                TypedTyRef {
                    interner: right.interner,
                    ty: *right_value,
                },
            )
        }
        (
            Some(TyKind::Nominal {
                def_id: left_def,
                args: left_args,
                const_args: left_const_args,
            }),
            Some(TyKind::Nominal {
                def_id: right_def,
                args: right_args,
                const_args: right_const_args,
            }),
        ) => {
            left_def == right_def
                && typed_ref_slices_equivalent(left.interner, left_args, right.interner, right_args)
                && const_generic_args_equivalent(
                    left.interner,
                    left_const_args,
                    right.interner,
                    right_const_args,
                )
        }
        (
            Some(TyKind::BuiltinTrait {
                trait_id: left_trait,
                args: left_args,
            }),
            Some(TyKind::BuiltinTrait {
                trait_id: right_trait,
                args: right_args,
            }),
        ) => {
            left_trait == right_trait
                && typed_ref_slices_equivalent(left.interner, left_args, right.interner, right_args)
        }
        (
            Some(TyKind::TraitObject {
                is_readonly: left_readonly,
                trait_id: left_trait,
                trait_args: left_args,
                trait_const_args: left_const_args,
                associated_type_bindings: left_bindings,
            }),
            Some(TyKind::TraitObject {
                is_readonly: right_readonly,
                trait_id: right_trait,
                trait_args: right_args,
                trait_const_args: right_const_args,
                associated_type_bindings: right_bindings,
            }),
        ) => {
            left_readonly == right_readonly
                && trait_object_parts_equivalent(
                    TraitObjectParts {
                        interner: left.interner,
                        trait_id: *left_trait,
                        args: left_args,
                        const_args: left_const_args,
                        bindings: left_bindings,
                    },
                    TraitObjectParts {
                        interner: right.interner,
                        trait_id: *right_trait,
                        args: right_args,
                        const_args: right_const_args,
                        bindings: right_bindings,
                    },
                )
        }
        (
            Some(TyKind::TraitObjectPointee {
                trait_id: left_trait,
                trait_args: left_args,
                trait_const_args: left_const_args,
                associated_type_bindings: left_bindings,
            }),
            Some(TyKind::TraitObjectPointee {
                trait_id: right_trait,
                trait_args: right_args,
                trait_const_args: right_const_args,
                associated_type_bindings: right_bindings,
            }),
        ) => trait_object_parts_equivalent(
            TraitObjectParts {
                interner: left.interner,
                trait_id: *left_trait,
                args: left_args,
                const_args: left_const_args,
                bindings: left_bindings,
            },
            TraitObjectParts {
                interner: right.interner,
                trait_id: *right_trait,
                args: right_args,
                const_args: right_const_args,
                bindings: right_bindings,
            },
        ),
        (
            Some(TyKind::Projection {
                self_ty: left_self,
                trait_id: left_trait,
                trait_args: left_args,
                trait_const_args: left_const_args,
                name: left_name,
            }),
            Some(TyKind::Projection {
                self_ty: right_self,
                trait_id: right_trait,
                trait_args: right_args,
                trait_const_args: right_const_args,
                name: right_name,
            }),
        ) => {
            left_trait == right_trait
                && left_name == right_name
                && typed_refs_equivalent(
                    TypedTyRef {
                        interner: left.interner,
                        ty: *left_self,
                    },
                    TypedTyRef {
                        interner: right.interner,
                        ty: *right_self,
                    },
                )
                && typed_ref_slices_equivalent(left.interner, left_args, right.interner, right_args)
                && const_generic_args_equivalent(
                    left.interner,
                    left_const_args,
                    right.interner,
                    right_const_args,
                )
        }
        _ => false,
    }
}

fn optional_typed_refs_equivalent(
    left_interner: &TyInterner,
    left: Option<InternedTyId>,
    right_interner: &TyInterner,
    right: Option<InternedTyId>,
) -> bool {
    match (left, right) {
        (Some(left), Some(right)) => typed_refs_equivalent(
            TypedTyRef {
                interner: left_interner,
                ty: left,
            },
            TypedTyRef {
                interner: right_interner,
                ty: right,
            },
        ),
        (None, None) => true,
        _ => false,
    }
}

fn typed_ref_slices_equivalent(
    left_interner: &TyInterner,
    left: &[InternedTyId],
    right_interner: &TyInterner,
    right: &[InternedTyId],
) -> bool {
    left.len() == right.len()
        && left.iter().zip(right).all(|(left, right)| {
            typed_refs_equivalent(
                TypedTyRef {
                    interner: left_interner,
                    ty: *left,
                },
                TypedTyRef {
                    interner: right_interner,
                    ty: *right,
                },
            )
        })
}

fn array_lens_equivalent(
    left_interner: &TyInterner,
    left: &nia_ty::ArrayLenTy,
    right_interner: &TyInterner,
    right: &nia_ty::ArrayLenTy,
) -> bool {
    use nia_ty::ArrayLenTy;
    match (left, right) {
        (ArrayLenTy::Infer, ArrayLenTy::Infer) => true,
        (ArrayLenTy::GenericParam(left), ArrayLenTy::GenericParam(right)) => left == right,
        (ArrayLenTy::ConstValue(left), ArrayLenTy::ConstValue(right)) => left == right,
        (ArrayLenTy::ConstExpr(left), ArrayLenTy::ConstExpr(right)) => left == right,
        (
            ArrayLenTy::Builtin {
                builtin: left_builtin,
                ty: left_ty,
            },
            ArrayLenTy::Builtin {
                builtin: right_builtin,
                ty: right_ty,
            },
        ) => {
            left_builtin == right_builtin
                && typed_refs_equivalent(
                    TypedTyRef {
                        interner: left_interner,
                        ty: *left_ty,
                    },
                    TypedTyRef {
                        interner: right_interner,
                        ty: *right_ty,
                    },
                )
        }
        _ => false,
    }
}

fn const_generic_args_equivalent(
    left_interner: &TyInterner,
    left: &[nia_ty::ConstGenericArg],
    right_interner: &TyInterner,
    right: &[nia_ty::ConstGenericArg],
) -> bool {
    left.len() == right.len()
        && left.iter().zip(right).all(|(left, right)| {
            left.value == right.value
                && typed_refs_equivalent(
                    TypedTyRef {
                        interner: left_interner,
                        ty: left.ty,
                    },
                    TypedTyRef {
                        interner: right_interner,
                        ty: right.ty,
                    },
                )
        })
}

#[derive(Clone, Copy)]
struct TraitObjectParts<'a> {
    interner: &'a TyInterner,
    trait_id: TraitId,
    args: &'a [InternedTyId],
    const_args: &'a [nia_ty::ConstGenericArg],
    bindings: &'a [AssociatedTypeBindingTy],
}

fn trait_object_parts_equivalent(left: TraitObjectParts<'_>, right: TraitObjectParts<'_>) -> bool {
    left.trait_id == right.trait_id
        && typed_ref_slices_equivalent(left.interner, left.args, right.interner, right.args)
        && const_generic_args_equivalent(
            left.interner,
            left.const_args,
            right.interner,
            right.const_args,
        )
        && associated_type_bindings_equivalent(
            left.interner,
            left.bindings,
            right.interner,
            right.bindings,
        )
}

fn associated_type_bindings_equivalent(
    left_interner: &TyInterner,
    left: &[AssociatedTypeBindingTy],
    right_interner: &TyInterner,
    right: &[AssociatedTypeBindingTy],
) -> bool {
    left.len() == right.len()
        && left.iter().all(|left_binding| {
            right
                .iter()
                .find(|right_binding| {
                    associated_type_binding_keys_equivalent(
                        left_interner,
                        left_binding,
                        right_interner,
                        right_binding,
                    )
                })
                .is_some_and(|right_binding| {
                    typed_refs_equivalent(
                        TypedTyRef {
                            interner: left_interner,
                            ty: left_binding.ty,
                        },
                        TypedTyRef {
                            interner: right_interner,
                            ty: right_binding.ty,
                        },
                    )
                })
        })
}

fn associated_type_binding_keys_equivalent(
    left_interner: &TyInterner,
    left: &AssociatedTypeBindingTy,
    right_interner: &TyInterner,
    right: &AssociatedTypeBindingTy,
) -> bool {
    left.name == right.name
        && left.trait_id == right.trait_id
        && typed_ref_slices_equivalent(
            left_interner,
            &left.trait_args,
            right_interner,
            &right.trait_args,
        )
        && const_generic_args_equivalent(
            left_interner,
            &left.trait_const_args,
            right_interner,
            &right.trait_const_args,
        )
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
    generics: TypeSubstitutionGenerics<'a>,
}

#[derive(Clone, Copy)]
enum TypeSubstitutionGenerics<'a> {
    Local(&'a SymbolMap<InternedTyId>),
    Typed(&'a SymbolMap<SubstitutionTy<'a>>),
}

impl<'a> TypeSubstitutions<'a> {
    fn local(self_ty: Option<InternedTyId>, generics: &'a SymbolMap<InternedTyId>) -> Self {
        Self {
            self_ty,
            generics: TypeSubstitutionGenerics::Local(generics),
        }
    }

    fn typed_generics(generics: &'a SymbolMap<SubstitutionTy<'a>>) -> Self {
        Self {
            self_ty: None,
            generics: TypeSubstitutionGenerics::Typed(generics),
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
        TyKind::GenericParam(name) => substitute_generic_ty(interner, &name, substitutions, ty),
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
        | TyKind::ConstOnly
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

fn substitute_generic_ty(
    interner: &mut TyInterner,
    name: &SymbolId,
    substitutions: &TypeSubstitutions<'_>,
    fallback: InternedTyId,
) -> Option<InternedTyId> {
    match substitutions.generics {
        TypeSubstitutionGenerics::Local(generics) => generics.get(name).copied().or(Some(fallback)),
        TypeSubstitutionGenerics::Typed(generics) => generics
            .get(name)
            .copied()
            .map(|ty| import_substitution_ty(interner, ty))
            .transpose()
            .ok()
            .flatten()
            .or(Some(fallback)),
    }
}

fn import_substitution_ty(
    interner: &mut TyInterner,
    substitution: SubstitutionTy<'_>,
) -> Result<InternedTyId, nia_ty::TypeImportError> {
    match substitution {
        SubstitutionTy::External(ty) => nia_ty::try_import_type_into(interner, ty.interner, ty.ty),
    }
}

fn add_reachable_type_module(module_id: ModuleId, type_modules: &mut HashSet<ModuleId>) {
    type_modules.insert(module_id);
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
        module.interner,
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
        type_ids.extend([coercion.source_ty, coercion.target_ty, coercion.self_ty]);
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
            if let Some((trait_id, trait_method)) = semantic_builtin_method_trait(*method)
                && let Some(method_name) = builtin_trait_method_symbol(trait_method)
            {
                traits.insert_method(
                    module_id,
                    TraitId::Builtin(trait_id),
                    method_name,
                    *self_ty,
                    Vec::new(),
                );
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
        .map(|ty| PendingTy { ty, interner: None })
        .collect::<VecDeque<_>>();
    let mut seen = HashSet::new();
    while let Some(pending_ty) = pending.pop_front() {
        let ty_id = pending_ty.ty;
        let interner_id = pending_ty.interner.map(TyInterner::interner_id);
        if !seen.insert((ty_id, interner_id)) {
            continue;
        }
        let ty = if let Some(interner) = pending_ty.interner {
            interner.get(ty_id)
        } else {
            body_interner
                .get(ty_id)
                .or_else(|| type_lowering_interner.get(ty_id))
                .or_else(|| normalization_interner.get(ty_id))
        };
        let Some(ty) = ty else { continue };
        collect_ty_owner_modules(
            ty,
            pending_ty.interner,
            program_signatures,
            &mut pending,
            type_modules,
            traits,
            &mut seen,
        );
    }
}

#[derive(Clone, Copy)]
struct PendingTy<'a> {
    ty: InternedTyId,
    interner: Option<&'a TyInterner>,
}

fn collect_ty_owner_modules<'a>(
    ty: &TyKind,
    current_interner: Option<&'a TyInterner>,
    program_signatures: ExecutableSignatureIndex<'a>,
    type_ids: &mut VecDeque<PendingTy<'a>>,
    type_modules: &mut HashSet<ModuleId>,
    traits: &mut ReachableTraitRefs,
    seen: &mut HashSet<(InternedTyId, Option<nia_ids::TyInternerId>)>,
) {
    match ty {
        TyKind::Nominal { def_id, args, .. } => {
            add_reachable_type_module(def_id.module_id, type_modules);
            push_tys(type_ids, current_interner, args.iter().copied());
            collect_nominal_signature_owner_type_ids(
                *def_id,
                program_signatures,
                type_modules,
                traits,
                seen,
            );
        }
        TyKind::Pointer { elem, .. }
        | TyKind::VolatilePointer { elem, .. }
        | TyKind::Slice { elem, .. }
        | TyKind::SlicePointee { elem }
        | TyKind::Optional { elem } => {
            push_ty(type_ids, current_interner, *elem);
        }
        TyKind::Array { len, elem } => {
            push_ty(type_ids, current_interner, *elem);
            collect_array_len_owner_modules(len, current_interner, type_ids);
        }
        TyKind::Range { bound, .. } => {
            if let Some(bound) = bound {
                push_ty(type_ids, current_interner, *bound);
            }
        }
        TyKind::FunctionPointer {
            params,
            return_type,
            ..
        } => {
            push_tys(type_ids, current_interner, params.iter().copied());
            push_ty(type_ids, current_interner, *return_type);
        }
        TyKind::ErrorUnion { error, value } => {
            push_ty(type_ids, current_interner, *error);
            push_ty(type_ids, current_interner, *value);
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
            push_tys(type_ids, current_interner, trait_args.iter().copied());
            push_tys(
                type_ids,
                current_interner,
                trait_const_args.iter().map(|arg| arg.ty),
            );
            collect_associated_binding_owner_modules(
                associated_type_bindings,
                current_interner,
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
            push_ty(type_ids, current_interner, *self_ty);
            collect_trait_id_owner_module(*trait_id, type_modules, traits);
            push_tys(type_ids, current_interner, trait_args.iter().copied());
            push_tys(
                type_ids,
                current_interner,
                trait_const_args.iter().map(|arg| arg.ty),
            );
        }
        TyKind::BuiltinTrait { args, .. } => {
            push_tys(type_ids, current_interner, args.iter().copied())
        }
        TyKind::Error
        | TyKind::ConstOnly
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
    type_modules: &mut HashSet<ModuleId>,
    traits: &mut ReachableTraitRefs,
    seen: &mut HashSet<(InternedTyId, Option<nia_ids::TyInternerId>)>,
) {
    if let Some(signature) = (program_signatures.struct_)(def_id) {
        collect_ty_ids_owner_modules_with_interner(
            signature.signature.fields.iter().map(|field| field.ty),
            program_signatures,
            &signature.interner,
            type_modules,
            traits,
            seen,
        );
        collect_owned_where_predicate_type_ids_deque(
            &signature.signature.where_predicates,
            program_signatures,
            &signature.interner,
            type_modules,
            traits,
            seen,
        );
    }
    if let Some(signature) = (program_signatures.union)(def_id) {
        collect_ty_ids_owner_modules_with_interner(
            signature.signature.fields.iter().map(|field| field.ty),
            program_signatures,
            &signature.interner,
            type_modules,
            traits,
            seen,
        );
        collect_owned_where_predicate_type_ids_deque(
            &signature.signature.where_predicates,
            program_signatures,
            &signature.interner,
            type_modules,
            traits,
            seen,
        );
    }
}

fn collect_ty_ids_owner_modules_with_interner<'a>(
    tys: impl IntoIterator<Item = InternedTyId>,
    program_signatures: ExecutableSignatureIndex<'a>,
    interner: &'a TyInterner,
    type_modules: &mut HashSet<ModuleId>,
    traits: &mut ReachableTraitRefs,
    seen: &mut HashSet<(InternedTyId, Option<nia_ids::TyInternerId>)>,
) {
    let mut pending = tys
        .into_iter()
        .map(|ty| PendingTy {
            ty,
            interner: Some(interner),
        })
        .collect::<VecDeque<_>>();
    while let Some(pending_ty) = pending.pop_front() {
        let ty_id = pending_ty.ty;
        let interner_id = pending_ty.interner.map(TyInterner::interner_id);
        if !seen.insert((ty_id, interner_id)) {
            continue;
        }
        let Some(ty) = pending_ty.interner.and_then(|interner| interner.get(ty_id)) else {
            continue;
        };
        collect_ty_owner_modules(
            ty,
            pending_ty.interner,
            program_signatures,
            &mut pending,
            type_modules,
            traits,
            seen,
        );
    }
}

fn collect_owned_where_predicate_type_ids_deque(
    predicates: &[nia_defs::WherePredicateSignature],
    program_signatures: ExecutableSignatureIndex<'_>,
    interner: &TyInterner,
    type_modules: &mut HashSet<ModuleId>,
    traits: &mut ReachableTraitRefs,
    seen: &mut HashSet<(InternedTyId, Option<nia_ids::TyInternerId>)>,
) {
    let mut collected = Vec::new();
    collect_where_predicate_type_ids(predicates, &mut collected);
    collect_ty_ids_owner_modules_with_interner(
        collected,
        program_signatures,
        interner,
        type_modules,
        traits,
        seen,
    );
}

fn collect_array_len_owner_modules<'a>(
    len: &nia_ty::ArrayLenTy,
    current_interner: Option<&'a TyInterner>,
    type_ids: &mut VecDeque<PendingTy<'a>>,
) {
    if let nia_ty::ArrayLenTy::Builtin { ty, .. } = len {
        push_ty(type_ids, current_interner, *ty);
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
    current_interner: Option<&'a TyInterner>,
    type_ids: &mut VecDeque<PendingTy<'a>>,
    type_modules: &mut HashSet<ModuleId>,
    traits: &mut ReachableTraitRefs,
) {
    for binding in bindings {
        if let Some(trait_id) = binding.trait_id {
            collect_trait_id_owner_module(trait_id, type_modules, traits);
        }
        push_tys(
            type_ids,
            current_interner,
            binding.trait_args.iter().copied(),
        );
        push_tys(
            type_ids,
            current_interner,
            binding.trait_const_args.iter().map(|arg| arg.ty),
        );
        push_ty(type_ids, current_interner, binding.ty);
    }
}

fn push_ty<'a>(
    type_ids: &mut VecDeque<PendingTy<'a>>,
    interner: Option<&'a TyInterner>,
    ty: InternedTyId,
) {
    type_ids.push_back(PendingTy { ty, interner });
}

fn push_tys<'a>(
    type_ids: &mut VecDeque<PendingTy<'a>>,
    interner: Option<&'a TyInterner>,
    tys: impl IntoIterator<Item = InternedTyId>,
) {
    type_ids.extend(tys.into_iter().map(|ty| PendingTy { ty, interner }));
}
