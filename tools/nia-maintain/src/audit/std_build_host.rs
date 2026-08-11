use std::collections::{BTreeSet, VecDeque};
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::LazyLock;

use regex::Regex;
use serde::{Deserialize, Serialize};

use crate::MaintainResult;

const ROOT_MODULES: [&str; 3] = ["builtin.nia", "start.nia", "build.nia"];

static USING: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"^\s*using\s+pkg::([A-Za-z_][A-Za-z0-9_]*(?:::[A-Za-z_][A-Za-z0-9_]*)*)")
        .expect("valid using regex")
});
static MODULE: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"^\s*(?:pub(?:\(pkg\))?\s+)?module\s+([A-Za-z_][A-Za-z0-9_]*)\s*;")
        .expect("valid module regex")
});

#[derive(Debug, Clone)]
pub struct Options {
    pub print: bool,
    pub snapshot: PathBuf,
}

impl Options {
    pub fn for_repository(root: &Path) -> Self {
        Self {
            print: false,
            snapshot: root.join("tools/fixtures/std-build-host-dependencies.json"),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Snapshot {
    pub schema_version: u32,
    pub kind: String,
    pub roots: Vec<String>,
    pub modules: Vec<String>,
}

fn package_module_path(reference: &str, std_root: &Path) -> PathBuf {
    let parts = reference.split("::").collect::<Vec<_>>();
    for length in (1..=parts.len()).rev() {
        let mut candidate = std_root.to_path_buf();
        for part in &parts[..length] {
            candidate.push(part);
        }
        candidate.set_extension("nia");
        if candidate.is_file() {
            return candidate;
        }
    }
    let mut candidate = std_root.to_path_buf();
    for part in parts {
        candidate.push(part);
    }
    candidate.set_extension("nia");
    candidate
}

fn child_module_path(owner: &Path, name: &str) -> PathBuf {
    owner.with_extension("").join(format!("{name}.nia"))
}

fn source_dependencies(path: &Path, std_root: &Path) -> MaintainResult<BTreeSet<PathBuf>> {
    let source = fs::read_to_string(path)
        .map_err(|error| format!("failed to read {}: {error}", path.display()))?;
    let mut dependencies = BTreeSet::new();
    for line in source.lines() {
        if let Some(captures) = USING.captures(line) {
            dependencies.insert(package_module_path(&captures[1], std_root));
        }
        if let Some(captures) = MODULE.captures(line) {
            let child = child_module_path(path, &captures[1]);
            if child.is_file() {
                dependencies.insert(child);
            }
        }
    }
    Ok(dependencies)
}

pub fn build_host_closure(std_root: &Path) -> MaintainResult<Vec<String>> {
    let mut queue = ROOT_MODULES
        .iter()
        .map(|name| std_root.join(name))
        .collect::<VecDeque<_>>();
    let mut visited = BTreeSet::new();
    while let Some(path) = queue.pop_front() {
        if visited.contains(&path) {
            continue;
        }
        if !path.is_file() {
            return Err(format!(
                "build-host dependency does not exist: {}",
                path.display()
            ));
        }
        visited.insert(path.clone());
        for dependency in source_dependencies(&path, std_root)? {
            if !visited.contains(&dependency) {
                queue.push_back(dependency);
            }
        }
    }
    let parent = std_root
        .parent()
        .ok_or_else(|| format!("std root has no parent: {}", std_root.display()))?;
    let mut modules = visited
        .into_iter()
        .map(|path| {
            path.strip_prefix(parent)
                .map(|relative| relative.to_string_lossy().replace('\\', "/"))
                .map_err(|error| format!("failed to normalize {}: {error}", path.display()))
        })
        .collect::<MaintainResult<Vec<_>>>()?;
    modules.sort();
    Ok(modules)
}

pub fn snapshot(root: &Path) -> MaintainResult<Snapshot> {
    Ok(Snapshot {
        schema_version: 1,
        kind: "nia-std-build-host-source-closure".to_owned(),
        roots: ROOT_MODULES
            .iter()
            .map(|name| format!("std/{name}"))
            .collect(),
        modules: build_host_closure(&root.join("lib/std"))?,
    })
}

pub fn run(root: &Path, options: &Options) -> MaintainResult<()> {
    let current = snapshot(root)?;
    if options.print {
        println!(
            "{}",
            serde_json::to_string_pretty(&current)
                .map_err(|error| format!("failed to encode std closure: {error}"))?
        );
        return Ok(());
    }
    let source = fs::read_to_string(&options.snapshot).map_err(|error| {
        format!(
            "failed to read std closure snapshot {}: {error}",
            options.snapshot.display()
        )
    })?;
    let expected: Snapshot = serde_json::from_str(&source).map_err(|error| {
        format!(
            "failed to decode std closure snapshot {}: {error}",
            options.snapshot.display()
        )
    })?;
    if current != expected {
        return Err(
            "build-host std dependency closure changed; review API/layering impact, use \
             --print to inspect it, and update the snapshot deliberately"
                .to_owned(),
        );
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::repository_root;
    use crate::test_support::TestDirectory;

    fn create_roots(directory: &TestDirectory) -> PathBuf {
        for root in ROOT_MODULES {
            directory.write(&format!("std/{root}"), "");
        }
        directory.path().join("std")
    }

    #[test]
    fn maintained_snapshot_matches_source_closure() {
        let root = repository_root();
        let expected: Snapshot = serde_json::from_str(
            &fs::read_to_string(root.join("tools/fixtures/std-build-host-dependencies.json"))
                .expect("read maintained snapshot"),
        )
        .expect("decode maintained snapshot");
        assert_eq!(snapshot(&root).expect("compute snapshot"), expected);
    }

    #[test]
    fn follows_package_imports_and_declared_provider_modules() {
        let directory = TestDirectory::new("std-closure");
        let std_root = create_roots(&directory);
        directory.write(
            "std/build.nia",
            "pub(pkg) module core;\nusing pkg::support;\n",
        );
        directory.write("std/build/core.nia", "");
        directory.write("std/support.nia", "pub(pkg) module provider;\n");
        directory.write("std/support/provider.nia", "");

        assert_eq!(
            build_host_closure(&std_root).expect("compute closure"),
            vec![
                "std/build.nia",
                "std/build/core.nia",
                "std/builtin.nia",
                "std/start.nia",
                "std/support.nia",
                "std/support/provider.nia",
            ]
        );
    }

    #[test]
    fn rejects_missing_package_dependency() {
        let directory = TestDirectory::new("missing-std-dependency");
        let std_root = create_roots(&directory);
        directory.write("std/build.nia", "using pkg::missing;\n");

        let error = build_host_closure(&std_root).expect_err("missing module should fail");
        assert!(error.contains("does not exist"));
    }
}
