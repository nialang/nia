#!/usr/bin/env python3
"""Audit the source dependency closure required by the build host."""

from __future__ import annotations

import argparse
import json
import re
from collections import deque
from pathlib import Path


ROOT = Path(__file__).resolve().parents[1]
STD_ROOT = ROOT / "lib" / "std"
DEFAULT_SNAPSHOT = ROOT / "docs" / "std-build-host-dependencies.json"
ROOT_MODULES = (
    "builtin.nia",
    "start.nia",
    "build.nia",
)
USING = re.compile(r"^\s*using\s+pkg::([A-Za-z_][A-Za-z0-9_]*(?:::[A-Za-z_][A-Za-z0-9_]*)*)")
MODULE = re.compile(
    r"^\s*(?:pub(?:\(pkg\))?\s+)?module\s+([A-Za-z_][A-Za-z0-9_]*)\s*;"
)


def package_module_path(reference: str, std_root: Path = STD_ROOT) -> Path:
    parts = reference.split("::")
    for length in range(len(parts), 0, -1):
        candidate = std_root.joinpath(*parts[:length]).with_suffix(".nia")
        if candidate.is_file():
            return candidate
    return std_root.joinpath(*parts).with_suffix(".nia")


def child_module_path(owner: Path, name: str) -> Path:
    return owner.with_suffix("") / f"{name}.nia"


def source_dependencies(path: Path, std_root: Path = STD_ROOT) -> set[Path]:
    dependencies = set()
    for line in path.read_text(encoding="utf-8").splitlines():
        if match := USING.match(line):
            dependencies.add(package_module_path(match.group(1), std_root))
        if match := MODULE.match(line):
            child = child_module_path(path, match.group(1))
            if child.is_file():
                dependencies.add(child)
    return dependencies


def build_host_closure(std_root: Path = STD_ROOT) -> list[str]:
    queue = deque(std_root / name for name in ROOT_MODULES)
    visited: set[Path] = set()
    while queue:
        path = queue.popleft()
        if path in visited:
            continue
        if not path.is_file():
            raise ValueError(f"build-host dependency does not exist: {path}")
        visited.add(path)
        queue.extend(sorted(source_dependencies(path, std_root) - visited))
    return sorted(str(path.relative_to(std_root.parent)) for path in visited)


def snapshot() -> dict[str, object]:
    return {
        "schema_version": 1,
        "kind": "nia-std-build-host-source-closure",
        "roots": [f"std/{name}" for name in ROOT_MODULES],
        "modules": build_host_closure(),
    }


def parse_args() -> argparse.Namespace:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--check", type=Path)
    return parser.parse_args()


def main() -> int:
    args = parse_args()
    current = snapshot()
    if args.check is not None:
        expected = json.loads(args.check.read_text(encoding="utf-8"))
        if current != expected:
            raise SystemExit(
                "build-host std dependency closure changed; review API/layering "
                "impact and update the snapshot deliberately"
            )
    else:
        print(json.dumps(current, indent=2))
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
