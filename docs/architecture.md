# Nia Compiler Architecture

Status: implementation architecture reference

This document describes the compiler implementation architecture for Nia. It is
not the language specification; language behavior is defined in
[language-spec.md](language-spec.md). This file explains crate responsibilities,
data flow, phase boundaries, and the design rules used to keep the compiler
maintainable.

## 1. Architecture Goals

The compiler is a staged, table-driven pipeline. Each crate owns one clear phase,
accepts only the inputs it needs, and produces data that later phases consume
explicitly.

Primary goals:

- keep syntax, name resolution, typing, layout, lowering, and codegen separate;
- avoid a global mutable semantic context;
- pass data through stable ids, immutable tables, and diagnostic lists;
- keep AST as syntax, not as the long-term semantic storage format;
- use `ModuleId`, `DefId`, `LocalId`, `TyId`, and `GlobalDefId` for identity;
- make every phase independently testable;
- use typed backend IR as the backend boundary;
- keep compile-time value checking distinct from static storage checking.

Forbidden shapes:

- one pass that collects definitions, resolves names, infers types, and emits
  code;
- long-lived references between AST nodes;
- string concatenation as the only symbol identity;
- all phases mutating one shared world table;
- bypassing existing phases for temporary features by reinterpreting AST in the
  backend.

## 2. Pipeline

The full pipeline is:

```text
source files
  -> lexer
  -> parser / AST
  -> definition collection
  -> import graph / import aliases
  -> type name resolution
  -> type lowering
  -> item signatures
  -> type normalization
  -> value path resolution
  -> local name resolution
  -> comptime checking
  -> layout computation
  -> ABI check
  -> static initializer check
  -> flow check
  -> body check
  -> monomorphization
  -> backend lowering
  -> LLVM codegen
  -> LLVM IR / object / executable
```

The driver orchestrates these phases. Individual phases do not load files,
schedule whole-program work, or call later backends.

Optimization is configured separately from the phase graph. The CLI accepts
`-O0`, `-O1`, `-O2`, `-O3`, `-Os`, `-Oz`, and `-O` as `-O2`; these levels are
lowered into a Nia `OptimizationPolicy` before query execution. The policy is
threaded through compiler-query, backend lowering, and LLVM codegen even when a
phase has no optimization pass yet. This keeps future Nia IR passes from
depending directly on LLVM's smaller codegen-only optimization enum.

### 2.1 Optimization Levels And Policy

Nia optimization levels are user-facing presets. Internally, each level expands
to a policy matrix with separate decisions for CFG simplification, constant
folding, dead-code elimination, local copy propagation, inlining,
specialization, monomorphized instance deduplication, and size preference.

The levels have these architectural meanings:

- `O0` performs only required canonicalization. It should preserve a direct
  debugging shape and avoid optional backend cleanup.
- `O1` enables cheap, local, low-risk cleanup. It must not require heavy data
  flow, cross-function analysis, or optimization loops with unpredictable
  compile-time cost.
- `O2` is the normal optimized mode. It may use full CFG cleanup, function-local
  data-flow analyses, ordinary DCE, normal inlining, aggregate lowering
  improvements, and monomorphized instance deduplication.
- `O3` is performance-aggressive. It may spend more compile time on inlining,
  specialization, cross-function reasoning, and other transforms that can
  increase code size.
- `Os` is size-oriented but still permits profitable normal optimization. It
  should prefer deduplication and keep inlining and specialization conservative
  when they duplicate code.
- `Oz` is the most size-constrained mode. It should avoid specialization and
  inlining unless required or clearly size-reducing, and should maximize
  deduplication.

This policy split is intentional. LLVM's optimization enum only controls LLVM
codegen choices. Nia must also make earlier size and performance decisions in
monomorphization, backend lowering, specialization, and future inlining, where
LLVM cannot undo duplicated Nia-level work.

Current Nia-owned optimization consumers:

- `nia-monomorphize` collects concrete generic instances before backend
  lowering. It pre-indexes instantiations by source definition and caches
  effective generic lists and mangled type symbols during collection so
  repeated generic-instance discovery does not rebuild the same symbol inputs
  or clone whole definition maps. The policy keeps monomorphized instance
  deduplication visible at this boundary; exact-key deduplication is required
  for symbol uniqueness, while size-oriented levels may later add stronger
  cross-instance deduplication.
- `nia-backend-lower` consumes the policy while lowering function bodies into
  backend IR. Backend passes are selected from policy capabilities, not directly
  from the user-facing level. Cheap dead-code elimination enables same-type cast
  removal, no-op local store removal, removal of zero-sized local runtime
  binding/store operations while preserving initializer effects, removal of
  unused compiler-generated temporary bindings, and removal of discarded
  expressions whose entire wrapper tree is pure, including pure casts,
  operators, ranges, indexes, slices, and aggregate literals.
  Cheap constant folding enables short-circuit logical expression
  simplification when constant operands make it safe, plus constant boolean
  branch folding, including inside defer bodies. Cheap CFG simplification
  enables empty jump block merging,
  same-target branch simplification when the condition is pure, unreachable
  block removal, and the same CFG cleanup inside defer bodies. Full CFG
  simplification additionally folds pure same-target switch terminators to
  direct branches. Full constant folding plus full CFG simplification folds
  switches with literal targets and literal patterns to the selected branch.
  Full local-copy propagation enables local copy propagation. Aggressive
  constant folding plus aggressive local propagation enables local constant
  propagation for O3, but not for size-oriented modes. Full dead-code elimination
  enables overwritten-store cleanup, never-read local store cleanup, and unused
  user local binding cleanup. Full constant folding or size-oriented policy also
  simplifies all-zero static initializers to the canonical `Zero` initializer.
  Module-level leaf inlining is gated by `inline_threshold`: `O0` disables it,
  `O1`, `Os`, and `Oz` inline only no-argument leaf functions that return a
  backend constant. Size-oriented levels deliberately keep non-constant pure
  leaf returns as calls to avoid duplicating aggregate or expression trees.
  `O2`/`O3` additionally inline small pure leaf expressions under a fixed
  expression-cost budget. `O3` uses a larger aggressive budget than `O2`, so it
  may inline larger pure no-argument leaf returns that `O2` deliberately leaves
  as calls. `O2`/`O3` may substitute parameter locals when every call argument is
  itself a small pure expression and the substituted result still fits the
  active budget. The pass does not inline calls with effectful arguments, inline
  assembly, address-taking, assignments, trait-object conversions, or references
  to non-parameter callee locals.
- `nia-backend-lower` records a lightweight optimization report alongside the
  lowered backend program. The CLI report starts with the selected
  `OptimizationPolicy` summary, then lists each changed pass and the affected
  function or global context, including whether a function body was a
  monomorphized instance. This is the stable observability hook for reviewing
  pass behavior without embedding full before/after IR snapshots in normal
  compiler output. `niac check --emit-opt-report` prints this report for direct
  CLI inspection, and `niac emit llvm --emit-opt-report` writes the same report
  to stderr so stdout remains machine-readable LLVM IR.
- `nia-backend-lower` also owns compiler-throughput caches that are independent
  of the user optimization level. Repeated builtin trait-goal resolution during
  function-body instantiation is cached per module lowerer, because array,
  pointer, slice, and builtin place-method lowering can ask the same solver
  question many times while producing identical backend IR. Generic function
  instance discovery keeps a queued-instance set beside its FIFO work queue so
  repeated references do not rescan pending work or rebuild mangled symbols.
  Trait-object vtable collection caches vtable construction by concrete
  `(self_ty, object_ty)` key across instance-discovery rounds so repeated
  coercions do not rebuild identical vtable metadata while discovering
  monomorphized vtable entries. Vtable-driven generic instance discovery scans
  root functions once and then scans only function instances added by the
  previous queue drain, avoiding repeated full traversals of all
  already-discovered instances.
- `nia-codegen-llvm` maps the Nia level to LLVM's codegen optimization level.
  Size-oriented policy remains visible outside LLVM for future Nia-level
  decisions that affect code size before LLVM emission.
- `nia-codegen-llvm` also builds a whole-program index before validation and
  emission. This is compiler-throughput infrastructure rather than a generated
  code optimization: repeated module, item, function-instance, vtable, and
  layout lookups should use the index instead of rescanning backend modules on
  each query. Exact instance-layout keys are indexed as a fast path, enum
  variants are indexed with their owning enum and ordinal for emission, trait
  object vtables are indexed both by exact object type and by object trait for
  bounded cross-interner fallback, and structural type-argument matching is
  retained as a fallback for cross-interner cases.
- `nia-codegen-llvm` performs local ABI-lowering cleanup while preserving the
  backend IR contract. Aggregate literals stored into locals, returned through
  Nia hidden out pointers, or passed as Nia indirect readonly arguments are
  materialized directly into the destination storage instead of building a
  separate literal temporary and copying it again. Aggregate-return calls used
  immediately as local stores or indirect arguments reuse the destination
  storage as their hidden return pointer.
- Backend lowering canonicalizes all-zero static initializers to
  `StaticInit::Zero` under full constant folding or size-oriented policy, and
  static initializer emission may choose cheaper equivalent LLVM constants such
  as `zeroinitializer`. Both must preserve the documented Nia static data
  representation and ABI layout.

Future Nia-owned optimization consumers should follow the same boundary:

- required normalization belongs in the phase that needs the invariant and must
  not depend on a user optimization level;
- optional performance or size transforms must be gated by
  `OptimizationPolicy`;
- ABI-visible transforms must be documented in `docs/nia-abi.md`;
- backend-visible transforms must preserve type layout, parameter and return
  ABI, symbol identity, static data representation, source-level checks, and
  evaluation-order guarantees.

O2 and higher should use an explicit backend optimization pipeline rather than
adding an unstructured list of calls. Each pass should have a stable name,
documented level boundary, focused tests, and eventually statistics so IR diffs
can explain why a function changed.

## 3. Foundation Crates

### 3.1 `nia-span`

Defines source spans with byte offsets. It does not depend on AST, lexer,
parser, diagnostics, or any semantic phase.

### 3.2 `nia-source`

Owns source identity for compiler sessions:

- `SourcePath` identifies a path-like input name;
- `SourceId` identifies a source file inside one session;
- `SourceRevision` identifies a concrete version of that source;
- `SourceFile` carries id, path, revision, and text.

`SourceId` is session-local. Persistent cross-session incremental compilation
must use a separate fingerprint or cache key rather than treating `SourceId` as
stable across compiler runs.

### 3.3 `nia-node-id`

Defines source-versioned syntax node identity for semantic side tables and
diagnostics. `NodeKey` combines source id, revision, syntax kind, and either a
span or red/green child-path position.

AST nodes stay semantic-free. Semantic facts that need syntax identity are
stored in side tables keyed by `NodeKey`, `DefId`, `LocalId`, or `TyId`.

### 3.4 `nia-ids`

Defines stable cross-phase ids:

- `ModuleId`;
- `DefId`;
- `LocalId`;
- `TyId`;
- `GlobalDefId`.

It stores no semantic tables and has no filesystem, parser, or diagnostic
responsibility.

### 3.5 `nia-diagnostic`

Defines diagnostics and source rendering. It owns user-facing diagnostic display
but not semantic policy. Semantic crates create diagnostics; this crate renders
them consistently.

## 4. Syntax Crates

### 4.1 `nia-lexer`

Turns source text into tokens with spans. It handles comments, identifiers,
numbers, strings, multiline strings, character literals, punctuation, and lexer
errors.

The lexer does not know semantic meaning. It should not resolve types, evaluate
constants, or classify identifiers beyond keyword recognition.

The lexer also remains available for CLI/debug tooling such as `niac lex`.
Parser lowering does not depend on lexer token vectors.

### 4.2 `nia-syntax`

Defines the official lossless syntax representation. It builds green nodes and
red syntax nodes/tokens, preserves trivia and full source text, groups delimiter
subtrees, and exposes conservative partial reparsing for token/trivia edits.

Red syntax tokens carry source-versioned child paths. Parser diagnostics and AST
lowering use those identities to populate `NodeOriginTable` entries for later
semantic facts.

### 4.3 `nia-ast`

Defines the parsed syntax tree. AST nodes represent source structure and spans.
They do not store type ids, def ids, layout information, or backend values.

### 4.4 `nia-parser`

Builds AST from `nia-syntax` red tokens and reports parse errors. It owns
grammar decisions, local parse recovery, and syntax-to-AST lowering. While
lowering AST nodes, it records `NodeOriginTable` mappings from AST spans to
red/green child-path ranges.

Important parser boundary:

- expression bracket suffixes are parsed in a syntax-preserving form;
- semantic disambiguation of generic instantiation vs indexing happens later;
- removed historical spellings should not get special migration paths.

### 4.5 `nia-ast-walk`

Provides AST traversal helpers for phases that need tree walking. It should stay
small and generic. It must not embed semantic policy.

### 4.6 Query Frontend

`nia-loader-query` and `nia-compiler-query` provide the typed query frontend for
source loading, syntax parsing, AST lowering, import graph construction,
definition collection, public surfaces, and semantic checks. Query keys use
source versions where source text matters, and `nia-query` tracks in-memory
dependencies and invalidation.

The query frontend is batch-friendly. Persistent caches, cross-session reuse,
LSP scheduling, cancellation, and priority handling are separate future layers.

## 5. Definitions And Modules

### 5.1 `nia-defs`

Collects top-level definitions into module-local definition tables. It assigns
`DefId`s and tracks namespaces for values, types, modules, enum variants, and
methods.

It detects duplicate names in the same namespace and duplicate generic
parameters. It does not type-check bodies or load other files.

### 5.2 `nia-imports`

Normalizes import paths and records import aliases. It handles:

- relative imports such as `import .math;`;
- parent-relative imports such as `import ..lib;`;
- bare module-map imports such as `import std;`;
- duplicate local import aliases.

It does not perform semantic checking of imported items.

### 5.3 `nia-driver`

Loads source files, builds the import graph, computes public surfaces, and
schedules whole-program checking and codegen. It owns the cross-module pipeline.
The import graph may contain cycles; concrete semantic cycles are diagnosed by
the phase that owns the affected construct.

The driver should remain an orchestrator. It should not become a semantic
analysis crate.

## 6. Type Frontend

### 6.1 `nia-type-resolve`

Resolves type names in AST type syntax to definition identities or primitive
types. It validates type paths and generic names but does not lower them into
canonical type ids.

### 6.2 `nia-ty`

Defines the compiler type model and `TyInterner`. Type identities used by later
phases are interned as `TyId`.

### 6.3 `nia-type-lower`

Lowers AST type references into `TyId`s. It handles primitive types, pointers,
arrays, slices, function pointer types, nominal types, generics, enum backing
types, and inferred array lengths.

It also validates type-level restrictions such as invalid use of `void` or `!`
in value positions.

### 6.4 `nia-item-signatures`

Collects signatures for functions, methods, globals, structs, enums, aliases,
and extension methods after type lowering. Function signatures include whether a
function is `extern`, variadic, generic, and whether it has a body.

This phase intentionally ignores function body semantics.

### 6.5 `nia-type-normalize`

Expands type aliases and canonicalizes type forms where required. It detects
recursive aliases and keeps normalized type information separate from raw lowered
types.

## 7. Name Resolution

### 7.1 `nia-value-resolve`

Resolves value paths and qualified value names that refer to top-level values,
functions, enum variants, and imports. It intentionally defers local variables to
`nia-local-resolve`.

It must understand public surfaces across modules but should not type-check
expressions.

### 7.2 `nia-local-resolve`

Builds local scopes for functions and blocks. It resolves parameters, local
bindings, block-local `using`, deferred expressions, and local identifiers.

It also marks expressions that syntactically act as type prefixes for associated
function calls or enum variant paths.

## 8. Comptime Values, Static Data, Layout, And ABI

### 8.1 `nia-comptime-engine`

Evaluates the pure expression subset used by current compile-time values. It is
an evaluator, not a language semantic pass: it does not load modules, know
visibility, own storage rules, or make backend decisions.

Supported evaluation is intentionally small: integer literals, identifiers
resolved by a caller-provided comptime environment, casts that preserve the
underlying value, and simple arithmetic and bit operations.

### 8.2 `nia-comptime-check`

Consumes language-level semantic tables and uses `nia-comptime-engine` to check
and collect current compile-time values. It owns `comptime` binding dependency
resolution, cycle diagnostics, enum discriminant values, and array length values
that depend on local or imported comptime bindings.

This crate is the semantic boundary for current compile-time value requirements.
It is separate from static storage because `comptime` bindings have no runtime
storage or address, while top-level `const` and `var` bindings do.

### 8.3 `nia-static-check`

Validates static initializers for top-level storage. It distinguishes static data
from compile-time value bindings. Address initializers are allowed only when they
can be represented as target static relocations.

### 8.4 `nia-static-ir`

Defines the static/global initialization IR. It represents compile-time data,
not executable runtime control flow. It supports zero values, scalars,
strings/bytes, arrays, repeats, structs, null pointers, global addresses, and
function addresses.

Static address paths use static-only elements such as field ids and constant
indices. They must not carry source-shaped body expressions or runtime places.

### 8.5 `nia-layout`

Computes ABI-relevant layout for primitive, pointer, array, struct, enum, and
instantiated nominal types. It uses explicit target data layout assumptions, such
as LP64, rather than hidden host assumptions.

### 8.6 `nia-abi-check`

Checks C ABI boundaries for `extern` functions, globals, and structs. It rejects
Nia-only types that cannot be passed directly through the C ABI, such as slices,
arrays by value, closed enums where not supported, `bool`, `char`, and variadic
function pointers.

It also rejects unsupported extern forms, including variadic extern definitions
with bodies.

## 9. Control Flow And Body Checking

### 9.1 `nia-flow-check`

Checks flow-sensitive structural rules:

- missing returns;
- unreachable statements;
- `break` and `continue` outside loops;
- invalid control flow in deferred expressions;
- switch duplicate defaults and duplicate patterns.

It should not perform full type checking.

### 9.2 `nia-body-check`

Type-checks function bodies and expression semantics. It owns:

- local binding type checks;
- assignment target validation;
- pointer mutability and addressability checks;
- array-to-slice coercions;
- indexing, slicing, field access, and method calls;
- function calls and generic argument inference;
- enum casts and switch exhaustiveness;
- builtin expression typing;
- inline assembly configuration validation.

Body checking consumes earlier tables instead of rediscovering definitions or
types from source text.

It produces `nia-body-ir` as the stable body semantic boundary. Later phases
consume that IR instead of reading ad hoc body-check side tables or
rediscovering expression semantics from AST shape.

### 9.3 `nia-body-ir`

Defines the checked body semantic IR. It stores resolved, typed body facts that
later phases may consume, including expression types, final bracket-suffix
resolution, builtin values, call targets, coercions, function references, local
types, and recorded generic instantiations. It also defines the typed body,
statement, expression, place, call, aggregate, inline assembly, and control-flow
nodes produced after body checking.

This crate is source-shaped: blocks, if expressions, switch expressions, and
for headers still reflect the checked language form. It is not an optimization
MIR and does not own diagnostics or checking policy. It is the durable data
product of body checking and the input boundary for later lowering,
monomorphization, and backend phases.

### 9.4 `nia-function-ir`

Defines the lowered function body IR used by backend codegen: function-level
blocks, scopes, operations, terminators, places, callees, locals, builtin
values, inline assembly, and runtime expressions.

Function IR is the current function backend boundary. It removes source-shaped
control expressions from runtime expression trees: block, if, switch, for,
return, break, continue, and defer behavior is represented through blocks,
terminators, scope edges, and defer bodies. LLVM codegen consumes this IR rather
than rediscovering control-flow or place semantics from typed AST-shaped nodes.

`nia-function-ir` is a data and validation crate. It does not depend on
`nia-body-ir` and does not own the lowering pass from checked body IR. Its
validator checks IR invariants such as unique ids, valid block/scope/local
references, valid terminator successors, and recursively valid defer bodies.
This catches broken lowerers before backend codegen starts emitting LLVM.
The LLVM backend also runs this structural validator at the Backend IR boundary
so invalid Function IR is reported before LLVM blocks or instructions are
created.

Function IR is deliberately separate from static/global initialization. Static
initializers describe compile-time data for storage; function IR describes
runtime executable control and value flow.

### 9.5 `nia-function-lower`

Lowers `nia-body-ir::TypedBody` into `nia-function-ir::FunctionBody`. This crate
owns the translation from source-shaped checked bodies into explicit function
blocks, scope edges, terminators, defer bodies, locals, builtin values, and
inline assembly options.

This split keeps the Function IR data model reusable by validation, analyses,
backend lowering, and codegen without making the IR crate depend on the
source-shaped body IR that currently feeds it.

## 10. Monomorphization And Symbols

### 10.1 `nia-monomorphize`

Collects concrete generic function and method instances required by the checked
program. It deduplicates exact instance keys for symbol uniqueness and uses
recursive-expansion guards to diagnose cycles.

### 10.2 `nia-mangle`

Builds deterministic internal symbol names from module ids, definition ids, and
type encodings. It is not C++ or Rust mangling. It should stay readable and
debuggable.

Extern symbols bypass internal mangling and use their source names.

## 11. Backend IR

### 11.1 `nia-backend-ir`

Defines backend program, module, item, layout, static initializer, function IR,
and monomorphized instance structures consumed by codegen. Function bodies use
`nia-function-ir::FunctionBody`.

Backend IR is lower-level than AST and contains type-checked, resolved program
structure. It is not a single full-program MIR. It is the module-level backend
container around specialized IRs:

- `FunctionBody` for runtime function execution;
- `nia-static-ir::StaticInit` for global/static storage initialization;
- layout and ABI-ready item metadata for codegen.

Backend IR should be explicit enough for LLVM codegen without forcing codegen to
re-run semantic analysis.

Before LLVM emission, `nia-codegen-llvm` validates Backend IR for Function IR
structure, cross-module function/global references, static initializer address
paths, aggregate field/variant references, evaluated array lengths, and ABI
layouts needed by runtime values.

### 11.2 `nia-backend-lower`

Lowers checked modules into backend IR. It uses definitions, lowered types,
signatures, layouts, body-check results, monomorphized instances, and public
module information.

It owns translation from semantic expressions into typed backend expressions,
places, statements, static initializers, and inline assembly operands.

Backend lowering may temporarily use `nia-body-ir` typed bodies for
monomorphization and instance discovery, but typed bodies are not exposed through
backend IR as the function codegen boundary.

### 11.3 Static Initializer IR

`nia-static-ir::StaticInit` is the static/global initialization IR. It is a data
IR, not a body IR node. Body checking creates it for top-level storage
initializers after static-check has accepted the source expression.

Static initializer checking and lowering stay separate from `nia-function-ir`
because their invariants are different:

- static init must be representable as data before program execution;
- it cannot contain runtime control flow, defer, local storage, or runtime calls;
- it must preserve physical aggregate layout for codegen;
- it may reference globals/functions through statically valid address paths.

This separation is intentional. A future constant/data IR may refine
`StaticInit`, but it should remain a data-initialization boundary rather than
being folded into function IR.

## 12. LLVM Backend

### 12.1 `nia-llvm`

Provides thin wrappers around LLVM APIs. It should keep unsafe and FFI-heavy LLVM
interaction isolated from language phases.

### 12.2 `nia-codegen-llvm`

Emits LLVM IR, objects, and native codegen units from backend IR. It owns:

- LLVM type construction;
- function declarations and definitions;
- globals and static initializers;
- instruction emission;
- control flow lowering;
- defer lowering;
- aggregate operations;
- inline assembly emission;
- object emission.

Backend lowering caches generic type instantiations while expanding function
instances so repeated uses of the same type under the same substitutions do not
rebuild the same interned type graph.

LLVM codegen caches module-local layout queries so repeated aggregate ABI and
field-access decisions do not rescan generic layout instance lists.

LLVM object emission maps the Nia optimization level to LLVM's codegen
optimization level. Size-oriented levels (`-Os` and `-Oz`) also remain visible in
the Nia policy so monomorphization, inlining, specialization, and deduplication
can make size-aware decisions before LLVM sees the program.

It should not parse AST or make frontend semantic decisions.

## 13. CLI

### 13.1 `nia-cli`

The package is `nia-cli`. The installed binary name is `niac`.

The CLI supports:

```text
niac lex <file.nia>
niac parse <file.nia>
niac check <file.nia>
niac emit llvm <file.nia>
niac emit obj <file.nia> [-o file.o | --out-dir dir]
niac emit exe <file.nia> [-o executable]
```

Global module-map options:

```text
-M name=path
--module name=path
```

Global optimization options are listed explicitly in CLI help:

```text
-O
-O0
-O1
-O2
-O3
-Os
-Oz
```

`niac check --emit-opt-report` prints the active optimization policy and backend
optimization changes to stdout. `niac emit llvm --emit-opt-report` prints the
same report to stderr while leaving stdout as LLVM IR, which is useful when
reviewing pass behavior next to emitted code.

`emit obj` may produce multiple object files because backend lowering can produce
multiple codegen units. `-o` is only valid for single-unit output; `--out-dir` is
the multi-unit form. `emit exe` uses host linking and is therefore part of the
host execution model.

## 14. Diagnostics

Every phase returns diagnostics instead of panicking on user source errors.
Diagnostics should carry spans whenever source text is involved.

Implementation bugs may panic in tests, but normal invalid Nia programs should
flow through diagnostic reporting.

Backend IR is validated before LLVM emission. If lowering or stale query state
leaves invalid Function IR, unresolved array lengths, missing owner modules,
missing references, invalid static initializer paths, or missing ABI layouts in
runtime positions, LLVM codegen reports diagnostics at that boundary instead of
letting backend-specific lowering fail later.

Diagnostics should describe current language rules. The compiler should not keep
special migration diagnostics for syntax that only existed during earlier
experimental development.

## 15. File And Module Granularity

Each source file is one module. The import graph is file-based. There is no
language-level package manager or module declaration syntax.

Cross-module references should go through import aliases, public surfaces,
qualified paths, and stable `GlobalDefId`s. Phases should avoid storing direct
filesystem paths as semantic identity.

Import cycles are not errors by themselves. Modules in a cycle keep separate
`ModuleId`s and source paths, and references still go through explicit import
aliases and normal visibility checks. Recursive aliases, comptime dependencies,
layouts, generic expansion, or re-export chains remain concrete semantic errors
for their owning phases.

## 16. Evolution Rules

Nia is pre-1.0, so temporary historical forms are not compatibility
requirements. Once behavior is removed, tests and diagnostics should either
delete it or treat it as ordinary invalid syntax.

New features should be added by extending the correct phase boundary:

- syntax belongs in lexer/parser/AST;
- names belong in definition and resolution phases;
- type identity belongs in type lowering and interning;
- body semantics belong in body check;
- backend representation belongs in backend lowering and backend IR;
- target code belongs in codegen.

Do not add features by tunneling around the pipeline.

## 17. Design Principles

These principles guide future maintenance:

- prefer explicit language rules over hidden runtime policy;
- keep host and bare output models separate;
- keep C ABI interop direct but not contagious into normal Nia symbols;
- keep compile-time value bindings separate from static storage;
- prefer small, inspectable tables over large mutable world objects;
- prefer readable symbols and IR over compact but opaque encodings;
- keep the language small enough that the compiler can remain understandable.
