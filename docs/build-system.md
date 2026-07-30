# Nia Build System Architecture

Status: Phase B toolchain-layout migration in progress

This document owns the durable build-system boundary. The active migration and
acceptance sequence remains in `../build-std-roadmap.md`.

## 1. Product Boundary

The build system is part of the Nia toolchain in this repository. Package
registry, version solving, downloads, trust, publication, and a user-facing
package manager are separate products. Local package roots and explicit module
inputs belong to build because compiler actions cannot be reproducible without
them.

The target flow is:

```text
nia build
  -> ToolchainLayout plus host/target configuration
  -> compile and run build.nia for the host
  -> mutable std::build::Builder
  -> validate and freeze a versioned immutable BuildPlan
  -> nia-build coordinator
  -> deterministic resource-aware action execution
  -> typed compiler/process/filesystem services
  -> atomic artifacts, cache records, and diagnostics
```

The current callback runner is a bootstrap implementation, not an alternative
execution model that remains after migration.

## 2. Current Owners

| Concern | Current owner | Migration disposition |
| --- | --- | --- |
| CLI parsing and outer timing session | `crates/nia-cli/src/main.rs` | retain CLI ownership; pass typed layout/configuration |
| Package discovery, runner compilation, package lock | `crates/nia-build/src/lib.rs` | evolve into coordinator; remove checkout and global-lock assumptions |
| Build graph declaration and recursive execution | `lib/std/build/core.nia` | split builder from execution; physically delete recursive executor |
| Build API records and handles | `lib/std/build/types.nia` | replace index-only callback records with plan-owned typed values |
| Build-script error conversion | `lib/std/build/error.nia` | replace coarse exit mapping with structured diagnostics |
| Source/module loading and default std lookup | `nia-loader-query` | consume explicit `ToolchainLayout`; delete compile-time checkout lookup |
| Compiler actions and compiler cache | `nia-driver` and compiler query/codegen crates | remain compiler-owned typed actions/work products |
| Link execution and link-result reuse | `nia-linker` and `nia-driver` | remain linker/Driver-owned; build references declared artifacts |
| End-to-end build contracts | `crates/nia-cli/tests/build_cases.rs` | remain resource-accounted integration evidence |
| Build performance evidence | `tools/build_baseline.py` and `benchmarks/build/representative` | observational only; never cache truth |

## 3. Target Contracts

`ToolchainLayout` is the sole owner of compiler executable, resource root,
standard-library root, compatibility identity, supported host, and target
runtime resources. Development and installed layouts are explicit selections.
No production code reads a Rust compile-time workspace path.

The installed layout is versioned and relocatable as one directory:

```text
<toolchain>/
  bin/nia
  lib/nia/toolchain.meta
  lib/nia/std.nia
  lib/nia/std/...
```

With no override, `nia` resolves `lib/nia` relative to its executable. Source
development passes `--resource-root <checkout>/lib` explicitly. The manifest
owns the resource-layout, compiler, std, and build-protocol compatibility
identity; absolute installation paths are deliberately excluded. Layout
validation happens before user source is read, and the resolved layout is
threaded through CLI, Driver, loader, build coordinator, generated runner, and
`std::build`. There is no checkout-derived std fallback.

`BuildPlan` is immutable, versioned, deterministic to encode, and independently
validatable. It contains no function pointer, borrowed runner memory, raw
allocator, process handle, or opaque command callback. Local builder handles
include an owner identity; serialized identities derive from canonical plan
content rather than allocation order.

Every action has a typed kind, declared inputs, declared outputs, environment
policy, working directory policy, target/host role, cache policy, and resource
class. Initial action kinds cover compiler check/emit, external process,
generated file, and aggregate dependency. Raw compiler argv is not a plan
protocol.

Artifacts have typed roles and one producer. Publication is staged and atomic.
An action that fails or is cancelled cannot leave an output that appears valid.
Host tools and target artifacts remain distinct even if built from one package.

Build diagnostics carry phase, action identity, package-relative subject,
cause, and process/compiler detail where applicable. Invalid plan data, missing
resources, filesystem failures, process failures, and compiler diagnostics are
ordinary typed failures. Panic is reserved for a genuine internal invariant and
is caught by the existing ICE boundary.

Build-cache entries are immutable and content-addressed. Their identity includes
action schema, semantic inputs, toolchain/std identity, host/target, declared
environment, and dependency artifact identities. Corruption is retired and
reported as a miss reason. Timing and latest-run metadata never participate in
cache correctness.

## 4. Bootstrap Telemetry

Phase A uses an explicitly temporary observational protocol. With
`--timings=detail --timings-format=json`, the outer CLI emits the existing
`nia-timing` schema and the bootstrap runner emits one JSON line with
`kind="nia-build-actions"` and `schema_version=1`. It records declared graph
counts, started/succeeded/failed steps, action kinds, and compiler invocations.
Compiler subprocesses receive the same timing flags, so their wall/RSS and
compiler/link/cache counters remain visible as separate reports.

This report is not `BuildPlan`, is not persisted as cache truth, and gains no
compatibility promise. It exists to compare migration behavior with the
bootstrap before the bootstrap is deleted.

## 5. Current Contract Migration Matrix

| Current case | Target action/diagnostic contract |
| --- | --- |
| configured success | validated compiler emit action, explicit roots, typed executable artifact |
| configured optimization | optimization policy is a typed compiler-action field |
| executable dependency | artifact dependency and selected-step closure |
| step order | deterministic dependency order independent of completion order |
| dependency cycle | plan-validation cycle diagnostic before execution |
| unknown step | selection diagnostic listing plan-owned step names |
| missing default | plan-selection diagnostic; no implicit first step |
| missing script | package-discovery diagnostic before runner compilation |
| invalid target | target/runtime validation diagnostic with offending field |
| bare runtime executable | unsupported runtime/target diagnostic before action execution |
| duplicate target | duplicate stable artifact identity diagnostic |
| invalid output | path/output policy diagnostic; no partial directory or artifact |

Opaque custom callbacks, raw compiler arguments, fixed 16-import/48-argument
buffers, index-only handles, recursive `run_step`, and the package-wide executor
lock are explicitly deleted rather than mapped as supported target behavior.

## 6. Representative Workloads

The representative fixture covers multiple requirements in one real package:

| Workload state | Evidence |
| --- | --- |
| clean build | runner bootstrap, two artifacts, generated source, compiler/link execution |
| warm build | no-op package state and exact compiler/link cache counters |
| source edit | invalidation of one source-dependent artifact |
| module-map edit | runner/plan change and changed explicit module input |
| failed action | typed failed step with no successful-build claim |
| multi-artifact package | graph and artifact counts plus deterministic selected closure |

`tools/build_baseline.py` copies the fixture to an isolated temporary directory,
runs these states sequentially, checks current available memory before every
process, enforces a timeout, kills the complete subprocess group on timeout, and
records machine resource identity. It must not be called implicitly by cheap
unit tests.

The 2026-07-29 Phase A sample on a Linux/WSL resource view with 8.19 GB effective
memory recorded clean/warm wall time of 14.21/4.53 seconds and outer peak RSS of
869/545 MB. Both warm compiler actions had exact object and link-result reuse;
a source edit missed one object and one link, a module-map edit missed two
objects and one link, and the selected failed action ran one failed step with
zero compiler invocations. This is single-machine architecture evidence, not a
performance threshold.

## 7. Proposal Decision Gates

Language/compiler proposals are reviewed here when they can alter plan values,
build-script capabilities, ownership/lifetimes, error propagation,
compile-time/runtime capability, or host/target semantics. A proposal is
recorded as one of:

- decided dependency with an owner and required phase;
- compatible future extension that does not block the current contract;
- unresolved gate that prevents only the affected API from stabilization;
- rejected dependency with the build/std alternative recorded.

Unresolved proposal semantics are not guessed through an experimental std API.

## 8. Test And Resource Discipline

Cheap unit tests may inspect Rust planning and generated source strings. A test
that starts a complete compiler, LLVM backend, runner, or nested build belongs
in a resource-accounted integration harness. Default `cargo test` keeps natural
libtest concurrency; resource limits are derived from effective CPU, VM/system
memory, and cgroups rather than hidden WSL or CI profiles.

Subprocess tests and baseline tools use bounded time and process-tree cleanup.
The end-to-end configured build case is the owner of generated-runner codegen
coverage; a duplicate unaccounted unit-test compilation is not permitted.
