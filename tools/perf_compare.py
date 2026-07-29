#!/usr/bin/env python3
"""Compare two Nia performance baselines with resource-aware relative guards."""

from __future__ import annotations

import argparse
import json
import math
import statistics
import sys
from dataclasses import dataclass
from pathlib import Path
from typing import Any, Callable


@dataclass(frozen=True)
class Metric:
    name: str
    threshold_name: str
    read: Callable[[dict[str, Any]], int | float]


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


def load_baseline(path: Path) -> dict[str, Any]:
    try:
        value = json.loads(path.read_text(encoding="utf-8"))
    except (OSError, json.JSONDecodeError) as error:
        raise ValueError(f"failed to read baseline {path}: {error}") from error
    if not isinstance(value, dict) or value.get("schema_version") != 1:
        raise ValueError(f"baseline {path} does not use schema_version=1")
    return value


def relative_difference(left: int | float, right: int | float) -> float:
    scale = max(abs(float(left)), abs(float(right)), 1.0)
    return abs(float(left) - float(right)) / scale


def machine_mismatches(
    baseline: dict[str, Any], candidate: dict[str, Any]
) -> list[str]:
    left = baseline.get("machine", {})
    right = candidate.get("machine", {})
    mismatches = []
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


def grouped_results(baseline: dict[str, Any]) -> dict[str, list[dict[str, Any]]]:
    grouped: dict[str, list[dict[str, Any]]] = {}
    for result in baseline.get("results", []):
        name = result.get("name")
        if not isinstance(name, str):
            raise ValueError("baseline result is missing a string workload name")
        grouped.setdefault(name, []).append(result)
    return grouped


def median_metric(results: list[dict[str, Any]], metric: Metric) -> float:
    values = []
    for result in results:
        try:
            value = metric.read(result)
        except (KeyError, TypeError) as error:
            raise ValueError(
                f"workload {result.get('name')!r} is missing metric {metric.name}"
            ) from error
        if isinstance(value, bool) or not isinstance(value, (int, float)):
            raise ValueError(
                f"workload {result.get('name')!r} metric {metric.name} is not numeric"
            )
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
    baseline: dict[str, Any],
    candidate: dict[str, Any],
    thresholds: dict[str, float],
    allow_machine_mismatch: bool,
) -> dict[str, Any]:
    mismatches = machine_mismatches(baseline, candidate)
    left = grouped_results(baseline)
    right = grouped_results(candidate)
    missing = sorted(set(left) - set(right))
    added = sorted(set(right) - set(left))
    comparisons = []
    errors = []
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


def parse_args() -> argparse.Namespace:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("baseline", type=Path)
    parser.add_argument("candidate", type=Path)
    parser.add_argument("--max-wall-regression", type=float, default=50.0)
    parser.add_argument("--max-rss-regression", type=float, default=30.0)
    parser.add_argument("--max-query-regression", type=float, default=5.0)
    parser.add_argument("--max-allocation-regression", type=float, default=20.0)
    parser.add_argument("--allow-machine-mismatch", action="store_true")
    return parser.parse_args()


def main() -> int:
    args = parse_args()
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
