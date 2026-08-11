use std::fs;
use std::path::{Path, PathBuf};

use super::BuildResult;
use super::acceptance::validate_workload;
use super::process::run_bounded;
use super::reports::parse_build_reports;
use crate::{MaintainResult, TemporaryDirectory};

pub fn build_command(
    nia: &Path,
    resource_root: &Path,
    workspace: &Path,
    step: Option<&str>,
) -> Vec<String> {
    let mut command = vec![
        nia.to_string_lossy().into_owned(),
        "--resource-root".to_owned(),
        resource_root.to_string_lossy().into_owned(),
        "build".to_owned(),
        "--root".to_owned(),
        workspace.to_string_lossy().into_owned(),
        "--timings=detail".to_owned(),
        "--timings-format=json".to_owned(),
    ];
    if let Some(step) = step {
        command.insert(4, step.to_owned());
    }
    command
}

fn run_state(
    nia: &Path,
    resource_root: &Path,
    workspace: &Path,
    name: &str,
    timeout_seconds: u64,
    step: Option<&str>,
    expect_success: bool,
) -> MaintainResult<BuildResult> {
    let command = build_command(nia, resource_root, workspace, step);
    let output = run_bounded(&command, workspace, timeout_seconds)?;
    let succeeded = output.status.success();
    let stderr = String::from_utf8_lossy(&output.stderr);
    if succeeded != expect_success {
        eprint!("{stderr}");
        return Err(format!(
            "build state {name:?} returned {}; expected success={expect_success}",
            output.status
        ));
    }
    let reports = parse_build_reports(&stderr, succeeded)?;
    if reports.actions.success != expect_success {
        return Err(format!(
            "build state {name:?} action status disagrees with process status"
        ));
    }
    let _ = output.stdout;
    Ok(BuildResult {
        name: name.to_owned(),
        // Persist logical identities instead of checkout-specific absolute
        // paths so baseline artifacts remain comparable after relocation.
        command: vec![
            "$NIA".to_owned(),
            "--resource-root".to_owned(),
            "$RESOURCE_ROOT".to_owned(),
        ]
        .into_iter()
        .chain(command[3..].iter().cloned())
        .collect(),
        return_code: output.status.code().unwrap_or(-1),
        wall_seconds_observed: output.elapsed,
        available_memory_bytes_before: output.available_memory,
        corrupted_action_cache_entries: None,
        reports,
    })
}

fn copy_tree(source: &Path, destination: &Path) -> MaintainResult<()> {
    fs::create_dir_all(destination)
        .map_err(|error| format!("failed to create {}: {error}", destination.display()))?;
    for entry in fs::read_dir(source)
        .map_err(|error| format!("failed to read {}: {error}", source.display()))?
    {
        let entry =
            entry.map_err(|error| format!("failed to inspect {}: {error}", source.display()))?;
        let target = destination.join(entry.file_name());
        let file_type = entry
            .file_type()
            .map_err(|error| format!("failed to inspect {}: {error}", entry.path().display()))?;
        if file_type.is_dir() {
            copy_tree(&entry.path(), &target)?;
        } else if file_type.is_file() {
            fs::copy(entry.path(), &target)
                .map_err(|error| format!("failed to copy {}: {error}", entry.path().display()))?;
        }
    }
    Ok(())
}

fn collect_action_entries(root: &Path, entries: &mut Vec<PathBuf>) -> MaintainResult<()> {
    if !root.is_dir() {
        return Ok(());
    }
    for entry in
        fs::read_dir(root).map_err(|error| format!("failed to read {}: {error}", root.display()))?
    {
        let entry =
            entry.map_err(|error| format!("failed to inspect {}: {error}", root.display()))?;
        let file_type = entry
            .file_type()
            .map_err(|error| format!("failed to inspect {}: {error}", entry.path().display()))?;
        if file_type.is_dir() {
            collect_action_entries(&entry.path(), entries)?;
        } else if file_type.is_file()
            && entry
                .path()
                .extension()
                .is_some_and(|extension| extension == "entry")
        {
            entries.push(entry.path());
        }
    }
    Ok(())
}

pub fn corrupt_action_cache(workspace: &Path) -> MaintainResult<usize> {
    let mut entries = Vec::new();
    collect_action_entries(&workspace.join(".nia-cache/actions"), &mut entries)?;
    entries.sort();
    if entries.is_empty() {
        return Err("build workload produced no action-cache entries to corrupt".to_owned());
    }

    // Corrupt only immutable action records. Locks and temporary publication
    // files are intentionally left intact so this tests cache recovery rather
    // than an unrelated coordination failure.
    for path in &entries {
        fs::write(path, b"nia build baseline injected corruption\n")
            .map_err(|error| format!("failed to corrupt {}: {error}", path.display()))?;
    }
    Ok(entries.len())
}

pub(super) fn run_workload(
    nia: &Path,
    resource_root: &Path,
    fixture: &Path,
    timeout_seconds: u64,
) -> MaintainResult<(Vec<BuildResult>, TemporaryDirectory)> {
    let temporary = TemporaryDirectory::new("nia-build-baseline-")?;
    let workspace = temporary.path().join("representative");
    copy_tree(fixture, &workspace)?;

    // Each transition is observed before applying the next mutation. Reusing
    // one workspace is what makes warm reuse, typed invalidation, corruption
    // recovery, and failure isolation part of one coherent experiment.
    let mut results = vec![run_state(
        nia,
        resource_root,
        &workspace,
        "clean",
        timeout_seconds,
        None,
        true,
    )?];
    results.push(run_state(
        nia,
        resource_root,
        &workspace,
        "warm",
        timeout_seconds,
        None,
        true,
    )?);
    fs::copy(
        workspace.join("src/main.edited.nia"),
        workspace.join("src/main.nia"),
    )
    .map_err(|error| format!("failed to edit build fixture source: {error}"))?;
    results.push(run_state(
        nia,
        resource_root,
        &workspace,
        "source_edit",
        timeout_seconds,
        None,
        true,
    )?);
    let build_script = fs::read_to_string(workspace.join("build.nia"))
        .map_err(|error| format!("failed to read build fixture: {error}"))?;
    fs::write(
        workspace.join("build.nia"),
        build_script.replace("deps/helper.nia", "deps/helper_edited.nia"),
    )
    .map_err(|error| format!("failed to edit build fixture module map: {error}"))?;
    results.push(run_state(
        nia,
        resource_root,
        &workspace,
        "module_map_edit",
        timeout_seconds,
        None,
        true,
    )?);
    let corrupted = corrupt_action_cache(&workspace)?;
    let mut corrupt = run_state(
        nia,
        resource_root,
        &workspace,
        "corrupt_cache",
        timeout_seconds,
        None,
        true,
    )?;
    corrupt.corrupted_action_cache_entries = Some(corrupted);
    results.push(corrupt);
    results.push(run_state(
        nia,
        resource_root,
        &workspace,
        "recovered_warm",
        timeout_seconds,
        None,
        true,
    )?);
    results.push(run_state(
        nia,
        resource_root,
        &workspace,
        "failed_action",
        timeout_seconds,
        Some("fail"),
        false,
    )?);
    validate_workload(&results)?;
    Ok((results, temporary))
}
