use super::*;
use std::time::{Duration, Instant};

fn assert_wait_graph_released(nodes: impl IntoIterator<Item = QueryNodeId>) {
    let wait_graph = query_wait_graph().lock();
    for node in nodes {
        assert!(
            wait_graph
                .edges
                .get(&node)
                .is_none_or(|targets| targets.is_empty())
        );
        assert!(
            !wait_graph
                .edges
                .values()
                .any(|targets| targets.contains_key(&node))
        );
        assert!(!wait_graph.incoming.contains_key(&node));
        assert!(!wait_graph.frames.contains_key(&node));
    }
}

#[test]
fn wait_graph_tracks_parallel_waits_and_detects_cycles_through_any_branch() {
    let query_node = |index| QueryNodeId {
        db_id: QueryDbId(1),
        index,
    };
    let query_frame = |name| QueryFrame {
        name,
        stats_category: None,
        key: Arc::from(name),
        description: Arc::from(name),
    };
    let first = query_node(1);
    let second = query_node(2);
    let third = query_node(3);
    let fourth = query_node(4);
    let mut graph = QueryWaitGraph::default();

    assert_eq!(
        graph
            .begin(first, query_frame("first"), second, query_frame("second"))
            .expect("first edge"),
        None
    );
    assert_eq!(
        graph
            .begin(first, query_frame("first"), third, query_frame("third"))
            .expect("second concurrent edge"),
        None
    );
    assert_eq!(
        graph
            .begin(third, query_frame("third"), fourth, query_frame("fourth"))
            .expect("third edge"),
        None
    );

    let cycle = graph
        .begin(fourth, query_frame("fourth"), first, query_frame("first"))
        .expect("cycle check")
        .expect("branch should close a cycle");
    assert_eq!(
        cycle.iter().map(|frame| frame.name).collect::<Vec<_>>(),
        vec!["fourth", "first", "third", "fourth"]
    );

    graph.end(first, second).expect("release first edge");
    graph.end(first, third).expect("release second edge");
    graph.end(third, fourth).expect("release third edge");
    assert!(graph.edges.is_empty());
    assert!(graph.frames.is_empty());
}

#[test]
fn wait_graph_reference_counts_repeated_parallel_edges() {
    let first = QueryNodeId {
        db_id: QueryDbId(1),
        index: 1,
    };
    let second = QueryNodeId {
        db_id: QueryDbId(1),
        index: 2,
    };
    let frame = |name| QueryFrame {
        name,
        stats_category: None,
        key: Arc::from(name),
        description: Arc::from(name),
    };
    let mut graph = QueryWaitGraph::default();

    graph
        .begin(first, frame("first"), second, frame("second"))
        .expect("first waiter");
    graph
        .begin(first, frame("first"), second, frame("second"))
        .expect("second waiter on same slot");
    graph.end(first, second).expect("release one waiter");
    assert_eq!(graph.edges[&first].get(&second).copied(), Some(1));
    assert!(
        graph
            .begin(second, frame("second"), first, frame("first"))
            .expect("detect repeated-edge cycle")
            .is_some()
    );
    graph.end(first, second).expect("release final waiter");
    assert!(graph.edges.is_empty());
    assert!(graph.incoming.is_empty());
    assert!(graph.frames.is_empty());
}

#[test]
fn wait_graph_detects_a_single_node_cycle() {
    let node = QueryNodeId {
        db_id: QueryDbId(1),
        index: 1,
    };
    let frame = QueryFrame {
        name: "self_wait",
        stats_category: None,
        key: Arc::from("self_wait"),
        description: Arc::from("self_wait"),
    };
    let mut graph = QueryWaitGraph::default();

    let cycle = graph
        .begin(node, frame.clone(), node, frame)
        .expect("self-cycle check")
        .expect("self-cycle should be rejected");
    assert_eq!(cycle.len(), 2);
    assert_eq!(cycle[0].name, "self_wait");
    assert_eq!(cycle[1].name, "self_wait");
    assert!(graph.edges.is_empty());
}

#[test]
fn reports_same_thread_query_cycles() {
    let db = QueryDb::new_for_test(TestContext {
        executions: AtomicUsize::new(0),
    });

    let error = db.get(Recursive).expect_err("cycle should be reported");
    let cycle = match error {
        QueryError::Cycle { cycle } => cycle,
        QueryError::InvalidInput { .. } | QueryError::Internal(_) => {
            panic!("expected query cycle")
        }
    };
    assert_eq!(cycle.len(), 2);
    assert!(cycle.iter().all(|frame| frame.name == "recursive"));
}

struct ParallelBatchWaitContext {
    started: Arc<(Mutex<usize>, Condvar)>,
    release: Arc<(Mutex<bool>, Condvar)>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
struct ParallelBatchWaitChild(usize);

impl QueryKey<ParallelBatchWaitContext> for ParallelBatchWaitChild {
    type Value = usize;

    fn name() -> &'static str {
        "parallel_batch_wait_child"
    }

    fn description(&self) -> String {
        format!("parallel_batch_wait_child({})", self.0)
    }

    fn execute_result(&self, db: &QueryDb<ParallelBatchWaitContext>) -> QueryResult<Self::Value> {
        let (started, started_changed) = &*db.context().started;
        *started.lock() += 1;
        started_changed.notify_all();

        let (released, release_changed) = &*db.context().release;
        let mut released = released.lock();
        while !*released {
            release_changed.wait(&mut released);
        }
        Ok(self.0 * 10)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
struct ParallelBatchWaitParent;

impl QueryKey<ParallelBatchWaitContext> for ParallelBatchWaitParent {
    type Value = usize;

    fn name() -> &'static str {
        "parallel_batch_wait_parent"
    }

    fn execute_result(&self, db: &QueryDb<ParallelBatchWaitContext>) -> QueryResult<Self::Value> {
        Ok(db
            .get_many([ParallelBatchWaitChild(1), ParallelBatchWaitChild(2)])?
            .into_iter()
            .map(|value| *value)
            .sum())
    }
}

#[test]
fn concurrent_batch_waits_from_one_parent_are_supported() {
    let db = QueryDb::new_with_timings_in_session(
        ParallelBatchWaitContext {
            started: Arc::new((Mutex::new(0), Condvar::new())),
            release: Arc::new((Mutex::new(false), Condvar::new())),
        },
        nia_timing::TimingMode::Off,
        QuerySession::with_parallelism(4),
    )
    .expect("create parallel-wait database");
    let child_nodes = [
        db.slot_for(&ParallelBatchWaitChild(1))
            .expect("create first child slot")
            .node_id,
        db.slot_for(&ParallelBatchWaitChild(2))
            .expect("create second child slot")
            .node_id,
    ];
    let parent_node = db
        .slot_for(&ParallelBatchWaitParent)
        .expect("create parent slot")
        .node_id;
    let parent_db = db.clone();

    let (parent_result, observed_parallel_edges) = std::thread::scope(|scope| {
        let first_db = db.clone();
        let first_child = scope.spawn(move || first_db.get(ParallelBatchWaitChild(1)));
        let second_db = db.clone();
        let second_child = scope.spawn(move || second_db.get(ParallelBatchWaitChild(2)));

        let (started, started_changed) = &*db.context().started;
        let started_deadline = Instant::now() + Duration::from_secs(2);
        let mut started = started.lock();
        while *started < 2 {
            let remaining = started_deadline.saturating_duration_since(Instant::now());
            assert!(!remaining.is_zero(), "both child providers should start");
            assert!(
                !started_changed
                    .wait_for(&mut started, remaining)
                    .timed_out(),
                "both child providers should start"
            );
        }
        drop(started);

        let parent = scope.spawn(move || parent_db.get(ParallelBatchWaitParent));
        let wait_deadline = Instant::now() + Duration::from_secs(2);
        let observed_parallel_edges = loop {
            let count = query_wait_graph()
                .lock()
                .edges
                .get(&parent_node)
                .map_or(0, |targets| targets.len());
            if count >= 2 {
                break true;
            }
            if Instant::now() >= wait_deadline {
                break false;
            }
            std::thread::sleep(Duration::from_millis(1));
        };

        let (released, release_changed) = &*db.context().release;
        *released.lock() = true;
        release_changed.notify_all();
        first_child
            .join()
            .expect("first child waiter")
            .expect("first child query should succeed");
        second_child
            .join()
            .expect("second child waiter")
            .expect("second child query should succeed");
        let parent_result = parent.join().expect("parent batch waiter");
        (parent_result, observed_parallel_edges)
    });

    assert!(observed_parallel_edges, "parent should wait on both slots");
    assert_eq!(*parent_result.expect("parallel batch should succeed"), 30);
    db.session()
        .inner
        .ensure_healthy()
        .expect("wait graph should leave session reusable");
    assert_wait_graph_released([parent_node, child_nodes[0], child_nodes[1]]);
    let dependencies = db.query_trace().expect("query trace").dependencies;
    assert_eq!(
        dependencies
            .iter()
            .filter(|dependency| {
                dependency.from.name == "parallel_batch_wait_parent"
                    && dependency.to.name == "parallel_batch_wait_child"
            })
            .count(),
        2,
        "both batched query dependencies should be merged into the parent"
    );
}

#[test]
fn get_returns_query_error() {
    let db = QueryDb::new_for_test(TestContext {
        executions: AtomicUsize::new(0),
    });

    let error = db.get(Recursive).expect_err("cycle must fail");
    assert!(matches!(error, QueryError::Cycle { .. }));
}

#[test]
fn query_can_report_invalid_input_as_query_error() {
    let db = QueryDb::new_for_test(TestContext {
        executions: AtomicUsize::new(0),
    });

    let err = db
        .get(InvalidInputQuery)
        .expect_err("invalid input should be a query error");
    match err {
        QueryError::InvalidInput { query, message } => {
            assert_eq!(query.name, "invalid_input_query");
            assert_eq!(message, "bad fixture");
        }
        QueryError::Cycle { .. } | QueryError::Internal(_) => {
            panic!("expected invalid input error")
        }
    }
}

#[test]
fn get_many_reports_query_failures_as_values() {
    let db = QueryDb::new_for_test(TestContext {
        executions: AtomicUsize::new(0),
    });

    let err = db
        .get_many([InvalidInputQuery])
        .expect_err("invalid batch input should be a query error");
    match err {
        QueryError::InvalidInput { query, message } => {
            assert_eq!(query.name, "invalid_input_query");
            assert_eq!(message, "bad fixture");
        }
        QueryError::Cycle { .. } | QueryError::Internal(_) => {
            panic!("expected invalid input error")
        }
    }
}

#[test]
fn failed_parent_query_drops_speculative_dependencies() {
    let db = QueryDb::new_for_test(TestContext {
        executions: AtomicUsize::new(0),
    });

    let err = db
        .get(InvalidAfterDependency)
        .expect_err("parent query should fail after recording dependency");
    match err {
        QueryError::InvalidInput { query, message } => {
            assert_eq!(query.name, "invalid_after_dependency");
            assert_eq!(message, "failed after dependency");
        }
        QueryError::Cycle { .. } | QueryError::Internal(_) => {
            panic!("expected invalid input error")
        }
    }
    assert!(
        db.query_trace()
            .expect("query trace")
            .dependencies
            .is_empty()
    );

    let invalidation = db.invalidate(Double(3)).expect("invalidate query");
    assert_eq!(
        invalidation
            .invalidated
            .iter()
            .map(|frame| frame.description.as_ref())
            .collect::<Vec<_>>(),
        vec!["double(3)"]
    );
}

#[test]
fn get_many_workers_detect_cycles_through_parent_stack() {
    let db = QueryDb::new_for_test(TestContext {
        executions: AtomicUsize::new(0),
    });
    let worker_db = db.clone();
    let (sender, receiver) = std::sync::mpsc::channel();

    std::thread::spawn(move || {
        let error = worker_db
            .get(ParallelRecursive)
            .expect_err("parallel recursive query should fail");
        sender
            .send(matches!(error, QueryError::Cycle { .. }))
            .expect("send query result");
    });

    assert_eq!(
        receiver.recv_timeout(std::time::Duration::from_secs(2)),
        Ok(true)
    );
}

#[test]
fn distinct_workers_detect_cross_stack_query_cycles() {
    let session = QuerySession::with_parallelism(2);
    let db = QueryDb::new_with_timings_in_session(
        TestContext {
            executions: AtomicUsize::new(0),
        },
        nia_timing::TimingMode::Off,
        session,
    )
    .expect("create query database");
    let cycle_nodes = [
        db.slot_for(&ParallelCycle::Left)
            .expect("create left slot")
            .node_id,
        db.slot_for(&ParallelCycle::Right)
            .expect("create right slot")
            .node_id,
    ];
    let worker_db = db.clone();
    let (sender, receiver) = std::sync::mpsc::channel();

    let worker = std::thread::spawn(move || {
        let result = worker_db.get_many([ParallelCycle::Left, ParallelCycle::Right]);
        sender
            .send(matches!(result, Err(QueryError::Cycle { .. })))
            .expect("send cross-stack cycle result");
    });

    assert_eq!(
        receiver.recv_timeout(std::time::Duration::from_secs(2)),
        Ok(true),
        "parallel query cycle must fail instead of leaving both workers blocked"
    );
    worker.join().expect("parallel cycle worker");
    assert_wait_graph_released(cycle_nodes);
}

#[test]
fn distinct_sessions_detect_cross_stack_query_cycles() {
    let barrier = Arc::new(Barrier::new(2));
    let left_link = Arc::new(Mutex::new(None));
    let right_link = Arc::new(Mutex::new(None));
    let left_db = QueryDb::new_with_timings_in_session(
        CrossSessionCycleContext {
            other: Arc::clone(&right_link),
            barrier: Arc::clone(&barrier),
            executions: AtomicUsize::new(0),
        },
        nia_timing::TimingMode::Off,
        QuerySession::with_parallelism(1),
    )
    .expect("create left query database");
    let right_db = QueryDb::new_with_timings_in_session(
        CrossSessionCycleContext {
            other: Arc::clone(&left_link),
            barrier,
            executions: AtomicUsize::new(0),
        },
        nia_timing::TimingMode::Off,
        QuerySession::with_parallelism(1),
    )
    .expect("create right query database");
    let cycle_nodes = [
        left_db
            .slot_for(&CrossSessionCycle::Left)
            .expect("create left slot")
            .node_id,
        right_db
            .slot_for(&CrossSessionCycle::Right)
            .expect("create right slot")
            .node_id,
    ];
    *left_link.lock() = Some(left_db.clone());
    *right_link.lock() = Some(right_db.clone());
    let (sender, receiver) = std::sync::mpsc::channel();
    let left_sender = sender.clone();
    let left_thread = std::thread::spawn(move || {
        let result = left_db.get(CrossSessionCycle::Left);
        left_sender
            .send(matches!(result, Err(QueryError::Cycle { .. })))
            .expect("send left cross-session cycle result");
    });
    let right_thread = std::thread::spawn(move || {
        let result = right_db.get(CrossSessionCycle::Right);
        sender
            .send(matches!(result, Err(QueryError::Cycle { .. })))
            .expect("send right cross-session cycle result");
    });

    assert_eq!(
        receiver.recv_timeout(std::time::Duration::from_secs(2)),
        Ok(true)
    );
    assert_eq!(
        receiver.recv_timeout(std::time::Duration::from_secs(2)),
        Ok(true)
    );
    left_thread.join().expect("left cycle worker");
    right_thread.join().expect("right cycle worker");
    *left_link.lock() = None;
    *right_link.lock() = None;
    assert_wait_graph_released(cycle_nodes);
}

#[test]
fn failing_query_reports_internal_error_and_taints_session() {
    let db = QueryDb::new_for_test(TestContext {
        executions: AtomicUsize::new(0),
    });

    let first = db.get(FailsOnce).expect_err("first query should fail");
    assert!(matches!(first, QueryError::Internal(_)));

    assert!(matches!(db.get(FailsOnce), Err(QueryError::Internal(_))));
    assert_eq!(db.context().executions.load(Ordering::SeqCst), 1);
}
