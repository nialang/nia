# Frontend Incremental Evolution Tasks

Status: active branch plan for `evolve/frontend-incremental-red-green`

This document tracks the frontend architecture work from the current
source/token/parse query split toward a stable incremental frontend and a
future red/green syntax tree. It is intentionally kept in the branch root and
should live for the lifetime of this evolution branch.

## Principles

- Keep ownership boundaries explicit: source identity belongs in `nia-source`,
  import resolution belongs in `nia-imports`, query orchestration belongs in
  query-facing crates, and semantic facts belong in semantic tables.
- Prefer typed query inputs and outputs over program-wide synchronized `Vec`
  state.
- Do not move semantic ids into AST nodes.
- Do not introduce a second syntax representation until source identity,
  query keys, and invalidation are ready for it.
- Every check-path regression test that can affect emit must have an emit-path
  companion test.
- Avoid compatibility shims that become permanent historical baggage.

## Phase 1: Source Input Database

Goal: make source files explicit compiler inputs rather than path reads hidden
inside frontend queries.

Tasks:

- [x] Design `nia-source` as the owner of source session state.
- [x] Add a source database/input type that stores `SourceFile` values by
      `SourceId` and maps `SourcePath` to `SourceId`.
- [x] Keep `SourceTable` or replace it with the new database if the new type
      fully covers path interning.
- [x] Make source loading from disk a provider of source input, not logic hidden
      in parse/import queries.
- [x] Support test/in-memory source injection without using temporary files as
      the only path.
- [x] Preserve `SourceRevision` on each source file and make text replacement
      advance the revision.
- [x] Document whether source ids are session-local only or intended to become
      stable across sessions.

Completion criteria:

- `nia-loader-query` does not directly own source identity allocation.
- Source text reads flow through a source input abstraction.
- Existing CLI behavior is unchanged.
- Tests cover path id reuse, text replacement revision bumps, and in-memory
  source input.

Verification:

- [x] `cargo test -p nia-source -p nia-loader-query`
- [x] `cargo test`
- [x] `cargo clippy --all-targets -- -D warnings`

## Phase 2: SourceId And Revision Query Keys

Goal: make token, parse, and import queries depend on source identity and source
revision instead of raw paths.

Tasks:

- [x] Introduce query key structs for source-versioned inputs, such as
      `SourceVersion { id: SourceId, revision: SourceRevision }`.
- [x] Change token query keys from `SourcePath` to source-version keys.
- [x] Change parse query keys from `SourcePath` to source-version keys.
- [x] Change import query keys to use the parsed source version while still
      returning path/module import edges.
- [x] Keep path-based root loading as a convenience API that resolves to source
      ids before querying token/parse/import layers.
- [x] Ensure query descriptions include both path and source version where
      useful for diagnostics and trace readability.
- [x] Add dependency trace tests for:
      - parsed source depends on tokenized source;
      - tokenized source depends on source input;
      - module imports depend on parsed source.

Completion criteria:

- Frontend query cache identity changes when a source revision changes.
- Path strings are no longer the sole cache key for token/parse/import queries.
- Module graph behavior remains unchanged for batch CLI compilation.

Verification:

- [x] `cargo test -p nia-source -p nia-loader-query -p nia-query`
- [x] `cargo test`
- [x] `cargo clippy --all-targets -- -D warnings`

## Phase 3: Definition And Signature Queries

Goal: move top-level definition and signature collection onto typed query
boundaries before deeper semantic passes are split.

Tasks:

- [x] Identify current definition/signature collection paths in
      `nia-compiler-query` and related crates.
- [x] Add a module definition query that maps a parsed module to item defs.
- [x] Add public surface/signature queries for structs, enums, functions,
      globals, aliases, and extensions.
- [x] Keep ordered program aggregation only where order is semantically or
      diagnostically required.
- [x] Replace fragile cross-module lookup paths with direct queries keyed by
      module/item ids.
- [x] Add regression tests for imported nominal types, imported array lengths,
      imported enum variants, and imported associated items through the new
      query path.
- [x] Ensure no query provider reconstructs semantic facts by re-reading AST
      shapes when an earlier query already computed the fact.

Completion criteria:

- Cross-module public surface access is query-backed.
- Program-level aggregate data is derived from typed query results, not from
  hidden synchronized indexing assumptions.
- Existing check and emit behavior is unchanged except for bug fixes.

Verification:

- [x] `cargo test -p nia-compiler-query -p nia-loader-query`
- [x] `cargo test`
- [x] `cargo clippy --all-targets -- -D warnings`

## Phase 4: Semantic Query Boundaries

Goal: split semantic passes by stable module/item/function boundaries while
preserving the current pass ownership model.

Tasks:

- [x] Define query boundaries for name resolution, type resolution, comptime
      evaluation, body checking, flow checking, ABI checking, layout, and
      function IR lowering.
- [x] Prefer item/function-level body queries where the input identity is
      already stable.
- [x] Keep expression-level queries out of scope until stable node identity
      exists.
- [x] Make body-check outputs explicitly carry downstream decisions currently
      rediscovered by lowering/codegen, including:
      - bracket suffix resolution;
      - builtin call classification;
      - place/value classification where needed;
      - defer/control-flow facts needed by lowering.
- [x] Remove duplicated AST-shape inference in later passes when a semantic
      query output can be consumed instead.
- [x] Add tests that intentionally exercise check/emit parity for module
      boundaries and control-flow edges.

Completion criteria:

- Backend/lowering consumes semantic facts instead of guessing from raw AST for
  known unstable cases.
- Query boundaries align with existing crate responsibilities rather than
  concentrating logic in the driver.
- No new monolithic "mega MIR" or "mega frontend" crate is introduced.

Verification:

- [x] `cargo test -p nia-body-check -p nia-function-lower -p nia-backend-lower`
- [x] `cargo test`
- [x] `cargo clippy --all-targets -- -D warnings`

## Phase 5: In-Memory Invalidation

Goal: make the existing dependency trace useful for same-session incremental
recomputation.

Tasks:

- [ ] Extend `nia-query` with a cache invalidation API.
- [ ] Track reverse dependencies for query frames or typed query keys.
- [ ] Invalidate direct and transitive dependents when a source revision
      changes.
- [ ] Preserve deterministic behavior when invalidation races with scoped
      `query_many` workers.
- [ ] Decide whether invalidation is coarse by query frame or typed by query key
      storage.
- [ ] Add tests for invalidating:
      - source input;
      - token and parse results;
      - import graph after import text changes;
      - semantic query results after a public surface changes.

Completion criteria:

- Same-session source edits can recompute affected frontend queries without
  recreating the full query database.
- Batch CLI still works by building a fresh query database.
- No persistent cache format is required.

Verification:

- [ ] `cargo test -p nia-query -p nia-source -p nia-loader-query`
- [ ] `cargo test`
- [ ] `cargo clippy --all-targets -- -D warnings`

## Phase 6: Stable Node Identity

Goal: establish syntax node identity without embedding semantic ids in AST.

Tasks:

- [ ] Define a node identity model based on `SourceId`, `SourceRevision`, syntax
      kind, span, and later green-tree child path.
- [ ] Add a lightweight node key type if needed by semantic tables.
- [ ] Record semantic facts in side tables keyed by node identity or existing
      item/local ids.
- [ ] Keep AST structs semantic-free.
- [ ] Audit places where later passes need information currently lost after
      body-check and add explicit side-table outputs.
- [ ] Decide how node identity behaves across source edits before red/green
      trees exist.

Completion criteria:

- Semantic outputs can refer to syntax positions without mutating AST nodes.
- The design is compatible with later red/green child paths.
- There is a clear migration path from span-based keys to green-tree keys.

Verification:

- [ ] `cargo test -p nia-body-check -p nia-local-resolve`
- [ ] `cargo test`
- [ ] `cargo clippy --all-targets -- -D warnings`

## Phase 7: Red/Green Syntax Tree

Goal: introduce a lossless syntax layer only after source identity, query keys,
invalidation, and node identity are stable enough to support it.

Tasks:

- [ ] Design a `nia-syntax` crate for green nodes, red nodes, tokens, trivia,
      and syntax kinds.
- [ ] Decide whether the lexer emits syntax tokens directly or whether syntax
      tokens are adapted from existing lexer tokens.
- [ ] Implement lossless parsing into green trees without changing language
      semantics.
- [ ] Lower syntax trees into the existing AST or provide AST views over syntax
      trees.
- [ ] Keep parser diagnostics stable or intentionally improve them with tests.
- [ ] Add node identity based on green-tree child paths.
- [ ] Enable partial reparsing only after full-tree parsing is stable.
- [ ] Keep IDE/LSP use cases in mind, but do not force an LSP architecture into
      the compiler crates.

Completion criteria:

- Nia has one official syntax layer for lossless source representation.
- Existing AST consumers either receive lowered AST or syntax-backed AST views.
- Batch check/emit behavior is unchanged.
- Red/green nodes participate in query dependencies through source revisions.

Verification:

- [ ] `cargo test -p nia-lexer -p nia-parser`
- [ ] `cargo test`
- [ ] `cargo clippy --all-targets -- -D warnings`

## Deferred Until After Red/Green

- Persistent on-disk incremental cache.
- Cross-session fingerprint reuse.
- IDE/LSP server architecture.
- Fine-grained expression-level semantic queries.
- Partial reparsing optimizations beyond correctness-focused prototypes.
- Query scheduler/runtime with priorities, cancellation, or a dedicated thread
  pool.
