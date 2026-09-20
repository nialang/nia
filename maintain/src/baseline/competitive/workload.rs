use std::path::{Path, PathBuf};

use super::schema::{OutputKind, ToolContract, WorkloadContract};
use super::{Language, Profile};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum Workload {
    MinimalCheck,
    HelloCheck,
    HelloExecutable,
}

impl Workload {
    pub(super) const ALL: [Self; 3] = [Self::MinimalCheck, Self::HelloCheck, Self::HelloExecutable];

    pub(super) fn parse(name: &str) -> Option<Self> {
        Self::ALL
            .into_iter()
            .find(|workload| workload.name() == name)
    }

    pub(super) const fn name(self) -> &'static str {
        match self {
            Self::MinimalCheck => "minimal_check",
            Self::HelloCheck => "hello_check",
            Self::HelloExecutable => "hello_executable",
        }
    }

    pub(super) const fn source_class(self) -> &'static str {
        match self {
            Self::MinimalCheck => "minimal entry point without standard-library use",
            Self::HelloCheck | Self::HelloExecutable => {
                "standard-library hello world with observable output"
            }
        }
    }

    pub(super) const fn source_relative(self, language: Language) -> &'static str {
        match (self, language) {
            (Self::MinimalCheck, Language::Nia) => "benchmarks/minimal.nia",
            (Self::MinimalCheck, Language::Rust) => "benchmarks/competitive/minimal.rs",
            (Self::MinimalCheck, Language::Zig) => "benchmarks/competitive/minimal.zig",
            (Self::HelloCheck | Self::HelloExecutable, Language::Nia) => "examples/hello.nia",
            (Self::HelloCheck | Self::HelloExecutable, Language::Rust) => {
                "benchmarks/competitive/hello.rs"
            }
            (Self::HelloCheck | Self::HelloExecutable, Language::Zig) => {
                "benchmarks/competitive/hello.zig"
            }
        }
    }

    pub(super) const fn output_kind(self, language: Language) -> OutputKind {
        match self {
            Self::HelloExecutable => OutputKind::Executable,
            Self::MinimalCheck | Self::HelloCheck if matches!(language, Language::Rust) => {
                OutputKind::Metadata
            }
            Self::MinimalCheck | Self::HelloCheck => OutputKind::None,
        }
    }

    pub(super) const fn expected_output(self, language: Language) -> Option<&'static str> {
        match (self, language) {
            (Self::HelloExecutable, Language::Nia) => Some("hello from nia"),
            (Self::HelloExecutable, Language::Rust) => Some("hello from rust"),
            (Self::HelloExecutable, Language::Zig) => Some("hello from zig"),
            _ => None,
        }
    }

    pub(super) fn contract(self) -> WorkloadContract {
        let tool = |language, mode, comparability| ToolContract {
            language,
            mode,
            output: self.output_kind(language),
            comparability,
        };
        let tools = match self {
            Self::MinimalCheck | Self::HelloCheck => vec![
                tool(
                    Language::Nia,
                    "nia check",
                    "full Nia parse, semantic, and reachable-body check without native output",
                ),
                tool(
                    Language::Rust,
                    "rustc --emit=metadata",
                    "rustc has no direct no-output check mode; metadata emission is the mode used by check-style compilation",
                ),
                tool(
                    Language::Zig,
                    "zig build-exe -fno-emit-bin",
                    "full Zig compile pipeline with binary emission disabled; zig ast-check is intentionally excluded because it omits semantic analysis",
                ),
            ],
            Self::HelloExecutable => vec![
                tool(
                    Language::Nia,
                    "nia emit --exe",
                    "native host executable using Nia's standard library and runtime",
                ),
                tool(
                    Language::Rust,
                    "rustc --emit=link",
                    "native host executable using Rust's distributed standard library",
                ),
                tool(
                    Language::Zig,
                    "zig build-exe",
                    "native host executable using Zig's source-distributed standard library",
                ),
            ],
        };
        WorkloadContract {
            name: self.name(),
            source_class: self.source_class(),
            tools,
        }
    }
}

pub(super) struct Programs<'a> {
    pub(super) nia: &'a Path,
    pub(super) rustc: &'a Path,
    pub(super) zig: &'a Path,
    pub(super) resource_root: &'a Path,
}

pub(super) fn command(
    programs: &Programs<'_>,
    workload: Workload,
    language: Language,
    profile: Profile,
    source: &Path,
    output: &Path,
    cache: &Path,
) -> Vec<String> {
    let path = |value: &Path| value.to_string_lossy().into_owned();
    match language {
        Language::Nia => {
            let mut command = vec![
                path(programs.nia),
                "--resource-root".to_owned(),
                path(programs.resource_root),
                "--profile".to_owned(),
                profile.nia_profile().to_owned(),
                profile.nia_optimization().to_owned(),
            ];
            match workload {
                Workload::MinimalCheck | Workload::HelloCheck => {
                    command.extend([
                        "check".to_owned(),
                        path(source),
                        "--cache-dir".to_owned(),
                        path(cache),
                    ]);
                }
                Workload::HelloExecutable => {
                    command.extend([
                        "emit".to_owned(),
                        "--exe".to_owned(),
                        path(source),
                        "-o".to_owned(),
                        path(output),
                        "--cache-dir".to_owned(),
                        path(cache),
                    ]);
                }
            }
            command
        }
        Language::Rust => {
            let mut command = vec![
                path(programs.rustc),
                "--edition=2024".to_owned(),
                "--crate-name=competitive_fixture".to_owned(),
                "--color=never".to_owned(),
                "-C".to_owned(),
                profile.rust_optimization().to_owned(),
                "-C".to_owned(),
                "debuginfo=0".to_owned(),
                path(source),
                "-o".to_owned(),
                path(output),
            ];
            command.push(match workload {
                Workload::MinimalCheck | Workload::HelloCheck => "--emit=metadata".to_owned(),
                Workload::HelloExecutable => "--emit=link".to_owned(),
            });
            command
        }
        Language::Zig => {
            let mut command = vec![
                path(programs.zig),
                "build-exe".to_owned(),
                path(source),
                "-fno-incremental".to_owned(),
                "--cache-dir".to_owned(),
                path(cache),
                "-O".to_owned(),
                profile.zig_optimization().to_owned(),
            ];
            match workload {
                Workload::MinimalCheck | Workload::HelloCheck => {
                    command.push("-fno-emit-bin".to_owned());
                }
                Workload::HelloExecutable => {
                    command.push(format!("-femit-bin={}", path(output)));
                }
            }
            command
        }
    }
}

pub(super) fn normalized_command(command: &[String], root: &Path, workspace: &Path) -> Vec<String> {
    let replace = |value: &str, prefix: &Path, label: &str| {
        value.replace(&prefix.to_string_lossy().to_string(), label)
    };
    command
        .iter()
        .map(|value| replace(&replace(value, workspace, "$WORKSPACE"), root, "$REPO"))
        .collect()
}

pub(super) fn output_path(workspace: &Path, kind: OutputKind) -> PathBuf {
    match kind {
        OutputKind::None => workspace.join("main"),
        OutputKind::Metadata => workspace.join("output.rmeta"),
        OutputKind::Executable => workspace.join("hello"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn check_contracts_do_not_claim_ast_check_equivalence() {
        let contract = Workload::HelloCheck.contract();
        let zig = contract
            .tools
            .iter()
            .find(|tool| tool.language == Language::Zig)
            .unwrap();
        assert_eq!(zig.mode, "zig build-exe -fno-emit-bin");
        assert!(zig.comparability.contains("semantic analysis"));
    }

    #[test]
    fn normalization_hides_ephemeral_paths() {
        let root = Path::new("/repo");
        let workspace = Path::new("/tmp/sample");
        assert_eq!(
            normalized_command(
                &["/repo/nia".to_owned(), "/tmp/sample/main.nia".to_owned()],
                root,
                workspace,
            ),
            ["$REPO/nia", "$WORKSPACE/main.nia"]
        );
    }
}
