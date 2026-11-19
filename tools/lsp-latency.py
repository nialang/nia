# SPDX-License-Identifier: GPL-3.0-or-later
"""Measure editor requests over stdio against a Nia checkout or workspace."""

import argparse
import json
import math
from pathlib import Path
from queue import Queue
import statistics
import subprocess
import threading
from time import perf_counter


class Client:
    def __init__(self, process, timeout):
        self.process = process
        self.timeout = timeout
        self.messages = Queue()
        self.started = {}
        self.responses = {}
        self.sequence = 0
        threading.Thread(target=self.read, daemon=True).start()

    def read(self):
        stream = self.process.stdout
        try:
            while header := stream.readline():
                headers = {}
                while header.strip():
                    name, _, value = header.partition(b":")
                    headers[name.lower()] = value.strip()
                    header = stream.readline()
                payload = stream.read(int(headers[b"content-length"]))
                self.messages.put((perf_counter(), json.loads(payload)))
        except (KeyError, ValueError, OSError) as error:
            self.messages.put(error)
        finally:
            self.messages.put(EOFError("Nia LSP closed stdout"))

    def send(self, message):
        body = json.dumps(dict(jsonrpc="2.0", **message)).encode()
        self.process.stdin.write(f"Content-Length: {len(body)}\r\n\r\n".encode() + body)
        self.process.stdin.flush()

    def notify(self, method, params):
        self.send(dict(method=method, params=params))

    def request(self, method, params):
        self.sequence += 1
        ident = self.sequence
        self.started[ident] = perf_counter()
        self.send(dict(id=ident, method=method, params=params))
        return ident

    def wait(self, ident):
        while ident not in self.responses:
            item = self.messages.get(timeout=self.timeout)
            if isinstance(item, Exception):
                raise item
            timestamp, message = item
            if "method" in message:
                if "id" in message:
                    self.send(dict(id=message["id"], result=None))
            elif "id" in message:
                self.responses[message["id"]] = (timestamp, message)
        timestamp, response = self.responses.pop(ident)
        return 1000 * (timestamp - self.started.pop(ident)), response

    def measure(self, method, params):
        elapsed, response = self.wait(self.request(method, params))
        if "error" in response:
            raise RuntimeError(f"{method}: {response['error']['message']}")
        return elapsed


def summary(samples):
    ordered = sorted(samples)
    return dict(
        samples=len(samples),
        median_ms=round(statistics.median(samples), 2),
        p95_ms=round(ordered[math.ceil(len(samples) * 0.95) - 1], 2),
    )


def position(text, symbol):
    offset = text.index(symbol)
    head = text[:offset]
    line = head.rsplit("\n", 1)[-1]
    return dict(line=head.count("\n"), character=len(line.encode("utf-16-le")) // 2)


def benchmark(client, source, root, symbol, repeat):
    client.measure("initialize", {
        "processId": None,
        "rootUri": root.as_uri(),
        "capabilities": {"workspace": {"semanticTokens": {"refreshSupport": True}}},
    })
    client.notify("initialized", {})
    text = source.read_text(encoding="utf-8")
    document = dict(uri=source.as_uri())
    client.notify("textDocument/didOpen", {
        "textDocument": dict(**document, languageId="nia", version=1, text=text),
    })
    text_document = dict(textDocument=document)
    cursor = dict(**text_document, position=position(text, symbol))
    references = dict(**cursor, context=dict(includeDeclaration=True))
    report = {
        "first_highlight": summary([client.measure("textDocument/semanticTokens/full", text_document)]),
        "folding": summary([client.measure("textDocument/foldingRange", text_document) for _ in range(repeat)]),
        "cold_references": summary([client.measure("textDocument/references", references)]),
        "warm_references": summary([client.measure("textDocument/references", references) for _ in range(repeat)]),
    }
    highlight, completion = [], []
    for version in range(2, repeat + 2):
        client.notify("textDocument/didChange", {
            "textDocument": dict(**document, version=version),
            "contentChanges": [dict(text=text + f"\n// latency sample {version}\n")],
        })
        highlight.append(client.measure("textDocument/semanticTokens/full", text_document))
        completion.append(client.measure("textDocument/completion", cursor))
    report["highlight_after_edit"] = summary(highlight)
    report["completion_after_edit"] = summary(completion)
    rename = client.request("textDocument/rename", dict(**cursor, newName=symbol + "LspLatencyProbe"))
    interactive = client.request("textDocument/completion", cursor)
    completion_ms, response = client.wait(interactive)
    if "error" in response:
        raise RuntimeError(response["error"]["message"])
    report["completion_during_rename"] = summary([completion_ms])
    rename_ms, response = client.wait(rename)
    report["rename"] = summary([rename_ms])
    if "error" in response:
        report["rename"]["error"] = response["error"]["message"]
    client.measure("shutdown", None)
    client.notify("exit", None)
    return report


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--server", type=Path, required=True)
    parser.add_argument("--resource-root", type=Path, required=True)
    parser.add_argument("--workspace", type=Path)
    parser.add_argument("--source", type=Path, required=True)
    parser.add_argument("--symbol", required=True, help="first exact substring to use as the cursor")
    parser.add_argument("--repeat", type=int, default=15)
    parser.add_argument("--timeout", type=float, default=120, help="seconds to wait for each message")
    args = parser.parse_args()
    if args.repeat < 1:
        parser.error("--repeat must be positive")
    source = args.source.resolve()
    root = args.workspace.resolve() if args.workspace else source.parent
    command = [str(args.server.resolve()), "--resource-root", str(args.resource_root.resolve())]
    with subprocess.Popen(command, stdin=subprocess.PIPE, stdout=subprocess.PIPE) as process:
        try:
            report = benchmark(Client(process, args.timeout), source, root, args.symbol, args.repeat)
            process.stdin.close()
            process.wait(timeout=args.timeout)
            if process.returncode:
                raise RuntimeError(f"Nia LSP exited with status {process.returncode}")
        finally:
            if process.poll() is None:
                process.kill()
    print(json.dumps(report, indent=2))


if __name__ == "__main__":
    main()
