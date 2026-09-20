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
    BuildReports, BuildResult, Distribution, ExpectedValue, InitialProductState, Measurement,
    Number, StateSummary,
};
use schema::{AggregateAcceptance, BuildBaseline, BuildBaselineConfiguration, BuildRunSample};
pub use summary::summarize_runs;
use workload::run_workload;
pub use workload::{build_command, corrupt_action_cache};

use crate::system::machine::machine_metadata;
use crate::system::toolchain::{compiler_identity, toolchain_identity};
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
    /// Runner-only build fixture copied for each workload.
    pub runner_fixture: PathBuf,
    /// Destination JSON report path.
    pub output: PathBuf,
    /// Per-state child-process timeout.
    pub timeout_seconds: u64,
    /// Number of independent workload repetitions.
    pub repetitions: usize,
    /// Whether to retain generated workload directories.
    pub keep_workspace: bool,
    /// Whether to build the repository-default compiler before measuring.
    pub build_compiler: bool,
}

impl Options {
    /// Creates options using repository-standard executable, fixture, and output paths.
    pub fn for_repository(root: &Path) -> Self {
        Self {
            nia: root.join("target/release/nia"),
            resource_root: root.join("lib"),
            fixture: root.join("benchmarks/build/representative"),
            runner_fixture: root.join("benchmarks/build/runner-only"),
            output: root.join("target/nia-build-baseline/baseline.json"),
            timeout_seconds: DEFAULT_TIMEOUT_SECONDS,
            repetitions: DEFAULT_REPETITIONS,
            keep_workspace: false,
            build_compiler: true,
        }
    }
}

/// Runs the representative build matrix and writes its report.
pub fn run(root: &Path, options: &Options) -> MaintainResult<()> {
    let requested_nia = absolute_path(&options.nia)?;
    let default_compiler = root.join("target/release/nia");
    let compiler_built_by_baseline = options.build_compiler && requested_nia == default_compiler;
    if compiler_built_by_baseline {
        let status = std::process::Command::new("cargo")
            .args(["build", "--release", "-p", "nia-cli"])
            .current_dir(root)
            .status()
            .map_err(|error| format!("failed to build compiler: {error}"))?;
        if !status.success() {
            return Err(format!("compiler build failed with {status}"));
        }
    }
    let nia = requested_nia.canonicalize().map_err(|error| {
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
    let runner_fixture = options.runner_fixture.canonicalize().map_err(|error| {
        format!(
            "runner-only build fixture does not exist: {}: {error}",
            options.runner_fixture.display()
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
    if !runner_fixture.join("build.nia").is_file() {
        return Err(format!(
            "runner-only build fixture does not exist: {}",
            runner_fixture.display()
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
        let (results, temporary) = run_workload(
            &nia,
            &resource_root,
            &fixture,
            &runner_fixture,
            options.timeout_seconds,
        )?;
        runs.push(results);
        temporaries.push(temporary);
    }
    let samples = runs
        .iter()
        .map(|run| workload_acceptance(run))
        .collect::<MaintainResult<Vec<_>>>()?;
    let baseline = BuildBaseline {
        kind: "nia-build-baseline",
        machine: machine_metadata(None),
        toolchain: toolchain_identity(root)?,
        compiler: compiler_identity(&nia)?,
        configuration: BuildBaselineConfiguration {
            compiler_built_by_baseline,
            compiler_cargo_profile: compiler_built_by_baseline.then_some("release"),
            compiler_cargo_features: Vec::new(),
            nia_profile: "debug",
            nia_optimization: "O0",
            nia_compilation_mode: "normal",
            timing_mode: "detail",
            project_cache_state: "fresh for clean states; inherited for transition states",
            os_page_cache_state: "uncontrolled; may be warm",
        },
        fixture: "benchmarks/build/representative",
        runner_fixture: "benchmarks/build/runner-only",
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
