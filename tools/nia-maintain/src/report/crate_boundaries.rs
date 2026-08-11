use std::collections::{BTreeMap, BTreeSet};
use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::LazyLock;

use regex::Regex;
use serde::Deserialize;

use crate::MaintainResult;

static PUBLIC_ITEM: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(
        r"^pub\s+(?:async\s+|unsafe\s+|const\s+|extern\s+)*(?:struct|enum|union|trait|type|const|static|fn|mod|use)\b",
    )
    .expect("valid public item regex")
});

#[derive(Debug, Clone, Default)]
pub struct Options {
    pub max_rust_loc: Option<usize>,
    pub max_production_dependents: Option<usize>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CrateBoundary {
    pub name: String,
    pub rust_loc: usize,
    pub public_items: usize,
    pub production_dependencies: Vec<String>,
    pub production_dependents: Vec<String>,
    pub dev_only_dependents: Vec<String>,
}

#[derive(Debug, Clone, Deserialize)]
pub struct DependencyKind {
    #[serde(default)]
    pub kind: Option<String>,
}

#[derive(Debug, Clone, Deserialize)]
pub struct CargoDependency {
    pub name: String,
    #[serde(default)]
    pub kind: Option<String>,
    #[serde(default)]
    pub dep_kinds: Vec<DependencyKind>,
}

#[derive(Debug, Clone, Deserialize)]
pub struct CargoPackage {
    pub name: String,
    pub id: String,
    pub manifest_path: PathBuf,
    #[serde(default)]
    pub dependencies: Vec<CargoDependency>,
}

#[derive(Debug, Clone, Deserialize)]
pub struct CargoMetadata {
    pub packages: Vec<CargoPackage>,
    pub workspace_members: Vec<String>,
}

fn cargo_metadata(root: &Path) -> MaintainResult<CargoMetadata> {
    let output = Command::new("cargo")
        .args(["metadata", "--no-deps", "--format-version", "1"])
        .current_dir(root)
        .output()
        .map_err(|error| format!("failed to run cargo metadata: {error}"))?;
    if !output.status.success() {
        return Err(format!(
            "cargo metadata failed with {}:\n{}",
            output.status,
            String::from_utf8_lossy(&output.stderr)
        ));
    }
    serde_json::from_slice(&output.stdout)
        .map_err(|error| format!("failed to decode cargo metadata: {error}"))
}

fn dependency_kinds(dependency: &CargoDependency) -> BTreeSet<String> {
    if let Some(kind) = &dependency.kind {
        return BTreeSet::from([kind.clone()]);
    }
    if !dependency.dep_kinds.is_empty() {
        return dependency
            .dep_kinds
            .iter()
            .map(|entry| entry.kind.clone().unwrap_or_else(|| "normal".to_owned()))
            .collect();
    }
    BTreeSet::from(["normal".to_owned()])
}

fn collect_rust_sources(root: &Path, paths: &mut Vec<PathBuf>) -> MaintainResult<()> {
    for entry in
        fs::read_dir(root).map_err(|error| format!("failed to read {}: {error}", root.display()))?
    {
        let entry =
            entry.map_err(|error| format!("failed to inspect {}: {error}", root.display()))?;
        let file_type = entry
            .file_type()
            .map_err(|error| format!("failed to inspect {}: {error}", entry.path().display()))?;
        if file_type.is_dir() {
            collect_rust_sources(&entry.path(), paths)?;
        } else if file_type.is_file()
            && entry
                .path()
                .extension()
                .is_some_and(|extension| extension == "rs")
        {
            paths.push(entry.path());
        }
    }
    Ok(())
}

pub fn rust_source_metrics(crate_root: &Path) -> MaintainResult<(usize, usize)> {
    let source_root = crate_root.join("src");
    if !source_root.is_dir() {
        return Ok((0, 0));
    }
    let mut paths = Vec::new();
    collect_rust_sources(&source_root, &mut paths)?;
    paths.sort();
    let mut rust_loc = 0;
    let mut public_items = 0;
    for path in paths {
        let source = fs::read_to_string(&path)
            .map_err(|error| format!("failed to read {}: {error}", path.display()))?;
        for line in source
            .lines()
            .map(str::trim)
            .filter(|line| !line.is_empty())
        {
            rust_loc += 1;
            if PUBLIC_ITEM.is_match(line) {
                public_items += 1;
            }
        }
    }
    Ok((rust_loc, public_items))
}

pub fn workspace_boundaries(metadata: &CargoMetadata) -> MaintainResult<Vec<CrateBoundary>> {
    let members = metadata
        .workspace_members
        .iter()
        .cloned()
        .collect::<BTreeSet<_>>();
    let packages = metadata
        .packages
        .iter()
        .filter(|package| members.contains(&package.id))
        .map(|package| (package.name.clone(), package))
        .collect::<BTreeMap<_, _>>();
    let mut production_dependencies = packages
        .keys()
        .map(|name| (name.clone(), BTreeSet::new()))
        .collect::<BTreeMap<_, _>>();
    let mut production_dependents = production_dependencies.clone();
    let mut dev_dependents = production_dependencies.clone();

    for (consumer, package) in &packages {
        for dependency in &package.dependencies {
            let provider = &dependency.name;
            if !packages.contains_key(provider) {
                continue;
            }
            let kinds = dependency_kinds(dependency);
            if kinds.iter().any(|kind| kind != "dev") {
                production_dependencies
                    .get_mut(consumer)
                    .expect("workspace consumer set")
                    .insert(provider.clone());
                production_dependents
                    .get_mut(provider)
                    .expect("workspace provider set")
                    .insert(consumer.clone());
            }
            if kinds.contains("dev") {
                dev_dependents
                    .get_mut(provider)
                    .expect("workspace provider dev set")
                    .insert(consumer.clone());
            }
        }
    }

    let mut boundaries = Vec::new();
    for (name, package) in packages {
        let crate_root = package.manifest_path.parent().ok_or_else(|| {
            format!(
                "manifest has no parent: {}",
                package.manifest_path.display()
            )
        })?;
        let (rust_loc, public_items) = rust_source_metrics(crate_root)?;
        let production = &production_dependents[&name];
        let dev_only = dev_dependents[&name]
            .difference(production)
            .cloned()
            .collect();
        boundaries.push(CrateBoundary {
            name: name.clone(),
            rust_loc,
            public_items,
            production_dependencies: production_dependencies[&name].iter().cloned().collect(),
            production_dependents: production.iter().cloned().collect(),
            dev_only_dependents: dev_only,
        });
    }
    Ok(boundaries)
}

fn joined(values: &[String]) -> String {
    if values.is_empty() {
        "-".to_owned()
    } else {
        values.join(",")
    }
}

pub fn tsv(boundaries: &[CrateBoundary]) -> String {
    let mut output = String::from(
        "crate\trust_loc\tpublic_items\tproduction_dependencies\tproduction_dependents\tdev_only_dependents\n",
    );
    for boundary in boundaries {
        output.push_str(&format!(
            "{}\t{}\t{}\t{}\t{}\t{}\n",
            boundary.name,
            boundary.rust_loc,
            boundary.public_items,
            joined(&boundary.production_dependencies),
            joined(&boundary.production_dependents),
            joined(&boundary.dev_only_dependents),
        ));
    }
    output
}

pub fn run(root: &Path, options: &Options) -> MaintainResult<()> {
    let mut boundaries = workspace_boundaries(&cargo_metadata(root)?)?;
    boundaries.retain(|boundary| {
        options
            .max_rust_loc
            .is_none_or(|maximum| boundary.rust_loc <= maximum)
            && options
                .max_production_dependents
                .is_none_or(|maximum| boundary.production_dependents.len() <= maximum)
    });
    print!("{}", tsv(&boundaries));
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::test_support::TestDirectory;

    #[test]
    fn counts_non_empty_source_lines_and_public_items() {
        let directory = TestDirectory::new("source-metrics");
        directory.write(
            "src/lib.rs",
            "pub struct Public;\n\nstruct Private;\npub(crate) fn helper() {}\n",
        );
        assert_eq!(
            rust_source_metrics(directory.path()).expect("measure source"),
            (3, 1)
        );
    }

    #[test]
    fn separates_production_and_dev_only_dependents() {
        let directory = TestDirectory::new("crate-boundaries");
        let package = |name: &str, dependencies: Vec<CargoDependency>| CargoPackage {
            name: name.to_owned(),
            id: name.to_owned(),
            manifest_path: directory.path().join(name).join("Cargo.toml"),
            dependencies,
        };
        for name in ["provider", "consumer", "dev-consumer"] {
            directory.write(&format!("{name}/src/lib.rs"), "");
        }
        let dependency = |kind: Option<&str>| CargoDependency {
            name: "provider".to_owned(),
            kind: kind.map(str::to_owned),
            dep_kinds: Vec::new(),
        };
        let metadata = CargoMetadata {
            packages: vec![
                package("provider", Vec::new()),
                package("consumer", vec![dependency(None), dependency(Some("dev"))]),
                package("dev-consumer", vec![dependency(Some("dev"))]),
            ],
            workspace_members: vec![
                "provider".to_owned(),
                "consumer".to_owned(),
                "dev-consumer".to_owned(),
            ],
        };
        let boundaries = workspace_boundaries(&metadata)
            .expect("compute boundaries")
            .into_iter()
            .map(|boundary| (boundary.name.clone(), boundary))
            .collect::<BTreeMap<_, _>>();

        assert_eq!(boundaries["provider"].production_dependents, ["consumer"]);
        assert_eq!(boundaries["provider"].dev_only_dependents, ["dev-consumer"]);
        assert_eq!(boundaries["consumer"].production_dependencies, ["provider"]);
    }
}
