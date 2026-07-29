// SPDX-License-Identifier: GPL-3.0-or-later
use super::*;

#[derive(Debug, Clone)]
pub(super) enum ActiveBodyCheckFilter<'a> {
    All,
    ReachableItems {
        functions: &'a HashSet<GlobalDefId>,
        globals: &'a HashSet<GlobalDefId>,
        already_checked_functions: Option<&'a HashSet<GlobalDefId>>,
        already_checked_globals: Option<&'a HashSet<GlobalDefId>>,
        discovered_functions: HashSet<GlobalDefId>,
    },
}

impl<'a> ActiveBodyCheckFilter<'a> {
    pub(super) fn from_filter(filter: BodyCheckFilter<'a>) -> Self {
        match filter {
            BodyCheckFilter::All => Self::All,
            BodyCheckFilter::ReachableFunctions(functions) => Self::ReachableItems {
                functions,
                globals: empty_global_def_ids(),
                already_checked_functions: None,
                already_checked_globals: None,
                discovered_functions: HashSet::new(),
            },
            BodyCheckFilter::ReachableItems {
                functions,
                globals,
                already_checked_functions,
                already_checked_globals,
            } => Self::ReachableItems {
                functions,
                globals,
                already_checked_functions,
                already_checked_globals,
                discovered_functions: HashSet::new(),
            },
        }
    }

    pub(super) fn includes_function(&self, def_id: GlobalDefId) -> bool {
        match self {
            Self::All => true,
            Self::ReachableItems {
                functions,
                already_checked_functions,
                discovered_functions,
                ..
            } => {
                (functions.contains(&def_id) || discovered_functions.contains(&def_id))
                    && already_checked_functions.is_none_or(|checked| !checked.contains(&def_id))
            }
        }
    }

    pub(super) fn includes_global(&self, def_id: GlobalDefId) -> bool {
        match self {
            Self::All => true,
            Self::ReachableItems {
                globals,
                already_checked_globals,
                ..
            } => {
                globals.contains(&def_id)
                    && already_checked_globals.is_none_or(|checked| !checked.contains(&def_id))
            }
        }
    }

    pub(super) fn selects_global(&self, def_id: GlobalDefId) -> bool {
        match self {
            Self::All => true,
            Self::ReachableItems { globals, .. } => globals.contains(&def_id),
        }
    }

    pub(super) fn add_function(&mut self, def_id: GlobalDefId) -> bool {
        match self {
            Self::All => false,
            Self::ReachableItems {
                functions,
                already_checked_functions,
                discovered_functions,
                ..
            } => {
                if already_checked_functions.is_some_and(|checked| checked.contains(&def_id)) {
                    return false;
                }
                if functions.contains(&def_id) {
                    return false;
                }
                discovered_functions.insert(def_id)
            }
        }
    }

    pub(super) fn initial_functions(
        &self,
        available: &HashMap<GlobalDefId, FunctionItemRef<'_>>,
    ) -> Vec<GlobalDefId> {
        match self {
            Self::All => available.keys().copied().collect(),
            Self::ReachableItems {
                functions,
                already_checked_functions,
                ..
            } => functions
                .iter()
                .copied()
                .filter(|def_id| {
                    already_checked_functions.is_none_or(|checked| !checked.contains(def_id))
                })
                .filter(|def_id| available.contains_key(def_id))
                .collect(),
        }
    }
}

fn empty_global_def_ids() -> &'static HashSet<GlobalDefId> {
    static EMPTY: std::sync::OnceLock<HashSet<GlobalDefId>> = std::sync::OnceLock::new();
    EMPTY.get_or_init(HashSet::new)
}
