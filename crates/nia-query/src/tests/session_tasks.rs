use super::*;

#[test]
fn session_tasks_move_non_clone_outputs_in_submission_order() {
    let session = QuerySession::with_parallelism(2);

    let values = session
        .run_tasks((0..4).map(|value| move || OwnedNonCloneValue { value }))
        .expect("session tasks");

    assert_eq!(
        values
            .into_iter()
            .map(|value| value.value)
            .collect::<Vec<_>>(),
        vec![0, 1, 2, 3]
    );
}

#[test]
fn session_tasks_use_the_shared_executor_budget() {
    let session = QuerySession::with_parallelism(2);
    let active = Arc::new(AtomicUsize::new(0));
    let peak_active = Arc::new(AtomicUsize::new(0));
    let barrier = Arc::new(Barrier::new(2));
    let tasks = (0..4).map(|value| {
        let active = Arc::clone(&active);
        let peak_active = Arc::clone(&peak_active);
        let barrier = Arc::clone(&barrier);
        move || {
            let current = active.fetch_add(1, Ordering::SeqCst) + 1;
            peak_active.fetch_max(current, Ordering::SeqCst);
            barrier.wait();
            active.fetch_sub(1, Ordering::SeqCst);
            value
        }
    });

    assert_eq!(
        session.run_tasks(tasks).expect("session tasks"),
        vec![0, 1, 2, 3]
    );
    assert_eq!(active.load(Ordering::SeqCst), 0);
    assert_eq!(peak_active.load(Ordering::SeqCst), 2);
    assert_eq!(session.inner.executor.peak_active(), 2);
}

#[test]
fn bounded_session_tasks_preserve_order_and_limit_worker_lanes() {
    let session = QuerySession::with_parallelism(4);
    let active = Arc::new(AtomicUsize::new(0));
    let peak_active = Arc::new(AtomicUsize::new(0));
    let barrier = Arc::new(Barrier::new(2));
    let tasks = (0..6).map(|value| {
        let active = Arc::clone(&active);
        let peak_active = Arc::clone(&peak_active);
        let barrier = Arc::clone(&barrier);
        move || {
            let current = active.fetch_add(1, Ordering::SeqCst) + 1;
            peak_active.fetch_max(current, Ordering::SeqCst);
            barrier.wait();
            active.fetch_sub(1, Ordering::SeqCst);
            OwnedNonCloneValue { value }
        }
    });

    let values = session
        .run_tasks_bounded(tasks, 2)
        .expect("bounded session tasks");

    assert_eq!(
        values
            .into_iter()
            .map(|value| value.value)
            .collect::<Vec<_>>(),
        vec![0, 1, 2, 3, 4, 5]
    );
    assert_eq!(active.load(Ordering::SeqCst), 0);
    assert_eq!(peak_active.load(Ordering::SeqCst), 2);
    assert_eq!(session.inner.executor.peak_active(), 2);
}

#[test]
fn dropping_session_drains_all_accepted_executor_tasks() {
    let session = QuerySession::with_parallelism(2);
    let completed = Arc::new(AtomicUsize::new(0));
    let task_count = 8;
    let tasks = (0..task_count)
        .map(|batch| {
            let completed = Arc::clone(&completed);
            QueryTask {
                batch,
                settle: Box::new(move |_| {
                    completed.fetch_add(1, Ordering::SeqCst);
                }),
            }
        })
        .collect();

    session
        .inner
        .executor
        .submit_all(tasks)
        .expect("submit session tasks");
    drop(session);

    assert_eq!(completed.load(Ordering::SeqCst), task_count);
}

#[test]
fn session_construction_rejects_zero_parallelism() {
    let result = QuerySession::from_execution_budget(0, Arc::new(QueryExecutionBudget::owned(1)));
    let error = match result {
        Ok(_) => panic!("zero parallelism must be rejected"),
        Err(error) => error,
    };

    assert!(error.to_string().contains("parallelism must be non-zero"));
}

#[test]
fn batch_rejects_duplicate_completion_after_streaming_consumption() {
    let batch = QueryBatch::new(1);
    batch.complete(0, Ok(7)).expect("complete batch item");
    let (completed, is_complete) = batch.take_completed().expect("take completed item");

    assert_eq!(completed.len(), 1);
    assert_eq!(completed[0].0, 0);
    assert_eq!(completed[0].1.as_ref().expect("completed value"), &7);
    assert!(is_complete);
    let error = batch
        .complete(0, Ok(8))
        .expect_err("duplicate completion must fail");
    assert!(error.to_string().contains("completed twice"));
    assert!(batch.is_complete());
}

#[test]
fn executor_rejects_submission_after_shutdown_without_running_tasks() {
    let session = QuerySession::with_parallelism(1);
    session.inner.executor.shared.state.lock().shutdown = true;
    let ran = Arc::new(AtomicUsize::new(0));
    let task_ran = Arc::clone(&ran);
    let error = session
        .inner
        .executor
        .submit_all(vec![QueryTask {
            batch: 0,
            settle: Box::new(move |_| {
                task_ran.fetch_add(1, Ordering::SeqCst);
            }),
        }])
        .expect_err("shutdown executor must reject work");

    assert!(error.to_string().contains("after shutdown"));
    assert_eq!(ran.load(Ordering::SeqCst), 0);
}
