import tempfile
import unittest
from pathlib import Path

from tools.nia_tools.common.resources import (
    ResourceSnapshot,
    parse_cgroup_paths,
    parse_cpu_max,
    parse_proc_memory,
    probe_resources,
)


class ResourceTests(unittest.TestCase):
    def test_parses_proc_memory_without_order_dependency(self) -> None:
        total, available = parse_proc_memory(
            "MemAvailable: 512 kB\nIgnored: 4 kB\nMemTotal: 1024 kB\n"
        )

        self.assertEqual(total, 1024 * 1024)
        self.assertEqual(available, 512 * 1024)

    def test_parses_cgroup_v1_and_v2_paths(self) -> None:
        v2 = parse_cgroup_paths("0::/user.slice/session.scope\n", Path("/cgroup"))
        v1 = parse_cgroup_paths(
            "4:memory:/build\n3:cpu,cpuacct:/build\n", Path("/cgroup")
        )

        self.assertEqual(v2.unified, Path("/cgroup/user.slice/session.scope"))
        self.assertEqual(v1.memory, Path("/cgroup/memory/build"))
        self.assertEqual(v1.cpu, Path("/cgroup/cpu/build"))

    def test_rejects_unbounded_or_invalid_cpu_quota(self) -> None:
        self.assertIsNone(parse_cpu_max("max 100000"))
        self.assertIsNone(parse_cpu_max("100000 0"))
        self.assertEqual(parse_cpu_max("150000 100000"), 1.5)

    def test_snapshot_uses_tightest_available_memory(self) -> None:
        snapshot = ResourceSnapshot(
            system_memory_bytes=16_000,
            system_available_memory_bytes=8_000,
            cgroup_memory_limit_bytes=6_000,
            cgroup_memory_current_bytes=2_000,
            cgroup_cpu_quota=None,
            cpu_model=None,
        )

        self.assertEqual(snapshot.effective_memory_limit_bytes(), 6_000)
        self.assertEqual(snapshot.available_memory_bytes(), 4_000)

    def test_probes_unified_cgroup_files(self) -> None:
        with tempfile.TemporaryDirectory() as temporary:
            root = Path(temporary)
            proc = root / "proc"
            cgroup = root / "cgroup"
            (proc / "self").mkdir(parents=True)
            (cgroup / "job").mkdir(parents=True)
            (proc / "meminfo").write_text(
                "MemTotal: 1000 kB\nMemAvailable: 600 kB\n", encoding="utf-8"
            )
            (proc / "cpuinfo").write_text("model name: Test CPU\n", encoding="utf-8")
            (proc / "self" / "cgroup").write_text("0::/job\n", encoding="utf-8")
            (cgroup / "job" / "memory.max").write_text("512000\n", encoding="utf-8")
            (cgroup / "job" / "memory.current").write_text("128000\n", encoding="utf-8")
            (cgroup / "job" / "cpu.max").write_text("200000 100000\n", encoding="utf-8")

            snapshot = probe_resources(proc, cgroup)

        self.assertEqual(snapshot.system_memory_bytes, 1_024_000)
        self.assertEqual(snapshot.cgroup_memory_limit_bytes, 512_000)
        self.assertEqual(snapshot.available_memory_bytes(), 384_000)
        self.assertEqual(snapshot.cgroup_cpu_quota, 2.0)
        self.assertEqual(snapshot.cpu_model, "Test CPU")


if __name__ == "__main__":
    unittest.main()
