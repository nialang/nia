# Compiler Performance Baselines

Nia keeps a fixed workload suite for architecture and performance changes. Run
the complete baseline from the repository root with:

```sh
python3 tools/perf.py
```

The runner builds the release compiler once with the dedicated `perf-alloc`
instrumentation feature, runs each compiler workload in a fresh process, and
writes `target/nia-perf/baseline.json`. Source-tree runs pass the repository
`lib` directory as an explicit toolchain resource root; they never depend on the
release binary accidentally finding an installed-layout sibling. The compiler,
resource root, output path, and repeat count are explicit options:

```sh
python3 tools/perf.py --resource-root lib --repeat 3 \
  --output target/nia-perf/before.json
python3 tools/perf.py --no-build --workload traits --workload const_eval
```

Controlled CI runners may additionally attach an explicit comparison identity:

```sh
python3 tools/perf.py --repeat 3 \
  --runner-class github-hosted-ubuntu-24.04-x64
```

`--runner-class` is a trust assertion about a managed runner image and resource
class, not a way to rename a developer machine. Local runs should omit it.

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
bounded LLVM worker lanes and live ready-task submissions separately from stable
unit count; one lane may consume multiple units without coarsening their
work-product identities. Object-codegen workloads are rejected unless every
emitted unit was submitted through the live readiness path, so a synchronous or
aggregate fallback cannot silently become the benchmark.

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

To audit persistent frontend reuse across separate compiler processes, give both
checks the same explicit artifact cache directory:

```sh
target/release/nia --timings=detail --timings-format=json check benchmarks/minimal.nia --cache-dir target/nia-perf/frontend-cache
target/release/nia --timings=detail --timings-format=json check benchmarks/minimal.nia --cache-dir target/nia-perf/frontend-cache
```

The detailed counters include executions for parsing, loader item-tree and
serialized fact queries, compiler module definitions, and public-surface facts.
Compare those deterministic counts before interpreting wall-time differences.
A warm frontend cache does not imply that all revision-local semantic products
are persistent.

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

When both reports declare the same non-empty `runner_class`, the comparator
treats that class as the controlled hardware identity and permits the provider's
underlying CPU model string to vary. Operating system, architecture, effective
CPU limit, and effective memory limit must still match. If either report omits
the class, both reports must omit it and the physical CPU model remains part of
the strict compatibility check. A controlled CI artifact therefore cannot be
silently compared with a local sample.

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

The comparator first checks the physical machine identity or controlled runner
class and then the effective resource shape. It refuses incompatible machines
by default. The default relative guards are deliberately broad: 50% wall time,
30% RSS, 5% query executions, and 20% for both allocated and peak live Rust heap
bytes. The allocation threshold also guards finalization-window peak growth for
the `module_backend` workload. All thresholds are command-line options, and the
result is itself machine-readable JSON.
`--allow-machine-mismatch` exists for exploration, not for a release gate.

## Managed CI Trend

`.github/workflows/performance.yml` defines the managed Linux LLVM performance
job. It follows the newest Rust stable release, reports the resolved toolchain
identity, and installs the current Fedora-derived LLVM 22 identity on
`ubuntu-24.04`. It runs every workload three times with
the controlled `github-hosted-ubuntu-24.04-x64` runner class, and downloads the
most recent successful main-branch `nia-perf-baseline` artifact. The candidate
must pass the same broad comparator guards before the workflow succeeds.

Successful main-branch and scheduled runs upload `baseline.json`, the comparison
report when one exists, and run/revision identity with 90-day retention. Later
runs only search successful main workflow runs, so a failed regression candidate
cannot become the next baseline. Pull-request and failed-main candidates are
stored separately for 14 days for diagnosis. A failed collection retains its
combined candidate log plus run/revision identity even when no baseline JSON was
completed. The first main run is an explicit bootstrap because no earlier
controlled artifact exists; after that, every available baseline is compared.
This stores main-branch trends without committing machine-specific numbers to
the source tree or treating a developer sample as a project-wide absolute
threshold.
Each run also publishes an Actions step summary containing the candidate
revision, controlled runner class, selected main baseline run, and comparison
result when a prior artifact was available.

Performance evidence is interpreted under the end-to-end acceptance and failed
experiment rules in [compiler-maintenance.md](compiler-maintenance.md).
