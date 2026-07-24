use std::{
    fs::{self, File, OpenOptions},
    io::{self, Cursor, Read, Write},
    path::{Path, PathBuf},
    sync::atomic::{AtomicU64, Ordering},
};

use nia_compiler_query::{
    FrontendCacheNamespace, FrontendProviderSummaryCacheKey, SourceContentFingerprint,
};
use nia_imports::StableModuleKey;
use nia_provider_summary::{Provider, ProviderSummary, ProviderTarget, ProviderTypeRef};
use nia_query::{QueryFingerprint, QueryFingerprintBuilder};
use nia_symbol::SymbolId;

const PROVIDER_SUMMARY_MAGIC: &[u8; 8] = b"NIAFPS01";
const FRONTEND_CACHE_SCHEMA: &str = "v1";
const MAX_CACHE_PAYLOAD_BYTES: usize = 64 * 1024 * 1024;
const MAX_CACHE_ENTRY_BYTES: usize = MAX_CACHE_PAYLOAD_BYTES + 1024 * 1024;
const MAX_CACHE_SEQUENCE_LEN: usize = 1_000_000;
static FRONTEND_CACHE_STAGE_ID: AtomicU64 = AtomicU64::new(0);

#[derive(Debug)]
pub(crate) struct PersistentFrontendCache {
    root: PathBuf,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum ProviderSummaryCacheLookup {
    Hit(ProviderSummary),
    NotFound,
    Corrupt,
}

impl PersistentFrontendCache {
    pub(crate) fn new(root: PathBuf) -> Self {
        Self { root }
    }

    pub(crate) fn load_provider_summary(
        &self,
        key: FrontendProviderSummaryCacheKey,
        namespace: FrontendCacheNamespace,
        module: &StableModuleKey,
        source: SourceContentFingerprint,
    ) -> io::Result<ProviderSummaryCacheLookup> {
        let path = self.provider_summary_path(key);
        let encoded = match read_cache_entry(&path)? {
            Some(encoded) if encoded.len() <= MAX_CACHE_ENTRY_BYTES => encoded,
            Some(_) => {
                remove_corrupt(&path);
                return Ok(ProviderSummaryCacheLookup::Corrupt);
            }
            None => return Ok(ProviderSummaryCacheLookup::NotFound),
        };
        let Some(entry) = decode_provider_summary(&encoded) else {
            remove_corrupt(&path);
            return Ok(ProviderSummaryCacheLookup::Corrupt);
        };
        if entry.key != key.parts()
            || entry.namespace != namespace.parts()
            || entry.module != module.source_identity().normalized_path()
            || entry.source != source.parts()
            || path
                != self
                    .provider_summary_path(FrontendProviderSummaryCacheKey::from_parts(entry.key))
        {
            remove_corrupt(&path);
            return Ok(ProviderSummaryCacheLookup::Corrupt);
        }
        Ok(ProviderSummaryCacheLookup::Hit(entry.summary))
    }

    pub(crate) fn publish_provider_summary(
        &self,
        key: FrontendProviderSummaryCacheKey,
        namespace: FrontendCacheNamespace,
        module: &StableModuleKey,
        source: SourceContentFingerprint,
        summary: &ProviderSummary,
    ) -> io::Result<()> {
        let path = self.provider_summary_path(key);
        if path.is_file() {
            return Ok(());
        }
        let parent = path
            .parent()
            .ok_or_else(|| io::Error::other("invalid frontend cache path"))?;
        fs::create_dir_all(parent)?;
        let staged = path.with_extension(format!(
            "tmp.{}.{}",
            std::process::id(),
            FRONTEND_CACHE_STAGE_ID.fetch_add(1, Ordering::Relaxed)
        ));
        let encoded = encode_provider_summary(key, namespace, module, source, summary);
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

    pub(crate) fn remove_provider_summary(&self, key: FrontendProviderSummaryCacheKey) {
        remove_corrupt(&self.provider_summary_path(key));
    }

    pub(crate) fn provider_summary_path(&self, key: FrontendProviderSummaryCacheKey) -> PathBuf {
        let [first, second] = key.parts();
        self.root
            .join("artifacts")
            .join("frontend")
            .join(FRONTEND_CACHE_SCHEMA)
            .join("provider-summaries")
            .join(format!("{first:016x}{second:016x}.fps"))
    }
}

fn read_cache_entry(path: &Path) -> io::Result<Option<Vec<u8>>> {
    let file = match File::open(path) {
        Ok(file) => file,
        Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(None),
        Err(error) => return Err(error),
    };
    let mut encoded = Vec::new();
    file.take((MAX_CACHE_ENTRY_BYTES + 1) as u64)
        .read_to_end(&mut encoded)?;
    Ok(Some(encoded))
}

fn remove_corrupt(path: &Path) {
    match fs::remove_file(path) {
        Ok(()) => {}
        Err(error) if error.kind() == io::ErrorKind::NotFound => {}
        Err(_) => {}
    }
}

fn encode_provider_summary(
    key: FrontendProviderSummaryCacheKey,
    namespace: FrontendCacheNamespace,
    module: &StableModuleKey,
    source: SourceContentFingerprint,
    summary: &ProviderSummary,
) -> Vec<u8> {
    let payload = encode_provider_summary_payload(summary);
    let checksum = payload_checksum(&payload);
    let mut encoded =
        Vec::with_capacity(96 + module.source_identity().normalized_path().len() + payload.len());
    encoded.extend_from_slice(PROVIDER_SUMMARY_MAGIC);
    write_parts(&mut encoded, key.parts());
    write_parts(&mut encoded, namespace.parts());
    write_string(&mut encoded, module.source_identity().normalized_path());
    write_parts(&mut encoded, source.parts());
    encoded.extend_from_slice(&(payload.len() as u64).to_le_bytes());
    write_parts(&mut encoded, checksum.parts());
    encoded.extend_from_slice(&payload);
    encoded
}

struct DecodedProviderSummary {
    key: [u64; 2],
    namespace: [u64; 2],
    module: String,
    source: [u64; 2],
    summary: ProviderSummary,
}

fn decode_provider_summary(encoded: &[u8]) -> Option<DecodedProviderSummary> {
    let mut cursor = Cursor::new(encoded);
    let mut magic = [0; 8];
    cursor.read_exact(&mut magic).ok()?;
    (magic == *PROVIDER_SUMMARY_MAGIC).then_some(())?;
    let key = read_parts(&mut cursor)?;
    let namespace = read_parts(&mut cursor)?;
    let module = read_string(&mut cursor, encoded.len())?;
    let source = read_parts(&mut cursor)?;
    let payload_len = read_len(&mut cursor, MAX_CACHE_PAYLOAD_BYTES)?;
    let checksum = read_parts(&mut cursor)?;
    let mut payload = vec![0; payload_len];
    cursor.read_exact(&mut payload).ok()?;
    (cursor.position() as usize == encoded.len()).then_some(())?;
    (payload_checksum(&payload).parts() == checksum).then_some(())?;
    let summary = decode_provider_summary_payload(&payload)?;
    Some(DecodedProviderSummary {
        key,
        namespace,
        module,
        source,
        summary,
    })
}

fn encode_provider_summary_payload(summary: &ProviderSummary) -> Vec<u8> {
    let mut encoded = Vec::new();
    encoded.extend_from_slice(&(summary.providers().len() as u64).to_le_bytes());
    for provider in summary.providers() {
        write_provider_type_ref(&mut encoded, &provider.target.ty);
        match &provider.trait_ref {
            Some(trait_ref) => {
                encoded.push(1);
                write_provider_type_ref(&mut encoded, trait_ref);
            }
            None => encoded.push(0),
        }
        write_symbols(&mut encoded, &provider.associated_methods);
        write_symbols(&mut encoded, &provider.associated_values);
    }
    encoded
}

fn decode_provider_summary_payload(payload: &[u8]) -> Option<ProviderSummary> {
    let mut cursor = Cursor::new(payload);
    let provider_len = read_len(&mut cursor, MAX_CACHE_SEQUENCE_LEN)?;
    let mut providers = Vec::with_capacity(provider_len);
    for _ in 0..provider_len {
        let target = ProviderTarget {
            ty: read_provider_type_ref(&mut cursor)?,
        };
        let trait_ref = match read_u8(&mut cursor)? {
            0 => None,
            1 => Some(read_provider_type_ref(&mut cursor)?),
            _ => return None,
        };
        providers.push(Provider {
            target,
            trait_ref,
            associated_methods: read_symbols(&mut cursor)?,
            associated_values: read_symbols(&mut cursor)?,
        });
    }
    (cursor.position() as usize == payload.len()).then_some(())?;
    Some(ProviderSummary::from_providers(providers))
}

fn write_provider_type_ref(encoded: &mut Vec<u8>, ty: &ProviderTypeRef) {
    match ty.last_name {
        Some(symbol) => {
            encoded.push(1);
            encoded.extend_from_slice(&symbol.raw().to_le_bytes());
        }
        None => encoded.push(0),
    }
    encoded.push(u8::from(ty.is_generic_or_structural_target));
    encoded.push(u8::from(ty.semantic_is_conservative));
}

fn read_provider_type_ref(cursor: &mut Cursor<&[u8]>) -> Option<ProviderTypeRef> {
    let last_name = match read_u8(cursor)? {
        0 => None,
        1 => Some(SymbolId::from_stable_hash(read_u64(cursor)?)),
        _ => return None,
    };
    Some(ProviderTypeRef {
        last_name,
        is_generic_or_structural_target: read_bool(cursor)?,
        semantic_is_conservative: read_bool(cursor)?,
    })
}

fn write_symbols(encoded: &mut Vec<u8>, symbols: &[SymbolId]) {
    encoded.extend_from_slice(&(symbols.len() as u64).to_le_bytes());
    for symbol in symbols {
        encoded.extend_from_slice(&symbol.raw().to_le_bytes());
    }
}

fn read_symbols(cursor: &mut Cursor<&[u8]>) -> Option<Vec<SymbolId>> {
    let len = read_len(cursor, MAX_CACHE_SEQUENCE_LEN)?;
    let mut symbols = Vec::with_capacity(len);
    for _ in 0..len {
        symbols.push(SymbolId::from_stable_hash(read_u64(cursor)?));
    }
    Some(symbols)
}

fn payload_checksum(payload: &[u8]) -> QueryFingerprint {
    let mut builder = QueryFingerprintBuilder::new("nia.frontend.provider-summary-payload.v1");
    builder.write_bytes(payload);
    builder.finish()
}

fn write_parts(encoded: &mut Vec<u8>, parts: [u64; 2]) {
    for part in parts {
        encoded.extend_from_slice(&part.to_le_bytes());
    }
}

fn read_parts(cursor: &mut Cursor<&[u8]>) -> Option<[u64; 2]> {
    Some([read_u64(cursor)?, read_u64(cursor)?])
}

fn write_string(encoded: &mut Vec<u8>, value: &str) {
    encoded.extend_from_slice(&(value.len() as u64).to_le_bytes());
    encoded.extend_from_slice(value.as_bytes());
}

fn read_string(cursor: &mut Cursor<&[u8]>, encoded_len: usize) -> Option<String> {
    let len = read_len(cursor, encoded_len)?;
    let mut bytes = vec![0; len];
    cursor.read_exact(&mut bytes).ok()?;
    String::from_utf8(bytes).ok()
}

fn read_len(cursor: &mut Cursor<&[u8]>, limit: usize) -> Option<usize> {
    let len = usize::try_from(read_u64(cursor)?).ok()?;
    (len <= limit).then_some(len)
}

fn read_bool(cursor: &mut Cursor<&[u8]>) -> Option<bool> {
    match read_u8(cursor)? {
        0 => Some(false),
        1 => Some(true),
        _ => None,
    }
}

fn read_u8(cursor: &mut Cursor<&[u8]>) -> Option<u8> {
    let mut bytes = [0; 1];
    cursor.read_exact(&mut bytes).ok()?;
    Some(bytes[0])
}

fn read_u64(cursor: &mut Cursor<&[u8]>) -> Option<u64> {
    let mut bytes = [0; 8];
    cursor.read_exact(&mut bytes).ok()?;
    Some(u64::from_le_bytes(bytes))
}
