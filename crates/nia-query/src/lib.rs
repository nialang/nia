// SPDX-License-Identifier: GPL-3.0-or-later
use std::{
    any::{Any, TypeId},
    cell::RefCell,
    collections::{HashMap, HashSet, VecDeque},
    fmt::{self, Debug},
    hash::{DefaultHasher, Hash, Hasher},
    sync::{Arc, Condvar, Mutex},
};

const DEFAULT_MAX_QUERY_MANY_THREADS: usize = 4;
const QUERY_THREADS_ENV: &str = "NIA_QUERY_THREADS";

pub trait QueryKey<C>: Clone + Debug + Eq + Hash + Send + Sync + 'static {
    type Value: Clone + Send + Sync + 'static;

    fn name() -> &'static str;
    fn description(&self) -> String {
        format!("{}::{self:?}", Self::name())
    }
    fn execute(&self, db: &QueryDb<C>) -> Self::Value;
}

pub struct QueryDb<C> {
    inner: Arc<QueryDbInner<C>>,
}

struct QueryDbInner<C> {
    context: C,
    caches: Mutex<HashMap<TypeId, Box<dyn Any + Send + Sync>>>,
    slots: Mutex<HashMap<QueryFrameIdentity, Arc<dyn ErasedQuerySlot>>>,
    dependencies: Mutex<QueryDependencyGraph>,
}

struct QuerySlot<V> {
    state: Mutex<QueryState<V>>,
    ready: Condvar,
}

enum QueryState<V> {
    Empty,
    Computing { invalidated: bool },
    Ready(V),
}

trait ErasedQuerySlot: Send + Sync {
    fn invalidate(&self);
}

impl<V> ErasedQuerySlot for QuerySlot<V>
where
    V: Clone + Send + Sync + 'static,
{
    fn invalidate(&self) {
        let mut state = self.state.lock().expect("query cache lock poisoned");
        match &mut *state {
            QueryState::Empty => {}
            QueryState::Computing { invalidated } => {
                *invalidated = true;
            }
            QueryState::Ready(_) => {
                *state = QueryState::Empty;
                self.ready.notify_all();
            }
        }
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

#[derive(Clone)]
struct QueryFrameIdentity {
    type_id: TypeId,
    key_hash: u64,
    key_debug: String,
    key: Arc<dyn ErasedQueryKey>,
}

impl Debug for QueryFrameIdentity {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("QueryFrameIdentity")
            .field("type_id", &self.type_id)
            .field("key_hash", &self.key_hash)
            .field("key", &self.key_debug)
            .finish()
    }
}

impl PartialEq for QueryFrameIdentity {
    fn eq(&self, other: &Self) -> bool {
        self.type_id == other.type_id
            && self.key_hash == other.key_hash
            && self.key.eq_key(other.key.as_ref())
    }
}

impl Eq for QueryFrameIdentity {}

impl Hash for QueryFrameIdentity {
    fn hash<H: Hasher>(&self, state: &mut H) {
        self.type_id.hash(state);
        self.key_hash.hash(state);
    }
}

trait ErasedQueryKey: Send + Sync {
    fn as_any(&self) -> &dyn Any;
    fn eq_key(&self, other: &dyn ErasedQueryKey) -> bool;
}

impl<K> ErasedQueryKey for K
where
    K: Clone + Eq + Hash + Send + Sync + 'static,
{
    fn as_any(&self) -> &dyn Any {
        self
    }

    fn eq_key(&self, other: &dyn ErasedQueryKey) -> bool {
        other.as_any().downcast_ref::<K>() == Some(self)
    }
}

#[derive(Debug, Default)]
struct QueryDependencyGraph {
    dependencies: Vec<QueryDependencyEdge>,
    frames: HashMap<QueryFrameIdentity, QueryFrame>,
    forward: HashMap<QueryFrameIdentity, HashSet<QueryFrameIdentity>>,
    reverse: HashMap<QueryFrameIdentity, HashSet<QueryFrameIdentity>>,
}

#[derive(Debug, Clone)]
struct QueryDependencyEdge {
    from: QueryFrameIdentity,
    dependency: QueryDependency,
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
}

#[derive(Debug, Clone)]
struct QueryStackEntry {
    identity: QueryFrameIdentity,
    frame: QueryFrame,
}

struct QueryStackGuard;

struct QueryStackInstallGuard {
    previous: Vec<QueryStackEntry>,
}

thread_local! {
    static QUERY_STACK: RefCell<Vec<QueryStackEntry>> = const { RefCell::new(Vec::new()) };
}

impl<C> QueryDb<C> {
    pub fn new(context: C) -> Self {
        Self {
            inner: Arc::new(QueryDbInner {
                context,
                caches: Mutex::new(HashMap::new()),
                slots: Mutex::new(HashMap::new()),
                dependencies: Mutex::new(QueryDependencyGraph::default()),
            }),
        }
    }

    pub fn context(&self) -> &C {
        &self.inner.context
    }

    pub fn query<K>(&self, key: K) -> K::Value
    where
        K: QueryKey<C>,
    {
        self.try_query(key)
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

    pub fn try_query<K>(&self, key: K) -> QueryResult<K::Value>
    where
        K: QueryKey<C>,
    {
        self.record_dependency::<K>(&key);
        let slot = self.slot_for(&key);
        loop {
            let mut state = slot.state.lock().expect("query cache lock poisoned");
            match &*state {
                QueryState::Ready(value) => return Ok(value.clone()),
                QueryState::Computing { .. } => {
                    self.check_not_recursive::<K>(&key)?;
                    drop(
                        slot.ready
                            .wait(state)
                            .expect("query cache lock poisoned while waiting"),
                    );
                }
                QueryState::Empty => {
                    *state = QueryState::Computing { invalidated: false };
                    drop(state);

                    let entry = query_stack_entry::<C, K>(&key);
                    let identity = entry.identity.clone();
                    self.clear_dependencies_from(&identity);
                    let _guard = self.enter_query(entry)?;
                    let value = match std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                        key.execute(self)
                    })) {
                        Ok(value) => value,
                        Err(payload) => {
                            let mut state = slot.state.lock().expect("query cache lock poisoned");
                            *state = QueryState::Empty;
                            // Dependencies recorded during a failed execution are speculative:
                            // keeping them would make future invalidations report a query value
                            // that was never cached and can no longer be reused.
                            self.clear_dependencies_from(&identity);
                            slot.ready.notify_all();
                            drop(state);
                            match payload.downcast::<QueryError>() {
                                Ok(err) => return Err(*err),
                                Err(payload) => std::panic::resume_unwind(payload),
                            }
                        }
                    };

                    let mut state = slot.state.lock().expect("query cache lock poisoned");
                    let was_invalidated =
                        matches!(&*state, QueryState::Computing { invalidated: true });
                    if was_invalidated {
                        *state = QueryState::Empty;
                        // The value was computed from an input that changed while this query was
                        // running. Return it to the caller that did the work, but drop the cache
                        // entry and its edges so the next request recomputes against fresh inputs.
                        self.clear_dependencies_from(&identity);
                    } else {
                        *state = QueryState::Ready(value.clone());
                    }
                    slot.ready.notify_all();
                    return Ok(value);
                }
            }
        }
    }

    pub fn query_many<K>(&self, keys: impl IntoIterator<Item = K>) -> Vec<K::Value>
    where
        C: Send + Sync + 'static,
        K: QueryKey<C>,
    {
        let keys = keys.into_iter().collect::<Vec<_>>();
        if keys.is_empty() {
            return Vec::new();
        }
        for key in &keys {
            self.record_dependency::<K>(key);
        }
        // `query_many` runs work on fresh OS threads, so the thread-local query
        // stack has to be copied explicitly. Without this logical parent stack,
        // a worker that asks for an ancestor query would wait on the parent
        // thread, while the parent is waiting for the worker to finish.
        let parent_stack = current_query_stack();
        let worker_count = query_many_worker_count(keys.len());
        if worker_count == 1 {
            let _stack_guard = install_query_stack(parent_stack);
            return keys.into_iter().map(|key| self.query(key)).collect();
        }
        let queue = Arc::new(Mutex::new(
            keys.into_iter().enumerate().collect::<VecDeque<_>>(),
        ));
        std::thread::scope(|scope| {
            let handles = (0..worker_count)
                .map(|_| {
                    let db = self.clone();
                    let parent_stack = parent_stack.clone();
                    let queue = queue.clone();
                    scope.spawn(move || {
                        let _stack_guard = install_query_stack(parent_stack);
                        let mut values = Vec::new();
                        loop {
                            let work = queue
                                .lock()
                                .expect("query_many work queue lock poisoned")
                                .pop_front();
                            let Some((index, key)) = work else {
                                return values;
                            };
                            values.push((index, db.query(key)));
                        }
                    })
                })
                .collect::<Vec<_>>();
            let mut values = Vec::new();
            for handle in handles {
                match handle.join() {
                    Ok(worker_values) => values.extend(worker_values),
                    Err(payload) => std::panic::resume_unwind(payload),
                }
            }
            values.sort_by_key(|(index, _)| *index);
            values.into_iter().map(|(_, value)| value).collect()
        })
    }

    pub fn query_trace(&self) -> QueryTrace {
        QueryTrace {
            dependencies: self
                .inner
                .dependencies
                .lock()
                .expect("query dependency lock poisoned")
                .dependencies
                .iter()
                .map(|edge| edge.dependency.clone())
                .collect(),
        }
    }

    pub fn invalidate<K>(&self, key: K) -> QueryInvalidation
    where
        K: QueryKey<C>,
    {
        let root = query_frame_identity::<K>(&key);
        let invalidated = self.collect_invalidated_frames(root.clone());
        let slots = self
            .inner
            .slots
            .lock()
            .expect("query cache slot lock poisoned");
        for identity in &invalidated {
            if let Some(slot) = slots.get(identity) {
                slot.invalidate();
            }
        }
        drop(slots);

        let mut dependencies = self
            .inner
            .dependencies
            .lock()
            .expect("query dependency lock poisoned");
        let frames = invalidated
            .iter()
            .filter_map(|identity| dependencies.frames.get(identity).cloned())
            .collect::<Vec<_>>();
        for identity in &invalidated {
            dependencies.remove_dependencies_from(identity);
        }
        QueryInvalidation {
            invalidated: frames,
        }
    }

    fn slot_for<K>(&self, key: &K) -> Arc<QuerySlot<K::Value>>
    where
        K: QueryKey<C>,
    {
        let identity = query_frame_identity::<K>(key);
        let frame = query_frame::<C, K>(key);
        let mut caches = self.inner.caches.lock().expect("query cache lock poisoned");
        let cache = caches
            .entry(TypeId::of::<K>())
            .or_insert_with(|| Box::new(Mutex::new(HashMap::<K, Arc<QuerySlot<K::Value>>>::new())))
            .downcast_ref::<Mutex<HashMap<K, Arc<QuerySlot<K::Value>>>>>()
            .expect("query cache type mismatch");
        let mut cache = cache.lock().expect("query cache lock poisoned");
        let slot = cache
            .entry(key.clone())
            .or_insert_with(|| {
                Arc::new(QuerySlot {
                    state: Mutex::new(QueryState::Empty),
                    ready: Condvar::new(),
                })
            })
            .clone();
        self.inner
            .slots
            .lock()
            .expect("query cache slot lock poisoned")
            .insert(identity.clone(), slot.clone() as Arc<dyn ErasedQuerySlot>);
        self.inner
            .dependencies
            .lock()
            .expect("query dependency lock poisoned")
            .record_frame(identity, frame);
        slot
    }

    fn enter_query(&self, entry: QueryStackEntry) -> QueryResult<QueryStackGuard> {
        self.check_not_recursive_identity(&entry.identity)?;
        QUERY_STACK.with(|stack| {
            stack.borrow_mut().push(entry);
        });
        Ok(QueryStackGuard)
    }

    fn check_not_recursive<K>(&self, key: &K) -> QueryResult<()>
    where
        K: QueryKey<C>,
    {
        self.check_not_recursive_identity(&query_frame_identity::<K>(key))
    }

    fn check_not_recursive_identity(&self, identity: &QueryFrameIdentity) -> QueryResult<()> {
        QUERY_STACK.with(|stack| {
            let stack = stack.borrow();
            if let Some(position) = stack.iter().position(|entry| &entry.identity == identity) {
                let mut cycle = stack[position..]
                    .iter()
                    .map(|entry| entry.frame.clone())
                    .collect::<Vec<_>>();
                cycle.push(
                    stack
                        .iter()
                        .find(|entry| &entry.identity == identity)
                        .map(|entry| entry.frame.clone())
                        .unwrap_or_else(|| QueryFrame {
                            name: "<unknown>",
                            key: identity.key_debug.clone(),
                            description: identity.key_debug.clone(),
                        }),
                );
                return Err(QueryError::Cycle { cycle });
            }
            Ok(())
        })
    }

    fn record_dependency<K>(&self, key: &K)
    where
        K: QueryKey<C>,
    {
        self.record_dependency_from_stack(query_stack_entry::<C, K>(key));
    }

    fn record_dependency_from_stack(&self, to: QueryStackEntry) {
        QUERY_STACK.with(|stack| {
            let Some(from) = stack.borrow().last().cloned() else {
                return;
            };
            self.inner
                .dependencies
                .lock()
                .expect("query dependency lock poisoned")
                .record(from, to);
        });
    }

    fn collect_invalidated_frames(&self, root: QueryFrameIdentity) -> Vec<QueryFrameIdentity> {
        let dependencies = self
            .inner
            .dependencies
            .lock()
            .expect("query dependency lock poisoned");
        dependencies.collect_dependents(root)
    }

    fn clear_dependencies_from(&self, from: &QueryFrameIdentity) {
        self.inner
            .dependencies
            .lock()
            .expect("query dependency lock poisoned")
            .remove_dependencies_from(from);
    }
}

fn query_many_worker_count(work_items: usize) -> usize {
    if work_items <= 1 {
        return work_items;
    }
    let configured = std::env::var(QUERY_THREADS_ENV)
        .ok()
        .and_then(|value| value.parse::<usize>().ok())
        .filter(|value| *value > 0);
    let available = configured.unwrap_or_else(default_query_many_threads);
    available.clamp(1, work_items)
}

fn default_query_many_threads() -> usize {
    let available = std::thread::available_parallelism()
        .map(usize::from)
        .unwrap_or(1);
    available
        .div_ceil(2)
        .clamp(1, DEFAULT_MAX_QUERY_MANY_THREADS)
}

impl QueryDependencyGraph {
    fn record_frame(&mut self, identity: QueryFrameIdentity, frame: QueryFrame) {
        self.frames.entry(identity).or_insert(frame);
    }

    fn record(&mut self, from: QueryStackEntry, to: QueryStackEntry) {
        self.record_frame(from.identity.clone(), from.frame.clone());
        self.record_frame(to.identity.clone(), to.frame.clone());

        if self
            .forward
            .entry(from.identity.clone())
            .or_default()
            .insert(to.identity.clone())
        {
            self.reverse
                .entry(to.identity.clone())
                .or_default()
                .insert(from.identity.clone());
            self.dependencies.push(QueryDependencyEdge {
                from: from.identity,
                dependency: QueryDependency {
                    from: from.frame,
                    to: to.frame,
                },
            });
        }
    }

    fn collect_dependents(&self, root: QueryFrameIdentity) -> Vec<QueryFrameIdentity> {
        let mut seen = HashSet::new();
        let mut queue = vec![root];
        let mut invalidated = Vec::new();

        while let Some(identity) = queue.pop() {
            if !seen.insert(identity.clone()) {
                continue;
            }
            invalidated.push(identity.clone());

            let mut dependents = self
                .reverse
                .get(&identity)
                .into_iter()
                .flat_map(|dependents| dependents.iter().cloned())
                .collect::<Vec<_>>();
            dependents.sort_by_key(|dependent| {
                self.frames
                    .get(dependent)
                    .map(|frame| (frame.name, frame.key.clone()))
                    .unwrap_or(("", dependent.key_debug.clone()))
            });
            dependents.reverse();
            queue.extend(dependents);
        }

        invalidated
    }

    fn remove_dependencies_from(&mut self, from: &QueryFrameIdentity) {
        if let Some(targets) = self.forward.remove(from) {
            for target in targets {
                if let Some(dependents) = self.reverse.get_mut(&target) {
                    dependents.remove(from);
                    if dependents.is_empty() {
                        self.reverse.remove(&target);
                    }
                }
            }
        }
        self.dependencies
            .retain(|dependency| &dependency.from != from);
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

fn query_stack_entry<C, K>(key: &K) -> QueryStackEntry
where
    K: QueryKey<C>,
{
    QueryStackEntry {
        identity: query_frame_identity::<K>(key),
        frame: query_frame::<C, K>(key),
    }
}

fn query_frame_identity<K>(key: &K) -> QueryFrameIdentity
where
    K: Clone + Debug + Eq + Hash + Send + Sync + 'static,
{
    let mut hasher = DefaultHasher::new();
    key.hash(&mut hasher);
    // `key` stays as the human-readable query label used in traces. The hash is
    // part of identity because `Debug` is not required to be unique: two keys can
    // deliberately render the same label while still being distinct by `Eq`.
    QueryFrameIdentity {
        type_id: TypeId::of::<K>(),
        key_hash: hasher.finish(),
        key_debug: format!("{key:?}"),
        key: Arc::new(key.clone()),
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
        QUERY_STACK.with(|stack| {
            stack.borrow_mut().pop();
        });
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

fn install_query_stack(stack_snapshot: Vec<QueryStackEntry>) -> QueryStackInstallGuard {
    QUERY_STACK.with(|stack| QueryStackInstallGuard {
        previous: std::mem::replace(&mut *stack.borrow_mut(), stack_snapshot),
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::{
        Condvar,
        atomic::{AtomicUsize, Ordering},
    };

    struct TestContext {
        executions: AtomicUsize,
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
    struct Recursive;

    impl QueryKey<TestContext> for Recursive {
        type Value = usize;

        fn name() -> &'static str {
            "recursive"
        }

        fn execute(&self, db: &QueryDb<TestContext>) -> Self::Value {
            db.query(Recursive)
        }
    }

    #[test]
    fn memoizes_query_values() {
        let db = QueryDb::new(TestContext {
            executions: AtomicUsize::new(0),
        });

        assert_eq!(db.query(Double(21)), 42);
        assert_eq!(db.query(Double(21)), 42);
        assert_eq!(db.context().executions.load(Ordering::SeqCst), 1);
    }

    #[test]
    fn executes_query_many_in_key_order() {
        let db = QueryDb::new(TestContext {
            executions: AtomicUsize::new(0),
        });

        let values = db.query_many([Double(1), Double(4), Double(3)]);

        assert_eq!(values, vec![2, 8, 6]);
        assert_eq!(db.context().executions.load(Ordering::SeqCst), 3);
    }

    #[test]
    fn default_query_many_threads_is_bounded() {
        let count = default_query_many_threads();

        assert!(count >= 1);
        assert!(count <= DEFAULT_MAX_QUERY_MANY_THREADS);
    }

    #[test]
    fn reports_same_thread_query_cycles() {
        let db = QueryDb::new(TestContext {
            executions: AtomicUsize::new(0),
        });

        let error = db
            .try_query(Recursive)
            .expect_err("cycle should be reported");
        let cycle = match error {
            QueryError::Cycle { cycle } => cycle,
            QueryError::InvalidInput { .. } => panic!("expected query cycle"),
        };
        assert_eq!(cycle.len(), 2);
        assert!(cycle.iter().all(|frame| frame.name == "recursive"));
    }

    #[test]
    fn query_panics_with_query_error_for_legacy_callers() {
        let db = QueryDb::new(TestContext {
            executions: AtomicUsize::new(0),
        });

        let error = std::panic::catch_unwind(|| db.query(Recursive))
            .expect_err("legacy query should panic on cycles");
        assert!(error.is::<QueryError>());
    }

    #[test]
    fn query_can_report_invalid_input_as_query_error() {
        let db = QueryDb::new(TestContext {
            executions: AtomicUsize::new(0),
        });

        let err = db
            .try_query(InvalidInputQuery)
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
            .try_query(InvalidAfterDependency)
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
    fn query_many_workers_detect_cycles_through_parent_stack() {
        let db = QueryDb::new(TestContext {
            executions: AtomicUsize::new(0),
        });
        let worker_db = db.clone();
        let (sender, receiver) = std::sync::mpsc::channel();

        std::thread::spawn(move || {
            let error = std::panic::catch_unwind(|| worker_db.query(ParallelRecursive))
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

        let first = std::panic::catch_unwind(|| db.query(PanicsOnce))
            .expect_err("first query should panic");
        assert!(first.is::<&'static str>());

        assert_eq!(db.query(PanicsOnce), 99);
        assert_eq!(db.context().executions.load(Ordering::SeqCst), 2);
    }

    #[test]
    fn records_query_dependencies() {
        let db = QueryDb::new(TestContext {
            executions: AtomicUsize::new(0),
        });

        assert_eq!(db.query(DoubleTwice(7)), 28);
        let trace = db.query_trace();
        assert_eq!(trace.dependencies.len(), 1);
        assert_eq!(trace.dependencies[0].from.name, "double_twice");
        assert_eq!(trace.dependencies[0].to.description, "double(7)");
    }

    #[test]
    fn records_query_many_dependencies_from_parent_query() {
        let db = QueryDb::new(TestContext {
            executions: AtomicUsize::new(0),
        });

        assert_eq!(db.query(DoubleMany([2, 5])), 14);
        let trace = db.query_trace();

        assert!(trace.dependencies.iter().any(|dependency| {
            dependency.from.name == "double_many" && dependency.to.description == "double(2)"
        }));
        assert!(trace.dependencies.iter().any(|dependency| {
            dependency.from.name == "double_many" && dependency.to.description == "double(5)"
        }));
    }

    #[test]
    fn invalidates_direct_query_value() {
        let db = QueryDb::new(TestContext {
            executions: AtomicUsize::new(0),
        });

        assert_eq!(db.query(Double(9)), 18);
        assert_eq!(db.query(Double(9)), 18);
        assert_eq!(db.context().executions.load(Ordering::SeqCst), 1);

        let invalidation = db.invalidate(Double(9));
        assert_eq!(invalidation.invalidated.len(), 1);
        assert_eq!(invalidation.invalidated[0].description, "double(9)");

        assert_eq!(db.query(Double(9)), 18);
        assert_eq!(db.context().executions.load(Ordering::SeqCst), 2);
    }

    #[test]
    fn invalidates_transitive_dependents() {
        let db = QueryDb::new(TestContext {
            executions: AtomicUsize::new(0),
        });

        assert_eq!(db.query(DoubleTwice(7)), 28);
        assert_eq!(db.query(DoubleTwice(7)), 28);
        assert_eq!(db.context().executions.load(Ordering::SeqCst), 1);

        let invalidation = db.invalidate(Double(7));
        let invalidated = invalidation
            .invalidated
            .iter()
            .map(|frame| frame.description.as_str())
            .collect::<Vec<_>>();
        assert_eq!(invalidated, vec!["double(7)", "double_twice(7)"]);

        assert_eq!(db.query(DoubleTwice(7)), 28);
        assert_eq!(db.context().executions.load(Ordering::SeqCst), 2);
    }

    #[test]
    fn invalidates_query_many_dependents_without_reordering_results() {
        let db = QueryDb::new(TestContext {
            executions: AtomicUsize::new(0),
        });

        assert_eq!(db.query(DoubleMany([2, 5])), 14);
        let invalidation = db.invalidate(Double(2));
        let invalidated = invalidation
            .invalidated
            .iter()
            .map(|frame| frame.description.as_str())
            .collect::<Vec<_>>();
        assert_eq!(invalidated, vec!["double(2)", "double_many([2, 5])"]);

        assert_eq!(db.query(DoubleMany([2, 5])), 14);
    }

    #[test]
    fn dependency_identity_does_not_merge_keys_with_same_debug_label() {
        let db = QueryDb::new(TestContext {
            executions: AtomicUsize::new(0),
        });

        assert_eq!(db.query(DebugCollisionParent(1)), 4);
        assert_eq!(db.query(DebugCollisionParent(2)), 8);

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

        assert_eq!(db.query(DebugCollisionParent(2)), 8);
        assert_eq!(db.context().executions.load(Ordering::SeqCst), 2);
        assert_eq!(db.query(DebugCollisionParent(1)), 4);
        assert_eq!(db.context().executions.load(Ordering::SeqCst), 3);
    }

    #[test]
    fn invalidation_during_query_many_prevents_stale_cache_writeback() {
        let control = Arc::new((Mutex::new(RaceState::default()), Condvar::new()));
        let db = QueryDb::new(RaceContext {
            executions: AtomicUsize::new(0),
            control: control.clone(),
        });
        let worker_db = db.clone();

        std::thread::scope(|scope| {
            let handle = scope.spawn(move || worker_db.query_many([SlowDouble(1), SlowDouble(2)]));

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
                handle.join().expect("query_many worker panicked"),
                vec![2, 4]
            );
        });

        assert_eq!(db.query(SlowDouble(1)), 2);
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
            db.query(Double(self.0)) * 2
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
            db.query_many(self.0.map(Double)).into_iter().sum()
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
            db.query_many([ParallelRecursiveChild]).into_iter().sum()
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
            db.query(ParallelRecursive)
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
            let _ = db.query(Double(3));
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
            db.query(DebugCollisionLeaf(self.0)) * 2
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
