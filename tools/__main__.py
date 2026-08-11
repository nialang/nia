import sys
from pathlib import Path


required = (Path(__file__).resolve().parents[1] / ".python-version").read_text(
    encoding="utf-8"
).strip()
running = f"{sys.version_info.major}.{sys.version_info.minor}"
if running != required:
    raise SystemExit(
        f"Nia maintenance tools require CPython {required}; running {running}"
    )


def run() -> int:
    from tools.nia_tools.cli import main

    return main()


raise SystemExit(run())
