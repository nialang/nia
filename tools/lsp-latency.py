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
from time import perf_counter, sleep


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

    def wait_diagnostics(self, uri, version, accept=lambda _: True, *, since=None):
        key = (uri, version)
        deadline = perf_counter() + self.timeout
        while True:
            if key in self.diagnostics:
                timestamp, diagnostics = self.diagnostics.pop(key)
                if (since is None or timestamp >= since) and accept(diagnostics):
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
    return position_at(text, offset)


def position_at(text, offset):
    head = text[:offset]
    line = head.rsplit("\n", 1)[-1]
    return dict(line=head.count("\n"), character=len(line.encode("utf-16-le")) // 2)


def benchmark_members(client, source, root, repeat, member):
    original, receiver, name = member
    text = source.read_text(encoding="utf-8")
    if not original or text.count(original) != 1 or not receiver or not name:
        raise ValueError("--member requires one exact source match, a receiver, and a member name")
    before, after = text.split(original)
    client.measure("initialize", {
        "processId": None, "rootUri": root.as_uri(), "capabilities": {},
    })
    client.notify("initialized", {})
    document = dict(uri=source.as_uri())
    client.notify("textDocument/didOpen", {
        "textDocument": dict(**document, languageId="nia", version=1, text=text),
    })
    client.wait_diagnostics(document["uri"], 1)
    samples = []
    version = 1
    for cycle in range(repeat):
        for length in range(len(name) + 1):
            version += 1
            prefix = name[:length]
            expression = f"{receiver}.{prefix}"
            edited = before + expression + after + f"\n// member latency cycle {cycle + 1}\n"
            started = perf_counter()
            client.notify("textDocument/didChange", {
                "textDocument": dict(**document, version=version),
                "contentChanges": [dict(text=edited)],
            })
            elapsed, response = client.wait(client.request("textDocument/completion", {
                "textDocument": document,
                "position": position_at(edited, len(before) + len(expression)),
            }))
            if "error" in response:
                raise RuntimeError(f"Member completion failed for {expression!r}: {response['error']}")
            result = response["result"]
            items = result["items"] if isinstance(result, dict) else result or []
            if not any(item["label"] == name for item in items):
                raise RuntimeError(f"Member completion for {expression!r} omitted {name!r}")
            published, _ = client.wait_diagnostics(document["uri"], version)
            samples.append(dict(cycle=cycle + 1, prefix=prefix, completion_ms=round(elapsed, 2),
                                diagnostics_ms=round(1000 * (published - started), 2)))
    client.measure("shutdown", None)
    client.notify("exit", None)
    return dict(
        first_dot=samples[0],
        bare_dot=summary([sample["completion_ms"] for sample in samples if not sample["prefix"]]),
        prefix=summary([sample["completion_ms"] for sample in samples if sample["prefix"]]),
        diagnostics=summary([sample["diagnostics_ms"] for sample in samples]),
        members=samples,
    )


def benchmark_syntax(client, source, root, repeat, replacement):
    original = source.read_text(encoding="utf-8")
    old, new = replacement
    if not old or old == new or original.count(old) != 1:
        raise ValueError("--replace must change exactly one occurrence")
    alternate = original.replace(old, new)
    client.measure("initialize", {
        "processId": None,
        "rootUri": root.as_uri(),
        "capabilities": {"workspace": {"semanticTokens": {"refreshSupport": True}}},
    })
    client.notify("initialized", {})
    document = dict(uri=source.as_uri())
    params = dict(textDocument=document)
    methods = [
        ("highlight", "textDocument/semanticTokens/full", params),
        ("folding", "textDocument/foldingRange", params),
        ("formatting", "textDocument/formatting", dict(**params, options=dict(tabSize=4, insertSpaces=True))),
    ]
    client.notify("textDocument/didOpen", {"textDocument": dict(
        **document, languageId="nia", version=1, text=original,
    )})
    first_highlight = client.measure("textDocument/semanticTokens/full", params)
    client.wait_diagnostics(document["uri"], 1)
    report = {"first_highlight": summary([first_highlight])}
    samples = {name: [] for name, _, _ in methods}
    version = 1
    for _ in range(repeat):
        for name, method, request_params in methods:
            version += 1
            text = alternate if version % 2 == 0 else original
            started = perf_counter()
            client.notify("textDocument/didChange", {
                "textDocument": dict(**document, version=version),
                "contentChanges": [dict(text=text)],
            })
            ident = client.request(method, request_params)
            queued_ms = 1000 * (client.started[ident] - started)
            elapsed, response = client.wait(ident)
            if "error" in response:
                raise RuntimeError(f"{method}: {response['error']}")
            samples[name].append(elapsed + queued_ms)
    for name, values in samples.items():
        report[f"{name}_after_edit"] = summary(values)
    client.wait_diagnostics(document["uri"], version)
    report["semantic_highlight_after_diagnostics"] = summary([
        client.measure("textDocument/semanticTokens/full", params),
    ])
    if os.name == "nt":
        report["peak_rss_bytes"] = windows_working_set(client.process.pid)[1]
    client.measure("shutdown", None)
    client.notify("exit", None)
    return report


def benchmark(client, source, root, symbol, repeat, replacement=None, cancel_delay_ms=0,
              reopen_cycles=0):
    initialize = client.measure("initialize", {
        "processId": None,
        "rootUri": root.as_uri(),
        "capabilities": {"workspace": {
            "semanticTokens": {"refreshSupport": True},
            "workspaceFolders": bool(reopen_cycles),
        }},
    })
    client.notify("initialized", {})
    text = source.read_text(encoding="utf-8")
    if replacement is not None:
        old, new = replacement
        if not old or old == new or text.count(old) != 1:
            raise ValueError("--replace must change exactly one occurrence")
        alternate = text.replace(old, new)
        if symbol not in alternate:
            raise ValueError("--replace must preserve --symbol in the alternate snapshot")
    else:
        alternate = None
    document = dict(uri=source.as_uri())
    text_document = dict(textDocument=document)
    cursor = dict(**text_document, position=position(text, symbol))
    references = dict(**cursor, context=dict(includeDeclaration=True))

    def change(version):
        if alternate is None:
            edited = text + f"\n// latency sample {version}\n"
        else:
            edited = alternate if version % 2 == 0 else text
        client.notify("textDocument/didChange", {
            "textDocument": dict(**document, version=version),
            "contentChanges": [dict(text=edited)],
        })
        return dict(**text_document, position=position(edited, symbol))
    opened = perf_counter()
    client.notify("textDocument/didOpen", {
        "textDocument": dict(**document, languageId="nia", version=1, text=text),
    })
    highlight_request = client.request("textDocument/semanticTokens/full", text_document)
    definition_request = client.request("textDocument/definition", cursor)
    first_highlight, highlight_response = client.wait(highlight_request)
    if "error" in highlight_response:
        raise RuntimeError(highlight_response["error"]["message"])
    cold_definition, definition_response = client.wait(definition_request)
    if "error" in definition_response:
        raise RuntimeError(definition_response["error"]["message"])
    first_diagnostics, _ = client.wait_diagnostics(document["uri"], 1)
    client.measure("textDocument/semanticTokens/full", text_document)
    cold_semantic = 1000 * (perf_counter() - opened)
    report = {
        "initialize": summary([initialize]),
        "first_highlight": summary([first_highlight]),
        "cold_diagnostics": summary([1000 * (first_diagnostics - opened)]),
        "cold_semantic_highlight": summary([cold_semantic]),
        "folding": summary([client.measure("textDocument/foldingRange", text_document) for _ in range(repeat)]),
        "cold_definition": dict(summary([cold_definition]), resolved=bool(definition_response["result"])),
        "warm_definition": summary([client.measure("textDocument/definition", cursor) for _ in range(repeat)]),
        "cold_references": summary([client.measure("textDocument/references", references)]),
        "warm_references": summary([client.measure("textDocument/references", references) for _ in range(repeat)]),
    }
    highlight, completion = [], []
    for version in range(2, repeat + 2):
        cursor = change(version)
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
        cursor = change(version)
        client.measure("textDocument/semanticTokens/full", text_document)
        published, _ = client.wait_diagnostics(document["uri"], version)
        diagnostics.append(1000 * (published - started))
        client.measure("textDocument/semanticTokens/full", text_document)
        semantic.append(1000 * (perf_counter() - started))

        request = client.request("textDocument/references", dict(**cursor, context=references["context"]))
        if cancel_delay_ms:
            sleep(cancel_delay_ms / 1000)
        cancel_sent = perf_counter()
        client.notify("$/cancelRequest", {"id": request})
        started_request = client.started[request]
        elapsed, response = client.wait(request)
        if response.get("error", {}).get("code") == -32800:
            canceled.append(max(0, elapsed - 1000 * (cancel_sent - started_request)))
        elif "error" not in response:
            completed_before_cancel += 1
        else:
            raise RuntimeError(f"Cancellation returned {response['error']}")
    report["diagnostics_after_edit"] = summary(diagnostics)
    report["semantic_highlight_after_edit"] = summary(semantic)
    report["cancellation"] = summary(canceled) if canceled else {"samples": 0}
    report["cancellation"]["completed_before_cancel"] = completed_before_cancel
    references_after_edit = []
    for version in range(2 * repeat + 2, 3 * repeat + 2):
        cursor = change(version)
        client.wait_diagnostics(document["uri"], version)
        references_after_edit.append(client.measure(
            "textDocument/references", dict(**cursor, context=references["context"]),
        ))
    report["references_after_edit"] = summary(references_after_edit)
    if reopen_cycles:
        report["reopen"] = benchmark_reopen(
            client, source, root, document, text, 3 * repeat + 1, reopen_cycles,
        )
    if os.name == "nt":
        report["peak_rss_bytes"] = windows_working_set(client.process.pid)[1]
    client.measure("shutdown", None)
    client.notify("exit", None)
    return report


def benchmark_reopen(client, source, root, document, text, last_version, cycles):
    workspace = {"uri": root.as_uri(), "name": root.name}
    params = {"textDocument": document}
    baseline = current_rss(client.process.pid)
    idle_seconds = 0.25
    samples = []
    for version in range(last_version + 1, last_version + cycles + 1):
        client.notify("textDocument/didClose", params)
        client.wait_diagnostics(document["uri"], None, lambda items: not items)
        client.notify("workspace/didChangeWorkspaceFolders", {
            "event": {"added": [], "removed": [workspace]},
        })
        sleep(idle_seconds)
        idle_rss = current_rss(client.process.pid)
        client.notify("workspace/didChangeWorkspaceFolders", {
            "event": {"added": [workspace], "removed": []},
        })
        source_text = text + f"\n// Unsaved reopen sample {version}\n"
        started = perf_counter()
        client.notify("textDocument/didOpen", {
            "textDocument": dict(document, languageId="nia", version=version, text=source_text),
        })
        elapsed = client.measure("textDocument/semanticTokens/full", params)
        published, _ = client.wait_diagnostics(document["uri"], version)
        samples.append(dict(
            version=version,
            highlight_ms=round(elapsed, 2),
            diagnostics_ms=round(1000 * (published - started), 2),
            idle_rss_bytes=idle_rss,
            open_rss_bytes=current_rss(client.process.pid),
        ))
    if source.read_text(encoding="utf-8") != text:
        raise RuntimeError("source changed on disk during the reopen benchmark")
    return dict(
        idle_seconds=idle_seconds,
        baseline_rss_bytes=baseline,
        highlight=summary([sample["highlight_ms"] for sample in samples]),
        diagnostics=summary([sample["diagnostics_ms"] for sample in samples]),
        samples=samples,
    )


def current_rss(pid):
    if os.name == "nt":
        return windows_working_set(pid)[0]
    if platform.system() == "Linux":
        status = Path(f"/proc/{pid}/status").read_text(encoding="utf-8")
        rss = next(line for line in status.splitlines() if line.startswith("VmRSS:"))
        return int(rss.split()[1]) * 1024
    if platform.system() == "Darwin":
        value = subprocess.check_output(["ps", "-o", "rss=", "-p", str(pid)], text=True)
        return int(value.strip()) * 1024
    return None


def windows_working_set(pid):
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
        return counters.working_set, counters.peak_working_set
    finally:
        kernel.CloseHandle(process)


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--server", type=Path, required=True)
    parser.add_argument("--resource-root", type=Path, required=True)
    parser.add_argument("--workspace", type=Path)
    parser.add_argument("--source", type=Path, required=True)
    workload = parser.add_mutually_exclusive_group(required=True)
    workload.add_argument("--symbol", help="first exact substring to use as the cursor")
    workload.add_argument("--member", nargs=3, metavar=("ORIGINAL", "RECEIVER", "NAME"),
                          help="replace one expression and type NAME character by character after RECEIVER.")
    workload.add_argument("--syntax", action="store_true", help="measure syntax requests after --replace edits")
    parser.add_argument("--repeat", type=int, default=15)
    parser.add_argument("--reopen-cycles", type=int, default=0,
                        help="measure repeated close, workspace switch, and reopen in the same process")
    parser.add_argument("--timeout", type=float, default=120, help="seconds to wait for each message")
    parser.add_argument("--revision", help="commit used to build the server")
    parser.add_argument("--configuration", help="target and build settings used for the sample")
    parser.add_argument("--replace", nargs=2, metavar=("OLD", "NEW"),
                        help="alternate one exact source replacement with the original text")
    parser.add_argument("--cancel-delay-ms", type=float, default=0,
                        help="wait this long after sending a reference request before canceling it")
    args = parser.parse_args()
    if args.repeat < 1:
        parser.error("--repeat must be positive")
    if args.reopen_cycles < 0:
        parser.error("--reopen-cycles must be nonnegative")
    if not math.isfinite(args.cancel_delay_ms) or args.cancel_delay_ms < 0:
        parser.error("--cancel-delay-ms must be finite and nonnegative")
    if args.member and (args.replace or args.reopen_cycles or args.cancel_delay_ms):
        parser.error("--member cannot be combined with --replace, --reopen-cycles, or --cancel-delay-ms")
    if args.syntax and (not args.replace or args.reopen_cycles or args.cancel_delay_ms):
        parser.error("--syntax requires --replace and measures only syntax requests")
    source = args.source.resolve()
    root = args.workspace.resolve() if args.workspace else source.parent
    command = [str(args.server.resolve()), "--resource-root", str(args.resource_root.resolve())]
    with subprocess.Popen(command, stdin=subprocess.PIPE, stdout=subprocess.PIPE) as process:
        try:
            client = Client(process, args.timeout)
            if args.member:
                report = benchmark_members(client, source, root, args.repeat, args.member)
            elif args.syntax:
                report = benchmark_syntax(client, source, root, args.repeat, args.replace)
            else:
                report = benchmark(client, source, root, args.symbol, args.repeat,
                                   args.replace, args.cancel_delay_ms, args.reopen_cycles)
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
        replacement=args.replace,
        member=args.member,
        syntax=args.syntax,
        cancel_delay_ms=args.cancel_delay_ms,
    )
    print(json.dumps(report, indent=2))


if __name__ == "__main__":
    main()
