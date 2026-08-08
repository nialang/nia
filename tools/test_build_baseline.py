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
    def acceptance_result(self, name, counters):
        return {
            "name": name,
            "measurement": {"counters": counters},
        }

    def passing_acceptance_results(self, action_count=3):
        return [
            self.acceptance_result(
                "clean",
                {
                    "build.action_cache_lookups": action_count,
                    "build.action_cache_misses": action_count,
                },
            ),
            self.acceptance_result(
                "warm",
                {
                    "build.action_cache_lookups": action_count,
                    "build.action_cache_hits": action_count,
                    "build.action_cache_misses": 0,
                    "llvm.object_reuse_misses": 0,
                    "link.result_reuse_misses": 0,
                },
            ),
            self.acceptance_result(
                "source_edit",
                {
                    "build.action_cache_lookups": action_count,
                    "build.action_cache_misses": 1,
                    "build.action_cache_invalidation_sources": 1,
                    "llvm.object_reuse_misses": 1,
                    "link.result_reuse_misses": 1,
                },
            ),
            self.acceptance_result(
                "module_map_edit",
                {
                    "build.action_cache_lookups": action_count,
                    "build.action_cache_misses": 1,
                    "build.action_cache_invalidation_module": 1,
                    "llvm.object_reuse_misses": 1,
                    "link.result_reuse_misses": 1,
                },
            ),
            self.acceptance_result(
                "failed_action",
                {
                    "build.steps_executed": 0,
                    "build.actions_executed": 0,
                    "build.action_cache_lookups": 0,
                    "build.action_failures": 1,
                },
            ),
        ]

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
        results = self.passing_acceptance_results(action_count=1)
        results[1]["measurement"]["counters"]["llvm.object_reuse_misses"] = 15

        acceptance = workload_acceptance(results)

        self.assertFalse(acceptance["passed"])
        llvm = next(
            check
            for check in acceptance["checks"]
            if check["counter"] == "llvm.object_reuse_misses"
        )
        self.assertEqual(llvm["found"], 15)

    def test_warm_acceptance_scales_with_clean_action_count(self):
        acceptance = workload_acceptance(self.passing_acceptance_results())

        self.assertTrue(acceptance["passed"])
        self.assertEqual(
            next(
                check
                for check in acceptance["checks"]
                if check["state"] == "warm"
                and check["counter"] == "build.action_cache_hits"
            )["expected"],
            3,
        )

    def test_edit_acceptance_requires_typed_invalidation(self):
        results = self.passing_acceptance_results()
        results[2]["measurement"]["counters"].pop(
            "build.action_cache_invalidation_sources"
        )
        results[3]["measurement"]["counters"].pop(
            "build.action_cache_invalidation_module"
        )

        acceptance = workload_acceptance(results)

        self.assertFalse(acceptance["passed"])
        failed = {
            (check["state"], check["counter"])
            for check in acceptance["checks"]
            if not check["passed"]
        }
        self.assertIn(
            ("source_edit", "build.action_cache_invalidation_sources"), failed
        )
        self.assertIn(
            ("module_map_edit", "build.action_cache_invalidation_module"), failed
        )

    def test_failed_action_acceptance_rejects_executed_actions(self):
        results = self.passing_acceptance_results()
        results[4]["measurement"]["counters"]["build.actions_executed"] = 1

        acceptance = workload_acceptance(results)

        self.assertFalse(acceptance["passed"])
        action_check = next(
            check
            for check in acceptance["checks"]
            if check["state"] == "failed_action"
            and check["counter"] == "build.actions_executed"
        )
        self.assertEqual(action_check["found"], 1)

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
