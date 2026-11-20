# SPDX-License-Identifier: GPL-3.0-or-later
"""Compare incremental editor snapshots with fresh language-server sessions."""

import argparse
import json
import os
from pathlib import Path
import platform
import runpy
import subprocess
import tempfile
from time import perf_counter


transport = runpy.run_path(str(Path(__file__).with_name("lsp-latency.py")))
Client = transport["Client"]
position = transport["position"]
summary = transport["summary"]


def fixture(workspace):
    for package in ("app", "dep"):
        (workspace / package).mkdir()
        dependencies = '[dependencies]\nutil = { path = "../dep" }\n' if package == "app" else ""
        (workspace / package / "nia.toml").write_text(
            f'[package]\nname = "{package}"\nversion = "0.1.0"\n'
            f'[library]\npath = "root.nia"\n{dependencies}', encoding="utf-8",
        )
    (workspace / "nia.toml").write_text('[workspace]\nmembers = ["app", "dep"]\n', encoding="utf-8")
    dependency = """pub fun amount() i32 { 42 }
pub struct Counter { value: i32 }
extend Counter { pub fun measure(&self) i32 { self.value } }
"""
    (workspace / "dep" / "root.nia").write_text(dependency, encoding="utf-8")
    source = workspace / "app" / "root.nia"
    source.write_text(
        """using util;
pub fun result() i32 { util::amount() + 1 }
pub fun observe(counter: &util::Counter) i32 { counter.measure() }
pub fun unaffected() i32 { util::amount() }
""", encoding="utf-8",
    )
    return source


def replace_once(text, old, new):
    if text.count(old) != 1:
        raise ValueError(f"expected one occurrence of {old!r}")
    return text.replace(old, new, 1)


def workload(resources, workspace, name):
    if name == "workspace":
        source = fixture(workspace)
        body, other_body = "util::amount() + 1", "util::amount() + 2"
        signature, other_signature = "fun result() i32", "fun result() i64"
        use, label = "util::amount() +", "amount"
    else:
        workspace = resources / "std"
        source = workspace / "collections" / "array_list" / "list.nia"
        body, other_body = "self.len() == 0", "self.len() != 0"
        signature, other_signature = "fun isEmpty(&self) bool", "fun isEmpty(&self) usize"
        use, label = "self.len() ", "len"
    clean = source.read_text(encoding="utf-8")
    return workspace, source, clean, {
        "comment": "// Position-changing audit edit\n" + clean,
        "body": replace_once(clean, body, other_body),
        "signature": replace_once(clean, signature, other_signature),
    }, use, label


def request(client, method, params):
    elapsed, response = client.wait(client.request(method, params))
    if "error" in response:
        raise RuntimeError(f"{method}: {response['error']}")
    return elapsed, response["result"]


def inspect(client, document, text, use, label):
    point = position(text, use)
    point["character"] += use.index(label)
    definition_ms, definition = request(client, "textDocument/definition", dict(
        textDocument=document, position=point,
    ))
    completion_ms, completion = request(client, "textDocument/completion", dict(
        textDocument=document, position=dict(point, character=point["character"] + len(label)),
    ))
    items = completion["items"] if isinstance(completion, dict) else completion
    if not definition or not any(item["label"] == label for item in items or []):
        raise AssertionError(f"expected a definition and completion at {use!r}")
    return dict(definition=definition, completion=sorted(items, key=lambda item: json.dumps(item, sort_keys=True))), {
        "definition_ms": definition_ms, "completion_ms": completion_ms,
    }


def session(server, resources, workspace, source, texts, use, label, profile):
    document = {"uri": source.as_uri()}
    environment = os.environ.copy()
    environment["NIA_LSP_TIMINGS"] = "detail" if profile else "off"
    with tempfile.TemporaryFile() as stderr, subprocess.Popen(
        [str(server), "--resource-root", str(resources)], cwd=workspace, stdin=subprocess.PIPE,
        stdout=subprocess.PIPE, stderr=stderr, env=environment,
    ) as process:
        try:
            client = Client(process, 120)
            request(client, "initialize", {"processId": None, "rootUri": workspace.as_uri(), "capabilities": {}})
            client.notify("initialized", {})
            values, latency = [], []
            for version, text in enumerate(texts, 1):
                started = perf_counter()
                if version == 1:
                    client.notify("textDocument/didOpen", {"textDocument": dict(
                        **document, version=version, languageId="nia", text=text,
                    )})
                else:
                    client.notify("textDocument/didChange", {
                        "textDocument": dict(**document, version=version), "contentChanges": [{"text": text}],
                    })
                published, diagnostics = client.wait_diagnostics(source.as_uri(), version)
                value, elapsed = inspect(client, document, text, use, label)
                value["diagnostics"] = sorted(diagnostics, key=lambda item: json.dumps(item, sort_keys=True))
                values.append(value)
                latency.append(dict(diagnostics_ms=1000 * (published - started), **elapsed))
            request(client, "shutdown", None)
            client.notify("exit", None)
            process.stdin.close()
            if process.wait(timeout=30):
                raise RuntimeError(f"nia-lsp exited with status {process.returncode}")
        finally:
            if process.poll() is None:
                process.kill()
                process.wait()
        stderr.seek(0)
        lines = stderr.read().decode("utf-8").splitlines()
        timing = json.loads(lines[-1]) if profile else None
    return values, latency, timing


def count_delta(before, after):
    counters = {name: count - before["counters"].get(name, 0) for name, count in after["counters"].items()}
    stage_counts = {entry["name"]: entry["count"] for entry in before["timings"]}
    timings = {entry["name"]: entry["count"] - stage_counts.get(entry["name"], 0)
               for entry in after["timings"]}
    body_prefix = "body_check.profile.function.check_block["

    def query_counts(values, prefix):
        return {name.removeprefix(prefix): count for name, count in values.items()
                if name.startswith(prefix) and count}

    return {
        "body_checks_by_module": {name.removeprefix(body_prefix).removesuffix("]"): count
                                  for name, count in timings.items() if name.startswith(body_prefix) and count},
        "query_providers": query_counts(timings, "query.provider."),
        "query_validations": query_counts(counters, "query.validation."),
        "green_query_validations": query_counts(counters, "query.green_validation."),
        "query_validation_failures": query_counts(counters, "query.validation_failure."),
        "query_invalidation_clears": query_counts(counters, "query.invalidation_clear."),
        "direct_query_cache_hits": timings.get("query.record_cache_hit", 0),
        "provider_graph_changes": sum(count for name, count in counters.items()
                                      if name.startswith("compiler.executable_provider_demands.")
                                      and name.endswith(".graph_changed")),
        "new_provider_demands": sum(count for name, count in counters.items()
                                    if name.startswith("compiler.executable_provider_demands.") and name.endswith(".new")),
    }


def audit(server, resources, workspace, name, repeat, profile):
    root, source, original, edits, use, label = workload(resources, workspace, name)
    def run(texts, profiled=profile):
        return session(server, resources, root, source, texts, use, label, profiled)

    baseline_value, _, baseline = run([original])
    results = {}
    for edit, text in edits.items():
        fresh, _, fresh_timing = run([text])
        latency = []
        for _ in range(repeat):
            snapshots = [original, text] if profile else [original, text, original]
            incremental, timings, edited_timing = run(snapshots)
            if incremental[1] != fresh[0] or any(value != baseline_value[0] for value in incremental[::2]):
                raise AssertionError(f"{name}/{edit}: fresh and incremental snapshots differ:\n"
                                     f"fresh={fresh[0]!r}\nincremental={incremental[1]!r}")
            latency.append(timings[1])
        observations = {metric: summary([sample[metric] for sample in latency]) for metric in latency[0]}
        results[edit] = dict(latency=observations, diagnostics=len(fresh[0]["diagnostics"]))
        if profile:
            results[edit]["edit_count_delta"] = count_delta(baseline, edited_timing)
            results[edit]["timings"] = dict(baseline=baseline, incremental=edited_timing, fresh=fresh_timing)
    return results


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("server", type=Path)
    parser.add_argument("--resource-root", type=Path, required=True)
    parser.add_argument("--workload", choices=("workspace", "array-list"), default="workspace")
    parser.add_argument("--revision", help="commit used to build the server")
    measurement = parser.add_mutually_exclusive_group()
    measurement.add_argument("--repeat", type=int, default=1)
    measurement.add_argument("--profile", action="store_true")
    args = parser.parse_args()
    if args.repeat < 1:
        parser.error("--repeat must be positive")
    with tempfile.TemporaryDirectory(prefix="nia-lsp-incremental-") as directory:
        result = audit(args.server.resolve(strict=True), args.resource_root.resolve(strict=True),
                       Path(directory).resolve(), args.workload, args.repeat, args.profile)
    print(json.dumps(dict(platform=platform.platform(), revision=args.revision, workload=args.workload,
                          profiled=args.profile, edits=result), indent=2))


if __name__ == "__main__":
    main()
