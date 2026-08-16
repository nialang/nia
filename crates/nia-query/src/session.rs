// SPDX-License-Identifier: GPL-3.0-or-later
//! Query-session lifetime, task entry points, and retirement exclusion.
//!
//! Activity is counted once per outermost thread-local entry. Retirement closes
//! admission and waits for that count to reach zero before cache state changes.

use super::*;

impl Default for QuerySession {
    fn default() -> Self {
        Self::new()
    }
}

impl QuerySession {
    pub fn new() -> Self {
        let parallelism = default_query_parallelism();
        Self::with_execution_budget(parallelism, process_query_execution_budget(parallelism))
    }

    #[cfg(test)]
    pub(super) fn with_parallelism(parallelism: usize) -> Self {
        Self::with_execution_budget(
            parallelism,
            Arc::new(QueryExecutionBudget::owned(parallelism)),
        )
    }

    pub(super) fn with_execution_budget(
        parallelism: usize,
        execution_budget: Arc<QueryExecutionBudget>,
    ) -> Self {
        let id = QuerySessionId::fresh();
        Self {
            inner: Arc::new(QuerySessionInner {
                id,
                executor: QueryExecutor::new(id, parallelism, execution_budget),
                databases: Mutex::new(FastHashMap::default()),
                dependencies: Mutex::new(QueryDependencyGraph::default()),
                activity: Mutex::new(QueryActivityState::default()),
                activity_ready: Condvar::new(),
            }),
        }
    }

    pub fn ptr_eq(&self, other: &Self) -> bool {
        Arc::ptr_eq(&self.inner, &other.inner)
    }

    pub fn executor_parallelism(&self) -> usize {
        self.inner.executor.shared.parallelism
    }

    pub fn run_tasks<T, O>(&self, tasks: impl IntoIterator<Item = T>) -> Vec<O>
    where
        T: FnOnce() -> O + Send + 'static,
        O: Send + 'static,
    {
        let _activity = self.enter_activity();
        self.run_tasks_inner(tasks)
    }

    pub fn run_tasks_bounded<T, O>(
        &self,
        tasks: impl IntoIterator<Item = T>,
        max_parallelism: usize,
    ) -> Vec<O>
    where
        T: FnOnce() -> O + Send + 'static,
        O: Send + 'static,
    {
        assert!(
            max_parallelism > 0,
            "bounded task parallelism must be non-zero"
        );
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
                        .map(|(index, task)| (index, task()))
                        .collect::<Vec<_>>()
                }
            }))
            .into_iter()
            .flatten()
            .collect::<Vec<_>>();
        outcomes.sort_unstable_by_key(|(index, _)| *index);
        outcomes.into_iter().map(|(_, output)| output).collect()
    }

    pub fn task_pool<O>(&self, max_parallelism: usize) -> QueryTaskPool<'_, O>
    where
        O: Send + 'static,
    {
        assert!(
            max_parallelism > 0,
            "bounded task parallelism must be non-zero"
        );
        QueryTaskPool {
            session: self,
            _activity: self.enter_activity(),
            capacity: max_parallelism.min(self.executor_parallelism()),
            next_position: 0,
            pending: VecDeque::new(),
            completed: Vec::new(),
        }
    }

    pub(super) fn run_tasks_inner<T, O>(&self, tasks: impl IntoIterator<Item = T>) -> Vec<O>
    where
        T: FnOnce() -> O + Send + 'static,
        O: Send + 'static,
    {
        let tasks = tasks.into_iter().collect::<Vec<_>>();
        if tasks.len() <= 1 {
            return tasks.into_iter().map(|task| task()).collect();
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
                QueryTask {
                    batch: batch_id,
                    run: Box::new(move || {
                        batch.complete(index, catch_unwind(AssertUnwindSafe(task)));
                        executor_shared.notify_waiters();
                    }),
                }
            })
            .collect();
        executor.submit_all(tasks);
        while !batch.is_complete() {
            if !executor.try_run_one(batch_id) {
                executor.wait_for_batch_progress(&batch);
            }
        }
        batch.finish()
    }

    pub(super) fn with_task_completion_stream_inner<T, O, R>(
        &self,
        tasks: impl IntoIterator<Item = T>,
        consume: impl FnOnce(&mut TaskCompletionStream<'_, O>) -> R,
    ) -> R
    where
        T: FnOnce() -> O + Send + 'static,
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
                QueryTask {
                    batch: batch_id,
                    run: Box::new(move || {
                        batch.complete(index, catch_unwind(AssertUnwindSafe(task)));
                        executor_shared.notify_waiters();
                    }),
                }
            })
            .collect();
        executor.submit_all(tasks);
        let mut stream = TaskCompletionStream {
            executor,
            batch,
            batch_id,
            pending: VecDeque::new(),
            panic: None,
        };
        let result = catch_unwind(AssertUnwindSafe(|| consume(&mut stream)));
        let drain = catch_unwind(AssertUnwindSafe(|| stream.drain()));
        match result {
            Ok(value) => {
                if let Err(payload) = drain {
                    resume_unwind(payload);
                }
                value
            }
            Err(payload) => resume_unwind(payload),
        }
    }

    pub(super) fn enter_activity(&self) -> QueryActivityGuard<'_> {
        let identity = Arc::as_ptr(&self.inner) as usize;
        let nested = query_activity_is_active(identity);
        // Nested queries on the same thread participate in the outer activity lease. Counting
        // every frame would let retirement wait on frames that cannot finish until it releases
        // admission, while counting only the outer edge gives retirement a quiescence barrier.
        if !nested {
            let mut state = self
                .inner
                .activity
                .lock()
                .expect("query activity lock poisoned");
            while state.retiring {
                state = self
                    .inner
                    .activity_ready
                    .wait(state)
                    .expect("query activity lock poisoned while waiting");
            }
            state.active += 1;
        }
        enter_query_activity(identity);
        QueryActivityGuard {
            session: &self.inner,
        }
    }

    pub(super) fn enter_retirement(&self) -> QueryRetirementGuard<'_> {
        let identity = Arc::as_ptr(&self.inner) as usize;
        assert!(
            !query_activity_is_active(identity),
            "query cache retirement cannot run inside an active query"
        );
        let mut state = self
            .inner
            .activity
            .lock()
            .expect("query activity lock poisoned");
        while state.retiring {
            state = self
                .inner
                .activity_ready
                .wait(state)
                .expect("query activity lock poisoned while waiting");
        }
        // Set `retiring` before waiting for active work so no new outer activity can enter while
        // the current generation drains. The guard reopens admission even if retirement panics.
        state.retiring = true;
        self.inner.activity_ready.notify_all();
        while state.active > 0 {
            state = self
                .inner
                .activity_ready
                .wait(state)
                .expect("query activity lock poisoned while waiting for quiescence");
        }
        drop(state);
        QueryRetirementGuard {
            session: &self.inner,
        }
    }

    pub(super) fn register<C>(&self, db: &QueryDb<C>)
    where
        C: Send + Sync + 'static,
    {
        let registration: Arc<dyn ErasedQueryDatabase> = Arc::new(QueryDbRegistration {
            inner: Arc::downgrade(&db.inner),
        });
        let previous = self
            .inner
            .databases
            .lock()
            .expect("query session database lock poisoned")
            .insert(db.inner.id, registration);
        assert!(previous.is_none(), "query database registered twice");
    }

    pub(super) fn database(&self, db_id: QueryDbId) -> Arc<dyn ErasedQueryDatabase> {
        self.inner
            .databases
            .lock()
            .expect("query session database lock poisoned")
            .get(&db_id)
            .cloned()
            .expect("query node references an unknown database")
    }

    pub(super) fn frame(&self, node_id: QueryNodeId) -> QueryFrame {
        self.database(node_id.db_id)
            .frame(node_id)
            .expect("query node id must reference a registered slot")
    }

    pub(super) fn slot(&self, node_id: QueryNodeId) -> Arc<dyn ErasedQuerySlot> {
        self.database(node_id.db_id)
            .slot(node_id)
            .expect("query node id must reference a registered slot")
    }

    pub(super) fn ensure(&self, node_id: QueryNodeId) -> QueryResult<()> {
        self.database(node_id.db_id).ensure(node_id)
    }

    pub(super) fn begin_query_wait(
        &self,
        to: QueryNodeId,
        to_frame: QueryFrame,
    ) -> QueryResult<Option<QueryWaitGuard>> {
        let Some((from, from_frame)) = current_query_entry() else {
            return Ok(None);
        };
        let cycle = query_wait_graph()
            .lock()
            .expect("query wait-for graph lock poisoned")
            .begin(from, from_frame, to, to_frame);
        if let Some(cycle) = cycle {
            return Err(QueryError::Cycle { cycle });
        }
        Ok(Some(QueryWaitGuard { from, to }))
    }
}

impl Drop for QueryWaitGuard {
    fn drop(&mut self) {
        query_wait_graph()
            .lock()
            .expect("query wait-for graph lock poisoned")
            .end(self.from, self.to);
    }
}

impl Drop for QueryActivityGuard<'_> {
    fn drop(&mut self) {
        let identity = self.session as *const QuerySessionInner as usize;
        if !leave_query_activity(identity) {
            return;
        }
        let mut state = self
            .session
            .activity
            .lock()
            .expect("query activity lock poisoned");
        state.active = state
            .active
            .checked_sub(1)
            .expect("query activity count underflow");
        drop(state);
        self.session.activity_ready.notify_all();
    }
}

impl Drop for QueryRetirementGuard<'_> {
    fn drop(&mut self) {
        let mut state = self
            .session
            .activity
            .lock()
            .expect("query activity lock poisoned");
        assert!(state.retiring, "query retirement guard released twice");
        state.retiring = false;
        drop(state);
        self.session.activity_ready.notify_all();
    }
}
