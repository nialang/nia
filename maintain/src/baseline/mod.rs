//! Repeatable performance and representative-build evidence collectors.

use serde::Serialize;

/// Representative build baseline collection.
pub mod build;
/// Comparison of compiler performance baselines.
pub mod compare;
/// Cross-toolchain compiler performance baseline collection.
pub mod competitive;
/// Compiler workload performance baseline collection.
pub mod compiler;
mod synthetic;

pub(crate) fn repository_compiler(root: &std::path::Path) -> std::path::PathBuf {
    root.join("target/release")
        .join(format!("nia{}", std::env::consts::EXE_SUFFIX))
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize)]
#[serde(rename_all = "snake_case")]
/// Language toolchain represented by baseline workloads.
pub enum Language {
    Nia,
    Rust,
    Zig,
}

impl Language {
    const fn extension(self) -> &'static str {
        match self {
            Self::Nia => "nia",
            Self::Rust => "rs",
            Self::Zig => "zig",
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn baselines_select_the_native_cargo_executable() {
        let directory = crate::test_support::TestDirectory::new("baseline-compiler");
        let filename = if cfg!(windows) { "nia.exe" } else { "nia" };
        let expected = directory.path().join("target/release").join(filename);
        directory.write(&format!("target/release/{filename}"), "compiler");
        for compiler in [
            build::Options::for_repository(directory.path()).nia,
            compiler::Options::for_repository(directory.path()).compiler,
            competitive::Options::for_repository(directory.path()).nia,
        ] {
            assert_eq!(compiler, expected);
            assert!(compiler.is_file());
        }
    }
}
