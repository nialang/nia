# Nia Compiler Architecture

This document describes how the compiler is put together: the pipeline, the
query model, crate layering, and the data flows that cross crate boundaries.
Each crate's own responsibilities and internal rules are documented in its
crate-level rustdoc (`crates/<name>/src/lib.rs`); the [crate index](#5-crate-index)
below gives a one-line summary of each.

Related documents:

- [Language reference](../reference/language.md) for language semantics.
- [ABI reference](../reference/abi.md) and [ABI internals](abi-internals.md)
  for representation and symbol rules.
- [Build system internals](../../crates/nia-build/README.md) for `nia build`.
- [Contributing](contributing.md) for engineering rules and review policy.

## 1. Overview

The compiler is a typed query graph with explicit lowering boundaries. Each
crate owns one kind of data, accepts only the inputs it needs, and produces
immutable tables that dependent queries consume explicitly.

Architectural goals:

- keep syntax, name resolution, typing, layout, lowering, and codegen separate;
- centralize compilation identity and semantic storage without giving analysis
  crates unrestricted access to mutable global state;
- pass data through typed ids, immutable tables, and diagnostic lists;
- keep AST as syntax, not as the long-term semantic storage format;
- use `ModuleId`, `DefId`, `LocalId`, `TyId`, and `GlobalDefId` for identity;
- make every phase independently testable;
- use typed backend IR as the backend boundary;
- keep compile-time value checking distinct from static storage checking.

Forbidden patterns:

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
  -> syntax grammar / green-red tree
  -> AST lowering
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
files, schedule whole-program work, or call later backends. The arrows are
dependency directions, not execution order.

The pipeline is organized into product layers. A later layer consumes the
products of the preceding layer; it does not reinterpret their source
representation:

```text
+-----------------------+    requests     +--------------------------------+
| CLI and driver        | --------------> | Query session                  |
| nia-cli / nia-driver  |                 | nia-compiler-query / nia-query |
+-----------+-----------+                 +--------------------------------+
            |
            v
+-----------+--------------------------------------------------------------+
| Source and declaration products                                          |
| source -> tokens -> syntax/AST -> ItemTree -> Defs/Imports               |
|                                      -> TypeResolution -> Signatures     |
+-----------+--------------------------------------------------------------+
            | typed handles and declaration facts
            v
+-----------+--------------------------------------------------------------+
| Semantic products                                                        |
| TypeLowering/Normalization -> Value/LocalResolution                      |
| ConstCheck/Layout/ABI -> BodyCheck -> SemanticFacts + BodyIr             |
+-----------+--------------------------------------------------------------+
            | checked IR and executable facts
            v
+-----------+--------------------------------------------------------------+
| Native products                                                          |
| FunctionIr -> Reachability -> Monomorphization -> BackendIr -> LLVM/link |
+--------------------------------------------------------------------------+
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
lowered into an `OptimizationPolicy` before query execution, and the policy is
threaded through compiler-query, backend lowering, and LLVM codegen.

Optimization levels are user-facing presets that expand to a policy matrix with
separate decisions for CFG simplification, constant folding, dead-code
elimination, local copy propagation, inlining, specialization, monomorphized
instance deduplication, and size preference. `nia-opt` owns this declarative
matrix.

| Level | Meaning |
| --- | --- |
| `O0` | Only required canonicalization. |
| `O1` | Cheap, local, low-risk cleanup. |
| `O2` | Normal optimized mode: full CFG cleanup, function-local data-flow analyses, ordinary DCE, normal inlining, instance deduplication. |
| `O3` | Performance-aggressive: more compile time spent on inlining, specialization, and cross-function reasoning. |
| `Os` | Size-oriented, with conservative inlining and specialization. |
| `Oz` | Most size-constrained; avoids specialization and inlining unless required or clearly size-reducing. |

LLVM's optimization enum controls only LLVM codegen choices. Size and
performance decisions in monomorphization, backend lowering, specialization,
and inlining are made by Nia before LLVM sees the program.

The policy is data passed to optimization owners, not a second execution graph:

```text
-O2
 │
 ▼
OptimizationPolicy
 ├─ simplify_cfg                   Full
 ├─ const_fold                     Full
 ├─ dead_code_elim                 Full
 ├─ local_copy_prop                Full
 ├─ inline_threshold               Normal
 ├─ specialize_generics            Normal
 ├─ dedup_monomorphized_instances  true
 └─ prefer_size                    false
       │
       ├── nia-function-opt: function-local passes
       ├── nia-backend-lower: module-level passes
       └── nia-codegen-llvm: LLVM codegen mapping
```

The policy changes optional transformations. It does not change type layout,
calling convention, symbol identity, or static-data meaning.

### 2.2 Query Execution And Invalidation

A query is identified by a typed key and registered with a storage policy. The
cache-owned policy retains an immutable `Arc` in the query slot; the
single-consumer policy transfers ownership to its consumer. Both policies live
in the same session-owned dependency graph.

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
they do not implement cache invalidation independently. A query asking for
itself is caught on the requesting thread, and a process-wide wait-for graph
reports cross-thread cycles before a thread blocks.

Parallel work draws CPU tokens from a jobserver, inherited from the launching
process when one is present. LLVM work additionally holds a process-wide memory
permit. At most four LLVM lanes run at once; the cap is further limited by the
available CPUs and by half the observable memory limit (system or cgroup)
divided by a 512 MiB per-lane cost. When no memory limit is observable, one
lane is used. Another lane is admitted only while at least 1 GiB (the 512 MiB
lane cost plus 512 MiB headroom) is observed as available.

### 2.3 Crate Dependency Layers

The dependency direction is downward. A lower layer may provide data to a
higher layer but must not import a higher-level semantic or backend crate.
Crates in the same layer may depend on each other.

```text
+--------------------------------------------------------------+
| Product surface: nia-cli, nia-build, nia-build-plan          |
+--------------------------------------------------------------+
| Orchestration: nia-driver, nia-compiler-query,               |
| nia-loader-query, nia-loader-contract                        |
+--------------------------------------------------------------+
| Native backend: nia-linker, nia-codegen-llvm, nia-llvm       |
+--------------------------------------------------------------+
| Backend model: nia-backend-lower, nia-backend-ir             |
+--------------------------------------------------------------+
| Executable planning: reachability, monomorphization,         |
| function lowering and optimization                           |
+--------------------------------------------------------------+
| Semantic analysis: layout, ABI, trait solving, program       |
| signatures, const, static, flow, body, closure checking      |
+--------------------------------------------------------------+
| Declaration and resolution: target selection, defs, imports, |
| public surface, type/value/local resolution, signatures      |
+--------------------------------------------------------------+
| IR data models: semantic facts, const/body/function/static   |
| IR, IR names, symbol mangling                                |
+--------------------------------------------------------------+
| Syntax: lexer, syntax, AST, parser, literals, item tree      |
+--------------------------------------------------------------+
| Shared foundation: query, types, ids, source, spans, symbols,|
| diagnostics, target, compatibility, toolchain, package ids   |
+--------------------------------------------------------------+
```

The IR data-model crates sit low because several analysis crates construct or
read them: body checking produces Body IR that embeds static IR, and static IR
reuses function-IR operands. Orchestration crates assemble products across all
lower layers; they are the only crates that see the whole pipeline.

## 3. Cross-Cutting Rules

These rules apply in every crate that touches the named resource. Crate docs do
not restate them.

Type store access. Every phase reads existing type handles from the
session-wide canonical `TypeStore` and publishes new primitive, nominal,
projection, substituted, or synthesized types through a module-scoped
`TypeStoreAppend` capability. No phase opens a mutable interner transaction or
borrows a module-local interner mutably.

Protocol epoch. Persisted compiler data (frontend and signature cache records,
stable metadata, backend objects, build-plan encodings) is identified by format
magic plus the release-derived protocol epoch owned by `nia-compat`. Payloads do
not carry independent hand-bumped schema or release versions.

Persistent caches. Every persistent cache (loader frontend products, compiler
signature products, driver object/LTO, archive, and linked-executable caches,
and ThinLTO backend objects) holds disposable, identity-checked records. A
record is valid only after its key, schema, compiler identity, target, and
stable source inputs have been checked; the cache is never a semantic owner.
Before publication or reuse, every namespace directory is validated component
by component beneath the invocation cache root, and records and mutation locks
are opened without following symbolic links or reparse points. A cache miss or
invalid record falls back to the in-memory query graph.

Diagnostics. Providers collect diagnostics in phase order and suppress a
derived report only when a published root has a matching cause identity or
applicable source-span evidence. Independent errors stay visible.

Internal errors. Broken compiler invariants are reported as structured internal
compiler errors (`nia-ice`) that propagate to the CLI boundary instead of
unwinding through it.

## 4. Architectural Data Flows

### 4.1 Type Identity And Storage

`InternedTyId` contains a `TypeStoreId` and a slot index. The handle identifies
a session-local entry; it does not contain the type's semantic meaning.
`TypeStore` owns the meaning, canonicalization map, and immutable kind arena.
Consumers read through the store, while producers receive the narrower
`TypeStoreAppend` capability.

```text
                         +----------------------+
                         | TypeStore            |
                         | canonical TyKind map |
                         | immutable kind arena |
                         +----------+-----------+
                                    ^
                         read       | append synthesized types
                                    |
+--------------+       +------------+-------------+       +----------------+
| signatures   | ----> | TypeStoreAppend          | <---- | body checking  |
| normalization|       | module-scoped capability |       | layout/backend |
+--------------+       +--------------------------+       +----------------+
        |
        | publishes
        v
InternedTyId { store_id, index }
        |
        ├─> TypeStore::get(id) -> TyKind
        └─> layout / trait solving / lowering
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
read-only or writable view. Canonicalization converts a pointer to a callable
pointee into the corresponding callable view, so an invalid one-word
representation cannot escape into later phases.

### 4.2 Semantic Products And Runtime IR

Body checking consumes declarations, types, names, trait results, and const
facts. It publishes two products with different consumers:

```text
    AST body + semantic inputs
                |
                v
        +---------------+
        |nia-body-check |
        +-------+-------+
                |
       +--------+--------+
       |                 |
       v                 v
+--------------+  +--------------+
| SemanticFacts|  | BodyIr       |
| expression   |  | typed body   |
| types/calls  |  | static init  |
| coercions    |  | source shape |
+------+-------+  +------+-------+
       |                 |
       | reachability    | lowering
       v                 v
+--------------+   +--------------+
| executable   |   | FunctionIr   |
| facts        |   | blocks, ops, |
+--------------+   | terminators  |
       |           +------+-------+
       |                  |
       +--------+---------+
                v
    backend planning/lowering
```

`SemanticFacts` records semantic facts that are useful without reconstructing the
source body. `BodyIr` retains checked source-shaped expressions and static
initializers. `FunctionIr` is a separate runtime representation: control flow,
`defer`, places, calls, and scopes are explicit in blocks and terminators.

A checked call is represented by its resolved callee identity rather than by
the original path text:

```text
source:  value.method[T, N](arg)
             |
             v
SemanticFacts: receiver type, method definition, T, N, coercions
             |
             v
FunctionIr: FunctionCallee::Method {
              def_id, arg_module_id, self_arg, args, const_args,
              receiver_kind, receiver, ...
            }
```

Function lowering and backend lowering therefore never resolve names again. The
callee carries the complete identity required for instance planning; trait
arguments, method arguments, const arguments, and receiver substitutions remain
separate fields.

### 4.3 Executable Closure And Backend Partitioning

Executable emission does not lower every checked body. It first computes a
fixed point from entry points and semantic references:

```text
entry::main
    |
    v
SemanticFacts / FunctionIr references
    |
    ├──> direct functions and globals
    ├──> generic function and method instances
    ├──> trait implementations and vtables
    ├──> static addresses and type-only module owners
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

`modules` and `type_modules` are distinct. A module may be required for a
type, trait, or layout fact without contributing a runtime body; keeping the
two apart prevents compile-time or type-only dependencies from becoming
runtime entry points.

Backend lowering first creates program-wide ownership and then finalizes module
products. Codegen partitions are derived from ordered backend modules, so LLVM
tasks run independently while sharing the same program-level indexes and
canonical type store.

### 4.4 Query Products And Cache Ownership

Query keys, products, and storage policies form one contract:

```text
+------------------+       +----------------------+       +-------------+
| typed query key  | ----> | provider execution   | ----> | query slot  |
+------------------+       +----------+-----------+       +------+------+
                                      |                          |
                                      | records dependencies     |
                                      v                          v
                              QueryDependencyGraph        product ownership
                                                                 |
                                     +---------------------------+----------------+
                                     |                                            |
                                     v                                            v
                               CacheOwnedArc                              SingleConsumerOwned
                          shared immutable product                      one consumer moves value
```

A cache-owned product may be shared by multiple consumers and fingerprinted when
its query contract permits it. A single-consumer product moves out of its slot
and therefore does not retain a reusable value fingerprint. Source replacement
enters a retirement barrier, drains active work, removes obsolete keys and
graph edges, and then admits new queries. Persistent caches are an additional
storage layer under the rules in [section 3](#3-cross-cutting-rules); they do
not replace in-memory dependency tracking.

## 5. Crate Index

Layers follow [section 2.3](#23-crate-dependency-layers). Each crate's
crate-level rustdoc is the detailed reference.

| Crate | Responsibility |
| --- | --- |
| **Product surface** | |
| `nia-cli` | The `nia` command: argument parsing, toolchain resolution, command dispatch, ICE boundary. |
| `nia-build` | Bootstrap and invocation of `nia build`: runs `build.nia`, decodes its plan, executes actions. |
| `nia-build-plan` | Immutable build-plan model, codec, and graph validation. |
| **Orchestration** | |
| `nia-driver` | Request sequencing, artifact production, object/archive/executable caches, linking. |
| `nia-compiler-query` | Session-owned compiler query facade and registry joining loader facts to semantic and backend products. |
| `nia-loader-query` | Source loading, module discovery, incremental reparsing, persistent frontend products. |
| `nia-loader-contract` | Data contract between loading and compilation: loader facts, module snapshots, provider demands. |
| **Native backend** | |
| `nia-codegen-llvm` | Backend IR validation and LLVM IR, object, and LTO bitcode emission. |
| `nia-llvm` | Typed, fallible wrappers over the LLVM C API, including the LTO coordinator. |
| `nia-linker` | Typed linker and archiver invocation, LLD flavor selection, link cache identity. |
| **Backend model** | |
| `nia-backend-ir` | Validated, target-independent program representation consumed by codegen. |
| `nia-backend-lower` | Program-wide planning and lowering of function IR into backend IR, with module-level optimization. |
| **Executable planning** | |
| `nia-executable-facts` | Per-item executable dependency facts extracted from typed bodies or semantic facts. |
| `nia-executable-reachability` | Least-fixed-point closure of functions, globals, and modules reachable from entry points. |
| `nia-monomorphize` | Concrete generic instance collection with type and const substitution. |
| `nia-function-lower` | Lowering of checked Body IR into validated function-level control-flow IR. |
| `nia-function-opt` | Policy-driven function-local optimization passes. |
| **Semantic analysis** | |
| `nia-layout` | Size, alignment, and field offsets for concrete types. |
| `nia-abi-check` | C ABI boundary validation and internal Nia signature classification. |
| `nia-trait-solve` | Trait selection, associated types and consts, intrinsic trait semantics. |
| `nia-program-signatures` | Program-wide signature views, extension discovery, trait-impl indexes. |
| `nia-const-eval` | Environment-parametric interpreter for const IR. |
| `nia-const-check` | Staged type checking and validation of compile-time expressions. |
| `nia-static-check` | Validation of `static` initializers against relocatable static data. |
| `nia-flow-check` | Syntax-level control-flow diagnostics. |
| `nia-body-check` | Typed body checking producing semantic facts and `BodyIr`. |
| `nia-closure-check` | Interprocedural escape analysis for closures and callable views. |
| `nia-pattern-analysis` | Pure usefulness and exhaustiveness analysis over pattern matrices. |
| `nia-sema` | Shared validation helpers for array lengths, arity, and fields. |
| **Declaration and resolution** | |
| `nia-target-config` | Evaluation of target, profile, and test conditions; pruning to the active item tree. |
| `nia-provider-summary` | Conservative extension-provider summaries and cross-module provider demands. |
| `nia-defs` | Stable definition identities, namespaces, and module-local definition tables. |
| `nia-imports` | Module maps, stable module identities, and the module graph. |
| `nia-public-surface` | Exported module surfaces and lexical `using` scopes. |
| `nia-type-resolve` | Resolution of type names to definitions, primitives, and generics. |
| `nia-type-lower` | Lowering of type syntax into canonical type ids. |
| `nia-item-signatures` | Module-local declaration signatures. |
| `nia-type-normalize` | Alias expansion and canonicalization of interned types. |
| `nia-value-resolve` | Resolution of value paths, qualified values, and associated values. |
| `nia-local-resolve` | Local scopes, bindings, and block-local imports. |
| **IR data models** | |
| `nia-sema-ir` | Persistent semantic fact schema keyed by definitions, locals, and node keys. |
| `nia-const-ir` | Early and resolved const-capable IR. |
| `nia-body-ir` | Typed, source-shaped checked bodies (`BodyIr`). |
| `nia-function-ir` | Function-level control-flow IR and its invariant validator. |
| `nia-static-ir` | Static initializer IR and relocation reference extraction. |
| `nia-ir-names` | Stable names for function-IR locals and promoted allocations. |
| `nia-mangle` | Canonical `_N` linker symbols and type-name encodings. |
| **Syntax** | |
| `nia-lexer` | Byte-offset tokenizer. |
| `nia-syntax` | Grammar, recovery, syntax diagnostics, and the lossless green/red tree. |
| `nia-ast` | Semantic-free AST nodes with spans and node identities. |
| `nia-ast-walk` | Structural AST traversal. |
| `nia-parser` | Lowering of finished syntax trees into AST with origin tables. |
| `nia-literals` | Numeric, character, and string literal decoding. |
| `nia-item-tree` | Module item trees before and after conditional selection. |
| **Shared foundation** | |
| `nia-query` | Query storage, dependency tracking, invalidation, cycle detection, bounded parallelism. |
| `nia-ty` | Type model and session-wide canonical `TypeStore`. |
| `nia-ids` | Session-local typed ids and the builtin registry. |
| `nia-symbol` | Stable hashed symbol identities, known symbols, and source-defined predeclared trait contracts. |
| `nia-symbol-table` | Concurrent reverse text storage for symbols. |
| `nia-source` | Source identities, revisions, paths, and concurrent source storage. |
| `nia-span` | Half-open source ranges. |
| `nia-node-id` | Session-local syntax node handles and revision-stable node keys. |
| `nia-diagnostic` | Structured diagnostics, persistence, and rendering. |
| `nia-ice` | Structured internal compiler errors. |
| `nia-hash` | Deterministic non-cryptographic hashing for internal maps. |
| `nia-timing` | Timing, trace events, and optional heap accounting. |
| `nia-target` | Typed model of the maintained compilation targets. |
| `nia-compat` | Protocol registry: format magic and the release-derived protocol epoch. |
| `nia-opt` | Declarative optimization policy matrix. |
| `nia-package-metadata` | Relocation-independent package, module, and definition identities; stable type graphs. |
| `nia-toolchain` | Toolchain resource layout discovery and compatibility identity. |
| **Test infrastructure** | |
| `nia-test-support` | Resource-accounted child-process helpers for workspace tests. |
