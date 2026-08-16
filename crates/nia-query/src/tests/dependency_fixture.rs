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

    fn execute_result(&self, db: &QueryDb<TestContext>) -> QueryResult<Self::Value> {
        Ok(*db.get(Double(self.0))? * 2)
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

    fn execute_result(&self, db: &QueryDb<TestContext>) -> QueryResult<Self::Value> {
        Ok(db
            .get_many(self.0.map(Double))?
            .into_iter()
            .map(|value| *value)
            .sum())
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

    fn execute_result(&self, db: &QueryDb<TestContext>) -> QueryResult<Self::Value> {
        Ok(db
            .get_many([Double(self.0)])?
            .into_iter()
            .map(|value| *value)
            .sum())
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
struct ParallelRecursive;

impl QueryKey<TestContext> for ParallelRecursive {
    type Value = usize;

    fn name() -> &'static str {
        "parallel_recursive"
    }

    fn execute_result(&self, db: &QueryDb<TestContext>) -> QueryResult<Self::Value> {
        Ok(db
            .get_many_with([ParallelRecursiveChild], QueryDb::get)
            .into_iter()
            .collect::<QueryResult<Vec<_>>>()?
            .into_iter()
            .map(|value| *value)
            .sum())
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
struct ParallelRecursiveChild;

impl QueryKey<TestContext> for ParallelRecursiveChild {
    type Value = usize;

    fn name() -> &'static str {
        "parallel_recursive_child"
    }

    fn execute_result(&self, db: &QueryDb<TestContext>) -> QueryResult<Self::Value> {
        Ok(*db.get(ParallelRecursive)?)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
enum ParallelCycle {
    Left,
    Right,
}

impl QueryKey<TestContext> for ParallelCycle {
    type Value = usize;

    fn name() -> &'static str {
        "parallel_cycle"
    }

    fn description(&self) -> String {
        format!("parallel_cycle::{self:?}")
    }

    fn execute_result(&self, db: &QueryDb<TestContext>) -> QueryResult<Self::Value> {
        let dependency = match self {
            Self::Left => Self::Right,
            Self::Right => Self::Left,
        };
        Ok(*db.get(dependency)?)
    }
}

struct CrossSessionCycleContext {
    other: Arc<Mutex<Option<QueryDb<Self>>>>,
    barrier: Arc<Barrier>,
    executions: AtomicUsize,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
enum CrossSessionCycle {
    Left,
    Right,
}

impl QueryKey<CrossSessionCycleContext> for CrossSessionCycle {
    type Value = usize;

    fn name() -> &'static str {
        "cross_session_cycle"
    }

    fn description(&self) -> String {
        format!("cross_session_cycle::{self:?}")
    }

    fn execute_result(
        &self,
        db: &QueryDb<CrossSessionCycleContext>,
    ) -> QueryResult<Self::Value> {
        if db.context().executions.fetch_add(1, Ordering::SeqCst) == 0 {
            db.context().barrier.wait();
        }
        let other = db
            .context()
            .other
            .lock()
            .expect("cross-session cycle link lock poisoned")
            .clone()
            .expect("cross-session cycle link must be installed");
        let dependency = match self {
            Self::Left => Self::Right,
            Self::Right => Self::Left,
        };
        Ok(*other.get(dependency)?)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
struct PanicsOnce;

impl QueryKey<TestContext> for PanicsOnce {
    type Value = usize;

    fn name() -> &'static str {
        "panics_once"
    }

    fn execute_result(&self, db: &QueryDb<TestContext>) -> QueryResult<Self::Value> {
        let previous = db.context().executions.fetch_add(1, Ordering::SeqCst);
        if previous == 0 {
            panic!("transient query failure");
        }
        Ok(99)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
struct InvalidInputQuery;

impl QueryKey<TestContext> for InvalidInputQuery {
    type Value = usize;

    fn name() -> &'static str {
        "invalid_input_query"
    }

    fn execute_result(&self, db: &QueryDb<TestContext>) -> QueryResult<Self::Value> {
        Err(db.invalid_input(self, "bad fixture"))
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
struct InvalidAfterDependency;

impl QueryKey<TestContext> for InvalidAfterDependency {
    type Value = usize;

    fn name() -> &'static str {
        "invalid_after_dependency"
    }

    fn execute_result(&self, db: &QueryDb<TestContext>) -> QueryResult<Self::Value> {
        let _ = db.get(Double(3))?;
        Err(db.invalid_input(self, "failed after dependency"))
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

    fn execute_result(&self, db: &QueryDb<TestContext>) -> QueryResult<Self::Value> {
        Ok(*db.get(DebugCollisionLeaf(self.0))? * 2)
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

    fn execute_result(&self, db: &QueryDb<TestContext>) -> QueryResult<Self::Value> {
        db.context().executions.fetch_add(1, Ordering::SeqCst);
        Ok(self.0 * 2)
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

    fn execute_result(&self, db: &QueryDb<RaceContext>) -> QueryResult<Self::Value> {
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
        Ok(self.0 * 2)
    }
}
