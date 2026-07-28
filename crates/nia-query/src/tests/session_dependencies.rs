use super::*;

#[test]
fn shared_session_records_and_invalidates_cross_database_dependencies() {
    let session = QuerySession::new();
    let value = Arc::new(AtomicUsize::new(3));
    let input_db = QueryDb::new_with_timings_in_session(
        SessionInputContext {
            value: Arc::clone(&value),
        },
        nia_timing::TimingMode::Off,
        session.clone(),
    );
    let executions = Arc::new(AtomicUsize::new(0));
    let parent_db = QueryDb::new_with_timings_in_session(
        SessionParentContext {
            input_db: input_db.clone(),
            executions: Arc::clone(&executions),
        },
        nia_timing::TimingMode::Off,
        session,
    );

    assert!(parent_db.session().ptr_eq(&input_db.session()));
    assert_eq!(*parent_db.expect_get(SessionParent), 6);
    value.store(4, Ordering::SeqCst);
    let invalidation = input_db.invalidate(SessionInput);

    assert!(
        invalidation
            .invalidated
            .iter()
            .any(|frame| frame.name == "session_parent")
    );
    assert_eq!(*parent_db.expect_get(SessionParent), 8);
    assert_eq!(executions.load(Ordering::SeqCst), 2);
    assert!(
        parent_db
            .query_trace()
            .dependencies
            .iter()
            .any(|dependency| {
                dependency.from.name == "session_parent" && dependency.to.name == "session_input"
            })
    );
}

#[test]
fn separate_sessions_do_not_record_cross_database_dependencies() {
    let value = Arc::new(AtomicUsize::new(3));
    let input_db = QueryDb::new(SessionInputContext {
        value: Arc::clone(&value),
    });
    let executions = Arc::new(AtomicUsize::new(0));
    let parent_db = QueryDb::new(SessionParentContext {
        input_db: input_db.clone(),
        executions: Arc::clone(&executions),
    });

    assert!(!parent_db.session().ptr_eq(&input_db.session()));
    assert_eq!(*parent_db.expect_get(SessionParent), 6);
    value.store(4, Ordering::SeqCst);
    let invalidation = input_db.invalidate(SessionInput);

    assert!(
        invalidation
            .invalidated
            .iter()
            .all(|frame| frame.name != "session_parent")
    );
    assert_eq!(*parent_db.expect_get(SessionParent), 6);
    assert_eq!(executions.load(Ordering::SeqCst), 1);
}
