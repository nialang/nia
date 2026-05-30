// SPDX-License-Identifier: GPL-3.0-or-later

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

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct SourcePath(String);

impl SourcePath {
    pub fn new(path: impl Into<String>) -> Self {
        Self(path.into())
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SourceFile {
    pub id: SourceId,
    pub path: SourcePath,
    pub revision: SourceRevision,
    pub text: String,
}

impl SourceFile {
    pub fn new(id: SourceId, path: SourcePath, text: String) -> Self {
        Self {
            id,
            path,
            revision: SourceRevision::INITIAL,
            text,
        }
    }

    pub fn with_revision(mut self, revision: SourceRevision) -> Self {
        self.revision = revision;
        self
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
        let file = SourceFile::new(
            SourceId(7),
            SourcePath::new("main.nia"),
            "fn main() {}".into(),
        );

        assert_eq!(file.revision, SourceRevision::INITIAL);
        assert_eq!(file.path.as_str(), "main.nia");
    }
}
