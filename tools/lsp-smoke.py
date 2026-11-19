# SPDX-License-Identifier: GPL-3.0-or-later
"""Exercise a packaged Nia language server over stdio."""

import argparse
from pathlib import Path
import runpy
import subprocess
import tempfile
from time import perf_counter


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
                "capabilities": {"workspace": {
                    "semanticTokens": {"refreshSupport": True},
                    "didChangeWatchedFiles": {"dynamicRegistration": True},
                    "workspaceFolders": True,
                }},
            })["capabilities"]
            assert capabilities["positionEncoding"] == "utf-16"
            assert capabilities["workspaceSymbolProvider"] and capabilities["typeDefinitionProvider"]
            client.notify("initialized", {})

            canceled = client.request("workspace/symbol", {"query": "fixture::shapes::Shape"})
            client.notify("$/cancelRequest", {"id": canceled})
            _, canceled_response = client.wait(canceled)
            if "error" in canceled_response:
                assert canceled_response["error"]["code"] == -32800, canceled_response
            else:
                assert [symbol["name"] for symbol in canceled_response["result"]] == ["Shape"]

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
            assert "client/registerCapability" in client.server_requests
            deadline = perf_counter() + client.timeout
            while "workspace/semanticTokens/refresh" not in client.server_requests:
                client.receive(deadline)

            shapes.write_text("pub struct Shape { height: i32 }\n", encoding="utf-8")
            client.notify("workspace/didChangeWatchedFiles", {
                "changes": [{"uri": shapes.as_uri(), "type": 2}],
            })
            client.wait_diagnostics(main.as_uri(), 1, lambda items: any(
                diagnostic["severity"] == 1 and "width" in diagnostic["message"]
                for diagnostic in items
            ))
            start = field["position"]
            end = dict(start, character=start["character"] + len("width"))
            client.notify("textDocument/didChange", {
                "textDocument": dict(**document, version=2),
                "contentChanges": [{"range": {"start": start, "end": end}, "text": "height"}],
            })
            client.wait_diagnostics(main.as_uri(), 2, lambda items: not items)
            main.write_text(source.replace("width", "height"), encoding="utf-8")
            client.notify("textDocument/didSave", params)
            target = request(client, "textDocument/definition", field)
            assert target["uri"] == shapes.as_uri(), target

            client.notify("textDocument/didClose", params)
            client.wait_diagnostics(main.as_uri(), None, lambda items: not items)
            other = root / "next-workspace"
            other.mkdir()
            (other / "nia.toml").write_text(manifest.replace("fixture", "next"), encoding="utf-8")
            (other / "root.nia").write_text("pub struct NewWorkspace {}\n", encoding="utf-8")
            client.notify("workspace/didChangeWorkspaceFolders", {"event": {
                "added": [{"uri": other.as_uri(), "name": "next"}],
                "removed": [{"uri": root.as_uri(), "name": "fixture"}],
            }})
            assert not request(client, "workspace/symbol", {"query": "Shape"})
            symbols = request(client, "workspace/symbol", {"query": "next::NewWorkspace"})
            assert [symbol["location"]["uri"] for symbol in symbols] == [
                (other / "root.nia").as_uri()
            ], symbols

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
