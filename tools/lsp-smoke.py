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
    shapes_source = (
        "pub struct Shape { width: i32 }\n"
        "/// Returns the weight.\n"
        "pub fun weight() i32 { 1 }\n"
    )
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
            assert capabilities["documentFormattingProvider"]
            assert capabilities["workspaceSymbolProvider"] and capabilities["typeDefinitionProvider"]
            assert set(capabilities["workspace"]["fileOperations"]) == {"willRename", "didRename"}
            assert set(capabilities["codeActionProvider"]["codeActionKinds"]) == {
                "quickfix", "source.organizeImports",
            }
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
            formatting = dict(**params, options={"tabSize": 2, "insertSpaces": True})
            assert request(client, "textDocument/formatting", formatting) == [
                {"range": {"start": {"line": line, "character": 0},
                           "end": {"line": line, "character": 4}}, "newText": "  "}
                for line in (3, 4)
            ]
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
            tokens = request(client, "textDocument/semanticTokens/full", params)
            start = position(typo, "weigth")
            end = dict(start, character=start["character"] + len("weigth"))
            completions = request(client, "textDocument/completion", dict(
                **params, position=dict(start, character=start["character"] + len("weig")),
            ))
            completion, = completions
            assert completion["label"] == "weight", completion
            assert completion["detail"] == "fun weight() i32", completion
            assert completion["documentation"] == "Returns the weight.", completion
            assert completion["textEdit"] == {
                "range": {"start": start, "end": end}, "newText": "weight",
            }, completion
            _, diagnostics = client.wait_diagnostics(main.as_uri(), 3, lambda items: any(
                "unknown value `weigth`" in diagnostic["message"] for diagnostic in items
            ))
            assert request(client, "textDocument/semanticTokens/full", params) != tokens
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
            hover = request(client, "textDocument/hover", dict(**params, position=start))
            signature, docs = hover["contents"]
            assert signature == {"language": "nia", "value": "fun weight() i32"}
            assert docs == "Returns the weight."
            assert not request(client, "textDocument/codeAction", dict(
                **code_action, context={"diagnostics": diagnostics},
            ))

            missing = updated.replace("shape.height", "weight()")
            client.notify("textDocument/didChange", {
                "textDocument": dict(**document, version=5),
                "contentChanges": [{"text": missing}],
            })
            _, diagnostics = client.wait_diagnostics(main.as_uri(), 5, lambda items: any(
                "unknown value `weight`" in diagnostic["message"] for diagnostic in items
            ))
            start = position(missing, "weight")
            end = dict(start, character=start["character"] + len("weight"))
            quick_fix_at = dict(
                **params, range={"start": start, "end": end},
            )
            context = {"diagnostics": diagnostics, "only": ["quickfix"]}
            assert not request(client, "textDocument/codeAction", dict(
                **quick_fix_at, context=dict(**context, triggerKind=2),
            ))
            actions = request(client, "textDocument/codeAction", dict(
                **quick_fix_at, context=dict(**context, triggerKind=1),
            ))
            action, = actions
            assert action["title"] == "Import `root::shapes::weight`", actions
            change, = action["edit"]["documentChanges"]
            assert change["textDocument"] == dict(**document, version=5), change
            insertion, = change["edits"]
            assert insertion["newText"] == "using root::shapes::weight;\n\n", insertion
            client.notify("textDocument/didChange", {
                "textDocument": dict(**document, version=6),
                "contentChanges": [{"range": insertion["range"], "text": insertion["newText"]}],
            })
            client.wait_diagnostics(main.as_uri(), 6, lambda items: not items)

            unordered = updated.replace(
                "using std::process;",
                "using std::process;\nusing root::shapes as geo;",
            ).replace("shapes::Shape", "geo::Shape")
            client.notify("textDocument/didChange", {
                "textDocument": dict(**document, version=7),
                "contentChanges": [{"text": unordered}],
            })
            client.wait_diagnostics(main.as_uri(), 7, lambda items: not items)
            actions = request(client, "textDocument/codeAction", dict(
                **params, range={"start": {"line": 0, "character": 0},
                                 "end": {"line": 0, "character": 0}},
                context={"diagnostics": [], "only": ["source.organizeImports"]},
            ))
            action, = actions
            assert action["kind"] == "source.organizeImports", actions
            change, = action["edit"]["documentChanges"]
            assert change["textDocument"] == dict(**document, version=7), change
            reorder, = change["edits"]
            assert reorder["newText"] == "using root::shapes as geo;\nusing std::process;", reorder
            client.notify("textDocument/didChange", {
                "textDocument": dict(**document, version=8),
                "contentChanges": [{"range": reorder["range"], "text": reorder["newText"]}],
            })
            client.wait_diagnostics(main.as_uri(), 8, lambda items: not items)

            client.notify("textDocument/didClose", params)
            client.wait_diagnostics(main.as_uri(), None, lambda items: not items)
            for old_name, new_name in [("shapes", "geometry"), ("geometry", "Geometry")]:
                old_file = root / f"{old_name}.nia"
                renamed = root / f"{new_name}.nia"
                move = {"files": [{"oldUri": old_file.as_uri(), "newUri": renamed.as_uri()}]}
                edit = request(client, "workspace/willRenameFiles", move)
                assert set(edit) == {"documentChanges"}, edit
                change, = edit["documentChanges"]
                assert change["textDocument"]["uri"] == main.as_uri(), change
                assert [item["newText"] for item in change["edits"]] == [new_name, new_name], edit
                lines = updated.splitlines(keepends=True)
                for item in reversed(change["edits"]):
                    start, end = item["range"]["start"], item["range"]["end"]
                    assert start["line"] == end["line"]
                    line = lines[start["line"]]
                    assert line[start["character"]:end["character"]] == old_name
                    lines[start["line"]] = line[:start["character"]] + item["newText"] + line[end["character"]:]
                updated = "".join(lines)
                main.write_text(updated, encoding="utf-8")
                old_file.rename(renamed)
                assert renamed.name in {entry.name for entry in root.iterdir()}
                client.notify("workspace/didRenameFiles", move)
                symbols = request(client, "workspace/symbol", {"query": f"fixture::{new_name}::Shape"})
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
