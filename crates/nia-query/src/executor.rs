// SPDX-License-Identifier: GPL-3.0-or-later
//! Shared worker executor, nested execution budget, and ordered task batches.
//!
//! Workers may help their awaited batch while blocked, but a process-wide
//! execution budget bounds active work across nested query sessions and
//! independent databases. Batch results are restored to submission order
//! before exposure.

use super::*;

impl QueryExecutionBudget {
    pub(super) fn from_environment(parallelism: usize) -> Self {
        // SAFETY: the process launcher owns the jobserver environment contract. We validate that
        // inherited Unix descriptors are pipes, and the process-wide OnceLock below ensures Nia
        // opens them only once instead of creating competing clients for the same raw descriptors.
        let inherited = unsafe { jobserver::Client::from_env_ext(true) };
        let client = match inherited.client {
            Ok(client) => client,
            Err(error)
                if matches!(
                    error.kind(),
                    jobserver::FromEnvErrorKind::NoEnvVar
                        | jobserver::FromEnvErrorKind::NoJobserver
                ) =>
            {
                jobserver::Client::new(parallelism.saturating_sub(1))
                    .unwrap_or_else(|error| panic!("failed to create query jobserver: {error}"))
            }
            Err(error) => panic!("failed to inherit query jobserver: {error}"),
        };
        Self::from_client(client)
    }

    #[cfg(test)]
    pub(super) fn owned(parallelism: usize) -> Self {
        let client = jobserver::Client::new(parallelism.saturating_sub(1))
            .unwrap_or_else(|error| panic!("failed to create query jobserver: {error}"));
        Self::from_client(client)
    }

    fn from_client(client: jobserver::Client) -> Self {
        let shared = Arc::new(QueryExecutionBudgetShared {
            state: Mutex::new(QueryExecutionBudgetState {
                implicit_available: true,
                waiting: 0,
                pending_requests: 0,
                active: 0,
                deliveries: VecDeque::new(),
            }),
            ready: Condvar::new(),
            peak_active: AtomicUsize::new(0),
        });
        let callback_shared = Arc::clone(&shared);
        let helper = client
            .into_helper_thread(move |delivery| {
                let delivery = delivery.map_err(|error| error.to_string());
                let mut state = callback_shared
                    .state
                    .lock()
                    .expect("query execution budget lock poisoned");
                state.pending_requests = state
                    .pending_requests
                    .checked_sub(1)
                    .expect("query execution budget request count underflow");
                if state.waiting > 0 {
                    state.deliveries.push_back(delivery);
                }
                drop(state);
                callback_shared.ready.notify_all();
            })
            .unwrap_or_else(|error| panic!("failed to start query jobserver helper: {error}"));
        Self {
            shared,
            helper: Mutex::new(helper),
        }
    }

    fn acquire(&self) -> QueryExecutionPermit {
        let mut state = self
            .shared
            .state
            .lock()
            .expect("query execution budget lock poisoned");
        state.waiting += 1;
        loop {
            // A jobserver contributes one implicit slot plus its explicit tokens. Deliveries are
            // assigned before the implicit slot so an already-issued request cannot be stranded
            // while later waiters repeatedly take the process-local slot.
            if let Some(delivery) = state.deliveries.pop_front() {
                state.waiting -= 1;
                let token = match delivery {
                    Ok(token) => token,
                    Err(error) => {
                        drop(state);
                        panic!("failed to acquire query jobserver token: {error}");
                    }
                };
                state.active += 1;
                self.shared.record_active(state.active);
                drop(state);
                return QueryExecutionPermit {
                    shared: Arc::clone(&self.shared),
                    implicit: false,
                    token: Some(token),
                };
            }
            if state.implicit_available {
                state.implicit_available = false;
                state.waiting -= 1;
                state.active += 1;
                self.shared.record_active(state.active);
                return QueryExecutionPermit {
                    shared: Arc::clone(&self.shared),
                    implicit: true,
                    token: None,
                };
            }
            let represented_waiters = state.pending_requests + state.deliveries.len();
            let requests = state.waiting.saturating_sub(represented_waiters);
            state.pending_requests += requests;
            if requests > 0 {
                let helper = self
                    .helper
                    .lock()
                    .expect("query jobserver helper lock poisoned");
                for _ in 0..requests {
                    helper.request_token();
                }
            }
            state = self
                .shared
                .ready
                .wait(state)
                .expect("query execution budget lock poisoned while waiting");
        }
    }

    fn identity(&self) -> usize {
        Arc::as_ptr(&self.shared) as usize
    }

    #[cfg(test)]
    pub(super) fn peak_active(&self) -> usize {
        self.shared.peak_active.load(Ordering::Relaxed)
    }
}

impl QueryExecutionBudgetShared {
    fn record_active(&self, active: usize) {
        self.peak_active.fetch_max(active, Ordering::Relaxed);
    }
}

impl Drop for QueryExecutionPermit {
    fn drop(&mut self) {
        let mut state = self
            .shared
            .state
            .lock()
            .expect("query execution budget lock poisoned");
        state.active = state
            .active
            .checked_sub(1)
            .expect("query execution budget active count underflow");
        if self.implicit {
            assert!(
                !state.implicit_available,
                "query execution budget returned the implicit token twice"
            );
            state.implicit_available = true;
        }
        drop(state);
        drop(self.token.take());
        self.shared.ready.notify_all();
    }
}

impl QueryExecutor {
    pub(super) fn new(
        session_id: QuerySessionId,
        parallelism: usize,
        execution_budget: Arc<QueryExecutionBudget>,
    ) -> Self {
        assert!(
            parallelism > 0,
            "query executor parallelism must be non-zero"
        );
        Self {
            session_id,
            shared: Arc::new(QueryExecutorShared {
                parallelism,
                state: Mutex::new(QueryExecutorState::default()),
                ready: Condvar::new(),
                peak_active: AtomicUsize::new(0),
            }),
            execution_budget,
            workers: Mutex::new(QueryExecutorWorkers {
                handles: Vec::with_capacity(parallelism.saturating_sub(1)),
            }),
        }
    }

    fn ensure_workers(&self, work_items: usize) {
        let mut workers = self
            .workers
            .lock()
            .expect("query executor worker lock poisoned");
        let worker_target = self
            .shared
            .parallelism
            .saturating_sub(1)
            .min(work_items.saturating_sub(1));
        for worker_index in workers.handles.len()..worker_target {
            let shared = Arc::clone(&self.shared);
            let execution_budget = Arc::clone(&self.execution_budget);
            let handle = std::thread::Builder::new()
                .name(format!("nia-query-{}-{worker_index}", self.session_id.0))
                .spawn(move || shared.worker_loop(execution_budget))
                .unwrap_or_else(|error| panic!("failed to start query executor worker: {error}"));
            workers.handles.push(handle);
        }
    }

    pub(super) fn submit_all(&self, tasks: Vec<QueryTask>) {
        if tasks.is_empty() {
            return;
        }
        self.ensure_workers(tasks.len());
        let mut state = self
            .shared
            .state
            .lock()
            .expect("query executor state lock poisoned");
        assert!(
            !state.shutdown,
            "query executor accepted work after shutdown"
        );
        state.queue.extend(tasks);
        drop(state);
        self.shared.ready.notify_all();
    }

    fn submit_all_priority(&self, tasks: Vec<QueryTask>) {
        if tasks.is_empty() {
            return;
        }
        self.ensure_workers(self.shared.parallelism);
        let mut state = self
            .shared
            .state
            .lock()
            .expect("query executor state lock poisoned");
        assert!(
            !state.shutdown,
            "query executor accepted work after shutdown"
        );
        for task in tasks.into_iter().rev() {
            state.queue.push_front(task);
        }
        drop(state);
        self.shared.ready.notify_all();
    }

    pub(super) fn try_run_one(&self, batch: usize) -> bool {
        let nested = query_executor_is_active(self.identity());
        let can_run = {
            let state = self
                .shared
                .state
                .lock()
                .expect("query executor state lock poisoned");
            state.queue.iter().any(|task| task.batch == batch)
                && (nested || state.active < self.shared.parallelism)
        };
        if !can_run {
            return false;
        }
        // A query waiting inside this executor helps its own batch. Nested work inherits the
        // caller's executor activity and process-wide permit: reacquiring either capacity here
        // could deadlock when every admitted worker is waiting on descendants.
        let execution_permit = if query_execution_budget_is_active(self.execution_budget.identity())
        {
            None
        } else {
            Some(self.execution_budget.acquire())
        };
        let task = {
            let mut state = self
                .shared
                .state
                .lock()
                .expect("query executor state lock poisoned");
            let position = state.queue.iter().rposition(|task| task.batch == batch);
            if nested {
                position.map(|position| {
                    (
                        state
                            .queue
                            .remove(position)
                            .expect("query batch task position must remain valid"),
                        false,
                    )
                })
            } else if state.active < self.shared.parallelism {
                position.map(|position| {
                    state.active += 1;
                    self.shared.record_active(state.active);
                    (
                        state
                            .queue
                            .remove(position)
                            .expect("query batch task position must remain valid"),
                        true,
                    )
                })
            } else {
                None
            }
        };
        let Some((task, counts_activity)) = task else {
            drop(execution_permit);
            return false;
        };
        self.shared.run_task(
            task.run,
            counts_activity,
            execution_permit,
            self.execution_budget.identity(),
        );
        true
    }

    fn identity(&self) -> usize {
        Arc::as_ptr(&self.shared) as usize
    }

    pub(super) fn wait_for_batch_progress<V>(&self, batch: &QueryBatch<V>) {
        let state = self
            .shared
            .state
            .lock()
            .expect("query executor state lock poisoned");
        if !batch.is_complete() {
            drop(
                self.shared
                    .ready
                    .wait(state)
                    .expect("query executor state lock poisoned while waiting"),
            );
        }
    }

    #[cfg(test)]
    pub(super) fn peak_active(&self) -> usize {
        self.shared.peak_active.load(Ordering::Relaxed)
    }
}

impl Drop for QueryExecutor {
    fn drop(&mut self) {
        // Closing admission precedes joining so workers drain every accepted task and no task is
        // left without a thread that can publish its batch completion.
        {
            let mut state = self
                .shared
                .state
                .lock()
                .expect("query executor state lock poisoned");
            state.shutdown = true;
        }
        self.shared.ready.notify_all();
        let current_thread = std::thread::current().id();
        let handles = std::mem::take(
            &mut self
                .workers
                .lock()
                .expect("query executor worker lock poisoned")
                .handles,
        );
        for handle in handles {
            if handle.thread().id() == current_thread {
                drop(handle);
            } else {
                let _ = handle.join();
            }
        }
    }
}

impl QueryExecutorShared {
    fn worker_loop(self: Arc<Self>, execution_budget: Arc<QueryExecutionBudget>) {
        loop {
            {
                let mut state = self
                    .state
                    .lock()
                    .expect("query executor state lock poisoned");
                loop {
                    if state.shutdown && state.queue.is_empty() {
                        return;
                    }
                    if state.active < self.parallelism && !state.queue.is_empty() {
                        break;
                    }
                    state = self
                        .ready
                        .wait(state)
                        .expect("query executor state lock poisoned while waiting");
                }
            }
            let execution_permit = execution_budget.acquire();
            let task = {
                let mut state = self
                    .state
                    .lock()
                    .expect("query executor state lock poisoned");
                if state.active < self.parallelism {
                    match state.queue.pop_front() {
                        Some(task) => {
                            state.active += 1;
                            self.record_active(state.active);
                            Some(task)
                        }
                        None => None,
                    }
                } else {
                    None
                }
            };
            let Some(task) = task else {
                drop(execution_permit);
                continue;
            };
            self.run_task(
                task.run,
                true,
                Some(execution_permit),
                execution_budget.identity(),
            );
        }
    }

    fn run_task(
        self: &Arc<Self>,
        task: Box<dyn FnOnce() + Send + 'static>,
        counts_activity: bool,
        execution_permit: Option<QueryExecutionPermit>,
        execution_budget: usize,
    ) {
        let _activity = QueryExecutorActivityGuard {
            shared: Arc::clone(self),
            counts_activity,
            _execution_permit: execution_permit,
        };
        let _execution_budget_stack = QueryExecutionBudgetStackGuard::enter(execution_budget);
        let _stack = QueryExecutorStackGuard::enter(Arc::as_ptr(self) as usize);
        task();
    }

    fn record_active(&self, active: usize) {
        self.peak_active.fetch_max(active, Ordering::Relaxed);
    }

    pub(super) fn notify_waiters(&self) {
        drop(
            self.state
                .lock()
                .expect("query executor state lock poisoned"),
        );
        self.ready.notify_all();
    }
}

impl Drop for QueryExecutorActivityGuard {
    fn drop(&mut self) {
        if !self.counts_activity {
            return;
        }
        let mut state = self
            .shared
            .state
            .lock()
            .expect("query executor state lock poisoned");
        state.active = state
            .active
            .checked_sub(1)
            .expect("query executor active task count underflow");
        drop(state);
        self.shared.ready.notify_all();
    }
}

impl QueryExecutorStackGuard {
    fn enter(executor: usize) -> Self {
        QUERY_EXECUTOR_STACK.with(|stack| stack.borrow_mut().push(executor));
        Self { executor }
    }
}

impl QueryExecutionBudgetStackGuard {
    fn enter(budget: usize) -> Self {
        QUERY_EXECUTION_BUDGET_STACK.with(|stack| stack.borrow_mut().push(budget));
        Self { budget }
    }
}

impl Drop for QueryExecutionBudgetStackGuard {
    fn drop(&mut self) {
        QUERY_EXECUTION_BUDGET_STACK.with(|stack| {
            assert_eq!(
                stack.borrow_mut().pop(),
                Some(self.budget),
                "query execution budget stack is unbalanced"
            );
        });
    }
}

impl Drop for QueryExecutorStackGuard {
    fn drop(&mut self) {
        QUERY_EXECUTOR_STACK.with(|stack| {
            assert_eq!(
                stack.borrow_mut().pop(),
                Some(self.executor),
                "query executor stack is unbalanced"
            );
        });
    }
}

impl<O> QueryBatch<O> {
    pub(super) fn new(work_items: usize) -> Self {
        Self {
            state: Mutex::new(QueryBatchState {
                remaining: work_items,
                outcomes: (0..work_items).map(|_| None).collect(),
                completed: VecDeque::with_capacity(work_items),
            }),
        }
    }

    pub(super) fn complete(&self, index: usize, outcome: QueryBatchOutcome<O>) {
        let mut state = self.state.lock().expect("query batch state lock poisoned");
        let slot = state
            .outcomes
            .get_mut(index)
            .expect("query batch result index out of bounds");
        assert!(slot.is_none(), "query batch result completed twice");
        *slot = Some(outcome);
        state.completed.push_back(index);
        state.remaining = state
            .remaining
            .checked_sub(1)
            .expect("query batch remaining count underflow");
    }

    pub(super) fn is_complete(&self) -> bool {
        self.state
            .lock()
            .expect("query batch state lock poisoned")
            .remaining
            == 0
    }

    fn take_completed(&self) -> (Vec<(usize, QueryBatchOutcome<O>)>, bool) {
        let mut state = self.state.lock().expect("query batch state lock poisoned");
        let completed = std::mem::take(&mut state.completed);
        let outcomes = completed
            .into_iter()
            .map(|index| {
                let outcome = state.outcomes[index]
                    .take()
                    .expect("completed query batch result must exist");
                (index, outcome)
            })
            .collect();
        (outcomes, state.remaining == 0)
    }

    pub(super) fn finish(&self) -> Vec<O> {
        // `completed` drives responsive streaming in completion order; `outcomes` remains indexed
        // by submission position so the non-streaming API is deterministic across schedules.
        let outcomes = {
            let mut state = self.state.lock().expect("query batch state lock poisoned");
            assert_eq!(state.remaining, 0, "query batch finished before completion");
            std::mem::take(&mut state.outcomes)
        };
        let mut values = Vec::with_capacity(outcomes.len());
        for outcome in outcomes {
            let outcome = outcome.expect("completed query batch result must exist");
            let value = match outcome {
                Ok(value) => value,
                Err(payload) => resume_unwind(payload),
            };
            values.push(value);
        }
        values
    }
}

impl<O> TaskCompletionStream<'_, O> {
    pub(super) fn wait_next(&mut self) -> Option<(usize, O)> {
        loop {
            while let Some((position, outcome)) = self.pending.pop_front() {
                match outcome {
                    Ok(value) if self.panic.is_none() => return Some((position, value)),
                    Ok(_) => {}
                    Err(payload) => {
                        if self.panic.is_none() {
                            self.panic = Some(payload);
                        }
                    }
                }
            }
            let (completed, is_complete) = self.batch.take_completed();
            self.pending.extend(completed);
            if !self.pending.is_empty() {
                continue;
            }
            if is_complete {
                if let Some(payload) = self.panic.take() {
                    resume_unwind(payload);
                }
                return None;
            }
            if !self.executor.try_run_one(self.batch_id) {
                self.executor.wait_for_batch_progress(&self.batch);
            }
        }
    }

    pub(super) fn drain(&mut self) {
        while self.wait_next().is_some() {}
    }
}

impl<'session, O> QueryTaskPool<'session, O>
where
    O: Send + 'static,
{
    /// Returns the maximum number of accepted-but-not-yet-collected tasks.
    pub fn capacity(&self) -> usize {
        self.capacity
    }

    /// Submits one task, helping/draining when the pool reaches capacity.
    pub fn submit(&mut self, task: impl FnOnce() -> O + Send + 'static) {
        if self.pending.len() >= self.capacity {
            self.wait_one();
        }
        let batch = Arc::new(QueryBatch::new(1));
        let batch_id = Arc::as_ptr(&batch) as usize;
        let executor_shared = Arc::clone(&self.session.inner.executor.shared);
        let task_batch = Arc::clone(&batch);
        self.session
            .inner
            .executor
            .submit_all_priority(vec![QueryTask {
                batch: batch_id,
                run: Box::new(move || {
                    task_batch.complete(0, catch_unwind(AssertUnwindSafe(task)));
                    executor_shared.notify_waiters();
                }),
            }]);
        let position = self.next_position;
        self.next_position += 1;
        self.pending.push_back(SpawnedQueryTask {
            position,
            batch,
            batch_id,
        });
    }

    /// Drains all accepted tasks in submission order, rethrowing the first panic afterwards.
    pub fn finish(mut self) -> Vec<O> {
        let mut panic = None;
        while !self.pending.is_empty() {
            let result = catch_unwind(AssertUnwindSafe(|| self.wait_one()));
            if let Err(payload) = result
                && panic.is_none()
            {
                panic = Some(payload);
            }
        }
        if let Some(payload) = panic {
            resume_unwind(payload);
        }
        self.completed
            .sort_unstable_by_key(|(position, _)| *position);
        std::mem::take(&mut self.completed)
            .into_iter()
            .map(|(_, output)| output)
            .collect()
    }

    fn wait_one(&mut self) {
        loop {
            if let Some(index) = self
                .pending
                .iter()
                .position(|task| task.batch.is_complete())
            {
                let task = self
                    .pending
                    .remove(index)
                    .expect("completed query task must remain pending");
                let output = task.batch.finish().pop().expect("single query task output");
                self.completed.push((task.position, output));
                return;
            }
            let task = self
                .pending
                .front()
                .expect("query task pool must have a pending task");
            if !self.session.inner.executor.try_run_one(task.batch_id) {
                self.session
                    .inner
                    .executor
                    .wait_for_batch_progress(&task.batch);
            }
        }
    }
}

impl<O> Drop for QueryTaskPool<'_, O>
where
    O: Send + 'static,
{
    fn drop(&mut self) {
        while !self.pending.is_empty() {
            let result = catch_unwind(AssertUnwindSafe(|| self.wait_one()));
            if result.is_err() {
                continue;
            }
        }
    }
}
