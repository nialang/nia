// SPDX-License-Identifier: GPL-3.0-or-later
use std::{
    any::{Any, TypeId},
    cell::RefCell,
    collections::VecDeque,
    fmt::{self, Debug},
    hash::Hash,
    sync::{
        Arc, Condvar, Mutex, Weak,
        atomic::{AtomicU32, AtomicU64, AtomicUsize, Ordering},
    },
};

use nia_hash::{FastHashMap, FastHashSet};

const DEFAULT_MAX_QUERY_MANY_THREADS: usize = 4;
const QUERY_THREADS_ENV: &str = "NIA_QUERY_THREADS";

pub trait QueryKey<C>: Clone + Debug + Eq + Hash + Send + Sync + 'static {
    type Value: Send + Sync + 'static;

    const FINGERPRINT: QueryFingerprintPolicy = QueryFingerprintPolicy::None;

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
                provider: QueryProviderPolicy::KeyExecute,
                fingerprint: K::FINGERPRINT,
                storage: QueryStoragePolicy::CacheOwnedArc,
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

#[derive(Clone)]
pub struct QuerySession {
    inner: Arc<QuerySessionInner>,
}

struct QuerySessionInner {
    id: QuerySessionId,
    databases: Mutex<FastHashMap<QueryDbId, Arc<dyn ErasedQueryDatabase>>>,
    dependencies: Mutex<QueryDependencyGraph>,
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

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
struct QueryNodeId {
    db_id: QueryDbId,
    index: u32,
}

struct QuerySlotTable<C> {
    entries: Vec<QuerySlotRecord<C>>,
}

impl<C> Default for QuerySlotTable<C> {
    fn default() -> Self {
        Self {
            entries: Vec::new(),
        }
    }
}

struct QuerySlotRecord<C> {
    identity: QuerySlotIdentity,
    slot: Arc<dyn ErasedQuerySlot>,
    ensure: fn(&QueryDb<C>, &dyn ErasedQueryKey) -> QueryResult<()>,
}

impl<C> QuerySlotTable<C> {
    fn next_id(&self, db_id: QueryDbId) -> QueryNodeId {
        let index = u32::try_from(self.entries.len()).expect("query node identity space exhausted");
        QueryNodeId { db_id, index }
    }

    fn push(
        &mut self,
        node_id: QueryNodeId,
        identity: QuerySlotIdentity,
        slot: Arc<dyn ErasedQuerySlot>,
        ensure: fn(&QueryDb<C>, &dyn ErasedQueryKey) -> QueryResult<()>,
    ) {
        assert_eq!(node_id.index as usize, self.entries.len());
        self.entries.push(QuerySlotRecord {
            identity,
            slot,
            ensure,
        });
    }

    fn get(&self, db_id: QueryDbId, node_id: QueryNodeId) -> Option<&QuerySlotRecord<C>> {
        if node_id.db_id != db_id {
            return None;
        }
        self.entries.get(node_id.index as usize)
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
            QueryState::Empty => {}
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
            QueryState::Empty | QueryState::Ready { .. } => {
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

thread_local! {
    static QUERY_STACK: RefCell<Vec<QueryStackEntry>> = const { RefCell::new(Vec::new()) };
}

impl Default for QuerySession {
    fn default() -> Self {
        Self::new()
    }
}

impl QuerySession {
    pub fn new() -> Self {
        Self {
            inner: Arc::new(QuerySessionInner {
                id: QuerySessionId::fresh(),
                databases: Mutex::new(FastHashMap::default()),
                dependencies: Mutex::new(QueryDependencyGraph::default()),
            }),
        }
    }

    pub fn ptr_eq(&self, other: &Self) -> bool {
        Arc::ptr_eq(&self.inner, &other.inner)
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
        let node_id = slot.node_id;
        nia_timing::time_detail(detail_timing, "query.record_dependency", || {
            record_dependency_on_current_stack(self.inner.session.inner.id, node_id)
        });
        let mut stale_value = None;
        loop {
            let mut state = slot.state.lock().expect("query cache lock poisoned");
            match &*state {
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
        let keys = keys.into_iter().collect::<Vec<_>>();
        if keys.is_empty() {
            return Vec::new();
        }
        let parent_stack = current_query_stack();
        let records_fingerprints = parent_stack
            .last()
            .is_some_and(|entry| entry.dependency_fingerprints.is_some());
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
            let mut dependencies = RecordedDependencies {
                nodes: FastHashSet::default(),
                fingerprints: records_fingerprints.then(DependencyFingerprints::default),
            };
            for handle in handles {
                match handle.join() {
                    Ok((worker_values, worker_dependencies)) => {
                        values.extend(worker_values);
                        dependencies.nodes.extend(worker_dependencies.nodes);
                        if let (Some(dependencies), Some(worker_dependencies)) = (
                            dependencies.fingerprints.as_mut(),
                            worker_dependencies.fingerprints,
                        ) {
                            dependencies.extend(worker_dependencies);
                        }
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
                QueryState::Empty => return QueryInvalidation::default(),
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
            .enumerate()
            .map(|(index, record)| QueryTraceQuery {
                frame: slots.frame(
                    db_id,
                    QueryNodeId {
                        db_id,
                        index: index as u32,
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
    db.try_get(key.clone()).map(drop)
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
            let record = slots
                .get(inner.id, node_id)
                .expect("query dependency must reference a registered slot");
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
        Condvar,
        atomic::{AtomicUsize, Ordering},
    };

    struct TestContext {
        executions: AtomicUsize,
    }

    struct SessionInputContext {
        value: Arc<AtomicUsize>,
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
