# Nia Compiler Architecture

Status: implementation architecture reference

This document describes the compiler implementation architecture for Nia. It is
not the language specification; language behavior is defined in
[language-spec.md](language-spec.md). This file explains crate responsibilities,
data flow, phase boundaries, and the design rules used to keep the compiler
maintainable.

## 1. Architecture Goals

The compiler is a typed query graph with explicit lowering boundaries. Each
crate owns one clear kind of data, accepts only the inputs it needs, and
produces immutable tables that dependent queries consume explicitly.

Primary goals:

- keep syntax, name resolution, typing, layout, lowering, and codegen separate;
- centralize compilation identity and semantic storage without giving analysis
  crates unrestricted access to mutable global state;
- pass data through typed ids, immutable tables, and diagnostic lists;
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
- analysis phases mutating unrelated entries in a shared world table instead of
  going through typed store and query APIs;
- bypassing existing phases for temporary features by reinterpreting AST in the
  backend.

## 2. Query Flow

The current whole-program query flow is:

```text
source files
  -> lexer
  -> parser / AST
  -> module item tree
  -> active item surface
  -> definition collection
  -> using graph / using aliases
  -> type name resolution
  -> type lowering
  -> item signatures
  -> type normalization
  -> value path resolution
  -> local name resolution
  -> const value checking
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

The driver requests top-level query products. Individual crates do not load
files, schedule whole-program work, or call later backends. Some arrows above
are query dependencies rather than mandatory eager stages; for example a future
active item surface query can ask const branch queries which in turn depend
on already-lowered declaration surfaces.

Optimization is configured separately from the phase graph. The CLI accepts
`-O0`, `-O1`, `-O2`, `-O3`, `-Os`, `-Oz`, and `-O` as `-O2`; these levels are
lowered into a Nia `OptimizationPolicy` before query execution. The policy is
threaded through compiler-query, backend lowering, and LLVM codegen even when a
phase has no optimization pass yet. This keeps future Nia IR passes from
depending directly on LLVM's smaller codegen-only optimization enum.
LLVM codegen separately reports both an optimization level and a size policy:
`Os` maps to LLVM's default codegen level with a small-size policy, while `Oz`
maps to LLVM's less-aggressive codegen level with a tiny-size policy.

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
  deduplication visible at this boundary; the current implementation always
  performs exact-key deduplication as a correctness invariant for symbol
  uniqueness. `Os`/`Oz` therefore do not disable or reinterpret exact-key
  deduplication; they reserve the policy boundary for future stronger
  size-oriented cross-instance deduplication. Nested type arguments discovered while
  expanding generic bodies are substituted through a per-module working
  interner and a substitution-id cache, so recursive pointer, slice, array,
  nominal, and projection shapes are instantiated once for a given substitution
  map instead of cloning interners for every edge. The substitution id is built
  directly from the effective generic parameter order, avoiding a clone-and-sort
  pass for every nested generic edge.
- `nia-backend-lower` consumes the policy while lowering function bodies into
  backend IR and delegates function-local Function IR cleanup to
  `nia-function-opt`. Function passes are selected from policy capabilities, not
  directly from the user-facing level. Cheap dead-code elimination enables same-type cast
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
  Full local-copy propagation enables local copy propagation, including
  independent propagation inside defer bodies. Aggressive constant folding plus
  aggressive local propagation enables local constant propagation for O3,
  including inside defer bodies, but not for size-oriented modes. `O3` also
  enables a conservative module-level cross-function constant propagation pass:
  calls to no-argument leaf functions or function instances that return a
  backend constant are replaced with that constant and reported separately from
  leaf inlining. Full dead-code elimination enables overwritten-store cleanup,
  never-read local store cleanup, and unused user local binding cleanup. Full
  constant folding or size-oriented policy also canonicalizes static
  initializers: all-zero static data becomes `Zero`, and repeated array,
  byte-string, and char-string data becomes `Repeat`.
  Module-level leaf inlining is gated by `inline_threshold`: `O0` disables it,
  `O1` inlines only no-argument leaf functions that return a backend constant.
  `Os` and `Oz` also inline single-parameter forwarding wrappers that return
  the parameter unchanged, which removes trivial wrapper/thunk calls without
  copying expression trees or dropping evaluation of unused arguments.
  Size-oriented levels deliberately keep other non-constant pure leaf returns
  as calls to avoid duplicating aggregate or expression trees.
  `O2`/`O3` additionally inline small pure leaf expressions under a fixed
  expression-cost budget. `O3` uses a larger aggressive budget than `O2`, so it
  may inline larger pure no-argument leaf returns that `O2` deliberately leaves
  as calls. `O2`/`O3` may substitute parameter locals when every call argument is
  itself a small pure expression and the substituted result still fits the
  active budget. Calls to monomorphized function instances are also gated by
  `specialize_generics`: `Normal` and `Aggressive` may use the active inline
  budget, while `SizeAware` and `RequiredOnly` restrict instance inlining to
  constant leaf returns and single-parameter forwarding wrappers so
  size-oriented and required-only policies do not copy non-constant generic
  bodies into every call site. The pass does not inline calls with effectful
  arguments except for those single-argument forwarding wrappers where the
  argument expression is moved unchanged into the call result. It still rejects
  inline assembly, address-taking, assignments, trait-object conversions, or
  references to non-parameter callee locals.
  `O3` also enables a conservative direct trait-call devirtualization pass. It
  rewrites a dynamic trait method call only when the receiver is syntactically a
  trait-object coercion from a known concrete type and trait resolution finds a
  unique non-generic implementation method. The pass leaves parameter-carried
  trait objects, ambiguous implementations, generic implementation instances,
  and ordinary vtable dispatch unchanged.
- `nia-backend-lower` records a lightweight optimization report alongside the
  lowered backend program. The CLI report starts with the selected
  `OptimizationPolicy` summary, including the monomorphized-instance
  deduplication and size-preference switches, plus the LLVM codegen
  optimization level selected by `nia-codegen-llvm`, enabled backend module,
  function-body, and global pass inventories, and a `changes=<n>` summary. It
  then lists each changed pass and the affected function or global context,
  including whether a function body was a monomorphized instance. This is the
  stable observability hook for reviewing pass behavior without embedding full
  before/after IR snapshots in normal compiler output.
  `nia check <file.nia> --opt-report` prints this report for direct CLI
  inspection. `nia check <file.nia> --runtime freestanding` checks with the
  same startup runtime that `emit --exe` injects.
  `nia emit --checked <file.nia> --opt-report`,
  `nia emit --backend <file.nia> --opt-report`,
  `nia emit --llvm <file.nia> --opt-report`,
  `nia emit --obj <file.nia> --opt-report`, and
  `nia emit --exe <file.nia> --opt-report` write the same report to stderr
  so stdout remains machine-readable backend IR or LLVM IR and native emit
  targets keep object/executable output file-only.
  Dedicated `--emit-*-before-opt` / `--emit-*-after-opt` snapshots are not
  implemented yet; reviewers should currently use `emit --backend` for the final
  optimized backend IR and `--opt-report` for pass inventory and change
  attribution.
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
  already-discovered instances. Generic type instantiation interns the active
  substitution map once and keys recursive type-instantiation cache entries by
  that compact substitution id, avoiding repeated clone-and-sort work while
  expanding nested generic types. Extension trait-method resolution filters by
  method name, trait id, and trait-argument arity before importing extension
  trait arguments for structural matching, avoiding repeated temporary argument
  lists for unrelated methods; builtin operator dispatch uses the same cheap
  filtering shape before checking extension method trait arguments. Once a
  candidate method matches, target type-pattern matching runs once and reuses
  the resulting generic substitutions for instance argument construction.
  Generic parameter lists discovered from extension target types are cached by
  target type id, avoiding repeated recursive scans across trait-method and
  builtin-operator resolution.
  Function-instance discovery caches whether each lowered type contains generic
  parameters, so repeated instance-call scans do not recursively re-walk the
  same nested type shapes while rejecting still-generic call arguments.
  Module-level DCE also builds per-pass indexes from function ids and instance
  refs to bodies, then walks transitive reachability with queues instead of
  repeatedly scanning every lowered function for each discovered reference.
- `nia-codegen-llvm` maps the Nia level to LLVM's codegen optimization level.
  Size-oriented policy remains visible outside LLVM for Nia-level inlining,
  specialization, static-data canonicalization, vtable deduplication, and
  future code-size decisions that happen before LLVM emission.
- `nia-codegen-llvm` also builds a whole-program index before validation and
  emission. This is compiler-throughput infrastructure rather than a generated
  code optimization: repeated module, item, function-instance, vtable, and
  layout lookups should use the index instead of rescanning backend modules on
  each query. Exact instance-layout keys are indexed as a fast path, enum
  variants are indexed with their owning enum and ordinal for emission, trait
  object vtables are indexed both by exact object type and by object trait for
  bounded cross-interner fallback, and type-layout lookup is served directly
  from the index. Module codegen also memoizes trait-object vtable global
  lookup results after the exact key fast path, so repeated coercions avoid
  rescanning declared vtables. Structural type-argument matching is retained as
  a fallback for cross-interner cases, and module codegen memoizes structural
  type equality pairs so repeated layout, function-instance, and vtable fallback
  lookups do not recursively compare the same nested types. Function-instance
  declarations derive
  their LLVM function type directly from the signature helper, so declarations
  do not need to construct temporary backend function bodies or clone instance
  bodies. Function-instance body emission also uses a borrowed codegen signature
  view, avoiding temporary `BackendFunction` construction and cloned params or
  function bodies for every monomorphized instance. ABI parameter
  classification accepts type iterators directly, so parameter storage and call
  lowering do not need temporary vectors just to classify argument passing.
  Regular function, extern function, function-instance, function-pointer, and
  dynamic trait calls lower arguments directly from their backend-IR argument
  slices; only method calls that prepend an explicit receiver build a small
  borrowed argument list.
- `nia-codegen-llvm` performs local ABI-lowering cleanup while preserving the
  backend IR contract. Aggregate literals stored into locals, returned through
  Nia hidden out pointers, or passed as Nia indirect readonly arguments are
  materialized directly into the destination storage instead of building a
  separate literal temporary and copying it again. Aggregate-return calls used
  immediately as local stores or indirect arguments reuse the destination
  storage as their hidden return pointer.
- Backend lowering canonicalizes static initializers under full constant
  folding or size-oriented policy. All-zero static data becomes
  `StaticInit::Zero`; repeated array, byte-string, and char-string data becomes
  `StaticInit::Repeat`. Static initializer emission may choose cheaper
  equivalent LLVM constants such as `zeroinitializer`; repeated byte-array
  initializers are emitted as LLVM constant strings when possible instead of
  rebuilding element-by-element arrays. These forms must preserve the documented
  Nia static data representation and ABI layout.

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

Defines typed cross-phase ids:

- `ModuleId`;
- `DefId`;
- `LocalId`;
- the current module-interner `InternedTyId`;
- `GlobalDefId`.

It stores no semantic tables and has no filesystem, parser, or diagnostic
responsibility.

### 3.5 Semantic Identity Lifecycle

Nia's target type identity is one `TyId` index space owned by a compilation
session. It follows these rules:

- `TyId` is a typed, session-local index, not a module id and not a stable
  cross-process key. Primitive and structural types have no source-module owner;
  nominal `TyKind` data refers to its definition identity explicitly.
- Type slots are immutable and append-only for the session lifetime. A source
  revision may intern new types but never changes the meaning of an existing
  index and never reuses a removed index. Old unreachable entries are reclaimed
  when the session is dropped, not during ordinary incremental updates.
- A runtime store identity distinguishes handles from different compiler
  sessions. Store APIs reject a handle from another store. This is the dynamic
  stale-handle boundary for owned query products until all internal products can
  be tied to the session lifetime statically.
- Revision invalidation applies to facts that map syntax and definitions to
  types. It does not renumber the type store. Rebuilding every type handle on
  each revision would discard the identity stability needed by incremental
  queries.
- Persistent caches use a separate canonical `StableTyKey` derived from stable
  definition/source identity and structural type data. A stable key is converted
  to a session-local `TyId` at the cache boundary; it is not the compiler's hot
  path handle.
- Query execution may append through a typed store API, but analysis crates do
  not receive general mutable access to the semantic context. The store owns
  synchronization, canonicalization, and any future sharding policy.

The migration has established a compilation-owned `TypeStore`: a
`TyInternerId` now contains a unique `TypeStoreId` as well as its temporary
module shard, and every type-lowering and normalization variant appends through
the same store. Normalization is one algorithm over an explicit mutable
interner and explicit input handles; compiler query providers supply the store
transaction, while standalone algorithm tests use that same API. This prevents
handles from separate compiler sessions with equal `ModuleId`s from comparing
equal and prevents normalization variants from creating private divergent
interners. It does not yet satisfy the final contract because the module shard
remains part of the handle and query products still carry cloned interner
snapshots for unmigrated consumers. Migration next moves traits/body, then
layout, monomorphization, backend lowering, and codegen. A domain is migrated
only when its private mutation/import path is deleted. Temporary snapshots
exist only at the boundary between the migrated prefix and the next unmigrated
domain; there is no permanent module-local/session-global API choice.

Trait solving no longer creates a frozen clone of its mutable working interner
for enum classification. The solver holds enum metadata and classifies nominal
types directly through the same interner it mutates, so types appended during
solving remain visible. This is the first trait/body migration slice, not the
completion of that domain: body checking still owns a working interner snapshot,
while const checking still snapshots foreign modules and imports cross-module
signature types.

Const and body providers mutate the compilation-owned `TypeStore` module shard
directly. Array lengths, enum values, values, typed const facts, `ConstCheck`,
and `BodyConst` are ordinary semantic products and no longer carry or transfer
`TyInterner` snapshots. The const analyzer and production body checker borrow
the session shard for their local module, and typed const queries issued while
checking a body append through that same borrow. Temporary working snapshots
are reserved for foreign const modules and explicitly speculative body type
comparison; neither is a second production ownership path.

A store transaction must not invoke a provider that mutates the same module
shard. Providers acquire shared local trait, extension, and function-signature
facts before entering the transaction, while preserving item-level lazy
materialization inside resolvers. `TypeStore` rejects same-thread reentry into
one module shard so an ownership violation becomes an immediate internal error
instead of a mutex deadlock. Foreign const snapshots still select the newer
append-only prefix and report an internal error only when two views diverge.

`BodyIr` currently publishes the one remaining interner snapshot at the
boundary to layout, monomorphization, backend lowering, and codegen. Prechecked
body products and incremental seeds must be prefixes of the session shard and
cannot replace it. The next Phase B slice migrates those downstream consumers
and deletes `BodyIr.interner`; it must not move the snapshot into another
aggregate product or turn speculative snapshots into a permanent dual API.

Function IR lowering has crossed this boundary for mutation. It borrows the
session shard and appends synthesized types, while its single-body and batch
products contain only function IR and diagnostics. Until monomorphization and
backend lowering migrate, the compiler-query provider takes module snapshots
after all function lowering completes and lends them only for the duration of
that downstream call. These snapshots are not query values or semantic
products. The next writable fork to remove is
`MonoCollector.working_interners_by_module`, together with the published
`Monomorphization.type_interners`; only then can backend and reachability stop
using `BodyIr.interner` and `ProgramFunctionBodyInterners` as type stores.

### 3.6 `nia-diagnostic`

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

The lexer also remains available for CLI/debug tooling through
`nia emit --tokens`.
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

### 4.6 `nia-item-tree`

Defines the source item tree used as the first semantic-facing representation of
module contents. It keeps AST syntax out of long-lived semantic tables while
preserving item boundaries, item attributes, visibility, conditional item
attributes, and source spans.

`nia-item-tree` does not evaluate conditional attributes and does not resolve
names or types. It exposes a condition-resolver interface so higher-level
queries can select an active item surface for a target.

The loader records both the raw module item tree and the active item tree for
the current target. Module discovery, definition collection, type-name
resolution, type lowering, item-signature collection, value resolution, and
local resolution consume the active item tree. These phases therefore see a
single declaration surface selected by conditional attributes instead of
reinterpreting a pruned AST module. The raw tree remains available for future
source-addressable inactive-code diagnostics.

This boundary is the long-term replacement for phases directly interpreting
conditional source selection as a module declaration, definition, type, value,
or local-name pre-pass. Inactive items remain represented and
source-addressable; they are not semantically checked for a target unless a
query selects them.

Function bodies are still stored as AST body nodes inside active item-tree
function items until body checking. Declaration-surface phases use
`nia-item-tree`; body-oriented facts are produced later as `BodyFacts` instead
of adding long-lived meaning to AST expressions.

### 4.7 Query Frontend

`nia-loader-query` and `nia-compiler-query` provide the typed query frontend for
source loading, syntax parsing, AST lowering, using graph construction,
item-tree lowering, active item surfaces, definition collection, public
surfaces, and semantic checks. Query keys use source versions where source text
matters, and `nia-query` tracks in-memory dependencies and invalidation.
Public-surface computation builds a temporary
`ModuleId` index over definition collections, so repeated pub-using expansion
and enum namespace validation do not rescan the module list for every segment.
Extension-method collection and visible-extension queries depend on a
program-level type-normalization map query, so extension discovery and
per-module visibility filtering do not rebuild the same normalization map.
Const and body-check providers also depend on program-level module/definition
map queries, so they do not rebuild identical cross-module context maps for each
module.
Executable reachability pruning is intentionally outside the provider file in
`nia-executable-reachability`, and backend lowering input assembly is isolated
behind `query/backend_lowering.rs`; providers should wire query dependencies and
timing boundaries rather than owning program analysis or backend input shape.

The query frontend is batch-friendly. Persistent caches, cross-session reuse,
LSP scheduling, cancellation, and priority handling are separate future layers.

## 5. Definitions And Modules

### 5.1 `nia-defs`

Collects active item-tree definitions into module-local definition tables. It
assigns `DefId`s and tracks namespaces for values, types, modules, enum
variants, and methods.

It detects duplicate names in the same namespace and duplicate generic
parameters. It does not evaluate const conditions, type-check bodies, or load
other files.

### 5.2 `nia-imports`

Builds the explicit module graph and normalizes using paths. It handles:

- package roots such as `using std;`;
- entry-root paths such as `using root::math;`;
- current-package paths such as `using pkg::internal;`;
- child declarations such as `module probe;`;
- parent paths such as `using super::probe;`;
- module cycle diagnostics;
- duplicate local using aliases.

It does not perform semantic checking of selected items.

### 5.3 `nia-driver`

Loads source files, builds the using graph, computes public surfaces, and
schedules whole-program checking and codegen by requesting query products. It
owns orchestration across modules, not semantic interpretation. The using graph
is acyclic; semantic cycles inside loaded modules are diagnosed by the query or
crate that owns the affected construct.

The driver should remain an orchestrator. It should not become a semantic
analysis crate.

## 6. Type Frontend

### 6.1 `nia-type-resolve`

Resolves type names in active item-tree type syntax to definition identities or
primitive types. It validates type paths and generic names but does not lower
them into canonical type ids.

### 6.2 `nia-ty`

Defines the compiler type model and the current module-local `TyInterner`.
During the type-identity migration it will own the compilation-session type
store described in section 3.5; later phases will consume `TyId` without an
interner snapshot.

### 6.3 `nia-type-lower`

Lowers active item-tree type references into interned type ids. It handles
primitive types, pointers, arrays, slices, function pointer types, nominal
types, generics, enum backing types, and inferred array lengths.

It also validates type-level restrictions such as invalid use of `void` or `never`
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

## 8. Const Values, Static Data, Layout, And ABI

### 8.1 `nia-const-ir`

Defines the source-preserving semantic body used for compile-time execution.
This is Nia's const execution surface, not a generic HIR. It stores only the
expression, statement, block, function, parameter, binding, and field forms that
are valid inputs to compile-time evaluation, while preserving the semantic ids
and source spans needed for name, local, type-argument, and diagnostic queries.

AST is lowered into `ConstModule` before execution. A `ConstModule`
contains the module's const enums, global and local const initializers,
`const fn` bodies, and type-level constant expressions. That keeps parser
syntax out of the evaluator and gives the query system a cacheable module-level
boundary for ordinary const values such as user const structs, imported
`const fn` calls, and array length expressions.

Const lowering consumes `SemanticUseTable` from `nia-sema-ir` for source
positions that already have semantic identity: value uses, local definitions,
and type uses. The table is a shared semantic input surface, not a const
resolver. Callers such as `nia-const-check`, `nia-body-check`, and
`nia-static-check` decide which resolved locals and globals are valid in their
context, then pass one table to `nia-const-ir`. This prevents compile-time
lowering from reinterpreting bare names or carrying a parallel set of name,
local, and type lookup closures.

### 8.2 `nia-const-eval`

Evaluates the pure expression subset used by current compile-time values. It
consumes `nia-const-ir` rather than AST. It is an evaluator, not a language
semantic pass: it does not load modules, know visibility, own storage rules, or
make backend decisions.

Supported evaluation is intentionally small: integer, boolean, string, and
struct literal values; identifiers resolved by a caller-provided const
environment; struct field access; casts that preserve the underlying value;
boolean logic; equality over
matching primitive const values; and simple integer arithmetic and bit
operations. It also evaluates visible `const fn` bodies represented as
const semantic bodies. AST lowering is performed by callers such as
`nia-const-check`, `nia-body-check`, static validation, or the early target
pruner before values reach the engine; the engine itself only accepts the
const semantic representation.

### 8.3 `nia-const-check`

Lowers AST plus local/value/type semantic tables into `ConstModule`, then
uses `nia-const-eval` to check and collect current compile-time values. It
owns `const` binding dependency resolution, cycle diagnostics, enum
discriminant values, and array length values that depend on local or imported
const bindings or imported `const fn` calls.
Layout builtins such as `@size[T]()` and `@align[T]()` consume those evaluated
array lengths through narrow lookup closures while computing layouts; they do
not construct ad hoc `ConstCheck` result tables for layout queries.

This crate is the semantic boundary for current compile-time value requirements.
It is separate from static storage because `const` bindings have no runtime
storage or address, while `static` and `static mut` declarations do.

`nia-const-check` also owns the typed const value layer. The engine may
produce a pure value such as an integer, string, array, or struct, but the
checker records the semantic const type when it is known from source-level
semantic tables or builtin declarations. Runtime Nia types are represented as
one case of this typed const layer; pure compile-time structs are represented
structurally and do not have to be forced into the
runtime type interner. This keeps type ownership in the semantic query layer
instead of teaching the evaluator about Nia's type system, while still giving
generic const calls and ordinary user const structs one shared typed
representation.

Typed const bindings are not limited to explicit type annotations. When a
`const` binding has no source annotation, the checker derives its typed
value from the initializer's typed const expression and records that in
`ConstCheck::typed_values`. Cross-module body checking consumes those typed
values through the program const query result and imports runtime types into
the current module interner at the use site. Item signatures remain the source
signature surface; inferred const value types are semantic query output, not
retroactive signature data.

Typed const expression inference belongs to this checker as well. It derives
runtime types for source-shaped const expressions only when the type is a
semantic consequence of the expression and available tables, such as suffixed
integer literals, typed aggregate literals, inferable array literals, structural
field access, and optional constructors whose payload type is
already known. Constructors that need missing context, such as `null` or
one-sided error-union values, remain untyped until an explicit binding,
parameter, or call context supplies the full type.

Propagation expressions use the same typed value surface: when the operand type
is known as `?T` or `E!T`, `operand.?` has payload type `T` for later const
generic inference.

Binary const expressions are typed conservatively from operand types.
Boolean logic and comparisons produce `bool`; integer arithmetic and bit
operations produce the shared operand type only when both operands already have
the same concrete integer runtime type.

Boolean literals and supported unary const operators are typed directly:
`true`, `false`, and `not` produce `bool`, while integer negation preserves the
known concrete integer operand type.

Expected types are an input to this semantic query, not a fallback evaluator
rule. This lets generic const calls infer through partially-known parameter
types, for example `E!T` can type `!value` when `E` is already concrete, while
still refusing to invent the missing half of an error union from the value
shape alone.

File embedding is part of the same semantic boundary. The `@embed("path")`
builtin is lowered into `nia-const-ir`, evaluated by `nia-const-eval`
through the caller-provided const environment, and resolved by
`nia-const-check` against the `SourcePath` of the module currently
executing. It is deliberately not a loader fallback or cwd-relative operation;
cross-module execution receives the producing module's source path through the
program const context.

Const block expressions are typed from their tail expression. A block with
statements creates a typed const scope in the checker, records local binding
types from explicit annotations or inferable initializer expressions, and then
types the tail inside that scope. This remains a semantic typing operation; the
engine still owns value execution and the checker does not execute statement
effects just to discover a block tail type.

Const array literals are typed by expected array context when one exists,
or by peer element inference otherwise. List literals choose an element that
produces a concrete type as the anchor, then type the remaining elements from
that anchor; this lets contextual forms such as `[null, ?value]` infer the
optional element type without treating unresolved generic placeholders as real
types. Repeat literals use the repeated value type and the const repeat
count to build the array runtime type. When no runtime array context exists,
array literals can still be typed as structural const-only arrays. Their
element type is another `ConstValueType`, so arrays of structural const
structs behave like ordinary compile-time data tables and indexed elements can
feed field access and generic const call inference.
Const array slicing is part of the same value surface: slicing a const
array produces another const array value, and the typed const layer
records the sliced element type and known length so the result can feed field
access, indexing, and generic const call inference without becoming a
runtime slice.

Const struct literals have two typed surfaces. When an expected nominal
struct type exists, the checker uses that struct type, substitutes its generic
arguments into the field signature types, infers still-open generic arguments
from concrete field values, and then rechecks all fields with the completed
field types. Without a nominal expected type, the literal is a structural
compile-time-only value: the checker derives each field's const type and
records a `ConstValueType::Struct`. It does not invent anonymous runtime
struct types from field names; structural values stay in the const-only
typed surface.

Const field access consumes both sides of that typed value surface.
Structural const-only structs resolve fields from their structural field
type list, while runtime nominal struct values resolve fields from the nominal
struct signature with generic arguments substituted into the current execution
module. Structural const data stays on the same typed expression path
without forcing anonymous data into the runtime type interner.

Consumers outside `nia-const-check` should use the typed value surface's
accessors for structural field and array element queries instead of duplicating
shape matches. That keeps `nia-body-check` a consumer of typed const facts
rather than a second owner of const expression inference.

The same boundary applies to function-body const execution. `nia-body-check`
may execute lowered `nia-const-ir` expressions while checking body-local
`if` expressions, array lengths, and local `const` bindings, but generic
const-call instantiation is delegated back to `nia-const-check`'s typed
query surface. Body checking provides a typed const frame containing local
binding value types, name aliases, active const function type substitutions,
the current target, and the same program-level type lowering, normalization,
signature, definition, and const-module context available to top-level
const checking. The const checker uses that frame and program context
through `TypedConstQueryInput`, so expression type queries and const
function generic instantiation share one public input surface. This lets
function-local structural const values, imported `const fn` calls, and
imported structural const fields infer generic arguments without growing a
second type-inference implementation in body checking.

`TypedConstQueryInput` borrows existing typed const query output instead
of copying it into a new result table. The checker overlays those borrowed
facts with any typed values produced by the local query, preserving the query
graph boundary while avoiding an accidental clone-based API contract.

Typed const expression inference uses explicit probe helpers when it needs
to ask whether a subexpression can be evaluated as a const integer, array
length, or generic argument source. These probes deliberately return absence
rather than diagnostics: they are used to decide whether a type can be proven
from the current semantic facts, not to validate the program. The checking
paths that own required compile-time behavior still execute the same lowered
`nia-const-ir` through `nia-const-eval` and report engine errors there.
This keeps typed inference from becoming a second diagnostics pass while making
the optional probe boundary visible in code.

Const `if` expressions are typed from branch result types. The then block
tail and else expression are both typed through the same source-shaped const
expression rules, including nested block expressions and contextual
constructors such as `null`. Both selected result shapes must agree on the same
typed const value shape before the `if` expression can feed generic
const call inference: runtime Nia values agree by `TyId`, while structural
const-only structs agree by their field type surface.

Const `switch` expressions follow the same source-shaped typed surface as
runtime source `switch`: value patterns, integer ranges, and default cases.
Value-producing arm bodies are typed and unified to one typed const value
shape, while control-flow-only arms such as `return`, `break`, or `continue`
do not invent a switch result type. Recursive optional and error-union payload
patterns belong to if-pattern expressions; their payload locals are typed from the
target type while checking those arms. The evaluator still performs
actual matching; the checker only records the arm and payload types needed for
const generic inference.

Const function calls are typed by their signatures in the same layer. Generic
type arguments are inferred from typed argument expressions, substituted into
the return type, and then imported into the current execution module's working
interner. This makes nested const calls participate in later generic
inference without executing the callee solely to discover its type.

Early target pruning is intentionally narrower than full const execution: it
can evaluate target builtins and same-module helper functions before the module
graph is complete. It lowers its accepted AST conditions to `nia-const-ir`
locally, but does not participate in program-level const module queries. Full
imported const function execution belongs to the semantic query path after
imports, definitions, values, locals, and const modules are available.

### 8.4 `nia-static-check`

Validates static initializers for `static` storage. It distinguishes static data
from compile-time value bindings. Address initializers are allowed only when they
can be represented as target static relocations.

### 8.5 `nia-static-ir`

Defines the static/global initialization IR. It represents compile-time data,
not executable runtime control flow. It supports zero values, scalars,
strings/bytes, arrays, repeats, structs, null pointers, global addresses, and
function addresses.

Static address paths use static-only elements such as field ids and constant
indices. They must not carry source-shaped body expressions or runtime places.

### 8.6 `nia-layout`

Computes ABI-relevant layout for primitive, pointer, array, struct, enum, and
instantiated nominal types. It uses explicit target data layout assumptions, such
as LP64, rather than hidden host assumptions.

### 8.7 `nia-abi-check`

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
- ordinary control-flow validity inside deferred expressions;
- switch duplicate defaults and duplicate syntactic patterns.

It should not perform full type checking.

### 9.2 `nia-body-check`

Type-checks function bodies and expression semantics. It owns:

- local binding type checks;
- assignment target validation;
- pointer mutability and addressability checks;
- literal and pointer array-to-slice coercions;
- indexing, slicing, field access, and method calls;
- function calls and generic argument inference;
- enum casts, switch exhaustiveness, and switch range-pattern validation;
- builtin expression typing;
- inline assembly configuration validation.

Body checking consumes earlier tables instead of rediscovering definitions or
types from source text.

It produces two explicit products:

- `BodyFacts`, the body semantic surface: expression types, final
  bracket-suffix resolution, builtin values, call targets, coercions, function
  references, local types, generic instantiations, source-node fact keys, plus
  checked const-derived facts such as array repeat counts and switch pattern
  values;
- `BodyIr`, the runtime checked body boundary: typed function bodies, static
  initializers, and the interner required to interpret those typed bodies.

Later phases consume these products explicitly instead of reading ad hoc
body-check side tables or rediscovering expression semantics from AST shape.
Runtime body lowering is a consumer of `ConstCheck` and program const
query results. It must not implement a second imported-const evaluator; an
imported `const` value used in a runtime expression is read from the producing
module's `ConstCheck`, while body-local compile-time execution remains in the
body checker and delegates typed const queries back to `nia-const-check`.

### 9.3 `nia-body-ir`

Defines checked body data products:

- `BodyFacts` is the Nia body semantic surface. It stores resolved, typed body
  facts that later phases may consume, including expression types, final
  bracket-suffix resolution, builtin values, call targets, coercions, function
  references, local types, compile-time branch selections, source-node fact
  keys, checked array repeat counts, checked switch pattern values, and recorded
  generic instantiations.
- `BodyIr` is the runtime checked body product. It stores typed function
  bodies, static initializers, and the type interner used by those bodies.
- The crate also defines the typed body, statement, expression, place, call,
  aggregate, inline assembly, and control-flow nodes produced after body
  checking.

This crate is source-shaped: blocks, if expressions, switch expressions, and
for headers still reflect the checked language form. It is not an optimization
MIR and does not own diagnostics or checking policy. It is the durable data
product of body checking and the input boundary for later lowering,
monomorphization, and backend phases. `BodyFacts` carries semantic side facts;
`BodyIr` carries runtime typed bodies and static data.
Facts derived from const execution during body checking, such as array repeat
counts and switch pattern values, are recorded here so later lowering can
consume checked facts instead of re-running expression evaluation from source
shape. If body IR lowering does not have a checked integer-pattern fact because
the checker already rejected the pattern or the pattern is not an integer fact,
it keeps the original expression-shaped pattern instead of re-running const
evaluation or producing a second diagnostic.
Integer and boolean switch patterns therefore enter `BodyIr` as checked pattern
values or checked ranges; expression-shaped switch patterns remain only for
patterns whose semantics are not represented by the integer-pattern fact, such
as enum variant references.

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

Lowers `nia-body-ir::TypedBody` from `BodyIr` into
`nia-function-ir::FunctionBody`. This crate owns the translation from
source-shaped checked runtime bodies into explicit function blocks, scope
edges, terminators, defer bodies, locals, builtin values, and inline assembly
options.

This split keeps the Function IR data model reusable by validation, analyses,
backend lowering, optimization, and codegen without making the IR crate depend
on the source-shaped body IR that currently feeds it.

### 9.6 `nia-function-opt`

Optimizes `nia-function-ir::FunctionBody` using only function-local IR, the
Nia optimization policy, and narrow target/layout facts supplied by the caller.
It owns Function IR pass ordering, policy gating, local CFG cleanup, local copy
and constant propagation, dead-store and unused-local cleanup, pure wrapper
removal, same-type cast cleanup, defer-body CFG cleanup, and the shared
recursive Function IR traversal helpers used by those passes.

This crate is deliberately not a backend program optimizer. It does not know
about backend modules, symbol mangling, reachability, monomorphized function
instance queues, trait object vtables, global/static initializers, or layout
tables. When a pass needs target facts, such as whether a lowered type is
zero-sized, the caller supplies a narrow callback. `nia-backend-lower` consumes
this crate as an adapter boundary: it provides layout facts, records backend
optimization report entries, and keeps module-level backend transforms separate
from function-local optimization.

### 9.7 `nia-executable-reachability`

Computes the module and function set required for freestanding executable
codegen. It starts from the root `main` and freestanding `_start` runtime roots,
then walks typed bodies, semantic facts, trait/default methods, extension
witnesses, and owner modules for referenced types.

This crate deliberately returns reachability sets instead of owning
`CheckedModule`. `nia-compiler-query` keeps query orchestration and module
filtering, while this crate owns the executable pruning analysis. That boundary
prevents provider code from becoming a typed-body traversal or semantic-fact
analysis module.

## 10. Monomorphization And Symbols

### 10.1 `nia-monomorphize`

Collects concrete generic function and method instances required by the checked
program. It deduplicates exact instance keys for symbol uniqueness and uses
recursive-expansion guards to diagnose cycles. This exact-key deduplication is
a required correctness invariant at every optimization level; the
`dedup_monomorphized_instances` policy switch reports that the monomorphization
boundary participates in size policy, but it does not make exact-key
deduplication optional. Future size-oriented passes may use this boundary for
stronger cross-instance deduplication that preserves symbol identity.

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
signatures, layouts, `BodyFacts`, `BodyIr`, monomorphized instances, and public
module information.

It owns translation from semantic expressions into typed backend expressions,
places, statements, static initializers, and inline assembly operands.

Backend lowering consumes body semantic facts for expression types, call
resolution, and coercions, and consumes typed runtime bodies for function
lowering. Typed bodies are not exposed through backend IR as the function
codegen boundary.

### 11.3 Static Initializer IR

`nia-static-ir::StaticInit` is the static/global initialization IR. It is a data
IR, not a body IR node. Body checking creates it for `static` storage
initializers after static-check has accepted the source expression.

Static initializer checking and lowering stay separate from `nia-function-ir`
because their invariants are different:

- static init must be representable as data before program execution;
- it cannot contain runtime control flow, defer, local storage, or runtime calls;
- it must preserve physical aggregate layout for codegen;
- it may reference globals/functions through statically valid address paths.

When body checking materializes accepted static initializers into
`StaticInit`, any required compile-time integer or static-address index is
lowered through the same `nia-const-ir` surface and evaluated through a
single static-initializer helper. That helper is part of static data
materialization, not a general backend escape hatch for reinterpreting AST.

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
rebuild the same interned type graph. Substitution maps are interned before
recursive instantiation, so cache keys can carry a compact substitution id
instead of repeatedly cloning and sorting the same name-to-type map.

LLVM codegen uses the whole-program index for layout queries so repeated
aggregate ABI and field-access decisions do not rescan generic layout instance
lists. Function-instance declarations also reuse the signature type builder
instead of cloning instance bodies just to discover their LLVM function type.
The backend validator also memoizes layout probes during pre-codegen validation,
so repeated runtime-type checks do not recursively recompute the same ABI
layout. Its structural type equality fallback uses the same pair-cache shape as
module codegen, keeping generic instance validation from repeatedly comparing
the same cross-interner type arguments.

LLVM object emission maps the Nia optimization level to LLVM's codegen
optimization level and a reported codegen size policy. Size-oriented levels
(`-Os` and `-Oz`) also remain visible in the Nia policy so monomorphization,
inlining, specialization, and deduplication can make size-aware decisions before
LLVM sees the program. Today the native LLVM target machine is configured with
the mapped codegen optimization level; the size policy is reported and preserved
at the Nia/codegen boundary for size-aware Nia lowering and future target-option
plumbing rather than being a separate LLVM target-machine knob.

It should not parse AST or make frontend semantic decisions.

## 13. CLI

### 13.1 `nia-cli`

The package is `nia-cli`. The compiler binary name is `nia`.

The CLI supports:

```text
nia build [step] [--root dir]
nia check <file.nia> [--runtime bare|freestanding] [--opt-report]
nia emit --tokens <file.nia>
nia emit --ast <file.nia>
nia emit --checked <file.nia> [--runtime bare|freestanding] [--opt-report]
nia emit --backend <file.nia> [--runtime bare|freestanding] [--opt-report]
nia emit --llvm <file.nia> [--runtime bare|freestanding] [--opt-report]
nia emit --obj <file.nia> [-o file.o | --out-dir dir] [--runtime bare|freestanding] [--opt-report]
nia emit --exe <file.nia> [-o executable] [--runtime freestanding] [--link-arg arg] [--opt-report]
```

`nia build` is the package-build entry point and searches for `build.nia` from
the selected root directory and then each parent directory. Build outputs belong
under `.nia-build/`; reusable package or compiler cache entries belong under
`.nia-cache/`. The build runner is part of the Nia toolchain boundary so package
build scripts do not spell out LLVM, C runtime, or linker dependency details.
The build runner is implemented inside the Rust toolchain crates and calls the
compiler through `nia-driver`; it does not embed the compiler through a separate
ABI bridge.

`build.nia` is compiled and run as ordinary Nia code. The Rust toolchain only
owns package-root discovery, runner generation, and compiler invocation; build
logic stays in the Nia build script and can use `std`, including
`std::process`. The generated runner injects package-root context into
`std::build::Build`: `package_root()` is the directory containing `build.nia`,
`build_dir()` is `.nia-build/`, `cache_dir()` is `.nia-cache/`, and
`toolchain_executable()` is the `nia` executable that launched the build. The
toolchain creates the build and cache directories before executing the runner.
The current `std::build` surface is intentionally small:
`add_module(ModuleOptions::init(root_source))` records a root source module and
`add_executable(ExecutableOptions::init(name, root_module))` records a
script-owned executable artifact. `ModuleOptions::with_optimization`,
`ExecutableOptions::with_output_name`, and `ExecutableOptions::with_runtime`
customize those records without exposing raw compiler argv assembly.
`add_check_executable_step(name, target)` and
`add_emit_executable_step(name, target)` register graph steps that route that
artifact through the current toolchain. Emitted executables currently land at
`.nia-build/<output-name-or-target-name>`, with target names validated so
artifact paths cannot escape the build directory.
`set_default_step(step)` makes the no-argument build entry explicit; the runner
does not infer a default from step registration order.
This surface grows the build system through explicit step and artifact APIs
rather than a Rust-side manifest parser.

Path construction follows the standard-library view/buffer convention:
`std::StringView` and `fs::PathView` are borrowed `&[char]` views, while
`std::StringBuf` and `fs::PathBuf` own caller-allocated text storage.
`PathBuf::join` and `PathBuf::join_component` append with a native separator
and explicit allocator use.

Global module-map options:

```text
-M name=path
-Mname=path
-M=name=path
--module name=path
--module=name=path
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

Global timing options are also accepted before or after the command:

```text
--timings
--timings=summary
--timings=detail
--timing-trace=events
```

`--timings=detail` reports aggregated query timings. `--timing-trace=events`
also prints raw timing events and is intended for diagnosing timing collection
rather than normal performance measurement.

`nia check <file.nia> --opt-report` prints the active optimization
policy, LLVM codegen optimization level, enabled backend module/function/global
pass inventories, the backend optimization change count, and backend
optimization changes to stdout. `nia check <file.nia> --runtime freestanding`
injects the freestanding startup runtime and checks the same entry contract used
by `emit --exe`.
`nia emit --backend` prints the optimized backend IR to stdout for pass review.
`nia emit --checked <file.nia> --opt-report`,
`nia emit --backend <file.nia> --opt-report`,
`nia emit --llvm <file.nia> --opt-report`,
`nia emit --obj <file.nia> --opt-report`, and
`nia emit --exe <file.nia> --opt-report` print the report to stderr while
leaving stdout as backend IR or LLVM IR, and while keeping native
object/executable output file-only. This is useful when reviewing pass behavior
next to emitted code or native codegen artifacts.
Timing reports are written to stderr; detail mode also includes aggregated query
timings.
The CLI does not yet expose separate before/after backend optimization snapshots;
`emit --backend` is the post-lowering optimized backend IR, and
`--opt-report` is the stable pass-observability interface.
The CLI regression fixture emits and runs the same program at `-O0`, `-O1`,
`-O2`, `-O3`, `-Os`, `-Oz`, and `-O`; it exercises constant leaf inlining,
generic instance calls, local cleanup, and size-safe forwarding wrappers while
checking that the freestanding executable exits with the same value at every
level.

`emit --obj` defaults to the bare runtime and may produce multiple object files
because backend lowering can produce multiple codegen units. `emit --obj
--runtime freestanding` lowers with the same startup injection used by
executable emission. `-o` is only valid for single-unit output; `--out-dir` is
the multi-unit form. `emit --exe` lowers the freestanding runtime-selected
executable model and invokes the configured target linker. Extra linker
arguments are passed with repeated `--link-arg` options.
Native output paths are
mkdir-friendly by design: missing parent directories for `emit --obj -o`,
`emit --obj --out-dir`, and `emit --exe -o` are created before writing or
linking output artifacts. Input paths and module map paths are never created
implicitly.

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

Each source file is one module. Child files are loaded only through explicit
`module name;` or `pub module name;` declarations in the parent module. One
`-M name=path` entry is one pkg root.

Cross-module references should go through using aliases, public surfaces,
qualified paths, and stable `GlobalDefId`s. Phases should avoid storing direct
filesystem paths as semantic identity.

Module cycles are load-time errors. Loaded modules keep separate `ModuleId`s
and source paths, and references still go through explicit using aliases and
normal visibility checks. Recursive aliases, const dependencies, layouts,
generic expansion, or re-export chains remain concrete semantic errors for
their owning phases.

## 16. Evolution Rules

Nia is pre-1.0, so temporary historical forms are not compatibility
requirements. Once behavior is removed, tests and diagnostics should either
delete it or treat it as ordinary invalid syntax.

New features should be added by extending the correct phase boundary:

- syntax belongs in lexer/parser/AST;
- names belong in definition and resolution phases;
- type identity belongs in type lowering and interning;
- compile-time branch selection belongs in item-tree/body semantic queries;
- body semantics belong in body check;
- backend representation belongs in backend lowering and backend IR;
- target code belongs in codegen.

Do not add features by tunneling around query boundaries.

## 17. Design Principles

These principles guide future maintenance:

- prefer explicit language rules over hidden runtime policy;
- keep host and bare output models separate;
- keep C ABI interop direct but not contagious into normal Nia symbols;
- keep compile-time value bindings separate from static storage;
- prefer small, inspectable tables over large mutable world objects;
- prefer readable symbols and IR over compact but opaque encodings;
- keep the language small enough that the compiler can remain understandable.
