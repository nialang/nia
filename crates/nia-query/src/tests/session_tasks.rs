use super::*;

#[test]
fn session_tasks_move_non_clone_outputs_in_submission_order() {
    let session = QuerySession::with_parallelism(2);

    let values = session.run_tasks((0..4).map(|value| move || OwnedNonCloneValue { value }));

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

    assert_eq!(session.run_tasks(tasks), vec![0, 1, 2, 3]);
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

    let values = session.run_tasks_bounded(tasks, 2);

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
                run: Box::new(move || {
                    completed.fetch_add(1, Ordering::SeqCst);
                }),
            }
        })
        .collect();

    session.inner.executor.submit_all(tasks);
    drop(session);

    assert_eq!(completed.load(Ordering::SeqCst), task_count);
}
