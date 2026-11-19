// SPDX-License-Identifier: GPL-3.0-or-later
//! Read-only projection of configured build-plan inputs for compilers.

use std::path::{Path, PathBuf};

use nia_driver::CheckRequest;
use nia_driver::NiaOptimizationLevel;
use nia_package_metadata::DependencyScope;
use nia_source::SourcePath;
use nia_target::TargetConfig;

use crate::{
    ActionKind, ArtifactConfiguration, ArtifactKey, BuildPlan, CompilationMode, LogicalPath,
    LogicalPathRoot, OptimizationMode, PackageKey, PlanAction, PlanModule, Runtime,
};

/// Physical directories that anchor the plan's logical paths.
pub struct PlanDirectories<'a> {
    /// The configured invocation's build output directory.
    pub build: &'a Path,
    /// The invocation's cache directory.
    pub cache: &'a Path,
    /// The resource directory of the selected toolchain.
    pub toolchain: &'a Path,
}

/// Maps a package key in the frozen plan to its materialized source root.
pub type PackageRoot<'a> = dyn Fn(&PackageKey) -> Option<PathBuf> + 'a;

/// A validated plan path whose owning root cannot be resolved.
#[derive(Debug)]
pub enum PlanPathError {
    /// The owner package has no materialized root.
    Package(PackageKey),
    /// The requested artifact does not occur in the plan.
    Artifact(ArtifactKey),
}

impl PlanDirectories<'_> {
    /// Projects a logical plan path without running build actions or touching the filesystem.
    pub fn resolve(
        &self,
        plan: &BuildPlan,
        path: &LogicalPath,
        package_root: &PackageRoot<'_>,
    ) -> Result<PathBuf, PlanPathError> {
        let mut root = match path.root() {
            LogicalPathRoot::Package(package) => {
                package_root(package).ok_or_else(|| PlanPathError::Package(package.clone()))?
            }
            LogicalPathRoot::Build => self.build.to_path_buf(),
            LogicalPathRoot::Cache => self.cache.to_path_buf(),
            LogicalPathRoot::Toolchain => self.toolchain.to_path_buf(),
            LogicalPathRoot::Artifact(key) => {
                let artifact = plan
                    .artifacts()
                    .binary_search_by(|artifact| artifact.key.cmp(key))
                    .ok()
                    .map(|index| &plan.artifacts()[index])
                    .ok_or_else(|| PlanPathError::Artifact(key.clone()))?;
                self.resolve(plan, &artifact.output, package_root)?
            }
        };
        root.extend(path.components());
        Ok(root)
    }
}

/// Compiler context declared by a check or emit action.
pub struct PlanCompilation<'a> {
    /// Root source and imports of the compiler module.
    pub module: &'a PlanModule,
    /// Compiler target of the action.
    pub target: TargetConfig,
    /// Runtime selected by the action's artifact or check.
    pub runtime: Runtime,
    /// Whether test-only source is enabled.
    pub mode: CompilationMode,
    /// Explicit artifact settings, including any CPU override.
    pub configuration: &'a ArtifactConfiguration,
}

impl<'a> PlanCompilation<'a> {
    /// Looks up the compiler inputs of an action from this validated plan.
    pub fn for_action(plan: &'a BuildPlan, action: &'a PlanAction) -> Option<Self> {
        let (module, target, runtime, mode, configuration) = match &action.kind {
            ActionKind::CompilerCheck {
                module,
                target,
                runtime,
                mode,
                configuration,
            } => (module, *target, *runtime, *mode, configuration),
            ActionKind::CompilerEmit {
                artifact,
                target,
                configuration,
                ..
            } => {
                let index = plan
                    .artifacts()
                    .binary_search_by(|item| item.key.cmp(artifact))
                    .ok()?;
                let artifact = &plan.artifacts()[index];
                (
                    &artifact.root_module,
                    *target,
                    artifact.runtime,
                    artifact.mode,
                    configuration,
                )
            }
            _ => return None,
        };
        let index = plan
            .modules()
            .binary_search_by(|item| item.key.cmp(module))
            .ok()?;
        Some(Self {
            module: &plan.modules()[index],
            target,
            runtime,
            mode,
            configuration,
        })
    }

    /// Creates the request shared by build actions and editor analysis.
    pub fn check_request(&self, entry: SourcePath) -> CheckRequest {
        check_request(entry, self.module, self.mode)
    }
}

pub(crate) fn check_request(
    entry: SourcePath,
    module: &PlanModule,
    mode: CompilationMode,
) -> CheckRequest {
    CheckRequest::from_source_path(entry)
        .with_optimization(optimization(module.optimization))
        .with_compilation_mode(match mode {
            CompilationMode::Normal => nia_target_config::CompilationMode::Normal,
            CompilationMode::Test => nia_target_config::CompilationMode::Test,
        })
}

pub(crate) fn optimization(mode: OptimizationMode) -> NiaOptimizationLevel {
    match mode {
        OptimizationMode::O0 => NiaOptimizationLevel::O0,
        OptimizationMode::O1 => NiaOptimizationLevel::O1,
        OptimizationMode::O2 => NiaOptimizationLevel::O2,
        OptimizationMode::O3 => NiaOptimizationLevel::O3,
        OptimizationMode::Os => NiaOptimizationLevel::Os,
        OptimizationMode::Oz => NiaOptimizationLevel::Oz,
    }
}

/// Whether a package in this scope can appear as an explicit artifact import.
pub fn artifact_dependency_visible(scope: DependencyScope, mode: CompilationMode) -> bool {
    match scope {
        DependencyScope::Build => false,
        DependencyScope::Development => mode == CompilationMode::Test,
        DependencyScope::Runtime | DependencyScope::Optional | DependencyScope::Lazy => true,
    }
}

/// Whether a dependency contributes a module root for these manifest scopes.
pub fn manifest_dependency_visible(
    scope: DependencyScope,
    scopes: &[DependencyScope],
    mode: CompilationMode,
) -> bool {
    match scope {
        DependencyScope::Lazy => scopes.contains(&DependencyScope::Runtime),
        DependencyScope::Development => mode == CompilationMode::Test && scopes.contains(&scope),
        _ => scopes.contains(&scope),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{
        ActionKey, ArtifactConfiguration, ArtifactKey, BuildPlanDraft, ModuleKey, OptimizationMode,
        PackageKey, PlanAction, PlanArtifact, PlanArtifactKind, PlanPackage, PlanPackageSource,
        PlanStep, StepKey, StepVisibility,
    };

    fn plan() -> BuildPlan {
        let package = PackageKey::root();
        let module = ModuleKey::new(package.clone(), "main").unwrap();
        let artifact = ArtifactKey::new(package.clone(), "app").unwrap();
        let check = ActionKey::new(package.clone(), "check").unwrap();
        let emit = ActionKey::new(package.clone(), "emit").unwrap();
        let check_step = StepKey::new(package.clone(), "check").unwrap();
        let emit_step = StepKey::new(package.clone(), "emit").unwrap();
        let target = nia_target::TargetConfig::host().unwrap();
        BuildPlan::freeze(BuildPlanDraft {
            root_package: package.clone(),
            packages: vec![PlanPackage {
                key: package.clone(),
                source: PlanPackageSource::Local(String::new()),
            }],
            host_target: target,
            artifact_target: target,
            modules: vec![PlanModule {
                key: module.clone(),
                root_source: LogicalPath::new(
                    LogicalPathRoot::Package(package.clone()),
                    "src/main.nia",
                )
                .unwrap(),
                optimization: OptimizationMode::O2,
                imports: Vec::new(),
            }],
            artifacts: vec![PlanArtifact {
                key: artifact.clone(),
                root_module: module.clone(),
                kind: PlanArtifactKind::Executable,
                output: LogicalPath::new(LogicalPathRoot::Build, "bin/app").unwrap(),
                runtime: Runtime::Freestanding,
                mode: CompilationMode::Normal,
                configuration: ArtifactConfiguration::default(),
            }],
            actions: vec![
                PlanAction {
                    key: check.clone(),
                    kind: ActionKind::CompilerCheck {
                        module: module.clone(),
                        target,
                        runtime: Runtime::Bare,
                        mode: CompilationMode::Test,
                        configuration: ArtifactConfiguration::default(),
                    },
                },
                PlanAction {
                    key: emit.clone(),
                    kind: ActionKind::CompilerEmit {
                        artifact: artifact.clone(),
                        target,
                        static_archives: Vec::new(),
                        configuration: ArtifactConfiguration::default(),
                    },
                },
            ],
            steps: vec![
                PlanStep {
                    key: check_step.clone(),
                    visibility: StepVisibility::Internal,
                    action: check,
                    dependencies: Vec::new(),
                },
                PlanStep {
                    key: emit_step.clone(),
                    visibility: StepVisibility::Public,
                    action: emit,
                    dependencies: vec![check_step],
                },
            ],
            default_step: Some(emit_step),
            selected_step: None,
        })
        .unwrap()
    }

    #[test]
    fn plan_paths_share_package_build_cache_toolchain_and_artifact_projection() {
        let plan = plan();
        let root = PackageKey::root();
        let directories = PlanDirectories {
            build: Path::new("/work/build/out"),
            cache: Path::new("/work/build/cache"),
            toolchain: Path::new("/toolchain/lib"),
        };
        let package_root = |key: &PackageKey| (key == &root).then(|| PathBuf::from("/work/pkg"));
        for (logical, expected) in [
            (
                LogicalPath::new(LogicalPathRoot::Package(root.clone()), "src/main.nia").unwrap(),
                "/work/pkg/src/main.nia",
            ),
            (
                LogicalPath::new(LogicalPathRoot::Build, "generated/root.nia").unwrap(),
                "/work/build/out/generated/root.nia",
            ),
            (
                LogicalPath::new(LogicalPathRoot::Cache, "objects/a.o").unwrap(),
                "/work/build/cache/objects/a.o",
            ),
            (
                LogicalPath::new(LogicalPathRoot::Toolchain, "std/root.nia").unwrap(),
                "/toolchain/lib/std/root.nia",
            ),
            (
                LogicalPath::new(
                    LogicalPathRoot::Artifact(ArtifactKey::new(PackageKey::root(), "app").unwrap()),
                    "assets/config",
                )
                .unwrap(),
                "/work/build/out/bin/app/assets/config",
            ),
        ] {
            assert_eq!(
                directories.resolve(&plan, &logical, &package_root).unwrap(),
                PathBuf::from(expected)
            );
        }
        let missing = PackageKey::new("missing").unwrap();
        assert!(matches!(
            directories.resolve(
                &plan,
                &LogicalPath::new(LogicalPathRoot::Package(missing), "root.nia").unwrap(),
                &package_root,
            ),
            Err(PlanPathError::Package(_))
        ));
    }

    #[test]
    fn compiler_context_projection_preserves_each_action_configuration() {
        let plan = plan();
        let check = plan
            .actions()
            .iter()
            .find(|action| action.key.name() == "check")
            .unwrap();
        let test = PlanCompilation::for_action(&plan, check).unwrap();
        assert_eq!(test.mode, CompilationMode::Test);
        assert_eq!(test.runtime, Runtime::Bare);
        assert_eq!(
            test.check_request(SourcePath::new("/work/pkg/src/main.nia"))
                .compilation_mode,
            nia_target_config::CompilationMode::Test
        );
        let emit = plan
            .actions()
            .iter()
            .find(|action| action.key.name() == "emit")
            .unwrap();
        let executable = PlanCompilation::for_action(&plan, emit).unwrap();
        assert_eq!(executable.mode, CompilationMode::Normal);
        assert_eq!(executable.runtime, Runtime::Freestanding);
        assert_eq!(executable.module.optimization, OptimizationMode::O2);
    }

    #[test]
    fn artifact_dependency_visibility_matches_compiler_scope_rules() {
        for (scope, normal, test) in [
            (DependencyScope::Build, false, false),
            (DependencyScope::Runtime, true, true),
            (DependencyScope::Development, false, true),
            (DependencyScope::Optional, true, true),
            (DependencyScope::Lazy, true, true),
        ] {
            assert_eq!(
                artifact_dependency_visible(scope, CompilationMode::Normal),
                normal
            );
            assert_eq!(
                artifact_dependency_visible(scope, CompilationMode::Test),
                test
            );
        }
    }
}
