#!/usr/bin/env python3
"""Report evidence for reviewing Nia workspace crate boundaries."""

from __future__ import annotations

import argparse
import json
import re
import subprocess
from collections.abc import Sequence
from dataclasses import dataclass
from pathlib import Path
from typing import Any, Iterable

from tools.nia_tools.repository import REPOSITORY_ROOT

ROOT = REPOSITORY_ROOT
PUBLIC_ITEM = re.compile(
    r"^pub\s+(?:async\s+|unsafe\s+|const\s+|extern\s+)*"
    r"(?:struct|enum|union|trait|type|const|static|fn|mod|use)\b"
)


@dataclass(frozen=True)
class CrateBoundary:
    name: str
    rust_loc: int
    public_items: int
    production_dependencies: tuple[str, ...]
    production_dependents: tuple[str, ...]
    dev_only_dependents: tuple[str, ...]


def cargo_metadata(root: Path) -> dict[str, Any]:
    result = subprocess.run(
        ["cargo", "metadata", "--no-deps", "--format-version", "1"],
        cwd=root,
        check=True,
        stdout=subprocess.PIPE,
        text=True,
    )
    return json.loads(result.stdout)


def dependency_kinds(dependency: dict[str, Any]) -> set[str]:
    kinds = dependency.get("kind")
    if kinds is not None:
        return {kinds}
    detailed_kinds = dependency.get("dep_kinds")
    if detailed_kinds:
        return {entry.get("kind") or "normal" for entry in detailed_kinds}
    return {"normal"}


def rust_source_metrics(crate_root: Path) -> tuple[int, int]:
    rust_loc = 0
    public_items = 0
    source_root = crate_root / "src"
    if not source_root.is_dir():
        return rust_loc, public_items
    for path in sorted(source_root.rglob("*.rs")):
        for line in path.read_text(encoding="utf-8").splitlines():
            if not line.strip():
                continue
            rust_loc += 1
            if PUBLIC_ITEM.match(line.strip()):
                public_items += 1
    return rust_loc, public_items


def workspace_boundaries(
    metadata: dict[str, Any],
) -> list[CrateBoundary]:
    packages = {
        package["name"]: package
        for package in metadata["packages"]
        if package["id"] in metadata["workspace_members"]
    }
    production_dependencies = {name: set() for name in packages}
    production_dependents = {name: set() for name in packages}
    dev_dependents = {name: set() for name in packages}

    for consumer, package in packages.items():
        for dependency in package["dependencies"]:
            provider = dependency["name"]
            if provider not in packages:
                continue
            kinds = dependency_kinds(dependency)
            if kinds - {"dev"}:
                production_dependencies[consumer].add(provider)
                production_dependents[provider].add(consumer)
            if "dev" in kinds:
                dev_dependents[provider].add(consumer)

    boundaries = []
    for name, package in packages.items():
        crate_root = Path(package["manifest_path"]).parent
        rust_loc, public_items = rust_source_metrics(crate_root)
        dev_only = dev_dependents[name] - production_dependents[name]
        boundaries.append(
            CrateBoundary(
                name=name,
                rust_loc=rust_loc,
                public_items=public_items,
                production_dependencies=tuple(sorted(production_dependencies[name])),
                production_dependents=tuple(sorted(production_dependents[name])),
                dev_only_dependents=tuple(sorted(dev_only)),
            )
        )
    return sorted(boundaries, key=lambda boundary: boundary.name)


def joined(values: Iterable[str]) -> str:
    return ",".join(values) or "-"


def write_tsv(boundaries: Iterable[CrateBoundary]) -> None:
    print(
        "crate\trust_loc\tpublic_items\tproduction_dependencies\t"
        "production_dependents\tdev_only_dependents"
    )
    for boundary in boundaries:
        print(
            f"{boundary.name}\t{boundary.rust_loc}\t{boundary.public_items}\t"
            f"{joined(boundary.production_dependencies)}\t"
            f"{joined(boundary.production_dependents)}\t"
            f"{joined(boundary.dev_only_dependents)}"
        )


def parse_args(arguments: Sequence[str] | None = None) -> argparse.Namespace:
    parser = argparse.ArgumentParser(
        prog="python3 -m tools report crate-boundaries",
        description=(
            "Report deterministic crate size and workspace dependency evidence. "
            "Counts non-empty Rust lines and lexical public item declarations in src/."
        )
    )
    parser.add_argument(
        "--max-rust-loc",
        type=int,
        help="show only crates at or below this source size",
    )
    parser.add_argument(
        "--max-production-dependents",
        type=int,
        help="show only crates with at most this many production consumers",
    )
    return parser.parse_args(arguments)


def main(arguments: Sequence[str] | None = None) -> int:
    args = parse_args(arguments)
    boundaries = workspace_boundaries(cargo_metadata(ROOT))
    if args.max_rust_loc is not None:
        boundaries = [
            boundary
            for boundary in boundaries
            if boundary.rust_loc <= args.max_rust_loc
        ]
    if args.max_production_dependents is not None:
        boundaries = [
            boundary
            for boundary in boundaries
            if len(boundary.production_dependents)
            <= args.max_production_dependents
        ]
    write_tsv(boundaries)
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
