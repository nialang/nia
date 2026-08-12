// SPDX-License-Identifier: GPL-3.0-or-later
//! Immutable build-plan model and semantic validation boundary.
//!
//! Stable keys combine package identity with validated visible names, while
//! logical paths retain their typed root independently of physical checkout
//! paths. `BuildPlan::freeze` canonicalizes order and validates the complete
//! graph before a plan can be encoded or executed.

use std::collections::{BTreeMap, BTreeSet};
use std::fmt;

use nia_compat::formats::BUILD_PLAN;

mod actions;
mod codec;
mod dependencies;
mod handoff;
#[cfg(test)]
mod test_support;

use actions::*;
pub use codec::*;
use dependencies::*;
pub use handoff::*;

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

    pub(crate) fn overlaps(&self, other: &Self) -> bool {
        self.root == other.root
            && (self.components.starts_with(&other.components)
                || other.components.starts_with(&self.components))
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
    /// Package root relative to the invocation's root package.
    /// The root package is represented by the empty path.
    pub root: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PackageRootError {
    RootPackageMustBeEmpty,
    ExternalPackageMustBeNonempty,
    InvalidPath(LogicalPathError),
}

#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord)]
pub enum PlanArtifactKind {
    Executable,
    ObjectSet,
    StaticArchive,
}

#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord)]
pub struct PlanArtifact {
    pub key: ArtifactKey,
    pub root_module: ModuleKey,
    pub kind: PlanArtifactKind,
    pub output: LogicalPath,
    pub runtime: Runtime,
}

#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord)]
pub enum CommandProgram {
    Path(LogicalPath),
    Search(String),
}

#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord)]
pub enum CommandArgument {
    Literal(String),
    InputPath(LogicalPath),
    OutputPath(LogicalPath),
}

#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord)]
pub struct EnvironmentInput {
    pub name: String,
    pub value: Option<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum CommandEnvironmentPolicy {
    Inherit,
    Clear,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum CommandCachePolicy {
    Uncacheable,
    /// Cache by the resolved tool, cleared declared environment, and explicit
    /// `inputs`. The working directory contributes logical location identity,
    /// but its undeclared contents are deliberately not implicit inputs.
    DeclaredInputs,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum ActionResourceClass {
    Conservative,
    Cpu,
    Io,
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
        static_archives: Vec<ArtifactKey>,
    },
    ExternalCommand {
        resource_class: ActionResourceClass,
        environment_policy: CommandEnvironmentPolicy,
        cache_policy: CommandCachePolicy,
        program: CommandProgram,
        arguments: Vec<CommandArgument>,
        working_directory: LogicalPath,
        environment: Vec<EnvironmentInput>,
        inputs: Vec<LogicalPath>,
        outputs: Vec<LogicalPath>,
    },
    GeneratedFile {
        output: LogicalPath,
        contents: Vec<u8>,
    },
    InstallArtifact {
        artifact: ArtifactKey,
        destination: LogicalPath,
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

impl PlanAction {
    pub fn resource_class(&self) -> ActionResourceClass {
        match &self.kind {
            ActionKind::ExternalCommand { resource_class, .. } => *resource_class,
            ActionKind::CompilerCheck { .. } | ActionKind::CompilerEmit { .. } => {
                ActionResourceClass::Cpu
            }
            ActionKind::GeneratedFile { .. }
            | ActionKind::InstallArtifact { .. }
            | ActionKind::Aggregate => ActionResourceClass::Io,
            ActionKind::Uncacheable { .. } => ActionResourceClass::Conservative,
        }
    }
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
    InvalidPackageRoot {
        package: PackageKey,
        error: PackageRootError,
    },
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
    InvalidArtifactUse {
        action: ActionKey,
        artifact: ArtifactKey,
        reason: &'static str,
    },
    MissingBuildInputProducer {
        action: Box<ActionKey>,
        path: Box<LogicalPath>,
    },
    BuildInputProducerOutsideClosure {
        action: Box<ActionKey>,
        path: Box<LogicalPath>,
        producer: Box<ActionKey>,
    },
    MissingGeneratedSourceProducer {
        action: Box<ActionKey>,
        module: Box<ModuleKey>,
        path: Box<LogicalPath>,
    },
    GeneratedSourceProducerOutsideClosure {
        action: Box<ActionKey>,
        module: Box<ModuleKey>,
        path: Box<LogicalPath>,
        producer: Box<ActionKey>,
    },
    MissingDefaultStep,
    InvalidTarget {
        role: &'static str,
        reason: &'static str,
    },
    InvalidActionTarget(Box<InvalidActionTarget>),
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct InvalidActionTarget {
    pub action: ActionKey,
    pub target: TargetSpec,
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
        canonicalize_actions(
            &mut draft.actions,
            &draft.modules,
            &draft.artifacts,
            &draft.host_target,
            &draft.artifact_target,
        )?;
        canonicalize_steps(&mut draft.steps, &draft.actions)?;
        validate_step_selection(&draft)?;
        validate_step_cycles(&draft.steps)?;
        validate_artifact_dependencies(&draft.actions, &draft.steps)?;
        validate_output_ownership(&draft.actions, &draft.artifacts)?;
        validate_build_input_dependencies(&draft)?;
        validate_generated_source_dependencies(&draft)?;

        Ok(Self {
            schema_version: BUILD_PLAN.schema,
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
    for package in packages {
        let is_root = &package.key == root_package;
        let invalid = |error| PlanError::InvalidPackageRoot {
            package: package.key.clone(),
            error,
        };
        if is_root && !package.root.is_empty() {
            return Err(invalid(PackageRootError::RootPackageMustBeEmpty));
        }
        if !is_root && package.root.is_empty() {
            return Err(invalid(PackageRootError::ExternalPackageMustBeNonempty));
        }
        LogicalPath::new(LogicalPathRoot::Build, &package.root)
            .map_err(|error| invalid(PackageRootError::InvalidPath(error)))?;
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
            ActionKind::CompilerEmit {
                artifact,
                static_archives,
                ..
            } => {
                require(artifact.package())?;
                for archive in static_archives {
                    require(archive.package())?;
                }
            }
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
            ActionKind::InstallArtifact {
                artifact,
                destination,
            } => {
                require(artifact.package())?;
                validate_path_package(destination, &require)?;
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
            let Some(degree) = indegree.get_mut(&step.key) else {
                return Err(PlanError::MissingStep {
                    owner: "cycle validator".to_string(),
                    step: step.key.clone(),
                });
            };
            *degree += 1;
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
                let Some(degree) = indegree.get_mut(dependent) else {
                    return Err(PlanError::MissingStep {
                        owner: "cycle validator dependent".to_string(),
                        step: dependent.clone(),
                    });
                };
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
    use super::test_support::*;
    use super::*;

    #[test]
    fn freeze_is_independent_of_allocation_order() {
        assert_eq!(
            BuildPlan::freeze(draft(false)).unwrap(),
            BuildPlan::freeze(draft(true)).unwrap()
        );
    }

    #[test]
    fn freeze_preserves_typed_static_archive_link_order() {
        let plan = BuildPlan::freeze(static_archive_link_draft()).unwrap();
        let emit = plan
            .actions()
            .iter()
            .find(|action| action.key.name() == "emit")
            .unwrap();
        assert!(matches!(
            &emit.kind,
            ActionKind::CompilerEmit { static_archives, .. }
                if static_archives.iter().map(ArtifactKey::name).collect::<Vec<_>>()
                    == ["runtime", "support"]
        ));
    }

    #[test]
    fn freeze_rejects_invalid_static_archive_link_relationships() {
        let mut wrong_kind = static_archive_link_draft();
        wrong_kind
            .artifacts
            .iter_mut()
            .find(|artifact| artifact.key.name() == "support")
            .unwrap()
            .kind = PlanArtifactKind::ObjectSet;
        assert!(matches!(
            BuildPlan::freeze(wrong_kind),
            Err(PlanError::InvalidArtifactUse {
                reason: "executable link inputs must be static archives",
                ..
            })
        ));

        let mut duplicate = static_archive_link_draft();
        let ActionKind::CompilerEmit {
            static_archives, ..
        } = &mut duplicate
            .actions
            .iter_mut()
            .find(|action| action.key.name() == "emit")
            .unwrap()
            .kind
        else {
            unreachable!()
        };
        static_archives.push(static_archives[0].clone());
        assert!(matches!(
            BuildPlan::freeze(duplicate),
            Err(PlanError::InvalidArtifactUse {
                reason: "duplicate static archive link input",
                ..
            })
        ));

        let mut target_mismatch = static_archive_link_draft();
        let ActionKind::CompilerEmit { target, .. } = &mut target_mismatch
            .actions
            .iter_mut()
            .find(|action| action.key.name() == "emit-support")
            .unwrap()
            .kind
        else {
            unreachable!()
        };
        target.arch = "aarch64".to_string();
        assert!(matches!(
            BuildPlan::freeze(target_mismatch),
            Err(PlanError::InvalidArtifactUse {
                reason: "linked static archive has no emit action for the executable target",
                ..
            })
        ));

        let mut missing_dependency = static_archive_link_draft();
        missing_dependency
            .steps
            .iter_mut()
            .find(|step| step.key.name() == "emit")
            .unwrap()
            .dependencies
            .retain(|dependency| dependency.name() != "emit-support");
        assert!(matches!(
            BuildPlan::freeze(missing_dependency),
            Err(PlanError::InvalidCommand {
                reason: "linked static archive has no compiler emit dependency",
                ..
            })
        ));
    }

    #[test]
    fn freeze_rejects_compiler_targets_outside_the_plan_pair() {
        for action_name in ["check", "emit"] {
            let mut value = draft(false);
            let action = value
                .actions
                .iter_mut()
                .find(|action| action.key.name() == action_name)
                .unwrap();
            let target = match &mut action.kind {
                ActionKind::CompilerCheck { target, .. }
                | ActionKind::CompilerEmit { target, .. } => target,
                _ => unreachable!(),
            };
            target.arch = "third-architecture".to_string();

            assert!(matches!(
                BuildPlan::freeze(value),
                Err(PlanError::InvalidActionTarget(details))
                    if details.action.name() == action_name
                        && details.target.arch == "third-architecture"
            ));
        }
    }

    #[test]
    fn freeze_rejects_multiple_emitters_for_one_artifact() {
        let mut value = draft(false);
        value.actions.push(PlanAction {
            key: action_key("second-emit"),
            kind: ActionKind::CompilerEmit {
                artifact: artifact_key("app"),
                target: target(),
                static_archives: Vec::new(),
            },
        });

        assert!(matches!(
            BuildPlan::freeze(value),
            Err(PlanError::InvalidArtifactUse {
                action,
                artifact,
                reason: "artifact has multiple compiler emit actions",
            }) if action.name() == "second-emit" && artifact.name() == "app"
        ));
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
    fn freeze_validates_declared_package_roots() {
        let mut value = draft(false);
        value.packages.push(PlanPackage {
            key: PackageKey::new("assets").unwrap(),
            root: "packages/assets".to_string(),
        });
        let plan = BuildPlan::freeze(value.clone()).unwrap();
        assert_eq!(plan.packages()[0].key.as_str(), "assets");
        assert_eq!(plan.packages()[0].root, "packages/assets");

        value
            .packages
            .iter_mut()
            .find(|package| package.key == PackageKey::root())
            .unwrap()
            .root = "moved-root".to_string();
        assert!(matches!(
            BuildPlan::freeze(value),
            Err(PlanError::InvalidPackageRoot {
                package,
                error: PackageRootError::RootPackageMustBeEmpty,
            }) if package == PackageKey::root()
        ));

        let mut value = draft(false);
        value.packages.push(PlanPackage {
            key: PackageKey::new("assets").unwrap(),
            root: "packages/../assets".to_string(),
        });
        assert!(matches!(
            BuildPlan::freeze(value),
            Err(PlanError::InvalidPackageRoot {
                package,
                error: PackageRootError::InvalidPath(LogicalPathError::ParentDirectory),
            }) if package.as_str() == "assets"
        ));

        let mut value = draft(false);
        value.packages.push(PlanPackage {
            key: PackageKey::new("assets").unwrap(),
            root: String::new(),
        });
        assert!(matches!(
            BuildPlan::freeze(value),
            Err(PlanError::InvalidPackageRoot {
                package,
                error: PackageRootError::ExternalPackageMustBeNonempty,
            }) if package.as_str() == "assets"
        ));
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
    fn step_cycle_validator_matches_every_four_node_graph() {
        const NODE_COUNT: usize = 4;
        let keys: Vec<_> = (0..NODE_COUNT)
            .map(|index| step_key(&format!("step-{index}")))
            .collect();
        let edge_count = NODE_COUNT * NODE_COUNT;

        for mask in 0u32..(1u32 << edge_count) {
            let mut reach = [[false; NODE_COUNT]; NODE_COUNT];
            let steps: Vec<_> = (0..NODE_COUNT)
                .map(|from| {
                    let dependencies = (0..NODE_COUNT)
                        .filter_map(|to| {
                            let edge = from * NODE_COUNT + to;
                            let present = mask & (1u32 << edge) != 0;
                            reach[from][to] = present;
                            present.then(|| keys[to].clone())
                        })
                        .collect();
                    PlanStep {
                        key: keys[from].clone(),
                        action: action_key("model-action"),
                        dependencies,
                    }
                })
                .collect();

            for intermediate in 0..NODE_COUNT {
                for from in 0..NODE_COUNT {
                    for to in 0..NODE_COUNT {
                        reach[from][to] |= reach[from][intermediate] && reach[intermediate][to];
                    }
                }
            }
            let model_has_cycle = (0..NODE_COUNT).any(|index| reach[index][index]);
            let validator_has_cycle =
                matches!(validate_step_cycles(&steps), Err(PlanError::StepCycle(_)));
            assert_eq!(
                validator_has_cycle, model_has_cycle,
                "edge mask {mask:#06x}"
            );
        }
    }

    #[test]
    fn dependency_closure_reports_a_missing_step_without_panicking() {
        let root = PlanStep {
            key: step_key("root-step"),
            action: action_key("root-action"),
            dependencies: vec![step_key("missing-step")],
        };
        let steps = BTreeMap::from([(&root.key, &root)]);

        assert!(matches!(
            dependency_action_closure(&root, &steps),
            Err(PlanError::MissingStep { step, .. }) if step.name() == "missing-step"
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

    #[test]
    fn freeze_rejects_nested_output_ownership() {
        for (artifact_output, generated_output, conflicting_path) in [
            ("app", "app/metadata", "app/metadata"),
            ("app/binary", "app", "app/binary"),
        ] {
            let mut value = draft(false);
            value.artifacts[0].output =
                LogicalPath::new(LogicalPathRoot::Build, artifact_output).unwrap();
            value.actions.push(PlanAction {
                key: action_key("generate"),
                kind: ActionKind::GeneratedFile {
                    output: LogicalPath::new(LogicalPathRoot::Build, generated_output).unwrap(),
                    contents: vec![],
                },
            });

            let error = BuildPlan::freeze(value).unwrap_err();
            let PlanError::OutputCollision(collision) = error else {
                panic!("expected output collision, got {error:?}");
            };
            assert_eq!(collision.path.protocol_path(), conflicting_path);
        }
    }

    fn add_install_action(
        value: &mut BuildPlanDraft,
        artifact: ArtifactKey,
        destination: &str,
        dependencies: Vec<StepKey>,
    ) {
        let install_action = action_key("install");
        let install_step = step_key("install");
        value.actions.push(PlanAction {
            key: install_action.clone(),
            kind: ActionKind::InstallArtifact {
                artifact,
                destination: LogicalPath::new(LogicalPathRoot::Build, destination).unwrap(),
            },
        });
        value.steps.push(PlanStep {
            key: install_step.clone(),
            action: install_action,
            dependencies,
        });
        value.default_step = Some(install_step);
    }

    #[test]
    fn freeze_accepts_an_install_with_its_emit_in_the_dependency_closure() {
        let mut value = draft(false);
        add_install_action(
            &mut value,
            artifact_key("app"),
            "install/app",
            vec![step_key("emit")],
        );

        let plan = BuildPlan::freeze(value).unwrap();
        let install = plan
            .actions()
            .iter()
            .find(|action| action.key.name() == "install")
            .unwrap();
        assert_eq!(install.resource_class(), ActionResourceClass::Io);
        assert!(matches!(
            &install.kind,
            ActionKind::InstallArtifact { artifact, destination }
                if artifact.name() == "app" && destination.protocol_path() == "install/app"
        ));
    }

    #[test]
    fn freeze_requires_an_install_artifact_and_its_emit_dependency() {
        let mut missing_artifact = draft(false);
        add_install_action(
            &mut missing_artifact,
            artifact_key("missing"),
            "install/app",
            vec![step_key("emit")],
        );
        assert!(matches!(
            BuildPlan::freeze(missing_artifact),
            Err(PlanError::MissingArtifact { artifact, .. }) if artifact.name() == "missing"
        ));

        let mut missing_dependency = draft(false);
        add_install_action(
            &mut missing_dependency,
            artifact_key("app"),
            "install/app",
            Vec::new(),
        );
        assert!(matches!(
            BuildPlan::freeze(missing_dependency),
            Err(PlanError::InvalidCommand {
                reason: "artifact install has no compiler emit dependency",
                ..
            })
        ));
    }

    #[test]
    fn freeze_rejects_installing_an_object_set() {
        let mut value = draft(false);
        value.artifacts[0].kind = PlanArtifactKind::ObjectSet;
        add_install_action(
            &mut value,
            artifact_key("app"),
            "install/app",
            vec![step_key("emit")],
        );

        assert!(matches!(
            BuildPlan::freeze(value),
            Err(PlanError::InvalidArtifactUse {
                artifact,
                reason: "only file artifacts can be installed",
                ..
            }) if artifact.name() == "app"
        ));
    }

    #[test]
    fn freeze_accepts_installing_a_static_archive() {
        let mut value = draft(false);
        value.artifacts[0].kind = PlanArtifactKind::StaticArchive;
        add_install_action(
            &mut value,
            artifact_key("app"),
            "install/libapp.a",
            vec![step_key("emit")],
        );

        let plan = BuildPlan::freeze(value).expect("typed static archive install");
        let install = plan
            .actions()
            .iter()
            .find(|action| action.key.name() == "install")
            .unwrap();
        assert!(matches!(
            &install.kind,
            ActionKind::InstallArtifact { artifact, destination }
                if artifact.name() == "app"
                    && destination.protocol_path() == "install/libapp.a"
        ));
    }

    #[test]
    fn freeze_applies_build_output_rules_to_install_destinations() {
        let mut empty = draft(false);
        add_install_action(&mut empty, artifact_key("app"), "", vec![step_key("emit")]);
        assert!(matches!(
            BuildPlan::freeze(empty),
            Err(PlanError::InvalidOutput { path, .. }) if path.is_empty()
        ));

        let mut collision = draft(false);
        add_install_action(
            &mut collision,
            artifact_key("app"),
            "app",
            vec![step_key("emit")],
        );
        assert!(matches!(
            BuildPlan::freeze(collision),
            Err(PlanError::OutputCollision(_))
        ));
    }

    #[test]
    fn freeze_requires_a_producer_for_build_rooted_compiler_sources() {
        let mut value = generated_source_draft("generated/other.nia", vec![]);
        value.actions.remove(0);
        value.steps.remove(0);
        assert!(matches!(
            BuildPlan::freeze(value),
            Err(PlanError::MissingGeneratedSourceProducer { path, .. })
                if path.protocol_path() == "generated/root.nia"
        ));
    }

    #[test]
    fn freeze_requires_the_generated_source_producer_in_the_consumer_closure() {
        let value = generated_source_draft("generated/root.nia", vec![]);
        assert!(matches!(
            BuildPlan::freeze(value),
            Err(PlanError::GeneratedSourceProducerOutsideClosure { producer, .. })
                if producer.name() == "generate"
        ));
    }

    #[test]
    fn freeze_accepts_an_exact_generated_source_producer_edge() {
        let value = generated_source_draft("generated/root.nia", vec!["generate"]);
        assert!(BuildPlan::freeze(value).is_ok());
    }

    #[test]
    fn freeze_requires_exact_generated_source_path_identity() {
        let value = generated_source_draft("generated/other.nia", vec!["generate"]);
        assert!(matches!(
            BuildPlan::freeze(value),
            Err(PlanError::MissingGeneratedSourceProducer { path, .. })
                if path.protocol_path() == "generated/root.nia"
        ));
    }

    #[test]
    fn freeze_applies_generated_source_closure_to_module_imports() {
        let mut value = generated_source_draft("generated/import.nia", vec!["generate"]);
        value.modules[0].root_source =
            LogicalPath::new(LogicalPathRoot::Package(PackageKey::root()), "src/main.nia").unwrap();
        value.modules[0].imports = vec![ModuleImport {
            name: "generated".to_string(),
            path: LogicalPath::new(LogicalPathRoot::Build, "generated/import.nia").unwrap(),
        }];
        assert!(BuildPlan::freeze(value).is_ok());
    }

    #[test]
    fn freeze_rejects_an_unproduced_build_rooted_module_import() {
        let mut value = generated_source_draft("generated/other.nia", vec!["generate"]);
        value.modules[0].root_source =
            LogicalPath::new(LogicalPathRoot::Package(PackageKey::root()), "src/main.nia").unwrap();
        value.modules[0].imports = vec![ModuleImport {
            name: "generated".to_string(),
            path: LogicalPath::new(LogicalPathRoot::Build, "generated/import.nia").unwrap(),
        }];
        assert!(matches!(
            BuildPlan::freeze(value),
            Err(PlanError::MissingGeneratedSourceProducer { path, .. })
                if path.protocol_path() == "generated/import.nia"
        ));
    }

    #[test]
    fn freeze_reserves_the_output_transaction_journal_root() {
        let mut value = draft(false);
        value.artifacts[0].output =
            LogicalPath::new(LogicalPathRoot::Build, ".nia-transactions/forged-output").unwrap();

        assert!(matches!(
            BuildPlan::freeze(value),
            Err(PlanError::InvalidOutput { path, .. })
                if path.protocol_path() == ".nia-transactions/forged-output"
        ));
    }

    #[test]
    fn freeze_rejects_ambiguous_external_command_environment() {
        let mut value = draft(false);
        value.actions.push(PlanAction {
            key: action_key("run"),
            kind: ActionKind::ExternalCommand {
                resource_class: ActionResourceClass::Conservative,
                environment_policy: CommandEnvironmentPolicy::Inherit,
                cache_policy: CommandCachePolicy::Uncacheable,
                program: CommandProgram::Search("tool".to_string()),
                arguments: Vec::new(),
                working_directory: LogicalPath::new(
                    LogicalPathRoot::Package(PackageKey::root()),
                    "",
                )
                .unwrap(),
                environment: vec![
                    EnvironmentInput {
                        name: "MODE".to_string(),
                        value: Some("first".to_string()),
                    },
                    EnvironmentInput {
                        name: "MODE".to_string(),
                        value: Some("second".to_string()),
                    },
                ],
                inputs: Vec::new(),
                outputs: Vec::new(),
            },
        });

        assert!(matches!(
            BuildPlan::freeze(value),
            Err(PlanError::InvalidCommand {
                reason: "duplicate environment input",
                ..
            })
        ));
    }

    #[test]
    fn freeze_rejects_non_hermetic_or_outputless_cacheable_commands() {
        let output = LogicalPath::new(LogicalPathRoot::Build, "output.txt").unwrap();
        let cacheable = |environment_policy, outputs: Vec<LogicalPath>| PlanAction {
            key: action_key("tool"),
            kind: ActionKind::ExternalCommand {
                resource_class: ActionResourceClass::Io,
                environment_policy,
                cache_policy: CommandCachePolicy::DeclaredInputs,
                program: CommandProgram::Search("tool".to_string()),
                arguments: outputs
                    .iter()
                    .cloned()
                    .map(CommandArgument::OutputPath)
                    .collect(),
                working_directory: LogicalPath::new(
                    LogicalPathRoot::Package(PackageKey::root()),
                    "",
                )
                .unwrap(),
                environment: Vec::new(),
                inputs: Vec::new(),
                outputs,
            },
        };

        let mut inherited = draft(false);
        inherited
            .actions
            .push(cacheable(CommandEnvironmentPolicy::Inherit, vec![output]));
        assert!(matches!(
            BuildPlan::freeze(inherited),
            Err(PlanError::InvalidCommand {
                reason: "cacheable command must clear inherited environment",
                ..
            })
        ));

        let mut outputless = draft(false);
        outputless
            .actions
            .push(cacheable(CommandEnvironmentPolicy::Clear, Vec::new()));
        assert!(matches!(
            BuildPlan::freeze(outputless),
            Err(PlanError::InvalidCommand {
                reason: "cacheable command must declare an output",
                ..
            })
        ));
    }

    #[test]
    fn freeze_requires_command_path_arguments_to_match_declarations() {
        let mut value = draft(false);
        let input =
            LogicalPath::new(LogicalPathRoot::Package(PackageKey::root()), "input.txt").unwrap();
        let output = LogicalPath::new(LogicalPathRoot::Build, "output.txt").unwrap();
        value.actions.push(PlanAction {
            key: action_key("tool"),
            kind: ActionKind::ExternalCommand {
                resource_class: ActionResourceClass::Cpu,
                environment_policy: CommandEnvironmentPolicy::Inherit,
                cache_policy: CommandCachePolicy::Uncacheable,
                program: CommandProgram::Search("tool".to_string()),
                arguments: vec![
                    CommandArgument::InputPath(input),
                    CommandArgument::OutputPath(output.clone()),
                ],
                working_directory: LogicalPath::new(
                    LogicalPathRoot::Package(PackageKey::root()),
                    "",
                )
                .unwrap(),
                environment: Vec::new(),
                inputs: Vec::new(),
                outputs: vec![output],
            },
        });

        assert!(matches!(
            BuildPlan::freeze(value),
            Err(PlanError::InvalidCommand {
                reason: "input argument path is not declared as an input",
                ..
            })
        ));
    }

    #[test]
    fn freeze_rejects_unbound_command_outputs_and_accepts_multiple_outputs() {
        let output = LogicalPath::new(LogicalPathRoot::Build, "first.txt").unwrap();
        let mut unbound = draft(false);
        unbound.actions.push(PlanAction {
            key: action_key("unbound"),
            kind: ActionKind::ExternalCommand {
                resource_class: ActionResourceClass::Conservative,
                environment_policy: CommandEnvironmentPolicy::Inherit,
                cache_policy: CommandCachePolicy::Uncacheable,
                program: CommandProgram::Search("tool".to_string()),
                arguments: Vec::new(),
                working_directory: LogicalPath::new(
                    LogicalPathRoot::Package(PackageKey::root()),
                    "",
                )
                .unwrap(),
                environment: Vec::new(),
                inputs: Vec::new(),
                outputs: vec![output.clone()],
            },
        });
        assert!(matches!(
            BuildPlan::freeze(unbound),
            Err(PlanError::InvalidCommand {
                reason: "declared output has no staged command argument",
                ..
            })
        ));

        let second = LogicalPath::new(LogicalPathRoot::Build, "second.txt").unwrap();
        let mut multiple = draft(false);
        multiple.actions.push(PlanAction {
            key: action_key("multiple"),
            kind: ActionKind::ExternalCommand {
                resource_class: ActionResourceClass::Io,
                environment_policy: CommandEnvironmentPolicy::Clear,
                cache_policy: CommandCachePolicy::DeclaredInputs,
                program: CommandProgram::Search("tool".to_string()),
                arguments: vec![
                    CommandArgument::OutputPath(output.clone()),
                    CommandArgument::OutputPath(second.clone()),
                ],
                working_directory: LogicalPath::new(
                    LogicalPathRoot::Package(PackageKey::root()),
                    "",
                )
                .unwrap(),
                environment: Vec::new(),
                inputs: Vec::new(),
                outputs: vec![output, second],
            },
        });
        let plan = BuildPlan::freeze(multiple).unwrap();
        let multiple = plan
            .actions()
            .iter()
            .find(|action| action.key.name() == "multiple")
            .unwrap();
        let ActionKind::ExternalCommand { outputs, .. } = &multiple.kind else {
            panic!("expected external command action");
        };
        assert_eq!(outputs.len(), 2);
    }

    fn build_input_draft(
        generated_output: &str,
        command_input: &str,
        command_dependencies: Vec<&str>,
    ) -> BuildPlanDraft {
        let mut value = draft(false);
        let generated = LogicalPath::new(LogicalPathRoot::Build, generated_output).unwrap();
        let input = LogicalPath::new(LogicalPathRoot::Build, command_input).unwrap();
        value.actions.extend([
            PlanAction {
                key: action_key("generate"),
                kind: ActionKind::GeneratedFile {
                    output: generated,
                    contents: b"input".to_vec(),
                },
            },
            PlanAction {
                key: action_key("consume"),
                kind: ActionKind::ExternalCommand {
                    resource_class: ActionResourceClass::Io,
                    environment_policy: CommandEnvironmentPolicy::Inherit,
                    cache_policy: CommandCachePolicy::Uncacheable,
                    program: CommandProgram::Search("tool".to_string()),
                    arguments: vec![CommandArgument::InputPath(input.clone())],
                    working_directory: LogicalPath::new(
                        LogicalPathRoot::Package(PackageKey::root()),
                        "",
                    )
                    .unwrap(),
                    environment: Vec::new(),
                    inputs: vec![input],
                    outputs: Vec::new(),
                },
            },
        ]);
        value.steps.extend([
            PlanStep {
                key: step_key("generate"),
                action: action_key("generate"),
                dependencies: Vec::new(),
            },
            PlanStep {
                key: step_key("consume"),
                action: action_key("consume"),
                dependencies: command_dependencies.into_iter().map(step_key).collect(),
            },
        ]);
        value
    }

    #[test]
    fn freeze_requires_a_producer_for_build_rooted_command_inputs() {
        let value = build_input_draft("generated/other.txt", "generated/input.txt", vec![]);

        assert!(matches!(
            BuildPlan::freeze(value),
            Err(PlanError::MissingBuildInputProducer { path, .. })
                if path.protocol_path() == "generated/input.txt"
        ));
    }

    #[test]
    fn freeze_requires_build_input_producers_in_the_consumer_closure() {
        let value = build_input_draft("generated/input.txt", "generated/input.txt", vec![]);

        assert!(matches!(
            BuildPlan::freeze(value),
            Err(PlanError::BuildInputProducerOutsideClosure { producer, .. })
                if producer.name() == "generate"
        ));
    }

    #[test]
    fn freeze_rejects_descendants_of_file_build_outputs() {
        let value = build_input_draft("generated", "generated/input.txt", vec!["generate"]);

        assert!(matches!(
            BuildPlan::freeze(value),
            Err(PlanError::MissingBuildInputProducer { path, .. })
                if path.protocol_path() == "generated/input.txt"
        ));
    }

    #[test]
    fn freeze_accepts_descendants_of_object_set_outputs() {
        let mut value = build_input_draft("unused", "objects/app/member.o", vec!["emit"]);
        value
            .actions
            .retain(|action| action.key.name() != "generate");
        value.steps.retain(|step| step.key.name() != "generate");
        value.artifacts[0].kind = PlanArtifactKind::ObjectSet;
        value.artifacts[0].output =
            LogicalPath::new(LogicalPathRoot::Build, "objects/app").unwrap();

        assert!(BuildPlan::freeze(value).is_ok());
    }

    #[test]
    fn freeze_requires_generated_build_programs_in_the_consumer_closure() {
        let mut value = build_input_draft("tools/generated", "unused/input.txt", vec![]);
        let consume = value
            .actions
            .iter_mut()
            .find(|action| action.key.name() == "consume")
            .unwrap();
        let ActionKind::ExternalCommand {
            program,
            arguments,
            inputs,
            ..
        } = &mut consume.kind
        else {
            panic!("expected external command action");
        };
        *program = CommandProgram::Path(
            LogicalPath::new(LogicalPathRoot::Build, "tools/generated").unwrap(),
        );
        arguments.clear();
        inputs.clear();

        assert!(matches!(
            BuildPlan::freeze(value),
            Err(PlanError::BuildInputProducerOutsideClosure { producer, .. })
                if producer.name() == "generate"
        ));
    }

    fn build_working_directory_draft(
        produced_directory: &str,
        working_directory: &str,
        command_dependencies: Vec<&str>,
    ) -> BuildPlanDraft {
        let mut value = draft(false);
        value.artifacts[0].kind = PlanArtifactKind::ObjectSet;
        value.artifacts[0].output =
            LogicalPath::new(LogicalPathRoot::Build, produced_directory).unwrap();
        value.actions.push(PlanAction {
            key: action_key("consume"),
            kind: ActionKind::ExternalCommand {
                resource_class: ActionResourceClass::Io,
                environment_policy: CommandEnvironmentPolicy::Inherit,
                cache_policy: CommandCachePolicy::Uncacheable,
                program: CommandProgram::Search("tool".to_string()),
                arguments: Vec::new(),
                working_directory: LogicalPath::new(LogicalPathRoot::Build, working_directory)
                    .unwrap(),
                environment: Vec::new(),
                inputs: Vec::new(),
                outputs: Vec::new(),
            },
        });
        value.steps.push(PlanStep {
            key: step_key("consume"),
            action: action_key("consume"),
            dependencies: command_dependencies.into_iter().map(step_key).collect(),
        });
        value
    }

    #[test]
    fn freeze_requires_a_producer_for_build_rooted_working_directories() {
        let value =
            build_working_directory_draft("generated/other", "generated/work", vec!["emit"]);

        assert!(matches!(
            BuildPlan::freeze(value),
            Err(PlanError::MissingBuildInputProducer { path, .. })
                if path.protocol_path() == "generated/work"
        ));
    }

    #[test]
    fn freeze_requires_working_directory_producers_in_the_consumer_closure() {
        let value = build_working_directory_draft("generated/work", "generated/work", vec![]);

        assert!(matches!(
            BuildPlan::freeze(value),
            Err(PlanError::BuildInputProducerOutsideClosure { path, producer, .. })
                if path.protocol_path() == "generated/work" && producer.name() == "emit"
        ));
    }

    #[test]
    fn freeze_accepts_object_set_outputs_as_working_directories() {
        let value = build_working_directory_draft("generated/work", "generated/work", vec!["emit"]);

        assert!(BuildPlan::freeze(value).is_ok());
    }

    #[test]
    fn freeze_accepts_the_invocation_build_root_as_a_working_directory() {
        let value = build_working_directory_draft("generated/work", "", vec![]);

        assert!(BuildPlan::freeze(value).is_ok());
    }

    #[test]
    fn artifact_program_requires_its_emit_step_in_the_dependency_closure() {
        let mut value = draft(false);
        let artifact = artifact_key("app");
        let run_action = action_key("run");
        let run_step = step_key("run");
        value.actions.push(PlanAction {
            key: run_action.clone(),
            kind: ActionKind::ExternalCommand {
                resource_class: ActionResourceClass::Conservative,
                environment_policy: CommandEnvironmentPolicy::Inherit,
                cache_policy: CommandCachePolicy::Uncacheable,
                program: CommandProgram::Path(
                    LogicalPath::new(LogicalPathRoot::Artifact(artifact), "").unwrap(),
                ),
                arguments: vec![CommandArgument::Literal("argument".to_string())],
                working_directory: LogicalPath::new(
                    LogicalPathRoot::Package(PackageKey::root()),
                    "",
                )
                .unwrap(),
                environment: Vec::new(),
                inputs: Vec::new(),
                outputs: Vec::new(),
            },
        });
        value.steps.push(PlanStep {
            key: run_step.clone(),
            action: run_action,
            dependencies: Vec::new(),
        });
        value.default_step = Some(run_step.clone());

        assert!(matches!(
            BuildPlan::freeze(value.clone()),
            Err(PlanError::InvalidCommand {
                reason: "artifact program has no compiler emit dependency",
                ..
            })
        ));

        value
            .steps
            .iter_mut()
            .find(|step| step.key == run_step)
            .unwrap()
            .dependencies
            .push(step_key("emit"));
        assert!(BuildPlan::freeze(value).is_ok());
    }

    #[test]
    fn artifact_input_requires_its_emit_step_in_the_dependency_closure() {
        let mut value = draft(false);
        let artifact = artifact_key("app");
        let artifact_input = LogicalPath::new(LogicalPathRoot::Artifact(artifact), "").unwrap();
        let tool_action = action_key("tool");
        let tool_step = step_key("tool");
        value.actions.push(PlanAction {
            key: tool_action.clone(),
            kind: ActionKind::ExternalCommand {
                resource_class: ActionResourceClass::Io,
                environment_policy: CommandEnvironmentPolicy::Inherit,
                cache_policy: CommandCachePolicy::Uncacheable,
                program: CommandProgram::Search("tool".to_string()),
                arguments: vec![CommandArgument::InputPath(artifact_input.clone())],
                working_directory: LogicalPath::new(
                    LogicalPathRoot::Package(PackageKey::root()),
                    "",
                )
                .unwrap(),
                environment: Vec::new(),
                inputs: vec![artifact_input],
                outputs: Vec::new(),
            },
        });
        value.steps.push(PlanStep {
            key: tool_step.clone(),
            action: tool_action,
            dependencies: Vec::new(),
        });
        value.default_step = Some(tool_step.clone());

        assert!(matches!(
            BuildPlan::freeze(value.clone()),
            Err(PlanError::InvalidCommand {
                reason: "artifact input has no compiler emit dependency",
                ..
            })
        ));

        value
            .steps
            .iter_mut()
            .find(|step| step.key == tool_step)
            .unwrap()
            .dependencies
            .push(step_key("emit"));
        assert!(BuildPlan::freeze(value).is_ok());
    }

    #[test]
    fn artifact_inputs_require_a_declared_artifact() {
        let mut value = draft(false);
        let missing = artifact_key("missing");
        let input = LogicalPath::new(LogicalPathRoot::Artifact(missing.clone()), "").unwrap();
        value.actions.push(PlanAction {
            key: action_key("consume"),
            kind: ActionKind::ExternalCommand {
                resource_class: ActionResourceClass::Io,
                environment_policy: CommandEnvironmentPolicy::Inherit,
                cache_policy: CommandCachePolicy::Uncacheable,
                program: CommandProgram::Search("tool".to_string()),
                arguments: vec![CommandArgument::InputPath(input.clone())],
                working_directory: LogicalPath::new(
                    LogicalPathRoot::Package(PackageKey::root()),
                    "",
                )
                .unwrap(),
                environment: Vec::new(),
                inputs: vec![input],
                outputs: Vec::new(),
            },
        });
        value.steps.push(PlanStep {
            key: step_key("consume"),
            action: action_key("consume"),
            dependencies: Vec::new(),
        });

        assert!(matches!(
            BuildPlan::freeze(value),
            Err(PlanError::MissingArtifact { artifact, .. }) if artifact == missing
        ));
    }

    #[test]
    fn artifact_programs_must_be_executable() {
        let mut value = draft(false);
        value.artifacts[0].kind = PlanArtifactKind::ObjectSet;
        value.actions.push(PlanAction {
            key: action_key("run"),
            kind: ActionKind::ExternalCommand {
                resource_class: ActionResourceClass::Conservative,
                environment_policy: CommandEnvironmentPolicy::Inherit,
                cache_policy: CommandCachePolicy::Uncacheable,
                program: CommandProgram::Path(
                    LogicalPath::new(LogicalPathRoot::Artifact(artifact_key("app")), "").unwrap(),
                ),
                arguments: Vec::new(),
                working_directory: LogicalPath::new(
                    LogicalPathRoot::Package(PackageKey::root()),
                    "",
                )
                .unwrap(),
                environment: Vec::new(),
                inputs: Vec::new(),
                outputs: Vec::new(),
            },
        });
        // Invalid declarations are rejected even when no step exposes them.
        assert!(matches!(
            BuildPlan::freeze(value),
            Err(PlanError::InvalidArtifactUse { reason, .. })
                if reason == "external command programs must be executable artifacts"
        ));
    }

    fn artifact_working_directory_draft(
        kind: PlanArtifactKind,
        path: &str,
        dependencies: Vec<&str>,
    ) -> BuildPlanDraft {
        let mut value = draft(false);
        value.artifacts[0].kind = kind;
        value.actions.push(PlanAction {
            key: action_key("inspect"),
            kind: ActionKind::ExternalCommand {
                resource_class: ActionResourceClass::Io,
                environment_policy: CommandEnvironmentPolicy::Inherit,
                cache_policy: CommandCachePolicy::Uncacheable,
                program: CommandProgram::Search("tool".to_string()),
                arguments: Vec::new(),
                working_directory: LogicalPath::new(
                    LogicalPathRoot::Artifact(artifact_key("app")),
                    path,
                )
                .unwrap(),
                environment: Vec::new(),
                inputs: Vec::new(),
                outputs: Vec::new(),
            },
        });
        value.steps.push(PlanStep {
            key: step_key("inspect"),
            action: action_key("inspect"),
            dependencies: dependencies.into_iter().map(step_key).collect(),
        });
        value
    }

    #[test]
    fn artifact_working_directories_require_object_sets() {
        let value =
            artifact_working_directory_draft(PlanArtifactKind::Executable, "", vec!["emit"]);

        assert!(matches!(
            BuildPlan::freeze(value),
            Err(PlanError::InvalidArtifactUse { reason, .. })
                if reason == "external command working directories must be object-set artifacts"
        ));
    }

    #[test]
    fn artifact_working_directories_require_the_emit_dependency() {
        let value = artifact_working_directory_draft(PlanArtifactKind::ObjectSet, "", vec![]);

        assert!(matches!(
            BuildPlan::freeze(value),
            Err(PlanError::InvalidCommand { reason, .. })
                if reason == "artifact working directory has no compiler emit dependency"
        ));
    }

    #[test]
    fn artifact_working_directories_must_name_the_artifact_root() {
        let value =
            artifact_working_directory_draft(PlanArtifactKind::ObjectSet, "nested", vec!["emit"]);

        assert!(matches!(
            BuildPlan::freeze(value),
            Err(PlanError::InvalidCommand { reason, .. })
                if reason == "artifact working directory must name the artifact root"
        ));
    }

    #[test]
    fn object_set_working_directories_accept_the_emit_dependency() {
        let value = artifact_working_directory_draft(PlanArtifactKind::ObjectSet, "", vec!["emit"]);

        assert!(BuildPlan::freeze(value).is_ok());
    }
}
