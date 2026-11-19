// SPDX-License-Identifier: GPL-3.0-or-later
//! Package contexts for editor analysis.

use std::{
    collections::{BTreeMap, BTreeSet, HashMap},
    fmt,
    path::{Path, PathBuf},
    sync::{Arc, OnceLock},
};

use nia_build::compiler_inputs::CompilerContext;
use nia_build::compiler_inputs::build_script_path;
use nia_build::{ConfigureManifest, ObservationValidation};
use nia_build_plan::{
    BuildPlan, ModuleKey, PlanConfiguration, PlanHandoffError, read_configured_plan,
};
use nia_driver::CheckRequest;
use nia_imports::ModuleMap;
use nia_lexer::TokenKind;
use nia_package_metadata::{
    ArtifactKind, BuildLayout, DependencyScope, InstalledDependencies, PackageManifest,
    WorkspaceManifest,
};
use nia_source::{SourceIdentity, SourcePath};
use nia_syntax::SyntaxKind;
use nia_target_config::CompilationMode;

use crate::analysis::normalized;

mod configured;

#[derive(Default)]
pub(crate) struct Projects {
    packages: HashMap<PathBuf, Package>,
}

#[derive(PartialEq, Eq)]
pub(crate) struct WorkspaceRequests {
    pub(crate) entries: Vec<EditorRequest>,
    pub(crate) build_candidates: Vec<EditorRequest>,
    pub(crate) standalone: Vec<EditorRequest>,
    pub(crate) editable: BTreeSet<PathBuf>,
    pub(crate) managed: BTreeSet<PathBuf>,
    pub(crate) unavailable_plans: Vec<(PathBuf, String)>,
    pub(crate) package_names: BTreeMap<PathBuf, String>,
    pub(crate) plans: BTreeMap<PathBuf, Arc<(BuildPlan, PlanConfiguration)>>,
}

#[derive(Clone, PartialEq, Eq)]
pub(crate) struct EditorRequest {
    pub(crate) check: CheckRequest,
    pub(crate) context: CompilerContext,
    module: Option<ModuleKey>,
    manifest_context: Option<Context>,
}

impl EditorRequest {
    pub(crate) fn standalone(path: &Path, toolchain: &nia_toolchain::ToolchainLayout) -> Self {
        Self::manifest(
            CheckRequest::new(normalized(path)),
            *toolchain.artifact_target(),
        )
    }

    fn manifest(check: CheckRequest, target: nia_target::TargetConfig) -> Self {
        let context = CompilerContext {
            target,
            cpu: nia_target::CpuLevel::default(),
            runtime: nia_build_plan::Runtime::Bare,
            profile: check.profile,
            mode: match check.compilation_mode {
                CompilationMode::Normal => nia_build_plan::CompilationMode::Normal,
                CompilationMode::Test => nia_build_plan::CompilationMode::Test,
            },
        };
        Self {
            check,
            context,
            module: None,
            manifest_context: None,
        }
    }

    pub(crate) fn with_manifest_edits(
        &self,
        projects: &mut Projects,
        edits: &BTreeMap<PathBuf, String>,
    ) -> Result<Self, String> {
        let Some(context) = self.manifest_context else {
            if self.module.is_some() {
                return Err(
                    "An affected build plan must be regenerated to change its dependencies".into(),
                );
            }
            return Ok(self.clone());
        };
        let entry = Path::new(self.check.entry_path.as_str());
        let package = projects
            .package(entry)?
            .ok_or("Compilation package is unavailable")?;
        let mut request = self.clone();
        request.check.module_map = package.module_map_with_manifests(context, edits)?;
        Ok(request)
    }
}

impl fmt::Display for EditorRequest {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let context = self.context;
        write!(f, "`{}` (", self.check.entry_path.as_str())?;
        if let Some(module) = &self.module {
            write!(
                f,
                "module `{}::{}`, ",
                module.package().as_str(),
                module.name()
            )?;
        }
        write!(
            f,
            "target {}, CPU {}, {:?} runtime, {:?} profile, {:?} mode)",
            context.target, context.cpu, context.runtime, context.profile, context.mode,
        )
    }
}

impl Projects {
    pub(crate) fn workspace_requests(
        &mut self,
        entry: Option<&Path>,
        open: impl Iterator<Item = PathBuf>,
        folders: &[PathBuf],
        toolchain: &nia_toolchain::ToolchainLayout,
    ) -> Result<WorkspaceRequests, String> {
        let mut entries = BTreeSet::new();
        let mut standalone = BTreeSet::new();
        let mut roots = BTreeSet::new();
        let mut editable = BTreeSet::new();
        let mut managed = BTreeSet::new();
        let mut configured = Vec::new();
        let mut build_candidates = Vec::new();
        let mut unavailable_plans = Vec::new();
        let mut package_names = BTreeMap::new();
        let mut plans = BTreeMap::new();
        for folder in folders {
            if folder.join("nia.toml").is_file() {
                roots.insert(folder.canonicalize().map_err(|error| error.to_string())?);
            }
        }
        for path in entry.map(Path::to_path_buf).into_iter().chain(open) {
            standalone.insert(PathBuf::from(normalized(&path)));
            let Some(package) = self.package(&path)? else {
                if let Some(parent) = path.parent() {
                    editable.insert(PathBuf::from(normalized(parent)));
                }
                continue;
            };
            roots.insert(package.layout.state_root().to_path_buf());
            roots.insert(package.layout.package_root().to_path_buf());
        }
        let mut visited = BTreeSet::new();
        while let Some(root) = roots.pop_first() {
            if !visited.insert(root.clone()) {
                continue;
            }
            if let Some(workspace) =
                WorkspaceManifest::discover(&root).map_err(|error| error.to_string())?
                && root == workspace.root
            {
                let members = workspace
                    .member_packages()
                    .map_err(|error| error.to_string())?;
                let is_package = members.iter().any(|member| member.root == root);
                roots.extend(members.into_iter().map(|member| member.root));
                if !is_package {
                    continue;
                }
            }
            let package = self
                .package(&root.join("nia.toml"))?
                .ok_or("Workspace package disappeared")?;
            let package_root = package.source_root();
            package_names.insert(package_root.clone(), package.manifest.package.name.clone());
            if let Some(plan) = &package.plan {
                plans.insert(package_root.clone(), plan.clone());
            }
            editable.insert(PathBuf::from(normalized(package.layout.package_root())));
            managed.extend(
                package
                    .layout
                    .managed_directories()
                    .map(|path| PathBuf::from(normalized(&path))),
            );
            if let Some(build) = package.build_candidate() {
                build_candidates.push(package.build_request(&build, toolchain)?);
            }
            let planned = match package.configured_requests(toolchain) {
                Ok(requests) => requests,
                Err(error) => {
                    unavailable_plans.push((package.layout.package_root().to_path_buf(), error));
                    Vec::new()
                }
            };
            entries.extend(
                package
                    .declared_entries()
                    .into_iter()
                    .filter(|(path, context)| {
                        let path = normalized(path);
                        !planned.iter().any(|request| {
                            request.check.entry_path.as_str() == path
                                && (request.check.compilation_mode == CompilationMode::Test)
                                    == (*context == Context::Test)
                        })
                    }),
            );
            configured.extend(planned);
        }
        let mut entries = entries
            .into_iter()
            .map(|(path, context)| (PathBuf::from(normalized(&path)), context))
            .collect::<BTreeSet<_>>()
            .into_iter()
            .map(|(path, context)| match self.package(&path)? {
                Some(package) => package.editor_request(&path, context, toolchain),
                None => Ok(EditorRequest::standalone(&path, toolchain)),
            })
            .collect::<Result<Vec<_>, _>>()?;
        entries.extend(configured);
        let standalone = standalone
            .into_iter()
            .map(|path| match self.package(&path)? {
                Some(package) => {
                    package.editor_request(&path, package.fallback_context(&path), toolchain)
                }
                None => Ok(EditorRequest::standalone(&path, toolchain)),
            })
            .collect::<Result<_, _>>()?;
        Ok(WorkspaceRequests {
            entries,
            build_candidates,
            standalone,
            editable,
            managed,
            unavailable_plans,
            package_names,
            plans,
        })
    }

    pub(crate) fn package(&mut self, entry: &Path) -> Result<Option<&mut Package>, String> {
        let Some(mut root) = entry
            .ancestors()
            .skip(1)
            .find(|directory| {
                directory.join("nia.toml").is_file() || build_script_path(directory).is_file()
            })
            .map(Path::to_path_buf)
        else {
            return Ok(None);
        };
        let output = root.join("build/out");
        if entry.starts_with(&output)
            && let Some(workspace) =
                WorkspaceManifest::discover(&root).map_err(|error| error.to_string())?
        {
            for member in workspace
                .member_packages()
                .map_err(|error| error.to_string())?
            {
                let layout =
                    BuildLayout::for_package(&member.root).map_err(|error| error.to_string())?;
                if entry.starts_with(Path::new(&normalized(&layout.out_dir()))) {
                    root = layout.package_root().to_path_buf();
                    break;
                }
            }
        }
        let root = PathBuf::from(normalized(&root));
        if !self.packages.contains_key(&root) {
            let package = Package::load(&root)?;
            self.packages.insert(root.clone(), package);
        }
        Ok(self.packages.get_mut(&root))
    }
}

pub(crate) struct Package {
    layout: BuildLayout,
    manifest: PackageManifest,
    maps: HashMap<Context, ModuleMap>,
    plan: Option<Arc<(BuildPlan, PlanConfiguration)>>,
    configuration: ConfigurationStatus,
    planned_requests: OnceLock<Result<Vec<EditorRequest>, String>>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) enum ConfigurationStatus {
    Unconfigured,
    Missing,
    Ready,
    Corrupt(String),
    Stale(String),
}

impl ConfigurationStatus {
    pub(crate) fn message(&self) -> Option<String> {
        match self {
            Self::Unconfigured | Self::Ready => None,
            Self::Missing => Some(
                "The build script has no configured build plan. Using manifest entries until `nia build` publishes the plan."
                    .to_owned(),
            ),
            Self::Corrupt(detail) => Some(format!(
                "Configured build plan is corrupt. Using manifest entries. {detail}"
            )),
            Self::Stale(detail) => Some(format!(
                "Configured build plan is stale. Using manifest entries. {detail}"
            )),
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, Hash, Ord, PartialEq, PartialOrd)]
pub(crate) enum Context {
    Library,
    Executable,
    Test,
    Build,
}

impl Package {
    pub(crate) fn generated_origin(
        &self,
        generated: &Path,
        text: &str,
        span: nia_span::Span,
        load: impl FnOnce(&Path) -> std::io::Result<std::sync::Arc<str>>,
    ) -> Option<nia_build::source_map::SourceOrigin> {
        let source_map = self.generated_source_map(generated, text).ok()??;
        source_map
            .origin(self.layout.package_root(), &self.output_root(), span, load)
            .ok()?
    }

    pub(crate) fn generated_source_map<'a>(
        &self,
        generated: &Path,
        text: &'a str,
    ) -> std::io::Result<Option<nia_build::source_map::GeneratedSourceMap<'a>>> {
        let Some((plan, _)) = self.plan.as_deref() else {
            return Ok(None);
        };
        nia_build::source_map::GeneratedSourceMap::read(plan, &self.output_root(), generated, text)
    }

    pub(crate) fn source_root(&self) -> PathBuf {
        PathBuf::from(normalized(self.layout.package_root()))
    }

    pub(crate) fn output_root(&self) -> PathBuf {
        PathBuf::from(normalized(&self.layout.out_dir()))
    }

    pub(crate) fn generated_outputs_reading(&self, source: &Path) -> Vec<PathBuf> {
        let Some((plan, _)) = self.plan.as_deref() else {
            return Vec::new();
        };
        let Some(relative) = source.strip_prefix(self.source_root()).ok().map(normalized) else {
            return Vec::new();
        };
        nia_build::source_map::outputs_reading_input(plan, &self.output_root(), &relative)
    }

    fn build_candidate(&self) -> Option<PathBuf> {
        let source = build_script_path(self.layout.package_root());
        source.is_file().then_some(source)
    }

    fn fallback_context(&self, path: &Path) -> Context {
        if normalized(path) == normalized(&build_script_path(self.layout.package_root())) {
            Context::Build
        } else {
            Context::Executable
        }
    }

    fn declared_entries(&self) -> Vec<(PathBuf, Context)> {
        let root = self.layout.package_root();
        let mut entries = Vec::new();
        if let Some(library) = &self.manifest.library_path {
            entries.push((root.join(library), Context::Library));
        }
        entries.extend(self.manifest.artifacts.iter().map(|artifact| {
            let context = match artifact.kind {
                ArtifactKind::Executable => Context::Executable,
                ArtifactKind::Test => Context::Test,
            };
            (root.join(&artifact.path), context)
        }));
        entries
    }
    fn load(root: &Path) -> Result<Self, String> {
        let error = |error: std::io::Error| format!("{}: {error}", root.join("nia.toml").display());
        let layout = BuildLayout::for_package(root).map_err(error)?;
        let manifest = PackageManifest::read(layout.package_root()).map_err(error)?;
        let plan_path = layout.out_dir().join("build-plan.bin");
        let (plan, configuration) = match read_configured_plan(&plan_path) {
            Ok((draft, config)) => match ConfigureManifest::decode(&draft.observations) {
                Err(error) => (
                    None,
                    ConfigurationStatus::Corrupt(format!(
                        "Invalid observations in `{}`: {error:?}",
                        plan_path.display()
                    )),
                ),
                Ok(observations) => match observations.validate_filesystem(layout.package_root()) {
                    ObservationValidation::Match => (
                        Some(Arc::new((draft.plan, config))),
                        ConfigurationStatus::Ready,
                    ),
                    ObservationValidation::Changed { path, kind } => (
                        None,
                        ConfigurationStatus::Stale(format!(
                            "The {kind} observation for `{}` has changed.",
                            path.display()
                        )),
                    ),
                    ObservationValidation::Missing { path, kind } => (
                        None,
                        ConfigurationStatus::Stale(format!(
                            "The {kind} observation for `{}` could not be read.",
                            path.display()
                        )),
                    ),
                },
            },
            Err(PlanHandoffError::Open { error, .. })
                if error.kind() == std::io::ErrorKind::NotFound =>
            {
                let build_script = build_script_path(layout.package_root());
                let status = if nia_source::read_source_text(&build_script)
                    .is_ok_and(|text| declares_build_hook(&text))
                {
                    ConfigurationStatus::Missing
                } else {
                    ConfigurationStatus::Unconfigured
                };
                (None, status)
            }
            Err(error) => (None, ConfigurationStatus::Corrupt(error.to_string())),
        };
        Ok(Self {
            layout,
            manifest,
            maps: HashMap::new(),
            plan,
            configuration,
            planned_requests: OnceLock::new(),
        })
    }

    pub(crate) fn configuration_message(&self) -> Option<String> {
        self.configuration.message()
    }

    pub(crate) fn entries(&self, entry: &Path) -> Vec<(PathBuf, Context)> {
        let root = self.layout.package_root();
        let entry_path = normalized(entry);
        let library = self
            .manifest
            .library_path
            .as_deref()
            .map(|path| root.join(path));
        let artifacts: Vec<_> = self
            .manifest
            .artifacts
            .iter()
            .map(|artifact| {
                let context = match artifact.kind {
                    ArtifactKind::Executable => Context::Executable,
                    ArtifactKind::Test => Context::Test,
                };
                (root.join(&artifact.path), context)
            })
            .collect();
        let mut entries = Vec::new();
        if let Some((_, context)) = artifacts
            .iter()
            .find(|(path, _)| normalized(path) == entry_path)
        {
            entries.push((entry.to_path_buf(), *context));
        } else {
            if let Some(library) = library {
                if normalized(&library) == entry_path {
                    return vec![(library, Context::Library)];
                }
                entries.push((library, Context::Library));
            }
            entries.extend(artifacts);
            let fallback = self.fallback_context(entry);
            if fallback != Context::Build
                && let Some(build) = self.build_candidate()
            {
                entries.push((build, Context::Build));
            }
            entries.push((entry.to_path_buf(), fallback));
        }
        entries
    }

    pub(crate) fn editor_request(
        &mut self,
        path: &Path,
        context: Context,
        toolchain: &nia_toolchain::ToolchainLayout,
    ) -> Result<EditorRequest, String> {
        if context == Context::Build {
            self.build_request(path, toolchain)
        } else {
            let check = self.request(path, context)?;
            let mut request = EditorRequest::manifest(check, *toolchain.artifact_target());
            request.manifest_context = Some(context);
            Ok(request)
        }
    }

    pub(crate) fn request(
        &mut self,
        path: &Path,
        context: Context,
    ) -> Result<CheckRequest, String> {
        let modules = match self.maps.get(&context) {
            Some(modules) => modules.clone(),
            None => {
                let modules = self.module_map(context)?;
                self.maps.insert(context, modules.clone());
                modules
            }
        };
        Ok(CheckRequest::new(normalized(path))
            .with_module_map(modules)
            .with_compilation_mode(if context == Context::Test {
                CompilationMode::Test
            } else {
                CompilationMode::Normal
            }))
    }

    fn build_request(
        &mut self,
        path: &Path,
        toolchain: &nia_toolchain::ToolchainLayout,
    ) -> Result<EditorRequest, String> {
        let target = *toolchain.host_target();
        let profile = self
            .plan
            .as_deref()
            .map_or_else(Default::default, |(_, config)| config.profile);
        let check = self
            .request(path, Context::Build)?
            .with_profile(profile)
            .with_optimization(nia_driver::NiaOptimizationLevel::O0);
        let mut request = EditorRequest::manifest(check, target);
        request.manifest_context = Some(Context::Build);
        Ok(request)
    }

    fn module_map(&self, context: Context) -> Result<ModuleMap, String> {
        self.module_map_with_manifests(context, &BTreeMap::new())
    }

    fn module_map_with_manifests(
        &self,
        context: Context,
        edits: &BTreeMap<PathBuf, String>,
    ) -> Result<ModuleMap, String> {
        let root = self.layout.package_root();
        let installed = InstalledDependencies::read(&self.layout).map_err(|error| {
            format!(
                "{}: {error}",
                self.layout.state_root().join("nia.lock").display()
            )
        })?;
        let mut modules = ModuleMap::new();
        let mut seen = BTreeSet::new();
        let read = |root: &Path| {
            if let Some(text) = edits.get(Path::new(&normalized(root))) {
                PackageManifest::read_with_text(root, text)
            } else if root == self.layout.package_root() {
                Ok(self.manifest.clone())
            } else {
                PackageManifest::read(root)
            }
        };
        let mut pending = vec![(
            root.to_path_buf(),
            read(root).map_err(|error| error.to_string())?,
        )];
        while let Some((owner, manifest)) = pending.pop() {
            if !seen.insert(owner.clone()) {
                continue;
            }
            let scope = SourceIdentity::new(normalized(&owner));
            modules.add_scope(&scope);
            let is_root = owner == root;
            let scopes: &[_] = match context {
                Context::Build => &[DependencyScope::Build],
                Context::Test if is_root => {
                    &[DependencyScope::Runtime, DependencyScope::Development]
                }
                _ => &[DependencyScope::Runtime],
            };
            let mode = if context == Context::Test {
                nia_build_plan::CompilationMode::Test
            } else {
                nia_build_plan::CompilationMode::Normal
            };
            for dependency in manifest.dependencies.iter().filter(|dependency| {
                nia_build::compiler_inputs::manifest_dependency_visible(
                    dependency.scope,
                    scopes,
                    mode,
                )
            }) {
                let Some(target) = installed.source_root(&owner, dependency).map_err(|error| {
                    format!(
                        "{}: dependency `{}`: {error}",
                        owner.display(),
                        dependency.name
                    )
                })?
                else {
                    continue;
                };
                let target_manifest = read(&target).map_err(|error| {
                    format!(
                        "{}: dependency `{}`: {error}",
                        owner.display(),
                        dependency.name
                    )
                })?;
                if let Some(library_path) = &target_manifest.library_path {
                    modules.insert_scoped(
                        &scope,
                        &dependency.name,
                        SourcePath::new(normalized(&target.join(library_path))),
                    )?;
                    pending.push((target, target_manifest));
                }
            }
            if is_root
                && matches!(context, Context::Executable | Context::Test)
                && let Some(library) = &manifest.library_path
            {
                modules.insert_scoped(
                    &scope,
                    &manifest.package.name,
                    SourcePath::new(normalized(&owner.join(library))),
                )?;
            }
        }
        Ok(modules)
    }
}

pub(crate) fn declares_build_hook(text: &str) -> bool {
    let Ok(parsed) = nia_syntax::parse(text, None) else {
        return false;
    };
    parsed.tree.green_root().child_nodes().any(|item| {
        item.child(&SyntaxKind::FunctionDecl)
            .and_then(|function| function.token(&TokenKind::Ident))
            .is_some_and(|name| name.text() == "build")
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use nia_package_metadata::{LOCKFILE_VERSION, LockedPackage, LockedSource, PackageLock};

    #[test]
    fn configuration_states_keep_missing_corrupt_and_stale_distinct() {
        assert_eq!(ConfigurationStatus::Unconfigured.message(), None);
        assert!(
            ConfigurationStatus::Missing
                .message()
                .unwrap()
                .contains("no configured build plan")
        );
        assert!(
            ConfigurationStatus::Corrupt("decode failed".into())
                .message()
                .unwrap()
                .contains("corrupt")
        );
        assert!(
            ConfigurationStatus::Stale("nia.toml changed".into())
                .message()
                .unwrap()
                .contains("stale")
        );
    }

    #[test]
    fn maps_materialized_git_and_registry_dependencies_from_the_workspace_lock() {
        let root = nia_test_support::test_dir("editor-locked-deps");
        let write = |relative: &str, text: &str| {
            let path = root.join(relative);
            std::fs::create_dir_all(path.parent().unwrap()).unwrap();
            std::fs::write(&path, text).unwrap();
            path
        };
        write("nia.toml", "[workspace]\nmembers = [\"app\"]\n");
        write(
            "app/nia.toml",
            "[package]\nname = \"app\"\nversion = \"1.0.0\"\n\
             [dependencies]\nremote = { git = \"https://example.com/nia.git\", branch = \"main\" }\n\
             fixture = { registry = \"central\", package = \"demo/fixture\", version = \"^1.0\" }\n\
             missing = { git = \"https://example.com/missing.git\" }\n",
        );
        let fingerprint = |digit: &str| digit.repeat(64);
        let git = fingerprint("a");
        let registry = fingerprint("b");
        for (digest, name) in [(&git, "remote"), (&registry, "fixture")] {
            write(
                &format!("build/packages/{digest}/nia.toml"),
                &format!(
                    "[package]\nname = \"{name}\"\nnamespace = \"demo\"\nversion = \"1.0.0\"\n[library]\npath = \"root.nia\"\n"
                ),
            );
        }
        let lock_path = root.join("nia.lock");
        PackageLock {
            version: LOCKFILE_VERSION,
            package: vec![
                LockedPackage {
                    name: "remote".into(),
                    source: LockedSource::Git {
                        url: "https://example.com/nia.git".into(),
                        selector: "branch:main".into(),
                        revision: "1".repeat(40),
                        source_fingerprint: format!("blake3:{git}"),
                    },
                },
                LockedPackage {
                    name: "fixture".into(),
                    source: LockedSource::Registry {
                        registry: "central".into(),
                        namespace: "demo".into(),
                        package: "fixture".into(),
                        version: "1.0.0".into(),
                        archive_digest: format!("blake3:{}", fingerprint("c")),
                        source_fingerprint: format!("blake3:{registry}"),
                    },
                },
            ],
        }
        .write_atomic(&lock_path)
        .unwrap();
        let before = std::fs::read(&lock_path).unwrap();
        let entry = root.join("app/main.nia");
        let mut package = Package::load(entry.parent().unwrap()).unwrap();
        let request = package.request(&entry, Context::Executable).unwrap();
        let visible = request.module_map.for_source(&request.entry_path);
        for (name, digest) in [("remote", git), ("fixture", registry)] {
            assert_eq!(
                visible.get(name).unwrap().as_str(),
                normalized(&root.join(format!("build/packages/{digest}/root.nia")))
            );
        }
        assert!(visible.get("missing").is_none());
        assert_eq!(std::fs::read(lock_path).unwrap(), before);
    }
}
