use super::*;

#[test]
fn bounded_priority_task_pool_preserves_submission_order_and_lanes() {
    let session = QuerySession::with_parallelism(4);
    let active = Arc::new(AtomicUsize::new(0));
    let peak_active = Arc::new(AtomicUsize::new(0));
    let barrier = Arc::new(Barrier::new(2));
    let mut pool = session.task_pool(2);

    for value in 0..4 {
        let active = Arc::clone(&active);
        let peak_active = Arc::clone(&peak_active);
        let barrier = Arc::clone(&barrier);
        pool.submit(move || {
            let current = active.fetch_add(1, Ordering::SeqCst) + 1;
            peak_active.fetch_max(current, Ordering::SeqCst);
            barrier.wait();
            active.fetch_sub(1, Ordering::SeqCst);
            OwnedNonCloneValue { value }
        });
    }

    let values = pool.finish();

    assert_eq!(
        values
            .into_iter()
            .map(|value| value.value)
            .collect::<Vec<_>>(),
        vec![0, 1, 2, 3]
    );
    assert_eq!(active.load(Ordering::SeqCst), 0);
    assert_eq!(peak_active.load(Ordering::SeqCst), 2);
}

#[test]
fn priority_task_pool_runs_before_queued_batch_work() {
    let session = QuerySession::with_parallelism(2);
    let executor = &session.inner.executor;
    let normal_batch = Arc::new(QueryBatch::new(2));
    let normal_batch_id = Arc::as_ptr(&normal_batch) as usize;
    let order = Arc::new(Mutex::new(Vec::new()));
    let (started_sender, started_receiver) = std::sync::mpsc::channel();
    let (release_sender, release_receiver) = std::sync::mpsc::channel();
    let (normal_sender, normal_receiver) = std::sync::mpsc::channel();
    let blocker_batch = Arc::clone(&normal_batch);
    let blocker_shared = Arc::clone(&executor.shared);
    let normal_order = Arc::clone(&order);
    let normal_task_batch = Arc::clone(&normal_batch);
    let normal_shared = Arc::clone(&executor.shared);

    executor.submit_all(vec![
        QueryTask {
            batch: normal_batch_id,
            run: Box::new(move || {
                started_sender.send(()).expect("signal blocker start");
                release_receiver.recv().expect("release blocker");
                blocker_batch.complete(0, Ok(()));
                blocker_shared.notify_waiters();
            }),
        },
        QueryTask {
            batch: normal_batch_id,
            run: Box::new(move || {
                normal_order
                    .lock()
                    .expect("task order lock poisoned")
                    .push("normal");
                normal_task_batch.complete(1, Ok(()));
                normal_shared.notify_waiters();
                normal_sender.send(()).expect("signal normal completion");
            }),
        },
    ]);
    started_receiver.recv().expect("wait for blocker start");

    let priority_order = Arc::clone(&order);
    let mut pool = session.task_pool(1);
    pool.submit(move || {
        priority_order
            .lock()
            .expect("task order lock poisoned")
            .push("priority");
    });
    release_sender.send(()).expect("release executor worker");
    normal_receiver
        .recv_timeout(std::time::Duration::from_secs(2))
        .expect("queued normal task completion");

    assert_eq!(pool.finish(), vec![()]);
    assert_eq!(normal_batch.finish(), vec![(), ()]);
    assert_eq!(
        *order.lock().expect("task order lock poisoned"),
        vec!["priority", "normal"]
    );
}

#[test]
fn priority_task_pool_drains_after_task_panic() {
    let session = QuerySession::with_parallelism(2);
    let completed = Arc::new(AtomicUsize::new(0));
    let mut pool = session.task_pool(2);
    pool.submit(|| -> usize { panic!("priority task failure") });
    let task_completed = Arc::clone(&completed);
    pool.submit(move || {
        task_completed.fetch_add(1, Ordering::SeqCst);
        7
    });

    let result = catch_unwind(AssertUnwindSafe(|| pool.finish()));

    assert!(result.is_err());
    assert_eq!(completed.load(Ordering::SeqCst), 1);
    assert_eq!(session.run_tasks([|| 9]), vec![9]);
}
