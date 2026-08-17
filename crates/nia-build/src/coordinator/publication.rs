// SPDX-License-Identifier: GPL-3.0-or-later
//! Journaled publication for complete action output sets.
//!
//! Staged content is validated and synced before the journal becomes prepared.
//! Only then may destinations be renamed; any partial commit rolls back in
//! reverse order while the caller still holds every output lock.

use super::*;
#[cfg(unix)]
use std::os::unix::fs::OpenOptionsExt as _;

pub(super) fn path_text(action: &PlanAction, path: &Path) -> Result<String, CoordinatorError> {
    path.to_str()
        .map(str::to_string)
        .ok_or_else(|| CoordinatorError::NonUtf8Path {
            action: action.key.clone(),
            path: path.to_path_buf(),
        })
}

static STAGED_OUTPUT_SEQUENCE: AtomicU64 = AtomicU64::new(0);

pub(super) struct StagedOutputTransaction {
    pub(super) directory: PathBuf,
    pub(super) committed_directory: PathBuf,
    pub(super) outputs: Vec<StagedOutputEntry>,
    pub(super) journal: OutputTransactionJournal,
}

pub(super) fn take_staged_output_transaction(
    action: &PlanAction,
    staged: &mut Option<StagedOutputTransaction>,
) -> Result<StagedOutputTransaction, CoordinatorError> {
    staged.take().ok_or_else(|| {
        inconsistent(
            format!("action `{}`", action.key.name()),
            "staged output transaction".to_string(),
        )
    })
}

pub(super) struct StagedOutputEntry {
    pub(super) destination: PathBuf,
    pub(super) temporary: PathBuf,
    pub(super) backup: PathBuf,
    pub(super) kind: TransactionOutputKind,
}

struct StagedOutputPublication {
    had_previous: bool,
    backed_up: bool,
    installed: bool,
}

pub(super) fn prepare_staged_outputs(
    action: &PlanAction,
    build_dir: &Path,
    resolved_outputs: &[(&LogicalPath, PathBuf)],
) -> Result<StagedOutputTransaction, CoordinatorError> {
    let resolved_outputs = resolved_outputs
        .iter()
        .map(|(logical, destination)| ResolvedTransactionOutput {
            logical,
            destination: destination.clone(),
            kind: TransactionOutputKind::File,
        })
        .collect::<Vec<_>>();
    prepare_typed_staged_outputs(action, build_dir, &resolved_outputs)
}

pub(super) struct ResolvedTransactionOutput<'a> {
    pub(super) logical: &'a LogicalPath,
    pub(super) destination: PathBuf,
    pub(super) kind: TransactionOutputKind,
}

pub(super) fn prepare_typed_staged_outputs(
    action: &PlanAction,
    build_dir: &Path,
    resolved_outputs: &[ResolvedTransactionOutput<'_>],
) -> Result<StagedOutputTransaction, CoordinatorError> {
    let first = resolved_outputs.first().ok_or_else(|| {
        staged_output_io(
            action,
            Path::new(""),
            "resolve transaction root for",
            io::Error::new(io::ErrorKind::InvalidInput, "output transaction is empty"),
            None,
        )
    })?;
    let parent = first
        .destination
        .parent()
        .filter(|path| !path.as_os_str().is_empty())
        .ok_or_else(|| {
            staged_output_io(
                action,
                &first.destination,
                "resolve parent for",
                io::Error::new(io::ErrorKind::InvalidInput, "output has no parent"),
                None,
            )
        })?;
    fs::create_dir_all(parent).map_err(|error| {
        staged_output_io(action, parent, "create parent directory for", error, None)
    })?;
    // Staging and committed marker names share one sequence. Refusing an
    // existing committed marker prevents reuse of a name still owned by crash
    // recovery from an earlier process.
    let owner = ProcessIdentity::current();
    for _ in 0..128 {
        let sequence = STAGED_OUTPUT_SEQUENCE.fetch_add(1, Ordering::Relaxed);
        let directory = parent.join(format!(
            ".nia-command-{}-{}-{sequence}.stage",
            owner.pid, owner.start_time
        ));
        let committed_directory = parent.join(format!(
            ".nia-command-{}-{}-{sequence}.committed",
            owner.pid, owner.start_time
        ));
        if committed_directory.exists() {
            continue;
        }
        match fs::create_dir(&directory) {
            Ok(()) => {
                let outputs = resolved_outputs
                    .iter()
                    .enumerate()
                    .map(|(index, output)| StagedOutputEntry {
                        destination: output.destination.clone(),
                        temporary: directory.join(format!("output-{index}")),
                        backup: directory.join(format!("backup-{index}")),
                        kind: output.kind,
                    })
                    .collect::<Vec<_>>();
                let logical_outputs = resolved_outputs
                    .iter()
                    .map(|output| TransactionOutput {
                        path: output.logical.clone(),
                        kind: output.kind,
                    })
                    .collect::<Vec<_>>();
                let journal = match OutputTransactionJournal::create(
                    build_dir,
                    &action.key,
                    &logical_outputs,
                    &directory,
                    &committed_directory,
                ) {
                    Ok(journal) => journal,
                    Err(error) => {
                        let _ = fs::remove_dir_all(&directory);
                        return Err(staged_output_io(
                            action,
                            &directory,
                            "create recovery journal for",
                            error,
                            None,
                        ));
                    }
                };
                return Ok(StagedOutputTransaction {
                    directory,
                    committed_directory,
                    outputs,
                    journal,
                });
            }
            Err(error) if error.kind() == io::ErrorKind::AlreadyExists => {}
            Err(error) => {
                return Err(staged_output_io(
                    action,
                    &directory,
                    "create staging directory for",
                    error,
                    None,
                ));
            }
        }
    }
    Err(staged_output_io(
        action,
        parent,
        "create unique staging directory in",
        io::Error::new(
            io::ErrorKind::AlreadyExists,
            "exhausted staged-output directory names",
        ),
        None,
    ))
}

pub(super) fn publish_staged_outputs(
    action: &PlanAction,
    staged: StagedOutputTransaction,
) -> Result<(), CoordinatorError> {
    publish_staged_outputs_with(action, staged, |_| Ok(()))
}

pub(super) fn publish_staged_outputs_with(
    action: &PlanAction,
    staged: StagedOutputTransaction,
    mut before_install: impl FnMut(usize) -> io::Result<()>,
) -> Result<(), CoordinatorError> {
    // Preparation performs every fallible validation and durability step that
    // can happen before visible destinations are touched.
    let prepared = (|| {
        let mut publications = Vec::with_capacity(staged.outputs.len());
        for output in &staged.outputs {
            validate_and_sync_transaction_output(&output.temporary, output.kind).map_err(
                |error| {
                    staged_output_io(
                        action,
                        &output.temporary,
                        "validate and sync staged",
                        error,
                        None,
                    )
                },
            )?;
            let parent = output
                .destination
                .parent()
                .filter(|path| !path.as_os_str().is_empty())
                .ok_or_else(|| {
                    staged_output_io(
                        action,
                        &output.destination,
                        "resolve parent for",
                        io::Error::new(io::ErrorKind::InvalidInput, "output has no parent"),
                        None,
                    )
                })?;
            fs::create_dir_all(parent).map_err(|error| {
                staged_output_io(action, parent, "create parent directory for", error, None)
            })?;
            let had_previous = match fs::symlink_metadata(&output.destination) {
                Ok(_) => {
                    validate_and_sync_transaction_output(&output.destination, output.kind)
                        .map_err(|error| {
                            staged_output_io(
                                action,
                                &output.destination,
                                "validate previous",
                                error,
                                None,
                            )
                        })?;
                    true
                }
                Err(error) if error.kind() == io::ErrorKind::NotFound => false,
                Err(error) => {
                    return Err(staged_output_io(
                        action,
                        &output.destination,
                        "inspect previous",
                        error,
                        None,
                    ));
                }
            };
            publications.push(StagedOutputPublication {
                had_previous,
                backed_up: false,
                installed: false,
            });
        }
        fs::File::open(&staged.directory)
            .and_then(|directory| directory.sync_all())
            .map_err(|error| {
                staged_output_io(
                    action,
                    &staged.directory,
                    "sync prepared transaction",
                    error,
                    None,
                )
            })?;
        staged
            .journal
            .mark_prepared(
                &publications
                    .iter()
                    .map(|publication| publication.had_previous)
                    .collect::<Vec<_>>(),
            )
            .map_err(|error| {
                staged_output_io(
                    action,
                    staged.journal.path(),
                    "persist prepared recovery state for",
                    error,
                    None,
                )
            })?;
        Ok(publications)
    })();
    let mut publications = match prepared {
        Ok(publications) => publications,
        Err(cause) => {
            return cleanup_staged_outputs(action, staged, Some(Box::new(cause)));
        }
    };
    // The prepared journal now owns rollback. Per-entry flags record the exact
    // prefix made visible if a later rename or sync fails.
    let committed = (|| {
        for (index, (output, publication)) in staged
            .outputs
            .iter()
            .zip(publications.iter_mut())
            .enumerate()
        {
            before_install(index).map_err(|error| {
                staged_output_io(
                    action,
                    &output.destination,
                    "commit transaction entry for",
                    error,
                    None,
                )
            })?;
            if publication.had_previous {
                fs::rename(&output.destination, &output.backup).map_err(|error| {
                    staged_output_io(action, &output.destination, "back up previous", error, None)
                })?;
                publication.backed_up = true;
            }
            fs::rename(&output.temporary, &output.destination).map_err(|error| {
                staged_output_io(action, &output.destination, "install", error, None)
            })?;
            publication.installed = true;
        }
        let parents: BTreeSet<_> = staged
            .outputs
            .iter()
            .filter_map(|output| output.destination.parent())
            .collect();
        for parent in parents {
            fs::File::open(parent)
                .and_then(|directory| directory.sync_all())
                .map_err(|error| {
                    staged_output_io(
                        action,
                        parent,
                        "sync committed output directory for",
                        error,
                        None,
                    )
                })?;
        }
        fs::rename(&staged.directory, &staged.committed_directory).map_err(|error| {
            staged_output_io(
                action,
                &staged.directory,
                "mark transaction committed at",
                error,
                None,
            )
        })
    })();
    match committed {
        Ok(()) => {
            if let Some(parent) = staged.committed_directory.parent() {
                let _ = fs::File::open(parent).and_then(|directory| directory.sync_all());
            }
            let committed_cleaned = match fs::remove_dir_all(&staged.committed_directory) {
                Ok(()) => true,
                Err(error) if error.kind() == io::ErrorKind::NotFound => true,
                Err(_) => false,
            };
            if let Some(parent) = staged.committed_directory.parent() {
                let _ = fs::File::open(parent).and_then(|directory| directory.sync_all());
            }
            if committed_cleaned {
                let _ = staged.journal.cleanup();
            }
            Ok(())
        }
        Err(cause) => rollback_staged_outputs(action, staged, publications, cause),
    }
}

fn validate_and_sync_transaction_output(
    path: &Path,
    kind: TransactionOutputKind,
) -> io::Result<()> {
    let metadata = fs::symlink_metadata(path)?;
    match kind {
        TransactionOutputKind::File if metadata.file_type().is_file() => {
            fs::File::open(path)?.sync_all()
        }
        TransactionOutputKind::Directory if metadata.file_type().is_dir() => {
            validate_and_sync_transaction_directory(path)
        }
        TransactionOutputKind::File => Err(io::Error::new(
            io::ErrorKind::InvalidData,
            "transaction file output must be a regular file",
        )),
        TransactionOutputKind::Directory => Err(io::Error::new(
            io::ErrorKind::InvalidData,
            "transaction directory output must be a directory",
        )),
    }
}

fn validate_and_sync_transaction_directory(path: &Path) -> io::Result<()> {
    let mut entries = fs::read_dir(path)?
        .map(|entry| entry.map(|entry| entry.path()))
        .collect::<io::Result<Vec<_>>>()?;
    entries.sort();
    for entry in entries {
        let metadata = fs::symlink_metadata(&entry)?;
        if metadata.file_type().is_file() {
            fs::File::open(&entry)?.sync_all()?;
        } else if metadata.file_type().is_dir() {
            validate_and_sync_transaction_directory(&entry)?;
        } else {
            // Device nodes and symlinks would make restore semantics depend on
            // mutable state outside the transaction.
            return Err(io::Error::new(
                io::ErrorKind::InvalidData,
                format!(
                    "transaction directory contains non-file entry `{}`",
                    entry.display()
                ),
            ));
        }
    }
    fs::File::open(path)?.sync_all()
}

fn rollback_staged_outputs(
    action: &PlanAction,
    staged: StagedOutputTransaction,
    publications: Vec<StagedOutputPublication>,
    cause: CoordinatorError,
) -> Result<(), CoordinatorError> {
    let mut cause = Some(Box::new(cause));
    // Reverse commit order preserves the last fully valid visible prefix while
    // each installed output is moved aside and its predecessor restored.
    for (output, publication) in staged.outputs.iter().zip(&publications).rev() {
        if publication.installed
            && let Err(error) = fs::rename(&output.destination, &output.temporary)
        {
            return Err(staged_output_io(
                action,
                &output.destination,
                "roll back installed",
                error,
                cause.take(),
            ));
        }
        if publication.backed_up
            && let Err(error) = fs::rename(&output.backup, &output.destination)
        {
            return Err(staged_output_io(
                action,
                &output.destination,
                "restore previous",
                error,
                cause.take(),
            ));
        }
    }
    let parents: BTreeSet<_> = staged
        .outputs
        .iter()
        .filter_map(|output| output.destination.parent())
        .collect();
    for parent in parents {
        if let Err(error) = fs::File::open(parent).and_then(|directory| directory.sync_all()) {
            return Err(staged_output_io(
                action,
                parent,
                "sync rolled-back output directory for",
                error,
                cause.take(),
            ));
        }
    }
    cleanup_staged_outputs(action, staged, cause)
}

pub(super) fn cleanup_staged_outputs(
    action: &PlanAction,
    staged: StagedOutputTransaction,
    cause: Option<Box<CoordinatorError>>,
) -> Result<(), CoordinatorError> {
    match fs::remove_dir_all(&staged.directory) {
        Ok(()) => {}
        Err(error) if error.kind() == io::ErrorKind::NotFound => {}
        Err(error) => {
            return Err(staged_output_io(
                action,
                &staged.directory,
                "clean up",
                error,
                cause,
            ));
        }
    }
    if let Some(parent) = staged.directory.parent()
        && let Err(error) = fs::File::open(parent).and_then(|directory| directory.sync_all())
    {
        return Err(staged_output_io(
            action,
            parent,
            "sync cleanup directory for",
            error,
            cause,
        ));
    }
    if let Err(error) = staged.journal.cleanup() {
        return Err(staged_output_io(
            action,
            staged.journal.path(),
            "clean up recovery journal for",
            error,
            cause,
        ));
    }
    match cause {
        Some(cause) => Err(*cause),
        None => Ok(()),
    }
}

pub(super) fn staged_output_io(
    action: &PlanAction,
    path: &Path,
    operation: &'static str,
    error: io::Error,
    cause: Option<Box<CoordinatorError>>,
) -> CoordinatorError {
    CoordinatorError::StagedOutput {
        action: action.key.clone(),
        path: path.to_path_buf(),
        operation,
        error,
        cause,
    }
}

pub(super) static GENERATED_TEMP_SEQUENCE: AtomicU64 = AtomicU64::new(0);

pub(super) fn write_generated_file(
    action: &PlanAction,
    output: &std::path::Path,
    contents: &[u8],
) -> Result<(), CoordinatorError> {
    if generated_file_matches(output, contents) {
        return Ok(());
    }
    let parent = output
        .parent()
        .filter(|path| !path.as_os_str().is_empty())
        .ok_or_else(|| {
            generated_io(
                action,
                output,
                "resolve parent for",
                io::Error::new(
                    io::ErrorKind::InvalidInput,
                    "generated output has no parent",
                ),
            )
        })?;
    fs::create_dir_all(parent)
        .map_err(|error| generated_io(action, parent, "create parent directory for", error))?;
    let (temporary_path, mut temporary) = create_generated_temporary(action, parent)?;
    let result = (|| {
        temporary
            .write_all(contents)
            .map_err(|error| generated_io(action, &temporary_path, "write", error))?;
        temporary
            .sync_all()
            .map_err(|error| generated_io(action, &temporary_path, "sync", error))?;
        drop(temporary);
        fs::rename(&temporary_path, output)
            .map_err(|error| generated_io(action, output, "publish", error))?;
        fs::File::open(parent)
            .and_then(|directory| directory.sync_all())
            .map_err(|error| generated_io(action, parent, "sync parent directory for", error))
    })();
    if result.is_err() {
        match fs::remove_file(&temporary_path) {
            Ok(()) => {}
            Err(error) if error.kind() == io::ErrorKind::NotFound => {}
            Err(error) => {
                return Err(generated_io(
                    action,
                    &temporary_path,
                    "clean up temporary",
                    error,
                ));
            }
        }
    }
    result
}

fn generated_file_matches(path: &Path, expected: &[u8]) -> bool {
    (|| -> io::Result<bool> {
        let mut options = fs::OpenOptions::new();
        options.read(true);
        #[cfg(unix)]
        options.custom_flags(libc::O_NOFOLLOW);
        let mut file = options.open(path)?;
        let metadata = file.metadata()?;
        if !metadata.is_file() {
            return Ok(false);
        }
        generated_stream_matches(&mut file, metadata.len(), expected)
    })()
    .unwrap_or(false)
}

/// Compares an existing generated output with fixed memory and an exact stream
/// budget. A replacement or growth race becomes a mismatch and takes the normal
/// atomic publication path instead of allocating the complete existing file.
fn generated_stream_matches(
    reader: &mut impl io::Read,
    observed_len: u64,
    expected: &[u8],
) -> io::Result<bool> {
    if observed_len != u64::try_from(expected.len()).unwrap_or(u64::MAX) {
        return Ok(false);
    }
    let mut buffer = [0; 64 * 1024];
    let mut offset = 0;
    while offset != expected.len() {
        let chunk_len = (expected.len() - offset).min(buffer.len());
        reader.read_exact(&mut buffer[..chunk_len])?;
        if buffer[..chunk_len] != expected[offset..offset + chunk_len] {
            return Ok(false);
        }
        offset += chunk_len;
    }
    let mut trailing = [0; 1];
    Ok(reader.read(&mut trailing)? == 0)
}

pub(super) fn install_artifact_io(
    action: &PlanAction,
    path: &Path,
    operation: &'static str,
    error: io::Error,
) -> CoordinatorError {
    CoordinatorError::InstallArtifactIo {
        action: action.key.clone(),
        path: path.to_path_buf(),
        operation,
        error,
    }
}

fn create_generated_temporary(
    action: &PlanAction,
    parent: &std::path::Path,
) -> Result<(PathBuf, fs::File), CoordinatorError> {
    let owner = ProcessIdentity::current();
    for _ in 0..128 {
        let sequence = GENERATED_TEMP_SEQUENCE.fetch_add(1, Ordering::Relaxed);
        let path = parent.join(format!(
            ".nia-generated-{}-{}-{sequence}.tmp",
            owner.pid, owner.start_time
        ));
        match fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&path)
        {
            Ok(file) => return Ok((path, file)),
            Err(error) if error.kind() == io::ErrorKind::AlreadyExists => {}
            Err(error) => return Err(generated_io(action, &path, "create temporary", error)),
        }
    }
    Err(generated_io(
        action,
        parent,
        "create unique temporary in",
        io::Error::new(
            io::ErrorKind::AlreadyExists,
            "exhausted generated-file temporary names",
        ),
    ))
}

fn generated_io(
    action: &PlanAction,
    path: &std::path::Path,
    operation: &'static str,
    error: io::Error,
) -> CoordinatorError {
    CoordinatorError::GeneratedFileIo {
        action: action.key.clone(),
        path: path.to_path_buf(),
        operation,
        error,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn generated_stream_comparison_is_bounded_and_exact() {
        assert!(generated_stream_matches(&mut io::Cursor::new(b"source"), 6, b"source").unwrap());
        assert!(!generated_stream_matches(&mut io::Cursor::new(b"sourcf"), 6, b"source").unwrap());
        assert!(!generated_stream_matches(&mut io::Cursor::new(b"source!"), 6, b"source").unwrap());
        assert!(!generated_stream_matches(&mut io::Cursor::new(b"source"), 7, b"source").unwrap());
    }
}
