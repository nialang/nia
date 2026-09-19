// SPDX-License-Identifier: GPL-3.0-or-later
//! Query-session lifetime, task entry points, and retirement exclusion.
//!
//! Activity is counted once per outermost thread-local entry. Retirement closes
//! admission and waits for that count to reach zero before cache state changes.

use super::*;

impl QuerySession {
    /// Creates a session with process-budgeted executor parallelism.
    pub fn new() -> nia_ice::IceResult<Self> {
        let parallelism = default_query_parallelism();
        Self::with_execution_budget_result(parallelism, process_query_execution_budget(parallelism))
    }

    #[cfg(test)]
    pub(super) fn new_for_test() -> Self {
        Self::new().unwrap_or_else(|ice| panic!("failed to create query session: {ice}"))
    }

    #[cfg(test)]
    pub(super) fn with_parallelism(parallelism: usize) -> Self {
        Self::with_execution_budget(
            parallelism,
            Arc::new(QueryExecutionBudget::owned(parallelism)),
        )
    }

    #[cfg(test)]
    pub(super) fn with_execution_budget(
        parallelism: usize,
        execution_budget: Arc<QueryExecutionBudget>,
    ) -> Self {
        Self::from_execution_budget(parallelism, execution_budget)
            .unwrap_or_else(|ice| panic!("failed to create query session: {ice}"))
    }

    fn with_execution_budget_result(
        parallelism: usize,
        execution_budget: nia_ice::IceResult<Arc<QueryExecutionBudget>>,
    ) -> nia_ice::IceResult<Self> {
        Self::from_execution_budget(parallelism, execution_budget?)
    }

    pub(super) fn from_execution_budget(
        parallelism: usize,
        execution_budget: Arc<QueryExecutionBudget>,
    ) -> nia_ice::IceResult<Self> {
        let parallelism = NonZeroUsize::new(parallelism)
            .ok_or_else(|| nia_ice::Ice::new("query executor parallelism must be non-zero"))?;
        let id = QuerySessionId::fresh()?;
        Ok(Self {
            inner: Arc::new(QuerySessionInner {
                id,
                executor: QueryExecutor::new(id, parallelism, execution_budget),
                databases: Mutex::new(FastHashMap::default()),
                dependencies: Mutex::new(QueryDependencyGraph::default()),
                activity: Mutex::new(QueryActivityState::default()),
                activity_ready: Condvar::new(),
                internal_failure: Mutex::new(None),
            }),
        })
    }

    /// Tests whether two handles share the same dependency and cache domain.
    pub fn ptr_eq(&self, other: &Self) -> bool {
        Arc::ptr_eq(&self.inner, &other.inner)
    }

    /// Returns this session executor's configured worker lane count.
    pub fn executor_parallelism(&self) -> usize {
        self.inner.executor.shared.parallelism
    }

    /// Runs independent tasks concurrently and restores submission order in the result.
    pub fn run_tasks<T, O>(&self, tasks: impl IntoIterator<Item = T>) -> nia_ice::IceResult<Vec<O>>
    where
        T: FnOnce() -> nia_ice::IceResult<O> + Send + 'static,
        O: Send + 'static,
    {
        let _activity = self.enter_activity();
        self.run_tasks_inner(tasks)
    }

    /// Runs tasks with an additional caller-provided lane bound.
    pub fn run_tasks_bounded<T, O>(
        &self,
        tasks: impl IntoIterator<Item = T>,
        max_parallelism: usize,
    ) -> nia_ice::IceResult<Vec<O>>
    where
        T: FnOnce() -> nia_ice::IceResult<O> + Send + 'static,
        O: Send + 'static,
    {
        if max_parallelism == 0 {
            return Err(nia_ice::Ice::new(
                "bounded task parallelism must be non-zero",
            ));
        }
        let tasks = tasks.into_iter().collect::<Vec<_>>();
        let lane_count = tasks
            .len()
            .min(max_parallelism)
            .min(self.executor_parallelism());
        if tasks.len() <= lane_count {
            return self.run_tasks(tasks);
        }

        let mut lanes = (0..lane_count)
            .map(|_| Vec::new())
            .collect::<Vec<Vec<(usize, T)>>>();
        for (index, task) in tasks.into_iter().enumerate() {
            lanes[index % lane_count].push((index, task));
        }
        let mut outcomes = self
            .run_tasks(lanes.into_iter().map(|lane| {
                move || {
                    lane.into_iter()
                        .map(|(index, task)| task().map(|output| (index, output)))
                        .collect::<nia_ice::IceResult<Vec<_>>>()
                }
            }))?
            .into_iter()
            .flatten()
            .collect::<Vec<_>>();
        outcomes.sort_unstable_by_key(|(index, _)| *index);
        Ok(outcomes.into_iter().map(|(_, output)| output).collect())
    }

    /// Creates a backpressured pool whose `finish` result is submission ordered.
    pub fn task_pool<O>(&self, max_parallelism: usize) -> nia_ice::IceResult<QueryTaskPool<'_, O>>
    where
        O: Send + 'static,
    {
        if max_parallelism == 0 {
            return Err(nia_ice::Ice::new(
                "bounded task parallelism must be non-zero",
            ));
        }
        self.inner.ensure_healthy()?;
        Ok(QueryTaskPool {
            session: self,
            _activity: self.enter_activity(),
            capacity: max_parallelism.min(self.executor_parallelism()),
            next_position: 0,
            pending: VecDeque::new(),
            completed: Vec::new(),
            failure: None,
        })
    }

    pub(super) fn run_tasks_inner<T, O>(
        &self,
        tasks: impl IntoIterator<Item = T>,
    ) -> nia_ice::IceResult<Vec<O>>
    where
        T: FnOnce() -> nia_ice::IceResult<O> + Send + 'static,
        O: Send + 'static,
    {
        self.inner.ensure_healthy()?;
        let tasks = tasks.into_iter().collect::<Vec<_>>();
        if tasks.len() <= 1 {
            return tasks
                .into_iter()
                .map(|task| {
                    self.inner.ensure_healthy()?;
                    task().map_err(|ice| self.inner.record_failure(ice))
                })
                .collect();
        }
        let batch = Arc::new(QueryBatch::new(tasks.len()));
        let batch_id = Arc::as_ptr(&batch) as usize;
        let executor = &self.inner.executor;
        let executor_shared = Arc::clone(&executor.shared);
        let tasks = tasks
            .into_iter()
            .enumerate()
            .map(|(index, task)| {
                let batch = Arc::clone(&batch);
                let executor_shared = Arc::clone(&executor_shared);
                let session = Arc::clone(&self.inner);
                QueryTask {
                    batch: batch_id,
                    settle: Box::new(move |failure| {
                        let outcome = match failure {
                            Some(ice) => Err(session.record_failure(ice)),
                            None => session
                                .ensure_healthy()
                                .and_then(|()| task())
                                .map_err(|ice| session.record_failure(ice)),
                        };
                        if let Err(ice) = batch.complete(index, outcome) {
                            session.record_failure(ice);
                        }
                        executor_shared.notify_waiters();
                    }),
                }
            })
            .collect();
        executor
            .submit_all(tasks)
            .map_err(|ice| self.inner.record_failure(ice))?;
        while !batch.is_complete() {
            if !executor.try_run_one(batch_id)? {
                executor.wait_for_batch_progress(&batch);
            }
        }
        batch.finish()
    }

    pub(super) fn with_task_completion_stream_inner<T, O, R>(
        &self,
        tasks: impl IntoIterator<Item = T>,
        consume: impl FnOnce(&mut TaskCompletionStream<'_, O>) -> nia_ice::IceResult<R>,
    ) -> nia_ice::IceResult<R>
    where
        T: FnOnce() -> nia_ice::IceResult<O> + Send + 'static,
        O: Send + 'static,
    {
        let tasks = tasks.into_iter().collect::<Vec<_>>();
        let batch = Arc::new(QueryBatch::new(tasks.len()));
        let batch_id = Arc::as_ptr(&batch) as usize;
        let executor = &self.inner.executor;
        let executor_shared = Arc::clone(&executor.shared);
        let tasks = tasks
            .into_iter()
            .enumerate()
            .map(|(index, task)| {
                let batch = Arc::clone(&batch);
                let executor_shared = Arc::clone(&executor_shared);
                let session = Arc::clone(&self.inner);
                QueryTask {
                    batch: batch_id,
                    settle: Box::new(move |failure| {
                        let outcome = match failure {
                            Some(ice) => Err(session.record_failure(ice)),
                            None => session
                                .ensure_healthy()
                                .and_then(|()| task())
                                .map_err(|ice| session.record_failure(ice)),
                        };
                        if let Err(ice) = batch.complete(index, outcome) {
                            session.record_failure(ice);
                        }
                        executor_shared.notify_waiters();
                    }),
                }
            })
            .collect();
        executor
            .submit_all(tasks)
            .map_err(|ice| self.inner.record_failure(ice))?;
        let mut stream = TaskCompletionStream {
            executor,
            batch,
            batch_id,
            pending: VecDeque::new(),
            failure: None,
        };
        consume(&mut stream)
    }

    pub(super) fn enter_activity(&self) -> QueryActivityGuard<'_> {
        let identity = Arc::as_ptr(&self.inner) as usize;
        let nested = query_activity_is_active(identity);
        // Nested queries on the same thread participate in the outer activity lease. Counting
        // every frame would let retirement wait on frames that cannot finish until it releases
        // admission, while counting only the outer edge gives retirement a quiescence barrier.
        if !nested {
            let mut state = self.inner.activity.lock();
            while state.retiring {
                self.inner.activity_ready.wait(&mut state);
            }
            state.active += 1;
        }
        enter_query_activity(identity);
        QueryActivityGuard {
            session: &self.inner,
        }
    }

    pub(super) fn enter_retirement(&self) -> nia_ice::IceResult<QueryRetirementGuard<'_>> {
        let identity = Arc::as_ptr(&self.inner) as usize;
        if query_activity_is_active(identity) {
            return Err(nia_ice::Ice::new(
                "query cache retirement cannot run inside an active query",
            ));
        }
        let mut state = self.inner.activity.lock();
        while state.retiring {
            self.inner.activity_ready.wait(&mut state);
        }
        // Set `retiring` before waiting for active work so no new outer activity can enter while
        // the current generation drains. The guard reopens admission on every return path.
        state.retiring = true;
        self.inner.activity_ready.notify_all();
        while state.active > 0 {
            self.inner.activity_ready.wait(&mut state);
        }
        drop(state);
        Ok(QueryRetirementGuard {
            session: &self.inner,
        })
    }

    pub(super) fn register<C>(&self, db: &QueryDb<C>) -> nia_ice::IceResult<()>
    where
        C: Send + Sync + 'static,
    {
        let registration: Arc<dyn ErasedQueryDatabase> = Arc::new(QueryDbRegistration {
            inner: Arc::downgrade(&db.inner),
        });
        let mut databases = self.inner.databases.lock();
        if databases.contains_key(&db.inner.id) {
            return Err(nia_ice::Ice::new("query database registered twice"));
        }
        databases.insert(db.inner.id, registration);
        Ok(())
    }

    pub(super) fn database(&self, db_id: QueryDbId) -> QueryResult<Arc<dyn ErasedQueryDatabase>> {
        self.inner
            .databases
            .lock()
            .get(&db_id)
            .cloned()
            .ok_or_else(|| QueryError::internal("query node references an unknown database"))
    }

    pub(super) fn frame(&self, node_id: QueryNodeId) -> QueryResult<QueryFrame> {
        self.database(node_id.db_id)?
            .frame(node_id)
            .ok_or_else(|| QueryError::internal("query node id references no registered slot"))
    }

    pub(super) fn slot(&self, node_id: QueryNodeId) -> QueryResult<Arc<dyn ErasedQuerySlot>> {
        self.database(node_id.db_id)?
            .slot(node_id)
            .ok_or_else(|| QueryError::internal("query node id references no registered slot"))
    }

    pub(super) fn ensure(&self, node_id: QueryNodeId) -> QueryResult<()> {
        self.database(node_id.db_id)?.ensure(node_id)
    }

    pub(super) fn begin_query_wait(
        &self,
        to: QueryNodeId,
        to_frame: QueryFrame,
    ) -> QueryResult<Option<QueryWaitGuard>> {
        let Some((from, from_identity)) = current_query_entry() else {
            return Ok(None);
        };
        let from_frame = from_identity.frame();
        let cycle = query_wait_graph()
            .lock()
            .begin(from, from_frame, to, to_frame)?;
        if let Some(cycle) = cycle {
            return Err(QueryError::Cycle { cycle });
        }
        Ok(Some(QueryWaitGuard {
            session: Arc::clone(&self.inner),
            from,
            to,
        }))
    }
}

impl QuerySessionInner {
    pub(super) fn ensure_healthy(&self) -> nia_ice::IceResult<()> {
        let failure = self.internal_failure.lock();
        match failure.as_ref() {
            Some(ice) => Err(ice
                .clone()
                .with_context("query session is no longer reusable")),
            None => Ok(()),
        }
    }

    pub(super) fn record_failure(&self, ice: nia_ice::Ice) -> nia_ice::Ice {
        let mut failure = self.internal_failure.lock();
        if failure.is_none() {
            *failure = Some(ice.clone());
        }
        ice
    }
}

impl Drop for QueryWaitGuard {
    fn drop(&mut self) {
        if let Err(QueryError::Internal(ice)) = query_wait_graph().lock().end(self.from, self.to) {
            self.session.record_failure(ice);
        }
    }
}

impl Drop for QueryActivityGuard<'_> {
    fn drop(&mut self) {
        let identity = self.session as *const QuerySessionInner as usize;
        let Ok(outermost) = leave_query_activity(identity) else {
            self.session.record_failure(nia_ice::Ice::new(
                "query activity guard dropped without a matching activity entry",
            ));
            return;
        };
        if !outermost {
            return;
        }
        let mut state = self.session.activity.lock();
        let Some(active) = state.active.checked_sub(1) else {
            self.session
                .record_failure(nia_ice::Ice::new("query activity count underflow"));
            return;
        };
        state.active = active;
        drop(state);
        self.session.activity_ready.notify_all();
    }
}

impl Drop for QueryRetirementGuard<'_> {
    fn drop(&mut self) {
        let mut state = self.session.activity.lock();
        if !state.retiring {
            self.session
                .record_failure(nia_ice::Ice::new("query retirement guard released twice"));
            return;
        }
        state.retiring = false;
        drop(state);
        self.session.activity_ready.notify_all();
    }
}
