# nia-build

`nia-build` owns the Rust side of `nia build`. It resolves one package
invocation, compiles and runs the package's `build.nia`, validates the emitted
plan, and executes the selected dependency closure. Package registries, version
solving, downloads, publication, and network trust are outside this crate.

The execution boundary is deliberately split:

```text
nia build
  -> resolve BuildInvocation and ToolchainLayout
  -> compile and run build.nia for the host
  -> std::build::Build validates and encodes a draft
  -> BuildPlan::decode validates and freezes the graph
  -> coordinator executes the selected closure
  -> typed Driver, linker, process, and filesystem operations
```

The generated runner is a configuration process. It can inspect its injected
package, toolchain, host, artifact-target, optimization, and requested-step
values and construct a graph through `std::build`; it cannot execute graph
actions. The decoded immutable `BuildPlan` is the coordinator's only execution
input.

Package manifests are resolved before runner execution. The local package
resolver fingerprints declared source content, applies explicit dependency
classes, and keeps filtered optional, lazy, and test dependencies out of the
selected graph. `LocalPackageGraph::report` exposes declared versus resolved
edges without loading source modules.

The runner wrapper remains owned by `nia-build`, rather than becoming part of
the public `std::build` API, because it is a private protocol bootstrap: it
binds runner-config and plan protocol identities, cache inputs, and the
host invocation lifecycle. Build scripts need only the stable graph-building
types and operations exposed by `std::build`; exposing the wrapper would make
compiler/build transport details part of the language-visible API.

The runner itself is always compiled for the host at O0. The requested
optimization remains an explicit configuration value used by artifact actions;
it is not the compilation policy for the short-lived plan constructor. Runner
cache identity therefore excludes artifact optimization while retaining inputs
that can change the runner binary, including profile, test mode, host target,
build source, standard-library source, toolchain identity, and build protocol.

## Source Ownership

- [`src/lib.rs`](src/lib.rs) owns package discovery, invocation paths, runner
  generation and execution, and plan handoff.
- [`src/runner.nia`](src/runner.nia) is the private generated-runner wrapper,
  and [`src/runner_config.rs`](src/runner_config.rs) owns the private bounded
  configuration passed to it. [`src/runner_cache.rs`](src/runner_cache.rs)
  owns the content-addressed cache for compiled host runners.
- The immutable plan model lives in the separate
  [`nia-build-plan`](../nia-build-plan/) crate, re-exported from this crate:
  - [`src/lib.rs`](../nia-build-plan/src/lib.rs) owns stable keys, typed
    logical paths, and the immutable plan model.
  - [`src/validation.rs`](../nia-build-plan/src/validation.rs) owns
    freeze-time canonicalization and complete graph validation.
  - [`src/actions.rs`](../nia-build-plan/src/actions.rs) owns action-local
    semantic validation, including typed artifact use and external command
    contracts.
  - [`src/dependencies.rs`](../nia-build-plan/src/dependencies.rs) owns
    producer and dependency closure validation for modules, artifacts,
    actions, and steps.
  - [`src/codec.rs`](../nia-build-plan/src/codec.rs) owns the registered
    binary plan protocol. Decoding always returns through semantic freeze.
  - [`src/handoff.rs`](../nia-build-plan/src/handoff.rs) owns durable
    canonical plan publication.
- [`src/coordinator.rs`](src/coordinator.rs) owns selected-closure scheduling
  and typed action execution.
- [`src/action_cache.rs`](src/action_cache.rs) and its child modules own
  build-action records and invalidation reasons. Compiler object, archive, and
  link products remain owned by `nia-driver` and `nia-linker` caches.
- [`src/output_recovery.rs`](src/output_recovery.rs) owns recoverable
  multi-output publication, while [`src/lock.rs`](src/lock.rs) owns
  cross-process coordination for equal logical outputs.
- [`src/resources.rs`](src/resources.rs) maps declared action resource classes
  onto inherited query-session capacity.
- [`src/process_output.rs`](src/process_output.rs) owns bounded subprocess
  output capture and cleanup of owned process trees.
- [`src/report.rs`](src/report.rs) is the only presentation bridge for build
  bootstrap and coordinator failures: it assigns stable diagnostic codes and
  bounds child output in text and JSON reports.

The public build-script API and its ownership rules live with the Nia sources
in [`lib/std/build.nia`](../../lib/std/build.nia) and
[`lib/std/build/`](../../lib/std/build/). Language-visible build behavior is
specified in [`docs/language-spec.md`](../../docs/language-spec.md); the outer
CLI and compiler boundary are described in
[`docs/architecture.md`](../../docs/architecture.md).

## Plan Contract

`BuildPlan` contains stable package, module, artifact, action, and step keys. A
logical path carries a typed package, build, cache, toolchain, or artifact root;
physical checkout and installation paths are resolved only for the current
invocation. The plan contains no callback, allocator, process handle, borrowed
runner storage, or raw compiler argument vector.

The std builder rejects foreign handles before encoding. `BuildPlan::freeze`
then canonicalizes graph order and rejects invalid names and paths, duplicate
identities, unresolved references, dependency cycles, conflicting output
ownership, target mismatches, and missing producer closure. Protocol decoding
repeats these semantic checks rather than trusting runner-produced bytes.

Targets cross the runner and plan protocols as canonical target names such as
`aarch64-apple-macos`. Decoding rejects a name outside the maintained target
matrix; every other target fact is derived from the name, never transported
beside it. `TargetView` exposes the name, its architecture, vendor, OS, and
environment components, and the toolchain-derived byte order and pointer width.

The Nia encoder bounds every collection count by the registered `maxItems`
limit before narrowing it to the wire `u32` representation. Aggregate counts
and derived dependency/input/output counts use checked addition, so malformed
in-memory state is rejected instead of wrapping into a smaller protocol value.
Generated-file payload lengths are likewise checked before their fixed-width
length prefix is written.

The Rust encoder applies the 64 MiB plan budget at each raw write. Once a write
would exceed that budget it retains the first `TooLarge` error and stops growing
the output buffer, rather than allocating the complete oversized encoding and
rejecting it only at `finish`.

The decoder does not reserve typed list capacity from a runner-controlled count.
It grows a list only after each item has consumed and validated its bytes, so a
truncated count prefix cannot amplify a tiny draft into `count * size_of(T)` of
host allocation.

The registered protocol identities in `nia-compat` are the only schema and
namespace authorities. The release-line epoch follows the latest public
release, so internal `0.2.0-dev` changes keep the published `0.1.0` epoch.
Changing an encoded field or persistent cache input must update its owning
identity and is not a promise to read or migrate data from an earlier release.

## Execution And Publication

Runner and external-command output is drained concurrently into bounded tails.
Every started capture worker is joined, including when its sibling fails to
start or panics. Process-wait and stream read/forward failures retain all tails
returned by completed workers. Reports identify the failed operation and stream;
test-suite and staging-cleanup reports preserve the original action's context.
Text commands may forward live child output; JSON commands retain it in notes
without mixing child bytes into the diagnostic envelope.

The frozen plan's host and artifact targets must match the current invocation
before action selection or output preparation. Mismatch reports identify the
role and show the invocation target as expected and the plan target as found.
The CLI freezes a new plan from each invocation. A validated configure record
may be reused only when its non-poisoned observations recompute against the
current package root and its plan decodes through semantic freeze; corrupt,
missing, or changed observations fall back to normal runner configuration.
`build-plan.bin` remains a durable observation, never an unchecked execution
input.

Filesystem observation records distinguish content, metadata, directory
entries, and presence. Presence observations preserve an absent optional path
as a cacheable fact; creating, removing, or changing its entry kind invalidates
the configuration. Existing ancestors must resolve inside the package root,
including when several trailing path components are absent. Unresolvable
symlinks and unreadable paths remain errors rather than recorded absence.
Cache input files and directory trees also reject Windows reparse points during
both identity passes, preserving the same boundary on Windows.
Environment observations use the same explicit-absence rule and reject
non-UTF-8 values instead of silently changing the cache identity.
The runner transports bounded file-content, file-presence, directory,
environment, command, Git, and tool evidence in the draft envelope.
`Build::probeFile` requires caller-owned bounded storage;
the host re-reads the reported bytes under the package-root boundary before
canonicalizing them into the BLAKE3 manifest. `Build::probeFilePresence`
records absent, file, directory, and other-entry states. Environment records
distinguish absent and empty values. Malformed records, path escapes, oversized
values, duplicate facts, and read-after-run changes are rejected and cannot
enter the configure cache. Command, Git, and tool probes use separate
executable identity records. Host validation resolves the program through
`PATH`, runs its canonical path, and fingerprints its contents, request, exit
status, and bounded output.

External-command results are cached only under an explicit policy. Successful
test suites use a separate completion cache keyed by the executable, source
manifest, declared inputs, environment, toolchain, and test action contract;
failed or cancelled suites never publish records. Both
`DeclaredInputs` and `ToolAndInputs` retain tool content identity and require a
cleared environment with explicit declarations; the latter builder method also
selects that environment policy. `FullDeclaredEnvironment` retains a bounded
inherited environment snapshot and rejects explicit overrides. Oversized
snapshots execute normally and report an uncacheable miss without publishing
an external-command entry. Declared directory inputs use a deterministic
streamed identity with bounded encoded size, depth, and entry count; oversized
or excessively nested trees fail as typed input errors.

For maintenance and CI, `nia build --dry-run` validates the selected closure
without executing coordinator actions. `--print-plan`, `--explain`,
`--print-cache`, and `--print-observations` expose deterministic graph and
cache evidence. `--diagnostics-format=json` switches these inspection products
to newline-delimited stable JSON records: plan records include all canonical
graph nodes, explanation records include observations and tools, and action
cache records include typed decisions and publication state. Dry-run cache
records explicitly report that lookup and publication were not attempted.

The runner receives the toolchain's complete maintained target matrix. An
artifact without a target builds for the invocation artifact target;
`forTarget` selects any other member, such as `b.hostTarget()` for a tool the
build runs or `b.target("x86_64-apple-macos")` for an additional artifact.
Each compiler action carries its resolved target, and the coordinator creates
one driver per distinct target in the plan. Run, test and command programs must
be built for the host target.

The coordinator executes deterministic readiness waves through `QuerySession`,
so build actions share the process jobserver and compiler resource budgets.
`Cpu` and `Io` actions reserve one action slot; `Conservative` actions reserve
the complete action capacity. `--jobs` can reduce ready-action parallelism but
does not create a separate executor or replace LLVM memory backpressure.

Actions publish only declared build-root outputs. Equal logical destinations
share a cross-process lock; unrelated destinations may proceed independently.
Before staging, every output parent is created and checked component by
component beneath the invocation build root; symlinks and Windows reparse
points are rejected so a logical output cannot redirect publication elsewhere.
A lock is stale only when no live owner holds it: Unix reclaimers take the
owner's `flock`, and Windows reclaimers need an exclusive open that deletes the
same file object on close. Neither platform reclaims a live lock by age.
File and directory outputs use staged same-filesystem transactions. The
checksummed journal and prepared marker make interrupted publication recoverable
without guessing which partial destination is valid. Corrupt or contradictory
recovery state is a typed failure. Recovery rejects a journal namespace that
exceeds its bounded entry budget, and validates directory outputs with a
streaming traversal so malformed state cannot force an unbounded per-directory
allocation.

Generated-file and eligible external-command entries store complete validated
payloads. Compiler-check entries record only a prior successful zero-diagnostic
check against a complete source manifest. Compiler-emit entries bind build
identity to a Driver-owned executable cache reference rather than duplicating
the executable. Cache I/O and corruption produce explicit misses and never
replace the ordinary correctness path. Action, configure, and runner cache
records are opened as regular files without following symlinks or Windows
reparse points; recovery journals and lock-owner records use the same
no-follow rule. A redirected or malformed record is rejected and follows the
ordinary miss or recovery-error path.

## Verification

Focused owner tests should run before broad workspace gates. Tests that start a
compiler, LLVM backend, generated runner, nested build, or subprocess must use
the repository's resource-accounted harness and bounded process-tree cleanup.
The principal end-to-end contracts live in
[`nia-cli/tests/build_cases.rs`](../nia-cli/tests/build_cases.rs); the
representative clean/warm/edit/corruption workload lives under
[`benchmarks/build/`](../../benchmarks/build/) and is run through
`cargo maintain baseline build`. The maintenance command is documented in
[`maintain/README.md`](../../maintain/README.md).

Historical migration matrices, dated measurements, and hosted-run identifiers
belong in Git history and generated evidence, not in this maintained contract.
