use crate::LoaderContext;
use nia_compiler_query::{ProviderDemand, ProviderFactRevision, ProviderFactSnapshot};
use nia_query::{QueryDb, QueryFingerprintPolicy, QueryKey};
use std::{
    collections::HashSet,
    sync::{Arc, Mutex},
};

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct ProviderFacts {
    revision: ProviderFactRevision,
    reset_revision: ProviderFactRevision,
    demands: HashSet<ProviderDemand>,
}

impl ProviderFacts {
    pub(crate) fn revision(&self) -> ProviderFactRevision {
        self.revision
    }

    pub(crate) fn as_snapshot(&self) -> ProviderFactSnapshot {
        ProviderFactSnapshot::new(
            self.revision,
            self.reset_revision,
            self.demands.iter().cloned(),
        )
    }

    #[cfg(test)]
    pub(crate) fn demands(&self) -> impl Iterator<Item = &ProviderDemand> {
        self.demands.iter()
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum ProviderFactEvent {
    Current {
        demands: HashSet<ProviderDemand>,
    },
    Added {
        previous: ProviderFactRevision,
        demands: HashSet<ProviderDemand>,
    },
}

#[derive(Debug)]
struct ProviderFactState {
    current: ProviderFacts,
    transition: Option<ProviderFactEvent>,
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
                    reset_revision: revision,
                    demands: HashSet::new(),
                },
                transition: None,
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
            .all(|demand| state.current.demands.contains(demand))
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
            .filter(|demand| !state.current.demands.contains(demand))
            .collect::<HashSet<_>>();
        if !added.is_empty() {
            let previous = state.current.revision;
            let revision = previous.next();
            state.current.revision = revision;
            state.current.demands.extend(added.iter().cloned());
            state.transition = Some(ProviderFactEvent::Added {
                previous,
                demands: added.clone(),
            });
        }
        added
    }

    pub(crate) fn clear(&self) -> Option<ProviderFactRevision> {
        let mut state = self
            .state
            .lock()
            .expect("loader provider fact store lock poisoned");
        if state.current.demands.is_empty() {
            None
        } else {
            let previous = state.current.revision;
            let revision = previous.next();
            state.current.demands.clear();
            state.current.revision = revision;
            state.current.reset_revision = revision;
            state.transition = None;
            Some(previous)
        }
    }

    pub(crate) fn event(&self, revision: ProviderFactRevision) -> Option<ProviderFactEvent> {
        let state = self
            .state
            .lock()
            .expect("loader provider fact store lock poisoned");
        (state.current.revision == revision).then(|| {
            state
                .transition
                .clone()
                .unwrap_or_else(|| ProviderFactEvent::Current {
                    demands: state.current.demands.clone(),
                })
        })
    }

    pub(crate) fn compact_transition(&self, revision: ProviderFactRevision) {
        let mut state = self
            .state
            .lock()
            .expect("loader provider fact store lock poisoned");
        if state.current.revision == revision {
            state.transition = None;
        }
    }

    fn snapshot(&self) -> ProviderFacts {
        self.state
            .lock()
            .expect("loader provider fact store lock poisoned")
            .current
            .clone()
    }

    #[cfg(test)]
    pub(crate) fn retained_transition_count(&self) -> usize {
        usize::from(
            self.state
                .lock()
                .expect("loader provider fact store lock poisoned")
                .transition
                .is_some(),
        )
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub(crate) struct ProviderDemandsQuery;

impl QueryKey<LoaderContext> for ProviderDemandsQuery {
    type Value = ProviderFacts;

    const FINGERPRINT: QueryFingerprintPolicy = QueryFingerprintPolicy::SemanticValue;

    fn name() -> &'static str {
        "provider_demands"
    }

    fn execute(&self, db: &QueryDb<LoaderContext>) -> Self::Value {
        db.context().provider_facts.snapshot()
    }

    fn values_equal(&self, old: &Self::Value, new: &Self::Value) -> bool {
        old == new
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use nia_compiler_query::ProviderRequest;
    use nia_source::SourcePath;
    use nia_symbol::SymbolId;

    #[test]
    fn provider_fact_store_retains_only_the_current_transition() {
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
        assert!(store.event(initial.revision()).is_none());
        assert_eq!(store.retained_transition_count(), 1);
        assert!(store.insert_new([demand]).is_empty());
        assert_eq!(store.snapshot().revision(), added.revision());

        store.compact_transition(added.revision());
        assert_eq!(store.retained_transition_count(), 0);
        assert_eq!(
            store.event(added.revision()),
            Some(ProviderFactEvent::Current {
                demands: added.demands.clone()
            })
        );

        assert_eq!(store.clear(), Some(added.revision()));
        let cleared = store.snapshot();
        assert!(cleared.revision() > added.revision());
        assert!(cleared.demands().next().is_none());
        assert_eq!(
            store.event(cleared.revision()),
            Some(ProviderFactEvent::Current {
                demands: HashSet::new()
            })
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
        assert!(store.event(cleared.revision()).is_none());
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
