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
files, schedule whole-program work, or call later backends. The arrows above are
dependency directions, not a requirement that every stage execute eagerly.

Optimization is configured separately from the phase graph. The CLI accepts
`-O0`, `-O1`, `-O2`, `-O3`, `-Os`, `-Oz`, and `-O` as `-O2`; these levels are
lowered into a Nia `OptimizationPolicy` before query execution. The policy is
threaded through compiler-query, backend lowering, and LLVM codegen. Nia-owned
optimization consumers therefore do not depend directly on LLVM's smaller
codegen-only optimization enum.
LLVM codegen separately reports both an optimization level and a size policy:
`Os` maps to LLVM's default codegen level with a small-size policy, while `Oz`
maps to LLVM's less-aggressive codegen level with a tiny-size policy.

### 2.1 Optimization Levels And Policy

Nia optimization levels are user-facing presets. Internally, each level expands
to a policy matrix with separate decisions for CFG simplification, constant
folding, dead-code elimination, local copy propagation, inlining,
specialization, monomorphized instance deduplication, and size preference.
`nia-opt` owns this declarative matrix, not any optimization implementation.
`O0` still enables transformations required to establish compiler invariants;
performance and size work is selected independently through pass depth,
inlining, specialization, and `prefer_size` fields. Consumers must inspect the
policy rather than infer behavior from the user-facing level name.

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
monomorphization, backend lowering, specialization, and inlining, where
LLVM cannot undo duplicated Nia-level work.

Nia-owned optimization consumers:

- `nia-monomorphize` collects concrete generic instances before backend
  lowering. It pre-indexes instantiations by source definition and caches
  effective generic lists and mangled type symbols during collection so
  repeated generic-instance discovery does not rebuild the same symbol inputs
  or clone whole definition maps. The policy keeps monomorphized instance
  deduplication visible at this boundary. Exact-key deduplication is a
  correctness invariant for symbol
  uniqueness. `Os`/`Oz` therefore do not disable or reinterpret exact-key
  deduplication. Nested type arguments discovered while expanding generic
  bodies are published through the canonical `TypeStore` and memoized by a
  substitution-id cache, so recursive pointer, slice, array, nominal, and
  projection shapes are instantiated once for a given substitution map. The
  substitution id is built directly from the effective generic parameter order,
  avoiding a clone-and-sort pass for every nested generic edge.
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
  simplification additionally folds pure same-target match terminators to
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
  monomorphized vtable entries. A selected trait implementation retains the
  use-module context required to interpret its instance arguments. A trait
  default method instead uses the trait definition module: deriving that
  context from whichever consumer happens to materialize the vtable would give
  one `(self_ty, object_ty)` key conflicting definitions across facades and
  consumers. Vtable-driven generic instance discovery scans
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
  Supertrait obligation expansion uses semantic obligation equivalence for its
  cycle guard as well as its output deduplication. Type aliases and projection
  normalization may allocate distinct interned handles for one goal; raw handle
  hashing is therefore insufficient to terminate or deduplicate expansion.
  Trait-object supertrait traversal applies the same semantic matching to its
  visited path guard, keeping upcast binding collection and target-supertrait
  checks consistent with obligation expansion.
  Trait-solver layout fallback compares nominal const arguments by their
  semantic type and value, not by raw argument handles. Layout products may be
  interned through another module append, and integer const spelling/sign state
  must not make an otherwise identical concrete layout miss.
  Least-fixed-point trait resolution also guards active where-clause goals by
  semantic goal equivalence. Raw `TraitGoal` hashing is insufficient when
  normalization or const evaluation presents the same recursive goal through
  another interned handle or integer spelling.
  Type equivalence's projection resolver likewise tracks active type pairs as a
  path-local vector and compares their non-resolving structural shapes
  semantically. This prevents equivalent rebuilt projection pairs from escaping
  the unresolved-cycle result while preserving independent sibling comparisons.
  Function-instance discovery caches whether each lowered type contains generic
  parameters, so repeated instance-call scans do not recursively re-walk the
  same nested type shapes while rejecting still-generic call arguments.
  Method and trait-method instances use a complete identity: the receiver or
  trait type arguments are followed by the method's own type and const
  arguments in declaration order. Backend dispatch, instance planning,
  reachability, and LLVM fingerprints must preserve both argument groups when
  converting a trait callee to a concrete method or associated-function
  instance; dropping method const arguments can alias distinct symbols.
  Executable-facts extraction has two equivalent inputs, typed Body IR and
  retained semantic facts. Both paths must emit the method's own generic
  instantiation, including const arguments; typed-callee scanning cannot reduce
  a method call to only its trait or receiver identity.
  Function-pointer references follow the same rule: extension target
  substitutions retain both type and const arguments, including const-only
  targets, and BIR lowering must keep those arguments in `FunctionInstance`
  identity rather than collapsing the reference to a bare function.
  Module-level DCE also builds per-pass indexes from function ids and instance
  refs to bodies, then walks transitive reachability with queues instead of
  repeatedly scanning every lowered function for each discovered reference.
- `nia-codegen-llvm` maps the Nia level to LLVM's codegen optimization level.
  Size-oriented policy remains visible outside LLVM for Nia-level inlining,
  specialization, static-data canonicalization, vtable deduplication, and
  code-size decisions made before LLVM emission.
- `nia-codegen-llvm` also builds a whole-program index before validation and
  emission. This is compiler-throughput infrastructure rather than a generated
  code optimization: repeated module, item, function-instance, vtable, and
  layout lookups should use the index instead of rescanning backend modules on
  each query. Exact instance-layout keys are indexed as a fast path, enum
  variants are indexed with their owning enum and ordinal for emission, trait
  object vtables are indexed both by exact object type and by object trait for
  bounded semantic-equivalence fallback, and type-layout lookup is served directly
  from the index. Module codegen also memoizes trait-object vtable global
  lookup results after the exact key fast path, so repeated coercions avoid
  rescanning declared vtables. Structural type-argument matching is retained for
  types whose const expressions normalize to the same value, and module codegen
  memoizes semantic type-equality pairs so repeated layout, function-instance,
  and vtable fallback lookups do not recursively compare the same nested types.
  Once a trait object is accepted, frontend semantic facts and backend vtable
  entry expansion must walk the same complete source-supertrait graph. The
  walk substitutes declaration-ordered type and const arguments at every edge
  and uses both a path-local cycle guard and a per-coercion expanded-instance
  set. Inherited default methods retain their concrete identity, recursive
  graphs terminate, and diamond siblings do not duplicate frontend dependency
  facts even though backend slot expansion preserves its path-shaped layout.
  Function-instance declarations derive
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

Nia-owned optimization consumers follow the same boundary:

- required normalization belongs in the phase that needs the invariant and must
  not depend on a user optimization level;
- optional performance or size transforms must be gated by
  `OptimizationPolicy`;
- ABI-visible transforms must be documented in `docs/nia-abi.md`;
- backend-visible transforms must preserve type layout, parameter and return
  ABI, symbol identity, static data representation, source-level checks, and
  evaluation-order guarantees.

O2 and higher use an explicit backend optimization pipeline rather than an
unstructured list of calls. Each pass has a stable name, documented level
boundary, focused tests, and change attribution so reports explain why a
function changed.

## 3. Foundation Crates

### 3.1 `nia-span`

Defines source spans with byte offsets. It does not depend on AST, lexer,
parser, diagnostics, or any semantic phase.

### 3.2 `nia-source`

Owns source identity for compiler sessions:

- `SourcePath` keeps the physical path used for I/O separate from the normalized
  logical identity used by module graphs and persistent fingerprints. Ordinary
  CLI sources use their normalized path for both roles; build requests retain
  the frozen plan's package/build/toolchain/artifact identity while resolving a
  physical path for the current invocation. Lexical normalization removes
  resolvable `.`/`..` pairs but preserves leading relative `..` components, so
  parent-relative inputs neither change location nor collide with child paths;
- `SourceId` identifies a source file inside one session;
- `SourceRevision` identifies a concrete version of that source;
- `SourceFile` carries id, path, revision, and text.

All filesystem-backed compiler source reads use the `nia-source` 64 MiB stream
budget before UTF-8 decoding. Metadata rejects an already oversized input
without allocating from its length, and a `max + 1` read rejects growth after
the metadata observation. In-memory `set_source` inputs remain caller-owned and
are not silently truncated. CLI inspection, loader queries, and diagnostic
source recovery share this same file boundary.

`SourceId` is session-local. Persistent cross-session incremental compilation
must use a separate fingerprint or cache key rather than treating `SourceId` as
stable across compiler runs.

Recursive module discovery derives both roles from the parent `SourcePath`.
Relocating a package therefore changes where the loader reads `child.nia`
without changing that module's logical identity. A loader session still assumes
that one logical identity maps to one physical source tree; a relocated build
uses a new Driver and loader session.

The loader also owns the compiler source-input manifest. It sorts the complete
current module graph by logical source identity and records each physical path
as either missing or present with its content fingerprint and byte length. A
fully present manifest exposes the same aggregate program-source fingerprint
used by compiler frontend cache keys; any missing source makes that aggregate
unavailable. `nia-driver` forwards this manifest as a read-only result for build
validation. Its compiler-check publication API returns the checked program and
the final manifest from the same loader database after semantic provider
discovery, so concurrent Driver reuse cannot associate a result with another
request's source closure. Build code may use a pre-check manifest for lookup,
but must use this exact final manifest for publication; it must not rediscover
imports or compute a parallel source closure.

### 3.3 `nia-node-id`

Defines source-versioned syntax node identity for semantic side tables and
diagnostics. `VersionedNodeKey` combines source id, revision, syntax kind, and
either a span or red/green child-path position. The session-local hot-path
`NodeId` is an eight-byte owner/index handle allocated monotonically and never
reused.

The canonical `NodeStore` owns only active source-revision shards. A `NodeMap`
or `NodeOriginTable` retains the immutable shard containing its own IDs, rather
than cloning or retaining every session revision. Retiring a source revision
removes its shard and all index lookups from the current store; an immutable old
query value already held outside the query graph remains self-contained through
its own shard reference. Re-interning the same structural locator after
retirement receives a new monotonic index, so stale IDs cannot resolve to new
nodes.

AST nodes stay semantic-free. Semantic facts that need syntax identity are
stored in side tables keyed by `NodeKey`, `DefId`, `LocalId`, or `TyId`.

### 3.4 `nia-ids`

Defines typed cross-phase ids:

- `ModuleId`;
- `DefId`;
- `LocalId`;
- the session-scoped type handle `InternedTyId`;
- `GlobalDefId`.

It stores no semantic tables and has no filesystem, parser, or diagnostic
responsibility. In particular, `InternedTyId` does not expose a semantic owner
operation. A type's kind is a `TypeStore` fact and is never interpreted from
the handle alone.
The public identity handles document their session owner, module/definition
qualification, slot provenance, and visibility or trait identity variants;
builtin primitive anchors likewise expose canonical names without becoming
persistence keys.
Operator, iteration, value-builtin, and target-configuration enums use the
same documented registry identity boundary; their canonical names are lookup
inputs, not alternate semantic identities.
Layout queries, trait methods, receiver modes, associated members, and
supertrait descriptors are likewise schema metadata: they describe semantic
obligations without introducing a second identity domain.

### 3.5 `nia-symbol`

Owns the stable symbol boundary used by parser and semantic products.
`SymbolId` is an append-only hash identity; the `known` registry is the
canonical mapping for language and builtin names, while unresolved display text
remains deterministic and never interns arbitrary strings into persisted facts.

### 3.6 Semantic Identity Lifecycle

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
  synchronization, canonicalization, and sharding policy.

The compilation-owned `TypeStore` provides the sole session-wide identity
space. `InternedTyId` is exactly one 64-bit word containing `TypeStoreId` plus a
global `TypeStoreIndex`; it contains no module, interner, origin, or visibility
identity. The shared canonical core maps `TyKind` to that global slot, so the
same primitive or structural kind published from different modules has the
same ID. Handles from different stores remain distinct and are rejected at the
store boundary.

All algorithms read through `TypeStore` and publish through the cloneable,
write-only `TypeStoreAppend` capability. There is no module visibility log,
snapshot, checkout, recursive import, same-shard guard, or mutable interner
facade. Same-session handles are passed directly; publishing a kind validates
that every referenced child already exists in the same canonical store. The
canonicalization lock covers only one lookup-or-insert operation and is never
held across compiler algorithms.

Callable interfaces have an additional canonicalization invariant.
`TyKind::CallablePointee` is the unsized signature-bearing pointee, while
`TyKind::Callable { is_readonly, .. }` is its sized dynamic view. Publishing an
ordinary `TyKind::Pointer` whose element is a callable pointee canonicalizes to
the corresponding callable view. This prevents alias normalization, generic
substitution, or another structural reconstruction path from manufacturing an
incorrect one-word pointer to the unsized interface.

Const normalization and trait visibility use the current execution module,
while nominal layout ownership comes from `GlobalDefId`; an interned type has
no independent physical-origin owner. The canonical store validates
`TypeStoreId`, indexes an immutable
append-only kind arena, and returns a borrow tied to the store lifetime. The
arena is a sparse four-level `OnceLock` trie over the four bytes of a `u32` slot,
so reads neither acquire the canonicalization mutex nor require unsafe lifetime
extension. A foreign-session handle has no kind.

Trait solving reads every input handle from the canonical `TypeStore`, and its
append capability publishes synthesized types. Program trait implementations
and signatures therefore use the same handles without recursive import or
paired views. Enum classification uses explicit program metadata rather than
type origin or view membership.

Const and body providers publish directly to the compilation-owned `TypeStore`.
Array lengths, enum values, values, typed const facts, `ConstCheck`,
and `BodyConst` are ordinary semantic products without type snapshots.
`ConstInput` has no base interner, and `TypeLowering`
contains semantic facts rather than an append-prefix view; const type reads use
canonical storage. `BodyTypeCx` fixes the body algorithm contract in the same way: reads
always use canonical storage, while synthesized types append to the current
session shard. Program and local signatures therefore carry canonical handles
directly; body checking has no signature-import fallback. Explicitly
speculative body type comparison may clone the append target, but it retains
the same canonical read source.

A store transaction must not invoke a provider that mutates the same module
shard. Providers acquire shared local trait, extension, and function-signature
facts before entering the transaction, while preserving item-level lazy
materialization inside resolvers. `TypeStore` rejects same-thread reentry into
one module shard so an ownership violation becomes an immediate internal error
instead of a mutex deadlock. Foreign const append capabilities are restricted
to trait solving and layout operations that synthesize types. Const algorithms
never read through those capabilities or copy canonical handles into them.

`BodyIr` contains typed body data but no interner snapshot. Prechecked body
facts and incremental seeds borrow an explicit current session view, which must be a
prefix of the session shard and cannot replace it. Executable fact extraction
and reachability read every handle directly from the canonical `TypeStore`;
typed body data is not also a type-store product. Reachability receives a
separate append-only capability only while generic substitution synthesizes a
new structural type. That capability does not add the type to a module
visibility log, and every read remains bound to the canonical store.

Function IR lowering borrows the session shard and appends synthesized
types, while its single-body and batch products contain only function IR and
diagnostics. `MonoCollector` also borrows the store directly: recursive type
inspection clones one `TyKind` under a short transaction, projection solving
locks only the target shard, and type mangling uses a bounded transaction whose
callbacks cannot reenter the store. `MonoCollector` and `Monomorphization`
contain monomorphization facts, not writable interners or interner snapshots.

Layout root traversal and computation use a single `LayoutComputationInput`.
`LayoutTypeCx` reads every handle from canonical storage and appends substituted
generic types through `TypeStoreAppend`, so consuming a foreign signature does
not grow a module visibility log. Ordinary module layouts start from the merged,
deterministic semantic roots exposed by `ItemSignatures` and `TypeLowering`;
const layout builtins add their concrete operand as an explicit request root.
Precise executable and signature layouts receive an explicit `LayoutRoots` set.
Layout therefore never scans a module interner to discover work. `Layouts`
contains only layout facts and diagnostics; there is no read-only/owned
overload, interner snapshot, or positional convenience API beside
`LayoutComputationInput`. Array-length facts are prepared before layout
computation when evaluating them could recursively request another module's
layout.

Backend lowering reads existing handles from the canonical store and publishes
synthesized instance types through a module-scoped `TypeStoreAppend`; it does
not checkout an owning module shard for the whole-program fixed point.
`BackendProgram` contains typed handles and backend facts,
not a second type database. `CodegenProgram` retains only a lightweight handle
to the same session store. LLVM validation, compiler-builtin collection,
ABI/type lowering, static initialization, and mangling all resolve handles
directly from the canonical store; they neither checkout module shards nor
reconstruct a program module-view map. Missing or foreign handles fail at this
store boundary.

Compiler-query passes the canonical store into backend lowering rather than
constructing a module-snapshot map. `BackendTypeContext` reads every `TyId` from
`TypeStore`; foreign
function/global instance worklists carry only stable handles; extension and
trait candidates carry `ModuleId` for normalization rather than cloned
interners. Program normalization input borrows only the normalized-ID maps, so
it cannot expose a type view as an accidental backend side channel. The
backend's append capability neither exposes nor updates a module visibility
log.

Reachability follows the same boundary: its fact input contains one
canonical store reference, generic instances carry only stable handles, and
trait method/vtable deduplication includes the use-module visibility context
instead of relying on an argument interner identity. It does not snapshot,
import, or recursively adopt types. Program signature products and visible
extension products likewise contain only canonical handles; body, trait
solving, layout, reachability, backend, and codegen consume them without an
embedded interner. Const and program-signature analysis also read exclusively
from the canonical store. Signature equivalence, trait decomposition,
visibility alias resolution, semantic-use projection collection, and
array-length dependency scans all take an explicit `TypeStore`; substitutions
synthesize through `TypeStoreAppend`. `TypeNormalization` is a pure semantic
fact containing only normalized-ID mappings and diagnostics. Its single
algorithm entry receives the canonical store, creates a module-scoped append
capability for synthesized aliases, and never exposes mutable storage to its
callers; const/body/query inputs use explicit canonical storage instead of
normalization as a snapshot carrier. Type lowering exposes deterministic,
deduplicated source type roots, so normalization also does not enumerate a
module view. `TypeLowering` itself contains only source type facts, const
expressions, and diagnostics; all lowering entry points require an explicit
session `TypeStore` through `TypeLoweringContext`. ABI and flow checks read that
store directly. Item-signature collection has one input-object API, validates
that lowering handles belong to the same store, and uses a short-lived append
capability for synthesized builtin/error types. No production append contract
requires a module visibility view.

### 3.7 `nia-diagnostic`

Defines diagnostics and source rendering. It owns user-facing diagnostic display
but not semantic policy. Semantic crates create diagnostics; this crate renders
them consistently.
Diagnostic codes are registry-backed schema values: severity, category, and
stage are reconstructed from the registered definition during stable-bundle
decode, while labels retain explicit source/fallback/generated provenance.
Store-qualified bundle ids keep immutable diagnostic payloads isolated across
compiler sessions.

### 3.8 `nia-timing`

Owns process-wide timing collection and optional Rust heap instrumentation.
Allocation metrics exist only when the binary installs and registers the
`CountingAllocator`; one exclusive live-byte window may measure worker-thread
allocations at a time. Timing collectors serialize report ownership across
threads, while same-owner re-entry executes without creating a nested report.
Summary/detail modes, trace retention, and text/JSON encoding are independent
options. Query accumulators aggregate by stable names before emitting bounded
reports; the crate observes compiler work but owns no query or phase semantics.

## 4. Syntax Crates

### 4.1 `nia-lexer`

Turns source text into tokens with spans. It handles comments, identifiers,
numbers, strings, multiline strings, character literals, punctuation, and lexer
errors.

The lexer does not know semantic meaning. It should not resolve types, evaluate
constants, or classify identifiers beyond keyword recognition.

`tokenize` emits significant tokens plus one terminal EOF; `tokenize_lossless`
also retains contiguous whitespace and line-comment trivia. Both use half-open
UTF-8 byte spans. Lexical failures remain in-band `Error` tokens so parsing can
recover without a parallel stream; an unsupported Unicode scalar occupies one
sliceable error span even though identifiers are intentionally ASCII.

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

Every tree normalizes its input stream to one terminal EOF token at the actual
source boundary; caller-provided post-EOF tokens are unreachable. Token cursors
therefore remain total at end of input and use overflow-safe lookahead. Edits
must preserve UTF-8 boundaries. A partial reparse is accepted only when one
existing token can be rewritten without changing lexical token boundaries;
invalid spans or boundary-changing edits fall back to a full rebuild. Green
nodes remain lossless, including whitespace, comments, unsupported Unicode, and
unmatched delimiters, so parser recovery can inspect the original source.

### 4.3 `nia-ast`

Defines the parsed syntax tree. AST nodes represent source structure and spans.
They do not store type ids, def ids, layout information, or backend values.

AST expressions, statements, patterns, items, and type references retain only
syntax payloads plus `Span`/`VersionedNodeKey` identity. Generic arguments keep
ambiguous type-or-const forms until semantic consumers decide their meaning;
declaration-equality and stable-identity helpers intentionally ignore source
locations. Pattern helpers report structural binding presence only. This keeps
AST traversal reusable while type checking, name resolution, and const
evaluation remain owners of semantic interpretation.

`nia-pattern-analysis` is the shared, semantic-input coverage owner. Adapters
must provide canonical constructor identities and declaration-order fields;
the analyzer validates matrix/query widths, constructor arity, and scalar-domain
boundaries before producing usefulness witnesses. `Finite`, `Open`, incomplete
`Scalar`, and `Opaque` domains deliberately differ in whether a wildcard is
required. The crate returns witnesses only; runtime and const lowering retain
ownership of evaluation and control-flow behavior.

### 4.4 `nia-parser`

Builds AST from `nia-syntax` red tokens and reports parse errors. It owns
grammar decisions, local parse recovery, and syntax-to-AST lowering. While
lowering AST nodes, it records `NodeOriginTable` mappings from AST spans to
red/green child-path ranges.

Parser checkpoints roll back token position and origin-table mutations together
so speculative grammar branches cannot publish discarded identities. Item
recovery must consume input or reach EOF on every failed branch, and lexical
errors retain their originating syntax token key in the structured parse error.

Important parser boundary:

- expression bracket suffixes are parsed in a syntax-preserving form;
- semantic disambiguation of generic instantiation vs indexing happens later.

### 4.5 `nia-ast-walk`

Provides AST traversal helpers for phases that need tree walking. It should stay
small and generic. It must not embed semantic policy.
Its documented `Visitor` callbacks and `walk_*` entry points define structural
preorder ownership only; semantic passes may override a callback, but must
delegate when child traversal remains part of their input contract.

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
reinterpreting a pruned AST module. The raw tree preserves inactive items as
source-addressable syntax.

Semantic phases consume the selected active item tree rather than interpreting
conditional source selection independently as a declaration, definition, type,
value, or local-name pre-pass. Inactive items remain represented and
source-addressable; they are not semantically checked for a target unless a
query selects them.

Function bodies are stored as AST body nodes inside active item-tree
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

The query frontend owns batch compilation and the persistent products derived
directly from its facts. `nia-loader-query` owns provider summaries, facade
facts, module dependencies, public-surface facts, source-to-item-signature
dependency manifests, and provider-demand plans. `nia-compiler-query` owns
signature type resolution, signature type lowering, item signatures, extension
validation diagnostics, executable value-reference edges, and check
certificates. It does not own editor scheduling, cancellation, or priority
policy.

Every persisted frontend envelope repeats its content-addressed key and the
stable inputs needed to validate a lookup, rather than trusting the directory
path alone. Decoders authenticate the payload, reject truncation and trailing
bytes, and require every repeated identity field to match the caller's current
namespace, logical source/module, and product inputs. Corruption or stale
identity is retired under the per-key storage lock only while the same observed
record remains installed, so a stale reader cannot delete a concurrent
replacement. Non-replacing publication uses that same lock and preserves the
existing winner. Optional verification recomputes semantic products and
replaces structurally valid but semantically stale entries.

Query keys declare an explicit storage policy. The default
`CacheOwnedArc` policy retains one immutable value in its slot and publishes
shared handles. `SingleConsumerOwned` instead moves a non-`Clone` value directly
from its provider to one consumer. An `ExternallyPublished` owned query may
instead receive the same raw value from a tracked producer through
`publish_owned`; its slot temporarily owns that value without an `Arc`, and
records an explicit dependency on the producer. `get_owned` moves the payload
out and leaves only a payload-free `Consumed` state, execution statistics, and
dependency edges. Invalidating the producer drops an unconsumed published value
or reaches its downstream consumer after consumption. A key-executed owned
query can produce a fresh value on a later direct request; an externally
published key rejects access until its producer republishes it. Owned queries
cannot declare fingerprints and ordinary `get` rejects them. These policies are
for actual unique-consumer boundaries, not a way to hide shared data without an
`Arc`.

`QuerySession` owns a lazily started persistent executor. `get_many` submits
cache-owned queries and returns their `Arc` handles; `get_many_owned` submits
`SingleConsumerOwned` queries and moves their non-`Clone` values back without an
`Arc`. `run_tasks` submits non-query closures and returns their owned outputs in
submission order without creating query nodes or dependency edges. All three
use the same batch implementation, queue, workers, panic boundary, and nested
progress rules; query batches additionally merge the logical parent dependency
stack. Nested batches reuse the permit already held by the current execution
thread. Session shutdown first closes executor admission and then drains every
accepted task before joining its workers. Distinct sessions retain separate
queues and query graphs but share one process-wide CPU budget. That budget inherits the
Cargo/GNU Make jobserver when one is available; otherwise it creates a local
jobserver from the process-visible parallelism, including one implicit process
token in either case. There is no environment-variable worker-count override.
Each LLVM unit task additionally acquires one process-wide heavy-memory permit
before constructing its LLVM context or target machine. Capacity is the minimum
of visible CPU parallelism and half of the effective system/cgroup memory budget
charged at 1.5 GiB per task, capped at four; an unknown memory limit is
conservative and permits one task. While another LLVM task is active, low currently
available memory applies backpressure until that task releases its RAII permit;
one task can always proceed. Nested LLVM work on the same thread reuses its
permit. Production scheduling and the outer test-session pool read effective
memory limits and pressure from the same resource probe implementation. Unit
tests do not acquire a global compiler permit, and compiler/LLVM public APIs
have identical resource semantics in test and non-test builds. Integration
harnesses may assign weight to a complete process or compilation session.
Driver unit tests retain the same session around direct in-process compiler
work; an existing explicit case session is reused rather than nested. A
sequential runtime command inside such a compiler/build session retains its
separate runtime scheduling permit but reuses the session's larger memory
reservation; otherwise an exactly full constrained-host budget could wait for
itself after the compiler process had already exited.

Query waits have a separate process-wide wait-for graph because a provider may
block on a query in another worker or session. Thread-local stacks catch direct
recursion; the wait graph catches disjoint-worker and cross-session cycles before
condition-variable blocking. Every temporary wait edge has an RAII guard, and
cycle failure removes the edge before returning the error, so a later unrelated
query cannot inherit stale wait state. Session retirement closes admission before
draining active work and uses an RAII guard to reopen admission even if the
retirement callback panics. Deterministic owner tests assert both removal of the
participating nodes after parallel/cross-session cycle failures and successful
query admission after retirement panic.

Query values and identities have an explicit retirement boundary. A session
retirement request blocks new query activity, waits for current query execution,
validation, invalidation, and tracing to become quiescent, then removes the
obsolete key from typed lookup, the live slot table, and both directions of the
dependency graph. Slot indices are monotonic and never reused, so a retired
`QueryNodeId` cannot resolve to a later slot. Source replacement applies this
protocol to the retired revision's parsed module, syntax tree, declarations,
provider summary, and facade facts. Cache ownership is not a historical-reader
capability; only immutable values already held by external readers may outlive
retirement through their own `Arc`. Provider graph growth uses the previous
immutable graph once to preserve existing module handles, then seals the owned
current graph and retires that sole predecessor. The provider store retains only
the canonical current demand set and at most one pending additive transition;
there is no revision event history or live revision-query chain. Source
replacement performs source mutation, provider reset, root invalidation,
revision-keyed query retirement, and `NodeStore` shard retirement in one
session-wide quiescent transaction. Current query/cache/node owners therefore
retain no revision history; only immutable values already held externally may
keep their own payload shards alive.

## 5. Definitions And Modules

### 5.1 `nia-defs`

Collects active item-tree definitions into module-local definition tables. It
assigns `DefId`s and tracks namespaces for values, types, modules, enum
variants, and methods.

It detects duplicate names in the same namespace and duplicate generic
parameters. It does not evaluate const conditions, type-check bodies, or load
other files.

`DefId` is derived from canonical structural declaration identity rather than
collection order or source formatting. Top-level namespace, member ancestry,
extension target/trait/generic/where syntax, and duplicate ordinal participate
in that identity; unrelated insertions and module-session handles do not.
`DefMap` collision-checks the complete structural identity before publishing a
definition. `DefNodeMap` separately connects stable syntax locators to these
definition ids and retains one explicit node-store owner.

Public-surface persistence uses `PublicSurfaceModuleFacts`, a deterministic
reduced projection containing only declaration, namespace, enum-variant, and
module-using facts. Materializing that projection for another session rebases
the module handle without pretending cached facts own AST nodes, generic syntax,
or diagnostics.

Extension indexes distinguish declaration availability, ordinary callability,
and trait-witness capability. Visibility filtering visits the current module
once and deduplicates imported module ids at the owner boundary, so repeated or
overlapping visibility closures cannot duplicate method or associated-value
candidates. Associated-value lookup returns no result when multiple visible
implementations make an exact target/name pair ambiguous.

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

`CheckedProgramWithSourceManifest` and
`LinkedExecutableWithSourceManifest` carry the exact final source closure used
by their corresponding products; callers must not reconstruct that closure from
the module graph after checking. Cache references encode complete component
identities and exact wire lengths, and decode rejects truncated, overlong, or
otherwise mismatched records. Cache publication and retirement remain owned by
the cache owners rather than by artifact consumers. Artifact structs keep
diagnostics, optimization reports, output paths, link inputs, and cache
references as distinct products. `DriverOutput` preserves structured errors and
converts internal panics at the driver boundary. Inspection and report helpers
are presentation adapters only; they do not own semantic facts or policy.

## 6. Type Frontend

### 6.1 `nia-type-resolve`

Resolves type names in active item-tree type syntax to definition identities or
primitive types. It validates type paths and generic names but does not lower
them into canonical type ids.

### 6.2 `nia-ty`

Defines the compiler type model and session-wide canonical `TypeStore`. All
compiler passes and test fixtures read unified `TyId` handles from the store and
publish new types through `TypeStoreAppend`; there is no secondary type view or
storage API.

### 6.3 `nia-type-lower`

Lowers active item-tree type references into interned type ids. It handles
primitive types, pointers, arrays, slices, thin function pointer types,
unsized callable interfaces and their sized views, nominal types, generics,
enum backing types, and inferred array lengths. Source `TypeKind::Callable`
lowers to `TyKind::CallablePointee` when bare and to `TyKind::Callable` when it
is the direct target of `&` or `&mut`.

It also validates type-level restrictions such as invalid use of `()` or `never`
in value positions. Its semantic product exposes deterministic, deduplicated
source type roots for normalization and other downstream algorithms; consumers
must not enumerate a module interner to infer those roots. The lowerer reads
existing handles from the canonical `TypeStore` and publishes primitive,
nominal, projection, and structural types through a module-scoped
`TypeStoreAppend`; it never opens a mutable interner transaction.

### 6.4 `nia-item-signatures`

Collects signatures for functions, methods, globals, structs, enums, aliases,
and extension methods after type lowering. Function signatures include whether a
function is `extern`, variadic, generic, and whether it has a body.

This phase intentionally ignores function body semantics.

`ItemSignatures` is the declaration surface consumed by type resolution,
trait solving, layout, const checking, and backend planning. Its type roots are
explicitly collected and deduplicated from signatures, including generic const
types, where predicates, supertrait associated bindings, and enum payloads;
consumers must not scan the type store to rediscover them. Generic parameter
metadata preserves declaration order and kind, while
`generic_argument_substitutions` independently consumes type and const vectors
and rejects missing or surplus arguments. Trait implementation identities are
stable over normalized syntax and the program index stores candidate indexes,
not copied signatures. Body meaning remains owned by body checking.

### 6.5 `nia-program-signatures`

Qualifies the declaration-only products from `nia-item-signatures` with
`GlobalDefId` identities and indexes program-level trait implementations. Its
lookup/context APIs borrow or resolve existing signatures; they do not reparse
source or reconstruct body semantics. Collection functions explicitly qualify
module-local ids, preserve declaration and implementation ordering, and keep
trait-implementation indexes as candidate ids rather than copied signatures.

Visibility-aware extension discovery computes a deterministic closure from
using scopes, public surfaces, canonical type normalization, and nominal
extension providers. Every visible target is normalized in the module that
owns its signature, and missing definition or normalization facts are rejected
instead of guessed. Callable extension visibility and trait-witness visibility
are tracked separately so a public trait obligation cannot accidentally make a
private method callable.

Trait-goal assumptions expanded through source supertraits use a path-local
semantic guard over the complete goal, including `self_ty`, type arguments, and
const arguments. Assumption output deduplication uses the same equivalence rule;
integer const spellings and module-owned interned handles therefore cannot hide
an independent sibling or reopen a recursive path.

Projection substitution in an impl context uses the same structural type and
const-argument equivalence for its applicability check. Rebuilt handles and
integer signedness are semantic aliases at this boundary; exact interner and
declaration identities remain reserved for cache keys and ownership.

### 6.6 `nia-type-normalize`

Expands type aliases and canonicalizes type forms where required. It detects
recursive aliases and keeps normalized type information separate from raw lowered
types. `TypeNormalization` contains only normalized-ID facts and diagnostics;
it never owns a type view. The normalizer reads every input and referenced type
through the session `TypeStore` and uses its explicit append target only to
intern synthesized normalized forms.

### 6.7 `nia-trait-solve`

Resolves builtin and user trait goals, associated types, and associated consts
from canonical type handles and explicit program-signature facts. Solver
construction does not borrow a mutable module interner: all reads use the
session `TypeStore`, and synthesized goal or projection types are published
through a module-scoped `TypeStoreAppend`.

Trait and associated-type recursion use path-local semantic guards. Active
trait goals and projection keys compare normalized type and const arguments
through the solver's equivalence rules, so equivalent handles from different
module appenders or signed/unsigned integer spellings cannot reopen a cycle.
Each key is removed on every return path, including missing associated items.
Body-check projection normalization applies the same path-local semantic key
rule before asking the solver to resolve a projection, so frontend normalization
cannot reopen a recursive associated-type path through a rebuilt handle.

Program-signature trait-impl and associated-projection deduplication also uses
the shared structural const-argument relation. Integer values compare by bits,
while unresolved expression identities stay exact; this keeps equivalent
impls from being duplicated without collapsing distinct unresolved values.

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

### 7.3 `nia-sema-ir`

Owns the persistent semantic fact schema shared by name resolution, body
checking, const lowering, reachability, and backend planning. Facts are keyed by
global definitions, locals, or `VersionedNodeKey`; the crate carries no AST and
does not infer or reinterpret semantics. Call dispatch identities retain every
type and const argument required to distinguish trait, dynamic-object, builtin,
method, closure, callable, and function-pointer calls.

Module-level expressions and function bodies are separate ownership domains.
Combined iterators deliberately traverse both, while
`retain_module_level_facts` removes function-owned duplicates only from mutable
module staging maps. Frozen `NodeMap` products retain one explicit `NodeStore`;
merging rehomes incoming maps into the receiver's store, and thaw/freeze is the
only boundary for moving a product to another store. Stable locators, rather
than compact node handles, therefore define equality across node-store owners.

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
type uses, and nominal type prefixes used by associated calls. The table is a shared semantic input surface, not a const
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

`const fn` is const-capable rather than const-eval-only. Const evaluation
interprets its const semantic body when the call occurs in a constant
expression. The ordinary body-check, reachability, backend-lowering, and
code-generation pipeline retains the same function when it is reachable from
runtime code. Constant expressions reject ordinary `fn`; runtime expressions
may call either function kind. Receiver and associated calls use the shared
visible-extension index, including target generic substitutions, so const
execution does not maintain a name-based method whitelist.

Const capability is validated eagerly for every lowered `const fn`, including
unused functions and unselected source branches. This declaration pass checks
statement expressions and declared return contexts without executing
data-dependent failures. The evaluator remains the execution engine rather
than the place where an annotation first becomes semantically meaningful.
Runtime body checking stays reachability-driven, so eager const validation does
not create function bodies, executable facts, or backend roots.

Const iteration capability remains owned by ordinary semantic checking. For a
`for-in` statement, the body checker resolves the visible builtin `Iterable`
and `Iterator` obligations, identifies the selected user impl, and requires its
exact `iter` or `next` witness signature to be const-capable. Intrinsic
`Iterator: Iterable` adaptation requires no witness. Direct builtin iteration
method calls use the same check. Const execution then consumes the visible
extension and trait facts for the concrete generic instance; it does not treat
an inherent method with the same name as iteration protocol evidence.

The early const IR deliberately stops at this semantic boundary. It evaluates
the iterable expression so ordinary control-flow and error propagation remain
observable, then reports that `Iterator` execution is unavailable instead of
guessing a witness or silently running the loop body. Only resolved const IR,
after semantic iterator facts have been attached, may create iterator state and
drive repeated `next` calls. The evaluator regression
`early_const_for_in_reports_witness_dispatch_boundary` pins this distinction.
Resolved iteration threads the updated iterator value returned by every
`next` call into the following call and creates a fresh lexical scope for each
yielded pattern. Direct evaluator regressions cover normal exhaustion and prove
that a pattern-binding error restores both the item scope and enclosing block
scope before it escapes.

Production const environments carry a per-outer-evaluation budget shared by
nested calls and loops. The evaluator consumes steps at expression, statement,
and loop boundaries and enters a bounded function frame before binding call
locals. Both module const checking and function-body local const execution use
that mechanism. Limit failures return `ConstError` through the normal
diagnostic path; they cannot recurse until the host stack or process is lost.

### 8.3 `nia-const-check`

Lowers AST plus local/value/type semantic tables into `ConstModule`, then
uses `nia-const-eval` to check and collect current compile-time values. It
owns `const` binding dependency resolution, cycle diagnostics, enum
discriminant values, and array length values that depend on local or imported
const bindings or imported `const fn` calls.
Dependency-cycle detection is path-local to one active evaluation chain. Every
attempt removes its active key on success or failure, so a cyclic component
cannot poison later independent initializers in the same module. Const-check
owner and compiler-query regressions require the cycle diagnostic to coexist
with the independent value and its typed fact.
Layout builtins such as `std::builtin::size[T]()` and
`std::builtin::align[T]()` consume those evaluated array lengths through narrow
lookup closures while computing layouts; they do
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
values through the program const query result and reuses their canonical
runtime type handles in the current execution context. Item signatures remain the source
signature surface; inferred const value types are semantic query output, not
retroactive signature data.

All const phase entry points and typed const queries read existing types from
the compilation `TypeStore`. Each execution module receives a narrow
`TypeStoreAppend` capability for structural types synthesized by inference or
generic substitution. `ConstInput` and `TypedConstQueryInput` never borrow a
mutable interner, and full, signature, and executable query providers do not
checkout module shards around const evaluation.

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

Runtime body checking records automatic error conversions as ordinary
resolved trait-call facts on the propagation node. Typed body IR carries that
conversion separately from the operand, and function lowering materializes the
operand once before constructing the failure-only call. Function IR therefore
keeps success extraction, error conversion, trait witness resolution, executable
reachability, and defer ordering explicit. LLVM lowering evaluates the converted
return value before tail defers, matching the order of an explicit conversion
followed by propagation.
The backend validator checks this failure-edge contract before LLVM: the try
kind must match the input and enclosing return union, the success local must
match the input success payload, optional propagation cannot carry a conversion,
and either the direct or converted error payload must match the return error.
Malformed products are internal diagnostics rather than LLVM type failures or
silently ignored conversions.

Const checking applies the same protocol at its semantic boundary. It resolves
the unique `IntoError[Target]` witness, verifies that the selected method is a
`const fn`, and stores that invocation-local witness in the active const call
frame. The resolved const evaluator invokes the witness only when an error
union's failure edge is propagated; successful payloads bypass it. A
runtime-only witness is a const diagnostic, never a fallback execution path.
The const IR remains a resolved expression tree, while the call-frame fact
supplies the evaluation-only failure-edge operation without duplicating runtime
BIR.

Standard error-union combinators remain ordinary generic extension methods.
`mapError` calls `Fn(Source) Target` only on failure. `orElse` calls
`Fn(Source) Target!Value` only on failure and returns that callback result
directly, while its success arm reconstructs `!value`. The nested callback
return type therefore participates in ordinary callable/generic inference; no
compiler-owned error-union flattening operation or recovery side channel exists.

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

File embedding is part of the same semantic boundary. The
`std::builtin::embed("path")`
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
array literals can still be typed as const-only arrays. Their element type is
another `ConstValueType`, but any aggregate element carries its nominal source
type; const evaluation does not create structural struct types.
Const array slicing is part of the same value surface: slicing a const
array produces another const array value, and the typed const layer
records the sliced element type and known length so the result can feed field
access, indexing, and generic const call inference without becoming a
runtime slice.

Const struct literals have one typed surface: the source literal names a
nominal type, and resolved const IR stores that type as a required field. The
checker substitutes its generic arguments into field signatures, infers
still-open generic arguments from concrete field values, and then rechecks all
fields with the completed types. `ConstValue::Struct` remains the evaluator's
field-keyed storage representation, but it does not represent an anonymous or
structural type.

Const field access resolves nominal struct fields from the signature with
generic arguments substituted into the current execution module. This keeps
const data on the ordinary typed expression path without introducing a second
aggregate type system.

Const unions use a separate storage value, not `ConstValue::Struct` with one
field. `ConstValue::Union` contains target-ordered bytes, one initialized bit per
byte, and a stable const ABI descriptor for every supported field. A descriptor
is a scalar, a recursively nested fixed array, a SIMD vector with scalar lane
schema and allocation size, a nominal struct containing layout-owned field
offsets and an object size, or another untagged union with its complete field
schema and storage size. There is deliberately no active or last-written field
identity: source construction history is not part of an untagged union value.
Reads decode the requested field from the shared bytes, and writes replace only
that field's described byte and initialization ranges.

`nia-layout` owns the primitive layouts and the union max-size/max-alignment
formula used to size this storage. `nia-const-check` maps substituted semantic
field types and the artifact `TargetConfig` to the stable scalar descriptors;
`nia-const-eval` only executes the resulting value operations and does not
inspect `TyKind`, signatures, or session-local type ids. Resolved aggregate
construction is therefore a narrow environment operation. Before evaluating an
aggregate literal in a typed binding, function result, call argument, or
assignment RHS, the evaluator lets the typed environment prepare only that
operation's existing expected type; it never scans all function bodies to
recover context.

Scalar arrays are encoded element by element with the artifact endianness. They
reuse `nia-layout`'s checked array-size fact, including const and layout-builtin
lengths evaluated for the artifact pointer width. Nominal struct descriptors
come from the substituted `nia-layout` instance rather than source field order.
Encoding initializes each field's recursive byte range while leaving inter-field
and trailing padding uninitialized. Nested union encoding and decoding copies
both its bytes and initialization bitmap after checking its ABI schema, storage
size, and target endianness. Decoding a struct reads only its fields;
reinterpreting the same storage through a field that covers padding diagnoses an
uninitialized read. When runtime code projects a field from a union `const`,
body IR lowering reads that field from the same stored bytes and materializes
the decoded scalar, array, struct, or nested union.

Vectors have a distinct `ConstValue::Vector`; they are not represented as
arrays. Numeric lanes encode in lane order with artifact endianness. Boolean
lanes encode as a packed integer mask with lane 0 as the least significant bit,
and target-sized integer lanes use the artifact pointer width. Only the store
width is initialized; allocation-tail padding remains absent. Concrete vector
const values materialize into body IR through an internal splat followed by
lane inserts, reusing the ordinary runtime SIMD lowering rather than adding a
second backend literal model.

Whole const unions use an internal `UnionStorageLiteral` rather than an ordinary
source `UnionLiteral`. It carries one optional value per artifact byte through
typed body IR and function IR. Backend validation requires a concrete nominal
union and an exact artifact-layout byte count. LLVM lowering stores only present
bytes into nominal union storage; absent bytes remain uninitialized rather than
being fabricated as zero. This path is used for expression temporaries, direct
aggregate destinations, indirect arguments, and returns, so crossing from
comptime into runtime preserves storage without reconstructing a field. Whole
struct const values still materialize as ordinary `StructLiteral` expressions.

Const pointers use explicit provenance rather than a transparent boxed pointee
snapshot. A place pointer contains an evaluator allocation id and
a field/index projection path. Local bindings receive fresh allocation ids;
dereference resolves the active frame value, so a write to the owner is visible
through an existing pointer. Equality compares provenance, never pointee
contents. Before a function frame is removed, its result and mutable-receiver
writeback are recursively checked: a pointer owned by that frame or by an
already-ended nested scope is rejected, while a pointer into caller storage may
pass through. A top-level const result cannot retain any place pointer.

An rvalue reference created while an execution frame or block scope is active
receives a temporary place allocation owned by that scope. This matches runtime
block-temporary lifetime: nested calls may use or return the pointer to their
caller, but it becomes invalid when its owning scope ends. Only an rvalue
reference created directly by a module or local const-binding initializer,
without an active evaluator execution scope, uses the separate frozen-allocation
pointer carrying its defining module, source span, readonly state, and pointee
value. Writable frozen allocations cannot escape. The origin is semantic
allocation identity, not a host address. Typed-query context frames are marked
separately and do not fabricate execution lifetime merely by being present.

Const union storage represents pointers with typed relocations alongside its
raw bytes and per-byte initialization state. A relocation records its storage
offset, artifact pointer width, and `ConstPointerValue`; the covered bytes are
initialized placeholders and never contain a host address. Recursive array and
struct encoding shifts relocation offsets, nested unions preserve them, and a
field write invalidates overlapping relocations. Unwritten fragments of an
invalidated relocation become uninitialized rather than ordinary zero bytes. A
pointer read requires one exact relocation. Scalar/vector reinterpretation,
partial-relocation reads, and constructing a pointer from arbitrary initialized
bytes diagnose. Escape
validation recursively inspects relocation targets.

The runtime const pipeline represents a frozen relocation target with
`PromotedAllocationId`, whose module and source span identify the semantic
allocation independently of a host address. Body IR and function IR carry the
relocation's storage range, allocation identity, and typed pointee expression.
All recursive transforms, dependency collectors, and optimization visitors must
visit that pointee. Function IR validates relocation bounds, ordering,
non-overlap, initialized pointer storage, and pointee shape; backend validation adds artifact
pointer-width, origin-module, and runtime-type checks. Artifact fingerprints
encode the origin module through its normalized source identity rather than its
session-local `ModuleId`.

LLVM materializes a supported relocation target as a readonly link-once global.
The symbol derives from the origin module's normalized source identity and
origin span, so all uses and codegen partitions name the same allocation while
distinct source allocations remain distinct even when their contents match.
Function-body references include origin modules as readiness dependencies.
Runtime union construction skips relocation placeholder bytes and stores the
promoted global address into each relocation range. The module registry rejects
reuse of one identity with a different pointee type.

Supported promoted initializers include scalars, fixed array literals and
repeats, string and byte-string literals, SIMD vectors, and nominal structs
recursively composed from those forms. Arrays use LLVM constant arrays.
Vectors reconstruct canonical const `splat`/`insert` expressions into checked
lane lists for `LLVMConstVector`. Structs reuse the runtime layout's physical
field order and named LLVM struct type. This path never constructs a global
initializer from an `alloca`, load, or other runtime instruction.

When a promoted union or aggregate contains relocations, LLVM codegen switches
from the nominal constant to byte-exact artifact storage. A packed constant
contains initialized byte segments, `undef` segments for union or struct
padding, and pointer-valued fields at relocation offsets. The global keeps the
Nia pointee ABI alignment. LLVM opaque pointers allow later code to view that
storage through the nominal pointee type, while the packed initializer keeps
object relocations as pointers rather than encoding them as integer bytes.
Codegen rechecks relocation ordering, bounds, and pointer width before building
the initializer.

The artifact-storage composer recurses through fixed arrays and physical struct
layout, so union pointees, structs containing pointer-bearing unions, and arrays
of them do not require separate representation models. Relocation-free values
continue to use ordinary LLVM constants.

A zero-sized promoted pointee uses a one-byte packed identity allocation under
the same stable promoted symbol. This physical allocation does not change the
Nia pointee layout or any value ABI; it exists only because an allocation whose
address is observable must have runtime identity. The link-once global is not
`unnamed_addr`, so LLVM cannot infer that equal contents permit address folding.
Same-origin uses still deduplicate, while distinct origins use distinct symbols.

Readonly array, string, and slice const materialization uses the same promoted
allocation path. Typed and function `StaticArrayPointer` nodes carry a
`PromotedAllocationId`: frozen pointers use their evaluator origin, while a
const string without frozen provenance uses its defining global item or local
binding rather than its use site. Recursive lowering preserves that fallback,
function-body references include imported origin modules, and fingerprints use
the normalized module source identity plus span. LLVM therefore emits the same
stable link-once global as every other promotion.

Static initializer IR has no static-array-pointer variant. It had no compiler
producer and keeping it would have retained a dormant second promotion model.
All address-bearing readonly const arrays now cross typed/function IR with
explicit source identity. Mutable pointer write-through remains separate and
waits for a shared alias-aware place operation instead of reusing
mutable-receiver copy/writeback.

Imported generic pointer-bearing unions use this same path after concrete type
substitution. Whole-union arguments and returns do not replace the allocation
identity with a generic-instance or caller identity. Executable reachability
keeps runtime generic instances and excludes comptime-only instances exactly as
for pointer-free values. Static/global addresses remain represented by
`StaticInit::AddrOfGlobal`; they are not entered in the promoted-allocation
registry, so equal contents and a shared defining module cannot merge static
storage with readonly const promotion storage.

Executable reachability must consume `StaticInit::value_refs(module_id)` when
extracting dependencies from a static initializer. The compatibility
`StaticInit::refs()` view intentionally retains only bare function and global
definitions; using it for executable facts would erase the concrete type,
const, and receiver arguments of `AddrOfFunction` values and could leave a
generic function-pointer target out of the backend instance plan.

Foreign const execution receives three disjoint signature-fact channels:
types, functions, and values. Executable reachability may request each
`SignatureItemSet` independently, so function-call resolution must use the
function channel rather than assuming a type- or const-binding subset also
contains function signatures. This keeps signature-facts mode equivalent to
full-module const checking for nested imported `const fn` calls.

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
function-local const values, imported `const fn` calls, and imported nominal
const fields infer generic arguments without growing a
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

Const `match` expressions follow the same source-shaped typed surface as
runtime source `match`: recursive nominal patterns, value patterns, integer
ranges, and catch-all cases. Struct pattern fields are checked against the
instantiated target type, so generic arguments are inherited from the matched
value rather than repeated on the pattern constructor.
Successful static const-match typing delegates usefulness and exhaustiveness
to `nia-pattern-analysis`, using the same constructor identity and field-order
contract as runtime body checking. Const evaluation remains path-driven: when
executing a `const fn`, it evaluates the selected arm rather than pretending to
be a whole-function static analysis. `nia-body-check` therefore remains the
owner of whole-function soundness, while const-check applies the shared matrix
whenever a source-shaped const match passes through static const typing.
Value-producing arm bodies are typed and unified to one typed const value
shape, while control-flow-only arms such as `return`, `break`, or `continue`
do not invent a match result type. Optional and error-union payload locals are
typed from the target type while checking either match or if-pattern arms. The
evaluator still performs
actual matching; the checker only records the arm and payload types needed for
const generic inference.

Const function calls are typed by their signatures in the same layer. Generic
type arguments are inferred from typed argument expressions, substituted into
the return type, and then published through the current execution module's
append capability. This makes nested const calls participate in later generic
inference without executing the callee solely to discover its type.

Early target pruning is intentionally narrower than full const execution: it
can evaluate target builtins and same-module helper functions before the module
graph is complete. It lowers its accepted AST conditions to `nia-const-ir`
locally, but does not participate in program-level const module queries. Full
imported const function execution belongs to the semantic query path after
imports, definitions, values, locals, and const modules are available.

### 8.4 `nia-pattern-analysis`

Owns the pure pattern-matrix algorithm shared by runtime body checking and
static const-match typing. It accepts only canonical type-column identities,
constructor identities, constructor field types, scalar bounds, and normalized
patterns. It has no dependency on AST, name resolution, type storage,
diagnostics, or lowering.

The algorithm follows the specialization/default structure of Maranget-style
usefulness analysis. Finite constructors model enums, optional/error unions,
tuples, pointers, and nominal structs. Scalar endpoints partition finite integer
domains into disjoint intervals without enumerating the domain. Open domains
retain an unknown remainder that only a wildcard can cover. Opaque expression
patterns remain useful unless shadowed by a wildcard but never prove
exhaustiveness.

Adapters must use the same canonical constructor identity and declaration field
order as lowering. Named fields omitted under an explicit `..` become wildcard
children before analysis; omission without `..` is a type error. A witness
returned by the crate is a subpattern of the usefulness query and therefore a
valid concrete explanation of missing coverage. The crate-level rustdoc in
`nia-pattern-analysis/src/lib.rs` records the paper references, proof
obligations, conservative boundaries, and maintenance checklist behind this
split.

### 8.5 `nia-static-check`

Validates static initializers for `static` storage. It distinguishes static data
from compile-time value bindings. Address initializers are allowed only when they
can be represented as target static relocations.

### 8.6 `nia-static-ir`

Defines the static/global initialization IR. It represents compile-time data,
not executable runtime control flow. It supports zero values, scalars,
strings/bytes, arrays, repeats, structs, null pointers, global addresses, and
function addresses.

Static address paths use static-only elements such as field ids and constant
indices. They must not carry source-shaped body expressions or runtime places.

### 8.7 `nia-layout`

Computes ABI-relevant layout for primitive, pointer, array, struct, enum, and
instantiated nominal types. Every compiler layout provider derives its
`TargetDataLayout` from the artifact `CompilerTargetQuery`; ordinary, signature,
type-only executable, and runtime-body executable layouts therefore share the
same pointer size and alignment. The query dependency also makes an artifact
target change invalidate cached layout products. Standalone body-check entry
points derive the same data layout from the `TargetConfig` in scope; the
host-only convenience entry constructs `TargetConfig::host()` once and uses it
for both checking and layout rather than assuming LP64 independently.

Callable pointees are unsized and have no `TypeLayout`. A callable view is
`Sized`, with size equal to two target pointer words and target pointer
alignment. The builtin trait solver follows the same split:
`CallablePointee: Unsized`, callable views are `Sized`, and callable pointees
do not satisfy `Sized`.

`nia-layout::vector_layout` is the single vector storage owner used by frontend
layout, backend validation, and LLVM lowering. It computes the byte-rounded
native store width from lane bit width, including packed `bool` lanes, chooses
the next power of two as ABI alignment, and rounds allocation size to that
alignment. Backend-local scalar-alignment approximations are forbidden because
they disagree with LLVM aggregate offsets and union stores.

The algorithm reads every existing handle from the session `TypeStore` and
publishes structural types created by generic substitution through a
module-scoped `TypeStoreAppend`. `LayoutComputationInput` therefore has no
mutable interner or snapshot field. Compiler query providers and standalone
callers use the same API, and the result remains a layout fact table rather than
a second type view.

Struct and union signatures retain each generic parameter's declared kind and
order. Layout binds the nominal type-argument and const-argument vectors by
walking that ordered signature, so interleaved parameters do not imply a
types-first storage convention. Field instantiation uses the shared
`nia_ty::substitute_ty` traversal; layout does not maintain a smaller recursive
type substitution model.

Const-generic parameter types on imported nominal types are interpreted from
the defining declaration rather than by looking up that declaration's AST node
in the consuming module's `TypeResolution`. The currently supported scalar
const parameter types use `nia_ty::PrimitiveTy::from_known_symbol` as the shared
resolution-independent spelling owner.

The instance-detail layout APIs have a stronger contract than ordinary nominal
layout lookup: they return field layouts and offsets, not only aggregate
size/alignment. They therefore resolve the requested program struct or union
signature and compute its detailed instance directly. A hit in the ordinary
program layout cache cannot short-circuit detail materialization.

### 8.8 `nia-abi-check`

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
- match duplicate defaults and duplicate syntactic patterns.

It should not perform full type checking.

`nia-sema` provides shared, diagnostic-neutral checks used across semantic
owners. Array literal length reconciliation distinguishes inferred/unknown
lengths from concrete mismatches; arity checks encode exact versus variadic
minimum requirements; field-set checks preserve duplicate and unknown source
occurrences while reporting missing identities in expected order. Callers own
diagnostic wording, recovery, and any type-specific policy.

Flow filtering is applied only after a function is matched to its stable local
definition identity and the module id is paired with the reachable set. An
excluded function is skipped as a whole, so syntax-level diagnostics from its
body cannot leak into a reachability-pruned executable; the unfiltered entry
point remains the owner for complete module diagnostics. Flow itself stays
conservative: eager operands propagate termination, logical RHS termination is
conditional, loops retain a possible fallthrough, and closure bodies reset the
enclosing loop depth while deferred expressions are still traversed for their
own control-flow diagnostics.

### 9.2 `nia-body-check`

Type-checks function bodies and expression semantics. It owns:

- local binding type checks;
- assignment target validation;
- pointer mutability and addressability checks;
- literal and pointer array-to-slice coercions;
- indexing, slicing, field access, and method calls;
- function calls and generic argument inference;
- enum casts, match usefulness/exhaustiveness, missing witnesses, and match
  range-pattern validation;
- builtin expression typing;
- inline assembly configuration validation.

Body checking consumes earlier tables instead of rediscovering definitions or
types from source text.

A source where-bound is a complete obligation: its self type, trait identity,
type and const arguments, and associated-type bindings travel together. Generic
and method candidate filtering may defer an associated binding while its type
still contains unsubstituted parameters, but it must check every concrete
binding before accepting the candidate. Final call validation and recursive
nominal-type validation prove the trait goal and each associated projection
equality; proving only the bare trait goal is insufficient. Program trait-impl
signatures are phase products at this boundary, so candidate matching also
requires exact trait type- and const-argument arity rather than relying on
`zip` truncation. Reverse inference from an impl matches concrete associated
type definitions against bound bindings after substituting the impl target's
type and const parameters. The complete target, trait arguments, const
arguments, and binding set is one transactional candidate: a later mismatch
cannot publish substitutions inferred from an earlier component.

Trait declarations use the same complete obligation product for supertraits.
When a supertrait declares `Parent[Item = T]`, item-signature collection and
its persisted schema retain that binding beside the parent trait type. Body and
program-signature consumers instantiate the binding with the child trait's
generic and `Self` context, add it to projection assumptions, and validate an
explicit parent impl's associated type before accepting the child impl. The
item-signature cache schema is versioned when this shape changes; older entries
are rejected rather than decoded with the old positional layout.

The instantiated binding is also an intrinsic guarantee of a child trait
object. A value of type `&Child` where `Child : Parent[Item = i32]` is object
safe for parent methods returning `Parent::Item`, and may upcast to
`&Parent[Item = i32]` without restating the equality on `Child`. Object-safety
checks, dynamic-method projection normalization, and upcast validation derive
these guarantees through the same cycle-guarded supertrait traversal. Bindings
not present on that path are never synthesized, and incompatible target
bindings remain rejected.

Reachability checks use a path-local depth-first guard: each visited trait
instance is removed before returning from its branch, including unavailable
signature branches. The active identity includes receiver, type arguments, and
const arguments; structural type equivalence and integer-bit const comparison
are used for the guard, while unresolved expression IDs remain exact. This
preserves termination for recursive declarations while still allowing
independent sibling supertraits to be explored.

Object-safety traversal keys that guard by the complete source trait instance,
including type and const arguments, and rebuilds declaration-order substitutions
before checking inherited methods. A separate expanded-instance set suppresses
duplicate diagnostics and work when a diamond reaches the same parent through
multiple siblings without weakening the path-local recursion guard. Vtable
instantiation and dynamic-method traversal use the same semantic key comparison,
so equivalent rebuilt handles or integer const spellings cannot reopen a cycle,
duplicate an inherited method candidate, or materialize a second generic vtable
instance.

Backend supertrait vtable expansion keeps the exact `(self_ty, object_ty)` key for
cache and output identity, while its traversal uses semantic type/const matching.
The path-local recursion stack remains balanced for cyclic declarations, and a
separate semantic expanded set suppresses repeated diamond branches so each
inherited trait instance contributes one set of slots.

Backend extension-trait candidate selection applies the same semantic const
matching to concrete impl arguments, including recursive const-argument types;
generic pattern parameters remain wildcards. Extension specificity applies the
same rule when proving a concrete candidate subsumes a general one, so signed
and unsigned spellings of the same integer do not create a false mismatch.

Body-check method and trait-object pattern matching uses the same boundary:
const pattern types compare structurally, integer values compare by bits, and
unresolved expression IDs remain exact. Generic pattern parameters are still
wildcards, with repeated substitutions checked through this relation rather
than raw `ConstGenericArg` equality.

The extension type-pattern matcher uses that const-pattern relation at every
nominal, trait-object, pointee, projection, and associated-binding boundary.
Pattern matching therefore cannot fall back to raw const-argument vectors when
the surrounding type was rebuilt in another interner.

Executable reachability replays the same extension pattern match across cached
type stores. Its const witness relation compares integer values by semantic bits
and preserves typed structural comparison; unresolved const-expression IDs
remain exact because reachability does not own a const evaluator.

LLVM backend validation reuses the semantic const-argument validator when
checking a vtable payload against its trait-object type. This keeps malformed IR
rejection strict for types and non-equivalent values without rejecting integer
spellings that carry the same semantic bits.

The dynamic-call validator and LLVM vtable slot/upcast lookup use the same
relation. A rebuilt trait-object call therefore selects the same inherited slot
when its integer const arguments differ only in signedness, while different
values remain distinct.

Where-bound candidate matching applies the same nominal identity rule during
substitution: type arguments and const arguments remain separate, and each
const argument type is recursively substituted. This keeps const-generic
where obligations aligned with trait projection and impl matching.

Static trait-method fallback also preserves the complete trait identity. Backend
default-method self selection and concrete-implementation checks pass the
trait's const arguments into the source `TraitGoal`, matching dynamic vtable
dispatch; the fallback `FunctionCallee` also stores those arguments so a
type-only payload cannot materialize the wrong instance after selection.

Monomorphization convergence accounting treats const arguments as typed
identity-bearing inputs: the depth walker and concrete-instance admission both
traverse each `ConstGenericArg::ty`, including types nested inside nominal,
trait-object, and projection arguments.

Layout-root collection follows the same rule. It enqueues const argument types
from standalone instantiation facts and every nested nominal, trait-object,
associated-binding, or projection position so aggregate owners referenced only
by const metadata still receive layout materialization.

Backend function-instance admission applies the same typed-const rule to its
generic-parameter, unresolved-projection, error, and convergence-depth walkers.
Struct and union instance discovery also traverses const argument types in
nominal, trait-object, associated-binding, and projection positions before
requesting aggregate materialization. The backend type-registration frontier
uses the same traversal before adding field owners; preserving the identity
without walking these types would silently omit nested aggregate owners.

LLVM declaration readiness also treats unevaluated const-expression values as
owned dependencies. When a const argument contains a `GlobalConstExprId`, its
module remains in the pending closure alongside the argument type and nominal
owner. The backend validator mirrors this closure by recursively checking all
const-argument types before accepting a type into LLVM lowering.

Executable reachability applies the same owner rule to its type-only module
projection: const generic values and array lengths that remain as
`GlobalConstExprId` references add their expression modules even if those
modules contribute no runtime function or global. This keeps type/layout
products available for reachable signatures without promoting compile-time
owners into the runtime body set.

Before semantic queries run, const-expression input pruning discovers every
`GlobalConstExprId` reachable from active item-tree types. The traversal covers
nominal const args, trait-object/pointee const args, associated-binding
type/const args, and projection const args, including const-argument types that
may themselves contain array-length expressions.

Extension-trait signature indexing uses the same complete owner closure. Its
type-module walk retains source trait and nominal owners, including the trait
identity attached to an associated-type binding, const-expression owners in all
const metadata positions, and array-length expression owners. These modules
remain available when provider discovery expands a signature through nested
type references.

Object-safety validation rejects a source trait with builtin `Sized` as a
supertrait. It also rejects builtin supertraits that expose methods or
associated items until their object-level vtable contract is defined. Trait
objects are erased and therefore cannot satisfy the statically-known-layout
requirement; these checks belong at the body-check object boundary before
vtable construction. Nested object-safe type reconstruction preserves nominal
type and const arguments, including recursively normalized const argument
types, so erased method signatures retain the same identity used by trait
bindings and vtable lowering. Source associated values/consts are rejected at
the same boundary because current vtables materialize methods only and have no
object-level storage or lookup contract for those items.

Upcast validation matches target associated bindings against source bindings as
an unordered one-to-one set with transactional backtracking. A compatible
source candidate is not consumed permanently until the complete target set has
matched; this keeps trait-object coercion consistent with the solver and backend
binding rules.

Trait witness visibility uses the same module-graph predicate as ordinary item
visibility. Directly imported package-visible traits are available to sibling
modules in the package, but not to external callers; visibility filtering must
not reduce this distinction to a `Public`-only check.

Supertrait binding expansion also validates consistency of the inherited graph.
The identity of a constraint includes the parent trait, its type and const
arguments, and the associated type name. The same identity may be reached more
than once (for example through a diamond) when every occurrence has an
equivalent right-hand side; differing right-hand sides are rejected during
signature validation instead of leaving projection normalization or trait
object construction with an ambiguous assumption set.

All body-check entry points read existing handles from the compilation
`TypeStore` and publish inferred or substituted structural types through a
module-scoped `TypeStoreAppend`. They never borrow a mutable interner, and query
providers do not checkout a module shard around body checking. Incremental
`BodyCheckSeed` carries only previously computed `SemanticFacts`; type identity
and availability are guaranteed by the session store rather than a snapshot
prefix contract. Comparison probes share the same canonical store capability
instead of cloning a temporary type view.

It produces two explicit products:

- `BodyFacts`, the body semantic surface: expression types, final
  bracket-suffix resolution, builtin values, call targets, coercions, function
  references, local types, generic instantiations, source-node fact keys, plus
  checked const-derived facts such as array repeat counts and integer pattern
  values;
- `BodyIr`, the runtime checked body boundary: typed function bodies, static
  initializers, and no type-store snapshot. Consumers interpret its typed
  handles through the compilation session.

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
  keys, checked array repeat counts, checked integer pattern values, and recorded
  generic instantiations.
- `BodyIr` is the runtime checked body product. It stores typed function
  bodies and static initializers, but does not own the type interner used by
  those bodies.
- The crate also defines the typed body, statement, expression, place, call,
  aggregate, inline assembly, and control-flow nodes produced after body
  checking.

This crate is source-shaped: blocks, if expressions, match expressions, and
for headers still reflect the checked language form. It is not an optimization
MIR and does not own diagnostics or checking policy. It is the durable data
product of body checking and the input boundary for later lowering,
monomorphization, and backend phases. `BodyFacts` carries semantic side facts;
`BodyIr` carries runtime typed bodies and static data.
Each function entry owns an `Arc<TypedBody>`. This sharing is intentional:
the executable module aggregate and the per-function checked-body query are
concurrent immutable owners of the same payload. Extracting an item query does
not deep-clone the checked body.
Facts derived from const execution during body checking, such as array repeat
counts and integer pattern values, are recorded here so later lowering can
consume checked facts instead of re-running expression evaluation from source
shape. If body IR lowering does not have a checked integer-pattern fact because
the checker already rejected the pattern or the pattern is not an integer fact,
it keeps the original expression-shaped pattern instead of re-running const
evaluation or producing a second diagnostic.
Integer and boolean patterns therefore enter `BodyIr` as checked pattern values
or checked ranges; expression-shaped patterns remain only for patterns whose
semantics are not represented by the integer-pattern fact. Structs and enum
variants enter one typed nominal-pattern representation: its constructor
selects either a struct field projection or an enum tag/payload projection,
while fields are normalized to declaration order. A terminal source `..` is
not encoded as a fake field: omitted fields are materialized as typed wildcard
children so `field_defs[index]` and `fields[index]` remain aligned. `let`, `for`, `match`, and
if-pattern expressions therefore consume the same recursive representation;
only their source control-flow shape and irrefutability requirements differ.

### 9.4 `nia-function-ir`

Defines the lowered function body IR used by backend codegen: function-level
blocks, scopes, operations, terminators, places, callees, locals, builtin
values, inline assembly, and runtime expressions.

Function IR is the current function backend boundary. It removes source-shaped
control expressions from runtime expression trees: block, if, match, for,
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

`FunctionBody::value_refs` records method and trait substitutions as type roots.
For `TraitMethod` and `TraitAssociatedFunction` callees, the selected trait and
method are resolved only after concrete backend instantiation supplies the
implementation and argument module, so the pre-instantiation reference walk
does not manufacture a `FunctionInstanceRef`. It still retains both trait-level
and method-level type/const argument types; once resolution produces a concrete
`Method` or `FunctionInstance` callee, the normal complete instance key is
collected.

Executable trait closure deduplication uses the complete generic-instance
identity, including an optional method receiver (`self_arg`) in addition to the
definition, type arguments, and const arguments. Two calls to the same generic
method with equal explicit arguments but different receiver types can select
different where-predicate witnesses or default implementations and must remain
independent closure nodes.

Each lowered body is owned directly by
`LoweredFunctionBodyQuery(GlobalDefId)`. There is no module-level body store,
storage id, session arena, or second semantic identity. Dropping a retired
query product releases that function's payload without retaining revision
history.

Generated closure entries remain children of that same source-body query.
Their identity is `ClosureId { owner, ordinal }`, not a synthetic
`GlobalDefId`. Function IR records each entry body beside its owning source
body and represents a direct call with a dedicated callee carrying the closure
identity and an explicit readonly state-pointer expression. The generated body
receives that pointer as its first ABI parameter; captured local references are
rewritten to ordered state-field projections, so parent-function `LocalId`
values never cross into the entry body. Stable symbols and backend ABI records
are backend products, not part of source definition identity.

The callable interface type is deliberately separate from generated closure
entry materialization. `Fn(Args...) Return` is represented semantically by the
unsized `TyKind::CallablePointee`; `&Fn(...)` and `&mut Fn(...)` are
`TyKind::Callable` values whose readonly bit is part of type identity. The
interface signature participates in normalization, substitution, structural
equivalence, reachability, monomorphization, persistent signature caching, and
mangling.

Body checking constructs a callable view only when an expected `TyKind::Callable`
guides an explicit `&closure` or `&mut closure` expression. It requires exact
parameter and return signatures, permits a mutable state pointer to become a
readonly view, and rejects the opposite direction. The conversion does not
apply to a closure-state pointer after that pointer has been stored separately.
Generic method argument inference also treats a direct pointer to a
`ClosureState` as its callable signature while the expected callable parameter
is being inferred. This preserves the normal generic method path for synchronous
APIs such as `Source!T::mapError` and `Source!T::orElse`; it does not broaden
callable coercion after a closure-state pointer has been stored separately or
extend its lifetime.
Checked Body IR records the operation as `TypedExprKind::CallableCoercion` and
records calls with `TypedCallee::Callable`; Function IR preserves those as
`FunctionExprKind::CallableCoercion` and `FunctionCallee::Callable`. Both nodes
retain ordinary recursive expression dependencies, while the construction also
carries the owning `ClosureId` used to select the generated entry.

LLVM callable entries use the same return classification as direct Nia calls.
An indirect aggregate return has ABI order `out pointer, state pointer,
arguments...`; direct returns start with the state pointer. Callable-view
function types, generated closure entry declarations, and indirect call sites
all derive that order from the shared return/parameter classifiers.

No-capture closure conversion to the existing thin `&fn` type uses a separate
identity-preserving path. Body checking accepts only an expected-signature-guided
readonly `&closure` whose closure state has no captures and whose parameter and
return types match structurally. Capturing closures receive a dedicated
diagnostic directing them to `&Fn(...)`; mutable address expressions and
intermediate closure-state pointers do not participate. Body IR records the
conversion as `TypedExprKind::ClosureFunctionPointer`, and Function IR preserves
it as `FunctionExprKind::ClosureFunctionPointer`. The node carries the owning
`ClosureId` rather than pretending that a generated closure entry is a source
`GlobalDefId` function.

LLVM codegen declares each generated entry from its stable backend ABI record
and emits it in the same partition as its source function or concrete generic
instance. A direct `FunctionCallee::ClosureEntry` calls that declaration with
the hidden state pointer; `FunctionCallee::Callable` extracts the state and
entry fields from its two-word view and performs an indirect call with the same
ABI. `FunctionExprKind::ClosureFunctionPointer` resolves to a generated thin
adapter for the same entry. The adapter has the ordinary `&fn` signature,
creates a private non-null zero-state token for the duration of the call, and
forwards it as the entry's hidden state pointer. Adapters are keyed by the full
source-or-instance closure entry identity and use a stable symbol derived from
the entry symbol.

Extern method calls use the same complete instance test as ordinary method
lookup: a receiver substitution, type argument, or const argument selects the
materialized function-instance metadata and its C ABI. In particular, a method
with only const arguments is still an instance; it must not fall back to the
non-generic source declaration when deciding whether to emit the extern ABI.
The extern predicate and instance lookup must pass the same complete
`(self_arg, args, const_args)` identity. Looking up extern metadata with an
empty const-argument vector can silently classify a valid const-only instance
as an ordinary method and add the hidden Nia state parameter to its call.

The LLVM backend validator treats generated entries as first-class backend
products before codegen. It validates the hidden state parameter as a
readonly pointer to the published closure-state type, checks user parameter
order and local identities against the ABI record, and validates the generated
Function IR body and return type. This keeps malformed closure products from
reaching LLVM before declarations and bodies are emitted.

Atomic builtin type admission is target-relative before Body IR construction.
Fixed-width integers retain their declared width, while `isize`, `usize`, and
ordinary object pointers use the configured target pointer width; the checker
then rejects values wider than that width. This rule is shared by source-level
ordering/type diagnostics and the backend's pre-LLVM validation contract.

`nia-closure-check` is the independent semantic stage for this escape boundary.
It consumes stable `nia-body-ir::TypedBody` products and the session
`TypeStore`; it does not depend on `nia-body-check`, compiler queries, Function
IR, or LLVM. The stage first computes a monotone cross-function fixed point.
Each function summary records which parameters may be returned and which may be
retained by a store or call. Closure bodies use the same model with explicit
capture inputs. A second pass propagates those summaries together with
`ClosureId` stack-state provenance through aggregates, places, control-flow
joins, and nested closure scopes. Diagnostics retain their owning
`GlobalDefId`, so compiler-query maps them to the correct source module without
guessing from a potentially reused span. Unknown calls and dynamic dispatch are
conservative by design. This is a bounded callable-view escape check, not a
general borrow checker. Captured local addresses use a separate provenance
category and are rejected only when the containing closure state can escape.
The summary fixed point is exercised by a compiler-query regression with two
mutually recursive functions returning the same callable parameter; the
returned stack-backed view must still be rejected after both summaries grow.
Explicit allocator-backed ownership remains outside this crate: the standard
library's `Allocated[T]` and `CallableAllocation[V]` APIs use ordinary typed
pointers, layout values, integer/raw-address boundaries, and explicit `deinit`;
the compiler does not know about allocators or heap policy. Success and error
payload provenance are tracked separately so an error-only stack address cannot
contaminate a successfully allocated value.

The allocator protocol keeps non-empty ownership transitions explicit. A
successful resize to an empty layout must itself retire all ownership state;
otherwise it must return `false` so the default `realloc` creates an empty block
and frees the old owner through the allocator's fallible release channel. This
prevents allocator-specific tracking headers from becoming unreachable through
a zero-sized `Block` whose later `free` operation is intentionally a no-op.
Collection cleanup follows the same explicit-owner rule. `HashMap::deinit`
attempts its control, key, and value releases independently, detaches only the
successfully freed allocation slots, and retains failed slots for a later retry
with the same allocator. After a cleanup error the map is cleanup-only state;
ordinary lookup or mutation is not supported until ownership is fully retired.
Hash-map capacity is a logical entry bound rather than a count of untouched
control bytes. Removing an entry makes one assume-capacity insertion legal even
when the empty-slot growth budget is exhausted; insertion may land in an empty
or deleted slot while the budget remains saturated until a reserve-triggered
rehash restores the normal empty-slot ratio.

### 9.5 `nia-function-lower`

Lowers `nia-body-ir::TypedBody` from `BodyIr` into
`nia-function-ir::FunctionBody` plus any generated closure entry bodies owned
by that source body. This crate owns the translation from
source-shaped checked runtime bodies into explicit function blocks, scope
edges, terminators, defer bodies, locals, builtin values, and inline assembly
options.

This split keeps the Function IR data model reusable by validation, analyses,
backend lowering, optimization, and codegen without making the IR crate depend
on the source-shaped body IR that currently feeds it.

Production lowering receives a `FunctionTypeContext`: existing type handles are
read from the session `TypeStore`, while synthesized types such as iterator
optionals are published through a module-scoped `TypeStoreAppend`. Function
lowering does not checkout or snapshot type storage.

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

### 9.7 `nia-executable-facts`

Extracts executable dependency edges from typed bodies and retained semantic
facts. Typed IR is authoritative after body checking; semantic facts provide
the equivalent owner-indexed view for query paths that have not materialized a
typed body. The product records functions, globals, trait/vtable references,
and concrete generic instantiations without retaining complete body payloads.

Monomorphic function values contribute only their definition identity. A
`TypedExprKind::FunctionInstance` value contributes both that definition and
one `GenericInstantiation` containing its complete type and const arguments,
the same identity contract used by a direct generic callee. This applies to
function-pointer values and other expression containers, so taking a function
address cannot silently downgrade a concrete instance into a generic
definition-only dependency before reachability or backend planning.

### 9.8 `nia-executable-reachability`

Computes the module and function set required for freestanding executable
codegen. It starts from the root `main` and freestanding `_start` runtime roots,
then walks typed bodies, semantic facts, trait/default methods, extension
witnesses, and owner modules for referenced types.

This crate deliberately returns reachability sets instead of owning
`CheckedModule`. `nia-compiler-query` keeps query orchestration and module
filtering, while this crate owns the executable pruning analysis. That boundary
prevents provider code from becoming a typed-body traversal or semantic-fact
analysis module. Its inputs borrow the session `TypeStore` as the sole type read
source. Generic substitution uses a separate canonical append capability, so
the analysis cannot depend on module snapshots, recursive import, or paired
argument interners.

Trait/default-method closure keeps exact reachability output identities, but its
recursive supertrait guard is path-local and semantic. Rebuilt type handles and
signed/unsigned spellings of the same integer const cannot reopen or suppress a
recursive branch, and different receiver types remain independent.

## 10. Monomorphization And Symbols

### 10.1 `nia-monomorphize`

Collects concrete generic function and method instances required by the checked
program. It deduplicates exact instance keys for symbol uniqueness and uses
recursive-expansion guards to diagnose cycles. This exact-key deduplication is
a required correctness invariant at every optimization level; the
`dedup_monomorphized_instances` policy match reports that the monomorphization
boundary participates in size policy, but it does not make exact-key
deduplication optional or permit merging instances with distinct symbol
identity.

The collector mutates the compilation `TypeStore` for instantiated structural
types. Its output contains instance keys and diagnostics, not an alternate set
of type interners. Module inputs contain semantic facts only, without paired
interner snapshots or prefix contracts. Existing handles are read from the
canonical store and generated handles are published through module-scoped
append capabilities.

Associated-type instantiation uses a path-local semantic projection guard. The
guard compares projection self/type arguments structurally and integer const
arguments by bits, while unresolved const-expression identities remain exact;
it does not replace the exact instance-key deduplication or type-instantiation
cache contracts used for symbols and incremental products.

### 10.2 `nia-mangle`

Builds deterministic internal symbol names from module ids, definition ids, and
type encodings. It is not C++ or Rust mangling. It should stay readable and
debuggable.

Callable encodings include parameter arity and ordered parameter/return type
encodings. Readonly views use `callable_read`, writable views use `callable`,
and unsized interface pointees use `callable_pointee`, keeping all three
semantic identities distinct.

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
re-run semantic analysis. It stores typed handles but does not own or snapshot
the compilation type store; consumers receive the session store explicitly at
the backend boundary.

Before LLVM emission, `nia-codegen-llvm` validates Backend IR for Function IR
structure, cross-module function/global references, static initializer address
paths, aggregate field/variant references, evaluated array lengths, and ABI
layouts needed by runtime values.

### 11.2 `nia-backend-lower`

Lowers checked modules into backend IR. It uses definitions, lowered types,
signatures, layouts, semantic facts, per-item function/static IR,
monomorphized instances, and public module information.

It owns translation from semantic expressions into typed backend expressions,
places, statements, static initializers, and inline assembly operands.

Backend lowering consumes body semantic facts for expression types, call
resolution, and coercions, and consumes typed runtime bodies for function
lowering. Typed bodies are not exposed through backend IR as the function
codegen boundary.

Projection instantiation keeps a path-local semantic active stack. Recursive
associated-type expansion compares projection self/types and const arguments
structurally, including integer bits, instead of hashing raw interner handles;
the stack is popped after each expansion so sibling projections remain
independent. Exact type-instantiation caches continue to use their exact keys.

Duplicate trait-object vtables retain an exact `(self type, object type)` owner
key, but their repeated payload validation uses structural type equivalence and
semantic integer-const comparison. Function and module identities, method and
slot identities, and unresolved const-expression IDs remain exact, preserving
ABI/output ownership while avoiding false conflicts from equivalent payload
spellings.

`GlobalDefId` is both the semantic identity and query key of a function; there
is no separate body storage identity or body store. Immutable snapshots use
`Arc` only when they have concurrent owners, a single-call read path uses a
borrow, and a unique consumer receives owned data.

`ExecutableFunctionBodyQuery(GlobalDefId)` publishes a semantic-value checked
body product. `LoweredFunctionBodyQuery(GlobalDefId)` depends only on that
item product and owns one `FunctionBody`. The executable fixed point publishes
`ExecutableCheckedModuleFactsQuery` without any function-body payloads; the
checked-body item query lowers exactly one function from that function's frozen
semantic facts, and `ExecutableCheckedModulesQuery` assembles its aggregate view
from those item products. A body edit can still re-execute checked-body item
queries because their semantic facts are aggregate inputs, but
semantic equality preserves unchanged body fingerprints, so their lowered query
products validate green without executing. Checked-body production is per item,
while its semantic-analysis input remains aggregate-shaped.
Monomorphization and backend lowering share the same cache-owned handles.
Backend input assembly builds a short-lived whole-program index from references
to those query payloads; module-local lookup uses that same borrowed index and
does not clone bodies into a second owned map.

`MonomorphizationQuery` and `BackendLoweringQuery` are the tracked owners of
their aggregate products. The codegen program holds the exact cache-owned products;
the backend aggregate depends on monomorphization through module-local instance
plans rather than receiving the aggregate product directly. This provides one
red-green owner for each aggregate stage and prevents repeated public or
internal codegen requests from executing either stage again. It does not make
backend lowering item-grained: the current backend query still assembles all
module inputs, performs the cross-module function/global-instance fixed point,
and publishes one `BackendProgram`.

`BackendModuleSourceItemPlanQuery(ModuleId)` owns the deterministic
frontend source-item projection for one module. It derives sorted, deduplicated
module-local function, global, struct, and union keys from
`ExecutableCheckedModuleFactsQuery`; executable backend lowering consumes those
keys instead of reading reachable sets from a checked-module payload. Every
planned source function is materialized by its owner module's first pass, so a
cross-module reference to a source body already present in the query-owned body
index does not re-enter the outer backend fixed point. Bare, public, and
type-only root policies keep their function-root behavior. When a typed
struct/union plan is present, every root policy, including bare
`FunctionBodies`, filters initial source aggregates through that plan. The
backend must not emit all aggregates in an activated body module: doing so can
introduce nominal field owners and generic instances that are absent from the
closed executable type/layout set. Aggregate completion may add dependencies
of planned items, but missing-owner recovery is not a substitute for honoring
the producer plan.

`BackendModuleFunctionInstancePlanQuery(ModuleId)` is the corresponding narrow
boundary for frontend-discovered generic function instances. It validates the
requested executable module, filters the monomorphization aggregate by the
definition owner's `ModuleId`, orders instances by their deterministic mono
symbol, and rejects duplicate semantic instance keys. The plan retains the
argument module because type arguments are interpreted in that context. The
public backend-lowering API consumes only these module-local DTO slices.
Monomorphization details stay behind the query boundary: `nia-backend-lower`
does not depend on `nia-monomorphize`, and `BackendLoweringQuery` does not read
`MonomorphizationQuery` directly.

The module source and frontend function-instance plans cover only items known
before materialization. Function-body and vtable-induced instances, generic
global instances, vtables, and instance-induced source references converge from
closed call-scoped discovery deltas. Layout completion, module optimization,
and DCE execute after that aggregate closure. Only the complete closed plan may
cross the immutable module-query boundary; an incomplete module product must
not be cached and then mutated from an external worklist.

The aggregate cross-module closure drains newly discovered source functions,
function instances, and global instances into one iteration-local
`ForeignBackendItemPlan`. Exact semantic keys are deduplicated before grouping
by definition owner, source functions are ordered by `GlobalDefId`, and owner
modules are consumed in module-plan order. References produced while consuming
one snapshot enter the next snapshot instead of mutating the active batches.
Duplicate module owners and references to an owner absent from the module plan
are compiler errors; neither case is silently truncated or dropped.

This call-scoped plan is an internal convergence boundary rather than an
independent query product.
Concrete generic local-static global keys only appear when a function template
is substituted into a concrete backend function instance; the pre-backend
`FunctionBody` still contains the source local-static identity. Vtable-induced
function instances have the same post-substitution dependency. Consequently,
the authoritative global-instance and vtable plan is derived from closed
substitution results, not projected from source Function IR.

Function and global instance substitution returns a closed materialization
delta: the newly owned backend payload, every source-function,
function-instance, and global-instance reference, and every trait-object vtable
discovered from that concrete body or initializer. Source functions use the
same discovery shape. Nested instance expansion and the outer module closure
consume the same delta. Additional cross-module items seed the next worklist
from this result instead of rescanning all previously materialized functions
and globals. This is particularly important for a generic local static or a
trait-object coercion inside a generic function: their concrete edges only
exist after substitution and cannot be reconstructed from the source template.

The delta is moved within one backend query call and has no independent
identity or shared owner, so it is not stored behind an ID or `Arc`. Vtable
discovery walks only the newly produced concrete body; new entries immediately
enqueue their source/default-method instance references, and the module only
deduplicates vtable semantic keys. No aggregate collector or post-optimization
reachability rescan participates in this path.
Devirtualization, cross-function constant propagation, and inlining may remove
or copy already discovered edges but must not create a new semantic
reachability edge. The resulting closure is complete before it is published as
the consuming `BackendItemPlanQuery` product.

Initial modules remain materialization-only while the cross-module closure is
active. The backend drains every deterministic foreign-item snapshot before it
finalizes any module. Each owner module is then finalized exactly once:
devirtualization, cross-function constant propagation, inlining, DCE, reachable
aggregate/instance completion, and final layout construction all observe the
same closed item set. Additional-item handling therefore does not optimize a
partial module or repeatedly rebuild its instance layouts. This ordering is a
prerequisite for a consuming global item-plan query because the query product
cannot mix pre-closure optimized items with unoptimized late items.

The closed result is represented by a consuming `BackendItemPlan`. Planning
owns the complete, unfinalized module item sets plus diagnostics and
materialization-time optimization changes; it does not retain a `ModuleLowerer`
or borrow its substitution and trait caches. Finalization validates the module
owner sequence, rebuilds the read-only finalizer indexes from the original
module inputs, and moves the planned modules into the final `BackendLowering`.
The production compiler exposes planning as `BackendItemPlanQuery` and
finalization as its sole consumer inside `BackendLoweringQuery`. The plan uses
`SingleConsumerOwned` storage: query execution moves it directly to
finalization, while the payload-free consumed slot preserves dependency and
execution metadata. `backend_item_plan` depends on the per-module source and
function-instance plans, and `backend_lowering` depends on the consumed plan, so
upstream invalidation still propagates through the complete boundary. The plan
intentionally does not implement `Clone` and is not wrapped in an ID, store, or
`Arc`; repeated backend/codegen requests reuse the finalized backend product
rather than reproducing or duplicating the plan.

The aggregate plan is physically partitioned into non-`Clone`
`BackendModuleItemPlan` values only after cross-module closure has converged.
Finalization validates their owner order, rebuilds the program-wide read-only
indexes, and then consumes each module plan with `into_iter`. The resulting
`BackendLowering`, `BackendProgram`, and `BackendModule` also do not implement
`Clone`. Allocation-identity regression checks show that the function and
global vectors are moved from the module plan into the finalized module without
changing their backing allocations. This is the module ownership boundary used
by independently scheduled finalization queries; source modules are still not
claimed to be final CGU work products.

The partition crosses a formal per-module query boundary. The sole
`BackendItemPlanQuery` consumer destructures the aggregate once and publishes
each raw module payload directly to
`BackendModuleItemPlanQuery(ModuleId)`. Finalization then consumes those slots
by value; the module queries never execute the whole-program closure, retain an
`Arc`, or use a side store. Their dependency edges point back to the aggregate
plan, so invalidation drops an unconsumed module payload and causes a consumed
slot to be republished on the next backend request. Query regressions assert
that production leaves every module-plan slot payload-free. This establishes
module-keyed ownership and independently scheduled finalization inputs, but no
CGU partition is claimed yet.
The instrumented compiler records Rust global-allocator current and peak
live bytes, including already-live instrumented allocations at the detail
timing boundary. Backend fan-out emits snapshots before publication, after all
module slots are published, and after all are consumed. These counters expose
whether scheduling changes create a transient heap spike; process RSS remains
the authority for LLVM/native allocations that the Rust allocator cannot see.

Module finalization has an explicit task-shaped ownership boundary.
`BackendProgramFinalizationContext` contains only the program-wide read-only
indexes, type store, optimization policy, and timing flag; both that context and
`BackendLowerModuleInput` are compile-time checked as `Send + Sync`. Each
`finalize_module` call consumes one `BackendModuleItemPlan` and returns a
`Send`-checked `BackendModuleFinalization` that exclusively owns its finalized
module, diagnostics, and optimization changes. Results carry their original
batch position, and the sole merge function restores program order before
combining modules, diagnostics, or optimization changes, so task completion
order cannot affect output.

Production owns the complete read-only environment in
`BackendLoweringInputsQuery`. It keeps the existing immutable query handles and
derived program indexes alive without copying function bodies or static
initializers, and implements the single object-safe `BackendProgramFacts`
contract consumed by backend lowering. `BackendFinalizationTaskContextQuery`
shares that owner and the canonical type-store handle across the real set of
module tasks. Each `BackendModuleFinalizationQuery` then consumes one published
module plan and produces one non-`Clone` finalization result through
`SingleConsumerOwned` storage. `BackendLoweringQuery` dispatches those keys with
`get_many_owned` on the session's persistent executor and merges the returned
values in original module order. The `Arc` on the inputs/context side represents
actual concurrent consumers; module plans and results remain exclusively owned.
The source-module task split is an execution boundary, not a substitute for
deterministic CGU partitioning or CGU work-product caching.

The allocator instrumentation also provides a process-wide live window around
that `get_many_owned` call. It records start, end, and peak live Rust heap bytes
across the calling thread and all executor workers; overlapping windows are
rejected rather than merged. The maintained multi-module backend workload
requires these counters. Three instrumented release samples measured a median
peak growth of 1,102,630 bytes (about 0.64% of the window start), while the
whole-compilation allocator peak remained higher than the finalization-window
peak. This validates the current source-module task shape, but it is not a
budget exemption for later, finer CGU partitions.

The root checked and lowered bodies reuse `GlobalDefId` as their semantic and
query identity. Nested source-shaped bodies remain structurally owned by their
function because they have no independent cache, invalidation, release, or
cross-structure reference boundary; no synonymous `TypedBodyId` is introduced.
The aggregate checked-module view and the item query share one
`Arc<TypedBody>` allocation because both are live owners; the body retains
`GlobalDefId` as its sole semantic and query identity.

Materialization copies a body only when creating the corresponding
`BackendFunction` or `BackendFunctionInstance`. Generic-instance reference
discovery scans the body already owned by the newly appended backend instance,
rather than cloning a temporary discovery body. Checked-body production is
item-owned, while its frozen semantic-fact input is module/executable-aggregate
shaped. Backend materialization remains aggregate until cross-module closure
has converged, then finalization consumes the module-owned partitions.

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

`StaticInit` is the data-initialization boundary and remains separate from
function IR.

For `StaticInit::AddrOfFunction`, the concrete function identity is the pair
of declaration-order type arguments and const arguments. Backend lowering,
reachability, validation, LLVM lookup, and fingerprints must carry both
vectors; reducing a const-generic address to its declaration alone can select
the wrong monomorphized function.

`GlobalDefId` is also the semantic identity of a static initializer; there is
no separate `StaticInitId` or static-init store. `BodyIr.global_inits` shares
immutable `Arc<StaticInit>` payloads with
`ExecutableStaticInitQuery(GlobalDefId)`. The executable facts fixed point does
not retain complete initializer trees. It keeps sorted runtime-global keys and
per-global `FunctionBodyRefs` summaries containing complete executable
function, global, and concrete function-instance identities; reachability
consumes those summaries instead of recovering edges from an aggregate payload.
Zero-count repeats deliberately contribute no references.

The item query materializes exactly one initializer from frozen checked facts
with `StaticInitOnly`; a local static temporarily promotes the node facts owned
by its enclosing function into the item lowering view. The already-checked
global is not type-checked again. `ExecutableCheckedModulesQuery` reconstructs
its aggregate view from the item products, so there is no path that extracts an
item payload from the facts aggregate. Facts-only checking lowers a transient
tree once to preserve the single static-data
representability and diagnostic implementation, derives the same complete
`FunctionBodyRefs` value-reference summary as typed Body IR, and immediately
releases the tree.

Semantic-value equality lets an unchanged initializer remain green even when
its aggregate facts input causes the item query to execute again. The aggregate
view and item query share one `Arc<StaticInit>` allocation because both are live
owners; `GlobalDefId` remains the initializer's sole semantic and query identity.

Backend input assembly keeps the query handles alive and builds one
call-scoped `GlobalDefId -> &StaticInit` index. `nia-backend-lower` receives
that index rather than `BodyIr` or an owned initializer map. A non-generic
`BackendGlobal` makes its one required owned copy at materialization; a generic
global must additionally produce an independent tree because type
substitution rewrites the initializer. Size optimization consumes that owned
tree and returns a changed flag with the simplified value.

## 12. LLVM Backend

### 12.1 `nia-llvm`

Provides thin wrappers around LLVM APIs. It should keep unsafe and FFI-heavy LLVM
interaction isolated from language phases.

Typed GEP operations keep their LLVM layout preconditions at this boundary.
`Builder::build_gep` and `Builder::build_struct_gep` are explicitly `unsafe`:
the caller must establish pointee/layout compatibility, valid indices, and
pointer provenance before LLVM receives the handles. The code generator derives
those facts from checked backend projections and documents the corresponding
unsafe call sites. The unused pointer-difference wrapper was removed rather
than exposing an unchecked provenance contract with no consumer.

Aggregate extract/insert wrappers are checked at the same boundary: they
inspect the actual LLVM aggregate type, reject out-of-range struct/array
indices, and require inserted values to have the selected field type before
calling LLVM. This keeps malformed aggregate IR out of the backend without
spreading `unsafe` blocks across ordinary lowering code.

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

LLVM physical type lowering represents a callable view as the literal
two-field aggregate `{ ptr, ptr }`. Bare `CallablePointee` types never reach
physical lowering because they have no runtime layout. This representation is
distinct from the single LLVM pointer used for `TyKind::FunctionPointer`.

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
type arguments whose const expressions normalize to the same value.

Module-codegen aggregate and function-instance fallback scans use one shared
semantic const-argument matcher: const types compare through structural type
equivalence, integer values compare by bits, and unresolved non-integer values
retain their exact identity. Exact map keys, declaration ownership, and output
identity remain byte-for-byte exact; semantic matching is limited to fallback
resolution and validation so caches and ABI owners do not acquire aliases.

Declaration linkage membership uses that same semantic relation when deciding
whether a function or global instance in the current partition is its concrete
definition. Definition and argument-module identities remain exact, while
receiver/type/const payload spelling can vary across equivalent lowering
products without incorrectly downgrading the definition to an external
declaration.

LLVM entry points receive the compiler-owned `Arc<BackendLowering>` and
`Arc<TypeStore>` together with the producing compiler's `QuerySession`.
The whole-program index owns those two shared roots, so validation and emission
resolve every handle through the canonical immutable kind arena without module
checkout, copied interners, owner-module discovery, or a lock per recursive
type lookup. Module maps remain indexes for backend items and layout facts, not
a second type-interpretation path.

`ProgramIndex` has exactly those two owned shared roots. Every module, item,
instance, vtable, and layout map stores compact module/item positions rather
than references into the program. Accessors resolve those positions back to the
canonical allocation; they do not clone Backend IR or establish a second item
store. This removes the
self-referential map shape and makes the readonly context `Send + Sync +
'static`. One `Arc<ProgramIndex>` is built before validation and shared by all
unit tasks; no task rebuilds the whole-program index.

Finalized backend lowering also publishes one `CodegenPartitionPlan`. Each
source unit has two non-synonymous identities: `CodegenUnitId::SourceModule {
module_id, ordinal }` locates work only inside the current session, while
`CodegenUnitKey::SourceModule { source_identity, ordinal }` is stable across
module-handle reallocation and is the persistent work-product key. Source
modules below eight definitions remain one ordinal-zero unit. Larger modules
use at most four fixed buckets: ordinary globals/functions use their stable
source-local `DefId`, concrete instances use their stable mangled symbol, and
vtables remain in bucket zero rather than hashing session-local type handles.
Only non-empty buckets become units; once the split threshold is active, adding
an item does not shift all later ordinals. Units are ordered by stable key rather than `ModuleId` or
`BackendProgram.modules` position. Each plan entry keeps one module index plus
position-only definition membership for globals, global instances, functions,
function instances, and vtables; it never copies Backend IR. This membership is
the sole authority for LLVM initializers, function bodies, vtable definitions,
and the unit definition fingerprint. Declaration-only modules remain available
to the whole-program `ProgramIndex`, but do not become codegen work units. The
plan is reconstructed and validated against the program at the LLVM boundary,
so a caller cannot pair a stale plan with mutated definition membership.

LLVM IR and object emission consume this explicit plan instead of deriving units
from a second module loop. Both output forms carry the runtime ID and stable key.
Required compiler builtins use the distinct
`CodegenUnitId::CompilerBuiltins` synthetic identity and are appended only to a
native object result that actually needs them; a source module name can no
longer stand in for that role. Validation and cross-unit declaration/layout
lookup intentionally remain whole-program and readonly.

Every emitted unit also carries a `CodegenUnitFingerprint`, which is content
identity rather than location identity. Its encoder is explicitly versioned
and writes fixed-width values into the stable query fingerprint builder; it
does not hash `Debug` text, standard-library randomized hash state, `ModuleId`,
or raw `InternedTyId` slots. Module references are remapped to normalized
`SourceIdentity`, definition references pair that identity with `DefId`, and
types are encoded recursively from canonical `TyKind` together with their
resolved layouts.

A source-unit fingerprint contains the unit's complete definitions, including
function bodies and static initializers, plus the exact canonical declaration
membership required by those definitions. That dependency closure covers the
referenced ABI, vtable, const, and layout surfaces; declarations from unrelated
modules are excluded together with other units' bodies and initializers. A
referenced cross-module ABI/layout change therefore invalidates its consumers,
while unrelated declaration or body edits do not evict every CGU. Optimization
policy, artifact kind, compiler fingerprint schema, package version, and the
LLVM wrapper codegen ABI are part of the domain. Native objects additionally
include the exact LLVM target triple, CPU, and feature string used to construct
their target machine. Compiler builtins use their own domain over the requested
symbol set and the same policy/target inputs.

Spans and display-only local names are excluded because they cannot affect the
object product. Differential tests require stability across `ModuleId`, type
store slot, and module input-order changes, while definition, initializer,
cross-module ABI, optimization, and native target changes must alter the
fingerprint. The fingerprint is computed before creating the LLVM context or
module; hashing emitted IR or object bytes would be too late to support
work-product reuse.

Native object reuse crosses one explicit `ObjectWorkProductCache` boundary.
The LLVM crate owns the lookup timing but no filesystem policy; the Driver owns
the sole persistent implementation and enables it from the CLI/build cache
directory. Each unit task computes its target-specific fingerprint and performs
the lookup before acquiring an LLVM memory permit or creating a target machine,
context, or module. A hit reconstructs the normal typed object result with its
current session ID and stable key. A miss follows the only codegen path and
publishes the resulting bytes after successful emission.

Persistent entries live under
`artifacts/objects/<registered-namespace>/<stable-key-digest>/<full-fingerprint>.o`. The full
fingerprint is a versioned aggregate of four independently versioned components:
compiler/codegen policy, unit definitions, the exact declaration/ABI dependency
closure, and native target identity. The binary envelope records the aggregate, all four
components, canonical unit key, payload length, and a domain-separated payload
checksum. Reads recompute the aggregate from the stored components, validate the
key and content-addressed path, and reject trailing or truncated data. No prior
registered namespace is read.

An exact aggregate match is the only cache hit. On an exact miss, the Driver may
inspect validated entries in the same stable-key directory and compare against
the cached version with the fewest changed components, using the aggregate as a
deterministic tie-breaker. This reports policy, definition, declaration, and
target invalidation independently without a mutable latest-entry index. Prior
content-addressed versions remain immutable. A corrupt entry is physically
deleted and becomes a cache miss. Publication writes and syncs a unique
same-directory temporary file before atomic rename, so interrupted writers are
never visible as work products and concurrent identical publishers converge on
the same entry. Cache I/O failure can only lose reuse, never replace the
compiler's current Backend IR or object result.

Native object emission publishes one `IncrementalLinkInputs<NativeObject>`
product rather than a module list. Each entry keeps its stable
`CodegenUnitKey`, content `CodegenUnitFingerprint`, and object payload together;
the collection constructor rejects duplicate or descending keys, so ordering is
part of the typed contract instead of linker policy. The Driver may change only
the payload representation while writing objects, producing the same keyed and
fingerprinted `IncrementalLinkInputs<std::path::PathBuf>` product. The linker
accepts that typed collection directly and emits object arguments in its
existing order. It has no plain path-list entry point, no key recovery from file
names, and no secondary ordering truth source.

This boundary makes the exact ordered CGU work-product set available to
link-result fingerprinting. `nia-linker` owns the versioned canonical
`LinkResultFingerprintSet`. Its four independent component domains cover the
ordered CGU keys and fingerprints, target-derived facts, the resolved linker
path/binary/flavor, and structured link options respectively. A versioned
aggregate domain combines those component fingerprints; exact aggregate
equality is the only reuse condition. The encoder uses fixed discriminants and
length-delimited values and does not hash `Debug` output or temporary
object/output paths. Linker and archive-tool binaries are fingerprinted from an
opened file through a fixed 64 KiB buffer; truncation or growth rejects that
observation without allocating the complete executable. Links with a sysroot,
explicit native libraries, or raw linker arguments are not declared cacheable
because those options may name
external files outside the tracked input set.
Linux host dynamic-linker discovery likewise reads only the fixed ELF header,
each declared program header, and a bounded 4 KiB interpreter payload. Invalid
offset arithmetic, out-of-file tables, concurrent truncation, and oversized
interpreter records are rejected before allocating from file-controlled sizes.
Host `ld.so.conf` discovery canonicalizes and visits each configuration once,
sorts include matches before consuming them, and enforces per-file, aggregate,
file-count, and directory-entry budgets. Consequently native default library
paths and their link-result identity do not depend on directory enumeration
order or unbounded configuration input.

The Driver owns the sole persistent link-result cache. It computes the
fingerprint and attempts restoration before writing temporary object files or
invoking the linker. A miss invokes the linker over the complete typed input
collection and publishes only a successful executable. Entries live under
`artifacts/links/<registered-namespace>/<stable-input-key>/<full-fingerprint>.link`; their binary
envelope records a magic/schema, stable cache key, aggregate and component
fingerprints, payload length, and a domain-separated payload checksum. The
cache recomputes the aggregate and verifies the content-addressed path before
accepting an entry. Publication uses a unique same-directory staged file and
atomic rename, while a corrupt entry is physically deleted and treated as a
miss. Restored files are made executable. No prior registered namespace is read. Cache
I/O errors only lose reuse and never become an alternate link truth source.

CLI executable emission and the build runner use the same configured Driver for
object and link-result reuse. This cache has one registered namespace and one
fingerprint model, with no compatibility reader or fallback fingerprint. A miss
performs one full link; the cache does not provide partial relinking.

Work-product lookup outcomes are typed rather than collapsed into `Option`.
Timing reports count object and link-result hits and misses, and separate misses
caused by a disabled cache, an uncacheable link input, an absent entry, a
corrupt entry, or a cache read error; link-result publication errors are also
counted. These reasons describe facts observed during the current lookup. They
do not consult a persistent "latest fingerprint" manifest, so observability
metadata cannot become a second cache truth source. Object misses additionally
report the four versioned CGU component differences described above. A
link-result miss scans only validated immutable entries under the same stable
input key and reports differences across its `inputs`, `target`, `linker`, and
`options` components. The candidate with the fewest differing components is
selected, with aggregate fingerprint as a deterministic tie-break. This scan
does not participate in hit correctness, replace prior content-addressed
versions, or create a mutable latest-entry index.

After whole-program validation, each source partition crosses an independent
LLVM emission boundary. That boundary creates and consumes its own LLVM
`Context`, `ModuleCodegen`, and native `TargetMachine`, returning exactly one
typed IR/object result or one diagnostic. Source partitions and the optional
compiler-builtins object are submitted through `QuerySession::run_tasks`; task
completion order is hidden by submission-order result slots. Each task acquires
its own process LLVM memory permit before allocating LLVM state, so CPU fan-out
remains subject to both the session executor budget and heavy-memory
backpressure. The outer aggregation layer owns no module-local LLVM state.

This is a bounded source partition policy, not a profile-guided final CGU model.
`ModuleId` is a process-local owner identity, vtables remain together, and
validation is a whole-program predecessor. Frontend and LLVM execution do not
overlap. Quantitative CPU and RSS acceptance belongs to the maintained
performance suite. Link-result reuse operates on the whole result rather than
partial relinking.

LLVM object emission maps the Nia optimization level to LLVM's codegen
optimization level and a reported codegen size policy. Size-oriented levels
(`-Os` and `-Oz`) also remain visible in the Nia policy so monomorphization,
inlining, specialization, and deduplication can make size-aware decisions before
LLVM sees the program. The native LLVM target machine is configured with the
mapped codegen optimization level. The size policy is reported and preserved at
the Nia/codegen boundary for size-aware Nia lowering; it is not a separate LLVM
target-machine knob.

It should not parse AST or make frontend semantic decisions.

## 13. CLI

### 13.1 `nia-cli`

The package is `nia-cli`, and the compiler binary is `nia`. This crate owns
command-line parsing, explicit toolchain-layout selection, outer timing and ICE
boundaries, and conversion from command options into typed Driver or build
requests. It does not reproduce compiler phases, cache policy, or build-plan
execution.

The command families are:

- `check`, which validates a program through a typed `nia-driver` request;
- `emit`, which exposes tokens, AST, checked state, backend IR, LLVM IR,
  object files, and executables through the owning compiler boundary; and
- `build`, which resolves a `BuildRequest` and delegates package discovery,
  runner execution, plan validation, scheduling, caching, and publication to
  `nia-build`.

The executable help output is the command and option spelling authority. The
repository README provides user-oriented invocation examples; duplicating the
complete option matrix here would create another CLI definition.

Every source-tree invocation selects `lib` as an explicit resource root.
Installed invocations resolve their relocatable resource layout through
`ToolchainLayout`. CLI code passes that typed layout into Driver and build
requests and never derives production resources from a compile-time checkout
path. The fixed-field `toolchain.meta` compatibility manifest is read through a
64 KiB stream limit before UTF-8 parsing, so an oversized installed resource
cannot allocate from its full file length or smuggle a valid prefix.

Compiler commands preserve the distinction between bare/object/IR emission and
freestanding executable startup described by the language and ABI references.
Native output commands create missing output directories, but never create
input source or module-map paths. Timing and optimization reports use their
typed reporting channels rather than sharing stdout with emitted IR or native
artifacts.

The Rust build owner and its persistence boundaries are documented in
[`crates/nia-build/README.md`](../crates/nia-build/README.md). The build-script
API is documented beside [`lib/std/build.nia`](../lib/std/build.nia), and
standard-library filesystem and process behavior belongs to the corresponding
`lib/std` facades and providers.

The generated runner's plan encoder checks every collection and derived graph
count before narrowing it to the fixed-width protocol representation. Checked
aggregate addition covers package/artifact totals as well as action input,
output, and dependency counts; oversized generated-file payload lengths are
rejected before their length prefix is emitted. This keeps malformed
in-memory runner state from wrapping into an apparently valid smaller plan.
The Rust re-encoder enforces the total plan budget before every buffer growth
and retains the first attempted-size failure through `finish`, so canonical
publication cannot transiently allocate an oversized serialized plan either.
Build graph cleanup applies the same ownership rule recursively: a list backing
allocation is released only after every owning element succeeds. Failed nested
arguments, environment entries, imports, targets, modules, and steps therefore
remain reachable for a later cleanup retry.
On the receiving side, list counts are validated but never used to reserve
typed Rust capacity before item bytes are parsed. This prevents malformed
truncated drafts from turning a small count prefix into a large host allocation.
The standard filesystem close API consumes its descriptor before issuing the OS
close. Build-plan publication clears its fallback flag before the explicit
close, preserving the first close error without a second `BadFd` attempt.

Linux process spawning uses a close-on-exec pipe as a fixed-size child-to-parent
error handshake. The child writes the complete stage/errno record and retries
interrupted writes; the parent retries interrupted reads and reaps a failed
child with an EINTR-safe wait loop. EOF before any record remains the sole
successful-exec signal. Once a public `Child` records an exited status, pipe
cleanup attempts every still-owned stdin/stdout/stderr handle and returns only
the first close error, so one failing close cannot strand later handles behind
the cached status fast path.

The public `std::io::Reader` and `Writer` contracts require every implementation
to report no more bytes than the slice supplied to that call. Trait defaults and
all standard adapters validate this boundary before advancing a cursor,
buffer, or `LimitedReader` budget; an invalid count is converted to the
adapter's end-of-stream or short-write error while preserving buffered state.
The OS file-handle and child-pipe facades enforce the same rule at their syscall
boundary, so direct consumers cannot bypass the adapter invariant.

## 14. Diagnostics

Every phase returns diagnostics instead of panicking on user source errors.
Diagnostics should carry spans whenever source text is involved.

The same rule applies below the compiler boundary. `std::debug::print` returns
a closed error that separates formatting from the concrete stderr flush cause;
maintained executables propagate it through the standard process conversion.
Diagnostic convenience output is not an unchecked or invariant trap site.

Implementation bugs may panic in tests, but normal invalid Nia programs should
flow through diagnostic reporting.

Backend IR is validated before LLVM emission. If lowering or stale query state
leaves invalid Function IR, unresolved array lengths, missing owner modules,
missing references, invalid static initializer paths, or missing ABI layouts in
runtime positions, LLVM codegen reports diagnostics at that boundary instead of
letting backend-specific lowering fail later.

Diagnostics describe current language rules. Unsupported syntax receives
ordinary current-rule diagnostics rather than a reserved compatibility path.

`nia-ice` is the explicit invariant-failure boundary. `catch_ice` converts
panic payloads into a structured internal diagnostic, while its thread-local
panic hook records file/line/column context without leaking panic state across
calls. User-source errors must continue through ordinary diagnostics; ICE
rendering is reserved for compiler bugs and remains actionable for reporting.

## 15. File And Module Granularity

Each source file is one module. Child files are loaded only through explicit
`module name;` or `pub module name;` declarations in the parent module. One
`-M name=path` entry is one pkg root.

Cross-module references should go through using aliases, public surfaces,
qualified paths, and stable `GlobalDefId`s. Phases should avoid storing direct
filesystem paths as semantic identity.

The representative build baseline exercises this boundary through `source-app`,
whose entry module imports the fixture's separately mapped `helper` module. Each
ordered build state is a separate `nia build` process. Incremental source and
module-map edits are compared byte-for-byte with independent cold-workspace
recomputations, and baseline acceptance records both distinct process identities
and the multi-module artifact comparison instead of inferring either property
from a passing build.

Module cycles are load-time errors. Loaded modules keep separate `ModuleId`s
and source paths, and references still go through explicit using aliases and
normal visibility checks. Recursive aliases, const dependencies, layouts,
generic expansion, or re-export chains remain concrete semantic errors for
their owning phases.

## 16. Evolution Rules

Nia is pre-1.0, so temporary historical forms are not compatibility
requirements. Once behavior is removed, tests and diagnostics should either
delete it or treat it as ordinary invalid syntax.

Cross-cutting compiler changes also follow the root-cause, ownership,
incremental, resource, and acceptance rules in
[compiler-maintenance.md](compiler-maintenance.md).

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

These principles guide maintenance:

- prefer explicit language rules over hidden runtime policy;
- keep host and bare output models separate;
- keep C ABI interop direct but not contagious into normal Nia symbols;
- keep compile-time value bindings separate from static storage;
- prefer small, inspectable tables over large mutable world objects;
- prefer readable symbols and IR over compact but opaque encodings;
- keep the language small enough that the compiler can remain understandable.
