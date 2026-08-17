// SPDX-License-Identifier: GPL-3.0-or-later
use std::{
    fs::{self, File, OpenOptions},
    io::{self, Read, Seek, SeekFrom, Write},
    path::{Path, PathBuf},
    sync::atomic::{AtomicU64, Ordering},
};

use nia_compat::formats::{ARCHIVE_RESULT, ARCHIVE_RESULT_CACHE};
use nia_linker::{
    ArchiveCacheKey, ArchiveFingerprint, ArchiveFingerprintComponents, ArchiveFingerprintSet,
    ArchiveInvalidation,
};
use nia_query::{FingerprintDomain, QueryFingerprintBuilder};

const ARCHIVE_RESULT_PAYLOAD_DOMAIN: FingerprintDomain =
    FingerprintDomain::new("nia.archive-result-payload.v1");
const ARCHIVE_CACHE_STREAM_BYTES: usize = 64 * 1024;
static ARCHIVE_CACHE_STAGE_ID: AtomicU64 = AtomicU64::new(0);

#[derive(Debug)]
pub(crate) struct PersistentArchiveCache {
    root: PathBuf,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum ArchiveCacheLookup {
    Hit,
    NotFound,
    Invalidated(ArchiveInvalidation),
    Corrupt,
}

impl PersistentArchiveCache {
    pub(crate) fn new(root: PathBuf) -> Self {
        Self { root }
    }

    pub(crate) fn restore(
        &self,
        fingerprints: ArchiveFingerprintSet,
        output: &Path,
    ) -> io::Result<ArchiveCacheLookup> {
        let path = self.path(fingerprints.cache_key, fingerprints.fingerprint);
        let mut entry = match File::open(&path) {
            Ok(file) => file,
            Err(error) if error.kind() == io::ErrorKind::NotFound => {
                return self.lookup_invalidation(fingerprints);
            }
            Err(error) => return Err(error),
        };
        let encoded_len = entry.metadata()?.len();
        let Some(header) = read_archive_header(&mut entry, encoded_len)? else {
            retire_corrupt(&path, &mut entry);
            return Ok(ArchiveCacheLookup::Corrupt);
        };
        if header.fingerprints != fingerprints {
            retire_corrupt(&path, &mut entry);
            return Ok(ArchiveCacheLookup::Corrupt);
        }

        let parent = output
            .parent()
            .ok_or_else(|| io::Error::other("invalid static archive output path"))?;
        if !parent.as_os_str().is_empty() {
            fs::create_dir_all(parent)?;
        }
        let staged = staged_path(output);
        let result: io::Result<bool> = (|| {
            let mut staged_file = OpenOptions::new()
                .write(true)
                .create_new(true)
                .open(&staged)?;
            if !stream_payload(
                &mut entry,
                header.payload_len,
                header.checksum,
                Some(&mut staged_file),
            )? {
                return Ok(false);
            }
            staged_file.sync_all()?;
            drop(staged_file);
            fs::rename(&staged, output)?;
            Ok(true)
        })();
        if result.is_err() || staged.exists() {
            let _ = fs::remove_file(&staged);
        }
        match result? {
            true => Ok(ArchiveCacheLookup::Hit),
            false => {
                retire_corrupt(&path, &mut entry);
                Ok(ArchiveCacheLookup::Corrupt)
            }
        }
    }

    pub(crate) fn publish(
        &self,
        fingerprints: ArchiveFingerprintSet,
        output: &Path,
    ) -> io::Result<()> {
        let mut archive = PublishedArchive::open(output)?;
        let path = self.path(fingerprints.cache_key, fingerprints.fingerprint);
        let parent = path
            .parent()
            .ok_or_else(|| io::Error::other("invalid static archive cache path"))?;
        fs::create_dir_all(parent)?;
        let staged = staged_path(&path);
        let result = (|| {
            let mut staged_file = OpenOptions::new()
                .write(true)
                .create_new(true)
                .open(&staged)?;
            write_archive(&mut staged_file, fingerprints, &mut archive)?;
            staged_file.sync_all()?;
            drop(staged_file);

            let _lock = ArchiveCacheMutationLock::acquire(&path)?;
            match compare_installed(&path, fingerprints, &mut archive)? {
                InstalledEntry::NotFound => {}
                InstalledEntry::Identical => return Ok(()),
                InstalledEntry::Corrupt => fs::remove_file(&path)?,
                InstalledEntry::Collision => {
                    return Err(io::Error::new(
                        io::ErrorKind::AlreadyExists,
                        "static archive cache fingerprint collision",
                    ));
                }
            }
            fs::rename(&staged, &path)?;
            if let Ok(directory) = File::open(parent) {
                let _ = directory.sync_all();
            }
            Ok(())
        })();
        if result.is_err() || staged.exists() {
            let _ = fs::remove_file(&staged);
        }
        result
    }

    fn lookup_invalidation(
        &self,
        expected: ArchiveFingerprintSet,
    ) -> io::Result<ArchiveCacheLookup> {
        let directory = self.key_dir(expected.cache_key);
        let entries = match fs::read_dir(&directory) {
            Ok(entries) => entries,
            Err(error) if error.kind() == io::ErrorKind::NotFound => {
                return Ok(ArchiveCacheLookup::NotFound);
            }
            Err(error) => return Err(error),
        };
        let mut nearest = None::<(u32, ArchiveFingerprint, ArchiveInvalidation)>;
        let mut corrupt = false;
        for entry in entries {
            let path = entry?.path();
            if path.extension().and_then(|value| value.to_str()) != Some("archive") {
                continue;
            }
            let mut file = match File::open(&path) {
                Ok(file) => file,
                Err(error) if error.kind() == io::ErrorKind::NotFound => continue,
                Err(error) => return Err(error),
            };
            let encoded_len = file.metadata()?.len();
            let Some(header) = read_archive_header(&mut file, encoded_len)? else {
                retire_corrupt(&path, &mut file);
                corrupt = true;
                continue;
            };
            if header.fingerprints.cache_key != expected.cache_key
                || path
                    != self.path(
                        header.fingerprints.cache_key,
                        header.fingerprints.fingerprint,
                    )
                || !stream_payload(
                    &mut file,
                    header.payload_len,
                    header.checksum,
                    None::<&mut File>,
                )?
            {
                retire_corrupt(&path, &mut file);
                corrupt = true;
                continue;
            }
            let reasons =
                ArchiveInvalidation::between(header.fingerprints.components, expected.components);
            let candidate = (reasons.count(), header.fingerprints.fingerprint, reasons);
            if nearest
                .as_ref()
                .is_none_or(|current| (candidate.0, candidate.1) < (current.0, current.1))
            {
                nearest = Some(candidate);
            }
        }
        if let Some((_, _, reasons)) = nearest {
            Ok(ArchiveCacheLookup::Invalidated(reasons))
        } else if corrupt {
            Ok(ArchiveCacheLookup::Corrupt)
        } else {
            Ok(ArchiveCacheLookup::NotFound)
        }
    }

    fn key_dir(&self, cache_key: ArchiveCacheKey) -> PathBuf {
        let [first, second] = cache_key.parts();
        self.root
            .join("artifacts")
            .join("archives")
            .join(ARCHIVE_RESULT_CACHE.path_component)
            .join(format!("{first:016x}{second:016x}"))
    }

    fn path(&self, cache_key: ArchiveCacheKey, fingerprint: ArchiveFingerprint) -> PathBuf {
        let [first, second] = fingerprint.parts();
        self.key_dir(cache_key)
            .join(format!("{first:016x}{second:016x}.archive"))
    }
}

#[derive(Clone, Copy)]
struct ArchiveHeader {
    fingerprints: ArchiveFingerprintSet,
    payload_len: u64,
    checksum: ArchiveFingerprint,
}

struct PublishedArchive {
    file: File,
    length: u64,
    checksum: ArchiveFingerprint,
}

impl PublishedArchive {
    /// Captures the archive through one opened regular-file handle. Publication
    /// makes a second pass through that same handle and must reproduce this
    /// length/checksum, so an output mutation cannot poison the cache envelope.
    fn open(path: &Path) -> io::Result<Self> {
        let mut file = File::open(path)?;
        let metadata = file.metadata()?;
        if !metadata.is_file() {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                "static archive output must be a regular file",
            ));
        }
        let length = metadata.len();
        let checksum = fingerprint_file(&mut file, length)?;
        Ok(Self {
            file,
            length,
            checksum,
        })
    }

    fn write_verified_to(&mut self, output: &mut impl Write) -> io::Result<()> {
        stream_file(&mut self.file, self.length, self.checksum, Some(output))
    }
}

enum InstalledEntry {
    NotFound,
    Identical,
    Corrupt,
    Collision,
}

fn read_archive_header(
    reader: &mut (impl Read + Seek),
    encoded_len: u64,
) -> io::Result<Option<ArchiveHeader>> {
    reader.seek(SeekFrom::Start(0))?;
    let Some(magic) = read_array::<8>(reader)? else {
        return Ok(None);
    };
    if magic != *ARCHIVE_RESULT.magic {
        return Ok(None);
    }
    let Some(cache_key_first) = read_u64(reader)? else {
        return Ok(None);
    };
    let Some(cache_key_second) = read_u64(reader)? else {
        return Ok(None);
    };
    let cache_key = ArchiveCacheKey::from_parts([cache_key_first, cache_key_second]);
    let Some(fingerprint) = read_fingerprint(reader)? else {
        return Ok(None);
    };
    let Some(inputs) = read_fingerprint(reader)? else {
        return Ok(None);
    };
    let Some(toolchain) = read_fingerprint(reader)? else {
        return Ok(None);
    };
    let Some(target) = read_fingerprint(reader)? else {
        return Ok(None);
    };
    let Some(tool) = read_fingerprint(reader)? else {
        return Ok(None);
    };
    let Some(options) = read_fingerprint(reader)? else {
        return Ok(None);
    };
    let fingerprints = ArchiveFingerprintSet::new(
        cache_key,
        ArchiveFingerprintComponents {
            inputs,
            toolchain,
            target,
            tool,
            options,
        },
    );
    if fingerprints.fingerprint != fingerprint {
        return Ok(None);
    }
    let Some(payload_len) = read_u64(reader)? else {
        return Ok(None);
    };
    let Some(checksum) = read_fingerprint(reader)? else {
        return Ok(None);
    };
    let payload_offset = reader.stream_position()?;
    let Some(expected_len) = payload_offset.checked_add(payload_len) else {
        return Ok(None);
    };
    if encoded_len != expected_len {
        return Ok(None);
    }
    Ok(Some(ArchiveHeader {
        fingerprints,
        payload_len,
        checksum,
    }))
}

fn stream_payload<W: Write>(
    entry: &mut File,
    payload_len: u64,
    expected_checksum: ArchiveFingerprint,
    mut output: Option<&mut W>,
) -> io::Result<bool> {
    let mut builder = QueryFingerprintBuilder::new(ARCHIVE_RESULT_PAYLOAD_DOMAIN);
    let mut checksum = builder.bytes_writer(payload_len);
    let mut buffer = [0; ARCHIVE_CACHE_STREAM_BYTES];
    let mut remaining = payload_len;
    while remaining != 0 {
        let chunk_len = usize::try_from(remaining.min(buffer.len() as u64)).unwrap();
        if !read_exact_or_invalid(entry, &mut buffer[..chunk_len])? {
            return Ok(false);
        }
        checksum.write_chunk(&buffer[..chunk_len])?;
        if let Some(output) = output.as_deref_mut() {
            output.write_all(&buffer[..chunk_len])?;
        }
        remaining -= chunk_len as u64;
    }
    checksum.finish()?;
    Ok(ArchiveFingerprint::from_parts(builder.finish().parts()) == expected_checksum)
}

fn compare_installed(
    path: &Path,
    fingerprints: ArchiveFingerprintSet,
    archive: &mut PublishedArchive,
) -> io::Result<InstalledEntry> {
    let mut entry = match File::open(path) {
        Ok(file) => file,
        Err(error) if error.kind() == io::ErrorKind::NotFound => {
            return Ok(InstalledEntry::NotFound);
        }
        Err(error) => return Err(error),
    };
    let encoded_len = entry.metadata()?.len();
    let Some(header) = read_archive_header(&mut entry, encoded_len)? else {
        return Ok(InstalledEntry::Corrupt);
    };
    if header.fingerprints != fingerprints {
        return Ok(InstalledEntry::Collision);
    }
    if header.payload_len != archive.length {
        return Ok(InstalledEntry::Collision);
    }
    entry.seek(SeekFrom::Start(encoded_len - header.payload_len))?;
    archive.file.seek(SeekFrom::Start(0))?;
    let mut entry_builder = QueryFingerprintBuilder::new(ARCHIVE_RESULT_PAYLOAD_DOMAIN);
    let mut archive_builder = QueryFingerprintBuilder::new(ARCHIVE_RESULT_PAYLOAD_DOMAIN);
    let mut entry_checksum = entry_builder.bytes_writer(header.payload_len);
    let mut archive_checksum = archive_builder.bytes_writer(archive.length);
    let mut entry_buffer = [0; ARCHIVE_CACHE_STREAM_BYTES];
    let mut archive_buffer = [0; ARCHIVE_CACHE_STREAM_BYTES];
    let mut remaining = header.payload_len;
    let mut identical = true;
    while remaining != 0 {
        let chunk_len = usize::try_from(remaining.min(entry_buffer.len() as u64)).unwrap();
        if !read_exact_or_invalid(&mut entry, &mut entry_buffer[..chunk_len])? {
            return Ok(InstalledEntry::Corrupt);
        }
        archive.file.read_exact(&mut archive_buffer[..chunk_len])?;
        entry_checksum.write_chunk(&entry_buffer[..chunk_len])?;
        archive_checksum.write_chunk(&archive_buffer[..chunk_len])?;
        identical &= entry_buffer[..chunk_len] == archive_buffer[..chunk_len];
        remaining -= chunk_len as u64;
    }
    entry_checksum.finish()?;
    archive_checksum.finish()?;
    if ArchiveFingerprint::from_parts(entry_builder.finish().parts()) != header.checksum {
        return Ok(InstalledEntry::Corrupt);
    }
    if ArchiveFingerprint::from_parts(archive_builder.finish().parts()) != archive.checksum
        || archive.file.metadata()?.len() != archive.length
    {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            "static archive output changed during cache publication",
        ));
    }
    Ok(if identical {
        InstalledEntry::Identical
    } else {
        InstalledEntry::Collision
    })
}

fn write_archive(
    output: &mut impl Write,
    fingerprints: ArchiveFingerprintSet,
    archive: &mut PublishedArchive,
) -> io::Result<()> {
    output.write_all(ARCHIVE_RESULT.magic)?;
    for part in fingerprints.cache_key.parts() {
        output.write_all(&part.to_le_bytes())?;
    }
    write_fingerprint(output, fingerprints.fingerprint)?;
    for component in [
        fingerprints.components.inputs,
        fingerprints.components.toolchain,
        fingerprints.components.target,
        fingerprints.components.tool,
        fingerprints.components.options,
    ] {
        write_fingerprint(output, component)?;
    }
    output.write_all(&archive.length.to_le_bytes())?;
    write_fingerprint(output, archive.checksum)?;
    archive.write_verified_to(output)
}

fn fingerprint_file(file: &mut File, length: u64) -> io::Result<ArchiveFingerprint> {
    let mut builder = QueryFingerprintBuilder::new(ARCHIVE_RESULT_PAYLOAD_DOMAIN);
    let mut checksum = builder.bytes_writer(length);
    stream_file_chunks(file, length, |chunk| checksum.write_chunk(chunk))?;
    checksum.finish()?;
    Ok(ArchiveFingerprint::from_parts(builder.finish().parts()))
}

fn stream_file(
    file: &mut File,
    length: u64,
    expected_checksum: ArchiveFingerprint,
    mut output: Option<&mut impl Write>,
) -> io::Result<()> {
    let mut builder = QueryFingerprintBuilder::new(ARCHIVE_RESULT_PAYLOAD_DOMAIN);
    let mut checksum = builder.bytes_writer(length);
    stream_file_chunks(file, length, |chunk| {
        checksum.write_chunk(chunk)?;
        if let Some(output) = output.as_deref_mut() {
            output.write_all(chunk)?;
        }
        Ok(())
    })?;
    checksum.finish()?;
    if ArchiveFingerprint::from_parts(builder.finish().parts()) != expected_checksum {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            "static archive output changed before cache publication",
        ));
    }
    Ok(())
}

fn stream_file_chunks(
    file: &mut File,
    length: u64,
    mut consume: impl FnMut(&[u8]) -> io::Result<()>,
) -> io::Result<()> {
    let metadata = file.metadata()?;
    if !metadata.is_file() || metadata.len() != length {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            "static archive output changed length before cache publication",
        ));
    }
    file.seek(SeekFrom::Start(0))?;
    let mut buffer = [0; ARCHIVE_CACHE_STREAM_BYTES];
    let mut remaining = length;
    while remaining != 0 {
        let chunk_len = usize::try_from(remaining.min(buffer.len() as u64)).unwrap();
        file.read_exact(&mut buffer[..chunk_len])?;
        consume(&buffer[..chunk_len])?;
        remaining -= chunk_len as u64;
    }
    if file.metadata()?.len() != length {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            "static archive output changed length while cache publication read it",
        ));
    }
    Ok(())
}

fn staged_path(path: &Path) -> PathBuf {
    path.with_extension(format!(
        "tmp.{}.{}",
        std::process::id(),
        ARCHIVE_CACHE_STAGE_ID.fetch_add(1, Ordering::Relaxed)
    ))
}

struct ArchiveCacheMutationLock {
    _file: File,
}

impl ArchiveCacheMutationLock {
    fn acquire(path: &Path) -> io::Result<Self> {
        let file = OpenOptions::new()
            .read(true)
            .write(true)
            .create(true)
            .truncate(false)
            .open(path.with_extension("lock"))?;
        file.lock()?;
        Ok(Self { _file: file })
    }
}

/// Retires only the exact opened record that failed validation. Publishers use
/// the same per-entry lock, and the fixed-buffer comparison prevents a stale
/// reader from deleting a valid replacement installed while it was decoding.
fn retire_corrupt(path: &Path, observed: &mut File) {
    let Ok(_lock) = ArchiveCacheMutationLock::acquire(path) else {
        return;
    };
    let Ok(mut current) = File::open(path) else {
        return;
    };
    if files_equal(observed, &mut current).unwrap_or(false) {
        let _ = fs::remove_file(path);
    }
}

fn files_equal(left: &mut File, right: &mut File) -> io::Result<bool> {
    let left_len = left.metadata()?.len();
    if left_len != right.metadata()?.len() {
        return Ok(false);
    }
    left.seek(SeekFrom::Start(0))?;
    right.seek(SeekFrom::Start(0))?;
    let mut left_buffer = [0; ARCHIVE_CACHE_STREAM_BYTES];
    let mut right_buffer = [0; ARCHIVE_CACHE_STREAM_BYTES];
    let mut remaining = left_len;
    while remaining != 0 {
        let chunk_len = usize::try_from(remaining.min(left_buffer.len() as u64)).unwrap();
        left.read_exact(&mut left_buffer[..chunk_len])?;
        right.read_exact(&mut right_buffer[..chunk_len])?;
        if left_buffer[..chunk_len] != right_buffer[..chunk_len] {
            return Ok(false);
        }
        remaining -= chunk_len as u64;
    }
    Ok(true)
}

fn read_exact_or_invalid(reader: &mut impl Read, bytes: &mut [u8]) -> io::Result<bool> {
    match reader.read_exact(bytes) {
        Ok(()) => Ok(true),
        Err(error) if error.kind() == io::ErrorKind::UnexpectedEof => Ok(false),
        Err(error) => Err(error),
    }
}

fn read_array<const N: usize>(reader: &mut impl Read) -> io::Result<Option<[u8; N]>> {
    let mut bytes = [0; N];
    Ok(read_exact_or_invalid(reader, &mut bytes)?.then_some(bytes))
}

fn write_fingerprint(output: &mut impl Write, fingerprint: ArchiveFingerprint) -> io::Result<()> {
    for part in fingerprint.parts() {
        output.write_all(&part.to_le_bytes())?;
    }
    Ok(())
}

fn read_fingerprint(reader: &mut impl Read) -> io::Result<Option<ArchiveFingerprint>> {
    let Some(first) = read_u64(reader)? else {
        return Ok(None);
    };
    let Some(second) = read_u64(reader)? else {
        return Ok(None);
    };
    Ok(Some(ArchiveFingerprint::from_parts([first, second])))
}

fn read_u64(reader: &mut impl Read) -> io::Result<Option<u64>> {
    Ok(read_array::<8>(reader)?.map(u64::from_le_bytes))
}

#[cfg(test)]
fn encode_archive(fingerprints: ArchiveFingerprintSet, bytes: &[u8]) -> Vec<u8> {
    let mut encoded = Vec::new();
    encoded.extend_from_slice(ARCHIVE_RESULT.magic);
    for part in fingerprints.cache_key.parts() {
        encoded.extend_from_slice(&part.to_le_bytes());
    }
    write_fingerprint(&mut encoded, fingerprints.fingerprint).expect("write fingerprint");
    for component in [
        fingerprints.components.inputs,
        fingerprints.components.toolchain,
        fingerprints.components.target,
        fingerprints.components.tool,
        fingerprints.components.options,
    ] {
        write_fingerprint(&mut encoded, component).expect("write component");
    }
    encoded.extend_from_slice(&(bytes.len() as u64).to_le_bytes());
    write_fingerprint(&mut encoded, payload_checksum(bytes)).expect("write checksum");
    encoded.extend_from_slice(bytes);
    encoded
}

#[cfg(test)]
fn decode_archive(encoded: &[u8]) -> Option<(ArchiveFingerprintSet, Vec<u8>)> {
    let mut cursor = io::Cursor::new(encoded);
    let header = read_archive_header(&mut cursor, encoded.len() as u64).ok()??;
    let mut payload = vec![0; usize::try_from(header.payload_len).ok()?];
    cursor.read_exact(&mut payload).ok()?;
    (payload_checksum(&payload) == header.checksum).then_some((header.fingerprints, payload))
}

#[cfg(test)]
fn payload_checksum(bytes: &[u8]) -> ArchiveFingerprint {
    let mut builder = QueryFingerprintBuilder::new(ARCHIVE_RESULT_PAYLOAD_DOMAIN);
    builder.write_bytes(bytes);
    ArchiveFingerprint::from_parts(builder.finish().parts())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn temp_root(name: &str) -> PathBuf {
        std::env::temp_dir().join(format!(
            "nia_archive_cache_{name}_{}_{}",
            std::process::id(),
            ARCHIVE_CACHE_STAGE_ID.fetch_add(1, Ordering::Relaxed)
        ))
    }

    fn fingerprints(seed: u64) -> ArchiveFingerprintSet {
        ArchiveFingerprintSet::new(
            ArchiveCacheKey::from_parts([1, 2]),
            ArchiveFingerprintComponents {
                inputs: ArchiveFingerprint::from_parts([seed, 1]),
                toolchain: ArchiveFingerprint::from_parts([seed, 2]),
                target: ArchiveFingerprint::from_parts([seed, 3]),
                tool: ArchiveFingerprint::from_parts([seed, 4]),
                options: ArchiveFingerprint::from_parts([seed, 5]),
            },
        )
    }

    #[test]
    fn persistent_archive_round_trips() {
        let root = temp_root("round_trip");
        let cache = PersistentArchiveCache::new(root.clone());
        let fingerprints = fingerprints(10);
        let archive = root.join("archive");
        let restored = root.join("nested/restored");
        fs::create_dir_all(&root).expect("create cache root");
        fs::write(&archive, b"static archive").expect("write archive");

        assert_eq!(
            cache.restore(fingerprints, &restored).expect("cold miss"),
            ArchiveCacheLookup::NotFound
        );
        cache
            .publish(fingerprints, &archive)
            .expect("publish archive");
        assert_eq!(
            cache
                .restore(fingerprints, &restored)
                .expect("restore archive"),
            ArchiveCacheLookup::Hit
        );
        assert_eq!(
            fs::read(restored).expect("read restored"),
            b"static archive"
        );
        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn large_archive_round_trips_through_streaming_paths() {
        let root = temp_root("large");
        let cache = PersistentArchiveCache::new(root.clone());
        let fingerprints = fingerprints(12);
        let archive = root.join("archive");
        let restored = root.join("restored");
        let payload = vec![0x4d; ARCHIVE_CACHE_STREAM_BYTES * 5 + 23];
        fs::create_dir_all(&root).expect("create cache root");
        fs::write(&archive, &payload).expect("write archive");

        cache.publish(fingerprints, &archive).expect("publish");
        assert_eq!(
            cache.restore(fingerprints, &restored).expect("restore"),
            ArchiveCacheLookup::Hit
        );
        assert_eq!(fs::read(restored).expect("read restored"), payload);
        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn archive_entry_codec_rejects_noncanonical_bytes() {
        let fingerprints = fingerprints(15);
        let encoded = encode_archive(fingerprints, b"archive payload");

        for end in 0..encoded.len() {
            assert!(decode_archive(&encoded[..end]).is_none());
        }
        let mut trailing = encoded.clone();
        trailing.push(0);
        assert!(decode_archive(&trailing).is_none());
        let mut damaged = encoded;
        *damaged.last_mut().expect("archive payload byte") ^= 0xff;
        assert!(decode_archive(&damaged).is_none());
    }

    #[test]
    fn corrupt_archive_is_retired_and_can_be_republished() {
        let root = temp_root("corrupt");
        let cache = PersistentArchiveCache::new(root.clone());
        let fingerprints = fingerprints(20);
        let archive = root.join("archive");
        fs::create_dir_all(&root).expect("create cache root");
        fs::write(&archive, b"first").expect("write first archive");
        cache
            .publish(fingerprints, &archive)
            .expect("publish first");
        fs::write(
            cache.path(fingerprints.cache_key, fingerprints.fingerprint),
            b"corrupt",
        )
        .expect("corrupt cache");

        assert_eq!(
            cache
                .restore(fingerprints, &root.join("restored"))
                .expect("corrupt miss"),
            ArchiveCacheLookup::Corrupt
        );
        fs::write(&archive, b"second").expect("write second archive");
        cache
            .publish(fingerprints, &archive)
            .expect("republish archive");
        assert_eq!(
            cache
                .restore(fingerprints, &root.join("restored"))
                .expect("restore second"),
            ArchiveCacheLookup::Hit
        );
        assert_eq!(fs::read(root.join("restored")).unwrap(), b"second");
        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn changed_archive_components_report_nearest_invalidation() {
        let root = temp_root("invalidation");
        let cache = PersistentArchiveCache::new(root.clone());
        let first = fingerprints(30);
        let archive = root.join("archive");
        fs::create_dir_all(&root).expect("create cache root");
        fs::write(&archive, vec![0x7a; ARCHIVE_CACHE_STREAM_BYTES * 3 + 5]).expect("write archive");
        cache.publish(first, &archive).expect("publish first");
        let changed = ArchiveFingerprintSet::new(
            first.cache_key,
            ArchiveFingerprintComponents {
                target: ArchiveFingerprint::from_parts([99, 3]),
                ..first.components
            },
        );

        assert_eq!(
            cache
                .restore(changed, &root.join("changed"))
                .expect("lookup changed"),
            ArchiveCacheLookup::Invalidated(ArchiveInvalidation {
                inputs: false,
                toolchain: false,
                target: true,
                tool: false,
                options: false,
            })
        );
        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn stale_corruption_retirement_preserves_replacement() {
        let root = temp_root("stale-retirement");
        fs::create_dir_all(&root).expect("create cache root");
        let path = root.join("entry.archive");
        fs::write(&path, b"corrupt").expect("write corrupt entry");
        let mut observed = File::open(&path).expect("open observed entry");
        let replacement = root.join("replacement.tmp");
        fs::write(&replacement, b"replacement").expect("write replacement");
        fs::rename(replacement, &path).expect("install replacement");

        retire_corrupt(&path, &mut observed);

        assert_eq!(fs::read(&path).expect("read replacement"), b"replacement");
        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn publication_preserves_winner_and_rejects_collision() {
        let root = temp_root("publication-winner");
        let cache = PersistentArchiveCache::new(root.clone());
        let fingerprints = fingerprints(40);
        let archive = root.join("archive");
        fs::create_dir_all(&root).expect("create cache root");
        fs::write(&archive, b"winner").expect("write winner");
        cache
            .publish(fingerprints, &archive)
            .expect("publish winner");
        cache
            .publish(fingerprints, &archive)
            .expect("accept identical publication");

        fs::write(&archive, b"loser!").expect("write collision");
        let error = cache
            .publish(fingerprints, &archive)
            .expect_err("reject collision");
        assert_eq!(error.kind(), io::ErrorKind::AlreadyExists);
        let restored = root.join("restored");
        assert_eq!(
            cache.restore(fingerprints, &restored).expect("restore"),
            ArchiveCacheLookup::Hit
        );
        assert_eq!(fs::read(restored).expect("read winner"), b"winner");
        let _ = fs::remove_dir_all(root);
    }
}
