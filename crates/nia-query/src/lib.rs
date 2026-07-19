// SPDX-License-Identifier: GPL-3.0-or-later
use std::{
    any::{Any, TypeId},
    cell::RefCell,
    collections::VecDeque,
    fmt::{self, Debug},
    hash::{Hash, Hasher},
    sync::{
        Arc, Condvar, Mutex,
        atomic::{AtomicUsize, Ordering},
    },
};

use nia_hash::{FastHashMap, FastHashSet, FastHasher};

const DEFAULT_MAX_QUERY_MANY_THREADS: usize = 4;
const QUERY_THREADS_ENV: &str = "NIA_QUERY_THREADS";

pub trait QueryKey<C>: Clone + Debug + Eq + Hash + Send + Sync + 'static {
    type Value: Send + Sync + 'static;

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
    timings: nia_timing::TimingMode,
    caches: Mutex<FastHashMap<TypeId, Box<dyn Any + Send + Sync>>>,
    slots: Mutex<FastHashMap<QueryFrameIdentity, Arc<dyn ErasedQuerySlot>>>,
    dependencies: Mutex<QueryDependencyGraph>,
}

struct QuerySlot<V> {
    identity: QueryFrameIdentity,
    stats: QuerySlotStats,
    state: Mutex<QueryState<V>>,
    ready: Condvar,
}

#[derive(Debug, Default)]
struct QuerySlotStats {
    executions: AtomicUsize,
    cache_hits: AtomicUsize,
    waits: AtomicUsize,
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

    fn snapshot(&self) -> QueryFrameStats {
        QueryFrameStats {
            executions: self.executions.load(Ordering::Relaxed),
            cache_hits: self.cache_hits.load(Ordering::Relaxed),
            waits: self.waits.load(Ordering::Relaxed),
        }
    }
}

enum QueryState<V> {
    Empty,
    Computing { invalidated: bool },
    Ready(Arc<V>),
}

trait ErasedQuerySlot: Send + Sync {
    fn invalidate(&self);
    fn stats(&self) -> QueryFrameStats;
}

impl<V> ErasedQuerySlot for QuerySlot<V>
where
    V: Send + Sync + 'static,
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

#[derive(Clone)]
struct QueryFrameIdentity {
    type_id: TypeId,
    name: &'static str,
    key_hash: u64,
    key: Arc<dyn ErasedQueryKey>,
    make_frame: fn(&dyn ErasedQueryKey) -> QueryFrame,
}

impl Debug for QueryFrameIdentity {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("QueryFrameIdentity")
            .field("name", &self.name)
            .field("type_id", &self.type_id)
            .field("key_hash", &self.key_hash)
            .finish()
    }
}

impl QueryFrameIdentity {
    fn frame(&self) -> QueryFrame {
        (self.make_frame)(self.key.as_ref())
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
    forward: FastHashMap<QueryFrameIdentity, FastHashSet<QueryFrameIdentity>>,
    reverse: FastHashMap<QueryFrameIdentity, FastHashSet<QueryFrameIdentity>>,
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
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct QueryTraceQuery {
    pub frame: QueryFrame,
    pub stats: QueryFrameStats,
}

#[derive(Debug, Clone)]
struct QueryStackEntry {
    identity: QueryFrameIdentity,
    dependencies: FastHashSet<QueryFrameIdentity>,
}

struct QueryStackGuard {
    active: bool,
}

struct QueryStackInstallGuard {
    previous: Vec<QueryStackEntry>,
}

thread_local! {
    static QUERY_STACK: RefCell<Vec<QueryStackEntry>> = const { RefCell::new(Vec::new()) };
}

impl<C> QueryDb<C> {
    pub fn new(context: C) -> Self {
        Self::new_with_timings(context, nia_timing::TimingMode::Off)
    }

    pub fn new_with_timings(context: C, timings: nia_timing::TimingMode) -> Self {
        Self {
            inner: Arc::new(QueryDbInner {
                context,
                timings,
                caches: Mutex::new(FastHashMap::default()),
                slots: Mutex::new(FastHashMap::default()),
                dependencies: Mutex::new(QueryDependencyGraph::default()),
            }),
        }
    }

    pub fn context(&self) -> &C {
        &self.inner.context
    }

    pub fn get<K>(&self, key: K) -> Arc<K::Value>
    where
        K: QueryKey<C>,
    {
        self.try_get(key)
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

    fn try_get_cached<K>(&self, key: K) -> QueryResult<Arc<K::Value>>
    where
        K: QueryKey<C>,
    {
        let detail_timing = self.inner.timings.detail();
        let slot = nia_timing::time_detail(detail_timing, "query.slot_for", || self.slot_for(&key));
        let identity = &slot.identity;
        nia_timing::time_detail(detail_timing, "query.record_dependency", || {
            record_dependency_on_current_stack(identity)
        });
        loop {
            let mut state = slot.state.lock().expect("query cache lock poisoned");
            match &*state {
                QueryState::Ready(value) => {
                    nia_timing::time_detail(detail_timing, "query.record_cache_hit", || {
                        slot.stats.record_cache_hit()
                    });
                    return Ok(Arc::clone(value));
                }
                QueryState::Computing { .. } => {
                    self.check_not_recursive_identity(identity)?;
                    nia_timing::time_detail(detail_timing, "query.record_wait", || {
                        slot.stats.record_wait()
                    });
                    drop(
                        slot.ready
                            .wait(state)
                            .expect("query cache lock poisoned while waiting"),
                    );
                }
                QueryState::Empty => {
                    *state = QueryState::Computing { invalidated: false };
                    drop(state);

                    self.clear_dependencies_from(identity);
                    let entry = QueryStackEntry {
                        identity: identity.clone(),
                        dependencies: FastHashSet::default(),
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
                            self.clear_dependencies_from(identity);
                            slot.ready.notify_all();
                            drop(state);
                            match payload.downcast::<QueryError>() {
                                Ok(err) => return Err(*err),
                                Err(payload) => std::panic::resume_unwind(payload),
                            }
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
                        self.clear_dependencies_from(identity);
                    } else {
                        let dependencies = guard.take_dependencies();
                        self.replace_dependencies_from(identity, dependencies);
                        *state = QueryState::Ready(cached);
                    }
                    slot.ready.notify_all();
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
        let keys = keys.into_iter().collect::<Vec<_>>();
        if keys.is_empty() {
            return Vec::new();
        }
        let parent_stack = current_query_stack();
        let worker_count = batch_worker_count(keys.len());
        if worker_count == 1 {
            return keys.into_iter().map(|key| self.get(key)).collect();
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
                                .expect("get_many work queue lock poisoned")
                                .pop_front();
                            let Some((index, key)) = work else {
                                return (values, take_current_stack_dependencies());
                            };
                            values.push((index, db.get(key)));
                        }
                    })
                })
                .collect::<Vec<_>>();
            let mut values = Vec::new();
            let mut dependencies = FastHashSet::default();
            for handle in handles {
                match handle.join() {
                    Ok((worker_values, worker_dependencies)) => {
                        values.extend(worker_values);
                        dependencies.extend(worker_dependencies);
                    }
                    Err(payload) => std::panic::resume_unwind(payload),
                }
            }
            merge_dependencies_into_current_stack(dependencies);
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
                .dependencies(),
            queries: self.query_stats(),
        }
    }

    pub fn invalidate<K>(&self, key: K) -> QueryInvalidation
    where
        K: QueryKey<C>,
    {
        let root = query_frame_identity::<C, K>(&key);
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
            .map(QueryFrameIdentity::frame)
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
        let mut caches = self.inner.caches.lock().expect("query cache lock poisoned");
        let cache = caches
            .entry(TypeId::of::<K>())
            .or_insert_with(|| {
                Box::new(Mutex::new(
                    FastHashMap::<K, Arc<QuerySlot<K::Value>>>::default(),
                ))
            })
            .downcast_ref::<Mutex<FastHashMap<K, Arc<QuerySlot<K::Value>>>>>()
            .expect("query cache type mismatch");
        let mut cache = cache.lock().expect("query cache lock poisoned");
        if let Some(slot) = cache.get(key) {
            return slot.clone();
        }
        let identity = query_frame_identity::<C, K>(key);
        let slot = Arc::new(QuerySlot {
            identity: identity.clone(),
            stats: QuerySlotStats::default(),
            state: Mutex::new(QueryState::Empty),
            ready: Condvar::new(),
        });
        cache.insert(key.clone(), slot.clone());
        self.inner
            .slots
            .lock()
            .expect("query cache slot lock poisoned")
            .insert(identity, slot.clone() as Arc<dyn ErasedQuerySlot>);
        slot
    }

    fn enter_query(&self, entry: QueryStackEntry) -> QueryResult<QueryStackGuard> {
        self.check_not_recursive_identity(&entry.identity)?;
        QUERY_STACK.with(|stack| {
            stack.borrow_mut().push(entry);
        });
        Ok(QueryStackGuard { active: true })
    }

    fn check_not_recursive_identity(&self, identity: &QueryFrameIdentity) -> QueryResult<()> {
        QUERY_STACK.with(|stack| {
            let stack = stack.borrow();
            if let Some(position) = stack.iter().position(|entry| &entry.identity == identity) {
                let mut cycle = stack[position..]
                    .iter()
                    .map(|entry| entry.identity.frame())
                    .collect::<Vec<_>>();
                cycle.push(
                    stack
                        .iter()
                        .find(|entry| &entry.identity == identity)
                        .map(|entry| entry.identity.frame())
                        .unwrap_or_else(|| identity.frame()),
                );
                return Err(QueryError::Cycle { cycle });
            }
            Ok(())
        })
    }

    fn query_stats(&self) -> Vec<QueryTraceQuery> {
        let slots = self
            .inner
            .slots
            .lock()
            .expect("query cache slot lock poisoned");
        let mut queries = slots
            .iter()
            .map(|(identity, slot)| QueryTraceQuery {
                frame: identity.frame(),
                stats: slot.stats(),
            })
            .collect::<Vec<_>>();
        queries.sort_by(|lhs, rhs| {
            (lhs.frame.name, lhs.frame.key.as_str()).cmp(&(rhs.frame.name, rhs.frame.key.as_str()))
        });
        queries
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

    fn replace_dependencies_from(
        &self,
        from: &QueryFrameIdentity,
        targets: FastHashSet<QueryFrameIdentity>,
    ) {
        self.inner
            .dependencies
            .lock()
            .expect("query dependency lock poisoned")
            .replace_dependencies_from(from, targets);
    }
}

fn batch_worker_count(work_items: usize) -> usize {
    if work_items <= 1 {
        return work_items;
    }
    let configured = std::env::var(QUERY_THREADS_ENV)
        .ok()
        .and_then(|value| value.parse::<usize>().ok())
        .filter(|value| *value > 0);
    let available = configured.unwrap_or_else(default_batch_threads);
    available.clamp(1, work_items)
}

fn default_batch_threads() -> usize {
    let available = std::thread::available_parallelism()
        .map(usize::from)
        .unwrap_or(1);
    available
        .div_ceil(2)
        .clamp(1, DEFAULT_MAX_QUERY_MANY_THREADS)
}

impl QueryDependencyGraph {
    fn replace_dependencies_from(
        &mut self,
        from: &QueryFrameIdentity,
        targets: FastHashSet<QueryFrameIdentity>,
    ) {
        self.remove_dependencies_from(from);
        if targets.is_empty() {
            return;
        }
        for target in &targets {
            self.reverse
                .entry(target.clone())
                .or_default()
                .insert(from.clone());
        }
        self.forward.insert(from.clone(), targets);
    }

    fn dependencies(&self) -> Vec<QueryDependency> {
        let mut dependencies = self
            .forward
            .iter()
            .flat_map(|(from, targets)| {
                targets.iter().map(move |to| QueryDependency {
                    from: from.frame(),
                    to: to.frame(),
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

    fn collect_dependents(&self, root: QueryFrameIdentity) -> Vec<QueryFrameIdentity> {
        let mut seen = FastHashSet::default();
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
                let frame = dependent.frame();
                (frame.name, frame.key)
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

fn query_frame_identity<C, K>(key: &K) -> QueryFrameIdentity
where
    K: QueryKey<C>,
{
    let mut hasher = FastHasher::default();
    key.hash(&mut hasher);
    // The hash is only a fast prefilter. Equality still compares the typed key
    // through `ErasedQueryKey`, so debug labels are not part of identity.
    QueryFrameIdentity {
        type_id: TypeId::of::<K>(),
        name: K::name(),
        key_hash: hasher.finish(),
        key: Arc::new(key.clone()),
        make_frame: query_frame_from_erased::<C, K>,
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

    fn take_dependencies(&mut self) -> FastHashSet<QueryFrameIdentity> {
        if !self.active {
            return FastHashSet::default();
        }
        self.active = false;
        QUERY_STACK.with(|stack| {
            stack
                .borrow_mut()
                .pop()
                .map(|entry| entry.dependencies)
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

fn take_current_stack_dependencies() -> FastHashSet<QueryFrameIdentity> {
    QUERY_STACK.with(|stack| {
        stack
            .borrow_mut()
            .last_mut()
            .map(|entry| std::mem::take(&mut entry.dependencies))
            .unwrap_or_default()
    })
}

fn record_dependency_on_current_stack(to: &QueryFrameIdentity) {
    QUERY_STACK.with(|stack| {
        let mut stack = stack.borrow_mut();
        let Some(from) = stack.last_mut() else {
            return;
        };
        from.dependencies.insert(to.clone());
    });
}

fn merge_dependencies_into_current_stack(dependencies: FastHashSet<QueryFrameIdentity>) {
    if dependencies.is_empty() {
        return;
    }
    QUERY_STACK.with(|stack| {
        let mut stack = stack.borrow_mut();
        let Some(entry) = stack.last_mut() else {
            return;
        };
        entry.dependencies.extend(dependencies);
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
    fn default_batch_threads_is_bounded() {
        let count = default_batch_threads();

        assert!(count >= 1);
        assert!(count <= DEFAULT_MAX_QUERY_MANY_THREADS);
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
