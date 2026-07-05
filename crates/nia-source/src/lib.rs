// SPDX-License-Identifier: GPL-3.0-or-later
use std::{
    collections::HashMap,
    fs, io,
    sync::{Arc, Mutex},
};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct SourceId(pub u32);

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct SourceRevision(pub u64);

impl SourceRevision {
    pub const INITIAL: Self = Self(0);

    pub const fn next(self) -> Self {
        Self(self.0 + 1)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct SourceVersion {
    pub id: SourceId,
    pub revision: SourceRevision,
}

#[derive(Debug, Clone, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct SourceIdentity {
    normalized_path: String,
}

impl SourceIdentity {
    pub fn new(path: impl AsRef<str>) -> Self {
        Self {
            normalized_path: normalize_path(path.as_ref()),
        }
    }

    pub fn from_path(path: &SourcePath) -> Self {
        Self::new(path.as_str())
    }

    pub fn normalized_path(&self) -> &str {
        &self.normalized_path
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct SourcePath(String);

impl SourcePath {
    pub fn new(path: impl Into<String>) -> Self {
        Self(normalize_path(&path.into()))
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }

    pub fn identity(&self) -> SourceIdentity {
        SourceIdentity::from_path(self)
    }
}

pub fn normalize_path(path: &str) -> String {
    let absolute = path.starts_with('/');
    let mut parts = Vec::new();
    for part in path.split('/') {
        match part {
            "" | "." => {}
            ".." => {
                parts.pop();
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
pub struct SourceFile {
    pub id: SourceId,
    pub path: SourcePath,
    pub revision: SourceRevision,
    pub text: Arc<str>,
}

impl SourceFile {
    pub fn new(id: SourceId, path: SourcePath, text: impl Into<Arc<str>>) -> Self {
        Self {
            id,
            path,
            revision: SourceRevision::INITIAL,
            text: text.into(),
        }
    }

    pub fn with_revision(mut self, revision: SourceRevision) -> Self {
        self.revision = revision;
        self
    }

    pub fn version(&self) -> SourceVersion {
        SourceVersion {
            id: self.id,
            revision: self.revision,
        }
    }
}

#[derive(Debug, Clone, Default)]
pub struct SourceTable {
    inner: Arc<Mutex<SourceTableInner>>,
}

#[derive(Debug, Default)]
struct SourceTableInner {
    paths: HashMap<SourcePath, SourceId>,
    next_id: u32,
}

impl SourceTable {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn id_for_path(&self, path: &SourcePath) -> SourceId {
        // Source tables are shared across query workers. Poisoning means a
        // worker panicked while mutating the id map, so continuing could assign
        // inconsistent SourceIds and break versioned node identity.
        let mut inner = self.inner.lock().expect("source table lock poisoned");
        if let Some(id) = inner.paths.get(path).copied() {
            return id;
        }

        let id = SourceId(inner.next_id);
        inner.next_id = inner
            .next_id
            .checked_add(1)
            .expect("source id space exhausted");
        inner.paths.insert(path.clone(), id);
        id
    }

    pub fn existing_id_for_path(&self, path: &SourcePath) -> Option<SourceId> {
        self.inner
            .lock()
            .expect("source table lock poisoned")
            .paths
            .get(path)
            .copied()
    }
}

#[derive(Debug, Clone, Default)]
pub struct SourceDatabase {
    table: SourceTable,
    files: Arc<Mutex<HashMap<SourceId, SourceFile>>>,
}

impl SourceDatabase {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn id_for_path(&self, path: &SourcePath) -> SourceId {
        self.table.id_for_path(path)
    }

    pub fn source_for_path(&self, path: &SourcePath) -> Option<SourceFile> {
        let id = self.table.existing_id_for_path(path)?;
        self.source_for_id(id)
    }

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

    pub fn source_for_version(&self, version: SourceVersion) -> Option<SourceFile> {
        self.source_for_id(version.id)
            .filter(|file| file.revision == version.revision)
    }

    pub fn source_files(&self) -> Vec<SourceFile> {
        self.files
            .lock()
            .expect("source database lock poisoned")
            .values()
            .cloned()
            .collect()
    }

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

    pub fn read_source(&self, path: &SourcePath) -> io::Result<SourceFile> {
        if let Some(file) = self.source_for_path(path) {
            return Ok(file);
        }

        let text = fs::read_to_string(path.as_str())?;
        Ok(self.set_source(path.clone(), text))
    }

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
    fn source_table_reuses_path_ids() {
        let table = SourceTable::new();
        let main = SourcePath::new("main.nia");
        let defs = SourcePath::new("defs.nia");

        assert_eq!(table.id_for_path(&main), SourceId(0));
        assert_eq!(table.id_for_path(&defs), SourceId(1));
        assert_eq!(table.id_for_path(&main), SourceId(0));
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
}
