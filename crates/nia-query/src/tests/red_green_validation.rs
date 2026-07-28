use super::*;

#[test]
fn stable_input_validation_keeps_identical_values_green() {
    let db = QueryDb::new(TestContext {
        executions: AtomicUsize::new(7),
    });
    let first = db.expect_get(StableInputParent);
    assert_eq!(*first, 14);

    let invalidation = db.validate_input(StableInput, &7);

    assert!(invalidation.invalidated.is_empty());
    let second = db.expect_get(StableInputParent);
    assert!(Arc::ptr_eq(&first, &second));
}

#[test]
fn stable_input_validation_invalidates_changed_values_and_dependents() {
    let db = QueryDb::new(TestContext {
        executions: AtomicUsize::new(7),
    });
    assert_eq!(*db.expect_get(StableInputParent), 14);
    db.context().executions.store(9, Ordering::SeqCst);

    let invalidation = db.validate_input(StableInput, &9);
    let invalidated = invalidation
        .invalidated
        .iter()
        .map(|frame| frame.name)
        .collect::<Vec<_>>();

    assert_eq!(invalidated, ["stable_input", "stable_input_parent"]);
    assert_eq!(*db.expect_get(StableInputParent), 18);
}

#[test]
fn derived_red_green_validation_reuses_dependents_when_output_is_unchanged() {
    let db = QueryDb::new(RedGreenContext {
        input: AtomicUsize::new(7),
        derived_executions: AtomicUsize::new(0),
        parent_executions: AtomicUsize::new(0),
    });
    let first = db.expect_get(StableParityParent);
    assert_eq!(*first, 11);
    db.context().input.store(9, Ordering::SeqCst);

    let invalidation = db.validate_input(RedGreenInput, &9);
    assert_eq!(
        invalidation
            .invalidated
            .iter()
            .map(|frame| frame.name)
            .collect::<Vec<_>>(),
        ["red_green_input", "stable_parity", "stable_parity_parent"]
    );
    let second = db.expect_get(StableParityParent);

    assert!(Arc::ptr_eq(&first, &second));
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
    assert!(trace.dependencies.iter().any(|dependency| {
        dependency.from.name == "stable_parity_parent" && dependency.to.name == "stable_parity"
    }));
}
