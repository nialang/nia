use std::collections::BTreeMap;

use serde::Serialize;

use crate::system::machine::MachineMetadata;
use crate::system::toolchain::{ToolIdentity, ToolchainIdentity};

use super::{Language, Profile};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub(super) enum OutputKind {
    None,
    Metadata,
    Executable,
}

#[derive(Debug, Clone, Serialize)]
pub(super) struct ToolContract {
    pub(super) language: Language,
    pub(super) available: bool,
    pub(super) mode: &'static str,
    pub(super) output: OutputKind,
    pub(super) comparability: &'static str,
}

#[derive(Debug, Clone, Serialize)]
pub(super) struct WorkloadContract {
    pub(super) name: &'static str,
    pub(super) source_class: &'static str,
    pub(super) tools: Vec<ToolContract>,
}

#[derive(Debug, Clone, Serialize)]
pub(super) struct ProfileContract {
    pub(super) profile: Profile,
    pub(super) nia: &'static str,
    pub(super) rust: &'static str,
    pub(super) zig: &'static str,
}

#[derive(Debug, Serialize)]
pub(super) struct CompetitiveTools {
    pub(super) nia: ToolIdentity,
    pub(super) rustc: ToolIdentity,
    pub(super) cargo: ToolIdentity,
    pub(super) zig: ToolIdentity,
    pub(super) time: ToolIdentity,
}

#[derive(Debug, Serialize)]
pub(super) struct CompetitiveConfiguration {
    pub(super) compiler_built_by_baseline: bool,
    pub(super) compiler_cargo_profile: Option<&'static str>,
    pub(super) repetitions: usize,
    pub(super) profiles: Vec<ProfileContract>,
    pub(super) project_workspace_state: &'static str,
    pub(super) project_product_state: &'static str,
    pub(super) sdk_toolchain_cache_state: &'static str,
    pub(super) os_page_cache_state: &'static str,
}

#[derive(Debug, Clone, Serialize)]
pub(super) struct ProcessMetrics {
    pub(super) wall_seconds_observed: f64,
    pub(super) wall_seconds: f64,
    pub(super) user_seconds: f64,
    pub(super) system_seconds: f64,
    pub(super) cpu_utilization_percent: f64,
    pub(super) max_rss_bytes: u64,
}

#[derive(Debug, Clone, Serialize)]
pub(super) struct InitialState {
    pub(super) project_products_existed: bool,
    pub(super) output_existed: bool,
}

#[derive(Debug, Clone, Serialize)]
pub(super) struct Artifact {
    pub(super) kind: OutputKind,
    pub(super) size_bytes: u64,
    pub(super) blake3: String,
}

#[derive(Debug, Clone, Serialize)]
pub(super) struct ExecutionVerification {
    pub(super) return_code: i32,
    pub(super) stdout: String,
    pub(super) stderr: String,
    pub(super) expected_output: &'static str,
    pub(super) passed: bool,
}

#[derive(Debug, Clone, Serialize)]
pub(super) struct SampleAcceptance {
    pub(super) fresh_workspace: bool,
    pub(super) fresh_project_products: bool,
    pub(super) fresh_output: bool,
    pub(super) command_succeeded: bool,
    pub(super) output_contract_satisfied: bool,
    pub(super) executable_verified: Option<bool>,
    pub(super) passed: bool,
}

#[derive(Debug, Clone, Serialize)]
pub(super) struct CompetitiveSample {
    pub(super) sequence: usize,
    pub(super) repetition: usize,
    pub(super) profile: Profile,
    pub(super) workload: &'static str,
    pub(super) language: Language,
    pub(super) source: &'static str,
    pub(super) source_blake3: String,
    pub(super) command: Vec<String>,
    pub(super) process_id: u32,
    pub(super) return_code: i32,
    pub(super) stdout: String,
    pub(super) stderr: String,
    pub(super) metrics: ProcessMetrics,
    pub(super) initial_state: InitialState,
    pub(super) artifact: Option<Artifact>,
    pub(super) execution: Option<ExecutionVerification>,
    pub(super) acceptance: SampleAcceptance,
}

#[derive(Debug, Clone, Serialize)]
pub(super) struct Distribution {
    pub(super) median: f64,
    pub(super) p95: f64,
    pub(super) min: f64,
    pub(super) max: f64,
}

#[derive(Debug, Serialize)]
pub(super) struct CompetitiveSummary {
    pub(super) profile: Profile,
    pub(super) workload: &'static str,
    pub(super) language: Language,
    pub(super) sample_count: usize,
    pub(super) metrics: BTreeMap<&'static str, Distribution>,
    pub(super) artifact_size_bytes: Option<Distribution>,
}

#[derive(Debug, Serialize)]
pub(super) struct AggregateAcceptance {
    pub(super) passed: bool,
    pub(super) sample_count: usize,
    pub(super) failed_sequences: Vec<usize>,
}

#[derive(Debug, Serialize)]
pub(super) struct CompetitiveBaseline {
    pub(super) release_compatibility: u32,
    pub(super) schema_version: u32,
    pub(super) kind: &'static str,
    pub(super) experiment_id: String,
    pub(super) machine: MachineMetadata,
    pub(super) toolchain: ToolchainIdentity,
    pub(super) tools: CompetitiveTools,
    pub(super) configuration: CompetitiveConfiguration,
    pub(super) workloads: Vec<WorkloadContract>,
    pub(super) samples: Vec<CompetitiveSample>,
    pub(super) acceptance: AggregateAcceptance,
    pub(super) summary: Vec<CompetitiveSummary>,
}
