// SPDX-License-Identifier: GPL-3.0-or-later
//! Stable source identities, versioned files, and concurrent source storage.

use parking_lot::Mutex;
use std::{
    fs,
    hash::{Hash, Hasher},
    io::{self, Read},
    path::Path,
    sync::{
        Arc,
        atomic::{AtomicU32, Ordering},
    },
};

/// Maximum UTF-8 source bytes accepted from one filesystem file.
pub const MAX_SOURCE_FILE_BYTES: usize = 64 * 1024 * 1024;

/// Reads one UTF-8 source file through the shared compiler input budget.
///
/// Metadata rejects an already oversized file before allocation. Reading at
/// most `max + 1` bytes also detects growth after that metadata observation, so
/// a valid source prefix cannot hide an oversized trailing payload.
pub fn read_source_text(path: impl AsRef<Path>) -> io::Result<String> {
    let path = path.as_ref();
    let file = fs::File::open(path)?;
    let length = file.metadata()?.len();
    if length > MAX_SOURCE_FILE_BYTES as u64 {
        return Err(source_file_too_large());
    }
    let capacity = usize::try_from(length).unwrap_or(MAX_SOURCE_FILE_BYTES);
    let mut encoded = Vec::with_capacity(capacity);
    file.take((MAX_SOURCE_FILE_BYTES + 1) as u64)
        .read_to_end(&mut encoded)?;
    if encoded.len() > MAX_SOURCE_FILE_BYTES {
        return Err(source_file_too_large());
    }
    String::from_utf8(encoded).map_err(|error| io::Error::new(io::ErrorKind::InvalidData, error))
}

fn source_file_too_large() -> io::Error {
    io::Error::new(
        io::ErrorKind::InvalidData,
        format!("source file exceeds the {MAX_SOURCE_FILE_BYTES}-byte limit"),
    )
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
/// Identity of the source store that owns compact source handles.
pub struct SourceStoreId(u32);

impl SourceStoreId {
    fn fresh() -> Self {
        static NEXT_SOURCE_STORE_ID: AtomicU32 = AtomicU32::new(1);
        let id = NEXT_SOURCE_STORE_ID
            .fetch_update(Ordering::Relaxed, Ordering::Relaxed, |id| id.checked_add(1))
            .unwrap_or(u32::MAX);
        Self(id)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
/// Monotonic source index within one source store.
struct SourceIndex(u32);

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
/// Compact session-local source handle scoped to one source store.
pub struct SourceId {
    store_id: SourceStoreId,
    index: SourceIndex,
}

impl SourceId {
    /// Creates a handle for one source that is intentionally not table-managed.
    ///
    /// This is used by standalone parsing APIs. Related sources must instead be
    /// allocated by the same [`SourceTable`] so owner checks remain meaningful.
    pub fn isolated() -> Self {
        Self {
            store_id: SourceStoreId::fresh(),
            index: SourceIndex(0),
        }
    }

    /// Returns the source store that owns this handle.
    pub fn store_id(self) -> SourceStoreId {
        self.store_id
    }

    /// Returns the numeric identity of the owning source store.
    pub fn store_index(self) -> u32 {
        self.store_id.0
    }

    /// Returns the compact index within the owning source store.
    pub fn local_index(self) -> u32 {
        self.index.0
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
/// Monotonic version of source text stored under one [`SourceId`].
pub struct SourceRevision(pub u64);

impl SourceRevision {
    /// Revision assigned to the first stored version of a source.
    pub const INITIAL: Self = Self(0);

    /// Returns the following source revision.
    pub const fn next(self) -> Option<Self> {
        match self.0.checked_add(1) {
            Some(value) => Some(Self(value)),
            None => None,
        }
    }
}

/// Failure to allocate or resolve an owner-qualified source identity.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SourceIdentityError {
    /// The source store cannot allocate another compact index.
    IdentitySpaceExhausted,
    /// The source text revision cannot advance further.
    RevisionSpaceExhausted,
    /// A handle owned by another source store was presented to this store.
    ForeignSource {
        /// Source store required by the operation.
        expected: SourceStoreId,
        /// Source store carried by the supplied handle.
        actual: SourceStoreId,
    },
    /// The handle has this store's owner but does not name an allocated path.
    UnknownSource(SourceId),
}

impl std::fmt::Display for SourceIdentityError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::IdentitySpaceExhausted => f.write_str("source identity space exhausted"),
            Self::RevisionSpaceExhausted => f.write_str("source revision space exhausted"),
            Self::ForeignSource { .. } => {
                f.write_str("source handle belongs to a different source store")
            }
            Self::UnknownSource(_) => f.write_str("source handle does not refer to a known source"),
        }
    }
}

impl std::error::Error for SourceIdentityError {}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
/// Exact identity of one version of a source file.
pub struct SourceVersion {
    /// Stable source identity within the source table.
    pub id: SourceId,
    /// Text revision expected by the consumer.
    pub revision: SourceRevision,
}

#[derive(Debug, Clone, PartialEq, Eq, Hash, PartialOrd, Ord)]
/// Relocation-stable logical identity derived from normalized path text.
pub struct SourceIdentity {
    normalized_path: Arc<str>,
}

#[derive(Debug, Clone, PartialEq, Eq, Hash, PartialOrd, Ord)]
/// Stable logical source coordinate embedded in generated programs.
pub struct SourceLocation {
    /// Relocation-stable UTF-8 source identity.
    pub file: String,
    /// One-based source line.
    pub line: u32,
    /// One-based Unicode-scalar column.
    pub column: u32,
}

impl SourceLocation {
    /// Resolves a byte offset using the same scalar-column convention as diagnostics.
    pub fn at(identity: &SourceIdentity, source: &str, offset: usize) -> Self {
        let offset = clamp_to_char_boundary(source, offset.min(source.len()));
        let mut line = 1_u32;
        let mut line_start = 0;
        for (index, ch) in source.char_indices() {
            if index >= offset {
                break;
            }
            if ch == '\n' {
                line = line.saturating_add(1);
                line_start = index + ch.len_utf8();
            }
        }
        let column = source[line_start..offset].chars().count().saturating_add(1);
        Self {
            file: identity.normalized_path().to_owned(),
            line,
            column: u32::try_from(column).unwrap_or(u32::MAX),
        }
    }
}

fn clamp_to_char_boundary(source: &str, mut offset: usize) -> usize {
    while offset > 0 && !source.is_char_boundary(offset) {
        offset -= 1;
    }
    offset
}

impl SourceIdentity {
    /// Creates an identity after normalizing the supplied path text.
    pub fn new(path: impl AsRef<str>) -> Self {
        Self {
            normalized_path: normalize_path(path.as_ref()).into(),
        }
    }

    fn from_normalized(normalized_path: Arc<str>) -> Self {
        Self { normalized_path }
    }

    /// Clones the logical identity carried by a source path.
    pub fn from_path(path: &SourcePath) -> Self {
        path.identity.clone()
    }

    /// Returns normalized logical path text.
    pub fn normalized_path(&self) -> &str {
        &self.normalized_path
    }
}

#[derive(Debug, Clone)]
/// Physical source location paired with a logical identity.
///
/// Equality and hashing use only the logical identity, allowing relocated
/// toolchain/package sources to retain stable compiler identities.
pub struct SourcePath {
    physical_path: Arc<str>,
    identity: SourceIdentity,
}

impl PartialEq for SourcePath {
    fn eq(&self, other: &Self) -> bool {
        self.identity == other.identity
    }
}

impl Eq for SourcePath {}

impl Hash for SourcePath {
    fn hash<H: Hasher>(&self, state: &mut H) {
        self.identity.hash(state);
    }
}

impl SourcePath {
    /// Creates a path whose physical location and logical identity match.
    pub fn new(path: impl Into<String>) -> Self {
        let physical_path: Arc<str> = normalize_path(&path.into()).into();
        Self {
            identity: SourceIdentity::from_normalized(physical_path.clone()),
            physical_path,
        }
    }

    /// Builds a source path from text that already satisfies `normalize_path`.
    pub fn from_normalized_unchecked(path: impl Into<Arc<str>>) -> Self {
        let physical_path = path.into();
        Self {
            identity: SourceIdentity::from_normalized(physical_path.clone()),
            physical_path,
        }
    }

    /// Builds a relocated source path from text that already satisfies
    /// `normalize_path` for both its physical and logical coordinates.
    /// The caller is responsible for upholding that invariant.
    pub fn with_normalized_identity_unchecked(
        physical_path: impl Into<Arc<str>>,
        logical_identity: impl Into<Arc<str>>,
    ) -> Self {
        let physical_path = physical_path.into();
        let logical_identity = logical_identity.into();
        let normalized_path = if physical_path == logical_identity {
            physical_path.clone()
        } else {
            logical_identity
        };
        Self {
            physical_path,
            identity: SourceIdentity::from_normalized(normalized_path),
        }
    }

    /// Creates a path with separate physical and relocation-stable identities.
    pub fn with_identity(
        physical_path: impl Into<String>,
        logical_identity: impl AsRef<str>,
    ) -> Self {
        Self::with_normalized_identity_unchecked(
            normalize_path(&physical_path.into()),
            normalize_path(logical_identity.as_ref()),
        )
    }

    /// Returns the normalized physical path used for I/O.
    pub fn as_str(&self) -> &str {
        &self.physical_path
    }

    /// Clones the logical source identity.
    pub fn identity(&self) -> SourceIdentity {
        SourceIdentity::from_path(self)
    }

    /// Borrows the relocation-stable logical identity.
    pub fn identity_ref(&self) -> &SourceIdentity {
        &self.identity
    }

    /// Derives a normalized child file while preserving logical relocation.
    pub fn derived_child_file(&self, child: &str, sibling: bool) -> Self {
        let physical = derived_child_path_text(self.as_str(), child, sibling);
        let logical_parent = self.identity_ref().normalized_path();
        if self.as_str() == logical_parent {
            return Self::from_normalized_unchecked(physical);
        }
        let logical = derived_child_path_text(logical_parent, child, sibling);
        Self::with_normalized_identity_unchecked(physical, logical)
    }
}

fn derived_child_path_text(parent_path: &str, child: &str, sibling: bool) -> String {
    let base = if sibling {
        parent_path.rsplit_once('/').map_or("", |(dir, _)| dir)
    } else {
        parent_path.strip_suffix(".nia").unwrap_or(parent_path)
    };
    let mut path = String::with_capacity(
        base.len() + usize::from(!base.is_empty()) + child.len() + ".nia".len(),
    );
    if !base.is_empty() {
        path.push_str(base);
        path.push('/');
    }
    path.push_str(child);
    path.push_str(".nia");
    path
}

/// Lexically normalizes `/`, `.`, and `..` path components.
pub fn normalize_path(path: &str) -> String {
    let path = path.replace('\\', "/");
    let path = path.strip_prefix("//?/").unwrap_or(&path);
    let drive = path.as_bytes().get(1) == Some(&b':');
    let absolute = path.starts_with('/') || drive;
    let mut parts = Vec::new();
    let mut components = path.split('/');
    let prefix = if drive {
        components.next().unwrap_or_default()
    } else {
        ""
    };
    for part in components {
        match part {
            "" | "." => {}
            ".." => {
                if parts.last().is_some_and(|part| *part != "..") {
                    parts.pop();
                } else if !absolute {
                    parts.push(part);
                }
            }
            _ => parts.push(part),
        }
    }
    let normalized = if drive && !prefix.is_empty() {
        if parts.is_empty() {
            prefix.to_string()
        } else {
            format!("{prefix}/{}", parts.join("/"))
        }
    } else {
        parts.join("/")
    };
    if absolute && !drive {
        format!("/{normalized}")
    } else {
        normalized
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
/// Immutable snapshot of source text at one revision.
pub struct SourceFile {
    /// Session-local source identity.
    pub id: SourceId,
    /// Physical path and logical identity.
    pub path: SourcePath,
    /// Revision of the stored text.
    pub revision: SourceRevision,
    /// Shared source text.
    pub text: Arc<str>,
}

impl SourceFile {
    /// Creates a source file at [`SourceRevision::INITIAL`].
    pub fn new(id: SourceId, path: SourcePath, text: impl Into<Arc<str>>) -> Self {
        Self {
            id,
            path,
            revision: SourceRevision::INITIAL,
            text: text.into(),
        }
    }

    /// Replaces the snapshot revision.
    pub fn with_revision(mut self, revision: SourceRevision) -> Self {
        self.revision = revision;
        self
    }

    /// Returns the exact id/revision pair for this snapshot.
    pub fn version(&self) -> SourceVersion {
        SourceVersion {
            id: self.id,
            revision: self.revision,
        }
    }
}

#[derive(Debug, Clone)]
/// Concurrent bijection between logical source paths and session-local ids.
pub struct SourceTable {
    id: SourceStoreId,
    inner: Arc<Mutex<SourceTableInner>>,
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
/// Work counters for one source/path interner lifetime.
pub struct SourceTableStats {
    /// Number of canonical source paths interned so far.
    pub path_count: u64,
    /// Total child derivation requests.
    pub child_requests: u64,
    /// Child derivations served without constructing a path.
    pub child_hits: u64,
    /// Unique child derivations constructed and interned.
    pub child_misses: u64,
}

#[derive(Debug, Default)]
struct SourceTableInner {
    ids_by_path: nia_hash::FastHashMap<Arc<SourcePath>, SourceId>,
    paths_by_id: Vec<Arc<SourcePath>>,
    child_ids: nia_hash::FastHashMap<(SourceId, bool), nia_hash::FastHashMap<Arc<str>, SourceId>>,
    child_requests: u64,
    child_hits: u64,
    child_misses: u64,
    next_id: u32,
}

impl SourceTable {
    /// Creates an empty source identity table.
    pub fn new() -> Self {
        Self {
            id: SourceStoreId::fresh(),
            inner: Arc::new(Mutex::new(SourceTableInner::default())),
        }
    }

    /// Returns this table's unique session-local identity.
    pub fn id(&self) -> SourceStoreId {
        self.id
    }

    /// Returns current path and child-derivation work counts.
    pub fn stats(&self) -> SourceTableStats {
        let inner = self.inner.lock();
        SourceTableStats {
            path_count: inner.paths_by_id.len() as u64,
            child_requests: inner.child_requests,
            child_hits: inner.child_hits,
            child_misses: inner.child_misses,
        }
    }

    /// Returns the existing id for a path or allocates the next id.
    pub fn id_for_path(&self, path: &SourcePath) -> Result<SourceId, SourceIdentityError> {
        let mut inner = self.inner.lock();
        self.id_for_path_locked(&mut inner, path.clone())
    }

    /// Returns or allocates a child file derived from an interned parent.
    pub fn id_for_child_path(
        &self,
        parent: SourceId,
        child: &str,
        sibling: bool,
    ) -> Result<SourceId, SourceIdentityError> {
        if parent.store_id != self.id {
            return Err(SourceIdentityError::ForeignSource {
                expected: self.id,
                actual: parent.store_id,
            });
        }
        let mut inner = self.inner.lock();
        inner.child_requests = inner.child_requests.saturating_add(1);
        if let Some(id) = inner
            .child_ids
            .get(&(parent, sibling))
            .and_then(|children| children.get(child))
            .copied()
        {
            inner.child_hits = inner.child_hits.saturating_add(1);
            return Ok(id);
        }
        inner.child_misses = inner.child_misses.saturating_add(1);
        let parent_path = inner
            .paths_by_id
            .get(
                usize::try_from(parent.index.0)
                    .map_err(|_| SourceIdentityError::UnknownSource(parent))?,
            )
            .cloned()
            .ok_or(SourceIdentityError::UnknownSource(parent))?;
        let child_path = parent_path.derived_child_file(child, sibling);
        let child_id = self.id_for_path_locked(&mut inner, child_path)?;
        inner
            .child_ids
            .entry((parent, sibling))
            .or_default()
            .insert(Arc::from(child), child_id);
        Ok(child_id)
    }

    /// Looks up an id without allocating one for a missing path.
    pub fn existing_id_for_path(&self, path: &SourcePath) -> Option<SourceId> {
        self.inner.lock().ids_by_path.get(path).copied()
    }

    /// Returns the logical path registered for an id.
    pub fn path_for_id(&self, id: SourceId) -> Option<Arc<SourcePath>> {
        if id.store_id != self.id {
            return None;
        }
        self.inner
            .lock()
            .paths_by_id
            .get(usize::try_from(id.index.0).ok()?)
            .cloned()
    }

    fn id_for_path_locked(
        &self,
        inner: &mut SourceTableInner,
        path: SourcePath,
    ) -> Result<SourceId, SourceIdentityError> {
        if let Some(id) = inner.ids_by_path.get(&path).copied() {
            return Ok(id);
        }
        let id = SourceId {
            store_id: self.id,
            index: SourceIndex(inner.next_id),
        };
        inner.next_id = inner
            .next_id
            .checked_add(1)
            .ok_or(SourceIdentityError::IdentitySpaceExhausted)?;
        let path = Arc::new(path);
        inner.ids_by_path.insert(path.clone(), id);
        inner.paths_by_id.push(path);
        Ok(id)
    }
}

impl Default for SourceTable {
    fn default() -> Self {
        Self::new()
    }
}

#[derive(Debug, Clone, Default)]
/// Concurrent store of the current source snapshot for each source id.
pub struct SourceDatabase {
    table: SourceTable,
    files: Arc<Mutex<nia_hash::FastHashMap<SourceId, SourceFile>>>,
}

impl SourceDatabase {
    /// Creates an empty source database and identity table.
    pub fn new() -> Self {
        Self::default()
    }

    /// Returns or allocates the id for a logical path.
    pub fn id_for_path(&self, path: &SourcePath) -> Result<SourceId, SourceIdentityError> {
        self.table.id_for_path(path)
    }

    /// Returns the shared source/path interner backing this database.
    pub fn source_table(&self) -> SourceTable {
        self.table.clone()
    }

    /// Returns source/path interner work counts for this database.
    pub fn source_table_stats(&self) -> SourceTableStats {
        self.table.stats()
    }

    /// Returns the path registered for an id.
    pub fn path_for_id(&self, id: SourceId) -> Option<Arc<SourcePath>> {
        self.table.path_for_id(id)
    }

    /// Returns the current snapshot for a path without allocating an id.
    pub fn source_for_path(&self, path: &SourcePath) -> Option<SourceFile> {
        let id = self.table.existing_id_for_path(path)?;
        self.source_for_id(id)
    }

    /// Returns the current snapshot for an id.
    pub fn source_for_id(&self, id: SourceId) -> Option<SourceFile> {
        self.files.lock().get(&id).cloned()
    }

    /// Returns a snapshot only when its current revision exactly matches.
    pub fn source_for_version(&self, version: SourceVersion) -> Option<SourceFile> {
        self.source_for_id(version.id)
            .filter(|file| file.revision == version.revision)
    }

    /// Returns all current snapshots in unspecified order.
    pub fn source_files(&self) -> Vec<SourceFile> {
        self.files.lock().values().cloned().collect()
    }

    /// Stores text, preserving its id and advancing an existing revision.
    pub fn set_source(
        &self,
        path: SourcePath,
        text: impl Into<Arc<str>>,
    ) -> Result<SourceFile, SourceIdentityError> {
        let id = self.id_for_path(&path)?;
        let mut files = self.files.lock();
        let revision = match files.get(&id) {
            Some(file) => file
                .revision
                .next()
                .ok_or(SourceIdentityError::RevisionSpaceExhausted)?,
            None => SourceRevision::INITIAL,
        };
        let file = SourceFile::new(id, path, text).with_revision(revision);
        files.insert(id, file.clone());
        Ok(file)
    }

    /// Returns a cached snapshot or reads and stores the source from disk.
    pub fn read_source(&self, path: &SourcePath) -> io::Result<SourceFile> {
        if let Some(file) = self.source_for_path(path) {
            return Ok(file);
        }

        let text = read_source_text(path.as_str())?;
        self.set_source(path.clone(), text)
            .map_err(io::Error::other)
    }

    /// Creates an unstored empty snapshot for a path at the initial revision.
    pub fn empty_source(&self, path: &SourcePath) -> Result<SourceFile, SourceIdentityError> {
        Ok(SourceFile::new(self.id_for_path(path)?, path.clone(), ""))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn source_revision_advances_monotonically() {
        assert_eq!(SourceRevision::INITIAL.next(), Some(SourceRevision(1)));
    }

    #[test]
    fn source_identity_errors_do_not_expose_internal_handles() {
        let table = SourceTable::new();
        let foreign = SourceTable::new();
        let foreign_error = SourceIdentityError::ForeignSource {
            expected: table.id(),
            actual: foreign.id(),
        };
        let unknown_error = SourceIdentityError::UnknownSource(SourceId {
            store_id: table.id(),
            index: SourceIndex(u32::MAX),
        });

        assert_eq!(
            foreign_error.to_string(),
            "source handle belongs to a different source store"
        );
        assert_eq!(
            unknown_error.to_string(),
            "source handle does not refer to a known source"
        );
        assert!(!foreign_error.to_string().contains("SourceStoreId"));
        assert!(!unknown_error.to_string().contains("SourceId"));
    }

    #[test]
    fn source_revision_overflow_is_rejected() {
        assert_eq!(SourceRevision(u64::MAX).next(), None);
    }

    #[test]
    fn source_file_defaults_to_initial_revision() {
        let source_id = SourceId::isolated();
        let file = SourceFile::new(source_id, SourcePath::new("main.nia"), "fn main() {}");

        assert_eq!(file.revision, SourceRevision::INITIAL);
        assert_eq!(
            file.version(),
            SourceVersion {
                id: source_id,
                revision: SourceRevision::INITIAL
            }
        );
        assert_eq!(file.path.as_str(), "main.nia");
    }

    #[test]
    fn source_path_identity_uses_normalized_path_text() {
        let path = SourcePath::new("src/./root.nia");
        let identity = path.identity();

        assert_eq!(identity.normalized_path(), "src/root.nia");
        assert_eq!(identity, SourceIdentity::from_path(&path));
        assert!(Arc::ptr_eq(
            &path.physical_path,
            &path.identity.normalized_path
        ));
    }

    #[test]
    fn normalized_source_paths_share_equal_physical_and_logical_text() {
        let path = SourcePath::with_normalized_identity_unchecked(
            "toolchain:/std/pkg.nia",
            "toolchain:/std/pkg.nia",
        );

        assert_eq!(path.as_str(), path.identity_ref().normalized_path());
        assert!(Arc::ptr_eq(
            &path.physical_path,
            &path.identity.normalized_path
        ));
    }

    #[test]
    fn source_path_normalization_preserves_unresolved_relative_parents() {
        assert_eq!(normalize_path("../dep.nia"), "../dep.nia");
        assert_eq!(normalize_path("a/../../dep.nia"), "../dep.nia");
        assert_eq!(normalize_path("../../dep.nia"), "../../dep.nia");
        assert_eq!(normalize_path("/a/../../dep.nia"), "/dep.nia");
        assert_ne!(SourcePath::new("../dep.nia"), SourcePath::new("dep.nia"));
    }

    #[test]
    fn source_path_normalization_handles_windows_extended_drive_paths() {
        assert_eq!(
            normalize_path(r"\\?\C:\Users\nia\lib\std\pkg.nia"),
            "C:/Users/nia/lib/std/pkg.nia"
        );
        assert_eq!(normalize_path(r"C:\work\..\lib\pkg.nia"), "C:/lib/pkg.nia");
    }

    #[test]
    fn source_database_reads_parent_relative_physical_path() {
        let root =
            std::env::temp_dir().join(format!("nia-source-parent-relative-{}", std::process::id()));
        let child = root.join("child");
        fs::create_dir_all(&child).expect("create child directory");
        fs::write(root.join("dep.nia"), "parent").expect("write parent source");
        fs::write(child.join("dep.nia"), "child").expect("write child source");
        let parent_path = SourcePath::new(format!("{}/../dep.nia", child.display()));
        let child_path = SourcePath::new(child.join("dep.nia").to_string_lossy());
        let sources = SourceDatabase::new();

        let parent = sources
            .read_source(&parent_path)
            .expect("read parent source");
        let child = sources.read_source(&child_path).expect("read child source");

        assert_eq!(parent.text.as_ref(), "parent");
        assert_eq!(child.text.as_ref(), "child");
        assert_ne!(parent.id, child.id);
    }

    #[test]
    fn source_path_can_separate_physical_location_from_logical_identity() {
        let path = SourcePath::with_identity(
            "/opt/nia/lib/std/collections.nia",
            "toolchain:/std/collections.nia",
        );

        assert_eq!(path.as_str(), "/opt/nia/lib/std/collections.nia");
        assert_eq!(
            path.identity().normalized_path(),
            "toolchain:/std/collections.nia"
        );
        assert_ne!(path, SourcePath::new(path.as_str()));
        assert_eq!(
            path,
            SourcePath::with_identity(
                "/relocated/lib/std/collections.nia",
                "toolchain:/std/collections.nia",
            )
        );
    }

    #[test]
    fn source_locations_use_one_based_unicode_scalar_columns() {
        let identity = SourceIdentity::new("package:demo:/main.nia");
        let source = "first\n    let 文 = callerLocation();\n";
        let offset = source
            .find("callerLocation")
            .expect("caller location offset");

        assert_eq!(
            SourceLocation::at(&identity, source, offset),
            SourceLocation {
                file: "package:demo:/main.nia".to_string(),
                line: 2,
                column: 13,
            }
        );
    }

    #[test]
    fn source_table_reuses_path_ids() {
        let table = SourceTable::new();
        let main = SourcePath::new("main.nia");
        let defs = SourcePath::new("defs.nia");

        let main_id = table.id_for_path(&main).expect("main id");
        let defs_id = table.id_for_path(&defs).expect("defs id");

        assert_eq!(main_id.store_id(), table.id());
        assert_eq!(main_id.local_index(), 0);
        assert_eq!(defs_id.local_index(), 1);
        assert_eq!(table.id_for_path(&main), Ok(main_id));
        assert_eq!(table.path_for_id(main_id).as_deref(), Some(&main));
        assert_eq!(table.path_for_id(defs_id).as_deref(), Some(&defs));
    }

    #[test]
    fn source_table_rejects_foreign_ids_with_matching_local_indices() {
        let table = SourceTable::new();
        let foreign = SourceTable::new();
        let path = SourcePath::new("main.nia");
        let id = table.id_for_path(&path).expect("allocate source id");
        let foreign_id = foreign.id_for_path(&path).expect("foreign source id");

        assert_eq!(id.local_index(), foreign_id.local_index());
        assert_ne!(id, foreign_id);
        assert_eq!(table.path_for_id(foreign_id), None);
    }

    #[test]
    fn source_table_interns_child_derivations_and_preserves_relocation() {
        let table = SourceTable::new();
        let root = SourcePath::with_identity("/opt/nia/lib/std/pkg.nia", "toolchain:/std/pkg.nia");
        let root_id = table.id_for_path(&root).expect("root id");

        let first = table
            .id_for_child_path(root_id, "collections", true)
            .expect("first child id");
        let repeated = table
            .id_for_child_path(root_id, "collections", true)
            .expect("repeated child id");
        let nested = table
            .id_for_child_path(root_id, "collections", false)
            .expect("nested child id");

        assert_eq!(first, repeated);
        assert_ne!(first, nested);
        assert_eq!(
            table.stats(),
            SourceTableStats {
                path_count: 3,
                child_requests: 3,
                child_hits: 1,
                child_misses: 2,
            }
        );
        let first_path = table.path_for_id(first).expect("first child path");
        assert_eq!(first_path.as_str(), "/opt/nia/lib/std/collections.nia");
        assert_eq!(
            first_path.identity_ref().normalized_path(),
            "toolchain:/std/collections.nia"
        );
        assert_eq!(
            table
                .path_for_id(nested)
                .expect("nested child path")
                .as_str(),
            "/opt/nia/lib/std/pkg/collections.nia"
        );
    }

    #[test]
    fn source_table_rejects_foreign_child_parents() {
        let table = SourceTable::new();
        let foreign = SourceTable::new();
        let foreign_parent = foreign
            .id_for_path(&SourcePath::new("main.nia"))
            .expect("foreign parent id");

        assert_eq!(
            table
                .id_for_child_path(foreign_parent, "child", true)
                .expect_err("reject foreign parent"),
            SourceIdentityError::ForeignSource {
                expected: table.id(),
                actual: foreign.id(),
            }
        );

        let unknown_parent = SourceId {
            store_id: table.id(),
            index: SourceIndex(u32::MAX),
        };
        assert_eq!(
            table
                .id_for_child_path(unknown_parent, "child", true)
                .expect_err("reject unknown parent"),
            SourceIdentityError::UnknownSource(unknown_parent)
        );
    }

    #[test]
    fn source_database_stores_in_memory_sources() {
        let sources = SourceDatabase::new();
        let path = SourcePath::new("main.nia");

        let file = sources
            .set_source(path.clone(), "fn main() i32 { 0 }")
            .expect("store source");

        assert_eq!(file.id.store_id(), sources.table.id());
        assert_eq!(file.id.local_index(), 0);
        assert_eq!(file.revision, SourceRevision::INITIAL);
        assert_eq!(sources.source_for_path(&path), Some(file));
    }

    #[test]
    fn source_database_path_lookup_does_not_allocate_missing_ids() {
        let sources = SourceDatabase::new();
        let missing = SourcePath::new("missing.nia");
        let main = SourcePath::new("main.nia");

        assert_eq!(sources.source_for_path(&missing), None);

        let file = sources
            .set_source(main.clone(), "fn main() i32 { 0 }")
            .expect("store source");
        assert_eq!(file.id.local_index(), 0);
        let missing_id = sources.id_for_path(&missing).expect("missing source id");
        assert_eq!(missing_id.store_id(), file.id.store_id());
        assert_eq!(missing_id.local_index(), 1);
    }

    #[test]
    fn source_database_replacement_advances_revision() {
        let sources = SourceDatabase::new();
        let path = SourcePath::new("main.nia");

        let first = sources
            .set_source(path.clone(), "fn main() i32 { 0 }")
            .expect("store first source");
        let second = sources
            .set_source(path.clone(), "fn main() i32 { 1 }")
            .expect("store second source");

        assert_eq!(first.id, second.id);
        assert_eq!(first.revision, SourceRevision::INITIAL);
        assert_eq!(second.revision, SourceRevision(1));
        assert_eq!(second.text.as_ref(), "fn main() i32 { 1 }");
    }

    #[test]
    fn source_database_reads_sources_by_version() {
        let sources = SourceDatabase::new();
        let path = SourcePath::new("main.nia");

        let first = sources
            .set_source(path.clone(), "fn main() i32 { 0 }")
            .expect("store first source");
        let second = sources
            .set_source(path, "fn main() i32 { 1 }")
            .expect("store second source");

        assert_eq!(sources.source_for_version(first.version()), None);
        assert_eq!(sources.source_for_version(second.version()), Some(second));
    }

    #[test]
    fn filesystem_source_reads_reject_oversized_files_before_storing_them() {
        let path =
            std::env::temp_dir().join(format!("nia-source-oversized-{}", std::process::id()));
        let file = fs::File::create(&path).expect("create oversized source");
        file.set_len((MAX_SOURCE_FILE_BYTES + 1) as u64)
            .expect("extend oversized source");
        let source_path = SourcePath::new(path.to_string_lossy());
        let sources = SourceDatabase::new();

        let error = sources
            .read_source(&source_path)
            .expect_err("oversized source must be rejected");

        assert_eq!(error.kind(), io::ErrorKind::InvalidData);
        assert!(error.to_string().contains("67108864-byte limit"), "{error}");
        assert_eq!(sources.source_for_path(&source_path), None);
    }
}
