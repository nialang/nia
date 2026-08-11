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
        "cargo clippy --workspace --all-targets --all-features -- -D warnings",
        "cargo check --workspace --all-targets --all-features",
        "cargo test --workspace --all-features",
        "cargo test -p nia-cli --test build_cases",
        "cargo test -p nia-cli --test toolchain_relocation",
    ] {
        assert!(workflow.contains(command), "missing command {command}");
    }
    assert!(!workflow.contains("report[\"acceptance\"][\"passed\"]"));
    assert!(workflow.contains("actions/upload-artifact@v7"));
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
    assert!(workflow.contains("artifact_name=nia-perf-baseline"));
    assert!(workflow.contains("steps.comparison.outcome"));
    assert!(workflow.contains("name: nia-perf-candidate"));
    assert!(workflow.contains("retention-days: 90"));
}

#[test]
fn workflows_have_no_parallel_python_or_node_toolchain() {
    for name in ["build-std.yml", "performance.yml"] {
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
