use std::collections::BTreeMap;
use std::fs;
use std::path::Path;

use serde_json::{Map, Value, json};

use super::reports::BUILD_STAGE_NAMES;
use super::*;
use crate::test_support::TestDirectory;

fn timing_report(wall: f64, rss: i64, counters: Value) -> Value {
    json!({
        "schema_version": 1,
        "process": {"wall_seconds": wall, "max_rss_bytes": rss},
        "timings": BUILD_STAGE_NAMES.iter().enumerate().map(|(index, name)| json!({
            "kind": "stage",
            "name": name,
            "count": 1,
            "total_seconds": index as f64 + 0.1,
            "max_seconds": index as f64 + 0.1,
        })).collect::<Vec<_>>(),
        "counters": counters,
    })
}

fn fixture_result(process_id: u32, name: &str, counters: Map<String, Value>) -> BuildResult {
    BuildResult {
        name: name.to_owned(),
        command: Vec::new(),
        process_id,
        return_code: 0,
        wall_seconds_observed: 1.0,
        available_memory_bytes_before: None,
        corrupted_action_cache_entries: None,
        artifact_equivalence: None,
        reports: BuildReports {
            actions: ActionReport {
                schema_version: 1,
                kind: "actions".to_owned(),
                success: name != "failed_action",
                counters: BTreeMap::new(),
            },
            measurement: Measurement {
                process: Map::new(),
                stages: BTreeMap::new(),
                counters,
            },
            outer_timing: Value::Null,
        },
    }
}

fn passing_results() -> Vec<BuildResult> {
    let values = |items: &[(&str, i64)]| {
        items
            .iter()
            .map(|(name, value)| ((*name).to_owned(), json!(value)))
            .collect()
    };
    let mut next_process_id = 1000;
    let mut result = |name: &str, counters| {
        next_process_id += 1;
        fixture_result(next_process_id, name, counters)
    };
    let mut results = vec![
        result(
            "clean",
            values(&[
                ("build.action_cache_lookups", 3),
                ("build.action_cache_misses", 3),
            ]),
        ),
        result(
            "warm",
            values(&[
                ("build.action_cache_lookups", 3),
                ("build.action_cache_hits", 3),
                ("build.action_cache_misses", 0),
                ("llvm.object_reuse_misses", 0),
                ("link.result_reuse_misses", 0),
            ]),
        ),
        result(
            "source_edit",
            values(&[
                ("build.action_cache_lookups", 3),
                ("build.action_cache_misses", 1),
                ("build.action_cache_invalidation_sources", 1),
                ("llvm.object_reuse_misses", 1),
                ("link.result_reuse_misses", 1),
            ]),
        ),
        result(
            "source_edit_clean",
            values(&[
                ("build.action_cache_lookups", 3),
                ("build.action_cache_misses", 3),
                ("build.action_cache_hits", 0),
            ]),
        ),
        result(
            "module_map_edit",
            values(&[
                ("build.action_cache_lookups", 3),
                ("build.action_cache_misses", 1),
                ("build.action_cache_invalidation_module", 1),
                ("llvm.object_reuse_misses", 1),
                ("link.result_reuse_misses", 1),
            ]),
        ),
        result(
            "module_map_edit_clean",
            values(&[
                ("build.action_cache_lookups", 3),
                ("build.action_cache_misses", 3),
                ("build.action_cache_hits", 0),
            ]),
        ),
        result(
            "corrupt_cache",
            values(&[
                ("build.action_cache_lookups", 3),
                ("build.action_cache_misses", 3),
                ("build.action_cache_miss_corrupt", 3),
                ("build.action_cache_hits", 0),
                ("llvm.object_reuse_misses", 0),
                ("link.result_reuse_misses", 0),
            ]),
        ),
        result(
            "recovered_warm",
            values(&[
                ("build.action_cache_lookups", 3),
                ("build.action_cache_hits", 3),
                ("build.action_cache_misses", 0),
                ("llvm.object_reuse_misses", 0),
                ("link.result_reuse_misses", 0),
            ]),
        ),
        result(
            "failed_action",
            values(&[
                ("build.steps_executed", 0),
                ("build.actions_executed", 0),
                ("build.action_cache_lookups", 0),
                ("build.action_failures", 1),
            ]),
        ),
    ];
    results[2].artifact_equivalence = Some(ArtifactEquivalence {
        clean_state: "source_edit_clean".to_owned(),
        comparisons: vec![
            ArtifactComparison {
                name: "source-app".to_owned(),
                source_modules: vec!["src/main.nia".to_owned(), "deps/helper.nia".to_owned()],
                matching: true,
            },
            ArtifactComparison {
                name: "generated-app".to_owned(),
                source_modules: vec!["generated.nia".to_owned()],
                matching: true,
            },
        ],
    });
    results[4].artifact_equivalence = Some(ArtifactEquivalence {
        clean_state: "module_map_edit_clean".to_owned(),
        comparisons: results[2]
            .artifact_equivalence
            .as_ref()
            .unwrap()
            .comparisons
            .clone(),
    });
    results[6].corrupted_action_cache_entries = Some(3);
    results
}

#[test]
fn extracts_outer_build_measurement() {
    let outer = timing_report(
        0.4,
        20,
        json!({
            "build.runner_compilations": 1,
            "build.runner_executions": 1,
            "llvm.object_reuse_misses": 0,
        }),
    );
    let parsed = parse_build_reports(
        &format!("diagnostic\n{}", serde_json::to_string(&outer).unwrap()),
        true,
    )
    .unwrap();
    assert!(parsed.actions.success);
    assert_eq!(
        parsed.measurement.stages["build_compile_runner"]["total_seconds"],
        json!(2.1)
    );
    assert_eq!(
        parsed.measurement.counters["llvm.object_reuse_misses"],
        json!(0)
    );
}

#[test]
fn rejects_missing_or_malformed_timing_entries() {
    let missing = json!({"schema_version":1,"process":{},"timings":[],"counters":{"build.runner_executions":1}});
    assert!(
        parse_build_reports(&missing.to_string(), true)
            .unwrap_err()
            .contains("build_resolve_invocation")
    );
    let malformed = json!({"schema_version":1,"process":{},"timings":[1],"counters":{"build.runner_executions":1}});
    assert!(
        parse_build_reports(&malformed.to_string(), true)
            .unwrap_err()
            .contains("non-object")
    );
}

#[test]
fn acceptance_retains_failed_counter_evidence() {
    let mut results = passing_results();
    assert!(workload_acceptance(&results).unwrap().passed);
    results[1]
        .reports
        .measurement
        .counters
        .insert("llvm.object_reuse_misses".to_owned(), json!(15));
    let acceptance = workload_acceptance(&results).unwrap();
    assert!(!acceptance.passed);
    assert!(
        acceptance
            .checks
            .iter()
            .any(|check| check.counter == "llvm.object_reuse_misses" && check.found == 15)
    );
}

#[test]
fn acceptance_requires_typed_edit_invalidation() {
    let mut results = passing_results();
    results[2]
        .reports
        .measurement
        .counters
        .remove("build.action_cache_invalidation_sources");
    results[4]
        .reports
        .measurement
        .counters
        .remove("build.action_cache_invalidation_module");
    let acceptance = workload_acceptance(&results).unwrap();
    assert!(!acceptance.passed);
    assert!(acceptance.checks.iter().any(|check| {
        check.state == "source_edit"
            && check.counter == "build.action_cache_invalidation_sources"
            && !check.passed
    }));
    assert!(acceptance.checks.iter().any(|check| {
        check.state == "module_map_edit"
            && check.counter == "build.action_cache_invalidation_module"
            && !check.passed
    }));
}

#[test]
fn acceptance_requires_failed_action_and_recovered_warm_evidence() {
    let mut results = passing_results();
    results[7]
        .reports
        .measurement
        .counters
        .insert("build.action_cache_hits".to_owned(), json!(2));
    results[8]
        .reports
        .measurement
        .counters
        .insert("build.actions_executed".to_owned(), json!(1));
    let acceptance = workload_acceptance(&results).unwrap();
    assert!(!acceptance.passed);
    assert!(
        acceptance
            .checks
            .iter()
            .any(|check| check.state == "recovered_warm" && !check.passed)
    );
    assert!(acceptance.checks.iter().any(|check| {
        check.state == "failed_action" && check.counter == "build.actions_executed" && !check.passed
    }));
}

#[test]
fn acceptance_requires_independent_clean_artifact_comparison() {
    let mut results = passing_results();
    results[2].artifact_equivalence = None;
    assert!(
        workload_acceptance(&results)
            .unwrap_err()
            .contains("no independent clean artifact comparison")
    );
}

#[test]
fn acceptance_requires_distinct_processes_for_each_state() {
    let mut results = passing_results();
    results[3].process_id = results[2].process_id;
    let acceptance = workload_acceptance(&results).unwrap();
    assert!(!acceptance.passed);
    assert!(acceptance.checks.iter().any(|check| {
        check.state == "workload"
            && check.counter == "baseline.distinct_state_processes"
            && check.expected == ExpectedValue::Exact(9)
            && check.found == 8
            && !check.passed
    }));
}

#[test]
fn acceptance_requires_multi_module_artifact_equivalence() {
    let mut results = passing_results();
    results[2]
        .artifact_equivalence
        .as_mut()
        .unwrap()
        .comparisons[0]
        .matching = false;
    let acceptance = workload_acceptance(&results).unwrap();
    assert!(!acceptance.passed);
    assert!(acceptance.checks.iter().any(|check| {
        check.state == "source_edit"
            && check.counter == "baseline.clean_equivalent_multi_module_artifacts"
            && check.expected == ExpectedValue::Exact(1)
            && check.found == 0
            && !check.passed
    }));
}

#[test]
fn acceptance_retains_artifact_mismatch_evidence() {
    let mut results = passing_results();
    results[4]
        .artifact_equivalence
        .as_mut()
        .unwrap()
        .comparisons[1]
        .matching = false;
    let acceptance = workload_acceptance(&results).unwrap();
    assert!(!acceptance.passed);
    assert!(acceptance.checks.iter().any(|check| {
        check.state == "module_map_edit"
            && check.counter == "baseline.clean_equivalent_artifacts"
            && check.expected == ExpectedValue::Exact(2)
            && check.found == 1
            && !check.passed
    }));
}

#[test]
fn acceptance_requires_cold_independent_recomputation() {
    let mut results = passing_results();
    results[3]
        .reports
        .measurement
        .counters
        .insert("build.action_cache_hits".to_owned(), json!(1));
    results[3]
        .reports
        .measurement
        .counters
        .insert("build.action_cache_misses".to_owned(), json!(2));
    let acceptance = workload_acceptance(&results).unwrap();
    assert!(!acceptance.passed);
    assert!(acceptance.checks.iter().any(|check| {
        check.state == "source_edit_clean"
            && check.counter == "build.action_cache_hits"
            && !check.passed
    }));
    assert!(acceptance.checks.iter().any(|check| {
        check.state == "source_edit_clean"
            && check.counter == "build.action_cache_misses"
            && !check.passed
    }));
}

#[test]
fn rejects_boolean_counters() {
    let mut results = passing_results();
    results[0]
        .reports
        .measurement
        .counters
        .insert("build.action_cache_hits".to_owned(), json!(true));
    assert!(
        workload_acceptance(&results)
            .unwrap_err()
            .contains("not an integer")
    );
}

#[test]
fn summarizes_repeated_stage_and_counter_samples() {
    let mut runs = Vec::new();
    for index in 0..3 {
        let outer = timing_report(
            (index + 1) as f64,
            (index + 1) * 10,
            json!({
                "build.runner_compilations": 1,
                "build.runner_executions": 1,
            }),
        );
        let reports = parse_build_reports(&outer.to_string(), true).unwrap();
        runs.push(vec![BuildResult {
            name: "warm".to_owned(),
            command: Vec::new(),
            process_id: index as u32 + 1,
            return_code: 0,
            wall_seconds_observed: (index + 1) as f64,
            available_memory_bytes_before: None,
            corrupted_action_cache_entries: None,
            artifact_equivalence: None,
            reports,
        }]);
    }
    let summary = summarize_runs(&runs).unwrap().remove(0);
    assert_eq!(summary.wall_seconds_observed.median, Number::Float(2.0));
    assert_eq!(summary.wall_seconds_observed.p95, Number::Float(3.0));
    assert_eq!(summary.process["max_rss_bytes"].median, Number::Integer(20));
    assert_eq!(
        summary.counters["build.runner_executions"].min,
        Number::Integer(1)
    );
}

#[test]
fn corrupts_only_action_entries() {
    let directory = TestDirectory::new("action-corruption");
    directory.write(".nia-cache/actions/generated/v1/a/one.entry", "valid");
    directory.write(".nia-cache/actions/emits/v3/b/two.entry", "valid");
    directory.write(".nia-cache/actions/emits/v3/b/lock.tmp", "valid");
    assert_eq!(corrupt_action_cache(directory.path()).unwrap(), 2);
    assert_eq!(
        fs::read(
            directory
                .path()
                .join(".nia-cache/actions/generated/v1/a/one.entry")
        )
        .unwrap(),
        b"nia build baseline injected corruption\n"
    );
    assert_eq!(
        fs::read_to_string(
            directory
                .path()
                .join(".nia-cache/actions/emits/v3/b/lock.tmp")
        )
        .unwrap(),
        "valid"
    );
}

#[test]
fn build_command_places_named_step_after_build() {
    let command = build_command(
        Path::new("/tool/nia"),
        Path::new("/tool/lib"),
        Path::new("/tmp/package"),
        Some("fail"),
    );
    assert_eq!(
        &command[..5],
        ["/tool/nia", "--resource-root", "/tool/lib", "build", "fail"]
    );
    assert!(command.contains(&"--timings=detail".to_owned()));
}
