# Compiler Performance Baselines

Nia keeps a fixed workload suite for architecture and performance changes. Run
the complete baseline from the repository root with:

```sh
cargo maintain baseline compiler
```

The runner builds the release compiler once with the dedicated `perf-alloc`
instrumentation feature, runs each compiler workload in a fresh process, and
writes `target/nia-perf/baseline.json`. Source-tree runs pass the repository
`lib` directory as an explicit toolchain resource root; they never depend on the
release binary accidentally finding an installed-layout sibling. The compiler,
resource root, output path, and repeat count are explicit options:

```sh
cargo maintain baseline compiler --resource-root lib --repeat 3 \
  --output target/nia-perf/before.json
cargo maintain baseline compiler --no-build --workload traits --workload const_eval
```

Controlled CI runners may additionally attach an explicit comparison identity:

```sh
cargo maintain baseline compiler --repeat 3 \
  --runner-class github-hosted-ubuntu-24.04-x64
```

`--runner-class` is a trust assertion about a managed runner image and resource
class, not a way to rename a developer machine. Local runs should omit it.

Every compiler and build report records the full Git revision and dirty state;
resolved paths and version output for Rust, Cargo, `llvm-config`, and the linker
selected by `NIA_LINKER` or the native default; the measured Nia executable
identity where applicable; and the compiler/workload profile, feature,
optimization, project-cache, and operating-system page-cache policy. The build
baseline builds the repository-default release compiler before measurement;
`--no-build` is an explicit escape hatch for an externally prepared compiler,
and the report marks that distinction.

The suite currently fixes eleven compiler paths: minimal check, standard-library
Hello World check and executable emission, strings and slices, ArrayList,
trait-heavy code, const-eval-heavy code, multi-module backend lowering, small
and large bounded multi-unit object codegen, and a larger full executable
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
The standard-library Hello World workloads additionally require non-trivial
checked-body and query-execution counts, and its executable workload must use
the live multi-unit LLVM path. This prevents the cold standard-library boundary
from silently disappearing from the measurement through a fixture or cache
configuration change.

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

The compiler baseline command selects the feature automatically. When
`--no-build` or a custom `--compiler` is used, the runner rejects a timing report without
allocation counters. To build that binary manually:

```sh
cargo build --release -p nia-cli --features perf-alloc
```

The compiler can also emit one structured timing record directly:

```sh
target/release/nia --timings=detail --timings-format=json check benchmarks/minimal.nia
```

## Competitive Compiler Baseline

The fixed cross-toolchain matrix is collected independently from Nia's
internal compiler baseline:

```sh
cargo maintain baseline competitive
```

By default this builds the release Nia compiler, takes five samples of both
development and release profiles, and writes a revision-labelled report below
`target/nia-perf/competitive/`. `--no-build`, `--repeat`, `--profile`,
`--workload`, `--nia`, `--rustc`, `--cargo`, `--zig`, `--time`, `--resource-root`, and
`--output` make every material input explicit. The profile and workload options
may be repeated to select distinct entries; duplicate selections are rejected
rather than measured twice accidentally.

The matrix covers `minimal_check`, `hello_check`, `hello_executable`,
`empty_build`, and `hello_build`. The sources under `benchmarks/competitive/` are maintained
language-native Rust and Zig counterparts to `benchmarks/minimal.nia` and
`examples/hello.nia`. Check modes are deliberately described precisely:

- Nia uses `nia check` without native output.
- Rust uses direct `rustc --emit=metadata`. Rust has no direct full semantic
  no-output mode; a binary crate's metadata artifact may be empty even though
  the frontend performed the check.
- Zig uses `zig build-exe -fno-emit-bin`. `zig ast-check` is not treated as an
  equivalent because it stops before full semantic analysis.

Development means Nia `--profile debug -O0`, Rust `-C opt-level=0 -C
debuginfo=0`, and Zig `-O Debug -fno-incremental`. Release means Nia `--profile
release -O2`, Rust `-C opt-level=2 -C debuginfo=0`, and Zig `-O ReleaseSafe
-fno-incremental`. These settings align optimization intent and retain normal
runtime safety where the tools expose it; they do not imply identical optimizer
pass pipelines, runtime linkage, executable format, or standard-library
distribution strategy. Executable sizes therefore remain useful raw evidence,
not a direct quality ranking.

Build-system modes use fresh copied fixture trees. Nia and Zig both compile and
run their build-language programs for an empty graph. Cargo has no equivalent
build-script-only empty-graph mode and rejects a zero-member virtual workspace,
so that matrix cell is recorded as unavailable and no misleading sample is
fabricated. The Hello build does produce and execute one native executable
through `nia build`, Cargo, and `zig build`.

Every process gets an independently created workspace and absent expected
output. Direct compilation starts with absent explicit Nia/Zig project caches;
build-system samples start without Nia build/cache directories, Cargo's target
directory, or Zig's local cache and output directories. Direct rustc is invoked
without incremental compilation. Nia's selected resource root, rustc's distributed
sysroot, and Zig's shared global toolchain cache are retained: this is a
project-cold comparison, not an SDK/toolchain-cold comparison. OS page cache is
uncontrolled and shared across the interleaved tools. Compiler order rotates
between repetitions to reduce fixed ordering bias.

The versioned JSON retains the experiment and Git revision identities, dirty
state, complete compiler and GNU time identities, normalized command, source
hash, raw stdout/stderr, process wall/user/system time, CPU utilization, peak
RSS, artifact size and hash, executable output verification, and every raw
sample. Summaries report median, nearest-rank p95, minimum, and maximum. A
report is still published when a compiler or acceptance check fails so the
failure remains auditable.

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

The build baseline also measures a runner-only package in independent clean and
warm processes. Its build plan contains one aggregate step and no actions, so
`build_compile_runner`, `build_run_runner`, query work, and peak RSS expose the
fixed cost of compiling and executing `build.nia` without application codegen
or action-cache work. The representative fixture continues to cover clean,
warm, source-edit, module-map-edit, corruption recovery, clean-equivalent
artifacts, and failed-action behavior.

A project-cold build starts a fresh process in a copied workspace where both
`.nia-build` and `.nia-cache` were absent before process creation. The report
records those preconditions for every state. Runner-only clean acceptance also
requires a runner-cache miss, zero native-object and link-result reuse hits,
and not-found misses for every emitted LLVM unit and the link result. This
proves that the measured runner executable was produced from source without a
project artifact from an earlier invocation. Frontend persistence counters may
still report reuse within that same compiler process after newly computed
facts are published; such reuse is desirable and is not evidence of a warm
project cache.

Project-cold does not mean machine-cold. The operating system may retain source
and executable pages between samples. A machine-cold experiment must control
and report that state separately, and competing tools must use the same state.
Likewise, SDK/toolchain-cold measurements are a separate claim from the normal
project-cold baseline.

The current cold-build evidence is split into two different claims. The
runner-only clean baseline was about 36.1 seconds before native emission reuse;
the current implementation measures about 24.5--24.8 seconds on the same
workload (roughly a 31% reduction). This is a material improvement to the
build-script path, but it is not a claim that every cold Nia compilation now
matches Rust or Zig. The older empty-build clean baseline remains about 34.0
seconds and needs its own frontend/query investigation. Conversely, an
unchanged runner rebuild is a warm cache case and must not be conflated with
cold compilation.

Generated build runners use content-addressed `.nia-cache/runner/v5/<key>.cache`
records. The key includes the generated source, build-script bytes, standard
library source tree, toolchain identity, host target, profile, test mode,
optimization mode, and build protocol. A hit verifies and atomically restores
the private runner executable; the runner is still started and its plan is
still validated and executed. Malformed or corrupted records are retired as
misses.

Diagnostics and the JSON record both use stderr, but the JSON record is one
complete line beginning with `{"release_compatibility":2`; the baseline runner selects
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
cargo maintain baseline compiler --repeat 3 --output target/nia-perf/before.json
# build or select the candidate compiler
cargo maintain baseline compiler --repeat 3 --output target/nia-perf/after.json
cargo maintain baseline compare \
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
most recent completed main-branch artifact that contains a fully collected
`baseline.json`. The candidate must pass the same broad comparator guards before
the workflow succeeds.

Every fully collected main-branch or scheduled candidate becomes the rolling
`nia-perf-baseline`, with its comparison report and run/revision identity retained
for 90 days. The current run remains failed when a guard fires, but its already
landed main revision becomes the comparison point for the next push. This keeps
pull requests relative to current main and prevents one accepted regression or
changed workload from pinning all future comparisons to an arbitrarily old
revision. Baseline discovery reads only the `nia-perf-baseline` artifact; an
unpromoted candidate is diagnostic evidence, not an alternate trend source.

Pull-request candidates and main runs that fail before completing
`candidate.json` are stored separately for 14 days for diagnosis. A failed
collection retains its combined candidate log plus run/revision identity. The
first main run is an explicit bootstrap only when no earlier fully collected
controlled artifact exists; after that, every available baseline is compared.
This stores main-branch trends without committing machine-specific numbers to
the source tree or treating a developer sample as a project-wide absolute
threshold.
Each run also publishes an Actions step summary containing the candidate
revision, controlled runner class, selected main baseline run and artifact name,
and comparison result when a prior artifact was available.

Performance evidence is interpreted under the end-to-end acceptance and failed
experiment rules in [compiler-maintenance.md](../docs/compiler-maintenance.md).
