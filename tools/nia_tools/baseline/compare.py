#!/usr/bin/env python3
"""Compare two Nia performance baselines with resource-aware relative guards."""

from __future__ import annotations

import argparse
import json
import math
import statistics
import sys
from collections.abc import Sequence
from dataclasses import dataclass
from pathlib import Path
from typing import Callable, TypedDict

from tools.nia_tools.common.json_data import JsonValue, decode_json


type Number = int | float


class MachineIdentity(TypedDict, total=False):
    runner_class: str | None
    system: str
    architecture: str
    cpu_model: str | None
    effective_cpu_limit: Number | None
    effective_memory_limit_bytes: Number | None


class ProcessMetrics(TypedDict):
    wall_seconds: Number
    max_rss_bytes: Number


class PerformanceResult(TypedDict):
    name: str
    process: ProcessMetrics
    counters: dict[str, Number]


class PerformanceBaseline(TypedDict):
    schema_version: int
    machine: MachineIdentity
    results: list[PerformanceResult]


class MetricComparison(TypedDict):
    workload: str
    metric: str
    baseline: float
    candidate: float
    change_percent: float | None
    threshold_percent: float
    passed: bool


class ComparisonReport(TypedDict):
    schema_version: int
    machine_compatible: bool
    machine_mismatches: list[str]
    allow_machine_mismatch: bool
    thresholds_percent: dict[str, float]
    comparisons: list[MetricComparison]
    errors: list[str]
    passed: bool


@dataclass(frozen=True)
class Metric:
    name: str
    threshold_name: str
    read: Callable[[PerformanceResult], Number]


@dataclass(frozen=True)
class Options:
    baseline: Path
    candidate: Path
    max_wall_regression: float
    max_rss_regression: float
    max_query_regression: float
    max_allocation_regression: float
    allow_machine_mismatch: bool


METRICS = (
    Metric("wall_seconds", "wall", lambda result: result["process"]["wall_seconds"]),
    Metric("max_rss_bytes", "rss", lambda result: result["process"]["max_rss_bytes"]),
    Metric(
        "query.executions",
        "query",
        lambda result: result["counters"]["query.executions"],
    ),
    Metric(
        "allocator.allocated_bytes",
        "allocation",
        lambda result: result["counters"]["allocator.allocated_bytes"],
    ),
    Metric(
        "allocator.peak_live_bytes",
        "allocation",
        lambda result: result["counters"]["allocator.peak_live_bytes"],
    ),
)

WORKLOAD_METRICS = {
    "module_backend": (
        Metric(
            "backend.module_finalization.peak_growth_bytes",
            "allocation",
            lambda result: result["counters"][
                "backend.module_finalization.peak_growth_bytes"
            ],
        ),
    ),
}


def numeric(value: JsonValue | None, context: str) -> Number:
    if isinstance(value, bool) or not isinstance(value, (int, float)):
        raise ValueError(f"{context} is not numeric")
    return value


def optional_number(value: JsonValue | None, context: str) -> Number | None:
    return None if value is None else numeric(value, context)


def optional_text(value: JsonValue | None, context: str) -> str | None:
    if value is not None and not isinstance(value, str):
        raise ValueError(f"{context} is not text or null")
    return value


def parse_machine(value: JsonValue | None) -> MachineIdentity:
    if not isinstance(value, dict):
        raise ValueError("baseline machine identity is not an object")
    system = value.get("system")
    architecture = value.get("architecture")
    if not isinstance(system, str) or not isinstance(architecture, str):
        raise ValueError("baseline machine identity lacks system or architecture")
    return {
        "runner_class": optional_text(value.get("runner_class"), "runner_class"),
        "system": system,
        "architecture": architecture,
        "cpu_model": optional_text(value.get("cpu_model"), "cpu_model"),
        "effective_cpu_limit": optional_number(
            value.get("effective_cpu_limit"), "effective_cpu_limit"
        ),
        "effective_memory_limit_bytes": optional_number(
            value.get("effective_memory_limit_bytes"),
            "effective_memory_limit_bytes",
        ),
    }


def parse_result(value: JsonValue, index: int) -> PerformanceResult:
    if not isinstance(value, dict):
        raise ValueError(f"baseline result {index} is not an object")
    name = value.get("name")
    process = value.get("process")
    counters = value.get("counters")
    if not isinstance(name, str):
        raise ValueError(f"baseline result {index} lacks a string name")
    if not isinstance(process, dict) or not isinstance(counters, dict):
        raise ValueError(f"baseline result {name!r} lacks process or counters")
    parsed_counters = {
        key: numeric(item, f"workload {name!r} counter {key}")
        for key, item in counters.items()
    }
    return {
        "name": name,
        "process": {
            "wall_seconds": numeric(
                process.get("wall_seconds"), f"workload {name!r} wall_seconds"
            ),
            "max_rss_bytes": numeric(
                process.get("max_rss_bytes"), f"workload {name!r} max_rss_bytes"
            ),
        },
        "counters": parsed_counters,
    }


def parse_baseline(value: JsonValue, context: str) -> PerformanceBaseline:
    if (
        not isinstance(value, dict)
        or type(value.get("schema_version")) is not int
        or value["schema_version"] != 1
    ):
        raise ValueError(f"{context} does not use schema_version=1")
    results = value.get("results")
    if not isinstance(results, list):
        raise ValueError(f"{context} results are not an array")
    return {
        "schema_version": 1,
        "machine": parse_machine(value.get("machine")),
        "results": [parse_result(result, index) for index, result in enumerate(results)],
    }


def load_baseline(path: Path) -> PerformanceBaseline:
    try:
        value = decode_json(path.read_text(encoding="utf-8"), f"baseline {path}")
    except (OSError, ValueError) as error:
        raise ValueError(f"failed to read baseline {path}: {error}") from error
    return parse_baseline(value, f"baseline {path}")


def relative_difference(left: int | float, right: int | float) -> float:
    scale = max(abs(float(left)), abs(float(right)), 1.0)
    return abs(float(left) - float(right)) / scale


def machine_mismatches(
    baseline: PerformanceBaseline, candidate: PerformanceBaseline
) -> list[str]:
    left = baseline.get("machine", {})
    right = candidate.get("machine", {})
    mismatches: list[str] = []
    left_runner_class = left.get("runner_class")
    right_runner_class = right.get("runner_class")
    if left_runner_class != right_runner_class:
        mismatches.append(
            "runner_class differs: "
            f"{left_runner_class!r} != {right_runner_class!r}"
        )
    identity_fields = ["system", "architecture"]
    if left_runner_class is None and right_runner_class is None:
        identity_fields.append("cpu_model")
    for name in identity_fields:
        if left.get(name) != right.get(name):
            mismatches.append(f"{name} differs: {left.get(name)!r} != {right.get(name)!r}")
    for name, tolerance in (
        ("effective_cpu_limit", 0.01),
        ("effective_memory_limit_bytes", 0.10),
    ):
        left_value = left.get(name)
        right_value = right.get(name)
        if left_value is None or right_value is None:
            if left_value != right_value:
                mismatches.append(f"{name} differs: {left_value!r} != {right_value!r}")
            continue
        if relative_difference(left_value, right_value) > tolerance:
            mismatches.append(f"{name} differs: {left_value!r} != {right_value!r}")
    return mismatches


def grouped_results(
    baseline: PerformanceBaseline,
) -> dict[str, list[PerformanceResult]]:
    grouped: dict[str, list[PerformanceResult]] = {}
    for result in baseline["results"]:
        name = result["name"]
        grouped.setdefault(name, []).append(result)
    return grouped


def median_metric(results: list[PerformanceResult], metric: Metric) -> float:
    values: list[float] = []
    for result in results:
        try:
            value = metric.read(result)
        except (KeyError, TypeError) as error:
            raise ValueError(
                f"workload {result.get('name')!r} is missing metric {metric.name}"
            ) from error
        if not math.isfinite(float(value)):
            raise ValueError(
                f"workload {result.get('name')!r} metric {metric.name} is not finite"
            )
        values.append(float(value))
    return statistics.median(values)


def change_percent(baseline: float, candidate: float) -> float | None:
    if baseline == 0.0:
        return 0.0 if candidate == 0.0 else None
    return (candidate - baseline) * 100.0 / baseline


def compare_baselines(
    baseline: PerformanceBaseline,
    candidate: PerformanceBaseline,
    thresholds: dict[str, float],
    allow_machine_mismatch: bool,
) -> ComparisonReport:
    mismatches = machine_mismatches(baseline, candidate)
    left = grouped_results(baseline)
    right = grouped_results(candidate)
    missing = sorted(set(left) - set(right))
    added = sorted(set(right) - set(left))
    comparisons: list[MetricComparison] = []
    errors: list[str] = []
    if not left:
        errors.append("baseline contains no workloads")
    if missing:
        errors.append(f"candidate is missing workloads: {', '.join(missing)}")
    if added:
        errors.append(f"candidate has unexpected workloads: {', '.join(added)}")
    if mismatches and not allow_machine_mismatch:
        errors.append("machine resources are not comparable")

    if not errors:
        for workload in sorted(left):
            for metric in (*METRICS, *WORKLOAD_METRICS.get(workload, ())):
                baseline_value = median_metric(left[workload], metric)
                candidate_value = median_metric(right[workload], metric)
                change = change_percent(baseline_value, candidate_value)
                threshold = thresholds[metric.threshold_name]
                passed = change is not None and change <= threshold
                comparisons.append(
                    {
                        "workload": workload,
                        "metric": metric.name,
                        "baseline": baseline_value,
                        "candidate": candidate_value,
                        "change_percent": change,
                        "threshold_percent": threshold,
                        "passed": passed,
                    }
                )

    return {
        "schema_version": 1,
        "machine_compatible": not mismatches,
        "machine_mismatches": mismatches,
        "allow_machine_mismatch": allow_machine_mismatch,
        "thresholds_percent": thresholds,
        "comparisons": comparisons,
        "errors": errors,
        "passed": not errors and all(item["passed"] for item in comparisons),
    }


def parse_args(arguments: Sequence[str] | None = None) -> Options:
    parser = argparse.ArgumentParser(
        prog="python3 -m tools baseline compare", description=__doc__
    )
    parser.add_argument("baseline", type=Path)
    parser.add_argument("candidate", type=Path)
    parser.add_argument("--max-wall-regression", type=float, default=50.0)
    parser.add_argument("--max-rss-regression", type=float, default=30.0)
    parser.add_argument("--max-query-regression", type=float, default=5.0)
    parser.add_argument("--max-allocation-regression", type=float, default=20.0)
    parser.add_argument("--allow-machine-mismatch", action="store_true")
    namespace = parser.parse_args(arguments)
    if not isinstance(namespace.baseline, Path) or not isinstance(
        namespace.candidate, Path
    ):
        raise TypeError("argparse did not produce Paths for baseline inputs")
    thresholds = (
        namespace.max_wall_regression,
        namespace.max_rss_regression,
        namespace.max_query_regression,
        namespace.max_allocation_regression,
    )
    if not all(isinstance(value, float) for value in thresholds):
        raise TypeError("argparse did not produce float regression thresholds")
    return Options(
        baseline=namespace.baseline,
        candidate=namespace.candidate,
        max_wall_regression=thresholds[0],
        max_rss_regression=thresholds[1],
        max_query_regression=thresholds[2],
        max_allocation_regression=thresholds[3],
        allow_machine_mismatch=bool(namespace.allow_machine_mismatch),
    )


def main(arguments: Sequence[str] | None = None) -> int:
    args = parse_args(arguments)
    thresholds = {
        "wall": args.max_wall_regression,
        "rss": args.max_rss_regression,
        "query": args.max_query_regression,
        "allocation": args.max_allocation_regression,
    }
    if any(not math.isfinite(value) or value < 0.0 for value in thresholds.values()):
        raise SystemExit("regression thresholds must be finite and non-negative")
    try:
        report = compare_baselines(
            load_baseline(args.baseline),
            load_baseline(args.candidate),
            thresholds,
            args.allow_machine_mismatch,
        )
    except ValueError as error:
        print(f"error: {error}", file=sys.stderr)
        return 2
    print(json.dumps(report, indent=2, sort_keys=True))
    return 0 if report["passed"] else 1


if __name__ == "__main__":
    raise SystemExit(main())
