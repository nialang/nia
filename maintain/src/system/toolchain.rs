use std::env;
use std::path::{Path, PathBuf};
use std::process::Command;

use serde::Serialize;

use crate::MaintainResult;

const LLVM_SYS_PREFIX_ENV: &str = "LLVM_SYS_221_PREFIX";

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
/// One executable and the version output observed by the baseline process.
pub struct ToolIdentity {
    /// Program requested by repository policy or the environment.
    pub requested: String,
    /// Resolved canonical executable path.
    pub path: String,
    /// Complete trimmed version output.
    pub version: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
/// Git source identity from which the measured compiler is built.
pub struct SourceIdentity {
    /// Full repository commit hash.
    pub revision: String,
    /// Whether non-ignored repository changes were present.
    pub dirty: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
/// External tool identities required to reproduce a compiler measurement.
pub struct ToolchainIdentity {
    /// Repository source state.
    pub source: SourceIdentity,
    /// Rust compiler used to build Nia.
    pub rustc: ToolIdentity,
    /// Cargo used to build Nia and the maintenance runner.
    pub cargo: ToolIdentity,
    /// LLVM configuration selected for the Nia build.
    pub llvm_config: ToolIdentity,
    /// Executable linker selected by Nia for native outputs.
    pub linker: ToolIdentity,
}

fn resolve_program(program: &Path) -> MaintainResult<PathBuf> {
    if program.is_absolute() || program.components().count() > 1 {
        if !program.is_file() {
            return Err(format!("executable does not exist: {}", program.display()));
        }
        return if program.is_absolute() {
            Ok(program.to_path_buf())
        } else {
            env::current_dir()
                .map(|directory| directory.join(program))
                .map_err(|error| format!("failed to resolve {}: {error}", program.display()))
        };
    }
    let path = env::var_os("PATH").ok_or_else(|| "PATH is not set".to_owned())?;
    for directory in env::split_paths(&path) {
        let candidate = directory.join(program);
        if candidate.is_file() {
            return Ok(candidate);
        }
    }
    Err(format!("failed to find executable {:?} in PATH", program))
}

fn command_output(
    program: &Path,
    arguments: &[&str],
    current_dir: Option<&Path>,
) -> MaintainResult<String> {
    let mut command = Command::new(program);
    command.args(arguments);
    if let Some(current_dir) = current_dir {
        command.current_dir(current_dir);
    }
    let output = command
        .output()
        .map_err(|error| format!("failed to run {}: {error}", program.display()))?;
    if !output.status.success() {
        return Err(format!(
            "{} returned {} while reporting its identity",
            program.display(),
            output.status
        ));
    }
    let mut value = String::from_utf8_lossy(&output.stdout).trim().to_owned();
    let stderr = String::from_utf8_lossy(&output.stderr);
    let stderr = stderr.trim();
    if !stderr.is_empty() {
        if !value.is_empty() {
            value.push('\n');
        }
        value.push_str(stderr);
    }
    if value.is_empty() {
        return Err(format!(
            "{} produced empty identity output",
            program.display()
        ));
    }
    Ok(value)
}

fn tool_identity(program: impl Into<PathBuf>, arguments: &[&str]) -> MaintainResult<ToolIdentity> {
    let requested = program.into();
    let path = resolve_program(&requested)?;
    Ok(ToolIdentity {
        requested: requested.to_string_lossy().into_owned(),
        version: command_output(&path, arguments, None)?,
        path: path.to_string_lossy().into_owned(),
    })
}

fn llvm_config_program() -> PathBuf {
    env::var_os(LLVM_SYS_PREFIX_ENV)
        .map(|prefix| PathBuf::from(prefix).join("bin/llvm-config"))
        .unwrap_or_else(|| PathBuf::from("llvm-config"))
}

fn source_identity(root: &Path) -> MaintainResult<SourceIdentity> {
    let revision = command_output(Path::new("git"), &["rev-parse", "HEAD"], Some(root))?;
    let output = Command::new("git")
        .args(["status", "--porcelain"])
        .current_dir(root)
        .output()
        .map_err(|error| format!("failed to inspect repository state: {error}"))?;
    if !output.status.success() {
        return Err(format!(
            "git status returned {} while inspecting repository state",
            output.status
        ));
    }
    Ok(SourceIdentity {
        revision,
        dirty: !output.stdout.is_empty(),
    })
}

/// Captures source, Rust, LLVM, and linker identities for a performance report.
pub fn toolchain_identity(root: &Path) -> MaintainResult<ToolchainIdentity> {
    let linker = env::var_os("NIA_LINKER")
        .filter(|value| !value.is_empty())
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from("ld"));
    Ok(ToolchainIdentity {
        source: source_identity(root)?,
        rustc: tool_identity("rustc", &["--version", "--verbose"])?,
        cargo: tool_identity("cargo", &["--version"])?,
        llvm_config: tool_identity(llvm_config_program(), &["--version"])?,
        linker: tool_identity(linker, &["--version"])?,
    })
}

/// Captures the measured Nia executable identity.
pub fn compiler_identity(compiler: &Path) -> MaintainResult<ToolIdentity> {
    tool_identity(compiler, &["--version"])
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn resolves_current_test_executable_with_identity_output() {
        let executable = std::env::current_exe().unwrap();
        let identity = tool_identity(&executable, &["--list"]).unwrap();
        assert_eq!(Path::new(&identity.path), executable);
        assert!(!identity.version.is_empty());
    }

    #[test]
    fn preserves_rustup_proxy_entrypoints() {
        let rustc = tool_identity("rustc", &["--version", "--verbose"]).unwrap();
        let cargo = tool_identity("cargo", &["--version"]).unwrap();
        assert!(rustc.version.starts_with("rustc "), "{}", rustc.version);
        assert!(cargo.version.starts_with("cargo "), "{}", cargo.version);
    }
}
