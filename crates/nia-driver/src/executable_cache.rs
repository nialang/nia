// SPDX-License-Identifier: GPL-3.0-or-later
use std::{
    fs::{self, File, OpenOptions},
    io::{self, Read, Seek, SeekFrom, Write},
    path::{Path, PathBuf},
    sync::atomic::{AtomicU64, Ordering},
};

use nia_compat::formats::{LINK_RESULT, LINK_RESULT_CACHE};
use nia_linker::{
    LinkResultCacheKey, LinkResultFingerprint, LinkResultFingerprintComponents,
    LinkResultFingerprintSet, LinkResultInvalidation,
};
use nia_query::{FingerprintDomain, QueryFingerprintBuilder};

const LINK_RESULT_PAYLOAD_DOMAIN: FingerprintDomain =
    FingerprintDomain::new("nia.link-result-payload.v2");
const LINK_CACHE_STREAM_BYTES: usize = 64 * 1024;
static LINK_CACHE_STAGE_ID: AtomicU64 = AtomicU64::new(0);

#[derive(Debug)]
pub(crate) struct PersistentLinkResultCache {
    root: PathBuf,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum LinkResultCacheLookup {
    Hit,
    NotFound,
    Invalidated(LinkResultInvalidation),
    Corrupt,
}

impl PersistentLinkResultCache {
    pub(crate) fn new(root: PathBuf) -> Self {
        Self { root }
    }

    pub(crate) fn restore(
        &self,
        fingerprints: LinkResultFingerprintSet,
        output: &Path,
    ) -> io::Result<LinkResultCacheLookup> {
        let path = self.path(fingerprints.cache_key, fingerprints.fingerprint);
        let mut entry = match File::open(&path) {
            Ok(file) => file,
            Err(error) if error.kind() == io::ErrorKind::NotFound => {
                return self.lookup_invalidation(fingerprints);
            }
            Err(error) => return Err(error),
        };
        let encoded_len = entry.metadata()?.len();
        let Some(header) = read_link_result_header(&mut entry, encoded_len)? else {
            retire_corrupt(&path, &mut entry);
            return Ok(LinkResultCacheLookup::Corrupt);
        };
        if header.fingerprints != fingerprints {
            retire_corrupt(&path, &mut entry);
            return Ok(LinkResultCacheLookup::Corrupt);
        }

        let parent = output
            .parent()
            .ok_or_else(|| io::Error::other("invalid linked executable output path"))?;
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
            make_executable(&staged)?;
            staged_file.sync_all()?;
            drop(staged_file);
            fs::rename(&staged, output)?;
            Ok(true)
        })();
        if result.is_err() || staged.exists() {
            let _ = fs::remove_file(&staged);
        }
        match result? {
            true => Ok(LinkResultCacheLookup::Hit),
            false => {
                retire_corrupt(&path, &mut entry);
                Ok(LinkResultCacheLookup::Corrupt)
            }
        }
    }

    pub(crate) fn publish(
        &self,
        fingerprints: LinkResultFingerprintSet,
        output: &Path,
    ) -> io::Result<()> {
        let mut linked = PublishedLinkResult::open(output)?;
        let path = self.path(fingerprints.cache_key, fingerprints.fingerprint);
        let parent = path
            .parent()
            .ok_or_else(|| io::Error::other("invalid link-result cache path"))?;
        fs::create_dir_all(parent)?;
        let staged = staged_path(&path);
        let result = (|| {
            let mut staged_file = OpenOptions::new()
                .write(true)
                .create_new(true)
                .open(&staged)?;
            write_link_result(&mut staged_file, fingerprints, &mut linked)?;
            staged_file.sync_all()?;
            drop(staged_file);

            let _lock = LinkResultCacheMutationLock::acquire(&path)?;
            match compare_installed(&path, fingerprints, &mut linked)? {
                InstalledEntry::NotFound => {}
                InstalledEntry::Identical => return Ok(()),
                InstalledEntry::Corrupt => fs::remove_file(&path)?,
                InstalledEntry::Collision => {
                    return Err(io::Error::new(
                        io::ErrorKind::AlreadyExists,
                        "link-result cache fingerprint collision",
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
        expected: LinkResultFingerprintSet,
    ) -> io::Result<LinkResultCacheLookup> {
        let directory = self.key_dir(expected.cache_key);
        let entries = match fs::read_dir(&directory) {
            Ok(entries) => entries,
            Err(error) if error.kind() == io::ErrorKind::NotFound => {
                return Ok(LinkResultCacheLookup::NotFound);
            }
            Err(error) => return Err(error),
        };
        let mut nearest = None::<(u32, LinkResultFingerprint, LinkResultInvalidation)>;
        let mut corrupt = false;
        for entry in entries {
            let path = entry?.path();
            if path.extension().and_then(|value| value.to_str()) != Some("link") {
                continue;
            }
            let mut file = match File::open(&path) {
                Ok(file) => file,
                Err(error) if error.kind() == io::ErrorKind::NotFound => continue,
                Err(error) => return Err(error),
            };
            let encoded_len = file.metadata()?.len();
            let Some(header) = read_link_result_header(&mut file, encoded_len)? else {
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
            let reasons = LinkResultInvalidation::between(
                header.fingerprints.components,
                expected.components,
            );
            let candidate = (reasons.count(), header.fingerprints.fingerprint, reasons);
            if nearest
                .as_ref()
                .is_none_or(|current| (candidate.0, candidate.1) < (current.0, current.1))
            {
                nearest = Some(candidate);
            }
        }
        if let Some((_, _, reasons)) = nearest {
            Ok(LinkResultCacheLookup::Invalidated(reasons))
        } else if corrupt {
            Ok(LinkResultCacheLookup::Corrupt)
        } else {
            Ok(LinkResultCacheLookup::NotFound)
        }
    }

    fn key_dir(&self, cache_key: LinkResultCacheKey) -> PathBuf {
        let [first, second] = cache_key.parts();
        self.root
            .join("artifacts")
            .join("links")
            .join(LINK_RESULT_CACHE.path_component)
            .join(format!("{first:016x}{second:016x}"))
    }

    fn path(&self, cache_key: LinkResultCacheKey, fingerprint: LinkResultFingerprint) -> PathBuf {
        let [first, second] = fingerprint.parts();
        self.key_dir(cache_key)
            .join(format!("{first:016x}{second:016x}.link"))
    }
}

#[derive(Clone, Copy)]
struct LinkResultHeader {
    fingerprints: LinkResultFingerprintSet,
    payload_len: u64,
    checksum: LinkResultFingerprint,
}

struct PublishedLinkResult {
    file: File,
    length: u64,
    checksum: LinkResultFingerprint,
}

impl PublishedLinkResult {
    /// Captures the linked executable through one opened regular-file handle.
    /// Publication makes a second pass through that same handle and must
    /// reproduce this length/checksum, so output mutation cannot poison the
    /// cache envelope.
    fn open(path: &Path) -> io::Result<Self> {
        let mut file = File::open(path)?;
        let metadata = file.metadata()?;
        if !metadata.is_file() {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                "linked executable output must be a regular file",
            ));
        }
        let length = metadata.len();
        let checksum = fingerprint_link_file(&mut file, length)?;
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

fn read_link_result_header(
    reader: &mut (impl Read + Seek),
    encoded_len: u64,
) -> io::Result<Option<LinkResultHeader>> {
    reader.seek(SeekFrom::Start(0))?;
    let Some(magic) = read_array::<8>(reader)? else {
        return Ok(None);
    };
    if magic != *LINK_RESULT.magic {
        return Ok(None);
    }
    let Some(cache_key_first) = read_u64(reader)? else {
        return Ok(None);
    };
    let Some(cache_key_second) = read_u64(reader)? else {
        return Ok(None);
    };
    let cache_key = LinkResultCacheKey::from_parts([cache_key_first, cache_key_second]);
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
    let Some(linker) = read_fingerprint(reader)? else {
        return Ok(None);
    };
    let Some(options) = read_fingerprint(reader)? else {
        return Ok(None);
    };
    let fingerprints = LinkResultFingerprintSet::new(
        cache_key,
        LinkResultFingerprintComponents {
            inputs,
            toolchain,
            target,
            linker,
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
    Ok(Some(LinkResultHeader {
        fingerprints,
        payload_len,
        checksum,
    }))
}

fn stream_payload<W: Write>(
    entry: &mut File,
    payload_len: u64,
    expected_checksum: LinkResultFingerprint,
    mut output: Option<&mut W>,
) -> io::Result<bool> {
    let mut builder = QueryFingerprintBuilder::new(LINK_RESULT_PAYLOAD_DOMAIN);
    let mut checksum = builder.bytes_writer(payload_len);
    let mut buffer = [0; LINK_CACHE_STREAM_BYTES];
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
    Ok(LinkResultFingerprint::from_parts(builder.finish().parts()) == expected_checksum)
}

fn compare_installed(
    path: &Path,
    fingerprints: LinkResultFingerprintSet,
    linked: &mut PublishedLinkResult,
) -> io::Result<InstalledEntry> {
    let mut entry = match File::open(path) {
        Ok(file) => file,
        Err(error) if error.kind() == io::ErrorKind::NotFound => {
            return Ok(InstalledEntry::NotFound);
        }
        Err(error) => return Err(error),
    };
    let encoded_len = entry.metadata()?.len();
    let Some(header) = read_link_result_header(&mut entry, encoded_len)? else {
        return Ok(InstalledEntry::Corrupt);
    };
    if header.fingerprints != fingerprints {
        return Ok(InstalledEntry::Collision);
    }
    if header.payload_len != linked.length {
        return Ok(InstalledEntry::Collision);
    }
    entry.seek(SeekFrom::Start(encoded_len - header.payload_len))?;
    linked.file.seek(SeekFrom::Start(0))?;
    let mut entry_builder = QueryFingerprintBuilder::new(LINK_RESULT_PAYLOAD_DOMAIN);
    let mut linked_builder = QueryFingerprintBuilder::new(LINK_RESULT_PAYLOAD_DOMAIN);
    let mut entry_checksum = entry_builder.bytes_writer(header.payload_len);
    let mut linked_checksum = linked_builder.bytes_writer(linked.length);
    let mut entry_buffer = [0; LINK_CACHE_STREAM_BYTES];
    let mut linked_buffer = [0; LINK_CACHE_STREAM_BYTES];
    let mut remaining = header.payload_len;
    let mut identical = true;
    while remaining != 0 {
        let chunk_len = usize::try_from(remaining.min(entry_buffer.len() as u64)).unwrap();
        if !read_exact_or_invalid(&mut entry, &mut entry_buffer[..chunk_len])? {
            return Ok(InstalledEntry::Corrupt);
        }
        linked.file.read_exact(&mut linked_buffer[..chunk_len])?;
        entry_checksum.write_chunk(&entry_buffer[..chunk_len])?;
        linked_checksum.write_chunk(&linked_buffer[..chunk_len])?;
        identical &= entry_buffer[..chunk_len] == linked_buffer[..chunk_len];
        remaining -= chunk_len as u64;
    }
    entry_checksum.finish()?;
    linked_checksum.finish()?;
    if LinkResultFingerprint::from_parts(entry_builder.finish().parts()) != header.checksum {
        return Ok(InstalledEntry::Corrupt);
    }
    if LinkResultFingerprint::from_parts(linked_builder.finish().parts()) != linked.checksum
        || linked.file.metadata()?.len() != linked.length
    {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            "linked executable output changed during cache publication",
        ));
    }
    Ok(if identical {
        InstalledEntry::Identical
    } else {
        InstalledEntry::Collision
    })
}

fn write_link_result(
    output: &mut impl Write,
    fingerprints: LinkResultFingerprintSet,
    linked: &mut PublishedLinkResult,
) -> io::Result<()> {
    output.write_all(LINK_RESULT.magic)?;
    for part in fingerprints.cache_key.parts() {
        output.write_all(&part.to_le_bytes())?;
    }
    write_fingerprint(output, fingerprints.fingerprint)?;
    for component in [
        fingerprints.components.inputs,
        fingerprints.components.toolchain,
        fingerprints.components.target,
        fingerprints.components.linker,
        fingerprints.components.options,
    ] {
        write_fingerprint(output, component)?;
    }
    output.write_all(&linked.length.to_le_bytes())?;
    write_fingerprint(output, linked.checksum)?;
    linked.write_verified_to(output)
}

fn fingerprint_link_file(file: &mut File, length: u64) -> io::Result<LinkResultFingerprint> {
    let mut builder = QueryFingerprintBuilder::new(LINK_RESULT_PAYLOAD_DOMAIN);
    let mut checksum = builder.bytes_writer(length);
    stream_file_chunks(file, length, |chunk| checksum.write_chunk(chunk))?;
    checksum.finish()?;
    Ok(LinkResultFingerprint::from_parts(builder.finish().parts()))
}

fn stream_file(
    file: &mut File,
    length: u64,
    expected_checksum: LinkResultFingerprint,
    mut output: Option<&mut impl Write>,
) -> io::Result<()> {
    let mut builder = QueryFingerprintBuilder::new(LINK_RESULT_PAYLOAD_DOMAIN);
    let mut checksum = builder.bytes_writer(length);
    stream_file_chunks(file, length, |chunk| {
        checksum.write_chunk(chunk)?;
        if let Some(output) = output.as_deref_mut() {
            output.write_all(chunk)?;
        }
        Ok(())
    })?;
    checksum.finish()?;
    if LinkResultFingerprint::from_parts(builder.finish().parts()) != expected_checksum {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            "linked executable output changed before cache publication",
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
            "linked executable output changed length before cache publication",
        ));
    }
    file.seek(SeekFrom::Start(0))?;
    let mut buffer = [0; LINK_CACHE_STREAM_BYTES];
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
            "linked executable output changed length while cache publication read it",
        ));
    }
    Ok(())
}

fn staged_path(path: &Path) -> PathBuf {
    path.with_extension(format!(
        "tmp.{}.{}",
        std::process::id(),
        LINK_CACHE_STAGE_ID.fetch_add(1, Ordering::Relaxed)
    ))
}

struct LinkResultCacheMutationLock {
    _file: File,
}

impl LinkResultCacheMutationLock {
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
    let Ok(_lock) = LinkResultCacheMutationLock::acquire(path) else {
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
    let mut left_buffer = [0; LINK_CACHE_STREAM_BYTES];
    let mut right_buffer = [0; LINK_CACHE_STREAM_BYTES];
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

fn write_fingerprint(
    output: &mut impl Write,
    fingerprint: LinkResultFingerprint,
) -> io::Result<()> {
    for part in fingerprint.parts() {
        output.write_all(&part.to_le_bytes())?;
    }
    Ok(())
}

fn read_fingerprint(reader: &mut impl Read) -> io::Result<Option<LinkResultFingerprint>> {
    let Some(first) = read_u64(reader)? else {
        return Ok(None);
    };
    let Some(second) = read_u64(reader)? else {
        return Ok(None);
    };
    Ok(Some(LinkResultFingerprint::from_parts([first, second])))
}

fn read_u64(reader: &mut impl Read) -> io::Result<Option<u64>> {
    Ok(read_array::<8>(reader)?.map(u64::from_le_bytes))
}

#[cfg(unix)]
fn make_executable(path: &Path) -> io::Result<()> {
    use std::os::unix::fs::PermissionsExt;

    let mut permissions = fs::metadata(path)?.permissions();
    permissions.set_mode(0o755);
    fs::set_permissions(path, permissions)
}

#[cfg(not(unix))]
fn make_executable(_path: &Path) -> io::Result<()> {
    Ok(())
}

#[cfg(test)]
fn encode_link_result(fingerprints: LinkResultFingerprintSet, bytes: &[u8]) -> Vec<u8> {
    let mut encoded = Vec::new();
    encoded.extend_from_slice(LINK_RESULT.magic);
    for part in fingerprints.cache_key.parts() {
        encoded.extend_from_slice(&part.to_le_bytes());
    }
    write_fingerprint(&mut encoded, fingerprints.fingerprint).expect("write fingerprint");
    for component in [
        fingerprints.components.inputs,
        fingerprints.components.toolchain,
        fingerprints.components.target,
        fingerprints.components.linker,
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
fn decode_link_result(encoded: &[u8]) -> Option<(LinkResultFingerprintSet, Vec<u8>)> {
    let mut cursor = io::Cursor::new(encoded);
    let header = read_link_result_header(&mut cursor, encoded.len() as u64).ok()??;
    let mut payload = vec![0; usize::try_from(header.payload_len).ok()?];
    cursor.read_exact(&mut payload).ok()?;
    (payload_checksum(&payload) == header.checksum).then_some((header.fingerprints, payload))
}

#[cfg(test)]
fn payload_checksum(bytes: &[u8]) -> LinkResultFingerprint {
    let mut builder = QueryFingerprintBuilder::new(LINK_RESULT_PAYLOAD_DOMAIN);
    builder.write_bytes(bytes);
    LinkResultFingerprint::from_parts(builder.finish().parts())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn temp_root(name: &str) -> PathBuf {
        std::env::temp_dir().join(format!(
            "nia_link_cache_{name}_{}_{}",
            std::process::id(),
            LINK_CACHE_STAGE_ID.fetch_add(1, Ordering::Relaxed)
        ))
    }

    fn fingerprints(seed: u64) -> LinkResultFingerprintSet {
        LinkResultFingerprintSet::new(
            LinkResultCacheKey::from_parts([1, 2]),
            LinkResultFingerprintComponents {
                inputs: LinkResultFingerprint::from_parts([seed, 1]),
                toolchain: LinkResultFingerprint::from_parts([seed, 2]),
                target: LinkResultFingerprint::from_parts([seed, 3]),
                linker: LinkResultFingerprint::from_parts([seed, 4]),
                options: LinkResultFingerprint::from_parts([seed, 5]),
            },
        )
    }

    #[test]
    fn persistent_link_result_round_trips() {
        let root = temp_root("round_trip");
        let cache = PersistentLinkResultCache::new(root.clone());
        let fingerprints = fingerprints(10);
        let linked = root.join("linked");
        let restored = root.join("nested/restored");
        fs::create_dir_all(&root).expect("create cache root");
        fs::write(&linked, b"linked executable").expect("write linked");
        let legacy = root.join("artifacts/links/v1/legacy.link");
        fs::create_dir_all(legacy.parent().expect("legacy parent"))
            .expect("create legacy namespace");
        fs::write(legacy, b"legacy entry").expect("write legacy entry");

        assert_eq!(
            cache.restore(fingerprints, &restored).expect("cold miss"),
            LinkResultCacheLookup::NotFound
        );
        cache
            .publish(fingerprints, &linked)
            .expect("publish linked");
        assert_eq!(
            cache
                .restore(fingerprints, &restored)
                .expect("restore linked"),
            LinkResultCacheLookup::Hit
        );
        assert_eq!(
            fs::read(restored).expect("read restored"),
            b"linked executable"
        );
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;

            assert_ne!(
                fs::metadata(root.join("nested/restored"))
                    .unwrap()
                    .permissions()
                    .mode()
                    & 0o111,
                0
            );
        }
        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn large_link_result_round_trips_through_streaming_paths() {
        let root = temp_root("large");
        let cache = PersistentLinkResultCache::new(root.clone());
        let fingerprints = fingerprints(12);
        let linked = root.join("linked");
        let restored = root.join("restored");
        let payload = vec![0x4d; LINK_CACHE_STREAM_BYTES * 5 + 23];
        fs::create_dir_all(&root).expect("create cache root");
        fs::write(&linked, &payload).expect("write linked");

        cache.publish(fingerprints, &linked).expect("publish");
        assert_eq!(
            cache.restore(fingerprints, &restored).expect("restore"),
            LinkResultCacheLookup::Hit
        );
        assert_eq!(fs::read(restored).expect("read restored"), payload);
        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn link_result_codec_rejects_noncanonical_bytes() {
        let fingerprints = fingerprints(15);
        let encoded = encode_link_result(fingerprints, b"linked executable payload");

        for end in 0..encoded.len() {
            assert!(decode_link_result(&encoded[..end]).is_none());
        }
        let mut trailing = encoded.clone();
        trailing.push(0);
        assert!(decode_link_result(&trailing).is_none());
        let mut damaged = encoded;
        *damaged.last_mut().expect("linked executable payload byte") ^= 0xff;
        assert!(decode_link_result(&damaged).is_none());
    }

    #[test]
    fn corrupt_link_result_is_retired_and_can_be_republished() {
        let root = temp_root("corrupt");
        let cache = PersistentLinkResultCache::new(root.clone());
        let fingerprints = fingerprints(20);
        let linked = root.join("linked");
        fs::create_dir_all(&root).expect("create cache root");
        fs::write(&linked, b"first").expect("write first linked");
        cache.publish(fingerprints, &linked).expect("publish first");
        fs::write(
            cache.path(fingerprints.cache_key, fingerprints.fingerprint),
            b"corrupt",
        )
        .expect("corrupt cache");

        assert_eq!(
            cache
                .restore(fingerprints, &root.join("restored"))
                .expect("corrupt miss"),
            LinkResultCacheLookup::Corrupt
        );
        fs::write(&linked, b"second").expect("write second linked");
        cache
            .publish(fingerprints, &linked)
            .expect("republish linked");
        assert_eq!(
            cache
                .restore(fingerprints, &root.join("restored"))
                .expect("restore second"),
            LinkResultCacheLookup::Hit
        );
        assert_eq!(fs::read(root.join("restored")).unwrap(), b"second");
        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn changed_link_components_report_nearest_invalidation() {
        let root = temp_root("invalidation");
        let cache = PersistentLinkResultCache::new(root.clone());
        let first = fingerprints(30);
        let linked = root.join("linked");
        fs::create_dir_all(&root).expect("create cache root");
        fs::write(&linked, vec![0x7a; LINK_CACHE_STREAM_BYTES * 3 + 5]).expect("write linked");
        cache.publish(first, &linked).expect("publish first");
        let changed = LinkResultFingerprintSet::new(
            first.cache_key,
            LinkResultFingerprintComponents {
                target: LinkResultFingerprint::from_parts([99, 3]),
                ..first.components
            },
        );

        assert_eq!(
            cache
                .restore(changed, &root.join("changed"))
                .expect("lookup changed"),
            LinkResultCacheLookup::Invalidated(LinkResultInvalidation {
                inputs: false,
                toolchain: false,
                target: true,
                linker: false,
                options: false,
            })
        );
        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn stale_corruption_retirement_preserves_replacement() {
        let root = temp_root("stale-retirement");
        fs::create_dir_all(&root).expect("create cache root");
        let path = root.join("entry.link");
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
        let cache = PersistentLinkResultCache::new(root.clone());
        let fingerprints = fingerprints(40);
        let linked = root.join("linked");
        fs::create_dir_all(&root).expect("create cache root");
        fs::write(&linked, b"winner").expect("write winner");
        cache
            .publish(fingerprints, &linked)
            .expect("publish winner");
        cache
            .publish(fingerprints, &linked)
            .expect("accept identical publication");

        fs::write(&linked, b"loser!").expect("write collision");
        let error = cache
            .publish(fingerprints, &linked)
            .expect_err("reject collision");
        assert_eq!(error.kind(), io::ErrorKind::AlreadyExists);
        let restored = root.join("restored");
        assert_eq!(
            cache.restore(fingerprints, &restored).expect("restore"),
            LinkResultCacheLookup::Hit
        );
        assert_eq!(fs::read(restored).expect("read winner"), b"winner");
        let _ = fs::remove_dir_all(root);
    }
}
