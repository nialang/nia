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
    )
    .expect("create input database");
    let parent_db = QueryDb::new_with_timings_in_session(
        CrossSessionBatchContext { input_db },
        nia_timing::TimingMode::Off,
        parent_session,
    )
    .expect("create parent database");
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
    )
    .expect("create query database");
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
    )
    .expect("create query database");
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
fn get_many_internal_error_taints_session() {
    let session = QuerySession::with_parallelism(2);
    let db = QueryDb::new_with_timings_in_session(
        TestContext {
            executions: AtomicUsize::new(0),
        },
        nia_timing::TimingMode::Off,
        session,
    )
    .expect("create query database");

    let failure = db
        .get_many([FailsOnce, FailsOnce])
        .expect_err("batch should report the query ICE");
    let QueryError::Internal(ice) = failure else {
        panic!("expected internal query error");
    };
    assert!(ice.message.contains("transient query failure"));
    let executions_after_failure = db.context().executions.load(Ordering::SeqCst);
    assert!((1..=2).contains(&executions_after_failure));

    let retry = db
        .get_many([FailsOnce, FailsOnce])
        .expect_err("tainted session must reject retries");
    assert!(matches!(retry, QueryError::Internal(_)));
    assert_eq!(
        db.context().executions.load(Ordering::SeqCst),
        executions_after_failure
    );
}

/// One task of `batch` that runs `work`, completes its slot, and wakes the
/// batch owners, as session batches do.
fn probe_task(
    executor: &QueryExecutor,
    batch: &Arc<QueryBatch<usize>>,
    work: impl FnOnce() -> usize + Send + 'static,
) -> QueryTask {
    let task_batch = Arc::clone(batch);
    let shared = Arc::clone(&executor.shared);
    QueryTask {
        batch: Arc::as_ptr(batch) as usize,
        settle: Box::new(move |_| {
            task_batch
                .complete(0, Ok(work()))
                .expect("complete probe task");
            shared.notify_waiters();
        }),
    }
}

/// A batch owner checks whether it can run its own task and, when it cannot,
/// waits. Capacity freed between the two must end the wait: with parallelism
/// 1 there are no worker threads, so the owner is the only thread that could
/// ever run its task.
#[test]
fn batch_owner_notices_capacity_freed_between_its_check_and_its_wait() {
    let session = QuerySession::with_parallelism(1);
    let executor = &session.inner.executor;

    // Another owner holds the only unit of capacity until released.
    let holder = Arc::new(QueryBatch::new(1));
    let (release, released) = std::sync::mpsc::channel::<()>();
    executor
        .submit_all(vec![probe_task(executor, &holder, move || {
            released.recv().expect("release holder");
            0
        })])
        .expect("submit holder task");
    let holder_session = session.clone();
    let holder_batch = Arc::clone(&holder);
    let holder_thread = std::thread::spawn(move || {
        let executor = &holder_session.inner.executor;
        assert!(
            executor
                .try_run_one(Arc::as_ptr(&holder_batch) as usize)
                .expect("run holder task")
        );
    });
    while executor.shared.state.lock().active == 0 {
        std::thread::yield_now();
    }

    let waiting = Arc::new(QueryBatch::new(1));
    let waiting_id = Arc::as_ptr(&waiting) as usize;
    executor
        .submit_all(vec![probe_task(executor, &waiting, || 4)])
        .expect("submit waiting task");
    assert!(!executor.try_run_one(waiting_id).expect("check capacity"));

    // The capacity is freed, and its notification sent, before the owner
    // starts waiting.
    release.send(()).expect("release holder");
    holder_thread.join().expect("holder thread");
    assert!(holder.is_complete());
    assert_eq!(executor.shared.state.lock().active, 0);

    let (woke, woken) = std::sync::mpsc::channel();
    let waiter_session = session.clone();
    let waiter_batch = Arc::clone(&waiting);
    std::thread::spawn(move || {
        waiter_session
            .inner
            .executor
            .wait_for_batch_progress(waiting_id, &waiter_batch);
        woke.send(()).expect("report wake-up");
    });
    assert_eq!(
        woken.recv_timeout(std::time::Duration::from_secs(10)),
        Ok(())
    );
    assert!(executor.try_run_one(waiting_id).expect("run waiting task"));
    assert_eq!(waiting.finish().expect("waiting batch result"), vec![4]);
}
