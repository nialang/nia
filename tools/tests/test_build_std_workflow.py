import unittest
from pathlib import Path
from typing import ClassVar, override


ROOT = Path(__file__).resolve().parents[2]
WORKFLOW = ROOT / ".github" / "workflows" / "build-std.yml"


class BuildStdWorkflowTests(unittest.TestCase):
    workflow: ClassVar[str]

    @classmethod
    @override
    def setUpClass(cls) -> None:
        cls.workflow = WORKFLOW.read_text(encoding="utf-8")

    def test_runs_on_build_and_std_inputs(self) -> None:
        for path in [
            ".node-version",
            ".python-version",
            "crates/**",
            "lib/**",
            "examples/**",
            "benchmarks/build/**",
            "tools/**",
        ]:
            self.assertIn(f'      - "{path}"', self.workflow)
        self.assertIn("workflow_dispatch:", self.workflow)

    def test_uses_the_managed_linux_llvm_identity(self) -> None:
        self.assertIn("runs-on: ubuntu-24.04", self.workflow)
        self.assertIn("LLVM_SYS_221_PREFIX: /usr/lib/llvm-22", self.workflow)
        self.assertIn("llvm-toolchain-noble-22", self.workflow)
        self.assertIn("llvm-22-dev lld-22", self.workflow)

    def test_tracks_and_reports_the_latest_rust_stable_identity(self) -> None:
        self.assertIn("rustup toolchain install stable", self.workflow)
        self.assertIn("--component clippy --component rustfmt", self.workflow)
        for command in [
            "rustc --version --verbose",
            "cargo --version",
            "cargo clippy --version",
            "cargo fmt --version",
        ]:
            self.assertIn(command, self.workflow)

    def test_runs_static_and_dynamic_correctness_gates(self) -> None:
        for command in [
            "cargo fmt --all -- --check",
            "cargo clippy --workspace --all-targets --all-features -- -D warnings",
            "cargo check --workspace --all-targets --all-features",
            "cargo test --workspace --all-features",
            "cargo test -p nia-cli --test build_cases",
            "cargo test -p nia-cli --test toolchain_relocation",
            "python3 -m tools check",
            "python3 -m tools baseline build",
        ]:
            self.assertIn(command, self.workflow)

    def test_baseline_requires_real_acceptance_and_publishes_evidence(self) -> None:
        self.assertIn("--repetitions 1", self.workflow)
        self.assertIn('report["acceptance"]["passed"]', self.workflow)
        self.assertIn("actions/upload-artifact@v7", self.workflow)
        self.assertIn("nia-build-std-evidence", self.workflow)

    def test_uses_current_official_action_runtimes(self) -> None:
        for action in [
            "actions/checkout@v7",
            "actions/setup-python@v6",
            "actions/setup-node@v6",
            "actions/cache@v6",
            "actions/upload-artifact@v7",
        ]:
            self.assertIn(action, self.workflow)
        self.assertNotIn("@v4", self.workflow)

    def test_installs_repository_python_version(self) -> None:
        self.assertIn('python-version-file: ".python-version"', self.workflow)
        self.assertIn('node-version-file: ".node-version"', self.workflow)
        self.assertIn("npm ci --prefix tools --ignore-scripts", self.workflow)
        self.assertIn("cache-dependency-path: tools/package-lock.json", self.workflow)
        for command in ("python3 --version", "node --version", "npm --version"):
            self.assertIn(command, self.workflow)


if __name__ == "__main__":
    unittest.main()
