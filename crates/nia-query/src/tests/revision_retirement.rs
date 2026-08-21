use super::*;

#[test]
fn invalidates_direct_query_value() {
    let db = QueryDb::new(TestContext {
        executions: AtomicUsize::new(0),
    });
    assert_eq!(*db.expect_get(Double(9)), 18);
    assert_eq!(*db.expect_get(Double(9)), 18);
    assert_eq!(db.context().executions.load(Ordering::SeqCst), 1);

    let invalidation = db.invalidate(Double(9));
    assert_eq!(invalidation.invalidated.len(), 1);
    assert_eq!(invalidation.invalidated[0].description, "double(9)");
    assert_eq!(*db.expect_get(Double(9)), 18);
    assert_eq!(db.context().executions.load(Ordering::SeqCst), 2);
}

#[test]
fn retiring_query_key_removes_its_slot_and_edges_without_reusing_node_id() {
    let db = QueryDb::new(TestContext {
        executions: AtomicUsize::new(0),
    });
    let old_parent = db.expect_get(DoubleTwice(7));
    let old_node = db
        .cached_slot(&Double(7))
        .expect("cached child slot")
        .node_id;
    assert_eq!(db.query_trace().dependencies.len(), 1);

    assert!(db.retire(&Double(7)));
    let retired_trace = db.query_trace();
    assert_eq!(retired_trace.queries.len(), 1);
    assert!(retired_trace.dependencies.is_empty());
    assert_eq!(*old_parent, 28);
    assert!(
        db.inner
            .session
            .database(db.inner.id)
            .slot(old_node)
            .is_none()
    );

    let latest_parent = db.expect_get(DoubleTwice(7));
    let latest_node = db
        .cached_slot(&Double(7))
        .expect("replacement child slot")
        .node_id;
    assert_eq!(*latest_parent, 28);
    assert_ne!(old_node, latest_node);
    assert_eq!(db.context().executions.load(Ordering::SeqCst), 2);
    assert_eq!(db.query_trace().dependencies.len(), 1);
}

#[test]
fn sealing_owned_query_value_retires_its_only_predecessor_without_invalidation() {
    let db = QueryDb::new(TestContext {
        executions: AtomicUsize::new(0),
    });
    let current = db.expect_get(OwnedRevision(1));
    let predecessor = db.expect_get(OwnedRevision(0));
    let predecessor_node = db
        .cached_slot(&OwnedRevision(0))
        .expect("cached predecessor slot")
        .node_id;
    assert_eq!(&*current, &[0, 1]);
    assert_eq!(db.query_trace().dependencies.len(), 1);

    assert!(db.seal_and_retire_predecessor(&OwnedRevision(1), &OwnedRevision(0)));
    let trace = db.query_trace();
    assert_eq!(trace.queries.len(), 1);
    assert!(trace.dependencies.is_empty());
    assert!(Arc::ptr_eq(&current, &db.expect_get(OwnedRevision(1))));
    assert_eq!(&*predecessor, &[0]);
    assert!(
        db.inner
            .session
            .database(db.inner.id)
            .slot(predecessor_node)
            .is_none()
    );
    assert_eq!(db.context().executions.load(Ordering::SeqCst), 2);
}

#[test]
fn retirement_transaction_invalidates_and_retires_heterogeneous_keys_atomically() {
    let db = QueryDb::new(TestContext {
        executions: AtomicUsize::new(0),
    });
    let double = db.expect_get(Double(3));
    let owned = db.expect_get(OwnedRevision(0));
    let external_retirements = AtomicUsize::new(0);

    db.retirement_transaction(|retirement| {
        let invalidation = retirement.invalidate(Double(3));
        assert_eq!(invalidation.invalidated.len(), 1);
        assert!(retirement.retire(&Double(3)));
        assert!(retirement.retire(&OwnedRevision(0)));
        external_retirements.fetch_add(1, Ordering::SeqCst);
    });
    assert!(db.query_trace().queries.is_empty());
    assert_eq!(*double, 6);
    assert_eq!(&*owned, &[0]);
    assert_eq!(external_retirements.load(Ordering::SeqCst), 1);
}

#[test]
fn panicking_retirement_transaction_reopens_query_admission() {
    let db = QueryDb::new(TestContext {
        executions: AtomicUsize::new(0),
    });
    assert_eq!(*db.expect_get(Double(3)), 6);

    let panic = catch_unwind(AssertUnwindSafe(|| {
        db.retirement_transaction(|_| panic!("retirement fixture panic"));
    }))
    .expect_err("retirement operation should panic");
    assert!(panic.is::<&'static str>());
    assert!(
        !db.inner
            .session
            .inner
            .activity
            .lock()
            .expect("query activity lock poisoned")
            .retiring
    );

    assert_eq!(*db.expect_get(Double(4)), 8);
    assert_eq!(db.context().executions.load(Ordering::SeqCst), 2);
}

#[test]
fn retirement_waits_for_active_query_before_releasing_cached_slot() {
    let control = Arc::new((Mutex::new(RaceState::default()), Condvar::new()));
    let db = QueryDb::new(RaceContext {
        executions: AtomicUsize::new(0),
        control: Arc::clone(&control),
    });
    let worker_db = db.clone();
    let query = std::thread::spawn(move || worker_db.expect_get(SlowDouble(1)));
    let (lock, ready) = &*control;
    let mut state = lock.lock().expect("race state lock poisoned");
    while !state.started {
        state = ready.wait(state).expect("race state lock poisoned");
    }
    drop(state);

    let retirement_db = db.clone();
    let (sender, receiver) = std::sync::mpsc::channel();
    let retirement = std::thread::spawn(move || {
        sender
            .send(retirement_db.retire(&SlowDouble(1)))
            .expect("send retirement result");
    });
    let mut activity = db
        .inner
        .session
        .inner
        .activity
        .lock()
        .expect("query activity lock poisoned");
    while !activity.retiring {
        activity = db
            .inner
            .session
            .inner
            .activity_ready
            .wait(activity)
            .expect("query activity lock poisoned while waiting");
    }
    drop(activity);
    assert_eq!(
        receiver.try_recv(),
        Err(std::sync::mpsc::TryRecvError::Empty)
    );
    let trace_db = db.clone();
    let (trace_sender, trace_receiver) = std::sync::mpsc::channel();
    let trace = std::thread::spawn(move || {
        trace_sender
            .send(trace_db.query_trace())
            .expect("send query trace");
    });
    assert_eq!(
        trace_receiver.try_recv(),
        Err(std::sync::mpsc::TryRecvError::Empty)
    );

    let mut state = lock.lock().expect("race state lock poisoned");
    state.release = true;
    ready.notify_all();
    drop(state);
    let old_value = query.join().expect("query worker panicked");
    assert_eq!(receiver.recv(), Ok(true));
    retirement.join().expect("retirement worker panicked");
    assert!(
        trace_receiver
            .recv()
            .expect("receive query trace")
            .queries
            .is_empty()
    );
    trace.join().expect("query trace worker panicked");
    assert_eq!(*old_value, 2);
    assert!(db.query_trace().queries.is_empty());
}
