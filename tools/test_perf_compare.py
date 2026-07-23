import unittest

from tools.perf_compare import compare_baselines


def result(name, wall, rss, queries, allocations):
    return {
        "name": name,
        "process": {"wall_seconds": wall, "max_rss_bytes": rss},
        "counters": {
            "query.executions": queries,
            "allocator.allocated_bytes": allocations,
            "allocator.peak_live_bytes": allocations // 2,
        },
    }


def baseline(results, cpu=8, memory=16_000_000_000):
    return {
        "schema_version": 1,
        "machine": {
            "system": "Linux",
            "architecture": "x86_64",
            "cpu_model": "test cpu",
            "effective_cpu_limit": cpu,
            "effective_memory_limit_bytes": memory,
        },
        "results": results,
    }


class CompareBaselinesTests(unittest.TestCase):
    def setUp(self):
        self.thresholds = {
            "wall": 50.0,
            "rss": 30.0,
            "query": 5.0,
            "allocation": 20.0,
        }

    def test_uses_iteration_medians_and_accepts_values_under_thresholds(self):
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

    def test_rejects_regressions_over_threshold(self):
        before = baseline([result("check", 10.0, 100, 1000, 1000)])
        after = baseline([result("check", 16.0, 100, 1000, 1000)])

        report = compare_baselines(before, after, self.thresholds, False)

        self.assertFalse(report["passed"])
        wall = next(
            item for item in report["comparisons"] if item["metric"] == "wall_seconds"
        )
        self.assertFalse(wall["passed"])

    def test_rejects_incompatible_machine_by_default(self):
        before = baseline([result("check", 10.0, 100, 1000, 1000)], cpu=8)
        after = baseline([result("check", 10.0, 100, 1000, 1000)], cpu=4)

        report = compare_baselines(before, after, self.thresholds, False)

        self.assertFalse(report["passed"])
        self.assertFalse(report["machine_compatible"])
        self.assertEqual(report["comparisons"], [])


if __name__ == "__main__":
    unittest.main()
