// SPDX-License-Identifier: GPL-3.0-or-later
//! Dependency edits included in a verified module move.

use super::*;
use nia_package_metadata::{PackageGraph, PackageManifest};
use toml_edit::{DocumentMut, InlineTable, Item, Table, Value};

#[derive(Default)]
pub(super) struct ManifestEdits {
    additions: BTreeMap<PathBuf, BTreeMap<String, PathBuf>>,
}

#[derive(Default)]
pub(super) struct PreparedManifestEdits {
    pub(super) proposed: BTreeMap<PathBuf, String>,
    original: BTreeMap<PathBuf, (String, Option<i32>)>,
}

impl ManifestEdits {
    pub(super) fn require(
        &mut self,
        workspace: &WorkspaceIndex,
        owner: &Path,
        target: &Path,
    ) -> Result<String, String> {
        let packages = &workspace.configuration.package_names;
        let owner = owner
            .ancestors()
            .find(|path| packages.contains_key(*path))
            .ok_or("The package that needs a dependency is outside the workspace")?;
        let (target, name) = target
            .ancestors()
            .find_map(|path| packages.get_key_value(path))
            .ok_or("The new dependency is outside the workspace")?;
        let dependencies = self.additions.entry(owner.to_path_buf()).or_default();
        if let Some((alias, _)) = dependencies.iter().find(|(_, root)| *root == target) {
            return Ok(alias.clone());
        }
        let manifest = PackageManifest::read(owner).map_err(|error| error.to_string())?;
        let stem = name.replace('-', "_");
        let stem = if identifier(&stem) {
            stem.as_str()
        } else {
            "moved_package"
        };
        let alias = (1..)
            .map(|index| {
                if index == 1 {
                    stem.to_owned()
                } else {
                    format!("{stem}_{index}")
                }
            })
            .find(|candidate| {
                identifier(candidate)
                    && candidate != "std"
                    && candidate != "program"
                    && !dependencies.contains_key(candidate)
                    && !manifest
                        .dependencies
                        .iter()
                        .any(|dependency| dependency.name == *candidate)
            })
            .expect("a finite manifest cannot occupy every alias");
        dependencies.insert(alias.clone(), target.clone());
        Ok(alias)
    }

    pub(super) fn merge(&mut self, other: Self) -> Result<(), String> {
        for (owner, additions) in other.additions {
            let dependencies = self.additions.entry(owner).or_default();
            for (alias, target) in additions {
                if dependencies
                    .get(&alias)
                    .is_some_and(|existing| existing != &target)
                {
                    return Err(format!(
                        "Module moves require incompatible dependency aliases for `{alias}`"
                    ));
                }
                dependencies.insert(alias, target);
            }
        }
        Ok(())
    }

    pub(super) fn prepare(
        self,
        documents: &Documents,
        cancel: &Cancellation,
    ) -> Result<PreparedManifestEdits, String> {
        let mut prepared = PreparedManifestEdits::default();
        for (owner, additions) in self.additions {
            cancel.check()?;
            let path = owner.join("nia.toml");
            let original = read_manifest(&path)?;
            let version = if let Some(open) = documents.get(&path) {
                if open.document.text.as_ref() != original {
                    return Err(format!(
                        "Save the modified manifest before moving modules ({})",
                        path.display()
                    ));
                }
                Some(open.document.version)
            } else {
                None
            };
            let mut manifest = original
                .parse::<DocumentMut>()
                .map_err(|error| error.to_string())?;
            let installed = PackageManifest::read_with_text(&owner, &original)
                .map_err(|error| error.to_string())?;
            if manifest.get("dependencies").is_none() {
                manifest.insert("dependencies", Item::Table(Table::new()));
            }
            let dependencies = manifest["dependencies"]
                .as_table_like_mut()
                .ok_or("Expected a dependency table")?;
            for (alias, target) in additions {
                if installed
                    .dependencies
                    .iter()
                    .any(|dependency| dependency.name == alias)
                {
                    return Err(format!(
                        "The dependency alias `{alias}` is already occupied in {}",
                        path.display()
                    ));
                }
                let path = relative_dependency(&owner, &target)?;
                let mut dependency = InlineTable::new();
                dependency.insert("path", Value::from(path));
                dependencies.insert(&alias, Item::Value(Value::InlineTable(dependency)));
            }
            prepared.original.insert(path, (original, version));
            prepared.proposed.insert(owner, manifest.to_string());
        }
        for owner in prepared.proposed.keys() {
            cancel.check()?;
            PackageGraph::verify_installed_manifest_edits(owner, &prepared.proposed).map_err(
                |error| {
                    format!(
                        "Cannot change dependencies in {} ({error})",
                        owner.display()
                    )
                },
            )?;
        }
        Ok(prepared)
    }
}

impl PreparedManifestEdits {
    pub(super) fn text_edits(&self) -> Result<Vec<TextDocumentEdit>, String> {
        self.original
            .iter()
            .map(|(path, (text, version))| {
                let lines = crate::document::Lines::new(text);
                let range = lines.range(text, Span::new(0, text.len()));
                let owner = path.parent().expect("manifest has a package directory");
                Ok(TextDocumentEdit {
                    text_document: OptionalVersionedTextDocumentIdentifier {
                        uri: uri(path).ok_or("Cannot form a manifest URI")?,
                        version: *version,
                    },
                    edits: vec![OneOf::Left(TextEdit::new(
                        range,
                        self.proposed[owner].clone(),
                    ))],
                })
            })
            .collect()
    }

    pub(super) fn verify_current(&self) -> Result<(), String> {
        for (path, (before, _)) in &self.original {
            if read_manifest(path)? != *before {
                return Err(format!("Manifest changed on disk ({})", path.display()));
            }
        }
        Ok(())
    }
}

fn read_manifest(path: &Path) -> Result<String, String> {
    let physical = path
        .canonicalize()
        .map_err(|error| format!("{}: {error}", path.display()))?;
    if normalized(&physical) != normalized(path) {
        return Err(format!(
            "Cannot edit a linked manifest ({})",
            path.display()
        ));
    }
    std::fs::read_to_string(path).map_err(|error| format!("{}: {error}", path.display()))
}

fn relative_dependency(owner: &Path, target: &Path) -> Result<String, String> {
    let common = owner
        .ancestors()
        .find(|path| target.starts_with(path))
        .ok_or("The dependent packages do not share a filesystem root")?;
    let mut relative = PathBuf::new();
    for _ in owner
        .strip_prefix(common)
        .expect("shared ancestor")
        .components()
    {
        relative.push("..");
    }
    relative.push(target.strip_prefix(common).expect("shared ancestor"));
    relative
        .to_str()
        .map(|path| path.replace('\\', "/"))
        .ok_or("A dependency path is not UTF-8".into())
}
