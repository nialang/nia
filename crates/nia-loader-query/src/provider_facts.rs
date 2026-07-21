use crate::LoaderContext;
use nia_compiler_query::{ProviderDemand, ProviderFactRevision};
use nia_query::{QueryDb, QueryKey};
use std::{
    collections::{HashMap, HashSet},
    sync::{Arc, Mutex},
};

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct ProviderFacts {
    revision: ProviderFactRevision,
    reset_revision: ProviderFactRevision,
    demands: HashMap<ProviderDemand, ProviderFactRevision>,
}

impl ProviderFacts {
    pub(crate) fn revision(&self) -> ProviderFactRevision {
        self.revision
    }

    pub(crate) fn demands(&self) -> impl Iterator<Item = &ProviderDemand> {
        self.demands.keys()
    }

    pub(crate) fn added_after(
        &self,
        revision: ProviderFactRevision,
    ) -> impl Iterator<Item = &ProviderDemand> {
        self.demands
            .iter()
            .filter_map(move |(demand, added)| added.is_newer_than(revision).then_some(demand))
    }

    pub(crate) fn can_extend_graph_after(&self, revision: ProviderFactRevision) -> bool {
        matches!(
            self.revision.transition_from(revision),
            nia_compiler_query::ProviderFactRevisionTransition::Advanced
        ) && !self.reset_revision.is_newer_than(revision)
    }
}

#[derive(Clone)]
pub(crate) struct ProviderFactStore {
    state: Arc<Mutex<ProviderFacts>>,
}

impl Default for ProviderFactStore {
    fn default() -> Self {
        let revision = ProviderFactRevision::new_store();
        Self {
            state: Arc::new(Mutex::new(ProviderFacts {
                revision,
                reset_revision: revision,
                demands: HashMap::new(),
            })),
        }
    }
}

impl ProviderFactStore {
    pub(crate) fn contains_all(&self, demands: &[ProviderDemand]) -> bool {
        let state = self
            .state
            .lock()
            .expect("loader provider fact store lock poisoned");
        demands
            .iter()
            .all(|demand| state.demands.contains_key(demand))
    }

    pub(crate) fn insert_new(
        &self,
        demands: impl IntoIterator<Item = ProviderDemand>,
    ) -> HashSet<ProviderDemand> {
        let mut state = self
            .state
            .lock()
            .expect("loader provider fact store lock poisoned");
        let added = demands
            .into_iter()
            .filter(|demand| !state.demands.contains_key(demand))
            .collect::<HashSet<_>>();
        if !added.is_empty() {
            state.revision = state.revision.next();
            let revision = state.revision;
            state
                .demands
                .extend(added.iter().cloned().map(|demand| (demand, revision)));
        }
        added
    }

    pub(crate) fn clear(&self) -> bool {
        let mut state = self
            .state
            .lock()
            .expect("loader provider fact store lock poisoned");
        let changed = !state.demands.is_empty();
        if changed {
            state.demands.clear();
            state.revision = state.revision.next();
            state.reset_revision = state.revision;
        }
        changed
    }

    fn snapshot(&self) -> ProviderFacts {
        self.state
            .lock()
            .expect("loader provider fact store lock poisoned")
            .clone()
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub(crate) struct ProviderDemandsQuery;

impl QueryKey<LoaderContext> for ProviderDemandsQuery {
    type Value = ProviderFacts;

    fn name() -> &'static str {
        "provider_demands"
    }

    fn execute(&self, db: &QueryDb<LoaderContext>) -> Self::Value {
        db.context().provider_facts.snapshot()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use nia_compiler_query::ProviderRequest;
    use nia_source::SourcePath;
    use nia_symbol::SymbolId;

    #[test]
    fn provider_fact_store_tracks_monotonic_delta_revisions() {
        assert_eq!(std::mem::size_of::<ProviderFactRevision>(), 16);
        let store = ProviderFactStore::default();
        let demand = ProviderDemand {
            source_path: SourcePath::new("main.nia"),
            request: ProviderRequest::Method {
                target_type_name: None,
                method_name: SymbolId::default(),
            },
        };
        let initial = store.snapshot();
        assert!(initial.demands().next().is_none());
        assert_ne!(
            initial.revision(),
            ProviderFactStore::default().snapshot().revision()
        );

        assert_eq!(store.insert_new([demand.clone()]).len(), 1);
        let added = store.snapshot();
        assert!(added.revision() > initial.revision());
        assert!(added.can_extend_graph_after(initial.revision()));
        assert_eq!(added.added_after(initial.revision()).count(), 1);
        assert!(store.insert_new([demand]).is_empty());
        assert_eq!(store.snapshot().revision(), added.revision());

        assert!(store.clear());
        let cleared = store.snapshot();
        assert!(cleared.revision() > added.revision());
        assert!(cleared.demands().next().is_none());
        assert!(!cleared.can_extend_graph_after(added.revision()));

        let replacement = ProviderDemand {
            source_path: SourcePath::new("main.nia"),
            request: ProviderRequest::TraitImpl {
                trait_name: SymbolId::default(),
            },
        };
        assert_eq!(
            store.insert_new([replacement.clone()]),
            HashSet::from([replacement])
        );
        let replaced = store.snapshot();
        assert!(!replaced.can_extend_graph_after(added.revision()));
        assert!(replaced.can_extend_graph_after(cleared.revision()));
    }
}
