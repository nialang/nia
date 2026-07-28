#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
struct DuplicateDoubleName;

impl QueryKey<TestContext> for DuplicateDoubleName {
    type Value = usize;

    fn name() -> &'static str {
        "double"
    }

    fn execute_result(&self, _db: &QueryDb<TestContext>) -> QueryResult<Self::Value> {
        Ok(0)
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

    fn execute_result(&self, db: &QueryDb<TestContext>) -> QueryResult<Self::Value> {
        db.context().executions.fetch_add(1, Ordering::SeqCst);
        Ok(NonCloneValue { value: 42 })
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

    fn execute_result(&self, db: &QueryDb<TestContext>) -> QueryResult<Self::Value> {
        db.context().executions.fetch_add(1, Ordering::SeqCst);
        Ok(OwnedNonCloneValue { value: self.0 })
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
struct OwnedValueBatchParent;

impl QueryKey<TestContext> for OwnedValueBatchParent {
    type Value = usize;

    fn name() -> &'static str {
        "owned_value_batch_parent"
    }

    fn execute_result(&self, db: &QueryDb<TestContext>) -> QueryResult<Self::Value> {
        Ok(db
            .get_many_owned([
                OwnedNonCloneValueQuery(2),
                OwnedNonCloneValueQuery(5),
                OwnedNonCloneValueQuery(3),
            ])?
            .into_iter()
            .map(|value| value.value)
            .sum())
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

    fn execute_result(&self, _db: &QueryDb<TestContext>) -> QueryResult<Self::Value> {
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

    fn execute_result(&self, db: &QueryDb<TestContext>) -> QueryResult<Self::Value> {
        Ok(db.get_owned(OwnedNonCloneValueQuery(self.0))?.value * 2)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
struct Recursive;

impl QueryKey<TestContext> for Recursive {
    type Value = usize;

    fn name() -> &'static str {
        "recursive"
    }

    fn execute_result(&self, db: &QueryDb<TestContext>) -> QueryResult<Self::Value> {
        Ok(*db.get(Recursive)?)
    }
}
