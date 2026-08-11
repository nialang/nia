use serde_json::Value;

use super::{AcceptanceCheck, AcceptanceReport, BuildResult, ExpectedValue};
use crate::MaintainResult;

pub(super) fn counter(result: &BuildResult, name: &str) -> MaintainResult<i64> {
    result
        .reports
        .measurement
        .counters
        .get(name)
        .map_or(Some(0), Value::as_i64)
        .ok_or_else(|| format!("counter {name:?} is not an integer"))
}

pub(super) fn validate_workload(results: &[BuildResult]) -> MaintainResult<()> {
    // State order is part of the experiment: every state inherits the cache
    // and filesystem effects of the state before it.
    let expected = [
        "clean",
        "warm",
        "source_edit",
        "module_map_edit",
        "corrupt_cache",
        "recovered_warm",
        "failed_action",
    ];
    if results
        .iter()
        .map(|result| result.name.as_str())
        .ne(expected)
    {
        return Err("build workload states are missing or out of order".to_owned());
    }
    for result in results {
        let expected_success = result.name != "failed_action";
        if result.reports.actions.success != expected_success {
            return Err(format!(
                "build state {:?} has the wrong action status",
                result.name
            ));
        }
        if counter(result, "build.runner_compilations")? != 1 {
            return Err(format!(
                "build state {:?} did not compile exactly one runner",
                result.name
            ));
        }
        if counter(result, "build.runner_executions")? != 1 {
            return Err(format!(
                "build state {:?} did not execute exactly one runner",
                result.name
            ));
        }
    }
    if counter(
        results.last().expect("validated workload"),
        "build.action_failures",
    )? != 1
    {
        return Err("failed action state did not report exactly one action failure".to_owned());
    }
    Ok(())
}

pub fn workload_acceptance(results: &[BuildResult]) -> MaintainResult<AcceptanceReport> {
    if results.len() != 7 {
        return Err("acceptance requires all seven build workload states".to_owned());
    }
    let [
        clean,
        warm,
        source_edit,
        module_map_edit,
        corrupt_cache,
        recovered_warm,
        failed_action,
    ] = results
    else {
        unreachable!()
    };

    // All successful states execute the same action graph. Using the clean
    // lookup count as the graph cardinality catches both missing and surplus
    // work in later states without baking fixture-specific constants here.
    let expected_actions = counter(clean, "build.action_cache_lookups")?;
    let mut checks = Vec::new();

    fn exact(
        checks: &mut Vec<AcceptanceCheck>,
        state: &str,
        result: &BuildResult,
        name: &str,
        expected: i64,
    ) -> MaintainResult<()> {
        let found = counter(result, name)?;
        checks.push(AcceptanceCheck {
            state: state.to_owned(),
            counter: name.to_owned(),
            expected: ExpectedValue::Exact(expected),
            found,
            passed: found == expected,
        });
        Ok(())
    }

    fn positive(
        checks: &mut Vec<AcceptanceCheck>,
        state: &str,
        result: &BuildResult,
        name: &str,
    ) -> MaintainResult<()> {
        let found = counter(result, name)?;
        checks.push(AcceptanceCheck {
            state: state.to_owned(),
            counter: name.to_owned(),
            expected: ExpectedValue::Description("> 0".to_owned()),
            found,
            passed: found > 0,
        });
        Ok(())
    }

    positive(&mut checks, "clean", clean, "build.action_cache_lookups")?;
    exact(
        &mut checks,
        "clean",
        clean,
        "build.action_cache_misses",
        expected_actions,
    )?;
    exact(&mut checks, "clean", clean, "build.action_cache_hits", 0)?;
    exact(
        &mut checks,
        "warm",
        warm,
        "build.action_cache_lookups",
        expected_actions,
    )?;
    exact(
        &mut checks,
        "warm",
        warm,
        "build.action_cache_hits",
        expected_actions,
    )?;
    exact(&mut checks, "warm", warm, "build.action_cache_misses", 0)?;
    exact(&mut checks, "warm", warm, "llvm.object_reuse_misses", 0)?;
    exact(&mut checks, "warm", warm, "link.result_reuse_misses", 0)?;
    exact(
        &mut checks,
        "source_edit",
        source_edit,
        "build.action_cache_lookups",
        expected_actions,
    )?;
    positive(
        &mut checks,
        "source_edit",
        source_edit,
        "build.action_cache_misses",
    )?;
    positive(
        &mut checks,
        "source_edit",
        source_edit,
        "build.action_cache_invalidation_sources",
    )?;
    positive(
        &mut checks,
        "source_edit",
        source_edit,
        "llvm.object_reuse_misses",
    )?;
    positive(
        &mut checks,
        "source_edit",
        source_edit,
        "link.result_reuse_misses",
    )?;
    exact(
        &mut checks,
        "module_map_edit",
        module_map_edit,
        "build.action_cache_lookups",
        expected_actions,
    )?;
    positive(
        &mut checks,
        "module_map_edit",
        module_map_edit,
        "build.action_cache_misses",
    )?;
    positive(
        &mut checks,
        "module_map_edit",
        module_map_edit,
        "build.action_cache_invalidation_module",
    )?;
    positive(
        &mut checks,
        "module_map_edit",
        module_map_edit,
        "llvm.object_reuse_misses",
    )?;
    positive(
        &mut checks,
        "module_map_edit",
        module_map_edit,
        "link.result_reuse_misses",
    )?;

    let corrupted = corrupt_cache
        .corrupted_action_cache_entries
        .unwrap_or_default() as i64;
    checks.push(AcceptanceCheck {
        state: "corrupt_cache".to_owned(),
        counter: "baseline.corrupted_action_cache_entries".to_owned(),
        expected: ExpectedValue::Description("> 0".to_owned()),
        found: corrupted,
        passed: corrupted > 0,
    });
    exact(
        &mut checks,
        "corrupt_cache",
        corrupt_cache,
        "build.action_cache_lookups",
        expected_actions,
    )?;
    exact(
        &mut checks,
        "corrupt_cache",
        corrupt_cache,
        "build.action_cache_misses",
        expected_actions,
    )?;
    exact(
        &mut checks,
        "corrupt_cache",
        corrupt_cache,
        "build.action_cache_miss_corrupt",
        expected_actions,
    )?;
    exact(
        &mut checks,
        "corrupt_cache",
        corrupt_cache,
        "build.action_cache_hits",
        0,
    )?;
    exact(
        &mut checks,
        "corrupt_cache",
        corrupt_cache,
        "llvm.object_reuse_misses",
        0,
    )?;
    exact(
        &mut checks,
        "corrupt_cache",
        corrupt_cache,
        "link.result_reuse_misses",
        0,
    )?;
    exact(
        &mut checks,
        "recovered_warm",
        recovered_warm,
        "build.action_cache_lookups",
        expected_actions,
    )?;
    exact(
        &mut checks,
        "recovered_warm",
        recovered_warm,
        "build.action_cache_hits",
        expected_actions,
    )?;
    exact(
        &mut checks,
        "recovered_warm",
        recovered_warm,
        "build.action_cache_misses",
        0,
    )?;
    exact(
        &mut checks,
        "recovered_warm",
        recovered_warm,
        "llvm.object_reuse_misses",
        0,
    )?;
    exact(
        &mut checks,
        "recovered_warm",
        recovered_warm,
        "link.result_reuse_misses",
        0,
    )?;
    exact(
        &mut checks,
        "failed_action",
        failed_action,
        "build.steps_executed",
        0,
    )?;
    exact(
        &mut checks,
        "failed_action",
        failed_action,
        "build.actions_executed",
        0,
    )?;
    exact(
        &mut checks,
        "failed_action",
        failed_action,
        "build.action_cache_lookups",
        0,
    )?;
    exact(
        &mut checks,
        "failed_action",
        failed_action,
        "build.action_failures",
        1,
    )?;
    Ok(AcceptanceReport {
        passed: checks.iter().all(|check| check.passed),
        checks,
    })
}
