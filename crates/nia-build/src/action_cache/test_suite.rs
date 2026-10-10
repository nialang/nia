// SPDX-License-Identifier: GPL-3.0-or-later
//! Persistent completion records for successful test suites.
//!
//! A test suite has no publishable output. Its cache record therefore only
//! proves that the exact suite identity completed successfully; failed or
//! cancelled processes never publish a record.

use std::{
    fs,
    io::{self, Write},
    path::PathBuf,
    sync::atomic::Ordering,
};

use nia_compat::formats::{TEST_SUITE_CACHE, TEST_SUITE_ENTRY};
use nia_query::{FingerprintDomain, QueryFingerprint, QueryFingerprintBuilder};

use super::{
    ActionCacheMissReason, BoundedCacheEntry, CACHE_STAGE_SEQUENCE, fingerprint_text,
    read_bounded_cache_entry,
};
use crate::lock::ScopedFileLock;

const TEST_SUITE_KEY_DOMAIN: FingerprintDomain = FingerprintDomain::new("nia.build.test-suite-key");

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct TestSuiteCacheIdentity {
    pub(crate) fingerprint: QueryFingerprint,
}

impl TestSuiteCacheIdentity {
    pub(crate) fn from_bytes(bytes: &[u8]) -> Self {
        let mut builder = QueryFingerprintBuilder::new(TEST_SUITE_KEY_DOMAIN);
        builder.write_bytes(bytes);
        Self {
            fingerprint: builder.finish(),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum TestSuiteCacheLookup {
    Hit,
    Miss(ActionCacheMissReason),
}

#[derive(Debug)]
pub(crate) struct TestSuiteCache {
    root: PathBuf,
}

impl TestSuiteCache {
    pub(crate) fn new(root: PathBuf) -> Self {
        Self { root }
    }

    pub(crate) fn lookup(
        &self,
        identity: TestSuiteCacheIdentity,
    ) -> io::Result<TestSuiteCacheLookup> {
        let path = self.path(identity);
        let bytes = match read_bounded_cache_entry(&path, 28) {
            Ok(BoundedCacheEntry::Bytes(bytes)) => bytes,
            Ok(BoundedCacheEntry::Oversized) => {
                let _ = fs::remove_file(&path);
                return Ok(TestSuiteCacheLookup::Miss(ActionCacheMissReason::Corrupt));
            }
            Err(error) if error.kind() == io::ErrorKind::NotFound => {
                return Ok(TestSuiteCacheLookup::Miss(ActionCacheMissReason::NotFound));
            }
            Err(error) => return Err(error),
        };
        if bytes.len() != 8 + 4 + 16 || &bytes[..8] != TEST_SUITE_ENTRY.magic {
            let _ = fs::remove_file(&path);
            return Ok(TestSuiteCacheLookup::Miss(ActionCacheMissReason::Corrupt));
        }
        if bytes[8..12] != TEST_SUITE_ENTRY.protocol_epoch.to_le_bytes() {
            let _ = fs::remove_file(&path);
            return Ok(TestSuiteCacheLookup::Miss(ActionCacheMissReason::Corrupt));
        }
        let found = QueryFingerprint::from_parts([
            u64::from_le_bytes(bytes[12..20].try_into().unwrap()),
            u64::from_le_bytes(bytes[20..28].try_into().unwrap()),
        ]);
        if found != identity.fingerprint {
            let _ = fs::remove_file(&path);
            return Ok(TestSuiteCacheLookup::Miss(ActionCacheMissReason::Corrupt));
        }
        Ok(TestSuiteCacheLookup::Hit)
    }

    pub(crate) fn acquire_interruptible(
        &self,
        identity: TestSuiteCacheIdentity,
        is_cancelled: impl FnMut() -> bool,
    ) -> io::Result<Option<ScopedFileLock>> {
        let lock = self
            .root
            .join("coordination/test-suite-cache")
            .join(format!("{}.lock", fingerprint_text(identity.fingerprint)));
        ScopedFileLock::acquire_interruptible(lock, is_cancelled)
    }

    pub(crate) fn publish(&self, identity: TestSuiteCacheIdentity) -> io::Result<()> {
        let path = self.path(identity);
        let parent = path
            .parent()
            .ok_or_else(|| io::Error::other("invalid test-suite cache path"))?;
        fs::create_dir_all(parent)?;
        let staged = parent.join(format!(
            ".nia-test-suite-{}-{}.tmp",
            std::process::id(),
            CACHE_STAGE_SEQUENCE.fetch_add(1, Ordering::Relaxed)
        ));
        let mut bytes = Vec::with_capacity(28);
        bytes.extend_from_slice(TEST_SUITE_ENTRY.magic);
        bytes.extend_from_slice(&TEST_SUITE_ENTRY.protocol_epoch.to_le_bytes());
        for part in identity.fingerprint.parts() {
            bytes.extend_from_slice(&part.to_le_bytes());
        }
        let mut file = fs::OpenOptions::new()
            .create_new(true)
            .write(true)
            .open(&staged)?;
        let result = (|| {
            file.write_all(&bytes)?;
            file.sync_all()?;
            match fs::hard_link(&staged, &path) {
                Ok(()) => Ok(()),
                Err(error) if error.kind() == io::ErrorKind::AlreadyExists => Ok(()),
                Err(error) => Err(error),
            }
        })();
        drop(file);
        let _ = fs::remove_file(&staged);
        match result {
            Ok(()) => nia_compat::sync_directory(parent),
            Err(error) => Err(error),
        }
    }

    fn path(&self, identity: TestSuiteCacheIdentity) -> PathBuf {
        self.root
            .join("actions/test-suites")
            .join(TEST_SUITE_CACHE.path_component)
            .join(format!("{}.entry", fingerprint_text(identity.fingerprint)))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn root() -> PathBuf {
        std::env::temp_dir().join(format!(
            "nia-test-suite-cache-{}-{}",
            std::process::id(),
            CACHE_STAGE_SEQUENCE.fetch_add(1, Ordering::Relaxed)
        ))
    }

    #[test]
    fn successful_suite_record_is_reused_and_corruption_is_a_miss() {
        let root = root();
        let cache = TestSuiteCache::new(root.clone());
        let identity = TestSuiteCacheIdentity::from_bytes(b"suite identity");
        assert_eq!(
            cache.lookup(identity).unwrap(),
            TestSuiteCacheLookup::Miss(ActionCacheMissReason::NotFound)
        );
        cache.publish(identity).unwrap();
        assert_eq!(cache.lookup(identity).unwrap(), TestSuiteCacheLookup::Hit);
        fs::write(cache.path(identity), b"bad").unwrap();
        assert_eq!(
            cache.lookup(identity).unwrap(),
            TestSuiteCacheLookup::Miss(ActionCacheMissReason::Corrupt)
        );
        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn suite_identity_changes_with_any_input_bytes() {
        assert_ne!(
            TestSuiteCacheIdentity::from_bytes(b"first"),
            TestSuiteCacheIdentity::from_bytes(b"second")
        );
    }
}
