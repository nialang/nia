// SPDX-License-Identifier: GPL-3.0-or-later
use std::{
    fs::{self, OpenOptions},
    io::{self, Cursor, Read, Write},
    path::{Path, PathBuf},
    sync::atomic::{AtomicU64, Ordering},
};

use nia_codegen_llvm::{
    CodegenUnitFingerprint, CodegenUnitKey, ObjectWorkProductCache, ObjectWorkProductLookup,
};
use nia_query::QueryFingerprintBuilder;

const OBJECT_WORK_PRODUCT_MAGIC: &[u8; 8] = b"NIAOBJ01";
const OBJECT_WORK_PRODUCT_SCHEMA: &str = "v1";
static OBJECT_CACHE_STAGE_ID: AtomicU64 = AtomicU64::new(0);

#[derive(Debug)]
pub(crate) struct PersistentObjectWorkProductCache {
    root: PathBuf,
}

impl PersistentObjectWorkProductCache {
    pub(crate) fn new(root: PathBuf) -> Self {
        Self { root }
    }

    fn path(&self, fingerprint: CodegenUnitFingerprint) -> PathBuf {
        let [first, second] = fingerprint.parts();
        self.root
            .join("artifacts")
            .join("objects")
            .join(OBJECT_WORK_PRODUCT_SCHEMA)
            .join(format!("{first:016x}{second:016x}.o"))
    }

    fn remove_corrupt(path: &Path) {
        match fs::remove_file(path) {
            Ok(()) => {}
            Err(error) if error.kind() == io::ErrorKind::NotFound => {}
            Err(_) => {}
        }
    }
}

impl ObjectWorkProductCache for PersistentObjectWorkProductCache {
    fn load(
        &self,
        key: &CodegenUnitKey,
        fingerprint: CodegenUnitFingerprint,
    ) -> io::Result<ObjectWorkProductLookup> {
        let path = self.path(fingerprint);
        let encoded = match fs::read(&path) {
            Ok(encoded) => encoded,
            Err(error) if error.kind() == io::ErrorKind::NotFound => {
                return Ok(ObjectWorkProductLookup::NotFound);
            }
            Err(error) => return Err(error),
        };
        let Some(bytes) = decode_work_product(&encoded, key, fingerprint) else {
            Self::remove_corrupt(&path);
            return Ok(ObjectWorkProductLookup::Corrupt);
        };
        Ok(ObjectWorkProductLookup::Hit(bytes))
    }

    fn publish(
        &self,
        key: &CodegenUnitKey,
        fingerprint: CodegenUnitFingerprint,
        bytes: &[u8],
    ) -> io::Result<()> {
        if matches!(
            self.load(key, fingerprint)?,
            ObjectWorkProductLookup::Hit(_)
        ) {
            return Ok(());
        }
        let path = self.path(fingerprint);
        let parent = path
            .parent()
            .ok_or_else(|| io::Error::other("invalid object work-product path"))?;
        fs::create_dir_all(parent)?;
        let staged = path.with_extension(format!(
            "tmp.{}.{}",
            std::process::id(),
            OBJECT_CACHE_STAGE_ID.fetch_add(1, Ordering::Relaxed)
        ));
        let encoded = encode_work_product(key, fingerprint, bytes);
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
}

fn encode_work_product(
    key: &CodegenUnitKey,
    fingerprint: CodegenUnitFingerprint,
    bytes: &[u8],
) -> Vec<u8> {
    let key = encode_unit_key(key);
    let checksum = payload_checksum(bytes);
    let mut encoded = Vec::with_capacity(64 + key.len() + bytes.len());
    encoded.extend_from_slice(OBJECT_WORK_PRODUCT_MAGIC);
    for part in fingerprint.parts() {
        encoded.extend_from_slice(&part.to_le_bytes());
    }
    encoded.extend_from_slice(&(key.len() as u64).to_le_bytes());
    encoded.extend_from_slice(&key);
    encoded.extend_from_slice(&(bytes.len() as u64).to_le_bytes());
    for part in checksum.parts() {
        encoded.extend_from_slice(&part.to_le_bytes());
    }
    encoded.extend_from_slice(bytes);
    encoded
}

fn decode_work_product(
    encoded: &[u8],
    expected_key: &CodegenUnitKey,
    expected_fingerprint: CodegenUnitFingerprint,
) -> Option<Vec<u8>> {
    let mut cursor = Cursor::new(encoded);
    let mut magic = [0; 8];
    cursor.read_exact(&mut magic).ok()?;
    (magic == *OBJECT_WORK_PRODUCT_MAGIC).then_some(())?;
    let fingerprint =
        CodegenUnitFingerprint::from_parts([read_u64(&mut cursor)?, read_u64(&mut cursor)?]);
    (fingerprint == expected_fingerprint).then_some(())?;
    let key_len = usize::try_from(read_u64(&mut cursor)?).ok()?;
    let key_start = usize::try_from(cursor.position()).ok()?;
    (key_len <= encoded.len().checked_sub(key_start)?).then_some(())?;
    let mut key = vec![0; key_len];
    cursor.read_exact(&mut key).ok()?;
    (key == encode_unit_key(expected_key)).then_some(())?;
    let payload_len = usize::try_from(read_u64(&mut cursor)?).ok()?;
    let checksum =
        CodegenUnitFingerprint::from_parts([read_u64(&mut cursor)?, read_u64(&mut cursor)?]);
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

fn encode_unit_key(key: &CodegenUnitKey) -> Vec<u8> {
    let mut encoded = Vec::new();
    match key {
        CodegenUnitKey::SourceModule {
            source_identity,
            ordinal,
        } => {
            encoded.push(0);
            let path = source_identity.normalized_path().as_bytes();
            encoded.extend_from_slice(&(path.len() as u64).to_le_bytes());
            encoded.extend_from_slice(path);
            encoded.extend_from_slice(&ordinal.to_le_bytes());
        }
        CodegenUnitKey::CompilerBuiltins => encoded.push(1),
    }
    encoded
}

fn payload_checksum(bytes: &[u8]) -> CodegenUnitFingerprint {
    let mut builder = QueryFingerprintBuilder::new("nia.object-work-product-payload.v1");
    builder.write_bytes(bytes);
    CodegenUnitFingerprint::from_parts(builder.finish().parts())
}

#[cfg(test)]
mod tests {
    use std::sync::atomic::{AtomicU64, Ordering};

    use nia_source::SourceIdentity;

    use super::*;

    static TEMP_ID: AtomicU64 = AtomicU64::new(0);

    fn temp_root(name: &str) -> PathBuf {
        std::env::temp_dir().join(format!(
            "nia_object_cache_{name}_{}_{}",
            std::process::id(),
            TEMP_ID.fetch_add(1, Ordering::Relaxed)
        ))
    }

    fn key() -> CodegenUnitKey {
        CodegenUnitKey::SourceModule {
            source_identity: SourceIdentity::new("src/main.nia"),
            ordinal: 0,
        }
    }

    #[test]
    fn persistent_object_work_product_round_trips() {
        let root = temp_root("round_trip");
        let cache = PersistentObjectWorkProductCache::new(root.clone());
        let fingerprint = CodegenUnitFingerprint::from_parts([10, 20]);
        assert_eq!(
            cache.load(&key(), fingerprint).expect("cold load"),
            ObjectWorkProductLookup::NotFound
        );
        cache
            .publish(&key(), fingerprint, b"object bytes")
            .expect("publish");

        assert_eq!(
            cache.load(&key(), fingerprint).expect("load"),
            ObjectWorkProductLookup::Hit(b"object bytes".to_vec())
        );
        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn corrupt_work_product_is_deleted_and_can_be_republished() {
        let root = temp_root("corrupt");
        let cache = PersistentObjectWorkProductCache::new(root.clone());
        let fingerprint = CodegenUnitFingerprint::from_parts([30, 40]);
        cache
            .publish(&key(), fingerprint, b"first")
            .expect("publish");
        let path = cache.path(fingerprint);
        let mut encoded = fs::read(&path).expect("read cache");
        *encoded.last_mut().expect("payload byte") ^= 0xff;
        fs::write(&path, encoded).expect("corrupt cache");

        assert_eq!(
            cache.load(&key(), fingerprint).expect("load"),
            ObjectWorkProductLookup::Corrupt
        );
        assert!(!path.exists(), "corrupt cache entry must be retired");
        cache
            .publish(&key(), fingerprint, b"second")
            .expect("republish");
        assert_eq!(
            cache.load(&key(), fingerprint).expect("load"),
            ObjectWorkProductLookup::Hit(b"second".to_vec())
        );
        let _ = fs::remove_dir_all(root);
    }
}
