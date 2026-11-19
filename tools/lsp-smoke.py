# SPDX-License-Identifier: GPL-3.0-or-later
"""Exercise a packaged Nia language server over stdio."""

import argparse
from pathlib import Path
import runpy
import subprocess
import tempfile


transport = runpy.run_path(str(Path(__file__).with_name("lsp-latency.py")))
Client = transport["Client"]
position = transport["position"]


def request(client, method, params):
    _, response = client.wait(client.request(method, params))
    if "error" in response:
        raise RuntimeError(f"{method}: {response['error']}")
    return response["result"]


def smoke(server, root):
    manifest = '[package]\nname = "fixture"\nversion = "0.1.0"\n[library]\npath = "root.nia"\n'
    source = (
        "module shapes;\n"
        "using std::process;\n"
        "fun analyze(init: process::Init, shape: &shapes::Shape) i32 {\n"
        "    _ = init;\n"
        '    _ = "😀"; shape.width\n'
        "}\n"
    )
    (root / "nia.toml").write_text(manifest, encoding="utf-8")
    main = root / "root.nia"
    main.write_text(source, encoding="utf-8")
    shapes = root / "shapes.nia"
    shapes.write_text("pub struct Shape { width: i32 }\n", encoding="utf-8")
    with subprocess.Popen([str(server)], cwd=root, stdin=subprocess.PIPE, stdout=subprocess.PIPE) as process:
        try:
            client = Client(process, 60)
            capabilities = request(client, "initialize", {
                "processId": None,
                "rootUri": root.as_uri(),
                "capabilities": {"workspace": {"semanticTokens": {"refreshSupport": True}}},
            })["capabilities"]
            assert capabilities["positionEncoding"] == "utf-16"
            assert capabilities["workspaceSymbolProvider"] and capabilities["typeDefinitionProvider"]
            client.notify("initialized", {})

            symbols = request(client, "workspace/symbol", {"query": "fixture::shapes::Shape"})
            assert [symbol["location"]["uri"] for symbol in symbols] == [shapes.as_uri()], symbols

            document = dict(uri=main.as_uri())
            params = dict(textDocument=document)
            client.notify("textDocument/didOpen", {
                "textDocument": dict(**document, version=1, languageId="nia", text=source),
            })
            assert request(client, "textDocument/semanticTokens/full", params)["data"]
            assert request(client, "textDocument/foldingRange", params)
            shape = dict(**params, position=position(source, "shape.width"))
            target = request(client, "textDocument/typeDefinition", shape)
            assert target["uri"] == shapes.as_uri(), target
            field = dict(**params, position=position(source, "width"))
            target = request(client, "textDocument/definition", field)
            assert target["uri"] == shapes.as_uri(), target
            assert target["range"]["start"] == {"line": 0, "character": 19}, target

            edit = request(client, "textDocument/rename", dict(**field, newName="height"))
            assert sorted(edit["changes"]) == sorted([main.as_uri(), shapes.as_uri()]), edit
            client.notify("textDocument/didClose", params)
            request(client, "shutdown", None)
            client.notify("exit", None)
            process.stdin.close()
            if process.wait(timeout=60):
                raise RuntimeError(f"{server.name} exited with status {process.returncode}")
        finally:
            if process.poll() is None:
                process.kill()


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("server", type=Path)
    args = parser.parse_args()
    with tempfile.TemporaryDirectory(prefix="nia-lsp-smoke-") as directory:
        smoke(args.server.resolve(strict=True), Path(directory).resolve())


if __name__ == "__main__":
    main()
