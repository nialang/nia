//! Audits for repository-wide compatibility and standard-library ownership.
/// Compatibility identity and release-version audit.
pub mod compatibility;
/// Standard-library build-host closure audit.
pub mod std_build_host;
/// Toolchain system import descriptions against the platform sources.
pub mod system_imports;
