// SPDX-License-Identifier: GPL-3.0-or-later

use std::{
    collections::{BTreeMap, BTreeSet},
    fmt,
    path::PathBuf,
    sync::Arc,
};

use nia_driver::{
    CheckRequest, Driver, DriverConfig, DriverError, LinkExecutableRequest, ModuleMap,
    NiaOptimizationLevel, Runtime as DriverRuntime, SourcePath,
};
use nia_target_config::TargetConfig;

use crate::{
    ActionKey, ActionKind, ArtifactKey, BuildInvocation, BuildPlan, LogicalPath, LogicalPathRoot,
    ModuleKey, OptimizationMode, PackageKey, PlanAction, PlanArtifact, PlanModule, Runtime,
    StepKey, TargetSpec,
};

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ExecutionReport {
    pub steps: Vec<StepKey>,
    pub actions: Vec<ActionKey>,
}

#[derive(Debug)]
pub struct TargetMismatch {
    pub role: &'static str,
    pub expected: TargetSpec,
    pub found: TargetSpec,
}

#[derive(Debug)]
pub struct InvalidModuleImport {
    pub action: ActionKey,
    pub module: ModuleKey,
    pub name: String,
    pub reason: String,
}

#[derive(Debug)]
pub enum CoordinatorError {
    TargetMismatch(Box<TargetMismatch>),
    InconsistentPlan {
        owner: String,
        missing: String,
    },
    UnmappedPackage {
        action: ActionKey,
        package: PackageKey,
    },
    InvalidModuleImport(Box<InvalidModuleImport>),
    NonUtf8Path {
        action: ActionKey,
        path: PathBuf,
    },
    UnsupportedAction {
        action: ActionKey,
        kind: &'static str,
    },
    Driver {
        action: ActionKey,
        error: Box<DriverError>,
    },
}

impl fmt::Display for CoordinatorError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::TargetMismatch(details) => {
                let TargetMismatch {
                    role,
                    expected,
                    found,
                } = details.as_ref();
                write!(
                    f,
                    "build plan {role} target does not match the invocation: expected {}, found {}",
                    display_target(expected),
                    display_target(found)
                )
            }
            Self::InconsistentPlan { owner, missing } => {
                write!(
                    f,
                    "frozen build plan is inconsistent: {owner} references missing {missing}"
                )
            }
            Self::UnmappedPackage { action, package } => write!(
                f,
                "build action `{}` in package `{}` uses package `{}` without a resolved root",
                action.name(),
                action.package().as_str(),
                package.as_str()
            ),
            Self::InvalidModuleImport(details) => {
                let InvalidModuleImport {
                    action,
                    module,
                    name,
                    reason,
                } = details.as_ref();
                write!(
                    f,
                    "build action `{}` cannot map import `{name}` for module `{}`: {reason}",
                    action.name(),
                    module.name()
                )
            }
            Self::NonUtf8Path { action, path } => write!(
                f,
                "build action `{}` resolved non-UTF-8 path `{}`",
                action.name(),
                path.display()
            ),
            Self::UnsupportedAction { action, kind } => write!(
                f,
                "build action `{}` uses unsupported coordinator action kind `{kind}`",
                action.name()
            ),
            Self::Driver { action, error } => write!(
                f,
                "compiler action `{}` in package `{}` failed\n{}",
                action.name(),
                action.package().as_str(),
                nia_driver::render_driver_error(error, None, None)
            ),
        }
    }
}

impl std::error::Error for CoordinatorError {}

pub fn execute_build_plan(
    plan: &BuildPlan,
    invocation: &BuildInvocation,
) -> Result<ExecutionReport, CoordinatorError> {
    validate_invocation_targets(plan, invocation)?;
    let mut executor = DriverActionExecutor::new(plan, invocation);
    execute_selected_closure(plan, |action| executor.execute(action))
}

fn execute_selected_closure(
    plan: &BuildPlan,
    mut execute: impl FnMut(&PlanAction) -> Result<(), CoordinatorError>,
) -> Result<ExecutionReport, CoordinatorError> {
    let Some(selected) = plan.selected_step().or_else(|| plan.default_step()) else {
        return Ok(ExecutionReport {
            steps: Vec::new(),
            actions: Vec::new(),
        });
    };

    let steps = plan.steps();
    let mut closure = BTreeSet::new();
    let selected_index = find_step(steps, selected)
        .ok_or_else(|| inconsistent("plan selection", format!("step `{}`", selected.name())))?;
    let mut pending = vec![selected_index];
    while let Some(index) = pending.pop() {
        if !closure.insert(index) {
            continue;
        }
        for dependency in &steps[index].dependencies {
            pending.push(find_step(steps, dependency).ok_or_else(|| {
                inconsistent(
                    format!("step `{}`", steps[index].key.name()),
                    format!("dependency step `{}`", dependency.name()),
                )
            })?);
        }
    }

    let mut indegree = vec![0usize; steps.len()];
    let mut dependents = vec![Vec::new(); steps.len()];
    for &index in &closure {
        for dependency in &steps[index].dependencies {
            let dependency_index = find_step(steps, dependency).ok_or_else(|| {
                inconsistent(
                    format!("step `{}`", steps[index].key.name()),
                    format!("dependency step `{}`", dependency.name()),
                )
            })?;
            if closure.contains(&dependency_index) {
                indegree[index] += 1;
                dependents[dependency_index].push(index);
            }
        }
    }

    let mut ready: BTreeSet<_> = closure
        .iter()
        .copied()
        .filter(|index| indegree[*index] == 0)
        .collect();
    let mut executed_actions = BTreeSet::new();
    let mut report = ExecutionReport {
        steps: Vec::with_capacity(closure.len()),
        actions: Vec::new(),
    };

    while let Some(index) = ready.pop_first() {
        let step = &steps[index];
        if executed_actions.insert(step.action.clone()) {
            let action = find_action(plan.actions(), &step.action).ok_or_else(|| {
                inconsistent(
                    format!("step `{}`", step.key.name()),
                    format!("action `{}`", step.action.name()),
                )
            })?;
            execute(action)?;
            report.actions.push(action.key.clone());
        }
        report.steps.push(step.key.clone());
        for &dependent in &dependents[index] {
            indegree[dependent] = indegree[dependent].checked_sub(1).ok_or_else(|| {
                inconsistent(
                    format!("step `{}`", steps[dependent].key.name()),
                    "valid dependency degree".to_string(),
                )
            })?;
            if indegree[dependent] == 0 {
                ready.insert(dependent);
            }
        }
    }

    if report.steps.len() != closure.len() {
        return Err(inconsistent(
            "selected step closure",
            "acyclic dependency order".to_string(),
        ));
    }
    Ok(report)
}

struct DriverActionExecutor<'a> {
    plan: &'a BuildPlan,
    invocation: &'a BuildInvocation,
    drivers: BTreeMap<TargetSpec, Driver>,
}

impl<'a> DriverActionExecutor<'a> {
    fn new(plan: &'a BuildPlan, invocation: &'a BuildInvocation) -> Self {
        Self {
            plan,
            invocation,
            drivers: BTreeMap::new(),
        }
    }

    fn execute(&mut self, action: &PlanAction) -> Result<(), CoordinatorError> {
        match &action.kind {
            ActionKind::Aggregate => Ok(()),
            ActionKind::CompilerCheck {
                module,
                target,
                runtime,
            } => {
                let request = self.check_request(action, module, *runtime)?;
                let driver = self.driver(target);
                driver
                    .check_entry(request)
                    .result
                    .map(|_| ())
                    .map_err(|error| CoordinatorError::Driver {
                        action: action.key.clone(),
                        error: Box::new(error),
                    })
            }
            ActionKind::CompilerEmit { artifact, target } => {
                let artifact = self.artifact(action, artifact)?;
                let request =
                    self.check_request(action, &artifact.root_module, artifact.runtime)?;
                let output = self.resolve_path(action, &artifact.output)?;
                let driver = self.driver(target);
                driver
                    .link_executable(LinkExecutableRequest::new(request, output))
                    .result
                    .map(|_| ())
                    .map_err(|error| CoordinatorError::Driver {
                        action: action.key.clone(),
                        error: Box::new(error),
                    })
            }
            ActionKind::ExternalCommand { .. } => Err(unsupported(action, "external-command")),
            ActionKind::GeneratedFile { .. } => Err(unsupported(action, "generated-file")),
            ActionKind::Uncacheable { .. } => Err(unsupported(action, "uncacheable")),
        }
    }

    fn check_request(
        &self,
        action: &PlanAction,
        module_key: &ModuleKey,
        runtime: Runtime,
    ) -> Result<CheckRequest, CoordinatorError> {
        let module = find_module(self.plan.modules(), module_key).ok_or_else(|| {
            inconsistent(
                format!("action `{}`", action.key.name()),
                format!("module `{}`", module_key.name()),
            )
        })?;
        let entry = self.resolve_source_text(action, &module.root_source)?;
        let mut module_map = ModuleMap::new();
        for import in &module.imports {
            let path = SourcePath::new(self.resolve_source_text(action, &import.path)?);
            module_map
                .try_insert(&import.name, path)
                .map_err(|reason| {
                    CoordinatorError::InvalidModuleImport(Box::new(InvalidModuleImport {
                        action: action.key.clone(),
                        module: module.key.clone(),
                        name: import.name.clone(),
                        reason,
                    }))
                })?;
        }
        Ok(CheckRequest::new(entry)
            .with_module_map(module_map)
            .with_optimization(optimization(module.optimization))
            .with_timings(self.invocation.timings)
            .with_runtime(runtime_mode(runtime)))
    }

    fn artifact(
        &self,
        action: &PlanAction,
        key: &ArtifactKey,
    ) -> Result<&PlanArtifact, CoordinatorError> {
        find_artifact(self.plan.artifacts(), key).ok_or_else(|| {
            inconsistent(
                format!("action `{}`", action.key.name()),
                format!("artifact `{}`", key.name()),
            )
        })
    }

    fn driver(&mut self, target: &TargetSpec) -> &Driver {
        self.drivers.entry(target.clone()).or_insert_with(|| {
            Driver::with_config(
                DriverConfig {
                    artifact_cache_dir: Some(self.invocation.cache_dir.clone()),
                    ..DriverConfig::new(Arc::clone(&self.invocation.toolchain))
                }
                .with_artifact_target(target_config(target)),
            )
        })
    }

    fn resolve_source_text(
        &self,
        action: &PlanAction,
        logical: &LogicalPath,
    ) -> Result<String, CoordinatorError> {
        let path = self.resolve_path(action, logical)?;
        let text = path.to_str().ok_or_else(|| CoordinatorError::NonUtf8Path {
            action: action.key.clone(),
            path: path.clone(),
        })?;
        Ok(text.to_string())
    }

    fn resolve_path(
        &self,
        action: &PlanAction,
        logical: &LogicalPath,
    ) -> Result<PathBuf, CoordinatorError> {
        let mut path = match logical.root() {
            LogicalPathRoot::Package(package) => {
                if package != self.plan.root_package() {
                    return Err(CoordinatorError::UnmappedPackage {
                        action: action.key.clone(),
                        package: package.clone(),
                    });
                }
                self.invocation.package_root.clone()
            }
            LogicalPathRoot::Build => self.invocation.build_dir.clone(),
            LogicalPathRoot::Cache => self.invocation.cache_dir.clone(),
            LogicalPathRoot::Toolchain => self.invocation.toolchain.resource_root().to_path_buf(),
            LogicalPathRoot::Artifact(key) => {
                let artifact = self.artifact(action, key)?;
                self.resolve_path(action, &artifact.output)?
            }
        };
        for component in logical.components() {
            path.push(component);
        }
        Ok(path)
    }
}

fn validate_invocation_targets(
    plan: &BuildPlan,
    invocation: &BuildInvocation,
) -> Result<(), CoordinatorError> {
    for (role, found, expected) in [
        (
            "host",
            plan.host_target(),
            target_spec(invocation.toolchain.host_target()),
        ),
        (
            "artifact",
            plan.artifact_target(),
            target_spec(invocation.toolchain.artifact_target()),
        ),
    ] {
        if found != &expected {
            return Err(CoordinatorError::TargetMismatch(Box::new(TargetMismatch {
                role,
                expected,
                found: found.clone(),
            })));
        }
    }
    Ok(())
}

fn find_step(steps: &[crate::PlanStep], key: &StepKey) -> Option<usize> {
    steps.binary_search_by(|step| step.key.cmp(key)).ok()
}

fn find_action<'a>(actions: &'a [PlanAction], key: &ActionKey) -> Option<&'a PlanAction> {
    actions
        .binary_search_by(|action| action.key.cmp(key))
        .ok()
        .map(|index| &actions[index])
}

fn find_module<'a>(modules: &'a [PlanModule], key: &ModuleKey) -> Option<&'a PlanModule> {
    modules
        .binary_search_by(|module| module.key.cmp(key))
        .ok()
        .map(|index| &modules[index])
}

fn find_artifact<'a>(artifacts: &'a [PlanArtifact], key: &ArtifactKey) -> Option<&'a PlanArtifact> {
    artifacts
        .binary_search_by(|artifact| artifact.key.cmp(key))
        .ok()
        .map(|index| &artifacts[index])
}

fn inconsistent(owner: impl Into<String>, missing: String) -> CoordinatorError {
    CoordinatorError::InconsistentPlan {
        owner: owner.into(),
        missing,
    }
}

fn unsupported(action: &PlanAction, kind: &'static str) -> CoordinatorError {
    CoordinatorError::UnsupportedAction {
        action: action.key.clone(),
        kind,
    }
}

fn optimization(mode: OptimizationMode) -> NiaOptimizationLevel {
    match mode {
        OptimizationMode::O0 => NiaOptimizationLevel::O0,
        OptimizationMode::O1 => NiaOptimizationLevel::O1,
        OptimizationMode::O2 => NiaOptimizationLevel::O2,
        OptimizationMode::O3 => NiaOptimizationLevel::O3,
        OptimizationMode::Os => NiaOptimizationLevel::Os,
        OptimizationMode::Oz => NiaOptimizationLevel::Oz,
    }
}

fn runtime_mode(runtime: Runtime) -> DriverRuntime {
    match runtime {
        Runtime::Bare => DriverRuntime::Bare,
        Runtime::Freestanding => DriverRuntime::Freestanding,
    }
}

fn target_spec(target: &TargetConfig) -> TargetSpec {
    TargetSpec {
        arch: target.arch.clone(),
        vendor: target.vendor.clone(),
        os: target.os.clone(),
        env: target.env.clone(),
        abi: target.abi.clone(),
        endian: target.endian.clone(),
        pointer_width: target.pointer_width,
    }
}

fn target_config(target: &TargetSpec) -> TargetConfig {
    TargetConfig {
        arch: target.arch.clone(),
        vendor: target.vendor.clone(),
        os: target.os.clone(),
        env: target.env.clone(),
        abi: target.abi.clone(),
        endian: target.endian.clone(),
        pointer_width: target.pointer_width,
    }
}

fn display_target(target: &TargetSpec) -> String {
    format!(
        "{}-{}-{}-{}-{} ({}-bit {})",
        target.arch,
        target.vendor,
        target.os,
        target.env,
        target.abi,
        target.pointer_width,
        target.endian
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{BuildPlanDraft, PlanAction, PlanPackage, PlanStep};
    use std::sync::{Arc, OnceLock};

    fn action(name: &str) -> ActionKey {
        ActionKey::new(PackageKey::root(), name).unwrap()
    }

    fn step(name: &str) -> StepKey {
        StepKey::new(PackageKey::root(), name).unwrap()
    }

    fn target() -> TargetSpec {
        TargetSpec {
            arch: "x86_64".to_string(),
            vendor: "unknown".to_string(),
            os: "linux".to_string(),
            env: String::new(),
            abi: String::new(),
            endian: "little".to_string(),
            pointer_width: 64,
        }
    }

    fn test_invocation() -> BuildInvocation {
        static TOOLCHAIN: OnceLock<Arc<nia_toolchain::ToolchainLayout>> = OnceLock::new();
        let toolchain = Arc::clone(TOOLCHAIN.get_or_init(|| {
            let manifest_dir = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
            let workspace = manifest_dir
                .parent()
                .and_then(std::path::Path::parent)
                .expect("nia-build lives under crates");
            Arc::new(
                nia_toolchain::ToolchainLayout::resolve(
                    nia_toolchain::ToolchainLayoutRequest::explicit(
                        std::env::current_exe().expect("test executable"),
                        workspace.join("lib"),
                    ),
                )
                .expect("test toolchain"),
            )
        }));
        let package_root = std::env::temp_dir().join("nia-build-coordinator-test");
        let build_dir = package_root.join(".nia-build");
        BuildInvocation {
            toolchain,
            build_script: package_root.join("build.nia"),
            cache_dir: package_root.join(".nia-cache"),
            runner_dir: build_dir.join("runner"),
            runner_executable: build_dir.join("runner/nia-build-runner"),
            plan_draft: build_dir.join("build-plan.draft"),
            plan_path: build_dir.join("build-plan.bin"),
            build_dir,
            package_root,
            step: crate::BuildStepSelection::Default,
            timings: nia_driver::TimingMode::Off,
            timing_format: nia_timing::TimingFormat::Text,
        }
    }

    fn aggregate_plan(
        actions: &[&str],
        steps: Vec<(&str, &str, Vec<&str>)>,
        selected: &str,
    ) -> BuildPlan {
        BuildPlan::freeze(BuildPlanDraft {
            root_package: PackageKey::root(),
            packages: vec![PlanPackage {
                key: PackageKey::root(),
            }],
            host_target: target(),
            artifact_target: target(),
            modules: Vec::new(),
            artifacts: Vec::new(),
            actions: actions
                .iter()
                .map(|name| PlanAction {
                    key: action(name),
                    kind: ActionKind::Aggregate,
                })
                .collect(),
            steps: steps
                .into_iter()
                .map(|(name, action_name, dependencies)| PlanStep {
                    key: step(name),
                    action: action(action_name),
                    dependencies: dependencies.into_iter().map(step).collect(),
                })
                .collect(),
            default_step: None,
            selected_step: Some(step(selected)),
        })
        .unwrap()
    }

    #[test]
    fn selected_closure_is_iterative_deterministic_and_excludes_unselected_steps() {
        let plan = aggregate_plan(
            &["shared", "left", "right", "final", "unused"],
            vec![
                ("unused", "unused", vec![]),
                ("final", "final", vec!["right", "left"]),
                ("right", "right", vec!["shared"]),
                ("left", "left", vec!["shared"]),
                ("shared", "shared", vec![]),
            ],
            "final",
        );
        let mut observed = Vec::new();

        let report = execute_selected_closure(&plan, |item| {
            observed.push(item.key.name().to_string());
            Ok(())
        })
        .unwrap();

        assert_eq!(observed, ["shared", "left", "right", "final"]);
        assert_eq!(
            report.steps.iter().map(StepKey::name).collect::<Vec<_>>(),
            ["shared", "left", "right", "final"]
        );
    }

    #[test]
    fn shared_action_executes_once_across_multiple_steps() {
        let plan = aggregate_plan(
            &["shared", "final"],
            vec![
                ("a", "shared", vec![]),
                ("b", "shared", vec![]),
                ("final", "final", vec!["a", "b"]),
            ],
            "final",
        );
        let mut observed = Vec::new();

        let report = execute_selected_closure(&plan, |item| {
            observed.push(item.key.name().to_string());
            Ok(())
        })
        .unwrap();

        assert_eq!(observed, ["shared", "final"]);
        assert_eq!(report.steps.len(), 3);
        assert_eq!(report.actions.len(), 2);
    }

    #[test]
    fn action_failure_stops_dependents_and_retains_action_context() {
        let plan = aggregate_plan(
            &["first", "second"],
            vec![
                ("first", "first", vec![]),
                ("second", "second", vec!["first"]),
            ],
            "second",
        );

        let error = execute_selected_closure(&plan, |item| Err(unsupported(item, "test-failure")))
            .unwrap_err();

        assert!(matches!(
            error,
            CoordinatorError::UnsupportedAction { action, kind: "test-failure" }
                if action.name() == "first"
        ));
    }

    #[test]
    fn empty_plan_executes_nothing() {
        let plan = BuildPlan::freeze(BuildPlanDraft {
            root_package: PackageKey::root(),
            packages: vec![PlanPackage {
                key: PackageKey::root(),
            }],
            host_target: target(),
            artifact_target: target(),
            modules: Vec::new(),
            artifacts: Vec::new(),
            actions: Vec::new(),
            steps: Vec::new(),
            default_step: None,
            selected_step: None,
        })
        .unwrap();

        let report = execute_selected_closure(&plan, |_| unreachable!()).unwrap();
        assert!(report.steps.is_empty());
        assert!(report.actions.is_empty());
    }

    #[test]
    fn all_optimization_and_runtime_modes_map_exactly() {
        assert_eq!(optimization(OptimizationMode::O0), NiaOptimizationLevel::O0);
        assert_eq!(optimization(OptimizationMode::O1), NiaOptimizationLevel::O1);
        assert_eq!(optimization(OptimizationMode::O2), NiaOptimizationLevel::O2);
        assert_eq!(optimization(OptimizationMode::O3), NiaOptimizationLevel::O3);
        assert_eq!(optimization(OptimizationMode::Os), NiaOptimizationLevel::Os);
        assert_eq!(optimization(OptimizationMode::Oz), NiaOptimizationLevel::Oz);
        assert_eq!(runtime_mode(Runtime::Bare), DriverRuntime::Bare);
        assert_eq!(
            runtime_mode(Runtime::Freestanding),
            DriverRuntime::Freestanding
        );
    }

    #[test]
    fn explicit_uncacheable_action_has_no_legacy_execution_fallback() {
        let invocation = test_invocation();
        let target = target_spec(invocation.toolchain.host_target());
        let plan = BuildPlan::freeze(BuildPlanDraft {
            root_package: PackageKey::root(),
            packages: vec![PlanPackage {
                key: PackageKey::root(),
            }],
            host_target: target.clone(),
            artifact_target: target,
            modules: Vec::new(),
            artifacts: Vec::new(),
            actions: vec![PlanAction {
                key: action("opaque"),
                kind: ActionKind::Uncacheable {
                    description: "legacy callback".to_string(),
                },
            }],
            steps: vec![PlanStep {
                key: step("opaque"),
                action: action("opaque"),
                dependencies: Vec::new(),
            }],
            default_step: Some(step("opaque")),
            selected_step: None,
        })
        .unwrap();

        let error = execute_build_plan(&plan, &invocation).unwrap_err();
        assert!(matches!(
            error,
            CoordinatorError::UnsupportedAction { action, kind: "uncacheable" }
                if action.name() == "opaque" && action.package().as_str() == "root"
        ));
    }

    #[test]
    fn invocation_target_mismatch_is_rejected_before_actions() {
        let invocation = test_invocation();
        let mut mismatched = target_spec(invocation.toolchain.host_target());
        mismatched.arch = "mismatched".to_string();
        let plan = BuildPlan::freeze(BuildPlanDraft {
            root_package: PackageKey::root(),
            packages: vec![PlanPackage {
                key: PackageKey::root(),
            }],
            host_target: mismatched,
            artifact_target: target_spec(invocation.toolchain.artifact_target()),
            modules: Vec::new(),
            artifacts: Vec::new(),
            actions: Vec::new(),
            steps: Vec::new(),
            default_step: None,
            selected_step: None,
        })
        .unwrap();

        let error = execute_build_plan(&plan, &invocation).unwrap_err();
        assert!(matches!(
            error,
            CoordinatorError::TargetMismatch(details)
                if details.role == "host" && details.found.arch == "mismatched"
        ));
    }
}
