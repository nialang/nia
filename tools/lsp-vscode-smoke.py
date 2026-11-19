# SPDX-License-Identifier: GPL-3.0-or-later
"""Test an installed Nia release and VSIX through a real VS Code extension host."""

import argparse
import json
import os
from pathlib import Path
import shutil
import subprocess
import tempfile


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--code", required=True, type=Path, help="VS Code desktop executable")
    parser.add_argument("--cli", default="code", help="VS Code CLI for installing the VSIX")
    parser.add_argument("server", type=Path, help="nia-lsp from the unpacked release")
    parser.add_argument("vsix", type=Path)
    args = parser.parse_args()

    executable = args.code.resolve(strict=True)
    server = args.server.resolve(strict=True)
    vsix = args.vsix.resolve(strict=True)
    cli = shutil.which(args.cli)
    if cli is None:
        parser.error(f"cannot find VS Code CLI {args.cli}")

    tools = Path(__file__).resolve().parent
    harness = tools / "lsp-vscode-smoke"
    with tempfile.TemporaryDirectory(prefix="nia-vscode-smoke-") as directory:
        root = Path(directory)
        user = root / "user"
        settings = user / "User" / "settings.json"
        settings.parent.mkdir(parents=True)
        settings.write_text(json.dumps({
            "nia.server.path": str(server),
            "workbench.iconTheme": "nia",
            "telemetry.telemetryLevel": "off",
            "extensions.autoUpdate": False,
            "extensions.autoCheckUpdates": False,
        }), encoding="utf-8")
        workspace = root / "workspace"
        (workspace / "source").mkdir(parents=True)
        sources = {
            "nia.toml": '[package]\nname = "file_move"\nversion = "1.0.0"\n[library]\npath = "root.nia"\n',
            "root.nia": "module source;\nfun value() i32 {\n    source::nested::read()\n}\n",
            "source.nia": "pub module nested;\n",
            "source/nested.nia": "pub fun read() i32 { 42 }\n",
        }
        for path, source in sources.items():
            (workspace / path).write_text(source, encoding="utf-8")

        profile = [f"--user-data-dir={user}", f"--extensions-dir={root / 'extensions'}"]
        subprocess.run([cli, *profile, "--install-extension", str(vsix)], check=True, timeout=60)
        result = root / "result.txt"
        env = dict(os.environ, NIA_SMOKE_RESULT=str(result),
                   NIA_SMOKE_ICON=str(tools.parent / "assets/brand/nia-icon.svg"))
        completed = subprocess.run([
            str(executable), *profile, f"--extensionDevelopmentPath={harness}",
            f"--extensionTestsPath={harness / 'run.js'}",
            "--skip-welcome", "--skip-release-notes", "--disable-workspace-trust", str(workspace),
        ], env=env, timeout=60)
        if result.is_file():
            print(result.read_text(encoding="utf-8").strip())
        completed.check_returncode()
        if not result.is_file():
            raise RuntimeError("VS Code exited before reporting the smoke test result")


if __name__ == "__main__":
    main()
