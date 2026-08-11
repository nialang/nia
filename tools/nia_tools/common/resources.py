from __future__ import annotations

from dataclasses import dataclass
from pathlib import Path


PROC_ROOT = Path("/proc")
CGROUP_ROOT = Path("/sys/fs/cgroup")


def read_optional_text(path: Path) -> str | None:
    try:
        return path.read_text(encoding="utf-8")
    except OSError:
        return None


def parse_limit(value: str | None) -> int | None:
    if value is None:
        return None
    text = value.strip()
    if not text or text == "max":
        return None
    try:
        parsed = int(text)
    except ValueError:
        return None
    return parsed if parsed >= 0 else None


def parse_proc_memory(value: str | None) -> tuple[int | None, int | None]:
    total = None
    available = None
    for line in (value or "").splitlines():
        name, separator, rest = line.partition(":")
        if not separator or name not in {"MemTotal", "MemAvailable"}:
            continue
        fields = rest.split()
        try:
            amount = int(fields[0]) * 1024
        except (IndexError, ValueError):
            continue
        if name == "MemTotal":
            total = amount
        else:
            available = amount
    return total, available


def parse_cpu_model(value: str | None) -> str | None:
    for line in (value or "").splitlines():
        if not line.startswith(("model name", "Hardware")):
            continue
        _, separator, model = line.partition(":")
        if separator and model.strip():
            return model.strip()
    return None


@dataclass(frozen=True)
class CgroupPaths:
    unified: Path | None
    memory: Path | None
    cpu: Path | None


def parse_cgroup_paths(value: str | None, cgroup_root: Path = CGROUP_ROOT) -> CgroupPaths:
    unified = None
    memory = None
    cpu = None
    for line in (value or "").splitlines():
        fields = line.split(":", 2)
        if len(fields) != 3:
            continue
        hierarchy, controllers, relative_text = fields
        relative = Path(relative_text.lstrip("/"))
        if hierarchy == "0" and not controllers:
            unified = cgroup_root / relative
            continue
        names = set(controllers.split(","))
        if "memory" in names:
            memory = cgroup_root / "memory" / relative
        if "cpu" in names:
            cpu = cgroup_root / "cpu" / relative
    return CgroupPaths(unified=unified, memory=memory, cpu=cpu)


def parse_cpu_max(value: str | None) -> float | None:
    fields = (value or "").split()
    if len(fields) != 2 or fields[0] == "max":
        return None
    try:
        quota = int(fields[0])
        period = int(fields[1])
    except ValueError:
        return None
    if quota < 0 or period <= 0:
        return None
    return quota / period


@dataclass(frozen=True)
class ResourceSnapshot:
    system_memory_bytes: int | None
    system_available_memory_bytes: int | None
    cgroup_memory_limit_bytes: int | None
    cgroup_memory_current_bytes: int | None
    cgroup_cpu_quota: float | None
    cpu_model: str | None

    def effective_memory_limit_bytes(self) -> int | None:
        limits = [
            value
            for value in (self.system_memory_bytes, self.cgroup_memory_limit_bytes)
            if value is not None and value > 0
        ]
        return min(limits) if limits else None

    def available_memory_bytes(self) -> int | None:
        candidates = [
            value
            for value in (self.system_available_memory_bytes,)
            if value is not None and value >= 0
        ]
        if (
            self.cgroup_memory_limit_bytes is not None
            and self.cgroup_memory_current_bytes is not None
        ):
            candidates.append(
                max(
                    0,
                    self.cgroup_memory_limit_bytes
                    - self.cgroup_memory_current_bytes,
                )
            )
        return min(candidates) if candidates else None


def probe_resources(
    proc_root: Path = PROC_ROOT, cgroup_root: Path = CGROUP_ROOT
) -> ResourceSnapshot:
    total, available = parse_proc_memory(read_optional_text(proc_root / "meminfo"))
    cpu_model = parse_cpu_model(read_optional_text(proc_root / "cpuinfo"))
    paths = parse_cgroup_paths(
        read_optional_text(proc_root / "self" / "cgroup"), cgroup_root
    )

    if paths.unified is not None:
        memory_limit = parse_limit(read_optional_text(paths.unified / "memory.max"))
        memory_current = parse_limit(read_optional_text(paths.unified / "memory.current"))
        cpu_quota = parse_cpu_max(read_optional_text(paths.unified / "cpu.max"))
    else:
        memory_limit = (
            parse_limit(read_optional_text(paths.memory / "memory.limit_in_bytes"))
            if paths.memory is not None
            else None
        )
        memory_current = (
            parse_limit(read_optional_text(paths.memory / "memory.usage_in_bytes"))
            if paths.memory is not None
            else None
        )
        quota = (
            parse_limit(read_optional_text(paths.cpu / "cpu.cfs_quota_us"))
            if paths.cpu is not None
            else None
        )
        period = (
            parse_limit(read_optional_text(paths.cpu / "cpu.cfs_period_us"))
            if paths.cpu is not None
            else None
        )
        cpu_quota = (
            quota / period
            if quota is not None and period is not None and period > 0
            else None
        )

    return ResourceSnapshot(
        system_memory_bytes=total,
        system_available_memory_bytes=available,
        cgroup_memory_limit_bytes=memory_limit,
        cgroup_memory_current_bytes=memory_current,
        cgroup_cpu_quota=cpu_quota,
        cpu_model=cpu_model,
    )
