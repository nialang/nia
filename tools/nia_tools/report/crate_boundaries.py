#!/usr/bin/env python3
"""Report evidence for reviewing Nia workspace crate boundaries."""

from __future__ import annotations

import argparse
import re
import subprocess
from collections.abc import Sequence
from dataclasses import dataclass
from pathlib import Path
from typing import Iterable, NotRequired, TypedDict

from tools.nia_tools.common.json_data import JsonValue, decode_json
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


@dataclass(frozen=True)
class Options:
    max_rust_loc: int | None
    max_production_dependents: int | None


class DependencyKind(TypedDict, total=False):
    kind: str | None


class CargoDependency(TypedDict):
    name: str
    kind: NotRequired[str | None]
    dep_kinds: NotRequired[list[DependencyKind]]


class CargoPackage(TypedDict):
    name: str
    id: str
    manifest_path: str
    dependencies: list[CargoDependency]


class CargoMetadata(TypedDict):
    packages: list[CargoPackage]
    workspace_members: list[str]


def require_text(value: JsonValue | None, context: str) -> str:
    if not isinstance(value, str):
        raise ValueError(f"cargo metadata {context} is not text")
    return value


def parse_dependency(value: JsonValue, context: str) -> CargoDependency:
    if not isinstance(value, dict):
        raise ValueError(f"cargo metadata {context} is not an object")
    dependency: CargoDependency = {"name": require_text(value.get("name"), context)}
    kind = value.get("kind")
    if kind is not None and not isinstance(kind, str):
        raise ValueError(f"cargo metadata {context}.kind is not text or null")
    dependency["kind"] = kind
    detailed = value.get("dep_kinds")
    if detailed is not None:
        if not isinstance(detailed, list):
            raise ValueError(f"cargo metadata {context}.dep_kinds is not an array")
        parsed_kinds: list[DependencyKind] = []
        for index, entry in enumerate(detailed):
            if not isinstance(entry, dict):
                raise ValueError(
                    f"cargo metadata {context}.dep_kinds[{index}] is not an object"
                )
            entry_kind = entry.get("kind")
            if entry_kind is not None and not isinstance(entry_kind, str):
                raise ValueError(
                    f"cargo metadata {context}.dep_kinds[{index}].kind is invalid"
                )
            parsed_kinds.append({"kind": entry_kind})
        dependency["dep_kinds"] = parsed_kinds
    return dependency


def parse_metadata(value: JsonValue) -> CargoMetadata:
    if not isinstance(value, dict):
        raise ValueError("cargo metadata root is not an object")
    raw_packages = value.get("packages")
    raw_members = value.get("workspace_members")
    if not isinstance(raw_packages, list) or not isinstance(raw_members, list):
        raise ValueError("cargo metadata lacks package or workspace-member arrays")
    members = [require_text(member, "workspace member") for member in raw_members]
    packages: list[CargoPackage] = []
    for index, raw_package in enumerate(raw_packages):
        if not isinstance(raw_package, dict):
            raise ValueError(f"cargo metadata package {index} is not an object")
        raw_dependencies = raw_package.get("dependencies")
        if not isinstance(raw_dependencies, list):
            raise ValueError(f"cargo metadata package {index} lacks dependencies")
        packages.append(
            {
                "name": require_text(raw_package.get("name"), f"package {index}.name"),
                "id": require_text(raw_package.get("id"), f"package {index}.id"),
                "manifest_path": require_text(
                    raw_package.get("manifest_path"), f"package {index}.manifest_path"
                ),
                "dependencies": [
                    parse_dependency(dependency, f"package {index} dependency {dep_index}")
                    for dep_index, dependency in enumerate(raw_dependencies)
                ],
            }
        )
    return {"packages": packages, "workspace_members": members}


def cargo_metadata(root: Path) -> CargoMetadata:
    result = subprocess.run(
        ["cargo", "metadata", "--no-deps", "--format-version", "1"],
        cwd=root,
        check=True,
        stdout=subprocess.PIPE,
        text=True,
    )
    return parse_metadata(decode_json(result.stdout, "cargo metadata"))


def dependency_kinds(dependency: CargoDependency) -> set[str]:
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
    metadata: CargoMetadata,
) -> list[CrateBoundary]:
    packages: dict[str, CargoPackage] = {
        package["name"]: package
        for package in metadata["packages"]
        if package["id"] in metadata["workspace_members"]
    }
    production_dependencies: dict[str, set[str]] = {
        name: set() for name in packages
    }
    production_dependents: dict[str, set[str]] = {name: set() for name in packages}
    dev_dependents: dict[str, set[str]] = {name: set() for name in packages}

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

    boundaries: list[CrateBoundary] = []
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


def parse_args(arguments: Sequence[str] | None = None) -> Options:
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
    namespace = parser.parse_args(arguments)
    max_rust_loc = namespace.max_rust_loc
    max_dependents = namespace.max_production_dependents
    if max_rust_loc is not None and not isinstance(max_rust_loc, int):
        raise TypeError("argparse did not produce an int for --max-rust-loc")
    if max_dependents is not None and not isinstance(max_dependents, int):
        raise TypeError(
            "argparse did not produce an int for --max-production-dependents"
        )
    return Options(
        max_rust_loc=max_rust_loc,
        max_production_dependents=max_dependents,
    )


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
