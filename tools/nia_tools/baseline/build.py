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
from collections.abc import Sequence
from dataclasses import dataclass
from pathlib import Path
from typing import NotRequired, TypedDict, cast

from tools.nia_tools.common.machine import MachineMetadata, machine_metadata
from tools.nia_tools.common.json_data import JsonObject, JsonValue, decode_json
from tools.nia_tools.common.resources import probe_resources
from tools.nia_tools.repository import REPOSITORY_ROOT


ROOT = REPOSITORY_ROOT
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


@dataclass(frozen=True)
class Options:
    nia: Path
    resource_root: Path
    fixture: Path
    output: Path
    timeout_seconds: int
    repetitions: int
    keep_workspace: bool


type Number = int | float


class TimingReport(TypedDict):
    schema_version: int
    process: JsonObject
    counters: JsonObject
    timings: list[JsonObject]
    kind: NotRequired[str | None]


class ActionReport(TypedDict):
    schema_version: int
    kind: str
    success: bool
    counters: dict[str, int]


class Measurement(TypedDict):
    process: JsonObject
    stages: dict[str, JsonObject]
    counters: JsonObject


class BuildReports(TypedDict):
    actions: ActionReport
    measurement: Measurement
    outer_timing: TimingReport


class BuildResult(BuildReports):
    name: str
    command: list[str]
    return_code: int
    wall_seconds_observed: float
    available_memory_bytes_before: int | None
    corrupted_action_cache_entries: NotRequired[int]


class AcceptanceCheck(TypedDict):
    state: str
    counter: str
    expected: int | str
    found: int
    passed: bool


class AcceptanceReport(TypedDict):
    passed: bool
    checks: list[AcceptanceCheck]


class Distribution(TypedDict):
    median: Number
    p95: Number
    min: Number
    max: Number


class StateSummary(TypedDict):
    name: str
    sample_count: int
    wall_seconds_observed: Distribution
    process: dict[str, Distribution]
    stages: dict[str, Distribution]
    counters: dict[str, Distribution]


class BuildRunSample(TypedDict):
    sample: int
    acceptance: AcceptanceReport
    results: list[BuildResult]


class AggregateAcceptance(TypedDict):
    passed: bool
    samples: list[AcceptanceReport]


class BuildBaseline(TypedDict):
    schema_version: int
    kind: str
    machine: MachineMetadata
    fixture: str
    runs: list[BuildRunSample]
    acceptance: AggregateAcceptance
    summary: list[StateSummary]


def available_memory_bytes() -> int | None:
    return probe_resources().available_memory_bytes()


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


def json_lines(stderr: str) -> list[JsonObject]:
    reports: list[JsonObject] = []
    for line in stderr.splitlines():
        try:
            value = decode_json(line, "build timing report")
        except ValueError:
            continue
        if (
            isinstance(value, dict)
            and type(value.get("schema_version")) is int
            and value["schema_version"] == 1
        ):
            reports.append(value)
    return reports


def parse_build_reports(stderr: str, succeeded: bool) -> BuildReports:
    timings: list[TimingReport] = []
    for report in json_lines(stderr):
        if report.get("kind") is not None:
            continue
        if not isinstance(report.get("process"), dict) or not isinstance(
            report.get("counters"), dict
        ):
            continue
        raw_timings = report.get("timings")
        if not isinstance(raw_timings, list):
            continue
        if not all(isinstance(entry, dict) for entry in raw_timings):
            raise ValueError("build timing report timings contain a non-object entry")
        timings.append(cast(TimingReport, report))
    outer = [
        report
        for report in timings
        if "build.runner_executions" in report["counters"]
    ]
    if len(outer) != 1:
        raise ValueError(f"expected one outer build timing report, found {len(outer)}")
    outer_report = outer[0]
    counters = outer_report["counters"]
    timing_entries = outer_report["timings"]
    stages: dict[str, JsonObject] = {}
    for name in BUILD_STAGE_NAMES:
        entries = [
            entry
            for entry in timing_entries
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
    action_counters: dict[str, int] = {}
    for name in (
        "build.steps_executed",
        "build.actions_executed",
        "build.action_failures",
    ):
        value = counters.get(name, 0)
        if not isinstance(value, int) or isinstance(value, bool):
            raise ValueError(f"outer build counter {name!r} is not an integer")
        action_counters[name] = value
    return {
        "actions": {
            "schema_version": 1,
            "kind": "nia-build-coordinator-actions",
            "success": succeeded,
            "counters": action_counters,
        },
        "measurement": {
            "process": outer_report["process"],
            "stages": stages,
            "counters": measured_counters,
        },
        "outer_timing": outer_report,
    }


def counter(result: BuildResult, name: str) -> int:
    value = result["measurement"]["counters"].get(name, 0)
    if not isinstance(value, int) or isinstance(value, bool):
        raise ValueError(f"counter {name!r} is not an integer")
    return value


def validate_workload(results: list[BuildResult]) -> None:
    expected_names = (
        "clean",
        "warm",
        "source_edit",
        "module_map_edit",
        "corrupt_cache",
        "recovered_warm",
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


def workload_acceptance(results: list[BuildResult]) -> AcceptanceReport:
    clean = results[0]
    warm = results[1]
    source_edit = results[2]
    module_map_edit = results[3]
    corrupt_cache = results[4]
    recovered_warm = results[5]
    failed_action = results[6]
    expected_actions = counter(clean, "build.action_cache_lookups")
    checks: list[AcceptanceCheck] = []

    def exact(state: str, result: BuildResult, name: str, expected: int) -> None:
        found = counter(result, name)
        checks.append(
            {
                "state": state,
                "counter": name,
                "expected": expected,
                "found": found,
                "passed": found == expected,
            }
        )

    def positive(state: str, result: BuildResult, name: str) -> None:
        found = counter(result, name)
        checks.append(
            {
                "state": state,
                "counter": name,
                "expected": "> 0",
                "found": found,
                "passed": found > 0,
            }
        )

    positive("clean", clean, "build.action_cache_lookups")
    exact("clean", clean, "build.action_cache_misses", expected_actions)
    exact("clean", clean, "build.action_cache_hits", 0)

    exact("warm", warm, "build.action_cache_lookups", expected_actions)
    exact("warm", warm, "build.action_cache_hits", expected_actions)
    exact("warm", warm, "build.action_cache_misses", 0)
    exact("warm", warm, "llvm.object_reuse_misses", 0)
    exact("warm", warm, "link.result_reuse_misses", 0)

    exact("source_edit", source_edit, "build.action_cache_lookups", expected_actions)
    positive("source_edit", source_edit, "build.action_cache_misses")
    positive("source_edit", source_edit, "build.action_cache_invalidation_sources")
    positive("source_edit", source_edit, "llvm.object_reuse_misses")
    positive("source_edit", source_edit, "link.result_reuse_misses")

    exact(
        "module_map_edit",
        module_map_edit,
        "build.action_cache_lookups",
        expected_actions,
    )
    positive("module_map_edit", module_map_edit, "build.action_cache_misses")
    positive(
        "module_map_edit",
        module_map_edit,
        "build.action_cache_invalidation_module",
    )
    positive("module_map_edit", module_map_edit, "llvm.object_reuse_misses")
    positive("module_map_edit", module_map_edit, "link.result_reuse_misses")

    corrupted_entries = corrupt_cache.get("corrupted_action_cache_entries", 0)
    checks.append(
        {
            "state": "corrupt_cache",
            "counter": "baseline.corrupted_action_cache_entries",
            "expected": "> 0",
            "found": corrupted_entries,
            "passed": corrupted_entries > 0,
        }
    )
    exact(
        "corrupt_cache",
        corrupt_cache,
        "build.action_cache_lookups",
        expected_actions,
    )
    exact(
        "corrupt_cache",
        corrupt_cache,
        "build.action_cache_misses",
        expected_actions,
    )
    exact(
        "corrupt_cache",
        corrupt_cache,
        "build.action_cache_miss_corrupt",
        expected_actions,
    )
    exact("corrupt_cache", corrupt_cache, "build.action_cache_hits", 0)
    exact("corrupt_cache", corrupt_cache, "llvm.object_reuse_misses", 0)
    exact("corrupt_cache", corrupt_cache, "link.result_reuse_misses", 0)

    exact(
        "recovered_warm",
        recovered_warm,
        "build.action_cache_lookups",
        expected_actions,
    )
    exact(
        "recovered_warm",
        recovered_warm,
        "build.action_cache_hits",
        expected_actions,
    )
    exact("recovered_warm", recovered_warm, "build.action_cache_misses", 0)
    exact("recovered_warm", recovered_warm, "llvm.object_reuse_misses", 0)
    exact("recovered_warm", recovered_warm, "link.result_reuse_misses", 0)

    exact("failed_action", failed_action, "build.steps_executed", 0)
    exact("failed_action", failed_action, "build.actions_executed", 0)
    exact("failed_action", failed_action, "build.action_cache_lookups", 0)
    exact("failed_action", failed_action, "build.action_failures", 1)
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


def distribution(values: list[Number]) -> Distribution:
    return {
        "median": statistics.median(values),
        "p95": nearest_rank(values, 0.95),
        "min": min(values),
        "max": max(values),
    }


def numeric_value(value: JsonValue, context: str) -> Number:
    if isinstance(value, bool) or not isinstance(value, (int, float)):
        raise ValueError(f"{context} is not numeric")
    return value


def summarize_runs(runs: list[list[BuildResult]]) -> list[StateSummary]:
    summaries: list[StateSummary] = []
    for state_index, first in enumerate(runs[0]):
        samples = [run[state_index] for run in runs]
        counter_names = sorted(
            {
                str(name)
                for sample in samples
                for name in sample["measurement"]["counters"].keys()
            }
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
                        [
                            numeric_value(
                                sample["measurement"]["process"][name],
                                f"process metric {name}",
                            )
                            for sample in samples
                        ]
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
                            numeric_value(
                                sample["measurement"]["stages"][name]["total_seconds"],
                                f"stage metric {name}",
                            )
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
) -> BuildResult:
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


def corrupt_action_cache(workspace: Path) -> int:
    entries = sorted(
        path
        for path in workspace.joinpath(".nia-cache", "actions").rglob("*.entry")
        if path.is_file()
    )
    if not entries:
        raise ValueError("build workload produced no action-cache entries to corrupt")
    for path in entries:
        path.write_bytes(b"nia build baseline injected corruption\n")
    return len(entries)


def run_workload(
    nia: Path, resource_root: Path, fixture: Path, timeout_seconds: int
) -> tuple[list[BuildResult], Path]:
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
        corrupted_entries = corrupt_action_cache(workspace)
        corrupt_result = run_state(
            nia, resource_root, workspace, "corrupt_cache", timeout_seconds
        )
        corrupt_result["corrupted_action_cache_entries"] = corrupted_entries
        results.append(corrupt_result)
        results.append(
            run_state(nia, resource_root, workspace, "recovered_warm", timeout_seconds)
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


def parse_args(arguments: Sequence[str] | None = None) -> Options:
    parser = argparse.ArgumentParser(
        prog="python3 -m tools baseline build", description=__doc__
    )
    parser.add_argument("--nia", type=Path, default=DEFAULT_NIA)
    parser.add_argument("--resource-root", type=Path, default=DEFAULT_RESOURCE_ROOT)
    parser.add_argument("--fixture", type=Path, default=DEFAULT_FIXTURE)
    parser.add_argument("--output", type=Path, default=DEFAULT_OUTPUT)
    parser.add_argument("--timeout-seconds", type=int, default=DEFAULT_TIMEOUT_SECONDS)
    parser.add_argument("--repetitions", type=int, default=DEFAULT_REPETITIONS)
    parser.add_argument("--keep-workspace", action="store_true")
    namespace = parser.parse_args(arguments)
    paths = (namespace.nia, namespace.resource_root, namespace.fixture, namespace.output)
    if not all(isinstance(value, Path) for value in paths):
        raise TypeError("argparse did not produce Paths for build baseline options")
    if not isinstance(namespace.timeout_seconds, int) or not isinstance(
        namespace.repetitions, int
    ):
        raise TypeError("argparse did not produce integer build baseline limits")
    return Options(
        nia=paths[0],
        resource_root=paths[1],
        fixture=paths[2],
        output=paths[3],
        timeout_seconds=namespace.timeout_seconds,
        repetitions=namespace.repetitions,
        keep_workspace=bool(namespace.keep_workspace),
    )


def main(arguments: Sequence[str] | None = None) -> int:
    args = parse_args(arguments)
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
        runs: list[list[BuildResult]] = []
        for _ in range(args.repetitions):
            results, temporary = run_workload(
                nia, resource_root, fixture, args.timeout_seconds
            )
            runs.append(results)
            temporaries.append(temporary)
        baseline: BuildBaseline = {
            "schema_version": 3,
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
