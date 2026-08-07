// SPDX-License-Identifier: GPL-3.0-or-later
use std::{
    fs::{self, OpenOptions},
    io::{self, Cursor, Read, Write},
    path::{Path, PathBuf},
    sync::atomic::{AtomicU64, Ordering},
};

use nia_linker::{
    ArchiveCacheKey, ArchiveFingerprint, ArchiveFingerprintComponents, ArchiveFingerprintSet,
    ArchiveInvalidation,
};
use nia_query::QueryFingerprintBuilder;

const ARCHIVE_MAGIC: &[u8; 8] = b"NIAARC01";
const ARCHIVE_SCHEMA: &str = "v1";
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

enum ArchiveCacheEntryLookup {
    Hit(Vec<u8>),
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
        let bytes = match self.load(fingerprints)? {
            ArchiveCacheEntryLookup::Hit(bytes) => bytes,
            ArchiveCacheEntryLookup::NotFound => return Ok(ArchiveCacheLookup::NotFound),
            ArchiveCacheEntryLookup::Invalidated(reasons) => {
                return Ok(ArchiveCacheLookup::Invalidated(reasons));
            }
            ArchiveCacheEntryLookup::Corrupt => return Ok(ArchiveCacheLookup::Corrupt),
        };
        let parent = output
            .parent()
            .ok_or_else(|| io::Error::other("invalid static archive output path"))?;
        if !parent.as_os_str().is_empty() {
            fs::create_dir_all(parent)?;
        }
        let staged = staged_path(output);
        let result = (|| {
            let mut file = OpenOptions::new()
                .write(true)
                .create_new(true)
                .open(&staged)?;
            file.write_all(&bytes)?;
            file.sync_all()?;
            drop(file);
            fs::rename(&staged, output)
        })();
        if result.is_err() || staged.exists() {
            let _ = fs::remove_file(&staged);
        }
        result.map(|()| ArchiveCacheLookup::Hit)
    }

    pub(crate) fn publish(
        &self,
        fingerprints: ArchiveFingerprintSet,
        output: &Path,
    ) -> io::Result<()> {
        if matches!(self.load(fingerprints)?, ArchiveCacheEntryLookup::Hit(_)) {
            return Ok(());
        }
        let bytes = fs::read(output)?;
        let path = self.path(fingerprints.cache_key, fingerprints.fingerprint);
        let parent = path
            .parent()
            .ok_or_else(|| io::Error::other("invalid static archive cache path"))?;
        fs::create_dir_all(parent)?;
        let staged = staged_path(&path);
        let encoded = encode_archive(fingerprints, &bytes);
        let result = (|| {
            let mut file = OpenOptions::new()
                .write(true)
                .create_new(true)
                .open(&staged)?;
            file.write_all(&encoded)?;
            file.sync_all()?;
            drop(file);
            match fs::rename(&staged, &path) {
                Ok(()) => Ok(()),
                Err(_) if path.is_file() => Ok(()),
                Err(error) => Err(error),
            }
        })();
        if result.is_err() || staged.exists() {
            let _ = fs::remove_file(&staged);
        }
        result
    }

    fn load(&self, fingerprints: ArchiveFingerprintSet) -> io::Result<ArchiveCacheEntryLookup> {
        let path = self.path(fingerprints.cache_key, fingerprints.fingerprint);
        let encoded = match fs::read(&path) {
            Ok(encoded) => encoded,
            Err(error) if error.kind() == io::ErrorKind::NotFound => {
                return self.lookup_invalidation(fingerprints);
            }
            Err(error) => return Err(error),
        };
        let Some(entry) = decode_archive(&encoded) else {
            remove_corrupt(&path);
            return Ok(ArchiveCacheEntryLookup::Corrupt);
        };
        if entry.fingerprints != fingerprints
            || path != self.path(entry.fingerprints.cache_key, entry.fingerprints.fingerprint)
        {
            remove_corrupt(&path);
            return Ok(ArchiveCacheEntryLookup::Corrupt);
        }
        Ok(ArchiveCacheEntryLookup::Hit(entry.payload))
    }

    fn lookup_invalidation(
        &self,
        expected: ArchiveFingerprintSet,
    ) -> io::Result<ArchiveCacheEntryLookup> {
        let directory = self.key_dir(expected.cache_key);
        let entries = match fs::read_dir(&directory) {
            Ok(entries) => entries,
            Err(error) if error.kind() == io::ErrorKind::NotFound => {
                return Ok(ArchiveCacheEntryLookup::NotFound);
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
            let encoded = fs::read(&path)?;
            let Some(entry) = decode_archive(&encoded) else {
                remove_corrupt(&path);
                corrupt = true;
                continue;
            };
            if entry.fingerprints.cache_key != expected.cache_key
                || path != self.path(entry.fingerprints.cache_key, entry.fingerprints.fingerprint)
            {
                remove_corrupt(&path);
                corrupt = true;
                continue;
            }
            let reasons =
                ArchiveInvalidation::between(entry.fingerprints.components, expected.components);
            let candidate = (reasons.count(), entry.fingerprints.fingerprint, reasons);
            if nearest
                .as_ref()
                .is_none_or(|current| (candidate.0, candidate.1) < (current.0, current.1))
            {
                nearest = Some(candidate);
            }
        }
        if let Some((_, _, reasons)) = nearest {
            Ok(ArchiveCacheEntryLookup::Invalidated(reasons))
        } else if corrupt {
            Ok(ArchiveCacheEntryLookup::Corrupt)
        } else {
            Ok(ArchiveCacheEntryLookup::NotFound)
        }
    }

    fn key_dir(&self, cache_key: ArchiveCacheKey) -> PathBuf {
        let [first, second] = cache_key.parts();
        self.root
            .join("artifacts")
            .join("archives")
            .join(ARCHIVE_SCHEMA)
            .join(format!("{first:016x}{second:016x}"))
    }

    fn path(&self, cache_key: ArchiveCacheKey, fingerprint: ArchiveFingerprint) -> PathBuf {
        let [first, second] = fingerprint.parts();
        self.key_dir(cache_key)
            .join(format!("{first:016x}{second:016x}.archive"))
    }
}

fn staged_path(path: &Path) -> PathBuf {
    path.with_extension(format!(
        "tmp.{}.{}",
        std::process::id(),
        ARCHIVE_CACHE_STAGE_ID.fetch_add(1, Ordering::Relaxed)
    ))
}

fn remove_corrupt(path: &Path) {
    match fs::remove_file(path) {
        Ok(()) => {}
        Err(error) if error.kind() == io::ErrorKind::NotFound => {}
        Err(_) => {}
    }
}

fn encode_archive(fingerprints: ArchiveFingerprintSet, bytes: &[u8]) -> Vec<u8> {
    let checksum = payload_checksum(bytes);
    let mut encoded = Vec::with_capacity(128 + bytes.len());
    encoded.extend_from_slice(ARCHIVE_MAGIC);
    for part in fingerprints.cache_key.parts() {
        encoded.extend_from_slice(&part.to_le_bytes());
    }
    write_fingerprint(&mut encoded, fingerprints.fingerprint);
    for component in [
        fingerprints.components.inputs,
        fingerprints.components.toolchain,
        fingerprints.components.target,
        fingerprints.components.tool,
        fingerprints.components.options,
    ] {
        write_fingerprint(&mut encoded, component);
    }
    encoded.extend_from_slice(&(bytes.len() as u64).to_le_bytes());
    write_fingerprint(&mut encoded, checksum);
    encoded.extend_from_slice(bytes);
    encoded
}

struct DecodedArchive {
    fingerprints: ArchiveFingerprintSet,
    payload: Vec<u8>,
}

fn decode_archive(encoded: &[u8]) -> Option<DecodedArchive> {
    let mut cursor = Cursor::new(encoded);
    let mut magic = [0; 8];
    cursor.read_exact(&mut magic).ok()?;
    (magic == *ARCHIVE_MAGIC).then_some(())?;
    let cache_key = ArchiveCacheKey::from_parts([read_u64(&mut cursor)?, read_u64(&mut cursor)?]);
    let fingerprint = read_fingerprint(&mut cursor)?;
    let components = ArchiveFingerprintComponents {
        inputs: read_fingerprint(&mut cursor)?,
        toolchain: read_fingerprint(&mut cursor)?,
        target: read_fingerprint(&mut cursor)?,
        tool: read_fingerprint(&mut cursor)?,
        options: read_fingerprint(&mut cursor)?,
    };
    let fingerprints = ArchiveFingerprintSet::new(cache_key, components);
    (fingerprints.fingerprint == fingerprint).then_some(())?;
    let payload_len = usize::try_from(read_u64(&mut cursor)?).ok()?;
    let checksum = read_fingerprint(&mut cursor)?;
    let position = usize::try_from(cursor.position()).ok()?;
    (encoded.len().checked_sub(position)? == payload_len).then_some(())?;
    let payload = encoded[position..].to_vec();
    (payload_checksum(&payload) == checksum).then_some(DecodedArchive {
        fingerprints,
        payload,
    })
}

fn write_fingerprint(encoded: &mut Vec<u8>, fingerprint: ArchiveFingerprint) {
    for part in fingerprint.parts() {
        encoded.extend_from_slice(&part.to_le_bytes());
    }
}

fn read_fingerprint(cursor: &mut Cursor<&[u8]>) -> Option<ArchiveFingerprint> {
    Some(ArchiveFingerprint::from_parts([
        read_u64(cursor)?,
        read_u64(cursor)?,
    ]))
}

fn read_u64(cursor: &mut Cursor<&[u8]>) -> Option<u64> {
    let mut bytes = [0; 8];
    cursor.read_exact(&mut bytes).ok()?;
    Some(u64::from_le_bytes(bytes))
}

fn payload_checksum(bytes: &[u8]) -> ArchiveFingerprint {
    let mut builder = QueryFingerprintBuilder::new("nia.archive-result-payload.v1");
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
        fs::write(&archive, b"first").expect("write archive");
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
}
