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
    shapes_source = "pub struct Shape { width: i32 }\npub fun weight() i32 { 1 }\n"
    shapes.write_text(shapes_source, encoding="utf-8")
    with subprocess.Popen([str(server)], cwd=root, stdin=subprocess.PIPE, stdout=subprocess.PIPE) as process:
        try:
            client = Client(process, 60)
            capabilities = request(client, "initialize", {
                "processId": None,
                "rootUri": root.as_uri(),
                "capabilities": {
                    "workspace": {
                        "semanticTokens": {"refreshSupport": True},
                        "didChangeWatchedFiles": {"dynamicRegistration": True},
                        "fileOperations": {"willRename": True, "didRename": True},
                        "workspaceFolders": True,
                        "workspaceEdit": {"documentChanges": True},
                    },
                    "textDocument": {"codeAction": {
                        "codeActionLiteralSupport": {"codeActionKind": {"valueSet": ["quickfix"]}},
                    }},
                },
            })["capabilities"]
            assert capabilities["positionEncoding"] == "utf-16"
            assert capabilities["workspaceSymbolProvider"] and capabilities["typeDefinitionProvider"]
            assert set(capabilities["workspace"]["fileOperations"]) == {"willRename", "didRename"}
            assert capabilities["codeActionProvider"]["codeActionKinds"] == ["quickfix"]
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
            assert {item["textDocument"]["uri"] for item in edit["documentChanges"]} == {
                main.as_uri(), shapes.as_uri(),
            }, edit
            assert "client/registerCapability" in client.server_requests
            deadline = perf_counter() + client.timeout
            while "workspace/semanticTokens/refresh" not in client.server_requests:
                client.receive(deadline)

            shapes.write_text(shapes_source.replace("width", "height"), encoding="utf-8")
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

            updated = source.replace("width", "height")
            typo = updated.replace("shape.height", "shapes::weigth()")
            client.notify("textDocument/didChange", {
                "textDocument": dict(**document, version=3),
                "contentChanges": [{"text": typo}],
            })
            _, diagnostics = client.wait_diagnostics(main.as_uri(), 3, lambda items: any(
                "unknown value `weigth`" in diagnostic["message"] for diagnostic in items
            ))
            start = position(typo, "weigth")
            end = dict(start, character=start["character"] + len("weigth"))
            code_action = dict(**params, range={"start": start, "end": end})
            actions = request(client, "textDocument/codeAction", dict(
                **code_action, context={"diagnostics": diagnostics, "only": ["quickfix"]},
            ))
            assert len(actions) == 1, actions
            action = actions[0]
            assert action["kind"] == "quickfix" and "weight" in action["title"], action
            assert not action.get("isPreferred"), action
            change, = action["edit"]["documentChanges"]
            assert change["textDocument"] == dict(**document, version=3), change
            assert change["edits"] == [{"range": code_action["range"], "newText": "weight"}], change
            assert not request(client, "textDocument/codeAction", dict(
                **code_action, context={"diagnostics": diagnostics, "only": ["source.organizeImports"]},
            ))
            replacement = change["edits"][0]
            client.notify("textDocument/didChange", {
                "textDocument": dict(**document, version=4),
                "contentChanges": [{"range": replacement["range"], "text": replacement["newText"]}],
            })
            client.wait_diagnostics(main.as_uri(), 4, lambda items: not items)
            assert not request(client, "textDocument/codeAction", dict(
                **code_action, context={"diagnostics": diagnostics},
            ))

            client.notify("textDocument/didClose", params)
            client.wait_diagnostics(main.as_uri(), None, lambda items: not items)
            renamed = root / "geometry.nia"
            move = {"files": [{"oldUri": shapes.as_uri(), "newUri": renamed.as_uri()}]}
            edit = request(client, "workspace/willRenameFiles", move)
            assert set(edit) == {"documentChanges"}, edit
            change, = edit["documentChanges"]
            assert change["textDocument"]["uri"] == main.as_uri(), change
            assert [item["newText"] for item in change["edits"]] == [
                "geometry", "geometry",
            ], edit
            lines = updated.splitlines(keepends=True)
            for item in reversed(change["edits"]):
                start, end = item["range"]["start"], item["range"]["end"]
                assert start["line"] == end["line"]
                line = lines[start["line"]]
                assert line[start["character"]:end["character"]] == "shapes"
                lines[start["line"]] = line[:start["character"]] + item["newText"] + line[end["character"]:]
            main.write_text("".join(lines), encoding="utf-8")
            shapes.rename(renamed)
            client.notify("workspace/didRenameFiles", move)
            symbols = request(client, "workspace/symbol", {"query": "fixture::geometry::Shape"})
            assert [symbol["location"]["uri"] for symbol in symbols] == [renamed.as_uri()], symbols

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
