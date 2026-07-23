import unittest

from tools.perf import (
    large_codegen_source,
    require_allocation_instrumentation,
    require_codegen_bucket_instrumentation,
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

    def test_accepts_multiple_codegen_bucket_units(self):
        require_codegen_bucket_instrumentation(
            {"counters": {"llvm.units": 4, "llvm.memory_permits": 4}}, 4
        )

    def test_rejects_single_codegen_bucket_unit(self):
        with self.assertRaisesRegex(RuntimeError, "required LLVM units"):
            require_codegen_bucket_instrumentation(
                {"counters": {"llvm.units": 1, "llvm.memory_permits": 1}}, 2
            )

    def test_rejects_malformed_codegen_bucket_counter(self):
        with self.assertRaisesRegex(RuntimeError, "required LLVM units"):
            require_codegen_bucket_instrumentation(
                {"counters": {"llvm.units": True, "llvm.memory_permits": 4}}, 2
            )

    def test_rejects_cached_codegen_bucket_sample(self):
        with self.assertRaisesRegex(RuntimeError, "required LLVM units"):
            require_codegen_bucket_instrumentation(
                {"counters": {"llvm.units": 4, "llvm.memory_permits": 0}}, 4
            )

    def test_large_codegen_source_is_stable_and_reachable(self):
        source = large_codegen_source(blob_count=8, blob_bytes=4096)
        self.assertEqual(source.count("static blob"), 8)
        self.assertEqual(source.count("[4096]u8"), 8)
        self.assertEqual(source.count(".len()"), 8)
        self.assertIn("static blob007: [4096]u8 = [7u8; 4096];", source)


if __name__ == "__main__":
    unittest.main()
