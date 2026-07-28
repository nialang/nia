use super::*;

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

    assert_eq!(*db.expect_get(DoubleTwice(7)), 28);
    assert_eq!(*db.expect_get(DoubleTwice(7)), 28);
    assert_eq!(db.context().executions.load(Ordering::SeqCst), 1);

    let invalidation = db.invalidate(Double(7));
    let invalidated = invalidation
        .invalidated
        .iter()
        .map(|frame| frame.description.as_str())
        .collect::<Vec<_>>();
    assert_eq!(invalidated, vec!["double(7)", "double_twice(7)"]);

    assert_eq!(*db.expect_get(DoubleTwice(7)), 28);
    assert_eq!(db.context().executions.load(Ordering::SeqCst), 2);
}

#[test]
fn invalidates_get_many_dependents_without_reordering_results() {
    let db = QueryDb::new(TestContext {
        executions: AtomicUsize::new(0),
    });

    assert_eq!(*db.expect_get(DoubleMany([2, 5])), 14);
    let invalidation = db.invalidate(Double(2));
    let invalidated = invalidation
        .invalidated
        .iter()
        .map(|frame| frame.description.as_str())
        .collect::<Vec<_>>();
    assert_eq!(invalidated, vec!["double(2)", "double_many([2, 5])"]);

    assert_eq!(*db.expect_get(DoubleMany([2, 5])), 14);
}

#[test]
fn dependency_identity_does_not_merge_keys_with_same_debug_label() {
    let db = QueryDb::new(TestContext {
        executions: AtomicUsize::new(0),
    });

    assert_eq!(*db.expect_get(DebugCollisionParent(1)), 4);
    assert_eq!(*db.expect_get(DebugCollisionParent(2)), 8);

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

    assert_eq!(*db.expect_get(DebugCollisionParent(2)), 8);
    assert_eq!(db.context().executions.load(Ordering::SeqCst), 2);
    assert_eq!(*db.expect_get(DebugCollisionParent(1)), 4);
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
        let handle = scope.spawn(move || {
            worker_db
                .get_many([SlowDouble(1), SlowDouble(2)])
                .expect("worker batch should succeed")
        });

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

    assert_eq!(*db.expect_get(SlowDouble(1)), 2);
    assert_eq!(db.context().executions.load(Ordering::SeqCst), 3);
}
