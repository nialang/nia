use super::*;

#[test]
fn semantic_value_validation_reuses_fingerprint_only_for_equal_outputs() {
    let db = QueryDb::new(RedGreenContext {
        input: AtomicUsize::new(7),
        derived_executions: AtomicUsize::new(0),
        parent_executions: AtomicUsize::new(0),
    });
    let first = db.expect_get(SemanticParityParent);
    db.context().input.store(9, Ordering::SeqCst);
    db.validate_input(RedGreenInput, &9);
    let equal = db.expect_get(SemanticParityParent);
    assert!(Arc::ptr_eq(&first, &equal));
    assert_eq!(db.context().derived_executions.load(Ordering::SeqCst), 2);
    assert_eq!(db.context().parent_executions.load(Ordering::SeqCst), 1);

    db.context().input.store(10, Ordering::SeqCst);
    db.validate_input(RedGreenInput, &10);
    let changed = db.expect_get(SemanticParityParent);
    assert!(!Arc::ptr_eq(&equal, &changed));
    assert_eq!(*changed, 10);
    assert_eq!(db.context().derived_executions.load(Ordering::SeqCst), 3);
    assert_eq!(db.context().parent_executions.load(Ordering::SeqCst), 2);
}

#[test]
fn direct_invalidation_preserves_stable_dependents_for_validation() {
    let db = QueryDb::new(RedGreenContext {
        input: AtomicUsize::new(7),
        derived_executions: AtomicUsize::new(0),
        parent_executions: AtomicUsize::new(0),
    });
    let first = db.expect_get(StableParityParent);
    db.context().input.store(9, Ordering::SeqCst);
    let invalidation = db.invalidate(RedGreenInput);
    assert_eq!(
        invalidation
            .invalidated
            .iter()
            .map(|frame| frame.name)
            .collect::<Vec<_>>(),
        ["red_green_input", "stable_parity", "stable_parity_parent"]
    );
    let latest = db.expect_get(StableParityParent);

    assert!(Arc::ptr_eq(&first, &latest));
    assert_eq!(db.context().derived_executions.load(Ordering::SeqCst), 2);
    assert_eq!(db.context().parent_executions.load(Ordering::SeqCst), 1);
    let trace = db.query_trace();
    let parent = trace
        .queries
        .iter()
        .find(|query| query.frame.name == "stable_parity_parent")
        .expect("stable parent trace");
    assert_eq!(parent.stats.validations, 1);
    assert_eq!(parent.stats.green_validations, 1);
}

#[test]
fn derived_red_green_validation_reexecutes_dependents_when_output_changes() {
    let db = QueryDb::new(RedGreenContext {
        input: AtomicUsize::new(7),
        derived_executions: AtomicUsize::new(0),
        parent_executions: AtomicUsize::new(0),
    });
    assert_eq!(*db.expect_get(StableParityParent), 11);
    db.context().input.store(8, Ordering::SeqCst);
    db.validate_input(RedGreenInput, &8);
    assert_eq!(*db.expect_get(StableParityParent), 10);
    assert_eq!(db.context().derived_executions.load(Ordering::SeqCst), 2);
    assert_eq!(db.context().parent_executions.load(Ordering::SeqCst), 2);
    let trace = db.query_trace();
    let parent = trace
        .queries
        .iter()
        .find(|query| query.frame.name == "stable_parity_parent")
        .expect("stable parent trace");
    assert_eq!(parent.stats.validations, 1);
    assert_eq!(parent.stats.green_validations, 0);
}
