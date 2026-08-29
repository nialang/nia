use std::collections::BTreeMap;

use serde::Serialize;
use serde_json::{Map, Value};

use crate::system::machine::MachineMetadata;

#[derive(Debug, Clone, Serialize)]
/// Action-execution outcome emitted by the build coordinator.
pub struct ActionReport {
    /// Report schema version.
    pub schema_version: u32,
    /// Stable report kind discriminator.
    pub kind: String,
    /// Whether every selected action succeeded.
    pub success: bool,
    /// Build action counters keyed by stable names.
    pub counters: BTreeMap<String, i64>,
}

#[derive(Debug, Clone, Serialize)]
/// Process, stage, and counter measurements for one build state.
pub struct Measurement {
    /// Process-wide resource metrics.
    pub process: Map<String, Value>,
    /// Per-stage timing and resource metrics.
    pub stages: BTreeMap<String, Map<String, Value>>,
    /// Compiler and build counters.
    pub counters: Map<String, Value>,
}

#[derive(Debug, Clone, Serialize)]
/// Parsed reports associated with one measured build process.
pub struct BuildReports {
    /// Structured action outcome.
    pub actions: ActionReport,
    /// Outer build-process measurement.
    pub measurement: Measurement,
    /// Raw outer timing record retained for auditability.
    pub outer_timing: Value,
}

#[derive(Debug, Clone, Serialize)]
/// Byte-for-byte comparison of one incremental and clean artifact.
pub struct ArtifactComparison {
    /// Stable artifact name.
    pub name: String,
    /// Source modules contributing to the comparison.
    pub source_modules: Vec<String>,
    /// Whether incremental and clean bytes matched.
    pub matching: bool,
}

#[derive(Debug, Clone, Serialize)]
/// Clean-recomputation evidence for one incremental workload state.
pub struct ArtifactEquivalence {
    /// Independent clean state used as the reference.
    pub clean_state: String,
    /// Artifact comparisons performed against the clean state.
    pub comparisons: Vec<ArtifactComparison>,
}

#[derive(Debug, Clone, Serialize)]
/// Complete evidence captured for one ordered build workload state.
pub struct BuildResult {
    /// Stable workload-state name.
    pub name: String,
    /// Executed command with normalized baseline arguments.
    pub command: Vec<String>,
    /// Child process identity proving independent execution.
    pub process_id: u32,
    /// Child return code, or `-1` when unavailable.
    pub return_code: i32,
    /// Parent-observed wall time in seconds.
    pub wall_seconds_observed: f64,
    /// Available memory estimate immediately before execution.
    pub available_memory_bytes_before: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    /// Number of action-cache entries deliberately corrupted in this state.
    pub corrupted_action_cache_entries: Option<usize>,
    #[serde(skip_serializing_if = "Option::is_none")]
    /// Independent clean artifact comparison, when required by the state.
    pub artifact_equivalence: Option<ArtifactEquivalence>,
    #[serde(flatten)]
    /// Parsed action and measurement reports.
    pub reports: BuildReports,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(untagged)]
/// Exact or descriptive expectation recorded by an acceptance check.
pub enum ExpectedValue {
    /// Exact expected counter value.
    Exact(i64),
    /// Human-readable invariant when no single exact value applies.
    Description(String),
}

#[derive(Debug, Clone, Serialize)]
/// One counter or state invariant evaluated by baseline acceptance.
pub struct AcceptanceCheck {
    /// Workload state owning the check.
    pub state: String,
    /// Counter or invariant name.
    pub counter: String,
    /// Expected value or relationship.
    pub expected: ExpectedValue,
    /// Observed counter value.
    pub found: i64,
    /// Whether the check passed.
    pub passed: bool,
}

#[derive(Debug, Clone, Serialize)]
/// Acceptance result for one complete nine-state workload.
pub struct AcceptanceReport {
    /// Whether every contained check passed.
    pub passed: bool,
    /// Individual acceptance checks in evaluation order.
    pub checks: Vec<AcceptanceCheck>,
}

#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(untagged)]
/// Integer or finite floating-point sample retained without coercion.
pub enum Number {
    /// Integral sample.
    Integer(i64),
    /// Floating-point sample.
    Float(f64),
}

impl Number {
    pub(super) fn as_f64(&self) -> f64 {
        match self {
            Self::Integer(value) => *value as f64,
            Self::Float(value) => *value,
        }
    }
}

#[derive(Debug, Clone, Serialize)]
/// Median, p95, minimum, and maximum for one metric.
pub struct Distribution {
    /// Median sample value.
    pub median: Number,
    /// Nearest-rank 95th percentile.
    pub p95: Number,
    /// Minimum sample value.
    pub min: Number,
    /// Maximum sample value.
    pub max: Number,
}

#[derive(Debug, Clone, Serialize)]
/// Aggregate metrics for one aligned workload state.
pub struct StateSummary {
    /// Stable workload-state name.
    pub name: String,
    /// Number of independent samples.
    pub sample_count: usize,
    /// Distribution of parent-observed wall time.
    pub wall_seconds_observed: Distribution,
    /// Process metric distributions keyed by metric name.
    pub process: BTreeMap<String, Distribution>,
    /// Flattened stage metric distributions keyed by stage and metric.
    pub stages: BTreeMap<String, Distribution>,
    /// Counter distributions keyed by stable counter name.
    pub counters: BTreeMap<String, Distribution>,
}

#[derive(Debug, Serialize)]
/// One repetition and its raw state evidence.
pub(super) struct BuildRunSample<'a> {
    /// One-based repetition number.
    pub(super) sample: usize,
    /// Acceptance result for this repetition.
    pub(super) acceptance: AcceptanceReport,
    /// Ordered raw workload states.
    pub(super) results: &'a [BuildResult],
}

#[derive(Debug, Serialize)]
/// Acceptance results aggregated across all repetitions.
pub(super) struct AggregateAcceptance {
    /// Whether every repetition passed.
    pub(super) passed: bool,
    /// Per-repetition acceptance reports.
    pub(super) samples: Vec<AcceptanceReport>,
}

#[derive(Debug, Serialize)]
/// Schema-v5 representative build baseline report.
pub(super) struct BuildBaseline<'a> {
    /// Baseline schema version.
    pub(super) schema_version: u32,
    /// Stable report kind discriminator.
    pub(super) kind: &'static str,
    /// Machine and resource identity for the run.
    pub(super) machine: MachineMetadata,
    /// Repository-relative fixture identity.
    pub(super) fixture: &'static str,
    /// Independent raw workload repetitions.
    pub(super) runs: Vec<BuildRunSample<'a>>,
    /// Acceptance aggregate across repetitions.
    pub(super) acceptance: AggregateAcceptance,
    /// Per-state metric distributions.
    pub(super) summary: Vec<StateSummary>,
}
