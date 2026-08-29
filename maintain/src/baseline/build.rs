mod acceptance;
mod process;
mod reports;
mod schema;
mod summary;
mod workload;

use std::fs;
use std::path::{Path, PathBuf};

pub use acceptance::workload_acceptance;
pub use reports::parse_build_reports;
pub use schema::{
    AcceptanceCheck, AcceptanceReport, ActionReport, ArtifactComparison, ArtifactEquivalence,
    BuildReports, BuildResult, Distribution, ExpectedValue, Measurement, Number, StateSummary,
};
use schema::{AggregateAcceptance, BuildBaseline, BuildRunSample};
pub use summary::summarize_runs;
use workload::run_workload;
pub use workload::{build_command, corrupt_action_cache};

use crate::system::machine::machine_metadata;
use crate::{MaintainResult, absolute_path};

const DEFAULT_TIMEOUT_SECONDS: u64 = 420;
const DEFAULT_REPETITIONS: usize = 3;

#[derive(Debug, Clone)]
/// Inputs controlling representative build-baseline collection.
pub struct Options {
    /// Nia compiler executable used by every workload state.
    pub nia: PathBuf,
    /// Compiler resource root containing the standard library and toolchain metadata.
    pub resource_root: PathBuf,
    /// Representative build fixture copied for each workload.
    pub fixture: PathBuf,
    /// Destination JSON report path.
    pub output: PathBuf,
    /// Per-state child-process timeout.
    pub timeout_seconds: u64,
    /// Number of independent workload repetitions.
    pub repetitions: usize,
    /// Whether to retain generated workload directories.
    pub keep_workspace: bool,
}

impl Options {
    /// Creates options using repository-standard executable, fixture, and output paths.
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

/// Runs the representative build matrix and writes its schema-v5 report.
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
        schema_version: 5,
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

    // Evidence is authoritative even when acceptance fails. Persist it before
    // returning the nonzero result so CI can always publish the failed sample.
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
mod tests;
