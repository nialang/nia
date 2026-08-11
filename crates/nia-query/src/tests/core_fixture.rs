trait QueryDbTestExt<C> {
    fn expect_get<K>(&self, key: K) -> Arc<K::Value>
    where
        K: QueryKey<C>;

    fn expect_get_owned<K>(&self, key: K) -> K::Value
    where
        K: QueryKey<C>;
}

impl<C> QueryDbTestExt<C> for QueryDb<C> {
    fn expect_get<K>(&self, key: K) -> Arc<K::Value>
    where
        K: QueryKey<C>,
    {
        self.get(key).expect("test query must succeed")
    }

    fn expect_get_owned<K>(&self, key: K) -> K::Value
    where
        K: QueryKey<C>,
    {
        self.get_owned(key).expect("test query must succeed")
    }
}

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

    fn execute_result(&self, db: &QueryDb<SessionInputContext>) -> QueryResult<Self::Value> {
        Ok(db.context().value.load(Ordering::SeqCst))
    }

    fn fingerprint(&self, value: &Self::Value) -> Option<QueryFingerprint> {
        Some(test_usize_fingerprint(
            FingerprintDomain::new("nia.query.test.session-input.v1"),
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

    fn execute_result(&self, db: &QueryDb<SessionParentContext>) -> QueryResult<Self::Value> {
        db.context().executions.fetch_add(1, Ordering::SeqCst);
        Ok(*db.context().input_db.get(SessionInput)? * 2)
    }

    fn fingerprint(&self, value: &Self::Value) -> Option<QueryFingerprint> {
        Some(test_usize_fingerprint(
            FingerprintDomain::new("nia.query.test.session-parent.v1"),
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

    fn execute_result(
        &self,
        db: &QueryDb<CrossSessionBatchContext>,
    ) -> QueryResult<Self::Value> {
        Ok(db
            .context()
            .input_db
            .get_many([Double(2), Double(5)])?
            .into_iter()
            .map(|value| *value)
            .sum())
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

    fn execute_result(&self, db: &QueryDb<TestContext>) -> QueryResult<Self::Value> {
        db.context().executions.fetch_add(1, Ordering::SeqCst);
        Ok(self.0 * 2)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
struct OwnedRevision(usize);

impl QueryKey<TestContext> for OwnedRevision {
    type Value = Vec<usize>;

    fn name() -> &'static str {
        "owned_revision"
    }

    fn execute_result(&self, db: &QueryDb<TestContext>) -> QueryResult<Self::Value> {
        db.context().executions.fetch_add(1, Ordering::SeqCst);
        if self.0 == 0 {
            return Ok(vec![0]);
        }
        let mut value = db.get(Self(self.0 - 1))?.as_ref().clone();
        value.push(self.0);
        Ok(value)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
struct ExecutorProbe(usize);

impl QueryKey<ExecutorProbeContext> for ExecutorProbe {
    type Value = usize;

    fn name() -> &'static str {
        "executor_probe"
    }

    fn execute_result(&self, db: &QueryDb<ExecutorProbeContext>) -> QueryResult<Self::Value> {
        let active = db.context().active.fetch_add(1, Ordering::SeqCst) + 1;
        db.context().peak_active.fetch_max(active, Ordering::SeqCst);
        db.context().barrier.wait();
        db.context().active.fetch_sub(1, Ordering::SeqCst);
        Ok(self.0)
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

    fn execute_result(&self, db: &QueryDb<ExecutorProbeContext>) -> QueryResult<Self::Value> {
        let active = db.context().active.fetch_add(1, Ordering::SeqCst) + 1;
        db.context().peak_active.fetch_max(active, Ordering::SeqCst);
        db.context().barrier.wait();
        db.context().active.fetch_sub(1, Ordering::SeqCst);
        Ok(self.0)
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

    fn execute_result(&self, db: &QueryDb<CompletionOrderContext>) -> QueryResult<Self::Value> {
        while db.context().phase.load(Ordering::SeqCst) != self.0 {
            std::thread::yield_now();
        }
        Ok(self.0)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
struct FallibleOwnedCompletionProbe(usize);

impl QueryKey<TestContext> for FallibleOwnedCompletionProbe {
    type Value = usize;

    const STORAGE: QueryStoragePolicy = QueryStoragePolicy::SingleConsumerOwned;

    fn name() -> &'static str {
        "fallible_owned_completion_probe"
    }

    fn execute_result(&self, db: &QueryDb<TestContext>) -> QueryResult<Self::Value> {
        if self.0 == 1 {
            Err(db.invalid_input(self, "rejected completion"))
        } else {
            Ok(self.0)
        }
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

    fn execute_result(&self, db: &QueryDb<BatchIsolationContext>) -> QueryResult<Self::Value> {
        Ok(match self {
            Self::Parent => db
                .get_many([Self::Child, Self::ChildWait])?
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
            Self::DependsOnParent => *db.get(Self::Parent)?,
            Self::OtherFiller => 4,
        })
    }
}
