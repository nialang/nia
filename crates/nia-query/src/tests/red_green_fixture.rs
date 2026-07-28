#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
struct StableInput;

impl QueryKey<TestContext> for StableInput {
    type Value = usize;

    const FINGERPRINT: QueryFingerprintPolicy = QueryFingerprintPolicy::StableValue;

    fn name() -> &'static str {
        "stable_input"
    }

    fn execute_result(&self, db: &QueryDb<TestContext>) -> QueryResult<Self::Value> {
        Ok(db.context().executions.load(Ordering::SeqCst))
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

    fn execute_result(&self, db: &QueryDb<TestContext>) -> QueryResult<Self::Value> {
        Ok(*db.get(StableInput)? * 2)
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

    fn execute_result(&self, db: &QueryDb<RedGreenContext>) -> QueryResult<Self::Value> {
        Ok(db.context().input.load(Ordering::SeqCst))
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

    fn execute_result(&self, db: &QueryDb<RedGreenContext>) -> QueryResult<Self::Value> {
        db.context()
            .derived_executions
            .fetch_add(1, Ordering::SeqCst);
        Ok(*db.get(RedGreenInput)? % 2)
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

    fn execute_result(&self, db: &QueryDb<RedGreenContext>) -> QueryResult<Self::Value> {
        db.context()
            .parent_executions
            .fetch_add(1, Ordering::SeqCst);
        Ok(*db.get(StableParity)? + 10)
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

    fn execute_result(&self, db: &QueryDb<RedGreenContext>) -> QueryResult<Self::Value> {
        db.context()
            .derived_executions
            .fetch_add(1, Ordering::SeqCst);
        Ok(*db.get(RedGreenInput)? % 2)
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

    fn execute_result(&self, db: &QueryDb<RedGreenContext>) -> QueryResult<Self::Value> {
        db.context()
            .parent_executions
            .fetch_add(1, Ordering::SeqCst);
        Ok(*db.get(SemanticParity)? + 10)
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

    fn execute_result(&self, db: &QueryDb<RedGreenContext>) -> QueryResult<Self::Value> {
        db.context()
            .derived_executions
            .fetch_add(1, Ordering::SeqCst);
        Ok(*db.get(RedGreenInput)? % self.0)
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

    fn execute_result(&self, db: &QueryDb<RedGreenContext>) -> QueryResult<Self::Value> {
        db.context()
            .parent_executions
            .fetch_add(1, Ordering::SeqCst);
        Ok(db
            .get_many([StableModulo(2), StableModulo(3)])?
            .into_iter()
            .map(|value| *value)
            .sum())
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

    fn execute_result(&self, db: &QueryDb<ValidationRaceContext>) -> QueryResult<Self::Value> {
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
        Ok(db.context().input.load(Ordering::SeqCst))
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

    fn execute_result(&self, db: &QueryDb<ValidationRaceContext>) -> QueryResult<Self::Value> {
        db.context()
            .derived_executions
            .fetch_add(1, Ordering::SeqCst);
        Ok(*db.get(ValidationRaceInput)? % 2)
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
