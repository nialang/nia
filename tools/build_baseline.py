#!/usr/bin/env python3
"""Run the representative Nia build workload under bounded resources."""

from __future__ import annotations

import argparse
import json
import os
import shutil
import signal
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
DEFAULT_FIXTURE = ROOT / "benchmarks" / "build" / "representative"
DEFAULT_OUTPUT = ROOT / "target" / "nia-build-baseline" / "baseline.json"
DEFAULT_TIMEOUT_SECONDS = 420
MIN_AVAILABLE_MEMORY_BYTES = 2 * 1024 * 1024 * 1024


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
        "outer_timing": outer[0],
        "compiler_timings": [],
    }


def build_command(nia: Path, workspace: Path, step: str | None = None) -> list[str]:
    command = [
        str(nia),
        "build",
        "--root",
        str(workspace),
        "--timings=detail",
        "--timings-format=json",
    ]
    if step is not None:
        command.insert(2, step)
    return command


def run_state(
    nia: Path,
    workspace: Path,
    name: str,
    timeout_seconds: int,
    *,
    step: str | None = None,
    expect_success: bool = True,
) -> dict[str, Any]:
    command = build_command(nia, workspace, step)
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
        "command": ["$NIA", *command[1:]],
        "return_code": result.returncode,
        "wall_seconds_observed": elapsed,
        "available_memory_bytes_before": available,
        **reports,
    }


def run_workload(
    nia: Path, fixture: Path, timeout_seconds: int
) -> tuple[list[dict[str, Any]], Path]:
    temporary = Path(tempfile.mkdtemp(prefix="nia-build-baseline-"))
    workspace = temporary / "representative"
    shutil.copytree(fixture, workspace)
    try:
        results = [run_state(nia, workspace, "clean", timeout_seconds)]
        results.append(run_state(nia, workspace, "warm", timeout_seconds))
        shutil.copyfile(workspace / "src/main.edited.nia", workspace / "src/main.nia")
        results.append(run_state(nia, workspace, "source_edit", timeout_seconds))
        build_script = (workspace / "build.nia").read_text(encoding="utf-8")
        (workspace / "build.nia").write_text(
            build_script.replace("deps/helper.nia", "deps/helper_edited.nia"),
            encoding="utf-8",
        )
        results.append(run_state(nia, workspace, "module_map_edit", timeout_seconds))
        results.append(
            run_state(
                nia,
                workspace,
                "failed_action",
                timeout_seconds,
                step="fail",
                expect_success=False,
            )
        )
        return results, temporary
    except BaseException:
        shutil.rmtree(temporary, ignore_errors=True)
        raise


def parse_args() -> argparse.Namespace:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--nia", type=Path, default=DEFAULT_NIA)
    parser.add_argument("--fixture", type=Path, default=DEFAULT_FIXTURE)
    parser.add_argument("--output", type=Path, default=DEFAULT_OUTPUT)
    parser.add_argument("--timeout-seconds", type=int, default=DEFAULT_TIMEOUT_SECONDS)
    parser.add_argument("--keep-workspace", action="store_true")
    return parser.parse_args()


def main() -> int:
    args = parse_args()
    nia = args.nia.resolve()
    fixture = args.fixture.resolve()
    if not nia.is_file():
        raise SystemExit(f"nia executable does not exist: {nia}")
    if not fixture.joinpath("build.nia").is_file():
        raise SystemExit(f"build fixture does not exist: {fixture}")
    if args.timeout_seconds < 1:
        raise SystemExit("--timeout-seconds must be positive")

    temporary: Path | None = None
    try:
        results, temporary = run_workload(nia, fixture, args.timeout_seconds)
        baseline = {
            "schema_version": 1,
            "kind": "nia-build-baseline",
            "machine": machine_metadata(),
            "fixture": "benchmarks/build/representative",
            "results": results,
        }
        output = args.output.resolve()
        output.parent.mkdir(parents=True, exist_ok=True)
        output.write_text(json.dumps(baseline, indent=2) + "\n", encoding="utf-8")
        print(output)
        if args.keep_workspace:
            print(f"workspace: {temporary}", file=sys.stderr)
            temporary = None
    except (OSError, RuntimeError, ValueError) as error:
        raise SystemExit(f"error: {error}") from None
    finally:
        if temporary is not None:
            shutil.rmtree(temporary, ignore_errors=True)
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
