// SPDX-License-Identifier: GPL-3.0-or-later
//! Package contexts for editor analysis.

use std::{
    collections::{BTreeSet, HashMap},
    path::{Path, PathBuf},
};

use nia_driver::CheckRequest;
use nia_imports::ModuleMap;
use nia_package_metadata::{
    ArtifactKind, BuildLayout, DependencyScope, InstalledDependencies, PackageManifest,
    WorkspaceManifest,
};
use nia_source::{SourceIdentity, SourcePath};
use nia_target_config::CompilationMode;

use crate::analysis::normalized;

#[derive(Default)]
pub(crate) struct Projects {
    packages: HashMap<PathBuf, Package>,
}

pub(crate) struct WorkspaceRequests {
    pub(crate) entries: Vec<CheckRequest>,
    pub(crate) standalone: Vec<CheckRequest>,
    pub(crate) editable: BTreeSet<PathBuf>,
}

impl Projects {
    pub(crate) fn workspace_requests(
        &mut self,
        entry: &Path,
        open: impl Iterator<Item = PathBuf>,
        folders: &[PathBuf],
    ) -> Result<WorkspaceRequests, String> {
        let mut entries = BTreeSet::new();
        let mut standalone = BTreeSet::new();
        let mut roots = BTreeSet::new();
        let mut editable = BTreeSet::new();
        for folder in folders {
            if folder.join("nia.toml").is_file() {
                roots.insert(folder.canonicalize().map_err(|error| error.to_string())?);
            }
        }
        for path in std::iter::once(entry.to_path_buf()).chain(open) {
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
            editable.insert(PathBuf::from(normalized(package.layout.package_root())));
            entries.extend(package.declared_entries());
        }
        let entries = entries
            .into_iter()
            .map(|(path, context)| (PathBuf::from(normalized(&path)), context))
            .collect::<BTreeSet<_>>()
            .into_iter()
            .map(|(path, context)| match self.package(&path)? {
                Some(package) => package.request(&path, context),
                None => Ok(CheckRequest::new(normalized(&path))),
            })
            .collect::<Result<_, _>>()?;
        let standalone = standalone
            .into_iter()
            .map(|path| match self.package(&path)? {
                Some(package) => package.request(&path, Context::Executable),
                None => Ok(CheckRequest::new(normalized(&path))),
            })
            .collect::<Result<_, _>>()?;
        Ok(WorkspaceRequests {
            entries,
            standalone,
            editable,
        })
    }

    pub(crate) fn package(&mut self, entry: &Path) -> Result<Option<&mut Package>, String> {
        let Some(root) = entry
            .ancestors()
            .skip(1)
            .find(|directory| directory.join("nia.toml").is_file())
        else {
            return Ok(None);
        };
        if !self.packages.contains_key(root) {
            let package = Package::load(root)?;
            self.packages.insert(root.to_path_buf(), package);
        }
        Ok(self.packages.get_mut(root))
    }
}

pub(crate) struct Package {
    layout: BuildLayout,
    manifest: PackageManifest,
    maps: HashMap<Context, ModuleMap>,
}

#[derive(Clone, Copy, Debug, Eq, Hash, Ord, PartialEq, PartialOrd)]
pub(crate) enum Context {
    Library,
    Executable,
    Test,
    Build,
}

impl Package {
    fn declared_entries(&self) -> Vec<(PathBuf, Context)> {
        let root = self.layout.package_root();
        let mut entries = Vec::new();
        if let Some(library) = &self.manifest.library_path {
            entries.push((root.join(library), Context::Library));
        }
        let build = root.join("build.nia");
        if build.is_file() {
            entries.push((build, Context::Build));
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
        Ok(Self {
            layout,
            manifest,
            maps: HashMap::new(),
        })
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
        if entry_path == normalized(&root.join("build.nia")) {
            entries.push((entry.to_path_buf(), Context::Build));
        } else if let Some((_, context)) = artifacts
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
            entries.push((entry.to_path_buf(), Context::Executable));
        }
        entries
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

    fn module_map(&self, context: Context) -> Result<ModuleMap, String> {
        let root = self.layout.package_root();
        let installed = InstalledDependencies::read(&self.layout).map_err(|error| {
            format!(
                "{}: {error}",
                self.layout.state_root().join("nia.lock").display()
            )
        })?;
        let mut modules = ModuleMap::new();
        let mut seen = BTreeSet::new();
        let mut pending = vec![(root.to_path_buf(), self.manifest.clone())];
        while let Some((owner, manifest)) = pending.pop() {
            if !seen.insert(owner.clone()) {
                continue;
            }
            let scope = SourceIdentity::new(normalized(&owner));
            modules.add_scope(&scope);
            let is_root = owner == root;
            for dependency in
                manifest
                    .dependencies
                    .iter()
                    .filter(|dependency| match dependency.scope {
                        DependencyScope::Build => context == Context::Build,
                        DependencyScope::Runtime | DependencyScope::Lazy => {
                            context != Context::Build
                        }
                        DependencyScope::Development => context == Context::Test && is_root,
                        DependencyScope::Optional => false,
                    })
            {
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
                let target_manifest = PackageManifest::read(&target).map_err(|error| {
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

#[cfg(test)]
mod tests {
    use super::*;
    use nia_package_metadata::{LOCKFILE_VERSION, LockedPackage, LockedSource, PackageLock};

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
