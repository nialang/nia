# Nia Build And Standard Library Architecture Roadmap

Status: active design and execution roadmap

Baseline date: 2026-07-29

This roadmap begins after the compiler architecture roadmap closed. Before that
work, Nia had many compiler components but still behaved as an experimental
pipeline: semantic identity, query ownership, invalidation, scheduling, IR
ownership, codegen units, diagnostics, and persistent work products did not form
one coherent kernel. Those foundations now have explicit owners and acceptance
guards. Nia is therefore ready to build a real toolchain layer instead of adding
more features to a toy build runner.

The compiler is not "finished" and is not equivalent to rustc or Zig in
production breadth. The important change is that build and standard-library work
can now depend on a stable compiler kernel rather than compensating for missing
compiler ownership with scripts, global state, or duplicated caches.

This is a temporary project roadmap. Durable implementation architecture belongs
in `docs/architecture.md`; durable maintenance rules belong in
`docs/compiler-maintenance.md`. When every acceptance item here is closed, the
lasting rules and lessons must be migrated before this file is deleted.

## 1. Executive Decision

Nia will enter build and standard-library work as one coordinated project with
two distinct dependency tracks:

1. establish a relocatable, versioned toolchain/resource layout and the bounded
   build-host standard-library foundation needed to run `build.nia`;
2. replace the current in-runner recursive build executor with an immutable,
   versioned build plan validated and executed by the toolchain coordinator;
3. add incremental action caching, deterministic resource-aware scheduling, and
   a useful artifact surface only after that single plan/execution boundary is
   real;
4. broaden the standard library and supported targets from evidence gathered by
   real builds, without making "rewrite all of std" a prerequisite.

The project does not reopen the compiler roadmap. Compiler changes are allowed
only where build/std exposes a concrete missing boundary such as toolchain
layout, host/target configuration, a typed Driver action, diagnostics, or cache
identity. Such changes still follow `docs/compiler-maintenance.md` and must not
restore deleted compiler compatibility paths.

No percentage is assigned at roadmap creation. Design text, new type names, or
an unused protocol do not count as implementation progress. A phase changes
status only when its acceptance evidence is present.

## 2. Current Baseline

### 2.1 Compiler foundation available to this roadmap

The project can rely on the following implemented compiler contracts:

- session-wide canonical semantic identity and typed query storage;
- one revisioned fact/dependency graph with clean/incremental equivalence guards;
- explicit `QueryResult`, compiler/loader diagnostic stores, and an ICE boundary;
- a session-owned persistent executor with inherited CPU capacity and LLVM
  memory backpressure;
- item-owned checked/lowered bodies and explicit backend ownership transfer;
- deterministic codegen partitions, stable work-product identity, native object
  reuse, and complete link-result reuse;
- persistent frontend products and exact-input warm-check certificates;
- machine-readable performance workloads and a managed cross-main trend guard.

The build system must reuse these contracts. It must not create a second
compiler cache namespace, semantic dependency graph, thread pool, diagnostic
transport, or target identity.

### 2.2 Current build implementation

The current implementation is a functioning bootstrap:

- `nia build` finds `build.nia`, creates `.nia-build/` and `.nia-cache/`,
  generates a Nia runner, compiles it through `nia-driver`, and executes it;
- the generated runner constructs `std::build::Build` and invokes the package's
  `build` function;
- `std::build` can declare modules, executable targets, custom/check/emit steps,
  dependencies, and one explicit default step;
- module checks and executable emission invoke the same `nia` toolchain and use
  the compiler's artifact cache;
- a package-wide lock prevents concurrent mutation of the current build tree;
- twelve manifest build cases cover basic success, configuration, dependency
  order/cycles, missing/default/unknown steps, invalid targets and outputs, and
  executable dependencies.

This proves that Nia code can configure and drive its own package build. It does
not yet establish a long-term build graph:

- the runner both owns the graph and recursively executes it;
- compiler actions are reconstructed as raw CLI arguments;
- a custom step is an arbitrary function callback with no declared inputs or
  outputs;
- step, module, and artifact handles are plan-local numeric indexes without an
  owning plan identity;
- graph construction reads process arguments, environment, filesystem, and
  arbitrary standard-library services without recording configuration inputs;
- the graph has no stable plan schema, action fingerprint, generated-file
  identity, target/host separation, cancellation contract, or structured runtime
  diagnostic channel;
- fixed command/import buffers and one package-wide lock bound scale and
  serialize otherwise independent work.

### 2.3 Current standard library

The current standard library is not an empty prototype. It contains roughly
15,000 lines across 94 module files, including:

- compiler builtin declarations and primitive contracts;
- allocator interfaces, page/fixed-buffer/arena/general-purpose allocators;
- arrays, hash maps, strings, Unicode, hashing, formatting, and iterators;
- filesystem, path, I/O, process, environment, child-process, and terminal APIs;
- an OS facade with a Linux implementation;
- freestanding startup and process entry support;
- the current `std::build` API.

Rust integration tests exercise allocator failure/rollback, collection mutation,
filesystem and process behavior, atomics, I/O, startup, and executable runtime
paths. The library therefore needs dependency and contract refactoring, not a
blind rewrite based on file size.

The blocking limitations are structural:

- the compiler locates `lib/std.nia` from Rust's compile-time
  `CARGO_MANIFEST_DIR`, tying ordinary compiler use to a source checkout;
- freestanding runtime support is currently Linux x86_64 and no platform is Tier
  1;
- build scripts run on the host while emitted artifacts may target another
  platform, but that distinction is not represented by the build API;
- `std::build` depends on allocator, collections, path, filesystem, I/O,
  formatting, process, OS, string, slice, and Unicode layers without a written
  build-host dependency contract;
- standard-library tests are mostly Rust-driven compile/run integration tests;
  there is no explicit per-layer std conformance matrix or installed-toolchain
  test.

## 3. Scope And Non-Goals

### 3.1 In scope

- toolchain/resource-root discovery, explicit configuration, relocation, and
  compiler/std compatibility identity;
- a documented standard-library dependency hierarchy and the build-host subset
  required to evaluate build scripts;
- explicit host and artifact-target configuration;
- a versioned immutable build-plan model and deterministic codec;
- stable plan/action/artifact identities independent of allocation order and
  absolute checkout paths;
- typed compiler, external-command, generated-file, and aggregate build actions;
- validation, diagnostics, cancellation, failure cleanup, and selected-step
  closure;
- incremental action fingerprints, atomic publication, corruption retirement,
  and exact invalidation reasons;
- resource-aware scheduling that shares the project CPU/memory model;
- module, executable, object/library, generated-source, run/test, and install
  artifact boundaries required by ordinary Nia packages;
- migration of the current `std::build` surface and physical retirement of its
  recursive executor, raw-argv compiler bridge, fixed buffers, and coarse global
  lock;
- installed/source-tree toolchain tests, build/std performance workloads,
  architecture documentation, and user-facing build documentation.

### 3.2 Explicitly out of scope

- a public package registry, network dependency resolver, publication service,
  or trust/signing infrastructure;
- a complete package-manager design hidden inside the build graph;
- source compatibility for the current experimental `std::build` API;
- rewriting every standard-library module before build work begins;
- matching the full Rust or Zig standard-library surface;
- making every LLVM target a supported Nia runtime target;
- compiler self-hosting, remote execution, distributed builds, or a remote
  shared cache;
- PGO-driven codegen partitioning, full LTO design, or partial relinking;
- new language syntax or semantics unless a separately justified language
  design is required for a sound build/std contract.

Local package roots and explicit external module mappings are in scope because
build actions must describe their compiler inputs. Resolving package versions
from registries is not.

## 4. Non-Negotiable Invariants

### 4.1 One truth source per concern

- `build.nia` constructs configuration; it does not become a second compiler
  driver, cache owner, scheduler, or linker.
- The frozen `BuildPlan` is the only build graph consumed by execution. The
  mutable builder cannot remain reachable as a parallel execution model.
- The toolchain coordinator owns plan validation, selected-step closure,
  scheduling, action execution, output publication, and build diagnostics.
- Compiler object/link work products remain owned by the compiler/Driver cache.
  The build cache fingerprints compiler actions and references their declared
  artifacts; it does not copy their internal cache protocol.
- Observability, latest-run metadata, or timing summaries cannot become cache
  correctness inputs.

### 4.2 Build script host and artifact target are distinct

- `build.nia` is compiled and executed for the toolchain host.
- Compiler actions carry an explicit artifact target and runtime selection.
- Host paths, executables, environment, and process APIs are never inferred to
  describe the artifact target.
- Generated host tools and target artifacts have different typed identities even
  when produced from the same package.

### 4.3 Stable and local identity are separate

- Builder handles identify one live builder/plan and cannot be accepted by
  another builder merely because their numeric index is in range.
- Frozen nodes have deterministic stable keys derived from canonical package,
  action, artifact, and user-visible names plus schema domains.
- Persistent data never stores process-local pointers, function addresses,
  allocator order, session `ModuleId`, query slot, or builder index as stable
  identity.
- Duplicate stable identities are errors; insertion order cannot silently pick a
  winner.

### 4.4 Expected failures are values and diagnostics

- Missing inputs, invalid graph edges, cycles, command failures, incompatible
  toolchains, corrupt plans/caches, unavailable targets, and ordinary std I/O or
  allocation failures must not use panic/unwind.
- Panic remains limited to genuine toolchain invariants and flows to the existing
  ICE boundary.
- Runtime build diagnostics identify the package, step/action, input or output,
  and underlying cause. Exit status alone is not the diagnostic model.
- A failed action cannot publish a successful artifact or leave a cache entry
  that a later build can accept.

### 4.5 Inputs, outputs, and side effects are explicit

- Cacheable actions declare source/file/tree inputs, generated dependencies,
  relevant environment values, working directory policy, tool identity, target,
  options, and outputs.
- External commands use a typed action, not an arbitrary shell string.
- An opaque custom action is explicitly uncacheable and isolated. It cannot be
  mislabeled cacheable because it happens to return success once.
- Output paths are owned by exactly one action unless an explicit aggregate
  action defines the merge.
- Inputs under package, toolchain, build, and cache roots use canonical logical
  identities; absolute checkout paths are not hashed as project identity.

### 4.6 Determinism and resources are architectural

- Plan encoding, graph validation, ready-queue ordering, diagnostics, and output
  manifests are deterministic for equivalent inputs.
- Execution may finish out of order, but visible result order is stable.
- Compiler work shares inherited Cargo/GNU Make jobserver capacity and the
  existing process CPU budget. LLVM work continues to obey its memory permits.
- External actions declare a resource class or use a conservative default.
- No build provider or std helper creates an unmanaged private worker pool.
- Parallelism is not accepted until failure cancellation, output ownership, and
  memory bounds are defined.

### 4.7 Migration has a deletion boundary

- No permanent reader for the experimental plan shape is required.
- The current recursive `run_step`, raw compiler argv construction, `StepFn`
  execution node, fixed import/argv arrays, and package-wide executor lock are
  removed when their replacement becomes the only production path.
- Temporary adapters live at one advancing boundary and are deleted within the
  phase that introduces the replacement.
- Normal source-tree development may have an explicit resource-root option, but
  the final compiler cannot silently fall back to its Rust build-time checkout.

## 5. Target Architecture

The intended data flow is:

```text
nia build request
  -> ToolchainLayout + host/target configuration
  -> compile and run build.nia for the host
  -> mutable std::build::Builder
  -> validate and freeze versioned BuildPlan
  -> atomic plan handoff to nia-build coordinator
  -> selected-step closure + action fingerprints
  -> deterministic resource-aware scheduler
  -> typed compiler / process / filesystem executors
  -> atomic artifacts + build cache records + structured diagnostics
```

### 5.1 Toolchain layout

A typed toolchain layout owns at least:

- compiler executable identity;
- resource root and standard-library root;
- compiler/std/build-protocol schema identity;
- host target and selected artifact target;
- runtime/startup resources available for that target;
- default cache namespace and target/tool lookup policy.

Resolution order must be explicit and documented. An explicit CLI/typed API
configuration may select a development or installed resource root; the normal
installed layout is executable-relative or otherwise relocatable. Hidden
environment variables and compile-time source paths are not normal behavior.

Moving a complete toolchain directory must preserve `check`, `emit`, and
`build`. Moving only the compiler binary away from required resources must
produce a precise toolchain-layout diagnostic.

### 5.2 Standard-library layers

The roadmap will establish and enforce these conceptual layers without assuming
that each layer needs a separate package or directory:

1. compiler-known builtin declarations and pure core value contracts;
2. target/runtime primitives and the OS boundary;
3. allocation, strings, slices, collections, formatting, and iteration;
4. host services such as filesystem, I/O, process, environment, and child
   processes;
5. toolchain-coupled `std::build` plan construction.

Lower layers must not import build. Ordinary target programs should not load
build implementation modules unless they use `std::build`. Build can depend on
host services, but that dependency cannot leak host assumptions into target
artifacts.

The initial std work is limited to contracts exercised by build-plan evaluation
and toolchain relocation. Other std modules change only when a concrete defect,
layer violation, ABI issue, or missing acceptance test is found.

### 5.3 Plan construction and protocol

The build script executes configuration logic once per build invocation and uses
a mutable builder API. Freezing performs whole-plan validation and produces an
immutable `BuildPlan` with a versioned binary protocol.

The plan includes:

- canonical package and toolchain configuration identity;
- modules, artifacts, actions, dependencies, selected/default steps, host and
  target information;
- typed paths rooted in package, build, cache, toolchain, or generated-artifact
  namespaces;
- action inputs, outputs, options, environment dependencies, working-directory
  policy, and resource class;
- stable names/keys and source-facing descriptions needed by diagnostics;
- no function pointers, borrowed process memory, raw allocator handles, or
  session-local compiler identities.

The runner publishes the plan atomically to a coordinator-owned path or channel.
The codec repeats and validates schema and identity, rejects invalid tags,
lengths, duplicate keys, cycles, unknown references, truncated/trailing data,
and outputs escaping their allowed roots.

Arbitrary Nia code remains available while constructing the plan. Cacheable
execution nodes, however, are typed plan actions rather than callbacks into the
builder process. The experimental `add_step(name, StepFn)` model is retired; if
an opaque host action is retained, its untracked and uncacheable semantics are
visible in both API and diagnostics.

### 5.4 Toolchain coordinator

`nia-build` becomes the sole execution coordinator. It:

- resolves toolchain layout and compiles/runs the host build script;
- reads and validates the frozen plan;
- computes selected-step closure before execution;
- rejects graph/output conflicts before starting side effects;
- schedules ready actions under shared resource limits;
- executes compiler actions through typed Driver requests and external commands
  through a typed process request;
- captures structured action outcome, timing, stdout/stderr policy, diagnostics,
  and output manifests;
- cancels dependent work after failure, waits for active work to settle, and
  removes unpublished staged outputs;
- publishes only complete artifacts and cache records.

Whether a compiler action uses an in-process Driver session or an explicit
compiler subprocess is an executor policy, not part of plan semantics. Both
must consume the same typed action and produce the same outcome. Raw CLI argv is
not the graph's canonical representation.

### 5.5 Incremental build cache

Each cacheable action has a versioned fingerprint over all declared semantic
inputs. Depending on action kind, this includes:

- action schema and stable key;
- build-plan/toolchain/std/protocol identity;
- source or generated input content and logical path identity;
- dependency artifact fingerprints;
- host/target/runtime and compiler options;
- relevant environment values and external tool identity;
- working-directory and output policy where behavior depends on them.

Cache entries are immutable and content-addressed. Publication is staged,
synced as required, and atomically renamed. Reads validate identity, envelope,
payload length/checksum, and expected outputs. Corruption is retired and becomes
a miss. A miss reason distinguishes absent, changed input, toolchain, target,
environment, dependency, uncacheable action, corruption, and I/O failure without
creating a mutable latest-result truth source.

The initial cache is local. Its schema must not assume that paths or machine
state can be trusted merely because remote caching is out of scope.

## 6. Execution Phases

### Phase A: Baseline And Architecture Contract

Goal: turn the current audit into reproducible evidence and freeze the target
boundaries before migration.

Tasks:

- inventory current Rust `nia-build`, generated runner, `std::build`, Driver,
  cache, linker, std dependency, and test owners;
- record a build-host std dependency/public-surface matrix;
- define representative build workloads: runner bootstrap, no-op warm build,
  single-source edit, module-map edit, generated source, failed action, and
  multi-artifact package;
- add machine-readable build timing/action counters without making telemetry a
  correctness input;
- finalize the resource-root, plan protocol, host/target, action, artifact,
  diagnostic, and cache invariants in stable architecture documents;
- resolve the public project boundary: build remains toolchain-owned in this
  repository; package registry/manager work remains separate.

Acceptance:

- each current build success/error contract maps to a target action/diagnostic
  contract or is explicitly deleted as experimental behavior;
- the std modules required to compile and run `build.nia` are enumerated with no
  hidden facade/provider dependency;
- representative workloads run from clean isolated directories and record
  action/compiler/link executions and wall/RSS observations;
- the target protocol and toolchain layout have one owner and no unresolved
  competing execution model;
- no implementation phase is credited merely for this roadmap text.

### Phase B: Relocatable Toolchain And Std Identity

Goal: remove source-checkout coupling before the build protocol depends on it.

Tasks:

- introduce the typed toolchain/resource layout and thread it through CLI,
  loader, Driver, build, runtime injection, cache namespace, and tests;
- define explicit development and installed layout selection without hidden
  environment behavior;
- include compiler/std/resource schema identity in persistent frontend, object,
  link, build-runner, and future build-plan cache domains where relevant;
- diagnose missing resources, incompatible compiler/std, unsupported target
  runtime, and malformed layout through normal error channels;
- delete `default_std_module_path()` based on `CARGO_MANIFEST_DIR` and any
  duplicate std-root inference;
- document installation and source-tree invocation.

Acceptance:

- copying the complete toolchain layout to a different absolute path preserves
  representative `check`, `emit --obj`, `emit --exe`, and `build` behavior;
- the same binary with a missing or incompatible std/resource tree fails with a
  precise diagnostic before semantic analysis;
- source-tree tests pass an explicit layout and cannot succeed by accidentally
  finding the original checkout;
- cache entries created under one absolute installation path can be reused after
  relocation when their semantic toolchain identity is unchanged;
- no production path reads the Rust compile-time workspace path.

### Phase C: Build-Host Standard Library Foundation

Goal: stabilize only the std contracts required for build-plan construction and
handoff.

Tasks:

- enforce the builtin/core, runtime/OS, allocation/data, host-service, and build
  dependency layers;
- make host target, artifact target, path roots, process environment, and
  toolchain resources explicit at `std::build` boundaries;
- audit owned/borrowed path and string lifetimes retained by a frozen plan;
- audit allocator rollback and deinitialization for all plan-construction
  collections and codecs;
- keep ordinary fs/process/format/allocation failure as typed values with enough
  context for build diagnostics;
- add direct std conformance fixtures for the exact path, I/O, process,
  allocator, Unicode, collection, and formatting operations used by build;
- ensure ordinary programs do not load `std::build` implementation modules.

Acceptance:

- the build-host subset passes success, invalid UTF-8/path, unavailable file,
  process failure, allocation failure, and cleanup tests without panic or leaked
  output state;
- a build script can be compiled for the host while declaring an artifact for a
  distinct target without using target runtime APIs in the host process;
- plan-owned text/path values survive the builder call stack and allocator
  cleanup according to an explicit ownership contract;
- std public facade/provider tests prove unused build/OS/container modules are
  not loaded;
- no unrelated std-wide API redesign is used to inflate phase completion.

### Phase D: Immutable Build Plan And Versioned Handoff

Goal: separate graph construction from execution and establish one plan truth
source.

Tasks:

- introduce builder-owned typed handles with foreign-builder rejection;
- define stable package, step, action, artifact, and logical path keys;
- model compiler check/emit, external command, generated file, aggregate, and
  explicit uncacheable actions as typed plan variants;
- freeze the mutable builder into a deterministic immutable plan;
- implement the versioned plan codec, atomic handoff, decoder validation, and
  structured plan diagnostics;
- make default/selected step resolution a validated plan property;
- stop executing actions inside `std::build` and remove `StepFn` from cacheable
  graph semantics.

Acceptance:

- equivalent build scripts produce byte-identical canonical plans across
  allocation order and absolute checkout relocation;
- foreign handles, duplicate names/keys, missing references, dependency cycles,
  output collisions/escapes, invalid targets, and invalid action shapes are
  rejected before execution;
- truncated, trailing, corrupt, unknown-version, and semantically invalid plans
  are rejected and cannot cause side effects;
- plan round trips preserve all typed action inputs, outputs, targets,
  environment dependencies, resources, and diagnostic descriptions;
- the production runner publishes a plan but contains no recursive step
  executor or raw compiler argv assembly;
- the old `run_requested_step`/`run_step` and callback execution path are
  physically deleted when the coordinator path becomes live.

### Phase E: Coordinator Execution And Diagnostics

Goal: execute the validated plan through one deterministic toolchain owner.

Tasks:

- implement selected-step closure, ready-state transitions, cycle/conflict
  rejection, and deterministic result ordering;
- add typed compiler and external-command executors;
- reuse Driver module maps, host/target config, diagnostics, object cache, and
  link cache without reconstructing canonical graph data as raw argv;
- define stdout/stderr capture, passthrough, exit, timeout/cancellation, and
  process-tree cleanup policy;
- attach package/action/artifact context to compiler, process, filesystem, plan,
  and cache failures;
- replace the package-wide executor lock with scoped output/cache publication
  coordination;
- preserve successful independent work while preventing failed or canceled
  actions from publishing dependent artifacts.

Acceptance:

- current build cases pass through the coordinator with no legacy execution
  fallback;
- dependency closure executes each action at most once and produces the same
  visible order under one or multiple workers;
- compiler diagnostics retain source spans and action context; command failures
  retain executable, arguments policy, step, status, and captured output
  context;
- cancellation/failure waits for active actions, terminates owned child work,
  and leaves no accepted partial outputs;
- two non-conflicting builds can progress concurrently while conflicting output
  publication is serialized safely;
- normal failures never reach the ICE/panic path.

### Phase F: Incremental Build Cache And Resource Scheduling

Goal: make warm builds approach validation cost without sacrificing correctness
or bounded resources.

Tasks:

- implement typed action fingerprints and immutable local cache entries;
- track source/tree/generated inputs, dependency artifacts, target/options,
  relevant environment, external tools, cwd policy, and protocol/toolchain/std
  identity;
- restore outputs atomically and retire corrupt entries;
- publish precise hit/miss/invalidation reasons;
- schedule ready actions through inherited CPU capacity and declared resource
  classes while compiler/LLVM work keeps its existing budgets;
- define conservative behavior for unknown memory and untracked external state;
- add deterministic single-worker mode and concurrent stress/race tests.

Acceptance:

- a no-change second build executes only plan validation and unavoidable action
  validation; compiler/codegen/link/process execution counts approach zero for
  cacheable workloads;
- a source edit invalidates only affected compiler/action/artifact closure;
- target, toolchain/std, option, declared environment, external tool, and
  dependency artifact changes each produce the correct explicit miss reason;
- undeclared or opaque custom work is never reported as a cache hit;
- corruption, interrupted publication, duplicate publishers, and concurrent
  readers cannot expose a partial accepted artifact;
- fixed workloads demonstrate bounded CPU/RSS and useful concurrency without a
  global package execution lock.

### Phase G: Artifact And Package-Boundary Surface

Goal: make the graph useful for ordinary multi-artifact Nia projects without
smuggling in a package manager.

Tasks:

- finalize typed module, executable, object/library, generated source, run/test,
  install, and aggregate artifact relationships justified by real workloads;
- model host tools separately from target artifacts;
- make module maps/package roots first-class compiler action inputs;
- support generated inputs only through artifact dependencies, never by relying
  on execution order alone;
- define target/profile/optimization/runtime configuration inheritance and
  explicit override rules;
- define local/external package inputs without registry resolution or network
  policy;
- document the boundary between `nia build` and a future package manager.

Acceptance:

- a representative package builds multiple modules and artifacts, runs a host
  generation tool, consumes its declared output, checks/tests the target, and
  installs selected outputs through typed graph edges;
- changing one generated or source input invalidates only the correct closure;
- host and target artifacts cannot be substituted for each other by handle or
  cache-key collision;
- package-root/module-map changes participate in stable action identity;
- no raw ordering convention, shared output directory scan, or undeclared file
  discovery is required for correctness;
- package registry/version resolution remains absent rather than half-designed.

### Phase H: Hardening, Migration, And Roadmap Closure

Goal: remove the bootstrap architecture, prove installed-toolchain behavior, and
move lasting contracts into stable documentation.

Tasks:

- delete legacy runner positional protocol, recursive executor, raw compiler
  command construction, fixed import/argv capacities, coarse build exit-only
  reporting, duplicate caches, and obsolete locks/adapters;
- audit all ordinary build/std failure paths for explicit result/diagnostic flow
  and all remaining traps/panics for true unchecked or invariant semantics;
- fuzz/model-check plan codec and graph transitions where practical;
- run installed-layout, relocation, corruption, concurrent build, process
  lifecycle, allocator failure, std facade/provider, runtime, and representative
  package matrices;
- add managed CI appropriate to build/std correctness and a controlled trend
  workload without conflating it with compiler microbenchmarks;
- synchronize README, architecture, language/build surface, platform, install,
  contribution, and performance documentation;
- extract durable lessons into stable docs and delete this roadmap after every
  acceptance item closes.

Acceptance:

- structural searches find no production reference to deleted executor/protocol
  types or checkout-bound std discovery;
- source-tree and relocated installed-toolchain suites pass from clean and warm
  states;
- workspace all-target check, strict all-feature Clippy, formatting, focused
  std/build suites, and representative end-to-end builds pass;
- hosted build/std CI demonstrates a real clean build, warm reuse, targeted edit,
  failure/corruption recovery, and artifact execution;
- architecture and user docs describe the implemented single path rather than
  roadmap intent;
- build/std follow-on feature work has a separate bounded scope and this file no
  longer contains an open acceptance item.

## 7. Critical Path And Parallel Work

The mandatory dependency order is:

```text
Phase A
  -> Phase B toolchain/resource identity
  -> Phase C build-host std contracts
  -> Phase D immutable plan
  -> Phase E coordinator execution
  -> Phase F cache/resources
  -> Phase G artifact surface
  -> Phase H closure
```

Allowed parallel work is evidence-driven:

- Phase A workload/test inventory may proceed beside the stable architecture
  contract;
- Phase C std tests and dependency audits may proceed while Phase B threads the
  resource layout, but std build protocol code must wait for the layout identity;
- plan codec tests may be prepared while builder types are introduced, but no
  second execution path may ship;
- artifact API design may use fixtures during Phases D/E, but production
  expansion waits for one coordinator and cache identity;
- broader std modules may receive deep correctness fixes at any time when they
  are independently justified, but they do not count toward build-roadmap
  progress unless they close a stated acceptance item.

Do not start Phase F by hashing current raw CLI invocations. Do not start Phase G
by adding more callback step kinds. Those shortcuts would freeze the bootstrap
architecture and make later migration harder.

## 8. Validation Matrix

Every phase selects the narrowest relevant checks and expands to broader gates
before its batch commit. The final maintained matrix includes:

| Workload | Required evidence |
|---|---|
| Toolchain relocation | copied layout runs check/object/executable/build without original checkout |
| Missing/mismatched resources | stable diagnostic before semantic execution |
| Build script compile failure | source diagnostic with build-script/toolchain context |
| Build script runtime failure | structured package/plan diagnostic, no partial plan accepted |
| Deterministic plan | identical canonical bytes across allocation/order/path relocation |
| Invalid/corrupt plan | no action side effect; precise validation error |
| No-op warm build | action/compiler/codegen/link/process counts near validation-only |
| Source/module-map edit | exact dependent action/artifact invalidation |
| Generated source | producer output identity flows to consumer input fingerprint |
| External command | declared args/env/cwd/tool/inputs/outputs determine reuse |
| Action failure/cancel | dependent work stops; active work settles; staged outputs retired |
| Concurrent builds | independent work overlaps; conflicting publication remains safe |
| Allocator/path/process faults | build-host std returns typed errors and cleans ownership |
| Host/target split | host build tool cannot be mistaken for target artifact |
| Multi-artifact package | deterministic graph, outputs, install/run/test behavior |

Performance claims require repeated complete builds on compatible resources.
Isolated plan decode, cache lookup, or compiler cache-hit measurements cannot
substitute for end-to-end action execution counts and wall/RSS observations.

## 9. Delivery Discipline

- Work proceeds in meaningful dependency-complete batches, not one-symbol or
  one-fixture commits. A normal batch should close several related tasks and at
  least one observable contract.
- After several coherent implementation waves pass their relevant gates, commit
  with a descriptive `feat: ...` subject. Test-only or corrective commits may use
  the matching conventional prefix when genuinely separate.
- Each committed batch records a concise roadmap progress entry containing the
  changed ownership/data flow, the old path physically deleted, and actual
  validation evidence. Do not append a diary of every edit.
- Rejected experiments are recorded only when they establish a reusable
  architectural lesson. Their code, schemas, compatibility readers, flags, and
  counters are removed rather than left dormant.
- A failing broad gate caused by unrelated pre-existing behavior is reported and
  isolated; it is not silently fixed inside the current batch.
- No phase completion is inferred from line count, type count, cache-hit count,
  or the existence of a new API. Acceptance requires the production consumer and
  retirement boundary.

## 10. Risks And Forbidden Shortcuts

### 10.1 Bootstrap and resource cycle

Changing std can break the build runner that is needed to exercise build. Keep a
known-good compiler/toolchain path for fixture execution until the new relocated
layout is accepted. This is a controlled stage boundary, not a permanent old-std
fallback in production.

### 10.2 Configuration cannot be made reproducible by naming it so

A build script is arbitrary host code. Re-running it each invocation is
acceptable; claiming its opaque filesystem/environment reads are cached is not.
Only declared plan actions and declared configuration inputs may participate in
reuse. If future tracked build-script evaluation is desired, it requires a
separate capability design.

### 10.3 Std breadth can consume the project indefinitely

Do not block build-plan work on rewriting math, every collection, all targets, or
all I/O APIs. Stabilize the build-host dependency slice, preserve correct tested
modules, and expand from real package requirements.

### 10.4 A serialized callback graph is still a callback graph

Do not encode function addresses, enum tags that dispatch back into arbitrary
runner state, or raw command text and call it a build plan. Cacheable action
semantics must be independently validatable by the coordinator.

### 10.5 Global locking hides missing ownership

One package-wide lock is acceptable for the bootstrap but not the target. Output
and cache publication need explicit keys and scoped coordination. Removing the
lock before those owners exist would merely introduce races.

### 10.6 Package management scope creep

Local package/module inputs are necessary for build. Registry resolution,
version solving, lockfiles, downloads, trust, and publication are a separate
product. Do not let placeholder package-manager types become permanent build API
without that design.

### 10.7 Compiler stability must not be spent casually

Build/std may reveal a real missing compiler interface. Fix it at its owner and
validate it under the compiler maintenance contract. Do not bypass the query
graph, expose mutable compiler global state to build scripts, or reopen removed
identity/cache APIs for convenience.

## 11. Initial Status

The project is ready to execute Phase A and the first Phase B batches. No
implementation phase is marked complete by creating this roadmap.

The current compiler core is stable enough to support this work, but build/std
product maturity remains early. The first proof of progress is not a new build
feature. It is a compiler and std resource layout that works after relocation,
has explicit compatibility identity, and removes the compile-time checkout
dependency. The second proof is a deterministic frozen plan that the runner no
longer executes itself. Only after those two boundaries should cache,
parallelism, and artifact breadth accelerate.

This sequencing turns Nia's current experimental build bootstrap into a real
toolchain without discarding the valuable fact that build scripts are ordinary
Nia programs and can use a carefully layered standard library.
