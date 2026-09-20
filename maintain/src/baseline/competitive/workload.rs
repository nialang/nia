use std::path::{Path, PathBuf};

use super::schema::{OutputKind, SampleState, SyntheticContract, ToolContract, WorkloadContract};
use super::synthetic;
use super::{Language, Profile};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum Workload {
    MinimalCheck,
    HelloCheck,
    HelloExecutable,
    EmptyBuild,
    HelloBuild,
    Synthetic10Modules,
    Synthetic50Modules,
    Synthetic100Modules,
    Synthetic500Modules,
    Synthetic100ModulesBuild,
}

impl Workload {
    pub(super) const ALL: [Self; 10] = [
        Self::MinimalCheck,
        Self::HelloCheck,
        Self::HelloExecutable,
        Self::EmptyBuild,
        Self::HelloBuild,
        Self::Synthetic10Modules,
        Self::Synthetic50Modules,
        Self::Synthetic100Modules,
        Self::Synthetic500Modules,
        Self::Synthetic100ModulesBuild,
    ];

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
            Self::EmptyBuild => "empty_build",
            Self::HelloBuild => "hello_build",
            Self::Synthetic10Modules => "synthetic_10_modules",
            Self::Synthetic50Modules => "synthetic_50_modules",
            Self::Synthetic100Modules => "synthetic_100_modules",
            Self::Synthetic500Modules => "synthetic_500_modules",
            Self::Synthetic100ModulesBuild => "synthetic_100_modules_build",
        }
    }

    pub(super) const fn is_build(self) -> bool {
        matches!(
            self,
            Self::EmptyBuild | Self::HelloBuild | Self::Synthetic100ModulesBuild
        )
    }

    pub(super) const fn states(self) -> &'static [SampleState] {
        match self {
            Self::Synthetic100ModulesBuild => &[
                SampleState::Clean,
                SampleState::NoOpWarm,
                SampleState::LeafEdit,
            ],
            _ => &[SampleState::Clean],
        }
    }

    pub(super) const fn languages(self) -> &'static [Language] {
        match self {
            Self::EmptyBuild => &[Language::Nia, Language::Zig],
            _ => &[Language::Nia, Language::Rust, Language::Zig],
        }
    }

    pub(super) const fn source_class(self) -> &'static str {
        match self {
            Self::MinimalCheck => "minimal entry point without standard-library use",
            Self::HelloCheck | Self::HelloExecutable => {
                "standard-library hello world with observable output"
            }
            Self::EmptyBuild => "empty build graph or closest available coordinator-only mode",
            Self::HelloBuild => "standard-library hello world through the native build system",
            Self::Synthetic10Modules
            | Self::Synthetic50Modules
            | Self::Synthetic100Modules
            | Self::Synthetic500Modules => {
                "generated flat/star native executable with controlled frontend and reachable code"
            }
            Self::Synthetic100ModulesBuild => {
                "generated medium flat/star executable through native build systems with ordered clean, no-op warm, and one-leaf-edit states"
            }
        }
    }

    pub(super) const fn synthetic_modules(self) -> Option<usize> {
        match self {
            Self::Synthetic10Modules => Some(10),
            Self::Synthetic50Modules => Some(50),
            Self::Synthetic100Modules => Some(100),
            Self::Synthetic500Modules => Some(500),
            Self::Synthetic100ModulesBuild => Some(100),
            _ => None,
        }
    }

    pub(super) const fn is_synthetic_build(self) -> bool {
        matches!(self, Self::Synthetic100ModulesBuild)
    }

    pub(super) const fn source_descriptor(self, language: Language) -> &'static str {
        match self {
            Self::Synthetic10Modules => "generated:competitive_synthetic/10-leaf-modules",
            Self::Synthetic50Modules => "generated:competitive_synthetic/50-leaf-modules",
            Self::Synthetic100Modules => "generated:competitive_synthetic/100-leaf-modules",
            Self::Synthetic500Modules => "generated:competitive_synthetic/500-leaf-modules",
            Self::Synthetic100ModulesBuild => {
                "generated:competitive_synthetic/100-leaf-modules-native-build"
            }
            _ => match self.source_relative(language) {
                Some(relative) => relative,
                None => unreachable!(),
            },
        }
    }

    pub(super) const fn source_relative(self, language: Language) -> Option<&'static str> {
        match (self, language) {
            (Self::MinimalCheck, Language::Nia) => Some("benchmarks/minimal.nia"),
            (Self::MinimalCheck, Language::Rust) => Some("benchmarks/competitive/minimal.rs"),
            (Self::MinimalCheck, Language::Zig) => Some("benchmarks/competitive/minimal.zig"),
            (Self::HelloCheck | Self::HelloExecutable, Language::Nia) => Some("examples/hello.nia"),
            (Self::HelloCheck | Self::HelloExecutable, Language::Rust) => {
                Some("benchmarks/competitive/hello.rs")
            }
            (Self::HelloCheck | Self::HelloExecutable, Language::Zig) => {
                Some("benchmarks/competitive/hello.zig")
            }
            (Self::EmptyBuild, Language::Nia) => Some("benchmarks/competitive/build/nia-empty"),
            (Self::EmptyBuild, Language::Rust) => panic!("Cargo empty-build mode is unavailable"),
            (Self::EmptyBuild, Language::Zig) => Some("benchmarks/competitive/build/zig-empty"),
            (Self::HelloBuild, Language::Nia) => Some("benchmarks/competitive/build/nia-hello"),
            (Self::HelloBuild, Language::Rust) => Some("benchmarks/competitive/build/cargo-hello"),
            (Self::HelloBuild, Language::Zig) => Some("benchmarks/competitive/build/zig-hello"),
            (
                Self::Synthetic10Modules
                | Self::Synthetic50Modules
                | Self::Synthetic100Modules
                | Self::Synthetic500Modules
                | Self::Synthetic100ModulesBuild,
                _,
            ) => None,
        }
    }

    pub(super) const fn output_kind(self, language: Language) -> OutputKind {
        match self {
            Self::HelloExecutable
            | Self::Synthetic10Modules
            | Self::Synthetic50Modules
            | Self::Synthetic100Modules
            | Self::Synthetic500Modules
            | Self::Synthetic100ModulesBuild => OutputKind::Executable,
            Self::HelloBuild => OutputKind::Executable,
            Self::EmptyBuild => OutputKind::None,
            Self::MinimalCheck | Self::HelloCheck if matches!(language, Language::Rust) => {
                OutputKind::Metadata
            }
            Self::MinimalCheck | Self::HelloCheck => OutputKind::None,
        }
    }

    pub(super) const fn expected_output(
        self,
        language: Language,
        state: SampleState,
    ) -> Option<&'static str> {
        match (self, language, state) {
            (Self::HelloExecutable | Self::HelloBuild, Language::Nia, _) => Some("hello from nia"),
            (Self::HelloExecutable | Self::HelloBuild, Language::Rust, _) => {
                Some("hello from rust")
            }
            (Self::HelloExecutable | Self::HelloBuild, Language::Zig, _) => Some("hello from zig"),
            (
                Self::Synthetic10Modules
                | Self::Synthetic50Modules
                | Self::Synthetic100Modules
                | Self::Synthetic500Modules,
                _,
                _,
            ) => Some("synthetic-ok"),
            (Self::Synthetic100ModulesBuild, _, SampleState::Clean | SampleState::NoOpWarm) => {
                Some("synthetic-ok")
            }
            (Self::Synthetic100ModulesBuild, _, SampleState::LeafEdit) => Some("synthetic-edited"),
            _ => None,
        }
    }

    pub(super) fn contract(self) -> WorkloadContract {
        let tool = |language, mode, comparability| ToolContract {
            language,
            available: true,
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
            Self::EmptyBuild => vec![
                tool(
                    Language::Nia,
                    "nia build",
                    "compiles and runs build.nia, then executes an empty aggregate graph",
                ),
                ToolContract {
                    language: Language::Rust,
                    available: false,
                    mode: "unavailable",
                    output: OutputKind::None,
                    comparability: "Cargo has no build-script-only empty graph mode, and rejects a zero-member virtual workspace; substituting an empty library would add non-equivalent crate compilation",
                },
                tool(
                    Language::Zig,
                    "zig build (empty graph)",
                    "compiles and runs build.zig with no scheduled artifact steps",
                ),
            ],
            Self::HelloBuild => vec![
                tool(
                    Language::Nia,
                    "nia build",
                    "compiles and runs build.nia, then emits the native Hello executable",
                ),
                tool(
                    Language::Rust,
                    "cargo build",
                    "builds the native Hello package through Cargo",
                ),
                tool(
                    Language::Zig,
                    "zig build",
                    "compiles and runs build.zig, then installs the native Hello executable",
                ),
            ],
            Self::Synthetic10Modules
            | Self::Synthetic50Modules
            | Self::Synthetic100Modules
            | Self::Synthetic500Modules => vec![
                tool(
                    Language::Nia,
                    "nia emit --exe",
                    "native executable from a generated Nia module tree with every leaf reachable",
                ),
                tool(
                    Language::Rust,
                    "rustc --emit=link",
                    "native executable from the equivalent generated Rust module tree with every leaf reachable",
                ),
                tool(
                    Language::Zig,
                    "zig build-exe",
                    "native executable from the equivalent generated Zig module tree with every leaf reachable",
                ),
            ],
            Self::Synthetic100ModulesBuild => vec![
                tool(
                    Language::Nia,
                    "nia build",
                    "ordered clean, no-op warm, and one-leaf-edit native executable builds in one generated project workspace",
                ),
                tool(
                    Language::Rust,
                    "cargo build",
                    "ordered clean, no-op warm, and one-leaf-edit native executable builds in one generated Cargo workspace",
                ),
                tool(
                    Language::Zig,
                    "zig build",
                    "ordered clean, no-op warm, and one-leaf-edit native executable builds in one generated Zig workspace",
                ),
            ],
        };
        WorkloadContract {
            name: self.name(),
            source_class: self.source_class(),
            synthetic: self.synthetic_modules().map(|leaf_modules| SyntheticContract {
                generator: synthetic::GENERATOR_IDENTITY,
                leaf_modules,
                graph: "one executable root with a flat/star dependency on every leaf module",
                per_module_work: "one compile-time-derived constant, one module-local generic identity instance, and one reachable value function",
            }),
            states: self.states(),
            tools,
        }
    }
}

pub(super) struct Programs<'a> {
    pub(super) nia: &'a Path,
    pub(super) rustc: &'a Path,
    pub(super) cargo: &'a Path,
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
    if workload.is_build() {
        return match language {
            Language::Nia => vec![
                path(programs.nia),
                "--resource-root".to_owned(),
                path(programs.resource_root),
                "--profile".to_owned(),
                profile.nia_profile().to_owned(),
                profile.nia_optimization().to_owned(),
                "build".to_owned(),
                "--root".to_owned(),
                path(source),
            ],
            Language::Rust => {
                let mut command = vec![
                    path(programs.cargo),
                    "build".to_owned(),
                    "--manifest-path".to_owned(),
                    path(&source.join("Cargo.toml")),
                    "--target-dir".to_owned(),
                    path(cache),
                    "--offline".to_owned(),
                    "--color=never".to_owned(),
                    "--quiet".to_owned(),
                ];
                if matches!(profile, Profile::Release) {
                    command.push("--release".to_owned());
                }
                command
            }
            Language::Zig => vec![
                path(programs.zig),
                "build".to_owned(),
                "--build-file".to_owned(),
                path(&source.join("build.zig")),
                "--cache-dir".to_owned(),
                path(cache),
                format!("-Doptimize={}", profile.zig_optimization()),
            ],
        };
    }
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
                Workload::HelloExecutable
                | Workload::Synthetic10Modules
                | Workload::Synthetic50Modules
                | Workload::Synthetic100Modules
                | Workload::Synthetic500Modules => {
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
                Workload::EmptyBuild
                | Workload::HelloBuild
                | Workload::Synthetic100ModulesBuild => unreachable!(),
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
                Workload::HelloExecutable
                | Workload::Synthetic10Modules
                | Workload::Synthetic50Modules
                | Workload::Synthetic100Modules
                | Workload::Synthetic500Modules => "--emit=link".to_owned(),
                Workload::EmptyBuild
                | Workload::HelloBuild
                | Workload::Synthetic100ModulesBuild => unreachable!(),
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
                Workload::HelloExecutable
                | Workload::Synthetic10Modules
                | Workload::Synthetic50Modules
                | Workload::Synthetic100Modules
                | Workload::Synthetic500Modules => {
                    command.push(format!("-femit-bin={}", path(output)));
                }
                Workload::EmptyBuild
                | Workload::HelloBuild
                | Workload::Synthetic100ModulesBuild => unreachable!(),
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

pub(super) fn output_path(
    workspace: &Path,
    workload: Workload,
    language: Language,
    profile: Profile,
    kind: OutputKind,
) -> PathBuf {
    if workload.is_build() {
        return match language {
            Language::Nia => workspace.join(if workload.is_synthetic_build() {
                ".nia-build/synthetic"
            } else {
                ".nia-build/hello"
            }),
            Language::Rust => workspace.join(match (workload.is_synthetic_build(), profile) {
                (true, Profile::Development) => "target/debug/competitive-synthetic",
                (true, Profile::Release) => "target/release/competitive-synthetic",
                (false, Profile::Development) => "target/debug/competitive-hello",
                (false, Profile::Release) => "target/release/competitive-hello",
            }),
            Language::Zig => workspace.join(if workload.is_synthetic_build() {
                "zig-out/bin/synthetic"
            } else {
                "zig-out/bin/hello"
            }),
        };
    }
    match kind {
        OutputKind::None => workspace.join("main"),
        OutputKind::Metadata => workspace.join("output.rmeta"),
        OutputKind::Executable => workspace.join("hello"),
    }
}

pub(super) fn project_product_paths(
    workspace: &Path,
    workload: Workload,
    language: Language,
) -> Vec<PathBuf> {
    if !workload.is_build() {
        return vec![workspace.join("project-cache")];
    }
    match language {
        Language::Nia => vec![workspace.join(".nia-build"), workspace.join(".nia-cache")],
        Language::Rust => vec![workspace.join("target")],
        Language::Zig => vec![workspace.join(".zig-cache"), workspace.join("zig-out")],
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

    #[test]
    fn empty_cargo_contract_is_explicitly_not_runner_equivalent() {
        let contract = Workload::EmptyBuild.contract();
        let rust = contract
            .tools
            .iter()
            .find(|tool| tool.language == Language::Rust)
            .unwrap();
        assert!(!rust.available);
        assert!(rust.comparability.contains("no build-script-only"));
        assert_eq!(rust.output, OutputKind::None);
    }

    #[test]
    fn synthetic_scales_share_one_explicit_generator_contract() {
        let counts = [10, 50, 100, 500];
        for (workload, expected) in Workload::ALL[5..9].iter().zip(counts) {
            let contract = workload.contract();
            let synthetic = contract.synthetic.unwrap();
            assert_eq!(synthetic.generator, synthetic::GENERATOR_IDENTITY);
            assert_eq!(synthetic.leaf_modules, expected);
            assert_eq!(contract.tools.len(), 3);
            assert!(contract.tools.iter().all(|tool| tool.available));
            assert!(
                contract
                    .tools
                    .iter()
                    .all(|tool| tool.output == OutputKind::Executable)
            );
        }
    }

    #[test]
    fn synthetic_build_uses_one_ordered_incremental_sequence() {
        let workload = Workload::Synthetic100ModulesBuild;
        assert!(workload.is_build());
        assert_eq!(workload.synthetic_modules(), Some(100));
        assert_eq!(
            workload.states(),
            &[
                SampleState::Clean,
                SampleState::NoOpWarm,
                SampleState::LeafEdit,
            ]
        );
        assert_eq!(
            workload.expected_output(Language::Nia, SampleState::LeafEdit),
            Some("synthetic-edited")
        );
    }
}
