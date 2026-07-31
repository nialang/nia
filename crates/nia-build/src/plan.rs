// SPDX-License-Identifier: GPL-3.0-or-later

use std::collections::{BTreeMap, BTreeSet};
use std::fmt;

mod codec;
mod handoff;

pub use codec::*;
pub use handoff::*;

pub const BUILD_PLAN_SCHEMA_VERSION: u32 = nia_toolchain::BUILD_PROTOCOL_SCHEMA;

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum StableNameError {
    Empty,
    InvalidCharacter { index: usize, character: char },
}

impl fmt::Display for StableNameError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Empty => f.write_str("stable name is empty"),
            Self::InvalidCharacter { index, character } => write!(
                f,
                "stable name contains invalid character `{character}` at byte {index}"
            ),
        }
    }
}

impl std::error::Error for StableNameError {}

fn validate_stable_name(name: &str) -> Result<(), StableNameError> {
    if name.is_empty() {
        return Err(StableNameError::Empty);
    }
    for (index, character) in name.char_indices() {
        if !(character.is_ascii_alphanumeric() || matches!(character, '-' | '_' | '.')) {
            return Err(StableNameError::InvalidCharacter { index, character });
        }
    }
    Ok(())
}

#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct PackageKey(String);

impl PackageKey {
    pub fn new(name: impl Into<String>) -> Result<Self, StableNameError> {
        let name = name.into();
        validate_stable_name(&name)?;
        Ok(Self(name))
    }

    pub fn root() -> Self {
        Self("root".to_string())
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }
}

macro_rules! define_node_key {
    ($name:ident) => {
        #[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash)]
        pub struct $name {
            package: PackageKey,
            name: String,
        }

        impl $name {
            pub fn new(
                package: PackageKey,
                name: impl Into<String>,
            ) -> Result<Self, StableNameError> {
                let name = name.into();
                validate_stable_name(&name)?;
                Ok(Self { package, name })
            }

            pub fn package(&self) -> &PackageKey {
                &self.package
            }

            pub fn name(&self) -> &str {
                &self.name
            }
        }
    };
}

define_node_key!(ModuleKey);
define_node_key!(ArtifactKey);
define_node_key!(ActionKey);
define_node_key!(StepKey);

#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum LogicalPathRoot {
    Package(PackageKey),
    Build,
    Cache,
    Toolchain,
    Artifact(ArtifactKey),
}

#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct LogicalPath {
    root: LogicalPathRoot,
    components: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum LogicalPathError {
    Absolute,
    EmptyComponent,
    CurrentDirectory,
    ParentDirectory,
    Backslash,
    Nul,
}

impl fmt::Display for LogicalPathError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(match self {
            Self::Absolute => "logical path must be relative to its typed root",
            Self::EmptyComponent => "logical path contains an empty component",
            Self::CurrentDirectory => "logical path contains a `.` component",
            Self::ParentDirectory => "logical path contains a `..` component",
            Self::Backslash => "logical path must use `/` as its protocol separator",
            Self::Nul => "logical path contains a NUL byte",
        })
    }
}

impl std::error::Error for LogicalPathError {}

impl LogicalPath {
    pub fn new(root: LogicalPathRoot, path: &str) -> Result<Self, LogicalPathError> {
        if path.starts_with('/') {
            return Err(LogicalPathError::Absolute);
        }
        if path.contains('\\') {
            return Err(LogicalPathError::Backslash);
        }
        if path.contains('\0') {
            return Err(LogicalPathError::Nul);
        }
        let mut components = Vec::new();
        if !path.is_empty() {
            for component in path.split('/') {
                match component {
                    "" => return Err(LogicalPathError::EmptyComponent),
                    "." => return Err(LogicalPathError::CurrentDirectory),
                    ".." => return Err(LogicalPathError::ParentDirectory),
                    value => components.push(value.to_string()),
                }
            }
        }
        Ok(Self { root, components })
    }

    pub fn root(&self) -> &LogicalPathRoot {
        &self.root
    }

    pub fn components(&self) -> &[String] {
        &self.components
    }

    pub fn protocol_path(&self) -> String {
        self.components.join("/")
    }

    fn is_empty(&self) -> bool {
        self.components.is_empty()
    }
}

#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord)]
pub struct TargetSpec {
    pub arch: String,
    pub vendor: String,
    pub os: String,
    pub env: String,
    pub abi: String,
    pub endian: String,
    pub pointer_width: u32,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum OptimizationMode {
    O0,
    O1,
    O2,
    O3,
    Os,
    Oz,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum Runtime {
    Bare,
    Freestanding,
}

#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord)]
pub struct ModuleImport {
    pub name: String,
    pub path: LogicalPath,
}

#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord)]
pub struct PlanModule {
    pub key: ModuleKey,
    pub root_source: LogicalPath,
    pub optimization: OptimizationMode,
    pub imports: Vec<ModuleImport>,
}

#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord)]
pub struct PlanPackage {
    pub key: PackageKey,
}

#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord)]
pub struct PlanArtifact {
    pub key: ArtifactKey,
    pub root_module: ModuleKey,
    pub output: LogicalPath,
    pub runtime: Runtime,
}

#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord)]
pub enum CommandProgram {
    Path(LogicalPath),
    Search(String),
}

#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord)]
pub struct EnvironmentInput {
    pub name: String,
    pub value: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord)]
pub enum ActionKind {
    CompilerCheck {
        module: ModuleKey,
        target: TargetSpec,
        runtime: Runtime,
    },
    CompilerEmit {
        artifact: ArtifactKey,
        target: TargetSpec,
    },
    ExternalCommand {
        program: CommandProgram,
        arguments: Vec<String>,
        working_directory: LogicalPath,
        environment: Vec<EnvironmentInput>,
        inputs: Vec<LogicalPath>,
        outputs: Vec<LogicalPath>,
    },
    GeneratedFile {
        output: LogicalPath,
        contents: Vec<u8>,
    },
    Aggregate,
    Uncacheable {
        description: String,
    },
}

#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord)]
pub struct PlanAction {
    pub key: ActionKey,
    pub kind: ActionKind,
}

#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord)]
pub struct PlanStep {
    pub key: StepKey,
    pub action: ActionKey,
    pub dependencies: Vec<StepKey>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct OutputCollision {
    pub path: LogicalPath,
    pub first: ActionKey,
    pub second: ActionKey,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BuildPlanDraft {
    pub root_package: PackageKey,
    pub packages: Vec<PlanPackage>,
    pub host_target: TargetSpec,
    pub artifact_target: TargetSpec,
    pub modules: Vec<PlanModule>,
    pub artifacts: Vec<PlanArtifact>,
    pub actions: Vec<PlanAction>,
    pub steps: Vec<PlanStep>,
    pub default_step: Option<StepKey>,
    pub selected_step: Option<StepKey>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BuildPlan {
    schema_version: u32,
    root_package: PackageKey,
    packages: Vec<PlanPackage>,
    host_target: TargetSpec,
    artifact_target: TargetSpec,
    modules: Vec<PlanModule>,
    artifacts: Vec<PlanArtifact>,
    actions: Vec<PlanAction>,
    steps: Vec<PlanStep>,
    default_step: Option<StepKey>,
    selected_step: Option<StepKey>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PlanError {
    DuplicatePackage(PackageKey),
    MissingPackage(PackageKey),
    DuplicateModule(ModuleKey),
    DuplicateArtifact(ArtifactKey),
    DuplicateAction(ActionKey),
    DuplicateStep(StepKey),
    DuplicateImport {
        module: ModuleKey,
        name: String,
    },
    MissingModule {
        owner: String,
        module: ModuleKey,
    },
    MissingArtifact {
        action: ActionKey,
        artifact: ArtifactKey,
    },
    MissingAction {
        step: StepKey,
        action: ActionKey,
    },
    MissingStep {
        owner: String,
        step: StepKey,
    },
    StepCycle(Vec<StepKey>),
    InvalidOutput {
        action: ActionKey,
        path: LogicalPath,
    },
    OutputCollision(Box<OutputCollision>),
    InvalidCommand {
        action: ActionKey,
        reason: &'static str,
    },
    MissingDefaultStep,
    InvalidTarget {
        role: &'static str,
        reason: &'static str,
    },
}

impl fmt::Display for PlanError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "invalid build plan: {self:?}")
    }
}

impl std::error::Error for PlanError {}

impl BuildPlan {
    pub fn freeze(mut draft: BuildPlanDraft) -> Result<Self, PlanError> {
        validate_target(&draft.host_target, "host")?;
        validate_target(&draft.artifact_target, "artifact")?;
        canonicalize_packages(&mut draft.packages, &draft.root_package)?;
        validate_package_references(&draft)?;
        canonicalize_modules(&mut draft.modules)?;
        canonicalize_artifacts(&mut draft.artifacts, &draft.modules)?;
        canonicalize_actions(&mut draft.actions, &draft.modules, &draft.artifacts)?;
        canonicalize_steps(&mut draft.steps, &draft.actions)?;
        validate_step_selection(&draft)?;
        validate_step_cycles(&draft.steps)?;
        validate_output_ownership(&draft.actions, &draft.artifacts)?;

        Ok(Self {
            schema_version: BUILD_PLAN_SCHEMA_VERSION,
            root_package: draft.root_package,
            packages: draft.packages,
            host_target: draft.host_target,
            artifact_target: draft.artifact_target,
            modules: draft.modules,
            artifacts: draft.artifacts,
            actions: draft.actions,
            steps: draft.steps,
            default_step: draft.default_step,
            selected_step: draft.selected_step,
        })
    }

    pub fn schema_version(&self) -> u32 {
        self.schema_version
    }
    pub fn root_package(&self) -> &PackageKey {
        &self.root_package
    }
    pub fn packages(&self) -> &[PlanPackage] {
        &self.packages
    }
    pub fn host_target(&self) -> &TargetSpec {
        &self.host_target
    }
    pub fn artifact_target(&self) -> &TargetSpec {
        &self.artifact_target
    }
    pub fn modules(&self) -> &[PlanModule] {
        &self.modules
    }
    pub fn artifacts(&self) -> &[PlanArtifact] {
        &self.artifacts
    }
    pub fn actions(&self) -> &[PlanAction] {
        &self.actions
    }
    pub fn steps(&self) -> &[PlanStep] {
        &self.steps
    }
    pub fn default_step(&self) -> Option<&StepKey> {
        self.default_step.as_ref()
    }
    pub fn selected_step(&self) -> Option<&StepKey> {
        self.selected_step.as_ref()
    }
}

fn validate_target(target: &TargetSpec, role: &'static str) -> Result<(), PlanError> {
    if target.arch.is_empty() || target.os.is_empty() {
        return Err(PlanError::InvalidTarget {
            role,
            reason: "architecture and operating system must be named",
        });
    }
    if [
        &target.arch,
        &target.vendor,
        &target.os,
        &target.env,
        &target.abi,
        &target.endian,
    ]
    .into_iter()
    .any(|value| value.contains('\0'))
    {
        return Err(PlanError::InvalidTarget {
            role,
            reason: "target field contains NUL",
        });
    }
    if !matches!(target.endian.as_str(), "little" | "big") {
        return Err(PlanError::InvalidTarget {
            role,
            reason: "endianness must be `little` or `big`",
        });
    }
    if !matches!(target.pointer_width, 8 | 16 | 32 | 64 | 128) {
        return Err(PlanError::InvalidTarget {
            role,
            reason: "unsupported pointer width",
        });
    }
    Ok(())
}

fn canonicalize_packages(
    packages: &mut [PlanPackage],
    root_package: &PackageKey,
) -> Result<(), PlanError> {
    packages.sort_by(|left, right| left.key.cmp(&right.key));
    reject_duplicate_by(
        packages,
        |package| package.key.clone(),
        PlanError::DuplicatePackage,
    )?;
    if !packages.iter().any(|package| &package.key == root_package) {
        return Err(PlanError::MissingPackage(root_package.clone()));
    }
    Ok(())
}

fn validate_package_references(draft: &BuildPlanDraft) -> Result<(), PlanError> {
    let packages: BTreeSet<_> = draft.packages.iter().map(|package| &package.key).collect();
    let require = |package: &PackageKey| {
        if packages.contains(package) {
            Ok(())
        } else {
            Err(PlanError::MissingPackage(package.clone()))
        }
    };
    for module in &draft.modules {
        require(module.key.package())?;
        validate_path_package(&module.root_source, &require)?;
        for import in &module.imports {
            validate_path_package(&import.path, &require)?;
        }
    }
    for artifact in &draft.artifacts {
        require(artifact.key.package())?;
        require(artifact.root_module.package())?;
        validate_path_package(&artifact.output, &require)?;
    }
    for action in &draft.actions {
        require(action.key.package())?;
        match &action.kind {
            ActionKind::CompilerCheck { module, .. } => require(module.package())?,
            ActionKind::CompilerEmit { artifact, .. } => require(artifact.package())?,
            ActionKind::ExternalCommand {
                program,
                working_directory,
                inputs,
                outputs,
                ..
            } => {
                if let CommandProgram::Path(path) = program {
                    validate_path_package(path, &require)?;
                }
                validate_path_package(working_directory, &require)?;
                for path in inputs.iter().chain(outputs) {
                    validate_path_package(path, &require)?;
                }
            }
            ActionKind::GeneratedFile { output, .. } => {
                validate_path_package(output, &require)?;
            }
            ActionKind::Aggregate | ActionKind::Uncacheable { .. } => {}
        }
    }
    for step in &draft.steps {
        require(step.key.package())?;
        require(step.action.package())?;
        for dependency in &step.dependencies {
            require(dependency.package())?;
        }
    }
    if let Some(step) = &draft.default_step {
        require(step.package())?;
    }
    if let Some(step) = &draft.selected_step {
        require(step.package())?;
    }
    Ok(())
}

fn validate_path_package(
    path: &LogicalPath,
    require: &impl Fn(&PackageKey) -> Result<(), PlanError>,
) -> Result<(), PlanError> {
    match path.root() {
        LogicalPathRoot::Package(package) => require(package),
        LogicalPathRoot::Artifact(artifact) => require(artifact.package()),
        _ => Ok(()),
    }
}

fn canonicalize_modules(modules: &mut [PlanModule]) -> Result<(), PlanError> {
    modules.sort_by(|left, right| left.key.cmp(&right.key));
    reject_duplicate_by(
        modules,
        |module| module.key.clone(),
        PlanError::DuplicateModule,
    )?;
    for module in modules {
        module.imports.sort();
        for pair in module.imports.windows(2) {
            if pair[0].name == pair[1].name {
                return Err(PlanError::DuplicateImport {
                    module: module.key.clone(),
                    name: pair[0].name.clone(),
                });
            }
        }
    }
    Ok(())
}

fn canonicalize_artifacts(
    artifacts: &mut [PlanArtifact],
    modules: &[PlanModule],
) -> Result<(), PlanError> {
    artifacts.sort_by(|left, right| left.key.cmp(&right.key));
    reject_duplicate_by(
        artifacts,
        |artifact| artifact.key.clone(),
        PlanError::DuplicateArtifact,
    )?;
    let module_keys: BTreeSet<_> = modules.iter().map(|module| &module.key).collect();
    for artifact in artifacts {
        if !module_keys.contains(&artifact.root_module) {
            return Err(PlanError::MissingModule {
                owner: format!("artifact {}", artifact.key.name()),
                module: artifact.root_module.clone(),
            });
        }
    }
    Ok(())
}

fn canonicalize_actions(
    actions: &mut [PlanAction],
    modules: &[PlanModule],
    artifacts: &[PlanArtifact],
) -> Result<(), PlanError> {
    actions.sort_by(|left, right| left.key.cmp(&right.key));
    reject_duplicate_by(
        actions,
        |action| action.key.clone(),
        PlanError::DuplicateAction,
    )?;
    let module_keys: BTreeSet<_> = modules.iter().map(|module| &module.key).collect();
    let artifact_keys: BTreeSet<_> = artifacts.iter().map(|artifact| &artifact.key).collect();
    for action in actions {
        match &mut action.kind {
            ActionKind::CompilerCheck { module, .. } if !module_keys.contains(module) => {
                return Err(PlanError::MissingModule {
                    owner: format!("action {}", action.key.name()),
                    module: module.clone(),
                });
            }
            ActionKind::CompilerEmit { artifact, .. } if !artifact_keys.contains(artifact) => {
                return Err(PlanError::MissingArtifact {
                    action: action.key.clone(),
                    artifact: artifact.clone(),
                });
            }
            ActionKind::ExternalCommand {
                program,
                arguments,
                environment,
                inputs,
                outputs,
                ..
            } => {
                if matches!(program, CommandProgram::Search(name) if name.is_empty()) {
                    return Err(PlanError::InvalidCommand {
                        action: action.key.clone(),
                        reason: "empty program",
                    });
                }
                if arguments.iter().any(|value| value.contains('\0')) {
                    return Err(PlanError::InvalidCommand {
                        action: action.key.clone(),
                        reason: "argument contains NUL",
                    });
                }
                environment.sort();
                inputs.sort();
                outputs.sort();
            }
            _ => {}
        }
    }
    Ok(())
}

fn canonicalize_steps(steps: &mut [PlanStep], actions: &[PlanAction]) -> Result<(), PlanError> {
    steps.sort_by(|left, right| left.key.cmp(&right.key));
    reject_duplicate_by(steps, |step| step.key.clone(), PlanError::DuplicateStep)?;
    let action_keys: BTreeSet<_> = actions.iter().map(|action| &action.key).collect();
    let step_keys: BTreeSet<_> = steps.iter().map(|step| step.key.clone()).collect();
    for step in steps {
        if !action_keys.contains(&step.action) {
            return Err(PlanError::MissingAction {
                step: step.key.clone(),
                action: step.action.clone(),
            });
        }
        step.dependencies.sort();
        step.dependencies.dedup();
        for dependency in &step.dependencies {
            if !step_keys.contains(dependency) {
                return Err(PlanError::MissingStep {
                    owner: format!("step {}", step.key.name()),
                    step: dependency.clone(),
                });
            }
        }
    }
    Ok(())
}

fn validate_step_selection(draft: &BuildPlanDraft) -> Result<(), PlanError> {
    let keys: BTreeSet<_> = draft.steps.iter().map(|step| &step.key).collect();
    if !draft.steps.is_empty() && draft.default_step.is_none() && draft.selected_step.is_none() {
        return Err(PlanError::MissingDefaultStep);
    }
    for (owner, selected) in [
        ("default", &draft.default_step),
        ("selected", &draft.selected_step),
    ] {
        if let Some(step) = selected
            && !keys.contains(step)
        {
            return Err(PlanError::MissingStep {
                owner: owner.to_string(),
                step: step.clone(),
            });
        }
    }
    Ok(())
}

fn validate_step_cycles(steps: &[PlanStep]) -> Result<(), PlanError> {
    let mut indegree: BTreeMap<_, usize> = steps.iter().map(|step| (step.key.clone(), 0)).collect();
    let mut dependents: BTreeMap<StepKey, Vec<StepKey>> = BTreeMap::new();
    for step in steps {
        for dependency in &step.dependencies {
            *indegree.get_mut(&step.key).expect("validated step key") += 1;
            dependents
                .entry(dependency.clone())
                .or_default()
                .push(step.key.clone());
        }
    }
    let mut ready: BTreeSet<_> = indegree
        .iter()
        .filter_map(|(key, degree)| (*degree == 0).then_some(key.clone()))
        .collect();
    let mut visited = 0;
    while let Some(key) = ready.pop_first() {
        visited += 1;
        if let Some(items) = dependents.get(&key) {
            for dependent in items {
                let degree = indegree
                    .get_mut(dependent)
                    .expect("validated dependent key");
                *degree -= 1;
                if *degree == 0 {
                    ready.insert(dependent.clone());
                }
            }
        }
    }
    if visited != steps.len() {
        return Err(PlanError::StepCycle(
            indegree
                .into_iter()
                .filter_map(|(key, degree)| (degree != 0).then_some(key))
                .collect(),
        ));
    }
    Ok(())
}

fn validate_output_ownership(
    actions: &[PlanAction],
    artifacts: &[PlanArtifact],
) -> Result<(), PlanError> {
    let artifacts: BTreeMap<_, _> = artifacts
        .iter()
        .map(|artifact| (&artifact.key, &artifact.output))
        .collect();
    let mut owners: BTreeMap<LogicalPath, ActionKey> = BTreeMap::new();
    for action in actions {
        let outputs: Vec<&LogicalPath> = match &action.kind {
            ActionKind::CompilerEmit { artifact, .. } => vec![artifacts[artifact]],
            ActionKind::ExternalCommand { outputs, .. } => outputs.iter().collect(),
            ActionKind::GeneratedFile { output, .. } => vec![output],
            _ => Vec::new(),
        };
        for output in outputs {
            if output.is_empty() || !matches!(output.root(), LogicalPathRoot::Build) {
                return Err(PlanError::InvalidOutput {
                    action: action.key.clone(),
                    path: output.clone(),
                });
            }
            if let Some(first) = owners.insert(output.clone(), action.key.clone()) {
                return Err(PlanError::OutputCollision(Box::new(OutputCollision {
                    path: output.clone(),
                    first,
                    second: action.key.clone(),
                })));
            }
        }
    }
    Ok(())
}

fn reject_duplicate_by<T, K: Ord + Clone>(
    values: &[T],
    key: impl Fn(&T) -> K,
    error: impl Fn(K) -> PlanError,
) -> Result<(), PlanError> {
    for pair in values.windows(2) {
        let left = key(&pair[0]);
        let right = key(&pair[1]);
        if left == right {
            return Err(error(left));
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn module_key(name: &str) -> ModuleKey {
        ModuleKey::new(PackageKey::root(), name).unwrap()
    }

    fn artifact_key(name: &str) -> ArtifactKey {
        ArtifactKey::new(PackageKey::root(), name).unwrap()
    }

    fn action_key(name: &str) -> ActionKey {
        ActionKey::new(PackageKey::root(), name).unwrap()
    }

    fn step_key(name: &str) -> StepKey {
        StepKey::new(PackageKey::root(), name).unwrap()
    }

    fn target() -> TargetSpec {
        TargetSpec {
            arch: "x86_64".into(),
            vendor: "unknown".into(),
            os: "linux".into(),
            env: "gnu".into(),
            abi: "none".into(),
            endian: "little".into(),
            pointer_width: 64,
        }
    }

    pub(crate) fn draft(reverse: bool) -> BuildPlanDraft {
        let module_a = module_key("a");
        let module_b = module_key("b");
        let artifact = artifact_key("app");
        let check = action_key("check");
        let emit = action_key("emit");
        let check_step = step_key("check");
        let emit_step = step_key("emit");
        let mut modules = vec![
            PlanModule {
                key: module_a.clone(),
                root_source: LogicalPath::new(
                    LogicalPathRoot::Package(PackageKey::root()),
                    "src/a.nia",
                )
                .unwrap(),
                optimization: OptimizationMode::O2,
                imports: vec![],
            },
            PlanModule {
                key: module_b,
                root_source: LogicalPath::new(
                    LogicalPathRoot::Package(PackageKey::root()),
                    "src/b.nia",
                )
                .unwrap(),
                optimization: OptimizationMode::O0,
                imports: vec![],
            },
        ];
        let mut actions = vec![
            PlanAction {
                key: check.clone(),
                kind: ActionKind::CompilerCheck {
                    module: module_a.clone(),
                    target: target(),
                    runtime: Runtime::Freestanding,
                },
            },
            PlanAction {
                key: emit.clone(),
                kind: ActionKind::CompilerEmit {
                    artifact: artifact.clone(),
                    target: target(),
                },
            },
        ];
        let mut steps = vec![
            PlanStep {
                key: check_step.clone(),
                action: check,
                dependencies: vec![],
            },
            PlanStep {
                key: emit_step.clone(),
                action: emit,
                dependencies: vec![check_step],
            },
        ];
        if reverse {
            modules.reverse();
            actions.reverse();
            steps.reverse();
        }
        BuildPlanDraft {
            root_package: PackageKey::root(),
            packages: vec![PlanPackage {
                key: PackageKey::root(),
            }],
            host_target: target(),
            artifact_target: target(),
            modules,
            artifacts: vec![PlanArtifact {
                key: artifact,
                root_module: module_a,
                output: LogicalPath::new(LogicalPathRoot::Build, "app").unwrap(),
                runtime: Runtime::Freestanding,
            }],
            actions,
            steps,
            default_step: Some(emit_step),
            selected_step: None,
        }
    }

    #[test]
    fn freeze_is_independent_of_allocation_order() {
        assert_eq!(
            BuildPlan::freeze(draft(false)).unwrap(),
            BuildPlan::freeze(draft(true)).unwrap()
        );
    }

    #[test]
    fn logical_paths_reject_host_and_escape_semantics() {
        assert_eq!(
            LogicalPath::new(LogicalPathRoot::Package(PackageKey::root()), "/tmp/a"),
            Err(LogicalPathError::Absolute)
        );
        assert_eq!(
            LogicalPath::new(LogicalPathRoot::Package(PackageKey::root()), "src/../a",),
            Err(LogicalPathError::ParentDirectory)
        );
        assert_eq!(
            LogicalPath::new(LogicalPathRoot::Package(PackageKey::root()), "src\\a"),
            Err(LogicalPathError::Backslash)
        );
    }

    #[test]
    fn freeze_rejects_cycles_before_execution() {
        let mut value = draft(false);
        let first = value.steps[0].key.clone();
        let second = value.steps[1].key.clone();
        value.steps[0].dependencies = vec![second];
        value.steps[1].dependencies = vec![first];
        assert!(matches!(
            BuildPlan::freeze(value),
            Err(PlanError::StepCycle(_))
        ));
    }

    #[test]
    fn explicit_selection_does_not_require_an_unused_default() {
        let mut value = draft(false);
        value.selected_step = value.default_step.take();
        assert!(BuildPlan::freeze(value).is_ok());
    }

    #[test]
    fn nonempty_plan_requires_a_default_or_explicit_selection() {
        let mut value = draft(false);
        value.default_step = None;
        value.selected_step = None;
        assert_eq!(BuildPlan::freeze(value), Err(PlanError::MissingDefaultStep));
    }

    #[test]
    fn freeze_rejects_output_collisions() {
        let mut value = draft(false);
        value.actions.push(PlanAction {
            key: action_key("generate"),
            kind: ActionKind::GeneratedFile {
                output: LogicalPath::new(LogicalPathRoot::Build, "app").unwrap(),
                contents: vec![],
            },
        });
        assert!(matches!(
            BuildPlan::freeze(value),
            Err(PlanError::OutputCollision(_))
        ));
    }
}
