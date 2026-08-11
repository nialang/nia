#!/usr/bin/env python3
"""Audit Nia compatibility identities and fingerprint domain spelling."""

from __future__ import annotations

import argparse
import ast
import os
import re
import tomllib
from collections.abc import Sequence
from pathlib import Path

from tools.nia_tools.repository import REPOSITORY_ROOT

ROOT = REPOSITORY_ROOT
REGISTRY = Path("crates/nia-compat/src/lib.rs")
DOMAIN_IDENTITY = (
    r"nia\.[a-z0-9]+(?:-[a-z0-9]+)*"
    r"(?:\.[a-z0-9]+(?:-[a-z0-9]+)*)*\.v[1-9][0-9]*"
)
DOMAIN = re.compile(rf"^{DOMAIN_IDENTITY}$")
STRING = re.compile(r'"((?:\\.|[^"\\\r\n])*)"')
CONSTRUCTOR_DOMAIN = re.compile(
    r"(?:QueryFingerprintBuilder|Encoder)::new\s*\(\s*"
    r'"((?:\\.|[^"\\\r\n])*)"',
    re.DOTALL,
)
TYPED_DOMAIN_DECLARATION = re.compile(
    r"\bconst\s+([A-Z][A-Z0-9_]*)\s*:\s*FingerprintDomain\s*=\s*"
    r"FingerprintDomain::new\s*\(\s*"
    rf'"({DOMAIN_IDENTITY})"\s*\)',
    re.DOTALL,
)
BYTE_STRING = re.compile(r'b"((?:\\.|[^"\\\r\n])*)"')
FORBIDDEN_IDENTITY_NAMES = (
    "RESOURCE_LAYOUT_SCHEMA",
    "STD_SCHEMA",
    "BUILD_PROTOCOL_SCHEMA",
    "BUILD_PLAN_SCHEMA_VERSION",
    "MANGLE_ABI_VERSION",
    "CODEGEN_ABI_VERSION",
    "RUNNER_CONFIG_SCHEMA_VERSION",
    "RUNNER_CONFIG_MAGIC",
    "RUNNER_CONFIG_MAGIC_TEXT",
    "FRONTEND_CACHE_SCHEMA",
    "OBJECT_WORK_PRODUCT_SCHEMA",
    "LINK_RESULT_SCHEMA",
    "ARCHIVE_SCHEMA",
)
TEXT_SUFFIXES = {
    ".json",
    ".md",
    ".meta",
    ".nia",
    ".py",
    ".rs",
    ".sh",
    ".toml",
    ".txt",
    ".yaml",
    ".yml",
}
VERSION_AUTHORITIES = {
    Path("Cargo.toml"),
    Path("Cargo.lock"),
    Path("lib/toolchain.meta"),
}


def source_files(root: Path) -> list[Path]:
    files = []
    for directory, names, filenames in os.walk(root):
        names[:] = [name for name in names if name not in {".git", "target"}]
        files.extend(Path(directory) / filename for filename in filenames)
    return files


def rust_sources(root: Path) -> list[Path]:
    crates = root / "crates"
    return sorted(crates.glob("*/src/**/*.rs")) if crates.is_dir() else []


def production_rust_sources(root: Path) -> list[Path]:
    return [
        path
        for path in rust_sources(root)
        if path.name != "tests.rs" and "tests" not in path.relative_to(root).parts
    ]


def production_source(path: Path) -> str:
    source = path.read_text(encoding="utf-8")
    test_module = re.search(r"#\[cfg\(test\)\]\s*mod\s+tests\s*\{", source)
    return source[: test_module.start()] if test_module else source


def decode_string(value: str) -> str:
    return value


def registered_magics(root: Path) -> set[bytes]:
    source = (root / REGISTRY).read_text(encoding="utf-8")
    magics = set()
    for match in BYTE_STRING.finditer(source):
        if not match.group(1).startswith("NIA"):
            continue
        value = ast.literal_eval(f'b"{match.group(1)}"')
        if len(value) == 8:
            magics.add(value)
    return magics


def fingerprint_domain_errors(root: Path) -> list[str]:
    errors = []
    for path in production_rust_sources(root):
        source = production_source(path)
        relative = path.relative_to(root)
        checked = set()
        for match in CONSTRUCTOR_DOMAIN.finditer(source):
            domain = decode_string(match.group(1))
            checked.add((match.start(1), domain))
            if not DOMAIN.fullmatch(domain):
                errors.append(f"{relative}: invalid fingerprint domain `{domain}`")
        for match in STRING.finditer(source):
            value = decode_string(match.group(1))
            if not value.startswith("nia.") or value == "nia.compiler_builtins":
                continue
            if not DOMAIN.fullmatch(value) and (match.start(1), value) not in checked:
                errors.append(f"{relative}: invalid Nia identity string `{value}`")
    return errors


def typed_fingerprint_domain_errors(root: Path) -> list[str]:
    errors = []
    declarations: dict[str, list[str]] = {}
    for path in production_rust_sources(root):
        source = production_source(path)
        relative = path.relative_to(root)
        declaration_spans = []
        for match in TYPED_DOMAIN_DECLARATION.finditer(source):
            declaration_spans.append(match.span(2))
            declarations.setdefault(match.group(2), []).append(
                f"{relative}:{source.count(chr(10), 0, match.start()) + 1}"
            )
        for match in CONSTRUCTOR_DOMAIN.finditer(source):
            errors.append(
                f"{relative}: fingerprint builder receives raw literal `{match.group(1)}`"
            )
        for match in STRING.finditer(source):
            value = match.group(1)
            if not DOMAIN.fullmatch(value):
                continue
            if not any(start <= match.start(1) < end for start, end in declaration_spans):
                errors.append(
                    f"{relative}: fingerprint domain `{value}` is not an owner-local typed constant"
                )
    for domain, owners in declarations.items():
        if len(owners) > 1:
            errors.append(
                f"duplicate fingerprint domain `{domain}` declared at {', '.join(owners)}"
            )
    return errors


def global_identity_errors(root: Path) -> list[str]:
    errors = []
    registry_path = root / REGISTRY
    magics = registered_magics(root)
    for path in rust_sources(root):
        if path == registry_path:
            continue
        source = path.read_text(encoding="utf-8")
        relative = path.relative_to(root)
        for match in BYTE_STRING.finditer(source):
            if not match.group(1).startswith("NIA"):
                continue
            value = ast.literal_eval(f'b"{match.group(1)}"')
            if value in magics:
                errors.append(f"{relative}: registered magic is defined outside nia-compat")
        for name in FORBIDDEN_IDENTITY_NAMES:
            if re.search(rf"\b(?:pub(?:\([^)]*\))?\s+)?const\s+{name}\b", source):
                errors.append(f"{relative}: obsolete compatibility identity `{name}`")
        if "CARGO_PKG_VERSION" in source:
            errors.append(f"{relative}: release version is read outside nia-compat")
    return errors


def workspace_version(root: Path) -> str:
    manifest = tomllib.loads((root / "Cargo.toml").read_text(encoding="utf-8"))
    return manifest["workspace"]["package"]["version"]


def release_version_errors(root: Path) -> list[str]:
    version = workspace_version(root)
    errors = []
    for path in source_files(root):
        relative = path.relative_to(root)
        if relative in VERSION_AUTHORITIES or path.suffix not in TEXT_SUFFIXES:
            continue
        try:
            source = path.read_text(encoding="utf-8")
        except UnicodeDecodeError:
            continue
        if version in source:
            errors.append(f"{relative}: workspace version `{version}` is duplicated")
    return errors


def audit(root: Path = ROOT) -> list[str]:
    return [
        *fingerprint_domain_errors(root),
        *typed_fingerprint_domain_errors(root),
        *global_identity_errors(root),
        *release_version_errors(root),
    ]


def parse_args(arguments: Sequence[str] | None = None) -> argparse.Namespace:
    parser = argparse.ArgumentParser(
        prog="python3 -m tools audit compatibility", description=__doc__
    )
    parser.add_argument("--root", type=Path, default=ROOT)
    return parser.parse_args(arguments)


def main(arguments: Sequence[str] | None = None) -> int:
    errors = audit(parse_args(arguments).root.resolve())
    if errors:
        raise SystemExit("compatibility audit failed:\n" + "\n".join(errors))
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
