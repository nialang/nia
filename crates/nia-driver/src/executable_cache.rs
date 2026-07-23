// SPDX-License-Identifier: GPL-3.0-or-later
use std::{
    fs::{self, OpenOptions},
    io::{self, Cursor, Read, Write},
    path::{Path, PathBuf},
    sync::atomic::{AtomicU64, Ordering},
};

use nia_linker::{
    LinkResultCacheKey, LinkResultFingerprint, LinkResultFingerprintComponents,
    LinkResultFingerprintSet, LinkResultInvalidation,
};
use nia_query::QueryFingerprintBuilder;

const LINK_RESULT_MAGIC: &[u8; 8] = b"NIALNK02";
const LINK_RESULT_SCHEMA: &str = "v2";
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

enum LinkResultCacheEntryLookup {
    Hit(Vec<u8>),
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
        let bytes = match self.load(fingerprints)? {
            LinkResultCacheEntryLookup::Hit(bytes) => bytes,
            LinkResultCacheEntryLookup::NotFound => return Ok(LinkResultCacheLookup::NotFound),
            LinkResultCacheEntryLookup::Invalidated(reasons) => {
                return Ok(LinkResultCacheLookup::Invalidated(reasons));
            }
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
        fingerprints: LinkResultFingerprintSet,
        output: &Path,
    ) -> io::Result<()> {
        if matches!(self.load(fingerprints)?, LinkResultCacheEntryLookup::Hit(_)) {
            return Ok(());
        }
        let bytes = fs::read(output)?;
        let path = self.path(fingerprints.cache_key, fingerprints.fingerprint);
        let parent = path
            .parent()
            .ok_or_else(|| io::Error::other("invalid link-result cache path"))?;
        fs::create_dir_all(parent)?;
        let staged = staged_path(&path);
        let encoded = encode_link_result(fingerprints, &bytes);
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

    fn load(
        &self,
        fingerprints: LinkResultFingerprintSet,
    ) -> io::Result<LinkResultCacheEntryLookup> {
        let path = self.path(fingerprints.cache_key, fingerprints.fingerprint);
        let encoded = match fs::read(&path) {
            Ok(encoded) => encoded,
            Err(error) if error.kind() == io::ErrorKind::NotFound => {
                return self.lookup_invalidation(fingerprints);
            }
            Err(error) => return Err(error),
        };
        let Some(entry) = decode_link_result(&encoded) else {
            remove_corrupt(&path);
            return Ok(LinkResultCacheEntryLookup::Corrupt);
        };
        if entry.fingerprints != fingerprints
            || path != self.path(entry.fingerprints.cache_key, entry.fingerprints.fingerprint)
        {
            remove_corrupt(&path);
            return Ok(LinkResultCacheEntryLookup::Corrupt);
        }
        Ok(LinkResultCacheEntryLookup::Hit(entry.payload))
    }

    fn lookup_invalidation(
        &self,
        expected: LinkResultFingerprintSet,
    ) -> io::Result<LinkResultCacheEntryLookup> {
        let directory = self.key_dir(expected.cache_key);
        let entries = match fs::read_dir(&directory) {
            Ok(entries) => entries,
            Err(error) if error.kind() == io::ErrorKind::NotFound => {
                return Ok(LinkResultCacheEntryLookup::NotFound);
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
            let encoded = fs::read(&path)?;
            let Some(entry) = decode_link_result(&encoded) else {
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
                LinkResultInvalidation::between(entry.fingerprints.components, expected.components);
            let candidate = (reasons.count(), entry.fingerprints.fingerprint, reasons);
            if nearest
                .as_ref()
                .is_none_or(|current| (candidate.0, candidate.1) < (current.0, current.1))
            {
                nearest = Some(candidate);
            }
        }
        if let Some((_, _, reasons)) = nearest {
            Ok(LinkResultCacheEntryLookup::Invalidated(reasons))
        } else if corrupt {
            Ok(LinkResultCacheEntryLookup::Corrupt)
        } else {
            Ok(LinkResultCacheEntryLookup::NotFound)
        }
    }

    fn key_dir(&self, cache_key: LinkResultCacheKey) -> PathBuf {
        let [first, second] = cache_key.parts();
        self.root
            .join("artifacts")
            .join("links")
            .join(LINK_RESULT_SCHEMA)
            .join(format!("{first:016x}{second:016x}"))
    }

    fn path(&self, cache_key: LinkResultCacheKey, fingerprint: LinkResultFingerprint) -> PathBuf {
        let [first, second] = fingerprint.parts();
        self.key_dir(cache_key)
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

fn encode_link_result(fingerprints: LinkResultFingerprintSet, bytes: &[u8]) -> Vec<u8> {
    let checksum = payload_checksum(bytes);
    let mut encoded = Vec::with_capacity(128 + bytes.len());
    encoded.extend_from_slice(LINK_RESULT_MAGIC);
    for part in fingerprints.cache_key.parts() {
        encoded.extend_from_slice(&part.to_le_bytes());
    }
    write_fingerprint(&mut encoded, fingerprints.fingerprint);
    for component in [
        fingerprints.components.inputs,
        fingerprints.components.target,
        fingerprints.components.linker,
        fingerprints.components.options,
    ] {
        write_fingerprint(&mut encoded, component);
    }
    encoded.extend_from_slice(&(bytes.len() as u64).to_le_bytes());
    write_fingerprint(&mut encoded, checksum);
    encoded.extend_from_slice(bytes);
    encoded
}

struct DecodedLinkResult {
    fingerprints: LinkResultFingerprintSet,
    payload: Vec<u8>,
}

fn decode_link_result(encoded: &[u8]) -> Option<DecodedLinkResult> {
    let mut cursor = Cursor::new(encoded);
    let mut magic = [0; 8];
    cursor.read_exact(&mut magic).ok()?;
    (magic == *LINK_RESULT_MAGIC).then_some(())?;
    let cache_key =
        LinkResultCacheKey::from_parts([read_u64(&mut cursor)?, read_u64(&mut cursor)?]);
    let fingerprint = read_fingerprint(&mut cursor)?;
    let components = LinkResultFingerprintComponents {
        inputs: read_fingerprint(&mut cursor)?,
        target: read_fingerprint(&mut cursor)?,
        linker: read_fingerprint(&mut cursor)?,
        options: read_fingerprint(&mut cursor)?,
    };
    let fingerprints = LinkResultFingerprintSet::new(cache_key, components);
    (fingerprints.fingerprint == fingerprint).then_some(())?;
    let payload_len = usize::try_from(read_u64(&mut cursor)?).ok()?;
    let checksum = read_fingerprint(&mut cursor)?;
    let position = usize::try_from(cursor.position()).ok()?;
    (encoded.len().checked_sub(position)? == payload_len).then_some(())?;
    let payload = encoded[position..].to_vec();
    (payload_checksum(&payload) == checksum).then_some(DecodedLinkResult {
        fingerprints,
        payload,
    })
}

fn write_fingerprint(encoded: &mut Vec<u8>, fingerprint: LinkResultFingerprint) {
    for part in fingerprint.parts() {
        encoded.extend_from_slice(&part.to_le_bytes());
    }
}

fn read_fingerprint(cursor: &mut Cursor<&[u8]>) -> Option<LinkResultFingerprint> {
    Some(LinkResultFingerprint::from_parts([
        read_u64(cursor)?,
        read_u64(cursor)?,
    ]))
}

fn read_u64(cursor: &mut Cursor<&[u8]>) -> Option<u64> {
    let mut bytes = [0; 8];
    cursor.read_exact(&mut bytes).ok()?;
    Some(u64::from_le_bytes(bytes))
}

fn payload_checksum(bytes: &[u8]) -> LinkResultFingerprint {
    let mut builder = QueryFingerprintBuilder::new("nia.link-result-payload.v2");
    builder.write_bytes(bytes);
    LinkResultFingerprint::from_parts(builder.finish().parts())
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

    fn fingerprints(seed: u64) -> LinkResultFingerprintSet {
        LinkResultFingerprintSet::new(
            LinkResultCacheKey::from_parts([1, 2]),
            LinkResultFingerprintComponents {
                inputs: LinkResultFingerprint::from_parts([seed, 1]),
                target: LinkResultFingerprint::from_parts([seed, 2]),
                linker: LinkResultFingerprint::from_parts([seed, 3]),
                options: LinkResultFingerprint::from_parts([seed, 4]),
            },
        )
    }

    #[test]
    fn persistent_link_result_round_trips() {
        let root = temp_root("round_trip");
        let cache = PersistentLinkResultCache::new(root.clone());
        let fingerprints = fingerprints(10);
        let linked = root.join("linked");
        let restored = root.join("restored");
        fs::create_dir_all(&root).expect("create cache root");
        fs::write(&linked, b"linked executable").expect("write linked executable");
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
            .expect("publish link result");
        assert_eq!(
            cache
                .restore(fingerprints, &restored)
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
        let fingerprints = fingerprints(30);
        let linked = root.join("linked");
        let restored = root.join("restored");
        fs::create_dir_all(&root).expect("create cache root");
        fs::write(&linked, b"first").expect("write first executable");
        cache.publish(fingerprints, &linked).expect("publish first");
        fs::write(
            cache.path(fingerprints.cache_key, fingerprints.fingerprint),
            b"corrupt",
        )
        .expect("corrupt cache");

        assert_eq!(
            cache
                .restore(fingerprints, &restored)
                .expect("corrupt miss"),
            LinkResultCacheLookup::Corrupt
        );
        assert!(
            !cache
                .path(fingerprints.cache_key, fingerprints.fingerprint)
                .exists()
        );
        fs::write(&linked, b"second").expect("write second executable");
        cache
            .publish(fingerprints, &linked)
            .expect("republish link result");
        assert_eq!(
            cache
                .restore(fingerprints, &restored)
                .expect("restore second"),
            LinkResultCacheLookup::Hit
        );
        assert_eq!(fs::read(restored).expect("read second"), b"second");
        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn changed_components_report_nearest_invalidation_without_replacing_prior_entries() {
        let root = temp_root("invalidation");
        let cache = PersistentLinkResultCache::new(root.clone());
        let first = fingerprints(50);
        let linked = root.join("linked");
        fs::create_dir_all(&root).expect("create cache root");
        fs::write(&linked, b"first").expect("write executable");
        cache.publish(first, &linked).expect("publish first");
        let second = LinkResultFingerprintSet::new(
            first.cache_key,
            LinkResultFingerprintComponents {
                inputs: LinkResultFingerprint::from_parts([99, 1]),
                options: LinkResultFingerprint::from_parts([99, 4]),
                ..first.components
            },
        );
        fs::write(&linked, b"second").expect("write second executable");
        cache.publish(second, &linked).expect("publish second");
        let changed = LinkResultFingerprintSet::new(
            first.cache_key,
            LinkResultFingerprintComponents {
                target: LinkResultFingerprint::from_parts([99, 2]),
                ..second.components
            },
        );

        assert_eq!(
            cache
                .restore(changed, &root.join("changed"))
                .expect("lookup changed"),
            LinkResultCacheLookup::Invalidated(LinkResultInvalidation {
                inputs: false,
                target: true,
                linker: false,
                options: false,
            })
        );
        assert!(cache.path(first.cache_key, first.fingerprint).is_file());
        assert!(cache.path(second.cache_key, second.fingerprint).is_file());
        let _ = fs::remove_dir_all(root);
    }
}
