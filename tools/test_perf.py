import unittest

from tools.perf import require_allocation_instrumentation


class PerfRunnerTests(unittest.TestCase):
    def test_accepts_instrumented_timing_report(self):
        require_allocation_instrumentation(
            {
                "counters": {
                    "allocator.allocated_bytes": 123,
                    "allocator.peak_live_bytes": 45,
                }
            }
        )

    def test_rejects_uninstrumented_timing_report_with_build_command(self):
        with self.assertRaisesRegex(RuntimeError, "--features perf-alloc"):
            require_allocation_instrumentation({"counters": {}})

    def test_rejects_malformed_counters_with_build_command(self):
        with self.assertRaisesRegex(RuntimeError, "--features perf-alloc"):
            require_allocation_instrumentation({"counters": None})


if __name__ == "__main__":
    unittest.main()
