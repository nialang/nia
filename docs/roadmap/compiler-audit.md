# Compiler Audit And Soundness Roadmap

Status: active temporary roadmap

This roadmap governs the pre-0.1.0 whole-repository audit. It is intentionally
temporary: the implementation, tests, and durable maintenance rules belong in
their owning crates and stable documentation. Keep this file while work is in
progress so that architectural debt, review coverage, and residual risk remain
visible. Delete it only after every acceptance item in Section 10 is closed.

The roadmap is an audit and hardening project, not a version-release plan. It
does not change Nia's planned `0.1.0` milestone or introduce a compatibility
promise for pre-0.1.0 syntax and behavior.

## 1. Objectives

The project must leave Nia with:

- sound phase boundaries from source through LLVM and executable output;
- one owner for each identity, substitution, diagnostic, cache product, and
  execution policy;
- implementation files split when they expose independent algorithms or data
  ownership, without cosmetic fragmentation;
- complete propagation of type, const, lifetime, layout, and provenance facts;
- tests that exercise both ordinary behavior and invariant/error boundaries;
- professional Rustdoc and focused comments for non-obvious algorithms;
- explicit residual-risk records for behavior that cannot yet be verified;
- clean, reproducible, resource-controlled tests on constrained and larger
  machines.

The project does not preserve removed syntax or old implementation paths merely
for historical compatibility. Any compatibility adapter must be a bounded
migration boundary with a deletion condition.

## 2. Working Rules

Each phase follows the same loop:

1. Map the owner, consumers, identity, and current tests before editing.
2. Review the complete data flow, not only the first failing call site.
3. Repair the owning abstraction and remove obsolete duplicate paths.
4. Add focused unit/property/regression tests and the relevant end-to-end test.
5. Add Rustdoc or a short algorithm comment where a reviewer would otherwise
   need to reconstruct an invariant from several functions.
6. Run the narrow owner tests, affected consumer tests, workspace check,
   formatting, and the applicable integration/resource gate.
7. Update this roadmap and commit a coherent batch using Conventional Commit
   style before moving to an unrelated phase.

Tests must use libtest's normal concurrency and the shared resource accounting
from `nia-test-support`. Do not make `--test-threads=1` the acceptance path.

## 3. Coverage Baseline

The initial inventory found approximately 639 Rust files and 321k Rust code
lines. Tokei reports only about 1.1k Rust comment lines; even including the
embedded documentation bucket, the repository is below one percent comments.
This is an investigation signal, not a target ratio. Comments are required
where they explain invariants, not where they restate syntax.

The following areas have received recent focused changes, but are **not** yet
considered fully audited:

- const-generic identity propagation through semantic facts, monomorphization,
  trait dispatch, backend lowering, and executable reachability;
- pattern analysis, nominal patterns, arrays, tuple structs, and `match`;
- closure syntax and initial closure type inference;
- selected const/static initialization paths;
- query cycle detection and test-process resource accounting.

## 4. Phase A: Type, Trait, And Body Soundness (P0)

Owner crates: `nia-body-check`, `nia-trait-solve`, `nia-program-signatures`,
`nia-type-resolve`, `nia-type-lower`, `nia-type-normalize`, `nia-value-resolve`,
`nia-local-resolve`.

Primary files to audit first:

- `nia-body-check/src/inference.rs`;
- `nia-body-check/src/type_support.rs`;
- `nia-body-check/src/projection_obligations.rs`;
- `nia-body-check/src/calls/`;
- `nia-body-check/src/trait_objects.rs`;
- `nia-body-check/src/aggregates.rs`;
- `nia-trait-solve/src/`;
- `nia-program-signatures/src/analysis/`.

Required review topics:

- HM-style constraint generation, unification, occurs checks, rollback, and
  inference failure recovery;
- interleaved type and const generic parameter ordering at every substitution
  boundary;
- trait candidate filtering, specialization ordering, ambiguity, negative
  results, and visibility/module ownership;
- where predicates, supertraits, projection normalization, associated types,
  associated values, and recursive obligations;
- trait-object construction, upcasting, vtable identity, and dynamic calls;
- contextual inference for arrays, aggregates, closures, calls, and returns;
- pattern typing and binding-mode propagation;
- place typing, mutability, pointer coercions, temporary lifetimes, and error
  union/optional propagation;
- diagnostics for each rejected candidate and each unresolved obligation.

Architecture actions:

- split files only at stable ownership boundaries such as inference state,
  projection solving, aggregate typing, and call resolution;
- make substitution helpers canonical and remove local reimplementations;
- document the constraint lattice, candidate ordering, and cycle termination;
- add tests for repeated generic parameters, mixed type/const parameters,
  nested closures, projection cycles, ambiguous impls, and cross-module traits.

Acceptance:

- clean and incremental body/signature checks agree on all added matrices;
- no trait or projection path silently drops type, const, or module context;
- diagnostics identify the actual failed obligation without changing valid
  program acceptance;
- owner tests plus `nia-compiler-query` generic/trait suites pass.

## 5. Phase B: Layout, ABI, Backend IR, And LLVM Safety (P0)

Owner crates: `nia-layout`, `nia-abi-check`, `nia-backend-ir`,
`nia-backend-lower`, `nia-function-ir`, `nia-function-lower`, `nia-function-opt`,
`nia-llvm`, `nia-codegen-llvm`, `nia-mangle`.

Required review topics:

- layout and alignment for primitives, arrays, tuples, nominal structs,
  tuple structs, enums, unions, zero-sized values, closures, and trait objects;
- overflow, target pointer width, endianness assumptions, field offsets, and
  discriminant representation;
- ABI classification for direct/indirect parameters, hidden return storage,
  variadics, extern functions, function pointers, and dynamic trait calls;
- Body IR and backend IR validation, terminator completeness, unreachable
  blocks, ownership of promoted allocations, and static data;
- optimizer preservation of evaluation order, side effects, volatile/atomic
  operations, inline assembly, and defer bodies;
- LLVM handle lifetime, context ownership, null returns, disposal, target data
  layout, object buffers, and typed GEP/cast preconditions;
- deterministic codegen indexes, fingerprints, mangling, vtable emission, and
  incremental object reuse.

Mandatory LLVM boundary tests must cover malformed/empty returns, target-data
  failure, zero-sized values, aggregate returns, vtable calls, and at least two
  pointer widths. Unsafe wrapper preconditions belong in Rustdoc at the wrapper,
  not only in higher-level code.

Acceptance:

- every backend entry point has a documented invariant or explicit `Result`;
- backend validation rejects malformed IR before LLVM receives it;
- representative programs produce identical ABI/codegen results under clean
  and incremental paths;
- `nia-codegen-llvm` and linker integration tests pass on the supported target.

## 6. Phase C: Const, Static, Closure, Flow, And IR Semantics (P0)

Owner crates: `nia-const-ir`, `nia-const-eval`, `nia-const-check`,
`nia-static-check`, `nia-static-ir`, `nia-closure-check`, `nia-flow-check`,
`nia-body-ir`.

Required review topics:

- distinction between Nia `const` compile-time evaluation and `static` storage;
- evaluation frames, recursion, assignment writeback, error state rollback,
  overflow, target-dependent values, and const-generic expressions;
- pointer provenance, address-taking, promotion, local static ownership,
  initialization order, dependency cycles, and nested declarations;
- compile-time rejection of IO/asm/runtime-only operations;
- closure escape fixpoints, captured addresses, callable views, stack-backed
  storage, recursive/mutually recursive closures, and error provenance;
- flow reachability, match/if-pattern coverage, defer control flow, and exact
  typed Body IR invariants.

Acceptance:

- static and const tests explicitly cover the boundaries above, not only happy
  paths;
- closure safety has direct unit tests in addition to compiler-query tests;
- every Body IR variant is covered by walking, validation, lowering, and
  reachability tests;
- no optimizer or const evaluator drops an observable side effect.

## 7. Phase D: Query, Cache, Loader, Import, And Build State (P1)

Owner crates: `nia-query`, `nia-compiler-query`, `nia-loader-query`,
`nia-imports`, `nia-public-surface`, `nia-provider-summary`, `nia-build`,
`nia-source`, `nia-toolchain`, `nia-linker`, `nia-driver`, `nia-cli`.

Required review topics:

- query slot lifecycle, nested execution, cycle detection, cancellation,
  invalidation, retirement, and executor shutdown;
- clean/incremental red-green equivalence and randomized edit sequences;
- persistent cache schema, stable identity, fingerprints, corruption, bounds,
  truncation/trailing bytes, and verification replacement;
- module graph revisions, forks, facades, reexports, provider activation, and
  path-independent identities;
- build-plan canonical encoding, action cache keys, process groups, output
  recovery, stale locks, output races, and cancellation;
- command paths, target/toolchain selection, environment propagation, linker
  failures, and user-facing diagnostics;
- memory/CPU/jobserver accounting under low memory, cgroups, WSL, and nested
  compiler/build/runtime processes.

Acceptance:

- all persisted products have roundtrip, corruption, stale-identity, and
  replacement tests;
- concurrent query/build state machines have deterministic race-focused tests;
- clean and incremental compiler outputs are equivalent for representative
  multi-module and cross-process workloads;
- low-memory tests demonstrate bounded behavior without requiring serialized
  libtest execution.

## 8. Phase E: Frontend, Identity, Diagnostics, And Structural Boundaries (P1)

Owner crates: `nia-lexer`, `nia-syntax`, `nia-ast`, `nia-ast-walk`, `nia-parser`,
`nia-item-tree`, `nia-defs`, `nia-ids`, `nia-node-id`, `nia-symbol`,
`nia-symbol-table`, `nia-span`, `nia-diagnostic`, `nia-ice`.

Required review topics:

- parser recovery, speculative rollback, delimiter errors, precedence,
  closure/array/aggregate ambiguity, and stable node origins;
- lexical edge cases, UTF-8 boundaries, comments, literals, and source spans;
- stable identity allocation, generation/stale-handle behavior, symbol
  collision handling, and cross-revision source mapping;
- diagnostic ownership, ordering, spans, ICE boundaries, and persistent
  diagnostic identity;
- public API visibility and crate dependency direction.

Acceptance:

- parser tests cover every current syntax family and representative malformed
  input with stable diagnostics;
- identity tests cover revision replacement, stale handles, collision errors,
  and deterministic hashing;
- structural reports show no accidental dependency-cycle or ownership leak;
- large files are split only where the resulting owner API is narrower.

## 9. Phase F: Standard Library And Runtime (P1)

This is a separate product surface even when compiler changes are required.
Review `lib/std` in ownership order:

1. `builtin`, `atomic`, memory and allocators;
2. startup/runtime and process/syscall layers;
3. `io`, `fs`, paths, and error conversion;
4. collections, hashing, iterators, strings, formatting, and parsing;
5. build API and plan protocol.

Required evidence includes real compile/run tests, allocator and ownership
tests, syscall/ABI tests, error-path tests, target-specific behavior, and
build-host protocol roundtrips. Do not use compiler-only acceptance as a
substitute for runtime evidence.

## 10. Cross-Cutting Documentation And Test Work

Every touched algorithm must gain concise Rustdoc or an inline invariant
comment. At minimum, document:

- substitution and inference invariants;
- pattern usefulness/exhaustiveness matrix semantics;
- trait candidate ordering and cycle termination;
- layout/ABI classification rules;
- const evaluator state transitions;
- closure provenance and escape lattice;
- query/cache state machines;
- resource budget derivation and process ownership;
- LLVM wrapper safety preconditions;
- stable identity and persistence formats.

Tests must be added at the owner boundary first, then at the nearest end-to-end
consumer. Repeated matrices should be data-driven; dynamic process, filesystem,
allocator, startup, and runtime behavior should remain explicit integration
tests.

## 11. Progress Tracking

This section records implementation batches, not a fixed-size completion
percentage. The batch ledger grows whenever the audit discovers another
owner-boundary or regression matrix, so `completed / total` is intentionally
not a reliable project percentage: completing work can increase both numbers.
Track the fixed acceptance checklist below separately from this expandable
ledger, and report the two dimensions together.

Current snapshot (2026-08-22):

- Implementation batches: 253 completed entries in this ledger.
- Fixed acceptance items: 1 of 8 completed (the seven unchecked entries at the
  end of this section).
- The implementation ledger is evidence of covered batches; phase completion
  requires the corresponding fixed acceptance item plus the cross-cutting
  documentation, test-gap, and final validation evidence.

Use this section as the only temporary progress ledger. Update it in the same
commit as the corresponding implementation batch, and update the fixed
acceptance item only when its phase-wide evidence is complete.

- [x] Repository inventory and initial risk map.
- [x] Trait const arguments preserved through executable reachability.
- [x] Focused reachability identity regression test.
- [x] Initial Phase A owner map: local-resolution ordering, inference DAG
      boundary, and trait/projection cycle guards reviewed.
- [x] Canonicalized const-to-array-length substitution and rejected negative
      signed lengths at the type boundary.
- [x] Preserved const arguments and associated-type bindings during body-check
      trait-obligation deduplication; supertrait audit remains open.
- [x] Phase B LLVM target-layout and object-buffer null/empty boundaries
      hardened; broader backend audit remains open.
- [x] Unix build stale-lock reclamation made race-resistant with inode locks;
      non-Unix fallback and broader build/cache state audit remain open.
- [x] Build lock owner records and Linux process-start probes now use small
      stream-enforced protocol budgets. Oversized cache lock files cannot drive
      unbounded `read_to_string` allocations or smuggle a valid owner prefix,
      and owner Drop will not remove an oversized replacement record.
- [x] Query memory accounting now bounds every Linux meminfo/cgroup pseudo-file
      read on the opened stream and rejects cgroup membership paths containing
      non-normal components. Oversized kernel/container records degrade to the
      conservative memory fallback without unbounded allocation or escaping
      the expected cgroup mount.
- [x] Unix `nia-test-support` process-slot reclamation now retains owner-file
      locks, preventing stale slot cleanup from deleting a successor permit.
- [x] Phase A owner regression evidence: `nia-body-check` 228 libtest cases
      pass after obligation-identity hardening.
- [x] Resource-pool owner evidence: `nia-test-support` 17 libtest cases and
      strict Clippy pass.
- [x] Strict `-D warnings` dependency gate is clean for `nia-test-support` and
      `nia-build`; removed the `FunctionTerminator` and `TypedMatchArmBody`
      large-enum variants by boxing their uncommon/larger payloads.
- [x] Build lock acquisition now waits through the create/reclaimer inode-lock
      race; the full `nia-build` suite (189 passed, 1 ignored) covers concurrent
      publishers and stale-owner recovery.
- [x] Reachability trait-method expansion groups the concrete trait instance
      inputs into a named context, keeping the helper interface narrow without
      suppressing `too_many_arguments` diagnostics.
- [x] Persistent signature-cache publication enforces the same bounded entry
      size used by decoders, preventing oversized type/signature products from
      being written only to become guaranteed cache misses on the next load.
- [x] Persistent signature-cache reads enforce that shared 64 MiB budget on
      the opened stream as well as file metadata, so all six products reject
      oversized or concurrently growing entries without an unbounded
      `fs::read` allocation.
- [x] Persistent signature-cache publishers and corruption retirement now use
      per-key OS file locks. Retirement repeats the bounded read and deletes
      only the same observed bytes (or a still-oversized record), while
      non-replacing publication rechecks ownership under the lock; stale
      readers cannot remove or overwrite a concurrent replacement.
- [x] Loader frontend-cache publication now enforces its read-side size bound,
      and permission, I/O, and other publication failures remain visible to
      callers rather than being treated as generic concurrent-writer success.
- [x] Loader frontend-cache publication and retirement now share per-key OS
      locks on every persisted product. Publication rechecks the winner before
      rename on all platforms, while corrupt/invalidated observations are
      deleted only if the same bounded bytes remain installed; concurrent
      replacements are preserved.
- [x] Phase E syntax trees normalize caller-supplied token streams to one
      terminal EOF, discard unreachable post-EOF tokens, and make arbitrary
      cursor lookahead overflow-safe; syntax and parser owner suites pass.
- [x] Phase E node maps and origin tables compare and merge by stable locator
      across retired/reacquired handle generations, restoring `Eq`
      transitivity and preventing duplicate logical keys in one product.
- [x] Phase E symbol-table construction now has one initialization contract:
      `Default` and `new` both install the verified collision-free well-known
      symbol registry used by parsing, diagnostics, and persistence.
- [x] Stable diagnostic bundles enforce their 64 MiB budget incrementally and
      cap initial allocations derived from untrusted sequence lengths; all 222
      compiler-query persistence and incremental consumer tests pass.
- [x] Diagnostic reports now deduplicate by the complete structured diagnostic
      in linear expected time, preserving distinct secondary labels, notes,
      help, related locations, and internal debug evidence.
- [x] Phase E lexer error recovery consumes unsupported Unicode by scalar, so
      every byte span remains a UTF-8 boundary and lossless syntax trees retain
      the exact source even though language identifiers remain ASCII.
- [x] Builtin registries now expose exhaustive variant sets with bidirectional
      name/descriptor coverage tests; persisted builtin function and const tags
      use explicit append-only numbers rather than mutable `ALL` ordering.
- [x] Phase E canonical AST traversal now exposes pattern and generic-parameter
      callbacks, reaches binding/for patterns and every const generic type, and
      makes embedded pattern expressions available to semantic/reachability
      visitors while preserving const generic types in loader dependency
      discovery and versioned type-use products.
- [x] Phase C static lowering now treats newly emitted diagnostics as a failed
      product transaction: recovery `StaticInit::Zero` values are discarded
      from Full, FactsOnly, and StaticInitOnly Body IR paths, with a reachable
      rejected-initializer regression test and an explicit IR invariant.
- [x] Phase E caller-supplied syntax token streams normalize a terminal EOF's
      span to the actual source boundary, preserving root/cursor offsets even
      when external lexers provide a stale EOF location.
- [x] Phase C flow audit documented the ownership boundary between conservative
      syntactic return joins and typed pattern-matrix exhaustiveness; nominal
      constructors and literal/range coverage remain intentionally delegated
      to `nia-body-check::patterns`.
- [x] Phase C pattern analysis now validates non-empty queries before applying
      the empty-matrix witness shortcut, so uninhabited domains and invalid
      scalar/constructor combinations cannot be silently accepted; stable
      witness shape is preserved and matrix boundary tests cover the contract.
- [x] Phase C empty-matrix pattern validation now terminates on recursive type
      domains using an inductive type-path guard; cycle-only constructors remain
      uninhabited while finite-base recursive domains remain accepted, with
      independent-column regression coverage.
- [x] Phase C recursive pattern inhabitation now guards only unconstrained
      wildcard expansion instead of conflating an explicitly selected outer
      constructor with a nested occurrence of the same type. Mutable optional
      pointer patterns over recursive allocator nodes can reach their nested
      `null` base case without weakening rejection of cycle-only domains; owner
      and standard-library build-case regressions cover both boundaries.
- [x] Phase C backend planning now distinguishes source functions that merely
      have checked bodies from functions already selected by executable
      reachability. Vtables discovered after the frontend plan route late
      cross-module implementation methods back to their defining module, with
      owner and allocator-backed executable regressions covering the closure.
- [x] Phase C codegen readiness now treats dynamic trait calls as declaration
      edges to every planned candidate vtable and slot target. The owner
      directory indexes both exact object tables and supertrait segments, so
      definition validation cannot race ahead of cross-module function
      signatures; owner, upcast-boundary, and allocator executable regressions
      cover the publication-order invariant.
- [x] Phase B dynamic-call ABI validation now checks the selected method slot
      and target signature in every concrete vtable for an object view, rather
      than accepting whichever table was indexed first. Multiple-self
      regressions keep malformed later targets and publication-order changes
      from bypassing backend validation.
- [x] Phase B dynamic-call ABI validation now checks direct object tables and
      upcast source tables as one runtime candidate set. Source-table slots are
      rebased from the object view's principal trait segment, so a direct table
      cannot hide a malformed non-zero-offset upcast target or misclassify an
      unrelated table that merely shares a supertrait.
- [x] Phase A local HM inference now performs a symmetric occurs-check and
      transactional union rollback, preventing self-referential structural
      terms and late tuple/callable conflicts from poisoning later constraints;
      the self-reference diagnostic path has a no-recursion regression test.
- [x] Phase A associated-type projection cycle guards release their active key
      on missing source items as well as successful/recursive paths; the trait
      solver regression suite now checks this cleanup before resolving a valid
      sibling projection.
- [x] Phase B LLVM vtable emission, dynamic dispatch, and trait-object upcasts
      use checked conversions for array lengths and GEP indices; oversized
      entry/slot counts and slot-plus-one overflow now become diagnostics rather
      than truncating host `usize` values, with boundary tests at `u32::MAX`.
- [x] Phase B LLVM wrappers now validate serialized/object buffer starts before
      forming slices, handle verifier failures without dereferencing null error
      messages, reclaim partial bitcode modules and failed emission buffers, and
      propagate null target-machine triples instead of silently accepting an
      unconfigured module.
- [x] Phase B backend function validation now checks boolean control-flow
      conditions, integer switch targets and arm type equality, declared return
      values against the enclosing function signature, and `Never`-terminating
      bodies without rejecting valid defer-tail or trap lowering; malformed
      terminator contracts have a pre-LLVM regression test.
- [x] Phase B backend value validation now checks tuple and index projection
      result types plus tuple, array, repeat-array, and direct aggregate
      field initializer contracts before LLVM. A malformed-IR matrix covers
      element types, arity/count metadata, and selected-value types.
- [x] Phase A/B atomic validation now enforces pointer-width value types,
      pointee and write-permission contracts, operand/result shapes, RMW type
      restrictions, and the per-op LLVM ordering sets before emission.
      Cmpxchg failure ordering uses the actual acquire/release partial order,
      rejecting incomparable `Release`/`Acquire` pairs at both source and
      backend boundaries; focused matrices cover malformed producer IR.
- [x] Phase B bulk-memory validation now checks mutable destination slices,
      element metadata, copy/move source slices, and byte-only set operations
      before LLVM extracts fat-pointer fields or computes byte counts. The
      Function IR operation documents this producer/consumer contract, and a
      malformed-IR matrix covers each independent mismatch.
- [x] Phase B low-level builtin validation now checks unaligned-load metadata
      and byte pointers plus SIMD splat/lane/bitmask, integer bit-intrinsic,
      and Unicode scalar conversion operand/result contracts before typed LLVM
      casts, extracts, calls, or aggregate writes. Function IR documents the
      per-variant contract and a malformed-IR matrix covers all mismatch axes.
      Bitmask lane limits and result widening now follow the target `usize`
      width instead of assuming LP64.
- [x] Phase B operator validation now checks unary dereference/reference shape,
      numeric and integer operand classes, compatible binary operands, logical
      bool-only operations, comparison masks, and result types before LLVM
      selects scalar/vector or short-circuit builders. A malformed operator
      matrix covers arithmetic, comparison, logical, shift, and dereference
      boundaries.
- [x] Phase B cast validation now checks target/result metadata, numeric
      scalar-versus-vector shape, enum/integer and pointer/integer categories,
      volatile-pointer support, and rejects malformed casts before LLVM chooses
      typed pointer, float, or integer builders.
- [x] Phase B tagged-union expression validation now checks Optional/ErrorUnion
      constructor payloads, discriminant `u8` projections, and branch payload
      projections before LLVM extracts aggregate fields. Field projections now
      enforce selected-field result types, while slices validate source/result
      element identity, readonly propagation, and integer range bounds; focused
      malformed-IR matrices cover each independent mismatch.
- [x] Phase B enum expression validation now checks variant representation,
      payload arity/types, backing-width tags, and payload projection owner,
      index, and result contracts before LLVM computes enum offsets. The
      malformed-IR matrix covers scalar and payload-bearing enum paths.
- [x] Phase B scalar, character, boolean, string, byte-string, and null
      literal validation now checks target primitive/array shape and known
      lengths before LLVM literal builders receive a mismatched value type;
      malformed literal cases are included in the backend IR matrix.
- [x] Phase B range validation now ties lower/upper bound presence, inclusive
      metadata, payload types, and bound projections to `RangeTyKind` before
      LLVM constructs or extracts range structs. Malformed full, one-sided,
      two-sided, and projection cases extend the backend IR matrix.
- [x] Phase B builtin-value validation now requires `usize` results for target
      layout and field-offset constants, representable layout operands, valid
      aggregate field ownership, and integer-like const-eval results before
      LLVM chooses the result integer type.
- [x] Phase B promoted static-array pointers now validate their array payload,
      complete-array pointee type, readonly metadata, and origin module before
      LLVM publishes promoted storage; malformed producer cases cover each
      independent metadata mismatch.
- [x] Phase B global and generic-global-instance expressions now validate their
      typed view against published storage metadata, including only the
      source-approved mutable-to-readonly qualifier coercion, before LLVM loads
      the selected global.
- [x] Phase B place validation now derives each addressable path from its real
      local/global/deref base, checks pointer and integer-index preconditions,
      and verifies final projection plus address-of result types before LLVM
      forms typed GEPs or loads.
- [x] Phase B assignment validation now requires a unit result, an exactly
      typed writable local/global/deref target, and plain or compound RHS
      contracts matching the store and builtin binary operation LLVM emits.
- [x] Phase B closure-view validation now ties callable state pointers and
      non-capturing function-pointer adapters to the owner-qualified generated
      closure entry, checking identity, mutability direction, and full ABI
      signatures before LLVM materializes either view.
- [x] Phase B index-expression validation now rejects non-integer indices and
      non-indexable bases before LLVM computes element addresses, while still
      checking the selected element result type for valid arrays, pointers, and
      slices.
- [x] Phase B trait-object expression validation now checks upcast/coercion
      source and result identities, readonly direction, concrete data-element
      types, and required owner-indexed vtables before LLVM rewrites fat-pointer
      metadata.
- [x] Phase B effect-expression validation now requires unit-typed discards and
      rejects residual `Try` expressions that function lowering failed to
      rewrite into explicit CFG terminators before backend consumption.
- [x] Cross-cutting Function IR Rustdoc now records flat CFG/local/scope
      ownership, closure-entry ABI tables, promoted union relocation
      invariants, and inline-assembly operand ordering at the shared producer
      and backend consumer boundary.
- [x] Phase B layout fat-pointer sizing now uses checked target arithmetic and
      rejects zero alignment or `pointer_size * 2` overflow instead of wrapping
      a malformed target description; helper-level boundary tests cover both
      failure modes.
- [x] Phase B extern ABI checking recursively validates fields of imported
      extern structs, closing the cross-module path that previously accepted a
      nominally extern aggregate without checking nested bool/Nia-only types;
      an imported-struct regression test covers the boundary.
- [x] Phase B LLVM aggregate, inline-assembly, and parameter-slot lowering now
      use checked `usize` to `u32` conversions for tuple/field indices and
      extracted outputs, returning diagnostics instead of truncating malformed
      backend metadata.
- [x] Phase B function optimization now treats nested defer exits as structural
      edges of their enclosing CFG, protects address-observable local stores
      from alias-unsound overwrite elimination, and retains function
      relocations rooted by generic local-static instances; owner regressions
      and an optimized defer LLVM consumer test cover these boundaries.
- [x] Phase B function optimization now documents its valid-IR/effect
      preservation contract and debug-validates the body before optimization
      and after every enabled pass, attributing structural invariant failures
      to their introducing pass without adding release-build overhead.
- [x] Phase B cross-function optimization now preserves dynamic receiver ABI
      metadata during trait-call devirtualization, derives inline substitution
      order from the backend parameter contract rather than incidental local
      table order, and shares the canonical `FunctionInstanceKey`; const-generic
      instance propagation has a two-key regression test.
- [x] Phase B generic function materialization now enforces its 4096-instance
      limit across the complete module fixed point, including instances from
      earlier discovery rounds, with saturating boundary coverage.
- [x] Phase B backend validation now rejects duplicate or invalid ABI parameter
      mappings for functions and closure entries before LLVM, while respecting
      the flat local table's nested-closure parameters; source and generic
      closure ABI materialization now share one owner helper and consumer
      coverage.
- [x] Phase B LLVM function-instance reference validation now uses the canonical
      `nia-function-ir::FunctionInstanceKey` for its cache, avoiding a second
      backend-only identity representation.
- [x] Phase B backend lowering and LLVM value lookup now use the canonical
      `FunctionInstanceKey` for function-instance deduplication and negative
      lookup caching; tuple aliases remain only where the identity has a
      deliberately different shape (aggregate layout or LLVM value context).
- [x] Phase B backend validation now distinguishes structural type descriptors
      from runtime slots and requires layouts for function/closure returns,
      expression and place values, body results, and callable signatures before
      LLVM classification; an opaque-return regression covers the boundary.
- [x] Phase B backend validation now derives fallback pointer/fat-pointer/vector
      layouts from the module target instead of LP64, rejects LLVM-inexpressible
      array lengths, and verifies static array/string/repeat initializer counts
      before allocation or constant construction; 32-bit and oversized-repeat
      regressions cover the boundary.
- [x] Phase B LLVM lowering now derives `usize`/`isize`, layout-builtin values,
      slice lengths, and memory-loop counters from the target pointer width
      rather than an LP64 constant; a 32-bit function ABI regression verifies
      both the signature and returned value width.
- [x] Phase B LLVM call argument materialization now rejects fixed-arity
      mismatches instead of silently truncating through iterator `zip`, while
      variadic function pointers preserve and emit all tail arguments in source
      order; an indirect variadic call regression covers the ABI path.
- [x] Phase B backend validation now verifies dynamic trait-call vtable slots
      against emitted method identity, including vtables used by supertrait
      upcasts; malformed-slot and valid-upcast regressions cover both sides of
      the pre-LLVM boundary.
- [x] Phase B backend validation now checks dynamic trait-call receiver,
      argument, result, parameter, and return contracts against the canonical
      function or function-instance signature selected by the vtable; malformed
      ABI metadata and the existing dynamic-dispatch matrix cover the boundary.
- [x] Phase B backend validation now checks direct, function-instance, method,
      callable-view, function-pointer, builtin-method, and builtin-operator call
      contracts before LLVM; generic method instances also preserve their named
      extern and variadic flags instead of swapping positional tuple fields.
- [x] Phase B closure-entry calls now resolve through the exact enclosing
      source/function-instance owner and validate generated state-pointer,
      parameter, and return ABI metadata before LLVM; missing-entry and malformed
      call regressions cover the owner boundary.
- [x] Phase B backend validation now checks builtin slice length/pointer and
      range-bound receiver/result contracts and rejects unresolved iterator
      builtins before LLVM instead of relying on emitter-side type failures.
- [x] Phase B LLVM emission APIs now document the validated backend boundary,
      incremental readiness protocol, partial-output diagnostic contract,
      optimization mapping, and attributable object-cache fingerprints.
- [x] Phase B backend lowering APIs now document the program-wide planning,
      unique-owner assignment, parallel module finalization, readiness
      publication, and deterministic collection contracts.
- [x] Phase B source-function lowering now verifies AST/signature parameter
      arity before pairing source metadata with ABI types, rejecting stale
      phase products instead of silently truncating either side through `zip`.
- [x] Phase B function-instance and function-local-static substitution now
      rebuilds separate type/const maps from declaration-kind metadata, so
      interleaved parameters and nested const-generic references retain exact
      argument order and concrete const identity through backend IR; extension
      methods use their effective impl/method generic ownership metadata.
- [x] Phase B trait-object vtables now retain const arguments in their explicit
      table and per-entry payloads, impl/default-method instance identities,
      fingerprints, and dependency membership; kind-aware supertrait expansion
      propagates interleaved type/const substitutions and keys cycle guards by
      the complete trait instance. Pre-LLVM validation and LLVM slot selection
      distinguish absolute source-object slots from relative upcast slots and
      disambiguate repeated const-generic supertraits. Driver and LLVM
      regressions cover impl, default-method, supertrait dispatch, and an
      upcast across two instances of the same const-generic supertrait.
- [x] Phase B backend validation now requires trait-object vtable slot metadata
      to equal physical entry order and validates every per-entry trait type and
      const-argument type, preventing malformed but otherwise unreferenced
      tables from reaching LLVM array emission.
- [x] Phase A/B type aliases now retain declaration-order generic kinds in
      signatures and persistent caches, then rebuild separate type/const
      substitutions through one documented owner helper. Normalization and
      layout regressions cover interleaved type/const aliases end to end.
- [x] Phase B extern ABI checking now expands local and imported const-generic
      aliases with the canonical type/const substitution mapping before
      classifying nested fields, so diagnostics describe the forbidden concrete
      representation instead of rejecting valid alias argument structure;
      imported generic extern-struct fields are likewise instantiated before
      recursive ABI classification.
- [x] Phase B layout-root closure now substitutes aggregate fields with the
      canonical recursive type/const algorithm; mixed generic structs and
      unions no longer skip all nested roots or retain symbolic array lengths.
- [x] Phase A explicit-supertrait validation now substitutes and matches the
      complete trait instance, including const arguments and their types;
      `Base[8]` can no longer satisfy the required parent of `Child[4]`.
- [x] Phase A trait-object coercion now selects impl/default methods with full
      type/const patterns and records const-bearing semantic instantiations;
      specialization and executable dependency facts retain the vtable trait
      instance instead of emitting empty const vectors.
- [x] Phase B backend trait-method owner assumptions now rebuild type and const
      arguments from declaration-order generic kinds; specialization ordering
      also rejects mismatched type/const arities instead of accepting prefixes.
- [x] Phase C const union scalar encoding now rejects zero-width,
      non-byte-aligned, and wider-than-128-bit integer ABI descriptors before
      shifts or fixed-buffer slicing; malformed scalar and vector lanes have
      direct evaluator regressions.
- [x] Phase C const integer comparison now orders mixed signed/unsigned values
      without narrowing large `u128` inputs, while typed bitwise-not uses
      overflow-free sign extension and rejects invalid width metadata.
- [x] Phase C closure summaries now preserve capture-slot identity within known
      closure states, so selecting one captured value cannot pull unrelated
      captures into return or escape facts; flattened cross-function closure
      states retain a documented conservative fallback.
- [x] Phase C closure call analysis now follows runtime callee/receiver-before-
      argument evaluation order, preventing callee-side assignments from
      hiding stack-backed arguments from escape summaries and diagnostics.
- [x] Phase C closure loop analysis now includes repeated `while` condition
      effects in the backedge fixed point, so condition-side provenance updates
      after the first iteration reach the body and exit environment.
- [x] Phase C closure type filtering now distinguishes active structural cycles
      from repeated sibling types, preventing value-only tuples and error
      unions from spuriously retaining callable or captured-address provenance.
- [x] Phase C closure defer analysis now delays registered expressions until
      scope exit and applies them in LIFO order after the tail value, matching
      Function IR and exposing later callable-state updates to deferred escapes.
- [x] Phase C closure return and error-propagation paths now replay all active
      lexical defers against an isolated exit environment before recording the
      exit, so later fallthrough overwrites cannot hide deferred escapes.
- [x] Phase C closure assignment analysis now evaluates dereference/index place
      effects before the RHS, matching Function IR and preventing destination
      expressions from hiding the callable provenance actually stored.
- [x] Phase D compiler check/emit action-cache metadata now shares a 64 MiB
      read/write bound; oversized entries are detected from file metadata,
      retired under the mutation lock, and never read into unbounded buffers.
- [x] Phase D output-transaction recovery now enforces its journal limit on the
      opened byte stream as well as pre-read metadata, closing the file-growth
      race that could turn a nominally bounded recovery read into `fs::read`.
- [x] Phase D exact-key generated-file cache reads now derive their byte budget
      from the identity's payload length, enforce it across lookup, collision,
      and retirement paths, and reject publication payloads inconsistent with
      the identity.
- [x] Phase D generated-file invalidation scans now validate variable-size
      records with a fixed streaming buffer, bound persisted identity fields by
      the canonical build-plan string limit, and materialize payload bytes only
      for an exact entry published after the direct miss; corruption retirement
      revalidates under the mutation lock without an unbounded reread.
- [x] Phase D regular-file external-command tool/input identities now stream
      fingerprints from one opened handle and reject length growth or truncation;
      Unix declared inputs also use no-follow opens.
- [x] Phase D external-command directory identities now plan only sorted entry
      metadata, then stream the registered recursive byte format through one
      fixed buffer. File payloads and nested encodings no longer accumulate in
      memory, and a compatibility regression proves cache-key parity.
- [x] Phase D staged external-command output snapshots now open each regular
      file once and enforce the observed length on the byte stream, closing the
      metadata/growth race. The later streaming publication path retains these
      handles and their stable checksums instead of buffering output bytes.
- [x] Phase D generated-file publication now compares an existing destination
      through one no-follow handle, an exact expected-length budget, and a fixed
      buffer; unchanged-output detection no longer uses an unbounded `fs::read`.
- [x] Phase D external-command invalidation scans now bound identity metadata
      by canonical plan limits, checksum arbitrarily large output payloads with
      one fixed buffer, and retain only candidate fingerprints. Corrupt records
      are revalidated by the same streaming parser under the mutation lock so a
      concurrent valid replacement is not retired.
- [x] Phase D exact external-command cache hits now retain an opened entry plus
      bounded output offsets instead of materializing payloads. Restoration and
      immutable-output comparison stream from that handle with checksum
      revalidation, and exact corruption retirement no longer rereads the whole
      record under the mutation lock.
- [x] Phase D external-command cache publication now retains opened staged
      output handles with stable length/checksum identities, writes the existing
      envelope directly to its staged file, and compares publication collisions
      byte-for-byte through fixed buffers. Output snapshots and whole-record
      encoding/collision allocations are no longer proportional to payload size.
- [x] Phase D executable static-archive inputs now derive both compiler-emit
      and linker cache identities from one no-follow regular-file handle using
      fixed buffers and exact observed-length checks. Link orchestration no
      longer retains every archive in memory, and compatibility regressions
      preserve both registered fingerprint domains.
- [x] Phase D native object work-product records now parse bounded canonical
      keys and stream payload validation, invalidation scans, publication, and
      collision comparison through fixed buffers without materializing the
      complete cache envelope. Per-entry mutation locks preserve an immutable
      valid winner and retire corrupt observations only while identical bytes
      remain installed; exact hits allocate only the payload required by the
      codegen cache interface.
- [x] Phase D static-archive cache capture, publication, invalidation, collision
      comparison, and restoration now stream arbitrarily large payloads through
      fixed buffers from stable opened handles. The existing v1 record format
      is preserved, checksum failures cannot install partial outputs, and
      per-entry mutation locks prevent Unix rename replacement or stale-reader
      retirement from discarding a valid immutable winner.
- [x] Phase D executable link-result caching now applies the same fixed-buffer
      capture, validation, invalidation, publication-collision, and stale
      retirement protocol while preserving the v3 envelope and v2 checksum
      domain. Cache hits stream directly into an atomic staged executable,
      verify integrity before installation, and restore executable permissions
      without coordinator allocations proportional to the binary size.
- [x] Phase D static-archive orchestration now streams the archive tool's output
      into a sibling staged destination with exact opened-length validation and
      atomic installation. The Driver no longer materializes the temporary
      archive with `fs::read`, and missing, growing, or truncated tool outputs
      leave an existing destination untouched.
- [x] Phase B backend function and concrete function-instance values now carry
      the exact published source-level function-pointer signature. Validation
      rejects non-pointer result types plus parameter, return, and variadic
      mismatches before LLVM materializes the referenced symbol.
- [x] Phase B inline assembly now validates its unit effect result, scalar
      operand boundary, canonical constraints and clobbers, plus exact writable
      output storage before constructing an LLVM inline-assembly call.
- [x] Phase B local value expressions now resolve their body-local storage type
      and reject unrelated or readonly-to-mutable views while preserving the
      source-approved mutable-to-readonly pointer, volatile-pointer, and slice
      coercions represented directly in function IR.
- [x] Phase B struct and union literals now bind their definition identity and
      aggregate kind to the nominal expression type. Struct literals additionally
      require every declared field exactly once, preventing LLVM from loading a
      partially initialized aggregate allocation.
- [x] Phase B switch terminators now admit only directly representable constant
      integer patterns and reject duplicate target-width bit patterns before
      LLVM. Checked negative integer patterns retain their lowering-generated
      signed spelling through codegen, including the `i128::MIN` boundary.
- [x] Phase B function-expression literals now validate their decoded payloads
      as well as their types: integer spellings and target-width ranges, finite
      float ranges, byte-character syntax, and Unicode scalar invariants are
      rejected before LLVM constant construction can fail or truncate them.
- [x] Phase B static initializers now validate scalar variant/type identity,
      finite float and Unicode scalar payloads, typed char/byte arrays, and null
      pointer destinations before module-level LLVM constant emission. Address
      initializers also require pointer-like destination types before relocation
      lookup and path validation.
- [x] Phase B static function addresses now require an exact source-level
      function-pointer signature, including semantic parameter types, return
      type, and variadic status, before LLVM can erase mismatches with a
      constant pointer bitcast.
- [x] Phase B static global addresses now require data-pointer destinations
      whose pointee type matches the selected global/path and whose mutability
      cannot expose immutable storage as writable.
- [x] Phase B static aggregate initializers now require every struct field
      exactly once and exactly one declared union field, preventing missing or
      duplicate fields from being hidden by layout-ordered constant emission.
- [x] Phase B LLVM inline-assembly and shared basic type/value wrappers now
      reject null FFI results before constructing typed handles or querying
      their LLVM kind. Builder memory, call, and control-flow operations apply
      the same check before typed-handle construction or follow-up calls such
      as switch-case insertion. Atomic, arithmetic, conversion, and fence
      builders now use the same guard, so fallible wrapper APIs produce
      diagnostics instead of panicking or passing null back into LLVM.
- [x] Phase B LLVM enum and string attribute construction now rejects null
      handles at creation, and function declaration propagates attribute
      failures before an invalid handle can reach `LLVMAddAttributeAtIndex`.
- [x] Phase B LLVM debug-info builder creation now propagates a null DIBuilder
      result through `LlvmResult`, preventing metadata calls from starting with
      an invalid builder handle.
- [x] Phase B LLVM debug metadata factories and declare insertion now validate
      null metadata/instruction returns before constructing typed handles.
- [x] Phase C function-IR scope unwinding now rejects control-flow edges whose
      source and destination have unrelated root scopes, while preserving
      child/ancestor and function-return defer ordering.
- [x] Phase B LLVM context/module allocation wrappers now check module,
      function, global, and basic-block handles before typed construction;
      null-result helper tests and owner-level error contracts prevent FFI
      allocation failures from becoming wrapper-constructor panics.
- [x] Phase B LLVM function return-type inspection now rejects a null type
      before querying its kind, with a shared classifier and regression test
      covering the fallible wrapper boundary.
- [x] Phase B LLVM call-result classification now turns a null `LLVMTypeOf`
      result into the call site's existing fallible value channel before kind
      inspection, with a focused malformed-FFI regression test.
- [x] Phase B LLVM target lookup and object emission now dispose optional
      owned diagnostic messages on every successful and null-result path, so
      future C API behavior cannot leak messages outside the wrapper boundary.
- [x] Phase B Windows LLVM IR printing now removes partially written temporary
      output on failure and disposes an unexpected owned success message,
      closing both filesystem and C allocation ownership paths.
- [x] Cross-cutting LLVM Rustdoc now documents the public error/handle traits,
      predicates, atomic/linkage controls, context ownership, module transfer
      and linking contracts, target identities, object emission, and facade
      boundaries. `cargo rustdoc -p nia-llvm -- -W missing-docs` reduced the
      crate baseline from 382 warnings to 249 without suppressing the lint;
      typed value and builder method coverage remains open.
- [x] Cross-cutting LLVM typed-handle Rustdoc now identifies every generated
      type/value wrapper, shared enum variant, metadata alias, and the
      function/global/block/instruction/phi/call ownership views. The same
      unsuppressed missing-docs audit fell from 249 warnings to 183; primitive
      type/value operations and builder methods remain open.
- [x] Phase B LLVM module-flag attachment now validates value-to-metadata
      conversion before calling the infallible attachment API, returns the
      failure through `LlvmResult`, and has focused null-metadata coverage plus
      Rustdoc for the debug-version and flag ownership contracts.
- [x] Cross-cutting LLVM primitive type/value Rustdoc now covers function and
      aggregate construction, constant and undef semantics, typed extraction,
      shape queries, and GEP/value views. The unsuppressed missing-docs audit
      fell from 181 warnings to 95, all now isolated to the builder API.
- [x] Cross-cutting LLVM builder Rustdoc now covers insertion/debug state,
      memory and CFG operations, unsafe GEP contracts, atomic orderings,
      signed/unsigned arithmetic, scalar/vector operations, and conversions.
      `cargo rustdoc -p nia-llvm -- -D missing-docs` is now clean, reducing the
      crate's audited baseline from 382 warnings to zero without suppression.
- [x] Cross-cutting backend-IR Rustdoc now documents the program/module views,
      write-once concurrent module store, single-consumer readiness protocol,
      and preplanned owner directory invariants. Its unsuppressed missing-docs
      baseline fell from 279 warnings to 244; partition and payload schemas
      remain open, while `nia-backend-lower` is already clean.
- [x] Phase B backend partition Rustdoc now explains transient versus stable
      unit identities, canonical dependency and linker ordering, deterministic
      four-bucket splitting, and closure/vtable ownership. Existing invariant
      tests cover allocation-independent keys, duplicate rejection, stable
      membership, and malformed linker order; the unsuppressed backend-IR
      missing-docs baseline fell from 244 warnings to 212.
- [x] Cross-cutting backend-IR payload Rustdoc now documents module ownership,
      layout snapshots, closure-entry ABI, generic substitutions, static
      payloads, function parameters, and trait-object dispatch records. The
      crate's strict `missing-docs` audit is now clean; the remaining backend-IR
      schema review is limited to any newly introduced public types.
- [x] Phase C function-IR core Rustdoc now documents stable block/scope
      identities, local and defer ownership, terminator edge contracts,
      propagation tags, expression typing, reference identity keys, and scope
      unwinding APIs. Function-IR tests (15) and strict Clippy pass; its
      unsuppressed missing-docs baseline fell from 402 to 223, with the large
      expression/callee payload surface remaining for follow-up batches.
- [x] Phase C function-IR expression Rustdoc now documents aggregate and
      relocation payloads, atomic ordering, inline assembly, trait-object and
      closure coercions, callee ABI fields, and typed place projections.
      `cargo rustdoc -p nia-function-ir -- -D missing-docs` is clean; the same
      15 validation/reference tests and strict Clippy remain green.
- [x] Phase C function-lower public boundary Rustdoc now documents typed-store
      ownership, lowering diagnostics, closure extraction, and the validation
      contract of `lower_function_body`. Its strict `missing-docs` audit is
      clean; 55 lowering/validation/scope/match/loop tests and strict Clippy
      pass.
- [x] Phase C function-opt CFG/purity audit now documents the conservative
      discardability lattice, fixed-point empty-jump merging, and reachability
      retention for defer-only control-flow references. Existing optimization
      matrices cover effect preservation, scope boundaries, nested defers, and
      policy ordering; 54 tests, strict Clippy, and `-D missing-docs` pass.
- [x] Phase C body-IR foundation Rustdoc now documents module/body ownership,
      locals, statement/control-flow records, loop and match schemas, pattern
      constructors, and typed expression identity. The unsuppressed body-IR
      missing-docs baseline fell from 436 to 287 warnings; the expression
      payload, call, memory, atomic, and place schemas remain for follow-up
      batches.
- [x] Phase C body-IR expression Rustdoc now documents aggregate construction,
      closure captures, try conversion metadata, union relocations, slices,
      ranges, trait-object coercions, and nested control-flow expressions. The
      unsuppressed body-IR missing-docs baseline fell from 287 to 166 warnings;
      call, memory, atomic, and place payloads remain for the next batch.
- [x] Phase C body-IR call/memory/place Rustdoc now documents builtin queries,
      inline assembly operands, memory intrinsic overlap contracts, atomic
      ordering, resolved callee identities, and typed place projections.
      `cargo rustdoc -p nia-body-ir -- -D missing-docs` is clean and body-ir
      Clippy/test checks pass.
- [x] Phase C static-IR Rustdoc now documents target-independent initializer
      variants, aggregate field identity, address paths, and precise reference
      extraction semantics for zero-length repeats and generic function
      addresses. `nia-static-ir` strict Rustdoc, 3 tests, and Clippy pass.
- [x] Phase C closure-check provenance audit now documents the finite summary
      lattice, origin transfer rules, known/unknown call handling, and isolated
      defer exit environments. Existing recursive, loop, capture, assignment,
      and LIFO-defer matrices remain green (8 tests), with strict Clippy and
      Rustdoc clean.
- [x] Phase C static-check boundary audit now documents active-revision
      filtering, static-vs-const representation rules, cross-module lookup
      ownership, and shared const-evaluation step budgeting. The existing 10
      static initializer acceptance/rejection tests, strict Clippy, and
      Rustdoc checks pass.
- [x] Phase C const-eval budget audit now documents outer-session reset and
      nested-session sharing, charge-before-execution semantics, recursion
      rejection without state mutation, balanced cleanup, and the interpreter
      loop cap. All 39 const-eval tests and strict Clippy pass; its unsuppressed
      missing-docs baseline is 161 warnings for follow-up schema batches.
- [x] Phase C const-check query and cache boundary audit now documents the
      monotonic array-length, enum, value, and typed-fact phase pipeline,
      cross-module provider ownership, execution-frame inputs, and cached
      result schemas. All 34 const-check tests, strict Clippy, and strict
      Rustdoc pass.
- [x] Phase C const-eval value and storage Rustdoc now documents allocation and
      frozen-pointer identity, live place projections, target ABI schemas,
      initialization masks, exact pointer relocations, and union write
      preservation rules. The unsuppressed missing-docs baseline fell from 161
      warnings to 59; all 39 tests and strict Clippy pass.
- [x] Phase C const-eval environment and execution-boundary Rustdoc now covers
      session hooks, lexical and function frames, early versus resolved name
      authority, typed preparation hooks, iterator/call dispatch, projected
      place writeback, and literal decoding. `nia-const-eval` strict Rustdoc is
      now clean; all 39 tests and strict Clippy continue to pass.
- [x] Phase C flow-check public boundary Rustdoc now documents active-revision
      and reachability filtering, syntax-versus-typed exhaustiveness ownership,
      conservative loop fallthrough, and continued diagnostics inside
      unreachable syntax. Its 14 control-flow/pattern tests, strict Clippy, and
      strict Rustdoc pass.
- [x] Phase C const-IR resolved-flow and expression Rustdoc now documents
      assignment paths, iterator/match payloads, pattern constructors, resolved
      expression variants, range/slice bounds, array repeat forms, and field
      initializers. Its unsuppressed missing-docs baseline fell from 547 to 349
      warnings; all 15 tests and strict Clippy pass, while type-argument and
      early-IR schemas remain.
- [x] Phase C const-IR early/resolved identity schema Rustdoc now documents
      early names, optional semantic ids, generic and associated targets,
      operator/range payloads, lowering input providers, and all public early
      statement/pattern fields. `nia-const-ir` strict Rustdoc is now clean;
      its 15 tests and strict Clippy pass.
- [x] Phase C body-check input/product boundary audit now documents cached const
      phase views, program providers, reachability/product filters, local and
      program signature scopes, diagnostic ownership, and incremental reuse.
      Pattern-matrix adapters now explain conservative opaque fallback and
      nominal rest-field expansion; provider revision/request APIs are now
      documented as well. `nia-body-check` strict Rustdoc is clean; its 230
      tests, strict Clippy, and format checks pass.
- [x] Phase C typed Body IR lexical-body walking now has direct preorder
      regressions for statement loops, block/if/if-pattern branches, expressions
      nested in patterns, and all three match-arm body forms. Closure captures
      remain in the enclosing function walk while closure bodies are explicitly
      excluded as separate function boundaries.
- [x] Phase C function-lowering input validation now checks `for` item patterns
      before consuming the iterator/body, closing the sole statement-path gap
      where a nested recovery expression could bypass malformed Body IR
      rejection and reach pattern lowering.
- [x] Phase C closure discovery and provenance analysis now traverse expression
      and range patterns across bindings, loops, if-patterns, and match arms;
      nested closures in those runtime-evaluated pattern operands no longer
      degrade to unknown-call summaries.
- [x] Phase C executable reachability now includes `for` item patterns when
      collecting function/global references, closing the last statement-level
      omission alongside existing binding, if-pattern, and match pattern
      traversal.
- [x] Phase C executable-facts Rustdoc now documents typed-IR versus semantic-
      fact authority, per-item merge schemas, trait method/vtable instance
      identity, sparse/dense collection strategy, and semantic-fact filtering
      ownership. Strict Rustdoc is clean.
- [x] Phase C executable-reachability input/result contracts now document root
      ownership, parse-valid module filtering, lazy signature callbacks,
      type-only module retention, per-module projections, and fixed-point body
      statistics.
- [x] Phase C reachability owner-state regressions now prove item insertion
      retains owner modules, pending modules enqueue once, and per-module body
      projection excludes type-only owners and compile-time-only globals.
- [x] Phase C semantic-fact filtering now has an owner matrix proving only
      requested function facts and global type entries survive reachability
      projection; node-wide facts remain intentionally preserved.
- [x] Phase C typed-body walking now directly covers bodies hidden in place
      dereference/index projections, expression callees, inline-asm operands,
      union relocations, memory intrinsics, and atomic compare-exchange
      operands, with strict preorder assertions.
- [x] Phase C executable-facts reachability now has a matching hidden-container
      matrix proving function/global references survive assignment places,
      function-pointer callees, inline asm, union relocations, memory intrinsics,
      and atomic operands.
- [x] Phase C function-lowering malformed-input coverage now rejects recovery
      expressions hidden in places, function-pointer callees, inline-asm input
      and output operands, union relocations, memory destinations, and atomic
      pointers before Function IR construction.
- [x] Phase C flow-check boundary coverage now traverses return-value
      expressions before termination and resets loop-control scope across
      closure bodies, preventing hidden diagnostics and cross-function
      `break`/`continue` targets.
- [x] Phase C flow reachability now propagates termination through eager
      conditions and expression operands while preserving a fallthrough path
      for short-circuit logical RHS operands; a container matrix covers calls,
      unary/binary operators, `if`, and `while` conditions.
- [x] Phase C function-tail flow now uses the normal expression traversal and
      caches block/statement outcomes for return-value analysis, exposing
      diagnostics in tail calls, closure bodies, and deferred block tails
      without reporting nested match diagnostics twice.
- [x] Phase C static checking and lowering now preserve negative floating-point
      literals as backend-representable `StaticInit::Float` values instead of
      routing every unary negation through integer const evaluation; an LLVM
      integration regression covers both `f32` and `f64` globals.
- [x] Phase C consecutive closure expressions now have parser and LLVM
      regressions proving right-nested currying and nested closure-state
      returns. Local resolution enforces explicit capture boundaries before
      Body IR, replacing late missing-local backend failures with source-level
      diagnostics when an inner closure omits an outer local from its captures.
- [x] Phase C method receivers now obey the same explicit closure boundary as
      named locals; direct `self` uses diagnose the required bind-and-capture
      form before Body IR instead of reaching backend entries as missing locals.
- [x] Phase C function-lowering input validation now checks closure-state
      identity, capture and parameter arity/types, return ABI, distinct slot
      locals, and direct closure-callee shape before entry extraction. Malformed
      typed products return diagnostics instead of reaching closure lowering's
      assertions; Body IR Rustdoc records capture-alias versus body-local
      ownership.
- [x] Phase C closure entry extraction now rejects duplicate `ClosureId`
      definitions within one typed function body instead of silently retaining
      the first definition through the lowering entry cache.
- [x] Phase B Function IR closure-entry validation now requires the body result
      type to equal the declared ABI return and requires `state_param` plus
      `params` to enumerate every parameter local exactly once, preventing
      cached or hand-built IR from exposing an uninitialized hidden parameter.
- [x] Phase B function lowering now removes nested closure aliases and
      parameters from the enclosing entry's flat local table. Curried closures
      retain separate ABI-owned local tables instead of presenting inner
      parameters as uninitialized storage in the outer generated function.
- [x] Phase B backend validation now independently requires closure state types
      to match the entry identity and callable signature and rejects unmapped
      closure-body parameter locals, covering direct/cached Backend IR inputs
      that do not pass through function lowering.
- [x] Phase B closure-entry keys now validate that source and instantiated
      owners match the source `ClosureId` and resolve to an emitted backend
      function in the same published BackendModule before owner-relative calls
      consume them. Partition planning keeps dangling cached owners
      deterministic so validation can diagnose them instead of panicking at
      the pre-LLVM boundary.
- [x] Phase B closure-entry symbols are checked against deterministic
      owner-derived mangling before LLVM declaration, preventing cached IR from
      aliasing an unrelated function name while retaining a valid entry key.
- [x] Phase A generic inference now applies the same callable mutability
      relation as coercion: mutable closure state may infer a readonly
      `&Fn(T) T` target, while readonly state cannot satisfy a mutable target.
- [x] Phase A inferred closure, callable, and tuple signatures now require
      matching arity before contributing generic substitutions, preventing
      malformed closures from partially selecting a concrete instance before
      the authoritative argument check reports the structural mismatch.
- [x] Phase A closure-signature inference now preserves the `&` versus `&mut`
      address operator when matching canonical callable views, so readonly
      closure addresses cannot seed substitutions for mutable `&mut Fn`
      candidates before authoritative argument checking.
- [x] Phase A closure-signature inference now probes the complete recursive
      type shape before committing substitutions; nested tuple/callable
      mismatches cannot leak a valid prefix into later arguments. Function,
      method-candidate, and thin-function-pointer regressions cover the shared
      inference path.
- [x] Phase A transactional closure inference now mirrors the ordinary generic
      inferencer across ranges, nominal/builtin-trait types, trait objects and
      associated bindings, and projections. Range and generic trait-object
      regressions ensure the safety probe does not suppress valid inference.
- [x] Phase B codegen validation rejects duplicate concrete closure-entry keys
      before ProgramIndex overwrite semantics can alias multiple definitions
      onto one generated LLVM entry identity.
- [x] Phase A const-generic call inference now traverses arrays, pointer/slice
      families, tuples, optionals, error unions, ranges, callable forms,
      nominal/builtin-trait types, trait objects and associated bindings, and
      projections. A complete compatibility probe precedes staged inference,
      so an incompatible outer or later-nested shape cannot leak a partial
      const substitution into instance selection.
- [x] Phase A normalized type matching now compares tuple, optional, and error-
      union structure recursively instead of requiring equivalent reconstructed
      types to retain the same interned identity. Nested const substitutions
      cover the independently interned tuple path.
- [x] Phase A structural type equivalence now covers reconstructed builtin
      traits, trait-object pointees, and closure states in body checking, while
      the cross-store `nia-ty` contract also covers const-only, vector, and
      closure-state variants. Associated bindings compare as unordered keyed
      sets, and a dual-store regression prevents intern identity from masking
      future variant omissions.
- [x] Phase A type-match cache invalidation now follows unevaluated const
      expressions through nominal/trait/projection const arguments, associated
      binding keys, and closure-state components, preventing an early negative
      comparison from surviving after its nested const value becomes available.
- [x] Phase A call inference now preserves recursive type-parameter discovery
      through mixed type/const nominal arguments, trait-object const argument
      types, associated-binding keys, and projection const argument types.
      Owner regressions cover nominal and associated-binding-key failures that
      previously stopped before recording an otherwise inferable type.
- [x] Phase A method/impl type-pattern matching now treats trait-object
      associated binding keys and values as one transactional candidate. A
      failed binding alternative cannot leak substitutions or prevent a later
      compatible binding from being selected; method argument matching covers
      multiple associated bindings with independent inferred type parameters.
- [x] Phase A trait-solver candidate filtering now matches each trait-object
      associated binding's key and value transactionally, preserving type and
      const substitutions while trying later compatible candidates.
- [x] Phase A `nia-ty` and executable-reachability trait-object equivalence now
      compares associated binding values as part of the key candidate, rather
      than rejecting a reordered binding set after selecting the first key-only
      match. Cross-store and reachability regressions cover the reordered case.
- [x] Phase B backend instance matching now traverses trait-object associated
      binding keys and values together, preserving extension-method type
      substitutions while trying compatible bindings in either trait-object
      form. The backend-lowering owner suite remains green after the fix.
- [x] Phase A body-check generic inference now matches associated binding keys
      and values transactionally for both type and const inference. A
      value-incompatible first candidate cannot seed substitutions or hide a
      later compatible binding; overlapping generic-key candidate regressions
      cover the generic call path.
- [x] Phase C const-execution type inference now probes associated binding
      keys and values with cloned type substitutions, retaining the first
      useful diagnostic only after every compatible candidate is exhausted.
      Const-evaluation trait-object inference covers distinct associated-
      binding instances with the same member name.
- [x] Phase A body-check and trait-solver structural equivalence now tests
      associated binding key and value together in the same candidate. Reordered
      bindings with overlapping keys cannot stop at a value-incompatible first
      match.
- [x] Phase A associated-binding equivalence now consumes each actual binding
      at most once. Body-check, trait-solver, type-store, and reachability
      comparisons reject mismatched duplicate-key multiplicities instead of
      reusing one successful candidate.
- [x] Cross-cutting Phase A type-resolution Rustdoc now documents the
      resolution product, name categories, provider context, and all public
      active-tree/module entry points. `nia-type-resolve` strict Rustdoc,
      seven owner tests, and strict Clippy pass.
- [x] Cross-cutting Phase A type-lowering Rustdoc now documents canonical
      storage, const-expression preservation, provider context, versioned type
      uses, and declaration-versus-full lowering entry points. `nia-type-lower`
      strict Rustdoc, 17 owner tests, and strict Clippy pass.
- [x] Cross-cutting Phase A value-resolution Rustdoc now documents stable node
      products, value categories, associated-value providers, module context,
      and module/expression entry points. `nia-value-resolve` strict Rustdoc,
      four owner tests, and strict Clippy pass.
- [x] Cross-cutting Phase A local-resolution Rustdoc now documents stable
      local products, lexical binding categories, type-prefix identities,
      filtered active-tree context, and origin-aware entry points.
      `nia-local-resolve` strict Rustdoc, 17 owner tests, and strict Clippy pass.
- [x] Cross-cutting Phase A type-normalization Rustdoc now documents alias
      expansion inputs, normalized identity products, recursive-cycle
      diagnostics, and explicit-input normalization scope. `nia-type-normalize`
      strict Rustdoc, nine owner tests, and strict Clippy pass.
- [x] Cross-cutting Phase B ABI-check Rustdoc now documents imported and local
      signature-family views plus diagnostic ownership. `nia-abi-check` strict
      Rustdoc, six owner tests, and strict Clippy pass.
- [x] Cross-cutting Phase B layout Rustdoc now documents target-dependent
      aggregate schemas, concrete instance keys, cross-module providers, root
      selection, and checked ABI helper contracts. `nia-layout` strict Rustdoc,
      26 owner tests, and strict Clippy pass.
- [x] Cross-cutting Phase E target-configuration Rustdoc now documents host and
      explicit target identities, active-item-tree pruning, inactive-span
      preservation, symbol-aware condition evaluation, and diagnostic
      ownership. `nia-target-config` strict Rustdoc, two owner tests, and strict
      Clippy pass.
- [x] Cross-cutting Phase B mangling Rustdoc now documents stable module hashes,
      sanitized symbol components, base and concrete-instance names, and
      resolver-driven type/const encoding. `nia-mangle` strict Rustdoc, eight
      owner tests, and strict Clippy pass.
- [x] Cross-cutting Phase E symbol-table Rustdoc now documents stable content
      identities, collision rejection, known-symbol initialization, shared
      resolver views, and unresolved display fallback. `nia-symbol-table`
      strict Rustdoc, three owner tests, and strict Clippy pass.
- [x] Cross-cutting Phase E public-surface Rustdoc now documents exported
      surfaces, using-scope products, module-owned diagnostics, and the stable
      type-exposure reverse index. `nia-public-surface` strict Rustdoc, three
      owner tests, and strict Clippy pass.
- [x] Cross-cutting Phase C literal-decoding Rustdoc now documents numeric
      suffix/radix evaluation, Unicode and byte character decoding, string
      concatenation, and checked literal-length contracts. `nia-literals`
      strict Rustdoc, five owner tests, and strict Clippy pass.
- [x] Cross-cutting Phase C IR-name Rustdoc now documents promoted-allocation
      identities, receiver/source/generated/temporary local categories, and
      stable internal storage names. `nia-ir-names` strict Rustdoc and strict
      Clippy pass; the owner contains no unit tests.
- [x] Cross-cutting Phase A provider-summary Rustdoc now documents extension
      target classification, conservative nominal candidates, deterministic
      provider indexes, associated-item queries, and facade visibility filters.
      `nia-provider-summary` strict Rustdoc, 11 owner tests, and strict Clippy
      pass.
- [x] Cross-cutting Phase D imports Rustdoc now documents reserved module roots,
      source/module stable identities, graph snapshots and handles, declaration
      resolution, visibility checks, and child-path derivation. `nia-imports`
      strict Rustdoc, five owner tests, and strict Clippy pass.
- [x] Cross-cutting infrastructure hash Rustdoc now documents the deterministic
      non-cryptographic hasher and its map/set adapters. `nia-hash` strict
      Rustdoc and strict Clippy pass; the owner contains no unit tests.
- [x] Cross-cutting infrastructure span Rustdoc now defines half-open source
      ranges, saturating length, and empty-range semantics. `nia-span` strict
      Rustdoc and strict Clippy pass; the owner contains no unit tests.
- [x] Cross-cutting Phase D source Rustdoc now documents relocation-stable
      logical identities versus physical paths, session-local ids, monotonic
      revisions, exact-version lookup, and concurrent current-snapshot storage.
      `nia-source` strict Rustdoc, nine owner tests, and strict Clippy pass.
- [x] Cross-cutting Phase E item-tree Rustdoc now documents conditional item
      selection, inactive ranges, versioned node identity, declaration versus
      shallow-definition comparison, signature-consumer filtering, and AST
      projection. `nia-item-tree` strict Rustdoc, ten owner tests, and strict
      Clippy pass.
- [x] Cross-cutting Phase E node-identity Rustdoc now documents versioned
      span/child-path locators, store-scoped compact handles, revision
      retirement, locator-based map equality/merge, and transactional AST
      origin tables. `nia-node-id` strict Rustdoc, 18 owner tests, and strict
      Clippy pass.
- [x] Cross-cutting Phase D query Rustdoc now documents declarative provider,
      storage, fingerprint, and registry contracts; shared versus owned access;
      red/green validation; cross-database sessions; completion-order streams;
      quiescent retirement; bounded task execution; and process-wide LLVM
      memory permits. `nia-query` strict Rustdoc, 81 owner tests, and strict
      Clippy pass.
- [x] Cross-cutting Phase D toolchain Rustdoc now documents relocatable resource
      discovery, versioned manifest compatibility, path-independent identity
      fingerprints, host versus artifact targets, required standard-library and
      runtime resources, and structured discovery failures. `nia-toolchain`
      strict Rustdoc, four owner tests, and strict Clippy pass.
- [x] Cross-cutting Phase D compatibility-registry Rustdoc now documents the
      central ownership of generated ABI versions, toolchain schemas, persisted
      payload magic/schema identities, versioned cache namespaces, build output
      recovery formats, and compiler/link/archive work products. `nia-compat`
      strict Rustdoc, three owner tests, and strict Clippy pass.
- [x] Phase D link/archive result identity now has explicit contracts for
      logical versus physical inputs, component fingerprints, conservative
      cache disabling for opaque external inputs, linker/archive-tool bytes,
      target and toolchain identity, deterministic options, and exact stale
      attribution. Driver cache lookup regressions exercise every input,
      toolchain, target, linker/tool, and option component. `nia-linker` strict
      Rustdoc and 26 owner tests plus all 635 `nia-driver` owner tests and strict
      Clippy pass.
- [x] Cross-cutting Phase D loader Rustdoc now documents source-manifest
      identity, session sharing, source revision retirement, provider-demand
      updates, persistent frontend cache verification, and target/runtime
      request boundaries. The frontend cache owner suite also fixes the
      namespace-root regression and retains bounded corruption retirement;
      `nia-loader-query` strict Rustdoc, 94 owner tests, and strict Clippy pass.
- [x] Cross-cutting Phase D build Rustdoc now documents runner bootstrap and
      invocation ownership, logical plan identities, semantic freeze and
      canonical codec boundaries, action-cache outcomes, deterministic
      scheduling, atomic plan handoff, and recoverable output publication.
      The existing boundary matrix covers bounded/corrupt cache records,
      truncation and trailing bytes, cross-process cache publication, stale
      locks, cancellation, process groups, and interrupted transactions;
      `nia-build` strict Rustdoc, its owner suite (211 passed, 1 ignored), and
      strict Clippy pass.
- [x] Cross-cutting Phase D compiler-query Rustdoc now documents loader fact
      ownership, provider-demand revision/fixed-point updates, compiler session
      compatibility, check/codegen entry points, completion-order backend
      finalization, source/module fingerprints, target/toolchain/runtime cache
      namespaces, product-specific cache keys, and diagnostic-store ownership.
      The 238 owner tests include randomized clean/incremental consistency plus
      persisted-product corruption and verification replacement; strict
      Rustdoc and strict Clippy pass.
- [x] Cross-cutting Phase D CLI crate documentation now records typed option
      translation, stdout/stderr and native-output ownership, toolchain/build
      dispatch, timing policy, and the process ICE boundary. Strict Rustdoc,
      three parser/ICE owner tests, the command-case matrix, and strict Clippy
      pass. The build-case matrix passed 13 cases but encountered a transient
      test-resource slot lock failure; its exact retry passed, while a clean
      whole-matrix rerun remains part of final resource evidence.
- [x] Phase D cross-process test-resource slot creation now waits through the
      brief interval in which a concurrent stale-slot reclaimer can lock the
      newly visible owner inode. The creator locks before writing its owner
      record, so normal contention is no longer reported as a fatal corrupt
      permit. A deterministic lock-order regression, all 18
      `nia-test-support` owner tests, strict Clippy, and the complete concurrent
      CLI build-case matrix (14 passed) cover the repaired boundary.
- [x] Phase D representative build baseline completed its default three
      independent samples on Linux/WSL2 with an 8.18 GB effective memory limit.
      All 21 ordered states and all 102 per-sample acceptance checks passed:
      clean and warm lookup cardinality, source and module-map typed
      invalidation, compiler-object and link reuse, corruption of five action
      entries, recovered warm reuse, and failed-action isolation. The generated
      schema-v3 evidence retained machine resources, process measurements, and
      every counter. The later schema-v5 and 3 GiB/no-swap workspace batches
      close the independent-recomputation and constrained-memory gaps recorded
      by this original run.
- [x] Phase D representative build baseline schema v5 now recomputes source-
      edited and module-map-edited fixtures in separate cold-cache workspaces,
      then compares both emitted executables byte-for-byte through fixed 64 KiB
      buffers. Its default three-sample run covered 27 ordered states and 135
      acceptance checks; all six incremental/clean comparisons matched both
      artifacts, every state had a distinct child-process PID, and the named
      `source-app` comparison records both `src/main.nia` and its mapped helper
      module. Focused acceptance and buffer-boundary tests preserve missing,
      mismatched, and falsely warm evidence. The complete 3 GiB/no-swap
      workspace run below supplies the constrained-memory evidence.
- [x] Phase D low-memory test scheduling now reuses an active compiler session's
      memory reservation for sequential emitted-runtime commands while retaining
      an independent runtime scheduling slot. The nested-session regression,
      `nia-test-support` owner suite (19 passed), strict Clippy, and focused CLI
      build cases pass; the complete constrained workspace run below verifies
      the same accounting under default libtest concurrency.
- [x] Backend dynamic-trait validation now accepts a runtime trait-object call
      when no concrete object vtable is materialized in the closed executable,
      while still validating every published candidate's slot and ABI. The
      standard-library debug error executable regression covers an empty
      formatter-object slice and its `IntoError` dynamic calls.
- [x] Backend place validation now preserves address-of and projection
      contracts for compiler-generated pointer-qualified place views. The
      process argument/iterator executable regressions that exercise indirect
      return values pass, alongside the complete 43-case process suite and
      strict Clippy for the affected crates.
- [x] Link-cache acceptance now counts persisted `.link` records separately
      from their durable per-record mutation locks. The complete linker case
      matrix passes in ordinary execution; the constrained workspace run also
      reached this matrix without resource failure before exposing a separate
      later standard-library validation regression.
- [x] Backend projection compatibility now applies readonly-view coercion
      recursively through nested pointer and slice elements. The standard
      `ArrayList` executable regression covering mutable/readonly iterator views
      passes in ordinary and 3 GiB/no-swap execution (5 passed, 822.5 MiB
      peak), preserving strict rejection of readonly-to-mutable misuse.
- [x] Phase D direct in-process `nia-driver` unit tests now retain the shared
      compiler resource session instead of bypassing it, while explicit case
      sessions are reused without double charging. Under 3 GiB/no-swap, the
      default-concurrency 635-test owner suite changed from an OOM at the exact
      3 GiB limit to 635/635 passing with a 1.1 GiB peak. The reentrant-session
      regression, 20 `nia-test-support` tests, workspace check, formatting, and
      strict Clippy for both owner crates pass.
- [x] The complete all-feature workspace test suite now passes with normal
      libtest concurrency under a 3 GiB cgroup memory limit and no swap. The
      single-job build/test run completed without an OOM and reached a 2.1 GiB
      peak, closing the constrained-workspace resource evidence that exposed
      and verified the nested-session and in-process-driver fixes above.
- [x] Phase D query-executor shutdown now closes admission before draining its
      queue and joining the worker set. A direct owner regression proves that
      dropping the final session handle completes every already accepted task;
      the stable architecture record now states this lifetime contract alongside
      nested execution and the process-wide CPU budget.
- [x] Phase D linker and archive-tool identity now streams each opened tool
      executable through a fixed 64 KiB buffer instead of allocating the full
      file. Declared-length streaming preserves the existing cache fingerprint,
      rejects truncation or growth, and retains conservative cache disabling for
      an unreadable linker. The 27 `nia-linker` owner tests, including a
      multi-buffer equivalence regression, formatting, and strict Clippy pass.
- [x] Phase D Linux host dynamic-linker discovery now reads the fixed ELF header
      and individual program headers by offset instead of allocating the entire
      host executable. Checked table arithmetic and a 4 KiB `PT_INTERP` limit
      reject truncated, out-of-file, overflowing, or oversized records. The 28
      `nia-linker` owner tests include valid and oversized interpreter fixtures;
      formatting and strict Clippy pass.
- [x] Phase D Linux default-library discovery now canonicalizes and visits each
      `ld.so.conf` input once, sorts include matches before recursion, and bounds
      individual/aggregate bytes, file count, and directory matches. Oversized
      files and include cycles cannot drive unbounded work or make link-result
      target identity depend on filesystem enumeration order. The 30
      `nia-linker` owner tests, formatting, and strict Clippy pass.
- [x] Phase D toolchain resource-manifest reads now enforce a 64 KiB stream
      budget before UTF-8 and field parsing. Metadata rejects known oversized
      files without allocation, while the `max + 1` read catches concurrent
      growth and prevents a valid compatibility prefix from being accepted.
      All five `nia-toolchain` owner tests, formatting, and strict Clippy pass.
- [x] Phase D filesystem source loading now has one 64 MiB stream budget owned
      by `nia-source` and shared by loader queries, CLI entry inspection, and
      diagnostic source recovery. Known oversized files are rejected before
      allocation, concurrent growth cannot hide behind a valid UTF-8 prefix,
      and rejected inputs are not installed in the source database. The ten
      `nia-source`, 94 `nia-loader-query`, and three CLI owner tests, affected
      strict Clippy, consumer checks, and formatting pass.
- [x] Phase D source identity normalization now preserves unresolved leading
      `..` components in relative paths while clamping absolute paths at their
      root. Parent-relative sources no longer read a different physical file or
      collide with the corresponding child-path identity. The focused matrix
      covers nested resolution, repeated parents, absolute roots, and distinct
      `SourcePath` identities, including actual parent/child file reads. All 12
      `nia-source` and 94 `nia-loader-query` tests, strict Clippy, workspace
      check, and formatting pass.
- [x] Phase D `ld.so.conf` canonical identities now serve only cycle/duplicate
      suppression; relative includes remain anchored to the visible config
      pathname, including when that file is a symlink. The Unix regression
      preserves deterministic traversal without silently changing established
      include resolution. All 31 `nia-linker` tests, strict Clippy, workspace
      check, and formatting pass.
- [x] Phase D persisted compiler signature products now have direct stale-
      identity matrices at their codec owner boundary. Valid envelopes for
      signature type resolution, signature type lowering, item signatures,
      extension validation diagnostics, executable value-reference edges, and
      check certificates reject every duplicated key, namespace, stable
      module/entry, program-source/input, signature-set, source-length, body-
      owner, and scope field independently. These tests complement each
      product's roundtrip/corruption coverage and the shared storage tests for
      atomic publication and replacement-preserving retirement. All 244
      `nia-compiler-query` owner tests pass.
- [x] Phase D loader dependency manifests now directly verify their complete
      persisted identity instead of relying only on derived cache paths. A
      valid checksummed record installed at the expected path is retired when
      its duplicated key, namespace, stable module, or source fingerprint is
      stale; the existing cross-session roundtrip, corruption recovery,
      semantic verification replacement, and shared concurrent replacement
      tests complete this product's persistence evidence. All 95
      `nia-loader-query` owner tests pass.
- [x] Phase D compiler-check and compiler-emit action caches now have direct
      owner tests for the complete corruption-replacement lifecycle. A corrupt
      exact record is retired and republished, while a delayed retirement using
      the old observed bytes preserves the newly valid entry. Compiler-emit
      additionally retains its explicit reference-binding replacement
      protocol; these tests complement canonical roundtrips, component-exact
      stale identity/invalidation matrices, and bounded oversized retirement.
      The stable maintenance rules now require publication and retirement to
      share a per-path mutation lock and revalidate the observed winner.
- [x] Phase D query/build concurrency evidence now closes two lifecycle gaps:
      deterministic parallel and cross-session cycle tests assert that the
      process-wide wait-for graph retains no edges or frames for participating
      nodes after failure, and a retirement-transaction panic test proves RAII reopens query
      admission for later work. Existing scheduler tests retain canonical
      failure-position cancellation, active-wave draining, and dependent
      suppression; query owner tests now cover 83 cases with the new lifecycle
      assertions.
- [x] Phase D representative clean/incremental evidence is now self-describing
      in build baseline schema v5. Every ordered state records the PID of its
      separately spawned `nia build` process, acceptance requires nine distinct
      state processes, and artifact comparisons separately require the
      multi-module `source-app` (entry plus mapped helper module) to match its
      independent cold-workspace recomputation. The existing source-edit and
      module-map-edit comparisons therefore provide explicit multi-module and
      cross-process evidence rather than relying on fixture inspection.
- [x] Phase D fixed acceptance is closed across all four required gates. The
      persisted-product inventory has roundtrip, corruption, complete stale-
      identity, and replacement coverage at its codec/cache owners; query and
      build lifecycle races have deterministic owner tests; schema-v5 baseline
      samples prove independent-process multi-module incremental/clean artifact
      equivalence; and the all-feature workspace passes at normal libtest
      concurrency under a 3 GiB/no-swap cgroup with a 2.1 GiB recorded peak.
      The same constrained command was rerun successfully on the schema-v5
      acceptance head, including 14/14 CLI build cases, 635/635 driver tests,
      and 83/83 query tests. Required owner crates also have strict
      Rustdoc/Clippy and focused boundary evidence recorded above; broader final
      validation remains tracked separately.
- [x] Phase A where-bound validation now preserves associated-type bindings
      through generic/method candidate filtering, final function and method
      call checks, and recursive nominal-type obligations. Concrete projection
      mismatches fail with binding-specific call diagnostics, while matching
      bindings remain accepted. Impl-based reverse inference additionally
      rejects phase products whose trait type-argument arity differs from the
      requested goal instead of accepting a truncated `zip`. Body-check owner
      regressions cover accepted and rejected bindings, nominal instantiations,
      and a deliberately malformed upstream impl signature; trait-solver owner
      tests and body-check strict Clippy remain green.
- [x] Phase A where-bound reverse inference now derives unresolved type
      parameters from the selected impl's associated type definitions, after
      substituting type and const parameters captured from the impl target.
      Target, trait arguments, const arguments, and all associated bindings are
      matched as one candidate, so a nested binding mismatch cannot leak an
      earlier partial substitution. Owner regressions cover successful
      `Source[Item = Item]` inference and rejection of `(Item, Item)` against
      `(i32, bool)`; all 252 body-check and 244 compiler-query tests plus strict
      body-check Clippy pass.
- [x] Phase A supertrait obligations now retain associated-type bindings from
      source collection through item-signature type roots, schema-v10 cache
      roundtrips, program-signature projection assumptions, body-check
      supertrait expansion, and explicit parent-impl validation. Default trait
      methods resolve a bound parent projection, an impl with the wrong parent
      associated type is rejected, and a clean/incremental edit matrix produces
      identical diagnostics. All 253 body-check, 245 compiler-query, and 636
      driver tests pass together with the owner/signature-cache suites, strict
      Clippy, and workspace checks.
- [x] Phase A body-check method inference and overload specialization now
      compare trait-object associated bindings as a complete unordered
      bijection. Candidate probes backtrack with isolated type/const
      substitutions, and one actual binding cannot satisfy multiple pattern
      bindings; obligation deduplication uses the same order-independent,
      single-consumption rule. All 253 body-check tests, 42 focused driver
      associated-type tests, and strict body-check Clippy pass.
- [x] Phase A trait-solver impl matching and ordinary-call shape filtering now
      apply the same associated-binding bijection rule. Impl selection
      backtracks transactionally to find a complete binding permutation and
      records only its winning generic substitutions; fast call filtering
      cannot reuse one actual binding to admit a malformed candidate. Direct
      solver regressions cover both backtracking and rejected reuse; all 11
      trait-solver and 253 body-check tests plus strict affected Clippy pass.
- [x] Phase A/B backend extension instantiation now preserves the frontend's
      associated-binding set semantics. Trait-object patterns backtrack over a
      complete one-to-one binding permutation before committing generic
      substitutions, and backend type equality cannot reuse the first matching
      key for duplicate bindings. All 112 backend-lower and 264 LLVM codegen
      owner/consumer tests plus strict backend-lower Clippy pass.
- [x] Phase A trait objects now inherit associated-type equalities declared by
      their supertrait graph. A bare `&Child` for
      `Child : Parent[Item = i32]` is object safe for parent projection methods
      and upcasts to `&Parent[Item = i32]`; a `bool` target and an unrelated
      parent binding remain rejected. The traversal substitutes child type and
      const arguments, is cycle guarded, and feeds object safety, dynamic call
      normalization, and upcast validation. Clean/incremental edits agree, and
      all 253 body-check, 246 compiler-query, 638 driver, and 265 LLVM codegen
      tests plus strict affected Clippy pass.
- [x] Phase A supertrait validation now expands the complete inherited
      associated-binding graph and rejects conflicting equalities for the same
      parent trait instance. Repeated constraints with the same right-hand side
      remain valid, including diamond inheritance; distinct right-hand sides
      produce a binding-specific diagnostic. Direct and diamond conflict
      regressions pass with 639 driver tests, 246 compiler-query tests, the
      workspace check, formatting, and strict affected Clippy.
- [x] Phase A trait-object supertrait reachability now uses a path-local cycle
      guard. DFS nodes are removed on every return, including missing-signature
      diagnostics, so a failed branch cannot suppress a later sibling path.
      Body-check (253), focused supertrait driver (23), workspace check, and
      strict body-check Clippy pass.
- [ ] Phase A: type, trait, and body soundness.
- [ ] Phase B: layout, ABI, backend IR, and LLVM safety.
- [ ] Phase C: const, static, closure, flow, and IR semantics.
- [x] Phase D: query, cache, loader, import, and build state.
- [ ] Phase E: frontend, identity, diagnostics, and structural boundaries.
- [ ] Phase F: standard library and runtime.
- [ ] Cross-cutting Rustdoc/comment completion and test-gap closure.
- [ ] Final clean/incremental, workspace, integration, resource, and external
      target evidence.

## 12. Roadmap Retirement

Delete this file only when:

1. Every phase has a committed implementation/test result or an explicitly
   recorded, accepted residual risk in the owning stable documentation.
2. Obsolete APIs, compatibility shims, duplicate identities, and temporary
   audit fixtures are removed.
3. Owner tests, affected consumer tests, workspace check, formatting, strict
   Clippy, relevant integration tests, and resource-controlled workloads pass.
4. Durable architectural rules and lessons have moved to `docs/architecture.md`,
   `docs/compiler-maintenance.md`, the relevant crate README, or `lib/README.md`.
5. A final audit confirms no phase is merely marked complete because it was
   recently touched by an unrelated commit.

The deletion itself must be a dedicated `docs:` commit after the final
acceptance batch; Git history retains this roadmap and its intermediate work.
