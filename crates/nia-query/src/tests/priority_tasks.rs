use super::*;

#[test]
fn bounded_priority_task_pool_preserves_submission_order_and_lanes() {
    let session = QuerySession::with_parallelism(4);
    let active = Arc::new(AtomicUsize::new(0));
    let peak_active = Arc::new(AtomicUsize::new(0));
    let barrier = Arc::new(Barrier::new(2));
    let mut pool = session.task_pool(2).expect("create task pool");

    for value in 0..4 {
        let active = Arc::clone(&active);
        let peak_active = Arc::clone(&peak_active);
        let barrier = Arc::clone(&barrier);
        pool.submit(move || {
            let current = active.fetch_add(1, Ordering::SeqCst) + 1;
            peak_active.fetch_max(current, Ordering::SeqCst);
            barrier.wait();
            active.fetch_sub(1, Ordering::SeqCst);
            Ok(OwnedNonCloneValue { value })
        })
        .expect("submit priority task");
    }

    let values = pool.finish().expect("finish priority tasks");

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

    executor
        .submit_all(vec![
            QueryTask {
                batch: normal_batch_id,
                settle: Box::new(move |_| {
                    started_sender.send(()).expect("signal blocker start");
                    release_receiver.recv().expect("release blocker");
                    blocker_batch.complete(0, Ok(())).expect("complete blocker");
                    blocker_shared.notify_waiters();
                }),
            },
            QueryTask {
                batch: normal_batch_id,
                settle: Box::new(move |_| {
                    normal_order.lock().push("normal");
                    normal_task_batch
                        .complete(1, Ok(()))
                        .expect("complete normal task");
                    normal_shared.notify_waiters();
                    normal_sender.send(()).expect("signal normal completion");
                }),
            },
        ])
        .expect("submit normal tasks");
    started_receiver.recv().expect("wait for blocker start");

    let priority_order = Arc::clone(&order);
    let mut pool = session.task_pool(1).expect("create task pool");
    pool.submit(move || {
        priority_order.lock().push("priority");
        Ok(())
    })
    .expect("submit priority task");
    release_sender.send(()).expect("release executor worker");
    normal_receiver
        .recv_timeout(std::time::Duration::from_secs(2))
        .expect("queued normal task completion");

    assert_eq!(pool.finish().expect("finish priority task"), vec![()]);
    assert_eq!(
        normal_batch.finish().expect("finish normal batch"),
        vec![(), ()]
    );
    assert_eq!(*order.lock(), vec!["priority", "normal"]);
}

#[test]
fn priority_task_pool_drains_after_internal_failure() {
    let session = QuerySession::with_parallelism(2);
    let completed = Arc::new(AtomicUsize::new(0));
    let mut pool = session.task_pool(2).expect("create task pool");
    pool.submit(|| Err(nia_ice::Ice::new("priority task failure")))
        .expect("submit failing task");
    let task_completed = Arc::clone(&completed);
    pool.submit(move || {
        task_completed.fetch_add(1, Ordering::SeqCst);
        Ok(7)
    })
    .expect("submit completing task");

    let failure = pool.finish().expect_err("task ICE must be propagated");

    assert_eq!(failure.message, "priority task failure");
    assert!(
        failure
            .location
            .as_deref()
            .is_some_and(|location| location.contains("priority_tasks.rs")),
        "{failure:?}"
    );
    let completed_after_failure = completed.load(Ordering::SeqCst);
    assert!(completed_after_failure <= 1);
    assert!(session.run_tasks([|| Ok(9)]).is_err());
    assert_eq!(completed.load(Ordering::SeqCst), completed_after_failure);
}
