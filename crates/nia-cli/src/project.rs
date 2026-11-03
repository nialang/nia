// SPDX-License-Identifier: GPL-3.0-or-later
//! Project creation and manifest editing for Nia's package model.
//!
//! This module deliberately stays outside the compiler and build runner. It
//! creates ordinary `nia.toml` packages and workspace members that the normal
//! metadata reader validates before they become visible to the caller.

use std::{
    fs::{self, OpenOptions},
    io::{self, Write},
    path::{Path, PathBuf},
    sync::atomic::{AtomicU64, Ordering},
};

use nia_package_metadata::{PackageManifest, WorkspaceManifest};
use toml_edit::{Array, DocumentMut, InlineTable, Item, Table, Value};

static TEMPORARY_ID: AtomicU64 = AtomicU64::new(0);

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct Artifacts {
    pub(crate) executable: bool,
    pub(crate) library: bool,
}

impl Artifacts {
    fn normalized(self) -> Self {
        if self.executable || self.library {
            self
        } else {
            Self {
                executable: true,
                library: false,
            }
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) enum DependencySpec {
    Path(String),
    Git {
        url: String,
        rev: Option<String>,
        tag: Option<String>,
        branch: Option<String>,
    },
    Registry {
        registry: String,
        package: String,
        version: String,
    },
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum DependencySection {
    Runtime,
    Build,
    Development,
    Optional,
    Lazy,
}

impl DependencySection {
    fn manifest_key(self) -> &'static str {
        match self {
            Self::Runtime => "dependencies",
            Self::Build => "build-dependencies",
            Self::Development => "dev-dependencies",
            Self::Optional => "optional-dependencies",
            Self::Lazy => "lazy-dependencies",
        }
    }
}

/// Creates a new standalone package or workspace in a previously absent path.
pub(crate) fn new_project(
    root: &Path,
    name: Option<&str>,
    artifacts: Artifacts,
    workspace: bool,
    members: &[String],
) -> Result<(), String> {
    if destination_exists(root) {
        return Err(format!("destination `{}` already exists", root.display()));
    }
    if !workspace && !members.is_empty() {
        return Err("`--member` is only valid with `--workspace`".to_owned());
    }
    if workspace {
        if !members.is_empty() && name.is_some() {
            return Err(
                "`--name` cannot be combined with explicit workspace `--member` names".to_owned(),
            );
        }
        let member_names = if members.is_empty() {
            vec![inferred_project_name(root, name, "path")?]
        } else {
            members.to_vec()
        };
        validate_unique_names(&member_names)?;
        create_new_tree(root, |staging| {
            populate_workspace(staging, &member_names, artifacts.normalized())
        })
    } else {
        let project_name = inferred_project_name(root, name, "path")?;
        create_new_tree(root, |staging| {
            populate_package(staging, &project_name, artifacts.normalized(), false, true)
        })
    }
}

/// Initializes a package or workspace in an existing directory without
/// replacing any user file.
pub(crate) fn init_project(
    root: &Path,
    name: Option<&str>,
    artifacts: Artifacts,
    workspace: bool,
    members: &[String],
) -> Result<(), String> {
    if !workspace && !members.is_empty() {
        return Err("`--member` is only valid with `--workspace`".to_owned());
    }
    if workspace && !members.is_empty() && name.is_some() {
        return Err(
            "`--name` cannot be combined with explicit workspace `--member` names".to_owned(),
        );
    }
    let requested_metadata = fs::symlink_metadata(root).map_err(|error| {
        format!(
            "cannot inspect project directory `{}`: {error}",
            root.display()
        )
    })?;
    if nia_compat::is_link_or_reparse_point(&requested_metadata) || !requested_metadata.is_dir() {
        return Err(format!(
            "project path `{}` is not a real directory",
            root.display()
        ));
    }
    let root = root.canonicalize().map_err(|error| {
        format!(
            "cannot resolve project directory `{}`: {error}",
            root.display()
        )
    })?;
    let root_metadata = fs::symlink_metadata(&root).map_err(|error| {
        format!(
            "cannot inspect project directory `{}`: {error}",
            root.display()
        )
    })?;
    if nia_compat::is_link_or_reparse_point(&root_metadata) || !root_metadata.is_dir() {
        return Err(format!(
            "project path `{}` is not a real directory",
            root.display()
        ));
    }
    if root.join("nia.toml").exists() {
        return Err(format!("`{}` already contains nia.toml", root.display()));
    }
    let member_names = if workspace {
        if members.is_empty() {
            vec![inferred_project_name(&root, name, "directory")?]
        } else {
            members.to_vec()
        }
    } else {
        vec![inferred_project_name(&root, name, "directory")?]
    };
    validate_unique_names(&member_names)?;

    let staging = create_temporary_directory(root.parent().unwrap_or(&root), "nia-init")?;
    let result = if workspace {
        populate_workspace(&staging, &member_names, artifacts.normalized())
    } else {
        populate_package(
            &staging,
            &member_names[0],
            artifacts.normalized(),
            false,
            true,
        )
    };
    if let Err(error) = result {
        let _ = fs::remove_dir_all(&staging);
        return Err(error);
    }
    let files = match collect_regular_files(&staging) {
        Ok(files) => files,
        Err(error) => {
            let _ = fs::remove_dir_all(&staging);
            return Err(error);
        }
    };
    let mut created_files = Vec::new();
    let mut created_dirs = Vec::new();
    for relative in &files {
        let destination = root.join(relative);
        if destination.exists() || fs::symlink_metadata(&destination).is_ok() {
            rollback_created(&created_files, &created_dirs);
            let _ = fs::remove_dir_all(&staging);
            return Err(format!("refusing to overwrite `{}`", destination.display()));
        }
    }
    for relative in &files {
        let source = staging.join(relative);
        let destination = root.join(relative);
        if let Some(parent) = destination.parent()
            && let Err(error) = create_real_directories_recording(parent, &root, &mut created_dirs)
        {
            rollback_created(&created_files, &created_dirs);
            let _ = fs::remove_dir_all(&staging);
            return Err(format!("cannot create `{}`: {error}", parent.display()));
        }
        match copy_file_new(&source, &destination) {
            Ok(()) => created_files.push(destination),
            Err(error) => {
                rollback_created(&created_files, &created_dirs);
                let _ = fs::remove_dir_all(&staging);
                return Err(format!(
                    "cannot create `{}`: {error}",
                    destination.display()
                ));
            }
        }
    }
    let _ = fs::remove_dir_all(&staging);
    Ok(())
}

/// Adds a package dependency while preserving unrelated TOML formatting and
/// comments. The package metadata reader validates the edited manifest before
/// the atomic replacement is committed.
pub(crate) fn add_dependency(
    root: Option<&Path>,
    name: &str,
    section: DependencySection,
    source: DependencySpec,
) -> Result<(), String> {
    validate_project_name(name)?;
    let package_root = discover_package_root(root)?;
    let package_layout = nia_package_metadata::BuildLayout::for_package(&package_root)
        .map_err(|error| format!("cannot resolve package state: {error}"))?;
    let _state_lock = package_layout
        .lock_state()
        .map_err(|error| format!("cannot lock package state: {error}"))?;
    PackageManifest::read(&package_root).map_err(|error| {
        format!(
            "`{}` is not a valid package root: {error}",
            package_root.display()
        )
    })?;
    let manifest_path = package_root.join("nia.toml");
    let original = fs::read_to_string(&manifest_path)
        .map_err(|error| format!("cannot read `{}`: {error}", manifest_path.display()))?;
    let mut document = original
        .parse::<DocumentMut>()
        .map_err(|error| format!("cannot parse `{}`: {error}", manifest_path.display()))?;

    for key in [
        "build-dependencies",
        "dependencies",
        "dev-dependencies",
        "optional-dependencies",
        "lazy-dependencies",
    ] {
        if document
            .get(key)
            .and_then(Item::as_table)
            .is_some_and(|table| table.contains_key(name))
        {
            return Err(format!(
                "dependency `{name}` is already declared in `{key}`"
            ));
        }
    }

    if let DependencySpec::Path(path) = &source {
        let dependency_root = package_root.join(path);
        PackageManifest::read(&dependency_root).map_err(|error| {
            format!(
                "path dependency `{name}` at `{}` is not a valid package: {error}",
                dependency_root.display()
            )
        })?;
        if fs::canonicalize(&dependency_root).ok() == fs::canonicalize(&package_root).ok() {
            return Err(format!("package `{name}` cannot depend on itself"));
        }
    }
    let dependency = dependency_item(source)?;
    let table = document
        .entry(section.manifest_key())
        .or_insert(Item::Table(Table::new()));
    let table = table
        .as_table_mut()
        .ok_or_else(|| format!("`{}` must be a TOML table", section.manifest_key()))?;
    table.insert(name, dependency);
    let updated = document.to_string();

    write_atomic(&manifest_path, updated.as_bytes())
        .map_err(|error| format!("cannot update `{}`: {error}", manifest_path.display()))?;
    if let Err(error) = PackageManifest::read(&package_root) {
        let _ = write_atomic(&manifest_path, original.as_bytes());
        return Err(format!("edited manifest is invalid: {error}"));
    }
    Ok(())
}

/// Creates a package under `packages/` and registers it as a workspace member.
/// Existing workspace patterns are honored; otherwise the explicit member
/// path is appended to the manifest.
pub(crate) fn add_workspace_member(
    root: Option<&Path>,
    name: &str,
    artifacts: Artifacts,
) -> Result<(), String> {
    validate_project_name(name)?;
    let workspace_root = discover_workspace_root(root)?;
    // The workspace root owns the shared build state and gives every member
    // the same package-state lock. Resolve it directly so the lock identity
    // does not depend on a potentially changing first-member list.
    let workspace_layout = nia_package_metadata::BuildLayout::for_package(&workspace_root)
        .map_err(|error| format!("cannot resolve workspace state: {error}"))?;
    let _state_lock = workspace_layout
        .lock_state()
        .map_err(|error| format!("cannot lock workspace state: {error}"))?;

    // Re-read all workspace inputs under the shared lock. In particular, a
    // concurrent member addition must be observed before choosing a path or
    // updating the manifest.
    let workspace = WorkspaceManifest::read(&workspace_root)
        .map_err(|error| format!("cannot read workspace: {error}"))?;
    let members_on_disk = workspace
        .member_packages()
        .map_err(|error| format!("cannot read workspace members: {error}"))?;
    if members_on_disk.iter().any(|member| member.name == name) {
        return Err(format!(
            "workspace already contains a package named `{name}`"
        ));
    }
    let relative = format!("packages/{name}");
    let destination = workspace_root.join(&relative);
    let manifest_path = workspace_root.join("nia.toml");
    let original = fs::read_to_string(&manifest_path)
        .map_err(|error| format!("cannot read `{}`: {error}", manifest_path.display()))?;
    let mut document = original
        .parse::<DocumentMut>()
        .map_err(|error| format!("cannot parse `{}`: {error}", manifest_path.display()))?;
    let covered_by_pattern = document
        .get("workspace")
        .and_then(Item::as_table)
        .and_then(|workspace| workspace.get("members"))
        .and_then(Item::as_array)
        .is_some_and(|members| {
            members.iter().any(|member| {
                member.as_str().is_some_and(|pattern| {
                    glob::Pattern::new(pattern)
                        .is_ok_and(|pattern| pattern.matches_path(Path::new(&relative)))
                })
            })
        });
    let workspace_table = document
        .get_mut("workspace")
        .and_then(Item::as_table_mut)
        .ok_or_else(|| "workspace manifest has no `[workspace]` table".to_owned())?;
    if let Some(members) = workspace_table.get_mut("members") {
        let members = members
            .as_array_mut()
            .ok_or_else(|| "workspace `[workspace].members` must be an array".to_owned())?;
        if !covered_by_pattern {
            members.push(relative.as_str());
        }
    } else {
        let mut members = Array::new();
        members.push(relative.as_str());
        workspace_table["members"] = value_item(members);
    }
    let updated = document.to_string();

    if destination_exists(&destination) {
        return Err(format!(
            "workspace member path `{}` already exists",
            destination.display()
        ));
    }
    let parent = destination.parent().expect("workspace member has a parent");
    create_real_directory(parent)?;
    let staging_parent = workspace_root.parent().unwrap_or(&workspace_root);
    let staging = create_temporary_directory(staging_parent, "nia-member")?;
    if let Err(error) = populate_package(&staging, name, artifacts.normalized(), true, false) {
        let _ = fs::remove_dir_all(&staging);
        return Err(error);
    }
    if let Err(error) = fs::rename(&staging, &destination) {
        let _ = fs::remove_dir_all(&staging);
        return Err(format!(
            "cannot create workspace member `{}`: {error}",
            destination.display()
        ));
    }
    if let Err(error) = if covered_by_pattern {
        Ok(())
    } else {
        write_atomic(&manifest_path, updated.as_bytes())
    } {
        return Err(rollback_added_member(
            &destination,
            &manifest_path,
            &original,
            covered_by_pattern,
            format!("cannot update workspace manifest: {error}"),
        ));
    }
    let verified_members =
        WorkspaceManifest::read(&workspace_root).and_then(|workspace| workspace.member_packages());
    let verified_members = match verified_members {
        Ok(members) => members,
        Err(error) => {
            return Err(rollback_added_member(
                &destination,
                &manifest_path,
                &original,
                covered_by_pattern,
                format!("updated workspace is invalid: {error}"),
            ));
        }
    };
    let canonical_destination = match fs::canonicalize(&destination) {
        Ok(path) => path,
        Err(error) => {
            return Err(rollback_added_member(
                &destination,
                &manifest_path,
                &original,
                covered_by_pattern,
                format!(
                    "cannot resolve new workspace member `{}`: {error}",
                    destination.display()
                ),
            ));
        }
    };
    if !verified_members
        .iter()
        .any(|member| member.name == name && member.root == canonical_destination)
    {
        return Err(rollback_added_member(
            &destination,
            &manifest_path,
            &original,
            covered_by_pattern,
            format!("workspace member `{relative}` is excluded by workspace configuration"),
        ));
    }
    Ok(())
}

fn rollback_added_member(
    destination: &Path,
    manifest_path: &Path,
    original_manifest: &str,
    manifest_was_unchanged: bool,
    primary_error: String,
) -> String {
    let mut rollback_errors = Vec::new();
    if !manifest_was_unchanged
        && let Err(error) = write_atomic(manifest_path, original_manifest.as_bytes())
    {
        rollback_errors.push(format!("cannot restore workspace manifest: {error}"));
    }
    if destination_exists(destination)
        && let Err(error) = fs::remove_dir_all(destination)
    {
        rollback_errors.push(format!(
            "cannot remove incomplete workspace member `{}`: {error}",
            destination.display()
        ));
    }
    if rollback_errors.is_empty() {
        primary_error
    } else {
        format!(
            "{primary_error}; rollback incomplete: {}",
            rollback_errors.join("; ")
        )
    }
}

fn dependency_item(source: DependencySpec) -> Result<Item, String> {
    let mut table = InlineTable::new();
    match source {
        DependencySpec::Path(path) => {
            table.insert("path", Value::from(path));
        }
        DependencySpec::Git {
            url,
            rev,
            tag,
            branch,
        } => {
            if [rev.as_ref(), tag.as_ref(), branch.as_ref()]
                .into_iter()
                .flatten()
                .count()
                > 1
            {
                return Err(
                    "a Git dependency accepts only one of `--rev`, `--tag`, or `--branch`"
                        .to_owned(),
                );
            }
            table.insert("git", Value::from(url));
            if let Some(value) = rev {
                table.insert("rev", Value::from(value));
            }
            if let Some(value) = tag {
                table.insert("tag", Value::from(value));
            }
            if let Some(value) = branch {
                table.insert("branch", Value::from(value));
            }
        }
        DependencySpec::Registry {
            registry,
            package,
            version,
        } => {
            if !package.contains('/') {
                return Err("registry package must use `namespace/name`".to_owned());
            }
            table.insert("registry", Value::from(registry));
            table.insert("package", Value::from(package));
            table.insert("version", Value::from(version));
        }
    }
    Ok(Item::Value(Value::InlineTable(table)))
}

fn populate_workspace(root: &Path, members: &[String], artifacts: Artifacts) -> Result<(), String> {
    let mut document = DocumentMut::new();
    let mut workspace = Table::new();
    let mut member_paths = Array::new();
    member_paths.push("packages/*");
    workspace["members"] = value_item(member_paths);
    document["workspace"] = Item::Table(workspace);
    let mut workspace_package = Table::new();
    workspace_package["namespace"] = value_item("local");
    workspace_package["version"] = value_item("0.1.0");
    workspace_package["protocol-epoch"] = value_item(nia_package_metadata::PROTOCOL_EPOCH as i64);
    document["workspace"]["package"] = Item::Table(workspace_package);
    create_real_directory(root)?;
    write_new_file(&root.join("nia.toml"), document.to_string().as_bytes())?;
    for member in members {
        let member_root = root.join("packages").join(member);
        create_real_directory(&member_root)?;
        populate_package(&member_root, member, artifacts, true, false)?;
    }
    WorkspaceManifest::read(root)
        .map_err(|error| format!("generated workspace is invalid: {error}"))?;
    Ok(())
}

fn populate_package(
    root: &Path,
    name: &str,
    artifacts: Artifacts,
    inherit_workspace: bool,
    validate: bool,
) -> Result<(), String> {
    validate_project_name(name)?;
    create_real_directory(root)?;
    let mut document = DocumentMut::new();
    let mut package = Table::new();
    package["name"] = value_item(name);
    if inherit_workspace {
        package["namespace"] = inheritable_workspace_item();
        package["version"] = inheritable_workspace_item();
        package["protocol-epoch"] = inheritable_workspace_item();
    } else {
        package["version"] = value_item("0.1.0");
        package["protocol-epoch"] = value_item(nia_package_metadata::PROTOCOL_EPOCH as i64);
    }
    document["package"] = Item::Table(package);
    if artifacts.library {
        let mut library = Table::new();
        library["path"] = value_item("lib/root.nia");
        document["library"] = Item::Table(library);
    }
    if artifacts.executable {
        let mut executable = Table::new();
        executable["name"] = value_item(name);
        executable["path"] = value_item("src/main.nia");
        let mut entries = toml_edit::ArrayOfTables::new();
        entries.push(executable);
        document["executable"] = Item::ArrayOfTables(entries);
    }

    write_new_file(&root.join("nia.toml"), document.to_string().as_bytes())?;
    if artifacts.library {
        write_new_file(
            &root.join("lib/root.nia"),
            b"// Public library entry point. Add exported modules and functions here.\n",
        )?;
    }
    if artifacts.executable {
        write_new_file(&root.join("src/main.nia"), executable_source().as_bytes())?;
    }
    if validate {
        PackageManifest::read_standalone(root)
            .map_err(|error| format!("generated package `{name}` is invalid: {error}"))?;
    }
    Ok(())
}

fn executable_source() -> String {
    "using std::io;\nusing std::process;\n\npub fun main(init: process::Init) process::ExitCode!() {\n    _ = init;\n    io::debugPrint(\"hello from nia\\n\", []).?;\n    !()\n}\n".to_owned()
}

fn value_item(value: impl Into<Value>) -> Item {
    Item::Value(value.into())
}

fn inheritable_workspace_item() -> Item {
    let mut table = InlineTable::new();
    table.insert("workspace", Value::from(true));
    Item::Value(Value::InlineTable(table))
}

fn destination_exists(path: &Path) -> bool {
    path.exists() || fs::symlink_metadata(path).is_ok()
}

fn create_new_tree(
    root: &Path,
    populate: impl FnOnce(&Path) -> Result<(), String>,
) -> Result<(), String> {
    let root = if root.is_absolute() {
        root.to_path_buf()
    } else {
        std::env::current_dir()
            .map_err(|error| format!("cannot determine current directory: {error}"))?
            .join(root)
    };
    if root.exists() || fs::symlink_metadata(&root).is_ok() {
        return Err(format!("destination `{}` already exists", root.display()));
    }
    let parent = root
        .parent()
        .filter(|parent| !parent.as_os_str().is_empty())
        .unwrap_or_else(|| Path::new("."));
    create_real_directory(parent).map_err(|error| {
        format!(
            "cannot create destination parent `{}`: {error}",
            parent.display()
        )
    })?;
    let staging = create_temporary_directory(parent, "nia-new")?;
    if let Err(error) = populate(&staging) {
        let _ = fs::remove_dir_all(&staging);
        return Err(error);
    }
    if root.exists() || fs::symlink_metadata(&root).is_ok() {
        let _ = fs::remove_dir_all(&staging);
        return Err(format!("destination `{}` already exists", root.display()));
    }
    fs::rename(&staging, &root).map_err(|error| {
        let _ = fs::remove_dir_all(&staging);
        format!("cannot create project `{}`: {error}", root.display())
    })
}

fn create_temporary_directory(parent: &Path, label: &str) -> Result<PathBuf, String> {
    create_real_directory(parent).map_err(|error| {
        format!(
            "cannot create staging parent `{}`: {error}",
            parent.display()
        )
    })?;
    for _ in 0..128 {
        let id = TEMPORARY_ID.fetch_add(1, Ordering::Relaxed);
        let path = parent.join(format!(".{label}-{}-{id}", std::process::id()));
        match fs::create_dir(&path) {
            Ok(()) => return Ok(path),
            Err(error) if error.kind() == io::ErrorKind::AlreadyExists => continue,
            Err(error) => return Err(format!("cannot create staging directory: {error}")),
        }
    }
    Err("cannot allocate a unique staging directory".to_owned())
}

fn create_real_directory(path: &Path) -> Result<(), String> {
    nia_compat::create_real_directory(path)
        .map_err(|error| format!("cannot create real directory `{}`: {error}", path.display()))
}

fn write_new_file(path: &Path, bytes: &[u8]) -> Result<(), String> {
    if let Some(parent) = path.parent() {
        create_real_directory(parent)?;
    }
    let mut file = OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(path)
        .map_err(|error| format!("cannot create `{}`: {error}", path.display()))?;
    file.write_all(bytes)
        .map_err(|error| format!("cannot write `{}`: {error}", path.display()))?;
    file.sync_all()
        .map_err(|error| format!("cannot finish `{}`: {error}", path.display()))
}

fn copy_file_new(source: &Path, destination: &Path) -> io::Result<()> {
    let mut input = fs::File::open(source)?;
    let mut output = OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(destination)?;
    io::copy(&mut input, &mut output)?;
    output.sync_all()
}

fn collect_regular_files(root: &Path) -> Result<Vec<PathBuf>, String> {
    fn walk(root: &Path, current: &Path, files: &mut Vec<PathBuf>) -> Result<(), String> {
        for entry in fs::read_dir(current)
            .map_err(|error| format!("cannot inspect staging directory: {error}"))?
        {
            let entry = entry.map_err(|error| format!("cannot inspect staging entry: {error}"))?;
            let path = entry.path();
            let metadata = fs::symlink_metadata(&path)
                .map_err(|error| format!("cannot inspect `{}`: {error}", path.display()))?;
            if nia_compat::is_link_or_reparse_point(&metadata) {
                return Err(format!(
                    "staging tree unexpectedly contains a link: `{}`",
                    path.display()
                ));
            }
            if metadata.is_dir() {
                walk(root, &path, files)?;
            } else if metadata.is_file() {
                files.push(
                    path.strip_prefix(root)
                        .expect("staging descendant")
                        .to_path_buf(),
                );
            } else {
                return Err(format!(
                    "staging tree contains a non-file entry: `{}`",
                    path.display()
                ));
            }
        }
        Ok(())
    }
    let mut files = Vec::new();
    walk(root, root, &mut files)?;
    files.sort();
    Ok(files)
}

fn create_real_directories_recording(
    path: &Path,
    root: &Path,
    created: &mut Vec<PathBuf>,
) -> io::Result<()> {
    let relative = path.strip_prefix(root).map_err(|_| {
        io::Error::new(
            io::ErrorKind::InvalidInput,
            "directory escaped project root",
        )
    })?;
    let mut current = root.to_path_buf();
    for component in relative.components() {
        current.push(component);
        match fs::create_dir(&current) {
            Ok(()) => created.push(current.clone()),
            Err(error) if error.kind() == io::ErrorKind::AlreadyExists => {
                let metadata = fs::symlink_metadata(&current)?;
                if nia_compat::is_link_or_reparse_point(&metadata) || !metadata.is_dir() {
                    return Err(io::Error::new(
                        io::ErrorKind::InvalidInput,
                        format!("`{}` is not a real directory", current.display()),
                    ));
                }
            }
            Err(error) => return Err(error),
        }
    }
    Ok(())
}

fn rollback_created(files: &[PathBuf], directories: &[PathBuf]) {
    for file in files.iter().rev() {
        let _ = fs::remove_file(file);
    }
    for directory in directories.iter().rev() {
        let _ = fs::remove_dir(directory);
    }
}

fn path_name(path: &Path) -> Option<String> {
    path.file_name()?.to_str().map(str::to_owned)
}

fn inferred_project_name(
    root: &Path,
    name: Option<&str>,
    location: &str,
) -> Result<String, String> {
    let name = name
        .map(str::to_owned)
        .or_else(|| path_name(root))
        .ok_or_else(|| {
            format!("cannot infer a package name from this {location}; pass `--name`")
        })?;
    validate_project_name(&name)?;
    Ok(name)
}

fn validate_project_name(name: &str) -> Result<(), String> {
    let mut bytes = name.bytes();
    let valid_first = bytes
        .next()
        .is_some_and(|byte| byte.is_ascii_alphabetic() || byte == b'_');
    if !valid_first
        || name.len() > 128
        || !bytes.all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'_' | b'-'))
    {
        return Err(format!(
            "invalid package name `{name}`; use an ASCII letter or `_` followed by letters, digits, `_`, or `-`"
        ));
    }
    Ok(())
}

fn validate_unique_names(names: &[String]) -> Result<(), String> {
    let mut seen = std::collections::BTreeSet::new();
    for name in names {
        validate_project_name(name)?;
        if !seen.insert(name) {
            return Err(format!("duplicate workspace member name `{name}`"));
        }
    }
    if names.is_empty() {
        return Err("a new workspace needs at least one initial package member".to_owned());
    }
    Ok(())
}

fn discover_package_root(root: Option<&Path>) -> Result<PathBuf, String> {
    let start = root
        .map(Path::to_path_buf)
        .unwrap_or_else(|| std::env::current_dir().unwrap_or_else(|_| PathBuf::from(".")));
    nia_package_metadata::BuildLayout::discover(&start)
        .map(|layout| layout.package_root().to_path_buf())
        .map_err(|error| format!("cannot find package from `{}`: {error}", start.display()))
}

fn discover_workspace_root(root: Option<&Path>) -> Result<PathBuf, String> {
    let start = root
        .map(Path::to_path_buf)
        .unwrap_or_else(|| std::env::current_dir().unwrap_or_else(|_| PathBuf::from(".")));
    WorkspaceManifest::discover(&start)
        .map_err(|error| format!("cannot find workspace from `{}`: {error}", start.display()))?
        .map(|workspace| workspace.root)
        .ok_or_else(|| format!("no workspace found from `{}`", start.display()))
}

fn write_atomic(path: &Path, bytes: &[u8]) -> io::Result<()> {
    let parent = path.parent().unwrap_or_else(|| Path::new("."));
    nia_compat::create_real_directory(parent)?;
    for _ in 0..128 {
        let id = TEMPORARY_ID.fetch_add(1, Ordering::Relaxed);
        let temporary = parent.join(format!(".nia-manifest-{}-{id}.tmp", std::process::id()));
        let mut file = match OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&temporary)
        {
            Ok(file) => file,
            Err(error) if error.kind() == io::ErrorKind::AlreadyExists => continue,
            Err(error) => return Err(error),
        };
        let result = file.write_all(bytes).and_then(|()| file.sync_all());
        drop(file);
        if let Err(error) = result {
            let _ = fs::remove_file(&temporary);
            return Err(error);
        }
        if let Err(error) = nia_compat::replace_path(&temporary, path) {
            let _ = fs::remove_file(&temporary);
            return Err(error);
        }
        return Ok(());
    }
    Err(io::Error::new(
        io::ErrorKind::AlreadyExists,
        "cannot allocate a unique manifest temporary file",
    ))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::{SystemTime, UNIX_EPOCH};

    fn temporary_root(label: &str) -> PathBuf {
        let nonce = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .expect("clock after epoch")
            .as_nanos();
        let path = std::env::temp_dir().join(format!(
            "nia-project-{label}-{}-{nonce}",
            std::process::id()
        ));
        fs::create_dir_all(&path).expect("create test root");
        path
    }

    #[test]
    fn package_templates_validate_as_native_package_manifests() {
        for artifacts in [
            Artifacts {
                executable: true,
                library: false,
            },
            Artifacts {
                executable: false,
                library: true,
            },
            Artifacts {
                executable: true,
                library: true,
            },
        ] {
            let root = temporary_root("package").join("demo");
            new_project(&root, Some("demo"), artifacts, false, &[]).unwrap();
            let manifest = PackageManifest::read_standalone(&root).unwrap();
            assert_eq!(manifest.library_path.is_some(), artifacts.library);
            assert_eq!(
                manifest.artifacts.iter().any(|artifact| artifact.kind == nia_package_metadata::ArtifactKind::Executable),
                artifacts.executable
            );
            let _ = fs::remove_dir_all(root.parent().unwrap());
        }
    }

    #[test]
    fn workspace_template_has_a_real_initial_member_and_shared_identity() {
        let parent = temporary_root("workspace");
        let root = parent.join("mono");
        new_project(
            &root,
            None,
            Artifacts {
                executable: true,
                library: false,
            },
            true,
            &["app".to_owned(), "core".to_owned()],
        )
        .unwrap();
        let workspace = WorkspaceManifest::read(&root).unwrap();
        assert_eq!(workspace.members, ["packages/app", "packages/core"]);
        assert_eq!(workspace.member_packages().unwrap().len(), 2);
        add_workspace_member(
            Some(&root),
            "tools",
            Artifacts {
                executable: false,
                library: true,
            },
        )
        .unwrap();
        let updated = WorkspaceManifest::read(&root).unwrap();
        assert_eq!(
            updated.members,
            ["packages/app", "packages/core", "packages/tools"]
        );
        assert_eq!(updated.member_packages().unwrap().len(), 3);
        let manifest = PackageManifest::read(&root.join("packages/tools")).unwrap();
        assert_eq!(manifest.library_path.as_deref(), Some("lib/root.nia"));
        assert!(
            add_workspace_member(
                Some(&root),
                "tools",
                Artifacts {
                    executable: false,
                    library: true,
                },
            )
            .is_err()
        );
        let _ = fs::remove_dir_all(parent);
    }

    #[test]
    fn workspace_member_updates_explicit_paths_and_supports_both_artifacts() {
        let parent = temporary_root("workspace-explicit");
        let root = parent.join("mono");
        new_project(
            &root,
            None,
            Artifacts {
                executable: true,
                library: false,
            },
            true,
            &["app".to_owned()],
        )
        .unwrap();
        let manifest_path = root.join("nia.toml");
        let manifest = fs::read_to_string(&manifest_path).unwrap();
        fs::write(
            &manifest_path,
            manifest.replace("members = [\"packages/*\"]", "members = [\"packages/app\"]"),
        )
        .unwrap();

        add_workspace_member(
            Some(&root),
            "core",
            Artifacts {
                executable: true,
                library: true,
            },
        )
        .unwrap();

        let workspace = WorkspaceManifest::read(&root).unwrap();
        assert_eq!(workspace.members, ["packages/app", "packages/core"]);
        let member = PackageManifest::read(&root.join("packages/core")).unwrap();
        assert!(member.library_path.is_some());
        assert_eq!(member.artifacts.len(), 1);
        assert!(root.join("packages/core/lib/root.nia").is_file());
        assert!(root.join("packages/core/src/main.nia").is_file());
        let _ = fs::remove_dir_all(parent);
    }

    #[test]
    fn concurrent_workspace_member_additions_are_serialized_without_lost_updates() {
        let parent = temporary_root("workspace-concurrent");
        let root = parent.join("mono");
        new_project(
            &root,
            None,
            Artifacts {
                executable: true,
                library: false,
            },
            true,
            &["app".to_owned()],
        )
        .unwrap();

        std::thread::scope(|scope| {
            let first_root = root.clone();
            let second_root = root.clone();
            let first = scope.spawn(move || {
                add_workspace_member(
                    Some(&first_root),
                    "first",
                    Artifacts {
                        executable: true,
                        library: false,
                    },
                )
            });
            let second = scope.spawn(move || {
                add_workspace_member(
                    Some(&second_root),
                    "second",
                    Artifacts {
                        executable: false,
                        library: true,
                    },
                )
            });
            first.join().unwrap().unwrap();
            second.join().unwrap().unwrap();
        });

        let workspace = WorkspaceManifest::read(&root).unwrap();
        let members = workspace.member_packages().unwrap();
        assert_eq!(members.len(), 3);
        assert!(members.iter().any(|member| member.name == "first"));
        assert!(members.iter().any(|member| member.name == "second"));
        let _ = fs::remove_dir_all(parent);
    }

    #[test]
    fn excluded_workspace_member_creation_rolls_back_its_directory() {
        let parent = temporary_root("workspace-excluded");
        let root = parent.join("mono");
        new_project(
            &root,
            None,
            Artifacts {
                executable: true,
                library: false,
            },
            true,
            &["app".to_owned()],
        )
        .unwrap();
        let manifest_path = root.join("nia.toml");
        let manifest = fs::read_to_string(&manifest_path).unwrap();
        let manifest = manifest.replace(
            "members = [\"packages/*\"]",
            "members = [\"packages/*\"]\nexclude = [\"packages/core\"]",
        );
        fs::write(&manifest_path, &manifest).unwrap();

        assert!(
            add_workspace_member(
                Some(&root),
                "core",
                Artifacts {
                    executable: true,
                    library: false,
                },
            )
            .is_err()
        );
        assert!(!root.join("packages/core").exists());
        assert_eq!(fs::read_to_string(&manifest_path).unwrap(), manifest);
        let _ = fs::remove_dir_all(parent);
    }

    #[test]
    fn init_preserves_existing_files_and_refuses_template_collisions() {
        let root = temporary_root("init");
        fs::write(root.join("notes.txt"), "keep me").unwrap();
        init_project(
            &root,
            Some("demo"),
            Artifacts {
                executable: true,
                library: false,
            },
            false,
            &[],
        )
        .unwrap();
        assert_eq!(
            fs::read_to_string(root.join("notes.txt")).unwrap(),
            "keep me"
        );
        assert!(
            init_project(
                &root,
                Some("demo"),
                Artifacts {
                    executable: true,
                    library: false
                },
                false,
                &[]
            )
            .is_err()
        );
        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn init_dot_uses_the_canonical_directory_name() {
        let parent = temporary_root("init-dot");
        let root = parent.join("demo");
        fs::create_dir(&root).unwrap();
        init_project(
            &root.join("."),
            None,
            Artifacts {
                executable: false,
                library: true,
            },
            false,
            &[],
        )
        .unwrap();
        let package = PackageManifest::read_standalone(&root).unwrap();
        assert_eq!(package.package.name, "demo");
        assert!(root.join("lib/root.nia").is_file());
        let _ = fs::remove_dir_all(parent);
    }

    #[test]
    fn default_template_is_an_executable_named_from_its_destination() {
        let parent = temporary_root("default-template");
        let root = parent.join("hello");
        new_project(
            &root,
            None,
            Artifacts {
                executable: false,
                library: false,
            },
            false,
            &[],
        )
        .unwrap();
        let package = PackageManifest::read_standalone(&root).unwrap();
        assert_eq!(package.package.name, "hello");
        assert_eq!(package.artifacts.len(), 1);
        assert_eq!(package.artifacts[0].name, "hello");
        assert!(root.join("src/main.nia").is_file());
        assert!(!root.join("lib/root.nia").exists());
        let _ = fs::remove_dir_all(parent);
    }

    #[test]
    fn add_dependency_preserves_comments_and_validates_the_result() {
        let parent = temporary_root("add");
        let root = parent.join("app");
        new_project(
            &root,
            Some("app"),
            Artifacts {
                executable: true,
                library: false,
            },
            false,
            &[],
        )
        .unwrap();
        let dependency = parent.join("helper");
        new_project(
            &dependency,
            Some("helper"),
            Artifacts {
                executable: false,
                library: true,
            },
            false,
            &[],
        )
        .unwrap();
        let manifest_path = root.join("nia.toml");
        let original = fs::read_to_string(&manifest_path).unwrap();
        fs::write(&manifest_path, format!("# package comment\n{original}")).unwrap();
        add_dependency(
            Some(&root),
            "helper",
            DependencySection::Runtime,
            DependencySpec::Path("../helper".to_owned()),
        )
        .unwrap();
        let updated = fs::read_to_string(&manifest_path).unwrap();
        assert!(updated.starts_with("# package comment"));
        assert!(updated.contains("[dependencies]"));
        assert!(updated.contains("helper = { path = \"../helper\" }"));
        assert!(
            add_dependency(
                Some(&root),
                "helper",
                DependencySection::Runtime,
                DependencySpec::Path("../helper".to_owned())
            )
            .is_err()
        );
        assert!(
            add_dependency(
                Some(&root),
                "missing",
                DependencySection::Runtime,
                DependencySpec::Path("../missing".to_owned()),
            )
            .is_err()
        );
        let _ = fs::remove_dir_all(parent);
    }
}
