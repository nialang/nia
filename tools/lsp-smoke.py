# SPDX-License-Identifier: GPL-3.0-or-later
"""Exercise a packaged Nia language server over stdio."""

import argparse
import json
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
        'fun measure(low: i32) i32 { _="\U0001f34e"; shapes::distance(low, 10) }\n'
    )
    (root / "nia.toml").write_text(manifest, encoding="utf-8")
    main = root / "root.nia"
    main.write_text(source, encoding="utf-8")
    shapes = root / "shapes.nia"
    shapes_source = (
        "pub struct Shape { width: i32 }\n"
        "/// Returns the weight.\n"
        "pub fun weight() i32 { 1 }\n"
        "pub fun distance(low: i32, high: i32) i32 { high - low }\n"
    )
    shapes.write_text(shapes_source, encoding="utf-8")
    (root / "extensions.nia").write_text(
        "extend root::shapes::Shape { pub fun measureHeight(&self) i32 { 42 } }\n",
        encoding="utf-8",
    )
    (root / "other_extensions.nia").write_text(
        "struct Other {}\nextend Other { pub fun measureHeight(&self) i32 { 0 } }\n",
        encoding="utf-8",
    )
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
                    "textDocument": {
                        "codeAction": {
                            "codeActionLiteralSupport": {"codeActionKind": {"valueSet": ["quickfix"]}},
                        },
                        "semanticTokens": {
                            "requests": {"full": {"delta": True}, "range": True},
                            "tokenTypes": [], "tokenModifiers": [], "formats": ["relative"],
                        },
                    },
                },
            })["capabilities"]
            assert capabilities["positionEncoding"] == "utf-16"
            assert capabilities["documentFormattingProvider"]
            assert capabilities["documentRangeFormattingProvider"]
            assert capabilities["inlayHintProvider"]
            assert capabilities["callHierarchyProvider"]
            assert capabilities["semanticTokensProvider"]["full"]["delta"]
            assert capabilities["semanticTokensProvider"]["range"]
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
            before_eq = position(source, '="')
            after_eq = dict(before_eq, character=before_eq["character"] + 1)
            indent_edits = [
                {"range": {"start": {"line": line, "character": 0},
                           "end": {"line": line, "character": 4}}, "newText": "  "}
                for line in (3, 4)
            ]
            operator_edits = [{"range": {"start": point, "end": point}, "newText": " "}
                              for point in (before_eq, after_eq)]
            assert request(client, "textDocument/formatting", formatting) == indent_edits + operator_edits
            assert request(client, "textDocument/rangeFormatting", dict(**formatting, range={
                "start": position(source, "shape.width"), "end": {"line": 5, "character": 0},
            })) == indent_edits[1:]
            assert request(client, "textDocument/rangeFormatting", dict(**formatting, range={
                "start": position(source, "shapes::distance"), "end": {"line": 7, "character": 0},
            })) == operator_edits
            shape = dict(**params, position=position(source, "shape.width"))
            target = request(client, "textDocument/typeDefinition", shape)
            assert target["uri"] == shapes.as_uri(), target
            field = dict(**params, position=position(source, "width"))
            target = request(client, "textDocument/definition", field)
            assert target["uri"] == shapes.as_uri(), target
            assert target["range"]["start"] == {"line": 0, "character": 19}, target
            hint_position = position(source, "10)")
            hints = request(client, "textDocument/inlayHint", dict(
                **params, range={"start": {"line": 6, "character": 0},
                                 "end": {"line": 7, "character": 0}},
            ))
            assert hints == [{
                "label": "high:", "position": hint_position, "kind": 2, "paddingRight": True,
            }], hints
            callee, = request(client, "textDocument/prepareCallHierarchy", dict(
                **params, position=position(source, "distance"),
            ))
            assert callee["name"] == "distance" and callee["uri"] == shapes.as_uri(), callee
            incoming, = request(client, "callHierarchy/incomingCalls", {"item": callee})
            assert incoming["from"]["name"] == "measure", incoming
            outgoing, = request(client, "callHierarchy/outgoingCalls", {"item": incoming["from"]})
            assert outgoing["to"] == callee, outgoing
            assert outgoing["fromRanges"] == incoming["fromRanges"] == [{
                "start": position(source, "shapes::distance"),
                "end": dict(hint_position, character=hint_position["character"] + 3),
            }], outgoing

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
            _, stale = client.wait(client.request("callHierarchy/incomingCalls", {"item": callee}))
            assert stale["error"]["code"] == -32801, stale

            updated = source.replace("width", "height")
            typo = updated.replace("shape.height", "shapes::weigth()")
            client.notify("textDocument/didChange", {
                "textDocument": dict(**document, version=3),
                "contentChanges": [{"text": typo}],
            })
            tokens = request(client, "textDocument/semanticTokens/full", params)
            region = dict(**params, range={
                "start": {"line": 4, "character": 0},
                "end": {"line": 5, "character": 0},
            })
            visible_tokens = request(client, "textDocument/semanticTokens/range", region)
            assert "resultId" not in visible_tokens
            assert 0 < len(visible_tokens["data"]) < len(tokens["data"]), visible_tokens
            line = 0
            column = 0
            full_line = []
            for index in range(0, len(tokens["data"]), 5):
                token = tokens["data"][index:index + 5]
                line += token[0]
                column = token[1] if token[0] else column + token[1]
                if line == 4:
                    full_line.extend(token if full_line else [line, column, *token[2:]])
            assert visible_tokens["data"] == full_line, visible_tokens
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
            delta = request(client, "textDocument/semanticTokens/full/delta", dict(
                **params, previousResultId=tokens["resultId"],
            ))
            assert delta["edits"], delta
            applied = tokens["data"].copy()
            for edit in reversed(delta["edits"]):
                start_index = edit["start"]
                applied[start_index:start_index + edit["deleteCount"]] = edit.get("data", [])
            unchanged = request(client, "textDocument/semanticTokens/full/delta", dict(
                **params, previousResultId=delta["resultId"],
            ))
            assert unchanged["edits"] == [], unchanged
            fallback = request(client, "textDocument/semanticTokens/full/delta", dict(
                **params, previousResultId=tokens["resultId"],
            ))
            assert fallback["data"] == applied and applied != tokens["data"], fallback
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

            partial = updated.replace("shape.height", "weig()")
            client.notify("textDocument/didChange", {
                "textDocument": dict(**document, version=5),
                "contentChanges": [{"text": partial}],
            })
            start = position(partial, "weig")
            cursor = dict(start, character=start["character"] + len("weig"))
            completion, = request(client, "textDocument/completion", dict(**params, position=cursor))
            assert completion["label"] == "weight", completion
            assert completion["textEdit"] == {
                "range": {"start": start, "end": cursor}, "newText": "weight",
            }, completion
            auto_import, = completion["additionalTextEdits"]
            assert auto_import["newText"] == "using root::shapes::weight;\n\n", completion
            client.notify("textDocument/didChange", {
                "textDocument": dict(**document, version=6),
                "contentChanges": [
                    {"range": edit["range"], "text": edit["newText"]}
                    for edit in [completion["textEdit"], auto_import]
                ],
            })
            client.wait_diagnostics(main.as_uri(), 6, lambda items: not items)
            target = request(client, "textDocument/definition", dict(
                **params, position=dict(start, line=start["line"] + 2),
            ))
            assert target["uri"] == shapes.as_uri(), target

            missing = updated.replace("shape.height", "weight()")
            client.notify("textDocument/didChange", {
                "textDocument": dict(**document, version=7),
                "contentChanges": [{"text": missing}],
            })
            _, diagnostics = client.wait_diagnostics(main.as_uri(), 7, lambda items: any(
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
            assert change["textDocument"] == dict(**document, version=7), change
            insertion, = change["edits"]
            assert insertion["newText"] == "using root::shapes::weight;\n\n", insertion
            client.notify("textDocument/didChange", {
                "textDocument": dict(**document, version=8),
                "contentChanges": [{"range": insertion["range"], "text": insertion["newText"]}],
            })
            client.wait_diagnostics(main.as_uri(), 8, lambda items: not items)

            for name, declaration in [
                ("alternate", "pub fun weight() i32 { 2 }\n"),
                ("incompatible", "pub fun weight(limit: i32) i32 { limit }\n"),
            ]:
                (root / f"{name}.nia").write_text(declaration, encoding="utf-8")
            ambiguous = missing + "\nmodule alternate;\nmodule incompatible;\n"
            client.notify("textDocument/didChange", {
                "textDocument": dict(**document, version=9),
                "contentChanges": [{"text": ambiguous}],
            })
            _, diagnostics = client.wait_diagnostics(main.as_uri(), 9, lambda items: any(
                "unknown value `weight`" in diagnostic["message"] for diagnostic in items
            ))
            actions = request(client, "textDocument/codeAction", dict(
                **quick_fix_at, context={"diagnostics": diagnostics, "only": ["quickfix"], "triggerKind": 1},
            ))
            assert [action["title"] for action in actions] == [
                "Import `root::alternate::weight`", "Import `root::shapes::weight`",
            ], actions
            completions = request(client, "textDocument/completion", dict(**params, position=end))
            assert [item["labelDetails"]["description"] for item in completions] == [
                "root::alternate::weight", "root::shapes::weight",
            ], completions

            unordered = updated.replace(
                "using std::process;",
                "using std::process;\n"
                "using root::shapes::{weight as w, distance as d};\n"
                "using root::shapes as geo;",
            ).replace("shapes::Shape", "geo::Shape").replace(
                "shapes::distance(low, 10)", "d(low, 10) + w()",
            )
            unordered += "\n// Public API\npub using root::shapes::{weight as mass, distance as width};\n"
            unordered += (
                "fun local() i32 {\n"
                "    using root::shapes::weight as measured_weight;\n"
                "    using root::shapes::distance as measured_distance;\n"
                "    measured_weight() + measured_distance(2, 4)\n"
                "}\n"
            )
            client.notify("textDocument/didChange", {
                "textDocument": dict(**document, version=10),
                "contentChanges": [{"text": unordered}],
            })
            client.wait_diagnostics(main.as_uri(), 10, lambda items: not items)
            actions = request(client, "textDocument/codeAction", dict(
                **params, range={"start": {"line": 0, "character": 0},
                                 "end": {"line": 0, "character": 0}},
                context={"diagnostics": [], "only": ["source.organizeImports"]},
            ))
            action, = actions
            assert action["kind"] == "source.organizeImports", actions
            change, = action["edit"]["documentChanges"]
            assert change["textDocument"] == dict(**document, version=10), change
            reorder, public, local = change["edits"]
            assert reorder["newText"] == (
                "using root::shapes as geo;\n"
                "using root::shapes::{distance as d, weight as w};\n"
                "using std::process;"
            ), reorder
            assert public["newText"] == "distance as width, weight as mass", public
            assert local["newText"] == (
                "using root::shapes::distance as measured_distance;\n"
                "    using root::shapes::weight as measured_weight;"
            ), local
            client.notify("textDocument/didChange", {
                "textDocument": dict(**document, version=11),
                "contentChanges": [
                    {"range": edit["range"], "text": edit["newText"]}
                    for edit in reversed(change["edits"])
                ],
            })
            client.wait_diagnostics(main.as_uri(), 11, lambda items: not items)

            member_source = updated.replace("shape.height", "shape.he")
            client.notify("textDocument/didChange", {
                "textDocument": dict(**document, version=12),
                "contentChanges": [{"text": member_source}],
            })
            start = position(member_source, "he\n")
            end = dict(start, character=start["character"] + 2)
            completions = request(client, "textDocument/completion", dict(
                **params, position=end, context={"triggerKind": 1},
            ))
            if isinstance(completions, dict):
                assert completions["isIncomplete"], completions
                completions = completions["items"]
            height, = [item for item in completions if item["label"] == "height"]
            assert height["textEdit"] == {
                "range": {"start": start, "end": end}, "newText": "height",
            }, height
            client.wait_diagnostics(main.as_uri(), 12, lambda items: any(
                diagnostic["severity"] == 1 and "`he`" in diagnostic["message"]
                for diagnostic in items
            ))
            completions = request(client, "textDocument/completion", dict(
                **params, position=end, context={"triggerKind": 3},
            ))
            assert [item["label"] for item in completions] == ["height"], completions

            missing_method = updated.replace(
                "shape.height", "shape.measureHeight()",
            ) + "module extensions;\nmodule other_extensions;\n"
            client.notify("textDocument/didChange", {
                "textDocument": dict(**document, version=13),
                "contentChanges": [{"text": missing_method}],
            })
            _, diagnostics = client.wait_diagnostics(main.as_uri(), 13, lambda items: any(
                "unknown method `measureHeight`" in diagnostic["message"] for diagnostic in items
            ))
            missing, = [item for item in diagnostics if "unknown method" in item["message"]]
            fix, = request(client, "textDocument/codeAction", dict(
                **params, range=missing["range"],
                context={"diagnostics": [missing], "only": ["quickfix"]},
            ))
            assert fix["title"] == "Import `root::extensions`", fix
            change, = fix["edit"]["documentChanges"]
            assert change["textDocument"] == dict(**document, version=13), change
            assert change["edits"][0]["newText"] == "using root::extensions;\n\n", change
            client.notify("textDocument/didChange", {
                "textDocument": dict(**document, version=14),
                "contentChanges": [
                    {"range": edit["range"], "text": edit["newText"]} for edit in change["edits"]
                ],
            })
            client.wait_diagnostics(main.as_uri(), 14, lambda items: not items)

            client.notify("textDocument/didClose", params)
            client.wait_diagnostics(main.as_uri(), None, lambda items: not items)
            palette = root / "palette.nia"
            palette.write_text("pub struct Palette {}\n", encoding="utf-8")
            updated += "module palette;\nfun color() palette::Palette { .{} }\n"
            main.write_text(updated, encoding="utf-8")
            client.notify("workspace/didChangeWatchedFiles", {
                "changes": [{"uri": main.as_uri(), "type": 2}, {"uri": palette.as_uri(), "type": 1}],
            })
            unsaved = shapes.read_text(encoding="utf-8") + "\npub struct BufferOnly {}\n"
            client.notify("textDocument/didOpen", {"textDocument": {
                "uri": shapes.as_uri(), "languageId": "nia", "version": 11, "text": unsaved,
            }})
            client.wait_diagnostics(shapes.as_uri(), 11, lambda items: not items)
            for old_name, new_name in [("shapes", "geometry"), ("geometry", "Geometry")]:
                old_file = root / f"{old_name}.nia"
                renamed = root / f"{new_name}.nia"
                pairs = [(old_file, renamed)]
                if old_name == "shapes":
                    pairs.append((palette, root / "colors.nia"))
                names = {old.stem: new.stem for old, new in pairs}
                move = {"files": [
                    {"oldUri": old.as_uri(), "newUri": new.as_uri()} for old, new in pairs
                ]}
                edit = request(client, "workspace/willRenameFiles", move)
                assert set(edit) == {"documentChanges"}, edit
                change, = edit["documentChanges"]
                assert change["textDocument"]["uri"] == main.as_uri(), change
                assert [item["newText"] for item in change["edits"]] == [
                    new for old, new in names.items()
                    for _ in range(2 if old == "palette" else 3)
                ], edit
                lines = updated.splitlines(keepends=True)
                for item in reversed(change["edits"]):
                    start, end = item["range"]["start"], item["range"]["end"]
                    assert start["line"] == end["line"]
                    line = lines[start["line"]]
                    utf16 = line.encode("utf-16-le")
                    left = len(utf16[:2 * start["character"]].decode("utf-16-le"))
                    right = len(utf16[:2 * end["character"]].decode("utf-16-le"))
                    before = line[left:right]
                    assert item["newText"] == names[before], item
                    lines[start["line"]] = line[:left] + item["newText"] + line[right:]
                updated = "".join(lines)
                main.write_text(updated, encoding="utf-8")
                for old, new in pairs:
                    old.rename(new)
                assert renamed.name in {entry.name for entry in root.iterdir()}
                client.notify("workspace/didRenameFiles", move)
                client.wait_diagnostics(old_file.as_uri(), None, lambda items: not items)
                client.wait_diagnostics(renamed.as_uri(), 11, lambda items: not items)
                symbols = request(client, "workspace/symbol", {"query": f"fixture::{new_name}::Shape"})
                assert [symbol["location"]["uri"] for symbol in symbols] == [renamed.as_uri()], symbols
                symbols = request(client, "workspace/symbol", {"query": f"fixture::{new_name}::BufferOnly"})
                assert [symbol["location"]["uri"] for symbol in symbols] == [renamed.as_uri()], symbols
                symbols = request(client, "workspace/symbol", {"query": "fixture::colors::Palette"})
                assert [symbol["location"]["uri"] for symbol in symbols] == [
                    (root / "colors.nia").as_uri()
                ], symbols
            client.notify("textDocument/didClose", {"textDocument": {"uri": renamed.as_uri()}})
            client.wait_diagnostics(renamed.as_uri(), None, lambda items: not items)
            assert not request(client, "workspace/symbol", {"query": "BufferOnly"})

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


def smoke_build_context(server, root):
    compiler = server.with_name("nia" + server.suffix)
    member = root / "app"

    def write(path, text):
        file = root / path
        file.parent.mkdir(parents=True, exist_ok=True)
        file.write_text(text, encoding="utf-8", newline="\n")
        return file

    write('nia.toml', '[workspace]\nmembers = ["app", "flags"]\n')
    flags = write('flags/root.nia', 'pub fun marker() i32 { 1 }\n')
    write('flags/nia.toml', '[package]\nname = "flags"\nversion = "1.0.0"\n[library]\npath = "root.nia"\n')
    write('app/nia.toml', '''[package]
name = "matrix"
version = "1.0.0"
[library]
path = "root.nia"
[[test]]
name = "unit"
path = "unit.nia"
[build-dependencies]
flags = { path = "../flags" }
''')
    library = write('app/root.nia', 'pub fun answer() i32 { 42 }\n')
    unit_src = '''using matrix;
using std::process;
@[test] fun test_value() i32 { matrix::answer() }
pub fun main(init: process::Init) process::ExitCode!() {
    _ = init;
    _ = test_value();
    !()
}
'''
    unit = write('app/unit.nia', unit_src)
    build_src = r'''using std::build;
using std::fs;
using flags;
pub fun build(b: &mut build::Build) build::Error!() {
    _ = flags::marker();
    let generated = build::BuildPathView::init("generated.nia");
    _ = b.addGeneratedFileStep("generate", generated, b"pub fun marker() i32 { 1 }\n").?;
    let imports = [build::ModuleImport::fromBuild("generated", generated)];
    let app = b.addModule(build::ModuleOptions::init("app", fs::PathView::init("main.nia")).withImports(imports)).?;
    let exe = b.addExecutable(build::ExecutableOptions::init("app", app)).?;
    _ = b.addCheckExecutableStep("check", exe).?;
    !()
}
'''
    build = write('app/build.nia', build_src)
    app_src = '''using generated;
using std::process;
@[debug] @[if os == "windows" and not hasFeature("avx2")] fun selected() i32 { 0 }
@[release] @[if os == "linux" and hasFeature("avx2")] fun selected() i32 { 1 }
pub fun main(init: process::Init) process::ExitCode!() {
    _ = init;
    _ = selected();
    _ = generated::marker();
    !()
}
'''
    app = write('app/main.nia', app_src)
    plan = root / 'build/out/app/build-plan.bin'
    generated = plan.with_name('generated.nia')

    def run(step, *args, status=0):
        command = [str(compiler), "build", step, "--diagnostics-format=json", *args]
        completed = subprocess.run(
            command, cwd=member, capture_output=True, text=True, encoding="utf-8", timeout=180,
        )
        assert completed.returncode == status, completed.stdout + completed.stderr
        assert plan.is_file(), plan
        if status:
            return json.loads(completed.stderr)["diagnostics"]

    run('library')
    run("check", "--debug", "--cpu", "baseline", "--target", "x86_64-pc-windows-msvc")
    with subprocess.Popen([str(server)], cwd=root,
                          stdin=subprocess.PIPE, stdout=subprocess.PIPE) as process:
        try:
            client = Client(process, 60)
            request(client, 'initialize', {'processId': None, 'rootUri': root.as_uri(), 'capabilities': {}})
            client.notify('initialized', {})

            def opened(file, source):
                client.notify('textDocument/didOpen', {'textDocument': {
                    'uri': file.as_uri(), 'languageId': 'nia', 'version': 1, 'text': source,
                }})
                _, diagnostics = client.wait_diagnostics(file.as_uri(), 1)
                return diagnostics

            def definition(file, source, needle):
                return request(client, 'textDocument/definition', {'textDocument': {'uri': file.as_uri()},
                                                                   'position': position(source, needle)})

            def changed(file):
                client.notify('workspace/didChangeWatchedFiles', {'changes': [{'uri': file.as_uri(), 'type': 2}]})

            assert opened(app, app_src) == []
            assert definition(app, app_src, 'selected();')['range']['start']['line'] == 2
            assert definition(app, app_src, 'marker();')['uri'] == generated.as_uri()
            assert opened(build, build_src) == []
            assert definition(build, build_src, 'marker();')['uri'] == flags.as_uri()
            run('check', '--release', '--cpu', 'x86_64-v3', '--target', 'x86_64-unknown-linux')
            changed(plan)
            assert definition(app, app_src, 'selected();')['range']['start']['line'] == 3
            assert definition(app, app_src, 'marker();')['uri'] == generated.as_uri()
            build_error, = run("check", "--release", "--cpu", "baseline", "--target",
                               "x86_64-unknown-linux", status=1)
            assert Path(build_error["path"]).resolve() == app, build_error
            assert "selected" in build_error["summary"], build_error
            changed(plan)
            _, diagnostics = client.wait_diagnostics(app.as_uri(), 1, lambda items: any(
                diagnostic["severity"] == 1 and build_error["summary"] in diagnostic["message"]
                for diagnostic in items
            ))
            diagnostic, = diagnostics
            assert diagnostic["code"] == build_error["code"], (diagnostic, build_error)
            assert diagnostic["range"]["start"] == position(app_src, "selected();"), diagnostic
            run('unit')
            changed(plan)
            assert opened(unit, unit_src) == []
            assert definition(unit, unit_src, 'answer()')['uri'] == library.as_uri()
            assert definition(unit, unit_src, 'test_value();')['range']['start']['line'] == 2
            request(client, 'shutdown', None)
            client.notify('exit', None)
            process.stdin.close()
            assert process.wait(timeout=30) == 0
        finally:
            if process.poll() is None:
                process.kill()


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("server", type=Path)
    args = parser.parse_args()
    server = args.server.resolve(strict=True)
    with tempfile.TemporaryDirectory(prefix="nia-lsp-smoke-") as directory:
        smoke(server, Path(directory).resolve())
    with tempfile.TemporaryDirectory(prefix="nia-lsp-build-smoke-") as directory:
        smoke_build_context(server, Path(directory).resolve())


if __name__ == "__main__":
    main()
