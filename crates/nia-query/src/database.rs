// SPDX-License-Identifier: GPL-3.0-or-later
//! Typed query caches and their incremental state machine.
//!
//! Shared queries use red/green validation and stable fingerprints. Owned
//! queries transfer a payload exactly once and never participate in validation.
//! Retirement mutates cache identity only after the session is quiescent.

use super::*;

impl<C> QueryDb<C> {
    pub(super) fn retire_scope_during_retirement(
        &self,
        retain: &dyn Fn(&QueryFrame) -> bool,
    ) -> QueryResult<()> {
        let ids = {
            let slots = self.inner.slots.lock();
            slots
                .entries
                .iter()
                .filter_map(|(index, record)| {
                    (!retain(&record.identity.frame())).then_some(QueryNodeId {
                        db_id: self.inner.id,
                        index: *index,
                    })
                })
                .collect::<Vec<_>>()
        };
        let retained = {
            let slots = self.inner.slots.lock();
            slots
                .entries
                .iter()
                .filter_map(|(index, record)| {
                    retain(&record.identity.frame()).then_some(QueryNodeId {
                        db_id: self.inner.id,
                        index: *index,
                    })
                })
                .collect::<Vec<_>>()
        };
        for id in &retained {
            self.inner.session.slot(*id)?.stabilize();
        }
        for id in &ids {
            self.inner.session.slot(*id)?.invalidate();
        }
        let node_set = FastHashSet::from_iter(ids.iter().copied());
        self.inner
            .caches
            .read()
            .values()
            .for_each(|cache| cache.remove_nodes(&node_set));
        {
            let mut slots = self.inner.slots.lock();
            for id in &ids {
                slots.remove(self.inner.id, *id);
            }
        }
        let mut dependencies = self.inner.session.inner.dependencies.lock();
        for id in &ids {
            dependencies.remove_node(*id);
        }
        for id in retained {
            dependencies.remove_dependencies_from(id);
        }
        Ok(())
    }

    /// Creates an unregistered database with an isolated default session.
    pub fn new(context: C) -> nia_ice::IceResult<Self>
    where
        C: Send + Sync + 'static,
    {
        Self::new_with_timings(context, nia_timing::TimingMode::Off)
    }

    #[cfg(test)]
    pub(super) fn new_for_test(context: C) -> Self
    where
        C: Send + Sync + 'static,
    {
        Self::new(context).unwrap_or_else(|ice| panic!("failed to create query database: {ice}"))
    }

    /// Creates an unregistered database with timing instrumentation enabled as requested.
    pub fn new_with_timings(context: C, timings: nia_timing::TimingMode) -> nia_ice::IceResult<Self>
    where
        C: Send + Sync + 'static,
    {
        Self::new_with_timings_in_session(context, timings, QuerySession::new()?)
    }

    /// Creates an unregistered database attached to an explicitly shared session.
    pub fn new_with_timings_in_session(
        context: C,
        timings: nia_timing::TimingMode,
        session: QuerySession,
    ) -> nia_ice::IceResult<Self>
    where
        C: Send + Sync + 'static,
    {
        Self::new_inner(context, timings, None, session)
    }

    /// Creates a database that enforces membership in `registry` for every query type.
    pub fn new_registered(context: C, registry: QueryRegistry) -> nia_ice::IceResult<Self>
    where
        C: Send + Sync + 'static,
    {
        Self::new_registered_with_timings(context, nia_timing::TimingMode::Off, registry)
    }

    #[cfg(test)]
    pub(super) fn new_registered_for_test(context: C, registry: QueryRegistry) -> Self
    where
        C: Send + Sync + 'static,
    {
        Self::new_registered(context, registry)
            .unwrap_or_else(|ice| panic!("failed to create registered query database: {ice}"))
    }

    /// Creates a registered database attached to an explicitly shared session.
    pub fn new_registered_in_session(
        context: C,
        registry: QueryRegistry,
        session: QuerySession,
    ) -> nia_ice::IceResult<Self>
    where
        C: Send + Sync + 'static,
    {
        Self::new_registered_with_timings_in_session(
            context,
            nia_timing::TimingMode::Off,
            registry,
            session,
        )
    }

    /// Creates a registered database with timing instrumentation enabled as requested.
    pub fn new_registered_with_timings(
        context: C,
        timings: nia_timing::TimingMode,
        registry: QueryRegistry,
    ) -> nia_ice::IceResult<Self>
    where
        C: Send + Sync + 'static,
    {
        Self::new_registered_with_timings_in_session(
            context,
            timings,
            registry,
            QuerySession::new()?,
        )
    }

    /// Creates a registered database with explicit timing and session configuration.
    pub fn new_registered_with_timings_in_session(
        context: C,
        timings: nia_timing::TimingMode,
        registry: QueryRegistry,
        session: QuerySession,
    ) -> nia_ice::IceResult<Self>
    where
        C: Send + Sync + 'static,
    {
        Self::new_inner(context, timings, Some(registry), session)
    }

    fn new_inner(
        context: C,
        timings: nia_timing::TimingMode,
        registry: Option<QueryRegistry>,
        session: QuerySession,
    ) -> nia_ice::IceResult<Self>
    where
        C: Send + Sync + 'static,
    {
        let db = Self {
            inner: Arc::new(QueryDbInner {
                id: QueryDbId::fresh()?,
                session: session.clone(),
                context,
                timings,
                registry,
                caches: RwLock::new(FastHashMap::default()),
                slots: Mutex::new(QuerySlotTable::default()),
            }),
        };
        session.register(&db)?;
        Ok(db)
    }

    /// Returns the immutable compiler context owned by this database.
    pub fn context(&self) -> &C {
        &self.inner.context
    }

    /// Returns a clone of the session handle governing this database.
    pub fn session(&self) -> QuerySession {
        self.inner.session.clone()
    }

    /// Lists registered query contracts sorted by query name, or an empty list for unregistered DBs.
    pub fn registered_queries(&self) -> Vec<QueryDescriptor> {
        self.inner
            .registry
            .as_ref()
            .map(QueryRegistry::descriptors)
            .unwrap_or_default()
    }

    /// Builds an [`QueryError::InvalidInput`] tied to the frame for `key`.
    pub fn invalid_input<K>(&self, key: &K, message: impl Into<String>) -> QueryError
    where
        K: QueryKey<C>,
    {
        QueryError::InvalidInput {
            query: query_frame::<C, K>(key),
            message: message.into(),
        }
    }

    fn internal_query_error<K>(key: &K, message: impl Into<String>) -> QueryError
    where
        K: QueryKey<C>,
    {
        QueryError::internal(message).with_query_context(query_frame::<C, K>(key))
    }

    /// Gets a shared cached value, executing and memoizing it when necessary.
    ///
    /// `K` must use [`QueryStoragePolicy::CacheOwnedArc`]. The returned `Arc`
    /// remains valid while invalidation or retirement removes the slot identity.
    pub fn get<K>(&self, key: K) -> QueryResult<Arc<K::Value>>
    where
        K: QueryKey<C>,
    {
        self.try_get_cached(key)
    }

    /// Gets and consumes a single-consumer owned value.
    ///
    /// A key-executed owned query recomputes after consumption. An externally
    /// published query returns an error until its producer calls
    /// [`Self::publish_owned`], and a published payload can be consumed only once.
    pub fn get_owned<K>(&self, key: K) -> QueryResult<K::Value>
    where
        K: QueryKey<C>,
    {
        if K::STORAGE != QueryStoragePolicy::SingleConsumerOwned {
            return Err(Self::internal_query_error(
                &key,
                "query does not declare single-consumer owned storage",
            ));
        }
        if K::FINGERPRINT != QueryFingerprintPolicy::None {
            return Err(Self::internal_query_error(
                &key,
                "single-consumer query cannot retain a value fingerprint",
            ));
        }
        let _activity = self.inner.session.enter_activity();
        let detail_timing = self.inner.timings.detail();
        let slot =
            nia_timing::time_detail(detail_timing, "query.slot_for", || self.slot_for(&key))?;
        let node_id = slot.node_id;
        nia_timing::time_detail(detail_timing, "query.record_dependency", || {
            record_dependency_on_current_stack(self.inner.session.inner.id, node_id)
        });
        loop {
            let mut state = slot.state.lock();
            match &*state {
                QueryState::Published { .. } => {
                    let previous = std::mem::replace(&mut *state, QueryState::Consumed);
                    let value = match previous {
                        QueryState::Published { value } => value,
                        unexpected => {
                            *state = unexpected;
                            return Err(Self::internal_query_error(
                                &key,
                                "published query state changed while locked",
                            ));
                        }
                    };
                    slot.ready.notify_all();
                    record_dependency_fingerprint_on_current_stack(
                        self.inner.session.inner.id,
                        node_id,
                        None,
                    );
                    return Ok(value);
                }
                QueryState::Empty | QueryState::Consumed => {
                    if K::PROVIDER == QueryProviderPolicy::ExternallyPublished {
                        return Err(QueryError::InvalidInput {
                            query: query_frame::<C, K>(&key),
                            message: "owned product has not been published by its producer".into(),
                        });
                    }
                    *state = QueryState::Computing { invalidated: false };
                    drop(state);

                    self.clear_dependencies_from(node_id);
                    let entry = QueryStackEntry {
                        session_id: self.inner.session.inner.id,
                        node_id,
                        identity: Arc::clone(&slot.identity),
                        dependencies: FastHashSet::default(),
                        dependency_fingerprints: None,
                    };
                    let mut guard = self.enter_query(entry)?;
                    nia_timing::time_detail(detail_timing, "query.record_execution", || {
                        slot.stats.record_execution()
                    });
                    let value = match self.inner.session.inner.capture_unexpected_panic(|| {
                        nia_timing::time_detail(detail_timing, "query.provider", || {
                            key.execute_result(self)
                        })
                    }) {
                        Ok(Ok(value)) => value,
                        Ok(Err(error)) => {
                            let mut state = slot.state.lock();
                            *state = QueryState::Empty;
                            guard.discard();
                            self.clear_dependencies_from(node_id);
                            slot.ready.notify_all();
                            return Err(error.with_query_context(query_frame::<C, K>(&key)));
                        }
                        Err(ice) => {
                            let mut state = slot.state.lock();
                            *state = QueryState::Empty;
                            guard.discard();
                            self.clear_dependencies_from(node_id);
                            slot.ready.notify_all();
                            drop(state);
                            return Err(QueryError::Internal(ice)
                                .with_query_context(query_frame::<C, K>(&key)));
                        }
                    };

                    let mut state = slot.state.lock();
                    let was_invalidated =
                        matches!(&*state, QueryState::Computing { invalidated: true });
                    if was_invalidated {
                        *state = QueryState::Empty;
                        guard.discard();
                        self.clear_dependencies_from(node_id);
                    } else {
                        let dependencies = guard.take_dependencies();
                        self.replace_dependencies_from(node_id, dependencies.nodes);
                        *state = QueryState::Consumed;
                    }
                    slot.ready.notify_all();
                    record_dependency_fingerprint_on_current_stack(
                        self.inner.session.inner.id,
                        node_id,
                        None,
                    );
                    return Ok(value);
                }
                QueryState::Computing { .. } | QueryState::Validating { .. } => {
                    self.check_not_recursive_node(node_id)?;
                    nia_timing::time_detail(detail_timing, "query.record_wait", || {
                        slot.stats.record_wait()
                    });
                    // Thread-local recursion catches a query asking for itself. The wait-for
                    // graph covers the distinct-worker case (A waits for B while B waits for A)
                    // before this condition-variable wait can deadlock the executor.
                    let _wait = self
                        .inner
                        .session
                        .begin_query_wait(node_id, query_frame::<C, K>(&key))?;
                    slot.ready.wait(&mut state);
                }
                QueryState::Ready { .. } | QueryState::PotentiallyOutdated { .. } => {
                    return Err(Self::internal_query_error(
                        &key,
                        "single-consumer query reached shared cache state",
                    ));
                }
            }
        }
    }

    /// Returns whether an externally published owned slot can accept a new
    /// payload. A live published payload must remain untouched until its
    /// consumer takes ownership; empty and consumed slots are publishable.
    pub fn can_publish_owned<K>(&self, key: K) -> QueryResult<bool>
    where
        K: QueryKey<C>,
    {
        if K::STORAGE != QueryStoragePolicy::SingleConsumerOwned {
            return Err(Self::internal_query_error(
                &key,
                "query does not declare single-consumer owned storage",
            ));
        }
        if K::PROVIDER != QueryProviderPolicy::ExternallyPublished {
            return Err(Self::internal_query_error(
                &key,
                "query does not declare an external producer",
            ));
        }
        let slot = self.slot_for(&key)?;
        let state = slot.state.lock();
        Ok(matches!(&*state, QueryState::Empty | QueryState::Consumed))
    }

    /// Publishes an already-owned payload to one externally-published query slot.
    ///
    /// The published slot depends on `predecessor`, so invalidating the producer
    /// drops an unconsumed payload and invalidates an already-consumed consumer.
    pub fn publish_owned<K, P>(&self, key: K, value: K::Value, predecessor: &P) -> QueryResult<()>
    where
        K: QueryKey<C>,
        P: QueryKey<C>,
    {
        if K::STORAGE != QueryStoragePolicy::SingleConsumerOwned {
            return Err(Self::internal_query_error(
                &key,
                "published query must use single-consumer owned storage",
            ));
        }
        if K::PROVIDER != QueryProviderPolicy::ExternallyPublished {
            return Err(Self::internal_query_error(
                &key,
                "query does not declare an external producer",
            ));
        }
        if K::FINGERPRINT != QueryFingerprintPolicy::None {
            return Err(Self::internal_query_error(
                &key,
                "published query cannot retain a value fingerprint",
            ));
        }
        let _activity = self.inner.session.enter_activity();
        let slot = self.slot_for(&key)?;
        let predecessor_slot = self.slot_for(predecessor)?;
        {
            let mut state = slot.state.lock();
            if !matches!(&*state, QueryState::Empty | QueryState::Consumed) {
                return Err(Self::internal_query_error(
                    &key,
                    "published query already has a live payload or consumer",
                ));
            }
            *state = QueryState::Published { value };
            slot.ready.notify_all();
        }
        self.replace_dependencies_from(
            slot.node_id,
            FastHashSet::from_iter([predecessor_slot.node_id]),
        );
        Ok(())
    }

    /// Returns whether an externally published shared slot can accept its payload.
    pub fn can_publish_shared<K>(&self, key: K) -> QueryResult<bool>
    where
        K: QueryKey<C>,
    {
        if K::STORAGE != QueryStoragePolicy::CacheOwnedArc {
            return Err(Self::internal_query_error(
                &key,
                "query does not declare shared cache storage",
            ));
        }
        if K::PROVIDER != QueryProviderPolicy::ExternallyPublished {
            return Err(Self::internal_query_error(
                &key,
                "query does not declare an external producer",
            ));
        }
        let slot = self.slot_for(&key)?;
        let state = slot.state.lock();
        Ok(matches!(&*state, QueryState::Empty))
    }

    /// Publishes an immutable shared payload with an explicit predecessor.
    pub fn publish_shared<K, P>(&self, key: K, value: K::Value, predecessor: &P) -> QueryResult<()>
    where
        K: QueryKey<C>,
        P: QueryKey<C>,
    {
        if K::STORAGE != QueryStoragePolicy::CacheOwnedArc {
            return Err(Self::internal_query_error(
                &key,
                "published query must use shared cache storage",
            ));
        }
        if K::PROVIDER != QueryProviderPolicy::ExternallyPublished {
            return Err(Self::internal_query_error(
                &key,
                "query does not declare an external producer",
            ));
        }
        if K::FINGERPRINT != QueryFingerprintPolicy::None {
            return Err(Self::internal_query_error(
                &key,
                "published query cannot retain a value fingerprint",
            ));
        }
        let _activity = self.inner.session.enter_activity();
        let slot = self.slot_for(&key)?;
        let predecessor_slot = self.slot_for(predecessor)?;
        {
            let mut state = slot.state.lock();
            if !matches!(&*state, QueryState::Empty) {
                return Err(Self::internal_query_error(
                    &key,
                    "published query already has a live payload",
                ));
            }
            *state = QueryState::Ready {
                value: Arc::new(value),
                fingerprint: None,
                dependency_fingerprints: DependencyFingerprints::default(),
            };
            slot.ready.notify_all();
        }
        self.replace_dependencies_from(
            slot.node_id,
            FastHashSet::from_iter([predecessor_slot.node_id]),
        );
        Ok(())
    }

    fn try_get_cached<K>(&self, key: K) -> QueryResult<Arc<K::Value>>
    where
        K: QueryKey<C>,
    {
        if K::STORAGE != QueryStoragePolicy::CacheOwnedArc {
            return Err(Self::internal_query_error(
                &key,
                "single-consumer query must be requested with get_owned",
            ));
        }
        let _activity = self.inner.session.enter_activity();
        let detail_timing = self.inner.timings.detail();
        let slot =
            nia_timing::time_detail(detail_timing, "query.slot_for", || self.slot_for(&key))?;
        let node_id = slot.node_id;
        nia_timing::time_detail(detail_timing, "query.record_dependency", || {
            record_dependency_on_current_stack(self.inner.session.inner.id, node_id)
        });
        let mut stale_value = None;
        loop {
            let mut state = slot.state.lock();
            match &*state {
                QueryState::Published { .. } => {
                    return Err(Self::internal_query_error(
                        &key,
                        "shared query reached published owned state",
                    ));
                }
                QueryState::Ready {
                    value, fingerprint, ..
                } => {
                    nia_timing::time_detail(detail_timing, "query.record_cache_hit", || {
                        slot.stats.record_cache_hit()
                    });
                    record_dependency_fingerprint_on_current_stack(
                        self.inner.session.inner.id,
                        node_id,
                        *fingerprint,
                    );
                    return Ok(Arc::clone(value));
                }
                QueryState::PotentiallyOutdated { .. } => {
                    self.check_not_recursive_node(node_id)?;
                    let previous = std::mem::replace(
                        &mut *state,
                        QueryState::Validating { invalidated: false },
                    );
                    let (value, fingerprint, dependency_fingerprints) = match previous {
                        QueryState::PotentiallyOutdated {
                            value,
                            fingerprint,
                            dependency_fingerprints,
                        } => (value, fingerprint, dependency_fingerprints),
                        unexpected => {
                            *state = unexpected;
                            slot.ready.notify_all();
                            return Err(Self::internal_query_error(
                                &key,
                                "query state changed while locked",
                            ));
                        }
                    };
                    drop(state);

                    let entry = QueryStackEntry {
                        session_id: self.inner.session.inner.id,
                        node_id,
                        identity: Arc::clone(&slot.identity),
                        dependencies: FastHashSet::default(),
                        dependency_fingerprints: Some(DependencyFingerprints::default()),
                    };
                    let mut guard = match self.enter_query(entry) {
                        Ok(guard) => guard,
                        Err(error) => {
                            let mut state = slot.state.lock();
                            *state = QueryState::PotentiallyOutdated {
                                value,
                                fingerprint,
                                dependency_fingerprints,
                            };
                            slot.ready.notify_all();
                            return Err(error);
                        }
                    };
                    slot.stats.record_validation();
                    let is_green = match self.dependencies_are_green(&dependency_fingerprints) {
                        Ok(is_green) => is_green,
                        Err(error) => {
                            guard.discard();
                            let mut state = slot.state.lock();
                            let was_invalidated = matches!(
                                &*state,
                                QueryState::Validating { invalidated: true }
                                    | QueryState::Computing { invalidated: true }
                            );
                            if was_invalidated {
                                *state = QueryState::Empty;
                                self.clear_dependencies_from(node_id);
                            } else {
                                *state = QueryState::PotentiallyOutdated {
                                    value,
                                    fingerprint,
                                    dependency_fingerprints,
                                };
                            }
                            slot.ready.notify_all();
                            return Err(error.with_query_context(query_frame::<C, K>(&key)));
                        }
                    };
                    guard.discard();

                    let mut state = slot.state.lock();
                    let was_invalidated = matches!(
                        &*state,
                        QueryState::Validating { invalidated: true }
                            | QueryState::Computing { invalidated: true }
                    );
                    if is_green && !was_invalidated {
                        *state = QueryState::Ready {
                            value: Arc::clone(&value),
                            fingerprint: Some(fingerprint),
                            dependency_fingerprints,
                        };
                        slot.stats.record_green_validation();
                        slot.stats.record_cache_hit();
                        slot.ready.notify_all();
                        record_dependency_fingerprint_on_current_stack(
                            self.inner.session.inner.id,
                            node_id,
                            Some(fingerprint),
                        );
                        return Ok(value);
                    }
                    stale_value = Some((value, fingerprint));
                    *state = QueryState::Empty;
                    slot.ready.notify_all();
                }
                QueryState::Computing { .. } | QueryState::Validating { .. } => {
                    self.check_not_recursive_node(node_id)?;
                    nia_timing::time_detail(detail_timing, "query.record_wait", || {
                        slot.stats.record_wait()
                    });
                    let _wait = self
                        .inner
                        .session
                        .begin_query_wait(node_id, query_frame::<C, K>(&key))?;
                    slot.ready.wait(&mut state);
                }
                QueryState::Consumed => {
                    return Err(Self::internal_query_error(
                        &key,
                        "shared query reached single-consumer state",
                    ));
                }
                QueryState::Empty => {
                    if K::PROVIDER == QueryProviderPolicy::ExternallyPublished {
                        return Err(QueryError::InvalidInput {
                            query: query_frame::<C, K>(&key),
                            message: "shared product has not been published by its producer".into(),
                        });
                    }
                    *state = QueryState::Computing { invalidated: false };
                    drop(state);

                    self.clear_dependencies_from(node_id);
                    let entry = QueryStackEntry {
                        session_id: self.inner.session.inner.id,
                        node_id,
                        identity: Arc::clone(&slot.identity),
                        dependencies: FastHashSet::default(),
                        dependency_fingerprints: (K::FINGERPRINT != QueryFingerprintPolicy::None)
                            .then(DependencyFingerprints::default),
                    };
                    let mut guard = self.enter_query(entry)?;
                    nia_timing::time_detail(detail_timing, "query.record_execution", || {
                        slot.stats.record_execution()
                    });
                    let value = match self.inner.session.inner.capture_unexpected_panic(|| {
                        nia_timing::time_detail(detail_timing, "query.provider", || {
                            key.execute_result(self)
                        })
                    }) {
                        Ok(Ok(value)) => value,
                        Ok(Err(error)) => {
                            let mut state = slot.state.lock();
                            *state = QueryState::Empty;
                            guard.discard();
                            self.clear_dependencies_from(node_id);
                            slot.ready.notify_all();
                            return Err(error.with_query_context(query_frame::<C, K>(&key)));
                        }
                        Err(ice) => {
                            let mut state = slot.state.lock();
                            *state = QueryState::Empty;
                            // Dependencies recorded during a failed execution are speculative:
                            // keeping them would make future invalidations report a query value
                            // that was never cached and can no longer be reused.
                            guard.discard();
                            self.clear_dependencies_from(node_id);
                            slot.ready.notify_all();
                            drop(state);
                            return Err(QueryError::Internal(ice)
                                .with_query_context(query_frame::<C, K>(&key)));
                        }
                    };

                    let fingerprint = match K::FINGERPRINT {
                        QueryFingerprintPolicy::None => {
                            if key.fingerprint(&value).is_some() {
                                Err(Self::internal_query_error(
                                    &key,
                                    "query returned a fingerprint without declaring a policy",
                                ))
                            } else {
                                Ok(None)
                            }
                        }
                        QueryFingerprintPolicy::StableValue => {
                            key.fingerprint(&value).map(Some).ok_or_else(|| {
                                Self::internal_query_error(
                                    &key,
                                    "stable value query did not produce a fingerprint",
                                )
                            })
                        }
                        QueryFingerprintPolicy::SemanticValue => {
                            if key.fingerprint(&value).is_some() {
                                Err(Self::internal_query_error(
                                    &key,
                                    "semantic value query must use values_equal, not fingerprint",
                                ))
                            } else {
                                Ok(Some(
                                    stale_value
                                        .take()
                                        .filter(|(old, _)| key.values_equal(old, &value))
                                        .map_or_else(
                                            || slot.next_semantic_fingerprint(K::name()),
                                            |(_, fingerprint)| fingerprint,
                                        ),
                                ))
                            }
                        }
                    };
                    let fingerprint = match fingerprint {
                        Ok(fingerprint) => fingerprint,
                        Err(error) => {
                            let mut state = slot.state.lock();
                            *state = QueryState::Empty;
                            guard.discard();
                            self.clear_dependencies_from(node_id);
                            slot.ready.notify_all();
                            return Err(error);
                        }
                    };
                    let cached = Arc::new(value);
                    let output = Arc::clone(&cached);
                    let mut state = slot.state.lock();
                    let was_invalidated =
                        matches!(&*state, QueryState::Computing { invalidated: true });
                    if was_invalidated {
                        *state = QueryState::Empty;
                        // The value was computed from an input that changed while this query was
                        // running. Return it to the caller that did the work, but drop the cache
                        // entry and its edges so the next request recomputes against fresh inputs.
                        guard.discard();
                        self.clear_dependencies_from(node_id);
                    } else {
                        let dependencies = guard.take_dependencies();
                        self.replace_dependencies_from(node_id, dependencies.nodes);
                        *state = QueryState::Ready {
                            value: cached,
                            fingerprint,
                            dependency_fingerprints: dependencies.fingerprints.unwrap_or_default(),
                        };
                    }
                    slot.ready.notify_all();
                    record_dependency_fingerprint_on_current_stack(
                        self.inner.session.inner.id,
                        node_id,
                        (!was_invalidated).then_some(fingerprint).flatten(),
                    );
                    return Ok(output);
                }
            }
        }
    }

    /// Evaluates shared queries in parallel and returns values in input order.
    pub fn get_many<K>(&self, keys: impl IntoIterator<Item = K>) -> QueryResult<Vec<Arc<K::Value>>>
    where
        C: Send + Sync + 'static,
        K: QueryKey<C>,
    {
        self.get_many_with(keys, Self::get::<K>)?
            .into_iter()
            .collect()
    }

    /// Evaluates owned queries in parallel and returns consumed values in input order.
    pub fn get_many_owned<K>(&self, keys: impl IntoIterator<Item = K>) -> QueryResult<Vec<K::Value>>
    where
        C: Send + Sync + 'static,
        K: QueryKey<C>,
    {
        self.get_many_with(keys, Self::get_owned::<K>)?
            .into_iter()
            .collect()
    }

    /// Consumes owned query results as they complete, exposing submission indices.
    ///
    /// The callback is always drained after it returns or panics so worker tasks
    /// finish and their dependency facts are merged into the parent stack.
    pub fn with_many_owned_completion<K, R>(
        &self,
        keys: impl IntoIterator<Item = K>,
        consume: impl FnOnce(&mut QueryCompletionStream<'_, '_, QueryResult<K::Value>>) -> R,
    ) -> QueryResult<R>
    where
        C: Send + Sync + 'static,
        K: QueryKey<C>,
    {
        self.with_many_completion_with(keys, Self::get_owned::<K>, consume)
    }

    fn with_many_completion_with<K, O, R>(
        &self,
        keys: impl IntoIterator<Item = K>,
        get: fn(&Self, K) -> O,
        consume: impl FnOnce(&mut QueryCompletionStream<'_, '_, O>) -> R,
    ) -> QueryResult<R>
    where
        C: Send + Sync + 'static,
        K: QueryKey<C>,
        O: Send + 'static,
    {
        let _activity = self.inner.session.enter_activity();
        let parent_stack = current_query_stack();
        let records_fingerprints = parent_stack
            .last()
            .is_some_and(|entry| entry.dependency_fingerprints.is_some());
        let tasks = keys.into_iter().map(|key| {
            let db = self.clone();
            let parent_stack = parent_stack.clone();
            move || {
                let _stack_guard = install_query_stack(parent_stack);
                let value = get(&db, key);
                (value, take_current_stack_dependencies())
            }
        });
        let (result, dependencies) = self
            .inner
            .session
            .with_task_completion_stream_inner(tasks, |tasks| {
                let mut stream = QueryCompletionStream {
                    tasks,
                    dependencies: RecordedDependencies {
                        nodes: FastHashSet::default(),
                        fingerprints: records_fingerprints.then(DependencyFingerprints::default),
                    },
                };
                let result = self
                    .inner
                    .session
                    .inner
                    .capture_unexpected_panic(|| consume(&mut stream));
                let drain = (|| -> nia_ice::IceResult<()> {
                    while stream.wait_next()?.is_some() {}
                    Ok(())
                })();
                let dependencies = stream.dependencies;
                match (result, drain) {
                    (Ok(value), Ok(())) => Ok((value, dependencies)),
                    (Err(ice), _) | (Ok(_), Err(ice)) => Err(ice),
                }
            })
            .map_err(QueryError::from)?;
        merge_dependencies_into_current_stack(dependencies);
        Ok(result)
    }

    pub(super) fn get_many_with<K, O>(
        &self,
        keys: impl IntoIterator<Item = K>,
        get: fn(&Self, K) -> O,
    ) -> QueryResult<Vec<O>>
    where
        C: Send + Sync + 'static,
        K: QueryKey<C>,
        O: Send + 'static,
    {
        let _activity = self.inner.session.enter_activity();
        let parent_stack = current_query_stack();
        let records_fingerprints = parent_stack
            .last()
            .is_some_and(|entry| entry.dependency_fingerprints.is_some());
        let tasks: Vec<_> = keys
            .into_iter()
            .map(|key| {
                let db = self.clone();
                let parent_stack = parent_stack.clone();
                move || {
                    let _stack_guard = install_query_stack(parent_stack);
                    let value = get(&db, key);
                    (value, take_current_stack_dependencies())
                }
            })
            .collect();
        let outcomes: Vec<(O, RecordedDependencies)> = self
            .inner
            .session
            .run_tasks_inner(tasks)
            .map_err(QueryError::from)?;
        let mut values = Vec::with_capacity(outcomes.len());
        let mut dependencies = RecordedDependencies {
            nodes: FastHashSet::default(),
            fingerprints: records_fingerprints.then(DependencyFingerprints::default),
        };
        for (value, task_dependencies) in outcomes {
            values.push(value);
            dependencies.nodes.extend(task_dependencies.nodes);
            if let (Some(dependencies), Some(task_dependencies)) = (
                dependencies.fingerprints.as_mut(),
                task_dependencies.fingerprints,
            ) {
                dependencies.extend(task_dependencies);
            }
        }
        merge_dependencies_into_current_stack(dependencies);
        Ok(values)
    }

    /// Returns persistent dependency edges and slot statistics for this database.
    pub fn query_trace(&self) -> QueryResult<QueryTrace> {
        let _activity = self.inner.session.enter_activity();
        let queries = {
            let slots = self.inner.slots.lock();
            Self::query_stats(&slots)
        };
        Ok(QueryTrace {
            dependencies: self
                .inner
                .session
                .inner
                .dependencies
                .lock()
                .dependencies(self.inner.id, &self.inner.session)?,
            queries,
        })
    }

    /// Invalidates `key` and every known dependent while retaining slot identity.
    pub fn invalidate<K>(&self, key: K) -> QueryResult<QueryInvalidation>
    where
        K: QueryKey<C>,
    {
        let _activity = self.inner.session.enter_activity();
        self.invalidate_during_retirement(key)
    }

    fn invalidate_during_retirement<K>(&self, key: K) -> QueryResult<QueryInvalidation>
    where
        K: QueryKey<C>,
    {
        let Some(root) = self.cached_slot(&key)?.map(|slot| slot.node_id) else {
            return Ok(QueryInvalidation {
                invalidated: vec![query_frame::<C, K>(&key)],
            });
        };
        self.invalidate_cached_root(root)
    }

    /// Validates a stable input fingerprint and invalidates it only when changed.
    ///
    /// This is the red/green fast path for [`QueryFingerprintPolicy::StableValue`].
    pub fn validate_input<K>(
        &self,
        key: K,
        current_value: &K::Value,
    ) -> QueryResult<QueryInvalidation>
    where
        K: QueryKey<C>,
    {
        let _activity = self.inner.session.enter_activity();
        if K::FINGERPRINT != QueryFingerprintPolicy::StableValue {
            return Err(Self::internal_query_error(
                &key,
                "query must declare a stable value fingerprint before input validation",
            ));
        }
        let current_fingerprint = key.fingerprint(current_value).ok_or_else(|| {
            Self::internal_query_error(&key, "stable value query did not produce a fingerprint")
        })?;
        let Some(slot) = self.cached_slot(&key)? else {
            return Ok(QueryInvalidation::default());
        };
        let is_green = {
            let mut state = slot.state.lock();
            match &*state {
                QueryState::Empty | QueryState::Consumed | QueryState::Published { .. } => {
                    return Ok(QueryInvalidation::default());
                }
                QueryState::Computing { .. } | QueryState::Validating { .. } => false,
                QueryState::PotentiallyOutdated { fingerprint, .. } => {
                    if *fingerprint == current_fingerprint {
                        let previous = std::mem::replace(&mut *state, QueryState::Empty);
                        let (value, fingerprint, dependency_fingerprints) = match previous {
                            QueryState::PotentiallyOutdated {
                                value,
                                fingerprint,
                                dependency_fingerprints,
                            } => (value, fingerprint, dependency_fingerprints),
                            unexpected => {
                                *state = unexpected;
                                return Err(Self::internal_query_error(
                                    &key,
                                    "query input state changed while locked",
                                ));
                            }
                        };
                        *state = QueryState::Ready {
                            value,
                            fingerprint: Some(fingerprint),
                            dependency_fingerprints,
                        };
                        true
                    } else {
                        false
                    }
                }
                QueryState::Ready { fingerprint, .. } => *fingerprint == Some(current_fingerprint),
            }
        };
        if is_green {
            Ok(QueryInvalidation::default())
        } else {
            self.invalidate_cached_root(slot.node_id)
        }
    }

    /// Removes a typed slot and its dependency identity after session quiescence.
    pub fn retire<K>(&self, key: &K) -> QueryResult<bool>
    where
        K: QueryKey<C>,
    {
        let _retirement = self.inner.session.enter_retirement();
        self.retire_during_retirement(key)
    }

    /// Runs several invalidation or retirement operations under one quiescent barrier.
    pub fn with_retirement<R>(
        &self,
        operation: impl FnOnce(&QueryRetirement<'_, C>) -> QueryResult<R>,
    ) -> QueryResult<R> {
        let _retirement = self.inner.session.enter_retirement();
        operation(&QueryRetirement { db: self })
    }

    fn retire_during_retirement<K>(&self, key: &K) -> QueryResult<bool>
    where
        K: QueryKey<C>,
    {
        if let Some(registry) = &self.inner.registry {
            registry.require_registered::<C, K>()?;
        }
        let caches = self.inner.caches.read();
        let Some(cache) = caches.get(&TypeId::of::<K>()) else {
            return Ok(false);
        };
        let cache = cache
            .as_any()
            .downcast_ref::<Mutex<FastHashMap<Arc<K>, Arc<QuerySlot<K::Value>>>>>()
            .ok_or_else(|| Self::internal_query_error(key, "query cache type mismatch"))?;
        let mut cache = cache.lock();
        let Some(slot) = cache.get(key).cloned() else {
            return Ok(false);
        };
        let node_id = slot.node_id;

        {
            let slots = self.inner.slots.lock();
            let Some(record) = slots.get(self.inner.id, node_id) else {
                return Err(Self::internal_query_error(
                    key,
                    "retired query slot is not registered",
                ));
            };
            if !Arc::ptr_eq(&record.slot, &(slot.clone() as Arc<dyn ErasedQuerySlot>)) {
                return Err(Self::internal_query_error(
                    key,
                    "retired typed cache and slot identity disagree",
                ));
            }
        }

        self.invalidate_cached_root(node_id)?;
        if cache.remove_entry(key).is_none() {
            return Err(Self::internal_query_error(
                key,
                "retired query cache entry disappeared",
            ));
        }
        if self
            .inner
            .slots
            .lock()
            .remove(self.inner.id, node_id)
            .is_none()
        {
            return Err(Self::internal_query_error(
                key,
                "retired query slot registration disappeared",
            ));
        }
        self.inner
            .session
            .inner
            .dependencies
            .lock()
            .remove_node(node_id);
        Ok(true)
    }

    /// Seals an owned current value, severs its sole predecessor edge, and retires that
    /// predecessor. The current query must have copied everything it needs from the immutable
    /// predecessor rather than retaining the predecessor value as part of its own payload.
    pub fn seal_and_retire_predecessor<K>(&self, current: &K, predecessor: &K) -> QueryResult<bool>
    where
        K: QueryKey<C>,
    {
        if K::FINGERPRINT != QueryFingerprintPolicy::None {
            return Err(Self::internal_query_error(
                current,
                "query predecessor retirement requires an owned, non-validating query value",
            ));
        }
        if let Some(registry) = &self.inner.registry {
            registry.require_registered::<C, K>()?;
        }
        let _retirement = self.inner.session.enter_retirement();
        let caches = self.inner.caches.read();
        let Some(cache) = caches.get(&TypeId::of::<K>()) else {
            return Ok(false);
        };
        let cache = cache
            .as_any()
            .downcast_ref::<Mutex<FastHashMap<Arc<K>, Arc<QuerySlot<K::Value>>>>>()
            .ok_or_else(|| Self::internal_query_error(current, "query cache type mismatch"))?;
        let mut cache = cache.lock();
        let Some(current_node) = cache.get(current).map(|slot| slot.node_id) else {
            return Ok(false);
        };
        let Some(predecessor_slot) = cache.get(predecessor).cloned() else {
            return Ok(false);
        };
        let predecessor_node = predecessor_slot.node_id;
        if current_node == predecessor_node {
            return Err(Self::internal_query_error(
                current,
                "query cannot retire itself as its predecessor",
            ));
        }
        let Some(current_slot) = cache.get(current) else {
            return Err(Self::internal_query_error(
                current,
                "current query cache entry disappeared",
            ));
        };
        if !matches!(&*current_slot.state.lock(), QueryState::Ready { .. }) {
            return Err(Self::internal_query_error(
                current,
                "current query must own a ready value before sealing its predecessor",
            ));
        }
        let mut dependencies = self.inner.session.inner.dependencies.lock();
        dependencies
            .require_only_predecessor(predecessor_node, current_node)
            .map_err(|error| error.with_query_context(query_frame::<C, K>(current)))?;

        {
            let slots = self.inner.slots.lock();
            let Some(record) = slots.get(self.inner.id, predecessor_node) else {
                return Err(Self::internal_query_error(
                    predecessor,
                    "retired predecessor slot is not registered",
                ));
            };
            if !Arc::ptr_eq(
                &record.slot,
                &(predecessor_slot.clone() as Arc<dyn ErasedQuerySlot>),
            ) {
                return Err(Self::internal_query_error(
                    predecessor,
                    "retired predecessor cache and slot identity disagree",
                ));
            }
        }

        if cache.remove_entry(predecessor).is_none() {
            return Err(Self::internal_query_error(
                predecessor,
                "retired predecessor cache entry disappeared",
            ));
        }
        if self
            .inner
            .slots
            .lock()
            .remove(self.inner.id, predecessor_node)
            .is_none()
        {
            return Err(Self::internal_query_error(
                predecessor,
                "retired predecessor slot registration disappeared",
            ));
        }
        dependencies.remove_node(predecessor_node);
        Ok(true)
    }

    fn invalidate_cached_root(&self, root: QueryNodeId) -> QueryResult<QueryInvalidation> {
        let invalidated = self.collect_invalidated_nodes(root);
        let mut cleared = Vec::new();
        for (index, node_id) in invalidated.iter().enumerate() {
            let slot = self.inner.session.slot(*node_id)?;
            // The changed root is definitely red and must be cleared. Dependents retain their
            // previous value and fingerprints as validation evidence; they are recomputed only
            // when an ensured dependency proves that evidence stale.
            let disposition = if index == 0 {
                slot.invalidate();
                QueryInvalidationDisposition::Cleared
            } else {
                slot.mark_potentially_outdated()
            };
            if disposition == QueryInvalidationDisposition::Cleared {
                cleared.push(*node_id);
            }
        }
        let mut frames = invalidated
            .iter()
            .map(|node_id| self.inner.session.frame(*node_id))
            .collect::<QueryResult<Vec<_>>>()?;
        // Traversal order has no semantic effect. Keep the changed root first and make the
        // diagnostic portion deterministic without formatting keys for every dependency edge.
        frames[1..].sort_by(|left, right| {
            (left.name, left.key.as_str(), left.description.as_str()).cmp(&(
                right.name,
                right.key.as_str(),
                right.description.as_str(),
            ))
        });

        let mut dependencies = self.inner.session.inner.dependencies.lock();
        for node_id in cleared {
            dependencies.remove_dependencies_from(node_id);
        }
        Ok(QueryInvalidation {
            invalidated: frames,
        })
    }

    pub(super) fn slot_for<K>(&self, key: &K) -> QueryResult<Arc<QuerySlot<K::Value>>>
    where
        K: QueryKey<C>,
    {
        let caches = self.inner.caches.read();
        let Some(cache) = caches.get(&TypeId::of::<K>()) else {
            drop(caches);
            if let Some(registry) = &self.inner.registry {
                registry.require_registered::<C, K>()?;
            }
            self.inner
                .caches
                .write()
                .entry(TypeId::of::<K>())
                .or_insert_with(|| {
                    Box::new(Mutex::new(
                        FastHashMap::<Arc<K>, Arc<QuerySlot<K::Value>>>::default(),
                    )) as Box<dyn ErasedQueryCache>
                });
            return self.slot_for(key);
        };
        let cache = cache
            .as_any()
            .downcast_ref::<Mutex<FastHashMap<Arc<K>, Arc<QuerySlot<K::Value>>>>>()
            .ok_or_else(|| Self::internal_query_error(key, "query cache type mismatch"))?;
        let mut cache = cache.lock();
        if let Some(slot) = cache.get(key) {
            return Ok(slot.clone());
        }
        let key = Arc::new(key.clone());
        let identity = Arc::new(query_slot_identity::<C, K>(key.as_ref()));
        let ensure_key = Arc::clone(&key);
        let ensure: Arc<QueryEnsure<C>> = Arc::new(move |db| match K::STORAGE {
            QueryStoragePolicy::CacheOwnedArc => db.get((*ensure_key).clone()).map(drop),
            QueryStoragePolicy::SingleConsumerOwned => {
                db.get_owned((*ensure_key).clone()).map(drop)
            }
        });
        let mut slots = self.inner.slots.lock();
        let node_id = slots.next_id(self.inner.id)?;
        let slot = Arc::new(QuerySlot {
            node_id,
            identity: Arc::clone(&identity),
            stats: QuerySlotStats::default(),
            fingerprint_revision: AtomicU64::new(0),
            state: Mutex::new(QueryState::Empty),
            ready: Condvar::new(),
        });
        slots.push(
            node_id,
            identity,
            slot.clone() as Arc<dyn ErasedQuerySlot>,
            ensure,
        )?;
        if cache.insert(Arc::clone(&key), slot.clone()).is_some() {
            slots.remove(self.inner.id, node_id);
            return Err(Self::internal_query_error(
                key.as_ref(),
                "query cache key was inserted concurrently",
            ));
        }
        Ok(slot)
    }

    pub(super) fn cached_slot<K>(&self, key: &K) -> QueryResult<Option<Arc<QuerySlot<K::Value>>>>
    where
        K: QueryKey<C>,
    {
        let caches = self.inner.caches.read();
        let Some(cache) = caches.get(&TypeId::of::<K>()) else {
            return Ok(None);
        };
        let cache = cache
            .as_any()
            .downcast_ref::<Mutex<FastHashMap<Arc<K>, Arc<QuerySlot<K::Value>>>>>()
            .ok_or_else(|| Self::internal_query_error(key, "query cache type mismatch"))?;
        Ok(cache.lock().get(key).cloned())
    }

    fn enter_query(&self, entry: QueryStackEntry) -> QueryResult<QueryStackGuard> {
        self.check_not_recursive_node(entry.node_id)?;
        QUERY_STACK.with(|stack| {
            stack.borrow_mut().push(entry);
        });
        Ok(QueryStackGuard { active: true })
    }

    fn check_not_recursive_node(&self, node_id: QueryNodeId) -> QueryResult<()> {
        QUERY_STACK.with(|stack| {
            let stack = stack.borrow();
            if let Some(position) = stack.iter().position(|entry| entry.node_id == node_id) {
                let mut cycle = stack[position..]
                    .iter()
                    .map(|entry| entry.identity.frame())
                    .collect::<Vec<_>>();
                cycle.push(self.frame(node_id)?);
                return Err(QueryError::Cycle { cycle });
            }
            Ok(())
        })
    }

    fn query_stats(slots: &QuerySlotTable<C>) -> Vec<QueryTraceQuery> {
        let mut queries = slots
            .entries
            .values()
            .map(|record| QueryTraceQuery {
                frame: record.identity.frame(),
                stats: record.slot.stats(),
            })
            .collect::<Vec<_>>();
        queries.sort_by(|lhs, rhs| {
            (lhs.frame.name, lhs.frame.key.as_str()).cmp(&(rhs.frame.name, rhs.frame.key.as_str()))
        });
        queries
    }

    fn collect_invalidated_nodes(&self, root: QueryNodeId) -> Vec<QueryNodeId> {
        let dependencies = self.inner.session.inner.dependencies.lock();
        dependencies.collect_dependents(root)
    }

    fn dependencies_are_green(&self, expected: &DependencyFingerprints) -> QueryResult<bool> {
        let mut dependencies = expected.iter().collect::<Vec<_>>();
        dependencies.sort_unstable_by_key(|(node_id, _)| (node_id.db_id.0, node_id.index));
        for (node_id, expected_fingerprint) in dependencies {
            let Some(expected_fingerprint) = expected_fingerprint else {
                return Ok(false);
            };
            // Ensuring first recursively validates the dependency. Its stored fingerprint is only
            // meaningful after that state transition, so comparing the pre-ensure value would let
            // an outdated dependency incorrectly keep this query green.
            self.ensure_node(*node_id)?;
            if self.node_fingerprint(*node_id)? != Some(*expected_fingerprint) {
                return Ok(false);
            }
        }
        Ok(true)
    }

    fn ensure_node(&self, node_id: QueryNodeId) -> QueryResult<()> {
        self.inner.session.ensure(node_id)
    }

    fn node_fingerprint(&self, node_id: QueryNodeId) -> QueryResult<Option<QueryFingerprint>> {
        Ok(self.inner.session.slot(node_id)?.fingerprint())
    }

    fn clear_dependencies_from(&self, from: QueryNodeId) {
        self.inner
            .session
            .inner
            .dependencies
            .lock()
            .remove_dependencies_from(from);
    }

    fn replace_dependencies_from(&self, from: QueryNodeId, targets: FastHashSet<QueryNodeId>) {
        self.inner
            .session
            .inner
            .dependencies
            .lock()
            .replace_dependencies_from(from, targets);
    }

    fn frame(&self, node_id: QueryNodeId) -> QueryResult<QueryFrame> {
        self.inner.session.frame(node_id)
    }
}

impl<C> QueryRetirement<'_, C> {
    /// Invalidates a key while the enclosing retirement transaction is quiescent.
    pub fn invalidate<K>(&self, key: K) -> QueryResult<QueryInvalidation>
    where
        K: QueryKey<C>,
    {
        self.db.invalidate_during_retirement(key)
    }

    /// Retires a key while the enclosing retirement transaction is quiescent.
    pub fn retire<K>(&self, key: &K) -> QueryResult<bool>
    where
        K: QueryKey<C>,
    {
        self.db.retire_during_retirement(key)
    }
}
