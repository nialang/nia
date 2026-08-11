import unittest
from pathlib import Path


ROOT = Path(__file__).resolve().parents[1]
WORKFLOW = ROOT / ".github" / "workflows" / "build-std.yml"


class BuildStdWorkflowTests(unittest.TestCase):
    @classmethod
    def setUpClass(cls):
        cls.workflow = WORKFLOW.read_text(encoding="utf-8")

    def test_runs_on_build_and_std_inputs(self):
        for path in [
            "crates/**",
            "lib/**",
            "examples/**",
            "benchmarks/build/**",
            "tools/perf.py",
            "tools/std_build_host_audit.py",
            "tools/test_std_build_host_audit.py",
            "tools/fixtures/std-build-host-dependencies.json",
        ]:
            self.assertIn(f'      - "{path}"', self.workflow)
        self.assertIn("workflow_dispatch:", self.workflow)

    def test_uses_the_managed_linux_llvm_identity(self):
        self.assertIn("runs-on: ubuntu-24.04", self.workflow)
        self.assertIn("LLVM_SYS_221_PREFIX: /usr/lib/llvm-22", self.workflow)
        self.assertIn("llvm-toolchain-noble-22", self.workflow)
        self.assertIn("llvm-22-dev lld-22", self.workflow)

    def test_tracks_and_reports_the_latest_rust_stable_identity(self):
        self.assertIn("rustup toolchain install stable", self.workflow)
        self.assertIn("--component clippy --component rustfmt", self.workflow)
        for command in [
            "rustc --version --verbose",
            "cargo --version",
            "cargo clippy --version",
            "cargo fmt --version",
        ]:
            self.assertIn(command, self.workflow)

    def test_runs_static_and_dynamic_correctness_gates(self):
        for command in [
            "cargo fmt --all -- --check",
            "cargo clippy --workspace --all-targets --all-features -- -D warnings",
            "cargo check --workspace --all-targets --all-features",
            "cargo test --workspace --all-features",
            "cargo test -p nia-cli --test build_cases",
            "cargo test -p nia-cli --test toolchain_relocation",
            "python3 tools/build_baseline.py",
            "tools.test_std_build_host_audit",
        ]:
            self.assertIn(command, self.workflow)

    def test_baseline_requires_real_acceptance_and_publishes_evidence(self):
        self.assertIn("--repetitions 1", self.workflow)
        self.assertIn('report["acceptance"]["passed"]', self.workflow)
        self.assertIn("actions/upload-artifact@v7", self.workflow)
        self.assertIn("nia-build-std-evidence", self.workflow)

    def test_uses_current_official_action_runtimes(self):
        for action in [
            "actions/checkout@v7",
            "actions/cache@v6",
            "actions/upload-artifact@v7",
        ]:
            self.assertIn(action, self.workflow)
        self.assertNotIn("@v4", self.workflow)


if __name__ == "__main__":
    unittest.main()
