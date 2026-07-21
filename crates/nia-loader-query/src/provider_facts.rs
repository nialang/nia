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
    demands: HashMap<ProviderDemand, ProviderFactRevision>,
}

impl ProviderFacts {
    pub(crate) fn revision(&self) -> ProviderFactRevision {
        self.revision
    }

    #[cfg(test)]
    pub(crate) fn demands(&self) -> impl Iterator<Item = &ProviderDemand> {
        self.demands.keys()
    }

    #[cfg(test)]
    pub(crate) fn added_after(
        &self,
        revision: ProviderFactRevision,
    ) -> impl Iterator<Item = &ProviderDemand> {
        self.demands
            .iter()
            .filter_map(move |(demand, added)| added.is_newer_than(revision).then_some(demand))
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum ProviderFactEvent {
    Root,
    Added {
        previous: ProviderFactRevision,
        demands: HashSet<ProviderDemand>,
    },
    Reset,
}

#[derive(Debug)]
struct ProviderFactState {
    current: ProviderFacts,
    events: HashMap<ProviderFactRevision, ProviderFactEvent>,
}

#[derive(Clone)]
pub(crate) struct ProviderFactStore {
    state: Arc<Mutex<ProviderFactState>>,
}

impl Default for ProviderFactStore {
    fn default() -> Self {
        let revision = ProviderFactRevision::new_store();
        Self {
            state: Arc::new(Mutex::new(ProviderFactState {
                current: ProviderFacts {
                    revision,
                    demands: HashMap::new(),
                },
                events: HashMap::from([(revision, ProviderFactEvent::Root)]),
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
            .all(|demand| state.current.demands.contains_key(demand))
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
            .filter(|demand| !state.current.demands.contains_key(demand))
            .collect::<HashSet<_>>();
        if !added.is_empty() {
            let previous = state.current.revision;
            let revision = previous.next();
            state.current.revision = revision;
            state
                .current
                .demands
                .extend(added.iter().cloned().map(|demand| (demand, revision)));
            state.events.insert(
                revision,
                ProviderFactEvent::Added {
                    previous,
                    demands: added.clone(),
                },
            );
        }
        added
    }

    pub(crate) fn clear(&self) -> bool {
        let mut state = self
            .state
            .lock()
            .expect("loader provider fact store lock poisoned");
        let changed = !state.current.demands.is_empty();
        if changed {
            let previous = state.current.revision;
            let revision = previous.next();
            state.current.demands.clear();
            state.current.revision = revision;
            state.events.insert(revision, ProviderFactEvent::Reset);
        }
        changed
    }

    pub(crate) fn event(&self, revision: ProviderFactRevision) -> Option<ProviderFactEvent> {
        self.state
            .lock()
            .expect("loader provider fact store lock poisoned")
            .events
            .get(&revision)
            .cloned()
    }

    fn snapshot(&self) -> ProviderFacts {
        self.state
            .lock()
            .expect("loader provider fact store lock poisoned")
            .current
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
        assert_eq!(added.added_after(initial.revision()).count(), 1);
        assert!(store.insert_new([demand]).is_empty());
        assert_eq!(store.snapshot().revision(), added.revision());

        assert!(store.clear());
        let cleared = store.snapshot();
        assert!(cleared.revision() > added.revision());
        assert!(cleared.demands().next().is_none());
        assert_eq!(
            store.event(cleared.revision()),
            Some(ProviderFactEvent::Reset)
        );

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
        assert_eq!(
            store.event(replaced.revision()),
            Some(ProviderFactEvent::Added {
                previous: cleared.revision(),
                demands: HashSet::from([ProviderDemand {
                    source_path: SourcePath::new("main.nia"),
                    request: ProviderRequest::TraitImpl {
                        trait_name: SymbolId::default(),
                    },
                }])
            })
        );
    }
}
