use super::*;

#[test]
fn consecutive_input_revisions_validate_against_latest_value() {
    let db = QueryDb::new(RedGreenContext {
        input: AtomicUsize::new(7),
        derived_executions: AtomicUsize::new(0),
        parent_executions: AtomicUsize::new(0),
    });
    let first = db.expect_get(StableParityParent);
    for value in [9, 11] {
        db.context().input.store(value, Ordering::SeqCst);
        db.validate_input(RedGreenInput, &value);
    }
    let latest = db.expect_get(StableParityParent);
    assert!(Arc::ptr_eq(&first, &latest));
    assert_eq!(db.context().derived_executions.load(Ordering::SeqCst), 2);
    assert_eq!(db.context().parent_executions.load(Ordering::SeqCst), 1);
}

#[test]
fn stable_get_many_records_dependency_fingerprints_for_green_validation() {
    let db = QueryDb::new(RedGreenContext {
        input: AtomicUsize::new(7),
        derived_executions: AtomicUsize::new(0),
        parent_executions: AtomicUsize::new(0),
    });
    let first = db.expect_get(StableModuloBatchParent);
    assert_eq!(*first, 2);
    db.context().input.store(13, Ordering::SeqCst);
    db.validate_input(RedGreenInput, &13);
    let latest = db.expect_get(StableModuloBatchParent);
    assert!(Arc::ptr_eq(&first, &latest));
    assert_eq!(db.context().derived_executions.load(Ordering::SeqCst), 4);
    assert_eq!(db.context().parent_executions.load(Ordering::SeqCst), 1);
    assert_eq!(
        db.query_trace()
            .dependencies
            .iter()
            .filter(|edge| edge.from.name == "stable_modulo_batch_parent"
                && edge.to.name == "stable_modulo")
            .count(),
        2
    );
}

#[test]
fn invalidation_during_validation_cannot_restore_stale_green_value() {
    let control = Arc::new((Mutex::new(ValidationRaceState::default()), Condvar::new()));
    let db = QueryDb::new(ValidationRaceContext {
        input: AtomicUsize::new(7),
        input_executions: AtomicUsize::new(0),
        derived_executions: AtomicUsize::new(0),
        control: Arc::clone(&control),
    });
    let first = db.expect_get(ValidationRaceDerived);
    db.context().input.store(9, Ordering::SeqCst);
    db.validate_input(ValidationRaceInput, &9);
    let worker_db = db.clone();
    let latest = std::thread::scope(|scope| {
        let handle = scope.spawn(move || worker_db.expect_get(ValidationRaceDerived));
        let (lock, ready) = &*control;
        let mut state = lock.lock().expect("validation race lock poisoned");
        while !state.started {
            state = ready.wait(state).expect("validation race lock poisoned");
        }
        drop(state);
        db.context().input.store(11, Ordering::SeqCst);
        db.validate_input(ValidationRaceInput, &11);
        let mut state = lock.lock().expect("validation race lock poisoned");
        state.release = true;
        ready.notify_all();
        drop(state);
        handle.join().expect("validation worker panicked")
    });
    assert_eq!(*latest, 1);
    assert!(!Arc::ptr_eq(&first, &latest));
    assert_eq!(db.context().input_executions.load(Ordering::SeqCst), 3);
    assert_eq!(db.context().derived_executions.load(Ordering::SeqCst), 2);
    let trace = db.query_trace();
    let derived = trace
        .queries
        .iter()
        .find(|query| query.frame.name == "validation_race_derived")
        .expect("validation race derived trace");
    assert_eq!(derived.stats.validations, 1);
    assert_eq!(derived.stats.green_validations, 0);
}
