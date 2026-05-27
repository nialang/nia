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

## 3. Foundation Crates

### 3.1 `nia-span`

Defines source spans with byte offsets. It does not depend on AST, lexer,
parser, diagnostics, or any semantic phase.

### 3.2 `nia-ids`

Defines stable cross-phase ids:

- `ModuleId`;
- `DefId`;
- `LocalId`;
- `TyId`;
- `GlobalDefId`.

It stores no semantic tables and has no filesystem, parser, or diagnostic
responsibility.

### 3.3 `nia-diagnostic`

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

### 4.2 `nia-ast`

Defines the parsed syntax tree. AST nodes represent source structure and spans.
They do not store type ids, def ids, layout information, or backend values.

### 4.3 `nia-parser`

Builds AST from tokens and reports parse errors. It owns grammar decisions and
local parse recovery.

Important parser boundary:

- expression bracket suffixes are parsed in a syntax-preserving form;
- semantic disambiguation of generic instantiation vs indexing happens later;
- removed historical spellings should not get special migration paths.

### 4.4 `nia-ast-walk`

Provides AST traversal helpers for phases that need tree walking. It should stay
small and generic. It must not embed semantic policy.

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

### 8.4 `nia-layout`

Computes ABI-relevant layout for primitive, pointer, array, struct, enum, and
instantiated nominal types. It uses explicit target data layout assumptions, such
as LP64, rather than hidden host assumptions.

### 8.5 `nia-abi-check`

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

## 10. Monomorphization And Symbols

### 10.1 `nia-monomorphize`

Collects concrete generic function and method instances required by the checked
program. It deduplicates instances and diagnoses recursive generic expansion.

### 10.2 `nia-mangle`

Builds deterministic internal symbol names from module ids, definition ids, and
type encodings. It is not C++ or Rust mangling. It should stay readable and
debuggable.

Extern symbols bypass internal mangling and use their source names.

## 11. Backend IR

### 11.1 `nia-backend-ir`

Defines typed backend IR consumed by codegen. It is lower-level than AST and
contains type-checked, resolved program structure. It is not a full MIR.

Backend IR should be explicit enough for LLVM codegen without forcing codegen to
re-run semantic analysis.

### 11.2 `nia-backend-lower`

Lowers checked modules into backend IR. It uses definitions, lowered types,
signatures, layouts, body-check results, monomorphized instances, and public
module information.

It owns translation from semantic expressions into typed backend expressions,
places, statements, static initializers, and inline assembly operands.

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

`emit obj` may produce multiple object files because backend lowering can produce
multiple codegen units. `-o` is only valid for single-unit output; `--out-dir` is
the multi-unit form. `emit exe` uses host linking and is therefore part of the
host execution model.

## 14. Diagnostics

Every phase returns diagnostics instead of panicking on user source errors.
Diagnostics should carry spans whenever source text is involved.

Implementation bugs may panic in tests, but normal invalid Nia programs should
flow through diagnostic reporting.

Diagnostics should describe current language rules. The compiler should not keep
special migration diagnostics for syntax that only existed during pre-release
development.

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

Nia is pre-public-release, so temporary historical forms are not compatibility
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
