mod process;
mod schema;
mod summary;
mod workload;

use std::collections::{BTreeMap, BTreeSet};
use std::ffi::OsStr;
use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::time::{SystemTime, UNIX_EPOCH};

use serde::Serialize;

use self::process::measure;
use self::schema::{
    AggregateAcceptance, Artifact, ArtifactRelation, CompetitiveBaseline, CompetitiveConfiguration,
    CompetitiveSample, CompetitiveTools, ExecutionVerification, InitialState, OutputKind,
    ProfileContract, ProjectProductState, SampleAcceptance, SampleState, SourceManifest,
    TransitionEvidence,
};
use self::summary::summarize;
use self::workload::{Programs, Workload};
pub use super::Language;
use super::synthetic;
use crate::system::machine::machine_metadata;
use crate::system::process::run_bounded;
use crate::system::toolchain::{external_tool_identity, toolchain_identity};
use crate::{MaintainResult, TemporaryDirectory, absolute_path};

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize)]
#[serde(rename_all = "snake_case")]
/// Optimization/profile contract used for all three compilers.
pub enum Profile {
    Development,
    Release,
}

impl Profile {
    const ALL: [Self; 2] = [Self::Development, Self::Release];

    /// Parses a CLI profile name.
    pub fn parse(value: &str) -> MaintainResult<Self> {
        match value {
            "development" | "dev" | "debug" => Ok(Self::Development),
            "release" => Ok(Self::Release),
            _ => Err(format!(
                "unknown competitive profile {value:?}; expected development or release"
            )),
        }
    }

    const fn nia_profile(self) -> &'static str {
        match self {
            Self::Development => "debug",
            Self::Release => "release",
        }
    }

    const fn nia_optimization(self) -> &'static str {
        match self {
            Self::Development => "-O0",
            Self::Release => "-O2",
        }
    }

    const fn rust_optimization(self) -> &'static str {
        match self {
            Self::Development => "opt-level=0",
            Self::Release => "opt-level=2",
        }
    }

    const fn zig_optimization(self) -> &'static str {
        match self {
            Self::Development => "Debug",
            Self::Release => "ReleaseSafe",
        }
    }

    fn contract(self) -> ProfileContract {
        match self {
            Self::Development => ProfileContract {
                profile: self,
                nia: "--profile debug -O0",
                rust: "direct rustc -C opt-level=0 -C debuginfo=0; Cargo dev with debug=0",
                zig: "direct -O Debug -fno-incremental; zig build -Doptimize=Debug",
            },
            Self::Release => ProfileContract {
                profile: self,
                nia: "--profile release -O2",
                rust: "direct rustc -C opt-level=2 -C debuginfo=0; Cargo release with opt-level=2 and debug=0",
                zig: "direct -O ReleaseSafe -fno-incremental; zig build -Doptimize=ReleaseSafe",
            },
        }
    }
}

#[derive(Debug, Clone)]
/// Inputs controlling cross-toolchain baseline collection.
pub struct Options {
    pub nia: PathBuf,
    pub rustc: PathBuf,
    pub cargo: PathBuf,
    pub zig: PathBuf,
    pub time: PathBuf,
    pub resource_root: PathBuf,
    pub output: Option<PathBuf>,
    pub repeat: usize,
    pub timeout_seconds: u64,
    pub build_compiler: bool,
    pub profiles: Vec<Profile>,
    pub workloads: Vec<String>,
}

impl Options {
    /// Creates options for the repository compiler and the active host tools.
    pub fn for_repository(root: &Path) -> Self {
        Self {
            nia: root.join("target/release/nia"),
            rustc: PathBuf::from("rustc"),
            cargo: PathBuf::from("cargo"),
            zig: PathBuf::from("zig"),
            time: PathBuf::from("/usr/bin/time"),
            resource_root: root.join("lib"),
            output: None,
            repeat: 5,
            timeout_seconds: 120,
            build_compiler: true,
            profiles: Vec::new(),
            workloads: Vec::new(),
        }
    }
}

fn build_compiler(root: &Path, compiler: &Path, enabled: bool) -> MaintainResult<bool> {
    let default = root.join("target/release/nia");
    if !enabled || compiler != default {
        return Ok(false);
    }
    let status = Command::new("cargo")
        .args(["build", "--release", "-p", "nia-cli"])
        .current_dir(root)
        .status()
        .map_err(|error| format!("failed to build compiler: {error}"))?;
    if !status.success() {
        return Err(format!("compiler build failed with {status}"));
    }
    Ok(true)
}

fn resolve_tool(
    program: &Path,
    version_arguments: &[&str],
) -> MaintainResult<(PathBuf, crate::system::toolchain::ToolIdentity)> {
    let identity = external_tool_identity(program, version_arguments)?;
    Ok((PathBuf::from(&identity.path), identity))
}

fn artifact(path: &Path, kind: OutputKind) -> MaintainResult<Option<Artifact>> {
    if !path.is_file() {
        return Ok(None);
    }
    let bytes = fs::read(path)
        .map_err(|error| format!("failed to read artifact {}: {error}", path.display()))?;
    Ok(Some(Artifact {
        kind,
        size_bytes: bytes.len() as u64,
        blake3: blake3::hash(&bytes).to_hex().to_string(),
    }))
}

fn copy_tree(source: &Path, destination: &Path) -> MaintainResult<()> {
    fs::create_dir_all(destination)
        .map_err(|error| format!("failed to create {}: {error}", destination.display()))?;
    let mut entries = fs::read_dir(source)
        .map_err(|error| format!("failed to read {}: {error}", source.display()))?
        .collect::<Result<Vec<_>, _>>()
        .map_err(|error| format!("failed to inspect {}: {error}", source.display()))?;
    entries.sort_by_key(std::fs::DirEntry::file_name);
    for entry in entries {
        if is_project_product_directory(&entry.file_name()) {
            continue;
        }
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

fn is_project_product_directory(name: &OsStr) -> bool {
    matches!(
        name.to_str(),
        Some(".nia-build" | ".nia-cache" | ".zig-cache" | "zig-out" | "target")
    )
}

fn collect_tree_files(
    root: &Path,
    directory: &Path,
    files: &mut Vec<PathBuf>,
) -> MaintainResult<()> {
    for entry in fs::read_dir(directory)
        .map_err(|error| format!("failed to read {}: {error}", directory.display()))?
    {
        let entry =
            entry.map_err(|error| format!("failed to inspect {}: {error}", directory.display()))?;
        let file_type = entry
            .file_type()
            .map_err(|error| format!("failed to inspect {}: {error}", entry.path().display()))?;
        if file_type.is_dir() {
            if is_project_product_directory(&entry.file_name()) {
                continue;
            }
            collect_tree_files(root, &entry.path(), files)?;
        } else if file_type.is_file() {
            files.push(
                entry
                    .path()
                    .strip_prefix(root)
                    .expect("collected path is below root")
                    .to_path_buf(),
            );
        }
    }
    Ok(())
}

struct SourceSnapshot {
    manifest: SourceManifest,
    files: BTreeMap<PathBuf, String>,
}

fn source_snapshot(path: &Path, descriptor: &'static str) -> MaintainResult<SourceSnapshot> {
    if path.is_file() {
        let bytes = fs::read(path)
            .map_err(|error| format!("failed to read fixture {}: {error}", path.display()))?;
        let blake3 = blake3::hash(&bytes).to_hex().to_string();
        return Ok(SourceSnapshot {
            manifest: SourceManifest {
                descriptor,
                blake3: blake3.clone(),
                file_count: 1,
                size_bytes: bytes.len() as u64,
            },
            files: BTreeMap::from([(PathBuf::from("."), blake3)]),
        });
    }
    let mut files = Vec::new();
    collect_tree_files(path, path, &mut files)?;
    files.sort();
    let file_count = files.len();
    let mut hasher = blake3::Hasher::new();
    let mut size_bytes = 0u64;
    let mut file_hashes = BTreeMap::new();
    for relative in files {
        let bytes = fs::read(path.join(&relative)).map_err(|error| {
            format!(
                "failed to read fixture {}: {error}",
                path.join(&relative).display()
            )
        })?;
        hasher.update(relative.to_string_lossy().as_bytes());
        hasher.update(&[0]);
        hasher.update(&(bytes.len() as u64).to_le_bytes());
        hasher.update(&bytes);
        size_bytes += bytes.len() as u64;
        file_hashes.insert(relative, blake3::hash(&bytes).to_hex().to_string());
    }
    Ok(SourceSnapshot {
        manifest: SourceManifest {
            descriptor,
            blake3: hasher.finalize().to_hex().to_string(),
            file_count,
            size_bytes,
        },
        files: file_hashes,
    })
}

fn changed_files(before: &SourceSnapshot, after: &SourceSnapshot) -> Vec<String> {
    before
        .files
        .keys()
        .chain(after.files.keys())
        .collect::<BTreeSet<_>>()
        .into_iter()
        .filter(|path| before.files.get(*path) != after.files.get(*path))
        .map(|path| path.to_string_lossy().replace('\\', "/"))
        .collect()
}

fn verify_executable(
    path: &Path,
    expected_output: &'static str,
    timeout_seconds: u64,
) -> ExecutionVerification {
    let command = [path.to_string_lossy().into_owned()];
    match run_bounded(
        &command,
        path.parent().unwrap_or_else(|| Path::new(".")),
        timeout_seconds,
    ) {
        Ok(output) => {
            let stdout = String::from_utf8_lossy(&output.stdout).trim().to_owned();
            let stderr = String::from_utf8_lossy(&output.stderr).trim().to_owned();
            let actual = format!("{stdout}\n{stderr}").trim().to_owned();
            ExecutionVerification {
                return_code: output.status.code().unwrap_or(-1),
                passed: output.status.success() && actual == expected_output,
                stdout,
                stderr,
                expected_output,
            }
        }
        Err(error) => ExecutionVerification {
            return_code: -1,
            stdout: String::new(),
            stderr: error,
            expected_output,
            passed: false,
        },
    }
}

#[derive(Clone, Copy)]
struct SampleInputs<'a> {
    root: &'a Path,
    programs: &'a Programs<'a>,
    time: &'a Path,
    workload: Workload,
    language: Language,
    profile: Profile,
    repetition: usize,
    sequence: usize,
    total_samples: usize,
    timeout_seconds: u64,
}

fn sample_acceptance(
    state: SampleState,
    initial_state: &InitialState,
    transition: &TransitionEvidence,
    command_succeeded: bool,
    output_contract_satisfied: bool,
    executable_verified: Option<bool>,
) -> SampleAcceptance {
    let expected_products = !matches!(state, SampleState::Clean);
    let expected_output = !matches!(state, SampleState::Clean);
    let initial_state_satisfied = initial_state
        .project_products
        .iter()
        .all(|product| product.existed == expected_products)
        && initial_state.output_existed == expected_output;
    SampleAcceptance {
        expected_project_products_existed: expected_products,
        expected_output_existed: expected_output,
        initial_state_satisfied,
        process_transition_satisfied: transition.process_satisfied,
        source_transition_satisfied: transition.source_satisfied,
        artifact_transition_satisfied: transition.artifact_satisfied,
        command_succeeded,
        output_contract_satisfied,
        executable_verified,
        passed: initial_state_satisfied
            && transition.process_satisfied
            && transition.source_satisfied
            && transition.artifact_satisfied
            && command_succeeded
            && output_contract_satisfied
            && executable_verified.unwrap_or(true),
    }
}

struct PreparedProject {
    _temporary: TemporaryDirectory,
    workspace: PathBuf,
    source: PathBuf,
    manifest_root: PathBuf,
    measurements: PathBuf,
}

fn prepare_project(inputs: &SampleInputs<'_>) -> MaintainResult<PreparedProject> {
    let temporary = TemporaryDirectory::new("nia-competitive-")?;
    let workspace = temporary.path().join("project");
    fs::create_dir_all(&workspace)
        .map_err(|error| format!("failed to create {}: {error}", workspace.display()))?;
    let source_fixture = inputs
        .workload
        .source_relative(inputs.language)
        .map(|relative| inputs.root.join(relative));
    let (source, manifest_root) = if inputs.workload.is_synthetic_build() {
        synthetic::generate_build_project(
            &workspace,
            inputs.language,
            inputs
                .workload
                .synthetic_specification()
                .expect("synthetic build has a specification"),
        )?;
        (workspace.clone(), workspace.clone())
    } else if let Some(specification) = inputs.workload.synthetic_specification() {
        let source = synthetic::generate(&workspace, inputs.language, specification)?;
        (source, workspace.clone())
    } else if inputs.workload.is_build() {
        let source_fixture = source_fixture
            .as_ref()
            .expect("build workload has a fixture");
        copy_tree(source_fixture, &workspace)?;
        (workspace.clone(), workspace.clone())
    } else {
        let source_fixture = source_fixture
            .as_ref()
            .expect("direct workload has a fixture");
        let source = workspace.join(format!("main.{}", inputs.language.extension()));
        fs::copy(source_fixture, &source).map_err(|error| {
            format!(
                "failed to copy fixture {} to {}: {error}",
                source_fixture.display(),
                source.display()
            )
        })?;
        (source.clone(), source)
    };
    let measurements = temporary.path().join("measurements");
    fs::create_dir_all(&measurements)
        .map_err(|error| format!("failed to create {}: {error}", measurements.display()))?;
    Ok(PreparedProject {
        _temporary: temporary,
        workspace,
        source,
        manifest_root,
        measurements,
    })
}

struct Predecessor<'a> {
    sequence: usize,
    process_id: u32,
    source: &'a SourceSnapshot,
    artifact: Option<&'a Artifact>,
}

fn transition_evidence(
    state: SampleState,
    predecessor: Option<&Predecessor<'_>>,
    process_id: u32,
    source: &SourceSnapshot,
    artifact: Option<&Artifact>,
    edited_leaf: Option<&str>,
) -> TransitionEvidence {
    let predecessor_process_id = predecessor.map(|previous| previous.process_id);
    let process_satisfied = match state {
        SampleState::Clean => predecessor_process_id.is_none(),
        SampleState::NoOpWarm | SampleState::LeafEdit => {
            predecessor_process_id.is_some_and(|predecessor| predecessor != process_id)
        }
    };
    let changed_files = predecessor
        .map(|previous| changed_files(previous.source, source))
        .unwrap_or_default();
    let expected_changed_files = match state {
        SampleState::Clean | SampleState::NoOpWarm => Vec::new(),
        SampleState::LeafEdit => vec![edited_leaf.expect("leaf edit path is known").to_owned()],
    };
    let source_satisfied = changed_files == expected_changed_files;
    let expected_artifact_relation = match state {
        SampleState::Clean => ArtifactRelation::NotApplicable,
        SampleState::NoOpWarm => ArtifactRelation::Identical,
        SampleState::LeafEdit => ArtifactRelation::Different,
    };
    let artifact_satisfied = match expected_artifact_relation {
        ArtifactRelation::NotApplicable => true,
        ArtifactRelation::Identical => predecessor
            .and_then(|previous| previous.artifact)
            .zip(artifact)
            .is_some_and(|(before, after)| before.blake3 == after.blake3),
        ArtifactRelation::Different => predecessor
            .and_then(|previous| previous.artifact)
            .zip(artifact)
            .is_some_and(|(before, after)| before.blake3 != after.blake3),
    };
    TransitionEvidence {
        predecessor_sequence: predecessor.map(|previous| previous.sequence),
        predecessor_process_id,
        process_satisfied,
        expected_changed_files,
        changed_files,
        source_satisfied,
        expected_artifact_relation,
        artifact_satisfied,
    }
}

fn collect_prepared_sample(
    inputs: &SampleInputs<'_>,
    prepared: &PreparedProject,
    state: SampleState,
    predecessor: Option<Predecessor<'_>>,
    edited_leaf: Option<&str>,
) -> MaintainResult<(CompetitiveSample, SourceSnapshot)> {
    let source_descriptor = inputs.workload.source_descriptor(inputs.language);
    let source_snapshot = source_snapshot(&prepared.manifest_root, source_descriptor)?;
    let product_paths =
        workload::project_product_paths(&prepared.workspace, inputs.workload, inputs.language);
    let cache = product_paths[0].clone();
    let output_kind = inputs.workload.output_kind(inputs.language);
    let output = workload::output_path(
        &prepared.workspace,
        inputs.workload,
        inputs.language,
        inputs.profile,
        output_kind,
    );
    let initial_state = InitialState {
        project_products: product_paths
            .iter()
            .map(|path| ProjectProductState {
                path: path
                    .strip_prefix(&prepared.workspace)
                    .expect("product path is below workspace")
                    .to_string_lossy()
                    .replace('\\', "/"),
                existed: path.exists(),
            })
            .collect(),
        output_existed: output.exists(),
    };
    let command = workload::command(
        inputs.programs,
        inputs.workload,
        inputs.language,
        inputs.profile,
        &prepared.source,
        &output,
        &cache,
    );
    let command_label = workload::normalized_command(&command, inputs.root, &prepared.workspace);
    let measured = measure(
        inputs.time,
        &command,
        &prepared.workspace,
        &prepared
            .measurements
            .join(format!("{}.txt", inputs.sequence)),
        inputs.timeout_seconds,
    )?;
    let command_succeeded = measured.return_code == 0;
    let artifact = artifact(&output, output_kind)?;
    let output_contract_satisfied = match output_kind {
        OutputKind::None => artifact.is_none(),
        OutputKind::Metadata | OutputKind::Executable => artifact.is_some(),
    };
    let execution = inputs
        .workload
        .expected_output(inputs.language, state)
        .filter(|_| artifact.is_some())
        .map(|expected| verify_executable(&output, expected, inputs.timeout_seconds));
    let executable_verified = inputs
        .workload
        .expected_output(inputs.language, state)
        .map(|_| execution.as_ref().is_some_and(|execution| execution.passed));
    let transition = transition_evidence(
        state,
        predecessor.as_ref(),
        measured.process_id,
        &source_snapshot,
        artifact.as_ref(),
        edited_leaf,
    );
    let acceptance = sample_acceptance(
        state,
        &initial_state,
        &transition,
        command_succeeded,
        output_contract_satisfied,
        executable_verified,
    );
    let sample = CompetitiveSample {
        sequence: inputs.sequence,
        repetition: inputs.repetition,
        profile: inputs.profile,
        workload: inputs.workload.name(),
        language: inputs.language,
        state,
        source: source_snapshot.manifest.clone(),
        command: command_label,
        process_id: measured.process_id,
        return_code: measured.return_code,
        stdout: String::from_utf8_lossy(&measured.stdout).into_owned(),
        stderr: String::from_utf8_lossy(&measured.stderr).into_owned(),
        metrics: measured.metrics,
        initial_state,
        transition,
        artifact,
        execution,
        acceptance,
    };
    Ok((sample, source_snapshot))
}

fn collect_samples(inputs: &SampleInputs<'_>) -> MaintainResult<Vec<CompetitiveSample>> {
    let prepared = prepare_project(inputs)?;
    let mut samples = Vec::<CompetitiveSample>::new();
    let mut predecessor_source = None;
    let mut predecessor_artifact = None;
    let mut predecessor_sequence = None;
    let mut edited_leaf = None;
    for (offset, state) in inputs.workload.states().iter().copied().enumerate() {
        if matches!(state, SampleState::LeafEdit) {
            let relative = synthetic::edit_leaf(
                &prepared.workspace,
                inputs.language,
                inputs
                    .workload
                    .synthetic_specification()
                    .expect("leaf edit workload has a specification")
                    .module_count
                    / 2,
            )?;
            edited_leaf = Some(relative.to_string_lossy().replace('\\', "/"));
        }
        let sequence = inputs.sequence + offset;
        eprintln!(
            "competitive: {sequence}/{} repetition={} profile={:?} workload={} language={:?} state={state:?}",
            inputs.total_samples,
            inputs.repetition,
            inputs.profile,
            inputs.workload.name(),
            inputs.language,
        );
        let state_inputs = SampleInputs {
            sequence,
            ..*inputs
        };
        let predecessor = predecessor_source.as_ref().map(|source| Predecessor {
            sequence: predecessor_sequence.expect("predecessor sequence exists"),
            process_id: samples
                .last()
                .expect("predecessor sample exists")
                .process_id,
            source,
            artifact: predecessor_artifact.as_ref(),
        });
        let (sample, source) = collect_prepared_sample(
            &state_inputs,
            &prepared,
            state,
            predecessor,
            edited_leaf.as_deref(),
        )?;
        predecessor_sequence = Some(sample.sequence);
        predecessor_artifact = sample.artifact.clone();
        predecessor_source = Some(source);
        samples.push(sample);
    }
    Ok(samples)
}

fn selected_profiles(options: &Options) -> MaintainResult<Vec<Profile>> {
    let profiles = if options.profiles.is_empty() {
        Profile::ALL.to_vec()
    } else {
        options.profiles.clone()
    };
    let unique = profiles.iter().copied().collect::<BTreeSet<_>>();
    if unique.len() != profiles.len() {
        return Err("competitive profiles must not be repeated".to_owned());
    }
    Ok(profiles)
}

fn selected_workloads(options: &Options) -> MaintainResult<Vec<Workload>> {
    if options.workloads.is_empty() {
        return Ok(Workload::ALL.to_vec());
    }
    let mut selected = Vec::new();
    for name in &options.workloads {
        let workload = Workload::parse(name).ok_or_else(|| {
            format!(
                "unknown competitive workload {name:?}; expected one of: {}",
                Workload::ALL.map(Workload::name).join(", ")
            )
        })?;
        if selected.contains(&workload) {
            return Err(format!("competitive workload {name:?} was repeated"));
        }
        selected.push(workload);
    }
    Ok(selected)
}

fn write_report(path: &Path, report: &CompetitiveBaseline) -> MaintainResult<()> {
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)
            .map_err(|error| format!("failed to create {}: {error}", parent.display()))?;
    }
    let temporary = PathBuf::from(format!("{}.tmp", path.to_string_lossy()));
    let bytes = serde_json::to_vec_pretty(report)
        .map_err(|error| format!("failed to encode competitive baseline: {error}"))?;
    fs::write(&temporary, [bytes.as_slice(), b"\n"].concat())
        .map_err(|error| format!("failed to write {}: {error}", temporary.display()))?;
    fs::rename(&temporary, path)
        .map_err(|error| format!("failed to publish {}: {error}", path.display()))
}

/// Runs the fixed Nia/rustc/Zig competitive matrix and writes raw JSON evidence.
pub fn run(root: &Path, options: &Options) -> MaintainResult<()> {
    if options.repeat == 0 {
        return Err("--repeat must be at least 1".to_owned());
    }
    if options.timeout_seconds == 0 {
        return Err("--timeout-seconds must be at least 1".to_owned());
    }
    let profiles = selected_profiles(options)?;
    let workloads = selected_workloads(options)?;
    let nia_input = absolute_path(&options.nia)?;
    let built = build_compiler(root, &nia_input, options.build_compiler)?;
    let resource_root = options.resource_root.canonicalize().map_err(|error| {
        format!(
            "resource root is invalid: {}: {error}",
            options.resource_root.display()
        )
    })?;
    if !resource_root.join("toolchain.meta").is_file() {
        return Err(format!(
            "resource root is invalid: {}",
            resource_root.display()
        ));
    }
    let (nia, nia_identity) = resolve_tool(&nia_input, &["--version"])?;
    let (rustc, rustc_identity) = resolve_tool(&options.rustc, &["--version", "--verbose"])?;
    let (cargo, cargo_identity) = resolve_tool(&options.cargo, &["--version"])?;
    let (zig, zig_identity) = resolve_tool(&options.zig, &["version"])?;
    let (time, time_identity) = resolve_tool(&options.time, &["--version"])?;
    let programs = Programs {
        nia: &nia,
        rustc: &rustc,
        cargo: &cargo,
        zig: &zig,
        resource_root: &resource_root,
    };
    let toolchain = toolchain_identity(root)?;
    let started = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_err(|error| format!("system clock precedes Unix epoch: {error}"))?
        .as_secs();
    let revision = &toolchain.source.revision;
    let short_revision = &revision[..revision.len().min(12)];
    let experiment_id = format!(
        "competitive-{short_revision}-{started}-{}",
        std::process::id()
    );

    let mut samples = Vec::new();
    let samples_per_repetition = profiles.len()
        * workloads
            .iter()
            .map(|workload| workload.languages().len() * workload.states().len())
            .sum::<usize>();
    let total_samples = options.repeat * samples_per_repetition;
    for repetition in 1..=options.repeat {
        for profile in &profiles {
            for workload in &workloads {
                let languages = workload.languages();
                let rotation = (repetition - 1) % languages.len();
                for offset in 0..languages.len() {
                    let language = languages[(rotation + offset) % languages.len()];
                    let sequence = samples.len() + 1;
                    samples.extend(collect_samples(&SampleInputs {
                        root,
                        programs: &programs,
                        time: &time,
                        workload: *workload,
                        language,
                        profile: *profile,
                        repetition,
                        sequence,
                        total_samples,
                        timeout_seconds: options.timeout_seconds,
                    })?);
                }
            }
        }
    }
    let failed_sequences = samples
        .iter()
        .filter(|sample| !sample.acceptance.passed)
        .map(|sample| sample.sequence)
        .collect::<Vec<_>>();
    let acceptance = AggregateAcceptance {
        passed: failed_sequences.is_empty(),
        sample_count: samples.len(),
        failed_sequences,
    };
    let report = CompetitiveBaseline {
        kind: "competitive_toolchain_baseline",
        experiment_id: experiment_id.clone(),
        machine: machine_metadata(None),
        toolchain,
        tools: CompetitiveTools {
            nia: nia_identity,
            rustc: rustc_identity,
            cargo: cargo_identity,
            zig: zig_identity,
            time: time_identity,
        },
        configuration: CompetitiveConfiguration {
            compiler_built_by_baseline: built,
            compiler_cargo_profile: built.then_some("release"),
            repetitions: options.repeat,
            profiles: profiles.iter().map(|profile| profile.contract()).collect(),
            project_workspace_state: "fresh temporary workspace for every clean sample or ordered state sequence; no-op warm and leaf-edit states share only their sequence workspace",
            project_product_state: "absent Nia build/cache, Cargo target, and Zig local cache/output roots before clean build processes; retained for ordered no-op warm and leaf-edit states; fresh explicit Nia/Zig project cache per direct process; direct rustc incremental compilation disabled by omission",
            sdk_toolchain_cache_state: "selected Nia resource root, rustc sysroot, and Zig global cache retained; SDK/toolchain-cold is a separate experiment",
            os_page_cache_state: "uncontrolled; may be warm and shared across interleaved tools",
        },
        workloads: workloads
            .iter()
            .map(|workload| workload.contract())
            .collect(),
        summary: summarize(&samples),
        acceptance,
        samples,
    };
    let output = options
        .output
        .as_ref()
        .map(|path| absolute_path(path))
        .transpose()?
        .unwrap_or_else(|| {
            root.join("target/nia-perf/competitive")
                .join(format!("{experiment_id}.json"))
        });
    write_report(&output, &report)?;
    println!("{}", output.display());
    if report.acceptance.passed {
        Ok(())
    } else {
        Err(format!(
            "competitive baseline acceptance failed; evidence written to {}",
            output.display()
        ))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn profile_contracts_fix_explicit_optimization_modes() {
        assert_eq!(Profile::Development.nia_optimization(), "-O0");
        assert_eq!(Profile::Development.rust_optimization(), "opt-level=0");
        assert_eq!(Profile::Development.zig_optimization(), "Debug");
        assert_eq!(Profile::Release.nia_optimization(), "-O2");
        assert_eq!(Profile::Release.rust_optimization(), "opt-level=2");
        assert_eq!(Profile::Release.zig_optimization(), "ReleaseSafe");
    }

    #[test]
    fn rejects_duplicate_selection() {
        let root = Path::new("/repo");
        let mut options = Options::for_repository(root);
        options.profiles = vec![Profile::Development, Profile::Development];
        assert!(
            selected_profiles(&options)
                .unwrap_err()
                .contains("repeated")
        );
        options.profiles.clear();
        options.workloads = vec!["minimal_check".to_owned(), "minimal_check".to_owned()];
        assert!(
            selected_workloads(&options)
                .unwrap_err()
                .contains("repeated")
        );
    }

    #[test]
    fn acceptance_enforces_state_and_transition_contracts() {
        let transition = |state, passed| TransitionEvidence {
            predecessor_sequence: (!matches!(state, SampleState::Clean)).then_some(1),
            predecessor_process_id: (!matches!(state, SampleState::Clean)).then_some(100),
            process_satisfied: passed,
            expected_changed_files: Vec::new(),
            changed_files: Vec::new(),
            source_satisfied: passed,
            expected_artifact_relation: ArtifactRelation::NotApplicable,
            artifact_satisfied: passed,
        };
        let initial = |product_existed, output_existed| InitialState {
            project_products: vec![ProjectProductState {
                path: "cache".to_owned(),
                existed: product_existed,
            }],
            output_existed,
        };
        let cold = initial(false, false);
        assert!(
            sample_acceptance(
                SampleState::Clean,
                &cold,
                &transition(SampleState::Clean, true),
                true,
                true,
                Some(true),
            )
            .passed
        );
        assert!(
            !sample_acceptance(
                SampleState::Clean,
                &cold,
                &transition(SampleState::Clean, true),
                true,
                true,
                Some(false),
            )
            .passed
        );
        assert!(
            !sample_acceptance(
                SampleState::Clean,
                &cold,
                &transition(SampleState::Clean, true),
                false,
                true,
                None,
            )
            .passed
        );
        assert!(
            !sample_acceptance(
                SampleState::Clean,
                &initial(true, false),
                &transition(SampleState::Clean, true),
                true,
                true,
                None,
            )
            .passed
        );
        assert!(
            !sample_acceptance(
                SampleState::Clean,
                &initial(false, true),
                &transition(SampleState::Clean, true),
                true,
                true,
                None,
            )
            .passed
        );
        let warm = initial(true, true);
        assert!(
            sample_acceptance(
                SampleState::NoOpWarm,
                &warm,
                &transition(SampleState::NoOpWarm, true),
                true,
                true,
                Some(true),
            )
            .passed
        );
        assert!(
            !sample_acceptance(
                SampleState::NoOpWarm,
                &warm,
                &transition(SampleState::NoOpWarm, false),
                true,
                true,
                Some(true),
            )
            .passed
        );
        let partial_warm = InitialState {
            project_products: vec![
                ProjectProductState {
                    path: "build".to_owned(),
                    existed: true,
                },
                ProjectProductState {
                    path: "cache".to_owned(),
                    existed: false,
                },
            ],
            output_existed: true,
        };
        assert!(
            !sample_acceptance(
                SampleState::NoOpWarm,
                &partial_warm,
                &transition(SampleState::NoOpWarm, true),
                true,
                true,
                Some(true),
            )
            .passed
        );
    }

    #[test]
    fn generated_source_manifest_is_relocation_stable() {
        let specifications = [
            synthetic::Specification::flat_star(100),
            synthetic::Specification::deep_chain(100),
            synthetic::Specification::mixed_fan_out(100),
        ];
        for specification in specifications {
            for language in [Language::Nia, Language::Rust, Language::Zig] {
                let first = TemporaryDirectory::new("nia-synthetic-manifest-").unwrap();
                let second = TemporaryDirectory::new("nia-synthetic-manifest-").unwrap();
                synthetic::generate(first.path(), language, specification).unwrap();
                synthetic::generate(second.path(), language, specification).unwrap();
                let first =
                    source_snapshot(first.path(), "generated:competitive_synthetic/test").unwrap();
                let second =
                    source_snapshot(second.path(), "generated:competitive_synthetic/test").unwrap();
                assert_eq!(first.manifest.blake3, second.manifest.blake3);
                assert_eq!(first.manifest.file_count, 101);
                assert_eq!(first.manifest.file_count, second.manifest.file_count);
                assert_eq!(first.manifest.size_bytes, second.manifest.size_bytes);
            }
        }
    }

    #[test]
    fn source_diff_ignores_products_and_identifies_one_leaf() {
        let temporary = TemporaryDirectory::new("nia-synthetic-diff-").unwrap();
        let project = temporary.path();
        fs::create_dir_all(project.join("src")).unwrap();
        fs::write(project.join("src/main.nia"), "pub module leaf;\n").unwrap();
        fs::write(
            project.join("src/leaf.nia"),
            "pub fn value() i64 { 1i64 }\n",
        )
        .unwrap();
        let before = source_snapshot(project, "generated:test").unwrap();

        fs::create_dir_all(project.join(".nia-build")).unwrap();
        fs::create_dir_all(project.join(".nia-cache")).unwrap();
        fs::create_dir_all(project.join("target")).unwrap();
        fs::create_dir_all(project.join(".zig-cache")).unwrap();
        fs::create_dir_all(project.join("zig-out")).unwrap();
        fs::write(project.join(".nia-build/app"), "product").unwrap();
        fs::write(project.join("target/app"), "product").unwrap();
        fs::write(
            project.join("src/leaf.nia"),
            "pub fn value() i64 { 2i64 }\n",
        )
        .unwrap();

        let after = source_snapshot(project, "generated:test").unwrap();
        assert_eq!(changed_files(&before, &after), ["src/leaf.nia"]);
        assert_eq!(before.manifest.file_count, after.manifest.file_count);
    }
}
