# nia-build

`nia-build` owns the Rust side of `nia build`. It resolves one package
invocation, compiles and runs the package's `build.nia`, validates the emitted
plan, and executes the selected dependency closure. Package registries, version
solving, downloads, publication, and network trust are outside this crate.

This README is the single owner of build-system internals. Writing build scripts
is covered for users in [Custom Builds](../../docs/guides/build-scripts.md); the
public `std::build` API and its ownership rules live with the Nia sources in
[`lib/std/build.nia`](../../lib/std/build.nia) and
[`lib/std/build/`](../../lib/std/build/). Language-visible build behavior is
specified in the [language reference](../../docs/reference/language.md); the
outer CLI and compiler boundary are described in
[Compiler architecture](../../docs/dev/architecture.md).

## Invocation

```text
nia build
  -> resolve BuildRequest into BuildInvocation and ToolchainLayout
  -> resolve package manifests and the selected package graph
  -> compile and run build.nia for the host
  -> std::build::Build validates and encodes a draft
  -> BuildPlan::decode validates and freezes the graph
  -> coordinator executes the selected closure
  -> typed Driver, linker, process, and filesystem operations
```

`BuildInvocation` is invocation-local: it owns the physical package, build,
cache, runner, and handoff paths. The generated runner is a configuration
process. It can inspect its injected package, toolchain, host, artifact-target,
optimization, and requested-step values and construct a graph through
`std::build`; it cannot execute graph actions. The runner emits a draft protocol
payload, and decoding always performs the same semantic freeze and validation
as an in-process plan. The decoded immutable `BuildPlan` is the coordinator's
only execution input.

The runner wrapper, [`src/runner.nia`](src/runner.nia), is owned by
`nia-build`. It decodes the private runner configuration, binds the
runner-config and plan protocol identities, and drives the invocation
lifecycle. It is ordinary Nia code compiled against the standard library, so
every `std::build` item it calls is `pub`: `Build::init` with
`BuildInitAttempt::finish`, `Build::deinit`, `attachEnvironment`,
`declareManifestExecutable` and `declareManifestTest` for the manifest's
`[[executable]]` and `[[test]]` entries before the script's `build` runs,
`reportError`, and `writePlanDraft`, which calls `validatePlan` before
encoding. A build script receives the same `Build` value and can call these
items too. Only the wrapper source and the configuration format it decodes are
private to this crate.

The runner is always compiled for the host at O0. The requested optimization is
an explicit configuration value used by artifact actions, not the compilation
policy for the short-lived plan constructor.

## Source Ownership

- [`src/lib.rs`](src/lib.rs) owns package discovery, invocation paths, runner
  generation and execution, plan handoff, and inspection reports.
- [`src/runner.nia`](src/runner.nia) is the private generated-runner wrapper,
  and [`src/runner_config.rs`](src/runner_config.rs) owns the private bounded
  configuration passed to it. [`src/runner_cache.rs`](src/runner_cache.rs)
  owns the content-addressed cache for compiled host runners.
- [`src/configure_cache.rs`](src/configure_cache.rs) owns reusable configure
  records, and [`src/configure_observation.rs`](src/configure_observation.rs)
  and [`src/runner_observation.rs`](src/runner_observation.rs) own the
  observations recorded for them.
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
  - [`src/observations.rs`](../nia-build-plan/src/observations.rs) owns the
    build-draft envelope that carries a plan and its configuration evidence.
- Package manifests, source identities, and dependency classes are owned by
  [`nia-package-metadata`](../nia-package-metadata/).
- [`src/coordinator.rs`](src/coordinator.rs) owns selected-closure execution
  and its report, with child modules under
  [`src/coordinator/`](src/coordinator/):
  - [`scheduling.rs`](src/coordinator/scheduling.rs) builds the canonical
    selected-step closure and schedules it by resource class.
  - [`executor.rs`](src/coordinator/executor.rs) executes typed actions and
    their cache lookups under the complete output-lock set.
  - [`external_command.rs`](src/coordinator/external_command.rs) owns
    external-command identity, the declared-input view, execution, and
    bounded diagnostics.
  - [`publication.rs`](src/coordinator/publication.rs) owns journaled
    publication of complete action output sets.
  - [`helpers.rs`](src/coordinator/helpers.rs) owns frozen-plan lookup,
    target conversion, and typed consistency errors.
- [`src/program_search.rs`](src/program_search.rs) resolves programs launched
  by name for both external-command steps and configuration probes, so a
  step, its cache identity, and a probe agree on the file.
- [`src/progress.rs`](src/progress.rs) owns the invocation-local progress
  event stream described under [Progress Observation](#progress-observation).
- [`src/action_cache.rs`](src/action_cache.rs) and its child modules own
  build-action records and invalidation reasons. Compiler object, archive, and
  link products are owned by `nia-driver` and `nia-linker` caches.
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

## Package Metadata

Package manifests are resolved before runner execution. The package resolver
fingerprints declared source content and records every edge with its
`DependencyScope`: `Build`, `Runtime`, `Development`, `Optional`, or `Lazy`.
`PackageGraph::report` exposes declared versus resolved edges without loading
source modules.

Dependencies are fetched by Nia itself: Git sources through the Nia-owned
`nia-git` transport (local repositories, the Git TCP protocol, HTTP(S), and
SSH), and registry packages over HTTPS. The user-facing source,
credential, proxy, and lockfile contract is in
[Dependency sources](../../docs/guides/packages.md#where-a-dependency-comes-from) and
[Git Sources](../../docs/reference/manifest.md#git-sources); the
registry and lockfile formats are specified in
[`nia-package-metadata`](../nia-package-metadata/README.md).
Every selected source is recorded in `nia.lock`, and fetched trees are kept
below the ignored `build/packages` directory and checked against their recorded
content hash before reuse.

Resolution records each dependency's logical graph root and checkout, including
checkouts whose edge was filtered from the selected graph. Build plans encode
manifest dependencies by declared alias and local source trees by relative
path, so the host resolves each package instance without guessing from a path.

The user-facing meaning of each scope table is in
[Manifest Reference](../../docs/reference/manifest.md). A
compiler request receives dependency roots through two filters, applied with
the compilation mode of the artifact or check being compiled; a test suite's
executable is compiled in test mode, every other artifact in normal mode:

- `manifest_module_roots` preinstalls the `[library]` roots of declared
  dependencies in the module map. The host runner receives `Build` roots.
  Artifact compilations request `Runtime` and `Development` roots: `Lazy`
  roots are selected whenever `Runtime` is, `Development` roots only in test
  mode, and `Optional` roots never.
- The coordinator then keeps an explicit plan module import whose path names a
  package only when one of that package's recorded dependency classes admits
  it: `Runtime`, `Optional`, and `Lazy` always, `Development` in test mode,
  and `Build` never. This is how an `Optional` dependency reaches an artifact.
  Imports of generated build roots, and of packages with no recorded class,
  are always kept.

Source identity and manifest bounds are owned by
[`nia-package-metadata`](../nia-package-metadata/README.md#source-identity).

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
Module imports may form cycles; name resolution across them belongs to the
compiler's using graph.

Profile and default optimization are invocation settings, not plan fields: the
runner receives the default optimization, each module carries its effective
level, and the coordinator passes the profile and that level to every
compiler request. Both enter the cache identity of the compiler actions they
affect.

Output-changing artifact settings are plan fields. Each artifact carries its
runtime and an `ArtifactConfiguration` record of linkage, debug info, panic
strategy, LTO, defines, and system libraries; each compiler-check action
carries a copy of the record of the artifact it checks. Only the executable
link consumes the record: linkage selects the executable form on ELF targets
and is ignored elsewhere, system libraries are added to the link options, and
LTO selects the link request's link-time optimization policy. Object-set and
static-archive emits read none of it. Debug info, panic strategy, and defines
appear only in the plan, in cache identity, and in inspection output. The
compiler-check and compiler-emit identities encode the complete record, and a
change is reported as the `artifact` invalidation component; the test-suite
completion identity encodes the artifact's LTO. Runtime is a separate artifact
field with its own `runtime` invalidation component.

The invocation `--lto` setting is not an artifact setting. It selects
link-time optimization for the host runner link only, and it is part of the
runner cache key and the configure invocation identity. The `lto` value that
`--print-plan` and `--explain` show under the invocation configuration is this
runner setting.

Targets cross the runner and plan protocols as canonical target names such as
`aarch64-apple-macos`. Decoding rejects a name outside the maintained target
matrix; every other target fact is derived from the name, never transported
beside it. `TargetView` exposes the name, its architecture, vendor, OS, and
environment components, and the toolchain-derived byte order and pointer width.

The runner receives the toolchain's complete maintained target matrix. An
artifact without a target builds for the invocation artifact target;
`forTarget` selects any other member, such as `b.hostTarget()` for a tool the
build runs or `b.target("x86_64-apple-macos")` for an additional artifact.
Each compiler action carries its resolved target, and the coordinator creates
one driver per distinct target in the plan. Run, test, and command programs must
be built for the host target.

### Encoding bounds

The Nia encoder bounds every collection count by the registered `maxItems`
limit before narrowing it to the wire `u32` representation. Aggregate counts
and derived dependency/input/output counts use checked addition, so malformed
in-memory state is rejected instead of wrapping into a smaller protocol value.
Generated-file payload lengths are checked before their fixed-width length
prefix is written.

The Rust encoder applies the 64 MiB plan budget at each raw write. Once a write
would exceed that budget it retains the first `TooLarge` error and stops growing
the output buffer, rather than allocating the complete oversized encoding and
rejecting it only at `finish`.

The decoder does not reserve typed list capacity from a runner-controlled count.
It grows a list only after each item has consumed and validated its bytes, so a
truncated count prefix cannot amplify a tiny draft into `count * size_of(T)` of
host allocation.

### Protocol identity

The registered protocol identities in `nia-compat` are the only schema and
namespace authorities. One protocol epoch is shared by compiler, toolchain,
ABI, metadata, runner, and cache products, and it is derived from the compiler
version's release line: `0.x` versions use epoch `x`, so `0.3.0-dev` and every
`0.3.x` release use epoch `3`. Internal format changes within a line do not
change the epoch. Decoders require an exact match; earlier protocols have no
readers, migrations, or aliases. Payload magic identifies the payload kind; it
is not an independently bumped version. Owner-local fingerprint domains are
dependency-invalidation details, never additional user-facing version tracks.
Changing an encoded field or persistent cache input updates its owning
contract without promising to read or migrate data from an earlier release.

## Execution

The frozen plan's host and artifact targets must match the current invocation
before action selection or output preparation. Mismatch reports identify the
role and show the invocation target as expected and the plan target as found.

The coordinator executes deterministic readiness waves through `QuerySession`,
so build actions share the process CPU and compiler resource budgets.
`Cpu` and `Io` actions reserve one action slot; `Conservative` actions reserve
the complete action capacity. `--jobs` can reduce ready-action parallelism but
does not create a separate executor or replace LLVM memory backpressure.

Runner and external-command output is drained concurrently into bounded tails.
Every started capture worker is joined, including when its sibling fails to
start or panics. Process-wait and stream read/forward failures retain all tails
returned by completed workers. Reports identify the failed operation and stream;
test-suite and staging-cleanup reports preserve the original action's context.
Text commands may forward live child output; JSON commands retain it in notes
without mixing child bytes into the diagnostic envelope.

## Progress Observation

Build progress is an invocation-local observation stream. It is created by the
CLI and passed through the request; library callers remain silent unless they
provide an observer. Progress never enters the frozen plan, cache identities,
resource scheduling, output publication, or diagnostic selection.

The user-facing levels, modes, text formats, and JSON schema are documented in
[Progress output](../../docs/reference/cli.md#progress-output); this
section owns the observation contract the CLI renderers rely on.

The selected dependency closure is the accounting boundary. Shared actions are
counted once, while every selected step remains represented in the final build
report. A terminal action event is emitted exactly once and distinguishes
cached, executed, no-work, failed, and cancelled outcomes. Compiler checking,
code generation, and linking are nested activities of one action. Wait events
are emitted only by owners that observed real contention: dependency admission,
output locks, and action-cache mutation locks. Runner/configure cache waits
use their bootstrap phase context and do not count as selected actions.
Queue delay and resource permit wait are reported separately after admission,
and resource snapshots carry an
owner generation so stale notifications cannot overwrite newer state.
Cache lookup decisions are observed once at the actual lookup boundary. Failed
or cancelled actions retain that decision in their terminal record; successful
actions carry their final cache outcome, including publication fallback. A
lookup hit followed by a failed output restore remains a failed action and does
not increase the cached-success count.

The progress worker retains O(selected actions + selected steps) pending data
and never invokes a renderer while a build ownership lock is held. Resource
snapshots and repeated complete
wait/activity spans are coalesced; lifecycle and terminal events retain their
sequence order. Renderer failures shut down progress output without changing
the build result or child-output capture policy. Progress is written to stderr;
in text modes the renderer also owns forwarded child output, so a redrawn status
line is cleared before child bytes are written.

The JSON form keeps observation semantics visible to consumers. Elapsed time is
measured at the producer, before observer delivery. `sequence` is observation
order and may have gaps from verbosity projection and coalescing, while
`ordinal` is canonical selected order. Diagnostics and timing records keep their
own schemas and can share stderr with progress; build progress is flushed
before those final records.

## State Layout

`nia_package_metadata::BuildLayout` is the only owner of state paths; the
user-facing layout and `nia clean` contract are in
[The Build Directory](../../docs/reference/cli.md#the-build-directory). The state
root is the workspace root for a workspace member and the package root
otherwise. `BuildLayout::out_dir` is `build/out/<member path>/` for a member;
the cache, tmp, packages, and registry directories are shared by every package
under the state root.

`build/tmp/` holds the runner executable, runner configuration, and plan draft
of invocations in progress, named `<pid>-<sequence>` from the process id and a
per-process counter. `build/out/build-plan.bin` (below the member's out
directory) is the published plan handoff. `build/packages.lock` is the
package-state lock that dependency resolution, `nia update`, and `nia clean`
hold while they change state; a build holds it at most while resolving
dependencies, not while it runs the runner or executes actions.

`nia clean` (`run_clean`) discovers a package like `nia build`, holds the
package-state lock, and removes `BuildLayout::out_dir`, `cache_dir`, and
`tmp_dir`. Run from a workspace member, it therefore removes only that
member's `build/out/<member path>/` but the `build/cache/` and `build/tmp/`
shared by the whole workspace. It keeps `build/packages/`, `build/registry/`,
and `build/packages.lock`. It does nothing when `build/` is absent, and it
refuses to run when no `nia.toml` or `build.nia` is found or when `build/` is a
link, a reparse point, or not a directory; a link in place of one of the three
removed directories is removed as a link.

In tests, `NIA_TEST_SHARED_ARTIFACT_CACHE` (set only by `nia-test-support`)
names an absolute root that replaces the invocation's artifact cache
directory, so the runner and compiler-product caches are shared across test
packages; configure, action, and suite caches stay under the package's
`build/cache/`.

## Caches

| Cache | Stores | Owner |
| --- | --- | --- |
| Runner cache | Checked host runner executable | `src/runner_cache.rs` |
| Configure cache | Frozen plan plus its bounded observation manifest | `src/configure_cache.rs`, `src/configure_observation.rs` |
| Action cache | Generated files, eligible external-command results, references to compiler products | `src/action_cache.rs` |
| Test completion cache | Witness of a successful test suite, no output payload | `src/action_cache/test_suite.rs` |
| Compiler and linker product caches | Objects, archives, link results | `nia-driver`, `nia-linker` |

Cache I/O failures and corruption produce explicit misses and never replace the
ordinary correctness path.

### Runner cache

Generated runners use content-addressed
`build/cache/runner/protocol/<key>.cache` records. The namespace and payload
identity are registered in `nia-compat`; there is no runner-local schema
version. The key includes the generated runner source, the build-script bytes
(or an `absent` marker for a manifest-only package), the build-dependency
closure, the standard-library source tree, the toolchain identity, the host
target, the invocation profile and `--lto` policy, and the build protocol. The
build-dependency closure starts at the root package's `Build` edges and
follows every non-development edge transitively; each package in it
contributes its logical root, canonical identity, `[library]` path, and source
fingerprint. Runtime and development dependencies and the root's own sources
are outside the closure, so editing them does not rebuild the runner. The
runner is always a normal-mode compilation. Artifact optimization is
transported through the private runner configuration but is excluded from the
key, because the runner is always compiled at O0. A hit verifies and atomically
restores the runner executable; malformed or corrupted records are retired as
misses.

### Configuration cache

The CLI freezes a new plan for each invocation; only a configure miss starts the
runner. A configure record is keyed by three identities: the runner cache key,
the invocation identity, and the observation-manifest fingerprint. The
invocation identity (`BuildInvocationIdentity`) encodes the package-graph
fingerprint, the host and artifact targets, the profile, the default
optimization, the `--lto` policy, and the step selection (default, `--list`,
or a named step). A record may be reused only when the runner and invocation
identities match, every observation recomputes against the current package
root, and its plan decodes through semantic freeze. A missing, changed,
poisoned, or corrupt record falls back to normal runner configuration.
Candidate scans are bounded; an oversized configure namespace is a miss. The
durable `build-plan.bin` handoff is an atomically published observation of the
last completed configuration, never an unchecked execution input.

A record whose manifest has the `Poison` policy or no observations at all is
never reused and reports a `poisoned` miss. When the runner reports no
observations, the host records the contents of `build.nia` as the only
observation, so a script without probes is still reusable. A manifest-only
package records that `build.nia` is absent instead, so its record is reused
until a script appears. A `build.nia` over the 1 MiB file-content observation
limit cannot be observed, so its record is empty and every invocation runs the
runner again.

The runner transports bounded file-content, file-presence, directory,
environment, command, Git, and tool evidence in the draft envelope, and the
host rechecks each reported value before publishing a record:

- Filesystem observations distinguish content, metadata, directory entries,
  and presence. A missing optional path is a stable `absent` fact, so creating,
  removing, or changing its entry kind invalidates the record. Existing
  ancestors must resolve inside the package root, including when several
  trailing components are absent. Unresolvable symlinks and unreadable paths
  are errors, not recorded absence. Input files and directory trees reject
  Windows reparse points during both identity passes.
- `Build::probeFile` requires caller-owned bounded storage; the host re-reads
  the reported bytes under the package-root boundary before canonicalizing them
  into the BLAKE3 manifest. `Build::probeFilePresence` records absent, file,
  directory, and other-entry states.
- Environment observations distinguish an absent variable, an empty value, and
  each UTF-8 value; non-UTF-8 values are rejected rather than silently changing
  the identity.
- Command, Git, and tool probes use separate executable identity records. The
  host resolves the program through `PATH`, runs its canonical path, and
  fingerprints its contents, request, exit status, and bounded output. They are
  not package-relative filesystem facts.

Malformed records, path escapes, oversized values, duplicate facts, and
read-after-run changes are rejected and cannot enter the configure cache.

### Action cache

Generated-file and eligible external-command entries store complete validated
payloads. Compiler-check entries record only a prior successful zero-diagnostic
check against a complete source manifest. Compiler-emit entries bind build
identity to a Driver-owned executable cache reference rather than duplicating
the executable.

External-command results are cached only under an explicit
`CommandCachePolicy`; the builder methods and their user-facing requirements
are in [Running External Commands](../../docs/guides/build-scripts.md#running-external-commands).
Every cacheable identity binds the typed command declaration (program,
arguments, stdin, working directory, environment and cache policies, and
resource class), the content of the resolved program, the content of every
declared input and of every artifact input, the explicit environment, the
declared outputs, package roots, and the compiler version and protocol epoch.
`FullDeclaredEnvironment` adds a sorted snapshot of the inherited process
environment bounded at 1 MiB; an oversized snapshot executes normally and
reports an uncacheable miss. `SuccessfulTests` applies only to test suites and
publishes a completion witness, described below.

Declared directory inputs use a deterministic streamed identity bounded at
64 MiB of encoded size, depth 256, and 1,000,000 entries; oversized or
excessively nested trees fail as typed input errors.

Successful test suites use a separate completion cache keyed by the test
executable, source input manifest, declared inputs, effective environment,
toolchain, and test action contract. Failed or cancelled suites never publish a
record, and cache failures fall back to running the suite.

A cacheable external command runs in a temporary working-directory view
created under the system temporary directory and named
`nia-command-inputs-<pid>-<action>-<sequence>`. The view is populated by
copying only declared inputs; the copy rejects links and reparse points and
is bounded by depth 256, 1,000,000 entries, and 64 MiB of copied bytes. Only
typed input arguments and declared stdin are remapped into the view; literal
arguments keep their exact text, and declared outputs keep their staged build
paths. Before the command runs, the coordinator records a BLAKE3 content
snapshot of the view; after a successful exit it compares a second snapshot
and fails the action when any file other than a declared output was created,
changed, or deleted. Standard input is always either closed or the declared
file; the coordinator never passes its own stdin, and a declared stdin path is
part of the input set, command identity, and dependency closure. The view is
not an OS-level sandbox, as the guide states.

## Publication and Locks

Actions publish only declared build-root outputs. Equal logical destinations
share a cross-process lock; unrelated destinations proceed independently.
Before staging, every output parent is created and checked component by
component beneath the invocation build root; symlinks and Windows reparse
points are rejected so a logical output cannot redirect publication elsewhere.

File and directory outputs use staged same-filesystem transactions. The
checksummed journal and prepared marker make interrupted publication
recoverable without guessing which partial destination is valid. Corrupt or
contradictory recovery state is a typed failure. Recovery rejects a journal
namespace that exceeds its bounded entry budget and validates directory outputs
with a streaming traversal, so malformed state cannot force an unbounded
per-directory allocation.

A lock is stale only when no live owner holds it: Unix reclaimers take the
owner's `flock`, and Windows reclaimers need an exclusive open that deletes the
same file object on close. Neither platform reclaims a live lock by age. The
bounded owner record is `pid:start-time:sequence`; readers require the complete
record before checking liveness, and malformed records, including PID-zero
records, are reclaimed as stale.

Action, configure, and runner cache records, recovery journals, and lock-owner
records are opened as regular files without following symlinks or Windows
reparse points. Cache namespaces are created and traversed component by
component under the invocation cache root; symlinked or reparse parent
directories are rejected on both publication and lookup. A redirected or
malformed record follows the ordinary miss or recovery-error path.

Every mutable cache operation holds the entry's coordination lock: publication
holds it while installing a complete record, and lookup and namespace scans
hold it while validating or retiring corruption. A reader that observed
malformed bytes therefore cannot delete a valid replacement published by
another invocation after its scan. Test-suite completion records use the same
lock while the coordinator rechecks identity, executes the suite, and publishes
its witness. The runner and configure caches use invocation-local lock
namespaces; compiler and linker product caches keep their owner-specific locks.

## Inspection

`nia build --dry-run` validates the selected closure without executing
coordinator actions. `--print-plan`, `--explain`, `--print-cache`,
`--print-observations`, and `--list` expose deterministic graph and cache
state. `--diagnostics-format=json` switches these products to newline-delimited
deterministic JSON records, each identified by its `kind` field:

| Record | Emitted by | Contents |
| --- | --- | --- |
| `build-plan` | `--print-plan` | Canonical package, module, artifact, action, and step nodes with targets, selection, and protocol epoch, plus the invocation configuration and configure and runner cache status. |
| `build-explanation` | `--explain` | The plan, package-graph fingerprint and decisions, configure cache candidate, configuration observations, declared tools, invocation configuration, and selected closure. |
| `configure-observations` | `--print-observations` | The observation policy and each observation's path, kind, and fingerprint. |
| `configure-cache` | `--print-cache`, `--explain` | The configure candidate status, whether the cached plan was reused or the runner executed, and the runner executable cache status. |
| `steps` | `--list` | Every step's name and kind and whether it is the default. |
| `action-cache` | `--print-cache`, `--explain` | A decision object with status, miss reason, component invalidations, declared outputs, and publication state. |

`--explain` reports each declared dependency's selected-graph resolution, the
number of its modules represented in the frozen plan, and whether the selected
action closure owns or references that package. Module counts describe plan
declarations; closure use follows action and step package ownership plus typed
module, artifact, and package-path references. JSON dependency entries expose
these as `resolved`, `loaded_modules`, and `used`. `resolved_dependency_count`
counts selected dependency edges, while `resolved_count` counts selected
package nodes including the root. Canonical checkout paths used to join
metadata edges to plan roots are invocation-local and are not serialized.
Dependency scopes in text and JSON use the stable names `build`, `runtime`,
`development`, `optional`, and `lazy`.

Text action-cache entries use stable status and reason names and include
publication state and declared outputs. Component invalidations are listed on
the following indented line. The text `--print-plan` artifact line prints the
artifact configuration as compact JSON with keys in alphabetical order, such as
`configuration={"debug_info":"line-tables","defines":[],...}`; the JSON
`build-plan` record carries the same object.

Dry runs consult action caches to predict hits and miss reasons. They do not run
compiler, linker, or external-command actions, restore outputs, or publish cache
records. An action is reported as unavailable when its identity depends on an
input product that is not present yet. Explicit command environment values are
represented by variable name, byte length, and a domain-separated fingerprint,
never the value itself; inherited variables are represented as `inherited`.

## Evolution Rule

New configure observations, package identities, and dependency policies are
represented as typed inputs registered at the appropriate boundary. They must
not make the durable plan an implicit source of execution truth, conflate
package metadata with ABI or type metadata, or bypass freeze-time validation.

## Verification

Run focused owner tests before broad workspace gates; the harness rules are in
[Testing](../../docs/dev/testing.md). The principal end-to-end contracts live in
[`nia-cli/tests/build_cases.rs`](../nia-cli/tests/build_cases.rs) and the
`std_build_*` suites. The representative clean/warm/edit/corruption workload
lives under [`benchmarks/build/`](../../benchmarks/build/) and runs through
`cargo maintain baseline build`, documented in
[`maintain/performance.md`](../../maintain/performance.md). Its runner-only
package measures the fixed cost of configuring through `build_configure_plan`
and `build_run_runner` timings, alongside exact configure hits and runner
executions per transition.
