import unittest
from pathlib import Path


ROOT = Path(__file__).resolve().parents[1]
WORKFLOW = ROOT / ".github" / "workflows" / "performance.yml"


class PerformanceWorkflowTests(unittest.TestCase):
    @classmethod
    def setUpClass(cls):
        cls.workflow = WORKFLOW.read_text(encoding="utf-8")

    def test_runs_complete_controlled_runner_baseline(self):
        self.assertIn("--repeat 3", self.workflow)
        self.assertIn(
            "--runner-class github-hosted-ubuntu-24.04-x64", self.workflow
        )
        self.assertNotIn("--allow-machine-mismatch", self.workflow)

    def test_downloads_only_successful_main_runs(self):
        self.assertIn("--branch main", self.workflow)
        self.assertIn("--status success", self.workflow)
        self.assertIn("--name nia-perf-baseline", self.workflow)
        self.assertIn("steps.main_baseline.outputs.available", self.workflow)

    def test_promotes_only_successful_main_baselines(self):
        self.assertIn("if: success() && github.ref == 'refs/heads/main'", self.workflow)
        self.assertIn("retention-days: 90", self.workflow)
        self.assertIn("name: nia-perf-candidate", self.workflow)
        self.assertIn("retention-days: 14", self.workflow)

    def test_publishes_baseline_provenance_in_step_summary(self):
        self.assertIn("BASELINE_RUN_ID", self.workflow)
        self.assertIn("steps.main_baseline.outputs.run_id", self.workflow)
        self.assertNotIn("outputs.run-", self.workflow)
        self.assertIn("GITHUB_STEP_SUMMARY", self.workflow)
        self.assertIn("Comparison passed", self.workflow)

    def test_installs_and_selects_llvm_22(self):
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


if __name__ == "__main__":
    unittest.main()
