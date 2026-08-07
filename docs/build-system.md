# Nia Build System Architecture

Status: Phases A-F complete; Phase G artifact and package-boundary work in progress

The Rust-side `BuildInvocation` is resolved bootstrap state: package and
toolchain paths, requested step, runner locations, and timing options. It is not
the graph protocol and no longer uses the `BuildPlan` name.

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

The generated runner is a configuration process: it constructs, validates, and
encodes the graph, but it cannot execute an action. The decoded frozen plan is
the coordinator's only execution input.

## 2. Current Owners

| Concern | Current owner | Migration disposition |
| --- | --- | --- |
| CLI parsing and outer timing session | `crates/nia-cli/src/main.rs` | retain CLI ownership; pass typed layout/configuration |
| Package discovery and runner compilation | `crates/nia-build/src/lib.rs` | retain invocation ownership with isolated transient paths |
| Build graph declaration | `lib/std/build/core.nia` and `plan.nia` | retain builder ownership and codec; execution is physically absent |
| Build API records and handles | `lib/std/build/types.nia` | replace index-only callback records with plan-owned typed values |
| Build-script error conversion | `lib/std/build/error.nia` | structured operation/subject/cause errors implemented; add package/action identity at frozen-plan handoff |
| Source/module loading and default std lookup | `nia-loader-query` | consume explicit `ToolchainLayout`; delete compile-time checkout lookup |
| Compiler actions and compiler cache | `nia-driver` and compiler query/codegen crates | remain compiler-owned typed actions/work products |
| Link execution and link-result reuse | `nia-linker` and `nia-driver` | remain linker/Driver-owned; build references declared artifacts |
| Build-action cache | `nia-build` | generated-file and zero-diagnostic compiler check/emit slices implemented; external commands remain gated on complete input closure and restorable multi-output identity |
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

The graph plan is created only by freezing a `BuildPlanDraft`. Package, module,
artifact, action, and step keys combine a package key with a validated visible
name. Package-rooted logical paths also carry their package key, preventing
equal relative paths in different packages from aliasing. Freeze canonicalizes
node and reference order and validates references, cycles, output roots, and
single-producer ownership before returning an immutable value. Build-rooted
module roots and imports are generated inputs: freeze resolves their exact
generated-file or external-command output owner and rejects a compiler
check/emit step unless that producer is in its transitive dependency closure.
Artifact-root command inputs use the same dependency-action closure traversal.

The root package is implicit. `Build::addPackage(PackageOptions::init(name,
root))` declares another local package and returns an owner-checked
`PackageHandle`; `root` is a canonical relative path beneath the invocation's
root package, never an absolute host path. `CommandArgument::packageInput`
requires that handle, so the same protocol path in two packages remains two
different inputs. Package declarations encode both stable key and relative
root, and the coordinator resolves the physical directory only for the current
invocation. Empty external roots, `.`/`..`, backslashes, duplicate keys or
roots, and the reserved `root` key are rejected. Registry lookup, versions,
downloads, and network policy remain absent. `ModuleOptions::fromPackage` and
`ModuleImport::fromPackage` explicitly select one declared local package for
compiler sources and module-map entries; the module declaration itself remains
owned by the root build graph.

The binary codec uses `ToolchainLayout`'s build-protocol schema as its sole
version source. It bounds the full envelope, collection counts, strings, and
generated-content blobs separately; rejects bad magic, unknown versions/tags,
invalid UTF-8/names/paths, truncation, trailing bytes, and semantic invalidity;
and always routes decoded drafts back through freeze. The runner receives that
schema value as data and writes an exclusive, synced Nia-encoded draft after
builder validation. The coordinator decodes and freezes the draft, re-encodes
canonical allocation-order-independent bytes, writes and syncs a same-directory
temporary file, atomically renames it, syncs the parent directory, and removes
the runner draft on every normal success or failure path. The durable handoff is
`.nia-build/build-plan.bin`; a failed runner cannot replace its last valid
contents.

Each build invocation compiles to a process/sequence-qualified runner executable
and writes a matching private draft. Both transient files are retired when the
runner finishes. Concurrent invocations therefore never remove, execute, or
decode one another's runner state; canonical `build-plan.bin` publication
remains an atomic last-completed observation and is not the execution truth for
an already decoded plan.

The invocation also supplies one normalized optimization default from the
global `-O*` option. `ModuleOptions` retains absence separately from an explicit
mode until `Build::addModule` resolves inheritance, so the frozen plan contains
only concrete module optimization values. A module-level `withOptimization`
always wins over the invocation default. There is no named build profile yet:
with only one inherited policy dimension it would be an alias for optimization,
not an independent contract. Executable target and runtime do not participate
in this precedence chain; the current executable artifact kind derives the
artifact target and freestanding runtime from its role.

After the runner exits, the coordinator decodes and freezes the draft before
any action can run. It publishes canonical plan bytes and executes only the
selected dependency closure. The closure uses iterative deterministic Kahn
traversal and executes a shared action at most once. Aggregate actions are
no-ops; compiler check and executable emission call `nia-driver` directly with
typed module maps, optimization, runtime, target, cache, and output values.
Generated-file actions publish declared bytes atomically under the build root.
`ModuleOptions::fromBuild` and `ModuleImport::fromBuild` expose generated root
sources and imports without treating them as package files. Builder validation
adds their exact producer edges after all steps have been declared, making the
graph independent of declaration order; freeze repeats the closure check at the
protocol boundary.
`addRunExecutableStep` requires an existing emit step for the declared artifact,
adds that producer dependency, and encodes the artifact as an external-command
program with a package-root working directory and no declared outputs. Freeze
independently rejects an Artifact-root command lacking the matching compiler
emit in its dependency closure. An ordinary external command can declare the
same relationship through `CommandArgument::artifactInput`: the builder
validates the handle owner, requires its emit step, retains the typed artifact
input, and adds the producer edge. Freeze applies the same closure check to
every Artifact-root program and input. The coordinator closes stdin, forwards
stdout/stderr while retaining a bounded 64-KiB tail for each stream, enforces a
seven-minute timeout, and retires the owned process group on timeout or after
the leader exits. Spawn, wait, timeout, capture, and nonzero-exit failures retain
action, program, cwd, argument count, status, and output context.
`addTestExecutableStep` records the same typed artifact-to-emit relationship
with test intent and retained `RunOptions` arguments. It intentionally lowers
to the same uncacheable process action as `addRunExecutableStep`; test results
must never be mistaken for reusable build outputs, while the graph still
rejects missing or foreign emit ownership.
External-command arguments distinguish literals, declared inputs, and declared
outputs. Every output argument resolves to its own path in one
coordinator-owned same-filesystem transaction directory. Before publication,
all produced values must be regular files and are synced. The coordinator holds
all destination locks, backs up every accepted destination, installs the new
files, syncs every affected parent, and marks the transaction accepted only
after the complete set is present. Any ordinary failure before that acceptance
point restores backups in reverse order and removes destinations that did not
previously exist. This is a recoverable multi-path transaction, not a claim that
separate filesystem directory entries switch in one indivisible rename.
Explicit uncacheable actions remain unsupported; no path falls back to a runner
callback.

Process-death recovery state for these transactions lives under
`.nia-build/.nia-transactions/v1/`, outside the disposable cache. A versioned,
checksummed journal records the stable action key, ordered logical Build
outputs, and logical stage/commit paths before the external command can mutate
staging. Once every produced regular file is synced, a separately checksummed
prepared marker records which destinations previously existed; that marker and
its directory entry are synced before any destination changes. The
same-directory stage-to-committed rename remains the acceptance point.

Before dispatching plan actions, the coordinator scans journals in deterministic
order and acquires their complete output-lock sets in canonical order. It then
rereads the journal under those locks to reject replacement while waiting.
Unprepared staging is discarded without touching destinations; a prepared but
unaccepted transaction is rolled back in reverse order; an accepted transaction
keeps its outputs and retires committed backup state. Rollback moves installed
files back into staging so recovery itself can be interrupted and repeated.
Missing, truncated, trailing, checksummed-corrupt, non-regular, or contradictory
state produces a typed recovery failure instead of guessing which output is
valid. On Linux, unpublished temporary journals carry PID/start-time identity,
so later invocations collect dead owners without removing a live publisher.
Build plans reserve `.nia-transactions` as coordinator-owned output space.

The former package-wide executor lock is absent. Every output-producing action
derives a stable coordination key from its validated build-root logical path and
acquires that cross-process lock under `.nia-cache/coordination/output-locks/`
for the action lifetime. Equal destinations serialize across concurrent builds;
different destinations use different locks and may progress independently.
Compiler object/link cache publication remains owned by the Driver cache rather
than being folded into this build-output lock namespace. Owner records include
process identity, process start time, and an acquisition sequence; dead owners
are reclaimed without allowing an older same-process guard to remove a newer
lock.

The selected closure executes in deterministic readiness waves. Each wave is
submitted to a `QuerySession`, so build actions share the process-wide
Cargo/GNU Make jobserver budget with compiler queries instead of creating a
private worker pool. Target-specific Drivers are constructed once per
coordinator invocation and shared by actions in the wave. Completion order is
not observable: steps, actions, and failures are merged in canonical plan order,
and single-worker and multi-worker executions therefore produce the same
report.

Every action also has a declared resource class. Compiler actions are `Cpu`,
generated-file and aggregate actions are `Io`, and explicitly uncacheable
actions are `Conservative`. External commands use the public
`ActionResourceClass` selected through
`ExternalCommandOptions::withResourceClass`; the default is `Conservative`
because an undeclared tool may consume CPU, memory, or nested process capacity.
Within a readiness wave, `Cpu` and `Io` actions each reserve one action slot,
while a `Conservative` action reserves the complete action capacity and cannot
overlap another action. The protocol encodes this declaration, and unknown enum
values or protocol tags are rejected rather than downgraded.

`nia build -j N` / `--jobs N` and
`BuildRequest::with_max_parallel_actions` place a nonzero upper bound on ready
build actions submitted from each wave. The bound is combined with, and can
only reduce, the `QuerySession` executor's inherited capacity. It does not
create another executor, alter the process jobserver, or replace compiler and
LLVM memory backpressure. `--jobs=1` is therefore the deterministic
single-build-action-worker mode; compiler actions still use their existing
internally resource-accounted query and LLVM paths.

The effective action-resource capacity is the minimum of this optional bound
and inherited `QuerySession` capacity. Timing output reports it as
`build.action_resource_capacity` and counts dispatched classes with
`build.resource_class_conservative_actions`,
`build.resource_class_cpu_actions`, and
`build.resource_class_io_actions`. These counters describe coordinator
scheduling; they do not claim that `Io` work is free or disable nested compiler
resource accounting.

Each wave also owns an ordered failure token. A failing action prevents all
dependent waves and cancels later canonical actions, while earlier actions are
allowed to settle so that the first reported failure cannot change with worker
timing. Cancellation is checked before execution, while waiting for an output
publication lock, and in the external-process wait loop. Cancelled process
actions terminate their owned process group and retire staged output; active
in-process compiler work settles under its existing resource budgets before the
coordinator returns. Successful independent output that completed before the
failure remains accepted.

Bootstrap `StepHandle`, `ModuleHandle`, and `ExecutableHandle` values already
carry a private process-local owner id beside their index. Every API receiving a
handle rejects a different live builder before indexing its collections. The
owner id comes from a monotonic atomic counter and is deliberately absent from
serialization, diagnostics, fingerprints, and future stable plan keys.

Bootstrap modules also carry an explicit retained name. Graph construction
rejects invalid or duplicate module names instead of deriving identity from a
root-source path or insertion index. Module, executable, and step names use the
same stable-name alphabet as protocol keys, and duplicate module imports are
rejected before retention. Before the draft is written, the builder resolves
the requested/default step and validates the entire dependency graph, including
unselected components. A successful result is cached only until the next graph
mutation. Decoder freeze remains the production validation owner, so malformed
or semantically invalid bytes cannot reach coordinator execution.

Every action has a typed kind, declared inputs, declared outputs, environment
policy, working directory policy, target/host role, cache policy, and resource
class. Initial action kinds cover compiler check/emit, external process,
generated file, and aggregate dependency. Raw compiler argv is not a plan
protocol.

Artifacts have typed roles and one producer. Publication is staged and atomic.
An action that fails or is cancelled cannot leave an output that appears valid.
Host tools and target artifacts remain distinct even if built from one package.

Driver invocations own their effective artifact target. A normal Driver starts
from the layout-selected artifact target, while build-runner compilation
explicitly overrides that value with the layout host target. The generated
runner receives complete host and artifact descriptors as separate arguments
and passes borrowed `TargetView` values into `std::build`; the builder retains
owned copies. Executable records carry only the selected artifact role. Before
execution, the coordinator verifies that both plan-level targets match the
resolved toolchain invocation, then creates target-specific Driver sessions
from each compiler action's typed target rather than reconstructing CLI argv.

Build diagnostics carry phase, action identity, package-relative subject,
cause, and process/compiler detail where applicable. Invalid plan data, missing
resources, filesystem failures, process failures, and compiler diagnostics are
ordinary typed failures. Panic is reserved for a genuine internal invariant and
is caught by the existing ICE boundary.

Builder `Error` values distinguish invalid input, internal invariants, cycles,
and contextual construction/handoff failures. The generated runner reports
path and target decoding, construction cleanup, build-script, plan encoding,
and final deinitialization failures without collapsing them to an unexplained
exit code. Coordinator errors add package/action identity to target mismatch,
logical-root, module-map, unsupported-action, and Driver failures.

Build-cache entries are immutable and content-addressed. Generated-file actions
fingerprint their stable action key, logical output identity, byte contents,
and separate compiler, resource-layout, std, and build-protocol compatibility
components. Successful compiler-check and compiler-emit actions additionally
have bounded action records.

Build protocol schema 6 adds canonical local-package root declarations and
typed package-rooted command inputs. It retains the schema 5 separation of
external-command environment and cache policies.
`ExternalCommandOptions::search` defaults to inherited process
environment plus `Uncacheable`, preserving the behavior of existing build
scripts. `withClearedEnvironment` starts the child with an empty environment,
after which `withEnvironment` supplies the command's explicit values.
`withDeclaredInputCache` declares that every semantic file input is represented
by a typed command input. Plan freeze accepts that declaration only with a
cleared environment and at least one declared output.

Eligible external commands have immutable records under
`.nia-cache/actions/external-commands/v2/`. Their identity includes the stable
action and command declaration, logical working directory, explicit
environment, logical paths plus contents of declared regular-file inputs,
logical dependency-artifact inputs when present, ordered logical outputs, the
referenced package-key/package-root mappings, and separate compiler,
resource-layout, std, and build-protocol components. Search
programs are resolved before lookup; the declared search name and resolved
executable bytes form the tool identity, while its absolute installation path
does not.

Each record stores every regular-file output with an independent length and
checksum. A hit restores all payloads through the same journaled multi-output
transaction used after process execution, so readers cannot accept a partial
set. Truncation, trailing bytes, checksum or identity damage, and content-path
mismatch retire the exact record and become typed misses. Read and publication
failures remain nonfatal cache misses. If the tool or inputs change while the
process runs, the accepted outputs remain valid for that invocation but are not
cached. Concurrent publishers accept only identical output sets for one
identity; differing results expose undeclared state as a write miss rather than
replacing the first immutable record.

Inherited and explicitly uncacheable commands never perform a lookup or report
a hit. A `DeclaredInputs` assertion remains the build script's responsibility:
the cache cannot prove that a command refrained from inspecting undeclared cwd
state. Artifact inputs name the complete emitted file, resolve through the
artifact declaration, and use a cache fingerprint component separate from
ordinary declared files so dependency changes have an exact invalidation
reason.

Compiler requests nevertheless preserve the plan identity needed to close that
input boundary. Each package-, build-, cache-, toolchain-, or artifact-rooted
source has a physical path for current-invocation I/O and a stable logical
`SourcePath` identity derived from its typed root and protocol path. The Driver
passes both roles intact into the loader, whose recursive module discovery
derives child physical paths and logical identities in parallel. Moving the
same package tree therefore leaves its recursive logical source manifest
unchanged.

The loader materializes that closure as a stable source-input manifest and the
Driver exposes it without duplicating module discovery. Entries distinguish a
missing source from a present source's content fingerprint and byte length; an
aggregate program fingerprint exists only when every entry is present. Dynamic
provider modules remain part of the same loader graph. The existing persistent
provider-demand plan validates its recorded source closure before restoring
those demands, allowing a warm loader session to reconstruct the complete
manifest across process and toolchain relocation.

Build-rooted module roots and explicit module-map imports enter compiler-action
identity through this same Driver source-input manifest. Their producer actions
control scheduling and materialize the files, but producer recipes are not an
additional compiler-input identity: if a recipe changes while its output bytes
remain identical, the consuming compiler action may still hit. Changing either
a generated root or generated import invalidates the exact dependent compiler
closure as `Sources`; an unrelated package-source artifact remains reusable,
and the converse holds for edits to that package source.

Compiler-check records live under
`.nia-cache/actions/compiler-checks/v1/`. Their identity includes the stable
action key, module root and import mapping, target, optimization, runtime,
sorted logical source identities with content fingerprints and lengths, and
separate compiler, resource-layout, std, and build-protocol components.
Absolute physical paths are excluded. Lookup asks the loader for a pre-check
manifest; an exact hit proves only that the same action previously completed
with zero diagnostics, so it can skip semantic Driver checking but not source
closure validation.

On a miss, the Driver returns the checked program together with the final
manifest from that exact loader session after provider discovery. Publication
uses that final manifest, never the earlier lookup candidate. A record is
published only for a successful check whose diagnostic list is empty and whose
source closure is complete. Warnings, missing sources, failed checks, and
incomplete manifests remain explicitly uncacheable. The record stores no
session-local checked program and restores no compiler emit output; object and
link products remain owned by the Driver caches. Reads repeat and validate all
canonical identities and fingerprints, reject truncation and trailing bytes,
and retire corruption under a scoped mutation lock. Source, module, target,
optimization, runtime, and toolchain component changes remain distinct miss
reasons.

Compiler-emit records live under
`.nia-cache/actions/compiler-emits/v1/`. They reuse the complete compiler
source/module/target/toolchain identity with the effective freestanding runtime
and additionally bind the declared artifact identity and runtime, logical
output, and current linker/target/options environment to a fixed-width typed
Driver executable-cache reference. Executable bytes are not duplicated in the
build cache. On a hit, the coordinator asks the Driver to validate the full
current link environment and restore its owned link product atomically.

A missing, corrupt, unreadable, or invalidated Driver referent retires only the
matching build binding before ordinary compile/link fallback. A replacement
binding cannot overwrite a live different reference. Publication uses the
final loader manifest paired with that Driver compilation and requires a
successful zero-diagnostic emit, a complete closure, and successful Driver link
cache publication. Warnings and uncacheable link environments never publish an
emit record. Relocation changes physical source and destination paths without
changing their logical identities, so a shared cache can restore the executable
into the new build root.

Generated-file entries live in a versioned action-kind namespace under
`.nia-cache/actions/`. Their envelope repeats the action key fingerprint,
complete action fingerprint, component fingerprints, canonical logical output,
payload length, and payload checksum. Reads reject truncation, trailing data,
identity mismatch, and payload corruption. Corruption is retired under the same
entry-mutation key used by publishers, so a stale reader cannot remove a newer
valid entry.

Publication writes and syncs a same-directory temporary regular file, then
installs it with a no-overwrite hard link. Duplicate publishers either install
the identical immutable entry or validate the already accepted entry; ordinary
readers remain lock-free and can observe only absence or a complete validated
envelope. A hit validates the current destination and restores it through the
existing generated-output atomic publication path when it is absent or stale.
Cache read/write failures remain explicit nonfatal miss outcomes and never
replace the output correctness path. Timing and latest-run metadata do not
participate in cache correctness.

This publication contract is exercised across processes as well as threads. A
fixed stress probe starts independent publishers and lock-free readers against
one shared multi-megabyte entry from a common barrier. Readers may observe only
`NotFound` or the complete validated payload; after every process settles, the
cache key contains exactly one entry and no staging file or mutation lock. Test
workers are owned as an RAII child set by the probe, so timeout or assertion
failure waits for their termination rather than leaking background work.

## 4. Execution Telemetry

With `--timings=detail --timings-format=json`, the outer CLI emits the existing
`nia-timing` schema. Coordinator step/action totals, failures, build-action
cache hits and typed miss/invalidation reasons, and Driver
compiler/link/cache counters share that report because compiler actions now run
in the coordinator process. These observational counters are not `BuildPlan`,
are not persisted as cache truth, and gain no compatibility promise.
An explicitly configured build-action limit is reported as
`build.action_parallelism_limit`; absence means inherited capacity was not
further constrained.

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
| build optimization | invocation default plus exact per-module override in the frozen plan |
| duplicate target | duplicate stable artifact identity diagnostic |
| invalid output | path/output policy diagnostic; no partial directory or artifact |

Opaque custom callbacks, raw compiler arguments, index-only handles, and
recursive `run_step` are deleted rather than mapped as supported target
behavior. Invocation-private runner/draft paths and stable per-output locks have
replaced the package-wide executor lock. Builder validation rejects duplicate
module names and cycles anywhere in the declared graph before writing the draft;
decoder freeze is the single production graph validation boundary.
An explicit selected step is sufficient when a script intentionally has no
default; a nonempty plan with neither selection nor default is invalid.

## 6. Representative Workloads

The representative fixture covers multiple requirements in one real package:

| Workload state | Evidence |
| --- | --- |
| clean build | runner configuration, generated source, two artifacts, coordinator compiler/link execution |
| warm build | no-op package state and exact compiler/link cache counters |
| source edit | invalidation of one source-dependent artifact |
| module-map edit | runner/plan change and changed explicit module input |
| failed action | explicit uncacheable action rejection with no successful-build claim |
| multi-artifact package | graph and artifact counts plus deterministic selected closure |

`tools/build_baseline.py` uses the explicit resource root and copies the fixture
to a fresh isolated temporary directory for each repetition. It runs these
states sequentially, checks current available memory before every process,
enforces a timeout, kills the complete subprocess group on timeout, and records
machine resource identity. The default three-sample report retains every raw
run, records warm action/object/link acceptance, and summarizes wall, RSS,
runner compilation, runner execution, and plan execution with median, p95, min,
and max values. It must not be called implicitly by cheap unit tests.

The 2026-07-29 Phase A sample on a Linux/WSL resource view with 8.19 GB effective
memory recorded clean/warm wall time of 14.21/4.53 seconds and outer peak RSS of
869/545 MB. Both warm compiler actions had exact object and link-result reuse;
a source edit missed one object and one link, a module-map edit missed two
objects and one link, and the selected failed action ran one failed step with
zero compiler invocations. This is single-machine architecture evidence, not a
performance threshold.

The 2026-08-01 Phase F three-sample run recorded clean/warm wall medians of
17.06/5.82 seconds. Warm time split into 4.73 seconds compiling the build
runner, about 0.003 seconds executing it, and 1.02 seconds validating/executing
the plan. Every first warm build hit its generated action entry, reused all 126
runner objects and all three link results, and reported zero object/link misses.
The dominant remaining cost is therefore runner compilation and compiler-plan
validation, not execution of `build.nia`.

A 2026-08-02 production-path `CompilerEmit` action-cache integration sample
sharpens that split. It was executed by the then-unoptimized test-profile
compiler and is not comparable to the release baseline above. The copied
`configured_optimization` fixture took 90.947 seconds cold, with 86.340 seconds
compiling the runner and 4.598 seconds executing the plan. The unchanged run
took 31.032 seconds, with 30.609 seconds compiling the runner and 0.417 seconds
validating/executing the plan; it reported one build-action cache hit. The warm
object/link hits belong to runner compilation. The artifact emit performed no
semantic, codegen, or link execution after action validation, so this result
credits the action cut without treating test-profile wall time as a compiler
release trend.

A 2026-08-02 copied production-path `configured_success` integration sample
exercises the external-command cache together with compiler emit, a real
generated runner, two executable artifacts, and two staged command outputs. It
used the same unoptimized test-profile compiler. The cold run took 119.939
seconds: 108.052 seconds compiled the runner and 11.876 seconds executed the
plan. The unchanged run reported two of two build-action cache hits, reduced
plan execution to 0.728 seconds, and restored both command outputs correctly.
Its 39.030-second total remained dominated by 38.294 seconds of runner
compilation. This sample proves the process-execution cut but is not release
performance evidence.

This measurement exposed a cache correctness boundary. Codegen partition
definition membership must be canonicalized by stable source definition
identity and stable mangled instance symbol before both fingerprinting and LLVM
emission. Session/discovery order is not persistent identity. A separately
persisted extension-trait-solving product was removed after it failed to cut the
end-to-end dependency chain and could perturb downstream definition identity;
the typed in-session query remains the semantic owner.

The 15-case CLI build suite is a correctness gate, not the performance
baseline. Every fixture is an independent libtest case with an isolated scoped
workspace and cache. Runner-only contracts reserve one compiler slot; cases
that execute a nested compiler action reserve the conservative build weight.
Libtest can therefore schedule independent fixtures while the shared resource
pool remains the authority for actual compiler concurrency. A fixture-index
test requires every configured directory to have a named libtest owner.

The 2026-08-02 test-infrastructure audit measured the same small
`dependency_cycle` runner at 86.1 seconds with the former unoptimized test
profile, 16.24 seconds with `opt-level = 1`, and 13.9 seconds with the release
compiler. All three performed the same 27,073 query executions, so this was a
profile mismatch rather than a compiler-query regression. The audit also found
a 192 GiB legacy `target/debug` containing hundreds of obsolete compiler
variants and unowned CLI scratch caches on the WSL `/tmp` tmpfs. Line-table
debug profiles limit future artifact size; scoped test-directory guards remove
scratch caches on both success and unwind. Performance conclusions continue to
use the isolated release workload and its deterministic counters.

With the repaired profile, fixture ownership, and resource classification, the
complete gate passed all 14 fixtures plus the fixture-index test in 159.31
seconds on the same 8 GiB WSL resource view. The former serial, unoptimized run
had taken 1291.21 seconds. This is test-infrastructure evidence, not a release
compiler performance threshold.

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
Scratch workspaces use owned guards and are removed when the test ends rather
than accumulating compiler caches in a memory-backed temporary filesystem. The
end-to-end configured build case is the owner of generated-runner codegen
coverage; a duplicate unaccounted unit-test compilation is not permitted.

Fault injection must prove that the injected operation is the operation under
test. In particular, allocator growth may release empty sentinel blocks; a
`free` failure injector intended to test rollback cleanup must ignore those
non-owned empty blocks. Tests assert both the structured operation/subject/cause
and the final active-allocation count so an earlier incidental call cannot
masquerade as cleanup coverage.
