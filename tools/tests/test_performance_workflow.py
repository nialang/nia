import unittest
from pathlib import Path


ROOT = Path(__file__).resolve().parents[2]
WORKFLOW = ROOT / ".github" / "workflows" / "performance.yml"


class PerformanceWorkflowTests(unittest.TestCase):
    @classmethod
    def setUpClass(cls):
        cls.workflow = WORKFLOW.read_text(encoding="utf-8")

    def test_runs_complete_controlled_runner_baseline(self):
        self.assertIn("python3 -m tools check", self.workflow)
        self.assertIn("python3 -m tools baseline compiler", self.workflow)
        self.assertIn("python3 -m tools baseline compare", self.workflow)
        self.assertIn("--repeat 3", self.workflow)
        self.assertIn(
            "--runner-class github-hosted-ubuntu-24.04-x64", self.workflow
        )
        self.assertNotIn("--allow-machine-mismatch", self.workflow)

    def test_downloads_latest_completed_main_baseline(self):
        self.assertIn("--branch main", self.workflow)
        self.assertIn("--status completed", self.workflow)
        self.assertIn('--name "${artifact_name}"', self.workflow)
        self.assertIn("artifact_name=nia-perf-baseline", self.workflow)
        self.assertNotIn("nia-perf-baseline nia-perf-candidate", self.workflow)
        self.assertIn('test -f "${destination}/baseline.json"', self.workflow)
        self.assertIn("steps.main_baseline.outputs.available", self.workflow)

    def test_rolls_forward_every_collected_main_baseline(self):
        self.assertIn('echo "candidate_available=true"', self.workflow)
        self.assertIn(
            "if: always() && github.ref == 'refs/heads/main' && "
            "steps.prepare.outputs.candidate_available == 'true'",
            self.workflow,
        )
        self.assertIn("retention-days: 90", self.workflow)
        self.assertIn("name: nia-perf-candidate", self.workflow)
        self.assertIn("retention-days: 14", self.workflow)

    def test_failure_artifact_does_not_require_a_completed_candidate(self):
        self.assertIn("tee target/nia-perf/candidate.log", self.workflow)
        self.assertIn(
            "if test -f target/nia-perf/candidate.json", self.workflow
        )
        self.assertIn("target/nia-perf/artifact/candidate.log", self.workflow)

    def test_uses_current_official_action_runtimes(self):
        for action in [
            "actions/checkout@v7",
            "actions/setup-python@v6",
            "actions/cache@v6",
            "actions/upload-artifact@v7",
        ]:
            self.assertIn(action, self.workflow)
        self.assertNotIn("@v4", self.workflow)

    def test_installs_repository_python_version(self):
        self.assertIn('python-version-file: ".python-version"', self.workflow)

    def test_publishes_baseline_provenance_in_step_summary(self):
        self.assertIn("BASELINE_RUN_ID", self.workflow)
        self.assertIn("BASELINE_ARTIFACT_NAME", self.workflow)
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
