use std::fs;
use std::io::Read;
use std::path::{Path, PathBuf};

use super::acceptance::validate_workload;
use super::process::run_bounded;
use super::reports::parse_build_reports;
use super::{ArtifactComparison, ArtifactEquivalence, BuildResult};
use crate::{MaintainResult, TemporaryDirectory};

/// Constructs the measured `nia build` command for one fixture state.
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
        process_id: output.process_id,
        return_code: output.status.code().unwrap_or(-1),
        wall_seconds_observed: output.elapsed,
        available_memory_bytes_before: output.available_memory,
        corrupted_action_cache_entries: None,
        artifact_equivalence: None,
        reports,
    })
}

const ARTIFACT_COMPARE_BUFFER_BYTES: usize = 64 * 1024;

fn streams_match(left: &Path, right: &Path) -> MaintainResult<bool> {
    let mut left = fs::File::open(left)
        .map_err(|error| format!("failed to open {}: {error}", left.display()))?;
    let mut right = fs::File::open(right)
        .map_err(|error| format!("failed to open {}: {error}", right.display()))?;
    if left
        .metadata()
        .map_err(|error| format!("failed to inspect artifact: {error}"))?
        .len()
        != right
            .metadata()
            .map_err(|error| format!("failed to inspect clean artifact: {error}"))?
            .len()
    {
        return Ok(false);
    }
    let mut left_buffer = [0; ARTIFACT_COMPARE_BUFFER_BYTES];
    let mut right_buffer = [0; ARTIFACT_COMPARE_BUFFER_BYTES];
    loop {
        let left_read = left
            .read(&mut left_buffer)
            .map_err(|error| format!("failed to read incremental artifact: {error}"))?;
        let right_read = right
            .read(&mut right_buffer)
            .map_err(|error| format!("failed to read clean artifact: {error}"))?;
        if left_read != right_read || left_buffer[..left_read] != right_buffer[..right_read] {
            return Ok(false);
        }
        if left_read == 0 {
            return Ok(true);
        }
    }
}

fn compare_representative_artifacts(
    incremental: &Path,
    clean: &Path,
    clean_state: &str,
    source_app_modules: &[&str],
) -> MaintainResult<ArtifactEquivalence> {
    let incremental = incremental.join(".nia-build");
    let clean = clean.join(".nia-build");
    let comparisons = vec![
        ArtifactComparison {
            name: "source-app".to_owned(),
            source_modules: source_app_modules
                .iter()
                .map(|module| (*module).to_owned())
                .collect(),
            matching: streams_match(&incremental.join("source-app"), &clean.join("source-app"))?,
        },
        ArtifactComparison {
            name: "generated-app".to_owned(),
            source_modules: vec!["generated.nia".to_owned()],
            matching: streams_match(
                &incremental.join("generated-app"),
                &clean.join("generated-app"),
            )?,
        },
    ];
    Ok(ArtifactEquivalence {
        clean_state: clean_state.to_owned(),
        comparisons,
    })
}

fn replace_source(workspace: &Path) -> MaintainResult<()> {
    fs::copy(
        workspace.join("src/main.edited.nia"),
        workspace.join("src/main.nia"),
    )
    .map_err(|error| format!("failed to edit build fixture source: {error}"))?;
    Ok(())
}

fn replace_module_map(workspace: &Path) -> MaintainResult<()> {
    let build_script = fs::read_to_string(workspace.join("build.nia"))
        .map_err(|error| format!("failed to read build fixture: {error}"))?;
    fs::write(
        workspace.join("build.nia"),
        build_script.replace("deps/helper.nia", "deps/helper_edited.nia"),
    )
    .map_err(|error| format!("failed to edit build fixture module map: {error}"))
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

/// Corrupts every persisted action entry in a workload workspace.
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
    let source_edit_clean_workspace = temporary.path().join("source-edit-clean");
    let module_map_edit_clean_workspace = temporary.path().join("module-map-edit-clean");
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
    replace_source(&workspace)?;
    let mut source_edit = run_state(
        nia,
        resource_root,
        &workspace,
        "source_edit",
        timeout_seconds,
        None,
        true,
    )?;
    copy_tree(fixture, &source_edit_clean_workspace)?;
    replace_source(&source_edit_clean_workspace)?;
    let source_edit_clean = run_state(
        nia,
        resource_root,
        &source_edit_clean_workspace,
        "source_edit_clean",
        timeout_seconds,
        None,
        true,
    )?;
    source_edit.artifact_equivalence = Some(compare_representative_artifacts(
        &workspace,
        &source_edit_clean_workspace,
        "source_edit_clean",
        &["src/main.nia", "deps/helper.nia"],
    )?);
    results.push(source_edit);
    results.push(source_edit_clean);

    replace_module_map(&workspace)?;
    let mut module_map_edit = run_state(
        nia,
        resource_root,
        &workspace,
        "module_map_edit",
        timeout_seconds,
        None,
        true,
    )?;
    copy_tree(fixture, &module_map_edit_clean_workspace)?;
    replace_source(&module_map_edit_clean_workspace)?;
    replace_module_map(&module_map_edit_clean_workspace)?;
    let module_map_edit_clean = run_state(
        nia,
        resource_root,
        &module_map_edit_clean_workspace,
        "module_map_edit_clean",
        timeout_seconds,
        None,
        true,
    )?;
    module_map_edit.artifact_equivalence = Some(compare_representative_artifacts(
        &workspace,
        &module_map_edit_clean_workspace,
        "module_map_edit_clean",
        &["src/main.nia", "deps/helper_edited.nia"],
    )?);
    results.push(module_map_edit);
    results.push(module_map_edit_clean);
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::test_support::TestDirectory;

    #[test]
    fn artifact_comparison_crosses_fixed_buffer_boundaries() {
        let directory = TestDirectory::new("build-artifact-comparison");
        let left = directory.path().join("left");
        let right = directory.path().join("right");
        let mut contents = vec![0x5a; ARTIFACT_COMPARE_BUFFER_BYTES * 2 + 1];
        fs::write(&left, &contents).unwrap();
        fs::write(&right, &contents).unwrap();
        assert!(streams_match(&left, &right).unwrap());

        contents[ARTIFACT_COMPARE_BUFFER_BYTES] ^= 0xff;
        fs::write(&right, &contents).unwrap();
        assert!(!streams_match(&left, &right).unwrap());

        contents.pop();
        fs::write(&right, &contents).unwrap();
        assert!(!streams_match(&left, &right).unwrap());
    }
}
