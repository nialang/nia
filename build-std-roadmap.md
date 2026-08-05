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
- the former `StringBuf` is an owned `ArrayList[char]`, but allocation ownership
  is repeated at construction, every growth operation, extraction, and
  deinitialization;
- adjacent and multiline literals cover compile-time construction, while
  runtime concatenation is split between append and formatting without one
  documented choice;
- scalar UTF-8 decoding is typed, and owned text now has a whole-buffer
  `String::fromUtf8` boundary; empty input is valid text while non-empty
  truncation and invalid sequences retain their decoder cause;
- comparison, search, hashing, splitting, replacement, parsing, and formatting
  must work consistently across borrowed literals and owned text rather than
  requiring wrapper-specific adapters.

The redesign starts from user workflows, not from renaming the existing two
types. Its required decisions and acceptance order are:

1. make `&[char]` the canonical borrowed text unless a wrapper demonstrates an
   invariant beyond storage shape; expose exactly one public owned/mutable
   scalar-text type, now named `String`;
2. specify scalar length versus UTF-8 byte length, validation, truncation, and
   invalid-sequence errors. Validated UTF-8 views/buffers, if introduced, remain
   distinct from arbitrary bytes and from scalar text;
3. choose allocator ownership by role rather than applying one wrapper shape
   globally. Standalone collections may remain unmanaged when that avoids
   hidden allocator lifetimes, while an operation helper or aggregate owner may
   retain the allocator for the operation or complete object graph it owns;
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

### Language hardening track: Dual-stage Const Semantics

Goal: turn the dual-stage `const fn` model exposed by the Unicode scalar API
into a complete language contract before more standard-library APIs depend on
it. A `const fn` is one definition that is valid for compile-time execution and
may also be called through the ordinary runtime pipeline; it is neither a
const-eval-only function kind nor a promise checked only when one particular
constant happens to call it.

This track starts from the Unicode scalar batch during the standard-library
reconstruction track. Its declaration-contract and evaluator-resource work
must close before further const-capable std APIs freeze. Cross-stage,
incremental, and stress coverage may proceed beside the remaining text work,
but must close before Phase H.

Required semantic pipeline:

- the ordinary body/type checker is the sole owner of expression typing,
  traits and operator projections, aggregates, patterns, places, and control
  flow for both `fn` and `const fn` declarations;
- declaration validation checks every `const fn` without making it a comptime
  evaluation root or a runtime/backend root;
- comptime reachability is demand-driven from const-expression use sites and
  feeds `nia-const-ir`/`nia-const-eval`; runtime reachability is independently
  rooted from executable entry points and feeds the ordinary backend;
- const-specific checking is limited to capability, staging, evaluation
  budgets, and traps. It must consume shared typed semantic facts rather than
  maintaining a second expression type system.

Round 1, declaration contract and evaluator safety:

- validate every `const fn` body at its declaration, including tail and return
  types, expression statements, calls in unselected branches, generic bodies,
  receiver methods, and associated functions;
- run that validation through a const-declaration root filter over the ordinary
  body checker in facts-only mode: it must not follow runtime callees, lower
  BIR, or add executable reachability roots;
- reject ordinary runtime-only `fn` calls as a const-capability error without
  evaluating the body, while retaining legal data-dependent const failures such
  as an unselected `std::builtin::error` branch;
- keep signature, definition, value/type resolution, and visible-extension
  facts shared with the ordinary semantic pipeline instead of adding a
  name-based const call resolver;
- replace per-loop-only protection with one deterministic evaluation budget and
  a bounded const call stack shared across nested calls, loops, imports, and
  generic instantiations;
- report budget and depth exhaustion as source diagnostics with the active call
  site rather than allowing host stack exhaustion, process abort, or a hung
  compiler.

Round 2, cross-stage semantic and backend conformance:

- execute the same const-capable definitions at compile time and in emitted
  programs across arithmetic, casts, aggregates, `if ... is`, `switch`, `for`,
  methods, associated functions, imports, generics, and const generics;
- specify and test the defined cross-stage behavior of overflow, division,
  remainder, shifts, casts, traps, and other boundary operations instead of
  inheriting accidental evaluator/backend differences;
- cover const-only, runtime-only, dual-used, and unused definitions, including
  function references and generic instances, so const-only work never becomes
  a backend root and runtime work is never omitted or emitted twice;
- keep const execution in `nia-const-ir`/`nia-const-eval` and runtime execution
  in the ordinary checked-function/backend path while moving genuinely shared
  semantic rules to their existing common owner.

Round 3, query, incremental, and resource hardening:

- compare incremental and clean compilation while changing `fn` to/from
  `const fn`, changing use sites among const-only/runtime-only/dual-used, and
  editing imported const-capable bodies and generic targets;
- prove const values, diagnostics, reachable bodies, instances, backend roots,
  and cached signatures invalidate as one coherent dependency closure without
  stale stage-specific facts;
- add deterministic parallel stress for deep calls, recursion, nested loops,
  cross-module calls, and budget exhaustion, with bounded wall time and memory;
- record focused const-check/evaluation/query timing baselines so dual-stage
  support does not silently duplicate whole-program body checking.

Acceptance:

- an unused invalid `const fn` is rejected at its declaration, including wrong
  returns and ordinary function calls in expression statements or unselected
  branches, while a valid unused definition produces no runtime body or backend
  root;
- terminating recursive const evaluation succeeds within the documented
  implementation limits, while direct, mutual, imported, generic, and
  non-terminating recursion fail deterministically without exhausting the host;
- one maintained executable matrix observes equal results from compile-time and
  runtime calls over every accepted const control-flow and aggregate family;
- reachability and monomorphization matrices distinguish const-only,
  runtime-only, dual-used, referenced, and generic definitions without leaks,
  omissions, or duplicate instances;
- randomized incremental/clean comparison includes stage transitions and
  imported const edits, and strict workspace checks plus focused compiler and
  executable suites pass;
- architecture and language documentation state declaration validation,
  use-site staging, resource limits, and the ownership boundary between const
  checking, evaluation, ordinary body checking, reachability, and codegen.

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
         +-> dual-stage const hardening
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
| Const stage transition | clean/incremental agreement for values, diagnostics, runtime roots, and instances |
| Const resource exhaustion | deterministic depth/step diagnostic with bounded wall time and memory |
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
lower-camel `decodeUtf8First` returning
`Utf8DecodeError!DecodedUtf8Scalar`. Empty input, truncation, invalid leading
bytes, invalid continuation bytes, overlong forms, and invalid Unicode scalar
values are separate error values. Runtime conformance covers a successful
non-ASCII scalar and every error category.

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

Standard-library reconstruction progress (2026-08-02, transactional UTF-8
append batch): `StringBuf::appendUtf8(allocator, bytes)` validates and counts
the complete byte slice, checks the resulting scalar length, and reserves
capacity before changing visible text. Invalid UTF-8 and allocation failure
therefore leave the original buffer unchanged; an internal conditional `defer`
also truncates back to the original length if the already-validated second
decode cannot complete. Runtime conformance appends a non-ASCII scalar, rejects
an invalid sequence after a valid byte prefix without retaining that prefix,
and uses a bounded allocator to prove OOM rollback. The calling program uses
payload `switch` patterns for precise failure classification. This closes
transactional whole-buffer UTF-8 append only: formatter composition, the owned
text name, and the repeated allocator protocol remain open.

Standard-library reconstruction progress (2026-08-02, transactional format
append batch): `StringBuf::appendFormat(allocator, template, args)` formats into
an allocator-owned temporary byte list and commits only through
`appendUtf8`. `StringBuf` therefore does not pretend to be an arbitrary byte
writer, and a custom `Format` implementation that emits invalid UTF-8 cannot
violate scalar-text storage. The format-specific error preserves checked-template
errors; a writer side channel recovers the concrete memory error otherwise
collapsed into `fmt::Error::Write`; invalid formatted bytes remain a typed UTF-8
failure. Temporary-buffer release is an explicit pre-commit step; an
infallible conditional `defer` restores the old scalar length on release
failure. This avoids assuming that a second defer will run after propagating an
error from a cleanup defer. Runtime conformance proves successful numeric and
non-ASCII composition, rollback after partial invalid-template output, invalid
byte output, bounded-buffer OOM, and release failure after successful append.
The maintained calling form uses
`using std::fmt` plus one `[N]&fmt::Format` array annotation, exercising Nia's
trait-object coercion without per-element casts. This closes transactional
format append only: owned-text naming, comparison/search/hash composition, and
the repeated allocator protocol remain open.

Standard-library reconstruction progress (2026-08-02, scalar-text relation
batch): borrowed `[char]` and owned `StringBuf` now share the allocation-free
`equals`, `startsWith`, `endsWith`, `find`, and `contains` vocabulary. The
borrowed slice owns the implementation and `StringBuf` delegates through its
scalar view, so literals, borrowed parameters, and owned plan storage do not
need wrapper adapters or separate algorithms. `find` returns the first scalar
index as `?usize`; empty text and empty needles have explicit prefix, suffix,
search, and containment behavior. Maintained build name, target, environment,
and plan comparisons now consume this public text API instead of reaching into
the generic memory helper. Runtime conformance covers non-ASCII scalars, empty
needles, absent results, overlapping matches, owned mutation, `if value is
?index`, and ordinary `for` iteration. Content equality remains an explicit
operation rather than overloading reference equality. This closes equality and
substring search only: ordering, hash/provider composition, owned-text naming,
and the repeated allocator protocol remain open.

Standard-library reconstruction progress (2026-08-02, scalar-text hash batch):
`[char]` now implements `Hash[H]` by committing scalar count followed by each
scalar value, and the demand-loaded `string/hash` provider delegates
`StringBuf` hashing to that borrowed representation. `StringBuf` content
`Eq[StringBuf]` completes the existing `DefaultHashMapContext` bounds, so two
independently allocated equal keys hash and compare identically. Runtime
conformance checks the scalar stream against a manually driven `Wyhash`, checks
borrowed/owned identity, exercises `==` and `!=`, and retrieves a default map
entry through a separately allocated equal key. It also found that implementing
`ne` as `not self.eq(other)` re-entered method/provider resolution and produced
incorrect runtime behavior; both provider methods now delegate directly to the
unambiguous content operation.

This accepts hash and owned-equality provider semantics, not the surrounding
collection ownership workflow. `HashMap[StringBuf, V]` still requires an owned
lookup key instead of a borrowed `[char]`, and collection teardown does not
deeply deinitialize allocating keys. The conformance program must remove and
explicitly release its stored key. Borrowed-key lookup, rejected/replaced key
ownership, and element cleanup now join owned-text naming and the repeated
allocator protocol as concrete follow-up design work.

Standard-library reconstruction progress (2026-08-02, owned-text naming
batch): the sole owned/mutable scalar-text type is now `String`; the bootstrap
name `StringBuf` is physically absent from std and conformance code, with no
type alias. `Buf` suggested an arbitrary byte writer even though formatting and
UTF-8 append deliberately validate into scalar storage, while `String` states
the role opposite canonical borrowed `&[char]` without introducing another
wrapper. Root and module paths are `std::String` and `std::string::String`.

Accepted String methods now use lower camel case: `initCapacity`,
`fromOwnedSlice`, `fromSlice`, `textMut`, `isEmpty`, `ensureTotalCapacity`, and
`intoOwnedSlice`. `text()` remains the sole read-only borrowed view; the
duplicate `as_slice()` and `clear_retaining_capacity()` surfaces are removed.
The owned Path conversion is `PathBuf::fromString`, and its unused
`string_buf()` accessor is removed rather than renamed. Runtime conformance
repeats UTF-8, transactional formatting, relation, hash, default-map, failure,
and cleanup workflows through `String`. This closes the owned-text name and
method spelling only: explicit allocator repetition, owned collection element
cleanup, and borrowed-key lookup remain open and are not hidden inside the
rename.

Standard-library reconstruction progress (2026-08-02, borrowed map lookup
batch): `HashMapLookupContext[K, Q]` separates the stored key from a lightweight
query view. Every existing `HashMapContext[K]` automatically supplies the
same-key `Q = &K` provider, preserving custom contexts and the expected type
context of calls such as `get(&{})`. New lower-camel query-view operations are
`containsKeyBy`, `getBy`, `getMutBy`, `getEntryBy`, `getEntryMutBy`, `getKeyBy`,
and `removeEntryBy`. The default context's demand-loaded String provider hashes
and compares `&[char]` with exactly the same scalar stream as the stored
`String`, so lookup and removal allocate no temporary text.

The design exploration rejected two superficially simpler forms. Unsized
`[char]` is not legal as a trait generic argument even when methods only take a
pointer, so `Q` represents the pointer view `&[char]`. Replacing existing
`get/remove` signatures with method-level generics removed their `&K` expected
type and made `&{}` infer as `&void`; distinct `*By` methods retain old
inference. Slice variables infer `Q` directly. A literal uses the maintained
one-annotation spelling `map.getBy[&[char]](&"name")` because current provider
impl patterns cannot match `&[N]char` in a trait argument. Runtime conformance
exercises immutable/mutable value, entry, stored-key, absent-literal, and owned
key removal paths. `removeEntryBy` returns the key for explicit cleanup. Map
teardown and rejected/replaced incoming-key ownership remain open alongside the
common allocator protocol.

Standard-library reconstruction progress (2026-08-02, owned map insertion
batch): reviewed `insert`/`insertAssumeCapacity` operations now return an
optional `HashMapReplacement`. Inserting a new key returns `null`, while
replacing the value for an equal stored key retains the stored key and returns
both the rejected incoming key and the replaced stored value.
`insertIfAbsent` and its assume-capacity form return an optional complete
incoming entry when no insertion occurs. The former `put`, `fetch_put`,
`put_if_absent`, and assume-capacity spellings are physically absent rather than
kept as ownership-losing compatibility paths.
The adjacent owned-entry methods are now `intoKey`/`intoValue`, and mutable
entry access is `valueMut`; the former snake-case spellings have no aliases.

The result design deliberately uses Nia's native optional rather than a
single-field result wrapper or a tag enum plus generic union. The latter would
freeze the temporary enum-plus-union emulation prohibited by the language
decision gate, while the wrapper would add no invariant beyond the optional.
The resulting `if result is ?replacement` spelling is the native single-branch
workflow. Runtime conformance uses independently allocated equal `String` keys
and proves replacement and
if-absent rejection both return the incoming allocation for explicit release;
the stored key remains available for borrowed lookup and later removal.
Fallible calls retain the established transparent transfer rule: allocating
named inputs remain caller-owned until success and use conditional `defer`
rollback. Deep map element cleanup, the ownership-losing `get_or_put` entry
family, and the common allocator protocol remain open.

Standard-library reconstruction progress (2026-08-02, owned map entry batch):
the ownership-losing `get_or_put_value` family is replaced by lower-camel
`getOrInsert` and `getOrInsertAssumeCapacity`. `HashMapGetOrInsertResult`
contains references to the stored key and mutable value plus an optional
rejected incoming entry. When the key is new, the supplied value is initialized
before the result becomes visible. When an equal key exists,
`intoRejected()` returns both incoming values and the stored value is unchanged.
The no-value `get_or_put` operations are physically removed because exposing an
uninitialized `V` is not an ordinary safe collection workflow.

This result wrapper has a real invariant unlike the rejected single-optional
insertion wrapper explored in the preceding batch: it composes a stored entry
view with conditionally returned ownership. Focused conformance covers new and
existing entries, assume-capacity insertion, allocation rollback, randomized
model churn, and independently allocated equal `String` keys whose rejected
allocation is explicitly released. Lazy value construction remains a future
entry-API design rather than preserving uninitialized storage as convenience.
Deep element cleanup and the common allocator protocol remain open.

Standard-library reconstruction progress (2026-08-02, owned map drain batch):
`HashMap::drain()` and `HashMapDrainWithContext` provide linear bulk ownership
transfer without storing an allocator or inventing automatic drop. Each
iterator step copies out one `HashMapEntry`, marks that bucket deleted, and
updates the live length. Early termination therefore leaves only unvisited
entries owned by the map. Full exhaustion restores the empty control table and
growth budget so retained capacity is immediately reusable.

Runtime conformance drains primitive entries, verifies count/value totals and
capacity reuse, then inserts again through the assume-capacity path. Owned-text
conformance drains independently allocated `String` keys through ordinary
`for`, transfers each key with `intoKey()`, and explicitly deinitializes it
before map storage teardown. This accepts linear explicit element extraction,
not a hidden destructor protocol: cleanup error aggregation and the common
allocator protocol remain open.

The same batch completes the HashMap lower-camel migration across public APIs,
fields, parameters, locals, and package providers. There are no compatibility
aliases: duplicate `clearRetainingCapacity`, `clearAndFree`, `reserveExact`,
`fetchRemove`, and `getKeyValue` surfaces are physically removed, while
`ensureUnusedCapacity` becomes a private implementation helper behind
`reserve`. Contextual decimal literals omit `usize`; direct place-driven
updates such as `self.len += 1` compile without annotation, and linear bucket
scans use `for index in 0..capacity` so the range endpoint supplies the index
type. HashMap provider modules import `pkg::iter::range` directly, so those
loops remain valid through the narrow `std::collections` facade instead of
depending on the root facade to activate Range's `Iterable` implementation.
Algorithm-width `u8`/`u64` literals remain explicit.

Standard-library reconstruction progress (2026-08-02, initialized array-list
batch): `ArrayList` now uses lower-camel names across its public surface,
provider fields/locals, build, string, process, examples, and conformance.
Duplicate `clearRetainingCapacity`, `clearAndFree`, and `extendFromSlice`
operations are physically removed. Public `addOne`, `addManyAsSlice`,
`addManyAt`, arbitrary `resize`, `expandToCapacity`, `allocatedSlice`, and
`unusedCapacitySlice` operations are also removed because they represented
uninitialized storage as live `T` values. Their uninitialized mechanics remain
private implementation details behind operations that initialize every new
element before returning.

`truncate` is the sole length-discarding command. `shrinkToFit` and
`shrinkToCapacity` preserve every element and only reduce allocation; callers
use `clear` explicitly before releasing empty retained capacity. `reserveExact`
is retained as a real non-geometric allocation contract, while duplicate
ensure-unused and precise-total helpers become private. ZST, owned-slice,
aliasing insert/replace, assume-capacity, post-shrink reuse, and range
conformance all pass through the initialized API.

This batch also exposed and fixes asymmetric binary literal inference:
`64 / elemLen` formerly defaulted the unsuffixed left operand to `i32` despite
the concrete `usize` peer. Body checking now lets a concrete right operand
drive an unsuffixed left numeric literal for arithmetic, bitwise, comparison,
and equality operators when the peer is numeric; focused regression coverage
includes division, comparison, equality, non-numeric peers, and explicit
suffixes. This is a compiler correction discovered by std idiom cleanup, not a
reason to restore redundant suffixes.

Standard-library reconstruction progress (2026-08-02, typed path ownership
batch): `PathView` remains the nominal borrowed scalar-path role and `PathBuf`
owns its scalar storage. `PathBuf::fromUtf8` now constructs the owned path
directly and preserves `TextError`; the former public `from_utf8_into` scratch
ArrayList protocol and its collapsed `fs::Error::Invalid` result are physically
absent. Pure owned copy, capacity, mutation, join, and release operations report
`mem::Error` rather than pretending allocation is a filesystem failure.

The public path surface is lower camel with no aliases. Duplicate
`to_path_buf`/`from_path`, `join`/`join_component`, and
`encode`/`encode_bytes` pairs are reduced to `fromView`, `joinComponent`, and
one typed `encode`. Encoding distinguishes `PathError::ContainsNul` from
`PathError::TooLong`, and `EncodedPath::init` is private so checked encoding is
the only ordinary construction path. `joinComponent` reserves its complete
mutation before writing a separator, so OOM preserves the original path.
Runtime conformance covers non-ASCII UTF-8 ownership, precise decode and encode
errors, the terminating-NUL view, and join rollback under a bounded allocator.

Provider encoding imports `pkg::slice` explicitly so the narrow fs facade does
not rely on root-facade activation. Borrowed-slice `Iterable` providers now let
encoding use ordinary `for &item in slice` loops. This is an implementation for
the expression's actual `&[T]` type, not a compiler auto-dereference exception.

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

Standard-library reconstruction progress (2026-08-02, borrowed-slice iteration
batch): `&[T]` and `&mut [T]` now implement `Iterable` directly. Both yield
`&T` because `Iterable::iter` receives a shared borrow of the iterable;
mutation remains explicit through `iterMut()` and `SliceIterMut`. Path and
C-string providers plus maintained examples use direct `for &value in slice`
or direct collection iteration instead of ceremonial `.iter()` calls.

The slice/iterator surface is lower camel with no compatibility aliases:
`isEmpty`, `iterMut`, `nextBack`, `forwardChecked`, `backwardChecked`, and
`fromBounds` replace the old spellings. Unused public slice-iterator
constructors are removed, raw-pointer constructors and range provider
constructors are private, and one generic `DoubleEndedIterator::rev()`
extension replaces four per-type copies. Context-driven counters and steps no
longer carry redundant numeric suffixes.

Body-check conformance proves provider implementations on both pointer-slice
types guide `for &value` bindings. A narrow `std::slice` executable covers
borrowed parameters, a temporary sub-slice, read-only direct iteration of a
mutable slice, explicit reverse mutable iteration, and iterator length state.
The language specification now records the actual two-stage contract:
`Iterable` supplies `Iter`, which must implement `Iterator` with the same item
type; iterator values satisfy `Iterable` intrinsically. No general automatic
dereference rule was added.

Full configured-build validation also removed two stale generated-runner
dependencies left behind by the owned text and path batches. Target fields now
use `string::String`; path arguments own `fs::PathBuf` values constructed by
`fromUtf8`, with no `StringBuf`, `ArrayList[char]` path scratch storage, or
`from_utf8_into`. Generated argument indices and offsets rely on their `usize`
parameter context instead of suffixing every literal. The exercise exposed an
over-broad error union: UTF-8 construction could never produce the former
format variant but exhaustive `switch` still had to classify it. `TextError`
now contains only `InvalidUtf8` and `Allocation`, while `appendFormat` returns
the distinct `TextFormatError` that adds `Format`.

Test-infrastructure repair (2026-08-02): the WSL failure was not evidence of a
release-compiler regression. A bounded `dependency_cycle` runner performed the
same 27,073 compiler queries in every profile, but took `86.1s` through the
former unoptimized test binary, `16.24s` with the repaired `opt-level = 1` test
profile, and `13.9s` through release. Earlier `90-108s` production-path cache
samples are therefore integration correctness timings, not release trend
points.

The audit also found `target/debug` at roughly `192 GiB`, with 200-320 obsolete
hashed variants for several compiler crates, plus CLI scratch workspaces whose
compiler caches survived on the WSL `/tmp` tmpfs. Dev and test profiles now use
line-table debug information, shared test-directory guards remove complete
scratch trees on drop, and the 14 build fixtures are independent libtest cases.
Runner-only fixtures reserve one compiler slot while the three nested-compiler
fixtures retain the conservative build weight. A fixture-index test prevents a
new directory from silently falling back into an unowned serial loop.
The repaired gate passes all 14 fixtures plus that index test in `159.31s` on
the same 8 GiB WSL view, down from the former `1291.21s` serial unoptimized run.

Standard-library reconstruction progress (2026-08-02, checked slice access
batch): Nia's direct indexing and slicing syntax deliberately performs no
runtime bounds checks, so std now owns the ordinary data-dependent path through
optional `get`, `getMut`, `first`, `firstMut`, `last`, and `lastMut` methods.
`getRange` and `getRangeMut` validate half-open bounds, accept `len, len` as an
empty range, and reject reversed or overlong ranges before using the native
slice operation. There are no duplicate unchecked methods: `slice[index]` and
`&slice[start..end]` already spell that precondition-bearing primitive.

The maintained narrow-facade executable uses direct `for`, `if access is
?value` for a single read-only branch, and exhaustive `switch` with
`mut ?value` for mutable references. It covers in-range and out-of-range
elements, empty and non-empty endpoints, valid empty/middle ranges, reversed
ranges, overlong ranges, and mutation through a checked subrange. This audit
also found that `ArrayList::asMutSlice` exposed allocation capacity rather than
logical length, representing uninitialized slots as live `T` values despite
the initialized-value contract. It now returns only `0..len`; list checked
access delegates to the slice implementation, and focused ArrayList
conformance asserts the mutable view length.

Rechecking the scalar-text workflow after slice provider invalidation exposed a
contextual-inference bug that an older persistent result had hidden:
`String.find` returns `?usize`, but its ordinary `return ?0` defaulted the
wrapped literal to `i32`. Optional construction now normalizes the expected
wrapper, checks the payload against its `T`, and materializes contextual numeric
literals before forming `?T`. Error-union success and failure constructors
normalize their expected wrappers through the same boundary. Focused body
coverage proves `?0` under `?usize`, `!1` under `i32!u8`, and `2!` under
`u8!i32`; no redundant suffix was added to std.

Standard-library reconstruction progress (2026-08-03, slice sequence relation
batch): ordered slice comparison and contiguous search now live once on
`[T] where T: Sized + Eq[T]`. `equals`, `startsWith`, `endsWith`, `find`, and
`contains` cover arbitrary comparable element types; `find` returns the first
element index, empty needles match at zero, and prefix/suffix/search never
allocate. The `[char]` copy of these algorithms is physically removed, owned
`String` continues to delegate through its borrowed slice, and the duplicate
public `mem::equal` helper is absent rather than retained as an alias. Existing
memory and collection conformance now uses the user-facing `slice.equals(...)`
spelling.

The generic conformance uses a user-defined `Eq` implementation and direct
`for index in 0..len`/`0..=lastStart` iteration, while single optional branches
use `if result is ?index`. Moving real owned-slice workflows onto the method
surface exposed that `&mut [T]` could not call a read-only pointee extension
even though mutable-to-read-only slice coercion already worked at typed
boundaries. Method candidate matching now considers that coercion before
rejecting the provider; focused body coverage and the ArrayList ownership path
both keep the direct `owned.equals(...)` form. The optional/error tutorial also
uses `slice.get` instead of reimplementing bounds checks. Removing the final
test-only `std::mem` import also exposed `fs::path` relying on `mem` to activate
checked arithmetic transitively; the path provider now imports `pkg::math`
directly.

Standard-library reconstruction progress (2026-08-03, slice copy and ordering
batch): ordinary contiguous copying is now the overlap-safe
`destination.copyFrom(source) -> usize` operation. It copies the common prefix,
so a short destination is represented by the returned count rather than hidden
behind the former `void` helpers. Repository Nia cannot discard that result
implicitly: maintained internals use `_ =` only where their constructed ranges
already prove an exact copy, while the slice tutorial returns the count to its
caller. Runtime conformance covers exact, long-destination, short-destination,
left/right overlap, and zero-sized element paths.

Lexicographic ordering is `slice.compare(other)` for `T: Ord[T]`, returning the
new closed `std::cmp::Ordering` type. Comparison is no longer presented as a
memory operation. The obsolete `mem::copy_forwards`, `mem::copy_backwards`,
`mem::order`, `mem::Order`, and the entire `mem::copy` provider are physically
removed without aliases. ArrayList mutation/allocation, allocator remap, I/O
buffers, Wyhash staging, and maintained examples all use the slice surface.
The language specification now also records the compiler-backed distinction:
`memcpy` is forward and has an overlap precondition, `memmove` is overlap-safe,
and both are shallow representation operations over the common slice prefix.

Like pointer-slice types, `&ArrayList[T]` and `&mut ArrayList[T]` implement
`Iterable` directly so borrowed collection parameters retain the ordinary
`for item in values` spelling. Both providers yield `&T`; mutable element
iteration remains the explicit `iterMut()` operation because `Iterable::iter`
receives a shared borrow. This also keeps generic formatting independent of an
implicit compiler dereference rule.

The borrowed provider exposed an LLVM call-lowering bug rather than a reason to
retreat to explicit `.iter()`: a method whose extension target was already a
pointer lost the additional borrow required by its `&self` receiver. Pointer
receivers are now passed through directly only when their type actually matches
the ABI parameter (including the outer mutable-to-read-only coercion); nested
pointer receiver methods take the required address. LLVM-level and executable
coverage preserve that distinction.

Standard-library reconstruction progress (2026-08-03, process view naming
batch): borrowed process arguments and environment entries retain their
zero-copy `CStringView`/byte-slice representation, but their public surface is
now lower camel: `rawPtr`, `isEmpty`, `skipProgram`, `rawArgv`, and `rawEnvp`.
The old snake-case spellings are physically absent with no aliases. This
accepts naming for borrowed host views only; command-owned argument
construction, spawn/wait errors, OS-facing ownership, and the broader process
command API remain open.

Standard-library reconstruction progress (2026-08-03, typed process command
batch): `Command` now borrows `PathView`, scalar `&[&[char]]` arguments, and an
`Env` view instead of exposing `CStringView`, `&&u8`, and caller-built argv.
The executable path is argv[0]; spawn-time lowering validates path/NUL rules,
UTF-8 encodes arguments into temporary storage, builds the native pointer
array, and frees it before returning. Ordinary `spawn`/`run` use page-backed
temporary allocation, while `spawnWithAllocator`/`runWithAllocator` retain an
explicit bounded/failure-injection path. Runtime coverage proves non-ASCII
arguments, embedded-NUL rejection before exec, allocator failure, cwd, stdio
pipes, failed-spawn cleanup, wait caching, and signals.

Command configuration uses immutable `withArguments`, `withStdin`,
`withStdout`, `withStderr`, and `withCwd` values. Duplicate free `spawn`/`run`,
the raw `Command::init` contract, public raw wait-status helpers, and the
incomplete `run_path_args` bridge are physically absent. `spawnRaw` remains the
single explicit raw process boundary. The surrounding child/term API is lower
camel, with `succeeded`, `exitCode`, `signalCode`, `takeStdin`, `tryWait`, and
`killWith`. Typed spawn/wait error payloads, custom environment ownership, and
the lower-level public `std::os` naming/ownership audit remain open.

Standard-library reconstruction progress (2026-08-03, typed process error
batch): the former flat open-integer `process::Error` is physically replaced by
a closed payload enum. Command lowering preserves `mem::Error` and
`fs::PathError`; embedded NUL reports the zero-based `withArguments` element;
native spawn preserves the exact open `os::SpawnError`. Child wait, try-wait,
and kill failures retain their `os::Error`, while pipe close failures retain
both `StdStream` identity and the OS cause. The former flat `OutOfMemory`,
`Invalid`, `SpawnStdio`, `SpawnCwd`, `SpawnExec`, operation-only child errors,
and catch-all `System` values have no aliases; `SpawnSetup` now requires its OS
cause rather than naming an integer category.

`process::Error.asExitCode` now derives its result from the retained payload;
path-too-long and spawn-stage values therefore remain distinguishable from
ordinary invalid input and I/O. `std::build.ErrorCause::Process` uses the same
mapping and formats nested operation/cause paths such as
`process/spawn/executable`, `process/kill/invalid`, and
`process/close/stdout/bad file descriptor`. Runtime conformance exercises path
encoding, a nonzero argument index, allocation failure, exact exec/cwd stages,
the `if error is pattern` spelling, and an exact invalid-signal kill cause.
Custom environment ownership and the lower-level public `std::os`
naming/ownership audit remain open.

Standard-library reconstruction progress (2026-08-03, typed process
environment batch): `Command` now models startup-environment inheritance,
exact replacement through borrowed scalar `EnvEntry` values, and an explicit
empty environment as three distinct states. `withEnvironment` never silently
merges with inherited entries, while `withoutEnvironment` communicates the
empty intent directly. The former `env()` getter is physically absent because
a single `Env` result cannot truthfully represent all three states; there is no
compatibility facade.

Spawn validates exact entries before transient allocation: names must be
non-empty and contain neither `=` nor NUL, values cannot contain NUL, and names
must be unique. `process::Error::Environment` retains the failing entry index;
`EnvEntryError::DuplicateName` also retains the first matching index. Build
diagnostics preserve those paths, and process exit conversion classifies all
environment validation failures as invalid input. Spawn owns temporary UTF-8
`NAME=value\0` storage and the envp pointer array, so public callers borrow
scalar text without constructing native buffers or transferring allocator
ownership.

Runtime conformance proves non-ASCII values, exact replacement rather than
inheritance merging, explicit empty envp, all validation causes, duplicate
indices, and bounded-allocation failure. The example uses direct borrowed
arrays and immutable command configuration. Lower-level public `std::os`
naming/ownership and the broader UTF-8/C-string role audit remain open.

Standard-library reconstruction progress (2026-08-03, OS provider boundary
batch): page mapping, raw filesystem operations, random data, spawn/wait,
signals, process termination, directory parsing, metadata conversion, wait
status, and spawn configuration are now package-private `std::os` capabilities.
Cross-facade provider names use lower camel (`maxPathBytes`, `pageSize`,
`openAt`, `mapPages`, `spawnRaw`, `tryWait`, and `killWith`) without retaining
the snake-case spellings. A negative compiler test proves an ordinary package
cannot call the provider surface.

At this batch point `std::os` remained public because service signatures still
named `Error`, `SpawnError`, opaque `FileHandle`, and opaque `ProcessId`. The
raw handle escape supported child-pipe adaptation through `readSome`,
`writeSome`, and `close`; process identity exposed `raw`. Filesystem `borrow_handle` and
`take_handle` are physically removed from the user surface and are package-only
`borrowHandle`/`takeHandle`; close-state conformance now observes `BadFd`
through ordinary high-level file/directory operations. `File::setPermissions`
and `syncData` replace their old spellings with no aliases.

This was a visibility boundary, not acceptance of raw handles as the long-term
I/O design. Typed child stdin/stdout/stderr roles and an I/O error model were
therefore the next ownership boundary. Linux syscall/backend naming remains
package-internal audit work.
The build-host source-closure snapshot is also reconciled with the already
reviewed plan, ordering, text hashing, and removed memory-copy providers: it now
records the actual 95-module closure instead of a stale 93-module list.

Production-path validation exposed a compiler bug rather than a reason to
re-publicize `os::exit`: full module DCE classified every non-`pub` function as
private, so a `pub(pkg)` definition with only cross-module callers disappeared
from its object while another object retained the call. The backend now removes
only `Visibility::Private` functions and instances. A focused O2 regression
keeps an otherwise locally unused package-visible provider, and the complete
configured-build path proves the startup/provider call survives multi-object
code generation.

Standard-library reconstruction progress (2026-08-03, typed child-pipe and I/O
boundary batch): `Child::takeStdin`, `takeStdout`, and `takeStderr` now transfer
distinct `ChildStdin`, `ChildStdout`, and `ChildStderr` owners rather than raw OS
handles. Stdin implements `Writer`; stdout and stderr implement `Reader`.
`buffered(buffer)` composes each role with the generic buffered adapter without
requiring users to repeat its type argument. Pipe owners invalidate themselves
before close, make repeated close idempotent, report later access as
`io::Error::Closed`, and remain caller-owned across child wait. Runtime and
compile-fail conformance covers all three roles, EOF, direct and buffered I/O,
repeated close, and rejecting reads from stdin or writes to stdout.

The public runtime `Io` trait and `BlockingIo` value are physically removed:
they had no alternate implementation and forced every file or standard-stream
adapter through `init.io()` while exposing `os::FileHandle`. `process::Init`
now carries only startup arguments and environment. File readers/writers take
only caller-provided storage and call the package-private provider directly.
The raw `FileHandle` type and its operations are package-private, with a
negative ordinary-package guard. Reviewed Reader/Writer methods are lower camel
(`readExact`, `writeAll`, `writeByte`, `endOfStream`, `shortWrite`, and
`discardBuffered`) with no old aliases. Child pipe failures use the closed high-level `io::Error` plus an
open `io::SystemError` cause; process cleanup retains stream identity around
that error. Process identity and spawn/wait/kill causes still name OS types and
remain the next process-provider boundary.

Standard-library reconstruction progress (2026-08-03, process-owned identity
and spawn-cause batch): `Child::pid` now returns `process::ProcessId`; the OS
identity wrapper is package-private and ordinary packages cannot name it.
Process state converts through the private provider only at wait, try-wait,
and signal boundaries. The process-owned identity retains the deliberately
narrow `raw()` observation without exposing provider representation.

`process::Error` no longer names OS types. Spawn setup, wait, try-wait, and
kill retain `process::SystemError`; native spawn uses the closed process-owned
`SpawnError::{Setup, Stdio, Cwd, Exec}(process::SystemError)` surface. The Linux
error-pipe record already carried both stage and errno, but the former integer
spawn enum discarded errno when decoding a child failure. Native and OS
provider errors now retain both fields through the conversion chain. Exact
exec and cwd failures can therefore use Nia's payload patterns while build
diagnostics report paths such as `process/spawn/executable/not found`.
Process exit conversion follows the retained system cause rather than an
invented stage number. No compatibility variants or OS aliases were added.
The remaining public OS error escape is hash-map random seeding and is a
separate collection initialization boundary.

Standard-library reconstruction progress (2026-08-03, HashMap initialization
and private OS root batch): ordinary empty maps now use the direct
`HashMap::init()` and `initContext(context)` constructors. They obtain a
randomized seed internally and treat failure to establish the default hashing
policy as unrecoverable. Callers that can recover use `tryInit()` or
`tryInitContext(context)` and the collection-owned `HashMapInitError`;
deterministic tests and algorithms retain the explicit `initSeed` and
`initContextSeed` forms. The maintained example now uses the ordinary direct
constructor instead of teaching users to invent a numeric seed.

With collection initialization no longer returning `os::Error`, the root OS
module, its generic error, spawn error, process identity, handles, and all
provider operations are package-private. Ordinary packages cannot import
`std::os`; typed `fs`, `io`, `process`, `mem`, and collection services remain
the only host boundaries. No public OS alias or compatibility constructor was
retained.

Standard-library reconstruction progress (2026-08-03, allocator surface and
arena reset batch): the public memory contract now uses lower-camel
`allocBytes`, `allocSlice`, `freeSlice`, `isEmpty`, `asSlice`, and
`deinitWithoutLeakCheck`. Arena and general-purpose capacity observations are
`capacity()` and `used()` without query-prefixed spellings. The former
snake-case names have no aliases, and compile-fail conformance proves they are
absent.

Arena reset semantics are reduced to one ordinary workflow: `reset()`
invalidates all arena-backed blocks, slices, and containers while retaining
capacity for reuse; `deinit()` invalidates them and releases the backing
storage. The duplicate retain-reset spelling and the unused limit-retention
policy are physically removed. Fixed-buffer ownership and last-allocation
probes are private mechanics rather than public capabilities.

The implementation audit also moves fields, locals, and helper operations to
lower camel. Contextual numeric literals omit `usize`; accumulators state their
type at the declaration when no expression supplies it, and small-page slot
initialization uses direct `for index in 0..slotCount` iteration. Runtime
conformance covers typed slices, realloc, fixed buffers, retained arena reset,
full arena release, GPA small/large allocation, invalid frees, and leak status.
The build ownership fixture now follows the already accepted `PathBuf::fromView`
boundary: allocation and release failures remain `ErrorCause::Memory` rather
than being relabeled as filesystem errors. Executable finalization-count
fixtures are reconciled with the smaller provider closure after obsolete arena
methods are removed.
This accepts the low-level memory vocabulary and reset behavior only: the
repeated allocator argument across owned collections and text remains the
common allocator protocol design problem, not compatibility debt hidden by
this naming pass.

Standard-library reconstruction progress (2026-08-03, allocator ownership
protocol batch): allocator storage is now decided by ownership suitability,
not by a library-wide managed/unmanaged switch. Standalone `ArrayList`,
`HashMap`, and `String` values remain unmanaged. They do not retain a mutable
allocator reference or pay for one in every empty or nested value. `Build`
retains its allocator because it owns the complete plan object graph;
operation-scoped formatting and process-lowering helpers may likewise retain
one for the operation they own. A future managed representation requires a
concrete ownership role and workload rather than symmetry with unmanaged
collections.

The provenance rule is explicit: every remap, extraction, and release of a
current non-empty backing allocation uses the allocator that produced it.
`fromOwnedSlice` adopts the caller's provenance and `intoOwnedSlice` preserves
it; `toOwnedSlice` and `clone` may use a different target allocator because
they create independent storage. Conformance uses two general-purpose
allocators to prove that an owned copy belongs to its target, a transfer stays
with its source, and a rejected wrong-allocator release leaves the list
available for correct cleanup. Allocators without ownership diagnostics do not
weaken the caller's contract.

Allocator repetition is removed where capacity makes allocation impossible,
not by hiding allocator state. `ArrayList` and `HashMap` already compose
`reserve` with assume-capacity mutation. `String` now has `reserve`,
`pushAssumeCapacity`, and `appendAssumeCapacity` for the same batch workflow.
Ordinary potentially allocating methods keep the allocator immediately after
the receiver, while reads and capacity-preserving operations remain allocator-
free. `PathBuf::fromOwnedSlice` is physically removed: path ownership transfers
through `fromString`, and raw scalar-slice adoption remains at the underlying
text/collection boundary. The accepted protocol still leaves deep element
cleanup and type-specific ownership designs as separate work; it does not
declare that every std type must be unmanaged.

Standard-library reconstruction progress (2026-08-03, borrowed split batch):
contiguous sequence splitting now belongs to `[T] where T: Eq[T]`, beside the
existing equality and search operations. `split(separator)` returns the
allocation-free `SliceSplit[T]` iterator and `String::split` delegates to its
borrowed scalar slice, so ordinary code uses `for part in
text.split(separator)` without constructing temporary owned strings or naming
an allocator. The maintained slice/string conformance and slice example use
direct `for`, if-pattern/search composition, and contextual numeric inference.

Matching is left-to-right and non-overlapping. Leading, trailing, and adjacent
separators retain empty segments; empty input retains one empty segment, and an
empty separator yields the original slice once because direct element
iteration already covers that workflow. Source and separator storage must stay
valid and unchanged for the iterator lifetime. `SliceSplit` deliberately does
not implement `DoubleEndedIterator`: a self-overlapping multi-element
separator showed that a naive final-match scan disagrees with forward
boundaries, while recomputing those boundaries for every `nextBack` is
quadratic. Reverse splitting remains unaccepted until a searcher or retained-
boundary design carries that contract honestly.

Standard-library reconstruction progress (2026-08-03, scalar replacement
batch): borrowed `[char]` and owned `String` now expose
`replaceAll(allocator, needle, replacement)`. The operation always creates an
independent `String`; it does not mutate or consume its receiver. It shares
`split`'s left-to-right, non-overlapping matching model, treats an empty needle
as a request for an independent unchanged copy, accepts replacement text that
borrows from the source, and computes the exact scalar capacity before its
single allocation. Fixed-buffer conformance proves that allocation failure
returns no partial text and leaves the source unchanged.

The direct Nia spelling `(&"aba").replaceAll(...)` exposed a compiler gap:
typed parameter boundaries already coerced `&[N]T` to `&[T]`, while extension
method lookup did not. Method resolution now uses that same coercion as a
fallback for ordinary and explicit-generic methods, including mutable array
pointers. A method defined directly for the fixed-length array retains
priority, and the normal expected-type path records the coercion for BIR and
code generation. This removes the need for a duplicate free-function text API.

Standard-library reconstruction progress (2026-08-03, borrowed text join
batch): `&[&[char]]` now provides
`join(allocator, separator) -> mem::Error!String`. It accepts literals,
sub-slices, and `String::text()` views without a nominal adapter, scans the
repeatable borrowed slice to compute exact capacity, then fills one allocation.
Empty input yields an empty owned string, an empty separator is the sole concat
spelling, and failure leaves every borrowed source unchanged. The maintained
example uses one `[N]&[char]` annotation followed by the direct
`(&parts).join(...)` spelling.

Join deliberately does not accept an arbitrary `Iterable` and does not add a
public text-conversion trait: one-shot iteration cannot support the exact
sizing pass, and resource-owning generic elements would reopen ownership and
conversion questions unrelated to borrowed scalar composition. The nested
slice extension also exposed that generic-parameter lookahead consumed
`extend [&[char]]` before ordinary target parsing. Empty generic lookahead now
rewinds uniformly, with parser and executable regressions for the concrete
structural target. Parsing vocabulary and the maintained end-to-end text/path/
process workflow remain the next reconstruction work.

Standard-library reconstruction progress (2026-08-03, textual parsing batch):
textual value parsing is now the independent root `std::parse` facade rather
than a formatting submodule. Character slices, byte slices, C-string views,
process arguments, and environment values expose ordinary `input.parse[T]()`
and explicit `input.parseRadix[T](radix)` calls. `parse::From` and
`parse::FromRadix` are separate capabilities with associated error types:
custom values retain domain-specific failures, while `bool` does not pretend
to support radix parsing. Primitive integer errors retain empty, digit, sign,
overflow, and radix distinctions; invalid boolean text is `InvalidValue`.

The old `fmt::parse`, `fmt::parse_radix`, `fmt::ParseFrom`, `ParseError`, and
snake-case protocol methods are physically absent. The implementation uses
lower-camel names, contextual numeric literals, shared sign/digit scanning,
and direct borrowed-slice `for` traversal. Build-runner target arguments and
the C-string/process adapters now depend on parsing directly, so `fmt` owns
only formatting. The reviewed build-host closure records the new shallow
facade and provider instead of the deleted `std/fmt/parse.nia` path.

The receiver surface leaves the input type with its owning source API, so
fixed-array pointers take their ordinary slice coercion before result-protocol
selection. Character and byte literals, named arrays, and array fields are
covered without `[..]`.
All 153 body-check tests, focused cross-module visibility/reachability tests,
primitive/custom-error and process-argument executable regressions, one
production configured build, strict workspace Clippy, formatting, and the
build-host dependency audit pass.

Standard-library reconstruction progress (2026-08-03, Unicode scalar and
dual-stage const batch): the builtin `Char` trait and `u32.Char` dispatch are
physically removed. Checked construction is the lower-camel
`unicode::fromScalarValue`, backed by `std::builtin::charFromU32`; validation is
`unicode::isValidScalarValue`. Scalar inspection and encoding are inherent
`char.codepoint()` and `char.encodeUtf8()` operations. Encoded and decoded
records are named `Utf8Scalar` and `DecodedUtf8Scalar`, with `byteLen()`,
`bytes()`, and `scalar()` accessors. No snake-case or builtin-trait aliases
remain.

This API exposed a language-level requirement rather than a std naming issue:
Nia `const fn` is now formally const-capable, not const-eval-only. A const
expression may call only `const fn`; the same function is also retained by
runtime reachability and lowered through the ordinary backend when runtime code
calls it. Free functions, receiver methods, associated functions, imported
public functions, and generic extension targets follow that contract. The
const evaluator consumes the shared semantic type-prefix and visible-extension
facts, while ordinary runtime calls continue through body checking and
codegen. Executable coverage uses the same definitions for an array length and
runtime results, and also proves a private comptime-only helper does not need to
become a backend root. This is the general staging model for future std APIs,
not a Unicode-specific exception or a compatibility layer around a restricted
evaluator.

Dual-stage const hardening progress (2026-08-03, declaration and resource
foundation batch): the roadmap now carries a separate three-round const stage
covering declaration validity, cross-stage/backend conformance, and
query/incremental/resource hardening. The first implementation wave makes
unused `const fn` bodies participate in declaration checking for tail/return
contexts, explicit local initializer types, expression statements, assignment
operands, and ordinary free/method calls even in unselected branches. Enum
payload constructors remain const data construction rather than being
misclassified as function calls, and semantically equal array types compare
evaluated lengths instead of requiring identical const-expression identities.

Const evaluation now opens one resource session per outer expression. Module
const checking and function-body local const execution share a 1,000,000-step
budget and a 256-frame function-call limit across nested evaluation; the
existing single-loop limit remains an additional guard. Exhaustion returns an
ordinary source diagnostic at the active site. Infinite recursion therefore
cannot consume the host stack, while terminating recursion remains valid. Unit
coverage proves budget reset and depth release; driver coverage proves unused
wrong returns, wrong local initializers, ordinary calls in statements,
assignments, and unselected branches, finite recursion, and bounded infinite
recursion. All 17 const-evaluator tests, all 505 driver tests, and all 194
compiler-query tests pass. The remaining declaration-depth audit, cross-stage
differential matrix, and incremental/stress rounds stay explicitly open under
the new track.

Dual-stage const hardening progress (2026-08-04, assignment declaration audit):
the typed declaration frame now records local mutability as well as type, and
assignment checking resolves local, field, and array paths without executing
parameter-dependent indexes. Unused `const fn` definitions therefore reject
immutable writes, known-invalid target paths, RHS type mismatches, non-numeric
compound assignments, readonly slice element writes, and assignment tails that
cannot satisfy a value return. Plain generic assignment and parameter-indexed
array mutation remain valid when their declaration is not yet instantiated.
Existing execution-time assignment validation remains responsible for concrete
values, shapes, and bounds. The shared generic-place/trait operator model and
the rest of the declaration-depth audit remain open; this batch does not freeze
a const-only indexing or operator type system.

Dual-stage const hardening progress (2026-08-04, condition and builtin scalar
operator audit): unused const-capable definitions now diagnose known non-bool
`if`/`while` conditions, invalid primitive unary operands, incompatible
primitive binary/equality operands, and incompatible concrete `if` branch
types. Condition failure no longer stops declaration traversal before the
corresponding branches or following statements. Generic, `Self`, projection,
and non-primitive operator types remain unresolved here rather than being
rejected by an evaluator-specific approximation; constrained generic operator
bodies have an explicit acceptance regression. Their trait obligations and
operator output projections must converge with the ordinary body checker in a
later shared-semantics batch. Round 1 remains open for that convergence and the
remaining expression, pattern, aggregate, and control-flow audit.

Dual-stage const hardening progress (2026-08-04, switch declaration audit):
unused const-capable definitions now reject known-incompatible switch patterns
and concrete arm result types. An invalid pattern no longer prevents its arm
body or later arms from being declaration-checked, so a runtime-only call
cannot hide behind a malformed or unselected pattern. Pattern compatibility is
diagnosed once outside contextual arm-type retries. A pattern whose type is not
yet available also leaves the switch result unresolved while its body is still
audited. Targets containing generic, `Self`, or projection types remain
unresolved instead of being treated as definite pattern errors. Exhaustiveness
and the shared ordinary/const pattern contract remain open, as do aggregate
literal and indexing diagnostics.

Dual-stage const hardening progress (2026-08-04, array literal declaration
audit): unused const-capable definitions now diagnose known array element type
mismatches, concrete list-length mismatches, non-integer repeat counts, untyped
empty arrays, and array literals placed in a known non-array context. Array
checking visits every element before failing, so an earlier mismatch cannot
hide a later const-capability error. Contextual generic element types remain
legal and have an unused-definition regression; unknown repeat values and
generic lengths remain unresolved rather than being executed during declaration
checking. Struct/enum aggregate fields and indexing remain open in Round 1.

Dual-stage const hardening progress (2026-08-04, typed aggregate and struct
literal declaration audit): explicit typed arrays now pass through the same
element/count/length audit as contextual arrays instead of returning their
annotation without visiting children. Contextual and explicitly typed nominal
struct literals now diagnose duplicate, unknown, and missing fields plus known
field type mismatches, while visiting values for every supplied field. Untyped
structural literals also reject duplicates without hiding the duplicate value's
const-capability errors. Known non-struct contexts are rejected; generic field
types remain unresolved and have an unused-definition acceptance regression.
Enum payload aggregates, union-specific semantics, and indexing remain open.

Dual-stage const hardening progress (2026-08-04, indexing and slicing audit,
superseded by shared declaration checking): unused const-capable definitions
visit index operands and both slice bounds even when an earlier expression is
invalid, so nested const-capability failures remain visible. The ordinary body
checker owns `Index`/`Slice` trait obligations, result projections, integer
bound typing, pointer-to-slice syntax, and contextual result typing. Concrete
bounds, reversed ranges, inclusive-end overflow, and other value-dependent
failures are evaluation-time traps only when comptime reachability executes the
expression; an unused declaration is not evaluated merely to find a trap.
Parameter-dependent indexing therefore remains a valid declaration and is
checked when a call supplies concrete values.

Dual-stage const hardening progress (2026-08-04, enum payload declaration
audit): unit, tuple, and named enum variant construction now participates in
unused-definition checking without stopping at the first malformed payload.
Tuple variants diagnose payload-shape and concrete arity/type mismatches while
visiting every supplied value. Named variants diagnose wrong payload shape,
duplicate, unknown, and missing fields plus known field type mismatches while
also visiting every value, including duplicate and unknown fields. Valid
parameter-dependent tuple and named construction remains accepted. Optional
and error-union construction/propagation, shared trait/operator semantics, and
the remaining control-flow declaration audit stay open in Round 1.

Dual-stage const hardening progress (2026-08-04, shared declaration checker
foundation): `BodyCheckFilter::ConstDeclarations` now selects every const
function declaration as an ordinary body-check root, excludes globals and
module bindings, and never follows discovered runtime calls. Compiler-query
invokes it in `FactsOnly` mode after const semantic facts are available and
merges its diagnostics into the module const result. Direct architecture
coverage proves that ordinary functions are not declaration roots, referenced
runtime callee bodies are not followed, and no typed function body is produced.

The ordinary checker now owns declaration type diagnostics, including trait
and output-projection validation for compound assignments. Shared
`ResolvedCall` facts and `FunctionSignature::is_const` also own the rule that a
const-capable body may call only another `const fn`; free, generic, method,
trait, associated, dynamic, and function-pointer calls use one resolved-call
classification, and diagnostics are deduplicated by source node.

`compute_module_const_typed_facts` no longer traverses every const function.
The dead `analyze_functions`/`check_const_function_body` entry and its separate
function-block type audit are deleted. Remaining
`resolved_const_expr_type`/`const_function_types_match` use is demand-driven by
actual const evaluation, local value typing, and result normalization rather
than declaration validation. Round 1 remains open for const-only builtin and
operation capability coverage, optional/error-union and union-specific
semantics, and the remaining control-flow audit.

Dual-stage const hardening progress (2026-08-04, builtin capability table):
`BuiltinFunction::is_const_capable` is now the exhaustive shared contract for
builtin calls. The implemented const set is `error`, `size`, `align`, `offset`,
`embed`, and `charFromU32`; trap, asm, memory, SIMD, bit, and atomic intrinsics
remain runtime-only until they gain an explicit const lowering and evaluator
contract. Const declaration checking rejects a runtime-only builtin even in an
unused function and names that builtin in the diagnostic. Adding a new builtin
now requires an explicit const-capability decision in the enum's exhaustive
match rather than silently inheriting acceptance.

Dual-stage const hardening progress (2026-08-04, intrinsic operation
capabilities): compiler intrinsic methods now carry the same explicit
second-stage capability decision as builtin functions. `len`, `start`, and
`end` match the const IR/evaluator implementation; value operators and the
direct AST forms for reference, dereference, indexing, and slicing retain their
existing const execution paths. Mutable dereference/pointer extraction and
`Iterable::iter`/`Iterator::next` were initially rejected during declaration
checking instead of being accepted until evaluation discovered an unsupported
operation. Const `for` was deliberately not implemented by duck-typing a value
loop: the ordinary runtime lowering calls `Iterable::iter` and repeatedly
mutates the `Iterator::next(&mut self)` receiver, so const execution first
needed the shared call/place writeback contract recorded below.

Dual-stage const hardening progress (2026-08-04, optional/error-union
declaration contract): unused const-capable definitions now have full-query
regressions for contextual optional and error-union construction, both success
and error payload types, optional/error propagation return constraints, invalid
propagation operands, and recursive optional/error-union patterns. Malformed
patterns do not suppress capability diagnostics in their arm bodies. These
rules come from the ordinary expression and pattern checker under the const
declaration filter; `nia-const-check` retains only demand-driven value typing
and propagation normalization during real evaluation. Optional/error-union
declaration semantics are therefore closed in Round 1. Cross-stage executable
comparison remains a Round 2 matrix item; const `for`, place writeback, and
union-specific representation remain open.

Dual-stage const hardening progress (2026-08-04, union capability boundary):
ordinary runtime union construction and field access remain fully supported,
but the const declaration filter now rejects those operations explicitly until
the evaluator has an ABI-backed active-field and reinterpreting-value model.
This prevents an unused `const fn` from appearing valid and then failing later
inside the const type layer with an unrelated structural-value error. The
rejection is a named Round 1 capability boundary, not a second union type
checker; Round 2 must replace it with a shared const/runtime union
representation and differential coverage.

Dual-stage const hardening progress (2026-08-04, first cross-stage executable
matrix): one maintained build-resource executable now calls the same
const-capable definitions at comptime and runtime across arithmetic, numeric
casts, fixed arrays/indexing, optional `if ... is`, error-union and payload-enum
switches, `while`, receiver methods, generic functions, and public imported
functions. It also evaluates a private imported helper only at comptime and
proves that the helper does not become a backend root. Imported optional
payload matching covers the cross-module evaluator path directly.

Constructing that matrix exposed an evaluator type-recovery recursion when
optional and error-union functions with nominal payloads were both evaluated:
pattern-local recovery scanned every function body and re-entered an unrelated
switch target. Local value/type/mutability lookup is now bounded by the current
execution root, and pattern-local type recovery inspects only the active
function body (including imported bodies). Module initializer recovery no
longer scans function declarations. The differential executable and the
original combined optional/error-union reproducer both complete without host
stack growth.

Dual-stage const hardening progress (2026-08-04, associated and const-generic
calls): the executable matrix now includes one associated `const fn` and one
function-level const-generic instance, with each definition evaluated at
comptime and emitted through the ordinary runtime pipeline. Const call IR now
stores an ordered type-or-value generic argument sequence instead of treating
every bracket argument as a type. Lowering uses ordinary semantic type and
value facts to resolve the parser's dual candidates; const-function
instantiation follows `FunctionSignature::generic_params` order and evaluates
const arguments with the declared primitive type and target range. The former
fallback that encoded an outer const parameter as a generic type has been
removed.

Adding the associated call exposed a separate semantic-fact ownership bug:
field typing tried const-IR recovery before checking an ordinary call on the
field lhs, so runtime lowering could receive a typed field with no resolved
callee fact. Ordinary expression checking now owns the lhs first, and const
field recovery is restricted to genuinely `ConstOnly` structural values. The
matrix therefore reaches both associated and const-generic runtime instances
without turning their comptime uses into backend roots. Round 2 remains open
for `for` after shared iterator/place writeback exists,
add/subtract/multiply/negate overflow, shift, explicit-trap and integer-vector
lane boundaries, and the union-representation work already identified above.

Dual-stage const hardening progress (2026-08-04, function-reference boundary):
runtime code now also exercises a `const fn` through a function pointer in
the executable matrix. The existing `FunctionReference` semantic fact makes
that target an executable-reachability root, while a focused compiler-query
regression asserts the exact runtime-function/body set and excludes an unused
valid `const fn`. Const-only evaluation still adds no runtime reference edge.

Function pointers are deliberately a capability boundary for const evaluation:
their type records an ordinary runtime call signature but carries no proof that
every possible target is const-capable. The const declaration filter therefore
rejects both forming a function pointer value and calling one indirectly with
specific staging diagnostics. Future comptime indirect calls require an
explicit const-callable function type/effect or equivalent static data-flow
proof; evaluator-local provenance guessing is not an acceptable substitute.
Function references are closed for the current Round 2 contract.

Dual-stage const hardening progress (2026-08-04, integer division/remainder
boundary): scalar integer `/`, `%`, `/=`, and `%=` now branch around LLVM
operations through one shared codegen helper. A zero divisor always reaches
`llvm.trap`; signed `MIN / -1` and `MIN % -1` also trap before LLVM can create
poison. Const evaluation already reports zero divisors at the active source
expression, and the language contract now states that the corresponding runtime
conditions are traps rather than backend-dependent behavior. Focused const
diagnostics and LLVM IR regressions cover both signed and unsigned scalar paths.
Integer-vector division remains a separate per-lane trap/reduction design item;
this batch preserves its existing lowering rather than applying scalar checks
to a vector value.

This audit also exposed the prerequisite for the remaining integer boundaries:
const values retain integer magnitude and signedness, but const IR operations do
not retain their concrete primitive width. Final-value range validation cannot
detect an overflowing intermediate that later returns to range, and its generic
128-bit shift limit cannot define `u8`, `i32`, or target-width semantics. The
const evaluator now consumes ordinary semantic expression types through a
narrow integer-operation fact (`bits` plus signedness). Facts are scoped to the
active const root or function instance, so expected-context literals,
substituted generic operators, imported function bodies, and target-dependent
`usize`/`isize` widths do not share a stale global expression cache. Integer
add/subtract/multiply/negate therefore diagnose an overflowing intermediate at
the operation rather than relying on final-value validation; typed bitwise-not
also uses the concrete width, and integer value equality is independent of the
evaluator's internal signedness encoding. The evaluator neither inspects
literal suffixes to recover types nor owns a second type system.

Concrete-width const shifts now use the same facts: the count must be below the
instantiated operand width, left shift diagnoses a result that does not fit,
and signed versus unsigned right shift follows the concrete primitive. The
32-bit target regression proves `usize` shift bounds do not follow the compiler
host. Scalar runtime add/subtract/multiply/negate now use LLVM signed/unsigned
`with.overflow` intrinsics and branch to `llvm.trap` on the returned flag;
compound assignment shares that lowering, and integer negation is checked as
zero minus the operand. Scalar runtime shifts now preserve the independently
typed right operand through Function IR codegen. They reject negative signed
counts and counts at least as wide as the left operand before any narrowing or
LLVM shift can occur. Left shift computes in twice the left width and compares
the exact result with a signed- or zero-extended round trip, trapping when the
mathematical value is not representable; signed and unsigned right shift lower
to arithmetic and logical shift respectively. Ordinary operators, builtin
operator calls, and compound assignment share this path. Host-width checked
arithmetic must not become the language model, and integer-vector lanes remain
a separate reduction/trap design item. Explicit `std::builtin::trap` is now a
real dual-stage operation rather than a runtime-only exception: its std
declaration is `const fn`, const IR retains a distinct trap node, unselected
branches remain valid, an evaluated comptime trap becomes a source diagnostic,
and runtime reachability still lowers the same function body to `llvm.trap`.
This remains separate from the message-bearing, const-only `error` builtin. The
integer-vector boundary is now explicit for add/subtract/multiply/negate:
boolean masks are no longer accidentally classified as numeric, while integer
vectors use LLVM vector `with.overflow` intrinsics and reduce the returned lane
mask to one any-lane trap condition. The reduction packs `<N x i1>` as `iN`, so
it neither scalarizes the operation nor inherits the public 64-lane `bitmask`
limit. Vector division/remainder now use the same reduction for zero-divisor
and signed `MIN / -1` or `MIN % -1` lane masks, branching around the LLVM
operation until all lanes are valid; compound assignment shares the helper.
Vector shifts now close the remaining numeric boundary. Ordinary semantic
checking requires a same-type integer count vector, rejecting the former
accidental scalar, mismatched-lane, and boolean-mask shapes; uniform counts use
an explicit splat. Codegen checks negative and out-of-width counts per lane,
reduces them before shifting, computes left shifts in a vector with doubled
lane width, and traps on any failed representability round trip. Signed and
unsigned right shifts remain arithmetic and logical respectively, and compound
assignment shares the same path. The scalar and vector contracts now avoid
LLVM poison without introducing an evaluator-local or backend-local type
system.

Dual-stage const hardening progress (2026-08-04, mutable receiver place
writeback foundation): const function parameters now retain their ordinary
`ReceiverKind`, so evaluator frames no longer guess whether `self` is mutable
from an erased or recovered type. A resolved method receiver is evaluated into
one explicit const place rooted at a caller local. Field projections and array
indices are captured before the call, and an index expression is therefore
executed exactly once even when the callee mutates `&mut self`. Const function
execution returns the updated mutable receiver beside its ordinary result; the
caller then reconstructs the containing aggregate and writes it through the
same checked local-frame ownership boundary used by assignment. `self` is also
accepted as an ordinary const assignment root, closing the method-body gap
that previously made mutable receiver declarations appear supported while
their bodies could not be lowered. The indexed-place regression also exposed
that const type/bounds probes were evaluating against the live execution frame:
a side-effecting index was committed repeatedly while recovering its type and
checking its bounds. Integer and array-length probes are now transactional;
they restore call frames, diagnostics, and the evaluation budget after
observation, so semantic probing cannot alter comptime state or steal resources
from the real execution.

This is the shared call/place prerequisite for const `for`, not a special
iterator evaluator. A focused const regression mutates a nested struct in an
indexed array place, uses a side effect in that index, and fixes the exact
comptime result. The maintained dual-stage executable also calls one mutable
receiver definition at comptime and runtime, checking both the returned value
and the updated runtime owner. General mutable pointer arguments and
dereferenced-place aliasing remain a separate extension of the same place
model; this batch makes no snapshot-pointer claim for them.

Dual-stage const hardening progress (2026-08-04, shared const iteration): const
`for` now follows the ordinary iteration contract. An intrinsic Iterable impl
for an Iterator preserves the input value as explicit loop state; a user
Iterable instead invokes its visible `Iterable::iter` trait witness. Every
iteration invokes the visible `Iterator::next` witness through the mutable
receiver call contract above, stores the returned receiver state, matches the
optional Item payload with the ordinary resolved pattern representation, and
propagates `continue`, `break`, return, and error/optional propagation through
the existing evaluator flow. Trait witness selection filters by builtin trait
identity and visibility, so a same-named inherent method cannot accidentally
become `for` semantics. Direct const `iter()` and `next()` calls use the same
capability and call path. Focused regressions fix exact results for intrinsic
Iterator self-iteration, user Iterable construction, direct method calls, and
loop control flow. The maintained emitted executable runs the same iterator
definition and `const fn` loop at comptime and runtime. Const `for` and its
place-writeback prerequisite are closed for Round 1; imported/generic
iteration is closed by the following Round 2 hardening batch.

Dual-stage const hardening progress (2026-08-04, imported/generic iteration):
cross-module generic `Pair[T]`/`PairIter[T]` regressions now execute visible
`Iterable::iter` and `Iterator::next` witnesses with substituted target and
associated `Item`/`Iter` types. The maintained emitted executable uses the same
imported generic definitions for an array-length comptime call and a runtime
call, proving both evaluator execution and backend witness closure.

Const declaration checking now validates the exact builtin-trait witness
selected by the ordinary trait solver. A runtime-only imported `iter` or `next`
is rejected even in an unused `const fn`; a same-named inherent const method
cannot replace the witness used by `for`. Direct `iter()` and `next()` calls
enforce the same concrete-witness capability. An imported impl outside the
visible extension closure remains unavailable and produces the ordinary
`Iterable` diagnostic rather than becoming a const-only visibility exception.

The backend reachability matrix instantiates one generic iterator target only
at comptime, one at both stages, and one only at runtime. The initial backend
plan contains only the two runtime `count` instances, and final backend closure
contains exactly the corresponding two `count`, `iter`, and `next` instances;
the const-only target does not become a backend root. Imported/generic
iteration is therefore closed for the current Round 2 contract.

Dual-stage const hardening progress (2026-08-04, scalar union representation):
const union values no longer reuse the field-keyed struct representation. The
shared value stores artifact-target-ordered bytes, per-byte initialization,
stable scalar field ABI descriptors, and the latest written field identity.
Reading another field decodes those same bytes; switching fields writes only
the selected width. `nia-layout` now exposes the primitive-layout and union
max-size/max-alignment facts used by the codec, so const evaluation does not
invent a parallel ABI. Little- and big-endian tests fix different results for a
wide-to-narrow read, signed and floating reinterpretation are covered, and a
larger read after smaller-field construction diagnoses uninitialized storage
instead of fabricating zero bytes. A generic `Slot[T]` regression proves that
the ABI schema is created after concrete type substitution rather than from an
open generic placeholder.

The ordinary const declaration filter now accepts scalar unions even in unused
`const fn` bodies and rejects unions containing fields without a const ABI model
by naming the unsupported field. Typed aggregate preparation is bounded to the
binding, result, call argument, or assignment RHS about to execute; an attempted
whole-function recovery reopened the previous cross-function recursion and was
removed. The maintained executable
runs the same union functions at comptime and runtime and consumes a top-level
union const field from runtime code.

This closes scalar union Round 2a. Arrays, pointers, vectors, nested aggregates,
padding propagation, and pointer provenance remain the next union rounds;
imported generic differential coverage must accompany them before the general
union shared-representation boundary is considered closed. The remaining
cross-stage aggregate boundaries stay open.

Dual-stage const hardening progress (2026-08-04, imported generic scalar union
differential): a public imported `ScalarSlot[T]` now crosses construction,
whole-value return, whole-value argument, and reinterpreting field-read
boundaries after substituting `T = f32`. Driver coverage checks the imported
definition in both a comptime const and an ordinary runtime body. The maintained
emitted executable evaluates the same imported generic functions at both
stages, passes an imported generic union const into runtime code, independently
constructs the runtime value, and observes the same `f32`/`u32` representation
from both paths.

This closes the imported/generic differential requirement for scalar union
Round 2a. It does not widen the const ABI codec: each future aggregate, vector,
or pointer field round must add its own imported generic differential together
with its padding, initialization, or provenance rules.

Dual-stage const hardening progress (2026-08-04, scalar-array union
representation): union field schemas are now recursive `ConstAbiType` values.
Fixed arrays of scalars, including nested arrays, const-expression lengths, and
layout-builtin lengths, encode and decode element by element using artifact
endianness. Const layout queries now consume the artifact pointer width instead
of assuming LP64. `nia-layout` owns the checked array-size rule as well as
primitive and union layout; overflow is rejected instead of saturating into a
fabricated layout. Per-byte
initialization still applies to the resulting field range, and invalid `bool`
or `char` representations are diagnosed while decoding individual elements.

The declaration capability accepts recursively scalar arrays while continuing
to reject structs, unions, pointers, and vectors with a supported-const-ABI
diagnostic. Runtime const materialization now builds ordinary array literals for
all supported scalar and nested-array values rather than only byte and character
strings. This exposed two independent runtime ownership gaps: index checking
performed const recovery before recording the ordinary call lhs, and LLVM array
indexing required an rvalue to already be a place despite an existing array
temporary helper. Ordinary lhs checking now runs first and const structural
element recovery is limited to a genuine `ConstOnly` lhs; comptime range slicing
retains its ordinary const-array result after that lhs check. Rvalue array
indexing uses the same temporary-address path already used by slicing.

Focused little-/big-endian, 32-bit pointer-width, nested-array, generic
substitution, invalid-element, and layout-overflow tests fix the representation
contract. The maintained
executable imports a generic `PairBytes[T]`, evaluates its encode/decode
functions at comptime and runtime, passes const arrays across the runtime
boundary, and indexes both call results and materialized const arrays. This
closes scalar-array union Round 2b. Nominal aggregate padding, nested union
storage, vectors, and pointer provenance remain open and require separate
representation rounds.

Dual-stage const hardening progress (2026-08-04, artifact layout ownership):
the remaining compiler layout providers no longer assume LP64. Ordinary,
signature, executable runtime-body, and executable type-only layout products
derive `TargetDataLayout` from `CompilerTargetQuery`, making the artifact target
an explicit query dependency and invalidation input. The standalone body-check
orchestration helpers likewise derive layout from their existing `TargetConfig`
instead of silently mixing a caller target with LP64 layouts.

Focused 32-bit query regressions fix `usize` size and alignment at four bytes in
ordinary and signature products, then carry the same target through a runtime
entry module and a cross-module executable type owner. This closes runtime
artifact layout threading as an architecture batch. It does not claim that
every supported target has an end-to-end LLVM backend differential; later
pointer/provenance rounds must still test the relevant emitted artifacts rather
than inferring backend correctness from layout-query coverage.

Dual-stage const hardening progress (2026-08-05, nominal struct union
representation): `ConstAbiType` now describes nominal structs with substituted
`nia-layout` field offsets and object size. ABI encoding produces bytes and an
equally sized initialization bitmap: recursive field ranges become initialized,
while inter-field and trailing padding remains uninitialized. Struct decoding
reads only described fields, so a same-field round trip succeeds while a union
reinterpretation covering padding is rejected instead of observing fabricated
zero bytes. Arrays inherit the same bitmap behavior for nested struct elements.

The declaration capability now accepts recursively supported nominal structs
and still rejects a struct containing pointers, vectors, nested unions, or other
unsupported leaves. Const-generic nominal structs remain closed until their
field substitution contract is shared with ordinary aggregate checking. Runtime
const materialization now lowers whole struct values to ordinary
`StructLiteral` IR. This exposed the corresponding LLVM place gap: aggregate
rvalue field projection now uses the general value-temporary path, shared with
array rvalue indexing rather than special-casing const expressions.

Local padding, generic substitution, invalid nested scalar representation, and
imported generic differentials fix the semantic boundary. The maintained
executable encodes and decodes an imported generic padded struct at both
comptime and runtime, crosses the whole-const-struct boundary, and projects the
materialized rvalue after LLVM emission. This closes nominal struct union Round
2c. Const-generic nominal structs, nested union storage, vectors, and pointer
provenance remain open representation rounds.

Standard-library reconstruction progress (2026-08-05, receiver parsing and
generic-call cleanup): textual parsing now has one ordinary user surface:
supported inputs expose `input.parse[T]()` and `input.parseRadix[T](radix)`.
Borrowed character and byte slices own the structural receiver extension;
`CStringView`, `Arg`, and `EnvVar` own their view-specific methods. The
result-side `parse::From[Input]` and `parse::FromRadix[Input]` protocols retain
input polymorphism and associated error types, with `from` and `fromRadix` as
their implementation methods. The redundant free-function facade is physically
absent rather than retained as an alias.

The facade had introduced a language-wide implicit-prefix rule solely to hide
its second `Input` generic. That rule is now removed: an explicit generic
function-call list has the declaration's full arity, while `_` explicitly marks
each type or const position that inference must fill. Omitting brackets still
requests full inference. Bracket `_` is deferred by syntax type-lowering and
consumed by function-call binding, so no inference sentinel leaks into semantic
types, const arguments, monomorphization, or backend IR. The 12-test type-lower,
167-test body-check, 198-test compiler-query, 559-test driver, and 122-case
build-library suites pass (one build helper is intentionally ignored), together
with primitive/custom-error and process-argument emitted executables. Workspace
check, strict all-target Clippy, formatting, old-surface search, and diff
whitespace audit also pass.

Dual-stage const hardening progress (2026-08-05, const-generic nominal struct
union representation): aggregate signatures now retain ordered generic
parameter kinds in addition to their declaration names, including through the
signature cache and cross-module program signatures. `nia-layout` consumes one
kind-aware binding path for local and imported struct/union instances, so type
and const parameters may be interleaved instead of relying on an implicit
"types first, consts last" split. Layout field instantiation now uses the shared
`nia_ty::substitute_ty` traversal rather than a smaller layout-local copy.

Const aggregate field typing uses the same parameter-kind binding and shared
substitution contract. Demand-driven const type comparison now handles nominal
types recursively and resolves const-expression arguments through the ordinary
const evaluator, so a named integer or boolean const is semantically equal to
its literal value rather than being compared by expression identity. The
standalone const-check fixture now includes the same value-resolution pass for
type-lowering const expressions that compiler-query already uses in production.
Focused regressions cover interleaved `T, N: usize, U` struct/union layout,
little-endian padded union reinterpretation, named-versus-literal array lengths,
and named-versus-literal boolean nominal arguments. The production executable
matrix also exercises an imported const-generic struct through both compile-time
and runtime `const fn` calls. That regression closes two cross-module contract
gaps: type lowering interprets an imported const parameter type in its defining
module instead of reusing the caller's node resolution, and detailed
struct/union instance layout bypasses a size/align-only program cache hit so it
always returns field offsets. Const-generic nominal structs are therefore
closed for union Round 2d; nested union storage, vectors, and pointer provenance
remain open representation rounds.

Dual-stage const hardening progress (2026-08-05, nested union storage): an untagged
union value is artifact storage, not a source-field construction history.
`ConstUnionValue` no longer retains a `last_written_field`; that
identity was only an escape hatch for rebuilding a const result as an ordinary
field-based union literal and is not part of the language value model. Reads
select a field schema and decode shared bytes, while writes replace
only that field's described byte ranges.

`ConstAbiType` now has a recursive union descriptor containing every field's ABI
descriptor and the layout-owned storage size. Encoding a nested union copies
its bytes and initialization bitmap after checking the descriptor, target
endianness, and storage size. Decoding creates the same raw union storage without
inventing an active field. This keeps nested arrays and structs compositional:
their padding and a nested union's unwritten tail remain uninitialized through
every enclosing aggregate.

Whole const unions cross into runtime IR through a dedicated internal
union-storage literal containing one optional value per artifact byte. This is
distinct from the source `UnionLiteral`, which still evaluates its one selected
field normally. Backend validation requires the storage literal's semantic
type to be a concrete union and its byte count to equal the artifact union
layout. LLVM lowering allocates that nominal union storage and stores only
the initialized bytes through byte-addressed offsets; absent bytes receive no
store and are never fabricated as zero. Direct aggregate destinations,
indirect arguments, returns, expression temporaries, fingerprinting, reachability,
and optimization traversal all recognize the storage literal as a
side-effect-free leaf.

Focused evaluator coverage fixes same-field nested round trips, retained wider
tails after a smaller nested write, padding propagation, and uninitialized wider
reads. Declaration checking recursively accepts both nominal struct and union
schemas while retaining cycle rejection and the pointer/vector capability
boundary. The driver and maintained executable exercise an imported generic
nested union at comptime and runtime, materialize a whole const outer union as a
runtime argument, then reinterpret it again to prove that the stage boundary
preserves the exact byte state. This closes nested union storage Round 2e;
vectors and pointer provenance remain later representation rounds.

Dual-stage const hardening progress (2026-08-05, Round 2f vector
representation): vector support is an artifact-representation round, not only
another const evaluator value variant. `nia-layout` is the owner of the vector
storage contract. A
vector's store width is the byte-rounded total lane bit width, including packed
`bool` mask lanes; its ABI alignment is the next power of two of that store
width, and its allocation size is rounded to that alignment. All layout
consumers must call that shared rule instead of retaining backend-local copies.
This also corrects the existing mismatch where Nia described `boolx16` as 16
bytes and `u8x16` as byte-aligned while LLVM stores them as a 2-byte mask and a
16-byte-aligned vector respectively.

Const evaluation now has a distinct vector value and ABI descriptor. Vector
encoding and decoding preserve lane order, artifact endianness, floating-point
bits, packed boolean masks, target-sized integer lanes, and any allocation tail
padding without conflating vectors with arrays. The minimal SIMD construction
and observation surface (`splat`, `extract`, `insert`, and `bitmask`) becomes
`const fn`; their evaluator uses the fully instantiated concrete signature, so
expected-return inference and explicit generic arguments take the same path.
Lane-wise arithmetic and comparison are not silently claimed by this round and
remain a later const-execution decision.

`extract` and `insert` now have one stage-independent bounds contract. A
constant out-of-range index produces a const diagnostic; a runtime
out-of-range index traps before LLVM's element instruction, so LLVM poison is
not observable language behavior. The executable regressions cover both
paths, direct runtime SIMD use after comptime construction, imported generic
vector unions, whole-union runtime materialization followed by another
reinterpretation, little- and big-endian integer storage, floating-point bit
preservation, boolean packing and `bitmask`, 32- and 64-bit `usize` lanes, and
LLVM IR alignment for vector fields in union storage. Executable signature-fact
mode also gained a distinct foreign function-signature channel after this round
exposed that its Types and Values subsets could not resolve nested imported
`const fn` calls. Pointer provenance stays outside Round 2f and remains its own
representation and ownership design. This closes vector representation Round
2f without claiming lane-wise operator const execution.

Dual-stage const hardening next stage (Round 2g pointer provenance): pointer
support is split by semantic dependency rather than enabled by teaching the
union codec to copy pointer-width integers. The current evaluator-only
`Pointer(Box<ConstValue>)` is a pointee snapshot, not provenance: it aliases no
allocation, compares through the pointee value, and can outlive the local whose
address produced it. It must be replaced rather than retained as a compatibility
representation.

Round 2g1 establishes explicit const allocation and place identity. Taking a
reference to a live place records that allocation plus its projection path;
dereference resolves the live allocation, pointer equality compares provenance
instead of pointee contents, and a function or const root rejects a place
pointer that would escape its owner. Read-only direct const-binding promotions use a
distinct frozen-allocation provenance and retain a stable source origin. Mutable pointer
write-through and general alias analysis are not inferred from the earlier
mutable-receiver writeback mechanism; they require an explicit shared place
operation in a later batch.

Round 2g2 extends union storage with relocations. A pointer field contributes a
pointer-width initialized range plus a typed relocation carrying its target
allocation and offset. Reinterpreting relocation bytes as an integer or
constructing a pointer from arbitrary integer bytes is not a comptime address
operation and must diagnose; copying nested aggregate or union storage preserves
the relocation. No host address may enter `ConstValue`, fingerprints, caches, or
artifact bytes.

Round 2g3 is split into three reviewable batches. Round 2g3a gives every frozen
allocation a shared body/function IR identity, propagates typed relocations
through every recursive transform and reachability collector, validates their
storage shape and pointee expression, and fingerprints the allocation through
stable source identity rather than session-local `ModuleId` allocation. Round
2g3b materializes that identity as one stable link-once LLVM allocation across
uses and codegen partitions, writes relocation pointers into runtime union
storage, and replaces the legacy per-use static-array counter path rather than
retaining two promotion models. Round 2g3c closes the imported/generic
differential. The maintained executable must prove same-provenance equality,
distinct allocation inequality, pointer-field round trips, whole-union
crossing, and comptime/runtime agreement. Static/global provenance and
promoted readonly allocation provenance must remain distinguishable, while
function pointers stay under their separate const-callable capability
boundary. Pointer-containing unions remain rejected until all three subrounds
are complete.

Dual-stage const hardening progress (2026-08-05, Round 2g1 pointer provenance
foundation): the evaluator snapshot representation has been physically removed.
`ConstPointerValue` now distinguishes a frozen promoted allocation from a live
place allocation plus field/index path. Every evaluated local binding receives
a fresh allocation id. Dereference looks up that live allocation, so mutation of
the owner is observable through an existing pointer; equality compares frozen
origin or place allocation/path and no longer treats a pointer as equal to its
pointee value.

Function results are recursively validated before their frame is removed. A
pointer into a callee local or an ended nested scope diagnoses an escape, while
a pointer received from caller storage may return unchanged. Top-level const
results reject all live-place provenance. Rvalue references inside a function
or block are scope-owned temporary allocations rather than promotions. A direct
module or local read-only const promotion uses a stable module-and-source frozen
origin; writable const promotions are rejected. Typed-query context frames are
explicitly distinct from evaluator lifetime frames. The checks recurse through
ordinary aggregate and payload values, and frozen storage cannot smuggle a live
place pointer inside it.
Focused driver regressions cover same-place equality, distinct equal-valued
allocations, read-after-owner-mutation, caller-pointer pass-through, ended-scope
dereference, direct and nested dangling results, module/local readonly promotion,
function-temporary escape, and writable-promotion rejection.

This closes Round 2g1 only. Frozen-origin runtime deduplication, mutable
write-through, union relocations, artifact materialization, and the imported
differential remain the explicit Round 2g2/2g3 work above; pointer-containing
const unions are still rejected rather than accepted through snapshot fallback.

Dual-stage const hardening progress (2026-08-05, Round 2g2 typed union
relocations): `ConstAbiType` now has an artifact-width pointer representation,
and `ConstUnionValue` owns relocations alongside bytes and initialization state.
Each relocation records its storage offset, pointer width, and typed
`ConstPointerValue`; its byte range is initialized placeholder storage, never a
host address or fabricated integer. Array and nominal-struct encoding shifts
nested relocation offsets, nested-union encoding preserves them wholesale, and
field writes invalidate every relocation whose storage they overlap. Any
unwritten fragment of an invalidated relocation becomes uninitialized rather
than turning placeholder zeroes into integer bytes; genuinely disjoint union
tail storage remains intact.

Decoding a pointer requires one exact relocation covering that pointer field.
Raw integer bytes cannot manufacture one. Conversely, scalar and vector views
reject relocation-bearing storage, and aggregate subdivision rejects fields
that would read only part of a relocation. Relocation bounds, overlap, and
initialization metadata are validated with the union ABI. Pointer-width
regressions exercise both 32-bit and 64-bit artifact layouts. Function/root
escape validation now traverses relocation targets, so hiding a callee-local
pointer inside a union cannot bypass Round 2g1.

This closes evaluator/storage Round 2g2. Runtime lowering deliberately rejects
a relocation-bearing `UnionStorageLiteral`: body IR, function IR, backend
validation, LLVM global allocation/deduplication, and imported-module
differentials remain Round 2g3. Mutable pointer write-through remains a separate
alias-aware place-operation task and is not implied by union relocation support.

Dual-stage const hardening progress (2026-08-05, Round 2g3a promoted allocation
IR): `PromotedAllocationId` now carries the defining module and source span on a
shared IR surface. Pointer ABI entries retain their original pointee type, so a
union reinterpretation may change the pointer view without changing how its
frozen allocation is constructed. Body IR and function IR relocations carry
storage offset, artifact pointer width, allocation identity, and the typed
pointee expression.

Instantiation, operator resolution, inlining, local substitution, optimization
traversal, cross-function constant propagation, devirtualization, aggregate
instance discovery, trait-object discovery, compiler-builtin discovery, and
typed/function reachability all recurse through relocation pointees. Function
IR rejects zero-width, overflowing, out-of-bounds, overlapping, unsorted, or
uninitialized relocation ranges and validates their pointee expression.
Backend validation additionally requires a published allocation-origin module,
artifact pointer width, and a runtime-storable pointee type. Codegen
fingerprints include offset, width, origin span, pointee expression, and the
origin module's normalized source identity; no session-local module handle or
host address enters the cache key.

This closes Round 2g3a only. LLVM still diagnoses if a non-empty relocation
reaches materialization. Round 2g3b must introduce stable link-once promoted
globals, relocation stores, and one unified readonly-promotion path before
pointer-containing const unions become executable; Round 2g3c retains the
imported/generic executable matrix.

This sequencing turns Nia's current experimental build bootstrap into a real
toolchain without discarding the valuable fact that build scripts are ordinary
Nia programs and can use a carefully layered standard library.
