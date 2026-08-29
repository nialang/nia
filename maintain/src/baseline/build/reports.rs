use std::collections::BTreeMap;

use serde_json::{Map, Value};

use super::{ActionReport, BuildReports, Measurement};
use crate::MaintainResult;

pub(super) const BUILD_STAGE_NAMES: [&str; 5] = [
    "build_resolve_invocation",
    "build_prepare_directories",
    "build_compile_runner",
    "build_run_runner",
    "build_execute_plan",
];

const MEASUREMENT_COUNTER_PREFIXES: [&str; 7] = [
    "build.",
    "compiler.check_certificate_",
    "frontend.",
    "link.result_",
    "llvm.",
    "query.cache_hits",
    "query.executions",
];

fn json_lines(stderr: &str) -> Vec<Map<String, Value>> {
    stderr
        .lines()
        .filter_map(|line| serde_json::from_str::<Value>(line).ok())
        .filter_map(|value| match value {
            Value::Object(report)
                if report.get("schema_version").and_then(Value::as_u64) == Some(1) =>
            {
                Some(report)
            }
            _ => None,
        })
        .collect()
}

/// Extracts one outer build measurement and action report from mixed stderr.
pub fn parse_build_reports(stderr: &str, succeeded: bool) -> MaintainResult<BuildReports> {
    // stderr deliberately mixes diagnostics with JSON timing lines. The outer
    // build invocation is identified structurally by its runner counter rather
    // than by line order, which may change as nested compiler reports evolve.
    let mut timings = Vec::<Map<String, Value>>::new();
    for report in json_lines(stderr) {
        if report.get("kind").is_some_and(|value| !value.is_null()) {
            continue;
        }
        if !report.get("process").is_some_and(Value::is_object)
            || !report.get("counters").is_some_and(Value::is_object)
        {
            continue;
        }
        let Some(entries) = report.get("timings").and_then(Value::as_array) else {
            continue;
        };
        if entries.iter().any(|entry| !entry.is_object()) {
            return Err("build timing report timings contain a non-object entry".to_owned());
        }
        timings.push(report);
    }
    let outer = timings
        .into_iter()
        .filter(|report| {
            report["counters"]
                .as_object()
                .is_some_and(|counters| counters.contains_key("build.runner_executions"))
        })
        .collect::<Vec<_>>();
    if outer.len() != 1 {
        return Err(format!(
            "expected one outer build timing report, found {}",
            outer.len()
        ));
    }
    let outer = outer.into_iter().next().expect("one outer report");
    let counters = outer["counters"].as_object().expect("validated counters");
    let timing_entries = outer["timings"].as_array().expect("validated timings");
    let mut stages = BTreeMap::new();
    for name in BUILD_STAGE_NAMES {
        let entries = timing_entries
            .iter()
            .filter_map(Value::as_object)
            .filter(|entry| {
                entry.get("kind").and_then(Value::as_str) == Some("stage")
                    && entry.get("name").and_then(Value::as_str) == Some(name)
            })
            .collect::<Vec<_>>();
        if entries.len() != 1 {
            return Err(format!(
                "expected one {name:?} timing entry, found {}",
                entries.len()
            ));
        }
        stages.insert(name.to_owned(), entries[0].clone());
    }
    let measured_counters = counters
        .iter()
        .filter(|(name, _)| {
            MEASUREMENT_COUNTER_PREFIXES
                .iter()
                .any(|prefix| name.starts_with(prefix))
        })
        .map(|(name, value)| (name.clone(), value.clone()))
        .collect();
    let mut action_counters = BTreeMap::new();
    for name in [
        "build.steps_executed",
        "build.actions_executed",
        "build.action_failures",
    ] {
        let value = counters
            .get(name)
            .map_or(Some(0), Value::as_i64)
            .ok_or_else(|| format!("outer build counter {name:?} is not an integer"))?;
        action_counters.insert(name.to_owned(), value);
    }
    Ok(BuildReports {
        actions: ActionReport {
            schema_version: 1,
            kind: "nia-build-coordinator-actions".to_owned(),
            success: succeeded,
            counters: action_counters,
        },
        measurement: Measurement {
            process: outer["process"]
                .as_object()
                .expect("validated process")
                .clone(),
            stages,
            counters: measured_counters,
        },
        outer_timing: Value::Object(outer),
    })
}
