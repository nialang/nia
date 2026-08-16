// SPDX-License-Identifier: GPL-3.0-or-later
//! Incremental query storage, dependency tracking, and bounded task execution.
//!
//! The session owns cross-database dependency state and execution resources;
//! each `QueryDb` owns typed slots for one context. A slot may publish a cached
//! shared value or transfer one single-consumer value, never both.

mod database;
mod dependency;
mod descriptor;
mod executor;
mod resources;
mod session;

pub(crate) use dependency::*;
pub use descriptor::*;

pub use resources::{
    ProcessMemoryPermit, acquire_llvm_memory_permit, effective_available_memory_bytes,
    effective_memory_limit_bytes, llvm_memory_task_capacity,
};

const SEMANTIC_VALUE_DOMAIN: FingerprintDomain =
    FingerprintDomain::new("nia.query.semantic-value.v1");

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

/// Typed memoization database for one compiler context.
///
/// Databases created in the same [`QuerySession`] share dependency tracking and
/// execution resources, which allows invalidation to cross database boundaries.
pub struct QueryDb<C> {
    inner: Arc<QueryDbInner<C>>,
}

/// Quiescent capability exposed during a cache-retirement transaction.
///
/// While this value exists, the session admits no new outer query activity, so
/// invalidation and slot removal can update typed caches and dependency identity
/// atomically.
pub struct QueryRetirement<'a, C> {
    db: &'a QueryDb<C>,
}

/// Shared execution, dependency, and retirement domain for query databases.
///
/// Cloning a session preserves identity. Constructing a new session deliberately
/// isolates invalidation dependencies, while the process execution budget still
/// limits aggregate worker pressure across sessions.
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

/// Process-wide wait-for edges between queries that are currently blocked.
///
/// A thread-local query stack detects direct recursion, but two executor workers
/// can otherwise wait on each other through disjoint stacks. The graph is
/// process-wide because providers may call databases belonging to different
/// sessions. It is only live while a caller is blocked on a slot and remains
/// separate from the persistent dependency graph used for invalidation.
#[derive(Debug, Default)]
struct QueryWaitGraph {
    edges: FastHashMap<QueryNodeId, QueryNodeId>,
    frames: FastHashMap<QueryNodeId, QueryFrame>,
}

impl QueryWaitGraph {
    fn begin(
        &mut self,
        from: QueryNodeId,
        from_frame: QueryFrame,
        to: QueryNodeId,
        to_frame: QueryFrame,
    ) -> Option<Vec<QueryFrame>> {
        let mut path = vec![from];
        let mut current = to;
        let mut seen = FastHashSet::default();
        while seen.insert(current) {
            path.push(current);
            if current == from {
                return Some(
                    path.into_iter()
                        .map(|node_id| {
                            self.frames
                                .get(&node_id)
                                .cloned()
                                .expect("active query wait node must retain its frame")
                        })
                        .collect(),
                );
            }
            let Some(next) = self.edges.get(&current).copied() else {
                break;
            };
            current = next;
        }
        assert!(
            !self.edges.contains_key(&from),
            "query cannot wait on multiple slots simultaneously"
        );
        self.frames.entry(from).or_insert(from_frame);
        self.frames.entry(to).or_insert(to_frame);
        self.edges.insert(from, to);
        None
    }

    fn end(&mut self, from: QueryNodeId, to: QueryNodeId) {
        let target = self.edges.remove(&from);
        assert_eq!(
            target,
            Some(to),
            "query wait-for edge was released out of order"
        );
        if !self.edges.contains_key(&from) && !self.edges.values().any(|target| *target == from) {
            self.frames.remove(&from);
        }
        if !self.edges.contains_key(&to) && !self.edges.values().any(|target| *target == to) {
            self.frames.remove(&to);
        }
    }
}

struct QueryWaitGuard {
    from: QueryNodeId,
    to: QueryNodeId,
}

fn query_wait_graph() -> &'static Mutex<QueryWaitGraph> {
    static GRAPH: OnceLock<Mutex<QueryWaitGraph>> = OnceLock::new();
    GRAPH.get_or_init(|| Mutex::new(QueryWaitGraph::default()))
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

/// Incrementally submitted task set with a fixed in-flight capacity.
///
/// Submission applies backpressure once `capacity` tasks are pending. [`finish`](Self::finish)
/// drains every accepted task and restores submission order even when workers complete out of
/// order or one task panics.
pub struct QueryTaskPool<'session, O: Send + 'static> {
    session: &'session QuerySession,
    _activity: QueryActivityGuard<'session>,
    capacity: usize,
    next_position: usize,
    pending: VecDeque<SpawnedQueryTask<O>>,
    completed: Vec<(usize, O)>,
}

/// Completion-order view over a batch of owned query results.
///
/// Dependency facts from each completed worker are merged into the logical parent query before
/// the item is yielded, so streaming consumption preserves the same invalidation graph as
/// [`QueryDb::get_many_owned`].
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
        let mut builder = QueryFingerprintBuilder::new(SEMANTIC_VALUE_DOMAIN);
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
    frame: QueryFrame,
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

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::{
        Barrier, Condvar,
        atomic::{AtomicUsize, Ordering},
    };

    include!("tests/core_fixture.rs");

    include!("tests/red_green_fixture.rs");

    include!("tests/storage_fixture.rs");

    include!("tests/dependency_fixture.rs");

    #[path = "storage_contracts.rs"]
    mod storage_contracts;

    #[path = "registry_contracts.rs"]
    mod registry_contracts;

    #[path = "session_dependencies.rs"]
    mod session_dependencies;

    #[path = "batch_access.rs"]
    mod batch_access;

    #[path = "session_tasks.rs"]
    mod session_tasks;

    #[path = "priority_tasks.rs"]
    mod priority_tasks;

    #[path = "executor_budget.rs"]
    mod executor_budget;

    #[path = "nested_execution.rs"]
    mod nested_execution;

    #[path = "error_recovery.rs"]
    mod error_recovery;

    #[path = "dependency_trace.rs"]
    mod dependency_trace;

    #[path = "revision_retirement.rs"]
    mod revision_retirement;

    #[path = "red_green_validation.rs"]
    mod red_green_validation;

    #[path = "red_green_advanced.rs"]
    mod red_green_advanced;

    #[path = "red_green_revisions.rs"]
    mod red_green_revisions;

    #[path = "invalidation_contracts.rs"]
    mod invalidation_contracts;
}
