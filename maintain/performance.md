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

`--runner-class` identifies a managed runner image and resource class, not a
promise that every hosted machine has the same CPU. Local runs should omit it.
CPU model and effective resource limits remain part of the compatibility check.

CI builds the comparison revision in a separate checkout, then measures both
compilers with the candidate revision's maintenance runner and workload sources.
The reference invocation supplies `--no-build`, `--compiler`, `--resource-root`,
and `--compiler-source-root` explicitly. The last option attributes source
revision and dirty state to the measured compiler's checkout rather than the
runner's checkout; each compiler uses its own standard library and runtime.

Every compiler and build report records the full Git revision and dirty state;
resolved paths and version output for Rust, Cargo, `llvm-config`, and the linker
selected through the compiler's native link policy (`NIA_LINKER`, then
`NIA_LLD`, bundled LLD from the measured resource root, or the target's LLD
driver on `PATH`); the measured Nia executable
identity where applicable; and the compiler/workload profile, feature,
optimization, project-cache, and operating-system page-cache policy. The build
baseline builds the repository-default release compiler before measurement;
`--no-build` is an explicit escape hatch for an externally prepared compiler,
and the report marks that distinction.

The suite fixes twenty-four compiler workloads. Generated sources, objects,
executables, and reports stay under temporary or `target/` directories; the
remaining sources live in `benchmarks/` or reuse maintained examples.

| Workload | Command | Source | Purpose and acceptance |
| --- | --- | --- | --- |
| `minimal` | `check` | `benchmarks/minimal.nia` | Minimal check. |
| `std_hello_check` | `check` | `examples/hello.nia` | Cold standard-library boundary; requires non-trivial checked-body and query-execution counts. |
| `std_hello_exe` | `emit --exe` | `examples/hello.nia` | As above, and must use the live multi-unit LLVM path. |
| `strings_slices` | `check` | `examples/data.nia` | Strings and slices. |
| `array_list` | `check` | `examples/collections.nia` | ArrayList. |
| `traits` | `check` | `examples/traits.nia` | Trait-heavy code. |
| `const_eval` | `check` | `benchmarks/const_heavy.nia` | Const-eval-heavy code. |
| `module_backend` | `emit --backend` | `examples/modules/main.nia` | Multi-module backend lowering; records a process-wide live-allocation window around parallel finalization. |
| `codegen_buckets` | `emit --obj` | `benchmarks/codegen_buckets.nia` | One source with eight reachable definitions; must compile at least two LLVM units. |
| `codegen_buckets_large` | `emit --obj` | generated | Sixteen reachable 1 MiB static definitions and one small function body; must compile at least four LLVM units. |
| `emit_exe` | `emit --exe` | `examples/collections.nia` | Larger full executable emission. |
| `source_volume_{16,64,256,1024}` | `check` | generated | Single-module curve growing top-level declarations. |
| `source_text_{1,4,16,64}k` | `check` | generated | Comment padding that grows source bytes without semantic items. |
| `synthetic_{10,50,100,500}_modules` | `emit --exe` | generated | Flat/star executable scales from the competitive matrix's Nia generator. |
| `synthetic_1000_modules` | `emit --exe` | generated | Nia-only extension of the flat/star scale; no Rust or Zig counterpart. |

Source-volume records include the generated source byte count, so size effects
are not inferred from workload names. The object-codegen workloads are rejected
unless every emitted unit was submitted through the live readiness path and
the minimum unit count is met, so their CPU/RSS trends cannot silently become
single-unit, cache-hit, linker, or synchronous-fallback measurements. The
compiler-builtins unit is emitted synchronously after the readiness tasks; the
total must equal the task submissions plus that one unit, and every unit must
acquire a memory permit. Reports
record bounded LLVM worker lanes and live ready-task submissions separately
from stable unit count; one lane may consume several units without coarsening
their work-product identities. Each synthetic sample is rejected unless its
loaded-module, query-execution, checked-body, reachable-body, LLVM-unit, and
live ready-task counters retain at least the requested scale, so repeated
samples give the internal distributions behind the competitive baseline's cold
wall, CPU-utilization, and peak-RSS scales.

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
all query workers. The compiler JSON report is consumed as a structured timing
payload. Maintain does not persist the compiler's protocol identity and does
not introduce a separate report or schema version axis; Git revision and
complete tool identities provide experiment attribution.

Allocation counters require both the `perf-alloc` build feature and
`--timings=detail`; the normal compiler binary installs mimalloc (`nia-alloc`)
without a counting wrapper. The counters stop before the collector flushes or
serializes its report. They describe traffic and live allocations through the
Rust global allocator; `allocator.peak_live_bytes` starts from the process's
already-live instrumented heap at the timing boundary and records the maximum
thereafter. They do not include allocations performed inside LLVM or other
native libraries, so maximum RSS remains the whole-process measure.
Query products are returned as shared handles or transferred owned values;
the query engine does not clone cached semantic payloads on cache hits.

The compiler baseline command selects the feature automatically. When
`--no-build` or a custom `--compiler` is used, the runner rejects a timing report without
allocation counters. To build that binary manually:

```sh
cargo build --release -p nia-cli --features perf-alloc
```

The compiler can also emit one structured timing record directly:

```sh
target/release/nia --resource-root lib --timings=detail --timings-format=json check benchmarks/minimal.nia
```

A normal build emits every timing field except the allocation counters, which
require the `perf-alloc` build above.

To audit persistent frontend reuse across separate compiler processes, give both
checks the same explicit artifact cache directory:

```sh
target/release/nia --resource-root lib --timings=detail --timings-format=json check benchmarks/minimal.nia --cache-dir target/nia-perf/frontend-cache
target/release/nia --resource-root lib --timings=detail --timings-format=json check benchmarks/minimal.nia --cache-dir target/nia-perf/frontend-cache
```

The detailed counters include executions for parsing, loader item-tree and
serialized fact queries, compiler module definitions, and public-surface facts.
Compare those deterministic counts before interpreting wall-time differences.
A warm frontend cache does not imply that all revision-local semantic products
are persistent.

## Competitive Compiler Baseline

The fixed cross-toolchain matrix is collected independently from Nia's
internal compiler baseline:

```sh
cargo maintain baseline competitive
```

By default this builds the release Nia compiler, takes five samples of both
development and release profiles, and writes a revision-labelled report below
`target/nia-perf/competitive/`. `--no-build`, `--repeat`, `--profile`,
`--workload`, `--nia`, `--rustc`, `--cargo`, `--zig`, `--time`, `--resource-root`,
`--timeout-seconds`, and `--output` make every material input explicit. The profile and workload options
may be repeated to select distinct entries; duplicate selections are rejected
rather than measured twice accidentally.

The matrix covers `minimal_check`, `hello_check`, `hello_executable`,
`empty_build`, `hello_build`, and the `synthetic_10_modules`,
`synthetic_50_modules`, `synthetic_100_modules`, and
`synthetic_500_modules` flat/star clean scales. The
`synthetic_100_modules_deep_chain` and
`synthetic_100_modules_mixed_fan_out` workloads hold module count and semantic
work constant while varying dependency shape. A separate
`synthetic_100_modules_build` workload runs the medium corpus through Nia
build, Cargo, and Zig build. The Nia-only `project_planner_build` workload
builds the source-owned application under `benchmarks/apps/project-planner`.
The sources under `benchmarks/competitive/` are
maintained language-native Rust and Zig counterparts to
`benchmarks/minimal.nia` and `examples/hello.nia`. Check modes are deliberately
described precisely:

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

The synthetic scales are generated directly into each sample's fresh workspace
from one deterministic workload specification. A scale of N means N generated
modules plus one executable root source file. Each generated module contains one
compile-time-derived constant, one module-local generic identity instance, and
one reachable value function. Flat/star roots call every module directly; the
deep chain has one root-to-module edge followed by 99 module edges; mixed
fan-out has ten root branches, each followed by a ten-module chain. All three
100-module graphs therefore contain the same 100 generated modules, 100 total
dependency edges when the executable root is included, and 100 reachable value
calls. The executable checks the exact aggregate and prints `synthetic-ok`; the
collector runs it and rejects a wrong result. The flat/star 100-module point is
the fixed medium synthetic corpus. Nia, Rust, and Zig use language-native source
spellings, so source byte counts and hashes differ, but the graph and semantic
work contract are the same.

The medium build workload runs an ordered three-process sequence in one fresh
workspace: clean, no-op warm, then one-leaf edit. The flat/star edit changes
only module 50 and changes the executable's required output from `synthetic-ok`
to `synthetic-edited`. Acceptance compares per-file source hashes, requires the
no-op artifact to remain byte-identical, requires the edited artifact to
change, and executes every artifact. It also proves that each state used a
different process and that build products were absent only before clean and
present before both non-cold states.

Project Planner is a non-generated medium Nia application. It is an ordinary multi-module `nia build` project that reads a plan
file, indexes task names, validates references and cycles, computes a
topological schedule and critical path, and formats a deterministic report.
The collector copies the complete project, builds it from source, then runs
the executable from the copied project root with `data/reference.plan`; the
sample is rejected unless the exact 63-task summary is produced. Its report
contract explicitly records one Nia tool and no synthetic generator. No Rust
or Zig translation is supplied because this workload represents real Nia
development and is not a cross-language parity claim.

Every clean sample or state sequence gets an independently created workspace.
Direct compilation starts with absent explicit Nia/Zig project caches;
build-system clean states start without Nia build/cache directories, Cargo's
target directory, or Zig's local cache and output directories. Direct rustc is
invoked without incremental compilation. The stateful medium workload retains
each native build system's project products after clean. Nia's selected
resource root, rustc's distributed sysroot, and Zig's shared global toolchain
cache are retained: clean states are project-cold rather than
SDK/toolchain-cold. OS page cache is uncontrolled and shared across the
interleaved tools. Compiler order rotates between repetitions to reduce fixed
ordering bias.

The JSON retains the experiment and Git revision identities, dirty state,
complete compiler and GNU time identities, normalized command, typed source
manifest (descriptor, tree hash, file count, and source bytes), raw
stdout/stderr, process wall/user/system time, CPU utilization, peak RSS,
artifact size and hash, state/predecessor/source-diff evidence, executable
output verification, and every raw sample.
The report shape is owned by the current maintenance source and has no
independent schema or compatibility version; Git revision and complete tool
identities provide experiment attribution. Summaries report median,
nearest-rank p95, minimum, and maximum. A report is still published when a
compiler or acceptance check fails so the failure remains auditable.

## Build Baseline

`cargo maintain baseline build` runs the representative package under
`benchmarks/build/`. It also measures a runner-only package in independent clean and
warm processes. Its build plan contains one aggregate step and no actions, so
`build_compile_runner`, `build_run_runner`, query work, and peak RSS expose the
fixed cost of compiling and executing `build.nia` without application codegen
or action-cache work. The representative fixture covers clean,
warm, source-edit, module-map-edit, corruption recovery, clean-equivalent
artifacts, and failed-action behavior.

A project-cold build starts a fresh process in a copied workspace where both
`build/out` and `build/cache` were absent before process creation. The report
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

The configure and runner caches are described in the
[build-system README](../crates/nia-build/README.md#caches).
`build_configure_plan` measures both the cached and runner paths, including
observation validation; `build_run_runner` appears only when the runner
executes. The baseline checks exact configure hits and runner executions for
each transition alongside action and native-product reuse.

Diagnostics and compiler timing records both use stderr. The baseline runner
selects a complete structured timing record for the current toolchain rather
than parsing diagnostic or timing prose.

## Machine Resource Model

Benchmarks do not select WSL, workstation, container, or rental-machine
profiles. The report records the capabilities visible to the process: CPU
affinity, system memory, and cgroup memory limit. WSL therefore reports the
Linux VM's resources, constrained containers and rental hosts report their
cgroup limits, and bare Linux hosts report system resources. Comparing results
still requires comparable hardware and system load; CI guards should remain
wide while a dedicated perf runner tracks trends.

Both reports must agree on `runner_class`, CPU model, operating system,
architecture, effective CPU limit, and effective memory limit. A shared hosted
runner label does not make different CPU generations interchangeable. A
controlled CI artifact also cannot be silently compared with a local sample.

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
identity, and installs LLVM 23 from apt.llvm.org on `ubuntu-24.04`. It builds and
measures both revisions in the same job, running each workload three times with
the `github-hosted-ubuntu-24.04-x64` runner class. Pull requests compare against
their base revision; pushes compare against the branch tip before the push;
scheduled and manually dispatched runs compare against the current revision's
parent. The reference uses its own source tree, standard library, workload
harness, and release compiler in a detached worktree. Both builds use the same
Rust and LLVM installations. Failure to build or measure an available reference
fails the job. The candidate must pass the existing comparator guards before
the workflow succeeds.

Every fully collected main-branch or scheduled candidate becomes the rolling
`nia-perf-baseline`, with its comparison report and run/revision identity retained
for 90 days. The artifact also retains `reference.json` and `reference.log` when
available, so both sides of the same-machine comparison can be inspected. The
current run remains failed when a guard fires. Artifacts record trends; later
jobs rebuild their comparison revision rather than comparing timing samples
from different hosted machines.

Pull-request candidates and main runs that fail before completing
`candidate.json` are stored separately for 14 days for diagnosis. A failed
collection retains its combined candidate log plus run/revision identity. The
first main run is an explicit bootstrap only when the selected comparison
revision predates the performance harness; every available reference is measured.
This stores main-branch trends without committing machine-specific numbers to
the source tree or treating a developer sample as a project-wide absolute
threshold.
Each run also publishes an Actions step summary containing the candidate
revision, controlled runner class, comparison revision, and comparison result
when a reference was available.

Draw performance conclusions from the complete workload path, repeated samples,
and compatible resource identity, and understand the deterministic query,
cache, allocation, and codegen counters before interpreting wall-time changes.
Hosted infrastructure, publication, cache reuse, and linking claims need
results from the managed workflow itself. Engineering principles are in
[Contributing](../docs/dev/contributing.md#engineering-principles).
