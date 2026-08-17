// SPDX-License-Identifier: GPL-3.0-or-later
//! Recoverable publication for one action's complete output set.
//!
//! A checksummed journal is durable before staging can affect destinations, and
//! a separately synced prepared marker records rollback ownership. Recovery
//! runs under the complete canonical output-lock set and rejects ambiguous or
//! corrupt state instead of selecting a partial result.

use std::{
    collections::BTreeSet,
    fmt, fs, io,
    io::{Cursor, Read, Write},
    path::{Component, Path, PathBuf},
    sync::atomic::{AtomicU64, Ordering},
};

use nia_compat::formats::{
    OUTPUT_TRANSACTION, OUTPUT_TRANSACTION_JOURNAL, OUTPUT_TRANSACTION_PREPARED,
};
use nia_query::{FingerprintDomain, QueryFingerprintBuilder};

use crate::{
    ActionKey, LogicalPath, LogicalPathRoot, PackageKey,
    lock::{ProcessIdentity, ScopedFileLock, output_lock_path},
};

pub(crate) const OUTPUT_TRANSACTION_DIRECTORY: &str = ".nia-transactions";
const MAX_JOURNAL_BYTES: usize = 1024 * 1024;
const MAX_OUTPUTS: usize = 4096;
const OUTPUT_TRANSACTION_JOURNAL_DOMAIN: FingerprintDomain =
    FingerprintDomain::new("nia.build.output-transaction-journal.v2");
static JOURNAL_SEQUENCE: AtomicU64 = AtomicU64::new(0);

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum TransactionOutputKind {
    File,
    Directory,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct TransactionOutput {
    pub(crate) path: LogicalPath,
    pub(crate) kind: TransactionOutputKind,
}

#[derive(Debug)]
pub struct OutputRecoveryError {
    pub(crate) action: Option<ActionKey>,
    pub(crate) journal: PathBuf,
    pub(crate) path: PathBuf,
    pub(crate) operation: &'static str,
    pub(crate) error: io::Error,
}

impl fmt::Display for OutputRecoveryError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            f,
            "failed to {} interrupted output transaction `{}`",
            self.operation,
            self.journal.display()
        )?;
        if let Some(action) = &self.action {
            write!(
                f,
                " for action `{}` in package `{}`",
                action.name(),
                action.package().as_str()
            )?;
        }
        write!(f, " at `{}`: {}", self.path.display(), self.error)
    }
}

impl std::error::Error for OutputRecoveryError {}

#[derive(Debug)]
pub(crate) struct OutputTransactionJournal {
    directory: PathBuf,
}

impl OutputTransactionJournal {
    pub(crate) fn create(
        build_dir: &Path,
        action: &ActionKey,
        outputs: &[TransactionOutput],
        staged_directory: &Path,
        committed_directory: &Path,
    ) -> io::Result<Self> {
        let root = journal_root(build_dir);
        fs::create_dir_all(&root)?;
        let header = JournalHeader {
            action: action.clone(),
            outputs: outputs.to_vec(),
            staged_path: build_relative_path(build_dir, staged_directory)?,
            committed_path: build_relative_path(build_dir, committed_directory)?,
        };
        let encoded = encode_journal(&header);
        for _ in 0..128 {
            let sequence = JOURNAL_SEQUENCE.fetch_add(1, Ordering::Relaxed);
            let owner = ProcessIdentity::current();
            let name = format!(".nia-output-{}-{}-{sequence}", owner.pid, owner.start_time);
            let temporary = root.join(format!("{name}.tmp"));
            let directory = root.join(format!("{name}.transaction"));
            match fs::create_dir(&temporary) {
                Ok(()) => {
                    let result = (|| {
                        write_synced_new(&temporary.join("journal.bin"), &encoded)?;
                        fs::File::open(&temporary)?.sync_all()?;
                        fs::rename(&temporary, &directory)?;
                        fs::File::open(&root)?.sync_all()
                    })();
                    if result.is_err() || temporary.exists() {
                        let _ = fs::remove_dir_all(&temporary);
                    }
                    result?;
                    return Ok(Self { directory });
                }
                Err(error) if error.kind() == io::ErrorKind::AlreadyExists => {}
                Err(error) => return Err(error),
            }
        }
        Err(io::Error::new(
            io::ErrorKind::AlreadyExists,
            "exhausted output transaction journal names",
        ))
    }

    pub(crate) fn mark_prepared(&self, had_previous: &[bool]) -> io::Result<()> {
        let encoded = encode_prepared(had_previous);
        let temporary = self.directory.join("prepared.tmp");
        let prepared = self.directory.join("prepared.bin");
        let result = (|| {
            write_synced_new(&temporary, &encoded)?;
            fs::rename(&temporary, &prepared)?;
            fs::File::open(&self.directory)?.sync_all()
        })();
        if result.is_err() || temporary.exists() {
            let _ = fs::remove_file(&temporary);
        }
        result
    }

    pub(crate) fn cleanup(&self) -> io::Result<()> {
        match fs::remove_dir_all(&self.directory) {
            Ok(()) => {}
            Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(()),
            Err(error) => return Err(error),
        }
        sync_parent(&self.directory)
    }

    pub(crate) fn path(&self) -> &Path {
        &self.directory
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct JournalHeader {
    action: ActionKey,
    outputs: Vec<TransactionOutput>,
    staged_path: LogicalPath,
    committed_path: LogicalPath,
}

pub(crate) fn recover_interrupted_output_transactions(
    cache_dir: &Path,
    build_dir: &Path,
) -> Result<(), OutputRecoveryError> {
    let root = journal_root(build_dir);
    let entries = match fs::read_dir(&root) {
        Ok(entries) => entries,
        Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(()),
        Err(error) => return Err(recovery_error(&root, &root, "scan", error)),
    };
    let mut journals = entries
        .map(|entry| {
            entry
                .map(|entry| entry.path())
                .map_err(|error| recovery_error(&root, &root, "read directory entry for", error))
        })
        .collect::<Result<Vec<_>, _>>()?;
    retire_dead_temporary_journals(&root, &journals)?;
    journals
        .retain(|path| path.extension().and_then(|value| value.to_str()) == Some("transaction"));
    journals.sort();
    for journal in journals {
        recover_journal(cache_dir, build_dir, &journal)?;
    }
    Ok(())
}

fn recover_journal(
    cache_dir: &Path,
    build_dir: &Path,
    journal: &Path,
) -> Result<(), OutputRecoveryError> {
    let header = read_header(journal)?;
    let action = header.action.clone();
    let result = (|| {
        let mut outputs = header
            .outputs
            .iter()
            .map(|output| output.path.clone())
            .collect::<Vec<_>>();
        outputs.sort();
        outputs.dedup();
        let mut locks = Vec::with_capacity(outputs.len());
        for output in &outputs {
            let path = output_lock_path(cache_dir, output);
            let lock = ScopedFileLock::acquire_interruptible(path.clone(), || false)
                .map_err(|error| recovery_error(journal, &path, "acquire output lock for", error))?
                .ok_or_else(|| {
                    recovery_error(
                        journal,
                        &path,
                        "acquire output lock for",
                        io::Error::other("output recovery lock was cancelled"),
                    )
                })?;
            locks.push(lock);
        }
        if !journal.exists() {
            return Ok(());
        }
        let current_header = read_header(journal)?;
        if current_header != header {
            return Err(recovery_error(
                journal,
                journal,
                "revalidate locked journal for",
                io::Error::new(
                    io::ErrorKind::InvalidData,
                    "output transaction journal changed while locks were acquired",
                ),
            ));
        }
        let staged = resolve_build_path(build_dir, &header.staged_path);
        let committed = resolve_build_path(build_dir, &header.committed_path);
        validate_transaction_paths(build_dir, &header, &staged, &committed)
            .map_err(|error| recovery_error(journal, &staged, "validate paths for", error))?;
        let staged_exists = directory_exists(journal, &staged)?;
        let committed_exists = directory_exists(journal, &committed)?;
        if staged_exists && committed_exists {
            return Err(recovery_error(
                journal,
                &staged,
                "resolve state for",
                io::Error::new(
                    io::ErrorKind::InvalidData,
                    "both staged and committed transaction directories exist",
                ),
            ));
        }
        if committed_exists {
            remove_directory(journal, &committed, "clean accepted directory for")?;
            remove_directory(journal, journal, "clean journal for")?;
            return Ok(());
        }
        if !staged_exists {
            remove_directory(journal, journal, "clean completed journal for")?;
            return Ok(());
        }
        let prepared = read_prepared(journal, header.outputs.len())?;
        let Some(had_previous) = prepared else {
            remove_directory(journal, &staged, "clean unprepared staging for")?;
            remove_directory(journal, journal, "clean unprepared journal for")?;
            return Ok(());
        };
        rollback_interrupted(journal, build_dir, &header.outputs, &staged, &had_previous)?;
        remove_directory(journal, &staged, "clean rolled-back staging for")?;
        remove_directory(journal, journal, "clean rolled-back journal for")
    })();
    result.map_err(|mut error: OutputRecoveryError| {
        error.action = Some(action);
        error
    })
}

fn retire_dead_temporary_journals(
    root: &Path,
    entries: &[PathBuf],
) -> Result<(), OutputRecoveryError> {
    for path in entries {
        if path.extension().and_then(|value| value.to_str()) != Some("tmp") {
            continue;
        }
        let Some(owner) = temporary_journal_owner(path) else {
            continue;
        };
        if owner.is_alive() {
            continue;
        }
        remove_directory(root, path, "retire abandoned temporary journal in")?;
    }
    Ok(())
}

fn temporary_journal_owner(path: &Path) -> Option<ProcessIdentity> {
    let name = path.file_stem()?.to_str()?;
    let owner = name.strip_prefix(".nia-output-")?;
    let mut parts = owner.split('-');
    let identity = ProcessIdentity {
        pid: parts.next()?.parse().ok()?,
        start_time: parts.next()?.parse().ok()?,
    };
    parts.next()?.parse::<u64>().ok()?;
    parts.next().is_none().then_some(identity)
}

fn rollback_interrupted(
    journal: &Path,
    build_dir: &Path,
    outputs: &[TransactionOutput],
    staged: &Path,
    had_previous: &[bool],
) -> Result<(), OutputRecoveryError> {
    for (index, (output, had_previous)) in outputs.iter().zip(had_previous).enumerate().rev() {
        let destination = resolve_build_path(build_dir, &output.path);
        let temporary = staged.join(format!("output-{index}"));
        let backup = staged.join(format!("backup-{index}"));
        let temporary_exists = output_exists(journal, &temporary, output.kind)?;
        let backup_exists = output_exists(journal, &backup, output.kind)?;
        let destination_exists = output_exists(journal, &destination, output.kind)?;
        if *had_previous {
            if backup_exists {
                if destination_exists {
                    fs::rename(&destination, &temporary).map_err(|error| {
                        recovery_error(
                            journal,
                            &destination,
                            "retire interrupted output for",
                            error,
                        )
                    })?;
                }
                fs::rename(&backup, &destination).map_err(|error| {
                    recovery_error(journal, &destination, "restore previous output for", error)
                })?;
            } else if !temporary_exists || !destination_exists {
                return Err(recovery_error(
                    journal,
                    &destination,
                    "reconstruct previous output for",
                    io::Error::new(
                        io::ErrorKind::InvalidData,
                        "prepared transaction lost its previous output or backup",
                    ),
                ));
            }
        } else if temporary_exists {
            if destination_exists {
                return Err(recovery_error(
                    journal,
                    &destination,
                    "reconstruct absent output for",
                    io::Error::new(
                        io::ErrorKind::InvalidData,
                        "uninstalled transaction output unexpectedly exists",
                    ),
                ));
            }
        } else if destination_exists {
            fs::rename(&destination, &temporary).map_err(|error| {
                recovery_error(
                    journal,
                    &destination,
                    "retire newly installed output for",
                    error,
                )
            })?;
        } else {
            return Err(recovery_error(
                journal,
                &destination,
                "reconstruct installed output for",
                io::Error::new(
                    io::ErrorKind::InvalidData,
                    "installed transaction output is missing",
                ),
            ));
        }
    }
    let parents: BTreeSet<_> = outputs
        .iter()
        .filter_map(|output| {
            resolve_build_path(build_dir, &output.path)
                .parent()
                .map(Path::to_path_buf)
        })
        .collect();
    for parent in parents {
        fs::File::open(&parent)
            .and_then(|directory| directory.sync_all())
            .map_err(|error| {
                recovery_error(journal, &parent, "sync rolled-back directory for", error)
            })?;
    }
    Ok(())
}

fn read_header(journal: &Path) -> Result<JournalHeader, OutputRecoveryError> {
    let path = journal.join("journal.bin");
    let encoded = read_bounded(&path)
        .map_err(|error| recovery_error(journal, &path, "read journal for", error))?;
    decode_journal(&encoded).ok_or_else(|| {
        recovery_error(
            journal,
            &path,
            "decode journal for",
            io::Error::new(
                io::ErrorKind::InvalidData,
                "invalid output transaction journal",
            ),
        )
    })
}

fn read_prepared(
    journal: &Path,
    output_count: usize,
) -> Result<Option<Vec<bool>>, OutputRecoveryError> {
    let path = journal.join("prepared.bin");
    let encoded = match read_bounded(&path) {
        Ok(encoded) => encoded,
        Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(None),
        Err(error) => {
            return Err(recovery_error(
                journal,
                &path,
                "read prepared state for",
                error,
            ));
        }
    };
    decode_prepared(&encoded, output_count)
        .map(Some)
        .ok_or_else(|| {
            recovery_error(
                journal,
                &path,
                "decode prepared state for",
                io::Error::new(
                    io::ErrorKind::InvalidData,
                    "invalid prepared transaction state",
                ),
            )
        })
}

fn encode_journal(header: &JournalHeader) -> Vec<u8> {
    let mut payload = Vec::new();
    write_text(&mut payload, header.action.package().as_str());
    write_text(&mut payload, header.action.name());
    write_text(&mut payload, &header.staged_path.protocol_path());
    write_text(&mut payload, &header.committed_path.protocol_path());
    payload.extend_from_slice(&(header.outputs.len() as u64).to_le_bytes());
    for output in &header.outputs {
        payload.push(match output.kind {
            TransactionOutputKind::File => 0,
            TransactionOutputKind::Directory => 1,
        });
        write_text(&mut payload, &output.path.protocol_path());
    }
    encode_envelope(OUTPUT_TRANSACTION_JOURNAL.magic, &payload)
}

fn decode_journal(encoded: &[u8]) -> Option<JournalHeader> {
    let payload = decode_envelope(encoded, OUTPUT_TRANSACTION_JOURNAL.magic)?;
    let mut cursor = Cursor::new(payload);
    let package = read_text(&mut cursor, payload.len())?;
    let action = read_text(&mut cursor, payload.len())?;
    let staged = read_text(&mut cursor, payload.len())?;
    let committed = read_text(&mut cursor, payload.len())?;
    let count = usize::try_from(read_u64(&mut cursor)?).ok()?;
    (count > 0 && count <= MAX_OUTPUTS).then_some(())?;
    let mut outputs = Vec::with_capacity(count);
    for _ in 0..count {
        let kind = match read_u8(&mut cursor)? {
            0 => TransactionOutputKind::File,
            1 => TransactionOutputKind::Directory,
            _ => return None,
        };
        outputs.push(TransactionOutput {
            path: LogicalPath::new(
                LogicalPathRoot::Build,
                &read_text(&mut cursor, payload.len())?,
            )
            .ok()?,
            kind,
        });
    }
    // Rollback treats every destination as an independent rename target.
    // Nested destinations would make the result depend on output order and
    // could move a parent while a child backup is still being reconstructed.
    let outputs_are_disjoint = outputs.iter().enumerate().all(|(index, output)| {
        !output.path.components().is_empty()
            && outputs[index + 1..]
                .iter()
                .all(|other| !output.path.overlaps(&other.path))
    });
    outputs_are_disjoint.then_some(())?;
    (usize::try_from(cursor.position()).ok()? == payload.len()).then_some(())?;
    Some(JournalHeader {
        action: ActionKey::new(PackageKey::new(package).ok()?, action).ok()?,
        outputs,
        staged_path: LogicalPath::new(LogicalPathRoot::Build, &staged).ok()?,
        committed_path: LogicalPath::new(LogicalPathRoot::Build, &committed).ok()?,
    })
}

fn encode_prepared(had_previous: &[bool]) -> Vec<u8> {
    let mut payload = Vec::with_capacity(8 + had_previous.len());
    payload.extend_from_slice(&(had_previous.len() as u64).to_le_bytes());
    payload.extend(had_previous.iter().map(|value| u8::from(*value)));
    encode_envelope(OUTPUT_TRANSACTION_PREPARED.magic, &payload)
}

fn decode_prepared(encoded: &[u8], expected_count: usize) -> Option<Vec<bool>> {
    let payload = decode_envelope(encoded, OUTPUT_TRANSACTION_PREPARED.magic)?;
    let mut cursor = Cursor::new(payload);
    let count = usize::try_from(read_u64(&mut cursor)?).ok()?;
    (count == expected_count && count <= MAX_OUTPUTS).then_some(())?;
    let position = usize::try_from(cursor.position()).ok()?;
    (payload.len().checked_sub(position)? == count).then_some(())?;
    payload[position..]
        .iter()
        .map(|value| match value {
            0 => Some(false),
            1 => Some(true),
            _ => None,
        })
        .collect()
}

fn encode_envelope(magic: &[u8; 8], payload: &[u8]) -> Vec<u8> {
    let checksum = checksum(payload);
    let mut encoded = Vec::with_capacity(32 + payload.len());
    encoded.extend_from_slice(magic);
    encoded.extend_from_slice(&(payload.len() as u64).to_le_bytes());
    for part in checksum {
        encoded.extend_from_slice(&part.to_le_bytes());
    }
    encoded.extend_from_slice(payload);
    encoded
}

fn decode_envelope<'a>(encoded: &'a [u8], magic: &[u8; 8]) -> Option<&'a [u8]> {
    let mut cursor = Cursor::new(encoded);
    let mut found_magic = [0; 8];
    cursor.read_exact(&mut found_magic).ok()?;
    (found_magic == *magic).then_some(())?;
    let payload_len = usize::try_from(read_u64(&mut cursor)?).ok()?;
    let expected_checksum = [read_u64(&mut cursor)?, read_u64(&mut cursor)?];
    let position = usize::try_from(cursor.position()).ok()?;
    (encoded.len().checked_sub(position)? == payload_len).then_some(())?;
    let payload = &encoded[position..];
    (checksum(payload) == expected_checksum).then_some(payload)
}

fn checksum(payload: &[u8]) -> [u64; 2] {
    let mut builder = QueryFingerprintBuilder::new(OUTPUT_TRANSACTION_JOURNAL_DOMAIN);
    builder.write_bytes(payload);
    builder.finish().parts()
}

fn validate_transaction_paths(
    build_dir: &Path,
    header: &JournalHeader,
    staged: &Path,
    committed: &Path,
) -> io::Result<()> {
    let first = header
        .outputs
        .first()
        .ok_or_else(|| io::Error::new(io::ErrorKind::InvalidData, "transaction has no outputs"))?;
    let first_destination = resolve_build_path(build_dir, &first.path);
    let expected_parent = first_destination
        .parent()
        .ok_or_else(|| io::Error::new(io::ErrorKind::InvalidData, "output has no parent"))?;
    if staged.parent() != Some(expected_parent) || committed.parent() != Some(expected_parent) {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            "transaction directory does not share the first output parent",
        ));
    }
    let staged_name = staged
        .file_name()
        .and_then(|name| name.to_str())
        .unwrap_or("");
    let committed_name = committed
        .file_name()
        .and_then(|name| name.to_str())
        .unwrap_or("");
    if !transaction_directory_name_is_valid(staged_name, ".stage")
        || !transaction_directory_name_is_valid(committed_name, ".committed")
        || staged_name.strip_suffix(".stage") != committed_name.strip_suffix(".committed")
    {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            "transaction directory names do not form one stage/commit pair",
        ));
    }
    Ok(())
}

fn transaction_directory_name_is_valid(name: &str, suffix: &str) -> bool {
    let Some(identity) = name
        .strip_prefix(".nia-command-")
        .and_then(|name| name.strip_suffix(suffix))
    else {
        return false;
    };
    let parts = identity.split('-').collect::<Vec<_>>();
    // Two components are the persisted pre-generation format. Recovery must
    // keep accepting those journals across upgrades; new writers use three.
    matches!(parts.len(), 2 | 3) && parts.iter().all(|part| part.parse::<u64>().is_ok())
}

fn build_relative_path(build_dir: &Path, path: &Path) -> io::Result<LogicalPath> {
    let relative = path.strip_prefix(build_dir).map_err(|_| {
        io::Error::new(
            io::ErrorKind::InvalidInput,
            "output transaction path is outside the build directory",
        )
    })?;
    let mut components = Vec::new();
    for component in relative.components() {
        match component {
            Component::Normal(value) => components.push(value.to_str().ok_or_else(|| {
                io::Error::new(
                    io::ErrorKind::InvalidInput,
                    "output transaction path is not valid UTF-8",
                )
            })?),
            _ => {
                return Err(io::Error::new(
                    io::ErrorKind::InvalidInput,
                    "output transaction path is not a normal relative path",
                ));
            }
        }
    }
    LogicalPath::new(LogicalPathRoot::Build, &components.join("/")).map_err(|error| {
        io::Error::new(
            io::ErrorKind::InvalidInput,
            format!("invalid output transaction path: {error}"),
        )
    })
}

fn resolve_build_path(build_dir: &Path, logical: &LogicalPath) -> PathBuf {
    let mut path = build_dir.to_path_buf();
    for component in logical.components() {
        path.push(component);
    }
    path
}

fn journal_root(build_dir: &Path) -> PathBuf {
    build_dir
        .join(OUTPUT_TRANSACTION_DIRECTORY)
        .join(OUTPUT_TRANSACTION.path_component)
}

fn read_bounded(path: &Path) -> io::Result<Vec<u8>> {
    let metadata = fs::symlink_metadata(path)?;
    if !metadata.file_type().is_file() {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            "transaction record is not a regular file",
        ));
    }
    let length = usize::try_from(metadata.len()).map_err(|_| {
        io::Error::new(
            io::ErrorKind::InvalidData,
            "transaction record is too large",
        )
    })?;
    validate_transaction_record_size(length)?;
    let mut file = fs::File::open(path)?;
    read_bounded_contents(&mut file, length)
}

/// Reads through the byte limit even when the file grows after metadata was
/// sampled. The extra byte distinguishes an exact-limit record from a raced or
/// otherwise oversized stream without ever allocating the complete input.
fn read_bounded_contents(reader: &mut impl Read, expected_len: usize) -> io::Result<Vec<u8>> {
    validate_transaction_record_size(expected_len)?;
    let mut encoded = Vec::with_capacity(expected_len);
    reader
        .take((MAX_JOURNAL_BYTES + 1) as u64)
        .read_to_end(&mut encoded)?;
    validate_transaction_record_size(encoded.len())?;
    Ok(encoded)
}

fn write_synced_new(path: &Path, bytes: &[u8]) -> io::Result<()> {
    validate_transaction_record_size(bytes.len())?;
    let mut file = fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(path)?;
    file.write_all(bytes)?;
    file.sync_all()
}

fn validate_transaction_record_size(length: usize) -> io::Result<()> {
    if length > MAX_JOURNAL_BYTES {
        Err(io::Error::new(
            io::ErrorKind::InvalidData,
            "transaction record exceeds its size limit",
        ))
    } else {
        Ok(())
    }
}

fn write_text(encoded: &mut Vec<u8>, text: &str) {
    encoded.extend_from_slice(&(text.len() as u64).to_le_bytes());
    encoded.extend_from_slice(text.as_bytes());
}

fn read_text(cursor: &mut Cursor<&[u8]>, encoded_len: usize) -> Option<String> {
    let length = usize::try_from(read_u64(cursor)?).ok()?;
    let position = usize::try_from(cursor.position()).ok()?;
    (length <= encoded_len.checked_sub(position)?).then_some(())?;
    let mut bytes = vec![0; length];
    cursor.read_exact(&mut bytes).ok()?;
    String::from_utf8(bytes).ok()
}

fn read_u64(cursor: &mut Cursor<&[u8]>) -> Option<u64> {
    let mut bytes = [0; 8];
    cursor.read_exact(&mut bytes).ok()?;
    Some(u64::from_le_bytes(bytes))
}

fn read_u8(cursor: &mut Cursor<&[u8]>) -> Option<u8> {
    let mut byte = [0; 1];
    cursor.read_exact(&mut byte).ok()?;
    Some(byte[0])
}

fn directory_exists(journal: &Path, path: &Path) -> Result<bool, OutputRecoveryError> {
    match fs::symlink_metadata(path) {
        Ok(metadata) if metadata.file_type().is_dir() => Ok(true),
        Ok(_) => Err(recovery_error(
            journal,
            path,
            "inspect directory for",
            io::Error::new(
                io::ErrorKind::InvalidData,
                "transaction path is not a directory",
            ),
        )),
        Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(false),
        Err(error) => Err(recovery_error(
            journal,
            path,
            "inspect directory for",
            error,
        )),
    }
}

fn output_exists(
    journal: &Path,
    path: &Path,
    kind: TransactionOutputKind,
) -> Result<bool, OutputRecoveryError> {
    match fs::symlink_metadata(path) {
        Ok(metadata) if kind == TransactionOutputKind::File && metadata.file_type().is_file() => {
            Ok(true)
        }
        Ok(metadata)
            if kind == TransactionOutputKind::Directory && metadata.file_type().is_dir() =>
        {
            validate_directory_tree(path)
                .map(|()| true)
                .map_err(|error| {
                    recovery_error(journal, path, "validate directory output for", error)
                })
        }
        Ok(_) => Err(recovery_error(
            journal,
            path,
            "inspect output for",
            io::Error::new(
                io::ErrorKind::InvalidData,
                match kind {
                    TransactionOutputKind::File => "transaction file output is not a regular file",
                    TransactionOutputKind::Directory => {
                        "transaction directory output is not a directory"
                    }
                },
            ),
        )),
        Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(false),
        Err(error) => Err(recovery_error(journal, path, "inspect output for", error)),
    }
}

fn validate_directory_tree(path: &Path) -> io::Result<()> {
    let mut entries = fs::read_dir(path)?
        .map(|entry| entry.map(|entry| entry.path()))
        .collect::<io::Result<Vec<_>>>()?;
    entries.sort();
    for entry in entries {
        let metadata = fs::symlink_metadata(&entry)?;
        if metadata.file_type().is_dir() {
            validate_directory_tree(&entry)?;
        } else if !metadata.file_type().is_file() {
            return Err(io::Error::new(
                io::ErrorKind::InvalidData,
                format!(
                    "transaction directory contains non-file entry `{}`",
                    entry.display()
                ),
            ));
        }
    }
    Ok(())
}

fn remove_directory(
    journal: &Path,
    path: &Path,
    operation: &'static str,
) -> Result<(), OutputRecoveryError> {
    match fs::remove_dir_all(path) {
        Ok(()) => {
            sync_parent(path).map_err(|error| recovery_error(journal, path, operation, error))
        }
        Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(()),
        Err(error) => Err(recovery_error(journal, path, operation, error)),
    }
}

fn sync_parent(path: &Path) -> io::Result<()> {
    let Some(parent) = path.parent() else {
        return Ok(());
    };
    fs::File::open(parent)?.sync_all()
}

fn recovery_error(
    journal: &Path,
    path: &Path,
    operation: &'static str,
    error: io::Error,
) -> OutputRecoveryError {
    OutputRecoveryError {
        action: None,
        journal: journal.to_path_buf(),
        path: path.to_path_buf(),
        operation,
        error,
    }
}

#[cfg(test)]
mod tests {
    use std::{
        sync::{Arc, mpsc},
        time::Duration,
    };

    use super::*;

    struct TestTransaction {
        root: PathBuf,
        cache_dir: PathBuf,
        build_dir: PathBuf,
        outputs: Vec<LogicalPath>,
        destinations: Vec<PathBuf>,
        staged: PathBuf,
        committed: PathBuf,
        journal: OutputTransactionJournal,
    }

    fn test_transaction(name: &str, output_paths: &[&str]) -> TestTransaction {
        let outputs = output_paths
            .iter()
            .map(|path| (*path, TransactionOutputKind::File))
            .collect::<Vec<_>>();
        test_typed_transaction(name, &outputs)
    }

    fn test_typed_transaction(
        name: &str,
        output_specs: &[(&str, TransactionOutputKind)],
    ) -> TestTransaction {
        let root = std::env::temp_dir().join(format!(
            "nia-output-recovery-{name}-{}-{}",
            std::process::id(),
            JOURNAL_SEQUENCE.fetch_add(1, Ordering::Relaxed)
        ));
        let cache_dir = root.join(".nia-cache");
        let build_dir = root.join(".nia-build");
        let outputs = output_specs
            .iter()
            .map(|(path, _)| {
                LogicalPath::new(LogicalPathRoot::Build, path).expect("logical output")
            })
            .collect::<Vec<_>>();
        let destinations = outputs
            .iter()
            .map(|output| resolve_build_path(&build_dir, output))
            .collect::<Vec<_>>();
        let parent = destinations[0].parent().expect("output parent");
        fs::create_dir_all(parent).expect("create output parent");
        // Use the pre-generation writer format to keep upgrade recovery under
        // test while production writers emit pid/start-time/sequence names.
        let legacy_name = format!(".nia-command-{}-0", std::process::id());
        let staged = parent.join(format!("{legacy_name}.stage"));
        let committed = parent.join(format!("{legacy_name}.committed"));
        fs::create_dir(&staged).expect("create staging directory");
        let journal = OutputTransactionJournal::create(
            &build_dir,
            &ActionKey::new(PackageKey::root(), "tool").expect("action"),
            &outputs
                .iter()
                .cloned()
                .zip(output_specs.iter())
                .map(|(path, (_, kind))| TransactionOutput { path, kind: *kind })
                .collect::<Vec<_>>(),
            &staged,
            &committed,
        )
        .expect("create journal");
        TestTransaction {
            root,
            cache_dir,
            build_dir,
            outputs,
            destinations,
            staged,
            committed,
            journal,
        }
    }

    fn assert_journal_removed(transaction: &TestTransaction) {
        assert!(!transaction.staged.exists());
        assert!(!transaction.committed.exists());
        assert!(!transaction.journal.path().exists());
    }

    #[test]
    fn unprepared_transaction_is_cleaned_without_changing_outputs() {
        let transaction = test_transaction("unprepared", &["tool/result.txt"]);
        fs::write(&transaction.destinations[0], b"accepted").expect("write destination");
        fs::write(transaction.staged.join("output-0"), b"unaccepted").expect("write staged");

        recover_interrupted_output_transactions(&transaction.cache_dir, &transaction.build_dir)
            .expect("recover");

        assert_eq!(
            fs::read(&transaction.destinations[0]).expect("read destination"),
            b"accepted"
        );
        assert_journal_removed(&transaction);
        let _ = fs::remove_dir_all(transaction.root);
    }

    #[test]
    fn prepared_partial_transaction_restores_old_and_absent_outputs() {
        let transaction = test_transaction(
            "partial",
            &["tool/first.txt", "other/absent.txt", "tool/last.txt"],
        );
        for destination in &transaction.destinations {
            fs::create_dir_all(destination.parent().expect("destination parent"))
                .expect("create destination parent");
        }
        fs::write(&transaction.destinations[0], b"old first").expect("write old first");
        fs::write(&transaction.destinations[2], b"old last").expect("write old last");
        for (index, contents) in [b"new first".as_slice(), b"new absent", b"new last"]
            .into_iter()
            .enumerate()
        {
            fs::write(transaction.staged.join(format!("output-{index}")), contents)
                .expect("write staged output");
        }
        transaction
            .journal
            .mark_prepared(&[true, false, true])
            .expect("mark prepared");
        fs::rename(
            &transaction.destinations[0],
            transaction.staged.join("backup-0"),
        )
        .expect("back up first");
        fs::rename(
            transaction.staged.join("output-0"),
            &transaction.destinations[0],
        )
        .expect("install first");
        fs::rename(
            transaction.staged.join("output-1"),
            &transaction.destinations[1],
        )
        .expect("install absent");

        recover_interrupted_output_transactions(&transaction.cache_dir, &transaction.build_dir)
            .expect("recover");

        assert_eq!(
            fs::read(&transaction.destinations[0]).expect("read first"),
            b"old first"
        );
        assert!(!transaction.destinations[1].exists());
        assert_eq!(
            fs::read(&transaction.destinations[2]).expect("read last"),
            b"old last"
        );
        assert_journal_removed(&transaction);
        let _ = fs::remove_dir_all(transaction.root);
    }

    #[test]
    fn prepared_directory_replacement_restores_previous_tree() {
        let transaction = test_typed_transaction(
            "directory-replacement",
            &[("tool/objects", TransactionOutputKind::Directory)],
        );
        fs::create_dir(&transaction.destinations[0]).expect("create previous directory");
        fs::write(transaction.destinations[0].join("old.o"), b"old")
            .expect("write previous object");
        let temporary = transaction.staged.join("output-0");
        fs::create_dir(&temporary).expect("create staged directory");
        fs::write(temporary.join("new.o"), b"new").expect("write staged object");
        transaction
            .journal
            .mark_prepared(&[true])
            .expect("mark prepared");
        fs::rename(
            &transaction.destinations[0],
            transaction.staged.join("backup-0"),
        )
        .expect("back up directory");
        fs::rename(&temporary, &transaction.destinations[0]).expect("install directory");

        recover_interrupted_output_transactions(&transaction.cache_dir, &transaction.build_dir)
            .expect("recover");

        assert_eq!(
            fs::read(transaction.destinations[0].join("old.o")).expect("read previous object"),
            b"old"
        );
        assert!(!transaction.destinations[0].join("new.o").exists());
        assert_journal_removed(&transaction);
        let _ = fs::remove_dir_all(transaction.root);
    }

    #[test]
    fn prepared_new_directory_is_removed_during_recovery() {
        let transaction = test_typed_transaction(
            "new-directory",
            &[("tool/objects", TransactionOutputKind::Directory)],
        );
        let temporary = transaction.staged.join("output-0");
        fs::create_dir(&temporary).expect("create staged directory");
        fs::write(temporary.join("new.o"), b"new").expect("write staged object");
        transaction
            .journal
            .mark_prepared(&[false])
            .expect("mark prepared");
        fs::rename(&temporary, &transaction.destinations[0]).expect("install directory");

        recover_interrupted_output_transactions(&transaction.cache_dir, &transaction.build_dir)
            .expect("recover");

        assert!(!transaction.destinations[0].exists());
        assert_journal_removed(&transaction);
        let _ = fs::remove_dir_all(transaction.root);
    }

    #[test]
    fn prepared_mixed_transaction_restores_file_directory_and_absent_output() {
        let transaction = test_typed_transaction(
            "mixed",
            &[
                ("tool/metadata.txt", TransactionOutputKind::File),
                ("other/objects", TransactionOutputKind::Directory),
                ("tool/new.txt", TransactionOutputKind::File),
            ],
        );
        for destination in &transaction.destinations {
            fs::create_dir_all(destination.parent().expect("destination parent"))
                .expect("create destination parent");
        }
        fs::write(&transaction.destinations[0], b"old metadata").expect("write old metadata");
        fs::create_dir(&transaction.destinations[1]).expect("create old object directory");
        fs::write(transaction.destinations[1].join("old.o"), b"old object")
            .expect("write old object");
        fs::write(transaction.staged.join("output-0"), b"new metadata")
            .expect("write staged metadata");
        fs::create_dir(transaction.staged.join("output-1")).expect("create staged objects");
        fs::write(transaction.staged.join("output-1/new.o"), b"new object")
            .expect("write staged object");
        fs::write(transaction.staged.join("output-2"), b"new file").expect("write staged new file");
        transaction
            .journal
            .mark_prepared(&[true, true, false])
            .expect("mark prepared");
        for index in 0..2 {
            fs::rename(
                &transaction.destinations[index],
                transaction.staged.join(format!("backup-{index}")),
            )
            .expect("back up previous output");
            fs::rename(
                transaction.staged.join(format!("output-{index}")),
                &transaction.destinations[index],
            )
            .expect("install replacement");
        }
        fs::rename(
            transaction.staged.join("output-2"),
            &transaction.destinations[2],
        )
        .expect("install new file");

        recover_interrupted_output_transactions(&transaction.cache_dir, &transaction.build_dir)
            .expect("recover");

        assert_eq!(
            fs::read(&transaction.destinations[0]).expect("read old metadata"),
            b"old metadata"
        );
        assert_eq!(
            fs::read(transaction.destinations[1].join("old.o")).expect("read old object"),
            b"old object"
        );
        assert!(!transaction.destinations[1].join("new.o").exists());
        assert!(!transaction.destinations[2].exists());
        assert_journal_removed(&transaction);
        let _ = fs::remove_dir_all(transaction.root);
    }

    #[test]
    fn wrong_declared_directory_type_blocks_recovery_without_touching_destination() {
        let transaction = test_typed_transaction(
            "wrong-directory-type",
            &[("tool/objects", TransactionOutputKind::Directory)],
        );
        fs::write(&transaction.destinations[0], b"accepted file")
            .expect("write wrong-type destination");
        fs::create_dir(transaction.staged.join("output-0")).expect("create staged directory");
        transaction
            .journal
            .mark_prepared(&[false])
            .expect("mark prepared");

        let error =
            recover_interrupted_output_transactions(&transaction.cache_dir, &transaction.build_dir)
                .expect_err("wrong output type must block recovery");

        assert_eq!(error.operation, "inspect output for");
        assert_eq!(
            fs::read(&transaction.destinations[0]).expect("read accepted file"),
            b"accepted file"
        );
        assert!(transaction.staged.exists());
        assert!(transaction.journal.path().exists());
        let _ = fs::remove_dir_all(transaction.root);
    }

    #[test]
    fn prepared_partially_rolled_back_transaction_finishes_cleanup() {
        let transaction = test_transaction(
            "partial-rollback",
            &["tool/previous.txt", "tool/absent.txt"],
        );
        fs::write(&transaction.destinations[0], b"old").expect("write previous output");
        fs::write(transaction.staged.join("output-0"), b"new previous")
            .expect("write retired replacement");
        fs::write(transaction.staged.join("output-1"), b"new absent")
            .expect("write retired new output");
        transaction
            .journal
            .mark_prepared(&[true, false])
            .expect("mark prepared");

        recover_interrupted_output_transactions(&transaction.cache_dir, &transaction.build_dir)
            .expect("finish interrupted rollback");

        assert_eq!(
            fs::read(&transaction.destinations[0]).expect("read previous output"),
            b"old"
        );
        assert!(!transaction.destinations[1].exists());
        assert_journal_removed(&transaction);
        let _ = fs::remove_dir_all(transaction.root);
    }

    #[test]
    fn accepted_transaction_keeps_outputs_and_retires_recovery_state() {
        let transaction = test_transaction("accepted", &["tool/result.txt"]);
        fs::write(&transaction.destinations[0], b"old").expect("write old");
        fs::write(transaction.staged.join("output-0"), b"new").expect("write staged");
        transaction
            .journal
            .mark_prepared(&[true])
            .expect("mark prepared");
        fs::rename(
            &transaction.destinations[0],
            transaction.staged.join("backup-0"),
        )
        .expect("back up old");
        fs::rename(
            transaction.staged.join("output-0"),
            &transaction.destinations[0],
        )
        .expect("install new");
        fs::rename(&transaction.staged, &transaction.committed).expect("accept transaction");

        recover_interrupted_output_transactions(&transaction.cache_dir, &transaction.build_dir)
            .expect("recover");

        assert_eq!(
            fs::read(&transaction.destinations[0]).expect("read output"),
            b"new"
        );
        assert_journal_removed(&transaction);
        let _ = fs::remove_dir_all(transaction.root);
    }

    #[test]
    fn accepted_directory_transaction_keeps_new_tree() {
        let transaction = test_typed_transaction(
            "accepted-directory",
            &[("tool/objects", TransactionOutputKind::Directory)],
        );
        fs::create_dir(&transaction.destinations[0]).expect("create old directory");
        fs::write(transaction.destinations[0].join("old.o"), b"old").expect("write old object");
        let temporary = transaction.staged.join("output-0");
        fs::create_dir(&temporary).expect("create staged directory");
        fs::write(temporary.join("new.o"), b"new").expect("write new object");
        transaction
            .journal
            .mark_prepared(&[true])
            .expect("mark prepared");
        fs::rename(
            &transaction.destinations[0],
            transaction.staged.join("backup-0"),
        )
        .expect("back up old directory");
        fs::rename(&temporary, &transaction.destinations[0]).expect("install new directory");
        fs::rename(&transaction.staged, &transaction.committed).expect("accept transaction");

        recover_interrupted_output_transactions(&transaction.cache_dir, &transaction.build_dir)
            .expect("recover");

        assert_eq!(
            fs::read(transaction.destinations[0].join("new.o")).expect("read new object"),
            b"new"
        );
        assert!(!transaction.destinations[0].join("old.o").exists());
        assert_journal_removed(&transaction);
        let _ = fs::remove_dir_all(transaction.root);
    }

    #[test]
    fn corrupt_journal_blocks_recovery_without_touching_outputs() {
        let transaction = test_transaction("corrupt", &["tool/result.txt"]);
        fs::write(&transaction.destinations[0], b"accepted").expect("write destination");
        fs::write(transaction.staged.join("output-0"), b"unaccepted").expect("write staged");
        let journal_path = transaction.journal.path().join("journal.bin");
        let mut encoded = fs::read(&journal_path).expect("read journal");
        *encoded.last_mut().expect("journal byte") ^= 0xff;
        fs::write(&journal_path, encoded).expect("corrupt journal");

        let error =
            recover_interrupted_output_transactions(&transaction.cache_dir, &transaction.build_dir)
                .expect_err("corrupt journal must fail safely");

        assert_eq!(error.operation, "decode journal for");
        assert_eq!(
            fs::read(&transaction.destinations[0]).expect("read destination"),
            b"accepted"
        );
        assert!(transaction.staged.exists());
        assert!(transaction.journal.path().exists());
        let _ = fs::remove_dir_all(transaction.root);
    }

    #[test]
    fn corrupt_prepared_state_blocks_recovery_without_guessing() {
        let transaction = test_transaction("corrupt-prepared", &["tool/result.txt"]);
        fs::write(&transaction.destinations[0], b"old").expect("write destination");
        fs::write(transaction.staged.join("output-0"), b"new").expect("write staged");
        transaction
            .journal
            .mark_prepared(&[true])
            .expect("mark prepared");
        let prepared_path = transaction.journal.path().join("prepared.bin");
        fs::write(&prepared_path, b"corrupt").expect("corrupt prepared state");

        let error =
            recover_interrupted_output_transactions(&transaction.cache_dir, &transaction.build_dir)
                .expect_err("corrupt prepared state must fail safely");

        assert_eq!(error.operation, "decode prepared state for");
        assert_eq!(
            fs::read(&transaction.destinations[0]).expect("read destination"),
            b"old"
        );
        assert!(transaction.staged.exists());
        let _ = fs::remove_dir_all(transaction.root);
    }

    #[test]
    fn recovery_waits_for_the_active_output_owner() {
        let transaction = Arc::new(test_transaction("wait-owner", &["tool/result.txt"]));
        fs::write(&transaction.destinations[0], b"accepted").expect("write destination");
        fs::write(transaction.staged.join("output-0"), b"unaccepted").expect("write staged");
        let held = ScopedFileLock::acquire(output_lock_path(
            &transaction.cache_dir,
            &transaction.outputs[0],
        ))
        .expect("hold output lock");
        let worker_transaction = Arc::clone(&transaction);
        let (finished_tx, finished_rx) = mpsc::channel();
        let worker = std::thread::spawn(move || {
            let result = recover_interrupted_output_transactions(
                &worker_transaction.cache_dir,
                &worker_transaction.build_dir,
            );
            finished_tx.send(result).expect("send recovery result");
        });

        assert!(
            finished_rx
                .recv_timeout(Duration::from_millis(100))
                .is_err()
        );
        drop(held);
        finished_rx
            .recv_timeout(Duration::from_secs(5))
            .expect("recovery result")
            .expect("recover");
        worker.join().expect("recovery worker");
        assert_eq!(
            fs::read(&transaction.destinations[0]).expect("read destination"),
            b"accepted"
        );
        assert_journal_removed(&transaction);
        let root = transaction.root.clone();
        drop(transaction);
        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn recovery_rejects_a_journal_changed_while_waiting_for_locks() {
        let transaction = Arc::new(test_transaction("changed-journal", &["tool/result.txt"]));
        fs::write(&transaction.destinations[0], b"accepted").expect("write destination");
        fs::write(transaction.staged.join("output-0"), b"unaccepted").expect("write staged");
        let held = ScopedFileLock::acquire(output_lock_path(
            &transaction.cache_dir,
            &transaction.outputs[0],
        ))
        .expect("hold output lock");
        let worker_transaction = Arc::clone(&transaction);
        let (finished_tx, finished_rx) = mpsc::channel();
        let worker = std::thread::spawn(move || {
            let result = recover_interrupted_output_transactions(
                &worker_transaction.cache_dir,
                &worker_transaction.build_dir,
            );
            finished_tx.send(result).expect("send recovery result");
        });
        assert!(
            finished_rx
                .recv_timeout(Duration::from_millis(100))
                .is_err()
        );
        let journal_path = transaction.journal.path().join("journal.bin");
        let mut changed = read_header(transaction.journal.path()).expect("read journal");
        changed.action = ActionKey::new(PackageKey::root(), "changed").expect("changed action");
        fs::write(&journal_path, encode_journal(&changed)).expect("replace journal");

        drop(held);
        let error = finished_rx
            .recv_timeout(Duration::from_secs(5))
            .expect("recovery result")
            .expect_err("changed journal must fail");
        worker.join().expect("recovery worker");
        assert_eq!(error.operation, "revalidate locked journal for");
        assert_eq!(error.action.as_ref().map(ActionKey::name), Some("tool"));
        assert_eq!(
            fs::read(&transaction.destinations[0]).expect("read destination"),
            b"accepted"
        );
        assert!(transaction.staged.exists());
        let root = transaction.root.clone();
        drop(transaction);
        let _ = fs::remove_dir_all(root);
    }

    #[cfg(target_os = "linux")]
    #[test]
    fn dead_temporary_journal_is_collected_without_touching_live_one() {
        let root = std::env::temp_dir().join(format!(
            "nia-output-recovery-temp-{}-{}",
            std::process::id(),
            JOURNAL_SEQUENCE.fetch_add(1, Ordering::Relaxed)
        ));
        let cache_dir = root.join(".nia-cache");
        let build_dir = root.join(".nia-build");
        let journals = journal_root(&build_dir);
        fs::create_dir_all(&journals).expect("create journal root");
        let current = ProcessIdentity::current();
        let dead = journals.join(format!(
            ".nia-output-{}-{}-0.tmp",
            current.pid,
            current.start_time + 1
        ));
        let live = journals.join(format!(
            ".nia-output-{}-{}-1.tmp",
            current.pid, current.start_time
        ));
        fs::create_dir(&dead).expect("create dead temporary");
        fs::create_dir(&live).expect("create live temporary");

        recover_interrupted_output_transactions(&cache_dir, &build_dir).expect("collect");

        assert!(!dead.exists());
        assert!(live.exists());
        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn journal_codec_rejects_truncation_trailing_bytes_and_overlapping_outputs() {
        let output =
            LogicalPath::new(LogicalPathRoot::Build, "tool/result.txt").expect("logical output");
        let header = JournalHeader {
            action: ActionKey::new(PackageKey::root(), "tool").expect("action"),
            outputs: vec![TransactionOutput {
                path: output.clone(),
                kind: TransactionOutputKind::Directory,
            }],
            staged_path: LogicalPath::new(LogicalPathRoot::Build, "tool/.nia-command-test-0.stage")
                .expect("staged path"),
            committed_path: LogicalPath::new(
                LogicalPathRoot::Build,
                "tool/.nia-command-test-0.committed",
            )
            .expect("committed path"),
        };
        let encoded = encode_journal(&header);
        assert_eq!(decode_journal(&encoded), Some(header.clone()));
        let mut unknown_kind_payload = decode_envelope(&encoded, OUTPUT_TRANSACTION_JOURNAL.magic)
            .expect("journal payload")
            .to_vec();
        let kind_offset = {
            let mut cursor = Cursor::new(unknown_kind_payload.as_slice());
            for _ in 0..4 {
                read_text(&mut cursor, unknown_kind_payload.len()).expect("journal text");
            }
            assert_eq!(read_u64(&mut cursor), Some(1));
            usize::try_from(cursor.position()).expect("kind offset")
        };
        unknown_kind_payload[kind_offset] = u8::MAX;
        assert!(
            decode_journal(&encode_envelope(
                OUTPUT_TRANSACTION_JOURNAL.magic,
                &unknown_kind_payload,
            ))
            .is_none()
        );
        assert!(decode_journal(&encoded[..encoded.len() - 1]).is_none());
        let mut trailing = encoded.clone();
        trailing.push(0);
        assert!(decode_journal(&trailing).is_none());
        let duplicate = JournalHeader {
            outputs: vec![
                TransactionOutput {
                    path: output.clone(),
                    kind: TransactionOutputKind::File,
                },
                TransactionOutput {
                    path: output,
                    kind: TransactionOutputKind::Directory,
                },
            ],
            ..header
        };
        assert!(decode_journal(&encode_journal(&duplicate)).is_none());
        let nested = JournalHeader {
            outputs: vec![
                TransactionOutput {
                    path: LogicalPath::new(LogicalPathRoot::Build, "tool").unwrap(),
                    kind: TransactionOutputKind::Directory,
                },
                TransactionOutput {
                    path: LogicalPath::new(LogicalPathRoot::Build, "tool/result.txt").unwrap(),
                    kind: TransactionOutputKind::File,
                },
            ],
            ..duplicate
        };
        assert!(decode_journal(&encode_journal(&nested)).is_none());
    }

    #[test]
    fn bounded_journal_read_rechecks_stream_length_after_metadata() {
        let mut contents = Cursor::new(vec![0; MAX_JOURNAL_BYTES + 1]);
        let error = read_bounded_contents(&mut contents, 1)
            .expect_err("a record that grows after metadata must be rejected");

        assert_eq!(error.kind(), io::ErrorKind::InvalidData);
        assert!(error.to_string().contains("exceeds its size limit"));
        assert!(validate_transaction_record_size(MAX_JOURNAL_BYTES).is_ok());
        assert_eq!(
            validate_transaction_record_size(MAX_JOURNAL_BYTES + 1)
                .expect_err("publication must share the read-side limit")
                .kind(),
            io::ErrorKind::InvalidData
        );
    }
}
