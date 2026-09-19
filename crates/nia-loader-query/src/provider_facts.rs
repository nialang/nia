use crate::LoaderContext;
use nia_compiler_query::{ProviderDemand, ProviderFactRevision, ProviderFactSnapshot};
use nia_query::{QueryDb, QueryError, QueryFingerprintPolicy, QueryKey, QueryResult};
use parking_lot::Mutex;
use std::{collections::HashSet, sync::Arc};

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

#[cfg(test)]
impl Default for ProviderFactStore {
    fn default() -> Self {
        let revision = ProviderFactRevision::new_store().expect("provider fact test store");
        Self::from_revision(revision)
    }
}

impl ProviderFactStore {
    pub(crate) fn new() -> QueryResult<Self> {
        let revision = ProviderFactRevision::new_store()
            .ok_or_else(|| QueryError::internal("provider fact owner space exhausted"))?;
        Ok(Self::from_revision(revision))
    }

    fn from_revision(revision: ProviderFactRevision) -> Self {
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
    pub(crate) fn revision_if_nonempty(&self) -> Option<ProviderFactRevision> {
        let state = self.state.lock();
        (!state.current.demands.is_empty()).then_some(state.current.revision)
    }

    pub(crate) fn contains_all(&self, demands: &[ProviderDemand]) -> bool {
        let state = self.state.lock();
        demands
            .iter()
            .all(|demand| state.current.demands.contains(demand))
    }

    pub(crate) fn insert_new(
        &self,
        demands: impl IntoIterator<Item = ProviderDemand>,
    ) -> QueryResult<HashSet<ProviderDemand>> {
        let mut state = self.state.lock();
        let added = demands
            .into_iter()
            .filter(|demand| !state.current.demands.contains(demand))
            .collect::<HashSet<_>>();
        if !added.is_empty() {
            let previous = state.current.revision;
            let revision = previous
                .next()
                .ok_or_else(|| QueryError::internal("provider fact revision overflow"))?;
            state.current.revision = revision;
            state.current.demands.extend(added.iter().cloned());
            state.transition = Some(ProviderFactEvent::Added {
                previous,
                demands: added.clone(),
            });
        }
        Ok(added)
    }

    pub(crate) fn clear(&self) -> QueryResult<Option<ProviderFactRevision>> {
        let mut state = self.state.lock();
        if state.current.demands.is_empty() {
            Ok(None)
        } else {
            let previous = state.current.revision;
            let revision = previous
                .next()
                .ok_or_else(|| QueryError::internal("provider fact revision overflow"))?;
            state.current.demands.clear();
            state.current.revision = revision;
            state.current.reset_revision = revision;
            state.transition = None;
            Ok(Some(previous))
        }
    }

    pub(crate) fn event(&self, revision: ProviderFactRevision) -> Option<ProviderFactEvent> {
        let state = self.state.lock();
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
        let mut state = self.state.lock();
        if state.current.revision == revision {
            state.transition = None;
        }
    }

    fn snapshot(&self) -> ProviderFacts {
        self.state.lock().current.clone()
    }

    #[cfg(test)]
    pub(crate) fn retained_transition_count(&self) -> usize {
        usize::from(self.state.lock().transition.is_some())
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

    fn execute_result(&self, db: &QueryDb<LoaderContext>) -> QueryResult<Self::Value> {
        Ok(db.context().provider_facts.snapshot())
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

        assert_eq!(
            store
                .insert_new([demand.clone()])
                .expect("insert test demand")
                .len(),
            1
        );
        let added = store.snapshot();
        assert!(added.revision() > initial.revision());
        assert!(store.event(initial.revision()).is_none());
        assert_eq!(store.retained_transition_count(), 1);
        assert!(
            store
                .insert_new([demand])
                .expect("repeat test demand")
                .is_empty()
        );
        assert_eq!(store.snapshot().revision(), added.revision());

        store.compact_transition(added.revision());
        assert_eq!(store.retained_transition_count(), 0);
        assert_eq!(
            store.event(added.revision()),
            Some(ProviderFactEvent::Current {
                demands: added.demands.clone()
            })
        );

        assert_eq!(
            store.clear().expect("clear test provider facts"),
            Some(added.revision())
        );
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
                target_type_name: None,
                trait_name: SymbolId::default(),
                trait_type_argument_names: Vec::new(),
            },
        };
        assert_eq!(
            store
                .insert_new([replacement.clone()])
                .expect("insert replacement demand"),
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
                        target_type_name: None,
                        trait_name: SymbolId::default(),
                        trait_type_argument_names: Vec::new(),
                    },
                }])
            })
        );
    }
}
