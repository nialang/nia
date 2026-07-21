use crate::LoaderContext;
use nia_compiler_query::ProviderDemand;
use nia_query::{QueryDb, QueryKey};
use std::{
    collections::HashSet,
    sync::{Arc, Mutex},
};

#[derive(Clone, Default)]
pub(crate) struct ProviderFactStore {
    demands: Arc<Mutex<HashSet<ProviderDemand>>>,
}

impl ProviderFactStore {
    pub(crate) fn contains_all(&self, demands: &[ProviderDemand]) -> bool {
        let stored = self
            .demands
            .lock()
            .expect("loader provider fact store lock poisoned");
        demands.iter().all(|demand| stored.contains(demand))
    }

    pub(crate) fn insert_new(
        &self,
        demands: impl IntoIterator<Item = ProviderDemand>,
    ) -> HashSet<ProviderDemand> {
        let mut stored = self
            .demands
            .lock()
            .expect("loader provider fact store lock poisoned");
        demands
            .into_iter()
            .filter(|demand| stored.insert(demand.clone()))
            .collect()
    }

    pub(crate) fn clear(&self) -> bool {
        let mut stored = self
            .demands
            .lock()
            .expect("loader provider fact store lock poisoned");
        let changed = !stored.is_empty();
        stored.clear();
        changed
    }

    fn snapshot(&self) -> HashSet<ProviderDemand> {
        self.demands
            .lock()
            .expect("loader provider fact store lock poisoned")
            .clone()
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub(crate) struct ProviderDemandsQuery;

impl QueryKey<LoaderContext> for ProviderDemandsQuery {
    type Value = HashSet<ProviderDemand>;

    fn name() -> &'static str {
        "provider_demands"
    }

    fn execute(&self, db: &QueryDb<LoaderContext>) -> Self::Value {
        db.context().provider_facts.snapshot()
    }
}
