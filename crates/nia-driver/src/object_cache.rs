// SPDX-License-Identifier: GPL-3.0-or-later
use std::{
    fs::{self, OpenOptions},
    io::{self, Cursor, Read, Write},
    path::{Path, PathBuf},
    sync::atomic::{AtomicU64, Ordering},
};

use nia_codegen_llvm::{
    CodegenUnitFingerprint, CodegenUnitFingerprintComponents, CodegenUnitFingerprintSet,
    CodegenUnitKey, ObjectWorkProductCache, ObjectWorkProductInvalidation, ObjectWorkProductLookup,
};
use nia_compat::formats::{OBJECT_WORK_PRODUCT, OBJECT_WORK_PRODUCT_CACHE};
use nia_query::{FingerprintDomain, QueryFingerprintBuilder};

const OBJECT_WORK_PRODUCT_KEY_DOMAIN: FingerprintDomain =
    FingerprintDomain::new("nia.object-work-product-key.v2");
const OBJECT_WORK_PRODUCT_PAYLOAD_DOMAIN: FingerprintDomain =
    FingerprintDomain::new("nia.object-work-product-payload.v1");
static OBJECT_CACHE_STAGE_ID: AtomicU64 = AtomicU64::new(0);

#[derive(Debug)]
pub(crate) struct PersistentObjectWorkProductCache {
    root: PathBuf,
}

impl PersistentObjectWorkProductCache {
    pub(crate) fn new(root: PathBuf) -> Self {
        Self { root }
    }

    fn key_dir(&self, key: &CodegenUnitKey) -> PathBuf {
        let mut builder = QueryFingerprintBuilder::new(OBJECT_WORK_PRODUCT_KEY_DOMAIN);
        builder.write_bytes(&encode_unit_key(key));
        let [first, second] = builder.finish().parts();
        self.root
            .join("artifacts")
            .join("objects")
            .join(OBJECT_WORK_PRODUCT_CACHE.path_component)
            .join(format!("{first:016x}{second:016x}"))
    }

    fn path(&self, key: &CodegenUnitKey, fingerprint: CodegenUnitFingerprint) -> PathBuf {
        let [first, second] = fingerprint.parts();
        self.key_dir(key)
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
        fingerprints: CodegenUnitFingerprintSet,
    ) -> io::Result<ObjectWorkProductLookup> {
        let path = self.path(key, fingerprints.fingerprint);
        let encoded = match fs::read(&path) {
            Ok(encoded) => encoded,
            Err(error) if error.kind() == io::ErrorKind::NotFound => {
                return self.lookup_invalidation(key, fingerprints);
            }
            Err(error) => return Err(error),
        };
        let Some(entry) = decode_work_product(&encoded) else {
            Self::remove_corrupt(&path);
            return Ok(ObjectWorkProductLookup::Corrupt);
        };
        if entry.key != encode_unit_key(key)
            || entry.fingerprints != fingerprints
            || path != self.path(key, entry.fingerprints.fingerprint)
        {
            Self::remove_corrupt(&path);
            return Ok(ObjectWorkProductLookup::Corrupt);
        }
        Ok(ObjectWorkProductLookup::Hit(entry.payload))
    }

    fn publish(
        &self,
        key: &CodegenUnitKey,
        fingerprints: CodegenUnitFingerprintSet,
        bytes: &[u8],
    ) -> io::Result<()> {
        if matches!(
            self.load(key, fingerprints)?,
            ObjectWorkProductLookup::Hit(_)
        ) {
            return Ok(());
        }
        let path = self.path(key, fingerprints.fingerprint);
        let parent = path
            .parent()
            .ok_or_else(|| io::Error::other("invalid object work-product path"))?;
        fs::create_dir_all(parent)?;
        let staged = path.with_extension(format!(
            "tmp.{}.{}",
            std::process::id(),
            OBJECT_CACHE_STAGE_ID.fetch_add(1, Ordering::Relaxed)
        ));
        let encoded = encode_work_product(key, fingerprints, bytes);
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

impl PersistentObjectWorkProductCache {
    fn lookup_invalidation(
        &self,
        key: &CodegenUnitKey,
        expected: CodegenUnitFingerprintSet,
    ) -> io::Result<ObjectWorkProductLookup> {
        let directory = self.key_dir(key);
        let entries = match fs::read_dir(&directory) {
            Ok(entries) => entries,
            Err(error) if error.kind() == io::ErrorKind::NotFound => {
                return Ok(ObjectWorkProductLookup::NotFound);
            }
            Err(error) => return Err(error),
        };
        let expected_key = encode_unit_key(key);
        let mut nearest = None::<(u32, CodegenUnitFingerprint, ObjectWorkProductInvalidation)>;
        let mut corrupt = false;
        for entry in entries {
            let path = entry?.path();
            if path.extension().and_then(|value| value.to_str()) != Some("o") {
                continue;
            }
            let encoded = fs::read(&path)?;
            let Some(entry) = decode_work_product(&encoded) else {
                Self::remove_corrupt(&path);
                corrupt = true;
                continue;
            };
            if entry.key != expected_key || path != self.path(key, entry.fingerprints.fingerprint) {
                Self::remove_corrupt(&path);
                corrupt = true;
                continue;
            }
            let reasons = ObjectWorkProductInvalidation::between(
                entry.fingerprints.components,
                expected.components,
            );
            let candidate = (reasons.count(), entry.fingerprints.fingerprint, reasons);
            if nearest
                .as_ref()
                .is_none_or(|current| (candidate.0, candidate.1) < (current.0, current.1))
            {
                nearest = Some(candidate);
            }
        }
        if let Some((_, _, reasons)) = nearest {
            Ok(ObjectWorkProductLookup::Invalidated(reasons))
        } else if corrupt {
            Ok(ObjectWorkProductLookup::Corrupt)
        } else {
            Ok(ObjectWorkProductLookup::NotFound)
        }
    }
}

fn encode_work_product(
    key: &CodegenUnitKey,
    fingerprints: CodegenUnitFingerprintSet,
    bytes: &[u8],
) -> Vec<u8> {
    let key = encode_unit_key(key);
    let checksum = payload_checksum(bytes);
    let mut encoded = Vec::with_capacity(128 + key.len() + bytes.len());
    encoded.extend_from_slice(OBJECT_WORK_PRODUCT.magic);
    write_fingerprint(&mut encoded, fingerprints.fingerprint);
    for component in [
        fingerprints.components.policy,
        fingerprints.components.definition,
        fingerprints.components.declarations,
        fingerprints.components.target,
    ] {
        write_fingerprint(&mut encoded, component);
    }
    encoded.extend_from_slice(&(key.len() as u64).to_le_bytes());
    encoded.extend_from_slice(&key);
    encoded.extend_from_slice(&(bytes.len() as u64).to_le_bytes());
    write_fingerprint(&mut encoded, checksum);
    encoded.extend_from_slice(bytes);
    encoded
}

struct DecodedWorkProduct {
    key: Vec<u8>,
    fingerprints: CodegenUnitFingerprintSet,
    payload: Vec<u8>,
}

fn decode_work_product(encoded: &[u8]) -> Option<DecodedWorkProduct> {
    let mut cursor = Cursor::new(encoded);
    let mut magic = [0; 8];
    cursor.read_exact(&mut magic).ok()?;
    (magic == *OBJECT_WORK_PRODUCT.magic).then_some(())?;
    let fingerprint = read_fingerprint(&mut cursor)?;
    let components = CodegenUnitFingerprintComponents {
        policy: read_fingerprint(&mut cursor)?,
        definition: read_fingerprint(&mut cursor)?,
        declarations: read_fingerprint(&mut cursor)?,
        target: read_fingerprint(&mut cursor)?,
    };
    let fingerprints = CodegenUnitFingerprintSet::new(components);
    (fingerprints.fingerprint == fingerprint).then_some(())?;
    let key_len = usize::try_from(read_u64(&mut cursor)?).ok()?;
    let key_start = usize::try_from(cursor.position()).ok()?;
    (key_len <= encoded.len().checked_sub(key_start)?).then_some(())?;
    let mut key = vec![0; key_len];
    cursor.read_exact(&mut key).ok()?;
    let payload_len = usize::try_from(read_u64(&mut cursor)?).ok()?;
    let checksum = read_fingerprint(&mut cursor)?;
    let position = usize::try_from(cursor.position()).ok()?;
    (encoded.len().checked_sub(position)? == payload_len).then_some(())?;
    let payload = encoded[position..].to_vec();
    (payload_checksum(&payload) == checksum).then_some(DecodedWorkProduct {
        key,
        fingerprints,
        payload,
    })
}

fn write_fingerprint(encoded: &mut Vec<u8>, fingerprint: CodegenUnitFingerprint) {
    for part in fingerprint.parts() {
        encoded.extend_from_slice(&part.to_le_bytes());
    }
}

fn read_fingerprint(cursor: &mut Cursor<&[u8]>) -> Option<CodegenUnitFingerprint> {
    Some(CodegenUnitFingerprint::from_parts([
        read_u64(cursor)?,
        read_u64(cursor)?,
    ]))
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
    let mut builder = QueryFingerprintBuilder::new(OBJECT_WORK_PRODUCT_PAYLOAD_DOMAIN);
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

    fn fingerprints(seed: u64) -> CodegenUnitFingerprintSet {
        CodegenUnitFingerprintSet::new(CodegenUnitFingerprintComponents {
            policy: CodegenUnitFingerprint::from_parts([seed, 1]),
            definition: CodegenUnitFingerprint::from_parts([seed, 2]),
            declarations: CodegenUnitFingerprint::from_parts([seed, 3]),
            target: CodegenUnitFingerprint::from_parts([seed, 4]),
        })
    }

    #[test]
    fn persistent_object_work_product_round_trips() {
        let root = temp_root("round_trip");
        let cache = PersistentObjectWorkProductCache::new(root.clone());
        let fingerprints = fingerprints(10);
        let legacy = root.join("artifacts/objects/v1/legacy.o");
        fs::create_dir_all(legacy.parent().expect("legacy parent"))
            .expect("create legacy namespace");
        fs::write(&legacy, b"legacy entry").expect("write legacy entry");
        assert_eq!(
            cache.load(&key(), fingerprints).expect("cold load"),
            ObjectWorkProductLookup::NotFound
        );
        cache
            .publish(&key(), fingerprints, b"object bytes")
            .expect("publish");

        assert_eq!(
            cache.load(&key(), fingerprints).expect("load"),
            ObjectWorkProductLookup::Hit(b"object bytes".to_vec())
        );
        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn corrupt_work_product_is_deleted_and_can_be_republished() {
        let root = temp_root("corrupt");
        let cache = PersistentObjectWorkProductCache::new(root.clone());
        let fingerprints = fingerprints(30);
        cache
            .publish(&key(), fingerprints, b"first")
            .expect("publish");
        let path = cache.path(&key(), fingerprints.fingerprint);
        let mut encoded = fs::read(&path).expect("read cache");
        *encoded.last_mut().expect("payload byte") ^= 0xff;
        fs::write(&path, encoded).expect("corrupt cache");

        assert_eq!(
            cache.load(&key(), fingerprints).expect("load"),
            ObjectWorkProductLookup::Corrupt
        );
        assert!(!path.exists(), "corrupt cache entry must be retired");
        cache
            .publish(&key(), fingerprints, b"second")
            .expect("republish");
        assert_eq!(
            cache.load(&key(), fingerprints).expect("load"),
            ObjectWorkProductLookup::Hit(b"second".to_vec())
        );
        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn changed_components_report_invalidation_without_replacing_prior_entry() {
        let root = temp_root("invalidation");
        let cache = PersistentObjectWorkProductCache::new(root.clone());
        let first = fingerprints(50);
        cache
            .publish(&key(), first, b"first")
            .expect("publish first");
        let changed = CodegenUnitFingerprintSet::new(CodegenUnitFingerprintComponents {
            definition: CodegenUnitFingerprint::from_parts([99, 2]),
            ..first.components
        });

        assert_eq!(
            cache.load(&key(), changed).expect("load changed"),
            ObjectWorkProductLookup::Invalidated(ObjectWorkProductInvalidation {
                policy: false,
                definition: true,
                declarations: false,
                target: false,
            })
        );
        cache
            .publish(&key(), changed, b"changed")
            .expect("publish changed");
        assert!(cache.path(&key(), first.fingerprint).is_file());
        assert!(cache.path(&key(), changed.fingerprint).is_file());
        let _ = fs::remove_dir_all(root);
    }
}
