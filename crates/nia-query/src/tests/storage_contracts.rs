use super::*;

#[test]
fn memoizes_query_values() {
    let db = QueryDb::new(TestContext {
        executions: AtomicUsize::new(0),
    });

    assert_eq!(*db.expect_get(Double(21)), 42);
    assert_eq!(*db.expect_get(Double(21)), 42);
    assert_eq!(db.context().executions.load(Ordering::SeqCst), 1);
}

#[test]
fn get_reuses_cached_value_handles() {
    let db = QueryDb::new(TestContext {
        executions: AtomicUsize::new(0),
    });

    let first = db.expect_get(Double(21));
    let second = db.expect_get(Double(21));

    assert!(Arc::ptr_eq(&first, &second));
    assert_eq!(*first, 42);
    assert_eq!(*db.expect_get(Double(21)), 42);
    assert_eq!(db.context().executions.load(Ordering::SeqCst), 1);
}

#[test]
fn get_supports_non_clone_query_values() {
    let db = QueryDb::new(TestContext {
        executions: AtomicUsize::new(0),
    });

    let first = db.expect_get(NonCloneValueQuery);
    let second = db.expect_get(NonCloneValueQuery);

    assert!(Arc::ptr_eq(&first, &second));
    assert_eq!(first.value, 42);
    assert_eq!(db.context().executions.load(Ordering::SeqCst), 1);
}

#[test]
fn single_consumer_query_moves_non_clone_value_and_tracks_parent_dependency() {
    let db = QueryDb::new(TestContext {
        executions: AtomicUsize::new(0),
    });

    assert_eq!(*db.expect_get(OwnedValueParent(21)), 42);
    assert_eq!(*db.expect_get(OwnedValueParent(21)), 42);
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
    assert_eq!(*db.expect_get(OwnedValueParent(21)), 42);
    assert_eq!(db.context().executions.load(Ordering::SeqCst), 2);
}

#[test]
fn single_consumer_query_reproduces_after_its_payload_is_consumed() {
    let db = QueryDb::new(TestContext {
        executions: AtomicUsize::new(0),
    });

    assert_eq!(db.expect_get_owned(OwnedNonCloneValueQuery(3)).value, 3);
    assert_eq!(db.expect_get_owned(OwnedNonCloneValueQuery(3)).value, 3);
    assert_eq!(db.context().executions.load(Ordering::SeqCst), 2);
}

#[test]
fn externally_published_owned_query_moves_once_and_tracks_its_predecessor() {
    let db = QueryDb::new(TestContext {
        executions: AtomicUsize::new(0),
    });
    let predecessor = OwnedNonCloneValueQuery(3);
    assert_eq!(db.expect_get_owned(predecessor).value, 3);
    let drops = Arc::new(AtomicUsize::new(0));

    db.publish_owned(
        PublishedOwnedValueQuery(3),
        PublishedOwnedValue {
            value: 9,
            drops: Arc::clone(&drops),
        },
        &predecessor,
    );
    let value = db.expect_get_owned(PublishedOwnedValueQuery(3));
    assert_eq!(value.value, 9);
    assert_eq!(drops.load(Ordering::SeqCst), 0);
    drop(value);
    assert_eq!(drops.load(Ordering::SeqCst), 1);
    assert!(db.get_owned(PublishedOwnedValueQuery(3)).is_err());
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
    assert_eq!(db.expect_get_owned(predecessor).value, 5);
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
    assert!(db.get_owned(PublishedOwnedValueQuery(5)).is_err());
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
