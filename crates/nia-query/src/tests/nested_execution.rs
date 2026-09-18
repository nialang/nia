use super::*;

#[test]
fn nested_batches_across_sessions_reuse_the_current_process_permit() {
    let execution_budget = Arc::new(QueryExecutionBudget::owned(1));
    let input_session = QuerySession::with_execution_budget(1, Arc::clone(&execution_budget));
    let parent_session = QuerySession::with_execution_budget(1, Arc::clone(&execution_budget));
    let input_db = QueryDb::new_with_timings_in_session(
        TestContext {
            executions: AtomicUsize::new(0),
        },
        nia_timing::TimingMode::Off,
        input_session,
    );
    let parent_db = QueryDb::new_with_timings_in_session(
        CrossSessionBatchContext { input_db },
        nia_timing::TimingMode::Off,
        parent_session,
    );
    let (sender, receiver) = std::sync::mpsc::channel();

    std::thread::spawn(move || {
        let values = parent_db
            .get_many([CrossSessionBatch])
            .expect("cross-session batch should succeed");
        sender
            .send(values.into_iter().map(|value| *value).collect::<Vec<_>>())
            .expect("send cross-session batch result");
    });

    assert_eq!(
        receiver.recv_timeout(std::time::Duration::from_secs(2)),
        Ok(vec![14])
    );
    assert_eq!(execution_budget.peak_active(), 1);
}

#[test]
fn nested_get_many_completes_with_full_session_budget() {
    let session = QuerySession::with_parallelism(2);
    let db = QueryDb::new_with_timings_in_session(
        TestContext {
            executions: AtomicUsize::new(0),
        },
        nia_timing::TimingMode::Off,
        session,
    );
    let (sender, receiver) = std::sync::mpsc::channel();

    std::thread::spawn(move || {
        let values = db
            .get_many([DoubleMany([1, 2]), DoubleMany([3, 4])])
            .expect("nested batch should succeed");
        sender
            .send(values.into_iter().map(|value| *value).collect::<Vec<_>>())
            .expect("send nested batch result");
    });

    assert_eq!(
        receiver.recv_timeout(std::time::Duration::from_secs(2)),
        Ok(vec![6, 14])
    );
}

#[test]
fn batch_waiter_does_not_run_tasks_that_depend_on_its_paused_query() {
    let session = QuerySession::with_parallelism(1);
    let db = QueryDb::new_with_timings_in_session(
        BatchIsolationContext {
            session: Mutex::new(Some(session.clone())),
            child_started: Mutex::new(false),
            child_ready: Condvar::new(),
        },
        nia_timing::TimingMode::Off,
        session,
    );
    let (first_sender, first_receiver) = std::sync::mpsc::channel();
    let (second_sender, second_receiver) = std::sync::mpsc::channel();
    let second_db = db.clone();
    std::thread::spawn(move || {
        let mut started = second_db.context().child_started.lock();
        while !*started {
            second_db.context().child_ready.wait(&mut started);
        }
        drop(started);
        let values = second_db
            .get_many([
                BatchIsolationQuery::DependsOnParent,
                BatchIsolationQuery::OtherFiller,
            ])
            .expect("second isolated batch should succeed");
        second_sender
            .send(values.into_iter().map(|value| *value).collect::<Vec<_>>())
            .expect("send second isolated batch result");
    });
    std::thread::spawn(move || {
        let values = db
            .get_many([
                BatchIsolationQuery::Parent,
                BatchIsolationQuery::OuterFiller,
            ])
            .expect("first isolated batch should succeed");
        first_sender
            .send(values.into_iter().map(|value| *value).collect::<Vec<_>>())
            .expect("send first isolated batch result");
    });

    assert_eq!(
        first_receiver.recv_timeout(std::time::Duration::from_secs(10)),
        Ok(vec![3, 0])
    );
    assert_eq!(
        second_receiver.recv_timeout(std::time::Duration::from_secs(10)),
        Ok(vec![3, 4])
    );
}

#[test]
fn get_many_panic_becomes_internal_error_and_taints_session() {
    let session = QuerySession::with_parallelism(2);
    let db = QueryDb::new_with_timings_in_session(
        TestContext {
            executions: AtomicUsize::new(0),
        },
        nia_timing::TimingMode::Off,
        session,
    );

    let failure = db
        .get_many([PanicsOnce, PanicsOnce])
        .expect_err("batch should report the query panic");
    let QueryError::Internal(ice) = failure else {
        panic!("expected internal query error");
    };
    assert_eq!(ice.origin, nia_ice::IceOrigin::UnexpectedPanic);

    let retry = db
        .get_many([PanicsOnce, PanicsOnce])
        .expect_err("tainted session must reject retries");
    assert!(matches!(retry, QueryError::Internal(_)));
    assert_eq!(db.context().executions.load(Ordering::SeqCst), 1);
}
