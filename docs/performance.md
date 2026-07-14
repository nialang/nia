# Compiler Performance Baselines

Nia keeps a fixed workload suite for architecture and performance changes. Run
the complete baseline from the repository root with:

```sh
python3 tools/perf.py
```

The runner builds the release compiler once, runs each compiler workload in a
fresh process, and writes `target/nia-perf/baseline.json`. The output path and
repeat count are explicit options:

```sh
python3 tools/perf.py --repeat 3 --output target/nia-perf/before.json
python3 tools/perf.py --no-build --workload traits --workload comptime
```

The suite currently fixes six compiler paths: minimal check, strings and
slices, ArrayList, trait-heavy code, comptime-heavy code, and full executable
emission. Benchmark sources live in `benchmarks/` or reuse maintained examples;
generated executables and reports remain under temporary or `target/`
directories.

Each result contains process wall, user, and system time; maximum resident set
size; CPU utilization; aggregated stage/query timings; query execution and
cache-hit/value-clone counts; provider-demand rounds; checked and reachable body
counts; and LLVM unit/object-reuse counters when codegen runs. The JSON schema is
versioned so trend tooling does not need to parse the human timing report.

The compiler can also emit one structured timing record directly:

```sh
target/release/nia --timings=detail --timings-format=json check benchmarks/minimal.nia
```

Diagnostics and the JSON record both use stderr, but the JSON record is one
complete line beginning with `{"schema_version":1`; the baseline runner selects
that record structurally rather than parsing diagnostic or timing prose.

## Machine Resource Model

Benchmarks do not select WSL, workstation, container, or rental-machine
profiles. The report records the capabilities visible to the process: CPU
affinity, system memory, and cgroup memory limit. WSL therefore reports the
Linux VM's resources, constrained containers and rental hosts report their
cgroup limits, and bare Linux hosts report system resources. Comparing results
still requires comparable hardware and system load; CI guards should remain
wide while a dedicated perf runner tracks trends.

On Unix, process CPU time and peak RSS come from `getrusage`. Unsupported host
metrics are encoded as JSON `null`, never inferred from a machine category.
