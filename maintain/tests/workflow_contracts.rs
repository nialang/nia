use std::fs;
use std::path::{Path, PathBuf};

fn repository_root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("..")
        .canonicalize()
        .unwrap()
}

fn workflow(name: &str) -> String {
    fs::read_to_string(repository_root().join(".github/workflows").join(name)).unwrap()
}

#[test]
fn build_std_workflow_runs_complete_rust_maintenance_and_correctness_gates() {
    let workflow = workflow("build-std.yml");
    for path in [
        ".cargo/**",
        "Cargo.lock",
        "Cargo.toml",
        "crates/**",
        "lib/**",
        "examples/**",
        "benchmarks/build/**",
        "maintain/**",
    ] {
        assert!(
            workflow.contains(&format!("      - \"{path}\"")),
            "missing trigger {path}"
        );
    }
    for command in [
        "cargo maintain check",
        "cargo maintain baseline build",
        "cargo fmt --all -- --check",
        "cargo clippy --workspace --all-targets -- -D warnings",
        "cargo check --workspace --all-targets",
        "cargo test --workspace",
        "cargo test -p nia-cli --test build_cases",
        "cargo test -p nia-cli --test toolchain_relocation",
    ] {
        assert!(workflow.contains(command), "missing command {command}");
    }
    // Clang is the reference for the target C calling conventions.
    assert!(workflow.contains("llvm-23-dev libpolly-23-dev lld-23 clang-23"));
    // aarch64 Linux executables run under user-mode QEMU.
    assert!(workflow.contains("qemu-user-static"));
    // Executables link with LLD by default, so its drivers must be on PATH.
    assert!(workflow.contains("echo \"/usr/lib/llvm-23/bin\" >> \"${GITHUB_PATH}\""));
    assert!(!workflow.contains("report[\"acceptance\"][\"passed\"]"));
    assert!(workflow.contains("actions/upload-artifact@v7"));
}

#[test]
fn build_std_workflow_runs_the_windows_static_llvm_matrix() {
    let workflow = workflow("build-std.yml");
    let build_script = fs::read_to_string(repository_root().join("tools/llvm/build-static.ps1"))
        .expect("read Windows LLVM build script");
    assert!(workflow.contains("name: Windows x64 correctness matrix"));
    assert!(workflow.contains("runs-on: windows-2022"));
    assert!(workflow.contains("tools/llvm/build-static.ps1"));
    assert!(workflow.contains("VsDevCmd.bat"));
    assert!(!workflow.contains("windows-env.ps1"));
    for command in [
        "cargo maintain check",
        "cargo fmt --all -- --check",
        "cargo clippy --workspace --all-targets --no-default-features --target x86_64-pc-windows-msvc -- -D warnings",
        "cargo test --workspace --no-default-features --target x86_64-pc-windows-msvc",
        "cargo test -p nia-cli --test diagnostic_failures --no-default-features --target x86_64-pc-windows-msvc",
        "cargo build --release -p nia-cli --no-default-features --target x86_64-pc-windows-msvc",
    ] {
        assert!(
            workflow.contains(command),
            "missing Windows command {command}"
        );
    }
    assert!(workflow.contains("LLVM_SYS_231_PREFIX"));
    assert!(workflow.contains("LLVM_ENABLE_PROJECTS: clang;lld"));
    assert!(workflow.contains("CLANG=$env:CLANG"));
    assert!(workflow.contains("target\\x86_64-pc-windows-msvc\\release\\nia.exe"));
    assert!(!workflow.contains("-Lnative="));
    assert!(workflow.contains("LLVM_INSTALL_ROOT"));
    assert!(workflow.contains("llvm-toolchain-noble-23"));
    assert!(build_script.contains("CMAKE_RC_COMPILER"));
    assert!(build_script.contains("CMAKE_MT"));
}

#[test]
fn performance_workflow_uses_rust_baselines_and_preserves_evidence() {
    let workflow = workflow("performance.yml");
    for command in [
        "cargo maintain check",
        "cargo maintain baseline compiler",
        "cargo maintain baseline compare",
        "--repeat 3",
        "--runner-class github-hosted-ubuntu-24.04-x64",
    ] {
        assert!(workflow.contains(command), "missing command {command}");
    }
    assert!(!workflow.contains("--allow-machine-mismatch"));
    assert!(workflow.contains("llvm-toolchain-noble-23"));
    assert!(workflow.contains("artifact_name=nia-perf-baseline"));
    assert!(workflow.contains("steps.comparison.outcome"));
    assert!(workflow.contains("name: nia-perf-candidate"));
    assert!(workflow.contains("retention-days: 90"));
}

#[test]
fn release_workflow_publishes_only_versioned_linux_archives() {
    let workflow = workflow("release.yml");
    assert!(workflow.contains("      - \"v*\""));
    assert!(workflow.contains("permissions:\n  contents: write"));
    assert!(workflow.contains("tools/llvm/build-static.sh"));
    assert!(workflow.contains("Validate formal release tag"));
    assert!(workflow.contains("^v[0-9]+\\.[0-9]+\\.0$"));
    assert!(workflow.contains("tools/release/package.sh"));
    assert!(workflow.contains("SHA256SUMS"));
    assert!(workflow.contains("gh release create"));
    assert!(workflow.contains("--verify-tag"));
    assert!(workflow.contains("--generate-notes"));
    assert!(!workflow.contains("workflow_dispatch"));
}

#[test]
fn workflows_have_no_parallel_python_or_node_toolchain() {
    for name in [
        "build-std.yml",
        "performance.yml",
        "platform.yml",
        "release.yml",
    ] {
        let workflow = workflow(name);
        for forbidden in [
            "setup-python",
            "setup-node",
            "python3",
            "npm",
            ".python-version",
            ".node-version",
            "package-lock.json",
        ] {
            assert!(
                !workflow.contains(forbidden),
                "{name} still contains {forbidden}"
            );
        }
    }
}

#[test]
fn platform_workflow_runs_both_darwin_execution_matrices() {
    let workflow = workflow("platform.yml");
    for runner in ["macos-15-intel", "macos-15"] {
        assert!(
            workflow.contains(&format!("runner: {runner}")),
            "missing {runner}"
        );
    }
    for target in ["x86_64-apple-macos", "aarch64-apple-macos"] {
        assert!(
            workflow.contains(&format!("target: {target}")),
            "missing {target}"
        );
    }
    for command in [
        "tools/llvm/build-static.sh",
        "LLVM_TARGETS_TO_BUILD: X86;AArch64",
        "cargo maintain check",
        "cargo fmt --all -- --check",
        "cargo clippy -p nia-cli --tests --no-default-features --features llvm-static -- -D warnings",
        "cargo test -p nia-cli --tests --no-default-features --features llvm-static",
        "actions/upload-artifact@v7",
    ] {
        assert!(workflow.contains(command), "missing command {command}");
    }
    assert!(workflow.contains("MACOSX_DEPLOYMENT_TARGET: \"15.0\""));
    assert!(workflow.contains("LLVM_ENABLE_PROJECTS: clang;lld"));
    assert!(workflow.contains("${{ env.LLVM_TARGETS_TO_BUILD }}-${{ env.LLVM_ENABLE_PROJECTS }}"));
    assert!(
        workflow.contains("CLANG: ${{ github.workspace }}/target/llvm-static/install/bin/clang")
    );
    assert!(workflow.contains("test -x \"${CLANG}\""));
    assert!(workflow.contains("ld64.lld"));
}

#[test]
fn platform_evidence_records_suite_outcome_and_run_identity() {
    let workflow = workflow("platform.yml");
    for field in [
        "id: darwin-suite",
        "printf 'suite_status=%s\\n' '${{ steps.darwin-suite.outcome }}'",
        "printf 'commit=%s\\n' \"${GITHUB_SHA}\"",
        "printf 'run_id=%s\\n' \"${GITHUB_RUN_ID}\"",
        "printf 'job=%s\\n' \"${GITHUB_JOB}\"",
        "printf 'workflow=%s\\n' \"${GITHUB_WORKFLOW}\"",
        "printf 'clang=%s\\n' \"$(\"${CLANG}\" --version | head -n 1)\"",
        "if: always()",
        "actions/upload-artifact@v7",
    ] {
        assert!(workflow.contains(field), "missing evidence field {field}");
    }
}
