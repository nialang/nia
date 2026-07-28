use super::*;

#[test]
fn session_executor_caps_concurrent_batch_tasks() {
    let session = QuerySession::with_parallelism(2);
    let db = QueryDb::new_with_timings_in_session(
        ExecutorProbeContext {
            active: AtomicUsize::new(0),
            peak_active: AtomicUsize::new(0),
            barrier: Arc::new(Barrier::new(2)),
        },
        nia_timing::TimingMode::Off,
        session.clone(),
    );

    let values = db
        .get_many([
            ExecutorProbe(0),
            ExecutorProbe(1),
            ExecutorProbe(2),
            ExecutorProbe(3),
        ])
        .expect("batch should succeed");

    assert_eq!(
        values.iter().map(|value| **value).collect::<Vec<_>>(),
        vec![0, 1, 2, 3]
    );
    assert_eq!(db.context().active.load(Ordering::SeqCst), 0);
    assert_eq!(db.context().peak_active.load(Ordering::SeqCst), 2);
    assert_eq!(session.inner.executor.peak_active(), 2);
}

#[test]
fn shared_execution_budget_caps_tasks_across_sessions() {
    let execution_budget = Arc::new(QueryExecutionBudget::owned(2));
    let first_session = QuerySession::with_execution_budget(2, Arc::clone(&execution_budget));
    let second_session = QuerySession::with_execution_budget(2, Arc::clone(&execution_budget));
    let barrier = Arc::new(Barrier::new(2));
    let first_db = QueryDb::new_with_timings_in_session(
        ExecutorProbeContext {
            active: AtomicUsize::new(0),
            peak_active: AtomicUsize::new(0),
            barrier: Arc::clone(&barrier),
        },
        nia_timing::TimingMode::Off,
        first_session,
    );
    let second_db = QueryDb::new_with_timings_in_session(
        ExecutorProbeContext {
            active: AtomicUsize::new(0),
            peak_active: AtomicUsize::new(0),
            barrier,
        },
        nia_timing::TimingMode::Off,
        second_session,
    );
    let (sender, receiver) = std::sync::mpsc::channel();
    let second_sender = sender.clone();
    std::thread::spawn(move || {
        let values = first_db
            .get_many([ExecutorProbe(0), ExecutorProbe(1)])
            .expect("first shared-budget batch should succeed");
        sender
            .send(values.into_iter().map(|value| *value).collect::<Vec<_>>())
            .expect("send first shared-budget batch");
    });
    std::thread::spawn(move || {
        let values = second_db
            .get_many([ExecutorProbe(2), ExecutorProbe(3)])
            .expect("second shared-budget batch should succeed");
        second_sender
            .send(values.into_iter().map(|value| *value).collect::<Vec<_>>())
            .expect("send second shared-budget batch");
    });

    let mut batches = vec![
        receiver
            .recv_timeout(std::time::Duration::from_secs(10))
            .expect("first shared-budget batch must complete"),
        receiver
            .recv_timeout(std::time::Duration::from_secs(10))
            .expect("second shared-budget batch must complete"),
    ];
    batches.sort();
    assert_eq!(batches, vec![vec![0, 1], vec![2, 3]]);
    assert_eq!(execution_budget.peak_active(), 2);
}

#[test]
fn default_sessions_share_the_process_execution_budget() {
    let first = QuerySession::new();
    let second = QuerySession::new();

    assert!(!first.ptr_eq(&second));
    assert!(Arc::ptr_eq(
        &first.inner.executor.execution_budget,
        &second.inner.executor.execution_budget
    ));
}

#[test]
fn default_query_parallelism_is_bounded() {
    let count = default_query_parallelism();

    assert!(count >= 1);
    assert!(count <= DEFAULT_MAX_QUERY_EXECUTOR_PARALLELISM);
}
