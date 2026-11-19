// SPDX-License-Identifier: GPL-3.0-or-later
//! Compiler entry points from the last configured build plan.

use super::*;
use nia_build::compiler_inputs::{
    PlanCompilation, PlanDependencyScopes, PlanDirectories, package_dependency_selected,
};
use nia_build_plan::{LogicalPathRoot, PlanPackageSource};

impl Package {
    pub(crate) fn configured_requests(
        &self,
        toolchain: &nia_toolchain::ToolchainLayout,
    ) -> Result<Vec<EditorRequest>, String> {
        self.planned_requests
            .get_or_init(|| self.read_configured_requests(toolchain))
            .clone()
    }

    fn read_configured_requests(
        &self,
        toolchain: &nia_toolchain::ToolchainLayout,
    ) -> Result<Vec<EditorRequest>, String> {
        let Some(plan) = self.plan.as_ref() else {
            if let Some(message) = self.configuration_message() {
                return Err(message);
            }
            return Ok(Vec::new());
        };
        let installed =
            InstalledDependencies::read(&self.layout).map_err(|error| error.to_string())?;
        let root = self.layout.package_root();
        let mut package_roots = HashMap::new();
        for package in plan.packages() {
            let path = match &package.source {
                PlanPackageSource::Local(path) => root.join(path),
                PlanPackageSource::Dependency(alias) => {
                    let dependency = self
                        .manifest
                        .dependencies
                        .iter()
                        .find(|dependency| &dependency.name == alias)
                        .ok_or_else(|| format!("Build plan uses unknown dependency `{alias}`"))?;
                    installed
                        .source_root(root, dependency)
                        .map_err(|error| error.to_string())?
                        .ok_or_else(|| format!("Build plan dependency `{alias}` is unavailable"))?
                }
            };
            package_roots.insert(&package.key, path);
        }
        let dependency_scopes = self
            .plan_dependency_scopes(plan, &installed)
            .classify(plan, |package| package_roots.get(&package.key).cloned());
        let output = self.layout.out_dir();
        let cache = self.layout.cache_dir();
        let directories = PlanDirectories {
            build: &output,
            cache: &cache,
            toolchain: toolchain.resource_root(),
        };
        let package_root: &nia_build::compiler_inputs::PackageRoot<'_> =
            &|key| package_roots.get(key).cloned();
        let mut contexts = std::collections::BTreeMap::new();
        for input in plan
            .actions()
            .iter()
            .filter_map(|action| PlanCompilation::for_action(plan, action))
        {
            let cpu = input.configuration.cpu.unwrap_or_default();
            contexts
                .entry((
                    &input.module.key,
                    input.mode,
                    input.target,
                    input.runtime,
                    cpu,
                ))
                .or_insert(input);
        }
        let mut requests = Vec::new();
        for ((_, mode, target, runtime, cpu), input) in contexts {
            let module = input.module;
            let entry = directories
                .resolve(plan, &module.root_source, package_root)
                .map_err(|error| format!("Could not resolve compiler entry: {error:?}"))?;
            let context = if mode == nia_build_plan::CompilationMode::Test {
                Context::Test
            } else {
                Context::Executable
            };
            let mut modules = self.module_map(context)?;
            let directory = match module.root_source.root() {
                LogicalPathRoot::Package(package) => &package_roots[package],
                LogicalPathRoot::Build => &output,
                _ => entry.parent().unwrap_or(&output),
            };
            let scope = SourceIdentity::new(normalized(directory));
            modules.add_scope(&scope);
            for import in &module.imports {
                if let LogicalPathRoot::Package(package) = import.path.root()
                    && !package_dependency_selected(&dependency_scopes, package, mode)
                {
                    continue;
                }
                let path = directories
                    .resolve(plan, &import.path, package_root)
                    .map_err(|error| {
                        format!("Could not resolve import `{}`: {error:?}", import.name)
                    })?;
                modules.insert_scoped(&scope, &import.name, SourcePath::new(normalized(&path)))?;
            }
            requests.push(EditorRequest {
                check: input
                    .check_request(SourcePath::new(normalized(&entry)))
                    .with_module_map(modules),
                target: Some((target, cpu, runtime)),
            });
        }
        Ok(requests)
    }

    fn plan_dependency_scopes(
        &self,
        plan: &BuildPlan,
        installed: &InstalledDependencies,
    ) -> PlanDependencyScopes {
        let mut scopes = PlanDependencyScopes::default();
        let needs_directories = plan.packages().iter().any(|package| {
            &package.key != plan.root_package()
                && matches!(package.source, PlanPackageSource::Local(_))
        });
        let root = self.layout.package_root();
        let mut pending = vec![(root.to_path_buf(), self.manifest.clone())];
        let mut seen = BTreeSet::from([root.to_path_buf()]);
        while let Some((owner, manifest)) = pending.pop() {
            let is_root = owner == root;
            for dependency in &manifest.dependencies {
                let development_only = !is_root && dependency.scope == DependencyScope::Development;
                // Filtered development dependencies still classify their local directories.
                let directory = if needs_directories
                    && (!development_only
                        || matches!(
                            dependency.source,
                            nia_package_metadata::DependencySource::Path(_)
                        )) {
                    installed.source_root(&owner, dependency).ok().flatten()
                } else {
                    None
                };
                scopes.declare(
                    is_root.then_some(dependency.name.as_str()),
                    directory.as_deref(),
                    dependency.scope,
                );
                if !development_only
                    && let Some(directory) = directory
                    && seen.insert(directory.clone())
                    && let Ok(manifest) = PackageManifest::read(&directory)
                {
                    pending.push((directory, manifest));
                }
            }
        }
        scopes
    }
}
