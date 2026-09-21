// SPDX-License-Identifier: GPL-3.0-or-later
use std::{
    fs::{self, File, OpenOptions},
    io::{self, Read, Seek, SeekFrom, Write},
    path::{Path, PathBuf},
    sync::atomic::{AtomicU64, Ordering},
};

use nia_codegen_llvm::{
    CodegenUnitFingerprint, CodegenUnitFingerprintComponents, CodegenUnitFingerprintSet,
    CodegenUnitKey, CodegenWorkProductInvalidation, CodegenWorkProductLookup, LtoMode,
    LtoModuleWorkProductCache, ObjectWorkProductCache,
};
use nia_compat::{
    PersistedFormat, PersistedNamespace,
    formats::{
        FULL_LTO_MODULE, LTO_MODULE_CACHE, OBJECT_WORK_PRODUCT, OBJECT_WORK_PRODUCT_CACHE,
        THIN_LTO_MODULE,
    },
};
use nia_query::{FingerprintDomain, QueryFingerprintBuilder};

const WORK_PRODUCT_STREAM_BYTES: usize = 64 * 1024;
static WORK_PRODUCT_STAGE_ID: AtomicU64 = AtomicU64::new(0);
const OBJECT_WORK_PRODUCT_KEY_DOMAIN: FingerprintDomain =
    FingerprintDomain::new("nia.object-work-product-key");
const OBJECT_WORK_PRODUCT_PAYLOAD_DOMAIN: FingerprintDomain =
    FingerprintDomain::new("nia.object-work-product-payload");
const THIN_LTO_MODULE_KEY_DOMAIN: FingerprintDomain =
    FingerprintDomain::new("nia.thin-lto-module-key");
const THIN_LTO_MODULE_PAYLOAD_DOMAIN: FingerprintDomain =
    FingerprintDomain::new("nia.thin-lto-module-payload");
const FULL_LTO_MODULE_KEY_DOMAIN: FingerprintDomain =
    FingerprintDomain::new("nia.full-lto-module-key");
const FULL_LTO_MODULE_PAYLOAD_DOMAIN: FingerprintDomain =
    FingerprintDomain::new("nia.full-lto-module-payload");

#[derive(Debug, Clone, Copy)]
struct WorkProductKind {
    directory: &'static str,
    extension: &'static str,
    format: PersistedFormat,
    namespace: PersistedNamespace,
    key_domain: FingerprintDomain,
    payload_domain: FingerprintDomain,
}

const OBJECT_WORK_PRODUCT_KIND: WorkProductKind = WorkProductKind {
    directory: "objects",
    extension: "o",
    format: OBJECT_WORK_PRODUCT,
    namespace: OBJECT_WORK_PRODUCT_CACHE,
    key_domain: OBJECT_WORK_PRODUCT_KEY_DOMAIN,
    payload_domain: OBJECT_WORK_PRODUCT_PAYLOAD_DOMAIN,
};
const THIN_LTO_MODULE_KIND: WorkProductKind = WorkProductKind {
    directory: "thin-lto-modules",
    extension: "bc",
    format: THIN_LTO_MODULE,
    namespace: LTO_MODULE_CACHE,
    key_domain: THIN_LTO_MODULE_KEY_DOMAIN,
    payload_domain: THIN_LTO_MODULE_PAYLOAD_DOMAIN,
};
const FULL_LTO_MODULE_KIND: WorkProductKind = WorkProductKind {
    directory: "full-lto-modules",
    extension: "bc",
    format: FULL_LTO_MODULE,
    namespace: LTO_MODULE_CACHE,
    key_domain: FULL_LTO_MODULE_KEY_DOMAIN,
    payload_domain: FULL_LTO_MODULE_PAYLOAD_DOMAIN,
};

#[derive(Debug)]
pub(crate) struct PersistentObjectWorkProductCache {
    inner: PersistentWorkProductCache,
}

#[derive(Debug)]
pub(crate) struct PersistentLtoModuleWorkProductCache {
    thin: PersistentWorkProductCache,
    full: PersistentWorkProductCache,
}

#[derive(Debug)]
struct PersistentWorkProductCache {
    root: PathBuf,
    kind: WorkProductKind,
}

impl PersistentObjectWorkProductCache {
    pub(crate) fn new(root: PathBuf) -> Self {
        Self {
            inner: PersistentWorkProductCache::new(root, OBJECT_WORK_PRODUCT_KIND),
        }
    }

    #[cfg(test)]
    fn path(&self, key: &CodegenUnitKey, fingerprint: CodegenUnitFingerprint) -> PathBuf {
        self.inner.path(key, fingerprint)
    }
}

impl PersistentLtoModuleWorkProductCache {
    pub(crate) fn new(root: PathBuf) -> Self {
        Self {
            thin: PersistentWorkProductCache::new(root.clone(), THIN_LTO_MODULE_KIND),
            full: PersistentWorkProductCache::new(root, FULL_LTO_MODULE_KIND),
        }
    }

    fn cache(&self, mode: LtoMode) -> &PersistentWorkProductCache {
        match mode {
            LtoMode::Thin => &self.thin,
            LtoMode::Full => &self.full,
        }
    }
}

impl PersistentWorkProductCache {
    fn new(root: PathBuf, kind: WorkProductKind) -> Self {
        Self { root, kind }
    }

    fn key_dir(&self, key: &CodegenUnitKey) -> PathBuf {
        let mut builder = QueryFingerprintBuilder::new(self.kind.key_domain);
        builder.write_bytes(&encode_unit_key(key));
        let [first, second] = builder.finish().parts();
        self.root
            .join("artifacts")
            .join(self.kind.directory)
            .join(self.kind.namespace.path_component)
            .join(format!("{first:016x}{second:016x}"))
    }

    fn path(&self, key: &CodegenUnitKey, fingerprint: CodegenUnitFingerprint) -> PathBuf {
        let [first, second] = fingerprint.parts();
        self.key_dir(key)
            .join(format!("{first:016x}{second:016x}.{}", self.kind.extension))
    }

    fn lookup_invalidation(
        &self,
        key: &CodegenUnitKey,
        expected: CodegenUnitFingerprintSet,
    ) -> io::Result<CodegenWorkProductLookup> {
        let directory = self.key_dir(key);
        let entries = match fs::read_dir(&directory) {
            Ok(entries) => entries,
            Err(error) if error.kind() == io::ErrorKind::NotFound => {
                return Ok(CodegenWorkProductLookup::NotFound);
            }
            Err(error) => return Err(error),
        };
        let expected_key = encode_unit_key(key);
        let mut nearest = None::<(u32, CodegenUnitFingerprint, CodegenWorkProductInvalidation)>;
        let mut corrupt = false;
        for entry in entries {
            let path = entry?.path();
            if path.extension().and_then(|value| value.to_str()) != Some(self.kind.extension) {
                continue;
            }
            let mut file = match File::open(&path) {
                Ok(file) => file,
                Err(error) if error.kind() == io::ErrorKind::NotFound => continue,
                Err(error) => return Err(error),
            };
            let Some(header) = read_work_product_header(&mut file, &expected_key, self.kind)?
            else {
                retire_corrupt(&path, &mut file);
                corrupt = true;
                continue;
            };
            if path != self.path(key, header.fingerprints.fingerprint)
                || !validate_payload(&mut file, header.payload_len, header.checksum, self.kind)?
            {
                retire_corrupt(&path, &mut file);
                corrupt = true;
                continue;
            }
            let reasons = CodegenWorkProductInvalidation::between(
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
            Ok(CodegenWorkProductLookup::Invalidated(reasons))
        } else if corrupt {
            Ok(CodegenWorkProductLookup::Corrupt)
        } else {
            Ok(CodegenWorkProductLookup::NotFound)
        }
    }

    fn load(
        &self,
        key: &CodegenUnitKey,
        fingerprints: CodegenUnitFingerprintSet,
    ) -> io::Result<CodegenWorkProductLookup> {
        let path = self.path(key, fingerprints.fingerprint);
        let mut file = match File::open(&path) {
            Ok(file) => file,
            Err(error) if error.kind() == io::ErrorKind::NotFound => {
                return self.lookup_invalidation(key, fingerprints);
            }
            Err(error) => return Err(error),
        };
        let expected_key = encode_unit_key(key);
        let Some(header) = read_work_product_header(&mut file, &expected_key, self.kind)? else {
            retire_corrupt(&path, &mut file);
            return Ok(CodegenWorkProductLookup::Corrupt);
        };
        if header.fingerprints != fingerprints {
            retire_corrupt(&path, &mut file);
            return Ok(CodegenWorkProductLookup::Corrupt);
        }
        let Some(payload) =
            read_payload(&mut file, header.payload_len, header.checksum, self.kind)?
        else {
            retire_corrupt(&path, &mut file);
            return Ok(CodegenWorkProductLookup::Corrupt);
        };
        Ok(CodegenWorkProductLookup::Hit(payload))
    }

    fn publish(
        &self,
        key: &CodegenUnitKey,
        fingerprints: CodegenUnitFingerprintSet,
        bytes: &[u8],
    ) -> io::Result<()> {
        let path = self.path(key, fingerprints.fingerprint);
        let parent = path
            .parent()
            .ok_or_else(|| io::Error::other("invalid codegen work-product path"))?;
        fs::create_dir_all(parent)?;
        let staged = staged_path(&path);
        let result = (|| {
            let mut file = OpenOptions::new()
                .write(true)
                .create_new(true)
                .open(&staged)?;
            write_work_product(&mut file, key, fingerprints, bytes, self.kind)?;
            // Codegen work products are validated, disposable cache entries.
            // Closing before rename preserves atomic visibility; a crash may
            // only turn an entry into a cache miss or corrupt-entry recovery.
            drop(file);

            let _lock = WorkProductMutationLock::acquire(&path)?;
            match compare_installed(&path, key, fingerprints, bytes, self.kind)? {
                InstalledEntry::NotFound => {}
                InstalledEntry::Identical => return Ok(()),
                InstalledEntry::Corrupt => {
                    fs::remove_file(&path)?;
                }
                InstalledEntry::Collision => {
                    return Err(io::Error::new(
                        io::ErrorKind::AlreadyExists,
                        "codegen work-product fingerprint collision",
                    ));
                }
            }
            fs::rename(&staged, &path)?;
            Ok(())
        })();
        if result.is_err() || staged.exists() {
            let _ = fs::remove_file(&staged);
        }
        result
    }
}

impl ObjectWorkProductCache for PersistentObjectWorkProductCache {
    fn load(
        &self,
        key: &CodegenUnitKey,
        fingerprints: CodegenUnitFingerprintSet,
    ) -> io::Result<CodegenWorkProductLookup> {
        self.inner.load(key, fingerprints)
    }

    fn publish(
        &self,
        key: &CodegenUnitKey,
        fingerprints: CodegenUnitFingerprintSet,
        bytes: &[u8],
    ) -> io::Result<()> {
        self.inner.publish(key, fingerprints, bytes)
    }
}

impl LtoModuleWorkProductCache for PersistentLtoModuleWorkProductCache {
    fn load(
        &self,
        mode: LtoMode,
        key: &CodegenUnitKey,
        fingerprints: CodegenUnitFingerprintSet,
    ) -> io::Result<CodegenWorkProductLookup> {
        self.cache(mode).load(key, fingerprints)
    }

    fn publish(
        &self,
        mode: LtoMode,
        key: &CodegenUnitKey,
        fingerprints: CodegenUnitFingerprintSet,
        bytes: &[u8],
    ) -> io::Result<()> {
        self.cache(mode).publish(key, fingerprints, bytes)
    }
}

#[derive(Clone, Copy)]
struct WorkProductHeader {
    fingerprints: CodegenUnitFingerprintSet,
    payload_len: u64,
    checksum: CodegenUnitFingerprint,
}

enum InstalledEntry {
    NotFound,
    Identical,
    Corrupt,
    Collision,
}

/// Parses only the bounded record header. The persisted key length must equal
/// the already-known canonical key length, so corrupt metadata cannot request
/// an attacker-sized allocation before payload validation begins.
fn read_work_product_header(
    file: &mut File,
    expected_key: &[u8],
    kind: WorkProductKind,
) -> io::Result<Option<WorkProductHeader>> {
    if !file.metadata()?.is_file() {
        return Ok(None);
    }
    file.seek(SeekFrom::Start(0))?;
    let Some(magic) = read_array::<8>(file)? else {
        return Ok(None);
    };
    if magic != *kind.format.magic {
        return Ok(None);
    }
    let Some(release_compatibility) = read_u32(file)? else {
        return Ok(None);
    };
    if release_compatibility != kind.format.release_compatibility {
        return Ok(None);
    }
    let Some(fingerprint) = read_fingerprint(file)? else {
        return Ok(None);
    };
    let Some(policy) = read_fingerprint(file)? else {
        return Ok(None);
    };
    let Some(definition) = read_fingerprint(file)? else {
        return Ok(None);
    };
    let Some(declarations) = read_fingerprint(file)? else {
        return Ok(None);
    };
    let Some(target) = read_fingerprint(file)? else {
        return Ok(None);
    };
    let fingerprints = CodegenUnitFingerprintSet::new(CodegenUnitFingerprintComponents {
        policy,
        definition,
        declarations,
        target,
    });
    if fingerprints.fingerprint != fingerprint {
        return Ok(None);
    }
    let Some(key_len) = read_u64(file)? else {
        return Ok(None);
    };
    if key_len != u64::try_from(expected_key.len()).unwrap_or(u64::MAX) {
        return Ok(None);
    }
    let mut key = vec![0; expected_key.len()];
    if !read_exact_or_invalid(file, &mut key)? || key != expected_key {
        return Ok(None);
    }
    let Some(payload_len) = read_u64(file)? else {
        return Ok(None);
    };
    let Some(checksum) = read_fingerprint(file)? else {
        return Ok(None);
    };
    let payload_offset = file.stream_position()?;
    let Some(encoded_len) = payload_offset.checked_add(payload_len) else {
        return Ok(None);
    };
    if file.metadata()?.len() != encoded_len {
        return Ok(None);
    }
    Ok(Some(WorkProductHeader {
        fingerprints,
        payload_len,
        checksum,
    }))
}

fn read_payload(
    file: &mut File,
    payload_len: u64,
    expected_checksum: CodegenUnitFingerprint,
    kind: WorkProductKind,
) -> io::Result<Option<Vec<u8>>> {
    let Ok(payload_len) = usize::try_from(payload_len) else {
        return Ok(None);
    };
    let mut payload = Vec::new();
    payload.try_reserve_exact(payload_len).map_err(|_| {
        io::Error::new(
            io::ErrorKind::OutOfMemory,
            "codegen work-product payload allocation failed",
        )
    })?;
    payload.resize(payload_len, 0);
    if !read_exact_or_invalid(file, &mut payload)? {
        return Ok(None);
    }
    Ok((payload_checksum(&payload, kind) == expected_checksum).then_some(payload))
}

fn validate_payload(
    file: &mut File,
    payload_len: u64,
    expected_checksum: CodegenUnitFingerprint,
    kind: WorkProductKind,
) -> io::Result<bool> {
    let mut builder = QueryFingerprintBuilder::new(kind.payload_domain);
    let mut checksum = builder.bytes_writer(payload_len);
    let mut buffer = [0; WORK_PRODUCT_STREAM_BYTES];
    let mut remaining = payload_len;
    while remaining != 0 {
        let chunk_len = crate::stream_chunk_len(remaining, buffer.len());
        if !read_exact_or_invalid(file, &mut buffer[..chunk_len])? {
            return Ok(false);
        }
        checksum.write_chunk(&buffer[..chunk_len])?;
        remaining -= chunk_len as u64;
    }
    checksum.finish()?;
    Ok(CodegenUnitFingerprint::from_parts(builder.finish().parts()) == expected_checksum)
}

fn compare_installed(
    path: &Path,
    key: &CodegenUnitKey,
    fingerprints: CodegenUnitFingerprintSet,
    bytes: &[u8],
    kind: WorkProductKind,
) -> io::Result<InstalledEntry> {
    let mut file = match File::open(path) {
        Ok(file) => file,
        Err(error) if error.kind() == io::ErrorKind::NotFound => {
            return Ok(InstalledEntry::NotFound);
        }
        Err(error) => return Err(error),
    };
    let expected_key = encode_unit_key(key);
    let Some(header) = read_work_product_header(&mut file, &expected_key, kind)? else {
        return Ok(InstalledEntry::Corrupt);
    };
    if header.fingerprints != fingerprints {
        return Ok(InstalledEntry::Collision);
    }
    let expected_len = u64::try_from(bytes.len()).unwrap_or(u64::MAX);
    if header.payload_len != expected_len {
        return Ok(InstalledEntry::Collision);
    }
    let mut builder = QueryFingerprintBuilder::new(kind.payload_domain);
    let mut checksum = builder.bytes_writer(header.payload_len);
    let mut buffer = [0; WORK_PRODUCT_STREAM_BYTES];
    let mut offset = 0usize;
    let mut identical = true;
    while offset != bytes.len() {
        let chunk_len = (bytes.len() - offset).min(buffer.len());
        if !read_exact_or_invalid(&mut file, &mut buffer[..chunk_len])? {
            return Ok(InstalledEntry::Corrupt);
        }
        checksum.write_chunk(&buffer[..chunk_len])?;
        identical &= buffer[..chunk_len] == bytes[offset..offset + chunk_len];
        offset += chunk_len;
    }
    checksum.finish()?;
    if CodegenUnitFingerprint::from_parts(builder.finish().parts()) != header.checksum {
        Ok(InstalledEntry::Corrupt)
    } else if identical {
        Ok(InstalledEntry::Identical)
    } else {
        Ok(InstalledEntry::Collision)
    }
}

fn write_work_product(
    output: &mut impl Write,
    key: &CodegenUnitKey,
    fingerprints: CodegenUnitFingerprintSet,
    bytes: &[u8],
    kind: WorkProductKind,
) -> io::Result<()> {
    let key = encode_unit_key(key);
    output.write_all(kind.format.magic)?;
    output.write_all(&kind.format.release_compatibility.to_le_bytes())?;
    write_fingerprint(output, fingerprints.fingerprint)?;
    for component in [
        fingerprints.components.policy,
        fingerprints.components.definition,
        fingerprints.components.declarations,
        fingerprints.components.target,
    ] {
        write_fingerprint(output, component)?;
    }
    write_u64(output, key.len())?;
    output.write_all(&key)?;
    write_u64(output, bytes.len())?;
    write_fingerprint(output, payload_checksum(bytes, kind))?;
    output.write_all(bytes)
}

fn staged_path(path: &Path) -> PathBuf {
    path.with_extension(format!(
        "tmp.{}.{}",
        std::process::id(),
        WORK_PRODUCT_STAGE_ID.fetch_add(1, Ordering::Relaxed)
    ))
}

struct WorkProductMutationLock {
    _file: File,
}

impl WorkProductMutationLock {
    fn acquire(path: &Path) -> io::Result<Self> {
        let lock_path = path.with_extension("lock");
        let file = OpenOptions::new()
            .read(true)
            .write(true)
            .create(true)
            .truncate(false)
            .open(lock_path)?;
        file.lock()?;
        Ok(Self { _file: file })
    }
}

/// A reader validates an opened inode, then waits for the same per-entry lock
/// used by publishers and removes the path only if its current bytes still
/// match that observation. This prevents a slow corrupt read from deleting a
/// valid replacement installed before retirement acquires the lock.
fn retire_corrupt(path: &Path, observed: &mut File) {
    let Ok(_lock) = WorkProductMutationLock::acquire(path) else {
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
    let mut left_buffer = [0; WORK_PRODUCT_STREAM_BYTES];
    let mut right_buffer = [0; WORK_PRODUCT_STREAM_BYTES];
    let mut remaining = left_len;
    while remaining != 0 {
        let chunk_len = crate::stream_chunk_len(remaining, left_buffer.len());
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
    fingerprint: CodegenUnitFingerprint,
) -> io::Result<()> {
    for part in fingerprint.parts() {
        output.write_all(&part.to_le_bytes())?;
    }
    Ok(())
}

fn read_fingerprint(reader: &mut impl Read) -> io::Result<Option<CodegenUnitFingerprint>> {
    let Some(first) = read_u64(reader)? else {
        return Ok(None);
    };
    let Some(second) = read_u64(reader)? else {
        return Ok(None);
    };
    Ok(Some(CodegenUnitFingerprint::from_parts([first, second])))
}

fn write_u64(output: &mut impl Write, value: usize) -> io::Result<()> {
    let value = u64::try_from(value)
        .map_err(|_| io::Error::new(io::ErrorKind::InvalidInput, "cache field is too large"))?;
    output.write_all(&value.to_le_bytes())
}

fn read_u64(reader: &mut impl Read) -> io::Result<Option<u64>> {
    Ok(read_array::<8>(reader)?.map(u64::from_le_bytes))
}

fn read_u32(reader: &mut impl Read) -> io::Result<Option<u32>> {
    Ok(read_array::<4>(reader)?.map(u32::from_le_bytes))
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
        CodegenUnitKey::LinkageUnit {
            source_identity,
            ordinal,
        } => {
            encoded.push(2);
            let path = source_identity.normalized_path().as_bytes();
            encoded.extend_from_slice(&(path.len() as u64).to_le_bytes());
            encoded.extend_from_slice(path);
            encoded.extend_from_slice(&ordinal.to_le_bytes());
        }
    }
    encoded
}

fn payload_checksum(bytes: &[u8], kind: WorkProductKind) -> CodegenUnitFingerprint {
    let mut builder = QueryFingerprintBuilder::new(kind.payload_domain);
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
        assert_eq!(
            cache.load(&key(), fingerprints).expect("cold load"),
            CodegenWorkProductLookup::NotFound
        );
        cache
            .publish(&key(), fingerprints, b"object bytes")
            .expect("publish");

        assert_eq!(
            cache.load(&key(), fingerprints).expect("load"),
            CodegenWorkProductLookup::Hit(b"object bytes".to_vec())
        );
        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn lto_module_work_products_are_mode_isolated_and_recover_corruption() {
        let root = temp_root("lto-modules");
        let cache = PersistentLtoModuleWorkProductCache::new(root.clone());
        let fingerprints = fingerprints(15);

        cache
            .publish(LtoMode::Thin, &key(), fingerprints, b"thin bitcode")
            .expect("publish ThinLTO module");
        assert_eq!(
            cache
                .load(LtoMode::Thin, &key(), fingerprints)
                .expect("load ThinLTO module"),
            CodegenWorkProductLookup::Hit(b"thin bitcode".to_vec())
        );
        assert_eq!(
            cache
                .load(LtoMode::Full, &key(), fingerprints)
                .expect("cold full-LTO load"),
            CodegenWorkProductLookup::NotFound
        );

        let path = cache
            .cache(LtoMode::Thin)
            .path(&key(), fingerprints.fingerprint);
        let mut encoded = fs::read(&path).expect("read ThinLTO cache entry");
        *encoded.last_mut().expect("payload byte") ^= 0xff;
        fs::write(&path, encoded).expect("corrupt ThinLTO cache entry");
        assert_eq!(
            cache
                .load(LtoMode::Thin, &key(), fingerprints)
                .expect("load corrupt ThinLTO module"),
            CodegenWorkProductLookup::Corrupt
        );
        assert!(!path.exists(), "corrupt pre-link module must be retired");

        cache
            .publish(LtoMode::Thin, &key(), fingerprints, b"regenerated")
            .expect("republish ThinLTO module");
        assert_eq!(
            cache
                .load(LtoMode::Thin, &key(), fingerprints)
                .expect("load regenerated ThinLTO module"),
            CodegenWorkProductLookup::Hit(b"regenerated".to_vec())
        );
        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn large_work_product_round_trips_without_whole_record_encoding() {
        let root = temp_root("large");
        let cache = PersistentObjectWorkProductCache::new(root.clone());
        let fingerprints = fingerprints(20);
        let payload = vec![0x5a; WORK_PRODUCT_STREAM_BYTES * 5 + 17];

        cache
            .publish(&key(), fingerprints, &payload)
            .expect("publish large object");
        assert_eq!(
            cache.load(&key(), fingerprints).expect("load large object"),
            CodegenWorkProductLookup::Hit(payload)
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
            CodegenWorkProductLookup::Corrupt
        );
        assert!(!path.exists(), "corrupt cache entry must be retired");
        cache
            .publish(&key(), fingerprints, b"second")
            .expect("republish");
        assert_eq!(
            cache.load(&key(), fingerprints).expect("load"),
            CodegenWorkProductLookup::Hit(b"second".to_vec())
        );
        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn changed_components_report_invalidation_without_replacing_prior_entry() {
        let root = temp_root("invalidation");
        let cache = PersistentObjectWorkProductCache::new(root.clone());
        let first = fingerprints(50);
        cache
            .publish(&key(), first, &vec![0x33; WORK_PRODUCT_STREAM_BYTES * 3])
            .expect("publish first");
        let changed = CodegenUnitFingerprintSet::new(CodegenUnitFingerprintComponents {
            definition: CodegenUnitFingerprint::from_parts([99, 2]),
            ..first.components
        });

        assert_eq!(
            cache.load(&key(), changed).expect("load changed"),
            CodegenWorkProductLookup::Invalidated(CodegenWorkProductInvalidation {
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

    #[test]
    fn malformed_key_length_is_rejected_without_allocating_it() {
        let root = temp_root("key-length");
        let cache = PersistentObjectWorkProductCache::new(root.clone());
        let fingerprints = fingerprints(60);
        let path = cache.path(&key(), fingerprints.fingerprint);
        fs::create_dir_all(path.parent().expect("cache parent")).expect("create cache parent");
        let mut encoded = Vec::new();
        write_work_product(
            &mut encoded,
            &key(),
            fingerprints,
            b"payload",
            OBJECT_WORK_PRODUCT_KIND,
        )
        .expect("encode");
        let key_len_offset = 8 + 4 + 16 * 5;
        encoded[key_len_offset..key_len_offset + 8].copy_from_slice(&u64::MAX.to_le_bytes());
        fs::write(&path, encoded).expect("write malformed entry");

        assert_eq!(
            cache
                .load(&key(), fingerprints)
                .expect("load malformed entry"),
            CodegenWorkProductLookup::Corrupt
        );
        assert!(!path.exists());
        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn incompatible_release_identity_is_rejected() {
        let root = temp_root("release-identity");
        let cache = PersistentObjectWorkProductCache::new(root.clone());
        let fingerprints = fingerprints(65);
        cache
            .publish(&key(), fingerprints, b"payload")
            .expect("publish entry");
        let path = cache.path(&key(), fingerprints.fingerprint);
        let mut encoded = fs::read(&path).expect("read cache entry");
        encoded[8..12].copy_from_slice(&u32::MAX.to_le_bytes());
        fs::write(&path, encoded).expect("replace release identity");

        assert_eq!(
            cache
                .load(&key(), fingerprints)
                .expect("load incompatible entry"),
            CodegenWorkProductLookup::Corrupt
        );
        assert!(!path.exists());
        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn stale_corruption_retirement_preserves_replacement() {
        let root = temp_root("stale-retirement");
        fs::create_dir_all(&root).expect("create cache root");
        let path = root.join("entry.o");
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
    fn publication_preserves_identical_winner_and_rejects_collision() {
        let root = temp_root("publication-winner");
        let cache = PersistentObjectWorkProductCache::new(root.clone());
        let fingerprints = fingerprints(70);
        cache
            .publish(&key(), fingerprints, b"winner")
            .expect("publish winner");
        cache
            .publish(&key(), fingerprints, b"winner")
            .expect("accept identical publication");

        let error = cache
            .publish(&key(), fingerprints, b"loser")
            .expect_err("reject fingerprint collision");
        assert_eq!(error.kind(), io::ErrorKind::AlreadyExists);
        assert_eq!(
            cache.load(&key(), fingerprints).expect("load winner"),
            CodegenWorkProductLookup::Hit(b"winner".to_vec())
        );
        let _ = fs::remove_dir_all(root);
    }
}
