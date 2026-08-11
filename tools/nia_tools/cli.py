from __future__ import annotations

import subprocess
import sys
import unittest
from collections.abc import Callable, Sequence
from dataclasses import dataclass

from tools.nia_tools.audit import compatibility, std_build_host
from tools.nia_tools.baseline import build, compare, compiler
from tools.nia_tools.report import crate_boundaries
from tools.nia_tools.repository import REPOSITORY_ROOT
from tools.nia_tools.runtime import require_python_version


CommandHandler = Callable[[Sequence[str] | None], int]


@dataclass(frozen=True)
class Command:
    path: tuple[str, ...]
    summary: str
    handler: CommandHandler


def check(arguments: Sequence[str] | None = None) -> int:
    if arguments:
        raise SystemExit("usage: python3 -m tools check")

    tools_root = REPOSITORY_ROOT / "tools"
    typechecker = tools_root / "node_modules" / ".bin" / "pyright"
    if not typechecker.is_file():
        print(
            "Pyright is not installed; run `npm ci --prefix tools --ignore-scripts`",
            file=sys.stderr,
        )
        return 1
    typecheck = subprocess.run(
        [str(typechecker), "--project", str(tools_root / "pyrightconfig.json")],
        cwd=tools_root,
        check=False,
    )
    if typecheck.returncode != 0:
        return typecheck.returncode

    suite = unittest.defaultTestLoader.discover(
        str(REPOSITORY_ROOT / "tools" / "tests"),
        top_level_dir=str(REPOSITORY_ROOT),
    )
    result = unittest.TextTestRunner(verbosity=1).run(suite)
    if not result.wasSuccessful():
        return 1
    compatibility_errors = compatibility.audit(REPOSITORY_ROOT)
    if compatibility_errors:
        print("compatibility audit failed:", file=sys.stderr)
        print("\n".join(compatibility_errors), file=sys.stderr)
        return 1
    return std_build_host.main(())


COMMANDS = (
    Command(("audit", "compatibility"), "check compatibility identities", compatibility.main),
    Command(("audit", "std-build-host"), "check the std build-host closure", std_build_host.main),
    Command(("report", "crate-boundaries"), "report workspace crate evidence", crate_boundaries.main),
    Command(("baseline", "compiler"), "collect compiler performance samples", compiler.main),
    Command(("baseline", "compare"), "compare compiler performance samples", compare.main),
    Command(("baseline", "build"), "collect the representative build baseline", build.main),
    Command(("check",), "run all fast tool tests and audits", check),
)


def usage(prefix: tuple[str, ...] = ()) -> str:
    lines = ["usage: python3 -m tools <command> [options]", "", "commands:"]
    for command in COMMANDS:
        if command.path[: len(prefix)] != prefix:
            continue
        suffix = command.path[len(prefix) :]
        if not suffix:
            continue
        lines.append(f"  {' '.join(suffix):24} {command.summary}")
    return "\n".join(lines)


def main(arguments: Sequence[str] | None = None) -> int:
    require_python_version()
    values = tuple(sys.argv[1:] if arguments is None else arguments)
    if not values or values == ("--help",) or values == ("-h",):
        print(usage())
        return 0

    for command in COMMANDS:
        if values[: len(command.path)] == command.path:
            return command.handler(values[len(command.path) :])

    if len(values) == 2 and values[1] in {"--help", "-h"}:
        prefix = (values[0],)
        if any(command.path[:1] == prefix for command in COMMANDS):
            print(usage(prefix))
            return 0

    print(f"unknown maintenance command: {' '.join(values)}", file=sys.stderr)
    print(usage(), file=sys.stderr)
    return 2
