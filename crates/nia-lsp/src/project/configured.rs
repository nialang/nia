// SPDX-License-Identifier: GPL-3.0-or-later
//! Compiler entry points from the last configured build plan.

use super::*;

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
        let Some(plan) = self.plan.as_ref().map_err(Clone::clone)? else {
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
        let output = self.layout.out_dir();
        let cache = self.layout.cache_dir();
        let directories = nia_build::compiler_inputs::PlanDirectories {
            build: &output,
            cache: &cache,
            toolchain: toolchain.resource_root(),
        };
        let package_root: &nia_build::compiler_inputs::PackageRoot<'_> =
            &|key| package_roots.get(key).cloned();
        let mut contexts = std::collections::BTreeMap::new();
        for input in plan.actions().iter().filter_map(|action| {
            nia_build::compiler_inputs::PlanCompilation::for_action(plan, action)
        }) {
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
                nia_build_plan::LogicalPathRoot::Package(package) => &package_roots[package],
                nia_build_plan::LogicalPathRoot::Build => &output,
                _ => entry.parent().unwrap_or(&output),
            };
            let scope = SourceIdentity::new(normalized(directory));
            modules.add_scope(&scope);
            for import in &module.imports {
                if let nia_build_plan::LogicalPathRoot::Package(package) = import.path.root()
                    && !self.selected_plan_dependency(plan, package, mode)
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

    fn selected_plan_dependency(
        &self,
        plan: &BuildPlan,
        key: &nia_build_plan::PackageKey,
        mode: nia_build_plan::CompilationMode,
    ) -> bool {
        let Some(package) = plan.packages().iter().find(|package| &package.key == key) else {
            return false;
        };
        let PlanPackageSource::Dependency(alias) = &package.source else {
            return true;
        };
        self.manifest
            .dependencies
            .iter()
            .find(|dependency| &dependency.name == alias)
            .is_some_and(|dependency| {
                nia_build::compiler_inputs::artifact_dependency_visible(dependency.scope, mode)
            })
    }
}
