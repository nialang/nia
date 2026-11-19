# SPDX-License-Identifier: GPL-3.0-or-later
"""Measure editor requests over stdio against a Nia checkout or workspace."""

import argparse
import json
import math
import os
from pathlib import Path
import platform
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
        self.diagnostics = {}
        self.server_requests = set()
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

    def receive(self, deadline):
        item = self.messages.get(timeout=max(0, deadline - perf_counter()))
        if isinstance(item, Exception):
            raise item
        timestamp, message = item
        if message.get("method") == "textDocument/publishDiagnostics":
            params = message["params"]
            self.diagnostics[params["uri"], params.get("version")] = (timestamp, params["diagnostics"])
        elif "method" in message:
            if "id" in message:
                self.server_requests.add(message["method"])
                self.send(dict(id=message["id"], result=None))
        elif "id" in message:
            self.responses[message["id"]] = (timestamp, message)

    def wait_diagnostics(self, uri, version, accept=lambda _: True):
        key = (uri, version)
        deadline = perf_counter() + self.timeout
        while True:
            if key in self.diagnostics:
                timestamp, diagnostics = self.diagnostics.pop(key)
                if accept(diagnostics):
                    return timestamp, diagnostics
            self.receive(deadline)

    def wait(self, ident):
        deadline = perf_counter() + self.timeout
        while ident not in self.responses:
            self.receive(deadline)
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
    initialize = client.measure("initialize", {
        "processId": None,
        "rootUri": root.as_uri(),
        "capabilities": {"workspace": {"semanticTokens": {"refreshSupport": True}}},
    })
    client.notify("initialized", {})
    text = source.read_text(encoding="utf-8")
    document = dict(uri=source.as_uri())
    opened = perf_counter()
    client.notify("textDocument/didOpen", {
        "textDocument": dict(**document, languageId="nia", version=1, text=text),
    })
    text_document = dict(textDocument=document)
    cursor = dict(**text_document, position=position(text, symbol))
    references = dict(**cursor, context=dict(includeDeclaration=True))
    first_highlight = client.measure("textDocument/semanticTokens/full", text_document)
    first_diagnostics, _ = client.wait_diagnostics(document["uri"], 1)
    client.measure("textDocument/semanticTokens/full", text_document)
    cold_semantic = 1000 * (perf_counter() - opened)
    report = {
        "initialize": summary([initialize]),
        "first_highlight": summary([first_highlight]),
        "cold_diagnostics": summary([1000 * (first_diagnostics - opened)]),
        "cold_semantic_highlight": summary([cold_semantic]),
        "folding": summary([client.measure("textDocument/foldingRange", text_document) for _ in range(repeat)]),
        "first_definition": summary([client.measure("textDocument/definition", cursor)]),
        "warm_definition": summary([client.measure("textDocument/definition", cursor) for _ in range(repeat)]),
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

    diagnostics, semantic, canceled = [], [], []
    completed_before_cancel = 0
    for version in range(repeat + 2, 2 * repeat + 2):
        started = perf_counter()
        client.notify("textDocument/didChange", {
            "textDocument": dict(**document, version=version),
            "contentChanges": [dict(text=text + f"\n// diagnostics sample {version}\n")],
        })
        client.measure("textDocument/semanticTokens/full", text_document)
        published, _ = client.wait_diagnostics(document["uri"], version)
        diagnostics.append(1000 * (published - started))
        client.measure("textDocument/semanticTokens/full", text_document)
        semantic.append(1000 * (perf_counter() - started))

        request = client.request("textDocument/references", references)
        client.notify("$/cancelRequest", {"id": request})
        elapsed, response = client.wait(request)
        if response.get("error", {}).get("code") == -32800:
            canceled.append(elapsed)
        elif "error" not in response:
            completed_before_cancel += 1
        else:
            raise RuntimeError(f"Cancellation returned {response['error']}")
    report["diagnostics_after_edit"] = summary(diagnostics)
    report["semantic_highlight_after_edit"] = summary(semantic)
    report["cancellation"] = summary(canceled) if canceled else {"samples": 0}
    report["cancellation"]["completed_before_cancel"] = completed_before_cancel
    if os.name == "nt":
        report["peak_rss_bytes"] = windows_peak_rss(client.process.pid)
    client.measure("shutdown", None)
    client.notify("exit", None)
    return report


def windows_peak_rss(pid):
    import ctypes
    from ctypes import wintypes

    class ProcessMemoryCounters(ctypes.Structure):
        _fields_ = [
            ("size", wintypes.DWORD),
            ("page_faults", wintypes.DWORD),
            ("peak_working_set", ctypes.c_size_t),
            ("working_set", ctypes.c_size_t),
            ("peak_paged_pool", ctypes.c_size_t),
            ("paged_pool", ctypes.c_size_t),
            ("peak_nonpaged_pool", ctypes.c_size_t),
            ("nonpaged_pool", ctypes.c_size_t),
            ("pagefile_usage", ctypes.c_size_t),
            ("peak_pagefile_usage", ctypes.c_size_t),
        ]

    kernel = ctypes.WinDLL("kernel32", use_last_error=True)
    kernel.OpenProcess.argtypes = (wintypes.DWORD, wintypes.BOOL, wintypes.DWORD)
    kernel.OpenProcess.restype = wintypes.HANDLE
    kernel.CloseHandle.argtypes = (wintypes.HANDLE,)
    kernel.K32GetProcessMemoryInfo.argtypes = (
        wintypes.HANDLE, ctypes.POINTER(ProcessMemoryCounters), wintypes.DWORD
    )
    process = kernel.OpenProcess(0x0400 | 0x0010, False, pid)
    if not process:
        raise ctypes.WinError(ctypes.get_last_error())
    try:
        counters = ProcessMemoryCounters(size=ctypes.sizeof(ProcessMemoryCounters))
        if not kernel.K32GetProcessMemoryInfo(process, ctypes.byref(counters), counters.size):
            raise ctypes.WinError(ctypes.get_last_error())
        return counters.peak_working_set
    finally:
        kernel.CloseHandle(process)


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--server", type=Path, required=True)
    parser.add_argument("--resource-root", type=Path, required=True)
    parser.add_argument("--workspace", type=Path)
    parser.add_argument("--source", type=Path, required=True)
    parser.add_argument("--symbol", required=True, help="first exact substring to use as the cursor")
    parser.add_argument("--repeat", type=int, default=15)
    parser.add_argument("--timeout", type=float, default=120, help="seconds to wait for each message")
    parser.add_argument("--revision", help="commit used to build the server")
    parser.add_argument("--configuration", help="target and build settings used for the sample")
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
            if os.name != "nt":
                import resource

                rss = resource.getrusage(resource.RUSAGE_CHILDREN).ru_maxrss
                report["peak_rss_bytes"] = rss if platform.system() == "Darwin" else rss * 1024
        finally:
            if process.poll() is None:
                process.kill()
    report["sample"] = dict(
        platform=platform.platform(),
        workspace=str(root),
        source=str(source),
        symbol=args.symbol,
        revision=args.revision,
        configuration=args.configuration,
    )
    print(json.dumps(report, indent=2))


if __name__ == "__main__":
    main()
