# Compiler Performance Baselines

Nia keeps a fixed workload suite for architecture and performance changes. Run
the complete baseline from the repository root with:

```sh
python3 tools/perf.py
```

The runner builds the release compiler once with the dedicated `perf-alloc`
instrumentation feature, runs each compiler workload in a fresh process, and
writes `target/nia-perf/baseline.json`. The output path and repeat count are
explicit options:

```sh
python3 tools/perf.py --repeat 3 --output target/nia-perf/before.json
python3 tools/perf.py --no-build --workload traits --workload const_eval
```

The suite currently fixes nine compiler paths: minimal check, strings and
slices, ArrayList, trait-heavy code, const-eval-heavy code, multi-module backend
lowering, small and large bounded multi-unit object codegen, and full executable
emission. Benchmark sources live in `benchmarks/`, reuse maintained examples,
or are generated deterministically by the runner; generated sources, objects,
executables, and reports remain under temporary or `target/` directories. The
`codegen_buckets` workload is a single source with eight reachable definitions.
`codegen_buckets_large` generates sixteen reachable 1 MiB static definitions
with only one small function body, keeping frontend work out of the object
codegen comparison. The workloads are rejected unless they compile at least two
and four LLVM units respectively, so their CPU/RSS trends cannot silently become
single-unit, cache-hit, or linker measurements. The reports also record the
bounded LLVM worker lanes separately from stable unit count; one lane may
consume multiple units without coarsening their work-product identities.

Each result contains process wall, user, and system time; maximum resident set
size; CPU utilization; aggregated stage/query timings; query execution and
cache-hit/value-clone counts; Rust heap allocation/deallocation/reallocation
calls and requested bytes; provider-demand rounds; checked and reachable body
counts; current and peak live Rust heap bytes; and LLVM unit/object-reuse
counters when codegen runs. Backend codegen reports live/peak snapshots before
module-plan publication, after publication, and after consuming the per-module
query slots. The multi-module backend workload additionally requires a
process-wide live-allocation window around the parallel finalization batch. It
reports start, end, peak, and peak growth bytes across allocations performed by
all query workers. The JSON schema is versioned so trend tooling does not need
to parse the human timing report.

Allocation counters require both the `perf-alloc` build feature and
`--timings=detail`; the normal compiler binary uses the ordinary Rust allocator
without a counting wrapper. The counters stop before the collector flushes or
serializes its report. They describe traffic and live allocations through the
Rust global allocator; `allocator.peak_live_bytes` starts from the process's
already-live instrumented heap at the timing boundary and records the maximum
thereafter. They do not include allocations performed inside LLVM or other
native libraries, so maximum RSS remains the whole-process measure.
`query.value_clone_bytes` is the allocator traffic observed on the cloning
thread while owned query values are cloned, so heap-owned vectors and maps are
counted without treating their shallow `size_of` as deep size.

`tools/perf.py` selects the feature automatically. When `--no-build` or a
custom `--compiler` is used, the runner rejects a timing report without
allocation counters. To build that binary manually:

```sh
cargo build --release -p nia-cli --features perf-alloc
```

The compiler can also emit one structured timing record directly:

```sh
target/release/nia --timings=detail --timings-format=json check benchmarks/minimal.nia
```

A normal build emits all non-allocation timing data with this command;
allocation counters are present only in the instrumented build above.

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

## Comparing Baselines

Collect before and after results on the same machine or controlled runner, then
compare their per-workload medians:

```sh
python3 tools/perf.py --repeat 3 --output target/nia-perf/before.json
# build or select the candidate compiler
python3 tools/perf.py --repeat 3 --output target/nia-perf/after.json
python3 tools/perf_compare.py \
  target/nia-perf/before.json target/nia-perf/after.json
```

The comparator first checks the operating system, architecture, CPU model,
effective CPU limit, and effective memory limit. It refuses incompatible
machines by default. The default relative guards are deliberately broad: 50%
wall time, 30% RSS, 5% query executions, and 20% for both allocated and peak
live Rust heap bytes. The allocation threshold also guards finalization-window
peak growth for the `module_backend` workload. All thresholds are command-line
options, and the result is itself machine-readable JSON.
`--allow-machine-mismatch` exists for exploration, not for a release gate.

The repository does not yet define a CI environment capable of building and
running Nia's LLVM suite. When one is added, its main-branch baseline should be
stored as a runner artifact or in a trend service and compared on the same
resource shape. A developer-machine sample must not become a project-wide
absolute threshold.
