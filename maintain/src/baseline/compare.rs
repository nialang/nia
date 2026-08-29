use std::collections::{BTreeMap, BTreeSet};
use std::fs;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

use crate::MaintainResult;

#[derive(Debug, Clone)]
/// Inputs and thresholds for comparing two performance baselines.
pub struct Options {
    /// Baseline JSON path.
    pub baseline: PathBuf,
    /// Candidate JSON path.
    pub candidate: PathBuf,
    /// Maximum allowed wall-time regression percentage.
    pub max_wall_regression: f64,
    /// Maximum allowed peak RSS regression percentage.
    pub max_rss_regression: f64,
    /// Maximum allowed query-count regression percentage.
    pub max_query_regression: f64,
    /// Maximum allowed allocation regression percentage.
    pub max_allocation_regression: f64,
    /// Whether to continue despite machine-identity mismatches.
    pub allow_machine_mismatch: bool,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
/// Machine identity and effective resource limits for a baseline run.
pub struct MachineIdentity {
    #[serde(default)]
    /// Optional controlled-runner classification.
    pub runner_class: Option<String>,
    /// Operating-system family.
    pub system: String,
    /// Host architecture.
    pub architecture: String,
    #[serde(default)]
    /// CPU model, when available.
    pub cpu_model: Option<String>,
    #[serde(default)]
    /// Effective CPU quota.
    pub effective_cpu_limit: Option<f64>,
    #[serde(default)]
    /// Effective memory limit in bytes.
    pub effective_memory_limit_bytes: Option<f64>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
/// Process-level metrics captured for one workload sample.
pub struct ProcessMetrics {
    /// Wall-clock execution time in seconds.
    pub wall_seconds: f64,
    /// Peak resident set size in bytes.
    pub max_rss_bytes: f64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
/// Named compiler workload with process metrics and counters.
pub struct PerformanceResult {
    /// Stable workload name.
    pub name: String,
    /// Process measurements for the workload.
    pub process: ProcessMetrics,
    /// Named compiler counters.
    pub counters: BTreeMap<String, f64>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
/// Schema-v1 performance baseline and its machine identity.
pub struct PerformanceBaseline {
    /// Baseline schema version.
    pub schema_version: u32,
    /// Machine identity captured with the samples.
    pub machine: MachineIdentity,
    /// Workload samples, including repeated entries.
    pub results: Vec<PerformanceResult>,
}

#[derive(Debug, Clone, Serialize)]
/// One metric comparison and its regression decision.
pub struct MetricComparison {
    /// Workload owning the metric.
    pub workload: String,
    /// Metric name.
    pub metric: String,
    /// Baseline median value.
    pub baseline: f64,
    /// Candidate median value.
    pub candidate: f64,
    /// Relative change percentage, when defined.
    pub change_percent: Option<f64>,
    /// Allowed regression threshold percentage.
    pub threshold_percent: f64,
    /// Whether the metric is within its threshold.
    pub passed: bool,
}

#[derive(Debug, Clone, Serialize)]
/// Complete machine, threshold, and metric comparison report.
pub struct ComparisonReport {
    /// Comparison report schema version.
    pub schema_version: u32,
    /// Whether machine identity matched within tolerance.
    pub machine_compatible: bool,
    /// Human-readable machine mismatches.
    pub machine_mismatches: Vec<String>,
    /// Whether mismatches were explicitly allowed.
    pub allow_machine_mismatch: bool,
    /// Thresholds applied by metric family.
    pub thresholds_percent: BTreeMap<String, f64>,
    /// Per-workload metric decisions.
    pub comparisons: Vec<MetricComparison>,
    /// Structural errors preventing comparison.
    pub errors: Vec<String>,
    /// Whether all structural and metric checks passed.
    pub passed: bool,
}

#[derive(Debug, Clone, Copy)]
enum MetricSource {
    ProcessWall,
    ProcessRss,
    Counter(&'static str),
}

#[derive(Debug, Clone, Copy)]
struct Metric {
    name: &'static str,
    threshold: &'static str,
    source: MetricSource,
}

const METRICS: [Metric; 5] = [
    Metric {
        name: "wall_seconds",
        threshold: "wall",
        source: MetricSource::ProcessWall,
    },
    Metric {
        name: "max_rss_bytes",
        threshold: "rss",
        source: MetricSource::ProcessRss,
    },
    Metric {
        name: "query.executions",
        threshold: "query",
        source: MetricSource::Counter("query.executions"),
    },
    Metric {
        name: "allocator.allocated_bytes",
        threshold: "allocation",
        source: MetricSource::Counter("allocator.allocated_bytes"),
    },
    Metric {
        name: "allocator.peak_live_bytes",
        threshold: "allocation",
        source: MetricSource::Counter("allocator.peak_live_bytes"),
    },
];
const MODULE_FINALIZATION: Metric = Metric {
    name: "backend.module_finalization.peak_growth_bytes",
    threshold: "allocation",
    source: MetricSource::Counter("backend.module_finalization.peak_growth_bytes"),
};

fn validate_baseline(
    baseline: PerformanceBaseline,
    context: &str,
) -> MaintainResult<PerformanceBaseline> {
    if baseline.schema_version != 1 {
        return Err(format!("{context} does not use schema_version=1"));
    }
    let machine_numbers = [
        baseline.machine.effective_cpu_limit,
        baseline.machine.effective_memory_limit_bytes,
    ];
    if machine_numbers
        .into_iter()
        .flatten()
        .any(|value| !value.is_finite())
    {
        return Err(format!(
            "{context} machine identity contains a non-finite number"
        ));
    }
    for result in &baseline.results {
        if !result.process.wall_seconds.is_finite()
            || !result.process.max_rss_bytes.is_finite()
            || result.counters.values().any(|value| !value.is_finite())
        {
            return Err(format!(
                "workload {:?} contains a non-finite metric",
                result.name
            ));
        }
    }
    Ok(baseline)
}

/// Loads and validates one schema-v1 performance baseline JSON file.
pub fn load_baseline(path: &Path) -> MaintainResult<PerformanceBaseline> {
    let source = fs::read_to_string(path)
        .map_err(|error| format!("failed to read baseline {}: {error}", path.display()))?;
    let baseline = serde_json::from_str(&source)
        .map_err(|error| format!("failed to read baseline {}: {error}", path.display()))?;
    validate_baseline(baseline, &format!("baseline {}", path.display()))
}

fn relative_difference(left: f64, right: f64) -> f64 {
    (left - right).abs() / left.abs().max(right.abs()).max(1.0)
}

/// Returns machine identity differences that exceed comparison tolerances.
pub fn machine_mismatches(
    baseline: &PerformanceBaseline,
    candidate: &PerformanceBaseline,
) -> Vec<String> {
    let left = &baseline.machine;
    let right = &candidate.machine;
    let mut mismatches = Vec::new();
    if left.runner_class != right.runner_class {
        mismatches.push(format!(
            "runner_class differs: {:?} != {:?}",
            left.runner_class, right.runner_class
        ));
    }
    if left.system != right.system {
        mismatches.push(format!(
            "system differs: {:?} != {:?}",
            left.system, right.system
        ));
    }
    if left.architecture != right.architecture {
        mismatches.push(format!(
            "architecture differs: {:?} != {:?}",
            left.architecture, right.architecture
        ));
    }
    if left.runner_class.is_none()
        && right.runner_class.is_none()
        && left.cpu_model != right.cpu_model
    {
        mismatches.push(format!(
            "cpu_model differs: {:?} != {:?}",
            left.cpu_model, right.cpu_model
        ));
    }
    for (name, left, right, tolerance) in [
        (
            "effective_cpu_limit",
            left.effective_cpu_limit,
            right.effective_cpu_limit,
            0.01,
        ),
        (
            "effective_memory_limit_bytes",
            left.effective_memory_limit_bytes,
            right.effective_memory_limit_bytes,
            0.10,
        ),
    ] {
        match (left, right) {
            (Some(left), Some(right)) if relative_difference(left, right) > tolerance => {
                mismatches.push(format!("{name} differs: {left:?} != {right:?}"));
            }
            (None, Some(right)) => mismatches.push(format!("{name} differs: None != {right:?}")),
            (Some(left), None) => mismatches.push(format!("{name} differs: {left:?} != None")),
            _ => {}
        }
    }
    mismatches
}

fn grouped_results(baseline: &PerformanceBaseline) -> BTreeMap<&str, Vec<&PerformanceResult>> {
    let mut grouped = BTreeMap::<&str, Vec<&PerformanceResult>>::new();
    for result in &baseline.results {
        grouped.entry(&result.name).or_default().push(result);
    }
    grouped
}

fn metric_value(result: &PerformanceResult, metric: Metric) -> MaintainResult<f64> {
    match metric.source {
        MetricSource::ProcessWall => Ok(result.process.wall_seconds),
        MetricSource::ProcessRss => Ok(result.process.max_rss_bytes),
        MetricSource::Counter(name) => result.counters.get(name).copied().ok_or_else(|| {
            format!(
                "workload {:?} is missing metric {}",
                result.name, metric.name
            )
        }),
    }
}

fn median_metric(results: &[&PerformanceResult], metric: Metric) -> MaintainResult<f64> {
    let mut values = results
        .iter()
        .map(|result| metric_value(result, metric))
        .collect::<MaintainResult<Vec<_>>>()?;
    values.sort_by(f64::total_cmp);
    let middle = values.len() / 2;
    if values.len().is_multiple_of(2) {
        Ok((values[middle - 1] + values[middle]) / 2.0)
    } else {
        Ok(values[middle])
    }
}

fn change_percent(baseline: f64, candidate: f64) -> Option<f64> {
    if baseline == 0.0 {
        (candidate == 0.0).then_some(0.0)
    } else {
        Some((candidate - baseline) * 100.0 / baseline)
    }
}

/// Compares workload medians and applies metric-family regression thresholds.
pub fn compare_baselines(
    baseline: &PerformanceBaseline,
    candidate: &PerformanceBaseline,
    thresholds: &BTreeMap<String, f64>,
    allow_machine_mismatch: bool,
) -> MaintainResult<ComparisonReport> {
    let mismatches = machine_mismatches(baseline, candidate);
    let left = grouped_results(baseline);
    let right = grouped_results(candidate);
    let left_names = left.keys().copied().collect::<BTreeSet<_>>();
    let right_names = right.keys().copied().collect::<BTreeSet<_>>();
    let missing = left_names
        .difference(&right_names)
        .copied()
        .collect::<Vec<_>>();
    let added = right_names
        .difference(&left_names)
        .copied()
        .collect::<Vec<_>>();
    let mut errors = Vec::new();
    if left.is_empty() {
        errors.push("baseline contains no workloads".to_owned());
    }
    if !missing.is_empty() {
        errors.push(format!(
            "candidate is missing workloads: {}",
            missing.join(", ")
        ));
    }
    if !added.is_empty() {
        errors.push(format!(
            "candidate has unexpected workloads: {}",
            added.join(", ")
        ));
    }
    if !mismatches.is_empty() && !allow_machine_mismatch {
        errors.push("machine resources are not comparable".to_owned());
    }
    let mut comparisons = Vec::new();
    if errors.is_empty() {
        for (workload, baseline_results) in &left {
            let mut metrics = METRICS.to_vec();
            if *workload == "module_backend" {
                metrics.push(MODULE_FINALIZATION);
            }
            for metric in metrics {
                let baseline_value = median_metric(baseline_results, metric)?;
                let candidate_value = median_metric(&right[workload], metric)?;
                let change = change_percent(baseline_value, candidate_value);
                let threshold = *thresholds
                    .get(metric.threshold)
                    .ok_or_else(|| format!("missing threshold {}", metric.threshold))?;
                comparisons.push(MetricComparison {
                    workload: (*workload).to_owned(),
                    metric: metric.name.to_owned(),
                    baseline: baseline_value,
                    candidate: candidate_value,
                    change_percent: change,
                    threshold_percent: threshold,
                    passed: change.is_some_and(|value| value <= threshold),
                });
            }
        }
    }
    let passed = errors.is_empty() && comparisons.iter().all(|item| item.passed);
    Ok(ComparisonReport {
        schema_version: 1,
        machine_compatible: mismatches.is_empty(),
        machine_mismatches: mismatches,
        allow_machine_mismatch,
        thresholds_percent: thresholds.clone(),
        comparisons,
        errors,
        passed,
    })
}

/// Loads two baselines, prints a comparison report, and returns pass status.
pub fn run(options: &Options) -> MaintainResult<bool> {
    let thresholds = BTreeMap::from([
        ("wall".to_owned(), options.max_wall_regression),
        ("rss".to_owned(), options.max_rss_regression),
        ("query".to_owned(), options.max_query_regression),
        ("allocation".to_owned(), options.max_allocation_regression),
    ]);
    if thresholds
        .values()
        .any(|value| !value.is_finite() || *value < 0.0)
    {
        return Err("regression thresholds must be finite and non-negative".to_owned());
    }
    let report = compare_baselines(
        &load_baseline(&options.baseline)?,
        &load_baseline(&options.candidate)?,
        &thresholds,
        options.allow_machine_mismatch,
    )?;
    println!(
        "{}",
        serde_json::to_string_pretty(&report)
            .map_err(|error| format!("failed to encode comparison report: {error}"))?
    );
    Ok(report.passed)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn result(
        name: &str,
        wall: f64,
        rss: f64,
        queries: f64,
        allocations: f64,
    ) -> PerformanceResult {
        let mut counters = BTreeMap::from([
            ("query.executions".to_owned(), queries),
            ("allocator.allocated_bytes".to_owned(), allocations),
            ("allocator.peak_live_bytes".to_owned(), allocations / 2.0),
        ]);
        if name == "module_backend" {
            counters.insert(
                "backend.module_finalization.peak_growth_bytes".to_owned(),
                allocations / 10.0,
            );
        }
        PerformanceResult {
            name: name.to_owned(),
            process: ProcessMetrics {
                wall_seconds: wall,
                max_rss_bytes: rss,
            },
            counters,
        }
    }

    fn baseline(
        results: Vec<PerformanceResult>,
        cpu: f64,
        runner: Option<&str>,
        model: &str,
    ) -> PerformanceBaseline {
        PerformanceBaseline {
            schema_version: 1,
            machine: MachineIdentity {
                runner_class: runner.map(str::to_owned),
                system: "Linux".to_owned(),
                architecture: "x86_64".to_owned(),
                cpu_model: Some(model.to_owned()),
                effective_cpu_limit: Some(cpu),
                effective_memory_limit_bytes: Some(16_000_000_000.0),
            },
            results,
        }
    }

    fn thresholds() -> BTreeMap<String, f64> {
        BTreeMap::from([
            ("wall".to_owned(), 50.0),
            ("rss".to_owned(), 30.0),
            ("query".to_owned(), 5.0),
            ("allocation".to_owned(), 20.0),
        ])
    }

    #[test]
    fn uses_iteration_medians_and_accepts_values_under_thresholds() {
        let before = baseline(
            vec![
                result("check", 10.0, 100.0, 1000.0, 1000.0),
                result("check", 12.0, 120.0, 1000.0, 1200.0),
                result("check", 100.0, 1000.0, 1000.0, 10_000.0),
            ],
            8.0,
            None,
            "cpu",
        );
        let after = baseline(
            vec![
                result("check", 12.0, 120.0, 1020.0, 1200.0),
                result("check", 14.0, 130.0, 1020.0, 1300.0),
                result("check", 200.0, 2000.0, 1020.0, 20_000.0),
            ],
            8.0,
            None,
            "cpu",
        );
        let report = compare_baselines(&before, &after, &thresholds(), false).unwrap();
        assert!(report.passed);
        let wall = report
            .comparisons
            .iter()
            .find(|item| item.metric == "wall_seconds")
            .unwrap();
        assert_eq!(wall.baseline, 12.0);
        assert_eq!(wall.candidate, 14.0);
    }

    #[test]
    fn rejects_regressions_and_machine_drift() {
        let before = baseline(
            vec![result("check", 10.0, 100.0, 1000.0, 1000.0)],
            8.0,
            None,
            "cpu-a",
        );
        let regression = baseline(
            vec![result("check", 16.0, 100.0, 1000.0, 1000.0)],
            8.0,
            None,
            "cpu-a",
        );
        assert!(
            !compare_baselines(&before, &regression, &thresholds(), false)
                .unwrap()
                .passed
        );
        let drift = baseline(
            vec![result("check", 10.0, 100.0, 1000.0, 1000.0)],
            4.0,
            None,
            "cpu-b",
        );
        let report = compare_baselines(&before, &drift, &thresholds(), false).unwrap();
        assert!(!report.passed);
        assert!(!report.machine_compatible);
        assert!(report.comparisons.is_empty());
    }

    #[test]
    fn controlled_runner_allows_cpu_model_but_not_resource_drift() {
        let before = baseline(
            vec![result("check", 10.0, 100.0, 1000.0, 1000.0)],
            4.0,
            Some("hosted"),
            "cpu-a",
        );
        let model_drift = baseline(
            vec![result("check", 10.0, 100.0, 1000.0, 1000.0)],
            4.0,
            Some("hosted"),
            "cpu-b",
        );
        assert!(
            compare_baselines(&before, &model_drift, &thresholds(), false)
                .unwrap()
                .passed
        );
        let resource_drift = baseline(
            vec![result("check", 10.0, 100.0, 1000.0, 1000.0)],
            2.0,
            Some("hosted"),
            "cpu-b",
        );
        assert!(
            !compare_baselines(&before, &resource_drift, &thresholds(), false)
                .unwrap()
                .passed
        );
    }

    #[test]
    fn guards_module_finalization_peak_growth() {
        let before = baseline(
            vec![result("module_backend", 10.0, 100.0, 1000.0, 1000.0)],
            8.0,
            None,
            "cpu",
        );
        let after = baseline(
            vec![result("module_backend", 10.0, 100.0, 1000.0, 1300.0)],
            8.0,
            None,
            "cpu",
        );
        let report = compare_baselines(&before, &after, &thresholds(), false).unwrap();
        assert!(report.comparisons.iter().any(|comparison| {
            comparison.metric == "backend.module_finalization.peak_growth_bytes"
                && !comparison.passed
        }));
    }

    #[test]
    fn rejects_boolean_metrics_at_the_schema_boundary() {
        let source = serde_json::json!({
            "schema_version": 1,
            "machine": {"system": "Linux", "architecture": "x86_64"},
            "results": [{
                "name": "check",
                "process": {"wall_seconds": 1.0, "max_rss_bytes": 100},
                "counters": {"query.executions": true},
            }],
        });
        assert!(serde_json::from_value::<PerformanceBaseline>(source).is_err());
    }

    #[test]
    fn rejects_different_runner_classes_and_controlled_local_pairs() {
        let sample = || vec![result("check", 1.0, 1.0, 1.0, 1.0)];
        let hosted = baseline(sample(), 4.0, Some("hosted"), "cpu");
        let self_hosted = baseline(sample(), 4.0, Some("self-hosted"), "cpu");
        let local = baseline(sample(), 4.0, None, "cpu");
        assert!(
            !compare_baselines(&hosted, &self_hosted, &thresholds(), false)
                .unwrap()
                .passed
        );
        assert!(
            !compare_baselines(&hosted, &local, &thresholds(), false)
                .unwrap()
                .passed
        );
    }
}
