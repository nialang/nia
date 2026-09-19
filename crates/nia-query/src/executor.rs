// SPDX-License-Identifier: GPL-3.0-or-later
//! Shared worker executor, nested execution budget, and ordered task batches.
//!
//! Workers may help their awaited batch while blocked, but a process-wide
//! execution budget bounds active work across nested query sessions and
//! independent databases. Batch results are restored to submission order
//! before exposure.

use super::*;

impl QueryExecutionBudget {
    pub(super) fn from_environment(parallelism: usize) -> nia_ice::IceResult<Self> {
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
                jobserver::Client::new(parallelism.saturating_sub(1)).map_err(|error| {
                    nia_ice::Ice::new(format!("failed to create query jobserver: {error}"))
                })?
            }
            Err(error) => {
                return Err(nia_ice::Ice::new(format!(
                    "failed to inherit query jobserver: {error}"
                )));
            }
        };
        Self::from_client(client)
    }

    #[cfg(test)]
    pub(super) fn owned(parallelism: usize) -> Self {
        let client = jobserver::Client::new(parallelism.saturating_sub(1))
            .unwrap_or_else(|error| panic!("Nia ICE: failed to create query jobserver: {error}"));
        Self::from_client(client).unwrap_or_else(|ice| panic!("{ice}"))
    }

    fn from_client(client: jobserver::Client) -> nia_ice::IceResult<Self> {
        let shared = Arc::new(QueryExecutionBudgetShared {
            state: Mutex::new(QueryExecutionBudgetState {
                implicit_available: true,
                waiting: 0,
                pending_requests: 0,
                active: 0,
                deliveries: VecDeque::new(),
                failure: None,
            }),
            ready: Condvar::new(),
            peak_active: AtomicUsize::new(0),
        });
        let callback_shared = Arc::clone(&shared);
        let helper = client
            .into_helper_thread(move |delivery| {
                let delivery = delivery.map_err(|error| error.to_string());
                let mut state = callback_shared.state.lock();
                let Some(pending_requests) = state.pending_requests.checked_sub(1) else {
                    state.failure = Some(nia_ice::Ice::new(
                        "query execution budget received an unrequested token",
                    ));
                    drop(state);
                    callback_shared.ready.notify_all();
                    return;
                };
                state.pending_requests = pending_requests;
                match delivery {
                    Ok(token) if state.waiting > 0 => {
                        state.deliveries.push_back(Ok(token));
                    }
                    Ok(_) => {}
                    Err(error) => {
                        state.failure = Some(nia_ice::Ice::new(format!(
                            "failed to acquire query jobserver token: {error}"
                        )));
                    }
                }
                drop(state);
                callback_shared.ready.notify_all();
            })
            .map_err(|error| {
                nia_ice::Ice::new(format!("failed to start query jobserver helper: {error}"))
            })?;
        Ok(Self {
            shared,
            helper: Mutex::new(helper),
        })
    }

    fn acquire(&self) -> nia_ice::IceResult<QueryExecutionPermit> {
        let mut state = self.shared.state.lock();
        state.waiting += 1;
        loop {
            if let Some(ice) = state.failure.clone() {
                state.waiting = state.waiting.saturating_sub(1);
                return Err(ice);
            }
            // A jobserver contributes one implicit slot plus its explicit tokens. Deliveries are
            // assigned before the implicit slot so an already-issued request cannot be stranded
            // while later waiters repeatedly take the process-local slot.
            if let Some(delivery) = state.deliveries.pop_front() {
                state.waiting -= 1;
                let token = match delivery {
                    Ok(token) => token,
                    Err(error) => {
                        drop(state);
                        return Err(nia_ice::Ice::new(format!(
                            "failed to acquire query jobserver token: {error}"
                        )));
                    }
                };
                state.active += 1;
                self.shared.record_active(state.active);
                drop(state);
                return Ok(QueryExecutionPermit {
                    shared: Arc::clone(&self.shared),
                    implicit: false,
                    token: Some(token),
                });
            }
            if state.implicit_available {
                state.implicit_available = false;
                state.waiting -= 1;
                state.active += 1;
                self.shared.record_active(state.active);
                return Ok(QueryExecutionPermit {
                    shared: Arc::clone(&self.shared),
                    implicit: true,
                    token: None,
                });
            }
            let represented_waiters = state.pending_requests + state.deliveries.len();
            let requests = state.waiting.saturating_sub(represented_waiters);
            state.pending_requests += requests;
            if requests > 0 {
                let helper = self.helper.lock();
                for _ in 0..requests {
                    helper.request_token();
                }
            }
            self.shared.ready.wait(&mut state);
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
        let mut state = self.shared.state.lock();
        let Some(active) = state.active.checked_sub(1) else {
            state.failure = Some(nia_ice::Ice::new(
                "query execution budget permit count underflow",
            ));
            drop(state);
            self.shared.ready.notify_all();
            return;
        };
        state.active = active;
        if self.implicit {
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
        parallelism: NonZeroUsize,
        execution_budget: Arc<QueryExecutionBudget>,
    ) -> Self {
        let parallelism = parallelism.get();
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

    fn ensure_workers(&self, work_items: usize) -> nia_ice::IceResult<()> {
        let mut workers = self.workers.lock();
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
                .map_err(|error| {
                    nia_ice::Ice::new(format!("failed to start query executor worker: {error}"))
                })?;
            workers.handles.push(handle);
        }
        Ok(())
    }

    pub(super) fn submit_all(&self, tasks: Vec<QueryTask>) -> nia_ice::IceResult<()> {
        if tasks.is_empty() {
            return Ok(());
        }
        self.ensure_workers(tasks.len())?;
        let mut state = self.shared.state.lock();
        if state.shutdown {
            return Err(nia_ice::Ice::new(
                "query executor cannot accept work after shutdown",
            ));
        }
        state.queue.extend(tasks);
        drop(state);
        self.shared.ready.notify_all();
        Ok(())
    }

    fn submit_all_priority(&self, tasks: Vec<QueryTask>) -> nia_ice::IceResult<()> {
        if tasks.is_empty() {
            return Ok(());
        }
        self.ensure_workers(self.shared.parallelism)?;
        let mut state = self.shared.state.lock();
        if state.shutdown {
            return Err(nia_ice::Ice::new(
                "query executor cannot accept work after shutdown",
            ));
        }
        for task in tasks.into_iter().rev() {
            state.queue.push_front(task);
        }
        drop(state);
        self.shared.ready.notify_all();
        Ok(())
    }

    pub(super) fn try_run_one(&self, batch: usize) -> nia_ice::IceResult<bool> {
        let nested = query_executor_is_active(self.identity());
        let can_run = {
            let state = self.shared.state.lock();
            state.queue.iter().any(|task| task.batch == batch)
                && (nested || state.active < self.shared.parallelism)
        };
        if !can_run {
            return Ok(false);
        }
        // A query waiting inside this executor helps its own batch. Nested work inherits the
        // caller's executor activity and process-wide permit: reacquiring either capacity here
        // could deadlock when every admitted worker is waiting on descendants.
        let execution_budget = &self.execution_budget;
        let execution_permit = if query_execution_budget_is_active(execution_budget.identity()) {
            None
        } else {
            match execution_budget.acquire() {
                Ok(permit) => Some(permit),
                Err(ice) => {
                    if let Some(task) = self.take_batch_task(batch) {
                        (task.settle)(Some(ice));
                        self.shared.notify_waiters();
                        return Ok(true);
                    }
                    return Err(ice);
                }
            }
        };
        let task = {
            let mut state = self.shared.state.lock();
            let position = state.queue.iter().rposition(|task| task.batch == batch);
            if let Some(position) = position {
                let Some(task) = state.queue.remove(position) else {
                    return Err(nia_ice::Ice::new("query batch task position disappeared"));
                };
                if nested {
                    Some((task, false))
                } else if state.active < self.shared.parallelism {
                    state.active += 1;
                    self.shared.record_active(state.active);
                    Some((task, true))
                } else {
                    state.queue.insert(position, task);
                    None
                }
            } else {
                None
            }
        };
        let Some((task, counts_activity)) = task else {
            drop(execution_permit);
            return Ok(false);
        };
        self.shared.run_task(
            task.settle,
            counts_activity,
            execution_permit,
            execution_budget.identity(),
            None,
        );
        Ok(true)
    }

    fn take_batch_task(&self, batch: usize) -> Option<QueryTask> {
        let mut state = self.shared.state.lock();
        let position = state.queue.iter().rposition(|task| task.batch == batch)?;
        state.queue.remove(position)
    }

    fn identity(&self) -> usize {
        Arc::as_ptr(&self.shared) as usize
    }

    pub(super) fn wait_for_batch_progress<V>(&self, batch: &QueryBatch<V>) {
        let mut state = self.shared.state.lock();
        if !batch.is_complete() {
            self.shared.ready.wait(&mut state);
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
            let mut state = self.shared.state.lock();
            state.shutdown = true;
        }
        self.shared.ready.notify_all();
        let current_thread = std::thread::current().id();
        let handles = std::mem::take(&mut self.workers.lock().handles);
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
                let mut state = self.state.lock();
                loop {
                    if state.shutdown && state.queue.is_empty() {
                        return;
                    }
                    if state.active < self.parallelism && !state.queue.is_empty() {
                        break;
                    }
                    self.ready.wait(&mut state);
                }
            }
            let execution_permit = execution_budget.acquire();
            let task = {
                let mut state = self.state.lock();
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
            let (execution_permit, failure) = match execution_permit {
                Ok(permit) => (Some(permit), None),
                Err(ice) => (None, Some(ice)),
            };
            self.run_task(
                task.settle,
                true,
                execution_permit,
                execution_budget.identity(),
                failure,
            );
        }
    }

    fn run_task(
        self: &Arc<Self>,
        settle: Box<dyn FnOnce(Option<nia_ice::Ice>) + Send + 'static>,
        counts_activity: bool,
        execution_permit: Option<QueryExecutionPermit>,
        execution_budget: usize,
        failure: Option<nia_ice::Ice>,
    ) {
        let _activity = QueryExecutorActivityGuard {
            shared: Arc::clone(self),
            counts_activity,
            _execution_permit: execution_permit,
        };
        let _execution_budget_stack = QueryExecutionBudgetStackGuard::enter(execution_budget);
        let _stack = QueryExecutorStackGuard::enter(Arc::as_ptr(self) as usize);
        settle(failure);
    }

    fn record_active(&self, active: usize) {
        self.peak_active.fetch_max(active, Ordering::Relaxed);
    }

    pub(super) fn notify_waiters(&self) {
        drop(self.state.lock());
        self.ready.notify_all();
    }
}

impl Drop for QueryExecutorActivityGuard {
    fn drop(&mut self) {
        if !self.counts_activity {
            return;
        }
        let mut state = self.shared.state.lock();
        state.active = state.active.saturating_sub(1);
        drop(state);
        self.shared.ready.notify_all();
    }
}

impl QueryExecutorStackGuard {
    fn enter(executor: usize) -> Self {
        let previous_depth = QUERY_EXECUTOR_STACK.with(|stack| {
            let mut stack = stack.borrow_mut();
            let previous_depth = stack.len();
            stack.push(executor);
            previous_depth
        });
        Self { previous_depth }
    }
}

impl QueryExecutionBudgetStackGuard {
    fn enter(budget: usize) -> Self {
        let previous_depth = QUERY_EXECUTION_BUDGET_STACK.with(|stack| {
            let mut stack = stack.borrow_mut();
            let previous_depth = stack.len();
            stack.push(budget);
            previous_depth
        });
        Self { previous_depth }
    }
}

impl Drop for QueryExecutionBudgetStackGuard {
    fn drop(&mut self) {
        QUERY_EXECUTION_BUDGET_STACK.with(|stack| {
            stack.borrow_mut().truncate(self.previous_depth);
        });
    }
}

impl Drop for QueryExecutorStackGuard {
    fn drop(&mut self) {
        QUERY_EXECUTOR_STACK.with(|stack| {
            stack.borrow_mut().truncate(self.previous_depth);
        });
    }
}

impl<O> QueryBatch<O> {
    pub(super) fn new(work_items: usize) -> Self {
        Self {
            state: Mutex::new(QueryBatchState {
                remaining: work_items,
                outcomes: (0..work_items).map(|_| QueryBatchSlot::Pending).collect(),
                completed: VecDeque::with_capacity(work_items),
                failure: None,
            }),
        }
    }

    pub(super) fn complete(
        &self,
        index: usize,
        outcome: QueryBatchOutcome<O>,
    ) -> nia_ice::IceResult<()> {
        let mut state = self.state.lock();
        if let Some(ice) = &state.failure {
            return Err(ice.clone());
        }
        let Some(slot) = state.outcomes.get_mut(index) else {
            let ice = nia_ice::Ice::new("query batch result index is out of bounds");
            state.fail(ice.clone());
            return Err(ice);
        };
        if !matches!(slot, QueryBatchSlot::Pending) {
            let ice = nia_ice::Ice::new("query batch result completed twice");
            state.fail(ice.clone());
            return Err(ice);
        }
        *slot = QueryBatchSlot::Ready(outcome);
        state.completed.push_back(index);
        if state.remaining == 0 {
            let ice = nia_ice::Ice::new("query batch remaining count underflow");
            state.fail(ice.clone());
            return Err(ice);
        }
        state.remaining -= 1;
        Ok(())
    }

    pub(super) fn is_complete(&self) -> bool {
        self.state.lock().remaining == 0
    }

    pub(super) fn take_completed(&self) -> nia_ice::IceResult<QueryBatchProgress<O>> {
        let mut state = self.state.lock();
        if let Some(ice) = &state.failure {
            return Err(ice.clone());
        }
        let completed = std::mem::take(&mut state.completed);
        let mut outcomes = Vec::with_capacity(completed.len());
        for index in completed {
            let Some(slot) = state.outcomes.get_mut(index) else {
                let ice = nia_ice::Ice::new("completed query batch result is missing");
                state.fail(ice.clone());
                return Err(ice);
            };
            let QueryBatchSlot::Ready(outcome) = std::mem::replace(slot, QueryBatchSlot::Taken)
            else {
                let ice = nia_ice::Ice::new("completed query batch result is missing");
                state.fail(ice.clone());
                return Err(ice);
            };
            outcomes.push((index, outcome));
        }
        Ok((outcomes, state.remaining == 0))
    }

    pub(super) fn finish(&self) -> nia_ice::IceResult<Vec<O>> {
        // `completed` drives responsive streaming in completion order; `outcomes` remains indexed
        // by submission position so the non-streaming API is deterministic across schedules.
        let outcomes = {
            let mut state = self.state.lock();
            if let Some(ice) = &state.failure {
                return Err(ice.clone());
            }
            if state.remaining != 0 {
                return Err(nia_ice::Ice::new("query batch finished before completion"));
            }
            std::mem::take(&mut state.outcomes)
        };
        let mut values = Vec::with_capacity(outcomes.len());
        let mut failure = None;
        for outcome in outcomes {
            let QueryBatchSlot::Ready(outcome) = outcome else {
                return Err(nia_ice::Ice::new("completed query batch result is missing"));
            };
            match outcome {
                Ok(value) if failure.is_none() => values.push(value),
                Ok(_) => {}
                Err(ice) if failure.is_none() => failure = Some(ice),
                Err(_) => {}
            }
        }
        match failure {
            Some(ice) => Err(ice),
            None => Ok(values),
        }
    }
}

impl<O> QueryBatchState<O> {
    fn fail(&mut self, ice: nia_ice::Ice) {
        if self.failure.is_none() {
            self.failure = Some(ice);
        }
        self.remaining = 0;
        self.completed.clear();
    }
}

impl<O> TaskCompletionStream<'_, O> {
    pub(super) fn wait_next(&mut self) -> nia_ice::IceResult<Option<(usize, O)>> {
        loop {
            while let Some((position, outcome)) = self.pending.pop_front() {
                match outcome {
                    Ok(value) if self.failure.is_none() => return Ok(Some((position, value))),
                    Ok(_) => {}
                    Err(ice) => {
                        if self.failure.is_none() {
                            self.failure = Some(ice);
                        }
                    }
                }
            }
            let (completed, is_complete) = self.batch.take_completed()?;
            self.pending.extend(completed);
            if !self.pending.is_empty() {
                continue;
            }
            if is_complete {
                if let Some(ice) = self.failure.take() {
                    return Err(ice);
                }
                return Ok(None);
            }
            if !self.executor.try_run_one(self.batch_id)? {
                self.executor.wait_for_batch_progress(&self.batch);
            }
        }
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
    pub fn submit(
        &mut self,
        task: impl FnOnce() -> nia_ice::IceResult<O> + Send + 'static,
    ) -> nia_ice::IceResult<()> {
        if self.pending.len() >= self.capacity {
            self.wait_one()?;
        }
        self.session.inner.ensure_healthy()?;
        let batch = Arc::new(QueryBatch::new(1));
        let batch_id = Arc::as_ptr(&batch) as usize;
        let executor_shared = Arc::clone(&self.session.inner.executor.shared);
        let task_batch = Arc::clone(&batch);
        let session = Arc::clone(&self.session.inner);
        self.session
            .inner
            .executor
            .submit_all_priority(vec![QueryTask {
                batch: batch_id,
                settle: Box::new(move |failure| {
                    let outcome = match failure {
                        Some(ice) => Err(session.record_failure(ice)),
                        None => session
                            .ensure_healthy()
                            .and_then(|()| task())
                            .map_err(|ice| session.record_failure(ice)),
                    };
                    if let Err(ice) = task_batch.complete(0, outcome) {
                        session.record_failure(ice);
                    }
                    executor_shared.notify_waiters();
                }),
            }])
            .map_err(|ice| self.session.inner.record_failure(ice))?;
        let position = self.next_position;
        self.next_position += 1;
        self.pending.push_back(SpawnedQueryTask {
            position,
            batch,
            batch_id,
        });
        Ok(())
    }

    /// Drains all accepted tasks and returns the first internal failure after cleanup.
    pub fn finish(mut self) -> nia_ice::IceResult<Vec<O>> {
        while !self.pending.is_empty() {
            if let Err(ice) = self.wait_one()
                && self.failure.is_none()
            {
                self.failure = Some(ice);
            }
        }
        if let Some(ice) = self.failure.take() {
            return Err(ice);
        }
        self.completed
            .sort_unstable_by_key(|(position, _)| *position);
        let completed = std::mem::take(&mut self.completed)
            .into_iter()
            .map(|(_, output)| output)
            .collect();
        Ok(completed)
    }

    fn wait_one(&mut self) -> nia_ice::IceResult<()> {
        loop {
            if let Some(index) = self
                .pending
                .iter()
                .position(|task| task.batch.is_complete())
            {
                let Some(task) = self.pending.remove(index) else {
                    return Err(nia_ice::Ice::new(
                        "completed query task disappeared from the pending pool",
                    ));
                };
                let Some(output) = task.batch.finish()?.pop() else {
                    return Err(nia_ice::Ice::new(
                        "single query task completed without an output",
                    ));
                };
                self.completed.push((task.position, output));
                return Ok(());
            }
            let Some(task) = self.pending.front() else {
                return Err(nia_ice::Ice::new("query task pool lost its pending task"));
            };
            if !self.session.inner.executor.try_run_one(task.batch_id)? {
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
            let _ = self.wait_one();
        }
    }
}
