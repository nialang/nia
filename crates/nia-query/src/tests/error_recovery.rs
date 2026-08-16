use super::*;

#[test]
fn reports_same_thread_query_cycles() {
    let db = QueryDb::new(TestContext {
        executions: AtomicUsize::new(0),
    });

    let error = db.get(Recursive).expect_err("cycle should be reported");
    let cycle = match error {
        QueryError::Cycle { cycle } => cycle,
        QueryError::InvalidInput { .. } => panic!("expected query cycle"),
    };
    assert_eq!(cycle.len(), 2);
    assert!(cycle.iter().all(|frame| frame.name == "recursive"));
}

#[test]
fn get_returns_query_error() {
    let db = QueryDb::new(TestContext {
        executions: AtomicUsize::new(0),
    });

    let error = db.get(Recursive).expect_err("cycle must fail");
    assert!(matches!(error, QueryError::Cycle { .. }));
}

#[test]
fn query_can_report_invalid_input_as_query_error() {
    let db = QueryDb::new(TestContext {
        executions: AtomicUsize::new(0),
    });

    let err = db
        .get(InvalidInputQuery)
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
fn get_many_reports_query_failures_as_values() {
    let db = QueryDb::new(TestContext {
        executions: AtomicUsize::new(0),
    });

    let err = db
        .get_many([InvalidInputQuery])
        .expect_err("invalid batch input should be a query error");
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
        .get(InvalidAfterDependency)
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
        let error = worker_db
            .get(ParallelRecursive)
            .expect_err("parallel recursive query should fail");
        sender
            .send(matches!(error, QueryError::Cycle { .. }))
            .expect("send query result");
    });

    assert_eq!(
        receiver.recv_timeout(std::time::Duration::from_secs(2)),
        Ok(true)
    );
}

#[test]
fn distinct_workers_detect_cross_stack_query_cycles() {
    let session = QuerySession::with_parallelism(2);
    let db = QueryDb::new_with_timings_in_session(
        TestContext {
            executions: AtomicUsize::new(0),
        },
        nia_timing::TimingMode::Off,
        session,
    );
    let worker_db = db.clone();
    let (sender, receiver) = std::sync::mpsc::channel();

    let worker = std::thread::spawn(move || {
        let result = worker_db.get_many([ParallelCycle::Left, ParallelCycle::Right]);
        sender
            .send(matches!(result, Err(QueryError::Cycle { .. })))
            .expect("send cross-stack cycle result");
    });

    assert_eq!(
        receiver.recv_timeout(std::time::Duration::from_secs(2)),
        Ok(true),
        "parallel query cycle must fail instead of leaving both workers blocked"
    );
    worker.join().expect("parallel cycle worker");
}

#[test]
fn distinct_sessions_detect_cross_stack_query_cycles() {
    let barrier = Arc::new(Barrier::new(2));
    let left_link = Arc::new(Mutex::new(None));
    let right_link = Arc::new(Mutex::new(None));
    let left_db = QueryDb::new_with_timings_in_session(
        CrossSessionCycleContext {
            other: Arc::clone(&right_link),
            barrier: Arc::clone(&barrier),
            executions: AtomicUsize::new(0),
        },
        nia_timing::TimingMode::Off,
        QuerySession::with_parallelism(1),
    );
    let right_db = QueryDb::new_with_timings_in_session(
        CrossSessionCycleContext {
            other: Arc::clone(&left_link),
            barrier,
            executions: AtomicUsize::new(0),
        },
        nia_timing::TimingMode::Off,
        QuerySession::with_parallelism(1),
    );
    *left_link.lock().expect("left cycle link lock poisoned") = Some(left_db.clone());
    *right_link.lock().expect("right cycle link lock poisoned") = Some(right_db.clone());
    let (sender, receiver) = std::sync::mpsc::channel();
    let left_sender = sender.clone();
    let left_thread = std::thread::spawn(move || {
        let result = left_db.get(CrossSessionCycle::Left);
        left_sender
            .send(matches!(result, Err(QueryError::Cycle { .. })))
            .expect("send left cross-session cycle result");
    });
    let right_thread = std::thread::spawn(move || {
        let result = right_db.get(CrossSessionCycle::Right);
        sender
            .send(matches!(result, Err(QueryError::Cycle { .. })))
            .expect("send right cross-session cycle result");
    });

    assert_eq!(
        receiver.recv_timeout(std::time::Duration::from_secs(2)),
        Ok(true)
    );
    assert_eq!(
        receiver.recv_timeout(std::time::Duration::from_secs(2)),
        Ok(true)
    );
    left_thread.join().expect("left cycle worker");
    right_thread.join().expect("right cycle worker");
    *left_link.lock().expect("left cycle link lock poisoned") = None;
    *right_link.lock().expect("right cycle link lock poisoned") = None;
}

#[test]
fn panicking_query_resets_slot_for_later_attempts() {
    let db = QueryDb::new(TestContext {
        executions: AtomicUsize::new(0),
    });

    let first = std::panic::catch_unwind(|| db.expect_get(PanicsOnce))
        .expect_err("first query should panic");
    assert!(first.is::<&'static str>());

    assert_eq!(*db.expect_get(PanicsOnce), 99);
    assert_eq!(db.context().executions.load(Ordering::SeqCst), 2);
}
