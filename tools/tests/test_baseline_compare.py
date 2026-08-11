import unittest
from typing import override

from tools.nia_tools.baseline.compare import (
    PerformanceBaseline,
    PerformanceResult,
    compare_baselines,
    parse_baseline,
)
from tools.nia_tools.common.json_data import decode_json


def result(
    name: str, wall: float, rss: int, queries: int, allocations: int
) -> PerformanceResult:
    value: PerformanceResult = {
        "name": name,
        "process": {"wall_seconds": wall, "max_rss_bytes": rss},
        "counters": {
            "query.executions": queries,
            "allocator.allocated_bytes": allocations,
            "allocator.peak_live_bytes": allocations // 2,
        },
    }
    if name == "module_backend":
        value["counters"]["backend.module_finalization.peak_growth_bytes"] = (
            allocations // 10
        )
    return value


def baseline(
    results: list[PerformanceResult],
    cpu: int = 8,
    memory: int = 16_000_000_000,
    runner_class: str | None = None,
    cpu_model: str = "test cpu",
) -> PerformanceBaseline:
    return {
        "schema_version": 1,
        "machine": {
            "runner_class": runner_class,
            "system": "Linux",
            "architecture": "x86_64",
            "cpu_model": cpu_model,
            "effective_cpu_limit": cpu,
            "effective_memory_limit_bytes": memory,
        },
        "results": results,
    }


class CompareBaselinesTests(unittest.TestCase):
    @override
    def setUp(self) -> None:
        self.thresholds = {
            "wall": 50.0,
            "rss": 30.0,
            "query": 5.0,
            "allocation": 20.0,
        }

    def test_uses_iteration_medians_and_accepts_values_under_thresholds(self) -> None:
        before = baseline(
            [
                result("check", 10.0, 100, 1000, 1000),
                result("check", 12.0, 120, 1000, 1200),
                result("check", 100.0, 1000, 1000, 10_000),
            ]
        )
        after = baseline(
            [
                result("check", 12.0, 120, 1020, 1200),
                result("check", 14.0, 130, 1020, 1300),
                result("check", 200.0, 2000, 1020, 20_000),
            ]
        )

        report = compare_baselines(before, after, self.thresholds, False)

        self.assertTrue(report["passed"])
        wall = next(
            item for item in report["comparisons"] if item["metric"] == "wall_seconds"
        )
        self.assertEqual(wall["baseline"], 12.0)
        self.assertEqual(wall["candidate"], 14.0)

    def test_rejects_malformed_nested_baseline_metrics(self) -> None:
        malformed = decode_json(
            '{"schema_version":1,"machine":{"system":"Linux",'
            '"architecture":"x86_64"},"results":[{"name":"check",'
            '"process":{"wall_seconds":"fast","max_rss_bytes":100},'
            '"counters":{"query.executions":1}}]}'
        )

        with self.assertRaisesRegex(ValueError, "wall_seconds is not numeric"):
            parse_baseline(malformed, "fixture")

    def test_rejects_boolean_counter_as_non_numeric(self) -> None:
        malformed = decode_json(
            '{"schema_version":1,"machine":{"system":"Linux",'
            '"architecture":"x86_64"},"results":[{"name":"check",'
            '"process":{"wall_seconds":1,"max_rss_bytes":100},'
            '"counters":{"query.executions":true}}]}'
        )

        with self.assertRaisesRegex(ValueError, "query.executions is not numeric"):
            parse_baseline(malformed, "fixture")

    def test_rejects_regressions_over_threshold(self) -> None:
        before = baseline([result("check", 10.0, 100, 1000, 1000)])
        after = baseline([result("check", 16.0, 100, 1000, 1000)])

        report = compare_baselines(before, after, self.thresholds, False)

        self.assertFalse(report["passed"])
        wall = next(
            item for item in report["comparisons"] if item["metric"] == "wall_seconds"
        )
        self.assertFalse(wall["passed"])

    def test_guards_module_finalization_peak_growth(self) -> None:
        before = baseline([result("module_backend", 10.0, 100, 1000, 1000)])
        after = baseline([result("module_backend", 10.0, 100, 1000, 1300)])

        report = compare_baselines(before, after, self.thresholds, False)

        finalization_peak = next(
            item
            for item in report["comparisons"]
            if item["metric"]
            == "backend.module_finalization.peak_growth_bytes"
        )
        self.assertFalse(finalization_peak["passed"])

    def test_rejects_incompatible_machine_by_default(self) -> None:
        before = baseline([result("check", 10.0, 100, 1000, 1000)], cpu=8)
        after = baseline([result("check", 10.0, 100, 1000, 1000)], cpu=4)

        report = compare_baselines(before, after, self.thresholds, False)

        self.assertFalse(report["passed"])
        self.assertFalse(report["machine_compatible"])
        self.assertEqual(report["comparisons"], [])

    def test_local_samples_still_require_the_same_cpu_model(self) -> None:
        before = baseline(
            [result("check", 10.0, 100, 1000, 1000)], cpu_model="local cpu a"
        )
        after = baseline(
            [result("check", 10.0, 100, 1000, 1000)], cpu_model="local cpu b"
        )

        report = compare_baselines(before, after, self.thresholds, False)

        self.assertFalse(report["passed"])
        self.assertIn("cpu_model differs", report["machine_mismatches"][0])

    def test_controlled_runner_class_accepts_cpu_model_drift(self) -> None:
        before = baseline(
            [result("check", 10.0, 100, 1000, 1000)],
            runner_class="github-hosted-ubuntu-24.04-x64",
            cpu_model="host cpu a",
        )
        after = baseline(
            [result("check", 10.0, 100, 1000, 1000)],
            runner_class="github-hosted-ubuntu-24.04-x64",
            cpu_model="host cpu b",
        )

        report = compare_baselines(before, after, self.thresholds, False)

        self.assertTrue(report["passed"])
        self.assertTrue(report["machine_compatible"])

    def test_rejects_different_controlled_runner_classes(self) -> None:
        before = baseline(
            [result("check", 10.0, 100, 1000, 1000)],
            runner_class="github-hosted-ubuntu-24.04-x64",
        )
        after = baseline(
            [result("check", 10.0, 100, 1000, 1000)],
            runner_class="self-hosted-linux-x64",
        )

        report = compare_baselines(before, after, self.thresholds, False)

        self.assertFalse(report["passed"])
        self.assertIn("runner_class differs", report["machine_mismatches"][0])

    def test_controlled_runner_class_still_requires_the_same_resources(self) -> None:
        before = baseline(
            [result("check", 10.0, 100, 1000, 1000)],
            cpu=4,
            runner_class="github-hosted-ubuntu-24.04-x64",
        )
        after = baseline(
            [result("check", 10.0, 100, 1000, 1000)],
            cpu=2,
            runner_class="github-hosted-ubuntu-24.04-x64",
        )

        report = compare_baselines(before, after, self.thresholds, False)

        self.assertFalse(report["passed"])
        self.assertIn("effective_cpu_limit differs", report["machine_mismatches"][0])

    def test_rejects_controlled_runner_against_local_sample(self) -> None:
        before = baseline([result("check", 10.0, 100, 1000, 1000)])
        after = baseline(
            [result("check", 10.0, 100, 1000, 1000)],
            runner_class="github-hosted-ubuntu-24.04-x64",
        )

        report = compare_baselines(before, after, self.thresholds, False)

        self.assertFalse(report["passed"])
        self.assertFalse(report["machine_compatible"])


if __name__ == "__main__":
    unittest.main()
