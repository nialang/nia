// SPDX-License-Identifier: GPL-3.0-or-later
use std::{
    fs::{self, OpenOptions},
    io::{self, Cursor, Read, Write},
    path::{Path, PathBuf},
    sync::atomic::{AtomicU64, Ordering},
};

use nia_linker::LinkResultFingerprint;
use nia_query::QueryFingerprintBuilder;

const LINK_RESULT_MAGIC: &[u8; 8] = b"NIALNK01";
const LINK_RESULT_SCHEMA: &str = "v1";
static LINK_CACHE_STAGE_ID: AtomicU64 = AtomicU64::new(0);

#[derive(Debug)]
pub(crate) struct PersistentLinkResultCache {
    root: PathBuf,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum LinkResultCacheLookup {
    Hit,
    NotFound,
    Corrupt,
}

enum LinkResultCacheEntryLookup {
    Hit(Vec<u8>),
    NotFound,
    Corrupt,
}

impl PersistentLinkResultCache {
    pub(crate) fn new(root: PathBuf) -> Self {
        Self { root }
    }

    pub(crate) fn restore(
        &self,
        fingerprint: LinkResultFingerprint,
        output: &Path,
    ) -> io::Result<LinkResultCacheLookup> {
        let bytes = match self.load(fingerprint)? {
            LinkResultCacheEntryLookup::Hit(bytes) => bytes,
            LinkResultCacheEntryLookup::NotFound => return Ok(LinkResultCacheLookup::NotFound),
            LinkResultCacheEntryLookup::Corrupt => return Ok(LinkResultCacheLookup::Corrupt),
        };
        let parent = output
            .parent()
            .ok_or_else(|| io::Error::other("invalid executable output path"))?;
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
            make_executable(&staged)?;
            fs::rename(&staged, output)
        })();
        if result.is_err() || staged.exists() {
            let _ = fs::remove_file(&staged);
        }
        result.map(|()| LinkResultCacheLookup::Hit)
    }

    pub(crate) fn publish(
        &self,
        fingerprint: LinkResultFingerprint,
        output: &Path,
    ) -> io::Result<()> {
        if matches!(self.load(fingerprint)?, LinkResultCacheEntryLookup::Hit(_)) {
            return Ok(());
        }
        let bytes = fs::read(output)?;
        let path = self.path(fingerprint);
        let parent = path
            .parent()
            .ok_or_else(|| io::Error::other("invalid link-result cache path"))?;
        fs::create_dir_all(parent)?;
        let staged = staged_path(&path);
        let encoded = encode_link_result(fingerprint, &bytes);
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

    fn load(&self, fingerprint: LinkResultFingerprint) -> io::Result<LinkResultCacheEntryLookup> {
        let path = self.path(fingerprint);
        let encoded = match fs::read(&path) {
            Ok(encoded) => encoded,
            Err(error) if error.kind() == io::ErrorKind::NotFound => {
                return Ok(LinkResultCacheEntryLookup::NotFound);
            }
            Err(error) => return Err(error),
        };
        let Some(bytes) = decode_link_result(&encoded, fingerprint) else {
            remove_corrupt(&path);
            return Ok(LinkResultCacheEntryLookup::Corrupt);
        };
        Ok(LinkResultCacheEntryLookup::Hit(bytes))
    }

    fn path(&self, fingerprint: LinkResultFingerprint) -> PathBuf {
        let [first, second] = fingerprint.parts();
        self.root
            .join("artifacts")
            .join("links")
            .join(LINK_RESULT_SCHEMA)
            .join(format!("{first:016x}{second:016x}.link"))
    }
}

fn staged_path(path: &Path) -> PathBuf {
    path.with_extension(format!(
        "tmp.{}.{}",
        std::process::id(),
        LINK_CACHE_STAGE_ID.fetch_add(1, Ordering::Relaxed)
    ))
}

fn remove_corrupt(path: &Path) {
    match fs::remove_file(path) {
        Ok(()) => {}
        Err(error) if error.kind() == io::ErrorKind::NotFound => {}
        Err(_) => {}
    }
}

fn encode_link_result(fingerprint: LinkResultFingerprint, bytes: &[u8]) -> Vec<u8> {
    let checksum = payload_checksum(bytes);
    let mut encoded = Vec::with_capacity(48 + bytes.len());
    encoded.extend_from_slice(LINK_RESULT_MAGIC);
    for part in fingerprint.parts() {
        encoded.extend_from_slice(&part.to_le_bytes());
    }
    encoded.extend_from_slice(&(bytes.len() as u64).to_le_bytes());
    for part in checksum {
        encoded.extend_from_slice(&part.to_le_bytes());
    }
    encoded.extend_from_slice(bytes);
    encoded
}

fn decode_link_result(
    encoded: &[u8],
    expected_fingerprint: LinkResultFingerprint,
) -> Option<Vec<u8>> {
    let mut cursor = Cursor::new(encoded);
    let mut magic = [0; 8];
    cursor.read_exact(&mut magic).ok()?;
    (magic == *LINK_RESULT_MAGIC).then_some(())?;
    let fingerprint =
        LinkResultFingerprint::from_parts([read_u64(&mut cursor)?, read_u64(&mut cursor)?]);
    (fingerprint == expected_fingerprint).then_some(())?;
    let payload_len = usize::try_from(read_u64(&mut cursor)?).ok()?;
    let checksum = [read_u64(&mut cursor)?, read_u64(&mut cursor)?];
    let position = usize::try_from(cursor.position()).ok()?;
    (encoded.len().checked_sub(position)? == payload_len).then_some(())?;
    let payload = encoded[position..].to_vec();
    (payload_checksum(&payload) == checksum).then_some(payload)
}

fn read_u64(cursor: &mut Cursor<&[u8]>) -> Option<u64> {
    let mut bytes = [0; 8];
    cursor.read_exact(&mut bytes).ok()?;
    Some(u64::from_le_bytes(bytes))
}

fn payload_checksum(bytes: &[u8]) -> [u64; 2] {
    let mut builder = QueryFingerprintBuilder::new("nia.link-result-payload.v1");
    builder.write_bytes(bytes);
    builder.finish().parts()
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
mod tests {
    use super::*;

    fn temp_root(name: &str) -> PathBuf {
        std::env::temp_dir().join(format!(
            "nia_link_cache_{name}_{}_{}",
            std::process::id(),
            LINK_CACHE_STAGE_ID.fetch_add(1, Ordering::Relaxed)
        ))
    }

    #[test]
    fn persistent_link_result_round_trips() {
        let root = temp_root("round_trip");
        let cache = PersistentLinkResultCache::new(root.clone());
        let fingerprint = LinkResultFingerprint::from_parts([10, 20]);
        let linked = root.join("linked");
        let restored = root.join("restored");
        fs::create_dir_all(&root).expect("create cache root");
        fs::write(&linked, b"linked executable").expect("write linked executable");

        assert_eq!(
            cache.restore(fingerprint, &restored).expect("cold miss"),
            LinkResultCacheLookup::NotFound
        );
        cache
            .publish(fingerprint, &linked)
            .expect("publish link result");
        assert_eq!(
            cache
                .restore(fingerprint, &restored)
                .expect("restore link result"),
            LinkResultCacheLookup::Hit
        );
        assert_eq!(
            fs::read(restored).expect("read restored"),
            b"linked executable"
        );
        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn corrupt_link_result_is_retired_and_can_be_republished() {
        let root = temp_root("corrupt");
        let cache = PersistentLinkResultCache::new(root.clone());
        let fingerprint = LinkResultFingerprint::from_parts([30, 40]);
        let linked = root.join("linked");
        let restored = root.join("restored");
        fs::create_dir_all(&root).expect("create cache root");
        fs::write(&linked, b"first").expect("write first executable");
        cache.publish(fingerprint, &linked).expect("publish first");
        fs::write(cache.path(fingerprint), b"corrupt").expect("corrupt cache");

        assert_eq!(
            cache.restore(fingerprint, &restored).expect("corrupt miss"),
            LinkResultCacheLookup::Corrupt
        );
        assert!(!cache.path(fingerprint).exists());
        fs::write(&linked, b"second").expect("write second executable");
        cache
            .publish(fingerprint, &linked)
            .expect("republish link result");
        assert_eq!(
            cache
                .restore(fingerprint, &restored)
                .expect("restore second"),
            LinkResultCacheLookup::Hit
        );
        assert_eq!(fs::read(restored).expect("read second"), b"second");
        let _ = fs::remove_dir_all(root);
    }
}
