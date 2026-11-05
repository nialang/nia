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

use nia_package_metadata::{PackageManifest, WorkspaceManifest, WorkspacePackageFields};
use toml_edit::{Array, DocumentMut, InlineTable, Item, Table, Value};

static TEMPORARY_ID: AtomicU64 = AtomicU64::new(0);
const GITIGNORE_TEMPLATE: &str = "/build/\n";

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
                library: true,
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
        if workspace_target(root)?.is_some() {
            return Err("cannot create a workspace nested inside another workspace".to_owned());
        }
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
        if let Some(target) = workspace_target(root)? {
            return create_workspace_package(&project_name, artifacts.normalized(), target);
        }
        create_new_tree(root, |staging| {
            populate_package(staging, &project_name, artifacts.normalized(), None, true)
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
    let containing_workspace = workspace_target(&root)?;
    if workspace && containing_workspace.is_some() {
        return Err("cannot create a workspace nested inside another workspace".to_owned());
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

    // Hold the shared workspace lock until both the complete package and its
    // membership are published, just as `new` does.
    let _workspace_lock = containing_workspace
        .as_ref()
        .map(WorkspaceTarget::lock_state)
        .transpose()?;
    let shared_package = containing_workspace
        .as_ref()
        .map(|target| target.package_fields(&member_names[0]))
        .transpose()?;
    let staging_parent = containing_workspace
        .as_ref()
        .and_then(|workspace| workspace.root.parent())
        .unwrap_or_else(|| root.parent().unwrap_or(&root));
    let staging = create_temporary_directory(staging_parent, "nia-init")?;
    let result = if workspace {
        populate_workspace(&staging, &member_names, artifacts.normalized())
    } else {
        populate_package(
            &staging,
            &member_names[0],
            artifacts.normalized(),
            shared_package.as_ref(),
            containing_workspace.is_none(),
        )
    };
    if let Err(error) = result {
        let _ = fs::remove_dir_all(&staging);
        return Err(error);
    }
    let mut files = match collect_regular_files(&staging) {
        Ok(files) => files,
        Err(error) => {
            let _ = fs::remove_dir_all(&staging);
            return Err(error);
        }
    };
    files.sort_by_key(|path| path == Path::new("nia.toml"));
    // `init` adds Nia's ignore rule when no ignore file exists, while leaving
    // repository-specific ignore policy entirely under the user's control.
    files.retain(|relative| {
        relative != Path::new(".gitignore") || !destination_exists(&root.join(relative))
    });
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
    if let Some(target) = containing_workspace
        && let Err(error) = register_workspace_member(&root, &member_names[0], &target)
    {
        if !error.preserve_package {
            rollback_created(&created_files, &created_dirs);
        }
        let _ = fs::remove_dir_all(&staging);
        return Err(if error.preserve_package {
            format!(
                "{}; generated package files remain at `{}`",
                error.message,
                root.display()
            )
        } else {
            error.message
        });
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

struct WorkspaceTarget {
    root: PathBuf,
    relative: String,
}

impl WorkspaceTarget {
    fn lock_state(&self) -> Result<nia_package_metadata::PackageStateLock, String> {
        nia_package_metadata::BuildLayout::for_package(&self.root)
            .map_err(|error| format!("cannot resolve workspace state: {error}"))?
            .lock_state()
            .map_err(|error| format!("cannot lock workspace state: {error}"))
    }

    fn package_fields(&self, name: &str) -> Result<WorkspacePackageFields, String> {
        let workspace = WorkspaceManifest::read(&self.root)
            .map_err(|error| format!("cannot read workspace: {error}"))?;
        let members = workspace
            .member_packages()
            .map_err(|error| format!("cannot read workspace members: {error}"))?;
        if members.iter().any(|member| member.name == name) {
            return Err(format!(
                "workspace already contains a package named `{name}`"
            ));
        }
        if workspace.package.namespace.is_none()
            || workspace.package.version.is_none()
            || workspace.package.protocol_epoch.is_none()
        {
            return Err(
                "workspace package identity is incomplete; define namespace, version, and protocol-epoch before creating a member"
                    .to_owned(),
            );
        }
        Ok(workspace.package)
    }
}

fn absolute_path(path: &Path) -> Result<PathBuf, String> {
    let absolute = if path.is_absolute() {
        path.to_path_buf()
    } else {
        std::env::current_dir()
            .map_err(|error| format!("cannot determine current directory: {error}"))?
            .join(path)
    };
    let mut normalized = PathBuf::new();
    for component in absolute.components() {
        match component {
            std::path::Component::Prefix(prefix) => normalized.push(prefix.as_os_str()),
            std::path::Component::RootDir => normalized.push(component.as_os_str()),
            std::path::Component::CurDir => {}
            std::path::Component::ParentDir => {
                if !normalized.pop() {
                    return Err(format!("path `{}` escapes its root", path.display()));
                }
            }
            std::path::Component::Normal(segment) => normalized.push(segment),
        }
    }
    Ok(normalized)
}

fn workspace_target(path: &Path) -> Result<Option<WorkspaceTarget>, String> {
    let absolute = absolute_path(path)?;
    let mut existing = absolute.as_path();
    while !existing.exists() {
        existing = existing.parent().ok_or_else(|| {
            format!(
                "cannot find an existing parent for `{}`",
                absolute.display()
            )
        })?;
    }
    let existing_metadata = fs::symlink_metadata(existing)
        .map_err(|error| format!("cannot inspect `{}`: {error}", existing.display()))?;
    if nia_compat::is_link_or_reparse_point(&existing_metadata) || !existing_metadata.is_dir() {
        return Err(format!(
            "project parent `{}` is not a real directory",
            existing.display()
        ));
    }
    let canonical_existing = fs::canonicalize(existing)
        .map_err(|error| format!("cannot resolve `{}`: {error}", existing.display()))?;
    let unresolved = absolute
        .strip_prefix(existing)
        .map_err(|_| format!("cannot resolve project path `{}`", absolute.display()))?;
    let candidate = canonical_existing.join(unresolved);
    let Some(workspace) = WorkspaceManifest::discover(&canonical_existing)
        .map_err(|error| format!("cannot discover workspace: {error}"))?
    else {
        return Ok(None);
    };
    let relative_path = candidate.strip_prefix(&workspace.root).map_err(|_| {
        format!(
            "project path `{}` is outside its workspace",
            candidate.display()
        )
    })?;
    let relative = relative_path
        .to_str()
        .ok_or_else(|| {
            format!(
                "workspace package path `{}` is not UTF-8",
                candidate.display()
            )
        })?
        .replace('\\', "/");
    if relative.is_empty() || relative == "." {
        return Ok(None);
    }
    if workspace.is_excluded_path(&relative) {
        return Ok(None);
    }
    Ok(Some(WorkspaceTarget {
        root: workspace.root,
        relative,
    }))
}

fn add_explicit_workspace_member(manifest: &mut DocumentMut, relative: &str) -> Result<(), String> {
    let workspace = manifest
        .get_mut("workspace")
        .and_then(Item::as_table_mut)
        .ok_or_else(|| "workspace manifest has no `[workspace]` table".to_owned())?;
    let members = workspace
        .entry("members")
        .or_insert_with(|| value_item(Array::new()))
        .as_array_mut()
        .ok_or_else(|| "workspace `[workspace].members` must be an array".to_owned())?;
    members.push(relative);
    Ok(())
}

fn ensure_workspace_member(root: &Path, relative: &str) -> Result<Option<String>, String> {
    let workspace =
        WorkspaceManifest::read(root).map_err(|error| format!("cannot read workspace: {error}"))?;
    if workspace
        .member_pattern_matches_path(relative)
        .map_err(|error| format!("cannot resolve workspace member pattern: {error}"))?
    {
        return Ok(None);
    }
    let manifest_path = root.join("nia.toml");
    let original = fs::read_to_string(&manifest_path)
        .map_err(|error| format!("cannot read `{}`: {error}", manifest_path.display()))?;
    let mut manifest = original
        .parse::<DocumentMut>()
        .map_err(|error| format!("cannot parse `{}`: {error}", manifest_path.display()))?;
    add_explicit_workspace_member(&mut manifest, relative)?;
    write_atomic(&manifest_path, manifest.to_string().as_bytes())
        .map_err(|error| format!("cannot update workspace manifest: {error}"))?;
    Ok(Some(original))
}

fn create_workspace_package(
    name: &str,
    artifacts: Artifacts,
    target: WorkspaceTarget,
) -> Result<(), String> {
    let _lock = target.lock_state()?;
    let shared_package = target.package_fields(name)?;
    let destination = target.root.join(&target.relative);
    if destination_exists(&destination) {
        return Err(format!(
            "destination `{}` already exists",
            destination.display()
        ));
    }
    let parent = destination
        .parent()
        .ok_or_else(|| format!("project path `{}` has no parent", destination.display()))?;
    let staging_parent = target.root.parent().unwrap_or(&target.root);
    let staging = create_temporary_directory(staging_parent, "nia-new-member")?;
    if let Err(error) = populate_package(&staging, name, artifacts, Some(&shared_package), false) {
        let _ = fs::remove_dir_all(&staging);
        return Err(error);
    }
    let mut created_dirs = Vec::new();
    let mut created_files = Vec::new();
    if let Err(error) = create_real_directories_recording(parent, &target.root, &mut created_dirs) {
        let _ = fs::remove_dir_all(&staging);
        rollback_created(&[], &created_dirs);
        return Err(format!("cannot create `{}`: {error}", parent.display()));
    }
    if let Err(error) = fs::create_dir(&destination) {
        let _ = fs::remove_dir_all(&staging);
        rollback_created(&[], &created_dirs);
        return Err(format!(
            "cannot create project directory `{}`: {error}",
            destination.display()
        ));
    }
    created_dirs.push(destination.clone());
    let mut files = match collect_regular_files(&staging) {
        Ok(files) => files,
        Err(error) => {
            let _ = fs::remove_dir_all(&staging);
            rollback_created(&created_files, &created_dirs);
            return Err(error);
        }
    };
    files.sort_by_key(|path| path == Path::new("nia.toml"));
    for relative in files {
        let source = staging.join(&relative);
        let target_file = destination.join(&relative);
        if let Some(parent) = target_file.parent()
            && let Err(error) =
                create_real_directories_recording(parent, &target.root, &mut created_dirs)
        {
            rollback_created(&created_files, &created_dirs);
            let _ = fs::remove_dir_all(&staging);
            return Err(format!("cannot create `{}`: {error}", parent.display()));
        }
        match copy_file_new(&source, &target_file) {
            Ok(()) => created_files.push(target_file),
            Err(error) => {
                rollback_created(&created_files, &created_dirs);
                let _ = fs::remove_dir_all(&staging);
                return Err(format!(
                    "cannot create `{}`: {error}",
                    target_file.display()
                ));
            }
        }
    }
    let _ = fs::remove_dir_all(&staging);
    let destination_root = match fs::canonicalize(&destination) {
        Ok(root) => root,
        Err(error) => {
            rollback_created(&created_files, &created_dirs);
            return Err(format!(
                "cannot resolve generated package `{}`: {error}",
                target.relative
            ));
        }
    };
    if let Err(error) = register_workspace_member(&destination_root, name, &target) {
        if !error.preserve_package {
            rollback_created(&created_files, &created_dirs);
        }
        return Err(if error.preserve_package {
            format!(
                "{}; generated package files remain at `{}`",
                error.message,
                destination.display()
            )
        } else {
            error.message
        });
    }
    Ok(())
}

struct WorkspaceMemberRegistrationError {
    message: String,
    preserve_package: bool,
}

impl WorkspaceMemberRegistrationError {
    fn rollback(message: String) -> Self {
        Self {
            message,
            preserve_package: false,
        }
    }
}

/// Registers a complete package while the caller holds the workspace lock.
fn register_workspace_member(
    package_root: &Path,
    name: &str,
    target: &WorkspaceTarget,
) -> Result<(), WorkspaceMemberRegistrationError> {
    let manifest_path = target.root.join("nia.toml");
    let original = ensure_workspace_member(&target.root, &target.relative)
        .map_err(WorkspaceMemberRegistrationError::rollback)?;
    let verified = WorkspaceManifest::read(&target.root)
        .and_then(|workspace| workspace.member_packages())
        .map_err(|error| format!("updated workspace is invalid: {error}"));
    match verified {
        Ok(members)
            if members
                .iter()
                .any(|member| member.name == name && member.root == package_root) =>
        {
            Ok(())
        }
        Ok(_) => {
            let message = format!("workspace member `{}` was not registered", target.relative);
            Err(restore_registration_failure(
                message,
                &manifest_path,
                original.as_deref(),
            ))
        }
        Err(error) => Err(restore_registration_failure(
            error.to_string(),
            &manifest_path,
            original.as_deref(),
        )),
    }
}

fn restore_registration_failure(
    message: String,
    manifest_path: &Path,
    original: Option<&str>,
) -> WorkspaceMemberRegistrationError {
    if let Some(original) = original
        && let Err(error) = write_atomic(manifest_path, original.as_bytes())
    {
        return WorkspaceMemberRegistrationError {
            message: format!("{message}; the workspace manifest could not be restored: {error}"),
            preserve_package: true,
        };
    }
    WorkspaceMemberRegistrationError::rollback(message)
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
    let workspace_fields = WorkspacePackageFields {
        namespace: Some("local".to_owned()),
        version: Some("0.1.0".to_owned()),
        protocol_epoch: Some(nia_package_metadata::PROTOCOL_EPOCH),
    };
    create_real_directory(root)?;
    write_new_file(&root.join("nia.toml"), document.to_string().as_bytes())?;
    write_new_file(&root.join(".gitignore"), GITIGNORE_TEMPLATE.as_bytes())?;
    for member in members {
        let member_root = root.join("packages").join(member);
        create_real_directory(&member_root)?;
        populate_package(
            &member_root,
            member,
            artifacts,
            Some(&workspace_fields),
            false,
        )?;
    }
    WorkspaceManifest::read(root)
        .map_err(|error| format!("generated workspace is invalid: {error}"))?;
    Ok(())
}

fn populate_package(
    root: &Path,
    name: &str,
    artifacts: Artifacts,
    workspace: Option<&WorkspacePackageFields>,
    validate: bool,
) -> Result<(), String> {
    validate_project_name(name)?;
    create_real_directory(root)?;
    let mut document = DocumentMut::new();
    let mut package = Table::new();
    package["name"] = value_item(name);
    if workspace.is_some() {
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
    let mut test = Table::new();
    test["name"] = value_item("unit");
    test["path"] = value_item("tests/unit.nia");
    let mut tests = toml_edit::ArrayOfTables::new();
    tests.push(test);
    document["test"] = Item::ArrayOfTables(tests);

    write_new_file(&root.join("nia.toml"), document.to_string().as_bytes())?;
    if workspace.is_none() {
        write_new_file(&root.join(".gitignore"), GITIGNORE_TEMPLATE.as_bytes())?;
    }
    if artifacts.library {
        write_new_file(&root.join("lib/root.nia"), library_source().as_bytes())?;
    }
    if artifacts.executable {
        write_new_file(
            &root.join("src/main.nia"),
            executable_source(name, artifacts.library).as_bytes(),
        )?;
    }
    write_new_file(
        &root.join("tests/unit.nia"),
        test_source(name, artifacts.library).as_bytes(),
    )?;
    if validate {
        PackageManifest::read_standalone(root)
            .map_err(|error| format!("generated package `{name}` is invalid: {error}"))?;
    }
    Ok(())
}

fn executable_source(package_name: &str, uses_library: bool) -> String {
    let package_import = if uses_library {
        format!("using {package_name};\n")
    } else {
        String::new()
    };
    let print = if uses_library {
        format!("io::debugPrint(\"{{}}\\n\", [{package_name}::greeting()])")
    } else {
        "io::debugPrint(\"hello from nia\\n\", [])".to_owned()
    };
    format!(
        "using std::io;\nusing std::process;\n{package_import}\npub fun main(init: process::Init) process::ExitCode!() {{\n    _ = init;\n    {print}.?;\n    !()\n}}\n"
    )
}

fn library_source() -> String {
    concat!(
        "// A public API shared by the executable and the test suite.\n",
        "\n",
        "// The text lives in static storage, so the slice that `greeting` returns\n",
        "// stays valid after the call. Returning the literal itself would point at a\n",
        "// temporary that ends with the function.\n",
        "static greetingText: [char; 14] = \"hello from nia\";\n",
        "\n",
        "pub fun greeting() &[char] {\n",
        "    greetingText\n",
        "}\n",
    )
    .to_owned()
}

fn test_source(package_name: &str, tests_library: bool) -> String {
    let imports = if tests_library {
        format!("using {package_name};\nusing std::process;\nusing std::slice;\nusing std::test;\n")
    } else {
        "using std::process;\nusing std::test;\n".to_owned()
    };
    let test_function = if tests_library {
        format!(
            "fun greetingIsHello() test::Error!() {{\n    test::expect({package_name}::greeting().equals(\"hello from nia\")).?;\n    !()\n}}\n"
        )
    } else {
        "fun smoke() test::Error!() {\n    test::expect(true).?;\n    !()\n}\n".to_owned()
    };
    let (test_name, callback) = if tests_library {
        ("greeting is hello", "greetingIsHello")
    } else {
        ("smoke", "smoke")
    };
    format!(
        "{imports}\n{test_function}\npub fun main(init: process::Init) process::ExitCode!() {{\n    _ = init;\n    let cases = [test::Case::init(\"{test_name}\", {callback})];\n    if not test::Runner::init(cases).run().isSuccessful() {{\n        return process::ExitCode(1)!;\n    }}\n    !()\n}}\n"
    )
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
    let tokens = (name.len() <= 128).then(|| nia_lexer::tokenize(name));
    let valid_identifier = tokens.as_ref().is_some_and(|tokens| {
        matches!(
            tokens.as_slice(),
            [identifier, eof]
                if identifier.kind == nia_lexer::TokenKind::Ident
                    && identifier.span.start == 0
                    && identifier.span.end == name.len()
                    && eof.kind == nia_lexer::TokenKind::Eof
        )
    });
    if !valid_identifier {
        return Err(format!(
            "invalid package name `{name}`; use a Nia identifier no longer than 128 bytes"
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
        for (artifacts, executable, library) in [
            (
                Artifacts {
                    executable: false,
                    library: false,
                },
                true,
                true,
            ),
            (
                Artifacts {
                    executable: true,
                    library: false,
                },
                true,
                false,
            ),
            (
                Artifacts {
                    executable: false,
                    library: true,
                },
                false,
                true,
            ),
            (
                Artifacts {
                    executable: true,
                    library: true,
                },
                true,
                true,
            ),
        ] {
            let root = temporary_root("package").join("demo");
            new_project(&root, Some("demo"), artifacts, false, &[]).unwrap();
            let manifest = PackageManifest::read_standalone(&root).unwrap();
            assert_eq!(manifest.library_path.is_some(), library);
            assert_eq!(
                manifest.artifacts.iter().any(|artifact| artifact.kind == nia_package_metadata::ArtifactKind::Executable),
                executable
            );
            assert!(root.join("tests/unit.nia").is_file());
            assert_eq!(root.join("lib/root.nia").is_file(), library);
            assert_eq!(root.join("src/main.nia").is_file(), executable);
            assert_eq!(
                fs::read_to_string(root.join(".gitignore")).unwrap(),
                GITIGNORE_TEMPLATE
            );
            let _ = fs::remove_dir_all(root.parent().unwrap());
        }
    }

    #[test]
    fn workspace_template_has_real_members_and_shared_identity() {
        let parent = temporary_root("workspace");
        let root = parent.join("mono");
        new_project(
            &root,
            None,
            Artifacts {
                executable: true,
                library: true,
            },
            true,
            &["app".to_owned(), "core".to_owned()],
        )
        .unwrap();
        let workspace = WorkspaceManifest::read(&root).unwrap();
        assert_eq!(workspace.members, ["packages/app", "packages/core"]);
        assert_eq!(workspace.member_packages().unwrap().len(), 2);
        assert!(root.join("packages/app/lib/root.nia").is_file());
        assert!(root.join("packages/app/src/main.nia").is_file());
        assert!(root.join("packages/app/tests/unit.nia").is_file());
        assert_eq!(
            fs::read_to_string(root.join(".gitignore")).unwrap(),
            GITIGNORE_TEMPLATE
        );
        assert!(!root.join("packages/app/.gitignore").exists());
        let _ = fs::remove_dir_all(parent);
    }

    #[test]
    fn new_inside_workspace_registers_the_member_and_inherits_identity() {
        let parent = temporary_root("workspace-new");
        let root = parent.join("mono");
        new_project(
            &root,
            None,
            Artifacts {
                executable: true,
                library: true,
            },
            true,
            &["app".to_owned()],
        )
        .unwrap();
        let member = root.join("packages/core");
        new_project(
            &member,
            None,
            Artifacts {
                executable: false,
                library: true,
            },
            false,
            &[],
        )
        .unwrap();
        let workspace = WorkspaceManifest::read(&root).unwrap();
        assert!(workspace.members.contains(&"packages/core".to_owned()));
        let package = PackageManifest::read(&member).unwrap();
        assert_eq!(package.package.name, "core");
        assert!(package.library_path.is_some());
        assert!(member.join("tests/unit.nia").is_file());
        let _ = fs::remove_dir_all(parent);
    }

    #[test]
    fn new_inside_workspace_appends_nested_paths_not_covered_by_member_globs() {
        let parent = temporary_root("workspace-nested-member");
        let root = parent.join("mono");
        new_project(
            &root,
            None,
            Artifacts {
                executable: true,
                library: true,
            },
            true,
            &["app".to_owned()],
        )
        .unwrap();
        let member = root.join("packages/tools/core");
        new_project(
            &member,
            None,
            Artifacts {
                executable: false,
                library: true,
            },
            false,
            &[],
        )
        .unwrap();

        let workspace = WorkspaceManifest::read(&root).unwrap();
        assert!(
            workspace
                .members
                .contains(&"packages/tools/core".to_owned())
        );
        assert!(workspace.member_packages().unwrap().iter().any(|package| {
            package.name == "core" && package.root == member.canonicalize().unwrap()
        }));
        let manifest = fs::read_to_string(root.join("nia.toml")).unwrap();
        assert!(manifest.contains("\"packages/tools/core\""));
        let _ = fs::remove_dir_all(parent);
    }

    #[test]
    fn new_respects_normalized_workspace_member_patterns() {
        let parent = temporary_root("workspace-normalized-pattern");
        let root = parent.join("mono");
        new_project(
            &root,
            None,
            Artifacts {
                executable: true,
                library: true,
            },
            true,
            &["app".to_owned()],
        )
        .unwrap();
        let manifest_path = root.join("nia.toml");
        let manifest = fs::read_to_string(&manifest_path).unwrap();
        fs::write(
            &manifest_path,
            manifest.replace("packages/*", "./packages/*/"),
        )
        .unwrap();

        new_project(
            &root.join("packages/core"),
            None,
            Artifacts {
                executable: false,
                library: true,
            },
            false,
            &[],
        )
        .unwrap();

        let workspace = WorkspaceManifest::read(&root).unwrap();
        assert_eq!(workspace.members, ["packages/app", "packages/core"]);
        let manifest = fs::read_to_string(&manifest_path).unwrap();
        assert!(!manifest.contains("\"packages/core\""));
        let _ = fs::remove_dir_all(parent);
    }

    #[test]
    fn concurrent_workspace_package_creations_keep_every_member() {
        let parent = temporary_root("workspace-concurrent-new");
        let root = parent.join("mono");
        new_project(
            &root,
            None,
            Artifacts {
                executable: true,
                library: true,
            },
            true,
            &["app".to_owned()],
        )
        .unwrap();

        std::thread::scope(|scope| {
            let first_path = root.join("packages/first");
            let second_path = root.join("packages/second");
            let first = scope.spawn(move || {
                new_project(
                    &first_path,
                    None,
                    Artifacts {
                        executable: false,
                        library: true,
                    },
                    false,
                    &[],
                )
            });
            let second = scope.spawn(move || {
                new_project(
                    &second_path,
                    None,
                    Artifacts {
                        executable: true,
                        library: false,
                    },
                    false,
                    &[],
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
    fn init_inside_workspace_registers_the_package_and_inherits_identity() {
        let parent = temporary_root("workspace-init");
        let root = parent.join("mono");
        new_project(
            &root,
            None,
            Artifacts {
                executable: true,
                library: true,
            },
            true,
            &["app".to_owned()],
        )
        .unwrap();
        let member = root.join("packages/core");
        fs::create_dir_all(&member).unwrap();
        init_project(
            &member,
            None,
            Artifacts {
                executable: false,
                library: true,
            },
            false,
            &[],
        )
        .unwrap();

        let workspace = WorkspaceManifest::read(&root).unwrap();
        assert!(workspace.members.contains(&"packages/core".to_owned()));
        let package = PackageManifest::read(&member).unwrap();
        assert_eq!(package.package.name, "core");
        assert_eq!(package.package.namespace, "local");
        assert!(package.library_path.is_some());
        assert!(member.join("tests/unit.nia").is_file());
        let _ = fs::remove_dir_all(parent);
    }

    #[test]
    fn new_under_a_workspace_exclusion_creates_an_independent_package() {
        let parent = temporary_root("workspace-excluded-package");
        let root = parent.join("mono");
        new_project(
            &root,
            None,
            Artifacts {
                executable: true,
                library: true,
            },
            true,
            &["app".to_owned()],
        )
        .unwrap();
        let manifest_path = root.join("nia.toml");
        let manifest = fs::read_to_string(&manifest_path).unwrap();
        fs::write(
            &manifest_path,
            manifest.replace(
                "members = [\"packages/*\"]",
                "members = [\"packages/*\"]\nexclude = [\"packages/vendor\"]",
            ),
        )
        .unwrap();
        let independent = root.join("packages/vendor/helper");
        new_project(
            &independent,
            None,
            Artifacts {
                executable: false,
                library: true,
            },
            false,
            &[],
        )
        .unwrap();

        assert_eq!(
            PackageManifest::read_standalone(&independent)
                .unwrap()
                .package
                .name,
            "helper"
        );
        assert!(
            !fs::read_to_string(&manifest_path)
                .unwrap()
                .contains("packages/vendor/helper")
        );
        assert_eq!(
            WorkspaceManifest::read(&root)
                .unwrap()
                .member_packages()
                .unwrap()
                .len(),
            1
        );
        let _ = fs::remove_dir_all(parent);
    }

    #[test]
    fn failed_workspace_init_rolls_back_files_and_member_registration() {
        let parent = temporary_root("workspace-init-rollback");
        let root = parent.join("mono");
        new_project(
            &root,
            None,
            Artifacts {
                executable: true,
                library: true,
            },
            true,
            &["app".to_owned()],
        )
        .unwrap();
        let existing = root.join("packages/nested/app");
        fs::create_dir_all(&existing).unwrap();
        let manifest_path = root.join("nia.toml");
        let original_manifest = fs::read_to_string(&manifest_path).unwrap();

        let error = init_project(
            &existing,
            None,
            Artifacts {
                executable: true,
                library: false,
            },
            false,
            &[],
        )
        .unwrap_err();

        assert!(
            error.contains("workspace already contains a package named `app`"),
            "unexpected rollback error: {error}"
        );
        assert!(!existing.join("nia.toml").exists());
        assert_eq!(
            fs::read_to_string(&manifest_path).unwrap(),
            original_manifest
        );
        let _ = fs::remove_dir_all(parent);
    }

    #[test]
    fn init_preserves_existing_files_and_refuses_template_collisions() {
        let root = temporary_root("init");
        fs::write(root.join("notes.txt"), "keep me").unwrap();
        fs::write(
            root.join(".gitignore"),
            "# existing repository rules\nlocal-output/\n",
        )
        .unwrap();
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
        assert_eq!(
            fs::read_to_string(root.join(".gitignore")).unwrap(),
            "# existing repository rules\nlocal-output/\n"
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
        assert_eq!(package.artifacts.len(), 2);
        assert_eq!(package.artifacts[0].name, "hello");
        assert!(root.join("src/main.nia").is_file());
        assert!(root.join("lib/root.nia").is_file());
        assert!(root.join("tests/unit.nia").is_file());
        let _ = fs::remove_dir_all(parent);
    }

    #[test]
    fn generated_package_names_follow_the_language_identifier_grammar() {
        for name in [
            "",
            "fun",
            "Self",
            "bad-name",
            "contains space",
            " leading",
            "trailing ",
        ] {
            assert!(validate_project_name(name).is_err(), "accepted `{name}`");
        }
        for name in ["hello", "_core", "library2", "var"] {
            validate_project_name(name).unwrap();
        }
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
