// SPDX-License-Identifier: GPL-3.0-or-later
//! Reachability result sets and module projections.
use super::*;

#[derive(Debug, Clone, Default, PartialEq, Eq)]
/// Least-fixed-point executable item set for one program revision.
pub struct ExecutableReachability {
    pub(super) modules: HashSet<ModuleId>,
    pub(super) type_modules: HashSet<ModuleId>,
    pub(super) functions: HashSet<GlobalDefId>,
    pub(super) globals: HashSet<GlobalDefId>,
    pub(super) stats: ExecutableReachabilityStats,
}

impl ExecutableReachability {
    /// Returns modules discovered as owners of reachable functions/globals.
    pub fn modules(&self) -> &HashSet<ModuleId> {
        &self.modules
    }

    /// Returns modules needed for type-only trait and layout resolution.
    pub fn type_modules(&self) -> &HashSet<ModuleId> {
        &self.type_modules
    }

    /// Returns reachable runtime function identities.
    pub fn functions(&self) -> &HashSet<GlobalDefId> {
        &self.functions
    }

    /// Returns reachable runtime global identities.
    pub fn globals(&self) -> &HashSet<GlobalDefId> {
        &self.globals
    }

    /// Returns the latest body-count statistics.
    pub fn stats(&self) -> ExecutableReachabilityStats {
        self.stats
    }

    /// Projects this result into per-module function/global sets.
    pub fn by_module(&self) -> ExecutableReachabilityByModule {
        ExecutableReachabilityByModule::new(self)
    }

    /// Inserts one function and its owner module.
    pub fn insert_function(&mut self, def_id: GlobalDefId) -> bool {
        let changed = self.functions.insert(def_id);
        self.modules.insert(def_id.module_id);
        changed
    }

    /// Inserts functions and returns whether any item was new.
    pub fn insert_functions(&mut self, def_ids: impl IntoIterator<Item = GlobalDefId>) -> bool {
        let mut changed = false;
        for def_id in def_ids {
            changed |= self.insert_function(def_id);
        }
        changed
    }

    /// Inserts one global and its owner module.
    pub fn insert_global(&mut self, def_id: GlobalDefId) -> bool {
        let changed = self.globals.insert(def_id);
        self.modules.insert(def_id.module_id);
        changed
    }

    /// Inserts globals and returns whether any item was new.
    pub fn insert_globals(&mut self, def_ids: impl IntoIterator<Item = GlobalDefId>) -> bool {
        let mut changed = false;
        for def_id in def_ids {
            changed |= self.insert_global(def_id);
        }
        changed
    }

    pub(super) fn insert_module(&mut self, module_id: ModuleId) -> bool {
        self.modules.insert(module_id)
    }

    pub(super) fn insert_module_pending(
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

    pub(super) fn insert_function_pending(
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

    pub(super) fn insert_global_pending(
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

    pub(super) fn set_stats(&mut self, stats: ExecutableReachabilityStats) {
        self.stats = stats;
    }

    pub(super) fn change_key(&self) -> (usize, usize, usize, usize) {
        (
            self.functions.len(),
            self.globals.len(),
            self.modules.len(),
            self.type_modules.len(),
        )
    }
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
/// Function/global reachability partition for one module.
pub struct ExecutableModuleReachability {
    /// Reachable functions owned by the module.
    pub functions: HashSet<GlobalDefId>,
    /// Reachable globals owned by the module.
    pub globals: HashSet<GlobalDefId>,
}

impl ExecutableModuleReachability {
    /// Reports whether this partition contains runtime body work.
    pub fn has_body_items(&self, is_runtime_global: impl FnMut(GlobalDefId) -> bool) -> bool {
        !self.functions.is_empty() || self.globals.iter().copied().any(is_runtime_global)
    }
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
/// Read-only index of executable items grouped by module.
pub struct ExecutableReachabilityByModule {
    modules: HashMap<ModuleId, ExecutableModuleReachability>,
}

impl ExecutableReachabilityByModule {
    /// Builds a module index from a fixed-point result.
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

    /// Looks up one module's reachable item partition.
    pub fn get(&self, module_id: ModuleId) -> Option<&ExecutableModuleReachability> {
        self.modules.get(&module_id)
    }

    /// Returns modules containing functions or runtime globals.
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
/// Counts of checked and retained executable bodies.
pub struct ExecutableReachabilityStats {
    /// Number of checked module products considered.
    pub checked_modules: usize,
    /// Number of function bodies available in those products.
    pub checked_bodies: usize,
    /// Number of available bodies selected by reachability.
    pub reachable_bodies: usize,
}
