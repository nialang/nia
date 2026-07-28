use super::*;

#[test]
fn executes_get_many_in_key_order() {
    let db = QueryDb::new(TestContext {
        executions: AtomicUsize::new(0),
    });

    let values = db
        .get_many([Double(1), Double(4), Double(3)])
        .expect("batch should succeed");

    assert_eq!(
        values.iter().map(|value| **value).collect::<Vec<_>>(),
        vec![2, 8, 6]
    );
    assert_eq!(db.context().executions.load(Ordering::SeqCst), 3);
}

#[test]
fn get_many_reuses_non_clone_cached_handles_in_key_order() {
    let db = QueryDb::new(TestContext {
        executions: AtomicUsize::new(0),
    });

    let values = db
        .get_many([NonCloneValueQuery, NonCloneValueQuery])
        .expect("batch should succeed");

    assert_eq!(values.len(), 2);
    assert!(Arc::ptr_eq(&values[0], &values[1]));
    assert_eq!(values[0].value, 42);
    assert_eq!(db.context().executions.load(Ordering::SeqCst), 1);
}

#[test]
fn get_many_owned_moves_non_clone_values_in_key_order() {
    let db = QueryDb::new(TestContext {
        executions: AtomicUsize::new(0),
    });

    let values = db
        .get_many_owned([
            OwnedNonCloneValueQuery(4),
            OwnedNonCloneValueQuery(1),
            OwnedNonCloneValueQuery(3),
        ])
        .expect("owned batch should succeed");

    assert_eq!(
        values
            .into_iter()
            .map(|value| value.value)
            .collect::<Vec<_>>(),
        vec![4, 1, 3]
    );
    assert_eq!(db.context().executions.load(Ordering::SeqCst), 3);
}

#[test]
fn typed_owned_completion_stream_moves_values_in_completion_order() {
    let session = QuerySession::with_parallelism(2);
    let db = QueryDb::new_with_timings_in_session(
        CompletionOrderContext {
            phase: AtomicUsize::new(0),
        },
        nia_timing::TimingMode::Off,
        session,
    );

    let completed = db.with_many_owned_completion(
        [CompletionOrderProbe(1), CompletionOrderProbe(0)],
        |stream| {
            let mut completed = Vec::new();
            while let Some((position, value)) = stream.wait_next() {
                let value = value.expect("completion query should succeed");
                completed.push((position, value));
                db.context().phase.store(value + 1, Ordering::SeqCst);
            }
            completed
        },
    );

    assert_eq!(completed, vec![(1, 0), (0, 1)]);
}

#[test]
fn typed_owned_completion_stream_reports_query_failures() {
    let db = QueryDb::new(TestContext {
        executions: AtomicUsize::new(0),
    });

    let mut completed = db.with_many_owned_completion(
        [
            FallibleOwnedCompletionProbe(0),
            FallibleOwnedCompletionProbe(1),
        ],
        |stream| {
            let mut completed = Vec::new();
            while let Some((position, value)) = stream.wait_next() {
                completed.push((position, value));
            }
            completed
        },
    );
    completed.sort_by_key(|(position, _)| *position);

    assert_eq!(completed[0], (0, Ok(0)));
    assert!(matches!(
        &completed[1],
        (
            1,
            Err(QueryError::InvalidInput {
                message,
                ..
            })
        ) if message == "rejected completion"
    ));
}

#[test]
fn get_many_owned_records_dependencies_from_parent_query() {
    let db = QueryDb::new(TestContext {
        executions: AtomicUsize::new(0),
    });

    assert_eq!(*db.expect_get(OwnedValueBatchParent), 10);
    let invalidation = db.invalidate(OwnedNonCloneValueQuery(5));

    assert!(
        invalidation
            .invalidated
            .iter()
            .any(|frame| frame.name == "owned_value_batch_parent")
    );
}

#[test]
fn get_many_owned_uses_the_session_executor_budget() {
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
        .get_many_owned([
            OwnedExecutorProbe(0),
            OwnedExecutorProbe(1),
            OwnedExecutorProbe(2),
            OwnedExecutorProbe(3),
        ])
        .expect("owned batch should succeed");

    assert_eq!(values, vec![0, 1, 2, 3]);
    assert_eq!(db.context().active.load(Ordering::SeqCst), 0);
    assert_eq!(db.context().peak_active.load(Ordering::SeqCst), 2);
    assert_eq!(session.inner.executor.peak_active(), 2);
}
