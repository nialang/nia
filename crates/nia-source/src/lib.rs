// SPDX-License-Identifier: GPL-3.0-or-later
//! Stable source identities, versioned files, and concurrent source storage.

use std::{
    fs,
    hash::{Hash, Hasher},
    io::{self, Read},
    path::Path,
    sync::{Arc, Mutex},
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
/// Session-local identity assigned to one logical source path.
pub struct SourceId(pub u32);

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
/// Monotonic version of source text stored under one [`SourceId`].
pub struct SourceRevision(pub u64);

impl SourceRevision {
    /// Revision assigned to the first stored version of a source.
    pub const INITIAL: Self = Self(0);

    /// Returns the following source revision.
    pub const fn next(self) -> Self {
        Self(
            self.0
                .checked_add(1)
                .expect("source revision space exhausted"),
        )
    }
}

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
    normalized_path: String,
}

impl SourceIdentity {
    /// Creates an identity after normalizing the supplied path text.
    pub fn new(path: impl AsRef<str>) -> Self {
        Self {
            normalized_path: normalize_path(path.as_ref()),
        }
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
    physical_path: String,
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
        let physical_path = normalize_path(&path.into());
        Self {
            identity: SourceIdentity::new(&physical_path),
            physical_path,
        }
    }

    /// Builds a source path from text that already satisfies `normalize_path`.
    pub fn from_normalized_unchecked(path: impl Into<String>) -> Self {
        let physical_path = path.into();
        Self {
            identity: SourceIdentity::new(&physical_path),
            physical_path,
        }
    }

    /// Creates a path with separate physical and relocation-stable identities.
    pub fn with_identity(
        physical_path: impl Into<String>,
        logical_identity: impl AsRef<str>,
    ) -> Self {
        Self {
            physical_path: normalize_path(&physical_path.into()),
            identity: SourceIdentity::new(logical_identity),
        }
    }

    /// Returns the normalized physical path used for I/O.
    pub fn as_str(&self) -> &str {
        &self.physical_path
    }

    /// Clones the logical source identity.
    pub fn identity(&self) -> SourceIdentity {
        SourceIdentity::from_path(self)
    }
}

/// Lexically normalizes `/`, `.`, and `..` path components.
pub fn normalize_path(path: &str) -> String {
    let absolute = path.starts_with('/');
    let mut parts = Vec::new();
    for part in path.split('/') {
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
    let normalized = parts.join("/");
    if absolute {
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

#[derive(Debug, Clone, Default)]
/// Concurrent bijection between logical source paths and session-local ids.
pub struct SourceTable {
    inner: Arc<Mutex<SourceTableInner>>,
}

#[derive(Debug, Default)]
struct SourceTableInner {
    ids_by_path: nia_hash::FastHashMap<Arc<SourcePath>, SourceId>,
    paths_by_id: Vec<Arc<SourcePath>>,
    next_id: u32,
}

impl SourceTable {
    /// Creates an empty source identity table.
    pub fn new() -> Self {
        Self::default()
    }

    /// Returns the existing id for a path or allocates the next id.
    pub fn id_for_path(&self, path: &SourcePath) -> SourceId {
        // Source tables are shared across query workers. Poisoning means a
        // worker panicked while mutating the id map, so continuing could assign
        // inconsistent SourceIds and break versioned node identity.
        let mut inner = self.inner.lock().expect("source table lock poisoned");
        if let Some(id) = inner.ids_by_path.get(path).copied() {
            return id;
        }

        let id = SourceId(inner.next_id);
        inner.next_id = inner
            .next_id
            .checked_add(1)
            .expect("source id space exhausted");
        let path = Arc::new(path.clone());
        inner.ids_by_path.insert(path.clone(), id);
        inner.paths_by_id.push(path);
        id
    }

    /// Looks up an id without allocating one for a missing path.
    pub fn existing_id_for_path(&self, path: &SourcePath) -> Option<SourceId> {
        self.inner
            .lock()
            .expect("source table lock poisoned")
            .ids_by_path
            .get(path)
            .copied()
    }

    /// Returns the logical path registered for an id.
    pub fn path_for_id(&self, id: SourceId) -> Option<Arc<SourcePath>> {
        self.inner
            .lock()
            .expect("source table lock poisoned")
            .paths_by_id
            .get(id.0 as usize)
            .cloned()
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
    pub fn id_for_path(&self, path: &SourcePath) -> SourceId {
        self.table.id_for_path(path)
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
        // A poisoned source database may contain a partially updated revision.
        // Treat that as process-level corruption rather than a recoverable
        // missing-source diagnostic.
        self.files
            .lock()
            .expect("source database lock poisoned")
            .get(&id)
            .cloned()
    }

    /// Returns a snapshot only when its current revision exactly matches.
    pub fn source_for_version(&self, version: SourceVersion) -> Option<SourceFile> {
        self.source_for_id(version.id)
            .filter(|file| file.revision == version.revision)
    }

    /// Returns all current snapshots in unspecified order.
    pub fn source_files(&self) -> Vec<SourceFile> {
        self.files
            .lock()
            .expect("source database lock poisoned")
            .values()
            .cloned()
            .collect()
    }

    /// Stores text, preserving its id and advancing an existing revision.
    pub fn set_source(&self, path: SourcePath, text: impl Into<Arc<str>>) -> SourceFile {
        let id = self.id_for_path(&path);
        // Holding this lock covers both revision selection and replacement; a
        // poisoned lock would make stale source-version queries indistinguishable
        // from valid older revisions.
        let mut files = self.files.lock().expect("source database lock poisoned");
        let revision = files
            .get(&id)
            .map(|file| file.revision.next())
            .unwrap_or(SourceRevision::INITIAL);
        let file = SourceFile::new(id, path, text).with_revision(revision);
        files.insert(id, file.clone());
        file
    }

    /// Returns a cached snapshot or reads and stores the source from disk.
    pub fn read_source(&self, path: &SourcePath) -> io::Result<SourceFile> {
        if let Some(file) = self.source_for_path(path) {
            return Ok(file);
        }

        let text = read_source_text(path.as_str())?;
        Ok(self.set_source(path.clone(), text))
    }

    /// Creates an unstored empty snapshot for a path at the initial revision.
    pub fn empty_source(&self, path: &SourcePath) -> SourceFile {
        SourceFile::new(self.id_for_path(path), path.clone(), "")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn source_revision_advances_monotonically() {
        assert_eq!(SourceRevision::INITIAL.next(), SourceRevision(1));
    }

    #[test]
    #[should_panic(expected = "source revision space exhausted")]
    fn source_revision_overflow_is_rejected() {
        SourceRevision(u64::MAX).next();
    }

    #[test]
    fn source_file_defaults_to_initial_revision() {
        let file = SourceFile::new(SourceId(7), SourcePath::new("main.nia"), "fn main() {}");

        assert_eq!(file.revision, SourceRevision::INITIAL);
        assert_eq!(
            file.version(),
            SourceVersion {
                id: SourceId(7),
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
            "/opt/nia/lib/nia/std/collections.nia",
            "toolchain:/std/collections.nia",
        );

        assert_eq!(path.as_str(), "/opt/nia/lib/nia/std/collections.nia");
        assert_eq!(
            path.identity().normalized_path(),
            "toolchain:/std/collections.nia"
        );
        assert_ne!(path, SourcePath::new(path.as_str()));
        assert_eq!(
            path,
            SourcePath::with_identity(
                "/relocated/lib/nia/std/collections.nia",
                "toolchain:/std/collections.nia",
            )
        );
    }

    #[test]
    fn source_table_reuses_path_ids() {
        let table = SourceTable::new();
        let main = SourcePath::new("main.nia");
        let defs = SourcePath::new("defs.nia");

        assert_eq!(table.id_for_path(&main), SourceId(0));
        assert_eq!(table.id_for_path(&defs), SourceId(1));
        assert_eq!(table.id_for_path(&main), SourceId(0));
        assert_eq!(table.path_for_id(SourceId(0)).as_deref(), Some(&main));
        assert_eq!(table.path_for_id(SourceId(1)).as_deref(), Some(&defs));
        assert_eq!(table.path_for_id(SourceId(2)), None);
    }

    #[test]
    fn source_database_stores_in_memory_sources() {
        let sources = SourceDatabase::new();
        let path = SourcePath::new("main.nia");

        let file = sources.set_source(path.clone(), "fn main() i32 { 0 }");

        assert_eq!(file.id, SourceId(0));
        assert_eq!(file.revision, SourceRevision::INITIAL);
        assert_eq!(sources.source_for_path(&path), Some(file));
    }

    #[test]
    fn source_database_path_lookup_does_not_allocate_missing_ids() {
        let sources = SourceDatabase::new();
        let missing = SourcePath::new("missing.nia");
        let main = SourcePath::new("main.nia");

        assert_eq!(sources.source_for_path(&missing), None);

        let file = sources.set_source(main.clone(), "fn main() i32 { 0 }");
        assert_eq!(file.id, SourceId(0));
        assert_eq!(sources.id_for_path(&missing), SourceId(1));
    }

    #[test]
    fn source_database_replacement_advances_revision() {
        let sources = SourceDatabase::new();
        let path = SourcePath::new("main.nia");

        let first = sources.set_source(path.clone(), "fn main() i32 { 0 }");
        let second = sources.set_source(path.clone(), "fn main() i32 { 1 }");

        assert_eq!(first.id, second.id);
        assert_eq!(first.revision, SourceRevision::INITIAL);
        assert_eq!(second.revision, SourceRevision(1));
        assert_eq!(second.text.as_ref(), "fn main() i32 { 1 }");
    }

    #[test]
    fn source_database_reads_sources_by_version() {
        let sources = SourceDatabase::new();
        let path = SourcePath::new("main.nia");

        let first = sources.set_source(path.clone(), "fn main() i32 { 0 }");
        let second = sources.set_source(path, "fn main() i32 { 1 }");

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
