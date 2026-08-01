#!/usr/bin/env python3
"""Run the representative Nia build workload under bounded resources."""

from __future__ import annotations

import argparse
import json
import os
import shutil
import signal
import statistics
import subprocess
import sys
import tempfile
import time
from pathlib import Path
from typing import Any

try:
    from tools.perf import machine_metadata
except ModuleNotFoundError:
    from perf import machine_metadata


ROOT = Path(__file__).resolve().parents[1]
DEFAULT_NIA = ROOT / "target" / "release" / "nia"
DEFAULT_RESOURCE_ROOT = ROOT / "lib"
DEFAULT_FIXTURE = ROOT / "benchmarks" / "build" / "representative"
DEFAULT_OUTPUT = ROOT / "target" / "nia-build-baseline" / "baseline.json"
DEFAULT_TIMEOUT_SECONDS = 420
MIN_AVAILABLE_MEMORY_BYTES = 2 * 1024 * 1024 * 1024
DEFAULT_REPETITIONS = 3
BUILD_STAGE_NAMES = (
    "build_resolve_invocation",
    "build_prepare_directories",
    "build_compile_runner",
    "build_run_runner",
    "build_execute_plan",
)
MEASUREMENT_COUNTER_PREFIXES = (
    "build.",
    "compiler.check_certificate_",
    "frontend.",
    "link.result_",
    "llvm.",
    "query.cache_hits",
    "query.executions",
)


def read_text(path: Path) -> str | None:
    try:
        return path.read_text(encoding="utf-8")
    except OSError:
        return None


def parse_limit(value: str | None) -> int | None:
    if value is None:
        return None
    value = value.strip()
    if not value or value == "max":
        return None
    try:
        return int(value)
    except ValueError:
        return None


def available_memory_bytes() -> int | None:
    candidates: list[int] = []
    meminfo = read_text(Path("/proc/meminfo"))
    if meminfo is not None:
        for line in meminfo.splitlines():
            if line.startswith("MemAvailable:"):
                try:
                    candidates.append(int(line.split()[1]) * 1024)
                except (IndexError, ValueError):
                    pass
                break

    cgroup = read_text(Path("/proc/self/cgroup"))
    if cgroup is not None:
        for line in cgroup.splitlines():
            if not line.startswith("0::"):
                continue
            relative = line[3:].lstrip("/")
            root = Path("/sys/fs/cgroup") / relative
            limit = parse_limit(read_text(root / "memory.max"))
            current = parse_limit(read_text(root / "memory.current"))
            if limit is not None and current is not None:
                candidates.append(max(0, limit - current))
            break
    return min(candidates) if candidates else None


def require_memory_headroom() -> int | None:
    available = available_memory_bytes()
    if available is not None and available < MIN_AVAILABLE_MEMORY_BYTES:
        raise RuntimeError(
            "build baseline refused to start under memory pressure: "
            f"available={available} required={MIN_AVAILABLE_MEMORY_BYTES}"
        )
    return available


def terminate_process_group(process: subprocess.Popen[str]) -> None:
    if process.poll() is not None:
        return
    try:
        os.killpg(process.pid, signal.SIGTERM)
        process.wait(timeout=5)
    except (OSError, subprocess.TimeoutExpired):
        try:
            os.killpg(process.pid, signal.SIGKILL)
        except OSError:
            pass
        process.wait()


def run_bounded(
    command: list[str], cwd: Path, timeout_seconds: int
) -> tuple[subprocess.CompletedProcess[str], float, int | None]:
    available = require_memory_headroom()
    started = time.monotonic()
    process = subprocess.Popen(
        command,
        cwd=cwd,
        stdout=subprocess.PIPE,
        stderr=subprocess.PIPE,
        text=True,
        start_new_session=True,
    )
    try:
        stdout, stderr = process.communicate(timeout=timeout_seconds)
    except subprocess.TimeoutExpired:
        terminate_process_group(process)
        raise RuntimeError(
            f"command timed out after {timeout_seconds}s: {' '.join(command)}"
        ) from None
    elapsed = time.monotonic() - started
    return (
        subprocess.CompletedProcess(command, process.returncode, stdout, stderr),
        elapsed,
        available,
    )


def json_lines(stderr: str) -> list[dict[str, Any]]:
    reports = []
    for line in stderr.splitlines():
        try:
            value = json.loads(line)
        except json.JSONDecodeError:
            continue
        if isinstance(value, dict) and value.get("schema_version") == 1:
            reports.append(value)
    return reports


def parse_build_reports(stderr: str, succeeded: bool) -> dict[str, Any]:
    reports = json_lines(stderr)
    timings = [
        report
        for report in reports
        if report.get("kind") is None
        and isinstance(report.get("process"), dict)
        and isinstance(report.get("counters"), dict)
    ]
    outer = [
        report
        for report in timings
        if "build.runner_executions" in report["counters"]
    ]
    if len(outer) != 1:
        raise ValueError(f"expected one outer build timing report, found {len(outer)}")
    counters = outer[0]["counters"]
    stages = {}
    for name in BUILD_STAGE_NAMES:
        entries = [
            entry
            for entry in outer[0].get("timings", [])
            if entry.get("kind") == "stage" and entry.get("name") == name
        ]
        if len(entries) != 1:
            raise ValueError(f"expected one {name!r} timing entry, found {len(entries)}")
        stages[name] = entries[0]
    measured_counters = {
        name: value
        for name, value in counters.items()
        if name.startswith(MEASUREMENT_COUNTER_PREFIXES)
    }
    return {
        "actions": {
            "schema_version": 1,
            "kind": "nia-build-coordinator-actions",
            "success": succeeded,
            "counters": {
                name: counters.get(name, 0)
                for name in (
                    "build.steps_executed",
                    "build.actions_executed",
                    "build.action_failures",
                )
            },
        },
        "measurement": {
            "process": outer[0]["process"],
            "stages": stages,
            "counters": measured_counters,
        },
        "outer_timing": outer[0],
    }


def counter(result: dict[str, Any], name: str) -> int:
    value = result["measurement"]["counters"].get(name, 0)
    if not isinstance(value, int):
        raise ValueError(f"counter {name!r} is not an integer")
    return value


def validate_workload(results: list[dict[str, Any]]) -> None:
    expected_names = (
        "clean",
        "warm",
        "source_edit",
        "module_map_edit",
        "failed_action",
    )
    if tuple(result.get("name") for result in results) != expected_names:
        raise ValueError("build workload states are missing or out of order")

    for result in results:
        name = result["name"]
        expected_success = name != "failed_action"
        if result["actions"].get("success") != expected_success:
            raise ValueError(f"build state {name!r} has the wrong action status")
        if counter(result, "build.runner_compilations") != 1:
            raise ValueError(f"build state {name!r} did not compile exactly one runner")
        if counter(result, "build.runner_executions") != 1:
            raise ValueError(f"build state {name!r} did not execute exactly one runner")

    failed = results[-1]
    if counter(failed, "build.action_failures") != 1:
        raise ValueError("failed action state did not report exactly one action failure")


def workload_acceptance(results: list[dict[str, Any]]) -> dict[str, Any]:
    warm = results[1]
    expectations = {
        "build.action_cache_lookups": 1,
        "build.action_cache_hits": 1,
        "build.action_cache_misses": 0,
        "llvm.object_reuse_misses": 0,
        "link.result_reuse_misses": 0,
    }
    checks = [
        {
            "state": "warm",
            "counter": name,
            "expected": expected,
            "found": counter(warm, name),
            "passed": counter(warm, name) == expected,
        }
        for name, expected in expectations.items()
    ]
    return {
        "passed": all(check["passed"] for check in checks),
        "checks": checks,
    }


def nearest_rank(values: list[float | int], percentile: float) -> float | int:
    ordered = sorted(values)
    index = max(
        0,
        min(len(ordered) - 1, int(len(ordered) * percentile + 0.999999) - 1),
    )
    return ordered[index]


def distribution(values: list[float | int]) -> dict[str, float | int]:
    return {
        "median": statistics.median(values),
        "p95": nearest_rank(values, 0.95),
        "min": min(values),
        "max": max(values),
    }


def summarize_runs(runs: list[list[dict[str, Any]]]) -> list[dict[str, Any]]:
    summaries = []
    for state_index, first in enumerate(runs[0]):
        samples = [run[state_index] for run in runs]
        counter_names = sorted(
            set().union(
                *(sample["measurement"]["counters"].keys() for sample in samples)
            )
        )
        summaries.append(
            {
                "name": first["name"],
                "sample_count": len(samples),
                "wall_seconds_observed": distribution(
                    [sample["wall_seconds_observed"] for sample in samples]
                ),
                "process": {
                    name: distribution(
                        [sample["measurement"]["process"][name] for sample in samples]
                    )
                    for name in ("wall_seconds", "max_rss_bytes")
                    if all(
                        sample["measurement"]["process"].get(name) is not None
                        for sample in samples
                    )
                },
                "stages": {
                    name: distribution(
                        [
                            sample["measurement"]["stages"][name]["total_seconds"]
                            for sample in samples
                        ]
                    )
                    for name in BUILD_STAGE_NAMES
                },
                "counters": {
                    name: distribution([counter(sample, name) for sample in samples])
                    for name in counter_names
                },
            }
        )
    return summaries


def build_command(
    nia: Path, resource_root: Path, workspace: Path, step: str | None = None
) -> list[str]:
    command = [
        str(nia),
        "--resource-root",
        str(resource_root),
        "build",
        "--root",
        str(workspace),
        "--timings=detail",
        "--timings-format=json",
    ]
    if step is not None:
        command.insert(4, step)
    return command


def run_state(
    nia: Path,
    resource_root: Path,
    workspace: Path,
    name: str,
    timeout_seconds: int,
    *,
    step: str | None = None,
    expect_success: bool = True,
) -> dict[str, Any]:
    command = build_command(nia, resource_root, workspace, step)
    result, elapsed, available = run_bounded(command, workspace, timeout_seconds)
    succeeded = result.returncode == 0
    if succeeded != expect_success:
        sys.stderr.write(result.stderr)
        raise RuntimeError(
            f"build state {name!r} returned {result.returncode}; "
            f"expected success={expect_success}"
        )
    reports = parse_build_reports(result.stderr, succeeded)
    if reports["actions"].get("success") != expect_success:
        raise RuntimeError(f"build state {name!r} action status disagrees with process status")
    return {
        "name": name,
        "command": [
            "$NIA",
            "--resource-root",
            "$RESOURCE_ROOT",
            *command[3:],
        ],
        "return_code": result.returncode,
        "wall_seconds_observed": elapsed,
        "available_memory_bytes_before": available,
        **reports,
    }


def run_workload(
    nia: Path, resource_root: Path, fixture: Path, timeout_seconds: int
) -> tuple[list[dict[str, Any]], Path]:
    temporary = Path(tempfile.mkdtemp(prefix="nia-build-baseline-"))
    workspace = temporary / "representative"
    shutil.copytree(fixture, workspace)
    try:
        results = [
            run_state(nia, resource_root, workspace, "clean", timeout_seconds)
        ]
        results.append(
            run_state(nia, resource_root, workspace, "warm", timeout_seconds)
        )
        shutil.copyfile(workspace / "src/main.edited.nia", workspace / "src/main.nia")
        results.append(
            run_state(nia, resource_root, workspace, "source_edit", timeout_seconds)
        )
        build_script = (workspace / "build.nia").read_text(encoding="utf-8")
        (workspace / "build.nia").write_text(
            build_script.replace("deps/helper.nia", "deps/helper_edited.nia"),
            encoding="utf-8",
        )
        results.append(
            run_state(
                nia, resource_root, workspace, "module_map_edit", timeout_seconds
            )
        )
        results.append(
            run_state(
                nia,
                resource_root,
                workspace,
                "failed_action",
                timeout_seconds,
                step="fail",
                expect_success=False,
            )
        )
        validate_workload(results)
        return results, temporary
    except BaseException:
        shutil.rmtree(temporary, ignore_errors=True)
        raise


def parse_args() -> argparse.Namespace:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--nia", type=Path, default=DEFAULT_NIA)
    parser.add_argument("--resource-root", type=Path, default=DEFAULT_RESOURCE_ROOT)
    parser.add_argument("--fixture", type=Path, default=DEFAULT_FIXTURE)
    parser.add_argument("--output", type=Path, default=DEFAULT_OUTPUT)
    parser.add_argument("--timeout-seconds", type=int, default=DEFAULT_TIMEOUT_SECONDS)
    parser.add_argument("--repetitions", type=int, default=DEFAULT_REPETITIONS)
    parser.add_argument("--keep-workspace", action="store_true")
    return parser.parse_args()


def main() -> int:
    args = parse_args()
    nia = args.nia.resolve()
    resource_root = args.resource_root.resolve()
    fixture = args.fixture.resolve()
    if not nia.is_file():
        raise SystemExit(f"nia executable does not exist: {nia}")
    if not resource_root.joinpath("toolchain.meta").is_file():
        raise SystemExit(f"Nia resource root is invalid: {resource_root}")
    if not fixture.joinpath("build.nia").is_file():
        raise SystemExit(f"build fixture does not exist: {fixture}")
    if args.timeout_seconds < 1:
        raise SystemExit("--timeout-seconds must be positive")
    if args.repetitions < 1:
        raise SystemExit("--repetitions must be positive")

    temporaries: list[Path] = []
    try:
        runs = []
        for _ in range(args.repetitions):
            results, temporary = run_workload(
                nia, resource_root, fixture, args.timeout_seconds
            )
            runs.append(results)
            temporaries.append(temporary)
        baseline = {
            "schema_version": 2,
            "kind": "nia-build-baseline",
            "machine": machine_metadata(),
            "fixture": "benchmarks/build/representative",
            "runs": [
                {
                    "sample": index + 1,
                    "acceptance": workload_acceptance(results),
                    "results": results,
                }
                for index, results in enumerate(runs)
            ],
            "acceptance": {
                "passed": all(workload_acceptance(run)["passed"] for run in runs),
                "samples": [workload_acceptance(run) for run in runs],
            },
            "summary": summarize_runs(runs),
        }
        output = args.output.resolve()
        output.parent.mkdir(parents=True, exist_ok=True)
        output.write_text(json.dumps(baseline, indent=2) + "\n", encoding="utf-8")
        print(output)
        if args.keep_workspace:
            for temporary in temporaries:
                print(f"workspace: {temporary}", file=sys.stderr)
            temporaries.clear()
    except (OSError, RuntimeError, ValueError) as error:
        raise SystemExit(f"error: {error}") from None
    finally:
        for temporary in temporaries:
            shutil.rmtree(temporary, ignore_errors=True)
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
