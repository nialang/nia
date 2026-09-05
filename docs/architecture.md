# Nia Compiler Architecture

Status: implementation architecture reference

This document describes the compiler implementation architecture for Nia. For
language semantics, see [language-spec.md](language-spec.md). For ABI and layout
rules, see [nia-abi.md](nia-abi.md). For maintenance rules, see
[compiler-maintenance.md](compiler-maintenance.md).

## 1. Architecture Overview

The compiler is a typed query graph with explicit lowering boundaries. Each
crate owns one clear kind of data, accepts only the inputs it needs, and
produces immutable tables that dependent queries consume explicitly.

Primary architectural goals:

- keep syntax, name resolution, typing, layout, lowering, and codegen separate;
- centralize compilation identity and semantic storage without giving analysis
  crates unrestricted access to mutable global state;
- pass data through typed ids, immutable tables, and diagnostic lists;
- keep AST as syntax, not as the long-term semantic storage format;
- use `ModuleId`, `DefId`, `LocalId`, `TyId`, and `GlobalDefId` for identity;
- make every phase independently testable;
- use typed backend IR as the backend boundary;
- keep compile-time value checking distinct from static storage checking.

Forbidden architectural patterns:

- one pass that collects definitions, resolves names, infers types, and emits
  code;
- long-lived references between AST nodes;
- string concatenation as the only symbol identity;
- analysis phases mutating unrelated entries in a shared world table instead of
  going through typed store and query APIs;
- bypassing existing phases for temporary features by reinterpreting AST in the
  backend.

## 2. Compilation Pipeline

The whole-program query flow:

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
files, schedule whole-program work, or call later backends. The arrows represent
dependency directions, not execution order.

The pipeline is organized into four product layers. A later layer consumes the
products of the preceding layer; it does not reinterpret their source representation:

```text
+-----------------------+    requests    +-------------------------------+
| CLI and driver        | --------------> | Query session                 |
| nia-cli / nia-driver  |                | nia-compiler-query / nia-query|
+-----------+-----------+                +-------------------------------+
            |
            v
+-----------+-------------------------------------------------------------+
| Source and declaration products                                         |
| source -> tokens -> syntax/AST -> ItemTree -> Defs/Imports              |
|                                      -> TypeResolution -> Signatures     |
+-----------+-------------------------------------------------------------+
            | typed handles and declaration facts
            v
+-----------+-------------------------------------------------------------+
| Semantic products                                                        |
| TypeLowering/Normalization -> Value/LocalResolution                      |
| ConstCheck/Layout/ABI -> BodyCheck -> BodyFacts + BodyIr                 |
+-----------+-------------------------------------------------------------+
            | checked IR and executable facts
            v
+-----------+-------------------------------------------------------------+
| Native products                                                          |
| FunctionIr -> Reachability -> Monomorphization -> BackendIr -> LLVM/link |
+---------------------------------------------------------------------------+
```

The shared objects crossing these layers are handles and immutable products:
`TypeStore` supplies canonical `TyId` values, `nia-sema-ir` supplies semantic
facts, and `nia-query` records dependency edges. No layer receives a mutable
global compiler context.

For one declaration, the product flow is:

```text
AST StructItem
    | source shape and spans only
    v
DefCollection(Point -> DefId)
    v
ItemSignatures(Point fields -> TyId(i32))
    v
TypeLayout(Point -> size, alignment, field offsets)
    v
Backend aggregate metadata
    v
LLVM struct, global, or parameter representation
```

A definition can occur in several products, but each product owns a different
fact. A signature owns declared types, layout owns representation, and code
generation owns LLVM handles.

### 2.1 Optimization Policy

Optimization is configured separately from the phase graph. The CLI accepts
`-O0`, `-O1`, `-O2`, `-O3`, `-Os`, `-Oz`, and `-O` as `-O2`. These levels are
lowered into a Nia `OptimizationPolicy` before query execution. The policy is
threaded through compiler-query, backend lowering, and LLVM codegen.

Nia optimization levels are user-facing presets that expand to a policy matrix
with separate decisions for CFG simplification, constant folding, dead-code
elimination, local copy propagation, inlining, specialization, monomorphized
instance deduplication, and size preference. `nia-opt` owns this declarative
matrix.

Level meanings:

- `O0` performs only required canonicalization
- `O1` enables cheap, local, low-risk cleanup
- `O2` is normal optimized mode with full CFG cleanup, function-local data-flow
  analyses, ordinary DCE, normal inlining, and instance deduplication
- `O3` is performance-aggressive with more compile time spent on inlining,
  specialization, and cross-function reasoning
- `Os` is size-oriented with conservative inlining and specialization
- `Oz` is most size-constrained, avoiding specialization and inlining unless
  required or clearly size-reducing

This split is intentional: LLVM's optimization enum controls only LLVM codegen
choices. Nia must make earlier size and performance decisions in monomorphization,
backend lowering, specialization, and inlining before LLVM sees the program.

The policy is data passed to optimization owners, not a second execution graph:

```text
-O2
 │
 ▼
OptimizationPolicy
 ├─ simplify_cfg        Full
 ├─ const_fold          Full
 ├─ dead_code_elim      Full
 ├─ local_copy_prop     Full
 ├─ inline_threshold    Normal
 ├─ specialize_generics Normal
 ├─ deduplicate         true
 └─ prefer_size         false
       │
       ├── nia-function-opt: function-local passes
       ├── nia-backend-lower: module-level passes
       └── nia-codegen-llvm: LLVM codegen mapping
```

The policy changes optional transformations. It does not change type layout,
calling convention, symbol identity, or static-data meaning.

### 2.2 Query Execution And Invalidation

A query is identified by a typed key and registered with a storage policy. The
normal policy retains an immutable `Arc` in the query slot; a single-consumer
policy transfers ownership to its consumer. Both policies remain in the same
session-owned dependency graph.

```text
get(K)
  │
  ├─ cache hit ───────────────► return immutable product
  │
  └─ cache miss
       │ record parent -> K
       ▼
   execute provider
       │
       ├─ request dependencies recursively
       ├─ publish product in K's slot
       └─ record dependency edges and fingerprint

source revision changes
       │
       ▼
retirement barrier
  ├─ stop new query admission
  ├─ wait for active work
  ├─ remove retired keys, slots, and graph edges
  └─ allow new queries
```

`nia-query` owns this lifecycle. Compiler crates own query keys and products;
they do not implement cache invalidation independently. A persistent frontend
cache is an additional storage layer and does not replace in-memory dependency
tracking.

### 2.3 Crate Dependency Layers

The dependency direction is downward. A lower layer may provide data to a higher
layer, but must not import a higher-level semantic or backend crate.

```text
+--------------------------------------------------------------+
| Product surface: nia-cli, nia-driver, nia-build              |
+--------------------------------------------------------------+
| Native backend: nia-linker, nia-codegen-llvm, nia-llvm       |
+--------------------------------------------------------------+
| Backend model: nia-backend-lower, nia-backend-ir              |
+--------------------------------------------------------------+
| Executable and function IR: reachability, monomorphize,       |
| nia-function-lower, nia-function-opt, nia-function-ir         |
+--------------------------------------------------------------+
| Semantic analysis: body, flow, closure, const, static,       |
| layout, ABI, trait solving, normalization                     |
+--------------------------------------------------------------+
| Declaration and resolution: defs, imports, signatures,        |
| type/value/local resolution, public surface                   |
+--------------------------------------------------------------+
| Syntax: item tree, parser, AST, syntax, lexer, literals       |
+--------------------------------------------------------------+
| Shared foundation: query, loader, types, ids, source, spans,   |
| symbols, diagnostics, target and compatibility                 |
+--------------------------------------------------------------+
```

`nia-driver` and `nia-compiler-query` assemble products across the layers. They
are orchestration boundaries, not an exception to dependency direction.

## 3. Foundation Crates

### `nia-query`

Query execution kernel. Provides lazy evaluation, dependency tracking, caching,
invalidation, parallelism, and cycle detection. Query products are stored as
`Arc<T>` or moved through owned queries. Tracks dependencies automatically for
incremental compilation.

Parallelism uses jobserver for CPU budget and heavyweight permits for LLVM (1.5GB
each, max 4 concurrent). Cycle detection uses a wait-for graph that catches
cross-thread deadlocks before blocking.

### `nia-loader-query`

Owns source file loading, module discovery, and module-to-source mapping. Provides
loader facade, source manifests, and module activation. Resolves logical module
paths to physical source files. Builds the compiler source-input manifest with
content fingerprints.

### `nia-compiler-query`

Session-owned compiler query facade. Wraps `nia-query` with compiler-specific
query types and manages compilation session lifecycle.

### `nia-node-id`

Source-versioned syntax node identity for semantic side tables and diagnostics.
`VersionedNodeKey` combines source id, revision, syntax kind, and position.
Session-local `NodeId` is an eight-byte handle allocated monotonically.

The canonical `NodeStore` owns only active source-revision shards. Retiring a
source revision removes its shard from the current store.

### `nia-ids`

Typed cross-phase ids: `ModuleId`, `DefId`, `LocalId`, `InternedTyId`,
`GlobalDefId`. Stores no semantic tables and has no filesystem, parser, or
diagnostic responsibility. Type handles expose their session owner but do not
contain semantic interpretation.

### `nia-symbol`

Stable symbol boundary used by parser and semantic products. `SymbolId` is an
append-only hash identity. The `known` registry is the canonical mapping for
language and builtin names.

### `nia-ty`

Compiler type model and session-wide canonical `TypeStore`. All compiler passes
read unified `TyId` handles from the store and publish new types through
`TypeStoreAppend`. The store is append-only within a session, with handles
validated by store identity.

Type canonicalization maps `TyKind` to a global slot so the same primitive or
structural kind published from different modules has the same ID. The arena is
a sparse four-level `OnceLock` trie providing lock-free reads after initialization.

Callable interfaces have additional canonicalization: `TyKind::CallablePointee`
is the unsized signature-bearing pointee, while `TyKind::Callable` is its sized
dynamic view. Publishing an ordinary pointer to a callable pointee canonicalizes
to the corresponding callable view.

### `nia-diagnostic`

Diagnostics and source rendering. Owns user-facing diagnostic display but not
semantic policy. Diagnostic codes are registry-backed schema values with severity,
category, and stage reconstructed from registered definitions during stable-bundle
decode.

### `nia-timing`

Process-wide timing collection and optional Rust heap instrumentation. Timing
collectors serialize report ownership across threads. Query accumulators aggregate
by stable names before emitting bounded reports.


## 4. Syntax Crates

### `nia-lexer`

Turns source text into tokens with spans. Handles comments, identifiers, numbers,
strings, multiline strings, character literals, punctuation, and lexer errors. Does
not resolve types, evaluate constants, or classify identifiers beyond keyword
recognition. Numeric separator placement is lexical grammar: every `_` must sit
between two digits valid for that literal's radix.

### `nia-syntax`

Defines the lossless syntax representation. Builds green nodes and red syntax
nodes/tokens, preserves trivia and full source text, groups delimiter subtrees, and
exposes conservative partial reparsing for token/trivia edits. Red syntax tokens
carry source-versioned child paths used by diagnostics and AST lowering.

### `nia-ast`

Defines the parsed syntax tree. AST nodes represent source structure and spans but
do not store type ids, def ids, layout information, or backend values. AST expressions,
statements, patterns, items, and type references retain only syntax payloads plus
span/node-key identity.

### `nia-parser`

Builds AST from red tokens and reports parse errors. Owns grammar decisions, local
parse recovery, and syntax-to-AST lowering. Records `NodeOriginTable` mappings from
AST spans to red/green child-path ranges. Parser checkpoints roll back token position
and origin-table mutations together.

Expression bracket suffixes are parsed in syntax-preserving form; semantic
disambiguation of generic instantiation vs indexing happens later.

### `nia-ast-walk`

Provides AST traversal helpers. Stays small and generic, with no embedded semantic
policy. Documented `Visitor` callbacks and `walk_*` entry points define structural
preorder ownership only.

### `nia-item-tree`

Defines the source item tree used as the first semantic-facing representation of
module contents. Keeps AST syntax out of long-lived semantic tables while preserving
item boundaries, attributes, visibility, conditional attributes, and source spans.
Does not evaluate conditional attributes and does not resolve names or types.

The loader records both the raw module item tree and the active item tree for the
current target.

## 5. Definitions And Modules

### `nia-defs`

Collects active item-tree definitions into module-local definition tables. Assigns
`DefId`s and tracks namespaces for values, types, modules, enum variants, and methods.
Detects duplicate names in the same namespace and duplicate generic parameters. Does
not evaluate const conditions, type-check bodies, or load other files.

`DefId` is derived from canonical structural declaration identity rather than
collection order or source formatting. Top-level namespace, member ancestry, extension
target/trait/generic/where syntax, and duplicate ordinal participate in that identity.

Public-surface persistence uses `PublicSurfaceModuleFacts`, a deterministic reduced
projection containing only declaration, namespace, enum-variant, and module-using facts.

### `nia-imports`

Builds the explicit module graph and normalizes using paths. Handles package roots,
entry-root paths, current-package paths, child declarations, parent paths, module cycle
diagnostics, and duplicate local using aliases. Does not perform semantic checking of
selected items.

### `nia-driver`

Loads source files, builds the using graph, computes public surfaces, and schedules
whole-program checking and codegen by requesting query products. Owns orchestration
across modules, not semantic interpretation. The using graph is acyclic; semantic
cycles inside loaded modules are diagnosed by the query or crate that owns the
affected construct.

## 6. Type Frontend

### `nia-type-resolve`

Resolves type names in active item-tree type syntax to definition identities or
primitive types. Validates type paths and generic names but does not lower them into
canonical type ids.

### `nia-type-lower`

Lowers active item-tree type references into interned type ids. Handles primitive
types, pointers, arrays, slices, thin function pointer types, unsized callable
interfaces and their sized views, nominal types, generics, enum backing types, and
inferred array lengths.

The lowerer reads existing handles from the canonical `TypeStore` and publishes
primitive, nominal, projection, and structural types through a module-scoped
`TypeStoreAppend`. It never opens a mutable interner transaction.

### `nia-item-signatures`

Collects signatures for functions, methods, globals, structs, enums, aliases, and
extension methods after type lowering. Function signatures include whether a function
is `extern`, variadic, generic, and whether it has a body.

`ItemSignatures` is the declaration surface consumed by type resolution, trait solving,
layout, const checking, and backend planning.

### `nia-program-signatures`

Qualifies the declaration-only products from `nia-item-signatures` with `GlobalDefId`
identities and indexes program-level trait implementations. Its lookup/context APIs
borrow or resolve existing signatures; they do not reparse source or reconstruct body
semantics.

Visibility-aware extension discovery computes a deterministic closure from using scopes,
public surfaces, canonical type normalization, and nominal extension providers.

### `nia-type-normalize`

Expands type aliases and canonicalizes type forms where required. Detects recursive
aliases and keeps normalized type information separate from raw lowered types.
`TypeNormalization` contains only normalized-ID facts and diagnostics; it never owns
a type view.

### `nia-trait-solve`

Resolves builtin and user trait goals, associated types, and associated consts from
canonical type handles and explicit program-signature facts. Solver construction does
not borrow a mutable module interner: all reads use the session `TypeStore`, and
synthesized goal or projection types are published through a module-scoped
`TypeStoreAppend`.

Trait and associated-type recursion use path-local semantic guards.


## 7. Name Resolution

### `nia-value-resolve`

Resolves value paths and qualified value names that refer to top-level values,
functions, enum variants, and imports. Defers local variables to `nia-local-resolve`.

### `nia-local-resolve`

Builds local scopes for functions and blocks. Resolves parameters, local bindings,
block-local `using`, deferred expressions, and local identifiers. Marks expressions
that syntactically act as type prefixes for associated function calls or enum
variant paths.

### `nia-sema-ir`

Owns the persistent semantic fact schema shared by name resolution, body checking,
const lowering, reachability, and backend planning. Facts are keyed by global
definitions, locals, or `VersionedNodeKey`. Module-level expressions and function
bodies are separate ownership domains.

## 8. Const Values, Static Data, Layout, And ABI

### `nia-const-ir`

Defines the source-preserving semantic body used for compile-time execution. Stores
only the expression, statement, block, function, parameter, binding, and field forms
that are valid inputs to compile-time evaluation, while preserving semantic ids and
source spans needed for name, local, type-argument, and diagnostic queries.

AST is lowered into `ConstModule` before execution. A `ConstModule` contains the
module's const enums, global and local const initializers, `const fn` bodies, and
type-level constant expressions.

### `nia-const-eval`

Evaluates the pure expression subset used by current compile-time values. Consumes
`nia-const-ir` rather than AST. Is an evaluator, not a language semantic pass: does
not load modules, know visibility, own storage rules, or make backend decisions.

`const fn` is const-capable rather than const-eval-only. Const evaluation interprets
its const semantic body when the call occurs in a constant expression. The ordinary
body-check, reachability, backend-lowering, and code-generation pipeline retains the
same function when it is reachable from runtime code.

### `nia-const-check`

Performs type checking and semantic validation for compile-time expressions. Produces
typed const facts consumed by body checking, layout, and backend planning. Delegates
execution to `nia-const-eval` after semantic validation.

### `nia-pattern-analysis`

Owns the pure pattern-matrix algorithm shared by runtime body checking and static
const-match typing. Accepts only canonical type-column identities, constructor
identities, constructor field types, scalar bounds, and normalized patterns. Has no
dependency on AST, name resolution, type storage, diagnostics, or lowering.

The algorithm follows the specialization/default structure of Maranget-style usefulness
analysis. Finite constructors model enums, optional/error unions, tuples, pointers,
and nominal structs. Scalar endpoints partition finite integer domains into disjoint
intervals without enumerating the domain.

### `nia-static-check`

Validates static initializers for `static` storage. Distinguishes static data from
compile-time value bindings. Address initializers are allowed only when they can be
represented as target static relocations.

### `nia-static-ir`

Defines the static/global initialization IR. Represents compile-time data, not
executable runtime control flow. Supports zero values, scalars, strings/bytes, arrays,
repeats, structs, null pointers, global addresses, and function addresses.

### `nia-layout`

Computes ABI-relevant layout for primitive, pointer, array, struct, enum, and
instantiated nominal types. Every compiler layout provider derives its
`TargetDataLayout` from the artifact `CompilerTargetQuery`, so layouts share the same
pointer size and alignment.

Callable pointees are unsized and have no `TypeLayout`. A callable view is `Sized`,
with size equal to two target pointer words and target pointer alignment.

The algorithm reads every existing handle from the session `TypeStore` and publishes
structural types created by generic substitution through a module-scoped
`TypeStoreAppend`.

### `nia-abi-check`

Checks C ABI boundaries for `extern` functions, globals, and structs. Rejects Nia-only
types that cannot be passed directly through the C ABI, such as slices, arrays by
value, closed enums where not supported, `bool`, `char`, and variadic function pointers.
Also rejects unsupported extern forms, including variadic extern definitions with bodies.

## 9. Control Flow And Body Checking

### `nia-flow-check`

Checks flow-sensitive structural rules: missing returns, unreachable statements,
`break` and `continue` outside loops, ordinary control-flow validity inside deferred
expressions, and match duplicate arms.

### `nia-body-check`

Type-checks function bodies, methods, globals, and static initializers. Produces
`BodyFacts` (the semantic surface with expression types, call targets, coercions,
generic instantiations) and `BodyIr` (the runtime checked body product with typed
function bodies and static initializers).

All body-check entry points read existing handles from the compilation `TypeStore` and
publish inferred or substituted structural types through a module-scoped
`TypeStoreAppend`. They never borrow a mutable interner.

Later phases consume these products explicitly instead of reading ad hoc body-check
side tables or rediscovering expression semantics from AST shape.

### `nia-body-ir`

Defines checked body data products: `BodyFacts` (semantic surface with resolved, typed
body facts) and `BodyIr` (runtime checked body with typed function bodies and static
initializers). This crate is source-shaped: blocks, if expressions, match expressions,
and for headers still reflect the checked language form.

### `nia-function-ir`

Defines the lowered function body IR used by backend codegen: function-level blocks,
scopes, operations, terminators, places, callees, locals, builtin values, inline
assembly, and runtime expressions.

Function IR is the current function backend boundary. It removes source-shaped control
expressions from runtime expression trees: block, if, match, for, return, break,
continue, and defer behavior is represented through blocks, terminators, scope edges,
and defer bodies.

### `nia-function-lower`

Lowers checked body IR (`nia-body-ir`) into function IR (`nia-function-ir`). Transforms
source-shaped control flow into explicit blocks and terminators. Each lowered body is
owned directly by `LoweredFunctionBodyQuery(GlobalDefId)`.

### `nia-function-opt`

Applies function-local optimization passes to Function IR: dead-code elimination,
constant folding, CFG simplification, and local copy propagation. Pass selection comes
from the `OptimizationPolicy`, not directly from user-facing levels.

### `nia-closure-check`

Validates closure capture semantics and escape analysis. Ensures captured values have
appropriate lifetimes and that callable views do not escape their defining scope
incorrectly.

## 10. Monomorphization And Symbols

### `nia-executable-facts`

Extracts semantic facts from typed body IR for executable closure: function calls,
trait method calls, trait object coercions, generic instantiations, and static/global
references. Produces structured facts consumed by reachability and backend planning.

### `nia-executable-reachability`

Computes the transitive closure of reachable functions, types, traits, and globals
from entry points. Starts from the program entry function and follows calls, trait
dispatch, generic instantiations, and static references. The result is the minimal
set of items needed for the executable.

### `nia-monomorphize`

Collects concrete generic instances before backend lowering. Expands generic functions
and types to their concrete instantiations based on reachable call sites. Pre-indexes
instantiations by source definition and caches mangled type symbols during collection.

Nested type arguments discovered while expanding generic bodies are published through
the canonical `TypeStore` and memoized by a substitution-id cache, so recursive shapes
are instantiated once for a given substitution map.

### `nia-mangle`

Produces stable symbol names for functions, methods, globals, and generic instances.
Symbol format is deterministic and includes module id, definition id, source name, and
for generic instances, encoded type and const arguments. Mangling is traceable for
debugging and linking.

## 11. Backend IR

### `nia-backend-ir`

Defines the backend intermediate representation: the typed IR consumed by LLVM codegen.
Includes backend functions, globals, vtables, aggregate instances, and their metadata.
Backend IR is the stable boundary between Nia semantics and LLVM emission.

### `nia-backend-lower`

Lowers Function IR into Backend IR. Applies module-level optimization passes (leaf
inlining, cross-function constant propagation, direct trait-call devirtualization)
based on the `OptimizationPolicy`. Records an optimization report showing which passes
ran and which functions/globals were transformed.

The policy controls pass depth and budgets rather than enabling all-or-nothing. Backend
lowering reads existing handles from the canonical `TypeStore` and publishes synthesized
instance types through a module-scoped `TypeStoreAppend`.

## 12. LLVM Backend

### `nia-llvm`

Provides the LLVM wrapper layer: typed LLVM values, types, modules, contexts, builders,
and target configuration. Wraps the LLVM C API with Rust types and validates construction
results before exposing typed handles. All wrapper construction is fallible; null LLVM
results become `LlvmResult` errors that codegen propagates.

### `nia-codegen-llvm`

Emits LLVM IR, objects, and native codegen units from backend IR. Owns LLVM type
construction, function declarations and definitions, globals and static initializers,
instruction emission, control flow lowering, defer lowering, aggregate operations, and
inline assembly emission.

LLVM physical type lowering represents a callable view as the literal two-field aggregate
`{ ptr, ptr }`. Backend lowering caches generic type instantiations while expanding
function instances so repeated uses of the same type under the same substitutions do not
rebuild the same interned type graph.

Module-codegen uses whole-program indexes for layout queries and signature type building.
One `Arc<ProgramIndex>` is built before validation and shared by all unit tasks.

### `nia-linker`

Invokes the system linker to produce executables. Handles linker selection (lld, system
ld), link arguments, library paths, and startup objects. The default Linux x86_64
runtime exports `_start` and calls the Nia-level root entry contract from standard-library
code.

## 13. CLI

### `nia-cli`

The `nia` command-line compiler frontend. Owns CLI parsing, toolchain resolution, command
dispatch, and ICE boundaries. Core pipeline commands:

- `nia build [step|dir] [--root dir]` - discovers and runs package build.nia
- `nia check <file.nia>` - validates without codegen
- `nia emit --tokens|--ast|--checked|--backend|--llvm|--obj|--exe <file.nia>` - emits intermediate or final products

Accepts global options: `-O0` through `-Oz` for optimization, `-M name=path` for module
aliases, `--timings` for performance analysis, `--opt-report` for optimization report.

## 14. Architectural Data Flows

### 14.1 Type Identity And Storage

`InternedTyId` contains a `TypeStoreId` and a slot index. The handle identifies a
session-local entry; it does not contain the type's semantic meaning. `TypeStore`
owns the meaning, canonicalization map, and immutable kind arena. Consumers read
through the store, while producers receive the narrower `TypeStoreAppend` capability.

```text
                         +----------------------+
                         | TypeStore             |
                         | canonical TyKind map |
                         | immutable kind arena  |
                         +----------+-----------+
                                    ^
                         read       | append synthesized types
                                    |
+-------------+       +------------+-------------+       +----------------+
| signatures  | ----> | TypeStoreAppend         | <---- | body checking  |
| normalization|      | module-scoped capability |       | layout/backend |
+-------------+       +--------------------------+       +----------------+
        |
        | publishes
        v
InternedTyId { store_id, index }
        |
        +--> TypeStore::get(id) -> TyKind
        +--> layout / trait solving / lowering
```

The store boundary prevents two failure modes. A foreign-session handle is
rejected before a phase interprets it, and a phase cannot mutate unrelated
semantic entries while reading a type. Structural equivalence is applied to
`TyKind` payloads; raw handle equality is reserved for exact identities and
fast paths.

The type model distinguishes representation-bearing and unsized forms:

```text
TyKind
├── scalar: Primitive, Vector
├── aggregates: Tuple, Array, Nominal
├── memory views: Pointer, VolatilePointer, Slice, SlicePointee
├── callable: FunctionPointer, CallablePointee, Callable, ClosureState
├── language wrappers: Optional, ErrorUnion, Range
└── semantic forms: Projection, BuiltinType, BuiltinTrait, Error, ConstOnly

CallablePointee --unsized signature--> Callable --two pointer words--> runtime view
```

`CallablePointee` is a signature-bearing unsized type. `Callable` is the sized
read-only or writable view. The canonicalization path converts a pointer to a
callable pointee into the corresponding callable view instead of allowing an
invalid one-word representation to escape into later phases.

### 14.2 Semantic Products And Runtime IR

Body checking consumes declarations, types, names, trait results, and const facts.
It publishes two products with different consumers:

```text
AST body + semantic inputs
             |
             v
      +----------------+
      | nia-body-check |
      +--------+-------+
               |
       +-------+--------+
       |                |
       v                v
+--------------+  +--------------+
| BodyFacts    |  | BodyIr       |
| expression   |  | typed body   |
| types/calls  |  | static init  |
| coercions    |  | source shape |
+------+-------+  +------+-------+
       |                 |
       | reachability    | lowering
       v                 v
+--------------+  +--------------+
| executable   |  | FunctionIr   |
| facts        |  | blocks, ops, |
+--------------+  | terminators  |
       |           +------+-------+
       |                  |
       +--------+---------+
                v
        backend planning/lowering
```

`BodyFacts` records semantic facts that are useful without reconstructing the
source body. `BodyIr` retains checked source-shaped expressions and static
initializers. `FunctionIr` is a separate runtime representation: control flow,
`defer`, places, calls, and scopes are explicit in blocks and terminators.

For example, a checked call is represented by its resolved callee identity rather
than by the original path text:

```text
source:  value.method[T, N](arg)
             |
             v
BodyFacts: receiver type, method definition, T, N, coercions
             |
             v
FunctionIr: FunctionCallee::Method {
              def_id, arg_module_id, self_arg, args, const_args,
              receiver_kind, receiver, ...
            }
```

This is why function lowering and backend lowering do not need to resolve names
again. The callee carries the complete identity required for instance planning;
trait arguments, method arguments, const arguments, and receiver substitutions
remain separate fields.

### 14.3 Executable Closure And Backend Partitioning

Executable emission does not lower every checked body. It first computes a fixed
point from entry points and semantic references:

```text
root::main
    |
    v
BodyFacts / FunctionIr references
    |
    +--> direct functions and globals
    +--> generic function and method instances
    +--> trait implementations and vtables
    +--> static addresses and type-only module owners
    |
    v
ExecutableReachability
    ├── functions: HashSet<GlobalDefId>
    ├── globals: HashSet<GlobalDefId>
    ├── modules: runtime body owners
    └── type_modules: layout/signature-only owners
    |
    v
Monomorphization
    └── MonoInstance { def_id, self_arg, args, const_args, symbol }
    |
    v
BackendLowering
    ├── BackendProgram: ordered BackendModule values
    ├── owner directory: definition and generated-item ownership
    ├── partition plan: independent codegen units
    └── optimization report
    |
    v
LLVM module tasks -> object files -> linker -> executable
```

`modules` and `type_modules` are intentionally distinct. A module may be required
for a type, trait, or layout fact without contributing a runtime body. Keeping this
distinction prevents compile-time or type-only dependencies from becoming runtime
entry points.

Backend lowering first creates program-wide ownership and then finalizes module
products. Codegen partitions are derived from ordered backend modules, so LLVM tasks
can run independently while sharing the same program-level indexes and canonical
type store.

### 14.4 Query Products And Cache Ownership

Query keys, products, and storage policies form one contract:

```text
+------------------+       +----------------------+       +-------------------+
| typed query key  | ----> | provider execution   | ----> | query slot        |
+------------------+       +----------+-----------+       +---------+---------+
                                      |                             |
                                      | records dependencies       |
                                      v                             v
                              QueryDependencyGraph          product ownership
                                                                  |
                                      +---------------------------+----------------+
                                      |                                            |
                                      v                                            v
                             CacheOwnedArc                                 SingleConsumerOwned
                         shared immutable product                         one consumer moves value
```

A cache-owned product may be shared by multiple consumers and fingerprinted when
its query contract permits it. A single-consumer product moves out of its slot and
therefore does not retain a reusable value fingerprint. Source replacement enters
a retirement barrier, drains active work, removes obsolete keys and graph edges,
and then admits new queries.

The cache is not a semantic owner. A cached signature, body, or frontend product
is valid only after its key, schema, compiler identity, target, and stable source
inputs have been checked. Semantic interpretation remains in the owning crate.
