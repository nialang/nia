import unittest

from tools.build_baseline import build_command, json_lines, parse_build_reports


class BuildBaselineTests(unittest.TestCase):
    def test_extracts_outer_child_and_action_reports(self):
        action = {
            "schema_version": 1,
            "kind": "nia-build-actions",
            "success": True,
            "counters": {"compiler_invocations": 2},
        }
        child = {
            "schema_version": 1,
            "process": {"wall_seconds": 0.2, "max_rss_bytes": 10},
            "timings": [],
            "counters": {"llvm.units": 1},
        }
        outer = {
            "schema_version": 1,
            "process": {"wall_seconds": 0.4, "max_rss_bytes": 20},
            "timings": [],
            "counters": {"build.runner_executions": 1},
        }
        stderr = "diagnostic\n" + "\n".join(
            __import__("json").dumps(value) for value in (child, action, outer)
        )

        parsed = parse_build_reports(stderr)

        self.assertEqual(parsed["actions"], action)
        self.assertEqual(parsed["outer_timing"], outer)
        self.assertEqual(parsed["compiler_timings"], [child])

    def test_rejects_missing_action_report(self):
        outer = (
            '{"schema_version":1,"process":{},"timings":[],'
            '"counters":{"build.runner_executions":1}}'
        )
        with self.assertRaisesRegex(ValueError, "one build action report"):
            parse_build_reports(outer)

    def test_ignores_non_json_stderr(self):
        self.assertEqual(json_lines("error: ordinary diagnostic\nnot-json"), [])

    def test_build_command_places_named_step_after_build(self):
        command = build_command(
            __import__("pathlib").Path("/tool/nia"),
            __import__("pathlib").Path("/tmp/package"),
            "fail",
        )
        self.assertEqual(command[:3], ["/tool/nia", "build", "fail"])
        self.assertIn("--timings=detail", command)
        self.assertIn("--timings-format=json", command)


if __name__ == "__main__":
    unittest.main()
