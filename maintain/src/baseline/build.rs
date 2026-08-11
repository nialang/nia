use std::collections::{BTreeMap, BTreeSet};
use std::fs;
use std::io::Read;
use std::os::unix::process::CommandExt;
use std::path::{Path, PathBuf};
use std::process::{Command, ExitStatus, Stdio};
use std::thread;
use std::time::{Duration, Instant};

use serde::Serialize;
use serde_json::{Map, Value};

#[cfg(test)]
use serde_json::json;

use crate::system::machine::{MachineMetadata, machine_metadata};
use crate::system::resources::probe_host_resources;
use crate::{MaintainResult, TemporaryDirectory, absolute_path};

const DEFAULT_TIMEOUT_SECONDS: u64 = 420;
const DEFAULT_REPETITIONS: usize = 3;
const MIN_AVAILABLE_MEMORY_BYTES: u64 = 2 * 1024 * 1024 * 1024;
const BUILD_STAGE_NAMES: [&str; 5] = [
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

#[derive(Debug, Clone)]
pub struct Options {
    pub nia: PathBuf,
    pub resource_root: PathBuf,
    pub fixture: PathBuf,
    pub output: PathBuf,
    pub timeout_seconds: u64,
    pub repetitions: usize,
    pub keep_workspace: bool,
}

impl Options {
    pub fn for_repository(root: &Path) -> Self {
        Self {
            nia: root.join("target/release/nia"),
            resource_root: root.join("lib"),
            fixture: root.join("benchmarks/build/representative"),
            output: root.join("target/nia-build-baseline/baseline.json"),
            timeout_seconds: DEFAULT_TIMEOUT_SECONDS,
            repetitions: DEFAULT_REPETITIONS,
            keep_workspace: false,
        }
    }
}

#[derive(Debug, Clone, Serialize)]
pub struct ActionReport {
    pub schema_version: u32,
    pub kind: String,
    pub success: bool,
    pub counters: BTreeMap<String, i64>,
}

#[derive(Debug, Clone, Serialize)]
pub struct Measurement {
    pub process: Map<String, Value>,
    pub stages: BTreeMap<String, Map<String, Value>>,
    pub counters: Map<String, Value>,
}

#[derive(Debug, Clone, Serialize)]
pub struct BuildReports {
    pub actions: ActionReport,
    pub measurement: Measurement,
    pub outer_timing: Value,
}

#[derive(Debug, Clone, Serialize)]
pub struct BuildResult {
    pub name: String,
    pub command: Vec<String>,
    pub return_code: i32,
    pub wall_seconds_observed: f64,
    pub available_memory_bytes_before: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub corrupted_action_cache_entries: Option<usize>,
    #[serde(flatten)]
    pub reports: BuildReports,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(untagged)]
pub enum ExpectedValue {
    Exact(i64),
    Description(String),
}

#[derive(Debug, Clone, Serialize)]
pub struct AcceptanceCheck {
    pub state: String,
    pub counter: String,
    pub expected: ExpectedValue,
    pub found: i64,
    pub passed: bool,
}

#[derive(Debug, Clone, Serialize)]
pub struct AcceptanceReport {
    pub passed: bool,
    pub checks: Vec<AcceptanceCheck>,
}

#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(untagged)]
pub enum Number {
    Integer(i64),
    Float(f64),
}

impl Number {
    fn as_f64(&self) -> f64 {
        match self {
            Self::Integer(value) => *value as f64,
            Self::Float(value) => *value,
        }
    }
}

#[derive(Debug, Clone, Serialize)]
pub struct Distribution {
    pub median: Number,
    pub p95: Number,
    pub min: Number,
    pub max: Number,
}

#[derive(Debug, Clone, Serialize)]
pub struct StateSummary {
    pub name: String,
    pub sample_count: usize,
    pub wall_seconds_observed: Distribution,
    pub process: BTreeMap<String, Distribution>,
    pub stages: BTreeMap<String, Distribution>,
    pub counters: BTreeMap<String, Distribution>,
}

#[derive(Debug, Serialize)]
struct BuildRunSample<'a> {
    sample: usize,
    acceptance: AcceptanceReport,
    results: &'a [BuildResult],
}

#[derive(Debug, Serialize)]
struct AggregateAcceptance {
    passed: bool,
    samples: Vec<AcceptanceReport>,
}

#[derive(Debug, Serialize)]
struct BuildBaseline<'a> {
    schema_version: u32,
    kind: &'static str,
    machine: MachineMetadata,
    fixture: &'static str,
    runs: Vec<BuildRunSample<'a>>,
    acceptance: AggregateAcceptance,
    summary: Vec<StateSummary>,
}

fn require_memory_headroom() -> MaintainResult<Option<u64>> {
    let available = probe_host_resources().available_memory_bytes();
    if available.is_some_and(|value| value < MIN_AVAILABLE_MEMORY_BYTES) {
        return Err(format!(
            "build baseline refused to start under memory pressure: available={} required={MIN_AVAILABLE_MEMORY_BYTES}",
            available.unwrap_or_default()
        ));
    }
    Ok(available)
}

fn terminate_process_group(child: &mut std::process::Child) {
    if child.try_wait().ok().flatten().is_some() {
        return;
    }
    let group = format!("-{}", child.id());
    let _ = Command::new("kill")
        .args(["-TERM", "--", &group])
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status();
    let deadline = Instant::now() + Duration::from_secs(5);
    while Instant::now() < deadline {
        if child.try_wait().ok().flatten().is_some() {
            return;
        }
        thread::sleep(Duration::from_millis(25));
    }
    let _ = Command::new("kill")
        .args(["-KILL", "--", &group])
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status();
    let _ = child.wait();
}

struct BoundedOutput {
    status: ExitStatus,
    stdout: Vec<u8>,
    stderr: Vec<u8>,
    elapsed: f64,
    available_memory: Option<u64>,
}

fn read_pipe<R: Read + Send + 'static>(
    mut pipe: R,
) -> thread::JoinHandle<std::io::Result<Vec<u8>>> {
    thread::spawn(move || {
        let mut bytes = Vec::new();
        pipe.read_to_end(&mut bytes)?;
        Ok(bytes)
    })
}

fn join_pipe(
    handle: thread::JoinHandle<std::io::Result<Vec<u8>>>,
    name: &str,
) -> MaintainResult<Vec<u8>> {
    handle
        .join()
        .map_err(|_| format!("{name} reader thread panicked"))?
        .map_err(|error| format!("failed to read child {name}: {error}"))
}

fn run_bounded(
    command: &[String],
    cwd: &Path,
    timeout_seconds: u64,
) -> MaintainResult<BoundedOutput> {
    let available_memory = require_memory_headroom()?;
    let started = Instant::now();
    let mut process = Command::new(&command[0]);
    process
        .args(&command[1..])
        .current_dir(cwd)
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .process_group(0);
    let mut child = process
        .spawn()
        .map_err(|error| format!("failed to run {}: {error}", command.join(" ")))?;
    let stdout = read_pipe(child.stdout.take().expect("piped child stdout"));
    let stderr = read_pipe(child.stderr.take().expect("piped child stderr"));
    let deadline = started + Duration::from_secs(timeout_seconds);
    let status = loop {
        if let Some(status) = child
            .try_wait()
            .map_err(|error| format!("failed to wait for child process: {error}"))?
        {
            break status;
        }
        if Instant::now() >= deadline {
            terminate_process_group(&mut child);
            let _ = join_pipe(stdout, "stdout");
            let _ = join_pipe(stderr, "stderr");
            return Err(format!(
                "command timed out after {timeout_seconds}s: {}",
                command.join(" ")
            ));
        }
        thread::sleep(Duration::from_millis(25));
    };
    Ok(BoundedOutput {
        status,
        stdout: join_pipe(stdout, "stdout")?,
        stderr: join_pipe(stderr, "stderr")?,
        elapsed: started.elapsed().as_secs_f64(),
        available_memory,
    })
}

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

pub fn parse_build_reports(stderr: &str, succeeded: bool) -> MaintainResult<BuildReports> {
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

fn counter(result: &BuildResult, name: &str) -> MaintainResult<i64> {
    result
        .reports
        .measurement
        .counters
        .get(name)
        .map_or(Some(0), Value::as_i64)
        .ok_or_else(|| format!("counter {name:?} is not an integer"))
}

fn validate_workload(results: &[BuildResult]) -> MaintainResult<()> {
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

fn numeric(value: &Value, context: &str) -> MaintainResult<Number> {
    if let Some(value) = value.as_i64() {
        return Ok(Number::Integer(value));
    }
    let value = value
        .as_f64()
        .filter(|value| value.is_finite())
        .ok_or_else(|| format!("{context} is not numeric"))?;
    Ok(Number::Float(value))
}

fn distribution(mut values: Vec<Number>) -> MaintainResult<Distribution> {
    if values.is_empty() {
        return Err("cannot summarize an empty distribution".to_owned());
    }
    values.sort_by(|left, right| left.as_f64().total_cmp(&right.as_f64()));
    let middle = values.len() / 2;
    let median = if values.len().is_multiple_of(2) {
        Number::Float((values[middle - 1].as_f64() + values[middle].as_f64()) / 2.0)
    } else {
        values[middle].clone()
    };
    let p95_index = ((values.len() as f64 * 0.95).ceil() as usize)
        .saturating_sub(1)
        .min(values.len() - 1);
    Ok(Distribution {
        median,
        p95: values[p95_index].clone(),
        min: values[0].clone(),
        max: values.last().expect("nonempty values").clone(),
    })
}

pub fn summarize_runs(runs: &[Vec<BuildResult>]) -> MaintainResult<Vec<StateSummary>> {
    let first_run = runs
        .first()
        .ok_or_else(|| "cannot summarize zero runs".to_owned())?;
    let mut summaries = Vec::new();
    for (state_index, first) in first_run.iter().enumerate() {
        let samples = runs
            .iter()
            .map(|run| {
                run.get(state_index)
                    .ok_or_else(|| "build run has missing state".to_owned())
            })
            .collect::<MaintainResult<Vec<_>>>()?;
        let counter_names = samples
            .iter()
            .flat_map(|sample| sample.reports.measurement.counters.keys().cloned())
            .collect::<BTreeSet<_>>();
        let mut process = BTreeMap::new();
        for name in ["wall_seconds", "max_rss_bytes"] {
            if samples.iter().all(|sample| {
                sample
                    .reports
                    .measurement
                    .process
                    .get(name)
                    .is_some_and(|value| !value.is_null())
            }) {
                process.insert(
                    name.to_owned(),
                    distribution(
                        samples
                            .iter()
                            .map(|sample| {
                                numeric(
                                    &sample.reports.measurement.process[name],
                                    &format!("process metric {name}"),
                                )
                            })
                            .collect::<MaintainResult<Vec<_>>>()?,
                    )?,
                );
            }
        }
        let stages = BUILD_STAGE_NAMES
            .iter()
            .map(|name| {
                let values = samples
                    .iter()
                    .map(|sample| {
                        numeric(
                            &sample.reports.measurement.stages[*name]["total_seconds"],
                            &format!("stage metric {name}"),
                        )
                    })
                    .collect::<MaintainResult<Vec<_>>>()?;
                Ok(((*name).to_owned(), distribution(values)?))
            })
            .collect::<MaintainResult<BTreeMap<_, _>>>()?;
        let counters = counter_names
            .into_iter()
            .map(|name| {
                let values = samples
                    .iter()
                    .map(|sample| counter(sample, &name).map(Number::Integer))
                    .collect::<MaintainResult<Vec<_>>>()?;
                Ok((name, distribution(values)?))
            })
            .collect::<MaintainResult<BTreeMap<_, _>>>()?;
        summaries.push(StateSummary {
            name: first.name.clone(),
            sample_count: samples.len(),
            wall_seconds_observed: distribution(
                samples
                    .iter()
                    .map(|sample| Number::Float(sample.wall_seconds_observed))
                    .collect(),
            )?,
            process,
            stages,
            counters,
        });
    }
    Ok(summaries)
}

pub fn build_command(
    nia: &Path,
    resource_root: &Path,
    workspace: &Path,
    step: Option<&str>,
) -> Vec<String> {
    let mut command = vec![
        nia.to_string_lossy().into_owned(),
        "--resource-root".to_owned(),
        resource_root.to_string_lossy().into_owned(),
        "build".to_owned(),
        "--root".to_owned(),
        workspace.to_string_lossy().into_owned(),
        "--timings=detail".to_owned(),
        "--timings-format=json".to_owned(),
    ];
    if let Some(step) = step {
        command.insert(4, step.to_owned());
    }
    command
}

fn run_state(
    nia: &Path,
    resource_root: &Path,
    workspace: &Path,
    name: &str,
    timeout_seconds: u64,
    step: Option<&str>,
    expect_success: bool,
) -> MaintainResult<BuildResult> {
    let command = build_command(nia, resource_root, workspace, step);
    let output = run_bounded(&command, workspace, timeout_seconds)?;
    let succeeded = output.status.success();
    let stderr = String::from_utf8_lossy(&output.stderr);
    if succeeded != expect_success {
        eprint!("{stderr}");
        return Err(format!(
            "build state {name:?} returned {}; expected success={expect_success}",
            output.status
        ));
    }
    let reports = parse_build_reports(&stderr, succeeded)?;
    if reports.actions.success != expect_success {
        return Err(format!(
            "build state {name:?} action status disagrees with process status"
        ));
    }
    let _ = output.stdout;
    Ok(BuildResult {
        name: name.to_owned(),
        command: vec![
            "$NIA".to_owned(),
            "--resource-root".to_owned(),
            "$RESOURCE_ROOT".to_owned(),
        ]
        .into_iter()
        .chain(command[3..].iter().cloned())
        .collect(),
        return_code: output.status.code().unwrap_or(-1),
        wall_seconds_observed: output.elapsed,
        available_memory_bytes_before: output.available_memory,
        corrupted_action_cache_entries: None,
        reports,
    })
}

fn copy_tree(source: &Path, destination: &Path) -> MaintainResult<()> {
    fs::create_dir_all(destination)
        .map_err(|error| format!("failed to create {}: {error}", destination.display()))?;
    for entry in fs::read_dir(source)
        .map_err(|error| format!("failed to read {}: {error}", source.display()))?
    {
        let entry =
            entry.map_err(|error| format!("failed to inspect {}: {error}", source.display()))?;
        let target = destination.join(entry.file_name());
        let file_type = entry
            .file_type()
            .map_err(|error| format!("failed to inspect {}: {error}", entry.path().display()))?;
        if file_type.is_dir() {
            copy_tree(&entry.path(), &target)?;
        } else if file_type.is_file() {
            fs::copy(entry.path(), &target)
                .map_err(|error| format!("failed to copy {}: {error}", entry.path().display()))?;
        }
    }
    Ok(())
}

fn collect_action_entries(root: &Path, entries: &mut Vec<PathBuf>) -> MaintainResult<()> {
    if !root.is_dir() {
        return Ok(());
    }
    for entry in
        fs::read_dir(root).map_err(|error| format!("failed to read {}: {error}", root.display()))?
    {
        let entry =
            entry.map_err(|error| format!("failed to inspect {}: {error}", root.display()))?;
        let file_type = entry
            .file_type()
            .map_err(|error| format!("failed to inspect {}: {error}", entry.path().display()))?;
        if file_type.is_dir() {
            collect_action_entries(&entry.path(), entries)?;
        } else if file_type.is_file()
            && entry
                .path()
                .extension()
                .is_some_and(|extension| extension == "entry")
        {
            entries.push(entry.path());
        }
    }
    Ok(())
}

pub fn corrupt_action_cache(workspace: &Path) -> MaintainResult<usize> {
    let mut entries = Vec::new();
    collect_action_entries(&workspace.join(".nia-cache/actions"), &mut entries)?;
    entries.sort();
    if entries.is_empty() {
        return Err("build workload produced no action-cache entries to corrupt".to_owned());
    }
    for path in &entries {
        fs::write(path, b"nia build baseline injected corruption\n")
            .map_err(|error| format!("failed to corrupt {}: {error}", path.display()))?;
    }
    Ok(entries.len())
}

fn run_workload(
    nia: &Path,
    resource_root: &Path,
    fixture: &Path,
    timeout_seconds: u64,
) -> MaintainResult<(Vec<BuildResult>, TemporaryDirectory)> {
    let temporary = TemporaryDirectory::new("nia-build-baseline-")?;
    let workspace = temporary.path().join("representative");
    copy_tree(fixture, &workspace)?;
    let mut results = vec![run_state(
        nia,
        resource_root,
        &workspace,
        "clean",
        timeout_seconds,
        None,
        true,
    )?];
    results.push(run_state(
        nia,
        resource_root,
        &workspace,
        "warm",
        timeout_seconds,
        None,
        true,
    )?);
    fs::copy(
        workspace.join("src/main.edited.nia"),
        workspace.join("src/main.nia"),
    )
    .map_err(|error| format!("failed to edit build fixture source: {error}"))?;
    results.push(run_state(
        nia,
        resource_root,
        &workspace,
        "source_edit",
        timeout_seconds,
        None,
        true,
    )?);
    let build_script = fs::read_to_string(workspace.join("build.nia"))
        .map_err(|error| format!("failed to read build fixture: {error}"))?;
    fs::write(
        workspace.join("build.nia"),
        build_script.replace("deps/helper.nia", "deps/helper_edited.nia"),
    )
    .map_err(|error| format!("failed to edit build fixture module map: {error}"))?;
    results.push(run_state(
        nia,
        resource_root,
        &workspace,
        "module_map_edit",
        timeout_seconds,
        None,
        true,
    )?);
    let corrupted = corrupt_action_cache(&workspace)?;
    let mut corrupt = run_state(
        nia,
        resource_root,
        &workspace,
        "corrupt_cache",
        timeout_seconds,
        None,
        true,
    )?;
    corrupt.corrupted_action_cache_entries = Some(corrupted);
    results.push(corrupt);
    results.push(run_state(
        nia,
        resource_root,
        &workspace,
        "recovered_warm",
        timeout_seconds,
        None,
        true,
    )?);
    results.push(run_state(
        nia,
        resource_root,
        &workspace,
        "failed_action",
        timeout_seconds,
        Some("fail"),
        false,
    )?);
    validate_workload(&results)?;
    Ok((results, temporary))
}

pub fn run(options: &Options) -> MaintainResult<()> {
    let nia = options.nia.canonicalize().map_err(|error| {
        format!(
            "nia executable does not exist: {}: {error}",
            options.nia.display()
        )
    })?;
    let resource_root = options.resource_root.canonicalize().map_err(|error| {
        format!(
            "Nia resource root is invalid: {}: {error}",
            options.resource_root.display()
        )
    })?;
    let fixture = options.fixture.canonicalize().map_err(|error| {
        format!(
            "build fixture does not exist: {}: {error}",
            options.fixture.display()
        )
    })?;
    if !resource_root.join("toolchain.meta").is_file() {
        return Err(format!(
            "Nia resource root is invalid: {}",
            resource_root.display()
        ));
    }
    if !fixture.join("build.nia").is_file() {
        return Err(format!(
            "build fixture does not exist: {}",
            fixture.display()
        ));
    }
    if options.timeout_seconds == 0 {
        return Err("--timeout-seconds must be positive".to_owned());
    }
    if options.repetitions == 0 {
        return Err("--repetitions must be positive".to_owned());
    }
    let mut runs = Vec::new();
    let mut temporaries = Vec::new();
    for _ in 0..options.repetitions {
        let (results, temporary) =
            run_workload(&nia, &resource_root, &fixture, options.timeout_seconds)?;
        runs.push(results);
        temporaries.push(temporary);
    }
    let samples = runs
        .iter()
        .map(|run| workload_acceptance(run))
        .collect::<MaintainResult<Vec<_>>>()?;
    let baseline = BuildBaseline {
        schema_version: 3,
        kind: "nia-build-baseline",
        machine: machine_metadata(None),
        fixture: "benchmarks/build/representative",
        runs: runs
            .iter()
            .enumerate()
            .map(|(index, results)| {
                Ok(BuildRunSample {
                    sample: index + 1,
                    acceptance: workload_acceptance(results)?,
                    results,
                })
            })
            .collect::<MaintainResult<Vec<_>>>()?,
        acceptance: AggregateAcceptance {
            passed: samples.iter().all(|sample| sample.passed),
            samples,
        },
        summary: summarize_runs(&runs)?,
    };
    let acceptance_passed = baseline.acceptance.passed;
    let output = absolute_path(&options.output)?;
    if let Some(parent) = output.parent() {
        fs::create_dir_all(parent)
            .map_err(|error| format!("failed to create {}: {error}", parent.display()))?;
    }
    fs::write(
        &output,
        format!(
            "{}\n",
            serde_json::to_string_pretty(&baseline)
                .map_err(|error| format!("failed to encode build baseline: {error}"))?
        ),
    )
    .map_err(|error| format!("failed to write {}: {error}", output.display()))?;
    println!("{}", output.display());
    if options.keep_workspace {
        for temporary in temporaries {
            eprintln!("workspace: {}", temporary.persist().display());
        }
    }
    if acceptance_passed {
        Ok(())
    } else {
        Err(format!(
            "build baseline acceptance failed; evidence written to {}",
            output.display()
        ))
    }
}

#[cfg(test)]
mod tests {
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

    fn result(name: &str, counters: Map<String, Value>) -> BuildResult {
        BuildResult {
            name: name.to_owned(),
            command: Vec::new(),
            return_code: 0,
            wall_seconds_observed: 1.0,
            available_memory_bytes_before: None,
            corrupted_action_cache_entries: None,
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
        results[4].corrupted_action_cache_entries = Some(3);
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
        results[3]
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
        results[5]
            .reports
            .measurement
            .counters
            .insert("build.action_cache_hits".to_owned(), json!(2));
        results[6]
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
            check.state == "failed_action"
                && check.counter == "build.actions_executed"
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
                return_code: 0,
                wall_seconds_observed: (index + 1) as f64,
                available_memory_bytes_before: None,
                corrupted_action_cache_entries: None,
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
}
