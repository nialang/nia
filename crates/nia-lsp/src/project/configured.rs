// SPDX-License-Identifier: GPL-3.0-or-later
//! Compiler entry points from the last configured build plan.

use super::*;

impl Package {
    pub(crate) fn configured_requests(&self) -> Result<Vec<EditorRequest>, String> {
        self.planned_requests
            .get_or_init(|| self.read_configured_requests())
            .clone()
    }

    fn read_configured_requests(&self) -> Result<Vec<EditorRequest>, String> {
        let Some(plan) = &self.plan else {
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
        let resolve = |path: &LogicalPath| {
            let root = match path.root() {
                LogicalPathRoot::Package(package) => package_roots.get(package)?,
                LogicalPathRoot::Build => &output,
                _ => return None,
            };
            Some(root.join(path.protocol_path()))
        };
        let entries: BTreeSet<_> = plan
            .actions()
            .iter()
            .filter_map(|action| match &action.kind {
                ActionKind::CompilerCheck {
                    module,
                    mode,
                    target,
                    runtime,
                    configuration,
                } => Some((
                    module,
                    *mode,
                    *target,
                    *runtime,
                    configuration.cpu.unwrap_or_default(),
                )),
                ActionKind::CompilerEmit {
                    artifact,
                    target,
                    configuration,
                    ..
                } => {
                    let artifact = plan
                        .artifacts()
                        .iter()
                        .find(|candidate| &candidate.key == artifact)?;
                    Some((
                        &artifact.root_module,
                        artifact.mode,
                        *target,
                        artifact.runtime,
                        configuration.cpu.unwrap_or_default(),
                    ))
                }
                _ => None,
            })
            .collect();
        let mut requests = Vec::new();
        for (key, mode, target, runtime, cpu) in entries {
            let module = plan
                .modules()
                .iter()
                .find(|module| &module.key == key)
                .ok_or("Build plan entry has no module")?;
            let entry = resolve(&module.root_source)
                .ok_or("Build plan entry has an unsupported source root")?;
            let context = if mode == nia_build_plan::CompilationMode::Test {
                Context::Test
            } else {
                Context::Executable
            };
            let mut modules = self.module_map(context)?;
            let directory = match module.root_source.root() {
                LogicalPathRoot::Package(package) => &package_roots[package],
                _ => &output,
            };
            let scope = SourceIdentity::new(normalized(directory));
            modules.add_scope(&scope);
            for import in &module.imports {
                let path = resolve(&import.path).ok_or_else(|| {
                    format!(
                        "Build plan import `{}` has an unsupported source root",
                        import.name
                    )
                })?;
                modules.insert_scoped(&scope, &import.name, SourcePath::new(normalized(&path)))?;
            }
            requests.push(EditorRequest {
                check: CheckRequest::new(normalized(&entry))
                    .with_module_map(modules)
                    .with_compilation_mode(if context == Context::Test {
                        CompilationMode::Test
                    } else {
                        CompilationMode::Normal
                    }),
                target: Some((target, cpu, runtime)),
            });
        }
        Ok(requests)
    }
}
