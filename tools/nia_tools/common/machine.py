from __future__ import annotations

import os
import platform
from typing import TypedDict

from tools.nia_tools.common.resources import probe_resources


class MachineMetadata(TypedDict):
    runner_class: str | None
    system: str
    platform: str
    architecture: str
    cpu_model: str | None
    logical_cpus: int | None
    affinity_cpus: int | None
    cgroup_cpu_quota: float | None
    effective_cpu_limit: float | int | None
    system_memory_bytes: int | None
    cgroup_memory_limit_bytes: int | None
    effective_memory_limit_bytes: int | None


def machine_metadata(runner_class: str | None = None) -> MachineMetadata:
    affinity = None
    if hasattr(os, "sched_getaffinity"):
        affinity = len(os.sched_getaffinity(0))
    resources = probe_resources()
    cpu_limits = [
        value
        for value in (affinity, resources.cgroup_cpu_quota)
        if value is not None and value > 0
    ]
    return {
        "runner_class": runner_class,
        "system": platform.system(),
        "platform": platform.platform(),
        "architecture": platform.machine(),
        "cpu_model": resources.cpu_model,
        "logical_cpus": os.cpu_count(),
        "affinity_cpus": affinity,
        "cgroup_cpu_quota": resources.cgroup_cpu_quota,
        "effective_cpu_limit": min(cpu_limits) if cpu_limits else None,
        "system_memory_bytes": resources.system_memory_bytes,
        "cgroup_memory_limit_bytes": resources.cgroup_memory_limit_bytes,
        "effective_memory_limit_bytes": resources.effective_memory_limit_bytes(),
    }
