use super::*;

#[test]
fn records_query_dependencies() {
    let db = QueryDb::new(TestContext {
        executions: AtomicUsize::new(0),
    });

    assert_eq!(*db.expect_get(DoubleTwice(7)), 28);
    let trace = db.query_trace();
    assert_eq!(trace.dependencies.len(), 1);
    assert_eq!(trace.dependencies[0].from.name, "double_twice");
    assert_eq!(trace.dependencies[0].to.description, "double(7)");
}

#[test]
fn records_query_execution_and_cache_hit_stats() {
    let db = QueryDb::new(TestContext {
        executions: AtomicUsize::new(0),
    });

    assert_eq!(*db.expect_get(Double(21)), 42);
    assert_eq!(*db.expect_get(Double(21)), 42);
    let trace = db.query_trace();
    let stats = trace
        .queries
        .iter()
        .find(|query| query.frame.description == "double(21)")
        .map(|query| &query.stats)
        .expect("double query stats");

    assert_eq!(stats.executions, 1);
    assert_eq!(stats.cache_hits, 1);
    assert_eq!(stats.waits, 0);
}

#[test]
fn records_get_many_dependencies_from_parent_query() {
    let db = QueryDb::new(TestContext {
        executions: AtomicUsize::new(0),
    });

    assert_eq!(*db.expect_get(DoubleMany([2, 5])), 14);
    let trace = db.query_trace();

    assert!(trace.dependencies.iter().any(|dependency| {
        dependency.from.name == "double_many" && dependency.to.description == "double(2)"
    }));
    assert!(trace.dependencies.iter().any(|dependency| {
        dependency.from.name == "double_many" && dependency.to.description == "double(5)"
    }));
}

#[test]
fn records_single_item_get_many_dependencies_from_parent_query() {
    let db = QueryDb::new(TestContext {
        executions: AtomicUsize::new(0),
    });

    assert_eq!(*db.expect_get(SingleDoubleMany(2)), 4);
    let trace = db.query_trace();

    assert!(trace.dependencies.iter().any(|dependency| {
        dependency.from.name == "single_double_many" && dependency.to.description == "double(2)"
    }));

    let invalidation = db.invalidate(Double(2));
    let invalidated = invalidation
        .invalidated
        .iter()
        .map(|frame| frame.description.as_str())
        .collect::<Vec<_>>();
    assert_eq!(invalidated, vec!["double(2)", "single_double_many(2)"]);
}
