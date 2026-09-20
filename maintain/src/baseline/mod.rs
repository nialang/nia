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
