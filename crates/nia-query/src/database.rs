// SPDX-License-Identifier: GPL-3.0-or-later
//! Typed query caches and their incremental state machine.
//!
//! Shared queries use red/green validation and stable fingerprints. Owned
//! queries transfer a payload exactly once and never participate in validation.
//! Retirement mutates cache identity only after the session is quiescent.

use super::*;

impl<C> QueryDb<C> {
    pub fn new(context: C) -> Self
    where
        C: Send + Sync + 'static,
    {
        Self::new_with_timings(context, nia_timing::TimingMode::Off)
    }

    pub fn new_with_timings(context: C, timings: nia_timing::TimingMode) -> Self
    where
        C: Send + Sync + 'static,
    {
        Self::new_with_timings_in_session(context, timings, QuerySession::new())
    }

    pub fn new_with_timings_in_session(
        context: C,
        timings: nia_timing::TimingMode,
        session: QuerySession,
    ) -> Self
    where
        C: Send + Sync + 'static,
    {
        Self::new_inner(context, timings, None, session)
    }

    pub fn new_registered(context: C, registry: QueryRegistry) -> Self
    where
        C: Send + Sync + 'static,
    {
        Self::new_registered_with_timings(context, nia_timing::TimingMode::Off, registry)
    }

    pub fn new_registered_in_session(
        context: C,
        registry: QueryRegistry,
        session: QuerySession,
    ) -> Self
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

    pub fn new_registered_with_timings(
        context: C,
        timings: nia_timing::TimingMode,
        registry: QueryRegistry,
    ) -> Self
    where
        C: Send + Sync + 'static,
    {
        Self::new_registered_with_timings_in_session(
            context,
            timings,
            registry,
            QuerySession::new(),
        )
    }

    pub fn new_registered_with_timings_in_session(
        context: C,
        timings: nia_timing::TimingMode,
        registry: QueryRegistry,
        session: QuerySession,
    ) -> Self
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
    ) -> Self
    where
        C: Send + Sync + 'static,
    {
        let db = Self {
            inner: Arc::new(QueryDbInner {
                id: QueryDbId::fresh(),
                session: session.clone(),
                context,
                timings,
                registry,
                caches: Mutex::new(FastHashMap::default()),
                slots: Mutex::new(QuerySlotTable::default()),
            }),
        };
        session.register(&db);
        db
    }

    pub fn context(&self) -> &C {
        &self.inner.context
    }

    pub fn session(&self) -> QuerySession {
        self.inner.session.clone()
    }

    pub fn registered_queries(&self) -> Vec<QueryDescriptor> {
        self.inner
            .registry
            .as_ref()
            .map(QueryRegistry::descriptors)
            .unwrap_or_default()
    }

    pub fn invalid_input<K>(&self, key: &K, message: impl Into<String>) -> QueryError
    where
        K: QueryKey<C>,
    {
        QueryError::InvalidInput {
            query: query_frame::<C, K>(key),
            message: message.into(),
        }
    }

    pub fn get<K>(&self, key: K) -> QueryResult<Arc<K::Value>>
    where
        K: QueryKey<C>,
    {
        self.try_get_cached(key)
    }

    pub fn get_owned<K>(&self, key: K) -> QueryResult<K::Value>
    where
        K: QueryKey<C>,
    {
        assert_eq!(
            K::STORAGE,
            QueryStoragePolicy::SingleConsumerOwned,
            "query `{}` does not declare single-consumer owned storage",
            K::name()
        );
        assert_eq!(
            K::FINGERPRINT,
            QueryFingerprintPolicy::None,
            "single-consumer query `{}` cannot retain a value fingerprint",
            K::name()
        );
        let _activity = self.inner.session.enter_activity();
        let detail_timing = self.inner.timings.detail();
        let slot = nia_timing::time_detail(detail_timing, "query.slot_for", || self.slot_for(&key));
        let node_id = slot.node_id;
        nia_timing::time_detail(detail_timing, "query.record_dependency", || {
            record_dependency_on_current_stack(self.inner.session.inner.id, node_id)
        });
        loop {
            let mut state = slot.state.lock().expect("query cache lock poisoned");
            match &*state {
                QueryState::Published { .. } => {
                    let QueryState::Published { value } =
                        std::mem::replace(&mut *state, QueryState::Consumed)
                    else {
                        unreachable!("published query state changed while locked");
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
                        dependencies: FastHashSet::default(),
                        dependency_fingerprints: None,
                    };
                    let mut guard = self.enter_query(entry)?;
                    nia_timing::time_detail(detail_timing, "query.record_execution", || {
                        slot.stats.record_execution()
                    });
                    let value = match catch_unwind(AssertUnwindSafe(|| {
                        nia_timing::time_detail(detail_timing, "query.provider", || {
                            key.execute_result(self)
                        })
                    })) {
                        Ok(Ok(value)) => value,
                        Ok(Err(error)) => {
                            let mut state = slot.state.lock().expect("query cache lock poisoned");
                            *state = QueryState::Empty;
                            guard.discard();
                            self.clear_dependencies_from(node_id);
                            slot.ready.notify_all();
                            return Err(error);
                        }
                        Err(payload) => {
                            let mut state = slot.state.lock().expect("query cache lock poisoned");
                            *state = QueryState::Empty;
                            guard.discard();
                            self.clear_dependencies_from(node_id);
                            slot.ready.notify_all();
                            drop(state);
                            resume_unwind(payload)
                        }
                    };

                    let mut state = slot.state.lock().expect("query cache lock poisoned");
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
                    drop(
                        slot.ready
                            .wait(state)
                            .expect("query cache lock poisoned while waiting"),
                    );
                }
                QueryState::Ready { .. } | QueryState::PotentiallyOutdated { .. } => {
                    panic!(
                        "Nia ICE: single-consumer query `{}` reached shared cache state",
                        K::name()
                    );
                }
            }
        }
    }

    /// Publishes an already-owned payload to one externally-published query slot.
    ///
    /// The published slot depends on `predecessor`, so invalidating the producer
    /// drops an unconsumed payload and invalidates an already-consumed consumer.
    pub fn publish_owned<K, P>(&self, key: K, value: K::Value, predecessor: &P)
    where
        K: QueryKey<C>,
        P: QueryKey<C>,
    {
        assert_eq!(
            K::STORAGE,
            QueryStoragePolicy::SingleConsumerOwned,
            "published query `{}` must use single-consumer owned storage",
            K::name()
        );
        assert_eq!(
            K::PROVIDER,
            QueryProviderPolicy::ExternallyPublished,
            "query `{}` does not declare an external producer",
            K::name()
        );
        assert_eq!(
            K::FINGERPRINT,
            QueryFingerprintPolicy::None,
            "published query `{}` cannot retain a value fingerprint",
            K::name()
        );
        let _activity = self.inner.session.enter_activity();
        let slot = self.slot_for(&key);
        let predecessor_slot = self.slot_for(predecessor);
        {
            let mut state = slot.state.lock().expect("query cache lock poisoned");
            assert!(
                matches!(&*state, QueryState::Empty | QueryState::Consumed),
                "published query `{}` already has a live payload or consumer",
                K::name()
            );
            *state = QueryState::Published { value };
            slot.ready.notify_all();
        }
        self.replace_dependencies_from(
            slot.node_id,
            FastHashSet::from_iter([predecessor_slot.node_id]),
        );
    }

    fn try_get_cached<K>(&self, key: K) -> QueryResult<Arc<K::Value>>
    where
        K: QueryKey<C>,
    {
        assert_eq!(
            K::STORAGE,
            QueryStoragePolicy::CacheOwnedArc,
            "single-consumer query `{}` must be requested with get_owned",
            K::name()
        );
        let _activity = self.inner.session.enter_activity();
        let detail_timing = self.inner.timings.detail();
        let slot = nia_timing::time_detail(detail_timing, "query.slot_for", || self.slot_for(&key));
        let node_id = slot.node_id;
        nia_timing::time_detail(detail_timing, "query.record_dependency", || {
            record_dependency_on_current_stack(self.inner.session.inner.id, node_id)
        });
        let mut stale_value = None;
        loop {
            let mut state = slot.state.lock().expect("query cache lock poisoned");
            match &*state {
                QueryState::Published { .. } => {
                    panic!(
                        "Nia ICE: shared query `{}` reached published owned state",
                        K::name()
                    )
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
                    let QueryState::PotentiallyOutdated {
                        value,
                        fingerprint,
                        dependency_fingerprints,
                    } = previous
                    else {
                        unreachable!("query state changed while locked")
                    };
                    drop(state);

                    let entry = QueryStackEntry {
                        session_id: self.inner.session.inner.id,
                        node_id,
                        dependencies: FastHashSet::default(),
                        dependency_fingerprints: Some(DependencyFingerprints::default()),
                    };
                    let mut guard = match self.enter_query(entry) {
                        Ok(guard) => guard,
                        Err(error) => {
                            let mut state = slot.state.lock().expect("query cache lock poisoned");
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
                    let is_green = self.dependencies_are_green(&dependency_fingerprints);
                    guard.discard();

                    let mut state = slot.state.lock().expect("query cache lock poisoned");
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
                    drop(
                        slot.ready
                            .wait(state)
                            .expect("query cache lock poisoned while waiting"),
                    );
                }
                QueryState::Consumed => {
                    panic!(
                        "Nia ICE: shared query `{}` reached single-consumer state",
                        K::name()
                    );
                }
                QueryState::Empty => {
                    *state = QueryState::Computing { invalidated: false };
                    drop(state);

                    self.clear_dependencies_from(node_id);
                    let entry = QueryStackEntry {
                        session_id: self.inner.session.inner.id,
                        node_id,
                        dependencies: FastHashSet::default(),
                        dependency_fingerprints: (K::FINGERPRINT != QueryFingerprintPolicy::None)
                            .then(DependencyFingerprints::default),
                    };
                    let mut guard = self.enter_query(entry)?;
                    nia_timing::time_detail(detail_timing, "query.record_execution", || {
                        slot.stats.record_execution()
                    });
                    let value = match std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                        nia_timing::time_detail(detail_timing, "query.provider", || {
                            key.execute_result(self)
                        })
                    })) {
                        Ok(Ok(value)) => value,
                        Ok(Err(error)) => {
                            let mut state = slot.state.lock().expect("query cache lock poisoned");
                            *state = QueryState::Empty;
                            guard.discard();
                            self.clear_dependencies_from(node_id);
                            slot.ready.notify_all();
                            return Err(error);
                        }
                        Err(payload) => {
                            let mut state = slot.state.lock().expect("query cache lock poisoned");
                            *state = QueryState::Empty;
                            // Dependencies recorded during a failed execution are speculative:
                            // keeping them would make future invalidations report a query value
                            // that was never cached and can no longer be reused.
                            guard.discard();
                            self.clear_dependencies_from(node_id);
                            slot.ready.notify_all();
                            drop(state);
                            std::panic::resume_unwind(payload)
                        }
                    };

                    let fingerprint = match K::FINGERPRINT {
                        QueryFingerprintPolicy::None => {
                            assert!(
                                key.fingerprint(&value).is_none(),
                                "query `{}` returned a fingerprint without declaring a policy",
                                K::name()
                            );
                            None
                        }
                        QueryFingerprintPolicy::StableValue => Some(
                            key.fingerprint(&value)
                                .expect("stable value query must produce a fingerprint"),
                        ),
                        QueryFingerprintPolicy::SemanticValue => {
                            assert!(
                                key.fingerprint(&value).is_none(),
                                "semantic value query `{}` must use values_equal, not fingerprint",
                                K::name()
                            );
                            Some(
                                stale_value
                                    .take()
                                    .filter(|(old, _)| key.values_equal(old, &value))
                                    .map_or_else(
                                        || slot.next_semantic_fingerprint(K::name()),
                                        |(_, fingerprint)| fingerprint,
                                    ),
                            )
                        }
                    };
                    let cached = Arc::new(value);
                    let output = Arc::clone(&cached);
                    let mut state = slot.state.lock().expect("query cache lock poisoned");
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

    pub fn get_many<K>(&self, keys: impl IntoIterator<Item = K>) -> QueryResult<Vec<Arc<K::Value>>>
    where
        C: Send + Sync + 'static,
        K: QueryKey<C>,
    {
        self.get_many_with(keys, Self::get::<K>)
            .into_iter()
            .collect()
    }

    pub fn get_many_owned<K>(&self, keys: impl IntoIterator<Item = K>) -> QueryResult<Vec<K::Value>>
    where
        C: Send + Sync + 'static,
        K: QueryKey<C>,
    {
        self.get_many_with(keys, Self::get_owned::<K>)
            .into_iter()
            .collect()
    }

    pub fn with_many_owned_completion<K, R>(
        &self,
        keys: impl IntoIterator<Item = K>,
        consume: impl FnOnce(&mut QueryCompletionStream<'_, '_, QueryResult<K::Value>>) -> R,
    ) -> R
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
    ) -> R
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
        let (result, dependencies) =
            self.inner
                .session
                .with_task_completion_stream_inner(tasks, |tasks| {
                    let mut stream = QueryCompletionStream {
                        tasks,
                        dependencies: RecordedDependencies {
                            nodes: FastHashSet::default(),
                            fingerprints: records_fingerprints
                                .then(DependencyFingerprints::default),
                        },
                    };
                    let result = catch_unwind(AssertUnwindSafe(|| consume(&mut stream)));
                    let drain =
                        catch_unwind(AssertUnwindSafe(|| while stream.wait_next().is_some() {}));
                    let dependencies = stream.dependencies;
                    match result {
                        Ok(value) => {
                            if let Err(payload) = drain {
                                resume_unwind(payload);
                            }
                            (value, dependencies)
                        }
                        Err(payload) => resume_unwind(payload),
                    }
                });
        merge_dependencies_into_current_stack(dependencies);
        result
    }

    pub(super) fn get_many_with<K, O>(
        &self,
        keys: impl IntoIterator<Item = K>,
        get: fn(&Self, K) -> O,
    ) -> Vec<O>
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
        let outcomes: Vec<(O, RecordedDependencies)> = self.inner.session.run_tasks_inner(tasks);
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
        values
    }

    pub fn query_trace(&self) -> QueryTrace {
        let _activity = self.inner.session.enter_activity();
        let queries = {
            let slots = self
                .inner
                .slots
                .lock()
                .expect("query cache slot lock poisoned");
            Self::query_stats(self.inner.id, &slots)
        };
        QueryTrace {
            dependencies: self
                .inner
                .session
                .inner
                .dependencies
                .lock()
                .expect("query dependency lock poisoned")
                .dependencies(self.inner.id, &self.inner.session),
            queries,
        }
    }

    pub fn invalidate<K>(&self, key: K) -> QueryInvalidation
    where
        K: QueryKey<C>,
    {
        let _activity = self.inner.session.enter_activity();
        self.invalidate_during_retirement(key)
    }

    fn invalidate_during_retirement<K>(&self, key: K) -> QueryInvalidation
    where
        K: QueryKey<C>,
    {
        let Some(root) = self.cached_slot(&key).map(|slot| slot.node_id) else {
            return QueryInvalidation {
                invalidated: vec![query_frame::<C, K>(&key)],
            };
        };
        self.invalidate_cached_root(root)
    }

    pub fn validate_input<K>(&self, key: K, current_value: &K::Value) -> QueryInvalidation
    where
        K: QueryKey<C>,
    {
        let _activity = self.inner.session.enter_activity();
        assert_eq!(
            K::FINGERPRINT,
            QueryFingerprintPolicy::StableValue,
            "query `{}` must declare a stable value fingerprint before input validation",
            K::name()
        );
        let current_fingerprint = key
            .fingerprint(current_value)
            .expect("stable value query must produce a fingerprint");
        let Some(slot) = self.cached_slot(&key) else {
            return QueryInvalidation::default();
        };
        let is_green = {
            let state = slot.state.lock().expect("query cache lock poisoned");
            match &*state {
                QueryState::Empty | QueryState::Consumed | QueryState::Published { .. } => {
                    return QueryInvalidation::default();
                }
                QueryState::Computing { .. }
                | QueryState::Validating { .. }
                | QueryState::PotentiallyOutdated { .. } => false,
                QueryState::Ready { fingerprint, .. } => *fingerprint == Some(current_fingerprint),
            }
        };
        if is_green {
            QueryInvalidation::default()
        } else {
            self.invalidate_cached_root(slot.node_id)
        }
    }

    pub fn retire<K>(&self, key: &K) -> bool
    where
        K: QueryKey<C>,
    {
        let _retirement = self.inner.session.enter_retirement();
        self.retire_during_retirement(key)
    }

    pub fn retirement_transaction<R>(
        &self,
        operation: impl FnOnce(&QueryRetirement<'_, C>) -> R,
    ) -> R {
        let _retirement = self.inner.session.enter_retirement();
        operation(&QueryRetirement { db: self })
    }

    fn retire_during_retirement<K>(&self, key: &K) -> bool
    where
        K: QueryKey<C>,
    {
        if let Some(registry) = &self.inner.registry {
            registry.assert_registered::<C, K>();
        }
        let mut caches = self.inner.caches.lock().expect("query cache lock poisoned");
        let Some(cache) = caches.get_mut(&TypeId::of::<K>()) else {
            return false;
        };
        let cache = cache
            .downcast_ref::<Mutex<FastHashMap<Arc<K>, Arc<QuerySlot<K::Value>>>>>()
            .expect("query cache type mismatch");
        let mut cache = cache.lock().expect("query cache lock poisoned");
        let Some(node_id) = cache.get(key).map(|slot| slot.node_id) else {
            return false;
        };

        self.invalidate_cached_root(node_id);
        let (_, slot) = cache
            .remove_entry(key)
            .expect("retired query cache entry must remain present");
        let record = self
            .inner
            .slots
            .lock()
            .expect("query cache slot lock poisoned")
            .remove(self.inner.id, node_id)
            .expect("retired query slot must remain registered");
        assert!(
            Arc::ptr_eq(&record.slot, &(slot as Arc<dyn ErasedQuerySlot>)),
            "retired typed cache and slot identity disagree"
        );
        self.inner
            .session
            .inner
            .dependencies
            .lock()
            .expect("query dependency lock poisoned")
            .remove_node(node_id);
        true
    }

    /// Seals an owned current value, severs its sole predecessor edge, and retires that
    /// predecessor. The current query must have copied everything it needs from the immutable
    /// predecessor rather than retaining the predecessor value as part of its own payload.
    pub fn seal_and_retire_predecessor<K>(&self, current: &K, predecessor: &K) -> bool
    where
        K: QueryKey<C>,
    {
        assert_eq!(
            K::FINGERPRINT,
            QueryFingerprintPolicy::None,
            "query predecessor retirement requires an owned, non-validating query value"
        );
        if let Some(registry) = &self.inner.registry {
            registry.assert_registered::<C, K>();
        }
        let _retirement = self.inner.session.enter_retirement();
        let mut caches = self.inner.caches.lock().expect("query cache lock poisoned");
        let Some(cache) = caches.get_mut(&TypeId::of::<K>()) else {
            return false;
        };
        let cache = cache
            .downcast_ref::<Mutex<FastHashMap<Arc<K>, Arc<QuerySlot<K::Value>>>>>()
            .expect("query cache type mismatch");
        let mut cache = cache.lock().expect("query cache lock poisoned");
        let Some(current_node) = cache.get(current).map(|slot| slot.node_id) else {
            return false;
        };
        let Some(predecessor_node) = cache.get(predecessor).map(|slot| slot.node_id) else {
            return false;
        };
        assert_ne!(
            current_node, predecessor_node,
            "query cannot retire itself as its predecessor"
        );
        assert!(
            matches!(
                &*cache
                    .get(current)
                    .expect("current query slot must remain cached")
                    .state
                    .lock()
                    .expect("query cache lock poisoned"),
                QueryState::Ready { .. }
            ),
            "current query must own a ready value before sealing its predecessor"
        );
        let mut dependencies = self
            .inner
            .session
            .inner
            .dependencies
            .lock()
            .expect("query dependency lock poisoned");
        dependencies.assert_only_predecessor(predecessor_node, current_node);

        let (_, predecessor_slot) = cache
            .remove_entry(predecessor)
            .expect("retired predecessor cache entry must remain present");
        let record = self
            .inner
            .slots
            .lock()
            .expect("query cache slot lock poisoned")
            .remove(self.inner.id, predecessor_node)
            .expect("retired predecessor slot must remain registered");
        assert!(
            Arc::ptr_eq(
                &record.slot,
                &(predecessor_slot as Arc<dyn ErasedQuerySlot>)
            ),
            "retired predecessor cache and slot identity disagree"
        );
        dependencies.remove_node(predecessor_node);
        true
    }

    fn invalidate_cached_root(&self, root: QueryNodeId) -> QueryInvalidation {
        let invalidated = self.collect_invalidated_nodes(root);
        let mut cleared = Vec::new();
        for (index, node_id) in invalidated.iter().enumerate() {
            let slot = self.inner.session.slot(*node_id);
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
        let frames = invalidated
            .iter()
            .map(|node_id| self.inner.session.frame(*node_id))
            .collect::<Vec<_>>();

        let mut dependencies = self
            .inner
            .session
            .inner
            .dependencies
            .lock()
            .expect("query dependency lock poisoned");
        for node_id in cleared {
            dependencies.remove_dependencies_from(node_id);
        }
        QueryInvalidation {
            invalidated: frames,
        }
    }

    pub(super) fn slot_for<K>(&self, key: &K) -> Arc<QuerySlot<K::Value>>
    where
        K: QueryKey<C>,
    {
        if let Some(registry) = &self.inner.registry {
            registry.assert_registered::<C, K>();
        }
        let mut caches = self.inner.caches.lock().expect("query cache lock poisoned");
        let cache = caches
            .entry(TypeId::of::<K>())
            .or_insert_with(|| {
                Box::new(Mutex::new(
                    FastHashMap::<Arc<K>, Arc<QuerySlot<K::Value>>>::default(),
                ))
            })
            .downcast_ref::<Mutex<FastHashMap<Arc<K>, Arc<QuerySlot<K::Value>>>>>()
            .expect("query cache type mismatch");
        let mut cache = cache.lock().expect("query cache lock poisoned");
        if let Some(slot) = cache.get(key) {
            return slot.clone();
        }
        let key = Arc::new(key.clone());
        let identity = query_slot_identity::<C, K>(Arc::clone(&key));
        let mut slots = self
            .inner
            .slots
            .lock()
            .expect("query cache slot lock poisoned");
        let node_id = slots.next_id(self.inner.id);
        let slot = Arc::new(QuerySlot {
            node_id,
            stats: QuerySlotStats::default(),
            fingerprint_revision: AtomicU64::new(0),
            state: Mutex::new(QueryState::Empty),
            ready: Condvar::new(),
        });
        cache.insert(key, slot.clone());
        slots.push(
            node_id,
            identity,
            slot.clone() as Arc<dyn ErasedQuerySlot>,
            ensure_query_from_erased::<C, K>,
        );
        slot
    }

    pub(super) fn cached_slot<K>(&self, key: &K) -> Option<Arc<QuerySlot<K::Value>>>
    where
        K: QueryKey<C>,
    {
        let caches = self.inner.caches.lock().expect("query cache lock poisoned");
        let cache = caches
            .get(&TypeId::of::<K>())?
            .downcast_ref::<Mutex<FastHashMap<Arc<K>, Arc<QuerySlot<K::Value>>>>>()
            .expect("query cache type mismatch");
        cache
            .lock()
            .expect("query cache lock poisoned")
            .get(key)
            .cloned()
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
                    .map(|entry| self.frame(entry.node_id))
                    .collect::<Vec<_>>();
                cycle.push(self.frame(node_id));
                return Err(QueryError::Cycle { cycle });
            }
            Ok(())
        })
    }

    fn query_stats(db_id: QueryDbId, slots: &QuerySlotTable<C>) -> Vec<QueryTraceQuery> {
        let mut queries = slots
            .entries
            .iter()
            .map(|(index, record)| QueryTraceQuery {
                frame: slots.frame(
                    db_id,
                    QueryNodeId {
                        db_id,
                        index: *index,
                    },
                ),
                stats: record.slot.stats(),
            })
            .collect::<Vec<_>>();
        queries.sort_by(|lhs, rhs| {
            (lhs.frame.name, lhs.frame.key.as_str()).cmp(&(rhs.frame.name, rhs.frame.key.as_str()))
        });
        queries
    }

    fn collect_invalidated_nodes(&self, root: QueryNodeId) -> Vec<QueryNodeId> {
        let dependencies = self
            .inner
            .session
            .inner
            .dependencies
            .lock()
            .expect("query dependency lock poisoned");
        dependencies.collect_dependents(&self.inner.session, root)
    }

    fn dependencies_are_green(&self, expected: &DependencyFingerprints) -> bool {
        let mut dependencies = expected.iter().collect::<Vec<_>>();
        dependencies.sort_unstable_by_key(|(node_id, _)| (node_id.db_id.0, node_id.index));
        for (node_id, expected_fingerprint) in dependencies {
            let Some(expected_fingerprint) = expected_fingerprint else {
                return false;
            };
            // Ensuring first recursively validates the dependency. Its stored fingerprint is only
            // meaningful after that state transition, so comparing the pre-ensure value would let
            // an outdated dependency incorrectly keep this query green.
            if self.ensure_node(*node_id).is_err()
                || self.node_fingerprint(*node_id) != Some(*expected_fingerprint)
            {
                return false;
            }
        }
        true
    }

    fn ensure_node(&self, node_id: QueryNodeId) -> QueryResult<()> {
        self.inner.session.ensure(node_id)
    }

    fn node_fingerprint(&self, node_id: QueryNodeId) -> Option<QueryFingerprint> {
        self.inner.session.slot(node_id).fingerprint()
    }

    fn clear_dependencies_from(&self, from: QueryNodeId) {
        self.inner
            .session
            .inner
            .dependencies
            .lock()
            .expect("query dependency lock poisoned")
            .remove_dependencies_from(from);
    }

    fn replace_dependencies_from(&self, from: QueryNodeId, targets: FastHashSet<QueryNodeId>) {
        self.inner
            .session
            .inner
            .dependencies
            .lock()
            .expect("query dependency lock poisoned")
            .replace_dependencies_from(from, targets);
    }

    fn frame(&self, node_id: QueryNodeId) -> QueryFrame {
        self.inner.session.frame(node_id)
    }
}

impl<C> QueryRetirement<'_, C> {
    pub fn invalidate<K>(&self, key: K) -> QueryInvalidation
    where
        K: QueryKey<C>,
    {
        self.db.invalidate_during_retirement(key)
    }

    pub fn retire<K>(&self, key: &K) -> bool
    where
        K: QueryKey<C>,
    {
        self.db.retire_during_retirement(key)
    }
}
