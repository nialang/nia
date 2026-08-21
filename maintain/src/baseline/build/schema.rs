use std::collections::BTreeMap;

use serde::Serialize;
use serde_json::{Map, Value};

use crate::system::machine::MachineMetadata;

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
pub struct ArtifactComparison {
    pub name: String,
    pub source_modules: Vec<String>,
    pub matching: bool,
}

#[derive(Debug, Clone, Serialize)]
pub struct ArtifactEquivalence {
    pub clean_state: String,
    pub comparisons: Vec<ArtifactComparison>,
}

#[derive(Debug, Clone, Serialize)]
pub struct BuildResult {
    pub name: String,
    pub command: Vec<String>,
    pub process_id: u32,
    pub return_code: i32,
    pub wall_seconds_observed: f64,
    pub available_memory_bytes_before: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub corrupted_action_cache_entries: Option<usize>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub artifact_equivalence: Option<ArtifactEquivalence>,
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
    pub(super) fn as_f64(&self) -> f64 {
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
pub(super) struct BuildRunSample<'a> {
    pub(super) sample: usize,
    pub(super) acceptance: AcceptanceReport,
    pub(super) results: &'a [BuildResult],
}

#[derive(Debug, Serialize)]
pub(super) struct AggregateAcceptance {
    pub(super) passed: bool,
    pub(super) samples: Vec<AcceptanceReport>,
}

#[derive(Debug, Serialize)]
pub(super) struct BuildBaseline<'a> {
    pub(super) schema_version: u32,
    pub(super) kind: &'static str,
    pub(super) machine: MachineMetadata,
    pub(super) fixture: &'static str,
    pub(super) runs: Vec<BuildRunSample<'a>>,
    pub(super) acceptance: AggregateAcceptance,
    pub(super) summary: Vec<StateSummary>,
}
