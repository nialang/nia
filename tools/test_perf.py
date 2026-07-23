import unittest

from tools.perf import (
    require_allocation_instrumentation,
    require_module_finalization_instrumentation,
)


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

    def test_accepts_backend_finalization_live_window_counters(self):
        require_module_finalization_instrumentation(
            {
                "counters": {
                    "backend.module_finalization.start_live_bytes": 100,
                    "backend.module_finalization.end_live_bytes": 110,
                    "backend.module_finalization.peak_live_bytes": 120,
                    "backend.module_finalization.peak_growth_bytes": 20,
                }
            }
        )

    def test_rejects_missing_backend_finalization_live_window_counters(self):
        with self.assertRaisesRegex(RuntimeError, "finalization live-window counters"):
            require_module_finalization_instrumentation({"counters": {}})


if __name__ == "__main__":
    unittest.main()
