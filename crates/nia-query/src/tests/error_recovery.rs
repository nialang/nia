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
