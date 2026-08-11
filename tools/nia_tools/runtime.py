from __future__ import annotations

import sys
from dataclasses import dataclass

from tools.nia_tools.repository import PYTHON_VERSION_FILE


@dataclass(frozen=True, order=True)
class PythonVersion:
    major: int
    minor: int

    @classmethod
    def parse(cls, value: str) -> PythonVersion:
        parts = value.strip().split(".")
        if len(parts) != 2 or not all(part.isdecimal() for part in parts):
            raise ValueError(".python-version must contain one major.minor version")
        return cls(*(int(part) for part in parts))

    def __str__(self) -> str:
        return f"{self.major}.{self.minor}"


def required_python_version() -> PythonVersion:
    return PythonVersion.parse(PYTHON_VERSION_FILE.read_text(encoding="utf-8"))


def require_python_version() -> None:
    required = required_python_version()
    running = PythonVersion(sys.version_info.major, sys.version_info.minor)
    if running != required:
        raise SystemExit(
            f"Nia maintenance tools require CPython {required}; running {running}"
        )
