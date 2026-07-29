// SPDX-License-Identifier: GPL-3.0-or-later
use super::*;

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ExecutableReachability {
    pub(super) modules: HashSet<ModuleId>,
    pub(super) type_modules: HashSet<ModuleId>,
    pub(super) functions: HashSet<GlobalDefId>,
    pub(super) globals: HashSet<GlobalDefId>,
    pub(super) stats: ExecutableReachabilityStats,
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
