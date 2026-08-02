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

The current public surface is also not a compatibility baseline. Much of std,
especially the build-host slice, was designed to make the first programs run
rather than after a deliberate API review. Phase A must classify existing APIs
by layering, ownership/lifetime, error model, naming/surface, and bootstrap-only
status. A working API is evidence about required capability, not evidence that
its present signature or module placement should survive.

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
- source compatibility for other experimental std APIs merely because current
  examples or the bootstrap runner use them;
- rewriting every standard-library module before build work begins;
- matching the full Rust or Zig standard-library surface;
- making every LLVM target a supported Nia runtime target;
- compiler self-hosting, remote execution, distributed builds, or a remote
  shared cache;
- PGO-driven codegen partitioning, full LTO design, or partial relinking;
- new language syntax or semantics unless a separately justified language
  design is required for a sound build/std contract.

Language/compiler proposals that may change build or std boundaries are not
silently absorbed and are not ignored. Phase A records their impact on
ownership, errors, host/target semantics, compile-time/runtime capability, and
the plan protocol. A decided proposal may become an explicit dependency; an
undecided proposal remains a named decision gate and cannot be implemented by
accident through std API stabilization.

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

### 4.6 Repository Nia must exercise the language users receive

- Standard-library, example, benchmark, and integration-fixture source follows
  the same idioms expected in user packages. Bootstrap history is not a reason
  to preserve cast-heavy or annotation-heavy source.
- Executable entries use `process::exit(code)!` for a numeric failure status,
  `process::ExitCode::Success` when an `ExitCode` success value is directly
  required, and `.exit().?` for reviewed error-union conversion. A raw
  `as process::ExitCode` belongs only inside the conversion implementation or a
  test whose stated subject is enum casting.
- Numeric literal types are inferred from function parameters, return types,
  fields, places, operators, and peer branches. A suffix remains only when it
  establishes otherwise absent type information or documents a required ABI,
  layout, serialization, bit-width, overflow, or mixed-width algorithm rule.
- Source cleanup is reviewed by subsystem rather than implemented as a blind
  repository-wide suffix deletion. Literal typing/coercion tests retain the
  explicit forms that they intentionally test; production-path std builds
  provide continuing inference evidence for ordinary unsuffixed forms.
- Aggregate type information is normally written once. A named local may use
  `let value: Point = { ... }` or `let values: [3]i32 = [1, 2, 3]`; an
  expression that must stand alone may use `Point { ... }` or
  `[_]i32[...]`. Repeating an inferred array type on both sides is reserved for
  tests of that syntax, not ordinary style.
- When a slice is already expected, code takes `&array` or `&mut array` and
  relies on the ordinary pointer-array-to-slice coercion. `&array[..]` remains
  meaningful for an explicitly materialized slice value or an actual subrange;
  it is not boilerplate required at every call.
- Representative maintained programs exercise adjacent and multiline strings,
  aggregate contextual typing, pointer-array coercion, and `if ... is`. A
  language feature documented only in parser tests is not sufficient ergonomic
  evidence for std design.

### 4.7 Determinism and resources are architectural

- Plan encoding, graph validation, ready-queue ordering, diagnostics, and output
  manifests are deterministic for equivalent inputs.
- Execution may finish out of order, but visible result order is stable.
- Compiler work shares inherited Cargo/GNU Make jobserver capacity and the
  existing process CPU budget. LLVM work continues to obey its memory permits.
- External actions declare a resource class or use a conservative default.
- No build provider or std helper creates an unmanaged private worker pool.
- Parallelism is not accepted until failure cancellation, output ownership, and
  memory bounds are defined.

### 4.8 Migration has a deletion boundary

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

### 5.2.1 Standard-library usability reconstruction

The completion of the build-host foundation does not freeze its bootstrap-era
API shapes. A standard-library reconstruction track runs beside Phases E and F
and gates the public artifact/path surface in Phase G. Phase H verifies and
removes leftovers; it is too late to begin the design there.

The first reconstruction slice is text because build plans, paths, diagnostics,
formatting, process arguments, and environment values all cross it. The current
surface has capability but no single ergonomic contract:

- a string literal is an addressed fixed `[N]char` value and naturally coerces
  to `&[char]`;
- the former `StringView` was a one-field wrapper around `&[char]` with duplicate
  `text()` and `as_slice()` accessors and no nominal invariant; the active text
  reconstruction retires it in favor of the slice;
- `StringBuf` is an owned `ArrayList[char]`, but allocation ownership is repeated
  at construction, every growth operation, extraction, and deinitialization;
- adjacent and multiline literals cover compile-time construction, while
  runtime concatenation is split between append and formatting without one
  documented choice;
- scalar UTF-8 decoding is typed, and owned text now has a whole-buffer
  `StringBuf::fromUtf8` boundary; empty input is valid text while non-empty
  truncation and invalid sequences retain their decoder cause;
- comparison, search, hashing, splitting, replacement, parsing, and formatting
  must work consistently across borrowed literals and owned text rather than
  requiring wrapper-specific adapters.

The redesign starts from user workflows, not from renaming the existing two
types. Its required decisions and acceptance order are:

1. make `&[char]` the provisional canonical borrowed text unless a wrapper
   demonstrates an invariant beyond storage shape; define exactly one public
   owned/mutable scalar-text type before deciding whether the surviving name is
   `String` or `StringBuf`;
2. specify scalar length versus UTF-8 byte length, validation, truncation, and
   invalid-sequence errors. Validated UTF-8 views/buffers, if introduced, remain
   distinct from arbitrary bytes and from scalar text;
3. choose one allocator ownership model. Explicit allocator parameters may
   remain when they preserve Nia's transparent memory model, but common
   construction/mutation must compose with `defer expr;` without repeated
   unsafe ownership reconstruction or hidden allocator lifetimes;
4. align literal coercion, adjacent and multiline literal construction,
   runtime append/format construction, comparison/hash/search, and path/process
   conversion around those roles;
5. migrate one vertical build workflow from literal through owned plan storage,
   formatting, path encoding, process execution, and `defer` cleanup before
   broad std renaming.

This track is explicitly an API and Nia-idiom exploration, not only a contract
migration. For every slice, representative programs must show the intended
ordinary Nia spelling and identify where users would still bypass `std`, write
the facility themselves, or descend to raw/native providers. A low-level
primitive passing conformance tests does not close its public API design when
the adjacent ownership, propagation, cleanup, or conversion workflow remains
awkward. Those findings feed the next bounded slice instead of being hidden by
compatibility helpers.

Legal aggregate and literal forms are not removed to manufacture style
uniformity. Ordinary repository code writes type information once and uses
direct array-pointer coercion when a slice is expected; alternate legal forms
remain covered by syntax and semantic tests.

Compiler-known convenience traits are a separate dependency-complete audit,
not a search-and-replace in `lib/std/builtin/place.nia`:

| Trait | Current structural role | Initial disposition |
| --- | --- | --- |
| `Len` | array length and slice metadata | ordinary trait candidate with compiler-provided array/slice impl bodies |
| `Start` / `End` | range field projection | ordinary trait or inherent range API candidate |
| `Ptr` / `PtrMut` | slice data-pointer projection and associated target | may retain intrinsic impl bodies, but builtin trait identity is unproven |
| `Char` | checked `u32` to `char` conversion | move to reviewed Unicode/inherent API; builtin trait identity is unjustified |

For each trait, the audit traces parser/symbol identity, type resolution,
projection solving, const evaluation, executable reachability, backend
dispatch, validation, LLVM lowering, and std declarations. Removal is accepted
only after user-defined implementations and structural compiler-provided
implementations share one ordinary trait-selection path, with const/runtime and
facade-closure regressions. A trait is retained as builtin only with a recorded
language-semantic or representation reason that an ordinary trait plus
intrinsic implementation cannot express.

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

### 5.6 Language And Public-Surface Decision Gates

Phase A classified the two language/public-surface proposals that can otherwise
force repeated churn through the build-host standard library. They are explicit
dependencies, not incidental syntax changes hidden inside Phase C. Implementing
them does not count as build-roadmap progress by itself, and Phase B toolchain
relocation does not wait for them.

#### Unified patterns and payload enums

Nia will use one pattern semantic model with context-specific legal subsets:

- `let` and `for` accept only patterns that cannot fail for their input type;
- ordinary `if condition` continues to require `bool`;
- `if value is pattern` performs one refutable match and scopes bindings to the
  successful branch;
- `switch value` is the canonical multi-arm matcher and owns overlap,
  reachability, and exhaustiveness diagnostics;
- the current `if pattern = value` and `or pattern` surface is removed after
  migration rather than retained as a compatibility spelling;
- optional and error-union patterns use the same matcher as enum, scalar, and
  range patterns while keeping their dedicated construction and propagation
  semantics.

The implementation must have one checked pattern representation and one
coverage/exhaustiveness owner. A shared AST enum with separate checker,
const-evaluator, and lowering semantics is not sufficient. Before changing the
parser, a focused syntax audit must settle the one remaining lexical ambiguity:
a bare identifier in binding position versus a named constant value pattern.
The rule must be explicit and independent of identifier capitalization; the
parser must not preserve two meanings by heuristic lookahead.

User-defined payload-carrying algebraic data uses the existing `enum`
declaration rather than a new `sum` keyword. The language supports all four
variant shapes:

```nia
enum Event {
    Closed,
    Data(Bytes),
    Move(i32, i32),
    Resize { width: i32, height: i32 },
}
```

All ordinary enums use a fixed `u8` backing tag when the backing type is
omitted. An explicitly written integer backing type controls tag size,
alignment, range, and discriminants. The compiler never silently widens a tag;
an enum with too many or out-of-range variants requires a wider explicit
backing type. Fieldless enums contain only the tag. If any variant has payload,
the representation is a tag at offset zero followed by padding and union
storage aligned for the largest variant payload. Size and alignment are fully
specified by the Nia ABI. Initial payload enums are closed, perform no niche
optimization or implicit allocation, and do not use the open-enum `_` form.

Enum values retain ordinary Nia value and pointer semantics:

- matching does not consume the scrutinee or create a moved state;
- payload bindings copy ordinary values into new locals, and `mut` changes only
  the local copy;
- pointer payloads copy pointers; Nia does not infer a borrow or lifetime;
- changing a variant is whole-value assignment and performs no automatic drop
  or cleanup;
- large or mutable shared payloads use explicit pointer types until a separate
  interior-payload pointer design is accepted.

`?T` and `E!T` remain language-defined optional and error-union types with
their existing constructors and propagation behavior. Their matcher and tagged
layout machinery may be generalized, but ordinary payload enums do not acquire
implicit propagation or a user-extensible `Try` protocol.

The initial feature does not expose a tag-extraction operation. If later build,
serialization, or ABI work justifies one, it must enter Nia's existing typed
builtin function/trait contract. It must not add a new `@tag` expression merely
because historical layout and low-level intrinsics use `@...` spellings.

Implementation proceeds as dependency-complete compiler batches:

1. consolidate pattern representation, resolution, coverage, const behavior,
   flow checking, and lowering, then let `switch` destructure current optional
   and error-union values;
2. introduce `if value is pattern`, migrate production/tests/std, and physically
   remove `if pattern = value`, `or pattern`, and their diagnostics/lowering;
3. extend enum signatures, constructors, patterns, type identity, layout,
   mangling, const evaluation, backend IR, codegen, and ABI validation for unit,
   tuple, and named payload variants;
4. add focused compile-fail, exhaustiveness, layout, const/runtime equivalence,
   generic, nested-pattern, invalid-tag-boundary, and codegen tests before std
   adopts payload enums in public contracts.

Language-track status:

- batch 1 is complete: AST, resolution, coverage, checked IR, const lowering and
  evaluation, flow analysis, executable-fact traversal, and function lowering
  use one recursive pattern model; `switch` destructures `?T` and `E!T`, copies
  payload bindings into arm locals, rejects bindings across alternative
  patterns, has const/IR/LLVM/resource-accounted executable equivalence guards,
  and leaves no `SwitchPattern` compatibility representation;
- batch 2 is complete: `if value is pattern` is the only single-match syntax;
  production, std, examples, documentation, and embedded test source use it or
  `switch`; the AST and checked IR own one pattern and one successful branch;
  const and function lowering emit one test with then/else edges; and the old
  `if pattern = value`, `or pattern`, multi-arm AST/IR, parser lookahead, and CFG
  chain have been physically removed;
- batch 3 is complete: enum signatures and checked types represent unit, tuple,
  and named payloads; constructors and recursive patterns share semantic field
  validation; const lowering/evaluation preserves nominal variant identity and
  payload values; function/backend IR, optimization traversals, fingerprints,
  LLVM construction/projection, ABI layout, and backend validation cover every
  variant shape; backend finalization recomputes the complete layout closure of
  finalized and cross-module-inlined bodies rather than relying only on source
  signatures; and focused parser, checker, const, LLVM, and resource-accounted
  linked-executable guards pass without a legacy enum path;
- batch 4 is complete: focused compile-fail tests reject payload arity, shape,
  missing, unknown, and duplicate-field errors; recursive exhaustiveness joins
  single-field variant arms through nested enums, optionals, and error unions,
  while multi-field products remain conservative unless one arm covers the
  product; default-backing tag boundaries reject implicit and explicit
  overflow; generic checking and a cross-module generic executable preserve
  nominal enum values; and exact layout, LLVM construction/projection, const
  evaluation, and resource-accounted const/runtime equivalence guards pass.
  The language track is closed, so reviewed Phase C APIs may now use payload
  enums without a temporary compatibility representation.

These batches must use the normal diagnostic and ICE boundary and the existing
resource-accounted integration harness. They must not create parallel legacy
and new pattern engines or run repeated unconstrained full compiler/LLVM suites.

#### Nia public naming

Reviewed Nia source APIs will use `lowerCamelCase` for functions, methods,
fields, parameters, and ordinary values. Types, traits, enums, and variants use
`UpperCamelCase`. Acronyms are word-cased, for example `readUtf8`, not
`readUTF8`. Rust compiler internals retain Rust `snake_case`; external schemas
and protocols retain their explicitly versioned field spellings. Module and
file naming remains a separate decision rather than changing mechanically with
function names.

Phase C first samples redesigned target APIs across allocation, collections,
text, path, filesystem, I/O, process, and build. It then migrates surviving
public contracts by subsystem together with compiler-known symbol references and
tests. Existing APIs are simplified or deleted before renaming: changing
`add_emit_executable_step` into `addEmitExecutableStep` is not a substitute for
designing a smaller `addExecutable` contract. A permanently mixed public std
surface or compatibility aliases for experimental spellings are not accepted.

## 6. Execution Phases

### Phase A: Baseline And Architecture Contract

Goal: turn the current audit into reproducible evidence and freeze the target
boundaries before migration.

Tasks:

- inventory current Rust `nia-build`, generated runner, `std::build`, Driver,
  cache, linker, std dependency, and test owners;
- record a build-host std dependency/public-surface matrix;
- classify each API in that slice as retain, layer violation,
  ownership/lifetime issue, error-model issue, naming/surface issue, or
  bootstrap-only/retire; current usability alone is not a retain decision;
- define representative build workloads: runner bootstrap, no-op warm build,
  single-source edit, module-map edit, generated source, failed action, and
  multi-artifact package;
- add machine-readable build timing/action counters without making telemetry a
  correctness input;
- finalize the resource-root, plan protocol, host/target, action, artifact,
  diagnostic, and cache invariants in stable architecture documents;
- resolve the public project boundary: build remains toolchain-owned in this
  repository; package registry/manager work remains separate.
- record language/compiler proposal decision gates that can change build/std
  contracts without making Phase A depend on unresolved proposal details;
- keep default test execution resource-safe: complete compiler/LLVM/build
  sessions run only through resource-accounted integration harnesses, with
  bounded subprocess time and process-tree cleanup.

Acceptance:

- each current build success/error contract maps to a target action/diagnostic
  contract or is explicitly deleted as experimental behavior;
- the std modules required to compile and run `build.nia` are enumerated with no
  hidden facade/provider dependency;
- representative workloads run from clean isolated directories and record
  action/compiler/link executions and wall/RSS observations;
- the target protocol and toolchain layout have one owner and no unresolved
  competing execution model;
- no current std/build API is promoted to a stable contract without an explicit
  maturity classification and migration disposition;
- the pattern/payload-enum and public-naming proposals have recorded semantic,
  migration, std/build-impact, and deletion dispositions;
- ordinary `cargo test` retains natural libtest concurrency without an
  unaccounted full build session or a hidden machine-specific test mode;
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
- distinguish releasing every owned allocation from transactionally restoring
  allocator cursor state; fixed/arena allocators need an explicit
  checkpoint/restore contract if plan construction relies on exact rewind;
- keep ordinary fs/process/format/allocation failure as typed values with enough
  context for build diagnostics;
- apply the accepted `lowerCamelCase` convention to reviewed surviving public
  contracts without retaining experimental spelling aliases;
- use payload enums for structured errors or build-plan state only after the
  unified matcher and payload-enum compiler gates pass their own acceptance
  matrix; do not freeze temporary enum-plus-union emulation into public APIs;
- add direct std conformance fixtures for the exact path, I/O, process,
  allocator, Unicode, collection, and formatting operations used by build;
- measure the actual loader semantic/body/backend closure rather than treating
  the conservative source-declared closure as compilation work;
- prove equivalent broad and narrow public `using` forms select the same work;
  do not optimize std by exposing or spelling package-private provider paths;
- keep package-root facade activation generic; only target/runtime resource
  roles such as the selected `start` module may receive std-specific handling;
- ensure ordinary programs do not load `std::build` implementation modules.

Acceptance:

- the build-host subset passes success, invalid UTF-8/path, unavailable file,
  process failure, allocation failure, and cleanup tests without panic or leaked
  output state;
- a build script can be compiled for the host while declaring an artifact for a
  distinct target without using target runtime APIs in the host process;
- plan-owned text/path values survive the builder call stack and allocator
  cleanup according to an explicit ownership contract;
- the reviewed public build-host surface follows one naming convention and its
  structured errors do not depend on a language compatibility shim;
- std public facade/provider tests prove unused build/OS/container modules are
  not loaded;
- changing only an equivalent public `using` spelling does not expand the
  selected provider, semantic, body, or backend module closure;
- the loader contains no std-name branch for ordinary package/facade loading;
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

### Standard-library reconstruction track: Text And Convenience Traits

Goal: replace bootstrap-era string/path friction and unjustified compiler-known
convenience dispatch before the broader artifact API depends on it.

This track starts during Phase E and can proceed beside Phase F. It must close
before Phase G freezes generated-source, run/test, install, environment, or
external-tool text/path contracts.

Tasks:

- publish the current text/path/Unicode/format/process API and error-flow matrix;
- settle borrowed scalar text, owned scalar text, validated UTF-8, arbitrary
  bytes, C strings, and OS paths as distinct roles with explicit conversions;
- redesign and sample construction, append/format concatenation, comparison,
  hashing, search, parsing, UTF-8 conversion, and path/process boundaries;
- prove allocator failure and normal cleanup with Nia's conditional and
  propagating `defer` forms, including cleanup-error precedence;
- migrate reviewed APIs to `lowerCamelCase` only after their role and error
  contract is accepted, with no compatibility aliases for experimental names;
- audit `Len`, `Start`, `End`, `Ptr`, `PtrMut`, and `Char` through every compiler
  layer listed in 5.2.1, removing builtin identity where ordinary trait
  selection plus compiler-provided structural impls is sufficient;
- keep aggregate/literal/coercion alternatives legal while making maintained
  std and examples exercise the canonical one-annotation style.

Acceptance:

- one maintained program carries non-ASCII literal text through owned mutation,
  runtime concatenation, formatting, UTF-8 encoding/decoding, path/process use,
  and deterministic `defer` cleanup with typed invalid/truncated/allocation
  failures;
- borrowed and owned text have one obvious conversion and comparison path, and
  no public wrapper duplicates `&[char]` without a documented invariant;
- literal scalar count, encoded byte count, embedded NUL, invalid UTF-8, and OS
  path representation are distinct tested contracts;
- std facade/provider tests show the redesigned workflow loads only the
  implementations demanded by behavior, independent of broad/narrow `using`
  spelling;
- each audited convenience trait is either ordinary or has a documented
  irreducible builtin reason, with structural, generic, const, IR, LLVM, and
  executable regressions;
- syntax tests retain all accepted aggregate, adjacent/multiline string, and
  array-pointer-to-slice forms while ordinary examples avoid redundant type
  annotations.

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
- consume the closed text/path roles from the standard-library reconstruction
  track rather than freezing bootstrap `StringView`/`StringBuf` accidents into
  artifact, environment, or command APIs;
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
- complete the text-model and convenience-trait migrations accepted by the
  reconstruction track and remove rejected wrappers, aliases, builtin
  identities, and bootstrap adapters;
- migrate maintained std/examples/fixtures to the source idiom matrix in 4.6
  and retain explicit syntax-focused cases for every alternative legal form;
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
- representative examples compile with user-facing string, pattern, aggregate,
  and coercion idioms, while structural checks show that raw exit-code casts and
  redundant contextual literal annotations have not returned;
- every remaining builtin trait has a recorded semantic reason that cannot be
  represented by an ordinary trait plus compiler-provided structural impl, and
  the reviewed string surface has end-to-end construction/mutation/format/path
  conformance rather than isolated capability tests;
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
     +-> Phase F cache/resources
     +-> std reconstruction
  -> Phase G artifact surface
  -> Phase H closure
```

The accepted language track proceeds beside Phase B but must settle before
Phase C freezes affected public contracts:

```text
unified matcher
  -> `if value is pattern` migration and old-surface deletion
  -> payload enum representation and codegen
  -> payload-enum use in reviewed Phase C APIs
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

The previous compiler roadmap recorded a real WSL OOM caused by concurrent
full build chains. Do not reintroduce it through build baselines or std
conformance tests. Unit tests stay cheap and deterministic; tests that spawn a
complete compiler, LLVM, or build session belong behind the existing
resource-accounted integration harness. External baseline commands run
sequentially, check available memory, enforce timeouts, and terminate the whole
subprocess group on timeout. WSL is not a separate semantic profile: the same
tests use the effective CPU/cgroup/VM memory limits visible to Linux.

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

## 11. Status And Progress

Phases A, B, C, D, E, and F are complete. The standard-library reconstruction
track remains active, and Phase G artifact/package-boundary work is the next
main build-system phase. Later implementation phases are not marked complete by
roadmap text.

The current compiler core is stable enough to support this work, but build/std
product maturity remains early. The first proof of progress is not a new build
feature. It is a compiler and std resource layout that works after relocation,
has explicit compatibility identity, and removes the compile-time checkout
dependency. The second proof is a deterministic frozen plan that the runner no
longer executes itself. Only after those two boundaries should cache,
parallelism, and artifact breadth accelerate.

Progress (2026-07-29): the Phase A owner/API audit, stable build/std contracts,
bootstrap action telemetry, representative isolated workload, current-case
migration matrix, and resource-safe execution rules are implemented. The
machine-readable build-host source closure contains 92 modules and exposes the
current facade overreach instead of treating existing std APIs as stable. A real
five-state release sample proved clean/warm, source-edit, module-map-edit,
generated-source, failed-action, and two-artifact behavior: warm object/link
reuse was exact, the source edit missed one object/link, the module-map edit
missed two objects/one link, and the failed action invoked no compiler. The
resource-accounted 12-case build integration passed; its 588.59-second runtime
also confirms that telemetry iteration belongs in the isolated baseline rather
than repeated full correctness runs. The unified matcher/payload-enum proposal
and the `lowerCamelCase` public naming proposal are now classified in Section
5.6 with explicit compiler, std/build, migration, and deletion boundaries. This
closes the final Phase A decision gates. Phase B relocation remains independent;
the language track must settle only before Phase C freezes APIs that depend on
its enum, error, or naming surface.

Language-track progress (2026-07-30): all four payload-enum batches are
complete. Syntax and semantic checking, recursive coverage, const evaluation,
finalized backend layout closure, LLVM lowering, invalid construction and tag
diagnostics, generic transport, nested patterns, and resource-accounted
cross-module const/runtime execution now have focused acceptance evidence. The
temporary language track is closed; Phase B relocation is the next critical
path, and Phase C may rely on the accepted enum and pattern surface when it
reviews build-host standard-library contracts.

Phase B progress (2026-07-30): the first relocation batch establishes
`nia-toolchain::ToolchainLayout` as the single owner of the compiler executable,
versioned resource root, std/runtime modules, host/artifact targets, and
path-independent compatibility identity. CLI, Driver, loader, `nia-build`, the
generated build runner, and `std::build` now carry that layout explicitly;
source development uses `--resource-root`, while installed discovery follows
`bin/nia -> ../lib/nia`. The production `default_std_module_path()` and its
`CARGO_MANIFEST_DIR` checkout inference were physically deleted. Focused
toolchain, loader relocation, CLI layout-failure, build, Driver, and
resource-accounted build acceptance passed. The next batch must place semantic
toolchain identity in frontend/object/link/build cache domains and prove copied
installed-tree `check`, object, executable, build, and relocation reuse before
Phase B can close.

Phase B completion (2026-07-30): the second relocation batch threads the
path-independent toolchain compatibility fingerprint through frontend,
provider-demand, LLVM object, compiler-builtins, link-result, and build-runner
compilation domains. `SourcePath` now separates the physical path used for I/O
and diagnostics from its logical `SourceIdentity`; installed std modules use
`toolchain:/...` identities, and persisted provider-demand plans contain only
those identities before strictly remapping them through the current entry and
module-map roots. No legacy cache reader or absolute-path fallback was retained.
The relocation probe also exposed and fixed a deeper executable-closure defect:
parse-valid shallow providers may now be activated by resolved references
without eagerly checking unrelated provider bodies. A resource-accounted CLI
acceptance test constructs an installed tree, copies it to another absolute
location, and proves relocated `check`, `emit --obj`, `emit --exe`, executable
launch, and `build`, with complete object/link reuse and no toolchain
invalidation. The current bootstrap `BuildPlan` is an in-memory request model
and has no persistent cache; build-runner compilation already uses the same
Driver frontend/object/link caches. The immutable frozen-plan cache introduced
in later phases must use this compatibility identity rather than inventing a
parallel domain. Together with the first batch's layout diagnostics and removal
of compile-time checkout inference, this satisfies every Phase B acceptance
gate. Phase C is now the critical path.

Phase C progress (2026-07-30): the first closure-discipline batch restores the
original contract that public `using` spelling is a human-facing namespace
choice, not a compilation-work control. The conservative 92-module
source-declared build-host closure remains a layering alarm only; observed
loader activation, checked/body modules, and backend modules are the execution
evidence. Ordinary package-root activation no longer contains a std-name
branch. A real broad-versus-narrow `std::collections::ArrayList` compilation
now proves identical semantic, body, and backend toolchain closures. That probe
also exposed a latent second-round defect: a shallow reexport provider selected
for semantic method resolution was checked without its own import scope.
Semantic provider activation now recursively selects the provider's concrete
import dependencies without promoting those dependencies to body activation;
the `ArrayList` raw implementation remains semantic-only when compiling a
`len` call. Loader and Driver regressions preserve both spelling independence
and the semantic/body boundary. Phase C remains open: its public build-host API
review and conformance matrix still have to be completed.

Phase C progress (2026-07-30): the second batch replaces the bootstrap build
graph's borrowed retained state with explicit builder ownership. Public
`ModuleOptions`, `ExecutableOptions`, and `ModuleImport` values remain cheap
borrowed call descriptors, but `Build::init`, `addStep`, `addModule`, and
`addExecutable` copy every retained root, name, path, import, and output into
`PathBuf`/`StringBuf`-backed internal records. `Build::init` is now fallible;
multi-stage construction registers conditional `defer` rollback before the
first allocation, marks transfer only after collection insertion, and deep
deinitialization runs in reverse order while attempting every release and
returning the first cleanup error. Rollback defers propagate cleanup errors, so
Nia's specified defer control-flow override exposes a failed release instead of
silently preserving only the initiating allocation error. The reviewed build API, generated runner,
fixtures, benchmark, help, and user documentation use `lowerCamelCase`; old
spellings and aliases are physically absent. A configured build constructs all
records from non-static arrays in a helper frame, returns, then successfully
emits and launches the custom executable. A fault-injecting allocator proves
partial BuildPaths, second-import, and target-output allocation failures return
`OutOfMemory`, preserve the builder's active allocation count, and finish with
zero active allocations; an injected rollback-free failure also proves the
defer cleanup error overrides the initiating `OutOfMemory` instead of being
discarded. This probe also records that successful `free` calls
do not imply exact cursor rewind for a fixed-buffer allocator when alignment
padding exists; exact transactional rewind requires the explicit checkpoint
contract above rather than an unsafe inferred offset. Phase C remains open for
host/target API separation, contextual errors, fixed-buffer removal from build
argv assembly, and the rest of the build-host conformance matrix.

Phase C progress (2026-07-30): the third batch separates build-host compilation
from artifact declaration at the actual Driver invocation boundary. A Driver
now owns its effective artifact target, defaulting once from `ToolchainLayout`;
build-runner compilation explicitly overrides it with the host target, and
loader pruning, object generation, linking, and cache identity consume that one
invocation value. The runner argv protocol transports both complete seven-field
target descriptors independently. `Build` deep-copies them into package-private
storage and exposes only borrowed `TargetView` accessors, following Nia's
existing view/storage discipline rather than publishing parallel `Target` and
`OwnedTarget` records. Executable records carry an explicit artifact role but
offer no arbitrary-target setter. The callback bootstrap rejects execution
when host and artifact differ because the current public CLI cannot yet accept
an explicit target; it cannot silently substitute a host artifact for a
cross-target declaration. Driver target-pruning, runner protocol, helper-stack
lifetime, mid-host and mid-artifact allocation rollback, distinct declaration,
and real configured runner execution now have focused evidence. Phase C remains
open for contextual errors, fixed-buffer removal from build argv assembly, and
the remaining build-host failure/conformance matrix.

Phase C progress (2026-07-30): the fourth batch removes the bootstrap's nested
argv capacity traps instead of moving them between layers. The fixed 16-import
encoding matrix and 48-entry build argv are replaced by allocator-owned
contiguous import bytes plus offsets and a dynamic pointer list. Import pointers
are created only after byte encoding is complete, so list growth cannot
invalidate retained addresses. The package-private process bridge likewise
replaces its 64-entry `ArgList` with an allocator-backed argv including argv[0]
and the null terminator; `ArgList` is physically deleted. Process allocation
failure now has a typed `OutOfMemory` category that maps through build without
panic or `InvalidTarget`. Conditional cleanup restores the active-allocation
count after a mid-import encoding failure. A real build with 32 module imports
crosses all three former limits, compiles, links, and launches its artifact.
Phase C remains open for contextual errors and the remaining invalid UTF-8/path,
unavailable file, process failure, partial-I/O, and cleanup conformance matrix.

Phase C progress (2026-07-30): the fifth batch replaces coarse bootstrap build
errors with payload-enum diagnostics that retain operation, indexed subject,
and the exact memory, formatting, filesystem, process, or child-termination
cause. Path/target decoding, retained input construction, argv assembly,
compiler execution, reporting, step cycles, and failed-step propagation all
use the same model. The generated runner reports initialization and defer
cleanup failures before a `Build` exists, and reports build-script/action
failures before telemetry, so an exit status is never the only user-visible
failure. Focused execution proves missing compiler `SpawnExec`, compiler exit
code 1, embedded-NUL module path, and rollback release failures preserve exact
context. The fault allocator was corrected after the probe showed that failing
`free` on an empty growth block produced false evidence: cleanup fault
injection now applies only to non-empty owned blocks, and the asserted
`Release/PackageRoot/FileSystem(Invalid)` path genuinely comes from rollback.
Error formatting also stopped returning slices to function-local string
literals; Nia has no inferred borrow lifetime, so such a view cannot escape its
frame. Adding early runner reporting exposed a compiler ICE where identical
trait-object vtables materialized in two modules gave default methods different
argument-module identities. Default vtable methods now use their trait
definition module, while selected implementations keep the actual use-module
context; the full 12-case runner integration passes. Phase C remains open for
the unavailable-file and partial-I/O conformance cases plus facade/provider
isolation of ordinary programs.

Phase C completion (2026-07-30): the final conformance/isolation batch adds
explicit unavailable-file coverage beside embedded-NUL path rejection and
confirms the existing partial-read, partial-write, process spawn, and repeated
failed-spawn cleanup fixtures. Loader coverage proves ordinary and freestanding
programs keep build and unrelated process providers shallow or absent. Driver
coverage now proves equivalent broad/narrow `ArrayList::len` imports select the
same semantic, body, and backend work and that none of those closures contains
`std::build`, hash-map, or OS provider modules. The 92-module source-declared
snapshot remains a deliberate conceptual layering alarm, not a compiler-work
metric. Together with owned retained values, host/artifact separation, dynamic
argv, contextual failures, reviewed `lowerCamelCase` APIs, and generic loader
activation, this satisfies every Phase C acceptance gate. Phase D immutable
plan ownership and handoff is now the critical path.

Phase D progress (2026-07-30): the first builder-identity batch replaces
index-only `StepHandle`, `ModuleHandle`, and `ExecutableHandle` values with
private `(ownerId, index)` handles. Each successfully initialized `Build`
receives a process-local owner from a monotonic atomic counter; the id is never
serialized and is not a stable plan key. Every public handle consumer validates
the owner before the index, including module-to-executable references,
executable-to-step references, dependencies, and default-step selection. A
two-builder runtime probe gives both builders index-zero values and proves all
four cross-builder paths return the exact contextual invalid-handle error. The
new atomic dependency deliberately moves the conservative build-host source
closure from 92 to 93 modules; observed ordinary `ArrayList` semantic/body/backend
work remains isolated. Phase D remains open for stable keys, typed plan values,
deterministic freeze/codec, validation, and handoff.

Phase D progress (2026-07-31): the outer Rust bootstrap request formerly named
`BuildPlan` is now `BuildInvocation`; no alias preserves the misleading name.
The actual immutable graph `BuildPlan` is constructed only through a validating
freeze from `BuildPlanDraft`. Package, module, artifact, action, and step keys
are structured stable identities rather than builder indices or integer hashes.
Package-rooted logical paths include their package key and reject absolute,
parent, current-directory, empty-component, NUL, and platform-separator
ambiguity. Freeze canonicalizes node and reference order and rejects duplicate
identities/imports, missing references, cycles, invalid output roots, and output
collisions. Typed actions cover compiler check/emit, external command, generated
file, aggregate, and explicit uncacheable semantics. Allocation-order
equivalence is directly tested. The versioned codec and production runner
handoff remain the next gate; this model is not yet claimed as the production
graph truth source.

Phase D progress (2026-07-31, codec/handoff batch): the canonical binary codec
uses `nia-toolchain`'s build-protocol schema as the single version source and
round-trips every typed action variant. It has distinct bounded domains for the
whole plan, collection counts, UTF-8 strings, and generated-content blobs.
Decode rejects bad magic, unknown versions/tags, invalid text/names/logical
paths, truncation, trailing data, and semantically invalid payloads, then runs
the decoded draft through the same freeze validator. Allocation-order-equivalent
plans encode byte-identically. The coordinator-side handoff uses a synced
same-directory temporary file, atomic rename, parent-directory sync, bounded
read, and cleanup on pre-publication failure; replace-existing and corrupt-file
cases are tested. This completes the Rust codec and atomic handoff primitives,
not the production handoff acceptance gate: `std::build` still needs to freeze
and publish the real Nia-built graph, after which the coordinator must decode it
before any action executes.

Phase D progress (2026-07-31, bootstrap graph-validation batch): Nia-side
modules now retain an explicit validated name and reject duplicates rather than
using root paths or builder indexes as identity. Before any action starts, the
bootstrap resolves the requested/default step and validates the complete graph,
including cycles outside the selected closure; successful validation is reused
until a graph mutation invalidates it. Allocation-failure probes separately
cover retained module/import rollback, validation rollback, and later command
assembly. The real build matrix exposed a compiler defect where indexing a
call-produced `&mut [T]` was rejected as a writable place. The checker and BIR
now model pointer/slice indexing as indirect place formation, while arrays and
user `IndexMut` receivers still require writable place bases; body-check and
LLVM tests preserve that boundary. These bootstrap checks prevent pre-action
side effects during migration, but do not complete Phase D: the Nia builder
still must freeze and publish the immutable protocol, and the coordinator must
become its sole consumer before recursive callbacks are removed.

Phase D progress (2026-07-31, connected handoff batch): the generated runner
now receives the toolchain-owned protocol schema and an explicit draft path.
After whole-graph builder validation it encodes the retained Nia modules,
artifacts, typed check/emit or explicit uncacheable actions, dependencies, host
and artifact targets, and resolved selection into the exact versioned binary
schema. The draft is exclusively created, fully written, flushed, and synced.
The coordinator decodes it through the existing bounded decoder and unique
freeze validator, then atomically publishes canonical bytes to
`.nia-build/build-plan.bin`; the draft is removed on success and runner failure,
and a later failed runner cannot replace a valid published plan. Stable-name,
logical-path, duplicate-import, target, count, and size rules are aligned on the
Nia side so a known-invalid builder cannot reach legacy action execution merely
to fail in the decoder. Explicit selected steps no longer require an unused
default in Rust freeze, while nonempty unselected plans still do. Real runner
evidence decodes the published module/import/artifact/action/step fields and
observes selected-plan replacement. This completes the Nia-to-Rust codec and
canonical publication connection, but not the Phase D production gate: legacy
callbacks still execute before the runner returns, so coordinator decode is not
yet universally prior to side effects. The next batch must execute the selected
closure from the decoded plan and physically remove runner-side recursive
execution and raw compiler argv assembly.

Phase D completion and Phase E progress (2026-07-31, coordinator execution
batch): `nia-build` now computes the selected dependency closure with iterative,
deterministic Kahn traversal and executes a shared action at most once. Typed
compiler check and executable-emission actions resolve package/build/cache/
toolchain/artifact logical paths, construct module maps, and call the Driver
directly with plan optimization, runtime, target, cache, timing, and output
values. Invocation host/artifact targets are checked against the frozen plan
before action execution, and package-root, import-map, path, unsupported-action,
and Driver failures retain action/package context without panic. The generated
runner now stops after validating and exclusively writing its draft. `StepFn`,
step runtime state, recursive execution, compiler subprocess argv assembly, and
runner action telemetry were physically deleted; explicit aggregate and
uncacheable declarations replace callback-shaped graph nodes. The complete
14-case build matrix passes through the coordinator, including typed check and
emit, deterministic dependencies, selected-step replacement, full-graph cycle
rejection, and pre-action invalid-plan failures. This closes every Phase D
acceptance gate. Phase E remains open for external-command execution, process
capture/cancellation policy, scoped publication replacing the package-wide
lock, and deterministic multi-worker execution.

Phase E progress (2026-07-31, generated-file batch): reviewed `BuildPathView`
values now distinguish build-rooted paths from package paths in the public
builder, and modules can explicitly consume a build-rooted generated source.
Generated-file payloads retain output text and bytes with allocator rollback,
encode as the existing bounded blob action, and execute through the coordinator
with exclusive temporary creation, full write, file sync, atomic rename, parent
directory sync, and temporary cleanup on failure. Focused Rust tests replace
previous contents atomically and reject a failed publication without a
leftover temporary file. The representative workload again generates a source,
then compiles and launches both source and generated artifacts through typed
dependencies; no package/build logical-path alias or runner callback is used.

Phase E progress (2026-07-31, run-action batch): reviewed `RunOptions` and
`addRunExecutableStep` retain an arbitrary argument list and encode the declared
executable as an Artifact-root external-command program with Package-root cwd
and no outputs. The builder requires the matching emit producer, inserts that
dependency atomically, and freeze rejects any decoded Artifact-root command
without the producer in its dependency closure. The coordinator closes stdin,
forwards stdout/stderr while retaining bounded 64-KiB tails, applies a
seven-minute limit, and terminates the owned Unix process group on timeout and
after leader exit so background descendants cannot retain pipes or outlive the
action. Spawn, capture, wait, timeout, and nonzero-exit failures preserve
action/program/cwd/status/output context without panic. Output-bearing external
commands are rejected before spawn because the current string argument schema
cannot redirect tools to staged outputs safely. Fault-allocation probes cover
partial argument retention, step/edge rollback, and the configured production
fixture now emits then runs its artifact through the decoded plan, preserving
arguments and forwarded output. A separate production case round-trips and
executes 32 imports and 32 run arguments, proving the old fixed argv limit did
not return. Phase E remains open for typed staging of output-producing external
tools, coordinator-wide failure cancellation, scoped publication replacing the
package lock, and deterministic multi-worker execution.

Phase E progress (2026-07-31, staged external-tool batch): build-plan schema 2
replaces opaque command strings with typed `Literal`, `InputPath`, and
`OutputPath` arguments. Freeze requires every path argument to match its
input/output declaration and rejects unbound outputs. The reviewed
`CommandArgument`/`ExternalCommandOptions` std API derives declarations from
`packageInput`, `buildInput`, and `buildOutput` arguments, retains all values in
builder-owned storage, and proves partial-allocation rollback with the fault
allocator.

The coordinator resolves input arguments to accepted paths and output
arguments to a unique same-filesystem staging directory. A successful tool
must create a regular file; the coordinator syncs it, atomically renames it,
syncs the destination parent, and retires staging. Nonzero exit and
missing-output regressions preserve the previous accepted destination and
leave no stage directory. The configured production build now executes
compiler emit, artifact run, and a package-input-to-build-output external tool
through one decoded plan, and asserts the typed protocol fields and published
contents. The versioned toolchain manifest moved with the schema, so old plans
are rejected rather than heuristically decoded.

This batch deliberately limits external commands to at most one output file.
Publishing several destinations requires a transaction that can restore every
old destination after a partial commit; sequential renames are not presented
as atomic. Phase E remains open for that multi-output transaction,
coordinator-wide failure cancellation, scoped output/cache publication in
place of the package lock, and deterministic multi-worker execution.

Cross-cutting progress (2026-07-31, representative-source and provider-signature
batch): repository executable fixtures now use `process::exit(code)!`,
`process::ExitCode::Success`, or reviewed `.exit().?` conversion instead of
scattered raw exit-code casts. A structural guard rejects qualified raw casts,
and ordinary build/std numeric literals rely on contextual inference while ABI,
layout, bit-width, and syntax-test annotations remain explicit. Maintained
examples now exercise adjacent and multiline strings, `if value is pattern`,
contextual struct/array literals, and direct array-pointer-to-slice coercion;
all 11 examples parse and seven representative examples pass semantic checks.

The expanded examples exposed a clean-baseline executable defect rather than a
std import workaround: a shallow provider selected into checked/codegen modules
was present in the extension-method index but absent from monomorphization and
backend non-function signatures. Direct dispatch could therefore work while a
nested generic `where` chain such as
`Take[Rev[SliceIterMut[i32]]] : Iterator` lost its
`SliceIterMut : DoubleEndedIterator` witness and produced unresolved method and
layout ICE diagnostics. The executable signature snapshot now merges exactly
the actually checked modules with the base semantic set, deduplicates impl
identity, and rebuilds trait indexes. A focused shallow-provider generic
regression, the ArrayList nested-adapter executable, and the std-root
CString/path executable all pass. Equivalent `using` spelling remains a
namespace choice; no provider-path import narrowing or std-specific loader
branch was introduced.

The same batch promotes text ergonomics and convenience-trait removal from a
late Phase H audit into the active reconstruction track in 5.2.1. Current
`StringView`/`StringBuf` names and builtin `Len`/`Start`/`End`/`Ptr`/`PtrMut`/
`Char` identities are explicitly unfrozen. This records direction and
acceptance evidence; it does not claim that the text or builtin-trait migration
is implemented.

Phase E progress (2026-07-31, scoped-publication batch): concurrent builds no
longer share a runner executable or plan-draft path. Each invocation uses a
process/sequence-qualified pair and retires both after runner completion, while
the canonical plan remains an atomic last-completed observation rather than an
execution input after decode. The package-wide executor lock and its timing
stage are physically deleted.

Every compiler emit, generated file, or external-tool output now acquires a
cross-process lock derived from the validated build-root logical path. Locks
live in the cache coordination namespace rather than consuming a user-visible
build output path. Equal outputs serialize for the complete action; distinct
outputs use distinct locks and can progress independently. Owner records carry
PID, process start time, and an acquisition sequence to prevent stale-owner and
same-process ABA removal. Focused tests prove matching-path serialization,
different-path independence, dead-owner reclamation, stable/domain-local keys,
invocation path isolation, and lock retirement on successful and failed
publication. Phase E remains open for coordinator-wide failure cancellation,
deterministic multi-worker execution, and a real multi-output publication
transaction.

Phase E progress (2026-07-31, deterministic-scheduler batch): selected closure
execution now advances in canonical readiness waves and submits each wave to a
`QuerySession`. Build actions therefore inherit the same process-wide
Cargo/GNU Make jobserver capacity as compiler queries, with no coordinator-owned
worker pool. Target-specific Drivers are created once per invocation and shared
across concurrent compiler actions. Submission-order outcome merge keeps step,
action, and failure reporting identical when completion order changes.

Failure state is ordered by canonical action position. A later failure cancels
later work immediately but allows earlier positions to settle and supersede it,
so worker timing cannot choose a different first diagnostic. No dependent wave
is submitted after failure, all active work is joined, output-lock acquisition
is interruptible, and external commands observe cancellation in their wait loop,
terminate the owned process group, preserve captured output context, and retire
staged output. Focused tests cover reversed completion order, stable visible
reports, dependency suppression, active-wave settlement, ordered cancellation,
interruptible lock waits, and real child-process termination. Phase E remains
open for the previously identified multi-output publication transaction; this
batch does not claim that later cache/resource-class scheduling in Phase F is
implemented.

Phase E completion (2026-07-31, multi-output transaction batch): build protocol
schema 3 admits multiple declared `BuildOutput` arguments through the reviewed
Nia API, binary codec, freeze validation, and coordinator. Every logical output
maps to a distinct same-filesystem staging file while the action holds all
destination locks in canonical order.

Publication validates and syncs the entire produced set before modifying a
destination. It then backs up old regular files, installs new files, syncs every
affected parent, and reaches an explicit transaction acceptance point only
after all outputs are present. Pre-acceptance failure restores backups in
reverse order, removes newly installed paths that were previously absent, and
retires staging. Separate destination entries are not described as an
instantaneous filesystem swap; they are unaccepted and protected by the action's
cross-process locks until the complete transaction commits. Process-death
journal recovery and garbage collection remain part of the interrupted
publication work in Phase F, distinct from Phase E's ordinary failure and
cancellation contract.

Focused tests prove successful cross-directory multi-output publication,
missing-output zero-publication, rollback after a partial install, rollback when
the acceptance marker fails, restoration of both old and previously absent
destinations, and cleanup of staging/backup state. The production build matrix
now emits two typed tool outputs and validates both decoded bindings and file
contents; all 14 cases pass through schema 3. With deterministic scheduling,
process-tree cancellation, scoped output ownership, and multi-output rollback
all accepted, Phase E is complete. This does not claim the action cache,
fingerprints, restoration, miss diagnostics, or resource classes assigned to
Phase F.

Phase F progress (2026-07-31, generated-file action-cache batch): the
coordinator now owns a versioned immutable cache slice for `GeneratedFile`
actions. Its typed fingerprint covers the stable package/action key, canonical
logical output, exact contents, and resolved toolchain compatibility identity;
the latter carries compiler, resource-layout, std, and build-protocol identity.
Compiler and external-command actions deliberately remain outside this slice,
so incomplete source/dependency or executable/environment identity can never be
reported as an action-cache hit.

Entries are partitioned by stable action key and addressed by the complete
fingerprint. Their envelope repeats key, component, output, length, checksum,
and payload facts. Reads reject truncation, trailing bytes, mismatched paths,
and corrupt payloads, retire corruption, and distinguish absent, contents,
output, compiler, resource-layout, std, build-protocol, corruption, read-I/O,
and write-I/O miss outcomes. A cache hit validates or atomically restores the
generated destination; an already matching regular destination avoids
publication work.

Publication syncs a same-directory temporary file and installs it through a
no-overwrite hard link. Duplicate publishers validate the same immutable entry,
ordinary readers remain lock-free, and mutation locks cover only install and
corruption retirement. Revalidation under that lock prevents a stale corrupt
reader from deleting a newly published valid entry. Focused tests cover cold
and warm restoration, component-exact invalidation, nonfatal cache I/O failure,
corruption retirement and republish, duplicate publishers, stale retirement,
and concurrent readers that never accept partial bytes. Phase F remains open
for compiler/external action closure caching, dependency artifact propagation,
resource classes and inherited capacity policy, interrupted output-transaction
recovery, stress matrices, and the full no-change acceptance measurement.
Validation passes all 68 `nia-build` tests, workspace all-target/all-feature
check, strict Clippy, formatting, and the 14-case production CLI build matrix
(`1154.19s`).

Phase F progress (2026-08-01, interrupted-output recovery batch): multi-output
external-command publication now writes a versioned, length-bounded,
checksummed journal under `.nia-build/.nia-transactions/v1/`, rather than under
the disposable cache. The journal records the stable action identity, ordered
logical Build outputs, and logical staging/acceptance paths and is published and
synced before the command can mutate staging. After all staged regular files
are synced, a separate checksummed prepared marker records which destinations
previously existed and is durably published before any destination rename. The
existing stage-to-committed directory rename remains the acceptance point.

Every coordinator invocation scans recovery state before action dispatch,
acquires the complete output set in canonical lock order, and rereads the
journal under those locks. It discards unprepared staging without changing
destinations, reverses prepared but unaccepted installs, and preserves accepted
outputs while retiring backup state. Recovery rollback moves installed outputs
back into staging so another interruption remains repeatable. Corrupt,
truncated, trailing, non-regular, contradictory, or lock-raced state blocks the
build with typed action/package/path context rather than guessing. Linux
temporary journals include PID/start-time identity for dead-owner collection;
live publishers are retained. Plan validation reserves `.nia-transactions` as
coordinator-owned Build space.

Focused tests cover unprepared cleanup, partial old/absent-output rollback,
repeatable partially completed rollback, accepted-output preservation, corrupt
journal and prepared-marker failure, output-owner waiting, locked journal
revalidation, dead/live temporary collection, strict codec rejection, startup
recovery before dispatch, and ordinary success/failure cleanup. Compiler action
caching remains blocked on a complete recursively discovered source manifest;
external-command caching remains blocked on inherited environment and arbitrary
working-directory reads. Phase F therefore remains open for those input
closures, dependency-artifact propagation, declared resource classes,
deterministic single-worker and concurrent stress acceptance, and the complete
warm-build measurement. Validation passes all 80 `nia-build` tests, workspace
all-target/all-feature check, strict Clippy, formatting, and the 14-case
production CLI build matrix (`1162.18s`).

Phase F progress (2026-08-01, bounded-action scheduling batch): `nia build` now
accepts the conventional `-j N`, `-jN`, `--jobs N`, and `--jobs=N` forms, while
the Rust API carries the same nonzero policy through
`BuildRequest::with_max_parallel_actions`. Each readiness wave continues to use
the shared `QuerySession`; the configured value is passed to its bounded task
path and can only reduce inherited executor/jobserver capacity. No private pool,
worker-count environment variable, or separate scheduler was introduced.
Compiler actions retain their existing query and LLVM memory budgets, so
`--jobs=1` means one ready build action at a time rather than disabling safe
internal compiler resource accounting.

The configured limit is observationally reported as
`build.action_parallelism_limit`. Focused tests cover all CLI spellings and
invalid zero/missing/nonnumeric values, request-to-invocation propagation, a
64-task wave whose single-worker peak is exactly one, and 32 bounded executions
of a 48-wide graph with scheduling jitter that must reproduce the same canonical
step/action report. The real step-order build fixture runs through `--jobs=1`.
The focused `nia-build` suite (83 tests), workspace check, strict Clippy,
formatting, and the production build matrix all reach the new path; the matrix
passes in `1106.21s`, followed by passing `command_cases` (`15.76s`) and
`linker_cases` (`29.83s`). A later full workspace run also passes both cache
fixtures and the surrounding build/integration suites. Its allocator blocker
had two causes: the realloc-cleanup fixture used `checked_add` and
`align_forward` without explicitly importing `std::math`, and the global
extension trait index included shallow provider modules but not the trait
modules referenced by their signatures. The fixture now declares its actual
math dependency, while the compiler closes shallow provider trait facts over
their signature type-module dependencies before validation and indexing.
Focused shallow imported-trait validation, HashMap context/iteration, and all
10 allocator acceptance cases pass.

The newly unblocked workspace tail exposed two older acceptance gaps. The LLVM
codegen test harness had not been migrated to the explicit relocatable
toolchain layout, so its std/start tests could not load the `std` namespace;
and the payload-enum lowering migration sent zero-field enum patterns through
the generic condition-chain path, losing the established direct LLVM switch
shape. Codegen tests now receive the same explicit development layout as other
compiler tests, and zero-field enum patterns lower through a tag-valued
`FunctionTerminator::Switch` while payload patterns retain the condition-chain
path. The full workspace, including the 14-case production build matrix,
`command_cases`, `linker_cases`, 197 LLVM codegen tests, 490 Driver tests, all
remaining crate tests, and doc tests, passes. This closes the explicit
deterministic single-build-action-worker slice, while Phase F remains open for
declared resource classes, broader cross-process/cache stress acceptance,
compiler/external input-closure caching, dependency-artifact propagation, and
the complete warm-build measurement.

Phase F progress (2026-08-01, repeated warm-build measurement and equivalence
batch): the long production build-matrix runtime is not evidence of a recent
matrix-wide regression. Its recorded runs remain in the same band: `1162.18s`,
`1106.21s`, `1130.73s`, `1135.83s`, `1125.78s`, and `1159.63s`. The suite loops
14 fixtures serially, creates an isolated workspace/cache for each fixture, and
holds one conservatively weighted `Build` resource permit for the complete
matrix. That structure deliberately favors bounded correctness evidence and
amplifies the absolute cost of cold compiler startup; it is not the performance
sampler.

The representative baseline now uses the explicit relocatable resource root,
runs three fresh-workspace repetitions by default, preserves raw samples, and
reports median/p95/min/max for wall/RSS and the outer runner compile, runner
execution, and plan execution stages. Its accepted sample records clean/warm
wall medians of `17.058s`/`5.818s`. Warm time is dominated by runner compilation
(`4.731s` median) and plan validation/execution (`1.020s`); executing
`build.nia` itself takes about `0.0029s`. All three first-warm samples hit the
generated action cache, reused 126/126 runner objects and all three link
results, and reported zero object or link misses.

The repeated gate first exposed 15 definition-invalidated objects and one link
miss on some first-warm runs. The two object entries for every affected generic
std partition contained byte-identical native payloads but different definition
fingerprints. Backend cross-module discovery had allowed function/global
instances to reach codegen partitions in discovery order; partition membership
now canonicalizes source definitions by stable `DefId` and instances by stable
mangled symbol before both fingerprinting and LLVM consumption. A persistent
`extension-trait-solving-facts` product was also rejected and physically
removed: it did not reduce the end-to-end warm path, had no measured speed
benefit, and could perturb the downstream identity sequence. The in-session
typed query remains.

This closes Phase F's complete warm-build measurement and clean/first-warm
object/link equivalence slice. Phase F remains open for declared resource
classes, broader cross-process/cache stress acceptance, compiler/external
input-closure caching, and dependency-artifact propagation.

Phase F progress (2026-08-01, declared resource-class scheduling batch): build
protocol schema 4 adds a required resource class to every external-command
action. `std::build::ActionResourceClass` exposes `Conservative`, `Cpu`, and
`Io`; `ExternalCommandOptions::search` chooses the conservative default and
`withResourceClass` permits an explicit declaration. Nia-side validation and
encoding and Rust-side decoding reject unknown open-enum values or protocol
tags. Compiler actions map to `Cpu`, generated-file and aggregate actions map
to `Io`, and uncacheable actions remain conservative.

The coordinator now derives one action-resource capacity from the minimum of
the optional `--jobs` bound and inherited `QuerySession` capacity. CPU and I/O
actions each reserve one slot. A conservative action reserves the complete
capacity, so an undeclared external tool cannot overlap another same-wave
action. The budget wraps the existing session-owned task path; it creates no
executor, worker-count environment variable, or alternate compiler/LLVM
resource policy. Timing reports the effective capacity once per invocation and
counts dispatched actions by class. Focused tests cover schema round-trip and
invalid tags, inherited-capacity reduction, conservative exclusivity and
waiting, declared-class sharing, and deterministic 48-wide scheduling stress.
A real configured build with `--jobs=2` observes capacity two, one action of
each class, and complete 133-object/two-link warm reuse without misses. The
resource-accounted 14-case production CLI build matrix passes in `1214.08s`,
followed by passing workspace all-target/all-feature check and strict Clippy.

This closes Phase F's declared resource-class slice. Phase F remains open for
broader cross-process/cache stress acceptance, complete compiler and external
action input closures, and dependency-artifact propagation.

Phase F progress (2026-08-01, cross-process action-cache stress batch): the
generated-file cache acceptance now crosses the process boundary rather than
inferring it from thread races. Eight independent worker processes, released
from one filesystem barrier, divide into duplicate publishers and lock-free
readers of the same 4-MiB content-addressed entry. Readers accept only absence
or a fully decoded, identity-checked, checksummed payload. After all workers
settle, the key namespace must contain exactly one immutable entry and no
staging file or mutation lock. The parent owns every worker through an RAII
child set and kills and waits for outstanding children on timeout or
assertion failure, so the stress gate cannot leak background work.

Together with the existing stale-corruption revalidation, concurrent-reader,
duplicate-publisher, interrupted output-transaction recovery, distinct-output
coordination, deterministic wide-wave scheduling, and repeated warm-build
evidence, this closes Phase F's broader cross-process/cache stress slice. Phase
F remains open for complete compiler and external-action input closures and
dependency-artifact propagation. All 89 ordinary `nia-build` tests pass, the
worker-only entry remains explicitly ignored outside its parent probe, and
workspace all-target/all-feature check and strict Clippy pass.

Phase F progress (2026-08-01, relocatable compiler-source identity batch):
compiler actions now carry the frozen plan's typed source identity through the
coordinator, Driver, and loader instead of collapsing it into an absolute path.
Package, build, cache, toolchain, and dependency-artifact roots receive
domain-separated logical identities while retaining their current-invocation
physical paths for I/O. Recursive module discovery derives both roles from the
parent source, so relocating the same package changes every physical path but
produces the same loader-owned logical source manifest. Focused loader tests
exercise entry and child-module relocation, and coordinator tests cover both a
package-rooted entry and a build-rooted explicit module mapping.

This establishes the stable identity prerequisite for a compiler action input
manifest; it does not yet make compiler actions cacheable. Phase F remains open
for persisting and validating the complete loader-owned source closure,
external-command input closure, and dependency-artifact propagation. The full
workspace test suite passes, including the 14-case production build matrix in
`1116.57s`, `command_cases` in `15.92s`, and installed-toolchain relocation in
`134.80s`; workspace all-target/all-feature check, strict Clippy, and formatting
also pass.

Phase F progress (2026-08-02, loader-owned compiler source-manifest batch): the
loader now exports one stable, recursively discovered source-input manifest
instead of requiring a build consumer to inspect or reproduce its module graph.
Each logical source identity retains its current physical I/O path and records
either a content fingerprint plus byte length or an explicit missing state. A
fully present closure uses the compiler frontend's existing order-independent
program-source fingerprint; any missing source suppresses that aggregate so a
deleted file cannot validate a cache hit. The Driver forwards the exact loader
product through a read-only API.

Relocation tests prove that equal source trees under different absolute roots
produce equal manifests and aggregate fingerprints, while a content edit
changes the fingerprint. Missing-child coverage proves that the closure remains
inspectable but uncacheable, and a Driver regression proves the public boundary
retains recursive build identities. This closes the representation/API slice of
the compiler input closure. Phase F still requires a versioned compiler action
record that combines and validates this manifest with target, runtime, options,
toolchain, output, and dependency artifacts before compiler execution can be
skipped; external-command closure and dependency-artifact propagation also
remain open. All 81 `nia-loader-query`, 491 `nia-driver`, and 90 ordinary
`nia-build` tests pass; the build-cache worker helper remains intentionally
ignored outside its parent probe. Workspace all-target/all-feature check,
strict Clippy, and formatting also pass.

Phase F progress (2026-08-02, compiler-check action-cache batch): the build
coordinator now owns a versioned immutable success record for `CompilerCheck`.
Its canonical identity combines the stable action and module declaration,
target, optimization, runtime, the loader-owned sorted logical source closure,
and separate compiler, resource-layout, standard-library, and build-protocol
components without retaining absolute physical paths. Lookup validates the
current loader manifest before an exact hit skips semantic Driver checking.
Source, module, target, optimization, runtime, and toolchain changes remain
component-exact invalidation reasons.

Publication uses the final source manifest paired with the exact Driver loader
session after semantic provider discovery, rather than reusing the pre-check
lookup candidate. Only a successful check with a complete source closure and
zero diagnostics publishes a record. Warning-producing checks, missing sources,
incomplete manifests, and failures remain uncacheable. The record contains no
session-local `CheckedProgram` and does not claim compiler emit restoration.
Truncated, trailing, identity-mismatched, or corrupt records are rejected and
retired under scoped cache coordination; publication remains immutable and
atomic for concurrent readers.

Focused coordinator coverage proves cold/warm behavior, source and optimization
invalidation, relocation through one shared cache, repeated warning misses,
corrupt-record recompilation, and missing-source non-publication. Codec tests
cover canonical round-trip, every truncated prefix, trailing bytes, identity
mismatch, and the new component classifications; a freestanding std-dependent
case additionally proves that the persistent provider-demand plan restores the
final semantic source closure for a warm hit. All 81 `nia-loader-query`, 491
`nia-driver`, and 99 ordinary `nia-build` tests pass; the build-cache worker
helper remains intentionally ignored outside its parent probe. Workspace all-
target/all-feature check, strict Clippy, formatting, and diff checks pass.

A real copied `configured_optimization` package run through production
`nia build` reports `Miss(NotFound)` and `4.421s` plan execution when cold, then
one action-cache hit and `0.407s` plan execution when warm. Total invocation
time remains dominated by build-runner compilation (`84.469s` cold and
`30.203s` warm), so this evidence credits only the compiler-check execution cut
and does not reinterpret the separately recorded runner compilation bottleneck.
This closes the successful zero-diagnostic compiler-check slice of Phase F.
Compiler emit output restoration, external-command input closure, and
dependency-artifact propagation remain open.

Phase F progress (2026-08-02, compiler-emit action-cache batch): the build
coordinator now owns a versioned immutable `CompilerEmit` success binding while
the Driver remains the only owner of executable bytes and link-result cache
protocol. The action identity reuses the final loader-owned logical source
closure and effective freestanding compiler configuration, then adds declared
artifact/runtime identity, logical output, and the current target/linker/options
environment. Its payload is only a checksummed fixed-width typed reference to a
Driver link product; executable bytes are not duplicated under the build action
namespace.

On an exact action hit, the Driver revalidates toolchain, target and default
library paths, resolved linker path/bytes/flavor, and structured link options
before atomically restoring the executable. Missing, corrupt, unreadable, or
invalidated referents retire the exact build binding and fall back to normal
compile/link execution. A live binding cannot be silently replaced by a
different reference. Publication requires a successful zero-diagnostic emit,
the complete final source manifest from that exact loader/Driver session, and a
successfully published Driver link result. Warning-producing emits and opaque
link environments remain uncacheable.

Focused coverage proves cold miss/warm hit, deleted-output restoration, source,
optimization, logical-output, and artifact-runtime invalidation, relocation
with a shared cache, dynamic std provider closure restoration, warning
non-publication, action-record corruption, and missing/corrupt Driver referent
self-repair. Codec tests reject every truncated prefix, trailing bytes, raw
identity mismatch, and reference checksum damage; linker and Driver owner tests
cover environment changes and fixed-width reference round trips. This closes
the compiler-emit output-restoration slice of Phase F. External-command input
closure and dependency-artifact propagation remain open.

A production copied `configured_optimization` package confirms the complete
cut. Its cold run reports `Miss(NotFound)`, `4.598s` plan execution, and
`90.947s` total wall time, of which `86.340s` is build-runner compilation. The
unchanged run reports one build-action cache hit, `0.417s` plan execution, and
`31.032s` total wall time, still dominated by `30.609s` runner compilation. The
warm run's 103 object hits and one link hit belong to that runner compilation;
the artifact emit itself executes no semantic/codegen/link path after action
validation. This preserves the earlier conclusion that runner compilation, not
coordinator action execution, is the remaining warm-build bottleneck.

All 19 `nia-linker`, 491 `nia-driver`, and 107 ordinary `nia-build` tests pass;
the build-cache worker helper remains intentionally ignored outside its parent
probe. Workspace all-target/all-feature check, strict Clippy, formatting, and
diff checks also pass.

Phase F progress (2026-08-02, hermetic external-command declaration batch):
build protocol schema 5 now records independent environment and cache policies
for every external command. Existing callers retain inherited environment and
explicitly uncacheable behavior. A build script can instead clear the child
environment, provide owned explicit name/value entries, and assert that its
typed command inputs form the complete semantic file-input set. Plan freeze
accepts that assertion only when the environment is cleared and at least one
output is declared; unknown policy tags, invalid or duplicate environment
names, and outputless or inherited cache declarations are rejected.

The coordinator applies the clear-before-explicit-value contract to the real
child process, and the configured production fixture exercises that contract
with two staged outputs. This batch intentionally establishes eligibility
only: it does not publish or restore an external-command cache record, and it
does not claim that a declared command is unable to inspect arbitrary
working-directory state. The next external-command batch must bind the resolved
tool bytes, logical working directory, explicit environment, declared regular
file and dependency-artifact inputs, logical outputs, and toolchain/protocol
identity into one immutable multi-output record. Inherited and uncacheable
commands must never report hits. Dependency-artifact propagation remains open
alongside that persistence work.

All 110 ordinary `nia-build` tests pass with one cache-worker helper ignored,
and all 4 `nia-toolchain` tests pass. Workspace all-target/all-feature check,
strict Clippy, formatting, and diff checks pass.

Phase F progress (2026-08-02, external-command multi-output cache batch):
commands admitted by the schema-5 hermetic boundary now use an immutable local
record under `.nia-cache/actions/external-commands/v1/`. The stable identity
binds the action and typed command declaration, logical cwd, sorted explicit
environment, resolved tool declaration and bytes, logical paths plus contents
of declared regular inputs, separately classified dependency-artifact inputs,
ordered logical outputs, and compiler/resource-layout/std/build-protocol
compatibility components. Search tools resolve to a concrete executable before
lookup without retaining its absolute installation path in the identity.

The payload is the complete ordered regular-file output vector, with an
independent length and checksum for every entry. Hits stage and restore that
vector through the existing recoverable multi-output transaction and start no
child process. Misses execute normally, commit outputs first, revalidate tool
and input snapshots, and only then publish. A racing tool/input change leaves
the successful build output intact but makes that execution uncacheable.
Truncation, trailing bytes, checksum/identity corruption, read failure, and
write failure remain typed misses. Concurrent publishers may share an
identical output vector, while different outputs for one identity are rejected
instead of replacing the first immutable record.

Focused coverage proves cold miss/warm hit, deleted and stale two-output
restoration, package/build relocation through one shared cache, exact input,
environment, and resolved-tool invalidation, corrupt-record retirement and
republication, every truncated prefix, trailing bytes, payload damage, and
nondeterministic same-identity output rejection. Inherited and uncacheable
commands retain the old execution path and never report hits. Phase F remains
open for dependency-artifact producer-closure propagation and its public std
surface; the cache already reserves a separate identity component for those
inputs once the graph boundary supplies them.

A copied production `configured_success` package confirms the integrated cut.
The cold run took `119.939s`, including `108.052s` compiling the build runner
and `11.876s` executing the plan. The unchanged run reported two of two
build-action cache hits, restored both external-command outputs, and reduced
plan execution to `0.728s`. Its `39.030s` total was still dominated by
`38.294s` of runner compilation. This is consistent with the previously
recorded bottleneck: the external tool process is removed from the warm plan,
while build-runner compilation remains the main end-to-end cost.

All 119 ordinary `nia-build` tests pass with one cache-worker helper ignored.
Workspace all-target/all-feature check, strict Clippy, formatting, and diff
checks pass.

Phase F completion (2026-08-02, dependency-artifact propagation batch):
`std::build::CommandArgument::artifactInput` lets an external command consume a
declared executable artifact without spelling its build output as an untyped
path. The builder validates the `ExecutableHandle` owner, requires an existing
emit step, retains the typed handle, and automatically adds the producer edge.
Rust plan freeze independently requires every Artifact-root command program or
input to name the complete artifact and finds its matching compiler emit in the
step dependency closure, so malformed or hand-authored protocol bytes cannot
rely on execution order alone.

The coordinator resolves the typed input to the artifact's declared output.
The external-command cache snapshots those bytes separately from ordinary file
inputs; focused coverage proves a changed dependency artifact reports
`Dependencies` invalidation. A foreign-builder handle is rejected by the public
Nia API, and the configured production fixture proves the artifact is present
when the command runs, appears in the canonical plan input list, and adds the
emit producer to the tool step. Its isolated cold run took `123.074s`, including
`111.097s` compiling the runner and `11.966s` executing the plan. The unchanged
run reported two of two build-action cache hits and reduced plan execution to
`0.705s`; runner compilation remained the dominant `38.852s` of the `39.559s`
total.

Together with the compiler check/emit records, hermetic external-command
records, generated-file cache, declared resource scheduling, deterministic and
cross-process stress gates, and repeated warm-build evidence above, this closes
every Phase F task and acceptance item. The next build-system phase is Phase G;
the independent standard-library reconstruction track remains open and must
close before Phase G freezes its broader text/path-facing artifact APIs.

Standard-library reconstruction progress (2026-08-02, typed UTF-8 decode
batch): the optional `utf8_decode_first` API is physically replaced by
lower-camel `decodeUtf8First` returning `Utf8DecodeError!Utf8Decode`. Empty
input, truncation, invalid leading bytes, invalid continuation bytes, overlong
forms, and invalid Unicode scalar values are separate error values. Runtime
conformance covers a successful non-ASCII scalar and every error category.

`PathView::from_utf8_into` consumes the typed decoder and explicitly maps its
six causes into the current coarse `fs::Error::Invalid` boundary. It now clears
partial decoded output on Unicode or allocation failure, so callers never
observe a successful prefix as the result of a failed replacement. A real
filesystem-path executable proves valid ASCII/non-ASCII decoding, explicit
invalid-path mapping, and transactional cleanup. The stable std document now
publishes the current text/path/process role and error-flow matrix. This closes
the scalar decoder slice only: whole-buffer validated UTF-8, C-string errors,
OS path representation, borrowed-wrapper retirement, owned-text naming, and
the convenience-trait audit remain open.

Standard-library reconstruction progress (2026-08-02, typed C-string
validation batch): checked slice construction is now
`CStringView::fromBytes`, returning `CStringError!CStringView`. Zero-length
slices, missing trailing NUL bytes, and interior NUL bytes are distinct
failures. The
former optional `from_bytes` API is absent and has no compatibility alias;
`fromPtrUnchecked` remains the explicit trusted boundary for external pointers
whose allocation extent cannot be validated by the view. Runtime conformance covers
successful construction and all three error categories. This closes checked
C-string slice validation only: owned C-string storage, process command-owned
argument construction, the process facade's remaining legacy naming, and
OS-facing ownership remain open. The implementation deliberately uses ordinary
Nia slice iteration and pointer destructuring; the conformance program uses
`if value is !pattern` for a single success path and `switch` for exhaustive
error classification, so this batch records user-facing Nia idioms rather than
only exercising the underlying representation.

Standard-library reconstruction progress (2026-08-02, typed owned UTF-8
batch): `StringBuf::fromUtf8(allocator, bytes)` now performs a two-pass decode
and returns `TextError!StringBuf`. `TextError::InvalidUtf8` preserves the
specific `Utf8DecodeError`, while `TextError::Allocation` preserves the memory
failure. Empty bytes produce empty scalar text; non-empty truncated, leading,
continuation, overlong, and invalid-scalar sequences are rejected before an
owned result is returned. The generated runner now decodes target triple text
through `StringBuf` and reserves `PathView` decoding for actual filesystem
arguments, so ordinary text no longer borrows a path API. Runtime conformance
covers non-ASCII iteration, empty input, all five non-empty decoder failures,
fixed-buffer OOM, and nested `switch` error patterns. This closes the
whole-buffer owned UTF-8 conversion boundary only: incremental append,
formatting into scalar text, owned-text naming, and the common allocator
protocol remain open.

Standard-library reconstruction progress (2026-08-02, borrowed scalar-text
batch): the redundant `StringView` type, root export, constructors, accessors,
formatting implementation, and append compatibility path are physically
removed with no alias. `&[char]` is now the canonical borrowed scalar-text
representation across string formatting, build names and options, target
fields, command environment values, and run arguments. `PathView` remains
nominal because it carries path semantics rather than merely forwarding slice
operations; `StringBuf` and `PathBuf` continue to own copied storage when values
must outlive a call. The generated build runner, plan encoder, and production
configured-build fixture use the native slice APIs, including direct arrays of
borrowed literal arguments, without changing the serialized plan protocol.
Implementation and conformance code retain ordinary Nia `for` iteration,
`if value is pattern` for a single optional branch, and `switch` where the full
sum must be classified. This accepts the borrowed role only: the public
owned-text name, allocator protocol, mutation/format composition, and broader
text workflow remain active API exploration rather than frozen design.

This sequencing turns Nia's current experimental build bootstrap into a real
toolchain without discarding the valuable fact that build scripts are ordinary
Nia programs and can use a carefully layered standard library.
