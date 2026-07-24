// SPDX-License-Identifier: GPL-3.0-or-later
mod resources;

pub use resources::{
    ProcessMemoryPermit, acquire_llvm_memory_permit, effective_available_memory_bytes,
    effective_memory_limit_bytes, llvm_memory_task_capacity,
};

use std::{
    any::{Any, TypeId},
    cell::RefCell,
    collections::VecDeque,
    fmt::{self, Debug},
    hash::Hash,
    panic::{AssertUnwindSafe, catch_unwind, resume_unwind},
    sync::{
        Arc, Condvar, Mutex, OnceLock, Weak,
        atomic::{AtomicU32, AtomicU64, AtomicUsize, Ordering},
    },
    thread::JoinHandle,
};

use nia_hash::{FastHashMap, FastHashSet};

const DEFAULT_MAX_QUERY_EXECUTOR_PARALLELISM: usize = 4;

pub trait QueryKey<C>: Clone + Debug + Eq + Hash + Send + Sync + 'static {
    type Value: Send + Sync + 'static;

    const FINGERPRINT: QueryFingerprintPolicy = QueryFingerprintPolicy::None;
    const STORAGE: QueryStoragePolicy = QueryStoragePolicy::CacheOwnedArc;
    const PROVIDER: QueryProviderPolicy = QueryProviderPolicy::KeyExecute;

    fn name() -> &'static str;
    fn description(&self) -> String {
        format!("{}::{self:?}", Self::name())
    }
    fn execute(&self, db: &QueryDb<C>) -> Self::Value;
    fn fingerprint(&self, _value: &Self::Value) -> Option<QueryFingerprint> {
        None
    }
    fn values_equal(&self, _old: &Self::Value, _new: &Self::Value) -> bool {
        false
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum QueryProviderPolicy {
    KeyExecute,
    ExternallyPublished,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum QueryFingerprintPolicy {
    None,
    StableValue,
    SemanticValue,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct QueryFingerprint([u64; 2]);

impl QueryFingerprint {
    pub const fn parts(self) -> [u64; 2] {
        self.0
    }
}

pub struct QueryFingerprintBuilder {
    state: [u64; 2],
}

impl QueryFingerprintBuilder {
    const FIRST_OFFSET: u64 = 0xcbf2_9ce4_8422_2325;
    const FIRST_PRIME: u64 = 0x0000_0100_0000_01b3;
    const SECOND_OFFSET: u64 = 0x6c62_272e_07bb_0142;
    const SECOND_PRIME: u64 = 0x9e37_79b1_85eb_ca87;

    pub fn new(domain: &str) -> Self {
        let mut builder = Self {
            state: [Self::FIRST_OFFSET, Self::SECOND_OFFSET],
        };
        builder.write_str(domain);
        builder
    }

    pub fn write_u8(&mut self, value: u8) {
        self.write_raw_bytes(&[value]);
    }

    pub fn write_u64(&mut self, value: u64) {
        self.write_raw_bytes(&value.to_le_bytes());
    }

    pub fn write_bytes(&mut self, bytes: &[u8]) {
        self.write_u64(bytes.len() as u64);
        self.write_raw_bytes(bytes);
    }

    pub fn write_str(&mut self, text: &str) {
        self.write_bytes(text.as_bytes());
    }

    pub fn write_fingerprint(&mut self, fingerprint: QueryFingerprint) {
        for part in fingerprint.parts() {
            self.write_u64(part);
        }
    }

    pub fn finish(self) -> QueryFingerprint {
        QueryFingerprint(self.state)
    }

    fn write_raw_bytes(&mut self, bytes: &[u8]) {
        for byte in bytes {
            self.state[0] ^= u64::from(*byte);
            self.state[0] = self.state[0].wrapping_mul(Self::FIRST_PRIME);
            self.state[1] ^= u64::from(*byte);
            self.state[1] = self.state[1]
                .rotate_left(7)
                .wrapping_mul(Self::SECOND_PRIME);
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum QueryStoragePolicy {
    CacheOwnedArc,
    SingleConsumerOwned,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct QueryDescriptor {
    pub name: &'static str,
    pub context_type: &'static str,
    pub key_type: &'static str,
    pub value_type: &'static str,
    pub provider: QueryProviderPolicy,
    pub fingerprint: QueryFingerprintPolicy,
    pub storage: QueryStoragePolicy,
}

#[derive(Debug, Default)]
pub struct QueryRegistry {
    descriptors: FastHashMap<TypeId, QueryDescriptor>,
    names: FastHashMap<&'static str, TypeId>,
}

impl QueryRegistry {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn register<C, K>(&mut self)
    where
        C: 'static,
        K: QueryKey<C>,
    {
        assert!(
            K::STORAGE == QueryStoragePolicy::CacheOwnedArc
                || K::FINGERPRINT == QueryFingerprintPolicy::None,
            "single-consumer query `{}` cannot retain a value fingerprint",
            K::name()
        );
        assert!(
            K::PROVIDER == QueryProviderPolicy::KeyExecute
                || (K::STORAGE == QueryStoragePolicy::SingleConsumerOwned
                    && K::FINGERPRINT == QueryFingerprintPolicy::None),
            "externally published query `{}` must use single-consumer owned storage",
            K::name()
        );
        let key_type_id = TypeId::of::<K>();
        assert!(
            !self.descriptors.contains_key(&key_type_id),
            "query key type `{}` is already registered",
            std::any::type_name::<K>()
        );
        if let Some(existing) = self.names.get(K::name()) {
            let existing = self
                .descriptors
                .get(existing)
                .expect("query registry name index must reference a descriptor");
            panic!(
                "query name `{}` is already registered for `{}`",
                K::name(),
                existing.key_type
            );
        }
        self.names.insert(K::name(), key_type_id);
        self.descriptors.insert(
            key_type_id,
            QueryDescriptor {
                name: K::name(),
                context_type: std::any::type_name::<C>(),
                key_type: std::any::type_name::<K>(),
                value_type: std::any::type_name::<K::Value>(),
                provider: K::PROVIDER,
                fingerprint: K::FINGERPRINT,
                storage: K::STORAGE,
            },
        );
    }

    pub fn descriptors(&self) -> Vec<QueryDescriptor> {
        let mut descriptors = self.descriptors.values().cloned().collect::<Vec<_>>();
        descriptors.sort_by_key(|descriptor| descriptor.name);
        descriptors
    }

    fn assert_registered<C, K>(&self)
    where
        K: QueryKey<C>,
    {
        assert!(
            self.descriptors.contains_key(&TypeId::of::<K>()),
            "query key type `{}` is not in the declarative registry",
            std::any::type_name::<K>()
        );
    }
}

pub struct QueryDb<C> {
    inner: Arc<QueryDbInner<C>>,
}

pub struct QueryRetirement<'a, C> {
    db: &'a QueryDb<C>,
}

#[derive(Clone)]
pub struct QuerySession {
    inner: Arc<QuerySessionInner>,
}

struct QuerySessionInner {
    id: QuerySessionId,
    executor: QueryExecutor,
    databases: Mutex<FastHashMap<QueryDbId, Arc<dyn ErasedQueryDatabase>>>,
    dependencies: Mutex<QueryDependencyGraph>,
    activity: Mutex<QueryActivityState>,
    activity_ready: Condvar,
}

#[derive(Default)]
struct QueryActivityState {
    active: usize,
    retiring: bool,
}

struct QueryActivityGuard<'a> {
    session: &'a QuerySessionInner,
}

struct QueryRetirementGuard<'a> {
    session: &'a QuerySessionInner,
}

struct QueryTask {
    batch: usize,
    run: Box<dyn FnOnce() + Send + 'static>,
}

struct QueryExecutor {
    session_id: QuerySessionId,
    shared: Arc<QueryExecutorShared>,
    execution_budget: Arc<QueryExecutionBudget>,
    workers: Mutex<QueryExecutorWorkers>,
}

struct QueryExecutorShared {
    parallelism: usize,
    state: Mutex<QueryExecutorState>,
    ready: Condvar,
    peak_active: AtomicUsize,
}

#[derive(Default)]
struct QueryExecutorState {
    queue: VecDeque<QueryTask>,
    active: usize,
    shutdown: bool,
}

struct QueryExecutorWorkers {
    handles: Vec<JoinHandle<()>>,
}

struct QueryExecutorStackGuard {
    executor: usize,
}

struct QueryExecutorActivityGuard {
    shared: Arc<QueryExecutorShared>,
    counts_activity: bool,
    _execution_permit: Option<QueryExecutionPermit>,
}

struct QueryExecutionBudget {
    shared: Arc<QueryExecutionBudgetShared>,
    helper: Mutex<jobserver::HelperThread>,
}

struct QueryExecutionBudgetShared {
    state: Mutex<QueryExecutionBudgetState>,
    ready: Condvar,
    peak_active: AtomicUsize,
}

struct QueryExecutionBudgetState {
    implicit_available: bool,
    waiting: usize,
    pending_requests: usize,
    active: usize,
    deliveries: VecDeque<Result<jobserver::Acquired, String>>,
}

struct QueryExecutionPermit {
    shared: Arc<QueryExecutionBudgetShared>,
    implicit: bool,
    token: Option<jobserver::Acquired>,
}

struct QueryExecutionBudgetStackGuard {
    budget: usize,
}

type QueryBatchOutcome<O> = Result<O, Box<dyn Any + Send>>;

struct QueryBatch<O> {
    state: Mutex<QueryBatchState<O>>,
}

struct QueryBatchState<O> {
    remaining: usize,
    outcomes: Vec<Option<QueryBatchOutcome<O>>>,
    completed: VecDeque<usize>,
}

struct TaskCompletionStream<'a, O> {
    executor: &'a QueryExecutor,
    batch: Arc<QueryBatch<O>>,
    batch_id: usize,
    pending: VecDeque<(usize, QueryBatchOutcome<O>)>,
    panic: Option<Box<dyn Any + Send>>,
}

struct SpawnedQueryTask<O> {
    position: usize,
    batch: Arc<QueryBatch<O>>,
    batch_id: usize,
}

pub struct QueryTaskPool<'session, O: Send + 'static> {
    session: &'session QuerySession,
    _activity: QueryActivityGuard<'session>,
    capacity: usize,
    next_position: usize,
    pending: VecDeque<SpawnedQueryTask<O>>,
    completed: Vec<(usize, O)>,
}

pub struct QueryCompletionStream<'stream, 'executor, O> {
    tasks: &'stream mut TaskCompletionStream<'executor, (O, RecordedDependencies)>,
    dependencies: RecordedDependencies,
}

struct QueryDbInner<C> {
    id: QueryDbId,
    session: QuerySession,
    context: C,
    timings: nia_timing::TimingMode,
    registry: Option<QueryRegistry>,
    caches: Mutex<FastHashMap<TypeId, Box<dyn Any + Send + Sync>>>,
    slots: Mutex<QuerySlotTable<C>>,
}

struct QuerySlot<V> {
    node_id: QueryNodeId,
    stats: QuerySlotStats,
    fingerprint_revision: AtomicU64,
    state: Mutex<QueryState<V>>,
    ready: Condvar,
}

impl<V> QuerySlot<V> {
    fn next_semantic_fingerprint(&self, query_name: &str) -> QueryFingerprint {
        let revision = self.fingerprint_revision.fetch_add(1, Ordering::Relaxed);
        let mut builder = QueryFingerprintBuilder::new("nia.query.semantic-value.v1");
        builder.write_str(query_name);
        builder.write_u64(u64::from(self.node_id.db_id.0));
        builder.write_u64(u64::from(self.node_id.index));
        builder.write_u64(revision);
        builder.finish()
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
struct QueryDbId(u32);

impl QueryDbId {
    fn fresh() -> Self {
        static NEXT_QUERY_DB_ID: AtomicU32 = AtomicU32::new(1);
        let id = NEXT_QUERY_DB_ID
            .fetch_update(Ordering::Relaxed, Ordering::Relaxed, |id| id.checked_add(1))
            .expect("query database identity space exhausted");
        Self(id)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
struct QuerySessionId(u32);

impl QuerySessionId {
    fn fresh() -> Self {
        static NEXT_QUERY_SESSION_ID: AtomicU32 = AtomicU32::new(1);
        let id = NEXT_QUERY_SESSION_ID
            .fetch_update(Ordering::Relaxed, Ordering::Relaxed, |id| id.checked_add(1))
            .expect("query session identity space exhausted");
        Self(id)
    }
}

impl QueryExecutionBudget {
    fn from_environment(parallelism: usize) -> Self {
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
    fn owned(parallelism: usize) -> Self {
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
    fn peak_active(&self) -> usize {
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
    fn new(
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

    fn submit_all(&self, tasks: Vec<QueryTask>) {
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

    fn try_run_one(&self, batch: usize) -> bool {
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

    fn wait_for_batch_progress<V>(&self, batch: &QueryBatch<V>) {
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
    fn peak_active(&self) -> usize {
        self.shared.peak_active.load(Ordering::Relaxed)
    }
}

impl Drop for QueryExecutor {
    fn drop(&mut self) {
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

    fn notify_waiters(&self) {
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
    fn new(work_items: usize) -> Self {
        Self {
            state: Mutex::new(QueryBatchState {
                remaining: work_items,
                outcomes: (0..work_items).map(|_| None).collect(),
                completed: VecDeque::with_capacity(work_items),
            }),
        }
    }

    fn complete(&self, index: usize, outcome: QueryBatchOutcome<O>) {
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

    fn is_complete(&self) -> bool {
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

    fn finish(&self) -> Vec<O> {
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
    fn wait_next(&mut self) -> Option<(usize, O)> {
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

    fn drain(&mut self) {
        while self.wait_next().is_some() {}
    }
}

impl<'session, O> QueryTaskPool<'session, O>
where
    O: Send + 'static,
{
    pub fn capacity(&self) -> usize {
        self.capacity
    }

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

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
struct QueryNodeId {
    db_id: QueryDbId,
    index: u32,
}

struct QuerySlotTable<C> {
    next_index: u32,
    entries: FastHashMap<u32, QuerySlotRecord<C>>,
}

impl<C> Default for QuerySlotTable<C> {
    fn default() -> Self {
        Self {
            next_index: 0,
            entries: FastHashMap::default(),
        }
    }
}

struct QuerySlotRecord<C> {
    identity: QuerySlotIdentity,
    slot: Arc<dyn ErasedQuerySlot>,
    ensure: fn(&QueryDb<C>, &dyn ErasedQueryKey) -> QueryResult<()>,
}

impl<C> QuerySlotTable<C> {
    fn next_id(&mut self, db_id: QueryDbId) -> QueryNodeId {
        let index = self.next_index;
        self.next_index = self
            .next_index
            .checked_add(1)
            .expect("query node identity space exhausted");
        QueryNodeId { db_id, index }
    }

    fn push(
        &mut self,
        node_id: QueryNodeId,
        identity: QuerySlotIdentity,
        slot: Arc<dyn ErasedQuerySlot>,
        ensure: fn(&QueryDb<C>, &dyn ErasedQueryKey) -> QueryResult<()>,
    ) {
        let previous = self.entries.insert(
            node_id.index,
            QuerySlotRecord {
                identity,
                slot,
                ensure,
            },
        );
        assert!(previous.is_none(), "query node identity was reused");
    }

    fn get(&self, db_id: QueryDbId, node_id: QueryNodeId) -> Option<&QuerySlotRecord<C>> {
        if node_id.db_id != db_id {
            return None;
        }
        self.entries.get(&node_id.index)
    }

    fn remove(&mut self, db_id: QueryDbId, node_id: QueryNodeId) -> Option<QuerySlotRecord<C>> {
        (node_id.db_id == db_id)
            .then(|| self.entries.remove(&node_id.index))
            .flatten()
    }

    fn frame(&self, db_id: QueryDbId, node_id: QueryNodeId) -> QueryFrame {
        self.get(db_id, node_id)
            .expect("query node id must reference a registered slot")
            .identity
            .frame()
    }
}

#[derive(Debug, Default)]
struct QuerySlotStats {
    executions: AtomicUsize,
    cache_hits: AtomicUsize,
    waits: AtomicUsize,
    validations: AtomicUsize,
    green_validations: AtomicUsize,
}

impl QuerySlotStats {
    fn record_execution(&self) {
        self.executions.fetch_add(1, Ordering::Relaxed);
    }

    fn record_cache_hit(&self) {
        self.cache_hits.fetch_add(1, Ordering::Relaxed);
    }

    fn record_wait(&self) {
        self.waits.fetch_add(1, Ordering::Relaxed);
    }

    fn record_validation(&self) {
        self.validations.fetch_add(1, Ordering::Relaxed);
    }

    fn record_green_validation(&self) {
        self.green_validations.fetch_add(1, Ordering::Relaxed);
    }

    fn snapshot(&self) -> QueryFrameStats {
        QueryFrameStats {
            executions: self.executions.load(Ordering::Relaxed),
            cache_hits: self.cache_hits.load(Ordering::Relaxed),
            waits: self.waits.load(Ordering::Relaxed),
            validations: self.validations.load(Ordering::Relaxed),
            green_validations: self.green_validations.load(Ordering::Relaxed),
        }
    }
}

enum QueryState<V> {
    Empty,
    Consumed,
    Published {
        value: V,
    },
    Computing {
        invalidated: bool,
    },
    Validating {
        invalidated: bool,
    },
    Ready {
        value: Arc<V>,
        fingerprint: Option<QueryFingerprint>,
        dependency_fingerprints: DependencyFingerprints,
    },
    PotentiallyOutdated {
        value: Arc<V>,
        fingerprint: QueryFingerprint,
        dependency_fingerprints: DependencyFingerprints,
    },
}

type DependencyFingerprints = FastHashMap<QueryNodeId, Option<QueryFingerprint>>;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum QueryInvalidationDisposition {
    Cleared,
    PotentiallyOutdated,
}

trait ErasedQuerySlot: Send + Sync {
    fn invalidate(&self);
    fn mark_potentially_outdated(&self) -> QueryInvalidationDisposition;
    fn fingerprint(&self) -> Option<QueryFingerprint>;
    fn stats(&self) -> QueryFrameStats;
}

impl<V> ErasedQuerySlot for QuerySlot<V>
where
    V: Send + Sync + 'static,
{
    fn invalidate(&self) {
        let mut state = self.state.lock().expect("query cache lock poisoned");
        match &mut *state {
            QueryState::Empty | QueryState::Consumed | QueryState::Published { .. } => {
                *state = QueryState::Empty;
            }
            QueryState::Computing { invalidated } | QueryState::Validating { invalidated } => {
                *invalidated = true;
            }
            QueryState::Ready { .. } | QueryState::PotentiallyOutdated { .. } => {
                *state = QueryState::Empty;
                self.ready.notify_all();
            }
        }
    }

    fn mark_potentially_outdated(&self) -> QueryInvalidationDisposition {
        let mut state = self.state.lock().expect("query cache lock poisoned");
        let previous = std::mem::replace(&mut *state, QueryState::Empty);
        match previous {
            QueryState::Ready {
                value,
                fingerprint: Some(fingerprint),
                dependency_fingerprints,
            } => {
                *state = QueryState::PotentiallyOutdated {
                    value,
                    fingerprint,
                    dependency_fingerprints,
                };
                QueryInvalidationDisposition::PotentiallyOutdated
            }
            QueryState::PotentiallyOutdated {
                value,
                fingerprint,
                dependency_fingerprints,
            } => {
                *state = QueryState::PotentiallyOutdated {
                    value,
                    fingerprint,
                    dependency_fingerprints,
                };
                QueryInvalidationDisposition::PotentiallyOutdated
            }
            QueryState::Computing { .. } => {
                *state = QueryState::Computing { invalidated: true };
                QueryInvalidationDisposition::Cleared
            }
            QueryState::Validating { .. } => {
                *state = QueryState::Validating { invalidated: true };
                QueryInvalidationDisposition::Cleared
            }
            QueryState::Empty
            | QueryState::Consumed
            | QueryState::Published { .. }
            | QueryState::Ready { .. } => {
                self.ready.notify_all();
                QueryInvalidationDisposition::Cleared
            }
        }
    }

    fn fingerprint(&self) -> Option<QueryFingerprint> {
        let state = self.state.lock().expect("query cache lock poisoned");
        match &*state {
            QueryState::Ready { fingerprint, .. } => *fingerprint,
            QueryState::Empty
            | QueryState::Consumed
            | QueryState::Published { .. }
            | QueryState::Computing { .. }
            | QueryState::Validating { .. }
            | QueryState::PotentiallyOutdated { .. } => None,
        }
    }

    fn stats(&self) -> QueryFrameStats {
        self.stats.snapshot()
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct QueryFrame {
    pub name: &'static str,
    pub key: String,
    pub description: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct QueryDependency {
    pub from: QueryFrame,
    pub to: QueryFrame,
}

#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct QueryInvalidation {
    pub invalidated: Vec<QueryFrame>,
}

struct QuerySlotIdentity {
    key: Arc<dyn ErasedQueryKey>,
    make_frame: fn(&dyn ErasedQueryKey) -> QueryFrame,
}

impl QuerySlotIdentity {
    fn frame(&self) -> QueryFrame {
        (self.make_frame)(self.key.as_ref())
    }
}

trait ErasedQueryKey: Send + Sync {
    fn as_any(&self) -> &dyn Any;
}

impl<K> ErasedQueryKey for K
where
    K: Clone + Eq + Hash + Send + Sync + 'static,
{
    fn as_any(&self) -> &dyn Any {
        self
    }
}

#[derive(Debug, Default)]
struct QueryDependencyGraph {
    forward: FastHashMap<QueryNodeId, FastHashSet<QueryNodeId>>,
    reverse: FastHashMap<QueryNodeId, FastHashSet<QueryNodeId>>,
}

trait ErasedQueryDatabase: Send + Sync {
    fn frame(&self, node_id: QueryNodeId) -> Option<QueryFrame>;
    fn slot(&self, node_id: QueryNodeId) -> Option<Arc<dyn ErasedQuerySlot>>;
    fn ensure(&self, node_id: QueryNodeId) -> QueryResult<()>;
}

struct QueryDbRegistration<C> {
    inner: Weak<QueryDbInner<C>>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum QueryError {
    Cycle { cycle: Vec<QueryFrame> },
    InvalidInput { query: QueryFrame, message: String },
}

impl fmt::Display for QueryError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            QueryError::Cycle { cycle } => {
                writeln!(f, "query cycle detected")?;
                for frame in cycle {
                    writeln!(f, "  {}", frame.description)?;
                }
                Ok(())
            }
            QueryError::InvalidInput { query, message } => {
                write!(
                    f,
                    "invalid query input for {}: {message}",
                    query.description
                )
            }
        }
    }
}

impl std::error::Error for QueryError {}

pub type QueryResult<T> = Result<T, QueryError>;

#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct QueryTrace {
    pub dependencies: Vec<QueryDependency>,
    pub queries: Vec<QueryTraceQuery>,
}

#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct QueryFrameStats {
    pub executions: usize,
    pub cache_hits: usize,
    pub waits: usize,
    pub validations: usize,
    pub green_validations: usize,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct QueryTraceQuery {
    pub frame: QueryFrame,
    pub stats: QueryFrameStats,
}

#[derive(Debug, Clone)]
struct QueryStackEntry {
    session_id: QuerySessionId,
    node_id: QueryNodeId,
    dependencies: FastHashSet<QueryNodeId>,
    dependency_fingerprints: Option<DependencyFingerprints>,
}

#[derive(Default)]
struct RecordedDependencies {
    nodes: FastHashSet<QueryNodeId>,
    fingerprints: Option<DependencyFingerprints>,
}

struct QueryStackGuard {
    active: bool,
}

struct QueryStackInstallGuard {
    previous: Vec<QueryStackEntry>,
}

impl<O> QueryCompletionStream<'_, '_, O> {
    pub fn wait_next(&mut self) -> Option<(usize, O)> {
        let (position, (value, task_dependencies)) = self.tasks.wait_next()?;
        self.dependencies.nodes.extend(task_dependencies.nodes);
        if let (Some(dependencies), Some(task_dependencies)) = (
            self.dependencies.fingerprints.as_mut(),
            task_dependencies.fingerprints,
        ) {
            dependencies.extend(task_dependencies);
        }
        Some((position, value))
    }
}

thread_local! {
    static QUERY_STACK: RefCell<Vec<QueryStackEntry>> = const { RefCell::new(Vec::new()) };
    static QUERY_EXECUTOR_STACK: RefCell<Vec<usize>> = const { RefCell::new(Vec::new()) };
    static QUERY_EXECUTION_BUDGET_STACK: RefCell<Vec<usize>> = const { RefCell::new(Vec::new()) };
    static QUERY_ACTIVITY_DEPTHS: RefCell<Vec<(usize, usize)>> = const { RefCell::new(Vec::new()) };
}

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
    fn with_parallelism(parallelism: usize) -> Self {
        Self::with_execution_budget(
            parallelism,
            Arc::new(QueryExecutionBudget::owned(parallelism)),
        )
    }

    fn with_execution_budget(
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

    fn run_tasks_inner<T, O>(&self, tasks: impl IntoIterator<Item = T>) -> Vec<O>
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

    fn with_task_completion_stream_inner<T, O, R>(
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

    fn enter_activity(&self) -> QueryActivityGuard<'_> {
        let identity = Arc::as_ptr(&self.inner) as usize;
        let nested = query_activity_is_active(identity);
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

    fn enter_retirement(&self) -> QueryRetirementGuard<'_> {
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

    fn register<C>(&self, db: &QueryDb<C>)
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

    fn database(&self, db_id: QueryDbId) -> Arc<dyn ErasedQueryDatabase> {
        self.inner
            .databases
            .lock()
            .expect("query session database lock poisoned")
            .get(&db_id)
            .cloned()
            .expect("query node references an unknown database")
    }

    fn frame(&self, node_id: QueryNodeId) -> QueryFrame {
        self.database(node_id.db_id)
            .frame(node_id)
            .expect("query node id must reference a registered slot")
    }

    fn slot(&self, node_id: QueryNodeId) -> Arc<dyn ErasedQuerySlot> {
        self.database(node_id.db_id)
            .slot(node_id)
            .expect("query node id must reference a registered slot")
    }

    fn ensure(&self, node_id: QueryNodeId) -> QueryResult<()> {
        self.database(node_id.db_id).ensure(node_id)
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

    pub fn get<K>(&self, key: K) -> Arc<K::Value>
    where
        K: QueryKey<C>,
    {
        self.try_get(key)
            .unwrap_or_else(|err| std::panic::panic_any(err))
    }

    pub fn get_owned<K>(&self, key: K) -> K::Value
    where
        K: QueryKey<C>,
    {
        self.try_get_owned(key)
            .unwrap_or_else(|err| std::panic::panic_any(err))
    }

    pub fn invalid_input<K>(&self, key: &K, message: impl Into<String>) -> !
    where
        K: QueryKey<C>,
    {
        std::panic::panic_any(QueryError::InvalidInput {
            query: query_frame::<C, K>(key),
            message: message.into(),
        })
    }

    pub fn try_get<K>(&self, key: K) -> QueryResult<Arc<K::Value>>
    where
        K: QueryKey<C>,
    {
        self.try_get_cached(key)
    }

    pub fn try_get_owned<K>(&self, key: K) -> QueryResult<K::Value>
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
                            key.execute(self)
                        })
                    })) {
                        Ok(value) => value,
                        Err(payload) => {
                            let mut state = slot.state.lock().expect("query cache lock poisoned");
                            *state = QueryState::Empty;
                            guard.discard();
                            self.clear_dependencies_from(node_id);
                            slot.ready.notify_all();
                            drop(state);
                            match payload.downcast::<QueryError>() {
                                Ok(err) => return Err(*err),
                                Err(payload) => resume_unwind(payload),
                            }
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
                            key.execute(self)
                        })
                    })) {
                        Ok(value) => value,
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
                            match payload.downcast::<QueryError>() {
                                Ok(err) => return Err(*err),
                                Err(payload) => std::panic::resume_unwind(payload),
                            }
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

    pub fn get_many<K>(&self, keys: impl IntoIterator<Item = K>) -> Vec<Arc<K::Value>>
    where
        C: Send + Sync + 'static,
        K: QueryKey<C>,
    {
        self.get_many_with(keys, Self::get::<K>)
    }

    pub fn get_many_owned<K>(&self, keys: impl IntoIterator<Item = K>) -> Vec<K::Value>
    where
        C: Send + Sync + 'static,
        K: QueryKey<C>,
    {
        self.get_many_with(keys, Self::get_owned::<K>)
    }

    pub fn for_each_many_owned<K>(
        &self,
        keys: impl IntoIterator<Item = K>,
        on_complete: impl FnMut(usize, K::Value),
    ) where
        C: Send + Sync + 'static,
        K: QueryKey<C>,
    {
        self.for_each_many_with(keys, Self::get_owned::<K>, on_complete)
    }

    pub fn with_many_owned_completion<K, R>(
        &self,
        keys: impl IntoIterator<Item = K>,
        consume: impl FnOnce(&mut QueryCompletionStream<'_, '_, K::Value>) -> R,
    ) -> R
    where
        C: Send + Sync + 'static,
        K: QueryKey<C>,
    {
        self.with_many_completion_with(keys, Self::get_owned::<K>, consume)
    }

    fn for_each_many_with<K, O>(
        &self,
        keys: impl IntoIterator<Item = K>,
        get: fn(&Self, K) -> O,
        mut on_complete: impl FnMut(usize, O),
    ) where
        C: Send + Sync + 'static,
        K: QueryKey<C>,
        O: Send + 'static,
    {
        self.with_many_completion_with(keys, get, |stream| {
            while let Some((position, value)) = stream.wait_next() {
                on_complete(position, value);
            }
        });
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

    fn get_many_with<K, O>(
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

    fn slot_for<K>(&self, key: &K) -> Arc<QuerySlot<K::Value>>
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

    fn cached_slot<K>(&self, key: &K) -> Option<Arc<QuerySlot<K::Value>>>
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

fn default_query_parallelism() -> usize {
    let available = std::thread::available_parallelism()
        .map(usize::from)
        .unwrap_or(1);
    available
        .div_ceil(2)
        .clamp(1, DEFAULT_MAX_QUERY_EXECUTOR_PARALLELISM)
}

fn process_query_execution_budget(parallelism: usize) -> Arc<QueryExecutionBudget> {
    static BUDGET: OnceLock<Arc<QueryExecutionBudget>> = OnceLock::new();
    Arc::clone(BUDGET.get_or_init(|| Arc::new(QueryExecutionBudget::from_environment(parallelism))))
}

impl QueryDependencyGraph {
    fn replace_dependencies_from(&mut self, from: QueryNodeId, targets: FastHashSet<QueryNodeId>) {
        self.remove_dependencies_from(from);
        if targets.is_empty() {
            return;
        }
        for target in &targets {
            self.reverse.entry(*target).or_default().insert(from);
        }
        self.forward.insert(from, targets);
    }

    fn dependencies(&self, db_id: QueryDbId, session: &QuerySession) -> Vec<QueryDependency> {
        let mut dependencies = self
            .forward
            .iter()
            .filter(|(from, _)| from.db_id == db_id)
            .flat_map(|(from, targets)| {
                targets.iter().map(move |to| QueryDependency {
                    from: session.frame(*from),
                    to: session.frame(*to),
                })
            })
            .collect::<Vec<_>>();
        dependencies.sort_by(|left, right| {
            (
                left.from.name,
                left.from.key.as_str(),
                left.to.name,
                left.to.key.as_str(),
            )
                .cmp(&(
                    right.from.name,
                    right.from.key.as_str(),
                    right.to.name,
                    right.to.key.as_str(),
                ))
        });
        dependencies
    }

    fn collect_dependents(&self, session: &QuerySession, root: QueryNodeId) -> Vec<QueryNodeId> {
        let mut seen = FastHashSet::default();
        let mut queue = vec![root];
        let mut invalidated = Vec::new();

        while let Some(identity) = queue.pop() {
            if !seen.insert(identity) {
                continue;
            }
            invalidated.push(identity);

            let mut dependents = self
                .reverse
                .get(&identity)
                .into_iter()
                .flat_map(|dependents| dependents.iter().cloned())
                .collect::<Vec<_>>();
            dependents.sort_by_key(|dependent| {
                let frame = session.frame(*dependent);
                (frame.name, frame.key)
            });
            dependents.reverse();
            queue.extend(dependents);
        }

        invalidated
    }

    fn remove_dependencies_from(&mut self, from: QueryNodeId) {
        if let Some(targets) = self.forward.remove(&from) {
            for target in targets {
                if let Some(dependents) = self.reverse.get_mut(&target) {
                    dependents.remove(&from);
                    if dependents.is_empty() {
                        self.reverse.remove(&target);
                    }
                }
            }
        }
    }

    fn remove_node(&mut self, node: QueryNodeId) {
        self.remove_dependencies_from(node);
        if let Some(dependents) = self.reverse.remove(&node) {
            for dependent in dependents {
                if let Some(targets) = self.forward.get_mut(&dependent) {
                    targets.remove(&node);
                    if targets.is_empty() {
                        self.forward.remove(&dependent);
                    }
                }
            }
        }
    }

    fn assert_only_predecessor(&self, predecessor: QueryNodeId, current: QueryNodeId) {
        let dependents = self
            .reverse
            .get(&predecessor)
            .expect("sealed predecessor must have a current dependent");
        assert_eq!(
            dependents.len(),
            1,
            "sealed predecessor must have exactly one dependent"
        );
        assert!(
            dependents.contains(&current),
            "sealed predecessor must only feed the current query"
        );
    }
}

fn query_frame<C, K>(key: &K) -> QueryFrame
where
    K: QueryKey<C>,
{
    QueryFrame {
        name: K::name(),
        key: format!("{key:?}"),
        description: key.description(),
    }
}

fn retired_query_frame(node_id: QueryNodeId) -> QueryFrame {
    let key = format!("{}:{}", node_id.db_id.0, node_id.index);
    QueryFrame {
        name: "<retired-query>",
        description: format!("retired query node {key}"),
        key,
    }
}

fn query_slot_identity<C, K>(key: Arc<K>) -> QuerySlotIdentity
where
    K: QueryKey<C>,
{
    QuerySlotIdentity {
        key,
        make_frame: query_frame_from_erased::<C, K>,
    }
}

fn ensure_query_from_erased<C, K>(db: &QueryDb<C>, key: &dyn ErasedQueryKey) -> QueryResult<()>
where
    K: QueryKey<C>,
{
    let key = key
        .as_any()
        .downcast_ref::<K>()
        .expect("query ensure identity key type mismatch");
    match K::STORAGE {
        QueryStoragePolicy::CacheOwnedArc => db.try_get(key.clone()).map(drop),
        QueryStoragePolicy::SingleConsumerOwned => db.try_get_owned(key.clone()).map(drop),
    }
}

fn query_frame_from_erased<C, K>(key: &dyn ErasedQueryKey) -> QueryFrame
where
    K: QueryKey<C>,
{
    let key = key
        .as_any()
        .downcast_ref::<K>()
        .expect("query frame identity key type mismatch");
    query_frame::<C, K>(key)
}

impl<C> ErasedQueryDatabase for QueryDbRegistration<C>
where
    C: Send + Sync + 'static,
{
    fn frame(&self, node_id: QueryNodeId) -> Option<QueryFrame> {
        let inner = self.inner.upgrade()?;
        inner
            .slots
            .lock()
            .expect("query cache slot lock poisoned")
            .get(inner.id, node_id)
            .map(|record| record.identity.frame())
    }

    fn slot(&self, node_id: QueryNodeId) -> Option<Arc<dyn ErasedQuerySlot>> {
        let inner = self.inner.upgrade()?;
        inner
            .slots
            .lock()
            .expect("query cache slot lock poisoned")
            .get(inner.id, node_id)
            .map(|record| Arc::clone(&record.slot))
    }

    fn ensure(&self, node_id: QueryNodeId) -> QueryResult<()> {
        let inner = self
            .inner
            .upgrade()
            .expect("query dependency database was dropped");
        let (key, ensure) = {
            let slots = inner.slots.lock().expect("query cache slot lock poisoned");
            let Some(record) = slots.get(inner.id, node_id) else {
                return Err(QueryError::InvalidInput {
                    query: retired_query_frame(node_id),
                    message: "query dependency was retired".into(),
                });
            };
            (Arc::clone(&record.identity.key), record.ensure)
        };
        ensure(&QueryDb { inner }, key.as_ref())
    }
}

impl<C> Clone for QueryDb<C> {
    fn clone(&self) -> Self {
        Self {
            inner: self.inner.clone(),
        }
    }
}

impl Drop for QueryStackGuard {
    fn drop(&mut self) {
        if self.active {
            QUERY_STACK.with(|stack| {
                stack.borrow_mut().pop();
            });
        }
    }
}

impl QueryStackGuard {
    fn discard(&mut self) {
        if self.active {
            QUERY_STACK.with(|stack| {
                stack.borrow_mut().pop();
            });
            self.active = false;
        }
    }

    fn take_dependencies(&mut self) -> RecordedDependencies {
        if !self.active {
            return RecordedDependencies::default();
        }
        self.active = false;
        QUERY_STACK.with(|stack| {
            stack
                .borrow_mut()
                .pop()
                .map(|entry| RecordedDependencies {
                    nodes: entry.dependencies,
                    fingerprints: entry.dependency_fingerprints,
                })
                .unwrap_or_default()
        })
    }
}

impl Drop for QueryStackInstallGuard {
    fn drop(&mut self) {
        QUERY_STACK.with(|stack| {
            *stack.borrow_mut() = std::mem::take(&mut self.previous);
        });
    }
}

fn current_query_stack() -> Vec<QueryStackEntry> {
    QUERY_STACK.with(|stack| stack.borrow().clone())
}

fn query_executor_is_active(executor: usize) -> bool {
    QUERY_EXECUTOR_STACK.with(|stack| stack.borrow().contains(&executor))
}

fn query_execution_budget_is_active(budget: usize) -> bool {
    QUERY_EXECUTION_BUDGET_STACK.with(|stack| stack.borrow().contains(&budget))
}

fn query_activity_is_active(session: usize) -> bool {
    QUERY_ACTIVITY_DEPTHS.with(|depths| {
        depths
            .borrow()
            .iter()
            .any(|(active_session, _depth)| *active_session == session)
    })
}

fn enter_query_activity(session: usize) {
    QUERY_ACTIVITY_DEPTHS.with(|depths| {
        let mut depths = depths.borrow_mut();
        if let Some((_, depth)) = depths
            .iter_mut()
            .find(|(active_session, _depth)| *active_session == session)
        {
            *depth += 1;
        } else {
            depths.push((session, 1));
        }
    });
}

fn leave_query_activity(session: usize) -> bool {
    QUERY_ACTIVITY_DEPTHS.with(|depths| {
        let mut depths = depths.borrow_mut();
        let position = depths
            .iter()
            .position(|(active_session, _depth)| *active_session == session)
            .expect("query activity guard dropped without an active depth");
        let depth = &mut depths[position].1;
        *depth = depth
            .checked_sub(1)
            .expect("query activity depth underflow");
        if *depth == 0 {
            depths.swap_remove(position);
            true
        } else {
            false
        }
    })
}

fn take_current_stack_dependencies() -> RecordedDependencies {
    QUERY_STACK.with(|stack| {
        stack
            .borrow_mut()
            .last_mut()
            .map(|entry| RecordedDependencies {
                nodes: std::mem::take(&mut entry.dependencies),
                fingerprints: entry.dependency_fingerprints.as_mut().map(std::mem::take),
            })
            .unwrap_or_default()
    })
}

fn record_dependency_on_current_stack(session_id: QuerySessionId, to: QueryNodeId) {
    QUERY_STACK.with(|stack| {
        let mut stack = stack.borrow_mut();
        let Some(from) = stack.last_mut() else {
            return;
        };
        if from.session_id == session_id {
            from.dependencies.insert(to);
            if let Some(fingerprints) = &mut from.dependency_fingerprints {
                fingerprints.entry(to).or_insert(None);
            }
        }
    });
}

fn record_dependency_fingerprint_on_current_stack(
    session_id: QuerySessionId,
    to: QueryNodeId,
    fingerprint: Option<QueryFingerprint>,
) {
    QUERY_STACK.with(|stack| {
        let mut stack = stack.borrow_mut();
        let Some(from) = stack.last_mut() else {
            return;
        };
        if from.session_id == session_id
            && let Some(fingerprints) = &mut from.dependency_fingerprints
        {
            fingerprints.insert(to, fingerprint);
        }
    });
}

fn merge_dependencies_into_current_stack(dependencies: RecordedDependencies) {
    if dependencies.nodes.is_empty() {
        return;
    }
    QUERY_STACK.with(|stack| {
        let mut stack = stack.borrow_mut();
        let Some(entry) = stack.last_mut() else {
            return;
        };
        entry.dependencies.extend(dependencies.nodes);
        if let (Some(entry_fingerprints), Some(fingerprints)) = (
            entry.dependency_fingerprints.as_mut(),
            dependencies.fingerprints,
        ) {
            entry_fingerprints.extend(fingerprints);
        }
    });
}

fn install_query_stack(stack_snapshot: Vec<QueryStackEntry>) -> QueryStackInstallGuard {
    QUERY_STACK.with(|stack| QueryStackInstallGuard {
        previous: std::mem::replace(&mut *stack.borrow_mut(), stack_snapshot),
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::{
        Barrier, Condvar,
        atomic::{AtomicUsize, Ordering},
    };

    struct TestContext {
        executions: AtomicUsize,
    }

    struct SessionInputContext {
        value: Arc<AtomicUsize>,
    }

    struct ExecutorProbeContext {
        active: AtomicUsize,
        peak_active: AtomicUsize,
        barrier: Arc<Barrier>,
    }

    struct CompletionOrderContext {
        phase: AtomicUsize,
    }

    struct BatchIsolationContext {
        session: Mutex<Option<QuerySession>>,
        child_started: Mutex<bool>,
        child_ready: Condvar,
    }

    #[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
    struct SessionInput;

    impl QueryKey<SessionInputContext> for SessionInput {
        type Value = usize;

        const FINGERPRINT: QueryFingerprintPolicy = QueryFingerprintPolicy::StableValue;

        fn name() -> &'static str {
            "session_input"
        }

        fn execute(&self, db: &QueryDb<SessionInputContext>) -> Self::Value {
            db.context().value.load(Ordering::SeqCst)
        }

        fn fingerprint(&self, value: &Self::Value) -> Option<QueryFingerprint> {
            Some(test_usize_fingerprint(
                "nia.query.test.session-input.v1",
                *value,
            ))
        }
    }

    struct SessionParentContext {
        input_db: QueryDb<SessionInputContext>,
        executions: Arc<AtomicUsize>,
    }

    struct CrossSessionBatchContext {
        input_db: QueryDb<TestContext>,
    }

    #[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
    struct SessionParent;

    impl QueryKey<SessionParentContext> for SessionParent {
        type Value = usize;

        const FINGERPRINT: QueryFingerprintPolicy = QueryFingerprintPolicy::StableValue;

        fn name() -> &'static str {
            "session_parent"
        }

        fn execute(&self, db: &QueryDb<SessionParentContext>) -> Self::Value {
            db.context().executions.fetch_add(1, Ordering::SeqCst);
            *db.context().input_db.get(SessionInput) * 2
        }

        fn fingerprint(&self, value: &Self::Value) -> Option<QueryFingerprint> {
            Some(test_usize_fingerprint(
                "nia.query.test.session-parent.v1",
                *value,
            ))
        }
    }

    #[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
    struct CrossSessionBatch;

    impl QueryKey<CrossSessionBatchContext> for CrossSessionBatch {
        type Value = usize;

        fn name() -> &'static str {
            "cross_session_batch"
        }

        fn execute(&self, db: &QueryDb<CrossSessionBatchContext>) -> Self::Value {
            db.context()
                .input_db
                .get_many([Double(2), Double(5)])
                .into_iter()
                .map(|value| *value)
                .sum()
        }
    }

    #[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
    struct Double(usize);

    impl QueryKey<TestContext> for Double {
        type Value = usize;

        fn name() -> &'static str {
            "double"
        }

        fn description(&self) -> String {
            format!("double({})", self.0)
        }

        fn execute(&self, db: &QueryDb<TestContext>) -> Self::Value {
            db.context().executions.fetch_add(1, Ordering::SeqCst);
            self.0 * 2
        }
    }

    #[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
    struct OwnedRevision(usize);

    impl QueryKey<TestContext> for OwnedRevision {
        type Value = Vec<usize>;

        fn name() -> &'static str {
            "owned_revision"
        }

        fn execute(&self, db: &QueryDb<TestContext>) -> Self::Value {
            db.context().executions.fetch_add(1, Ordering::SeqCst);
            if self.0 == 0 {
                return vec![0];
            }
            let mut value = db.get(Self(self.0 - 1)).as_ref().clone();
            value.push(self.0);
            value
        }
    }

    #[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
    struct ExecutorProbe(usize);

    impl QueryKey<ExecutorProbeContext> for ExecutorProbe {
        type Value = usize;

        fn name() -> &'static str {
            "executor_probe"
        }

        fn execute(&self, db: &QueryDb<ExecutorProbeContext>) -> Self::Value {
            let active = db.context().active.fetch_add(1, Ordering::SeqCst) + 1;
            db.context().peak_active.fetch_max(active, Ordering::SeqCst);
            db.context().barrier.wait();
            db.context().active.fetch_sub(1, Ordering::SeqCst);
            self.0
        }
    }

    #[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
    struct OwnedExecutorProbe(usize);

    impl QueryKey<ExecutorProbeContext> for OwnedExecutorProbe {
        type Value = usize;

        const STORAGE: QueryStoragePolicy = QueryStoragePolicy::SingleConsumerOwned;

        fn name() -> &'static str {
            "owned_executor_probe"
        }

        fn execute(&self, db: &QueryDb<ExecutorProbeContext>) -> Self::Value {
            let active = db.context().active.fetch_add(1, Ordering::SeqCst) + 1;
            db.context().peak_active.fetch_max(active, Ordering::SeqCst);
            db.context().barrier.wait();
            db.context().active.fetch_sub(1, Ordering::SeqCst);
            self.0
        }
    }

    #[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
    struct CompletionOrderProbe(usize);

    impl QueryKey<CompletionOrderContext> for CompletionOrderProbe {
        type Value = usize;

        const STORAGE: QueryStoragePolicy = QueryStoragePolicy::SingleConsumerOwned;

        fn name() -> &'static str {
            "completion_order_probe"
        }

        fn execute(&self, db: &QueryDb<CompletionOrderContext>) -> Self::Value {
            while db.context().phase.load(Ordering::SeqCst) != self.0 {
                std::thread::yield_now();
            }
            self.0
        }
    }

    #[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
    enum BatchIsolationQuery {
        Parent,
        OuterFiller,
        Child,
        ChildWait,
        DependsOnParent,
        OtherFiller,
    }

    impl QueryKey<BatchIsolationContext> for BatchIsolationQuery {
        type Value = usize;

        fn name() -> &'static str {
            "batch_isolation"
        }

        fn execute(&self, db: &QueryDb<BatchIsolationContext>) -> Self::Value {
            match self {
                Self::Parent => db
                    .get_many([Self::Child, Self::ChildWait])
                    .into_iter()
                    .map(|value| *value)
                    .sum(),
                Self::OuterFiller => 0,
                Self::Child => 2,
                Self::ChildWait => {
                    *db.context()
                        .child_started
                        .lock()
                        .expect("batch isolation state lock poisoned") = true;
                    db.context().child_ready.notify_all();
                    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(10);
                    loop {
                        let session = db
                            .context()
                            .session
                            .lock()
                            .expect("batch isolation session lock poisoned")
                            .clone()
                            .expect("batch isolation session must be installed");
                        let queued = session
                            .inner
                            .executor
                            .shared
                            .state
                            .lock()
                            .expect("query executor state lock poisoned")
                            .queue
                            .len();
                        if queued >= 3 {
                            break;
                        }
                        assert!(
                            std::time::Instant::now() < deadline,
                            "second batch was not submitted"
                        );
                        std::thread::yield_now();
                    }
                    1
                }
                Self::DependsOnParent => *db.get(Self::Parent),
                Self::OtherFiller => 4,
            }
        }
    }

    #[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
    struct StableInput;

    impl QueryKey<TestContext> for StableInput {
        type Value = usize;

        const FINGERPRINT: QueryFingerprintPolicy = QueryFingerprintPolicy::StableValue;

        fn name() -> &'static str {
            "stable_input"
        }

        fn execute(&self, db: &QueryDb<TestContext>) -> Self::Value {
            db.context().executions.load(Ordering::SeqCst)
        }

        fn fingerprint(&self, value: &Self::Value) -> Option<QueryFingerprint> {
            let mut builder = QueryFingerprintBuilder::new("nia.query.test.stable-input.v1");
            builder.write_u64(*value as u64);
            Some(builder.finish())
        }
    }

    #[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
    struct StableInputParent;

    impl QueryKey<TestContext> for StableInputParent {
        type Value = usize;

        fn name() -> &'static str {
            "stable_input_parent"
        }

        fn execute(&self, db: &QueryDb<TestContext>) -> Self::Value {
            *db.get(StableInput) * 2
        }
    }

    struct RedGreenContext {
        input: AtomicUsize,
        derived_executions: AtomicUsize,
        parent_executions: AtomicUsize,
    }

    #[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
    struct RedGreenInput;

    impl QueryKey<RedGreenContext> for RedGreenInput {
        type Value = usize;

        const FINGERPRINT: QueryFingerprintPolicy = QueryFingerprintPolicy::StableValue;

        fn name() -> &'static str {
            "red_green_input"
        }

        fn execute(&self, db: &QueryDb<RedGreenContext>) -> Self::Value {
            db.context().input.load(Ordering::SeqCst)
        }

        fn fingerprint(&self, value: &Self::Value) -> Option<QueryFingerprint> {
            Some(test_usize_fingerprint(
                "nia.query.test.red-green-input.v1",
                *value,
            ))
        }
    }

    #[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
    struct StableParity;

    impl QueryKey<RedGreenContext> for StableParity {
        type Value = usize;

        const FINGERPRINT: QueryFingerprintPolicy = QueryFingerprintPolicy::StableValue;

        fn name() -> &'static str {
            "stable_parity"
        }

        fn execute(&self, db: &QueryDb<RedGreenContext>) -> Self::Value {
            db.context()
                .derived_executions
                .fetch_add(1, Ordering::SeqCst);
            *db.get(RedGreenInput) % 2
        }

        fn fingerprint(&self, value: &Self::Value) -> Option<QueryFingerprint> {
            Some(test_usize_fingerprint(
                "nia.query.test.stable-parity.v1",
                *value,
            ))
        }
    }

    #[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
    struct StableParityParent;

    impl QueryKey<RedGreenContext> for StableParityParent {
        type Value = usize;

        const FINGERPRINT: QueryFingerprintPolicy = QueryFingerprintPolicy::StableValue;

        fn name() -> &'static str {
            "stable_parity_parent"
        }

        fn execute(&self, db: &QueryDb<RedGreenContext>) -> Self::Value {
            db.context()
                .parent_executions
                .fetch_add(1, Ordering::SeqCst);
            *db.get(StableParity) + 10
        }

        fn fingerprint(&self, value: &Self::Value) -> Option<QueryFingerprint> {
            Some(test_usize_fingerprint(
                "nia.query.test.stable-parity-parent.v1",
                *value,
            ))
        }
    }

    #[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
    struct SemanticParity;

    impl QueryKey<RedGreenContext> for SemanticParity {
        type Value = usize;

        const FINGERPRINT: QueryFingerprintPolicy = QueryFingerprintPolicy::SemanticValue;

        fn name() -> &'static str {
            "semantic_parity"
        }

        fn execute(&self, db: &QueryDb<RedGreenContext>) -> Self::Value {
            db.context()
                .derived_executions
                .fetch_add(1, Ordering::SeqCst);
            *db.get(RedGreenInput) % 2
        }

        fn values_equal(&self, old: &Self::Value, new: &Self::Value) -> bool {
            old == new
        }
    }

    #[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
    struct SemanticParityParent;

    impl QueryKey<RedGreenContext> for SemanticParityParent {
        type Value = usize;

        const FINGERPRINT: QueryFingerprintPolicy = QueryFingerprintPolicy::StableValue;

        fn name() -> &'static str {
            "semantic_parity_parent"
        }

        fn execute(&self, db: &QueryDb<RedGreenContext>) -> Self::Value {
            db.context()
                .parent_executions
                .fetch_add(1, Ordering::SeqCst);
            *db.get(SemanticParity) + 10
        }

        fn fingerprint(&self, value: &Self::Value) -> Option<QueryFingerprint> {
            Some(test_usize_fingerprint(
                "nia.query.test.semantic-parity-parent.v1",
                *value,
            ))
        }
    }

    #[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
    struct StableModulo(usize);

    impl QueryKey<RedGreenContext> for StableModulo {
        type Value = usize;

        const FINGERPRINT: QueryFingerprintPolicy = QueryFingerprintPolicy::StableValue;

        fn name() -> &'static str {
            "stable_modulo"
        }

        fn execute(&self, db: &QueryDb<RedGreenContext>) -> Self::Value {
            db.context()
                .derived_executions
                .fetch_add(1, Ordering::SeqCst);
            *db.get(RedGreenInput) % self.0
        }

        fn fingerprint(&self, value: &Self::Value) -> Option<QueryFingerprint> {
            Some(test_usize_fingerprint(
                "nia.query.test.stable-modulo.v1",
                *value,
            ))
        }
    }

    #[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
    struct StableModuloBatchParent;

    impl QueryKey<RedGreenContext> for StableModuloBatchParent {
        type Value = usize;

        const FINGERPRINT: QueryFingerprintPolicy = QueryFingerprintPolicy::StableValue;

        fn name() -> &'static str {
            "stable_modulo_batch_parent"
        }

        fn execute(&self, db: &QueryDb<RedGreenContext>) -> Self::Value {
            db.context()
                .parent_executions
                .fetch_add(1, Ordering::SeqCst);
            db.get_many([StableModulo(2), StableModulo(3)])
                .into_iter()
                .map(|value| *value)
                .sum()
        }

        fn fingerprint(&self, value: &Self::Value) -> Option<QueryFingerprint> {
            Some(test_usize_fingerprint(
                "nia.query.test.stable-modulo-batch-parent.v1",
                *value,
            ))
        }
    }

    struct ValidationRaceContext {
        input: AtomicUsize,
        input_executions: AtomicUsize,
        derived_executions: AtomicUsize,
        control: Arc<(Mutex<ValidationRaceState>, Condvar)>,
    }

    #[derive(Default)]
    struct ValidationRaceState {
        started: bool,
        release: bool,
    }

    #[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
    struct ValidationRaceInput;

    impl QueryKey<ValidationRaceContext> for ValidationRaceInput {
        type Value = usize;

        const FINGERPRINT: QueryFingerprintPolicy = QueryFingerprintPolicy::StableValue;

        fn name() -> &'static str {
            "validation_race_input"
        }

        fn execute(&self, db: &QueryDb<ValidationRaceContext>) -> Self::Value {
            let execution = db.context().input_executions.fetch_add(1, Ordering::SeqCst);
            if execution > 0 {
                let (lock, ready) = &*db.context().control;
                let mut state = lock.lock().expect("validation race lock poisoned");
                state.started = true;
                ready.notify_all();
                while !state.release {
                    state = ready.wait(state).expect("validation race lock poisoned");
                }
            }
            db.context().input.load(Ordering::SeqCst)
        }

        fn fingerprint(&self, value: &Self::Value) -> Option<QueryFingerprint> {
            Some(test_usize_fingerprint(
                "nia.query.test.validation-race-input.v1",
                *value,
            ))
        }
    }

    #[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
    struct ValidationRaceDerived;

    impl QueryKey<ValidationRaceContext> for ValidationRaceDerived {
        type Value = usize;

        const FINGERPRINT: QueryFingerprintPolicy = QueryFingerprintPolicy::StableValue;

        fn name() -> &'static str {
            "validation_race_derived"
        }

        fn execute(&self, db: &QueryDb<ValidationRaceContext>) -> Self::Value {
            db.context()
                .derived_executions
                .fetch_add(1, Ordering::SeqCst);
            *db.get(ValidationRaceInput) % 2
        }

        fn fingerprint(&self, value: &Self::Value) -> Option<QueryFingerprint> {
            Some(test_usize_fingerprint(
                "nia.query.test.validation-race-derived.v1",
                *value,
            ))
        }
    }

    fn test_usize_fingerprint(domain: &str, value: usize) -> QueryFingerprint {
        let mut builder = QueryFingerprintBuilder::new(domain);
        builder.write_u64(value as u64);
        builder.finish()
    }

    #[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
    struct DuplicateDoubleName;

    impl QueryKey<TestContext> for DuplicateDoubleName {
        type Value = usize;

        fn name() -> &'static str {
            "double"
        }

        fn execute(&self, _db: &QueryDb<TestContext>) -> Self::Value {
            0
        }
    }

    struct NonCloneValue {
        value: usize,
    }

    #[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
    struct NonCloneValueQuery;

    impl QueryKey<TestContext> for NonCloneValueQuery {
        type Value = NonCloneValue;

        fn name() -> &'static str {
            "non_clone_value"
        }

        fn execute(&self, db: &QueryDb<TestContext>) -> Self::Value {
            db.context().executions.fetch_add(1, Ordering::SeqCst);
            NonCloneValue { value: 42 }
        }
    }

    struct OwnedNonCloneValue {
        value: usize,
    }

    #[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
    struct OwnedNonCloneValueQuery(usize);

    impl QueryKey<TestContext> for OwnedNonCloneValueQuery {
        type Value = OwnedNonCloneValue;

        const STORAGE: QueryStoragePolicy = QueryStoragePolicy::SingleConsumerOwned;

        fn name() -> &'static str {
            "owned_non_clone_value"
        }

        fn execute(&self, db: &QueryDb<TestContext>) -> Self::Value {
            db.context().executions.fetch_add(1, Ordering::SeqCst);
            OwnedNonCloneValue { value: self.0 }
        }
    }

    #[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
    struct OwnedValueBatchParent;

    impl QueryKey<TestContext> for OwnedValueBatchParent {
        type Value = usize;

        fn name() -> &'static str {
            "owned_value_batch_parent"
        }

        fn execute(&self, db: &QueryDb<TestContext>) -> Self::Value {
            db.get_many_owned([
                OwnedNonCloneValueQuery(2),
                OwnedNonCloneValueQuery(5),
                OwnedNonCloneValueQuery(3),
            ])
            .into_iter()
            .map(|value| value.value)
            .sum()
        }
    }

    #[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
    struct OwnedValueCompletionParent;

    impl QueryKey<TestContext> for OwnedValueCompletionParent {
        type Value = usize;

        fn name() -> &'static str {
            "owned_value_completion_parent"
        }

        fn execute(&self, db: &QueryDb<TestContext>) -> Self::Value {
            let mut sum = 0;
            db.for_each_many_owned(
                [
                    OwnedNonCloneValueQuery(2),
                    OwnedNonCloneValueQuery(5),
                    OwnedNonCloneValueQuery(3),
                ],
                |_position, value| sum += value.value,
            );
            sum
        }
    }

    struct PublishedOwnedValue {
        value: usize,
        drops: Arc<AtomicUsize>,
    }

    impl Drop for PublishedOwnedValue {
        fn drop(&mut self) {
            self.drops.fetch_add(1, Ordering::SeqCst);
        }
    }

    #[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
    struct PublishedOwnedValueQuery(usize);

    impl QueryKey<TestContext> for PublishedOwnedValueQuery {
        type Value = PublishedOwnedValue;

        const STORAGE: QueryStoragePolicy = QueryStoragePolicy::SingleConsumerOwned;
        const PROVIDER: QueryProviderPolicy = QueryProviderPolicy::ExternallyPublished;

        fn name() -> &'static str {
            "published_owned_value"
        }

        fn execute(&self, _db: &QueryDb<TestContext>) -> Self::Value {
            unreachable!("externally published queries do not execute their key provider")
        }
    }

    #[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
    struct OwnedValueParent(usize);

    impl QueryKey<TestContext> for OwnedValueParent {
        type Value = usize;

        fn name() -> &'static str {
            "owned_value_parent"
        }

        fn execute(&self, db: &QueryDb<TestContext>) -> Self::Value {
            db.get_owned(OwnedNonCloneValueQuery(self.0)).value * 2
        }
    }

    #[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
    struct Recursive;

    impl QueryKey<TestContext> for Recursive {
        type Value = usize;

        fn name() -> &'static str {
            "recursive"
        }

        fn execute(&self, db: &QueryDb<TestContext>) -> Self::Value {
            *db.get(Recursive)
        }
    }

    #[test]
    fn memoizes_query_values() {
        let db = QueryDb::new(TestContext {
            executions: AtomicUsize::new(0),
        });

        assert_eq!(*db.get(Double(21)), 42);
        assert_eq!(*db.get(Double(21)), 42);
        assert_eq!(db.context().executions.load(Ordering::SeqCst), 1);
    }

    #[test]
    fn get_reuses_cached_value_handles() {
        let db = QueryDb::new(TestContext {
            executions: AtomicUsize::new(0),
        });

        let first = db.get(Double(21));
        let second = db.get(Double(21));

        assert!(Arc::ptr_eq(&first, &second));
        assert_eq!(*first, 42);
        assert_eq!(*db.get(Double(21)), 42);
        assert_eq!(db.context().executions.load(Ordering::SeqCst), 1);
    }

    #[test]
    fn get_supports_non_clone_query_values() {
        let db = QueryDb::new(TestContext {
            executions: AtomicUsize::new(0),
        });

        let first = db.get(NonCloneValueQuery);
        let second = db.get(NonCloneValueQuery);

        assert!(Arc::ptr_eq(&first, &second));
        assert_eq!(first.value, 42);
        assert_eq!(db.context().executions.load(Ordering::SeqCst), 1);
    }

    #[test]
    fn single_consumer_query_moves_non_clone_value_and_tracks_parent_dependency() {
        let db = QueryDb::new(TestContext {
            executions: AtomicUsize::new(0),
        });

        assert_eq!(*db.get(OwnedValueParent(21)), 42);
        assert_eq!(*db.get(OwnedValueParent(21)), 42);
        assert_eq!(db.context().executions.load(Ordering::SeqCst), 1);
        assert!(db.query_trace().dependencies.iter().any(|dependency| {
            dependency.from.name == "owned_value_parent"
                && dependency.to.name == "owned_non_clone_value"
        }));

        let invalidation = db.invalidate(OwnedNonCloneValueQuery(21));
        assert!(
            invalidation
                .invalidated
                .iter()
                .any(|frame| frame.name == "owned_value_parent")
        );
        assert_eq!(*db.get(OwnedValueParent(21)), 42);
        assert_eq!(db.context().executions.load(Ordering::SeqCst), 2);
    }

    #[test]
    fn single_consumer_query_reproduces_after_its_payload_is_consumed() {
        let db = QueryDb::new(TestContext {
            executions: AtomicUsize::new(0),
        });

        assert_eq!(db.get_owned(OwnedNonCloneValueQuery(3)).value, 3);
        assert_eq!(db.get_owned(OwnedNonCloneValueQuery(3)).value, 3);
        assert_eq!(db.context().executions.load(Ordering::SeqCst), 2);
    }

    #[test]
    fn externally_published_owned_query_moves_once_and_tracks_its_predecessor() {
        let db = QueryDb::new(TestContext {
            executions: AtomicUsize::new(0),
        });
        let predecessor = OwnedNonCloneValueQuery(3);
        assert_eq!(db.get_owned(predecessor).value, 3);
        let drops = Arc::new(AtomicUsize::new(0));

        db.publish_owned(
            PublishedOwnedValueQuery(3),
            PublishedOwnedValue {
                value: 9,
                drops: Arc::clone(&drops),
            },
            &predecessor,
        );
        let value = db.get_owned(PublishedOwnedValueQuery(3));
        assert_eq!(value.value, 9);
        assert_eq!(drops.load(Ordering::SeqCst), 0);
        drop(value);
        assert_eq!(drops.load(Ordering::SeqCst), 1);
        assert!(db.try_get_owned(PublishedOwnedValueQuery(3)).is_err());
        assert!(db.query_trace().dependencies.iter().any(|dependency| {
            dependency.from.name == "published_owned_value"
                && dependency.to.name == "owned_non_clone_value"
        }));

        let invalidation = db.invalidate(predecessor);
        assert!(
            invalidation
                .invalidated
                .iter()
                .any(|frame| { frame.name == "published_owned_value" })
        );
    }

    #[test]
    fn invalidating_a_producer_drops_an_unconsumed_published_payload() {
        let db = QueryDb::new(TestContext {
            executions: AtomicUsize::new(0),
        });
        let predecessor = OwnedNonCloneValueQuery(5);
        assert_eq!(db.get_owned(predecessor).value, 5);
        let drops = Arc::new(AtomicUsize::new(0));
        db.publish_owned(
            PublishedOwnedValueQuery(5),
            PublishedOwnedValue {
                value: 25,
                drops: Arc::clone(&drops),
            },
            &predecessor,
        );

        db.invalidate(predecessor);
        assert_eq!(drops.load(Ordering::SeqCst), 1);
        assert!(db.try_get_owned(PublishedOwnedValueQuery(5)).is_err());
    }

    #[test]
    fn query_storage_policy_rejects_the_wrong_access_mode() {
        let db = QueryDb::new(TestContext {
            executions: AtomicUsize::new(0),
        });

        assert!(catch_unwind(AssertUnwindSafe(|| db.get(OwnedNonCloneValueQuery(1)))).is_err());
        assert!(catch_unwind(AssertUnwindSafe(|| db.get_owned(Double(1)))).is_err());
        assert_eq!(db.context().executions.load(Ordering::SeqCst), 0);
    }

    #[test]
    fn declarative_registry_records_single_consumer_storage() {
        let mut registry = QueryRegistry::new();
        registry.register::<TestContext, OwnedNonCloneValueQuery>();

        let descriptors = registry.descriptors();
        assert_eq!(descriptors.len(), 1);
        assert_eq!(
            descriptors[0].storage,
            QueryStoragePolicy::SingleConsumerOwned
        );
    }

    #[test]
    fn declarative_registry_records_an_external_owned_producer() {
        let mut registry = QueryRegistry::new();
        registry.register::<TestContext, PublishedOwnedValueQuery>();

        let descriptors = registry.descriptors();
        assert_eq!(
            descriptors[0].provider,
            QueryProviderPolicy::ExternallyPublished
        );
        assert_eq!(
            descriptors[0].storage,
            QueryStoragePolicy::SingleConsumerOwned
        );
    }

    #[test]
    fn declarative_registry_records_and_enforces_query_contracts() {
        let mut registry = QueryRegistry::new();
        registry.register::<TestContext, Double>();
        let db = QueryDb::new_registered(
            TestContext {
                executions: AtomicUsize::new(0),
            },
            registry,
        );

        assert_eq!(*db.get(Double(21)), 42);
        let descriptors = db.registered_queries();
        assert_eq!(descriptors.len(), 1);
        assert_eq!(descriptors[0].name, "double");
        assert_eq!(descriptors[0].key_type, std::any::type_name::<Double>());
        assert_eq!(descriptors[0].value_type, std::any::type_name::<usize>());
        assert_eq!(descriptors[0].provider, QueryProviderPolicy::KeyExecute);
        assert_eq!(descriptors[0].fingerprint, QueryFingerprintPolicy::None);
        assert_eq!(descriptors[0].storage, QueryStoragePolicy::CacheOwnedArc);

        let missing = std::panic::catch_unwind(|| db.get(NonCloneValueQuery));
        assert!(missing.is_err());
    }

    #[test]
    fn fingerprint_builder_is_deterministic_and_domain_separated() {
        let fingerprint = |domain| {
            let mut builder = QueryFingerprintBuilder::new(domain);
            builder.write_u8(7);
            builder.write_u64(42);
            builder.write_str("nia");
            builder.finish()
        };

        assert_eq!(fingerprint("query-a.v1"), fingerprint("query-a.v1"));
        assert_ne!(fingerprint("query-a.v1"), fingerprint("query-b.v1"));
        assert_eq!(std::mem::size_of::<QueryFingerprint>(), 16);
    }

    #[test]
    fn declarative_registry_records_stable_value_fingerprints() {
        let mut registry = QueryRegistry::new();
        registry.register::<TestContext, StableInput>();

        assert_eq!(
            registry.descriptors()[0].fingerprint,
            QueryFingerprintPolicy::StableValue
        );
    }

    #[test]
    #[should_panic(expected = "is already registered")]
    fn declarative_registry_rejects_duplicate_key_types() {
        let mut registry = QueryRegistry::new();
        registry.register::<TestContext, Double>();
        registry.register::<TestContext, Double>();
    }

    #[test]
    #[should_panic(expected = "query name `double` is already registered")]
    fn declarative_registry_rejects_duplicate_names() {
        let mut registry = QueryRegistry::new();
        registry.register::<TestContext, Double>();
        registry.register::<TestContext, DuplicateDoubleName>();
    }

    #[test]
    fn query_node_ids_are_word_sized_and_database_scoped() {
        assert_eq!(std::mem::size_of::<QueryNodeId>(), 8);
        let first = QueryDb::new(TestContext {
            executions: AtomicUsize::new(0),
        });
        let second = QueryDb::new(TestContext {
            executions: AtomicUsize::new(0),
        });

        let first_id = first.slot_for(&Double(1)).node_id;
        let second_id = second.slot_for(&Double(1)).node_id;

        assert_ne!(first_id, second_id);
        assert_eq!(first_id.index, second_id.index);
        assert_ne!(first_id.db_id, second_id.db_id);
    }

    #[test]
    fn shared_session_records_and_invalidates_cross_database_dependencies() {
        let session = QuerySession::new();
        let value = Arc::new(AtomicUsize::new(3));
        let input_db = QueryDb::new_with_timings_in_session(
            SessionInputContext {
                value: Arc::clone(&value),
            },
            nia_timing::TimingMode::Off,
            session.clone(),
        );
        let executions = Arc::new(AtomicUsize::new(0));
        let parent_db = QueryDb::new_with_timings_in_session(
            SessionParentContext {
                input_db: input_db.clone(),
                executions: Arc::clone(&executions),
            },
            nia_timing::TimingMode::Off,
            session,
        );

        assert!(parent_db.session().ptr_eq(&input_db.session()));
        assert_eq!(*parent_db.get(SessionParent), 6);
        value.store(4, Ordering::SeqCst);
        let invalidation = input_db.invalidate(SessionInput);

        assert!(
            invalidation
                .invalidated
                .iter()
                .any(|frame| frame.name == "session_parent")
        );
        assert_eq!(*parent_db.get(SessionParent), 8);
        assert_eq!(executions.load(Ordering::SeqCst), 2);
        assert!(
            parent_db
                .query_trace()
                .dependencies
                .iter()
                .any(|dependency| {
                    dependency.from.name == "session_parent"
                        && dependency.to.name == "session_input"
                })
        );
    }

    #[test]
    fn separate_sessions_do_not_record_cross_database_dependencies() {
        let value = Arc::new(AtomicUsize::new(3));
        let input_db = QueryDb::new(SessionInputContext {
            value: Arc::clone(&value),
        });
        let executions = Arc::new(AtomicUsize::new(0));
        let parent_db = QueryDb::new(SessionParentContext {
            input_db: input_db.clone(),
            executions: Arc::clone(&executions),
        });

        assert!(!parent_db.session().ptr_eq(&input_db.session()));
        assert_eq!(*parent_db.get(SessionParent), 6);
        value.store(4, Ordering::SeqCst);
        let invalidation = input_db.invalidate(SessionInput);

        assert!(
            invalidation
                .invalidated
                .iter()
                .all(|frame| frame.name != "session_parent")
        );
        assert_eq!(*parent_db.get(SessionParent), 6);
        assert_eq!(executions.load(Ordering::SeqCst), 1);
    }

    #[test]
    fn executes_get_many_in_key_order() {
        let db = QueryDb::new(TestContext {
            executions: AtomicUsize::new(0),
        });

        let values = db.get_many([Double(1), Double(4), Double(3)]);

        assert_eq!(
            values.iter().map(|value| **value).collect::<Vec<_>>(),
            vec![2, 8, 6]
        );
        assert_eq!(db.context().executions.load(Ordering::SeqCst), 3);
    }

    #[test]
    fn get_many_reuses_non_clone_cached_handles_in_key_order() {
        let db = QueryDb::new(TestContext {
            executions: AtomicUsize::new(0),
        });

        let values = db.get_many([NonCloneValueQuery, NonCloneValueQuery]);

        assert_eq!(values.len(), 2);
        assert!(Arc::ptr_eq(&values[0], &values[1]));
        assert_eq!(values[0].value, 42);
        assert_eq!(db.context().executions.load(Ordering::SeqCst), 1);
    }

    #[test]
    fn get_many_owned_moves_non_clone_values_in_key_order() {
        let db = QueryDb::new(TestContext {
            executions: AtomicUsize::new(0),
        });

        let values = db.get_many_owned([
            OwnedNonCloneValueQuery(4),
            OwnedNonCloneValueQuery(1),
            OwnedNonCloneValueQuery(3),
        ]);

        assert_eq!(
            values
                .into_iter()
                .map(|value| value.value)
                .collect::<Vec<_>>(),
            vec![4, 1, 3]
        );
        assert_eq!(db.context().executions.load(Ordering::SeqCst), 3);
    }

    #[test]
    fn for_each_many_owned_moves_values_in_completion_order() {
        let session = QuerySession::with_parallelism(3);
        let db = QueryDb::new_with_timings_in_session(
            CompletionOrderContext {
                phase: AtomicUsize::new(0),
            },
            nia_timing::TimingMode::Off,
            session,
        );
        let mut completed = Vec::new();

        db.for_each_many_owned(
            [
                CompletionOrderProbe(2),
                CompletionOrderProbe(1),
                CompletionOrderProbe(0),
            ],
            |position, value| {
                completed.push((position, value));
                db.context().phase.store(value + 1, Ordering::SeqCst);
            },
        );

        assert_eq!(completed, vec![(2, 0), (1, 1), (0, 2)]);
    }

    #[test]
    fn typed_owned_completion_stream_moves_values_in_completion_order() {
        let session = QuerySession::with_parallelism(3);
        let db = QueryDb::new_with_timings_in_session(
            CompletionOrderContext {
                phase: AtomicUsize::new(0),
            },
            nia_timing::TimingMode::Off,
            session,
        );

        let completed = db.with_many_owned_completion(
            [
                CompletionOrderProbe(2),
                CompletionOrderProbe(1),
                CompletionOrderProbe(0),
            ],
            |stream| {
                let mut completed = Vec::new();
                while let Some((position, value)) = stream.wait_next() {
                    completed.push((position, value));
                    db.context().phase.store(value + 1, Ordering::SeqCst);
                }
                completed
            },
        );

        assert_eq!(completed, vec![(2, 0), (1, 1), (0, 2)]);
    }

    #[test]
    fn get_many_owned_records_dependencies_from_parent_query() {
        let db = QueryDb::new(TestContext {
            executions: AtomicUsize::new(0),
        });

        assert_eq!(*db.get(OwnedValueBatchParent), 10);
        let invalidation = db.invalidate(OwnedNonCloneValueQuery(5));

        assert!(
            invalidation
                .invalidated
                .iter()
                .any(|frame| frame.name == "owned_value_batch_parent")
        );
    }

    #[test]
    fn for_each_many_owned_records_dependencies_from_parent_query() {
        let db = QueryDb::new(TestContext {
            executions: AtomicUsize::new(0),
        });

        assert_eq!(*db.get(OwnedValueCompletionParent), 10);
        let invalidation = db.invalidate(OwnedNonCloneValueQuery(5));

        assert!(
            invalidation
                .invalidated
                .iter()
                .any(|frame| frame.name == "owned_value_completion_parent")
        );
    }

    #[test]
    fn get_many_owned_uses_the_session_executor_budget() {
        let session = QuerySession::with_parallelism(2);
        let db = QueryDb::new_with_timings_in_session(
            ExecutorProbeContext {
                active: AtomicUsize::new(0),
                peak_active: AtomicUsize::new(0),
                barrier: Arc::new(Barrier::new(2)),
            },
            nia_timing::TimingMode::Off,
            session.clone(),
        );

        let values = db.get_many_owned([
            OwnedExecutorProbe(0),
            OwnedExecutorProbe(1),
            OwnedExecutorProbe(2),
            OwnedExecutorProbe(3),
        ]);

        assert_eq!(values, vec![0, 1, 2, 3]);
        assert_eq!(db.context().active.load(Ordering::SeqCst), 0);
        assert_eq!(db.context().peak_active.load(Ordering::SeqCst), 2);
        assert_eq!(session.inner.executor.peak_active(), 2);
    }

    #[test]
    fn session_tasks_move_non_clone_outputs_in_submission_order() {
        let session = QuerySession::with_parallelism(2);

        let values = session.run_tasks((0..4).map(|value| move || OwnedNonCloneValue { value }));

        assert_eq!(
            values
                .into_iter()
                .map(|value| value.value)
                .collect::<Vec<_>>(),
            vec![0, 1, 2, 3]
        );
    }

    #[test]
    fn session_tasks_use_the_shared_executor_budget() {
        let session = QuerySession::with_parallelism(2);
        let active = Arc::new(AtomicUsize::new(0));
        let peak_active = Arc::new(AtomicUsize::new(0));
        let barrier = Arc::new(Barrier::new(2));
        let tasks = (0..4).map(|value| {
            let active = Arc::clone(&active);
            let peak_active = Arc::clone(&peak_active);
            let barrier = Arc::clone(&barrier);
            move || {
                let current = active.fetch_add(1, Ordering::SeqCst) + 1;
                peak_active.fetch_max(current, Ordering::SeqCst);
                barrier.wait();
                active.fetch_sub(1, Ordering::SeqCst);
                value
            }
        });

        assert_eq!(session.run_tasks(tasks), vec![0, 1, 2, 3]);
        assert_eq!(active.load(Ordering::SeqCst), 0);
        assert_eq!(peak_active.load(Ordering::SeqCst), 2);
        assert_eq!(session.inner.executor.peak_active(), 2);
    }

    #[test]
    fn bounded_session_tasks_preserve_order_and_limit_worker_lanes() {
        let session = QuerySession::with_parallelism(4);
        let active = Arc::new(AtomicUsize::new(0));
        let peak_active = Arc::new(AtomicUsize::new(0));
        let barrier = Arc::new(Barrier::new(2));
        let tasks = (0..6).map(|value| {
            let active = Arc::clone(&active);
            let peak_active = Arc::clone(&peak_active);
            let barrier = Arc::clone(&barrier);
            move || {
                let current = active.fetch_add(1, Ordering::SeqCst) + 1;
                peak_active.fetch_max(current, Ordering::SeqCst);
                barrier.wait();
                active.fetch_sub(1, Ordering::SeqCst);
                OwnedNonCloneValue { value }
            }
        });

        let values = session.run_tasks_bounded(tasks, 2);

        assert_eq!(
            values
                .into_iter()
                .map(|value| value.value)
                .collect::<Vec<_>>(),
            vec![0, 1, 2, 3, 4, 5]
        );
        assert_eq!(active.load(Ordering::SeqCst), 0);
        assert_eq!(peak_active.load(Ordering::SeqCst), 2);
        assert_eq!(session.inner.executor.peak_active(), 2);
    }

    #[test]
    fn bounded_priority_task_pool_preserves_submission_order_and_lanes() {
        let session = QuerySession::with_parallelism(4);
        let active = Arc::new(AtomicUsize::new(0));
        let peak_active = Arc::new(AtomicUsize::new(0));
        let barrier = Arc::new(Barrier::new(2));
        let mut pool = session.task_pool(2);

        for value in 0..4 {
            let active = Arc::clone(&active);
            let peak_active = Arc::clone(&peak_active);
            let barrier = Arc::clone(&barrier);
            pool.submit(move || {
                let current = active.fetch_add(1, Ordering::SeqCst) + 1;
                peak_active.fetch_max(current, Ordering::SeqCst);
                barrier.wait();
                active.fetch_sub(1, Ordering::SeqCst);
                OwnedNonCloneValue { value }
            });
        }

        let values = pool.finish();

        assert_eq!(
            values
                .into_iter()
                .map(|value| value.value)
                .collect::<Vec<_>>(),
            vec![0, 1, 2, 3]
        );
        assert_eq!(active.load(Ordering::SeqCst), 0);
        assert_eq!(peak_active.load(Ordering::SeqCst), 2);
    }

    #[test]
    fn priority_task_pool_runs_before_queued_batch_work() {
        let session = QuerySession::with_parallelism(2);
        let executor = &session.inner.executor;
        let normal_batch = Arc::new(QueryBatch::new(2));
        let normal_batch_id = Arc::as_ptr(&normal_batch) as usize;
        let order = Arc::new(Mutex::new(Vec::new()));
        let (started_sender, started_receiver) = std::sync::mpsc::channel();
        let (release_sender, release_receiver) = std::sync::mpsc::channel();
        let (normal_sender, normal_receiver) = std::sync::mpsc::channel();
        let blocker_batch = Arc::clone(&normal_batch);
        let blocker_shared = Arc::clone(&executor.shared);
        let normal_order = Arc::clone(&order);
        let normal_task_batch = Arc::clone(&normal_batch);
        let normal_shared = Arc::clone(&executor.shared);

        executor.submit_all(vec![
            QueryTask {
                batch: normal_batch_id,
                run: Box::new(move || {
                    started_sender.send(()).expect("signal blocker start");
                    release_receiver.recv().expect("release blocker");
                    blocker_batch.complete(0, Ok(()));
                    blocker_shared.notify_waiters();
                }),
            },
            QueryTask {
                batch: normal_batch_id,
                run: Box::new(move || {
                    normal_order
                        .lock()
                        .expect("task order lock poisoned")
                        .push("normal");
                    normal_task_batch.complete(1, Ok(()));
                    normal_shared.notify_waiters();
                    normal_sender.send(()).expect("signal normal completion");
                }),
            },
        ]);
        started_receiver.recv().expect("wait for blocker start");

        let priority_order = Arc::clone(&order);
        let mut pool = session.task_pool(1);
        pool.submit(move || {
            priority_order
                .lock()
                .expect("task order lock poisoned")
                .push("priority");
        });
        release_sender.send(()).expect("release executor worker");
        normal_receiver
            .recv_timeout(std::time::Duration::from_secs(2))
            .expect("queued normal task completion");

        assert_eq!(pool.finish(), vec![()]);
        assert_eq!(normal_batch.finish(), vec![(), ()]);
        assert_eq!(
            *order.lock().expect("task order lock poisoned"),
            vec!["priority", "normal"]
        );
    }

    #[test]
    fn priority_task_pool_drains_after_task_panic() {
        let session = QuerySession::with_parallelism(2);
        let completed = Arc::new(AtomicUsize::new(0));
        let mut pool = session.task_pool(2);
        pool.submit(|| -> usize { panic!("priority task failure") });
        let task_completed = Arc::clone(&completed);
        pool.submit(move || {
            task_completed.fetch_add(1, Ordering::SeqCst);
            7
        });

        let result = catch_unwind(AssertUnwindSafe(|| pool.finish()));

        assert!(result.is_err());
        assert_eq!(completed.load(Ordering::SeqCst), 1);
        assert_eq!(session.run_tasks([|| 9]), vec![9]);
    }

    #[test]
    fn session_executor_caps_concurrent_batch_tasks() {
        let session = QuerySession::with_parallelism(2);
        let db = QueryDb::new_with_timings_in_session(
            ExecutorProbeContext {
                active: AtomicUsize::new(0),
                peak_active: AtomicUsize::new(0),
                barrier: Arc::new(Barrier::new(2)),
            },
            nia_timing::TimingMode::Off,
            session.clone(),
        );

        let values = db.get_many([
            ExecutorProbe(0),
            ExecutorProbe(1),
            ExecutorProbe(2),
            ExecutorProbe(3),
        ]);

        assert_eq!(
            values.iter().map(|value| **value).collect::<Vec<_>>(),
            vec![0, 1, 2, 3]
        );
        assert_eq!(db.context().active.load(Ordering::SeqCst), 0);
        assert_eq!(db.context().peak_active.load(Ordering::SeqCst), 2);
        assert_eq!(session.inner.executor.peak_active(), 2);
    }

    #[test]
    fn shared_execution_budget_caps_tasks_across_sessions() {
        let execution_budget = Arc::new(QueryExecutionBudget::owned(2));
        let first_session = QuerySession::with_execution_budget(2, Arc::clone(&execution_budget));
        let second_session = QuerySession::with_execution_budget(2, Arc::clone(&execution_budget));
        let barrier = Arc::new(Barrier::new(2));
        let first_db = QueryDb::new_with_timings_in_session(
            ExecutorProbeContext {
                active: AtomicUsize::new(0),
                peak_active: AtomicUsize::new(0),
                barrier: Arc::clone(&barrier),
            },
            nia_timing::TimingMode::Off,
            first_session,
        );
        let second_db = QueryDb::new_with_timings_in_session(
            ExecutorProbeContext {
                active: AtomicUsize::new(0),
                peak_active: AtomicUsize::new(0),
                barrier,
            },
            nia_timing::TimingMode::Off,
            second_session,
        );
        let (sender, receiver) = std::sync::mpsc::channel();
        let second_sender = sender.clone();
        std::thread::spawn(move || {
            let values = first_db.get_many([ExecutorProbe(0), ExecutorProbe(1)]);
            sender
                .send(values.into_iter().map(|value| *value).collect::<Vec<_>>())
                .expect("send first shared-budget batch");
        });
        std::thread::spawn(move || {
            let values = second_db.get_many([ExecutorProbe(2), ExecutorProbe(3)]);
            second_sender
                .send(values.into_iter().map(|value| *value).collect::<Vec<_>>())
                .expect("send second shared-budget batch");
        });

        let mut batches = vec![
            receiver
                .recv_timeout(std::time::Duration::from_secs(10))
                .expect("first shared-budget batch must complete"),
            receiver
                .recv_timeout(std::time::Duration::from_secs(10))
                .expect("second shared-budget batch must complete"),
        ];
        batches.sort();
        assert_eq!(batches, vec![vec![0, 1], vec![2, 3]]);
        assert_eq!(execution_budget.peak_active(), 2);
    }

    #[test]
    fn default_sessions_share_the_process_execution_budget() {
        let first = QuerySession::new();
        let second = QuerySession::new();

        assert!(!first.ptr_eq(&second));
        assert!(Arc::ptr_eq(
            &first.inner.executor.execution_budget,
            &second.inner.executor.execution_budget
        ));
    }

    #[test]
    fn nested_batches_across_sessions_reuse_the_current_process_permit() {
        let execution_budget = Arc::new(QueryExecutionBudget::owned(1));
        let input_session = QuerySession::with_execution_budget(1, Arc::clone(&execution_budget));
        let parent_session = QuerySession::with_execution_budget(1, Arc::clone(&execution_budget));
        let input_db = QueryDb::new_with_timings_in_session(
            TestContext {
                executions: AtomicUsize::new(0),
            },
            nia_timing::TimingMode::Off,
            input_session,
        );
        let parent_db = QueryDb::new_with_timings_in_session(
            CrossSessionBatchContext { input_db },
            nia_timing::TimingMode::Off,
            parent_session,
        );
        let (sender, receiver) = std::sync::mpsc::channel();

        std::thread::spawn(move || {
            let values = parent_db.get_many([CrossSessionBatch]);
            sender
                .send(values.into_iter().map(|value| *value).collect::<Vec<_>>())
                .expect("send cross-session batch result");
        });

        assert_eq!(
            receiver.recv_timeout(std::time::Duration::from_secs(2)),
            Ok(vec![14])
        );
        assert_eq!(execution_budget.peak_active(), 1);
    }

    #[test]
    fn nested_get_many_completes_with_full_session_budget() {
        let session = QuerySession::with_parallelism(2);
        let db = QueryDb::new_with_timings_in_session(
            TestContext {
                executions: AtomicUsize::new(0),
            },
            nia_timing::TimingMode::Off,
            session,
        );
        let (sender, receiver) = std::sync::mpsc::channel();

        std::thread::spawn(move || {
            let values = db.get_many([DoubleMany([1, 2]), DoubleMany([3, 4])]);
            sender
                .send(values.into_iter().map(|value| *value).collect::<Vec<_>>())
                .expect("send nested batch result");
        });

        assert_eq!(
            receiver.recv_timeout(std::time::Duration::from_secs(2)),
            Ok(vec![6, 14])
        );
    }

    #[test]
    fn batch_waiter_does_not_run_tasks_that_depend_on_its_paused_query() {
        let session = QuerySession::with_parallelism(1);
        let db = QueryDb::new_with_timings_in_session(
            BatchIsolationContext {
                session: Mutex::new(Some(session.clone())),
                child_started: Mutex::new(false),
                child_ready: Condvar::new(),
            },
            nia_timing::TimingMode::Off,
            session,
        );
        let (first_sender, first_receiver) = std::sync::mpsc::channel();
        let (second_sender, second_receiver) = std::sync::mpsc::channel();
        let second_db = db.clone();
        std::thread::spawn(move || {
            let mut started = second_db
                .context()
                .child_started
                .lock()
                .expect("batch isolation state lock poisoned");
            while !*started {
                started = second_db
                    .context()
                    .child_ready
                    .wait(started)
                    .expect("batch isolation state lock poisoned while waiting");
            }
            drop(started);
            let values = second_db.get_many([
                BatchIsolationQuery::DependsOnParent,
                BatchIsolationQuery::OtherFiller,
            ]);
            second_sender
                .send(values.into_iter().map(|value| *value).collect::<Vec<_>>())
                .expect("send second isolated batch result");
        });
        std::thread::spawn(move || {
            let values = db.get_many([
                BatchIsolationQuery::Parent,
                BatchIsolationQuery::OuterFiller,
            ]);
            first_sender
                .send(values.into_iter().map(|value| *value).collect::<Vec<_>>())
                .expect("send first isolated batch result");
        });

        assert_eq!(
            first_receiver.recv_timeout(std::time::Duration::from_secs(10)),
            Ok(vec![3, 0])
        );
        assert_eq!(
            second_receiver.recv_timeout(std::time::Duration::from_secs(10)),
            Ok(vec![3, 4])
        );
    }

    #[test]
    fn get_many_panic_does_not_poison_session_executor() {
        let session = QuerySession::with_parallelism(2);
        let db = QueryDb::new_with_timings_in_session(
            TestContext {
                executions: AtomicUsize::new(0),
            },
            nia_timing::TimingMode::Off,
            session,
        );

        let panic = catch_unwind(AssertUnwindSafe(|| db.get_many([PanicsOnce, PanicsOnce])))
            .expect_err("batch should propagate the query panic");
        assert!(panic.is::<&'static str>());

        let values = db.get_many([PanicsOnce, PanicsOnce]);
        assert_eq!(
            values.iter().map(|value| **value).collect::<Vec<_>>(),
            vec![99, 99]
        );
        assert_eq!(db.context().executions.load(Ordering::SeqCst), 2);
    }

    #[test]
    fn default_query_parallelism_is_bounded() {
        let count = default_query_parallelism();

        assert!(count >= 1);
        assert!(count <= DEFAULT_MAX_QUERY_EXECUTOR_PARALLELISM);
    }

    #[test]
    fn reports_same_thread_query_cycles() {
        let db = QueryDb::new(TestContext {
            executions: AtomicUsize::new(0),
        });

        let error = db.try_get(Recursive).expect_err("cycle should be reported");
        let cycle = match error {
            QueryError::Cycle { cycle } => cycle,
            QueryError::InvalidInput { .. } => panic!("expected query cycle"),
        };
        assert_eq!(cycle.len(), 2);
        assert!(cycle.iter().all(|frame| frame.name == "recursive"));
    }

    #[test]
    fn get_panics_with_query_error() {
        let db = QueryDb::new(TestContext {
            executions: AtomicUsize::new(0),
        });

        let error =
            std::panic::catch_unwind(|| db.get(Recursive)).expect_err("get should panic on cycles");
        assert!(error.is::<QueryError>());
    }

    #[test]
    fn query_can_report_invalid_input_as_query_error() {
        let db = QueryDb::new(TestContext {
            executions: AtomicUsize::new(0),
        });

        let err = db
            .try_get(InvalidInputQuery)
            .expect_err("invalid input should be a query error");
        match err {
            QueryError::InvalidInput { query, message } => {
                assert_eq!(query.name, "invalid_input_query");
                assert_eq!(message, "bad fixture");
            }
            QueryError::Cycle { .. } => panic!("expected invalid input error"),
        }
    }

    #[test]
    fn failed_parent_query_drops_speculative_dependencies() {
        let db = QueryDb::new(TestContext {
            executions: AtomicUsize::new(0),
        });

        let err = db
            .try_get(InvalidAfterDependency)
            .expect_err("parent query should fail after recording dependency");
        match err {
            QueryError::InvalidInput { query, message } => {
                assert_eq!(query.name, "invalid_after_dependency");
                assert_eq!(message, "failed after dependency");
            }
            QueryError::Cycle { .. } => panic!("expected invalid input error"),
        }
        assert!(db.query_trace().dependencies.is_empty());

        let invalidation = db.invalidate(Double(3));
        assert_eq!(
            invalidation
                .invalidated
                .iter()
                .map(|frame| frame.description.as_str())
                .collect::<Vec<_>>(),
            vec!["double(3)"]
        );
    }

    #[test]
    fn get_many_workers_detect_cycles_through_parent_stack() {
        let db = QueryDb::new(TestContext {
            executions: AtomicUsize::new(0),
        });
        let worker_db = db.clone();
        let (sender, receiver) = std::sync::mpsc::channel();

        std::thread::spawn(move || {
            let error = std::panic::catch_unwind(|| worker_db.get(ParallelRecursive))
                .expect_err("parallel recursive query should panic");
            sender
                .send(error.is::<QueryError>())
                .expect("send query result");
        });

        assert_eq!(
            receiver.recv_timeout(std::time::Duration::from_secs(2)),
            Ok(true)
        );
    }

    #[test]
    fn panicking_query_resets_slot_for_later_attempts() {
        let db = QueryDb::new(TestContext {
            executions: AtomicUsize::new(0),
        });

        let first =
            std::panic::catch_unwind(|| db.get(PanicsOnce)).expect_err("first query should panic");
        assert!(first.is::<&'static str>());

        assert_eq!(*db.get(PanicsOnce), 99);
        assert_eq!(db.context().executions.load(Ordering::SeqCst), 2);
    }

    #[test]
    fn records_query_dependencies() {
        let db = QueryDb::new(TestContext {
            executions: AtomicUsize::new(0),
        });

        assert_eq!(*db.get(DoubleTwice(7)), 28);
        let trace = db.query_trace();
        assert_eq!(trace.dependencies.len(), 1);
        assert_eq!(trace.dependencies[0].from.name, "double_twice");
        assert_eq!(trace.dependencies[0].to.description, "double(7)");
    }

    #[test]
    fn records_query_execution_and_cache_hit_stats() {
        let db = QueryDb::new(TestContext {
            executions: AtomicUsize::new(0),
        });

        assert_eq!(*db.get(Double(21)), 42);
        assert_eq!(*db.get(Double(21)), 42);
        let trace = db.query_trace();
        let stats = trace
            .queries
            .iter()
            .find(|query| query.frame.description == "double(21)")
            .map(|query| &query.stats)
            .expect("double query stats");

        assert_eq!(stats.executions, 1);
        assert_eq!(stats.cache_hits, 1);
        assert_eq!(stats.waits, 0);
    }

    #[test]
    fn records_get_many_dependencies_from_parent_query() {
        let db = QueryDb::new(TestContext {
            executions: AtomicUsize::new(0),
        });

        assert_eq!(*db.get(DoubleMany([2, 5])), 14);
        let trace = db.query_trace();

        assert!(trace.dependencies.iter().any(|dependency| {
            dependency.from.name == "double_many" && dependency.to.description == "double(2)"
        }));
        assert!(trace.dependencies.iter().any(|dependency| {
            dependency.from.name == "double_many" && dependency.to.description == "double(5)"
        }));
    }

    #[test]
    fn records_single_item_get_many_dependencies_from_parent_query() {
        let db = QueryDb::new(TestContext {
            executions: AtomicUsize::new(0),
        });

        assert_eq!(*db.get(SingleDoubleMany(2)), 4);
        let trace = db.query_trace();

        assert!(trace.dependencies.iter().any(|dependency| {
            dependency.from.name == "single_double_many" && dependency.to.description == "double(2)"
        }));

        let invalidation = db.invalidate(Double(2));
        let invalidated = invalidation
            .invalidated
            .iter()
            .map(|frame| frame.description.as_str())
            .collect::<Vec<_>>();
        assert_eq!(invalidated, vec!["double(2)", "single_double_many(2)"]);
    }

    #[test]
    fn invalidates_direct_query_value() {
        let db = QueryDb::new(TestContext {
            executions: AtomicUsize::new(0),
        });

        assert_eq!(*db.get(Double(9)), 18);
        assert_eq!(*db.get(Double(9)), 18);
        assert_eq!(db.context().executions.load(Ordering::SeqCst), 1);

        let invalidation = db.invalidate(Double(9));
        assert_eq!(invalidation.invalidated.len(), 1);
        assert_eq!(invalidation.invalidated[0].description, "double(9)");

        assert_eq!(*db.get(Double(9)), 18);
        assert_eq!(db.context().executions.load(Ordering::SeqCst), 2);
    }

    #[test]
    fn retiring_query_key_removes_its_slot_and_edges_without_reusing_node_id() {
        let db = QueryDb::new(TestContext {
            executions: AtomicUsize::new(0),
        });
        let old_parent = db.get(DoubleTwice(7));
        let old_node = db
            .cached_slot(&Double(7))
            .expect("cached child slot")
            .node_id;
        assert_eq!(db.query_trace().dependencies.len(), 1);

        assert!(db.retire(&Double(7)));

        let retired_trace = db.query_trace();
        assert_eq!(retired_trace.queries.len(), 1);
        assert!(retired_trace.dependencies.is_empty());
        assert_eq!(*old_parent, 28);
        assert!(
            db.inner
                .session
                .database(db.inner.id)
                .slot(old_node)
                .is_none()
        );

        let latest_parent = db.get(DoubleTwice(7));
        let latest_node = db
            .cached_slot(&Double(7))
            .expect("replacement child slot")
            .node_id;
        assert_eq!(*latest_parent, 28);
        assert_ne!(old_node, latest_node);
        assert_eq!(db.context().executions.load(Ordering::SeqCst), 2);
        assert_eq!(db.query_trace().dependencies.len(), 1);
    }

    #[test]
    fn sealing_owned_query_value_retires_its_only_predecessor_without_invalidation() {
        let db = QueryDb::new(TestContext {
            executions: AtomicUsize::new(0),
        });
        let current = db.get(OwnedRevision(1));
        let predecessor = db.get(OwnedRevision(0));
        let predecessor_node = db
            .cached_slot(&OwnedRevision(0))
            .expect("cached predecessor slot")
            .node_id;
        assert_eq!(&*current, &[0, 1]);
        assert_eq!(db.query_trace().dependencies.len(), 1);

        assert!(db.seal_and_retire_predecessor(&OwnedRevision(1), &OwnedRevision(0)));

        let trace = db.query_trace();
        assert_eq!(trace.queries.len(), 1);
        assert!(trace.dependencies.is_empty());
        assert!(Arc::ptr_eq(&current, &db.get(OwnedRevision(1))));
        assert_eq!(&*predecessor, &[0]);
        assert!(
            db.inner
                .session
                .database(db.inner.id)
                .slot(predecessor_node)
                .is_none()
        );
        assert_eq!(db.context().executions.load(Ordering::SeqCst), 2);
    }

    #[test]
    fn retirement_transaction_invalidates_and_retires_heterogeneous_keys_atomically() {
        let db = QueryDb::new(TestContext {
            executions: AtomicUsize::new(0),
        });
        let double = db.get(Double(3));
        let owned = db.get(OwnedRevision(0));
        let external_retirements = AtomicUsize::new(0);

        db.retirement_transaction(|retirement| {
            let invalidation = retirement.invalidate(Double(3));
            assert_eq!(invalidation.invalidated.len(), 1);
            assert!(retirement.retire(&Double(3)));
            assert!(retirement.retire(&OwnedRevision(0)));
            external_retirements.fetch_add(1, Ordering::SeqCst);
        });

        assert!(db.query_trace().queries.is_empty());
        assert_eq!(*double, 6);
        assert_eq!(&*owned, &[0]);
        assert_eq!(external_retirements.load(Ordering::SeqCst), 1);
    }

    #[test]
    fn retirement_waits_for_active_query_before_releasing_cached_slot() {
        let control = Arc::new((Mutex::new(RaceState::default()), Condvar::new()));
        let db = QueryDb::new(RaceContext {
            executions: AtomicUsize::new(0),
            control: Arc::clone(&control),
        });
        let worker_db = db.clone();
        let query = std::thread::spawn(move || worker_db.get(SlowDouble(1)));
        let (lock, ready) = &*control;
        let mut state = lock.lock().expect("race state lock poisoned");
        while !state.started {
            state = ready.wait(state).expect("race state lock poisoned");
        }
        drop(state);

        let retirement_db = db.clone();
        let (sender, receiver) = std::sync::mpsc::channel();
        let retirement = std::thread::spawn(move || {
            sender
                .send(retirement_db.retire(&SlowDouble(1)))
                .expect("send retirement result");
        });
        let mut activity = db
            .inner
            .session
            .inner
            .activity
            .lock()
            .expect("query activity lock poisoned");
        while !activity.retiring {
            activity = db
                .inner
                .session
                .inner
                .activity_ready
                .wait(activity)
                .expect("query activity lock poisoned while waiting");
        }
        drop(activity);
        assert_eq!(
            receiver.try_recv(),
            Err(std::sync::mpsc::TryRecvError::Empty)
        );
        let trace_db = db.clone();
        let (trace_sender, trace_receiver) = std::sync::mpsc::channel();
        let trace = std::thread::spawn(move || {
            trace_sender
                .send(trace_db.query_trace())
                .expect("send query trace");
        });
        assert_eq!(
            trace_receiver.try_recv(),
            Err(std::sync::mpsc::TryRecvError::Empty)
        );

        let mut state = lock.lock().expect("race state lock poisoned");
        state.release = true;
        ready.notify_all();
        drop(state);
        let old_value = query.join().expect("query worker panicked");
        assert_eq!(receiver.recv(), Ok(true));
        retirement.join().expect("retirement worker panicked");
        assert!(
            trace_receiver
                .recv()
                .expect("receive query trace")
                .queries
                .is_empty()
        );
        trace.join().expect("query trace worker panicked");

        assert_eq!(*old_value, 2);
        assert!(db.query_trace().queries.is_empty());
    }

    #[test]
    fn stable_input_validation_keeps_identical_values_green() {
        let db = QueryDb::new(TestContext {
            executions: AtomicUsize::new(7),
        });
        let first = db.get(StableInputParent);
        assert_eq!(*first, 14);

        let invalidation = db.validate_input(StableInput, &7);

        assert!(invalidation.invalidated.is_empty());
        let second = db.get(StableInputParent);
        assert!(Arc::ptr_eq(&first, &second));
    }

    #[test]
    fn stable_input_validation_invalidates_changed_values_and_dependents() {
        let db = QueryDb::new(TestContext {
            executions: AtomicUsize::new(7),
        });
        assert_eq!(*db.get(StableInputParent), 14);
        db.context().executions.store(9, Ordering::SeqCst);

        let invalidation = db.validate_input(StableInput, &9);
        let invalidated = invalidation
            .invalidated
            .iter()
            .map(|frame| frame.name)
            .collect::<Vec<_>>();

        assert_eq!(invalidated, ["stable_input", "stable_input_parent"]);
        assert_eq!(*db.get(StableInputParent), 18);
    }

    #[test]
    fn derived_red_green_validation_reuses_dependents_when_output_is_unchanged() {
        let db = QueryDb::new(RedGreenContext {
            input: AtomicUsize::new(7),
            derived_executions: AtomicUsize::new(0),
            parent_executions: AtomicUsize::new(0),
        });
        let first = db.get(StableParityParent);
        assert_eq!(*first, 11);
        db.context().input.store(9, Ordering::SeqCst);

        let invalidation = db.validate_input(RedGreenInput, &9);
        assert_eq!(
            invalidation
                .invalidated
                .iter()
                .map(|frame| frame.name)
                .collect::<Vec<_>>(),
            ["red_green_input", "stable_parity", "stable_parity_parent"]
        );
        let second = db.get(StableParityParent);

        assert!(Arc::ptr_eq(&first, &second));
        assert_eq!(db.context().derived_executions.load(Ordering::SeqCst), 2);
        assert_eq!(db.context().parent_executions.load(Ordering::SeqCst), 1);
        let trace = db.query_trace();
        let parent = trace
            .queries
            .iter()
            .find(|query| query.frame.name == "stable_parity_parent")
            .expect("stable parent trace");
        assert_eq!(parent.stats.validations, 1);
        assert_eq!(parent.stats.green_validations, 1);
        assert!(trace.dependencies.iter().any(|dependency| {
            dependency.from.name == "stable_parity_parent" && dependency.to.name == "stable_parity"
        }));
    }

    #[test]
    fn semantic_value_validation_reuses_fingerprint_only_for_equal_outputs() {
        let db = QueryDb::new(RedGreenContext {
            input: AtomicUsize::new(7),
            derived_executions: AtomicUsize::new(0),
            parent_executions: AtomicUsize::new(0),
        });
        let first = db.get(SemanticParityParent);
        db.context().input.store(9, Ordering::SeqCst);
        db.validate_input(RedGreenInput, &9);

        let equal = db.get(SemanticParityParent);

        assert!(Arc::ptr_eq(&first, &equal));
        assert_eq!(db.context().derived_executions.load(Ordering::SeqCst), 2);
        assert_eq!(db.context().parent_executions.load(Ordering::SeqCst), 1);

        db.context().input.store(10, Ordering::SeqCst);
        db.validate_input(RedGreenInput, &10);
        let changed = db.get(SemanticParityParent);

        assert!(!Arc::ptr_eq(&equal, &changed));
        assert_eq!(*changed, 10);
        assert_eq!(db.context().derived_executions.load(Ordering::SeqCst), 3);
        assert_eq!(db.context().parent_executions.load(Ordering::SeqCst), 2);
    }

    #[test]
    fn direct_invalidation_preserves_stable_dependents_for_validation() {
        let db = QueryDb::new(RedGreenContext {
            input: AtomicUsize::new(7),
            derived_executions: AtomicUsize::new(0),
            parent_executions: AtomicUsize::new(0),
        });
        let first = db.get(StableParityParent);
        db.context().input.store(9, Ordering::SeqCst);

        let invalidation = db.invalidate(RedGreenInput);
        assert_eq!(
            invalidation
                .invalidated
                .iter()
                .map(|frame| frame.name)
                .collect::<Vec<_>>(),
            ["red_green_input", "stable_parity", "stable_parity_parent"]
        );
        let latest = db.get(StableParityParent);

        assert!(Arc::ptr_eq(&first, &latest));
        assert_eq!(db.context().derived_executions.load(Ordering::SeqCst), 2);
        assert_eq!(db.context().parent_executions.load(Ordering::SeqCst), 1);
        let trace = db.query_trace();
        let parent = trace
            .queries
            .iter()
            .find(|query| query.frame.name == "stable_parity_parent")
            .expect("stable parent trace");
        assert_eq!(parent.stats.validations, 1);
        assert_eq!(parent.stats.green_validations, 1);
    }

    #[test]
    fn derived_red_green_validation_reexecutes_dependents_when_output_changes() {
        let db = QueryDb::new(RedGreenContext {
            input: AtomicUsize::new(7),
            derived_executions: AtomicUsize::new(0),
            parent_executions: AtomicUsize::new(0),
        });
        assert_eq!(*db.get(StableParityParent), 11);
        db.context().input.store(8, Ordering::SeqCst);

        db.validate_input(RedGreenInput, &8);
        assert_eq!(*db.get(StableParityParent), 10);

        assert_eq!(db.context().derived_executions.load(Ordering::SeqCst), 2);
        assert_eq!(db.context().parent_executions.load(Ordering::SeqCst), 2);
        let trace = db.query_trace();
        let parent = trace
            .queries
            .iter()
            .find(|query| query.frame.name == "stable_parity_parent")
            .expect("stable parent trace");
        assert_eq!(parent.stats.validations, 1);
        assert_eq!(parent.stats.green_validations, 0);
    }

    #[test]
    fn consecutive_input_revisions_validate_against_latest_value() {
        let db = QueryDb::new(RedGreenContext {
            input: AtomicUsize::new(7),
            derived_executions: AtomicUsize::new(0),
            parent_executions: AtomicUsize::new(0),
        });
        let first = db.get(StableParityParent);
        db.context().input.store(9, Ordering::SeqCst);
        db.validate_input(RedGreenInput, &9);
        db.context().input.store(11, Ordering::SeqCst);
        db.validate_input(RedGreenInput, &11);

        let latest = db.get(StableParityParent);

        assert!(Arc::ptr_eq(&first, &latest));
        assert_eq!(db.context().derived_executions.load(Ordering::SeqCst), 2);
        assert_eq!(db.context().parent_executions.load(Ordering::SeqCst), 1);
    }

    #[test]
    fn stable_get_many_records_dependency_fingerprints_for_green_validation() {
        let db = QueryDb::new(RedGreenContext {
            input: AtomicUsize::new(7),
            derived_executions: AtomicUsize::new(0),
            parent_executions: AtomicUsize::new(0),
        });
        let first = db.get(StableModuloBatchParent);
        assert_eq!(*first, 2);
        db.context().input.store(13, Ordering::SeqCst);
        db.validate_input(RedGreenInput, &13);

        let latest = db.get(StableModuloBatchParent);

        assert!(Arc::ptr_eq(&first, &latest));
        assert_eq!(db.context().derived_executions.load(Ordering::SeqCst), 4);
        assert_eq!(db.context().parent_executions.load(Ordering::SeqCst), 1);
        let trace = db.query_trace();
        assert_eq!(
            trace
                .dependencies
                .iter()
                .filter(|dependency| {
                    dependency.from.name == "stable_modulo_batch_parent"
                        && dependency.to.name == "stable_modulo"
                })
                .count(),
            2
        );
    }

    #[test]
    fn invalidation_during_validation_cannot_restore_stale_green_value() {
        let control = Arc::new((Mutex::new(ValidationRaceState::default()), Condvar::new()));
        let db = QueryDb::new(ValidationRaceContext {
            input: AtomicUsize::new(7),
            input_executions: AtomicUsize::new(0),
            derived_executions: AtomicUsize::new(0),
            control: Arc::clone(&control),
        });
        let first = db.get(ValidationRaceDerived);
        db.context().input.store(9, Ordering::SeqCst);
        db.validate_input(ValidationRaceInput, &9);
        let worker_db = db.clone();

        let latest = std::thread::scope(|scope| {
            let handle = scope.spawn(move || worker_db.get(ValidationRaceDerived));
            let (lock, ready) = &*control;
            let mut state = lock.lock().expect("validation race lock poisoned");
            while !state.started {
                state = ready.wait(state).expect("validation race lock poisoned");
            }
            drop(state);

            db.context().input.store(11, Ordering::SeqCst);
            db.validate_input(ValidationRaceInput, &11);

            let mut state = lock.lock().expect("validation race lock poisoned");
            state.release = true;
            ready.notify_all();
            drop(state);
            handle.join().expect("validation worker panicked")
        });

        assert_eq!(*latest, 1);
        assert!(!Arc::ptr_eq(&first, &latest));
        assert_eq!(db.context().input_executions.load(Ordering::SeqCst), 3);
        assert_eq!(db.context().derived_executions.load(Ordering::SeqCst), 2);
        let trace = db.query_trace();
        let derived = trace
            .queries
            .iter()
            .find(|query| query.frame.name == "validation_race_derived")
            .expect("validation race derived trace");
        assert_eq!(derived.stats.validations, 1);
        assert_eq!(derived.stats.green_validations, 0);
    }

    #[test]
    fn invalidating_uncached_key_reports_root_without_allocating_slot() {
        let db = QueryDb::new(TestContext {
            executions: AtomicUsize::new(0),
        });

        let invalidation = db.invalidate(Double(9));

        assert_eq!(invalidation.invalidated.len(), 1);
        assert_eq!(invalidation.invalidated[0].description, "double(9)");
        assert!(db.query_trace().queries.is_empty());
    }

    #[test]
    fn invalidates_transitive_dependents() {
        let db = QueryDb::new(TestContext {
            executions: AtomicUsize::new(0),
        });

        assert_eq!(*db.get(DoubleTwice(7)), 28);
        assert_eq!(*db.get(DoubleTwice(7)), 28);
        assert_eq!(db.context().executions.load(Ordering::SeqCst), 1);

        let invalidation = db.invalidate(Double(7));
        let invalidated = invalidation
            .invalidated
            .iter()
            .map(|frame| frame.description.as_str())
            .collect::<Vec<_>>();
        assert_eq!(invalidated, vec!["double(7)", "double_twice(7)"]);

        assert_eq!(*db.get(DoubleTwice(7)), 28);
        assert_eq!(db.context().executions.load(Ordering::SeqCst), 2);
    }

    #[test]
    fn invalidates_get_many_dependents_without_reordering_results() {
        let db = QueryDb::new(TestContext {
            executions: AtomicUsize::new(0),
        });

        assert_eq!(*db.get(DoubleMany([2, 5])), 14);
        let invalidation = db.invalidate(Double(2));
        let invalidated = invalidation
            .invalidated
            .iter()
            .map(|frame| frame.description.as_str())
            .collect::<Vec<_>>();
        assert_eq!(invalidated, vec!["double(2)", "double_many([2, 5])"]);

        assert_eq!(*db.get(DoubleMany([2, 5])), 14);
    }

    #[test]
    fn dependency_identity_does_not_merge_keys_with_same_debug_label() {
        let db = QueryDb::new(TestContext {
            executions: AtomicUsize::new(0),
        });

        assert_eq!(*db.get(DebugCollisionParent(1)), 4);
        assert_eq!(*db.get(DebugCollisionParent(2)), 8);

        let invalidation = db.invalidate(DebugCollisionLeaf(1));
        let invalidated_names = invalidation
            .invalidated
            .iter()
            .map(|frame| frame.name)
            .collect::<Vec<_>>();
        assert_eq!(
            invalidated_names,
            vec!["debug_collision_leaf", "debug_collision_parent"]
        );

        assert_eq!(*db.get(DebugCollisionParent(2)), 8);
        assert_eq!(db.context().executions.load(Ordering::SeqCst), 2);
        assert_eq!(*db.get(DebugCollisionParent(1)), 4);
        assert_eq!(db.context().executions.load(Ordering::SeqCst), 3);
    }

    #[test]
    fn invalidation_during_get_many_prevents_stale_cache_writeback() {
        let control = Arc::new((Mutex::new(RaceState::default()), Condvar::new()));
        let db = QueryDb::new(RaceContext {
            executions: AtomicUsize::new(0),
            control: control.clone(),
        });
        let worker_db = db.clone();

        std::thread::scope(|scope| {
            let handle = scope.spawn(move || worker_db.get_many([SlowDouble(1), SlowDouble(2)]));

            let (lock, ready) = &*control;
            let mut state = lock.lock().expect("race state lock poisoned");
            while !state.started {
                state = ready.wait(state).expect("race state lock poisoned");
            }
            drop(state);

            let invalidation = db.invalidate(SlowDouble(1));
            assert_eq!(invalidation.invalidated[0].description, "slow_double(1)");

            let mut state = lock.lock().expect("race state lock poisoned");
            state.release = true;
            ready.notify_all();
            drop(state);

            assert_eq!(
                handle
                    .join()
                    .expect("get_many worker panicked")
                    .iter()
                    .map(|value| **value)
                    .collect::<Vec<_>>(),
                vec![2, 4]
            );
        });

        assert_eq!(*db.get(SlowDouble(1)), 2);
        assert_eq!(db.context().executions.load(Ordering::SeqCst), 3);
    }

    #[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
    struct DoubleTwice(usize);

    impl QueryKey<TestContext> for DoubleTwice {
        type Value = usize;

        fn name() -> &'static str {
            "double_twice"
        }

        fn description(&self) -> String {
            format!("double_twice({})", self.0)
        }

        fn execute(&self, db: &QueryDb<TestContext>) -> Self::Value {
            *db.get(Double(self.0)) * 2
        }
    }

    #[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
    struct DoubleMany([usize; 2]);

    impl QueryKey<TestContext> for DoubleMany {
        type Value = usize;

        fn name() -> &'static str {
            "double_many"
        }

        fn description(&self) -> String {
            format!("double_many({:?})", self.0)
        }

        fn execute(&self, db: &QueryDb<TestContext>) -> Self::Value {
            db.get_many(self.0.map(Double))
                .into_iter()
                .map(|value| *value)
                .sum()
        }
    }

    #[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
    struct SingleDoubleMany(usize);

    impl QueryKey<TestContext> for SingleDoubleMany {
        type Value = usize;

        fn name() -> &'static str {
            "single_double_many"
        }

        fn description(&self) -> String {
            format!("single_double_many({})", self.0)
        }

        fn execute(&self, db: &QueryDb<TestContext>) -> Self::Value {
            db.get_many([Double(self.0)])
                .into_iter()
                .map(|value| *value)
                .sum()
        }
    }

    #[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
    struct ParallelRecursive;

    impl QueryKey<TestContext> for ParallelRecursive {
        type Value = usize;

        fn name() -> &'static str {
            "parallel_recursive"
        }

        fn execute(&self, db: &QueryDb<TestContext>) -> Self::Value {
            db.get_many([ParallelRecursiveChild])
                .into_iter()
                .map(|value| *value)
                .sum()
        }
    }

    #[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
    struct ParallelRecursiveChild;

    impl QueryKey<TestContext> for ParallelRecursiveChild {
        type Value = usize;

        fn name() -> &'static str {
            "parallel_recursive_child"
        }

        fn execute(&self, db: &QueryDb<TestContext>) -> Self::Value {
            *db.get(ParallelRecursive)
        }
    }

    #[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
    struct PanicsOnce;

    impl QueryKey<TestContext> for PanicsOnce {
        type Value = usize;

        fn name() -> &'static str {
            "panics_once"
        }

        fn execute(&self, db: &QueryDb<TestContext>) -> Self::Value {
            let previous = db.context().executions.fetch_add(1, Ordering::SeqCst);
            if previous == 0 {
                panic!("transient query failure");
            }
            99
        }
    }

    #[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
    struct InvalidInputQuery;

    impl QueryKey<TestContext> for InvalidInputQuery {
        type Value = usize;

        fn name() -> &'static str {
            "invalid_input_query"
        }

        fn execute(&self, db: &QueryDb<TestContext>) -> Self::Value {
            db.invalid_input(self, "bad fixture")
        }
    }

    #[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
    struct InvalidAfterDependency;

    impl QueryKey<TestContext> for InvalidAfterDependency {
        type Value = usize;

        fn name() -> &'static str {
            "invalid_after_dependency"
        }

        fn execute(&self, db: &QueryDb<TestContext>) -> Self::Value {
            let _ = db.get(Double(3));
            db.invalid_input(self, "failed after dependency")
        }
    }

    #[derive(Clone, Copy, PartialEq, Eq, Hash)]
    struct DebugCollisionParent(usize);

    impl Debug for DebugCollisionParent {
        fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
            f.write_str("DebugCollisionParent(<hidden>)")
        }
    }

    impl QueryKey<TestContext> for DebugCollisionParent {
        type Value = usize;

        fn name() -> &'static str {
            "debug_collision_parent"
        }

        fn execute(&self, db: &QueryDb<TestContext>) -> Self::Value {
            *db.get(DebugCollisionLeaf(self.0)) * 2
        }
    }

    #[derive(Clone, Copy, PartialEq, Eq, Hash)]
    struct DebugCollisionLeaf(usize);

    impl Debug for DebugCollisionLeaf {
        fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
            f.write_str("DebugCollisionLeaf(<hidden>)")
        }
    }

    impl QueryKey<TestContext> for DebugCollisionLeaf {
        type Value = usize;

        fn name() -> &'static str {
            "debug_collision_leaf"
        }

        fn execute(&self, db: &QueryDb<TestContext>) -> Self::Value {
            db.context().executions.fetch_add(1, Ordering::SeqCst);
            self.0 * 2
        }
    }

    struct RaceContext {
        executions: AtomicUsize,
        control: Arc<(Mutex<RaceState>, Condvar)>,
    }

    #[derive(Default)]
    struct RaceState {
        started: bool,
        release: bool,
    }

    #[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
    struct SlowDouble(usize);

    impl QueryKey<RaceContext> for SlowDouble {
        type Value = usize;

        fn name() -> &'static str {
            "slow_double"
        }

        fn description(&self) -> String {
            format!("slow_double({})", self.0)
        }

        fn execute(&self, db: &QueryDb<RaceContext>) -> Self::Value {
            db.context().executions.fetch_add(1, Ordering::SeqCst);
            if self.0 == 1 {
                let (lock, ready) = &*db.context().control;
                let mut state = lock.lock().expect("race state lock poisoned");
                state.started = true;
                ready.notify_all();
                while !state.release {
                    state = ready.wait(state).expect("race state lock poisoned");
                }
            }
            self.0 * 2
        }
    }
}
