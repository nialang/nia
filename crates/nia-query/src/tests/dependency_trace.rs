use super::*;

#[test]
fn records_query_dependencies() {
    let db = QueryDb::new_for_test(TestContext {
        executions: AtomicUsize::new(0),
    });

    assert_eq!(*db.expect_get(DoubleTwice(7)), 28);
    let trace = db.query_trace().expect("query trace");
    assert_eq!(trace.dependencies.len(), 1);
    assert_eq!(trace.dependencies[0].from.name, "double_twice");
    assert_eq!(trace.dependencies[0].to.description.as_ref(), "double(7)");
}

#[test]
fn cloned_query_frames_share_identity_text() {
    let db = QueryDb::new_for_test(TestContext {
        executions: AtomicUsize::new(0),
    });

    assert_eq!(*db.expect_get(DoubleTwice(7)), 28);
    let frame = db
        .query_trace()
        .expect("query trace")
        .queries
        .into_iter()
        .find(|query| query.frame.name == "double_twice")
        .expect("double_twice frame")
        .frame;
    let clone = frame.clone();
    assert!(Arc::ptr_eq(&frame.key, &clone.key));
    assert!(Arc::ptr_eq(&frame.description, &clone.description));
}

#[test]
fn records_query_execution_and_cache_hit_stats() {
    let db = QueryDb::new_for_test(TestContext {
        executions: AtomicUsize::new(0),
    });

    assert_eq!(*db.expect_get(Double(21)), 42);
    assert_eq!(*db.expect_get(Double(21)), 42);
    let trace = db.query_trace().expect("query trace");
    let stats = trace
        .queries
        .iter()
        .find(|query| query.frame.description.as_ref() == "double(21)")
        .map(|query| {
            assert_eq!(query.frame.stats_category, Some("arithmetic"));
            &query.stats
        })
        .expect("double query stats");

    assert_eq!(stats.executions, 1);
    assert_eq!(stats.cache_hits, 1);
    assert_eq!(stats.waits, 0);
}

#[test]
fn records_get_many_dependencies_from_parent_query() {
    let db = QueryDb::new_for_test(TestContext {
        executions: AtomicUsize::new(0),
    });

    assert_eq!(*db.expect_get(DoubleMany([2, 5])), 14);
    let trace = db.query_trace().expect("query trace");

    assert!(trace.dependencies.iter().any(|dependency| {
        dependency.from.name == "double_many" && dependency.to.description.as_ref() == "double(2)"
    }));
    assert!(trace.dependencies.iter().any(|dependency| {
        dependency.from.name == "double_many" && dependency.to.description.as_ref() == "double(5)"
    }));
}

#[test]
fn records_single_item_get_many_dependencies_from_parent_query() {
    let db = QueryDb::new_for_test(TestContext {
        executions: AtomicUsize::new(0),
    });

    assert_eq!(*db.expect_get(SingleDoubleMany(2)), 4);
    let trace = db.query_trace().expect("query trace");

    assert!(trace.dependencies.iter().any(|dependency| {
        dependency.from.name == "single_double_many"
            && dependency.to.description.as_ref() == "double(2)"
    }));

    let invalidation = db.invalidate(Double(2)).expect("invalidate query");
    let invalidated = invalidation
        .invalidated
        .iter()
        .map(|frame| frame.description.to_string())
        .collect::<Vec<_>>();
    assert_eq!(invalidated, vec!["double(2)", "single_double_many(2)"]);
}

#[test]
fn repeated_dependency_reads_publish_one_edge() {
    let db = QueryDb::new_for_test(TestContext {
        executions: AtomicUsize::new(0),
    });

    assert_eq!(*db.expect_get(DuplicateDouble(3)), 12);
    let dependencies = db
        .query_trace()
        .expect("query trace")
        .dependencies
        .into_iter()
        .filter(|dependency| dependency.from.name == "duplicate_double")
        .collect::<Vec<_>>();

    assert_eq!(dependencies.len(), 1);
    assert_eq!(dependencies[0].to.description.as_ref(), "double(3)");
}

#[test]
fn duplicate_task_dependencies_publish_one_edge() {
    let db = QueryDb::new_for_test(TestContext {
        executions: AtomicUsize::new(0),
    });

    assert_eq!(*db.expect_get(DoubleMany([3, 3])), 12);
    let dependencies = db
        .query_trace()
        .expect("query trace")
        .dependencies
        .into_iter()
        .filter(|dependency| dependency.from.name == "double_many")
        .collect::<Vec<_>>();

    assert_eq!(dependencies.len(), 1);
    assert_eq!(dependencies[0].to.description.as_ref(), "double(3)");
}

#[test]
fn wide_task_dependencies_promote_without_losing_invalidation_edges() {
    let db = QueryDb::new_for_test(TestContext {
        executions: AtomicUsize::new(0),
    });

    assert_eq!(
        *db.expect_get(WideDoubleMany([0, 1, 2, 3, 4, 5, 6, 7, 8, 9])),
        90
    );
    let trace = db.query_trace().expect("query trace");
    assert_eq!(
        trace
            .dependencies
            .iter()
            .filter(|dependency| dependency.from.name == "wide_double_many")
            .count(),
        10
    );

    let invalidated = db
        .invalidate(Double(8))
        .expect("invalidate dependency")
        .invalidated;
    assert!(
        invalidated
            .iter()
            .any(|frame| frame.name == "wide_double_many")
    );
}
