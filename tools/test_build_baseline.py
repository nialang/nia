import unittest

from tools.build_baseline import (
    BUILD_STAGE_NAMES,
    build_command,
    json_lines,
    parse_build_reports,
    summarize_runs,
    workload_acceptance,
)


class BuildBaselineTests(unittest.TestCase):
    def timing_report(self, wall=0.4, rss=20, counters=None):
        return {
            "schema_version": 1,
            "process": {"wall_seconds": wall, "max_rss_bytes": rss},
            "timings": [
                {
                    "kind": "stage",
                    "name": name,
                    "count": 1,
                    "total_seconds": index + 0.1,
                    "max_seconds": index + 0.1,
                }
                for index, name in enumerate(BUILD_STAGE_NAMES)
            ],
            "counters": counters
            or {
                "build.runner_compilations": 1,
                "build.runner_executions": 1,
                "llvm.object_reuse_misses": 0,
            },
        }

    def test_extracts_outer_build_measurement(self):
        child = self.timing_report(counters={"llvm.units": 1})
        outer = {
            **self.timing_report(),
        }
        stderr = "diagnostic\n" + "\n".join(
            __import__("json").dumps(value) for value in (child, outer)
        )

        parsed = parse_build_reports(stderr, True)

        self.assertTrue(parsed["actions"]["success"])
        self.assertEqual(parsed["outer_timing"], outer)
        self.assertEqual(
            parsed["measurement"]["stages"]["build_compile_runner"][
                "total_seconds"
            ],
            2.1,
        )
        self.assertEqual(
            parsed["measurement"]["counters"]["llvm.object_reuse_misses"], 0
        )

    def test_rejects_missing_outer_stages(self):
        outer = (
            '{"schema_version":1,"process":{},"timings":[],'
            '"counters":{"build.runner_executions":1}}'
        )
        with self.assertRaisesRegex(ValueError, "build_resolve_invocation"):
            parse_build_reports(outer, True)

    def test_rejects_missing_build_stage(self):
        outer = self.timing_report()
        outer["timings"].pop()
        with self.assertRaisesRegex(ValueError, "build_execute_plan"):
            parse_build_reports(__import__("json").dumps(outer), True)

    def test_summarizes_repeated_stage_and_counter_samples(self):
        runs = []
        for index in range(3):
            parsed = parse_build_reports(
                __import__("json").dumps(
                    self.timing_report(wall=float(index + 1), rss=(index + 1) * 10)
                ),
                True,
            )
            runs.append(
                [
                    {
                        "name": "warm",
                        "wall_seconds_observed": float(index + 1),
                        **parsed,
                    }
                ]
            )

        summary = summarize_runs(runs)[0]

        self.assertEqual(summary["wall_seconds_observed"]["median"], 2.0)
        self.assertEqual(summary["wall_seconds_observed"]["p95"], 3.0)
        self.assertEqual(summary["process"]["max_rss_bytes"]["median"], 20)
        self.assertEqual(summary["counters"]["build.runner_executions"]["min"], 1)

    def test_warm_acceptance_retains_failed_counter_evidence(self):
        result = {
            "name": "warm",
            "measurement": {
                "counters": {
                    "build.action_cache_lookups": 1,
                    "build.action_cache_hits": 1,
                    "llvm.object_reuse_misses": 15,
                    "link.result_reuse_misses": 0,
                }
            },
        }

        acceptance = workload_acceptance([result, result])

        self.assertFalse(acceptance["passed"])
        llvm = next(
            check
            for check in acceptance["checks"]
            if check["counter"] == "llvm.object_reuse_misses"
        )
        self.assertEqual(llvm["found"], 15)

    def test_ignores_non_json_stderr(self):
        self.assertEqual(json_lines("error: ordinary diagnostic\nnot-json"), [])

    def test_build_command_places_named_step_after_build(self):
        command = build_command(
            __import__("pathlib").Path("/tool/nia"),
            __import__("pathlib").Path("/tool/lib"),
            __import__("pathlib").Path("/tmp/package"),
            "fail",
        )
        self.assertEqual(
            command[:5],
            ["/tool/nia", "--resource-root", "/tool/lib", "build", "fail"],
        )
        self.assertIn("--timings=detail", command)
        self.assertIn("--timings-format=json", command)


if __name__ == "__main__":
    unittest.main()
